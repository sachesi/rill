use std::collections::HashMap;
use std::rc::Rc;
use std::cell::RefCell;


use adw::prelude::*;
use gtk::{gio, glib};
use gtk::subclass::prelude::*;
use adw::subclass::prelude::*;
use async_channel::Sender;

use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};
use crate::model::TorrentModel;
use super::torrent_row::RillRow;
use super::RillAddDialog;
use super::RillInfoDialog;

const APP_CSS: &str = "
progressbar.thin > trough,
progressbar.thin > trough > progress {
    min-height: 4px;
}
.torrent-icon.accent {
    color: @accent_color;
}
.torrent-icon.success {
    color: @success_color;
}
.torrent-icon.error {
    color: @error_color;
}
.torrent-icon.dim {
    opacity: 0.55;
}
";

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/com/github/sachesi/rill/ui/window.ui")]
    pub struct RillWindow {
        // Template children
        #[template_child]
        pub toolbar_view: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub add_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub menu_btn: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub scroll: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub clamp: TemplateChild<adw::Clamp>,
        #[template_child]
        pub content_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub empty_page: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub dl_header: TemplateChild<gtk::Label>,
        #[template_child]
        pub dl_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub pause_header: TemplateChild<gtk::Label>,
        #[template_child]
        pub pause_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub done_header: TemplateChild<gtk::Label>,
        #[template_child]
        pub done_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,

        // Engine and model
        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub model: RefCell<Option<Rc<RefCell<TorrentModel>>>>,
        pub tx: RefCell<Option<Sender<UiEvent>>>,
        pub rows: RefCell<HashMap<String, RillRow>>,
        pub info_dialogs: RefCell<HashMap<String, Rc<RillInfoDialog>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillWindow {
        const NAME: &'static str = "RillWindow";
        type Type = super::RillWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl RillWindow {
    }

    impl ObjectImpl for RillWindow {}
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

impl RillWindow {
    pub fn new(
        engine: Rc<RefCell<TorrentEngine>>,
        model: Rc<RefCell<TorrentModel>>,
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
        
        // Store engine, model, and tx
        window.imp().engine.replace(Some(engine));
        window.imp().model.replace(Some(model));
        window.imp().tx.replace(Some(tx));
        
        window.setup_update_loop(rx);
        window.setup_key_controller();
        window.setup_search();
        window.setup_add_button();
        window.setup_dnd();
        window.setup_breakpoints();
        window
    }

    fn setup_key_controller(&self) {
        let key_controller = gtk::EventControllerKey::new();
        let window = self.downgrade();
        key_controller.connect_key_pressed(move |_controller, keyval, _keycode, state| {
            let window = match window.upgrade() {
                Some(w) => w,
                None => return glib::Propagation::Proceed,
            };

            if state.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                if let Some(ref name) = keyval.name() {
                    if &**name == "f" {
                        window.toggle_search();
                        return glib::Propagation::Stop;
                    }
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
                    "Escape" => {
                        if window.imp().search_entry.is_visible() {
                            window.imp().search_entry.set_text("");
                            window.imp().search_entry.set_visible(false);
                            window.imp().window_title.set_visible(true);
                            window.clear_search_filter();
                            return glib::Propagation::Stop;
                        }
                    }
                    _ => {}
                }
            }
            glib::Propagation::Proceed
        });
        self.add_controller(key_controller);
    }

    fn toggle_search(&self) {
        let visible = self.imp().search_entry.is_visible();
        if visible {
            self.imp().search_entry.set_text("");
            self.imp().search_entry.set_visible(false);
            self.imp().window_title.set_visible(true);
            self.clear_search_filter();
        } else {
            self.imp().search_entry.set_visible(true);
            self.imp().window_title.set_visible(false);
            self.imp().search_entry.grab_focus();
        }
    }

    fn setup_search(&self) {
        let window = self.downgrade();
        let search_entry = self.imp().search_entry.clone();
        search_entry.connect_search_changed(move |entry| {
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
        for list in [&self.imp().dl_list, &self.imp().pause_list, &self.imp().done_list] {
            if let Some(row) = list.selected_row() {
                if let Ok(rill_row) = row.downcast::<RillRow>() {
                    return Some(rill_row);
                }
            }
        }
        None
    }

    fn toggle_selected(&self) {
        if let Some(row) = self.selected_row() {
            let state = row.state();
            match state {
                TorrentUiState::Downloading | TorrentUiState::Paused => {
                    if let Some(engine) = self.imp().engine.borrow().as_ref() {
                        engine.borrow().toggle(&row.info_hash());
                    }
                }
                _ => {}
            }
        }
    }

    fn delete_selected(&self) {
        if let Some(row) = self.selected_row() {
            let info_hash = row.info_hash();
            if let Some(engine) = self.imp().engine.borrow().as_ref() {
                engine.borrow().stop(&info_hash);
            }
            self.imp().dl_list.remove(&row);
            self.imp().pause_list.remove(&row);
            self.imp().done_list.remove(&row);
            self.imp().rows.borrow_mut().remove(&info_hash);
            if let Some(model) = self.imp().model.borrow().as_ref() {
                model.borrow_mut().remove_torrent(&info_hash);
            }
            self.update_section_visibility();
        }
    }

    fn setup_breakpoints(&self) {
        // Clamp handles responsive width automatically.
    }

    fn setup_dnd(&self) {
        use gtk::gdk;
        use std::path::PathBuf;

        let drop_target = gtk::DropTarget::default();
        drop_target.set_actions(gdk::DragAction::COPY);
        let window = self.downgrade();

        drop_target.connect_drop(move |target, value, _x, _y| {
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
                if let Some(ext) = path.extension() {
                    if ext == "torrent" {
                        let name = path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Unknown")
                            .to_string();
                        let dir = dirs_next::download_dir()
                            .unwrap_or_else(|| PathBuf::from("."));
                        glib::spawn_future_local(async move {
                            engine.borrow_mut().start(
                                name, path.to_string_lossy().to_string(), dir, tx,
                            );
                        });
                        return true;
                    }
                }
            }
            false
        });
        self.add_controller(drop_target);
    }

    fn setup_add_button(&self) {
        let engine = self.imp().engine.borrow().clone();
        let tx = self.imp().tx.borrow().clone();
        let window = self.downgrade();
        let add_btn = self.imp().add_btn.clone();
        add_btn.connect_clicked(move |_| {
            let window = match window.upgrade() {
                Some(w) => w,
                None => return,
            };
            let engine = match engine.as_ref() {
                Some(e) => e,
                None => return,
            };
            let tx = match tx.as_ref() {
                Some(t) => t,
                None => return,
            };
            show_add_dialog(&window, engine.clone(), tx.clone());
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
                        UiEvent::Finished { info_hash, error } => {
                            match error {
                                Some(err) => {
                                    eprintln!("Torrent {} finished with error: {}", info_hash, err);
                        }
                                None => {
                                    let name = window.imp().rows.borrow()
                                        .get(&info_hash)
                                        .map(|r| r.name())
                                        .unwrap_or_else(|| info_hash.clone());
                                    let notification = gio::Notification::new("Download Complete");
                                    notification.set_body(Some(&format!("{} has finished downloading", name)));
                                    if let Some(app) = window.application() {
                                        app.send_notification(None, &notification);
                                    }
                                }
                            }
                        }
                    }
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    fn process_update(&self, update: &UiUpdate) {
        if let Some(model) = self.imp().model.borrow().as_ref() {
            model.borrow_mut().update_torrent(update.clone());
        }
        
        let mut rows = self.imp().rows.borrow_mut();
        if let Some(existing_row) = rows.get(&update.info_hash) {
            let old_state = existing_row.state();
            existing_row.update(update);
            // Move to correct section if state changed
            if old_state != update.state {
                self.imp().dl_list.remove(existing_row);
                self.imp().pause_list.remove(existing_row);
                self.imp().done_list.remove(existing_row);
                let target_list = list_for_state(update.state, &self.imp());
                target_list.append(existing_row);
                self.update_section_visibility();
            }
        } else {
            let engine = self.imp().engine.borrow();
            let tx = self.imp().tx.borrow();
            if let (Some(engine), Some(tx)) = (engine.as_ref(), tx.as_ref()) {
                let new_row = RillRow::new(
                    update.info_hash.clone(),
                    engine.clone(),
                    tx.clone(),
                );
                new_row.update(update);
                rows.insert(update.info_hash.clone(), new_row.clone());
                
                let target_list = list_for_state(update.state, &self.imp());
                target_list.append(&new_row);
                self.update_section_visibility();
            }
        }
    }
    
    fn update_section_visibility(&self) {
        let dl_has_children = self.imp().dl_list.first_child().is_some();
        let pause_has_children = self.imp().pause_list.first_child().is_some();
        let done_has_children = self.imp().done_list.first_child().is_some();
        
        self.imp().dl_header.set_visible(dl_has_children);
        self.imp().dl_list.set_visible(dl_has_children);
        self.imp().pause_header.set_visible(pause_has_children);
        self.imp().pause_list.set_visible(pause_has_children);
        self.imp().done_header.set_visible(done_has_children);
        self.imp().done_list.set_visible(done_has_children);
        
        let has_any = dl_has_children || pause_has_children || done_has_children;
        self.imp().empty_page.set_visible(!has_any);
    }

    pub fn present(&self) {
        gtk::prelude::GtkWindowExt::present(self);
    }

    pub fn show_add_dialog(&self) {
        let engine = self.imp().engine.borrow();
        let tx = self.imp().tx.borrow();
        if let (Some(engine), Some(tx)) = (engine.as_ref(), tx.as_ref()) {
            show_add_dialog(self, engine.clone(), tx.clone());
        }
    }
}

fn list_for_state(state: TorrentUiState, imp: &imp::RillWindow) -> &gtk::ListBox {
    match state {
        TorrentUiState::Downloading => &imp.dl_list,
        TorrentUiState::Paused => &imp.pause_list,
        TorrentUiState::Completed | TorrentUiState::Error => &imp.done_list,
    }
}

fn show_add_dialog(window: &RillWindow, engine: Rc<RefCell<TorrentEngine>>, tx: Sender<UiEvent>) {
    let dialog = RillAddDialog::new(engine, tx, window);
    dialog.present();
}