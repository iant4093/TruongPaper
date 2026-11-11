//! Publish a still of the selected wallpaper through the system personalization API.

use crate::commands::library::is_strictly_within;
use crate::commands::monitor::{lock_monitor_apply, validate_monitor_id};
use crate::host::ActiveRuntime;
use crate::{db, AppState};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

static SET_LOCK_SCREEN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn prepare_still(source: &Path, output: &Path) -> anyhow::Result<()> {
    let decoded = truongpaper_photo_api::decode_photo_for_wgpu(source, 3840)?;
    let image = image::DynamicImage::ImageRgba8(decoded.rgba).into_rgb8();
    let mut writer = BufWriter::new(fs::File::create_new(output)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 90).encode_image(&image)?;
    writer.flush()?;
    Ok(())
}

#[tauri::command]
pub async fn wallpaper_set_lock_screen(
    app: AppHandle,
    state: State<'_, AppState>,
    monitor_id: String,
) -> Result<(), String> {
    validate_monitor_id(&monitor_id)?;
    let _setting = SET_LOCK_SCREEN.lock().await;
    let source = {
        let _apply = lock_monitor_apply(&monitor_id).await;
        let runtime = state
            .host
            .monitor(&monitor_id)
            .ok_or("No wallpaper is active on this display")?;
        match runtime.active {
            ActiveRuntime::Photo { path: Some(path) } => path,
            ActiveRuntime::None | ActiveRuntime::Photo { path: None } => {
                return Err("No wallpaper is active on this display".into());
            }
            _ => {
                let id = runtime
                    .wallpaper_id
                    .ok_or("No wallpaper is active on this display")?;
                let row = db::get_wallpaper(&state.db.lock(), &id)?
                    .ok_or("Wallpaper is no longer installed")?;
                let path = row
                    .preview_path
                    .map(PathBuf::from)
                    .ok_or("This wallpaper has no preview image")?;
                if !is_strictly_within(&path, Path::new(&row.install_dir)) {
                    return Err("This wallpaper has no local preview image".into());
                }
                path
            }
        }
    };

    let folder = state.app_data_dir.join("lock-screen");
    let output = folder.join(format!(
        "wallpaper-{}-{}.jpg",
        std::process::id(),
        truongpaper_core::now_ms()
    ));
    let still = output.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        fs::create_dir_all(&folder)?;
        prepare_still(&source, &still)
    })
    .await
    .map_err(|err| err.to_string())?;
    if let Err(error) = prepared {
        let _ = tokio::fs::remove_file(&output).await;
        return Err(format!("Could not prepare a wallpaper image: {error}"));
    }
    if let Err(error) = apply_still(app, output.clone()).await {
        let _ = tokio::fs::remove_file(&output).await;
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn apply_still(app: AppHandle, output: PathBuf) -> Result<(), String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = objc2::MainThreadMarker::new()
            .ok_or_else(|| "Could not access the macOS wallpaper settings".into())
            .and_then(|mtm| {
                truongpaper_platform::mac_desktop::prepare_system_wallpaper(mtm, &output)
            });
        let _ = send.send(result);
    })
    .map_err(|err| err.to_string())?;
    receive.await.map_err(|err| err.to_string())?
}

#[cfg(target_os = "windows")]
async fn apply_still(_: AppHandle, output: PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> windows::core::Result<()> {
        use windows::core::HSTRING;
        use windows::Storage::StorageFile;
        use windows::System::UserProfile::LockScreen;
        use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

        // SAFETY: both operations run on this blocking thread, with a balanced WinRT apartment.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        let result = (|| {
            let image =
                StorageFile::GetFileFromPathAsync(&HSTRING::from(output.as_os_str()))?.join()?;
            LockScreen::SetImageFileAsync(&image)?.join()
        })();
        unsafe { RoUninitialize() };
        result
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| format!("Windows could not set the lock screen image: {err}"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
async fn apply_still(_: AppHandle, _: PathBuf) -> Result<(), String> {
    Err("System wallpaper integration is available on macOS and Windows".into())
}
