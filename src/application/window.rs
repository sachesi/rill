use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};


use async_channel::Sender;

use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};
use crate::model::TorrentModel;
use crate::storage::{SavedTorrent, Storage};

use super::torrent_row::RillRow;
use super::{RillAddDialog, AddMode};
use super::RillInfoDialog;

const APP_CSS: &str = "
progressbar.thin > trough,
progressbar.thin > trough > progress {
    min-height: 4px;
}

.torrent-icon-avatar {
    border-radius: 8px;
    min-width: 40px;
    min-height: 40px;
    padding: 0;
    -gtk-icon-size: 18px;
}
.torrent-icon-avatar image { -gtk-icon-size: 18px; }
.torrent-icon-avatar.accent { background-color: alpha(@accent_color, 0.18); color: @accent_color; }
.torrent-icon-avatar.success { background-color: alpha(@success_color, 0.15); color: @success_color; }
.torrent-icon-avatar.error { background-color: alpha(@error_color, 0.15); color: @error_color; }
.torrent-icon-avatar.dim { background-color: alpha(@card_fg_color, 0.06); color: alpha(@card_fg_color, 0.40); }
";

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct RillWindow {
        pub dl_list: RefCell<Option<gtk::ListBox>>,
        pub pause_list: RefCell<Option<gtk::ListBox>>,
        pub done_list: RefCell<Option<gtk::ListBox>>,
        pub dl_header: RefCell<Option<gtk::Label>>,
        pub pause_header: RefCell<Option<gtk::Label>>,
        pub done_header: RefCell<Option<gtk::Label>>,
        pub empty_page: RefCell<Option<adw::StatusPage>>,
        pub search_entry: RefCell<Option<gtk::SearchEntry>>,
        pub add_btn: RefCell<Option<gtk::MenuButton>>,
        pub title_stack: RefCell<Option<gtk::Stack>>,

        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub model: RefCell<Option<Rc<RefCell<TorrentModel>>>>,
        pub storage: RefCell<Option<Storage>>,
        pub tx: RefCell<Option<Sender<UiEvent>>>,
        pub rows: RefCell<HashMap<String, RillRow>>,
        pub info_dialogs: RefCell<HashMap<String, Rc<RillInfoDialog>>>,
        pub deleted_torrents: RefCell<std::collections::HashSet<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillWindow {
        const NAME: &'static str = "RillWindow";
        type Type = super::RillWindow;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for RillWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // ---- Widget creation ----

            let empty_page = adw::StatusPage::builder()
                .icon_name("folder-download-symbolic")
                .title("No Torrents")
                .description("Add a magnet link or .torrent file to get started")
                .vexpand(true)
                .hexpand(true)
                .build();

            let dl_header = gtk::Label::builder()
                .label("Downloading")
                .halign(gtk::Align::Start)
                .css_classes(["title-4"])
                .margin_start(6)
                .margin_top(12)
                .visible(false)
                .build();

            let dl_list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .visible(false)
                .build();

            let pause_header = gtk::Label::builder()
                .label("Paused")
                .halign(gtk::Align::Start)
                .css_classes(["title-4"])
                .margin_start(6)
                .margin_top(12)
                .visible(false)
                .build();

            let pause_list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .visible(false)
                .build();

            let done_header = gtk::Label::builder()
                .label("Completed")
                .halign(gtk::Align::Start)
                .css_classes(["title-4"])
                .margin_start(6)
                .margin_top(12)
                .visible(false)
                .build();

            let done_list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .visible(false)
                .build();

            let obj_weak1 = obj.downgrade();
            dl_list.connect_row_activated(move |_list, row| {
                if let Some(window) = obj_weak1.upgrade()
                    && let Ok(rill_row) = row.clone().downcast::<RillRow>()
                {
                    window.open_info_dialog(&rill_row);
                }
            });

            let obj_weak2 = obj.downgrade();
            pause_list.connect_row_activated(move |_list, row| {
                if let Some(window) = obj_weak2.upgrade()
                    && let Ok(rill_row) = row.clone().downcast::<RillRow>()
                {
                    window.open_info_dialog(&rill_row);
                }
            });

            let obj_weak3 = obj.downgrade();
            done_list.connect_row_activated(move |_list, row| {
                if let Some(window) = obj_weak3.upgrade()
                    && let Ok(rill_row) = row.clone().downcast::<RillRow>()
                {
                    window.open_info_dialog(&rill_row);
                }
            });

            let content_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
            content_box.append(&empty_page);
            content_box.append(&dl_header);
            content_box.append(&dl_list);
            content_box.append(&pause_header);
            content_box.append(&pause_list);
            content_box.append(&done_header);
            content_box.append(&done_list);

            let clamp = adw::Clamp::builder()
                .maximum_size(760)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(16)
                .margin_end(16)
                .build();
            clamp.set_child(Some(&content_box));

            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build();
            scroll.set_child(Some(&clamp));

            // ---- Header bar ----
            let window_title = adw::WindowTitle::new("Rill", "");

            let search_entry = gtk::SearchEntry::builder()
                .placeholder_text("Search torrents…")
                .hexpand(true)
                .build();

            let title_stack = gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .hhomogeneous(false)
                .vhomogeneous(false)
                .build();
            title_stack.add_named(&window_title, Some("title"));
            title_stack.add_named(&search_entry, Some("search"));

            let add_menu = gio::Menu::new();
            add_menu.append(Some("_Add Torrent File…"), Some("win.add-file"));
            add_menu.append(Some("_Add Magnet Link…"), Some("win.add-magnet"));

            let add_btn = gtk::MenuButton::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("Add Torrent")
                .menu_model(&add_menu)
                .build();

            // Menu
            let menu = gio::Menu::new();
            let prefs_section = gio::Menu::new();
            prefs_section.append(Some("_Preferences"), Some("app.preferences"));
            menu.append_section(None, &prefs_section);
            let about_section = gio::Menu::new();
            about_section.append(Some("_About Rill"), Some("app.about"));
            about_section.append(Some("_Quit"), Some("app.quit"));
            menu.append_section(None, &about_section);

            let menu_btn = gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .primary(true)
                .tooltip_text("Main Menu")
                .build();

            let header_bar = adw::HeaderBar::builder()
                .build();
            header_bar.set_title_widget(Some(&title_stack));
            header_bar.pack_start(&add_btn);
            header_bar.pack_end(&menu_btn);

            let toolbar_view = adw::ToolbarView::builder().build();
            toolbar_view.add_top_bar(&header_bar);
            toolbar_view.set_content(Some(&scroll));

            obj.set_content(Some(&toolbar_view));

            // Store
            self.dl_list.replace(Some(dl_list));
            self.pause_list.replace(Some(pause_list));
            self.done_list.replace(Some(done_list));
            self.dl_header.replace(Some(dl_header));
            self.pause_header.replace(Some(pause_header));
            self.done_header.replace(Some(done_header));
            self.empty_page.replace(Some(empty_page));
            self.search_entry.replace(Some(search_entry));
            self.add_btn.replace(Some(add_btn));
            self.title_stack.replace(Some(title_stack));
        }
    }
    impl WidgetImpl for RillWindow {}
    impl WindowImpl for RillWindow {}
    impl ApplicationWindowImpl for RillWindow {}
    impl AdwApplicationWindowImpl for RillWindow {}
}

glib::wrapper! {
    pub struct RillWindow(ObjectSubclass<imp::RillWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

// ---- Getters ----
impl RillWindow {
    fn dl_list(&self) -> gtk::ListBox {
        self.imp().dl_list.borrow().clone().unwrap()
    }
    fn pause_list(&self) -> gtk::ListBox {
        self.imp().pause_list.borrow().clone().unwrap()
    }
    fn done_list(&self) -> gtk::ListBox {
        self.imp().done_list.borrow().clone().unwrap()
    }
    fn dl_header(&self) -> gtk::Label {
        self.imp().dl_header.borrow().clone().unwrap()
    }
    fn pause_header(&self) -> gtk::Label {
        self.imp().pause_header.borrow().clone().unwrap()
    }
    fn done_header(&self) -> gtk::Label {
        self.imp().done_header.borrow().clone().unwrap()
    }
    fn empty_page(&self) -> adw::StatusPage {
        self.imp().empty_page.borrow().clone().unwrap()
    }
    fn search_entry(&self) -> gtk::SearchEntry {
        self.imp().search_entry.borrow().clone().unwrap()
    }
    #[allow(dead_code)]
    fn add_btn(&self) -> gtk::MenuButton {
        self.imp().add_btn.borrow().clone().unwrap()
    }
    fn title_stack(&self) -> gtk::Stack {
        self.imp().title_stack.borrow().clone().unwrap()
    }
}

// ---- Constructor ----
impl RillWindow {
    pub fn new(
        engine: Rc<RefCell<TorrentEngine>>,
        model: Rc<RefCell<TorrentModel>>,
        storage: Storage,
        saved_torrents: Vec<SavedTorrent>,
        app: &adw::Application,
    ) -> Self {
        let (tx, rx) = async_channel::unbounded();

        let css = gtk::CssProvider::new();
        css.load_from_string(APP_CSS);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let window: Self = glib::Object::builder()
            .property("application", app)
            .build();

        let settings = storage.load_settings();
        window.imp().engine.replace(Some(engine));
        window.imp().model.replace(Some(model));
        window.imp().storage.replace(Some(storage));
        window.imp().tx.replace(Some(tx));
        window.set_default_size(settings.window_width, settings.window_height);
        if settings.window_maximized {
            window.maximize();
        }

        window.restore_torrents(saved_torrents);
        window.setup_update_loop(rx);
        window.setup_key_controller();
        window.setup_search();
        window.setup_add_button();
        window.setup_dnd();
        window.setup_window_state_handlers();

        window
    }
}

// ---- Signaling / Setup ----
#[allow(clippy::single_match)]
impl RillWindow {
    fn setup_key_controller(&self) {
        let key_controller = gtk::EventControllerKey::new();
        let window = self.downgrade();
        key_controller.connect_key_pressed(move |_controller, keyval, _keycode, state| {
            let window = match window.upgrade() {
                Some(w) => w,
                None => return glib::Propagation::Proceed,
            };

            if state.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                if let Some(ref name) = keyval.name()
                    && &**name == "f"
                {
                    window.toggle_search();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }

            if let Some(ref name) = keyval.name() {
                match &**name as &str {
                    "Delete" => {
                        window.delete_selected();
                        return glib::Propagation::Stop;
                    }
                    "space" => {
                        window.toggle_selected();
                        return glib::Propagation::Stop;
                    }
                    "Escape" if window.search_entry().is_visible() => {
                        window.search_entry().set_text("");
                        window.search_entry().set_visible(false);
                        window.title_stack().set_visible_child_name("title");
                        window.clear_search_filter();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
            }
            glib::Propagation::Proceed
        });
        self.add_controller(key_controller);
    }

    fn toggle_search(&self) {
        let showing = self.title_stack().visible_child_name().as_deref() == Some("search");
        if showing {
            self.search_entry().set_text("");
            self.title_stack().set_visible_child_name("title");
            self.clear_search_filter();
        } else {
            self.title_stack().set_visible_child_name("search");
            self.search_entry().grab_focus();
        }
    }

    fn setup_search(&self) {
        let window = self.downgrade();
        self.search_entry().connect_search_changed(move |entry| {
            let window = match window.upgrade() {
                Some(w) => w,
                None => return,
            };
            let query = entry.text().to_string();
            Self::apply_search_filter(&window, &query);
        });
    }

    fn apply_search_filter(&self, query: &str) {
        let rows = self.imp().rows.borrow();
        if query.is_empty() {
            for row in rows.values() {
                row.set_visible(true);
            }
        } else {
            let lower = query.to_lowercase();
            for (id, row) in rows.iter() {
                let name_lower = row.name().to_lowercase();
                row.set_visible(name_lower.contains(&lower) || id.contains(&lower));
            }
        }
        self.update_section_visibility();
    }

    fn clear_search_filter(&self) {
        let rows = self.imp().rows.borrow();
        for row in rows.values() {
            row.set_visible(true);
        }
        self.update_section_visibility();
    }

    fn selected_row(&self) -> Option<RillRow> {
        for list in [self.dl_list(), self.pause_list(), self.done_list()] {
            if let Some(row) = list.selected_row()
                && let Ok(rill_row) = row.downcast::<RillRow>()
            {
                return Some(rill_row);
            }
        }
        None
    }

    fn toggle_selected(&self) {
        if let Some(row) = self.selected_row() {
            let state = row.state();
            if matches!(state, TorrentUiState::Downloading | TorrentUiState::Paused)
                && let Some(engine) = self.imp().engine.borrow().as_ref()
            {
                engine.borrow().toggle(&row.info_hash());
                self.enforce_queue_limits();
            }
        }
    }

    fn delete_selected(&self) {
        if let Some(row) = self.selected_row() {
            self.delete_torrent(&row.info_hash());
        }
    }

    pub fn allow_torrent(&self, info_hash: &str) {
        self.imp().deleted_torrents.borrow_mut().remove(info_hash);
    }

    pub fn delete_torrent(&self, info_hash: &str) {
        let torrent_name = {
            let rows = self.imp().rows.borrow();
            if let Some(row) = rows.get(info_hash) {
                row.name()
            } else {
                "this torrent".to_string()
            }
        };

        let delete_files_check = gtk::CheckButton::builder()
            .label("Delete downloaded data files")
            .active(false)
            .build();

        let dialog = adw::MessageDialog::builder()
            .transient_for(self)
            .modal(true)
            .heading("Delete Torrent?")
            .body(format!("Are you sure you want to delete \"{}\"? This action cannot be undone.", torrent_name))
            .extra_child(&delete_files_check)
            .build();

        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let window_weak = self.downgrade();
        let hash_str = info_hash.to_string();
        dialog.connect_response(None, move |dialog, response| {
            if response == "delete" && let Some(window) = window_weak.upgrade() {
                let delete_files = delete_files_check.is_active();
                window.delete_torrent_confirmed(&hash_str, delete_files);
            }
            dialog.destroy();
        });

        dialog.present();
    }

    fn delete_torrent_confirmed(&self, info_hash: &str, delete_files: bool) {
        self.imp().deleted_torrents.borrow_mut().insert(info_hash.to_string());

        let mut output_dir_to_remove: Option<std::path::PathBuf> = None;
        let mut torrent_name: Option<String> = None;

        if delete_files
            && let Some(storage) = self.imp().storage.borrow().as_ref()
            && let Ok(saved) = storage.load_torrents()
            && let Some(torrent) = saved.iter().find(|t| t.info_hash == info_hash)
        {
            output_dir_to_remove = Some(torrent.output_dir_path());
            torrent_name = Some(torrent.name.clone());
        }

        if let Some(engine) = self.imp().engine.borrow().as_ref() {
            engine.borrow().stop(info_hash);
        }
        if let Some(storage) = self.imp().storage.borrow().as_ref()
            && let Err(e) = storage.delete_torrent(info_hash)
        {
            log::warn!("Failed to delete torrent from database: {}", e);
        }

        if delete_files
            && let (Some(dir), Some(name)) = (output_dir_to_remove, torrent_name)
        {
            let torrent_path = dir.join(&name);
            if torrent_path.exists() {
                log::info!("Deleting torrent data path: {}", torrent_path.display());
                if torrent_path.is_dir() {
                    if let Err(e) = std::fs::remove_dir_all(&torrent_path) {
                        log::warn!("Failed to delete torrent data directory {}: {}", torrent_path.display(), e);
                    }
                } else {
                    if let Err(e) = std::fs::remove_file(&torrent_path) {
                        log::warn!("Failed to delete torrent data file {}: {}", torrent_path.display(), e);
                    }
                }
            } else {
                log::warn!("Torrent data path does not exist: {}", torrent_path.display());
            }
        }

        let row_opt = self.imp().rows.borrow_mut().remove(info_hash);
        if let Some(row) = row_opt
            && let Some(parent) = row.parent()
            && let Ok(list_box) = parent.downcast::<gtk::ListBox>()
        {
            list_box.remove(&row);
        }
        if let Some(model) = self.imp().model.borrow().as_ref() {
            model.borrow_mut().remove_torrent(info_hash);
        }
        self.update_section_visibility();
        self.enforce_queue_limits();
    }

    fn setup_window_state_handlers(&self) {
        let storage = self.imp().storage.borrow().clone();
        self.connect_close_request(move |window| {
            if let Some(storage) = storage.as_ref() {
                let mut settings = storage.load_settings();
                let (width, height) = window.default_size();
                settings.window_width = width;
                settings.window_height = height;
                settings.window_maximized = window.is_maximized();
                if let Err(e) = storage.save_settings(&settings) {
                    log::warn!("Failed to save window state: {}", e);
                } else {
                    log::info!(
                        "Window state saved: {}x{} (maximized: {})",
                        width,
                        height,
                        settings.window_maximized
                    );
                }
            }

            // Check if any torrent is active (downloading)
            let has_active = window.imp().rows.borrow().values().any(|row| {
                row.state() == TorrentUiState::Downloading
            });

            if has_active {
                log::info!("Active torrents present, hiding window instead of exiting");
                #[allow(deprecated)]
                window.hide();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }

    fn setup_dnd(&self) {
        use std::path::PathBuf;

        let drop_target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::COPY);
        let window = self.downgrade();

        drop_target.connect_drop(move |_target, value, _x, _y| {
            let window = match window.upgrade() {
                Some(w) => w,
                None => return false,
            };
            let engine = match window.imp().engine.borrow().as_ref().cloned() {
                Some(e) => e,
                None => return false,
            };
            let tx = match window.imp().tx.borrow().as_ref().cloned() {
                Some(t) => t,
                None => return false,
            };

            let uris: String = value.get().unwrap_or_default();
            if let Some(path) = uris.lines().next() {
                let path = path.trim();
                let path = path.strip_prefix("file://").unwrap_or(path);
                let path = PathBuf::from(path);
                if let Some(ext) = path.extension()
                    && ext == "torrent"
                {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    let dir = dirs_next::download_dir().unwrap_or_else(|| PathBuf::from("."));
                    glib::spawn_future_local(async move {
                        engine
                            .borrow_mut()
                            .start(name, path.to_string_lossy().to_string(), dir, tx);
                    });
                    return true;
                }
            }
            false
        });
        self.add_controller(drop_target);
    }

    fn setup_add_button(&self) {
        let window_weak = self.downgrade();

        let add_file_action = gio::SimpleAction::new("add-file", None);
        let weak1 = window_weak.clone();
        add_file_action.connect_activate(move |_, _| {
            if let Some(window) = weak1.upgrade() {
                window.add_torrent_file();
            }
        });
        self.add_action(&add_file_action);

        let add_magnet_action = gio::SimpleAction::new("add-magnet", None);
        let weak2 = window_weak.clone();
        add_magnet_action.connect_activate(move |_, _| {
            if let Some(window) = weak2.upgrade() {
                let engine = window.imp().engine.borrow().clone();
                let tx = window.imp().tx.borrow().clone();
                let storage = window.imp().storage.borrow().clone();
                if let (Some(engine), Some(tx), Some(storage)) = (engine, tx, storage) {
                    show_add_dialog(&window, engine, tx, storage);
                }
            }
        });
        self.add_action(&add_magnet_action);
    }

    fn add_torrent_file(&self) {
        let file_chooser = gtk::FileDialog::builder()
            .title("Select Torrent File")
            .modal(true)
            .build();

        let window_weak = self.downgrade();
        file_chooser.open(Some(self), gtk::gio::Cancellable::NONE, move |result| {
            let window = match window_weak.upgrade() {
                Some(w) => w,
                None => return,
            };
            let file = match result {
                Ok(f) => f,
                Err(_) => return,
            };
            if let Some(path) = file.path() {
                window.show_add_dialog_with_file(&path);
            }
        });
    }

    fn setup_update_loop(&self, rx: async_channel::Receiver<UiEvent>) {
        let window = self.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if let Some(window) = window.upgrade() {
                while let Ok(event) = rx.try_recv() {
                    match event {
                        UiEvent::Update(update) => {
                            window.process_update(&update);
                        }
                        UiEvent::Finished { info_hash, error } => match error {
                            Some(err) => {
                                if err == "user_requested_removal" {
                                    window.delete_torrent(&info_hash);
                                } else {
                                    log::error!("Torrent {} finished with error: {}", info_hash, err);
                                }
                            }
                            None => {
                                if let Some(storage) = window.imp().storage.borrow().as_ref()
                                    && let Err(e) = storage.mark_completed(&info_hash)
                                {
                                    log::warn!(
                                        "Failed to mark torrent as completed: {}",
                                        e
                                    );
                                }
                                let name = window
                                    .imp()
                                    .rows
                                    .borrow()
                                    .get(&info_hash)
                                    .map(|r| r.name())
                                    .unwrap_or_else(|| info_hash.clone());
                                let notification =
                                    gio::Notification::new("Download Complete");
                                notification.set_body(Some(
                                    &format!("{} has finished downloading", name),
                                ));
                                if let Some(app) = window.application() {
                                    app.send_notification(None, &notification);
                                }
                            }
                        },
                    }
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    fn process_update(&self, update: &UiUpdate) {
        if self.imp().deleted_torrents.borrow().contains(&update.info_hash) {
            log::debug!("Ignoring update for deleted torrent: {}", update.info_hash);
            return;
        }

        if let Some(model) = self.imp().model.borrow().as_ref() {
            model.borrow_mut().update_torrent(update.clone());
        }

        let mut rows = self.imp().rows.borrow_mut();
        if let Some(existing_row) = rows.get(&update.info_hash) {
            let old_state = existing_row.state();
            existing_row.update(update);

            self.save_torrent_to_db(update);

            if old_state != update.state {
                if let Some(parent) = existing_row.parent()
                    && let Ok(list_box) = parent.downcast::<gtk::ListBox>()
                {
                    list_box.remove(existing_row);
                }
                let target_list = list_for_state(update.state, self);
                target_list.append(existing_row);
                self.update_section_visibility();
                drop(rows);
                self.enforce_queue_limits();
            }
        } else {
            if let Some(storage) = self.imp().storage.borrow().as_ref() {
                let torrent = SavedTorrent::new(
                    update.info_hash.clone(),
                    update.name.clone(),
                    update.uri.clone(),
                    "paused".to_string(),
                    update.downloaded,
                    update.total,
                    update.output_dir.clone(),
                );
                if let Err(e) = storage.save_torrent(&torrent) {
                    log::warn!("Failed to save new torrent: {}", e);
                }
            }

            let engine = self.imp().engine.borrow();
            let tx = self.imp().tx.borrow();
            if let (Some(engine), Some(tx)) = (engine.as_ref(), tx.as_ref()) {
                let new_row = RillRow::new(update.info_hash.clone(), engine.clone(), tx.clone());
                new_row.update(update);
                rows.insert(update.info_hash.clone(), new_row.clone());

                let target_list = list_for_state(update.state, self);
                target_list.append(&new_row);
                self.update_section_visibility();
                drop(rows);
                self.enforce_queue_limits();
            }
        }
    }

    fn update_section_visibility(&self) {
        let dl_has = self.dl_list().first_child().is_some();
        let pause_has = self.pause_list().first_child().is_some();
        let done_has = self.done_list().first_child().is_some();

        self.dl_header().set_visible(dl_has);
        self.dl_list().set_visible(dl_has);
        self.pause_header().set_visible(pause_has);
        self.pause_list().set_visible(pause_has);
        self.done_header().set_visible(done_has);
        self.done_list().set_visible(done_has);

        self.empty_page().set_visible(!(dl_has || pause_has || done_has));
    }

    pub fn present(&self) {
        gtk::prelude::GtkWindowExt::present(self);
    }

    pub fn open_info_dialog(&self, rill_row: &RillRow) {
        let info_hash = rill_row.info_hash();
        let mut dialogs = self.imp().info_dialogs.borrow_mut();
        
        let mut needs_create = true;
        if let Some(dialog) = dialogs.get(&info_hash) {
            if dialog.is_visible() {
                dialog.present();
                needs_create = false;
            } else {
                dialogs.remove(&info_hash);
            }
        }

        if needs_create && rill_row.imp().latest_update.borrow().is_some() {
            let (w_width, w_height) = self.default_size();
            let target_width = (w_width - 60).max(540);
            let target_height = (w_height - 40).max(500);

            let dialog = RillInfoDialog::new(
                rill_row.latest_update(),
                &rill_row.name(),
            );
            dialog.set_transient_for(Some(self));
            dialog.set_modal(true);
            dialog.set_default_width(target_width);
            dialog.set_default_height(target_height);

            dialog.present();
            dialogs.insert(info_hash.clone(), Rc::new(dialog.clone()));

            let window_weak = self.downgrade();
            let hash_str = info_hash.clone();
            dialog.connect_destroy(move |_| {
                if let Some(w) = window_weak.upgrade()
                    && let Ok(mut dialogs) = w.imp().info_dialogs.try_borrow_mut()
                {
                    dialogs.remove(&hash_str);
                }
            });
        }
    }

    pub fn show_add_dialog(&self) {
        let engine = self.imp().engine.borrow();
        let tx = self.imp().tx.borrow();
        let storage = self.imp().storage.borrow();
        if let (Some(engine), Some(tx), Some(storage)) =
            (engine.as_ref(), tx.as_ref(), storage.as_ref())
        {
            show_add_dialog(self, engine.clone(), tx.clone(), storage.clone());
        }
    }

    pub fn show_add_dialog_with_uri(&self, uri: &str) {
        let engine = self.imp().engine.borrow();
        let tx = self.imp().tx.borrow();
        let storage = self.imp().storage.borrow();
        if let (Some(engine), Some(tx), Some(storage)) =
            (engine.as_ref(), tx.as_ref(), storage.as_ref())
        {
            let dialog = RillAddDialog::new(engine.clone(), tx.clone(), storage.clone(), self, AddMode::Magnet);
            dialog.set_uri(uri);
            dialog.present();
        }
    }

    pub fn show_add_dialog_with_file(&self, path: &std::path::Path) {
        let engine = self.imp().engine.borrow();
        let tx = self.imp().tx.borrow();
        let storage = self.imp().storage.borrow();
        if let (Some(engine), Some(tx), Some(storage)) =
            (engine.as_ref(), tx.as_ref(), storage.as_ref())
        {
            let dialog = RillAddDialog::new(engine.clone(), tx.clone(), storage.clone(), self, AddMode::File);
            dialog.set_selected_file(path);
            dialog.present();
        }
    }

    fn restore_torrents(&self, saved_torrents: Vec<SavedTorrent>) {
        for torrent in saved_torrents {
            log::info!("Restoring torrent: {} ({})", torrent.name, torrent.state);

            let state = match torrent.state.as_str() {
                "downloading" | "paused" => TorrentUiState::Paused,
                "completed" => TorrentUiState::Completed,
                "error" => TorrentUiState::Error,
                _ => TorrentUiState::Paused,
            };

            let update = UiUpdate {
                info_hash: torrent.info_hash.clone(),
                name: torrent.name.clone(),
                state,
                downloaded: torrent.downloaded,
                total: torrent.total,
                peers: 0,
                speed_down: 0,
                speed_up: 0,
                output_dir: torrent.output_dir_path(),
                uri: torrent.uri.clone(),
                peers_list: Vec::new(),
                total_pieces: torrent.total_pieces as usize,
                downloaded_pieces: torrent.downloaded_pieces as usize,
            };

            if let Some(model) = self.imp().model.borrow().as_ref() {
                model.borrow_mut().update_torrent(update.clone());
            }

            let engine = self.imp().engine.borrow();
            let tx = self.imp().tx.borrow();
            if let (Some(engine), Some(tx)) = (engine.as_ref(), tx.as_ref()) {
                // Register in backend engine to support resume/toggle operations
                engine.borrow().add_paused_silent(
                    torrent.name.clone(),
                    torrent.uri.clone(),
                    torrent.output_dir_path(),
                    tx.clone(),
                );

                let new_row = RillRow::new(torrent.info_hash.clone(), engine.clone(), tx.clone());
                new_row.update(&update);
                self.imp()
                    .rows
                    .borrow_mut()
                    .insert(torrent.info_hash.clone(), new_row.clone());
                let target_list = list_for_state(state, self);
                target_list.append(&new_row);
            }
        }
        self.update_section_visibility();
    }

    fn save_torrent_to_db(&self, update: &UiUpdate) {
        if let Some(storage) = self.imp().storage.borrow().as_ref() {
            let state_str = match update.state {
                TorrentUiState::Downloading => "downloading",
                TorrentUiState::Paused => "paused",
                TorrentUiState::Completed => "completed",
                TorrentUiState::Error => "error",
            };
            if let Err(e) =
                storage.update_torrent_state(
                    &update.info_hash,
                    state_str,
                    update.downloaded,
                    update.total,
                    update.total_pieces as u64,
                    update.downloaded_pieces as u64,
                )
            {
                log::warn!("Failed to save torrent state: {}", e);
            }
        }
    }

    pub fn enforce_queue_limits(&self) {
        let storage = match self.imp().storage.borrow().as_ref() {
            Some(s) => s.clone(),
            None => return,
        };
        let settings = storage.load_settings();
        let max_dl = settings.max_active_downloads as usize;

        // Get all rows
        let rows = self.imp().rows.borrow();
        let mut active_downloads = 0;
        let mut queued_torrents: Vec<(String, i64)> = Vec::new();

        // Load all torrents from DB
        let saved = storage.load_torrents().unwrap_or_default();
        let added_at_map: HashMap<String, i64> = saved.iter().map(|t| (t.info_hash.clone(), t.added_at)).collect();
        let state_map: HashMap<String, String> = saved.iter().map(|t| (t.info_hash.clone(), t.state.clone())).collect();

        let engine_ref = self.imp().engine.borrow();
        let engine = match engine_ref.as_ref() {
            Some(e) => e,
            None => return,
        };

        for info_hash in rows.keys() {
            let db_state = state_map.get(info_hash).cloned().unwrap_or_default();
            if db_state == "completed" {
                continue;
            }

            if engine.borrow().is_active(info_hash) {
                active_downloads += 1;
            } else if db_state == "paused" || db_state == "downloading" {
                let added_at = added_at_map.get(info_hash).cloned().unwrap_or(0);
                queued_torrents.push((info_hash.clone(), added_at));
            }
        }

        queued_torrents.sort_by_key(|&(_, added_at)| added_at);

        if active_downloads > max_dl {
            let mut downloading_with_time: Vec<(String, i64)> = rows.keys()
                .filter(|hash| {
                    state_map.get(*hash).map(|s| s.as_str()) != Some("completed") && engine.borrow().is_active(hash)
                })
                .map(|hash| {
                    let t = added_at_map.get(hash).cloned().unwrap_or(0);
                    (hash.clone(), t)
                })
                .collect();
            downloading_with_time.sort_by_key(|&(_, added_at)| -added_at);

            let excess = active_downloads - max_dl;
            log::info!("Active downloads limit exceeded ({} > {}). Auto-pausing {} torrent(s).", active_downloads, max_dl, excess);
            for (info_hash, _) in downloading_with_time.iter().take(excess) {
                log::info!("Auto-pausing torrent: {}", info_hash);
                engine.borrow().toggle(info_hash);
            }
        } else if active_downloads < max_dl && !queued_torrents.is_empty() {
            let capacity = max_dl - active_downloads;
            let to_resume = capacity.min(queued_torrents.len());
            log::info!("Below active downloads limit ({} < {}). Auto-resuming {} torrent(s).", active_downloads, max_dl, to_resume);
            for (info_hash, _) in queued_torrents.iter().take(to_resume) {
                log::info!("Auto-resuming torrent: {}", info_hash);
                engine.borrow().toggle(info_hash);
            }
        }
    }
}

fn list_for_state(state: TorrentUiState, window: &RillWindow) -> gtk::ListBox {
    match state {
        TorrentUiState::Downloading => window.dl_list(),
        TorrentUiState::Paused => window.pause_list(),
        TorrentUiState::Completed | TorrentUiState::Error => window.done_list(),
    }
}

fn show_add_dialog(
    window: &RillWindow,
    engine: Rc<RefCell<TorrentEngine>>,
    tx: Sender<UiEvent>,
    storage: Storage,
) {
    let dialog = RillAddDialog::new(engine, tx, storage, window, AddMode::Magnet);
    dialog.present();
}
