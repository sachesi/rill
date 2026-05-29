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
    #[allow(dead_code)]
    config_dir: PathBuf,
    last_downloaded: u64,
    last_time: Option<std::time::Instant>,
    downloaded_bytes: Arc<Mutex<u64>>,
    total_bytes: Arc<Mutex<u64>>,
    total_pieces: usize,
    downloaded_pieces: usize,
    sequential: Arc<std::sync::atomic::AtomicBool>,
    info_hash_resolved: bool,
    /// Directory holding the persisted `.mtorrent` piece-state file and the real
    /// 20-byte info hash keying it. Resolved once from the URI + output dir.
    state_target: Option<(PathBuf, [u8; 20])>,
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
        config_dir: PathBuf,
        downloaded_bytes: Arc<Mutex<u64>>,
        total_bytes: Arc<Mutex<u64>>,
        sequential: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            canceller,
            tx,
            info_hash,
            name,
            uri,
            output_dir,
            config_dir,
            last_downloaded: 0,
            last_time: None,
            downloaded_bytes,
            total_bytes,
            total_pieces: 0,
            downloaded_pieces: 0,
            sequential,
            info_hash_resolved: false,
            state_target: None,
        }
    }
}

/// Number of segments in the downsampled fragmentation map sent to the UI.
const PIECE_MAP_BUCKETS: usize = 200;

/// Resolves the directory of the persisted `.mtorrent` piece-state file and the
/// real info hash that keys it, mirroring how mtorrent derives the content dir
/// (`output_dir/<metainfo file stem>` for files, `output_dir/<magnet name>` for
/// magnets).
fn resolve_state_target(uri: &str, output_dir: &std::path::Path) -> Option<(PathBuf, [u8; 20])> {
    use mtorrent::utils::re_exports::mtorrent_core::input::{MagnetLink, Metainfo};
    use std::str::FromStr;

    let path = std::path::Path::new(uri);
    if path.is_file() {
        let meta = Metainfo::from_file(path).ok()?;
        let stem = path.file_stem()?;
        Some((output_dir.join(stem), *meta.info_hash()))
    } else if let Ok(magnet) = MagnetLink::from_str(uri) {
        let name = magnet.name().unwrap_or("unnamed");
        Some((output_dir.join(name), *magnet.info_hash()))
    } else {
        None
    }
}

/// Name of the bencoded progress file mtorrent rewrites in the content dir on
/// every snapshot interval (a dictionary of `{info_hash: bitfield}`).
const STATE_FILENAME: &str = ".mtorrent";

/// Reads the live piece bitfield mtorrent persists each interval and downsamples
/// it to a fixed-width fill map (0..=255 per segment, in piece order). The
/// bitfield is big-endian (piece 0 = most significant bit of the first byte).
fn build_piece_map(state_dir: &std::path::Path, info_hash: &[u8; 20], total_pieces: usize) -> Vec<u8> {
    use mtorrent::utils::re_exports::mtorrent_utils::benc::Element;

    if total_pieces == 0 {
        return Vec::new();
    }
    let Ok(buf) = std::fs::read(state_dir.join(STATE_FILENAME)) else {
        return Vec::new();
    };
    let Ok(Element::Dictionary(mut root)) = Element::from_bytes(&buf) else {
        return Vec::new();
    };
    let Some(Element::ByteString(bytes)) = root.remove(&Element::ByteString(info_hash.to_vec()))
    else {
        return Vec::new();
    };

    let has_piece = |i: usize| -> bool {
        let byte = i / 8;
        byte < bytes.len() && (bytes[byte] >> (7 - (i % 8))) & 1 == 1
    };

    let n = total_pieces;
    let buckets = PIECE_MAP_BUCKETS.min(n);
    let mut out = vec![0u8; buckets];
    for (b, slot) in out.iter_mut().enumerate() {
        let start = b * n / buckets;
        let end = ((b + 1) * n / buckets).max(start + 1).min(n);
        let have = (start..end).filter(|&i| has_piece(i)).count();
        let tot = end - start;
        if tot > 0 {
            *slot = (have * 255 / tot) as u8;
        }
    }
    out
}

/// Resolves the display name from the torrent metadata when the URI points at a
/// `.torrent` file. Magnet links return `None` (the caller already extracted the
/// `dn` parameter; the real name only arrives once metadata is downloaded).
fn resolve_real_name(uri: &str) -> Option<String> {
    use mtorrent::utils::re_exports::mtorrent_core::input::{MagnetLink, Metainfo};
    use std::str::FromStr;

    if MagnetLink::from_str(uri).is_ok() {
        return None;
    }
    if let Ok(meta) = Metainfo::from_file(std::path::Path::new(uri)) {
        return meta.name().map(|s| s.to_string());
    }
    None
}

impl StateListener for GtkListener {
    const INTERVAL: Duration = Duration::from_secs(1);

    fn on_snapshot(&mut self, snapshot: StateSnapshot<'_>) -> ControlFlow<()> {
        let is_sequential = self.sequential.load(std::sync::atomic::Ordering::Relaxed);
        let total_pieces = snapshot.pieces.total;
        let downloaded_pieces = snapshot.pieces.downloaded;
        self.total_pieces = total_pieces;
        self.downloaded_pieces = downloaded_pieces;

        if !self.info_hash_resolved {
            // Override the filename-stem name with the real name from the
            // .torrent metadata's info.name field when available.
            if let Some(name) = resolve_real_name(&self.uri).filter(|n| !n.is_empty()) {
                self.name = name;
            }
            self.state_target = resolve_state_target(&self.uri, &self.output_dir);
            self.info_hash_resolved = true;
        }

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
                total_pieces,
                downloaded_pieces,
                sequential: is_sequential,
                piece_map: Vec::new(),
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
        let mut speed_up = 0u64;
        for (addr, p_state) in &snapshot.peers {
            let client = p_state.extensions.as_ref()
                .and_then(|ext| ext.client_type.as_deref())
                .unwrap_or("n/a")
                .to_string();
            speed_up += p_state.upload.last_bitrate_bps as u64;
            peers_list.push(crate::engine::PeerInfo {
                address: addr.to_string(),
                client,
                speed_down: p_state.download.last_bitrate_bps as u64,
                speed_up: p_state.upload.last_bitrate_bps as u64,
                encrypted: p_state.encryption,
            });
        }

        let piece_map = match &self.state_target {
            Some((dir, ih)) => build_piece_map(dir, ih, total_pieces),
            None => Vec::new(),
        };

        let _ = self.tx.try_send(UiEvent::Update(UiUpdate {
            info_hash: self.info_hash.clone(),
            name: self.name.clone(),
            state,
            downloaded,
            total,
            peers,
            speed_down,
            speed_up,
            output_dir: self.output_dir.clone(),
            uri: self.uri.clone(),
            peers_list,
            total_pieces,
            downloaded_pieces,
            sequential: is_sequential,
            piece_map,
        }));
        ControlFlow::Continue(())
    }
}
