//! Per-monitor scene loop glue: `truongpaper_renderer::SceneGraph` + `truongpaper_scenescript`
//! property scripts + camera parallax/shake + particles + sound-layer plans (docs/03 §7-§8).
//!
//! `truongpaper-renderer` and `truongpaper-scenescript` do not depend on each other, so the frame
//! step lives here. Everything is device-free; GPU drawing (`SceneRenderer::render`) and audio
//! playback (`SceneSoundPlayer::play_plans`) are driven by the caller.
//!
//! Frame order ([`SceneRuntime::step`]):
//! 1. push graph layer state into `thisLayer` objects that changed on the host side,
//! 2. `set_input_state` + `step_frame` (`update(value)` of every property script),
//! 3. pull `thisLayer` writes back into the graph (`apply_layer_state`),
//! 4. apply each slot's returned value (`apply_script_output`; slot results win),
//! 5. step the camera (parallax / shake) and particles.
//!
//! [`SceneScriptEngine`] wraps a QuickJS runtime and is not `Send`: keep a `SceneRuntime` on the
//! thread that created it (the monitor's render thread).

use std::collections::{HashMap, HashSet};
use truongpaper_audio::{effective_volume, SoundLayerPlan, SoundPlaybackMode};
use truongpaper_renderer::{CameraState, DiagnosticsLog, SceneGraph, ScriptProperty};
use truongpaper_scenescript::{CursorEventKind, LayerState, SceneScriptEngine};
use truongpaper_we_formats::scene_schema::{resolve_bound_bool, resolve_bound_value};
use truongpaper_we_formats::WeVirtualFileSystem;

/// `input.cursorWorldPosition` for a cursor in `[0,1]` with a top-left origin (docs/03 §8.2):
/// `(x * scene_w, (1 - y) * scene_h, 0)`.
pub fn cursor_world_position(cursor_norm: [f32; 2], scene_size: [f32; 2]) -> [f32; 3] {
    let [x, y] = finite_cursor(cursor_norm);
    [x * scene_size[0], (1.0 - y) * scene_size[1], 0.0]
}

/// `input.cursorScreenPosition`: `(x * monitor_w, y * monitor_h)`.
pub fn cursor_screen_position(cursor_norm: [f32; 2], monitor_size: [f32; 2]) -> [f32; 2] {
    let [x, y] = finite_cursor(cursor_norm);
    [x * monitor_size[0], y * monitor_size[1]]
}

fn finite_cursor(c: [f32; 2]) -> [f32; 2] {
    let f = |v: f32| if v.is_finite() { v } else { 0.5 };
    [f(c[0]), f(c[1])]
}

/// Local time of day in `[0, 1)` from UTC (no timezone database is available here); hosts that
/// know the local offset call [`SceneRuntime::set_time_of_day`].
fn utc_time_of_day() -> f32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    ((secs % 86_400.0) / 86_400.0) as f32
}

/// JS expression for a slot's initial value: `register_property_script` evaluates it as code.
fn initial_value_expr(prop: Option<ScriptProperty>, raw: &str) -> String {
    let raw = raw.trim();
    let js_string = || serde_json::to_string(raw).unwrap_or_else(|_| "\"\"".into());
    match prop {
        Some(
            ScriptProperty::Origin
            | ScriptProperty::Scale
            | ScriptProperty::Angles
            | ScriptProperty::Color,
        ) => format!("new Vec3({})", js_string()),
        Some(ScriptProperty::Size) => format!("new Vec2({})", js_string()),
        Some(ScriptProperty::Alpha | ScriptProperty::Brightness) => raw
            .trim_matches('"')
            .parse::<f32>()
            .ok()
            .filter(|n| n.is_finite())
            .map_or_else(|| "0".to_string(), |n| format!("{n}")),
        Some(ScriptProperty::Text) => js_string(),
        Some(ScriptProperty::Visible) => {
            let visible = match raw.trim_matches('"') {
                "false" => false,
                "true" => true,
                // Unparsable values default to visible; any non-zero number is visible.
                other => other.parse::<f32>() != Ok(0.0),
            };
            visible.to_string()
        }
        None => js_string(),
    }
}

/// Small deterministic RNG (SplitMix64) for `playbackmode: "random"` sound selection. The root
/// package has no `rand` dependency and `truongpaper-audio` does not re-export it, so
/// [`SceneRuntime::sound_plans`] mirrors `truongpaper_audio::plan_scene_sounds` with this RNG.
#[derive(Debug, Clone)]
pub struct SoundRng(u64);

impl SoundRng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Seeded from the clock (non-deterministic).
    pub fn from_time() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self(nanos)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish index in `0..len` (`len > 0`).
    pub fn index(&mut self, len: usize) -> usize {
        (self.next_u64() % len.max(1) as u64) as usize
    }
}

/// Normalizes a user-property JSON object to `{ key: value }`: `project.json`-style property
/// objects (`{ "type": .., "value": v }`) are unwrapped to `v`. Invalid JSON is returned as is
/// (the caller reports the parse error).
fn plain_properties_json(json: &str) -> String {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(json) else {
        return json.to_string();
    };
    let plain: serde_json::Map<String, serde_json::Value> = map
        .into_iter()
        .map(|(k, v)| match v {
            serde_json::Value::Object(mut o) if o.contains_key("value") => {
                (k, o.remove("value").unwrap_or_default())
            }
            other => (k, other),
        })
        .collect();
    serde_json::Value::Object(plain).to_string()
}

pub struct SceneRuntime {
    graph: SceneGraph,
    engine: SceneScriptEngine,
    camera: CameraState,
    /// `(layer index, SceneScript layer name = SceneLayer::key())`, unique names only.
    script_layers: Vec<(usize, String)>,
    /// Layer state the script context currently holds (last push or pull).
    synced: HashMap<String, LayerState>,
    registered_slots: Vec<String>,
    skipped_slots: Vec<String>,
    elapsed: f32,
    paused: bool,
    time_of_day: Option<f32>,
    /// Notes merged in by the host (e.g. [`truongpaper_renderer::SceneRenderer::diagnostics`]).
    extra_diagnostics: DiagnosticsLog,
}

impl std::fmt::Debug for SceneRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneRuntime")
            .field("scene_path", &self.graph.scene_path)
            .field("layers", &self.graph.layers.len())
            .field("registered_slots", &self.registered_slots)
            .field("skipped_slots", &self.skipped_slots)
            .field("elapsed", &self.elapsed)
            .field("paused", &self.paused)
            .finish()
    }
}

impl SceneRuntime {
    /// Loads `scene_path` through the VFS, creates the SceneScript sandbox, registers every
    /// layer and compiles every property script (running its `init`). Scripts that fail to
    /// compile or initialize are logged and skipped; the scene still loads.
    pub fn load(
        vfs: &WeVirtualFileSystem,
        scene_path: &str,
        user_props_json: Option<&str>,
        monitor_size: [f32; 2],
    ) -> anyhow::Result<Self> {
        let graph = SceneGraph::load(vfs, scene_path, user_props_json, monitor_size.into())?;
        for w in &graph.warnings {
            tracing::warn!("scene {scene_path}: {w}");
        }
        let engine = SceneScriptEngine::new(
            graph.scene_size.x,
            graph.scene_size.y,
            monitor_size[0],
            monitor_size[1],
        )?;

        let mut seen = HashSet::new();
        let script_layers: Vec<(usize, String)> = graph
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (i, l.key()))
            .filter(|(_, key)| seen.insert(key.clone()))
            .collect();

        let mut rt = Self {
            graph,
            engine,
            camera: CameraState::new(),
            script_layers,
            synced: HashMap::new(),
            registered_slots: Vec::new(),
            skipped_slots: Vec::new(),
            elapsed: 0.0,
            paused: false,
            time_of_day: None,
            extra_diagnostics: DiagnosticsLog::default(),
        };

        for (_, key) in &rt.script_layers {
            let s = rt.graph_layer_state(key);
            rt.engine
                .register_layer(key, s.origin, s.scale, s.angles, s.alpha, s.visible)?;
        }
        // register_layer has no size/color: sync the full state once.
        rt.push_layers()?;

        let initial_props = user_props_json.map(plain_properties_json);
        if let Some(json) = &initial_props {
            if let Err(err) = rt.engine.apply_user_properties(json) {
                tracing::warn!("scene {scene_path}: user properties not passed to scripts: {err}");
            }
        }

        let mut init_outputs = Vec::new();
        for slot in rt.graph.property_scripts() {
            let layer_key = slot
                .slot_key
                .rsplit_once('/')
                .map_or(slot.layer_name.as_str(), |(k, _)| k)
                .to_string();
            let prop = ScriptProperty::parse(&slot.property);
            let raw = if slot.initial_value.trim().is_empty() {
                rt.graph
                    .find_layer(&layer_key)
                    .zip(prop)
                    .and_then(|(li, p)| rt.graph.property_value_string(li, p))
                    .unwrap_or_default()
            } else {
                slot.initial_value.clone()
            };
            match rt.engine.register_property_script_with_props(
                &slot.slot_key,
                &layer_key,
                &slot.script,
                &initial_value_expr(prop, &raw),
                &slot.script_properties_json,
            ) {
                Ok(value) => {
                    rt.registered_slots.push(slot.slot_key.clone());
                    init_outputs.push((slot.slot_key, value));
                }
                Err(err) => {
                    tracing::warn!(
                        "scene {scene_path}: SceneScript '{}' skipped: {err}",
                        slot.slot_key
                    );
                    rt.skipped_slots.push(slot.slot_key);
                }
            }
        }
        // `init` may also write `thisLayer`.
        if !rt.registered_slots.is_empty() {
            rt.pull_layers()?;
        }
        for (slot, value) in init_outputs {
            rt.graph.apply_script_output(&slot, &value);
        }
        // Scripts only exist now: give each its `applyUserProperties(all values)` call, as
        // Wallpaper Engine does at load, so saved origin/scale/... settings take effect.
        if let Some(json) = &initial_props {
            if !rt.registered_slots.is_empty() && json != "{}" {
                if let Err(err) = rt.engine.apply_user_properties(json) {
                    tracing::warn!("scene {scene_path}: applyUserProperties at load: {err}");
                }
                rt.pull_layers()?;
            }
        }
        rt.settle_scripts();
        Ok(rt)
    }

    /// Runs one zero-length script frame so `update` outputs (text, origin, visibility) are in
    /// the graph before anything is drawn: a text layer must never show its editor placeholder.
    fn settle_scripts(&mut self) {
        if self.registered_slots.is_empty() {
            return;
        }
        let cursor = [0.5, 0.5];
        let world = cursor_world_position(cursor, self.scene_size());
        if let Err(err) = self.step_scripts(0.0, cursor, world, false, &[0.0; 64], &[0.0; 64]) {
            tracing::warn!(
                "scene {}: SceneScript settle failed: {err}",
                self.graph.scene_path
            );
        }
    }

    pub fn graph(&self) -> &SceneGraph {
        &self.graph
    }

    pub fn camera(&self) -> &CameraState {
        &self.camera
    }

    /// Seconds stepped while not paused (`engine.runtime`).
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Smart Pause / user pause: while paused [`Self::step`] does nothing.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Overrides `engine.timeOfDay` (`[0,1]`); `None` falls back to UTC.
    pub fn set_time_of_day(&mut self, time_of_day: Option<f32>) {
        self.time_of_day = time_of_day
            .filter(|t| t.is_finite())
            .map(|t| t.clamp(0.0, 1.0));
    }

    /// Short human-readable notes about content that is skipped or approximated in this scene
    /// (unsupported layers, scripts that failed to compile, plus anything added with
    /// [`Self::add_diagnostics`], such as the renderer's skipped effect passes). Deduplicated
    /// and capped at 50 entries.
    pub fn diagnostics(&self) -> Vec<String> {
        let mut log = self.graph.diagnostics.clone();
        for slot in &self.skipped_slots {
            log.push(format!(
                "SceneScript {slot} skipped: failed to compile or start"
            ));
        }
        log.extend(self.extra_diagnostics.to_vec());
        log.to_vec()
    }

    /// Merges externally produced notes (e.g. `SceneRenderer::diagnostics()`) into
    /// [`Self::diagnostics`].
    pub fn add_diagnostics<I: IntoIterator<Item = String>>(&mut self, notes: I) {
        self.extra_diagnostics.extend(notes);
    }

    /// Slot keys whose scripts compiled and initialized.
    pub fn script_slots(&self) -> &[String] {
        &self.registered_slots
    }

    /// Slot keys whose scripts failed to compile or initialize.
    pub fn skipped_script_slots(&self) -> &[String] {
        &self.skipped_slots
    }

    /// Advances the scene by `dt` seconds. `cursor_norm` is the cursor on this monitor in
    /// `[0,1]` with a top-left origin; `left_64` / `right_64` are
    /// `AudioSpectrumFrame::{left_64, right_64}`. Returns `false` (and does nothing) while
    /// paused. Script errors (e.g. the frame budget) are logged; camera and particles still step.
    pub fn step(
        &mut self,
        dt: f32,
        cursor_norm: [f32; 2],
        left_down: bool,
        left_64: &[f32; 64],
        right_64: &[f32; 64],
    ) -> bool {
        if self.paused {
            return false;
        }
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.elapsed += dt;
        let world = cursor_world_position(cursor_norm, self.scene_size());
        if !self.registered_slots.is_empty() {
            if let Err(err) =
                self.step_scripts(dt, cursor_norm, world, left_down, left_64, right_64)
            {
                tracing::warn!(
                    "scene {}: SceneScript frame failed: {err}",
                    self.graph.scene_path
                );
            }
        }
        // Parallax uses the pointer in scene orientation (y up), like cursorWorldPosition.
        let [x, y] = finite_cursor(cursor_norm);
        self.camera
            .step(dt, [x, 1.0 - y].into(), &self.graph.camera);
        self.graph.step_particles(dt, [world[0], world[1]].into());
        true
    }

    fn step_scripts(
        &mut self,
        dt: f32,
        cursor_norm: [f32; 2],
        world: [f32; 3],
        left_down: bool,
        left_64: &[f32; 64],
        right_64: &[f32; 64],
    ) -> anyhow::Result<()> {
        self.push_layers()?;
        let screen = cursor_screen_position(cursor_norm, self.graph.monitor_size.to_array());
        let tod = self.time_of_day.unwrap_or_else(utc_time_of_day);
        self.engine.set_input_state(screen, left_down, tod)?;
        let outputs = self
            .engine
            .step_frame(dt, self.elapsed, world, left_64, right_64)?;
        self.pull_layers()?;
        // Registration order keeps multiple slots on one layer deterministic.
        for slot in &self.registered_slots {
            if let Some(value) = outputs.get(slot) {
                self.graph.apply_script_output(slot, value);
            }
        }
        Ok(())
    }

    /// Forwards a desktop cursor event (`cursorMove/Down/Up/Click`) to every script.
    pub fn dispatch_cursor(
        &mut self,
        kind: CursorEventKind,
        cursor_norm: [f32; 2],
    ) -> anyhow::Result<()> {
        if self.registered_slots.is_empty() {
            return Ok(());
        }
        let world = cursor_world_position(cursor_norm, self.scene_size());
        self.push_layers()?;
        self.engine
            .dispatch_cursor_event(kind, world[0], world[1])?;
        self.pull_layers()?;
        Ok(())
    }

    /// Applies changed user properties (JSON object of `key: value`) to the scene graph (bound
    /// values re-resolve) and calls `applyUserProperties(changedProps)` on every script.
    pub fn apply_user_properties(&mut self, changed_json: &str) -> anyhow::Result<()> {
        let changed_json = plain_properties_json(changed_json);
        self.graph.apply_user_properties(&changed_json)?;
        // Instance-level `scriptproperties` bound to the changed properties.
        for slot in self.graph.property_scripts() {
            if slot.script_properties_json != "{}" && self.registered_slots.contains(&slot.slot_key)
            {
                self.engine
                    .update_script_properties(&slot.slot_key, &slot.script_properties_json)?;
            }
        }
        self.push_layers()?;
        self.engine.apply_user_properties(&changed_json)?;
        self.pull_layers()?;
        self.settle_scripts();
        Ok(())
    }

    /// Sound layers ready for `SceneSoundPlayer::play_plans` (same rules as
    /// `truongpaper_audio::plan_scene_sounds`: hidden objects, empty lists and unreadable files
    /// are skipped; `volume` and `visible` resolve against the current user properties).
    pub fn sound_plans(
        &self,
        vfs: &WeVirtualFileSystem,
        master_volume: f32,
        rng: &mut SoundRng,
    ) -> Vec<SoundLayerPlan> {
        let user_props = &self.graph.user_props;
        let mut plans = Vec::new();
        for obj in &self.graph.scene.objects {
            let Some(paths) = obj.sound.as_ref().filter(|p| !p.is_empty()) else {
                continue;
            };
            if !resolve_bound_bool(&obj.visible, user_props) {
                continue;
            }
            let mode = SoundPlaybackMode::parse(obj.playbackmode.as_deref());
            let rel = match mode {
                SoundPlaybackMode::Random => &paths[rng.index(paths.len())],
                SoundPlaybackMode::Loop | SoundPlaybackMode::Single => &paths[0],
            };
            let bytes = match vfs.read_bytes(rel) {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!("Sound layer '{}': cannot read {rel}: {err}", obj.name);
                    continue;
                }
            };
            let layer_volume = obj
                .volume
                .as_ref()
                .map(|v| resolve_bound_value(v, user_props))
                .unwrap_or(1.0);
            plans.push(SoundLayerPlan {
                object_name: obj.name.clone(),
                path: rel.clone(),
                bytes,
                mode,
                volume: effective_volume(layer_volume, master_volume),
            });
        }
        plans
    }

    fn scene_size(&self) -> [f32; 2] {
        self.graph.scene_size.to_array()
    }

    fn graph_layer_state(&self, key: &str) -> LayerState {
        let layer = self.graph.find_layer(key).map(|i| &self.graph.layers[i]);
        match layer {
            Some(l) => LayerState {
                name: key.to_string(),
                origin: l.transform.origin.to_array(),
                scale: l.transform.scale.to_array(),
                angles: l.transform.angles.to_array(),
                size: l.transform.size.to_array(),
                color: l.color.to_array(),
                alpha: l.alpha,
                visible: l.visible,
                text: match &l.kind {
                    truongpaper_renderer::SceneLayerKind::Text { text, .. } => text.clone(),
                    _ => String::new(),
                },
            },
            None => LayerState {
                name: key.to_string(),
                origin: [0.0; 3],
                scale: [1.0; 3],
                angles: [0.0; 3],
                size: [0.0; 2],
                color: [1.0; 3],
                alpha: 1.0,
                visible: true,
                text: String::new(),
            },
        }
    }

    /// Sends host-side changes (script outputs, user properties) to the script context.
    fn push_layers(&mut self) -> anyhow::Result<()> {
        let changed: Vec<LayerState> = self
            .script_layers
            .iter()
            .map(|(_, key)| self.graph_layer_state(key))
            .filter(|s| self.synced.get(&s.name) != Some(s))
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        self.engine.sync_layer_states(&changed)?;
        for s in changed {
            self.synced.insert(s.name.clone(), s);
        }
        Ok(())
    }

    /// `thisLayer` write-back: applies layers whose script-side state changed since the last sync.
    fn pull_layers(&mut self) -> anyhow::Result<usize> {
        let mut applied = 0;
        for s in self.engine.layer_states()? {
            if self.synced.get(&s.name) == Some(&s) {
                continue;
            }
            if self.graph.apply_layer_state(
                &s.name,
                s.origin.into(),
                s.scale.into(),
                s.angles.into(),
                s.size.into(),
                s.color.into(),
                s.alpha,
                s.visible,
            ) {
                applied += 1;
            }
            self.graph.apply_layer_text(&s.name, &s.text);
            self.synced.insert(s.name.clone(), s);
        }
        Ok(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    const Z64: [f32; 64] = [0.0; 64];
    const MONITOR: [f32; 2] = [2560.0, 1440.0];

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "truongpaper-scene-runtime-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, rel: &str, contents: &[u8]) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
        }

        fn vfs(&self) -> WeVirtualFileSystem {
            WeVirtualFileSystem::mount(self.0.clone(), self.0.join("core"))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const SCENE: &str = r#"{
        "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } },
        "objects": [
            { "name": "Hero", "image": "models/solid.json",
              "origin": { "script": "'use strict';\nexport function update(value) { value.x += 10; return value; }",
                          "value": "960 540 0" },
              "alpha": { "user": "heroalpha", "value": 1 } },
            { "name": "Mover", "image": "models/solid.json", "origin": "100 100 0",
              "visible": { "script": "export function update(v) { thisLayer.origin = thisLayer.origin.add(new Vec3(0, 5, 0)); thisScene.getLayer('Hero').color = new Vec3(1, 0, 0); return v; }",
                           "value": true } },
            { "name": "Broken", "image": "models/solid.json",
              "scale": { "script": "export function update( {", "value": "1 1 1" } }
        ]
    }"#;

    fn scene_dir(tag: &str) -> TempDir {
        let t = TempDir::new(tag);
        t.write(
            "models/solid.json",
            br#"{ "solidlayer": true, "width": 100, "height": 50 }"#,
        );
        t.write("scene.json", SCENE.as_bytes());
        t
    }

    fn layer<'a>(rt: &'a SceneRuntime, name: &str) -> &'a truongpaper_renderer::SceneLayer {
        &rt.graph().layers[rt.graph().find_layer(name).unwrap()]
    }

    fn step(rt: &mut SceneRuntime) -> bool {
        rt.step(0.016, [0.5, 0.5], false, &Z64, &Z64)
    }

    #[test]
    fn cursor_world_conversion_flips_y() {
        assert_eq!(
            cursor_world_position([0.25, 0.25], [1920.0, 1080.0]),
            [480.0, 810.0, 0.0]
        );
        assert_eq!(
            cursor_world_position([0.0, 0.0], [1920.0, 1080.0]),
            [0.0, 1080.0, 0.0]
        );
        assert_eq!(
            cursor_world_position([1.0, 1.0], [1920.0, 1080.0]),
            [1920.0, 0.0, 0.0]
        );
        assert_eq!(
            cursor_screen_position([0.25, 0.75], MONITOR),
            [640.0, 1080.0]
        );
        assert_eq!(
            cursor_world_position([f32::NAN, 0.0], [100.0, 100.0]),
            [50.0, 100.0, 0.0]
        );
    }

    #[test]
    fn initial_value_expressions_are_js() {
        use ScriptProperty::*;
        assert_eq!(
            initial_value_expr(Some(Origin), "960 540 0"),
            r#"new Vec3("960 540 0")"#
        );
        assert_eq!(
            initial_value_expr(Some(Origin), r#"1"); evil("#),
            r#"new Vec3("1\"); evil(")"#
        );
        assert_eq!(
            initial_value_expr(Some(Size), "10 20"),
            r#"new Vec2("10 20")"#
        );
        assert_eq!(initial_value_expr(Some(Alpha), "0.5"), "0.5");
        assert_eq!(initial_value_expr(Some(Alpha), "nope"), "0");
        assert_eq!(initial_value_expr(Some(Visible), "false"), "false");
        assert_eq!(initial_value_expr(Some(Visible), "0"), "false");
        assert_eq!(initial_value_expr(Some(Visible), "true"), "true");
        assert_eq!(initial_value_expr(None, "x"), r#""x""#);
    }

    #[test]
    fn scripts_move_layers_and_broken_scripts_are_skipped() {
        let t = scene_dir("move");
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", None, MONITOR).unwrap();
        assert_eq!(rt.script_slots(), ["Hero/origin", "Mover/visible"]);
        assert_eq!(rt.skipped_script_slots(), ["Broken/scale"]);
        assert_eq!(
            layer(&rt, "Hero").transform.origin.to_array(),
            [970.0, 540.0, 0.0]
        );

        assert!(step(&mut rt));
        assert_eq!(
            layer(&rt, "Hero").transform.origin.to_array(),
            [980.0, 540.0, 0.0]
        );
        // thisLayer / thisScene writes come back into the graph.
        assert_eq!(
            layer(&rt, "Mover").transform.origin.to_array(),
            [100.0, 110.0, 0.0]
        );
        assert_eq!(layer(&rt, "Hero").color.to_array(), [1.0, 0.0, 0.0]);
        let hero_world = layer(&rt, "Hero").world_matrix.w_axis;
        assert_eq!((hero_world.x, hero_world.y), (980.0, 540.0));

        assert!(step(&mut rt));
        assert_eq!(
            layer(&rt, "Hero").transform.origin.to_array(),
            [990.0, 540.0, 0.0]
        );
        assert_eq!(
            layer(&rt, "Mover").transform.origin.to_array(),
            [100.0, 115.0, 0.0]
        );
        assert!((rt.elapsed() - 0.032).abs() < 1e-6);
    }

    #[test]
    fn paused_step_is_a_noop() {
        let t = scene_dir("paused");
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", None, MONITOR).unwrap();
        rt.set_paused(true);
        assert!(rt.is_paused());
        assert!(!step(&mut rt));
        assert_eq!(
            layer(&rt, "Hero").transform.origin.to_array(),
            [970.0, 540.0, 0.0]
        );
        assert_eq!(rt.elapsed(), 0.0);
        assert_eq!(rt.camera().time, 0.0);
        rt.set_paused(false);
        assert!(step(&mut rt));
        assert_eq!(
            layer(&rt, "Hero").transform.origin.to_array(),
            [980.0, 540.0, 0.0]
        );
    }

    #[test]
    fn user_properties_change_alpha_and_reach_scripts() {
        let t = scene_dir("props");
        let mut rt = SceneRuntime::load(
            &t.vfs(),
            "scene.json",
            Some(r#"{"heroalpha": 0.5}"#),
            MONITOR,
        )
        .unwrap();
        assert_eq!(layer(&rt, "Hero").alpha, 0.5);
        rt.apply_user_properties(r#"{"heroalpha": 0.25}"#).unwrap();
        assert_eq!(layer(&rt, "Hero").alpha, 0.25);
        assert!(rt.apply_user_properties("not json").is_err());

        // The script context sees the merged value too.
        let t2 = TempDir::new("props-js");
        t2.write(
            "models/solid.json",
            br#"{ "solidlayer": true, "width": 10, "height": 10 }"#,
        );
        t2.write(
            "scene.json",
            br#"{ "general": { "orthogonalprojection": { "width": 100, "height": 100 } }, "objects": [
                { "name": "A", "image": "models/solid.json",
                  "alpha": { "script": "export function update(v) { return engine.userProperties.fade ?? 1; }", "value": 1 } } ] }"#,
        );
        let mut rt = SceneRuntime::load(&t2.vfs(), "scene.json", None, [100.0, 100.0]).unwrap();
        step(&mut rt);
        assert_eq!(layer(&rt, "A").alpha, 1.0);
        rt.apply_user_properties(r#"{"fade": 0.3}"#).unwrap();
        step(&mut rt);
        assert!((layer(&rt, "A").alpha - 0.3).abs() < 1e-6);
    }

    fn text_of(rt: &SceneRuntime, name: &str) -> String {
        match &layer(rt, name).kind {
            truongpaper_renderer::SceneLayerKind::Text { text, .. } => text.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn text_scripts_drive_layer_text_each_frame() {
        let t = TempDir::new("text");
        t.write(
            "scene.json",
            br#"{ "general": { "orthogonalprojection": { "width": 100, "height": 100 } }, "objects": [
                { "name": "Clock", "size": "100 20",
                  "text": { "script": "let n = 0;\nexport function update(v) { n += 1; return 'tick ' + n; }", "value": "init" } },
                { "name": "Other", "text": "x",
                  "visible": { "script": "export function update(v) { thisScene.getLayer('Other').text = 'set by script'; return v; }", "value": true } }
            ] }"#,
        );
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", None, [100.0, 100.0]).unwrap();
        // The editor placeholder ("init") is never exposed: scripts settle before the first draw.
        assert_eq!(text_of(&rt, "Clock"), "tick 1");
        assert_eq!(text_of(&rt, "Other"), "set by script");
        step(&mut rt);
        assert_eq!(text_of(&rt, "Clock"), "tick 2");
        step(&mut rt);
        assert_eq!(text_of(&rt, "Clock"), "tick 3");
    }

    #[test]
    fn diagnostics_report_skipped_content_and_merge_external_notes() {
        let t = scene_dir("diag");
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", None, MONITOR).unwrap();
        let d = rt.diagnostics();
        assert!(
            d.iter()
                .any(|m| m.contains("Broken/scale") && m.contains("skipped")),
            "{d:?}"
        );
        rt.add_diagnostics(["Effect waterwaves.json skipped: shader failed to compile".into()]);
        rt.add_diagnostics(["Effect waterwaves.json skipped: shader failed to compile".into()]);
        let d = rt.diagnostics();
        assert_eq!(
            d.iter().filter(|m| m.contains("waterwaves")).count(),
            1,
            "deduplicated"
        );
        rt.add_diagnostics((0..80).map(|i| format!("note {i}")));
        assert_eq!(rt.diagnostics().len(), 50);
    }

    #[test]
    fn cursor_events_and_input_reach_scripts() {
        let t = TempDir::new("cursor");
        t.write(
            "models/solid.json",
            br#"{ "solidlayer": true, "width": 10, "height": 10 }"#,
        );
        t.write(
            "scene.json",
            br#"{ "general": { "orthogonalprojection": { "width": 1000, "height": 500 } }, "objects": [
                { "name": "C", "image": "models/solid.json",
                  "origin": { "script": "export function cursorClick(e) { thisLayer.origin = new Vec3(e.x, e.y, 0); }\nexport function update(v) { return v; }", "value": "0 0 0" } },
                { "name": "F", "image": "models/solid.json",
                  "origin": { "script": "export function update(v) { return new Vec3(input.cursorWorldPosition.x, input.cursorWorldPosition.y, input.cursorLeftDown ? 1 : 0); }", "value": "0 0 0" } } ] }"#,
        );
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", None, [2000.0, 1000.0]).unwrap();
        rt.dispatch_cursor(CursorEventKind::Click, [0.1, 0.2])
            .unwrap();
        assert_eq!(
            layer(&rt, "C").transform.origin.to_array(),
            [100.0, 400.0, 0.0]
        );

        rt.step(0.016, [0.5, 0.0], true, &Z64, &Z64);
        assert_eq!(
            layer(&rt, "F").transform.origin.to_array(),
            [500.0, 500.0, 1.0]
        );
    }

    #[test]
    fn camera_and_particles_step() {
        let t = TempDir::new("camera");
        t.write(
            "particles/sparks.json",
            br#"{ "maxcount": 32, "emitter": [ { "name": "boxrandom", "rate": 100 } ], "renderer": [ { "name": "sprite" } ] }"#,
        );
        t.write(
            "scene.json",
            br#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 },
                     "cameraparallax": true, "cameraparallaxamount": 1, "cameraparallaxdelay": 0,
                     "cameraparallaxmouseinfluence": 1 },
                 "objects": [ { "name": "Spark", "particle": "particles/sparks.json", "origin": "960 540 0" } ] }"#,
        );
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", None, MONITOR).unwrap();
        assert!(rt.script_slots().is_empty());
        let spark = rt.graph().find_layer("Spark").unwrap();
        assert_eq!(
            rt.graph().particle_system(spark).unwrap().particles.len(),
            0
        );
        rt.step(0.1, [1.0, 0.0], false, &Z64, &Z64);
        assert!(!rt
            .graph()
            .particle_system(spark)
            .unwrap()
            .particles
            .is_empty());
        assert!((rt.camera().time - 0.1).abs() < 1e-6);
        // Top-right cursor -> pointer (1, 1) in scene orientation.
        assert_eq!(rt.camera().pointer.to_array(), [1.0, 1.0]);
    }

    /// Minimal 16-bit mono PCM WAV.
    fn wav_bytes(frames: usize) -> Vec<u8> {
        let data_len = (frames * 2) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend((36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(8_000u32.to_le_bytes());
        b.extend(16_000u32.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend(data_len.to_le_bytes());
        b.extend(vec![0u8; frames * 2]);
        b
    }

    #[test]
    fn sound_plans_follow_scene_and_user_properties() {
        let t = TempDir::new("sound");
        for name in ["a.wav", "r1.wav", "r2.wav"] {
            t.write(&format!("sounds/{name}"), &wav_bytes(16));
        }
        t.write(
            "scene.json",
            br#"{ "general": { "orthogonalprojection": { "width": 100, "height": 100 } }, "objects": [
                { "name": "Ambience", "sound": ["sounds/a.wav"], "volume": { "user": "vol", "value": 0.5 } },
                { "name": "Rain", "sound": ["sounds/r1.wav", "sounds/r2.wav"], "playbackmode": "random" },
                { "name": "Missing", "sound": ["sounds/missing.wav"] },
                { "name": "Hidden", "sound": ["sounds/a.wav"], "visible": { "user": "show", "value": false } },
                { "name": "Escape", "sound": ["../secret.wav"] } ] }"#,
        );
        let vfs = t.vfs();
        let mut rt = SceneRuntime::load(&vfs, "scene.json", None, [100.0, 100.0]).unwrap();
        let plans = rt.sound_plans(&vfs, 0.8, &mut SoundRng::new(1));
        let names: Vec<&str> = plans.iter().map(|p| p.object_name.as_str()).collect();
        assert_eq!(names, ["Ambience", "Rain"]);
        assert!((plans[0].volume - 0.4).abs() < 1e-6);
        assert_eq!(plans[0].mode, SoundPlaybackMode::Loop);
        assert_eq!(plans[1].mode, SoundPlaybackMode::Random);
        assert!(plans[1].path.starts_with("sounds/r"));

        let pick = |seed| {
            let mut rng = SoundRng::new(seed);
            (0..16)
                .map(|_| rt.sound_plans(&vfs, 1.0, &mut rng)[1].path.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(pick(9), pick(9), "seeded selection is deterministic");
        let mut seen = pick(9);
        seen.sort();
        seen.dedup();
        assert_eq!(seen, ["sounds/r1.wav", "sounds/r2.wav"]);

        rt.apply_user_properties(r#"{"vol": 1.0, "show": true}"#)
            .unwrap();
        let plans = rt.sound_plans(&vfs, 1.0, &mut SoundRng::from_time());
        let names: Vec<&str> = plans.iter().map(|p| p.object_name.as_str()).collect();
        assert_eq!(names, ["Ambience", "Rain", "Hidden"]);
        assert_eq!(plans[0].volume, 1.0);
        assert_eq!(t.path().join("sounds").read_dir().unwrap().count(), 3);
    }

    #[test]
    fn missing_scene_is_an_error() {
        let t = TempDir::new("missing");
        assert!(SceneRuntime::load(&t.vfs(), "scene.json", None, MONITOR).is_err());
    }

    fn bound_scene() -> TempDir {
        let t = TempDir::new("bound");
        t.write(
            "models/solid.json",
            br#"{ "solidlayer": true, "width": 10, "height": 10 }"#,
        );
        t.write(
            "scene.json",
            br#"{ "general": { "orthogonalprojection": { "width": 1000, "height": 1000 } }, "objects": [
                { "name": "Gate", "image": "models/solid.json",
                  "visible": { "user": { "name": "mode", "condition": "2" }, "value": false } },
                { "name": "Pick", "image": "models/solid.json",
                  "visible": { "user": { "name": "flag", "condition": "1" }, "value": false } },
                { "name": "Fx", "image": "models/solid.json",
                  "effects": [ { "file": "effects/x/effect.json",
                    "visible": { "user": "fxon", "value": true },
                    "passes": [ { "constantshadervalues": { "amount": { "user": "amt", "value": 0.5 } } } ] } ] },
                { "name": "Scripted", "image": "models/solid.json",
                  "alpha": { "script": "export var scriptProperties = createScriptProperties().addCheckbox({ name: 'on', value: false }).finish();\nexport function update(v) { return scriptProperties.on ? 1 : 0.2; }",
                             "scriptproperties": { "on": { "user": "flag", "value": false } }, "value": 1 } },
                { "name": "Moved", "image": "models/solid.json",
                  "origin": { "script": "let x = 5;\nexport function update(v) { return new Vec3(x, 7, 0); }\nexport function applyUserProperties(p) { if (p.px !== undefined) x = p.px; }", "value": "0 0 0" } }
            ] }"#,
        );
        t
    }

    #[test]
    fn condition_bindings_and_saved_values_apply_at_load_and_live() {
        let t = bound_scene();
        // Saved values arrive both as plain values and as project.json property objects.
        let props = r#"{"mode": {"type":"combo","value":"2"}, "flag": true, "px": 40}"#;
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", Some(props), MONITOR).unwrap();
        step(&mut rt);
        assert!(
            layer(&rt, "Gate").visible,
            "combo condition matches at load"
        );
        assert!(layer(&rt, "Pick").visible, "bool matches condition \"1\"");
        assert_eq!(
            layer(&rt, "Scripted").alpha,
            1.0,
            "bound scriptproperties at load"
        );
        assert_eq!(
            layer(&rt, "Moved").transform.origin.x,
            40.0,
            "applyUserProperties at load"
        );

        rt.apply_user_properties(r#"{"mode": "3", "flag": false, "px": 80}"#)
            .unwrap();
        step(&mut rt);
        assert!(!layer(&rt, "Gate").visible);
        assert!(!layer(&rt, "Pick").visible);
        assert_eq!(
            layer(&rt, "Scripted").alpha,
            0.2,
            "scriptproperties re-bound live"
        );
        assert_eq!(layer(&rt, "Moved").transform.origin.x, 80.0);
    }

    #[test]
    fn render_epoch_changes_only_for_baked_bindings() {
        let t = bound_scene();
        let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", None, MONITOR).unwrap();
        let e0 = rt.graph().render_epoch();
        rt.apply_user_properties(r#"{"mode": "3"}"#).unwrap();
        assert_eq!(rt.graph().render_epoch(), e0);
        rt.apply_user_properties(r#"{"amt": 2}"#).unwrap();
        let e1 = rt.graph().render_epoch();
        assert!(e1 > e0, "effect shader constant is baked into the GPU plan");
        rt.apply_user_properties(r#"{"fxon": false}"#).unwrap();
        assert!(rt.graph().render_epoch() > e1, "effect visibility too");
    }

    /// Persica's clock: origin = screenResolution / 100 * percent, in a 3840x2160 scene.
    #[test]
    fn percent_of_screen_origin_scripts_stay_on_canvas_on_retina() {
        let t = TempDir::new("percent-origin");
        t.write(
            "models/solid.json",
            br#"{ "solidlayer": true, "width": 100, "height": 50 }"#,
        );
        t.write(
            "scene.json",
            br#"{
            "general": { "orthogonalprojection": { "width": 3840, "height": 2160 } },
            "objects": [ { "name": "Clock", "image": "models/solid.json",
              "origin": { "script": "let ox = 960; let oy = 1035;\nexport function update(v) { return new Vec3(ox, oy, v.z); }\nexport function applyUserProperties(p) { if (p.cx != undefined) ox = (engine.screenResolution.x/100)*p.cx; if (p.cy != undefined) oy = (engine.screenResolution.y/100)*p.cy; }",
                          "value": "1920 1080 0" } } ]
            }"#,
        );
        let props = r#"{"cx":100,"cy":100}"#;
        let origin = |monitor: [f32; 2]| {
            let mut rt = SceneRuntime::load(&t.vfs(), "scene.json", Some(props), monitor).unwrap();
            step(&mut rt);
            layer(&rt, "Clock").transform.origin.to_array()
        };
        // Physical pixels put the layer off the top of the 2160-high canvas...
        assert_eq!(origin([3456.0, 2234.0])[1], 2234.0);
        // ...the logical size the desktop host passes keeps it near the centre.
        let logical = crate::desktop_host::scene_monitor_size(3456, 2234, 2.0);
        assert_eq!(logical, [1728.0, 1117.0]);
        let o = origin(logical);
        assert_eq!((o[0], o[1]), (1728.0, 1117.0));
        assert!(o[1] < 2160.0);
    }
}
