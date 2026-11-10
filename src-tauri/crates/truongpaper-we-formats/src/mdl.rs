//! `.mdl` puppet mesh parser (`MDLV0005`/`MDLV0013`/`MDLV0021`/`MDLV0023`, `MDLS`, `MDLA`).
//!
//! Layout verified against `linux-wallpaperengine` (`CImage::loadPuppetMesh`); see spec section 3.2.

use bytemuck::{Pod, Zeroable};
use std::io::{Error, ErrorKind, Result as IoResult};

/// Bytes per vertex in the `.mdl` file:
/// `pos vec3 | weight_indices uvec4 | weights vec4 | (28 unused bytes) | tc vec2`.
pub const MDL_VERTEX_STRIDE: usize = 80;
/// Bytes per triangle in the index block (`3 * u16`).
pub const MDL_TRIANGLE_BYTES: usize = 6;

/// Parsed vertex (CPU/GPU side). Note `size_of::<MdlPuppetVertex>()` is 52, not the 80-byte
/// on-disk [`MDL_VERTEX_STRIDE`]; the 28 padding bytes in the file are dropped.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct MdlPuppetVertex {
    pub position: [f32; 3],
    pub blend_indices: [u32; 4],
    pub blend_weights: [f32; 4],
    pub uv: [f32; 2],
}

/// A tagged trailing section (`MDLS` skeleton or `MDLA` animation) located in the file.
/// `offset` is the absolute byte offset of the 4-byte tag; `version` is e.g. `"MDLS0003"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdlSection {
    pub tag: String,
    pub version: String,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct ParsedMdlMesh {
    /// e.g. `"MDLV0023"`.
    pub version: String,
    pub vertices: Vec<MdlPuppetVertex>,
    pub indices: Vec<u16>,
    /// `MDLS` / `MDLA` sections found after the mesh block, in file order.
    pub sections: Vec<MdlSection>,
}

impl ParsedMdlMesh {
    pub fn section(&self, tag: &str) -> Option<&MdlSection> {
        self.sections.iter().find(|s| s.tag == tag)
    }
}

fn read_u32(bytes: &[u8], at: usize) -> Option<usize> {
    let b = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn read_f32(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn read_u32_raw(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn find_tag(bytes: &[u8], tag: &[u8; 4], from: usize) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(4)
        .position(|w| w == tag)
        .map(|p| p + from)
}

/// Collects `MDLS`/`MDLA` tags that are followed by four ASCII digits (e.g. `MDLS0003`).
fn find_sections(bytes: &[u8], from: usize) -> Vec<MdlSection> {
    let mut out = Vec::new();
    for (i, w) in bytes.get(from..).unwrap_or(&[]).windows(8).enumerate() {
        if (&w[..4] == b"MDLS" || &w[..4] == b"MDLA") && w[4..8].iter().all(u8::is_ascii_digit) {
            out.push(MdlSection {
                tag: String::from_utf8_lossy(&w[..4]).into_owned(),
                version: String::from_utf8_lossy(w).into_owned(),
                offset: from + i,
            });
        }
    }
    out
}

/// Parses the puppet mesh. `layer_size` is the layer's `[size_x, size_y]`; vertices are transformed
/// to layer space: `pos = (size_x/2 + x, size_y/2 - y, z)`.
pub fn parse_mdl(bytes: &[u8], layer_size: [f32; 2]) -> IoResult<ParsedMdlMesh> {
    if bytes.len() < 24 || !bytes.starts_with(b"MDLV") {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Invalid .mdl magic header",
        ));
    }
    let version = String::from_utf8_lossy(&bytes[..8]).into_owned();
    if !version[4..].bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("Invalid .mdl version tag: {version}"),
        ));
    }

    // The mesh block ends where the "MDLS" skeleton section begins (if any).
    let scan_limit = find_tag(bytes, b"MDLS", 9).unwrap_or(bytes.len());

    let mut found_block: Option<(usize, usize, usize)> = None;
    for offset in 9..scan_limit.saturating_sub(16) {
        let Some(v_bytes) = read_u32(bytes, offset + 4) else {
            break;
        };
        if v_bytes == 0 || v_bytes % MDL_VERTEX_STRIDE != 0 || v_bytes > scan_limit {
            continue;
        }
        let idx_len_pos = offset + 8 + v_bytes;
        if idx_len_pos + 4 > scan_limit {
            continue;
        }
        let Some(i_bytes) = read_u32(bytes, idx_len_pos) else {
            continue;
        };
        if i_bytes > 0
            && i_bytes % MDL_TRIANGLE_BYTES == 0
            && idx_len_pos + 4 + i_bytes <= scan_limit
        {
            found_block = Some((offset + 8, v_bytes, i_bytes));
            break;
        }
    }

    let (v_start, v_bytes, i_bytes) = found_block.ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidData,
            "Could not locate valid 80-byte stride mesh block in .mdl",
        )
    })?;

    let v_count = v_bytes / MDL_VERTEX_STRIDE;
    let mut vertices = Vec::with_capacity(v_count);
    let half_w = layer_size[0] * 0.5;
    let half_h = layer_size[1] * 0.5;

    for i in 0..v_count {
        let start = v_start + i * MDL_VERTEX_STRIDE;
        let b = &bytes[start..start + MDL_VERTEX_STRIDE];
        let x = read_f32(b, 0);
        let y = read_f32(b, 4);
        let z = read_f32(b, 8);
        vertices.push(MdlPuppetVertex {
            position: [half_w + x, half_h - y, z],
            blend_indices: [
                read_u32_raw(b, 12),
                read_u32_raw(b, 16),
                read_u32_raw(b, 20),
                read_u32_raw(b, 24),
            ],
            blend_weights: [
                read_f32(b, 28),
                read_f32(b, 32),
                read_f32(b, 36),
                read_f32(b, 40),
            ],
            uv: [read_f32(b, 72), read_f32(b, 76)],
        });
    }

    let i_start = v_start + v_bytes + 4;
    let indices = bytes[i_start..i_start + i_bytes]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();

    let sections = find_sections(bytes, i_start + i_bytes);

    Ok(ParsedMdlMesh {
        version,
        vertices,
        indices,
        sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32le(v: f32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// One 80-byte vertex.
    fn vertex(pos: [f32; 3], idx: [u32; 4], w: [f32; 4], uv: [f32; 2]) -> Vec<u8> {
        let mut b = Vec::new();
        for p in pos {
            b.extend(f32le(p));
        }
        for i in idx {
            b.extend(i.to_le_bytes());
        }
        for x in w {
            b.extend(f32le(x));
        }
        b.extend([0u8; 28]); // bytes 44..72
        for t in uv {
            b.extend(f32le(t));
        }
        assert_eq!(b.len(), MDL_VERTEX_STRIDE);
        b
    }

    fn build_mdl(magic: &str, with_sections: bool) -> Vec<u8> {
        let mut b = magic.as_bytes().to_vec();
        b.push(0); // 9-byte magic
        b.extend([0u8; 3]); // pad so the block header lands at a non-trivial offset
                            // header_offset = 12: [u32 unk][u32 vertex_bytes][vertices][u32 index_bytes][indices]
        b.extend(1u32.to_le_bytes());
        b.extend((2 * 80u32).to_le_bytes());
        b.extend(vertex(
            [10.0, 4.0, 1.0],
            [1, 2, 3, 4],
            [0.5, 0.5, 0.0, 0.0],
            [0.25, 0.75],
        ));
        b.extend(vertex(
            [-10.0, -4.0, 2.0],
            [0, 0, 0, 0],
            [1.0, 0.0, 0.0, 0.0],
            [1.0, 0.0],
        ));
        b.extend(6u32.to_le_bytes());
        for i in [0u16, 1, 0] {
            b.extend(i.to_le_bytes());
        }
        if with_sections {
            b.extend(b"MDLS0003\0");
            b.extend([0u8; 16]);
            b.extend(b"MDLA0004\0");
            b.extend([0u8; 8]);
        }
        b
    }

    #[test]
    fn vertex_stride_is_80_bytes_on_disk() {
        assert_eq!(MDL_VERTEX_STRIDE, 80);
        // Fields: 12 + 16 + 16 + 8 = 52 bytes parsed; the 28 padding bytes are skipped.
        assert_eq!(std::mem::size_of::<MdlPuppetVertex>(), 52);
        assert_eq!(
            MDL_VERTEX_STRIDE - std::mem::size_of::<MdlPuppetVertex>(),
            28
        );
    }

    #[test]
    fn parses_mesh_block_and_applies_layer_transform() {
        for magic in ["MDLV0005", "MDLV0013", "MDLV0021", "MDLV0023"] {
            let mdl = parse_mdl(&build_mdl(magic, false), [200.0, 100.0]).unwrap();
            assert_eq!(mdl.version, magic);
            assert_eq!(mdl.vertices.len(), 2);
            assert_eq!(mdl.indices, vec![0, 1, 0]);
            // (size_x/2 + x, size_y/2 - y, z)
            assert_eq!(mdl.vertices[0].position, [110.0, 46.0, 1.0]);
            assert_eq!(mdl.vertices[1].position, [90.0, 54.0, 2.0]);
            assert_eq!(mdl.vertices[0].blend_indices, [1, 2, 3, 4]);
            assert_eq!(mdl.vertices[0].blend_weights, [0.5, 0.5, 0.0, 0.0]);
            assert_eq!(mdl.vertices[0].uv, [0.25, 0.75]);
            assert!(mdl.sections.is_empty());
        }
    }

    #[test]
    fn locates_mds_and_mda_sections() {
        let bytes = build_mdl("MDLV0023", true);
        let mdl = parse_mdl(&bytes, [100.0, 100.0]).unwrap();
        assert_eq!(mdl.vertices.len(), 2);
        let s = mdl.section("MDLS").unwrap();
        assert_eq!(s.version, "MDLS0003");
        assert_eq!(&bytes[s.offset..s.offset + 8], b"MDLS0003");
        let a = mdl.section("MDLA").unwrap();
        assert_eq!(a.version, "MDLA0004");
        assert!(a.offset > s.offset);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_mdl(b"nope", [1.0, 1.0]).is_err());
        assert!(parse_mdl(
            &[
                b'M', b'D', b'L', b'V', b'0', b'0', b'2', b'1', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0
            ],
            [1.0, 1.0]
        )
        .is_err());
    }
}
