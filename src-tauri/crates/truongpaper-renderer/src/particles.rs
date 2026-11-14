//! CPU particle simulation & vertex builders (`docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` §5).
//!
//! Deviations from the doc listing:
//! - `serde`/`serde_json` are not dependencies of this crate, so the `#[derive(Serialize,
//!   Deserialize)]` + `#[serde(default = ...)]` attributes are replaced by hand-written
//!   `from_json` constructors that apply the same defaults (`rate = 10`, `directions = "1 1 1"`,
//!   `length = 1`, `maxlength = 10`, everything else `Default`), and `serde_json::Value` is
//!   replaced by `crate::json::Value`.
//! - Unused `Vec2`/`Vec4` imports were dropped.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::json::Value;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct SpriteParticleVertex {
    pub position: [f32; 3],          // offset 0..12
    pub texcoord_rot_size: [f32; 4], // offset 12..28: (u, v, rotation.z, size)
    pub color: [f32; 4],             // offset 28..44: (r, g, b, a)
    pub velocity_life: [f32; 4],     // offset 44..60: (vx, vy, vz, normalized_age)
    pub rotation_xy: [f32; 2],       // offset 60..68: sampling uv (after sprite-sheet remap)
}
const _: () = assert!(std::mem::size_of::<SpriteParticleVertex>() == 68);

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct RopeParticleVertex {
    pub pos_start_size: [f32; 4], // offset 0..16:  (start.xyz, size_start)
    pub pos_end_len: [f32; 4],    // offset 16..32: (end.xyz, trail_length)
    pub pos_prev_tpos: [f32; 4],  // offset 32..48: (prev.xyz, trail_position)
    pub pos_after_size_end: [f32; 4], // offset 48..64: (after.xyz, size_end)
    pub color_end: [f32; 4],      // offset 64..80: (r, g, b, a)
    pub uv: [f32; 2],             // offset 80..88: (uv_x, uv_y)
    pub color_start: [f32; 4],    // offset 88..104: (r, g, b, a)
}
const _: () = assert!(std::mem::size_of::<RopeParticleVertex>() == 104);

#[derive(Debug, Clone, Default)]
pub struct ParticleFile {
    pub maxcount: u32,
    pub starttime: f32,
    pub material: String,
    pub emitter: Vec<ParticleEmitterSpec>,
    pub initializer: Vec<Value>,
    pub operator: Vec<Value>,
    pub renderer: Vec<ParticleRendererSpec>,
    pub controlpoint: Vec<ParticleControlPointSpec>,
    /// Particle `flags` (bit 0: world space, particles stay put when the system moves).
    pub flags: u32,
    /// Sprite-sheet animation cycles per particle lifetime (`sequencemultiplier`).
    pub sequence_multiplier: f32,
    /// `animationmode: "randomframe"`: each particle shows one random sheet frame for its whole
    /// life instead of playing the sequence.
    pub random_frame: bool,
    /// Child systems that follow this system's particles.
    pub children: Vec<ParticleChildSpec>,
    /// The parsed file of each entry of `children` (`None` when it could not be loaded); filled
    /// by [`ParticleFile::resolve_children`].
    pub child_files: Vec<Option<std::sync::Arc<ParticleFile>>>,
}

/// How a child system is spawned from its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticleChildKind {
    /// One instance at a fixed offset from the parent system.
    Static,
    /// One instance per live parent particle, following it.
    EventFollow,
    /// A short burst at each parent particle's spawn position.
    EventSpawn,
    /// A short burst where each parent particle dies.
    EventDeath,
}

/// One entry of a particle file's `children` array.
#[derive(Debug, Clone)]
pub struct ParticleChildSpec {
    pub name: String,
    pub kind: ParticleChildKind,
    pub origin: Vec3,
    pub scale: Vec3,
    pub angles: Vec3,
    pub maxcount: u32,
    pub probability: f32,
}

impl ParticleChildSpec {
    pub fn from_json(v: &Value) -> Self {
        let kind = match json_string(v, "type", "static")
            .to_ascii_lowercase()
            .as_str()
        {
            "eventfollow" => ParticleChildKind::EventFollow,
            "eventspawn" => ParticleChildKind::EventSpawn,
            "eventdeath" => ParticleChildKind::EventDeath,
            // WE omits the key for its default kind, a single fixed instance.
            _ => ParticleChildKind::Static,
        };
        Self {
            name: json_string(v, "name", ""),
            kind,
            origin: parse_vec3_json(v.get("origin"), Vec3::ZERO),
            scale: parse_vec3_json(v.get("scale"), Vec3::ONE),
            angles: parse_vec3_json(v.get("angles"), Vec3::ZERO),
            maxcount: json_u32(v, "maxcount", 10),
            probability: json_f32(v, "probability", 1.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParticleEmitterSpec {
    pub name: String,
    pub rate: f32,
    pub instantaneous: u32,
    pub directions: String,
    pub distancemin: Value,
    pub distancemax: Value,
    pub origin: String,
    pub controlpoint: usize,
}

#[derive(Debug, Clone)]
pub struct ParticleRendererSpec {
    pub name: String,
    pub length: f32,
    pub maxlength: f32,
    pub subdivision: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ParticleControlPointSpec {
    pub id: usize,
    pub offset: String,
    pub flags: u32, // bit 0 (1) = link to mouse cursor
    pub locktopointer: bool,
}

/// Error returned when a particle JSON document cannot be decoded.
#[derive(Debug, thiserror::Error)]
pub enum ParticleSpecError {
    #[error("particle JSON parse error: {0}")]
    Json(#[from] crate::json::JsonError),
    #[error("particle JSON root must be an object")]
    NotAnObject,
    #[error("particle {0} entry is missing required string field `name`")]
    MissingName(&'static str),
}

fn json_f32(v: &Value, key: &str, default: f32) -> f32 {
    v.get(key)
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .unwrap_or(default)
}

fn json_u32(v: &Value, key: &str, default: u32) -> u32 {
    v.get(key)
        .and_then(Value::as_f64)
        .map(|n| n.max(0.0) as u32)
        .unwrap_or(default)
}

fn json_string(v: &Value, key: &str, default: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn json_array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).unwrap_or(&[])
}

impl Default for ParticleEmitterSpec {
    /// Mirrors the doc's `#[serde(default = ...)]` values (`rate = 10`, `directions = "1 1 1"`).
    fn default() -> Self {
        Self {
            name: String::new(),
            rate: 10.0,
            instantaneous: 0,
            directions: "1 1 1".into(),
            distancemin: Value::Null,
            distancemax: Value::Null,
            origin: String::new(),
            controlpoint: 0,
        }
    }
}

impl ParticleEmitterSpec {
    pub fn from_json(v: &Value) -> Result<Self, ParticleSpecError> {
        let d = Self::default();
        Ok(Self {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .ok_or(ParticleSpecError::MissingName("emitter"))?
                .to_string(),
            rate: json_f32(v, "rate", d.rate),
            instantaneous: json_u32(v, "instantaneous", d.instantaneous),
            directions: json_string(v, "directions", &d.directions),
            distancemin: v.get("distancemin").cloned().unwrap_or_default(),
            distancemax: v.get("distancemax").cloned().unwrap_or_default(),
            origin: json_string(v, "origin", ""),
            controlpoint: json_u32(v, "controlpoint", 0) as usize,
        })
    }
}

impl Default for ParticleRendererSpec {
    /// Mirrors the doc's `#[serde(default = ...)]` values (`length = 1`, `maxlength = 10`).
    fn default() -> Self {
        Self {
            name: String::new(),
            length: 1.0,
            maxlength: 10.0,
            subdivision: 0,
        }
    }
}

impl ParticleRendererSpec {
    pub fn from_json(v: &Value) -> Result<Self, ParticleSpecError> {
        let d = Self::default();
        Ok(Self {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .ok_or(ParticleSpecError::MissingName("renderer"))?
                .to_string(),
            length: json_f32(v, "length", d.length),
            maxlength: json_f32(v, "maxlength", d.maxlength),
            subdivision: json_u32(v, "subdivision", d.subdivision),
        })
    }
}

impl ParticleControlPointSpec {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: json_u32(v, "id", 0) as usize,
            offset: json_string(v, "offset", ""),
            flags: json_u32(v, "flags", 0),
            locktopointer: v
                .get("locktopointer")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

impl ParticleFile {
    /// Decodes a Wallpaper Engine `particles/*.json` document.
    pub fn from_json_str(src: &str) -> Result<Self, ParticleSpecError> {
        Self::from_json(&Value::parse(src)?)
    }

    pub fn from_json(v: &Value) -> Result<Self, ParticleSpecError> {
        if !matches!(v, Value::Object(_)) {
            return Err(ParticleSpecError::NotAnObject);
        }
        Ok(Self {
            maxcount: json_u32(v, "maxcount", 0),
            starttime: json_f32(v, "starttime", 0.0),
            material: json_string(v, "material", ""),
            emitter: json_array(v, "emitter")
                .iter()
                .map(ParticleEmitterSpec::from_json)
                .collect::<Result<_, _>>()?,
            initializer: json_array(v, "initializer").to_vec(),
            operator: json_array(v, "operator").to_vec(),
            renderer: json_array(v, "renderer")
                .iter()
                .map(ParticleRendererSpec::from_json)
                .collect::<Result<_, _>>()?,
            controlpoint: json_array(v, "controlpoint")
                .iter()
                .map(ParticleControlPointSpec::from_json)
                .collect(),
            flags: json_u32(v, "flags", 0),
            sequence_multiplier: json_f32(v, "sequencemultiplier", 1.0),
            random_frame: v
                .get("animationmode")
                .and_then(Value::as_str)
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("randomframe")),
            children: json_array(v, "children")
                .iter()
                .map(ParticleChildSpec::from_json)
                .collect(),
            child_files: Vec::new(),
        })
    }

    /// Loads and parses the file of every child system through `read` (a path -> text lookup),
    /// recursively up to a small depth. Each distinct file is parsed once.
    pub fn resolve_children(&mut self, read: &dyn Fn(&str) -> Option<String>) {
        let mut cache = std::collections::HashMap::new();
        self.resolve_children_cached(read, &mut cache, 0);
    }

    fn resolve_children_cached(
        &mut self,
        read: &dyn Fn(&str) -> Option<String>,
        cache: &mut std::collections::HashMap<String, Option<std::sync::Arc<ParticleFile>>>,
        depth: u32,
    ) {
        self.child_files.clear();
        if depth >= 4 {
            self.child_files = vec![None; self.children.len()];
            return;
        }
        for child in &self.children {
            let loaded = match cache.get(&child.name) {
                Some(hit) => hit.clone(),
                None => {
                    let parsed = read(&child.name)
                        .and_then(|t| ParticleFile::from_json_str(&t).ok())
                        .map(|mut f| {
                            f.resolve_children_cached(read, cache, depth + 1);
                            std::sync::Arc::new(f)
                        });
                    cache.insert(child.name.clone(), parsed.clone());
                    parsed
                }
            };
            self.child_files.push(loaded);
        }
    }
}

#[derive(Debug, Clone)]
pub struct SingleParticle {
    pub position: Vec3,
    pub initial_position: Vec3,
    pub velocity: Vec3,
    pub rotation: Vec3,
    pub angular_velocity: Vec3,
    pub color: Vec3,
    pub initial_color: Vec3,
    pub alpha: f32,
    pub initial_alpha: f32,
    pub size: f32,
    pub initial_size: f32,
    pub age: f32,
    pub lifetime: f32,
    pub noise_seed: f32,
    pub alive: bool,
    /// Unique within the system; child instances use it to find the particle they follow.
    pub id: u64,
}

/// Scene-object `instanceoverride` multipliers applied on top of the particle preset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleOverrides {
    pub count: f32,
    pub rate: f32,
    pub size: f32,
    pub alpha: f32,
    pub lifetime: f32,
    pub speed: f32,
    pub brightness: f32,
    /// Tint multiplied into the particle color (`colorn`).
    pub color: Vec3,
}

impl Default for ParticleOverrides {
    fn default() -> Self {
        Self {
            count: 1.0,
            rate: 1.0,
            size: 1.0,
            alpha: 1.0,
            lifetime: 1.0,
            speed: 1.0,
            brightness: 1.0,
            color: Vec3::ONE,
        }
    }
}

pub struct ParticleSystemInstance {
    pub overrides: ParticleOverrides,
    pub spec: ParticleFile,
    pub particles: Vec<SingleParticle>,
    pub emit_accumulators: Vec<f32>,
    pub control_points: [Vec3; 8],
    pub rng_state: u64,
    pub elapsed_time: f32,
    pub spawned_instantaneous: bool,
    /// Offset of this system's origin from the layer origin (non-zero for child systems).
    pub anchor: Vec3,
    /// World-space systems keep particles where they were emitted when `anchor` moves.
    pub worldspace: bool,
    /// Scale applied to particle sizes when drawn (a child's `scale`).
    pub draw_scale: f32,
    /// `false` stops spawning (the system drains and can then be dropped).
    pub emitting: bool,
    /// Elapsed time after which emission stops (burst children).
    pub emit_until: Option<f32>,
    pub(crate) next_id: u64,
    /// Layer-space positions of particles spawned / dead during the last step.
    pub(crate) spawn_events: Vec<Vec3>,
    pub(crate) death_events: Vec<Vec3>,
    pub(crate) groups: Vec<crate::particle_tree::ChildGroup>,
    /// Free child-instance slots shared by the whole particle tree.
    pub(crate) budget: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl ParticleSystemInstance {
    pub fn new(spec: ParticleFile) -> Self {
        let mut inst = Self::new_unwarmed(spec);
        inst.prewarm();
        inst
    }

    /// Runs the `starttime` warm-up (at most 10 s).
    pub fn prewarm(&mut self) {
        let prewarm = self.spec.starttime.min(10.0);
        if prewarm > 0.0 {
            let steps = (prewarm / 0.05).ceil() as usize;
            for _ in 0..steps {
                self.step(0.05, Vec3::ZERO);
            }
        }
    }

    pub fn new_unwarmed(spec: ParticleFile) -> Self {
        let max_count = spec.maxcount.clamp(1, 10_000) as usize;
        let mut control_points = [Vec3::ZERO; 8];
        for cp in &spec.controlpoint {
            if cp.id < 8 {
                control_points[cp.id] = parse_vec3_str(&cp.offset, Vec3::ZERO);
            }
        }
        let inst = Self {
            worldspace: spec.flags & 1 != 0,
            anchor: Vec3::ZERO,
            draw_scale: 1.0,
            emitting: true,
            emit_until: None,
            next_id: 0,
            spawn_events: Vec::new(),
            death_events: Vec::new(),
            groups: Vec::new(),
            budget: crate::particle_tree::new_budget(),
            emit_accumulators: vec![0.0; spec.emitter.len()],
            particles: Vec::with_capacity(max_count),
            control_points,
            rng_state: 0x853C_49E6_748F_EA9B,
            elapsed_time: 0.0,
            spawned_instantaneous: false,
            overrides: ParticleOverrides::default(),
            spec,
        };
        let mut inst = inst;
        inst.groups = crate::particle_tree::ChildGroup::build(&inst.spec);
        inst
    }

    /// Replaces the instance overrides. Particles already alive keep their values; new ones use
    /// the new multipliers (so a property change shows as the old particles die off).
    pub fn set_overrides(&mut self, overrides: ParticleOverrides) {
        self.overrides = overrides;
    }

    /// Particle cap after the `count` override.
    fn max_count(&self) -> usize {
        let base = self.spec.maxcount.clamp(1, 10_000) as f32;
        // Never above the GPU buffer capacity sized from the preset's own `maxcount`.
        (base * self.overrides.count.max(0.0))
            .round()
            .clamp(0.0, base) as usize
    }

    #[inline]
    fn rand01(&mut self) -> f32 {
        let old = self.rng_state;
        self.rng_state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        let r = xorshifted.rotate_right(rot);
        (r as f32) / (u32::MAX as f32)
    }

    #[inline]
    fn rand_range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.rand01()
    }

    /// Steps this system, then its child systems.
    pub fn step(&mut self, dt: f32, cursor_layer_pos: Vec3) {
        if dt <= 0.0 {
            return;
        }
        self.step_self(dt, cursor_layer_pos);
        self.step_children(dt, cursor_layer_pos);
    }

    fn step_self(&mut self, dt: f32, cursor_layer_pos: Vec3) {
        self.elapsed_time += dt;
        if self.emit_until.is_some_and(|t| self.elapsed_time > t) {
            self.emitting = false;
        }

        // 1. Update cursor-bound control points
        for cp in &self.spec.controlpoint {
            if cp.id < 8 && (cp.locktopointer || (cp.flags & 1) != 0) {
                self.control_points[cp.id] =
                    cursor_layer_pos + parse_vec3_str(&cp.offset, Vec3::ZERO);
            }
        }

        // 2. Spawn new particles from emitters
        let max_count = self.max_count();
        for (e_idx, emitter) in self.spec.emitter.clone().iter().enumerate() {
            let mut to_spawn = 0usize;
            if !self.spawned_instantaneous && emitter.instantaneous > 0 {
                to_spawn += emitter.instantaneous as usize;
            }
            self.emit_accumulators[e_idx] += emitter.rate * self.overrides.rate.max(0.0) * dt;
            let periodic = self.emit_accumulators[e_idx].floor() as usize;
            self.emit_accumulators[e_idx] -= periodic as f32;
            to_spawn += periodic;

            if !self.emitting {
                continue;
            }
            for _ in 0..to_spawn {
                if self.particles.len() >= max_count {
                    break;
                }
                let mut p = self.spawn_one_particle(emitter);
                if self.worldspace {
                    p.position += self.anchor;
                    p.initial_position += self.anchor;
                }
                self.next_id += 1;
                p.id = self.next_id;
                self.spawn_events.push(if self.worldspace {
                    p.position
                } else {
                    self.anchor + p.position
                });
                self.particles.push(p);
            }
        }
        self.spawned_instantaneous = true;

        // 3. Run operators and integrate active particles
        let operators = self.spec.operator.clone();
        let cps = self.control_points;
        let time = self.elapsed_time;

        let (anchor, worldspace) = (self.anchor, self.worldspace);
        let mut deaths = Vec::new();
        self.particles.retain_mut(|p| {
            p.age += dt;
            if p.age >= p.lifetime {
                deaths.push(if worldspace {
                    p.position
                } else {
                    anchor + p.position
                });
                return false;
            }
            let life_frac = (p.age / p.lifetime.max(0.0001)).clamp(0.0, 1.0);

            for op in &operators {
                let op_name = op.get("name").and_then(|v| v.as_str()).unwrap_or("");
                match op_name {
                    "movement" => {
                        let gravity = parse_vec3_json(op.get("gravity"), Vec3::ZERO);
                        let drag = op.get("drag").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        p.velocity += gravity * dt;
                        p.velocity *= (1.0 - drag * dt).max(0.0);
                        p.position += p.velocity * dt;
                    }
                    "angularmovement" => {
                        let drag = op.get("drag").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let force = parse_vec3_json(op.get("force"), Vec3::ZERO);
                        p.angular_velocity += force * dt;
                        p.angular_velocity *= (1.0 - drag * dt).max(0.0);
                        p.rotation += p.angular_velocity * dt;
                    }
                    "alphafade" => {
                        // Wallpaper Engine's defaults are 0.5 / 0.5 (linux-wallpaperengine's
                        // ObjectParser agrees): fade in over the first half, out over the second.
                        let fade_in =
                            op.get("fadeintime").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32;
                        let fade_out = op
                            .get("fadeouttime")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.5) as f32;
                        let factor = if life_frac < fade_in && fade_in > 0.0 {
                            life_frac / fade_in
                        } else if life_frac > fade_out && fade_out < 1.0 {
                            1.0 - (life_frac - fade_out) / (1.0 - fade_out)
                        } else {
                            1.0
                        };
                        p.alpha = p.initial_alpha * factor.clamp(0.0, 1.0);
                    }
                    "sizechange" => {
                        let st = op.get("starttime").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let et = op.get("endtime").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let sv =
                            op.get("startvalue").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let ev = op.get("endvalue").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let t = ((life_frac - st) / (et - st).max(0.0001)).clamp(0.0, 1.0);
                        p.size = p.initial_size * (sv + (ev - sv) * t);
                    }
                    "colorchange" => {
                        let st = op.get("starttime").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let et = op.get("endtime").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let sv = parse_vec3_json(op.get("startvalue"), Vec3::ONE);
                        let ev = parse_vec3_json(op.get("endvalue"), Vec3::ONE);
                        let t = ((life_frac - st) / (et - st).max(0.0001)).clamp(0.0, 1.0);
                        p.color = p.initial_color * sv.lerp(ev, t);
                    }
                    "oscillateposition" => {
                        let freq = op
                            .get("frequencymax")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0) as f32;
                        let scale =
                            op.get("scalemax").and_then(|v| v.as_f64()).unwrap_or(5.0) as f32;
                        let mask = parse_vec3_json(op.get("mask"), Vec3::new(1.0, 1.0, 0.0));
                        let wave =
                            (time * freq + p.noise_seed * std::f32::consts::TAU).sin() * scale;
                        p.position += mask * wave * dt;
                    }
                    "turbulence" => {
                        let speed =
                            op.get("speedmax").and_then(|v| v.as_f64()).unwrap_or(50.0) as f32;
                        let scale = op.get("scale").and_then(|v| v.as_f64()).unwrap_or(0.01) as f32;
                        let mask = parse_vec3_json(op.get("mask"), Vec3::new(1.0, 1.0, 0.0));
                        let nx = ((p.position.y * scale + time + p.noise_seed).sin()
                            + (p.position.x * scale * 0.7 - time).cos())
                            * 0.5;
                        let ny = ((p.position.x * scale + time * 1.3 + p.noise_seed).cos()
                            - (p.position.y * scale * 0.7 + time).sin())
                            * 0.5;
                        p.velocity += Vec3::new(nx, ny, 0.0) * mask * speed * dt;
                    }
                    "vortex" => {
                        let cp_id =
                            op.get("controlpoint").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let center = cps.get(cp_id).copied().unwrap_or(Vec3::ZERO)
                            + parse_vec3_json(op.get("offset"), Vec3::ZERO);
                        let axis = parse_vec3_json(op.get("axis"), Vec3::Z).normalize_or_zero();
                        let r_vec = p.position - center;
                        let dist = r_vec.length().max(0.001);
                        let inner = op
                            .get("distanceinner")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0) as f32;
                        let outer = op
                            .get("distanceouter")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(250.0) as f32;
                        let s_in = op
                            .get("speedinner")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(100.0) as f32;
                        let s_out =
                            op.get("speedouter").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let t = ((dist - inner) / (outer - inner).max(0.001)).clamp(0.0, 1.0);
                        let speed = s_in + (s_out - s_in) * t;
                        let tangent = axis.cross(r_vec / dist).normalize_or_zero();
                        p.velocity += tangent * speed * dt;
                    }
                    "controlpointattract" => {
                        let cp_id =
                            op.get("controlpoint").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let center = cps.get(cp_id).copied().unwrap_or(Vec3::ZERO)
                            + parse_vec3_json(op.get("origin"), Vec3::ZERO);
                        let scale =
                            op.get("scale").and_then(|v| v.as_f64()).unwrap_or(200.0) as f32;
                        let threshold = op
                            .get("threshold")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(512.0) as f32;
                        let to_cp = center - p.position;
                        let dist = to_cp.length();
                        if dist > 0.001 && dist < threshold {
                            let falloff = 1.0 - (dist / threshold);
                            p.velocity += (to_cp / dist) * scale * falloff * dt;
                        }
                    }
                    _ => {}
                }
            }
            // A NaN / inf from a bad expression would corrupt the vertex buffer.
            p.position.is_finite()
                && p.velocity.is_finite()
                && p.size.is_finite()
                && p.alpha.is_finite()
                && p.rotation.is_finite()
        });
        self.death_events = deaths;
    }

    fn spawn_one_particle(&mut self, emitter: &ParticleEmitterSpec) -> SingleParticle {
        let cp_pos = self
            .control_points
            .get(emitter.controlpoint)
            .copied()
            .unwrap_or(Vec3::ZERO);
        let origin = cp_pos + parse_vec3_str(&emitter.origin, Vec3::ZERO);
        let dirs = parse_vec3_str(&emitter.directions, Vec3::ONE);

        let pos = if emitter.name == "boxrandom" {
            let dmax = parse_vec3_json(Some(&emitter.distancemax), Vec3::splat(100.0));
            let rx = self.rand_range(-dmax.x, dmax.x);
            let ry = self.rand_range(-dmax.y, dmax.y);
            let rz = self.rand_range(-dmax.z, dmax.z);
            origin + Vec3::new(rx * dirs.x, ry * dirs.y, rz * dirs.z)
        } else {
            let r_min = emitter.distancemin.as_f64().unwrap_or(0.0) as f32;
            let r_max = emitter.distancemax.as_f64().unwrap_or(100.0) as f32;
            let radius = self.rand_range(r_min, r_max);
            let theta = self.rand_range(0.0, std::f32::consts::TAU);
            let phi = self.rand_range(-1.0, 1.0).acos();
            let dir = Vec3::new(
                phi.sin() * theta.cos() * dirs.x,
                phi.sin() * theta.sin() * dirs.y,
                phi.cos() * dirs.z,
            );
            origin + dir * radius
        };

        let mut p = SingleParticle {
            position: pos,
            initial_position: pos,
            velocity: Vec3::ZERO,
            rotation: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            color: Vec3::ONE,
            initial_color: Vec3::ONE,
            alpha: 1.0,
            initial_alpha: 1.0,
            size: 20.0,
            initial_size: 20.0,
            age: 0.0,
            lifetime: 5.0,
            noise_seed: self.rand01(),
            alive: true,
            id: 0,
        };

        for init in &self.spec.initializer.clone() {
            let name = init.get("name").and_then(|v| v.as_str()).unwrap_or("");
            match name {
                "lifetimerandom" => {
                    let min = init.get("min").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                    let max = init.get("max").and_then(|v| v.as_f64()).unwrap_or(5.0) as f32;
                    p.lifetime = self.rand_range(min, max).max(0.05);
                }
                "sizerandom" => {
                    let min = init.get("min").and_then(|v| v.as_f64()).unwrap_or(10.0) as f32;
                    let max = init.get("max").and_then(|v| v.as_f64()).unwrap_or(30.0) as f32;
                    p.size = self.rand_range(min, max);
                    p.initial_size = p.size;
                }
                "alpharandom" => {
                    let min = init.get("min").and_then(|v| v.as_f64()).unwrap_or(0.5) as f32;
                    let max = init.get("max").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                    p.alpha = self.rand_range(min, max);
                    p.initial_alpha = p.alpha;
                }
                "colorrandom" => {
                    let cmin = parse_vec3_json(init.get("min"), Vec3::ZERO) / 255.0;
                    let cmax = parse_vec3_json(init.get("max"), Vec3::splat(255.0)) / 255.0;
                    let t = self.rand01();
                    p.color = cmin.lerp(cmax, t);
                    p.initial_color = p.color;
                }
                "velocityrandom" => {
                    let vmin = parse_vec3_json(init.get("min"), Vec3::splat(-20.0));
                    let vmax = parse_vec3_json(init.get("max"), Vec3::splat(20.0));
                    p.velocity = Vec3::new(
                        self.rand_range(vmin.x, vmax.x),
                        self.rand_range(vmin.y, vmax.y),
                        self.rand_range(vmin.z, vmax.z),
                    );
                }
                "rotationrandom" => {
                    let rmin = parse_vec3_json(init.get("min"), Vec3::ZERO);
                    let rmax = parse_vec3_json(
                        init.get("max"),
                        Vec3::new(0.0, 0.0, std::f32::consts::TAU),
                    );
                    p.rotation = Vec3::new(
                        self.rand_range(rmin.x, rmax.x),
                        self.rand_range(rmin.y, rmax.y),
                        self.rand_range(rmin.z, rmax.z),
                    );
                }
                "angularvelocityrandom" => {
                    let amin = parse_vec3_json(init.get("min"), Vec3::ZERO);
                    let amax = parse_vec3_json(init.get("max"), Vec3::new(0.0, 0.0, 1.0));
                    p.angular_velocity = Vec3::new(
                        self.rand_range(amin.x, amax.x),
                        self.rand_range(amin.y, amax.y),
                        self.rand_range(amin.z, amax.z),
                    );
                }
                _ => {}
            }
        }

        let ov = self.overrides;
        p.lifetime = (p.lifetime * ov.lifetime).max(0.05);
        p.size *= ov.size;
        p.initial_size *= ov.size;
        p.alpha *= ov.alpha;
        p.initial_alpha *= ov.alpha;
        p.velocity *= ov.speed;
        p.color *= ov.color * ov.brightness;
        p.initial_color *= ov.color * ov.brightness;
        p
    }

    /// Emits 4 vertices (68 bytes each) and 6 indices per alive particle for `genericparticle.vert`.
    pub fn build_sprite_vertices(&self) -> (Vec<SpriteParticleVertex>, Vec<u16>) {
        let count = self.particles.len().min((u16::MAX as usize) / 4);
        let mut verts = Vec::with_capacity(count * 4);
        let mut indices = Vec::with_capacity(count * 6);

        let uvs = [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

        for (i, p) in self.particles.iter().take(count).enumerate() {
            let base = (i * 4) as u16;
            let norm_age = (p.age / p.lifetime.max(0.0001)).clamp(0.0, 1.0);
            for uv in uvs {
                verts.push(SpriteParticleVertex {
                    position: p.position.to_array(),
                    texcoord_rot_size: [uv[0], uv[1], p.rotation.z, p.size * 0.5],
                    color: [p.color.x, p.color.y, p.color.z, p.alpha],
                    velocity_life: [p.velocity.x, p.velocity.y, p.velocity.z, norm_age],
                    rotation_xy: uv,
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        (verts, indices)
    }

    /// Emits 4 vertices (104 bytes each) and 6 indices per segment for `genericropeparticle.vert`.
    pub fn build_rope_vertices(&self) -> (Vec<RopeParticleVertex>, Vec<u16>) {
        if self.particles.len() < 2 {
            return (Vec::new(), Vec::new());
        }
        let seg_count = (self.particles.len() - 1).min((u16::MAX as usize) / 4);
        let mut verts = Vec::with_capacity(seg_count * 4);
        let mut indices = Vec::with_capacity(seg_count * 6);
        let total = seg_count as f32;

        for i in 0..seg_count {
            let p_start = &self.particles[i];
            let p_end = &self.particles[i + 1];
            let p_prev = if i > 0 {
                &self.particles[i - 1]
            } else {
                p_start
            };
            let p_after = if i + 2 < self.particles.len() {
                &self.particles[i + 2]
            } else {
                p_end
            };

            let base = (i * 4) as u16;
            let t0 = (i as f32) / total;
            let t1 = ((i + 1) as f32) / total;
            let corner_uvs = [[0.0f32, t0], [1.0, t0], [1.0, t1], [0.0, t1]];

            for (c_idx, uv) in corner_uvs.into_iter().enumerate() {
                let trail_pos = if c_idx < 2 { t0 } else { t1 };
                verts.push(RopeParticleVertex {
                    pos_start_size: [
                        p_start.position.x,
                        p_start.position.y,
                        p_start.position.z,
                        p_start.size * 0.5,
                    ],
                    pos_end_len: [p_end.position.x, p_end.position.y, p_end.position.z, total],
                    pos_prev_tpos: [
                        p_prev.position.x,
                        p_prev.position.y,
                        p_prev.position.z,
                        trail_pos,
                    ],
                    pos_after_size_end: [
                        p_after.position.x,
                        p_after.position.y,
                        p_after.position.z,
                        p_end.size * 0.5,
                    ],
                    color_end: [p_end.color.x, p_end.color.y, p_end.color.z, p_end.alpha],
                    uv,
                    color_start: [
                        p_start.color.x,
                        p_start.color.y,
                        p_start.color.z,
                        p_start.alpha,
                    ],
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        (verts, indices)
    }
}

pub fn parse_vec3_str(s: &str, default: Vec3) -> Vec3 {
    let mut it = s.split_whitespace().filter_map(|p| p.parse::<f32>().ok());
    match (it.next(), it.next(), it.next()) {
        (Some(x), Some(y), Some(z)) => Vec3::new(x, y, z),
        (Some(x), Some(y), None) => Vec3::new(x, y, 0.0),
        (Some(x), None, None) => Vec3::splat(x),
        _ => default,
    }
}

fn parse_vec3_json(val: Option<&Value>, default: Vec3) -> Vec3 {
    match val {
        Some(Value::String(s)) => parse_vec3_str(s, default),
        Some(Value::Number(n)) => Vec3::splat(*n as f32),
        _ => default,
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;

    fn system() -> ParticleSystemInstance {
        let spec = ParticleFile::from_json_str(
            r#"{ "maxcount": 100, "emitter": [ { "name": "boxrandom", "rate": 100,
                 "distancemax": "10 10 0" } ],
                 "initializer": [ { "name": "sizerandom", "min": 10, "max": 10 },
                                  { "name": "alpharandom", "min": 1, "max": 1 } ] }"#,
        )
        .unwrap();
        ParticleSystemInstance::new(spec)
    }

    #[test]
    fn instance_overrides_scale_count_size_alpha_and_color() {
        let mut base = system();
        base.step(1.0, Vec3::ZERO);
        assert_eq!(base.particles.len(), 100);

        let mut ov = system();
        ov.set_overrides(ParticleOverrides {
            count: 0.5,
            size: 2.0,
            alpha: 0.5,
            color: Vec3::new(1.0, 0.0, 0.0),
            ..ParticleOverrides::default()
        });
        ov.step(1.0, Vec3::ZERO);
        assert_eq!(ov.particles.len(), 50);
        let p = &ov.particles[0];
        assert_eq!((p.size, p.alpha), (20.0, 0.5));
        assert_eq!(p.color, Vec3::new(1.0, 0.0, 0.0));

        let mut slow = system();
        slow.set_overrides(ParticleOverrides {
            rate: 0.1,
            ..ParticleOverrides::default()
        });
        slow.step(1.0, Vec3::ZERO);
        assert_eq!(slow.particles.len(), 10);
    }
}
