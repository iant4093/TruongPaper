// src-tauri/src/tray.rs
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Listener, Manager, PhysicalPosition, PhysicalSize, Position, Rect, WindowEvent,
};

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

    if let Some(popover) = app.get_webview_window("tray-popover") {
        let pop_clone = popover.clone();
        popover.on_window_event(move |event| {
            if let WindowEvent::Focused(false) = event {
                let _ = pop_clone.hide();
            }
        });
    }

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

fn toggle_tray_popover(app: &AppHandle, tray_rect: Rect) {
    let Some(popover) = app.get_webview_window("tray-popover") else {
        return;
    };

    if popover.is_visible().unwrap_or(false) {
        let _ = popover.hide();
        return;
    }

    let scale_factor = popover.scale_factor().unwrap_or(1.0);
    let win_size: PhysicalSize<u32> = popover.outer_size().unwrap_or_else(|_| {
        PhysicalSize::new((360.0 * scale_factor) as u32, (480.0 * scale_factor) as u32)
    });

    let (icon_x, icon_y) = match tray_rect.position {
        Position::Physical(p) => (p.x as f64, p.y as f64),
        Position::Logical(l) => (l.x * scale_factor, l.y * scale_factor),
    };
    let (icon_w, icon_h) = match tray_rect.size {
        tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
        tauri::Size::Logical(l) => (l.width * scale_factor, l.height * scale_factor),
    };

    let mut target_x = (icon_x + (icon_w / 2.0) - (win_size.width as f64 / 2.0)).round() as i32;

    let margin_px = (6.0 * scale_factor).round() as i32;
    let mut target_y = if cfg!(target_os = "macos") {
        (icon_y + icon_h).round() as i32 + margin_px
    } else {
        icon_y.round() as i32 - (win_size.height as i32) - margin_px
    };

    if let Ok(Some(monitor)) = popover.current_monitor() {
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let min_x = mon_pos.x + margin_px;
        let max_x = mon_pos.x + (mon_size.width as i32) - (win_size.width as i32) - margin_px;
        let min_y = mon_pos.y + margin_px;
        let max_y = mon_pos.y + (mon_size.height as i32) - (win_size.height as i32) - margin_px;

        target_x = target_x.clamp(min_x, max_x.max(min_x));
        target_y = target_y.clamp(min_y, max_y.max(min_y));
    }

    let _ = popover.set_position(Position::Physical(PhysicalPosition::new(
        target_x, target_y,
    )));
    let _ = popover.show();
    let _ = popover.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_item_says_all_displays() {
        assert_eq!(pause_label(false), "Pause All Displays");
        assert_eq!(pause_label(true), "Resume All Displays");
    }
}
