use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_channel::Sender;
use mtorrent::app;
use mtorrent::utils::re_exports::mtorrent_dht as dht;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::listener::GtkListener;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct UiUpdate {
    pub info_hash: String,
    pub name: String,
    pub state: TorrentUiState,
    pub downloaded: u64,
    pub total: u64,
    pub peers: usize,
    pub speed_down: u64,
    pub speed_up: u64,
    pub output_dir: PathBuf,
    pub uri: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TorrentUiState {
    #[default]
    Downloading,
    Paused,
    Completed,
    Error,
}

pub enum UiEvent {
    Update(UiUpdate),
    Finished { info_hash: String, error: Option<String> },
}

#[allow(dead_code)]
#[derive(Debug)]
struct ActiveTorrent {
    canceller: Arc<()>,
    name: String,
    uri: String,
    output_dir: PathBuf,
    ui_tx: Sender<UiEvent>,
}

enum EngineCmd {
    Start {
        name: String,
        uri: String,
        output_dir: PathBuf,
        canceller: Arc<()>,
        ui_tx: Sender<UiEvent>,
    },
}

#[derive(Debug)]
pub struct TorrentEngine {
    active: Arc<Mutex<HashMap<String, ActiveTorrent>>>,
    saved: Arc<Mutex<HashMap<String, ActiveTorrent>>>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<EngineCmd>,
    peer_id: PeerId,
    config_dir: PathBuf,
    dht_sink: dht::CommandSink,
}

impl TorrentEngine {
    pub fn new(
        peer_id: PeerId,
        config_dir: PathBuf,
        pwp_handle: tokio::runtime::Handle,
        storage_handle: tokio::runtime::Handle,
        dht_sink: dht::CommandSink,
    ) -> Self {
        let active: Arc<Mutex<HashMap<String, ActiveTorrent>>> = Arc::new(Mutex::new(HashMap::new()));
        let saved: Arc<Mutex<HashMap<String, ActiveTorrent>>> = Arc::new(Mutex::new(HashMap::new()));
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<EngineCmd>();

        let config_dir_clone = config_dir.clone();
        let peer_id_clone = peer_id.clone();
        let pwp_clone = pwp_handle.clone();
        let storage_clone = storage_handle.clone();
        let dht_clone = dht_sink.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                let local = tokio::task::LocalSet::new();
                local.run_until(async {
                    while let Some(cmd) = cmd_rx.recv().await {
                        match cmd {
                            EngineCmd::Start { name, uri, output_dir, canceller, ui_tx } => {
                                let pid = peer_id_clone.clone();
                                let cd = config_dir_clone.clone();
                                let pwp = pwp_clone.clone();
                                let stor = storage_clone.clone();
                                let dht = dht_clone.clone();
                                let info_hash = hash_uri(&uri);

                                tokio::task::spawn_local(async move {
                                    let listener = GtkListener::new(
                                        Arc::downgrade(&canceller),
                                        ui_tx.clone(),
                                        info_hash.clone(),
                                        name,
                                        uri.clone(),
                                        output_dir.clone(),
                                    );
                                    let config = app::main::Config {
                                        local_peer_id: pid,
                                        output_dir,
                                        config_dir: cd,
                                        use_upnp: true,
                                        pwp_port: None,
                                        bind_interface: None,
                                    };
                                    let ctx = app::main::Context {
                                        dht_handle: Some(dht),
                                        pwp_runtime: pwp,
                                        storage_runtime: stor,
                                    };

                                    let result = app::main::single_torrent(&uri, listener, config, ctx).await;
                                    // If strong_count == 1, only background task holds the Arc
                                    // → engine dropped its ref (user paused). Don't send Finished.
                                    if Arc::strong_count(&canceller) > 1 {
                                        let _ = ui_tx
                                            .send(UiEvent::Finished {
                                                info_hash,
                                                error: result.err().map(|e| e.to_string()),
                                            })
                                            .await;
                                    }
                                });
                            }
                        }
                    }
                }).await;
            });
        });

        Self {
            active,
            saved,
            cmd_tx,
            peer_id,
            config_dir,
            dht_sink,
        }
    }

    pub fn start(&self, name: String, uri: String, output_dir: PathBuf, ui_tx: Sender<UiEvent>) -> String {
        let info_hash = hash_uri(&uri);
        let mut map = self.active.lock().unwrap();

        if map.contains_key(&info_hash) {
            return info_hash;
        }

        let canceller = Arc::new(());
        map.insert(info_hash.clone(), ActiveTorrent {
            canceller: Arc::clone(&canceller),
            name: name.clone(),
            uri: uri.clone(),
            output_dir: output_dir.clone(),
            ui_tx: ui_tx.clone(),
        });
        drop(map);

        let _ = self.cmd_tx.send(EngineCmd::Start {
            name,
            uri,
            output_dir,
            canceller,
            ui_tx,
        });
        info_hash
    }

    pub fn stop(&self, info_hash: &str) {
        self.active.lock().unwrap().remove(info_hash);
        self.saved.lock().unwrap().remove(info_hash);
    }

    pub fn toggle(&self, info_hash: &str) {
        let mut active_map = self.active.lock().unwrap();
        let mut saved_map = self.saved.lock().unwrap();
        if active_map.contains_key(info_hash) {
            // Pause: move from active to saved
            if let Some(torrent) = active_map.remove(info_hash) {
                saved_map.insert(info_hash.to_string(), torrent);
            }
        } else if let Some(torrent) = saved_map.remove(info_hash) {
            // Resume: move from saved to active, restart download
            drop(active_map);
            drop(saved_map);
            let canceller = Arc::new(());
            let ui_tx = torrent.ui_tx.clone();
            let name = torrent.name.clone();
            let _ = self.cmd_tx.send(EngineCmd::Start {
                name,
                uri: torrent.uri.clone(),
                output_dir: torrent.output_dir.clone(),
                canceller: Arc::clone(&canceller),
                ui_tx: ui_tx.clone(),
            });
            let mut active_map2 = self.active.lock().unwrap();
            active_map2.insert(info_hash.to_string(), ActiveTorrent {
                canceller,
                name: torrent.name,
                uri: torrent.uri,
                output_dir: torrent.output_dir,
                ui_tx,
            });
        }
    }

    pub fn is_active(&self, info_hash: &str) -> bool {
        self.active.lock().unwrap().contains_key(info_hash)
    }

    pub fn is_paused(&self, info_hash: &str) -> bool {
        self.saved.lock().unwrap().contains_key(info_hash)
    }

    pub fn saved_torrent(&self, info_hash: &str) -> Option<(String, PathBuf)> {
        let map = self.active.lock().unwrap();
        if let Some(t) = map.get(info_hash) {
            return Some((t.uri.clone(), t.output_dir.clone()));
        }
        drop(map);
        let saved_map = self.saved.lock().unwrap();
        saved_map.get(info_hash).map(|t| (t.uri.clone(), t.output_dir.clone()))
    }
}

fn hash_uri(uri: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    uri.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
