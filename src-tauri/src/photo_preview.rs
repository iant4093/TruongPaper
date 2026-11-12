//! Small local photo previews served only to Studio and its tray popover.

use crate::commands::photo::PhotoServiceState;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tauri::http::{header, Method, Request, Response, StatusCode};
use truongpaper_core::{get_cached_photo, photo_preview_path};
use truongpaper_photo_api::{decoder::encode_photo_preview, queue::is_safe_photo_id};

pub const SCHEME: &str = "photo-preview";
static GENERATE_PREVIEW: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn serve(
    state: Option<PhotoServiceState>,
    request: Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let id = request.uri().path().trim_start_matches('/');
    if request.method() != Method::GET || !is_safe_photo_id(id) {
        return empty_response(StatusCode::BAD_REQUEST);
    }
    let Some(state) = state else {
        return empty_response(StatusCode::FORBIDDEN);
    };
    let row = get_cached_photo(&state.db.lock(), id).ok().flatten();
    let Some(row) = row.filter(|photo| photo.file_present) else {
        return empty_response(StatusCode::NOT_FOUND);
    };
    let root = state.app_data_dir.join("wallpapers").join("photos");
    let source = state.app_data_dir.join(row.file_path);
    match load_preview(root, source).await {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "private, max-age=3600")
            .body(bytes)
            .expect("valid preview response"),
        Err(error) => {
            tracing::debug!("photo preview unavailable for {id}: {error}");
            empty_response(StatusCode::NOT_FOUND)
        }
    }
}

fn empty_response(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(Vec::new())
        .expect("valid status")
}

async fn load_preview(root: PathBuf, source: PathBuf) -> anyhow::Result<Vec<u8>> {
    let root = tokio::fs::canonicalize(root).await?;
    let original = tokio::fs::canonicalize(source).await?;
    anyhow::ensure!(original.starts_with(root), "photo is outside its cache");
    let preview = photo_preview_path(&original);
    let modified = tokio::fs::metadata(&original).await?.modified()?;
    if let Some(bytes) = cached_preview(&preview, modified).await {
        return Ok(bytes);
    }

    // One full-size decode at a time, on a worker thread. Warm thumbnails bypass this lock.
    let _generate = GENERATE_PREVIEW.lock().await;
    let modified = tokio::fs::metadata(&original).await?.modified()?;
    if let Some(bytes) = cached_preview(&preview, modified).await {
        return Ok(bytes);
    }
    tokio::task::spawn_blocking(move || {
        let bytes = encode_photo_preview(&original)?;
        anyhow::ensure!(
            std::fs::metadata(&original)?.modified()? == modified,
            "original changed while generating its preview"
        );
        if let Err(error) = save_preview(&preview, &bytes) {
            tracing::debug!("could not cache photo preview: {error}");
        }
        if !original.is_file() {
            let _ = std::fs::remove_file(&preview);
            anyhow::bail!("original was removed");
        }
        Ok(bytes)
    })
    .await?
}

async fn cached_preview(path: &Path, original_modified: SystemTime) -> Option<Vec<u8>> {
    let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
    if metadata.is_file() && metadata.modified().ok()? >= original_modified {
        return tokio::fs::read(path).await.ok();
    }
    None
}

fn save_preview(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("part");
    let result = std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
