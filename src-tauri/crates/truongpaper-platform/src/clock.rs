//! Whether the user's clock shows 24-hour time (Windows region format, macOS "24-hour time").
//!
//! Callers (the render thread) never wait for the system: the answer is cached and refreshed
//! every few minutes on a background thread (reading it on macOS starts a `defaults` process).

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CACHE_FOR: Duration = Duration::from_secs(300);

static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// `true` when the system clock is set to 24-hour time. Returns the cached answer at once
/// (`false` until the first read finishes) and starts a background refresh when it is stale.
pub fn uses_24_hour_clock() -> bool {
    let cached = *CACHE.lock();
    let stale = cached.is_none_or(|(at, _)| at.elapsed() >= CACHE_FOR);
    if stale && !REFRESHING.swap(true, Ordering::AcqRel) {
        let spawned = std::thread::Builder::new()
            .name("clock-format".into())
            .spawn(|| {
                let value = read_system_setting();
                *CACHE.lock() = Some((Instant::now(), value));
                REFRESHING.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            REFRESHING.store(false, Ordering::Release);
        }
    }
    cached.is_some_and(|(_, value)| value)
}

/// A Windows short-time pattern uses 24-hour time when its hours are `H` / `HH`.
pub fn pattern_is_24_hour(pattern: &str) -> bool {
    pattern.contains('H')
}

/// Regions whose default clock is 12-hour (used on macOS when the user has not forced either).
pub fn region_defaults_to_12_hour(region: &str) -> bool {
    matches!(
        region.to_ascii_uppercase().as_str(),
        "US" | "CA" | "AU" | "NZ" | "IN" | "PH" | "PK" | "BD" | "EG" | "SA" | "MY" | "CO" | "MX"
    )
}

#[cfg(target_os = "windows")]
fn read_system_setting() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let mut buf = [0u16; 80];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: `buf` and `len` outlive the call; `len` is the buffer size in bytes.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Control Panel\International"),
            w!("sShortTime"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }
    let chars = (len as usize / 2).min(buf.len());
    let pattern = String::from_utf16_lossy(&buf[..chars]);
    pattern_is_24_hour(pattern.trim_end_matches('\0'))
}

#[cfg(target_os = "macos")]
fn read_system_setting() -> bool {
    let read = |key: &str| -> Option<String> {
        let out = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "-g", key])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    if read("AppleICUForce24HourTime").as_deref() == Some("1") {
        return true;
    }
    if read("AppleICUForce12HourTime").as_deref() == Some("1") {
        return false;
    }
    let locale = read("AppleLocale").unwrap_or_default();
    let region = locale
        .split(['_', '-', '@'])
        .nth(1)
        .unwrap_or("US")
        .to_string();
    !region_defaults_to_12_hour(&region)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn read_system_setting() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_windows_patterns() {
        assert!(pattern_is_24_hour("HH:mm"));
        assert!(pattern_is_24_hour("H:mm"));
        assert!(!pattern_is_24_hour("h:mm tt"));
    }

    #[test]
    fn knows_12_hour_regions() {
        assert!(region_defaults_to_12_hour("US"));
        assert!(!region_defaults_to_12_hour("DE"));
        assert!(!region_defaults_to_12_hour("gb"));
    }

    #[test]
    fn the_system_setting_can_be_read() {
        // Either answer is fine; the first call returns at once and the background read
        // fills the cache soon after.
        let _ = uses_24_hour_clock();
        let deadline = Instant::now() + Duration::from_secs(10);
        while CACHE.lock().is_none() {
            assert!(Instant::now() < deadline, "clock format was never read");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
