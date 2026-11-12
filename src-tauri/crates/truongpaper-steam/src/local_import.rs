//! Imports a local Wallpaper Engine item folder or a video file into the app's `local` root.
//!
//! The source is always copied (never moved or linked). Symlinks anywhere in the source are
//! rejected, and the copy is limited to [`MAX_IMPORT_BYTES`].

use crate::local_sync::{read_item_dir, DiscoveredWorkshopWallpaper};
use anyhow::{bail, ensure, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Largest source (sum of file sizes) that will be copied.
pub const MAX_IMPORT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ENTRIES: usize = 50_000;
const MAX_DEPTH: usize = 32;
const MAX_PROJECT_BYTES: u64 = 1024 * 1024;
pub const VIDEO_EXTENSIONS: [&str; 4] = ["mp4", "webm", "mov", "mkv"];
/// Wallpaper id prefix of imported items.
pub const LOCAL_ID_PREFIX: &str = "local:";

/// FNV-1a, enough to make slugs of different sources distinct yet stable across runs.
fn fnv1a(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |hash, b| {
        (hash ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

fn slugify(name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.truncate(40);
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "item".into()
    } else {
        slug
    }
}

/// `<slug>-<hash of the canonical source path>`: the same source always maps to the same item.
fn folder_name_for(name: &str, canonical_source: &Path) -> String {
    format!(
        "{}-{:08x}",
        slugify(name),
        fnv1a(&canonical_source.to_string_lossy())
    )
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Walks `dir`, rejecting symlinks and enforcing the size/entry limits. Returns total bytes.
fn measure(dir: &Path, depth: usize, entries: &mut usize) -> Result<u64> {
    ensure!(depth <= MAX_DEPTH, "That folder is nested too deeply.");
    let mut total = 0u64;
    for entry in fs::read_dir(dir).with_context(|| format!("Cannot read {}", dir.display()))? {
        let entry = entry?;
        *entries += 1;
        ensure!(*entries <= MAX_ENTRIES, "That folder has too many files.");
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("The folder contains a symbolic link, which cannot be imported.");
        }
        if kind.is_dir() {
            total += measure(&entry.path(), depth + 1, entries)?;
        } else if kind.is_file() {
            total += entry.metadata()?.len();
        }
        ensure!(
            total <= MAX_IMPORT_BYTES,
            "That folder is larger than 4 GB."
        );
    }
    Ok(total)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_symlink() {
            bail!("The folder contains a symbolic link, which cannot be imported.");
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Replaces `<local_root>/<folder>` with the staged copy.
fn install(staging: &Path, local_root: &Path, folder: &str) -> Result<PathBuf> {
    let dest = local_root.join(folder);
    if dest.exists() {
        fs::remove_dir_all(&dest).context("Could not replace the earlier import")?;
    }
    fs::rename(staging, &dest).context("Could not finish the import")?;
    Ok(dest)
}

fn finish(dest: &Path, folder: &str) -> Result<DiscoveredWorkshopWallpaper> {
    let mut found =
        read_item_dir(dest, folder).context("The imported folder has no readable project.json")?;
    found.wallpaper_id = format!("{LOCAL_ID_PREFIX}{folder}");
    Ok(found)
}

/// Copies a Wallpaper Engine item folder or a video file into `<local_root>/<slug>-<hash>`.
pub fn import_local(source: &Path, local_root: &Path) -> Result<DiscoveredWorkshopWallpaper> {
    let meta = fs::symlink_metadata(source).context("That path does not exist.")?;
    ensure!(
        !meta.file_type().is_symlink(),
        "Symbolic links cannot be imported."
    );
    let canonical = source.canonicalize().context("That path does not exist.")?;
    fs::create_dir_all(local_root)?;
    let root = local_root.canonicalize()?;
    ensure!(
        !canonical.starts_with(&root) && !root.starts_with(&canonical),
        "Choose a wallpaper outside TruongPaper's own library folder."
    );
    let name = canonical
        .file_stem()
        .or_else(|| canonical.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let folder = folder_name_for(&name, &canonical);
    let staging = tempfile::Builder::new()
        .prefix(".import-")
        .tempdir_in(&root)?;
    let staged = staging.path().join("item");

    if meta.is_dir() {
        let project = canonical.join("project.json");
        let project_meta =
            fs::symlink_metadata(&project).context("That folder has no project.json.")?;
        ensure!(
            project_meta.is_file() && project_meta.len() <= MAX_PROJECT_BYTES,
            "That folder has no usable project.json."
        );
        let raw = fs::read_to_string(&project).context("project.json is not readable text.")?;
        serde_json::from_str::<serde_json::Value>(raw.trim_start_matches('\u{feff}'))
            .context("project.json is not valid JSON.")?;
        measure(&canonical, 0, &mut 0)?;
        copy_tree(&canonical, &staged)?;
    } else if meta.is_file() {
        ensure!(
            is_video(&canonical),
            "Choose a folder with project.json or a .mp4, .webm, .mov or .mkv video."
        );
        ensure!(meta.len() > 0, "That video file is empty.");
        ensure!(
            meta.len() <= MAX_IMPORT_BYTES,
            "That video is larger than 4 GB."
        );
        let ext = canonical
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp4")
            .to_ascii_lowercase();
        let file_name = format!("{}.{ext}", slugify(&name));
        fs::create_dir_all(&staged)?;
        fs::copy(&canonical, staged.join(&file_name))?;
        let project = serde_json::json!({ "type": "video", "file": file_name, "title": name });
        fs::write(
            staged.join("project.json"),
            serde_json::to_vec_pretty(&project)?,
        )?;
    } else {
        bail!("Choose a folder or a video file.");
    }

    let dest = install(&staged, &root, &folder)?;
    finish(&dest, &folder)
}

/// Lists the imported items under `local_root` (ids `local:<folder>`).
pub fn scan_local_root(local_root: &Path) -> Vec<DiscoveredWorkshopWallpaper> {
    let Ok(entries) = fs::read_dir(local_root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let folder = e.file_name().to_string_lossy().into_owned();
            if folder.starts_with('.') {
                return None;
            }
            finish(&e.path(), &folder).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "truongpaper-import-{tag}-{}-{}",
            std::process::id(),
            fnv1a(tag)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn imports_a_project_folder_by_copy() {
        let base = tmp("folder");
        let src = base.join("My Scene");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(
            src.join("project.json"),
            r#"{"title":"Neon","type":"scene"}"#,
        )
        .unwrap();
        fs::write(src.join("sub/a.bin"), [1u8; 8]).unwrap();
        let root = base.join("local");

        let a = import_local(&src, &root).unwrap();
        assert!(a.wallpaper_id.starts_with("local:my-scene-"));
        assert_eq!(a.title, "Neon");
        assert!(Path::new(&a.install_dir).join("sub/a.bin").is_file());
        assert!(src.join("project.json").is_file(), "source must remain");

        // Same source -> same id, replaced in place.
        let b = import_local(&src, &root).unwrap();
        assert_eq!(a.wallpaper_id, b.wallpaper_id);
        assert_eq!(scan_local_root(&root).len(), 1);
        assert_eq!(scan_local_root(&root)[0].wallpaper_id, a.wallpaper_id);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn imports_a_video_with_generated_project() {
        let base = tmp("video");
        let video = base.join("Clip One.MP4");
        fs::write(&video, [0u8; 16]).unwrap();
        let root = base.join("local");
        let item = import_local(&video, &root).unwrap();
        assert_eq!(item.r#type, "video");
        assert_eq!(item.title, "Clip One");
        let project: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(Path::new(&item.install_dir).join("project.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(project["file"], "clip-one.mp4");
        assert!(Path::new(&item.install_dir).join("clip-one.mp4").is_file());
        assert!(video.is_file());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_bad_sources() {
        let base = tmp("bad");
        let root = base.join("local");
        let no_project = base.join("empty");
        fs::create_dir_all(&no_project).unwrap();
        assert!(import_local(&no_project, &root).is_err());
        let text = base.join("notes.txt");
        fs::write(&text, "x").unwrap();
        assert!(import_local(&text, &root).is_err());
        let empty_video = base.join("e.mp4");
        fs::write(&empty_video, "").unwrap();
        assert!(import_local(&empty_video, &root).is_err());
        assert!(import_local(&base.join("missing"), &root).is_err());
        // Importing the library folder itself (or something inside it) is refused.
        fs::create_dir_all(&root).unwrap();
        assert!(import_local(&root, &root).is_err());
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        let base = tmp("symlink");
        let root = base.join("local");
        let src = base.join("item");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("project.json"), r#"{"title":"x"}"#).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", src.join("link")).unwrap();
        assert!(import_local(&src, &root).is_err());
        let link = base.join("v.mp4");
        fs::write(base.join("real.mp4"), [0u8; 4]).unwrap();
        std::os::unix::fs::symlink(base.join("real.mp4"), &link).unwrap();
        assert!(import_local(&link, &root).is_err());
        assert!(scan_local_root(&root).is_empty());
        let _ = fs::remove_dir_all(&base);
    }
}
