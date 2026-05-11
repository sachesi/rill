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
        
        // Engine and model
        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub model: RefCell<Option<Rc<RefCell<TorrentModel>>>>,
        pub tx: RefCell<Option<Sender<UiEvent>>>,
        pub rows: RefCell<HashMap<String, RillRow>>,
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
        #[template_callback]
        fn on_add_clicked(&self, _button: &gtk::Button) {
            let engine = self.engine.borrow();
            let tx = self.tx.borrow();
            if let (Some(engine), Some(tx)) = (engine.as_ref(), tx.as_ref()) {
                show_add_dialog(&self.obj(), engine.clone(), tx.clone());
            }
        }
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
        window
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
                            // Handle torrent completion
                            if let Some(err) = error {
                                eprintln!("Torrent {} finished with error: {}", info_hash, err);
                            } else {
                                println!("Torrent {} completed successfully", info_hash);
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
}

fn list_for_state(state: TorrentUiState, imp: &imp::RillWindow) -> &gtk::ListBox {
    match state {
        TorrentUiState::Downloading => &imp.dl_list,
        TorrentUiState::Paused => &imp.pause_list,
        TorrentUiState::Completed | TorrentUiState::Error => &imp.done_list,
    }
}

fn show_add_dialog(_window: &RillWindow, _engine: Rc<RefCell<TorrentEngine>>, _tx: Sender<UiEvent>) {
    log::info!("Add dialog clicked - not implemented yet");
}