//! `.mdl` puppet skeleton (`MDLS`), animation (`MDLA`) parser and CPU bone/morph deformer
//! (`docs/05` Phase 6). `docs/03` §3.2 only specifies the `MDLV` mesh block (parsed by
//! `truongpaper_we_formats::parse_mdl`); the `MDLS`/`MDLA` layouts and the skinning math follow
//! the task reference notes `reference-mdl-skeleton-and-puppet.md` (summarized from the
//! open-source `catsout/wallpaper-scene-renderer`).
//!
//! All reads are bounds-checked: truncated input returns [`PuppetError::Truncated`] instead of
//! panicking. Morph-target files have no documented layout, so [`PuppetDeformer::deform`] takes
//! per-vertex deltas from the caller and no morph file parser exists.

use std::collections::HashMap;

use glam::{EulerRot, Mat4, Quat, Vec3, Vec4};
use truongpaper_we_formats::{MdlPuppetVertex, ParsedMdlMesh};

/// Bone transform record size in bytes (16 x f32).
pub const MDLS_BONE_TRANSFORM_BYTES: u32 = 64;
/// Animation frame record size in bytes (position, angle, scale: 9 x f32).
pub const MDLA_FRAME_BYTES: u32 = 36;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PuppetError {
    #[error("no {0} section in .mdl")]
    MissingSection(&'static str),
    #[error("truncated .mdl at byte {offset} while reading {what}")]
    Truncated { offset: usize, what: &'static str },
    #[error("invalid {tag} version string '{found}'")]
    BadVersion { tag: &'static str, found: String },
    #[error("bone {bone} has invalid parent {parent}")]
    BadParent { bone: usize, parent: u32 },
    #[error("bone {bone} has transform size {size}, expected 64")]
    BadBoneSize { bone: usize, size: u32 },
    #[error(
        "animation {animation} bone {bone} frame block of {size} bytes is not a multiple of 36"
    )]
    BadFrameBlock {
        animation: usize,
        bone: usize,
        size: u32,
    },
    #[error("animation {animation} has invalid id {id}")]
    BadAnimationId { animation: usize, id: i32 },
    #[error("{what} count {count} exceeds the remaining input")]
    CountTooLarge { what: &'static str, count: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PuppetBone {
    pub name: String,
    pub parent: Option<usize>,
    /// Local bind transform (column-major in the file).
    pub transform: Mat4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuppetAnimationMode {
    Loop,
    Mirror,
    Single,
}

impl PuppetAnimationMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "mirror" => PuppetAnimationMode::Mirror,
            "single" => PuppetAnimationMode::Single,
            _ => PuppetAnimationMode::Loop,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuppetFrame {
    pub position: Vec3,
    /// Euler angles in radians.
    pub angle: Vec3,
    pub scale: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PuppetAnimation {
    pub id: i32,
    pub name: String,
    pub mode: PuppetAnimationMode,
    pub fps: f32,
    /// Frame count.
    pub length: u32,
    /// `bone_frames[bone][frame]`.
    pub bone_frames: Vec<Vec<PuppetFrame>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PuppetSkeleton {
    /// `N` from `MDLS000N`.
    pub version: u32,
    pub bones: Vec<PuppetBone>,
    pub animations: Vec<PuppetAnimation>,
    /// `inverse(combined bind transform)` per bone.
    pub offsets: Vec<Mat4>,
}

impl PuppetSkeleton {
    pub fn animation(&self, id: i32) -> Option<&PuppetAnimation> {
        self.animations.iter().find(|a| a.id == id)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], PuppetError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.bytes.len())
            .ok_or(PuppetError::Truncated {
                offset: self.pos,
                what,
            })?;
        let s = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8, PuppetError> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &'static str) -> Result<u16, PuppetError> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, PuppetError> {
        let b = self.take(4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self, what: &'static str) -> Result<i32, PuppetError> {
        Ok(self.u32(what)? as i32)
    }

    fn f32(&mut self, what: &'static str) -> Result<f32, PuppetError> {
        Ok(f32::from_bits(self.u32(what)?))
    }

    fn vec3(&mut self, what: &'static str) -> Result<Vec3, PuppetError> {
        Ok(Vec3::new(self.f32(what)?, self.f32(what)?, self.f32(what)?))
    }

    fn mat4(&mut self, what: &'static str) -> Result<Mat4, PuppetError> {
        let mut m = [0f32; 16];
        for v in &mut m {
            *v = self.f32(what)?;
        }
        Ok(Mat4::from_cols_array(&m))
    }

    /// NUL-terminated string.
    fn cstr(&mut self, what: &'static str) -> Result<String, PuppetError> {
        let rest = &self.bytes[self.pos.min(self.bytes.len())..];
        let len = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or(PuppetError::Truncated {
                offset: self.bytes.len(),
                what,
            })?;
        let s = String::from_utf8_lossy(&rest[..len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }

    fn skip(&mut self, n: usize, what: &'static str) -> Result<(), PuppetError> {
        self.take(n, what).map(|_| ())
    }

    /// Rejects `count * min_bytes` larger than the remaining input (avoids huge allocations).
    fn check_count(
        &self,
        count: usize,
        min_bytes: usize,
        what: &'static str,
    ) -> Result<(), PuppetError> {
        if count.saturating_mul(min_bytes) > self.remaining() {
            Err(PuppetError::CountTooLarge { what, count })
        } else {
            Ok(())
        }
    }
}

fn version_number(found: &str, tag: &'static str) -> Result<u32, PuppetError> {
    found
        .strip_prefix(tag)
        .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|d| d.parse().ok())
        .ok_or_else(|| PuppetError::BadVersion {
            tag,
            found: found.to_string(),
        })
}

/// Parses the `MDLS` skeleton and the following `MDLA` animations of a `.mdl` whose mesh was
/// parsed by `parse_mdl` (section offsets come from `mesh.sections`).
pub fn parse_mdl_skeleton(
    bytes: &[u8],
    mesh: &ParsedMdlMesh,
) -> Result<PuppetSkeleton, PuppetError> {
    let mdls = mesh
        .section("MDLS")
        .ok_or(PuppetError::MissingSection("MDLS"))?;
    let mut r = Reader {
        bytes,
        pos: mdls.offset,
    };
    let version_str = r.cstr("MDLS version")?;
    let version = version_number(&version_str, "MDLS")?;
    let _bones_file_end = r.u32("MDLS size")?;
    let bones_num = r.u16("bone count")? as usize;
    let _unk = r.u16("MDLS unknown")?;
    // Smallest bone record: empty name + i32 + parent + size + 64 bytes + empty json.
    r.check_count(bones_num, 1 + 4 + 4 + 4 + 64 + 1, "bone")?;

    let mut bones = Vec::with_capacity(bones_num);
    for i in 0..bones_num {
        let name = r.cstr("bone name")?;
        let _unk = r.i32("bone unknown")?;
        let parent_raw = r.u32("bone parent")?;
        let parent = if parent_raw == u32::MAX {
            None
        } else if (parent_raw as usize) < i {
            Some(parent_raw as usize)
        } else {
            return Err(PuppetError::BadParent {
                bone: i,
                parent: parent_raw,
            });
        };
        let size = r.u32("bone transform size")?;
        if size != MDLS_BONE_TRANSFORM_BYTES {
            return Err(PuppetError::BadBoneSize { bone: i, size });
        }
        let transform = r.mat4("bone transform")?;
        let _simulation_json = r.cstr("bone simulation json")?;
        bones.push(PuppetBone {
            name,
            parent,
            transform,
        });
    }

    if version > 1 {
        let _unk = r.u16("MDLS extra")?;
        if r.u8("has_trans")? == 1 {
            r.skip(bones_num * 64, "bone trans")?;
        }
        let size_unk = r.u32("size_unk")? as usize;
        r.check_count(size_unk, 12, "size_unk")?;
        r.skip(size_unk * 12, "size_unk block")?;
        let _unk = r.u32("MDLS extra 2")?;
        if r.u8("has_offset_trans")? == 1 {
            r.skip(bones_num * (12 + 64), "offset trans")?;
        }
        if r.u8("has_index")? == 1 {
            r.skip(bones_num * 4, "bone index")?;
        }
    }

    let offsets = bind_offsets(&bones);
    let animations = match mesh
        .sections
        .iter()
        .find(|s| s.tag == "MDLA" && s.offset > mdls.offset)
    {
        Some(mdla) => parse_mdla(bytes, mdla.offset, bones_num, mesh.extended_layout)?,
        None => Vec::new(),
    };

    Ok(PuppetSkeleton {
        version,
        bones,
        animations,
        offsets,
    })
}

/// Like [`parse_mdl_skeleton`], but a mesh with no `MDLS` section (an unskinned puppet) gets a
/// single identity root bone and no animations, so every vertex stays at its rest position.
pub fn parse_mdl_skeleton_or_rest(
    bytes: &[u8],
    mesh: &ParsedMdlMesh,
) -> Result<PuppetSkeleton, PuppetError> {
    if mesh.section("MDLS").is_some() {
        return parse_mdl_skeleton(bytes, mesh);
    }
    let bones = vec![PuppetBone {
        name: "root".to_string(),
        parent: None,
        transform: Mat4::IDENTITY,
    }];
    let offsets = bind_offsets(&bones);
    Ok(PuppetSkeleton {
        version: 0,
        bones,
        animations: Vec::new(),
        offsets,
    })
}

fn parse_mdla(
    bytes: &[u8],
    offset: usize,
    bones_num: usize,
    extended_layout: bool,
) -> Result<Vec<PuppetAnimation>, PuppetError> {
    let mut r = Reader { bytes, pos: offset };
    let version_str = r.cstr("MDLA version")?;
    let version = version_number(&version_str, "MDLA")?;
    if version == 0 {
        return Ok(Vec::new());
    }
    let _end_size = r.u32("MDLA size")?;
    let anim_num = r.u32("animation count")? as usize;
    r.check_count(anim_num, 4 * 6, "animation")?;

    let mut out = Vec::with_capacity(anim_num);
    let mut frames_left = MAX_TOTAL_FRAMES;
    for a in 0..anim_num {
        let mut id = 0i32;
        while id == 0 {
            id = r.i32("animation id")?;
        }
        if id <= 0 {
            return Err(PuppetError::BadAnimationId { animation: a, id });
        }
        let _unk = r.i32("animation unknown")?;
        let mut name = r.cstr("animation name")?;
        if name.is_empty() {
            name = r.cstr("animation name")?;
        }
        let mode = PuppetAnimationMode::parse(&r.cstr("animation mode")?);
        let fps = r.f32("animation fps")?;
        let length = r.i32("animation length")?.max(0) as u32;
        let _unk = r.i32("animation unknown 2")?;
        let b_num = r.u32("animation bone count")? as usize;
        r.check_count(b_num, 8, "animation bone")?;
        if b_num != bones_num {
            tracing::warn!("animation {id} has {b_num} bone tracks for {bones_num} bones");
        }
        let mut bone_frames = Vec::with_capacity(b_num);
        for b in 0..b_num {
            let _unk = r.i32("bone track unknown")?;
            let size = r.u32("bone track size")?;
            if size % MDLA_FRAME_BYTES != 0 {
                return Err(PuppetError::BadFrameBlock {
                    animation: a,
                    bone: b,
                    size,
                });
            }
            let n = (size / MDLA_FRAME_BYTES) as usize;
            r.check_count(n, MDLA_FRAME_BYTES as usize, "frame")?;
            frames_left = frames_left
                .checked_sub(n)
                .ok_or(PuppetError::CountTooLarge {
                    what: "total frame",
                    count: n,
                })?;
            let mut frames = Vec::with_capacity(n);
            for _ in 0..n {
                frames.push(PuppetFrame {
                    position: r.vec3("frame position")?,
                    angle: r.vec3("frame angle")?,
                    scale: r.vec3("frame scale")?,
                });
            }
            bone_frames.push(frames);
        }
        let tracks_end = r.pos;
        let standard = skip_mdla_trailer(&mut r, version, extended_layout);
        if a + 1 == anim_num {
            standard?;
        } else {
            if standard.is_err() || !plausible_header(bytes, r.pos) {
                // Newer exports append extra per-clip blocks (e.g. float tracks); resync on the
                // next valid clip header instead of trusting a fixed trailer size.
                match find_next_header(bytes, tracks_end) {
                    Some(pos) => r.pos = pos,
                    None => {
                        return Err(PuppetError::Truncated {
                            offset: tracks_end,
                            what: "next animation header",
                        })
                    }
                }
            }
        }
        out.push(PuppetAnimation {
            id,
            name,
            mode,
            fps,
            length,
            bone_frames,
        });
    }
    Ok(out)
}

fn skip_mdla_trailer(
    r: &mut Reader<'_>,
    version: u32,
    extended_layout: bool,
) -> Result<(), PuppetError> {
    if extended_layout {
        // Extended mesh exports pad the next clip's ID with byte-sized fields.
        r.skip(if version >= 6 { 3 } else { 2 }, "extended MDLA trailer")
    } else if version == 3 {
        r.skip(1, "MDLA3 trailer")
    } else {
        let n = r.u32("animation event count")? as usize;
        r.check_count(n, 5, "animation event")?;
        for _ in 0..n {
            let _t = r.f32("animation event time")?;
            let _s = r.cstr("animation event name")?;
        }
        Ok(())
    }
}

/// Whether a well-formed animation clip header starts at `pos` (after optional zero padding).
fn plausible_header(bytes: &[u8], pos: usize) -> bool {
    let mut r = Reader { bytes, pos };
    let check = |r: &mut Reader<'_>| -> Result<bool, PuppetError> {
        let id = r.i32("id")?;
        if !(1..=100_000).contains(&id) {
            return Ok(false);
        }
        let _ = r.i32("unk")?;
        let mut name = r.cstr("name")?;
        if name.is_empty() {
            name = r.cstr("name")?;
        }
        if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
            return Ok(false);
        }
        if !matches!(r.cstr("mode")?.as_str(), "loop" | "single" | "mirror") {
            return Ok(false);
        }
        let fps = r.f32("fps")?;
        let length = r.i32("length")?;
        let _ = r.i32("unk2")?;
        let bones = r.u32("bones")? as usize;
        // Slow clips exist: 2477602742's FrontPole_puppet.mdl has a 0.48 fps "Animation 2".
        Ok(fps.is_finite()
            && fps > 0.0
            && fps <= 240.0
            && (1..=100_000).contains(&length)
            && bones <= 4096
            && bones * 8 <= r.remaining())
    };
    check(&mut r).unwrap_or(false)
}

/// Most animation frames (over all clips and bones) one file may hold.
const MAX_TOTAL_FRAMES: usize = 2_000_000;
/// Farthest `find_next_header` looks ahead, in bytes.
const MAX_HEADER_SCAN: usize = 4 * 1024 * 1024;

/// First byte offset at or after `from` where a clip header starts.
fn find_next_header(bytes: &[u8], from: usize) -> Option<usize> {
    let end = bytes
        .len()
        .saturating_sub(24)
        .min(from.saturating_add(MAX_HEADER_SCAN));
    let mut p = (from..end).find(|&p| plausible_header(bytes, p))?;
    // Zero padding before the id can also parse as a shifted header; take the last of the run.
    while plausible_header(bytes, p + 1) {
        p += 1;
    }
    Some(p)
}

/// `m.inverse()`, or identity when `m` is singular or the result is not finite (a bone scaled
/// to zero would otherwise fill the mesh with NaN).
fn safe_inverse(m: &Mat4) -> Mat4 {
    let det = m.determinant();
    // Relative to the column lengths (Hadamard's bound), so a small uniform scale is still
    // invertible while a collapsed axis is not.
    let bound: f32 = [m.x_axis, m.y_axis, m.z_axis, m.w_axis]
        .iter()
        .map(|c| c.length())
        .product();
    if !det.is_finite() || !bound.is_finite() || bound <= 0.0 || det.abs() < bound * 1e-6 {
        return Mat4::IDENTITY;
    }
    let inv = m.inverse();
    if inv.to_cols_array().iter().all(|v| v.is_finite()) {
        inv
    } else {
        Mat4::IDENTITY
    }
}

/// `combined[i] = combined[parent] * transform`; returns `inverse(combined[i])`.
fn bind_offsets(bones: &[PuppetBone]) -> Vec<Mat4> {
    let mut combined: Vec<Mat4> = Vec::with_capacity(bones.len());
    for b in bones {
        let parent = b
            .parent
            .and_then(|p| combined.get(p).copied())
            .unwrap_or(Mat4::IDENTITY);
        combined.push(parent * b.transform);
    }
    combined.iter().map(safe_inverse).collect()
}

/// `Rz(angle.z) * Ry(angle.y) * Rx(angle.x)`.
pub fn euler_to_quat(angle: Vec3) -> Quat {
    Quat::from_euler(EulerRot::ZYX, angle.z, angle.y, angle.x)
}

/// Maps a frame index of the doubled mirror timeline back onto `0..length`.
pub fn mirror_frame_index(k: usize, length: usize) -> usize {
    if length == 0 {
        0
    } else if k >= length {
        (length - 1).saturating_sub(k - length)
    } else {
        k
    }
}

/// `(frame_a, frame_b, blend)` for time `t` seconds. `None` when `fps` or `length` is unusable.
pub fn animation_frame_pair(
    mode: PuppetAnimationMode,
    t: f32,
    fps: f32,
    length: u32,
) -> Option<(usize, usize, f32)> {
    if fps.is_nan() || fps <= 0.0 || length == 0 || !t.is_finite() {
        return None;
    }
    let len = length as usize;
    if mode == PuppetAnimationMode::Single {
        let last = len - 1;
        let frame = (t * fps).clamp(0.0, last as f32);
        let a = frame as usize;
        return Some((a, (a + 1).min(last), frame.fract()));
    }
    let frame_time = 1.0 / fps;
    let (frames, max_time) = match mode {
        PuppetAnimationMode::Mirror => (2 * len, 2.0 * length as f32 / fps),
        _ => (len, length as f32 / fps),
    };
    let tt = t.rem_euclid(max_time);
    let r = tt / frame_time;
    let a = (r.floor() as usize) % frames;
    let b = (a + 1) % frames;
    let f = (r - r.floor()).clamp(0.0, 1.0);
    Some(match mode {
        PuppetAnimationMode::Mirror => (mirror_frame_index(a, len), mirror_frame_index(b, len), f),
        _ => (a, b, f),
    })
}

/// Plays one animation layer at blend 1.0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuppetAnimator {
    pub time: f32,
    pub rate: f32,
    /// `None` = rest pose.
    pub animation_id: Option<i32>,
}

impl Default for PuppetAnimator {
    fn default() -> Self {
        Self {
            time: 0.0,
            rate: 1.0,
            animation_id: None,
        }
    }
}

impl PuppetAnimator {
    pub fn new(animation_id: Option<i32>) -> Self {
        Self {
            animation_id,
            ..Self::default()
        }
    }

    /// Advances by `dt * rate` and returns `final[i] = world[i] * offset[i]` per bone.
    /// Without a (known) animation every matrix is the identity (rest pose).
    pub fn bone_matrices(&mut self, skeleton: &PuppetSkeleton, dt: f32) -> Vec<Mat4> {
        if dt.is_finite() && self.rate.is_finite() {
            self.time += dt * self.rate;
        }
        let n = skeleton.bones.len();
        let Some(anim) = self.animation_id.and_then(|id| skeleton.animation(id)) else {
            return vec![Mat4::IDENTITY; n];
        };
        let pair = animation_frame_pair(anim.mode, self.time, anim.fps, anim.length);
        let mut world: Vec<Mat4> = Vec::with_capacity(n);
        for (i, bone) in skeleton.bones.iter().enumerate() {
            let frames = anim.bone_frames.get(i).filter(|f| !f.is_empty());
            let local = match (frames, pair) {
                (Some(frames), Some((a, b, f))) => {
                    let fa = frames.get(a).unwrap_or(&frames[frames.len() - 1]);
                    let fb = frames.get(b).unwrap_or(&frames[frames.len() - 1]);
                    let t = fa.position.lerp(fb.position, f);
                    let rot = euler_to_quat(fa.angle).slerp(euler_to_quat(fb.angle), f);
                    let s = fa.scale.lerp(fb.scale, f);
                    Mat4::from_scale_rotation_translation(s, rot, t)
                }
                _ => bone.transform,
            };
            let parent = bone
                .parent
                .and_then(|p| world.get(p).copied())
                .unwrap_or(Mat4::IDENTITY);
            world.push(parent * local);
        }
        world
            .iter()
            .zip(&skeleton.offsets)
            .map(|(w, o)| *w * *o)
            .collect()
    }
}

/// An animation selected by the scene, with its user properties resolved.
#[derive(Debug, Clone, Copy)]
pub struct PuppetLayer {
    pub animation: i32,
    pub blend: f32,
    pub rate: f32,
    pub additive: bool,
}

#[derive(Clone, Copy)]
struct BonePose {
    position: Vec3,
    rotation: Quat,
    scale: Vec3,
}

impl From<PuppetFrame> for BonePose {
    fn from(frame: PuppetFrame) -> Self {
        Self {
            position: frame.position,
            rotation: euler_to_quat(frame.angle),
            scale: frame.scale,
        }
    }
}

impl BonePose {
    fn interpolate(self, next: Self, weight: f32) -> Self {
        Self {
            position: self.position.lerp(next.position, weight),
            rotation: self.rotation.slerp(next.rotation, weight),
            scale: self.scale.lerp(next.scale, weight),
        }
    }

    fn add_delta(&mut self, start: Self, current: Self, weight: f32) {
        self.position += (current.position - start.position) * weight;
        self.scale += (current.scale - start.scale) * weight;
        let delta = current.rotation * start.rotation.conjugate();
        self.rotation = Quat::IDENTITY.slerp(delta, weight) * self.rotation;
    }
}

/// Evaluates the scene's animation layers together. Looping additive clips apply motion from
/// their starting pose, while a one-shot clip applies its offset from the rig's reference pose.
pub fn layered_bone_matrices(
    skeleton: &PuppetSkeleton,
    layers: &[PuppetLayer],
    time: f32,
) -> Vec<Mat4> {
    let active: Vec<_> = layers
        .iter()
        .filter_map(|layer| {
            if !layer.blend.is_finite() || layer.blend <= 0.0 {
                return None;
            }
            let clip = skeleton.animation(layer.animation)?;
            let pair = animation_frame_pair(clip.mode, time * layer.rate, clip.fps, clip.length)?;
            Some((layer, clip, pair))
        })
        .collect();
    if active.is_empty() {
        return vec![Mat4::IDENTITY; skeleton.bones.len()];
    }

    let mut world: Vec<Mat4> = Vec::with_capacity(skeleton.bones.len());
    for (index, bone) in skeleton.bones.iter().enumerate() {
        let (scale, rotation, position) = bone.transform.to_scale_rotation_translation();
        let rest = BonePose {
            scale,
            rotation,
            position,
        };
        let samples = || {
            active.iter().filter_map(|(layer, clip, (a, b, t))| {
                let track = clip.bone_frames.get(index)?;
                let last = *track.last()?;
                let start = BonePose::from(track[0]);
                let frame = BonePose::from(track.get(*a).copied().unwrap_or(last))
                    .interpolate(track.get(*b).copied().unwrap_or(last).into(), *t);
                Some((*layer, clip.mode, start, frame))
            })
        };
        let opaque_weight: f32 = samples()
            .filter(|(layer, _, _, _)| !layer.additive)
            .map(|(layer, _, _, _)| layer.blend.min(1.0))
            .sum();
        let mut pose = rest;
        let mut accumulated = (1.0 - opaque_weight).max(0.0);
        if opaque_weight == 0.0 {
            // Character sheets can store their assembled starting pose in the clip, while the
            // bind pose still describes the separate pieces on the source texture.
            if let Some((_, _, start, _)) =
                samples().find(|(_, mode, _, _)| *mode != PuppetAnimationMode::Single)
            {
                pose = start;
            }
        }
        for (layer, _, _, frame) in samples().filter(|(layer, _, _, _)| !layer.additive) {
            let weight = layer.blend.min(1.0);
            accumulated += weight;
            pose = pose.interpolate(frame, weight / accumulated);
        }
        for (layer, mode, start, frame) in samples().filter(|(layer, _, _, _)| layer.additive) {
            let reference = if mode == PuppetAnimationMode::Single {
                rest
            } else {
                start
            };
            pose.add_delta(reference, frame, layer.blend.min(1.0));
        }
        let parent = bone
            .parent
            .and_then(|p| world.get(p).copied())
            .unwrap_or(Mat4::IDENTITY);
        world.push(
            parent
                * Mat4::from_scale_rotation_translation(pose.scale, pose.rotation, pose.position),
        );
    }
    world
        .iter()
        .zip(&skeleton.offsets)
        .map(|(world, offset)| *world * *offset)
        .collect()
}

/// CPU morph + skinning deformer.
pub struct PuppetDeformer;

impl PuppetDeformer {
    /// `p' = p + sum_j(w_j * delta_j[v])` (morphs, before skinning), then
    /// `p'' = sum_k(weights[k] * (bones[indices[k]] * vec4(p', 1))).xyz`. A zero weight sum keeps
    /// the morphed position; out-of-range bone indices use the identity.
    pub fn deform(
        vertices: &[MdlPuppetVertex],
        bones: &[Mat4],
        morphs: &[(f32, &[[f32; 3]])],
    ) -> Vec<[f32; 3]> {
        vertices
            .iter()
            .enumerate()
            .map(|(vi, v)| {
                let mut p = Vec3::from_array(v.position);
                for (w, deltas) in morphs {
                    if let Some(d) = deltas.get(vi) {
                        p += Vec3::from_array(*d) * *w;
                    }
                }
                let weight_sum: f32 = v.blend_weights.iter().sum();
                if weight_sum == 0.0 || !weight_sum.is_finite() {
                    return p.to_array();
                }
                let p4 = Vec4::new(p.x, p.y, p.z, 1.0);
                let mut acc = Vec4::ZERO;
                for k in 0..4 {
                    let w = v.blend_weights[k];
                    if w == 0.0 {
                        continue;
                    }
                    let m = bones
                        .get(v.blend_indices[k] as usize)
                        .copied()
                        .unwrap_or(Mat4::IDENTITY);
                    acc += (m * p4) * w;
                }
                acc.truncate().to_array()
            })
            .collect()
    }
}

/// Shader combos for a skinned puppet: `SKINNING = 1`, `BONECOUNT = bone_count`.
pub fn puppet_combos(bone_count: usize) -> HashMap<String, i32> {
    HashMap::from([
        ("SKINNING".to_string(), 1),
        (
            "BONECOUNT".to_string(),
            i32::try_from(bone_count).unwrap_or(i32::MAX),
        ),
    ])
}

#[cfg(test)]
mod resync_tests {
    use super::*;

    fn header(id: i32, bones: u32) -> Vec<u8> {
        header_fps(id, bones, 30.0)
    }

    fn header_fps(id: i32, bones: u32, fps: f32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(id.to_le_bytes());
        b.extend(0i32.to_le_bytes());
        b.extend(b"wave\0loop\0");
        b.extend(fps.to_le_bytes());
        b.extend(10i32.to_le_bytes());
        b.extend(0i32.to_le_bytes());
        b.extend(bones.to_le_bytes());
        b.extend(vec![0u8; bones as usize * 8]);
        b
    }

    #[test]
    fn finds_clip_header_after_unknown_extra_block() {
        let mut bytes = vec![0u8, 0, 0, 0, 1, 0xc4, 3, 0, 0, 0, 0];
        bytes.extend(vec![0u8; 29]);
        let at = bytes.len();
        bytes.extend(header(241, 2));
        assert_eq!(find_next_header(&bytes, 0), Some(at));
        assert!(!plausible_header(&bytes, 0));
    }

    /// Regression (2477602742): a 0.48 fps clip header is still a clip header.
    #[test]
    fn slow_clip_header_is_plausible() {
        let bytes = header_fps(159, 2, 0.483);
        assert!(plausible_header(&bytes, 0));
        assert!(!plausible_header(&header_fps(159, 2, 0.0), 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_uniform_scale_is_still_inverted() {
        let m = Mat4::from_scale(Vec3::splat(1e-4));
        let inv = safe_inverse(&m);
        assert!((inv.x_axis.x - 1e4).abs() < 1.0);
        assert_eq!(
            safe_inverse(&Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0))),
            Mat4::IDENTITY
        );
    }

    #[test]
    fn singular_bone_gives_finite_bind_offsets() {
        let bones = vec![
            PuppetBone {
                name: "zero".into(),
                parent: None,
                transform: Mat4::from_scale(Vec3::ZERO),
            },
            PuppetBone {
                name: "child".into(),
                parent: Some(0),
                transform: Mat4::from_translation(Vec3::new(1.0, 2.0, 0.0)),
            },
            PuppetBone {
                name: "nan".into(),
                parent: None,
                transform: Mat4::from_scale(Vec3::splat(f32::NAN)),
            },
        ];
        for m in bind_offsets(&bones) {
            assert!(m.to_cols_array().iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn regular_bone_keeps_its_inverse() {
        let m = Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0));
        assert!(safe_inverse(&m).abs_diff_eq(m.inverse(), 1e-6));
    }

    #[test]
    fn header_scan_is_bounded() {
        let bytes = vec![0u8; MAX_HEADER_SCAN + 4096];
        assert!(find_next_header(&bytes, 0).is_none());
    }
}
