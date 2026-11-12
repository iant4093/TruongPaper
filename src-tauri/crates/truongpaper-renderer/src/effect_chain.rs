//! Multi-pass ping-pong FBO effect chain planner and bloom plan
//! (`docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` §7.2 item 3, `docs/05` Phase 6).
//!
//! [`EffectChainPlan::build`] turns a layer's `effects` into a device-free list of
//! [`EffectStep`]s that `scene_renderer::SceneRenderer` executes:
//! 1. `DrawBase { into: FboA }` renders the layer's base texture into ping-pong target A.
//! 2. For each visible effect and each effect pass: `command: "copy"` passes become
//!    [`EffectStep::Copy`]; material passes become [`EffectStep::Pass`]. A pass with a `target`
//!    draws into that named FBO without swapping; otherwise it draws into the other ping-pong
//!    buffer and the buffers swap.
//! 3. `Composite` draws the last ping-pong output into the scene with the layer's material
//!    blending.
//!
//! Texture slot rules: slot 0 defaults to the current ping-pong source; material textures,
//! then `ScenePassOverride.textures` (same pass index), then `pass.bind` override that.
//! `"previous"` names the current source, `_rt_FullFrameBuffer` the scene-wide full-frame copy,
//! names declared in `effect.fbos` the effect's own FBOs (scoped per effect), and anything else
//! a `materials/<name>.tex` asset. Combos: material combos overridden by the scene pass override.
//! Constants: material `constantshadervalues` overridden by the scene pass override, whose
//! user-bound values are resolved against the user properties.

use std::collections::{BTreeMap, HashMap};

use glam::Vec2;
use truongpaper_we_formats::{
    resolve_bound_bool, resolve_bound_value, SceneGeneral, ScenePassOverride, UserPropertyMap,
    WeVirtualFileSystem,
};

use crate::json::Value;
use crate::scene_assets::{
    material_texture_path, EffectFboSpec, EffectFile, MaterialFile, MaterialPassSpec,
};
use crate::scene_graph::SceneLayer;

/// Name of the scene-wide full-frame color copy.
pub const RT_FULL_FRAME_BUFFER: &str = "_rt_FullFrameBuffer";
/// Bind name for the current ping-pong source.
pub const RT_PREVIOUS: &str = "previous";
/// Bloom render targets (linux-wallpaperengine names).
pub const RT_4_FRAME_BUFFER: &str = "_rt_4FrameBuffer";
pub const RT_8_FRAME_BUFFER: &str = "_rt_8FrameBuffer";
pub const RT_BLOOM: &str = "_rt_Bloom";
/// The scene color target the layers are composited into.
pub const RT_SCENE: &str = "_rt_Scene";

pub const DEFAULT_BLOOM_STRENGTH: f32 = 2.0;
pub const DEFAULT_BLOOM_THRESHOLD: f32 = 0.65;

/// Material `blending` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Translucent,
    Additive,
    Multiply,
    Screen,
}

impl BlendMode {
    /// Parses a material `blending` string. Unknown values map to `Normal` with a warning;
    /// `""` and `"disabled"` map to `Normal` silently.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "normal" | "" | "disabled" => BlendMode::Normal,
            "translucent" => BlendMode::Translucent,
            "additive" => BlendMode::Additive,
            "multiply" => BlendMode::Multiply,
            "screen" => BlendMode::Screen,
            other => {
                tracing::warn!("unknown material blending '{other}', using normal");
                BlendMode::Normal
            }
        }
    }

    /// `None` = replace (no blending). Color and alpha use the same factors.
    pub fn to_wgpu_blend(self) -> Option<wgpu::BlendState> {
        use wgpu::BlendFactor as F;
        let comp = |src, dst| wgpu::BlendComponent {
            src_factor: src,
            dst_factor: dst,
            operation: wgpu::BlendOperation::Add,
        };
        let c = match self {
            BlendMode::Normal => return None,
            BlendMode::Translucent => comp(F::SrcAlpha, F::OneMinusSrcAlpha),
            BlendMode::Additive => comp(F::SrcAlpha, F::One),
            BlendMode::Multiply => comp(F::Dst, F::Zero),
            BlendMode::Screen => comp(F::One, F::OneMinusSrc),
        };
        Some(wgpu::BlendState { color: c, alpha: c })
    }
}

/// A render target referenced by the plan.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenderTargetId {
    FboA,
    FboB,
    /// An FBO declared in `effects[effect].fbos`.
    Named {
        effect: usize,
        name: String,
    },
    /// `_rt_FullFrameBuffer`.
    FullFrame,
    /// Output of another image layer, sampled by composition effects.
    LayerOutput(i32),
    /// The scene color target.
    Scene,
}

impl RenderTargetId {
    fn other_ping_pong(&self) -> RenderTargetId {
        match self {
            RenderTargetId::FboA => RenderTargetId::FboB,
            _ => RenderTargetId::FboA,
        }
    }
}

/// What a texture slot samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureBinding {
    Target(RenderTargetId),
    /// A `.tex` asset VFS path.
    Asset(String),
}

/// An FBO to allocate.
#[derive(Debug, Clone, PartialEq)]
pub struct FboDesc {
    pub id: RenderTargetId,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
}

/// One material pass of an effect.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectPassPlan {
    pub effect_index: usize,
    pub pass_index: usize,
    pub effect_file: String,
    pub material_path: String,
    /// `material.passes[0]`.
    pub material: MaterialPassSpec,
    pub shader: String,
    pub blending: BlendMode,
    pub combos: HashMap<String, i32>,
    pub constants: HashMap<String, Value>,
    /// Texture slot -> binding.
    pub textures: BTreeMap<u32, TextureBinding>,
    pub target: RenderTargetId,
    /// `true` when the pass drew into the other ping-pong buffer and the buffers swapped.
    pub swaps: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EffectStep {
    DrawBase {
        into: RenderTargetId,
    },
    Pass(Box<EffectPassPlan>),
    Copy {
        source: RenderTargetId,
        target: RenderTargetId,
    },
    Composite {
        source: RenderTargetId,
        blending: BlendMode,
    },
}

/// Device-free effect chain for one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectChainPlan {
    pub width: u32,
    pub height: u32,
    /// `FboA`, `FboB` (layer size, `Rgba8Unorm`).
    pub ping_pong: [FboDesc; 2],
    /// Named effect FBOs.
    pub fbos: Vec<FboDesc>,
    pub steps: Vec<EffectStep>,
    /// Any pass binds `_rt_FullFrameBuffer`.
    pub uses_full_frame_buffer: bool,
    pub warnings: Vec<String>,
}

/// `'rgba8888'`/`'rgba_backbuffer'`/`''` -> `Rgba8Unorm`, `'r8'` -> `R8Unorm`,
/// `'rg88'` -> `Rg8Unorm`; unknown -> `Rgba8Unorm` with a warning.
pub fn fbo_texture_format(format: &str) -> wgpu::TextureFormat {
    match format.trim().to_ascii_lowercase().as_str() {
        "" | "rgba8888" | "rgba_backbuffer" => wgpu::TextureFormat::Rgba8Unorm,
        "r8" => wgpu::TextureFormat::R8Unorm,
        "rg88" => wgpu::TextureFormat::Rg8Unorm,
        other => {
            tracing::warn!("unknown fbo format '{other}', using rgba8888");
            wgpu::TextureFormat::Rgba8Unorm
        }
    }
}

/// `(size / scale).max(1)` per axis; `scale` 0 is treated as 1.
pub fn fbo_size(layer_w: u32, layer_h: u32, scale: u32) -> (u32, u32) {
    let s = scale.max(1);
    ((layer_w / s).max(1), (layer_h / s).max(1))
}

fn size_px(v: f32) -> u32 {
    if v.is_finite() && v >= 1.0 {
        v.round().min(16384.0) as u32
    } else {
        1
    }
}

/// Converts a resolved user-bound value (we-formats `serde_json::Value`) into [`Value`] through
/// its JSON text.
fn to_local_value(json_text: &str) -> Value {
    Value::parse(json_text).unwrap_or(Value::String(json_text.to_string()))
}

struct Resolver<'a> {
    effect_index: usize,
    fbos: &'a [EffectFboSpec],
}

impl Resolver<'_> {
    /// Resolves a target/source/bind name. `None` for unknown names.
    fn target(&self, name: &str, current: &RenderTargetId) -> Option<RenderTargetId> {
        let n = name.trim();
        if n.is_empty() || n.eq_ignore_ascii_case(RT_PREVIOUS) {
            Some(current.clone())
        } else if n == RT_FULL_FRAME_BUFFER {
            Some(RenderTargetId::FullFrame)
        } else if self.fbos.iter().any(|f| f.name == n) {
            Some(RenderTargetId::Named {
                effect: self.effect_index,
                name: n.to_string(),
            })
        } else {
            None
        }
    }

    fn texture(&self, name: &str, current: &RenderTargetId) -> TextureBinding {
        if let Some(id) = name
            .strip_prefix("_rt_imageLayerComposite_")
            .and_then(|name| name.strip_suffix("_a"))
            .and_then(|id| id.parse().ok())
        {
            return TextureBinding::Target(RenderTargetId::LayerOutput(id));
        }
        match self.target(name, current) {
            Some(t) => TextureBinding::Target(t),
            None if name.starts_with("_rt_") => {
                tracing::warn!("unknown render target '{name}', binding the current source");
                TextureBinding::Target(current.clone())
            }
            None => TextureBinding::Asset(material_texture_path(name)),
        }
    }
}

impl EffectChainPlan {
    /// Plans the effect chain for `layer`. Missing effect/material files are recorded in
    /// `warnings` and skipped.
    pub fn build(
        vfs: &WeVirtualFileSystem,
        layer: &SceneLayer,
        user_props: &UserPropertyMap,
    ) -> EffectChainPlan {
        let width = size_px(layer.transform.size.x);
        let height = size_px(layer.transform.size.y);
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let mut plan = EffectChainPlan {
            width,
            height,
            ping_pong: [
                FboDesc {
                    id: RenderTargetId::FboA,
                    width,
                    height,
                    format: rgba,
                },
                FboDesc {
                    id: RenderTargetId::FboB,
                    width,
                    height,
                    format: rgba,
                },
            ],
            fbos: Vec::new(),
            steps: vec![EffectStep::DrawBase {
                into: RenderTargetId::FboA,
            }],
            uses_full_frame_buffer: false,
            warnings: Vec::new(),
        };
        let mut current = RenderTargetId::FboA;

        for (effect_index, effect_ref) in layer.effects.iter().enumerate() {
            if !resolve_bound_bool(&effect_ref.visible, user_props) {
                continue;
            }
            let effect = match vfs
                .read_string(&effect_ref.file)
                .map_err(|e| e.to_string())
                .and_then(|s| EffectFile::from_json_str(&s).map_err(|e| e.to_string()))
            {
                Ok(e) => e,
                Err(e) => {
                    plan.warn(format!("effect {} skipped: {e}", effect_ref.file));
                    continue;
                }
            };
            for fbo in &effect.fbos {
                let (w, h) = fbo_size(width, height, fbo.scale);
                plan.fbos.push(FboDesc {
                    id: RenderTargetId::Named {
                        effect: effect_index,
                        name: fbo.name.clone(),
                    },
                    width: w,
                    height: h,
                    format: fbo_texture_format(&fbo.format),
                });
            }
            let resolver = Resolver {
                effect_index,
                fbos: &effect.fbos,
            };

            for (pass_index, pass) in effect.passes.iter().enumerate() {
                let over = effect_ref.passes.get(pass_index);
                if pass
                    .command
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case("copy"))
                {
                    let source = pass
                        .source
                        .as_deref()
                        .and_then(|s| resolver.target(s, &current))
                        .unwrap_or_else(|| current.clone());
                    match pass
                        .target
                        .as_deref()
                        .and_then(|t| resolver.target(t, &current))
                    {
                        Some(target) => plan.steps.push(EffectStep::Copy { source, target }),
                        None => {
                            let target = current.other_ping_pong();
                            plan.steps.push(EffectStep::Copy {
                                source,
                                target: target.clone(),
                            });
                            current = target;
                        }
                    }
                    if matches!(
                        plan.steps.last(),
                        Some(EffectStep::Copy {
                            source: RenderTargetId::FullFrame,
                            ..
                        })
                    ) {
                        plan.uses_full_frame_buffer = true;
                    }
                    continue;
                }

                let Some(material_path) = pass.material.clone() else {
                    plan.warn(format!(
                        "effect {} pass {pass_index} has neither material nor command",
                        effect_ref.file
                    ));
                    continue;
                };
                let material = match vfs
                    .read_string(&material_path)
                    .map_err(|e| e.to_string())
                    .and_then(|s| MaterialFile::from_json_str(&s).map_err(|e| e.to_string()))
                {
                    Ok(m) => m,
                    Err(e) => {
                        plan.warn(format!("material {material_path} skipped: {e}"));
                        continue;
                    }
                };
                let Some(mpass) = material.passes.into_iter().next() else {
                    plan.warn(format!("material {material_path} has no passes"));
                    continue;
                };

                let combos = merge_combos(&mpass.combos, over);
                let constants = merge_constants(&mpass.constantshadervalues, over, user_props);

                let mut textures = BTreeMap::new();
                textures.insert(0u32, TextureBinding::Target(current.clone()));
                for (slot, name) in mpass.textures.iter().enumerate().skip(1) {
                    if let Some(name) = name.as_deref().filter(|n| !n.is_empty()) {
                        textures.insert(slot as u32, resolver.texture(name, &current));
                    }
                }
                if let Some(over) = over {
                    for (slot, name) in over.textures.iter().enumerate() {
                        if let Some(name) = name.as_deref().filter(|n| !n.is_empty()) {
                            textures.insert(slot as u32, resolver.texture(name, &current));
                        }
                    }
                }
                for bind in &pass.bind {
                    textures.insert(bind.index as u32, resolver.texture(&bind.name, &current));
                }
                if textures
                    .values()
                    .any(|t| *t == TextureBinding::Target(RenderTargetId::FullFrame))
                {
                    plan.uses_full_frame_buffer = true;
                }

                let explicit_target =
                    pass.target
                        .as_deref()
                        .filter(|t| !t.trim().is_empty())
                        .map(|t| match resolver.target(t, &current) {
                            Some(id) => id,
                            None => {
                                tracing::warn!("unknown pass target '{t}', drawing into ping-pong");
                                current.other_ping_pong()
                            }
                        });
                let (target, swaps) = match explicit_target {
                    Some(t) if t != current.other_ping_pong() && t != current => (t, false),
                    _ => (current.other_ping_pong(), true),
                };

                plan.steps.push(EffectStep::Pass(Box::new(EffectPassPlan {
                    effect_index,
                    pass_index,
                    effect_file: effect_ref.file.clone(),
                    material_path,
                    shader: mpass.shader.clone(),
                    blending: BlendMode::parse(&mpass.blending),
                    material: mpass,
                    combos,
                    constants,
                    textures,
                    target: target.clone(),
                    swaps,
                })));
                if swaps {
                    current = target;
                }
            }
        }

        let blending = layer
            .kind
            .material()
            .and_then(|m| m.passes.first())
            .map(|p| BlendMode::parse(&p.blending))
            .unwrap_or_default();
        plan.steps.push(EffectStep::Composite {
            source: current,
            blending,
        });
        plan
    }

    fn warn(&mut self, msg: String) {
        tracing::warn!("{msg}");
        self.warnings.push(msg);
    }

    /// The ping-pong buffer the composite step reads.
    pub fn final_source(&self) -> &RenderTargetId {
        match self.steps.last() {
            Some(EffectStep::Composite { source, .. }) => source,
            _ => &RenderTargetId::FboA,
        }
    }

    /// The material passes in execution order.
    pub fn passes(&self) -> impl Iterator<Item = &EffectPassPlan> {
        self.steps.iter().filter_map(|s| match s {
            EffectStep::Pass(p) => Some(p.as_ref()),
            _ => None,
        })
    }

    /// Every FBO the plan uses (ping-pong first).
    pub fn all_fbos(&self) -> impl Iterator<Item = &FboDesc> {
        self.ping_pong.iter().chain(self.fbos.iter())
    }
}

fn merge_combos(
    material: &HashMap<String, i32>,
    over: Option<&ScenePassOverride>,
) -> HashMap<String, i32> {
    let mut out = material.clone();
    if let Some(o) = over {
        for (k, v) in &o.combos {
            out.insert(k.clone(), *v);
        }
    }
    out
}

fn merge_constants(
    material: &HashMap<String, Value>,
    over: Option<&ScenePassOverride>,
    user_props: &UserPropertyMap,
) -> HashMap<String, Value> {
    let mut out = material.clone();
    if let Some(o) = over {
        for (k, bound) in &o.constantshadervalues {
            let resolved = resolve_bound_value(bound, user_props);
            out.insert(k.clone(), to_local_value(&resolved.to_string()));
        }
    }
    out
}

/// Bloom pass kinds (`shaders/bloom.wgsl` entry points).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BloomStage {
    ThresholdDownsample,
    DownsampleBlur,
    Blur,
    Combine,
}

impl BloomStage {
    pub fn fragment_entry_point(self) -> &'static str {
        match self {
            BloomStage::ThresholdDownsample => "fs_threshold_downsample",
            BloomStage::DownsampleBlur => "fs_downsample_blur",
            BloomStage::Blur => "fs_blur",
            BloomStage::Combine => "fs_combine",
        }
    }
}

/// Vertex entry point of `shaders/bloom.wgsl`.
pub const BLOOM_VERTEX_ENTRY_POINT: &str = "vs_main";

#[derive(Debug, Clone, PartialEq)]
pub struct BloomPassPlan {
    pub stage: BloomStage,
    pub source: &'static str,
    pub target: &'static str,
    /// Target size in pixels.
    pub width: u32,
    pub height: u32,
    pub blending: BlendMode,
}

/// Scene bloom (`general.bloom`, `bloomstrength`, `bloomthreshold`).
#[derive(Debug, Clone, PartialEq)]
pub struct BloomPlan {
    pub enabled: bool,
    pub strength: f32,
    pub threshold: f32,
    /// Empty when disabled.
    pub passes: Vec<BloomPassPlan>,
}

impl BloomPlan {
    /// Threshold + downsample into `_rt_4FrameBuffer` (1/4), downsample + blur into
    /// `_rt_8FrameBuffer` (1/8), blur into `_rt_Bloom` (1/8), additive combine into the scene.
    pub fn from_general(
        general: &SceneGeneral,
        user_props: &UserPropertyMap,
        scene_size: Vec2,
    ) -> BloomPlan {
        let enabled = general
            .bloom
            .as_ref()
            .map(|b| resolve_bound_bool(b, user_props))
            .unwrap_or(false);
        let strength = general
            .bloomstrength
            .as_ref()
            .map(|b| resolve_bound_value(b, user_props))
            .unwrap_or(DEFAULT_BLOOM_STRENGTH);
        let threshold = general
            .bloomthreshold
            .as_ref()
            .map(|b| resolve_bound_value(b, user_props))
            .unwrap_or(DEFAULT_BLOOM_THRESHOLD);
        let (w, h) = (size_px(scene_size.x), size_px(scene_size.y));
        let (w4, h4) = fbo_size(w, h, 4);
        let (w8, h8) = fbo_size(w, h, 8);
        let passes = if enabled {
            vec![
                BloomPassPlan {
                    stage: BloomStage::ThresholdDownsample,
                    source: RT_SCENE,
                    target: RT_4_FRAME_BUFFER,
                    width: w4,
                    height: h4,
                    blending: BlendMode::Normal,
                },
                BloomPassPlan {
                    stage: BloomStage::DownsampleBlur,
                    source: RT_4_FRAME_BUFFER,
                    target: RT_8_FRAME_BUFFER,
                    width: w8,
                    height: h8,
                    blending: BlendMode::Normal,
                },
                BloomPassPlan {
                    stage: BloomStage::Blur,
                    source: RT_8_FRAME_BUFFER,
                    target: RT_BLOOM,
                    width: w8,
                    height: h8,
                    blending: BlendMode::Normal,
                },
                BloomPassPlan {
                    stage: BloomStage::Combine,
                    source: RT_BLOOM,
                    target: RT_SCENE,
                    width: w,
                    height: h,
                    blending: BlendMode::Additive,
                },
            ]
        } else {
            Vec::new()
        };
        BloomPlan {
            enabled,
            strength,
            threshold,
            passes,
        }
    }
}
