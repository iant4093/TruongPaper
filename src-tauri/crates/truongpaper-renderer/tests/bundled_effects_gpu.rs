//! Offscreen GPU checks that the bundled clean-room stock effects change pixels the way their
//! names promise (skipped when no adapter is available).
mod common;

use std::path::PathBuf;

use common::TempDir;
use glam::Vec2;
use truongpaper_renderer::scene_renderer::{FrameUniformInputs, HDR_FORMAT};
use truongpaper_renderer::{CameraState, SceneGraph, SceneRenderer};
use truongpaper_we_formats::WeVirtualFileSystem;

const SIZE: u32 = 64;

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

/// Uncompressed RGBA8888 `.tex` (TEXV0005 / TEXB0003, one mip).
fn tex_file(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    let cstr = |b: &mut Vec<u8>, s: &str| {
        b.extend_from_slice(s.as_bytes());
        b.push(0);
    };
    let i32le = |b: &mut Vec<u8>, v: i32| b.extend_from_slice(&v.to_le_bytes());
    cstr(&mut b, "TEXV0005");
    cstr(&mut b, "TEXI0001");
    for v in [
        0,
        0,
        width as i32,
        height as i32,
        width as i32,
        height as i32,
        0,
    ] {
        i32le(&mut b, v);
    }
    cstr(&mut b, "TEXB0003");
    for v in [
        1,
        -1,
        1,
        width as i32,
        height as i32,
        0,
        0,
        rgba.len() as i32,
    ] {
        i32le(&mut b, v);
    }
    b.extend_from_slice(rgba);
    b
}

/// Image layer filling the scene with `pixel(x, y)`, plus `effects` (scene.json effect list).
fn render(
    t: &TempDir,
    pixel: impl Fn(u32, u32) -> [u8; 4],
    effects: &str,
) -> Option<(Vec<f32>, Vec<String>)> {
    let (device, queue) = device()?;
    let rgba: Vec<u8> = (0..SIZE * SIZE)
        .flat_map(|i| pixel(i % SIZE, i / SIZE))
        .collect();
    let tex = tex_file(SIZE, SIZE, &rgba);
    std::fs::create_dir_all(t.path().join("materials")).unwrap();
    std::fs::write(t.path().join("materials/img.tex"), &tex).unwrap();
    t.write(
        "models/img.json",
        r#"{ "material": "materials/img.json", "width": 64, "height": 64 }"#,
    );
    t.write(
        "materials/img.json",
        r#"{ "passes": [ { "shader": "genericimage2", "blending": "translucent", "textures": ["img"] } ] }"#,
    );
    t.write(
        "scene.json",
        &format!(
            r#"{{ "general": {{ "orthogonalprojection": {{ "width": 64, "height": 64 }}, "clearcolor": "0 0 0" }},
                "objects": [ {{ "name": "Img", "image": "models/img.json", "origin": "32 32 0",
                               "size": "64 64", "effects": {effects} }} ] }}"#
        ),
    );
    let bundled =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/core-assets/assets");
    let vfs = WeVirtualFileSystem::mount(t.path().to_path_buf(), t.path().join("no-core"))
        .with_bundled_assets_dir(Some(bundled));
    let graph = SceneGraph::load(&vfs, "scene.json", None, Vec2::splat(SIZE as f32)).unwrap();
    let mut renderer = SceneRenderer::new(&device, &queue, HDR_FORMAT, graph.scene_size);
    let warnings = renderer.load(&device, &queue, &vfs, &graph);
    assert!(warnings.is_empty(), "{warnings:?}");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
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
    let row = SIZE * 8;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * SIZE) as u64,
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
        target.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(SIZE),
            },
        },
        target.size(),
    );
    queue.submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::Maintain::Wait);
    let px = slice
        .get_mapped_range()
        .chunks_exact(2)
        .map(|c| f16_to_f32(u16::from_le_bytes([c[0], c[1]])))
        .collect();
    Some((px, renderer.diagnostics()))
}

fn at(px: &[f32], x: u32, y: u32) -> [f32; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

fn assert_no_skips(diag: &[String]) {
    assert!(
        !diag.iter().any(|d| d.contains("skipped")),
        "effect passes were skipped: {diag:?}"
    );
}

#[test]
fn bundled_blur_softens_a_hard_edge() {
    let edge = |x: u32, _y: u32| {
        if x < 32 {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    };
    let t = TempDir::new("gpu-bundled-blur");
    let Some((sharp, _)) = render(&t, edge, "[]") else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    for effect in ["blurprecise", "blur"] {
        let t = TempDir::new(&format!("gpu-bundled-{effect}"));
        let effects = format!(
            r#"[ {{ "file": "effects/{effect}/effect.json",
                    "passes": [ {{ "constantshadervalues": {{ "scale": "2 2" }} }},
                                {{ "constantshadervalues": {{ "scale": "2 2" }} }} ] }} ]"#
        );
        let (blurred, diag) = render(&t, edge, &effects).unwrap();
        assert_no_skips(&diag);
        assert!(
            at(&sharp, 30, 32)[0] < 0.02,
            "edge is sharp without the effect"
        );
        assert!(
            at(&blurred, 30, 32)[0] > 0.1,
            "{effect}: dark side next to the edge should pick up light, got {:?}",
            at(&blurred, 30, 32)
        );
        assert!(
            at(&blurred, 33, 32)[0] < 0.9,
            "{effect}: bright side should darken"
        );
        assert!(
            at(&blurred, 4, 32)[0] < 0.02,
            "{effect}: far pixels stay dark"
        );
    }
}

#[test]
fn bundled_godrays_cast_light_away_from_the_source() {
    // A bright spot near the top-right corner on black.
    let spot = |x: u32, y: u32| {
        let (dx, dy) = (x as f32 - 54.0, y as f32 - 10.0);
        if dx * dx + dy * dy < 25.0 {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 255]
        }
    };
    let t = TempDir::new("gpu-bundled-godrays-off");
    let Some((plain, _)) = render(&t, spot, "[]") else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // Directional rays whose source lies up and to the right (direction 5π/4).
    let effects = r#"[ { "file": "effects/godrays/effect.json", "passes": [
        { "constantshadervalues": { "raythreshold": 0.3, "noiseamount": 0.0 } },
        { "combos": { "CASTER": 1 },
          "constantshadervalues": { "direction": 3.92699, "raylength": 1.0, "rayintensity": 2.0 } },
        {}, {}, {} ] } ]"#;
    let t = TempDir::new("gpu-bundled-godrays-on");
    let (rays, diag) = render(&t, spot, effects).unwrap();
    assert_no_skips(&diag);
    let gain = |x, y| at(&rays, x, y)[0] - at(&plain, x, y)[0];
    let downstream = gain(42, 22); // down-left of the spot, along the rays
    let aside = gain(54, 40); // straight below: its path to the light misses the spot
    eprintln!("godrays gain along {downstream}, aside {aside}");
    assert!(
        downstream > 0.05,
        "pixels along the rays brighten, got {downstream}"
    );
    assert!(
        downstream > aside * 2.0 + 0.02,
        "rays follow the light direction (along {downstream}, aside {aside})"
    );
}

#[test]
fn bundled_tint_recolours_the_image() {
    let grey = |_x: u32, _y: u32| [128, 128, 128, 255];
    let t = TempDir::new("gpu-bundled-tint");
    let effects = r#"[ { "file": "effects/tint/effect.json", "passes": [
        { "constantshadervalues": { "color": "1 0 0", "alpha": 1.0 } } ] } ]"#;
    let Some((px, diag)) = render(&t, grey, effects) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    assert_no_skips(&diag);
    let c = at(&px, 32, 32);
    assert!(
        c[0] > 0.9 && c[1] < 0.1 && c[2] < 0.1,
        "expected red, got {c:?}"
    );
}

#[test]
fn bundled_xray_reveals_a_spot_at_the_pointer_using_size_multiply_and_mask() {
    let grey = |_x: u32, _y: u32| [64, 64, 64, 255];
    let t = TempDir::new("gpu-bundled-xray-off");
    let Some((plain, _)) = render(&t, grey, "[]") else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let run = |name: &str, values: &str| {
        let t = TempDir::new(name);
        let effects = format!(
            r#"[ {{ "file": "effects/xray/effect.json",
                    "passes": [ {{ "constantshadervalues": {values} }} ] }} ]"#
        );
        let (px, diag) = render(&t, grey, &effects).unwrap();
        assert_no_skips(&diag);
        px
    };
    // Pointer rests at the centre: the white blend texture shows there, not in the corner.
    let spot = run("gpu-bundled-xray-on", r#"{ "size": 0.5, "multiply": 1 }"#);
    assert!(
        at(&spot, 32, 32)[0] > at(&plain, 32, 32)[0] + 0.2,
        "centre should be revealed, got {:?}",
        at(&spot, 32, 32)
    );
    assert!(
        (at(&spot, 2, 2)[0] - at(&plain, 2, 2)[0]).abs() < 0.01,
        "corner stays untouched"
    );
    // A smaller size shrinks the revealed spot.
    let small = run("gpu-bundled-xray-small", r#"{ "size": 0.15, "multiply": 1 }"#);
    assert!(
        at(&small, 44, 32)[0] < at(&spot, 44, 32)[0] - 0.05,
        "smaller size reveals less off-centre"
    );
    // multiply 0 and size 0 both switch it off.
    let none = run("gpu-bundled-xray-mult0", r#"{ "size": 0.5, "multiply": 0 }"#);
    assert!((at(&none, 32, 32)[0] - at(&plain, 32, 32)[0]).abs() < 0.01);
    let off = run("gpu-bundled-xray-size0", r#"{ "size": 0, "multiply": 1 }"#);
    assert!((at(&off, 32, 32)[0] - at(&plain, 32, 32)[0]).abs() < 0.01);
}
