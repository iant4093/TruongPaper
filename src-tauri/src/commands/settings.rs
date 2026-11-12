//! `settings_get` / `settings_update` (docs/04 §5.1). Settings are persisted in the SQLite
//! `settings` table; the photo pipeline keys are mirrored so `photo_*` commands see them.

use crate::state::AppState;
use crate::state::{merge_settings_patch, overlay_photo_keys, persist_settings, sanitize_settings};
use tauri::{AppHandle, Emitter, State};
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
        // Registered first: a shortcut that is invalid or taken rejects the whole update.
        if before.next_hotkey != after.next_hotkey || before.pause_hotkey != after.pause_hotkey {
            crate::controls::apply_hotkeys(&app, &after.next_hotkey, &after.pause_hotkey)?;
        }
        {
            let conn = state.db.lock();
            persist_settings(&conn, &after).map_err(|e| e.to_string())?;
        }
        *guard = after.clone();
        (before, after)
    };

    if before.default_fit_mode != after.default_fit_mode
        || before.transition_type != after.transition_type
    {
        state.host.update_photo_defaults(
            crate::models::IpcFitMode::from_setting(&after.default_fit_mode),
            crate::models::IpcTransitionType::from_setting(&after.transition_type),
        );
    }

    if before.launch_at_login != after.launch_at_login {
        apply_launch_at_login(&app, after.launch_at_login);
    }
    #[cfg(target_os = "windows")]
    if before.sync_lock_screen != after.sync_lock_screen {
        super::lock_screen::request_sync(&app);
    }
    let _ = app.emit(crate::models::EVENT_SETTINGS_CHANGED, after);
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
