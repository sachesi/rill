use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::engine::TorrentEngine;
use crate::model::TorrentModel;
use crate::storage::{Storage, SavedTorrent};
use super::RillWindow;
use super::show_preferences;
use super::tray::{self, TrayCommand};

pub struct RillApplication {
    app: adw::Application,
    #[allow(dead_code)]
    engine: Rc<RefCell<TorrentEngine>>,
    #[allow(dead_code)]
    model: Rc<RefCell<TorrentModel>>,
    storage: Storage,
    saved_torrents: Vec<SavedTorrent>,
    // Keeps the application alive while the window is hidden to the tray.
    _hold: gio::ApplicationHoldGuard,
    // Source ID of the 100ms tray-command drain loop, kept for explicit ownership.
    tray_source: RefCell<Option<glib::SourceId>>,
}

impl RillApplication {
    pub fn new(
        app: adw::Application,
        engine: Rc<RefCell<TorrentEngine>>,
        storage: Storage,
        saved_torrents: Vec<SavedTorrent>,
    ) -> Self {
        let model = Rc::new(RefCell::new(TorrentModel::new()));

        let app_self = Self {
            app: app.clone(),
            engine: engine.clone(),
            model: model.clone(),
            storage: storage.clone(),
            saved_torrents,
            _hold: app.hold(),
            tray_source: RefCell::new(None),
        };

        // Set up application-level actions
        app_self.setup_actions();

        // System tray: drives show/quit while the window is hidden.
        app_self.setup_tray();
        
        // Connect activate signal
        let storage_clone1 = storage.clone();
        let saved_torrents_clone1 = app_self.saved_torrents.clone();
        app.connect_activate(glib::clone!(
            #[weak] engine,
            #[weak] model,
            move |app| {
                let existing_window = app.windows().into_iter()
                    .find_map(|win| win.downcast::<RillWindow>().ok());

                if let Some(window) = existing_window {
                    window.present();
                } else {
                    let window = RillWindow::new(
                        engine.clone(),
                        model.clone(),
                        storage_clone1.clone(),
                        saved_torrents_clone1.clone(),
                        app,
                    );
                    window.present();
                }
            }
        ));

        // Connect open signal
        let storage_clone2 = storage.clone();
        let saved_torrents_clone2 = app_self.saved_torrents.clone();
        app.connect_open(glib::clone!(
            #[weak] engine,
            #[weak] model,
            move |app, files, _hint| {
                let existing_window = app.windows().into_iter()
                    .find_map(|win| win.downcast::<RillWindow>().ok());

                let window = if let Some(window) = existing_window {
                    window
                } else {
                    let win = RillWindow::new(
                        engine.clone(),
                        model.clone(),
                        storage_clone2.clone(),
                        saved_torrents_clone2.clone(),
                        app,
                    );
                    win.present();
                    win
                };

                window.present();

                for file in files {
                    let uri = file.uri().to_string();
                    log::info!("Opening URI from system: {}", uri);
                    if uri.starts_with("magnet:") {
                        window.show_add_dialog_with_uri(&uri);
                    } else if uri.starts_with("file://") {
                        if let Some(path) = file.path() {
                            window.show_add_dialog_with_file(&path);
                        }
                    } else {
                        if let Some(path) = file.path() {
                            window.show_add_dialog_with_file(&path);
                        } else {
                            window.show_add_dialog_with_uri(&uri);
                        }
                    }
                }
            }
        ));

        // Connect shutdown signal to pause all torrents on exit
        let storage_clone_shutdown = storage.clone();
        app.connect_shutdown(glib::clone!(
            #[weak] engine,
            move |_app| {
                log::info!("Application is shutting down. Pausing all torrents...");
                engine.borrow().pause_all();
                if let Err(e) = storage_clone_shutdown.pause_all_torrents() {
                    log::error!("Failed to pause torrents in database during shutdown: {}", e);
                }
                // Drain the storage worker so every queued write (state snapshots,
                // the pause_all update above) reaches disk before the process exits.
                // This replaces a fixed sleep with a precise barrier.
                storage_clone_shutdown.flush_blocking();
            }
        ));

        app_self
    }

    fn setup_actions(&self) {
        // Quit action
        let quit_action = gio::SimpleAction::new("quit", None);
        quit_action.connect_activate(glib::clone!(
            #[weak(rename_to = app)] self.app,
            move |_, _| app.quit()
        ));
        self.app.add_action(&quit_action);
        self.app.set_accels_for_action("app.quit", &["<Control>q"]);

        // About action  
        let about_action = gio::SimpleAction::new("about", None);
        about_action.connect_activate(glib::clone!(
            #[weak(rename_to = app)] self.app,
            move |_, _| {
                let about = adw::AboutWindow::builder()
                    .application_name("Rill")
                    .version("0.1.8")
                    .developer_name("sachesi")
                    .license_type(gtk::License::MitX11)
                    .comments(gettext("Minimalistic GTK4/Libadwaita BitTorrent client"))
                    .application_icon("com.github.sachesi.rill")
                    .build();
                if let Some(window) = app.active_window() {
                    about.set_transient_for(Some(&window));
                }
                about.present();
            }
        ));
        self.app.add_action(&about_action);

        // Preferences action  
        let prefs_action = gio::SimpleAction::new("preferences", None);
        let app_weak = self.app.downgrade();
        let storage_clone = self.storage.clone();
        prefs_action.connect_activate(move |_, _| {
            if let Some(app) = app_weak.upgrade()
                && let Some(window) = app.active_window()
                && let Ok(rill_window) = window.downcast::<RillWindow>()
            {
                show_preferences(
                    &rill_window,
                    storage_clone.clone(),
                );
            }
        });
        self.app.add_action(&prefs_action);
        self.app.set_accels_for_action("app.preferences", &["<Control>comma"]);

        // Add torrent action
        let add_action = gio::SimpleAction::new("add-torrent", None);
        let app_weak = self.app.downgrade();
        add_action.connect_activate(move |_, _| {
            if let Some(app) = app_weak.upgrade()
                && let Some(window) = app.active_window()
                && let Ok(rill_window) = window.downcast::<RillWindow>()
            {
                rill_window.show_add_dialog();
            }
        });
        self.app.add_action(&add_action);
        self.app.set_accels_for_action("app.add-torrent", &["<Control>n"]);
    }

    fn setup_tray(&self) {
        let rx = tray::spawn();
        let app_weak = self.app.downgrade();
        let source = glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let Some(app) = app_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    TrayCommand::ShowWindow => {
                        if let Some(window) = app.active_window() {
                            window.set_visible(true);
                            window.present();
                        }
                    }
                    TrayCommand::QuitApp => app.quit(),
                }
            }
            glib::ControlFlow::Continue
        });
        self.tray_source.replace(Some(source));
    }

    pub fn run(&self) -> glib::ExitCode {
        self.app.run()
    }
}

// Note: No Default implementation since engine parameter is required