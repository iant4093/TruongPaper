//! Headless tests for the CPU particle system and vertex builders (docs/03 §5).

use glam::Vec3;
use std::mem::{offset_of, size_of};
use truongpaper_renderer::*;

#[test]
fn sprite_vertex_is_17_floats_68_bytes_with_spec_offsets() {
    assert_eq!(size_of::<SpriteParticleVertex>(), 68);
    assert_eq!(size_of::<SpriteParticleVertex>(), 17 * size_of::<f32>());
    assert_eq!(offset_of!(SpriteParticleVertex, position), 0);
    assert_eq!(offset_of!(SpriteParticleVertex, texcoord_rot_size), 12);
    assert_eq!(offset_of!(SpriteParticleVertex, color), 28);
    assert_eq!(offset_of!(SpriteParticleVertex, velocity_life), 44);
    assert_eq!(offset_of!(SpriteParticleVertex, rotation_xy), 60);
}

#[test]
fn rope_vertex_is_26_floats_104_bytes_with_spec_offsets() {
    assert_eq!(size_of::<RopeParticleVertex>(), 104);
    assert_eq!(size_of::<RopeParticleVertex>(), 26 * size_of::<f32>());
    assert_eq!(offset_of!(RopeParticleVertex, pos_start_size), 0);
    assert_eq!(offset_of!(RopeParticleVertex, pos_end_len), 16);
    assert_eq!(offset_of!(RopeParticleVertex, pos_prev_tpos), 32);
    assert_eq!(offset_of!(RopeParticleVertex, pos_after_size_end), 48);
    assert_eq!(offset_of!(RopeParticleVertex, color_end), 64);
    assert_eq!(offset_of!(RopeParticleVertex, uv), 80);
    assert_eq!(offset_of!(RopeParticleVertex, color_start), 88);
}

const SNOW_JSON: &str = r#"{
    "maxcount": 50,
    "starttime": 0,
    "material": "materials/particle/snow.json",
    "emitter": [
        {"name": "boxrandom", "rate": 0, "instantaneous": 20, "distancemax": "100 50 0", "origin": "960 540 0"}
    ],
    "initializer": [
        {"name": "lifetimerandom", "min": 100, "max": 100},
        {"name": "sizerandom", "min": 10, "max": 20},
        {"name": "colorrandom", "min": "255 0 0", "max": "255 0 0"},
        {"name": "velocityrandom", "min": "0 -10 0", "max": "0 -10 0"}
    ],
    "operator": [
        {"name": "movement", "gravity": "0 -5 0", "drag": 0},
        {"name": "alphafade", "fadeintime": 0.0, "fadeouttime": 1.0}
    ],
    "renderer": [{"name": "sprite"}],
    "controlpoint": [{"id": 1, "offset": "10 20 0", "flags": 1}]
}"#;

#[test]
fn particle_json_decodes_with_spec_defaults() {
    let f = ParticleFile::from_json_str(SNOW_JSON).unwrap();
    assert_eq!(f.maxcount, 50);
    assert_eq!(f.material, "materials/particle/snow.json");
    assert_eq!(f.emitter.len(), 1);
    assert_eq!(f.emitter[0].name, "boxrandom");
    assert_eq!(f.emitter[0].instantaneous, 20);
    assert_eq!(f.emitter[0].directions, "1 1 1"); // default
    assert_eq!(f.initializer.len(), 4);
    assert_eq!(f.operator.len(), 2);
    assert_eq!(f.renderer[0].name, "sprite");
    assert_eq!(f.renderer[0].length, 1.0); // default
    assert_eq!(f.renderer[0].maxlength, 10.0); // default
    assert_eq!(f.controlpoint[0].id, 1);
    assert_eq!(f.controlpoint[0].flags, 1);

    let minimal = ParticleFile::from_json_str(r#"{"emitter":[{"name":"sphererandom"}]}"#).unwrap();
    assert_eq!(minimal.emitter[0].rate, 10.0);
    assert!(ParticleFile::from_json_str("[]").is_err());
    assert!(ParticleFile::from_json_str(r#"{"emitter":[{}]}"#).is_err());
    assert!(ParticleFile::from_json_str("{").is_err());
}

#[test]
fn instantaneous_emitter_spawns_once_and_respects_maxcount() {
    let mut spec = ParticleFile::from_json_str(SNOW_JSON).unwrap();
    spec.maxcount = 12;
    let mut sys = ParticleSystemInstance::new(spec);
    assert!(sys.particles.is_empty());
    sys.step(0.016, Vec3::ZERO);
    assert_eq!(sys.particles.len(), 12);
    sys.step(0.016, Vec3::ZERO);
    assert_eq!(sys.particles.len(), 12);
    // dt <= 0 is a no-op.
    let t = sys.elapsed_time;
    sys.step(0.0, Vec3::ZERO);
    assert_eq!(sys.elapsed_time, t);
}

#[test]
fn initializers_and_movement_operator_apply() {
    let mut sys = ParticleSystemInstance::new(ParticleFile::from_json_str(SNOW_JSON).unwrap());
    sys.step(0.1, Vec3::ZERO);
    assert_eq!(sys.particles.len(), 20);
    for p in &sys.particles {
        assert!((10.0..=20.0).contains(&p.size));
        assert_eq!(p.color, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(p.lifetime, 100.0);
        // velocity -10 then gravity -5 * 0.1 applied once.
        assert!((p.velocity.y - (-10.5)).abs() < 1e-4);
        assert!((p.position.x - 960.0).abs() <= 100.0);
        assert!((p.position.y - 540.0 - p.velocity.y * 0.1).abs() <= 50.0 + 1e-3);
        assert_eq!(p.position.z, 0.0);
    }
}

#[test]
fn particles_expire_after_lifetime() {
    let json = r#"{"maxcount":5,"emitter":[{"name":"sphererandom","rate":0,"instantaneous":5}],
        "initializer":[{"name":"lifetimerandom","min":0.2,"max":0.2}]}"#;
    let mut sys = ParticleSystemInstance::new(ParticleFile::from_json_str(json).unwrap());
    sys.step(0.1, Vec3::ZERO);
    assert_eq!(sys.particles.len(), 5);
    sys.step(0.15, Vec3::ZERO);
    assert!(sys.particles.is_empty());
}

#[test]
fn rate_emitter_accumulates_fractional_spawns() {
    let json = r#"{"maxcount":100,"emitter":[{"name":"sphererandom","rate":10}]}"#;
    let mut sys = ParticleSystemInstance::new(ParticleFile::from_json_str(json).unwrap());
    for _ in 0..10 {
        sys.step(0.05, Vec3::ZERO); // 0.5 particles per step
    }
    assert_eq!(sys.particles.len(), 5);
}

#[test]
fn cursor_linked_control_point_follows_pointer() {
    let mut sys = ParticleSystemInstance::new(ParticleFile::from_json_str(SNOW_JSON).unwrap());
    assert_eq!(sys.control_points[1], Vec3::new(10.0, 20.0, 0.0));
    sys.step(0.016, Vec3::new(100.0, 200.0, 0.0));
    assert_eq!(sys.control_points[1], Vec3::new(110.0, 220.0, 0.0));
}

#[test]
fn starttime_prewarms_simulation() {
    let json = r#"{"maxcount":100,"starttime":1.0,"emitter":[{"name":"sphererandom","rate":10}]}"#;
    let sys = ParticleSystemInstance::new(ParticleFile::from_json_str(json).unwrap());
    assert!((sys.elapsed_time - 1.0).abs() < 1e-4);
    assert!(sys.particles.len() >= 9 && sys.particles.len() <= 10);
}

#[test]
fn sprite_builder_emits_4_vertices_6_indices_per_particle() {
    let mut sys = ParticleSystemInstance::new(ParticleFile::from_json_str(SNOW_JSON).unwrap());
    sys.step(0.016, Vec3::ZERO);
    let n = sys.particles.len();
    let (verts, idx) = sys.build_sprite_vertices();
    assert_eq!(verts.len(), n * 4);
    assert_eq!(idx.len(), n * 6);
    assert_eq!(&idx[..6], &[0, 1, 2, 0, 2, 3]);
    assert_eq!(&idx[6..12], &[4, 5, 6, 4, 6, 7]);
    assert_eq!(bytemuck::cast_slice::<_, u8>(&verts).len(), n * 4 * 68);
    let p = &sys.particles[0];
    assert_eq!(verts[0].position, p.position.to_array());
    assert_eq!(verts[0].texcoord_rot_size[3], p.size * 0.5);
    assert_eq!(
        [verts[0].texcoord_rot_size[0], verts[0].texcoord_rot_size[1]],
        [0.0, 1.0]
    );
    assert_eq!(
        [verts[2].texcoord_rot_size[0], verts[2].texcoord_rot_size[1]],
        [1.0, 0.0]
    );
    assert_eq!(verts[0].color[3], p.alpha);
}

#[test]
fn rope_builder_emits_4_vertices_6_indices_per_segment() {
    let mut sys = ParticleSystemInstance::new(ParticleFile::from_json_str(SNOW_JSON).unwrap());
    let (v0, i0) = sys.build_rope_vertices();
    assert!(v0.is_empty() && i0.is_empty());

    sys.step(0.016, Vec3::ZERO);
    let n = sys.particles.len();
    let (verts, idx) = sys.build_rope_vertices();
    assert_eq!(verts.len(), (n - 1) * 4);
    assert_eq!(idx.len(), (n - 1) * 6);
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&verts).len(),
        (n - 1) * 4 * 104
    );
    // Trail positions run from 0 at the first segment start to 1 at the last segment end.
    assert_eq!(verts[0].pos_prev_tpos[3], 0.0);
    assert_eq!(verts.last().unwrap().pos_prev_tpos[3], 1.0);
    assert_eq!(verts[0].pos_end_len[3], (n - 1) as f32);
    assert_eq!(
        &verts[0].pos_start_size[..3],
        &sys.particles[0].position.to_array()
    );
    assert_eq!(
        &verts[0].pos_end_len[..3],
        &sys.particles[1].position.to_array()
    );
}

#[test]
fn parse_vec3_str_handles_1_2_3_components() {
    assert_eq!(
        parse_vec3_str("1 2 3", Vec3::ZERO),
        Vec3::new(1.0, 2.0, 3.0)
    );
    assert_eq!(parse_vec3_str("1 2", Vec3::ZERO), Vec3::new(1.0, 2.0, 0.0));
    assert_eq!(parse_vec3_str("4", Vec3::ZERO), Vec3::splat(4.0));
    assert_eq!(parse_vec3_str("", Vec3::ONE), Vec3::ONE);
    assert_eq!(parse_vec3_str("x y", Vec3::ONE), Vec3::ONE);
}
