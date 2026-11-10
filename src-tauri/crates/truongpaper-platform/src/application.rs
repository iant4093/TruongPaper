//! Windows `Application` wallpaper host.
//! Spec: `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` section 10.
//!
//! `"type": "application"` Workshop items are native Windows executables. On macOS the UI flags
//! them `windowsOnly: true` and shows a "Windows Only" badge (see
//! [`application_wallpapers_supported`]); on Windows they are launched with window-bounds
//! arguments and terminated when switching wallpapers or shutting down.

/// `true` when `"type": "application"` wallpapers can run on the current OS (Windows only).
pub const fn application_wallpapers_supported() -> bool {
    cfg!(target_os = "windows")
}

/// `true` when a Wallpaper Engine `project.json` `type` can be played on this OS.
/// Only `application` is OS-restricted; every other type is cross-platform.
pub fn is_wallpaper_type_supported_on_this_os(wallpaper_type: &str) -> bool {
    if wallpaper_type.eq_ignore_ascii_case("application") {
        application_wallpapers_supported()
    } else {
        true
    }
}

/// Command-line arguments passed to Unity-style application wallpapers.
pub fn application_launch_args(width: i32, height: i32) -> Vec<String> {
    vec![
        "-popupwindow".into(),
        "-screen-fullscreen".into(),
        "0".into(),
        "-screen-width".into(),
        width.to_string(),
        "-screen-height".into(),
        height.to_string(),
    ]
}

#[cfg(target_os = "windows")]
pub fn spawn_application_wallpaper(
    exe_path: &std::path::Path,
    width: i32,
    height: i32,
) -> anyhow::Result<std::process::Child> {
    let child = std::process::Command::new(exe_path)
        .current_dir(
            exe_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new(".")),
        )
        .args(application_launch_args(width, height))
        .spawn()?;
    Ok(child)
}

/// Owns a running application wallpaper process; kills it on [`stop`](Self::stop) or drop.
#[cfg(target_os = "windows")]
pub struct ApplicationWallpaperProcess {
    child: Option<std::process::Child>,
}

#[cfg(target_os = "windows")]
impl ApplicationWallpaperProcess {
    pub fn spawn(exe_path: &std::path::Path, width: i32, height: i32) -> anyhow::Result<Self> {
        Ok(Self {
            child: Some(spawn_application_wallpaper(exe_path, width, height)?),
        })
    }

    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }

    /// Terminates the process (if still running) and reaps it.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if let Ok(None) = child.try_wait() {
                if let Err(e) = child.kill() {
                    tracing::warn!("failed to kill application wallpaper: {e}");
                }
            }
            let _ = child.wait();
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for ApplicationWallpaperProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_support_flag_matches_os() {
        assert_eq!(
            application_wallpapers_supported(),
            cfg!(target_os = "windows")
        );
        assert_eq!(
            is_wallpaper_type_supported_on_this_os("application"),
            cfg!(target_os = "windows")
        );
        assert_eq!(
            is_wallpaper_type_supported_on_this_os("Application"),
            cfg!(target_os = "windows")
        );
        for t in ["scene", "video", "web", "preset"] {
            assert!(is_wallpaper_type_supported_on_this_os(t), "{t}");
        }
    }

    #[test]
    fn launch_args_carry_bounds() {
        assert_eq!(
            application_launch_args(2560, 1440),
            [
                "-popupwindow",
                "-screen-fullscreen",
                "0",
                "-screen-width",
                "2560",
                "-screen-height",
                "1440"
            ]
        );
    }
}
