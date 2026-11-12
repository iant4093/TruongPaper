//! Per-monitor `wgpu` 23 compositor host pipeline (`docs/01-SYSTEM-ARCHITECTURE.md` §3.3).
//!
//! Deviation from the doc: `wgpu::RenderPassColorAttachment::depth_slice` does not exist in
//! `wgpu` 23 (added in a later release), so those field initialisers were removed.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BlurUniforms {
    pub texel_size: [f32; 2],
    pub offset: f32,
    pub _pad: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct CompositorUniforms {
    pub progress: f32,
    pub transition_mode: u32,
    pub from_fit_mode: u32,
    pub to_fit_mode: u32,
    pub monitor_aspect: f32,
    pub from_image_aspect: f32,
    pub to_image_aspect: f32,
    pub zoom: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub hue_shift: f32,
    pub time: f32,
    pub _pad: f32,
    /// Span mode: this monitor's sub-rectangle of the shared canvas (`uv = offset + uv * scale`).
    /// Identity (`0,0` / `1,1`) for independent monitors.
    pub span_offset_x: f32,
    pub span_offset_y: f32,
    pub span_scale_x: f32,
    pub span_scale_y: f32,
}

impl Default for CompositorUniforms {
    fn default() -> Self {
        Self {
            progress: 1.0,
            transition_mode: 0,
            from_fit_mode: 0,
            to_fit_mode: 0,
            monitor_aspect: 16.0 / 9.0,
            from_image_aspect: 16.0 / 9.0,
            to_image_aspect: 16.0 / 9.0,
            zoom: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            hue_shift: 0.0,
            time: 0.0,
            _pad: 0.0,
            span_offset_x: 0.0,
            span_offset_y: 0.0,
            span_scale_x: 1.0,
            span_scale_y: 1.0,
        }
    }
}

pub struct CompositorPipeline {
    blur_bgl: wgpu::BindGroupLayout,
    compositor_bgl: wgpu::BindGroupLayout,
    blur_downsample_pipeline: wgpu::RenderPipeline,
    blur_upsample_pipeline: wgpu::RenderPipeline,
    compositor_pipeline: wgpu::RenderPipeline,
    sampler_linear: wgpu::Sampler,
    compositor_uniform_buf: wgpu::Buffer,
    blur_pass_uniform_bufs: [wgpu::Buffer; 4],
    ping_view: wgpu::TextureView,
    pong_view: wgpu::TextureView,
    pub from_blurred_view: wgpu::TextureView,
    pub to_blurred_view: wgpu::TextureView,
    pub blur_width: u32,
    pub blur_height: u32,
    pub surface_format: wgpu::TextureFormat,
}

impl CompositorPipeline {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        monitor_width: u32,
        monitor_height: u32,
    ) -> Self {
        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kawase_blur.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::KAWASE_BLUR_WGSL.into()),
        });
        let comp_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor.wgsl"),
            source: wgpu::ShaderSource::Wgsl(crate::COMPOSITOR_WGSL.into()),
        });

        let sampler_linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("compositor_linear_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let blur_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };

        let compositor_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compositor_bgl"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let blur_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur_pipeline_layout"),
            bind_group_layouts: &[&blur_bgl],
            push_constant_ranges: &[],
        });

        let comp_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compositor_pipeline_layout"),
            bind_group_layouts: &[&compositor_bgl],
            push_constant_ranges: &[],
        });

        let make_pipeline = |label: &str,
                             layout: &wgpu::PipelineLayout,
                             module: &wgpu::ShaderModule,
                             fs_entry: &str,
                             format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs_entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
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
        };

        let blur_format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let blur_downsample_pipeline = make_pipeline(
            "kawase_downsample",
            &blur_layout,
            &blur_shader,
            "fs_downsample",
            blur_format,
        );
        let blur_upsample_pipeline = make_pipeline(
            "kawase_upsample",
            &blur_layout,
            &blur_shader,
            "fs_upsample",
            blur_format,
        );
        let compositor_pipeline = make_pipeline(
            "compositor_main",
            &comp_layout,
            &comp_shader,
            "fs_main",
            surface_format,
        );

        let blur_width = (monitor_width / 4).max(64);
        let blur_height = (monitor_height / 4).max(64);
        let (ping_view, pong_view, from_blurred_view, to_blurred_view) =
            Self::create_blur_targets(device, blur_width, blur_height, blur_format);

        let texel_size = [1.0 / blur_width as f32, 1.0 / blur_height as f32];
        let offsets = [1.5_f32, 2.5_f32, 2.5_f32, 1.5_f32];
        let blur_pass_uniform_bufs = offsets.map(|offset| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("blur_uniform_buf"),
                contents: bytemuck::bytes_of(&BlurUniforms {
                    texel_size,
                    offset,
                    _pad: 0.0,
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        });

        let compositor_uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("compositor_uniform_buf"),
            contents: bytemuck::bytes_of(&CompositorUniforms::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        Self {
            blur_bgl,
            compositor_bgl,
            blur_downsample_pipeline,
            blur_upsample_pipeline,
            compositor_pipeline,
            sampler_linear,
            compositor_uniform_buf,
            blur_pass_uniform_bufs,
            ping_view,
            pong_view,
            from_blurred_view,
            to_blurred_view,
            blur_width,
            blur_height,
            surface_format,
        }
    }

    fn create_blur_targets(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> (
        wgpu::TextureView,
        wgpu::TextureView,
        wgpu::TextureView,
        wgpu::TextureView,
    ) {
        let make_tex = |label: &str| {
            device
                .create_texture(&wgpu::TextureDescriptor {
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
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        (
            make_tex("kawase_ping"),
            make_tex("kawase_pong"),
            make_tex("from_blurred"),
            make_tex("to_blurred"),
        )
    }

    /// Runs a 4-pass Dual-Kawase blur chain (2 downsample + 2 upsample passes) from `src_view`
    /// into either `self.from_blurred_view` (`is_target_to == false`) or `self.to_blurred_view` (`is_target_to == true`).
    pub fn precompute_kawase_blur(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        is_target_to: bool,
    ) {
        let final_dest = if is_target_to {
            &self.to_blurred_view
        } else {
            &self.from_blurred_view
        };

        let passes: [(&wgpu::TextureView, &wgpu::TextureView, bool, usize); 4] = [
            (src_view, &self.ping_view, true, 0),
            (&self.ping_view, &self.pong_view, true, 1),
            (&self.pong_view, &self.ping_view, false, 2),
            (&self.ping_view, final_dest, false, 3),
        ];

        for (input_view, output_view, is_down, buf_idx) in passes {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("kawase_pass_bg"),
                layout: &self.blur_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(input_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.blur_pass_uniform_bufs[buf_idx].as_entire_binding(),
                    },
                ],
            });

            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("kawase_blur_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rpass.set_pipeline(if is_down {
                &self.blur_downsample_pipeline
            } else {
                &self.blur_upsample_pipeline
            });
            rpass.set_bind_group(0, &bg, &[]);
            rpass.draw(0..3, 0..1);
        }
    }

    /// Renders the fitted, transitioned, and color-graded wallpaper frame into `target_surface_view`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        from_sharp_view: &wgpu::TextureView,
        to_sharp_view: &wgpu::TextureView,
        target_surface_view: &wgpu::TextureView,
        uniforms: &CompositorUniforms,
    ) {
        queue.write_buffer(
            &self.compositor_uniform_buf,
            0,
            bytemuck::bytes_of(uniforms),
        );

        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compositor_bg"),
            layout: &self.compositor_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(from_sharp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(to_sharp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.from_blurred_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.to_blurred_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_linear),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.compositor_uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("compositor_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_surface_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        rpass.set_pipeline(&self.compositor_pipeline);
        rpass.set_bind_group(0, &bg, &[]);
        rpass.draw(0..3, 0..1);
    }
}

// ---------------------------------------------------------------------------------------------
// Photo decode + upload (the host crate has no `image` dependency, so decoding lives here).
// ---------------------------------------------------------------------------------------------

/// Largest photo edge uploaded as a texture (`wgpu::Limits::default().max_texture_dimension_2d`).
pub const MAX_PHOTO_TEXTURE_DIMENSION: u32 = 8192;

/// A decoded photo: tightly packed straight-alpha RGBA8 rows (`width * height * 4` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImageData {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImageData {
    /// `width / height` (`CompositorUniforms::{from,to}_image_aspect`).
    pub fn aspect(&self) -> f32 {
        self.width.max(1) as f32 / self.height.max(1) as f32
    }
}

/// Decodes a cached photo (JPEG / PNG / WebP / GIF / BMP / TGA, sniffed from the content) to
/// RGBA8. Images whose longer edge exceeds `max_dimension` are downscaled (aspect preserved) so
/// they fit in a texture.
pub fn load_rgba_image(
    path: &std::path::Path,
    max_dimension: u32,
) -> anyhow::Result<RgbaImageData> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    let max = max_dimension.max(1);
    let img = if img.width() > max || img.height() > max {
        img.resize(max, max, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(RgbaImageData {
        width,
        height,
        pixels: rgba.into_raw(),
    })
}

/// Uploads a decoded photo as an `Rgba8UnormSrgb` texture usable as a compositor `from`/`to`
/// layer (and as a Kawase blur source).
pub fn upload_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &RgbaImageData,
    label: &str,
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: image.width.max(1),
                height: image.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &image.pixels,
    )
}

// ---------------------------------------------------------------------------------------------
// CPU-side helpers mirroring `compositor.wgsl` (not in the doc's §3.3 listing; they encode the
// constants documented in the WGSL comments and `docs/04` §"Transition Duration Slider" so the
// host can build `CompositorUniforms` without magic numbers, and so the UV math is unit-testable).
// ---------------------------------------------------------------------------------------------

/// `CompositorUniforms::{from,to}_fit_mode` values.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FitMode {
    /// 0 = Fill (Cover / center-crop).
    Fill = 0,
    /// 1 = Fit + Blurred Backdrop (dual Kawase).
    #[default]
    FitBlur = 1,
    /// 2 = Fit + Black Bars.
    FitBlack = 2,
    /// 3 = Stretch.
    Stretch = 3,
}

impl FitMode {
    /// Parses the IPC / settings string form (`'fill' | 'fit_blur' | 'fit_black' | 'stretch'`).
    pub fn from_setting_str(s: &str) -> Option<Self> {
        match s {
            "fill" => Some(Self::Fill),
            "fit_blur" => Some(Self::FitBlur),
            "fit_black" => Some(Self::FitBlack),
            "stretch" => Some(Self::Stretch),
            _ => None,
        }
    }
}

/// `CompositorUniforms::transition_mode` values.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransitionMode {
    /// 0 = Smooth Crossfade.
    Crossfade = 0,
    /// 1 = Blur-Dissolve.
    BlurDissolve = 1,
    /// 2 = Liquid Glass Morph.
    #[default]
    LiquidGlass = 2,
}

impl TransitionMode {
    /// Parses the settings string form (`AppSettings::transition_type`).
    pub fn from_setting_str(s: &str) -> Option<Self> {
        match s {
            "crossfade" | "smooth_crossfade" => Some(Self::Crossfade),
            "blur_dissolve" => Some(Self::BlurDissolve),
            "liquid_glass" | "liquid_glass_morph" => Some(Self::LiquidGlass),
            _ => None,
        }
    }
}

pub const MIN_TRANSITION_DURATION_SECS: f32 = 0.3;
pub const MAX_TRANSITION_DURATION_SECS: f32 = 3.0;
pub const DEFAULT_TRANSITION_DURATION_SECS: f32 = 1.2;

/// Clamps a user transition duration to `[0.3, 3.0]` seconds. Non-finite input yields the default.
pub fn clamp_transition_duration(secs: f32) -> f32 {
    if secs.is_finite() {
        secs.clamp(MIN_TRANSITION_DURATION_SECS, MAX_TRANSITION_DURATION_SECS)
    } else {
        DEFAULT_TRANSITION_DURATION_SECS
    }
}

/// Linear transition progress in `[0, 1]` for `elapsed` seconds into a (clamped) transition.
/// The shader applies `smoothstep` on top of this value.
pub fn transition_progress(elapsed_secs: f32, duration_secs: f32) -> f32 {
    let d = clamp_transition_duration(duration_secs);
    (elapsed_secs / d).clamp(0.0, 1.0)
}

/// What `sample_fitted_layer` in `compositor.wgsl` samples for a given screen UV.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FittedSample {
    /// Sample the sharp layer texture at this UV.
    Sharp([f32; 2]),
    /// Sample the Kawase-blurred backdrop at this UV (result dimmed by 0.78).
    Blurred([f32; 2]),
    /// Opaque black bar.
    Black,
}

/// Exact CPU mirror of `sample_fitted_layer` in `compositor.wgsl`.
pub fn fitted_layer_sample(
    screen_uv: [f32; 2],
    img_aspect: f32,
    fit_mode: FitMode,
    u: &CompositorUniforms,
) -> FittedSample {
    let clamp01 = |v: f32| v.clamp(0.0, 1.0);
    let zoom = u.zoom.max(0.05);
    let centered = [
        (screen_uv[0] - 0.5) / zoom - u.offset_x * 0.5,
        (screen_uv[1] - 0.5) / zoom - u.offset_y * 0.5,
    ];

    if fit_mode == FitMode::Stretch {
        return FittedSample::Sharp([clamp01(centered[0] + 0.5), clamp01(centered[1] + 0.5)]);
    }

    let ratio = u.monitor_aspect / img_aspect.max(0.001);
    // Cover scale (Fill + backdrop) and contain scale (Fit) are reciprocal-per-axis choices.
    let cover_scale = if ratio > 1.0 {
        [1.0, 1.0 / ratio]
    } else {
        [ratio, 1.0]
    };

    if fit_mode == FitMode::Fill {
        return FittedSample::Sharp([
            clamp01(centered[0] * cover_scale[0] + 0.5),
            clamp01(centered[1] * cover_scale[1] + 0.5),
        ]);
    }

    let fit_scale = if ratio > 1.0 {
        [ratio, 1.0]
    } else {
        [1.0, 1.0 / ratio]
    };
    let uv_fit = [
        centered[0] * fit_scale[0] + 0.5,
        centered[1] * fit_scale[1] + 0.5,
    ];
    let in_bounds = (0.0..=1.0).contains(&uv_fit[0]) && (0.0..=1.0).contains(&uv_fit[1]);

    if in_bounds {
        FittedSample::Sharp(uv_fit)
    } else if fit_mode == FitMode::FitBlur {
        FittedSample::Blurred([
            clamp01((screen_uv[0] - 0.5) * cover_scale[0] + 0.5),
            clamp01((screen_uv[1] - 0.5) * cover_scale[1] + 0.5),
        ])
    } else {
        FittedSample::Black
    }
}
