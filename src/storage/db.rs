use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;

use super::models::{AppSettings, SavedTorrent};

const SCHEMA_VERSION: i32 = 4;

#[derive(Debug)]
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create database at specified path
    pub fn open<P: AsRef<Path>>(path: P) -> SqlResult<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        log::info!("Opening database: {}", path_str);
        let conn = Connection::open(path.as_ref())?;

        // Enable WAL mode for better concurrency and performance
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "busy_timeout", "5000")?;

        let db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    /// Initialize schema if not exists
    fn initialize(&self) -> SqlResult<()> {
        // Create schema version table
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            )",
            [],
        )?;

        // Check current version
        let current_version: Option<i32> = self
            .conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .ok();

        if let Some(version) = current_version {
            log::debug!("Database schema version: {}", version);
            if version < SCHEMA_VERSION {
                self.migrate(version)?;
            }
        } else {
            log::info!("Creating fresh database schema");
            self.create_schema()?;
            self.conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [SCHEMA_VERSION],
            )?;
        }

        Ok(())
    }

    fn migrate(&self, from_version: i32) -> SqlResult<()> {
        log::info!(
            "Migrating database schema from version {} to {}",
            from_version,
            SCHEMA_VERSION
        );
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            if from_version < 2 {
                self.conn.execute("INSERT OR IGNORE INTO settings (key, value) VALUES ('max_active_downloads', '3')", [])?;
                self.conn.execute("INSERT OR IGNORE INTO settings (key, value) VALUES ('max_active_uploads', '3')", [])?;
                self.conn.execute("INSERT OR IGNORE INTO settings (key, value) VALUES ('global_download_limit', '0')", [])?;
                self.conn.execute("INSERT OR IGNORE INTO settings (key, value) VALUES ('global_upload_limit', '0')", [])?;
                self.conn.execute("INSERT OR IGNORE INTO settings (key, value) VALUES ('seeding_ratio_limit', '1.0')", [])?;
            }
            if from_version < 3 {
                self.conn.execute(
                    "ALTER TABLE torrents ADD COLUMN total_pieces INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
                self.conn.execute(
                    "ALTER TABLE torrents ADD COLUMN downloaded_pieces INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            if from_version < 4 {
                self.conn.execute(
                    "ALTER TABLE torrents ADD COLUMN sequential INTEGER NOT NULL DEFAULT 0",
                    [],
                )?;
            }
            self.conn
                .execute("UPDATE schema_version SET version = ?1", [SCHEMA_VERSION])?;
            Ok(())
        })();
        match result {
            Ok(()) => self.conn.execute_batch("COMMIT"),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    fn create_schema(&self) -> SqlResult<()> {
        // Torrents table
        self.conn.execute(
            "CREATE TABLE torrents (
                info_hash TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                uri TEXT NOT NULL,
                state TEXT NOT NULL,
                downloaded INTEGER NOT NULL,
                total INTEGER NOT NULL,
                output_dir TEXT NOT NULL,
                added_at INTEGER NOT NULL,
                completed_at INTEGER,
                last_active INTEGER NOT NULL,
                total_pieces INTEGER NOT NULL DEFAULT 0,
                downloaded_pieces INTEGER NOT NULL DEFAULT 0,
                sequential INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;

        self.conn
            .execute("CREATE INDEX idx_state ON torrents(state)", [])?;

        self.conn.execute(
            "CREATE INDEX idx_last_active ON torrents(last_active DESC)",
            [],
        )?;

        // Settings table
        self.conn.execute(
            "CREATE TABLE settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        Ok(())
    }

    // ==================== Torrent CRUD ====================

    /// Save or update a torrent
    pub fn save_torrent(&self, torrent: &SavedTorrent) -> SqlResult<()> {
        log::debug!("Saving torrent: {} ({})", torrent.name, torrent.info_hash);
        self.conn.execute(
            "INSERT OR REPLACE INTO torrents 
             (info_hash, name, uri, state, downloaded, total, output_dir, 
              added_at, completed_at, last_active, total_pieces, downloaded_pieces, sequential)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                torrent.info_hash,
                torrent.name,
                torrent.uri,
                torrent.state,
                torrent.downloaded as i64,
                torrent.total as i64,
                torrent.output_dir,
                torrent.added_at,
                torrent.completed_at,
                torrent.last_active,
                torrent.total_pieces as i64,
                torrent.downloaded_pieces as i64,
                torrent.sequential as i32,
            ],
        )?;
        Ok(())
    }

    /// Load all torrents
    pub fn load_torrents(&self) -> SqlResult<Vec<SavedTorrent>> {
        log::debug!("Loading all torrents from database");
        let mut stmt = self.conn.prepare(
            "SELECT info_hash, name, uri, state, downloaded, total, 
                    output_dir, added_at, completed_at, last_active,
                    total_pieces, downloaded_pieces, sequential
             FROM torrents
             ORDER BY last_active DESC",
        )?;

        let torrents = stmt
            .query_map([], |row| {
                Ok(SavedTorrent {
                    info_hash: row.get(0)?,
                    name: row.get(1)?,
                    uri: row.get(2)?,
                    state: row.get(3)?,
                    downloaded: row.get::<_, i64>(4)? as u64,
                    total: row.get::<_, i64>(5)? as u64,
                    output_dir: row.get(6)?,
                    added_at: row.get(7)?,
                    completed_at: row.get(8)?,
                    last_active: row.get(9)?,
                    total_pieces: row.get::<_, i64>(10)? as u64,
                    downloaded_pieces: row.get::<_, i64>(11)? as u64,
                    sequential: row.get::<_, i32>(12)? != 0,
                })
            })?
            .collect::<SqlResult<Vec<_>>>()?;

        log::info!("Loaded {} torrent(s) from database", torrents.len());
        Ok(torrents)
    }

    /// Load a single torrent by info hash, or `None` if absent.
    pub fn load_torrent(&self, info_hash: &str) -> SqlResult<Option<SavedTorrent>> {
        let mut stmt = self.conn.prepare(
            "SELECT info_hash, name, uri, state, downloaded, total,
                    output_dir, added_at, completed_at, last_active,
                    total_pieces, downloaded_pieces, sequential
             FROM torrents
             WHERE info_hash = ?1",
        )?;

        let mut rows = stmt.query_map([info_hash], |row| {
            Ok(SavedTorrent {
                info_hash: row.get(0)?,
                name: row.get(1)?,
                uri: row.get(2)?,
                state: row.get(3)?,
                downloaded: row.get::<_, i64>(4)? as u64,
                total: row.get::<_, i64>(5)? as u64,
                output_dir: row.get(6)?,
                added_at: row.get(7)?,
                completed_at: row.get(8)?,
                last_active: row.get(9)?,
                total_pieces: row.get::<_, i64>(10)? as u64,
                downloaded_pieces: row.get::<_, i64>(11)? as u64,
                sequential: row.get::<_, i32>(12)? != 0,
            })
        })?;

        rows.next().transpose()
    }

    /// Update torrent state
    #[allow(clippy::too_many_arguments)]
    pub fn update_torrent_state(
        &self,
        info_hash: &str,
        state: &str,
        downloaded: u64,
        total: u64,
        total_pieces: u64,
        downloaded_pieces: u64,
        last_active: i64,
    ) -> SqlResult<()> {
        log::debug!(
            "Updating torrent state: {} → {} (downloaded: {}, total: {}, pieces: {}/{})",
            info_hash,
            state,
            downloaded,
            total,
            downloaded_pieces,
            total_pieces
        );
        self.conn.execute(
            "UPDATE torrents 
             SET state = ?1, downloaded = ?2, total = ?3, last_active = ?4,
                 total_pieces = ?5, downloaded_pieces = ?6
             WHERE info_hash = ?7",
            rusqlite::params![
                state,
                downloaded as i64,
                total as i64,
                last_active,
                total_pieces as i64,
                downloaded_pieces as i64,
                info_hash
            ],
        )?;
        Ok(())
    }

    /// Mark torrent as completed
    pub fn mark_completed(&self, info_hash: &str, completed_at: i64) -> SqlResult<()> {
        log::info!("Marking torrent as completed: {}", info_hash);
        self.conn.execute(
            "UPDATE torrents 
             SET state = 'completed', completed_at = ?1, last_active = ?1
             WHERE info_hash = ?2",
            rusqlite::params![completed_at, info_hash],
        )?;
        Ok(())
    }

    /// Delete a torrent
    pub fn delete_torrent(&self, info_hash: &str) -> SqlResult<()> {
        log::info!("Deleting torrent from database: {}", info_hash);
        self.conn
            .execute("DELETE FROM torrents WHERE info_hash = ?1", [info_hash])?;
        Ok(())
    }

    /// Update torrent sequential flag
    pub fn update_torrent_sequential(&self, info_hash: &str, sequential: bool) -> SqlResult<()> {
        log::debug!(
            "Updating torrent sequential flag in DB: {} -> {}",
            info_hash,
            sequential
        );
        self.conn.execute(
            "UPDATE torrents SET sequential = ?1 WHERE info_hash = ?2",
            rusqlite::params![sequential as i32, info_hash],
        )?;
        Ok(())
    }

    /// Pause all downloading torrents
    pub fn pause_all_torrents(&self) -> SqlResult<()> {
        log::info!("Pausing all downloading torrents in database");
        self.conn.execute(
            "UPDATE torrents SET state = 'paused' WHERE state = 'downloading'",
            [],
        )?;
        Ok(())
    }

    // ==================== Settings ====================

    /// Get setting value by key
    pub fn get_setting(&self, key: &str) -> SqlResult<Option<String>> {
        let value: Option<String> = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .ok();
        Ok(value)
    }

    /// Set setting value
    pub fn set_setting(&self, key: &str, value: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Read just the PWP port setting (single query).
    pub fn get_pwp_port(&self) -> u16 {
        self.get_setting("pwp_port")
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// Load app settings from database.
    ///
    /// Reads the whole settings table in one query rather than firing a separate
    /// SELECT per key; callers such as `enforce_queue_limits()` invoke this often.
    pub fn load_settings(&self) -> AppSettings {
        let map: std::collections::HashMap<String, String> = self
            .conn
            .prepare("SELECT key, value FROM settings")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<SqlResult<std::collections::HashMap<_, _>>>()
            })
            .unwrap_or_default();

        let download_folder = map.get("download_folder").cloned().unwrap_or_else(|| {
            dirs_next::download_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .to_string_lossy()
                .to_string()
        });

        let window_width = map
            .get("window_width")
            .and_then(|v| v.parse().ok())
            .unwrap_or(375);

        let window_height = map
            .get("window_height")
            .and_then(|v| v.parse().ok())
            .unwrap_or(480);

        let window_maximized = map
            .get("window_maximized")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false);

        let log_level = map
            .get("log_level")
            .cloned()
            .unwrap_or_else(|| "info".to_string());

        let log_torrent_ops = map
            .get("log_torrent_ops")
            .and_then(|v| v.parse().ok())
            .unwrap_or(false);

        let max_active_downloads = map
            .get("max_active_downloads")
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let max_active_uploads = map
            .get("max_active_uploads")
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);

        let global_download_limit = map
            .get("global_download_limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let global_upload_limit = map
            .get("global_upload_limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let seeding_ratio_limit = map
            .get("seeding_ratio_limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);

        let pwp_port = map
            .get("pwp_port")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        AppSettings {
            download_folder,
            window_width,
            window_height,
            window_maximized,
            log_level,
            log_torrent_ops,
            max_active_downloads,
            max_active_uploads,
            global_download_limit,
            global_upload_limit,
            seeding_ratio_limit,
            pwp_port,
        }
    }

    /// Save app settings to database.
    ///
    /// All writes commit atomically inside a single transaction: a crash partway
    /// through can never leave some settings updated and others stale.
    pub fn save_settings(&self, settings: &AppSettings) -> SqlResult<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| {
            self.set_setting("download_folder", &settings.download_folder)?;
            self.set_setting("window_width", &settings.window_width.to_string())?;
            self.set_setting("window_height", &settings.window_height.to_string())?;
            self.set_setting("window_maximized", &settings.window_maximized.to_string())?;
            self.set_setting("log_level", &settings.log_level)?;
            self.set_setting("log_torrent_ops", &settings.log_torrent_ops.to_string())?;
            self.set_setting(
                "max_active_downloads",
                &settings.max_active_downloads.to_string(),
            )?;
            self.set_setting(
                "max_active_uploads",
                &settings.max_active_uploads.to_string(),
            )?;
            self.set_setting(
                "global_download_limit",
                &settings.global_download_limit.to_string(),
            )?;
            self.set_setting(
                "global_upload_limit",
                &settings.global_upload_limit.to_string(),
            )?;
            self.set_setting(
                "seeding_ratio_limit",
                &settings.seeding_ratio_limit.to_string(),
            )?;
            self.set_setting("pwp_port", &settings.pwp_port.to_string())?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rill-{name}-{}-{stamp}.db", std::process::id()))
    }

    fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({})", table))
            .unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .any(|name| name.as_deref() == Ok(column))
    }

    #[test]
    fn migrate_v3_adds_sequential_and_updates_version() {
        let path = temp_db_path("migrate-v3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
                 INSERT INTO schema_version (version) VALUES (3);
                 CREATE TABLE torrents (
                    info_hash TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    uri TEXT NOT NULL,
                    state TEXT NOT NULL,
                    downloaded INTEGER NOT NULL,
                    total INTEGER NOT NULL,
                    output_dir TEXT NOT NULL,
                    added_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    last_active INTEGER NOT NULL,
                    total_pieces INTEGER NOT NULL DEFAULT 0,
                    downloaded_pieces INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .unwrap();
        }

        let db = Database::open(&path).unwrap();
        let version: i32 = db
            .conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(column_exists(&db.conn, "torrents", "sequential"));
        drop(db);

        Database::open(&path).unwrap();
        let _ = std::fs::remove_file(path);
    }
}
