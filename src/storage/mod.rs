//! The SQLite database of torrents and settings, and the worker thread that runs it.

mod db;
pub mod models;

use db::Database;
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

    /// Run a job on the worker thread and await its result. The job is queued
    /// by this call, not when the future is first polled, so it keeps its place
    /// among the jobs queued before and after it. Awaiting the returned future on
    /// the GTK main context never blocks the UI: the oneshot receiver is a plain
    /// waker future, woken when the worker sends the result.
    pub fn query<F, R>(&self, f: F) -> impl Future<Output = Result<R, String>> + use<F, R>
    where
        F: FnOnce(&Storage) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (otx, orx) = tokio::sync::oneshot::channel();
        self.execute(move |s| {
            let _ = otx.send(f(s));
        });
        async move {
            orx.await
                .map_err(|_| "Storage worker dropped query".to_string())
        }
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
        let now = models::unix_time();
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
        let now = models::unix_time();
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

    /// Re-key a torrent record to its canonical info hash. Returns whether the
    /// row was re-keyed (false when the target key already exists).
    pub fn migrate_torrent_hash(&self, old_hash: &str, new_hash: &str) -> Result<bool, String> {
        self.db()
            .migrate_torrent_hash(old_hash, new_hash)
            .map_err(|e| format!("Failed to migrate torrent hash: {}", e))
    }

    /// Rename a torrent
    pub fn update_torrent_name(&self, info_hash: &str, name: &str) -> Result<(), String> {
        self.db()
            .update_torrent_name(info_hash, name)
            .map_err(|e| format!("Failed to rename torrent: {}", e))
    }

    /// Update where a torrent's content is kept
    pub fn update_torrent_output_dir(
        &self,
        info_hash: &str,
        output_dir: &str,
    ) -> Result<(), String> {
        self.db()
            .update_torrent_output_dir(info_hash, output_dir)
            .map_err(|e| format!("Failed to save the download folder: {}", e))
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

    /// Load app settings
    pub fn load_settings(&self) -> AppSettings {
        self.db().load_settings()
    }

    /// Save app settings
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.db()
            .save_settings(settings)
            .map_err(|e| format!("Failed to save settings: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(name: &str) -> (Storage, crate::test_support::ScratchDir) {
        let dir = crate::test_support::ScratchDir::new(name);
        (Storage::open(dir.path().join("torrents.db")).unwrap(), dir)
    }

    #[test]
    fn queued_jobs_run_in_order_and_a_flush_waits_for_them() {
        let (storage, _dir) = open("storage-order");
        let seen = Arc::new(Mutex::new(Vec::new()));
        for n in 0..20 {
            let seen = seen.clone();
            storage.execute(move |_| seen.lock().unwrap().push(n));
        }
        storage.flush_blocking();
        assert_eq!(*seen.lock().unwrap(), (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn a_read_sees_the_writes_queued_before_it() {
        let (storage, _dir) = open("storage-read");
        let torrent = SavedTorrent::new(
            "aa".into(),
            "name".into(),
            "uri".into(),
            "paused".into(),
            0,
            0,
            "/downloads".into(),
        );
        storage.execute(move |s| s.save_torrent(&torrent).unwrap());
        let loaded = block_on(storage.query(|s| s.load_torrent("aa")));
        assert!(loaded.unwrap().unwrap().is_some());
    }

    #[test]
    fn a_job_that_panics_does_not_stop_the_worker() {
        let (storage, _dir) = open("storage-panic");
        storage.execute(|_| panic!("a job gone wrong"));
        let answer = block_on(storage.query(|_| 42));
        assert_eq!(answer, Ok(42));
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }
}
