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

#[cfg(target_os = "windows")]
mod sync {
    use super::*;
    use crate::commands::monitor::live_monitors;
    use crate::models::EVENT_WALLPAPER_CHANGED;
    use std::sync::Arc;
    use tauri::{Listener, Manager};
    use tokio::sync::Notify;

    struct SyncRequest(Arc<Notify>);

    pub fn start(app: &AppHandle) {
        let pending = Arc::new(Notify::new());
        app.manage(SyncRequest(pending.clone()));
        let app = app.clone();
        let on_event = app.clone();
        app.listen(EVENT_WALLPAPER_CHANGED, move |_| {
            if on_event
                .state::<AppState>()
                .settings
                .lock()
                .sync_lock_screen
            {
                request(&on_event);
            }
        });
        tauri::async_runtime::spawn(async move {
            let mut last = None;
            loop {
                pending.notified().await;
                let state = app.state::<AppState>();
                if !state.settings.lock().sync_lock_screen {
                    last = None;
                    continue;
                }
                if let Err(error) = update(&app, &state, &mut last).await {
                    tracing::warn!("could not sync lock screen: {error}");
                }
            }
        });
    }

    pub fn request(app: &AppHandle) {
        if let Some(pending) = app.try_state::<SyncRequest>() {
            pending.0.notify_one();
        }
    }

    async fn update(
        app: &AppHandle,
        state: &AppState,
        last: &mut Option<(String, u64)>,
    ) -> Result<(), String> {
        let _setting = SET_LOCK_SCREEN.lock().await;
        if !state.settings.lock().sync_lock_screen {
            return Ok(());
        }
        let Some(monitor) = live_monitors(app).into_iter().find(|m| m.is_primary) else {
            return Ok(());
        };
        let Ok((generation, source)) = capture_source(state, &monitor.id).await else {
            // Some wallpapers have no local still. Leave the last usable image in place.
            return Ok(());
        };
        let key = (monitor.id, generation);
        if last.as_ref() == Some(&key) {
            return Ok(());
        }
        let folder = state.app_data_dir.join("lock-screen").join("sync");
        let output = prepare_output(source, folder.clone()).await?;
        // Encoding may take a while. A wallpaper change queues another pass, so drop this
        // result when the monitor already shows something else.
        if !state.settings.lock().sync_lock_screen
            || !state
                .host
                .monitor(&key.0)
                .is_some_and(|rt| rt.generation == key.1)
        {
            let _ = tokio::fs::remove_file(&output).await;
            return Ok(());
        }
        if let Err(error) = apply_still(app.clone(), output.clone()).await {
            let _ = tokio::fs::remove_file(&output).await;
            return Err(error);
        }
        *last = Some(key);
        // Keep the image Windows currently references; replace it with a new filename each time.
        if let Ok(mut files) = tokio::fs::read_dir(folder).await {
            while let Ok(Some(file)) = files.next_entry().await {
                let path = file.path();
                if path != output && is_generated_still(&path) {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn is_generated_still(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("wallpaper-")?.strip_suffix(".jpg"))
        .and_then(|name| name.split_once('-'))
        .is_some_and(|(pid, millis)| pid.parse::<u32>().is_ok() && millis.parse::<u64>().is_ok())
}

#[cfg(target_os = "windows")]
pub(crate) use sync::{request as request_sync, start as start_sync};

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
    let (_, source) = capture_source(&state, &monitor_id).await?;
    let output = prepare_output(source, state.app_data_dir.join("lock-screen")).await?;
    if let Err(error) = apply_still(app, output.clone()).await {
        let _ = tokio::fs::remove_file(&output).await;
        return Err(error);
    }
    Ok(())
}

async fn capture_source(state: &AppState, monitor_id: &str) -> Result<(u64, PathBuf), String> {
    let _apply = lock_monitor_apply(monitor_id).await;
    let runtime = state
        .host
        .monitor(monitor_id)
        .ok_or("No wallpaper is active on this display")?;
    let path = match runtime.active {
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
    };
    Ok((runtime.generation, path))
}

async fn prepare_output(source: PathBuf, folder: PathBuf) -> Result<PathBuf, String> {
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
    Ok(output)
}

#[cfg(target_os = "macos")]
async fn apply_still(app: AppHandle, output: PathBuf) -> Result<(), String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = objc2::MainThreadMarker::new()
            .ok_or_else(|| "Could not access the macOS wallpaper settings".into())
            .map(|mtm| truongpaper_platform::mac_desktop::reveal_system_wallpaper(mtm, &output));
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
