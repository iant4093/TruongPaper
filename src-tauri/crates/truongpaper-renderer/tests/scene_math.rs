//! Headless tests for scene-space MVP construction (docs/03 §7.2).

use glam::{Mat4, Vec2, Vec3, Vec4};
use truongpaper_renderer::*;

fn ndc(m: Mat4, p: Vec3) -> Vec3 {
    let c = m * p.extend(1.0);
    c.truncate() / c.w
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).abs().max_element() < 1e-4
}

#[test]
fn orthographic_projection_maps_bottom_left_origin_to_ndc() {
    let p = scene_orthographic_projection(1920.0, 1080.0);
    assert!(close(ndc(p, Vec3::ZERO), Vec3::new(-1.0, -1.0, 0.5)));
    assert!(close(
        ndc(p, Vec3::new(1920.0, 1080.0, 0.0)),
        Vec3::new(1.0, 1.0, 0.5)
    ));
    assert!(close(
        ndc(p, Vec3::new(960.0, 540.0, 0.0)),
        Vec3::new(0.0, 0.0, 0.5)
    ));
    assert_eq!(
        p,
        Mat4::orthographic_rh(0.0, 1920.0, 0.0, 1080.0, -1000.0, 1000.0)
    );
    // Invalid sizes fall back to the 1920x1080 default.
    assert_eq!(scene_orthographic_projection(0.0, -1.0), p);
}

#[test]
fn alignment_offsets_follow_spec_signs() {
    let s = Vec2::new(200.0, 100.0);
    assert_eq!(alignment_offset("center", s), Vec2::ZERO);
    assert_eq!(alignment_offset("top-left", s), Vec2::new(100.0, -50.0));
    assert_eq!(alignment_offset("bottom-left", s), Vec2::new(100.0, 50.0));
    assert_eq!(alignment_offset("top-right", s), Vec2::new(-100.0, -50.0));
    assert_eq!(alignment_offset("bottom-right", s), Vec2::new(-100.0, 50.0));
    assert_eq!(alignment_offset("unknown", s), Vec2::ZERO);
}

#[test]
fn model_matrix_applies_origin_scale_rotation() {
    let t = LayerTransform {
        origin: Vec3::new(100.0, 200.0, 0.0),
        scale: Vec3::new(2.0, 3.0, 1.0),
        angles: Vec3::new(0.0, 0.0, std::f32::consts::FRAC_PI_2),
        size: Vec2::new(10.0, 10.0),
        alignment: "center".into(),
    };
    let m = layer_model_matrix(&t);
    // Local +X (scaled by 2) rotated 90° about Z -> +Y.
    let p = m * Vec4::new(1.0, 0.0, 0.0, 1.0);
    assert!(close(p.truncate(), Vec3::new(100.0, 202.0, 0.0)));
    let origin = m * Vec4::new(0.0, 0.0, 0.0, 1.0);
    assert!(close(origin.truncate(), t.origin));
}

#[test]
fn pass_matrices_compose_and_invert() {
    let layer = LayerTransform::default();
    let pm = build_pass_matrices(Vec2::new(1920.0, 1080.0), &layer, Vec2::ZERO);
    // Default full-scene layer centered at (960, 540): its local origin lands at NDC centre.
    assert!(close(
        ndc(pm.model_view_projection, Vec3::ZERO),
        Vec3::new(0.0, 0.0, 0.5)
    ));
    // Quad corner (-w/2, -h/2) lands at NDC bottom-left.
    assert!(close(
        ndc(pm.model_view_projection, Vec3::new(-960.0, -540.0, 0.0)),
        Vec3::new(-1.0, -1.0, 0.5)
    ));
    let id = pm.model_view_projection * pm.model_view_projection_inverse;
    assert!(id.abs_diff_eq(Mat4::IDENTITY, 1e-4));
    assert!((pm.model * pm.model_inverse).abs_diff_eq(Mat4::IDENTITY, 1e-4));
    assert_eq!(pm.model_view_projection, pm.view_projection * pm.model);
}

#[test]
fn parallax_offsets_eye_toward_pointer() {
    assert_eq!(parallax_eye_offset(Vec2::splat(0.5), 1.0, 1.0), Vec2::ZERO);
    assert_eq!(
        parallax_eye_offset(Vec2::new(1.0, 0.0), 20.0, 0.5),
        Vec2::new(5.0, -5.0)
    );
    let pm = build_pass_matrices(
        Vec2::new(1920.0, 1080.0),
        &LayerTransform::default(),
        Vec2::new(96.0, 0.0),
    );
    // Eye moves +96 units -> scene shifts -96 units = -0.1 NDC.
    assert!(close(
        ndc(pm.model_view_projection, Vec3::ZERO),
        Vec3::new(-0.1, 0.0, 0.5)
    ));
}
