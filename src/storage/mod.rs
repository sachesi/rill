//! Storage layer for torrent sessions and application settings

mod db;
pub mod models;

pub use db::Database;
pub use models::{AppSettings, SavedTorrent};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

    /// Load all torrents from database
    pub fn load_torrents(&self) -> Result<Vec<SavedTorrent>, String> {
        log::info!("Loading all torrents");
        self.db
            .lock()
            .unwrap()
            .load_torrents()
            .map_err(|e| format!("Failed to load torrents: {}", e))
    }

    /// Save or update a torrent
    pub fn save_torrent(&self, torrent: &SavedTorrent) -> Result<(), String> {
        self.db
            .lock()
            .unwrap()
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
        self.db
            .lock()
            .unwrap()
            .update_torrent_state(info_hash, state, downloaded, total, total_pieces, downloaded_pieces, now)
            .map_err(|e| format!("Failed to update torrent: {}", e))
    }

    /// Mark torrent as completed
    pub fn mark_completed(&self, info_hash: &str) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        self.db
            .lock()
            .unwrap()
            .mark_completed(info_hash, now)
            .map_err(|e| format!("Failed to mark completed: {}", e))
    }

    /// Delete a torrent
    pub fn delete_torrent(&self, info_hash: &str) -> Result<(), String> {
        self.db
            .lock()
            .unwrap()
            .delete_torrent(info_hash)
            .map_err(|e| format!("Failed to delete torrent: {}", e))
    }

    /// Load app settings
    pub fn load_settings(&self) -> AppSettings {
        self.db.lock().unwrap().load_settings()
    }

    /// Save app settings
    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        self.db
            .lock()
            .unwrap()
            .save_settings(settings)
            .map_err(|e| format!("Failed to save settings: {}", e))
    }
}
