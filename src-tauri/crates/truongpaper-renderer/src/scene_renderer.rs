//! `wgpu` executor for a loaded [`SceneGraph`] (`docs/05` Phase 6, `docs/03` §4, §5, §7.2).
//!
//! The pure helpers ([`pack_uniform_block`], [`layer_quad_vertices`],
//! [`texture_bind_group_layout_entries`], [`reflect_texture_slots`],
//! [`vertex_attribute_locations`], [`map_material_constants`]) are unit tested headlessly. The
//! [`SceneRenderer`] itself needs a `wgpu::Device` and is compile-checked only.
//!
//! Rendering model:
//! - Each image/composition layer runs its [`EffectChainPlan`]: the base texture (or, for
//!   composition layers, the `_rt_FullFrameBuffer` copy of the scene so far) is drawn into
//!   ping-pong target A with the layer material's WE shader, effect passes run in plan order and
//!   the last ping-pong output is composited into the scene target with the layer's world MVP
//!   and material blending.
//! - Solid layers draw a colored quad; particle layers draw the 68-byte sprite vertices with a
//!   built-in WGSL sprite shader ([`SCENE_PARTICLE_WGSL`]) instead of `genericparticle.vert`.
//! - Bloom runs after all layers (`shaders/bloom.wgsl`), then the scene target is blitted into
//!   the caller's view.
//! - Puppets are skinned on the CPU with [`PuppetDeformer`] every frame and the material shader
//!   is compiled without the `SKINNING`/`BONECOUNT` combos: the shader preprocessor's std140
//!   reflection cannot size `uniform mat4x3 g_Bones[BONECOUNT]`, so GPU skinning would get a
//!   wrong uniform layout. [`crate::puppet::puppet_combos`] is kept for a future GPU path.
//! - Shader compile and pipeline errors are caught with `wgpu` error scopes; the affected pass
//!   falls back to a plain texture copy instead of panicking.
//! - Embedded MP4 `.tex` layers get a white placeholder texture sized from the `.tex` header in
//!   [`EMBEDDED_VIDEO_FORMAT`] (BGRA); the host decodes the video on the platform side and
//!   pushes frames with [`SceneRenderer::upload_video_frame`] (see
//!   [`SceneRenderer::embedded_video_paths`]). BGRA frames of the texture size are uploaded
//!   without a CPU conversion ([`plan_video_upload`]).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use ab_glyph::FontVec;
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3, Vec4};
use truongpaper_we_formats::{
    decode_tex_mipmap_to_rgba8, parse_mdl, parse_tex, parse_vec_components, resolve_bound_bool,
    resolve_bound_value, MdlPuppetVertex, TexFormat, WeVirtualFileSystem, FIF_UNKNOWN,
};
use wgpu::util::DeviceExt;

use crate::diagnostics::DiagnosticsLog;
use crate::effect_chain::{
    short_name, BlendMode, BloomPassPlan, BloomPlan, BloomStage, EffectChainPlan, EffectStep,
    RenderTargetId, TextureBinding, BLOOM_VERTEX_ENTRY_POINT, RT_FULL_FRAME_BUFFER, RT_SCENE,
};
use crate::json::Value;
use crate::particles::SpriteParticleVertex;
use crate::puppet::{
    layered_bone_matrices, parse_mdl_skeleton, PuppetDeformer, PuppetLayer, PuppetSkeleton,
};
use crate::scene_assets::{material_texture_path, shader_stage_paths, MaterialFile};
use crate::scene_graph::{CameraState, SceneGraph, SceneLayer, SceneLayerKind};
use crate::scene_math::PassMatrices;
use crate::shader_preprocessor::{
    compile_we_shader_pair, parse_uniform_comment_metadata, preprocess_we_shader_pair,
    ReflectedUniformField, UniformCommentMetadata, WE_SAMPLER_BINDING_OFFSET,
};
use crate::text::{bitmap_size, load_font, rasterize, TextCache, TextKey, TextStyle};
use crate::video_runtime::{tightly_packed_rows, VideoFrame, VideoPixelFormat};

// ------------------------------------------------------------------------------------------
// Pure helpers
// ------------------------------------------------------------------------------------------

/// Converts a decoded video frame into tightly packed RGBA8 rows of exactly `width x height`
/// (nearest-neighbour scaling when the sizes differ; BGRA frames are swizzled). Used to feed the
/// placeholder texture of an embedded MP4 `.tex`. Returns `None` for invalid frames.
pub fn video_frame_to_rgba(frame: &VideoFrame, width: u32, height: u32) -> Option<Vec<u8>> {
    if !frame.is_valid() || width == 0 || height == 0 {
        return None;
    }
    let packed = tightly_packed_rows(frame);
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    let (w, h) = (width as usize, height as usize);
    let bgra = frame.format == VideoPixelFormat::Bgra8;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = (y * fh / h).min(fh - 1);
        for x in 0..w {
            let sx = (x * fw / w).min(fw - 1);
            let s = (sy * fw + sx) * 4;
            let d = (y * w + x) * 4;
            let px = &packed[s..s + 4];
            if bgra {
                out[d..d + 4].copy_from_slice(&[px[2], px[1], px[0], px[3]]);
            } else {
                out[d..d + 4].copy_from_slice(px);
            }
        }
    }
    Some(out)
}

/// Texture format of embedded-video textures. Platform decoders deliver BGRA, so a frame of the
/// texture's size is written as is (same unorm encoding as the RGBA scene textures).
pub const EMBEDDED_VIDEO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// How a decoded frame reaches an embedded-video texture of `width x height`.
#[derive(Debug, PartialEq)]
pub enum VideoUpload<'a> {
    /// BGRA frame of exactly the texture size: uploaded directly with its row stride.
    Direct { data: &'a [u8], bytes_per_row: u32 },
    /// Any other frame: scaled / swizzled on the CPU into tightly packed BGRA rows.
    Converted(Vec<u8>),
}

/// Chooses the upload path for `frame` into an [`EMBEDDED_VIDEO_FORMAT`] texture. Returns
/// `None` for invalid frames or an empty texture.
pub fn plan_video_upload(frame: &VideoFrame, width: u32, height: u32) -> Option<VideoUpload<'_>> {
    if !frame.is_valid() || width == 0 || height == 0 {
        return None;
    }
    if frame.format == VideoPixelFormat::Bgra8 && frame.width == width && frame.height == height {
        if let Ok(bytes_per_row) = u32::try_from(frame.stride) {
            return Some(VideoUpload::Direct {
                data: &frame.data,
                bytes_per_row,
            });
        }
    }
    let mut bgra = video_frame_to_rgba(frame, width, height)?;
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Some(VideoUpload::Converted(bgra))
}

/// Layer quad vertex: `a_Position` (vec3, offset 0) + `a_TexCoord` (vec2, offset 12).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct LayerVertex {
    pub position: [f32; 3],
    pub texcoord: [f32; 2],
}
const _: () = assert!(std::mem::size_of::<LayerVertex>() == 20);

/// Two triangles over the 4 vertices of [`layer_quad_vertices`].
pub const QUAD_INDICES: [u16; 6] = [0, 1, 2, 2, 1, 3];

/// Quad centered on the origin in layer-local units (`±size/2`, y up). Order: bottom-left,
/// bottom-right, top-left, top-right. UV `(0,0)` is the top-left texel; UVs are multiplied by
/// `uv_scale` so NPOT padding is never sampled (pitfall #5).
pub fn layer_quad_vertices(size: Vec2, uv_scale: [f32; 2]) -> [LayerVertex; 4] {
    let (hw, hh) = (size.x * 0.5, size.y * 0.5);
    let [su, sv] = uv_scale;
    [
        LayerVertex {
            position: [-hw, -hh, 0.0],
            texcoord: [0.0, sv],
        },
        LayerVertex {
            position: [hw, -hh, 0.0],
            texcoord: [su, sv],
        },
        LayerVertex {
            position: [-hw, hh, 0.0],
            texcoord: [0.0, 0.0],
        },
        LayerVertex {
            position: [hw, hh, 0.0],
            texcoord: [su, 0.0],
        },
    ]
}

/// Orthographic projection mapping a layer-local quad of `size` onto a whole FBO.
pub fn layer_local_projection(size: Vec2) -> Mat4 {
    let (hw, hh) = (size.x.max(1.0) * 0.5, size.y.max(1.0) * 0.5);
    Mat4::orthographic_rh(-hw, hw, -hh, hh, -1.0, 1.0)
}

/// Set-1 layout entries for WE textures: `texture_2d` at binding `N` and its filtering sampler
/// at `N + 8` (matches `translate_we_shader_pair`).
pub fn texture_bind_group_layout_entries(texture_slots: &[u32]) -> Vec<wgpu::BindGroupLayoutEntry> {
    let vis = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
    let mut out = Vec::with_capacity(texture_slots.len() * 2);
    for &n in texture_slots {
        out.push(wgpu::BindGroupLayoutEntry {
            binding: n,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        out.push(wgpu::BindGroupLayoutEntry {
            binding: n + WE_SAMPLER_BINDING_OFFSET,
            visibility: vis,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    out
}

/// Texture slots `N` of every `uniform sampler2D g_TextureN` declared in `sources`, sorted.
pub fn reflect_texture_slots(sources: &[&str]) -> Vec<u32> {
    let mut slots = Vec::new();
    for src in sources {
        for line in src.lines() {
            let Some(rest) = line.trim().strip_prefix("uniform sampler2D ") else {
                continue;
            };
            let name = rest.split(['/', ';']).next().unwrap_or("").trim();
            if let Some(n) = name
                .strip_prefix("g_Texture")
                .and_then(|d| d.parse::<u32>().ok())
                .filter(|n| *n < 8)
            {
                if !slots.contains(&n) {
                    slots.push(n);
                }
            }
        }
    }
    slots.sort_unstable();
    slots
}

/// One `attribute <type> <name>;` declaration with the location `translate_we_shader_pair`
/// assigns (declaration order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAttributeDecl {
    pub location: u32,
    pub glsl_type: String,
    pub name: String,
}

/// Lists the vertex attributes of a preprocessed WE vertex shader in location order.
pub fn vertex_attribute_locations(vertex_src: &str) -> Vec<VertexAttributeDecl> {
    let mut out = Vec::new();
    for line in vertex_src.lines() {
        let Some(rest) = line.trim().strip_prefix("attribute ") else {
            continue;
        };
        let decl = rest
            .split("//")
            .next()
            .unwrap_or(rest)
            .trim()
            .trim_end_matches(';');
        let mut parts = decl.split_whitespace();
        let (Some(ty), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        out.push(VertexAttributeDecl {
            location: out.len() as u32,
            glsl_type: ty.to_string(),
            name: name.to_string(),
        });
    }
    out
}

/// Maps WE attributes onto [`LayerVertex`]: `a_Position` (vec3) and `a_TexCoord` (vec2).
/// Other attributes are not provided (the pipeline then fails validation and the pass falls
/// back to a plain copy).
pub fn layer_vertex_attributes(decls: &[VertexAttributeDecl]) -> Vec<wgpu::VertexAttribute> {
    decls
        .iter()
        .filter_map(|d| {
            let ty = d.glsl_type.as_str();
            match (d.name.as_str(), ty) {
                ("a_Position", "vec3" | "float3") => Some(wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: d.location,
                }),
                ("a_TexCoord", "vec2" | "float2") => Some(wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 12,
                    shader_location: d.location,
                }),
                _ => None,
            }
        })
        .collect()
}

/// Inputs for [`pack_uniform_block`].
#[derive(Debug, Clone)]
pub struct UniformValues {
    pub matrices: PassMatrices,
    pub time: f32,
    pub daytime: f32,
    pub pointer: Vec2,
    pub pointer_last: Vec2,
    /// `(1/scene_width, 1/scene_height)`.
    pub texel_size: Vec2,
    /// `g_TextureNResolution = (texture_w, texture_h, image_w, image_h)`.
    pub texture_resolutions: [Vec4; 8],
    pub alpha: f32,
    pub color: Vec3,
    pub brightness: f32,
    pub ambient_color: Vec3,
    pub skylight_color: Vec3,
    pub audio_left: [f32; 64],
    pub audio_right: [f32; 64],
    /// Non-builtin uniforms by GLSL name (see [`map_material_constants`]).
    pub constants: HashMap<String, Vec<f32>>,
}

impl Default for UniformValues {
    fn default() -> Self {
        Self {
            matrices: PassMatrices {
                model_view_projection: Mat4::IDENTITY,
                model_view_projection_inverse: Mat4::IDENTITY,
                model: Mat4::IDENTITY,
                model_inverse: Mat4::IDENTITY,
                view_projection: Mat4::IDENTITY,
            },
            time: 0.0,
            daytime: 0.5,
            pointer: Vec2::splat(0.5),
            pointer_last: Vec2::splat(0.5),
            texel_size: Vec2::new(1.0 / 1920.0, 1.0 / 1080.0),
            texture_resolutions: [Vec4::ONE; 8],
            alpha: 1.0,
            color: Vec3::ONE,
            brightness: 1.0,
            ambient_color: Vec3::splat(0.3),
            skylight_color: Vec3::splat(0.3),
            audio_left: [0.0; 64],
            audio_right: [0.0; 64],
            constants: HashMap::new(),
        }
    }
}

/// Averages groups of `64 / bands` bins (`bands` 16 or 32; 64 returns the input).
pub fn downsample_spectrum(src: &[f32; 64], bands: usize) -> Vec<f32> {
    let bands = bands.clamp(1, 64);
    let group = 64 / bands;
    (0..bands)
        .map(|i| src[i * group..(i + 1) * group].iter().sum::<f32>() / group as f32)
        .collect()
}

fn texture_index(name: &str, suffix: &str) -> Option<usize> {
    name.strip_prefix("g_Texture")?
        .strip_suffix(suffix)?
        .parse::<usize>()
        .ok()
        .filter(|n| *n < 8)
}

/// Built-in uniform values (`WE_BUILTIN_UNIFORMS`) as flat column-major floats.
pub fn builtin_uniform_floats(name: &str, v: &UniformValues) -> Option<Vec<f32>> {
    let m = &v.matrices;
    let cols = |x: Mat4| x.to_cols_array().to_vec();
    Some(match name {
        "g_ModelViewProjectionMatrix" | "g_EffectModelViewProjectionMatrix" => {
            cols(m.model_view_projection)
        }
        "g_ModelViewProjectionMatrixInverse" => cols(m.model_view_projection_inverse),
        "g_ModelMatrix" => cols(m.model),
        "g_ModelMatrixInverse" => cols(m.model_inverse),
        "g_ViewProjectionMatrix" => cols(m.view_projection),
        "g_Time" => vec![v.time],
        "g_Daytime" => vec![v.daytime],
        "g_PointerPosition" => v.pointer.to_array().to_vec(),
        "g_PointerPositionLast" => v.pointer_last.to_array().to_vec(),
        "g_TexelSize" => v.texel_size.to_array().to_vec(),
        "g_TexelSizeHalf" => (v.texel_size * 0.5).to_array().to_vec(),
        "g_Alpha" | "g_UserAlpha" => vec![v.alpha],
        "g_Brightness" => vec![v.brightness],
        "g_Color" | "g_CompositeColor" => v.color.to_array().to_vec(),
        "g_Color4" => v.color.extend(v.alpha).to_array().to_vec(),
        "g_LightAmbientColor" => v.ambient_color.to_array().to_vec(),
        "g_LightSkylightColor" => v.skylight_color.to_array().to_vec(),
        "g_OrientationUp" | "g_ViewUp" => vec![0.0, 1.0, 0.0],
        "g_OrientationRight" | "g_ViewRight" => vec![1.0, 0.0, 0.0],
        "g_OrientationForward" => vec![0.0, 0.0, 1.0],
        "g_EyePosition" => vec![0.0, 0.0, 1000.0],
        "g_Texture0Rotation" => vec![1.0, 0.0, 0.0, 1.0],
        "g_Texture0Translation" => vec![0.0, 0.0],
        "g_AudioSpectrum16Left" => downsample_spectrum(&v.audio_left, 16),
        "g_AudioSpectrum16Right" => downsample_spectrum(&v.audio_right, 16),
        "g_AudioSpectrum32Left" => downsample_spectrum(&v.audio_left, 32),
        "g_AudioSpectrum32Right" => downsample_spectrum(&v.audio_right, 32),
        "g_AudioSpectrum64Left" => v.audio_left.to_vec(),
        "g_AudioSpectrum64Right" => v.audio_right.to_vec(),
        other => {
            let n = texture_index(other, "Resolution")?;
            v.texture_resolutions[n].to_array().to_vec()
        }
    })
}

/// Number to floats: numbers, booleans, `"x y z"` strings, numeric arrays and `{ "value": .. }`.
pub fn json_value_floats(v: &Value) -> Option<Vec<f32>> {
    match v {
        Value::Number(n) => Some(vec![*n as f32]),
        Value::Bool(b) => Some(vec![if *b { 1.0 } else { 0.0 }]),
        Value::String(s) => {
            let c = parse_vec_components(s);
            (!c.is_empty()).then_some(c)
        }
        Value::Array(a) => {
            let c: Vec<f32> = a
                .iter()
                .filter_map(Value::as_f64)
                .map(|n| n as f32)
                .collect();
            (!c.is_empty()).then_some(c)
        }
        Value::Object(_) => v.get("value").and_then(json_value_floats),
        Value::Null => None,
    }
}

/// Maps material `constantshadervalues` (keyed by the uniform comment `material` key, or by the
/// uniform name) to GLSL uniform names, falling back to each uniform's comment `default`.
pub fn map_material_constants(
    metadata: &[UniformCommentMetadata],
    constants: &HashMap<String, Value>,
) -> HashMap<String, Vec<f32>> {
    let mut out = HashMap::new();
    for meta in metadata {
        if meta.glsl_type == "sampler2D" {
            continue;
        }
        let value = meta
            .material
            .as_deref()
            .and_then(|k| constants.get(k))
            .or_else(|| constants.get(&meta.name))
            .or(meta.default.as_ref());
        if let Some(f) = value.and_then(json_value_floats) {
            out.insert(meta.name.clone(), f);
        }
    }
    for (k, v) in constants {
        if k.starts_with("g_") && !out.contains_key(k) {
            if let Some(f) = json_value_floats(v) {
                out.insert(k.clone(), f);
            }
        }
    }
    out
}

fn put_bytes(buf: &mut [u8], at: usize, bytes: [u8; 4]) {
    if let Some(dst) = at.checked_add(4).and_then(|end| buf.get_mut(at..end)) {
        dst.copy_from_slice(&bytes);
    }
}

fn put_scalar(buf: &mut [u8], at: usize, v: f32, is_int: bool) {
    let bytes = if is_int {
        (v.round() as i32).to_le_bytes()
    } else {
        v.to_le_bytes()
    };
    put_bytes(buf, at, bytes);
}

fn type_components(ty: &str) -> usize {
    match ty {
        "float" | "int" | "uint" | "bool" => 1,
        "vec2" | "ivec2" | "float2" | "int2" => 2,
        "vec3" | "ivec3" | "float3" | "int3" => 3,
        "vec4" | "ivec4" | "float4" | "int4" => 4,
        "mat3" => 9,
        "mat4" => 16,
        _ => 4,
    }
}

/// Writes built-in uniforms and material constants into a std140 block at the offsets reflected
/// by `translate_we_shader_pair`. Arrays use a 16-byte element stride; `int`/`ivecN`/`bool`
/// fields are written as `i32`; `mat3` is written as 3 padded columns. Unknown uniforms stay 0.
pub fn pack_uniform_block(
    fields: &[ReflectedUniformField],
    size: usize,
    values: &UniformValues,
) -> Vec<u8> {
    let mut buf = vec![0u8; size.max(16)];
    for field in fields {
        let base = field.name.split('[').next().unwrap_or(&field.name).trim();
        let array_len = field
            .name
            .split_once('[')
            .and_then(|(_, rest)| rest.trim_end_matches(']').trim().parse::<usize>().ok());
        let Some(data) =
            builtin_uniform_floats(base, values).or_else(|| values.constants.get(base).cloned())
        else {
            continue;
        };
        let ty = field.glsl_type.as_str();
        let comps = type_components(ty);
        let is_int = matches!(ty, "int" | "uint" | "bool") || ty.starts_with("ivec");
        let off = field.byte_offset;
        if let Some(n) = array_len {
            let per = comps.min(4);
            for i in 0..n {
                let Some(elem) = data.get(i * per..) else {
                    break;
                };
                for (c, v) in elem.iter().take(per).enumerate() {
                    put_scalar(&mut buf, off + 16 * i + 4 * c, *v, is_int);
                }
            }
        } else if ty == "mat3" {
            let src_stride = if data.len() >= 16 { 4 } else { 3 };
            for col in 0..3 {
                for row in 0..3 {
                    if let Some(v) = data.get(col * src_stride + row) {
                        put_scalar(&mut buf, off + 16 * col + 4 * row, *v, false);
                    }
                }
            }
        } else {
            for (c, v) in data.iter().take(comps).enumerate() {
                put_scalar(&mut buf, off + 4 * c, *v, is_int);
            }
        }
    }
    buf
}

// ------------------------------------------------------------------------------------------
// Built-in WGSL
// ------------------------------------------------------------------------------------------

/// Textured quad used for composites, copies, solid layers and the final blit.
pub const SCENE_COMPOSITE_WGSL: &str = r#"
struct SimpleUniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: SimpleUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;
@group(1) @binding(0) var scene_backdrop: texture_2d<f32>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.position = u.mvp * vec4<f32>(v.position, 1.0);
    out.uv = v.uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_sampler, in.uv) * u.color;
}

@fragment
fn fs_layer_color(in: VsOut) -> @location(0) vec4<f32> {
    let src = textureSample(src_tex, src_sampler, in.uv);
    let dst = textureLoad(scene_backdrop, vec2<i32>(in.position.xy), 0);
    let alpha = clamp(src.a, 0.0, 1.0);
    let screen = src.rgb + dst.rgb - src.rgb * dst.rgb;
    let low = dst.rgb - (1.0 - 2.0 * src.rgb) * dst.rgb * (1.0 - dst.rgb);
    let high = dst.rgb + (2.0 * src.rgb - 1.0) * (sqrt(max(dst.rgb, vec3(0.0))) - dst.rgb);
    let soft_light = mix(low, high, step(vec3(0.5), src.rgb));
    let rgb = select(screen, soft_light, u.color.x == 12.0);
    return vec4(mix(dst.rgb, rgb, alpha), alpha + dst.a * (1.0 - alpha));
}
"#;

/// Sprite particle shader for the 68-byte [`SpriteParticleVertex`] layout.
pub const SCENE_PARTICLE_WGSL: &str = r#"
struct SimpleUniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: SimpleUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) texcoord_rot_size: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) velocity_life: vec4<f32>,
    @location(4) rotation_xy: vec2<f32>,
};
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let uv = v.texcoord_rot_size.xy;
    let half_size = v.texcoord_rot_size.w;
    let corner = vec2<f32>(uv.x - 0.5, 0.5 - uv.y) * 2.0 * half_size;
    let c = cos(v.texcoord_rot_size.z);
    let s = sin(v.texcoord_rot_size.z);
    let rotated = vec2<f32>(corner.x * c - corner.y * s, corner.x * s + corner.y * c);
    var out: VsOut;
    out.position = u.mvp * vec4<f32>(v.position + vec3<f32>(rotated, 0.0), 1.0);
    out.uv = uv;
    out.color = v.color * u.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_sampler, in.uv) * in.color;
}
"#;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SimpleUniforms {
    mvp: [f32; 16],
    color: [f32; 4],
}

impl SimpleUniforms {
    fn new(mvp: Mat4, color: Vec4) -> Self {
        Self {
            mvp: mvp.to_cols_array(),
            color: color.to_array(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BloomUniforms {
    texel_size: [f32; 2],
    strength: f32,
    threshold: f32,
}

// ------------------------------------------------------------------------------------------
// GPU state
// ------------------------------------------------------------------------------------------

/// Per-frame inputs for [`SceneRenderer::render`].
#[derive(Debug, Clone, Copy)]
pub struct FrameUniformInputs {
    pub time: f32,
    /// `[0,1]` on the monitor.
    pub pointer: Vec2,
    /// `[left, right]` 64-band spectrum.
    pub audio_64: [[f32; 64]; 2],
}

impl Default for FrameUniformInputs {
    fn default() -> Self {
        Self {
            time: 0.0,
            pointer: Vec2::splat(0.5),
            audio_64: [[0.0; 64]; 2],
        }
    }
}

/// Format of uploaded assets and per-layer effect FBOs.
const FBO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Format of the scene accumulation target, `_rt_FullFrameBuffer` and bloom targets. Half-float
/// so bright (HDR) values survive until bloom; the final blit converts to the output format.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct GpuTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

struct GpuTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    resolution: Vec4,
    uv_scale: [f32; 2],
    tex_format: Option<TexFormat>,
}

struct SimpleDraw {
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

struct GpuPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture_bind_group: wgpu::BindGroup,
    uniform_fields: Vec<ReflectedUniformField>,
    uniform_size: usize,
    constants: HashMap<String, Vec<f32>>,
    resolutions: [Vec4; 8],
    target: RenderTargetId,
}

enum GpuStep {
    DrawBase,
    Pass(Option<Box<GpuPass>>),
    Copy {
        draw: SimpleDraw,
        target: RenderTargetId,
    },
    Composite {
        draw: SimpleDraw,
        blending: BlendMode,
        source: RenderTargetId,
    },
}

struct GpuPuppet {
    skeleton: PuppetSkeleton,
    /// Model-space vertices (layer-local, y up).
    vertices: Vec<MdlPuppetVertex>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    /// Hash of the pose inputs (clips, blend, rate, time) last skinned and uploaded.
    pose_hash: Option<u64>,
    /// Reused CPU vertex scratch (no per-frame allocation).
    scratch: Vec<LayerVertex>,
}

struct GpuEffectLayer {
    plan: EffectChainPlan,
    targets: HashMap<RenderTargetId, GpuTarget>,
    base: Option<GpuPass>,
    base_copy: Option<SimpleDraw>,
    steps: Vec<GpuStep>,
    base_quad: wgpu::Buffer,
    effect_quad: wgpu::Buffer,
    puppet: Option<GpuPuppet>,
    is_composition: bool,
    samples_scene: bool,
    fullscreen: bool,
}

struct GpuParticles {
    draw: SimpleDraw,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    capacity: usize,
    blending: BlendMode,
}

/// A rasterized text layer: the bitmap is rebuilt only when the string or style changes.
struct GpuText {
    font: Arc<FontVec>,
    cache: TextCache,
    draw: SimpleDraw,
    quad: wgpu::Buffer,
    /// Size of the quad in layer units (the layer size, or the bitmap size for auto-sized text).
    quad_size: Vec2,
    /// Inputs of the last `update_text`; an identical call is a no-op (no allocation).
    last_input: Option<(String, TextStyle, Vec2)>,
    /// Keeps the bitmap texture alive for `draw`.
    _texture: wgpu::Texture,
}

enum GpuLayer {
    Text(Box<GpuText>),
    Effects(Box<GpuEffectLayer>),
    Solid(SimpleDraw),
    Particles(Box<GpuParticles>),
}

struct GpuBloom {
    targets: HashMap<&'static str, GpuTarget>,
    passes: Vec<(BloomPassPlan, wgpu::Buffer, wgpu::BindGroup)>,
    pipelines: HashMap<BloomStage, wgpu::RenderPipeline>,
}

/// Executes [`EffectChainPlan`]s, particles and bloom for a [`SceneGraph`].
pub struct SceneRenderer {
    target_format: wgpu::TextureFormat,
    scene: GpuTarget,
    full_frame: GpuTarget,
    simple_layout: wgpu::BindGroupLayout,
    ubo_layout: wgpu::BindGroupLayout,
    composite_module: wgpu::ShaderModule,
    color_blend_layout: wgpu::BindGroupLayout,
    color_blend_pipeline: wgpu::RenderPipeline,
    color_blend_backdrop: wgpu::BindGroup,
    particle_module: wgpu::ShaderModule,
    bloom_module: wgpu::ShaderModule,
    composite_pipelines: HashMap<(wgpu::TextureFormat, BlendMode), wgpu::RenderPipeline>,
    particle_pipelines: HashMap<BlendMode, wgpu::RenderPipeline>,
    clamp_sampler: wgpu::Sampler,
    repeat_sampler: wgpu::Sampler,
    white: GpuTexture,
    soft_particle: wgpu::TextureView,
    fullscreen_quad: wgpu::Buffer,
    quad_indices: wgpu::Buffer,
    textures: HashMap<String, GpuTexture>,
    /// Embedded MP4 `.tex` paths -> placeholder texture size.
    video_textures: HashMap<String, (u32, u32)>,
    layers: Vec<Option<GpuLayer>>,
    layer_outputs: HashMap<i32, GpuTarget>,
    bloom: Option<GpuBloom>,
    blit: SimpleDraw,
    last_pointer: Vec2,
    diagnostics: DiagnosticsLog,
}

fn block_on<F: Future>(device: &wgpu::Device, fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        let _ = device.poll(wgpu::Maintain::Wait);
        std::thread::yield_now();
    }
}

/// Runs `f` inside a validation error scope.
fn with_validation<T>(device: &wgpu::Device, f: impl FnOnce() -> T) -> Result<T, String> {
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let out = f();
    match block_on(device, device.pop_error_scope()) {
        Some(err) => Err(err.to_string()),
        None => Ok(out),
    }
}

fn create_target(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> GpuTarget {
    let (width, height) = (width.max(1), height.max(1));
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    GpuTarget {
        texture,
        view,
        width,
        height,
        format,
    }
}

fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    upload_texture(device, queue, label, width, height, FBO_FORMAT, rgba)
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    rgba: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// `images[0][0]` of a `.tex` decoded to RGBA8.
struct DecodedTex {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    resolution: Vec4,
    uv_scale: [f32; 2],
    format: TexFormat,
}

/// Result of decoding a `.tex` asset.
enum TexDecode {
    Image(DecodedTex),
    /// `FIF_MP4` texture: frames come from a platform decoder at runtime
    /// ([`SceneRenderer::upload_video_frame`]); `width`/`height` size the placeholder texture.
    EmbeddedVideo {
        width: u32,
        height: u32,
        resolution: Vec4,
    },
}

/// Largest edge of an embedded-video placeholder texture.
const MAX_EMBEDDED_VIDEO_DIMENSION: u32 = 4096;

fn decode_tex_file(bytes: &[u8]) -> Result<TexDecode, String> {
    let tex = parse_tex(bytes).map_err(|e| e.to_string())?;
    if tex.is_mp4() {
        let h_ = &tex.header;
        let pick = |image: u32, texture: u32| {
            let v = if image > 0 { image } else { texture };
            v.clamp(1, MAX_EMBEDDED_VIDEO_DIMENSION)
        };
        let width = pick(h_.image_width as u32, h_.texture_width as u32);
        let height = pick(h_.image_height as u32, h_.texture_height as u32);
        return Ok(TexDecode::EmbeddedVideo {
            width,
            height,
            resolution: Vec4::new(width as f32, height as f32, width as f32, height as f32),
        });
    }
    let mip = tex
        .images
        .first()
        .and_then(|i| i.first())
        .ok_or("texture has no mipmaps")?;
    let (rgba, w, h) = if tex.free_image_format != FIF_UNKNOWN {
        let img = image::load_from_memory(&mip.raw_bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let (w, h) = img.dimensions();
        (img.into_raw(), w, h)
    } else {
        let rgba = decode_tex_mipmap_to_rgba8(tex.header.format, tex.free_image_format, mip)
            .map_err(|e| e.to_string())?;
        (rgba, mip.width.max(1) as u32, mip.height.max(1) as u32)
    };
    if rgba.len() < (w as usize) * (h as usize) * 4 {
        return Err("decoded texture is smaller than its dimensions".into());
    }
    let h_ = &tex.header;
    // The header describes the original storage allocation. A decoded mip may already
    // be trimmed to the image dimensions, so UVs must follow the uploaded texture.
    let visible_width = u32::try_from(h_.image_width)
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(w)
        .min(w);
    let visible_height = u32::try_from(h_.image_height)
        .ok()
        .filter(|v| *v > 0)
        .unwrap_or(h)
        .min(h);
    let resolution = Vec4::new(
        w as f32,
        h as f32,
        visible_width as f32,
        visible_height as f32,
    );
    Ok(TexDecode::Image(DecodedTex {
        rgba,
        width: w,
        height: h,
        resolution,
        uv_scale: [resolution.z / resolution.x, resolution.w / resolution.y],
        format: h_.format,
    }))
}

fn simple_layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    let vis = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: vis,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

fn layer_vertex_layout(attrs: &[wgpu::VertexAttribute]) -> wgpu::VertexBufferLayout<'_> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<LayerVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: attrs,
    }
}

const COMPOSITE_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2];
const PARTICLE_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x2
];

fn run_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
    f: impl FnOnce(&mut wgpu::RenderPass<'_>),
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: match clear {
                    Some(c) => wgpu::LoadOp::Clear(c),
                    None => wgpu::LoadOp::Load,
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    f(&mut pass);
}

/// A texture view plus its `g_TextureNResolution` and sampler choice.
struct SlotBinding<'a> {
    view: &'a wgpu::TextureView,
    resolution: Vec4,
    repeat: bool,
}

impl SceneRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        scene_size: Vec2,
    ) -> Self {
        let (w, h) = (
            scene_size.x.round().max(1.0) as u32,
            scene_size.y.round().max(1.0) as u32,
        );
        let simple_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_simple_bgl"),
            entries: &simple_layout_entries(),
        });
        let ubo_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_we_ubo_bgl"),
            entries: &[simple_layout_entries()[0]],
        });
        let module = |label: &str, src: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.to_string().into()),
            })
        };
        let composite_module = module("scene_composite", SCENE_COMPOSITE_WGSL);
        let particle_module = module("scene_particles", SCENE_PARTICLE_WGSL);
        let bloom_module = module("bloom.wgsl", crate::BLOOM_WGSL);
        let sampler = |label: &str, mode: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            })
        };
        let clamp_sampler = sampler("scene_clamp", wgpu::AddressMode::ClampToEdge);
        let repeat_sampler = sampler("scene_repeat", wgpu::AddressMode::Repeat);
        let (white_tex, white_view) = upload_rgba(device, queue, "scene_white", 1, 1, &[255; 4]);
        let white = GpuTexture {
            texture: white_tex,
            view: white_view,
            resolution: Vec4::ONE,
            uv_scale: [1.0, 1.0],
            tex_format: None,
        };
        const SPRITE_SIZE: u32 = 32;
        let mut sprite = Vec::with_capacity((SPRITE_SIZE * SPRITE_SIZE * 4) as usize);
        for y in 0..SPRITE_SIZE {
            for x in 0..SPRITE_SIZE {
                let uv = (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) / SPRITE_SIZE as f32;
                let falloff = (1.0 - (uv * 2.0 - Vec2::ONE).length()).max(0.0);
                sprite.extend_from_slice(&[255, 255, 255, (255.0 * falloff.powf(1.5)) as u8]);
            }
        }
        let (_, soft_particle) = upload_rgba(
            device,
            queue,
            "scene_soft_particle",
            SPRITE_SIZE,
            SPRITE_SIZE,
            &sprite,
        );
        let fullscreen_quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_fullscreen_quad"),
            contents: bytemuck::cast_slice(&layer_quad_vertices(Vec2::splat(2.0), [1.0, 1.0])),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let quad_indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_quad_indices"),
            contents: bytemuck::cast_slice(&QUAD_INDICES),
            usage: wgpu::BufferUsages::INDEX,
        });
        let scene = create_target(device, "scene_target", w, h, HDR_FORMAT);
        let full_frame = create_target(device, RT_FULL_FRAME_BUFFER, w, h, HDR_FORMAT);
        let color_blend_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("scene_color_blend_backdrop"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
        let color_blend_pipeline = Self::build_composite_pipeline(
            device,
            &composite_module,
            &[&simple_layout, &color_blend_layout],
            HDR_FORMAT,
            BlendMode::Normal,
            "fs_layer_color",
        );
        let color_blend_backdrop =
            Self::make_backdrop_group(device, &color_blend_layout, &full_frame.view);

        let mut renderer = Self {
            target_format,
            blit: Self::make_simple_draw_raw(
                device,
                &simple_layout,
                &scene.view,
                &clamp_sampler,
                SimpleUniforms::new(Mat4::IDENTITY, Vec4::ONE),
            ),
            scene,
            full_frame,
            simple_layout,
            ubo_layout,
            composite_module,
            color_blend_layout,
            color_blend_pipeline,
            color_blend_backdrop,
            particle_module,
            bloom_module,
            composite_pipelines: HashMap::new(),
            particle_pipelines: HashMap::new(),
            clamp_sampler,
            repeat_sampler,
            white,
            soft_particle,
            fullscreen_quad,
            quad_indices,
            textures: HashMap::new(),
            video_textures: HashMap::new(),
            layers: Vec::new(),
            layer_outputs: HashMap::new(),
            bloom: None,
            last_pointer: Vec2::splat(0.5),
            diagnostics: DiagnosticsLog::default(),
        };
        for blend in [
            BlendMode::Normal,
            BlendMode::Translucent,
            BlendMode::Additive,
            BlendMode::Multiply,
            BlendMode::Screen,
        ] {
            renderer.ensure_composite_pipeline(device, HDR_FORMAT, blend);
        }
        renderer.ensure_composite_pipeline(device, FBO_FORMAT, BlendMode::Normal);
        renderer.ensure_composite_pipeline(device, target_format, BlendMode::Normal);
        renderer
    }

    fn make_simple_draw_raw(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        uniforms: SimpleUniforms,
    ) -> SimpleDraw {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_simple_uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_simple_bg"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        SimpleDraw {
            uniform,
            bind_group,
        }
    }

    fn make_simple_draw(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> SimpleDraw {
        Self::make_simple_draw_raw(
            device,
            &self.simple_layout,
            view,
            &self.clamp_sampler,
            SimpleUniforms::new(Mat4::IDENTITY, Vec4::ONE),
        )
    }

    fn ensure_composite_pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        blend: BlendMode,
    ) {
        if self.composite_pipelines.contains_key(&(format, blend)) {
            return;
        }
        let pipeline = Self::build_composite_pipeline(
            device,
            &self.composite_module,
            &[&self.simple_layout],
            format,
            blend,
            "fs_main",
        );
        self.composite_pipelines.insert((format, blend), pipeline);
    }

    fn build_composite_pipeline(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        bind_group_layouts: &[&wgpu::BindGroupLayout],
        format: wgpu::TextureFormat,
        blend: BlendMode,
        fragment: &str,
    ) -> wgpu::RenderPipeline {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_composite_layout"),
            bind_group_layouts,
            push_constant_ranges: &[],
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene_composite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                buffers: &[layer_vertex_layout(&COMPOSITE_ATTRS)],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fragment),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: blend.to_wgpu_blend(),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    fn make_backdrop_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_color_blend_backdrop"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            }],
        })
    }

    fn ensure_particle_pipeline(&mut self, device: &wgpu::Device, blend: BlendMode) {
        if self.particle_pipelines.contains_key(&blend) {
            return;
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_particle_layout"),
            bind_group_layouts: &[&self.simple_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene_particles"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.particle_module,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SpriteParticleVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &PARTICLE_ATTRS,
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.particle_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: blend.to_wgpu_blend(),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        self.particle_pipelines.insert(blend, pipeline);
    }

    /// Loads a `.tex` asset into the cache. Returns `false` (and caches nothing) on failure.
    fn ensure_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &WeVirtualFileSystem,
        path: &str,
        warnings: &mut Vec<String>,
    ) -> bool {
        if self.textures.contains_key(path) {
            return true;
        }
        let decoded = vfs
            .read_bytes(path)
            .map_err(|e| e.to_string())
            .and_then(|b| decode_tex_file(&b));
        match decoded {
            Ok(TexDecode::Image(d)) => {
                let (texture, view) = upload_rgba(device, queue, path, d.width, d.height, &d.rgba);
                self.textures.insert(
                    path.to_string(),
                    GpuTexture {
                        texture,
                        view,
                        resolution: d.resolution,
                        uv_scale: d.uv_scale,
                        tex_format: Some(d.format),
                    },
                );
                true
            }
            Ok(TexDecode::EmbeddedVideo {
                width,
                height,
                resolution,
            }) => {
                // White until the host uploads the first decoded frame.
                let white = vec![255u8; width as usize * height as usize * 4];
                let (texture, view) = upload_texture(
                    device,
                    queue,
                    path,
                    width,
                    height,
                    EMBEDDED_VIDEO_FORMAT,
                    &white,
                );
                self.textures.insert(
                    path.to_string(),
                    GpuTexture {
                        texture,
                        view,
                        resolution,
                        uv_scale: [1.0, 1.0],
                        tex_format: None,
                    },
                );
                self.video_textures
                    .insert(path.to_string(), (width, height));
                true
            }
            Err(e) => {
                let msg = format!("texture {path}: {e}; using a fallback texture");
                tracing::warn!("{msg}");
                warnings.push(msg);
                self.diagnostics.push(format!(
                    "Texture {} unreadable; a placeholder is drawn instead",
                    short_name(path)
                ));
                false
            }
        }
    }

    /// VFS paths of every embedded MP4 `.tex` used by the loaded scene. Each one shows white
    /// until frames are pushed with [`Self::upload_video_frame`].
    pub fn embedded_video_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.video_textures.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// Writes a decoded frame into the embedded-video texture loaded for `path`. BGRA frames of
    /// the texture size are written directly; others are scaled / swizzled to the texture size
    /// (existing bind groups stay valid). Returns `false` when `path` is not an embedded video
    /// or the frame is invalid.
    pub fn upload_video_frame(&self, queue: &wgpu::Queue, path: &str, frame: &VideoFrame) -> bool {
        let (Some(&(width, height)), Some(tex)) =
            (self.video_textures.get(path), self.textures.get(path))
        else {
            return false;
        };
        let Some(upload) = plan_video_upload(frame, width, height) else {
            return false;
        };
        let (data, bytes_per_row): (&[u8], u32) = match &upload {
            VideoUpload::Direct {
                data,
                bytes_per_row,
            } => (data, *bytes_per_row),
            VideoUpload::Converted(bgra) => (bgra, 4 * width),
        };
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &tex.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        true
    }

    fn asset_binding(&self, path: &str) -> SlotBinding<'_> {
        match self.textures.get(path) {
            Some(t) => SlotBinding {
                view: &t.view,
                resolution: t.resolution,
                repeat: true,
            },
            None => self.white_binding(),
        }
    }

    fn white_binding(&self) -> SlotBinding<'_> {
        SlotBinding {
            view: &self.white.view,
            resolution: Vec4::ONE,
            repeat: false,
        }
    }

    fn target_binding<'a>(
        &'a self,
        id: &RenderTargetId,
        targets: &'a HashMap<RenderTargetId, GpuTarget>,
    ) -> SlotBinding<'a> {
        let t = match id {
            RenderTargetId::FullFrame => Some(&self.full_frame),
            RenderTargetId::Scene => Some(&self.scene),
            RenderTargetId::LayerOutput(id) => self.layer_outputs.get(id),
            other => targets.get(other),
        };
        match t {
            Some(t) => SlotBinding {
                view: &t.view,
                resolution: Vec4::new(
                    t.width as f32,
                    t.height as f32,
                    t.width as f32,
                    t.height as f32,
                ),
                repeat: false,
            },
            None => self.white_binding(),
        }
    }

    /// Preprocesses, compiles and builds the pipeline + bind groups for one WE material pass.
    #[allow(clippy::too_many_arguments)]
    fn build_we_pass(
        &self,
        device: &wgpu::Device,
        vfs: &WeVirtualFileSystem,
        shader: &str,
        combos: &HashMap<String, i32>,
        constants: &HashMap<String, Value>,
        bindings: &BTreeMap<u32, SlotBinding<'_>>,
        tex0_format: Option<TexFormat>,
        target: RenderTargetId,
        target_format: wgpu::TextureFormat,
        blending: BlendMode,
    ) -> Result<GpuPass, String> {
        let mut bound = [false; 8];
        for slot in bindings.keys() {
            if let Some(b) = bound.get_mut(*slot as usize) {
                *b = true;
            }
        }
        let (vpath, fpath) = shader_stage_paths(shader);
        let (vert, frag) =
            preprocess_we_shader_pair(vfs, &vpath, &fpath, combos, tex0_format, &bound)
                .map_err(|e| format!("{vpath}, {fpath}: {e}"))?;
        let compiled = with_validation(device, || {
            compile_we_shader_pair(device, shader, &vert, &frag)
        })??;
        let slots = reflect_texture_slots(&[&vert, &frag]);
        let metadata = parse_uniform_comment_metadata(&format!("{vert}\n{frag}"));
        let constants = map_material_constants(&metadata, constants);
        let attrs = layer_vertex_attributes(&vertex_attribute_locations(&vert));

        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene_we_textures_bgl"),
            entries: &texture_bind_group_layout_entries(&slots),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_we_layout"),
            bind_group_layouts: &[&self.ubo_layout, &tex_layout],
            push_constant_ranges: &[],
        });
        let pipeline = with_validation(device, || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(shader),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &compiled.vertex_module,
                    entry_point: Some("main"),
                    buffers: &[layer_vertex_layout(&attrs)],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &compiled.fragment_module,
                    entry_point: Some("main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: blending.to_wgpu_blend(),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        })
        .map_err(|e| format!("pipeline {shader}: {e}"))?;

        let uniform_size = (compiled.uniform_buffer_size.max(16) + 15) & !15;
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_we_ubo"),
            size: uniform_size as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_we_ubo_bg"),
            layout: &self.ubo_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let mut resolutions = [Vec4::ONE; 8];
        let white = self.white_binding();
        let mut entries = Vec::with_capacity(slots.len() * 2);
        for &slot in &slots {
            let b = bindings.get(&slot).unwrap_or(&white);
            resolutions[slot as usize] = b.resolution;
            entries.push(wgpu::BindGroupEntry {
                binding: slot,
                resource: wgpu::BindingResource::TextureView(b.view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: slot + WE_SAMPLER_BINDING_OFFSET,
                resource: wgpu::BindingResource::Sampler(if b.repeat {
                    &self.repeat_sampler
                } else {
                    &self.clamp_sampler
                }),
            });
        }
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene_we_textures_bg"),
            layout: &tex_layout,
            entries: &entries,
        });
        Ok(GpuPass {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            texture_bind_group,
            uniform_fields: compiled.uniform_fields,
            uniform_size,
            constants,
            resolutions,
            target,
        })
    }

    /// Short human-readable notes about content skipped or approximated by the last
    /// [`Self::load`] (skipped effect passes, shaders that fell back to a copy, missing
    /// textures or fonts). Deduplicated, at most 50 entries.
    pub fn diagnostics(&self) -> Vec<String> {
        self.diagnostics.to_vec()
    }

    /// Uploads textures, compiles every pass shader and allocates FBOs for `graph`. Returns
    /// non-fatal warnings (failed shaders fall back to plain copies, missing textures to white).
    pub fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &WeVirtualFileSystem,
        graph: &SceneGraph,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        let (w, h) = (
            graph.scene_size.x.round().max(1.0) as u32,
            graph.scene_size.y.round().max(1.0) as u32,
        );
        if (self.scene.width, self.scene.height) != (w, h) {
            self.scene = create_target(device, "scene_target", w, h, HDR_FORMAT);
            self.full_frame = create_target(device, RT_FULL_FRAME_BUFFER, w, h, HDR_FORMAT);
            self.color_blend_backdrop =
                Self::make_backdrop_group(device, &self.color_blend_layout, &self.full_frame.view);
            self.blit = self.make_simple_draw(device, &self.scene.view);
        }
        self.layers.clear();
        self.layer_outputs.clear();
        self.bloom = None;
        self.diagnostics = DiagnosticsLog::default();

        let mut plans: Vec<_> = graph
            .layers
            .iter()
            .map(|layer| {
                matches!(
                    layer.kind,
                    SceneLayerKind::Image { .. } | SceneLayerKind::Composition { .. }
                )
                .then(|| EffectChainPlan::build(vfs, layer, &graph.user_props))
            })
            .collect();
        let referenced: std::collections::HashSet<_> = plans
            .iter()
            .flatten()
            .flat_map(|plan| plan.passes())
            .flat_map(|pass| pass.textures.values())
            .filter_map(|binding| match binding {
                TextureBinding::Target(RenderTargetId::LayerOutput(id)) => Some(*id),
                _ => None,
            })
            .collect();
        for (layer, plan) in graph.layers.iter().zip(&plans) {
            if let (Some(id), Some(plan)) = (layer.id, plan) {
                if referenced.contains(&id) {
                    self.layer_outputs.insert(
                        id,
                        create_target(
                            device,
                            "scene_layer_output",
                            plan.width,
                            plan.height,
                            FBO_FORMAT,
                        ),
                    );
                }
            }
        }
        let mut fonts: HashMap<Option<String>, Option<Arc<FontVec>>> = HashMap::new();
        for (layer, plan) in graph.layers.iter().zip(plans.drain(..)) {
            let gpu = match &layer.kind {
                SceneLayerKind::Text { style, .. } => {
                    let font = fonts
                        .entry(style.font.clone())
                        .or_insert_with(|| load_font(vfs, style.font.as_deref()).ok().map(Arc::new))
                        .clone();
                    match font {
                        Some(font) => Some(GpuLayer::Text(Box::new(
                            self.new_gpu_text(device, queue, font),
                        ))),
                        None => {
                            let msg = format!(
                                "text layer '{}' skipped: no font could be loaded",
                                layer.key()
                            );
                            tracing::warn!("{msg}");
                            warnings.push(msg);
                            self.diagnostics.push(format!(
                                "Text layer '{}' skipped: no usable font",
                                layer.key()
                            ));
                            None
                        }
                    }
                }
                SceneLayerKind::Image { .. } | SceneLayerKind::Composition { .. } => self
                    .load_effect_layer(
                        device,
                        queue,
                        vfs,
                        layer,
                        plan.expect("image effect plan"),
                        &mut warnings,
                    )
                    .map(|l| GpuLayer::Effects(Box::new(l))),
                SceneLayerKind::Solid { .. } => Some(GpuLayer::Solid(Self::make_simple_draw_raw(
                    device,
                    &self.simple_layout,
                    &self.white.view,
                    &self.clamp_sampler,
                    SimpleUniforms::new(Mat4::IDENTITY, Vec4::ONE),
                ))),
                SceneLayerKind::Particle { spec, .. } => {
                    let tex_path = vfs
                        .read_string(&spec.material)
                        .ok()
                        .and_then(|s| MaterialFile::from_json_str(&s).ok())
                        .and_then(|m| m.passes.into_iter().next())
                        .map(|p| {
                            (
                                p.textures
                                    .first()
                                    .cloned()
                                    .flatten()
                                    .map(|t| material_texture_path(&t)),
                                BlendMode::parse(&p.blending),
                            )
                        });
                    let (tex_path, blending) = match tex_path {
                        Some((t, b)) => (t, b),
                        None => (None, BlendMode::Translucent),
                    };
                    let blending = if blending == BlendMode::Normal {
                        BlendMode::Translucent
                    } else {
                        blending
                    };
                    if let Some(p) = &tex_path {
                        self.ensure_texture(device, queue, vfs, p, &mut warnings);
                    }
                    self.ensure_particle_pipeline(device, blending);
                    let view = tex_path
                        .as_deref()
                        .and_then(|p| self.textures.get(p))
                        .map(|t| &t.view)
                        .unwrap_or(&self.soft_particle);
                    let draw = Self::make_simple_draw_raw(
                        device,
                        &self.simple_layout,
                        view,
                        &self.clamp_sampler,
                        SimpleUniforms::new(Mat4::IDENTITY, Vec4::ONE),
                    );
                    let capacity = spec.maxcount.clamp(1, 10_000) as usize;
                    let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("scene_particle_vertices"),
                        size: (capacity * 4 * std::mem::size_of::<SpriteParticleVertex>()) as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("scene_particle_indices"),
                        size: (capacity * 6 * 2) as u64,
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    Some(GpuLayer::Particles(Box::new(GpuParticles {
                        draw,
                        vertex_buffer,
                        index_buffer,
                        capacity,
                        blending,
                    })))
                }
                SceneLayerKind::Sound { .. } => None,
            };
            self.layers.push(gpu);
        }

        let bloom =
            BloomPlan::from_general(&graph.scene.general, &graph.user_props, graph.scene_size);
        if bloom.enabled {
            self.bloom = Some(self.build_bloom(device, bloom));
        }
        warnings
    }

    fn load_effect_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &WeVirtualFileSystem,
        layer: &SceneLayer,
        plan: EffectChainPlan,
        warnings: &mut Vec<String>,
    ) -> Option<GpuEffectLayer> {
        warnings.extend(plan.warnings.iter().cloned());
        let is_composition = matches!(layer.kind, SceneLayerKind::Composition { .. });
        let fullscreen = matches!(&layer.kind, SceneLayerKind::Composition { model, model_path, .. }
            if model.fullscreen == Some(true) || model_path.eq_ignore_ascii_case("models/util/fullscreenlayer.json"));
        let samples_scene = is_composition
            && !plan.passes().any(|pass| {
                pass.textures.values().any(|binding| {
                    matches!(
                        binding,
                        TextureBinding::Target(RenderTargetId::LayerOutput(_))
                    )
                })
            });

        // Phase 1 (mutable): textures and pipelines.
        let mut asset_paths: Vec<String> = Vec::new();
        if let SceneLayerKind::Image { texture_paths, .. } = &layer.kind {
            asset_paths.extend(
                texture_paths
                    .iter()
                    .flatten()
                    .filter(|p| !p.starts_with("_rt_"))
                    .cloned(),
            );
        }
        for pass in plan.passes() {
            for b in pass.textures.values() {
                if let TextureBinding::Asset(p) = b {
                    asset_paths.push(p.clone());
                }
            }
        }
        for p in &asset_paths {
            self.ensure_texture(device, queue, vfs, p, warnings);
        }
        for fbo in plan.all_fbos() {
            self.ensure_composite_pipeline(device, fbo.format, BlendMode::Normal);
        }

        // Phase 2 (immutable): targets, passes, bind groups.
        let mut targets = HashMap::new();
        for fbo in plan.all_fbos() {
            let label = format!("{}:{:?}", layer.key(), fbo.id);
            targets.insert(
                fbo.id.clone(),
                create_target(device, &label, fbo.width, fbo.height, fbo.format),
            );
        }
        let target_format = |id: &RenderTargetId| match id {
            RenderTargetId::FullFrame | RenderTargetId::Scene => Some(HDR_FORMAT),
            other => targets.get(other).map(|t: &GpuTarget| t.format),
        };
        let layer_size = Vec2::new(plan.width as f32, plan.height as f32);

        let mut local_diag: Vec<String> = Vec::new();
        local_diag.extend(plan.diagnostics.iter().cloned());
        let (mut base, mut base_copy, mut base_uv) = (None, None, [1.0f32, 1.0]);
        if samples_scene {
            base_copy = Some(self.make_simple_draw(device, &self.full_frame.view));
        } else if let SceneLayerKind::Image {
            material,
            texture_paths,
            ..
        } = &layer.kind
        {
            let mut bindings = BTreeMap::new();
            for (slot, path) in texture_paths.iter().enumerate().take(8) {
                if let Some(p) = path {
                    let b = if p == RT_FULL_FRAME_BUFFER {
                        self.target_binding(&RenderTargetId::FullFrame, &targets)
                    } else if p.starts_with("_rt_") {
                        self.white_binding()
                    } else {
                        self.asset_binding(p)
                    };
                    bindings.insert(slot as u32, b);
                }
            }
            let tex0 = texture_paths
                .first()
                .cloned()
                .flatten()
                .and_then(|p| self.textures.get(&p));
            base_uv = tex0.map(|t| t.uv_scale).unwrap_or([1.0, 1.0]);
            let tex0_format = tex0.and_then(|t| t.tex_format);
            let tex0_view = tex0.map(|t| &t.view).unwrap_or(&self.white.view);
            if let Some(mpass) = material.as_ref().and_then(|m| m.passes.first()) {
                match self.build_we_pass(
                    device,
                    vfs,
                    &mpass.shader,
                    &mpass.combos,
                    &mpass.constantshadervalues,
                    &bindings,
                    tex0_format,
                    RenderTargetId::FboA,
                    FBO_FORMAT,
                    BlendMode::Normal,
                ) {
                    Ok(p) => base = Some(p),
                    Err(e) => {
                        let msg = format!(
                            "layer '{}' base shader: {e}; using a plain copy",
                            layer.key()
                        );
                        tracing::warn!("{msg}");
                        warnings.push(msg);
                        local_diag.push(format!(
                            "Layer '{}' shader {} failed to compile; drawn as a plain copy",
                            layer.key(),
                            short_name(&mpass.shader)
                        ));
                    }
                }
            }
            if base.is_none() {
                base_copy = Some(self.make_simple_draw(device, tex0_view));
            }
        }

        let mut steps = Vec::with_capacity(plan.steps.len());
        let mut failed_effects = std::collections::HashSet::new();
        for step in &plan.steps {
            steps.push(match step {
                EffectStep::DrawBase { .. } => GpuStep::DrawBase,
                EffectStep::Pass(p) => match target_format(&p.target) {
                    None => GpuStep::Pass(None),
                    Some(fmt) => {
                        let mut bindings = BTreeMap::new();
                        for (slot, b) in &p.textures {
                            bindings.insert(
                                *slot,
                                match b {
                                    TextureBinding::Target(id) => self.target_binding(id, &targets),
                                    TextureBinding::Asset(path) => self.asset_binding(path),
                                },
                            );
                        }
                        let tex0_format = match p.textures.get(&0) {
                            Some(TextureBinding::Asset(path)) => {
                                self.textures.get(path).and_then(|t| t.tex_format)
                            }
                            _ => None,
                        };
                        match self.build_we_pass(
                            device,
                            vfs,
                            &p.shader,
                            &p.combos,
                            &p.constants,
                            &bindings,
                            tex0_format,
                            p.target.clone(),
                            fmt,
                            p.blending,
                        ) {
                            Ok(pass) => GpuStep::Pass(Some(Box::new(pass))),
                            Err(e) => {
                                let msg = format!(
                                    "layer '{}' effect {} pass {}: {e}; pass skipped",
                                    layer.key(),
                                    p.effect_file,
                                    p.pass_index
                                );
                                tracing::warn!("{msg}");
                                warnings.push(msg);
                                failed_effects.insert(p.effect_index);
                                local_diag.push(format!(
                                    "Effect {} skipped: shader {} failed to compile",
                                    short_name(&p.effect_file),
                                    short_name(&p.shader)
                                ));
                                if p.swaps {
                                    let source = if p.target == RenderTargetId::FboA {
                                        RenderTargetId::FboB
                                    } else {
                                        RenderTargetId::FboA
                                    };
                                    GpuStep::Copy {
                                        draw: self.make_simple_draw(
                                            device,
                                            self.target_binding(&source, &targets).view,
                                        ),
                                        target: p.target.clone(),
                                    }
                                } else {
                                    GpuStep::Pass(None)
                                }
                            }
                        }
                    }
                },
                EffectStep::Copy { source, target } => GpuStep::Copy {
                    draw: self.make_simple_draw(device, self.target_binding(source, &targets).view),
                    target: target.clone(),
                },
                EffectStep::Composite { source, blending } => GpuStep::Composite {
                    draw: self.make_simple_draw(device, self.target_binding(source, &targets).view),
                    blending: *blending,
                    source: source.clone(),
                },
            });
        }

        // A multi-pass effect with one broken pass would feed its later passes empty or
        // wrongly sized intermediates (tiled, shrunken copies of the layer). Skip it as a whole.
        if !failed_effects.is_empty() {
            for (step, gpu) in plan.steps.iter().zip(steps.iter_mut()) {
                let EffectStep::Pass(p) = step else { continue };
                if !failed_effects.contains(&p.effect_index) {
                    continue;
                }
                *gpu = if p.swaps {
                    let source = if p.target == RenderTargetId::FboA {
                        RenderTargetId::FboB
                    } else {
                        RenderTargetId::FboA
                    };
                    GpuStep::Copy {
                        draw: self
                            .make_simple_draw(device, self.target_binding(&source, &targets).view),
                        target: p.target.clone(),
                    }
                } else {
                    GpuStep::Pass(None)
                };
            }
        }

        let base_quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_layer_base_quad"),
            contents: bytemuck::cast_slice(&layer_quad_vertices(layer_size, base_uv)),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let effect_quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_layer_effect_quad"),
            contents: bytemuck::cast_slice(&layer_quad_vertices(layer_size, [1.0, 1.0])),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let puppet = match &layer.kind {
            SceneLayerKind::Image {
                puppet_path: Some(path),
                ..
            } => match Self::load_puppet(device, vfs, path) {
                Ok(p) => Some(p),
                Err(e) => {
                    let msg = format!(
                        "layer '{}' puppet {path}: {e}; drawing a flat quad",
                        layer.key()
                    );
                    tracing::warn!("{msg}");
                    warnings.push(msg);
                    local_diag.push(format!(
                        "Layer '{}' puppet animation unavailable; drawn as a flat image",
                        layer.key()
                    ));
                    None
                }
            },
            _ => None,
        };
        self.diagnostics.extend(local_diag);

        Some(GpuEffectLayer {
            plan,
            targets,
            base,
            base_copy,
            steps,
            base_quad,
            effect_quad,
            puppet,
            is_composition,
            samples_scene,
            fullscreen,
        })
    }

    fn new_gpu_text(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: Arc<FontVec>,
    ) -> GpuText {
        let (texture, view) = upload_rgba(device, queue, "scene_text", 1, 1, &[255, 255, 255, 0]);
        GpuText {
            font,
            cache: TextCache::default(),
            draw: self.make_simple_draw(device, &view),
            quad: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene_text_quad"),
                size: (4 * std::mem::size_of::<LayerVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            quad_size: Vec2::ONE,
            last_input: None,
            _texture: texture,
        }
    }

    /// Re-rasterizes a text layer when its string, style or size changed.
    #[allow(clippy::too_many_arguments)]
    fn update_text(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        simple_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        t: &mut GpuText,
        layer_size: Vec2,
        text: &str,
        style: &TextStyle,
    ) {
        if t.last_input
            .as_ref()
            .is_some_and(|(s, st, sz)| s == text && st == style && *sz == layer_size)
        {
            return;
        }
        t.last_input = Some((text.to_string(), style.clone(), layer_size));
        let (w, h) = bitmap_size(&t.font, text, style, (layer_size.x, layer_size.y));
        let key = TextKey {
            text: text.to_string(),
            style: style.clone(),
            width: w,
            height: h,
        };
        let font = t.font.clone();
        if t.cache.ensure(key, |k| {
            rasterize(&font, &k.text, k.width, k.height, &k.style)
        }) {
            if let Some(bmp) = t.cache.bitmap() {
                let (texture, view) = upload_rgba(
                    device,
                    queue,
                    "scene_text",
                    bmp.width,
                    bmp.height,
                    &bmp.rgba,
                );
                t.draw = Self::make_simple_draw_raw(
                    device,
                    simple_layout,
                    &view,
                    sampler,
                    SimpleUniforms::new(Mat4::IDENTITY, Vec4::ONE),
                );
                t._texture = texture;
            }
        }
        let valid = |v: f32| v.is_finite() && v >= 1.0;
        t.quad_size = if valid(layer_size.x) && valid(layer_size.y) {
            layer_size
        } else {
            Vec2::new(w as f32, h as f32)
        };
    }

    fn load_puppet(
        device: &wgpu::Device,
        vfs: &WeVirtualFileSystem,
        path: &str,
    ) -> Result<GpuPuppet, String> {
        let bytes = vfs.read_bytes(path).map_err(|e| e.to_string())?;
        // Layer size 0 keeps file coordinates (x, -y); flip y back to model space (y up).
        let mesh = parse_mdl(&bytes, [0.0, 0.0]).map_err(|e| e.to_string())?;
        let skeleton = parse_mdl_skeleton(&bytes, &mesh).map_err(|e| e.to_string())?;
        let vertices: Vec<MdlPuppetVertex> = mesh
            .vertices
            .iter()
            .map(|v| MdlPuppetVertex {
                position: [v.position[0], -v.position[1], v.position[2]],
                ..*v
            })
            .collect();
        let mut indices = mesh.indices.clone();
        if indices.len() % 2 == 1 {
            indices.push(0);
        }
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene_puppet_vertices"),
            size: (vertices.len().max(1) * std::mem::size_of::<LayerVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("scene_puppet_indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(GpuPuppet {
            skeleton,
            vertices,
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            pose_hash: None,
            scratch: Vec::new(),
        })
    }

    fn build_bloom(&self, device: &wgpu::Device, plan: BloomPlan) -> GpuBloom {
        let mut targets = HashMap::new();
        for p in &plan.passes {
            if p.target != RT_SCENE {
                targets.entry(p.target).or_insert_with(|| {
                    create_target(device, p.target, p.width, p.height, HDR_FORMAT)
                });
            }
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene_bloom_layout"),
            bind_group_layouts: &[&self.simple_layout],
            push_constant_ranges: &[],
        });
        let mut pipelines = HashMap::new();
        let mut passes = Vec::new();
        for p in &plan.passes {
            pipelines.entry(p.stage).or_insert_with(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(p.stage.fragment_entry_point()),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &self.bloom_module,
                        entry_point: Some(BLOOM_VERTEX_ENTRY_POINT),
                        buffers: &[],
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &self.bloom_module,
                        entry_point: Some(p.stage.fragment_entry_point()),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: HDR_FORMAT,
                            blend: p.blending.to_wgpu_blend(),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview: None,
                    cache: None,
                })
            });
            let source = if p.source == RT_SCENE {
                &self.scene
            } else {
                &targets[p.source]
            };
            let uniforms = BloomUniforms {
                texel_size: [1.0 / source.width as f32, 1.0 / source.height as f32],
                strength: plan.strength,
                threshold: plan.threshold,
            };
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scene_bloom_uniforms"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene_bloom_bg"),
                layout: &self.simple_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&source.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.clamp_sampler),
                    },
                ],
            });
            passes.push((p.clone(), buffer, bind_group));
        }
        GpuBloom {
            targets,
            passes,
            pipelines,
        }
    }

    fn write_we_uniforms(queue: &wgpu::Queue, pass: &GpuPass, base: &UniformValues) {
        let mut values = base.clone();
        values.texture_resolutions = pass.resolutions;
        values.constants = pass.constants.clone();
        let bytes = pack_uniform_block(&pass.uniform_fields, pass.uniform_size, &values);
        queue.write_buffer(&pass.uniform_buffer, 0, &bytes);
    }

    /// Draws `graph` into `target_view`: clears to `clearcolor`, runs every visible layer through
    /// its effect chain (copying the scene into `_rt_FullFrameBuffer` first when a pass binds
    /// it), draws particles, applies bloom and blits the scene target into `target_view`.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        graph: &SceneGraph,
        camera: &CameraState,
        frame: &FrameUniformInputs,
    ) {
        let pointer_last = self.last_pointer;
        self.last_pointer = frame.pointer;
        let daytime = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_secs() % 86_400) as f32 / 86_400.0)
            .unwrap_or(0.5);
        let general = &graph.scene.general;
        let base_values = UniformValues {
            time: frame.time,
            daytime,
            pointer: frame.pointer,
            pointer_last,
            texel_size: Vec2::new(
                1.0 / self.scene.width as f32,
                1.0 / self.scene.height as f32,
            ),
            ambient_color: general
                .ambientcolor
                .as_deref()
                .map(|s| truongpaper_we_formats::parse_vec3_or(s, Vec3::splat(0.3)))
                .unwrap_or(Vec3::splat(0.3)),
            skylight_color: general
                .skylightcolor
                .as_deref()
                .map(|s| truongpaper_we_formats::parse_vec3_or(s, Vec3::splat(0.3)))
                .unwrap_or(Vec3::splat(0.3)),
            audio_left: frame.audio_64[0],
            audio_right: frame.audio_64[1],
            ..UniformValues::default()
        };

        let clear = if graph.clear_enabled {
            wgpu::Color {
                r: graph.clear_color.x as f64,
                g: graph.clear_color.y as f64,
                b: graph.clear_color.z as f64,
                a: 1.0,
            }
        } else {
            wgpu::Color::TRANSPARENT
        };
        run_pass(
            encoder,
            "scene_clear",
            &self.scene.view,
            Some(clear),
            |_| {},
        );

        let layer_count = graph.layers.len().min(self.layers.len());
        for li in 0..layer_count {
            let layer = &graph.layers[li];
            if !layer.visible
                && layer
                    .id
                    .is_none_or(|id| !self.layer_outputs.contains_key(&id))
            {
                continue;
            }
            let matrices = graph.layer_pass_matrices(li, camera);
            if let (SceneLayerKind::Text { text, style }, Some(GpuLayer::Text(t))) =
                (&layer.kind, self.layers[li].as_mut())
            {
                Self::update_text(
                    device,
                    queue,
                    &self.simple_layout,
                    &self.clamp_sampler,
                    t,
                    layer.transform.size,
                    text,
                    style,
                );
            }
            match self.layers[li].as_mut() {
                Some(GpuLayer::Effects(fx)) => {
                    // Puppet deformation (CPU skinning).
                    if let Some(p) = fx.puppet.as_mut() {
                        let configured = &graph.scene.objects[layer.object_index].animation_layers;
                        let mut clips: Vec<_> = configured
                            .iter()
                            .filter(|clip| resolve_bound_bool(&clip.visible, &graph.user_props))
                            .map(|clip| PuppetLayer {
                                animation: clip.animation,
                                blend: resolve_bound_value(&clip.blend, &graph.user_props),
                                rate: resolve_bound_value(&clip.rate, &graph.user_props),
                                additive: clip.additive,
                            })
                            .collect();
                        if configured.is_empty() {
                            if let Some(clip) = p.skeleton.animations.first() {
                                clips.push(PuppetLayer {
                                    animation: clip.id,
                                    blend: 1.0,
                                    rate: 1.0,
                                    additive: false,
                                });
                            }
                        }
                        let pose_hash = {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            frame.time.to_bits().hash(&mut h);
                            for c in &clips {
                                c.animation.hash(&mut h);
                                c.blend.to_bits().hash(&mut h);
                                c.rate.to_bits().hash(&mut h);
                                c.additive.hash(&mut h);
                            }
                            h.finish()
                        };
                        // A paused scene (same time, same clips) keeps the uploaded pose.
                        if p.pose_hash != Some(pose_hash) {
                            p.pose_hash = Some(pose_hash);
                            let bones = layered_bone_matrices(&p.skeleton, &clips, frame.time);
                            let positions = PuppetDeformer::deform(&p.vertices, &bones, &[]);
                            p.scratch.clear();
                            p.scratch
                                .extend(positions.iter().zip(&p.vertices).map(|(pos, v)| {
                                    LayerVertex {
                                        position: *pos,
                                        // Base and effect passes already crop the source padding.
                                        texcoord: v.uv,
                                    }
                                }));
                            if !p.scratch.is_empty() {
                                queue.write_buffer(
                                    &p.vertex_buffer,
                                    0,
                                    bytemuck::cast_slice(&p.scratch),
                                );
                            }
                        }
                    }
                }
                Some(GpuLayer::Particles(ps)) => {
                    if let Some(sys) = graph.particle_system(li) {
                        let (mut verts, mut idx) = sys.build_sprite_vertices();
                        verts.truncate(ps.capacity * 4);
                        idx.truncate(ps.capacity * 6);
                        if !verts.is_empty() {
                            queue.write_buffer(&ps.vertex_buffer, 0, bytemuck::cast_slice(&verts));
                            queue.write_buffer(&ps.index_buffer, 0, bytemuck::cast_slice(&idx));
                        }
                    }
                }
                _ => {}
            }
            let Some(gpu) = self.layers[li].as_ref() else {
                continue;
            };
            match gpu {
                GpuLayer::Effects(fx) => {
                    self.render_effect_layer(queue, encoder, fx, layer, &matrices, &base_values);
                }
                GpuLayer::Text(t) => {
                    if !layer.visible {
                        continue;
                    }
                    queue.write_buffer(
                        &t.draw.uniform,
                        0,
                        bytemuck::bytes_of(&SimpleUniforms::new(
                            matrices.model_view_projection,
                            (layer.color * layer.brightness).extend(layer.alpha),
                        )),
                    );
                    queue.write_buffer(
                        &t.quad,
                        0,
                        bytemuck::cast_slice(&layer_quad_vertices(t.quad_size, [1.0, 1.0])),
                    );
                    let pipeline = &self.composite_pipelines[&(HDR_FORMAT, BlendMode::Translucent)];
                    run_pass(encoder, "scene_text", &self.scene.view, None, |rp| {
                        rp.set_pipeline(pipeline);
                        rp.set_bind_group(0, &t.draw.bind_group, &[]);
                        rp.set_vertex_buffer(0, t.quad.slice(..));
                        rp.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint16);
                        rp.draw_indexed(0..6, 0, 0..1);
                    });
                }
                GpuLayer::Solid(draw) => {
                    queue.write_buffer(
                        &draw.uniform,
                        0,
                        bytemuck::bytes_of(&SimpleUniforms::new(
                            matrices.model_view_projection,
                            (layer.color * layer.brightness).extend(layer.alpha),
                        )),
                    );
                    let pipeline = &self.composite_pipelines[&(HDR_FORMAT, BlendMode::Translucent)];
                    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("scene_solid_quad"),
                        contents: bytemuck::cast_slice(&layer_quad_vertices(
                            layer.transform.size,
                            [1.0, 1.0],
                        )),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    run_pass(encoder, "scene_solid", &self.scene.view, None, |rp| {
                        rp.set_pipeline(pipeline);
                        rp.set_bind_group(0, &draw.bind_group, &[]);
                        rp.set_vertex_buffer(0, vb.slice(..));
                        rp.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint16);
                        rp.draw_indexed(0..6, 0, 0..1);
                    });
                }
                GpuLayer::Particles(ps) => {
                    let count = graph
                        .particle_system(li)
                        .map(|s| s.particles.len().min(ps.capacity))
                        .unwrap_or(0);
                    if count == 0 {
                        continue;
                    }
                    queue.write_buffer(
                        &ps.draw.uniform,
                        0,
                        bytemuck::bytes_of(&SimpleUniforms::new(
                            matrices.model_view_projection,
                            (layer.color * layer.brightness).extend(layer.alpha),
                        )),
                    );
                    let pipeline = &self.particle_pipelines[&ps.blending];
                    run_pass(encoder, "scene_particles", &self.scene.view, None, |rp| {
                        rp.set_pipeline(pipeline);
                        rp.set_bind_group(0, &ps.draw.bind_group, &[]);
                        rp.set_vertex_buffer(0, ps.vertex_buffer.slice(..));
                        rp.set_index_buffer(ps.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                        rp.draw_indexed(0..(count * 6) as u32, 0, 0..1);
                    });
                }
            }
        }

        if let Some(bloom) = &self.bloom {
            for (p, _buffer, bind_group) in &bloom.passes {
                let view = if p.target == RT_SCENE {
                    &self.scene.view
                } else {
                    &bloom.targets[p.target].view
                };
                let clear = (p.target != RT_SCENE).then_some(wgpu::Color::BLACK);
                let pipeline = &bloom.pipelines[&p.stage];
                run_pass(encoder, p.stage.fragment_entry_point(), view, clear, |rp| {
                    rp.set_pipeline(pipeline);
                    rp.set_bind_group(0, bind_group, &[]);
                    rp.draw(0..3, 0..1);
                });
            }
        }

        let pipeline = &self.composite_pipelines[&(self.target_format, BlendMode::Normal)];
        run_pass(
            encoder,
            "scene_blit",
            target_view,
            Some(wgpu::Color::BLACK),
            |rp| {
                rp.set_pipeline(pipeline);
                rp.set_bind_group(0, &self.blit.bind_group, &[]);
                rp.set_vertex_buffer(0, self.fullscreen_quad.slice(..));
                rp.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint16);
                rp.draw_indexed(0..6, 0, 0..1);
            },
        );
    }

    fn render_effect_layer(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        fx: &GpuEffectLayer,
        layer: &SceneLayer,
        matrices: &PassMatrices,
        base_values: &UniformValues,
    ) {
        let color_blend = matches!(layer.color_blend_mode, 7 | 12) && layer.visible;
        if fx.plan.uses_full_frame_buffer || fx.samples_scene || color_blend {
            encoder.copy_texture_to_texture(
                self.scene.texture.as_image_copy(),
                self.full_frame.texture.as_image_copy(),
                wgpu::Extent3d {
                    width: self.scene.width,
                    height: self.scene.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let layer_size = Vec2::new(fx.plan.width as f32, fx.plan.height as f32);
        let local = layer_local_projection(layer_size);
        let local_values = UniformValues {
            matrices: PassMatrices {
                model_view_projection: local,
                model_view_projection_inverse: local.inverse(),
                model: Mat4::IDENTITY,
                model_inverse: Mat4::IDENTITY,
                view_projection: local,
            },
            alpha: layer.alpha,
            color: layer.color,
            brightness: layer.brightness,
            ..base_values.clone()
        };
        let target_view = |id: &RenderTargetId| -> Option<&wgpu::TextureView> {
            match id {
                RenderTargetId::Scene => Some(&self.scene.view),
                RenderTargetId::FullFrame => None,
                other => fx.targets.get(other).map(|t| &t.view),
            }
        };
        let target_format = |id: &RenderTargetId| match id {
            RenderTargetId::Scene | RenderTargetId::FullFrame => HDR_FORMAT,
            other => fx
                .targets
                .get(other)
                .map(|t| t.format)
                .unwrap_or(HDR_FORMAT),
        };
        let transparent = Some(wgpu::Color::TRANSPARENT);

        for step in &fx.steps {
            match step {
                GpuStep::DrawBase => {
                    let Some(view) = target_view(&RenderTargetId::FboA) else {
                        continue;
                    };
                    if let Some(base) = &fx.base {
                        Self::write_we_uniforms(queue, base, &local_values);
                        run_pass(encoder, "scene_layer_base", view, transparent, |rp| {
                            rp.set_pipeline(&base.pipeline);
                            rp.set_bind_group(0, &base.uniform_bind_group, &[]);
                            rp.set_bind_group(1, &base.texture_bind_group, &[]);
                            rp.set_vertex_buffer(0, fx.base_quad.slice(..));
                            rp.set_index_buffer(
                                self.quad_indices.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            rp.draw_indexed(0..6, 0, 0..1);
                        });
                    } else if let Some(draw) = &fx.base_copy {
                        let (mvp, vb, color) = if fx.is_composition {
                            (Mat4::IDENTITY, &self.fullscreen_quad, Vec4::ONE)
                        } else {
                            (
                                local,
                                &fx.base_quad,
                                (layer.color * layer.brightness).extend(layer.alpha),
                            )
                        };
                        queue.write_buffer(
                            &draw.uniform,
                            0,
                            bytemuck::bytes_of(&SimpleUniforms::new(mvp, color)),
                        );
                        let pipeline = &self.composite_pipelines[&(FBO_FORMAT, BlendMode::Normal)];
                        run_pass(encoder, "scene_layer_base_copy", view, transparent, |rp| {
                            rp.set_pipeline(pipeline);
                            rp.set_bind_group(0, &draw.bind_group, &[]);
                            rp.set_vertex_buffer(0, vb.slice(..));
                            rp.set_index_buffer(
                                self.quad_indices.slice(..),
                                wgpu::IndexFormat::Uint16,
                            );
                            rp.draw_indexed(0..6, 0, 0..1);
                        });
                    } else {
                        run_pass(
                            encoder,
                            "scene_layer_clear",
                            view,
                            Some(wgpu::Color::BLACK),
                            |_| {},
                        );
                    }
                }
                GpuStep::Pass(Some(pass)) => {
                    let Some(view) = target_view(&pass.target) else {
                        continue;
                    };
                    Self::write_we_uniforms(queue, pass, &local_values);
                    run_pass(encoder, "scene_effect_pass", view, transparent, |rp| {
                        rp.set_pipeline(&pass.pipeline);
                        rp.set_bind_group(0, &pass.uniform_bind_group, &[]);
                        rp.set_bind_group(1, &pass.texture_bind_group, &[]);
                        rp.set_vertex_buffer(0, fx.effect_quad.slice(..));
                        rp.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint16);
                        rp.draw_indexed(0..6, 0, 0..1);
                    });
                }
                GpuStep::Pass(None) => {}
                GpuStep::Copy { draw, target } => {
                    let Some(view) = target_view(target) else {
                        continue;
                    };
                    let Some(pipeline) = self
                        .composite_pipelines
                        .get(&(target_format(target), BlendMode::Normal))
                    else {
                        continue;
                    };
                    queue.write_buffer(
                        &draw.uniform,
                        0,
                        bytemuck::bytes_of(&SimpleUniforms::new(Mat4::IDENTITY, Vec4::ONE)),
                    );
                    run_pass(encoder, "scene_effect_copy", view, transparent, |rp| {
                        rp.set_pipeline(pipeline);
                        rp.set_bind_group(0, &draw.bind_group, &[]);
                        rp.set_vertex_buffer(0, self.fullscreen_quad.slice(..));
                        rp.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint16);
                        rp.draw_indexed(0..6, 0, 0..1);
                    });
                }
                GpuStep::Composite {
                    draw,
                    blending,
                    source,
                } => {
                    if let Some(output) = layer.id.and_then(|id| self.layer_outputs.get(&id)) {
                        if let Some(source) = fx.targets.get(source) {
                            encoder.copy_texture_to_texture(
                                source.texture.as_image_copy(),
                                output.texture.as_image_copy(),
                                output.texture.size(),
                            );
                        }
                    }
                    if !layer.visible {
                        continue;
                    }
                    queue.write_buffer(
                        &draw.uniform,
                        0,
                        bytemuck::bytes_of(&SimpleUniforms::new(
                            if fx.fullscreen {
                                Mat4::IDENTITY
                            } else {
                                matrices.model_view_projection
                            },
                            if color_blend {
                                Vec4::splat(layer.color_blend_mode as f32)
                            } else {
                                Vec4::ONE
                            },
                        )),
                    );
                    let blend = if fx.is_composition && !fx.samples_scene {
                        BlendMode::Translucent
                    } else {
                        *blending
                    };
                    let pipeline = if color_blend {
                        &self.color_blend_pipeline
                    } else {
                        &self.composite_pipelines[&(HDR_FORMAT, blend)]
                    };
                    run_pass(
                        encoder,
                        "scene_layer_composite",
                        &self.scene.view,
                        None,
                        |rp| {
                            rp.set_pipeline(pipeline);
                            rp.set_bind_group(0, &draw.bind_group, &[]);
                            if color_blend {
                                rp.set_bind_group(1, &self.color_blend_backdrop, &[]);
                            }
                            // Effects operate on the source image. Warp the finished image
                            // when drawing into the scene, where bones can move past the quad.
                            let (vb, ib, count) = match &fx.puppet {
                                Some(p) if p.index_count > 0 => {
                                    (&p.vertex_buffer, &p.index_buffer, p.index_count)
                                }
                                _ if fx.fullscreen => {
                                    (&self.fullscreen_quad, &self.quad_indices, 6)
                                }
                                _ => (&fx.effect_quad, &self.quad_indices, 6),
                            };
                            rp.set_vertex_buffer(0, vb.slice(..));
                            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint16);
                            rp.draw_indexed(0..count, 0, 0..1);
                        },
                    );
                }
            }
        }
    }
}
