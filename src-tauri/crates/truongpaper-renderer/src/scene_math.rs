//! Scene coordinate system and `g_ModelViewProjectionMatrix` construction
//! (`docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` §7.2, items 1 and 2). Pure `glam` math.
//!
//! The doc gives prose, not code; these functions transcribe it directly:
//! - Projection: `Mat4::orthographic_rh(0, w, 0, h, -1000, 1000)`; `(0,0)` is bottom-left.
//! - Model: `"origin"`, `"scale"`, `"angles"` (radians), `"size"`, plus the `"alignment"` anchor
//!   offset (`center = (0,0)`, `top-left = (+w/2, -h/2)`, `bottom-left = (+w/2, +h/2)`; the
//!   remaining anchors follow the same sign pattern).
//! - Camera parallax: eye offset `(g_PointerPosition - 0.5) * amount * influence`.

use glam::{EulerRot, Mat4, Quat, Vec2, Vec3};

/// Default `scene.general.orthogonalprojection` when absent.
pub const DEFAULT_SCENE_SIZE: Vec2 = Vec2::new(1920.0, 1080.0);
pub const SCENE_Z_NEAR: f32 = -1000.0;
pub const SCENE_Z_FAR: f32 = 1000.0;

/// Orthographic projection for a WE scene of `width x height` units.
pub fn scene_orthographic_projection(width: f32, height: f32) -> Mat4 {
    let (w, h) = if width > 0.0 && height > 0.0 {
        (width, height)
    } else {
        (DEFAULT_SCENE_SIZE.x, DEFAULT_SCENE_SIZE.y)
    };
    Mat4::orthographic_rh(0.0, w, 0.0, h, SCENE_Z_NEAR, SCENE_Z_FAR)
}

/// Anchor offset (local units) applied for a layer `"alignment"` value. Unknown values -> center.
pub fn alignment_offset(alignment: &str, size: Vec2) -> Vec2 {
    let hw = size.x * 0.5;
    let hh = size.y * 0.5;
    match alignment.trim().to_ascii_lowercase().as_str() {
        "top-left" | "topleft" => Vec2::new(hw, -hh),
        "top" => Vec2::new(0.0, -hh),
        "top-right" | "topright" => Vec2::new(-hw, -hh),
        "left" => Vec2::new(hw, 0.0),
        "right" => Vec2::new(-hw, 0.0),
        "bottom-left" | "bottomleft" => Vec2::new(hw, hh),
        "bottom" => Vec2::new(0.0, hh),
        "bottom-right" | "bottomright" => Vec2::new(-hw, hh),
        _ => Vec2::ZERO,
    }
}

/// Layer transform inputs parsed from `scene.json` (`"origin"`, `"scale"`, `"angles"`, `"size"`).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerTransform {
    pub origin: Vec3,
    pub scale: Vec3,
    /// Euler angles in radians (`"angles": "RX RY RZ"`).
    pub angles: Vec3,
    pub size: Vec2,
    pub alignment: String,
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            origin: Vec3::new(DEFAULT_SCENE_SIZE.x * 0.5, DEFAULT_SCENE_SIZE.y * 0.5, 0.0),
            scale: Vec3::ONE,
            angles: Vec3::ZERO,
            size: DEFAULT_SCENE_SIZE,
            alignment: "center".into(),
        }
    }
}

/// Model matrix mapping a layer quad in local units (`±size/2`) into scene space:
/// `T(origin) * R(angles) * S(scale) * T(alignment_offset)`.
pub fn layer_model_matrix(t: &LayerTransform) -> Mat4 {
    let anchor = alignment_offset(&t.alignment, t.size);
    let rot = Quat::from_euler(EulerRot::ZYX, t.angles.z, t.angles.y, t.angles.x);
    Mat4::from_scale_rotation_translation(t.scale, rot, t.origin)
        * Mat4::from_translation(anchor.extend(0.0))
}

/// Camera parallax eye offset: `(pointer - 0.5) * amount * mouse_influence` (pointer in `[0,1]`).
pub fn parallax_eye_offset(pointer: Vec2, amount: f32, mouse_influence: f32) -> Vec2 {
    (pointer - Vec2::splat(0.5)) * amount * mouse_influence
}

/// View matrix for an eye shifted by `eye_offset` in the XY plane.
pub fn scene_view_matrix(eye_offset: Vec2) -> Mat4 {
    Mat4::from_translation(Vec3::new(-eye_offset.x, -eye_offset.y, 0.0))
}

/// Per-pass matrices bound as built-in uniforms (§4.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PassMatrices {
    /// `g_ModelViewProjectionMatrix` (also `g_EffectModelViewProjectionMatrix`).
    pub model_view_projection: Mat4,
    /// `g_ModelViewProjectionMatrixInverse`.
    pub model_view_projection_inverse: Mat4,
    /// `g_ModelMatrix`.
    pub model: Mat4,
    /// `g_ModelMatrixInverse`.
    pub model_inverse: Mat4,
    /// `g_ViewProjectionMatrix`.
    pub view_projection: Mat4,
}

/// Builds every matrix uniform for a layer pass.
pub fn build_pass_matrices(
    scene_size: Vec2,
    layer: &LayerTransform,
    eye_offset: Vec2,
) -> PassMatrices {
    let projection = scene_orthographic_projection(scene_size.x, scene_size.y);
    let view_projection = projection * scene_view_matrix(eye_offset);
    let model = layer_model_matrix(layer);
    let mvp = view_projection * model;
    PassMatrices {
        model_view_projection: mvp,
        model_view_projection_inverse: mvp.inverse(),
        model,
        model_inverse: model.inverse(),
        view_projection,
    }
}
