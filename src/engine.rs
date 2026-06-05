use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

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
    /// Downsampled piece-availability map (0..=255 fill per segment, in piece
    /// order from start to finish). Empty when no real state is available.
    pub piece_map: Vec<u8>,
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
    Finished {
        info_hash: String,
        error: Option<String>,
    },
}

#[allow(dead_code)]
#[derive(Debug)]
struct ActiveTorrent {
    canceller: Option<Arc<()>>,
    cancel_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Set true when this torrent is paused/stopped, so the listener can detect
    /// cancellation atomically rather than racing on `Arc::strong_count`.
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
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
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
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
    cmd_tx: tokio::sync::mpsc::Sender<EngineCmd>,
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
        let active: Arc<Mutex<HashMap<String, ActiveTorrent>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let saved: Arc<Mutex<HashMap<String, ActiveTorrent>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Bounded so a wedged recv loop applies backpressure instead of growing
        // the queue without limit. The loop drains commands promptly in normal
        // operation, so the capacity is never approached.
        const CMD_QUEUE_CAP: usize = 256;
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<EngineCmd>(CMD_QUEUE_CAP);

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
                            EngineCmd::Start { name, uri, output_dir, canceller, cancel_rx, cancel_flag, ui_tx, sequential, pwp_port } => {
                                let pid = peer_id_clone;
                                let cd = config_dir_clone.clone();
                                let pwp = pwp_clone.clone();
                                let stor = storage_clone.clone();
                                let dht = dht_clone.clone();
                                let info_hash = hash_uri(&uri);
                                // mtorrent derives the metainfo filename and the
                                // download subfolder from the magnet's `dn` value,
                                // then writes the fetched metainfo with a bare
                                // fs::write (no parent mkdir). A `dn` containing a
                                // path separator points at a non-existent subdir, so
                                // the write fails with ENOENT ("No such file or
                                // directory") right after metadata is fetched.
                                // Sanitise `dn` so the derived path stays inside the
                                // output dir.
                                let uri = sanitize_magnet_dn(&uri);

                                tokio::task::spawn_local(async move {
                                    // Ensure the download dir exists; mtorrent's magnet
                                    // preliminary stage writes the fetched metainfo into
                                    // output_dir before content storage is created, which
                                    // fails with ENOENT if the dir is missing.
                                    if let Err(e) = std::fs::create_dir_all(&output_dir) {
                                        log::warn!("Failed to create output dir {:?}: {}", output_dir, e);
                                    }

                                    let downloaded_bytes = Arc::new(Mutex::new(0u64));
                                    let total_bytes = Arc::new(Mutex::new(0u64));
                                    let dl_clone = Arc::clone(&downloaded_bytes);
                                    let tot_clone = Arc::clone(&total_bytes);

                                    let listener = GtkListener::new(
                                        Arc::downgrade(&canceller),
                                        Arc::clone(&cancel_flag),
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
                                        // Port 0 means "unset": let mtorrent pick a stable port
                                        // (port_from_hash) instead of binding an ephemeral one and
                                        // announcing port 0 to trackers.
                                        pwp_port: (pwp_port != 0).then_some(pwp_port),
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
                                        let dl = *lock_recover(&dl_clone, "downloaded bytes");
                                        let tot = *lock_recover(&tot_clone, "total bytes");
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
                                                piece_map: Vec::new(),
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

    pub fn start(
        &self,
        name: String,
        uri: String,
        output_dir: PathBuf,
        sequential: bool,
        ui_tx: Sender<UiEvent>,
    ) -> String {
        let info_hash = hash_uri(&uri);
        let mut map = lock_recover(&self.active, "active map");

        if let Some(existing) = map.get(&info_hash) {
            log::info!("Torrent already active: {} ({})", name, info_hash);
            existing
                .sequential
                .store(sequential, std::sync::atomic::Ordering::Relaxed);
            // Re-add with a possibly-changed sequential flag: notify the UI so the
            // displayed setting does not go stale. Zeroed counters are backfilled
            // from the previous update by the UI's coalescing logic.
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
                piece_map: Vec::new(),
            }));
            return info_hash;
        }

        log::info!(
            "Starting torrent: {} ({}) with sequential={}",
            name,
            info_hash,
            sequential
        );

        let pwp_port = self.storage.pwp_port();
        let canceller = Arc::new(());
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seq = Arc::new(std::sync::atomic::AtomicBool::new(sequential));
        if let Err(e) = self.cmd_tx.try_send(EngineCmd::Start {
            name: name.clone(),
            uri: uri.clone(),
            output_dir: output_dir.clone(),
            canceller: Arc::clone(&canceller),
            cancel_rx,
            cancel_flag: Arc::clone(&cancel_flag),
            ui_tx: ui_tx.clone(),
            sequential: Arc::clone(&seq),
            pwp_port,
        }) {
            drop(map);
            log::error!("Failed to queue torrent start {}: {}", info_hash, e);
            let _ = ui_tx.try_send(UiEvent::Finished {
                info_hash: info_hash.clone(),
                error: Some("Engine unavailable".into()),
            });
            return info_hash;
        }
        map.insert(
            info_hash.clone(),
            ActiveTorrent {
                canceller: Some(canceller),
                cancel_tx: Some(cancel_tx),
                cancel_flag,
                name: name.clone(),
                uri: uri.clone(),
                output_dir: output_dir.clone(),
                ui_tx: ui_tx.clone(),
                sequential: seq,
            },
        );
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
            piece_map: Vec::new(),
        }));
        info_hash
    }

    /// Adds a torrent in a paused state without starting the download.
    pub fn add_paused(
        &self,
        name: String,
        uri: String,
        output_dir: PathBuf,
        sequential: bool,
        ui_tx: Sender<UiEvent>,
    ) -> String {
        let info_hash = hash_uri(&uri);
        let mut map = lock_recover(&self.saved, "saved map");

        if let Some(existing) = map.get(&info_hash) {
            log::info!("Torrent already saved/paused: {} ({})", name, info_hash);
            existing
                .sequential
                .store(sequential, std::sync::atomic::Ordering::Relaxed);
            // Notify the UI of the (possibly changed) sequential flag on re-add.
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
                piece_map: Vec::new(),
            }));
            return info_hash;
        }

        log::info!(
            "Adding paused torrent: {} ({}) with sequential={}",
            name,
            info_hash,
            sequential
        );

        let seq = Arc::new(std::sync::atomic::AtomicBool::new(sequential));
        map.insert(
            info_hash.clone(),
            ActiveTorrent {
                canceller: None,
                cancel_tx: None,
                cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                name: name.clone(),
                uri: uri.clone(),
                output_dir: output_dir.clone(),
                ui_tx: ui_tx.clone(),
                sequential: seq,
            },
        );
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
            piece_map: Vec::new(),
        }));

        info_hash
    }

    pub fn add_paused_silent(
        &self,
        name: String,
        uri: String,
        output_dir: PathBuf,
        sequential: bool,
        ui_tx: Sender<UiEvent>,
    ) -> String {
        let info_hash = hash_uri(&uri);
        let mut map = lock_recover(&self.saved, "saved map");

        if let Some(existing) = map.get(&info_hash) {
            log::info!("Torrent already saved/paused: {} ({})", name, info_hash);
            existing
                .sequential
                .store(sequential, std::sync::atomic::Ordering::Relaxed);
            return info_hash;
        }

        log::info!(
            "Adding paused torrent silently: {} ({}) with sequential={}",
            name,
            info_hash,
            sequential
        );

        let seq = Arc::new(std::sync::atomic::AtomicBool::new(sequential));
        map.insert(
            info_hash.clone(),
            ActiveTorrent {
                canceller: None,
                cancel_tx: None,
                cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                name,
                uri,
                output_dir,
                ui_tx,
                sequential: seq,
            },
        );

        info_hash
    }

    /// Stops and removes the torrent from the engine entirely.
    pub fn stop(&self, info_hash: &str) {
        log::info!("Stopping torrent: {}", info_hash);
        let mut active = lock_recover(&self.active, "active map");
        if let Some(torrent) = active.get(info_hash) {
            // Signal the listener atomically before tearing down, so an in-flight
            // snapshot cannot emit a stale update after removal.
            torrent
                .cancel_flag
                .store(true, std::sync::atomic::Ordering::Release);
        }
        active.remove(info_hash);
        drop(active);
        lock_recover(&self.saved, "saved map").remove(info_hash);
    }

    /// Sets the sequential download flag for a torrent.
    pub fn set_sequential(&self, info_hash: &str, sequential: bool) {
        log::info!("Toggling sequential for {}: {}", info_hash, sequential);
        let active_map = lock_recover(&self.active, "active map");
        if let Some(torrent) = active_map.get(info_hash) {
            torrent
                .sequential
                .store(sequential, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        drop(active_map);
        let saved_map = lock_recover(&self.saved, "saved map");
        if let Some(torrent) = saved_map.get(info_hash) {
            torrent
                .sequential
                .store(sequential, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Toggles the torrent between paused and downloading states.
    pub fn toggle(&self, info_hash: &str) {
        let mut active_map = lock_recover(&self.active, "active map");
        let mut saved_map = lock_recover(&self.saved, "saved map");
        if active_map.contains_key(info_hash) {
            log::info!("Pausing torrent: {}", info_hash);
            if let Some(mut torrent) = active_map.remove(info_hash) {
                torrent
                    .cancel_flag
                    .store(true, std::sync::atomic::Ordering::Release); // Signal listener atomically.
                torrent.canceller = None; // Drop the strong reference to trigger stop!
                torrent.cancel_tx = None; // Drop the oneshot Sender to cancel immediately!
                saved_map.insert(info_hash.to_string(), torrent);
            }
        } else if let Some(torrent) = saved_map.remove(info_hash) {
            log::info!("Resuming torrent: {}", info_hash);
            // Resume: move from saved to active, restart download.
            // Re-acquire `active` under one lock spanning the check-and-insert so a
            // concurrent start/toggle for the same hash cannot double-dispatch.
            drop(saved_map);
            let mut active_map2 = active_map;
            if active_map2.contains_key(info_hash) {
                log::info!(
                    "Torrent already active during resume, skipping: {}",
                    info_hash
                );
                return;
            }
            let canceller = Arc::new(());
            let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
            let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let ui_tx = torrent.ui_tx.clone();
            let name = torrent.name.clone();
            let seq = Arc::clone(&torrent.sequential);
            if let Err(e) = self.cmd_tx.try_send(EngineCmd::Start {
                name,
                uri: torrent.uri.clone(),
                output_dir: torrent.output_dir.clone(),
                canceller: Arc::clone(&canceller),
                cancel_rx,
                cancel_flag: Arc::clone(&cancel_flag),
                ui_tx: ui_tx.clone(),
                sequential: Arc::clone(&seq),
                pwp_port: self.storage.pwp_port(),
            }) {
                log::error!("Failed to queue torrent resume {}: {}", info_hash, e);
                let _ = ui_tx.try_send(UiEvent::Finished {
                    info_hash: info_hash.to_string(),
                    error: Some("Engine unavailable".into()),
                });
                drop(active_map2);
                lock_recover(&self.saved, "saved map").insert(info_hash.to_string(), torrent);
                return;
            }
            active_map2.insert(
                info_hash.to_string(),
                ActiveTorrent {
                    canceller: Some(canceller),
                    cancel_tx: Some(cancel_tx),
                    cancel_flag,
                    name: torrent.name,
                    uri: torrent.uri,
                    output_dir: torrent.output_dir,
                    ui_tx,
                    sequential: seq,
                },
            );
        }
    }

    /// Pauses all currently active torrents.
    pub fn pause_all(&self) {
        log::info!("Pausing all active torrents in engine");
        let mut active_map = lock_recover(&self.active, "active map");
        let mut saved_map = lock_recover(&self.saved, "saved map");
        let active_keys: Vec<String> = active_map.keys().cloned().collect();
        for info_hash in active_keys {
            if let Some(mut torrent) = active_map.remove(&info_hash) {
                torrent
                    .cancel_flag
                    .store(true, std::sync::atomic::Ordering::Release);
                torrent.canceller = None;
                torrent.cancel_tx = None;
                saved_map.insert(info_hash, torrent);
            }
        }
    }

    /// Returns true if the torrent is currently active and downloading/seeding.
    pub fn is_active(&self, info_hash: &str) -> bool {
        lock_recover(&self.active, "active map").contains_key(info_hash)
    }

    /// Returns true if the torrent is paused.
    pub fn is_paused(&self, info_hash: &str) -> bool {
        lock_recover(&self.saved, "saved map").contains_key(info_hash)
    }

    /// Retrieves basic active state information about a torrent.
    pub fn saved_torrent(&self, info_hash: &str) -> Option<(String, PathBuf)> {
        let map = lock_recover(&self.active, "active map");
        if let Some(t) = map.get(info_hash) {
            return Some((t.uri.clone(), t.output_dir.clone()));
        }
        drop(map);
        let saved_map = lock_recover(&self.saved, "saved map");
        saved_map
            .get(info_hash)
            .map(|t| (t.uri.clone(), t.output_dir.clone()))
    }

    /// Returns the engine's config directory path.
    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }
}

/// Rewrites the `dn` (display name) parameter of a magnet URI so it cannot
/// contain path separators or other filesystem-hostile characters. mtorrent
/// joins the decoded `dn` straight onto the output directory to form the
/// metainfo filename and the content subfolder; an unsanitised `dn` such as
/// "Show / Season 1" yields a path with a missing intermediate directory and
/// the metainfo write fails with ENOENT. Non-magnet URIs are returned
/// unchanged.
pub(crate) fn sanitize_magnet_dn(uri: &str) -> String {
    let Some(dn_pos) = uri.find("dn=") else {
        return uri.to_string();
    };
    let value_start = dn_pos + 3;
    let value_end = uri[value_start..]
        .find('&')
        .map(|i| value_start + i)
        .unwrap_or(uri.len());

    let raw = &uri[value_start..value_end];
    let decoded = urlencoding::decode(raw)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| raw.to_string());

    let cleaned: String = decoded
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    let cleaned = if cleaned.is_empty() {
        "torrent".to_string()
    } else {
        cleaned
    };

    if cleaned == raw {
        return uri.to_string();
    }
    let encoded = urlencoding::encode(&cleaned);
    format!("{}{}{}", &uri[..value_start], encoded, &uri[value_end..])
}

/// Locks a mutex, recovering from poisoning instead of panicking. A panic in one
/// critical section must not cascade into every later operation; the poison is
/// logged so silent state corruption is at least traceable.
fn lock_recover<'a, T>(m: &'a Mutex<T>, what: &str) -> MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|e| {
        log::error!("Mutex poison detected on {}; recovering inner state", what);
        e.into_inner()
    })
}

fn hash_uri(uri: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(uri.as_bytes());
    format!("{:x}", hasher.finalize())
}
