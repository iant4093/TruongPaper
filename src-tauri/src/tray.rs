// src-tauri/src/tray.rs
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Listener, Manager, PhysicalPosition, PhysicalSize, Position, Rect, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

const POPOVER_LABEL: &str = "tray-popover";
/// A hidden popover is closed after this long, so its web view does not hold memory while
/// nobody uses it. The next tray click builds it again.
const POPOVER_IDLE_CLOSE: std::time::Duration = std::time::Duration::from_secs(60);
/// Bumped every time the popover is shown; a pending close only runs if it did not change.
static POPOVER_SHOWN: AtomicU64 = AtomicU64::new(0);

/// When the popover last hid because it lost focus (see `toggle_tray_popover`).
static LAST_BLUR_HIDE: parking_lot::Mutex<Option<std::time::Instant>> =
    parking_lot::Mutex::new(None);

/// When the popover was last shown, and how often it has taken its focus back since.
static LAST_SHOW: parking_lot::Mutex<Option<(std::time::Instant, u32)>> =
    parking_lot::Mutex::new(None);

/// On Windows, opening the popover from the hidden-icons flyout hands the focus back to the
/// taskbar as that flyout closes, a moment after the popover appeared. A focus loss this soon
/// after showing is that, not the user clicking away, so the popover takes the focus back.
const SHOW_FOCUS_GRACE: std::time::Duration = std::time::Duration::from_millis(700);
const SHOW_FOCUS_RETRIES: u32 = 3;

/// Whether a focus loss right now should take the focus back instead of hiding the popover.
fn reclaim_focus_after_show() -> bool {
    if !cfg!(windows) {
        return false;
    }
    let mut last = LAST_SHOW.lock();
    match last.as_mut() {
        Some((at, retries)) => should_reclaim(at.elapsed(), retries),
        None => false,
    }
}

/// `true` (and counts the retry) while still inside [`SHOW_FOCUS_GRACE`] with retries left.
fn should_reclaim(since_show: std::time::Duration, retries: &mut u32) -> bool {
    if since_show < SHOW_FOCUS_GRACE && *retries < SHOW_FOCUS_RETRIES {
        *retries += 1;
        true
    } else {
        false
    }
}

const PAUSE_LABEL: &str = "Pause All Displays";
const RESUME_LABEL: &str = "Resume All Displays";

/// Label of the tray's pause item: resuming only when every connected display is paused (the
/// condition `controls::toggle_pause_now` uses).
pub fn pause_label(all_paused: bool) -> &'static str {
    if all_paused {
        RESUME_LABEL
    } else {
        PAUSE_LABEL
    }
}

/// Managed handle of the tray's pause item, so its label can follow the pause state.
struct PauseItem(MenuItem<tauri::Wry>);

fn refresh_pause_label(app: &AppHandle) {
    let (Some(item), Some(state)) = (
        app.try_state::<PauseItem>(),
        app.try_state::<crate::AppState>(),
    ) else {
        return;
    };
    let monitors = crate::commands::monitor::live_monitors(app);
    let all_paused = if monitors.is_empty() {
        state.host.user_paused("")
    } else {
        monitors.iter().all(|m| state.host.user_paused(&m.id))
    };
    let _ = item.0.set_text(pause_label(all_paused));
}

pub(crate) fn show_studio(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn setup_system_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_studio = MenuItem::with_id(
        app,
        "open_studio",
        "Open TruongPaper Studio",
        true,
        None::<&str>,
    )?;
    let next_wallpaper =
        MenuItem::with_id(app, "next_wallpaper", "Next Wallpaper", true, None::<&str>)?;
    let toggle_pause = MenuItem::with_id(app, "toggle_pause", PAUSE_LABEL, true, None::<&str>)?;
    app.manage(PauseItem(toggle_pause.clone()));
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit TruongPaper", true, None::<&str>)?;

    let context_menu = Menu::with_items(
        app,
        &[&open_studio, &next_wallpaper, &toggle_pause, &sep, &quit],
    )?;

    let icon = if cfg!(target_os = "macos") {
        // Template images need a transparent mark, not an opaque application-icon tile.
        tauri::include_image!("icons/tray-icon.png").clone()
    } else {
        app.default_window_icon()
            .cloned()
            .unwrap_or_else(|| Image::new(&[], 0, 0))
    };

    TrayIconBuilder::with_id("truongpaper-tray")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("TruongPaper")
        .menu(&context_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open_studio" => {
                show_studio(app);
            }
            // Same Rust handlers as the global hotkeys; they work with Studio closed.
            "next_wallpaper" => crate::controls::next_wallpaper(app),
            "toggle_pause" => crate::controls::toggle_pause(app),
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                toggle_tray_popover(tray.app_handle(), rect);
            }
        })
        .build(app)?;

    let handle = app.clone();
    app.listen(crate::models::EVENT_PLAYBACK_PAUSE_CHANGED, move |_| {
        refresh_pause_label(&handle);
    });
    refresh_pause_label(app);
    Ok(())
}

/// The popover window, built on first use (it is not created at launch).
fn popover_window(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(window) = app.get_webview_window(POPOVER_LABEL) {
        return Some(window);
    }
    let window = WebviewWindowBuilder::new(app, POPOVER_LABEL, WebviewUrl::App("tray.html".into()))
        .title("TruongPaper Quick Controls")
        .inner_size(POPOVER_WIDTH, POPOVER_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .theme(Some(tauri::Theme::Dark))
        .build()
        .map_err(|err| tracing::warn!("tray popover: {err}"))
        .ok()?;
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            if handle.is_visible().unwrap_or(false) && reclaim_focus_after_show() {
                let _ = handle.set_focus();
                return;
            }
            if handle.is_visible().unwrap_or(false) {
                *LAST_BLUR_HIDE.lock() = Some(std::time::Instant::now());
            }
            hide_popover(&handle);
        }
    });
    Some(window)
}

/// Hides the popover and closes it once it has stayed hidden for [`POPOVER_IDLE_CLOSE`].
fn hide_popover(popover: &WebviewWindow) {
    let _ = popover.hide();
    let shown = POPOVER_SHOWN.load(Ordering::Acquire);
    let popover = popover.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(POPOVER_IDLE_CLOSE).await;
        if POPOVER_SHOWN.load(Ordering::Acquire) == shown && !popover.is_visible().unwrap_or(true) {
            let _ = popover.close();
        }
    });
}

fn toggle_tray_popover(app: &AppHandle, tray_rect: Rect) {
    if app
        .get_webview_window(POPOVER_LABEL)
        .is_some_and(|p| p.is_visible().unwrap_or(false))
    {
        if let Some(popover) = app.get_webview_window(POPOVER_LABEL) {
            hide_popover(&popover);
        }
        return;
    }
    // On Windows a click on the tray icon first blurs the popover (which hides it), then
    // arrives here: without this the click meant to close it would open it again.
    if LAST_BLUR_HIDE
        .lock()
        .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(300))
    {
        return;
    }
    let Some(popover) = popover_window(app) else {
        return;
    };

    // The hidden popover's own monitor and scale say nothing about where the tray is: use the
    // monitor under the icon (its DPI, bounds and work area).
    let fallback_scale = popover.scale_factor().unwrap_or(1.0);
    let (icon_x, icon_y) = match tray_rect.position {
        Position::Physical(p) => (p.x as f64, p.y as f64),
        Position::Logical(l) => (l.x * fallback_scale, l.y * fallback_scale),
    };
    let (icon_w, icon_h) = match tray_rect.size {
        tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
        tauri::Size::Logical(l) => (l.width * fallback_scale, l.height * fallback_scale),
    };
    let monitor = app
        .monitor_from_point(icon_x + icon_w / 2.0, icon_y + icon_h / 2.0)
        .ok()
        .flatten()
        .or_else(|| popover.current_monitor().ok().flatten());
    let scale = monitor
        .as_ref()
        .map(|m| m.scale_factor())
        .unwrap_or(fallback_scale);
    let popover_size = PhysicalSize::new(
        (POPOVER_WIDTH * scale).round() as u32,
        (POPOVER_HEIGHT * scale).round() as u32,
    );
    let _ = popover.set_size(tauri::Size::Physical(popover_size));
    let (target_x, target_y) = match &monitor {
        Some(m) => {
            let (mp, ms) = (m.position(), m.size());
            let wa = m.work_area();
            place_popover(
                IRect::new(icon_x, icon_y, icon_w, icon_h),
                (popover_size.width as f64, popover_size.height as f64),
                IRect::new(mp.x as f64, mp.y as f64, ms.width as f64, ms.height as f64),
                IRect::new(
                    wa.position.x as f64,
                    wa.position.y as f64,
                    wa.size.width as f64,
                    wa.size.height as f64,
                ),
                6.0 * scale,
            )
        }
        None => (
            (icon_x + icon_w / 2.0 - popover_size.width as f64 / 2.0).round() as i32,
            (icon_y - popover_size.height as f64).round() as i32,
        ),
    };

    let _ = popover.set_position(Position::Physical(PhysicalPosition::new(
        target_x, target_y,
    )));
    POPOVER_SHOWN.fetch_add(1, Ordering::AcqRel);
    *LAST_SHOW.lock() = Some((std::time::Instant::now(), 0));
    let _ = popover.show();
    let _ = popover.set_focus();
}

/// Popover size in logical points.
const POPOVER_WIDTH: f64 = 360.0;
const POPOVER_HEIGHT: f64 = 480.0;

/// A physical-pixel rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
struct IRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl IRect {
    fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// Top-left of the popover: next to the tray icon on the inner side of the taskbar, which may
/// be at the bottom, top, left or right (found by comparing the work area with the monitor),
/// kept inside the work area so it never covers the taskbar. The macOS menu bar is a top bar.
fn place_popover(
    icon: IRect,
    size: (f64, f64),
    monitor: IRect,
    work: IRect,
    margin: f64,
) -> (i32, i32) {
    let (w, h) = size;
    let (cx, cy) = (icon.x + icon.w / 2.0, icon.y + icon.h / 2.0);
    // The edge the taskbar takes room from; with an auto-hidden taskbar (work area equals the
    // monitor) the nearest monitor edge to the icon.
    #[derive(PartialEq)]
    enum Edge {
        Top,
        Bottom,
        Left,
        Right,
    }
    let edge = if work.y > monitor.y {
        Edge::Top
    } else if work.bottom() < monitor.bottom() {
        Edge::Bottom
    } else if work.x > monitor.x {
        Edge::Left
    } else if work.right() < monitor.right() {
        Edge::Right
    } else {
        let d = [
            (cy - monitor.y, 0),
            (monitor.bottom() - cy, 1),
            (cx - monitor.x, 2),
            (monitor.right() - cx, 3),
        ];
        match d.iter().min_by(|a, b| a.0.total_cmp(&b.0)).map(|d| d.1) {
            Some(0) => Edge::Top,
            Some(2) => Edge::Left,
            Some(3) => Edge::Right,
            _ => Edge::Bottom,
        }
    };
    let (x, y) = match edge {
        Edge::Bottom => (cx - w / 2.0, work.bottom().min(icon.y) - h - margin),
        Edge::Top => (cx - w / 2.0, work.y.max(icon.bottom()) + margin),
        Edge::Left => (work.x.max(icon.right()) + margin, cy - h / 2.0),
        Edge::Right => (work.right().min(icon.x) - w - margin, cy - h / 2.0),
    };
    let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi.max(lo));
    (
        clamp(x, work.x + margin, work.right() - w - margin).round() as i32,
        clamp(y, work.y + margin, work.bottom() - h - margin).round() as i32,
    )
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    const SIZE: (f64, f64) = (360.0, 480.0);
    const MON: IRect = IRect {
        x: 0.0,
        y: 0.0,
        w: 2560.0,
        h: 1440.0,
    };

    #[test]
    fn bottom_taskbar_opens_above_the_icon() {
        let work = IRect::new(0.0, 0.0, 2560.0, 1392.0);
        let icon = IRect::new(2300.0, 1400.0, 24.0, 24.0);
        let (x, y) = place_popover(icon, SIZE, MON, work, 6.0);
        assert_eq!(y, 1392 - 480 - 6);
        assert_eq!(x, 2312 - 180);
    }

    #[test]
    fn top_taskbar_opens_below_the_icon() {
        let work = IRect::new(0.0, 48.0, 2560.0, 1392.0);
        let icon = IRect::new(2300.0, 12.0, 24.0, 24.0);
        assert_eq!(place_popover(icon, SIZE, MON, work, 6.0).1, 54);
    }

    #[test]
    fn side_taskbars_open_beside_the_icon() {
        let left = IRect::new(62.0, 0.0, 2498.0, 1440.0);
        let icon = IRect::new(20.0, 1300.0, 24.0, 24.0);
        let (x, y) = place_popover(icon, SIZE, MON, left, 6.0);
        assert_eq!(x, 68);
        assert_eq!(y, 1440 - 480 - 6);
        let right = IRect::new(0.0, 0.0, 2498.0, 1440.0);
        let icon = IRect::new(2516.0, 700.0, 24.0, 24.0);
        assert_eq!(place_popover(icon, SIZE, MON, right, 6.0).0, 2498 - 360 - 6);
    }

    #[test]
    fn auto_hidden_taskbar_uses_the_nearest_edge_and_stays_on_the_monitor() {
        // Second monitor above-right of the primary (negative y), taskbar auto-hidden.
        let mon = IRect::new(2560.0, -820.0, 1440.0, 2560.0);
        let icon = IRect::new(3900.0, 1700.0, 24.0, 24.0);
        let (x, y) = place_popover(icon, SIZE, mon, mon, 6.0);
        assert!((-820..1700).contains(&y), "{y}");
        assert!((2560..=4000 - 360).contains(&x), "{x}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_lost_right_after_showing_is_taken_back_a_few_times() {
        use std::time::Duration;
        let mut retries = 0;
        for _ in 0..SHOW_FOCUS_RETRIES {
            assert!(should_reclaim(Duration::from_millis(50), &mut retries));
        }
        // Out of retries: the popover hides as usual.
        assert!(!should_reclaim(Duration::from_millis(60), &mut retries));
        // A click away after the grace period always hides it.
        let mut retries = 0;
        assert!(!should_reclaim(SHOW_FOCUS_GRACE, &mut retries));
        assert_eq!(retries, 0);
    }

    #[test]
    fn pause_item_says_all_displays() {
        assert_eq!(pause_label(false), "Pause All Displays");
        assert_eq!(pause_label(true), "Resume All Displays");
    }
}
