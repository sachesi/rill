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
        #[template_child]
        pub file_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub download_folder_row: TemplateChild<adw::ActionRow>,

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
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for RillAddDialog {}
    impl WidgetImpl for RillAddDialog {}
    impl WindowImpl for RillAddDialog {}

    impl RillAddDialog {
        pub fn start_torrent(&self) {
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
                let name = extract_name_from_uri(&url);
                let dir = download_dir.clone().unwrap_or_else(|| PathBuf::from("."));
                let engine_clone = engine.clone();
                let tx_clone = tx.clone();
                glib::spawn_future_local(async move {
                    engine_clone.borrow_mut().start(name, url, dir, tx_clone);
                });
            } else if let Some(file_path) = self.selected_file.borrow().as_ref() {
                let name = file_path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("torrent")
                    .to_string();
                let dir = download_dir.unwrap_or_else(|| {
                    file_path.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
                });
                let engine_clone = engine.clone();
                let tx_clone = tx.clone();
                let path_str = file_path.to_string_lossy().to_string();
                glib::spawn_future_local(async move {
                    engine_clone.borrow_mut().start(name, path_str, dir, tx_clone);
                });
            }
        }
    }
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

        dialog.setup_signals();

        dialog
    }

    fn setup_signals(&self) {
        let imp = self.imp();
        let dialog = self.downgrade();

        // Cancel button
        let dlg_weak = dialog.clone();
        imp.cancel_btn.connect_clicked(move |_| {
            if let Some(d) = dlg_weak.upgrade() {
                d.close();
            }
        });

        // Add button
        let dlg_weak = dialog.clone();
        imp.add_btn.connect_clicked(move |_| {
            if let Some(d) = dlg_weak.upgrade() {
                d.imp().start_torrent();
            }
        });

        // URL entry apply (Enter key) and changed
        let dlg_weak = dialog.clone();
        imp.url_entry.connect_apply(move |_entry| {
            if let Some(d) = dlg_weak.upgrade() {
                d.imp().start_torrent();
            }
        });
        let dlg_weak = dialog.clone();
        imp.url_entry.connect_changed(move |entry| {
            if let Some(d) = dlg_weak.upgrade() {
                let text = entry.text().to_string();
                let has_text = !text.trim().is_empty();
                let imp = d.imp();
                *imp.selected_url.borrow_mut() = if has_text {
                    Some(text.trim().to_string())
                } else {
                    None
                };
                let file_selected = imp.selected_file.borrow().is_some();
                imp.add_btn.set_sensitive(has_text || file_selected);
            }
        });

        // File browse
        let dlg_weak = dialog.clone();
        imp.file_row.connect_activated(move |_| {
            let dlg = match dlg_weak.upgrade() {
                Some(d) => d,
                None => return,
            };
            let file_dialog = gtk::FileDialog::builder()
                .title("Select .torrent File")
                .accept_label("Open")
                .modal(true)
                .build();

            let filter = gtk::FileFilter::new();
            filter.add_suffix("torrent");
            filter.set_name(Some("Torrent Files"));
            file_dialog.set_default_filter(Some(&filter));

            let imp2 = dlg.imp().downgrade();
            file_dialog.open(
                None::<&gtk::Window>,
                gtk::gio::Cancellable::NONE,
                move |result| {
                    let imp2 = imp2.upgrade().unwrap();
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            *imp2.selected_file.borrow_mut() = Some(path.clone());
                            imp2.drop_label.set_label(&path.to_string_lossy());
                            imp2.drop_icon.set_icon_name(Some("emblem-ok-symbolic"));
                            imp2.drop_icon.remove_css_class("dim-label");
                            imp2.add_btn.set_sensitive(true);
                        }
                    }
                },
            );
        });

        // Folder browse
        let dlg_weak = dialog;
        imp.download_folder_row.connect_activated(move |_| {
            let dlg = match dlg_weak.upgrade() {
                Some(d) => d,
                None => return,
            };
            let folder_dialog = gtk::FileDialog::builder()
                .title("Choose Download Folder")
                .accept_label("Select")
                .modal(true)
                .build();

            let imp2 = dlg.imp().downgrade();
            folder_dialog.select_folder(
                None::<&gtk::Window>,
                gtk::gio::Cancellable::NONE,
                move |result| {
                    let imp2 = imp2.upgrade().unwrap();
                    if let Ok(folder) = result {
                        if let Some(path) = folder.path() {
                            imp2.folder_label.set_label(&path.to_string_lossy());
                        }
                    }
                },
            );
        });
    }
}

fn extract_name_from_uri(uri: &str) -> String {
    if uri.starts_with("magnet:") {
        for pair in uri.split('&') {
            if let Some(value) = pair.strip_prefix("dn=") {
                if let Ok(decoded) = urlencoding::decode(value) {
                    return decoded.into_owned();
                }
            }
        }
    }
    if let Some(pos) = uri.rfind('/') {
        let filename = &uri[pos + 1..];
        if !filename.is_empty() {
            if let Some(qm) = filename.find('?') {
                return filename[..qm].to_string();
            }
            return filename.to_string();
        }
    }
    uri.to_string()
}
