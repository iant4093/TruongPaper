//! Whether the user's clock shows 24-hour time (Windows region format, macOS "24-hour time").
//! The answer is cached for a few minutes, so a change in system settings is picked up soon.

use parking_lot::Mutex;
use std::time::{Duration, Instant};

const CACHE_FOR: Duration = Duration::from_secs(300);

/// `true` when the system clock is set to 24-hour time.
pub fn uses_24_hour_clock() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock();
    if let Some((at, value)) = *cache {
        if at.elapsed() < CACHE_FOR {
            return value;
        }
    }
    let value = read_system_setting();
    *cache = Some((Instant::now(), value));
    value
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
        // Either answer is fine; this only checks that reading does not fail or hang.
        let _ = uses_24_hour_clock();
    }
}
