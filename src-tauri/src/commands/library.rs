//! Wallpaper library & preset commands: `library_list_wallpapers`, `library_delete_wallpaper`,
//! `wallpaper_save_preset`, `wallpaper_apply_preset` (docs/04 §5.1).

use crate::db;
use crate::models::{IpcFitMode, IpcTransitionType, WallpaperItem, WallpaperPreset};
use crate::state::AppState;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

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
    tokio::task::spawn_blocking(move || rows.iter().filter_map(|r| r.to_item(fit)).collect())
        .await
        .map_err(|e| e.to_string())
}

/// `true` when `child` resolves to a path strictly inside `parent` (symlinks resolved).
pub fn is_strictly_within(child: &Path, parent: &Path) -> bool {
    match (child.canonicalize(), parent.canonicalize()) {
        (Ok(c), Ok(p)) => c != p && c.starts_with(&p),
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
        if preview.is_file() && is_strictly_within(preview, Path::new(&item.install_dir)) {
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

/// Removes the wallpaper from the library. Files are deleted only for items TruongPaper
/// downloaded itself (`<app_data>/wallpapers/workshop/<id>`); folders owned by a local Steam
/// install are left for Steam to manage.
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
    let workshop_root = state.workshop_dir();
    tokio::task::spawn_blocking(move || {
        if is_strictly_within(&install_dir, &workshop_root) {
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
}
