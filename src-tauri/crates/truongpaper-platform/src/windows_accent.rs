//! Windows half of "Adjust Windows color": writes a wallpaper's scheme color as the user's
//! accent color and tells Explorer and running apps to pick it up.
//!
//! The accent lives in the per-user registry values Settings > Personalization > Colors writes
//! (`Explorer\Accent` and `DWM`); `WM_SETTINGCHANGE("ImmersiveColorSet")` makes Start, the
//! taskbar and UWP/WinUI apps reload it.
//! "Pick an accent color from my background automatically" is switched off so Windows does not
//! replace it with one taken from its own (hidden) wallpaper.

use crate::accent::{abgr, accent_palette, accent_palette_bytes, argb};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, WPARAM};
use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_BINARY, REG_DWORD};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

const ACCENT_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent");
const DWM_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\DWM");
const DESKTOP_KEY: PCWSTR = w!(r"Control Panel\Desktop");

fn set_dword(key: PCWSTR, name: PCWSTR, value: u32) -> Result<(), String> {
    let bytes = value.to_le_bytes();
    // SAFETY: `bytes` outlives the call and its length is passed.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            REG_DWORD.0,
            Some(bytes.as_ptr().cast()),
            bytes.len() as u32,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry write failed ({})", status.0))
    }
}

fn set_binary(key: PCWSTR, name: PCWSTR, value: &[u8]) -> Result<(), String> {
    // SAFETY: `value` outlives the call and its length is passed.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            REG_BINARY.0,
            Some(value.as_ptr().cast()),
            value.len() as u32,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry write failed ({})", status.0))
    }
}

/// Makes `rgb` the Windows accent color. Blocks for up to about a second while windows
/// handle the broadcast; call it off the UI thread.
pub fn set_accent_color(rgb: [u8; 3]) -> Result<(), String> {
    let palette = accent_palette(rgb);
    set_binary(ACCENT_KEY, w!("AccentPalette"), &accent_palette_bytes(rgb))?;
    set_dword(ACCENT_KEY, w!("AccentColorMenu"), abgr(rgb))?;
    set_dword(ACCENT_KEY, w!("StartColorMenu"), abgr(palette[5]))?;
    set_dword(DWM_KEY, w!("AccentColor"), abgr(rgb))?;
    set_dword(DWM_KEY, w!("ColorizationColor"), argb(rgb, 0xC4))?;
    set_dword(DWM_KEY, w!("ColorizationAfterglow"), argb(rgb, 0xC4))?;
    if let Err(err) = set_dword(DESKTOP_KEY, w!("AutoColorization"), 0) {
        tracing::debug!("AutoColorization: {err}");
    }
    let topic = w!("ImmersiveColorSet");
    // SAFETY: broadcast with a static, NUL-terminated string; hung windows are skipped.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(topic.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            500,
            None,
        );
    }
    Ok(())
}
