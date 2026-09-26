//! What the tests need around the engine: a scratch directory, a .torrent file made up on the
//! spot, and an engine with its runtimes and DHT node.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use mtorrent::utils::re_exports::mtorrent_utils::benc::Element;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::engine::{TorrentEngine, UiEvent, UiUpdate};

/// A directory of its own for one test, removed with it.
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    pub fn new(name: &str) -> Self {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rill-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A .torrent file in `dir` for a single file of `length` bytes named `name`.
pub struct TestTorrent {
    pub info_hash: [u8; 20],
    pub metainfo_path: PathBuf,
}

impl TestTorrent {
    pub fn create(dir: &Path, name: &str, length: usize, piece_length: usize) -> Self {
        use sha1::{Digest, Sha1};

        let pieces = length.div_ceil(piece_length);
        let info = Element::Dictionary(
            [
                (Element::from("length"), Element::Integer(length as i64)),
                (
                    Element::from("name"),
                    Element::ByteString(name.as_bytes().to_vec()),
                ),
                (
                    Element::from("piece length"),
                    Element::Integer(piece_length as i64),
                ),
                // The hashes are never checked: nothing is downloaded.
                (
                    Element::from("pieces"),
                    Element::ByteString(vec![0; pieces * 20]),
                ),
            ]
            .into(),
        );
        let info_hash = Sha1::digest(info.encode()).into();
        let metainfo = Element::Dictionary([(Element::from("info"), info)].into());
        let metainfo_path = dir.join(format!("{name}.torrent"));
        std::fs::write(&metainfo_path, metainfo.encode()).unwrap();
        Self {
            info_hash,
            metainfo_path,
        }
    }

    pub fn hex_hash(&self) -> String {
        self.info_hash.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// An engine as Rill runs it, with runtimes and a DHT node that knows no one, in a directory
/// of its own.
pub struct Harness {
    pub engine: TorrentEngine,
    pub tx: async_channel::Sender<UiEvent>,
    pub events: async_channel::Receiver<UiEvent>,
    _storage_runtime: tokio::runtime::Runtime,
    // Stops the DHT node, which saves its state, before the directory goes.
    _dht: mtorrent::utils::re_exports::mtorrent_utils::worker::rt::Handle,
    pub dir: ScratchDir,
}

impl Harness {
    /// `pwp_port` is the listening port setting: 0 lets mtorrent derive one per torrent.
    pub fn new(name: &str, pwp_port: u16) -> Self {
        let dir = ScratchDir::new(name);
        let pwp = crate::spawn_local_runtime("test-pwp-runtime").unwrap();
        let storage_runtime = crate::storage_runtime().unwrap();
        let (dht, dht_cmds) =
            mtorrent::app::dht::launch_dht_node_runtime(mtorrent::app::dht::Config {
                local_port: 0,
                max_concurrent_queries: Some(10),
                config_dir: dir.path().to_path_buf(),
                use_upnp: false,
                bootstrap_nodes_override: Some(Vec::new()),
                bind_interface: None,
                query_timeout: None,
            })
            .unwrap();
        let engine = TorrentEngine::new(
            PeerId::generate_new(),
            dir.path().to_path_buf(),
            pwp,
            storage_runtime.handle().clone(),
            dht_cmds,
            pwp_port,
        )
        .unwrap();
        let (tx, events) = async_channel::unbounded();
        Self {
            engine,
            tx,
            events,
            _storage_runtime: storage_runtime,
            _dht: dht,
            dir,
        }
    }

    pub fn output_dir(&self) -> PathBuf {
        self.dir.path().join("downloads")
    }

    /// The next event, or `None` after `timeout`.
    pub fn next_event(&self, timeout: Duration) -> Option<UiEvent> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.events.try_recv() {
                Ok(event) => return Some(event),
                Err(async_channel::TryRecvError::Closed) => return None,
                Err(async_channel::TryRecvError::Empty) => {}
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The first update of `hash` that `wanted` accepts; panics after `timeout`.
    pub fn wait_for_update(
        &self,
        hash: &str,
        timeout: Duration,
        mut wanted: impl FnMut(&UiUpdate) -> bool,
    ) -> UiUpdate {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.next_event(left) {
                Some(UiEvent::Update(update)) if update.info_hash == hash && wanted(&update) => {
                    return update;
                }
                Some(UiEvent::Finished { info_hash, error }) if info_hash == hash => {
                    panic!("{hash} finished ({error:?}) before the update waited for");
                }
                Some(_) => {}
                None => panic!("no such update of {hash} within {timeout:?}"),
            }
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.engine.pause_all();
        // The engine thread may still be setting up a torrent started last, folders
        // included.
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A local address nothing listens on.
pub fn closed_addr() -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}
