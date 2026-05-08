use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::Weak;
use std::time::Duration;

use async_channel::Sender;
use mtorrent::utils::listener::{StateListener, StateSnapshot};

use crate::engine::{TorrentUiState, UiEvent, UiUpdate};

pub struct GtkListener {
    canceller: Weak<()>,
    tx: Sender<UiEvent>,
    info_hash: String,
    uri: String,
    output_dir: PathBuf,
    last_downloaded: u64,
    last_time: Option<std::time::Instant>,
}

impl GtkListener {
    pub fn new(
        canceller: Weak<()>,
        tx: Sender<UiEvent>,
        info_hash: String,
        uri: String,
        output_dir: PathBuf,
    ) -> Self {
        Self {
            canceller,
            tx,
            info_hash,
            uri,
            output_dir,
            last_downloaded: 0,
            last_time: None,
        }
    }
}

impl StateListener for GtkListener {
    const INTERVAL: Duration = Duration::from_secs(1);

    fn on_snapshot(&mut self, snapshot: StateSnapshot<'_>) -> ControlFlow<()> {
        if self.canceller.strong_count() == 0 {
            let _ = self.tx.try_send(UiEvent::Update(UiUpdate {
                info_hash: self.info_hash.clone(),
                name: String::new(),
                state: TorrentUiState::Paused,
                downloaded: self.last_downloaded,
                total: snapshot.bytes.total as u64,
                peers: 0,
                speed_down: 0,
                speed_up: 0,
                output_dir: self.output_dir.clone(),
                uri: self.uri.clone(),
            }));
            return ControlFlow::Break(());
        }

        let downloaded = snapshot.bytes.downloaded as u64;
        let total = snapshot.bytes.total as u64;
        let peers = snapshot.peers.len();

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
            TorrentUiState::Completed
        } else {
            TorrentUiState::Downloading
        };

        let _ = self.tx.try_send(UiEvent::Update(UiUpdate {
            info_hash: self.info_hash.clone(),
            name: String::new(),
            state,
            downloaded,
            total,
            peers,
            speed_down,
            speed_up: 0,
            output_dir: self.output_dir.clone(),
            uri: self.uri.clone(),
        }));
        ControlFlow::Continue(())
    }
}
