//! `truongpaper-scenescript`: Embedded `rquickjs` QuickJS sandbox implementing Wallpaper Engine's
//! SceneScript runtime (`engine`, `input`, `thisLayer`, `thisScene`, `Vec2/Vec3/Vec4`, `shared`).
//! Complete specification: `docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` (Sections 8.1-8.3).
//!
//! Lifecycle callbacks: `init(value)`, `update(value)`, `cursorMove/Down/Up/Click(event)`,
//! `applyUserProperties(changedProps)`. Each property script lives in a "slot" keyed by the host
//! (e.g. `"layer7.alpha"`); `update` runs for every slot each frame via [`SceneScriptEngine::step_frame`].

use parking_lot::Mutex;
use rquickjs::loader::{Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{CaughtError, Context, Ctx, Function, Module, Runtime};
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
    getLayer(name) {
        const l = globalThis.__layers[name];
        return l === undefined ? null : l;
    },
    getLayerByID(id) {
        for (const l of Object.values(globalThis.__layers)) if (l.id === id) return l;
        return null;
    },
    enumerateLayers() { return Object.values(globalThis.__layers).filter((l) => !l.__group); },
};

globalThis.console = {
    log(...a) { __log('info', a.map(String).join(' ')); },
    info(...a) { __log('info', a.map(String).join(' ')); },
    warn(...a) { __log('warn', a.map(String).join(' ')); },
    error(...a) { __log('error', a.map(String).join(' ')); },
};


for (const C of [Vec2, Vec3, Vec4]) {
    const keys = C === Vec2 ? ['x', 'y'] : C === Vec3 ? ['x', 'y', 'z'] : ['x', 'y', 'z', 'w'];
    const map = (self, f) => new C(...keys.map(k => f(self[k], k)));
    C.prototype.negate = function () { return map(this, v => -v); };
    C.prototype.abs = function () { return map(this, Math.abs); };
    C.prototype.round = function () { return map(this, Math.round); };
    C.prototype.floor = function () { return map(this, Math.floor); };
    C.prototype.ceil = function () { return map(this, Math.ceil); };
    C.prototype.sign = function () { return map(this, Math.sign); };
    C.prototype.min = function (o) { return map(this, (v, k) => Math.min(v, o[k] ?? o)); };
    C.prototype.max = function (o) { return map(this, (v, k) => Math.max(v, o[k] ?? o)); };
    C.prototype.equals = function (o) { return keys.every(k => this[k] === o[k]); };
    C.prototype.distance = function (o) { return this.subtract(o).length(); };
    C.prototype.toJSON = function () { return this.toString(); };
}
Vec3.prototype.xy = function () { return new Vec2(this.x, this.y); };
Object.defineProperty(Vec2.prototype, 'xy', { get() { return this.copy(); } });
Vec2.prototype.angle = function () { return Math.atan2(this.y, this.x); };
Vec3.prototype.reflect = function (n) { return this.subtract(n.multiply(2 * this.dot(n))); };

globalThis.__WEMath = {
    PI: Math.PI,
    TWO_PI: Math.PI * 2,
    mix: (a, b, t) => a + (b - a) * t,
    clamp: (v, lo, hi) => Math.min(Math.max(v, lo), hi),
    saturate: v => Math.min(Math.max(v, 0), 1),
    smoothStep: (lo, hi, v) => { const t = Math.min(Math.max((v - lo) / ((hi - lo) || 1), 0), 1); return t * t * (3 - 2 * t); },
    deg2rad: d => d * Math.PI / 180,
    rad2deg: r => r * 180 / Math.PI,
    sign: Math.sign,
    fract: v => v - Math.floor(v),
    step: (e, v) => (v < e ? 0 : 1),
};
globalThis.__WEColor = {
    rgb2hsv(c) {
        const r = c.x, g = c.y, b = c.z, mx = Math.max(r, g, b), mn = Math.min(r, g, b), d = mx - mn;
        let h = 0;
        if (d > 0) {
            if (mx === r) h = ((g - b) / d) % 6; else if (mx === g) h = (b - r) / d + 2; else h = (r - g) / d + 4;
            h /= 6; if (h < 0) h += 1;
        }
        return new Vec3(h, mx === 0 ? 0 : d / mx, mx);
    },
    hsv2rgb(c) {
        const h = ((c.x % 1) + 1) % 1 * 6, s = c.y, v = c.z, i = Math.floor(h), f = h - i;
        const p = v * (1 - s), q = v * (1 - f * s), t = v * (1 - (1 - f) * s);
        const t3 = [[v, t, p], [q, v, p], [p, v, t], [p, q, v], [t, p, v], [v, p, q]][i % 6];
        return new Vec3(t3[0], t3[1], t3[2]);
    },
    normalizeColor(c) { return new Vec3(c.x / 255, c.y / 255, c.z / 255); },
    expandColor(c) { return new Vec3(c.x * 255, c.y * 255, c.z * 255); },
    rgb2hex(c) {
        const h = v => Math.round(Math.min(Math.max(v, 0), 1) * 255).toString(16).padStart(2, '0');
        return '#' + h(c.x) + h(c.y) + h(c.z);
    },
    hsv2hex(c) { return globalThis.__WEColor.rgb2hex(globalThis.__WEColor.hsv2rgb(c)); },
};
(function () {
    const store = {};
    globalThis.localStorage = {
        get(k) { return store[k]; },
        set(k, v) { store[k] = v; },
        getItem(k) { return k in store ? store[k] : null; },
        setItem(k, v) { store[k] = String(v); },
        removeItem(k) { delete store[k]; },
        clear() { for (const k of Object.keys(store)) delete store[k]; },
    };
    globalThis.engine.AUDIO_RESOLUTION_16 = 16;
    globalThis.engine.AUDIO_RESOLUTION_32 = 32;
    globalThis.engine.AUDIO_RESOLUTION_64 = 64;
    globalThis.engine.openUserShortcut = function () {};
    // Timers run on engine.runtime and fire from `__fireTimers` each frame.
    const timers = new Map();
    let nextTimer = 1;
    const addTimer = (fn, ms, repeat) => {
        if (typeof fn !== 'function') return -1;
        const delay = Math.max(0, Number(ms) || 0) / 1000;
        const id = nextTimer++;
        timers.set(id, { fn, delay, repeat, due: globalThis.engine.runtime + delay });
        return id;
    };
    // Like Wallpaper Engine, the returned handle is a function that cancels the timer.
    const handle = (id) => { const stop = () => { timers.delete(id); }; stop.id = id; return stop; };
    globalThis.engine.setTimeout = (fn, ms) => handle(addTimer(fn, ms, false));
    globalThis.engine.setInterval = (fn, ms) => handle(addTimer(fn, ms, true));
    const clear = (h) => { if (typeof h === 'function') h(); else timers.delete(h); };
    globalThis.engine.clearTimeout = clear;
    globalThis.engine.clearInterval = clear;
    globalThis.__fireTimers = function () {
        const now = globalThis.engine.runtime;
        for (const [id, t] of [...timers]) {
            if (!timers.has(id) || t.due > now) continue;
            if (t.repeat) t.due = now + Math.max(t.delay, 0.001); else timers.delete(id);
            try { t.fn(); } catch (e) { __log('error', `[timer] ${e}`); }
        }
    };
    globalThis.engine.isScreensaver = function () { return false; };
    globalThis.engine.isRunningInEditor = function () { return false; };
    globalThis.engine.isPortrait = function () { return false; };
    globalThis.engine.isLandscape = function () { return true; };
})();


// Windows media-integration events: classes exist so scripts can reference them; the host
// never delivers media callbacks.
globalThis.MediaPlaybackEvent = class MediaPlaybackEvent {
    constructor(state) { this.state = state === undefined ? MediaPlaybackEvent.PLAYBACK_STOPPED : state; }
};
Object.assign(globalThis.MediaPlaybackEvent, { PLAYBACK_STOPPED: 0, PLAYBACK_PLAYING: 1, PLAYBACK_PAUSED: 2 });
globalThis.MediaPropertiesEvent = class MediaPropertiesEvent {
    constructor() { this.title = ''; this.artist = ''; this.subTitle = ''; this.albumTitle = ''; this.albumArtist = ''; this.genres = []; this.contentType = ''; }
};
globalThis.MediaThumbnailEvent = class MediaThumbnailEvent {
    constructor() { this.primaryColor = new Vec3(0, 0, 0); this.secondaryColor = new Vec3(0, 0, 0); this.tertiaryColor = new Vec3(0, 0, 0); this.highContrastColor = new Vec3(1, 1, 1); this.textColor = new Vec3(1, 1, 1); }
};
globalThis.MediaTimelineEvent = class MediaTimelineEvent {
    constructor() { this.position = 0; this.duration = 0; }
};

// Animation / hierarchy API: accepted and inert (the renderer drives animation itself).
globalThis.__animStub = function () {
    const a = {
        rate: 1, frame: 0, frameCount: 1, visible: true, name: '', duration: 0,
        play() {}, pause() {}, stop() {}, isPlaying() { return false; },
        setFrame(f) { a.frame = f; }, getFrame() { return a.frame; },
    };
    return a;
};
globalThis.__parents = {};
// 4x4 matrix, column-major like the engine's (`m[12..14]` is the translation).
globalThis.Mat4 = class Mat4 {
    constructor(m) { this.m = m ? Array.from(m) : [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]; }
    static identity() { return new Mat4(); }
    copy() { return new Mat4(this.m); }
    identity() { this.m = new Mat4().m; return this; }
    multiply(o) {
        const a = this.m, b = o.m, r = new Array(16).fill(0);
        for (let c = 0; c < 4; c++) for (let rw = 0; rw < 4; rw++) {
            for (let k = 0; k < 4; k++) r[c * 4 + rw] += a[k * 4 + rw] * b[c * 4 + k];
        }
        return new Mat4(r);
    }
    translation(v) { if (v === undefined) return new Vec3(this.m[12], this.m[13], this.m[14]); const r = this.copy(); r.m[12] += v.x; r.m[13] += v.y; r.m[14] += v.z; return r; }
    scale(v) { if (v === undefined) return new Vec3(Math.hypot(this.m[0], this.m[1], this.m[2]), Math.hypot(this.m[4], this.m[5], this.m[6]), Math.hypot(this.m[8], this.m[9], this.m[10])); const s = new Mat4(); s.m[0] = v.x; s.m[5] = v.y; s.m[10] = v.z; return this.multiply(s); }
    transformPoint(v) { const m = this.m; return new Vec3(m[0] * v.x + m[4] * v.y + m[8] * v.z + m[12], m[1] * v.x + m[5] * v.y + m[9] * v.z + m[13], m[2] * v.x + m[6] * v.y + m[10] * v.z + m[14]); }
    toString() { return this.m.join(' '); }
};
globalThis.__localMatrix = function (l) {
    const d = Math.PI / 180, a = l.angles || new Vec3(0, 0, 0), s = l.scale || new Vec3(1, 1, 1), o = l.origin || new Vec3(0, 0, 0);
    const cx = Math.cos(a.x * d), sx = Math.sin(a.x * d), cy = Math.cos(a.y * d), sy = Math.sin(a.y * d), cz = Math.cos(a.z * d), sz = Math.sin(a.z * d);
    // R = Rz * Ry * Rx
    const r00 = cz * cy, r01 = cz * sy * sx - sz * cx, r02 = cz * sy * cx + sz * sx;
    const r10 = sz * cy, r11 = sz * sy * sx + cz * cx, r12 = sz * sy * cx - cz * sx;
    const r20 = -sy, r21 = cy * sx, r22 = cy * cx;
    return new Mat4([r00 * s.x, r10 * s.x, r20 * s.x, 0, r01 * s.y, r11 * s.y, r21 * s.y, 0, r02 * s.z, r12 * s.z, r22 * s.z, 0, o.x, o.y, o.z, 1]);
};
globalThis.__videoStub = function () {
    const v = { _playing: false, _pos: 0, rate: 1, volume: 1, duration: 0 };
    v.play = function () { v._playing = true; };
    v.pause = function () { v._playing = false; };
    v.stop = function () { v._playing = false; v._pos = 0; };
    v.isPlaying = function () { return v._playing; };
    v.setCurrentTime = function (t) { v._pos = t; };
    v.getCurrentTime = function () { return v._pos; };
    return v;
};
globalThis.__layerStubs = function () {
    return {
        getAnimation() { return globalThis.__animStub(); },
        getAnimationLayer() { return globalThis.__animStub(); },
        getTextureAnimation() { return globalThis.__animStub(); },
        play() {}, stop() {}, pause() {}, isPlaying() { return false; },
        setVolume() {}, getVolume() { return 1; }, volume: 1,
        getTransformMatrix() {
            let m = globalThis.__localMatrix(this), cur = globalThis.__parents[this.name], hops = 0;
            while (cur !== undefined && globalThis.__layers[cur] && hops++ < 256) {
                m = globalThis.__localMatrix(globalThis.__layers[cur]).multiply(m);
                cur = globalThis.__parents[cur];
            }
            return m;
        },
        getVideoTexture() { return this.__video || (this.__video = globalThis.__videoStub()); },
        getParent() {
            const p = globalThis.__parents[this.name];
            return p === undefined ? null : (globalThis.__layers[p] || null);
        },
        getChildren() {
            const me = this.name;
            return Object.keys(globalThis.__parents)
                .filter((k) => globalThis.__parents[k] === me && globalThis.__layers[k])
                .map((k) => globalThis.__layers[k]);
        },
        getBoundingBox() { return null; },
        getInitialLayer() { return this; },
        setParent() {},
        destroy() {},
        localToWorld(p) { return p; },
        worldToLocal(p) { return p; },
    };
};
{
    const stubLayer = (name) => Object.assign({
        name, origin: new Vec3(0, 0, 0), scale: new Vec3(1, 1, 1), angles: new Vec3(0, 0, 0),
        size: new Vec2(0, 0), color: new Vec3(1, 1, 1), alpha: 1, visible: true, text: '',
        getEffect() { return { visible: true }; },
    }, globalThis.__layerStubs());
    globalThis.thisScene.getAnimation = function () { return globalThis.__animStub(); };
    globalThis.thisScene.destroyLayer = function () {};
    globalThis.thisScene.createLayer = function (d) { return stubLayer(typeof d === 'string' ? d : 'layer'); };
    globalThis.thisScene.sortLayer = function () {};
    globalThis.thisScene.getLayerIndex = function () { return 0; };
    globalThis.thisScene.bloom = {}; globalThis.thisScene.camera = {};
    globalThis.thisScene.clearColor = new Vec3(0, 0, 0);
}
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
    rt.set_loader(WeModuleResolver, WeModuleLoader);
    let ctx = Context::full(&rt)?;
    Ok((rt, ctx))
}

/// Resolves import specifiers to themselves; only the built-in WE modules exist.
struct WeModuleResolver;

impl Resolver for WeModuleResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
    ) -> rquickjs::Result<String> {
        if we_module_source(name).is_some() {
            Ok(name.to_string())
        } else {
            Err(rquickjs::Error::new_resolving(base, name))
        }
    }
}

struct WeModuleLoader;

impl Loader for WeModuleLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Module<'js, Declared>> {
        match we_module_source(name) {
            Some(src) => Module::declare(ctx.clone(), name, src),
            None => Err(rquickjs::Error::new_loading(name)),
        }
    }
}

/// Source of the Wallpaper Engine script modules (`import * as WEMath from 'WEMath'`).
fn we_module_source(name: &str) -> Option<&'static str> {
    Some(match name {
        "WEMath" => {
            "const M = globalThis.__WEMath; export default M;\n\
             export const mix = M.mix, smoothStep = M.smoothStep, clamp = M.clamp, deg2rad = M.deg2rad, rad2deg = M.rad2deg, sign = M.sign, saturate = M.saturate, fract = M.fract, step = M.step, PI = M.PI, TWO_PI = M.TWO_PI;"
        }
        "WEVector" => {
            "export const Vec2 = globalThis.Vec2, Vec3 = globalThis.Vec3, Vec4 = globalThis.Vec4;\n\
             export default { Vec2, Vec3, Vec4 };"
        }
        "WEColor" => {
            "const C = globalThis.__WEColor; export default C;\n\
             export const rgb2hsv = C.rgb2hsv, hsv2rgb = C.hsv2rgb, normalizeColor = C.normalizeColor, expandColor = C.expandColor, hsv2hex = C.hsv2hex, rgb2hex = C.rgb2hex;"
        }
        _ => return None,
    })
}

/// Loads `source` as a real ES module (so `export`, `import`, arrow functions, classes, ... all
/// work) and returns its namespace object. Per-slot globals are provided through a preamble that
/// reads `globalThis.__cur`, which the caller sets right before.
fn load_script_module<'js>(
    ctx: &Ctx<'js>,
    unique: usize,
    source: &str,
) -> anyhow::Result<rquickjs::Object<'js>> {
    let full = format!(
        "let thisObject = globalThis.__cur.thisLayer, thisLayer = thisObject;\nconst createScriptProperties = globalThis.__cur.createScriptProperties;\n{source}"
    );
    let module = Module::declare(ctx.clone(), format!("__slot{unique}"), full)
        .map_err(|e| js_err(ctx, e))?;
    let (module, promise) = module.eval().map_err(|e| js_err(ctx, e))?;
    promise.finish::<()>().map_err(|e| js_err(ctx, e))?;
    module.namespace().map_err(|e| js_err(ctx, e))
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

    /// Records the layer hierarchy (`(child key, parent key)`) for `getParent()/getChildren()`.
    /// `group_keys` are transform-only parents: reachable through `getParent()` and
    /// `thisScene.getLayer()` but not listed by `enumerateLayers()`.
    pub fn set_layer_hierarchy(
        &self,
        parents: &[(String, String)],
        group_keys: &[String],
        ids: &[(String, i32)],
    ) -> anyhow::Result<()> {
        let ids_json = serde_json::to_string(ids)?;
        let map: std::collections::BTreeMap<&str, &str> = parents
            .iter()
            .map(|(c, p)| (c.as_str(), p.as_str()))
            .collect();
        let map_json = serde_json::to_string(&map)?;
        let groups_json = serde_json::to_string(group_keys)?;
        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "globalThis.__parents = {map_json};
                     for (const [k, id] of {ids_json}) if (globalThis.__layers[k]) globalThis.__layers[k].id = id;
                     for (const g of {groups_json}) if (globalThis.__layers[g]) globalThis.__layers[g].__group = true;"
                ),
            )
        })
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
                    getEffect() {{ return {{ visible: true }}; }},
                    ...__layerStubs()
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
        static NEXT_MODULE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = NEXT_MODULE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "(function() {{
                        const __spStore = {{}};
                        const __sp = {script_props};
                        const __pick = function(o, fallback) {{
                            return Object.prototype.hasOwnProperty.call(__sp, o.name) ? __sp[o.name] : fallback;
                        }};
                        const builder = {{
                            addSlider(o) {{ __spStore[o.name] = __pick(o, o.value); return builder; }},
                            addCheckbox(o) {{ __spStore[o.name] = __pick(o, o.value); return builder; }},
                            addColor(o) {{ __spStore[o.name] = new Vec3(__pick(o, o.value)); return builder; }},
                            addText(o) {{ __spStore[o.name] = __pick(o, o.value); return builder; }},
                            addCombo(o) {{ __spStore[o.name] = __pick(o, o.value ?? o.options?.[0]?.value ?? 0); return builder; }},
                            addFile(o) {{ __spStore[o.name] = __pick(o, o.value); return builder; }},
                            finish() {{ return __spStore; }}
                        }};
                        globalThis.__cur = {{
                            thisLayer: globalThis.__layers[{layer_name}] || {{}},
                            createScriptProperties() {{ return builder; }},
                            store: __spStore,
                        }};
                    }})()",
                    layer_name = js_str(layer_name),
                ),
            )?;
            let ns = load_script_module(ctx, unique, raw_es_script)?;
            ctx.globals()
                .set("__ns", ns)
                .map_err(|e| js_err(ctx, e))?;
            eval_js::<String>(
                ctx,
                format!(
                    "(function() {{
                        const exports = globalThis.__ns, cur = globalThis.__cur;
                        globalThis.__ns = undefined; globalThis.__cur = undefined;
                        let val = {initial_value_expr};
                        if (typeof exports.init === 'function') {{
                            const r = exports.init(val);
                            if (r !== undefined) val = r;
                        }}
                        globalThis.__scripts[{slot_key}] = {{ exports, thisLayer: cur.thisLayer, currentValue: val, scriptProperties: cur.store }};
                        return String(val);
                    }})()",
                    slot_key = js_str(slot_key),
                ),
            )
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

    /// Tells every script that no media is playing, as Wallpaper Engine does at load when
    /// nothing is playing: `mediaStatusChanged`, `mediaPlaybackChanged`,
    /// `mediaPropertiesChanged`, `mediaThumbnailChanged` (`hasThumbnail: false`, black colors)
    /// and `mediaTimelineChanged`. Scripts use these to hide album-art layers.
    pub fn dispatch_media_idle(&self) -> anyhow::Result<()> {
        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                "(function() {
                    const black = () => new Vec3(0, 0, 0);
                    __callAll('mediaStatusChanged', { enabled: false });
                    __callAll('mediaPlaybackChanged', { state: 0 });
                    __callAll('mediaPropertiesChanged', {
                        title: '', artist: '', subTitle: '', albumTitle: '',
                        albumArtist: '', genres: '', contentType: 0,
                    });
                    __callAll('mediaThumbnailChanged', {
                        hasThumbnail: false, primaryColor: black(), secondaryColor: black(),
                        tertiaryColor: black(), textColor: black(), highContrastColor: black(),
                    });
                    __callAll('mediaTimelineChanged', { position: 0, duration: 0 });
                })()"
                    .to_string(),
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
                    globalThis.__fireTimers();
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
    fn this_layer_can_be_reassigned_like_a_sloppy_global() {
        let e = engine();
        e.register_layer("hero", [0.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_layer("bg", [0.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_property_script(
            "s",
            "hero",
            "thisLayer = thisScene.getLayer('bg');\nexport function update(v) { thisLayer.visible = false; return thisLayer.name; }",
            "0",
        )
        .unwrap();
        assert_eq!(step(&e)["s"], "bg");
        assert!(!e.layer_state("bg").unwrap().unwrap().visible);
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
        assert!(err.to_string().contains("__slot"), "{err}");
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
    fn es_module_syntax_is_supported() {
        let e = engine();
        // Minified, mid-line export, import of WE modules, arrow functions, classes, template
        // literals, `export var scriptProperties` and a localStorage stub.
        let out = e
            .register_property_script(
                "m",
                "L",
                "import * as WEMath from 'WEMath'; import { Vec3 as V3 } from 'WEVector';\n\
                 import WEColor from 'WEColor';\n\
                 export var scriptProperties = createScriptProperties().addSlider({name:'k',value:2}).finish();\n\
                 class Acc { constructor(){ this.n = 0; } bump(){ return ++this.n; } }\n\
                 const acc = new Acc(); var n=0;export function update(v){ try { return up(v);} catch(e){ return 'ERR '+e; } } function up(v){ localStorage.setItem('a','1'); \
                 const f = (a) => WEMath.mix(0, 10, a) * scriptProperties.k; \
                 return `${f(0.5)}|${acc.bump()}|${new V3(1,2,3).length() > 3}|${localStorage.getItem('a')}|${WEColor.rgb2hex(new Vec3(1,0,0))}`; }",
                "0",
            )
            .unwrap();
        assert_eq!(out, "0");
        let r = step(&e);
        assert_eq!(r["m"], "10|1|true|1|#ff0000");
    }

    #[test]
    fn unknown_import_is_an_error() {
        let e = engine();
        assert!(e
            .register_property_script(
                "z",
                "L",
                "import x from 'nope'; export function update(v){return v;}",
                "0"
            )
            .is_err());
    }

    fn run(e: &SceneScriptEngine, slot: &str, layer: &str, body: &str) -> String {
        e.register_property_script(slot, layer, body, "0").unwrap();
        step(e)[slot].clone()
    }

    #[test]
    fn get_layer_unknown_is_null_and_hierarchy_works() {
        let e = engine();
        e.register_layer("child", [1.0, 2.0, 0.0], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_layer("grp", [10.0, 0.0, 0.0], [2.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.set_layer_hierarchy(
            &[("child".into(), "grp".into())],
            &["grp".into()],
            &[("child".into(), 7)],
        )
        .unwrap();
        let out = run(
            &e,
            "h",
            "child",
            "export function update() { return [thisScene.getLayer('nope') === null,\
             thisScene.getLayer('grp').name, thisLayer.getParent().name,\
             thisLayer.getParent().getChildren().length, thisLayer.getParent().getParent() === null,\
             thisScene.getLayerByID(7).name, thisScene.enumerateLayers().length,\
             thisLayer.getTransformMatrix().m[12]].join(','); }",
        );
        assert_eq!(out, "true,grp,grp,1,true,child,1,12");
    }

    #[test]
    fn engine_screensaver_and_animation_frame_count_exist() {
        let e = engine();
        e.register_layer("L", [0.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_property_script(
            "a",
            "L",
            "export function update() { const a = thisLayer.getTextureAnimation();\
             return [engine.isScreensaver(), a.frameCount, a.getFrame() == a.frameCount - 1].join(','); }",
            "0",
        )
        .unwrap();
        let out = e.step_frame(0.016, 0.1, [0.0; 3], &Z64, &Z64).unwrap()["a"].clone();
        assert_eq!(out, "false,1,true");
    }

    #[test]
    fn timers_fire_and_cancel_with_function_handle() {
        let e = engine();
        e.register_property_script(
            "t",
            "L",
            "let n = 0; let stop;\
             engine.setTimeout(() => { n += 10; }, 500);\
             stop = engine.setTimeout(() => { n += 1000; }, 500); stop();\
             engine.setInterval(() => { n += 1; }, 1000);\
             export function update() { return n; }",
            "0",
        )
        .unwrap();
        let at = |t: f32| e.step_frame(0.016, t, [0.0; 3], &Z64, &Z64).unwrap()["t"].clone();
        assert_eq!(at(0.1), "0");
        assert_eq!(at(0.6), "10");
        assert_eq!(at(1.2), "11");
    }

    #[test]
    fn media_classes_and_video_texture_exist() {
        let e = engine();
        e.register_layer("v", [0.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        let out = run(
            &e,
            "m",
            "v",
            "export function update() { const ev = new MediaPlaybackEvent(MediaPlaybackEvent.PLAYBACK_STOPPED);\
             new MediaPropertiesEvent(); new MediaThumbnailEvent(); new MediaTimelineEvent();\
             const vt = thisLayer.getVideoTexture(); vt.play(); const p = vt.isPlaying(); vt.pause();\
             return ev.state + ':' + p + ':' + vt.isPlaying(); }",
        );
        assert_eq!(out, "0:true:false");
    }

    #[test]
    fn mat4_multiply_and_translation() {
        let e = engine();
        let out = run(
            &e,
            "x",
            "L",
            "export function update() { const a = new Mat4().translation(new Vec3(1, 2, 3));\
             const b = a.multiply(new Mat4().scale(new Vec3(2, 2, 2)));\
             return b.transformPoint(new Vec3(1, 1, 1)).toString() + '|' + b.scale().x; }",
        );
        assert_eq!(out, "3 4 5|2");
    }
}
