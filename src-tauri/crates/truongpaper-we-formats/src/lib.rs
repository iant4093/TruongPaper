//! `truongpaper-we-formats`: Wallpaper Engine binary format parsers (`.pkg` PKGV, `.tex` TEXV/TEXI/TEXB/TEXS,
//! `.mdl` MDLV/MDLS/MDLA), 4-layer Virtual File System (`WeVirtualFileSystem`), and `scene.json` schema.
//! Complete specification: `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` (Sections 2, 3, 5.4, 8).

pub mod binary;
pub mod mdl;
pub mod scene_schema;
pub mod vfs;

pub use binary::{
    decode_tex_mipmap_to_rgba8, npot_uv_scale, parse_pkg_index, parse_tex, tex_flags, ParsedTex,
    PkgArchive, PkgEntry, TexFormat, TexFrameInfo, TexHeader, TexMipmap, WeBinaryExt, FIF_MP4,
    FIF_UNKNOWN,
};
pub use mdl::{
    parse_mdl, MdlPuppetVertex, MdlSection, ParsedMdlMesh, MDL_TRIANGLE_BYTES, MDL_VERTEX_STRIDE,
};
pub use scene_schema::{
    parse_scene_json, parse_user_properties, parse_vec2_or, parse_vec3_or, parse_vec4_or,
    parse_vec_components, resolve_bound_bool, resolve_bound_value, EffectBindSlot, EffectFboSpec,
    EffectFile, EffectPassSpec, MaterialFile, MaterialPassSpec, ModelJsonFile,
    OrthogonalProjection, SceneCamera, SceneEffectRef, SceneFile, SceneGeneral, SceneObject,
    ScenePassOverride, UserBoundValue, UserConditionSpec, UserPropertyMap,
};
pub use vfs::{MappedPkg, WallpaperVfs, WeVirtualFileSystem};

use std::io::{self, Read};

/// Reads an `i32` length-prefixed UTF-8 string used in `.pkg` (`PKGV0001..PKGV0023`) headers.
pub fn read_string_i32_size<R: Read>(r: &mut R) -> io::Result<String> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = i32::from_le_bytes(len_bytes);
    if !(0..=65_536).contains(&len) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid .pkg string length: {len}"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Reads a null-terminated C-string (up to `max_bytes`) used in `.tex` (`TEXV`, `TEXI`, `TEXB`, `TEXS`) headers.
pub fn read_n_string<R: Read>(r: &mut R, max_bytes: usize) -> io::Result<String> {
    let mut bytes = Vec::with_capacity(16);
    let mut byte = [0u8; 1];
    for _ in 0..max_bytes {
        r.read_exact(&mut byte)?;
        if byte[0] == 0 {
            break;
        }
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_pkg_and_tex_strings() {
        let mut pkg_cursor = Cursor::new(vec![
            8, 0, 0, 0, b'P', b'K', b'G', b'V', b'0', b'0', b'0', b'2',
        ]);
        assert_eq!(read_string_i32_size(&mut pkg_cursor).unwrap(), "PKGV0002");

        let mut tex_cursor = Cursor::new(b"TEXV0005\0".to_vec());
        assert_eq!(read_n_string(&mut tex_cursor, 16).unwrap(), "TEXV0005");
    }
}
