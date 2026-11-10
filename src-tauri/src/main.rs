// src-tauri/src/main.rs
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Keep startup/render/download failures visible in local test logs.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "truongpaper=info,truongpaper_platform=info,truongpaper_audio=info".into()
            }),
        )
        .try_init();
    truongpaper_lib::run();
}
