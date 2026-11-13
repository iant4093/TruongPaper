//! Renders one Workshop scene offscreen and saves a PNG frame, for checking scene fixes.
//!
//! `cargo run --example scene_probe -- <wallpaper dir> <core assets dir> <out.png>`
//! Set `PROBE_TEXTURE=<path in package>` to save one decoded texture instead.
use std::path::PathBuf;
use truongpaper_lib::scene_runtime::SceneRuntime;
use truongpaper_renderer::{scene_renderer::FrameUniformInputs, SceneRenderer};
use truongpaper_we_formats::WeVirtualFileSystem;

/// Synthetic spectrum for `PROBE_AUDIO=sine`: bass-heavy bands with a per-frame wobble.
fn probe_spectrum(frame: u32) -> [f32; 64] {
    let t = frame as f32 / 30.0;
    std::array::from_fn(|b| {
        let x = b as f32 / 63.0;
        let wobble = 0.5 + 0.5 * (t * 6.0 + b as f32 * 0.7).sin();
        ((1.0 - 0.7 * x) * (0.3 + 0.7 * wobble)).clamp(0.0, 1.0)
    })
}

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
    // Our bundled stock effects/includes sit behind the wallpaper and real WE core assets.
    // PROBE_NO_BUNDLED=1 renders without them.
    if std::env::var("PROBE_NO_BUNDLED").is_err() {
        truongpaper_we_formats::set_bundled_assets_dir(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/core-assets/assets"),
        );
    }
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
    // PROBE_MONITOR=3456x2234 sets the monitor size handed to the scene (engine.screenResolution),
    // PROBE_OUT=1920x1080 sets the rendered image size.
    let dims = |key: &str, default: (u32, u32)| -> (u32, u32) {
        std::env::var(key)
            .ok()
            .and_then(|v| {
                let (w, h) = v.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or(default)
    };
    let (mon_w, mon_h) = dims("PROBE_MONITOR", (1280, 720));
    let mut runtime = SceneRuntime::load(
        &vfs,
        "scene.json",
        defaults.as_deref(),
        [mon_w as f32, mon_h as f32],
    )?;
    // PROBE_LIST=1 prints layers; PROBE_SHOW="i,j,.." renders only those layer indices.
    if std::env::var("PROBE_LIST").is_ok() {
        for (i, l) in runtime.graph().layers.iter().enumerate() {
            let k = format!("{:?}", l.kind);
            eprintln!(
                "L{i} {} vis={} a={} {}",
                l.key(),
                l.visible,
                l.alpha,
                k.chars().take(24).collect::<String>()
            );
            eprintln!("    {:?}", l.transform);
        }
    }
    if let Ok(show) = std::env::var("PROBE_SHOW") {
        let keep: Vec<usize> = show.split(',').filter_map(|v| v.parse().ok()).collect();
        for (i, l) in runtime.graph_mut().layers.iter_mut().enumerate() {
            l.visible &= keep.contains(&i);
        }
    }
    if let Some(z) = std::env::var("PROBE_ZOOM")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        runtime.graph_mut().camera.zoom = z;
    }
    // PROBE_FXKEEP="0,2" keeps only those effect indices on every layer.
    if let Ok(keep) = std::env::var("PROBE_FXKEEP") {
        let keep: Vec<usize> = keep
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        for l in runtime.graph_mut().layers.iter_mut() {
            let mut i = 0;
            l.effects.retain(|_| {
                i += 1;
                keep.contains(&(i - 1))
            });
        }
    }
    if std::env::var("PROBE_NOFX").is_ok() {
        for l in runtime.graph_mut().layers.iter_mut() {
            l.effects.clear();
        }
    }
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = SceneRenderer::new(&device, &queue, fmt, runtime.graph().scene_size);
    let warnings = renderer.load(&device, &queue, &vfs, runtime.graph());
    // Embedded MP4 textures stay white until the host pushes frames; the probe pushes the first
    // frame decoded with `ffmpeg` (when installed) so renders look like the running wallpaper.
    for path in renderer.embedded_video_paths() {
        let Ok(bytes) = vfs.read_bytes(&path) else {
            continue;
        };
        let Ok(tex) = truongpaper_we_formats::parse_tex(&bytes) else {
            continue;
        };
        let Some(mip) = tex.images.first().and_then(|m| m.first()) else {
            continue;
        };
        let tmp = std::env::temp_dir().join(format!("probe-{}.mp4", std::process::id()));
        if std::fs::write(&tmp, &mip.raw_bytes).is_err() {
            continue;
        }
        let out = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-i"])
            .arg(&tmp)
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output();
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
            .args(["stream=width,height", "-of", "csv=p=0"])
            .arg(&tmp)
            .output();
        let _ = std::fs::remove_file(&tmp);
        let (Ok(out), Ok(probe)) = (out, probe) else {
            continue;
        };
        let dims = String::from_utf8_lossy(&probe.stdout);
        let mut it = dims
            .trim()
            .split(',')
            .filter_map(|v| v.trim().parse::<u32>().ok());
        let (Some(w), Some(h)) = (it.next(), it.next()) else {
            continue;
        };
        if out.stdout.len() == (w * h * 4) as usize {
            let frame = truongpaper_renderer::video_runtime::VideoFrame {
                width: w,
                height: h,
                stride: (w * 4) as usize,
                format: truongpaper_renderer::video_runtime::VideoPixelFormat::Rgba8,
                data: out.stdout,
            };
            renderer.upload_video_frame(&queue, &path, &frame);
        }
    }
    eprintln!(
        "{} layers, {} renderer warnings",
        runtime.graph().layers.len(),
        warnings.len()
    );
    let (out_w, out_h) = dims("PROBE_OUT", (1280, 720));
    // The desktop host shows a scene with `FitMode::Fill` (cover), so a non-16:9 orthographic
    // scene (e.g. 5760x1080) is cropped, not squeezed. The probe renders a cover-sized target and
    // crops its centre; PROBE_STRETCH=1 keeps the old squeezed output.
    let (width, height) = if std::env::var("PROBE_STRETCH").is_ok() {
        (out_w, out_h)
    } else {
        cover_render_size(runtime.graph().scene_size.to_array(), out_w, out_h)
    };
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
    // PROBE_AUDIO=sine: synthetic 64-band spectrum that moves per frame, to check that
    // audio-reactive layers and scripts respond (silence otherwise).
    let synth_audio = std::env::var("PROBE_AUDIO").is_ok_and(|v| v == "sine");
    for i in 0..frame_count {
        let mut encoder = device.create_command_encoder(&Default::default());
        let audio = if synth_audio {
            probe_spectrum(i)
        } else {
            [0.0; 64]
        };
        runtime.step(1.0 / 30.0, [0.5, 0.5], false, &audio, &audio);
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
                audio_64: [audio, audio],
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
    // PROBE_TRACE=a,b: prints local/world state of layers whose name contains a filter.
    if let Ok(filter) = std::env::var("PROBE_TRACE") {
        let g = runtime.graph();
        for (i, l) in g.layers.iter().enumerate() {
            if filter
                .split(',')
                .any(|f| !f.is_empty() && l.name.contains(f))
            {
                eprintln!(
                    "T L{i} {} vis={} eff={} o={:?} s={:?} wpos={:?} wscale=({:.3},{:.3},{:.3})",
                    l.key(),
                    l.visible,
                    g.effective_visible(i),
                    l.transform.origin,
                    l.transform.scale,
                    l.world_matrix.w_axis.truncate(),
                    l.world_matrix.x_axis.truncate().length(),
                    l.world_matrix.y_axis.truncate().length(),
                    l.world_matrix.z_axis.truncate().length()
                );
            }
        }
        for l in g.layers.iter() {
            if let truongpaper_renderer::scene_graph::SceneLayerKind::Model { mdl_path } = &l.kind {
                if filter
                    .split(',')
                    .any(|f| !f.is_empty() && l.name.contains(f))
                {
                    if let Ok(m) = vfs
                        .read_bytes(mdl_path)
                        .map_err(|e| e.to_string())
                        .and_then(|b| {
                            truongpaper_we_formats::parse_static_mdl(&b).map_err(|e| e.to_string())
                        })
                    {
                        let mut vol = 0.0f64;
                        for t in m.indices.chunks_exact(3) {
                            let p = |i: u32| m.vertices[i as usize].position.map(f64::from);
                            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                            vol += a[0] * (b[1] * c[2] - b[2] * c[1])
                                - a[1] * (b[0] * c[2] - b[2] * c[0])
                                + a[2] * (b[0] * c[1] - b[1] * c[0]);
                        }
                        // Triangles by winding (+ = CCW outward) and mean radius.
                        let (mut np, mut nn, mut rp, mut rn) = (0, 0, 0.0f64, 0.0f64);
                        for t in m.indices.chunks_exact(3) {
                            let p = |i: u32| m.vertices[i as usize].position.map(f64::from);
                            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                            let n = [
                                e1[1] * e2[2] - e1[2] * e2[1],
                                e1[2] * e2[0] - e1[0] * e2[2],
                                e1[0] * e2[1] - e1[1] * e2[0],
                            ];
                            let cen = [
                                (a[0] + b[0] + c[0]) / 3.0,
                                (a[1] + b[1] + c[1]) / 3.0,
                                (a[2] + b[2] + c[2]) / 3.0,
                            ];
                            let r = (cen[0] * cen[0] + cen[1] * cen[1] + cen[2] * cen[2]).sqrt();
                            if n[0] * cen[0] + n[1] * cen[1] + n[2] * cen[2] >= 0.0 {
                                np += 1;
                                rp += r;
                            } else {
                                nn += 1;
                                rn += r;
                            }
                        }
                        eprintln!(
                            "T tris outward={np} (mean r {:.3}) inward={nn} (mean r {:.3})",
                            rp / np.max(1) as f64,
                            rn / nn.max(1) as f64
                        );
                        eprintln!(
                            "T mdl {} bbox {:?}..{:?} signed_volume {:.3}",
                            l.name,
                            m.bbox_min,
                            m.bbox_max,
                            vol / 6.0
                        );
                    }
                }
            }
        }
        eprintln!("T camera {:?}", runtime.camera());
    }
    output
        .slice(..)
        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
    device.poll(wgpu::Maintain::Wait);
    let pixels = output.slice(..).get_mapped_range().to_vec();
    let pixels = crop_center(&pixels, width, height, out_w, out_h);
    let (width, height) = (out_w, out_h);
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

/// Render-target size that covers `out_w`x`out_h` with the scene's aspect ratio. The width is a
/// multiple of 64 so `bytes_per_row` stays 256-byte aligned for the readback copy.
fn cover_render_size(scene: [f32; 2], out_w: u32, out_h: u32) -> (u32, u32) {
    let aspect = (scene[0] / scene[1].max(1.0)).max(0.001);
    let out_aspect = out_w as f32 / out_h as f32;
    let (w, h) = if aspect > out_aspect {
        ((out_h as f32 * aspect).round() as u32, out_h)
    } else {
        (out_w, (out_w as f32 / aspect).round() as u32)
    };
    (w.max(out_w).div_ceil(64) * 64, h.max(out_h).min(8192))
}

/// Centre `out_w`x`out_h` crop of a tightly packed RGBA8 `width`x`height` image.
fn crop_center(pixels: &[u8], width: u32, height: u32, out_w: u32, out_h: u32) -> Vec<u8> {
    if (width, height) == (out_w, out_h) {
        return pixels.to_vec();
    }
    let (x0, y0) = ((width - out_w) / 2, (height - out_h) / 2);
    let mut out = Vec::with_capacity((out_w * out_h * 4) as usize);
    for y in y0..y0 + out_h {
        let start = ((y * width + x0) * 4) as usize;
        out.extend_from_slice(&pixels[start..start + (out_w * 4) as usize]);
    }
    out
}
