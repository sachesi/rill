mod application;
mod engine;
mod listener;
mod model;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use mtorrent as mt;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::engine::TorrentEngine;
use crate::application::RillApplication;

fn main() {
    pretty_env_logger::init();
    gtk::init().expect("Failed to initialize GTK");

    // Register GResource
    let resource_data = include_bytes!(concat!(env!("OUT_DIR"), "/rill.gresource"));
    let resource = gtk::gio::Resource::from_data(&gtk::glib::Bytes::from_static(resource_data))
        .expect("Failed to load GResource");
    gtk::gio::resources_register(&resource);

    let local_data_dir = dirs_next::data_local_dir()
        .or_else(dirs_next::data_dir)
        .or_else(dirs_next::config_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let local_data_dir = local_data_dir.join("rill");
    let _ = std::fs::create_dir_all(&local_data_dir);
    log::info!("Data directory: {}", local_data_dir.display());

    let storage_handle = tokio::runtime::Handle::current();
    let pwp_handle = tokio::runtime::Handle::current();

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
        pwp_handle,
        storage_handle,
        dht_cmds,
    )));

    let app = RillApplication::new(engine);
    std::process::exit(app.run().into());
}
