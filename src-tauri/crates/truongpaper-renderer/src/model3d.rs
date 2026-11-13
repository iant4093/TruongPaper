//! 3D `model` scene objects: static `.mdl` meshes drawn with a perspective camera, a depth
//! buffer and a basic lit material (`texture * (ambient + lambert)` plus a masked emissive term).
//!
//! The scene projection/view and each layer's parent chain come from the scene graph
//! (`layer_pass_matrices`); this module only owns the GPU side: mesh buffers, pipelines (keyed by
//! blend/cull/depth state), the shared scene-sized depth target and the shader.

use std::cell::Cell;
use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use truongpaper_we_formats::StaticMdl;

use crate::json::Value;
use crate::scene_assets::MaterialPassSpec;
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelBlend {
    Opaque,
    Alpha,
    Additive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelCull {
    None,
    Back,
    Front,
}

/// Material state of a model draw (first pass of the mesh's material).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelMaterial {
    pub lit: bool,
    pub color: Vec3,
    pub brightness: f32,
    pub alpha: f32,
    /// `emissivecolor * emissivebrightness` (scaled by the mask texture's red channel).
    pub emissive: Vec3,
    pub blend: ModelBlend,
    pub cull: ModelCull,
    pub depth_test: bool,
    pub depth_write: bool,
}

fn floats(v: &Value) -> Option<Vec<f32>> {
    match v {
        Value::Number(n) => Some(vec![*n as f32]),
        Value::String(s) => s.split_whitespace().map(|t| t.parse().ok()).collect(),
        // `{ "user": ..., "value": ... }` / `{ "script": ..., "value": ... }`: use the default.
        Value::Object(_) => floats(v.get("value")?),
        _ => None,
    }
}

impl ModelMaterial {
    pub fn from_pass(pass: &MaterialPassSpec) -> Self {
        let c = &pass.constantshadervalues;
        let get = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| c.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)))
                .and_then(|(_, v)| floats(v))
        };
        let scalar = |k: &str, d: f32| get(&[k]).and_then(|v| v.first().copied()).unwrap_or(d);
        let vec3 = |k: &str, d: Vec3| {
            get(&[k])
                .filter(|v| v.len() >= 3)
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .unwrap_or(d)
        };
        let emissive_brightness = scalar("emissivebrightness", 0.0);
        Self {
            lit: pass.combos.get("LIGHTING").copied().unwrap_or(0) != 0,
            color: vec3("color", Vec3::ONE),
            brightness: scalar("brightness", 1.0),
            alpha: scalar("alpha", 1.0),
            emissive: vec3("emissivecolor", Vec3::ONE) * emissive_brightness,
            blend: match pass.blending.as_str() {
                // `normal` is plain alpha blending in Wallpaper Engine (opaque only when the
                // texture is); `none`/`opaque` disable blending.
                "none" | "opaque" => ModelBlend::Opaque,
                "additive" => ModelBlend::Additive,
                _ => ModelBlend::Alpha,
            },
            cull: match pass.cullmode.as_str() {
                "normal" | "back" => ModelCull::Back,
                "front" => ModelCull::Front,
                _ => ModelCull::None,
            },
            depth_test: pass.depthtest == "enabled",
            depth_write: pass.depthwrite == "enabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineKey {
    blend: ModelBlend,
    cull: ModelCull,
    depth_test: bool,
    depth_write: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelUniforms {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    tint: [f32; 4],
    /// rgb = emissive, w = 1 when lit.
    emissive: [f32; 4],
    ambient: [f32; 4],
    light_dir: [f32; 4],
    light_color: [f32; 4],
}

/// Per-frame inputs for one model draw.
#[derive(Debug, Clone, Copy)]
pub struct ModelFrame {
    pub mvp: Mat4,
    pub model: Mat4,
    /// Layer `color * brightness` (rgb) and `alpha` (w).
    pub tint: Vec4,
    pub ambient: Vec3,
    /// Direction the light travels (from the light towards the scene).
    pub light_dir: Vec3,
    pub light_color: Vec3,
}

/// A directional light used when a scene defines none (down and slightly toward the camera).
pub const DEFAULT_LIGHT_DIR: Vec3 = Vec3::new(-0.35, -0.6, -0.72);

pub struct GpuModel {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pub material: ModelMaterial,
}

struct DepthTarget {
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

/// Shared 3D state: shader, bind-group layout, pipelines and the scene-sized depth buffer.
pub struct ModelResources {
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    color_format: wgpu::TextureFormat,
    depth: DepthTarget,
    clear_pending: Cell<bool>,
}

const SHADER: &str = r#"
struct U {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    tint: vec4<f32>,
    emissive: vec4<f32>,
    ambient: vec4<f32>,
    light_dir: vec4<f32>,
    light_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var diffuse_tex: texture_2d<f32>;
@group(0) @binding(2) var mask_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = u.mvp * vec4<f32>(v.position, 1.0);
    o.normal = (u.model * vec4<f32>(v.normal, 0.0)).xyz;
    o.uv = v.uv;
    return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse_tex, samp, in.uv);
    var rgb = tex.rgb * u.tint.rgb;
    if (u.emissive.w > 0.5) {
        let n = normalize(in.normal);
        let lambert = max(dot(n, -normalize(u.light_dir.xyz)), 0.0);
        let mask = textureSample(mask_tex, samp, in.uv).r;
        rgb = rgb * (u.ambient.rgb + lambert * u.light_color.rgb) + u.emissive.rgb * mask;
    }
    let a = tex.a * u.tint.a;
    if (a < 0.004) {
        discard;
    }
    return vec4<f32>(rgb, a);
}
"#;

impl ModelResources {
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model3d_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("model3d_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model3d_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        Self {
            bgl,
            sampler,
            shader,
            pipelines: HashMap::new(),
            color_format,
            depth: Self::create_depth(device, width, height),
            clear_pending: Cell::new(true),
        }
    }

    fn create_depth(device: &wgpu::Device, width: u32, height: u32) -> DepthTarget {
        let (width, height) = (width.max(1), height.max(1));
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("model3d_depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        DepthTarget {
            view: tex.create_view(&wgpu::TextureViewDescriptor::default()),
            width,
            height,
        }
    }

    /// Depth buffer extent; it must match the color target it is drawn with.
    pub fn depth_size(&self) -> (u32, u32) {
        (self.depth.width, self.depth.height)
    }

    /// Call once per frame before the first model draw: the next draw clears depth.
    pub fn begin_frame(&self) {
        self.clear_pending.set(true);
    }

    fn ensure_pipeline(&mut self, device: &wgpu::Device, key: PipelineKey) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model3d_layout"),
            bind_group_layouts: &[&self.bgl],
            push_constant_ranges: &[],
        });
        let blend = match key.blend {
            ModelBlend::Opaque => None,
            ModelBlend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
            ModelBlend::Additive => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
        };
        let attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 24,
                shader_location: 2,
            },
        ];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model3d_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<ModelVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.color_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: match key.cull {
                    ModelCull::None => None,
                    ModelCull::Back => Some(wgpu::Face::Back),
                    ModelCull::Front => Some(wgpu::Face::Front),
                },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: key.depth_write,
                depth_compare: if key.depth_test {
                    wgpu::CompareFunction::LessEqual
                } else {
                    wgpu::CompareFunction::Always
                },
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        self.pipelines.insert(key, pipeline);
    }

    /// Uploads `mesh` and builds the bind group. `diffuse`/`mask` are sampled in the shader.
    pub fn create_model(
        &mut self,
        device: &wgpu::Device,
        mesh: &StaticMdl,
        material: ModelMaterial,
        diffuse: &wgpu::TextureView,
        mask: &wgpu::TextureView,
    ) -> GpuModel {
        let vertices: Vec<ModelVertex> = mesh
            .vertices
            .iter()
            .map(|v| ModelVertex {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
            })
            .collect();
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model3d_vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model3d_indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model3d_uniform"),
            size: std::mem::size_of::<ModelUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model3d_bind_group"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(diffuse),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(mask),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.ensure_pipeline(device, Self::key(&material));
        GpuModel {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            uniform,
            bind_group,
            material,
        }
    }

    fn key(m: &ModelMaterial) -> PipelineKey {
        PipelineKey {
            blend: m.blend,
            cull: m.cull,
            depth_test: m.depth_test,
            depth_write: m.depth_write,
        }
    }

    /// Draws `model` into `color` (the scene target), depth-testing against the shared buffer.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        model: &GpuModel,
        frame: &ModelFrame,
    ) {
        let m = &model.material;
        let tint = frame.tint
            * Vec4::new(m.color.x, m.color.y, m.color.z, m.alpha)
            * Vec4::new(m.brightness, m.brightness, m.brightness, 1.0);
        let uniforms = ModelUniforms {
            mvp: frame.mvp.to_cols_array_2d(),
            model: frame.model.to_cols_array_2d(),
            tint: tint.to_array(),
            emissive: [m.emissive.x, m.emissive.y, m.emissive.z, f32::from(m.lit)],
            ambient: frame.ambient.extend(0.0).to_array(),
            light_dir: frame.light_dir.extend(0.0).to_array(),
            light_color: frame.light_color.extend(0.0).to_array(),
        };
        queue.write_buffer(&model.uniform, 0, bytemuck::bytes_of(&uniforms));
        let Some(pipeline) = self.pipelines.get(&Self::key(m)) else {
            return;
        };
        let depth_load = if self.clear_pending.replace(false) {
            wgpu::LoadOp::Clear(1.0)
        } else {
            wgpu::LoadOp::Load
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene_model3d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth.view,
                depth_ops: Some(wgpu::Operations {
                    load: depth_load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &model.bind_group, &[]);
        pass.set_vertex_buffer(0, model.vertex_buffer.slice(..));
        pass.set_index_buffer(model.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..model.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(src: &str) -> MaterialPassSpec {
        crate::scene_assets::MaterialFile::from_json_str(&format!(r#"{{"passes":[{src}]}}"#))
            .unwrap()
            .passes
            .remove(0)
    }

    #[test]
    fn material_from_pass_reads_constants_and_state() {
        let m = ModelMaterial::from_pass(&pass(
            r#"{
            "shader": "generic4", "blending": "translucent", "cullmode": "normal",
            "depthtest": "enabled", "depthwrite": "enabled", "combos": { "LIGHTING": 1 },
            "constantshadervalues": {
                "brightness": { "user": "b", "value": 3 },
                "color": "0.5 0.25 1.0",
                "emissivebrightness": 2,
                "emissivecolor": { "script": "x", "value": "1 0 0" }
            } }"#,
        ));
        assert!(m.lit && m.depth_test && m.depth_write);
        assert_eq!((m.blend, m.cull), (ModelBlend::Alpha, ModelCull::Back));
        assert_eq!(m.brightness, 3.0);
        assert_eq!(m.color, Vec3::new(0.5, 0.25, 1.0));
        assert_eq!(m.emissive, Vec3::new(2.0, 0.0, 0.0));
        let d = ModelMaterial::from_pass(&pass(r#"{ "shader": "generic4" }"#));
        assert!(!d.lit && !d.depth_test);
        assert_eq!(d.cull, ModelCull::None);
        assert_eq!(d.blend, ModelBlend::Alpha);
    }

    #[test]
    fn perspective_projection_orders_depth_and_centres_target() {
        let proj = Mat4::perspective_rh(50f32.to_radians(), 16.0 / 9.0, 0.01, 10000.0);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y);
        let vp = proj * view;
        let near = vp.project_point3(Vec3::new(0.0, 0.0, 5.0));
        let far = vp.project_point3(Vec3::new(0.0, 0.0, -5.0));
        assert!(near.x.abs() < 1e-5 && near.y.abs() < 1e-5);
        assert!((0.0..1.0).contains(&near.z) && near.z < far.z && far.z < 1.0);
        // Model scale/rotation/translation compose as T * R * S.
        let model = Mat4::from_scale_rotation_translation(
            Vec3::splat(2.0),
            glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(1.0, 0.0, 0.0),
        );
        let p = model.transform_point3(Vec3::new(0.0, 0.0, 1.0));
        assert!((p - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-5);
    }
}
