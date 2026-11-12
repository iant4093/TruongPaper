//! User controls that work without the Studio window: global hotkeys, the tray's Next / Pause
//! actions (the same Rust handlers), and persistence of the user pause and display volumes.
//!
//! - Hotkeys are `AppSettings::next_hotkey` / `pause_hotkey` (Tauri accelerator strings, empty =
//!   disabled), registered with `tauri-plugin-global-shortcut` and handled here, so they work
//!   while the Studio webview is closed.
//! - `settings` keys: [`SETTING_USER_PAUSE`] (`{ global, monitors }`) and
//!   [`SETTING_MONITOR_VOLUMES`] (`{ monitorId: 0..1 }`) are restored at startup before the
//!   first frame.

use crate::commands::monitor::{apply_user_pause, live_monitors};
use crate::commands::photo::{rotate_photo_next, PhotoServiceState};
use crate::host::WallpaperHost;
use crate::models::WallpaperSource;
use crate::state::AppState;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tauri::{AppHandle, Listener, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use truongpaper_core::{get_setting, set_setting, CoreResult};

/// `settings` key of the persisted user pause.
pub const SETTING_USER_PAUSE: &str = "user_pause";
/// `settings` key of the persisted per-display volumes.
pub const SETTING_MONITOR_VOLUMES: &str = "monitor_volumes";

// ---------------------------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------------------------

/// Persisted user pause: the "pause all" flag plus the displays that differ from it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PauseState {
    pub global: bool,
    pub monitors: BTreeMap<String, bool>,
}

pub fn load_pause(conn: &Connection) -> PauseState {
    get_setting::<PauseState>(conn, SETTING_USER_PAUSE)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Records a user pause change (`monitor_id = None` = all displays).
pub fn persist_pause(conn: &Connection, monitor_id: Option<&str>, paused: bool) -> CoreResult<()> {
    let mut state = load_pause(conn);
    match monitor_id {
        None => {
            state.global = paused;
            state.monitors.clear();
        }
        Some(id) if paused == state.global => {
            state.monitors.remove(id);
        }
        Some(id) => {
            state.monitors.insert(id.to_string(), paused);
        }
    }
    set_setting(conn, SETTING_USER_PAUSE, &state)
}

pub fn load_monitor_volumes(conn: &Connection) -> BTreeMap<String, f32> {
    get_setting::<BTreeMap<String, f32>>(conn, SETTING_MONITOR_VOLUMES)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn persist_monitor_volume(
    conn: &Connection,
    monitor_id: &str,
    volume: f32,
) -> Result<(), String> {
    let mut volumes = load_monitor_volumes(conn);
    if (volume - 1.0).abs() < f32::EPSILON {
        volumes.remove(monitor_id);
    } else {
        volumes.insert(monitor_id.to_string(), volume);
    }
    set_setting(conn, SETTING_MONITOR_VOLUMES, &volumes).map_err(|e| e.to_string())
}

/// `volume` must be a finite number in `0..=1`.
pub fn validate_volume(volume: f32) -> Result<f32, String> {
    if volume.is_finite() && (0.0..=1.0).contains(&volume) {
        Ok(volume)
    } else {
        Err("Volume must be between 0 and 1".into())
    }
}

/// Applies the persisted pause and display volumes to the host. Called once during setup,
/// before the render thread and the webview bridge start, so the first frame already honors them.
pub fn restore_persisted(conn: &Connection, host: &WallpaperHost) {
    let pause = load_pause(conn);
    host.set_user_paused(None, pause.global);
    for (id, paused) in &pause.monitors {
        host.set_user_paused(Some(id), *paused);
    }
    for (id, volume) in load_monitor_volumes(conn) {
        if let Ok(volume) = validate_volume(volume) {
            host.set_monitor_volume(&id, volume);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Actions shared by the tray menu and the global hotkeys
// ---------------------------------------------------------------------------------------------

/// Event the frontend emits when the user selects a display in Studio: `{ "monitorId": "..." }`.
pub const EVENT_SELECTED_MONITOR: &str = "ui://selected-monitor";

/// Display currently selected in Studio (`None` until the UI reports one).
static SELECTED_MONITOR: Mutex<Option<String>> = parking_lot::const_mutex(None);

pub fn set_selected_monitor(monitor_id: Option<String>) {
    *SELECTED_MONITOR.lock() = monitor_id.filter(|id| !id.trim().is_empty());
}

pub fn selected_monitor() -> Option<String> {
    SELECTED_MONITOR.lock().clone()
}

/// Parses an `ui://selected-monitor` payload.
fn parse_selected_payload(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    value
        .get("monitorId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Tracks the Studio display selection so the tray and hotkeys act on it.
pub fn listen_selected_monitor(app: &AppHandle) {
    app.listen(EVENT_SELECTED_MONITOR, |event| {
        set_selected_monitor(parse_selected_payload(event.payload()));
    });
}

/// The display Next acts on: the one selected in Studio while it is connected, else the primary
/// display, else the first one.
pub fn pick_target_monitor<'a, T>(
    selected: Option<&str>,
    monitors: &'a [T],
    id_of: impl Fn(&T) -> &str,
    is_primary: impl Fn(&T) -> bool,
) -> Option<&'a T> {
    selected
        .and_then(|id| monitors.iter().find(|m| id_of(m) == id))
        .or_else(|| monitors.iter().find(|m| is_primary(m)))
        .or_else(|| monitors.first())
}

/// Next wallpaper on the display selected in Studio (primary when none): the next photo, or the
/// next playlist item.
pub fn next_wallpaper(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = next_wallpaper_now(&app).await {
            tracing::warn!("next wallpaper: {err}");
        }
    });
}

pub async fn next_wallpaper_now(app: &AppHandle) -> Result<(), String> {
    let state = app.try_state::<AppState>().ok_or("App is starting")?;
    let photo = app
        .try_state::<PhotoServiceState>()
        .ok_or("App is starting")?;
    let monitors = live_monitors(app);
    let primary = pick_target_monitor(
        selected_monitor().as_deref(),
        &monitors,
        |m| m.id.as_str(),
        |m| m.is_primary,
    )
    .cloned()
    .ok_or("No display is connected")?;
    let source = crate::db::monitor_sources(&state.db.lock())?
        .remove(&primary.id)
        .unwrap_or_default();
    match source {
        WallpaperSource::PhotoApi => {
            rotate_photo_next(app, &photo, Some(primary.id.clone())).await?;
            crate::scheduler::note_manual_rotation(Some(&primary.id), false);
            Ok(())
        }
        WallpaperSource::Playlist { .. } => {
            crate::scheduler::request_next(&primary.id);
            Ok(())
        }
        WallpaperSource::Workshop { .. } => {
            Err("This display shows a single wallpaper: nothing to advance".into())
        }
    }
}

/// Pause every display, or resume them all when every display is already paused. This is
/// deliberately "all displays", whatever display Studio has selected.
pub fn toggle_pause(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = toggle_pause_now(&app) {
            tracing::warn!("toggle pause: {err}");
        }
    });
}

pub fn toggle_pause_now(app: &AppHandle) -> Result<(), String> {
    let state = app.try_state::<AppState>().ok_or("App is starting")?;
    let monitors = live_monitors(app);
    let pause = if monitors.is_empty() {
        // No live display: still flip the global flag.
        !state.host.user_paused("")
    } else {
        monitors.iter().any(|m| !state.host.user_paused(&m.id))
    };
    apply_user_pause(app, &state, None, pause)
}

// ---------------------------------------------------------------------------------------------
// Global hotkeys
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Next,
    Pause,
}

#[derive(Default, Clone)]
struct Registered {
    next: Option<Shortcut>,
    pause: Option<Shortcut>,
}

static REGISTERED: Mutex<Registered> = parking_lot::const_mutex(Registered {
    next: None,
    pause: None,
});

/// Empty = disabled (`None`); otherwise a valid Tauri accelerator such as `CmdOrCtrl+Alt+P`.
pub fn parse_accelerator(raw: &str) -> Result<Option<Shortcut>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<Shortcut>()
        .map(Some)
        .map_err(|e| format!("'{raw}' is not a valid shortcut: {e}"))
}

/// Plugin callback: runs the action of a pressed registered shortcut.
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, state: ShortcutState) {
    if state != ShortcutState::Pressed {
        return;
    }
    let action = {
        let registered = REGISTERED.lock();
        if registered.next.as_ref() == Some(shortcut) {
            Action::Next
        } else if registered.pause.as_ref() == Some(shortcut) {
            Action::Pause
        } else {
            return;
        }
    };
    match action {
        Action::Next => next_wallpaper(app),
        Action::Pause => toggle_pause(app),
    }
}

/// Replaces the registered hotkeys. On failure (invalid, duplicate or already taken by another
/// application) the previous hotkeys stay registered and a clear error is returned.
pub fn apply_hotkeys(app: &AppHandle, next: &str, pause: &str) -> Result<(), String> {
    let wanted = Registered {
        next: parse_accelerator(next)?,
        pause: parse_accelerator(pause)?,
    };
    if wanted.next.is_some() && wanted.next == wanted.pause {
        return Err("Next and Pause cannot use the same shortcut".into());
    }
    let mut registered = REGISTERED.lock();
    let manager = app.global_shortcut();
    let previous = registered.clone();
    for shortcut in [previous.next, previous.pause].into_iter().flatten() {
        let _ = manager.unregister(shortcut);
    }
    let mut done: Vec<Shortcut> = Vec::new();
    for (label, raw, shortcut) in [
        ("Next", next, wanted.next),
        ("Pause / Resume", pause, wanted.pause),
    ] {
        let Some(shortcut) = shortcut else { continue };
        match manager.register(shortcut) {
            Ok(()) => done.push(shortcut),
            Err(err) => {
                for s in done {
                    let _ = manager.unregister(s);
                }
                for s in [previous.next, previous.pause].into_iter().flatten() {
                    let _ = manager.register(s);
                }
                return Err(format!(
                    "Cannot use '{}' for {label}: it is already taken by another shortcut or application ({err})",
                    raw.trim()
                ));
            }
        }
    }
    *registered = wanted;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_db;

    #[test]
    fn default_accelerators_parse() {
        for raw in [
            truongpaper_core::DEFAULT_NEXT_HOTKEY,
            truongpaper_core::DEFAULT_PAUSE_HOTKEY,
        ] {
            assert!(parse_accelerator(raw).unwrap().is_some(), "{raw}");
        }
        assert_eq!(parse_accelerator("  ").unwrap(), None);
        assert_eq!(parse_accelerator("").unwrap(), None);
        let err = parse_accelerator("Banana+Nope").unwrap_err();
        assert!(err.contains("Banana+Nope"), "{err}");
    }

    #[test]
    fn next_targets_the_selected_display_with_primary_fallback() {
        let monitors = [("A", true), ("B", false), ("C", false)];
        let pick = |sel: Option<&str>, list: &[(&'static str, bool)]| {
            pick_target_monitor(sel, list, |m| m.0, |m| m.1).map(|m| m.0)
        };
        assert_eq!(pick(Some("B"), &monitors), Some("B"));
        assert_eq!(pick(None, &monitors), Some("A"));
        // A selection that is no longer connected falls back to the primary display.
        assert_eq!(pick(Some("gone"), &monitors), Some("A"));
        // No primary flagged: first display.
        assert_eq!(pick(None, &[("X", false), ("Y", false)]), Some("X"));
        assert_eq!(pick(Some("B"), &[]), None);
    }

    #[test]
    fn selected_monitor_event_payload_is_stored() {
        assert_eq!(
            parse_selected_payload(r#"{"monitorId":"DISPLAY-2"}"#).as_deref(),
            Some("DISPLAY-2")
        );
        assert_eq!(parse_selected_payload(r#"{"monitorId":null}"#), None);
        assert_eq!(parse_selected_payload("garbage"), None);
        set_selected_monitor(Some("DISPLAY-2".into()));
        assert_eq!(selected_monitor().as_deref(), Some("DISPLAY-2"));
        set_selected_monitor(Some("  ".into()));
        assert_eq!(selected_monitor(), None);
    }

    #[test]
    fn pause_persistence_tracks_global_and_exceptions() {
        let conn = test_db();
        assert_eq!(load_pause(&conn), PauseState::default());
        persist_pause(&conn, Some("A"), true).unwrap();
        assert_eq!(load_pause(&conn).monitors.get("A"), Some(&true));
        persist_pause(&conn, None, true).unwrap();
        // "Pause all" overrides every display.
        assert_eq!(
            load_pause(&conn),
            PauseState {
                global: true,
                monitors: BTreeMap::new()
            }
        );
        // Resuming one display while paused globally is an exception.
        persist_pause(&conn, Some("B"), false).unwrap();
        assert_eq!(load_pause(&conn).monitors.get("B"), Some(&false));
        // Matching the global value drops the exception.
        persist_pause(&conn, Some("B"), true).unwrap();
        assert!(load_pause(&conn).monitors.is_empty());
    }

    #[test]
    fn volumes_are_validated_and_persisted() {
        assert_eq!(validate_volume(0.0), Ok(0.0));
        assert_eq!(validate_volume(1.0), Ok(1.0));
        assert!(validate_volume(1.01).is_err());
        assert!(validate_volume(-0.1).is_err());
        assert!(validate_volume(f32::NAN).is_err());
        let conn = test_db();
        persist_monitor_volume(&conn, "A", 0.25).unwrap();
        persist_monitor_volume(&conn, "B", 1.0).unwrap();
        let volumes = load_monitor_volumes(&conn);
        assert_eq!(volumes.get("A"), Some(&0.25));
        assert!(!volumes.contains_key("B"), "full volume is the default");
    }

    #[test]
    fn restore_applies_pause_and_volume_before_any_runtime_exists() {
        let conn = test_db();
        persist_pause(&conn, Some("A"), true).unwrap();
        persist_monitor_volume(&conn, "A", 0.5).unwrap();
        persist_pause(&conn, None, true).unwrap();
        persist_pause(&conn, Some("B"), false).unwrap();
        persist_monitor_volume(&conn, "B", 0.0).unwrap();
        let host = WallpaperHost::new();
        restore_persisted(&conn, &host);
        // Displays that connect later still follow "pause all".
        assert!(host.user_paused("unseen"));
        assert!(!host.monitor_should_render("unseen"));
        assert!(!host.user_paused("B"));
        assert_eq!(host.monitor_volume("B"), 0.0);
        assert_eq!(host.monitor_volume("A"), 0.5);
        assert_eq!(host.monitor_volume("unseen"), 1.0);
    }
}
