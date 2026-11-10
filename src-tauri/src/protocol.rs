//! `wpe://localhost/<path>` custom protocol (docs/03 §6.1): serves a wallpaper's 4-layer
//! `WeVirtualFileSystem` through `truongpaper_platform::handle_wpe_protocol_request`
//! (HTTP `206 Partial Content` for media `Range` requests, pitfall #16). Each wallpaper webview
//! gets the VFS of the wallpaper it shows (`WallpaperHost::vfs_for_webview`), so monitors with
//! different Web / Video wallpapers do not read each other's folders.

use crate::host::WallpaperHost;
use std::borrow::Cow;
use tauri::http::{header, Request, Response, StatusCode};
use truongpaper_platform::handle_wpe_protocol_request;

pub const WPE_SCHEME: &str = "wpe";

/// Answers one request from webview `webview_label`; `404` when no wallpaper VFS is mounted.
pub fn serve(
    host: Option<&WallpaperHost>,
    webview_label: &str,
    request: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    match host.and_then(|h| h.vfs_for_webview(webview_label)) {
        Some(vfs) => handle_wpe_protocol_request(vfs.as_ref(), request),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(Cow::Borrowed(&b"No active wallpaper"[..]))
            .unwrap_or_else(|_| Response::new(Cow::Borrowed(&b""[..]))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use truongpaper_we_formats::WeVirtualFileSystem;

    fn req(path: &str, range: Option<&str>) -> Request<Vec<u8>> {
        let mut b = Request::builder().uri(format!("wpe://localhost/{path}"));
        if let Some(r) = range {
            b = b.header(header::RANGE, r);
        }
        b.body(Vec::new()).unwrap()
    }

    #[test]
    fn serves_active_vfs_with_ranges() {
        assert_eq!(
            serve(None, "main", req("a.txt", None)).status(),
            StatusCode::NOT_FOUND
        );
        let host = WallpaperHost::new();
        assert_eq!(
            serve(Some(&host), "main", req("a.txt", None)).status(),
            StatusCode::NOT_FOUND
        );

        let dir = std::env::temp_dir().join(format!("truongpaper-wpe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("clip.mp4"), b"0123456789").unwrap();
        host.set_active_vfs(
            "workshop:1",
            Arc::new(WeVirtualFileSystem::mount(dir.clone(), dir.join("core"))),
        );

        let full = serve(Some(&host), "main", req("clip.mp4", None));
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.body().as_ref(), b"0123456789");

        let part = serve(Some(&host), "main", req("clip.mp4", Some("bytes=2-4")));
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.body().as_ref(), b"234");
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 2-4/10");

        let traversal = serve(Some(&host), "main", req("..%2F..%2Fetc%2Fpasswd", None));
        assert_eq!(traversal.status(), StatusCode::FORBIDDEN);

        // Built-in video host page is served from the VFS built-in layer.
        assert_eq!(
            serve(Some(&host), "main", req("__video_host__.html", None)).status(),
            StatusCode::OK
        );

        // A wallpaper webview reads its own wallpaper folder, not the last mounted one.
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("clip.mp4"), b"abc").unwrap();
        host.set_webview_vfs(
            "wallpaper-B",
            "workshop:2",
            Arc::new(WeVirtualFileSystem::mount(
                other.clone(),
                other.join("core"),
            )),
        );
        let b = serve(Some(&host), "wallpaper-B", req("clip.mp4", None));
        assert_eq!(b.body().as_ref(), b"abc");
        let a = serve(Some(&host), "wallpaper-A", req("clip.mp4", None));
        assert_eq!(a.body().as_ref(), b"0123456789");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
