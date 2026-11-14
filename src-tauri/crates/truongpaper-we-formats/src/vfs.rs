//! 4-layer Virtual File System (`WeVirtualFileSystem`, spec section 2.1).
//!
//! Resolution order: (1) loose `<wallpaper_dir>/<path>`, (2) `scene.pkg`, (3) `gifscene.pkg`,
//! (4) `<core_assets_dir>/<path>` (`we_core_assets/assets/`), then the app's own bundled
//! clean-room core assets (`resources/core-assets/assets/`, see [`set_bundled_assets_dir`]),
//! then built-in virtual files.

use crate::binary::{parse_pkg_index, PkgArchive};
use memmap2::Mmap;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Error, ErrorKind, Read, Result as IoResult, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

static BUNDLED_ASSETS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Registers the app's bundled core-assets tree (our own implementations of the stock effects and
/// shader headers). Every [`WeVirtualFileSystem`] mounted afterwards falls back to it when a file is
/// in neither the wallpaper nor the real Wallpaper Engine core assets. First call wins.
pub fn set_bundled_assets_dir(dir: PathBuf) {
    let _ = BUNDLED_ASSETS_DIR.set(dir);
}

/// The directory registered with [`set_bundled_assets_dir`], if any.
pub fn bundled_assets_dir() -> Option<&'static Path> {
    BUNDLED_ASSETS_DIR.get().map(PathBuf::as_path)
}

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
    /// Our own bundled stand-ins for stock effects and includes; consulted after `core_assets_dir`.
    pub bundled_assets_dir: Option<PathBuf>,
    pub builtin_virtual_files: HashMap<&'static str, &'static [u8]>,
    /// `(root, canonical root)` pairs computed at mount, so opens do not canonicalize the root.
    /// An entry is only used while its `root` still equals the current (public) field.
    canonical_roots: Vec<(PathBuf, PathBuf)>,
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

/// `true` when `meta` describes a symlink, or on Windows any reparse point (junction, mount point).
fn is_link(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// `true` when `root/rel` (or any directory on the way) is a link, so it needs the canonical check.
/// `Err` means a component does not exist or cannot be inspected.
fn path_has_link(root: &Path, rel: &str) -> IoResult<bool> {
    let mut cur = root.to_path_buf();
    for comp in Path::new(rel).components() {
        cur.push(comp);
        if is_link(&std::fs::symlink_metadata(&cur)?) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Opens `root/rel` when it is a regular file that stays inside `root`. `Ok(None)` means no such
/// file in this layer (including a path that cannot be canonicalized); a symlink or junction that
/// points outside `root` is `PermissionDenied`, so untrusted wallpaper folders cannot read
/// arbitrary local files.
///
/// Plain files (no link on the way, checked with `symlink_metadata`) are opened directly. Only
/// linked paths are canonicalized, against `canonical_root` cached at mount.
fn open_contained(
    root: &Path,
    canonical_root: Option<&Path>,
    rel: &str,
) -> IoResult<Option<VfsAsset<'static>>> {
    let candidate = root.join(rel);
    match path_has_link(root, rel) {
        Ok(false) => {
            return match open_file_asset(&candidate) {
                Ok(asset) if candidate.is_file() => Ok(Some(asset)),
                _ => Ok(None),
            };
        }
        Ok(true) => {}
        Err(_) => return Ok(None),
    }
    if !candidate.is_file() {
        return Ok(None);
    }
    let Ok(real) = candidate.canonicalize() else {
        return Ok(None);
    };
    let real_root = match canonical_root {
        Some(r) => r.to_path_buf(),
        None => match root.canonicalize() {
            Ok(r) => r,
            Err(_) => return Ok(None),
        },
    };
    if !real.starts_with(&real_root) {
        return Err(Error::new(
            ErrorKind::PermissionDenied,
            format!("Rejected link escaping the wallpaper folder: {rel}"),
        ));
    }
    open_file_asset(&real).map(Some)
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
        builtin_virtual_files.insert(
            "shaders/common.h",
            include_bytes!("shader_fallbacks/common.glsl"),
        );
        builtin_virtual_files.insert(
            "shaders/common_blending.h",
            include_bytes!("shader_fallbacks/blending.glsl"),
        );
        builtin_virtual_files.insert(
            "shaders/common_perspective.h",
            include_bytes!("shader_fallbacks/perspective.glsl"),
        );

        let mut vfs = Self {
            wallpaper_dir,
            scene_pkg,
            gifscene_pkg,
            core_assets_dir,
            bundled_assets_dir: bundled_assets_dir().map(Path::to_path_buf),
            builtin_virtual_files,
            canonical_roots: Vec::new(),
        };
        vfs.refresh_canonical_roots();
        vfs
    }

    fn refresh_canonical_roots(&mut self) {
        let roots = [
            Some(self.wallpaper_dir.clone()),
            Some(self.core_assets_dir.clone()),
            self.bundled_assets_dir.clone(),
        ];
        self.canonical_roots = roots
            .into_iter()
            .flatten()
            .filter_map(|r| r.canonicalize().ok().map(|c| (r, c)))
            .collect();
    }

    fn canonical_root(&self, root: &Path) -> Option<&Path> {
        self.canonical_roots
            .iter()
            .find(|(r, _)| r == root)
            .map(|(_, c)| c.as_path())
    }

    /// Overrides the bundled-assets layer (tests and tools that do not use the global).
    pub fn with_bundled_assets_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.bundled_assets_dir = dir;
        self.refresh_canonical_roots();
        self
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

        // Layer 1: loose file in the wallpaper directory.
        if let Some(asset) = open_contained(
            &self.wallpaper_dir,
            self.canonical_root(&self.wallpaper_dir),
            &rel,
        )? {
            return Ok(asset);
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
        if let Some(asset) = open_contained(
            &self.core_assets_dir,
            self.canonical_root(&self.core_assets_dir),
            &rel,
        )? {
            return Ok(asset);
        }

        // Layer 4b: bundled clean-room core assets.
        if let Some(bundled) = &self.bundled_assets_dir {
            if let Some(asset) = open_contained(bundled, self.canonical_root(bundled), &rel)? {
                return Ok(asset);
            }
        }

        // Layer 4c: built-in in-memory virtual files.
        if let Some(bytes) = self.builtin_virtual_files.get(rel.as_str()) {
            return Ok(VfsAsset::Bytes(bytes));
        }

        Err(Error::new(
            ErrorKind::NotFound,
            format!("VFS file not found across all 4 layers: {rel}"),
        ))
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
    fn bundled_assets_sit_between_core_assets_and_builtins() {
        let tmp = TempDir::new("bundled");
        let (wall, core, bundled) = (
            tmp.0.join("wall"),
            tmp.0.join("core"),
            tmp.0.join("bundled"),
        );
        for d in [&wall, &core, &bundled] {
            fs::create_dir_all(d.join("shaders")).unwrap();
        }
        fs::write(wall.join("w.txt"), b"wall").unwrap();
        fs::write(bundled.join("w.txt"), b"bundled-w").unwrap();
        fs::write(core.join("shaders/common_blur.h"), b"real").unwrap();
        fs::write(bundled.join("shaders/common_blur.h"), b"ours").unwrap();
        fs::write(bundled.join("shaders/common.h"), b"ours-common").unwrap();
        let vfs = WeVirtualFileSystem::mount(wall, core).with_bundled_assets_dir(Some(bundled));
        assert_eq!(vfs.read_bytes("w.txt").unwrap(), b"wall");
        assert_eq!(vfs.read_bytes("shaders/common_blur.h").unwrap(), b"real");
        assert_eq!(vfs.read_bytes("shaders/common.h").unwrap(), b"ours-common");
        // beats built-in
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

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_wallpaper_dir_is_denied() {
        let tmp = TempDir::new("symlink");
        let (wall, outside) = (tmp.0.join("wall"), tmp.0.join("outside"));
        fs::create_dir_all(wall.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        fs::write(wall.join("real.txt"), b"ok").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), wall.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, wall.join("sub/dir")).unwrap();
        std::os::unix::fs::symlink(wall.join("real.txt"), wall.join("inner.txt")).unwrap();

        let vfs = WeVirtualFileSystem::mount(wall.clone(), tmp.0.join("core"));
        assert_eq!(vfs.read_bytes("real.txt").unwrap(), b"ok");
        // A link that stays inside the folder is fine.
        assert_eq!(vfs.read_bytes("inner.txt").unwrap(), b"ok");
        for p in ["link.txt", "sub/dir/secret.txt"] {
            assert_eq!(
                vfs.read_bytes(p).unwrap_err().kind(),
                ErrorKind::PermissionDenied,
                "{p}"
            );
        }
        assert!(!vfs.exists("link.txt"));

        // The mounted folder itself may be reached through a symlink.
        let alias = tmp.0.join("alias");
        std::os::unix::fs::symlink(&wall, &alias).unwrap();
        let vfs = WeVirtualFileSystem::mount(alias, tmp.0.join("core"));
        assert_eq!(vfs.read_bytes("real.txt").unwrap(), b"ok");
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

    #[cfg(unix)]
    #[test]
    fn links_are_contained_and_plain_files_skip_canonicalize() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new("links");
        let wall = tmp.0.join("wall");
        let outside = tmp.0.join("outside");
        fs::create_dir_all(wall.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(wall.join("sub/a.txt"), b"plain").unwrap();
        fs::write(wall.join("real.txt"), b"real").unwrap();
        fs::write(outside.join("secret.txt"), b"secret").unwrap();
        symlink(outside.join("secret.txt"), wall.join("evil.txt")).unwrap();
        symlink(&outside, wall.join("evildir")).unwrap();
        symlink(wall.join("real.txt"), wall.join("inner.txt")).unwrap();

        let vfs = WeVirtualFileSystem::mount(wall, tmp.0.join("core"));
        assert_eq!(vfs.read_bytes("sub/a.txt").unwrap(), b"plain");
        assert_eq!(vfs.read_bytes("inner.txt").unwrap(), b"real");
        for p in ["evil.txt", "evildir/secret.txt"] {
            let err = vfs.read_bytes(p).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::PermissionDenied, "{p}");
        }
        assert_eq!(
            vfs.read_bytes("missing.txt").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_link_falls_through_to_scene_pkg() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new("dangling");
        let wall = tmp.0.join("wall");
        fs::create_dir_all(&wall).unwrap();
        fs::write(wall.join("scene.pkg"), build_pkg(&[("x.txt", b"from-pkg")])).unwrap();
        symlink(tmp.0.join("nowhere"), wall.join("x.txt")).unwrap();
        let vfs = WeVirtualFileSystem::mount(wall, tmp.0.join("core"));
        assert_eq!(vfs.read_bytes("x.txt").unwrap(), b"from-pkg");
    }
}
