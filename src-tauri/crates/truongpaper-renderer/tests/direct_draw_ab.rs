//! A/B pixel check of the direct-draw path (skipped when no adapter is available). Lives in its
//! own test binary because it sets a process-wide environment variable.
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

/// Plain solid layers (no effects) take the direct path by default. Drawing them through the
/// offscreen path (`TRUONGPAPER_NO_DIRECT_DRAW`) must give the same pixels within 1/255.
#[test]
fn direct_draw_matches_the_offscreen_path() {
    let t = TempDir::new("gpu-direct-ab");
    t.write(
        "models/solid.json",
        r#"{ "solidlayer": true, "width": 64, "height": 64 }"#,
    );
    let scene = r#"{ "general": { "orthogonalprojection": { "width": 64, "height": 64 },
                       "clearcolor": "0.1 0.2 0.3" },
        "objects": [
          { "name": "A", "image": "models/solid.json", "origin": "32 32 0", "size": "64 64",
            "color": "0.8 0.3 0.1" },
          { "name": "B", "image": "models/solid.json", "origin": "24 40 0", "size": "30 30",
            "color": "0.2 0.9 0.4", "alpha": 0.5 },
          { "name": "C", "image": "models/solid.json", "origin": "44 20 0", "size": "20 40",
            "color": "1 1 1", "brightness": 0.6, "alpha": 0.75 } ] }"#;
    std::env::remove_var("TRUONGPAPER_NO_DIRECT_DRAW");
    let Some((direct, _)) = render_hdr_with(&t, scene, 64, true) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    std::env::set_var("TRUONGPAPER_NO_DIRECT_DRAW", "1");
    let fbo = render_hdr_with(&t, scene, 64, true);
    std::env::remove_var("TRUONGPAPER_NO_DIRECT_DRAW");
    let (fbo, _) = fbo.expect("adapter vanished");
    assert_eq!(direct.len(), fbo.len());
    let worst = direct
        .iter()
        .zip(&fbo)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst <= 1.0 / 255.0, "max channel difference {worst}");
}
