mod engine;
mod listener;
mod ui;

use std::path::PathBuf;
use std::rc::Rc;
use std::cell::RefCell;

use mtorrent as mt;
use mt::utils::re_exports::mtorrent_utils::{peer_id::PeerId, worker};
use gtk::prelude::*;

use crate::engine::TorrentEngine;

const APP_ID: &str = "xyz.sachesi.Rill";

fn main() {
    pretty_env_logger::init();
    gtk::init().expect("Failed to initialize GTK");

    let local_data_dir = dirs_next::data_local_dir()
        .or_else(dirs_next::data_dir)
        .or_else(dirs_next::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let local_data_dir = local_data_dir.join("rill");
    let _ = std::fs::create_dir_all(&local_data_dir);
    log::info!("Data directory: {}", local_data_dir.display());

    let storage_worker = worker::with_runtime(worker::rt::Config {
        name: "storage".to_owned(),
        io_enabled: false,
        time_enabled: false,
        ..Default::default()
    })
    .expect("Failed to create storage runtime");

    let pwp_worker = worker::with_local_runtime(worker::rt::Config {
        name: "pwp".to_owned(),
        io_enabled: true,
        time_enabled: true,
        max_blocking_threads: 32,
        ..Default::default()
    })
    .expect("Failed to create pwp runtime");

    let (_dht_worker, dht_cmds) = mt::app::dht::launch_dht_node_runtime(mt::app::dht::Config {
        local_port: 6881,
        max_concurrent_queries: Some(0),
        config_dir: local_data_dir.clone(),
        use_upnp: true,
        bootstrap_nodes_override: None,
        bind_interface: None,
        query_timeout: None,
    })
    .expect("Failed to launch DHT");

    let peer_id = PeerId::generate_new();

    let engine = Rc::new(RefCell::new(TorrentEngine::new(
        peer_id,
        local_data_dir.clone(),
        pwp_worker.runtime_handle().clone(),
        storage_worker.runtime_handle().clone(),
        dht_cmds,
    )));

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    let engine_weak = Rc::downgrade(&engine);
    app.connect_activate(move |app| {
        if let Some(engine) = engine_weak.upgrade() {
            ui::build_ui(app, engine);
        }
    });

    app.run();
}
