mod common;

use common::TempDir;
use glam::{Mat4, Vec2, Vec3};
use truongpaper_renderer::{
    build_pass_matrices, camera_shake_offset, CameraRuntime, CameraState, HAlign, SceneGraph,
    SceneLayerKind, SceneProjection, VAlign,
};
use truongpaper_we_formats::WeVirtualFileSystem;

const MONITOR: Vec2 = Vec2::new(3840.0, 2160.0);

/// TEXV0005 / TEXI0001 / TEXB0001 RGBA8888 texture: `tw x th` padded, `iw x ih` image.
fn synthetic_tex(tw: i32, th: i32, iw: i32, ih: i32) -> Vec<u8> {
    let mut b = Vec::new();
    for magic in ["TEXV0005", "TEXI0001"] {
        b.extend_from_slice(magic.as_bytes());
        b.push(0);
    }
    for v in [0, 0, tw, th, iw, ih, 0] {
        b.extend(v.to_le_bytes());
    }
    b.extend_from_slice(b"TEXB0001\0");
    b.extend(1i32.to_le_bytes()); // image count
    b.extend(1i32.to_le_bytes()); // mipmap count
    b.extend(2i32.to_le_bytes());
    b.extend(2i32.to_le_bytes());
    b.extend(16i32.to_le_bytes());
    b.extend([255u8; 16]);
    b
}

/// Writes `models/img.json` -> `materials/img.json` -> `materials/img.tex` (300x200 image).
fn write_image_assets(t: &TempDir) {
    t.write("models/img.json", r#"{ "material": "materials/img.json" }"#);
    t.write(
        "materials/img.json",
        r#"{ "passes": [ { "shader": "genericimage2", "blending": "translucent", "textures": ["img", null, "noise"] } ] }"#,
    );
    std::fs::write(
        t.path().join("materials/img.tex"),
        synthetic_tex(512, 256, 300, 200),
    )
    .unwrap();
}

fn mount(t: &TempDir) -> WeVirtualFileSystem {
    WeVirtualFileSystem::mount(t.path().to_path_buf(), t.path().join("core"))
}

fn load(t: &TempDir, scene: &str, props: Option<&str>) -> SceneGraph {
    t.write("scene.json", scene);
    SceneGraph::load(&mount(t), "scene.json", props, MONITOR).expect("scene loads")
}

#[test]
fn orthographic_and_perspective_projection_selection() {
    let t = TempDir::new("sg-proj");
    let g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 2560, "height": 1440 } }, "objects": [] }"#,
        None,
    );
    assert_eq!(
        g.camera.projection,
        SceneProjection::Orthographic {
            width: 2560.0,
            height: 1440.0
        }
    );
    assert_eq!(g.scene_size, Vec2::new(2560.0, 1440.0));

    // Absent => perspective with documented defaults and the monitor aspect.
    let g = load(&t, r#"{ "general": {}, "objects": [] }"#, None);
    match g.camera.projection {
        SceneProjection::Perspective {
            fov_degrees,
            nearz,
            farz,
            aspect,
            eye,
            center,
            up,
        } => {
            assert_eq!((fov_degrees, nearz, farz), (50.0, 0.01, 10000.0));
            assert!((aspect - 16.0 / 9.0).abs() < 1e-6);
            assert_eq!((eye, center, up), (Vec3::Z, Vec3::ZERO, Vec3::Y));
        }
        other => panic!("expected perspective, got {other:?}"),
    }
    assert_eq!(g.scene_size, MONITOR);

    // Explicit null + custom camera fields.
    let g = load(
        &t,
        r#"{ "camera": { "eye": "0 0 5", "center": "0 0 0", "up": "0 1 0" },
             "general": { "orthogonalprojection": null, "fov": 70, "nearz": 0.5, "farz": 500 }, "objects": [] }"#,
        None,
    );
    assert!(g.camera.projection.is_perspective());
    let expected = Mat4::perspective_rh(70f32.to_radians(), 16.0 / 9.0, 0.5, 500.0)
        * Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
    assert!(g.camera.projection.matrix().abs_diff_eq(expected, 1e-5));
}

#[test]
fn auto_projection_uses_the_monitor_size() {
    let t = TempDir::new("sg-auto");
    let g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "auto": true } }, "objects": [] }"#,
        None,
    );
    assert_eq!(g.scene_size, MONITOR);
    assert_eq!(
        g.camera.projection,
        SceneProjection::Orthographic {
            width: 3840.0,
            height: 2160.0
        }
    );
}

#[test]
fn alignment_parent_chain_and_size_fallbacks() {
    let t = TempDir::new("sg-parent");
    write_image_assets(&t);
    t.write(
        "models/sized.json",
        r#"{ "material": "materials/img.json", "width": 640, "height": 480 }"#,
    );
    let g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } }, "objects": [
            { "id": 1, "name": "Group", "origin": "100 100 0" },
            { "id": 2, "name": "Child", "parent": 1, "image": "models/img.json",
              "origin": "10 20 0", "alignment": "top-left", "size": "200 100" },
            { "id": 3, "name": "Orphan", "parent": 99, "image": "models/img.json", "origin": "5 5 0" },
            { "id": 4, "name": "Sized", "image": "models/sized.json" },
            { "id": 5, "name": "CycleA", "parent": 6, "image": "models/img.json", "origin": "1 0 0" },
            { "id": 6, "name": "CycleB", "parent": 5, "image": "models/img.json", "origin": "0 1 0" }
        ] }"#,
        None,
    );
    let names: Vec<&str> = g.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Child", "Orphan", "Sized", "CycleA", "CycleB"]);

    // Group (no payload) is a transform-only parent node.
    let child = &g.layers[0];
    assert_eq!(child.parent_object, Some(0));
    let p = child.world_matrix.transform_point3(Vec3::ZERO);
    // T(100,100) * T(10,20) * T(+w/2, -h/2)
    assert!(p.abs_diff_eq(Vec3::new(210.0, 70.0, 0.0), 1e-4), "{p}");
    assert_eq!(
        g.object_world_matrix(0).unwrap(),
        Mat4::from_translation(Vec3::new(100.0, 100.0, 0.0))
    );

    // Missing parent -> root, with a warning.
    let orphan = &g.layers[1];
    assert_eq!(orphan.parent_object, None);
    assert!(g
        .warnings
        .iter()
        .any(|w| w.contains("missing parent id 99")));
    // Size falls back to the TEXI image size (300x200), not the padded 512x256.
    assert_eq!(orphan.transform.size, Vec2::new(300.0, 200.0));
    // Model width/height win over the texture size.
    assert_eq!(g.layers[2].transform.size, Vec2::new(640.0, 480.0));

    // A cycle is broken: CycleA becomes a root, CycleB keeps CycleA as its parent.
    assert!(g.warnings.iter().any(|w| w.contains("cycle")));
    assert_eq!(g.layers[3].parent_object, None);
    assert_eq!(g.layers[4].parent_object, Some(4));
    let b = g.layers[4].world_matrix.transform_point3(Vec3::ZERO);
    assert!(b.abs_diff_eq(Vec3::new(1.0, 1.0, 0.0), 1e-5));

    // Image kind payload.
    match &child.kind {
        SceneLayerKind::Image {
            model_path,
            texture_paths,
            material,
            puppet_path,
            ..
        } => {
            assert_eq!(model_path, "models/img.json");
            assert_eq!(
                texture_paths,
                &vec![
                    Some("materials/img.tex".to_string()),
                    None,
                    Some("materials/noise.tex".to_string())
                ]
            );
            assert_eq!(material.as_ref().unwrap().passes[0].blending, "translucent");
            assert!(puppet_path.is_none());
        }
        other => panic!("expected image, got {other:?}"),
    }
}

#[test]
fn user_bound_values_forms_a_b_c() {
    let t = TempDir::new("sg-bound");
    write_image_assets(&t);
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } }, "objects": [
        { "name": "A", "image": "models/img.json", "alpha": 0.5, "visible": true },
        { "name": "B", "image": "models/img.json", "alpha": { "user": "opacity", "value": 1.0 },
          "color": { "user": "tint", "value": "1 1 1" }, "origin": { "user": "pos", "value": "1 2 3" } },
        { "name": "C", "image": "models/img.json",
          "visible": { "user": { "name": "mode", "condition": "2" }, "value": true } }
    ] }"#;

    let g = load(&t, scene, None);
    assert_eq!(g.layers[0].alpha, 0.5);
    assert_eq!(g.layers[1].alpha, 1.0);
    assert!(g.layers[2].visible);

    let mut g = load(
        &t,
        scene,
        Some(
            r#"{ "opacity": 0.25, "tint": "1 0 0", "mode": 1, "pos": { "type": "text", "value": "7 8 9" } }"#,
        ),
    );
    assert_eq!(g.layers[1].alpha, 0.25);
    assert_eq!(g.layers[1].color, Vec3::new(1.0, 0.0, 0.0));
    assert_eq!(g.layers[1].transform.origin, Vec3::new(7.0, 8.0, 9.0));
    assert!(!g.layers[2].visible);

    g.apply_user_properties(r#"{ "mode": 2, "opacity": "0.75" }"#)
        .unwrap();
    assert!(g.layers[2].visible);
    assert_eq!(g.layers[1].alpha, 0.75);
    // Previously set values are kept (merge, not replace).
    assert_eq!(g.layers[1].color, Vec3::new(1.0, 0.0, 0.0));
    assert!(g.apply_user_properties("[1, 2]").is_err());
}

#[test]
fn layer_kind_detection_and_missing_assets() {
    let t = TempDir::new("sg-kinds");
    write_image_assets(&t);
    t.write(
        "core/models/util/composelayer.json",
        r#"{ "material": "" }"#,
    );
    t.write("models/pass.json", r#"{ "passthrough": true }"#);
    t.write("models/full.json", r#"{ "fullscreen": true }"#);
    t.write(
        "models/solid.json",
        r#"{ "solidlayer": true, "width": 50, "height": 60 }"#,
    );
    t.write("core/models/util/solidlayer.json", r#"{ }"#);
    t.write(
        "models/badtex.json",
        r#"{ "material": "materials/bad.json" }"#,
    );
    t.write(
        "materials/bad.json",
        r#"{ "passes": [ { "shader": "genericimage2", "textures": ["missing"] } ] }"#,
    );
    t.write(
        "particles/sparks.json",
        r#"{ "maxcount": 16, "emitter": [ { "name": "boxrandom", "rate": 10 } ], "renderer": [ { "name": "sprite" } ] }"#,
    );
    let g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } }, "objects": [
            { "name": "Compose", "image": "models/util/composelayer.json" },
            { "name": "Pass", "image": "models/pass.json" },
            { "name": "Full", "image": "models/full.json" },
            { "name": "Solid", "image": "models/solid.json", "color": "0 0.5 1" },
            { "name": "SolidUtil", "image": "models/util/solidlayer.json" },
            { "name": "Spark", "particle": "particles/sparks.json" },
            { "name": "Rain", "sound": ["sounds/rain.mp3"], "playbackmode": "random",
              "volume": { "user": "vol", "value": 0.8 }, "mintime": 1, "maxtime": 5 },
            { "name": "Label", "text": "Hello" },
            { "name": "Missing", "image": "models/nope.json" },
            { "name": "MissingParticle", "particle": "particles/nope.json" },
            { "name": "MissingTex", "image": "models/badtex.json" }
        ] }"#,
        Some(r#"{ "vol": 0.3 }"#),
    );
    let kinds: Vec<(&str, &str)> = g
        .layers
        .iter()
        .map(|l| (l.name.as_str(), l.kind.name()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Compose", "composition"),
            ("Pass", "composition"),
            ("Full", "composition"),
            ("Solid", "solid"),
            ("SolidUtil", "solid"),
            ("Spark", "particle"),
            ("Rain", "sound"),
            ("Label", "text"),
        ]
    );
    // Composition layers default to the scene size; solid uses model width/height.
    assert_eq!(g.layers[0].transform.size, Vec2::new(1920.0, 1080.0));
    assert_eq!(g.layers[3].transform.size, Vec2::new(50.0, 60.0));
    match &g.layers[3].kind {
        SceneLayerKind::Solid { color } => assert_eq!(*color, Vec3::new(0.0, 0.5, 1.0)),
        other => panic!("{other:?}"),
    }
    match &g.layers[6].kind {
        SceneLayerKind::Sound {
            paths,
            playbackmode,
            volume,
            mintime,
            maxtime,
        } => {
            assert_eq!(paths, &vec!["sounds/rain.mp3".to_string()]);
            assert_eq!(playbackmode, "random");
            assert_eq!((*volume, *mintime, *maxtime), (0.3, 1.0, 5.0));
        }
        other => panic!("{other:?}"),
    }
    assert!(g.particle_system(5).is_some());
    assert_eq!(g.particle_systems().count(), 1);

    // Missing model / particle are skipped with warnings; text warns once.
    assert!(g.warnings.iter().any(|w| w.contains("'Missing' skipped")));
    assert!(g
        .warnings
        .iter()
        .any(|w| w.contains("'MissingParticle' skipped")));
    assert!(g
        .warnings
        .iter()
        .any(|w| w.contains("'MissingTex' skipped")));
    assert!(!g.warnings.iter().any(|w| w.contains("not drawn")));
    let diag = g.diagnostics.as_slice();
    assert!(
        diag.iter().any(|d| d.contains("'Missing' skipped")),
        "{diag:?}"
    );
}

#[test]
fn text_layers_carry_style_and_accept_script_text() {
    let t = TempDir::new("sg-text");
    let mut g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } }, "objects": [
            { "name": "Clock", "size": "400 100", "text": { "script": "export function update(v) { return v; }", "value": "00:00" },
              "pointsize": 48, "font": "fonts/a.ttf", "horizontalalign": "right",
              "verticalalign": "top", "padding": 6 },
            { "name": "Plain", "text": "Hi" }
        ] }"#,
        None,
    );
    match &g.layers[0].kind {
        SceneLayerKind::Text { text, style } => {
            assert_eq!(text, "00:00");
            assert_eq!(style.pointsize, 48.0);
            assert_eq!(style.font.as_deref(), Some("fonts/a.ttf"));
            assert_eq!(style.halign, HAlign::Right);
            assert_eq!(style.valign, VAlign::Top);
            assert_eq!(style.padding, 6.0);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(g.layers[0].transform.size, Vec2::new(400.0, 100.0));
    match &g.layers[1].kind {
        SceneLayerKind::Text { style, .. } => {
            assert_eq!(style.halign, HAlign::Center);
            assert_eq!(style.valign, VAlign::Center);
        }
        other => panic!("{other:?}"),
    }
    let slots = g.property_scripts();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].slot_key, "Clock/text");
    assert_eq!(slots[0].initial_value, "00:00");

    assert!(g.apply_script_output("Clock/text", "12:34"));
    assert!(g.apply_layer_text("Plain", "Bye"));
    assert!(
        !g.apply_layer_text("Plain", "Bye"),
        "unchanged text reports false"
    );
    assert!(!g.apply_script_output("Missing/text", "x"));
    match (&g.layers[0].kind, &g.layers[1].kind) {
        (SceneLayerKind::Text { text: a, .. }, SceneLayerKind::Text { text: b, .. }) => {
            assert_eq!((a.as_str(), b.as_str()), ("12:34", "Bye"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn particles_step_with_cursor() {
    let t = TempDir::new("sg-particles");
    t.write(
        "particles/sparks.json",
        r#"{ "maxcount": 32, "emitter": [ { "name": "boxrandom", "rate": 100 } ], "renderer": [ { "name": "sprite" } ] }"#,
    );
    let mut g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } }, "objects": [
            { "name": "Spark", "particle": "particles/sparks.json", "origin": "960 540 0" } ] }"#,
        None,
    );
    assert_eq!(g.particle_system(0).unwrap().particles.len(), 0);
    g.step_particles(0.1, Vec2::new(100.0, 100.0));
    assert!(!g.particle_system(0).unwrap().particles.is_empty());
}

#[test]
fn parallax_smoothing_with_delay() {
    let rt = CameraRuntime {
        parallax_enabled: true,
        parallax_amount: 1.0,
        parallax_mouse_influence: 1.0,
        parallax_delay: 0.5,
        ..CameraRuntime::default()
    };
    let mut cam = CameraState::new();
    cam.step(0.1, Vec2::new(1.0, 0.5), &rt);
    assert_eq!(cam.parallax_target, Vec2::new(0.5, 0.0));
    let expected = 0.5 * (1.0 - (-0.2f32).exp());
    assert!((cam.parallax_offset.x - expected).abs() < 1e-6);
    // Converges toward the target.
    for _ in 0..200 {
        cam.step(0.1, Vec2::new(1.0, 0.5), &rt);
    }
    assert!((cam.parallax_offset.x - 0.5).abs() < 1e-4);

    // delay <= 0 is immediate.
    let mut cam = CameraState::new();
    cam.step(
        0.016,
        Vec2::new(0.0, 1.0),
        &CameraRuntime {
            parallax_delay: 0.0,
            ..rt.clone()
        },
    );
    assert_eq!(cam.parallax_offset, Vec2::new(-0.5, 0.5));

    // Disabled => no offset.
    let mut cam = CameraState::new();
    cam.step(0.1, Vec2::ONE, &CameraRuntime::default());
    assert_eq!(cam.parallax_offset, Vec2::ZERO);

    // Per-layer parallaxDepth scales the eye offset (fraction of the scene size).
    let mut cam = CameraState::new();
    cam.step(
        0.1,
        Vec2::new(1.0, 0.5),
        &CameraRuntime {
            parallax_delay: 0.0,
            ..rt
        },
    );
    assert_eq!(
        cam.eye_offset(Vec2::new(0.5, 1.0), Vec2::new(1000.0, 500.0)),
        Vec2::new(250.0, 0.0)
    );
}

#[test]
fn parallax_settings_and_pass_matrices_from_scene() {
    let t = TempDir::new("sg-parallax");
    write_image_assets(&t);
    let g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 },
                "cameraparallax": { "user": "parallax", "value": false }, "cameraparallaxamount": 0.4,
                "cameraparallaxdelay": 0.2, "cameraparallaxmouseinfluence": 0.3 },
             "objects": [
                { "name": "Bg", "image": "models/img.json", "origin": "960 540 0", "size": "1920 1080",
                  "parallaxDepth": "1 0.5" } ] }"#,
        Some(r#"{ "parallax": true }"#),
    );
    assert!(g.camera.parallax_enabled);
    assert_eq!(
        (
            g.camera.parallax_amount,
            g.camera.parallax_delay,
            g.camera.parallax_mouse_influence
        ),
        (0.4, 0.2, 0.3)
    );
    let layer = &g.layers[0];
    assert_eq!(layer.parallax_depth, Vec2::new(1.0, 0.5));

    // Root layer, no camera motion: identical to scene_math::build_pass_matrices.
    let cam = CameraState::new();
    let m = g.layer_pass_matrices(0, &cam);
    let expected = build_pass_matrices(g.scene_size, &layer.transform, Vec2::ZERO);
    assert!(m
        .model_view_projection
        .abs_diff_eq(expected.model_view_projection, 1e-6));

    // With parallax the eye offset is scaled by parallaxDepth and the scene size.
    let mut cam = CameraState::new();
    cam.step(
        0.1,
        Vec2::new(1.0, 1.0),
        &CameraRuntime {
            parallax_delay: 0.0,
            ..g.camera.clone()
        },
    );
    let eye = cam.eye_offset(layer.parallax_depth, g.scene_size);
    let expected = build_pass_matrices(g.scene_size, &layer.transform, eye);
    let m = g.layer_pass_matrices(0, &cam);
    assert!(m
        .model_view_projection
        .abs_diff_eq(expected.model_view_projection, 1e-6));
    assert!(eye.x > 0.0 && eye.y > 0.0 && eye.x > eye.y);
}

#[test]
fn camera_shake_is_deterministic() {
    let a = camera_shake_offset(1.25, 2.0, 0.5, 3.0);
    let b = camera_shake_offset(1.25, 2.0, 0.5, 3.0);
    assert_eq!(a, b);
    assert_ne!(a, camera_shake_offset(1.5, 2.0, 0.5, 3.0));
    // Roughness 0 => pure base frequency.
    let base = camera_shake_offset(0.7, 1.0, 0.0, 2.0);
    assert!((base.x - (1.4f32).sin()).abs() < 1e-6);
    assert!((base.y - (1.4f32 * 1.3).cos()).abs() < 1e-6);
    assert!(a.length() <= 2.0 * 2f32.sqrt() + 1e-5);

    let t = TempDir::new("sg-shake");
    let g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 },
             "camerashake": { "user": "shake", "value": false }, "camerashakeamplitude": 4,
             "camerashakeroughness": 0.25, "camerashakespeed": 2 }, "objects": [] }"#,
        Some(r#"{ "shake": true }"#),
    );
    assert!(g.camera.shake_enabled);
    let mut cam = CameraState::new();
    cam.step(0.5, Vec2::splat(0.5), &g.camera);
    assert_eq!(cam.shake_offset, camera_shake_offset(0.5, 4.0, 0.25, 2.0));
    let mut cam2 = CameraState::new();
    cam2.step(0.5, Vec2::splat(0.5), &g.camera);
    assert_eq!(cam, cam2);
}

#[test]
fn property_scripts_and_script_output() {
    let t = TempDir::new("sg-scripts");
    write_image_assets(&t);
    let mut g = load(
        &t,
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } }, "objects": [
            { "name": "Spark", "image": "models/img.json",
              "origin": { "script": "export function update(value) { return value; }", "value": "960 540 0" },
              "alpha": { "script": "export function update(v) { return v; }", "value": 0.5 } },
            { "id": 42, "image": "models/img.json",
              "visible": { "script": "export function update(v) { return v; }", "value": true } }
        ] }"#,
        None,
    );
    let slots = g.property_scripts();
    let keys: Vec<&str> = slots.iter().map(|s| s.slot_key.as_str()).collect();
    assert_eq!(keys, ["Spark/origin", "Spark/alpha", "42/visible"]);
    assert_eq!(slots[0].initial_value, "960 540 0");
    assert_eq!(slots[0].layer_name, "Spark");
    assert_eq!(slots[0].property, "origin");
    assert!(slots[0].script.contains("export function update"));
    assert_eq!(slots[1].initial_value, "0.5");
    assert_eq!(slots[2].initial_value, "true");
    assert_eq!(slots[2].layer_name, "");

    assert!(g.apply_script_output("Spark/origin", "1 2 3"));
    assert_eq!(g.layers[0].transform.origin, Vec3::new(1.0, 2.0, 3.0));
    assert!(g.layers[0]
        .world_matrix
        .transform_point3(Vec3::ZERO)
        .abs_diff_eq(Vec3::new(1.0, 2.0, 3.0), 1e-5));
    assert!(g.apply_script_output("Spark/origin", r#"{"x":5,"y":6,"z":7}"#));
    assert_eq!(g.layers[0].transform.origin, Vec3::new(5.0, 6.0, 7.0));
    assert!(g.apply_script_output("Spark/origin", "\"8 9\""));
    assert_eq!(g.layers[0].transform.origin, Vec3::new(8.0, 9.0, 7.0));
    assert!(g.apply_script_output("Spark/alpha", "0.25"));
    assert_eq!(g.layers[0].alpha, 0.25);
    assert!(g.apply_script_output("42/visible", "false"));
    assert!(!g.layers[1].visible);
    assert!(g.apply_script_output("42/visible", "1"));
    assert!(g.layers[1].visible);
    assert!(g.apply_script_output("Spark/color", "0 1 0"));
    assert_eq!(g.layers[0].color, Vec3::new(0.0, 1.0, 0.0));
    assert!(g.apply_script_output("Spark/size", "10 20"));
    assert_eq!(g.layers[0].transform.size, Vec2::new(10.0, 20.0));

    assert!(!g.apply_script_output("Nope/origin", "1 2 3"));
    assert!(!g.apply_script_output("Spark/unknown", "1"));
    assert!(!g.apply_script_output("Spark/alpha", "abc"));
    assert!(!g.apply_script_output("Spark/origin", "not a vector"));
    assert!(!g.apply_script_output("no-slash", "1"));

    // thisLayer write-back.
    assert!(g.apply_layer_state(
        "Spark",
        Vec3::new(100.0, 50.0, 0.0),
        Vec3::splat(2.0),
        Vec3::ZERO,
        Vec2::new(30.0, 40.0),
        Vec3::ONE,
        0.9,
        true,
    ));
    let l = &g.layers[0];
    assert_eq!(
        (l.transform.origin, l.transform.scale, l.alpha),
        (Vec3::new(100.0, 50.0, 0.0), Vec3::splat(2.0), 0.9)
    );
    assert!(!g.apply_layer_state(
        "Ghost",
        Vec3::ZERO,
        Vec3::ONE,
        Vec3::ZERO,
        Vec2::ONE,
        Vec3::ONE,
        1.0,
        true
    ));
}

#[test]
fn load_errors() {
    let t = TempDir::new("sg-errors");
    let vfs = mount(&t);
    assert!(SceneGraph::load(&vfs, "scene.json", None, MONITOR).is_err());
    t.write("scene.json", "{ not json");
    assert!(SceneGraph::load(&vfs, "scene.json", None, MONITOR).is_err());
    t.write("scene.json", r#"{ "general": {} }"#);
    assert!(SceneGraph::load(&vfs, "scene.json", Some("[1]"), MONITOR).is_err());
    assert!(SceneGraph::load(&vfs, "scene.json", Some(""), MONITOR).is_ok());
    assert!(SceneGraph::load(&vfs, "../escape.json", None, MONITOR).is_err());
}
