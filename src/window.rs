//! The main window: the torrents grouped by state, search, selection mode, and the
//! `win.*` actions through which the rows, the dialogs and the tray reach the engine.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use async_channel::{Receiver, Sender};
use gettextrs::{gettext, ngettext};
use gtk::{gdk, gio, glib};

use crate::dialogs::{AddTorrentDialog, TorrentInfoDialog};
use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};
use crate::storage::{SavedTorrent, Storage};
use crate::torrent_paths;
use crate::torrent_row::TorrentRow;
use crate::torrents::{Torrents, state_key};
use crate::tray;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sachesi/rill/ui/window.ui")]
    #[properties(wrapper_type = super::RillWindow)]
    pub struct RillWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub search_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub search_bar: TemplateChild<gtk::SearchBar>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub content_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub downloading_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub downloading_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub paused_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub paused_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub finished_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub finished_list: TemplateChild<gtk::ListBox>,

        #[property(get, set = Self::set_selection_mode)]
        pub selection_mode: Cell<bool>,
        /// What the list is ordered by, as `SortOrder::key` spells it.
        #[property(get, set = Self::set_sort)]
        pub sort: RefCell<String>,

        pub engine: OnceCell<Rc<TorrentEngine>>,
        pub storage: OnceCell<Storage>,
        pub tx: OnceCell<Sender<UiEvent>>,
        pub rows: RefCell<HashMap<String, TorrentRow>>,
        pub info_dialogs: RefCell<HashMap<String, TorrentInfoDialog>>,
        /// What there is to know about the torrents of the rows, the queue included.
        pub torrents: RefCell<Torrents>,
        /// How many downloads may run at once.
        pub download_limit: Cell<usize>,
        /// Whether the notice that Rill keeps running in the tray has been sent.
        pub background_notice_sent: Cell<bool>,
        pub queue_check_pending: Cell<bool>,
        /// Set while the files of the torrents are being looked for.
        pub checking_files: Cell<bool>,
        /// The layout of each torrent's content, read from its metadata once rather than
        /// on every look for its files.
        pub layouts: Arc<Layouts>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillWindow {
        const NAME: &'static str = "RillWindow";
        type Type = super::RillWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action("win.add-file", None, |win, _, _| win.choose_torrent_file());
            klass.install_action("win.add-magnet", None, |win, _, _| win.add_magnet_link(""));
            klass.install_action("win.search", None, |win, _, _| {
                let button = &win.imp().search_button;
                button.set_active(!button.is_active());
            });

            klass.install_action("win.select", None, |win, _, _| win.set_selection_mode(true));
            klass.install_action("win.leave-selection", None, |win, _, _| {
                win.set_selection_mode(false)
            });
            klass.install_action("win.select-all", None, |win, _, _| win.select_all(true));
            klass.install_action("win.select-none", None, |win, _, _| win.select_all(false));
            klass.install_action("win.resume-selected", None, |win, _, _| {
                for hash in win.selected_hashes() {
                    win.resume_torrent(&hash);
                }
                win.set_selection_mode(false);
            });
            klass.install_action("win.pause-selected", None, |win, _, _| {
                for hash in win.selected_hashes() {
                    win.pause_torrent(&hash);
                }
                win.set_selection_mode(false);
            });
            klass.install_action("win.delete-selected", None, |win, _, _| {
                win.confirm_delete(win.selected_hashes());
            });
            klass.install_property_action("win.sort", "sort");
            klass.install_action("win.pause-all", None, |win, _, _| {
                win.for_each_in_state(
                    TorrentUiState::Downloading,
                    super::RillWindow::pause_torrent,
                );
            });
            klass.install_action("win.resume-all", None, |win, _, _| win.resume_all());

            let hash = Some(glib::VariantTy::STRING);
            klass.install_action("win.pause-torrent", hash, |win, _, v| {
                if let Some(hash) = v.and_then(|v| v.str()) {
                    win.pause_torrent(hash);
                }
            });
            klass.install_action("win.resume-torrent", hash, |win, _, v| {
                if let Some(hash) = v.and_then(|v| v.str()) {
                    win.resume_torrent(hash);
                }
            });
            klass.install_action("win.change-folder", hash, |win, _, v| {
                if let Some(hash) = v.and_then(|v| v.str()) {
                    win.choose_folder_for(hash);
                }
            });
            klass.install_action("win.delete-torrent", hash, |win, _, v| {
                if let Some(hash) = v.and_then(|v| v.str()) {
                    win.confirm_delete(vec![hash.to_string()]);
                }
            });

            klass.add_binding_action(
                gdk::Key::Escape,
                gdk::ModifierType::empty(),
                "win.leave-selection",
            );
            klass.add_binding_action(
                gdk::Key::Delete,
                gdk::ModifierType::empty(),
                "win.delete-selected",
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for RillWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.update_selection_actions();
            obj.setup_drop_target();
            // Blueprint cannot do this: without it the bar captures keys with nowhere to
            // put them, and GTK says so on every keystroke.
            self.search_bar.connect_entry(&*self.search_entry);
            self.search_bar
                .connect_search_mode_enabled_notify(glib::clone!(
                    #[weak]
                    obj,
                    move |bar| {
                        if !bar.is_search_mode() {
                            obj.imp().search_entry.set_text("");
                        }
                    }
                ));
        }
    }

    impl WidgetImpl for RillWindow {}

    impl WindowImpl for RillWindow {
        fn close_request(&self) -> glib::Propagation {
            let obj = self.obj();
            obj.save_window_state();

            let Some(app) = obj.application() else {
                return self.parent_close_request();
            };
            // Without a tray there is no way back to a hidden window, so closing quits,
            // which pauses every torrent.
            if !tray::is_available() {
                app.quit();
                return glib::Propagation::Stop;
            }
            // Otherwise the window hides and the transfers go on. Say so the first time,
            // since a window vanishing mid-download looks just like quitting.
            if !self.background_notice_sent.replace(true) {
                let notification = gio::Notification::new(&gettext("Rill is still running"));
                notification.set_body(Some(&gettext(
                    "Downloads continue in the background. Use the tray icon to reopen or quit Rill.",
                )));
                app.send_notification(Some("background"), &notification);
            }
            obj.set_visible(false);
            glib::Propagation::Stop
        }
    }

    impl ApplicationWindowImpl for RillWindow {}
    impl AdwApplicationWindowImpl for RillWindow {}

    #[gtk::template_callbacks]
    impl RillWindow {
        #[template_callback]
        fn on_search_changed(&self) {
            self.obj().apply_filter();
        }

        #[template_callback]
        fn on_row_activated(&self, row: &gtk::ListBoxRow) {
            let Some(row) = row.downcast_ref::<TorrentRow>() else {
                return;
            };
            if self.selection_mode.get() {
                row.set_selected(!row.selected());
            } else {
                self.obj().show_info(row);
            }
        }
    }

    impl RillWindow {
        fn set_selection_mode(&self, active: bool) {
            if self.selection_mode.replace(active) == active {
                return;
            }
            let obj = self.obj();
            if active {
                self.search_bar.set_search_mode(false);
            } else {
                for row in self.rows.borrow().values() {
                    row.set_selected(false);
                }
            }
            obj.update_selection_actions();
            obj.notify_selection_mode();
        }

        fn set_sort(&self, order: String) {
            if *self.sort.borrow() == order {
                return;
            }
            self.sort.replace(order.clone());
            let obj = self.obj();
            obj.sort_rows();
            obj.notify_sort();
            // Not set yet while the window is being built from the stored settings.
            if let Some(storage) = self.storage.get() {
                let storage = storage.clone();
                storage.execute(move |s| {
                    let mut settings = s.load_settings();
                    settings.sort_order = order;
                    if let Err(e) = s.save_settings(&settings) {
                        log::warn!("Failed to save the sort order: {e}");
                    }
                });
            }
        }
    }
}

/// How often the files of the torrents are looked for, to notice ones removed while Rill
/// runs.
const FILE_CHECK_INTERVAL: Duration = Duration::from_secs(10);

/// The content layout of each torrent, by info hash, with the source and folder it was
/// read for.
type Layouts = Mutex<HashMap<String, (String, PathBuf, Arc<torrent_paths::ContentLayout>)>>;

/// The layout of the torrent `update` is about, read once and kept in `layouts` for as long
/// as the torrent keeps its source and folder. Not kept while there is none to read.
fn cached_layout(
    layouts: &Layouts,
    hash: &str,
    update: &UiUpdate,
) -> Option<Arc<torrent_paths::ContentLayout>> {
    let lock = || layouts.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((uri, dir, layout)) = lock().get(hash)
        && *uri == update.uri
        && *dir == update.output_dir
    {
        return Some(layout.clone());
    }
    let layout = Arc::new(torrent_paths::content_layout(
        &update.uri,
        &update.output_dir,
    )?);
    lock().insert(
        hash.to_string(),
        (
            update.uri.clone(),
            update.output_dir.clone(),
            layout.clone(),
        ),
    );
    Some(layout)
}

/// What the torrent list is ordered by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    /// Oldest first, which is the order the queue starts them in.
    #[default]
    Added,
    Name,
    /// Largest first.
    Size,
    /// Closest to done first.
    Progress,
}

impl SortOrder {
    /// The order as the settings and the menu spell it.
    pub fn key(self) -> &'static str {
        match self {
            SortOrder::Added => "added",
            SortOrder::Name => "name",
            SortOrder::Size => "size",
            SortOrder::Progress => "progress",
        }
    }

    /// The order `key` names; anything unknown is the default one.
    pub fn from_key(key: &str) -> Self {
        match key {
            "name" => SortOrder::Name,
            "size" => SortOrder::Size,
            "progress" => SortOrder::Progress,
            _ => SortOrder::Added,
        }
    }
}

/// What one torrent is ordered by.
struct SortKey {
    name: String,
    total: u64,
    downloaded: u64,
    /// When it was added, and in which place this session learnt of it.
    added: (i64, u64),
}

/// Orders two torrents, the one added first coming before the other when they are
/// otherwise equal.
fn compare(order: SortOrder, a: &SortKey, b: &SortKey) -> std::cmp::Ordering {
    let fraction = |key: &SortKey| {
        if key.total > 0 {
            key.downloaded as f64 / key.total as f64
        } else {
            0.0
        }
    };
    match order {
        SortOrder::Added => std::cmp::Ordering::Equal,
        SortOrder::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortOrder::Size => b.total.cmp(&a.total),
        SortOrder::Progress => fraction(b).total_cmp(&fraction(a)),
    }
    .then_with(|| a.added.cmp(&b.added))
}

glib::wrapper! {
    pub struct RillWindow(ObjectSubclass<imp::RillWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl RillWindow {
    pub fn new(
        app: &impl IsA<gtk::Application>,
        engine: Rc<TorrentEngine>,
        storage: Storage,
        saved: Vec<SavedTorrent>,
    ) -> Self {
        let window: Self = glib::Object::builder().property("application", app).build();
        let imp = window.imp();
        let (tx, rx) = async_channel::unbounded();

        let settings = storage.load_settings();
        imp.download_limit
            .set(settings.max_active_downloads.max(1) as usize);
        // Before the storage is set, so that reading the order does not save it again.
        window.set_sort(SortOrder::from_key(&settings.sort_order).key());
        window.install_sort();
        window.set_default_size(settings.window_width, settings.window_height);
        if settings.window_maximized {
            window.maximize();
        }

        imp.engine.set(engine).ok();
        imp.storage.set(storage).ok();
        imp.tx.set(tx).ok();

        window.restore_torrents(saved);
        window.listen(rx);
        window
    }

    /// Puts the rows of every section in the chosen order.
    fn sort_rows(&self) {
        let imp = self.imp();
        for list in [
            imp.downloading_list.get(),
            imp.paused_list.get(),
            imp.finished_list.get(),
        ] {
            list.invalidate_sort();
        }
    }

    /// Teaches each section how to order its rows. The order itself is read afresh on
    /// every comparison, so changing it only needs the rows sorted again.
    fn install_sort(&self) {
        let imp = self.imp();
        for list in [
            imp.downloading_list.get(),
            imp.paused_list.get(),
            imp.finished_list.get(),
        ] {
            list.set_sort_func(glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                gtk::Ordering::Equal,
                move |a, b| {
                    let order = SortOrder::from_key(&window.sort());
                    match (
                        window.sort_key(a.upcast_ref()),
                        window.sort_key(b.upcast_ref()),
                    ) {
                        (Some(a), Some(b)) => compare(order, &a, &b).into(),
                        _ => gtk::Ordering::Equal,
                    }
                }
            ));
        }
    }

    /// What `row` is ordered by, or `None` when it is not a torrent row.
    fn sort_key(&self, row: &gtk::Widget) -> Option<SortKey> {
        let row = row.downcast_ref::<TorrentRow>()?;
        let (downloaded, total) = row.progress();
        Some(SortKey {
            name: row.name(),
            total,
            downloaded,
            added: self.imp().torrents.borrow().added(&row.info_hash()),
        })
    }

    /// Whether `row` still sorts between the rows before and after it, which is all that
    /// can have changed when only its own snapshot is new.
    fn in_order(&self, row: &TorrentRow) -> bool {
        let order = SortOrder::from_key(&self.sort());
        if order == SortOrder::Added {
            return true;
        }
        let Some(key) = self.sort_key(row.upcast_ref()) else {
            return true;
        };
        let before = row.prev_sibling().and_then(|prev| self.sort_key(&prev));
        let after = row.next_sibling().and_then(|next| self.sort_key(&next));
        before.is_none_or(|before| compare(order, &before, &key).is_le())
            && after.is_none_or(|after| compare(order, &key, &after).is_le())
    }

    fn engine(&self) -> &TorrentEngine {
        self.imp().engine.get().expect("engine is set in new()")
    }

    pub fn storage(&self) -> &Storage {
        self.imp().storage.get().expect("storage is set in new()")
    }

    fn sender(&self) -> Sender<UiEvent> {
        self.imp().tx.get().expect("sender is set in new()").clone()
    }

    pub fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    pub fn add_magnet_link(&self, uri: &str) {
        let dialog = AddTorrentDialog::new(self);
        dialog.set_magnet(uri);
        dialog.present(Some(self));
    }

    pub fn add_torrent_file(&self, path: &Path) {
        let dialog = AddTorrentDialog::new(self);
        dialog.set_file(path);
        dialog.present(Some(self));
    }

    /// Hands a torrent, known by `hash`, to the engine, running or paused. The row
    /// appears with the engine's first update.
    pub fn start_torrent(
        &self,
        hash: String,
        name: String,
        uri: String,
        dir: PathBuf,
        sequential: bool,
        start_now: bool,
    ) {
        if start_now {
            self.engine()
                .start(hash.clone(), name, uri, dir, sequential, self.sender());
        } else {
            self.engine()
                .add_paused(hash.clone(), name, uri, dir, sequential, self.sender());
        }
        // A torrent deleted earlier in this session may come back.
        let mut torrents = self.imp().torrents.borrow_mut();
        torrents.undelete(&hash);
        if start_now {
            torrents.user_start(&hash);
        }
    }

    /// Where Rill keeps its database and copies of .torrent files.
    pub fn data_dir(&self) -> PathBuf {
        self.engine().config_dir().clone()
    }

    fn engine_rc(&self) -> Rc<TorrentEngine> {
        self.imp()
            .engine
            .get()
            .expect("engine is set in new()")
            .clone()
    }

    fn choose_torrent_file(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&gettext("Torrent Files")));
        filter.add_mime_type("application/x-bittorrent");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Add Torrent File"))
            .filters(&filters)
            .modal(true)
            .build();
        dialog.open(
            Some(self),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |result| {
                    if let Ok(path) = result.map(|f| f.path()) {
                        match path {
                            Some(path) => window.add_torrent_file(&path),
                            None => window.show_toast(&gettext("Only local files can be added")),
                        }
                    }
                }
            ),
        );
    }

    /// Accepts .torrent files and magnet links dropped on the window.
    fn setup_drop_target(&self) {
        let target = gtk::DropTarget::new(glib::Type::INVALID, gdk::DragAction::COPY);
        target.set_types(&[gdk::FileList::static_type(), glib::Type::STRING]);
        target.connect_drop(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                if let Ok(files) = value.get::<gdk::FileList>() {
                    let paths: Vec<PathBuf> = files
                        .files()
                        .iter()
                        .filter_map(|f| f.path())
                        .filter(|p| p.extension().is_some_and(|e| e == "torrent"))
                        .collect();
                    for path in &paths {
                        window.add_torrent_file(path);
                    }
                    return !paths.is_empty();
                }
                let text = value.get::<String>().unwrap_or_default();
                let Some(line) = text.lines().map(str::trim).find(|l| !l.is_empty()) else {
                    return false;
                };
                if line.starts_with("magnet:") {
                    window.add_magnet_link(line);
                    return true;
                }
                let path = gio::File::for_uri(line)
                    .path()
                    .unwrap_or_else(|| PathBuf::from(line));
                if path.extension().is_some_and(|e| e == "torrent") {
                    window.add_torrent_file(&path);
                    return true;
                }
                false
            }
        ));
        self.add_controller(target);
    }

    fn save_window_state(&self) {
        let (width, height) = self.default_size();
        let maximized = self.is_maximized();
        let storage = self.storage().clone();
        storage.execute(move |s| {
            let mut settings = s.load_settings();
            settings.window_width = width;
            settings.window_height = height;
            settings.window_maximized = maximized;
            if let Err(e) = s.save_settings(&settings) {
                log::warn!("Failed to save the window size: {e}");
            }
        });
    }

    // Search and selection

    fn apply_filter(&self) {
        let imp = self.imp();
        let query = imp.search_entry.text().to_lowercase();
        for row in imp.rows.borrow().values() {
            row.set_visible(row.matches(&query));
        }
        self.update_sections();
    }

    fn select_all(&self, selected: bool) {
        for row in self.imp().rows.borrow().values() {
            if row.get_visible() {
                row.set_selected(selected);
            }
        }
    }

    fn selected_hashes(&self) -> Vec<String> {
        self.imp()
            .rows
            .borrow()
            .values()
            .filter(|row| row.selected())
            .map(TorrentRow::info_hash)
            .collect()
    }

    fn update_selection_actions(&self) {
        let active = self.selection_mode();
        let any = !self.selected_hashes().is_empty();
        self.action_set_enabled("win.select", !active);
        self.action_set_enabled("win.leave-selection", active);
        self.action_set_enabled("win.select-all", active);
        self.action_set_enabled("win.select-none", active && any);
        self.action_set_enabled("win.resume-selected", active && any);
        self.action_set_enabled("win.pause-selected", active && any);
        self.action_set_enabled("win.delete-selected", active && any);

        let title = &self.imp().window_title;
        if active {
            let count = self.selected_hashes().len();
            title.set_title(&gettext("Select Torrents"));
            title.set_subtitle(
                &ngettext("%d selected", "%d selected", count as u32)
                    .replace("%d", &count.to_string()),
            );
        } else {
            title.set_title("Rill");
            title.set_subtitle("");
        }
    }

    // Transfers

    /// Runs `action` for every torrent the window shows in `state`.
    fn for_each_in_state(&self, state: TorrentUiState, action: impl Fn(&Self, &str)) {
        let hashes: Vec<String> = self
            .imp()
            .rows
            .borrow()
            .iter()
            .filter(|(_, row)| row.state() == state)
            .map(|(hash, _)| hash.clone())
            .collect();
        for hash in hashes {
            action(self, &hash);
        }
    }

    /// Resumes every paused torrent through the queue: those over the download limit wait
    /// for a slot, rather than start only to be paused again straight away.
    fn resume_all(&self) {
        self.for_each_in_state(TorrentUiState::Paused, |window, hash| {
            window.imp().torrents.borrow_mut().enqueue(hash);
            window.show_queued(hash);
            // Saved as waiting to download, so that a restart starts it too.
            let latest = window
                .imp()
                .rows
                .borrow()
                .get(hash)
                .and_then(TorrentRow::latest);
            if let Some(update) = latest {
                window.persist(&update);
            }
        });
        self.check_queue();
    }

    fn pause_torrent(&self, hash: &str) {
        let imp = self.imp();
        let Some(row) = imp.rows.borrow().get(hash).cloned() else {
            return;
        };
        if row.state() != TorrentUiState::Downloading {
            return;
        }
        imp.torrents.borrow_mut().leave_queue(hash);
        if let Some(mut update) = row.latest() {
            update.state = TorrentUiState::Paused;
            update.speed_down = 0;
            update.speed_up = 0;
            self.process_update(&update);
        }
        self.engine().toggle(hash);
        self.check_queue();
    }

    fn resume_torrent(&self, hash: &str) {
        let imp = self.imp();
        let Some(row) = imp.rows.borrow().get(hash).cloned() else {
            return;
        };
        if !matches!(row.state(), TorrentUiState::Paused | TorrentUiState::Error) {
            return;
        }
        imp.torrents.borrow_mut().user_start(hash);
        if let Some(mut update) = row.latest() {
            update.state = TorrentUiState::Downloading;
            self.process_update(&update);
        }
        self.engine().toggle(hash);
        self.check_queue();
    }

    /// Asks where a torrent's content should go from now on.
    fn choose_folder_for(&self, hash: &str) {
        let Some(update) = self
            .imp()
            .rows
            .borrow()
            .get(hash)
            .and_then(TorrentRow::latest)
        else {
            return;
        };
        let chooser = gtk::FileDialog::builder()
            .title(gettext("Choose a Download Folder"))
            .initial_folder(&gio::File::for_path(&update.output_dir))
            .modal(true)
            .build();
        let hash = hash.to_string();
        chooser.select_folder(
            Some(self),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |result| {
                    if let Ok(Some(folder)) = result.map(|f| f.path()) {
                        window.move_torrent(&hash, folder);
                    }
                }
            ),
        );
    }

    /// Moves a torrent's content to `folder` and downloads the rest of it there. The
    /// torrent stops for the move and goes on afterwards if it was running.
    fn move_torrent(&self, hash: &str, folder: PathBuf) {
        let Some(row) = self.imp().rows.borrow().get(hash).cloned() else {
            return;
        };
        let Some(update) = row.latest() else {
            return;
        };
        if update.output_dir == folder {
            return;
        }
        let running = self.engine().is_active(hash);
        if running {
            self.pause_torrent(hash);
        }
        if let Err(e) = torrent_paths::move_content(&update.uri, &update.output_dir, &folder) {
            log::warn!("Failed to move {hash} to {}: {e}", folder.display());
            self.show_toast(&gettext(
                "Could not move the files; the folder is unchanged",
            ));
            if running {
                self.resume_torrent(hash);
            }
            return;
        }

        self.engine().set_output_dir(hash, folder.clone());
        let (key, dir) = (hash.to_string(), folder.to_string_lossy().into_owned());
        self.storage().execute(move |s| {
            if let Err(e) = s.update_torrent_output_dir(&key, &dir) {
                log::warn!("{e}");
            }
        });
        if let Some(mut update) = row.latest() {
            update.output_dir = folder;
            self.process_update(&update);
        }
        if running {
            self.resume_torrent(hash);
        }
    }

    /// Asks before deleting `hashes`, and whether to delete their downloaded data too.
    fn confirm_delete(&self, hashes: Vec<String>) {
        let Some(first) = hashes.first() else {
            return;
        };
        let (heading, body) = if hashes.len() == 1 {
            let name = self
                .imp()
                .rows
                .borrow()
                .get(first)
                .map(TorrentRow::name)
                .unwrap_or_default();
            (
                gettext("Delete Torrent?"),
                // Translators: %s is the name of a torrent.
                gettext("“%s” will be removed from the list.").replace("%s", &name),
            )
        } else {
            (
                gettext("Delete Torrents?"),
                ngettext(
                    "%d torrent will be removed from the list.",
                    "%d torrents will be removed from the list.",
                    hashes.len() as u32,
                )
                .replace("%d", &hashes.len().to_string()),
            )
        };
        let delete_data =
            gtk::CheckButton::with_label(&gettext("Also delete the downloaded files"));
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .extra_child(&delete_data)
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_response("cancel", &gettext("_Cancel"));
        dialog.add_response("delete", &gettext("_Delete"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.connect_response(
            Some("delete"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    for hash in &hashes {
                        window.delete_torrent(hash, delete_data.is_active());
                    }
                    window.set_selection_mode(false);
                }
            ),
        );
        dialog.present(Some(self));
    }

    fn delete_torrent(&self, hash: &str, delete_data: bool) {
        let imp = self.imp();
        imp.torrents.borrow_mut().delete(hash);
        imp.layouts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(hash);
        let dialog = imp.info_dialogs.borrow_mut().remove(hash);
        if let Some(dialog) = dialog {
            dialog.close();
        }
        self.engine().stop(hash);

        let row = imp.rows.borrow_mut().remove(hash);
        if let Some(row) = row
            && let Some(list) = row.parent().and_downcast::<gtk::ListBox>()
        {
            list.remove(&row);
        }
        self.update_sections();
        self.update_selection_actions();
        self.check_queue();

        let storage = self.storage().clone();
        let hash = hash.to_string();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                // Read where the data is before the record goes; the worker runs jobs in
                // order, so the delete below comes after this read.
                if delete_data {
                    let key = hash.clone();
                    let record = storage
                        .query(move |s| s.load_torrent(&key).ok().flatten())
                        .await
                        .ok()
                        .flatten();
                    let path = record
                        .and_then(|t| torrent_paths::content_path(&t.uri, &t.output_dir_path()));
                    match path {
                        Some(path) => {
                            let target = path.clone();
                            let result =
                                gio::spawn_blocking(move || torrent_paths::remove_content(&target))
                                    .await;
                            match result {
                                Ok(Ok(())) => log::info!("Deleted {}", path.display()),
                                Ok(Err(e)) => {
                                    log::warn!("Failed to delete {}: {e}", path.display());
                                    window.show_toast(
                                        &gettext("Could not delete the downloaded files: %s")
                                            .replace("%s", &e.to_string()),
                                    );
                                }
                                Err(_) => log::warn!("The delete task panicked"),
                            }
                        }
                        None => {
                            log::warn!("No content path for torrent {hash}");
                            window.show_toast(&gettext("Could not find the downloaded files"));
                        }
                    }
                }
                storage.execute(move |s| {
                    if let Err(e) = s.delete_torrent(&hash) {
                        log::warn!("Failed to delete torrent {hash} from the database: {e}");
                    }
                });
            }
        ));
    }

    fn show_info(&self, row: &TorrentRow) {
        let hash = row.info_hash();
        if let Some(dialog) = self.imp().info_dialogs.borrow().get(&hash) {
            dialog.present(Some(self));
            return;
        }
        let Some(update) = row.latest() else {
            return;
        };
        let dialog = TorrentInfoDialog::new(&update, self.engine_rc(), self.storage().clone());
        dialog.connect_closed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                window.imp().info_dialogs.borrow_mut().remove(&hash);
            }
        ));
        self.imp()
            .info_dialogs
            .borrow_mut()
            .insert(row.info_hash(), dialog.clone());
        dialog.present(Some(self));
    }

    // Updates from the engine

    fn listen(&self, rx: Receiver<UiEvent>) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                while let Ok(event) = rx.recv().await {
                    match event {
                        UiEvent::Update(update) => {
                            window.process_update(&update);
                            let row = window.imp().rows.borrow().get(&update.info_hash).cloned();
                            if let Some(row) = row {
                                row.note_snapshot(&update);
                            }
                        }
                        UiEvent::Finished { info_hash, error } => {
                            window.torrent_finished(&info_hash, error)
                        }
                    }
                }
            }
        ));
    }

    /// A torrent task ended, done or failed.
    fn torrent_finished(&self, hash: &str, error: Option<String>) {
        // The final snapshot may never have come (a zero-byte torrent never reports its
        // bytes done), so the row is moved here rather than left downloading.
        let state = if error.is_some() {
            TorrentUiState::Error
        } else {
            TorrentUiState::Completed
        };
        let row = self.imp().rows.borrow().get(hash).cloned();
        if let Some(mut update) = row.as_ref().and_then(TorrentRow::latest) {
            update.state = state;
            update.peers = 0;
            update.speed_down = 0;
            update.speed_up = 0;
            update.peers_list.clear();
            self.process_update(&update);
        }
        let name = row.map_or_else(|| hash.to_string(), |r| r.name());

        match error {
            Some(error) => {
                log::error!("Torrent {hash} failed: {error}");
                // The failed task leaves the engine's running set, so Retry restarts it.
                self.engine().mark_failed(hash);
                self.show_toast(
                    // Translators: %1 is a torrent name, %2 the reason it failed.
                    &gettext("“%1” failed: %2")
                        .replace("%1", &name)
                        .replace("%2", &error),
                );
            }
            None => {
                let key = hash.to_string();
                self.storage().execute(move |s| {
                    if let Err(e) = s.mark_completed(&key) {
                        log::warn!("Failed to mark {key} completed: {e}");
                    }
                });
                let notification = gio::Notification::new(&gettext("Download Complete"));
                notification.set_body(Some(&name));
                if let Some(app) = self.application() {
                    app.send_notification(None, &notification);
                }
            }
        }
    }

    fn process_update(&self, update: &UiUpdate) {
        let imp = self.imp();
        if imp.torrents.borrow().is_deleted(&update.info_hash) {
            return;
        }

        let existing = imp.rows.borrow().get(&update.info_hash).cloned();
        let previous = existing.as_ref().and_then(TorrentRow::latest);
        let mut update = update.clone();
        // Snapshots taken before the metadata arrived, and those the engine makes up
        // itself, carry no sizes, and the engine's no name: keep the last known ones.
        if let Some(previous) = &previous {
            if update.name.is_empty() {
                update.name = previous.name.clone();
            }
            if update.total == 0 {
                update.total = previous.total;
                update.downloaded = previous.downloaded;
            }
            if update.total_pieces == 0 {
                update.total_pieces = previous.total_pieces;
                update.downloaded_pieces = previous.downloaded_pieces;
            }
            // The run a torrent found missing files was paused from reports what it had
            // counted, removed files included.
            let files_missing = existing.as_ref().is_some_and(TorrentRow::files_missing);
            if files_missing && update.state == TorrentUiState::Paused {
                update.downloaded = previous.downloaded;
                update.downloaded_pieces = previous.downloaded_pieces;
            }
        }

        let row = match existing {
            Some(row) => row,
            None => match self.insert_torrent(&update) {
                Some(row) => row,
                None => return,
            },
        };
        let old_state = row.state();
        row.set_queued(imp.torrents.borrow().is_queued(&update.info_hash));
        row.update(&update);
        imp.torrents
            .borrow_mut()
            .set_state(&update.info_hash, update.state);
        if previous.is_some_and(|previous| previous.name != update.name) {
            self.rename(&update.info_hash, &update.name);
        }
        if let Some(dialog) = imp.info_dialogs.borrow().get(&update.info_hash) {
            dialog.apply_update(&update);
        }
        self.persist(&update);

        if row.parent().is_none() || old_state != update.state {
            if let Some(list) = row.parent().and_downcast::<gtk::ListBox>() {
                list.remove(&row);
            }
            self.list_for(update.state).append(&row);
            self.update_sections();
            self.check_queue();
        } else if !self.in_order(&row)
            && let Some(list) = row.parent().and_downcast::<gtk::ListBox>()
        {
            // Sorting reorders every row and lays the list out again, so it happens only
            // when this one has moved past a neighbour, not on every snapshot.
            list.invalidate_sort();
        }
    }

    /// Keeps the name a torrent's metadata gave it, for its next run and the next session.
    fn rename(&self, hash: &str, name: &str) {
        self.engine().rename(hash, name);
        let (key, name) = (hash.to_string(), name.to_string());
        self.storage().execute(move |s| {
            if let Err(e) = s.update_torrent_name(&key, &name) {
                log::warn!("Failed to save the name of {key}: {e}");
            }
        });
    }

    /// Saves a torrent seen for the first time and makes its row.
    fn insert_torrent(&self, update: &UiUpdate) -> Option<TorrentRow> {
        let mut record = SavedTorrent::new(
            update.info_hash.clone(),
            update.name.clone(),
            update.uri.clone(),
            state_key(update.state).to_string(),
            update.downloaded,
            update.total,
            update.output_dir.clone(),
        );
        record.total_pieces = update.total_pieces as u64;
        record.downloaded_pieces = update.downloaded_pieces as u64;
        record.sequential = update.sequential;
        // Written at once rather than queued: a torrent that cannot be saved is not kept.
        if let Err(e) = self.storage().save_torrent(&record) {
            log::warn!("Failed to save new torrent: {e}");
            self.engine().stop(&update.info_hash);
            self.show_toast(&gettext("Could not save the torrent: %s").replace("%s", &e));
            return None;
        }
        self.imp()
            .torrents
            .borrow_mut()
            .add(&update.info_hash, update.state, record.added_at);
        Some(self.make_row(&update.info_hash))
    }

    fn make_row(&self, hash: &str) -> TorrentRow {
        let row = TorrentRow::new(hash);
        self.bind_property("selection-mode", &row, "selection-mode")
            .sync_create()
            .build();
        row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.update_selection_actions()
        ));
        let query = self.imp().search_entry.text().to_lowercase();
        row.set_visible(row.matches(&query));
        self.imp()
            .rows
            .borrow_mut()
            .insert(hash.to_string(), row.clone());
        row
    }

    fn list_for(&self, state: TorrentUiState) -> gtk::ListBox {
        let imp = self.imp();
        match state {
            TorrentUiState::Downloading => imp.downloading_list.get(),
            TorrentUiState::Paused => imp.paused_list.get(),
            TorrentUiState::Completed | TorrentUiState::Error => imp.finished_list.get(),
        }
    }

    /// Shows the sections that have rows to show, or the page saying there are none.
    fn update_sections(&self) {
        let imp = self.imp();
        let any_in = |state| imp.rows.borrow().values().any(|row| row.state() == state);
        self.action_set_enabled("win.pause-all", any_in(TorrentUiState::Downloading));
        self.action_set_enabled("win.resume-all", any_in(TorrentUiState::Paused));
        let visible_rows = |list: &gtk::ListBox| {
            let mut child = list.first_child();
            while let Some(widget) = child {
                if widget.get_visible() {
                    return true;
                }
                child = widget.next_sibling();
            }
            false
        };
        let mut any = false;
        for (title, list) in [
            (&imp.downloading_title, &imp.downloading_list),
            (&imp.paused_title, &imp.paused_list),
            (&imp.finished_title, &imp.finished_list),
        ] {
            let shown = visible_rows(list);
            title.set_visible(shown);
            list.set_visible(shown);
            any |= shown;
        }
        let page = if any {
            "list"
        } else if imp.rows.borrow().is_empty() {
            "empty"
        } else {
            "no-results"
        };
        imp.content_stack.set_visible_child_name(page);
    }

    fn restore_torrents(&self, saved: Vec<SavedTorrent>) {
        let tx = self.sender();
        for torrent in saved {
            let state = self.imp().torrents.borrow_mut().restore(
                &torrent.info_hash,
                &torrent.state,
                torrent.added_at,
            );
            let update = UiUpdate {
                downloaded: torrent.downloaded,
                total: torrent.total,
                total_pieces: torrent.total_pieces as usize,
                downloaded_pieces: torrent.downloaded_pieces as usize,
                ..UiUpdate::idle(
                    torrent.info_hash.clone(),
                    torrent.name.clone(),
                    state,
                    torrent.output_dir_path(),
                    torrent.uri.clone(),
                    torrent.sequential,
                )
            };
            // Registered as paused, so that resuming starts the task.
            self.engine().add_paused_silent(
                torrent.info_hash.clone(),
                torrent.name.clone(),
                torrent.uri.clone(),
                torrent.output_dir_path(),
                torrent.sequential,
                tx.clone(),
            );
            let row = self.make_row(&torrent.info_hash);
            row.set_queued(self.imp().torrents.borrow().is_queued(&torrent.info_hash));
            row.update(&update);
            self.list_for(state).append(&row);
        }
        self.update_sections();
        // Torrents still stored as downloading are started once their files have been
        // looked for, so that one whose files are gone waits for the user instead.
        self.check_files(true, true);
        // Running torrents are looked at all along, since mtorrent would write on into
        // removed files; the others only while someone looks at the window, and as soon as
        // someone does, which is also when files were likely removed in another window.
        glib::timeout_add_local(
            FILE_CHECK_INTERVAL,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    window.check_files(window.is_active(), false);
                    glib::ControlFlow::Continue
                }
            ),
        );
        self.connect_is_active_notify(|window| {
            if window.is_active() {
                window.check_files(true, false);
            }
        });
    }

    /// Looks for the files of the running torrents, and with `idle_too` of the others as well,
    /// off the main thread, and shows the ones whose files were removed or cut short as
    /// paused, with what is left. With `then_queue`, the download queue runs once that is done.
    fn check_files(&self, idle_too: bool, then_queue: bool) {
        let imp = self.imp();
        if imp.checking_files.replace(true) {
            return;
        }
        // A failed torrent says so already, one with nothing downloaded has nothing to lose,
        // one already found missing stays so until it runs again, and one whose run has yet
        // to make its files is not missing them.
        let torrents: Vec<(String, TorrentUiState, UiUpdate)> = imp
            .rows
            .borrow()
            .iter()
            .filter(|(_, row)| {
                let state = row.state();
                state != TorrentUiState::Error
                    && (idle_too || state == TorrentUiState::Downloading)
                    && !row.files_missing()
                    && row.files_expected()
                    && (state == TorrentUiState::Completed || row.progress().0 > 0)
            })
            .filter_map(|(hash, row)| Some((hash.clone(), row.state(), row.latest()?)))
            .collect();
        let layouts = imp.layouts.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let missing = gio::spawn_blocking(move || {
                    torrents
                        .into_iter()
                        .filter_map(|(hash, state, update)| {
                            let layout = cached_layout(&layouts, &hash, &update);
                            // Only looked at: a running torrent may be writing its progress
                            // file, and the engine corrects it before the next run anyway.
                            let missing = torrent_paths::find_missing_content(
                                &update.uri,
                                &update.output_dir,
                                layout.as_deref(),
                                false,
                            )?;
                            Some((hash, state, update.output_dir, missing))
                        })
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                window.imp().checking_files.set(false);
                for (hash, state, output_dir, missing) in missing {
                    window.show_files_missing(&hash, state, &output_dir, missing);
                }
                if then_queue {
                    window.check_queue();
                }
            }
        ));
    }

    /// Shows a torrent found `missing` files as paused, provided it is still in the `state`
    /// and the folder it was looked for in. A running torrent is paused: mtorrent would go on
    /// writing to the removed files and count them as downloaded.
    fn show_files_missing(
        &self,
        hash: &str,
        state: TorrentUiState,
        output_dir: &Path,
        missing: torrent_paths::MissingContent,
    ) {
        let Some(row) = self.imp().rows.borrow().get(hash).cloned() else {
            return;
        };
        let moved = row
            .latest()
            .is_none_or(|update| update.output_dir != output_dir);
        if row.state() != state || moved {
            return;
        }
        log::warn!(
            "Files of {hash} are missing; {} bytes left",
            missing.present_bytes
        );
        match state {
            TorrentUiState::Downloading => self.pause_torrent(hash),
            // Its run is over, but the engine still counts it as running: resuming it must
            // start a new one.
            TorrentUiState::Completed if self.engine().is_active(hash) => {
                self.engine().toggle(hash)
            }
            _ => {}
        }
        // Not for the queue to start behind the user's back either.
        self.imp().torrents.borrow_mut().leave_queue(hash);
        row.show_files_missing();
        if let Some(mut update) = row.latest() {
            update.state = TorrentUiState::Paused;
            update.downloaded = missing.present_bytes.min(update.total);
            update.downloaded_pieces = missing.present_pieces as usize;
            self.process_update(&update);
        }
    }

    fn persist(&self, update: &UiUpdate) {
        let imp = self.imp();
        let state = imp
            .torrents
            .borrow()
            .stored_state(&update.info_hash, update.state);
        let snapshot = (
            state,
            update.downloaded,
            update.total,
            update.total_pieces as u64,
            update.downloaded_pieces as u64,
        );
        if !imp.torrents.borrow_mut().record_snapshot(
            &update.info_hash,
            snapshot,
            std::time::Instant::now(),
        ) {
            return;
        }

        let storage = self.storage().clone();
        let hash = update.info_hash.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = window)]
            self,
            async move {
                let key = hash.clone();
                let (state, downloaded, total, total_pieces, downloaded_pieces) = snapshot;
                let result = storage
                    .query(move |s| {
                        s.update_torrent_state(
                            &key,
                            state,
                            downloaded,
                            total,
                            total_pieces,
                            downloaded_pieces,
                        )
                    })
                    .await
                    .and_then(|r| r);
                if let Err(e) = result {
                    log::warn!("Failed to save the state of {hash}: {e}");
                    // Forget it, so that the next identical snapshot tries again.
                    window
                        .imp()
                        .torrents
                        .borrow_mut()
                        .snapshot_failed(&hash, snapshot);
                }
            }
        ));
    }

    /// Runs the download queue once the current burst of changes is over.
    pub fn check_queue(&self) {
        let imp = self.imp();
        if imp.queue_check_pending.replace(true) {
            return;
        }
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(50),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    window.imp().queue_check_pending.set(false);
                    window.run_queue();
                }
            ),
        );
    }

    /// Sets how many downloads may run at once, and applies it.
    pub fn set_download_limit(&self, limit: usize) {
        self.imp().download_limit.set(limit.max(1));
        self.check_queue();
    }

    /// Pauses the newest downloads above the limit, or starts the oldest waiting ones
    /// while there is room. A torrent the user paused is never started.
    fn run_queue(&self) {
        let imp = self.imp();
        let limit = imp.download_limit.get();
        let engine = self.engine();
        let plan = imp
            .torrents
            .borrow_mut()
            .plan_queue(limit, |hash| engine.is_active(hash));
        if !plan.pause.is_empty() {
            log::info!(
                "Downloads over the limit of {limit}; pausing {}",
                plan.pause.len()
            );
        }
        for hash in &plan.pause {
            engine.toggle(hash);
            self.show_queued(hash);
        }
        for hash in &plan.start {
            log::info!("Starting queued torrent {hash}");
            engine.toggle(hash);
            self.show_queued(hash);
        }
    }

    /// Lets a torrent's row say whether the queue holds it.
    fn show_queued(&self, hash: &str) {
        let queued = self.imp().torrents.borrow().is_queued(hash);
        let row = self.imp().rows.borrow().get(hash).cloned();
        if let Some(row) = row {
            row.set_queued(queued);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SortKey, SortOrder, compare};
    use std::cmp::Ordering;

    fn key(name: &str, total: u64, downloaded: u64, added: i64) -> SortKey {
        SortKey {
            name: name.to_string(),
            total,
            downloaded,
            added: (added, added as u64),
        }
    }

    #[test]
    fn an_order_falls_back_to_the_one_the_torrents_were_added_in() {
        let old = key("Zulu", 100, 50, 1);
        let new = key("Alpha", 100, 50, 2);

        assert_eq!(compare(SortOrder::Added, &old, &new), Ordering::Less);
        assert_eq!(compare(SortOrder::Name, &old, &new), Ordering::Greater);
        // Same name, same size, same progress: the older one comes first.
        let same = key("Zulu", 100, 50, 3);
        for order in [
            SortOrder::Added,
            SortOrder::Name,
            SortOrder::Size,
            SortOrder::Progress,
        ] {
            assert_eq!(compare(order, &old, &same), Ordering::Less, "{order:?}");
        }
    }

    #[test]
    fn the_biggest_and_the_most_complete_torrents_come_first() {
        let big = key("Big", 900, 90, 1);
        let small = key("Small", 100, 90, 2);

        assert_eq!(compare(SortOrder::Size, &big, &small), Ordering::Less);
        // 10% against 90%.
        assert_eq!(
            compare(SortOrder::Progress, &big, &small),
            Ordering::Greater
        );

        // A torrent whose size is not known yet is last by size and by progress.
        let unknown = key("Unknown", 0, 0, 3);
        assert_eq!(
            compare(SortOrder::Size, &unknown, &small),
            Ordering::Greater
        );
        assert_eq!(
            compare(SortOrder::Progress, &unknown, &big),
            Ordering::Greater
        );
    }

    #[test]
    fn an_unknown_stored_order_is_the_default_one() {
        assert_eq!(SortOrder::from_key("nonsense"), SortOrder::Added);
        for order in [
            SortOrder::Added,
            SortOrder::Name,
            SortOrder::Size,
            SortOrder::Progress,
        ] {
            assert_eq!(SortOrder::from_key(order.key()), order);
        }
    }
}
