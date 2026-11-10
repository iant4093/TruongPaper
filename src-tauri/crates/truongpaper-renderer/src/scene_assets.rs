//! Renderer-local `model.json`, `effect.json` and `material.json` types
//! (`docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` §7.1).
//!
//! These mirror `truongpaper_we_formats::{ModelJsonFile, EffectFile, EffectFboSpec,
//! EffectPassSpec, EffectBindSlot, MaterialFile, MaterialPassSpec}` field for field, with the same
//! defaults (`blending = "normal"`, `depthtest`/`depthwrite = "disabled"`, `cullmode = "nocull"`,
//! FBO `scale = 1`). They exist because the upstream types derive `serde::Deserialize` and this
//! crate has no `serde`/`serde_json` dependency, so the documents are parsed with
//! [`crate::json::Value`] instead (same pattern as `particles::ParticleFile::from_json`).
//! `constantshadervalues` entries are kept as raw [`Value`]s.
//!
//! `ModelJsonFile` additionally reads `solidlayer`, `passthrough` and `nopadding`, which the
//! upstream type does not declare but real Wallpaper Engine model files use.

use std::collections::HashMap;

use crate::json::{JsonError, Value};

#[derive(Debug, thiserror::Error)]
pub enum SceneAssetError {
    #[error("asset JSON parse error: {0}")]
    Json(#[from] JsonError),
    #[error("{0} JSON root must be an object")]
    NotAnObject(&'static str),
    #[error("{0} entry is missing required field `{1}`")]
    MissingField(&'static str, &'static str),
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn get_string_or(v: &Value, key: &str, default: &str) -> String {
    get_str(v, key).unwrap_or_else(|| default.to_string())
}

fn get_bool(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

/// Accepts JSON numbers and numeric strings (`"1920"`), like loosely typed WE files.
fn get_number(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => Some(*n),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn get_array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).unwrap_or(&[])
}

fn expect_object(v: &Value, what: &'static str) -> Result<(), SceneAssetError> {
    if v.is_object() {
        Ok(())
    } else {
        Err(SceneAssetError::NotAnObject(what))
    }
}

/// `textures: ["bg", null, ...]` -> `[Some("bg"), None, ...]`.
fn parse_texture_slots(v: &Value) -> Vec<Option<String>> {
    get_array(v, "textures")
        .iter()
        .map(|t| t.as_str().map(str::to_string))
        .collect()
}

/// `combos: { "NAME": 1 }`. Booleans map to 0/1; non-numeric values are skipped.
fn parse_combos(v: &Value) -> HashMap<String, i32> {
    let mut out = HashMap::new();
    if let Some(Value::Object(entries)) = v.get("combos") {
        for (k, val) in entries {
            let n = match val {
                Value::Number(n) => Some(*n as i32),
                Value::Bool(b) => Some(i32::from(*b)),
                Value::String(s) => s.trim().parse::<i32>().ok(),
                _ => None,
            };
            if let Some(n) = n {
                out.insert(k.clone(), n);
            }
        }
    }
    out
}

fn parse_constants(v: &Value) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    if let Some(Value::Object(entries)) = v.get("constantshadervalues") {
        for (k, val) in entries {
            out.insert(k.clone(), val.clone());
        }
    }
    out
}

/// `models/*.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelJsonFile {
    pub material: String,
    pub puppet: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub autosize: Option<bool>,
    pub fullscreen: Option<bool>,
    pub solidlayer: Option<bool>,
    pub passthrough: Option<bool>,
    pub nopadding: Option<bool>,
}

impl ModelJsonFile {
    pub fn from_json_str(src: &str) -> Result<Self, SceneAssetError> {
        Self::from_json(&Value::parse(src)?)
    }

    pub fn from_json(v: &Value) -> Result<Self, SceneAssetError> {
        expect_object(v, "model")?;
        Ok(Self {
            material: get_string_or(v, "material", ""),
            puppet: get_str(v, "puppet").filter(|s| !s.is_empty()),
            width: get_number(v, "width").map(|n| n as i32),
            height: get_number(v, "height").map(|n| n as i32),
            autosize: get_bool(v, "autosize"),
            fullscreen: get_bool(v, "fullscreen"),
            solidlayer: get_bool(v, "solidlayer"),
            passthrough: get_bool(v, "passthrough"),
            nopadding: get_bool(v, "nopadding"),
        })
    }
}

/// `effects/*/effect.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectFile {
    pub name: String,
    pub fbos: Vec<EffectFboSpec>,
    pub passes: Vec<EffectPassSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectFboSpec {
    pub name: String,
    /// Divisor applied to the layer size (default 1).
    pub scale: u32,
    pub format: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectPassSpec {
    pub material: Option<String>,
    pub command: Option<String>,
    pub target: Option<String>,
    pub source: Option<String>,
    pub bind: Vec<EffectBindSlot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EffectBindSlot {
    pub name: String,
    pub index: usize,
}

impl EffectFile {
    pub fn from_json_str(src: &str) -> Result<Self, SceneAssetError> {
        Self::from_json(&Value::parse(src)?)
    }

    pub fn from_json(v: &Value) -> Result<Self, SceneAssetError> {
        expect_object(v, "effect")?;
        let fbos = get_array(v, "fbos")
            .iter()
            .map(EffectFboSpec::from_json)
            .collect::<Result<_, _>>()?;
        let passes = get_array(v, "passes")
            .iter()
            .map(EffectPassSpec::from_json)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            name: get_string_or(v, "name", ""),
            fbos,
            passes,
        })
    }
}

impl EffectFboSpec {
    pub fn from_json(v: &Value) -> Result<Self, SceneAssetError> {
        expect_object(v, "effect fbo")?;
        Ok(Self {
            name: get_str(v, "name").ok_or(SceneAssetError::MissingField("effect fbo", "name"))?,
            scale: get_number(v, "scale")
                .map(|n| n.max(0.0) as u32)
                .unwrap_or(1),
            format: get_string_or(v, "format", ""),
        })
    }
}

impl EffectPassSpec {
    pub fn from_json(v: &Value) -> Result<Self, SceneAssetError> {
        expect_object(v, "effect pass")?;
        let bind = get_array(v, "bind")
            .iter()
            .map(EffectBindSlot::from_json)
            .collect::<Result<_, _>>()?;
        Ok(Self {
            material: get_str(v, "material"),
            command: get_str(v, "command"),
            target: get_str(v, "target"),
            source: get_str(v, "source"),
            bind,
        })
    }
}

impl EffectBindSlot {
    pub fn from_json(v: &Value) -> Result<Self, SceneAssetError> {
        expect_object(v, "effect bind")?;
        Ok(Self {
            name: get_str(v, "name").ok_or(SceneAssetError::MissingField("effect bind", "name"))?,
            index: get_number(v, "index")
                .filter(|n| *n >= 0.0)
                .map(|n| n as usize)
                .ok_or(SceneAssetError::MissingField("effect bind", "index"))?,
        })
    }
}

/// `materials/*.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaterialFile {
    pub passes: Vec<MaterialPassSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialPassSpec {
    pub shader: String,
    pub blending: String,
    pub depthtest: String,
    pub depthwrite: String,
    pub cullmode: String,
    pub textures: Vec<Option<String>>,
    pub combos: HashMap<String, i32>,
    pub constantshadervalues: HashMap<String, Value>,
}

impl MaterialFile {
    pub fn from_json_str(src: &str) -> Result<Self, SceneAssetError> {
        Self::from_json(&Value::parse(src)?)
    }

    pub fn from_json(v: &Value) -> Result<Self, SceneAssetError> {
        expect_object(v, "material")?;
        let passes = v
            .get("passes")
            .and_then(Value::as_array)
            .ok_or(SceneAssetError::MissingField("material", "passes"))?
            .iter()
            .map(MaterialPassSpec::from_json)
            .collect::<Result<_, _>>()?;
        Ok(Self { passes })
    }
}

impl MaterialPassSpec {
    pub fn from_json(v: &Value) -> Result<Self, SceneAssetError> {
        expect_object(v, "material pass")?;
        Ok(Self {
            shader: get_str(v, "shader")
                .ok_or(SceneAssetError::MissingField("material pass", "shader"))?,
            blending: get_string_or(v, "blending", "normal"),
            depthtest: get_string_or(v, "depthtest", "disabled"),
            depthwrite: get_string_or(v, "depthwrite", "disabled"),
            cullmode: get_string_or(v, "cullmode", "nocull"),
            textures: parse_texture_slots(v),
            combos: parse_combos(v),
            constantshadervalues: parse_constants(v),
        })
    }
}

/// Maps a material texture name to its VFS path: `"bg"` -> `"materials/bg.tex"`. Names that
/// already carry the `.tex` extension or a `materials/` prefix are kept as given.
pub fn material_texture_path(name: &str) -> String {
    let trimmed = name.trim().trim_start_matches(['/', '\\']);
    let with_dir = if trimmed.starts_with("materials/") {
        trimmed.to_string()
    } else {
        format!("materials/{trimmed}")
    };
    if with_dir.to_ascii_lowercase().ends_with(".tex") {
        with_dir
    } else {
        format!("{with_dir}.tex")
    }
}

/// Maps a material `shader` name to its two VFS paths: `"genericimage2"` ->
/// `("shaders/genericimage2.vert", "shaders/genericimage2.frag")`.
pub fn shader_stage_paths(shader: &str) -> (String, String) {
    let base = shader.trim().trim_start_matches(['/', '\\']);
    let base = base.strip_prefix("shaders/").unwrap_or(base);
    (
        format!("shaders/{base}.vert"),
        format!("shaders/{base}.frag"),
    )
}
