//! IPC types shared by the Tauri command layer that are not already provided by
//! `truongpaper-core` / `truongpaper-photo-api` (docs/04 §5 `src/types/ipc.ts`).
//!
//! Photo types (`PhotoItem`, `SavedAlbum`, `PublicCatalogAlbum`, `PhotoStatus`, `FilterMode`)
//! come from `truongpaper_photo_api` re-exports; `WorkshopPreview` comes from
//! `truongpaper_steam`; `AppSettings` comes from `truongpaper_core`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use truongpaper_renderer::{CompositorUniforms, FitMode, TransitionMode};

// ---------------------------------------------------------------------------------------------
// Event channel names (docs/04 §5.1 "Rust → Frontend Event Channels")
// ---------------------------------------------------------------------------------------------

pub const EVENT_WALLPAPER_CHANGED: &str = "wallpaper://changed";
pub const EVENT_PHOTO_QUEUE_UPDATED: &str = "photo-queue://updated";
pub const EVENT_SETTINGS_CHANGED: &str = "settings://changed";
pub const EVENT_STEAM_DOWNLOAD_PROGRESS: &str = "steam-download://progress";
pub const EVENT_STEAM_QR_MATRIX: &str = "steam-auth://qr-matrix";
pub const EVENT_STEAM_NEED_2FA: &str = "steam-auth://need-2fa";
pub const EVENT_POLICY_STATE_CHANGED: &str = "policy://state-changed";
pub const EVENT_PLAYBACK_PAUSE_CHANGED: &str = "playback://pause-changed";
pub const EVENT_SYSTEM_POWER: &str = "system-power";

// ---------------------------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------------------------

/// `FitMode` (`'fill' | 'fit_blur' | 'fit_black' | 'stretch'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcFitMode {
    #[default]
    Fill,
    FitBlur,
    FitBlack,
    Stretch,
}

impl IpcFitMode {
    /// Parses `AppSettings::default_fit_mode`; unknown strings fall back to `fill`.
    pub fn from_setting(s: &str) -> Self {
        match FitMode::from_setting_str(s) {
            Some(FitMode::Fill) | None => Self::Fill,
            Some(FitMode::FitBlack) => Self::FitBlack,
            Some(FitMode::Stretch) => Self::Stretch,
            Some(FitMode::FitBlur) => Self::FitBlur,
        }
    }

    pub fn to_renderer(self) -> FitMode {
        match self {
            Self::Fill => FitMode::Fill,
            Self::FitBlur => FitMode::FitBlur,
            Self::FitBlack => FitMode::FitBlack,
            Self::Stretch => FitMode::Stretch,
        }
    }
}

/// `TransitionType` (`'crossfade' | 'blur_dissolve' | 'liquid_glass'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcTransitionType {
    Crossfade,
    BlurDissolve,
    #[default]
    LiquidGlass,
}

impl IpcTransitionType {
    /// Parses `AppSettings::transition_type`; unknown strings fall back to `liquid_glass`.
    pub fn from_setting(s: &str) -> Self {
        match TransitionMode::from_setting_str(s) {
            Some(TransitionMode::Crossfade) => Self::Crossfade,
            Some(TransitionMode::BlurDissolve) => Self::BlurDissolve,
            Some(TransitionMode::LiquidGlass) | None => Self::LiquidGlass,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crossfade => "crossfade",
            Self::BlurDissolve => "blur_dissolve",
            Self::LiquidGlass => "liquid_glass",
        }
    }

    pub fn to_renderer(self) -> TransitionMode {
        match self {
            Self::Crossfade => TransitionMode::Crossfade,
            Self::BlurDissolve => TransitionMode::BlurDissolve,
            Self::LiquidGlass => TransitionMode::LiquidGlass,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Universal adjustments
// ---------------------------------------------------------------------------------------------

/// `UniversalAdjustments` with the documented ranges (docs/04 §5).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UniversalAdjustments {
    pub volume: f32,
    pub muted: bool,
    pub audio_sensitivity: f32,
    pub playback_speed: f32,
    pub fit_mode: IpcFitMode,
    pub zoom: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub hue_shift: f32,
}

impl Default for UniversalAdjustments {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
            audio_sensitivity: 1.0,
            playback_speed: 1.0,
            fit_mode: IpcFitMode::Fill,
            zoom: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            hue_shift: 0.0,
        }
    }
}

fn clamp_or(v: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        fallback
    }
}

impl UniversalAdjustments {
    /// Clamps every field to its documented range; non-finite values reset to the default.
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        Self {
            volume: clamp_or(self.volume, 0.0, 1.0, d.volume),
            muted: self.muted,
            audio_sensitivity: clamp_or(self.audio_sensitivity, 0.0, 5.0, d.audio_sensitivity),
            playback_speed: truongpaper_renderer::clamp_playback_speed(self.playback_speed),
            fit_mode: self.fit_mode,
            zoom: clamp_or(self.zoom, 0.5, 2.0, d.zoom),
            offset_x: clamp_or(self.offset_x, -1.0, 1.0, d.offset_x),
            offset_y: clamp_or(self.offset_y, -1.0, 1.0, d.offset_y),
            brightness: clamp_or(self.brightness, -0.5, 0.5, d.brightness),
            contrast: clamp_or(self.contrast, 0.5, 2.0, d.contrast),
            saturation: clamp_or(self.saturation, 0.0, 2.0, d.saturation),
            hue_shift: clamp_or(
                self.hue_shift,
                -std::f32::consts::PI,
                std::f32::consts::PI,
                d.hue_shift,
            ),
        }
    }

    /// Parses a stored `adjustments_json` blob. Missing fields take defaults; a missing
    /// `fitMode` takes `default_fit` (from `AppSettings::default_fit_mode`).
    pub fn from_stored_json(raw: &str, default_fit: IpcFitMode) -> Self {
        let mut value: Value = serde_json::from_str(raw).unwrap_or(Value::Object(Map::new()));
        if let Some(obj) = value.as_object_mut() {
            if !obj.contains_key("fitMode") {
                obj.insert(
                    "fitMode".into(),
                    serde_json::to_value(default_fit).unwrap_or(Value::Null),
                );
            }
        }
        serde_json::from_value::<Self>(value)
            .unwrap_or(Self {
                fit_mode: default_fit,
                ..Self::default()
            })
            .sanitized()
    }

    /// Maps fit mode + color grading onto the compositor uniform block (`truongpaper-renderer`).
    /// `progress = 1.0` (no transition in flight) and fit modes `from == to`.
    pub fn compositor_uniforms(&self, transition: IpcTransitionType) -> CompositorUniforms {
        let a = self.sanitized();
        let fit = a.fit_mode.to_renderer() as u32;
        CompositorUniforms {
            transition_mode: transition.to_renderer() as u32,
            from_fit_mode: fit,
            to_fit_mode: fit,
            zoom: a.zoom,
            offset_x: a.offset_x,
            offset_y: a.offset_y,
            brightness: a.brightness,
            contrast: a.contrast,
            saturation: a.saturation,
            hue_shift: a.hue_shift,
            ..CompositorUniforms::default()
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Wallpapers, monitors, playlists
// ---------------------------------------------------------------------------------------------

/// `WallpaperSource` (tagged by `kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WallpaperSource {
    #[default]
    PhotoApi,
    Workshop {
        #[serde(rename = "wallpaperId")]
        wallpaper_id: String,
    },
    Playlist {
        #[serde(rename = "playlistId")]
        playlist_id: String,
    },
}

impl WallpaperSource {
    /// Parses `monitor_assignments.source_json`; anything unreadable means `photo_api`.
    pub fn from_stored_json(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperItem {
    pub wallpaper_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<String>,
    pub title: String,
    #[serde(rename = "type")]
    pub wallpaper_type: String,
    pub preview_path: String,
    pub install_dir: String,
    pub bytes: u64,
    pub audio_reactive: bool,
    pub windows_only: bool,
    pub project_json: Value,
    pub user_properties: Map<String, Value>,
    pub adjustments: UniversalAdjustments,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperPreset {
    pub preset_id: String,
    pub wallpaper_id: String,
    pub name: String,
    pub properties: Map<String, Value>,
    pub adjustments: UniversalAdjustments,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub monitor_id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub refresh_rate_hz: f64,
    pub is_primary: bool,
    pub is_portrait: bool,
    pub user_paused: bool,
    pub active_source: WallpaperSource,
    pub active_wallpaper_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackPauseChanged {
    pub monitor_id: Option<String>,
    pub paused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistMode {
    PhotosOnly,
    WorkshopOnly,
    #[default]
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaylistItem {
    PhotoApiDraw,
    Workshop {
        #[serde(rename = "wallpaperId")]
        wallpaper_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    #[serde(default)]
    pub playlist_id: String,
    pub name: String,
    #[serde(default)]
    pub mode: PlaylistMode,
    #[serde(default = "default_playlist_interval")]
    pub interval_seconds: u64,
    #[serde(default)]
    pub items: Vec<PlaylistItem>,
}

fn default_playlist_interval() -> u64 {
    900
}

// ---------------------------------------------------------------------------------------------
// Event payloads
// ---------------------------------------------------------------------------------------------

/// The source assignment and its currently displayed item (if any).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperChangedPayload<'a, T: Serialize> {
    pub monitor_id: &'a str,
    pub source: &'a WallpaperSource,
    pub current_item: Option<&'a T>,
    pub transition_used: &'static str,
}

/// `steam-download://progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamDownloadProgress {
    pub workshop_id: String,
    pub title: String,
    pub progress_percent: f32,
    /// `'downloading' | 'completed' | 'error'`
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `steam-auth://qr-matrix`.
#[derive(Debug, Clone, Serialize)]
pub struct QrMatrixPayload {
    pub matrix: Vec<Vec<bool>>,
}

/// `steam-auth://need-2fa`.
#[derive(Debug, Clone, Serialize)]
pub struct Need2FaPayload {
    /// `'totp' | 'email'`
    pub kind: &'static str,
    pub prompt: String,
}

/// `policy://state-changed`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyStatePayload {
    pub monitor_id: String,
    /// `'playing' | 'paused_fullscreen' | 'paused_maximized' | 'paused_battery' | 'paused_display_sleep'`
    pub state: &'static str,
}

/// Accepts only the scalar property value types allowed by `monitor_update_property`.
pub fn validate_property_update(prop_key: &str, prop_value: &Value) -> Result<(), String> {
    let key = prop_key.trim();
    if key.is_empty() || key.len() > 128 {
        return Err("Property key must be 1-128 characters".into());
    }
    match prop_value {
        Value::String(s) if s.len() <= 4096 => Ok(()),
        Value::String(_) => Err("Property value is too long".into()),
        Value::Bool(_) => Ok(()),
        Value::Number(n) if n.as_f64().is_some_and(f64::is_finite) => Ok(()),
        _ => Err("Property value must be a string, number or boolean".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wallpaper_source_matches_ipc_shape() {
        assert_eq!(
            serde_json::to_value(WallpaperSource::PhotoApi).unwrap(),
            json!({"kind": "photo_api"})
        );
        let ws = WallpaperSource::Workshop {
            wallpaper_id: "workshop:1".into(),
        };
        assert_eq!(
            serde_json::to_value(&ws).unwrap(),
            json!({"kind": "workshop", "wallpaperId": "workshop:1"})
        );
        let pl: WallpaperSource =
            serde_json::from_value(json!({"kind": "playlist", "playlistId": "p1"})).unwrap();
        assert_eq!(
            pl,
            WallpaperSource::Playlist {
                playlist_id: "p1".into()
            }
        );
        // Matches the core schema's default `source_json`.
        assert_eq!(
            WallpaperSource::from_stored_json("{\"kind\":\"photo_api\"}"),
            WallpaperSource::PhotoApi
        );
        assert_eq!(
            WallpaperSource::from_stored_json("garbage"),
            WallpaperSource::PhotoApi
        );
    }

    #[test]
    fn playlist_round_trips_ipc_shape() {
        let raw = json!({
            "playlistId": "p1",
            "name": "Mix",
            "mode": "mixed",
            "intervalSeconds": 600,
            "items": [{"kind": "photo_api_draw"}, {"kind": "workshop", "wallpaperId": "workshop:9"}]
        });
        let p: Playlist = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(serde_json::to_value(&p).unwrap(), raw);
        let minimal: Playlist = serde_json::from_value(json!({"name": "x"})).unwrap();
        assert_eq!(minimal.interval_seconds, 900);
        assert!(minimal.playlist_id.is_empty());
    }

    #[test]
    fn adjustments_clamp_and_parse_with_default_fit() {
        let wild = UniversalAdjustments {
            volume: 4.0,
            zoom: f32::NAN,
            brightness: -9.0,
            hue_shift: 10.0,
            playback_speed: 0.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(wild.volume, 1.0);
        assert_eq!(wild.zoom, 1.0);
        assert_eq!(wild.brightness, -0.5);
        assert_eq!(wild.hue_shift, std::f32::consts::PI);
        assert_eq!(wild.playback_speed, 0.1);

        let a = UniversalAdjustments::from_stored_json("{}", IpcFitMode::Fill);
        assert_eq!(a.fit_mode, IpcFitMode::Fill);
        let b = UniversalAdjustments::from_stored_json(
            r#"{"fitMode":"stretch","contrast":1.5,"playbackSpeed":3.0}"#,
            IpcFitMode::Fill,
        );
        assert_eq!(b.fit_mode, IpcFitMode::Stretch);
        assert_eq!(b.contrast, 1.5);
        assert_eq!(b.playback_speed, 2.0);
        let c = UniversalAdjustments::from_stored_json("not json", IpcFitMode::FitBlack);
        assert_eq!(c.fit_mode, IpcFitMode::FitBlack);

        let json = serde_json::to_value(UniversalAdjustments::default()).unwrap();
        for key in [
            "volume",
            "muted",
            "audioSensitivity",
            "playbackSpeed",
            "fitMode",
            "zoom",
            "offsetX",
            "offsetY",
            "brightness",
            "contrast",
            "saturation",
            "hueShift",
        ] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
        assert_eq!(json["fitMode"], "fill");
    }

    #[test]
    fn adjustments_map_to_compositor_uniforms() {
        let a = UniversalAdjustments {
            fit_mode: IpcFitMode::Stretch,
            zoom: 1.5,
            saturation: 0.25,
            ..Default::default()
        };
        let u = a.compositor_uniforms(IpcTransitionType::BlurDissolve);
        assert_eq!(u.transition_mode, TransitionMode::BlurDissolve as u32);
        assert_eq!(u.to_fit_mode, FitMode::Stretch as u32);
        assert_eq!(u.from_fit_mode, FitMode::Stretch as u32);
        assert_eq!(u.zoom, 1.5);
        assert_eq!(u.saturation, 0.25);
        assert_eq!(u.progress, 1.0);
    }

    #[test]
    fn setting_string_parsing() {
        assert_eq!(IpcFitMode::from_setting("fit_black"), IpcFitMode::FitBlack);
        assert_eq!(IpcFitMode::from_setting("bogus"), IpcFitMode::Fill);
        assert_eq!(
            IpcTransitionType::from_setting("crossfade").as_str(),
            "crossfade"
        );
        assert_eq!(
            IpcTransitionType::from_setting("???"),
            IpcTransitionType::LiquidGlass
        );
    }

    #[test]
    fn event_payload_shapes() {
        let item = json!({"id": "p"});
        let changed = WallpaperChangedPayload {
            monitor_id: "DISPLAY-1",
            source: &WallpaperSource::PhotoApi,
            current_item: Some(&item),
            transition_used: "liquid_glass",
        };
        assert_eq!(
            serde_json::to_value(&changed).unwrap(),
            json!({"monitorId": "DISPLAY-1", "source": {"kind": "photo_api"}, "currentItem": {"id": "p"}, "transitionUsed": "liquid_glass"})
        );
        let prog = SteamDownloadProgress {
            workshop_id: "1".into(),
            title: "T".into(),
            progress_percent: 42.5,
            state: "downloading",
            error: None,
        };
        assert_eq!(
            serde_json::to_value(&prog).unwrap(),
            json!({"workshopId": "1", "title": "T", "progressPercent": 42.5, "state": "downloading"})
        );
        let err = SteamDownloadProgress {
            error: Some("boom".into()),
            state: "error",
            ..prog
        };
        assert_eq!(serde_json::to_value(&err).unwrap()["error"], "boom");
        assert_eq!(
            serde_json::to_value(QrMatrixPayload {
                matrix: vec![vec![true, false]]
            })
            .unwrap(),
            json!({"matrix": [[true, false]]})
        );
        assert_eq!(
            serde_json::to_value(Need2FaPayload {
                kind: "totp",
                prompt: "code?".into()
            })
            .unwrap(),
            json!({"kind": "totp", "prompt": "code?"})
        );
        assert_eq!(
            serde_json::to_value(PolicyStatePayload {
                monitor_id: "A".into(),
                state: "paused_fullscreen"
            })
            .unwrap(),
            json!({"monitorId": "A", "state": "paused_fullscreen"})
        );
    }

    #[test]
    fn wallpaper_item_serializes_type_and_optional_workshop_id() {
        let item = WallpaperItem {
            wallpaper_id: "local:x".into(),
            workshop_id: None,
            title: "t".into(),
            wallpaper_type: "video".into(),
            preview_path: String::new(),
            install_dir: String::new(),
            bytes: 1,
            audio_reactive: false,
            windows_only: false,
            project_json: json!({}),
            user_properties: Map::new(),
            adjustments: UniversalAdjustments::default(),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "video");
        assert!(v.get("workshopId").is_none());
        assert!(v.get("windowsOnly").is_some());
    }

    #[test]
    fn property_validation() {
        assert!(validate_property_update("color", &json!("1 0 0")).is_ok());
        assert!(validate_property_update("speed", &json!(0.5)).is_ok());
        assert!(validate_property_update("on", &json!(true)).is_ok());
        assert!(validate_property_update("", &json!(1)).is_err());
        assert!(validate_property_update("x", &json!({"a": 1})).is_err());
        assert!(validate_property_update("x", &json!(null)).is_err());
    }
}
