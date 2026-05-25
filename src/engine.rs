use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_channel::Sender;
use mtorrent::app;
use mtorrent::utils::re_exports::mtorrent_dht as dht;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::listener::GtkListener;

/// Information about a connected peer.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub address: String,
    pub client: String,
    pub speed_down: u64,
    pub speed_up: u64,
    pub encrypted: bool,
}

/// UI update payload sent from the backend to the frontend.
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
    pub peers_list: Vec<PeerInfo>,
    pub total_pieces: usize,
    pub downloaded_pieces: usize,
    pub sequential: bool,
}

/// Represents the current user-facing state of a torrent.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TorrentUiState {
    #[default]
    Downloading,
    Paused,
    Completed,
    Error,
}

/// Events emitted by the engine to update the UI.
pub enum UiEvent {
    Update(UiUpdate),
    Finished { info_hash: String, error: Option<String> },
}

#[allow(dead_code)]
#[derive(Debug)]
struct ActiveTorrent {
    canceller: Option<Arc<()>>,
    cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
    name: String,
    uri: String,
    output_dir: PathBuf,
    ui_tx: Sender<UiEvent>,
    sequential: Arc<std::sync::atomic::AtomicBool>,
}

enum EngineCmd {
    Start {
        name: String,
        uri: String,
        output_dir: PathBuf,
        canceller: Arc<()>,
        cancel_rx: tokio::sync::oneshot::Receiver<()>,
        ui_tx: Sender<UiEvent>,
        sequential: Arc<std::sync::atomic::AtomicBool>,
        pwp_port: u16,
    },
}

/// The core background engine managing torrent sessions and state.
#[derive(Debug)]
pub struct TorrentEngine {
    active: Arc<Mutex<HashMap<String, ActiveTorrent>>>,
    saved: Arc<Mutex<HashMap<String, ActiveTorrent>>>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<EngineCmd>,
    #[allow(dead_code)]
    peer_id: PeerId,
    #[allow(dead_code)]
    config_dir: PathBuf,
    #[allow(dead_code)]
    dht_sink: dht::CommandSink,
    storage: crate::storage::Storage,
}

impl TorrentEngine {
    /// Creates and initializes a new TorrentEngine instance.
    pub fn new(
        peer_id: PeerId,
        config_dir: PathBuf,
        pwp_handle: tokio::runtime::Handle,
        storage_handle: tokio::runtime::Handle,
        dht_sink: dht::CommandSink,
        storage: crate::storage::Storage,
    ) -> Self {
        log::info!("Creating torrent engine, config_dir: {:?}", config_dir);
        let active: Arc<Mutex<HashMap<String, ActiveTorrent>>> = Arc::new(Mutex::new(HashMap::new()));
        let saved: Arc<Mutex<HashMap<String, ActiveTorrent>>> = Arc::new(Mutex::new(HashMap::new()));
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<EngineCmd>();

        let config_dir_clone = config_dir.clone();
        let peer_id_clone = peer_id;
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
                            EngineCmd::Start { name, uri, output_dir, canceller, cancel_rx, ui_tx, sequential, pwp_port } => {
                                let pid = peer_id_clone;
                                let cd = config_dir_clone.clone();
                                let pwp = pwp_clone.clone();
                                let stor = storage_clone.clone();
                                let dht = dht_clone.clone();
                                let info_hash = hash_uri(&uri);

                                tokio::task::spawn_local(async move {
                                    let downloaded_bytes = Arc::new(Mutex::new(0u64));
                                    let total_bytes = Arc::new(Mutex::new(0u64));
                                    let dl_clone = Arc::clone(&downloaded_bytes);
                                    let tot_clone = Arc::clone(&total_bytes);

                                    let listener = GtkListener::new(
                                        Arc::downgrade(&canceller),
                                        ui_tx.clone(),
                                        info_hash.clone(),
                                        name.clone(),
                                        uri.clone(),
                                        output_dir.clone(),
                                        cd.clone(),
                                        downloaded_bytes,
                                        total_bytes,
                                        Arc::clone(&sequential),
                                    );
                                    let config = app::main::Config {
                                        local_peer_id: pid,
                                        output_dir: output_dir.clone(),
                                        config_dir: cd,
                                        use_upnp: false,
                                        pwp_port: Some(pwp_port),
                                        bind_interface: None,
                                    };
                                    let ctx = app::main::Context {
                                        dht_handle: Some(dht),
                                        pwp_runtime: pwp,
                                        storage_runtime: stor,
                                    };

                                    let mut rx = cancel_rx;
                                    let seq_clone = Arc::clone(&sequential);
                                    let is_seq = seq_clone.load(std::sync::atomic::Ordering::Relaxed);
                                    let result = mtorrent::utils::re_exports::mtorrent_core::SEQUENTIAL.scope(seq_clone, async {
                                        tokio::select! {
                                            res = app::main::single_torrent(&uri, listener, config, ctx) => Some(res),
                                            _ = &mut rx => {
                                                log::info!("Torrent task paused/cancelled: {}", info_hash);
                                                None
                                            }
                                        }
                                    }).await;

                                    if let Some(res) = result {
                                        if Arc::strong_count(&canceller) > 1 {
                                            match &res {
                                                Ok(_) => log::info!("Torrent completed: {}", info_hash),
                                                Err(e) => log::error!("Torrent failed: {}: {}", info_hash, e),
                                            }
                                            let _ = ui_tx
                                                .send(UiEvent::Finished {
                                                    info_hash,
                                                    error: res.err().map(|e| e.to_string()),
                                                })
                                                .await;
                                        }
                                    } else {
                                        let dl = *dl_clone.lock().unwrap();
                                        let tot = *tot_clone.lock().unwrap();
                                        let _ = ui_tx
                                            .send(UiEvent::Update(UiUpdate {
                                                info_hash,
                                                name,
                                                state: TorrentUiState::Paused,
                                                downloaded: dl,
                                                total: tot,
                                                peers: 0,
                                                speed_down: 0,
                                                speed_up: 0,
                                                output_dir,
                                                uri,
                                                peers_list: Vec::new(),
                                                total_pieces: 0,
                                                downloaded_pieces: 0,
                                                sequential: is_seq,
                                            }))
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
            storage,
        }
    }

    pub fn start(&self, name: String, uri: String, output_dir: PathBuf, sequential: bool, ui_tx: Sender<UiEvent>) -> String {
        let info_hash = hash_uri(&uri);
        let mut map = self.active.lock().unwrap();

        if map.contains_key(&info_hash) {
            log::info!("Torrent already active: {} ({})", name, info_hash);
            map.get(&info_hash).unwrap().sequential.store(sequential, std::sync::atomic::Ordering::Relaxed);
            return info_hash;
        }

        log::info!("Starting torrent: {} ({}) with sequential={}", name, info_hash, sequential);

        let canceller = Arc::new(());
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let seq = Arc::new(std::sync::atomic::AtomicBool::new(sequential));
        map.insert(info_hash.clone(), ActiveTorrent {
            canceller: Some(Arc::clone(&canceller)),
            cancel_tx: Some(cancel_tx),
            name: name.clone(),
            uri: uri.clone(),
            output_dir: output_dir.clone(),
            ui_tx: ui_tx.clone(),
            sequential: Arc::clone(&seq),
        });
        drop(map);

        // Immediately notify UI of the new downloading torrent
        let _ = ui_tx.try_send(UiEvent::Update(UiUpdate {
            info_hash: info_hash.clone(),
            name: name.clone(),
            state: TorrentUiState::Downloading,
            downloaded: 0,
            total: 0,
            peers: 0,
            speed_down: 0,
            speed_up: 0,
            output_dir: output_dir.clone(),
            uri: uri.clone(),
            peers_list: Vec::new(),
            total_pieces: 0,
            downloaded_pieces: 0,
            sequential,
        }));

        let _ = self.cmd_tx.send(EngineCmd::Start {
            name,
            uri,
            output_dir,
            canceller,
            cancel_rx,
            ui_tx,
            sequential: seq,
            pwp_port: self.storage.load_settings().pwp_port,
        });
        info_hash
    }

    /// Adds a torrent in a paused state without starting the download.
    pub fn add_paused(&self, name: String, uri: String, output_dir: PathBuf, sequential: bool, ui_tx: Sender<UiEvent>) -> String {
        let info_hash = hash_uri(&uri);
        let mut map = self.saved.lock().unwrap();

        if map.contains_key(&info_hash) {
            log::info!("Torrent already saved/paused: {} ({})", name, info_hash);
            map.get(&info_hash).unwrap().sequential.store(sequential, std::sync::atomic::Ordering::Relaxed);
            return info_hash;
        }

        log::info!("Adding paused torrent: {} ({}) with sequential={}", name, info_hash, sequential);

        let seq = Arc::new(std::sync::atomic::AtomicBool::new(sequential));
        map.insert(info_hash.clone(), ActiveTorrent {
            canceller: None,
            cancel_tx: None,
            name: name.clone(),
            uri: uri.clone(),
            output_dir: output_dir.clone(),
            ui_tx: ui_tx.clone(),
            sequential: seq,
        });
        drop(map);

        // Immediately notify UI of the new paused torrent
        let _ = ui_tx.try_send(UiEvent::Update(UiUpdate {
            info_hash: info_hash.clone(),
            name: name.clone(),
            state: TorrentUiState::Paused,
            downloaded: 0,
            total: 0,
            peers: 0,
            speed_down: 0,
            speed_up: 0,
            output_dir: output_dir.clone(),
            uri: uri.clone(),
            peers_list: Vec::new(),
            total_pieces: 0,
            downloaded_pieces: 0,
            sequential,
        }));

        info_hash
    }

    pub fn add_paused_silent(&self, name: String, uri: String, output_dir: PathBuf, sequential: bool, ui_tx: Sender<UiEvent>) -> String {
        let info_hash = hash_uri(&uri);
        let mut map = self.saved.lock().unwrap();

        if map.contains_key(&info_hash) {
            log::info!("Torrent already saved/paused: {} ({})", name, info_hash);
            map.get(&info_hash).unwrap().sequential.store(sequential, std::sync::atomic::Ordering::Relaxed);
            return info_hash;
        }

        log::info!("Adding paused torrent silently: {} ({}) with sequential={}", name, info_hash, sequential);

        let seq = Arc::new(std::sync::atomic::AtomicBool::new(sequential));
        map.insert(info_hash.clone(), ActiveTorrent {
            canceller: None,
            cancel_tx: None,
            name,
            uri,
            output_dir,
            ui_tx,
            sequential: seq,
        });

        info_hash
    }

    /// Stops and removes the torrent from the engine entirely.
    pub fn stop(&self, info_hash: &str) {
        log::info!("Stopping torrent: {}", info_hash);
        self.active.lock().unwrap().remove(info_hash);
        self.saved.lock().unwrap().remove(info_hash);
    }

    /// Sets the sequential download flag for a torrent.
    pub fn set_sequential(&self, info_hash: &str, sequential: bool) {
        log::info!("Toggling sequential for {}: {}", info_hash, sequential);
        let active_map = self.active.lock().unwrap();
        if let Some(torrent) = active_map.get(info_hash) {
            torrent.sequential.store(sequential, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        drop(active_map);
        let saved_map = self.saved.lock().unwrap();
        if let Some(torrent) = saved_map.get(info_hash) {
            torrent.sequential.store(sequential, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Toggles the torrent between paused and downloading states.
    pub fn toggle(&self, info_hash: &str) {
        let mut active_map = self.active.lock().unwrap();
        let mut saved_map = self.saved.lock().unwrap();
        if active_map.contains_key(info_hash) {
            log::info!("Pausing torrent: {}", info_hash);
            if let Some(mut torrent) = active_map.remove(info_hash) {
                torrent.canceller = None; // Drop the strong reference to trigger stop!
                torrent.cancel_tx = None; // Drop the oneshot Sender to cancel immediately!
                saved_map.insert(info_hash.to_string(), torrent);
            }
        } else if let Some(torrent) = saved_map.remove(info_hash) {
            log::info!("Resuming torrent: {}", info_hash);
            // Resume: move from saved to active, restart download
            drop(active_map);
            drop(saved_map);
            let canceller = Arc::new(());
            let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
            let ui_tx = torrent.ui_tx.clone();
            let name = torrent.name.clone();
            let seq = Arc::clone(&torrent.sequential);
            let _ = self.cmd_tx.send(EngineCmd::Start {
                name,
                uri: torrent.uri.clone(),
                output_dir: torrent.output_dir.clone(),
                canceller: Arc::clone(&canceller),
                cancel_rx,
                ui_tx: ui_tx.clone(),
                sequential: Arc::clone(&seq),
                pwp_port: self.storage.load_settings().pwp_port,
            });
            let mut active_map2 = self.active.lock().unwrap();
            active_map2.insert(info_hash.to_string(), ActiveTorrent {
                canceller: Some(canceller),
                cancel_tx: Some(cancel_tx),
                name: torrent.name,
                uri: torrent.uri,
                output_dir: torrent.output_dir,
                ui_tx,
                sequential: seq,
            });
        }
    }

    /// Pauses all currently active torrents.
    pub fn pause_all(&self) {
        log::info!("Pausing all active torrents in engine");
        let mut active_map = self.active.lock().unwrap();
        let mut saved_map = self.saved.lock().unwrap();
        let active_keys: Vec<String> = active_map.keys().cloned().collect();
        for info_hash in active_keys {
            if let Some(mut torrent) = active_map.remove(&info_hash) {
                torrent.canceller = None;
                torrent.cancel_tx = None;
                saved_map.insert(info_hash, torrent);
            }
        }
    }

    /// Returns true if the torrent is currently active and downloading/seeding.
    pub fn is_active(&self, info_hash: &str) -> bool {
        self.active.lock().unwrap().contains_key(info_hash)
    }

    /// Returns true if the torrent is paused.
    pub fn is_paused(&self, info_hash: &str) -> bool {
        self.saved.lock().unwrap().contains_key(info_hash)
    }

    /// Retrieves basic active state information about a torrent.
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
    use sha1::{Sha1, Digest};
    let mut hasher = Sha1::new();
    hasher.update(uri.as_bytes());
    format!("{:x}", hasher.finalize())
}
