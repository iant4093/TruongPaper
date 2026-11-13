//! GPU test for 3D model drawing: a near cube must occlude a far cube drawn after it
//! (skipped when no adapter is available).
use glam::{Mat4, Quat, Vec3, Vec4};
use truongpaper_renderer::model3d::{
    ModelBlend, ModelCull, ModelFrame, ModelMaterial, ModelResources, DEFAULT_LIGHT_DIR,
};
use truongpaper_we_formats::{StaticMdl, StaticMdlVertex};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()
}

fn cube() -> StaticMdl {
    let p = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        4, 5, 6, 4, 6, 7, 1, 0, 3, 1, 3, 2, 5, 1, 2, 5, 2, 6,
        0, 4, 7, 0, 7, 3, 7, 6, 2, 7, 2, 3, 0, 1, 5, 0, 5, 4,
    ];
    StaticMdl {
        version: "MDLV0023".into(),
        material: "materials/cube.json".into(),
        bbox_min: [-1.0; 3],
        bbox_max: [1.0; 3],
        vertices: p
            .iter()
            .map(|&position| StaticMdlVertex {
                position,
                normal: Vec3::from(position).normalize().to_array(),
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            })
            .collect(),
        indices,
    }
}

fn pixel_texture(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        tex.as_image_copy(),
        &rgba,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn material(depth_test: bool) -> ModelMaterial {
    ModelMaterial {
        lit: false,
        color: Vec3::ONE,
        brightness: 1.0,
        alpha: 1.0,
        emissive: Vec3::ZERO,
        blend: ModelBlend::Opaque,
        cull: ModelCull::Back,
        depth_test,
        depth_write: true,
    }
}

/// Draws the near green cube first, then the far red cube; returns the center pixel and a
/// pixel covered only by the far cube.
fn render(depth_test: bool) -> Option<([u8; 4], [u8; 4])> {
    let (device, queue) = device()?;
    let white = pixel_texture(&device, &queue, [255; 4]);
    let black = pixel_texture(&device, &queue, [0; 4]);
    let mut res = ModelResources::new(&device, FORMAT, SIZE, SIZE);
    let mesh = cube();
    let near = res.create_model(&device, &mesh, material(depth_test), &white, &black);
    let far = res.create_model(&device, &mesh, material(depth_test), &white, &black);

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
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let vp = Mat4::perspective_rh(60f32.to_radians(), 1.0, 0.1, 100.0)
        * Mat4::look_at_rh(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO, Vec3::Y);
    let frame = |model: Mat4, tint: Vec4| ModelFrame {
        mvp: vp * model,
        model,
        tint,
        ambient: Vec3::ONE,
        light_dir: DEFAULT_LIGHT_DIR,
        light_color: Vec3::ZERO,
    };
    let near_m = Mat4::from_scale_rotation_translation(
        Vec3::splat(0.5),
        Quat::from_rotation_y(0.3),
        Vec3::new(0.0, 0.0, 2.0),
    );
    let far_m = Mat4::from_scale_rotation_translation(
        Vec3::splat(2.0),
        Quat::IDENTITY,
        Vec3::new(0.0, 0.0, -3.0),
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        // Clear the color target first.
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    res.begin_frame();
    res.draw(
        &queue,
        &mut encoder,
        &view,
        &near,
        &frame(near_m, Vec4::new(0.0, 1.0, 0.0, 1.0)),
    );
    res.draw(
        &queue,
        &mut encoder,
        &view,
        &far,
        &frame(far_m, Vec4::new(1.0, 0.0, 0.0, 1.0)),
    );
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (SIZE * SIZE * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(SIZE * 4),
                rows_per_image: Some(SIZE),
            },
        },
        wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::Maintain::Wait);
    let data = buffer.slice(..).get_mapped_range().to_vec();
    let px = |x: u32, y: u32| {
        let i = ((y * SIZE + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    };
    // Far cube (half-size 2 at distance 9) spans roughly +-0.38 of the view; pick a pixel near
    // its edge but outside the near cube.
    Some((px(SIZE / 2, SIZE / 2), px(SIZE / 2 + 12, SIZE / 2)))
}

#[test]
fn depth_test_keeps_the_nearer_cube_in_front() {
    let Some((center, side)) = render(true) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    assert_eq!(
        &center[..3],
        &[0, 255, 0],
        "near green cube must win the center"
    );
    assert_eq!(&side[..3], &[255, 0, 0], "far red cube visible beside it");
}

#[test]
fn without_depth_test_the_later_cube_overwrites() {
    let Some((center, _)) = render(false) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    assert_eq!(&center[..3], &[255, 0, 0], "painter's order without depth");
}
