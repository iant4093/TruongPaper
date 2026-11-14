//! Puppet attachment points (`MDAT` section of a `.mdl`).
//!
//! An object whose parent is a puppet image can name one of the parent's attachment points
//! (`"attachment": "head"`). Its `origin` is then measured from that point instead of from the
//! parent's centre (3463520581 hangs Asuna's hair off the `head` attachment of her body).
//!
//! `MDAT0001` layout: version string, `u32` end offset, `u16` count, then per attachment a `u16`
//! bone index, a null-terminated name and a 4x4 float matrix relative to that bone's bind pose.
//! The point lives in the puppet's mesh space: centred on the layer, y up, the same space as the
//! layer's local coordinates.

use glam::{Mat4, Vec3};

use crate::puppet::PuppetBone;

#[derive(Debug, Clone, PartialEq)]
pub struct PuppetAttachment {
    pub name: String,
    pub bone: usize,
    /// Transform relative to the bone.
    pub local: Mat4,
}

fn find_mdat(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(8)
        .position(|w| &w[..4] == b"MDAT" && w[4..8].iter().all(u8::is_ascii_digit))
}

/// Parses the `MDAT` section of a `.mdl` file. Missing or truncated data gives an empty list.
pub fn parse_attachments(bytes: &[u8]) -> Vec<PuppetAttachment> {
    let Some(start) = find_mdat(bytes) else {
        return Vec::new();
    };
    let mut pos = start + 9 + 4;
    let take = |pos: &mut usize, n: usize| -> Option<&[u8]> {
        let s = bytes.get(*pos..pos.checked_add(n)?)?;
        *pos += n;
        Some(s)
    };
    let u16_at = |pos: &mut usize| take(pos, 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let Some(count) = u16_at(&mut pos) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for _ in 0..count {
        let Some(bone) = u16_at(&mut pos) else { break };
        let Some(end) = bytes
            .get(pos..)
            .and_then(|b| b.iter().position(|&c| c == 0))
        else {
            break;
        };
        let name = String::from_utf8_lossy(&bytes[pos..pos + end]).into_owned();
        pos += end + 1;
        let Some(m) = take(&mut pos, 64) else { break };
        let mut cols = [0.0f32; 16];
        for (i, c) in m.chunks_exact(4).enumerate() {
            cols[i] = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
        if cols.iter().any(|v| !v.is_finite()) {
            continue;
        }
        out.push(PuppetAttachment {
            name,
            bone: bone as usize,
            local: Mat4::from_cols_array(&cols),
        });
    }
    out
}

/// Bind-pose position of attachment `name` in the puppet's layer-local space.
pub fn attachment_point(
    bones: &[PuppetBone],
    attachments: &[PuppetAttachment],
    name: &str,
) -> Option<Vec3> {
    let a = attachments.iter().find(|a| a.name == name)?;
    let mut world = vec![Mat4::IDENTITY; bones.len()];
    for (i, b) in bones.iter().enumerate() {
        let parent = b
            .parent
            .and_then(|p| world.get(p).copied())
            .unwrap_or(Mat4::IDENTITY);
        world[i] = parent * b.transform;
    }
    let bone = world.get(a.bone)?;
    let p = (*bone * a.local).w_axis.truncate();
    p.is_finite().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mdat(entries: &[(u16, &str, [f32; 2])]) -> Vec<u8> {
        let mut b = b"junkMDLS0004....".to_vec();
        b.extend(b"MDAT0001\0");
        b.extend(0u32.to_le_bytes());
        b.extend((entries.len() as u16).to_le_bytes());
        for (bone, name, t) in entries {
            b.extend(bone.to_le_bytes());
            b.extend(name.as_bytes());
            b.push(0);
            let m = Mat4::from_translation(Vec3::new(t[0], t[1], 0.0));
            for v in m.to_cols_array() {
                b.extend(v.to_le_bytes());
            }
        }
        b
    }

    fn bone(parent: Option<usize>, x: f32, y: f32) -> PuppetBone {
        PuppetBone {
            name: String::new(),
            parent,
            transform: Mat4::from_translation(Vec3::new(x, y, 0.0)),
        }
    }

    #[test]
    fn out_of_range_bone_parent_does_not_panic() {
        let bones = vec![bone(Some(99), 1.0, 2.0)];
        let a = parse_attachments(&mdat(&[(0, "head", [0.0, 0.0])]));
        let p = attachment_point(&bones, &a, "head").unwrap();
        assert_eq!(p, Vec3::new(1.0, 2.0, 0.0));
    }

    #[test]
    fn parses_named_attachments() {
        let a = parse_attachments(&mdat(&[
            (2, "head", [-32.5, 116.4]),
            (0, "foot", [1.0, 2.0]),
        ]));
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].name, "head");
        assert_eq!(a[0].bone, 2);
        assert_eq!(a[1].local.w_axis.truncate(), Vec3::new(1.0, 2.0, 0.0));
    }

    #[test]
    fn missing_or_truncated_section_is_empty() {
        assert!(parse_attachments(b"MDLV0023 no attachments").is_empty());
        let mut b = mdat(&[(1, "head", [0.0, 0.0])]);
        b.truncate(b.len() - 10);
        assert!(parse_attachments(&b).is_empty());
    }

    #[test]
    fn point_follows_bone_chain() {
        let bones = [bone(None, 10.0, 0.0), bone(Some(0), 0.0, 100.0)];
        let a = parse_attachments(&mdat(&[(1, "head", [5.0, 20.0])]));
        assert_eq!(
            attachment_point(&bones, &a, "head"),
            Some(Vec3::new(15.0, 120.0, 0.0))
        );
        assert_eq!(attachment_point(&bones, &a, "tail"), None);
    }
}
