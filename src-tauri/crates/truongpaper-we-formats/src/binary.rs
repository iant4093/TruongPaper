//! `.pkg` archive index reader and `.tex` texture decoder (spec section 3.1).
//!
//! **String encodings differ** (pitfall #3): `.pkg` uses `i32`-length-prefixed strings without a
//! terminator ([`WeBinaryExt::read_string_i32_size`]); `.tex` uses null-terminated C strings
//! ([`WeBinaryExt::read_n_string`]).

use std::collections::HashMap;
use std::io::{Cursor, Error, ErrorKind, Read, Result as IoResult, Seek};

fn invalid(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, msg.into())
}

/// Upper bound for a single decompressed mipmap (1 GiB). Rejects hostile `decompressed_bytes_count`.
const MAX_MIPMAP_BYTES: usize = 1 << 30;

pub trait WeBinaryExt: Read + Seek {
    fn read_u32_le(&mut self) -> IoResult<u32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn read_i32_le(&mut self) -> IoResult<i32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(i32::from_le_bytes(b))
    }

    fn read_f32_le(&mut self) -> IoResult<f32> {
        let mut b = [0u8; 4];
        self.read_exact(&mut b)?;
        Ok(f32::from_le_bytes(b))
    }

    /// Used by `.pkg` files: `[i32 length][utf8 bytes]` (NO null terminator).
    fn read_string_i32_size(&mut self, max_len: usize) -> IoResult<String> {
        let len = self.read_i32_le()?;
        if len < 0 || (len as usize) > max_len {
            return Err(invalid(format!("Invalid i32 string length: {len}")));
        }
        let mut buf = vec![0u8; len as usize];
        self.read_exact(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Used by `.tex` files: null-terminated ASCII/UTF-8 string (e.g. `"TEXV0005\0"`).
    /// Reads at most `max_len` bytes before the terminator; errors if no terminator was found.
    fn read_n_string(&mut self, max_len: usize) -> IoResult<String> {
        let mut buf = Vec::with_capacity(16);
        let mut byte = [0u8; 1];
        loop {
            self.read_exact(&mut byte)?;
            if byte[0] == 0 {
                break;
            }
            if buf.len() >= max_len {
                return Err(invalid(format!(
                    "Null-terminated string exceeds {max_len} bytes"
                )));
            }
            buf.push(byte[0]);
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

impl<T: Read + Seek> WeBinaryExt for T {}

// ============================================================================
// 1. PKG Archive Reader (PKGV0001 .. PKGV0022+)
// ============================================================================

#[derive(Debug, Clone)]
pub struct PkgEntry {
    pub full_path: String,
    /// Absolute byte offset inside the `.pkg` file.
    pub offset: usize,
    pub length: usize,
}

#[derive(Debug)]
pub struct PkgArchive {
    pub magic: String,
    /// Keyed by lower-cased, `/`-separated virtual path.
    pub entries: HashMap<String, PkgEntry>,
}

pub fn parse_pkg_index<R: Read + Seek>(reader: &mut R) -> IoResult<PkgArchive> {
    let magic = reader.read_string_i32_size(32)?;
    if !magic.starts_with("PKGV") {
        return Err(invalid(format!("Unexpected PKG magic: {magic}")));
    }

    let entry_count = reader.read_u32_le()? as usize;
    // Each entry needs at least 12 bytes; cap the pre-allocation so a hostile count cannot OOM.
    let mut raw_entries = Vec::with_capacity(entry_count.min(65_536));

    for _ in 0..entry_count {
        let full_path = reader.read_string_i32_size(512)?.replace('\\', "/");
        let rel_offset = reader.read_u32_le()? as usize;
        let length = reader.read_u32_le()? as usize;
        raw_entries.push((full_path, rel_offset, length));
    }

    let data_start = reader.stream_position()? as usize;
    let mut entries = HashMap::with_capacity(raw_entries.len());
    for (full_path, rel_offset, length) in raw_entries {
        let offset = data_start
            .checked_add(rel_offset)
            .ok_or_else(|| invalid("PKG entry offset overflow"))?;
        entries.insert(
            full_path.to_lowercase(),
            PkgEntry {
                full_path,
                offset,
                length,
            },
        );
    }

    Ok(PkgArchive { magic, entries })
}

// ============================================================================
// 2. TEX Texture Reader (TEXV0005 / TEXI0001 / TEXB0001..0004 / TEXS0001..0003)
// ============================================================================

/// `FIF_MP4` in Wallpaper Engine's extended FreeImage format enumeration.
pub const FIF_MP4: i32 = 35;
/// `FIF_UNKNOWN`: pixels are raw / block-compressed, not an embedded image container.
pub const FIF_UNKNOWN: i32 = -1;

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TexFormat {
    RGBA8888 = 0,
    DXT5 = 4,
    DXT3 = 6,
    DXT1 = 7,
    RG88 = 8,
    R8 = 9,
}

impl TexFormat {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::RGBA8888),
            4 => Some(Self::DXT5),
            6 => Some(Self::DXT3),
            7 => Some(Self::DXT1),
            8 => Some(Self::RG88),
            9 => Some(Self::R8),
            _ => None,
        }
    }
}

pub mod tex_flags {
    pub const NONE: i32 = 0;
    /// Use Nearest filter.
    pub const NO_INTERPOLATION: i32 = 1;
    /// Use ClampToEdge address mode.
    pub const CLAMP_UVS: i32 = 2;
    /// Has trailing TEXS frame info block.
    pub const IS_GIF: i32 = 4;
    pub const IS_VIDEO_TEXTURE: i32 = 32;
}

/// NPOT padding factor (pitfall #5): `image_dim / texture_dim`. Multiply sampled UVs by this so the
/// padded power-of-two border is never shown. Returns `1.0` for a zero texture dimension.
pub fn npot_uv_scale(image_dim: i32, texture_dim: i32) -> f32 {
    if texture_dim <= 0 {
        1.0
    } else {
        image_dim as f32 / texture_dim as f32
    }
}

#[derive(Debug, Clone)]
pub struct TexHeader {
    pub format: TexFormat,
    pub flags: i32,
    /// Padded power-of-two GPU width.
    pub texture_width: i32,
    /// Padded power-of-two GPU height.
    pub texture_height: i32,
    /// Actual visible content width (UV max_u = image_width / texture_width).
    pub image_width: i32,
    /// Actual visible content height (UV max_v = image_height / texture_height).
    pub image_height: i32,
    pub unk_int0: u32,
}

impl TexHeader {
    /// `[image_width / texture_width, image_height / texture_height]`.
    pub fn uv_scale(&self) -> [f32; 2] {
        [
            npot_uv_scale(self.image_width, self.texture_width),
            npot_uv_scale(self.image_height, self.texture_height),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct TexMipmap {
    pub width: i32,
    pub height: i32,
    pub is_lz4_compressed: bool,
    pub decompressed_bytes_count: i32,
    /// Decompressed via `lz4_flex` when `is_lz4_compressed == true`.
    pub raw_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TexFrameInfo {
    pub image_id: i32,
    pub frametime: f32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub width_y: f32,
    pub height_x: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct ParsedTex {
    pub header: TexHeader,
    /// FreeImageFormat: -1 = raw/DXT pixels, 0 = BMP, 2 = JPEG, 13 = PNG, 25 = GIF, 35 = MP4 video.
    pub free_image_format: i32,
    /// Effective TEXB container layout used for the mipmaps (1..=4, after the TEXB0004 downgrade).
    pub container_version: u32,
    pub images: Vec<Vec<TexMipmap>>,
    /// `(gif_width, gif_height, frames)`.
    pub sprite_frames: Option<(i32, i32, Vec<TexFrameInfo>)>,
}

impl ParsedTex {
    pub fn uv_scale(&self) -> [f32; 2] {
        self.header.uv_scale()
    }

    pub fn is_mp4(&self) -> bool {
        self.free_image_format == FIF_MP4
    }
}

/// Resolves the TEXB container magic. Returns `(effective_version, free_image_format)`.
///
/// RePKG rule (pitfall #4): `TEXB0004` whose resolved format is not `FIF_MP4` (35) uses the
/// **Version 3** mipmap layout.
fn read_container<R: Read + Seek>(r: &mut R, magic: &str) -> IoResult<(u32, i32)> {
    match magic {
        "TEXB0001" => Ok((1, FIF_UNKNOWN)),
        "TEXB0002" => Ok((2, FIF_UNKNOWN)),
        "TEXB0003" => Ok((3, r.read_i32_le()?)),
        "TEXB0004" => {
            let fmt = r.read_i32_le()?;
            let is_video_mp4 = r.read_i32_le()? == 1;
            let fif = if fmt == FIF_UNKNOWN && is_video_mp4 {
                FIF_MP4
            } else {
                fmt
            };
            Ok((if fif == FIF_MP4 { 4 } else { 3 }, fif))
        }
        other => Err(invalid(format!("Unknown TEXB magic: {other}"))),
    }
}

fn remaining(r: &Cursor<&[u8]>) -> usize {
    r.get_ref().len().saturating_sub(r.position() as usize)
}

fn read_count(r: &mut Cursor<&[u8]>, what: &str) -> IoResult<usize> {
    let n = r.read_i32_le()?;
    if n < 0 {
        return Err(invalid(format!("Negative {what} count: {n}")));
    }
    Ok(n as usize)
}

pub fn parse_tex(bytes: &[u8]) -> IoResult<ParsedTex> {
    let mut r = Cursor::new(bytes);

    let magic1 = r.read_n_string(16)?;
    if magic1 != "TEXV0005" {
        return Err(invalid(format!("Invalid TEX magic1: {magic1}")));
    }

    let magic2 = r.read_n_string(16)?;
    if magic2 != "TEXI0001" {
        return Err(invalid(format!("Invalid TEX magic2: {magic2}")));
    }

    let format_raw = r.read_i32_le()?;
    let format = TexFormat::from_i32(format_raw)
        .ok_or_else(|| invalid(format!("Unknown TexFormat: {format_raw}")))?;

    let header = TexHeader {
        format,
        flags: r.read_i32_le()?,
        texture_width: r.read_i32_le()?,
        texture_height: r.read_i32_le()?,
        image_width: r.read_i32_le()?,
        image_height: r.read_i32_le()?,
        unk_int0: r.read_u32_le()?,
    };

    let container_magic = r.read_n_string(16)?;
    let image_count = read_count(&mut r, "image")?;
    let (container_version, free_image_format) = read_container(&mut r, &container_magic)?;

    let mut images = Vec::with_capacity(image_count.min(1024));
    for _ in 0..image_count {
        let mipmap_count = read_count(&mut r, "mipmap")?;
        let mut mipmaps = Vec::with_capacity(mipmap_count.min(64));

        for _ in 0..mipmap_count {
            if container_version == 4 {
                let _param1 = r.read_i32_le()?; // == 1
                let _param2 = r.read_i32_le()?; // == 2
                let _condition_json = r.read_n_string(4096)?;
                let _param3 = r.read_i32_le()?; // == 1
            }

            let width = r.read_i32_le()?;
            let height = r.read_i32_le()?;
            let (is_lz4, decompressed_len) = if container_version >= 2 {
                (r.read_i32_le()? == 1, r.read_i32_le()?)
            } else {
                (false, 0)
            };

            let byte_count = read_count(&mut r, "mipmap byte")?;
            if byte_count > remaining(&r) {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    format!("Mipmap payload of {byte_count} bytes exceeds file size"),
                ));
            }
            let mut payload = vec![0u8; byte_count];
            r.read_exact(&mut payload)?;

            let raw_bytes = if is_lz4 {
                if decompressed_len < 0 || decompressed_len as usize > MAX_MIPMAP_BYTES {
                    return Err(invalid(format!(
                        "Invalid LZ4 decompressed size: {decompressed_len}"
                    )));
                }
                lz4_flex::block::decompress(&payload, decompressed_len as usize)
                    .map_err(|e| invalid(e.to_string()))?
            } else {
                payload
            };

            mipmaps.push(TexMipmap {
                width,
                height,
                is_lz4_compressed: is_lz4,
                decompressed_bytes_count: decompressed_len,
                raw_bytes,
            });
        }
        images.push(mipmaps);
    }

    // Animated sprite sheet frames when IsGif flag (4) is set.
    let sprite_frames = if (header.flags & tex_flags::IS_GIF) != 0 {
        let texs_magic = r.read_n_string(16)?;
        if !matches!(texs_magic.as_str(), "TEXS0001" | "TEXS0002" | "TEXS0003") {
            return Err(invalid(format!("Unknown TEXS magic: {texs_magic}")));
        }
        let frame_count = read_count(&mut r, "frame")?;
        let (mut gif_w, mut gif_h) = if texs_magic == "TEXS0003" {
            (r.read_i32_le()?, r.read_i32_le()?)
        } else {
            (0, 0)
        };

        let mut frames = Vec::with_capacity(frame_count.min(65_536));
        for _ in 0..frame_count {
            let f = if texs_magic == "TEXS0001" {
                TexFrameInfo {
                    image_id: r.read_i32_le()?,
                    frametime: r.read_f32_le()?,
                    x: r.read_i32_le()? as f32,
                    y: r.read_i32_le()? as f32,
                    width: r.read_i32_le()? as f32,
                    width_y: r.read_i32_le()? as f32,
                    height_x: r.read_i32_le()? as f32,
                    height: r.read_i32_le()? as f32,
                }
            } else {
                TexFrameInfo {
                    image_id: r.read_i32_le()?,
                    frametime: r.read_f32_le()?,
                    x: r.read_f32_le()?,
                    y: r.read_f32_le()?,
                    width: r.read_f32_le()?,
                    width_y: r.read_f32_le()?,
                    height_x: r.read_f32_le()?,
                    height: r.read_f32_le()?,
                }
            };
            frames.push(f);
        }
        if (gif_w == 0 || gif_h == 0) && !frames.is_empty() {
            gif_w = frames[0].width as i32;
            gif_h = frames[0].height as i32;
        }
        Some((gif_w, gif_h, frames))
    } else {
        None
    };

    Ok(ParsedTex {
        header,
        free_image_format,
        container_version,
        images,
        sprite_frames,
    })
}

/// Decodes any `TexMipmap` (`DXT1`, `DXT3`, `DXT5`, `RG88`, `R8`, `RGBA8888`, or embedded
/// FreeImage PNG/JPEG/GIF) into tightly packed `width * height * 4` `RGBA8888` bytes ready for
/// `wgpu::Queue::write_texture`. This avoids `wgpu` BC1/BC2/BC3 4x4 block-alignment panics on
/// odd-sized textures (`width % 4 != 0 || height % 4 != 0`).
///
/// For an embedded image container the returned buffer uses the decoded image's own dimensions,
/// which may differ from `mipmap.width/height`.
pub fn decode_tex_mipmap_to_rgba8(
    format: TexFormat,
    free_image_format: i32,
    mipmap: &TexMipmap,
) -> IoResult<Vec<u8>> {
    if free_image_format == FIF_MP4 {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "FIF_MP4 textures carry video and cannot be decoded to RGBA8",
        ));
    }

    // 1. Embedded standard image container (PNG / JPEG / BMP / GIF / WEBP).
    if free_image_format != FIF_UNKNOWN {
        let img = image::load_from_memory(&mipmap.raw_bytes)
            .map_err(|e| invalid(format!("FreeImage decode failed: {e}")))?;
        return Ok(img.to_rgba8().into_raw());
    }

    // 2. Native Wallpaper Engine pixel & block-compressed formats.
    if mipmap.width < 0 || mipmap.height < 0 {
        return Err(invalid("Negative mipmap dimensions"));
    }
    let w = (mipmap.width as usize).max(1);
    let h = (mipmap.height as usize).max(1);
    let pixels = w
        .checked_mul(h)
        .filter(|p| p.saturating_mul(4) <= MAX_MIPMAP_BYTES)
        .ok_or_else(|| invalid("Mipmap dimensions too large"))?;
    let mut out = vec![0u8; pixels * 4];
    let src = &mipmap.raw_bytes;
    match format {
        TexFormat::RGBA8888 => {
            let expected = pixels * 4;
            if src.len() < expected {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "Truncated RGBA8888 mipmap",
                ));
            }
            out.copy_from_slice(&src[..expected]);
        }
        TexFormat::R8 => {
            if src.len() < pixels {
                return Err(Error::new(ErrorKind::UnexpectedEof, "Truncated R8 mipmap"));
            }
            for (i, &r) in src[..pixels].iter().enumerate() {
                out[i * 4..i * 4 + 4].copy_from_slice(&[r, r, r, r]);
            }
        }
        TexFormat::RG88 => {
            if src.len() < pixels * 2 {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "Truncated RG88 mipmap",
                ));
            }
            for i in 0..pixels {
                let r = src[i * 2];
                let g = src[i * 2 + 1];
                out[i * 4..i * 4 + 4].copy_from_slice(&[r, r, r, g]);
            }
        }
        TexFormat::DXT1 => {
            decode_bc_blocks(src, w, h, 8, &mut out, |b, px| {
                decode_dxt1_block(b, px, true)
            })?;
        }
        TexFormat::DXT3 => {
            decode_bc_blocks(src, w, h, 16, &mut out, decode_dxt3_block)?;
        }
        TexFormat::DXT5 => {
            decode_bc_blocks(src, w, h, 16, &mut out, decode_dxt5_block)?;
        }
    }

    Ok(out)
}

fn decode_bc_blocks<F>(
    src: &[u8],
    width: usize,
    height: usize,
    block_bytes: usize,
    out_rgba: &mut [u8],
    decode_block: F,
) -> IoResult<()>
where
    F: Fn(&[u8], &mut [[u8; 4]; 16]),
{
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let mut offset = 0usize;
    let mut pixels = [[0u8; 4]; 16];

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            if offset + block_bytes > src.len() {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "Truncated DXT/BC mipmap",
                ));
            }
            decode_block(&src[offset..offset + block_bytes], &mut pixels);
            offset += block_bytes;

            for py in 0..4 {
                let y = by * 4 + py;
                if y >= height {
                    continue;
                }
                for px in 0..4 {
                    let x = bx * 4 + px;
                    if x >= width {
                        continue;
                    }
                    let dst_idx = (y * width + x) * 4;
                    out_rgba[dst_idx..dst_idx + 4].copy_from_slice(&pixels[py * 4 + px]);
                }
            }
        }
    }
    Ok(())
}

fn rgb565_to_rgb888(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1F) as u8;
    let g = ((c >> 5) & 0x3F) as u8;
    let b = (c & 0x1F) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    ]
}

/// BC1 colour block. `allow_punch_through` is `true` for DXT1 (`c0 <= c1` selects the 3-colour +
/// transparent mode). BC2/BC3 colour blocks always use the 4-colour mode, so they pass `false`.
fn decode_dxt1_block(block: &[u8], out: &mut [[u8; 4]; 16], allow_punch_through: bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    let rgb0 = rgb565_to_rgb888(c0);
    let rgb1 = rgb565_to_rgb888(c1);

    let mut palette = [[0u8; 4]; 4];
    palette[0] = [rgb0[0], rgb0[1], rgb0[2], 255];
    palette[1] = [rgb1[0], rgb1[1], rgb1[2], 255];
    if c0 > c1 || !allow_punch_through {
        for i in 0..3 {
            palette[2][i] = ((2 * rgb0[i] as u16 + rgb1[i] as u16) / 3) as u8;
            palette[3][i] = ((rgb0[i] as u16 + 2 * rgb1[i] as u16) / 3) as u8;
        }
        palette[2][3] = 255;
        palette[3][3] = 255;
    } else {
        for i in 0..3 {
            palette[2][i] = ((rgb0[i] as u16 + rgb1[i] as u16) / 2) as u8;
        }
        palette[2][3] = 255;
        palette[3] = [0, 0, 0, 0];
    }

    for (i, px) in out.iter_mut().enumerate() {
        let idx = ((bits >> (i * 2)) & 0x03) as usize;
        *px = palette[idx];
    }
}

fn decode_dxt3_block(block: &[u8], out: &mut [[u8; 4]; 16]) {
    decode_dxt1_block(&block[8..16], out, false);
    let alpha_bits = u64::from_le_bytes([
        block[0], block[1], block[2], block[3], block[4], block[5], block[6], block[7],
    ]);
    for (i, px) in out.iter_mut().enumerate() {
        let a4 = ((alpha_bits >> (i * 4)) & 0x0F) as u8;
        px[3] = (a4 << 4) | a4;
    }
}

fn decode_dxt5_block(block: &[u8], out: &mut [[u8; 4]; 16]) {
    decode_dxt1_block(&block[8..16], out, false);
    let a0 = block[0];
    let a1 = block[1];
    let mut alphas = [0u8; 8];
    alphas[0] = a0;
    alphas[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            alphas[i + 1] = (((7 - i as u16) * a0 as u16 + (i as u16) * a1 as u16) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            alphas[i + 1] = (((5 - i as u16) * a0 as u16 + (i as u16) * a1 as u16) / 5) as u8;
        }
        alphas[6] = 0;
        alphas[7] = 255;
    }

    let mut alpha_indices: u64 = 0;
    for i in 0..6 {
        alpha_indices |= (block[2 + i] as u64) << (i * 8);
    }
    for (i, px) in out.iter_mut().enumerate() {
        let idx = ((alpha_indices >> (i * 3)) & 0x07) as usize;
        px[3] = alphas[idx];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- fixture helpers -------------------------------------------------------------------

    fn nstr(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    }

    fn i32le(v: i32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    fn f32le(v: f32) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }

    /// TEXV0005 + TEXI0001 + header fields (format RGBA8888, 512x256 padded, 300x200 image).
    fn tex_header(format: i32, flags: i32) -> Vec<u8> {
        let mut b = nstr("TEXV0005");
        b.extend(nstr("TEXI0001"));
        for v in [format, flags, 512, 256, 300, 200, 0] {
            b.extend(i32le(v));
        }
        b
    }

    fn mip_v1(w: i32, h: i32, data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(i32le(w));
        b.extend(i32le(h));
        b.extend(i32le(data.len() as i32));
        b.extend_from_slice(data);
        b
    }

    fn mip_v2(w: i32, h: i32, lz4: bool, dec_len: i32, data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(i32le(w));
        b.extend(i32le(h));
        b.extend(i32le(lz4 as i32));
        b.extend(i32le(dec_len));
        b.extend(i32le(data.len() as i32));
        b.extend_from_slice(data);
        b
    }

    // ---- pkg -------------------------------------------------------------------------------

    #[test]
    fn pkg_string_i32_size_vs_tex_null_string() {
        let mut c = Cursor::new(vec![
            8, 0, 0, 0, b'P', b'K', b'G', b'V', b'0', b'0', b'0', b'2',
        ]);
        assert_eq!(c.read_string_i32_size(32).unwrap(), "PKGV0002");
        assert_eq!(c.position(), 12);

        let mut t = Cursor::new(b"TEXV0005\0".to_vec());
        assert_eq!(t.read_n_string(16).unwrap(), "TEXV0005");
        assert_eq!(t.position(), 9);

        // Over-long and negative lengths are rejected.
        let mut bad = Cursor::new(vec![0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(bad.read_string_i32_size(32).is_err());
        let mut long = Cursor::new(vec![64, 0, 0, 0]);
        assert!(long.read_string_i32_size(32).is_err());
        // Unterminated null string errors instead of silently truncating.
        let mut unterminated = Cursor::new(vec![b'A'; 40]);
        assert!(unterminated.read_n_string(16).is_err());
    }

    #[test]
    fn pkg_index_parses_entries_with_absolute_offsets() {
        let mut b = Vec::new();
        let magic = b"PKGV0019";
        b.extend(i32le(magic.len() as i32));
        b.extend_from_slice(magic);
        b.extend(i32le(2));
        for (name, off, len) in [
            ("Shaders\\Foo.vert", 0i32, 5i32),
            ("materials/bg.tex", 5, 3),
        ] {
            b.extend(i32le(name.len() as i32));
            b.extend_from_slice(name.as_bytes());
            b.extend(i32le(off));
            b.extend(i32le(len));
        }
        let data_start = b.len();
        b.extend_from_slice(b"hello");
        b.extend_from_slice(b"abc");

        let pkg = parse_pkg_index(&mut Cursor::new(&b[..])).unwrap();
        assert_eq!(pkg.magic, "PKGV0019");
        let e = &pkg.entries["shaders/foo.vert"];
        assert_eq!(e.full_path, "Shaders/Foo.vert");
        assert_eq!((e.offset, e.length), (data_start, 5));
        let e = &pkg.entries["materials/bg.tex"];
        assert_eq!(&b[e.offset..e.offset + e.length], b"abc");
    }

    #[test]
    fn pkg_rejects_bad_magic() {
        let mut b = i32le(4);
        b.extend_from_slice(b"NOPE");
        assert!(parse_pkg_index(&mut Cursor::new(&b[..])).is_err());
    }

    // ---- tex header / container versions --------------------------------------------------

    #[test]
    fn tex_header_and_texb0001_detection() {
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0001"));
        b.extend(i32le(1)); // image count
        b.extend(i32le(1)); // mipmap count
        b.extend(mip_v1(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]));
        let t = parse_tex(&b).unwrap();
        assert_eq!(t.header.format, TexFormat::RGBA8888);
        assert_eq!(t.container_version, 1);
        assert_eq!(t.free_image_format, FIF_UNKNOWN);
        assert_eq!(t.images[0][0].raw_bytes, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(!t.images[0][0].is_lz4_compressed);
        assert!(t.sprite_frames.is_none());
    }

    #[test]
    fn tex_texb0002_lz4_roundtrip() {
        let raw: Vec<u8> = (0..64u8).collect();
        let comp = lz4_flex::block::compress(&raw);
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0002"));
        b.extend(i32le(1));
        b.extend(i32le(1));
        b.extend(mip_v2(4, 4, true, raw.len() as i32, &comp));
        let t = parse_tex(&b).unwrap();
        assert_eq!(t.container_version, 2);
        assert!(t.images[0][0].is_lz4_compressed);
        assert_eq!(t.images[0][0].raw_bytes, raw);
    }

    #[test]
    fn tex_texb0003_reads_free_image_format() {
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0003"));
        b.extend(i32le(1));
        b.extend(i32le(13)); // FIF_PNG
        b.extend(i32le(1));
        b.extend(mip_v2(1, 1, false, 0, &[9, 9]));
        let t = parse_tex(&b).unwrap();
        assert_eq!(t.container_version, 3);
        assert_eq!(t.free_image_format, 13);
    }

    #[test]
    fn tex_texb0004_downgrades_to_v3_when_not_mp4() {
        // fmt = 13 (PNG), is_video_mp4 = 0 -> not FIF_MP4 -> Version 3 mipmap layout (no v4 prefix).
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0004"));
        b.extend(i32le(1)); // image count
        b.extend(i32le(13)); // free image format
        b.extend(i32le(0)); // is_video_mp4
        b.extend(i32le(1)); // mipmap count
        b.extend(mip_v2(1, 1, false, 0, &[7, 7, 7]));
        let t = parse_tex(&b).unwrap();
        assert_eq!(
            t.container_version, 3,
            "TEXB0004 w/o MP4 must use v3 layout"
        );
        assert_eq!(t.free_image_format, 13);
        assert_eq!(t.images[0][0].raw_bytes, vec![7, 7, 7]);

        // fmt = -1 and not video also downgrades.
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0004"));
        b.extend(i32le(1));
        b.extend(i32le(-1));
        b.extend(i32le(0));
        b.extend(i32le(1));
        b.extend(mip_v2(1, 1, false, 0, &[1]));
        let t = parse_tex(&b).unwrap();
        assert_eq!(t.container_version, 3);
        assert_eq!(t.free_image_format, FIF_UNKNOWN);
    }

    #[test]
    fn tex_texb0004_keeps_v4_layout_for_mp4() {
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0004"));
        b.extend(i32le(1));
        b.extend(i32le(-1)); // fmt
        b.extend(i32le(1)); // is_video_mp4 -> FIF_MP4
        b.extend(i32le(1)); // mipmap count
        b.extend(i32le(1)); // param1
        b.extend(i32le(2)); // param2
        b.extend(nstr("{}")); // condition json
        b.extend(i32le(1)); // param3
        b.extend(mip_v2(1, 1, false, 0, &[0xAA, 0xBB]));
        let t = parse_tex(&b).unwrap();
        assert_eq!(t.container_version, 4);
        assert_eq!(t.free_image_format, FIF_MP4);
        assert!(t.is_mp4());
        assert_eq!(t.images[0][0].raw_bytes, vec![0xAA, 0xBB]);
        let err = decode_tex_mipmap_to_rgba8(t.header.format, t.free_image_format, &t.images[0][0])
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Unsupported);
    }

    #[test]
    fn tex_rejects_bad_magics_and_oversized_payload() {
        let mut b = nstr("TEXV0004");
        b.extend(nstr("TEXI0001"));
        assert!(parse_tex(&b).is_err());

        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0009"));
        b.extend(i32le(1));
        assert!(parse_tex(&b).is_err());

        // Declared payload far larger than the buffer.
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0001"));
        b.extend(i32le(1));
        b.extend(i32le(1));
        b.extend(i32le(1));
        b.extend(i32le(1));
        b.extend(i32le(i32::MAX));
        assert!(parse_tex(&b).is_err());
    }

    #[test]
    fn tex_texs_sprite_frames() {
        let mut b = tex_header(0, tex_flags::IS_GIF);
        b.extend(nstr("TEXB0001"));
        b.extend(i32le(0)); // no images
        b.extend(nstr("TEXS0003"));
        b.extend(i32le(1)); // frame count
        b.extend(i32le(64)); // gif w
        b.extend(i32le(32)); // gif h
        b.extend(i32le(0)); // image id
        b.extend(f32le(0.1)); // frametime
        for v in [1.0f32, 2.0, 64.0, 0.0, 0.0, 32.0] {
            b.extend(f32le(v));
        }
        let t = parse_tex(&b).unwrap();
        let (w, h, frames) = t.sprite_frames.unwrap();
        assert_eq!((w, h), (64, 32));
        assert_eq!(frames.len(), 1);
        assert_eq!((frames[0].x, frames[0].y), (1.0, 2.0));
        assert_eq!((frames[0].width, frames[0].height), (64.0, 32.0));

        // TEXS0001 uses i32 geometry and infers gif size from frame 0.
        let mut b = tex_header(0, tex_flags::IS_GIF);
        b.extend(nstr("TEXB0001"));
        b.extend(i32le(0));
        b.extend(nstr("TEXS0001"));
        b.extend(i32le(1));
        b.extend(i32le(0));
        b.extend(f32le(0.5));
        for v in [0, 0, 16, 0, 0, 8] {
            b.extend(i32le(v));
        }
        let (w, h, frames) = parse_tex(&b).unwrap().sprite_frames.unwrap();
        assert_eq!((w, h), (16, 8));
        assert_eq!(frames[0].frametime, 0.5);
    }

    // ---- NPOT uv scale ---------------------------------------------------------------------

    #[test]
    fn npot_uv_scale_math() {
        let mut b = tex_header(0, 0);
        b.extend(nstr("TEXB0001"));
        b.extend(i32le(0));
        let t = parse_tex(&b).unwrap();
        // image 300x200 inside padded 512x256.
        let [su, sv] = t.uv_scale();
        assert!((su - 300.0 / 512.0).abs() < 1e-6);
        assert!((sv - 200.0 / 256.0).abs() < 1e-6);
        assert_eq!(npot_uv_scale(256, 256), 1.0);
        assert_eq!(npot_uv_scale(10, 0), 1.0);
    }

    // ---- pixel decoders --------------------------------------------------------------------

    fn mip(w: i32, h: i32, data: Vec<u8>) -> TexMipmap {
        TexMipmap {
            width: w,
            height: h,
            is_lz4_compressed: false,
            decompressed_bytes_count: 0,
            raw_bytes: data,
        }
    }

    #[test]
    fn decode_uncompressed_formats() {
        let rgba = decode_tex_mipmap_to_rgba8(
            TexFormat::RGBA8888,
            FIF_UNKNOWN,
            &mip(1, 1, vec![1, 2, 3, 4]),
        )
        .unwrap();
        assert_eq!(rgba, vec![1, 2, 3, 4]);

        let r8 = decode_tex_mipmap_to_rgba8(TexFormat::R8, FIF_UNKNOWN, &mip(2, 1, vec![10, 20]))
            .unwrap();
        assert_eq!(r8, vec![10, 10, 10, 10, 20, 20, 20, 20]);

        let rg = decode_tex_mipmap_to_rgba8(TexFormat::RG88, FIF_UNKNOWN, &mip(1, 1, vec![5, 9]))
            .unwrap();
        assert_eq!(rg, vec![5, 5, 5, 9]);

        assert!(
            decode_tex_mipmap_to_rgba8(TexFormat::R8, FIF_UNKNOWN, &mip(4, 4, vec![0; 3])).is_err()
        );
    }

    #[test]
    fn decode_bc_blocks_handle_non_multiple_of_four() {
        // DXT1 block: c0 = pure red (0xF800), c1 = 0, all indices 0 -> red.
        let dxt1 = [0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];
        let px =
            decode_tex_mipmap_to_rgba8(TexFormat::DXT1, FIF_UNKNOWN, &mip(3, 2, dxt1.to_vec()))
                .unwrap();
        assert_eq!(px.len(), 3 * 2 * 4);
        assert!(px.chunks(4).all(|p| p == [255, 0, 0, 255]));

        // DXT5 block: alpha endpoints 255/0 with all-zero indices -> alpha 255; colour red.
        let mut dxt5 = vec![255, 0, 0, 0, 0, 0, 0, 0];
        dxt5.extend_from_slice(&dxt1);
        let px =
            decode_tex_mipmap_to_rgba8(TexFormat::DXT5, FIF_UNKNOWN, &mip(4, 4, dxt5)).unwrap();
        assert!(px.chunks(4).all(|p| p == [255, 0, 0, 255]));

        // DXT3 block: explicit 4-bit alpha 0x8 -> 0x88. BC2 colour uses the 4-colour mode even if
        // c0 <= c1, so index 3 is an opaque blend rather than transparent black.
        let mut dxt3 = vec![0x88; 8];
        dxt3.extend_from_slice(&[0x00, 0x00, 0x00, 0xF8, 0xFF, 0xFF, 0xFF, 0xFF]);
        let px =
            decode_tex_mipmap_to_rgba8(TexFormat::DXT3, FIF_UNKNOWN, &mip(4, 4, dxt3)).unwrap();
        assert!(px.chunks(4).all(|p| p[3] == 0x88 && p[0] > 0));

        assert!(
            decode_tex_mipmap_to_rgba8(TexFormat::DXT1, FIF_UNKNOWN, &mip(4, 4, vec![0; 4]))
                .is_err()
        );
    }

    #[test]
    fn decode_embedded_png() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_pixel(2, 1, image::Rgba([1, 2, 3, 255]));
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let out = decode_tex_mipmap_to_rgba8(TexFormat::RGBA8888, 13, &mip(2, 1, png)).unwrap();
        assert_eq!(out, vec![1, 2, 3, 255, 1, 2, 3, 255]);
    }
}
