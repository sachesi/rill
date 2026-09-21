use std::ops::ControlFlow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_channel::Sender;
use mtorrent::utils::listener::{StateListener, StateSnapshot};

use crate::engine::{Stop, TorrentUiState, UiEvent, UiUpdate};

/// Receives mtorrent's once-a-second snapshots of a torrent and forwards them to the
/// window as [`UiUpdate`]s.
pub struct GtkListener {
    canceller: Weak<()>,
    /// Set by the engine when this torrent is paused/stopped. Checked first so
    /// cancellation is observed atomically rather than racing on the canceller's
    /// strong count.
    stop_flag: Arc<std::sync::atomic::AtomicU8>,
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
    sequential: Arc<std::sync::atomic::AtomicBool>,
    info_hash_resolved: bool,
    name_resolved: bool,
    /// Directory holding the persisted `.mtorrent` piece-state file and the real
    /// 20-byte info hash keying it. Resolved once from the URI + output dir.
    state_target: Option<(PathBuf, [u8; 20])>,
    /// The last piece map, with the (total, downloaded) piece counts it was built for:
    /// while those stay the same the state file is not read again.
    last_piece_map: Option<(usize, usize, Vec<u8>)>,
}

impl GtkListener {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        canceller: Weak<()>,
        stop_flag: Arc<std::sync::atomic::AtomicU8>,
        tx: Sender<UiEvent>,
        info_hash: String,
        name: String,
        uri: String,
        output_dir: PathBuf,
        downloaded_bytes: Arc<Mutex<u64>>,
        total_bytes: Arc<Mutex<u64>>,
        sequential: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            canceller,
            stop_flag,
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
            sequential,
            info_hash_resolved: false,
            name_resolved: false,
            state_target: None,
            last_piece_map: None,
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
    use mtorrent::utils::re_exports::mtorrent_base::input::{MagnetLink, Metainfo};
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
fn build_piece_map(
    state_dir: &std::path::Path,
    info_hash: &[u8; 20],
    total_pieces: usize,
) -> Vec<u8> {
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
        if let Some(fill) = (have * 255).checked_div(tot) {
            *slot = fill as u8;
        }
    }
    out
}

/// The name the torrent's metadata gives it: from the .torrent file it was added from, or
/// for a magnet link from the one mtorrent saved once it fetched the metadata.
fn metainfo_name(uri: &str, output_dir: &std::path::Path) -> Option<String> {
    use mtorrent::utils::re_exports::mtorrent_base::input::Metainfo;

    let path = crate::torrent_paths::metainfo_path(uri, output_dir)?;
    let meta = Metainfo::from_file(path).ok()?;
    meta.name()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
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
            self.state_target = resolve_state_target(&self.uri, &self.output_dir);
            self.info_hash_resolved = true;
        }
        // Pieces are known once the metadata is: from then on the torrent has its own
        // name, which replaces a file stem or a magnet link's `dn`.
        if !self.name_resolved && total_pieces > 0 {
            if let Some(name) = metainfo_name(&self.uri, &self.output_dir) {
                self.name = name;
            }
            self.name_resolved = true;
        }

        // A torrent whose entry went away without a reason ends as a pause does.
        let stop = Stop::from_code(self.stop_flag.load(std::sync::atomic::Ordering::Acquire))
            .or_else(|| (self.canceller.strong_count() < 2).then_some(Stop::Pause));
        if let Some(stop) = stop {
            log::debug!("Listener cancelled for: {} ({:?})", self.info_hash, stop);
            if matches!(stop, Stop::Restart) {
                // The torrent goes on in a new task, which reports its state.
                return ControlFlow::Break(());
            }
            let _ = self.tx.try_send(UiEvent::Update(UiUpdate {
                downloaded: self.last_downloaded,
                total: snapshot.bytes.total as u64,
                total_pieces,
                downloaded_pieces,
                ..UiUpdate::idle(
                    self.info_hash.clone(),
                    self.name.clone(),
                    TorrentUiState::Paused,
                    self.output_dir.clone(),
                    self.uri.clone(),
                    is_sequential,
                )
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
            log::trace!(
                "Torrent completed: {} ({}/{})",
                self.info_hash,
                downloaded,
                total
            );
            TorrentUiState::Completed
        } else {
            TorrentUiState::Downloading
        };

        let mut peers_list = Vec::new();
        let mut speed_up = 0u64;
        for (addr, p_state) in &snapshot.peers {
            let client = p_state
                .extensions
                .as_ref()
                .and_then(|ext| ext.client_type.clone());
            speed_up += p_state.upload.last_bitrate_bps as u64;
            peers_list.push(crate::engine::PeerInfo {
                address: addr.to_string(),
                client,
                speed_down: p_state.download.last_bitrate_bps as u64,
                speed_up: p_state.upload.last_bitrate_bps as u64,
                encrypted: p_state.encryption,
            });
        }

        let piece_map = if let Some((dir, ih)) = self.state_target.clone() {
            let reuse = matches!(
                &self.last_piece_map,
                Some((t, d, _)) if *t == total_pieces && *d == downloaded_pieces
            );
            if reuse {
                self.last_piece_map.as_ref().unwrap().2.clone()
            } else {
                let m = build_piece_map(&dir, &ih, total_pieces);
                self.last_piece_map = Some((total_pieces, downloaded_pieces, m.clone()));
                m
            }
        } else {
            Vec::new()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_magnet_link_takes_the_name_of_its_fetched_metadata() {
        let dir = std::env::temp_dir().join(format!("rill-listener-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let uri = "magnet:?xt=urn:btih:0123456789012345678901234567890123456789&dn=link%20name";
        assert_eq!(metainfo_name(uri, &dir), None);

        let metainfo = b"d4:infod6:lengthi1e4:name9:real name12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";
        std::fs::write(dir.join("link name.torrent"), metainfo).unwrap();
        let name = metainfo_name(uri, &dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(name.as_deref(), Some("real name"));
    }

    /// A listener as a torrent task has it, with the stop flag the engine would set.
    fn listener(stop: Option<Stop>, tx: Sender<UiEvent>) -> (GtkListener, Arc<()>) {
        use std::sync::atomic::{AtomicBool, AtomicU8};

        let canceller = Arc::new(());
        let listener = GtkListener::new(
            Arc::downgrade(&canceller),
            Arc::new(AtomicU8::new(stop.map_or(Stop::RUNNING, Stop::code))),
            tx,
            "hash".into(),
            "Name".into(),
            "magnet:?xt=urn:btih:0123456789012345678901234567890123456789".into(),
            PathBuf::from("/tmp"),
            Arc::new(Mutex::new(0)),
            Arc::new(Mutex::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        (listener, canceller)
    }

    fn empty_snapshot() -> StateSnapshot<'static> {
        StateSnapshot {
            peers: Default::default(),
            pieces: Default::default(),
            bytes: Default::default(),
            requests: Default::default(),
            metainfo: Default::default(),
        }
    }

    #[test]
    fn a_paused_torrent_is_reported_but_a_restarted_one_is_left_to_its_new_task() {
        let (tx, events) = async_channel::unbounded();

        let (mut paused, _canceller) = listener(Some(Stop::Pause), tx.clone());
        assert!(paused.on_snapshot(empty_snapshot()).is_break());
        let UiEvent::Update(update) = events.try_recv().expect("the pause is reported") else {
            panic!("expected an update");
        };
        assert_eq!(update.state, TorrentUiState::Paused);

        let (mut restarted, _canceller) = listener(Some(Stop::Restart), tx);
        assert!(restarted.on_snapshot(empty_snapshot()).is_break());
        assert!(events.try_recv().is_err(), "the restart reported something");
    }
}
