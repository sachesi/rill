//! Everything about one torrent: progress and a map of its pieces, transfer figures,
//! where it is saved, its files, peers and trackers. The window pushes each new snapshot
//! to it with [`TorrentInfoDialog::apply_update`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use mtorrent::utils::re_exports::mtorrent_base::input::{MagnetLink, Metainfo};

use super::file_item::FileItem;
use crate::engine::{PeerInfo, TorrentEngine, TorrentUiState, UiUpdate};
use crate::storage::Storage;
use crate::util::{format_eta, format_rate, format_size};

/// How long a change of the sequential switch wins over snapshots that still carry the
/// old value: the wait below, plus the time until the restarted torrent sends its first
/// snapshot, plus room to spare.
const SEQUENTIAL_GRACE: Duration = Duration::from_secs(4);

/// How long to leave a torrent whose metadata is not there yet before looking again.
const METADATA_RETRY: Duration = Duration::from_secs(5);

/// How long the switch waits for the user to settle before the engine acts on it. Each
/// change restarts the torrent, which costs it its peers, so a burst of flicking the
/// switch back and forth is worth only one restart.
const SEQUENTIAL_DEBOUNCE: Duration = Duration::from_secs(1);

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/rill/ui/torrent_info_dialog.ui")]
    pub struct TorrentInfoDialog {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub state_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub size_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub progress_bar: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub piece_map: TemplateChild<gtk::DrawingArea>,
        #[template_child]
        pub download_speed_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub upload_speed_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub peers_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub eta_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub sequential_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub source_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub files_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub files: TemplateChild<gio::ListStore>,
        #[template_child]
        pub peers_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub trackers_list: TemplateChild<gtk::ListBox>,

        /// The row shown for each connected peer, kept between snapshots and changed in
        /// place: rebuilding them every second churned widgets, and with them the
        /// renderer's textures, for a list that mostly stays the same.
        pub peer_rows: RefCell<HashMap<String, PeerRow>>,
        pub info_hash: RefCell<String>,
        pub pieces: Rc<RefCell<Vec<u8>>>,
        pub engine: RefCell<Option<Rc<TorrentEngine>>>,
        pub storage: RefCell<Option<Storage>>,
        /// Set while the switch is changed from a snapshot rather than by the user.
        pub updating: Cell<bool>,
        pub sequential_changed: Cell<Option<Instant>>,
        /// Counts changes of the switch, so a waiting one can tell it has been
        /// overtaken by a later change. Shared with the waiting ones, which outlive a
        /// dialog closed right after the switch moved.
        pub sequential_changes: Rc<Cell<u64>>,
        pub metadata_loaded: Cell<bool>,
        pub metadata_loading: Cell<bool>,
        /// When the metadata was last looked for. A magnet link has none until its peers
        /// send it, and looking on every snapshot means parsing a file every second.
        pub metadata_tried: Cell<Option<Instant>>,
        /// The trackers the page already lists, so that the same ones are not rebuilt.
        pub shown_trackers: RefCell<Vec<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TorrentInfoDialog {
        const NAME: &'static str = "RillTorrentInfoDialog";
        type Type = super::TorrentInfoDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            FileItem::ensure_type();
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TorrentInfoDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // The map is drawn in the widget's colour, the accent, over a faint track.
            let pieces = self.pieces.clone();
            self.piece_map
                .set_draw_func(move |area, cr, width, height| {
                    let (w, h) = (width as f64, height as f64);
                    let color = area.color();
                    let (r, g, b) = (
                        color.red() as f64,
                        color.green() as f64,
                        color.blue() as f64,
                    );
                    cr.set_source_rgba(r, g, b, 0.15);
                    cr.rectangle(0.0, 0.0, w, h);
                    let _ = cr.fill();

                    let pieces = pieces.borrow();
                    if pieces.is_empty() {
                        return;
                    }
                    let segment = w / pieces.len() as f64;
                    for (i, &fill) in pieces.iter().enumerate().filter(|(_, f)| **f > 0) {
                        cr.set_source_rgba(r, g, b, fill as f64 / 255.0);
                        cr.rectangle(i as f64 * segment, 0.0, segment.ceil(), h);
                        let _ = cr.fill();
                    }
                });

            self.peers_list
                .set_placeholder(Some(&placeholder(&gettext("No peers connected"))));
            self.trackers_list
                .set_placeholder(Some(&placeholder(&gettext("No trackers"))));

            self.sequential_row.connect_active_notify(glib::clone!(
                #[weak]
                obj,
                move |row| obj.sequential_toggled(row.is_active())
            ));
        }
    }

    impl WidgetImpl for TorrentInfoDialog {}
    impl AdwDialogImpl for TorrentInfoDialog {}

    #[gtk::template_callbacks]
    impl TorrentInfoDialog {
        #[template_callback]
        fn on_copy_source(&self) {
            let obj = self.obj();
            obj.clipboard()
                .set_text(&self.source_row.subtitle().unwrap_or_default());
            self.toast_overlay
                .add_toast(adw::Toast::new(&gettext("Source copied")));
        }
    }
}

glib::wrapper! {
    pub struct TorrentInfoDialog(ObjectSubclass<imp::TorrentInfoDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl TorrentInfoDialog {
    pub fn new(update: &UiUpdate, engine: Rc<TorrentEngine>, storage: Storage) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.info_hash.replace(update.info_hash.clone());
        imp.engine.replace(Some(engine));
        imp.storage.replace(Some(storage));
        dialog.apply_update(update);
        dialog
    }

    pub fn apply_update(&self, update: &UiUpdate) {
        let imp = self.imp();
        self.set_title(&update.name);
        imp.name_label.set_text(&update.name);

        // A snapshot taken before a change of the switch still has the old value.
        let recent_change = imp
            .sequential_changed
            .get()
            .is_some_and(|at| at.elapsed() < SEQUENTIAL_GRACE);
        if update.sequential == imp.sequential_row.is_active() {
            imp.sequential_changed.set(None);
        } else if !recent_change {
            imp.updating.set(true);
            imp.sequential_row.set_active(update.sequential);
            imp.updating.set(false);
        }

        let progress = if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        };
        // Translators: %s is a percentage, "42.0".
        let percent = gettext("%s%").replace("%s", &format!("{:.1}", progress * 100.0));
        let state = match update.state {
            TorrentUiState::Downloading => {
                // Translators: state and percentage done, "Downloading · 42.0%".
                gettext("Downloading · %s").replace("%s", &percent)
            }
            TorrentUiState::Paused => gettext("Paused · %s").replace("%s", &percent),
            TorrentUiState::Completed => gettext("Completed"),
            TorrentUiState::Error => gettext("Failed"),
        };
        imp.state_label.set_text(&state);
        imp.progress_bar.set_fraction(progress);
        imp.size_label.set_text(&if update.total > 0 {
            gettext("%1 of %2")
                .replace("%1", &format_size(update.downloaded))
                .replace("%2", &format_size(update.total))
        } else {
            gettext("Waiting for metadata")
        });

        imp.piece_map.set_visible(!update.piece_map.is_empty());
        // Drawn again only when it changed; most seconds it has not.
        let same_pieces = *imp.pieces.borrow() == update.piece_map;
        if !same_pieces {
            imp.pieces.replace(update.piece_map.clone());
            imp.piece_map.queue_draw();
        }

        imp.download_speed_label
            .set_text(&format_rate(update.speed_down));
        imp.upload_speed_label
            .set_text(&format_rate(update.speed_up));
        imp.peers_label.set_text(&update.peers.to_string());
        imp.eta_label.set_text(&match update.state {
            TorrentUiState::Downloading if update.speed_down > 0 && update.total > 0 => format_eta(
                update.total.saturating_sub(update.downloaded),
                update.speed_down,
            ),
            TorrentUiState::Completed => gettext("None"),
            _ => gettext("Unknown"),
        });

        imp.folder_row
            .set_subtitle(&update.output_dir.to_string_lossy());
        imp.source_row.set_subtitle(&update.uri);

        self.update_peers(&update.peers_list);
        self.load_metadata(update);
    }

    fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn sequential_toggled(&self, active: bool) {
        let imp = self.imp();
        if imp.updating.get() {
            return;
        }
        imp.sequential_changed.set(Some(Instant::now()));
        let hash = imp.info_hash.borrow().clone();
        if let Some(engine) = imp.engine.borrow().clone() {
            let changes = imp.sequential_changes.clone();
            let change = changes.get().wrapping_add(1);
            changes.set(change);
            let hash = hash.clone();
            // A dialog closed in the meantime leaves its last change to be applied: it
            // is what the user asked for, and what the database now holds.
            glib::timeout_add_local_once(SEQUENTIAL_DEBOUNCE, move || {
                if changes.get() == change {
                    engine.set_sequential(&hash, active);
                }
            });
        }

        let Some(storage) = imp.storage.borrow().clone() else {
            return;
        };
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            async move {
                let result = storage
                    .query(move |s| s.update_torrent_sequential(&hash, active))
                    .await
                    .and_then(|r| r);
                if let Err(e) = result {
                    log::warn!("Failed to save the sequential setting: {e}");
                    // The switch has moved; without a word it would move back on restart.
                    dialog.show_toast(&gettext("Could not save the sequential setting"));
                }
            }
        ));
    }

    fn update_peers(&self, peers: &[PeerInfo]) {
        let imp = self.imp();
        let list = &imp.peers_list;
        let mut rows = imp.peer_rows.borrow_mut();

        let connected: std::collections::HashSet<&str> =
            peers.iter().map(|peer| peer.address.as_str()).collect();
        rows.retain(|address, row| {
            let still_here = connected.contains(address.as_str());
            if !still_here {
                list.remove(&row.row);
            }
            still_here
        });

        for peer in peers {
            // A peer that was not there a second ago goes to the end of the list, so the
            // ones already shown stay where the reader last saw them.
            let row = rows
                .entry(peer.address.clone())
                .or_insert_with(|| PeerRow::new(&peer.address, list));
            row.show(peer);
        }
    }

    /// Fills the Files and Trackers pages once the torrent's metadata is on disk. The
    /// parsing runs on a worker, once at a time.
    fn load_metadata(&self, update: &UiUpdate) {
        let imp = self.imp();
        if imp.metadata_loaded.get() || imp.metadata_loading.get() {
            return;
        }
        if imp
            .metadata_tried
            .get()
            .is_some_and(|at| at.elapsed() < METADATA_RETRY)
        {
            return;
        }
        imp.metadata_tried.set(Some(Instant::now()));
        imp.metadata_loading.set(true);
        let uri = update.uri.clone();
        let output_dir = update.output_dir.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            async move {
                let loaded = gio::spawn_blocking(move || read_metadata(&uri, &output_dir))
                    .await
                    .unwrap_or_default();
                let imp = dialog.imp();
                imp.metadata_loading.set(false);
                let (files, trackers) = loaded;
                // The borrow ends here: replacing the list below needs it back.
                let same_trackers = *imp.shown_trackers.borrow() == trackers;
                if !same_trackers && (!trackers.is_empty() || !files.is_empty()) {
                    dialog.show_trackers(&trackers);
                    imp.shown_trackers.replace(trackers);
                }
                if !files.is_empty() {
                    let items: Vec<FileItem> = files
                        .iter()
                        .map(|(path, size)| FileItem::new(path, *size))
                        .collect();
                    imp.files.splice(0, imp.files.n_items(), &items);
                    imp.files_stack.set_visible_child_name("list");
                    imp.metadata_loaded.set(true);
                }
            }
        ));
    }

    fn show_trackers(&self, trackers: &[String]) {
        let list = &self.imp().trackers_list;
        list.remove_all();
        for tracker in trackers {
            let row = adw::ActionRow::builder()
                .title(tracker)
                .use_markup(false)
                .build();
            list.append(&row);
        }
    }
}

/// One peer's row: built once, then told what the peer is doing.
pub struct PeerRow {
    row: adw::ActionRow,
    down: gtk::Label,
    up: gtk::Label,
    encrypted: gtk::Image,
}

impl PeerRow {
    fn new(address: &str, list: &gtk::ListBox) -> Self {
        let row = adw::ActionRow::builder()
            .title(address)
            .use_markup(false)
            .build();
        let figures = gtk::Box::builder()
            .spacing(12)
            .valign(gtk::Align::Center)
            .build();
        let rate = || {
            gtk::Label::builder()
                .css_classes(["caption", "dim-label", "numeric"])
                .visible(false)
                .build()
        };
        let (down, up) = (rate(), rate());
        let encrypted = gtk::Image::builder()
            .icon_name("channel-secure-symbolic")
            .tooltip_text(gettext("Encrypted connection"))
            .visible(false)
            .build();
        figures.append(&down);
        figures.append(&up);
        figures.append(&encrypted);
        row.add_suffix(&figures);
        list.append(&row);
        Self {
            row,
            down,
            up,
            encrypted,
        }
    }

    /// Changes what the row says. Setting a label to what it already holds costs
    /// nothing, so a peer that has not moved is not redrawn.
    fn show(&self, peer: &PeerInfo) {
        self.row
            .set_subtitle(peer.client.as_deref().unwrap_or_default());
        for (label, arrow, rate) in [
            (&self.down, "↓", peer.speed_down),
            (&self.up, "↑", peer.speed_up),
        ] {
            label.set_visible(rate > 0);
            if rate > 0 {
                label.set_label(&format!("{arrow} {}", format_rate(rate)));
            }
        }
        self.encrypted.set_visible(peer.encrypted);
    }
}

fn placeholder(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .margin_top(18)
        .margin_bottom(18)
        .css_classes(["dim-label"])
        .build()
}

/// The files (path and size) and trackers of a torrent: from its .torrent file, or for a
/// magnet link from the metadata mtorrent saved in `output_dir`. Before that metadata
/// exists, a magnet link gives only the trackers it names.
fn read_metadata(uri: &str, output_dir: &Path) -> (Vec<(String, u64)>, Vec<String>) {
    let path = Path::new(uri);
    let meta = if path.is_file() {
        Metainfo::from_file(path).ok()
    } else {
        find_metainfo(uri, output_dir)
    };
    let Some(meta) = meta else {
        return (Vec::new(), magnet_trackers(uri));
    };

    let files = match meta.files() {
        Some(files) => files
            .map(|(len, path)| (path.to_string_lossy().into_owned(), len as u64))
            .collect(),
        None => vec![(
            meta.name().unwrap_or_default().to_string(),
            meta.length().unwrap_or(0) as u64,
        )],
    };
    let mut trackers: Vec<String> = meta.announce().map(|t| t.to_string()).into_iter().collect();
    for tier in meta.announce_list().into_iter().flatten() {
        for tracker in tier {
            let tracker = tracker.to_string();
            if !trackers.contains(&tracker) {
                trackers.push(tracker);
            }
        }
    }
    (files, trackers)
}

/// The .torrent in `output_dir` whose info hash is the magnet link's, if mtorrent has
/// saved it there; never another torrent's that happens to share the folder.
fn find_metainfo(uri: &str, output_dir: &Path) -> Option<Metainfo> {
    use std::str::FromStr;

    let target = *MagnetLink::from_str(uri).ok()?.info_hash();
    std::fs::read_dir(output_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|e| e == "torrent") && p.is_file())
        .filter_map(|p| Metainfo::from_file(&p).ok())
        .find(|meta| *meta.info_hash() == target)
}

fn magnet_trackers(uri: &str) -> Vec<String> {
    let mut trackers: Vec<String> = Vec::new();
    if !uri.starts_with("magnet:") {
        return trackers;
    }
    for value in uri.split(['?', '&']).filter_map(|p| p.strip_prefix("tr=")) {
        if let Ok(tracker) = urlencoding::decode(value)
            && !trackers.iter().any(|t| *t == tracker)
        {
            trackers.push(tracker.into_owned());
        }
    }
    trackers
}
