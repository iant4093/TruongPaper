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
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use truongpaper_core::AppSettings;
use truongpaper_platform::ScreenRect;
use truongpaper_renderer::{
    clamp_transition_duration, transition_progress, upload_rgba_texture, CompositorPipeline,
    CompositorUniforms, FitMode, FrameUniformInputs, RgbaImageData, SceneRenderer,
    VideoFrameSource, VideoPlaybackSettings, WebCursorEventKind,
};
use truongpaper_scenescript::CursorEventKind;
use truongpaper_we_formats::WeVirtualFileSystem;

/// Frame interval used for `fps_cap = "display_native"` (Tauri reports no refresh rate) and
/// for invalid values.
pub const DEFAULT_FPS: u32 = 60;
/// Poll interval while nothing animates (static photos, paused monitors, webview wallpapers).
pub const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How often the monitor geometry used for cursor normalization is refreshed.
const GEOMETRY_REFRESH: Duration = Duration::from_secs(2);
/// Largest offscreen scene target edge.
const MAX_SCENE_TARGET_DIMENSION: f32 = 8192.0;

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

/// Geometry of every connected monitor (ids as in `commands::monitor::live_monitors`).
pub fn monitor_geometries(app: &AppHandle) -> Vec<MonitorGeometry> {
    live_monitors(app)
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
    ];
    fa == fb
        && a.transition_mode == b.transition_mode
        && a.from_fit_mode == b.from_fit_mode
        && a.to_fit_mode == b.to_fit_mode
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
                    mac_desktop::start_screen_and_space_observer(mtm, |event| {
                        if event == MacScreenEvent::ScreenParametersChanged {
                            SCREENS_CHANGED.store(true, Ordering::SeqCst);
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
    paused: bool,
}

impl SceneState {
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
    Scene(Box<SceneState>),
}

struct MonitorRenderer {
    monitor_id: String,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
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
    fn new(adapter: &wgpu::Adapter, device: &wgpu::Device, target: SurfaceTarget) -> Option<Self> {
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
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        target.surface.configure(device, &config);
        let compositor = CompositorPipeline::new(device, format, config.width, config.height);
        Some(Self {
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
        rt: Option<&MonitorRuntime>,
    ) {
        let generation = rt.map_or(0, |r| r.generation);
        if self.generation == Some(generation) {
            return;
        }
        self.generation = Some(generation);
        self.dirty = true;
        let active = rt.map(|r| r.active.clone()).unwrap_or_default();
        match active {
            ActiveRuntime::Photo { path } => {
                let was_photo = matches!(self.content, Content::Photo);
                self.content = Content::Photo;
                let image: Option<Arc<RgbaImageData>> = host
                    .take_pending_photo(&self.monitor_id, generation)
                    .or_else(|| {
                        let path = path.as_ref()?;
                        match crate::commands::photo::decode_photo_for_display(true, path) {
                            Ok(img) => img,
                            Err(err) => {
                                tracing::warn!("photo {}: {err}", path.display());
                                None
                            }
                        }
                    });
                let Some(image) = image else {
                    if !was_photo {
                        self.from = None;
                        self.to = None;
                    }
                    return;
                };
                let texture = upload_rgba_texture(device, queue, &image, "photo");
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let layer = LayerTexture {
                    _texture: texture,
                    view,
                    aspect: image.aspect(),
                };
                // Photo -> photo changes run the configured transition.
                self.from = if was_photo { self.to.take() } else { None };
                self.transition_started = self.from.as_ref().map(|_| Instant::now());
                self.to = Some(layer);
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
                            if let Err(err) = scene
                                .runtime
                                .apply_user_properties(&desc.user_properties_json)
                            {
                                tracing::warn!("scene {}: property update: {err}", desc.scene_file);
                            }
                        }
                        scene.wallpaper_id = rt.and_then(|r| r.wallpaper_id.clone());
                        scene.desc = desc;
                        return;
                    }
                }
                self.content = match load_scene(device, queue, &self.config, &desc, rt) {
                    Ok(state) => Content::Scene(Box::new(state)),
                    Err(err) => {
                        tracing::warn!("scene {} failed to load: {err}", desc.scene_file);
                        Content::None
                    }
                };
            }
            ActiveRuntime::None
            | ActiveRuntime::Webview { .. }
            | ActiveRuntime::Application { .. } => {
                self.content = Content::None;
                self.from = None;
                self.to = None;
                self.transition_started = None;
            }
        }
    }

    /// Acquires the next surface texture, reconfiguring on `Lost` / `Outdated`.
    fn acquire(&mut self, device: &wgpu::Device) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
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
        cursor: &mut CursorPoller,
    ) -> bool {
        // WindowServer can discard a drawable while hidden/asleep. Static photos must
        // repaint once on resume even though their compositor settings did not change.
        if should_render && !self.was_rendering {
            self.dirty = true;
        }
        self.was_rendering = should_render;
        let base = rt.map(|r| r.uniforms).unwrap_or_default();
        let adjustments = rt.map(|r| r.adjustments).unwrap_or_default();
        if let Content::Scene(scene) = &mut self.content {
            scene.set_paused(!should_render);
            scene.set_sound_owner(sound_owner);
            scene.apply_adjustments(&adjustments);
        }
        let photo_needs_frame = self.dirty
            && matches!(self.content, Content::Photo)
            && !crate::power::rendering_suspended();
        if !should_render && !photo_needs_frame {
            // A newly selected photo still needs one frame while playback is paused.
            return false;
        }
        let time = self.started.elapsed().as_secs_f32();
        let monitor_aspect = self.monitor_aspect();
        if matches!(self.content, Content::Scene(_)) {
            self.render_scene(device, queue, &base, monitor_aspect, geoms, cursor);
            return true;
        }
        if matches!(self.content, Content::None) {
            if self.dirty && rt.is_none_or(|r| r.active == ActiveRuntime::None) {
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
            if self.dirty {
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
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("desktop_photo"),
        });
        let blur = uniforms.from_fit_mode == FitMode::FitBlur as u32
            || uniforms.to_fit_mode == FitMode::FitBlur as u32;
        if blur {
            self.compositor
                .precompute_kawase_blur(device, &mut encoder, from_view, false);
            self.compositor
                .precompute_kawase_blur(device, &mut encoder, &to.view, true);
        }
        self.compositor.render_frame(
            device,
            queue,
            &mut encoder,
            from_view,
            &to.view,
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
        let Content::Scene(scene) = &mut self.content else {
            return;
        };
        let now = Instant::now();
        let dt = now.duration_since(scene.last_frame).as_secs_f32().min(0.1);
        scene.last_frame = now;

        let (left, right) = crate::audio_bridge::latest_frame()
            .map(|f| (f.left_64, f.right_64))
            .unwrap_or(([0.0; 64], [0.0; 64]));
        let sample = cursor.poll(geoms, &self.monitor_id);
        for (kind, pos) in cursor_transitions(scene.last_cursor, sample) {
            if let Err(err) = scene.runtime.dispatch_cursor(scene_cursor_kind(kind), pos) {
                tracing::debug!("scene cursor event: {err}");
            }
        }
        if sample.is_some() {
            scene.last_cursor = sample;
        }
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
        let uniforms = frame_uniforms(base, 1.0, monitor_aspect, scene.aspect, scene.aspect, time);
        if uniforms.to_fit_mode == FitMode::FitBlur as u32 {
            self.compositor
                .precompute_kawase_blur(device, &mut encoder, &scene.sample_view, false);
            self.compositor
                .precompute_kawase_blur(device, &mut encoder, &scene.sample_view, true);
        }
        self.compositor.render_frame(
            device,
            queue,
            &mut encoder,
            &scene.sample_view,
            &scene.sample_view,
            &view,
            &uniforms,
        );
        queue.submit(Some(encoder.finish()));
        frame.present();
        self.dirty = false;
    }
}

/// Loads a scene wallpaper on the render thread: SceneRuntime (scripts), SceneRenderer (GPU
/// resources), an offscreen target, scene sounds and embedded videos.
fn load_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: &wgpu::SurfaceConfiguration,
    desc: &SceneDescriptor,
    rt: Option<&MonitorRuntime>,
) -> anyhow::Result<SceneState> {
    let vfs = WeVirtualFileSystem::mount(desc.install_dir.clone(), desc.core_assets_dir.clone());
    let monitor_size = [config.width as f32, config.height as f32];
    let runtime = SceneRuntime::load(
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
    for warning in renderer.load(device, queue, &vfs, runtime.graph()) {
        tracing::debug!("scene {}: {warning}", desc.scene_file);
    }
    let adjustments = rt.map(|r| r.adjustments).unwrap_or_default();
    let videos = open_embedded_videos(&renderer, &vfs, desc, &adjustments);
    let mut state = SceneState {
        wallpaper_id: rt.and_then(|r| r.wallpaper_id.clone()),
        desc: desc.clone(),
        runtime,
        renderer,
        vfs,
        _target: target,
        render_view,
        sample_view,
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
        paused: false,
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

fn render_main(app: AppHandle, host: Arc<WallpaperHost>, settings: Arc<Mutex<AppSettings>>) {
    let instance = Arc::new(wgpu::Instance::new(wgpu::InstanceDescriptor::default()));
    let mut desktop = platform::DesktopSurfaces::new();
    let mut geoms = monitor_geometries(&app);
    let mut geoms_at = Instant::now();
    let mut surface_geoms = geoms.clone();
    let mut targets = desktop.create(&app, &instance, &geoms);
    let Some(first) = targets.first() else {
        tracing::warn!("no desktop surfaces were created; desktop renderer not attached");
        host.set_live_renderer_attached(false);
        desktop.teardown(&app);
        return;
    };
    let Some(gpu) = create_gpu(&instance, &first.surface) else {
        tracing::warn!("no compatible wgpu adapter; desktop renderer not attached");
        host.set_live_renderer_attached(false);
        drop(targets);
        desktop.teardown(&app);
        return;
    };
    let mut monitors: Vec<MonitorRenderer> = targets
        .drain(..)
        .filter_map(|t| MonitorRenderer::new(&gpu.adapter, &gpu.device, t))
        .collect();
    host.set_live_renderer_attached(!monitors.is_empty());
    tracing::info!(
        "desktop renderer attached to {} monitor(s) ({:?})",
        monitors.len(),
        gpu.adapter.get_info().name
    );
    let mut cursor = CursorPoller::new();

    while !host.is_shutting_down() {
        let loop_start = Instant::now();
        desktop.pump();
        if desktop.take_changed() {
            let next = monitor_geometries(&app);
            // Windows must rebuild on Explorer restart even when geometry is unchanged.
            // macOS screen-parameter notifications alone must not tear down live drawables.
            if cfg!(target_os = "windows") || surface_geometry_changed(&surface_geoms, &next) {
                tracing::info!("display geometry changed; recreating desktop surfaces");
                monitors.clear();
                desktop.teardown(&app);
                geoms = next;
                surface_geoms = geoms.clone();
                geoms_at = Instant::now();
                monitors = desktop
                    .create(&app, &instance, &geoms)
                    .into_iter()
                    .filter_map(|t| MonitorRenderer::new(&gpu.adapter, &gpu.device, t))
                    .collect();
                host.set_live_renderer_attached(!monitors.is_empty());
                crate::wallpaper_webview::reattach_all();
            } else {
                tracing::debug!("ignored display notification with unchanged geometry");
            }
        }
        if geoms_at.elapsed() >= GEOMETRY_REFRESH {
            geoms = monitor_geometries(&app);
            geoms_at = Instant::now();
        }

        let (interval, transition_secs) = {
            let s = settings.lock();
            (frame_interval(&s.fps_cap), s.transition_duration_seconds)
        };
        let snapshot = host.monitors_snapshot();
        let events = host.drain_render_events();
        let suspended = crate::power::rendering_suspended();

        let mut animating = false;
        let rt_of = |id: &str| snapshot.iter().find(|(m, _)| m == id).map(|(_, rt)| rt);
        let playing = |id: &str| !suspended && host.monitor_should_render(id);
        for m in &mut monitors {
            let rt = rt_of(&m.monitor_id);
            m.sync_content(&gpu.device, &gpu.queue, &host, rt);
            if let Content::Scene(scene) = &mut m.content {
                for RenderEvent::SceneProperties {
                    wallpaper_id,
                    changed_json,
                } in &events
                {
                    if scene.wallpaper_id.as_deref() == Some(wallpaper_id.as_str()) {
                        if let Err(err) = scene.runtime.apply_user_properties(changed_json) {
                            tracing::warn!(
                                "scene {}: property update: {err}",
                                scene.desc.scene_file
                            );
                        }
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
            animating |= m.frame(
                &gpu.device,
                &gpu.queue,
                rt,
                should_render,
                sound_owner,
                transition_secs,
                &geoms,
                &mut cursor,
            );
        }

        let budget = if animating {
            interval
        } else {
            IDLE_POLL_INTERVAL.max(interval)
        };
        if let Some(rest) = budget.checked_sub(loop_start.elapsed()) {
            std::thread::sleep(rest);
        }
    }

    tracing::info!("desktop render thread stopping");
    host.set_live_renderer_attached(false);
    monitors.clear();
    desktop.teardown(&app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        assert!(IDLE_POLL_INTERVAL >= frame_interval(&json!(30)));
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
}
