//! Stable display identities.
//!
//! Windows numbers GDI display devices (`\\.\DISPLAY1`, `\\.\DISPLAY2`, ...) in the order it
//! (re)creates them, so the same monitor becomes `\\.\DISPLAY5` after a few power cycles or
//! reboots. Using that name as a TruongPaper monitor id loses the display's wallpaper, volume and
//! playlist position each time, and showing it to the user reads as "display 5 of 6" on a
//! two-monitor desk.
//!
//! Instead, each active GDI source is mapped (through `QueryDisplayConfig`) to the monitor
//! connected to it: its device interface path, which encodes the EDID product code and the
//! connector, and its EDID friendly name ("DELL U2720Q"). The path without the `\\?\` prefix
//! and the trailing interface GUID becomes the monitor id.
//!
//! On other platforms the OS display name is already stable and is used as is.

/// The monitor behind one OS display device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayIdentity {
    /// OS display device name (`\\.\DISPLAY1` on Windows).
    pub gdi_name: String,
    /// Stable TruongPaper monitor id base (before duplicate suffixes).
    pub stable_id: String,
    /// EDID monitor name, when the monitor reports one.
    pub friendly_name: Option<String>,
}

/// `\\?\DISPLAY#DEL41B8#5&1d5ab3f3&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}` ->
/// `DISPLAY#DEL41B8#5&1d5ab3f3&0&UID4352`. `None` for an empty path.
pub fn stable_id_from_device_path(path: &str) -> Option<String> {
    let path = path.trim();
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let path = match path.rfind("#{") {
        Some(i) if path.ends_with('}') => &path[..i],
        _ => path,
    };
    let path = path.trim_matches('#').trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// Builds the user-facing names for a list of displays: the friendly name, `Display <n>`
/// (1-based position) when there is none, and ` (2)`, ` (3)` ... for repeated names.
pub fn display_names(friendly: &[Option<String>]) -> Vec<String> {
    let base: Vec<String> = friendly
        .iter()
        .enumerate()
        .map(|(i, name)| {
            name.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Display {}", i + 1))
        })
        .collect();
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    base.iter()
        .map(|name| {
            let n = seen.entry(name.as_str()).or_insert(0);
            *n += 1;
            if *n > 1 {
                format!("{name} ({n})")
            } else {
                name.clone()
            }
        })
        .collect()
}

/// Identity of every active display (empty off Windows, where OS names are already stable).
pub fn display_identities() -> Vec<DisplayIdentity> {
    #[cfg(target_os = "windows")]
    {
        windows_impl::cached()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// Stable monitor id for an OS display name (the name itself when no identity is known).
pub fn stable_monitor_id(os_name: &str) -> String {
    display_identities()
        .into_iter()
        .find(|d| d.gdi_name.eq_ignore_ascii_case(os_name))
        .map(|d| d.stable_id)
        .unwrap_or_else(|| os_name.to_string())
}

/// Drops the cached identities (call after a display topology change).
pub fn invalidate_display_identities() {
    #[cfg(target_os = "windows")]
    windows_impl::invalidate();
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{stable_id_from_device_path, DisplayIdentity};
    use parking_lot::Mutex;
    use std::time::{Duration, Instant};
    use windows::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    };
    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};

    /// Short safety net in case a topology notification is missed.
    const TTL: Duration = Duration::from_secs(2);
    static CACHE: Mutex<Option<(Instant, Vec<DisplayIdentity>)>> = Mutex::new(None);

    pub(super) fn cached() -> Vec<DisplayIdentity> {
        let mut cache = CACHE.lock();
        if let Some((at, list)) = cache.as_ref() {
            if at.elapsed() < TTL {
                return list.clone();
            }
        }
        let list = query();
        *cache = Some((Instant::now(), list.clone()));
        list
    }

    pub(super) fn invalidate() {
        *CACHE.lock() = None;
    }

    fn wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    fn query() -> Vec<DisplayIdentity> {
        let paths = match active_paths() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let mut out: Vec<DisplayIdentity> = Vec::new();
        for path in &paths {
            // SAFETY: plain-data request packets sized by their own struct.
            let gdi_name = unsafe {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: path.sourceInfo.adapterId,
                        id: path.sourceInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut source.header) != ERROR_SUCCESS.0 as i32 {
                    continue;
                }
                wide(&source.viewGdiDeviceName)
            };
            if gdi_name.is_empty() || out.iter().any(|d| d.gdi_name == gdi_name) {
                // A cloned (duplicated) display: the first target names the source.
                continue;
            }
            let (device_path, friendly) = unsafe {
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                        adapterId: path.targetInfo.adapterId,
                        id: path.targetInfo.id,
                    },
                    ..Default::default()
                };
                if DisplayConfigGetDeviceInfo(&mut target.header) == ERROR_SUCCESS.0 as i32 {
                    (
                        wide(&target.monitorDevicePath),
                        wide(&target.monitorFriendlyDeviceName),
                    )
                } else {
                    (String::new(), String::new())
                }
            };
            let stable_id =
                stable_id_from_device_path(&device_path).unwrap_or_else(|| gdi_name.clone());
            // Two identical panels on the same connector type can never share a path (the UID
            // differs), but keep ids unique if a driver ever reports one twice.
            let stable_id = if out.iter().any(|d| d.stable_id == stable_id) {
                gdi_name.clone()
            } else {
                stable_id
            };
            let friendly = friendly.trim().to_string();
            out.push(DisplayIdentity {
                gdi_name,
                stable_id,
                friendly_name: (!friendly.is_empty()).then_some(friendly),
            });
        }
        out
    }

    fn active_paths() -> Option<Vec<DISPLAYCONFIG_PATH_INFO>> {
        // The topology can change between the size query and the query itself; retry then.
        for _ in 0..3 {
            let (mut n_paths, mut n_modes) = (0u32, 0u32);
            // SAFETY: out-pointers to local counters.
            let err = unsafe {
                GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_paths, &mut n_modes)
            };
            if err != ERROR_SUCCESS {
                return None;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n_paths as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); n_modes as usize];
            // SAFETY: buffers sized by the counts passed alongside them.
            let err = unsafe {
                QueryDisplayConfig(
                    QDC_ONLY_ACTIVE_PATHS,
                    &mut n_paths,
                    paths.as_mut_ptr(),
                    &mut n_modes,
                    modes.as_mut_ptr(),
                    None,
                )
            };
            if err == ERROR_SUCCESS {
                paths.truncate(n_paths as usize);
                return Some(paths);
            }
            if err != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_path_becomes_a_short_stable_id() {
        assert_eq!(
            stable_id_from_device_path(
                r"\\?\DISPLAY#DEL41B8#5&1d5ab3f3&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some("DISPLAY#DEL41B8#5&1d5ab3f3&0&UID4352")
        );
        assert_eq!(
            stable_id_from_device_path(r"DISPLAY#GSM5B7F#4&2b0d4c&0&UID256").as_deref(),
            Some("DISPLAY#GSM5B7F#4&2b0d4c&0&UID256")
        );
        assert_eq!(stable_id_from_device_path(""), None);
        assert_eq!(stable_id_from_device_path(r"\\?\"), None);
    }

    #[test]
    fn names_fall_back_to_position_and_number_repeats() {
        assert_eq!(
            display_names(&[
                Some("DELL U2720Q".into()),
                None,
                Some("DELL U2720Q".into()),
                Some("  ".into()),
            ]),
            vec!["DELL U2720Q", "Display 2", "DELL U2720Q (2)", "Display 4"]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn every_connected_windows_display_has_an_identity() {
        invalidate_display_identities();
        let ids = display_identities();
        let gdi = unsafe { crate::win_desktop::enumerate_monitors_win32() };
        for m in &gdi {
            let found = ids.iter().find(|d| d.gdi_name == m.device_name);
            assert!(
                found.is_some(),
                "{} has no identity in {ids:?}",
                m.device_name
            );
        }
        let mut unique: Vec<_> = ids.iter().map(|d| &d.stable_id).collect();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
        eprintln!("display identities: {ids:#?}");
    }
}
