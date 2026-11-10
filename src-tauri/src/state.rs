//! Managed application state: SQLite connection (`truongpaper.db` in `app_data_dir`), settings,
//! wallpaper host runtime and the Steam sidecar session.

use crate::commands::steam::SteamSession;
use crate::host::WallpaperHost;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use truongpaper_core::{get_setting, set_setting, AppSettings, CoreResult};

/// `settings` table key holding the serialized [`AppSettings`].
pub const SETTING_APP_SETTINGS: &str = "app_settings";
/// `settings` key read by `photo_rotate_next` (docs/02 §6).
pub const SETTING_SKIP_MISMATCHED_ORIENTATION: &str = "skip_mismatched_orientation";
/// `settings` key read by `photo_rotate_next`: `"independent"` (default) or `"clone"`.
pub const SETTING_MULTI_MONITOR_MODE: &str = "multi_monitor_mode";
/// Smallest accepted non-zero rotation interval (`0` = "Never (Manual Only)").
pub const MIN_ROTATION_INTERVAL_SECONDS: u64 = 10;

pub struct AppState {
    pub settings: Arc<Mutex<AppSettings>>,
    pub db: Arc<Mutex<Connection>>,
    pub app_data_dir: PathBuf,
    pub http: reqwest::Client,
    pub host: Arc<WallpaperHost>,
    pub steam: Arc<SteamSession>,
}

impl AppState {
    /// Builds the state around an already-initialized connection and loads persisted settings.
    pub fn new(conn: Connection, app_data_dir: PathBuf, http: reqwest::Client) -> Self {
        let settings = load_settings(&conn);
        Self {
            settings: Arc::new(Mutex::new(settings)),
            db: Arc::new(Mutex::new(conn)),
            app_data_dir,
            http,
            host: Arc::new(WallpaperHost::new()),
            steam: Arc::new(SteamSession::default()),
        }
    }

    /// `<app_data_dir>/wallpapers/workshop` (DepotDownloader output root).
    pub fn workshop_dir(&self) -> PathBuf {
        workshop_dir(&self.app_data_dir)
    }

    /// `<app_data_dir>/we_core_assets` (DepotDownloader core assets bootstrap target).
    pub fn core_assets_root(&self) -> PathBuf {
        self.app_data_dir.join("we_core_assets")
    }

    /// Terminates DepotDownloader sidecars and releases every wallpaper host resource so the
    /// updater can overwrite the binary (docs/06 §5.1).
    pub async fn shutdown_all_wallpaper_hosts(&self) {
        self.shutdown_now();
    }

    /// Stops DepotDownloader, closes the Web / Video wallpaper windows, and shuts the host down
    /// (the render thread, webview bridge and audio pushes exit; Application wallpaper
    /// processes are killed).
    pub fn shutdown_now(&self) {
        self.steam.abort_all();
        crate::wallpaper_webview::close_all();
        self.host.shutdown();
    }
}

pub fn workshop_dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("wallpapers").join("workshop")
}

/// Applies a partial camelCase settings object on top of `current` (unknown keys are ignored,
/// wrong value types are rejected).
pub fn merge_settings_patch(current: &AppSettings, patch: &Value) -> Result<AppSettings, String> {
    let patch_obj = patch
        .as_object()
        .ok_or_else(|| "settings must be an object".to_string())?;
    let mut merged = serde_json::to_value(current).map_err(|e| e.to_string())?;
    if let Some(cur_obj) = merged.as_object_mut() {
        for (k, v) in patch_obj {
            if cur_obj.contains_key(k) {
                cur_obj.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::from_value(merged).map_err(|e| format!("Invalid settings: {e}"))
}

/// Normalizes values the backend depends on.
pub fn sanitize_settings(mut s: AppSettings) -> AppSettings {
    // 0 is the UI's "Never (Manual Only)"; any other value is at least the minimum.
    if s.rotation_interval_seconds != 0 {
        s.rotation_interval_seconds = s
            .rotation_interval_seconds
            .max(MIN_ROTATION_INTERVAL_SECONDS);
    }
    s.transition_duration_seconds =
        truongpaper_renderer::clamp_transition_duration(s.transition_duration_seconds);
    if truongpaper_renderer::FitMode::from_setting_str(&s.default_fit_mode).is_none() {
        s.default_fit_mode = "fit_blur".into();
    }
    if truongpaper_renderer::TransitionMode::from_setting_str(&s.transition_type).is_none() {
        s.transition_type = "liquid_glass".into();
    }
    let fps_ok = s.fps_cap.as_str() == Some("display_native")
        || matches!(s.fps_cap.as_u64(), Some(30 | 60 | 120));
    if !fps_ok {
        s.fps_cap = serde_json::json!(60);
    }
    s
}

/// Loads [`AppSettings`] (stored blob merged over defaults), then overlays the photo keys that
/// `photo_*` commands write directly.
pub fn load_settings(conn: &Connection) -> AppSettings {
    let defaults = AppSettings::default();
    let mut s = match get_setting::<Value>(conn, SETTING_APP_SETTINGS) {
        Ok(Some(stored)) => merge_settings_patch(&defaults, &stored).unwrap_or(defaults),
        _ => defaults,
    };
    overlay_photo_keys(conn, &mut s);
    sanitize_settings(s)
}

/// Keys owned by the photo pipeline win over the settings blob.
pub fn overlay_photo_keys(conn: &Connection, s: &mut AppSettings) {
    if let Ok(Some(v)) = get_setting::<u64>(conn, truongpaper_core::SETTING_MAX_CACHE_BYTES) {
        s.max_photo_cache_bytes = v;
    }
    if let Ok(Some(v)) = get_setting::<bool>(conn, truongpaper_core::SETTING_EXHAUSTION_AUTO_RESET)
    {
        s.exhaustion_auto_reset = v;
    }
    if let Ok(Some(v)) = get_setting::<bool>(conn, SETTING_SKIP_MISMATCHED_ORIENTATION) {
        s.skip_mismatched_orientation = v;
    }
}

/// Persists the settings blob and mirrors the keys read by the photo pipeline.
pub fn persist_settings(conn: &Connection, s: &AppSettings) -> CoreResult<()> {
    set_setting(conn, SETTING_APP_SETTINGS, s)?;
    set_setting(
        conn,
        truongpaper_core::SETTING_MAX_CACHE_BYTES,
        &s.max_photo_cache_bytes,
    )?;
    set_setting(
        conn,
        truongpaper_core::SETTING_EXHAUSTION_AUTO_RESET,
        &s.exhaustion_auto_reset,
    )?;
    set_setting(
        conn,
        SETTING_SKIP_MISMATCHED_ORIENTATION,
        &s.skip_mismatched_orientation,
    )
}

#[cfg(test)]
pub(crate) fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(truongpaper_core::SQLITE_SCHEMA_SQL)
        .unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_applies_known_keys_and_rejects_bad_types() {
        let base = AppSettings::default();
        let merged = merge_settings_patch(
            &base,
            &json!({"rotationIntervalSeconds": 60, "pauseOnMaximized": true, "unknownKey": 1}),
        )
        .unwrap();
        assert_eq!(merged.rotation_interval_seconds, 60);
        assert!(merged.pause_on_maximized);
        assert_eq!(merged.transition_type, base.transition_type);
        assert!(merge_settings_patch(&base, &json!({"pauseOnMaximized": "yes"})).is_err());
        assert!(merge_settings_patch(&base, &json!([1, 2])).is_err());
    }

    #[test]
    fn sanitize_clamps_values() {
        let s = sanitize_settings(AppSettings {
            rotation_interval_seconds: 1,
            transition_duration_seconds: 99.0,
            default_fit_mode: "zoomy".into(),
            transition_type: "wipe".into(),
            fps_cap: json!(75),
            ..AppSettings::default()
        });
        assert_eq!(s.rotation_interval_seconds, MIN_ROTATION_INTERVAL_SECONDS);
        for (raw, expected) in [(9, 10), (10, 10), (60, 60), (86_400, 86_400)] {
            let s = sanitize_settings(AppSettings {
                rotation_interval_seconds: raw,
                ..AppSettings::default()
            });
            assert_eq!(s.rotation_interval_seconds, expected, "{raw}");
        }
        assert_eq!(s.transition_duration_seconds, 3.0);
        assert_eq!(s.default_fit_mode, "fit_blur");
        assert_eq!(s.transition_type, "liquid_glass");
        assert_eq!(s.fps_cap, json!(60));
        let native = sanitize_settings(AppSettings {
            fps_cap: json!("display_native"),
            ..AppSettings::default()
        });
        assert_eq!(native.fps_cap, json!("display_native"));
    }

    #[test]
    fn zero_rotation_interval_means_never() {
        let s = sanitize_settings(AppSettings {
            rotation_interval_seconds: 0,
            ..AppSettings::default()
        });
        assert_eq!(s.rotation_interval_seconds, 0);
        // Round trip through the UI patch + persistence keeps "Never (Manual Only)".
        let conn = test_db();
        let merged = merge_settings_patch(
            &AppSettings::default(),
            &json!({"rotationIntervalSeconds": 0}),
        )
        .unwrap();
        persist_settings(&conn, &sanitize_settings(merged)).unwrap();
        assert_eq!(load_settings(&conn).rotation_interval_seconds, 0);
    }

    #[test]
    fn settings_persist_and_photo_keys_overlay() {
        let conn = test_db();
        assert_eq!(
            load_settings(&conn).rotation_interval_seconds,
            AppSettings::default().rotation_interval_seconds
        );
        let s = AppSettings {
            rotation_interval_seconds: 120,
            skip_mismatched_orientation: false,
            ..AppSettings::default()
        };
        persist_settings(&conn, &s).unwrap();
        // photo_set_cache_limit writes the key directly; it must win over the blob.
        truongpaper_core::save_max_cache_bytes(&conn, 1234).unwrap();
        let loaded = load_settings(&conn);
        assert_eq!(loaded.rotation_interval_seconds, 120);
        assert!(!loaded.skip_mismatched_orientation);
        assert_eq!(loaded.max_photo_cache_bytes, 1234);
        assert_eq!(
            get_setting::<bool>(&conn, SETTING_SKIP_MISMATCHED_ORIENTATION).unwrap(),
            Some(false)
        );
    }
}
