//! Smart Pause, occlusion & battery policy monitor.
//! Spec: `docs/01-SYSTEM-ARCHITECTURE.md` section 6.
//!
//! The decision logic is split into pure functions over [`ScreenRect`]s
//! ([`decide_occlusion_win32`], [`decide_occlusion_by_coverage`], [`apply_system_policy`]) so it can
//! be unit tested headlessly. The OS wrappers ([`evaluate_monitor_occlusion_win32`],
//! [`evaluate_monitor_occlusion_macos`]) only gather window rects and call into them.
//!
//! macOS uses `CGWindowListCopyWindowInfo`, never `NSWindow.occlusionState` (pitfall #2: the
//! desktop window overrides `occlusionState` to always report visible).

use crate::geometry::ScreenRect;
use std::collections::HashMap;
use std::time::Duration;

/// How often the app should re-evaluate every monitor's policy.
pub const POLICY_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Windows: a normal window covering at least this share (percent) of the work area pauses.
pub const WIN32_MAXIMIZED_COVERAGE_PERCENT: f64 = 92.0;
/// macOS: share of the monitor a single window must cover to count as fullscreen.
pub const MACOS_FULLSCREEN_COVERAGE: f64 = 0.99;
/// macOS: share of the monitor a single window must cover to count as maximized.
pub const MACOS_MAXIMIZED_COVERAGE: f64 = 0.90;

/// Per-monitor playback state. Serialized names (`as_str`) are `snake_case` to match the IPC
/// contract; this crate has no serde dependency, so the doc's `serde::Serialize` derive is
/// replaced by [`MonitorPlaybackPolicy::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonitorPlaybackPolicy {
    Playing,
    PausedFullscreen,
    PausedMaximized,
    PausedBattery,
    PausedDisplaySleep,
}

/// Name used by the task breakdown for the same type.
pub type PolicyDecision = MonitorPlaybackPolicy;

impl MonitorPlaybackPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::PausedFullscreen => "paused_fullscreen",
            Self::PausedMaximized => "paused_maximized",
            Self::PausedBattery => "paused_battery",
            Self::PausedDisplaySleep => "paused_display_sleep",
        }
    }

    pub fn is_paused(self) -> bool {
        self != Self::Playing
    }
}

/// `AppSettings.battery_policy` (`'continue' | 'pause_animations' | 'switch_to_photo'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BatteryPolicy {
    Continue,
    #[default]
    PauseAnimations,
    /// The app swaps to a static photo; the live wallpaper itself is paused.
    SwitchToPhoto,
}

impl BatteryPolicy {
    /// Parses the settings string; unknown values fall back to the default (`pause_animations`).
    pub fn from_setting(value: &str) -> Self {
        match value {
            "continue" => Self::Continue,
            "switch_to_photo" => Self::SwitchToPhoto,
            _ => Self::PauseAnimations,
        }
    }

    pub fn pauses_on_battery(self) -> bool {
        !matches!(self, Self::Continue)
    }
}

/// User toggles from `AppSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcclusionPrefs {
    pub pause_on_fullscreen: bool,
    pub pause_on_maximized: bool,
}

/// System-wide inputs that apply to every monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemPowerState {
    pub on_battery_or_low_power: bool,
    pub display_asleep: bool,
}

/// Pure Windows decision (spec 6, `evaluate_monitor_occlusion_win32`).
///
/// - `foreground`: rect of the foreground window, already excluding `Progman` / `WorkerW` /
///   `SHELLDLL_DefView`. Fullscreen when it covers the whole monitor.
/// - `normal_windows`: visible, non-minimized, non-tool, non-desktop top-level windows. Maximized
///   when one covers >= 92% of the monitor work area.
pub fn decide_occlusion_win32(
    monitor_rect: ScreenRect,
    work_area_rect: ScreenRect,
    foreground: Option<ScreenRect>,
    normal_windows: &[ScreenRect],
    prefs: OcclusionPrefs,
) -> MonitorPlaybackPolicy {
    // 1. Foreground fullscreen app/game
    if prefs.pause_on_fullscreen {
        if let Some(fg) = foreground {
            if fg.covers(&monitor_rect) {
                return MonitorPlaybackPolicy::PausedFullscreen;
            }
        }
    }

    // 2. Any visible normal window covering >= 92% of the monitor work area
    if prefs.pause_on_maximized {
        let work_area = work_area_rect.area();
        if work_area > 0.0
            && normal_windows.iter().any(|w| {
                w.intersection_area(&work_area_rect) * 100.0
                    >= work_area * WIN32_MAXIMIZED_COVERAGE_PERCENT
            })
        {
            return MonitorPlaybackPolicy::PausedMaximized;
        }
    }

    MonitorPlaybackPolicy::Playing
}

/// Pure macOS decision (spec 6, `evaluate_monitor_occlusion_macos`). `monitor` and
/// `window_rects` are in Quartz top-left space. The first window that crosses a threshold wins.
pub fn decide_occlusion_by_coverage(
    monitor: ScreenRect,
    window_rects: &[ScreenRect],
    prefs: OcclusionPrefs,
) -> MonitorPlaybackPolicy {
    let monitor_area = monitor.area().max(1.0);
    for w in window_rects {
        let inter_area = monitor.intersection_area(w);
        if inter_area <= 0.0 {
            continue;
        }
        if prefs.pause_on_fullscreen && inter_area >= monitor_area * MACOS_FULLSCREEN_COVERAGE {
            return MonitorPlaybackPolicy::PausedFullscreen;
        }
        if prefs.pause_on_maximized && inter_area >= monitor_area * MACOS_MAXIMIZED_COVERAGE {
            return MonitorPlaybackPolicy::PausedMaximized;
        }
    }
    MonitorPlaybackPolicy::Playing
}

/// Combines the per-monitor occlusion result with system state.
/// Precedence: display sleep > fullscreen/maximized occlusion > battery.
pub fn apply_system_policy(
    occlusion: MonitorPlaybackPolicy,
    system: SystemPowerState,
    battery_policy: BatteryPolicy,
) -> MonitorPlaybackPolicy {
    if system.display_asleep {
        return MonitorPlaybackPolicy::PausedDisplaySleep;
    }
    if occlusion.is_paused() {
        return occlusion;
    }
    if system.on_battery_or_low_power && battery_policy.pauses_on_battery() {
        return MonitorPlaybackPolicy::PausedBattery;
    }
    MonitorPlaybackPolicy::Playing
}

/// Tracks the last decision per monitor so callers only act on transitions
/// (spec 6.1: pause the compositor / video / web host, suspend the FFT thread when all paused).
#[derive(Debug, Default)]
pub struct PolicyMonitor {
    states: HashMap<String, MonitorPlaybackPolicy>,
}

impl PolicyMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `decision` for `monitor_id`; returns `Some(decision)` only if it changed
    /// (a monitor seen for the first time always reports).
    pub fn update(
        &mut self,
        monitor_id: &str,
        decision: MonitorPlaybackPolicy,
    ) -> Option<MonitorPlaybackPolicy> {
        match self.states.insert(monitor_id.to_string(), decision) {
            Some(prev) if prev == decision => None,
            _ => Some(decision),
        }
    }

    pub fn get(&self, monitor_id: &str) -> Option<MonitorPlaybackPolicy> {
        self.states.get(monitor_id).copied()
    }

    /// Drops monitors that were unplugged.
    pub fn retain_monitors<'a>(&mut self, live_ids: impl IntoIterator<Item = &'a str>) {
        let live: std::collections::HashSet<&str> = live_ids.into_iter().collect();
        self.states.retain(|id, _| live.contains(id.as_str()));
    }

    /// `true` when at least one monitor is tracked and none are `Playing` (suspend audio FFT).
    pub fn all_paused(&self) -> bool {
        !self.states.is_empty() && self.states.values().all(|p| p.is_paused())
    }
}

/// Cross-platform check for whether the system is running on laptop battery or Low Power Mode.
pub fn is_on_battery_or_low_power() -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut status = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut status).is_ok() {
            // ACLineStatus == 0 means offline (on battery); SystemStatusFlag == 1 means Battery Saver on
            return status.ACLineStatus == 0 || status.SystemStatusFlag == 1;
        }
        false
    }

    #[cfg(target_os = "macos")]
    {
        // NSProcessInfo is thread-safe; isLowPowerModeEnabled may be queried off the main thread.
        objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled()
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        false
    }
}

#[cfg(target_os = "windows")]
fn rect_from_win32(r: &windows::Win32::Foundation::RECT) -> ScreenRect {
    ScreenRect::from_ltrb(r.left as f64, r.top as f64, r.right as f64, r.bottom as f64)
}

/// Windows: gathers the foreground window and all candidate top-level windows, then decides
/// via [`decide_occlusion_win32`].
///
/// # Safety
/// Calls raw Win32 APIs; call from a thread with a Win32 message context, passing handles
/// obtained from [`setup_desktop_layer`](crate::win_desktop::setup_desktop_layer).
#[cfg(target_os = "windows")]
pub unsafe fn evaluate_monitor_occlusion_win32(
    monitor_rect: windows::Win32::Foundation::RECT,
    work_area_rect: windows::Win32::Foundation::RECT,
    pause_on_fullscreen: bool,
    pause_on_maximized: bool,
    desktop: &crate::win_desktop::DesktopHandles,
) -> MonitorPlaybackPolicy {
    use windows::core::{w, BOOL, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::*;

    let prefs = OcclusionPrefs {
        pause_on_fullscreen,
        pause_on_maximized,
    };

    let mut foreground = None;
    if pause_on_fullscreen {
        let fg = GetForegroundWindow();
        if !fg.0.is_null()
            && fg != desktop.progman
            && fg != desktop.worker_w
            && fg != desktop.shell_dll_defview
        {
            let mut fg_rect = RECT::default();
            if GetWindowRect(fg, &mut fg_rect).is_ok() {
                foreground = Some(rect_from_win32(&fg_rect));
            }
        }
    }

    let mut normal_windows: Vec<ScreenRect> = Vec::new();
    if pause_on_maximized {
        struct OccCtx {
            progman: HWND,
            worker_w: HWND,
            rects: Vec<ScreenRect>,
        }
        let mut ctx = OccCtx {
            progman: desktop.progman,
            worker_w: desktop.worker_w,
            rects: Vec::new(),
        };

        unsafe extern "system" fn collect_win(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let ctx = &mut *(lparam.0 as *mut OccCtx);
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return BOOL(1);
            }
            if hwnd == ctx.progman || hwnd == ctx.worker_w {
                return BOOL(1);
            }
            // Classic desktop: the top-level WorkerW hosting SHELLDLL_DefView spans the monitor.
            if let Ok(def_view) =
                FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), PCWSTR::null())
            {
                if !def_view.0.is_null() {
                    return BOOL(1);
                }
            }
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if (ex_style & WS_EX_TOOLWINDOW.0) != 0 {
                return BOOL(1);
            }
            let mut w_rect = RECT::default();
            if GetWindowRect(hwnd, &mut w_rect).is_ok() {
                ctx.rects.push(rect_from_win32(&w_rect));
            }
            BOOL(1)
        }

        let _ = EnumWindows(Some(collect_win), LPARAM(&mut ctx as *mut OccCtx as isize));
        normal_windows = ctx.rects;
    }

    decide_occlusion_win32(
        rect_from_win32(&monitor_rect),
        rect_from_win32(&work_area_rect),
        foreground,
        &normal_windows,
        prefs,
    )
}

/// macOS: `monitor_quartz_rect` is `(x, y, width, height)` in Quartz top-left space
/// (see [`crate::cursor::cocoa_rect_to_quartz`]).
#[cfg(target_os = "macos")]
pub fn evaluate_monitor_occlusion_macos(
    monitor_quartz_rect: (f64, f64, f64, f64),
    pause_on_fullscreen: bool,
    pause_on_maximized: bool,
) -> MonitorPlaybackPolicy {
    let (mx, my, mw, mh) = monitor_quartz_rect;
    decide_occlusion_by_coverage(
        ScreenRect::new(mx, my, mw, mh),
        &crate::cursor::query_normal_window_bounds_macos(),
        OcclusionPrefs {
            pause_on_fullscreen,
            pause_on_maximized,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: OcclusionPrefs = OcclusionPrefs {
        pause_on_fullscreen: true,
        pause_on_maximized: true,
    };
    const FS_ONLY: OcclusionPrefs = OcclusionPrefs {
        pause_on_fullscreen: true,
        pause_on_maximized: false,
    };
    const NONE: OcclusionPrefs = OcclusionPrefs {
        pause_on_fullscreen: false,
        pause_on_maximized: false,
    };

    fn primary() -> ScreenRect {
        ScreenRect::from_ltrb(0.0, 0.0, 1920.0, 1080.0)
    }
    fn primary_work() -> ScreenRect {
        ScreenRect::from_ltrb(0.0, 0.0, 1920.0, 1040.0)
    }
    fn secondary() -> ScreenRect {
        ScreenRect::from_ltrb(1920.0, 0.0, 4480.0, 1440.0)
    }

    #[test]
    fn win32_fullscreen_foreground_on_monitor_pauses() {
        let d = decide_occlusion_win32(primary(), primary_work(), Some(primary()), &[], BOTH);
        assert_eq!(d, MonitorPlaybackPolicy::PausedFullscreen);
        // Borderless games often overshoot by a few pixels.
        let over = ScreenRect::from_ltrb(-8.0, -8.0, 1928.0, 1088.0);
        assert_eq!(
            decide_occlusion_win32(primary(), primary_work(), Some(over), &[], FS_ONLY),
            MonitorPlaybackPolicy::PausedFullscreen
        );
    }

    #[test]
    fn win32_fullscreen_on_other_monitor_does_not_pause() {
        let d = decide_occlusion_win32(secondary(), secondary(), Some(primary()), &[], BOTH);
        assert_eq!(d, MonitorPlaybackPolicy::Playing);
    }

    #[test]
    fn win32_maximized_window_pauses_only_when_enabled() {
        let maximized = ScreenRect::from_ltrb(-8.0, -8.0, 1928.0, 1048.0);
        assert_eq!(
            decide_occlusion_win32(primary(), primary_work(), None, &[maximized], BOTH),
            MonitorPlaybackPolicy::PausedMaximized
        );
        assert_eq!(
            decide_occlusion_win32(primary(), primary_work(), None, &[maximized], FS_ONLY),
            MonitorPlaybackPolicy::Playing
        );
        // A maximized (not fullscreen) foreground window does not count as fullscreen.
        assert_eq!(
            decide_occlusion_win32(primary(), primary_work(), Some(maximized), &[], FS_ONLY),
            MonitorPlaybackPolicy::Playing
        );
    }

    #[test]
    fn win32_coverage_threshold_is_92_percent() {
        let work = primary_work(); // 1920 x 1040
        let w91 = ScreenRect::new(0.0, 0.0, 1920.0 * 0.91, 1040.0);
        let w92 = ScreenRect::new(0.0, 0.0, 1920.0 * 0.92, 1040.0);
        assert_eq!(
            decide_occlusion_win32(primary(), work, None, &[w91], BOTH),
            MonitorPlaybackPolicy::Playing
        );
        assert_eq!(
            decide_occlusion_win32(primary(), work, None, &[w92], BOTH),
            MonitorPlaybackPolicy::PausedMaximized
        );
        // Two half windows never combine into occlusion: each is checked on its own.
        let left = ScreenRect::new(0.0, 0.0, 960.0, 1040.0);
        let right = ScreenRect::new(960.0, 0.0, 960.0, 1040.0);
        assert_eq!(
            decide_occlusion_win32(primary(), work, None, &[left, right], BOTH),
            MonitorPlaybackPolicy::Playing
        );
    }

    #[test]
    fn no_windows_means_playing() {
        assert_eq!(
            decide_occlusion_win32(primary(), primary_work(), None, &[], BOTH),
            MonitorPlaybackPolicy::Playing
        );
        assert_eq!(
            decide_occlusion_by_coverage(primary(), &[], BOTH),
            MonitorPlaybackPolicy::Playing
        );
        let full = primary();
        assert_eq!(
            decide_occlusion_win32(primary(), primary_work(), Some(full), &[full], NONE),
            MonitorPlaybackPolicy::Playing
        );
    }

    #[test]
    fn macos_coverage_thresholds() {
        let m = primary();
        let full = primary();
        // Zoomed window below the 24pt menu bar and above a 70pt Dock: ~91% coverage.
        let zoomed = ScreenRect::new(0.0, 25.0, 1920.0, 1080.0 - 25.0 - 70.0);
        let small = ScreenRect::new(100.0, 100.0, 800.0, 600.0);

        assert_eq!(
            decide_occlusion_by_coverage(m, &[small, full], BOTH),
            MonitorPlaybackPolicy::PausedFullscreen
        );
        assert_eq!(
            decide_occlusion_by_coverage(m, &[zoomed], BOTH),
            MonitorPlaybackPolicy::PausedMaximized
        );
        assert_eq!(
            decide_occlusion_by_coverage(m, &[zoomed], FS_ONLY),
            MonitorPlaybackPolicy::Playing
        );
        assert_eq!(
            decide_occlusion_by_coverage(m, &[small], BOTH),
            MonitorPlaybackPolicy::Playing
        );
        // Fullscreen window on the other display (Quartz: secondary above primary, negative y).
        let other = ScreenRect::new(0.0, -1024.0, 1280.0, 1024.0);
        assert_eq!(
            decide_occlusion_by_coverage(m, &[other], BOTH),
            MonitorPlaybackPolicy::Playing
        );
        assert_eq!(
            decide_occlusion_by_coverage(other, &[other], BOTH),
            MonitorPlaybackPolicy::PausedFullscreen
        );
    }

    #[test]
    fn battery_policy_and_display_sleep() {
        let on_battery = SystemPowerState {
            on_battery_or_low_power: true,
            display_asleep: false,
        };
        let asleep = SystemPowerState {
            on_battery_or_low_power: false,
            display_asleep: true,
        };
        let idle = SystemPowerState::default();
        let p = MonitorPlaybackPolicy::Playing;

        assert_eq!(
            apply_system_policy(p, on_battery, BatteryPolicy::PauseAnimations),
            MonitorPlaybackPolicy::PausedBattery
        );
        assert_eq!(
            apply_system_policy(p, on_battery, BatteryPolicy::SwitchToPhoto),
            MonitorPlaybackPolicy::PausedBattery
        );
        assert_eq!(
            apply_system_policy(p, on_battery, BatteryPolicy::Continue),
            MonitorPlaybackPolicy::Playing
        );
        assert_eq!(
            apply_system_policy(p, idle, BatteryPolicy::PauseAnimations),
            MonitorPlaybackPolicy::Playing
        );
        assert_eq!(
            apply_system_policy(
                MonitorPlaybackPolicy::PausedFullscreen,
                on_battery,
                BatteryPolicy::PauseAnimations
            ),
            MonitorPlaybackPolicy::PausedFullscreen
        );
        assert_eq!(
            apply_system_policy(
                MonitorPlaybackPolicy::PausedMaximized,
                asleep,
                BatteryPolicy::Continue
            ),
            MonitorPlaybackPolicy::PausedDisplaySleep
        );
    }

    #[test]
    fn battery_policy_parsing_and_names() {
        assert_eq!(
            BatteryPolicy::from_setting("continue"),
            BatteryPolicy::Continue
        );
        assert_eq!(
            BatteryPolicy::from_setting("switch_to_photo"),
            BatteryPolicy::SwitchToPhoto
        );
        assert_eq!(
            BatteryPolicy::from_setting("pause_animations"),
            BatteryPolicy::PauseAnimations
        );
        assert_eq!(
            BatteryPolicy::from_setting("bogus"),
            BatteryPolicy::PauseAnimations
        );
        assert_eq!(MonitorPlaybackPolicy::Playing.as_str(), "playing");
        assert_eq!(
            MonitorPlaybackPolicy::PausedDisplaySleep.as_str(),
            "paused_display_sleep"
        );
    }

    #[test]
    fn policy_monitor_reports_transitions_only() {
        let mut pm = PolicyMonitor::new();
        assert!(!pm.all_paused());
        assert_eq!(
            pm.update("A", MonitorPlaybackPolicy::Playing),
            Some(MonitorPlaybackPolicy::Playing)
        );
        assert_eq!(pm.update("A", MonitorPlaybackPolicy::Playing), None);
        assert_eq!(
            pm.update("A", MonitorPlaybackPolicy::PausedMaximized),
            Some(MonitorPlaybackPolicy::PausedMaximized)
        );
        pm.update("B", MonitorPlaybackPolicy::Playing);
        assert!(!pm.all_paused());
        pm.update("B", MonitorPlaybackPolicy::PausedFullscreen);
        assert!(pm.all_paused());
        pm.retain_monitors(["A"]);
        assert_eq!(pm.get("B"), None);
        assert_eq!(pm.get("A"), Some(MonitorPlaybackPolicy::PausedMaximized));
    }
}
