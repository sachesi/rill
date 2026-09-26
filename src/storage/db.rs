use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, OptionalExtension, Result as SqlResult, ToSql};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

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
            .optional()?;

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
                StoredPath(&torrent.output_dir),
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
                    output_dir: row.get::<_, StoredPath<PathBuf>>(6)?.0,
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
                output_dir: row.get::<_, StoredPath<PathBuf>>(6)?.0,
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

    /// Re-key a torrent record whose stored identity predates canonical
    /// info-hash IDs. `OR IGNORE` leaves the row untouched when the new key
    /// already exists (two legacy records for the same content); returns whether
    /// the row was actually re-keyed.
    pub fn migrate_torrent_hash(&self, old_hash: &str, new_hash: &str) -> SqlResult<bool> {
        let changed = self.conn.execute(
            "UPDATE OR IGNORE torrents SET info_hash = ?1 WHERE info_hash = ?2",
            rusqlite::params![new_hash, old_hash],
        )?;
        Ok(changed > 0)
    }

    /// Delete a torrent
    pub fn delete_torrent(&self, info_hash: &str) -> SqlResult<()> {
        log::info!("Deleting torrent from database: {}", info_hash);
        self.conn
            .execute("DELETE FROM torrents WHERE info_hash = ?1", [info_hash])?;
        Ok(())
    }

    /// Rename a torrent
    pub fn update_torrent_name(&self, info_hash: &str, name: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE torrents SET name = ?1 WHERE info_hash = ?2",
            rusqlite::params![name, info_hash],
        )?;
        Ok(())
    }

    /// Update where a torrent's content is kept
    pub fn update_torrent_output_dir(&self, info_hash: &str, output_dir: &Path) -> SqlResult<()> {
        log::info!(
            "Updating output dir of {info_hash} in DB: {}",
            output_dir.display()
        );
        self.conn.execute(
            "UPDATE torrents SET output_dir = ?1 WHERE info_hash = ?2",
            rusqlite::params![StoredPath(output_dir), info_hash],
        )?;
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

    /// Set setting value
    pub fn set_setting(&self, key: &str, value: impl ToSql) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Load app settings from database, in one query for the whole table.
    pub fn load_settings(&self) -> AppSettings {
        let map: HashMap<String, Value> = self
            .conn
            .prepare("SELECT key, value FROM settings")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Value>(1)?))
                })?
                .collect::<SqlResult<HashMap<_, _>>>()
            })
            .unwrap_or_default();
        let defaults = AppSettings::default();

        AppSettings {
            download_folder: map
                .get("download_folder")
                .and_then(|value| StoredPath::column_result(value.into()).ok())
                .map_or(defaults.download_folder, |path| path.0),
            window_width: parsed(&map, "window_width").unwrap_or(defaults.window_width),
            window_height: parsed(&map, "window_height").unwrap_or(defaults.window_height),
            window_maximized: parsed(&map, "window_maximized").unwrap_or(defaults.window_maximized),
            log_level: text(&map, "log_level").unwrap_or(defaults.log_level),
            max_active_downloads: parsed(&map, "max_active_downloads")
                .unwrap_or(defaults.max_active_downloads),
            pwp_port: parsed(&map, "pwp_port").unwrap_or(defaults.pwp_port),
            sort_order: text(&map, "sort_order").unwrap_or(defaults.sort_order),
        }
    }

    /// Save app settings to database, all of them in one transaction.
    pub fn save_settings(&self, settings: &AppSettings) -> SqlResult<()> {
        self.conn.execute_batch("BEGIN")?;
        let result = (|| {
            self.set_setting("download_folder", StoredPath(&settings.download_folder))?;
            self.set_setting("window_width", settings.window_width.to_string())?;
            self.set_setting("window_height", settings.window_height.to_string())?;
            self.set_setting("window_maximized", settings.window_maximized.to_string())?;
            self.set_setting("log_level", settings.log_level.as_str())?;
            self.set_setting(
                "max_active_downloads",
                settings.max_active_downloads.to_string(),
            )?;
            self.set_setting("pwp_port", settings.pwp_port.to_string())?;
            self.set_setting("sort_order", settings.sort_order.as_str())?;
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
}

fn text(map: &HashMap<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::Text(text) => Some(text.clone()),
        _ => None,
    }
}

fn parsed<T: std::str::FromStr>(map: &HashMap<String, Value>, key: &str) -> Option<T> {
    text(map, key).and_then(|v| v.parse().ok())
}

/// A path as the database keeps it: text when it is UTF-8, its bytes otherwise, so that a
/// path that is not comes back as it was rather than with its bytes replaced.
struct StoredPath<P>(P);

impl<P: AsRef<Path>> ToSql for StoredPath<P> {
    fn to_sql(&self) -> SqlResult<ToSqlOutput<'_>> {
        let path = self.0.as_ref();
        Ok(ToSqlOutput::Borrowed(match path.to_str() {
            Some(text) => ValueRef::Text(text.as_bytes()),
            None => ValueRef::Blob(path.as_os_str().as_bytes()),
        }))
    }
}

impl FromSql for StoredPath<PathBuf> {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                Ok(Self(PathBuf::from(OsStr::from_bytes(bytes))))
            }
            _ => Err(FromSqlError::InvalidType),
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

    fn torrent(hash: &str, state: &str) -> SavedTorrent {
        let mut torrent = SavedTorrent::new(
            hash.to_string(),
            format!("name of {hash}"),
            format!("magnet:?xt=urn:btih:{hash}"),
            state.to_string(),
            10,
            100,
            "/downloads".into(),
        );
        torrent.total_pieces = 4;
        torrent.downloaded_pieces = 1;
        torrent
    }

    fn open(name: &str) -> (Database, crate::test_support::ScratchDir) {
        let dir = crate::test_support::ScratchDir::new(name);
        (Database::open(dir.path().join("torrents.db")).unwrap(), dir)
    }

    fn state_of(db: &Database, hash: &str) -> String {
        db.load_torrent(hash).unwrap().unwrap().state
    }

    #[test]
    fn a_saved_torrent_loads_back_as_it_was() {
        let (db, _dir) = open("round-trip");
        let mut saved = torrent("aa", "paused");
        saved.sequential = true;
        db.save_torrent(&saved).unwrap();

        let loaded = db.load_torrent("aa").unwrap().unwrap();
        assert_eq!(
            (&loaded.name, &loaded.uri, &loaded.state, &loaded.output_dir),
            (&saved.name, &saved.uri, &saved.state, &saved.output_dir)
        );
        assert_eq!(
            (
                loaded.downloaded,
                loaded.total,
                loaded.total_pieces,
                loaded.downloaded_pieces
            ),
            (10, 100, 4, 1)
        );
        assert_eq!((loaded.added_at, loaded.sequential), (saved.added_at, true));
        assert!(db.load_torrent("bb").unwrap().is_none());
        assert_eq!(db.load_torrents().unwrap().len(), 1);
    }

    #[test]
    fn progress_completion_names_and_sequential_are_kept() {
        let (db, _dir) = open("updates");
        db.save_torrent(&torrent("aa", "downloading")).unwrap();

        db.update_torrent_state("aa", "paused", 50, 100, 4, 2, 7)
            .unwrap();
        let loaded = db.load_torrent("aa").unwrap().unwrap();
        assert_eq!((loaded.state.as_str(), loaded.downloaded), ("paused", 50));
        assert_eq!((loaded.downloaded_pieces, loaded.last_active), (2, 7));

        db.update_torrent_name("aa", "Real Name").unwrap();
        db.update_torrent_sequential("aa", true).unwrap();
        db.mark_completed("aa", 9).unwrap();
        let loaded = db.load_torrent("aa").unwrap().unwrap();
        assert_eq!(loaded.name, "Real Name");
        assert!(loaded.sequential);
        assert_eq!(
            (loaded.state.as_str(), loaded.completed_at),
            ("completed", Some(9))
        );
    }

    #[test]
    fn a_torrent_is_rekeyed_unless_its_new_key_is_taken() {
        let (db, _dir) = open("rekey");
        db.save_torrent(&torrent("old", "paused")).unwrap();
        assert!(db.migrate_torrent_hash("old", "new").unwrap());
        assert!(db.load_torrent("old").unwrap().is_none());
        assert!(db.load_torrent("new").unwrap().is_some());

        db.save_torrent(&torrent("other", "completed")).unwrap();
        assert!(!db.migrate_torrent_hash("other", "new").unwrap());
        assert_eq!(state_of(&db, "other"), "completed");
        assert_eq!(state_of(&db, "new"), "paused");

        db.delete_torrent("new").unwrap();
        assert!(db.load_torrent("new").unwrap().is_none());
    }

    #[test]
    fn settings_are_kept_and_unreadable_ones_fall_back_to_defaults() {
        let (db, _dir) = open("settings");
        let defaults = AppSettings::default();
        assert_eq!(
            db.load_settings().max_active_downloads,
            defaults.max_active_downloads
        );

        let mut settings = db.load_settings();
        settings.max_active_downloads = 5;
        settings.pwp_port = 51_000;
        settings.log_level = "debug".into();
        settings.sort_order = "size".into();
        settings.window_maximized = true;
        db.save_settings(&settings).unwrap();
        let loaded = db.load_settings();
        assert_eq!((loaded.max_active_downloads, loaded.pwp_port), (5, 51_000));
        assert_eq!(
            (loaded.log_level.as_str(), loaded.window_maximized),
            ("debug", true)
        );
        assert_eq!(loaded.sort_order, "size");

        db.set_setting("pwp_port", "not a port").unwrap();
        db.set_setting("max_active_downloads", "").unwrap();
        let loaded = db.load_settings();
        assert_eq!(loaded.pwp_port, defaults.pwp_port);
        assert_eq!(loaded.max_active_downloads, defaults.max_active_downloads);
    }

    #[test]
    fn paths_that_are_not_utf8_come_back_as_they_were() {
        let (db, _dir) = open("non-utf8");
        let path = PathBuf::from(OsStr::from_bytes(b"/downloads/caf\xe9"));
        let mut saved = torrent("aa", "paused");
        saved.output_dir = path.clone();
        db.save_torrent(&saved).unwrap();
        assert_eq!(db.load_torrent("aa").unwrap().unwrap().output_dir, path);

        let moved = PathBuf::from(OsStr::from_bytes(b"/elsewhere/\xff"));
        db.update_torrent_output_dir("aa", &moved).unwrap();
        assert_eq!(db.load_torrents().unwrap()[0].output_dir, moved);

        let mut settings = db.load_settings();
        settings.download_folder = path.clone();
        settings.max_active_downloads = 5;
        db.save_settings(&settings).unwrap();
        let loaded = db.load_settings();
        assert_eq!(loaded.download_folder, path);
        assert_eq!(loaded.max_active_downloads, 5);
    }

    #[test]
    fn an_unreadable_schema_version_is_reported_as_it_is() {
        let dir = crate::test_support::ScratchDir::new("bad-version");
        let path = dir.path().join("torrents.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE schema_version (version);
                 INSERT INTO schema_version VALUES ('x');
                 CREATE TABLE torrents (info_hash TEXT PRIMARY KEY);",
            )
            .unwrap();
        let err = Database::open(&path).unwrap_err();
        assert!(
            matches!(err, rusqlite::Error::InvalidColumnType(..)),
            "{err}"
        );
    }

    #[test]
    fn a_fresh_database_has_the_current_schema() {
        let (db, _dir) = open("fresh");
        let version: i32 = db
            .conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(column_exists(&db.conn, "torrents", "sequential"));
    }
}
