//! Headless tests for the compositor WGSL shaders, uniform layouts and CPU fit/transition math
//! (docs/01 §3). No `wgpu` adapter/device is created.

use std::mem::{align_of, offset_of, size_of};
use truongpaper_renderer::*;

fn parse_and_validate(src: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module =
        naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("validation failed: {:?}", e.into_inner()));
    (module, info)
}

fn entry_points(m: &naga::Module) -> Vec<(String, naga::ShaderStage)> {
    m.entry_points
        .iter()
        .map(|e| (e.name.clone(), e.stage))
        .collect()
}

/// Returns (span, [(member name, offset)]) of the named WGSL struct.
fn struct_layout(m: &naga::Module, name: &str) -> (u32, Vec<(String, u32)>) {
    let (_, ty) = m
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("struct {name} not found"));
    match &ty.inner {
        naga::TypeInner::Struct { members, span } => (
            *span,
            members
                .iter()
                .map(|mb| (mb.name.clone().unwrap(), mb.offset))
                .collect(),
        ),
        other => panic!("{name} is not a struct: {other:?}"),
    }
}

fn bindings(m: &naga::Module) -> Vec<(String, u32, u32)> {
    let mut v: Vec<_> = m
        .global_variables
        .iter()
        .filter_map(|(_, g)| {
            g.binding
                .as_ref()
                .map(|b| (g.name.clone().unwrap(), b.group, b.binding))
        })
        .collect();
    v.sort_by_key(|(_, g, b)| (*g, *b));
    v
}

#[test]
fn kawase_blur_wgsl_parses_and_validates() {
    let (m, _) = parse_and_validate(KAWASE_BLUR_WGSL);
    let eps = entry_points(&m);
    assert!(eps.contains(&("vs_main".into(), naga::ShaderStage::Vertex)));
    assert!(eps.contains(&("fs_downsample".into(), naga::ShaderStage::Fragment)));
    assert!(eps.contains(&("fs_upsample".into(), naga::ShaderStage::Fragment)));
    assert_eq!(
        bindings(&m),
        vec![
            ("t_input".into(), 0, 0),
            ("s_linear".into(), 0, 1),
            ("u_blur".into(), 0, 2),
        ]
    );
}

#[test]
fn compositor_wgsl_parses_and_validates() {
    let (m, _) = parse_and_validate(COMPOSITOR_WGSL);
    let eps = entry_points(&m);
    assert!(eps.contains(&("vs_main".into(), naga::ShaderStage::Vertex)));
    assert!(eps.contains(&("fs_main".into(), naga::ShaderStage::Fragment)));
    assert_eq!(
        bindings(&m),
        vec![
            ("t_from".into(), 0, 0),
            ("t_to".into(), 0, 1),
            ("t_from_blurred".into(), 0, 2),
            ("t_to_blurred".into(), 0, 3),
            ("s_linear".into(), 0, 4),
            ("u".into(), 0, 5),
        ]
    );
}

#[test]
fn shaders_match_doc_listing_exactly() {
    let doc_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/01-SYSTEM-ARCHITECTURE.md"
    );
    let Ok(doc) = std::fs::read_to_string(doc_path) else {
        eprintln!("docs/01 not found; skipping doc/shader parity check");
        return;
    };
    let doc = doc.replace("\r\n", "\n");
    let blocks: Vec<&str> = doc
        .split("```wgsl\n")
        .skip(1)
        .map(|rest| rest.split("```").next().unwrap())
        .collect();
    assert!(blocks.len() >= 2);
    assert_eq!(
        blocks[0],
        KAWASE_BLUR_WGSL.replace("\r\n", "\n"),
        "kawase_blur.wgsl drifted from docs/01 §3.1"
    );
    assert_eq!(
        blocks[1],
        COMPOSITOR_WGSL.replace("\r\n", "\n"),
        "compositor.wgsl drifted from docs/01 §3.2"
    );
}

#[test]
fn blur_uniforms_layout_matches_wgsl() {
    assert_eq!(size_of::<BlurUniforms>(), 16);
    assert_eq!(size_of::<BlurUniforms>() % 16, 0);
    assert_eq!(align_of::<BlurUniforms>(), 4);
    let (m, _) = parse_and_validate(KAWASE_BLUR_WGSL);
    let (span, members) = struct_layout(&m, "BlurUniforms");
    assert_eq!(span as usize, size_of::<BlurUniforms>());
    assert_eq!(
        members,
        vec![
            (
                "texel_size".into(),
                offset_of!(BlurUniforms, texel_size) as u32
            ),
            ("offset".into(), offset_of!(BlurUniforms, offset) as u32),
            ("_pad".into(), offset_of!(BlurUniforms, _pad) as u32),
        ]
    );
}

#[test]
fn compositor_uniforms_layout_matches_wgsl() {
    assert_eq!(size_of::<CompositorUniforms>(), 80);
    assert_eq!(size_of::<CompositorUniforms>() % 16, 0);
    let (m, _) = parse_and_validate(COMPOSITOR_WGSL);
    let (span, members) = struct_layout(&m, "CompositorUniforms");
    assert_eq!(span as usize, size_of::<CompositorUniforms>());
    macro_rules! off {
        ($f:ident) => {
            (
                stringify!($f).to_string(),
                offset_of!(CompositorUniforms, $f) as u32,
            )
        };
    }
    assert_eq!(
        members,
        vec![
            off!(progress),
            off!(transition_mode),
            off!(from_fit_mode),
            off!(to_fit_mode),
            off!(monitor_aspect),
            off!(from_image_aspect),
            off!(to_image_aspect),
            off!(zoom),
            off!(offset_x),
            off!(offset_y),
            off!(brightness),
            off!(contrast),
            off!(saturation),
            off!(hue_shift),
            off!(time),
            off!(_pad),
            off!(span_offset_x),
            off!(span_offset_y),
            off!(span_scale_x),
            off!(span_scale_y),
        ]
    );
}

#[test]
fn compositor_uniform_defaults_are_neutral() {
    let u = CompositorUniforms::default();
    assert_eq!(u.progress, 1.0);
    assert_eq!(u.zoom, 1.0);
    assert_eq!(
        (u.brightness, u.contrast, u.saturation, u.hue_shift),
        (0.0, 1.0, 1.0, 0.0)
    );
    assert_eq!(bytemuck::bytes_of(&u).len(), 80);
    assert_eq!(
        (
            u.span_offset_x,
            u.span_offset_y,
            u.span_scale_x,
            u.span_scale_y
        ),
        (0.0, 0.0, 1.0, 1.0)
    );
}

#[test]
fn fit_and_transition_enum_values_match_shader_constants() {
    assert_eq!(FitMode::Fill as u32, 0);
    assert_eq!(FitMode::FitBlur as u32, 1);
    assert_eq!(FitMode::FitBlack as u32, 2);
    assert_eq!(FitMode::Stretch as u32, 3);
    assert_eq!(TransitionMode::Crossfade as u32, 0);
    assert_eq!(TransitionMode::BlurDissolve as u32, 1);
    assert_eq!(TransitionMode::LiquidGlass as u32, 2);
    assert_eq!(
        FitMode::from_setting_str("fit_blur"),
        Some(FitMode::FitBlur)
    );
    assert_eq!(
        FitMode::from_setting_str("fit_black"),
        Some(FitMode::FitBlack)
    );
    assert_eq!(FitMode::from_setting_str("bogus"), None);
    assert_eq!(
        TransitionMode::from_setting_str("liquid_glass"),
        Some(TransitionMode::LiquidGlass)
    );
    assert_eq!(
        TransitionMode::from_setting_str("blur_dissolve"),
        Some(TransitionMode::BlurDissolve)
    );
}

#[test]
fn transition_duration_is_clamped_to_spec_range() {
    assert_eq!(clamp_transition_duration(0.0), 0.3);
    assert_eq!(clamp_transition_duration(0.1), 0.3);
    assert_eq!(clamp_transition_duration(-5.0), 0.3);
    assert_eq!(clamp_transition_duration(1.2), 1.2);
    assert_eq!(clamp_transition_duration(3.0), 3.0);
    assert_eq!(clamp_transition_duration(10.0), 3.0);
    assert_eq!(
        clamp_transition_duration(f32::NAN),
        DEFAULT_TRANSITION_DURATION_SECS
    );
    assert_eq!(
        clamp_transition_duration(f32::INFINITY),
        DEFAULT_TRANSITION_DURATION_SECS
    );

    assert_eq!(transition_progress(0.0, 1.0), 0.0);
    assert_eq!(transition_progress(0.5, 1.0), 0.5);
    assert_eq!(transition_progress(2.0, 1.0), 1.0);
    // Duration below the minimum is clamped before dividing.
    assert!((transition_progress(0.15, 0.01) - 0.5).abs() < 1e-6);
}

fn approx(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
}

fn sharp(s: FittedSample) -> [f32; 2] {
    match s {
        FittedSample::Sharp(uv) => uv,
        other => panic!("expected Sharp, got {other:?}"),
    }
}

#[test]
fn stretch_maps_screen_uv_directly() {
    let u = CompositorUniforms::default();
    for uv in [[0.0, 0.0], [0.25, 0.75], [1.0, 1.0]] {
        assert!(approx(
            sharp(fitted_layer_sample(uv, 0.5, FitMode::Stretch, &u)),
            uv
        ));
    }
}

#[test]
fn fill_crops_the_longer_image_axis() {
    // 16:9 monitor, 4:3 image -> monitor is wider -> crop top/bottom (v range shrinks).
    let u = CompositorUniforms {
        monitor_aspect: 16.0 / 9.0,
        ..Default::default()
    };
    let ratio = (16.0 / 9.0) / (4.0 / 3.0);
    let top = sharp(fitted_layer_sample(
        [0.0, 0.0],
        4.0 / 3.0,
        FitMode::Fill,
        &u,
    ));
    assert!(approx(top, [0.0, 0.5 - 0.5 / ratio]));
    let centre = sharp(fitted_layer_sample(
        [0.5, 0.5],
        4.0 / 3.0,
        FitMode::Fill,
        &u,
    ));
    assert!(approx(centre, [0.5, 0.5]));

    // Portrait image (9:16) on a 16:9 monitor -> heavy vertical crop; full u range kept.
    let left = sharp(fitted_layer_sample(
        [0.0, 0.5],
        9.0 / 16.0,
        FitMode::Fill,
        &u,
    ));
    assert!(approx(left, [0.0, 0.5]));

    // Taller monitor than image -> crop left/right.
    let u_tall = CompositorUniforms {
        monitor_aspect: 9.0 / 16.0,
        ..Default::default()
    };
    let r = (9.0 / 16.0) / (16.0 / 9.0);
    let corner = sharp(fitted_layer_sample(
        [0.0, 0.0],
        16.0 / 9.0,
        FitMode::Fill,
        &u_tall,
    ));
    assert!(approx(corner, [0.5 - 0.5 * r, 0.0]));
}

#[test]
fn fit_black_pillarboxes_portrait_image_on_landscape_monitor() {
    let u = CompositorUniforms {
        monitor_aspect: 16.0 / 9.0,
        ..Default::default()
    };
    let img = 9.0 / 16.0;
    assert_eq!(
        fitted_layer_sample([0.02, 0.5], img, FitMode::FitBlack, &u),
        FittedSample::Black
    );
    assert_eq!(
        fitted_layer_sample([0.98, 0.5], img, FitMode::FitBlack, &u),
        FittedSample::Black
    );
    assert!(approx(
        sharp(fitted_layer_sample([0.5, 0.0], img, FitMode::FitBlack, &u)),
        [0.5, 0.0]
    ));
    // Bar edge: image spans 1/ratio of the screen width.
    let ratio = (16.0 / 9.0) / img;
    let edge = 0.5 - 0.5 / ratio;
    assert!(matches!(
        fitted_layer_sample([edge + 1e-3, 0.5], img, FitMode::FitBlack, &u),
        FittedSample::Sharp(_)
    ));
    assert_eq!(
        fitted_layer_sample([edge - 1e-3, 0.5], img, FitMode::FitBlack, &u),
        FittedSample::Black
    );
}

#[test]
fn fit_blur_samples_cover_scaled_backdrop_in_bars() {
    let u = CompositorUniforms {
        monitor_aspect: 16.0 / 9.0,
        ..Default::default()
    };
    let img = 9.0 / 16.0;
    let ratio = (16.0 / 9.0) / img;
    match fitted_layer_sample([0.0, 0.0], img, FitMode::FitBlur, &u) {
        FittedSample::Blurred(uv) => assert!(approx(uv, [0.0, 0.5 - 0.5 / ratio])),
        other => panic!("expected Blurred, got {other:?}"),
    }
    // Letterbox case: wide image on a portrait monitor -> bars top/bottom.
    let u_tall = CompositorUniforms {
        monitor_aspect: 9.0 / 16.0,
        ..Default::default()
    };
    assert!(matches!(
        fitted_layer_sample([0.5, 0.02], 16.0 / 9.0, FitMode::FitBlur, &u_tall),
        FittedSample::Blurred(_)
    ));
    assert!(matches!(
        fitted_layer_sample([0.5, 0.5], 16.0 / 9.0, FitMode::FitBlur, &u_tall),
        FittedSample::Sharp(_)
    ));
}

#[test]
fn zoom_and_offset_shift_sampling() {
    let u = CompositorUniforms {
        zoom: 2.0,
        offset_x: 0.5,
        ..Default::default()
    };
    // Same aspect -> Fill scale is identity; zoom 2 halves the sampled extent, offset shifts it.
    let uv = sharp(fitted_layer_sample(
        [1.0, 1.0],
        16.0 / 9.0,
        FitMode::Fill,
        &u,
    ));
    assert!(approx(uv, [0.5 + 0.25 - 0.25, 0.75]));
    // Zoom is floored at 0.05 (no division by zero).
    let z0 = CompositorUniforms {
        zoom: 0.0,
        ..Default::default()
    };
    let uv = sharp(fitted_layer_sample(
        [0.5, 0.5],
        16.0 / 9.0,
        FitMode::Stretch,
        &z0,
    ));
    assert!(uv.iter().all(|c| c.is_finite()));
}

/// Unique temp dir for the photo decode tests (removed on drop).
struct PhotoDir(std::path::PathBuf);

impl PhotoDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "truongpaper-compositor-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for PhotoDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn load_rgba_image_decodes_a_tiny_png() {
    let dir = PhotoDir::new("png");
    let path = dir.0.join("photo.png");
    let mut img = image::RgbaImage::new(3, 2);
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    img.put_pixel(2, 1, image::Rgba([0, 0, 255, 128]));
    img.save(&path).unwrap();

    let decoded = load_rgba_image(&path, MAX_PHOTO_TEXTURE_DIMENSION).unwrap();
    assert_eq!((decoded.width, decoded.height), (3, 2));
    assert_eq!(decoded.pixels.len(), 3 * 2 * 4);
    assert_eq!(&decoded.pixels[0..4], &[255, 0, 0, 255]);
    assert_eq!(
        &decoded.pixels[(3 + 2) * 4..(3 + 2) * 4 + 4],
        &[0, 0, 255, 128]
    );
    assert!((decoded.aspect() - 1.5).abs() < 1e-6);

    // The format is sniffed from the content, not the extension.
    let renamed = dir.0.join("photo.jpg");
    std::fs::copy(&path, &renamed).unwrap();
    assert_eq!(load_rgba_image(&renamed, 64).unwrap().width, 3);
}

#[test]
fn load_rgba_image_downscales_and_rejects_bad_files() {
    let dir = PhotoDir::new("big");
    let path = dir.0.join("wide.png");
    image::RgbaImage::from_pixel(40, 10, image::Rgba([10, 20, 30, 255]))
        .save(&path)
        .unwrap();
    let decoded = load_rgba_image(&path, 20).unwrap();
    assert_eq!((decoded.width, decoded.height), (20, 5));
    assert_eq!(&decoded.pixels[0..4], &[10, 20, 30, 255]);

    let garbage = dir.0.join("broken.png");
    std::fs::write(&garbage, b"not an image").unwrap();
    assert!(load_rgba_image(&garbage, 64).is_err());
    assert!(load_rgba_image(&dir.0.join("missing.png"), 64).is_err());
}
