//! A row of the torrent list: name, progress and state, a button for the obvious next
//! step, and a context menu. What the row's actions do is up to the window, which they
//! reach through its `win.*-torrent` actions.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gdk, gio, glib};

use crate::engine::{TorrentUiState, UiUpdate};
use crate::util::{format_rate, format_size};
use crate::window::RillWindow;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sachesi/rill/ui/torrent_row.ui")]
    #[properties(wrapper_type = super::TorrentRow)]
    pub struct TorrentRow {
        #[template_child]
        pub state_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub details: TemplateChild<gtk::Box>,
        #[template_child]
        pub name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub status_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub progress_bar: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub action_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub torrent_menu: TemplateChild<gio::Menu>,
        #[template_child]
        pub selection_menu: TemplateChild<gio::Menu>,

        #[property(get, set)]
        pub selection_mode: Cell<bool>,
        #[property(get, set)]
        pub selected: Cell<bool>,

        pub info_hash: RefCell<String>,
        pub state: Cell<TorrentUiState>,
        /// The download queue holds it, to start when a slot frees up.
        pub queued: Cell<bool>,
        /// Its files were found gone; said instead of its progress until it runs again.
        pub files_missing: Cell<bool>,
        /// The current run has its files on disk: until then a run started after they were
        /// removed has not made them again, and they are not worth looking for.
        pub files_made: Cell<bool>,
        pub latest: RefCell<Option<UiUpdate>>,
        pub menu: RefCell<Option<gtk::PopoverMenu>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TorrentRow {
        const NAME: &'static str = "RillTorrentRow";
        type Type = super::TorrentRow;
        type ParentType = gtk::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action("row.primary", None, |row, _, _| row.primary_action());
            klass.install_action("row.pause", None, |row, _, _| {
                row.forward("win.pause-torrent")
            });
            klass.install_action("row.resume", None, |row, _, _| {
                row.forward("win.resume-torrent")
            });
            klass.install_action("row.retry", None, |row, _, _| {
                row.forward("win.resume-torrent")
            });
            klass.install_action("row.delete", None, |row, _, _| {
                row.forward("win.delete-torrent")
            });
            klass.install_action("row.select", None, |row, _, _| {
                row.activate_action("win.select", None).ok();
                row.set_selected(true);
            });
            klass.install_action("row.open-folder", None, |row, _, _| row.open_folder());
            klass.install_action("row.copy-link", None, |row, _, _| row.copy_link());
            klass.install_action("row.change-folder", None, |row, _, _| {
                row.forward("win.change-folder")
            });
            klass.install_action("row.menu", None, |row, _, _| {
                let width = row.width() as f64;
                row.popup_menu(width / 2.0, row.height() as f64 / 2.0);
            });
            klass.add_binding_action(gdk::Key::F10, gdk::ModifierType::SHIFT_MASK, "row.menu");
            klass.add_binding_action(gdk::Key::Menu, gdk::ModifierType::empty(), "row.menu");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for TorrentRow {
        fn dispose(&self) {
            if let Some(menu) = self.menu.take() {
                menu.unparent();
            }
        }
    }

    impl WidgetImpl for TorrentRow {}
    impl ListBoxRowImpl for TorrentRow {}

    #[gtk::template_callbacks]
    impl TorrentRow {
        #[template_callback]
        fn on_secondary_click(&self, _n_press: i32, x: f64, y: f64) {
            self.obj().popup_menu(x, y);
        }

        #[template_callback]
        fn on_long_press(&self, x: f64, y: f64) {
            self.obj().popup_menu(x, y);
        }
    }
}

glib::wrapper! {
    pub struct TorrentRow(ObjectSubclass<imp::TorrentRow>)
        @extends gtk::ListBoxRow, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl TorrentRow {
    pub fn new(info_hash: &str) -> Self {
        let row: Self = glib::Object::new();
        row.imp().info_hash.replace(info_hash.to_string());
        row
    }

    pub fn info_hash(&self) -> String {
        self.imp().info_hash.borrow().clone()
    }

    pub fn state(&self) -> TorrentUiState {
        self.imp().state.get()
    }

    pub fn name(&self) -> String {
        self.imp().name_label.text().to_string()
    }

    /// The last snapshot shown, if any.
    pub fn latest(&self) -> Option<UiUpdate> {
        self.imp().latest.borrow().clone()
    }

    /// The bytes downloaded and the total, as last shown; zero before any snapshot.
    pub fn progress(&self) -> (u64, u64) {
        self.imp()
            .latest
            .borrow()
            .as_ref()
            .map_or((0, 0), |update| (update.downloaded, update.total))
    }

    pub fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || self.name().to_lowercase().contains(query)
            || self.imp().info_hash.borrow().contains(query)
    }

    pub fn update(&self, update: &UiUpdate) {
        let imp = self.imp();
        imp.latest.replace(Some(update.clone()));

        let name = if update.name.is_empty() {
            // Translators: a torrent without a name yet; %s is the start of its info hash.
            gettext("Torrent %s").replace("%s", update.info_hash.get(..8).unwrap_or_default())
        } else {
            update.name.clone()
        };
        imp.name_label.set_text(&name);
        if update.state != TorrentUiState::Paused {
            imp.files_missing.set(false);
        }
        if update.state != TorrentUiState::Downloading {
            imp.files_made.set(false);
        }
        imp.status_label.set_text(&if imp.files_missing.get() {
            missing_text(update)
        } else if imp.queued.get() && update.state == TorrentUiState::Paused {
            queued_text(update)
        } else {
            status_text(update)
        });
        imp.progress_bar.set_fraction(if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        });
        self.set_state(update.state);
    }

    /// Says whether the download queue holds the torrent. The row shows it from its next
    /// snapshot on, or at once when it has one.
    pub fn set_queued(&self, queued: bool) {
        if self.imp().queued.replace(queued) == queued {
            return;
        }
        if let Some(update) = self.latest() {
            self.update(&update);
        }
    }

    /// Whether the files of the torrent are worth looking for: it is not running, or its run
    /// has made them.
    pub fn files_expected(&self) -> bool {
        self.state() != TorrentUiState::Downloading || self.imp().files_made.get()
    }

    /// Notes that a snapshot from the engine counts pieces, which it does once the run has
    /// made the torrent's files.
    pub fn note_snapshot(&self, update: &UiUpdate) {
        if update.state == TorrentUiState::Downloading && update.total_pieces > 0 {
            self.imp().files_made.set(true);
        }
    }

    /// Notes that the torrent's run is being replaced by one that makes files the last one
    /// did not have, so they are not missed before it has.
    pub fn expect_new_files(&self) {
        self.imp().files_made.set(false);
    }

    /// Whether the torrent's files were found gone since it last ran.
    pub fn files_missing(&self) -> bool {
        self.imp().files_missing.get()
    }

    /// Says the torrent's files are gone, some or all, until it runs again.
    pub fn show_files_missing(&self) {
        self.imp().files_missing.set(true);
        if let Some(update) = self.latest() {
            self.update(&update);
        }
    }

    /// Shows `state` ahead of the engine confirming it.
    pub fn set_state(&self, state: TorrentUiState) {
        let imp = self.imp();
        imp.state.set(state);

        let (icon, class, button_icon, button_tooltip) = match state {
            TorrentUiState::Downloading => (
                "folder-download-symbolic",
                "accent",
                "media-playback-pause-symbolic",
                gettext("Pause"),
            ),
            TorrentUiState::Paused => (
                "media-playback-pause-symbolic",
                "dim-label",
                "media-playback-start-symbolic",
                gettext("Resume"),
            ),
            TorrentUiState::Completed => (
                "object-select-symbolic",
                "success",
                "user-trash-symbolic",
                gettext("Delete"),
            ),
            TorrentUiState::Error => (
                "dialog-error-symbolic",
                "error",
                "user-trash-symbolic",
                gettext("Delete"),
            ),
        };
        imp.state_icon.set_icon_name(Some(icon));
        imp.state_icon.set_css_classes(&["torrent-icon", class]);
        imp.action_button.set_icon_name(button_icon);
        imp.action_button.set_tooltip_text(Some(&button_tooltip));
        imp.details.set_opacity(if state == TorrentUiState::Paused {
            0.55
        } else {
            1.0
        });

        self.action_set_enabled("row.pause", state == TorrentUiState::Downloading);
        self.action_set_enabled("row.resume", state == TorrentUiState::Paused);
        self.action_set_enabled("row.retry", state == TorrentUiState::Error);
    }

    fn primary_action(&self) {
        match self.state() {
            TorrentUiState::Downloading => self.forward("win.pause-torrent"),
            TorrentUiState::Paused => self.forward("win.resume-torrent"),
            TorrentUiState::Completed | TorrentUiState::Error => self.forward("win.delete-torrent"),
        }
    }

    /// Activates a window action that takes this torrent's info hash.
    fn forward(&self, action: &str) {
        let hash = self.info_hash();
        if let Err(e) = self.activate_action(action, Some(&hash.to_variant())) {
            log::warn!("{action} for {hash}: {e}");
        }
    }

    fn open_folder(&self) {
        let Some(update) = self.latest() else {
            return;
        };
        let dir = crate::torrent_paths::folder_to_open(&update.uri, &update.output_dir);
        let window = self.root().and_downcast::<RillWindow>();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(
            window.clone().as_ref(),
            gio::Cancellable::NONE,
            move |result| {
                if let Err(e) = result {
                    log::warn!("Failed to open {}: {e}", dir.display());
                    if let Some(window) = &window {
                        window.show_toast(&gettext("Could not open the folder"));
                    }
                }
            },
        );
    }

    /// Puts the torrent's magnet link on the clipboard, made up from what it was added
    /// from: a magnet link as it is, a .torrent file as a link to its info hash.
    fn copy_link(&self) {
        let Some(update) = self.latest() else {
            return;
        };
        let link = magnet_link(&update);
        self.clipboard().set_text(&link);
        if let Some(window) = self.root().and_downcast::<RillWindow>() {
            window.show_toast(&gettext("Magnet link copied"));
        }
    }

    fn popup_menu(&self, x: f64, y: f64) {
        let imp = self.imp();
        let model = if self.selection_mode() {
            imp.selection_menu.get()
        } else {
            imp.torrent_menu.get()
        };
        let existing = imp.menu.borrow().clone();
        let menu = existing.unwrap_or_else(|| {
            let menu = gtk::PopoverMenu::builder().has_arrow(false).build();
            menu.set_parent(self);
            imp.menu.replace(Some(menu.clone()));
            menu
        });
        menu.set_menu_model(Some(&model));
        menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        menu.popup();
    }
}

/// The magnet link of a torrent: the one it was added from, or one made of its info hash
/// and name.
fn magnet_link(update: &UiUpdate) -> String {
    if update.uri.starts_with("magnet:") {
        return update.uri.clone();
    }
    let link = format!("magnet:?xt=urn:btih:{}", update.info_hash);
    if update.name.is_empty() {
        link
    } else {
        format!("{link}&dn={}", urlencoding::encode(&update.name))
    }
}

fn queued_text(update: &UiUpdate) -> String {
    let size = status_text(update);
    if update.total == 0 {
        gettext("Queued")
    } else {
        // Translators: a torrent waiting for a free download slot; %s is its progress,
        // "1.2 GiB of 4.0 GiB".
        gettext("Queued · %s").replace("%s", &size)
    }
}

fn missing_text(update: &UiUpdate) -> String {
    if update.downloaded == 0 {
        gettext("Files not found")
    } else {
        // Translators: some of a torrent's files are gone; %s is how much is left, "1.2 GiB".
        gettext("Some files not found, %s left").replace("%s", &format_size(update.downloaded))
    }
}

fn status_text(update: &UiUpdate) -> String {
    let size = if update.total > 0 {
        // Translators: progress of a torrent, "1.2 GiB of 4.0 GiB".
        gettext("%1 of %2")
            .replace("%1", &format_size(update.downloaded))
            .replace("%2", &format_size(update.total))
    } else {
        String::new()
    };
    match (update.state, update.total) {
        (TorrentUiState::Downloading, 0) => return gettext("Waiting for metadata"),
        (_, 0) => return gettext("Size unknown"),
        (TorrentUiState::Downloading, _) => {}
        _ => return size,
    }
    let mut parts = vec![size];
    if update.speed_down > 0 {
        parts.push(format!("↓ {}", format_rate(update.speed_down)));
    }
    parts.push(
        ngettext("%d peer", "%d peers", update.peers as u32)
            .replace("%d", &update.peers.to_string()),
    );
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(uri: &str, name: &str) -> UiUpdate {
        UiUpdate::idle(
            "aabbccdd".into(),
            name.into(),
            TorrentUiState::Paused,
            std::path::PathBuf::from("/tmp"),
            uri.into(),
            false,
        )
    }

    #[test]
    fn a_magnet_link_is_copied_as_it_came_and_a_file_as_its_info_hash() {
        let magnet = "magnet:?xt=urn:btih:aabbccdd&dn=Name";
        assert_eq!(magnet_link(&update(magnet, "Name")), magnet);
        assert_eq!(
            magnet_link(&update("/tmp/film.torrent", "Some Film")),
            "magnet:?xt=urn:btih:aabbccdd&dn=Some%20Film"
        );
        assert_eq!(
            magnet_link(&update("/tmp/film.torrent", "")),
            "magnet:?xt=urn:btih:aabbccdd"
        );
    }
}
