use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use gtk::subclass::prelude::*;

use crate::engine::{TorrentUiState, UiUpdate};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/com/github/sachesi/rill/ui/info-dialog.ui")]
    pub struct RillInfoDialog {
        #[template_child]
        pub name_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub state_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub size_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub progress_bar: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub speed_down_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub speed_up_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub peers_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub source_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub save_path_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub close_btn: TemplateChild<gtk::Button>,

        pub update: RefCell<Option<UiUpdate>>,
        pub shared_update: RefCell<Option<Rc<RefCell<Option<UiUpdate>>>>>,
        pub refresh_source: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillInfoDialog {
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

    impl ObjectImpl for RillInfoDialog {}
    impl WidgetImpl for RillInfoDialog {}
    impl WindowImpl for RillInfoDialog {}
}

glib::wrapper! {
    pub struct RillInfoDialog(ObjectSubclass<imp::RillInfoDialog>)
        @extends gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl RillInfoDialog {
    pub fn new(
        initial: &UiUpdate,
        shared_update: Rc<RefCell<Option<UiUpdate>>>,
        parent: &impl IsA<gtk::Window>,
    ) -> Self {
        let dialog: Self = glib::Object::builder()
            .property("transient-for", parent)
            .property("modal", true)
            .build();

        dialog.imp().update.replace(Some(initial.clone()));
        dialog.imp().shared_update.replace(Some(shared_update));
        dialog.apply_update(initial);

        // Close button
        let dlg_weak = dialog.downgrade();
        dialog.imp().close_btn.connect_clicked(move |_| {
            if let Some(d) = dlg_weak.upgrade() {
                if let Some(id) = d.imp().refresh_source.borrow_mut().take() {
                    id.remove();
                }
                d.close();
            }
        });

        // Auto-refresh every 500ms
        let dlg_weak = dialog.downgrade();
        let source_id = glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if let Some(d) = dlg_weak.upgrade() {
                if let Some(shared) = d.imp().shared_update.borrow().as_ref() {
                    if let Some(update) = shared.borrow().as_ref() {
                        d.apply_update(update);
                    }
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
        dialog.imp().refresh_source.replace(Some(source_id));

        dialog
    }

    pub fn refresh(&self, update: &UiUpdate) {
        self.imp().update.replace(Some(update.clone()));
        self.apply_update(update);
    }

    fn apply_update(&self, update: &UiUpdate) {
        let imp = self.imp();

        let name = if update.name.is_empty() {
            format!("Torrent {}", &update.info_hash[..8])
        } else {
            update.name.clone()
        };
        imp.name_lbl.set_label(&name);

        let state_text = match update.state {
            TorrentUiState::Downloading => "Downloading",
            TorrentUiState::Paused => "Paused",
            TorrentUiState::Completed => "Completed",
            TorrentUiState::Error => "Error",
        };
        imp.state_lbl.set_label(state_text);

        imp.size_lbl.set_label(&format!(
            "{} / {}",
            format_size(update.downloaded),
            format_size(update.total)
        ));

        let fraction = if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        };
        imp.progress_bar.set_fraction(fraction);
        imp.progress_bar.set_text(Some(&format!("{:.1}%", fraction * 100.0)));

        imp.speed_down_lbl.set_label(&format!("{}/s", format_size(update.speed_down)));
        imp.speed_up_lbl.set_label(&format!("{}/s", format_size(update.speed_up)));
        imp.peers_lbl.set_label(&format!("{}", update.peers));

        // Truncate long URIs
        let source = if update.uri.len() > 60 {
            format!("{}...", &update.uri[..57])
        } else {
            update.uri.clone()
        };
        imp.source_lbl.set_label(&source);
        imp.source_lbl.set_tooltip_text(Some(&update.uri));

        imp.save_path_lbl.set_label(&update.output_dir.to_string_lossy());
    }

    pub fn info_hash(&self) -> String {
        self.imp().update.borrow().as_ref().map(|u| u.info_hash.clone()).unwrap_or_default()
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}
