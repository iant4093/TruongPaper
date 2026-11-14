//! Spritesheet (`TEXS` frame table) animation.
//!
//! A `.tex` with the IsGif flag stores several animation frames packed into one texture, plus a
//! table giving each frame's pixel rectangle and display time. The base image shader maps the
//! layer's 0..1 UVs into the current frame through `g_Texture0Rotation` / `g_Texture0Translation`.

use glam::{Vec2, Vec4};
use truongpaper_we_formats::TexFrameInfo;

#[derive(Debug, Clone, PartialEq)]
pub struct SpriteFrame {
    /// Frame origin in texture pixels.
    pub origin: Vec2,
    /// Frame U axis in texture pixels (x, y components).
    pub u_axis: Vec2,
    /// Frame V axis in texture pixels (x, y components).
    pub v_axis: Vec2,
    /// Display time in seconds.
    pub duration: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpriteSheet {
    frames: Vec<SpriteFrame>,
    texture_size: Vec2,
    total: f32,
}

impl SpriteSheet {
    /// Builds a sheet from a parsed frame table. Returns `None` when there is nothing to animate
    /// (fewer than two frames) or the texture size is degenerate.
    pub fn new(frames: &[TexFrameInfo], texture_width: u32, texture_height: u32) -> Option<Self> {
        if frames.len() < 2 || texture_width == 0 || texture_height == 0 {
            return None;
        }
        let frames: Vec<SpriteFrame> = frames
            .iter()
            .map(|f| SpriteFrame {
                origin: Vec2::new(f.x, f.y),
                u_axis: Vec2::new(f.width, f.width_y),
                v_axis: Vec2::new(f.height_x, f.height),
                duration: if f.frametime.is_finite() {
                    f.frametime.max(0.0)
                } else {
                    0.0
                },
            })
            .collect();
        let total = frames.iter().map(|f| f.duration).sum();
        Some(Self {
            frames,
            texture_size: Vec2::new(texture_width as f32, texture_height as f32),
            total,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Index of the frame shown at `time` seconds (the animation loops).
    pub fn frame_index(&self, time: f32) -> usize {
        if self.total <= 0.0 || !time.is_finite() {
            return 0;
        }
        let mut t = time.rem_euclid(self.total);
        for (i, f) in self.frames.iter().enumerate() {
            if t < f.duration {
                return i;
            }
            t -= f.duration;
        }
        self.frames.len() - 1
    }

    /// `(g_Texture0Rotation, g_Texture0Translation)` for the frame shown at `time`:
    /// `uv' = (dot(uv, rot.xy), dot(uv, rot.zw)) + translation`.
    pub fn uniforms(&self, time: f32) -> (Vec4, Vec2) {
        self.frame_uniforms(self.frame_index(time))
    }

    /// Maps a 0..1 quad uv into the texture region of `frame` (clamped to the last frame).
    pub fn map_uv(&self, frame: usize, uv: [f32; 2]) -> [f32; 2] {
        let (rot, trans) = self.frame_uniforms(frame);
        [
            uv[0] * rot.x + uv[1] * rot.y + trans.x,
            uv[0] * rot.z + uv[1] * rot.w + trans.y,
        ]
    }

    /// Like [`Self::uniforms`] for a given frame index.
    pub fn frame_uniforms(&self, frame: usize) -> (Vec4, Vec2) {
        let f = &self.frames[frame.min(self.frames.len() - 1)];
        let t = self.texture_size;
        (
            Vec4::new(
                f.u_axis.x / t.x,
                f.v_axis.x / t.x,
                f.u_axis.y / t.y,
                f.v_axis.y / t.y,
            ),
            f.origin / t,
        )
    }
}

/// Largest packed sheet side. wgpu's default `max_texture_dimension_2d` is 8192.
pub const MAX_PACKED_DIMENSION: u32 = 8192;
/// Largest packed RGBA sheet in bytes; bigger sheets are downscaled by powers of two.
pub const MAX_PACKED_BYTES: usize = 64 * 1024 * 1024;

/// Number of atlas images a frame table refers to (`max(image_id) + 1`).
pub fn sprite_atlas_count(frames: &[TexFrameInfo]) -> usize {
    frames
        .iter()
        .map(|f| f.image_id.max(0) as usize + 1)
        .max()
        .unwrap_or(1)
}

/// Several atlas images packed into one RGBA texture.
#[derive(Debug, Clone, PartialEq)]
pub struct PackedAtlas {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Most atlas images a sprite sheet may be packed from.
pub const MAX_ATLAS_IMAGES: usize = 16;

/// Packs the atlas images of a multi-image sprite sheet into one texture, one part at a time
/// (see [`pack_sprite_atlases`]): the peak is the output plus the part being added.
pub struct AtlasPacker {
    rgba: Vec<u8>,
    cols: u32,
    sw: u32,
    sh: u32,
    out_w: u32,
    shift: u32,
    n: u32,
    w: u32,
    h: u32,
}

impl AtlasPacker {
    /// `w` x `h` is the first atlas's size, `n` the atlas count. `None` when the layout is
    /// impossible or `n` is 0 or above [`MAX_ATLAS_IMAGES`].
    pub fn new(w: u32, h: u32, n: usize) -> Option<AtlasPacker> {
        if w == 0 || h == 0 || n == 0 || n > MAX_ATLAS_IMAGES {
            return None;
        }
        let n = n as u32;
        let mut shift = 0u32;
        let (cols, rows, sw, sh) = loop {
            if shift >= 32 {
                return None;
            }
            let (sw, sh) = (w >> shift, h >> shift);
            if sw == 0 || sh == 0 {
                return None;
            }
            let best = (1..=n)
                .map(|cols| (cols, n.div_ceil(cols)))
                .filter(|(c, r)| c * sw <= MAX_PACKED_DIMENSION && r * sh <= MAX_PACKED_DIMENSION)
                .min_by_key(|(c, r)| (c * sw).max(r * sh));
            if let Some((c, r)) = best {
                let bytes = (c * sw) as usize * (r * sh) as usize * 4;
                if bytes <= MAX_PACKED_BYTES {
                    break (c, r, sw, sh);
                }
            }
            shift += 1;
        };
        let (out_w, out_h) = (cols * sw, rows * sh);
        Some(AtlasPacker {
            rgba: vec![0u8; out_w as usize * out_h as usize * 4],
            cols,
            sw,
            sh,
            out_w,
            shift,
            n,
            w,
            h,
        })
    }

    /// Box-filters part `i` (`pw` x `ph`, RGBA8) into its slot. `false` when the part does
    /// not fit the slot grid or its buffer is too short.
    pub fn add(&mut self, i: usize, src: &[u8], pw: u32, ph: u32) -> bool {
        if i as u32 >= self.n
            || pw > self.w
            || ph > self.h
            || src.len() < (pw as usize) * (ph as usize) * 4
        {
            return false;
        }
        let step = 1usize << self.shift;
        let (col, row) = (i as u32 % self.cols, i as u32 / self.cols);
        let (dw, dh) = (pw as usize / step, ph as usize / step);
        let area = (step * step) as u32;
        for y in 0..dh {
            for x in 0..dw {
                let mut acc = [0u32; 4];
                for sy in 0..step {
                    let base = ((y * step + sy) * pw as usize + x * step) * 4;
                    for sx in 0..step {
                        for (c, a) in acc.iter_mut().enumerate() {
                            *a += src[base + sx * 4 + c] as u32;
                        }
                    }
                }
                let dst = (((row * self.sh) as usize + y) * self.out_w as usize
                    + (col * self.sw) as usize
                    + x)
                    * 4;
                for (c, a) in acc.iter().enumerate() {
                    self.rgba[dst + c] = (a / area) as u8;
                }
            }
        }
        true
    }

    /// Moves the frame rectangles into their slots and returns the packed texture.
    pub fn finish(self, frames: &mut [TexFrameInfo]) -> PackedAtlas {
        let scale = 1.0 / (1usize << self.shift) as f32;
        for f in frames.iter_mut() {
            let id = f.image_id.clamp(0, self.n as i32 - 1) as u32;
            f.x = f.x * scale + (id % self.cols * self.sw) as f32;
            f.y = f.y * scale + (id / self.cols * self.sh) as f32;
            f.width *= scale;
            f.width_y *= scale;
            f.height_x *= scale;
            f.height *= scale;
        }
        PackedAtlas {
            width: self.out_w,
            height: self.rgba.len() as u32 / 4 / self.out_w.max(1),
            rgba: self.rgba,
        }
    }
}

/// Packs the atlas images of a multi-image sprite sheet (frames carry an `image_id`) into one
/// texture: a grid of `w x h` slots (the first atlas's size; a smaller atlas is padded with
/// transparency), downscaled by a power of two when the result would exceed
/// [`MAX_PACKED_DIMENSION`] or [`MAX_PACKED_BYTES`]. Frame rectangles are moved into their
/// slot and scaled to match. `None` when the atlases cannot be packed (frames unchanged).
pub fn pack_sprite_atlases(
    parts: &[(Vec<u8>, u32, u32)],
    frames: &mut [TexFrameInfo],
) -> Option<PackedAtlas> {
    let &(_, w, h) = parts.first()?;
    let mut packer = AtlasPacker::new(w, h, parts.len())?;
    for (i, (src, pw, ph)) in parts.iter().enumerate() {
        if !packer.add(i, src, *pw, *ph) {
            return None;
        }
    }
    Some(packer.finish(frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f32, y: f32, w: f32, h: f32, t: f32) -> TexFrameInfo {
        TexFrameInfo {
            image_id: 0,
            frametime: t,
            x,
            y,
            width: w,
            width_y: 0.0,
            height_x: 0.0,
            height: h,
        }
    }

    #[test]
    fn single_frame_is_not_animated() {
        assert!(SpriteSheet::new(&[frame(0.0, 0.0, 8.0, 8.0, 0.1)], 16, 16).is_none());
    }

    #[test]
    fn frame_index_advances_and_loops() {
        let s = SpriteSheet::new(
            &[
                frame(0.0, 0.0, 8.0, 8.0, 0.1),
                frame(8.0, 0.0, 8.0, 8.0, 0.2),
                frame(0.0, 8.0, 8.0, 8.0, 0.1),
            ],
            16,
            16,
        )
        .unwrap();
        assert_eq!(s.frame_index(0.0), 0);
        assert_eq!(s.frame_index(0.15), 1);
        assert_eq!(s.frame_index(0.35), 2);
        assert_eq!(s.frame_index(0.45), 0);
        assert_eq!(s.frame_count(), 3);
    }

    #[test]
    fn uniforms_map_uv_into_frame() {
        let s = SpriteSheet::new(
            &[
                frame(0.0, 0.0, 8.0, 8.0, 1.0),
                frame(8.0, 0.0, 8.0, 4.0, 1.0),
            ],
            16,
            16,
        )
        .unwrap();
        let (rot, tr) = s.uniforms(1.5);
        assert_eq!(rot, Vec4::new(0.5, 0.0, 0.0, 0.25));
        assert_eq!(tr, Vec2::new(0.5, 0.0));
    }

    #[test]
    fn zero_durations_stay_on_first_frame() {
        let s = SpriteSheet::new(
            &[
                frame(0.0, 0.0, 8.0, 8.0, 0.0),
                frame(8.0, 0.0, 8.0, 8.0, 0.0),
            ],
            16,
            16,
        )
        .unwrap();
        assert_eq!(s.frame_index(5.0), 0);
    }

    #[test]
    fn every_base_image_vertex_shader_applies_the_frame_mapping() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../resources/core-assets/assets/shaders");
        for name in [
            "genericimage.vert",
            "genericimage2.vert",
            "genericimage3.vert",
            "genericimage4.vert",
        ] {
            let src = std::fs::read_to_string(dir.join(name)).unwrap();
            assert!(src.contains("g_Texture0Rotation"), "{name}");
            assert!(src.contains("g_Texture0Translation"), "{name}");
        }
    }
    fn atlas_frame(id: i32, x: f32, y: f32) -> TexFrameInfo {
        TexFrameInfo {
            image_id: id,
            ..frame(x, y, 2.0, 2.0, 0.1)
        }
    }

    #[test]
    fn atlas_count_follows_highest_image_id() {
        let frames = [atlas_frame(0, 0.0, 0.0), atlas_frame(2, 0.0, 0.0)];
        assert_eq!(sprite_atlas_count(&frames), 3);
        assert_eq!(sprite_atlas_count(&[]), 1);
    }

    #[test]
    fn packs_atlases_into_slots_and_moves_frames() {
        let parts = vec![(vec![1; 4 * 4 * 4], 4, 4), (vec![2; 4 * 2 * 4], 4, 2)];
        let mut frames = [atlas_frame(0, 2.0, 0.0), atlas_frame(1, 0.0, 2.0)];
        let p = pack_sprite_atlases(&parts, &mut frames).unwrap();
        // Two 4x4 slots stacked (ties keep one column); the short second atlas is padded.
        assert_eq!((p.width, p.height), (4, 8));
        assert_eq!(p.rgba[4 * 4 * 4], 2);
        assert_eq!(p.rgba[7 * 4 * 4], 0);
        assert_eq!((frames[0].x, frames[0].y), (2.0, 0.0));
        assert_eq!((frames[1].x, frames[1].y), (0.0, 6.0));
    }

    #[test]
    fn oversized_sheets_are_downscaled() {
        // 16384-wide atlases exceed the side limit, so they are halved and stacked.
        let w = MAX_PACKED_DIMENSION * 2;
        let parts = vec![
            (vec![255; w as usize * 2 * 4], w, 2),
            (vec![0; w as usize * 2 * 4], w, 2),
        ];
        let mut frames = [atlas_frame(1, 64.0, 0.0)];
        let p = pack_sprite_atlases(&parts, &mut frames).unwrap();
        assert_eq!((p.width, p.height), (MAX_PACKED_DIMENSION, 2));
        assert_eq!(frames[0].width, 1.0);
        assert_eq!((frames[0].x, frames[0].y), (32.0, 1.0));
        assert_eq!(p.rgba[0], 255);
        assert_eq!(p.rgba[MAX_PACKED_DIMENSION as usize * 4], 0);
    }

    #[test]
    fn larger_later_atlas_is_rejected() {
        let parts = vec![(vec![0; 2 * 2 * 4], 2, 2), (vec![0; 4 * 4 * 4], 4, 4)];
        let mut frames = [atlas_frame(1, 1.0, 1.0)];
        assert!(pack_sprite_atlases(&parts, &mut frames).is_none());
        assert_eq!(frames[0].x, 1.0);
    }

    #[test]
    fn packer_rejects_too_many_atlases() {
        assert!(AtlasPacker::new(4, 4, MAX_ATLAS_IMAGES + 1).is_none());
        assert!(AtlasPacker::new(4, 4, 0).is_none());
        assert!(AtlasPacker::new(0, 4, 2).is_none());
    }

    #[test]
    fn packed_sheet_never_exceeds_the_byte_budget() {
        // Four 8192x8192 atlases would be 1 GiB; the layout must shrink to the budget.
        let p = AtlasPacker::new(8192, 8192, 4);
        let p = p.unwrap();
        assert!(p.rgba.len() <= MAX_PACKED_BYTES);
    }

    #[test]
    fn incremental_packing_matches_the_batch_helper() {
        let parts = vec![
            (vec![200u8; 4 * 4 * 4], 4, 4),
            (vec![10u8; 4 * 4 * 4], 4, 4),
        ];
        let mut f1 = [atlas_frame(1, 1.0, 1.0)];
        let mut f2 = [atlas_frame(1, 1.0, 1.0)];
        let batch = pack_sprite_atlases(&parts, &mut f1).unwrap();
        let mut packer = AtlasPacker::new(4, 4, 2).unwrap();
        for (i, (src, w, h)) in parts.iter().enumerate() {
            assert!(packer.add(i, src, *w, *h));
        }
        assert_eq!(packer.finish(&mut f2), batch);
        assert_eq!(f1[0].x, f2[0].x);
    }
}
