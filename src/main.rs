mod application;
mod engine;
mod listener;
mod logging;
mod model;
mod storage;
mod util;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use mtorrent as mt;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::engine::TorrentEngine;
use crate::application::RillApplication;
use crate::storage::Storage;

fn main() {
    // Initialize logging (trace max, filtered at runtime via set_max_level)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trace"))
        .filter_module("mtorrent::app::dht", log::LevelFilter::Warn)
        .filter_module("mtorrent::app::main", log::LevelFilter::Warn)
        .filter_module("mtorrent_core::utp", log::LevelFilter::Error)
        // Routine peer churn (RESET / interrupted / bad ack) logged at ERROR upstream; not actionable.
        .filter_module("mtorrent_core::utp::handle", log::LevelFilter::Off)
        .filter_module("mtorrent_core::utp::udp", log::LevelFilter::Off)
        .init();

    // Log panics from any thread before delegating to the default hook. The PWP
    // and storage runtimes run on detached threads whose JoinHandles are dropped,
    // so a panic there would otherwise surface only on stderr; routing it through
    // the logger makes such failures visible in the application's log.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("Thread panicked: {}", info);
        default_hook(info);
    }));

    gtk::init().expect("Failed to initialize GTK");

    // Register GResource
    let resource_data = include_bytes!(concat!(env!("OUT_DIR"), "/rill.gresource"));
    let resource = gtk::gio::Resource::from_data(&gtk::glib::Bytes::from_static(resource_data))
        .expect("Failed to load GResource");
    gtk::gio::resources_register(&resource);

    // Make the bundled app icon resolvable by name (works uninstalled too).
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display)
            .add_resource_path("/com/github/sachesi/rill/icons");
    }
    gtk::Window::set_default_icon_name("com.github.sachesi.rill");

    // Build and register the application before any heavy initialization so we
    // can enforce a single running instance. Registering claims the application
    // id on the session bus; a second launch becomes "remote" and must forward
    // its activation (or magnet/file arguments) to the primary instance and exit
    // before opening the database or binding the DHT port.
    let app = adw::Application::builder()
        .application_id("com.github.sachesi.rill")
        .flags(gtk::gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    if let Err(e) = app.register(gtk::gio::Cancellable::NONE) {
        log::error!("Failed to register application: {}", e);
    }
    if app.is_remote() {
        // A primary instance already owns the id. Hand control to run(), which
        // forwards this launch's command line (a magnet/file argument, or a bare
        // activation) to the primary over the bus and exits — flushing the call
        // before the process terminates. No heavy init runs here.
        log::info!("Another instance is already running; forwarding launch to it");
        std::process::exit(app.run().into());
    }

    let local_data_dir = dirs_next::data_local_dir()
        .or_else(dirs_next::data_dir)
        .or_else(dirs_next::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let local_data_dir = local_data_dir.join("rill");
    let _ = std::fs::create_dir_all(&local_data_dir);
    log::info!("Data directory: {}", local_data_dir.display());

    // Initialize storage
    let db_path = local_data_dir.join("torrents.db");
    let storage = match Storage::open(db_path.clone()) {
        Ok(s) => {
            log::info!("Database opened: {}", db_path.display());
            s
        }
        Err(e) => {
            log::error!("Failed to open database: {}", e);
            log::warn!("Starting with empty session");
            // If DB fails, create in-memory fallback would go here
            // For now, just exit - we'll add graceful fallback in Phase 2.4
            eprintln!("Fatal: Could not initialize database: {}", e);
            std::process::exit(1);
        }
    };

    // Apply runtime log settings from storage
    logging::apply_settings(&storage.load_settings());

    // Create a background tokio runtime and enter it so Handle::current() works.
    // mtorrent's DHT/engine functions spawn tasks via the current runtime's handle.
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let _guard = rt.enter();

    // mtorrent needs current_thread runtimes for PWP and storage (spawn_local).
    // LocalRuntime (build_local) auto-provides LocalSet context for spawned tasks.
    // Builders are Send, LocalRuntimes are !Send — build on the background thread.
    let mut pwp_builder = tokio::runtime::Builder::new_current_thread();
    pwp_builder.enable_all();
    let (pwp_tx, pwp_rx) = tokio::sync::oneshot::channel();

    let mut storage_builder = tokio::runtime::Builder::new_current_thread();
    storage_builder.enable_all();
    let (storage_tx, storage_rx) = tokio::sync::oneshot::channel();

    std::thread::spawn(move || {
        let rt = pwp_builder.build_local(Default::default())
            .expect("Failed to create PWP runtime");
        let handle = rt.handle().clone();
        pwp_tx.send(handle).ok();
        rt.block_on(std::future::pending::<()>());
    });
    std::thread::spawn(move || {
        let rt = storage_builder.build_local(Default::default())
            .expect("Failed to create storage runtime");
        let handle = rt.handle().clone();
        storage_tx.send(handle).ok();
        rt.block_on(std::future::pending::<()>());
    });

    let pwp_handle = pwp_rx.blocking_recv().expect("PWP runtime failed");
    let storage_handle = storage_rx.blocking_recv().expect("Storage runtime failed");

    let (_dht_worker, dht_cmds) = mt::app::dht::launch_dht_node_runtime(mt::app::dht::Config {
        local_port: 6881,
            max_concurrent_queries: Some(10),
        config_dir: local_data_dir.clone(),
        use_upnp: false,
        bootstrap_nodes_override: None,
        bind_interface: None,
        query_timeout: None,
    })
    .expect("Failed to launch DHT");

    let peer_id = PeerId::generate_new();

    let engine = Rc::new(RefCell::new(TorrentEngine::new(
        peer_id,
        local_data_dir.clone(),
        pwp_handle,
        storage_handle,
        dht_cmds,
        storage.clone(),
    )));

    // Load saved torrents
    let saved_torrents = storage.load_torrents().unwrap_or_else(|e| {
        log::warn!("Failed to load torrents: {}", e);
        Vec::new()
    });
    log::info!("Loaded {} saved torrents from database", saved_torrents.len());

    let rill = RillApplication::new(app, engine, storage, saved_torrents);
    std::process::exit(rill.run().into());
}
