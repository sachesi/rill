use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use gtk::subclass::prelude::*;

use async_channel::Sender;
use crate::engine::{TorrentEngine, UiEvent};
use crate::storage::Storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddMode {
    File,
    Magnet,
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct RillAddDialog {
        pub cancel_btn: RefCell<Option<gtk::Button>>,
        pub add_btn: RefCell<Option<gtk::Button>>,
        pub url_entry: RefCell<Option<adw::EntryRow>>,
        pub folder_label: RefCell<Option<gtk::Label>>,
        pub file_row: RefCell<Option<adw::ActionRow>>,
        pub download_folder_row: RefCell<Option<adw::ActionRow>>,
        pub start_immediately_switch: RefCell<Option<adw::SwitchRow>>,
        pub url_group: RefCell<Option<adw::PreferencesGroup>>,
        pub file_group: RefCell<Option<adw::PreferencesGroup>>,

        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub tx: RefCell<Option<Sender<UiEvent>>>,
        pub storage: RefCell<Option<Storage>>,
        pub selected_url: RefCell<Option<String>>,
        pub selected_file: RefCell<Option<PathBuf>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillAddDialog {
        const NAME: &'static str = "RillAddDialog";
        type Type = super::RillAddDialog;
        type ParentType = adw::Window;
    }

    impl ObjectImpl for RillAddDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Cancel button
            let cancel_btn = gtk::Button::builder()
                .label("Cancel")
                .build();

            // Add button
            let add_btn = gtk::Button::builder()
                .label("Add")
                .css_classes(["suggested-action"])
                .sensitive(false)
                .build();

            // Header bar
            let header_bar = adw::HeaderBar::builder()
                .show_start_title_buttons(false)
                .show_end_title_buttons(false)
                .build();
            header_bar.pack_start(&cancel_btn);
            header_bar.pack_end(&add_btn);

            // URL entry
            let url_entry = adw::EntryRow::builder()
                .title("Magnet Link or HTTP URL")
                .build();

            let url_group = adw::PreferencesGroup::builder().build();
            url_group.add(&url_entry);

            // File picker row
            let file_arrow = gtk::Image::builder()
                .icon_name("go-next-symbolic")
                .build();
            let file_row = adw::ActionRow::builder()
                .title("Browse for .torrent file")
                .activatable(true)
                .build();
            file_row.add_suffix(&file_arrow);

            let file_group = adw::PreferencesGroup::builder().build();
            file_group.add(&file_row);

            // Options
            let folder_arrow = gtk::Image::builder()
                .icon_name("go-next-symbolic")
                .build();
            let download_folder_row = adw::ActionRow::builder()
                .title("Download Folder")
                .activatable(true)
                .build();
            download_folder_row.add_suffix(&folder_arrow);

            let start_immediately_switch = adw::SwitchRow::builder()
                .title("Start download immediately")
                .active(true)
                .build();

            let options_group = adw::PreferencesGroup::builder()
                .title("Options")
                .build();
            options_group.add(&download_folder_row);
            options_group.add(&start_immediately_switch);

            // Hidden folder label
            let folder_label = gtk::Label::builder()
                .visible(false)
                .build();

            // Main content box
            let content_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
            content_box.append(&url_group);
            content_box.append(&file_group);
            content_box.append(&options_group);
            content_box.append(&folder_label);

            let clamp = adw::Clamp::builder()
                .maximum_size(400)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(16)
                .margin_end(16)
                .build();
            clamp.set_child(Some(&content_box));

            let toolbar_view = adw::ToolbarView::builder().build();
            toolbar_view.add_top_bar(&header_bar);
            toolbar_view.set_content(Some(&clamp));

            obj.set_content(Some(&toolbar_view));

            self.cancel_btn.replace(Some(cancel_btn));
            self.add_btn.replace(Some(add_btn));
            self.url_entry.replace(Some(url_entry));
            self.folder_label.replace(Some(folder_label));
            self.file_row.replace(Some(file_row));
            self.download_folder_row.replace(Some(download_folder_row));
            self.start_immediately_switch.replace(Some(start_immediately_switch));
            self.url_group.replace(Some(url_group));
            self.file_group.replace(Some(file_group));
        }
    }
    impl WidgetImpl for RillAddDialog {}
    impl WindowImpl for RillAddDialog {}
    impl adw::subclass::prelude::AdwWindowImpl for RillAddDialog {}
}

glib::wrapper! {
    pub struct RillAddDialog(ObjectSubclass<imp::RillAddDialog>)
        @extends adw::Window, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl RillAddDialog {
    fn cancel_btn(&self) -> gtk::Button {
        self.imp().cancel_btn.borrow().clone().unwrap()
    }

    fn add_btn(&self) -> gtk::Button {
        self.imp().add_btn.borrow().clone().unwrap()
    }

    fn url_entry(&self) -> adw::EntryRow {
        self.imp().url_entry.borrow().clone().unwrap()
    }

    fn folder_label(&self) -> gtk::Label {
        self.imp().folder_label.borrow().clone().unwrap()
    }

    fn file_row(&self) -> adw::ActionRow {
        self.imp().file_row.borrow().clone().unwrap()
    }

    fn download_folder_row(&self) -> adw::ActionRow {
        self.imp().download_folder_row.borrow().clone().unwrap()
    }

    fn start_immediately_switch(&self) -> adw::SwitchRow {
        self.imp().start_immediately_switch.borrow().clone().unwrap()
    }

    pub fn new(
        engine: Rc<RefCell<TorrentEngine>>,
        tx: Sender<UiEvent>,
        storage: Storage,
        parent: &impl IsA<gtk::Window>,
        mode: AddMode,
    ) -> Self {
        let dialog: Self = glib::Object::builder()
            .property("title", "Add Torrent")
            .property("default-width", 480)
            .property("default-height", 260)
            .property("modal", true)
            .property("transient-for", parent)
            .build();

        let imp = dialog.imp();
        imp.engine.replace(Some(engine));
        imp.tx.replace(Some(tx));
        imp.storage.replace(Some(storage.clone()));

        let settings = storage.load_settings();
        dialog.folder_label().set_label(&settings.download_folder);

        dialog.setup_signals();
        dialog.set_mode(mode);

        dialog
    }

    pub fn set_mode(&self, mode: AddMode) {
        match mode {
            AddMode::Magnet => {
                self.set_title(Some("Add Torrent Link"));
                if let Some(url_group) = self.imp().url_group.borrow().as_ref() {
                    url_group.set_visible(true);
                }
                if let Some(file_group) = self.imp().file_group.borrow().as_ref() {
                    file_group.set_visible(false);
                }
                self.url_entry().grab_focus();
            }
            AddMode::File => {
                self.set_title(Some("Add Torrent File"));
                if let Some(url_group) = self.imp().url_group.borrow().as_ref() {
                    url_group.set_visible(false);
                }
                if let Some(file_group) = self.imp().file_group.borrow().as_ref() {
                    file_group.set_visible(true);
                }
            }
        }
    }

    pub fn set_uri(&self, uri: &str) {
        self.url_entry().set_text(uri);
        self.add_btn().set_sensitive(true);
    }

    pub fn set_selected_file(&self, path: &std::path::Path) {
        self.imp().selected_file.replace(Some(path.to_path_buf()));
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("torrent");
        self.file_row().set_subtitle(filename);
        self.add_btn().set_sensitive(true);
    }

    fn setup_signals(&self) {
        // URL entry - enable add button on text input
        let add_btn_downgrade = self.add_btn().downgrade();
        self.url_entry().connect_changed(move |entry| {
            let has_text = !entry.text().is_empty();
            if let Some(btn) = add_btn_downgrade.upgrade() {
                btn.set_sensitive(has_text);
            }
        });

        // File row click
        let dialog_weak = self.downgrade();
        self.file_row().connect_activated(move |_| {
            let dialog = match dialog_weak.upgrade() {
                Some(d) => d,
                None => return,
            };
            let file_chooser = gtk::FileDialog::builder()
                .title("Select Torrent File")
                .modal(true)
                .build();

            let dialog2 = dialog.downgrade();
            file_chooser.open(Some(&dialog), gtk::gio::Cancellable::NONE, move |result| {
                let dialog = match dialog2.upgrade() {
                    Some(d) => d,
                    None => return,
                };
                if let Ok(file) = result
                    && let Some(path) = file.path()
                {
                    dialog.set_selected_file(&path);
                }
            });
        });

        // Download folder row
        let dialog_weak2 = self.downgrade();
        self.download_folder_row().connect_activated(move |_| {
            let dialog = match dialog_weak2.upgrade() {
                Some(d) => d,
                None => return,
            };
            let folder_chooser = gtk::FileDialog::builder()
                .title("Choose Download Folder")
                .modal(true)
                .build();

            let dialog3 = dialog.downgrade();
            folder_chooser.select_folder(
                Some(&dialog),
                gtk::gio::Cancellable::NONE,
                move |result| {
                    let dialog = match dialog3.upgrade() {
                        Some(d) => d,
                        None => return,
                    };
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        dialog.folder_label().set_label(&path.to_string_lossy());
                        if let Some(storage) = dialog.imp().storage.borrow().as_ref() {
                            let mut settings = storage.load_settings();
                            settings.download_folder = path.to_string_lossy().to_string();
                            let _ = storage.save_settings(&settings);
                        }
                    }
                },
            );
        });

        // Cancel button
        let dialog_weak3 = self.downgrade();
        self.cancel_btn().connect_clicked(move |_| {
            if let Some(dialog) = dialog_weak3.upgrade() {
                dialog.close();
            }
        });

        // Add button
        let dialog_weak4 = self.downgrade();
        self.add_btn().connect_clicked(move |_| {
            if let Some(dialog) = dialog_weak4.upgrade() {
                dialog.start_torrent();
            }
        });
    }

    fn start_torrent(&self) {
        let engine = self.imp().engine.borrow();
        let tx = self.imp().tx.borrow();
        let (engine, tx) = match (engine.as_ref(), tx.as_ref()) {
            (Some(e), Some(t)) => (e.clone(), t.clone()),
            _ => return,
        };

        let download_dir = {
            let label_text = self.folder_label().label().to_string();
            if label_text.trim().is_empty() {
                dirs_next::download_dir().or_else(dirs_next::home_dir)
            } else {
                Some(PathBuf::from(label_text.trim()))
            }
        };

        let start_immediately = self.start_immediately_switch().is_active();
        self.close();

        if let Some(file_path) = self.imp().selected_file.borrow().as_ref() {
            let name = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("torrent")
                .to_string();
            let dir = download_dir.unwrap_or_else(|| {
                file_path
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
            let engine_clone = engine.clone();
            let tx_clone = tx.clone();
            let path_str = file_path.to_string_lossy().to_string();
            let window_weak = self.transient_for()
                .and_then(|t| t.downcast::<crate::application::RillWindow>().ok());
            glib::spawn_future_local(async move {
                let info_hash = if start_immediately {
                    engine_clone.borrow_mut().start(name, path_str, dir, tx_clone)
                } else {
                    engine_clone.borrow_mut().add_paused(name, path_str, dir, tx_clone)
                };
                if let Some(w) = window_weak {
                    w.allow_torrent(&info_hash);
                }
            });
        } else {
            let url = self.url_entry().text().to_string();
            if !url.trim().is_empty() {
                let name = extract_name_from_uri(&url);
                let dir = download_dir.clone().unwrap_or_else(|| PathBuf::from("."));
                let engine_clone = engine.clone();
                let tx_clone = tx.clone();
                let window_weak = self.transient_for()
                    .and_then(|t| t.downcast::<crate::application::RillWindow>().ok());
                glib::spawn_future_local(async move {
                    let info_hash = if start_immediately {
                        engine_clone.borrow_mut().start(name, url, dir, tx_clone)
                    } else {
                        engine_clone.borrow_mut().add_paused(name, url, dir, tx_clone)
                    };
                    if let Some(w) = window_weak {
                        w.allow_torrent(&info_hash);
                    }
                });
            }
        }
    }
}

fn extract_name_from_uri(uri: &str) -> String {
    if uri.starts_with("magnet:") {
        if let Some(start) = uri.find("&dn=") {
            let after_dn = &uri[start + 4..];
            if let Some(end) = after_dn.find('&') {
                urlencoding::decode(&after_dn[..end])
                    .unwrap_or_else(|_| "Unknown".into())
                    .to_string()
            } else {
                urlencoding::decode(after_dn)
                    .unwrap_or_else(|_| "Unknown".into())
                    .to_string()
            }
        } else {
            if let Some(start) = uri.find("btih:") {
                let hash = &uri[start + 5..];
                let hash = if hash.len() > 12 { &hash[..12] } else { hash };
                format!("Magnet {}", hash)
            } else {
                "Magnet Link".to_string()
            }
        }
    } else {
        let name = uri.rsplit('/').next().unwrap_or(uri);
        name.to_string()
    }
}
