use std::path::PathBuf;
use std::sync::Arc;

use async_channel::Sender;
use mtorrent::app;
use mtorrent::utils::re_exports::mtorrent_dht as dht;
use mtorrent::utils::re_exports::mtorrent_utils::peer_id::PeerId;

use crate::listener::GtkListener;

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
}

#[derive(Clone, Debug, PartialEq)]
pub enum TorrentUiState {
    Downloading,
    Paused,
    Completed,
    Error,
}

pub enum UiEvent {
    Update(UiUpdate),
    Finished {
        info_hash: String,
        error: Option<String>,
    },
}

pub struct TorrentEngine {
    tx: Sender<UiEvent>,
    start_tx: tokio::sync::mpsc::UnboundedSender<(String, PathBuf)>,
    _thread: std::thread::JoinHandle<()>,
}

impl TorrentEngine {
    pub fn new(
        peer_id: PeerId,
        config_dir: PathBuf,
        pwp_handle: tokio::runtime::Handle,
        storage_handle: tokio::runtime::Handle,
        dht_sink: dht::CommandSink,
    ) -> Self {
        let (tx, _rx) = async_channel::unbounded();
        let (start_tx, mut start_rx) = tokio::sync::mpsc::unbounded_channel::<(String, PathBuf)>();

        let tx_clone = tx.clone();
        let thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async {
                let local = tokio::task::LocalSet::new();
                local
                    .run_until(async {
                        while let Some((uri, output_dir)) = start_rx.recv().await {
                            let tx = tx_clone.clone();
                            let pid = peer_id.clone();
                            let cd = config_dir.clone();
                            let pwp = pwp_handle.clone();
                            let stor = storage_handle.clone();
                            let dht = dht_sink.clone();
                            let info_hash = hash_uri(&uri);

                            tokio::task::spawn_local(async move {
                                let canceller = Arc::new(());
                                let listener = GtkListener::new(
                                    Arc::downgrade(&canceller),
                                    tx.clone(),
                                    info_hash.clone(),
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
                                let _ = tx
                                    .send(UiEvent::Finished {
                                        info_hash,
                                        error: result.err().map(|e| e.to_string()),
                                    })
                                    .await;
                            });
                        }
                    })
                    .await;
            });
        });

        Self {
            tx,
            start_tx,
            _thread: thread,
        }
    }

    pub fn set_sender(&mut self, tx: Sender<UiEvent>) {
        self.tx = tx;
    }

    pub fn start(&self, uri: String, output_dir: PathBuf) {
        let _ = self.start_tx.send((uri, output_dir));
    }
}

fn hash_uri(uri: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    uri.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
