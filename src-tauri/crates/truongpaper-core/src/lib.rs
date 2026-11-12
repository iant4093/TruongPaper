//! `truongpaper-core`: Shared SQLite WAL database schema, IPC models, and error types.
//! Complete specification: `docs/02-PHOTO-API-AND-CACHE-SPEC.md` & `docs/04-UI-UX-LIQUID-GLASS-SPEC.md`.

pub mod error;
pub mod models;
pub mod repo;

pub use error::{CoreError, CoreResult};
pub use models::*;
pub use repo::*;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

macro_rules! pragmas_sql {
    () => {
        r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
"#
    };
}

macro_rules! tables_sql {
    () => {
        r#"
CREATE TABLE IF NOT EXISTS photos_cache (
    id              TEXT PRIMARY KEY,
    file_path       TEXT NOT NULL,
    file_present    INTEGER NOT NULL DEFAULT 1,
    album_slug      TEXT NOT NULL,
    album_url       TEXT NOT NULL,
    featured        INTEGER NOT NULL,
    fallback_used   INTEGER NOT NULL DEFAULT 0,
    width           INTEGER NOT NULL,
    height          INTEGER NOT NULL,
    orientation     TEXT NOT NULL,
    bytes           INTEGER NOT NULL,
    is_favorite     INTEGER NOT NULL DEFAULT 0,
    downloaded_at   INTEGER NOT NULL,
    last_shown_at   INTEGER,
    shown_count     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS photos_history (
    photo_id        TEXT PRIMARY KEY,
    album_slug      TEXT NOT NULL,
    featured        INTEGER NOT NULL,
    orientation     TEXT NOT NULL,
    first_shown_at  INTEGER NOT NULL,
    last_shown_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS photos_blacklist (
    photo_id        TEXT PRIMARY KEY,
    album_slug      TEXT,
    hidden_at       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS saved_albums (
    album_slug          TEXT PRIMARY KEY,
    title               TEXT NOT NULL,
    description         TEXT,
    category            TEXT,
    album_url           TEXT NOT NULL,
    cover_thumbnail_url TEXT,
    cover_blurhash      TEXT,
    image_count         INTEGER,
    is_enabled          INTEGER NOT NULL DEFAULT 1,
    added_at            INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS wallpaper_configs (
    wallpaper_id        TEXT PRIMARY KEY,
    workshop_id         TEXT,
    title               TEXT NOT NULL,
    wallpaper_type      TEXT NOT NULL,
    install_dir         TEXT NOT NULL,
    preview_path        TEXT,
    bytes               INTEGER NOT NULL DEFAULT 0,
    audio_reactive      INTEGER NOT NULL DEFAULT 0,
    user_properties_json TEXT NOT NULL DEFAULT '{}',
    adjustments_json    TEXT NOT NULL DEFAULT '{}',
    updated_at          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS wallpaper_presets (
    preset_id           TEXT PRIMARY KEY,
    wallpaper_id        TEXT NOT NULL,
    preset_name         TEXT NOT NULL,
    user_properties_json TEXT NOT NULL,
    adjustments_json    TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    FOREIGN KEY (wallpaper_id) REFERENCES wallpaper_configs(wallpaper_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS playlists (
    playlist_id         TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    mode                TEXT NOT NULL,
    interval_seconds    INTEGER NOT NULL DEFAULT 900,
    items_json          TEXT NOT NULL DEFAULT '[]',
    updated_at          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS monitor_assignments (
    monitor_id          TEXT PRIMARY KEY,
    source_json         TEXT NOT NULL,
    current_photo_id    TEXT,
    updated_at          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key                 TEXT PRIMARY KEY,
    value_json          TEXT NOT NULL
);
"#
    };
}

macro_rules! indexes_sql {
    () => {
        r#"
CREATE INDEX IF NOT EXISTS idx_photos_unused_queue
    ON photos_cache (file_present, last_shown_at, orientation, featured, album_slug);

CREATE INDEX IF NOT EXISTS idx_photos_unused_fifo
    ON photos_cache (downloaded_at ASC)
    WHERE file_present = 1 AND last_shown_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_photos_history_filter
    ON photos_history (album_slug, featured, orientation);
"#
    };
}

/// Full schema (pragmas, tables, indexes) for a database that is already at the current
/// version. Fresh in-memory databases use this directly; [`open_and_initialize_db`] migrates.
pub const SQLITE_SCHEMA_SQL: &str = concat!(pragmas_sql!(), tables_sql!(), indexes_sql!());
const PRAGMAS_SQL: &str = pragmas_sql!();
const TABLES_SQL: &str = tables_sql!();
const INDEXES_SQL: &str = indexes_sql!();

/// `PRAGMA user_version` written by this build. Version 1 is the schema above.
pub const SCHEMA_VERSION: i64 = 1;

/// Every non-primary-key column of every table, with a definition valid for
/// `ALTER TABLE ... ADD COLUMN` (NOT NULL columns carry a default). Used to upgrade databases
/// created by builds that predate a column.
const EXPECTED_COLUMNS: &[(&str, &str, &str)] = &[
    ("photos_cache", "file_path", "TEXT NOT NULL DEFAULT ''"),
    ("photos_cache", "file_present", "INTEGER NOT NULL DEFAULT 1"),
    ("photos_cache", "album_slug", "TEXT NOT NULL DEFAULT ''"),
    ("photos_cache", "album_url", "TEXT NOT NULL DEFAULT ''"),
    ("photos_cache", "featured", "INTEGER NOT NULL DEFAULT 0"),
    (
        "photos_cache",
        "fallback_used",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    ("photos_cache", "width", "INTEGER NOT NULL DEFAULT 0"),
    ("photos_cache", "height", "INTEGER NOT NULL DEFAULT 0"),
    (
        "photos_cache",
        "orientation",
        "TEXT NOT NULL DEFAULT 'landscape'",
    ),
    ("photos_cache", "bytes", "INTEGER NOT NULL DEFAULT 0"),
    ("photos_cache", "is_favorite", "INTEGER NOT NULL DEFAULT 0"),
    (
        "photos_cache",
        "downloaded_at",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    ("photos_cache", "last_shown_at", "INTEGER"),
    ("photos_cache", "shown_count", "INTEGER NOT NULL DEFAULT 0"),
    ("photos_history", "album_slug", "TEXT NOT NULL DEFAULT ''"),
    ("photos_history", "featured", "INTEGER NOT NULL DEFAULT 0"),
    (
        "photos_history",
        "orientation",
        "TEXT NOT NULL DEFAULT 'landscape'",
    ),
    (
        "photos_history",
        "first_shown_at",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "photos_history",
        "last_shown_at",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    ("photos_blacklist", "album_slug", "TEXT"),
    (
        "photos_blacklist",
        "hidden_at",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    ("saved_albums", "title", "TEXT NOT NULL DEFAULT ''"),
    ("saved_albums", "description", "TEXT"),
    ("saved_albums", "category", "TEXT"),
    ("saved_albums", "album_url", "TEXT NOT NULL DEFAULT ''"),
    ("saved_albums", "cover_thumbnail_url", "TEXT"),
    ("saved_albums", "cover_blurhash", "TEXT"),
    ("saved_albums", "image_count", "INTEGER"),
    ("saved_albums", "is_enabled", "INTEGER NOT NULL DEFAULT 1"),
    ("saved_albums", "added_at", "INTEGER NOT NULL DEFAULT 0"),
    ("wallpaper_configs", "workshop_id", "TEXT"),
    ("wallpaper_configs", "title", "TEXT NOT NULL DEFAULT ''"),
    (
        "wallpaper_configs",
        "wallpaper_type",
        "TEXT NOT NULL DEFAULT ''",
    ),
    (
        "wallpaper_configs",
        "install_dir",
        "TEXT NOT NULL DEFAULT ''",
    ),
    ("wallpaper_configs", "preview_path", "TEXT"),
    ("wallpaper_configs", "bytes", "INTEGER NOT NULL DEFAULT 0"),
    (
        "wallpaper_configs",
        "audio_reactive",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "wallpaper_configs",
        "user_properties_json",
        "TEXT NOT NULL DEFAULT '{}'",
    ),
    (
        "wallpaper_configs",
        "adjustments_json",
        "TEXT NOT NULL DEFAULT '{}'",
    ),
    (
        "wallpaper_configs",
        "updated_at",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "wallpaper_presets",
        "wallpaper_id",
        "TEXT NOT NULL DEFAULT ''",
    ),
    (
        "wallpaper_presets",
        "preset_name",
        "TEXT NOT NULL DEFAULT ''",
    ),
    (
        "wallpaper_presets",
        "user_properties_json",
        "TEXT NOT NULL DEFAULT '{}'",
    ),
    (
        "wallpaper_presets",
        "adjustments_json",
        "TEXT NOT NULL DEFAULT '{}'",
    ),
    (
        "wallpaper_presets",
        "created_at",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    ("playlists", "name", "TEXT NOT NULL DEFAULT ''"),
    ("playlists", "mode", "TEXT NOT NULL DEFAULT ''"),
    (
        "playlists",
        "interval_seconds",
        "INTEGER NOT NULL DEFAULT 900",
    ),
    ("playlists", "items_json", "TEXT NOT NULL DEFAULT '[]'"),
    ("playlists", "updated_at", "INTEGER NOT NULL DEFAULT 0"),
    (
        "monitor_assignments",
        "source_json",
        "TEXT NOT NULL DEFAULT '{}'",
    ),
    ("monitor_assignments", "current_photo_id", "TEXT"),
    (
        "monitor_assignments",
        "updated_at",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    ("settings", "value_json", "TEXT NOT NULL DEFAULT 'null'"),
];

fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

fn has_user_tables(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn backup_before_migration(conn: &Connection, db_path: &Path, old_version: i64) {
    // Fold the WAL into the main file so a plain file copy is a complete snapshot.
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    let Some(name) = db_path.file_name() else {
        return;
    };
    let mut backup_name = name.to_os_string();
    backup_name.push(format!(".bak-v{old_version}"));
    let backup = db_path.with_file_name(backup_name);
    if backup.exists() {
        return; // Keep the first (oldest) snapshot.
    }
    if let Err(err) = std::fs::copy(db_path, &backup) {
        tracing::warn!("could not back up database before migration: {err}");
    }
}

/// Upgrades `conn` to [`SCHEMA_VERSION`]. A database with `user_version = 0` is either new or
/// created before versioning: missing tables are created, missing columns are added with
/// `ALTER TABLE ADD COLUMN` (guarded by `pragma_table_info`), then indexes are created and the
/// version is stamped, all in one transaction.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    if user_version(conn)? >= SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute_batch(TABLES_SQL)?;
    for (table, column, definition) in EXPECTED_COLUMNS {
        let present: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            [table, column],
            |r| r.get(0),
        )?;
        if present == 0 {
            tx.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition}"
            ))?;
        }
    }
    tx.execute_batch(INDEXES_SQL)?;
    tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    tx.commit()
}

pub fn open_and_initialize_db(db_path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut conn = Connection::open(db_path)?;
    // Connection-level settings cannot run inside the migration transaction.
    conn.execute_batch(PRAGMAS_SQL)?;
    let version = user_version(&conn)?;
    if version < SCHEMA_VERSION && has_user_tables(&conn)? {
        backup_before_migration(&conn, db_path, version);
    }
    migrate(&mut conn)?;
    Ok(conn)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub rotation_interval_seconds: u64,
    pub transition_type: String,
    pub transition_duration_seconds: f32,
    pub skip_mismatched_orientation: bool,
    pub default_fit_mode: String,
    pub exhaustion_auto_reset: bool,
    pub max_photo_cache_bytes: u64,
    pub fps_cap: serde_json::Value,
    pub quality_preset: String,
    pub pause_on_fullscreen: bool,
    pub pause_on_maximized: bool,
    pub battery_policy: String,
    pub launch_at_login: bool,
    #[serde(default)]
    pub sync_lock_screen: bool,
    /// `"independent"` (each display has its own wallpaper) or `"span"` (one wallpaper across
    /// the union of all displays).
    #[serde(default = "default_multi_monitor_mode")]
    pub multi_monitor_mode: String,
}

fn default_multi_monitor_mode() -> String {
    "independent".into()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            rotation_interval_seconds: 1800,
            transition_type: "liquid_glass".into(),
            transition_duration_seconds: 1.2,
            skip_mismatched_orientation: true,
            default_fit_mode: "fill".into(),
            exhaustion_auto_reset: true,
            max_photo_cache_bytes: 250 * 1024 * 1024,
            fps_cap: serde_json::json!(60),
            quality_preset: "high".into(),
            pause_on_fullscreen: true,
            pause_on_maximized: false,
            battery_policy: "pause_animations".into(),
            launch_at_login: false,
            sync_lock_screen: false,
            multi_monitor_mode: default_multi_monitor_mode(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "truongpaper-core-mig-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("truongpaper.db")
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info(?1)")
            .unwrap();
        stmt.query_map([table], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn old_minimal_database_is_migrated_in_place() {
        let path = temp_db_path("old");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE photos_cache (id TEXT PRIMARY KEY, file_path TEXT NOT NULL);
                 INSERT INTO photos_cache VALUES ('a', 'wallpapers/photos/a.jpg');
                 CREATE TABLE settings (key TEXT PRIMARY KEY);",
            )
            .unwrap();
        }
        let conn = open_and_initialize_db(&path).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        for (table, column, _) in EXPECTED_COLUMNS {
            assert!(
                columns(&conn, table).iter().any(|c| c == column),
                "{table}.{column} missing after migration"
            );
        }
        let row = get_cached_photo(&conn, "a").unwrap().expect("row kept");
        assert_eq!(row.file_path, "wallpapers/photos/a.jpg");
        assert!(row.file_present && !row.is_favorite);
        assert_eq!(row.last_shown_at, None);
        // Normal queries work.
        let filter = PhotoFilter::default();
        assert_eq!(count_unused_in_queue(&conn, &filter).unwrap(), 1);
        set_favorite(&conn, "a", true).unwrap();
        assert!(list_favorite_photos(&conn, Path::new("/x")).unwrap().len() == 1);
        // The pre-migration snapshot exists and is untouched by the new columns.
        let backup = path.with_file_name("truongpaper.db.bak-v0");
        assert!(backup.exists());
        let old = Connection::open(&backup).unwrap();
        assert_eq!(columns(&old, "photos_cache").len(), 2);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reopening_is_idempotent_and_fresh_db_gets_no_backup() {
        let path = temp_db_path("fresh");
        {
            let conn = open_and_initialize_db(&path).unwrap();
            conn.execute(
                "INSERT INTO photos_cache (id, file_path, album_slug, album_url, featured,
                    width, height, orientation, bytes, downloaded_at)
                 VALUES ('a', 'p', 's', 'u', 0, 1, 1, 'landscape', 1, 1)",
                [],
            )
            .unwrap();
        }
        assert!(!path.with_file_name("truongpaper.db.bak-v0").exists());
        for _ in 0..2 {
            let conn = open_and_initialize_db(&path).unwrap();
            assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
            assert!(get_cached_photo(&conn, "a").unwrap().is_some());
        }
        assert!(!path.with_file_name("truongpaper.db.bak-v0").exists());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn initializes_in_memory_sqlite_schema() {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(SQLITE_SCHEMA_SQL)
            .expect("execute schema batch");
    }
}
