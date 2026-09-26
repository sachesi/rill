use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use mtorrent::utils::re_exports::mtorrent_utils::worker;

use crate::config;
use crate::dialogs::PreferencesDialog;
use crate::engine::TorrentEngine;
use crate::storage::{SavedTorrent, Storage};
use crate::tray::{self, TrayCommand};
use crate::window::RillWindow;

/// What the application runs on once it is the primary instance.
pub struct Session {
    /// Owns the DHT thread; dropping it stops the node.
    pub _dht_worker: worker::rt::Handle,
    /// Runs the torrents' disk storage; dropping it stops that.
    pub _storage_runtime: tokio::runtime::Runtime,
    pub engine: Rc<TorrentEngine>,
    pub storage: Storage,
    /// Torrents from the database, handed to the first window.
    pub saved: RefCell<Vec<SavedTorrent>>,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct RillApplication {
        pub session: OnceCell<Session>,
        /// Keeps the application running while the window is hidden to the tray.
        pub hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillApplication {
        const NAME: &'static str = "RillApplication";
        type Type = super::RillApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for RillApplication {}

    impl ApplicationImpl for RillApplication {
        fn startup(&self) {
            self.parent_startup();
            let app = self.obj();
            app.setup_actions();
            app.setup_tray();
            self.hold.replace(Some(app.hold()));
        }

        fn activate(&self) {
            self.parent_activate();
            if let Some(window) = self.obj().window() {
                window.present();
            }
        }

        fn open(&self, files: &[gio::File], hint: &str) {
            self.parent_open(files, hint);
            let Some(window) = self.obj().window() else {
                return;
            };
            window.present();
            for file in files {
                let uri = file.uri();
                log::info!("Opening {uri}");
                match file.path() {
                    Some(path) if !uri.starts_with("magnet:") => window.add_torrent_file(&path),
                    _ => window.add_magnet_link(&uri),
                }
            }
        }

        fn shutdown(&self) {
            if let Some(session) = self.session.get() {
                log::info!("Shutting down; stopping all torrents");
                // The database keeps every torrent as it was, so that the ones that were
                // downloading, or waiting to, start again on the next run.
                session.engine.pause_all();
                // Let every queued write reach the disk before the process exits.
                session.storage.flush_blocking();
            }
            self.parent_shutdown();
        }
    }

    impl GtkApplicationImpl for RillApplication {}
    impl AdwApplicationImpl for RillApplication {}
}

glib::wrapper! {
    pub struct RillApplication(ObjectSubclass<imp::RillApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl RillApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("resource-base-path", config::RESOURCE_PATH)
            .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
            .build()
    }

    pub fn set_session(&self, session: Session) {
        if self.imp().session.set(session).is_err() {
            log::error!("The session was set twice");
        }
    }

    /// The window, created on first use. `None` only before the session is set.
    fn window(&self) -> Option<RillWindow> {
        if let Some(window) = self.windows().into_iter().find_map(|w| w.downcast().ok()) {
            return Some(window);
        }
        let session = self.imp().session.get()?;
        let saved = session.saved.take();
        Some(RillWindow::new(
            self,
            session.engine.clone(),
            session.storage.clone(),
            saved,
        ))
    }

    fn setup_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| app.quit())
            .build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about())
            .build();
        let preferences = gio::ActionEntry::builder("preferences")
            .activate(|app: &Self, _, _| app.show_preferences())
            .build();
        self.add_action_entries([quit, about, preferences]);

        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("app.preferences", &["<Control>comma"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
        self.set_accels_for_action("win.add-file", &["<Control>o"]);
        self.set_accels_for_action("win.add-magnet", &["<Control>n"]);
        self.set_accels_for_action("win.search", &["<Control>f"]);
        self.set_accels_for_action("win.select-all", &["<Control>a"]);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Rill")
            .application_icon(config::APP_ID)
            .developer_name("sachesi")
            .version(config::VERSION)
            .website("https://github.com/sachesi/rill")
            .issue_url("https://github.com/sachesi/rill/issues")
            .license_type(gtk::License::Gpl30)
            .comments(gettext("Download files over BitTorrent"))
            // Translators: put your name here, one per line, optionally with an email address.
            .translator_credits(gettext("translator-credits"))
            .build();
        about.add_legal_section(
            "mtorrent",
            Some("© Mikhail Vasilyev"),
            gtk::License::Apache20,
            None,
        );
        about.present(self.active_window().as_ref());
    }

    fn show_preferences(&self) {
        let (Some(window), Some(session)) = (self.window(), self.imp().session.get()) else {
            return;
        };
        let storage = session.storage.clone();
        let settings = storage.query(|s| s.load_settings());
        glib::spawn_future_local(async move {
            let settings = settings.await.unwrap_or_default();
            PreferencesDialog::new(&window, storage, &settings).present(Some(&window));
        });
    }

    /// The tray runs on a thread of its own; its commands are handled here, on the main
    /// context.
    fn setup_tray(&self) {
        let commands = tray::spawn();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = app)]
            self,
            async move {
                while let Ok(command) = commands.recv().await {
                    match command {
                        TrayCommand::ShowWindow => {
                            if let Some(window) = app.window() {
                                window.present();
                            }
                        }
                        TrayCommand::Quit => app.quit(),
                    }
                }
            }
        ));
    }
}
