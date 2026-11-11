//! `truongpaper-platform`: OS desktop wallpaper window host (Windows `Progman`/`WorkerW` including
//! Windows 11 24H2 `WS_EX_NOREDIRECTIONBITMAP` + macOS `NSWindow` at `kCGDesktopWindowLevel`),
//! passive cursor hit-testing, `wpe://` custom protocol handler (with HTTP `206 Partial Content`),
//! and fullscreen/maximized/battery policy monitor.
//! Complete specification: `docs/01-SYSTEM-ARCHITECTURE.md` (Sections 2, 5, 6) &
//! `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` (Sections 6, 9, 10).

pub mod application;
pub mod cursor;
pub mod geometry;
pub mod policy;
pub mod wpe_protocol;

#[cfg(target_os = "windows")]
#[path = "windows_desktop.rs"]
pub mod win_desktop;

#[cfg(target_os = "macos")]
#[path = "macos_desktop.rs"]
pub mod mac_desktop;

#[cfg(target_os = "macos")]
#[path = "macos_video.rs"]
pub mod mac_video;

pub use application::{
    application_launch_args, application_wallpapers_supported,
    is_wallpaper_type_supported_on_this_os,
};
pub use geometry::ScreenRect;
pub use policy::{
    apply_system_policy, decide_occlusion_by_coverage, decide_occlusion_win32,
    is_on_battery_or_low_power, BatteryPolicy, MonitorPlaybackPolicy, OcclusionPrefs,
    PolicyDecision, PolicyMonitor, SystemPowerState, POLICY_POLL_INTERVAL,
};
pub use wpe_protocol::{guess_mime_type, handle_wpe_protocol_request, WpeAssetSource};

/// `wpe://localhost/__video_host__.html?file=<encoded_project_file>` (served by the VFS built-in layer).
pub const VIDEO_WALLPAPER_HOST_HTML: &str = include_str!("video-wallpaper-host.html");
/// Inject via `wry::WebViewBuilder::with_initialization_script` for `Web` wallpapers.
pub const WEB_WALLPAPER_PRELOAD_JS: &str = include_str!("web-wallpaper-preload.js");

/// WebView2 `AdditionalBrowserArguments` that keep Web/Video wallpapers playing behind the
/// desktop icons without a user gesture (spec section 6.3).
pub const WEBVIEW2_WALLPAPER_BROWSER_ARGS: &str = "--autoplay-policy=no-user-gesture-required \
--disable-background-timer-throttling \
--disable-backgrounding-occluded-windows \
--disable-background-media-suspend \
--disable-renderer-backgrounding \
--disable-features=WebContentsOcclusion,IsolateOrigins,site-per-process";

/// `WKAudiovisualMediaTypeNone`: value for `WKWebViewConfiguration.mediaTypesRequiringUserActionForPlayback`
/// (spec section 6.3). Also set `preferences.javaScriptCanOpenWindowsAutomatically = false`.
pub const WK_AUDIOVISUAL_MEDIA_TYPE_NONE: usize = 0;

/// Parses an HTTP `Range: bytes=start-end` header into inclusive `(start, end)` byte offsets.
pub fn parse_http_byte_range(range_header: &str, total_len: usize) -> Option<(usize, usize)> {
    let spec = range_header.strip_prefix("bytes=")?.trim();
    let range_part = spec.split(',').next()?.trim();
    let (start_str, end_str) = range_part.split_once('-')?;

    if start_str.is_empty() {
        let suffix_len: usize = end_str.parse().ok()?;
        if suffix_len == 0 || total_len == 0 {
            return None;
        }
        let start = total_len.saturating_sub(suffix_len);
        Some((start, total_len - 1))
    } else {
        let start: usize = start_str.parse().ok()?;
        if start >= total_len {
            return None;
        }
        let end = if end_str.is_empty() {
            total_len - 1
        } else {
            end_str.parse::<usize>().ok()?.min(total_len - 1)
        };
        if start <= end {
            Some((start, end))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_206_byte_ranges() {
        assert_eq!(parse_http_byte_range("bytes=0-1023", 4096), Some((0, 1023)));
        assert_eq!(
            parse_http_byte_range("bytes=1024-", 4096),
            Some((1024, 4095))
        );
        assert_eq!(
            parse_http_byte_range("bytes=-500", 4096),
            Some((3596, 4095))
        );
    }

    #[test]
    fn byte_range_edge_cases() {
        // End clamped to the last byte; single-byte ranges.
        assert_eq!(parse_http_byte_range("bytes=0-99999", 10), Some((0, 9)));
        assert_eq!(parse_http_byte_range("bytes=9-9", 10), Some((9, 9)));
        assert_eq!(parse_http_byte_range("bytes=0-0", 1), Some((0, 0)));
        // Suffix longer than the body returns the whole body.
        assert_eq!(parse_http_byte_range("bytes=-500", 10), Some((0, 9)));
        // Only the first range of a multi-range request is honored.
        assert_eq!(parse_http_byte_range("bytes=0-1, 4-5", 10), Some((0, 1)));
        assert_eq!(parse_http_byte_range("bytes= 2-3", 10), Some((2, 3)));
        // Unsatisfiable / invalid.
        assert_eq!(parse_http_byte_range("bytes=10-", 10), None);
        assert_eq!(parse_http_byte_range("bytes=5-2", 10), None);
        assert_eq!(parse_http_byte_range("bytes=-0", 10), None);
        assert_eq!(parse_http_byte_range("bytes=0-", 0), None);
        assert_eq!(parse_http_byte_range("bytes=-5", 0), None);
        assert_eq!(parse_http_byte_range("bytes=abc-", 10), None);
        assert_eq!(parse_http_byte_range("bytes=-", 10), None);
        assert_eq!(parse_http_byte_range("items=0-1", 10), None);
        assert_eq!(parse_http_byte_range("bytes=5", 10), None);
    }

    #[test]
    fn embedded_assets_match_spec_entry_points() {
        assert!(VIDEO_WALLPAPER_HOST_HTML.contains("__truongpaper_apply_video_adjustments"));
        assert!(VIDEO_WALLPAPER_HOST_HTML.contains("__truongpaper_set_paused"));
        assert!(VIDEO_WALLPAPER_HOST_HTML.contains("id=\"fg-video\""));
        // The video URL is resolved against the page URL so it works on both WKWebView
        // (`wpe://localhost/`) and WebView2 (`http://wpe.localhost/`).
        assert!(!VIDEO_WALLPAPER_HOST_HTML.contains("'wpe://localhost/'"));
        assert!(VIDEO_WALLPAPER_HOST_HTML.contains("window.location.href).href"));
        assert!(WEB_WALLPAPER_PRELOAD_JS.contains("wallpaperRegisterAudioListener"));
        assert!(WEB_WALLPAPER_PRELOAD_JS.contains("__truongpaper_dispatch_audio"));
        assert!(WEB_WALLPAPER_PRELOAD_JS.contains("__truongpaper_dispatch_properties"));
        assert!(WEB_WALLPAPER_PRELOAD_JS.contains("wallpaperRequestRandomFileForProperty"));
    }
}

/// Native resident-memory readings for the current app process.
pub mod memory;
