//! 4-layer Virtual File System (`WeVirtualFileSystem`, spec section 2.1).
//!
//! Resolution order: (1) loose `<wallpaper_dir>/<path>`, (2) `scene.pkg`, (3) `gifscene.pkg`,
//! (4) `<core_assets_dir>/<path>` (`we_core_assets/assets/`) then built-in virtual files.

use crate::binary::{parse_pkg_index, PkgArchive};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Error, ErrorKind, Read, Result as IoResult, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

/// A memory-mapped `.pkg` plus its parsed index. Entry reads are zero-copy slices of the mapping.
pub struct MappedPkg {
    pub mmap: Mmap,
    pub index: PkgArchive,
}

impl MappedPkg {
    pub fn open(path: &Path) -> IoResult<Self> {
        let file = File::open(path)?;
        // SAFETY: the mapping is read-only and wallpaper packages are not modified while mounted.
        let mmap = unsafe { Mmap::map(&file)? };
        let mut cursor = Cursor::new(&mmap[..]);
        let index = parse_pkg_index(&mut cursor)?;
        Ok(Self { mmap, index })
    }

    /// Zero-copy slice of the entry body. `virtual_path` must already be `/`-separated; lookup is
    /// case-insensitive.
    pub fn read_entry(&self, virtual_path: &str) -> Option<&[u8]> {
        let entry = self.index.entries.get(&virtual_path.to_lowercase())?;
        let end = entry.offset.checked_add(entry.length)?;
        self.mmap.get(entry.offset..end)
    }
}

pub struct WeVirtualFileSystem {
    pub wallpaper_dir: PathBuf,
    pub scene_pkg: Option<MappedPkg>,
    pub gifscene_pkg: Option<MappedPkg>,
    pub core_assets_dir: PathBuf,
    pub builtin_virtual_files: HashMap<&'static str, &'static [u8]>,
    generated_video: Option<(String, PathBuf)>,
}

/// Alias used by the spec text.
pub type WallpaperVfs = WeVirtualFileSystem;

pub enum VfsAsset<'a> {
    File(File, usize),
    Bytes(&'a [u8]),
}

impl VfsAsset<'_> {
    pub fn size(&self) -> usize {
        match self {
            Self::File(_, size) => *size,
            Self::Bytes(bytes) => bytes.len(),
        }
    }

    pub fn read(mut self, range: Range<usize>) -> IoResult<Vec<u8>> {
        if range.start > range.end || range.end > self.size() {
            return Err(Error::new(ErrorKind::InvalidInput, "Invalid asset range"));
        }
        match &mut self {
            Self::File(file, _) => {
                file.seek(SeekFrom::Start(range.start as u64))?;
                let mut bytes = Vec::new();
                file.take(range.len() as u64).read_to_end(&mut bytes)?;
                if bytes.len() != range.len() {
                    return Err(Error::new(ErrorKind::UnexpectedEof, "Asset was truncated"));
                }
                Ok(bytes)
            }
            Self::Bytes(bytes) => Ok(bytes[range].to_vec()),
        }
    }
}

fn open_file_asset(path: &Path) -> IoResult<VfsAsset<'static>> {
    let file = File::open(path)?;
    let size = usize::try_from(file.metadata()?.len())
        .map_err(|err| Error::new(ErrorKind::InvalidData, err))?;
    Ok(VfsAsset::File(file, size))
}

impl WeVirtualFileSystem {
    pub fn mount(wallpaper_dir: PathBuf, core_assets_dir: PathBuf) -> Self {
        let scene_pkg = MappedPkg::open(&wallpaper_dir.join("scene.pkg")).ok();
        let gifscene_pkg = MappedPkg::open(&wallpaper_dir.join("gifscene.pkg")).ok();

        let mut builtin_virtual_files: HashMap<&'static str, &'static [u8]> = HashMap::new();
        builtin_virtual_files.insert(
            "shaders/commands/copy.vert",
            b"attribute vec3 a_Position;\nattribute vec2 a_TexCoord;\nvarying vec2 v_TexCoord;\nvoid main() {\n    gl_Position = vec4(a_Position, 1.0);\n    v_TexCoord = a_TexCoord;\n}\n",
        );
        builtin_virtual_files.insert(
            "shaders/commands/copy.frag",
            b"varying vec2 v_TexCoord;\nuniform sampler2D g_Texture0;\nvoid main() {\n    gl_FragColor = texSample2D(g_Texture0, v_TexCoord);\n}\n",
        );
        builtin_virtual_files.insert(
            "__video_host__.html",
            include_bytes!("../../truongpaper-platform/src/video-wallpaper-host.html"),
        );

        Self {
            wallpaper_dir,
            scene_pkg,
            gifscene_pkg,
            core_assets_dir,
            builtin_virtual_files,
            generated_video: None,
        }
    }

    /// Normalizes a Wallpaper Engine virtual path and rejects any `..` traversal component.
    pub fn sanitize_virtual_path(raw: &str) -> IoResult<String> {
        let normalized = raw
            .trim()
            .trim_start_matches(['/', '\\'])
            .replace('\\', "/");
        for comp in Path::new(&normalized).components() {
            if matches!(
                comp,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            ) {
                return Err(Error::new(
                    ErrorKind::PermissionDenied,
                    format!("Rejected path traversal in VFS: {raw}"),
                ));
            }
        }
        Ok(normalized)
    }

    /// Resolves a virtual path across all 4 VFS layers in strict priority order.
    pub fn read_bytes(&self, virtual_path: &str) -> IoResult<Vec<u8>> {
        let asset = self.open_asset(virtual_path)?;
        let size = asset.size();
        asset.read(0..size)
    }

    /// Resolves the asset without loading a loose file into memory.
    pub fn open_asset(&self, virtual_path: &str) -> IoResult<VfsAsset<'_>> {
        let rel = Self::sanitize_virtual_path(virtual_path)?;

        if let Some((name, path)) = &self.generated_video {
            if *name == rel {
                return open_file_asset(path);
            }
        }

        // Layer 1: loose file in the wallpaper directory.
        let loose_path = self.wallpaper_dir.join(&rel);
        if loose_path.is_file() {
            return open_file_asset(&loose_path);
        }

        // Layer 2: scene.pkg.
        if let Some(pkg) = &self.scene_pkg {
            if let Some(slice) = pkg.read_entry(&rel) {
                return Ok(VfsAsset::Bytes(slice));
            }
        }

        // Layer 3: gifscene.pkg.
        if let Some(pkg) = &self.gifscene_pkg {
            if let Some(slice) = pkg.read_entry(&rel) {
                return Ok(VfsAsset::Bytes(slice));
            }
        }

        // Layer 4a: core Wallpaper Engine assets directory.
        let core_path = self.core_assets_dir.join(&rel);
        if core_path.is_file() {
            return open_file_asset(&core_path);
        }

        // Layer 4b: built-in in-memory virtual files.
        if let Some(bytes) = self.builtin_virtual_files.get(rel.as_str()) {
            return Ok(VfsAsset::Bytes(bytes));
        }

        Err(Error::new(
            ErrorKind::NotFound,
            format!("VFS file not found across all 4 layers: {rel}"),
        ))
    }

    /// Adds the one generated playback file associated with this wallpaper's mount.
    pub fn mount_generated_video(&mut self, name: String, path: PathBuf) {
        self.generated_video = Some((name, path));
    }

    pub fn read_string(&self, virtual_path: &str) -> IoResult<String> {
        let bytes = self.read_bytes(virtual_path)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// `true` when `virtual_path` resolves in any layer.
    pub fn exists(&self, virtual_path: &str) -> bool {
        self.read_bytes(virtual_path).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn build_pkg(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        let magic = b"PKGV0019";
        b.extend((magic.len() as i32).to_le_bytes());
        b.extend_from_slice(magic);
        b.extend((entries.len() as i32).to_le_bytes());
        let mut off = 0i32;
        for (name, data) in entries {
            b.extend((name.len() as i32).to_le_bytes());
            b.extend_from_slice(name.as_bytes());
            b.extend(off.to_le_bytes());
            b.extend((data.len() as i32).to_le_bytes());
            off += data.len() as i32;
        }
        for (_, data) in entries {
            b.extend_from_slice(data);
        }
        b
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "truongpaper-vfs-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn four_layers_resolve_in_priority_order() {
        let tmp = TempDir::new("layers");
        let wall = tmp.0.join("wall");
        let core = tmp.0.join("core");
        fs::create_dir_all(&wall).unwrap();
        fs::create_dir_all(core.join("shaders")).unwrap();

        fs::write(wall.join("a.txt"), b"loose").unwrap();
        fs::write(
            wall.join("scene.pkg"),
            build_pkg(&[
                ("a.txt", b"scene"),
                ("b.txt", b"scene-b"),
                ("Materials/Bg.tex", b"tex"),
            ]),
        )
        .unwrap();
        fs::write(
            wall.join("gifscene.pkg"),
            build_pkg(&[("b.txt", b"gif-b"), ("c.txt", b"gif-c")]),
        )
        .unwrap();
        fs::write(core.join("shaders").join("common.h"), b"core-common").unwrap();
        fs::write(core.join("c.txt"), b"core-c").unwrap();
        fs::write(core.join("d.txt"), b"core-d").unwrap();

        let vfs = WeVirtualFileSystem::mount(wall, core);
        assert!(vfs.scene_pkg.is_some() && vfs.gifscene_pkg.is_some());

        assert_eq!(vfs.read_bytes("a.txt").unwrap(), b"loose"); // L1 beats L2
        assert_eq!(vfs.read_bytes("b.txt").unwrap(), b"scene-b"); // L2 beats L3
        assert_eq!(vfs.read_bytes("c.txt").unwrap(), b"gif-c"); // L3 beats L4
        assert_eq!(vfs.read_bytes("d.txt").unwrap(), b"core-d"); // L4
        assert_eq!(vfs.read_string("shaders\\common.h").unwrap(), "core-common");
        assert_eq!(vfs.read_bytes("/materials/bg.tex").unwrap(), b"tex"); // case-insensitive pkg
        assert!(vfs
            .read_string("shaders/commands/copy.frag")
            .unwrap()
            .contains("texSample2D"));
        assert!(vfs.exists("__video_host__.html"));
        assert_eq!(
            vfs.read_bytes("nope.bin").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(WeVirtualFileSystem::sanitize_virtual_path("../secret").is_err());
        assert!(WeVirtualFileSystem::sanitize_virtual_path("a/../../b").is_err());
        assert_eq!(
            WeVirtualFileSystem::sanitize_virtual_path("\\shaders\\x.frag").unwrap(),
            "shaders/x.frag"
        );
    }

    #[test]
    fn missing_packages_are_skipped() {
        let tmp = TempDir::new("nopkg");
        let vfs = WeVirtualFileSystem::mount(tmp.0.clone(), tmp.0.join("core"));
        assert!(vfs.scene_pkg.is_none() && vfs.gifscene_pkg.is_none());
        assert!(vfs.exists("shaders/commands/copy.vert"));
    }

    #[test]
    fn mapped_pkg_slice_is_bounds_checked() {
        let tmp = TempDir::new("oob");
        let mut bytes = build_pkg(&[("x.bin", b"abcd")]);
        bytes.truncate(bytes.len() - 2); // entry now extends past EOF
        let path = tmp.0.join("scene.pkg");
        fs::write(&path, bytes).unwrap();
        let pkg = MappedPkg::open(&path).unwrap();
        assert!(pkg.read_entry("x.bin").is_none());
    }
}
