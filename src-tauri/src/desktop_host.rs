//! Desktop render surfaces and the `truongpaper-render` thread (docs/01 §1 thread model, §2
//! desktop windows, §3 compositor, §5 cursor, §6.1 pause semantics).
//!
//! - macOS: one `TruongDesktopWindow` per `NSScreen` at `kCGDesktopWindowLevel`
//!   (`truongpaper_platform::mac_desktop`), created on the main thread together with a `wgpu`
//!   surface on its content view (wgpu-hal requires the main thread to create the
//!   `CAMetalLayer`). Recreated on `ScreenParametersChanged`.
//! - Windows: one child `HWND` per monitor attached behind the desktop icons with
//!   `win_desktop::attach_hwnd_to_desktop` (classic `WorkerW` and 24H2 `Progman` paths), owned
//!   and pumped by the render thread. Recreated on `DesktopLifecycleEvent`s (TaskbarCreated,
//!   display topology changes).
//!
//! The render thread owns the `wgpu` instance/adapter/device/queue. Every frame (FPS cap from
//! `AppSettings::fps_cap`) and for every monitor whose runtime should render, photos go through
//! `CompositorPipeline::render_frame` (fit modes, transitions, Kawase backdrop, color grading)
//! and scenes run `SceneRuntime::step` + `SceneRenderer::render` into an offscreen texture that
//! is composited the same way. Paused monitors draw nothing. Web / Video / Application
//! wallpapers are separate windows (see `crate::wallpaper_webview`).
//!
//! `NSWindow.occlusionState` is never read (pitfall #2); Smart Pause (`smart_pause.rs`) decides.

use crate::commands::monitor::live_monitors;
use crate::host::{ActiveRuntime, MonitorRuntime, RenderEvent, SceneDescriptor, WallpaperHost};
use crate::models::UniversalAdjustments;
use crate::scene_runtime::{SceneRuntime, SoundRng};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use truongpaper_core::AppSettings;
use truongpaper_platform::ScreenRect;
use truongpaper_renderer::{
    clamp_transition_duration, downscale_for_display, next_view_id, transition_progress,
    upload_rgba_texture, CompositorPipeline, CompositorUniforms, FitMode, FrameUniformInputs,
    RgbaImageData, SceneRenderer, VideoFrameSource, VideoPlaybackSettings, WebCursorEventKind,
};
use truongpaper_scenescript::CursorEventKind;
use truongpaper_we_formats::WeVirtualFileSystem;

/// Frame interval used for `fps_cap = "display_native"` (Tauri reports no refresh rate) and
/// for invalid values.
pub const DEFAULT_FPS: u32 = 60;
/// Longest sleep while nothing animates. The render thread blocks on the host's change signal
/// ([`WallpaperHost::wait_for_change`]); this timeout only covers state that has no signal
/// (display sleep, platform window notifications).
pub const IDLE_WAIT: Duration = Duration::from_millis(250);
/// Retry delay after a frame could not be drawn (surface lost / timeout).
const RETRY_WAIT: Duration = Duration::from_millis(50);
/// Safety-net refresh of the shared monitor geometry; real changes arrive as display
/// notifications (`take_changed`) which invalidate the cache immediately.
const GEOMETRY_REFRESH: Duration = Duration::from_secs(15);
/// Age after which other threads re-query the shared monitor list.
const LIVE_MONITORS_TTL: Duration = Duration::from_secs(5);
/// Largest offscreen scene target edge.
const MAX_SCENE_TARGET_DIMENSION: f32 = 8192.0;
/// Scene load failed on a monitor: `{ monitorId, wallpaperId, message }`.
pub const EVENT_WALLPAPER_ERROR: &str = "wallpaper://error";
/// Non-fatal scene problems (skipped scripts, fallback shaders, ...):
/// `{ monitorId, wallpaperId, messages }`.
pub const EVENT_WALLPAPER_DIAGNOSTICS: &str = "wallpaper://diagnostics";
/// A newly applied wallpaper reached the screen: `{ monitorId, wallpaperId }` (`wallpaperId` is
/// `null` for Photo API images). The UI clears the monitor's error card on it.
pub const EVENT_WALLPAPER_LOADED: &str = "wallpaper://loaded";
/// Span mode is on but this wallpaper cannot span (Web / Video / Application): it stays on its
/// own display. `{ monitorId, wallpaperId, message }`; Span applies to photos and scenes.
pub const EVENT_SPAN_UNSUPPORTED: &str = "wallpaper://span-unsupported";
/// First delay before the render thread retries after a setup failure / panic.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
/// Longest delay between render thread restarts.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Scene run time after which the second diagnostics snapshot is taken.
const DIAGNOSTICS_SETTLE_SECS: f32 = 2.0;

/// Payload of [`EVENT_WALLPAPER_ERROR`].
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperErrorPayload {
    pub monitor_id: String,
    pub wallpaper_id: Option<String>,
    pub message: String,
}

/// Payload of [`EVENT_WALLPAPER_DIAGNOSTICS`].
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperDiagnosticsPayload {
    pub monitor_id: String,
    pub wallpaper_id: Option<String>,
    pub messages: Vec<String>,
}

/// Payload of [`EVENT_WALLPAPER_LOADED`].
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperLoadedPayload {
    pub monitor_id: String,
    pub wallpaper_id: Option<String>,
}

/// Payload of [`EVENT_SPAN_UNSUPPORTED`].
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpanUnsupportedPayload {
    pub monitor_id: String,
    pub wallpaper_id: Option<String>,
    pub message: String,
}

/// Web, Video and Application wallpapers run in their own per-display window and cannot span.
pub fn span_unsupported(active: &ActiveRuntime) -> bool {
    matches!(
        active,
        ActiveRuntime::Webview { .. } | ActiveRuntime::Application { .. }
    )
}

/// Emits [`EVENT_WALLPAPER_LOADED`] (also used by the webview manager once a page is up).
pub fn emit_loaded(app: &AppHandle, monitor_id: &str, wallpaper_id: Option<String>) {
    emit_json(
        app,
        EVENT_WALLPAPER_LOADED,
        WallpaperLoadedPayload {
            monitor_id: monitor_id.to_string(),
            wallpaper_id,
        },
    );
}

/// Monitors whose last persisted source must be applied again after a display change: every
/// monitor that appeared or came back (their runtime was dropped with the display, so they
/// would stay black) plus those that disappeared (so the scheduler restores their closed
/// window on replug). The scheduler skips ids it has no source for.
pub fn hotplug_restore_ids(old: &[String], new: &[String]) -> Vec<String> {
    let mut out: Vec<String> = new.iter().filter(|id| !old.contains(id)).cloned().collect();
    out.extend(old.iter().filter(|id| !new.contains(id)).cloned());
    out
}

/// Largest pixel size a photo may be displayed at: the biggest display, or the union canvas
/// in span mode. Photos are decoded no larger than this (plus headroom).
pub fn photo_display_cap(geoms: &[MonitorGeometry], span: bool) -> (u32, u32) {
    if geoms.is_empty() {
        return (1920, 1080);
    }
    if span && geoms.len() > 1 {
        let min_x = geoms
            .iter()
            .map(|g| i64::from(g.physical.0))
            .min()
            .unwrap_or(0);
        let min_y = geoms
            .iter()
            .map(|g| i64::from(g.physical.1))
            .min()
            .unwrap_or(0);
        let max_x = geoms
            .iter()
            .map(|g| i64::from(g.physical.0) + i64::from(g.physical.2))
            .max()
            .unwrap_or(0);
        let max_y = geoms
            .iter()
            .map(|g| i64::from(g.physical.1) + i64::from(g.physical.3))
            .max()
            .unwrap_or(0);
        return (
            u32::try_from(max_x - min_x).unwrap_or(u32::MAX).max(1),
            u32::try_from(max_y - min_y).unwrap_or(u32::MAX).max(1),
        );
    }
    geoms.iter().fold((1, 1), |acc, g| {
        (acc.0.max(g.physical.2), acc.1.max(g.physical.3))
    })
}

/// What the render loop has seen since its last full pass, to decide whether it can go back to
/// sleep without touching any state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleState {
    /// The previous pass ended with nothing animating and nothing waiting to be redrawn.
    pub was_idle: bool,
    pub host_changed: bool,
    pub suspended_changed: bool,
    pub span_changed: bool,
    /// A display / space / desktop notification arrived.
    pub platform_event: bool,
    pub photo_ready: bool,
}

/// `true` when the pass can be skipped entirely (no snapshot clone, no settings lookups).
pub fn can_skip_idle_pass(s: IdleState) -> bool {
    s.was_idle
        && !s.host_changed
        && !s.suspended_changed
        && !s.span_changed
        && !s.platform_event
        && !s.photo_ready
}

/// How long the render loop sleeps after a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopWait {
    /// Pace to the frame interval.
    Frame,
    /// A frame could not be presented: retry soon.
    Retry,
    /// Nothing to do: sleep on the host change signal.
    Idle,
}

pub fn loop_wait(animating: bool, stalled: bool) -> LoopWait {
    if animating {
        LoopWait::Frame
    } else if stalled {
        LoopWait::Retry
    } else {
        LoopWait::Idle
    }
}

/// Next frame deadline on a monotonic grid (`prev + interval`), re-anchored to `now` when the
/// loop fell more than one interval behind (no catch-up burst).
pub fn next_frame_deadline(prev: Instant, now: Instant, interval: Duration) -> Instant {
    let next = prev + interval;
    if next + interval < now {
        now + interval
    } else {
        next
    }
}

/// Present mode: vsync (`Fifo`) for a single display. With several displays one display's
/// vsync wait would stall the others' acquire in the shared loop, so prefer a non-blocking
/// mode and let the loop's frame timer pace them.
pub fn pick_present_mode(
    supported: &[wgpu::PresentMode],
    multi_display: bool,
) -> wgpu::PresentMode {
    if multi_display {
        for mode in [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate] {
            if supported.contains(&mode) {
                return mode;
            }
        }
    }
    wgpu::PresentMode::Fifo
}

/// `Some(current)` when there are diagnostics and they differ from what was last emitted.
pub fn diagnostics_to_emit(current: &[String], last_sent: &[String]) -> Option<Vec<String>> {
    (!current.is_empty() && current != last_sent).then(|| current.to_vec())
}

/// Next restart delay: 1s, 2s, 4s ... capped at [`MAX_BACKOFF`].
pub fn next_backoff(current: Duration) -> Duration {
    (current * 2).min(MAX_BACKOFF)
}

/// How an old and a new monitor layout differ (by monitor id).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GeometryDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Same id but different size / position / scale.
    pub changed: Vec<String>,
    /// Same id and identical geometry: their renderer state can be kept.
    pub unchanged: Vec<String>,
}

impl GeometryDiff {
    pub fn between(old: &[MonitorGeometry], new: &[MonitorGeometry]) -> Self {
        let mut diff = Self::default();
        for n in new {
            match old.iter().find(|o| o.id == n.id) {
                None => diff.added.push(n.id.clone()),
                Some(o) if o == n => diff.unchanged.push(n.id.clone()),
                Some(_) => diff.changed.push(n.id.clone()),
            }
        }
        diff.removed = old
            .iter()
            .filter(|o| !new.iter().any(|n| n.id == o.id))
            .map(|o| o.id.clone())
            .collect();
        diff
    }
}

/// A monitor's sub-rectangle of the shared span canvas (the union of all display frames).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanTransform {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    /// Width / height of the whole canvas.
    pub aspect: f32,
}

/// Sub-rectangle of the union of `geoms` (logical points) covered by `monitor_id`.
pub fn span_transform(geoms: &[MonitorGeometry], monitor_id: &str) -> Option<SpanTransform> {
    let me = geoms.iter().find(|g| g.id == monitor_id)?;
    let min_x = geoms.iter().map(|g| g.logical.x).fold(f64::MAX, f64::min);
    let min_y = geoms.iter().map(|g| g.logical.y).fold(f64::MAX, f64::min);
    let max_x = geoms
        .iter()
        .map(|g| g.logical.x + g.logical.width)
        .fold(f64::MIN, f64::max);
    let max_y = geoms
        .iter()
        .map(|g| g.logical.y + g.logical.height)
        .fold(f64::MIN, f64::max);
    let (w, h) = ((max_x - min_x), (max_y - min_y));
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(SpanTransform {
        offset: [
            ((me.logical.x - min_x) / w) as f32,
            ((me.logical.y - min_y) / h) as f32,
        ],
        scale: [
            (me.logical.width / w) as f32,
            (me.logical.height / h) as f32,
        ],
        aspect: (w / h) as f32,
    })
}

/// The monitor whose source feeds every display in span mode: the one at the origin, else the
/// first.
pub fn span_source_monitor(geoms: &[MonitorGeometry]) -> Option<&str> {
    geoms
        .iter()
        .find(|g| g.logical.x.abs() < 0.5 && g.logical.y.abs() < 0.5)
        .or_else(|| geoms.first())
        .map(|g| g.id.as_str())
}

/// `AppSettings::multi_monitor_mode == "span"`.
pub fn is_span_mode(mode: &str) -> bool {
    mode == "span"
}

// ---------------------------------------------------------------------------------------------
// Pure helpers (unit tested)
// ---------------------------------------------------------------------------------------------

/// Frames per second for `AppSettings::fps_cap` (`30 | 60 | 120 | "display_native"`).
pub fn fps_for_cap(fps_cap: &serde_json::Value) -> u32 {
    match fps_cap.as_u64() {
        Some(v @ (30 | 60 | 120)) => v as u32,
        _ => DEFAULT_FPS,
    }
}

/// Frame interval for `AppSettings::fps_cap`.
pub fn frame_interval(fps_cap: &serde_json::Value) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps_for_cap(fps_cap)))
}

/// A connected monitor in the coordinate spaces the desktop host needs.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorGeometry {
    pub id: String,
    /// Logical points, top-left origin (Quartz global space on macOS).
    pub logical: ScreenRect,
    /// Physical pixels in the virtual screen: `(x, y, width, height)`.
    pub physical: (i32, i32, u32, u32),
}

/// Display notifications can be emitted without any drawable geometry change. Recreating
/// windows in response to those notifications exposes the blank AppKit backing between frames.
pub fn surface_geometry_changed(current: &[MonitorGeometry], next: &[MonitorGeometry]) -> bool {
    !next.is_empty()
        && (current.len() != next.len()
            || current.iter().any(|old| !next.iter().any(|new| old == new)))
}

static LIVE_CACHE: Mutex<Option<(Instant, Vec<crate::commands::monitor::LiveMonitor>)>> =
    parking_lot::const_mutex(None);

/// The monitor list, queried at most once per [`LIVE_MONITORS_TTL`] and shared by the render
/// loop, the webview manager and Smart Pause (each query hops to the main thread).
pub fn live_monitors_cached(app: &AppHandle) -> Vec<crate::commands::monitor::LiveMonitor> {
    if let Some((at, list)) = LIVE_CACHE.lock().as_ref() {
        if at.elapsed() < LIVE_MONITORS_TTL {
            return list.clone();
        }
    }
    let list = live_monitors(app);
    *LIVE_CACHE.lock() = Some((Instant::now(), list.clone()));
    list
}

/// Forgets the shared monitor list (a display notification arrived).
pub fn invalidate_live_monitors() {
    *LIVE_CACHE.lock() = None;
}

/// Geometry of every connected monitor (ids as in `commands::monitor::live_monitors`).
pub fn monitor_geometries(app: &AppHandle) -> Vec<MonitorGeometry> {
    live_monitors_cached(app)
        .into_iter()
        .map(|m| {
            let (x, y, w, h) = m.logical_rect();
            MonitorGeometry {
                logical: ScreenRect::new(x, y, w, h),
                physical: (m.x, m.y, m.width, m.height),
                id: m.id,
            }
        })
        .collect()
}

/// Height of the primary display in logical points (the monitor at the Quartz origin), needed
/// to convert between Cocoa (bottom-left) and Quartz (top-left) coordinates.
pub fn primary_logical_height(geoms: &[MonitorGeometry]) -> f64 {
    geoms
        .iter()
        .find(|g| g.logical.x.abs() < 0.5 && g.logical.y.abs() < 0.5)
        .or_else(|| geoms.first())
        .map_or(0.0, |g| g.logical.height)
}

/// Maps desktop windows (screen rects in Quartz points) to monitor ids by the closest matching
/// logical rect (within 2 points per edge). Each monitor id is used at most once.
pub fn match_screens_to_monitors(
    screens: &[ScreenRect],
    monitors: &[(String, ScreenRect)],
) -> Vec<Option<String>> {
    const TOLERANCE: f64 = 2.0;
    let mut used = vec![false; monitors.len()];
    screens
        .iter()
        .map(|s| {
            let best = monitors
                .iter()
                .enumerate()
                .filter(|(i, _)| !used[*i])
                .map(|(i, (_, m))| {
                    let d = [
                        (s.x - m.x).abs(),
                        (s.y - m.y).abs(),
                        (s.width - m.width).abs(),
                        (s.height - m.height).abs(),
                    ];
                    (i, d)
                })
                .filter(|(_, d)| d.iter().all(|v| *v <= TOLERANCE))
                .min_by(|a, b| {
                    let sa: f64 = a.1.iter().sum();
                    let sb: f64 = b.1.iter().sum();
                    sa.total_cmp(&sb)
                })
                .map(|(i, _)| i);
            best.map(|i| {
                used[i] = true;
                monitors[i].0.clone()
            })
        })
        .collect()
}

/// Cursor state on one monitor: normalized `[0, 1]` top-left position and the primary button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorSample {
    pub pos: [f32; 2],
    pub left_down: bool,
}

/// Desktop cursor events between two polls: `Move` when the position changed, `Down` on a press,
/// `Up` + `Click` on a release. Leaving the monitor releases a held button without a click.
pub fn cursor_transitions(
    prev: Option<CursorSample>,
    cur: Option<CursorSample>,
) -> Vec<(WebCursorEventKind, [f32; 2])> {
    let mut out = Vec::new();
    match (prev, cur) {
        (_, Some(c)) => {
            let was_down = prev.is_some_and(|p| p.left_down);
            if prev.is_none_or(|p| p.pos != c.pos) {
                out.push((WebCursorEventKind::Move, c.pos));
            }
            if c.left_down && !was_down {
                out.push((WebCursorEventKind::Down, c.pos));
            } else if !c.left_down && was_down {
                out.push((WebCursorEventKind::Up, c.pos));
                out.push((WebCursorEventKind::Click, c.pos));
            }
        }
        (Some(p), None) if p.left_down => out.push((WebCursorEventKind::Up, p.pos)),
        _ => {}
    }
    out
}

/// SceneScript event for a desktop cursor event.
pub fn scene_cursor_kind(kind: WebCursorEventKind) -> CursorEventKind {
    match kind {
        WebCursorEventKind::Move => CursorEventKind::Move,
        WebCursorEventKind::Down => CursorEventKind::Down,
        WebCursorEventKind::Up => CursorEventKind::Up,
        WebCursorEventKind::Click => CursorEventKind::Click,
    }
}

/// Settings handed to scene embedded-video decoders.
pub fn video_settings(adj: &UniversalAdjustments) -> VideoPlaybackSettings {
    VideoPlaybackSettings {
        speed: adj.playback_speed,
        volume: adj.volume,
        muted: adj.muted,
    }
    .clamped()
}

/// Master volume for scene sound layers (`0` when muted).
pub fn scene_master_volume(adj: &UniversalAdjustments) -> f32 {
    video_settings(adj).effective_volume()
}

/// Key identifying "the same scene" across monitors.
pub fn scene_key(desc: &SceneDescriptor) -> String {
    format!("{}|{}", desc.install_dir.display(), desc.scene_file)
}

/// Whether `a` and `b` load the same scene (user property values aside, which are applied to
/// a live scene without reloading it).
pub fn same_scene(a: &SceneDescriptor, b: &SceneDescriptor) -> bool {
    a.install_dir == b.install_dir
        && a.core_assets_dir == b.core_assets_dir
        && a.scene_file == b.scene_file
        && a.content_revision == b.content_revision
}

/// Which monitor plays the sounds of each scene: for every scene key (`None` = no scene), the
/// first playing monitor showing it, or the first one showing it when all are paused (so its
/// paused sound layers are kept rather than restarted).
pub fn sound_owners(scenes: &[Option<(String, bool)>]) -> Vec<bool> {
    let owner_of = |key: &str| {
        let showing = || {
            scenes
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.as_ref().filter(|(k, _)| k == key).map(|s| (i, s.1)))
        };
        showing()
            .find(|(_, playing)| *playing)
            .or_else(|| showing().next())
            .map(|(i, _)| i)
    };
    scenes
        .iter()
        .enumerate()
        .map(|(i, s)| s.as_ref().is_some_and(|(k, _)| owner_of(k) == Some(i)))
        .collect()
}

/// Compositor uniforms for one frame: the monitor's fit / grading uniforms with the transition
/// progress, aspect ratios and time filled in.
pub fn frame_uniforms(
    base: &CompositorUniforms,
    progress: f32,
    monitor_aspect: f32,
    from_aspect: f32,
    to_aspect: f32,
    time: f32,
) -> CompositorUniforms {
    CompositorUniforms {
        progress: progress.clamp(0.0, 1.0),
        monitor_aspect,
        from_image_aspect: from_aspect,
        to_image_aspect: to_aspect,
        time,
        ..*base
    }
}

/// Field-wise equality (`CompositorUniforms` has no `PartialEq`).
fn same_uniforms(a: &CompositorUniforms, b: &CompositorUniforms) -> bool {
    let fa = [
        a.progress,
        a.monitor_aspect,
        a.from_image_aspect,
        a.to_image_aspect,
        a.zoom,
        a.offset_x,
        a.offset_y,
        a.brightness,
        a.contrast,
        a.saturation,
        a.hue_shift,
        a.span_offset_x,
        a.span_offset_y,
        a.span_scale_x,
        a.span_scale_y,
    ];
    let fb = [
        b.progress,
        b.monitor_aspect,
        b.from_image_aspect,
        b.to_image_aspect,
        b.zoom,
        b.offset_x,
        b.offset_y,
        b.brightness,
        b.contrast,
        b.saturation,
        b.hue_shift,
        b.span_offset_x,
        b.span_offset_y,
        b.span_scale_x,
        b.span_scale_y,
    ];
    fa == fb
        && a.transition_mode == b.transition_mode
        && a.from_fit_mode == b.from_fit_mode
        && a.to_fit_mode == b.to_fit_mode
}

/// Maps desktop coordinates through the same fit transform as the compositor.
fn scene_cursor_sample(
    sample: CursorSample,
    uniforms: &CompositorUniforms,
) -> Option<CursorSample> {
    // Span mode: monitor-local position -> shared canvas position (identity otherwise).
    let x = uniforms.span_offset_x + sample.pos[0] * uniforms.span_scale_x;
    let y = uniforms.span_offset_y + sample.pos[1] * uniforms.span_scale_y;
    let zoom = uniforms.zoom.max(0.05);
    let x = (x - 0.5) / zoom - uniforms.offset_x * 0.5;
    let y = (y - 0.5) / zoom - uniforms.offset_y * 0.5;
    let ratio = uniforms.monitor_aspect / uniforms.to_image_aspect.max(0.001);
    let fit = uniforms.to_fit_mode;
    let (sx, sy) = if fit == FitMode::Stretch as u32 {
        (1.0, 1.0)
    } else if fit == FitMode::Fill as u32 {
        (ratio.min(1.0), ratio.recip().min(1.0))
    } else {
        (ratio.max(1.0), ratio.recip().max(1.0))
    };
    let pos = [x * sx + 0.5, y * sy + 0.5];
    if fit == FitMode::Fill as u32 || fit == FitMode::Stretch as u32 {
        return Some(CursorSample {
            pos: pos.map(|v| v.clamp(0.0, 1.0)),
            ..sample
        });
    }
    // The backdrop outside the fitted image is not part of the interactive scene.
    pos.iter()
        .all(|v| (0.0..=1.0).contains(v))
        .then_some(CursorSample { pos, ..sample })
}

// ---------------------------------------------------------------------------------------------
// Cursor polling
// ---------------------------------------------------------------------------------------------

/// Polls the desktop cursor for one monitor (docs/01 §5). The desktop windows ignore mouse
/// events, so positions are read from the OS.
#[derive(Default)]
pub struct CursorPoller {
    #[cfg(target_os = "windows")]
    desktop: Option<truongpaper_platform::win_desktop::DesktopHandles>,
    #[cfg(target_os = "windows")]
    setup_attempts: u32,
}

impl CursorPoller {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(target_os = "macos")]
    pub fn poll(&mut self, geoms: &[MonitorGeometry], monitor_id: &str) -> Option<CursorSample> {
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let g = geoms.iter().find(|g| g.id == monitor_id)?;
        let primary_h = primary_logical_height(geoms);
        // The Quartz <-> Cocoa flip is its own inverse.
        let cocoa = truongpaper_platform::cursor::cocoa_rect_to_quartz(g.logical, primary_h);
        let frame = NSRect::new(
            NSPoint::new(cocoa.x, cocoa.y),
            NSSize::new(cocoa.width, cocoa.height),
        );
        let (x, y, left_down) =
            truongpaper_platform::cursor::poll_desktop_cursor_macos(frame, primary_h)?;
        Some(CursorSample {
            pos: [x, y],
            left_down,
        })
    }

    /// Windows: positions only while the desktop is under the cursor. Button state needs
    /// `GetAsyncKeyState` (`Win32_UI_Input_KeyboardAndMouse`, not enabled), so no clicks.
    #[cfg(target_os = "windows")]
    pub fn poll(&mut self, geoms: &[MonitorGeometry], monitor_id: &str) -> Option<CursorSample> {
        use windows::Win32::Foundation::RECT;
        let g = geoms.iter().find(|g| g.id == monitor_id)?;
        if self.desktop.is_none() && self.setup_attempts < 5 {
            self.setup_attempts += 1;
            // SAFETY: plain Win32 window lookups.
            self.desktop = unsafe { truongpaper_platform::win_desktop::setup_desktop_layer() };
        }
        let desktop = self.desktop.as_ref()?;
        let (x, y, w, h) = g.physical;
        let rect = RECT {
            left: x,
            top: y,
            right: x + w as i32,
            bottom: y + h as i32,
        };
        // SAFETY: handles come from setup_desktop_layer above.
        let (nx, ny, desktop_visible) =
            unsafe { truongpaper_platform::cursor::poll_desktop_cursor_win32(rect, desktop)? };
        desktop_visible.then_some(CursorSample {
            pos: [nx, ny],
            left_down: false,
        })
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub fn poll(&mut self, _geoms: &[MonitorGeometry], _monitor_id: &str) -> Option<CursorSample> {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Platform desktop surfaces
// ---------------------------------------------------------------------------------------------

/// A desktop surface for one monitor, created by the platform layer.
pub struct SurfaceTarget {
    pub monitor_id: String,
    pub surface: wgpu::Surface<'static>,
    /// Physical pixels.
    pub width: u32,
    pub height: u32,
    /// Backing scale (physical pixels per logical point); 1.0 where pixels are the unit.
    pub scale_factor: f32,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{match_screens_to_monitors, MonitorGeometry, SurfaceTarget};
    use objc2::rc::Retained;
    use objc2::{class, msg_send, MainThreadMarker};
    use objc2_app_kit::{NSScreen, NSWindow};
    use std::cell::RefCell;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;
    use tauri::AppHandle;
    use truongpaper_platform::mac_desktop::{self, MacScreenEvent};
    use truongpaper_platform::ScreenRect;

    thread_local! {
        /// Desktop windows; only touched on the main thread.
        static WINDOWS: RefCell<Vec<Retained<NSWindow>>> = const { RefCell::new(Vec::new()) };
    }
    static OBSERVER_STARTED: AtomicBool = AtomicBool::new(false);
    static SCREENS_CHANGED: AtomicBool = AtomicBool::new(false);
    static SPACE_CHANGED: AtomicBool = AtomicBool::new(false);

    pub fn take_space_changed() -> bool {
        SPACE_CHANGED.swap(false, Ordering::AcqRel)
    }

    pub struct DesktopSurfaces;

    fn surface_for_window(
        instance: &wgpu::Instance,
        window: &NSWindow,
    ) -> Result<wgpu::Surface<'static>, String> {
        let view = window
            .contentView()
            .ok_or("desktop window has no content view")?;
        // Own the Metal root layer rather than let AppKit paint a backing layer under a
        // wgpu-created sublayer. Layer-hosting views keep their last presented drawable even
        // while the wallpaper is static/paused and AppKit invalidates the window.
        // Apple requires setLayer before setWantsLayer for a layer-hosting view.
        // https://developer.apple.com/documentation/appkit/nsview/wantslayer
        unsafe {
            let layer: Retained<objc2::runtime::AnyObject> = msg_send![class!(CAMetalLayer), new];
            let _: () = msg_send![&*layer, setFrame: view.bounds()];
            let _: () = msg_send![&*layer, setContentsScale: window.backingScaleFactor()];
            let _: () = msg_send![&*layer, setOpaque: true];
            let _: () = msg_send![&*view, setLayer: &*layer];
            view.setWantsLayer(true);
        }
        let ptr = NonNull::new(Retained::as_ptr(&view) as *mut std::ffi::c_void)
            .ok_or("null content view")?;
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: wgpu::rwh::RawDisplayHandle::AppKit(
                wgpu::rwh::AppKitDisplayHandle::new(),
            ),
            raw_window_handle: wgpu::rwh::RawWindowHandle::AppKit(
                wgpu::rwh::AppKitWindowHandle::new(ptr),
            ),
        };
        // SAFETY: the window (and its content view) is retained in `WINDOWS` until
        // `teardown`, which the render thread calls only after dropping the surfaces.
        unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())
    }

    fn close_all_windows() {
        WINDOWS.with(|w| {
            for window in w.borrow_mut().drain(..) {
                window.orderOut(None);
                window.close();
            }
        });
    }

    impl DesktopSurfaces {
        pub fn new() -> Self {
            Self
        }

        pub fn create(
            &mut self,
            app: &AppHandle,
            instance: &Arc<wgpu::Instance>,
            geoms: &[MonitorGeometry],
        ) -> Vec<SurfaceTarget> {
            let (tx, rx) = mpsc::channel();
            let instance = Arc::clone(instance);
            let monitors: Vec<(String, ScreenRect)> =
                geoms.iter().map(|g| (g.id.clone(), g.logical)).collect();
            let dispatched = app.run_on_main_thread(move || {
                let Some(mtm) = MainThreadMarker::new() else {
                    let _ = tx.send(Vec::new());
                    return;
                };
                if !OBSERVER_STARTED.swap(true, Ordering::SeqCst) {
                    mac_desktop::start_screen_and_space_observer(mtm, |event| match event {
                        MacScreenEvent::ScreenParametersChanged => {
                            SCREENS_CHANGED.store(true, Ordering::SeqCst)
                        }
                        MacScreenEvent::ActiveSpaceChanged => {
                            SPACE_CHANGED.store(true, Ordering::Release)
                        }
                    });
                }
                close_all_windows();
                let primary_h = NSScreen::screens(mtm)
                    .iter()
                    .next()
                    .map_or(0.0, |s| s.frame().size.height);
                let windows = mac_desktop::create_desktop_windows_for_all_screens(mtm);
                let quartz: Vec<ScreenRect> = windows
                    .iter()
                    .map(|(_, w)| {
                        let f = w.frame();
                        truongpaper_platform::cursor::cocoa_rect_to_quartz(
                            ScreenRect::new(f.origin.x, f.origin.y, f.size.width, f.size.height),
                            primary_h,
                        )
                    })
                    .collect();
                let ids = match_screens_to_monitors(&quartz, &monitors);
                let mut out = Vec::new();
                let mut keep = Vec::new();
                for ((display_id, window), id) in windows.into_iter().zip(ids) {
                    let Some(monitor_id) = id else {
                        tracing::warn!("display {display_id} has no matching monitor id; skipped");
                        window.orderOut(None);
                        window.close();
                        continue;
                    };
                    let scale = window.backingScaleFactor().max(1.0);
                    let f = window.frame();
                    let width = (f.size.width * scale).round().max(1.0) as u32;
                    let height = (f.size.height * scale).round().max(1.0) as u32;
                    match surface_for_window(&instance, &window) {
                        Ok(surface) => out.push(SurfaceTarget {
                            monitor_id,
                            surface,
                            width,
                            height,
                            scale_factor: scale as f32,
                        }),
                        Err(err) => tracing::warn!("desktop surface for {monitor_id}: {err}"),
                    }
                    keep.push(window);
                }
                WINDOWS.with(|w| w.borrow_mut().extend(keep));
                let _ = tx.send(out);
            });
            if let Err(err) = dispatched {
                tracing::warn!("could not reach the main thread for desktop windows: {err}");
                return Vec::new();
            }
            rx.recv_timeout(Duration::from_secs(10)).unwrap_or_default()
        }

        /// `true` once after display hotplug / resolution / scale changes.
        pub fn take_changed(&mut self) -> bool {
            SCREENS_CHANGED.swap(false, Ordering::SeqCst)
        }

        pub fn pump(&mut self) {}

        /// Hides and releases every desktop window (call after dropping the surfaces).
        pub fn teardown(&mut self, app: &AppHandle) {
            let _ = app.run_on_main_thread(close_all_windows);
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{MonitorGeometry, SurfaceTarget};
    use std::num::NonZeroIsize;
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::Arc;
    use tauri::AppHandle;
    use truongpaper_platform::win_desktop::{self, DesktopLifecycleEvent};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    pub struct DesktopSurfaces {
        hwnds: Vec<HWND>,
        events: Receiver<DesktopLifecycleEvent>,
    }

    unsafe extern "system" fn surface_wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    impl DesktopSurfaces {
        pub fn new() -> Self {
            let (tx, rx) = channel();
            win_desktop::spawn_desktop_lifecycle_watcher(tx);
            Self {
                hwnds: Vec::new(),
                events: rx,
            }
        }

        pub fn create(
            &mut self,
            _app: &AppHandle,
            instance: &Arc<wgpu::Instance>,
            _geoms: &[MonitorGeometry],
        ) -> Vec<SurfaceTarget> {
            let mut out = Vec::new();
            // SAFETY: Win32 window creation on this (render) thread, which also pumps its
            // messages in `pump`; handles are destroyed in `teardown`.
            unsafe {
                let Some(desktop) = win_desktop::setup_desktop_layer() else {
                    tracing::warn!("Progman/WorkerW not found; desktop renderer not attached");
                    return out;
                };
                let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
                let class_name = w!("TruongPaperWallpaperSurface");
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(surface_wndproc),
                    hInstance: hinstance.into(),
                    lpszClassName: class_name,
                    ..Default::default()
                };
                // Fails harmlessly when the class is already registered (re-creation).
                RegisterClassExW(&wc);
                for m in win_desktop::enumerate_monitors_win32() {
                    let r = m.monitor_rect;
                    let (width, height) = ((r.right - r.left).max(1), (r.bottom - r.top).max(1));
                    let Ok(hwnd) = CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                        class_name,
                        w!("TruongPaper Wallpaper"),
                        WS_POPUP,
                        r.left,
                        r.top,
                        width,
                        height,
                        None,
                        None,
                        Some(hinstance.into()),
                        None,
                    ) else {
                        continue;
                    };
                    if !win_desktop::attach_hwnd_to_desktop(hwnd, &desktop, r) {
                        let _ = DestroyWindow(hwnd);
                        continue;
                    }
                    self.hwnds.push(hwnd);
                    let Some(raw) = NonZeroIsize::new(hwnd.0 as isize) else {
                        continue;
                    };
                    let mut handle = wgpu::rwh::Win32WindowHandle::new(raw);
                    handle.hinstance = NonZeroIsize::new(hinstance.0 as isize);
                    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: wgpu::rwh::RawDisplayHandle::Windows(
                            wgpu::rwh::WindowsDisplayHandle::new(),
                        ),
                        raw_window_handle: wgpu::rwh::RawWindowHandle::Win32(handle),
                    };
                    match instance.create_surface_unsafe(target) {
                        Ok(surface) => out.push(SurfaceTarget {
                            monitor_id: m.device_name.clone(),
                            surface,
                            width: width as u32,
                            height: height as u32,
                            scale_factor: 1.0,
                        }),
                        Err(err) => {
                            tracing::warn!("desktop surface for {}: {err}", m.device_name)
                        }
                    }
                }
            }
            out
        }

        /// `true` after explorer.exe restarted (TaskbarCreated) or the display topology changed.
        pub fn take_changed(&mut self) -> bool {
            let mut changed = false;
            while self.events.try_recv().is_ok() {
                changed = true;
            }
            changed
        }

        /// Dispatches messages for the surface windows owned by this thread.
        pub fn pump(&mut self) {
            // SAFETY: standard message loop on the thread that created the windows.
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
            }
        }

        pub fn teardown(&mut self, _app: &AppHandle) {
            for hwnd in self.hwnds.drain(..) {
                // SAFETY: windows created by this thread in `create`.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::{MonitorGeometry, SurfaceTarget};
    use std::sync::Arc;
    use tauri::AppHandle;

    pub struct DesktopSurfaces;

    impl DesktopSurfaces {
        pub fn new() -> Self {
            Self
        }

        pub fn create(
            &mut self,
            _app: &AppHandle,
            _instance: &Arc<wgpu::Instance>,
            _geoms: &[MonitorGeometry],
        ) -> Vec<SurfaceTarget> {
            tracing::warn!("no desktop wallpaper host on this OS");
            Vec::new()
        }

        pub fn take_changed(&mut self) -> bool {
            false
        }

        pub fn pump(&mut self) {}

        pub fn teardown(&mut self, _app: &AppHandle) {}
    }
}

// ---------------------------------------------------------------------------------------------
// Embedded `.tex` MP4 decoders
// ---------------------------------------------------------------------------------------------

/// `MacVideoDecoder` as a [`VideoFrameSource`] (BGRA frames, row stride may be padded).
#[cfg(target_os = "macos")]
struct MacVideoSource(truongpaper_platform::mac_video::MacVideoDecoder);

#[cfg(target_os = "macos")]
impl VideoFrameSource for MacVideoSource {
    fn latest_frame(&mut self) -> Option<truongpaper_renderer::VideoFrame> {
        let (width, height, stride, data) = self.0.copy_latest_bgra_frame()?;
        Some(truongpaper_renderer::VideoFrame {
            width,
            height,
            stride,
            format: truongpaper_renderer::VideoPixelFormat::Bgra8,
            data,
        })
    }

    fn apply_settings(&mut self, settings: VideoPlaybackSettings) {
        let s = settings.clamped();
        self.0.set_rate(s.speed);
        self.0.set_volume(s.effective_volume());
        self.0.set_muted(s.is_effectively_muted());
    }

    fn set_paused(&mut self, paused: bool) {
        if paused {
            self.0.pause();
        } else {
            self.0.play();
        }
    }
}

struct EmbeddedVideo {
    path: String,
    source: Box<dyn VideoFrameSource>,
}

/// Opens a platform decoder for every embedded MP4 `.tex` of the loaded scene.
fn open_embedded_videos(
    renderer: &SceneRenderer,
    vfs: &WeVirtualFileSystem,
    desc: &SceneDescriptor,
    adj: &UniversalAdjustments,
) -> Vec<EmbeddedVideo> {
    let paths = renderer.embedded_video_paths();
    if paths.is_empty() {
        return Vec::new();
    }
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut out = Vec::new();
    for path in paths {
        let extracted = vfs
            .read_bytes(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| truongpaper_we_formats::parse_tex(&b).map_err(|e| e.to_string()))
            .and_then(|tex| {
                truongpaper_renderer::extract_embedded_tex_mp4(&tex, &desc.install_dir, &path)
                    .map_err(|e| e.to_string())
            });
        let mp4 = match extracted {
            Ok(Some(p)) => p,
            Ok(None) => continue,
            Err(err) => {
                tracing::warn!("embedded video {path}: {err}");
                continue;
            }
        };
        #[cfg(target_os = "macos")]
        match truongpaper_platform::mac_video::MacVideoDecoder::open(&mp4) {
            Ok(decoder) => {
                let mut source = MacVideoSource(decoder);
                // Muted until the render loop makes this monitor the scene's sound owner.
                source.apply_settings(VideoPlaybackSettings {
                    muted: true,
                    ..video_settings(adj)
                });
                source.set_paused(false);
                out.push(EmbeddedVideo {
                    path,
                    source: Box::new(source),
                });
            }
            Err(err) => tracing::warn!("embedded video {}: {err}", mp4.display()),
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = adj;
            tracing::warn!(
                "embedded video {} stays on its placeholder: D3D11VA decode is not implemented",
                mp4.display()
            );
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Per-monitor renderer
// ---------------------------------------------------------------------------------------------

struct LayerTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    aspect: f32,
    /// Unique id for the compositor's bind-group / blur caches.
    id: u64,
}

// ---------------------------------------------------------------------------------------------
// Photo decode worker (keeps decoding and resizing off the render thread)
// ---------------------------------------------------------------------------------------------

enum PhotoInput {
    Path(PathBuf),
    /// Already decoded by a command; may still be larger than the display needs.
    Image(Arc<RgbaImageData>),
}

struct PhotoJob {
    monitor_id: String,
    generation: u64,
    input: PhotoInput,
    max_display: (u32, u32),
    /// Webview preview still instead of a regular photo.
    preview: bool,
}

struct PhotoDone {
    monitor_id: String,
    generation: u64,
    preview: bool,
    result: Result<Arc<RgbaImageData>, String>,
}

fn run_photo_job(job: PhotoJob) -> PhotoDone {
    let PhotoJob {
        monitor_id,
        generation,
        input,
        max_display,
        preview,
    } = job;
    let result = match input {
        PhotoInput::Image(img) => Ok(img),
        PhotoInput::Path(path) => crate::commands::photo::decode_photo_for_display(true, &path)
            .and_then(|img| img.ok_or_else(|| format!("photo {} was not decoded", path.display()))),
    }
    .map(|img| downscale_for_display(img, max_display));
    PhotoDone {
        monitor_id,
        generation,
        preview,
        result,
    }
}

/// Newest job per `(monitor, preview)`: a burst of applies decodes only the last photo.
fn coalesce_jobs(jobs: Vec<PhotoJob>) -> Vec<PhotoJob> {
    let mut out: Vec<PhotoJob> = Vec::new();
    for job in jobs {
        match out
            .iter_mut()
            .find(|j| j.monitor_id == job.monitor_id && j.preview == job.preview)
        {
            Some(slot) => *slot = job,
            None => out.push(job),
        }
    }
    out
}

/// Hands freed heap pages back to macOS. After a large scene or photo is dropped, the system
/// allocator keeps its empty pages (over 200 MB was measured), and Activity Monitor counts
/// them against TruongPaper until something else needs the memory.
fn release_free_memory() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone means "all zones", and a goal of 0 means "as much as possible".
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

struct PhotoWorker {
    jobs: mpsc::Sender<PhotoJob>,
    done: mpsc::Receiver<PhotoDone>,
}

impl PhotoWorker {
    fn spawn(host: Arc<WallpaperHost>) -> Option<Self> {
        let (jobs, job_rx) = mpsc::channel::<PhotoJob>();
        let (done_tx, done) = mpsc::channel::<PhotoDone>();
        std::thread::Builder::new()
            .name("truongpaper-photo-decode".into())
            .spawn(move || {
                while let Ok(first) = job_rx.recv() {
                    let mut batch = vec![first];
                    batch.extend(job_rx.try_iter());
                    for job in coalesce_jobs(batch) {
                        if done_tx.send(run_photo_job(job)).is_err() {
                            return;
                        }
                        host.wake();
                    }
                }
            })
            .ok()?;
        Some(Self { jobs, done })
    }

    fn submit(&self, job: PhotoJob) {
        if self.jobs.send(job).is_err() {
            tracing::warn!("photo decode worker is gone");
        }
    }

    fn drain(&self) -> Vec<PhotoDone> {
        self.done.try_iter().collect()
    }
}

struct SceneState {
    wallpaper_id: Option<String>,
    desc: SceneDescriptor,
    runtime: SceneRuntime,
    renderer: SceneRenderer,
    vfs: WeVirtualFileSystem,
    _target: wgpu::Texture,
    render_view: wgpu::TextureView,
    sample_view: wgpu::TextureView,
    /// Cache id of `sample_view` (compositor bind group).
    view_id: u64,
    /// Changes whenever the scene texture got new pixels (fit-blur is recomputed only then).
    content_serial: u64,
    aspect: f32,
    /// Only the sound owner (see [`sound_owners`]) opens an audio output and plays the scene's
    /// sound layers / embedded-video audio, so a scene on several monitors is heard once.
    sounds: Option<truongpaper_audio::SceneSoundPlayer>,
    sound_owner: bool,
    sound_unavailable: bool,
    sound_volume: Option<f32>,
    videos: Vec<EmbeddedVideo>,
    video_settings: Option<VideoPlaybackSettings>,
    rng: SoundRng,
    last_cursor: Option<CursorSample>,
    last_frame: Instant,
    has_frame: bool,
    paused: bool,
    diag: DiagState,
    /// `SceneGraph::render_epoch` at the last `SceneRenderer::load`.
    loaded_epoch: u64,
}

/// Diagnostics bookkeeping of one loaded scene.
#[derive(Default)]
struct DiagState {
    /// Warnings collected while loading GPU resources.
    load_warnings: Vec<String>,
    /// Last list emitted as `wallpaper://diagnostics`.
    last_sent: Vec<String>,
    /// 0 = nothing emitted yet, 1 = waiting for the settle time, 2 = done.
    stage: u8,
}

impl SceneState {
    /// Applies changed user properties: bound values and scripts update live; when a changed
    /// property feeds an effect / bloom binding (baked into GPU plans) the renderer reloads in
    /// place (textures stay cached, so this is cheap compared to a scene restart).
    fn apply_properties(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, json: &str) {
        if let Err(err) = self.runtime.apply_user_properties(json) {
            tracing::warn!("scene {}: property update: {err}", self.desc.scene_file);
            return;
        }
        let epoch = self.runtime.graph().render_epoch();
        if epoch != self.loaded_epoch {
            self.loaded_epoch = epoch;
            let warnings = self
                .renderer
                .load(device, queue, &self.vfs, self.runtime.graph());
            self.runtime.add_diagnostics(self.renderer.diagnostics());
            self.diag.load_warnings = warnings;
        }
    }

    /// Load warnings plus whatever the scene runtime reports.
    fn diagnostics(&self) -> Vec<String> {
        let mut out = self.diag.load_warnings.clone();
        for msg in self.runtime.diagnostics() {
            if !out.contains(&msg) {
                out.push(msg);
            }
        }
        out
    }

    fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        self.runtime.set_paused(paused);
        if let Some(sounds) = &mut self.sounds {
            sounds.set_paused(paused);
        }
        for v in &mut self.videos {
            v.source.set_paused(paused);
        }
        // No catch-up step after a pause.
        self.last_frame = Instant::now();
    }

    /// Opens (owner) or releases (non-owner) the scene's audio output. The next
    /// [`Self::apply_adjustments`] (re)starts the sound layers and re-applies the embedded-video
    /// mute state.
    fn set_sound_owner(&mut self, owner: bool) {
        if self.sound_owner == owner {
            return;
        }
        self.sound_owner = owner;
        if owner {
            if self.sounds.is_none() && !self.sound_unavailable {
                match truongpaper_audio::SceneSoundPlayer::new() {
                    Ok(player) => self.sounds = Some(player),
                    Err(err) => {
                        tracing::warn!("scene sounds disabled (audio output unavailable): {err}");
                        self.sound_unavailable = true;
                    }
                }
            }
        } else if let Some(mut sounds) = self.sounds.take() {
            sounds.stop_all();
        }
        self.sound_volume = None;
        self.video_settings = None;
    }

    /// Applies volume / mute / speed changes to the sound layers and embedded videos.
    fn apply_adjustments(&mut self, adj: &UniversalAdjustments) {
        let volume = if self.sound_owner {
            scene_master_volume(adj)
        } else {
            0.0
        };
        if self.sound_volume != Some(volume) {
            self.sound_volume = Some(volume);
            if let Some(sounds) = &mut self.sounds {
                sounds.stop_all();
                let plans = self.runtime.sound_plans(&self.vfs, volume, &mut self.rng);
                sounds.play_plans(plans);
                sounds.set_paused(self.paused);
            }
        }
        let mut settings = video_settings(adj);
        settings.muted |= !self.sound_owner;
        if self.video_settings != Some(settings) {
            self.video_settings = Some(settings);
            for v in &mut self.videos {
                v.source.apply_settings(settings);
            }
        }
    }
}

enum Content {
    /// Nothing loaded (no source yet, webview / application wallpaper, or load failure).
    None,
    Photo,
    WebviewPreview,
    Scene(Box<SceneState>),
}

struct MonitorRenderer {
    monitor_id: String,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    scale_factor: f32,
    compositor: CompositorPipeline,
    generation: Option<u64>,
    content: Content,
    from: Option<LayerTexture>,
    to: Option<LayerTexture>,
    transition_started: Option<Instant>,
    last_uniforms: Option<CompositorUniforms>,
    dirty: bool,
    was_rendering: bool,
    started: Instant,
    /// The last scene load failed and nothing could be kept: show black.
    load_failed: bool,
    /// Scene load error waiting to be emitted as `wallpaper://error`.
    pending_error: Option<WallpaperErrorPayload>,
    /// A photo for this generation is being decoded on the worker; the previous frame stays.
    awaiting_photo: Option<u64>,
    /// The last acquire failed: the loop retries soon instead of idling.
    stalled: bool,
    /// A scene was replaced by something else: tell the UI to drop its diagnostics.
    diag_clear: Option<Option<String>>,
}

/// Renderer state that survives a surface rebuild of an unchanged display.
struct Carry {
    generation: Option<u64>,
    content: Content,
    from: Option<LayerTexture>,
    to: Option<LayerTexture>,
    transition_started: Option<Instant>,
    last_uniforms: Option<CompositorUniforms>,
    started: Instant,
    awaiting_photo: Option<u64>,
}

/// Prefers an sRGB swapchain format (photos are uploaded as `Rgba8UnormSrgb`).
fn pick_surface_format(caps: &wgpu::SurfaceCapabilities) -> Option<wgpu::TextureFormat> {
    caps.formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .or_else(|| caps.formats.first().copied())
}

impl MonitorRenderer {
    fn new(
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        target: SurfaceTarget,
        multi_display: bool,
    ) -> Option<Self> {
        let caps = target.surface.get_capabilities(adapter);
        let Some(format) = pick_surface_format(&caps) else {
            tracing::warn!(
                "surface for {} is not supported by the adapter",
                target.monitor_id
            );
            return None;
        };
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        let max = device.limits().max_texture_dimension_2d;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: target.width.clamp(1, max),
            height: target.height.clamp(1, max),
            present_mode: pick_present_mode(&caps.present_modes, multi_display),
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        target.surface.configure(device, &config);
        let compositor = CompositorPipeline::new(device, format, config.width, config.height);
        Some(Self {
            scale_factor: target.scale_factor.max(1.0),
            monitor_id: target.monitor_id,
            surface: target.surface,
            config,
            compositor,
            generation: None,
            content: Content::None,
            from: None,
            to: None,
            transition_started: None,
            last_uniforms: None,
            dirty: true,
            was_rendering: false,
            started: Instant::now(),
            load_failed: false,
            pending_error: None,
            awaiting_photo: None,
            stalled: false,
            diag_clear: None,
        })
    }

    /// Detaches the GPU content (scene / photo textures are device-level, not surface-bound) and
    /// drops the surface, so the platform windows can be rebuilt without reloading the scene.
    fn into_carry(self) -> Carry {
        let Self {
            generation,
            content,
            from,
            to,
            transition_started,
            last_uniforms,
            started,
            awaiting_photo,
            ..
        } = self;
        Carry {
            generation,
            content,
            from,
            to,
            transition_started,
            last_uniforms,
            started,
            awaiting_photo,
        }
    }

    fn restore(&mut self, carry: Carry) {
        self.generation = carry.generation;
        self.content = carry.content;
        self.from = carry.from;
        self.to = carry.to;
        self.transition_started = carry.transition_started;
        self.last_uniforms = carry.last_uniforms;
        self.started = carry.started;
        self.awaiting_photo = carry.awaiting_photo;
        self.dirty = true;
    }

    /// Scene diagnostics to emit now: once after load and once after the first seconds of frames,
    /// each only when non-empty and changed.
    fn take_diagnostics(&mut self) -> Option<WallpaperDiagnosticsPayload> {
        if let Some(wallpaper_id) = self.diag_clear.take() {
            return Some(WallpaperDiagnosticsPayload {
                monitor_id: self.monitor_id.clone(),
                wallpaper_id,
                messages: Vec::new(),
            });
        }
        let Content::Scene(scene) = &mut self.content else {
            return None;
        };
        let first = match scene.diag.stage {
            0 => {
                scene.diag.stage = 1;
                true
            }
            1 if scene.runtime.elapsed() >= DIAGNOSTICS_SETTLE_SECS => {
                scene.diag.stage = 2;
                false
            }
            _ => return None,
        };
        let current = scene.diagnostics();
        // Every new load replaces the UI's list (an empty list clears the previous scene's
        // messages); the settle snapshot only adds when something new showed up.
        let messages = if first {
            current
        } else {
            diagnostics_to_emit(&current, &scene.diag.last_sent)?
        };
        scene.diag.last_sent = messages.clone();
        Some(WallpaperDiagnosticsPayload {
            monitor_id: self.monitor_id.clone(),
            wallpaper_id: scene.wallpaper_id.clone(),
            messages,
        })
    }

    fn monitor_aspect(&self) -> f32 {
        self.config.width.max(1) as f32 / self.config.height.max(1) as f32
    }

    /// (Re)loads the monitor content when the host generation changed.
    fn sync_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        host: &WallpaperHost,
        worker: &PhotoWorker,
        max_display: (u32, u32),
        rt: Option<&MonitorRuntime>,
    ) {
        let generation = rt.map_or(0, |r| r.generation);
        if self.generation == Some(generation) {
            return;
        }
        if let Some(ActiveRuntime::Webview {
            label,
            preview_path: Some(_),
            ..
        }) = rt.map(|runtime| &runtime.active)
        {
            if !crate::wallpaper_webview::can_show_preview(label) {
                return;
            }
        }
        self.generation = Some(generation);
        self.dirty = true;
        self.load_failed = false;
        let active = rt.map(|r| r.active.clone()).unwrap_or_default();
        self.awaiting_photo = None;
        if matches!(self.content, Content::Scene(_)) && !matches!(active, ActiveRuntime::Scene(_)) {
            if let Content::Scene(old) = &self.content {
                self.diag_clear = Some(old.wallpaper_id.clone());
            }
        }
        match active {
            ActiveRuntime::Photo { path } => {
                let was_photo = matches!(self.content, Content::Photo);
                // A Web/Video still is a real picture: fade from it instead of from black.
                let was_preview = matches!(self.content, Content::WebviewPreview);
                self.content = Content::Photo;
                if !was_photo {
                    self.from = None;
                    if !was_preview {
                        self.to = None;
                    }
                    self.compositor.invalidate_caches();
                    release_free_memory();
                }
                let pending = host.take_pending_photo(&self.monitor_id, generation);
                let input = match (pending, path) {
                    (Some(img), _) => Some(PhotoInput::Image(img)),
                    (None, Some(path)) => Some(PhotoInput::Path(path)),
                    (None, None) => None,
                };
                let Some(input) = input else {
                    return;
                };
                // The previous frame stays on screen until the worker delivers this one.
                self.awaiting_photo = Some(generation);
                worker.submit(PhotoJob {
                    monitor_id: self.monitor_id.clone(),
                    generation,
                    input,
                    max_display,
                    preview: false,
                });
            }
            ActiveRuntime::Scene(desc) => {
                self.from = None;
                self.to = None;
                self.transition_started = None;
                // Re-applying / restoring the scene already on screen keeps it running; only
                // changed user properties are pushed to it.
                if let Content::Scene(scene) = &mut self.content {
                    if same_scene(&scene.desc, &desc) {
                        if scene.desc.user_properties_json != desc.user_properties_json {
                            scene.apply_properties(device, queue, &desc.user_properties_json);
                        }
                        scene.wallpaper_id = rt.and_then(|r| r.wallpaper_id.clone());
                        scene.desc = desc;
                        return;
                    }
                }
                // Free the old scene first. Keeping it for a rollback held two scenes in memory
                // at once (well over 1 GB for large 4K scenes).
                let had_scene = matches!(self.content, Content::Scene(_));
                if had_scene {
                    self.content = Content::None;
                    release_free_memory();
                }
                match load_scene(device, queue, &self.config, self.scale_factor, &desc, rt) {
                    Ok(state) => {
                        self.content = Content::Scene(Box::new(state));
                        release_free_memory();
                    }
                    Err(err) => {
                        tracing::warn!("scene {} failed to load: {err}", desc.scene_file);
                        // A black surface instead of a stale frame.
                        self.content = Content::None;
                        self.load_failed = true;
                        self.pending_error = Some(WallpaperErrorPayload {
                            monitor_id: self.monitor_id.clone(),
                            wallpaper_id: rt.and_then(|r| r.wallpaper_id.clone()),
                            message: format!("Scene failed to load: {err}"),
                        });
                    }
                }
            }
            ActiveRuntime::Webview {
                preview_path: Some(path),
                ..
            } => {
                self.content = Content::None;
                self.from = None;
                self.to = None;
                self.transition_started = None;
                self.compositor.invalidate_caches();
                self.awaiting_photo = Some(generation);
                worker.submit(PhotoJob {
                    monitor_id: self.monitor_id.clone(),
                    generation,
                    input: PhotoInput::Path(path),
                    max_display,
                    preview: true,
                });
            }
            ActiveRuntime::None
            | ActiveRuntime::Webview {
                preview_path: None, ..
            }
            | ActiveRuntime::Application { .. } => {
                self.content = Content::None;
                self.from = None;
                self.to = None;
                self.transition_started = None;
                self.compositor.invalidate_caches();
                release_free_memory();
            }
        }
    }

    /// Uploads a decoded photo delivered by the worker. A result for a generation that has been
    /// replaced since is dropped.
    fn finish_photo(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, done: PhotoDone) {
        if self.generation != Some(done.generation) || self.awaiting_photo != Some(done.generation)
        {
            return;
        }
        self.awaiting_photo = None;
        let image = match done.result {
            Ok(image) => image,
            Err(err) => {
                tracing::warn!("{err}");
                // Lets the outgoing Web/Video window go instead of covering the desktop.
                self.load_failed = true;
                return;
            }
        };
        let label = if done.preview {
            "webview_preview"
        } else {
            "photo"
        };
        let texture = upload_rgba_texture(device, queue, &image, label);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let layer = LayerTexture {
            _texture: texture,
            view,
            aspect: image.aspect(),
            id: next_view_id(),
        };
        if done.preview {
            self.to = Some(layer);
            self.content = Content::WebviewPreview;
        } else {
            // Photo -> photo changes run the configured transition.
            self.from = self.to.take();
            self.transition_started = self.from.as_ref().map(|_| Instant::now());
            self.to = Some(layer);
        }
        self.compositor.invalidate_caches();
        self.dirty = true;
        // The decoded pixels now live on the GPU; return the CPU copy's pages.
        drop(image);
        release_free_memory();
    }

    /// Acquires the next surface texture, reconfiguring on `Lost` / `Outdated`.
    fn acquire(&mut self, device: &wgpu::Device) -> Option<wgpu::SurfaceTexture> {
        let result = self.surface.get_current_texture();
        self.stalled = result.is_err();
        match result {
            Ok(frame) => Some(frame),
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(device, &self.config);
                self.dirty = true;
                None
            }
            Err(wgpu::SurfaceError::Timeout) => None,
            Err(err) => {
                tracing::error!("desktop surface {}: {err}", self.monitor_id);
                None
            }
        }
    }

    /// Draws one frame. Returns `true` when the monitor animates (needs the next frame soon).
    #[allow(clippy::too_many_arguments)]
    fn frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rt: Option<&MonitorRuntime>,
        should_render: bool,
        sound_owner: bool,
        transition_secs: f32,
        geoms: &[MonitorGeometry],
        span: Option<SpanTransform>,
        cursor: &mut CursorPoller,
    ) -> bool {
        // WindowServer can discard a drawable while hidden/asleep. Static photos must
        // repaint once on resume even though their compositor settings did not change.
        if should_render && !self.was_rendering {
            self.dirty = true;
        }
        self.was_rendering = should_render;
        let mut base = rt.map(|r| r.uniforms).unwrap_or_default();
        if let Some(span) = span {
            base.span_offset_x = span.offset[0];
            base.span_offset_y = span.offset[1];
            base.span_scale_x = span.scale[0];
            base.span_scale_y = span.scale[1];
        }
        let adjustments = rt
            .map(MonitorRuntime::effective_adjustments)
            .unwrap_or_default();
        if let Content::Scene(scene) = &mut self.content {
            scene.set_paused(!should_render);
            scene.set_sound_owner(sound_owner);
            scene.apply_adjustments(&adjustments);
            self.dirty |= self
                .last_uniforms
                .as_ref()
                .is_none_or(|u| !same_uniforms(u, &base));
        }
        let needs_still = self.dirty
            && !matches!(self.content, Content::None)
            && !crate::power::rendering_suspended();
        if !should_render && !needs_still {
            // A new source or a revealed desktop still needs a frame while paused.
            return false;
        }
        let time = self.started.elapsed().as_secs_f32();
        let monitor_aspect = span.map_or_else(|| self.monitor_aspect(), |s| s.aspect);
        if matches!(self.content, Content::Scene(_)) {
            self.render_scene(device, queue, &base, monitor_aspect, geoms, cursor);
            return should_render;
        }
        if matches!(self.content, Content::None) {
            if self.dirty
                && (self.load_failed || rt.is_none_or(|r| r.active == ActiveRuntime::None))
            {
                self.present_clear(device, queue);
            }
            return false;
        }
        // Photo.
        let progress = match (should_render, self.transition_started) {
            (true, Some(t)) => transition_progress(
                t.elapsed().as_secs_f32(),
                clamp_transition_duration(transition_secs),
            ),
            _ => 1.0,
        };
        let Some(to) = &self.to else {
            // While the new photo decodes, leave the last frame on screen. Clearing here
            // showed a black flash between the old wallpaper and the photo.
            if self.dirty && self.awaiting_photo.is_none() {
                self.present_clear(device, queue);
            }
            return false;
        };
        let to_aspect = to.aspect;
        let from_aspect = self.from.as_ref().map_or(to_aspect, |f| f.aspect);
        let uniforms = frame_uniforms(
            &base,
            progress,
            monitor_aspect,
            from_aspect,
            to_aspect,
            time,
        );
        let changed = self
            .last_uniforms
            .as_ref()
            .is_none_or(|u| !same_uniforms(u, &uniforms));
        let animating = self.transition_started.is_some();
        if !(self.dirty || changed || animating) {
            return false;
        }
        if self.render_photo(device, queue, &uniforms) {
            self.last_uniforms = Some(uniforms);
            if progress >= 1.0 && animating {
                self.transition_started = None;
                self.from = None;
                // Do not keep the old photo alive through the cached bind group while idle.
                self.compositor.release_bind_group();
            }
        }
        self.transition_started.is_some()
    }

    fn present_clear(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(frame) = self.acquire(device) else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("desktop_clear"),
        });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("desktop_clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        queue.submit(Some(encoder.finish()));
        frame.present();
        self.dirty = false;
    }

    fn render_photo(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uniforms: &CompositorUniforms,
    ) -> bool {
        let Some(frame) = self.acquire(device) else {
            return false;
        };
        let Some(to) = &self.to else {
            return false;
        };
        let from_view = self.from.as_ref().map_or(&to.view, |f| &f.view);
        let from_id = self.from.as_ref().map_or(to.id, |f| f.id);
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("desktop_photo"),
        });
        let blur = uniforms.from_fit_mode == FitMode::FitBlur as u32
            || uniforms.to_fit_mode == FitMode::FitBlur as u32;
        // The backdrop is blurred once per source texture; without a transition source
        // (`from` is `to`) one blur serves both slots.
        let shared_blur = self.from.is_none();
        if blur {
            self.compositor.precompute_kawase_blur_keyed(
                device,
                &mut encoder,
                &to.view,
                true,
                to.id,
            );
            if !shared_blur {
                self.compositor.precompute_kawase_blur_keyed(
                    device,
                    &mut encoder,
                    from_view,
                    false,
                    from_id,
                );
            }
        }
        self.compositor.render_frame_keyed(
            device,
            queue,
            &mut encoder,
            (from_view, from_id),
            (&to.view, to.id),
            shared_blur,
            &view,
            uniforms,
        );
        queue.submit(Some(encoder.finish()));
        frame.present();
        self.dirty = false;
        true
    }

    fn render_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        base: &CompositorUniforms,
        monitor_aspect: f32,
        geoms: &[MonitorGeometry],
        cursor: &mut CursorPoller,
    ) {
        let time = self.started.elapsed().as_secs_f32();
        let Some(frame) = self.acquire(device) else {
            return;
        };
        let Content::Scene(scene) = &mut self.content else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("desktop_scene"),
        });
        let now = Instant::now();
        let dt = now.duration_since(scene.last_frame).as_secs_f32().min(0.1);
        scene.last_frame = now;
        let uniforms = frame_uniforms(base, 1.0, monitor_aspect, scene.aspect, scene.aspect, time);

        // Re-present the last scene texture when a paused desktop is revealed.
        if !scene.paused || !scene.has_frame {
            let (left, right) = crate::audio_bridge::latest_frame()
                .map(|f| (f.left_64, f.right_64))
                .unwrap_or(([0.0; 64], [0.0; 64]));
            let sample = cursor
                .poll(geoms, &self.monitor_id)
                .and_then(|sample| scene_cursor_sample(sample, &uniforms));
            for (kind, pos) in cursor_transitions(scene.last_cursor, sample) {
                if let Err(err) = scene.runtime.dispatch_cursor(scene_cursor_kind(kind), pos) {
                    tracing::debug!("scene cursor event: {err}");
                }
            }
            scene.last_cursor = sample.or_else(|| {
                scene.last_cursor.map(|last| CursorSample {
                    left_down: false,
                    ..last
                })
            });
            let c = scene.last_cursor.unwrap_or(CursorSample {
                pos: [0.5, 0.5],
                left_down: false,
            });
            scene.runtime.step(dt, c.pos, c.left_down, &left, &right);
            for v in &mut scene.videos {
                if let Some(frame) = v.source.latest_frame() {
                    scene.renderer.upload_video_frame(queue, &v.path, &frame);
                }
            }

            scene.renderer.render(
                device,
                queue,
                &mut encoder,
                &scene.render_view,
                scene.runtime.graph(),
                scene.runtime.camera(),
                &FrameUniformInputs {
                    time: scene.runtime.elapsed(),
                    pointer: c.pos.into(),
                    audio_64: [left, right],
                },
            );
            scene.has_frame = true;
            scene.content_serial = next_view_id();
        }
        // `from` and `to` are the same scene texture: blur it once, and only when the scene
        // actually drew new pixels this frame.
        if uniforms.to_fit_mode == FitMode::FitBlur as u32 {
            self.compositor.precompute_kawase_blur_keyed(
                device,
                &mut encoder,
                &scene.sample_view,
                true,
                scene.content_serial,
            );
        }
        self.compositor.render_frame_keyed(
            device,
            queue,
            &mut encoder,
            (&scene.sample_view, scene.view_id),
            (&scene.sample_view, scene.view_id),
            true,
            &view,
            &uniforms,
        );
        queue.submit(Some(encoder.finish()));
        frame.present();
        self.last_uniforms = Some(*base);
        self.dirty = false;
    }
}

/// The screen size scenes see as `engine.screenResolution`: logical points, not backing pixels.
/// Scripts position text/layers as a percentage of it (e.g. Persica's clock sits at
/// `screenResolution / 100 * 100` in a 3840x2160 scene), which assumes a 1080p-class value; the
/// 3456x2234 physical pixels of a Retina panel push those layers off the canvas.
pub fn scene_monitor_size(width: u32, height: u32, scale_factor: f32) -> [f32; 2] {
    let s = if scale_factor.is_finite() && scale_factor >= 1.0 {
        scale_factor
    } else {
        1.0
    };
    [(width as f32 / s).round(), (height as f32 / s).round()]
}

/// Loads a scene wallpaper on the render thread: SceneRuntime (scripts), SceneRenderer (GPU
/// resources), an offscreen target, scene sounds and embedded videos.
fn load_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &wgpu::SurfaceConfiguration,
    scale_factor: f32,
    desc: &SceneDescriptor,
    rt: Option<&MonitorRuntime>,
) -> anyhow::Result<SceneState> {
    let vfs = WeVirtualFileSystem::mount(desc.install_dir.clone(), desc.core_assets_dir.clone());
    let monitor_size = scene_monitor_size(config.width, config.height, scale_factor);
    let mut runtime = SceneRuntime::load(
        &vfs,
        &desc.scene_file,
        Some(&desc.user_properties_json),
        monitor_size,
    )?;
    let scene_size = runtime.graph().scene_size;
    let (w, h) = (
        scene_size.x.clamp(1.0, MAX_SCENE_TARGET_DIMENSION),
        scene_size.y.clamp(1.0, MAX_SCENE_TARGET_DIMENSION),
    );
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("desktop_scene_target"),
        size: wgpu::Extent3d {
            width: w.round() as u32,
            height: h.round() as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        // The scene renders gamma-space values (as Wallpaper Engine does); the compositor samples
        // them through an sRGB view so the sRGB swapchain reproduces them unchanged.
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    let render_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let sample_view = target.create_view(&wgpu::TextureViewDescriptor {
        format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
        ..Default::default()
    });
    let mut renderer =
        SceneRenderer::new(device, queue, wgpu::TextureFormat::Rgba8Unorm, scene_size);
    let load_warnings = renderer.load(device, queue, &vfs, runtime.graph());
    // Skipped effect passes and shader fallbacks join the scene report.
    runtime.add_diagnostics(renderer.diagnostics());
    for warning in &load_warnings {
        tracing::debug!("scene {}: {warning}", desc.scene_file);
    }
    let adjustments = rt
        .map(MonitorRuntime::effective_adjustments)
        .unwrap_or_default();
    let videos = open_embedded_videos(&renderer, &vfs, desc, &adjustments);
    let loaded_epoch = runtime.graph().render_epoch();
    let mut state = SceneState {
        loaded_epoch,
        wallpaper_id: rt.and_then(|r| r.wallpaper_id.clone()),
        desc: desc.clone(),
        runtime,
        renderer,
        vfs,
        _target: target,
        render_view,
        sample_view,
        view_id: next_view_id(),
        content_serial: next_view_id(),
        aspect: w / h,
        sounds: None,
        sound_owner: false,
        sound_unavailable: false,
        sound_volume: None,
        videos,
        video_settings: None,
        rng: SoundRng::from_time(),
        last_cursor: None,
        last_frame: Instant::now(),
        has_frame: false,
        paused: false,
        diag: DiagState {
            load_warnings,
            ..DiagState::default()
        },
    };
    // Embedded videos start muted; the sound owner is chosen by the render loop each frame.
    state.apply_adjustments(&adjustments);
    Ok(state)
}

// ---------------------------------------------------------------------------------------------
// Render thread
// ---------------------------------------------------------------------------------------------

/// Spawns the `truongpaper-render` thread for the lifetime of the app. A missing GPU adapter or
/// desktop layer logs a warning and leaves `live_renderer_attached()` false.
pub fn start(app: AppHandle, host: Arc<WallpaperHost>, settings: Arc<Mutex<AppSettings>>) {
    let spawned = std::thread::Builder::new()
        .name("truongpaper-render".into())
        .spawn(move || render_main(app, host, settings));
    if let Err(err) = spawned {
        tracing::error!("failed to start the desktop render thread: {err}");
    }
}

struct Gpu {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn create_gpu(instance: &wgpu::Instance, surface: &wgpu::Surface<'static>) -> Option<Gpu> {
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        force_fallback_adapter: false,
        compatible_surface: Some(surface),
    }))?;
    let limits = wgpu::Limits::default().using_resolution(adapter.limits());
    match pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("truongpaper-desktop"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::default(),
        },
        None,
    )) {
        Ok((device, queue)) => Some(Gpu {
            adapter,
            device,
            queue,
        }),
        Err(err) => {
            tracing::warn!("wgpu device request failed: {err}");
            None
        }
    }
}

/// Why [`run_renderer`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunEnd {
    /// The app is quitting.
    Shutdown,
    /// No desktop surface or GPU could be created.
    SetupFailed,
    /// The wgpu device was lost; everything must be recreated.
    DeviceLost,
}

/// Sleeps `total` in short slices, returning early on shutdown.
fn sleep_unless_shutdown(host: &WallpaperHost, total: Duration) {
    let end = Instant::now() + total;
    while !host.is_shutting_down() {
        let Some(left) = end.checked_duration_since(Instant::now()) else {
            return;
        };
        std::thread::sleep(left.min(Duration::from_millis(100)));
    }
}

/// Supervises the renderer: setup failures, device loss and panics restart it with a 1s, 2s,
/// 4s ... 30s backoff instead of leaving the desktop without a renderer.
fn render_main(app: AppHandle, host: Arc<WallpaperHost>, settings: Arc<Mutex<AppSettings>>) {
    if !cfg!(any(target_os = "macos", target_os = "windows")) {
        tracing::warn!("no desktop wallpaper host on this OS; desktop renderer not attached");
        return;
    }
    let instance = Arc::new(wgpu::Instance::new(wgpu::InstanceDescriptor::default()));
    let mut desktop = platform::DesktopSurfaces::new();
    let mut backoff = INITIAL_BACKOFF;
    while !host.is_shutting_down() {
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            run_renderer(&app, &host, &settings, &instance, &mut desktop)
        }));
        // Surfaces were dropped by `run_renderer` (or by unwinding) before the windows go.
        host.set_live_renderer_attached(false);
        desktop.teardown(&app);
        match outcome {
            Ok(RunEnd::Shutdown) => break,
            Ok(RunEnd::SetupFailed) => tracing::warn!("desktop renderer setup failed"),
            Ok(RunEnd::DeviceLost) => tracing::warn!("wgpu device lost; recreating the renderer"),
            Err(_) => tracing::error!("desktop render thread panicked; restarting it"),
        }
        if started.elapsed() >= MAX_BACKOFF {
            backoff = INITIAL_BACKOFF;
        }
        tracing::info!("retrying the desktop renderer in {backoff:?}");
        sleep_unless_shutdown(&host, backoff);
        backoff = next_backoff(backoff);
    }
    tracing::info!("desktop render thread stopped");
}

fn emit_json<S: Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if let Err(err) = app.emit(event, payload) {
        tracing::debug!("emit {event}: {err}");
    }
}

fn run_renderer(
    app: &AppHandle,
    host: &Arc<WallpaperHost>,
    settings: &Arc<Mutex<AppSettings>>,
    instance: &Arc<wgpu::Instance>,
    desktop: &mut platform::DesktopSurfaces,
) -> RunEnd {
    let mut geoms = monitor_geometries(app);
    let mut geoms_at = Instant::now();
    let mut surface_geoms = geoms.clone();
    let mut targets = desktop.create(app, instance, &geoms);
    let Some(first) = targets.first() else {
        tracing::warn!("no desktop surfaces were created; desktop renderer not attached");
        return RunEnd::SetupFailed;
    };
    let Some(gpu) = create_gpu(instance, &first.surface) else {
        tracing::warn!("no compatible wgpu adapter; desktop renderer not attached");
        drop(targets);
        return RunEnd::SetupFailed;
    };
    let device_lost = Arc::new(AtomicBool::new(false));
    {
        let flag = Arc::clone(&device_lost);
        gpu.device.set_device_lost_callback(move |reason, message| {
            if matches!(
                reason,
                wgpu::DeviceLostReason::Unknown | wgpu::DeviceLostReason::Destroyed
            ) {
                tracing::error!("wgpu device lost ({reason:?}): {message}");
                flag.store(true, Ordering::Release);
            }
        });
    }
    let multi_display = targets.len() > 1;
    let mut monitors: Vec<MonitorRenderer> = targets
        .drain(..)
        .filter_map(|t| MonitorRenderer::new(&gpu.adapter, &gpu.device, t, multi_display))
        .collect();
    if monitors.is_empty() {
        drop(monitors);
        return RunEnd::SetupFailed;
    }
    host.set_live_renderer_attached(true);
    tracing::info!(
        "desktop renderer attached to {} monitor(s) ({:?})",
        monitors.len(),
        gpu.adapter.get_info().name
    );
    let mut cursor = CursorPoller::new();
    let Some(worker) = PhotoWorker::spawn(Arc::clone(host)) else {
        tracing::error!("could not start the photo decode worker");
        monitors.clear();
        return RunEnd::SetupFailed;
    };
    let mut last_epoch = u64::MAX;
    let mut was_idle = false;
    let mut last_suspended = false;
    let mut last_span = false;
    let mut next_frame_at = Instant::now();
    let mut span_notified: HashSet<(String, u64)> = HashSet::new();

    let end = loop {
        if host.is_shutting_down() {
            break RunEnd::Shutdown;
        }
        if device_lost.load(Ordering::Acquire) {
            break RunEnd::DeviceLost;
        }
        let loop_start = Instant::now();
        desktop.pump();
        let mut platform_event = false;
        if desktop.take_changed() {
            platform_event = true;
            invalidate_live_monitors();
            let next = monitor_geometries(app);
            // Windows must rebuild on Explorer restart even when geometry is unchanged.
            // macOS screen-parameter notifications alone must not tear down live drawables.
            if cfg!(target_os = "windows") || surface_geometry_changed(&surface_geoms, &next) {
                let diff = GeometryDiff::between(&surface_geoms, &next);
                tracing::info!(
                    "display geometry changed (added {:?}, removed {:?}, changed {:?}); recreating desktop surfaces",
                    diff.added,
                    diff.removed,
                    diff.changed
                );
                // Unchanged displays keep their loaded scene / photo; only their surface is new.
                let restore_ids = hotplug_restore_ids(
                    &surface_geoms
                        .iter()
                        .map(|g| g.id.clone())
                        .collect::<Vec<_>>(),
                    &next.iter().map(|g| g.id.clone()).collect::<Vec<_>>(),
                );
                let mut carried: HashMap<String, Carry> = HashMap::new();
                for m in monitors.drain(..) {
                    if diff.unchanged.contains(&m.monitor_id) {
                        carried.insert(m.monitor_id.clone(), m.into_carry());
                    }
                }
                desktop.teardown(app);
                geoms = next;
                surface_geoms = geoms.clone();
                geoms_at = Instant::now();
                let targets = desktop.create(app, instance, &geoms);
                let multi_display = targets.len() > 1;
                monitors = targets
                    .into_iter()
                    .filter_map(|t| {
                        MonitorRenderer::new(&gpu.adapter, &gpu.device, t, multi_display)
                    })
                    .map(|mut m| {
                        if let Some(carry) = carried.remove(&m.monitor_id) {
                            m.restore(carry);
                        }
                        m
                    })
                    .collect();
                if !geoms.is_empty() {
                    let ids: Vec<String> = geoms.iter().map(|g| g.id.clone()).collect();
                    host.retain_monitors(&ids);
                }
                // Displays that came back (or are new) lost their runtime: have the scheduler
                // re-apply their persisted source so they are not left black.
                for id in &restore_ids {
                    crate::scheduler::note_monitor_disconnected(id);
                }
                host.set_live_renderer_attached(!monitors.is_empty());
                crate::wallpaper_webview::reattach_all();
            } else {
                tracing::debug!("ignored display notification with unchanged geometry");
            }
        }
        #[cfg(target_os = "macos")]
        if platform::take_space_changed() {
            platform_event = true;
            for monitor in &mut monitors {
                monitor.dirty = true;
            }
        }
        if geoms_at.elapsed() >= GEOMETRY_REFRESH {
            invalidate_live_monitors();
            geoms = monitor_geometries(app);
            geoms_at = Instant::now();
        }

        // Decoded photos delivered by the worker.
        let finished = worker.drain();
        let photo_ready = !finished.is_empty();
        for done in finished {
            if let Some(m) = monitors
                .iter_mut()
                .find(|m| m.monitor_id == done.monitor_id)
            {
                m.finish_photo(&gpu.device, &gpu.queue, done);
            }
        }

        let epoch = host.epoch();
        let (interval, transition_secs, span_mode) = {
            let s = settings.lock();
            (
                frame_interval(&s.fps_cap),
                s.transition_duration_seconds,
                is_span_mode(&s.multi_monitor_mode),
            )
        };
        let suspended = crate::power::rendering_suspended();
        if can_skip_idle_pass(IdleState {
            was_idle,
            host_changed: epoch != last_epoch,
            suspended_changed: suspended != last_suspended,
            span_changed: span_mode != last_span,
            platform_event,
            photo_ready,
        }) {
            host.wait_for_change(epoch, IDLE_WAIT);
            continue;
        }
        last_epoch = epoch;
        last_suspended = suspended;
        last_span = span_mode;
        crate::audio_bridge::set_tick_hz(crate::audio_bridge::analyzer_hz(
            (1.0 / interval.as_secs_f32().max(0.001)).round() as u32,
        ));
        let snapshot = host.monitors_snapshot();
        let events = host.drain_render_events();

        let mut animating = false;
        // Span mode: every display shows the source of the span source monitor.
        let span_on = span_mode && geoms.len() > 1;
        host.set_span_source(
            span_on
                .then(|| span_source_monitor(&geoms).map(str::to_string))
                .flatten(),
        );
        if span_on {
            for (id, rt) in &snapshot {
                if span_unsupported(&rt.active) && span_notified.insert((id.clone(), rt.generation))
                {
                    emit_json(
                        app,
                        EVENT_SPAN_UNSUPPORTED,
                        SpanUnsupportedPayload {
                            monitor_id: id.clone(),
                            wallpaper_id: rt.wallpaper_id.clone(),
                            message: "Span applies to photos and scenes; this wallpaper stays on its own display.".into(),
                        },
                    );
                }
            }
        }
        if span_notified.len() > 64 {
            span_notified.clear();
        }
        let max_display = photo_display_cap(&geoms, span_on);
        let span_rt = span_on
            .then(|| span_source_monitor(&geoms))
            .flatten()
            .and_then(|id| snapshot.iter().find(|(m, _)| m == id).map(|(_, rt)| rt));
        let rt_of = |id: &str| {
            if span_on {
                span_rt
            } else {
                snapshot.iter().find(|(m, _)| m == id).map(|(_, rt)| rt)
            }
        };
        let playing = |id: &str| !suspended && host.monitor_should_render(id);
        for m in &mut monitors {
            let rt = rt_of(&m.monitor_id);
            m.sync_content(&gpu.device, &gpu.queue, host, &worker, max_display, rt);
            if let Content::Scene(scene) = &mut m.content {
                for RenderEvent::SceneProperties {
                    wallpaper_id,
                    changed_json,
                } in &events
                {
                    if scene.wallpaper_id.as_deref() == Some(wallpaper_id.as_str()) {
                        scene.apply_properties(&gpu.device, &gpu.queue, changed_json);
                    }
                }
            }
        }
        // One monitor per scene plays its sounds.
        let owners = sound_owners(
            &monitors
                .iter()
                .map(|m| match &m.content {
                    Content::Scene(scene) => Some((scene_key(&scene.desc), playing(&m.monitor_id))),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        );
        for (m, sound_owner) in monitors.iter_mut().zip(owners) {
            let rt = rt_of(&m.monitor_id);
            let should_render = playing(&m.monitor_id);
            let span = span_on
                .then(|| span_transform(&geoms, &m.monitor_id))
                .flatten();
            animating |= m.frame(
                &gpu.device,
                &gpu.queue,
                rt,
                should_render,
                sound_owner,
                rt.and_then(|r| r.transition_secs)
                    .unwrap_or(transition_secs),
                &geoms,
                span,
                &mut cursor,
            );
            if let Some(rt) = rt {
                if m.generation == Some(rt.generation)
                    && rt.presented_generation != rt.generation
                    && !m.dirty
                    && m.awaiting_photo.is_none()
                    && (matches!(m.content, Content::Scene(_))
                        || matches!(m.content, Content::Photo) && m.to.is_some())
                    && host.note_presented(&m.monitor_id, rt.generation)
                {
                    emit_loaded(app, &m.monitor_id, rt.wallpaper_id.clone());
                } else if m.load_failed && m.generation == Some(rt.generation) {
                    // Nothing will be presented; release the outgoing Web/Video window so the
                    // error card is not hidden behind the old wallpaper.
                    host.note_presented(&m.monitor_id, rt.generation);
                }
            }
            if let Some(err) = m.pending_error.take() {
                emit_json(app, EVENT_WALLPAPER_ERROR, err);
            }
            if let Some(diagnostics) = m.take_diagnostics() {
                emit_json(app, EVENT_WALLPAPER_DIAGNOSTICS, diagnostics);
            }
        }

        let stalled = monitors.iter().any(|m| m.stalled);
        was_idle = !animating && !stalled;
        match loop_wait(animating, stalled) {
            LoopWait::Frame => {
                // Monotonic frame grid: no drift, and no display's vsync paces the others.
                next_frame_at = next_frame_deadline(next_frame_at, loop_start, interval);
                if let Some(rest) = next_frame_at.checked_duration_since(Instant::now()) {
                    std::thread::sleep(rest);
                }
            }
            LoopWait::Retry => {
                host.wait_for_change(epoch, RETRY_WAIT);
            }
            LoopWait::Idle => {
                next_frame_at = Instant::now();
                host.wait_for_change(epoch, IDLE_WAIT);
            }
        }
    };

    // Surfaces must be gone before the platform windows are torn down.
    monitors.clear();
    drop(gpu);
    end
}

#[cfg(test)]
mod tests {
    #[test]
    fn scene_monitor_size_is_logical() {
        assert_eq!(super::scene_monitor_size(3456, 2234, 2.0), [1728.0, 1117.0]);
        assert_eq!(super::scene_monitor_size(1920, 1080, 1.0), [1920.0, 1080.0]);
        assert_eq!(super::scene_monitor_size(1920, 1080, 0.0), [1920.0, 1080.0]);
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn scene_cursor_tracks_crop_and_excludes_fit_backdrop() {
        let mut uniforms = CompositorUniforms {
            monitor_aspect: 3456.0 / 2234.0,
            to_image_aspect: 16.0 / 9.0,
            to_fit_mode: FitMode::Fill as u32,
            ..Default::default()
        };
        let sample = CursorSample {
            pos: [0.0, 0.0],
            left_down: true,
        };
        let cropped = scene_cursor_sample(sample, &uniforms).unwrap();
        assert!((cropped.pos[0] - 0.0649).abs() < 0.0001);
        assert_eq!(cropped.pos[1], 0.0);
        assert!(cropped.left_down);

        for mode in [FitMode::FitBlack, FitMode::FitBlur] {
            uniforms.to_fit_mode = mode as u32;
            assert!(scene_cursor_sample(sample, &uniforms).is_none());
            let image_edge = CursorSample {
                pos: [0.5, 0.06491],
                ..sample
            };
            let fitted = scene_cursor_sample(image_edge, &uniforms).unwrap();
            assert!(fitted.pos[1].abs() < 0.0001);
        }
    }

    #[test]
    fn display_notifications_only_rebuild_changed_surface_geometry() {
        let display = MonitorGeometry {
            id: "A".into(),
            logical: ScreenRect::new(0.0, 0.0, 1728.0, 1117.0),
            physical: (0, 0, 3456, 2234),
        };
        let displays = [display.clone()];
        assert!(!surface_geometry_changed(&displays, &displays));
        assert!(
            !surface_geometry_changed(&displays, &[]),
            "sleep must not discard the last frame"
        );
        let mut resized = display.clone();
        resized.physical.2 = 3024;
        assert!(surface_geometry_changed(&displays, &[resized]));
        let mut moved = display.clone();
        moved.logical.x = 1728.0;
        assert!(surface_geometry_changed(&displays, &[moved]));
        let mut external = display;
        external.id = "B".into();
        let pair = [displays[0].clone(), external.clone()];
        assert!(surface_geometry_changed(&displays, &pair));
        assert!(!surface_geometry_changed(
            &pair,
            &[external, displays[0].clone()]
        ));
    }

    #[test]
    fn one_monitor_plays_the_sounds_of_each_scene() {
        let s = |k: &str, playing: bool| Some((k.to_string(), playing));
        // A on monitors 0 and 2, B on 3, photo on 1.
        assert_eq!(
            sound_owners(&[s("A", true), None, s("A", true), s("B", false)]),
            vec![true, false, false, true]
        );
        // The first monitor is paused: the next playing one takes over.
        assert_eq!(
            sound_owners(&[s("A", false), s("A", true)]),
            vec![false, true]
        );
        // All paused: the first keeps its (paused) player.
        assert_eq!(
            sound_owners(&[s("A", false), s("A", false)]),
            vec![true, false]
        );
        assert!(sound_owners(&[]).is_empty());
    }

    #[test]
    fn same_scene_ignores_user_property_values() {
        let desc = |dir: &str, props: &str| SceneDescriptor {
            install_dir: std::path::PathBuf::from(dir),
            core_assets_dir: std::path::PathBuf::from("/core"),
            scene_file: "scene.json".into(),
            content_revision: 0,
            user_properties_json: props.into(),
        };
        assert!(same_scene(&desc("/w/1", "{}"), &desc("/w/1", "{\"a\":1}")));
        let mut updated = desc("/w/1", "{}");
        updated.content_revision = 1;
        assert!(!same_scene(&desc("/w/1", "{}"), &updated));
        assert!(!same_scene(&desc("/w/1", "{}"), &desc("/w/2", "{}")));
        assert_eq!(
            scene_key(&desc("/w/1", "{}")),
            scene_key(&desc("/w/1", "{\"a\":1}"))
        );
        assert_ne!(
            scene_key(&desc("/w/1", "{}")),
            scene_key(&desc("/w/2", "{}"))
        );
    }

    #[test]
    fn fps_cap_maps_to_frame_interval() {
        assert_eq!(fps_for_cap(&json!(30)), 30);
        assert_eq!(fps_for_cap(&json!(120)), 120);
        assert_eq!(fps_for_cap(&json!("display_native")), DEFAULT_FPS);
        assert_eq!(fps_for_cap(&json!(75)), DEFAULT_FPS);
        assert_eq!(fps_for_cap(&json!(null)), DEFAULT_FPS);
        let i60 = frame_interval(&json!(60));
        assert!((i60.as_secs_f64() - 1.0 / 60.0).abs() < 1e-9);
        assert!(frame_interval(&json!(120)) < frame_interval(&json!(30)));
        assert!(IDLE_WAIT >= RETRY_WAIT);
    }

    #[test]
    fn screens_match_monitors_by_rect() {
        let monitors = vec![
            (
                "Built-in".to_string(),
                ScreenRect::new(0.0, 0.0, 1512.0, 982.0),
            ),
            (
                "LG".to_string(),
                ScreenRect::new(1512.0, -200.0, 2560.0, 1440.0),
            ),
        ];
        let screens = [
            ScreenRect::new(1512.0, -200.5, 2560.0, 1440.0),
            ScreenRect::new(0.0, 0.0, 1512.0, 982.0),
            ScreenRect::new(9000.0, 0.0, 800.0, 600.0),
        ];
        assert_eq!(
            match_screens_to_monitors(&screens, &monitors),
            vec![Some("LG".to_string()), Some("Built-in".to_string()), None]
        );
        // Identical rects are not assigned twice.
        let twin = [monitors[0].1, monitors[0].1];
        assert_eq!(
            match_screens_to_monitors(&twin, &monitors),
            vec![Some("Built-in".to_string()), None]
        );
    }

    #[test]
    fn primary_height_uses_the_origin_monitor() {
        let g = |id: &str, r: ScreenRect| MonitorGeometry {
            id: id.into(),
            logical: r,
            physical: (0, 0, 1, 1),
        };
        let geoms = [
            g("ext", ScreenRect::new(-1920.0, 0.0, 1920.0, 1080.0)),
            g("main", ScreenRect::new(0.0, 0.0, 1512.0, 982.0)),
        ];
        assert_eq!(primary_logical_height(&geoms), 982.0);
        assert_eq!(primary_logical_height(&geoms[..1]), 1080.0);
        assert_eq!(primary_logical_height(&[]), 0.0);
    }

    #[test]
    fn cursor_transitions_emit_move_down_up_click() {
        let s = |x: f32, down: bool| CursorSample {
            pos: [x, 0.5],
            left_down: down,
        };
        use WebCursorEventKind::*;
        assert_eq!(
            cursor_transitions(None, Some(s(0.1, false))),
            vec![(Move, [0.1, 0.5])]
        );
        assert!(cursor_transitions(Some(s(0.1, false)), Some(s(0.1, false))).is_empty());
        assert_eq!(
            cursor_transitions(Some(s(0.1, false)), Some(s(0.1, true))),
            vec![(Down, [0.1, 0.5])]
        );
        assert_eq!(
            cursor_transitions(Some(s(0.1, true)), Some(s(0.2, false))),
            vec![(Move, [0.2, 0.5]), (Up, [0.2, 0.5]), (Click, [0.2, 0.5])]
        );
        // Leaving the monitor with the button held releases it without a click.
        assert_eq!(
            cursor_transitions(Some(s(0.3, true)), None),
            vec![(Up, [0.3, 0.5])]
        );
        assert!(cursor_transitions(Some(s(0.3, false)), None).is_empty());
        assert_eq!(scene_cursor_kind(Click), CursorEventKind::Click);
    }

    #[test]
    fn adjustments_map_to_media_settings() {
        let adj = UniversalAdjustments {
            volume: 0.4,
            muted: false,
            playback_speed: 9.0,
            ..UniversalAdjustments::default()
        };
        let v = video_settings(&adj);
        assert_eq!(v.speed, truongpaper_renderer::MAX_PLAYBACK_SPEED);
        assert!((scene_master_volume(&adj) - 0.4).abs() < 1e-6);
        let muted = UniversalAdjustments { muted: true, ..adj };
        assert_eq!(scene_master_volume(&muted), 0.0);
    }

    #[test]
    fn frame_uniforms_keep_grading_and_fill_frame_values() {
        let base = CompositorUniforms {
            zoom: 1.3,
            saturation: 0.5,
            to_fit_mode: FitMode::Fill as u32,
            ..Default::default()
        };
        let u = frame_uniforms(&base, 2.0, 1.6, 1.0, 1.5, 3.0);
        assert_eq!(u.progress, 1.0);
        assert_eq!(
            (u.monitor_aspect, u.from_image_aspect, u.to_image_aspect),
            (1.6, 1.0, 1.5)
        );
        assert_eq!((u.zoom, u.saturation, u.time), (1.3, 0.5, 3.0));
        assert!(same_uniforms(
            &u,
            &frame_uniforms(&base, 1.0, 1.6, 1.0, 1.5, 9.0)
        ));
        assert!(!same_uniforms(
            &u,
            &frame_uniforms(&base, 0.5, 1.6, 1.0, 1.5, 3.0)
        ));
    }

    fn geom(id: &str, x: f64, y: f64, w: f64, h: f64, scale: f64) -> MonitorGeometry {
        MonitorGeometry {
            id: id.into(),
            logical: ScreenRect::new(x, y, w, h),
            physical: (
                (x * scale) as i32,
                (y * scale) as i32,
                (w * scale) as u32,
                (h * scale) as u32,
            ),
        }
    }

    #[test]
    fn geometry_diff_classifies_added_removed_changed_and_unchanged() {
        let old = [
            geom("A", 0.0, 0.0, 1920.0, 1080.0, 1.0),
            geom("B", 1920.0, 0.0, 1920.0, 1080.0, 1.0),
            geom("C", 3840.0, 0.0, 1280.0, 720.0, 1.0),
        ];
        let new = [
            geom("A", 0.0, 0.0, 1920.0, 1080.0, 1.0),
            // B moved.
            geom("B", 1920.0, 100.0, 1920.0, 1080.0, 1.0),
            // D appeared, C disappeared.
            geom("D", 3840.0, 0.0, 800.0, 600.0, 2.0),
        ];
        let diff = GeometryDiff::between(&old, &new);
        assert_eq!(diff.unchanged, vec!["A"]);
        assert_eq!(diff.changed, vec!["B"]);
        assert_eq!(diff.added, vec!["D"]);
        assert_eq!(diff.removed, vec!["C"]);
        // A pure scale change (same logical size, different physical size) counts as changed.
        let rescaled = [geom("A", 0.0, 0.0, 1920.0, 1080.0, 2.0)];
        assert_eq!(
            GeometryDiff::between(&old[..1], &rescaled).changed,
            vec!["A"]
        );
        assert_eq!(GeometryDiff::between(&old, &old).unchanged.len(), old.len());
    }

    #[test]
    fn backoff_doubles_up_to_thirty_seconds() {
        let mut d = INITIAL_BACKOFF;
        let mut seen = vec![d.as_secs()];
        for _ in 0..6 {
            d = next_backoff(d);
            seen.push(d.as_secs());
        }
        assert_eq!(seen, vec![1, 2, 4, 8, 16, 30, 30]);
    }

    #[test]
    fn diagnostics_emit_only_when_non_empty_and_changed() {
        let a = vec!["x".to_string()];
        assert_eq!(diagnostics_to_emit(&[], &[]), None);
        assert_eq!(diagnostics_to_emit(&a, &[]), Some(a.clone()));
        assert_eq!(diagnostics_to_emit(&a, &a), None);
        let b = vec!["x".to_string(), "y".to_string()];
        assert_eq!(diagnostics_to_emit(&b, &a), Some(b.clone()));
    }

    #[test]
    fn event_payloads_serialize_camel_case() {
        let v = serde_json::to_value(WallpaperErrorPayload {
            monitor_id: "m".into(),
            wallpaper_id: None,
            message: "boom".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            json!({ "monitorId": "m", "wallpaperId": null, "message": "boom" })
        );
        let v = serde_json::to_value(WallpaperDiagnosticsPayload {
            monitor_id: "m".into(),
            wallpaper_id: Some("w".into()),
            messages: vec!["a".into()],
        })
        .unwrap();
        assert_eq!(
            v,
            json!({ "monitorId": "m", "wallpaperId": "w", "messages": ["a"] })
        );
    }

    #[test]
    fn span_transform_maps_each_monitor_to_its_slice_of_the_union() {
        // 1920x1080 left of a 1280x1024 monitor whose top is lower.
        let geoms = [
            geom("L", 0.0, 0.0, 1920.0, 1080.0, 1.0),
            geom("R", 1920.0, 56.0, 1280.0, 1024.0, 1.0),
        ];
        let l = span_transform(&geoms, "L").unwrap();
        let r = span_transform(&geoms, "R").unwrap();
        assert!((l.aspect - 3200.0 / 1080.0).abs() < 1e-5);
        assert_eq!(l.offset, [0.0, 0.0]);
        assert!((l.scale[0] - 0.6).abs() < 1e-6 && (l.scale[1] - 1.0).abs() < 1e-6);
        assert!((r.offset[0] - 0.6).abs() < 1e-6);
        assert!((r.offset[1] - 56.0 / 1080.0).abs() < 1e-6);
        assert!((r.scale[0] - 0.4).abs() < 1e-6);
        assert!(span_transform(&geoms, "none").is_none());
        // Negative origins (monitor left of primary).
        let neg = [
            geom("P", 0.0, 0.0, 100.0, 100.0, 1.0),
            geom("N", -100.0, 0.0, 100.0, 100.0, 1.0),
        ];
        assert_eq!(span_transform(&neg, "N").unwrap().offset, [0.0, 0.0]);
        assert_eq!(span_transform(&neg, "P").unwrap().offset, [0.5, 0.0]);
        assert_eq!(span_source_monitor(&neg), Some("P"));
    }

    fn job(monitor: &str, generation: u64, preview: bool) -> PhotoJob {
        PhotoJob {
            monitor_id: monitor.into(),
            generation,
            input: PhotoInput::Path(PathBuf::from("x.jpg")),
            max_display: (100, 100),
            preview,
        }
    }

    #[test]
    fn decode_jobs_keep_only_the_newest_per_monitor() {
        let jobs = coalesce_jobs(vec![
            job("A", 1, false),
            job("B", 2, false),
            job("A", 3, false),
            job("A", 4, true),
        ]);
        let ids: Vec<(String, u64, bool)> = jobs
            .iter()
            .map(|j| (j.monitor_id.clone(), j.generation, j.preview))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("A".to_string(), 3, false),
                ("B".to_string(), 2, false),
                ("A".to_string(), 4, true)
            ]
        );
    }

    #[test]
    fn decode_job_downscales_pending_images_off_thread() {
        let big = Arc::new(RgbaImageData {
            width: 400,
            height: 200,
            pixels: vec![9; 400 * 200 * 4],
        });
        let done = run_photo_job(PhotoJob {
            monitor_id: "A".into(),
            generation: 7,
            input: PhotoInput::Image(big),
            max_display: (100, 50),
            preview: false,
        });
        let img = done.result.unwrap();
        assert_eq!((done.generation, img.width, img.height), (7, 126, 63));
        let missing = run_photo_job(job("A", 1, false));
        assert!(missing.result.is_err());
    }

    #[test]
    fn photo_cap_is_the_biggest_display_or_the_span_canvas() {
        let mut a = geom("A", 0.0, 0.0, 1920.0, 1080.0, 1.0);
        a.physical = (0, 0, 1920, 1080);
        let mut b = geom("B", 1920.0, 0.0, 1920.0, 1080.0, 2.0);
        b.physical = (1920, 0, 3840, 2160);
        let geoms = [a, b];
        assert_eq!(photo_display_cap(&geoms, false), (3840, 2160));
        assert_eq!(photo_display_cap(&geoms, true), (5760, 2160));
        assert_eq!(photo_display_cap(&[], false), (1920, 1080));
    }

    #[test]
    fn idle_pass_is_skipped_only_when_nothing_changed() {
        let idle = IdleState {
            was_idle: true,
            host_changed: false,
            suspended_changed: false,
            span_changed: false,
            platform_event: false,
            photo_ready: false,
        };
        assert!(can_skip_idle_pass(idle));
        for changed in [
            IdleState {
                was_idle: false,
                ..idle
            },
            IdleState {
                host_changed: true,
                ..idle
            },
            IdleState {
                suspended_changed: true,
                ..idle
            },
            IdleState {
                span_changed: true,
                ..idle
            },
            IdleState {
                platform_event: true,
                ..idle
            },
            IdleState {
                photo_ready: true,
                ..idle
            },
        ] {
            assert!(!can_skip_idle_pass(changed));
        }
        assert_eq!(loop_wait(true, true), LoopWait::Frame);
        assert_eq!(loop_wait(false, true), LoopWait::Retry);
        assert_eq!(loop_wait(false, false), LoopWait::Idle);
    }

    #[test]
    fn frame_deadlines_stay_on_a_grid_and_never_burst() {
        let t0 = Instant::now();
        let step = Duration::from_millis(16);
        assert_eq!(
            next_frame_deadline(t0, t0 + Duration::from_millis(5), step),
            t0 + step
        );
        // Far behind: re-anchor instead of catching up with a burst of frames.
        let late = t0 + Duration::from_secs(1);
        assert_eq!(next_frame_deadline(t0, late, step), late + step);
    }

    #[test]
    fn multi_display_avoids_blocking_present_modes() {
        use wgpu::PresentMode::*;
        assert_eq!(pick_present_mode(&[Fifo, Immediate], false), Fifo);
        assert_eq!(pick_present_mode(&[Fifo, Immediate], true), Immediate);
        assert_eq!(
            pick_present_mode(&[Fifo, Mailbox, Immediate], true),
            Mailbox
        );
        assert_eq!(pick_present_mode(&[Fifo], true), Fifo);
    }

    #[test]
    fn hotplug_restores_added_and_removed_displays() {
        let old = vec!["A".to_string(), "B".to_string()];
        let new = vec!["A".to_string(), "C".to_string()];
        assert_eq!(hotplug_restore_ids(&old, &new), vec!["C", "B"]);
        assert!(hotplug_restore_ids(&old, &old).is_empty());
    }

    #[test]
    fn only_web_video_and_application_wallpapers_cannot_span() {
        assert!(!span_unsupported(&ActiveRuntime::Photo { path: None }));
        assert!(!span_unsupported(&ActiveRuntime::None));
        assert!(span_unsupported(&ActiveRuntime::Application {
            exe_path: PathBuf::from("a.exe")
        }));
    }

    #[test]
    fn new_event_payloads_serialize_camel_case() {
        let loaded = serde_json::to_value(WallpaperLoadedPayload {
            monitor_id: "A".into(),
            wallpaper_id: None,
        })
        .unwrap();
        assert_eq!(loaded, json!({ "monitorId": "A", "wallpaperId": null }));
    }

    #[test]
    fn span_mode_setting_and_cursor_mapping() {
        assert!(is_span_mode("span"));
        assert!(!is_span_mode("independent"));
        assert!(!is_span_mode("bogus"));
        // Right half of a two-monitor canvas: local x = 0 is canvas x = 0.5.
        let uniforms = CompositorUniforms {
            span_offset_x: 0.5,
            span_scale_x: 0.5,
            to_fit_mode: FitMode::Stretch as u32,
            ..Default::default()
        };
        let sample = CursorSample {
            pos: [0.0, 0.5],
            left_down: false,
        };
        let mapped = scene_cursor_sample(sample, &uniforms).unwrap();
        assert!((mapped.pos[0] - 0.5).abs() < 1e-6);
        assert!(!same_uniforms(&uniforms, &CompositorUniforms::default()));
    }
}
