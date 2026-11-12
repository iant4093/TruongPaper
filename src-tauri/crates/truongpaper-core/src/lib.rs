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

pub const SQLITE_SCHEMA_SQL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

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

CREATE INDEX IF NOT EXISTS idx_photos_unused_queue
    ON photos_cache (file_present, last_shown_at, orientation, featured, album_slug);

CREATE INDEX IF NOT EXISTS idx_photos_unused_fifo
    ON photos_cache (downloaded_at ASC)
    WHERE file_present = 1 AND last_shown_at IS NULL;

CREATE TABLE IF NOT EXISTS photos_history (
    photo_id        TEXT PRIMARY KEY,
    album_slug      TEXT NOT NULL,
    featured        INTEGER NOT NULL,
    orientation     TEXT NOT NULL,
    first_shown_at  INTEGER NOT NULL,
    last_shown_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_photos_history_filter
    ON photos_history (album_slug, featured, orientation);

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
"#;

pub fn open_and_initialize_db(db_path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch(SQLITE_SCHEMA_SQL)?;
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

    #[test]
    fn initializes_in_memory_sqlite_schema() {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(SQLITE_SCHEMA_SQL)
            .expect("execute schema batch");
    }
}
