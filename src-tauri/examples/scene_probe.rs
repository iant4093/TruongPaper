//! Renders one Workshop scene offscreen and saves a PNG frame, for checking scene fixes.
//!
//! `cargo run --example scene_probe -- <wallpaper dir> <core assets dir> <out.png>`
//! Set `PROBE_TEXTURE=<path in package>` to save one decoded texture instead.
use std::path::PathBuf;
use truongpaper_lib::scene_runtime::SceneRuntime;
use truongpaper_renderer::{scene_renderer::FrameUniformInputs, SceneRenderer};
use truongpaper_we_formats::WeVirtualFileSystem;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("warn")
        .with_writer(std::io::stderr)
        .init();
    pollster::block_on(render())
}

async fn render() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .unwrap();
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            },
            None,
        )
        .await?;
    let dir = PathBuf::from(&args[1]);
    let vfs = WeVirtualFileSystem::mount(dir.clone(), PathBuf::from(&args[2]));
    let project: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("project.json"))?)?;
    // Same shape the app passes: project.json defaults overlaid with saved values.
    let mut merged = truongpaper_lib::commands::monitor::effective_user_properties(
        &project,
        &Default::default(),
    );
    // PROBE_PROPS='{"rays":false}' changes properties; PROBE_LOAD=1 passes them at load,
    // otherwise they are applied live after a warm-up frame (the drawer's code path).
    let probe_props: serde_json::Map<String, serde_json::Value> = match std::env::var("PROBE_PROPS")
    {
        Ok(v) => serde_json::from_str(&v)?,
        Err(_) => Default::default(),
    };
    let at_load = std::env::var("PROBE_LOAD").is_ok();
    if at_load {
        merged.extend(probe_props.clone());
    }
    let defaults = Some(serde_json::Value::Object(merged).to_string());
    if let Ok(file) = std::env::var("PROBE_FILE") {
        // Dumps one packaged file (e.g. scene.json) to stdout.
        use std::io::Write;
        std::io::stdout().write_all(&vfs.read_bytes(&file)?)?;
        return Ok(());
    }
    if let Ok(texture_path) = std::env::var("PROBE_TEXTURE") {
        let bytes = vfs.read_bytes(&texture_path)?;
        let tex = truongpaper_we_formats::parse_tex(&bytes)?;
        let mip = &tex.images[0][0];
        let data = truongpaper_we_formats::decode_tex_mipmap_to_rgba8(
            tex.header.format,
            tex.free_image_format,
            mip,
        )?;
        eprintln!(
            "tex {:?} mip {}x{} uv {:?}",
            tex.header,
            mip.width,
            mip.height,
            tex.uv_scale()
        );
        image::save_buffer(
            &args[3],
            &data,
            mip.width as u32,
            mip.height as u32,
            image::ColorType::Rgba8,
        )?;
        return Ok(());
    }
    let mut runtime = SceneRuntime::load(&vfs, "scene.json", defaults.as_deref(), [1280.0, 720.0])?;
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = SceneRenderer::new(&device, &queue, fmt, runtime.graph().scene_size);
    let warnings = renderer.load(&device, &queue, &vfs, runtime.graph());
    eprintln!(
        "{} layers, {} renderer warnings",
        runtime.graph().layers.len(),
        warnings.len()
    );
    let width = 1280;
    let height = 720;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (width * height * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut loaded_epoch = runtime.graph().render_epoch();
    let mut pending_props = (!at_load && !probe_props.is_empty())
        .then(|| serde_json::Value::Object(probe_props.clone()).to_string());
    let frame_count: u32 = args.get(4).map(|v| v.parse().unwrap()).unwrap_or(2);
    for i in 0..frame_count {
        let mut encoder = device.create_command_encoder(&Default::default());
        runtime.step(1.0 / 30.0, [0.5, 0.5], false, &[0.0; 64], &[0.0; 64]);
        if i == 1 {
            if let Some(json) = pending_props.take() {
                runtime.apply_user_properties(&json)?;
                if runtime.graph().render_epoch() != loaded_epoch {
                    loaded_epoch = runtime.graph().render_epoch();
                    renderer.load(&device, &queue, &vfs, runtime.graph());
                    eprintln!("renderer reloaded");
                }
            }
        }
        renderer.render(
            &device,
            &queue,
            &mut encoder,
            &view,
            runtime.graph(),
            runtime.camera(),
            &FrameUniformInputs {
                time: args
                    .get(5)
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or_default()
                    + i as f32 / 30.0,
                ..Default::default()
            },
        );
        if i + 1 == frame_count {
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::ImageCopyBuffer {
                    buffer: &output,
                    layout: wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(height),
                    },
                },
                target.size(),
            );
        }
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::Maintain::Wait);
    }
    output
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device.poll(wgpu::Maintain::Wait);
    let pixels = output.slice(..).get_mapped_range().to_vec();
    image::save_buffer(&args[3], &pixels, width, height, image::ColorType::Rgba8)?;
    if let Ok(reference) = std::env::var("PROBE_DIFF") {
        let other = image::open(&reference)?.to_rgba8();
        let total: u64 = pixels
            .iter()
            .zip(other.as_raw())
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum();
        println!(
            "mean abs diff vs {reference}: {:.3}",
            total as f64 / pixels.len() as f64
        );
    }
    Ok(())
}
