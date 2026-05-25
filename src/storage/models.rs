use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Serializable torrent record for database storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTorrent {
    pub info_hash: String,
    pub name: String,
    pub uri: String,
    pub state: String, // "downloading", "paused", "completed", "error"
    pub downloaded: u64,
    pub total: u64,
    pub output_dir: String,
    pub added_at: i64,
    pub completed_at: Option<i64>,
    pub last_active: i64,
    pub total_pieces: u64,
    pub downloaded_pieces: u64,
    pub sequential: bool,
}

impl SavedTorrent {
    pub fn new(
        info_hash: String,
        name: String,
        uri: String,
        state: String,
        downloaded: u64,
        total: u64,
        output_dir: PathBuf,
    ) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            info_hash,
            name,
            uri,
            state,
            downloaded,
            total,
            output_dir: output_dir.to_string_lossy().to_string(),
            added_at: now,
            completed_at: None,
            last_active: now,
            total_pieces: 0,
            downloaded_pieces: 0,
            sequential: false,
        }
    }

    pub fn output_dir_path(&self) -> PathBuf {
        PathBuf::from(&self.output_dir)
    }
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub download_folder: String,
    pub window_width: i32,
    pub window_height: i32,
    pub window_maximized: bool,
    pub log_level: String,
    pub log_torrent_ops: bool,
    pub max_active_downloads: i32,
    pub max_active_uploads: i32,
    pub global_download_limit: i32,
    pub global_upload_limit: i32,
    pub seeding_ratio_limit: f64,
    pub pwp_port: u16,
}

impl Default for AppSettings {
    fn default() -> Self {
        let download_folder = dirs_next::download_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .to_string_lossy()
            .to_string();

        Self {
            download_folder,
            window_width: 375,
            window_height: 480,
            window_maximized: false,
            log_level: "info".to_string(),
            log_torrent_ops: false,
            max_active_downloads: 3,
            max_active_uploads: 3,
            global_download_limit: 0,
            global_upload_limit: 0,
            seeding_ratio_limit: 1.0,
            pwp_port: 0,
        }
    }
}

impl AppSettings {
    pub fn download_folder_path(&self) -> PathBuf {
        PathBuf::from(&self.download_folder)
    }
}
