use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use gtk::subclass::prelude::*;

use async_channel::Sender;
use crate::engine::{TorrentEngine, UiEvent};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/com/github/sachesi/rill/ui/add-dialog.ui")]
    pub struct RillAddDialog {
        #[template_child]
        pub toolbar_view: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub header_bar: TemplateChild<adw::HeaderBar>,
        #[template_child]
        pub cancel_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub add_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub url_entry: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub drop_area: TemplateChild<gtk::Frame>,
        #[template_child]
        pub drop_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub drop_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub folder_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub start_immediately_switch: TemplateChild<adw::SwitchRow>,

        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub tx: RefCell<Option<Sender<UiEvent>>>,
        pub selected_url: RefCell<Option<String>>,
        pub selected_file: RefCell<Option<PathBuf>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillAddDialog {
        const NAME: &'static str = "RillAddDialog";
        type Type = super::RillAddDialog;
        type ParentType = gtk::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl RillAddDialog {
        #[template_callback]
        fn on_cancel_clicked(&self) {
            self.obj().close();
        }

        #[template_callback]
        fn on_add_clicked(&self) {
            self.start_torrent();
        }

        #[template_callback]
        fn on_url_apply(&self) {
            self.start_torrent();
        }

        #[template_callback]
        fn on_url_changed(&self) {
            let text = self.url_entry.text().to_string();
            let has_text = !text.trim().is_empty();
            *self.selected_url.borrow_mut() = if has_text {
                Some(text.trim().to_string())
            } else {
                None
            };
            let file_selected = self.selected_file.borrow().is_some();
            self.add_btn.set_sensitive(has_text || file_selected);
        }

        #[template_callback]
        fn on_file_browse_clicked(&self) {
            let dialog = gtk::FileDialog::builder()
                .title("Select .torrent File")
                .accept_label("Open")
                .modal(true)
                .build();

            let filter = gtk::FileFilter::new();
            filter.add_suffix("torrent");
            filter.set_name(Some("Torrent Files"));
            dialog.set_default_filter(Some(&filter));

            let imp = self.obj().imp().downgrade();
            dialog.open(
                None::<&gtk::Window>,
                gtk::gio::Cancellable::NONE,
                move |result| {
                    let imp = imp.upgrade().unwrap();
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            *imp.selected_file.borrow_mut() = Some(path.clone());
                            imp.drop_label.set_label(&path.to_string_lossy());
                            imp.drop_icon.set_icon_name(Some("emblem-ok-symbolic"));
                            imp.drop_icon.remove_css_class("dim-label");
                            imp.add_btn.set_sensitive(true);
                        }
                    }
                },
            );
        }

        #[template_callback]
        fn on_folder_browse_clicked(&self) {
            let dialog = gtk::FileDialog::builder()
                .title("Choose Download Folder")
                .accept_label("Select")
                .modal(true)
                .build();

            let imp = self.obj().imp().downgrade();
            dialog.select_folder(
                None::<&gtk::Window>,
                gtk::gio::Cancellable::NONE,
                move |result| {
                    let imp = imp.upgrade().unwrap();
                    if let Ok(folder) = result {
                        if let Some(path) = folder.path() {
                            imp.folder_label.set_label(&path.to_string_lossy());
                        }
                    }
                },
            );
        }

        fn start_torrent(&self) {
            let engine = self.engine.borrow();
            let tx = self.tx.borrow();
            let (engine, tx) = match (engine.as_ref(), tx.as_ref()) {
                (Some(e), Some(t)) => (e.clone(), t.clone()),
                _ => return,
            };

            let download_dir = {
                let label_text = self.folder_label.label().to_string();
                if label_text.trim().is_empty() {
                    dirs_next::download_dir().or_else(|| dirs_next::home_dir())
                } else {
                    Some(PathBuf::from(label_text.trim()))
                }
            };

            let _start_immediately = self.start_immediately_switch.is_active();
            self.obj().close();

            if let Some(url) = self.selected_url.borrow().as_ref() {
                let url = url.clone();
                let dir = download_dir.clone().unwrap_or_else(|| PathBuf::from("."));
                let engine_clone = engine.clone();
                let tx_clone = tx.clone();
                glib::spawn_future_local(async move {
                    engine_clone.borrow_mut().start(url, dir, tx_clone);
                });
            } else if let Some(file_path) = self.selected_file.borrow().as_ref() {
                let dir = download_dir.unwrap_or_else(|| {
                    file_path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
                });
                let engine_clone = engine.clone();
                let tx_clone = tx.clone();
                let path_str = file_path.to_string_lossy().to_string();
                glib::spawn_future_local(async move {
                    engine_clone.borrow_mut().start(path_str, dir, tx_clone);
                });
            }
        }
    }

    impl ObjectImpl for RillAddDialog {}
    impl WidgetImpl for RillAddDialog {}
    impl WindowImpl for RillAddDialog {}
}

glib::wrapper! {
    pub struct RillAddDialog(ObjectSubclass<imp::RillAddDialog>)
        @extends gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl RillAddDialog {
    pub fn new(
        engine: Rc<RefCell<TorrentEngine>>,
        tx: Sender<UiEvent>,
        parent: &impl IsA<gtk::Window>,
    ) -> Self {
        let dialog: Self = glib::Object::builder()
            .property("transient-for", parent)
            .build();

        let imp = dialog.imp();
        imp.engine.replace(Some(engine));
        imp.tx.replace(Some(tx));

        // Set default download folder
        if let Some(downloads) = dirs_next::download_dir() {
            imp.folder_label.set_label(&downloads.to_string_lossy());
        } else if let Some(home) = dirs_next::home_dir() {
            imp.folder_label.set_label(&home.to_string_lossy());
        }

        dialog
    }
}
