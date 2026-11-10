// src-tauri/examples/wallpaper_smoke.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let probe_mode = std::env::var("TRUONGPAPER_SMOKE_PROBE").as_deref() == Ok("1");
    println!("[wallpaper_smoke] Starting smoke check (probe_mode = {probe_mode})...");

    // 1. Validate headless wgpu adapter + shader module compilation
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("No compatible wgpu adapter found on runner")?;

        let (device, _queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await?;

        let _blur_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kawase_blur.wgsl"),
            source: wgpu::ShaderSource::Wgsl(truongpaper_renderer::KAWASE_BLUR_WGSL.into()),
        });
        let _comp_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor.wgsl"),
            source: wgpu::ShaderSource::Wgsl(truongpaper_renderer::COMPOSITOR_WGSL.into()),
        });

        println!(
            "[wallpaper_smoke] wgpu adapter & shaders initialized: {:?}",
            adapter.get_info().name
        );
        drop(device);
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;

    // 2. On Windows, verify Progman HWND discovery
    #[cfg(target_os = "windows")]
    if probe_mode {
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        let progman = unsafe { FindWindowW(w!("Progman"), PCWSTR::null()) }?;
        if progman.0.is_null() {
            return Err("Progman HWND not found on Windows desktop".into());
        }
        println!(
            "[wallpaper_smoke] Verified Windows Progman HWND: {:?}",
            progman
        );
    }

    println!("[wallpaper_smoke] PASS");
    Ok(())
}
