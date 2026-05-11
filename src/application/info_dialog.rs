use std::cell::RefCell;
use std::rc::Rc;

use gtk::{gio, glib};
use adw::prelude::*;
use gtk::subclass::prelude::*;

use crate::engine::{TorrentUiState, UiUpdate};

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/com/github/sachesi/rill/ui/info-dialog.ui")]
    pub struct RillInfoDialog {
        #[template_child]
        pub title_lbl: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub state_lbl: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub progress_bar: gtk::TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub size_lbl: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub speed_down_lbl: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub speed_up_lbl: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub peers_lbl: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub source_lbl: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub save_path_lbl: gtk::TemplateChild<gtk::Label>,

        pub shared_update: RefCell<Option<Rc<RefCell<Option<UiUpdate>>>>>,
    }

    #[glib::object_subclass]
    impl glib::subclass::types::ObjectSubclass for RillInfoDialog {
        const NAME: &'static str = "RillInfoDialog";
        type Type = super::RillInfoDialog;
        type ParentType = gtk::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl glib::subclass::object::ObjectImpl for RillInfoDialog {}
    impl gtk::subclass::widget::WidgetImpl for RillInfoDialog {}
    impl gtk::subclass::window::WindowImpl for RillInfoDialog {}
}

glib::wrapper! {
    pub struct RillInfoDialog(ObjectSubclass<imp::RillInfoDialog>)
        @extends gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl RillInfoDialog {
    pub fn new(shared_update: Rc<RefCell<Option<UiUpdate>>>, name: &str) -> Self {
        let obj: Self = glib::Object::builder().build();
        
        // Set window title to torrent name
        obj.imp().title_lbl.set_text(name);
        
        // Store the shared update reference
        *obj.imp().shared_update.borrow_mut() = Some(shared_update.clone());
        
        // Apply initial update if available
        if let Some(update) = shared_update.borrow().as_ref() {
            obj.apply_update(update);
        }
        
        // Set up auto-refresh timer
        let obj_weak = obj.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if let Some(obj) = obj_weak.upgrade() {
                if let Some(shared_ref) = obj.imp().shared_update.borrow().as_ref() {
                    if let Some(update) = shared_ref.borrow().as_ref() {
                        obj.apply_update(update);
                    }
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        
        obj.set_modal(true);
        obj
    }

    pub fn apply_update(&self, update: &UiUpdate) {
        let imp = self.imp();
        
        // State
        let state_text = match update.state {
            TorrentUiState::Downloading => "Downloading",
            TorrentUiState::Paused => "Paused", 
            TorrentUiState::Completed => "Completed",
            TorrentUiState::Error => "Error",
        };
        imp.state_lbl.set_text(state_text);
        
        // Progress
        let progress = if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        };
        imp.progress_bar.set_fraction(progress);
        imp.progress_bar.set_text(Some(&format!("{:.1}%", progress * 100.0)));
        
        // Size
        let size_text = if update.total > 0 {
            format!(
                "{} of {}",
                format_size(update.downloaded), 
                format_size(update.total)
            )
        } else {
            format_size(update.downloaded)
        };
        imp.size_lbl.set_text(&size_text);
        
        // Speeds
        imp.speed_down_lbl.set_text(&format!("{}/s", format_size(update.speed_down)));
        imp.speed_up_lbl.set_text(&format!("{}/s", format_size(update.speed_up)));
        
        // Peers
        imp.peers_lbl.set_text(&update.peers.to_string());
        
        // Source and save path
        imp.source_lbl.set_text(&update.uri);
        imp.save_path_lbl.set_text(&update.output_dir.to_string_lossy());
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    const THRESHOLD: f64 = 1024.0;
    
    if bytes == 0 {
        return "0 B".to_string();
    }
    
    let mut size = bytes as f64;
    let mut unit_index = 0;
    
    while size >= THRESHOLD && unit_index < UNITS.len() - 1 {
        size /= THRESHOLD;
        unit_index += 1;
    }
    
    if unit_index == 0 {
        format!("{:.0} {}", size, UNITS[unit_index])
    } else if size >= 100.0 {
        format!("{:.0} {}", size, UNITS[unit_index]) 
    } else if size >= 10.0 {
        format!("{:.1} {}", size, UNITS[unit_index])
    } else {
        format!("{:.2} {}", size, UNITS[unit_index])
    }
}