//! `wpe://` custom protocol handler with HTTP `206 Partial Content` byte-range streaming.
//! Spec: `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` section 6.1 (pitfall #16).
//!
//! `WKWebView` and `WebView2` refuse to play or loop `<video>` / `<audio>` over a custom scheme
//! unless `Range` requests get `206`, `Accept-Ranges: bytes`, `Content-Range` and `Content-Length`.

use crate::parse_http_byte_range;
use http::{header, Request, Response, StatusCode};
use std::borrow::Cow;
use std::io;
use std::path::Path;
use truongpaper_we_formats::vfs::{VfsAsset, WallpaperVfs, WeVirtualFileSystem};

/// Anything that can resolve a sanitized, `/`-separated virtual path to bytes.
///
/// Implemented for the real 4-layer [`WeVirtualFileSystem`]; tests (and future in-memory hosts)
/// can implement it directly.
pub trait WpeAssetSource {
    fn open_asset(&self, virtual_path: &str) -> io::Result<VfsAsset<'_>>;
}

impl WpeAssetSource for WeVirtualFileSystem {
    fn open_asset(&self, virtual_path: &str) -> io::Result<VfsAsset<'_>> {
        WeVirtualFileSystem::open_asset(self, virtual_path)
    }
}

/// Largest body returned for one request. Longer ranges are answered with a shorter `206`, which
/// `WKWebView` and `WebView2` follow with the next `Range` request.
pub const MAX_RESPONSE_CHUNK: usize = 8 * 1024 * 1024;
/// Largest non-media file served whole with `200` (scripts, pages, images, wasm).
pub const MAX_FULL_RESPONSE: usize = 64 * 1024 * 1024;
/// Largest audio/video file served whole with `200` when the client sent no `Range` header
/// (`fetch` + `decodeAudioData`). Tauri's protocol API needs the whole body in memory, so this
/// stays bounded; larger media falls back to a first-chunk `206`.
pub const MAX_FULL_MEDIA_RESPONSE: usize = 256 * 1024 * 1024;

/// Handles one `wpe://localhost/<path>` request against `vfs`.
///
/// - `200 OK` with the full body when there is no (or a syntactically invalid) `Range` header.
///   Audio and video up to [`MAX_FULL_MEDIA_RESPONSE`] are served whole (a `fetch` cannot handle a
///   partial `206`); larger media gets a `206` for the first chunk, and other files larger than
///   [`MAX_FULL_RESPONSE`] get `413`. Chunking only happens for requests that sent `Range`.
/// - `206 Partial Content` for a satisfiable `Range: bytes=...` header, at most
///   [`MAX_RESPONSE_CHUNK`] bytes per response.
/// - `416 Range Not Satisfiable` (`Content-Range: bytes */total`) for a well-formed but
///   unsatisfiable range.
/// - `403 Forbidden` for path traversal (`..`, absolute or drive-prefixed paths), `404` when missing.
///
/// `Access-Control-Allow-Origin` is only sent back for the app's own origins (see
/// [`allowed_origin`]).
pub fn handle_wpe_protocol_request<V: WpeAssetSource + ?Sized>(
    vfs: &V,
    request: Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(allowed_origin);
    let mut response = serve(vfs, &request);
    if let Some(origin) = origin {
        let headers = response.headers_mut();
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::HeaderValue::from_static(origin),
        );
        headers.insert(header::VARY, header::HeaderValue::from_static("Origin"));
    }
    response
}

/// The `Origin` values of our own wallpaper pages: `wpe://localhost` (`WKWebView`) and
/// `http(s)://wpe.localhost` (`WebView2`). Returns the static string to echo, or `None`.
pub fn allowed_origin(origin: &str) -> Option<&'static str> {
    match origin {
        "wpe://localhost" => Some("wpe://localhost"),
        "http://wpe.localhost" => Some("http://wpe.localhost"),
        "https://wpe.localhost" => Some("https://wpe.localhost"),
        _ => None,
    }
}

fn serve<V: WpeAssetSource + ?Sized>(
    vfs: &V,
    request: &Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    let uri_path = percent_encoding::percent_decode_str(request.uri().path())
        .decode_utf8_lossy()
        .into_owned();

    // Sanitize after percent-decoding so `%2e%2e/` cannot bypass traversal protection, regardless
    // of whether the backing source sanitizes on its own.
    let rel_path = match WallpaperVfs::sanitize_virtual_path(&uri_path) {
        Ok(p) => p,
        Err(_) => return error_response(StatusCode::FORBIDDEN, b"Forbidden"),
    };
    if rel_path.is_empty() {
        return error_response(StatusCode::NOT_FOUND, b"Not Found");
    }

    let asset = match vfs.open_asset(&rel_path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            return error_response(StatusCode::FORBIDDEN, b"Forbidden");
        }
        Err(_) => return error_response(StatusCode::NOT_FOUND, b"Not Found"),
    };

    let total_len = asset.size();
    let mime = guess_mime_type(Path::new(&rel_path));

    // HTTP Range header ("bytes=start-end") required by WKWebView & WebView2 media players.
    if let Some(range_val) = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
    {
        if let Some((start, end)) = parse_http_byte_range(range_val, total_len) {
            let end = end.min(start.saturating_add(MAX_RESPONSE_CHUNK - 1));
            let chunk_len = end - start + 1;
            let Ok(slice) = asset.read(start..end + 1) else {
                return error_response(StatusCode::NOT_FOUND, b"Not Found");
            };
            return Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, mime)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, chunk_len.to_string())
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total_len}"),
                )
                .body(Cow::Owned(slice))
                .expect("static 206 response headers are valid");
        }
        if is_well_formed_byte_range(range_val) {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_RANGE, format!("bytes */{total_len}"))
                .header(header::CONTENT_LENGTH, "0")
                .body(Cow::Borrowed(&b""[..]))
                .expect("static 416 response headers are valid");
        }
        // Malformed Range header: RFC 9110 says ignore it and serve the full representation.
    }

    let is_media = mime.starts_with("video/") || mime.starts_with("audio/");
    if is_media && total_len > MAX_FULL_MEDIA_RESPONSE {
        // Too large to hold in memory: answer with the first chunk and let the player
        // continue with `Range` requests.
        let end = MAX_RESPONSE_CHUNK - 1;
        let Ok(slice) = asset.read(0..end + 1) else {
            return error_response(StatusCode::NOT_FOUND, b"Not Found");
        };
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, mime)
            .header(header::ACCEPT_RANGES, "bytes")
            .header(header::CONTENT_LENGTH, slice.len().to_string())
            .header(header::CONTENT_RANGE, format!("bytes 0-{end}/{total_len}"))
            .body(Cow::Owned(slice))
            .expect("static 206 response headers are valid");
    }
    if !is_media && total_len > MAX_FULL_RESPONSE {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, b"Payload Too Large");
    }
    let Ok(bytes) = asset.read(0..total_len) else {
        return error_response(StatusCode::NOT_FOUND, b"Not Found");
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, total_len.to_string())
        .body(Cow::Owned(bytes))
        .expect("static 200 response headers are valid")
}

fn error_response(status: StatusCode, body: &'static [u8]) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Cow::Borrowed(body))
        .expect("static error response headers are valid")
}

/// `true` when the first range spec is syntactically `bytes=<digits?>-<digits?>` (not both empty).
/// Used to distinguish "unsatisfiable" (`416`) from "malformed" (ignored, `200`).
fn is_well_formed_byte_range(header_val: &str) -> bool {
    let Some(spec) = header_val.trim().strip_prefix("bytes=") else {
        return false;
    };
    let Some(first) = spec.split(',').next() else {
        return false;
    };
    let Some((start, end)) = first.trim().split_once('-') else {
        return false;
    };
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let (start, end) = (start.trim(), end.trim());
    if (start.is_empty() && end.is_empty()) || !digits(start) || !digits(end) {
        return false;
    }
    // RFC 9110 14.1.1: `last-pos < first-pos` makes the range invalid (ignored), not unsatisfiable.
    match (start.parse::<u128>(), end.parse::<u128>()) {
        (Ok(s), Ok(e)) => s <= e,
        _ => true,
    }
}

/// Maps a file extension to the `Content-Type` served over `wpe://`.
pub fn guess_mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("avi") => "video/x-msvideo",
        Some("ogv") => "video/ogg",
        Some("ogg" | "oga") => "audio/ogg",
        Some("opus") => "audio/opus",
        Some("mp3") => "audio/mpeg",
        Some("wav" | "wave") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("weba") => "audio/webm",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("ico") => "image/x-icon",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;

    struct MemVfs(HashMap<String, Vec<u8>>);

    impl MemVfs {
        fn with(files: &[(&str, &[u8])]) -> Self {
            Self(
                files
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_vec()))
                    .collect(),
            )
        }
    }

    impl WpeAssetSource for MemVfs {
        fn open_asset(&self, p: &str) -> io::Result<VfsAsset<'_>> {
            self.0
                .get(p)
                .map(|bytes| VfsAsset::Bytes(bytes))
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, p.to_string()))
        }
    }

    fn req(path: &str, range: Option<&str>) -> Request<Vec<u8>> {
        let mut b = Request::builder().uri(format!("wpe://localhost{path}"));
        if let Some(r) = range {
            b = b.header(header::RANGE, r);
        }
        b.body(Vec::new()).unwrap()
    }

    fn hdr<'a>(res: &'a Response<Cow<'static, [u8]>>, name: header::HeaderName) -> &'a str {
        res.headers().get(name).unwrap().to_str().unwrap()
    }

    fn sample() -> MemVfs {
        MemVfs::with(&[
            ("video.mp4", b"0123456789"),
            ("my clip.webm", b"abc"),
            ("empty.bin", b""),
        ])
    }

    #[test]
    fn full_body_returns_200_with_headers() {
        let res = handle_wpe_protocol_request(&sample(), req("/video.mp4", None));
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.body().as_ref(), b"0123456789");
        assert_eq!(hdr(&res, header::CONTENT_TYPE), "video/mp4");
        assert_eq!(hdr(&res, header::ACCEPT_RANGES), "bytes");
        assert_eq!(hdr(&res, header::CONTENT_LENGTH), "10");
        assert!(res
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
        assert!(res.headers().get(header::CONTENT_RANGE).is_none());
    }

    #[test]
    fn range_request_returns_206_partial_content() {
        let res = handle_wpe_protocol_request(&sample(), req("/video.mp4", Some("bytes=2-5")));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().as_ref(), b"2345");
        assert_eq!(hdr(&res, header::CONTENT_RANGE), "bytes 2-5/10");
        assert_eq!(hdr(&res, header::CONTENT_LENGTH), "4");
        assert_eq!(hdr(&res, header::ACCEPT_RANGES), "bytes");
        assert_eq!(hdr(&res, header::CONTENT_TYPE), "video/mp4");
    }

    #[test]
    fn open_ended_and_suffix_ranges_return_206() {
        let open = handle_wpe_protocol_request(&sample(), req("/video.mp4", Some("bytes=0-")));
        assert_eq!(open.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(hdr(&open, header::CONTENT_RANGE), "bytes 0-9/10");
        assert_eq!(open.body().len(), 10);

        let suffix = handle_wpe_protocol_request(&sample(), req("/video.mp4", Some("bytes=-3")));
        assert_eq!(suffix.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(suffix.body().as_ref(), b"789");
        assert_eq!(hdr(&suffix, header::CONTENT_RANGE), "bytes 7-9/10");

        let clamped =
            handle_wpe_protocol_request(&sample(), req("/video.mp4", Some("bytes=8-999")));
        assert_eq!(hdr(&clamped, header::CONTENT_RANGE), "bytes 8-9/10");
    }

    #[test]
    fn unsatisfiable_range_returns_416() {
        for r in ["bytes=10-", "bytes=50-60", "bytes=-0"] {
            let res = handle_wpe_protocol_request(&sample(), req("/video.mp4", Some(r)));
            assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE, "{r}");
            assert_eq!(hdr(&res, header::CONTENT_RANGE), "bytes */10");
            assert!(res.body().is_empty());
        }
        let empty = handle_wpe_protocol_request(&sample(), req("/empty.bin", Some("bytes=0-")));
        assert_eq!(empty.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(hdr(&empty, header::CONTENT_RANGE), "bytes */0");
    }

    #[test]
    fn malformed_range_is_ignored_and_returns_200() {
        for r in ["items=0-1", "bytes=abc", "bytes=-", "bytes=5-2"] {
            let res = handle_wpe_protocol_request(&sample(), req("/video.mp4", Some(r)));
            assert_eq!(res.status(), StatusCode::OK, "{r}");
            assert_eq!(res.body().len(), 10, "{r}");
        }
    }

    /// One synthetic file served for every path.
    struct BigVfs(Vec<u8>);

    impl WpeAssetSource for BigVfs {
        fn open_asset(&self, _: &str) -> io::Result<VfsAsset<'_>> {
            Ok(VfsAsset::Bytes(&self.0))
        }
    }

    fn big(len: usize) -> BigVfs {
        BigVfs((0..len).map(|i| (i % 251) as u8).collect())
    }

    #[test]
    fn open_ended_range_is_capped_to_one_chunk() {
        let vfs = big(MAX_RESPONSE_CHUNK * 2 + 10);
        let total = vfs.0.len();
        let res = handle_wpe_protocol_request(&vfs, req("/v.mp4", Some("bytes=0-")));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().len(), MAX_RESPONSE_CHUNK);
        assert_eq!(
            hdr(&res, header::CONTENT_RANGE),
            format!("bytes 0-{}/{total}", MAX_RESPONSE_CHUNK - 1)
        );
        assert_eq!(
            hdr(&res, header::CONTENT_LENGTH),
            MAX_RESPONSE_CHUNK.to_string()
        );

        // The player continues from the end of the previous chunk; the tail is shorter.
        let tail_start = MAX_RESPONSE_CHUNK * 2;
        let range = format!("bytes={tail_start}-");
        let res = handle_wpe_protocol_request(&vfs, req("/v.mp4", Some(&range)));
        assert_eq!(res.body().as_ref(), &vfs.0[tail_start..]);
        assert_eq!(
            hdr(&res, header::CONTENT_RANGE),
            format!("bytes {tail_start}-{}/{total}", total - 1)
        );
    }

    #[test]
    fn explicit_large_range_is_truncated_and_suffix_range_is_exact() {
        let vfs = big(MAX_RESPONSE_CHUNK * 3);
        let res = handle_wpe_protocol_request(&vfs, req("/v.mp4", Some("bytes=5-99999999")));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().len(), MAX_RESPONSE_CHUNK);
        assert_eq!(res.body().as_ref(), &vfs.0[5..5 + MAX_RESPONSE_CHUNK]);
        // The mp4 `moov` atom is often probed from the end of the file.
        let res = handle_wpe_protocol_request(&vfs, req("/v.mp4", Some("bytes=-100")));
        assert_eq!(res.body().as_ref(), &vfs.0[vfs.0.len() - 100..]);
    }

    #[test]
    fn media_without_range_is_served_whole_with_200() {
        let vfs = big(MAX_RESPONSE_CHUNK + 1);
        for p in ["/v.webm", "/a.mp3", "/a.flac"] {
            let res = handle_wpe_protocol_request(&vfs, req(p, None));
            assert_eq!(res.status(), StatusCode::OK, "{p}");
            assert_eq!(res.body().len(), MAX_RESPONSE_CHUNK + 1);
            assert!(res.headers().get(header::CONTENT_RANGE).is_none());
        }
        // Media above the whole-body limit falls back to a first-chunk 206.
        let vfs = big(MAX_FULL_MEDIA_RESPONSE + 1);
        let res = handle_wpe_protocol_request(&vfs, req("/v.webm", None));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().len(), MAX_RESPONSE_CHUNK);
    }

    #[test]
    fn huge_non_media_file_gets_413_but_ranges_still_work() {
        let vfs = big(MAX_FULL_RESPONSE + 1);
        let res = handle_wpe_protocol_request(&vfs, req("/a.bin", None));
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let res = handle_wpe_protocol_request(&vfs, req("/a.bin", Some("bytes=0-3")));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().len(), 4);
    }

    #[test]
    fn cors_header_is_only_sent_for_our_own_origins() {
        let with_origin = |origin: &str| {
            let r = Request::builder()
                .uri("wpe://localhost/video.mp4")
                .header(header::ORIGIN, origin)
                .body(Vec::new())
                .unwrap();
            handle_wpe_protocol_request(&sample(), r)
        };
        for o in [
            "wpe://localhost",
            "http://wpe.localhost",
            "https://wpe.localhost",
        ] {
            let res = with_origin(o);
            assert_eq!(hdr(&res, header::ACCESS_CONTROL_ALLOW_ORIGIN), o);
            assert_eq!(hdr(&res, header::VARY), "Origin");
        }
        for o in [
            "https://evil.example",
            "null",
            "http://wpe.localhost.evil.example",
        ] {
            let res = with_origin(o);
            assert!(
                res.headers()
                    .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .is_none(),
                "{o}"
            );
        }
        let missing = handle_wpe_protocol_request(&sample(), req("/nope", None));
        assert!(missing
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
    }

    #[test]
    fn missing_file_returns_404() {
        let res = handle_wpe_protocol_request(&sample(), req("/nope.mp4", None));
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let root = handle_wpe_protocol_request(&sample(), req("/", None));
        assert_eq!(root.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn path_traversal_is_rejected_with_403() {
        for p in [
            "/../secret.txt",
            "/a/../../etc/passwd",
            "/%2e%2e/secret.txt",
            "/a%2F..%2F..%2Fx",
        ] {
            let res = handle_wpe_protocol_request(&sample(), req(p, None));
            assert_eq!(res.status(), StatusCode::FORBIDDEN, "{p}");
        }
    }

    #[test]
    fn percent_encoded_paths_are_decoded() {
        let res = handle_wpe_protocol_request(&sample(), req("/my%20clip.webm", None));
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.body().as_ref(), b"abc");
        assert_eq!(hdr(&res, header::CONTENT_TYPE), "video/webm");
    }

    #[test]
    fn query_string_is_not_part_of_the_path() {
        let vfs = MemVfs::with(&[("__video_host__.html", b"<html>")]);
        let res = handle_wpe_protocol_request(
            &vfs,
            req("/__video_host__.html?file=video%20one.mp4", None),
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(hdr(&res, header::CONTENT_TYPE), "text/html; charset=utf-8");
    }

    #[test]
    fn mime_type_mapping() {
        let cases = [
            ("index.HTML", "text/html; charset=utf-8"),
            ("a.htm", "text/html; charset=utf-8"),
            ("app.mjs", "text/javascript; charset=utf-8"),
            ("s.css", "text/css; charset=utf-8"),
            ("p.json", "application/json; charset=utf-8"),
            ("v.M4V", "video/mp4"),
            ("v.mov", "video/quicktime"),
            ("v.ogv", "video/ogg"),
            ("a.ogg", "audio/ogg"),
            ("a.flac", "audio/flac"),
            ("a.m4a", "audio/mp4"),
            ("a.opus", "audio/opus"),
            ("a.aac", "audio/aac"),
            ("a.weba", "audio/webm"),
            ("f.otf", "font/otf"),
            ("a.mp3", "audio/mpeg"),
            ("a.wav", "audio/wav"),
            ("i.jpeg", "image/jpeg"),
            ("i.webp", "image/webp"),
            ("i.svg", "image/svg+xml"),
            ("m.wasm", "application/wasm"),
            ("f.woff2", "font/woff2"),
            ("f.ttf", "font/ttf"),
            ("scene.pkg", "application/octet-stream"),
            ("noext", "application/octet-stream"),
        ];
        for (p, m) in cases {
            assert_eq!(guess_mime_type(Path::new(p)), m, "{p}");
        }
    }

    #[test]
    fn serves_from_real_vfs_in_tempdir() {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "truongpaper-wpe-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("media")).unwrap();
        fs::write(dir.join("media").join("clip.mp4"), b"ftypmp42data").unwrap();

        let vfs = WeVirtualFileSystem::mount(dir.clone(), dir.join("core"));
        let res = handle_wpe_protocol_request(&vfs, req("/media/clip.mp4", Some("bytes=0-3")));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().as_ref(), b"ftyp");
        assert_eq!(hdr(&res, header::CONTENT_RANGE), "bytes 0-3/12");

        // Built-in virtual layer serves the video host document.
        let host = handle_wpe_protocol_request(&vfs, req("/__video_host__.html?file=x.mp4", None));
        assert_eq!(host.status(), StatusCode::OK);
        assert_eq!(
            host.body().as_ref(),
            crate::VIDEO_WALLPAPER_HOST_HTML.as_bytes()
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
