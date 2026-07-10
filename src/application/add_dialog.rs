use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::engine::{TorrentEngine, UiEvent};
use crate::storage::Storage;
use async_channel::Sender;

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
        pub url_error_label: RefCell<Option<gtk::Label>>,
        pub folder_label: RefCell<Option<gtk::Label>>,
        pub file_row: RefCell<Option<adw::ActionRow>>,
        pub download_folder_row: RefCell<Option<adw::ActionRow>>,
        pub start_immediately_switch: RefCell<Option<adw::SwitchRow>>,
        pub sequential_switch: RefCell<Option<adw::SwitchRow>>,
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
            let cancel_btn = gtk::Button::builder().label(gettext("Cancel")).build();

            // Add button
            let add_btn = gtk::Button::builder()
                .label(gettext("Add"))
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
                .title(gettext("Magnet Link"))
                .build();

            // Inline validation error, shown when the Add button is pressed with
            // text that is not a parseable magnet link.
            let url_error_label = gtk::Label::builder()
                .css_classes(["error", "caption"])
                .halign(gtk::Align::Start)
                .margin_start(12)
                .visible(false)
                .build();

            let url_group = adw::PreferencesGroup::builder().build();
            url_group.add(&url_entry);
            url_group.add(&url_error_label);

            // File picker row
            let file_arrow = gtk::Image::builder().icon_name("go-next-symbolic").build();
            let file_row = adw::ActionRow::builder()
                .title(gettext("Browse for .torrent file"))
                .activatable(true)
                .build();
            file_row.add_suffix(&file_arrow);

            let file_group = adw::PreferencesGroup::builder().build();
            file_group.add(&file_row);

            // Options
            let folder_arrow = gtk::Image::builder().icon_name("go-next-symbolic").build();
            let download_folder_row = adw::ActionRow::builder()
                .title(gettext("Download Folder"))
                .activatable(true)
                .build();
            download_folder_row.add_suffix(&folder_arrow);

            let start_immediately_switch = adw::SwitchRow::builder()
                .title(gettext("Start download immediately"))
                .active(true)
                .build();

            let sequential_switch = adw::SwitchRow::builder()
                .title(gettext("Download from start to finish (sequential)"))
                .active(false)
                .build();

            let options_group = adw::PreferencesGroup::builder()
                .title(gettext("Options"))
                .build();
            options_group.add(&download_folder_row);
            options_group.add(&start_immediately_switch);
            options_group.add(&sequential_switch);

            // Hidden folder label
            let folder_label = gtk::Label::builder().visible(false).build();

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
            self.url_error_label.replace(Some(url_error_label));
            self.folder_label.replace(Some(folder_label));
            self.file_row.replace(Some(file_row));
            self.download_folder_row.replace(Some(download_folder_row));
            self.start_immediately_switch
                .replace(Some(start_immediately_switch));
            self.sequential_switch.replace(Some(sequential_switch));
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

    fn url_error_label(&self) -> gtk::Label {
        self.imp().url_error_label.borrow().clone().unwrap()
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
        self.imp()
            .start_immediately_switch
            .borrow()
            .clone()
            .unwrap()
    }

    fn sequential_switch(&self) -> adw::SwitchRow {
        self.imp().sequential_switch.borrow().clone().unwrap()
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

        // Load the default download folder off the GTK thread.
        let dialog_weak = dialog.downgrade();
        glib::spawn_future_local(async move {
            if let Ok(settings) = storage.query(|s| s.load_settings()).await
                && let Some(dialog) = dialog_weak.upgrade()
            {
                dialog.folder_label().set_label(&settings.download_folder);
            }
        });

        dialog.setup_signals();
        dialog.set_mode(mode);

        dialog
    }

    pub fn set_mode(&self, mode: AddMode) {
        match mode {
            AddMode::Magnet => {
                self.set_title(Some(gettext("Add Magnet Link").as_str()));
                if let Some(url_group) = self.imp().url_group.borrow().as_ref() {
                    url_group.set_visible(true);
                }
                if let Some(file_group) = self.imp().file_group.borrow().as_ref() {
                    file_group.set_visible(false);
                }
                self.url_entry().grab_focus();
            }
            AddMode::File => {
                self.set_title(Some(gettext("Add Torrent File").as_str()));
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
        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("torrent");
        self.file_row().set_subtitle(filename);
        self.add_btn().set_sensitive(true);
    }

    fn setup_signals(&self) {
        // Escape dismisses the dialog like Cancel; the header close buttons are
        // hidden, so without this the small Cancel button is the only exit.
        let key_controller = gtk::EventControllerKey::new();
        let dialog_weak_esc = self.downgrade();
        key_controller.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                if let Some(dialog) = dialog_weak_esc.upgrade() {
                    dialog.close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.add_controller(key_controller);

        // URL entry - enable add button on text input, clear stale validation error
        let add_btn_downgrade = self.add_btn().downgrade();
        let dialog_weak_err = self.downgrade();
        self.url_entry().connect_changed(move |entry| {
            let has_text = !entry.text().is_empty();
            if let Some(btn) = add_btn_downgrade.upgrade() {
                btn.set_sensitive(has_text);
            }
            if let Some(dialog) = dialog_weak_err.upgrade() {
                dialog.url_error_label().set_visible(false);
                entry.remove_css_class("error");
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
                .title(gettext("Select Torrent File"))
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
                .title(gettext("Choose Download Folder"))
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
                        // Per-add destination only. The global default download
                        // folder is changed in Preferences, never as a side effect
                        // of picking a folder for one torrent.
                        dialog.folder_label().set_label(&path.to_string_lossy());
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
        let sequential = self.sequential_switch().is_active();

        // Validate magnet input before dispatching anything: a string the engine
        // cannot parse must not close the dialog, let alone show up as a
        // "Downloading" row. File mode is validated asynchronously below.
        if self.imp().selected_file.borrow().is_none() {
            use mtorrent::utils::re_exports::mtorrent_core::input::MagnetLink;
            use std::str::FromStr;

            let url = self.url_entry().text().trim().to_string();
            let sanitized = crate::engine::sanitize_magnet_dn(&url);
            if !url.starts_with("magnet:") || MagnetLink::from_str(&sanitized).is_err() {
                self.url_entry().add_css_class("error");
                self.url_error_label()
                    .set_label(&gettext("Not a valid magnet link"));
                self.url_error_label().set_visible(true);
                return;
            }
        }
        self.close();

        if let Some(file_path) = self.imp().selected_file.borrow().as_ref() {
            let file_path = file_path.clone();
            let fallback_name = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("torrent")
                .to_string();

            let config_dir = engine.borrow().config_dir().clone();
            let dir = download_dir.unwrap_or_else(|| {
                file_path
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            });
            let engine_clone = engine.clone();
            let tx_clone = tx.clone();
            let window_weak = self
                .transient_for()
                .and_then(|t| t.downcast::<crate::application::RillWindow>().ok());
            glib::spawn_future_local(async move {
                // Resolve the real torrent name from metadata off the GTK thread
                // (bencode parse + a possible file copy). When the real name
                // differs from the filename stem, resolve_torrent_file_path copies
                // the .torrent so mtorrent's download subfolder matches it.
                let fp = file_path.clone();
                let cd = config_dir.clone();
                let fb_name = fallback_name.clone();
                let (valid, resolved) = tokio::task::spawn_blocking(move || {
                    use mtorrent::utils::re_exports::mtorrent_core::input::Metainfo;
                    let valid = Metainfo::from_file(&fp).is_ok();
                    (valid, resolve_torrent_file_path(&fp, &cd))
                })
                .await
                .unwrap_or((false, None));

                // A file that does not parse as bencoded metainfo would only
                // produce a doomed "Downloading" row; reject it up front.
                if !valid {
                    if let Some(w) = &window_weak {
                        w.show_toast(
                            &gettext("{name} is not a valid torrent file")
                                .replace("{name}", &fallback_name),
                        );
                    }
                    return;
                }

                let (name, path_str) =
                    resolved.unwrap_or_else(|| (fb_name, file_path.to_string_lossy().to_string()));

                let info_hash = if start_immediately {
                    engine_clone
                        .borrow_mut()
                        .start(name, path_str, dir, sequential, tx_clone)
                } else {
                    engine_clone
                        .borrow_mut()
                        .add_paused(name, path_str, dir, sequential, tx_clone)
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
                let window_weak = self
                    .transient_for()
                    .and_then(|t| t.downcast::<crate::application::RillWindow>().ok());
                glib::spawn_future_local(async move {
                    let info_hash = if start_immediately {
                        engine_clone
                            .borrow_mut()
                            .start(name, url, dir, sequential, tx_clone)
                    } else {
                        engine_clone
                            .borrow_mut()
                            .add_paused(name, url, dir, sequential, tx_clone)
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
                // Truncate by characters, not bytes: a byte slice can panic on a
                // UTF-8 boundary in pasted garbage.
                let hash: String = uri[start + 5..].chars().take(12).collect();
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

/// Parses a `.torrent` file, extracts the real name from `info.name`,
/// and — when that name differs from the filename stem — copies the
/// file to `config_dir/torrents/<real_name>.torrent`.
///
/// Returns `(real_name, path_to_use)` on success.  The caller should
/// pass the returned path to the engine so mtorrent creates a download
/// subfolder matching the real torrent name.
fn resolve_torrent_file_path(file_path: &Path, config_dir: &Path) -> Option<(String, String)> {
    use mtorrent::utils::re_exports::mtorrent_core::input::Metainfo;

    let meta = Metainfo::from_file(file_path).ok()?;
    let real_name = meta.name().filter(|n| !n.is_empty())?;

    // For single-file torrents the name is the file name (e.g. "foo.rar"), and
    // mtorrent names the download subfolder after the .torrent filename stem.
    // Strip the trailing extension so the folder isn't named "foo.rar".
    // Multi-file torrents have a directory name — leave it untouched.
    let folder_stem: &str = if meta.files().is_none() {
        std::path::Path::new(real_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(real_name)
    } else {
        real_name
    };

    let original_stem = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    // Sanitise the folder stem for use as a filename.
    // Only strip characters that are actually invalid/problematic on
    // modern filesystems — preserves Unicode (Cyrillic, CJK, etc.).
    let safe_name: String = folder_stem
        .chars()
        .map(|c| match c {
            '/' | '\0' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_string();
    let safe_name = if safe_name.is_empty() {
        "torrent".to_string()
    } else {
        safe_name
    };

    // If the filename stem already matches, no copy is needed.
    if safe_name == original_stem {
        return Some((
            real_name.to_string(),
            file_path.to_string_lossy().to_string(),
        ));
    }

    let torrents_dir = config_dir.join("torrents");
    let _ = std::fs::create_dir_all(&torrents_dir);
    let new_path = torrents_dir.join(format!("{}.torrent", safe_name));

    match std::fs::copy(file_path, &new_path) {
        Ok(_) => {
            log::info!(
                "Copied .torrent file to {:?} (real name: '{}')",
                new_path,
                real_name
            );
            Some((
                real_name.to_string(),
                new_path.to_string_lossy().to_string(),
            ))
        }
        Err(e) => {
            log::warn!("Failed to copy .torrent file for rename: {}", e);
            // Still return the real name even if copy failed.
            Some((
                real_name.to_string(),
                file_path.to_string_lossy().to_string(),
            ))
        }
    }
}
