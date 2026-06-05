//! Storage layer for torrent sessions and application settings

mod db;
pub mod models;

pub use db::Database;
pub use models::{AppSettings, SavedTorrent};

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};

/// A unit of work for the storage worker thread.
enum Job {
    /// Run a closure against the storage handle on the worker thread.
    Run(Box<dyn FnOnce(&Storage) + Send>),
    /// Drain barrier: the worker acks once it reaches this point, so all jobs
    /// queued before it have completed. Used to flush pending writes on exit.
    Flush(mpsc::SyncSender<()>),
}

/// Thread-safe storage handle.
///
/// All public methods remain synchronous (they lock the database mutex and run
/// directly). On top of that, every handle carries a sender to a dedicated
/// worker thread so callers on the GTK main thread can offload database I/O via
/// [`Storage::execute`] (fire-and-forget writes) and [`Storage::query`] (async
/// reads) instead of blocking the UI.
#[derive(Clone, Debug)]
pub struct Storage {
    db: Arc<Mutex<Database>>,
    worker: mpsc::Sender<Job>,
}

impl Storage {
    /// Open or create storage at specified path
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let db = Arc::new(Mutex::new(
            Database::open(path).map_err(|e| format!("Database error: {}", e))?,
        ));
        let (tx, rx) = mpsc::channel::<Job>();
        // The worker owns its own handle (sharing the same db Arc + sender) so it
        // can run the high-level methods that closures call. The thread lives for
        // the whole process; jobs are drained in FIFO order, serializing SQLite.
        let worker_storage = Storage {
            db: db.clone(),
            worker: tx.clone(),
        };
        std::thread::Builder::new()
            .name("storage-worker".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Run(f) => {
                            if let Err(e) =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    f(&worker_storage)
                                }))
                            {
                                log::error!("Storage worker job panicked: {:?}", e);
                            }
                        }
                        Job::Flush(ack) => {
                            let _ = ack.send(());
                        }
                    }
                }
            })
            .map_err(|e| format!("Failed to spawn storage worker: {}", e))?;
        Ok(Self { db, worker: tx })
    }

    /// Queue a fire-and-forget database operation on the worker thread. Returns
    /// immediately; the closure runs off the calling (GTK) thread. Use for writes
    /// whose result the UI does not need to wait on.
    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce(&Storage) + Send + 'static,
    {
        if self.worker.send(Job::Run(Box::new(f))).is_err() {
            log::error!("Storage worker offline; dropped a write");
        }
    }

    /// Run a read on the worker thread and await its result. Awaiting the
    /// returned future on the GTK main context never blocks the UI: the oneshot
    /// receiver is a plain waker future, woken when the worker sends the result.
    pub async fn query<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&Storage) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (otx, orx) = tokio::sync::oneshot::channel();
        self.execute(move |s| {
            let _ = otx.send(f(s));
        });
        orx.await
            .map_err(|_| "Storage worker dropped query".to_string())
    }

    /// Block until the worker has drained every job queued before this call.
    /// Intended for shutdown, where briefly blocking the main thread is fine and
    /// guarantees in-flight writes hit disk before the process exits.
    pub fn flush_blocking(&self) {
        let (ack_tx, ack_rx) = mpsc::sync_channel(0);
        if self.worker.send(Job::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }

    /// Acquire the database guard, recovering from a poisoned mutex instead of
    /// panicking. A panic in one operation must not cascade into every later one.
    fn db(&self) -> MutexGuard<'_, Database> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Load all torrents from database
    pub fn load_torrents(&self) -> Result<Vec<SavedTorrent>, String> {
        log::info!("Loading all torrents");
        self.db()
            .load_torrents()
            .map_err(|e| format!("Failed to load torrents: {}", e))
    }

    /// Load a single torrent by info hash
    pub fn load_torrent(&self, info_hash: &str) -> Result<Option<SavedTorrent>, String> {
        self.db()
            .load_torrent(info_hash)
            .map_err(|e| format!("Failed to load torrent: {}", e))
    }

    /// Save or update a torrent
    pub fn save_torrent(&self, torrent: &SavedTorrent) -> Result<(), String> {
        self.db()
            .save_torrent(torrent)
            .map_err(|e| format!("Failed to save torrent: {}", e))
    }

    /// Update torrent state
    pub fn update_torrent_state(
        &self,
        info_hash: &str,
        state: &str,
        downloaded: u64,
        total: u64,
        total_pieces: u64,
        downloaded_pieces: u64,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        self.db()
            .update_torrent_state(
                info_hash,
                state,
                downloaded,
                total,
                total_pieces,
                downloaded_pieces,
                now,
            )
            .map_err(|e| format!("Failed to update torrent: {}", e))
    }

    /// Mark torrent as completed
    pub fn mark_completed(&self, info_hash: &str) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        self.db()
            .mark_completed(info_hash, now)
            .map_err(|e| format!("Failed to mark completed: {}", e))
    }

    /// Delete a torrent
    pub fn delete_torrent(&self, info_hash: &str) -> Result<(), String> {
        self.db()
            .delete_torrent(info_hash)
            .map_err(|e| format!("Failed to delete torrent: {}", e))
    }

    /// Update torrent sequential flag
    pub fn update_torrent_sequential(
        &self,
        info_hash: &str,
        sequential: bool,
    ) -> Result<(), String> {
        self.db()
            .update_torrent_sequential(info_hash, sequential)
            .map_err(|e| format!("Failed to update torrent sequential flag: {}", e))
    }

    /// Pause all downloading torrents
    pub fn pause_all_torrents(&self) -> Result<(), String> {
        self.db()
            .pause_all_torrents()
            .map_err(|e| format!("Failed to pause all torrents: {}", e))
    }

    /// Load app settings
    pub fn load_settings(&self) -> AppSettings {
        self.db().load_settings()
    }

    /// Load settings and all torrents under a single lock acquisition, so both
    /// reads observe the same database snapshot rather than two separate moments.
    pub fn load_settings_and_torrents(&self) -> (AppSettings, Vec<SavedTorrent>) {
        let db = self.db();
        let settings = db.load_settings();
        let torrents = db.load_torrents().unwrap_or_else(|e| {
            log::warn!("Failed to load torrents: {}", e);
            Vec::new()
        });
        (settings, torrents)
    }

    /// Read just the configured PWP port without loading every setting.
    pub fn pwp_port(&self) -> u16 {
        self.db().get_pwp_port()
    }

    /// Save app settings
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.db()
            .save_settings(settings)
            .map_err(|e| format!("Failed to save settings: {}", e))
    }
}
