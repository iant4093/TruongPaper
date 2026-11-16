//! "Adjust Windows color" (Wallpaper Engine parity): the wallpaper on the primary display
//! sets the Windows accent color from its `schemecolor` property, on apply and whenever the
//! user changes that property. Settings > Display > "Match Windows accent color" turns it off.
//! Other platforms have no settable accent color, so this does nothing there.

use crate::models::WallpaperItem;
use serde_json::Value;

/// User property Wallpaper Engine reads the scheme color from.
pub const SCHEME_COLOR_KEY: &str = "schemecolor";

/// The wallpaper's scheme color (the user's value, else the `project.json` default).
pub fn scheme_color(item: &WallpaperItem) -> Option<[u8; 3]> {
    let props = crate::commands::monitor::effective_user_properties(
        &item.project_json,
        &item.user_properties,
    );
    match props.get(SCHEME_COLOR_KEY)? {
        Value::String(raw) => truongpaper_platform::accent::parse_we_color(raw),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::scheme_color;
    use crate::commands::monitor::live_monitors;
    use crate::models::WallpaperItem;
    use crate::state::AppState;
    use parking_lot::Mutex;
    use tauri::{AppHandle, Manager};

    /// Last color written, so re-applying the same wallpaper does not rebroadcast.
    static LAST: Mutex<Option<[u8; 3]>> = Mutex::new(None);

    fn enabled(state: &AppState) -> bool {
        state.settings.lock().adjust_windows_color
    }

    fn apply(rgb: [u8; 3]) {
        {
            let mut last = LAST.lock();
            if *last == Some(rgb) {
                return;
            }
            *last = Some(rgb);
        }
        // The broadcast can wait on slow windows: keep it off the async runtime.
        let spawned = std::thread::Builder::new()
            .name("truongpaper-accent".into())
            .spawn(move || {
                if let Err(err) = truongpaper_platform::win_accent::set_accent_color(rgb) {
                    tracing::warn!("could not set the Windows accent color: {err}");
                    *LAST.lock() = None;
                }
            });
        if let Err(err) = spawned {
            tracing::warn!("accent color thread: {err}");
        }
    }

    fn is_primary(app: &AppHandle, monitor_id: &str) -> bool {
        live_monitors(app)
            .iter()
            .any(|m| m.is_primary && m.id == monitor_id)
    }

    /// `item` was just applied to `monitor_id`.
    pub fn wallpaper_applied(app: &AppHandle, monitor_id: &str, item: &WallpaperItem) {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if !enabled(&state) || !is_primary(app, monitor_id) {
            return;
        }
        if let Some(rgb) = scheme_color(item) {
            apply(rgb);
        }
    }

    /// Re-reads the primary display's wallpaper (after a property or settings change).
    pub fn refresh(app: &AppHandle) {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if !enabled(&state) {
            *LAST.lock() = None;
            return;
        }
        let Some(primary) = live_monitors(app).into_iter().find(|m| m.is_primary) else {
            return;
        };
        let Some(wallpaper_id) = state
            .host
            .monitor(&primary.id)
            .and_then(|rt| rt.wallpaper_id)
        else {
            return;
        };
        let row = crate::db::get_wallpaper(&state.db.lock(), &wallpaper_id);
        let fit = crate::models::IpcFitMode::from_setting(&state.settings.lock().default_fit_mode);
        if let Some(rgb) = row
            .ok()
            .flatten()
            .and_then(|row| row.to_item(fit))
            .as_ref()
            .and_then(scheme_color)
        {
            apply(rgb);
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use crate::models::WallpaperItem;
    use tauri::AppHandle;

    pub fn wallpaper_applied(_app: &AppHandle, _monitor_id: &str, _item: &WallpaperItem) {}

    pub fn refresh(_app: &AppHandle) {}
}

pub use imp::{refresh, wallpaper_applied};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UniversalAdjustments;
    use serde_json::{json, Map};

    fn item(project: Value, user: Map<String, Value>) -> WallpaperItem {
        WallpaperItem {
            wallpaper_id: "w".into(),
            workshop_id: None,
            title: "t".into(),
            wallpaper_type: "scene".into(),
            preview_path: String::new(),
            install_dir: String::new(),
            bytes: 0,
            audio_reactive: false,
            windows_only: false,
            project_json: project,
            user_properties: user,
            adjustments: UniversalAdjustments::default(),
            is_favorite: false,
        }
    }

    #[test]
    fn scheme_color_prefers_the_user_value() {
        let project = json!({"general": {"properties": {
            "schemecolor": {"type": "color", "value": "0 0 1"}
        }}});
        assert_eq!(
            scheme_color(&item(project.clone(), Map::new())),
            Some([0, 0, 255])
        );
        let mut user = Map::new();
        user.insert("schemecolor".into(), json!("1 0 0"));
        assert_eq!(scheme_color(&item(project, user)), Some([255, 0, 0]));
        assert_eq!(scheme_color(&item(json!({}), Map::new())), None);
    }
}
