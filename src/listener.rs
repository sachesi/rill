use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_channel::Sender;
use mtorrent::utils::listener::{StateListener, StateSnapshot};

use crate::engine::{TorrentUiState, UiEvent, UiUpdate};

pub struct GtkListener {
    canceller: Weak<()>,
    tx: Sender<UiEvent>,
    info_hash: String,
    name: String,
    uri: String,
    output_dir: PathBuf,
    last_downloaded: u64,
    last_time: Option<std::time::Instant>,
    downloaded_bytes: Arc<Mutex<u64>>,
    total_bytes: Arc<Mutex<u64>>,
    total_pieces: usize,
    downloaded_pieces: usize,
}

impl GtkListener {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        canceller: Weak<()>,
        tx: Sender<UiEvent>,
        info_hash: String,
        name: String,
        uri: String,
        output_dir: PathBuf,
        downloaded_bytes: Arc<Mutex<u64>>,
        total_bytes: Arc<Mutex<u64>>,
    ) -> Self {
        Self {
            canceller,
            tx,
            info_hash,
            name,
            uri,
            output_dir,
            last_downloaded: 0,
            last_time: None,
            downloaded_bytes,
            total_bytes,
            total_pieces: 0,
            downloaded_pieces: 0,
        }
    }
}

impl StateListener for GtkListener {
    const INTERVAL: Duration = Duration::from_secs(1);

    fn on_snapshot(&mut self, snapshot: StateSnapshot<'_>) -> ControlFlow<()> {
        if self.canceller.strong_count() < 2 {
            log::debug!("Listener cancelled for: {}", self.info_hash);
            let _ = self.tx.try_send(UiEvent::Update(UiUpdate {
                info_hash: self.info_hash.clone(),
                name: self.name.clone(),
                state: TorrentUiState::Paused,
                downloaded: self.last_downloaded,
                total: snapshot.bytes.total as u64,
                peers: 0,
                speed_down: 0,
                speed_up: 0,
                output_dir: self.output_dir.clone(),
                uri: self.uri.clone(),
                peers_list: Vec::new(),
                total_pieces: self.total_pieces,
                downloaded_pieces: self.downloaded_pieces,
            }));
            return ControlFlow::Break(());
        }

        let downloaded = snapshot.bytes.downloaded as u64;
        let total = snapshot.bytes.total as u64;
        let peers = snapshot.peers.len();

        if let Ok(mut dl) = self.downloaded_bytes.lock() {
            *dl = downloaded;
        }
        if let Ok(mut tot) = self.total_bytes.lock() {
            *tot = total;
        }

        let now = std::time::Instant::now();
        let speed_down = if let Some(last) = self.last_time {
            let elapsed = now.duration_since(last).as_secs_f64();
            if elapsed > 0.0 && downloaded >= self.last_downloaded {
                ((downloaded - self.last_downloaded) as f64 / elapsed) as u64
            } else {
                0
            }
        } else {
            0
        };

        self.last_downloaded = downloaded;
        self.last_time = Some(now);

        let state = if downloaded >= total && total > 0 {
            log::trace!("Torrent completed: {} ({}/{})", self.info_hash, downloaded, total);
            TorrentUiState::Completed
        } else {
            TorrentUiState::Downloading
        };

        let mut peers_list = Vec::new();
        for (addr, p_state) in &snapshot.peers {
            let client = p_state.extensions.as_ref()
                .and_then(|ext| ext.client_type.as_deref())
                .unwrap_or("n/a")
                .to_string();
            peers_list.push(crate::engine::PeerInfo {
                address: addr.to_string(),
                client,
                speed_down: p_state.download.last_bitrate_bps as u64,
                speed_up: p_state.upload.last_bitrate_bps as u64,
                encrypted: p_state.encryption,
            });
        }

        let total_pieces = snapshot.pieces.total;
        let downloaded_pieces = snapshot.pieces.downloaded;
        self.total_pieces = total_pieces;
        self.downloaded_pieces = downloaded_pieces;

        let _ = self.tx.try_send(UiEvent::Update(UiUpdate {
            info_hash: self.info_hash.clone(),
            name: self.name.clone(),
            state,
            downloaded,
            total,
            peers,
            speed_down,
            speed_up: 0,
            output_dir: self.output_dir.clone(),
            uri: self.uri.clone(),
            peers_list,
            total_pieces,
            downloaded_pieces,
        }));
        ControlFlow::Continue(())
    }
}
