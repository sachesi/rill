use std::collections::HashMap;
use std::rc::Rc;
use std::cell::RefCell;


use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib, glib::clone, glib::subclass::prelude::*};
use gtk::subclass::prelude::*;
use async_channel::Sender;

use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};
use crate::model::TorrentModel;

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
        // Update model first
        if let Some(model) = self.imp().model.borrow().as_ref() {
            model.borrow_mut().update_torrent(update.clone());
        }
        
        // Update or create UI row
        let mut rows = self.imp().rows.borrow_mut();
        if let Some(existing_row) = rows.get(&update.info_hash) {
            existing_row.update(update);
        } else {
            // Create new row
            let engine = self.imp().engine.borrow();
            let tx = self.imp().tx.borrow();
            if let (Some(engine), Some(tx)) = (engine.as_ref(), tx.as_ref()) {
                let new_row = RillRow::new(update, engine, tx.clone());
                let row_widget = new_row.row.clone();
                rows.insert(update.info_hash.clone(), new_row);
                
                // Add to appropriate list
                let target_list = match update.state {
                    TorrentUiState::Downloading => &self.imp().dl_list,
                    TorrentUiState::Paused => &self.imp().pause_list,
                    TorrentUiState::Completed | TorrentUiState::Error => &self.imp().done_list,
                };
                target_list.append(&row_widget);
                
                // Show/hide sections as needed
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

// Torrent Row Widget (simplified for now, will be template-based later)
#[derive(Debug, Clone)]
pub struct RillRow {
    row: gtk::ListBoxRow,
    name_label: gtk::Label,
    status_label: gtk::Label,
    progress_bar: gtk::ProgressBar,
    action_btn: gtk::Button,
    state: Rc<RefCell<TorrentUiState>>,
}

impl RillRow {
    fn new(update: &UiUpdate, _engine: &Rc<RefCell<TorrentEngine>>, _tx: Sender<UiEvent>) -> Self {
        let row = gtk::ListBoxRow::builder()
            .activatable(false)
            .selectable(false)
            .build();

        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        // Icon
        let icon = gtk::Image::builder()
            .icon_name("folder-download-symbolic")
            .pixel_size(24)
            .css_classes(["torrent-icon"])
            .build();

        // Info box
        let info_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();

        let name_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();

        let status_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build();

        let progress_bar = gtk::ProgressBar::builder()
            .hexpand(true)
            .css_classes(["thin"])
            .build();

        info_box.append(&name_label);
        info_box.append(&status_label);
        info_box.append(&progress_bar);

        // Action button
        let action_btn = gtk::Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();

        row_box.append(&icon);
        row_box.append(&info_box);
        row_box.append(&action_btn);
        
        row.set_child(Some(&row_box));

        let rill_row = Self {
            row,
            name_label,
            status_label,
            progress_bar,
            action_btn,
            state: Rc::new(RefCell::new(update.state.clone())),
        };

        rill_row.update(update);
        rill_row
    }

    fn update(&self, update: &UiUpdate) {
        let name = if update.name.is_empty() {
            format!("Torrent {}", &update.info_hash[..8])
        } else {
            update.name.clone()
        };
        self.name_label.set_text(&name);
        
        let status = format!(
            "{} • {} peers",
            self.format_size(update.downloaded, update.total),
            update.peers
        );
        self.status_label.set_text(&status);
        
        let progress = if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        };
        self.progress_bar.set_fraction(progress);
        
        // Update button and state
        *self.state.borrow_mut() = update.state.clone();
        match update.state {
            TorrentUiState::Downloading => {
                self.action_btn.set_icon_name("media-playback-pause-symbolic");
            }
            TorrentUiState::Paused => {
                self.action_btn.set_icon_name("media-playback-start-symbolic");
            }
            TorrentUiState::Completed | TorrentUiState::Error => {
                self.action_btn.set_icon_name("user-trash-symbolic");
            }
        }
    }
    
    fn format_size(&self, downloaded: u64, total: u64) -> String {
        fn format_bytes(bytes: u64) -> String {
            const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
            let mut size = bytes as f64;
            let mut unit_index = 0;
            
            while size >= 1024.0 && unit_index < UNITS.len() - 1 {
                size /= 1024.0;
                unit_index += 1;
            }
            
            if unit_index == 0 {
                format!("{} {}", bytes, UNITS[0])
            } else {
                format!("{:.1} {}", size, UNITS[unit_index])
            }
        }
        
        format!("{} of {}", format_bytes(downloaded), format_bytes(total))
    }
}



fn show_add_dialog(_window: &RillWindow, _engine: Rc<RefCell<TorrentEngine>>, _tx: Sender<UiEvent>) {
    // TODO: Implement add dialog with template
    log::info!("Add dialog clicked - not implemented yet");
}