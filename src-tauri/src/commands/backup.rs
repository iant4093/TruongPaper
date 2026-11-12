//! `backup_export` / `backup_import` (docs/04 §5.1): one JSON file with the user's settings,
//! playlists, favorites and per-wallpaper customization. Wallpaper files, Steam account data and
//! secrets are never part of it.

use crate::db;
use crate::models::{IpcFitMode, Playlist, UniversalAdjustments, WallpaperPreset, WallpaperSource};
use crate::state::AppState;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};
use truongpaper_core::{now_ms, SavedAlbumRow, SavedAlbumUpsert};

pub const BACKUP_FORMAT: &str = "truongpaper-backup";
pub const BACKUP_VERSION: u32 = 1;
/// Largest backup file `backup_import` reads.
pub const MAX_BACKUP_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub settings: bool,
    pub playlists: usize,
    pub favorites: usize,
    pub wallpapers: usize,
    pub missing_workshop_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryIndexEntry {
    pub wallpaper_id: String,
    pub title: String,
    #[serde(default)]
    pub workshop_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedAlbumBackup {
    pub album_slug: String,
    pub title: String,
    pub album_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub cover_thumbnail_url: Option<String>,
    #[serde(default)]
    pub image_count: Option<u32>,
    #[serde(default = "default_true")]
    pub is_enabled: bool,
}

fn default_true() -> bool {
    true
}

/// The backup document. Every collection defaults to empty so older/partial files load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub settings: Option<Value>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
    #[serde(default)]
    pub monitor_sources: BTreeMap<String, WallpaperSource>,
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default)]
    pub saved_albums: Vec<SavedAlbumBackup>,
    #[serde(default)]
    pub photo_favorites: Vec<String>,
    #[serde(default)]
    pub wallpaper_properties: BTreeMap<String, Map<String, Value>>,
    #[serde(default)]
    pub wallpaper_adjustments: BTreeMap<String, UniversalAdjustments>,
    #[serde(default)]
    pub wallpaper_presets: BTreeMap<String, Vec<WallpaperPreset>>,
    #[serde(default)]
    pub library_index: Vec<LibraryIndexEntry>,
}

fn album_backup(row: &SavedAlbumRow) -> SavedAlbumBackup {
    SavedAlbumBackup {
        album_slug: row.album_slug.clone(),
        title: row.title.clone(),
        album_url: row.album_url.clone(),
        description: row.description.clone(),
        category: row.category.clone(),
        cover_thumbnail_url: row.cover_thumbnail_url.clone(),
        image_count: row.image_count,
        is_enabled: row.is_enabled,
    }
}

fn s(err: impl std::fmt::Display) -> String {
    err.to_string()
}

/// Builds the backup from the database and the current settings (as JSON).
pub fn build_backup(conn: &Connection, settings: Value) -> Result<Backup, String> {
    let rows = db::list_wallpapers(conn)?;
    let mut backup = Backup {
        format: BACKUP_FORMAT.into(),
        version: BACKUP_VERSION,
        created_at: now_ms(),
        settings: Some(settings),
        playlists: db::list_playlists(conn)?,
        monitor_sources: db::monitor_sources(conn)?.into_iter().collect(),
        favorites: db::list_wallpaper_favorites(conn)?,
        saved_albums: truongpaper_core::list_saved_albums(conn)
            .map_err(s)?
            .iter()
            .map(album_backup)
            .collect(),
        photo_favorites: Vec::new(),
        wallpaper_properties: BTreeMap::new(),
        wallpaper_adjustments: BTreeMap::new(),
        wallpaper_presets: BTreeMap::new(),
        library_index: Vec::new(),
    };
    let mut stmt = conn
        .prepare("SELECT id FROM photos_cache WHERE is_favorite = 1 ORDER BY id")
        .map_err(s)?;
    backup.photo_favorites = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(s)?
        .collect::<Result<_, _>>()
        .map_err(s)?;
    for row in &rows {
        let id = row.wallpaper_id.clone();
        backup.library_index.push(LibraryIndexEntry {
            wallpaper_id: id.clone(),
            title: row.title.clone(),
            workshop_id: row.workshop_id.clone(),
        });
        let props = row.user_properties();
        if !props.is_empty() {
            backup.wallpaper_properties.insert(id.clone(), props);
        }
        let stored: Value = serde_json::from_str(&row.adjustments_json).unwrap_or(Value::Null);
        if stored.as_object().is_some_and(|o| !o.is_empty()) {
            backup
                .wallpaper_adjustments
                .insert(id.clone(), row.adjustments(IpcFitMode::Fill));
        }
        let presets = list_presets(conn, &id)?;
        if !presets.is_empty() {
            backup.wallpaper_presets.insert(id, presets);
        }
    }
    Ok(backup)
}

fn list_presets(conn: &Connection, wallpaper_id: &str) -> Result<Vec<WallpaperPreset>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT preset_id, preset_name, user_properties_json, adjustments_json
             FROM wallpaper_presets WHERE wallpaper_id = ?1 ORDER BY created_at, preset_id",
        )
        .map_err(s)?;
    let rows = stmt
        .query_map([wallpaper_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .map_err(s)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(s)?;
    Ok(rows
        .into_iter()
        .map(|(preset_id, name, props, adj)| WallpaperPreset {
            preset_id,
            wallpaper_id: wallpaper_id.to_string(),
            name,
            properties: serde_json::from_str(&props).unwrap_or_default(),
            adjustments: UniversalAdjustments::from_stored_json(&adj, IpcFitMode::Fill),
        })
        .collect())
}

/// Parses and validates backup text (size cap, format, version).
pub fn parse_backup(text: &str) -> Result<Backup, String> {
    if text.len() as u64 > MAX_BACKUP_BYTES {
        return Err("Backup file is larger than 10 MB".into());
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("Not a TruongPaper backup: {e}"))?;
    if value.get("format").and_then(Value::as_str) != Some(BACKUP_FORMAT) {
        return Err("Not a TruongPaper backup (unknown format)".into());
    }
    match value.get("version").and_then(Value::as_u64) {
        Some(v) if v == u64::from(BACKUP_VERSION) => {}
        Some(v) => return Err(format!("Unsupported backup version {v}")),
        None => return Err("Backup has no version".into()),
    }
    serde_json::from_value(value).map_err(|e| format!("Invalid backup: {e}"))
}

/// Monitor sources stored by an import, so the caller can refresh what is shown.
pub type AppliedSources = Vec<(String, WallpaperSource)>;

/// Applies everything except settings. Returns the summary (`settings = false`) and the monitor
/// sources that were stored.
pub fn apply_backup(
    conn: &Connection,
    backup: &Backup,
) -> Result<(BackupSummary, AppliedSources), String> {
    let existing: BTreeSet<String> = db::list_wallpapers(conn)?
        .into_iter()
        .map(|r| r.wallpaper_id)
        .collect();
    let mut summary = BackupSummary::default();

    for p in &backup.playlists {
        match db::save_playlist(conn, p.clone()) {
            Ok(_) => summary.playlists += 1,
            Err(err) => tracing::warn!("backup import: playlist skipped: {err}"),
        }
    }
    for id in &backup.favorites {
        if existing.contains(id) {
            db::set_wallpaper_favorite(conn, id, true)?;
            summary.favorites += 1;
        }
    }
    let mut touched = BTreeSet::new();
    for (id, props) in &backup.wallpaper_properties {
        if existing.contains(id) {
            conn.execute(
                "UPDATE wallpaper_configs SET user_properties_json = ?1, updated_at = ?2
                 WHERE wallpaper_id = ?3",
                params![serde_json::to_string(props).map_err(s)?, now_ms(), id],
            )
            .map_err(s)?;
            touched.insert(id.clone());
        }
    }
    for (id, adj) in &backup.wallpaper_adjustments {
        if existing.contains(id) {
            db::set_adjustments(conn, id, &adj.sanitized())?;
            touched.insert(id.clone());
        }
    }
    for (id, presets) in &backup.wallpaper_presets {
        if !existing.contains(id) {
            continue;
        }
        for p in presets {
            let name = p.name.trim();
            if name.is_empty() || name.len() > 128 || p.preset_id.is_empty() {
                continue;
            }
            conn.execute(
                "INSERT OR IGNORE INTO wallpaper_presets
                    (preset_id, wallpaper_id, preset_name, user_properties_json,
                     adjustments_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    p.preset_id,
                    id,
                    name,
                    serde_json::to_string(&p.properties).map_err(s)?,
                    serde_json::to_string(&p.adjustments.sanitized()).map_err(s)?,
                    now_ms(),
                ],
            )
            .map_err(s)?;
        }
    }
    summary.wallpapers = touched.len();

    let known_playlists: BTreeSet<String> = db::list_playlists(conn)?
        .into_iter()
        .map(|p| p.playlist_id)
        .collect();
    let mut applied_sources = Vec::new();
    for (monitor, source) in &backup.monitor_sources {
        let ok = match source {
            WallpaperSource::PhotoApi => true,
            WallpaperSource::Workshop { wallpaper_id } => existing.contains(wallpaper_id),
            WallpaperSource::Playlist { playlist_id } => known_playlists.contains(playlist_id),
        };
        if ok && !monitor.trim().is_empty() && monitor.len() <= 256 {
            db::set_monitor_source(conn, monitor, source)?;
            applied_sources.push((monitor.clone(), source.clone()));
        }
    }
    for album in &backup.saved_albums {
        if album.album_slug.is_empty() || album.album_url.is_empty() {
            continue;
        }
        truongpaper_core::upsert_saved_album(
            conn,
            &SavedAlbumUpsert {
                album_slug: album.album_slug.clone(),
                title: album.title.clone(),
                description: album.description.clone(),
                category: album.category.clone(),
                album_url: album.album_url.clone(),
                cover_thumbnail_url: album.cover_thumbnail_url.clone(),
                cover_blurhash: None,
                image_count: album.image_count,
                added_at: now_ms(),
            },
        )
        .map_err(s)?;
        let _ = truongpaper_core::set_album_enabled(conn, &album.album_slug, album.is_enabled);
    }
    for id in &backup.photo_favorites {
        let _ = truongpaper_core::set_favorite(conn, id, true);
    }

    let missing: BTreeSet<String> = backup
        .library_index
        .iter()
        .filter(|entry| !existing.contains(&entry.wallpaper_id))
        .filter_map(|entry| entry.workshop_id.clone())
        .filter(|id| !id.trim().is_empty())
        .collect();
    summary.missing_workshop_ids = missing.into_iter().collect();
    Ok((summary, applied_sources))
}

/// Snapshot of the live database next to it before an import rewrites it.
fn backup_database(conn: &Connection, dir: &Path) -> Result<PathBuf, String> {
    let target = dir.join(format!("truongpaper.db.pre-import-{}.bak", now_ms()));
    let _ = std::fs::create_dir_all(dir);
    conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])
        .map_err(|e| format!("Could not back up the database before import: {e}"))?;
    Ok(target)
}

#[tauri::command]
pub async fn backup_export(
    state: State<'_, AppState>,
    path: String,
) -> Result<BackupSummary, String> {
    if path.trim().is_empty() {
        return Err("Choose a file for the backup".into());
    }
    let settings = serde_json::to_value(&*state.settings.lock()).map_err(s)?;
    let backup = {
        let conn = state.db.lock();
        build_backup(&conn, settings)?
    };
    let text = serde_json::to_string_pretty(&backup).map_err(s)?;
    let target = PathBuf::from(path);
    let summary = BackupSummary {
        settings: true,
        playlists: backup.playlists.len(),
        favorites: backup.favorites.len(),
        wallpapers: backup.library_index.len(),
        missing_workshop_ids: Vec::new(),
    };
    tokio::task::spawn_blocking(move || {
        std::fs::write(&target, text)
            .map_err(|e| format!("Could not write {}: {e}", target.display()))
    })
    .await
    .map_err(s)??;
    Ok(summary)
}

#[tauri::command]
pub async fn backup_import(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<BackupSummary, String> {
    let file = PathBuf::from(path);
    let text = tokio::task::spawn_blocking(move || {
        let len = std::fs::metadata(&file)
            .map_err(|e| format!("Could not read {}: {e}", file.display()))?
            .len();
        if len > MAX_BACKUP_BYTES {
            return Err("Backup file is larger than 10 MB".to_string());
        }
        std::fs::read_to_string(&file).map_err(|e| format!("Could not read the backup: {e}"))
    })
    .await
    .map_err(s)??;
    let backup = parse_backup(&text)?;

    let (mut summary, sources) = {
        let conn = state.db.lock();
        backup_database(&conn, &state.app_data_dir)?;
        apply_backup(&conn, &backup)?
    };
    crate::scheduler::invalidate_playlist_cache();

    // Settings go through the same merge + sanitize + persist path as `settings_update`; a
    // hotkey that is invalid or taken here must not discard the other settings.
    if let Some(Value::Object(settings)) = backup.settings.clone() {
        let mut applied = crate::commands::settings::settings_update(
            app.clone(),
            app.state(),
            Value::Object(settings.clone()),
        )
        .await;
        if applied.is_err() {
            let mut without_hotkeys = settings;
            without_hotkeys.remove("nextHotkey");
            without_hotkeys.remove("pauseHotkey");
            applied = crate::commands::settings::settings_update(
                app.clone(),
                app.state(),
                Value::Object(without_hotkeys),
            )
            .await;
        }
        match applied {
            Ok(()) => summary.settings = true,
            Err(err) => tracing::warn!("backup import: settings not applied: {err}"),
        }
    }

    // Refresh connected displays whose source came from the backup.
    if let Some(photo) = app.try_state::<crate::commands::photo::PhotoServiceState>() {
        for (monitor_id, source) in sources {
            let res = {
                let _apply = crate::commands::monitor::lock_monitor_apply(&monitor_id).await;
                crate::commands::monitor::apply_source(&app, &state, &photo, &monitor_id, &source)
                    .await
            };
            if let Err(err) = res {
                tracing::debug!("backup import: {monitor_id} not refreshed: {err}");
            }
            crate::scheduler::note_manual_rotation(Some(&monitor_id), true);
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{PlaylistItem, PlaylistMode};
    use crate::state::test_db;
    use serde_json::json;

    fn add_wallpaper(conn: &Connection, id: &str, workshop: Option<&str>) {
        conn.execute(
            "INSERT INTO wallpaper_configs (wallpaper_id, workshop_id, title, wallpaper_type,
                install_dir, updated_at) VALUES (?1, ?2, 'T', 'scene', '/x', 1)",
            params![id, workshop],
        )
        .unwrap();
    }

    fn playlist(id: &str) -> Playlist {
        Playlist {
            playlist_id: id.into(),
            name: "Mix".into(),
            mode: PlaylistMode::Mixed,
            interval_seconds: 60,
            items: vec![PlaylistItem::PhotoApiDraw],
            transition: None,
        }
    }

    #[test]
    fn round_trip_restores_user_state_and_reports_missing_items() {
        let src = test_db();
        add_wallpaper(&src, "workshop:1", Some("1"));
        add_wallpaper(&src, "workshop:2", Some("2"));
        db::set_wallpaper_favorite(&src, "workshop:1", true).unwrap();
        db::set_user_property(&src, "workshop:1", "speed", &json!(3)).unwrap();
        db::set_adjustments(
            &src,
            "workshop:1",
            &UniversalAdjustments {
                zoom: 1.5,
                ..Default::default()
            },
        )
        .unwrap();
        db::save_playlist(&src, playlist("pl1")).unwrap();
        db::set_monitor_source(
            &src,
            "M1",
            &WallpaperSource::Playlist {
                playlist_id: "pl1".into(),
            },
        )
        .unwrap();
        let backup = build_backup(&src, json!({"rotationIntervalSeconds": 120})).unwrap();
        let text = serde_json::to_string(&backup).unwrap();
        assert!(!text.to_lowercase().contains("password"));
        let parsed = parse_backup(&text).unwrap();
        assert_eq!(parsed.library_index.len(), 2);

        // Destination only has workshop:1.
        let dst = test_db();
        add_wallpaper(&dst, "workshop:1", Some("1"));
        let (summary, sources) = apply_backup(&dst, &parsed).unwrap();
        assert_eq!(summary.playlists, 1);
        assert_eq!(summary.favorites, 1);
        assert_eq!(summary.wallpapers, 1);
        assert_eq!(summary.missing_workshop_ids, vec!["2".to_string()]);
        assert_eq!(sources.len(), 1);
        let row = db::get_wallpaper(&dst, "workshop:1").unwrap().unwrap();
        assert!(row.is_favorite);
        assert_eq!(row.user_properties()["speed"], json!(3));
        assert!((row.adjustments(IpcFitMode::Fill).zoom - 1.5).abs() < 1e-6);
        assert_eq!(db::list_playlists(&dst).unwrap().len(), 1);
        assert!(db::monitor_sources(&dst).unwrap().contains_key("M1"));
    }

    #[test]
    fn monitor_sources_for_missing_wallpapers_are_skipped() {
        let dst = test_db();
        let mut b = parse_backup(r#"{"format":"truongpaper-backup","version":1}"#).unwrap();
        b.monitor_sources.insert(
            "M".into(),
            WallpaperSource::Workshop {
                wallpaper_id: "workshop:9".into(),
            },
        );
        let (_, sources) = apply_backup(&dst, &b).unwrap();
        assert!(sources.is_empty());
        assert!(db::monitor_sources(&dst).unwrap().is_empty());
    }

    #[test]
    fn import_rejects_bad_format_version_and_oversize() {
        assert!(parse_backup("not json").is_err());
        assert!(parse_backup(r#"{"format":"other","version":1}"#).is_err());
        assert!(parse_backup(r#"{"format":"truongpaper-backup","version":2}"#).is_err());
        assert!(parse_backup(r#"{"format":"truongpaper-backup"}"#).is_err());
        let big = format!(
            r#"{{"format":"truongpaper-backup","version":1,"pad":"{}"}}"#,
            "a".repeat(MAX_BACKUP_BYTES as usize)
        );
        assert!(parse_backup(&big).unwrap_err().contains("10 MB"));
        assert!(parse_backup(r#"{"format":"truongpaper-backup","version":1}"#).is_ok());
    }

    #[test]
    fn database_snapshot_is_written_before_import() {
        let conn = test_db();
        add_wallpaper(&conn, "workshop:1", None);
        let dir = std::env::temp_dir().join(format!("truongpaper-bk-{}", now_ms()));
        let file = backup_database(&conn, &dir).unwrap();
        assert!(file.is_file());
        let _ = std::fs::remove_dir_all(dir);
    }
}
