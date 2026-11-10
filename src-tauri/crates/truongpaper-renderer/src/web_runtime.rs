//! Web runtime helpers (docs/05 Phase 5, docs/03 §6.2, §9).
//!
//! Builds the `wpe://` URLs loaded into the per-monitor desktop webview and the JavaScript
//! snippets passed to `evaluate_script` that call into `web-wallpaper-preload.js`
//! (`__truongpaper_dispatch_audio`, `__truongpaper_dispatch_properties`) and
//! `video-wallpaper-host.html` (`__truongpaper_apply_video_adjustments`,
//! `__truongpaper_set_paused`).
//!
//! Every JSON payload is parsed with [`crate::json::Value`] and re-serialized before it is placed
//! in a script, so callers can never inject script text.

use crate::json::Value;

/// Floats per Wallpaper Engine audio frame: 64 left bins followed by 64 right bins.
pub const WEB_AUDIO_FRAME_LEN: usize = 128;
/// Spectrum bins per channel.
pub const WEB_AUDIO_BINS_PER_CHANNEL: usize = WEB_AUDIO_FRAME_LEN / 2;

/// Path of the built-in video player page served by the `wpe://` protocol.
pub const VIDEO_HOST_PAGE: &str = "__video_host__.html";

fn sanitize_bin(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

/// Packs two 64-bin spectra into the 128-float layout expected by
/// `wallpaperRegisterAudioListener` callbacks (0..63 left, 64..127 right). Non-finite and
/// negative values become `0.0`.
pub fn pack_web_audio_frame(
    left: &[f32; WEB_AUDIO_BINS_PER_CHANNEL],
    right: &[f32; WEB_AUDIO_BINS_PER_CHANNEL],
) -> [f32; WEB_AUDIO_FRAME_LEN] {
    let mut out = [0.0f32; WEB_AUDIO_FRAME_LEN];
    for i in 0..WEB_AUDIO_BINS_PER_CHANNEL {
        out[i] = sanitize_bin(left[i]);
        out[WEB_AUDIO_BINS_PER_CHANNEL + i] = sanitize_bin(right[i]);
    }
    out
}

/// `window.__truongpaper_dispatch_audio([...128 numbers...]);` with 4 decimal places.
pub fn audio_dispatch_js(frame: &[f32; WEB_AUDIO_FRAME_LEN]) -> String {
    use std::fmt::Write;
    let mut js = String::with_capacity(48 + WEB_AUDIO_FRAME_LEN * 7);
    js.push_str("window.__truongpaper_dispatch_audio([");
    for (i, v) in frame.iter().enumerate() {
        if i > 0 {
            js.push(',');
        }
        let _ = write!(js, "{:.4}", sanitize_bin(*v));
    }
    js.push_str("]);");
    js
}

fn parse_object(json: &str, what: &str) -> Result<Value, String> {
    let value = Value::parse(json).map_err(|e| format!("{what}: {e}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(format!("{what} must be a JSON object"))
    }
}

/// `window.__truongpaper_dispatch_properties(<props>, <general or null>);`.
///
/// `user_props_json` (and `general_json` when given) must be JSON objects; they are re-serialized
/// before being embedded.
pub fn properties_dispatch_js(
    user_props_json: &str,
    general_json: Option<&str>,
) -> Result<String, String> {
    let props = parse_object(user_props_json, "user properties")?;
    let general = match general_json {
        Some(g) => parse_object(g, "general properties")?.to_json_string(),
        None => "null".to_string(),
    };
    Ok(format!(
        "window.__truongpaper_dispatch_properties({}, {general});",
        props.to_json_string()
    ))
}

/// Builds a `wallpaperPropertyListener.applyUserProperties` payload:
/// `{"key":{"value":<v>}, ...}` from `(key, already-serialized JSON value)` pairs.
pub fn we_property_payload(pairs: &[(String, String)]) -> Result<String, String> {
    let mut entries = Vec::with_capacity(pairs.len());
    for (key, raw) in pairs {
        let value = Value::parse(raw).map_err(|e| format!("property '{key}': {e}"))?;
        entries.push((
            key.clone(),
            Value::Object(vec![("value".to_string(), value)]),
        ));
    }
    Ok(Value::Object(entries).to_json_string())
}

/// `window.__truongpaper_apply_video_adjustments(<adjustments>);` (`adj_json` must be an object,
/// the camelCase `UniversalAdjustments` IPC shape).
pub fn video_adjustments_js(adj_json: &str) -> Result<String, String> {
    let adj = parse_object(adj_json, "video adjustments")?;
    Ok(format!(
        "window.__truongpaper_apply_video_adjustments({});",
        adj.to_json_string()
    ))
}

/// `window.__truongpaper_set_paused(true|false);`.
pub fn video_set_paused_js(paused: bool) -> String {
    format!("window.__truongpaper_set_paused({paused});")
}

/// Desktop cursor events forwarded into Web wallpapers (the desktop window ignores real mouse
/// events so Finder / Explorer icons stay clickable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebCursorEventKind {
    Move,
    Down,
    Up,
    Click,
}

impl WebCursorEventKind {
    pub fn dom_event_type(self) -> &'static str {
        match self {
            WebCursorEventKind::Move => "mousemove",
            WebCursorEventKind::Down => "mousedown",
            WebCursorEventKind::Up => "mouseup",
            WebCursorEventKind::Click => "click",
        }
    }

    /// `MouseEvent.buttons` while the event is dispatched (primary button held for `mousedown`).
    fn buttons(self) -> u8 {
        match self {
            WebCursorEventKind::Down => 1,
            _ => 0,
        }
    }
}

/// Dispatches a synthetic bubbling `MouseEvent` at `(x_px, y_px)` (CSS pixels, top-left origin)
/// on `document.elementFromPoint(x, y) || document`. Rejects non-finite coordinates.
pub fn cursor_event_js(kind: WebCursorEventKind, x_px: f64, y_px: f64) -> Result<String, String> {
    if !x_px.is_finite() || !y_px.is_finite() {
        return Err(format!("non-finite cursor position ({x_px}, {y_px})"));
    }
    Ok(format!(
        "(function(){{var x={x:.2},y={y:.2};var t=document.elementFromPoint(x,y)||document;\
t.dispatchEvent(new MouseEvent('{ty}',{{clientX:x,clientY:y,screenX:x,screenY:y,\
button:0,buttons:{buttons},bubbles:true,cancelable:true,view:window}}));}})();",
        x = x_px,
        y = y_px,
        ty = kind.dom_event_type(),
        buttons = kind.buttons(),
    ))
}

// ---------------------------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------------------------

/// Origin of the `wpe` custom protocol. Tauri v2 / wry map custom schemes to
/// `http://<scheme>.localhost/` on WebView2 (Windows); WKWebView uses `wpe://localhost/`.
pub fn wpe_origin() -> &'static str {
    if cfg!(target_os = "windows") {
        "http://wpe.localhost/"
    } else {
        "wpe://localhost/"
    }
}

/// `encodeURIComponent`: keeps `A-Z a-z 0-9 - _ . ! ~ * ' ( )` and percent-encodes every other
/// UTF-8 byte.
pub fn encode_uri_component(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Splits a project-relative file path into segments: `\` is treated as `/`, empty and `.`
/// segments are dropped, and any `..` segment rejects the whole path.
fn project_path_segments(project_file: &str) -> Option<Vec<&str>> {
    let mut segments = Vec::new();
    for seg in project_file.split(['/', '\\']) {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s => segments.push(s),
        }
    }
    if segments.is_empty() {
        None
    } else {
        Some(segments)
    }
}

/// URL of a Web wallpaper entry document (`project.json` `file`), e.g. `index.html` ->
/// `wpe://localhost/index.html`. Each path segment is percent-encoded.
pub fn web_wallpaper_url(project_file: &str) -> Option<String> {
    let segments = project_path_segments(project_file)?;
    let encoded: Vec<String> = segments.iter().map(|s| encode_uri_component(s)).collect();
    Some(format!("{}{}", wpe_origin(), encoded.join("/")))
}

/// URL of the built-in video player for a standalone Video wallpaper (docs/03 §6.2):
/// `<origin>__video_host__.html?file=<encodeURIComponent(file)>`.
pub fn video_host_url(project_file: &str) -> Option<String> {
    let segments = project_path_segments(project_file)?;
    Some(format!(
        "{}{VIDEO_HOST_PAGE}?file={}",
        wpe_origin(),
        encode_uri_component(&segments.join("/"))
    ))
}

/// Runtime selected from `project.json` `"type"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WallpaperRuntimeKind {
    Scene,
    Video,
    Web,
    Application,
}

impl WallpaperRuntimeKind {
    /// Case-insensitive `scene` / `video` / `web` / `application` (surrounding whitespace ignored).
    pub fn from_project_type(project_type: &str) -> Option<Self> {
        let t = project_type.trim();
        [
            ("scene", Self::Scene),
            ("video", Self::Video),
            ("web", Self::Web),
            ("application", Self::Application),
        ]
        .into_iter()
        .find(|(name, _)| t.eq_ignore_ascii_case(name))
        .map(|(_, kind)| kind)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scene => "scene",
            Self::Video => "video",
            Self::Web => "web",
            Self::Application => "application",
        }
    }

    /// `true` when the runtime is hosted in a desktop webview (Web pages and the video host page).
    pub fn uses_webview(self) -> bool {
        matches!(self, Self::Video | Self::Web)
    }
}
