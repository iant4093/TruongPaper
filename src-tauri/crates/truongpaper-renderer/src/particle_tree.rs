//! Child particle systems (`children` in a Wallpaper Engine particle file) and the geometry
//! builder that flattens a system and all its children into per-material draw sets.
//!
//! A child system is a separate particle file driven by the parent's particles:
//! - `static`: one instance at a fixed offset from the parent system;
//! - `eventfollow`: one instance per live parent particle, anchored to it (a glow, trail or
//!   body following a single moving emitter particle);
//! - `eventspawn` / `eventdeath`: a short burst where a parent particle spawns / dies.
//!
//! Approximations: a child's `angles` and `probability` are ignored, `scale` is a uniform
//! multiplier on the drawn size, and burst children emit for 0.1 s.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use glam::Vec3;

use crate::particles::{
    ParticleChildKind, ParticleChildSpec, ParticleFile, ParticleSystemInstance, SingleParticle,
    SpriteParticleVertex,
};
use crate::sprite::SpriteSheet;

/// How long a burst (`eventspawn` / `eventdeath`) child keeps emitting, in seconds.
const BURST_SECS: f32 = 0.1;
/// Largest quad count per draw set (16-bit indices, 4 vertices per quad).
const MAX_SET_QUADS: usize = 16_000;

/// Most live child instances in one particle tree (all depths together).
pub(crate) const MAX_TREE_INSTANCES: usize = 2048;
/// Most child instances one `children` entry may hold, whatever its `maxcount` says.
const MAX_GROUP_INSTANCES: usize = 1024;
/// Set when the draw-set quad cap dropped geometry (logged once per process).
static CAP_LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Shared counter of free child-instance slots for one particle tree.
pub(crate) fn new_budget() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(MAX_TREE_INSTANCES))
}

/// One taken slot of the tree budget; the slot is returned when the instance is dropped.
struct BudgetSlot(Arc<AtomicUsize>);

impl BudgetSlot {
    fn acquire(budget: &Arc<AtomicUsize>) -> Option<BudgetSlot> {
        budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
            .ok()
            .map(|_| BudgetSlot(budget.clone()))
    }
}

impl Drop for BudgetSlot {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

/// The instances of one entry of a particle file's `children`.
pub struct ChildGroup {
    spec: ParticleChildSpec,
    file: Option<Arc<ParticleFile>>,
    instances: Vec<ChildInstance>,
}

struct ChildInstance {
    /// Id of the followed parent particle (0 for static / burst children).
    parent_id: u64,
    inst: ParticleSystemInstance,
    /// The followed parent particle died: stop emitting and drop once drained.
    orphan: bool,
    _slot: BudgetSlot,
}

impl ChildGroup {
    pub(crate) fn build(spec: &ParticleFile) -> Vec<ChildGroup> {
        spec.children
            .iter()
            .enumerate()
            .map(|(i, c)| ChildGroup {
                spec: c.clone(),
                file: spec.child_files.get(i).cloned().flatten(),
                instances: Vec::new(),
            })
            .collect()
    }
}

impl ParticleSystemInstance {
    fn abs_position(&self, p: &SingleParticle) -> Vec3 {
        if self.worldspace {
            p.position
        } else {
            self.anchor + p.position
        }
    }

    fn make_child(
        &self,
        spec: &ParticleChildSpec,
        file: &Arc<ParticleFile>,
        anchor: Vec3,
        burst: bool,
    ) -> ParticleSystemInstance {
        let mut inst = ParticleSystemInstance::new_unwarmed((**file).clone());
        inst.anchor = anchor;
        inst.draw_scale = self.draw_scale * spec.scale.x;
        inst.overrides = self.overrides;
        inst.budget = self.budget.clone();
        if burst {
            inst.emit_until = Some(BURST_SECS);
        } else {
            inst.prewarm();
        }
        inst
    }

    /// Spawns, moves and steps the child systems after this system's own step.
    pub(crate) fn step_children(&mut self, dt: f32, cursor: Vec3) {
        if self.groups.is_empty() {
            return;
        }
        let spawns = std::mem::take(&mut self.spawn_events);
        let deaths = std::mem::take(&mut self.death_events);
        let mut groups = std::mem::take(&mut self.groups);
        for g in &mut groups {
            let Some(file) = g.file.clone() else {
                continue;
            };
            let spec = g.spec.clone();
            let cap = (spec.maxcount.max(1) as usize).min(MAX_GROUP_INSTANCES);
            match spec.kind {
                ParticleChildKind::Static => {
                    if g.instances.is_empty() {
                        if let Some(slot) = BudgetSlot::acquire(&self.budget) {
                            let inst =
                                self.make_child(&spec, &file, self.anchor + spec.origin, false);
                            g.instances.push(ChildInstance {
                                parent_id: 0,
                                inst,
                                orphan: false,
                                _slot: slot,
                            });
                        }
                    }
                    if let Some(first) = g.instances.first_mut() {
                        first.inst.anchor = self.anchor + spec.origin;
                    }
                }
                ParticleChildKind::EventFollow => {
                    // Particle ids grow with spawn order and `retain` keeps the order, so the
                    // parent is found by binary search.
                    for ci in &mut g.instances {
                        let found = self
                            .particles
                            .binary_search_by_key(&ci.parent_id, |p| p.id)
                            .ok()
                            .map(|i| &self.particles[i]);
                        match found {
                            Some(p) => {
                                let pos = if self.worldspace {
                                    p.position
                                } else {
                                    self.anchor + p.position
                                };
                                ci.inst.anchor = pos + spec.origin;
                            }
                            None => {
                                ci.orphan = true;
                                ci.inst.emitting = false;
                            }
                        }
                    }
                    let followed: HashSet<u64> = g.instances.iter().map(|c| c.parent_id).collect();
                    for p in &self.particles {
                        if g.instances.len() >= cap {
                            break;
                        }
                        if followed.contains(&p.id) {
                            continue;
                        }
                        let Some(slot) = BudgetSlot::acquire(&self.budget) else {
                            break;
                        };
                        let pos = self.abs_position(p) + spec.origin;
                        let inst = self.make_child(&spec, &file, pos, false);
                        g.instances.push(ChildInstance {
                            parent_id: p.id,
                            inst,
                            orphan: false,
                            _slot: slot,
                        });
                    }
                }
                ParticleChildKind::EventSpawn | ParticleChildKind::EventDeath => {
                    let events = if spec.kind == ParticleChildKind::EventSpawn {
                        &spawns
                    } else {
                        &deaths
                    };
                    for pos in events {
                        if g.instances.len() >= cap {
                            break;
                        }
                        let Some(slot) = BudgetSlot::acquire(&self.budget) else {
                            break;
                        };
                        let inst = self.make_child(&spec, &file, *pos + spec.origin, true);
                        g.instances.push(ChildInstance {
                            parent_id: 0,
                            inst,
                            orphan: false,
                            _slot: slot,
                        });
                    }
                }
            }
            let overrides = self.overrides;
            for ci in &mut g.instances {
                ci.inst.overrides = overrides;
                ci.inst.step(dt, cursor);
            }
            g.instances
                .retain(|ci| !((ci.orphan || !ci.inst.emitting) && ci.inst.is_drained()));
        }
        self.groups = groups;
    }

    /// No live particles and no live child instances.
    fn is_drained(&self) -> bool {
        self.particles.is_empty() && self.groups.iter().all(|g| g.instances.is_empty())
    }

    /// Appends this system's geometry, and its children's, to the matching sets of `out`.
    ///
    /// `sheets` holds the sprite-sheet of each plan entry's texture, if animated.
    pub fn gather_geometry(
        &self,
        plan: &[DrawSetPlan],
        sheets: &[Option<Arc<SpriteSheet>>],
        out: &mut [SetGeometry],
    ) {
        if let Some(rope) = draw_kind(&self.spec) {
            let material = self.spec.material.as_str();
            if let Some(i) = plan
                .iter()
                .position(|s| s.rope == rope && s.material == material)
            {
                if let Some(geo) = out.get_mut(i) {
                    if rope {
                        self.push_rope(geo);
                    } else {
                        self.push_sprites(geo, sheets.get(i).and_then(|s| s.as_deref()));
                    }
                }
            }
        }
        for g in &self.groups {
            for ci in &g.instances {
                ci.inst.gather_geometry(plan, sheets, out);
            }
        }
    }

    fn push_sprites(&self, geo: &mut SetGeometry, sheet: Option<&SpriteSheet>) {
        const UVS: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        for p in &self.particles {
            if geo.vertices.len() + 4 > u16::MAX as usize {
                break;
            }
            let base = geo.vertices.len() as u16;
            let pos = self.abs_position(p).to_array();
            let norm_age = (p.age / p.lifetime.max(0.0001)).clamp(0.0, 1.0);
            // Sprite sheets play through their frames `sequencemultiplier` times per lifetime.
            let frame = sheet.map(|s| {
                let n = s.frame_count().max(1);
                if self.spec.random_frame {
                    (random_frame_index(p.id, n), s)
                } else {
                    let t = (norm_age * self.spec.sequence_multiplier).rem_euclid(1.0);
                    (((t * n as f32) as usize).min(n - 1), s)
                }
            });
            for uv in UVS {
                let corner = uv;
                let uv = match frame {
                    Some((idx, s)) => s.map_uv(idx, uv),
                    None => uv,
                };
                geo.vertices.push(SpriteParticleVertex {
                    position: pos,
                    texcoord_rot_size: [
                        corner[0],
                        corner[1],
                        p.rotation.z,
                        p.size * 0.5 * self.draw_scale,
                    ],
                    color: [p.color.x, p.color.y, p.color.z, p.alpha],
                    velocity_life: [p.velocity.x, p.velocity.y, p.velocity.z, norm_age],
                    rotation_xy: uv,
                });
            }
            if !geo.skip_sprite_indices {
                geo.indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base,
                    base + 2,
                    base + 3,
                ]);
            }
        }
    }

    /// A ribbon through the particles in spawn order, as quads with the corner baked into the
    /// vertex position (the sprite shader adds a zero-size offset). `v` runs along the rope.
    fn push_rope(&self, geo: &mut SetGeometry) {
        let n = self.particles.len();
        if n < 2 {
            return;
        }
        let pt = |i: usize| self.abs_position(&self.particles[i]);
        let first = geo.vertices.len();
        for (i, p) in self.particles.iter().enumerate() {
            if geo.vertices.len() + 2 > u16::MAX as usize {
                break;
            }
            let prev = pt(i.saturating_sub(1));
            let next = pt((i + 1).min(n - 1));
            let dir = (next - prev).truncate().normalize_or_zero();
            let normal = Vec3::new(-dir.y, dir.x, 0.0);
            let half = p.size * 0.5 * self.draw_scale;
            let v = i as f32 / (n - 1) as f32;
            let norm_age = (p.age / p.lifetime.max(0.0001)).clamp(0.0, 1.0);
            for (side, u) in [(1.0f32, 0.0f32), (-1.0, 1.0)] {
                geo.vertices.push(SpriteParticleVertex {
                    position: (pt(i) + normal * half * side).to_array(),
                    texcoord_rot_size: [u, v, 0.0, 0.0],
                    color: [p.color.x, p.color.y, p.color.z, p.alpha],
                    velocity_life: [p.velocity.x, p.velocity.y, p.velocity.z, norm_age],
                    rotation_xy: [u, v],
                });
            }
        }
        let written = (geo.vertices.len() - first) / 2;
        let start = first as u16;
        for i in 0..written.saturating_sub(1) {
            let a = start + (2 * i) as u16;
            geo.indices
                .extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
        }
    }
}

/// `Some(true)` for a rope renderer, `Some(false)` for sprites, `None` when the file draws
/// nothing itself (no renderer, only children).
pub fn draw_kind(file: &ParticleFile) -> Option<bool> {
    match file.renderer.first() {
        Some(r) => Some(matches!(
            r.name.to_ascii_lowercase().as_str(),
            "rope" | "ropetrail"
        )),
        // Stock presets without renderer and without children keep drawing as sprites.
        None if file.children.is_empty() => Some(false),
        None => None,
    }
}

/// One GPU draw set: all systems sharing a material and renderer kind.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawSetPlan {
    pub material: String,
    pub rope: bool,
    /// Upper bound of quads drawn at once.
    pub capacity: usize,
}

/// Vertices and indices for one draw set, built each frame.
#[derive(Debug, Default, Clone)]
pub struct SetGeometry {
    pub vertices: Vec<SpriteParticleVertex>,
    pub indices: Vec<u16>,
    /// The caller draws sprites with a fixed quad index pattern, so `push_sprites` leaves
    /// `indices` empty.
    pub skip_sprite_indices: bool,
}

impl SetGeometry {
    /// Cuts the geometry to at most `capacity` quads, keeping only whole triangles whose
    /// vertices survive.
    pub fn fit(&mut self, capacity: usize) {
        if (self.vertices.len() > capacity * 4 || self.indices.len() > capacity * 6)
            && !CAP_LOGGED.swap(true, Ordering::Relaxed)
        {
            tracing::warn!("particle draw set exceeds {capacity} quads; extra particles dropped");
        }
        self.vertices.truncate(capacity * 4);
        self.indices.truncate(capacity * 6);
        let n = self.vertices.len();
        let keep = self
            .indices
            .chunks_exact(6)
            .take_while(|c| c.iter().all(|&i| (i as usize) < n))
            .count();
        self.indices.truncate(keep * 6);
    }
}

/// Lists the draw sets of a system tree in draw order (parent first, then children in file
/// order), merging systems with the same material and renderer kind.
pub fn plan_draw_sets(root: &ParticleFile) -> Vec<DrawSetPlan> {
    let mut out = Vec::new();
    plan_rec(root, 1, &mut out);
    for s in &mut out {
        s.capacity = s.capacity.clamp(1, MAX_SET_QUADS);
    }
    out
}

fn plan_rec(file: &ParticleFile, instances: usize, out: &mut Vec<DrawSetPlan>) {
    if let Some(rope) = draw_kind(file) {
        let quads = instances * file.maxcount.clamp(1, 10_000) as usize;
        match out
            .iter_mut()
            .find(|s| s.rope == rope && s.material == file.material)
        {
            Some(s) => s.capacity += quads,
            None => out.push(DrawSetPlan {
                material: file.material.clone(),
                rope,
                capacity: quads,
            }),
        }
    }
    let particles = instances * file.maxcount.clamp(1, 10_000) as usize;
    for (i, c) in file.children.iter().enumerate() {
        let Some(Some(child)) = file.child_files.get(i) else {
            continue;
        };
        let n = match c.kind {
            ParticleChildKind::Static => instances,
            _ => particles.min(c.maxcount.max(1) as usize * instances),
        };
        plan_rec(child, n, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> ParticleFile {
        ParticleFile::from_json_str(s).unwrap()
    }

    fn tree() -> ParticleFile {
        let mut root = parse(
            r#"{"maxcount":1,"renderer":[],
                "emitter":[{"name":"sphererandom","rate":100,"distancemin":0,"distancemax":0}],
                "initializer":[{"name":"lifetimerandom","min":100,"max":100}],
                "children":[
                  {"name":"c/body.json","type":"eventfollow","maxcount":4},
                  {"name":"c/trail.json","type":"eventfollow","maxcount":4}]}"#,
        );
        let read = |p: &str| -> Option<String> {
            match p {
                "c/body.json" => Some(
                    r#"{"maxcount":1,"material":"m/body.json","renderer":[{"name":"sprite"}],
                        "emitter":[{"name":"sphererandom","rate":1000,"distancemin":0,"distancemax":0}],
                        "initializer":[{"name":"lifetimerandom","min":1,"max":1},
                                       {"name":"sizerandom","min":10,"max":10}]}"#
                        .into(),
                ),
                "c/trail.json" => Some(
                    r#"{"maxcount":100,"flags":1,"material":"m/trail.json","renderer":[{"name":"rope"}],
                        "emitter":[{"name":"sphererandom","rate":50,"distancemin":0,"distancemax":0}],
                        "initializer":[{"name":"lifetimerandom","min":2,"max":2},
                                       {"name":"sizerandom","min":6,"max":6},
                                       {"name":"velocityrandom","min":"10 0 0","max":"10 0 0"}],
                        "operator":[{"name":"movement"}]}"#
                        .into(),
                ),
                _ => None,
            }
        };
        root.resolve_children(&read);
        root
    }

    #[test]
    fn parses_child_specs() {
        let f = parse(
            r#"{"children":[{"name":"a.json","type":"static","origin":"60 0 0","scale":"2 2 2","maxcount":3},
                            {"name":"b.json"}]}"#,
        );
        assert_eq!(f.children.len(), 2);
        assert_eq!(f.children[0].kind, ParticleChildKind::Static);
        assert_eq!(f.children[0].origin, Vec3::new(60.0, 0.0, 0.0));
        assert_eq!(f.children[0].scale, Vec3::splat(2.0));
        assert_eq!(f.children[0].maxcount, 3);
        // An omitted `type` is WE's default kind: a single static instance.
        assert_eq!(f.children[1].kind, ParticleChildKind::Static);
        assert_eq!(f.children[1].maxcount, 10);
    }

    #[test]
    fn plan_lists_children_in_order_without_parent_set() {
        let plan = plan_draw_sets(&tree());
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].material, "m/body.json");
        assert!(!plan[0].rope);
        assert!(plan[1].rope);
        assert_eq!(plan[1].capacity, 100);
    }

    #[test]
    fn follow_children_track_the_parent_particle_and_trail_stays_in_world() {
        let mut sys = ParticleSystemInstance::new(tree());
        let plan = plan_draw_sets(&sys.spec);
        for _ in 0..30 {
            sys.step(0.05, Vec3::ZERO);
            // Move the only parent particle 100 units per second to the right.
            for p in &mut sys.particles {
                p.position.x += 5.0;
            }
        }
        let mut out = vec![SetGeometry::default(); plan.len()];
        sys.gather_geometry(&plan, &[], &mut out);
        // Body sprite: 4 vertices, centred on the parent particle.
        assert_eq!(out[0].vertices.len(), 4);
        let parent_x = sys.particles[0].position.x;
        assert!((out[0].vertices[0].position[0] - parent_x).abs() < 6.0);
        // Rope: several points left behind, so its x range is wide, and 2 vertices per point.
        let xs: Vec<f32> = out[1].vertices.iter().map(|v| v.position[0]).collect();
        assert!(out[1].vertices.len() >= 6);
        let span = xs.iter().cloned().fold(f32::MIN, f32::max)
            - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(span > 20.0, "span {span}");
        assert_eq!(out[1].indices.len() % 6, 0);
        let n = out[1].vertices.len();
        assert!(out[1].indices.iter().all(|&i| (i as usize) < n));
    }

    #[test]
    fn static_child_is_offset_and_scaled() {
        let mut root = parse(
            r#"{"maxcount":1,"children":[{"name":"s.json","type":"static","origin":"60 0 0","scale":"2 2 2"}]}"#,
        );
        root.resolve_children(&|p| {
            (p == "s.json").then(|| {
                r#"{"maxcount":1,"material":"m","renderer":[{"name":"sprite"}],
                    "emitter":[{"name":"sphererandom","rate":100,"distancemin":0,"distancemax":0}],
                    "initializer":[{"name":"lifetimerandom","min":5,"max":5},
                                   {"name":"sizerandom","min":10,"max":10}]}"#
                    .to_string()
            })
        });
        let plan = plan_draw_sets(&root);
        let mut sys = ParticleSystemInstance::new(root);
        sys.step(0.1, Vec3::ZERO);
        let mut out = vec![SetGeometry::default(); plan.len()];
        sys.gather_geometry(&plan, &[], &mut out);
        assert_eq!(out[0].vertices.len(), 4);
        assert!((out[0].vertices[0].position[0] - 60.0).abs() < 1e-3);
        // size 10 -> half 5, doubled by the child's scale.
        assert!((out[0].vertices[0].texcoord_rot_size[3] - 10.0).abs() < 1e-3);
    }

    #[test]
    fn burst_children_end_and_are_dropped() {
        let mut root = parse(
            r#"{"maxcount":2,"emitter":[{"name":"sphererandom","rate":10,"distancemin":0,"distancemax":0}],
                "initializer":[{"name":"lifetimerandom","min":0.2,"max":0.2}],
                "children":[{"name":"d.json","type":"eventdeath","maxcount":50}]}"#,
        );
        root.resolve_children(&|p| {
            (p == "d.json").then(|| {
                r#"{"maxcount":5,"material":"m","renderer":[{"name":"sprite"}],
                    "emitter":[{"name":"sphererandom","rate":100,"distancemin":0,"distancemax":0}],
                    "initializer":[{"name":"lifetimerandom","min":0.1,"max":0.1}]}"#
                    .to_string()
            })
        });
        let mut sys = ParticleSystemInstance::new(root);
        let mut max_instances = 0;
        for _ in 0..200 {
            sys.step(0.05, Vec3::ZERO);
            max_instances = max_instances.max(sys.groups[0].instances.len());
        }
        assert!(max_instances > 0);
        assert!(max_instances <= 50);
        // Parent keeps spawning, so some instances are alive but none stay past their burst.
        assert!(sys.groups[0]
            .instances
            .iter()
            .all(|ci| ci.inst.emitting || !ci.inst.particles.is_empty()));
    }

    #[test]
    fn skipped_sprite_indices_leave_the_index_list_empty_but_keep_vertices() {
        let mut sys = ParticleSystemInstance::new(tree());
        let plan = plan_draw_sets(&sys.spec);
        for _ in 0..30 {
            sys.step(0.05, Vec3::ZERO);
        }
        let mut full = vec![SetGeometry::default(); plan.len()];
        sys.gather_geometry(&plan, &[], &mut full);
        let mut skip: Vec<SetGeometry> = plan
            .iter()
            .map(|e| SetGeometry {
                skip_sprite_indices: !e.rope,
                ..SetGeometry::default()
            })
            .collect();
        sys.gather_geometry(&plan, &[], &mut skip);
        assert_eq!(full[0].vertices.len(), skip[0].vertices.len());
        assert!(!full[0].indices.is_empty());
        assert!(skip[0].indices.is_empty());
        // Ropes keep their indices.
        assert_eq!(full[1].indices, skip[1].indices);
    }

    #[test]
    fn fit_drops_triangles_with_missing_vertices() {
        let v = SpriteParticleVertex {
            position: [0.0; 3],
            texcoord_rot_size: [0.0; 4],
            color: [1.0; 4],
            velocity_life: [0.0; 4],
            rotation_xy: [0.0; 2],
        };
        let mut g = SetGeometry {
            vertices: vec![v; 6],
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            ..SetGeometry::default()
        };
        g.fit(10);
        assert_eq!(g.indices.len(), 6);
    }

    #[test]
    fn renderless_leaf_still_draws_as_sprites() {
        let f = parse(r#"{"maxcount":3,"material":"m"}"#);
        assert_eq!(draw_kind(&f), Some(false));
        let f = parse(r#"{"children":[{"name":"x"}]}"#);
        assert_eq!(draw_kind(&f), None);
    }

    #[test]
    fn sprite_sheet_frame_follows_particle_age() {
        use truongpaper_we_formats::TexFrameInfo;
        let frame = |x: f32| TexFrameInfo {
            image_id: 0,
            frametime: 1.0,
            x,
            y: 0.0,
            width: 32.0,
            width_y: 0.0,
            height_x: 0.0,
            height: 64.0,
        };
        let sheet = Arc::new(SpriteSheet::new(&[frame(0.0), frame(32.0)], 64, 64).unwrap());
        let root = parse(
            r#"{"maxcount":1,"material":"m","renderer":[{"name":"sprite"}],
                "emitter":[{"name":"sphererandom","rate":100,"distancemin":0,"distancemax":0}],
                "initializer":[{"name":"lifetimerandom","min":10,"max":10}]}"#,
        );
        let plan = plan_draw_sets(&root);
        let mut sys = ParticleSystemInstance::new(root);
        let sheets = [Some(sheet)];
        let max_u = |sys: &ParticleSystemInstance| {
            let mut out = vec![SetGeometry::default(); 1];
            sys.gather_geometry(&plan, &sheets, &mut out);
            out[0]
                .vertices
                .iter()
                .map(|v| v.rotation_xy[0])
                .fold(0.0f32, f32::max)
        };
        sys.step(0.1, Vec3::ZERO);
        // First half of the lifetime: frame 0 covers u in 0..0.5.
        assert!((max_u(&sys) - 0.5).abs() < 1e-4);
        sys.step(6.0, Vec3::ZERO);
        sys.particles[0].age = 6.0;
        // Second half: frame 1 covers u in 0.5..1.
        assert!((max_u(&sys) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn hostile_maxcount_is_capped_and_budgeted() {
        let mut root = parse(
            r#"{"maxcount":10000,"renderer":[],
                "emitter":[{"name":"sphererandom","rate":200000,"distancemin":0,"distancemax":0}],
                "initializer":[{"name":"lifetimerandom","min":100,"max":100}],
                "children":[
                  {"name":"c/a.json","type":"eventfollow","maxcount":4000000000},
                  {"name":"c/a.json","type":"eventfollow","maxcount":4000000000},
                  {"name":"c/a.json","type":"eventfollow","maxcount":4000000000}]}"#,
        );
        root.resolve_children(&|_: &str| {
            Some(
                r#"{"maxcount":4,"material":"m/a.json","renderer":[{"name":"sprite"}],
                    "emitter":[{"name":"sphererandom","rate":10,"distancemin":0,"distancemax":0}],
                    "initializer":[{"name":"lifetimerandom","min":1,"max":1}]}"#
                    .into(),
            )
        });
        let mut sys = ParticleSystemInstance::new(root);
        for _ in 0..3 {
            sys.step(0.05, Vec3::ZERO);
        }
        assert!(sys.particles.len() > 3000);
        let live: usize = sys.groups.iter().map(|g| g.instances.len()).sum();
        assert!(live <= MAX_TREE_INSTANCES, "{live} instances");
        assert!(sys
            .groups
            .iter()
            .all(|g| g.instances.len() <= MAX_GROUP_INSTANCES));
        let taken = MAX_TREE_INSTANCES - sys.budget.load(Ordering::Acquire);
        assert_eq!(taken, live);
        // Dropping the tree returns every slot.
        let budget = sys.budget.clone();
        drop(sys);
        assert_eq!(budget.load(Ordering::Acquire), MAX_TREE_INSTANCES);
    }

    #[test]
    fn non_finite_particles_are_killed() {
        let root = parse(
            r#"{"maxcount":10,"renderer":[],
                "emitter":[{"name":"sphererandom","rate":100,"distancemin":0,"distancemax":0}],
                "initializer":[{"name":"lifetimerandom","min":100,"max":100}]}"#,
        );
        let mut sys = ParticleSystemInstance::new(root);
        sys.step(0.05, Vec3::ZERO);
        assert!(!sys.particles.is_empty());
        sys.particles[0].position.x = f32::NAN;
        sys.step(0.01, Vec3::ZERO);
        assert!(sys.particles.iter().all(|p| p.position.is_finite()));
    }
}

/// A stable pseudo-random sheet frame for particle `id` (`animationmode: "randomframe"`).
fn random_frame_index(id: u64, frames: usize) -> usize {
    let mut x = id.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x % frames.max(1) as u64) as usize
}

#[cfg(test)]
mod random_frame_tests {
    use super::random_frame_index;

    #[test]
    fn random_frames_are_stable_in_range_and_spread() {
        let picks: Vec<usize> = (0..64).map(|id| random_frame_index(id, 8)).collect();
        assert!(picks.iter().all(|&f| f < 8));
        assert_eq!(random_frame_index(5, 8), random_frame_index(5, 8));
        let mut seen = [false; 8];
        for f in picks {
            seen[f] = true;
        }
        assert!(seen.iter().filter(|s| **s).count() >= 6);
        assert_eq!(random_frame_index(3, 1), 0);
    }
}
