//! Storage layer for torrent sessions and application settings

mod db;
pub mod models;

pub use db::Database;
pub use models::{AppSettings, SavedTorrent};

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

/// Thread-safe storage handle
#[derive(Clone, Debug)]
pub struct Storage {
    db: Arc<Mutex<Database>>,
}

impl Storage {
    /// Open or create storage at specified path
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let db = Database::open(path).map_err(|e| format!("Database error: {}", e))?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
        })
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
            .update_torrent_state(info_hash, state, downloaded, total, total_pieces, downloaded_pieces, now)
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
    pub fn update_torrent_sequential(&self, info_hash: &str, sequential: bool) -> Result<(), String> {
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
