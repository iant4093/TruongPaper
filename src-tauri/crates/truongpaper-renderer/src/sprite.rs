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
        let f = &self.frames[self.frame_index(time)];
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
}
