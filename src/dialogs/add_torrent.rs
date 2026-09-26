//! Adds a torrent from a magnet link or a .torrent file, with the folder to save it to
//! and whether to start at once.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use mtorrent::utils::re_exports::mtorrent_base::input::{MagnetLink, Metainfo};

use crate::util::format_size;
use crate::window::RillWindow;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/rill/ui/add_torrent_dialog.ui")]
    pub struct AddTorrentDialog {
        #[template_child]
        pub add_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub magnet_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub magnet_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub magnet_error: TemplateChild<gtk::Label>,
        #[template_child]
        pub file_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub file_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub start_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub sequential_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub space_warning: TemplateChild<gtk::Label>,

        pub window: glib::WeakRef<RillWindow>,
        pub file: RefCell<Option<PathBuf>>,
        pub folder: RefCell<PathBuf>,
        /// What the chosen .torrent file says it needs, once it is known.
        pub needed_bytes: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AddTorrentDialog {
        const NAME: &'static str = "RillAddTorrentDialog";
        type Type = super::AddTorrentDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AddTorrentDialog {}
    impl WidgetImpl for AddTorrentDialog {}
    impl AdwDialogImpl for AddTorrentDialog {}

    #[gtk::template_callbacks]
    impl AddTorrentDialog {
        #[template_callback]
        fn on_cancel_clicked(&self) {
            self.obj().close();
        }

        #[template_callback]
        fn on_add_clicked(&self) {
            self.obj().add();
        }

        #[template_callback]
        fn on_magnet_changed(&self) {
            self.magnet_error.set_visible(false);
            self.magnet_row.remove_css_class("error");
            self.add_button
                .set_sensitive(!self.magnet_row.text().trim().is_empty());
        }

        #[template_callback]
        fn on_choose_file(&self) {
            self.obj().choose_file();
        }

        #[template_callback]
        fn on_choose_folder(&self) {
            self.obj().choose_folder();
        }
    }
}

glib::wrapper! {
    pub struct AddTorrentDialog(ObjectSubclass<imp::AddTorrentDialog>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl AddTorrentDialog {
    /// A dialog that saves to `folder` unless another is chosen.
    pub fn new(window: &RillWindow, folder: PathBuf) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.imp().window.set(Some(window));
        dialog.set_folder(folder);
        dialog
    }

    /// Asks for a magnet link, `uri` filled in when not empty.
    pub fn set_magnet(&self, uri: &str) {
        let imp = self.imp();
        self.set_title(&gettext("Add Magnet Link"));
        imp.file_group.set_visible(false);
        imp.magnet_row.set_text(uri);
    }

    pub fn set_file(&self, path: &Path) {
        let imp = self.imp();
        self.set_title(&gettext("Add Torrent File"));
        imp.magnet_group.set_visible(false);
        imp.needed_bytes.set(0);
        self.check_space();
        imp.file_row
            .set_subtitle(&path.file_name().unwrap_or_default().to_string_lossy());
        imp.file.replace(Some(path.to_path_buf()));
        imp.add_button.set_sensitive(true);
        self.set_focus(Some(&*imp.add_button));

        let path = path.to_path_buf();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            async move {
                let file = path.clone();
                let needed = gio::spawn_blocking(move || {
                    Metainfo::from_file(file).map_or(0, |metainfo| content_size(&metainfo))
                })
                .await
                .unwrap_or(0);
                let imp = dialog.imp();
                // Another file may have been chosen in the meantime.
                if imp.file.borrow().as_ref() == Some(&path) {
                    imp.needed_bytes.set(needed);
                    dialog.check_space();
                }
            }
        ));
    }

    fn set_folder(&self, folder: PathBuf) {
        self.imp()
            .folder_row
            .set_subtitle(&folder.to_string_lossy());
        self.imp().folder.replace(folder);
        self.check_space();
    }

    /// Says so when the chosen folder has less room than the torrent needs. Only a
    /// warning: the folder may well have grown by the time the download gets there, and
    /// a magnet link does not say how big it is until its metadata arrives.
    fn check_space(&self) {
        let imp = self.imp();
        let needed = imp.needed_bytes.get();
        if needed == 0 {
            imp.space_warning.set_visible(false);
            return;
        }
        let folder = imp.folder.borrow().clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = dialog)]
            self,
            async move {
                let dir = folder.clone();
                // A folder on a sleeping disk or a network share can take a while to say.
                let free = gio::spawn_blocking(move || free_space(&dir))
                    .await
                    .ok()
                    .flatten();
                let imp = dialog.imp();
                if imp.needed_bytes.get() != needed || *imp.folder.borrow() != folder {
                    return;
                }
                let short = free.is_some_and(|free| needed > free);
                imp.space_warning.set_visible(short);
                if short {
                    // Translators: %1 is what a torrent needs, %2 what the folder has left,
                    // both sizes like "4.0 GiB".
                    imp.space_warning.set_text(
                        &gettext("Not enough space in this folder: %1 needed, %2 free")
                            .replace("%1", &format_size(needed))
                            .replace("%2", &format_size(free.unwrap_or(0))),
                    );
                }
            }
        ));
    }

    fn choose_file(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&gettext("Torrent Files")));
        filter.add_mime_type("application/x-bittorrent");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder()
            .title(gettext("Choose a Torrent File"))
            .filters(&filters)
            .modal(true)
            .build();
        chooser.open(
            self.root().and_downcast_ref::<gtk::Window>(),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = dialog)]
                self,
                move |result| {
                    if let Ok(Some(path)) = result.map(|f| f.path()) {
                        dialog.set_file(&path);
                    }
                }
            ),
        );
    }

    /// Picks the folder for this torrent only; the default is changed in Preferences.
    fn choose_folder(&self) {
        let chooser = gtk::FileDialog::builder()
            .title(gettext("Choose a Download Folder"))
            .initial_folder(&gio::File::for_path(&*self.imp().folder.borrow()))
            .modal(true)
            .build();
        chooser.select_folder(
            self.root().and_downcast_ref::<gtk::Window>(),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = dialog)]
                self,
                move |result| {
                    if let Ok(Some(path)) = result.map(|f| f.path()) {
                        dialog.set_folder(path);
                    }
                }
            ),
        );
    }

    fn add(&self) {
        let imp = self.imp();
        let Some(window) = imp.window.upgrade() else {
            return;
        };
        let folder = imp.folder.borrow().clone();
        let start_now = imp.start_row.is_active();
        let sequential = imp.sequential_row.is_active();

        let Some(file) = imp.file.borrow().clone() else {
            let uri = imp.magnet_row.text().trim().to_string();
            // Refuse what the engine cannot parse here, rather than let it become a
            // transfer that never starts.
            let sanitized = crate::engine::sanitize_magnet_dn(&uri);
            let magnet = MagnetLink::from_str(&sanitized)
                .ok()
                .filter(|_| uri.starts_with("magnet:"));
            let Some(magnet) = magnet else {
                imp.magnet_row.add_css_class("error");
                imp.magnet_error
                    .set_label(&gettext("This is not a magnet link"));
                imp.magnet_error.set_visible(true);
                return;
            };
            self.close();
            let hash = crate::engine::hex(magnet.info_hash());
            let name = magnet_name(&uri);
            let uri = crate::engine::name_nameless_magnet(&uri);
            window.start_torrent(hash, name, uri, folder, sequential, start_now);
            return;
        };

        self.close();
        let data_dir = window.data_dir();
        glib::spawn_future_local(async move {
            let path = file.clone();
            let resolved = gio::spawn_blocking(move || prepare_torrent_file(&path, &data_dir))
                .await
                .ok()
                .flatten();
            match resolved {
                Some((hash, name, path)) => window.start_torrent(
                    hash,
                    name,
                    path.to_string_lossy().into_owned(),
                    folder,
                    sequential,
                    start_now,
                ),
                None => window.show_toast(
                    // Translators: %s is a file name.
                    &gettext("“%s” is not a torrent file").replace(
                        "%s",
                        &file.file_name().unwrap_or_default().to_string_lossy(),
                    ),
                ),
            }
        });
    }
}

/// The name a magnet link gives its torrent, as mtorrent reads it; empty when it has
/// none, until the metadata arrives.
/// How much room a torrent's content needs: its single file, or all of them together.
/// `Metainfo::size` is the size of the metainfo itself, which is not this.
fn content_size(metainfo: &Metainfo) -> u64 {
    if let Some(length) = metainfo.length() {
        return length as u64;
    }
    metainfo
        .files()
        .map_or(0, |files| files.map(|(length, _path)| length as u64).sum())
}

/// The bytes free in the filesystem `folder` is on, or `None` when it does not say.
fn free_space(folder: &Path) -> Option<u64> {
    let info = gio::File::for_path(folder)
        .query_filesystem_info("filesystem::free", gio::Cancellable::NONE)
        .ok()?;
    // A filesystem that does not report free space would otherwise look full.
    info.has_attribute("filesystem::free")
        .then(|| info.attribute_uint64("filesystem::free"))
}

fn magnet_name(uri: &str) -> String {
    MagnetLink::from_str(uri)
        .ok()
        .and_then(|magnet| magnet.name().map(str::to_string))
        .unwrap_or_default()
}

/// Reads a .torrent file and returns its info hash, its name and the path to give the
/// engine, or `None` when it is not a torrent.
///
/// mtorrent names the download folder after the file's stem, so when the torrent's own
/// name is different, the file is copied to `data_dir/torrents/<name>.torrent` and that
/// copy is used instead.
fn prepare_torrent_file(file: &Path, data_dir: &Path) -> Option<(String, String, PathBuf)> {
    let meta = Metainfo::from_file(file).ok()?;
    let hash = crate::engine::hex(meta.info_hash());
    let fallback = || {
        file.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    let Some(real_name) = meta.name().filter(|n| !n.is_empty()) else {
        return Some((hash, fallback(), file.to_path_buf()));
    };

    // A single-file torrent is named after its file ("film.mkv"); the folder is not.
    let stem = if meta.files().is_none() {
        Path::new(real_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(real_name)
    } else {
        real_name
    };
    // Only what no filesystem takes in a name is replaced; the rest, any script, stays.
    let safe: String = stem
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_string();
    let safe = if safe.is_empty() {
        "torrent".to_string()
    } else {
        safe
    };

    if file.file_stem().and_then(|s| s.to_str()) == Some(safe.as_str()) {
        return Some((hash, real_name.to_string(), file.to_path_buf()));
    }
    let dir = data_dir.join("torrents");
    let copy = dir.join(format!("{safe}.torrent"));
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(file, &copy)) {
        Ok(_) => {
            log::info!("Copied {} to {}", file.display(), copy.display());
            Some((hash, real_name.to_string(), copy))
        }
        Err(e) => {
            log::warn!(
                "Failed to copy {} to {}: {e}",
                file.display(),
                copy.display()
            );
            Some((hash, real_name.to_string(), file.to_path_buf()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_is_read_for_a_real_folder_and_not_for_a_missing_one() {
        let free = free_space(&std::env::temp_dir());
        assert!(free.is_some_and(|free| free > 0), "got {free:?}");
        assert_eq!(free_space(Path::new("/no/such/folder/here")), None);
    }

    #[test]
    fn a_torrent_file_says_how_much_room_its_content_needs() {
        let dir = std::env::temp_dir().join(format!("rill-space-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let single = dir.join("film.torrent");
        std::fs::write(
            &single,
            b"d4:infod6:lengthi1024e4:name8:film.mkv12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee",
        )
        .unwrap();

        let multi = dir.join("season.torrent");
        std::fs::write(
            &multi,
            b"d4:infod5:filesld6:lengthi600e4:pathl7:one.mkveed6:lengthi424e4:pathl7:two.mkveee4:name6:season12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee",
        )
        .unwrap();

        let sizes = [single, multi].map(|path| {
            Metainfo::from_file(&path)
                .map(|metainfo| content_size(&metainfo))
                .ok()
        });
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(sizes, [Some(1024), Some(1024)]);
    }

    #[test]
    fn magnet_name_is_the_display_name() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            magnet_name(&format!("magnet:?xt=urn:btih:{hash}&dn=Some%20Film")),
            "Some Film"
        );
        assert_eq!(magnet_name(&format!("magnet:?xt=urn:btih:{hash}")), "");
    }
}
