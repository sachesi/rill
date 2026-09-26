use std::cell::OnceCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::logging;
use crate::storage::{AppSettings, Storage};
use crate::window::RillWindow;

/// The port the spin row offers until the user picks another; the port BitTorrent is
/// known by, which a torrent counts up from.
const DEFAULT_PORT: u16 = 6881;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sachesi/rill/ui/preferences_dialog.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub max_downloads_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub auto_port_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub port_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub log_level_row: TemplateChild<adw::ComboRow>,

        /// Set while the rows are filled in from the settings, so that nothing is
        /// written back.
        pub filling: std::cell::Cell<bool>,
        pub window: glib::WeakRef<RillWindow>,
        pub storage: OnceCell<Storage>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "RillPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PreferencesDialog {}
    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}

    #[gtk::template_callbacks]
    impl PreferencesDialog {
        #[template_callback]
        fn on_choose_folder(&self) {
            self.obj().choose_folder();
        }

        #[template_callback]
        fn on_max_downloads_changed(&self) {
            let value = self.max_downloads_row.value() as i32;
            let obj = self.obj();
            obj.save(move |s| s.max_active_downloads = value);
            if let Some(window) = self.window.upgrade() {
                window.set_download_limit(value.max(1) as usize);
            }
        }

        #[template_callback]
        fn on_port_changed(&self) {
            if self.filling.get() {
                return;
            }
            let value = self.port_row.value() as u16;
            self.obj().save(move |s| s.pwp_port = value);
            if let Some(window) = self.window.upgrade() {
                window.set_listening_port(value);
            }
        }

        #[template_callback]
        fn on_auto_port_changed(&self) {
            if self.filling.get() {
                return;
            }
            let automatic = self.auto_port_row.is_active();
            // Zero is how a port of the torrent's own is stored; the row below says what
            // torrents count up from instead, and is worth showing only then.
            let port = if automatic {
                0
            } else {
                self.port_row.value() as u16
            };
            self.port_row.set_visible(!automatic);
            self.obj().save(move |s| s.pwp_port = port);
            if let Some(window) = self.window.upgrade() {
                window.set_listening_port(port);
            }
        }

        #[template_callback]
        fn on_log_level_changed(&self) {
            let level = logging::LEVELS
                .get(self.log_level_row.selected() as usize)
                .copied()
                .unwrap_or("info");
            self.obj().save(move |s| s.log_level = level.to_string());
        }
    }
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl PreferencesDialog {
    pub fn new(window: &RillWindow, storage: Storage, settings: &AppSettings) -> Self {
        let dialog: Self = glib::Object::new();
        let imp = dialog.imp();
        imp.window.set(Some(window));

        // Filled in before the storage is set, so that nothing is written back.
        imp.folder_row
            .set_subtitle(&settings.download_folder_path().to_string_lossy());
        imp.max_downloads_row
            .set_value(settings.max_active_downloads as f64);
        imp.filling.set(true);
        let automatic = settings.pwp_port == 0;
        imp.auto_port_row.set_active(automatic);
        imp.port_row.set_visible(!automatic);
        imp.port_row.set_value(if automatic {
            DEFAULT_PORT
        } else {
            settings.pwp_port
        } as f64);
        imp.filling.set(false);
        let levels = [
            gettext("Errors"),
            gettext("Warnings"),
            gettext("Information"),
            gettext("Debugging"),
            gettext("Everything"),
        ];
        let levels: Vec<&str> = levels.iter().map(String::as_str).collect();
        imp.log_level_row
            .set_model(Some(&gtk::StringList::new(&levels)));
        let level = logging::LEVELS
            .iter()
            .position(|l| *l == settings.log_level)
            .unwrap_or(2);
        imp.log_level_row.set_selected(level as u32);

        imp.storage.set(storage).ok();
        dialog
    }

    /// Changes one setting and stores it, saying so when that fails. The worker reads and
    /// writes the settings in one job, so that the ones it saves for the window are not
    /// lost in between.
    fn save(&self, change: impl FnOnce(&mut AppSettings) + Send + 'static) {
        let Some(storage) = self.imp().storage.get() else {
            return;
        };
        let saved = storage.query(move |s| {
            let mut settings = s.load_settings();
            change(&mut settings);
            s.save_settings(&settings).map(|()| settings)
        });
        let dialog = self.downgrade();
        glib::spawn_future_local(async move {
            match saved.await.and_then(|r| r) {
                Ok(settings) => logging::apply_settings(&settings),
                Err(e) => {
                    log::warn!("Failed to save settings: {e}");
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.add_toast(adw::Toast::new(&gettext("Could not save the setting")));
                    }
                }
            }
        });
    }

    fn choose_folder(&self) {
        let current = self.imp().folder_row.subtitle().unwrap_or_default();
        let chooser = gtk::FileDialog::builder()
            .title(gettext("Choose the Download Folder"))
            .initial_folder(&gio::File::for_path(current.as_str()))
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
                        dialog
                            .imp()
                            .folder_row
                            .set_subtitle(&path.to_string_lossy());
                        let folder = path.to_string_lossy().into_owned();
                        dialog.save(move |s| s.download_folder = folder);
                    }
                }
            ),
        );
    }
}
