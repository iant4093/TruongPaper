//! Cached 1.5x playback copies for macOS WebKit, which thins video frames above 2x.

use parking_lot::Mutex;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use truongpaper_we_formats::WeVirtualFileSystem;

// Serialize exports so changing speed on several displays cannot flood the video encoder.
static EXPORT_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Debug, PartialEq)]
enum Status {
    Waiting,
    Exporting,
    Ready,
    Failed,
}

#[derive(Debug)]
pub(crate) struct FastVideo {
    source: PathBuf,
    output: PathBuf,
    virtual_path: String,
    status: Mutex<Status>,
}

impl FastVideo {
    pub(crate) fn mount(
        vfs: &mut WeVirtualFileSystem,
        project_file: &str,
        cache_dir: &Path,
    ) -> Option<Arc<Self>> {
        let relative = WeVirtualFileSystem::sanitize_virtual_path(project_file).ok()?;
        let root = vfs.wallpaper_dir.canonicalize().ok()?;
        let source = root.join(relative).canonicalize().ok()?;
        let metadata = source.metadata().ok()?;
        if !source.starts_with(&root) || !metadata.is_file() {
            return None;
        }
        let mut hash = DefaultHasher::new();
        (1u8, &source, metadata.len(), metadata.modified().ok()).hash(&mut hash);
        let file_name = format!("{:016x}.mp4", hash.finish());
        let output = cache_dir.join("fast-video").join(&file_name);
        let virtual_path = format!("__truongpaper_fast/{file_name}");
        vfs.mount_generated_video(virtual_path.clone(), output.clone());
        Some(Arc::new(Self {
            source,
            output,
            virtual_path,
            status: Mutex::new(Status::Waiting),
        }))
    }

    pub(crate) fn virtual_path(&self) -> &str {
        &self.virtual_path
    }

    /// Starts preparation on demand. Subsequent calls only check the in-memory status.
    pub(crate) fn ready(self: &Arc<Self>) -> bool {
        let mut status = self.status.lock();
        if *status == Status::Waiting {
            *status = Status::Exporting;
            let video = Arc::clone(self);
            tauri::async_runtime::spawn(async move {
                let Ok(_permit) = EXPORT_SLOT.acquire().await else {
                    return;
                };
                let export = Arc::clone(&video);
                let result = tokio::task::spawn_blocking(move || export.prepare()).await;
                *video.status.lock() = match result {
                    Ok(Ok(())) => Status::Ready,
                    error => {
                        tracing::warn!(?error, "could not prepare macOS fast video");
                        Status::Failed
                    }
                };
            });
        }
        *status == Status::Ready
    }

    fn prepare(&self) -> anyhow::Result<()> {
        if self.output.is_file() {
            return Ok(());
        }
        if let Some(parent) = self.output.parent() {
            fs::create_dir_all(parent)?;
        }
        let partial = self
            .output
            .with_extension(format!("{}.part.mp4", std::process::id()));
        let _ = fs::remove_file(&partial);
        let result = truongpaper_platform::mac_video::export_fast_video(&self.source, &partial)
            .and_then(|()| fs::rename(&partial, &self.output).map_err(Into::into));
        if result.is_err() {
            let _ = fs::remove_file(partial);
        }
        result
    }
}
