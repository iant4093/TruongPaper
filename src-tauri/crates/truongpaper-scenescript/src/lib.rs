//! `truongpaper-scenescript`: Embedded `rquickjs` QuickJS sandbox implementing Wallpaper Engine's
//! SceneScript runtime (`engine`, `input`, `thisLayer`, `thisScene`, `Vec2/Vec3/Vec4`, `shared`).
//! Complete specification: `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` (Sections 8.1-8.3).
//!
//! Lifecycle callbacks: `init(value)`, `update(value)`, `cursorMove/Down/Up/Click(event)`,
//! `applyUserProperties(changedProps)`. Each property script lives in a "slot" keyed by the host
//! (e.g. `"layer7.alpha"`); `update` runs for every slot each frame via [`SceneScriptEngine::step_frame`].

use parking_lot::Mutex;
use rquickjs::{CaughtError, Context, Ctx, Function, Runtime};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Memory ceiling for one script runtime.
pub const MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
/// Wall-clock budget for one `init`/event/`applyUserProperties` host call.
pub const INIT_BUDGET: Duration = Duration::from_secs(2);
/// Wall-clock budget for one `step_frame` (all slots).
pub const FRAME_BUDGET: Duration = Duration::from_millis(250);

pub const SCENESCRIPT_BOOTSTRAP_JS: &str = r#"
class Vec2 {
    constructor(x = 0, y = x) {
        if (typeof x === 'string') {
            const p = x.trim().split(/\s+/).map(Number);
            this.x = p[0] || 0; this.y = p[1] ?? this.x;
        } else if (x && typeof x === 'object') {
            this.x = x.x || 0; this.y = x.y || 0;
        } else {
            this.x = Number(x) || 0; this.y = Number(y) || 0;
        }
    }
    add(v) { return new Vec2(this.x + (v.x ?? v), this.y + (v.y ?? v)); }
    subtract(v) { return new Vec2(this.x - (v.x ?? v), this.y - (v.y ?? v)); }
    multiply(v) { return new Vec2(this.x * (v.x ?? v), this.y * (v.y ?? v)); }
    divide(v) { return new Vec2(this.x / (v.x ?? v), this.y / (v.y ?? v)); }
    dot(v) { return this.x * v.x + this.y * v.y; }
    length() { return Math.hypot(this.x, this.y); }
    lengthSqr() { return this.x * this.x + this.y * this.y; }
    normalize() { const l = this.length() || 1; return new Vec2(this.x / l, this.y / l); }
    mix(v, t) { return new Vec2(this.x + (v.x - this.x) * t, this.y + (v.y - this.y) * t); }
    copy() { return new Vec2(this.x, this.y); }
    toString() { return `${this.x} ${this.y}`; }
}

class Vec3 {
    constructor(x = 0, y = x, z = x) {
        if (typeof x === 'string') {
            const p = x.trim().split(/\s+/).map(Number);
            this.x = p[0] || 0; this.y = p[1] ?? this.x; this.z = p[2] ?? 0;
        } else if (x && typeof x === 'object') {
            this.x = x.x || 0; this.y = x.y || 0; this.z = x.z || 0;
        } else {
            this.x = Number(x) || 0; this.y = Number(y) || 0; this.z = Number(z) || 0;
        }
    }
    add(v) { return new Vec3(this.x + (v.x ?? v), this.y + (v.y ?? v), this.z + (v.z ?? v)); }
    subtract(v) { return new Vec3(this.x - (v.x ?? v), this.y - (v.y ?? v), this.z - (v.z ?? v)); }
    multiply(v) { return new Vec3(this.x * (v.x ?? v), this.y * (v.y ?? v), this.z * (v.z ?? v)); }
    divide(v) { return new Vec3(this.x / (v.x ?? v), this.y / (v.y ?? v), this.z / (v.z ?? v)); }
    dot(v) { return this.x * v.x + this.y * v.y + this.z * v.z; }
    cross(v) {
        return new Vec3(
            this.y * v.z - this.z * v.y,
            this.z * v.x - this.x * v.z,
            this.x * v.y - this.y * v.x
        );
    }
    length() { return Math.hypot(this.x, this.y, this.z); }
    lengthSqr() { return this.x * this.x + this.y * this.y + this.z * this.z; }
    normalize() { const l = this.length() || 1; return new Vec3(this.x / l, this.y / l, this.z / l); }
    mix(v, t) {
        return new Vec3(
            this.x + (v.x - this.x) * t,
            this.y + (v.y - this.y) * t,
            this.z + (v.z - this.z) * t
        );
    }
    copy() { return new Vec3(this.x, this.y, this.z); }
    toString() { return `${this.x} ${this.y} ${this.z}`; }
}

class Vec4 {
    constructor(x = 0, y = x, z = x, w = 1) {
        if (typeof x === 'string') {
            const p = x.trim().split(/\s+/).map(Number);
            this.x = p[0] || 0; this.y = p[1] ?? this.x; this.z = p[2] ?? 0; this.w = p[3] ?? 1;
        } else if (x && typeof x === 'object') {
            this.x = x.x || 0; this.y = x.y || 0; this.z = x.z || 0; this.w = x.w ?? 1;
        } else {
            this.x = Number(x) || 0; this.y = Number(y) || 0;
            this.z = Number(z) || 0; this.w = Number(w) || 0;
        }
    }
    add(v) { return new Vec4(this.x + (v.x ?? v), this.y + (v.y ?? v), this.z + (v.z ?? v), this.w + (v.w ?? v)); }
    subtract(v) { return new Vec4(this.x - (v.x ?? v), this.y - (v.y ?? v), this.z - (v.z ?? v), this.w - (v.w ?? v)); }
    multiply(v) { return new Vec4(this.x * (v.x ?? v), this.y * (v.y ?? v), this.z * (v.z ?? v), this.w * (v.w ?? v)); }
    divide(v) { return new Vec4(this.x / (v.x ?? v), this.y / (v.y ?? v), this.z / (v.z ?? v), this.w / (v.w ?? v)); }
    dot(v) { return this.x * v.x + this.y * v.y + this.z * v.z + this.w * v.w; }
    length() { return Math.hypot(this.x, this.y, this.z, this.w); }
    lengthSqr() { return this.dot(this); }
    normalize() { const l = this.length() || 1; return new Vec4(this.x / l, this.y / l, this.z / l, this.w / l); }
    mix(v, t) {
        return new Vec4(
            this.x + (v.x - this.x) * t,
            this.y + (v.y - this.y) * t,
            this.z + (v.z - this.z) * t,
            this.w + (v.w - this.w) * t
        );
    }
    copy() { return new Vec4(this.x, this.y, this.z, this.w); }
    toString() { return `${this.x} ${this.y} ${this.z} ${this.w}`; }
}

globalThis.Vec2 = Vec2;
globalThis.Vec3 = Vec3;
globalThis.Vec4 = Vec4;
globalThis.shared = {};
globalThis.__layers = {};
globalThis.__scripts = {};
globalThis.__audioBuffers = {
    16: { left: new Float32Array(16), right: new Float32Array(16), average: new Float32Array(16) },
    32: { left: new Float32Array(32), right: new Float32Array(32), average: new Float32Array(32) },
    64: { left: new Float32Array(64), right: new Float32Array(64), average: new Float32Array(64) },
};

globalThis.engine = {
    frametime: 0.0166,
    runtime: 0.0,
    timeOfDay: 0.5,
    screenResolution: new Vec2(1920, 1080),
    canvasSize: new Vec2(1920, 1080),
    userProperties: {},
    registerAudioBuffers(res) {
        return globalThis.__audioBuffers[res] || globalThis.__audioBuffers[64];
    },
    isDesktopDevice() { return true; },
    isWallpaper() { return true; },
};

globalThis.input = {
    cursorWorldPosition: new Vec3(960, 540, 0),
    cursorScreenPosition: new Vec2(960, 540),
    cursorLeftDown: false,
};

globalThis.thisScene = {
    getLayer(name) { return globalThis.__layers[name] || null; },
    enumerateLayers() { return Object.values(globalThis.__layers); },
};

globalThis.console = {
    log(...a) { __log('info', a.map(String).join(' ')); },
    info(...a) { __log('info', a.map(String).join(' ')); },
    warn(...a) { __log('warn', a.map(String).join(' ')); },
    error(...a) { __log('error', a.map(String).join(' ')); },
};

// Host-side helper: call `fname(arg)` on every slot that exports it; one failing script
// must not take down the others.
globalThis.__callAll = function (fname, arg) {
    for (const [k, slot] of Object.entries(globalThis.__scripts)) {
        const f = slot.exports[fname];
        if (typeof f !== 'function') continue;
        try {
            f(arg);
        } catch (e) {
            __log('error', `[${k}] ${fname}: ${e}`);
        }
    }
};
"#;

/// Lifecycle callbacks that are lifted from a script's top level into `exports`.
const LIFECYCLE_FNS: [&str; 8] = [
    "init",
    "update",
    "cursorMove",
    "cursorDown",
    "cursorUp",
    "cursorClick",
    "applyUserProperties",
    "destroy",
];

/// Desktop cursor events forwarded to `cursorMove/Down/Up/Click`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorEventKind {
    Move,
    Down,
    Up,
    Click,
}

impl CursorEventKind {
    fn callback(self) -> &'static str {
        match self {
            Self::Move => "cursorMove",
            Self::Down => "cursorDown",
            Self::Up => "cursorUp",
            Self::Click => "cursorClick",
        }
    }
}

/// Layer state as left behind by scripts (read back by the renderer each frame).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct LayerState {
    pub name: String,
    pub origin: [f32; 3],
    pub scale: [f32; 3],
    pub angles: [f32; 3],
    pub size: [f32; 2],
    pub color: [f32; 3],
    pub alpha: f32,
    pub visible: bool,
    /// `thisLayer.text` (text layers; empty for other layers).
    #[serde(default)]
    pub text: String,
}

pub fn create_sandboxed_runtime() -> rquickjs::Result<(Runtime, Context)> {
    let rt = Runtime::new()?;
    rt.set_memory_limit(MEMORY_LIMIT_BYTES);
    let ctx = Context::full(&rt)?;
    Ok((rt, ctx))
}

/// Formats a float as a JS numeric literal (`NaN`/`inf` become `0`).
fn js_num(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".to_string()
    }
}

fn js_str(s: &str) -> String {
    // JSON string literals are valid JS string literals.
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn js_err(ctx: &Ctx<'_>, e: rquickjs::Error) -> anyhow::Error {
    anyhow::anyhow!("{}", CaughtError::from_error(ctx, e))
}

fn eval_js<'js, T>(ctx: &Ctx<'js>, src: impl Into<Vec<u8>>) -> anyhow::Result<T>
where
    T: rquickjs::FromJs<'js>,
{
    ctx.eval::<T, _>(src).map_err(|e| js_err(ctx, e))
}

/// Turns Wallpaper Engine's ES-module style script (`export function update(v) {...}`) into a
/// plain function body: strips `export` keywords and reports exported identifiers.
fn prepare_script(raw: &str) -> (String, Vec<String>) {
    let mut body = String::with_capacity(raw.len() + 64);
    let mut exported: Vec<String> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("export ") {
            let rest = rest.trim_start();
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            let name_part = ["function*", "function", "var", "let", "const", "class"]
                .iter()
                .find_map(|kw| rest.strip_prefix(kw));
            if let Some(after_kw) = name_part {
                let name: String = after_kw
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                    .collect();
                if !name.is_empty() {
                    exported.push(name);
                    let indent = &line[..line.len() - trimmed.len()];
                    body.push_str(indent);
                    body.push_str(&trimmed["export ".len()..]);
                    body.push('\n');
                    continue;
                }
            }
        }
        body.push_str(line);
        body.push('\n');
    }
    (body, exported)
}

pub struct SceneScriptEngine {
    _runtime: Runtime,
    context: Context,
    /// Deadline read by the QuickJS interrupt handler; `None` while no host call is running.
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl SceneScriptEngine {
    pub fn new(scene_w: f32, scene_h: f32, monitor_w: f32, monitor_h: f32) -> anyhow::Result<Self> {
        let (runtime, context) = create_sandboxed_runtime()?;
        let deadline: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let deadline_for_handler = Arc::clone(&deadline);
        runtime.set_interrupt_handler(Some(Box::new(
            move || matches!(*deadline_for_handler.lock(), Some(d) if Instant::now() > d),
        )));

        context.with(|ctx| -> anyhow::Result<()> {
            let log = Function::new(ctx.clone(), |level: String, msg: String| {
                match level.as_str() {
                    "error" => tracing::error!(target: "scenescript", "{msg}"),
                    "warn" => tracing::warn!(target: "scenescript", "{msg}"),
                    _ => tracing::info!(target: "scenescript", "{msg}"),
                }
            })
            .map_err(|e| js_err(&ctx, e))?;
            ctx.globals()
                .set("__log", log)
                .map_err(|e| js_err(&ctx, e))?;
            eval_js::<()>(&ctx, SCENESCRIPT_BOOTSTRAP_JS)?;
            eval_js::<()>(
                &ctx,
                format!(
                    "engine.canvasSize = new Vec2({}, {});
                     engine.screenResolution = new Vec2({}, {});",
                    js_num(scene_w),
                    js_num(scene_h),
                    js_num(monitor_w),
                    js_num(monitor_h)
                ),
            )?;
            Ok(())
        })?;

        Ok(Self {
            _runtime: runtime,
            context,
            deadline,
        })
    }

    /// Runs `f` inside the context with a wall-clock budget enforced by the interrupt handler.
    fn guarded<T>(
        &self,
        budget: Duration,
        f: impl FnOnce(&Ctx<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        *self.deadline.lock() = Some(Instant::now() + budget);
        let out = self.context.with(|ctx| f(&ctx));
        *self.deadline.lock() = None;
        out
    }

    pub fn register_layer(
        &self,
        layer_name: &str,
        origin: [f32; 3],
        scale: [f32; 3],
        angles: [f32; 3],
        alpha: f32,
        visible: bool,
    ) -> anyhow::Result<()> {
        self.guarded(INIT_BUDGET, |ctx| {
            let js = format!(
                "globalThis.__layers[{name}] = {{
                    name: {name},
                    origin: new Vec3({}, {}, {}),
                    scale: new Vec3({}, {}, {}),
                    angles: new Vec3({}, {}, {}),
                    size: new Vec2(0, 0),
                    color: new Vec3(1, 1, 1),
                    alpha: {},
                    visible: {},
                    text: '',
                    getEffect() {{ return {{ visible: true }}; }}
                }};",
                js_num(origin[0]),
                js_num(origin[1]),
                js_num(origin[2]),
                js_num(scale[0]),
                js_num(scale[1]),
                js_num(scale[2]),
                js_num(angles[0]),
                js_num(angles[1]),
                js_num(angles[2]),
                js_num(alpha),
                visible,
                name = js_str(layer_name),
            );
            eval_js::<()>(ctx, js)
        })
    }

    /// Reads a layer's current state (after scripts ran) for the renderer.
    pub fn layer_state(&self, layer_name: &str) -> anyhow::Result<Option<LayerState>> {
        self.guarded(INIT_BUDGET, |ctx| {
            let js = format!(
                "(function() {{
                    const l = globalThis.__layers[{name}];
                    if (!l) return 'null';
                    const v3 = (v) => [v.x, v.y, v.z];
                    return JSON.stringify({{
                        name: l.name,
                        origin: v3(l.origin), scale: v3(l.scale), angles: v3(l.angles),
                        size: [l.size.x, l.size.y], color: v3(l.color),
                        alpha: Number(l.alpha), visible: !!l.visible,
                        text: l.text == null ? '' : String(l.text),
                    }});
                }})()",
                name = js_str(layer_name)
            );
            let json: String = eval_js(ctx, js)?;
            Ok(serde_json::from_str::<Option<LayerState>>(&json)?)
        })
    }

    /// Reads every registered layer's state in one call (registration order).
    pub fn layer_states(&self) -> anyhow::Result<Vec<LayerState>> {
        self.guarded(FRAME_BUDGET, |ctx| {
            let json: String = eval_js(
                ctx,
                "(function() {
                    const f = (n) => { n = Number(n); return Number.isFinite(n) ? n : 0; };
                    const v3 = (v) => [f(v?.x), f(v?.y), f(v?.z)];
                    return JSON.stringify(Object.values(globalThis.__layers).map((l) => ({
                        name: String(l.name),
                        origin: v3(l.origin), scale: v3(l.scale), angles: v3(l.angles),
                        size: [f(l.size?.x), f(l.size?.y)], color: v3(l.color),
                        alpha: f(l.alpha), visible: !!l.visible,
                        text: l.text == null ? '' : String(l.text),
                    })));
                })()",
            )?;
            Ok(serde_json::from_str::<Vec<LayerState>>(&json)?)
        })
    }

    /// Writes host-side layer state into the existing `thisLayer` / `thisScene.getLayer()`
    /// objects (the layer object keeps its identity, so references captured by scripts stay
    /// valid; vector fields get fresh `Vec2`/`Vec3` values so they never alias a slot's
    /// `currentValue`). Unknown names are ignored.
    pub fn sync_layer_states(&self, states: &[LayerState]) -> anyhow::Result<()> {
        if states.is_empty() {
            return Ok(());
        }
        let v3 = |v: [f32; 3]| format!("[{},{},{}]", js_num(v[0]), js_num(v[1]), js_num(v[2]));
        let rows: Vec<String> = states
            .iter()
            .map(|s| {
                format!(
                    "[{},{},{},{},[{},{}],{},{},{},{}]",
                    js_str(&s.name),
                    v3(s.origin),
                    v3(s.scale),
                    v3(s.angles),
                    js_num(s.size[0]),
                    js_num(s.size[1]),
                    v3(s.color),
                    js_num(s.alpha),
                    s.visible,
                    js_str(&s.text)
                )
            })
            .collect();
        self.guarded(FRAME_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "(function() {{
                        const v3 = (a) => new Vec3(a[0], a[1], a[2]);
                        for (const [name, o, s, a, sz, c, alpha, vis, text] of [{rows}]) {{
                            const l = globalThis.__layers[name];
                            if (!l) continue;
                            l.origin = v3(o); l.scale = v3(s); l.angles = v3(a); l.color = v3(c);
                            l.size = new Vec2(sz[0], sz[1]);
                            l.alpha = alpha;
                            l.visible = vis;
                            l.text = text;
                        }}
                    }})()",
                    rows = rows.join(","),
                ),
            )
        })
    }

    /// Compiles a property script and runs its `init`. Returns the stringified initial value.
    pub fn register_property_script(
        &self,
        slot_key: &str,
        layer_name: &str,
        raw_es_script: &str,
        initial_value_expr: &str,
    ) -> anyhow::Result<String> {
        self.register_property_script_with_props(
            slot_key,
            layer_name,
            raw_es_script,
            initial_value_expr,
            "{}",
        )
    }

    /// Like [`Self::register_property_script`], with the instance-level `scriptproperties`
    /// (`{ name: value }`, user bindings already resolved) that override the defaults declared
    /// through `createScriptProperties()`.
    pub fn register_property_script_with_props(
        &self,
        slot_key: &str,
        layer_name: &str,
        raw_es_script: &str,
        initial_value_expr: &str,
        script_props_json: &str,
    ) -> anyhow::Result<String> {
        let script_props: serde_json::Value = serde_json::from_str(script_props_json)?;
        let script_props = serde_json::to_string(&script_props)?;
        let (valid_body, exported) = prepare_script(raw_es_script);
        let mut export_names: Vec<&str> = LIFECYCLE_FNS.to_vec();
        for n in &exported {
            if !export_names.contains(&n.as_str()) {
                export_names.push(n);
            }
        }
        let export_assigns: String = export_names
            .iter()
            .map(|n| format!("if (typeof {n} !== 'undefined') exports.{n} = {n};\n"))
            .collect();

        self.guarded(INIT_BUDGET, |ctx| {
            let wrapper = format!(
                "(function() {{
                    const exports = {{}};
                    const thisLayer = globalThis.__layers[{layer_name}] || {{}};
                    const __spStore = {{}};
                    var scriptProperties = __spStore;
                    const __sp = {script_props};
                    const __pick = function(o, fallback) {{
                        return Object.prototype.hasOwnProperty.call(__sp, o.name) ? __sp[o.name] : fallback;
                    }};
                    const createScriptProperties = function() {{
                        const builder = {{
                            addSlider(o) {{ __spStore[o.name] = __pick(o, o.value); return builder; }},
                            addCheckbox(o) {{ __spStore[o.name] = __pick(o, o.value); return builder; }},
                            addColor(o) {{ __spStore[o.name] = new Vec3(__pick(o, o.value)); return builder; }},
                            addText(o) {{ __spStore[o.name] = __pick(o, o.value); return builder; }},
                            addCombo(o) {{ __spStore[o.name] = __pick(o, o.value ?? o.options?.[0]?.value ?? 0); return builder; }},
                            finish() {{ return __spStore; }}
                        }};
                        return builder;
                    }};
                    {valid_body}
                    {export_assigns}
                    let val = {initial_value_expr};
                    if (typeof exports.init === 'function') {{
                        const r = exports.init(val);
                        if (r !== undefined) val = r;
                    }}
                    globalThis.__scripts[{slot_key}] = {{ exports, thisLayer, currentValue: val, scriptProperties: __spStore }};
                    return String(val);
                }})()",
                layer_name = js_str(layer_name),
                slot_key = js_str(slot_key),
            );
            eval_js::<String>(ctx, wrapper)
        })
    }

    /// Merges new instance-level `scriptproperties` (`{ name: value }`) into the live
    /// `scriptProperties` object of one registered slot. Unknown slots are ignored.
    pub fn update_script_properties(
        &self,
        slot_key: &str,
        script_props_json: &str,
    ) -> anyhow::Result<()> {
        let parsed: serde_json::Value = serde_json::from_str(script_props_json)?;
        let canonical = serde_json::to_string(&parsed)?;
        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "(function() {{
                        const s = globalThis.__scripts[{slot}];
                        if (!s || !s.scriptProperties) return;
                        const props = {canonical};
                        for (const k of Object.keys(props)) {{
                            const cur = s.scriptProperties[k];
                            const isVec = cur !== undefined && cur !== null && typeof cur === 'object' && 'z' in cur;
                            s.scriptProperties[k] = isVec ? new Vec3(props[k]) : props[k];
                        }}
                    }})()",
                    slot = js_str(slot_key),
                ),
            )
        })
    }

    /// Updates cursor/time inputs that are not part of [`Self::step_frame`].
    pub fn set_input_state(
        &self,
        cursor_screen: [f32; 2],
        cursor_left_down: bool,
        time_of_day: f32,
    ) -> anyhow::Result<()> {
        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "input.cursorScreenPosition.x = {}; input.cursorScreenPosition.y = {};
                     input.cursorLeftDown = {}; engine.timeOfDay = {};",
                    js_num(cursor_screen[0]),
                    js_num(cursor_screen[1]),
                    cursor_left_down,
                    js_num(time_of_day)
                ),
            )
        })
    }

    /// Forwards a desktop cursor event (`cursorMove/Down/Up/Click`) to all scripts.
    /// `world_x`/`world_y` are scene coordinates (see `input.cursorWorldPosition`).
    pub fn dispatch_cursor_event(
        &self,
        kind: CursorEventKind,
        world_x: f32,
        world_y: f32,
    ) -> anyhow::Result<()> {
        let left_down_update = match kind {
            CursorEventKind::Down => "input.cursorLeftDown = true;",
            CursorEventKind::Up => "input.cursorLeftDown = false;",
            _ => "",
        };
        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "input.cursorWorldPosition.x = {x}; input.cursorWorldPosition.y = {y};
                     {left_down_update}
                     __callAll('{cb}', {{ x: {x}, y: {y}, worldPosition: new Vec3({x}, {y}, 0) }});",
                    x = js_num(world_x),
                    y = js_num(world_y),
                    cb = kind.callback(),
                ),
            )
        })
    }

    /// Calls `applyUserProperties(changedProps)` on every script and merges the values into
    /// `engine.userProperties`. `changed_props_json` is a JSON object of changed properties.
    pub fn apply_user_properties(&self, changed_props_json: &str) -> anyhow::Result<()> {
        let parsed: serde_json::Value = serde_json::from_str(changed_props_json)?;
        let canonical = serde_json::to_string(&parsed)?;
        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "(function() {{
                        const props = {canonical};
                        Object.assign(engine.userProperties, props);
                        __callAll('applyUserProperties', props);
                    }})()"
                ),
            )
        })
    }

    pub fn step_frame(
        &self,
        dt: f32,
        runtime: f32,
        cursor_world: [f32; 3],
        left_64: &[f32; 64],
        right_64: &[f32; 64],
    ) -> anyhow::Result<HashMap<String, String>> {
        let csv = |a: &[f32; 64]| {
            a.iter()
                .map(|v| format!("{:.4}", if v.is_finite() { *v } else { 0.0 }))
                .collect::<Vec<_>>()
                .join(",")
        };
        let left_csv = csv(left_64);
        let right_csv = csv(right_64);
        self.guarded(FRAME_BUDGET, |ctx| {
            let step_js = format!(
                "(function() {{
                    engine.frametime = {dt};
                    engine.runtime = {runtime};
                    input.cursorWorldPosition.x = {cx};
                    input.cursorWorldPosition.y = {cy};
                    const L = [{left_csv}];
                    const R = [{right_csv}];
                    for (const res of [16, 32, 64]) {{
                        const stride = 64 / res;
                        const buf = globalThis.__audioBuffers[res];
                        for (let i = 0; i < res; i++) {{
                            const l = L[i * stride] || 0;
                            const r = R[i * stride] || 0;
                            buf.left[i] = l;
                            buf.right[i] = r;
                            buf.average[i] = (l + r) * 0.5;
                        }}
                    }}
                    const updated = {{}};
                    for (const [k, slot] of Object.entries(globalThis.__scripts)) {{
                        if (typeof slot.exports.update === 'function') {{
                            try {{
                                const next = slot.exports.update(slot.currentValue);
                                if (next !== undefined) slot.currentValue = next;
                            }} catch (e) {{
                                __log('error', `[${{k}}] update: ${{e}}`);
                            }}
                        }}
                        updated[k] = String(slot.currentValue);
                    }}
                    return JSON.stringify(updated);
                }})()",
                dt = js_num(dt),
                runtime = js_num(runtime),
                cx = js_num(cursor_world[0]),
                cy = js_num(cursor_world[1]),
            );
            let json_out: String = eval_js(ctx, step_js)?;
            Ok(serde_json::from_str::<HashMap<String, String>>(&json_out)?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z64: [f32; 64] = [0.0; 64];

    fn engine() -> SceneScriptEngine {
        SceneScriptEngine::new(1920.0, 1080.0, 2560.0, 1440.0).expect("engine")
    }

    fn step(e: &SceneScriptEngine) -> HashMap<String, String> {
        e.step_frame(0.016, 1.0, [10.0, 20.0, 0.0], &Z64, &Z64)
            .expect("step")
    }

    #[test]
    fn initializes_rquickjs_sandbox() {
        let (_rt, ctx) = create_sandboxed_runtime().expect("init quickjs");
        ctx.with(|ctx| {
            let val: i32 = ctx.eval("2 + 2").expect("eval");
            assert_eq!(val, 4);
        });
    }

    #[test]
    fn init_and_update_run() {
        let e = engine();
        let init = e
            .register_property_script(
                "n",
                "L",
                "'use strict';\nexport function init(v) { return v + 10; }\n\
                 export function update(v) { return v + 1; }",
                "0",
            )
            .unwrap();
        assert_eq!(init, "10");
        assert_eq!(step(&e)["n"], "11");
        assert_eq!(step(&e)["n"], "12");
    }

    #[test]
    fn vec2_add_works() {
        let e = engine();
        e.register_property_script(
            "v",
            "L",
            "export function update(v) { return new Vec2(1, 2).add(new Vec2(3, 4)); }",
            "new Vec2(0, 0)",
        )
        .unwrap();
        assert_eq!(step(&e)["v"], "4 6");
    }

    #[test]
    fn vec_math_and_string_parsing() {
        let e = engine();
        e.register_property_script(
            "x",
            "L",
            "export function update(v) {
                const c = new Vec3(1, 0, 0).cross(new Vec3(0, 1, 0));
                const m = new Vec2('0 0').mix(new Vec2(10, 20), 0.5);
                const n = new Vec2(3, 4).normalize();
                const w = new Vec4(1, 2, 3, 4).add(new Vec4(1, 1, 1, 1));
                return [c, m, n.length(), new Vec2(3, 4).length(), w].join('|');
            }",
            "0",
        )
        .unwrap();
        assert_eq!(step(&e)["x"], "0 0 1|5 10|1|5|2 3 4 5");
    }

    #[test]
    fn helper_functions_and_script_properties_work() {
        let e = engine();
        e.register_property_script(
            "p",
            "L",
            "const props = createScriptProperties().addSlider({ name: 'speed', value: 3 }).finish();
             function double(x) { return x * 2; }
             export function update(v) { return double(props.speed); }",
            "0",
        )
        .unwrap();
        assert_eq!(step(&e)["p"], "6");
    }

    #[test]
    fn engine_globals_are_updated_each_frame() {
        let e = engine();
        e.register_property_script(
            "g",
            "L",
            "export function update(v) {
                return [engine.frametime, engine.runtime, input.cursorWorldPosition.x,
                        input.cursorWorldPosition.y, engine.canvasSize, engine.screenResolution].join(',');
            }",
            "0",
        )
        .unwrap();
        let out = e
            .step_frame(0.5, 2.5, [10.0, 20.0, 0.0], &Z64, &Z64)
            .unwrap();
        assert_eq!(out["g"], "0.5,2.5,10,20,1920 1080,2560 1440");
    }

    #[test]
    fn register_audio_buffers_reflect_spectrum() {
        let e = engine();
        e.register_property_script(
            "a",
            "L",
            "const b16 = engine.registerAudioBuffers(16);
             const b64 = engine.registerAudioBuffers(64);
             export function update(v) {
                return [b16.left[1], b16.right[1], b16.average[1], b64.left[4], b16.left.length].join(',');
             }",
            "0",
        )
        .unwrap();
        let mut l = [0.0_f32; 64];
        let mut r = [0.0_f32; 64];
        l[4] = 0.5; // 16-band index 1 samples 64-band index 4
        r[4] = 0.25;
        let out = e.step_frame(0.016, 0.0, [0.0; 3], &l, &r).unwrap();
        assert_eq!(out["a"], "0.5,0.25,0.375,0.5,16");
    }

    #[test]
    fn this_layer_and_this_scene_read_and_write() {
        let e = engine();
        e.register_layer("hero", [1.0, 2.0, 3.0], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_layer("bg", [0.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_property_script(
            "s",
            "hero",
            "export function update(v) {
                thisLayer.alpha = 0.25;
                thisLayer.origin = thisLayer.origin.add(new Vec3(1, 1, 1));
                thisScene.getLayer('bg').visible = false;
                return thisScene.enumerateLayers().length + ':' + thisLayer.name;
            }",
            "0",
        )
        .unwrap();
        assert_eq!(step(&e)["s"], "2:hero");

        let hero = e.layer_state("hero").unwrap().unwrap();
        assert_eq!(hero.alpha, 0.25);
        assert_eq!(hero.origin, [2.0, 3.0, 4.0]);
        assert!(hero.visible);
        assert!(!e.layer_state("bg").unwrap().unwrap().visible);
        assert!(e.layer_state("missing").unwrap().is_none());
    }

    #[test]
    fn cursor_events_reach_scripts() {
        let e = engine();
        e.register_property_script(
            "c",
            "L",
            "let last = 'none';
             export function cursorMove(ev) { last = 'move@' + ev.x + ',' + ev.y; }
             export function cursorDown(ev) { last = 'down:' + input.cursorLeftDown; }
             export function cursorClick(ev) { last = 'click'; }
             export function update(v) { return last; }",
            "0",
        )
        .unwrap();
        e.dispatch_cursor_event(CursorEventKind::Move, 100.0, 200.0)
            .unwrap();
        assert_eq!(step(&e)["c"], "move@100,200");
        e.dispatch_cursor_event(CursorEventKind::Down, 1.0, 2.0)
            .unwrap();
        assert_eq!(step(&e)["c"], "down:true");
        e.dispatch_cursor_event(CursorEventKind::Click, 1.0, 2.0)
            .unwrap();
        assert_eq!(step(&e)["c"], "click");
    }

    #[test]
    fn apply_user_properties_is_delivered() {
        let e = engine();
        e.register_property_script(
            "u",
            "L",
            "let tint = 'unset';
             export function applyUserProperties(p) { if (p.tint !== undefined) tint = p.tint; }
             export function update(v) { return tint + '/' + engine.userProperties.tint; }",
            "0",
        )
        .unwrap();
        e.apply_user_properties(r#"{"tint":"1 0 0"}"#).unwrap();
        assert_eq!(step(&e)["u"], "1 0 0/1 0 0");
        assert!(e.apply_user_properties("not json").is_err());
    }

    #[test]
    fn shared_state_is_visible_across_scripts() {
        let e = engine();
        e.register_property_script(
            "w",
            "L",
            "export function update(v) { shared.count = (shared.count || 0) + 1; return v; }",
            "0",
        )
        .unwrap();
        e.register_property_script(
            "r",
            "L",
            "export function update(v) { return shared.count; }",
            "0",
        )
        .unwrap();
        assert_eq!(step(&e)["r"], "1");
    }

    #[test]
    fn failing_script_does_not_break_other_slots() {
        let e = engine();
        e.register_property_script(
            "bad",
            "L",
            "export function update(v) { throw new Error('boom'); }",
            "7",
        )
        .unwrap();
        e.register_property_script(
            "good",
            "L",
            "export function update(v) { return v + 1; }",
            "0",
        )
        .unwrap();
        let out = step(&e);
        assert_eq!(out["bad"], "7");
        assert_eq!(out["good"], "1");
    }

    #[test]
    fn syntax_error_is_reported() {
        let e = engine();
        let err = e
            .register_property_script("x", "L", "export function update( {", "0")
            .unwrap_err();
        assert!(err.to_string().contains("eval_script"), "{err}");
    }

    #[test]
    fn runaway_script_is_interrupted() {
        let e = engine();
        let started = Instant::now();
        let res = e.register_property_script(
            "loop",
            "L",
            "export function init(v) { while (true) {} }",
            "0",
        );
        assert!(res.is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
        // Runtime remains usable afterwards.
        e.register_property_script("ok", "L", "export function update(v) { return 1; }", "0")
            .unwrap();
        assert_eq!(step(&e)["ok"], "1");
    }

    #[test]
    fn layer_states_and_sync_round_trip() {
        let e = engine();
        e.register_layer("a", [1.0, 2.0, 3.0], [1.0; 3], [0.0; 3], 0.5, true)
            .unwrap();
        e.register_layer("b", [0.0; 3], [2.0; 3], [0.0, 0.0, 90.0], 1.0, false)
            .unwrap();
        let states = e.layer_states().unwrap();
        assert_eq!(
            states.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(states[0].origin, [1.0, 2.0, 3.0]);
        assert_eq!(states[0].alpha, 0.5);
        assert_eq!(states[1].angles, [0.0, 0.0, 90.0]);
        assert!(!states[1].visible);

        // A script captures `thisLayer`; syncing must keep that reference live.
        e.register_property_script(
            "s",
            "a",
            "export function update(v) { return thisLayer.origin.x + ':' + thisLayer.size.x; }",
            "0",
        )
        .unwrap();
        let mut a = states[0].clone();
        a.origin = [7.5, 8.0, 9.0];
        a.size = [300.0, 200.0];
        a.color = [0.0, 1.0, 0.0];
        let ghost = LayerState {
            name: "ghost \"quoted\"".into(),
            ..a.clone()
        };
        e.sync_layer_states(&[a.clone(), ghost]).unwrap();
        assert_eq!(step(&e)["s"], "7.5:300");
        assert_eq!(e.layer_state("a").unwrap().unwrap(), a);
        assert_eq!(e.layer_states().unwrap().len(), 2, "unknown names ignored");
        e.sync_layer_states(&[]).unwrap();

        // Non-finite script writes read back as 0 instead of breaking the JSON round trip.
        e.register_property_script(
            "inf",
            "b",
            "export function update(v) { thisLayer.alpha = Infinity; thisLayer.origin.x = NaN; return v; }",
            "0",
        )
        .unwrap();
        step(&e);
        let b = &e.layer_states().unwrap()[1];
        assert_eq!((b.alpha, b.origin[0]), (0.0, 0.0));
    }

    #[test]
    fn layer_text_round_trips_and_scripts_can_set_it() {
        let e = engine();
        e.register_layer("clock", [0.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        assert_eq!(e.layer_states().unwrap()[0].text, "");
        let mut s = e.layer_states().unwrap()[0].clone();
        s.text = "12:\"34\"".into();
        e.sync_layer_states(&[s.clone()]).unwrap();
        assert_eq!(e.layer_state("clock").unwrap().unwrap().text, s.text);

        e.register_property_script(
            "t",
            "clock",
            "export function update(v) { thisLayer.text = 'tick ' + 7; return v; }",
            "0",
        )
        .unwrap();
        step(&e);
        assert_eq!(e.layer_states().unwrap()[0].text, "tick 7");
    }

    #[test]
    fn prepare_script_strips_export_and_collects_names() {
        let (body, names) =
            prepare_script("  export function update(v) {}\nexport let a = 1;\nconst x = 2;");
        assert_eq!(names, vec!["update", "a"]);
        assert!(body.contains("  function update(v) {}"));
        assert!(body.contains("let a = 1;"));
        assert!(!body.contains("export"));
    }
}
