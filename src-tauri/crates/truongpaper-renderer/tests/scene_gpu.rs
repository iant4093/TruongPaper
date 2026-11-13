//! GPU smoke tests for the scene renderer (skipped when no adapter is available).
mod common;

use common::TempDir;
use glam::Vec2;
use truongpaper_renderer::scene_renderer::{FrameUniformInputs, HDR_FORMAT};
use truongpaper_renderer::{CameraState, SceneGraph, SceneRenderer};
use truongpaper_we_formats::WeVirtualFileSystem;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((bits >> 10) & 0x1f) as i32;
    let frac = (bits & 0x3ff) as f32;
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// Renders `scene` once into a `size`x`size` Rgba16Float target and returns RGBA f32 pixels.
fn render_hdr(t: &TempDir, scene: &str, size: u32) -> Option<(Vec<f32>, Vec<String>)> {
    render_hdr_with(t, scene, size, false)
}

/// [`render_hdr`], optionally with the bundled stock assets (effects, `util/white`) mounted.
fn render_hdr_with(
    t: &TempDir,
    scene: &str,
    size: u32,
    bundled: bool,
) -> Option<(Vec<f32>, Vec<String>)> {
    let (device, queue) = device()?;
    t.write("scene.json", scene);
    let mut vfs = WeVirtualFileSystem::mount(t.path().to_path_buf(), t.path().join("core"));
    if bundled {
        vfs = vfs.with_bundled_assets_dir(Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../resources/core-assets/assets"),
        ));
    }
    let graph = SceneGraph::load(&vfs, "scene.json", None, Vec2::splat(size as f32)).unwrap();
    let mut renderer = SceneRenderer::new(&device, &queue, HDR_FORMAT, graph.scene_size);
    renderer.load(&device, &queue, &vfs, &graph);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let row = size * 8;
    assert_eq!(row % 256, 0);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * size) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(
        &device,
        &queue,
        &mut encoder,
        &view,
        &graph,
        &CameraState::new(),
        &FrameUniformInputs::default(),
    );
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    let px = data
        .chunks_exact(2)
        .map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]])))
        .collect();
    Some((px, renderer.diagnostics()))
}

#[test]
fn scene_target_keeps_values_above_one() {
    let t = TempDir::new("gpu-hdr");
    t.write(
        "models/solid.json",
        r#"{ "solidlayer": true, "width": 64, "height": 64 }"#,
    );
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 64, "height": 64 } },
        "objects": [ { "name": "Bright", "image": "models/solid.json", "origin": "32 32 0",
                       "size": "64 64", "color": "1 1 1", "brightness": 3.0 } ] }"#;
    let Some((px, _)) = render_hdr(&t, scene, 64) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mid = ((32 * 64 + 32) * 4) as usize;
    assert!(px[mid] > 2.5, "expected HDR value, got {}", px[mid]);
}

#[test]
fn text_layer_draws_glyph_pixels_when_a_font_exists() {
    let t = TempDir::new("gpu-text");
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 64, "height": 64 },
                       "clearcolor": "0 0 0" },
        "objects": [ { "name": "Label", "text": "MMMM", "size": "64 64", "origin": "32 32 0",
                       "pointsize": 40, "color": "1 1 1" } ] }"#;
    let Some((px, diag)) = render_hdr(&t, scene, 64) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    if diag.iter().any(|d| d.contains("no usable font")) {
        return;
    }
    assert!(diag.is_empty(), "{diag:?}");
    let lit = px.chunks_exact(4).filter(|p| p[0] > 0.5).count();
    assert!(lit > 40, "text should light some pixels, got {lit}");
}

/// Regression (2887099508): a composition layer that has no visible effect must not change the
/// image behind it (it used to redraw the whole frame squeezed into its own bounds).
#[test]
fn effectless_composition_layer_leaves_the_scene_unchanged() {
    let t = TempDir::new("gpu-compose-noop");
    t.write(
        "models/solid.json",
        r#"{ "solidlayer": true, "width": 32, "height": 64 }"#,
    );
    t.write(
        "core/models/util/composelayer.json",
        r#"{ "passthrough": true, "autosize": true }"#,
    );
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 64, "height": 64 },
                       "clearcolor": "0 0 0" },
        "objects": [
          { "name": "Red", "image": "models/solid.json", "origin": "16 32 0", "size": "32 64",
            "color": "1 0 0" },
          { "name": "Blue", "image": "models/solid.json", "origin": "48 32 0", "size": "32 64",
            "color": "0 0 1" },
          { "name": "Glass", "image": "models/util/composelayer.json", "origin": "48 32 0",
            "size": "32 64", "copybackground": true } ] }"#;
    let Some((px, _)) = render_hdr(&t, scene, 64) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    for x in [34u32, 40, 60] {
        let i = ((32 * 64 + x) * 4) as usize;
        assert!(
            px[i] < 0.1 && px[i + 2] > 0.9,
            "x={x}: expected blue, got {:?}",
            &px[i..i + 4]
        );
    }
}

/// Regression (2370927443): a composition layer renders its child layers into itself, so its
/// effects apply to the children; `copybackground: false` starts the group transparent.
#[test]
fn composition_layer_renders_its_children_through_its_effects() {
    let t = TempDir::new("gpu-compose-group");
    t.write(
        "models/solid.json",
        r#"{ "solidlayer": true, "width": 8, "height": 8 }"#,
    );
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 64, "height": 64 },
                       "clearcolor": "0 0 0" },
        "objects": [
          { "id": 1, "name": "Group", "image": "models/util/composelayer.json",
            "origin": "48 32 0", "size": "32 64", "copybackground": false,
            "effects": [ { "file": "effects/tint/effect.json", "passes": [
                { "constantshadervalues": { "color": "1 0 0", "alpha": 1.0 } } ] } ] },
          { "id": 2, "name": "Child", "image": "models/solid.json", "parent": 1,
            "origin": "0 0 0", "size": "8 8", "color": "1 1 1" } ] }"#;
    let Some((px, _)) = render_hdr_with(&t, scene, 64, true) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let at = |x: u32, y: u32| {
        let i = ((y * 64 + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    let c = at(48, 32);
    assert!(
        c[0] > 0.9 && c[1] < 0.1 && c[2] < 0.1,
        "child tinted red, got {c:?}"
    );
    let bg = at(40, 8);
    assert!(
        bg.iter().all(|v| *v < 0.05),
        "group outside child is clear, got {bg:?}"
    );
}

/// Regression (2983846453 & co.): a named but missing texture draws nothing, not a white box.
#[test]
fn missing_texture_falls_back_to_transparent() {
    let t = TempDir::new("gpu-missing-tex");
    t.write(
        "models/img.json",
        r#"{ "material": "materials/img.json", "width": 64, "height": 64 }"#,
    );
    t.write(
        "materials/img.json",
        r#"{ "passes": [ { "shader": "genericimage2", "blending": "translucent",
             "textures": ["does_not_exist"] } ] }"#,
    );
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 64, "height": 64 },
                       "clearcolor": "0 0 0" },
        "objects": [ { "name": "Img", "image": "models/img.json", "origin": "32 32 0",
                       "size": "64 64" } ] }"#;
    let Some((px, _)) = render_hdr_with(&t, scene, 64, true) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mid = ((32 * 64 + 32) * 4) as usize;
    assert!(
        px[mid..mid + 3].iter().all(|v| *v < 0.05),
        "{:?}",
        &px[mid..mid + 4]
    );
}

/// Regression (3299228616): `colorBlendMode` 9 (linear dodge) adds the layer to the scene, so a
/// mostly black ripple layer no longer paints a black box over the water.
#[test]
fn color_blend_mode_linear_dodge_adds() {
    let t = TempDir::new("gpu-blend-add");
    t.write(
        "models/solid.json",
        r#"{ "solidlayer": true, "width": 64, "height": 64 }"#,
    );
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 64, "height": 64 },
                       "clearcolor": "0 0 0" },
        "objects": [
          { "name": "Bg", "image": "models/solid.json", "origin": "32 32 0", "size": "64 64",
            "color": "0 0.5 0" },
          { "name": "Add", "image": "models/solid.json", "origin": "32 32 0", "size": "64 64",
            "color": "0.25 0 0", "colorBlendMode": 9,
            "effects": [ { "file": "effects/tint/effect.json", "passes": [
                { "constantshadervalues": { "color": "0.25 0 0", "alpha": 0.0 } } ] } ] } ] }"#;
    let Some((px, _)) = render_hdr_with(&t, scene, 64, true) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let i = ((32 * 64 + 32) * 4) as usize;
    let c = &px[i..i + 3];
    assert!(
        (c[0] - 0.25).abs() < 0.05 && (c[1] - 0.5).abs() < 0.05,
        "expected 0.25 0.5 0, got {c:?}"
    );
}
