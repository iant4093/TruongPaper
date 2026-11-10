//! `settings_get` / `settings_update` (docs/04 §5.1). Settings are persisted in the SQLite
//! `settings` table; the photo pipeline keys are mirrored so `photo_*` commands see them.

use crate::state::AppState;
use crate::state::{merge_settings_patch, overlay_photo_keys, persist_settings, sanitize_settings};
use tauri::{AppHandle, State};
use truongpaper_core::AppSettings;

#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> Result<AppSettings, String> {
    let mut s = state.settings.lock().clone();
    {
        let conn = state.db.lock();
        overlay_photo_keys(&conn, &mut s);
    }
    Ok(s)
}

#[tauri::command]
pub async fn settings_update(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: serde_json::Value,
) -> Result<(), String> {
    let (before, after) = {
        let mut guard = state.settings.lock();
        let before = guard.clone();
        let after = sanitize_settings(merge_settings_patch(&before, &settings)?);
        {
            let conn = state.db.lock();
            persist_settings(&conn, &after).map_err(|e| e.to_string())?;
        }
        *guard = after.clone();
        (before, after)
    };

    if before.launch_at_login != after.launch_at_login {
        apply_launch_at_login(&app, after.launch_at_login);
    }
    Ok(())
}

fn apply_launch_at_login(app: &AppHandle, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    let res = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    if let Err(err) = res {
        tracing::warn!("launch-at-login update failed: {err}");
    }
}
