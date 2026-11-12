//! Wallpaper library & preset commands: `library_list_wallpapers`, `library_delete_wallpaper`,
//! `wallpaper_save_preset`, `wallpaper_apply_preset` (docs/04 §5.1).

use crate::db::{self, WallpaperRow};
use crate::models::{IpcFitMode, IpcTransitionType, WallpaperItem, WallpaperPreset};
use crate::state::AppState;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tauri::{AppHandle, Manager, State};

/// Parsed `project.json` per wallpaper folder, validated by the file's mtime and size, so
/// `library_list_wallpapers` stops re-reading and re-parsing every file on each call. Entries
/// are dropped explicitly on import, delete, download and sync; the stat key also catches
/// edits made outside the app (Steam updates, the Workshop watcher).
#[derive(Default)]
pub struct LibraryCache {
    projects: Mutex<HashMap<PathBuf, CachedProject>>,
    /// Preview files already granted to the asset protocol scope.
    allowed_previews: Mutex<HashSet<PathBuf>>,
}

struct CachedProject {
    modified: Option<SystemTime>,
    len: u64,
    value: Value,
}

impl LibraryCache {
    /// Parsed `project.json` of `install_dir`, from the cache while the file is unchanged.
    pub fn project_json(&self, install_dir: &Path) -> Option<Value> {
        let path = install_dir.join("project.json");
        let meta = std::fs::metadata(&path).ok()?;
        let (modified, len) = (meta.modified().ok(), meta.len());
        if let Some(hit) = self.projects.lock().get(install_dir) {
            if hit.modified == modified && hit.len == len {
                return Some(hit.value.clone());
            }
        }
        let value = db::read_project_json(install_dir)?;
        self.projects.lock().insert(
            install_dir.to_path_buf(),
            CachedProject {
                modified,
                len,
                value: value.clone(),
            },
        );
        Some(value)
    }

    /// Forgets one wallpaper folder (import, delete, download).
    pub fn invalidate(&self, install_dir: &Path) {
        self.projects.lock().remove(install_dir);
    }

    /// Forgets everything (library sync, filesystem watcher events).
    pub fn invalidate_all(&self) {
        self.projects.lock().clear();
    }

    /// Drops entries of folders that are no longer in the library.
    fn retain_dirs(&self, keep: &HashSet<&str>) {
        self.projects
            .lock()
            .retain(|dir, _| dir.to_str().is_some_and(|d| keep.contains(d)));
    }

    /// `true` the first time `preview` is seen, so scope grants happen once per file.
    fn mark_preview_allowed(&self, preview: &Path) -> bool {
        self.allowed_previews.lock().insert(preview.to_path_buf())
    }

    fn forget_preview(&self, preview: &Path) {
        self.allowed_previews.lock().remove(preview);
    }

    /// Same as `WallpaperRow::to_item`, with a cached `project.json`.
    pub fn item(&self, row: &WallpaperRow, default_fit: IpcFitMode) -> Option<WallpaperItem> {
        let project_json = self.project_json(Path::new(&row.install_dir))?;
        Some(WallpaperItem {
            wallpaper_id: row.wallpaper_id.clone(),
            workshop_id: row.workshop_id.clone(),
            title: row.title.clone(),
            wallpaper_type: row.wallpaper_type.clone(),
            preview_path: row.preview_path.clone().unwrap_or_default(),
            install_dir: row.install_dir.clone(),
            bytes: row.bytes,
            audio_reactive: row.audio_reactive,
            windows_only: !truongpaper_platform::is_wallpaper_type_supported_on_this_os(
                &row.wallpaper_type,
            ),
            project_json,
            user_properties: row.user_properties(),
            adjustments: row.adjustments(default_fit),
        })
    }
}

fn default_fit(state: &AppState) -> IpcFitMode {
    IpcFitMode::from_setting(&state.settings.lock().default_fit_mode)
}

/// Lists library rows whose folder still has a readable `project.json`.
pub async fn list_items(state: &AppState) -> Result<Vec<WallpaperItem>, String> {
    let rows = {
        let conn = state.db.lock();
        db::list_wallpapers(&conn)?
    };
    let fit = default_fit(state);
    let cache = state.library.clone();
    tokio::task::spawn_blocking(move || {
        let keep: HashSet<&str> = rows.iter().map(|r| r.install_dir.as_str()).collect();
        cache.retain_dirs(&keep);
        rows.iter().filter_map(|r| cache.item(r, fit)).collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// `<app_data>/wallpapers/local`: copies made by `library_import_local`.
pub fn local_dir(state: &AppState) -> PathBuf {
    state.app_data_dir.join("wallpapers").join("local")
}

/// Stores an imported item. It has no Workshop id, so `workshop_id` stays NULL.
pub fn upsert_local(
    conn: &rusqlite::Connection,
    item: &truongpaper_steam::local_sync::DiscoveredWorkshopWallpaper,
) -> Result<(), String> {
    db::upsert_discovered(conn, item)?;
    conn.execute(
        "UPDATE wallpaper_configs SET workshop_id = NULL WHERE wallpaper_id = ?1",
        [&item.wallpaper_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// `true` when `child` resolves to a path strictly inside `parent` (symlinks resolved).
pub fn is_strictly_within(child: &Path, parent: &Path) -> bool {
    match (child.canonicalize(), parent.canonicalize()) {
        (Ok(c), Ok(p)) => c != p && c.starts_with(&p),
        _ => false,
    }
}

/// `true` when `child` resolves (symlinks resolved) to an immediate subfolder of `parent`.
pub fn is_direct_child(child: &Path, parent: &Path) -> bool {
    match (child.canonicalize(), parent.canonicalize()) {
        (Ok(c), Ok(p)) => c.parent() == Some(p.as_path()),
        _ => false,
    }
}

#[tauri::command]
pub async fn library_list_wallpapers(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<WallpaperItem>, String> {
    let items = list_items(&state).await?;
    let scope = app.asset_protocol_scope();
    for item in &items {
        let preview = Path::new(&item.preview_path);
        // Local Steam installs live outside app data. Grant only a real preview image
        // within its wallpaper directory, never the whole Steam library or a symlink escape.
        if state.library.mark_preview_allowed(preview)
            && preview.is_file()
            && is_strictly_within(preview, Path::new(&item.install_dir))
        {
            if let Err(err) = scope.allow_file(preview) {
                tracing::warn!(
                    "cannot allow wallpaper preview {}: {err}",
                    preview.display()
                );
            }
        }
    }
    Ok(items)
}

/// Copies a Wallpaper Engine item folder (with `project.json`) or a video file
/// (.mp4/.webm/.mov/.mkv) into the app's local library and returns the new item (`local:<slug>`).
#[tauri::command]
pub async fn library_import_local(
    state: State<'_, AppState>,
    path: String,
) -> Result<WallpaperItem, String> {
    let root = local_dir(&state);
    let source = PathBuf::from(path);
    let found = tokio::task::spawn_blocking(move || {
        truongpaper_steam::local_import::import_local(&source, &root)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let fit = default_fit(&state);
    let row = {
        let conn = state.db.lock();
        upsert_local(&conn, &found)?;
        db::get_wallpaper(&conn, &found.wallpaper_id)?
    };
    state.library.invalidate(Path::new(&found.install_dir));
    row.and_then(|r| state.library.item(&r, fit))
        .ok_or_else(|| "The imported wallpaper could not be read".to_string())
}

/// Removes the wallpaper from the library. Files are deleted only for items TruongPaper
/// downloaded or imported itself (`<app_data>/wallpapers/workshop/<id>` or a direct child of
/// `<app_data>/wallpapers/local`); folders owned by a local Steam install are left for Steam.
#[tauri::command]
pub async fn library_delete_wallpaper(
    state: State<'_, AppState>,
    wallpaper_id: String,
) -> Result<(), String> {
    let row = {
        let conn = state.db.lock();
        db::delete_wallpaper(&conn, &wallpaper_id)?
    }
    .ok_or_else(|| format!("Wallpaper '{wallpaper_id}' is not in the library"))?;
    state.host.unmount_if(&wallpaper_id);

    let install_dir = PathBuf::from(&row.install_dir);
    state.library.invalidate(&install_dir);
    if let Some(preview) = &row.preview_path {
        state.library.forget_preview(Path::new(preview));
    }
    let workshop_root = state.workshop_dir();
    let local_root = local_dir(&state);
    tokio::task::spawn_blocking(move || {
        if is_strictly_within(&install_dir, &workshop_root)
            || is_direct_child(&install_dir, &local_root)
        {
            std::fs::remove_dir_all(&install_dir)
                .map_err(|e| format!("Removed from library but deleting files failed: {e}"))
        } else {
            Ok(())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn wallpaper_save_preset(
    state: State<'_, AppState>,
    wallpaper_id: String,
    preset_name: String,
) -> Result<WallpaperPreset, String> {
    let fit = default_fit(&state);
    let conn = state.db.lock();
    db::save_preset(&conn, &wallpaper_id, &preset_name, fit)
}

#[tauri::command]
pub async fn wallpaper_apply_preset(
    state: State<'_, AppState>,
    wallpaper_id: String,
    preset_id: String,
) -> Result<(), String> {
    let fit = default_fit(&state);
    let row = {
        let conn = state.db.lock();
        db::apply_preset(&conn, &wallpaper_id, &preset_id)?;
        db::get_wallpaper(&conn, &wallpaper_id)?
    };
    if let Some(row) = row {
        let transition = IpcTransitionType::from_setting(&state.settings.lock().transition_type);
        // Live wallpapers pick up the preset's adjustments and properties.
        crate::commands::monitor::forward_adjustments(
            &state,
            &wallpaper_id,
            &row.adjustments(fit),
            transition,
        );
        crate::commands::monitor::forward_properties(&state, &wallpaper_id, &row.user_properties());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project_dir(tag: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tp-libcache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.json"), body).unwrap();
        dir
    }

    #[test]
    fn project_json_is_cached_until_invalidated_or_changed() {
        let dir = project_dir("hit", r#"{"title":"a"}"#);
        let cache = LibraryCache::default();
        assert_eq!(cache.project_json(&dir), Some(json!({"title": "a"})));
        // Same size, preserved mtime: a hit does not touch the file contents.
        let path = dir.join("project.json");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, r#"{"title":"b"}"#).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_eq!(cache.project_json(&dir), Some(json!({"title": "a"})), "hit");
        // Explicit invalidation (import / delete / download) re-reads it.
        cache.invalidate(&dir);
        assert_eq!(cache.project_json(&dir), Some(json!({"title": "b"})));
        // A changed size is noticed without any invalidation.
        std::fs::write(&path, r#"{"title":"longer"}"#).unwrap();
        assert_eq!(cache.project_json(&dir), Some(json!({"title": "longer"})));
        // invalidate_all clears everything; a deleted file yields None.
        cache.invalidate_all();
        assert!(cache.projects.lock().is_empty());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(cache.project_json(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn retain_dirs_drops_removed_folders() {
        let (a, b) = (project_dir("ra", "{}"), project_dir("rb", "{}"));
        let cache = LibraryCache::default();
        cache.project_json(&a);
        cache.project_json(&b);
        cache.retain_dirs(&HashSet::from([a.to_str().unwrap()]));
        assert_eq!(cache.projects.lock().len(), 1);
        assert!(cache.mark_preview_allowed(&a));
        assert!(!cache.mark_preview_allowed(&a), "granted once");
        cache.forget_preview(&a);
        assert!(cache.mark_preview_allowed(&a));
        for d in [a, b] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn strictly_within_guards_deletion() {
        let root = std::env::temp_dir().join(format!("truongpaper-within-{}", std::process::id()));
        let child = root.join("123");
        std::fs::create_dir_all(&child).unwrap();
        assert!(is_strictly_within(&child, &root));
        assert!(!is_strictly_within(&root, &root));
        assert!(!is_strictly_within(&root.join("123/../.."), &root));
        assert!(!is_strictly_within(&std::env::temp_dir(), &root));
        assert!(!is_strictly_within(&root.join("missing"), &root));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn direct_child_guards_local_deletion() {
        let root = std::env::temp_dir().join(format!("truongpaper-direct-{}", std::process::id()));
        let item = root.join("clip-1");
        std::fs::create_dir_all(item.join("nested")).unwrap();
        assert!(is_direct_child(&item, &root));
        assert!(!is_direct_child(&item.join("nested"), &root));
        assert!(!is_direct_child(&root, &root));
        assert!(!is_direct_child(&root.join("clip-1/.."), &root));
        assert!(!is_direct_child(&root.join("missing"), &root));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
