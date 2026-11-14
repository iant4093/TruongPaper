//! Small still previews of library wallpapers, served only to Studio and its tray popover.
//!
//! Workshop previews are often full-size or animated GIFs. Showing them as-is made the Studio
//! web view hold hundreds of megabytes of decoded frames, so tiles get a 512 px JPEG of the
//! first frame instead, cached on disk next to the other app caches.

use crate::db;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tauri::http::{header, Method, Request, Response, StatusCode};

pub const SCHEME: &str = "wallpaper-preview";
/// Longest edge of a cached preview, in pixels.
const PREVIEW_EDGE: u32 = 512;
/// Larger sources are refused rather than decoded.
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
static GENERATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What `serve` needs from the app state.
#[derive(Clone)]
pub struct PreviewContext {
    pub db: Arc<Mutex<Connection>>,
    pub cache_dir: PathBuf,
}

pub async fn serve(ctx: Option<PreviewContext>, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(ctx) = ctx else {
        return empty(StatusCode::FORBIDDEN);
    };
    let raw = request.uri().path().trim_start_matches('/');
    let Ok(id) = percent_decode(raw) else {
        return empty(StatusCode::BAD_REQUEST);
    };
    if request.method() != Method::GET || id.is_empty() || id.len() > 200 {
        return empty(StatusCode::BAD_REQUEST);
    }
    let row = db::get_wallpaper(&ctx.db.lock(), &id).ok().flatten();
    let Some((install_dir, preview)) =
        row.and_then(|r| Some((PathBuf::from(r.install_dir), PathBuf::from(r.preview_path?))))
    else {
        return empty(StatusCode::NOT_FOUND);
    };
    match load(&ctx.cache_dir, &id, &install_dir, &preview).await {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "private, max-age=3600")
            .body(bytes)
            .expect("valid preview response"),
        Err(error) => {
            tracing::debug!("wallpaper preview unavailable for {id}: {error}");
            empty(StatusCode::NOT_FOUND)
        }
    }
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(Vec::new())
        .expect("valid status")
}

fn percent_decode(raw: &str) -> Result<String, ()> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = raw.get(i + 1..i + 3).ok_or(())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| ())?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

/// Stable cache file name for a wallpaper id (FNV-1a, so ids never reach the file system).
fn cache_file(cache_dir: &Path, id: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    cache_dir.join(format!("{hash:016x}.jpg"))
}

async fn load(
    cache_dir: &Path,
    id: &str,
    install_dir: &Path,
    preview: &Path,
) -> anyhow::Result<Vec<u8>> {
    let root = tokio::fs::canonicalize(install_dir).await?;
    let source = tokio::fs::canonicalize(preview).await?;
    anyhow::ensure!(
        source.starts_with(&root),
        "preview is outside its wallpaper folder"
    );
    let meta = tokio::fs::metadata(&source).await?;
    anyhow::ensure!(meta.len() <= MAX_SOURCE_BYTES, "preview file is too large");
    let modified = meta.modified()?;
    let cached = cache_file(cache_dir, id);
    if let Some(bytes) = fresh(&cached, modified).await {
        return Ok(bytes);
    }
    // One decode at a time keeps a burst of new tiles from spiking memory.
    let _one = GENERATE.lock().await;
    if let Some(bytes) = fresh(&cached, modified).await {
        return Ok(bytes);
    }
    let cache_dir = cache_dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let bytes = encode(&source)?;
        std::fs::create_dir_all(&cache_dir)?;
        let temp = cached.with_extension("part");
        if std::fs::write(&temp, &bytes)
            .and_then(|()| std::fs::rename(&temp, &cached))
            .is_err()
        {
            let _ = std::fs::remove_file(&temp);
        }
        Ok(bytes)
    })
    .await?
}

async fn fresh(path: &Path, source_modified: SystemTime) -> Option<Vec<u8>> {
    let meta = tokio::fs::symlink_metadata(path).await.ok()?;
    if meta.is_file() && meta.modified().ok()? >= source_modified {
        return tokio::fs::read(path).await.ok();
    }
    None
}

/// First frame, scaled to fit `PREVIEW_EDGE`, as a JPEG.
fn encode(source: &Path) -> anyhow::Result<Vec<u8>> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    let mut reader = image::ImageReader::open(source)?.with_guessed_format()?;
    reader.limits(limits);
    let image = reader.decode()?;
    let image = if image.width().max(image.height()) > PREVIEW_EDGE {
        image.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
    } else {
        image
    };
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 82)
        .encode_image(&image.into_rgb8())?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_percent_escapes_and_rejects_bad_ones() {
        assert_eq!(percent_decode("workshop%3A123").unwrap(), "workshop:123");
        assert_eq!(percent_decode("local:a-b").unwrap(), "local:a-b");
        assert!(percent_decode("bad%zz").is_err());
        assert!(percent_decode("cut%4").is_err());
    }

    #[test]
    fn cache_names_never_contain_the_id() {
        let dir = Path::new("/c");
        let a = cache_file(dir, "workshop:1/../../x");
        assert_eq!(a.parent(), Some(dir));
        assert_ne!(a, cache_file(dir, "workshop:2"));
    }

    #[test]
    fn encodes_a_small_still_from_a_large_image() {
        let dir = std::env::temp_dir().join(format!("tp-wallpaper-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("preview.png");
        image::RgbImage::from_pixel(2000, 1000, image::Rgb([10, 200, 30]))
            .save(&source)
            .unwrap();
        let bytes = encode(&source).unwrap();
        let out = image::load_from_memory(&bytes).unwrap();
        assert_eq!((out.width(), out.height()), (512, 256));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
