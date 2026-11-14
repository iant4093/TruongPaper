//! Desktop-level webview windows for Web and Video wallpapers (docs/03 §6.2, §6.3, §9) and the
//! bridge thread that feeds them.
//!
//! Each monitor showing a Web or Video wallpaper gets one undecorated `WebviewWindow` labelled
//! `wallpaper-<monitor id>` that loads the `wpe://` URL built by
//! `truongpaper_renderer::web_runtime` and is moved behind the desktop icons
//! (`mac_desktop::attach_window_to_desktop` / `win_desktop::attach_hwnd_to_desktop`). These
//! windows are deliberately NOT listed in `capabilities/default.json`, and `build.rs` declares
//! an app ACL manifest, so Tauri rejects every app and plugin command they invoke; they only
//! receive `eval` calls from the backend. Each window streams from its own wallpaper's VFS
//! (`WallpaperHost::set_webview_vfs`) and follows its monitor's rect on display changes.
//!
//! The bridge thread (`truongpaper-webview-bridge`) closes windows whose monitor switched to
//! another runtime, starts / suspends the audio loopback worker, pushes 128-float spectrum
//! frames (60 Hz max), forwards desktop cursor events to Web pages and applies user pause /
//! Smart Pause (docs/01 §6.1 items 2 and 3).

use crate::commands::monitor::LiveMonitor;
use crate::desktop_host::{
    cursor_transitions, emit_loaded, hotplug_restore_ids, live_monitors_cached, monitor_geometries,
    CursorPoller, CursorSample,
};
use crate::host::{ActiveRuntime, MonitorRuntime, WallpaperHost, WebviewKind};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use truongpaper_platform::MonitorPlaybackPolicy;
use truongpaper_renderer::{
    audio_dispatch_js, cursor_event_js, pack_web_audio_frame, video_set_paused_js,
};
use truongpaper_we_formats::WeVirtualFileSystem;

const WEB_PLAYBACK_PRELOAD_JS: &str = include_str!("web-wallpaper-playback.js");

/// Minimum spacing of `__truongpaper_dispatch_audio` pushes (60 Hz).
pub const AUDIO_PUSH_INTERVAL: Duration = Duration::from_nanos(16_666_667);
/// Bridge tick while a webview wallpaper is playing / while idle.
const ACTIVE_TICK: Duration = Duration::from_millis(16);
const IDLE_TICK: Duration = Duration::from_millis(250);
/// Bridge tick while only Video windows (or paused pages) exist: keeps volume edits responsive.
/// Longest bridge sleep while windows exist but nothing is a live Web page: host changes wake
/// the bridge immediately, this only covers state without a signal (display sleep).
const WINDOW_TICK: Duration = Duration::from_secs(1);
const GEOMETRY_REFRESH: Duration = Duration::from_secs(5);
#[cfg(target_os = "macos")]
const REVEAL_DURATION: Duration = Duration::from_millis(210);
#[cfg(target_os = "macos")]
const STAGE_SCRIPT: &str = include_str!("wallpaper-stage.js");

// ---------------------------------------------------------------------------------------------
// Pure helpers (unit tested)
// ---------------------------------------------------------------------------------------------

/// `wallpaper-<monitor id>` with every character outside `[A-Za-z0-9_-]` replaced by `_`
/// (Tauri labels allow a restricted character set; monitor ids can be `\\.\DISPLAY1` or
/// `LG UltraFine #2`).
pub fn wallpaper_window_label(monitor_id: &str) -> String {
    let sanitized: String = monitor_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("wallpaper-{sanitized}")
}

/// Script that pauses / resumes a webview wallpaper.
pub fn pause_script(kind: WebviewKind, paused: bool) -> String {
    match kind {
        WebviewKind::Video => video_set_paused_js(paused),
        WebviewKind::Web => format!("window.__truongpaper_set_paused?.({paused});"),
    }
}

/// `true` for the only pages a wallpaper window may navigate to: our own `wpe://localhost`
/// (`WKWebView`) and `http(s)://wpe.localhost` (`WebView2`) origins, plus `about:blank`.
///
/// `subframes_possible` widens this for engines whose navigation callback also fires for
/// iframes (`WKWebView`; `WebView2` only reports top-level navigations): ordinary web, `data:`
/// and `blob:` documents stay allowed there so embedded players keep working, while every other
/// scheme (`file:`, `tel:`, app-launching schemes) is refused.
fn wallpaper_navigation_allowed(url: &tauri::Url, subframes_possible: bool) -> bool {
    match url.scheme() {
        "wpe" => url.host_str() == Some("localhost"),
        "http" | "https" => subframes_possible || url.host_str() == Some("wpe.localhost"),
        "about" => url.as_str() == "about:blank",
        "data" | "blob" => subframes_possible,
        _ => false,
    }
}

pub fn web_volume_script(volume: f32, muted: bool) -> String {
    let level = if muted || !volume.is_finite() {
        0.0
    } else {
        volume.clamp(0.0, 1.0)
    };
    format!("window.__truongpaper_set_volume?.({level});")
}

/// Pause state of one webview window as tracked by the bridge.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowPauseState {
    pub monitor_id: String,
    pub label: String,
    pub kind: WebviewKind,
    /// The page finished loading (scripts can run).
    pub loaded: bool,
    /// Last pause state sent to the page (`None` = never sent since load).
    pub paused_sent: Option<bool>,
}

/// `(label, script, paused)` for every loaded window whose desired pause state differs from the
/// last one sent. `playing(monitor_id)` is the host's `monitor_should_render`.
pub fn plan_pause_updates(
    windows: &[WindowPauseState],
    playing: impl Fn(&str) -> bool,
) -> Vec<(String, String, bool)> {
    windows
        .iter()
        .filter(|w| w.loaded)
        .filter_map(|w| {
            let paused = !playing(&w.monitor_id);
            (w.paused_sent != Some(paused))
                .then(|| (w.label.clone(), pause_script(w.kind, paused), paused))
        })
        .collect()
}

/// Volume state of one webview window as tracked by the bridge.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowAudioState {
    pub monitor_id: String,
    pub label: String,
    pub kind: WebviewKind,
    pub loaded: bool,
    pub audio_sent: Option<String>,
}

/// Script that sets the sound of a webview wallpaper from its effective adjustments (volume
/// already scaled by the display volume, `muted` set for non-owners).
pub fn audio_script(
    kind: WebviewKind,
    adjustments: &crate::models::UniversalAdjustments,
) -> String {
    match kind {
        WebviewKind::Web => web_volume_script(adjustments.volume, adjustments.muted),
        WebviewKind::Video => serde_json::to_string(adjustments)
            .map_err(|e| e.to_string())
            .and_then(|json| truongpaper_renderer::video_adjustments_js(&json))
            .unwrap_or_else(|_| web_volume_script(0.0, true)),
    }
}

/// `(label, script)` for every loaded window whose desired sound differs from the last one sent.
pub fn plan_audio_updates(
    windows: &[WindowAudioState],
    adjustments: &HashMap<String, crate::models::UniversalAdjustments>,
) -> Vec<(String, String)> {
    windows
        .iter()
        .filter(|w| w.loaded)
        .filter_map(|w| {
            let script = audio_script(w.kind, adjustments.get(&w.monitor_id)?);
            (w.audio_sent.as_deref() != Some(script.as_str())).then(|| (w.label.clone(), script))
        })
        .collect()
}

/// Rate limiter for the 60 Hz audio push.
#[derive(Debug, Clone, Default)]
pub struct AudioPushThrottle {
    last: Option<Instant>,
}

impl AudioPushThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` (and records `now`) when at least [`AUDIO_PUSH_INTERVAL`] passed since the last
    /// accepted push.
    pub fn ready(&mut self, now: Instant) -> bool {
        let ok = self
            .last
            .is_none_or(|t| now.saturating_duration_since(t) >= AUDIO_PUSH_INTERVAL);
        if ok {
            self.last = Some(now);
        }
        ok
    }
}

/// `true` when the host still wants `window` on its monitor.
fn window_matches_host(window: &WebWindow, rt: Option<&MonitorRuntime>) -> bool {
    rt.is_some_and(|rt| {
        rt.wallpaper_id.as_deref() == Some(window.wallpaper_id.as_str())
            && matches!(&rt.active, ActiveRuntime::Webview { kind, label, .. }
                if *kind == window.kind && *label == window.label)
    })
}

fn keep_outgoing_window(rt: Option<&MonitorRuntime>, has_renderer: bool) -> bool {
    rt.is_some_and(|rt| match &rt.active {
        // show() owns the handoff between desktop webviews on Mac.
        ActiveRuntime::Webview { .. } => cfg!(target_os = "macos"),
        ActiveRuntime::Photo { path: Some(_) } | ActiveRuntime::Scene(_) => {
            has_renderer && rt.generation != rt.presented_generation
        }
        _ => false,
    })
}

/// Logical rect `(x, y, width, height)` of a monitor.
pub type LogicalRect = (f64, f64, f64, f64);

/// What happens to one open wallpaper window after the display configuration changed.
#[derive(Debug, Clone, PartialEq)]
pub enum GeometryUpdate {
    /// Its monitor is gone: close the window (restored when the monitor comes back).
    Close { monitor_id: String },
    /// Move / resize it to the monitor's current rect and attach it to the desktop again.
    Place {
        monitor_id: String,
        logical: LogicalRect,
        physical: (i32, i32, u32, u32),
    },
}

/// Plans [`GeometryUpdate`]s for the open windows `(monitor id, current logical rect)`.
/// `force` places every window that still has a monitor (desktop surfaces were recreated);
/// otherwise only windows whose rect changed are placed. An empty `live` list (the monitor
/// query failed) changes nothing.
pub fn plan_geometry_updates(
    windows: &[(String, LogicalRect)],
    live: &[LiveMonitor],
    force: bool,
) -> Vec<GeometryUpdate> {
    if live.is_empty() {
        return Vec::new();
    }
    windows
        .iter()
        .filter_map(|(monitor_id, rect)| {
            let Some(m) = live.iter().find(|m| &m.id == monitor_id) else {
                return Some(GeometryUpdate::Close {
                    monitor_id: monitor_id.clone(),
                });
            };
            let logical = m.logical_rect();
            (force || logical != *rect).then(|| GeometryUpdate::Place {
                monitor_id: monitor_id.clone(),
                logical,
                physical: (m.x, m.y, m.width, m.height),
            })
        })
        .collect()
}

/// `true` when the page should be suspended by the OS webview (WebView2 `TrySuspend`): the window
/// is covered by a fullscreen / maximized app or the display is asleep (`suspended`). A user
/// pause and Battery Saver keep the page alive: only the JS pause applies, so the frame stays
/// visible and resumes instantly.
pub fn os_suspend_wanted(policy: MonitorPlaybackPolicy, suspended: bool) -> bool {
    suspended
        || matches!(
            policy,
            MonitorPlaybackPolicy::PausedFullscreen
                | MonitorPlaybackPolicy::PausedMaximized
                | MonitorPlaybackPolicy::PausedDisplaySleep
        )
}

/// `(label, suspend)` for every loaded window whose OS-suspend state differs from the last one
/// applied. Input is `(label, loaded, wanted, applied)`.
pub fn plan_os_suspend(windows: &[(String, bool, bool, bool)]) -> Vec<(String, bool)> {
    windows
        .iter()
        .filter(|(_, loaded, wanted, applied)| *loaded && wanted != applied)
        .map(|(label, _, wanted, _)| (label.clone(), *wanted))
        .collect()
}

/// How often windows that should be awake get `Resume` + `SetIsVisible(true)` again (the safety
/// net for a suspend that raced with an uncover or failed half way).
const REASSERT_INTERVAL: Duration = Duration::from_secs(15);
/// A suspend that was not confirmed is not requested again for this long.
const SUSPEND_RETRY: Duration = Duration::from_secs(15);

/// Labels of loaded windows that must not be suspended or hidden. Input is
/// `(label, loaded, wanted)` where `wanted` is [`os_suspend_wanted`].
pub fn plan_reassert(windows: &[(String, bool, bool)]) -> Vec<String> {
    windows
        .iter()
        .filter(|(_, loaded, wanted)| *loaded && !*wanted)
        .map(|(label, _, _)| label.clone())
        .collect()
}

/// `true` when a suspend may be requested: none is pending or the retry delay has passed.
pub fn suspend_gate_open(not_before: Option<Instant>, now: Instant) -> bool {
    not_before.is_none_or(|t| now >= t)
}

/// Per window: sequence number of the last suspend / resume request (a late completion of an
/// older request is ignored) and the earliest time the next suspend may be requested.
type SuspendGate = (u64, Option<Instant>);
static SUSPEND_GATE: LazyLock<Mutex<HashMap<String, SuspendGate>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether a suspend request for `label` may go out now.
fn suspend_allowed(label: &str) -> bool {
    let gate = SUSPEND_GATE.lock();
    suspend_gate_open(gate.get(label).and_then(|g| g.1), Instant::now())
}

/// Registers a new request for `label` and returns its sequence number. A suspend closes the gate
/// until its completion (or [`SUSPEND_RETRY`]); a resume opens it.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn next_suspend_seq(label: &str, suspend: bool) -> u64 {
    let mut gate = SUSPEND_GATE.lock();
    let entry = gate.entry(label.to_string()).or_insert((0, None));
    entry.0 += 1;
    entry.1 = suspend.then(|| Instant::now() + SUSPEND_RETRY);
    entry.0
}

/// Whether `seq` is still the newest request for `label`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_latest_suspend(label: &str, seq: u64) -> bool {
    SUSPEND_GATE.lock().get(label).is_some_and(|g| g.0 == seq)
}

/// Suspends (`true`) or resumes (`false`) the WebView2 of `label`. WebView2 only suspends a hidden
/// webview, so it is hidden first and shown again after the resume. Requests for one window go
/// through `with_webview` in order (main thread), each tagged with a sequence number so a late
/// `TrySuspend` completion cannot overwrite the state of a newer resume. The completion callback
/// records `os_suspended`. Other platforms: no-op (the JS pause is all that is applied there).
#[cfg(target_os = "windows")]
fn set_os_suspended(app: &AppHandle, label: &str, suspend: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use webview2_com::TrySuspendCompletedHandler;
    use windows::core::Interface;

    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let seq = next_suspend_seq(label, suspend);
    let name = label.to_string();
    let result = window.with_webview(move |webview| unsafe {
        let controller = webview.controller();
        let Ok(core) = controller.CoreWebView2() else {
            tracing::debug!("webview {name}: CoreWebView2 unavailable (suspend={suspend})");
            return;
        };
        let Ok(core3) = core.cast::<ICoreWebView2_3>() else {
            tracing::debug!("webview {name}: ICoreWebView2_3 unavailable (suspend={suspend})");
            return;
        };
        if suspend {
            let hidden = controller.SetIsVisible(false);
            let done_name = name.clone();
            let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
                let ok = result.is_ok() && suspended;
                tracing::debug!(
                    "webview {done_name}: TrySuspend completed (ok={ok}, result={result:?})"
                );
                if is_latest_suspend(&done_name, seq) {
                    if let Some(w) = WINDOWS.lock().values_mut().find(|w| w.label == done_name) {
                        w.os_suspended = ok;
                    }
                }
                Ok(())
            }));
            let started = core3.TrySuspend(&handler);
            tracing::debug!(
                "webview {name}: SetIsVisible(false)={hidden:?} TrySuspend={started:?}"
            );
        } else {
            let resumed = core3.Resume();
            let shown = controller.SetIsVisible(true);
            tracing::debug!("webview {name}: Resume={resumed:?} SetIsVisible(true)={shown:?}");
        }
    });
    if let Err(err) = result {
        tracing::debug!("webview suspend on {label} failed: {err}");
    }
}

#[cfg(not(target_os = "windows"))]
fn set_os_suspended(_app: &AppHandle, _label: &str, _suspend: bool) {}

// ---------------------------------------------------------------------------------------------
// Window registry
// ---------------------------------------------------------------------------------------------

struct WebWindow {
    label: String,
    url: String,
    kind: WebviewKind,
    wallpaper_id: String,
    audio_reactive: bool,
    loaded: bool,
    preview_ready_at: Instant,
    paused_sent: Option<bool>,
    /// The OS webview is suspended (covered / hidden only, see [`os_suspend_wanted`]).
    os_suspended: bool,
    /// Last volume / mute script sent to the page (`None` = never sent since load).
    audio_sent: Option<String>,
    /// Logical rect of the window (its monitor's rect when it was last placed).
    logical: LogicalRect,
    /// CSS pixel size of the page (logical monitor size) for cursor coordinates.
    css_size: (f64, f64),
    last_cursor: Option<CursorSample>,
}

static APP: OnceLock<AppHandle> = OnceLock::new();
/// The host, so page-load callbacks can wake the bridge.
static BRIDGE_HOST: OnceLock<Arc<WallpaperHost>> = OnceLock::new();

fn wake_bridge() {
    if let Some(host) = BRIDGE_HOST.get() {
        host.wake();
    }
}

/// How long the bridge sleeps between passes: 16 ms only while a Web page is live (cursor and
/// audio frames have no push signal), otherwise it waits for host changes.
pub fn bridge_wait(live_pages: bool, has_windows: bool) -> Duration {
    if live_pages {
        ACTIVE_TICK
    } else if has_windows {
        WINDOW_TICK
    } else {
        IDLE_TICK
    }
}
/// `monitor id -> window`.
static WINDOWS: LazyLock<Mutex<HashMap<String, WebWindow>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn eval(app: &AppHandle, label: &str, js: &str) {
    if let Some(window) = app.get_webview_window(label) {
        if let Err(err) = window.eval(js) {
            tracing::debug!("eval on {label} failed: {err}");
        }
    }
}

fn destroy(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        if let Err(err) = window.destroy() {
            tracing::warn!("closing wallpaper window {label}: {err}");
        }
    }
}

/// What [`show`] creates for one monitor.
#[derive(Debug, Clone)]
pub struct WebviewSpec {
    pub monitor_id: String,
    pub kind: WebviewKind,
    pub wallpaper_id: String,
    /// `web_wallpaper_url` / `video_host_url` result.
    pub url: String,
    pub audio_reactive: bool,
    /// Sent after every page load: full property set (Web) / adjustments (Video).
    pub on_load_scripts: Vec<String>,
}

/// Creates (or reuses) the desktop webview window of `spec.monitor_id`. Fails when the monitor is
/// not connected (the window would otherwise land on another screen); the scheduler restores it
/// once the monitor is back.
pub async fn show(
    app: &AppHandle,
    host: &WallpaperHost,
    vfs: Arc<WeVirtualFileSystem>,
    spec: WebviewSpec,
    activate: impl FnOnce(String) -> Result<(), String>,
) -> Result<(), String> {
    let _ = APP.set(app.clone());
    let base_label = wallpaper_window_label(&spec.monitor_id);
    let url = tauri::Url::parse(&spec.url).map_err(|e| format!("invalid wallpaper URL: {e}"))?;
    let monitor = crate::commands::monitor::live_monitors(app)
        .into_iter()
        .find(|m| m.id == spec.monitor_id)
        .ok_or_else(|| format!("Monitor '{}' is not connected", spec.monitor_id))?;

    // Same wallpaper already open: just resend the scripts.
    let reuse = WINDOWS
        .lock()
        .get(&spec.monitor_id)
        .filter(|w| w.kind == spec.kind && w.wallpaper_id == spec.wallpaper_id && w.url == spec.url)
        .map(|w| w.label.clone())
        .filter(|label| app.get_webview_window(label).is_some());
    if let Some(label) = reuse {
        host.set_webview_vfs(&label, &spec.wallpaper_id, vfs);
        activate(label.clone())?;
        if let Some(w) = WINDOWS.lock().get_mut(&spec.monitor_id) {
            // The page keeps its volume, but re-sent scripts may reset it: resend ours.
            w.audio_sent = None;
        }
        for js in &spec.on_load_scripts {
            eval(app, &label, js);
        }
        emit_loaded(app, &spec.monitor_id, Some(spec.wallpaper_id.clone()));
        wake_bridge();
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    let label = {
        static NEXT_WINDOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "{base_label}-{}",
            NEXT_WINDOW.fetch_add(1, Ordering::Relaxed)
        )
    };
    #[cfg(not(target_os = "macos"))]
    let label = {
        WINDOWS.lock().remove(&spec.monitor_id);
        if app.get_webview_window(&base_label).is_some() {
            destroy(app, &base_label);
            // Destroy completes on the event loop before this label can be reused.
            for _ in 0..40 {
                if app.get_webview_window(&base_label).is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        base_label
    };

    let (lx, ly, lw, lh) = monitor.logical_rect();

    let load_label = label.clone();
    let load_scripts = spec.on_load_scripts.clone();
    let loaded = Arc::new(AtomicBool::new(false));
    let page_loaded = loaded.clone();
    #[cfg(target_os = "macos")]
    let kind = spec.kind;
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::CustomProtocol(url))
        // Wallpaper pages are untrusted: no popups or extra windows.
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .title("TruongPaper Wallpaper")
        .decorations(false)
        .skip_taskbar(true)
        .focused(false)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .position(lx, ly)
        .inner_size(lw, lh)
        .on_page_load(move |window, payload| {
            if payload.event() != tauri::webview::PageLoadEvent::Finished {
                return;
            }
            page_loaded.store(true, Ordering::Release);
            if let Some(w) = WINDOWS.lock().values_mut().find(|w| w.label == load_label) {
                w.loaded = true;
                // The bridge re-sends the pause state for the fresh page.
                w.paused_sent = None;
                w.os_suspended = false;
            }
            wake_bridge();
            for js in &load_scripts {
                if let Err(err) = window.eval(js) {
                    tracing::debug!("initial script on {}: {err}", window.label());
                }
            }
            #[cfg(target_os = "macos")]
            if kind == WebviewKind::Web {
                let _ = window.eval("window.__truongpaper_signal_ready?.()");
            }
        });
    #[cfg(target_os = "macos")]
    let (ready_sender, ready) = tokio::sync::oneshot::channel();
    #[cfg(target_os = "macos")]
    {
        let ready_sender = Mutex::new(Some(ready_sender));
        builder = builder
            .transparent(true)
            .initialization_script(STAGE_SCRIPT)
            .on_navigation(move |url| {
                if url.scheme() == "wpe"
                    && url.host_str() == Some("localhost")
                    && url.path() == "/__truongpaper_ready__"
                {
                    if let Some(sender) = ready_sender.lock().take() {
                        let _ = sender.send(());
                    }
                    return false;
                }
                wallpaper_navigation_allowed(url, true)
            });
    }
    // WebView2 reports top-level navigations only, so the strict form applies.
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.on_navigation(|url| wallpaper_navigation_allowed(url, false));
    }
    if spec.kind == WebviewKind::Web {
        builder = builder
            .initialization_script(WEB_PLAYBACK_PRELOAD_JS)
            .initialization_script(truongpaper_platform::WEB_WALLPAPER_PRELOAD_JS);
    }
    #[cfg(target_os = "windows")]
    {
        // Webviews with different browser arguments need their own data directory.
        builder =
            builder.additional_browser_args(truongpaper_platform::WEBVIEW2_WALLPAPER_BROWSER_ARGS);
        if let Ok(dir) = app.path().app_data_dir() {
            builder = builder.data_directory(dir.join("wallpaper-webview"));
        }
    }
    host.set_webview_vfs(&label, &spec.wallpaper_id, vfs);
    let window = match builder.build() {
        Ok(window) => window,
        Err(error) => {
            host.remove_webview_vfs(&label);
            return Err(format!("could not open the wallpaper window: {error}"));
        }
    };

    let physical = (monitor.x, monitor.y, monitor.width, monitor.height);
    #[cfg(target_os = "macos")]
    {
        attach_to_desktop(app, window.clone(), physical);
        if !matches!(
            tokio::time::timeout(Duration::from_secs(12), ready).await,
            Ok(Ok(()))
        ) {
            host.remove_webview_vfs(&label);
            destroy(app, &label);
            return Err(
                "The new wallpaper could not load. The previous wallpaper is still active.".into(),
            );
        }
    }
    if let Err(error) = activate(label.clone()) {
        host.remove_webview_vfs(&label);
        destroy(app, &label);
        return Err(error);
    }

    #[cfg(target_os = "macos")]
    let preview_ready_at = Instant::now() + REVEAL_DURATION;
    #[cfg(not(target_os = "macos"))]
    let preview_ready_at = Instant::now();
    let _previous = WINDOWS.lock().insert(
        spec.monitor_id.clone(),
        WebWindow {
            label: label.clone(),
            url: spec.url,
            kind: spec.kind,
            wallpaper_id: spec.wallpaper_id.clone(),
            audio_reactive: spec.audio_reactive,
            loaded: loaded.load(Ordering::Acquire),
            preview_ready_at,
            paused_sent: None,
            os_suspended: false,
            audio_sent: None,
            logical: (lx, ly, lw, lh),
            css_size: (lw, lh),
            last_cursor: None,
        },
    );
    #[cfg(target_os = "macos")]
    {
        attach_to_desktop(app, window, physical);
        eval(app, &label, "window.__truongpaper_reveal?.()");
        if let Some(previous) = _previous {
            eval(
                app,
                &previous.label,
                "window.__truongpaper_set_volume?.(0); document.querySelectorAll('audio,video').forEach(el => { el.muted = true; })",
            );
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(REVEAL_DURATION).await;
                destroy(&app, &previous.label);
            });
        }
    }
    #[cfg(not(target_os = "macos"))]
    attach_to_desktop(app, window, physical);
    emit_loaded(app, &spec.monitor_id, Some(spec.wallpaper_id));
    wake_bridge();
    Ok(())
}

pub fn can_show_preview(label: &str) -> bool {
    WINDOWS
        .lock()
        .values()
        .any(|window| window.label == label && Instant::now() >= window.preview_ready_at)
}

/// Moves the window behind the desktop icons (main thread).
#[cfg(target_os = "macos")]
fn attach_to_desktop(
    app: &AppHandle,
    window: tauri::WebviewWindow,
    _physical: (i32, i32, u32, u32),
) {
    let dispatched = app.run_on_main_thread(move || {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        match window.ns_window() {
            Ok(ptr) if !ptr.is_null() => {
                // SAFETY: Tauri returns the live NSWindow of this webview window; we are on the
                // main thread and only borrow it for the duration of the call.
                let ns_window: &objc2_app_kit::NSWindow =
                    unsafe { &*(ptr as *const objc2_app_kit::NSWindow) };
                truongpaper_platform::mac_desktop::attach_window_to_desktop(mtm, ns_window);
            }
            Ok(_) => tracing::warn!("wallpaper window has no NSWindow"),
            Err(err) => tracing::warn!("wallpaper window NSWindow: {err}"),
        }
    });
    if let Err(err) = dispatched {
        tracing::warn!("could not attach the wallpaper window: {err}");
    }
}

#[cfg(target_os = "windows")]
fn attach_to_desktop(
    app: &AppHandle,
    window: tauri::WebviewWindow,
    physical: (i32, i32, u32, u32),
) {
    let (x, y, w, h) = physical;
    let dispatched = app.run_on_main_thread(move || {
        let Ok(tauri_hwnd) = window.hwnd() else {
            tracing::warn!("wallpaper window has no HWND");
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(tauri_hwnd.0 as *mut std::ffi::c_void);
        let rect = windows::Win32::Foundation::RECT {
            left: x,
            top: y,
            right: x + w as i32,
            bottom: y + h as i32,
        };
        // SAFETY: `hwnd` belongs to this process; desktop handles are freshly queried.
        unsafe {
            match truongpaper_platform::win_desktop::setup_desktop_layer() {
                Some(desktop) => {
                    if !truongpaper_platform::win_desktop::attach_hwnd_to_desktop(
                        hwnd, &desktop, rect,
                    ) {
                        tracing::warn!("attaching the wallpaper window to the desktop failed");
                    }
                }
                None => tracing::warn!("Progman/WorkerW not found for the wallpaper window"),
            }
        }
    });
    if let Err(err) = dispatched {
        tracing::warn!("could not attach the wallpaper window: {err}");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn attach_to_desktop(
    _app: &AppHandle,
    window: tauri::WebviewWindow,
    _physical: (i32, i32, u32, u32),
) {
    let _ = window.show();
}

/// Applies `updates` to the open windows: closes windows whose monitor is gone, moves / resizes
/// the others to their monitor's current rect (also updating the CSS size used for cursor
/// coordinates) and attaches them to the desktop again.
fn apply_geometry_updates(app: &AppHandle, updates: Vec<GeometryUpdate>) {
    for update in updates {
        match update {
            GeometryUpdate::Close { monitor_id } => {
                tracing::info!("monitor {monitor_id} disconnected; closing its wallpaper window");
                close_monitor(&monitor_id);
                // Restored by the scheduler when the monitor is seen again, even if it comes
                // back before the scheduler's next monitor refresh.
                crate::scheduler::note_monitor_disconnected(&monitor_id);
            }
            GeometryUpdate::Place {
                monitor_id,
                logical,
                physical,
            } => {
                let label = {
                    let mut windows = WINDOWS.lock();
                    let Some(w) = windows.get_mut(&monitor_id) else {
                        continue;
                    };
                    w.logical = logical;
                    w.css_size = (logical.2, logical.3);
                    w.label.clone()
                };
                let Some(window) = app.get_webview_window(&label) else {
                    continue;
                };
                // Windows: `attach_hwnd_to_desktop` positions the child window in physical
                // pixels itself; elsewhere the top-level window is moved here.
                #[cfg(not(target_os = "windows"))]
                {
                    let (x, y, w, h) = logical;
                    if let Err(err) = window.set_position(tauri::LogicalPosition::new(x, y)) {
                        tracing::warn!("moving wallpaper window {label}: {err}");
                    }
                    if let Err(err) = window.set_size(tauri::LogicalSize::new(w, h)) {
                        tracing::warn!("resizing wallpaper window {label}: {err}");
                    }
                }
                attach_to_desktop(app, window, physical);
            }
        }
    }
}

/// Current `(monitor id, logical rect)` of every open window.
fn window_rects() -> Vec<(String, LogicalRect)> {
    WINDOWS
        .lock()
        .iter()
        .map(|(id, w)| (id.clone(), w.logical))
        .collect()
}

/// Re-places every open wallpaper window after the desktop surfaces were recreated (display
/// hotplug, resolution / scale change) or explorer.exe restarted: windows of disconnected
/// monitors are closed, the others follow their monitor's rect and are attached again.
pub fn reattach_all() {
    let Some(app) = APP.get() else {
        return;
    };
    let monitors = live_monitors_cached(app);
    note_display_set(&monitors);
    apply_geometry_updates(app, plan_geometry_updates(&window_rects(), &monitors, true));
}

/// Monitor ids seen by the last display check.
static KNOWN_MONITORS: Mutex<Option<Vec<String>>> = parking_lot::const_mutex(None);

/// Tells the scheduler about displays that appeared, came back or vanished since the last check,
/// so it re-applies their persisted Web / Video source (this module never creates a window on
/// its own: `plan_geometry_updates` only moves or closes existing ones).
fn note_display_set(live: &[LiveMonitor]) {
    if live.is_empty() {
        return;
    }
    let ids: Vec<String> = live.iter().map(|m| m.id.clone()).collect();
    let previous = KNOWN_MONITORS.lock().replace(ids.clone());
    if let Some(previous) = previous {
        for id in hotplug_restore_ids(&previous, &ids) {
            crate::scheduler::note_monitor_disconnected(&id);
        }
    }
}

/// Moves / closes windows whose monitor changed since the last check (bridge thread; also
/// covers display changes when no desktop renderer is attached).
fn follow_display_changes(app: &AppHandle) {
    let monitors = live_monitors_cached(app);
    note_display_set(&monitors);
    let rects = window_rects();
    if rects.is_empty() {
        return;
    }
    apply_geometry_updates(app, plan_geometry_updates(&rects, &monitors, false));
}

/// Closes the wallpaper window of `monitor_id` (no-op when none is open).
pub fn close_monitor(monitor_id: &str) {
    let removed = WINDOWS.lock().remove(monitor_id);
    if let (Some(app), Some(w)) = (APP.get(), removed) {
        destroy(app, &w.label);
    }
}

/// Closes every wallpaper window (shutdown).
pub fn close_all() {
    let windows: Vec<WebWindow> = WINDOWS.lock().drain().map(|(_, w)| w).collect();
    if let Some(app) = APP.get() {
        for w in windows {
            destroy(app, &w.label);
        }
    }
}

/// Labels of the open windows showing `wallpaper_id` with `kind`.
fn labels_for(wallpaper_id: &str, kind: WebviewKind) -> Vec<String> {
    WINDOWS
        .lock()
        .values()
        .filter(|w| w.wallpaper_id == wallpaper_id && w.kind == kind)
        .map(|w| w.label.clone())
        .collect()
}

/// Runs `js` in every Web (`kind = Web`) or Video window showing `wallpaper_id`.
pub fn eval_for_wallpaper(wallpaper_id: &str, kind: WebviewKind, js: &str) {
    let Some(app) = APP.get() else {
        return;
    };
    for label in labels_for(wallpaper_id, kind) {
        eval(app, &label, js);
    }
}

// ---------------------------------------------------------------------------------------------
// Bridge thread
// ---------------------------------------------------------------------------------------------

/// Starts the webview bridge thread (also owns the audio-worker demand for scenes).
pub fn start(app: AppHandle, host: Arc<WallpaperHost>) {
    let _ = APP.set(app.clone());
    let _ = BRIDGE_HOST.set(Arc::clone(&host));
    let spawned = std::thread::Builder::new()
        .name("truongpaper-webview-bridge".into())
        .spawn(move || bridge_main(app, host));
    if let Err(err) = spawned {
        tracing::error!("failed to start the webview bridge thread: {err}");
    }
}

/// A loaded, playing Web page (snapshot taken once per bridge tick).
struct LiveWebPage {
    monitor_id: String,
    label: String,
    audio_reactive: bool,
    css_size: (f64, f64),
    last_cursor: Option<CursorSample>,
}

fn bridge_main(app: AppHandle, host: Arc<WallpaperHost>) {
    let mut throttle = AudioPushThrottle::new();
    let mut cursor = CursorPoller::new();
    let mut geoms = Vec::new();
    let mut geoms_at: Option<Instant> = None;
    let mut display_check_at: Option<Instant> = None;
    let mut last_epoch = u64::MAX;
    let mut last_suspended = false;
    let mut reassert_at = Instant::now();

    while !host.is_shutting_down() {
        let epoch = host.epoch();
        let suspended = crate::power::rendering_suspended();
        // Pause / audio / source bookkeeping only runs when the host changed (or the display
        // slept); a live Web page below still ticks on its own.
        let changed = epoch != last_epoch || suspended != last_suspended;
        last_epoch = epoch;
        last_suspended = suspended;
        let mut has_windows = !WINDOWS.lock().is_empty();
        if changed {
            // 1. Close windows whose monitor switched to another source.
            let stale = {
                // Host locks are taken before `WINDOWS` everywhere else, so query the host
                // first and only then lock `WINDOWS`.
                let ids: Vec<String> = WINDOWS.lock().keys().cloned().collect();
                let runtimes: HashMap<String, Option<MonitorRuntime>> = ids
                    .into_iter()
                    .map(|id| {
                        let rt = host.monitor(&id);
                        (id, rt)
                    })
                    .collect();
                let attached = host.live_renderer_attached();
                let mut stale = Vec::new();
                WINDOWS.lock().retain(|id, window| {
                    // A window added since the snapshot is judged on the next pass.
                    let Some(rt) = runtimes.get(id) else {
                        return true;
                    };
                    let rt = rt.as_ref();
                    if window_matches_host(window, rt) || keep_outgoing_window(rt, attached) {
                        true
                    } else {
                        stale.push(window.label.clone());
                        false
                    }
                });
                stale
            };
            for label in stale {
                destroy(&app, &label);
            }

            // 2. Audio loopback worker runs only while an audio-reactive wallpaper plays.
            crate::audio_bridge::set_demand(if crate::power::rendering_suspended() {
                None
            } else {
                host.audio_demand()
            });

            // 3. Pause / resume (user pause + Smart Pause).
            let states: Vec<WindowPauseState> = WINDOWS
                .lock()
                .iter()
                .map(|(id, w)| WindowPauseState {
                    monitor_id: id.clone(),
                    label: w.label.clone(),
                    kind: w.kind,
                    loaded: w.loaded,
                    paused_sent: w.paused_sent,
                })
                .collect();
            let playing = |id: &str| !suspended && host.monitor_should_render(id);
            // 3a. Wake pages that are no longer covered / hidden before the JS pause state goes
            // out, then suspend the covered ones after it.
            let suspend_plan: Vec<(String, bool)> = {
                let windows: Vec<(String, bool, bool, bool)> = WINDOWS
                    .lock()
                    .iter()
                    .map(|(id, w)| {
                        let policy = host
                            .monitor(id)
                            .map_or(MonitorPlaybackPolicy::Playing, |rt| rt.policy);
                        let wanted = os_suspend_wanted(policy, suspended);
                        (w.label.clone(), w.loaded, wanted, w.os_suspended)
                    })
                    .collect();
                plan_os_suspend(&windows)
            };
            for (label, suspend) in suspend_plan.iter().filter(|(_, s)| !*s) {
                set_os_suspended(&app, label, *suspend);
            }
            for (label, js, paused) in plan_pause_updates(&states, playing) {
                eval(&app, &label, &js);
                if let Some(w) = WINDOWS.lock().values_mut().find(|w| w.label == label) {
                    w.paused_sent = Some(paused);
                }
            }

            for (label, suspend) in suspend_plan.iter().filter(|(_, s)| *s) {
                // A suspend that was not confirmed yet (or failed) is retried after a delay,
                // not on every pass.
                if cfg!(target_os = "windows") && !suspend_allowed(label) {
                    continue;
                }
                set_os_suspended(&app, label, *suspend);
                if !cfg!(target_os = "windows") {
                    if let Some(w) = WINDOWS.lock().values_mut().find(|w| &w.label == label) {
                        w.os_suspended = true;
                    }
                }
            }
            // A resume is recorded at once; a suspend is recorded by its completion callback.
            for (label, _) in suspend_plan.iter().filter(|(_, s)| !*s) {
                if let Some(w) = WINDOWS.lock().values_mut().find(|w| &w.label == label) {
                    w.os_suspended = false;
                }
            }

            // 3b. Volume: one sound owner per wallpaper, scaled by the display volume.
            let audio_states: Vec<WindowAudioState> = WINDOWS
                .lock()
                .iter()
                .map(|(id, w)| WindowAudioState {
                    monitor_id: id.clone(),
                    label: w.label.clone(),
                    kind: w.kind,
                    loaded: w.loaded,
                    audio_sent: w.audio_sent.clone(),
                })
                .collect();
            has_windows = !audio_states.is_empty();
            if audio_states.iter().any(|w| w.loaded) {
                let adjustments = host.webview_audio_adjustments(playing);
                for (label, script) in plan_audio_updates(&audio_states, &adjustments) {
                    eval(&app, &label, &script);
                    if let Some(w) = WINDOWS.lock().values_mut().find(|w| w.label == label) {
                        w.audio_sent = Some(script);
                    }
                }
            }
        }

        // 1b. Follow display changes (moved / resized / disconnected monitors); the monitor
        // list is shared and cached, so this is a cheap comparison between refreshes.
        if display_check_at.is_none_or(|t| t.elapsed() >= GEOMETRY_REFRESH) {
            follow_display_changes(&app);
            display_check_at = Some(Instant::now());
        }

        // 3c. Windows safety net: a suspend that raced with an uncover (or a failed
        // `TrySuspend` after the page was hidden) must not leave the wallpaper black.
        if cfg!(target_os = "windows") && reassert_at.elapsed() >= REASSERT_INTERVAL {
            reassert_at = Instant::now();
            let windows: Vec<(String, bool, bool)> = WINDOWS
                .lock()
                .iter()
                .map(|(id, w)| {
                    let policy = host
                        .monitor(id)
                        .map_or(MonitorPlaybackPolicy::Playing, |rt| rt.policy);
                    (
                        w.label.clone(),
                        w.loaded,
                        os_suspend_wanted(policy, suspended),
                    )
                })
                .collect();
            for label in plan_reassert(&windows) {
                set_os_suspended(&app, &label, false);
                if let Some(w) = WINDOWS.lock().values_mut().find(|w| w.label == label) {
                    w.os_suspended = false;
                }
            }
        }

        let playing = |id: &str| !suspended && host.monitor_should_render(id);
        // 4. Audio frames + cursor events for playing Web pages.
        let live: Vec<LiveWebPage> = WINDOWS
            .lock()
            .iter()
            .filter(|(id, w)| w.kind == WebviewKind::Web && w.loaded && playing(id))
            .map(|(id, w)| LiveWebPage {
                monitor_id: id.clone(),
                label: w.label.clone(),
                audio_reactive: w.audio_reactive,
                css_size: w.css_size,
                last_cursor: w.last_cursor,
            })
            .collect();
        if !live.is_empty() {
            let now = Instant::now();
            if live.iter().any(|p| p.audio_reactive) && throttle.ready(now) {
                if let Some(frame) = crate::audio_bridge::latest_frame() {
                    let js =
                        audio_dispatch_js(&pack_web_audio_frame(&frame.left_64, &frame.right_64));
                    for page in live.iter().filter(|p| p.audio_reactive) {
                        eval(&app, &page.label, &js);
                    }
                }
            }
            if geoms_at.is_none_or(|t| t.elapsed() >= GEOMETRY_REFRESH) {
                geoms = monitor_geometries(&app);
                geoms_at = Some(now);
            }
            for page in &live {
                let sample = cursor.poll(&geoms, &page.monitor_id);
                let (w, h) = page.css_size;
                for (kind, pos) in cursor_transitions(page.last_cursor, sample) {
                    match cursor_event_js(kind, f64::from(pos[0]) * w, f64::from(pos[1]) * h) {
                        Ok(js) => eval(&app, &page.label, &js),
                        Err(err) => tracing::debug!("cursor event: {err}"),
                    }
                }
                if let Some(win) = WINDOWS.lock().get_mut(&page.monitor_id) {
                    win.last_cursor = sample;
                }
            }
        }

        host.wait_for_change(epoch, bridge_wait(!live.is_empty(), has_windows));
    }
    close_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UniversalAdjustments;

    #[test]
    fn navigation_is_limited_to_our_own_origin() {
        let url = |s: &str| tauri::Url::parse(s).unwrap();
        // Top-level (WebView2): only the app origin.
        for ok in [
            "wpe://localhost/index.html",
            "http://wpe.localhost/index.html?x=1",
            "https://wpe.localhost/a/b.html",
            "about:blank",
        ] {
            assert!(wallpaper_navigation_allowed(&url(ok), false), "{ok}");
        }
        for bad in [
            "https://example.com/",
            "http://wpe.localhost.evil.example/",
            "wpe://evil/index.html",
            "file:///etc/passwd",
            "data:text/html,hi",
            "blob:https://wpe.localhost/1",
            "about:srcdoc",
            "tel:123",
        ] {
            assert!(!wallpaper_navigation_allowed(&url(bad), false), "{bad}");
        }
        // WKWebView also reports iframe navigations: embeds work, local files and app schemes
        // stay blocked.
        for ok in [
            "https://player.example/embed",
            "data:text/html,hi",
            "wpe://localhost/x",
        ] {
            assert!(wallpaper_navigation_allowed(&url(ok), true), "{ok}");
        }
        for bad in [
            "file:///etc/passwd",
            "tel:123",
            "wpe://evil/x",
            "javascript:alert(1)",
        ] {
            assert!(!wallpaper_navigation_allowed(&url(bad), true), "{bad}");
        }
    }

    #[test]
    fn bridge_ticks_fast_only_for_live_web_pages() {
        assert_eq!(bridge_wait(true, true), ACTIVE_TICK);
        assert_eq!(bridge_wait(false, true), WINDOW_TICK);
        assert_eq!(bridge_wait(false, false), IDLE_TICK);
        assert!(WINDOW_TICK >= Duration::from_secs(1));
    }

    #[test]
    fn labels_are_sanitized() {
        assert_eq!(wallpaper_window_label("DISPLAY-1"), "wallpaper-DISPLAY-1");
        assert_eq!(
            wallpaper_window_label("LG UltraFine #2"),
            "wallpaper-LG_UltraFine__2"
        );
        assert_eq!(
            wallpaper_window_label("\\\\.\\DISPLAY1"),
            "wallpaper-____DISPLAY1"
        );
        assert_eq!(wallpaper_window_label("Écran"), "wallpaper-_cran");
        let label = wallpaper_window_label("a/b:c");
        assert!(label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn pause_scripts_per_kind() {
        assert_eq!(
            pause_script(WebviewKind::Video, true),
            "window.__truongpaper_set_paused(true);"
        );
        assert_eq!(
            pause_script(WebviewKind::Video, false),
            "window.__truongpaper_set_paused(false);"
        );
        assert_eq!(
            pause_script(WebviewKind::Web, true),
            "window.__truongpaper_set_paused?.(true);"
        );
        assert_eq!(
            pause_script(WebviewKind::Web, false),
            "window.__truongpaper_set_paused?.(false);"
        );
    }

    #[test]
    fn pause_updates_go_only_to_loaded_windows_that_changed() {
        let w = |id: &str, kind, loaded, sent| WindowPauseState {
            monitor_id: id.into(),
            label: wallpaper_window_label(id),
            kind,
            loaded,
            paused_sent: sent,
        };
        let windows = [
            w("A", WebviewKind::Video, true, None),
            w("B", WebviewKind::Web, true, Some(false)),
            w("C", WebviewKind::Web, false, None),
            w("D", WebviewKind::Video, true, Some(true)),
        ];
        // A and D playing; B and C paused.
        let playing = |id: &str| id == "A" || id == "D";
        let plan = plan_pause_updates(&windows, playing);
        assert_eq!(
            plan,
            vec![
                ("wallpaper-A".to_string(), video_set_paused_js(false), false),
                (
                    "wallpaper-B".to_string(),
                    pause_script(WebviewKind::Web, true),
                    true
                ),
                ("wallpaper-D".to_string(), video_set_paused_js(false), false),
            ]
        );
        // Nothing to send once every loaded window is in sync.
        let synced = [
            w("A", WebviewKind::Video, true, Some(false)),
            w("B", WebviewKind::Web, true, Some(true)),
        ];
        assert!(plan_pause_updates(&synced, playing).is_empty());
    }

    #[test]
    fn audio_push_is_throttled_to_60_hz() {
        let mut t = AudioPushThrottle::new();
        let t0 = Instant::now();
        assert!(t.ready(t0));
        assert!(!t.ready(t0 + Duration::from_millis(5)));
        assert!(!t.ready(t0 + Duration::from_millis(16)));
        assert!(t.ready(t0 + Duration::from_millis(17)));
        assert!(!t.ready(t0 + Duration::from_millis(20)));
        assert!(t.ready(t0 + Duration::from_millis(40)));
        // Clock going backwards never panics and does not push.
        assert!(!t.ready(t0));
    }

    #[test]
    fn windows_follow_their_monitor_and_close_when_it_disconnects() {
        let monitor = |id: &str, x: i32, w: u32, scale: f64| LiveMonitor {
            id: id.into(),
            name: id.into(),
            width: w,
            height: 2160,
            x,
            y: 0,
            scale_factor: scale,
            is_primary: x == 0,
        };
        let windows = vec![
            ("BUILTIN".to_string(), (0.0, 0.0, 1920.0, 1080.0)),
            ("EXT".to_string(), (1920.0, 0.0, 3840.0, 2160.0)),
            ("GONE".to_string(), (0.0, 0.0, 1920.0, 1080.0)),
        ];
        // BUILTIN switched from 2x to 1x scaling; EXT is unchanged; GONE was unplugged.
        let live = vec![
            monitor("BUILTIN", 0, 3840, 1.0),
            monitor("EXT", 3840, 3840, 1.0),
        ];
        let plan = plan_geometry_updates(&windows, &live, false);
        assert_eq!(
            plan,
            vec![
                GeometryUpdate::Place {
                    monitor_id: "BUILTIN".into(),
                    logical: (0.0, 0.0, 3840.0, 2160.0),
                    physical: (0, 0, 3840, 2160),
                },
                GeometryUpdate::Place {
                    monitor_id: "EXT".into(),
                    logical: (3840.0, 0.0, 3840.0, 2160.0),
                    physical: (3840, 0, 3840, 2160),
                },
                GeometryUpdate::Close {
                    monitor_id: "GONE".into()
                },
            ]
        );
        // Unchanged windows are left alone unless the desktop surfaces were recreated.
        let settled = vec![("EXT".to_string(), live[1].logical_rect())];
        assert!(plan_geometry_updates(&settled, &live, false).is_empty());
        assert_eq!(plan_geometry_updates(&settled, &live, true).len(), 1);
        // A failed monitor query never closes everything.
        assert!(plan_geometry_updates(&windows, &[], true).is_empty());
    }

    #[test]
    fn windows_are_kept_only_while_the_host_shows_them() {
        let window = WebWindow {
            label: "wallpaper-A".into(),
            url: "wpe://localhost/index.html".into(),
            kind: WebviewKind::Web,
            wallpaper_id: "w1".into(),
            audio_reactive: false,
            loaded: true,
            preview_ready_at: Instant::now(),
            paused_sent: None,
            os_suspended: false,
            audio_sent: None,
            logical: (0.0, 0.0, 100.0, 100.0),
            css_size: (100.0, 100.0),
            last_cursor: None,
        };
        let host = WallpaperHost::new();
        host.set_source(
            "A",
            crate::models::WallpaperSource::Workshop {
                wallpaper_id: "w1".into(),
            },
            Some("w1".into()),
            Default::default(),
        );
        host.set_runtime(
            "A",
            ActiveRuntime::Webview {
                kind: WebviewKind::Web,
                label: "wallpaper-A".into(),
                preview_path: None,
            },
            UniversalAdjustments::default(),
            false,
        );
        assert!(window_matches_host(&window, host.monitor("A").as_ref()));
        // Same monitor now shows a video page: the web window must go.
        host.set_runtime(
            "A",
            ActiveRuntime::Webview {
                kind: WebviewKind::Video,
                label: "wallpaper-A".into(),
                preview_path: None,
            },
            UniversalAdjustments::default(),
            false,
        );
        assert!(!window_matches_host(&window, host.monitor("A").as_ref()));
        host.ensure_photo_runtime("A");
        assert!(!window_matches_host(&window, host.monitor("A").as_ref()));
        let generation = host.set_photo("A", "photo.jpg".into(), None);
        assert!(keep_outgoing_window(host.monitor("A").as_ref(), true));
        host.note_presented("A", generation - 1);
        assert!(keep_outgoing_window(host.monitor("A").as_ref(), true));
        host.note_presented("A", generation);
        assert!(!keep_outgoing_window(host.monitor("A").as_ref(), true));
        assert!(!window_matches_host(&window, None));
    }

    #[test]
    fn audio_updates_follow_owner_and_volume_changes() {
        let mut adj = std::collections::HashMap::new();
        let loud = UniversalAdjustments {
            volume: 0.5,
            ..UniversalAdjustments::default()
        };
        adj.insert("A".to_string(), loud);
        adj.insert(
            "B".to_string(),
            UniversalAdjustments {
                muted: true,
                ..loud
            },
        );
        let state = |id: &str, kind, loaded, sent: Option<String>| WindowAudioState {
            monitor_id: id.into(),
            label: format!("wallpaper-{id}"),
            kind,
            loaded,
            audio_sent: sent,
        };
        let windows = [
            state("A", WebviewKind::Web, true, None),
            state("B", WebviewKind::Web, true, None),
            state("C", WebviewKind::Web, false, None),
            state("D", WebviewKind::Web, true, None),
        ];
        let plan = plan_audio_updates(&windows, &adj);
        assert_eq!(
            plan,
            vec![
                ("wallpaper-A".to_string(), web_volume_script(0.5, false)),
                ("wallpaper-B".to_string(), web_volume_script(0.5, true)),
            ],
            "unloaded windows and windows without host state are skipped"
        );
        let synced = [
            state("A", WebviewKind::Web, true, Some(plan[0].1.clone())),
            state("B", WebviewKind::Web, true, Some(plan[1].1.clone())),
        ];
        assert!(plan_audio_updates(&synced, &adj).is_empty());
        let video = plan_audio_updates(&[state("A", WebviewKind::Video, true, None)], &adj);
        assert!(video[0]
            .1
            .starts_with("window.__truongpaper_apply_video_adjustments("));
    }

    #[test]
    fn os_suspend_only_for_covered_or_hidden_not_user_pause_or_battery() {
        use MonitorPlaybackPolicy::*;
        assert!(os_suspend_wanted(PausedFullscreen, false));
        assert!(os_suspend_wanted(PausedMaximized, false));
        assert!(os_suspend_wanted(PausedDisplaySleep, false));
        assert!(os_suspend_wanted(Playing, true));
        assert!(!os_suspend_wanted(Playing, false));
        assert!(!os_suspend_wanted(PausedBattery, false));
    }

    #[test]
    fn reassert_targets_loaded_windows_that_should_be_awake() {
        let windows = [
            ("a".to_string(), true, false),
            ("b".to_string(), true, true),
            ("c".to_string(), false, false),
        ];
        assert_eq!(plan_reassert(&windows), ["a"]);
    }

    #[test]
    fn suspend_requests_are_sequenced_and_retried_after_a_delay() {
        let label = "seq-test-window";
        let first = next_suspend_seq(label, true);
        assert!(is_latest_suspend(label, first));
        assert!(!suspend_allowed(label), "a pending suspend closes the gate");
        let resume = next_suspend_seq(label, false);
        assert!(
            !is_latest_suspend(label, first),
            "a late completion is ignored"
        );
        assert!(is_latest_suspend(label, resume));
        assert!(suspend_allowed(label), "a resume opens the gate");
        let now = Instant::now();
        assert!(suspend_gate_open(None, now));
        assert!(!suspend_gate_open(Some(now + SUSPEND_RETRY), now));
        assert!(suspend_gate_open(Some(now), now + SUSPEND_RETRY));
    }

    #[test]
    fn os_suspend_plan_lists_only_loaded_changes() {
        let w = |l: &str, loaded, wanted, applied| (l.to_string(), loaded, wanted, applied);
        let plan = plan_os_suspend(&[
            w("A", true, true, false),
            w("B", true, false, true),
            w("C", true, true, true),
            w("D", false, true, false),
        ]);
        assert_eq!(plan, vec![("A".into(), true), ("B".into(), false)]);
    }
}
