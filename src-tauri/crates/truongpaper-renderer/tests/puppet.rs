use glam::{Mat4, Vec3};
use truongpaper_renderer::puppet::{
    animation_frame_pair, euler_to_quat, layered_bone_matrices, mirror_frame_index,
    PuppetAnimationMode, PuppetLayer,
};
use truongpaper_renderer::{
    parse_mdl_skeleton, puppet_combos, PuppetAnimator, PuppetDeformer, PuppetError,
};
use truongpaper_we_formats::{parse_mdl, MdlPuppetVertex};

fn f32s(b: &mut Vec<u8>, vals: &[f32]) {
    for v in vals {
        b.extend(v.to_le_bytes());
    }
}

fn cstr(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(s.as_bytes());
    b.push(0);
}

/// One 80-byte MDLV vertex.
fn vertex(b: &mut Vec<u8>, pos: [f32; 3], idx: [u32; 4], w: [f32; 4]) {
    f32s(b, &pos);
    for i in idx {
        b.extend(i.to_le_bytes());
    }
    f32s(b, &w);
    b.extend([0u8; 28]);
    f32s(b, &[0.5, 0.5]);
}

struct BoneSpec {
    name: &'static str,
    parent: u32,
    size: u32,
    transform: Mat4,
}

struct Frame {
    pos: [f32; 3],
    angle: [f32; 3],
    scale: [f32; 3],
}

const REST: Frame = Frame {
    pos: [0.0; 3],
    angle: [0.0; 3],
    scale: [1.0; 3],
};

struct Built {
    bytes: Vec<u8>,
    mdls_offset: usize,
    mdls_end: usize,
    mdla_offset: usize,
}

/// MDLV0023 mesh (3 vertices) + MDLS000<mdls_version> + MDLA0004 with one animation whose
/// per-bone tracks are `tracks` (`frame_block_override` replaces the first track's byte size).
fn build(
    mdls_version: u32,
    bones: &[BoneSpec],
    tracks: &[Vec<Frame>],
    frame_block_override: Option<u32>,
) -> Built {
    let mut b = b"MDLV0023\0".to_vec();
    b.extend([0u8; 3]);
    b.extend(1u32.to_le_bytes());
    b.extend((3 * 80u32).to_le_bytes());
    vertex(&mut b, [1.0, 2.0, 0.0], [0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    vertex(&mut b, [3.0, 4.0, 0.0], [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    vertex(&mut b, [5.0, 6.0, 0.0], [0, 1, 0, 0], [0.0, 0.0, 0.0, 0.0]);
    b.extend(6u32.to_le_bytes());
    for i in [0u16, 1, 2] {
        b.extend(i.to_le_bytes());
    }

    let mdls_offset = b.len();
    cstr(&mut b, &format!("MDLS000{mdls_version}"));
    b.extend(0u32.to_le_bytes());
    b.extend((bones.len() as u16).to_le_bytes());
    b.extend(0u16.to_le_bytes());
    for bone in bones {
        cstr(&mut b, bone.name);
        b.extend(0i32.to_le_bytes());
        b.extend(bone.parent.to_le_bytes());
        b.extend(bone.size.to_le_bytes());
        f32s(&mut b, &bone.transform.to_cols_array());
        cstr(&mut b, "");
    }
    if mdls_version > 1 {
        b.extend(0i16.to_le_bytes());
        b.push(1); // has_trans
        for _ in bones {
            f32s(&mut b, &[0.0; 16]);
        }
        b.extend(2u32.to_le_bytes()); // size_unk
        b.extend([0u8; 24]);
        b.extend(0u32.to_le_bytes());
        b.push(1); // has_offset_trans
        for _ in bones {
            f32s(&mut b, &[0.0; 19]);
        }
        b.push(1); // has_index
        for i in 0..bones.len() as u32 {
            b.extend(i.to_le_bytes());
        }
    }
    let mdls_end = b.len();

    let mdla_offset = b.len();
    cstr(&mut b, "MDLA0004");
    b.extend(0u32.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    b.extend(0i32.to_le_bytes()); // zero padding before the id
    b.extend(7i32.to_le_bytes()); // id
    b.extend(0i32.to_le_bytes());
    cstr(&mut b, "");
    cstr(&mut b, "wave");
    cstr(&mut b, "loop");
    f32s(&mut b, &[1.0]); // fps
    let length = tracks.iter().map(Vec::len).max().unwrap_or(0) as i32;
    b.extend(length.to_le_bytes());
    b.extend(0i32.to_le_bytes());
    b.extend((tracks.len() as u32).to_le_bytes());
    for (i, track) in tracks.iter().enumerate() {
        b.extend(0i32.to_le_bytes());
        let size = match (i, frame_block_override) {
            (0, Some(s)) => s,
            _ => (track.len() * 36) as u32,
        };
        b.extend(size.to_le_bytes());
        for f in track {
            f32s(&mut b, &f.pos);
            f32s(&mut b, &f.angle);
            f32s(&mut b, &f.scale);
        }
    }
    b.extend(0u32.to_le_bytes()); // no events
    Built {
        bytes: b,
        mdls_offset,
        mdls_end,
        mdla_offset,
    }
}

fn two_bones() -> Vec<BoneSpec> {
    vec![
        BoneSpec {
            name: "root",
            parent: u32::MAX,
            size: 64,
            transform: Mat4::IDENTITY,
        },
        BoneSpec {
            name: "arm",
            parent: 0,
            size: 64,
            transform: Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)),
        },
    ]
}

fn moving_root_tracks() -> Vec<Vec<Frame>> {
    vec![
        vec![
            REST,
            Frame {
                pos: [10.0, 0.0, 0.0],
                ..REST
            },
        ],
        vec![
            Frame {
                pos: [5.0, 0.0, 0.0],
                ..REST
            },
            Frame {
                pos: [5.0, 0.0, 0.0],
                ..REST
            },
        ],
    ]
}

fn mesh_vertices() -> Vec<MdlPuppetVertex> {
    vec![
        MdlPuppetVertex {
            position: [1.0, 2.0, 0.0],
            blend_indices: [0, 0, 0, 0],
            blend_weights: [1.0, 0.0, 0.0, 0.0],
            uv: [0.0, 0.0],
        },
        MdlPuppetVertex {
            position: [3.0, 4.0, 0.0],
            blend_indices: [1, 0, 0, 0],
            blend_weights: [1.0, 0.0, 0.0, 0.0],
            uv: [0.0, 0.0],
        },
        MdlPuppetVertex {
            position: [5.0, 6.0, 0.0],
            blend_indices: [0, 1, 0, 0],
            blend_weights: [0.0; 4],
            uv: [0.0, 0.0],
        },
    ]
}

#[test]
fn parses_two_bones_and_an_animation() {
    for version in [1, 2] {
        let built = build(version, &two_bones(), &moving_root_tracks(), None);
        let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
        assert_eq!(mesh.vertices.len(), 3);
        let sk = parse_mdl_skeleton(&built.bytes, &mesh).unwrap();
        assert_eq!(sk.version, version);
        assert_eq!(sk.bones.len(), 2);
        assert_eq!(sk.bones[0].name, "root");
        assert_eq!(sk.bones[0].parent, None);
        assert_eq!(sk.bones[1].name, "arm");
        assert_eq!(sk.bones[1].parent, Some(0));
        assert_eq!(
            sk.bones[1].transform,
            Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0))
        );
        assert!(sk.offsets[1].abs_diff_eq(Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0)), 1e-6));
        assert_eq!(sk.animations.len(), 1);
        let a = &sk.animations[0];
        assert_eq!(
            (a.id, a.name.as_str(), a.mode),
            (7, "wave", PuppetAnimationMode::Loop)
        );
        assert_eq!((a.fps, a.length), (1.0, 2));
        assert_eq!(a.bone_frames.len(), 2);
        assert_eq!(a.bone_frames[0][1].position, Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(a.bone_frames[0][1].scale, Vec3::ONE);
        assert!(sk.animation(7).is_some() && sk.animation(8).is_none());
    }
}

#[test]
fn parses_extended_mesh_and_blends_its_selected_clips() {
    let built = build(2, &two_bones(), &moving_root_tracks(), None);
    let mut bytes = built.bytes;
    bytes[12..16].copy_from_slice(&0x0180000fu32.to_le_bytes());
    for vertex in bytes[20..20 + 3 * 80].as_chunks_mut::<80>().0 {
        vertex[12..72].rotate_right(28);
    }

    let mdla = built.mdla_offset;
    bytes[mdla + 7] = b'6';
    bytes[mdla + 13..mdla + 17].copy_from_slice(&2u32.to_le_bytes());
    let mut second_clip = bytes[mdla + 21..bytes.len() - 4].to_vec();
    second_clip[..4].copy_from_slice(&8i32.to_le_bytes());
    bytes.truncate(bytes.len() - 4);
    bytes.extend([0, 0, 0]);
    bytes.extend(second_clip);
    bytes.extend([0, 0, 0]);

    let mesh = parse_mdl(&bytes, [0.0, 0.0]).unwrap();
    assert!(mesh.extended_layout);
    assert_eq!(mesh.vertices[1].blend_indices, [1, 0, 0, 0]);
    assert_eq!(mesh.vertices[1].blend_weights, [1.0, 0.0, 0.0, 0.0]);
    let skeleton = parse_mdl_skeleton(&bytes, &mesh).unwrap();
    assert_eq!(
        skeleton
            .animations
            .iter()
            .map(|clip| clip.id)
            .collect::<Vec<_>>(),
        [7, 8]
    );

    let layers = [
        PuppetLayer {
            animation: 7,
            blend: 0.5,
            rate: 1.0,
            additive: true,
        },
        PuppetLayer {
            animation: 8,
            blend: 0.25,
            rate: 1.0,
            additive: true,
        },
    ];
    let matrices = layered_bone_matrices(&skeleton, &layers, 0.5);
    assert!(matrices[0]
        .transform_point3(Vec3::ZERO)
        .abs_diff_eq(Vec3::new(3.75, 0.0, 0.0), 1e-6));
}

#[test]
fn additive_intro_settles_at_the_idle_reference_pose() {
    let built = build(1, &two_bones(), &moving_root_tracks(), None);
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    let mut skeleton = parse_mdl_skeleton(&built.bytes, &mesh).unwrap();
    let mut intro = skeleton.animations[0].clone();
    intro.id = 8;
    intro.mode = PuppetAnimationMode::Single;
    intro.bone_frames[0].reverse();
    skeleton.animations.push(intro);

    let layers = [7, 8].map(|animation| PuppetLayer {
        animation,
        blend: 1.0,
        rate: 1.0,
        additive: true,
    });
    let position =
        |time| layered_bone_matrices(&skeleton, &layers, time)[0].transform_point3(Vec3::ZERO);
    assert!(position(0.0).abs_diff_eq(Vec3::new(10.0, 0.0, 0.0), 1e-6));
    assert!(position(4.0).abs_diff_eq(Vec3::ZERO, 1e-6));
    assert!(position(4.5).abs_diff_eq(Vec3::new(5.0, 0.0, 0.0), 1e-6));
}

#[test]
fn rejects_bad_parents_bone_sizes_and_frame_blocks() {
    let mut bones = two_bones();
    bones[1].parent = 1; // must be < current index
    let built = build(1, &bones, &moving_root_tracks(), None);
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    assert_eq!(
        parse_mdl_skeleton(&built.bytes, &mesh),
        Err(PuppetError::BadParent { bone: 1, parent: 1 })
    );

    let mut bones = two_bones();
    bones[0].parent = 0;
    let built = build(1, &bones, &moving_root_tracks(), None);
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    assert!(matches!(
        parse_mdl_skeleton(&built.bytes, &mesh),
        Err(PuppetError::BadParent { bone: 0, .. })
    ));

    let mut bones = two_bones();
    bones[1].size = 48;
    let built = build(1, &bones, &moving_root_tracks(), None);
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    assert_eq!(
        parse_mdl_skeleton(&built.bytes, &mesh),
        Err(PuppetError::BadBoneSize { bone: 1, size: 48 })
    );

    let built = build(1, &two_bones(), &moving_root_tracks(), Some(40));
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    assert_eq!(
        parse_mdl_skeleton(&built.bytes, &mesh),
        Err(PuppetError::BadFrameBlock {
            animation: 0,
            bone: 0,
            size: 40
        })
    );

    // No MDLS section at all.
    let built = build(1, &two_bones(), &moving_root_tracks(), None);
    let no_skeleton = &built.bytes[..built.mdls_offset];
    let mesh = parse_mdl(no_skeleton, [0.0, 0.0]).unwrap();
    assert_eq!(
        parse_mdl_skeleton(no_skeleton, &mesh),
        Err(PuppetError::MissingSection("MDLS"))
    );
}

#[test]
fn truncated_input_errors_without_panicking() {
    for version in [1, 2] {
        let built = build(version, &two_bones(), &moving_root_tracks(), None);
        let full = built.bytes.len();
        for cut in built.mdls_offset + 8..full {
            let bytes = &built.bytes[..cut];
            let Ok(mesh) = parse_mdl(bytes, [0.0, 0.0]) else {
                continue;
            };
            let result = parse_mdl_skeleton(bytes, &mesh);
            if cut < built.mdls_end {
                assert!(result.is_err(), "cut at {cut} inside MDLS should fail");
            } else if cut >= built.mdla_offset + 8 {
                assert!(result.is_err(), "cut at {cut} inside MDLA should fail");
            } else {
                // MDLA tag missing or incomplete: skeleton without animations.
                let sk = result.unwrap();
                assert!(sk.animations.is_empty());
            }
        }
        // Huge counts are rejected instead of allocating.
        let mut bytes = built.bytes.clone();
        let count_at = built.mdls_offset + 9 + 4;
        bytes[count_at..count_at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
        let mesh = parse_mdl(&bytes, [0.0, 0.0]).unwrap();
        assert!(matches!(
            parse_mdl_skeleton(&bytes, &mesh),
            Err(PuppetError::CountTooLarge { .. })
        ));
    }
}

#[test]
fn rest_pose_is_identity_and_keeps_vertices() {
    let built = build(1, &two_bones(), &moving_root_tracks(), None);
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    let sk = parse_mdl_skeleton(&built.bytes, &mesh).unwrap();
    let mut rest = PuppetAnimator::new(None);
    let bones = rest.bone_matrices(&sk, 0.5);
    assert_eq!(bones, vec![Mat4::IDENTITY; 2]);
    let verts = mesh_vertices();
    let out = PuppetDeformer::deform(&verts, &bones, &[]);
    let expected: Vec<[f32; 3]> = verts.iter().map(|v| v.position).collect();
    assert_eq!(out, expected);

    // An unknown animation id is also the rest pose.
    let mut unknown = PuppetAnimator::new(Some(99));
    assert_eq!(unknown.bone_matrices(&sk, 0.1), vec![Mat4::IDENTITY; 2]);

    // Frame 0 equals the bind pose, so t = 0 is the rest pose too.
    let mut anim = PuppetAnimator::new(Some(7));
    let at_zero = anim.bone_matrices(&sk, 0.0);
    for m in at_zero {
        assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }
}

#[test]
fn translated_keyframe_moves_fully_weighted_vertices() {
    let built = build(1, &two_bones(), &moving_root_tracks(), None);
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    let sk = parse_mdl_skeleton(&built.bytes, &mesh).unwrap();
    let mut anim = PuppetAnimator::new(Some(7));
    // Halfway between frame 0 (x = 0) and frame 1 (x = 10).
    let bones = anim.bone_matrices(&sk, 0.5);
    assert!((anim.time - 0.5).abs() < 1e-6);
    assert!(bones[0].abs_diff_eq(Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)), 1e-5));
    let out = PuppetDeformer::deform(&mesh_vertices(), &bones, &[]);
    // Root-weighted vertex moves by +5; the child bone follows its parent; zero weights stay.
    assert!(Vec3::from(out[0]).abs_diff_eq(Vec3::new(6.0, 2.0, 0.0), 1e-5));
    assert!(Vec3::from(out[1]).abs_diff_eq(Vec3::new(8.0, 4.0, 0.0), 1e-5));
    assert_eq!(out[2], [5.0, 6.0, 0.0]);

    // Rate scales time.
    let mut fast = PuppetAnimator {
        rate: 2.0,
        ..PuppetAnimator::new(Some(7))
    };
    let bones = fast.bone_matrices(&sk, 0.25);
    assert!(bones[0].abs_diff_eq(Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)), 1e-5));
}

#[test]
fn frame_interpolation_and_mirror_mapping() {
    let mapped: Vec<usize> = (0..6).map(|k| mirror_frame_index(k, 3)).collect();
    assert_eq!(mapped, [0, 1, 2, 2, 1, 0]);
    let (a, b, f) = animation_frame_pair(PuppetAnimationMode::Mirror, 3.5, 1.0, 3).unwrap();
    assert_eq!((a, b), (2, 1));
    assert!((f - 0.5).abs() < 1e-6);
    let (a, b, f) = animation_frame_pair(PuppetAnimationMode::Mirror, 5.5, 1.0, 3).unwrap();
    assert_eq!((a, b), (0, 0));
    assert!((f - 0.5).abs() < 1e-6);
    let (a, b, f) = animation_frame_pair(PuppetAnimationMode::Loop, 2.5, 1.0, 3).unwrap();
    assert_eq!((a, b), (2, 0));
    assert!((f - 0.5).abs() < 1e-6);
    let (a, b, f) = animation_frame_pair(PuppetAnimationMode::Single, 7.25, 2.0, 4).unwrap();
    assert_eq!((a, b, f), (3, 3, 0.0));
    assert!(animation_frame_pair(PuppetAnimationMode::Loop, 1.0, 0.0, 3).is_none());
    assert!(animation_frame_pair(PuppetAnimationMode::Loop, 1.0, 30.0, 0).is_none());
    assert_eq!(PuppetAnimationMode::parse(""), PuppetAnimationMode::Loop);
    assert_eq!(
        PuppetAnimationMode::parse("mirror"),
        PuppetAnimationMode::Mirror
    );
    assert_eq!(
        PuppetAnimationMode::parse("single"),
        PuppetAnimationMode::Single
    );

    // Euler order Rz * Ry * Rx.
    let q = euler_to_quat(Vec3::new(0.1, 0.2, 0.3));
    let expected = glam::Quat::from_rotation_z(0.3)
        * glam::Quat::from_rotation_y(0.2)
        * glam::Quat::from_rotation_x(0.1);
    assert!(q.abs_diff_eq(expected, 1e-6));
}

#[test]
fn morph_deltas_apply_before_skinning() {
    let verts = mesh_vertices();
    let deltas: Vec<[f32; 3]> = vec![[2.0, 0.0, 0.0], [0.0, 4.0, 0.0], [1.0, 1.0, 1.0]];
    let bones = [
        Mat4::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        Mat4::IDENTITY,
    ];
    let out = PuppetDeformer::deform(&verts, &bones, &[(0.5, &deltas)]);
    assert_eq!(out[0], [2.0, 2.0, 10.0]); // (1+1, 2, 0) then +10 z from bone 0
    assert_eq!(out[1], [3.0, 6.0, 0.0]); // bone 1 identity
    assert_eq!(out[2], [5.5, 6.5, 0.5]); // zero weights: morph only
                                         // Out-of-range bone indices are treated as identity.
    let out = PuppetDeformer::deform(&verts[1..2], &[], &[]);
    assert_eq!(out[0], [3.0, 4.0, 0.0]);
}

#[test]
fn puppet_combos_for_the_preprocessor() {
    let c = puppet_combos(12);
    assert_eq!(c.get("SKINNING"), Some(&1));
    assert_eq!(c.get("BONECOUNT"), Some(&12));
    assert_eq!(c.len(), 2);
}

#[test]
fn unskinned_mesh_gets_rest_skeleton() {
    let built = build(1, &two_bones(), &moving_root_tracks(), None);
    let no_skeleton = &built.bytes[..built.mdls_offset];
    let mesh = parse_mdl(no_skeleton, [0.0, 0.0]).unwrap();
    let sk = truongpaper_renderer::parse_mdl_skeleton_or_rest(no_skeleton, &mesh).unwrap();
    assert_eq!(sk.bones.len(), 1);
    assert!(sk.animations.is_empty());
    assert_eq!(sk.offsets.len(), 1);
    // A mesh that does have MDLS still takes the strict path.
    let mesh = parse_mdl(&built.bytes, [0.0, 0.0]).unwrap();
    let sk = truongpaper_renderer::parse_mdl_skeleton_or_rest(&built.bytes, &mesh).unwrap();
    assert_eq!(sk.bones.len(), 2);
}
