//! SQLite access for the tables `truongpaper-core` defines but does not wrap:
//! `wallpaper_configs`, `wallpaper_presets`, `playlists` and `monitor_assignments.source_json`.
//! All values are bound as parameters.

use crate::models::{
    IpcFitMode, Playlist, UniversalAdjustments, WallpaperItem, WallpaperPreset, WallpaperSource,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use truongpaper_core::now_ms;
use truongpaper_steam::DiscoveredWorkshopWallpaper;

pub type DbResult<T> = Result<T, String>;

fn e(err: impl std::fmt::Display) -> String {
    err.to_string()
}

/// Process-unique id with a readable prefix (`preset-<ms>-<n>`); no RNG dependency needed.
pub fn new_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{prefix}-{}-{}",
        now_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

// ---------------------------------------------------------------------------------------------
// wallpaper_configs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct WallpaperRow {
    pub wallpaper_id: String,
    pub workshop_id: Option<String>,
    pub title: String,
    pub wallpaper_type: String,
    pub install_dir: String,
    pub preview_path: Option<String>,
    pub bytes: u64,
    pub audio_reactive: bool,
    pub user_properties_json: String,
    pub adjustments_json: String,
}

impl WallpaperRow {
    pub fn user_properties(&self) -> Map<String, Value> {
        serde_json::from_str(&self.user_properties_json).unwrap_or_default()
    }

    pub fn adjustments(&self, default_fit: IpcFitMode) -> UniversalAdjustments {
        UniversalAdjustments::from_stored_json(&self.adjustments_json, default_fit)
    }

    /// Builds the IPC item; `project_json` is read from `<install_dir>/project.json`.
    /// Returns `None` when the folder or its `project.json` is gone.
    pub fn to_item(&self, default_fit: IpcFitMode) -> Option<WallpaperItem> {
        let project_json = read_project_json(Path::new(&self.install_dir))?;
        Some(WallpaperItem {
            wallpaper_id: self.wallpaper_id.clone(),
            workshop_id: self.workshop_id.clone(),
            title: self.title.clone(),
            wallpaper_type: self.wallpaper_type.clone(),
            preview_path: self.preview_path.clone().unwrap_or_default(),
            install_dir: self.install_dir.clone(),
            bytes: self.bytes,
            audio_reactive: self.audio_reactive,
            // docs/03 §10: application wallpapers only run on Windows; elsewhere the UI shows the
            // "Windows Only" badge.
            windows_only: !truongpaper_platform::is_wallpaper_type_supported_on_this_os(
                &self.wallpaper_type,
            ),
            project_json,
            user_properties: self.user_properties(),
            adjustments: self.adjustments(default_fit),
        })
    }
}

/// Parses `project.json` (tolerating a UTF-8 BOM).
pub fn read_project_json(install_dir: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(install_dir.join("project.json")).ok()?;
    serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()
}

const WALLPAPER_COLUMNS: &str = "wallpaper_id, workshop_id, title, wallpaper_type, install_dir,
    preview_path, bytes, audio_reactive, user_properties_json, adjustments_json";

fn wallpaper_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<WallpaperRow> {
    Ok(WallpaperRow {
        wallpaper_id: r.get(0)?,
        workshop_id: r.get(1)?,
        title: r.get(2)?,
        wallpaper_type: r.get(3)?,
        install_dir: r.get(4)?,
        preview_path: r.get(5)?,
        bytes: r.get::<_, i64>(6)?.max(0) as u64,
        audio_reactive: r.get::<_, i64>(7)? != 0,
        user_properties_json: r.get(8)?,
        adjustments_json: r.get(9)?,
    })
}

/// Inserts or refreshes a scanned wallpaper. User properties and adjustments are preserved.
pub fn upsert_discovered(conn: &Connection, w: &DiscoveredWorkshopWallpaper) -> DbResult<()> {
    conn.execute(
        "INSERT INTO wallpaper_configs (
            wallpaper_id, workshop_id, title, wallpaper_type, install_dir, preview_path,
            bytes, audio_reactive, user_properties_json, adjustments_json, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '{}', '{}', ?9)
        ON CONFLICT(wallpaper_id) DO UPDATE SET
            workshop_id = excluded.workshop_id,
            title = excluded.title,
            wallpaper_type = excluded.wallpaper_type,
            install_dir = excluded.install_dir,
            preview_path = excluded.preview_path,
            bytes = excluded.bytes,
            audio_reactive = excluded.audio_reactive,
            updated_at = excluded.updated_at",
        params![
            w.wallpaper_id,
            w.workshop_id,
            w.title,
            w.r#type,
            w.install_dir,
            w.preview_path,
            w.bytes.min(i64::MAX as u64) as i64,
            i32::from(w.audio_reactive),
            now_ms(),
        ],
    )
    .map_err(e)?;
    Ok(())
}

pub fn list_wallpapers(conn: &Connection) -> DbResult<Vec<WallpaperRow>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {WALLPAPER_COLUMNS} FROM wallpaper_configs ORDER BY title COLLATE NOCASE, wallpaper_id"
        ))
        .map_err(e)?;
    let rows = stmt
        .query_map([], wallpaper_row)
        .map_err(e)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(e)?;
    Ok(rows)
}

pub fn get_wallpaper(conn: &Connection, wallpaper_id: &str) -> DbResult<Option<WallpaperRow>> {
    conn.query_row(
        &format!("SELECT {WALLPAPER_COLUMNS} FROM wallpaper_configs WHERE wallpaper_id = ?1"),
        params![wallpaper_id],
        wallpaper_row,
    )
    .optional()
    .map_err(e)
}

fn require_wallpaper(conn: &Connection, wallpaper_id: &str) -> DbResult<WallpaperRow> {
    get_wallpaper(conn, wallpaper_id)?
        .ok_or_else(|| format!("Wallpaper '{wallpaper_id}' is not in the library"))
}

/// Merges one `project.json` user property override.
pub fn set_user_property(
    conn: &Connection,
    wallpaper_id: &str,
    key: &str,
    value: &Value,
) -> DbResult<Map<String, Value>> {
    let row = require_wallpaper(conn, wallpaper_id)?;
    let mut props = row.user_properties();
    props.insert(key.trim().to_string(), value.clone());
    let json = serde_json::to_string(&props).map_err(e)?;
    conn.execute(
        "UPDATE wallpaper_configs SET user_properties_json = ?1, updated_at = ?2 WHERE wallpaper_id = ?3",
        params![json, now_ms(), wallpaper_id],
    )
    .map_err(e)?;
    Ok(props)
}

pub fn set_adjustments(
    conn: &Connection,
    wallpaper_id: &str,
    adjustments: &UniversalAdjustments,
) -> DbResult<()> {
    require_wallpaper(conn, wallpaper_id)?;
    let json = serde_json::to_string(adjustments).map_err(e)?;
    conn.execute(
        "UPDATE wallpaper_configs SET adjustments_json = ?1, updated_at = ?2 WHERE wallpaper_id = ?3",
        params![json, now_ms(), wallpaper_id],
    )
    .map_err(e)?;
    Ok(())
}

/// Deletes the library row (presets cascade) and every monitor assignment pointing at it.
pub fn delete_wallpaper(conn: &Connection, wallpaper_id: &str) -> DbResult<Option<WallpaperRow>> {
    let row = get_wallpaper(conn, wallpaper_id)?;
    conn.execute(
        "DELETE FROM wallpaper_presets WHERE wallpaper_id = ?1",
        params![wallpaper_id],
    )
    .map_err(e)?;
    conn.execute(
        "DELETE FROM wallpaper_configs WHERE wallpaper_id = ?1",
        params![wallpaper_id],
    )
    .map_err(e)?;
    let source = serde_json::to_string(&WallpaperSource::Workshop {
        wallpaper_id: wallpaper_id.to_string(),
    })
    .map_err(e)?;
    conn.execute(
        "DELETE FROM monitor_assignments WHERE source_json = ?1",
        params![source],
    )
    .map_err(e)?;
    Ok(row)
}

// ---------------------------------------------------------------------------------------------
// wallpaper_presets
// ---------------------------------------------------------------------------------------------

/// Snapshots the wallpaper's current properties + adjustments as a named preset.
pub fn save_preset(
    conn: &Connection,
    wallpaper_id: &str,
    preset_name: &str,
    default_fit: IpcFitMode,
) -> DbResult<WallpaperPreset> {
    let name = preset_name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err("Preset name must be 1-128 characters".into());
    }
    let row = require_wallpaper(conn, wallpaper_id)?;
    let preset = WallpaperPreset {
        preset_id: new_id("preset"),
        wallpaper_id: wallpaper_id.to_string(),
        name: name.to_string(),
        properties: row.user_properties(),
        adjustments: row.adjustments(default_fit),
    };
    conn.execute(
        "INSERT INTO wallpaper_presets
            (preset_id, wallpaper_id, preset_name, user_properties_json, adjustments_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            preset.preset_id,
            preset.wallpaper_id,
            preset.name,
            serde_json::to_string(&preset.properties).map_err(e)?,
            serde_json::to_string(&preset.adjustments).map_err(e)?,
            now_ms(),
        ],
    )
    .map_err(e)?;
    Ok(preset)
}

/// Copies a preset back onto its wallpaper.
pub fn apply_preset(conn: &Connection, wallpaper_id: &str, preset_id: &str) -> DbResult<()> {
    let found: Option<(String, String)> = conn
        .query_row(
            "SELECT user_properties_json, adjustments_json FROM wallpaper_presets
             WHERE preset_id = ?1 AND wallpaper_id = ?2",
            params![preset_id, wallpaper_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(e)?;
    let (props, adjustments) =
        found.ok_or_else(|| format!("Preset '{preset_id}' not found for '{wallpaper_id}'"))?;
    conn.execute(
        "UPDATE wallpaper_configs
         SET user_properties_json = ?1, adjustments_json = ?2, updated_at = ?3
         WHERE wallpaper_id = ?4",
        params![props, adjustments, now_ms(), wallpaper_id],
    )
    .map_err(e)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// playlists
// ---------------------------------------------------------------------------------------------

pub const MIN_PLAYLIST_INTERVAL_SECONDS: u64 = 10;

pub fn list_playlists(conn: &Connection) -> DbResult<Vec<Playlist>> {
    let mut stmt = conn
        .prepare(
            "SELECT playlist_id, name, mode, interval_seconds, items_json
             FROM playlists ORDER BY name COLLATE NOCASE, playlist_id",
        )
        .map_err(e)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(e)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(e)?;
    Ok(rows
        .into_iter()
        .map(|(playlist_id, name, mode, interval, items)| Playlist {
            playlist_id,
            name,
            mode: serde_json::from_value(Value::String(mode)).unwrap_or_default(),
            interval_seconds: interval.max(0) as u64,
            items: serde_json::from_str(&items).unwrap_or_default(),
        })
        .collect())
}

pub fn get_playlist(conn: &Connection, playlist_id: &str) -> DbResult<Option<Playlist>> {
    Ok(list_playlists(conn)?
        .into_iter()
        .find(|p| p.playlist_id == playlist_id))
}

/// Upserts a playlist; assigns an id when empty and clamps the interval.
pub fn save_playlist(conn: &Connection, mut playlist: Playlist) -> DbResult<Playlist> {
    playlist.name = playlist.name.trim().to_string();
    if playlist.name.is_empty() || playlist.name.len() > 128 {
        return Err("Playlist name must be 1-128 characters".into());
    }
    if playlist.playlist_id.trim().is_empty() {
        playlist.playlist_id = new_id("playlist");
    }
    playlist.interval_seconds = playlist.interval_seconds.max(MIN_PLAYLIST_INTERVAL_SECONDS);
    let mode = serde_json::to_value(playlist.mode).map_err(e)?;
    conn.execute(
        "INSERT INTO playlists (playlist_id, name, mode, interval_seconds, items_json, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(playlist_id) DO UPDATE SET
            name = excluded.name,
            mode = excluded.mode,
            interval_seconds = excluded.interval_seconds,
            items_json = excluded.items_json,
            updated_at = excluded.updated_at",
        params![
            playlist.playlist_id,
            playlist.name,
            mode.as_str().unwrap_or("mixed"),
            playlist.interval_seconds.min(i64::MAX as u64) as i64,
            serde_json::to_string(&playlist.items).map_err(e)?,
            now_ms(),
        ],
    )
    .map_err(e)?;
    Ok(playlist)
}

// ---------------------------------------------------------------------------------------------
// monitor_assignments
// ---------------------------------------------------------------------------------------------

/// `monitor_id -> source` for every assigned monitor.
pub fn monitor_sources(conn: &Connection) -> DbResult<HashMap<String, WallpaperSource>> {
    let mut stmt = conn
        .prepare("SELECT monitor_id, source_json FROM monitor_assignments")
        .map_err(e)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(e)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(e)?;
    Ok(rows
        .into_iter()
        .map(|(id, json)| (id, WallpaperSource::from_stored_json(&json)))
        .collect())
}

/// Sets a monitor's source. Photo sources keep the current photo id; other sources clear it so
/// the photo becomes evictable.
pub fn set_monitor_source(
    conn: &Connection,
    monitor_id: &str,
    source: &WallpaperSource,
) -> DbResult<()> {
    let json = serde_json::to_string(source).map_err(e)?;
    let sql = if matches!(source, WallpaperSource::PhotoApi) {
        "INSERT INTO monitor_assignments (monitor_id, source_json, current_photo_id, updated_at)
         VALUES (?1, ?2, NULL, ?3)
         ON CONFLICT(monitor_id) DO UPDATE SET
            source_json = excluded.source_json, updated_at = excluded.updated_at"
    } else {
        "INSERT INTO monitor_assignments (monitor_id, source_json, current_photo_id, updated_at)
         VALUES (?1, ?2, NULL, ?3)
         ON CONFLICT(monitor_id) DO UPDATE SET
            source_json = excluded.source_json,
            current_photo_id = NULL,
            updated_at = excluded.updated_at"
    };
    conn.execute(sql, params![monitor_id, json, now_ms()])
        .map_err(e)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{PlaylistItem, PlaylistMode};
    use crate::state::test_db;
    use serde_json::json;

    fn discovered(id: &str, dir: &Path) -> DiscoveredWorkshopWallpaper {
        DiscoveredWorkshopWallpaper {
            wallpaper_id: format!("workshop:{id}"),
            workshop_id: id.into(),
            title: format!("Title {id}"),
            r#type: "scene".into(),
            preview_path: dir.join("preview.jpg").to_string_lossy().into_owned(),
            install_dir: dir.to_string_lossy().into_owned(),
            bytes: 42,
            audio_reactive: true,
            windows_only: false,
            project_json: json!({}),
        }
    }

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!(
                "truongpaper-app-db-{tag}-{}-{}",
                std::process::id(),
                new_id("t")
            ));
            std::fs::create_dir_all(&d).unwrap();
            TempDir(d)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn library_upsert_preserves_user_state_and_builds_items() {
        let conn = test_db();
        let dir = TempDir::new("lib");
        std::fs::write(
            dir.0.join("project.json"),
            "\u{feff}{\"title\":\"T\",\"type\":\"scene\",\"file\":\"scene.json\"}",
        )
        .unwrap();
        upsert_discovered(&conn, &discovered("1", &dir.0)).unwrap();
        set_user_property(&conn, "workshop:1", "speed", &json!(2)).unwrap();
        let adj = UniversalAdjustments {
            zoom: 1.25,
            ..Default::default()
        };
        set_adjustments(&conn, "workshop:1", &adj).unwrap();
        // Re-scan must not wipe user state.
        let mut again = discovered("1", &dir.0);
        again.title = "Renamed".into();
        upsert_discovered(&conn, &again).unwrap();

        let rows = list_wallpapers(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        let item = rows[0].to_item(IpcFitMode::FitBlur).unwrap();
        assert_eq!(item.title, "Renamed");
        assert_eq!(item.user_properties["speed"], json!(2));
        assert_eq!(item.adjustments.zoom, 1.25);
        assert_eq!(item.project_json["file"], "scene.json");
        assert!(item.audio_reactive);
        assert!(set_user_property(&conn, "workshop:missing", "a", &json!(1)).is_err());

        // Missing project.json hides the row from the library listing.
        std::fs::remove_file(dir.0.join("project.json")).unwrap();
        assert!(rows[0].to_item(IpcFitMode::FitBlur).is_none());
    }

    #[test]
    fn windows_only_flags_application_items_off_windows() {
        let dir = TempDir::new("winonly");
        std::fs::write(dir.0.join("project.json"), "{\"title\":\"T\"}").unwrap();
        for (ty, expected) in [
            ("application", !cfg!(target_os = "windows")),
            ("Application", !cfg!(target_os = "windows")),
            ("scene", false),
            ("video", false),
            ("web", false),
        ] {
            let row = WallpaperRow {
                wallpaper_id: format!("workshop:{ty}"),
                workshop_id: None,
                title: "T".into(),
                wallpaper_type: ty.into(),
                install_dir: dir.0.to_string_lossy().into_owned(),
                preview_path: None,
                bytes: 0,
                audio_reactive: false,
                user_properties_json: "{}".into(),
                adjustments_json: "{}".into(),
            };
            let item = row.to_item(IpcFitMode::FitBlur).unwrap();
            assert_eq!(item.windows_only, expected, "{ty}");
        }
    }

    #[test]
    fn presets_snapshot_and_restore() {
        let conn = test_db();
        let dir = TempDir::new("preset");
        upsert_discovered(&conn, &discovered("7", &dir.0)).unwrap();
        set_user_property(&conn, "workshop:7", "color", &json!("1 0 0")).unwrap();
        let preset = save_preset(&conn, "workshop:7", " Red ", IpcFitMode::Fill).unwrap();
        assert_eq!(preset.name, "Red");
        assert_eq!(preset.properties["color"], json!("1 0 0"));
        assert_eq!(preset.adjustments.fit_mode, IpcFitMode::Fill);

        set_user_property(&conn, "workshop:7", "color", &json!("0 0 1")).unwrap();
        apply_preset(&conn, "workshop:7", &preset.preset_id).unwrap();
        let row = get_wallpaper(&conn, "workshop:7").unwrap().unwrap();
        assert_eq!(row.user_properties()["color"], json!("1 0 0"));
        assert!(apply_preset(&conn, "workshop:7", "nope").is_err());
        assert!(save_preset(&conn, "workshop:7", "  ", IpcFitMode::Fill).is_err());

        // Deleting the wallpaper removes presets and its monitor assignments.
        set_monitor_source(
            &conn,
            "A",
            &WallpaperSource::Workshop {
                wallpaper_id: "workshop:7".into(),
            },
        )
        .unwrap();
        assert!(delete_wallpaper(&conn, "workshop:7").unwrap().is_some());
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM wallpaper_presets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(monitor_sources(&conn).unwrap().is_empty());
    }

    #[test]
    fn playlists_round_trip() {
        let conn = test_db();
        let saved = save_playlist(
            &conn,
            Playlist {
                playlist_id: String::new(),
                name: " Evening ".into(),
                mode: PlaylistMode::Mixed,
                interval_seconds: 1,
                items: vec![
                    PlaylistItem::PhotoApiDraw,
                    PlaylistItem::Workshop {
                        wallpaper_id: "workshop:1".into(),
                    },
                ],
            },
        )
        .unwrap();
        assert!(saved.playlist_id.starts_with("playlist-"));
        assert_eq!(saved.interval_seconds, MIN_PLAYLIST_INTERVAL_SECONDS);
        let mut renamed = saved.clone();
        renamed.name = "Night".into();
        renamed.mode = PlaylistMode::PhotosOnly;
        save_playlist(&conn, renamed).unwrap();
        let all = list_playlists(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Night");
        assert_eq!(all[0].mode, PlaylistMode::PhotosOnly);
        assert_eq!(all[0].items, saved.items);
        assert!(get_playlist(&conn, &saved.playlist_id).unwrap().is_some());
        assert!(save_playlist(
            &conn,
            Playlist {
                playlist_id: String::new(),
                name: "".into(),
                mode: PlaylistMode::Mixed,
                interval_seconds: 60,
                items: vec![]
            }
        )
        .is_err());
    }

    #[test]
    fn monitor_source_keeps_photo_only_for_photo_sources() {
        let conn = test_db();
        truongpaper_core::assign_photo_to_monitor(&conn, "A", "p1").unwrap();
        set_monitor_source(&conn, "A", &WallpaperSource::PhotoApi).unwrap();
        assert_eq!(
            truongpaper_core::list_displayed_photo_ids(&conn).unwrap(),
            vec!["p1".to_string()]
        );
        set_monitor_source(
            &conn,
            "A",
            &WallpaperSource::Playlist {
                playlist_id: "x".into(),
            },
        )
        .unwrap();
        assert!(truongpaper_core::list_displayed_photo_ids(&conn)
            .unwrap()
            .is_empty());
        assert_eq!(
            monitor_sources(&conn).unwrap()["A"],
            WallpaperSource::Playlist {
                playlist_id: "x".into()
            }
        );
    }
}
