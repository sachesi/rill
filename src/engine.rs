use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_channel::Sender;
use mtorrent::app;
use mtorrent::utils::re_exports::mtorrent_dht as dht;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::listener::GtkListener;

/// A connected peer.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub address: String,
    /// The client the peer says it runs, when it says.
    pub client: Option<String>,
    pub speed_down: u64,
    pub speed_up: u64,
    pub encrypted: bool,
}

/// A snapshot of one torrent, sent from the engine to the window.
#[derive(Clone, Debug)]
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

impl UiUpdate {
    /// A snapshot without transfer figures, for a torrent that is not transferring.
    pub fn idle(
        info_hash: String,
        name: String,
        state: TorrentUiState,
        output_dir: PathBuf,
        uri: String,
        sequential: bool,
    ) -> Self {
        Self {
            info_hash,
            name,
            state,
            downloaded: 0,
            total: 0,
            peers: 0,
            speed_down: 0,
            speed_up: 0,
            output_dir,
            uri,
            peers_list: Vec::new(),
            total_pieces: 0,
            downloaded_pieces: 0,
            sequential,
            piece_map: Vec::new(),
        }
    }
}

/// The state of a torrent as the window shows it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TorrentUiState {
    #[default]
    Downloading,
    Paused,
    Completed,
    Error,
}

/// What the engine tells the window.
pub enum UiEvent {
    Update(UiUpdate),
    Finished {
        info_hash: String,
        error: Option<String>,
    },
}

/// Why a running torrent task is being ended.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Stop {
    /// The torrent is being paused, and the task reports the paused state as it ends.
    Pause,
    /// The task is being replaced right away by one with different settings, so the
    /// torrent keeps running and the ending task, listener included, reports nothing.
    Restart,
}

impl Stop {
    /// What the flag holds while the torrent runs.
    pub(crate) const RUNNING: u8 = 0;

    pub(crate) fn code(self) -> u8 {
        match self {
            Stop::Pause => 1,
            Stop::Restart => 2,
        }
    }

    /// The reason the flag holds, or `None` while the torrent runs.
    pub(crate) fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Stop::Pause),
            2 => Some(Stop::Restart),
            _ => None,
        }
    }
}

/// A torrent the engine knows, running or not.
#[derive(Debug)]
struct TorrentEntry {
    /// Held, not read: the task watches the count of this Arc to tell a download that
    /// ended on its own from one that was stopped here.
    _canceller: Option<Arc<()>>,
    /// Ends the running task, telling it whether the torrent is stopping or starting
    /// anew. Both are None while the torrent is not running.
    cancel_tx: Option<tokio::sync::oneshot::Sender<Stop>>,
    /// Set to the reason this torrent is ending, so the task and its listener can
    /// detect cancellation atomically rather than racing on `Arc::strong_count`, and
    /// tell a pause from a restart.
    stop_flag: Arc<AtomicU8>,
    name: String,
    uri: String,
    output_dir: PathBuf,
    ui_tx: Sender<UiEvent>,
    sequential: Arc<AtomicBool>,
    /// The port the torrent listens on while it runs; 0 when mtorrent derives it.
    port: u16,
}

impl TorrentEntry {
    fn new(
        name: String,
        uri: String,
        output_dir: PathBuf,
        sequential: bool,
        ui_tx: Sender<UiEvent>,
    ) -> Self {
        Self {
            _canceller: None,
            cancel_tx: None,
            stop_flag: Arc::new(AtomicU8::new(Stop::RUNNING)),
            name,
            uri,
            output_dir,
            ui_tx,
            sequential: Arc::new(AtomicBool::new(sequential)),
            port: 0,
        }
    }

    /// Ends the running task, if any, telling it why.
    fn halt(&mut self, reason: Stop) {
        // Signal the listener before tearing down, so an in-flight snapshot cannot
        // emit a stale update afterwards, and a restart emits none at all.
        self.stop_flag.store(reason.code(), Ordering::Release);
        self._canceller = None;
        if let Some(tx) = self.cancel_tx.take() {
            // An error means the task is already gone, which ends it just the same.
            let _ = tx.send(reason);
        }
    }

    fn idle_update(&self, info_hash: &str, state: TorrentUiState) -> UiUpdate {
        UiUpdate::idle(
            info_hash.to_string(),
            self.name.clone(),
            state,
            self.output_dir.clone(),
            self.uri.clone(),
            self.sequential.load(Ordering::Relaxed),
        )
    }
}

/// What the engine thread needs to run one torrent.
struct StartCmd {
    info_hash: String,
    name: String,
    uri: String,
    output_dir: PathBuf,
    canceller: Arc<()>,
    cancel_rx: tokio::sync::oneshot::Receiver<Stop>,
    stop_flag: Arc<AtomicU8>,
    ui_tx: Sender<UiEvent>,
    sequential: Arc<AtomicBool>,
    pwp_port: u16,
}

/// What every torrent task shares: who we are, and where mtorrent's work runs.
#[derive(Clone)]
struct Shared {
    peer_id: PeerId,
    config_dir: PathBuf,
    pwp_runtime: tokio::runtime::Handle,
    storage_runtime: tokio::runtime::Handle,
    dht: dht::CommandSink,
}

/// Starts, pauses and stops torrents. Running torrents are in `active`, the others in
/// `saved`; the tasks themselves run on a thread of the engine's own.
#[derive(Debug)]
pub struct TorrentEngine {
    active: Mutex<HashMap<String, TorrentEntry>>,
    saved: Mutex<HashMap<String, TorrentEntry>>,
    cmd_tx: tokio::sync::mpsc::Sender<StartCmd>,
    config_dir: PathBuf,
    /// The listening port setting: where torrents started from now on count up from, or 0
    /// for each to take the one mtorrent derives.
    pwp_port: AtomicU16,
}

impl TorrentEngine {
    pub fn new(
        peer_id: PeerId,
        config_dir: PathBuf,
        pwp_handle: tokio::runtime::Handle,
        storage_handle: tokio::runtime::Handle,
        dht_sink: dht::CommandSink,
        pwp_port: u16,
    ) -> Self {
        log::info!("Creating torrent engine, config_dir: {:?}", config_dir);
        // Bounded so a wedged recv loop applies backpressure instead of growing
        // the queue without limit. The loop drains commands promptly in normal
        // operation, so the capacity is never approached.
        const CMD_QUEUE_CAP: usize = 256;
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<StartCmd>(CMD_QUEUE_CAP);

        let shared = Shared {
            peer_id,
            config_dir: config_dir.clone(),
            pwp_runtime: pwp_handle,
            storage_runtime: storage_handle,
            dht: dht_sink,
        };
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build_local(Default::default())
                .unwrap();

            rt.block_on(async {
                // The last run of each torrent. A new run waits for the one before
                // it to end, which a pause or restart has already asked of it, so
                // that the two never share the torrent's files.
                let mut runs: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
                while let Some(cmd) = cmd_rx.recv().await {
                    runs.retain(|_, run| !run.is_finished());
                    let previous = runs.remove(&cmd.info_hash);
                    let hash = cmd.info_hash.clone();
                    let shared = shared.clone();
                    let run = tokio::task::spawn_local(async move {
                        if let Some(previous) = previous {
                            let _ = previous.await;
                        }
                        run_torrent(cmd, shared).await
                    });
                    runs.insert(hash, run);
                }
            });
        });

        Self {
            active: Mutex::new(HashMap::new()),
            saved: Mutex::new(HashMap::new()),
            cmd_tx,
            config_dir,
            pwp_port: AtomicU16::new(pwp_port),
        }
    }

    /// Starts the torrent `uri` names, known by its `info_hash`, so that the same content
    /// added from a magnet link and from a file is one torrent.
    pub fn start(
        &self,
        info_hash: String,
        name: String,
        uri: String,
        output_dir: PathBuf,
        sequential: bool,
        ui_tx: Sender<UiEvent>,
    ) {
        let mut map = lock_recover(&self.active, "active map");

        if let Some(existing) = map.get(&info_hash) {
            log::info!("Torrent already active: {} ({})", name, info_hash);
            existing.sequential.store(sequential, Ordering::Relaxed);
            // Re-add with a possibly-changed sequential flag: notify the UI so the
            // displayed setting does not go stale. Zeroed counters are backfilled
            // from the previous update by the UI's coalescing logic.
            let _ = ui_tx.try_send(UiEvent::Update(
                existing.idle_update(&info_hash, TorrentUiState::Downloading),
            ));
            return;
        }

        log::info!(
            "Starting torrent: {} ({}) with sequential={}",
            name,
            info_hash,
            sequential
        );

        // One held paused runs on from where it is, in its own folder, rather than stay
        // behind as a second entry of the same torrent.
        let paused = lock_recover(&self.saved, "saved map").remove(&info_hash);
        let was_paused = paused.is_some();
        let torrent = match paused {
            Some(torrent) => {
                torrent.sequential.store(sequential, Ordering::Relaxed);
                torrent
            }
            None => TorrentEntry::new(name, uri, output_dir, sequential, ui_tx.clone()),
        };
        // Immediately notify UI of the new downloading torrent
        let update = torrent.idle_update(&info_hash, TorrentUiState::Downloading);
        if let Err(err) = self.launch(&info_hash, torrent, &mut map) {
            let (torrent, e) = *err;
            drop(map);
            log::error!("Failed to queue torrent start {}: {}", info_hash, e);
            if was_paused {
                lock_recover(&self.saved, "saved map").insert(info_hash.clone(), torrent);
            }
            let _ = ui_tx.try_send(UiEvent::Finished {
                info_hash,
                error: Some("Engine unavailable".into()),
            });
            return;
        }
        drop(map);

        let _ = ui_tx.try_send(UiEvent::Update(update));
    }

    /// Adds a torrent in a paused state without starting the download.
    pub fn add_paused(
        &self,
        info_hash: String,
        name: String,
        uri: String,
        output_dir: PathBuf,
        sequential: bool,
        ui_tx: Sender<UiEvent>,
    ) {
        self.add_paused_silent(
            info_hash.clone(),
            name,
            uri,
            output_dir,
            sequential,
            ui_tx.clone(),
        );
        // Notify the UI of the new paused torrent, or of the (possibly changed)
        // sequential flag on re-add.
        if let Some(torrent) = lock_recover(&self.saved, "saved map").get(&info_hash) {
            let _ = ui_tx.try_send(UiEvent::Update(
                torrent.idle_update(&info_hash, TorrentUiState::Paused),
            ));
        }
    }

    /// Adds a torrent in a paused state, without telling the window.
    pub fn add_paused_silent(
        &self,
        info_hash: String,
        name: String,
        uri: String,
        output_dir: PathBuf,
        sequential: bool,
        ui_tx: Sender<UiEvent>,
    ) {
        let mut map = lock_recover(&self.saved, "saved map");

        if let Some(existing) = map.get(&info_hash) {
            log::info!("Torrent already saved/paused: {} ({})", name, info_hash);
            existing.sequential.store(sequential, Ordering::Relaxed);
            return;
        }

        log::info!(
            "Adding paused torrent: {} ({}) with sequential={}",
            name,
            info_hash,
            sequential
        );
        map.insert(
            info_hash,
            TorrentEntry::new(name, uri, output_dir, sequential, ui_tx),
        );
    }

    /// Hands `torrent` to the engine thread and records it as running. The torrent comes
    /// back when the thread cannot take it.
    fn launch(
        &self,
        info_hash: &str,
        mut torrent: TorrentEntry,
        active: &mut HashMap<String, TorrentEntry>,
    ) -> Result<(), Box<(TorrentEntry, String)>> {
        let canceller = Arc::new(());
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<Stop>();
        let stop_flag = Arc::new(AtomicU8::new(Stop::RUNNING));
        let port = listening_port(
            self.pwp_port.load(Ordering::Relaxed),
            &sanitize_magnet_dn(&torrent.uri),
            active.values().map(|torrent| torrent.port),
            port_is_free,
        );
        let cmd = StartCmd {
            info_hash: info_hash.to_string(),
            name: torrent.name.clone(),
            uri: torrent.uri.clone(),
            output_dir: torrent.output_dir.clone(),
            canceller: Arc::clone(&canceller),
            cancel_rx,
            stop_flag: Arc::clone(&stop_flag),
            ui_tx: torrent.ui_tx.clone(),
            sequential: Arc::clone(&torrent.sequential),
            pwp_port: port,
        };
        if let Err(e) = self.cmd_tx.try_send(cmd) {
            return Err(Box::new((torrent, e.to_string())));
        }
        torrent._canceller = Some(canceller);
        torrent.cancel_tx = Some(cancel_tx);
        torrent.stop_flag = stop_flag;
        torrent.port = port;
        active.insert(info_hash.to_string(), torrent);
        Ok(())
    }

    /// Moves a torrent that terminated with an error from the active map to the
    /// saved map, so the UI can offer resume (which restarts the task) instead of
    /// leaving a dead entry that can only be paused.
    pub fn mark_failed(&self, info_hash: &str) {
        let mut active_map = lock_recover(&self.active, "active map");
        if let Some(mut torrent) = active_map.remove(info_hash) {
            torrent.halt(Stop::Pause);
            drop(active_map);
            lock_recover(&self.saved, "saved map").insert(info_hash.to_string(), torrent);
        }
    }

    /// Stops and removes the torrent from the engine entirely.
    pub fn stop(&self, info_hash: &str) {
        log::info!("Stopping torrent: {}", info_hash);
        let mut active = lock_recover(&self.active, "active map");
        if let Some(mut torrent) = active.remove(info_hash) {
            torrent.halt(Stop::Pause);
        }
        drop(active);
        lock_recover(&self.saved, "saved map").remove(info_hash);
    }

    /// Gives a torrent the name it has learnt from its metadata, for the snapshots of its
    /// next run.
    pub fn rename(&self, info_hash: &str, name: &str) {
        for map in [&self.active, &self.saved] {
            if let Some(torrent) = lock_recover(map, "torrent map").get_mut(info_hash) {
                torrent.name = name.to_string();
            }
        }
    }

    /// Tells a torrent where its content goes from now on. A torrent whose task is
    /// still running keeps the folder it started with until it runs again.
    pub fn set_output_dir(&self, info_hash: &str, output_dir: PathBuf) {
        log::info!("Torrent {} now downloads to {:?}", info_hash, output_dir);
        for map in [&self.active, &self.saved] {
            if let Some(torrent) = lock_recover(map, "torrent map").get_mut(info_hash) {
                torrent.output_dir = output_dir.clone();
            }
        }
    }

    /// Sets the sequential download flag for a torrent. A running torrent is restarted,
    /// since mtorrent takes the download strategy when the download starts and keeps it
    /// for the rest of the run.
    pub fn set_sequential(&self, info_hash: &str, sequential: bool) {
        log::info!("Toggling sequential for {}: {}", info_hash, sequential);
        let mut active_map = lock_recover(&self.active, "active map");
        if let Some(torrent) = active_map.get(info_hash) {
            if torrent.sequential.load(Ordering::Relaxed) == sequential {
                return;
            }
            let mut torrent = active_map.remove(info_hash).expect("just found above");
            torrent.halt(Stop::Restart);
            torrent.sequential.store(sequential, Ordering::Relaxed);
            log::info!("Restarting torrent to apply sequential: {}", info_hash);
            if let Err(err) = self.launch(info_hash, torrent, &mut active_map) {
                let (torrent, e) = *err;
                log::error!("Failed to queue torrent restart {}: {}", info_hash, e);
                let _ = torrent.ui_tx.try_send(UiEvent::Finished {
                    info_hash: info_hash.to_string(),
                    error: Some("Engine unavailable".into()),
                });
                drop(active_map);
                lock_recover(&self.saved, "saved map").insert(info_hash.to_string(), torrent);
            }
            return;
        }
        drop(active_map);
        let saved_map = lock_recover(&self.saved, "saved map");
        if let Some(torrent) = saved_map.get(info_hash) {
            torrent.sequential.store(sequential, Ordering::Relaxed);
        }
    }

    /// Toggles the torrent between paused and downloading states.
    pub fn toggle(&self, info_hash: &str) {
        let mut active_map = lock_recover(&self.active, "active map");
        let mut saved_map = lock_recover(&self.saved, "saved map");
        if let Some(mut torrent) = active_map.remove(info_hash) {
            log::info!("Pausing torrent: {}", info_hash);
            torrent.halt(Stop::Pause);
            saved_map.insert(info_hash.to_string(), torrent);
        } else if let Some(torrent) = saved_map.remove(info_hash) {
            log::info!("Resuming torrent: {}", info_hash);
            // Resume: move from saved to active, restart download. `active` stays locked
            // from the check above to the insert, so a concurrent start/toggle for the
            // same hash cannot double-dispatch.
            drop(saved_map);
            if let Err(err) = self.launch(info_hash, torrent, &mut active_map) {
                let (torrent, e) = *err;
                log::error!("Failed to queue torrent resume {}: {}", info_hash, e);
                let _ = torrent.ui_tx.try_send(UiEvent::Finished {
                    info_hash: info_hash.to_string(),
                    error: Some("Engine unavailable".into()),
                });
                drop(active_map);
                lock_recover(&self.saved, "saved map").insert(info_hash.to_string(), torrent);
            }
        }
    }

    /// Pauses all currently active torrents.
    pub fn pause_all(&self) {
        log::info!("Pausing all active torrents in engine");
        let mut active_map = lock_recover(&self.active, "active map");
        let mut saved_map = lock_recover(&self.saved, "saved map");
        for (info_hash, mut torrent) in active_map.drain() {
            torrent.halt(Stop::Pause);
            saved_map.insert(info_hash, torrent);
        }
    }

    /// Returns true if the torrent is currently active and downloading/seeding.
    pub fn is_active(&self, info_hash: &str) -> bool {
        lock_recover(&self.active, "active map").contains_key(info_hash)
    }

    /// Sets the listening port for the torrents started from now on; 0 lets each take the
    /// one mtorrent derives.
    pub fn set_pwp_port(&self, port: u16) {
        self.pwp_port.store(port, Ordering::Relaxed);
    }

    /// The data directory, where copied .torrent files are kept.
    pub fn config_dir(&self) -> &PathBuf {
        &self.config_dir
    }
}

/// The port a torrent about to run listens on: the first from `base` up that no running
/// torrent has and nothing else holds. When `base` is 0 the search starts from the port
/// mtorrent would derive from `uri`, so that a torrent keeps its port from run to run while
/// it can. 0, for mtorrent to derive one after all, when every port is taken.
///
/// Each torrent needs a port of its own: mtorrent's listeners share a TCP port, and the
/// system would hand a connection for one torrent to any of them. Nor may the port be held
/// by anything else: mtorrent binds uTP on the same port over UDP, and runs the torrent
/// without it when that fails. The DHT node holds such a port, and so does, for a moment,
/// the task a paused or restarted torrent is leaving behind.
fn listening_port(
    base: u16,
    uri: &str,
    taken: impl IntoIterator<Item = u16>,
    is_free: impl Fn(u16) -> bool,
) -> u16 {
    use mtorrent::utils::re_exports::mtorrent_utils::net::port_from_hash;

    let taken: std::collections::HashSet<u16> = taken.into_iter().collect();
    let usable = |port: &u16| *port != 0 && !taken.contains(port) && is_free(*port);
    if base != 0 {
        return (base..=u16::MAX).find(usable).unwrap_or(0);
    }
    let derived = port_from_hash(&uri);
    (derived..=u16::MAX)
        .chain(DYNAMIC_PORTS_START..derived)
        .find(usable)
        .unwrap_or(0)
}

/// Where the ports mtorrent derives from a torrent begin.
const DYNAMIC_PORTS_START: u16 = 49152;

/// Whether a torrent could listen on `port`, over TCP and over UDP.
fn port_is_free(port: u16) -> bool {
    use std::net::{Ipv4Addr, TcpListener, UdpSocket};

    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
        && TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

/// Runs one torrent on the engine thread until it ends or is cancelled, and tells the
/// window how it went.
async fn run_torrent(cmd: StartCmd, shared: Shared) {
    let StartCmd {
        info_hash,
        name,
        uri,
        output_dir,
        canceller,
        cancel_rx,
        stop_flag,
        ui_tx,
        sequential,
        pwp_port,
    } = cmd;
    // mtorrent derives the metainfo filename and the download subfolder from the
    // magnet's `dn` value, then writes the fetched metainfo with a bare fs::write (no
    // parent mkdir). A `dn` containing a path separator points at a non-existent
    // subdir, so the write fails with ENOENT ("No such file or directory") right
    // after metadata is fetched. Sanitise `dn` so the derived path stays inside the
    // output dir.
    let uri = sanitize_magnet_dn(&uri);

    // Ensure the download dir exists; mtorrent's magnet preliminary stage writes the
    // fetched metainfo into output_dir before content storage is created, which fails
    // with ENOENT if the dir is missing.
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        log::warn!("Failed to create output dir {:?}: {}", output_dir, e);
    }

    // Files removed since the last run would otherwise count as downloaded: mtorrent takes
    // the pieces its progress file lists without reading them again. No run of this
    // torrent is left to write that file.
    let (check_uri, check_dir) = (uri.clone(), output_dir.clone());
    let _ = tokio::task::spawn_blocking(move || {
        let layout = crate::torrent_paths::content_layout(&check_uri, &check_dir);
        if let Some(layout) = layout {
            crate::torrent_paths::find_missing_content(&check_uri, &check_dir, Some(&layout), true);
        }
    })
    .await;

    let downloaded_bytes = Arc::new(AtomicU64::new(0));
    let total_bytes = Arc::new(AtomicU64::new(0));

    let listener = GtkListener::new(
        Arc::downgrade(&canceller),
        Arc::clone(&stop_flag),
        ui_tx.clone(),
        info_hash.clone(),
        name.clone(),
        uri.clone(),
        output_dir.clone(),
        Arc::clone(&downloaded_bytes),
        Arc::clone(&total_bytes),
        Arc::clone(&sequential),
    );
    let is_seq = sequential.load(Ordering::Relaxed);
    let config = app::main::Config {
        local_peer_id: shared.peer_id,
        output_dir: output_dir.clone(),
        config_dir: shared.config_dir,
        use_upnp: false,
        // 0 only when no port was free: mtorrent derives one then, rather than binding an
        // ephemeral one and announcing port 0 to trackers.
        pwp_port: (pwp_port != 0).then_some(pwp_port),
        bind_interface: None,
        // Fixed for this run: the engine restarts the torrent when the switch moves.
        download_strategy: if is_seq {
            app::main::DownloadStrategy::Sequential
        } else {
            app::main::DownloadStrategy::RarestFirst
        },
    };
    let ctx = app::main::Context {
        dht_handle: Some(shared.dht),
        pwp_runtime: shared.pwp_runtime,
        storage_runtime: shared.storage_runtime,
    };

    let mut rx = cancel_rx;
    let mut stop_reason = Stop::Pause;
    let result = tokio::select! {
        res = app::main::single_torrent(&uri, listener, config, ctx) => Some(res),
        reason = &mut rx => {
            // A dropped sender means the entry went away without a reason, which ends
            // the torrent as a pause does.
            stop_reason = reason.unwrap_or(Stop::Pause);
            log::info!("Torrent task ended ({:?}): {}", stop_reason, info_hash);
            None
        }
    };

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
    } else if matches!(stop_reason, Stop::Pause) {
        let update = UiUpdate {
            downloaded: downloaded_bytes.load(Ordering::Relaxed),
            total: total_bytes.load(Ordering::Relaxed),
            // No name: the one the window has may be newer than the one this run began
            // with.
            ..UiUpdate::idle(
                info_hash,
                String::new(),
                TorrentUiState::Paused,
                output_dir,
                uri,
                is_seq,
            )
        };
        let _ = ui_tx.send(UiEvent::Update(update)).await;
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
    let Some((value_start, value_end)) = dn_value(uri) else {
        return uri.to_string();
    };

    let raw = &uri[value_start..value_end];
    let decoded = urlencoding::decode(raw)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| raw.to_string());

    let cleaned = clean_name(&decoded);
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

/// Where the value of a magnet link's first `dn` parameter starts and ends.
fn dn_value(uri: &str) -> Option<(usize, usize)> {
    let mut start = uri.strip_prefix("magnet:")?.find('?')? + "magnet:?".len();
    loop {
        let end = uri[start..].find('&').map_or(uri.len(), |i| start + i);
        if uri[start..end].starts_with("dn=") {
            return Some((start + "dn=".len(), end));
        }
        if end == uri.len() {
            return None;
        }
        start = end + 1;
    }
}

/// A torrent name as a single file name: separators and control characters become `_`,
/// and surrounding spaces and dots go. Empty when nothing is left.
fn clean_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

/// Names a magnet link after its info hash when it has no usable name (`dn`) of its own.
/// mtorrent names both the fetched metainfo and the download folder after `dn`, with
/// "unnamed" for a link without one, so two such links saved to one folder would share
/// them.
pub(crate) fn name_nameless_magnet(uri: &str) -> String {
    use mtorrent::utils::re_exports::mtorrent_base::input::MagnetLink;
    use std::str::FromStr;

    let Ok(magnet) = MagnetLink::from_str(uri) else {
        return uri.to_string();
    };
    if magnet
        .name()
        .is_some_and(|name| !clean_name(name).is_empty())
    {
        return uri.to_string();
    }
    let Some((head, query)) = uri.split_once('?') else {
        return uri.to_string();
    };
    let params: Vec<&str> = query
        .split('&')
        .filter(|param| !param.is_empty() && !param.starts_with("dn="))
        .collect();
    format!("{head}?{}&dn={}", params.join("&"), hex(magnet.info_hash()))
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

/// The identity of a torrent whose URI names neither a magnet link nor a readable
/// `.torrent` file: a hash of the URI text.
pub(crate) fn hash_uri(uri: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(uri.as_bytes());
    hex(&hasher.finalize().into())
}

/// The BitTorrent info hash (hex) of the magnet link or `.torrent` file `uri` names, or
/// `None` when it names neither. Reads the file: keep it off the main thread.
pub(crate) fn info_hash(uri: &str) -> Option<String> {
    use mtorrent::utils::re_exports::mtorrent_base::input::{MagnetLink, Metainfo};
    use std::str::FromStr;

    let path = std::path::Path::new(uri);
    if path.is_file() {
        Metainfo::from_file(path)
            .ok()
            .map(|meta| hex(meta.info_hash()))
    } else {
        MagnetLink::from_str(uri)
            .ok()
            .map(|magnet| hex(magnet.info_hash()))
    }
}

/// An info hash in lowercase hex.
pub(crate) fn hex(hash: &[u8; 20]) -> String {
    use std::fmt::Write;

    hash.iter().fold(String::with_capacity(40), |mut s, b| {
        let _ = write!(s, "{:02x}", b);
        s
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        TorrentUiState, hash_uri, info_hash, listening_port, lock_recover, name_nameless_magnet,
        sanitize_magnet_dn,
    };
    use crate::test_support::{Harness, TestTorrent, closed_addr};

    const WAIT: Duration = Duration::from_secs(20);

    /// Starts `uri` as the window does, keyed by its info hash, and returns that.
    fn start(h: &Harness, name: &str, uri: String, sequential: bool) -> String {
        let hash = info_hash(&uri).expect("a torrent");
        h.engine.start(
            hash.clone(),
            name.into(),
            uri,
            h.output_dir(),
            sequential,
            h.tx.clone(),
        );
        hash
    }

    /// A magnet link whose only peer does not exist: the torrent runs and gets nowhere.
    fn magnet_to_nowhere(n: u8) -> String {
        let hash: String = std::iter::repeat_n(format!("{n:02x}"), 20).collect();
        format!("magnet:?xt=urn:btih:{hash}&x.pe={}", closed_addr())
    }

    #[test]
    fn a_started_torrent_runs_until_paused_and_again_once_resumed() {
        let h = Harness::new("engine-toggle", 0);
        let hash = start(&h, "Name", magnet_to_nowhere(1), false);
        let first = h.wait_for_update(&hash, WAIT, |_| true);
        assert_eq!(first.state, TorrentUiState::Downloading);
        assert_eq!(first.name, "Name");
        assert!(h.engine.is_active(&hash));

        h.engine.toggle(&hash);
        assert!(!h.engine.is_active(&hash));
        h.wait_for_update(&hash, WAIT, |u| u.state == TorrentUiState::Paused);

        h.engine.toggle(&hash);
        assert!(h.engine.is_active(&hash));
        h.wait_for_update(&hash, WAIT, |u| u.state == TorrentUiState::Downloading);

        h.engine.stop(&hash);
        assert!(!h.engine.is_active(&hash));
        // A stopped torrent is gone: there is nothing to resume.
        h.engine.toggle(&hash);
        assert!(!h.engine.is_active(&hash));
    }

    #[test]
    fn starting_a_running_torrent_again_starts_nothing_new() {
        let h = Harness::new("engine-restart", 0);
        let uri = magnet_to_nowhere(2);
        let hash = start(&h, "", uri.clone(), false);
        let again = start(&h, "", uri, true);
        assert_eq!(hash, again);
        // The second start only reports the new sequential setting.
        h.wait_for_update(&hash, WAIT, |u| u.sequential);

        h.engine.pause_all();
        assert!(!h.engine.is_active(&hash));
        h.engine.toggle(&hash);
        assert!(h.engine.is_active(&hash));
    }

    #[test]
    fn toggling_sequential_restarts_a_running_torrent_without_pausing_it() {
        let h = Harness::new("engine-sequential", 0);
        let hash = start(&h, "Name", magnet_to_nowhere(9), false);
        let first = h.wait_for_update(&hash, WAIT, |_| true);
        assert!(!first.sequential);

        // The task that runs the torrent, told apart by the flag that cancels it.
        let run = |hash: &str| {
            let map = lock_recover(&h.engine.active, "active map");
            Arc::as_ptr(&map[hash].stop_flag)
        };
        let before = run(&hash);

        h.engine.set_sequential(&hash, true);
        // The torrent keeps running, but in a new task: mtorrent reads the strategy
        // once, when the download starts.
        assert!(h.engine.is_active(&hash));
        assert_ne!(run(&hash), before);
        // Two snapshots, so the replaced task has had a turn to report as well: none of
        // them says the torrent paused.
        let mut seen = 0;
        let update = h.wait_for_update(&hash, WAIT, |u| {
            assert_eq!(
                u.state,
                TorrentUiState::Downloading,
                "the restart reported the torrent as no longer downloading"
            );
            seen += u.sequential as usize;
            seen == 2
        });
        assert!(update.sequential);

        // Setting what is already set leaves the run alone.
        let after = run(&hash);
        h.engine.set_sequential(&hash, true);
        assert_eq!(run(&hash), after);
        assert!(h.engine.is_active(&hash));
    }

    #[test]
    fn a_torrent_added_paused_starts_when_resumed() {
        let h = Harness::new("engine-paused", 0);
        let uri = magnet_to_nowhere(3);
        let hash = info_hash(&uri).unwrap();
        h.engine.add_paused(
            hash.clone(),
            "Paused".into(),
            uri,
            h.output_dir(),
            false,
            h.tx.clone(),
        );
        let update = h.wait_for_update(&hash, WAIT, |_| true);
        assert_eq!(update.state, TorrentUiState::Paused);
        assert!(!h.engine.is_active(&hash));

        h.engine.toggle(&hash);
        assert!(h.engine.is_active(&hash));
    }

    #[test]
    fn starting_a_paused_torrent_resumes_it_where_it_was() {
        let h = Harness::new("engine-start-paused", 0);
        let uri = magnet_to_nowhere(10);
        let hash = info_hash(&uri).unwrap();
        h.engine.add_paused(
            hash.clone(),
            "Paused".into(),
            uri.clone(),
            h.output_dir(),
            false,
            h.tx.clone(),
        );
        let elsewhere = h.output_dir().join("elsewhere");
        h.engine.start(
            hash.clone(),
            "Paused".into(),
            uri,
            elsewhere,
            false,
            h.tx.clone(),
        );

        assert!(h.engine.is_active(&hash));
        assert!(!lock_recover(&h.engine.saved, "saved map").contains_key(&hash));
        let active = lock_recover(&h.engine.active, "active map");
        assert_eq!(active[&hash].output_dir, h.output_dir());
    }

    #[test]
    fn a_failed_torrent_can_be_retried() {
        let h = Harness::new("engine-failed", 0);
        let hash = start(&h, "", magnet_to_nowhere(4), false);
        h.engine.mark_failed(&hash);
        assert!(!h.engine.is_active(&hash));
        h.engine.toggle(&hash);
        assert!(h.engine.is_active(&hash));
    }

    #[test]
    fn running_torrents_get_consecutive_ports_and_a_new_one_the_first_free() {
        let h = Harness::new("engine-ports", 47_000);
        let start = |n| start(&h, "", magnet_to_nowhere(n), false);
        let port = |hash: &str| lock_recover(&h.engine.active, "active map")[hash].port;

        let hashes: Vec<String> = [5, 6, 7].into_iter().map(start).collect();
        let ports: Vec<u16> = hashes.iter().map(|hash| port(hash)).collect();
        assert_eq!(ports, [47_000, 47_001, 47_002]);

        // Paused, the first torrent gives its port up, though its task may hold it a moment
        // longer; resumed, it takes one that neither the others nor that task have.
        h.engine.toggle(&hashes[0]);
        let fourth = start(8);
        let resumed = {
            h.engine.toggle(&hashes[0]);
            port(&hashes[0])
        };
        let mut all = vec![ports[1], ports[2], port(&fourth), resumed];
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 4, "ports shared: {all:?}");
    }

    #[test]
    fn a_torrent_file_shows_its_name_and_size_at_once() {
        let h = Harness::new("engine-file", 0);
        let torrent = TestTorrent::create(h.dir.path(), "From A File", 100_000, 16 * 1024);
        let uri = torrent.metainfo_path.to_string_lossy().into_owned();
        let hash = start(&h, "stem", uri, false);
        assert_eq!(hash, torrent.hex_hash());
        let update = h.wait_for_update(&hash, WAIT, |u| u.total > 0);
        assert_eq!(update.name, "From A File");
        assert_eq!(update.total, 100_000);
        assert_eq!(update.total_pieces, 7);
    }

    #[test]
    fn running_torrents_listen_on_ports_of_their_own() {
        let free = |_| true;
        assert_eq!(listening_port(6881, "", [], free), 6881);
        assert_eq!(listening_port(6881, "", [6881, 6883], free), 6882);
        assert_eq!(listening_port(u16::MAX, "", [u16::MAX], free), 0);
        // Held elsewhere, by the DHT node for one.
        assert_eq!(listening_port(6881, "", [6882], |port| port != 6881), 6883);
    }

    #[test]
    fn an_automatic_port_is_the_derived_one_while_that_is_free() {
        use mtorrent::utils::re_exports::mtorrent_utils::net::port_from_hash;

        let uri = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
        let derived = port_from_hash(&uri);
        assert_eq!(listening_port(0, uri, [], |_| true), derived);
        let next = if derived == u16::MAX {
            49152
        } else {
            derived + 1
        };
        assert_eq!(listening_port(0, uri, [derived], |_| true), next);
        assert_eq!(listening_port(0, uri, [], |port| port != derived), next);
        assert_eq!(listening_port(0, uri, [], |_| false), 0);
    }

    #[test]
    fn info_hash_uses_magnet_info_hash() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let with_dn = format!("magnet:?xt=urn:btih:{hex}&dn=Some%20Name");
        let without_dn = format!("magnet:?xt=urn:btih:{hex}");
        // Same content, different URI text: identical identity.
        assert_eq!(info_hash(&with_dn).as_deref(), Some(hex));
        assert_eq!(info_hash(&with_dn), info_hash(&without_dn));
    }

    #[test]
    fn unparseable_uris_have_no_info_hash_but_a_hash_of_their_text() {
        assert_eq!(info_hash("not a magnet at all"), None);
        let a = hash_uri("not a magnet at all");
        let b = hash_uri("not a magnet at all");
        let c = hash_uri("something else");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 40);
    }

    #[test]
    fn only_the_name_parameter_of_a_magnet_link_is_cleaned() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            sanitize_magnet_dn(&format!("magnet:?xt=urn:btih:{hex}&dn=Show%20%2F%20S1")),
            format!("magnet:?xt=urn:btih:{hex}&dn=Show%20_%20S1")
        );
        assert_eq!(
            sanitize_magnet_dn(&format!("magnet:?dn=a%2Fb&xt=urn:btih:{hex}")),
            format!("magnet:?dn=a_b&xt=urn:btih:{hex}")
        );
        // A parameter whose name only ends in dn is not the name, nor is a path.
        let other = format!("magnet:?xt=urn:btih:{hex}&xdn=a%2Fb&dn=Show");
        assert_eq!(sanitize_magnet_dn(&other), other);
        assert_eq!(
            sanitize_magnet_dn("/tmp/dn=a/b.torrent"),
            "/tmp/dn=a/b.torrent"
        );
    }

    #[test]
    fn nameless_magnet_links_are_named_after_their_info_hash() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let named = format!("magnet:?xt=urn:btih:{hex}&dn=Some%20Name");
        assert_eq!(name_nameless_magnet(&named), named);
        assert_eq!(
            name_nameless_magnet(&format!("magnet:?xt=urn:btih:{hex}&tr=udp%3A%2F%2Fx%3A1")),
            format!("magnet:?xt=urn:btih:{hex}&tr=udp%3A%2F%2Fx%3A1&dn={hex}")
        );
        // A name that cleans up to nothing is no name.
        assert_eq!(
            name_nameless_magnet(&format!("magnet:?dn=..&xt=urn:btih:{hex}")),
            format!("magnet:?xt=urn:btih:{hex}&dn={hex}")
        );
        assert_eq!(name_nameless_magnet("not a magnet"), "not a magnet");
    }
}
