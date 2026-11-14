//! CPU cost of one particle step (run with `cargo test --release --test particles_bench -- --ignored --nocapture`).

use glam::Vec3;
use truongpaper_renderer::{ParticleFile, ParticleSystemInstance};

#[test]
#[ignore]
fn particle_step_cost() {
    let json = r#"{"maxcount":4000,"material":"m","renderer":[{"name":"sprite"}],
      "emitter":[{"name":"boxrandom","rate":2000,"distancemax":"500 500 0"}],
      "initializer":[{"name":"lifetimerandom","min":2,"max":3},{"name":"sizerandom","min":5,"max":10},
                     {"name":"velocityrandom","min":"-20 -20 0","max":"20 20 0"}],
      "operator":[{"name":"movement","gravity":"0 -30 0","drag":0.1},{"name":"alphafade","fadeintime":0.2,"fadeouttime":0.8},
                  {"name":"sizechange","startvalue":1,"endvalue":0.2},{"name":"colorchange","startvalue":"1 1 1","endvalue":"1 0.5 0.2"},
                  {"name":"turbulence","speedmax":40,"scale":0.01},{"name":"angularmovement","force":"0 0 1"}]}"#;
    let mut sys = ParticleSystemInstance::new(ParticleFile::from_json_str(json).unwrap());
    for _ in 0..120 {
        sys.step(1.0 / 30.0, Vec3::ZERO);
    }
    let n = 600;
    let t = std::time::Instant::now();
    for _ in 0..n {
        sys.step(1.0 / 30.0, Vec3::ZERO);
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let sum: f32 = sys
        .particles
        .iter()
        .map(|p| p.position.x + p.alpha + p.size)
        .sum();
    eprintln!(
        "PBENCH particles={} step_ms={ms:.4} checksum={sum}",
        sys.particles.len()
    );
}
