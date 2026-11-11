//! Phase 5 gate: `cargo test -p truongpaper-renderer --test video_web_runtime_tests`.

#[allow(dead_code)]
mod common;

use common::TempDir;
use std::borrow::Cow;
use truongpaper_renderer::json::Value;
use truongpaper_renderer::video_runtime::MUTE_VOLUME_THRESHOLD;
use truongpaper_renderer::*;
use truongpaper_we_formats::{ParsedTex, TexFormat, TexHeader, TexMipmap, FIF_MP4, FIF_UNKNOWN};

const VIDEO_HOST_HTML: &str =
    include_str!("../../truongpaper-platform/src/video-wallpaper-host.html");
const WEB_PRELOAD_JS: &str =
    include_str!("../../truongpaper-platform/src/web-wallpaper-preload.js");

// ------------------------------------------------------------------------------------------
// Playback rules
// ------------------------------------------------------------------------------------------

#[test]
fn playback_speed_is_clamped_to_spec_range() {
    assert_eq!(MIN_PLAYBACK_SPEED, 0.1);
    assert_eq!(MAX_PLAYBACK_SPEED, 2.0);
    assert_eq!(clamp_playback_speed(0.05), 0.1);
    assert_eq!(clamp_playback_speed(0.1), 0.1);
    assert_eq!(clamp_playback_speed(1.25), 1.25);
    assert_eq!(clamp_playback_speed(3.0), 2.0);
    assert_eq!(clamp_playback_speed(5.0), 2.0);
    assert_eq!(clamp_playback_speed(-2.0), 0.1);
    assert_eq!(clamp_playback_speed(f32::NAN), 1.0);
    assert_eq!(clamp_playback_speed(f32::INFINITY), 1.0);
    assert_eq!(clamp_playback_speed(f32::NEG_INFINITY), 1.0);
}

#[test]
fn volume_and_mute_rules_match_video_host() {
    assert_eq!(clamp_volume(-0.5), 0.0);
    assert_eq!(clamp_volume(0.4), 0.4);
    assert_eq!(clamp_volume(1.7), 1.0);
    assert_eq!(clamp_volume(f32::NAN), 0.0);

    let d = VideoPlaybackSettings::default();
    assert_eq!((d.speed, d.volume, d.muted), (1.0, 1.0, false));
    assert_eq!(d.effective_volume(), 1.0);

    let muted = VideoPlaybackSettings { muted: true, ..d };
    assert_eq!(muted.effective_volume(), 0.0);
    assert!(muted.is_effectively_muted());

    // The host page treats volume <= 0.001 as muted.
    assert!(VIDEO_HOST_HTML.contains("<= 0.001"));
    for v in [0.0, MUTE_VOLUME_THRESHOLD, -1.0, f32::NAN] {
        let s = VideoPlaybackSettings { volume: v, ..d };
        assert_eq!(s.effective_volume(), 0.0, "volume {v}");
        assert!(s.is_effectively_muted());
    }
    let quiet = VideoPlaybackSettings { volume: 0.002, ..d };
    assert!((quiet.effective_volume() - 0.002).abs() < 1e-7);
    let loud = VideoPlaybackSettings { volume: 2.0, ..d };
    assert_eq!(loud.effective_volume(), 1.0);

    let clamped = VideoPlaybackSettings {
        speed: 9.0,
        volume: 4.0,
        muted: false,
    }
    .clamped();
    assert_eq!((clamped.speed, clamped.volume), (2.0, 1.0));
}

#[test]
fn loop_clock_wraps_at_half_and_double_speed() {
    let mut clock = VideoLoopClock::new(4.0);
    // 0.5x: 3 s of wall time = 1.5 s of video per step.
    assert_eq!(clock.advance(3.0, 0.5), 0);
    assert!((clock.position_secs - 1.5).abs() < 1e-12);
    assert_eq!(clock.advance(3.0, 0.5), 0);
    assert!((clock.position_secs - 3.0).abs() < 1e-12);
    assert_eq!(clock.advance(3.0, 0.5), 1);
    assert!((clock.position_secs - 0.5).abs() < 1e-12);

    // 2.0x: 5 s of wall time = 10 s of video = 2 wraps + 2 s (from 0.5 -> 10.5 -> 2.5).
    assert_eq!(clock.advance(5.0, 2.0), 2);
    assert!((clock.position_secs - 2.5).abs() < 1e-12);
    assert_eq!(clock.frame_index(30.0), 75);

    // Speeds outside the range are clamped (5x -> 2x).
    let mut fast = VideoLoopClock::new(10.0);
    assert_eq!(fast.advance(4.0, 5.0), 0);
    assert!((fast.position_secs - 8.0).abs() < 1e-12);
}

#[test]
fn loop_clock_does_not_drift_over_long_sessions() {
    let mut clock = VideoLoopClock::new(1.0);
    let mut wraps = 0u64;
    // 10 minutes at 60 Hz.
    for _ in 0..36_000 {
        wraps += u64::from(clock.advance(1.0 / 60.0, 1.0));
        assert!(clock.position_secs >= 0.0 && clock.position_secs < 1.0);
    }
    assert!((599..=600).contains(&wraps), "wraps {wraps}");
    let total = wraps as f64 + clock.position_secs;
    assert!((total - 600.0).abs() < 1e-6, "total {total}");
}

#[test]
fn loop_clock_paused_and_zero_duration_are_safe() {
    let mut clock = VideoLoopClock::new(2.0);
    clock.paused = true;
    assert_eq!(clock.advance(10.0, 1.0), 0);
    assert_eq!(clock.position_secs, 0.0);
    clock.paused = false;
    assert_eq!(clock.advance(f64::NAN, 1.0), 0);
    assert_eq!(clock.advance(-1.0, 1.0), 0);
    assert_eq!(clock.position_secs, 0.0);

    for duration in [0.0, -3.0, f64::NAN, f64::INFINITY] {
        let mut c = VideoLoopClock::new(duration);
        c.position_secs = 1.0;
        assert_eq!(c.advance(1.0, 1.0), 0);
        assert_eq!(c.position_secs, 0.0);
        assert_eq!(c.frame_index(60.0), 0);
    }
    let c = VideoLoopClock::new(2.0);
    assert_eq!(c.frame_index(0.0), 0);
    assert_eq!(c.frame_index(f64::NAN), 0);
}

// ------------------------------------------------------------------------------------------
// Embedded `.tex` MP4
// ------------------------------------------------------------------------------------------

fn fake_mp4(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8, 0, 0, 0x18];
    bytes.extend_from_slice(b"ftypisom");
    bytes.resize(len.max(bytes.len()), 0xAB);
    bytes
}

fn tex_with(free_image_format: i32, payload: Vec<u8>) -> ParsedTex {
    ParsedTex {
        header: TexHeader {
            format: TexFormat::RGBA8888,
            flags: 32,
            texture_width: 1024,
            texture_height: 1024,
            image_width: 1920,
            image_height: 1080,
            unk_int0: 0,
        },
        free_image_format,
        container_version: 4,
        images: vec![vec![TexMipmap {
            width: 1920,
            height: 1080,
            is_lz4_compressed: false,
            decompressed_bytes_count: 0,
            raw_bytes: payload,
        }]],
        sprite_frames: None,
    }
}

#[test]
fn detects_mp4_containers() {
    assert!(is_mp4_container(&fake_mp4(64)));
    assert!(is_mp4_container(b"\0\0\0\x20ftyp"));
    assert!(!is_mp4_container(b"\0\0\0\x20moov"));
    assert!(!is_mp4_container(b"ftyp"));
    assert!(!is_mp4_container(b""));
    assert!(!is_mp4_container(&[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A
    ]));
}

#[test]
fn cache_path_is_sanitized_and_rejects_traversal() {
    let dir = std::path::Path::new("/wp/123");
    assert_eq!(
        embedded_mp4_cache_path(dir, "materials/videos/clip.tex"),
        Some(dir.join(".cache").join("materials_videos_clip.mp4"))
    );
    assert_eq!(
        embedded_mp4_cache_path(dir, "/materials\\Clip.TEX"),
        Some(dir.join(".cache").join("materials_Clip.mp4"))
    );
    assert_eq!(
        embedded_mp4_cache_path(dir, "./a/./b.tex"),
        Some(dir.join(".cache").join("a_b.mp4"))
    );
    assert_eq!(
        embedded_mp4_cache_path(dir, "noext"),
        Some(dir.join(".cache").join("noext.mp4"))
    );
    for bad in [
        "",
        "/",
        ".tex",
        "..",
        "../escape.tex",
        "materials/../../escape.tex",
        "a\\..\\b.tex",
    ] {
        assert_eq!(embedded_mp4_cache_path(dir, bad), None, "{bad:?}");
    }
    // The sanitized file always lives directly in `.cache`.
    let p = embedded_mp4_cache_path(dir, "a/b/c/d.tex").unwrap();
    assert_eq!(p.parent().unwrap(), dir.join(".cache"));
}

#[test]
fn extracts_embedded_mp4_once() {
    let tmp = TempDir::new("video-tex");
    let payload = fake_mp4(4096);
    let tex = tex_with(FIF_MP4, payload.clone());
    assert!(tex.is_mp4());

    let path = extract_embedded_tex_mp4(&tex, tmp.path(), "materials/clip.tex")
        .unwrap()
        .expect("mp4 tex extracts");
    assert_eq!(path, tmp.path().join(".cache").join("materials_clip.mp4"));
    assert_eq!(std::fs::read(&path).unwrap(), payload);
    assert!(!path.with_extension("mp4.part").exists());

    // Identical payloads do not rewrite a cached file.
    let stamp = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    let again = extract_embedded_tex_mp4(&tex, tmp.path(), "materials/clip.tex")
        .unwrap()
        .unwrap();
    assert_eq!(again, path);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);

    // Same-length corruption or an updated Workshop texture must be replaced too.
    std::fs::write(&path, vec![0x11u8; payload.len()]).unwrap();
    extract_embedded_tex_mp4(&tex, tmp.path(), "materials/clip.tex").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), payload);
    let mut updated = payload.clone();
    *updated.last_mut().unwrap() ^= 1;
    let updated_tex = tex_with(FIF_MP4, updated.clone());
    extract_embedded_tex_mp4(&updated_tex, tmp.path(), "materials/clip.tex").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), updated);

    // A stale file with a different length is replaced.
    std::fs::write(&path, b"short").unwrap();
    extract_embedded_tex_mp4(&tex, tmp.path(), "materials/clip.tex").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), payload);

    // Traversal is rejected as an error, nothing is written outside the wallpaper dir.
    let err = extract_embedded_tex_mp4(&tex, tmp.path(), "../evil.tex").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn non_mp4_textures_are_not_extracted() {
    let tmp = TempDir::new("video-tex-none");
    let not_mp4 = tex_with(FIF_UNKNOWN, fake_mp4(256));
    assert_eq!(
        extract_embedded_tex_mp4(&not_mp4, tmp.path(), "a.tex").unwrap(),
        None
    );
    let bad_payload = tex_with(FIF_MP4, vec![0u8; 256]);
    assert_eq!(
        extract_embedded_tex_mp4(&bad_payload, tmp.path(), "a.tex").unwrap(),
        None
    );
    let mut empty = tex_with(FIF_MP4, Vec::new());
    empty.images.clear();
    assert_eq!(
        extract_embedded_tex_mp4(&empty, tmp.path(), "a.tex").unwrap(),
        None
    );
    assert!(!tmp.path().join(".cache").exists());
}

// ------------------------------------------------------------------------------------------
// Frame upload
// ------------------------------------------------------------------------------------------

#[test]
fn tightly_packed_rows_drops_stride_padding() {
    // 2x3 BGRA frame with 4 bytes of padding per row.
    let width = 2u32;
    let height = 3u32;
    let stride = 12usize;
    let mut data = Vec::new();
    for y in 0..height {
        for x in 0..width {
            data.extend_from_slice(&[y as u8, x as u8, 0xCC, 0xFF]);
        }
        data.extend_from_slice(&[0xEE; 4]);
    }
    let frame = VideoFrame {
        width,
        height,
        stride,
        format: VideoPixelFormat::Bgra8,
        data,
    };
    assert!(frame.is_valid());
    let packed = tightly_packed_rows(&frame);
    assert!(matches!(packed, Cow::Owned(_)));
    assert_eq!(packed.len(), 2 * 3 * 4);
    assert!(!packed.contains(&0xEE));
    assert_eq!(&packed[8..16], &[1, 0, 0xCC, 0xFF, 1, 1, 0xCC, 0xFF]);

    let tight = VideoFrame {
        stride: 8,
        data: packed.to_vec(),
        ..frame.clone()
    };
    let borrowed = tightly_packed_rows(&tight);
    assert!(matches!(borrowed, Cow::Borrowed(_)));
    assert_eq!(&*borrowed, &*packed);

    // The last row does not need trailing padding.
    let mut short = frame.clone();
    short.data.truncate(stride * 2 + 8);
    assert!(short.is_valid());
    assert_eq!(tightly_packed_rows(&short).len(), 24);

    // Invalid frames yield nothing.
    let mut truncated = frame.clone();
    truncated.data.truncate(10);
    assert!(tightly_packed_rows(&truncated).is_empty());
    let narrow = VideoFrame { stride: 4, ..frame };
    assert!(tightly_packed_rows(&narrow).is_empty());

    assert_eq!(
        VideoPixelFormat::Bgra8.wgpu_format(),
        wgpu::TextureFormat::Bgra8UnormSrgb
    );
    assert_eq!(
        VideoPixelFormat::Rgba8.wgpu_format(),
        wgpu::TextureFormat::Rgba8UnormSrgb
    );
}

// ------------------------------------------------------------------------------------------
// Web runtime JS bridge
// ------------------------------------------------------------------------------------------

#[test]
fn audio_frame_layout_and_sanitizing() {
    let mut left = [0.0f32; 64];
    let mut right = [0.0f32; 64];
    for i in 0..64 {
        left[i] = i as f32 / 100.0;
        right[i] = 1.0 + i as f32 / 100.0;
    }
    left[3] = f32::NAN;
    right[5] = f32::INFINITY;
    left[7] = -0.5;
    let frame = pack_web_audio_frame(&left, &right);
    assert_eq!(frame.len(), WEB_AUDIO_FRAME_LEN);
    assert_eq!(frame[0], 0.0);
    assert_eq!(frame[1], 0.01);
    assert_eq!(frame[3], 0.0);
    assert_eq!(frame[7], 0.0);
    assert_eq!(frame[63], 0.63);
    assert_eq!(frame[64], 1.0);
    assert_eq!(frame[64 + 5], 0.0);
    assert_eq!(frame[127], 1.63);
}

#[test]
fn audio_dispatch_js_has_128_numbers() {
    let mut frame = [0.25f32; 128];
    frame[10] = f32::NAN;
    let js = audio_dispatch_js(&frame);
    assert!(WEB_PRELOAD_JS.contains("window.__truongpaper_dispatch_audio = function"));
    let inner = js
        .strip_prefix("window.__truongpaper_dispatch_audio([")
        .and_then(|s| s.strip_suffix("]);"))
        .expect("call shape");
    let numbers: Vec<f64> = inner.split(',').map(|n| n.parse().unwrap()).collect();
    assert_eq!(numbers.len(), 128);
    assert_eq!(numbers[0], 0.25);
    assert_eq!(numbers[10], 0.0);
    assert!(inner.split(',').all(|n| n.len() == "0.2500".len()));
}

#[test]
fn properties_dispatch_validates_and_reserializes() {
    assert!(WEB_PRELOAD_JS.contains("window.__truongpaper_dispatch_properties = function"));

    let js = properties_dispatch_js(
        r#" { "speed" : {"value": 2}, "on": {"value":true} } "#,
        None,
    )
    .unwrap();
    assert_eq!(
        js,
        r#"window.__truongpaper_dispatch_properties({"speed":{"value":2},"on":{"value":true}}, null);"#
    );

    let js = properties_dispatch_js("{}", Some(r#"{"fps":30}"#)).unwrap();
    assert_eq!(
        js,
        r#"window.__truongpaper_dispatch_properties({}, {"fps":30});"#
    );

    // Non-objects and script text are rejected.
    for bad in [
        "[]",
        "1",
        "\"x\"",
        "null",
        "",
        "{}); alert(1); ({",
        "alert(1)",
        "{\"a\":1}; fetch('x')",
    ] {
        assert!(properties_dispatch_js(bad, None).is_err(), "{bad:?}");
    }
    assert!(properties_dispatch_js("{}", Some("[1]")).is_err());
    assert!(properties_dispatch_js("{}", Some("})(); x(")).is_err());

    // Strings that look like script are kept inside an escaped JSON string.
    let js =
        properties_dispatch_js(r#"{"t":{"value":"</script><script>alert(1)"}}"#, None).unwrap();
    assert!(!js.contains("</script>"));
    assert!(js.contains(r"\u003c/script\u003e"));
    let payload = js
        .strip_prefix("window.__truongpaper_dispatch_properties(")
        .and_then(|s| s.strip_suffix(", null);"))
        .unwrap();
    let v = Value::parse(payload).unwrap();
    assert_eq!(
        v.get("t")
            .and_then(|t| t.get("value"))
            .and_then(Value::as_str),
        Some("</script><script>alert(1)")
    );
}

#[test]
fn we_property_payload_wraps_values() {
    let payload = we_property_payload(&[
        ("rain_speed".into(), "1.5".into()),
        ("schemecolor".into(), "\"0.1 0.2 0.3\"".into()),
        ("enabled".into(), "true".into()),
        ("quote\"key".into(), "null".into()),
    ])
    .unwrap();
    assert_eq!(
        payload,
        r#"{"rain_speed":{"value":1.5},"schemecolor":{"value":"0.1 0.2 0.3"},"enabled":{"value":true},"quote\"key":{"value":null}}"#
    );
    assert!(we_property_payload(&[("x".into(), "1); alert(1".into())]).is_err());
    assert_eq!(we_property_payload(&[]).unwrap(), "{}");
    assert!(properties_dispatch_js(&payload, None).is_ok());
}

#[test]
fn video_host_js_uses_exact_function_names() {
    for name in [
        "window.__truongpaper_apply_video_adjustments = function",
        "window.__truongpaper_set_paused = function",
    ] {
        assert!(VIDEO_HOST_HTML.contains(name), "{name}");
    }
    let js = video_adjustments_js(r#"{"fitMode":"fit_blur","volume":0.5,"muted":false}"#).unwrap();
    assert_eq!(
        js,
        r#"window.__truongpaper_apply_video_adjustments({"fitMode":"fit_blur","volume":0.5,"muted":false});"#
    );
    assert!(video_adjustments_js("[]").is_err());
    assert!(video_adjustments_js("{}); evil(); ({").is_err());

    assert_eq!(
        video_set_paused_js(true),
        "window.__truongpaper_set_paused(true);"
    );
    assert_eq!(
        video_set_paused_js(false),
        "window.__truongpaper_set_paused(false);"
    );
}

#[test]
fn cursor_events_dispatch_on_element_under_point() {
    let js = cursor_event_js(WebCursorEventKind::Move, 12.5, 40.0).unwrap();
    assert!(js.contains("document.elementFromPoint(x,y)||document"));
    assert!(js.contains("new MouseEvent('mousemove'"));
    assert!(js.contains("var x=12.50,y=40.00"));
    assert!(js.contains("bubbles:true"));
    assert!(js.contains("clientX:x,clientY:y"));

    for (kind, ty) in [
        (WebCursorEventKind::Down, "mousedown"),
        (WebCursorEventKind::Up, "mouseup"),
        (WebCursorEventKind::Click, "click"),
    ] {
        assert!(cursor_event_js(kind, 0.0, 0.0)
            .unwrap()
            .contains(&format!("new MouseEvent('{ty}'")));
    }
    assert!(cursor_event_js(WebCursorEventKind::Down, 1.0, 2.0)
        .unwrap()
        .contains("buttons:1"));

    assert!(cursor_event_js(WebCursorEventKind::Move, f64::NAN, 1.0).is_err());
    assert!(cursor_event_js(WebCursorEventKind::Click, 1.0, f64::INFINITY).is_err());
}

// ------------------------------------------------------------------------------------------
// URLs and runtime selection
// ------------------------------------------------------------------------------------------

#[test]
fn url_builders_encode_and_reject_traversal() {
    let origin = wpe_origin();
    if cfg!(target_os = "windows") {
        assert_eq!(origin, "http://wpe.localhost/");
    } else {
        assert_eq!(origin, "wpe://localhost/");
    }

    assert_eq!(
        web_wallpaper_url("index.html").unwrap(),
        format!("{origin}index.html")
    );
    assert_eq!(
        web_wallpaper_url("/My Site/ページ.html").unwrap(),
        format!("{origin}My%20Site/%E3%83%9A%E3%83%BC%E3%82%B8.html")
    );
    assert_eq!(
        web_wallpaper_url("sub\\a#b?.html").unwrap(),
        format!("{origin}sub/a%23b%3F.html")
    );

    assert_eq!(
        video_host_url("videos/My Clip.mp4").unwrap(),
        format!("{origin}__video_host__.html?file=videos%2FMy%20Clip.mp4")
    );
    assert_eq!(
        video_host_url("/é&x=1.mp4").unwrap(),
        format!("{origin}__video_host__.html?file=%C3%A9%26x%3D1.mp4")
    );

    for bad in ["", "/", "..", "../x.html", "a/../../x.html", "a\\..\\x.mp4"] {
        assert_eq!(web_wallpaper_url(bad), None, "{bad:?}");
        assert_eq!(video_host_url(bad), None, "{bad:?}");
    }
    assert_eq!(encode_uri_component("a-b_c.d!~*'()"), "a-b_c.d!~*'()");
    assert_eq!(encode_uri_component("a b/c"), "a%20b%2Fc");
}

#[test]
fn runtime_kind_parses_project_types() {
    assert_eq!(
        WallpaperRuntimeKind::from_project_type("scene"),
        Some(WallpaperRuntimeKind::Scene)
    );
    assert_eq!(
        WallpaperRuntimeKind::from_project_type("Video"),
        Some(WallpaperRuntimeKind::Video)
    );
    assert_eq!(
        WallpaperRuntimeKind::from_project_type(" WEB "),
        Some(WallpaperRuntimeKind::Web)
    );
    assert_eq!(
        WallpaperRuntimeKind::from_project_type("APPLICATION"),
        Some(WallpaperRuntimeKind::Application)
    );
    assert_eq!(WallpaperRuntimeKind::from_project_type("preset"), None);
    assert_eq!(WallpaperRuntimeKind::from_project_type(""), None);
    assert!(WallpaperRuntimeKind::Video.uses_webview());
    assert!(WallpaperRuntimeKind::Web.uses_webview());
    assert!(!WallpaperRuntimeKind::Scene.uses_webview());
    assert_eq!(WallpaperRuntimeKind::Application.as_str(), "application");
}
