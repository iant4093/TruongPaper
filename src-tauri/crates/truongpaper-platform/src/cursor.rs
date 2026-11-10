//! Passive desktop cursor forwarding & click-through hit-testing.
//! Spec: `docs/01-SYSTEM-ARCHITECTURE.md` section 5.
//!
//! The wallpaper window sits behind the desktop icons and never takes focus, so the cursor is
//! polled and normalized to `[0, 1]` monitor-local coordinates (top-left origin) for
//! `g_PointerPosition`, SceneScript `cursorMove` / `cursorDown` and Web wallpapers.

use crate::geometry::ScreenRect;

/// Normalizes a point in a top-left-origin space (Win32 virtual screen / Quartz) to `[0, 1]`
/// coordinates local to `monitor`. Returns `None` when the point is outside the monitor
/// (half-open bounds, so adjacent monitors never both claim a point).
pub fn normalize_point_top_left(px: f64, py: f64, monitor: ScreenRect) -> Option<(f32, f32)> {
    if !monitor.contains_half_open(px, py) {
        return None;
    }
    let norm_x = ((px - monitor.x) / monitor.width.max(1.0)) as f32;
    let norm_y = ((py - monitor.y) / monitor.height.max(1.0)) as f32;
    Some((norm_x.clamp(0.0, 1.0), norm_y.clamp(0.0, 1.0)))
}

/// Normalizes a Cocoa global point (bottom-left origin, `NSEvent.mouseLocation`) against a Cocoa
/// `NSScreen.frame` and flips Y so the result is top-down `[0, 1]`.
pub fn normalize_cocoa_point(px: f64, py: f64, cocoa_frame: ScreenRect) -> Option<(f32, f32)> {
    if !cocoa_frame.contains_half_open(px, py) {
        return None;
    }
    let norm_x = ((px - cocoa_frame.x) / cocoa_frame.width.max(1.0)) as f32;
    // Cocoa Y is bottom-up; convert to top-down [0.0, 1.0]
    let norm_y = (1.0 - (py - cocoa_frame.y) / cocoa_frame.height.max(1.0)) as f32;
    Some((norm_x.clamp(0.0, 1.0), norm_y.clamp(0.0, 1.0)))
}

/// Converts a Cocoa global point to Quartz global coordinates ((0,0) = top-left of the primary
/// display). `primary_screen_height` is `NSScreen.screens[0].frame.size.height`.
pub fn cocoa_point_to_quartz(px: f64, py: f64, primary_screen_height: f64) -> (f64, f64) {
    (px, primary_screen_height - py)
}

/// Converts a Cocoa `NSScreen.frame` (bottom-left origin) into the equivalent Quartz rect, which
/// is the space `CGWindowListCopyWindowInfo` reports `kCGWindowBounds` in.
pub fn cocoa_rect_to_quartz(cocoa_frame: ScreenRect, primary_screen_height: f64) -> ScreenRect {
    ScreenRect::new(
        cocoa_frame.x,
        primary_screen_height - (cocoa_frame.y + cocoa_frame.height),
        cocoa_frame.width,
        cocoa_frame.height,
    )
}

/// `true` when any of `window_rects` covers the point (closed bounds, as in the spec).
pub fn is_point_covered_by_any(px: f64, py: f64, window_rects: &[ScreenRect]) -> bool {
    window_rects.iter().any(|r| r.contains_inclusive(px, py))
}

/// Windows: returns `(norm_x, norm_y, is_desktop_visible_under_cursor)` for the monitor that
/// contains the cursor, or `None` if the cursor is on another monitor.
///
/// # Safety
/// Calls raw Win32 APIs; call from a thread with a Win32 message context, passing handles
/// obtained from [`setup_desktop_layer`](crate::win_desktop::setup_desktop_layer).
#[cfg(target_os = "windows")]
pub unsafe fn poll_desktop_cursor_win32(
    monitor_bounds: windows::Win32::Foundation::RECT,
    desktop: &crate::win_desktop::DesktopHandles,
) -> Option<(f32, f32, bool)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, WindowFromPoint};

    let mut pt = POINT::default();
    GetCursorPos(&mut pt).ok()?;

    let monitor = ScreenRect::from_ltrb(
        monitor_bounds.left as f64,
        monitor_bounds.top as f64,
        monitor_bounds.right as f64,
        monitor_bounds.bottom as f64,
    );
    let (norm_x, norm_y) = normalize_point_top_left(pt.x as f64, pt.y as f64, monitor)?;

    let hovered_hwnd = WindowFromPoint(pt);
    let is_desktop_visible_under_cursor = hovered_hwnd == desktop.shell_dll_defview
        || hovered_hwnd == desktop.worker_w
        || hovered_hwnd == desktop.progman;

    Some((norm_x, norm_y, is_desktop_visible_under_cursor))
}

/// macOS: returns `(norm_x, norm_y, left_pressed)` for `screen_frame` (Cocoa coordinates), where
/// `left_pressed` is suppressed whenever a normal app window (`kCGWindowLayer == 0`) is under the
/// cursor so clicks in other apps never reach the wallpaper.
#[cfg(target_os = "macos")]
pub fn poll_desktop_cursor_macos(
    screen_frame: objc2_foundation::NSRect,
    primary_screen_height: f64,
) -> Option<(f32, f32, bool)> {
    use objc2_app_kit::NSEvent;

    let mouse_cocoa = NSEvent::mouseLocation();
    let frame = ScreenRect::new(
        screen_frame.origin.x,
        screen_frame.origin.y,
        screen_frame.size.width,
        screen_frame.size.height,
    );
    let (norm_x, norm_y) = normalize_cocoa_point(mouse_cocoa.x, mouse_cocoa.y, frame)?;

    let (quartz_x, quartz_y) =
        cocoa_point_to_quartz(mouse_cocoa.x, mouse_cocoa.y, primary_screen_height);

    let pressed_buttons = NSEvent::pressedMouseButtons();
    let mut left_pressed = (pressed_buttons & 1) != 0;

    // Only report left_pressed = true if no normal application window (kCGWindowLayer == 0) covers (quartz_x, quartz_y)
    if left_pressed && is_point_covered_by_normal_window_macos(quartz_x, quartz_y) {
        left_pressed = false;
    }

    Some((norm_x, norm_y, left_pressed))
}

/// Bounds (Quartz, top-left origin) of every on-screen normal window (`kCGWindowLayer == 0`).
/// Uses `CGWindowListCopyWindowInfo`, never `NSWindow.occlusionState`.
#[cfg(target_os = "macos")]
pub fn query_normal_window_bounds_macos() -> Vec<ScreenRect> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
        CGWindowListCopyWindowInfo,
    };

    let mut rects = Vec::new();
    let arr_ptr = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if arr_ptr.is_null() {
        return rects;
    }
    let arr: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_create_rule(arr_ptr as _) };
    let k_layer = CFString::from_static_string("kCGWindowLayer");
    let k_bounds = CFString::from_static_string("kCGWindowBounds");
    let k_x = CFString::from_static_string("X");
    let k_y = CFString::from_static_string("Y");
    let k_w = CFString::from_static_string("Width");
    let k_h = CFString::from_static_string("Height");

    for dict in arr.iter() {
        let Some(layer_ref) = dict.find(&k_layer) else {
            continue;
        };
        let Some(layer_num) = layer_ref.downcast::<CFNumber>() else {
            continue;
        };
        if layer_num.to_i32() != Some(0) {
            continue;
        }
        let Some(bounds_ref) = dict.find(&k_bounds) else {
            continue;
        };
        let bounds_dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds_ref.as_CFTypeRef() as _) };
        let get_f64 = |key: &CFString| -> Option<f64> {
            bounds_dict.find(key)?.downcast::<CFNumber>()?.to_f64()
        };
        if let (Some(x), Some(y), Some(w), Some(h)) =
            (get_f64(&k_x), get_f64(&k_y), get_f64(&k_w), get_f64(&k_h))
        {
            if w > 1.0 && h > 1.0 {
                rects.push(ScreenRect::new(x, y, w, h));
            }
        }
    }
    rects
}

#[cfg(target_os = "macos")]
fn is_point_covered_by_normal_window_macos(quartz_x: f64, quartz_y: f64) -> bool {
    is_point_covered_by_any(quartz_x, quartz_y, &query_normal_window_bounds_macos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_style_normalization_is_monitor_local() {
        // Secondary monitor to the right of a 1920x1080 primary.
        let m = ScreenRect::from_ltrb(1920.0, 0.0, 4480.0, 1440.0);
        assert_eq!(normalize_point_top_left(1920.0, 0.0, m), Some((0.0, 0.0)));
        assert_eq!(normalize_point_top_left(3200.0, 720.0, m), Some((0.5, 0.5)));
        assert_eq!(normalize_point_top_left(1919.0, 10.0, m), None);
        // Right/bottom edges are exclusive.
        assert_eq!(normalize_point_top_left(4480.0, 10.0, m), None);
        assert_eq!(normalize_point_top_left(10.0, 1440.0, m), None);
        // Monitors left of / above the primary have negative origins.
        let left = ScreenRect::from_ltrb(-1280.0, -200.0, 0.0, 824.0);
        assert_eq!(
            normalize_point_top_left(-640.0, 312.0, left),
            Some((0.5, 0.5))
        );
    }

    #[test]
    fn cocoa_bottom_left_is_flipped_to_top_down() {
        let frame = ScreenRect::new(0.0, 0.0, 1000.0, 500.0);
        // Cocoa y=0 is the bottom edge -> normalized y = 1.0 (bottom).
        assert_eq!(normalize_cocoa_point(0.0, 0.0, frame), Some((0.0, 1.0)));
        assert_eq!(
            normalize_cocoa_point(250.0, 375.0, frame),
            Some((0.25, 0.25))
        );
        assert_eq!(normalize_cocoa_point(1000.0, 10.0, frame), None);

        // Secondary display stacked above the primary (Cocoa y grows upward).
        let above = ScreenRect::new(0.0, 500.0, 800.0, 400.0);
        assert_eq!(
            normalize_cocoa_point(400.0, 800.0, above),
            Some((0.5, 0.25))
        );
        assert_eq!(normalize_cocoa_point(400.0, 499.0, above), None);
    }

    #[test]
    fn cocoa_to_quartz_conversion() {
        assert_eq!(cocoa_point_to_quartz(100.0, 0.0, 1080.0), (100.0, 1080.0));
        assert_eq!(cocoa_point_to_quartz(100.0, 1080.0, 1080.0), (100.0, 0.0));

        // Primary 1920x1080; secondary 1280x1024 above it in Cocoa space.
        let secondary = ScreenRect::new(0.0, 1080.0, 1280.0, 1024.0);
        assert_eq!(
            cocoa_rect_to_quartz(secondary, 1080.0),
            ScreenRect::new(0.0, -1024.0, 1280.0, 1024.0)
        );
        let primary = ScreenRect::new(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(cocoa_rect_to_quartz(primary, 1080.0), primary);
    }

    #[test]
    fn covered_point_detection() {
        let rects = [
            ScreenRect::new(0.0, 0.0, 100.0, 100.0),
            ScreenRect::new(500.0, 500.0, 50.0, 50.0),
        ];
        assert!(is_point_covered_by_any(100.0, 100.0, &rects));
        assert!(is_point_covered_by_any(525.0, 525.0, &rects));
        assert!(!is_point_covered_by_any(300.0, 300.0, &rects));
        assert!(!is_point_covered_by_any(1.0, 1.0, &[]));
    }
}
