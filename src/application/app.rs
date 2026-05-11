use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::engine::TorrentEngine;
use crate::model::TorrentModel;
use super::RillWindow;
use super::show_preferences;

pub struct RillApplication {
    app: adw::Application,
    engine: Rc<RefCell<TorrentEngine>>,
    model: Rc<RefCell<TorrentModel>>,
}

impl RillApplication {
    pub fn new(engine: Rc<RefCell<TorrentEngine>>) -> Self {
        let app = adw::Application::builder()
            .application_id("com.github.sachesi.rill")
            .build();

        let model = Rc::new(RefCell::new(TorrentModel::new()));

        let app_self = Self {
            app: app.clone(),
            engine: engine.clone(), 
            model: model.clone(),
        };

        // Set up application-level actions
        app_self.setup_actions();
        
        // Connect activate signal
        app.connect_activate(glib::clone!(
            #[weak] engine,
            #[weak] model,
            move |app| {
                let window = RillWindow::new(engine.clone(), model.clone(), app);
                window.present();
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
                    .application_icon("com.github.sachesi.rill")
                    .developer_name("sachesi")
                    .version("0.1.0")
                    .comments("Minimalistic BitTorrent client for GNOME")
                    .license_type(gtk::License::Gpl30)
                    .website("https://github.com/sachesi/rill")
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
        prefs_action.connect_activate(move |_, _| {
            if let Some(app) = app_weak.upgrade() {
                if let Some(window) = app.active_window() {
                    if let Ok(rill_window) = window.downcast::<RillWindow>() {
                        show_preferences(rill_window.upcast_ref::<adw::ApplicationWindow>());
                    }
                }
            }
        });
        self.app.add_action(&prefs_action);
        self.app.set_accels_for_action("app.preferences", &["<Control>comma"]);

        // Add torrent action
        let add_action = gio::SimpleAction::new("add-torrent", None);
        let app_weak = self.app.downgrade();
        add_action.connect_activate(move |_, _| {
            if let Some(app) = app_weak.upgrade() {
                if let Some(window) = app.active_window() {
                    if let Ok(rill_window) = window.downcast::<RillWindow>() {
                        rill_window.show_add_dialog();
                    }
                }
            }
        });
        self.app.add_action(&add_action);
        self.app.set_accels_for_action("app.add-torrent", &["<Control>n"]);
    }

    pub fn run(&self) -> glib::ExitCode {
        self.app.run()
    }
}

// Note: No Default implementation since engine parameter is required