//! `scene.json` scene-graph loader (`docs/05` Phase 6, `docs/03` §7.1 and §7.2).
//!
//! [`SceneGraph::load`] reads `scene.json` through the 4-layer VFS (pitfall #6), parses it with
//! `truongpaper_we_formats::parse_scene_json`, resolves every user-bound value against the
//! `project.json` user properties and builds one [`SceneLayer`] per drawable/audible object in
//! `objects` order. Everything here is device-free.
//!
//! Notes on behavior the docs do not pin down:
//! - `general.orthogonalprojection` absent or `null` selects a perspective camera
//!   (`fov` 50, `nearz` 0.01, `farz` 10000 by default). These fields are read from the raw JSON
//!   because the we-formats schema has no slot for them.
//! - Camera parallax: the doc formula `(pointer - 0.5) * amount * influence` is treated as a
//!   fraction of the scene size, so the eye offset in scene units is
//!   `offset * layer.parallax_depth * scene_size`. `parallaxDepth` defaults to `"0 0"` (the layer
//!   does not move), as in linux-wallpaperengine.
//! - Camera shake: Wallpaper Engine does not document its shake curve. [`camera_shake_offset`]
//!   is a deterministic sum-of-sines approximation in scene units; the default amplitude,
//!   roughness and speed (0.5, 1.0, 3.0) are assumptions.
//! - `general.zoom` scales the orthographic view around the scene center; it is ignored for the
//!   perspective camera.
//! - Objects without an image/particle/sound/text payload are kept as transform-only nodes so
//!   they still work as parents, but they are not exposed as layers.

use std::collections::{HashMap, HashSet};

use glam::{Mat4, Vec2, Vec3};
use truongpaper_we_formats::{
    parse_scene_json, parse_tex, parse_user_properties, parse_vec2_or, parse_vec3_or,
    parse_vec_components, resolve_bound_bool, resolve_bound_value, SceneEffectRef, SceneFile,
    SceneObject, UserBoundValue, UserConditionSpec, UserPropertyMap, WeVirtualFileSystem,
};

use crate::json::Value;
use crate::particles::{ParticleFile, ParticleSystemInstance};
use crate::scene_assets::{material_texture_path, MaterialFile, ModelJsonFile};
use crate::scene_math::{
    build_pass_matrices, layer_model_matrix, parallax_eye_offset, scene_orthographic_projection,
    LayerTransform, PassMatrices, DEFAULT_SCENE_SIZE,
};

pub const DEFAULT_PERSPECTIVE_FOV_DEGREES: f32 = 50.0;
pub const DEFAULT_PERSPECTIVE_NEARZ: f32 = 0.01;
pub const DEFAULT_PERSPECTIVE_FARZ: f32 = 10000.0;
pub const DEFAULT_PARALLAX_AMOUNT: f32 = 0.5;
pub const DEFAULT_PARALLAX_DELAY: f32 = 0.1;
pub const DEFAULT_PARALLAX_MOUSE_INFLUENCE: f32 = 0.5;
pub const DEFAULT_SHAKE_AMPLITUDE: f32 = 0.5;
pub const DEFAULT_SHAKE_ROUGHNESS: f32 = 1.0;
pub const DEFAULT_SHAKE_SPEED: f32 = 3.0;

/// Model paths that always denote a composition layer.
const COMPOSITION_MODEL_PATHS: [&str; 3] = [
    "models/util/composelayer.json",
    "models/util/fullscreenlayer.json",
    "models/util/projectlayer.json",
];
const SOLID_MODEL_PATH: &str = "models/util/solidlayer.json";

#[derive(Debug, thiserror::Error)]
pub enum SceneLoadError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid scene.json {path}: {message}")]
    Scene { path: String, message: String },
    #[error("invalid user properties JSON: {0}")]
    UserProperties(String),
}

/// Camera projection selected from `scene.general`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SceneProjection {
    Orthographic {
        width: f32,
        height: f32,
    },
    Perspective {
        fov_degrees: f32,
        aspect: f32,
        nearz: f32,
        farz: f32,
        eye: Vec3,
        center: Vec3,
        up: Vec3,
    },
}

impl SceneProjection {
    pub fn is_perspective(&self) -> bool {
        matches!(self, SceneProjection::Perspective { .. })
    }

    /// Projection matrix (perspective includes the `look_at_rh` view).
    pub fn matrix(&self) -> Mat4 {
        match *self {
            SceneProjection::Orthographic { width, height } => {
                scene_orthographic_projection(width, height)
            }
            SceneProjection::Perspective {
                fov_degrees,
                aspect,
                nearz,
                farz,
                eye,
                center,
                up,
            } => {
                Mat4::perspective_rh(fov_degrees.to_radians(), aspect, nearz, farz)
                    * Mat4::look_at_rh(eye, center, up)
            }
        }
    }
}

/// Camera settings resolved from `scene.general` and `scene.camera`.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraRuntime {
    pub projection: SceneProjection,
    pub zoom: f32,
    pub parallax_enabled: bool,
    pub parallax_amount: f32,
    pub parallax_delay: f32,
    pub parallax_mouse_influence: f32,
    pub shake_enabled: bool,
    pub shake_amplitude: f32,
    pub shake_roughness: f32,
    pub shake_speed: f32,
}

impl Default for CameraRuntime {
    fn default() -> Self {
        Self {
            projection: SceneProjection::Orthographic {
                width: DEFAULT_SCENE_SIZE.x,
                height: DEFAULT_SCENE_SIZE.y,
            },
            zoom: 1.0,
            parallax_enabled: false,
            parallax_amount: DEFAULT_PARALLAX_AMOUNT,
            parallax_delay: DEFAULT_PARALLAX_DELAY,
            parallax_mouse_influence: DEFAULT_PARALLAX_MOUSE_INFLUENCE,
            shake_enabled: false,
            shake_amplitude: DEFAULT_SHAKE_AMPLITUDE,
            shake_roughness: DEFAULT_SHAKE_ROUGHNESS,
            shake_speed: DEFAULT_SHAKE_SPEED,
        }
    }
}

/// Per-frame camera motion (parallax smoothing and shake).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraState {
    /// Seconds accumulated by [`CameraState::step`].
    pub time: f32,
    /// Last pointer position, `[0,1]` on the monitor.
    pub pointer: Vec2,
    /// Unsmoothed parallax offset (fraction of the scene size).
    pub parallax_target: Vec2,
    /// Smoothed parallax offset (fraction of the scene size).
    pub parallax_offset: Vec2,
    /// Shake offset in scene units.
    pub shake_offset: Vec2,
}

impl Default for CameraState {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraState {
    pub fn new() -> Self {
        Self {
            time: 0.0,
            pointer: Vec2::splat(0.5),
            parallax_target: Vec2::ZERO,
            parallax_offset: Vec2::ZERO,
            shake_offset: Vec2::ZERO,
        }
    }

    /// Advances the camera by `dt` seconds. `pointer` is the cursor in `[0,1]` on the monitor.
    ///
    /// Parallax follows `docs/03` §7.2 item 2: the target is
    /// `parallax_eye_offset(pointer, amount, influence)` and the smoothed offset approaches it
    /// exponentially with time constant `cameraparallaxdelay` (`<= 0` jumps immediately).
    pub fn step(&mut self, dt: f32, pointer: Vec2, runtime: &CameraRuntime) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.time += dt;
        if pointer.is_finite() {
            self.pointer = pointer;
        }
        self.parallax_target = if runtime.parallax_enabled {
            parallax_eye_offset(
                self.pointer,
                runtime.parallax_amount,
                runtime.parallax_mouse_influence,
            )
        } else {
            Vec2::ZERO
        };
        if runtime.parallax_delay <= 0.0 || !runtime.parallax_delay.is_finite() {
            self.parallax_offset = self.parallax_target;
        } else {
            let k = 1.0 - (-dt / runtime.parallax_delay).exp();
            self.parallax_offset += (self.parallax_target - self.parallax_offset) * k;
        }
        self.shake_offset = if runtime.shake_enabled {
            camera_shake_offset(
                self.time,
                runtime.shake_amplitude,
                runtime.shake_roughness,
                runtime.shake_speed,
            )
        } else {
            Vec2::ZERO
        };
    }

    /// Eye offset in scene units for a layer with `parallax_depth`.
    pub fn eye_offset(&self, parallax_depth: Vec2, scene_size: Vec2) -> Vec2 {
        self.parallax_offset * parallax_depth * scene_size + self.shake_offset
    }
}

/// Deterministic camera shake approximation (scene units):
/// `amplitude * mix((sin(t*s*1.0), cos(t*s*1.3)), (sin(t*s*3.7), cos(t*s*4.3)), roughness)`.
/// `roughness` is clamped to `[0,1]` and blends in the higher-frequency term.
pub fn camera_shake_offset(time: f32, amplitude: f32, roughness: f32, speed: f32) -> Vec2 {
    let ts = time * speed;
    let base = Vec2::new((ts * 1.0).sin(), (ts * 1.3).cos());
    let high = Vec2::new((ts * 3.7).sin(), (ts * 4.3).cos());
    let r = roughness.clamp(0.0, 1.0);
    (base * (1.0 - r) + high * r) * amplitude
}

/// What a layer draws or plays.
#[derive(Debug, Clone)]
pub enum SceneLayerKind {
    Image {
        model_path: String,
        model: ModelJsonFile,
        material: Option<MaterialFile>,
        /// VFS paths per material texture slot (`"bg"` -> `"materials/bg.tex"`; `_rt_*`
        /// render-target names are kept verbatim; `None` for empty slots).
        texture_paths: Vec<Option<String>>,
        puppet_path: Option<String>,
    },
    Composition {
        model_path: String,
        model: ModelJsonFile,
        material: Option<MaterialFile>,
    },
    Solid {
        color: Vec3,
    },
    Particle {
        path: String,
        spec: ParticleFile,
    },
    Sound {
        paths: Vec<String>,
        playbackmode: String,
        volume: f32,
        mintime: f32,
        maxtime: f32,
    },
    /// Kept for SceneScript `thisLayer` access, not drawn.
    Text {
        text: String,
    },
}

impl SceneLayerKind {
    pub fn name(&self) -> &'static str {
        match self {
            SceneLayerKind::Image { .. } => "image",
            SceneLayerKind::Composition { .. } => "composition",
            SceneLayerKind::Solid { .. } => "solid",
            SceneLayerKind::Particle { .. } => "particle",
            SceneLayerKind::Sound { .. } => "sound",
            SceneLayerKind::Text { .. } => "text",
        }
    }

    /// `material.passes[0]` of image and composition layers.
    pub fn material(&self) -> Option<&MaterialFile> {
        match self {
            SceneLayerKind::Image { material, .. }
            | SceneLayerKind::Composition { material, .. } => material.as_ref(),
            _ => None,
        }
    }
}

/// One resolved scene layer.
#[derive(Debug, Clone)]
pub struct SceneLayer {
    /// Index into `scene.objects`.
    pub object_index: usize,
    pub id: Option<i32>,
    pub name: String,
    pub kind: SceneLayerKind,
    pub visible: bool,
    pub alpha: f32,
    pub brightness: f32,
    pub color: Vec3,
    /// Local transform (relative to the parent).
    pub transform: LayerTransform,
    /// `parent_world * local`.
    pub world_matrix: Mat4,
    /// `scene.objects` index of the resolved parent (cycles and missing parents -> `None`).
    pub parent_object: Option<usize>,
    pub parallax_depth: Vec2,
    pub color_blend_mode: i32,
    pub effects: Vec<SceneEffectRef>,
    /// Size used when the object has no `size` (model width/height or texture size).
    pub fallback_size: Vec2,
}

impl SceneLayer {
    /// `"<name>"`, or the id when the name is empty, or `"#<object index>"`.
    pub fn key(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else if let Some(id) = self.id {
            id.to_string()
        } else {
            format!("#{}", self.object_index)
        }
    }
}

/// Layer properties a SceneScript can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScriptProperty {
    Origin,
    Scale,
    Angles,
    Alpha,
    Visible,
    Color,
    Size,
    Brightness,
}

impl ScriptProperty {
    pub const ALL: [ScriptProperty; 8] = [
        ScriptProperty::Origin,
        ScriptProperty::Scale,
        ScriptProperty::Angles,
        ScriptProperty::Alpha,
        ScriptProperty::Visible,
        ScriptProperty::Color,
        ScriptProperty::Size,
        ScriptProperty::Brightness,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ScriptProperty::Origin => "origin",
            ScriptProperty::Scale => "scale",
            ScriptProperty::Angles => "angles",
            ScriptProperty::Alpha => "alpha",
            ScriptProperty::Visible => "visible",
            ScriptProperty::Color => "color",
            ScriptProperty::Size => "size",
            ScriptProperty::Brightness => "brightness",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == name)
    }
}

/// A scripted property (`docs/03` §7.1 Form D) for the SceneScript engine.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyScriptSlot {
    /// `"<layer name or id>/<property>"`.
    pub slot_key: String,
    pub layer_name: String,
    pub property: String,
    pub script: String,
    /// The scene's `value` as a string (`"960 540 0"`, `"1"`, `"true"`).
    pub initial_value: String,
}

#[derive(Debug, Clone)]
struct SceneNode {
    transform: LayerTransform,
    parent: Option<usize>,
    world: Mat4,
    layer: Option<usize>,
    fallback_size: Vec2,
}

/// Loaded scene graph.
pub struct SceneGraph {
    pub scene_path: String,
    pub scene: SceneFile,
    /// Raw `scene.general` (fields the we-formats schema lacks).
    pub general_raw: Value,
    pub user_props: UserPropertyMap,
    pub monitor_size: Vec2,
    /// Scene units (orthographic width/height, or the monitor size for `auto`/perspective).
    pub scene_size: Vec2,
    pub camera: CameraRuntime,
    pub clear_color: Vec3,
    pub clear_enabled: bool,
    pub layers: Vec<SceneLayer>,
    /// Non-fatal problems found while loading (missing assets, bad parents, ...).
    pub warnings: Vec<String>,
    nodes: Vec<SceneNode>,
    particles: Vec<(usize, ParticleSystemInstance)>,
}

impl std::fmt::Debug for SceneGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneGraph")
            .field("scene_path", &self.scene_path)
            .field("scene_size", &self.scene_size)
            .field("camera", &self.camera)
            .field("layers", &self.layers)
            .field("particle_systems", &self.particles.len())
            .finish_non_exhaustive()
    }
}

fn raw_number(v: &Value, key: &str) -> Option<f32> {
    match v.get(key)? {
        Value::Number(n) => Some(*n as f32),
        Value::String(s) => s.trim().parse().ok(),
        Value::Object(_) => raw_number(v.get(key)?, "value"),
        _ => None,
    }
}

fn value_to_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => Some(*n != 0.0),
        _ => None,
    }
}

fn value_to_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(*n as f32),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Builds a we-formats `UserBoundValue` from raw JSON (Form A/B/C) for fields the schema lacks.
fn raw_bound<T>(v: Option<&Value>, conv: fn(&Value) -> Option<T>) -> Option<UserBoundValue<T>> {
    let v = v?;
    if let Some(plain) = conv(v) {
        return Some(UserBoundValue::Plain(plain));
    }
    let value = conv(v.get("value")?)?;
    let user = match v.get("user") {
        Some(Value::String(name)) => Some(UserConditionSpec::PropertyName(name.clone())),
        Some(obj @ Value::Object(_)) => {
            obj.get("name")
                .and_then(Value::as_str)
                .map(|name| UserConditionSpec::Conditional {
                    name: name.to_string(),
                    condition: obj.get("condition").map(|c| match c {
                        Value::String(s) => s.clone(),
                        other => other.to_json_string(),
                    }),
                })
        }
        _ => None,
    };
    Some(UserBoundValue::Bound {
        user,
        script: None,
        scriptproperties: None,
        value,
    })
}

fn opt_bool(b: &Option<UserBoundValue<bool>>, props: &UserPropertyMap, default: bool) -> bool {
    b.as_ref()
        .map(|b| resolve_bound_bool(b, props))
        .unwrap_or(default)
}

fn opt_f32(b: &Option<UserBoundValue<f32>>, props: &UserPropertyMap, default: f32) -> f32 {
    b.as_ref()
        .map(|b| resolve_bound_value(b, props))
        .unwrap_or(default)
}

fn normalize_path(p: &str) -> String {
    p.trim()
        .trim_start_matches(['/', '\\'])
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn fmt_vec3(v: Vec3) -> String {
    format!("{} {} {}", v.x, v.y, v.z)
}

fn fmt_vec2(v: Vec2) -> String {
    format!("{} {}", v.x, v.y)
}

/// Parses a stringified JS value into vector components: `"x y z"`, `"x,y,z"`, a JSON array, or
/// a JSON object with `x`/`y`/`z` keys (what `JSON.stringify(Vec3)` produces).
fn parse_script_components(raw: &str) -> Vec<f32> {
    let t = raw.trim();
    if t.starts_with('{') || t.starts_with('[') {
        if let Ok(v) = Value::parse(t) {
            if let Some(arr) = v.as_array() {
                return arr
                    .iter()
                    .filter_map(Value::as_f64)
                    .map(|n| n as f32)
                    .collect();
            }
            let mut out = Vec::new();
            for key in ["x", "y", "z", "w"] {
                match v.get(key).and_then(Value::as_f64) {
                    Some(n) => out.push(n as f32),
                    None => break,
                }
            }
            return out;
        }
    }
    parse_vec_components(t.trim_matches('"'))
}

fn apply_components3(current: Vec3, comps: &[f32]) -> Vec3 {
    let mut out = current.to_array();
    for (slot, c) in out.iter_mut().zip(comps) {
        *slot = *c;
    }
    Vec3::from_array(out)
}

fn apply_components2(current: Vec2, comps: &[f32]) -> Vec2 {
    let mut out = current.to_array();
    for (slot, c) in out.iter_mut().zip(comps) {
        *slot = *c;
    }
    Vec2::from_array(out)
}

fn resolve_transform(
    obj: &SceneObject,
    props: &UserPropertyMap,
    fallback_size: Vec2,
) -> LayerTransform {
    LayerTransform {
        origin: parse_vec3_or(&resolve_bound_value(&obj.origin, props), Vec3::ZERO),
        scale: parse_vec3_or(&resolve_bound_value(&obj.scale, props), Vec3::ONE),
        angles: parse_vec3_or(&resolve_bound_value(&obj.angles, props), Vec3::ZERO),
        size: obj
            .size
            .as_ref()
            .map(|s| parse_vec2_or(&resolve_bound_value(s, props), fallback_size))
            .unwrap_or(fallback_size),
        alignment: obj.alignment.clone().unwrap_or_else(|| "center".into()),
    }
}

/// Output of [`load_layer_kind`]: the kind plus the size fallback for objects without `size`.
type LoadedKind = (SceneLayerKind, Vec2);

impl SceneGraph {
    /// Loads `scene_path` (normally `"scene.json"`) through `vfs`.
    ///
    /// `user_props_json` is a JSON object of current `project.json` user-property values (plain
    /// values or full property objects). `monitor_size` is used for `orthogonalprojection.auto`
    /// and perspective scenes. Missing assets are recorded in [`SceneGraph::warnings`] and the
    /// affected layer is skipped.
    pub fn load(
        vfs: &WeVirtualFileSystem,
        scene_path: &str,
        user_props_json: Option<&str>,
        monitor_size: Vec2,
    ) -> Result<SceneGraph, SceneLoadError> {
        let text = vfs
            .read_string(scene_path)
            .map_err(|source| SceneLoadError::Io {
                path: scene_path.to_string(),
                source,
            })?;
        let scene = parse_scene_json(&text).map_err(|e| SceneLoadError::Scene {
            path: scene_path.to_string(),
            message: e.to_string(),
        })?;
        let user_props = match user_props_json.map(str::trim).filter(|s| !s.is_empty()) {
            Some(json) => parse_user_properties(json)
                .map_err(|e| SceneLoadError::UserProperties(e.to_string()))?,
            None => UserPropertyMap::new(),
        };

        let mut warnings = Vec::new();
        let raw = match Value::parse(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                let msg = format!("{scene_path}: raw JSON re-parse failed ({e}); using defaults");
                tracing::warn!("{msg}");
                warnings.push(msg);
                None
            }
        };
        let general_raw = raw
            .as_ref()
            .and_then(|r| r.get("general"))
            .cloned()
            .unwrap_or_default();

        let mut graph = SceneGraph {
            scene_path: scene_path.to_string(),
            scene,
            general_raw,
            user_props,
            monitor_size,
            scene_size: DEFAULT_SCENE_SIZE,
            camera: CameraRuntime::default(),
            clear_color: Vec3::ZERO,
            clear_enabled: true,
            layers: Vec::new(),
            warnings,
            nodes: Vec::new(),
            particles: Vec::new(),
        };
        let perspective = raw.is_some()
            && graph
                .general_raw
                .get("orthogonalprojection")
                .is_none_or(Value::is_null);
        graph.resolve_general(perspective);
        graph.build_nodes(vfs);
        Ok(graph)
    }

    fn resolve_general(&mut self, perspective: bool) {
        let props = &self.user_props;
        let general = &self.scene.general;
        let raw = &self.general_raw;
        let monitor_ok = self.monitor_size.x > 0.0 && self.monitor_size.y > 0.0;

        self.scene_size = if perspective {
            if monitor_ok {
                self.monitor_size
            } else {
                DEFAULT_SCENE_SIZE
            }
        } else if general.orthogonalprojection.auto && monitor_ok {
            self.monitor_size
        } else {
            let p = &general.orthogonalprojection;
            if p.width > 0.0 && p.height > 0.0 {
                Vec2::new(p.width, p.height)
            } else {
                DEFAULT_SCENE_SIZE
            }
        };

        let projection = if perspective {
            let cam = &self.scene.camera;
            SceneProjection::Perspective {
                fov_degrees: raw_number(raw, "fov").unwrap_or(DEFAULT_PERSPECTIVE_FOV_DEGREES),
                aspect: self.scene_size.x / self.scene_size.y.max(1.0),
                nearz: raw_number(raw, "nearz").unwrap_or(DEFAULT_PERSPECTIVE_NEARZ),
                farz: raw_number(raw, "farz").unwrap_or(DEFAULT_PERSPECTIVE_FARZ),
                eye: parse_vec3_or(&cam.eye, Vec3::new(0.0, 0.0, 1.0)),
                center: parse_vec3_or(&cam.center, Vec3::ZERO),
                up: parse_vec3_or(&cam.up, Vec3::Y),
            }
        } else {
            SceneProjection::Orthographic {
                width: self.scene_size.x,
                height: self.scene_size.y,
            }
        };

        let shake = raw_bound(raw.get("camerashake"), value_to_bool);
        let shake_amp = raw_bound(raw.get("camerashakeamplitude"), value_to_f32);
        let shake_rough = raw_bound(raw.get("camerashakeroughness"), value_to_f32);
        let shake_speed = raw_bound(raw.get("camerashakespeed"), value_to_f32);
        let zoom = raw_bound(raw.get("zoom"), value_to_f32);

        self.camera = CameraRuntime {
            projection,
            zoom: opt_f32(&zoom, props, 1.0),
            parallax_enabled: opt_bool(&general.cameraparallax, props, false),
            parallax_amount: opt_f32(
                &general.cameraparallaxamount,
                props,
                DEFAULT_PARALLAX_AMOUNT,
            ),
            parallax_delay: opt_f32(&general.cameraparallaxdelay, props, DEFAULT_PARALLAX_DELAY),
            parallax_mouse_influence: opt_f32(
                &general.cameraparallaxmouseinfluence,
                props,
                DEFAULT_PARALLAX_MOUSE_INFLUENCE,
            ),
            shake_enabled: opt_bool(&shake, props, false),
            shake_amplitude: opt_f32(&shake_amp, props, DEFAULT_SHAKE_AMPLITUDE),
            shake_roughness: opt_f32(&shake_rough, props, DEFAULT_SHAKE_ROUGHNESS),
            shake_speed: opt_f32(&shake_speed, props, DEFAULT_SHAKE_SPEED),
        };
        self.clear_color =
            parse_vec3_or(&resolve_bound_value(&general.clearcolor, props), Vec3::ZERO);
        self.clear_enabled = general.clearenabled.unwrap_or(true);
    }

    fn warn(&mut self, msg: String) {
        tracing::warn!("{msg}");
        self.warnings.push(msg);
    }

    fn build_nodes(&mut self, vfs: &WeVirtualFileSystem) {
        let objects = self.scene.objects.clone();
        let mut id_to_index = HashMap::new();
        for (i, obj) in objects.iter().enumerate() {
            if let Some(id) = obj.id {
                id_to_index.entry(id).or_insert(i);
            }
        }

        // Parents by id; missing parents become roots.
        let mut parents: Vec<Option<usize>> = Vec::with_capacity(objects.len());
        for (i, obj) in objects.iter().enumerate() {
            let parent = match obj.parent {
                Some(pid) => match id_to_index.get(&pid) {
                    Some(&p) if p != i => Some(p),
                    Some(_) => {
                        self.warn(format!("object {i} ({}) is its own parent", obj.name));
                        None
                    }
                    None => {
                        self.warn(format!(
                            "object {i} ({}) has missing parent id {pid}; treated as root",
                            obj.name
                        ));
                        None
                    }
                },
                None => None,
            };
            parents.push(parent);
        }
        // Break cycles.
        for i in 0..parents.len() {
            let mut seen = HashSet::from([i]);
            let mut cur = parents[i];
            while let Some(p) = cur {
                if !seen.insert(p) {
                    let name = objects[i].name.clone();
                    self.warn(format!(
                        "object {i} ({name}) is part of a parent cycle; treated as root"
                    ));
                    parents[i] = None;
                    break;
                }
                cur = parents[p];
            }
        }

        let mut warned_text = false;
        for (i, obj) in objects.iter().enumerate() {
            let loaded = self.load_layer_kind(vfs, obj, &mut warned_text);
            let fallback_size = loaded.as_ref().map(|l| l.1).unwrap_or(Vec2::ZERO);
            let transform = resolve_transform(obj, &self.user_props, fallback_size);
            let mut node = SceneNode {
                transform: transform.clone(),
                parent: parents[i],
                world: Mat4::IDENTITY,
                layer: None,
                fallback_size,
            };
            if let Some((kind, _)) = loaded {
                let layer_index = self.layers.len();
                node.layer = Some(layer_index);
                if let SceneLayerKind::Particle { spec, .. } = &kind {
                    self.particles
                        .push((layer_index, ParticleSystemInstance::new(spec.clone())));
                }
                self.layers.push(SceneLayer {
                    object_index: i,
                    id: obj.id,
                    name: obj.name.clone(),
                    kind,
                    visible: true,
                    alpha: 1.0,
                    brightness: 1.0,
                    color: Vec3::ONE,
                    transform,
                    world_matrix: Mat4::IDENTITY,
                    parent_object: parents[i],
                    parallax_depth: obj
                        .parallax_depth
                        .as_deref()
                        .map(|d| parse_vec2_or(d, Vec2::ZERO))
                        .unwrap_or(Vec2::ZERO),
                    color_blend_mode: obj.color_blend_mode.unwrap_or(0),
                    effects: obj.effects.clone(),
                    fallback_size,
                });
            }
            self.nodes.push(node);
        }
        for li in 0..self.layers.len() {
            self.resolve_layer_values(li);
        }
        self.recompute_world();
    }

    fn load_layer_kind(
        &mut self,
        vfs: &WeVirtualFileSystem,
        obj: &SceneObject,
        warned_text: &mut bool,
    ) -> Option<LoadedKind> {
        let label = if obj.name.is_empty() {
            obj.id.map(|i| i.to_string()).unwrap_or_default()
        } else {
            obj.name.clone()
        };
        if let Some(model_path) = obj.image.as_deref() {
            return match self.load_model_kind(vfs, model_path) {
                Ok(k) => Some(k),
                Err(e) => {
                    self.warn(format!("layer '{label}' skipped: {e}"));
                    None
                }
            };
        }
        if let Some(path) = obj.particle.as_deref() {
            let spec = vfs
                .read_string(path)
                .map_err(|e| format!("cannot read particle {path}: {e}"))
                .and_then(|s| {
                    ParticleFile::from_json_str(&s)
                        .map_err(|e| format!("invalid particle {path}: {e}"))
                });
            return match spec {
                Ok(spec) => Some((
                    SceneLayerKind::Particle {
                        path: path.to_string(),
                        spec,
                    },
                    Vec2::ZERO,
                )),
                Err(e) => {
                    self.warn(format!("layer '{label}' skipped: {e}"));
                    None
                }
            };
        }
        if let Some(paths) = &obj.sound {
            return Some((
                SceneLayerKind::Sound {
                    paths: paths.clone(),
                    playbackmode: obj.playbackmode.clone().unwrap_or_else(|| "loop".into()),
                    volume: opt_f32(&obj.volume, &self.user_props, 1.0),
                    mintime: obj.mintime.unwrap_or(0.0),
                    maxtime: obj.maxtime.unwrap_or(0.0),
                },
                Vec2::ZERO,
            ));
        }
        if let Some(text) = &obj.text {
            if !*warned_text {
                *warned_text = true;
                self.warn(format!(
                    "text layer '{label}' is not drawn (text layers are not supported yet)"
                ));
            }
            return Some((
                SceneLayerKind::Text {
                    text: resolve_bound_value(text, &self.user_props),
                },
                Vec2::ZERO,
            ));
        }
        tracing::debug!("object '{label}' has no drawable payload; kept as a transform node");
        None
    }

    fn load_model_kind(
        &self,
        vfs: &WeVirtualFileSystem,
        model_path: &str,
    ) -> Result<LoadedKind, String> {
        let norm = normalize_path(model_path);
        let stock_utility =
            norm == SOLID_MODEL_PATH || COMPOSITION_MODEL_PATHS.contains(&norm.as_str());
        let model = match vfs.read_string(model_path) {
            Ok(text) => ModelJsonFile::from_json_str(&text)
                .map_err(|e| format!("invalid model {model_path}: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && stock_utility => {
                ModelJsonFile::default()
            }
            Err(e) => return Err(format!("cannot read model {model_path}: {e}")),
        };
        let model_size = match (model.width, model.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some(Vec2::new(w as f32, h as f32)),
            _ => None,
        };

        if model.solidlayer == Some(true) || norm.ends_with(SOLID_MODEL_PATH) {
            return Ok((
                SceneLayerKind::Solid { color: Vec3::ONE },
                model_size.unwrap_or(self.scene_size),
            ));
        }

        let material = if model.material.trim().is_empty() {
            None
        } else {
            let mtext = vfs
                .read_string(&model.material)
                .map_err(|e| format!("cannot read material {}: {e}", model.material))?;
            Some(
                MaterialFile::from_json_str(&mtext)
                    .map_err(|e| format!("invalid material {}: {e}", model.material))?,
            )
        };

        let is_composition = model.passthrough == Some(true)
            || model.fullscreen == Some(true)
            || COMPOSITION_MODEL_PATHS.iter().any(|p| norm.ends_with(p));
        if is_composition {
            return Ok((
                SceneLayerKind::Composition {
                    model_path: model_path.to_string(),
                    model,
                    material,
                },
                model_size.unwrap_or(self.scene_size),
            ));
        }

        let texture_paths: Vec<Option<String>> = material
            .as_ref()
            .and_then(|m| m.passes.first())
            .map(|p| {
                p.textures
                    .iter()
                    .map(|t| {
                        t.as_deref().filter(|s| !s.is_empty()).map(|name| {
                            if name.starts_with("_rt_") {
                                name.to_string()
                            } else {
                                material_texture_path(name)
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let base_tex = texture_paths
            .first()
            .cloned()
            .flatten()
            .filter(|p| !p.starts_with("_rt_"));
        let mut size = model_size;
        if let Some(tex_path) = &base_tex {
            let bytes = vfs
                .read_bytes(tex_path)
                .map_err(|e| format!("cannot read texture {tex_path}: {e}"))?;
            if size.is_none() {
                match parse_tex(&bytes) {
                    Ok(tex) if tex.header.image_width > 0 && tex.header.image_height > 0 => {
                        size = Some(Vec2::new(
                            tex.header.image_width as f32,
                            tex.header.image_height as f32,
                        ));
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("cannot parse texture header {tex_path}: {e}"),
                }
            }
        }

        let puppet_path = model.puppet.clone();
        Ok((
            SceneLayerKind::Image {
                model_path: model_path.to_string(),
                model,
                material,
                texture_paths,
                puppet_path,
            },
            size.unwrap_or(self.scene_size),
        ))
    }

    /// Re-resolves visible/alpha/brightness/color (and kind-specific bound values) for a layer.
    fn resolve_layer_values(&mut self, li: usize) {
        let props = &self.user_props;
        let layer = &mut self.layers[li];
        let obj = &self.scene.objects[layer.object_index];
        layer.visible = resolve_bound_bool(&obj.visible, props);
        layer.alpha = resolve_bound_value(&obj.alpha, props);
        layer.brightness = resolve_bound_value(&obj.brightness, props);
        layer.color = parse_vec3_or(&resolve_bound_value(&obj.color, props), Vec3::ONE);
        match &mut layer.kind {
            SceneLayerKind::Solid { color } => *color = layer.color,
            SceneLayerKind::Sound { volume, .. } => *volume = opt_f32(&obj.volume, props, 1.0),
            SceneLayerKind::Text { text } => {
                if let Some(t) = &obj.text {
                    *text = resolve_bound_value(t, props);
                }
            }
            _ => {}
        }
    }

    /// Copies layer transforms into the nodes and recomputes every world matrix.
    fn recompute_world(&mut self) {
        for node_index in 0..self.nodes.len() {
            if let Some(li) = self.nodes[node_index].layer {
                self.nodes[node_index].transform = self.layers[li].transform.clone();
            }
        }
        let locals: Vec<Mat4> = self
            .nodes
            .iter()
            .map(|n| layer_model_matrix(&n.transform))
            .collect();
        for i in 0..self.nodes.len() {
            let mut chain = vec![i];
            let mut cur = self.nodes[i].parent;
            while let Some(p) = cur {
                if chain.len() > self.nodes.len() {
                    break;
                }
                chain.push(p);
                cur = self.nodes[p].parent;
            }
            let world = chain
                .iter()
                .rev()
                .fold(Mat4::IDENTITY, |acc, &n| acc * locals[n]);
            self.nodes[i].world = world;
        }
        for node in &self.nodes {
            if let Some(li) = node.layer {
                self.layers[li].world_matrix = node.world;
            }
        }
    }

    /// World matrix of any `scene.objects` entry (including transform-only parent nodes).
    pub fn object_world_matrix(&self, object_index: usize) -> Option<Mat4> {
        self.nodes.get(object_index).map(|n| n.world)
    }

    /// Index of the first layer whose name (or id string) equals `key`.
    pub fn find_layer(&self, key: &str) -> Option<usize> {
        self.layers
            .iter()
            .position(|l| l.name == key)
            .or_else(|| self.layers.iter().position(|l| l.key() == key))
            .or_else(|| {
                self.layers
                    .iter()
                    .position(|l| l.id.map(|i| i.to_string()).as_deref() == Some(key))
            })
    }

    /// View-projection for an eye offset in scene units.
    pub fn view_projection(&self, eye_offset: Vec2) -> Mat4 {
        match self.camera.projection {
            SceneProjection::Orthographic { width, height } => {
                let center = Vec3::new(width * 0.5, height * 0.5, 0.0);
                let zoom = if self.camera.zoom > 0.0 && self.camera.zoom.is_finite() {
                    self.camera.zoom
                } else {
                    1.0
                };
                scene_orthographic_projection(width, height)
                    * Mat4::from_translation(center)
                    * Mat4::from_scale(Vec3::new(zoom, zoom, 1.0))
                    * Mat4::from_translation(-center)
                    * Mat4::from_translation(Vec3::new(-eye_offset.x, -eye_offset.y, 0.0))
            }
            SceneProjection::Perspective {
                fov_degrees,
                aspect,
                nearz,
                farz,
                eye,
                center,
                up,
            } => {
                let off = Vec3::new(eye_offset.x, eye_offset.y, 0.0);
                Mat4::perspective_rh(fov_degrees.to_radians(), aspect, nearz, farz)
                    * Mat4::look_at_rh(eye + off, center + off, up)
            }
        }
    }

    /// Per-pass matrices for layer `index` (`g_ModelViewProjectionMatrix` etc.), including the
    /// parent chain, parallax (scaled by the layer's `parallaxDepth`) and camera shake.
    pub fn layer_pass_matrices(&self, index: usize, camera: &CameraState) -> PassMatrices {
        let Some(layer) = self.layers.get(index) else {
            return build_pass_matrices(self.scene_size, &LayerTransform::default(), Vec2::ZERO);
        };
        let eye = camera.eye_offset(layer.parallax_depth, self.scene_size);
        let base = build_pass_matrices(self.scene_size, &layer.transform, eye);
        let simple_ortho = matches!(self.camera.projection, SceneProjection::Orthographic { width, height }
            if Vec2::new(width, height) == self.scene_size)
            && self.camera.zoom == 1.0;
        let view_projection = if simple_ortho {
            base.view_projection
        } else {
            self.view_projection(eye)
        };
        let model = layer.world_matrix;
        if view_projection == base.view_projection && model == base.model {
            return base;
        }
        let mvp = view_projection * model;
        PassMatrices {
            model_view_projection: mvp,
            model_view_projection_inverse: mvp.inverse(),
            model,
            model_inverse: model.inverse(),
            view_projection,
        }
    }

    /// Every scripted property (Form D) on every layer.
    pub fn property_scripts(&self) -> Vec<PropertyScriptSlot> {
        let mut out = Vec::new();
        for layer in &self.layers {
            let obj = &self.scene.objects[layer.object_index];
            let key = layer.key();
            let mut push = |prop: ScriptProperty, script: Option<&str>, initial: String| {
                if let Some(script) = script {
                    out.push(PropertyScriptSlot {
                        slot_key: format!("{key}/{}", prop.as_str()),
                        layer_name: layer.name.clone(),
                        property: prop.as_str().to_string(),
                        script: script.to_string(),
                        initial_value: initial,
                    });
                }
            };
            push(
                ScriptProperty::Origin,
                obj.origin.script(),
                obj.origin.default_value().clone(),
            );
            push(
                ScriptProperty::Scale,
                obj.scale.script(),
                obj.scale.default_value().clone(),
            );
            push(
                ScriptProperty::Angles,
                obj.angles.script(),
                obj.angles.default_value().clone(),
            );
            push(
                ScriptProperty::Alpha,
                obj.alpha.script(),
                obj.alpha.default_value().to_string(),
            );
            push(
                ScriptProperty::Visible,
                obj.visible.script(),
                obj.visible.default_value().to_string(),
            );
            push(
                ScriptProperty::Color,
                obj.color.script(),
                obj.color.default_value().clone(),
            );
            if let Some(size) = &obj.size {
                push(
                    ScriptProperty::Size,
                    size.script(),
                    size.default_value().clone(),
                );
            }
            push(
                ScriptProperty::Brightness,
                obj.brightness.script(),
                obj.brightness.default_value().to_string(),
            );
        }
        out
    }

    /// Current value of a layer property as a SceneScript-style string.
    pub fn property_value_string(&self, layer: usize, prop: ScriptProperty) -> Option<String> {
        let l = self.layers.get(layer)?;
        Some(match prop {
            ScriptProperty::Origin => fmt_vec3(l.transform.origin),
            ScriptProperty::Scale => fmt_vec3(l.transform.scale),
            ScriptProperty::Angles => fmt_vec3(l.transform.angles),
            ScriptProperty::Alpha => l.alpha.to_string(),
            ScriptProperty::Visible => l.visible.to_string(),
            ScriptProperty::Color => fmt_vec3(l.color),
            ScriptProperty::Size => fmt_vec2(l.transform.size),
            ScriptProperty::Brightness => l.brightness.to_string(),
        })
    }

    /// Applies a stringified SceneScript return value to `slot_key` (`"<layer>/<property>"`).
    /// Vectors accept `"x y z"` strings, JSON arrays or `{x,y,z}` objects (missing components
    /// keep their current value), numbers parse as `f32`, and `visible` accepts
    /// `"true"`/`"false"` or a number. Returns `false` for unknown slots or unparsable values.
    pub fn apply_script_output(&mut self, slot_key: &str, value: &str) -> bool {
        let Some((layer_key, prop_name)) = slot_key.rsplit_once('/') else {
            return false;
        };
        let (Some(li), Some(prop)) = (self.find_layer(layer_key), ScriptProperty::parse(prop_name))
        else {
            return false;
        };
        let raw = value.trim().trim_matches('"');
        let layer = &mut self.layers[li];
        let mut transform_changed = false;
        match prop {
            ScriptProperty::Origin | ScriptProperty::Scale | ScriptProperty::Angles => {
                let comps = parse_script_components(value);
                if comps.is_empty() {
                    return false;
                }
                let slot = match prop {
                    ScriptProperty::Origin => &mut layer.transform.origin,
                    ScriptProperty::Scale => &mut layer.transform.scale,
                    _ => &mut layer.transform.angles,
                };
                *slot = apply_components3(*slot, &comps);
                transform_changed = true;
            }
            ScriptProperty::Size => {
                let comps = parse_script_components(value);
                if comps.is_empty() {
                    return false;
                }
                layer.transform.size = apply_components2(layer.transform.size, &comps);
                transform_changed = true;
            }
            ScriptProperty::Color => {
                let comps = parse_script_components(value);
                if comps.is_empty() {
                    return false;
                }
                layer.color = apply_components3(layer.color, &comps);
                if let SceneLayerKind::Solid { color } = &mut layer.kind {
                    *color = layer.color;
                }
            }
            ScriptProperty::Alpha | ScriptProperty::Brightness => {
                let Ok(n) = raw.parse::<f32>() else {
                    return false;
                };
                if !n.is_finite() {
                    return false;
                }
                if prop == ScriptProperty::Alpha {
                    layer.alpha = n;
                } else {
                    layer.brightness = n;
                }
            }
            ScriptProperty::Visible => {
                layer.visible = match raw {
                    "true" => true,
                    "false" => false,
                    other => match other.parse::<f32>() {
                        Ok(n) => n != 0.0,
                        Err(_) => return false,
                    },
                };
            }
        }
        if transform_changed {
            self.recompute_world();
        }
        true
    }

    /// `thisLayer` write-back from SceneScript. Returns `false` when no layer matches `name`.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_layer_state(
        &mut self,
        name: &str,
        origin: Vec3,
        scale: Vec3,
        angles: Vec3,
        size: Vec2,
        color: Vec3,
        alpha: f32,
        visible: bool,
    ) -> bool {
        let Some(li) = self.find_layer(name) else {
            return false;
        };
        let layer = &mut self.layers[li];
        layer.transform.origin = origin;
        layer.transform.scale = scale;
        layer.transform.angles = angles;
        layer.transform.size = size;
        layer.color = color;
        layer.alpha = alpha;
        layer.visible = visible;
        if let SceneLayerKind::Solid { color: c } = &mut layer.kind {
            *c = color;
        }
        self.recompute_world();
        true
    }

    /// Merges changed user-property values (JSON object) into the current set and re-resolves
    /// every bound value (layer transforms, visibility, alpha, colors, camera, clear color).
    pub fn apply_user_properties(&mut self, json: &str) -> Result<(), SceneLoadError> {
        let changed = parse_user_properties(json)
            .map_err(|e| SceneLoadError::UserProperties(e.to_string()))?;
        self.user_props.extend(changed);
        let perspective = self.camera.projection.is_perspective();
        self.resolve_general(perspective);
        for i in 0..self.nodes.len() {
            let fallback = self.nodes[i].fallback_size;
            let t = resolve_transform(&self.scene.objects[i], &self.user_props, fallback);
            self.nodes[i].transform = t.clone();
            if let Some(li) = self.nodes[i].layer {
                self.layers[li].transform = t;
            }
        }
        for li in 0..self.layers.len() {
            self.resolve_layer_values(li);
        }
        self.recompute_world();
        Ok(())
    }

    /// Steps every particle system of a visible layer. `cursor_world` is in scene units and is
    /// converted into each layer's local space for cursor-bound control points.
    pub fn step_particles(&mut self, dt: f32, cursor_world: Vec2) {
        for (li, inst) in &mut self.particles {
            let Some(layer) = self.layers.get(*li) else {
                continue;
            };
            if !layer.visible {
                continue;
            }
            let inv = layer.world_matrix.inverse();
            let local = if inv.is_finite() {
                inv.transform_point3(cursor_world.extend(0.0))
            } else {
                cursor_world.extend(0.0)
            };
            inst.step(dt, local);
        }
    }

    /// Particle system of a particle layer.
    pub fn particle_system(&self, layer: usize) -> Option<&ParticleSystemInstance> {
        self.particles
            .iter()
            .find(|(li, _)| *li == layer)
            .map(|(_, p)| p)
    }

    /// `(layer index, system)` for every particle layer.
    pub fn particle_systems(&self) -> impl Iterator<Item = (usize, &ParticleSystemInstance)> {
        self.particles.iter().map(|(li, p)| (*li, p))
    }
}
