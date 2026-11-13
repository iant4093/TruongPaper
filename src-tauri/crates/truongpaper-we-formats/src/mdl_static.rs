//! Static 3D `.mdl` meshes referenced directly by a scene `model` object (`MDLV0016`/`MDLV0023`).
//!
//! Layout (reverse-engineered from shipped workshop scenes; cross-checked with the viewers
//! `linux-wallpaperengine` / `wallpaper-engine-kde-plugin`):
//!
//! ```text
//! "MDLVnnnn" NUL | u32 flags | u32 a | u32 b | material path (NUL terminated)
//! u32 0 | [bbox min f32x3, max f32x3  (MDLV0023 only)]
//! u32 vertex format (0xF = pos|normal|tangent|uv) | u32 vertex bytes | vertices (48 B each:
//!   pos f32x3, normal f32x3, tangent f32x4, uv f32x2)
//! u32 index bytes | indices (u16 triangles) | trailing zero padding
//! ```

use std::io::{Error, ErrorKind, Result as IoResult};

/// Bytes per static vertex (`pos 12 + normal 12 + tangent 16 + uv 8`).
pub const STATIC_VERTEX_STRIDE: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticMdlVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub tangent: [f32; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct StaticMdl {
    pub version: String,
    /// VFS path of the material json (e.g. `materials/models/1/Material__50.json`).
    pub material: String,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub vertices: Vec<StaticMdlVertex>,
    pub indices: Vec<u32>,
}

fn bad(msg: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, msg.into())
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Whether a vertex block header (`format`, `byte length`) at `at` is plausible.
fn plausible_vertex_header(b: &[u8], at: usize) -> bool {
    let (Some(fmt), Some(len)) = (u32_at(b, at), u32_at(b, at + 4)) else {
        return false;
    };
    if fmt != 0xF || len == 0 || !(len as usize).is_multiple_of(STATIC_VERTEX_STRIDE) {
        return false;
    }
    let idx_at = at + 8 + len as usize;
    match u32_at(b, idx_at) {
        Some(ilen) => ilen.is_multiple_of(6) && idx_at + 4 + ilen as usize <= b.len(),
        None => false,
    }
}

/// Parses a static (non-skinned) 3D mesh with a single material.
pub fn parse_static_mdl(bytes: &[u8]) -> IoResult<StaticMdl> {
    if bytes.len() < 32 || !bytes.starts_with(b"MDLV") {
        return Err(bad("Invalid .mdl magic header"));
    }
    let version = String::from_utf8_lossy(&bytes[..8]).into_owned();
    if !version[4..].bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad(format!("Invalid .mdl version tag: {version}")));
    }
    let name_at = 9 + 12;
    let name_end = bytes
        .get(name_at..)
        .and_then(|s| s.iter().position(|&c| c == 0))
        .ok_or_else(|| bad("unterminated material path"))?
        + name_at;
    let material = String::from_utf8_lossy(&bytes[name_at..name_end]).into_owned();

    // After `u32 0`: either the vertex header directly, or a 24-byte bounding box first.
    let after = name_end + 1 + 4;
    let (header, bbox) = if plausible_vertex_header(bytes, after) {
        (after, None)
    } else if plausible_vertex_header(bytes, after + 24) {
        (after + 24, Some(after))
    } else {
        return Err(bad("unsupported .mdl vertex layout"));
    };
    let vlen = u32_at(bytes, header + 4).unwrap_or(0) as usize;
    let vstart = header + 8;
    let count = vlen / STATIC_VERTEX_STRIDE;
    let mut vertices = Vec::with_capacity(count);
    for i in 0..count {
        let o = vstart + i * STATIC_VERTEX_STRIDE;
        let f = |k: usize| f32_at(bytes, o + k * 4);
        vertices.push(StaticMdlVertex {
            position: [f(0), f(1), f(2)],
            normal: [f(3), f(4), f(5)],
            tangent: [f(6), f(7), f(8), f(9)],
            uv: [f(10), f(11)],
        });
    }
    let ilen_at = vstart + vlen;
    let ilen = u32_at(bytes, ilen_at).unwrap_or(0) as usize;
    let indices: Vec<u32> = bytes[ilen_at + 4..ilen_at + 4 + ilen]
        .chunks_exact(2)
        .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
        .collect();
    if indices.iter().any(|&i| i as usize >= count) {
        return Err(bad("index out of range"));
    }
    let (bbox_min, bbox_max) = match bbox {
        Some(b) => (
            [f32_at(bytes, b), f32_at(bytes, b + 4), f32_at(bytes, b + 8)],
            [
                f32_at(bytes, b + 12),
                f32_at(bytes, b + 16),
                f32_at(bytes, b + 20),
            ],
        ),
        None => {
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for v in &vertices {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.position[k]);
                    hi[k] = hi[k].max(v.position[k]);
                }
            }
            (lo, hi)
        }
    };
    Ok(StaticMdl {
        version,
        material,
        bbox_min,
        bbox_max,
        vertices,
        indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(version: &str, with_bbox: bool) -> Vec<u8> {
        let mut b = version.as_bytes().to_vec();
        b.push(0);
        for v in [15u32, 1, 1] {
            b.extend(v.to_le_bytes());
        }
        b.extend(b"materials/m.json\0");
        b.extend(0u32.to_le_bytes());
        if with_bbox {
            for v in [-1f32, -1.0, -1.0, 1.0, 1.0, 1.0] {
                b.extend(v.to_le_bytes());
            }
        }
        b.extend(0xFu32.to_le_bytes());
        b.extend((3 * 48u32).to_le_bytes());
        for i in 0..3 {
            for v in [
                i as f32, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, 0.5, i as f32,
            ] {
                b.extend(v.to_le_bytes());
            }
        }
        b.extend(6u32.to_le_bytes());
        for i in [0u16, 1, 2] {
            b.extend(i.to_le_bytes());
        }
        b.extend([0u8; 7]);
        b
    }

    #[test]
    fn parses_with_and_without_bbox() {
        let a = parse_static_mdl(&build("MDLV0023", true)).unwrap();
        assert_eq!(a.material, "materials/m.json");
        assert_eq!(a.vertices.len(), 3);
        assert_eq!(a.indices, vec![0, 1, 2]);
        assert_eq!(a.bbox_min, [-1.0; 3]);
        assert_eq!(a.vertices[2].position, [2.0, 0.0, 0.0]);
        assert_eq!(a.vertices[1].normal, [0.0, 0.0, 1.0]);
        assert_eq!(a.vertices[1].uv, [0.5, 1.0]);
        let b = parse_static_mdl(&build("MDLV0016", false)).unwrap();
        assert_eq!(b.vertices.len(), 3);
        assert_eq!(b.bbox_max, [2.0, 0.0, 0.0]);
    }

    #[test]
    fn rejects_garbage_and_bad_indices() {
        assert!(parse_static_mdl(b"nope").is_err());
        let mut b = build("MDLV0023", true);
        let n = b.len();
        b[n - 7 - 6] = 9; // index 9 >= 3 vertices
        assert!(parse_static_mdl(&b).is_err());
    }
}
