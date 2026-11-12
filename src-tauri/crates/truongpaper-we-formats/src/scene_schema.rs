//! `scene.json` / `effect.json` / `material.json` / `model.json` schema and `project.json`
//! user-property bindings (`UserBoundValue<T>`, spec section 7.1).
//!
//! Any scene property may be a plain value (`"alpha": 1.0`), a user-property binding
//! (`{ "user": "opacity_slider", "value": 1.0 }`), a conditional binding
//! (`{ "user": { "name": "weather_mode", "condition": "2" }, "value": true }`) or a scripted
//! property (`{ "script": "...", "value": "960 540 0" }`). Unknown fields are ignored everywhere.

use glam::{Vec2, Vec3, Vec4};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// Current `project.json` user-property values keyed by property name.
///
/// Values are the raw property values (`true`, `0.5`, `"1 0 0"`, `2`). A full `project.json`
/// property object (`{ "type": "slider", "value": 0.5 }`) is also accepted; its `value` is used.
pub type UserPropertyMap = HashMap<String, serde_json::Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserConditionSpec {
    PropertyName(String),
    Conditional {
        name: String,
        #[serde(default)]
        condition: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserBoundValue<T> {
    Bound {
        #[serde(default)]
        user: Option<UserConditionSpec>,
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        scriptproperties: Option<HashMap<String, serde_json::Value>>,
        value: T,
    },
    Plain(T),
}

impl<T: Clone> UserBoundValue<T> {
    pub fn default_value(&self) -> &T {
        match self {
            Self::Plain(v) => v,
            Self::Bound { value, .. } => value,
        }
    }

    /// The bound `project.json` property name, if any.
    pub fn user_property_name(&self) -> Option<&str> {
        match self {
            Self::Bound {
                user: Some(UserConditionSpec::PropertyName(name)),
                ..
            }
            | Self::Bound {
                user: Some(UserConditionSpec::Conditional { name, .. }),
                ..
            } => Some(name),
            _ => None,
        }
    }

    /// Instance-level `scriptproperties` stored next to the script, if any.
    pub fn script_properties(&self) -> Option<&HashMap<String, serde_json::Value>> {
        match self {
            Self::Bound {
                scriptproperties: Some(p),
                ..
            } => Some(p),
            _ => None,
        }
    }

    /// The attached SceneScript source, if any.
    pub fn script(&self) -> Option<&str> {
        match self {
            Self::Bound {
                script: Some(s), ..
            } => Some(s),
            _ => None,
        }
    }
}

/// Looks up a user property and unwraps a `project.json` property object to its `value`.
fn user_prop_value<'a>(
    user_props: &'a UserPropertyMap,
    key: &str,
) -> Option<&'a serde_json::Value> {
    let raw = user_props.get(key)?;
    match raw {
        serde_json::Value::Object(obj) => obj.get("value").or(Some(raw)),
        other => Some(other),
    }
}

/// Whether a property value satisfies a binding `condition` string (a combo value such as `"2"`,
/// or `"1"`/`"true"` for a checked bool). Numbers compare numerically, so `2.0` matches `"2"`.
pub fn condition_matches(actual: &serde_json::Value, condition: &str) -> bool {
    let cond = condition.trim();
    match actual {
        serde_json::Value::String(s) => s.trim() == cond,
        serde_json::Value::Bool(b) => {
            let truthy = matches!(cond, "1" | "true");
            let falsy = matches!(cond, "0" | "false");
            (*b && truthy) || (!*b && falsy)
        }
        serde_json::Value::Number(n) => match (n.as_f64(), cond.parse::<f64>()) {
            (Some(a), Ok(c)) => (a - c).abs() < 1e-9,
            _ => n.to_string() == cond,
        },
        other => *other == cond,
    }
}

/// Resolves a raw JSON value that may be a `{ "user": ..., "value": ... }` binding (used for
/// SceneScript `scriptproperties` and particle `instanceoverride` entries). Non-binding values
/// are returned unchanged; a condition binding resolves to `true`/`false` like a bool.
pub fn resolve_json_binding(
    raw: &serde_json::Value,
    user_props: &UserPropertyMap,
) -> serde_json::Value {
    let Some(obj) = raw.as_object().filter(|o| o.contains_key("user")) else {
        return raw.clone();
    };
    let default = obj.get("value").cloned().unwrap_or(serde_json::Value::Null);
    let spec: Option<UserConditionSpec> = serde_json::from_value(obj["user"].clone()).ok();
    match spec {
        Some(UserConditionSpec::PropertyName(key))
        | Some(UserConditionSpec::Conditional {
            name: key,
            condition: None,
        }) => user_prop_value(user_props, &key)
            .cloned()
            .unwrap_or(default),
        Some(UserConditionSpec::Conditional {
            name,
            condition: Some(cond),
        }) => user_prop_value(user_props, &name)
            .map(|v| serde_json::Value::Bool(condition_matches(v, &cond)))
            .unwrap_or(default),
        None => default,
    }
}

/// Evaluates a UserBoundValue<bool> (such as layer or effect `visible`) against current project.json values
pub fn resolve_bound_bool(bound: &UserBoundValue<bool>, user_props: &UserPropertyMap) -> bool {
    match bound {
        UserBoundValue::Plain(b) => *b,
        UserBoundValue::Bound { user, value, .. } => {
            let Some(user_spec) = user else {
                return *value;
            };
            match user_spec {
                UserConditionSpec::PropertyName(prop_key) => user_prop_value(user_props, prop_key)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(*value),
                UserConditionSpec::Conditional { name, condition } => {
                    let Some(actual) = user_prop_value(user_props, name) else {
                        return *value;
                    };
                    if let Some(expected_cond) = condition {
                        condition_matches(actual, expected_cond)
                    } else {
                        actual.as_bool().unwrap_or(*value)
                    }
                }
            }
        }
    }
}

/// Resolves any `UserBoundValue<T>` against current `project.json` values.
///
/// The bound property value is used when it deserializes into `T` (numeric strings such as
/// `"0.5"` are also accepted for numeric `T`); otherwise the scene's default `value` is returned.
/// Conditional bindings only affect booleans, so for other types they resolve like a plain
/// property-name binding when no `condition` is set, and to the default otherwise.
pub fn resolve_bound_value<T: Clone + DeserializeOwned>(
    bound: &UserBoundValue<T>,
    user_props: &UserPropertyMap,
) -> T {
    let (key, value) = match bound {
        UserBoundValue::Plain(v) => return v.clone(),
        UserBoundValue::Bound {
            user: Some(UserConditionSpec::PropertyName(key)),
            value,
            ..
        }
        | UserBoundValue::Bound {
            user:
                Some(UserConditionSpec::Conditional {
                    name: key,
                    condition: None,
                }),
            value,
            ..
        } => (key, value),
        UserBoundValue::Bound { value, .. } => return value.clone(),
    };
    let Some(actual) = user_prop_value(user_props, key) else {
        return value.clone();
    };
    if let Ok(v) = serde_json::from_value::<T>(actual.clone()) {
        return v;
    }
    if let serde_json::Value::String(s) = actual {
        if let Ok(v) = serde_json::from_str::<T>(s.trim()) {
            return v;
        }
    }
    value.clone()
}

/// Splits a Wallpaper Engine vector string (`"0.5 1.0 0"`) into components. Commas are treated
/// as separators; unparsable components are skipped.
pub fn parse_vec_components(raw: &str) -> Vec<f32> {
    raw.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f32>().ok())
        .collect()
}

fn fill_components<const N: usize>(raw: &str, default: [f32; N]) -> [f32; N] {
    let mut out = default;
    for (slot, v) in out.iter_mut().zip(parse_vec_components(raw)) {
        *slot = v;
    }
    out
}

/// Parses `"X Y"`; missing components come from `default`.
pub fn parse_vec2_or(raw: &str, default: Vec2) -> Vec2 {
    Vec2::from_array(fill_components(raw, default.to_array()))
}

/// Parses `"X Y Z"`; missing components come from `default`.
pub fn parse_vec3_or(raw: &str, default: Vec3) -> Vec3 {
    Vec3::from_array(fill_components(raw, default.to_array()))
}

/// Parses `"X Y Z W"`; missing components come from `default` (e.g. alpha for `"R G B"`).
pub fn parse_vec4_or(raw: &str, default: Vec4) -> Vec4 {
    Vec4::from_array(fill_components(raw, default.to_array()))
}

/// Parses a `scene.json` document.
pub fn parse_scene_json(json: &str) -> serde_json::Result<SceneFile> {
    serde_json::from_str(json)
}

/// Parses a JSON object of user-property values into a [`UserPropertyMap`].
pub fn parse_user_properties(json: &str) -> serde_json::Result<UserPropertyMap> {
    serde_json::from_str(json)
}

/// Treats an explicit JSON `null` like a missing field.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFile {
    #[serde(default)]
    pub camera: SceneCamera,
    pub general: SceneGeneral,
    #[serde(default)]
    pub objects: Vec<SceneObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneCamera {
    #[serde(default = "default_zero_vec3")]
    pub center: String,
    #[serde(default = "default_eye_vec3")]
    pub eye: String,
    #[serde(default = "default_up_vec3")]
    pub up: String,
}
fn default_zero_vec3() -> String {
    "0 0 0".into()
}
fn default_eye_vec3() -> String {
    "0 0 1".into()
}
fn default_up_vec3() -> String {
    "0 1 0".into()
}
impl Default for SceneCamera {
    fn default() -> Self {
        Self {
            center: default_zero_vec3(),
            eye: default_eye_vec3(),
            up: default_up_vec3(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrthogonalProjection {
    #[serde(default = "default_width")]
    pub width: f32,
    #[serde(default = "default_height")]
    pub height: f32,
    #[serde(default)]
    pub auto: bool,
}
fn default_width() -> f32 {
    1920.0
}
fn default_height() -> f32 {
    1080.0
}
impl Default for OrthogonalProjection {
    fn default() -> Self {
        Self {
            width: 1920.0,
            height: 1080.0,
            auto: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneGeneral {
    #[serde(default, deserialize_with = "null_as_default")]
    pub orthogonalprojection: OrthogonalProjection,
    #[serde(default = "default_clear_color")]
    pub clearcolor: UserBoundValue<String>,
    #[serde(default)]
    pub clearenabled: Option<bool>,
    #[serde(default)]
    pub cameraparallax: Option<UserBoundValue<bool>>,
    #[serde(default)]
    pub cameraparallaxamount: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub cameraparallaxdelay: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub cameraparallaxmouseinfluence: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub bloom: Option<UserBoundValue<bool>>,
    #[serde(default)]
    pub bloomstrength: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub bloomthreshold: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub ambientcolor: Option<String>,
    #[serde(default)]
    pub skylightcolor: Option<String>,
}
fn default_clear_color() -> UserBoundValue<String> {
    UserBoundValue::Plain("0 0 0".into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneObject {
    #[serde(default)]
    pub id: Option<i32>,
    #[serde(default)]
    pub parent: Option<i32>,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_visible")]
    pub visible: UserBoundValue<bool>,
    #[serde(default = "default_origin")]
    pub origin: UserBoundValue<String>,
    #[serde(default = "default_scale")]
    pub scale: UserBoundValue<String>,
    #[serde(default = "default_angles")]
    pub angles: UserBoundValue<String>,
    #[serde(default)]
    pub size: Option<UserBoundValue<String>>,
    #[serde(default)]
    pub alignment: Option<String>,
    #[serde(default = "default_alpha")]
    pub alpha: UserBoundValue<f32>,
    #[serde(default = "default_brightness")]
    pub brightness: UserBoundValue<f32>,
    #[serde(default = "default_color")]
    pub color: UserBoundValue<String>,
    #[serde(default, rename = "colorBlendMode")]
    pub color_blend_mode: Option<i32>,
    #[serde(default, rename = "parallaxDepth")]
    pub parallax_depth: Option<String>,

    // Layer kind payloads
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub particle: Option<String>,
    #[serde(default)]
    pub sound: Option<Vec<String>>,
    #[serde(default)]
    pub playbackmode: Option<String>,
    #[serde(default)]
    pub volume: Option<UserBoundValue<f32>>,
    #[serde(default)]
    pub mintime: Option<f32>,
    #[serde(default)]
    pub maxtime: Option<f32>,
    #[serde(default)]
    pub text: Option<UserBoundValue<String>>,
    /// Text layers: font size in scene units.
    #[serde(default)]
    pub pointsize: Option<UserBoundValue<f32>>,
    /// Text layers: `systemfont_*` name or a VFS path to a `.ttf`/`.otf`.
    #[serde(default)]
    pub font: Option<String>,
    /// Text layers: `left` / `center` / `right`.
    #[serde(default)]
    pub horizontalalign: Option<String>,
    /// Text layers: `top` / `center` / `bottom`.
    #[serde(default)]
    pub verticalalign: Option<String>,
    /// Text layers: inner margin in scene units.
    #[serde(default)]
    pub padding: Option<UserBoundValue<f32>>,
    #[serde(
        default,
        rename = "animationlayers",
        deserialize_with = "null_as_default"
    )]
    pub animation_layers: Vec<ScenePuppetAnimation>,
    #[serde(default)]
    pub effects: Vec<SceneEffectRef>,
    /// Particle layers: per-instance overrides (`count`, `rate`, `size`, `alpha`, `lifetime`,
    /// `speed`, `colorn`, `brightness`); values may be bound to user properties.
    #[serde(default)]
    pub instanceoverride: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenePuppetAnimation {
    #[serde(default)]
    pub animation: i32,
    #[serde(default = "default_visible")]
    pub visible: UserBoundValue<bool>,
    #[serde(default = "default_alpha")]
    pub blend: UserBoundValue<f32>,
    #[serde(default = "default_alpha")]
    pub rate: UserBoundValue<f32>,
    #[serde(default)]
    pub additive: bool,
}
fn default_visible() -> UserBoundValue<bool> {
    UserBoundValue::Plain(true)
}
fn default_origin() -> UserBoundValue<String> {
    UserBoundValue::Plain("0 0 0".into())
}
fn default_scale() -> UserBoundValue<String> {
    UserBoundValue::Plain("1 1 1".into())
}
fn default_angles() -> UserBoundValue<String> {
    UserBoundValue::Plain("0 0 0".into())
}
fn default_alpha() -> UserBoundValue<f32> {
    UserBoundValue::Plain(1.0)
}
fn default_brightness() -> UserBoundValue<f32> {
    UserBoundValue::Plain(1.0)
}
fn default_color() -> UserBoundValue<String> {
    UserBoundValue::Plain("1 1 1".into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEffectRef {
    pub file: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_visible")]
    pub visible: UserBoundValue<bool>,
    #[serde(default)]
    pub passes: Vec<ScenePassOverride>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScenePassOverride {
    #[serde(default)]
    pub combos: HashMap<String, i32>,
    #[serde(default)]
    pub constantshadervalues: HashMap<String, UserBoundValue<serde_json::Value>>,
    #[serde(default)]
    pub textures: Vec<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelJsonFile {
    #[serde(default)]
    pub material: String,
    #[serde(default)]
    pub puppet: Option<String>,
    #[serde(default)]
    pub width: Option<i32>,
    #[serde(default)]
    pub height: Option<i32>,
    #[serde(default)]
    pub autosize: Option<bool>,
    #[serde(default)]
    pub fullscreen: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectFile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub fbos: Vec<EffectFboSpec>,
    #[serde(default)]
    pub passes: Vec<EffectPassSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectFboSpec {
    pub name: String,
    #[serde(default = "default_scale_u32")]
    pub scale: u32,
    #[serde(default)]
    pub format: String,
}
fn default_scale_u32() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectPassSpec {
    #[serde(default)]
    pub material: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub bind: Vec<EffectBindSlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectBindSlot {
    pub name: String,
    pub index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialFile {
    pub passes: Vec<MaterialPassSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialPassSpec {
    pub shader: String,
    #[serde(default = "default_blending")]
    pub blending: String,
    #[serde(default = "default_disabled")]
    pub depthtest: String,
    #[serde(default = "default_disabled")]
    pub depthwrite: String,
    #[serde(default = "default_cull_nocull")]
    pub cullmode: String,
    #[serde(default)]
    pub textures: Vec<Option<String>>,
    #[serde(default)]
    pub combos: HashMap<String, i32>,
    #[serde(default)]
    pub constantshadervalues: HashMap<String, serde_json::Value>,
}
fn default_blending() -> String {
    "normal".into()
}
fn default_disabled() -> String {
    "disabled".into()
}
fn default_cull_nocull() -> String {
    "nocull".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCENE: &str = r#"{
        "camera": { "center": "0.00000 0.00000 -1.00000", "eye": "0.00000 0.00000 0.00000", "up": "0.00000 1.00000 0.00000" },
        "general": {
            "ambientcolor": "0.30000 0.30000 0.30000",
            "bloom": false,
            "bloomstrength": 2,
            "bloomthreshold": 0.65,
            "camerafade": true,
            "cameraparallax": { "user": "parallax", "value": true },
            "cameraparallaxamount": 0.5,
            "cameraparallaxdelay": 0.1,
            "cameraparallaxmouseinfluence": 0.3,
            "clearcolor": "0.70000 0.70000 0.70000",
            "orthogonalprojection": { "height": 1440, "width": 2560 },
            "skylightcolor": "0.30000 0.30000 0.30000",
            "zoom": 1
        },
        "objects": [
            {
                "id": 12,
                "name": "Background",
                "image": "models/background.json",
                "origin": "1280.00000 720.00000 0.00000",
                "scale": "1.00000 1.00000 1.00000",
                "angles": "0.00000 0.00000 0.00000",
                "size": "2560.00000 1440.00000",
                "alignment": "center",
                "alpha": { "user": "opacity_slider", "value": 1.0 },
                "color": { "user": "tint", "value": "1.00000 1.00000 1.00000" },
                "colorBlendMode": 0,
                "parallaxDepth": "1.00000 1.00000",
                "visible": { "user": { "name": "weather_mode", "condition": "2" }, "value": true },
                "copybackground": true,
                "effects": [
                    {
                        "file": "effects/shake/effect.json",
                        "id": 13,
                        "name": "",
                        "visible": { "user": "enable_shake", "value": false },
                        "passes": [
                            {
                                "id": 14,
                                "combos": { "NOISE": 1 },
                                "constantshadervalues": {
                                    "speed": 2.0,
                                    "strength": { "user": "shake_strength", "value": 0.1 }
                                },
                                "textures": [null, "noise"]
                            }
                        ]
                    }
                ]
            },
            {
                "id": 20,
                "name": "Rain",
                "sound": ["sounds/rain.mp3", "sounds/rain2.mp3"],
                "playbackmode": "random",
                "volume": { "user": "rain_volume", "value": 0.8 },
                "mintime": 0,
                "maxtime": 10,
                "startsilent": false
            },
            {
                "id": 30,
                "name": "Spark",
                "particle": "particles/sparks.json",
                "origin": { "script": "'use strict'; export function update(value) { return value; }", "value": "960 540 0" }
            }
        ],
        "version": 3
    }"#;

    #[test]
    fn parses_realistic_scene_json() {
        let scene = parse_scene_json(SCENE).unwrap();
        assert_eq!(scene.camera.center, "0.00000 0.00000 -1.00000");
        assert_eq!(scene.general.orthogonalprojection.width, 2560.0);
        assert_eq!(scene.general.orthogonalprojection.height, 1440.0);
        assert_eq!(
            scene.general.clearcolor.default_value(),
            "0.70000 0.70000 0.70000"
        );
        assert_eq!(
            scene
                .general
                .bloomstrength
                .as_ref()
                .map(|v| *v.default_value()),
            Some(2.0)
        );
        let parallax = scene.general.cameraparallax.as_ref().unwrap();
        assert_eq!(parallax.user_property_name(), Some("parallax"));
        assert_eq!(scene.objects.len(), 3);

        let bg = &scene.objects[0];
        assert_eq!(bg.id, Some(12));
        assert_eq!(bg.image.as_deref(), Some("models/background.json"));
        assert_eq!(bg.color_blend_mode, Some(0));
        assert_eq!(bg.parallax_depth.as_deref(), Some("1.00000 1.00000"));
        assert_eq!(bg.alignment.as_deref(), Some("center"));
        let fx = &bg.effects[0];
        assert_eq!(fx.file, "effects/shake/effect.json");
        let pass = &fx.passes[0];
        assert_eq!(pass.combos.get("NOISE"), Some(&1));
        assert_eq!(pass.textures, vec![None, Some("noise".to_string())]);
        assert_eq!(
            pass.constantshadervalues["speed"].default_value(),
            &json!(2.0)
        );
        assert_eq!(
            pass.constantshadervalues["strength"].user_property_name(),
            Some("shake_strength")
        );

        let rain = &scene.objects[1];
        assert_eq!(rain.sound.as_ref().unwrap().len(), 2);
        assert_eq!(rain.playbackmode.as_deref(), Some("random"));
        assert_eq!(rain.maxtime, Some(10.0));
        assert_eq!(*rain.volume.as_ref().unwrap().default_value(), 0.8);

        let spark = &scene.objects[2];
        assert_eq!(spark.particle.as_deref(), Some("particles/sparks.json"));
        assert!(spark
            .origin
            .script()
            .unwrap()
            .contains("export function update"));
        assert_eq!(spark.origin.default_value(), "960 540 0");
    }

    #[test]
    fn missing_fields_use_spec_defaults() {
        let scene =
            parse_scene_json(r#"{ "general": {}, "objects": [ { "name": "x" } ] }"#).unwrap();
        assert_eq!(scene.camera.center, "0 0 0");
        assert_eq!(scene.camera.eye, "0 0 1");
        assert_eq!(scene.camera.up, "0 1 0");
        let proj = &scene.general.orthogonalprojection;
        assert_eq!(
            (proj.width, proj.height, proj.auto),
            (1920.0, 1080.0, false)
        );
        assert_eq!(scene.general.clearcolor.default_value(), "0 0 0");
        assert!(scene.general.cameraparallax.is_none());

        let obj = &scene.objects[0];
        assert!(*obj.visible.default_value());
        assert_eq!(obj.origin.default_value(), "0 0 0");
        assert_eq!(obj.scale.default_value(), "1 1 1");
        assert_eq!(obj.angles.default_value(), "0 0 0");
        assert_eq!(*obj.alpha.default_value(), 1.0);
        assert_eq!(*obj.brightness.default_value(), 1.0);
        assert_eq!(obj.color.default_value(), "1 1 1");
        assert!(obj.effects.is_empty() && obj.sound.is_none() && obj.image.is_none());

        let null_proj =
            parse_scene_json(r#"{ "general": { "orthogonalprojection": null } }"#).unwrap();
        assert_eq!(null_proj.general.orthogonalprojection.width, 1920.0);
        let auto_proj =
            parse_scene_json(r#"{ "general": { "orthogonalprojection": { "auto": true } } }"#)
                .unwrap();
        assert!(auto_proj.general.orthogonalprojection.auto);
        assert_eq!(auto_proj.general.orthogonalprojection.height, 1080.0);
    }

    #[test]
    fn unknown_fields_are_tolerated() {
        let scene = parse_scene_json(
            r#"{ "general": { "futureflag": [1, 2], "zoom": 2 }, "extra": { "a": 1 },
                 "objects": [ { "name": "x", "instanceoverride": { "id": 1 }, "locktransforms": true } ] }"#,
        )
        .unwrap();
        assert_eq!(scene.objects[0].name, "x");
    }

    #[test]
    fn bound_bool_with_and_without_user_override() {
        let bound: UserBoundValue<bool> =
            serde_json::from_value(json!({ "user": "enable_shake", "value": false })).unwrap();
        let empty = UserPropertyMap::new();
        assert!(!resolve_bound_bool(&bound, &empty));

        let props = parse_user_properties(r#"{ "enable_shake": true }"#).unwrap();
        assert!(resolve_bound_bool(&bound, &props));

        // Full project.json property object form.
        let props =
            parse_user_properties(r#"{ "enable_shake": { "type": "bool", "value": true } }"#)
                .unwrap();
        assert!(resolve_bound_bool(&bound, &props));

        let plain: UserBoundValue<bool> = serde_json::from_value(json!(false)).unwrap();
        assert!(!resolve_bound_bool(&plain, &props));

        let no_user: UserBoundValue<bool> =
            serde_json::from_value(json!({ "value": true })).unwrap();
        assert!(resolve_bound_bool(&no_user, &props));
    }

    #[test]
    fn conditional_visibility_binding() {
        let bound: UserBoundValue<bool> = serde_json::from_value(
            json!({ "user": { "name": "weather_mode", "condition": "2" }, "value": true }),
        )
        .unwrap();
        assert!(resolve_bound_bool(&bound, &UserPropertyMap::new()));
        let rain = parse_user_properties(r#"{ "weather_mode": 2 }"#).unwrap();
        assert!(resolve_bound_bool(&bound, &rain));
        let rain_str = parse_user_properties(r#"{ "weather_mode": "2" }"#).unwrap();
        assert!(resolve_bound_bool(&bound, &rain_str));
        let snow = parse_user_properties(r#"{ "weather_mode": 1 }"#).unwrap();
        assert!(!resolve_bound_bool(&bound, &snow));

        let no_cond: UserBoundValue<bool> =
            serde_json::from_value(json!({ "user": { "name": "flag" }, "value": true })).unwrap();
        let off = parse_user_properties(r#"{ "flag": false }"#).unwrap();
        assert!(!resolve_bound_bool(&no_cond, &off));
    }

    #[test]
    fn bound_values_resolve_against_user_properties() {
        let alpha: UserBoundValue<f32> =
            serde_json::from_value(json!({ "user": "opacity_slider", "value": 1.0 })).unwrap();
        assert_eq!(resolve_bound_value(&alpha, &UserPropertyMap::new()), 1.0);
        let props =
            parse_user_properties(r#"{ "opacity_slider": 0.25, "tint": "1 0 0", "bad": [] }"#)
                .unwrap();
        assert_eq!(resolve_bound_value(&alpha, &props), 0.25);
        let numeric_string = parse_user_properties(r#"{ "opacity_slider": "0.5" }"#).unwrap();
        assert_eq!(resolve_bound_value(&alpha, &numeric_string), 0.5);

        let tint: UserBoundValue<String> =
            serde_json::from_value(json!({ "user": "tint", "value": "1 1 1" })).unwrap();
        assert_eq!(resolve_bound_value(&tint, &props), "1 0 0");

        let mismatched: UserBoundValue<f32> =
            serde_json::from_value(json!({ "user": "bad", "value": 0.7 })).unwrap();
        assert_eq!(resolve_bound_value(&mismatched, &props), 0.7);

        let plain: UserBoundValue<f32> = serde_json::from_value(json!(0.3)).unwrap();
        assert_eq!(resolve_bound_value(&plain, &props), 0.3);

        let scripted: UserBoundValue<String> =
            serde_json::from_value(json!({ "script": "x", "value": "960 540 0" })).unwrap();
        assert_eq!(resolve_bound_value(&scripted, &props), "960 540 0");
    }

    #[test]
    fn vector_strings_parse_with_defaults() {
        assert_eq!(
            parse_vec_components("1.00000 -2.5 3e1"),
            vec![1.0, -2.5, 30.0]
        );
        assert_eq!(parse_vec_components(" 1,2 , 3 "), vec![1.0, 2.0, 3.0]);
        assert_eq!(
            parse_vec3_or("960.00000 540.00000 0.00000", Vec3::ZERO),
            Vec3::new(960.0, 540.0, 0.0)
        );
        assert_eq!(
            parse_vec2_or("2560 1440", Vec2::ZERO),
            Vec2::new(2560.0, 1440.0)
        );
        assert_eq!(parse_vec3_or("", Vec3::ONE), Vec3::ONE);
        assert_eq!(parse_vec3_or("2", Vec3::ONE), Vec3::new(2.0, 1.0, 1.0));
        assert_eq!(
            parse_vec4_or("0.5 0.25 1", Vec4::ONE),
            Vec4::new(0.5, 0.25, 1.0, 1.0)
        );
        assert_eq!(parse_vec2_or("1 2 3 4", Vec2::ZERO), Vec2::new(1.0, 2.0));
    }

    #[test]
    fn parses_effect_material_and_model_files() {
        let effect: EffectFile = serde_json::from_value(json!({
            "name": "blur",
            "fbos": [ { "name": "_rt_HalfCompoBuffer1", "scale": 2, "format": "rgba8888" }, { "name": "_rt_Q" } ],
            "passes": [
                { "material": "materials/effects/blur_h.json", "target": "_rt_HalfCompoBuffer1" },
                { "command": "copy", "source": "_rt_HalfCompoBuffer1", "target": "_rt_Q" },
                { "material": "materials/effects/blur_v.json",
                  "bind": [ { "name": "_rt_HalfCompoBuffer1", "index": 0 }, { "name": "_rt_FullFrameBuffer", "index": 1 } ] }
            ]
        }))
        .unwrap();
        assert_eq!(effect.fbos[0].scale, 2);
        assert_eq!(effect.fbos[0].format, "rgba8888");
        assert_eq!(effect.fbos[1].scale, 1);
        assert_eq!(effect.fbos[1].format, "");
        assert_eq!(effect.passes[1].command.as_deref(), Some("copy"));
        assert_eq!(effect.passes[2].bind[1].name, "_rt_FullFrameBuffer");
        assert_eq!(effect.passes[2].bind[1].index, 1);

        let material: MaterialFile = serde_json::from_value(json!({
            "passes": [ { "shader": "genericimage2", "textures": [ "bg", null ], "combos": { "ALPHA": 1 },
                          "constantshadervalues": { "g_Speed": 1.5 } } ]
        }))
        .unwrap();
        let p = &material.passes[0];
        assert_eq!(p.shader, "genericimage2");
        assert_eq!(
            (
                p.blending.as_str(),
                p.depthtest.as_str(),
                p.depthwrite.as_str(),
                p.cullmode.as_str()
            ),
            ("normal", "disabled", "disabled", "nocull")
        );
        assert_eq!(p.textures, vec![Some("bg".to_string()), None]);

        let model: ModelJsonFile = serde_json::from_value(
            json!({ "material": "materials/bg.json", "puppet": "models/bg.mdl", "autosize": true }),
        )
        .unwrap();
        assert_eq!(model.puppet.as_deref(), Some("models/bg.mdl"));
        assert_eq!(model.width, None);
    }

    #[test]
    fn scene_round_trips_through_serde() {
        let scene = parse_scene_json(SCENE).unwrap();
        let text = serde_json::to_string(&scene).unwrap();
        let again = parse_scene_json(&text).unwrap();
        assert_eq!(again.objects.len(), scene.objects.len());
        assert_eq!(
            again.objects[0].alpha.user_property_name(),
            Some("opacity_slider")
        );
    }

    #[test]
    fn condition_matching_handles_bools_numbers_and_strings() {
        use serde_json::json;
        assert!(condition_matches(&json!(true), "1"));
        assert!(condition_matches(&json!(false), "0"));
        assert!(!condition_matches(&json!(false), "1"));
        assert!(condition_matches(&json!(2), "2"));
        assert!(condition_matches(&json!(2.0), "2"));
        assert!(condition_matches(&json!("2"), "2"));
        assert!(!condition_matches(&json!("3"), "2"));
    }

    #[test]
    fn json_bindings_resolve_with_defaults_and_conditions() {
        use serde_json::json;
        let mut props = UserPropertyMap::new();
        assert_eq!(
            resolve_json_binding(&json!({"user": "a", "value": 1}), &props),
            json!(1)
        );
        props.insert("a".into(), json!({"type": "slider", "value": 7}));
        props.insert("m".into(), json!("2"));
        assert_eq!(
            resolve_json_binding(&json!({"user": "a", "value": 1}), &props),
            json!(7)
        );
        let cond = json!({"user": {"name": "m", "condition": "2"}, "value": false});
        assert_eq!(resolve_json_binding(&cond, &props), json!(true));
        assert_eq!(
            resolve_json_binding(&json!("plain"), &props),
            json!("plain")
        );
    }
}
