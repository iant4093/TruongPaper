//! Monitor, playback & property customization commands (docs/04 §5.1):
//! `monitors_list`, `monitor_apply_wallpaper`, `monitor_update_property`,
//! `monitor_update_adjustments`, `playback_set_paused`.

use crate::commands::photo::{rotate_photo_for_locked_monitor, PhotoServiceState};
use crate::db;
use crate::host::{ActiveRuntime, RenderEvent, SceneDescriptor, WebviewKind};
use crate::models::{
    validate_property_update, IpcFitMode, IpcTransitionType, MonitorInfo, Playlist, PlaylistItem,
    UniversalAdjustments, WallpaperChangedPayload, WallpaperItem, WallpaperSource,
    EVENT_WALLPAPER_CHANGED,
};
use crate::state::AppState;
use crate::wallpaper_webview::{self, WebviewSpec};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tauri::{AppHandle, Emitter, State};
use truongpaper_platform::is_wallpaper_type_supported_on_this_os;
use truongpaper_renderer::WallpaperRuntimeKind;
use truongpaper_we_formats::WeVirtualFileSystem;

/// Fallback when the OS does not report a refresh rate through Tauri's monitor API.
pub const DEFAULT_REFRESH_RATE_HZ: f64 = 60.0;

/// A connected display as reported by Tauri, with a stable TruongPaper monitor id.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveMonitor {
    pub id: String,
    pub name: String,
    /// Physical pixels.
    pub width: u32,
    pub height: u32,
    /// Physical top-left in the virtual screen.
    pub x: i32,
    pub y: i32,
    pub scale_factor: f64,
    pub is_primary: bool,
}

impl LiveMonitor {
    pub fn is_portrait(&self) -> bool {
        self.height > self.width
    }

    /// `(x, y, w, h)` in logical points (Quartz global space on macOS).
    pub fn logical_rect(&self) -> (f64, f64, f64, f64) {
        let s = if self.scale_factor > 0.0 {
            self.scale_factor
        } else {
            1.0
        };
        (
            self.x as f64 / s,
            self.y as f64 / s,
            self.width as f64 / s,
            self.height as f64 / s,
        )
    }
}

/// Monitor ids: the OS display name (`\\.\DISPLAY1` on Windows), `DISPLAY-<n>` when unnamed,
/// and a `#<k>` suffix for duplicate names (two identical external panels on macOS).
pub fn assign_monitor_ids(names: &[Option<String>]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let base = name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("DISPLAY-{}", i + 1));
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            if *n > 1 {
                format!("{base} #{n}")
            } else {
                base
            }
        })
        .collect()
}

pub fn live_monitors(app: &AppHandle) -> Vec<LiveMonitor> {
    let monitors = app.available_monitors().unwrap_or_default();
    let primary = app.primary_monitor().ok().flatten();
    let names: Vec<Option<String>> = monitors.iter().map(|m| m.name().cloned()).collect();
    let ids = assign_monitor_ids(&names);
    let mut out: Vec<LiveMonitor> = monitors
        .iter()
        .zip(ids)
        .enumerate()
        .map(|(i, (m, id))| LiveMonitor {
            name: m
                .name()
                .cloned()
                .unwrap_or_else(|| format!("Display {}", i + 1)),
            id,
            width: m.size().width,
            height: m.size().height,
            x: m.position().x,
            y: m.position().y,
            scale_factor: m.scale_factor(),
            is_primary: primary
                .as_ref()
                .is_some_and(|p| p.position() == m.position() && p.size() == m.size()),
        })
        .collect();
    if !out.is_empty() && !out.iter().any(|m| m.is_primary) {
        out[0].is_primary = true;
    }
    out
}

/// `(monitor_id, is_portrait)` for every connected display.
pub fn live_monitor_orientations(app: &AppHandle) -> Vec<(String, bool)> {
    live_monitors(app)
        .into_iter()
        .map(|m| {
            let portrait = m.is_portrait();
            (m.id, portrait)
        })
        .collect()
}

pub fn validate_monitor_id(monitor_id: &str) -> Result<(), String> {
    if monitor_id.trim().is_empty() || monitor_id.len() > 256 {
        Err("Invalid monitor id".into())
    } else {
        Ok(())
    }
}

static APPLY_LOCKS: LazyLock<parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

/// Serializes everything that changes what `monitor_id` shows (`monitor_apply_wallpaper` and
/// the rotation scheduler), so a scheduled rotation can neither overwrite a concurrent user
/// apply nor open a second wallpaper window with the same label. Hold the guard for the whole
/// apply; the helpers below never take it themselves (no re-entrancy).
pub(crate) async fn lock_monitor_apply(monitor_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = Arc::clone(
        APPLY_LOCKS
            .lock()
            .entry(monitor_id.to_string())
            .or_default(),
    );
    lock.lock_owned().await
}

/// Multi-monitor applies always lock sorted, unique IDs to avoid opposing clone deadlocks.
pub(crate) async fn lock_monitor_applies<'a>(
    monitor_ids: impl IntoIterator<Item = &'a str>,
) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
    let ids: std::collections::BTreeSet<_> = monitor_ids.into_iter().collect();
    let mut guards = Vec::with_capacity(ids.len());
    for id in ids {
        guards.push(lock_monitor_apply(id).await);
    }
    guards
}

fn settings_modes(state: &AppState) -> (IpcFitMode, IpcTransitionType) {
    let s = state.settings.lock();
    (
        IpcFitMode::from_setting(&s.default_fit_mode),
        IpcTransitionType::from_setting(&s.transition_type),
    )
}

/// Every user property of a wallpaper as `{ key: value }`: the `project.json`
/// `general.properties.<key>.value` defaults overlaid with the stored user overrides.
pub fn effective_user_properties(
    project_json: &Value,
    user: &Map<String, Value>,
) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(props) = project_json
        .pointer("/general/properties")
        .and_then(Value::as_object)
    {
        for (key, prop) in props {
            if let Some(v) = prop.get("value") {
                out.insert(key.clone(), v.clone());
            }
        }
    }
    for (key, v) in user {
        out.insert(key.clone(), v.clone());
    }
    out
}

/// `window.__truongpaper_dispatch_properties({"key":{"value":v},...}, null);` for Web pages.
pub fn web_properties_js(props: &Map<String, Value>) -> Result<String, String> {
    let pairs: Vec<(String, String)> = props
        .iter()
        .map(|(k, v)| (k.clone(), v.to_string()))
        .collect();
    truongpaper_renderer::properties_dispatch_js(
        &truongpaper_renderer::we_property_payload(&pairs)?,
        None,
    )
}

/// `<install_dir>/<project.json file>` for an Application wallpaper, which must resolve to an
/// existing file strictly inside the install directory (no `..` / symlink escapes).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn application_exe_path(
    install_dir: &Path,
    project_file: &str,
) -> Result<PathBuf, String> {
    let rel = project_file.trim();
    if rel.is_empty() {
        return Err("project.json has no executable 'file'".into());
    }
    let exe = install_dir.join(rel);
    if !exe.is_file() || !crate::commands::library::is_strictly_within(&exe, install_dir) {
        return Err(format!(
            "Application executable '{rel}' is not inside the wallpaper folder"
        ));
    }
    Ok(exe)
}

/// Starts the live runtime for a library wallpaper on `monitor_id` (scene descriptor for the
/// render thread, desktop webview window, or Windows Application process).
#[allow(clippy::too_many_arguments)]
async fn start_wallpaper_runtime(
    app: &AppHandle,
    state: &AppState,
    monitor_id: &str,
    wallpaper_id: &str,
    source: &WallpaperSource,
    kind: Result<WallpaperRuntimeKind, String>,
    item: &WallpaperItem,
    install_dir: &Path,
    core_assets_dir: PathBuf,
    vfs: WeVirtualFileSystem,
) -> Result<(), String> {
    let project_file = item
        .project_json
        .get("file")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let adjustments = item.adjustments;
    let audio_reactive = item.audio_reactive;
    let props = effective_user_properties(&item.project_json, &item.user_properties);
    let host = &state.host;
    let activate = || -> Result<(), String> {
        db::set_monitor_source(&state.db.lock(), monitor_id, source)?;
        host.set_source(
            monitor_id,
            source.clone(),
            Some(wallpaper_id.to_string()),
            adjustments.compositor_uniforms(settings_modes(state).1),
        );
        Ok(())
    };
    if !matches!(
        kind,
        Ok(WallpaperRuntimeKind::Web | WallpaperRuntimeKind::Video)
    ) {
        activate()?;
    }
    match kind {
        Ok(WallpaperRuntimeKind::Scene) => {
            let scene_file = if project_file.is_empty() {
                "scene.json".to_string()
            } else {
                project_file
            };
            let roots = vec![install_dir.to_path_buf(), core_assets_dir.clone()];
            let content_revision =
                tokio::task::spawn_blocking(move || crate::host::scene_content_revision(&roots))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
            host.set_runtime(
                monitor_id,
                ActiveRuntime::Scene(SceneDescriptor {
                    install_dir: install_dir.to_path_buf(),
                    core_assets_dir,
                    scene_file,
                    content_revision,
                    user_properties_json: Value::Object(props).to_string(),
                }),
                adjustments,
                audio_reactive,
            );
            if !host.live_renderer_attached() {
                tracing::warn!(
                    "scene wallpaper '{}' selected but no desktop renderer is attached",
                    item.title
                );
            }
            Ok(())
        }
        Ok(k @ (WallpaperRuntimeKind::Web | WallpaperRuntimeKind::Video)) => {
            let (webview_kind, url, on_load) = if k == WallpaperRuntimeKind::Web {
                (
                    WebviewKind::Web,
                    truongpaper_renderer::web_wallpaper_url(&project_file),
                    web_properties_js(&props)?,
                )
            } else {
                let adj_json = serde_json::to_string(&adjustments).map_err(|e| e.to_string())?;
                (
                    WebviewKind::Video,
                    truongpaper_renderer::video_host_url(&project_file),
                    truongpaper_renderer::video_adjustments_js(&adj_json)?,
                )
            };
            let url = url.ok_or_else(|| {
                format!("'{}' has no valid entry file in project.json", item.title)
            })?;
            let preview_path = PathBuf::from(&item.preview_path);
            let preview_path = (cfg!(target_os = "macos")
                && preview_path.is_file()
                && crate::commands::library::is_strictly_within(&preview_path, install_dir))
            .then_some(preview_path);
            let vfs = Arc::new(vfs);
            wallpaper_webview::show(
                app,
                host,
                Arc::clone(&vfs),
                WebviewSpec {
                    monitor_id: monitor_id.to_string(),
                    kind: webview_kind,
                    wallpaper_id: wallpaper_id.to_string(),
                    url,
                    audio_reactive,
                    on_load_scripts: vec![on_load],
                },
                |label| {
                    activate()?;
                    host.set_active_vfs(wallpaper_id, vfs);
                    host.set_runtime(
                        monitor_id,
                        ActiveRuntime::Webview {
                            kind: webview_kind,
                            label,
                            preview_path,
                        },
                        adjustments,
                        audio_reactive,
                    );
                    Ok(())
                },
            )
            .await
        }
        #[cfg(target_os = "windows")]
        Ok(WallpaperRuntimeKind::Application) => {
            let exe = application_exe_path(install_dir, &project_file)?;
            let (w, h) = live_monitors(app)
                .into_iter()
                .find(|m| m.id == monitor_id)
                .map(|m| (m.width as i32, m.height as i32))
                .ok_or_else(|| format!("Monitor '{monitor_id}' is not connected"))?;
            host.set_runtime(
                monitor_id,
                ActiveRuntime::Application {
                    exe_path: exe.clone(),
                },
                adjustments,
                false,
            );
            let process =
                truongpaper_platform::application::ApplicationWallpaperProcess::spawn(&exe, w, h)
                    .map_err(|e| format!("Could not start '{}': {e}", item.title))?;
            host.set_application_process(monitor_id, process);
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        Ok(WallpaperRuntimeKind::Application) => Err(format!(
            "'{}' is a Windows-only Application wallpaper",
            item.title
        )),
        Err(err) => {
            tracing::warn!("{}: {err}; nothing is rendered", item.title);
            host.set_runtime(monitor_id, ActiveRuntime::None, adjustments, false);
            Ok(())
        }
    }
}

/// Mounts a library wallpaper as the active `wpe://` VFS, persists `source` for the monitor,
/// starts its live runtime and emits `wallpaper://changed`.
pub(crate) async fn show_library_wallpaper(
    app: &AppHandle,
    state: &AppState,
    monitor_id: &str,
    wallpaper_id: &str,
    source: WallpaperSource,
) -> Result<(), String> {
    let row = {
        let conn = state.db.lock();
        db::get_wallpaper(&conn, wallpaper_id)?
    }
    .ok_or_else(|| format!("Wallpaper '{wallpaper_id}' is not in the library"))?;
    if !is_wallpaper_type_supported_on_this_os(&row.wallpaper_type) {
        return Err(format!(
            "'{}' is a Windows-only Application wallpaper",
            row.title
        ));
    }
    let kind = crate::host::runtime_kind_for(&row.wallpaper_type);
    let (default_fit, transition) = settings_modes(state);
    let core_fallback = state.core_assets_root().join("assets");
    let install_dir = PathBuf::from(&row.install_dir);
    let mount_dir = install_dir.clone();
    let (item, vfs, core_dir) = tokio::task::spawn_blocking(move || {
        let item = row.to_item(default_fit);
        let core_dir = truongpaper_steam::find_local_we_core_assets_dir().unwrap_or(core_fallback);
        let vfs = WeVirtualFileSystem::mount(mount_dir, core_dir.clone());
        (item, vfs, core_dir)
    })
    .await
    .map_err(|e| e.to_string())?;
    let item = item.ok_or_else(|| {
        format!("Wallpaper '{wallpaper_id}' has no readable project.json; re-sync the library")
    })?;
    #[cfg(target_os = "windows")]
    if kind == Ok(WallpaperRuntimeKind::Application) {
        let file = item
            .project_json
            .get("file")
            .and_then(Value::as_str)
            .unwrap_or_default();
        application_exe_path(&install_dir, file)?;
    }

    start_wallpaper_runtime(
        app,
        state,
        monitor_id,
        wallpaper_id,
        &source,
        kind,
        &item,
        &install_dir,
        core_dir,
        vfs,
    )
    .await?;
    let _ = app.emit(
        EVENT_WALLPAPER_CHANGED,
        WallpaperChangedPayload {
            monitor_id,
            source: &source,
            current_item: Some(&item),
            transition_used: transition.as_str(),
        },
    );
    Ok(())
}

/// Forwards changed user properties (`{ key: value }`) to the live Web pages and scenes of
/// `wallpaper_id`.
pub(crate) fn forward_properties(
    state: &AppState,
    wallpaper_id: &str,
    changed: &Map<String, Value>,
) {
    if changed.is_empty() {
        return;
    }
    let showing = state.host.monitors_showing(wallpaper_id);
    if showing.iter().any(|(_, rt)| {
        matches!(
            rt.active,
            ActiveRuntime::Webview {
                kind: WebviewKind::Web,
                ..
            }
        )
    }) {
        match web_properties_js(changed) {
            Ok(js) => wallpaper_webview::eval_for_wallpaper(wallpaper_id, WebviewKind::Web, &js),
            Err(err) => tracing::warn!("property update for {wallpaper_id}: {err}"),
        }
    }
    if showing.iter().any(|(_, rt)| rt.active.is_scene()) {
        let changed_json = Value::Object(changed.clone()).to_string();
        state.host.merge_scene_properties(wallpaper_id, changed);
        state.host.push_render_event(RenderEvent::SceneProperties {
            wallpaper_id: wallpaper_id.to_string(),
            changed_json,
        });
    }
}

/// Applies new adjustments to the host (compositor uniforms, scene volume) and to the video
/// pages of `wallpaper_id`.
pub(crate) fn forward_adjustments(
    state: &AppState,
    wallpaper_id: &str,
    adjustments: &UniversalAdjustments,
    transition: IpcTransitionType,
) {
    state.host.update_adjustments_for_wallpaper(
        wallpaper_id,
        *adjustments,
        adjustments.compositor_uniforms(transition),
    );
    match serde_json::to_string(adjustments)
        .map_err(|e| e.to_string())
        .and_then(|json| truongpaper_renderer::video_adjustments_js(&json))
    {
        Ok(js) => wallpaper_webview::eval_for_wallpaper(wallpaper_id, WebviewKind::Video, &js),
        Err(err) => tracing::warn!("adjustments for {wallpaper_id}: {err}"),
    }
}

/// Updates the source assignment and fit mode for the photo about to be shown.
pub(crate) fn set_photo_host_source(
    state: &AppState,
    monitor_id: &str,
) -> Result<WallpaperSource, String> {
    let (default_fit, transition) = settings_modes(state);
    let source = {
        let conn = state.db.lock();
        db::monitor_sources(&conn)?
            .remove(monitor_id)
            .unwrap_or_default()
    };
    let uniforms = UniversalAdjustments {
        fit_mode: default_fit,
        ..Default::default()
    }
    .compositor_uniforms(transition);
    state
        .host
        .set_source(monitor_id, source.clone(), None, uniforms);
    Ok(source)
}

/// Shows the monitor's current photo, or draws one from the queue. `draw_new` always draws the
/// next photo (playlist `photo_api_draw` items and scheduled rotations).
pub(crate) async fn show_photo(
    app: &AppHandle,
    state: &AppState,
    photo: &PhotoServiceState,
    monitor_id: &str,
    draw_new: bool,
) -> Result<(), String> {
    let (_, transition) = settings_modes(state);
    let current = if draw_new {
        None
    } else {
        let conn = state.db.lock();
        truongpaper_core::current_photo_by_monitor(&conn, &state.app_data_dir)
            .map_err(|e| e.to_string())?
            .remove(monitor_id)
            .flatten()
    };
    let match_orientation = state.settings.lock().skip_mismatched_orientation;
    let current = current.filter(|item| {
        !match_orientation || crate::commands::photo::photo_matches_display(app, monitor_id, item)
    });
    match current {
        Some(item) => {
            let path = PathBuf::from(&item.file_path);
            let on_screen = state.host.monitor(monitor_id).is_some_and(|rt| {
                rt.active
                    == ActiveRuntime::Photo {
                        path: Some(path.clone()),
                    }
            });
            let decoded = if on_screen {
                None
            } else {
                crate::commands::photo::decode_for_desktop(&state.host, &path).await
            };
            let source = set_photo_host_source(state, monitor_id)?;
            if !on_screen {
                state.host.set_photo(monitor_id, path, decoded);
            }
            let _ = app.emit(
                EVENT_WALLPAPER_CHANGED,
                WallpaperChangedPayload {
                    monitor_id,
                    source: &source,
                    current_item: Some(&item),
                    transition_used: transition.as_str(),
                },
            );
        }
        None => {
            // The source is already persisted; an empty queue / offline API is not fatal here.
            if let Err(err) = rotate_photo_for_locked_monitor(app, photo, monitor_id).await {
                tracing::warn!("no photo available for {monitor_id} yet: {err}");
                if let Some(previous) = state.host.monitor(monitor_id).filter(|rt| {
                    !matches!(rt.active, ActiveRuntime::None | ActiveRuntime::Photo { .. })
                }) {
                    db::set_monitor_source(&state.db.lock(), monitor_id, &previous.source)?;
                    return Err(err);
                }
                let source = set_photo_host_source(state, monitor_id)?;
                state.host.ensure_photo_runtime(monitor_id);
                if state
                    .host
                    .monitor(monitor_id)
                    .is_some_and(|rt| rt.active == (ActiveRuntime::Photo { path: None }))
                {
                    let _ = app.emit(
                        EVENT_WALLPAPER_CHANGED,
                        WallpaperChangedPayload::<truongpaper_photo_api::PhotoItem> {
                            monitor_id,
                            source: &source,
                            current_item: None,
                            transition_used: transition.as_str(),
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

/// Index of the playlist item `monitor_apply_wallpaper` shows first (the first item allowed by
/// the playlist mode).
pub fn first_playlist_index(playlist: &Playlist) -> Option<usize> {
    playlist
        .items
        .iter()
        .position(|item| crate::scheduler::playlist_item_allowed(playlist.mode, item))
}

/// Shows `item` of a playlist on `monitor_id` (the persisted source stays the playlist).
pub(crate) async fn show_playlist_item(
    app: &AppHandle,
    state: &AppState,
    photo: &PhotoServiceState,
    monitor_id: &str,
    playlist_id: &str,
    item: &PlaylistItem,
    draw_new_photo: bool,
) -> Result<(), String> {
    let source = WallpaperSource::Playlist {
        playlist_id: playlist_id.to_string(),
    };
    match item {
        PlaylistItem::Workshop { wallpaper_id } => {
            show_library_wallpaper(app, state, monitor_id, wallpaper_id, source).await
        }
        PlaylistItem::PhotoApiDraw => {
            {
                let conn = state.db.lock();
                db::set_monitor_source(&conn, monitor_id, &source)?;
            }
            show_photo(app, state, photo, monitor_id, draw_new_photo).await
        }
    }
}

/// Applies `source` to a monitor (the body of `monitor_apply_wallpaper`, also used by the
/// rotation scheduler to restore persisted sources on launch).
pub(crate) async fn apply_source(
    app: &AppHandle,
    state: &AppState,
    photo: &PhotoServiceState,
    monitor_id: &str,
    source: &WallpaperSource,
) -> Result<(), String> {
    match source {
        WallpaperSource::PhotoApi => {
            {
                let conn = state.db.lock();
                db::set_monitor_source(&conn, monitor_id, source)?;
            }
            show_photo(app, state, photo, monitor_id, false).await
        }
        WallpaperSource::Workshop { wallpaper_id } => {
            show_library_wallpaper(app, state, monitor_id, wallpaper_id, source.clone()).await
        }
        WallpaperSource::Playlist { playlist_id } => {
            let playlist = {
                let conn = state.db.lock();
                db::get_playlist(&conn, playlist_id)?
            }
            .ok_or_else(|| format!("Playlist '{playlist_id}' not found"))?;
            if playlist.items.is_empty() {
                return Err(format!("Playlist '{}' has no items", playlist.name));
            }
            let index = first_playlist_index(&playlist).ok_or_else(|| {
                format!(
                    "Playlist '{}' has no items allowed by its mode",
                    playlist.name
                )
            })?;
            show_playlist_item(
                app,
                state,
                photo,
                monitor_id,
                playlist_id,
                &playlist.items[index],
                false,
            )
            .await
        }
    }
}

#[tauri::command]
pub async fn monitors_list(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<MonitorInfo>, String> {
    let sources = {
        let conn = state.db.lock();
        db::monitor_sources(&conn)?
    };
    Ok(live_monitors(&app)
        .into_iter()
        .map(|m| {
            let runtime = state.host.monitor(&m.id).filter(|rt| rt.generation != 0);
            MonitorInfo {
                active_source: runtime
                    .as_ref()
                    .map(|rt| rt.source.clone())
                    .or_else(|| sources.get(&m.id).cloned())
                    .unwrap_or_default(),
                active_wallpaper_id: runtime.and_then(|rt| rt.wallpaper_id),
                is_portrait: m.is_portrait(),
                monitor_id: m.id,
                name: m.name,
                width: m.width,
                height: m.height,
                scale_factor: m.scale_factor,
                refresh_rate_hz: DEFAULT_REFRESH_RATE_HZ,
                is_primary: m.is_primary,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn monitor_apply_wallpaper(
    app: AppHandle,
    state: State<'_, AppState>,
    photo: State<'_, PhotoServiceState>,
    monitor_id: String,
    source: WallpaperSource,
) -> Result<(), String> {
    validate_monitor_id(&monitor_id)?;
    {
        let _apply = lock_monitor_apply(&monitor_id).await;
        apply_source(&app, &state, &photo, &monitor_id, &source).await?;
    }
    // The scheduler restarts this monitor's timer (and playlist position) from now.
    crate::scheduler::note_manual_rotation(Some(&monitor_id), true);
    Ok(())
}

#[tauri::command]
pub async fn monitor_update_property(
    state: State<'_, AppState>,
    wallpaper_id: String,
    prop_key: String,
    prop_value: Value,
) -> Result<(), String> {
    validate_property_update(&prop_key, &prop_value)?;
    {
        let conn = state.db.lock();
        db::set_user_property(&conn, &wallpaper_id, &prop_key, &prop_value)?;
    }
    let mut changed = Map::new();
    changed.insert(prop_key.trim().to_string(), prop_value);
    forward_properties(&state, &wallpaper_id, &changed);
    Ok(())
}

#[tauri::command]
pub async fn monitor_update_adjustments(
    state: State<'_, AppState>,
    wallpaper_id: String,
    adjustments: UniversalAdjustments,
) -> Result<(), String> {
    let adjustments = adjustments.sanitized();
    {
        let conn = state.db.lock();
        db::set_adjustments(&conn, &wallpaper_id, &adjustments)?;
    }
    let (_, transition) = settings_modes(&state);
    forward_adjustments(&state, &wallpaper_id, &adjustments, transition);
    Ok(())
}

#[tauri::command]
pub async fn playback_set_paused(
    state: State<'_, AppState>,
    monitor_id: Option<String>,
    paused: bool,
) -> Result<(), String> {
    if let Some(id) = &monitor_id {
        validate_monitor_id(id)?;
    }
    state.host.set_user_paused(monitor_id.as_deref(), paused);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_ids_are_unique_and_named() {
        let ids = assign_monitor_ids(&[
            Some("LG UltraFine".into()),
            None,
            Some("LG UltraFine".into()),
            Some("  ".into()),
            Some("\\\\.\\DISPLAY1".into()),
        ]);
        assert_eq!(
            ids,
            vec![
                "LG UltraFine".to_string(),
                "DISPLAY-2".into(),
                "LG UltraFine #2".into(),
                "DISPLAY-4".into(),
                "\\\\.\\DISPLAY1".into(),
            ]
        );
    }

    #[test]
    fn logical_rect_and_orientation() {
        let m = LiveMonitor {
            id: "A".into(),
            name: "A".into(),
            width: 2880,
            height: 5120,
            x: -2880,
            y: 0,
            scale_factor: 2.0,
            is_primary: false,
        };
        assert!(m.is_portrait());
        assert_eq!(m.logical_rect(), (-1440.0, 0.0, 1440.0, 2560.0));
    }

    #[test]
    fn monitor_id_validation() {
        assert!(validate_monitor_id("DISPLAY-1").is_ok());
        assert!(validate_monitor_id(" ").is_err());
        assert!(validate_monitor_id(&"x".repeat(300)).is_err());
    }

    #[test]
    fn effective_properties_overlay_user_values_on_project_defaults() {
        let project = serde_json::json!({
            "file": "index.html",
            "general": { "properties": {
                "speed": { "type": "slider", "value": 5 },
                "tint": { "type": "color", "value": "1 0 0" },
                "label": { "type": "text" }
            }}
        });
        let mut user = Map::new();
        user.insert("speed".into(), serde_json::json!(8));
        user.insert("extra".into(), serde_json::json!(true));
        let props = effective_user_properties(&project, &user);
        assert_eq!(props.get("speed"), Some(&serde_json::json!(8)));
        assert_eq!(props.get("tint"), Some(&serde_json::json!("1 0 0")));
        assert_eq!(props.get("extra"), Some(&serde_json::json!(true)));
        assert!(!props.contains_key("label"));
        assert!(effective_user_properties(&serde_json::json!({}), &Map::new()).is_empty());

        let js = web_properties_js(&props).unwrap();
        assert!(js.starts_with("window.__truongpaper_dispatch_properties({"));
        assert!(js.contains("\"speed\":{\"value\":8}"));
        assert!(js.ends_with(", null);"));
        // Script text in values is escaped, never injected.
        let mut evil = Map::new();
        evil.insert("x".into(), serde_json::json!("</script><script>alert(1)"));
        assert!(!web_properties_js(&evil).unwrap().contains("</script>"));
    }

    #[test]
    fn application_exe_must_stay_inside_the_install_dir() {
        let root = std::env::temp_dir().join(format!("truongpaper-app-exe-{}", std::process::id()));
        let install = root.join("wallpaper");
        std::fs::create_dir_all(install.join("bin")).unwrap();
        std::fs::write(install.join("bin/app.exe"), b"MZ").unwrap();
        std::fs::write(root.join("outside.exe"), b"MZ").unwrap();
        assert_eq!(
            application_exe_path(&install, "bin/app.exe").unwrap(),
            install.join("bin/app.exe")
        );
        assert!(application_exe_path(&install, "../outside.exe").is_err());
        assert!(application_exe_path(&install, "missing.exe").is_err());
        assert!(
            application_exe_path(&install, "bin").is_err(),
            "directories are rejected"
        );
        assert!(application_exe_path(&install, "  ").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
