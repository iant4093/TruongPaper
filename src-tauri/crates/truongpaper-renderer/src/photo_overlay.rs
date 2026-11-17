//! Caption drawn over Photos API wallpapers: the time and date in the bottom-left corner, the
//! album name and the camera settings (EXIF) in the bottom-right corner.
//!
//! The text is rasterized on the CPU into a strip as wide as the display and drawn over the
//! bottom of the photo with alpha blending. It is only rebuilt when the text or the display size
//! changes (once a minute while the clock is shown).

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use std::sync::OnceLock;

/// What the caption shows. Empty fields are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayText {
    /// Large clock, e.g. `1:07 PM`.
    pub time: Option<String>,
    /// Small line under the clock, e.g. `Monday, November 16`.
    pub date: Option<String>,
    /// Album name in the bottom-right corner.
    pub album: Option<String>,
    /// Camera settings under the album name.
    pub exif: Option<String>,
}

impl OverlayText {
    pub fn is_empty(&self) -> bool {
        self.time.is_none() && self.date.is_none() && self.album.is_none() && self.exif.is_none()
    }
}

struct OverlayFonts {
    serif: FontVec,
    sans: FontVec,
}

const SERIF_FONTS: &[&str] = &[
    r"C:\Windows\Fonts\georgia.ttf",
    "/System/Library/Fonts/Supplemental/Georgia.ttf",
    "/Library/Fonts/Georgia.ttf",
    "/System/Library/Fonts/NewYork.ttf",
    r"C:\Windows\Fonts\times.ttf",
    "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
];

const SANS_FONTS: &[&str] = &[
    r"C:\Windows\Fonts\segoeui.ttf",
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    r"C:\Windows\Fonts\arial.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];

fn first_font(paths: &[&str]) -> Option<FontVec> {
    paths.iter().find_map(|p| {
        std::fs::read(p)
            .ok()
            .and_then(|bytes| FontVec::try_from_vec_and_index(bytes, 0).ok())
    })
}

fn fonts() -> Option<&'static OverlayFonts> {
    static FONTS: OnceLock<Option<OverlayFonts>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let sans = first_font(SANS_FONTS)?;
            let serif = first_font(SERIF_FONTS).or_else(|| first_font(SANS_FONTS))?;
            Some(OverlayFonts { serif, sans })
        })
        .as_ref()
}

/// Pixel layout of the caption for a `width` x `height` display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayLayout {
    /// Height of the strip at the bottom of the display.
    pub band_height: u32,
    pub margin: f32,
    pub time_px: f32,
    pub date_px: f32,
    pub album_px: f32,
    pub exif_px: f32,
}

impl OverlayLayout {
    pub fn for_display(width: u32, height: u32) -> Self {
        let short = width.min(height).max(1) as f32;
        let time_px = short * 0.075;
        let date_px = short * 0.022;
        Self {
            band_height: ((short * 0.24).round() as u32).clamp(1, height.max(1)),
            margin: short * 0.034,
            time_px,
            date_px,
            album_px: short * 0.037,
            exif_px: short * 0.02,
        }
    }
}

/// Text color (warm white) and the shadow that keeps it readable on bright photos.
const TEXT_RGB: [f32; 3] = [0.95, 0.93, 0.89];
const SHADOW_ALPHA: f32 = 0.55;
/// Darkening at the very bottom of the strip.
const GRADIENT_ALPHA: f32 = 0.42;

/// Straight-alpha RGBA strip of `width` x `layout.band_height` pixels, or `None` when no font
/// is available or there is nothing to draw.
pub fn rasterize_overlay(
    text: &OverlayText,
    width: u32,
    height: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    if text.is_empty() || width == 0 || height == 0 {
        return None;
    }
    let fonts = fonts()?;
    let layout = OverlayLayout::for_display(width, height);
    let (w, h) = (width as usize, layout.band_height as usize);
    let mut cover = vec![0f32; w * h];

    let bottom = h as f32 - layout.margin;
    let left = layout.margin;
    let right = width as f32 - layout.margin;

    // Left: clock above the date.
    let mut y = bottom;
    if let Some(date) = &text.date {
        draw_line(
            &mut cover,
            w,
            h,
            &fonts.sans,
            layout.date_px,
            date,
            left,
            y,
            false,
        );
        y -= layout.date_px * 1.7;
    }
    if let Some(time) = &text.time {
        draw_line(
            &mut cover,
            w,
            h,
            &fonts.serif,
            layout.time_px,
            time,
            left,
            y,
            false,
        );
    }
    // Right: album name above the camera settings.
    let mut y = bottom;
    if let Some(exif) = &text.exif {
        draw_line(
            &mut cover,
            w,
            h,
            &fonts.sans,
            layout.exif_px,
            exif,
            right,
            y,
            true,
        );
        y -= layout.exif_px * 1.9;
    }
    if let Some(album) = &text.album {
        draw_line(
            &mut cover,
            w,
            h,
            &fonts.serif,
            layout.album_px,
            album,
            right,
            y,
            true,
        );
    }

    let radius = ((layout.date_px * 0.35).round() as usize).max(2);
    let shadow = box_blur(&box_blur(&cover, w, h, radius), w, h, radius);

    let mut rgba = vec![0u8; w * h * 4];
    for row in 0..h {
        let t = row as f32 / (h.max(2) - 1) as f32;
        let gradient = GRADIENT_ALPHA * t * t;
        for col in 0..w {
            let i = row * w + col;
            let text_a = cover[i].min(1.0);
            let shadow_a = (shadow[i] * 1.6).min(1.0) * SHADOW_ALPHA;
            // Black gradient, then black shadow, then text, composited front to back.
            let under = shadow_a + gradient * (1.0 - shadow_a);
            let alpha = text_a + under * (1.0 - text_a);
            let px = &mut rgba[i * 4..i * 4 + 4];
            if alpha > 0.0 {
                // Straight color: only the text contributes light.
                let k = text_a / alpha;
                for c in 0..3 {
                    px[c] = (TEXT_RGB[c] * k * 255.0).round() as u8;
                }
            }
            px[3] = (alpha * 255.0).round() as u8;
        }
    }
    Some((width, layout.band_height, rgba))
}

/// Width in pixels of `text` at `px`.
fn line_width(font: &FontVec, px: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut x = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            x += scaled.kern(p, id);
        }
        x += scaled.h_advance(id);
        prev = Some(id);
    }
    x
}

/// Adds the coverage of one line of text. `x` is the left edge, or the right edge when
/// `align_right`; `baseline` is the text baseline.
#[allow(clippy::too_many_arguments)]
fn draw_line(
    cover: &mut [f32],
    w: usize,
    h: usize,
    font: &FontVec,
    px: f32,
    text: &str,
    x: f32,
    baseline: f32,
    align_right: bool,
) {
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let mut pen = if align_right {
        x - line_width(font, px, text)
    } else {
        x
    };
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            pen += scaled.kern(p, id);
        }
        prev = Some(id);
        let glyph = id.with_scale_and_position(scale, point(pen, baseline));
        pen += scaled.h_advance(id);
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, c| {
            let px = bounds.min.x as i64 + gx as i64;
            let py = bounds.min.y as i64 + gy as i64;
            if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                let i = py as usize * w + px as usize;
                cover[i] = (cover[i] + c).min(1.0);
            }
        });
    }
}

/// Separable box blur with the given radius.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let norm = 1.0 / (2 * r + 1) as f32;
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut sum: f32 = (0..=r.min(w - 1)).map(|i| row[i]).sum();
        for x in 0..w {
            tmp[y * w + x] = sum * norm;
            if x + r + 1 < w {
                sum += row[x + r + 1];
            }
            if x >= r {
                sum -= row[x - r];
            }
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        let mut sum: f32 = (0..=r.min(h - 1)).map(|i| tmp[i * w + x]).sum();
        for y in 0..h {
            out[y * w + x] = sum * norm;
            if y + r + 1 < h {
                sum += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                sum -= tmp[(y - r) * w + x];
            }
        }
    }
    out
}

const OVERLAY_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // Full-viewport triangle; the viewport is the caption strip.
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var out: VsOut;
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
"#;

/// GPU side of the caption for one display surface.
pub struct PhotoOverlay {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Bind group and strip height for the current text, `None` when nothing is drawn.
    strip: Option<(wgpu::BindGroup, u32)>,
    key: Option<(OverlayText, u32, u32)>,
}

impl PhotoOverlay {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photo_overlay"),
            source: wgpu::ShaderSource::Wgsl(OVERLAY_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photo_overlay"),
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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("photo_overlay"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photo_overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("photo_overlay"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
            strip: None,
            key: None,
        }
    }

    /// Rebuilds the strip when the text or the display size changed. Returns `true` when the
    /// picture changed (the caller must redraw).
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        text: Option<&OverlayText>,
        width: u32,
        height: u32,
    ) -> bool {
        let key = text
            .filter(|t| !t.is_empty())
            .map(|t| (t.clone(), width, height));
        if key == self.key {
            return false;
        }
        self.key = key;
        self.strip = self.key.as_ref().and_then(|(text, w, h)| {
            let (sw, sh, pixels) = rasterize_overlay(text, *w, *h)?;
            let texture = crate::upload_rgba_texture(
                device,
                queue,
                &crate::RgbaImageData {
                    width: sw,
                    height: sh,
                    pixels,
                },
                "photo_overlay",
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("photo_overlay"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            Some((bind_group, sh))
        });
        true
    }

    /// Draws the strip over the bottom of `target` (a `width` x `height` surface).
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let Some((bind_group, strip_height)) = &self.strip else {
            return;
        };
        let strip_height = (*strip_height).min(height);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("photo_overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_viewport(
            0.0,
            (height - strip_height) as f32,
            width as f32,
            strip_height as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_scales_with_the_short_side() {
        let landscape = OverlayLayout::for_display(2560, 1440);
        let portrait = OverlayLayout::for_display(1440, 2560);
        assert_eq!(landscape.time_px, portrait.time_px);
        assert!(landscape.band_height < 1440);
        assert!(landscape.time_px > landscape.album_px && landscape.album_px > landscape.exif_px);
    }

    #[test]
    fn empty_text_draws_nothing() {
        assert!(OverlayText::default().is_empty());
        assert!(rasterize_overlay(&OverlayText::default(), 1920, 1080).is_none());
    }

    #[test]
    fn blur_keeps_total_coverage_away_from_edges() {
        let (w, h) = (21, 21);
        let mut src = vec![0f32; w * h];
        src[10 * w + 10] = 1.0;
        let out = box_blur(&src, w, h, 2);
        let total: f32 = out.iter().sum();
        assert!((total - 1.0).abs() < 1e-4, "{total}");
    }

    #[test]
    fn text_is_drawn_when_a_system_font_exists() {
        if fonts().is_none() {
            return; // CI images without fonts.
        }
        let text = OverlayText {
            time: Some("1:07 PM".into()),
            date: Some("Monday, November 16".into()),
            album: Some("Day 6 - Dresden".into()),
            exif: Some("Canon EOS R7 · 12mm · f/6.3 · 1/125s · ISO 100".into()),
        };
        let (w, h, px) = rasterize_overlay(&text, 1920, 1080).unwrap();
        assert_eq!((w as usize) * (h as usize) * 4, px.len());
        // Some fully lit text pixels on both sides.
        let lit = |x0: usize, x1: usize| {
            (0..h as usize).any(|y| (x0..x1).any(|x| px[(y * w as usize + x) * 4] > 200))
        };
        assert!(lit(0, 400));
        assert!(lit(1500, 1920));
    }
}
