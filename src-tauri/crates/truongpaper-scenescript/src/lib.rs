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
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Memory ceiling for one script runtime.
pub const MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;
/// Wall-clock budget for one `init`/event/`applyUserProperties` host call.
pub const INIT_BUDGET: Duration = Duration::from_secs(2);
/// Hard wall-clock limit for one per-frame step (all slots together). Only a runaway script
/// (infinite loop) should reach it: it is interrupted so it cannot stall the render thread for
/// good. Ordinary slowness is judged per slot, see [`SLOW_SLOT_MS`].
pub const FRAME_BUDGET: Duration = Duration::from_millis(250);
/// A slot whose own `update` takes longer than this (milliseconds) in a frame counts a slow
/// strike; [`SLOW_STRIKES`] frames in a row disable it.
pub const SLOW_SLOT_MS: f64 = 100.0;
/// Consecutive slow frames of one slot before it is disabled.
pub const SLOW_STRIKES: u32 = 30;
/// A slot whose `update` throws on every frame for this much stepped (frame delta) time is
/// disabled, provided it also threw on at least [`ERROR_DISABLE_MIN_FRAMES`] frames in a row.
pub const ERROR_DISABLE_AFTER: Duration = Duration::from_secs(3);
/// Minimum number of consecutive throwing frames before a slot is disabled for errors.
pub const ERROR_DISABLE_MIN_FRAMES: u32 = 60;
/// Minimum gap between repeated log lines about the same failing slot.
pub const ERROR_LOG_INTERVAL: Duration = Duration::from_secs(60);
/// Total wall-clock budget for compiling and initializing every property script at load (see
/// [`SceneScriptEngine::begin_load`]). Scripts that would start after it ran out are skipped.
pub const LOAD_BUDGET: Duration = Duration::from_secs(3);
/// Consecutive frames in which one slot may hit the hard [`FRAME_BUDGET`] before it is disabled.
pub const OVERRUN_STRIKES: u32 = 3;
/// QuickJS native stack ceiling. Well below the render thread's own stack.
pub const MAX_STACK_BYTES: usize = 512 * 1024;
/// Allocation volume between QuickJS garbage collections (the library default is 256 KiB).
pub const GC_THRESHOLD_BYTES: usize = 4 * 1024 * 1024;
/// Longest script log message kept, in characters.
pub const LOG_MAX_CHARS: usize = 512;
/// Script log lines let through per [`LOG_WINDOW`].
pub const LOG_MAX_PER_WINDOW: u32 = 20;
/// Rate-limit window for script logging.
pub const LOG_WINDOW: Duration = Duration::from_secs(1);
/// Numbers per layer in the packed layer arrays shared with the script runtime
/// (origin, scale, angles, size, color, alpha, visible).
const LAYER_STRIDE: usize = 16;

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

/// Host entry points compiled once after [`SCENESCRIPT_BOOTSTRAP_JS`]: the per-frame step, the
/// packed layer sync/diff helpers and the cursor dispatcher. They take typed arguments (packed
/// number and string arrays), so no source text is built or parsed per frame. Host internals are
/// then made non-writable so one script cannot spoof state for the others.
pub const SCENESCRIPT_RUNTIME_JS: &str = r#"
(function () {
    const g = globalThis;
    const f = (n) => { n = Number(n); return Number.isFinite(n) ? n : 0; };
    // Layer state last exchanged with the host (sent or received), keyed by layer name.
    const snaps = new Map();
    const tmp = new Array(16).fill(0);
    const readLayer = (l, a) => {
        const o = l.origin, s = l.scale, r = l.angles, z = l.size, c = l.color;
        a[0] = f(o?.x); a[1] = f(o?.y); a[2] = f(o?.z);
        a[3] = f(s?.x); a[4] = f(s?.y); a[5] = f(s?.z);
        a[6] = f(r?.x); a[7] = f(r?.y); a[8] = f(r?.z);
        a[9] = f(z?.x); a[10] = f(z?.y);
        a[11] = f(c?.x); a[12] = f(c?.y); a[13] = f(c?.z);
        a[14] = f(l.alpha); a[15] = l.visible ? 1 : 0;
    };
    const textOf = (l) => (l.text == null ? '' : String(l.text));
    const remember = (name, l) => {
        readLayer(l, tmp);
        let sn = snaps.get(name);
        if (sn === undefined) { sn = { a: new Array(16).fill(0), text: '' }; snaps.set(name, sn); }
        for (let i = 0; i < 16; i++) sn.a[i] = tmp[i];
        sn.text = textOf(l);
    };
    // Appends every layer whose state differs from the last exchanged one.
    const collect = (nums, strs) => {
        for (const k in g.__layers) {
            const l = g.__layers[k];
            const name = String(l.name);
            readLayer(l, tmp);
            const text = textOf(l);
            let sn = snaps.get(name);
            if (sn !== undefined && sn.text === text) {
                let same = true;
                for (let i = 0; i < 16; i++) if (sn.a[i] !== tmp[i]) { same = false; break; }
                if (same) continue;
            }
            if (sn === undefined) { sn = { a: new Array(16).fill(0), text: '' }; snaps.set(name, sn); }
            for (let i = 0; i < 16; i++) { sn.a[i] = tmp[i]; nums.push(tmp[i]); }
            sn.text = text;
            strs.push(name, text);
        }
    };
    g.__collectLayers = function () { const nums = [], strs = []; collect(nums, strs); return [nums, strs]; };
    // Host -> script: writes packed layer states into the existing layer objects.
    g.__sync = function (nums, strs) {
        for (let i = 0, n = strs.length / 2; i < n; i++) {
            const l = g.__layers[strs[2 * i]];
            if (!l) continue;
            const b = i * 16;
            l.origin = new Vec3(nums[b], nums[b + 1], nums[b + 2]);
            l.scale = new Vec3(nums[b + 3], nums[b + 4], nums[b + 5]);
            l.angles = new Vec3(nums[b + 6], nums[b + 7], nums[b + 8]);
            l.color = new Vec3(nums[b + 11], nums[b + 12], nums[b + 13]);
            l.size = new Vec2(nums[b + 9], nums[b + 10]);
            l.alpha = nums[b + 14];
            l.visible = nums[b + 15] !== 0;
            l.text = strs[2 * i + 1];
            remember(strs[2 * i], l);
        }
    };
    // Creates layer objects from packed states.
    g.__register = function (nums, strs) {
        for (let i = 0, n = strs.length / 2; i < n; i++) {
            const b = i * 16, name = strs[2 * i];
            g.__layers[name] = {
                name,
                origin: new Vec3(nums[b], nums[b + 1], nums[b + 2]),
                scale: new Vec3(nums[b + 3], nums[b + 4], nums[b + 5]),
                angles: new Vec3(nums[b + 6], nums[b + 7], nums[b + 8]),
                size: new Vec2(nums[b + 9], nums[b + 10]),
                color: new Vec3(nums[b + 11], nums[b + 12], nums[b + 13]),
                alpha: nums[b + 14],
                visible: nums[b + 15] !== 0,
                text: strs[2 * i + 1],
                getEffect() { return { visible: true }; },
                ...g.__layerStubs(),
            };
            remember(name, g.__layers[name]);
        }
    };
    const LAST = new Map(), PENDING = [], ERR = new Map();
    const RES = [16, 32, 64];
    // p = [dt, runtime, cursorWorldX, cursorWorldY, hasInput, screenX, screenY, leftDown,
    //      timeOfDay (<0: local clock), ...left64, ...right64]
    g.__step = function (p) {
        PENDING.length = 0;
        const e = g.engine, inp = g.input;
        const dt = Math.min(Math.max(p[0] || 0, 0), 0.1);
        e.frametime = p[0]; e.runtime = p[1];
        if (p[4] !== 0) {
            const sp = inp.cursorScreenPosition; sp.x = p[5]; sp.y = p[6];
            inp.cursorLeftDown = p[7] !== 0;
            let t = p[8];
            if (t < 0) {
                const d = new Date();
                t = (d.getHours() * 3600 + d.getMinutes() * 60 + d.getSeconds()) / 86400;
            }
            e.timeOfDay = t;
        }
        g.__fireTimers();
        const w = inp.cursorWorldPosition; w.x = p[2]; w.y = p[3];
        for (const res of RES) {
            const stride = 64 / res, buf = g.__audioBuffers[res];
            for (let i = 0; i < res; i++) {
                const l = p[9 + i * stride] || 0, r = p[73 + i * stride] || 0;
                buf.left[i] = l; buf.right[i] = r; buf.average[i] = (l + r) * 0.5;
            }
        }
        const out = [], slow = [], failed = [];
        let t = __now();
        for (const k in g.__scripts) {
            const slot = g.__scripts[k];
            g.__curSlot = k;
            if (typeof slot.exports.update === 'function') {
                try {
                    const next = slot.exports.update(slot.currentValue);
                    if (next !== undefined) slot.currentValue = next;
                    ERR.delete(k);
                } catch (err) {
                    const now = __now();
                    let s = ERR.get(k);
                    if (!s) { s = { frames: 0, stepped: 0, logAt: now, n: 0 }; ERR.set(k, s); __log('error', `[${k}] update: ${err}`); }
                    else if (now - s.logAt >= g.__errLogMs) {
                        s.logAt = now;
                        __log('error', `[${k}] update: ${err} (${s.n} more failures since the last report)`);
                        s.n = 0;
                    }
                    s.n++;
                    // Consecutive throwing frames and stepped time, not wall time: a pause, a
                    // sleep or a render-thread stall must not count towards the limit.
                    s.frames++;
                    s.stepped += dt;
                    if (s.frames >= g.__errFrames && s.stepped * 1000 >= g.__errDisableMs) failed.push(k);
                }
            }
            const str = String(slot.currentValue);
            // Committed at the end of the frame: see __resetDiff.
            if (LAST.get(k) !== str) { PENDING.push(k, str); out.push(k, str); }
            const n = __now();
            if (n - t > g.__slowMs) slow.push(k);
            t = n;
        }
        g.__curSlot = undefined;
        const nums = [], strs = [];
        collect(nums, strs);
        for (let i = 0; i < PENDING.length; i += 2) LAST.set(PENDING[i], PENDING[i + 1]);
        PENDING.length = 0;
        return [out, nums, strs, slow, failed];
    };
    // A frame that failed may have handed out values the host never received: forget what was
    // reported so the next frame sends every slot again.
    g.__resetDiff = function () { LAST.clear(); PENDING.length = 0; };
    g.__errLogMs = 60000; g.__errDisableMs = 3000; g.__errFrames = 60; g.__slowMs = 100;
    g.__disable = function (k) { delete g.__scripts[k]; LAST.delete(k); ERR.delete(k); };
    g.__hasHandler = function (name) {
        for (const k in g.__scripts) if (typeof g.__scripts[k].exports[name] === 'function') return true;
        return false;
    };
    // mode: 0 keep the button state, 1 pressed, 2 released.
    g.__cursor = function (cb, x, y, mode, call) {
        const w = g.input.cursorWorldPosition; w.x = x; w.y = y;
        if (mode === 1) g.input.cursorLeftDown = true; else if (mode === 2) g.input.cursorLeftDown = false;
        if (call) g.__callAll(cb, { x, y, worldPosition: new Vec3(x, y, 0) });
    };
    // Lock the host internals (`__*`) only. Public globals (`shared`, `console`, `engine`,
    // `Vec3`, ...) stay writable: real Workshop scripts reassign them at module top level
    // (`shared = shared || {}`, `console = {...}`), which Wallpaper Engine allows.
    for (const n of [
        '__log', '__now', '__resetDiff', '__callAll', '__fireTimers', '__step', '__sync', '__register', '__collectLayers',
        '__disable', '__hasHandler', '__cursor', '__layerStubs', '__animStub', '__videoStub',
        '__localMatrix', '__WEMath', '__WEColor', '__layers', '__scripts', '__parents', '__audioBuffers']) {
        const d = Object.getOwnPropertyDescriptor(g, n);
        if (d) Object.defineProperty(g, n, { value: d.value, writable: false, configurable: false, enumerable: d.enumerable });
    }
    Object.freeze(g.__WEMath);
    Object.freeze(g.__WEColor);
})();
"#;

/// Rate limiter for script log output: at most [`LOG_MAX_PER_WINDOW`] lines per [`LOG_WINDOW`].
#[derive(Debug, Default)]
pub struct LogLimiter {
    window_start: Option<Instant>,
    emitted: u32,
    dropped: u32,
}

impl LogLimiter {
    /// `Some(n)` when a line at `now` may be written (`n` lines were dropped in the previous
    /// window, `0` normally); `None` when it must be dropped.
    pub fn admit(&mut self, now: Instant) -> Option<u32> {
        match self.window_start {
            Some(start) if now.saturating_duration_since(start) < LOG_WINDOW => {
                if self.emitted < LOG_MAX_PER_WINDOW {
                    self.emitted += 1;
                    Some(0)
                } else {
                    self.dropped = self.dropped.saturating_add(1);
                    None
                }
            }
            _ => {
                let previously_dropped = std::mem::take(&mut self.dropped);
                self.window_start = Some(now);
                self.emitted = 1;
                Some(previously_dropped)
            }
        }
    }
}

/// Makes an untrusted script log message safe to write: control characters (newlines
/// especially) are escaped and the text is cut to [`LOG_MAX_CHARS`] characters.
pub fn sanitize_log_message(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len().min(LOG_MAX_CHARS + 32));
    for (chars, c) in msg.chars().enumerate() {
        if chars >= LOG_MAX_CHARS {
            let rest = msg.chars().count() - LOG_MAX_CHARS;
            out.push_str(&format!("...[{rest} more chars]"));
            break;
        }
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push(' '),
            c if c.is_control() => out.push('?'),
            c => out.push(c),
        }
    }
    out
}

/// Counts consecutive frame-budget overruns per slot and decides when to disable one.
#[derive(Debug, Default)]
struct Breaker {
    strikes: HashMap<String, u32>,
    slow_strikes: HashMap<String, u32>,
    disabled: Vec<String>,
    /// Slots disabled for throwing on every frame.
    failed: Vec<String>,
}

impl Breaker {
    /// Records an overrun by `slot`; returns `true` when it just reached [`OVERRUN_STRIKES`].
    fn overrun(&mut self, slot: &str) -> bool {
        let n = self.strikes.entry(slot.to_string()).or_insert(0);
        *n += 1;
        if *n >= OVERRUN_STRIKES {
            self.strikes.remove(slot);
            self.disabled.push(slot.to_string());
            true
        } else {
            false
        }
    }

    /// Records the slots that were slow (own time over [`SLOW_SLOT_MS`]) in one finished frame;
    /// returns those that just reached [`SLOW_STRIKES`] consecutive slow frames.
    fn slow_frame(&mut self, slow: &[String]) -> Vec<String> {
        self.slow_strikes.retain(|k, _| slow.contains(k));
        let mut tripped = Vec::new();
        for k in slow {
            let n = self.slow_strikes.entry(k.clone()).or_insert(0);
            *n += 1;
            if *n >= SLOW_STRIKES {
                self.slow_strikes.remove(k);
                self.disabled.push(k.clone());
                tripped.push(k.clone());
            }
        }
        tripped
    }

    /// A frame finished within budget: the strikes were not consecutive.
    fn frame_ok(&mut self) {
        if !self.strikes.is_empty() {
            self.strikes.clear();
        }
    }
}

/// Per-frame inputs of [`SceneScriptEngine::step_frame_diff`].
#[derive(Debug, Clone, Copy)]
pub struct FrameInput<'a> {
    pub dt: f32,
    pub runtime: f32,
    /// `input.cursorWorldPosition` (x, y used).
    pub cursor_world: [f32; 3],
    /// Cursor screen position, button state and `engine.timeOfDay` (`time_of_day: None` uses
    /// the script runtime's local clock). `None` here leaves those inputs untouched.
    pub input: Option<InputState>,
    pub left_64: &'a [f32; 64],
    pub right_64: &'a [f32; 64],
}

/// Cursor/clock inputs set at the start of a frame.
#[derive(Debug, Clone, Copy)]
pub struct InputState {
    pub cursor_screen: [f32; 2],
    pub left_down: bool,
    pub time_of_day: Option<f32>,
}

/// What one script frame changed.
#[derive(Debug, Default)]
pub struct FrameOutput {
    /// `(slot key, new stringified value)` for slots whose value differs from the last frame
    /// (every slot on its first frame).
    pub slots: Vec<(String, String)>,
    /// Layers whose script-side state differs from the last state exchanged with the host.
    pub layers: Vec<LayerState>,
}

/// Layers, groups and hierarchy registered in one script call at load.
#[derive(Debug, Default)]
pub struct SceneLayers<'a> {
    /// Real layers (full state), then transform-only groups (`group_keys` lists the latter).
    pub layers: &'a [LayerState],
    pub parents: &'a [(String, String)],
    pub group_keys: &'a [String],
    pub ids: &'a [(String, i32)],
}

/// An `f32` widened to `f64`; non-finite values become `0`.
fn num(v: f32) -> f64 {
    if v.is_finite() {
        f64::from(v)
    } else {
        0.0
    }
}

fn pack_layers(states: &[LayerState]) -> (Vec<f64>, Vec<String>) {
    let mut nums = Vec::with_capacity(states.len() * LAYER_STRIDE);
    let mut strs = Vec::with_capacity(states.len() * 2);
    for s in states {
        for v in s
            .origin
            .iter()
            .chain(&s.scale)
            .chain(&s.angles)
            .chain(&s.size)
            .chain(&s.color)
            .chain(std::iter::once(&s.alpha))
        {
            nums.push(num(*v));
        }
        nums.push(if s.visible { 1.0 } else { 0.0 });
        strs.push(s.name.clone());
        strs.push(s.text.clone());
    }
    (nums, strs)
}

fn unpack_layers(nums: &[f64], strs: Vec<String>) -> Vec<LayerState> {
    let mut names = strs.into_iter();
    nums.chunks_exact(LAYER_STRIDE)
        .map(|c| {
            let n = |i: usize| c[i] as f32;
            LayerState {
                name: names.next().unwrap_or_default(),
                origin: [n(0), n(1), n(2)],
                scale: [n(3), n(4), n(5)],
                angles: [n(6), n(7), n(8)],
                size: [n(9), n(10)],
                color: [n(11), n(12), n(13)],
                alpha: n(14),
                visible: c[15] != 0.0,
                text: names.next().unwrap_or_default(),
            }
        })
        .collect()
}

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
    rt.set_max_stack_size(MAX_STACK_BYTES);
    rt.set_gc_threshold(GC_THRESHOLD_BYTES);
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

/// What is needed to register a property script again.
#[derive(Debug, Clone)]
struct Registration {
    layer: String,
    source: String,
    initial: String,
    props: String,
}

pub struct SceneScriptEngine {
    _runtime: Runtime,
    context: Context,
    /// Deadline read by the QuickJS interrupt handler; `None` while no host call is running.
    deadline: Arc<Mutex<Option<Instant>>>,
    frame_budget: Cell<Duration>,
    error_disable_after: Cell<Duration>,
    /// Every successful property-script registration, so slots the breaker disabled can be
    /// brought back ([`Self::reenable_disabled_slots`]).
    registrations: RefCell<HashMap<String, Registration>>,
    /// Slots disabled by the breaker since the last re-enable.
    off_slots: RefCell<Vec<String>>,
    /// End of the total script-load budget while one is running ([`Self::begin_load`]).
    load_deadline: Cell<Option<Instant>>,
    breaker: RefCell<Breaker>,
    /// Per cursor event kind: does any script define the callback? (`None`: not computed.)
    handlers: Cell<[Option<bool>; 4]>,
    /// Last value of every slot, for the all-slots view of [`Self::step_frame`].
    slot_values: RefCell<HashMap<String, String>>,
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
            let limiter = Mutex::new(LogLimiter::default());
            let log = Function::new(ctx.clone(), move |level: String, msg: String| {
                let Some(dropped) = limiter.lock().admit(Instant::now()) else {
                    return;
                };
                if dropped > 0 {
                    tracing::warn!(target: "scenescript", "{dropped} script log lines dropped (rate limit)");
                }
                let msg = sanitize_log_message(&msg);
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
            let epoch = Instant::now();
            let now = Function::new(ctx.clone(), move || {
                epoch.elapsed().as_secs_f64() * 1000.0
            })
            .map_err(|e| js_err(&ctx, e))?;
            ctx.globals()
                .set("__now", now)
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
            eval_js::<()>(&ctx, SCENESCRIPT_RUNTIME_JS)?;
            Ok(())
        })?;

        Ok(Self {
            _runtime: runtime,
            context,
            deadline,
            frame_budget: Cell::new(FRAME_BUDGET),
            error_disable_after: Cell::new(ERROR_DISABLE_AFTER),
            registrations: RefCell::new(HashMap::new()),
            off_slots: RefCell::new(Vec::new()),
            load_deadline: Cell::new(None),
            breaker: RefCell::new(Breaker::default()),
            handlers: Cell::new([None; 4]),
            slot_values: RefCell::new(HashMap::new()),
        })
    }

    /// Overrides the per-frame budget (tests; the default is [`FRAME_BUDGET`]).
    pub fn set_frame_budget(&self, budget: Duration) {
        self.frame_budget.set(budget);
    }

    /// Starts a total budget for [`Self::register_property_script_with_props`] calls: once it
    /// is spent, further scripts fail fast instead of each running up to [`INIT_BUDGET`].
    pub fn begin_load(&self, budget: Duration) {
        self.load_deadline.set(Some(Instant::now() + budget));
    }

    /// Ends the budget started by [`Self::begin_load`].
    pub fn end_load(&self) {
        self.load_deadline.set(None);
    }

    /// Slots disabled since the last call because they overran the frame budget
    /// [`OVERRUN_STRIKES`] frames in a row.
    pub fn take_disabled_slots(&self) -> Vec<String> {
        std::mem::take(&mut self.breaker.borrow_mut().disabled)
    }

    /// Slots disabled since the last call because their `update` threw on every frame for
    /// [`ERROR_DISABLE_AFTER`]. (These are also part of [`Self::take_disabled_slots`].)
    pub fn take_failed_slots(&self) -> Vec<String> {
        std::mem::take(&mut self.breaker.borrow_mut().failed)
    }

    /// Overrides how long a slot may throw on every frame (tests).
    pub fn set_error_disable_after(&self, after: Duration) {
        self.error_disable_after.set(after);
        let _ = self.guarded(INIT_BUDGET, |ctx| {
            ctx.globals()
                .set("__errDisableMs", after.as_secs_f64() * 1000.0)
                .map_err(|e| js_err(ctx, e))
        });
    }

    /// Overrides the minimum number of consecutive throwing frames (tests).
    pub fn set_error_disable_min_frames(&self, frames: u32) {
        let _ = self.guarded(INIT_BUDGET, |ctx| {
            ctx.globals()
                .set("__errFrames", f64::from(frames))
                .map_err(|e| js_err(ctx, e))
        });
    }

    /// Registers again every slot the breaker disabled (frame-budget overruns, slow frames or
    /// throwing updates), e.g. after `applyUserProperties`. Returns `(slot, initial value)` of
    /// the slots that came back; a slot whose script fails to start stays off.
    pub fn reenable_disabled_slots(&self) -> Vec<(String, String)> {
        let off = std::mem::take(&mut *self.off_slots.borrow_mut());
        let mut back = Vec::new();
        for slot in off {
            let Some(r) = self.registrations.borrow().get(&slot).cloned() else {
                continue;
            };
            match self.register_property_script_with_props(
                &slot, &r.layer, &r.source, &r.initial, &r.props,
            ) {
                Ok(v) => back.push((slot, v)),
                Err(err) => {
                    tracing::warn!(target: "scenescript", "script '{slot}' not re-enabled: {err}");
                    self.off_slots.borrow_mut().push(slot);
                }
            }
        }
        back
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
                    "(function() {{
                        const P = globalThis.__parents;
                        for (const k of Object.keys(P)) delete P[k];
                        Object.assign(P, {map_json});
                     }})();
                     for (const [k, id] of {ids_json}) if (globalThis.__layers[k]) globalThis.__layers[k].id = id;
                     for (const g of {groups_json}) if (globalThis.__layers[g]) globalThis.__layers[g].__group = true;"
                ),
            )
        })
    }

    /// Registers layers (full state: size, color and text included) in one script call, plus
    /// the transform-only groups and the parent/child hierarchy. Used at load instead of one
    /// call per layer.
    pub fn register_scene_layers(&self, scene: &SceneLayers<'_>) -> anyhow::Result<()> {
        let (nums, strs) = pack_layers(scene.layers);
        let ids_json = serde_json::to_string(scene.ids)?;
        let map: std::collections::BTreeMap<&str, &str> = scene
            .parents
            .iter()
            .map(|(c, p)| (c.as_str(), p.as_str()))
            .collect();
        let map_json = serde_json::to_string(&map)?;
        let groups_json = serde_json::to_string(scene.group_keys)?;
        self.guarded(INIT_BUDGET, |ctx| {
            let register: Function = ctx
                .globals()
                .get("__register")
                .map_err(|e| js_err(ctx, e))?;
            register
                .call::<_, ()>((nums, strs))
                .map_err(|e| js_err(ctx, e))?;
            eval_js::<()>(
                ctx,
                format!(
                    "(function() {{
                        const P = globalThis.__parents;
                        for (const k of Object.keys(P)) delete P[k];
                        Object.assign(P, {map_json});
                     }})();
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

    /// Layers whose script-side state differs from the last state exchanged with the host
    /// (sent by [`Self::sync_layer_states`] / registration, or returned by an earlier call or
    /// step). Only these cross the boundary.
    pub fn changed_layers(&self) -> anyhow::Result<Vec<LayerState>> {
        self.guarded(self.frame_budget.get(), |ctx| {
            let f: Function = ctx
                .globals()
                .get("__collectLayers")
                .map_err(|e| js_err(ctx, e))?;
            let arr: rquickjs::Array = f.call(()).map_err(|e| js_err(ctx, e))?;
            let nums: Vec<f64> = arr.get(0).map_err(|e| js_err(ctx, e))?;
            let strs: Vec<String> = arr.get(1).map_err(|e| js_err(ctx, e))?;
            Ok(unpack_layers(&nums, strs))
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
        let (nums, strs) = pack_layers(states);
        self.guarded(self.frame_budget.get(), |ctx| {
            let f: Function = ctx.globals().get("__sync").map_err(|e| js_err(ctx, e))?;
            f.call::<_, ()>((nums, strs)).map_err(|e| js_err(ctx, e))
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

        let budget = match self.load_deadline.get() {
            Some(end) => {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    anyhow::bail!("script load budget exhausted");
                }
                left.min(INIT_BUDGET)
            }
            None => INIT_BUDGET,
        };
        self.handlers.set([None; 4]);
        let res = self.guarded(budget, |ctx| {
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
        });
        if res.is_ok() {
            self.registrations.borrow_mut().insert(
                slot_key.to_string(),
                Registration {
                    layer: layer_name.to_string(),
                    source: raw_es_script.to_string(),
                    initial: initial_value_expr.to_string(),
                    props: script_props.clone(),
                },
            );
            self.off_slots.borrow_mut().retain(|s| s != slot_key);
        }
        res
    }

    /// Local wall-clock time of day in `[0, 1)` as the script runtime's `Date` sees it
    /// (`engine.timeOfDay` is local time in Wallpaper Engine, not UTC).
    pub fn local_time_of_day(&self) -> anyhow::Result<f32> {
        self.guarded(INIT_BUDGET, |ctx| {
            let t: f64 = eval_js(
                ctx,
                "(function() { const d = new Date(); return (d.getHours() * 3600 + d.getMinutes() * 60 + d.getSeconds()) / 86400; })()"
                    .to_string(),
            )?;
            Ok(t as f32)
        })
    }

    /// Overwrites a registered slot's current value with a boolean. Used when `init` wrote the
    /// bound property through `thisLayer` (Wallpaper Engine then hands that value to `update`).
    pub fn set_slot_bool(&self, slot_key: &str, value: bool) -> anyhow::Result<()> {
        self.guarded(INIT_BUDGET, |ctx| {
            eval_js::<()>(
                ctx,
                format!(
                    "(function() {{ const s = globalThis.__scripts[{slot}]; if (s) s.currentValue = {value}; }})()",
                    slot = js_str(slot_key),
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

    /// Whether any script defines the callback of this cursor event kind (cached).
    pub fn has_cursor_handler(&self, kind: CursorEventKind) -> bool {
        let idx = kind as usize;
        let mut cache = self.handlers.get();
        if let Some(v) = cache[idx] {
            return v;
        }
        let found = self
            .guarded(INIT_BUDGET, |ctx| {
                let f: Function = ctx
                    .globals()
                    .get("__hasHandler")
                    .map_err(|e| js_err(ctx, e))?;
                f.call::<_, bool>((kind.callback(),))
                    .map_err(|e| js_err(ctx, e))
            })
            .unwrap_or(false);
        cache[idx] = Some(found);
        self.handlers.set(cache);
        found
    }

    /// Forwards a desktop cursor event (`cursorMove/Down/Up/Click`) to all scripts.
    /// `world_x`/`world_y` are scene coordinates (see `input.cursorWorldPosition`).
    pub fn dispatch_cursor_event(
        &self,
        kind: CursorEventKind,
        world_x: f32,
        world_y: f32,
    ) -> anyhow::Result<()> {
        let mode = match kind {
            CursorEventKind::Down => 1,
            CursorEventKind::Up => 2,
            _ => 0,
        };
        let call = self.has_cursor_handler(kind);
        self.guarded(INIT_BUDGET, |ctx| {
            let f: Function = ctx.globals().get("__cursor").map_err(|e| js_err(ctx, e))?;
            f.call::<_, ()>((kind.callback(), num(world_x), num(world_y), mode, call))
                .map_err(|e| js_err(ctx, e))
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

    /// One script frame: sets the inputs, fires timers, runs every slot's `update` and returns
    /// only what changed (slot values that differ from last frame, layers whose state differs
    /// from the last exchanged one). A frame that overruns the budget counts a strike against
    /// the slot that was running; see [`Self::take_disabled_slots`].
    pub fn step_frame_diff(&self, input: &FrameInput<'_>) -> anyhow::Result<FrameOutput> {
        let audio = |v: f32| {
            if v.is_finite() {
                (f64::from(v) * 1e4).round() / 1e4
            } else {
                0.0
            }
        };
        let mut p: Vec<f64> = Vec::with_capacity(9 + 128);
        let (has_input, screen, down, tod) = match input.input {
            Some(i) => (
                1.0,
                i.cursor_screen,
                f64::from(i.left_down),
                i.time_of_day.map_or(-1.0, num),
            ),
            None => (0.0, [0.0; 2], 0.0, -1.0),
        };
        p.extend([
            num(input.dt),
            num(input.runtime),
            num(input.cursor_world[0]),
            num(input.cursor_world[1]),
            has_input,
            num(screen[0]),
            num(screen[1]),
            down,
            tod,
        ]);
        p.extend(input.left_64.iter().map(|v| audio(*v)));
        p.extend(input.right_64.iter().map(|v| audio(*v)));

        let budget = self.frame_budget.get();
        let started = Instant::now();
        let res = self.guarded(budget, |ctx| {
            let f: Function = ctx.globals().get("__step").map_err(|e| js_err(ctx, e))?;
            let arr: rquickjs::Array = f.call((p,)).map_err(|e| js_err(ctx, e))?;
            let slots: Vec<String> = arr.get(0).map_err(|e| js_err(ctx, e))?;
            let nums: Vec<f64> = arr.get(1).map_err(|e| js_err(ctx, e))?;
            let strs: Vec<String> = arr.get(2).map_err(|e| js_err(ctx, e))?;
            let slow: Vec<String> = arr.get(3).map_err(|e| js_err(ctx, e))?;
            let failed: Vec<String> = arr.get(4).map_err(|e| js_err(ctx, e))?;
            Ok((slots, nums, strs, slow, failed))
        });
        match res {
            Ok((slots, nums, strs, slow, failed)) => {
                self.breaker.borrow_mut().frame_ok();
                let tripped = self.breaker.borrow_mut().slow_frame(&slow);
                for slot in tripped {
                    tracing::warn!(
                        target: "scenescript",
                        "script '{slot}' took over {SLOW_SLOT_MS} ms for {SLOW_STRIKES} frames in a row; disabled"
                    );
                    self.disable_slot(&slot);
                }
                for slot in failed {
                    tracing::warn!(
                        target: "scenescript",
                        "script '{slot}' threw on every frame for {} s; disabled",
                        self.error_disable_after.get().as_secs_f32()
                    );
                    {
                        let mut b = self.breaker.borrow_mut();
                        b.failed.push(slot.clone());
                        b.disabled.push(slot.clone());
                    }
                    self.disable_slot(&slot);
                }
                let mut it = slots.into_iter();
                let mut changed = Vec::new();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    changed.push((k, v));
                }
                Ok(FrameOutput {
                    slots: changed,
                    layers: unpack_layers(&nums, strs),
                })
            }
            Err(err) => {
                // Values handed out by the failed frame never reached the host.
                let _ = self.guarded(INIT_BUDGET, |ctx| {
                    let f: Function = ctx
                        .globals()
                        .get("__resetDiff")
                        .map_err(|e| js_err(ctx, e))?;
                    f.call::<_, ()>(()).map_err(|e| js_err(ctx, e))
                });
                if started.elapsed() >= budget {
                    self.record_overrun();
                }
                Err(err)
            }
        }
    }

    /// Blames the slot that was running when the frame budget ran out.
    fn record_overrun(&self) {
        let slot: Option<String> = self
            .guarded(INIT_BUDGET, |ctx| {
                eval_js::<Option<String>>(ctx, "globalThis.__curSlot")
            })
            .ok()
            .flatten();
        let Some(slot) = slot else { return };
        if self.breaker.borrow_mut().overrun(&slot) {
            tracing::warn!(
                target: "scenescript",
                "script '{slot}' overran the frame budget {OVERRUN_STRIKES} frames in a row; disabled"
            );
            self.disable_slot(&slot);
        }
    }

    /// Removes a slot from the script runtime.
    fn disable_slot(&self, slot: &str) {
        let _ = self.guarded(INIT_BUDGET, |ctx| {
            let f: Function = ctx.globals().get("__disable").map_err(|e| js_err(ctx, e))?;
            f.call::<_, ()>((slot,)).map_err(|e| js_err(ctx, e))
        });
        self.slot_values.borrow_mut().remove(slot);
        self.handlers.set([None; 4]);
        let mut off = self.off_slots.borrow_mut();
        if !off.iter().any(|s| s == slot) {
            off.push(slot.to_string());
        }
    }

    /// Like [`Self::step_frame_diff`] but returns the current value of every slot and leaves
    /// the cursor screen position, button and time of day alone. Layer changes found by the
    /// step are not reported (use [`Self::layer_states`]).
    pub fn step_frame(
        &self,
        dt: f32,
        runtime: f32,
        cursor_world: [f32; 3],
        left_64: &[f32; 64],
        right_64: &[f32; 64],
    ) -> anyhow::Result<HashMap<String, String>> {
        let out = self.step_frame_diff(&FrameInput {
            dt,
            runtime,
            cursor_world,
            input: None,
            left_64,
            right_64,
        })?;
        let mut values = self.slot_values.borrow_mut();
        for (k, v) in out.slots {
            values.insert(k, v);
        }
        Ok(values.clone())
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
    fn local_time_of_day_is_in_range() {
        let t = engine().local_time_of_day().unwrap();
        assert!((0.0..1.0).contains(&t), "{t}");
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

    fn frame<'a>(left: &'a [f32; 64], right: &'a [f32; 64], t: f32) -> FrameInput<'a> {
        FrameInput {
            dt: 0.016,
            runtime: t,
            cursor_world: [0.0; 3],
            input: Some(InputState {
                cursor_screen: [1.0, 2.0],
                left_down: false,
                time_of_day: Some(0.25),
            }),
            left_64: left,
            right_64: right,
        }
    }

    #[test]
    fn step_returns_only_changed_slots() {
        let e = engine();
        e.register_property_script("c", "L", "export function update(v) { return 5; }", "0")
            .unwrap();
        e.register_property_script("n", "L", "export function update(v) { return v + 1; }", "0")
            .unwrap();
        let first = e.step_frame_diff(&frame(&Z64, &Z64, 0.0)).unwrap();
        assert_eq!(first.slots.len(), 2, "every slot on its first frame");
        let second = e.step_frame_diff(&frame(&Z64, &Z64, 0.1)).unwrap();
        assert_eq!(second.slots, [("n".to_string(), "2".to_string())]);
        // The all-slots view of `step_frame` reports every slot.
        let e2 = engine();
        e2.register_property_script("c", "L", "export function update(v) { return 5; }", "0")
            .unwrap();
        assert_eq!(step(&e2)["c"], "5");
        assert_eq!(step(&e2)["c"], "5");
    }

    #[test]
    fn step_sets_inputs_and_time_of_day() {
        let e = engine();
        e.register_property_script(
            "i",
            "L",
            "export function update(v) { return [input.cursorScreenPosition.x, input.cursorLeftDown, engine.timeOfDay].join(','); }",
            "0",
        )
        .unwrap();
        let out = e.step_frame_diff(&frame(&Z64, &Z64, 0.0)).unwrap();
        assert_eq!(out.slots[0].1, "1,false,0.25");
    }

    #[test]
    fn step_returns_only_changed_layers() {
        let e = engine();
        e.register_layer("a", [1.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_layer("b", [2.0; 3], [1.0; 3], [0.0; 3], 1.0, true)
            .unwrap();
        e.register_property_script(
            "s",
            "a",
            "let n = 0; export function update(v) { if (n++ === 1) thisLayer.alpha = 0.5; return v; }",
            "0",
        )
        .unwrap();
        // Registered layers are reported once, then quiet.
        let f0 = e.step_frame_diff(&frame(&Z64, &Z64, 0.0)).unwrap();
        assert_eq!(f0.layers.len(), 2);
        let f1 = e.step_frame_diff(&frame(&Z64, &Z64, 0.1)).unwrap();
        assert_eq!(f1.layers.len(), 1);
        assert_eq!((f1.layers[0].name.as_str(), f1.layers[0].alpha), ("a", 0.5));
        let f2 = e.step_frame_diff(&frame(&Z64, &Z64, 0.2)).unwrap();
        assert!(f2.layers.is_empty());
        assert!(e.changed_layers().unwrap().is_empty());
    }

    #[test]
    fn host_sync_does_not_echo_back() {
        let e = engine();
        let mut a = LayerState {
            name: "a".into(),
            scale: [1.0; 3],
            color: [1.0; 3],
            alpha: 1.0,
            visible: true,
            ..Default::default()
        };
        e.register_scene_layers(&SceneLayers {
            layers: std::slice::from_ref(&a),
            ..Default::default()
        })
        .unwrap();
        assert!(e.changed_layers().unwrap().is_empty());
        a.origin = [3.5, 4.0, 5.0];
        a.text = "hi".into();
        e.sync_layer_states(std::slice::from_ref(&a)).unwrap();
        assert!(e.changed_layers().unwrap().is_empty());
        assert_eq!(e.layer_state("a").unwrap().unwrap(), a);
    }

    #[test]
    fn register_scene_layers_sets_groups_and_parents() {
        let e = engine();
        let mk = |n: &str| LayerState {
            name: n.into(),
            scale: [1.0; 3],
            color: [1.0; 3],
            alpha: 1.0,
            visible: true,
            ..Default::default()
        };
        let layers = [mk("child"), mk("grp")];
        e.register_scene_layers(&SceneLayers {
            layers: &layers,
            parents: &[("child".into(), "grp".into())],
            group_keys: &["grp".into()],
            ids: &[("child".into(), 7)],
        })
        .unwrap();
        let out = run(
            &e,
            "h",
            "child",
            "export function update() { return [thisLayer.getParent().name, thisScene.getLayerByID(7).name, thisScene.enumerateLayers().length].join(','); }",
        );
        assert_eq!(out, "grp,child,1");
    }

    #[test]
    fn overrunning_slot_is_disabled_after_three_strikes() {
        let e = engine();
        e.set_frame_budget(Duration::from_millis(20));
        e.register_property_script(
            "hang",
            "L",
            "export function update(v) { while (true) {} }",
            "0",
        )
        .unwrap();
        e.register_property_script(
            "ok",
            "L",
            "export function update(v) { return v + 1; }",
            "0",
        )
        .unwrap();
        for _ in 0..2 {
            assert!(e.step_frame_diff(&frame(&Z64, &Z64, 0.0)).is_err());
            assert!(e.take_disabled_slots().is_empty());
        }
        assert!(e.step_frame_diff(&frame(&Z64, &Z64, 0.0)).is_err());
        assert_eq!(e.take_disabled_slots(), ["hang"]);
        // Healthy scripts run again and the hung one is gone.
        let out = e.step_frame_diff(&frame(&Z64, &Z64, 0.1)).unwrap();
        assert!(out.slots.iter().any(|(k, _)| k == "ok"));
        assert!(out.slots.iter().all(|(k, _)| k != "hang"));
    }

    fn busy_ms(ms: u32) -> String {
        format!(
            "export function update(v) {{ const t = Date.now(); while (Date.now() - t < {ms}) {{}} return v + 1; }}"
        )
    }

    #[test]
    fn slow_but_healthy_scripts_are_not_disabled() {
        // Three slots of ~30 ms each: 90 ms per frame, over the old 33 ms frame budget but
        // under the hard limit and under the per-slot limit.
        let e = engine();
        for k in ["a", "b", "c"] {
            e.register_property_script(k, "L", &busy_ms(30), "0")
                .unwrap();
        }
        for i in 0..14 {
            e.step_frame_diff(&frame(&Z64, &Z64, i as f32 * 0.03))
                .unwrap();
        }
        assert!(e.take_disabled_slots().is_empty());
    }

    #[test]
    fn slot_slow_for_many_frames_is_disabled_but_not_its_neighbour() {
        let mut b = Breaker::default();
        let slow = vec!["s".to_string()];
        for _ in 0..SLOW_STRIKES - 1 {
            assert!(b.slow_frame(&slow).is_empty());
        }
        // A fast frame in between resets the count.
        assert!(b.slow_frame(&[]).is_empty());
        for _ in 0..SLOW_STRIKES - 1 {
            assert!(b.slow_frame(&slow).is_empty());
        }
        assert_eq!(b.slow_frame(&slow), ["s"]);
        assert_eq!(b.disabled, ["s"]);
    }

    #[test]
    fn interrupted_frame_resends_slot_values() {
        let e = engine();
        e.register_property_script(
            "n",
            "L",
            "let c = 0; export function update(v) { return ++c; }",
            "0",
        )
        .unwrap();
        e.register_property_script(
            "hang",
            "L",
            "let c = 0; export function update(v) { if (++c === 2) { for (;;) {} } return v; }",
            "0",
        )
        .unwrap();
        e.set_frame_budget(Duration::from_millis(30));
        let f0 = e.step_frame_diff(&frame(&Z64, &Z64, 0.0)).unwrap();
        assert!(f0.slots.iter().any(|(k, v)| k == "n" && v == "1"));
        // "n" runs first and changes, then "hang" is interrupted: the frame is lost.
        assert!(e.step_frame_diff(&frame(&Z64, &Z64, 0.1)).is_err());
        e.set_frame_budget(Duration::from_millis(250));
        let f2 = e.step_frame_diff(&frame(&Z64, &Z64, 0.2)).unwrap();
        assert!(
            f2.slots.iter().any(|(k, v)| k == "n" && v == "3"),
            "{:?}",
            f2.slots
        );
        assert!(f2.slots.iter().any(|(k, _)| k == "hang"));
    }

    #[test]
    fn slot_throwing_every_frame_is_disabled_and_reported() {
        let e = engine();
        e.set_error_disable_after(Duration::from_millis(60));
        e.set_error_disable_min_frames(3);
        e.register_property_script(
            "bad",
            "L",
            "export function update(v) { throw new Error('x'); }",
            "0",
        )
        .unwrap();
        e.register_property_script(
            "ok",
            "L",
            "export function update(v) { return v + 1; }",
            "0",
        )
        .unwrap();
        // dt is 16 ms: 60 ms of stepped time and 3 frames need 4 frames.
        for i in 0..3 {
            e.step_frame_diff(&frame(&Z64, &Z64, i as f32 * 0.016))
                .unwrap();
        }
        assert!(e.take_failed_slots().is_empty());
        e.step_frame_diff(&frame(&Z64, &Z64, 0.1)).unwrap();
        assert_eq!(e.take_failed_slots(), ["bad"]);
        assert_eq!(e.take_disabled_slots(), ["bad"]);
        let out = e.step_frame_diff(&frame(&Z64, &Z64, 0.2)).unwrap();
        assert!(out.slots.iter().all(|(k, _)| k != "bad"));
    }

    #[test]
    fn error_disable_ignores_wall_time_and_needs_min_frames() {
        let e = engine();
        e.set_error_disable_after(Duration::from_millis(10));
        e.register_property_script(
            "bad",
            "L",
            "export function update(v) { throw new Error('x'); }",
            "0",
        )
        .unwrap();
        // Wall time passes between two throwing frames (sleep, stall): no disable.
        e.step_frame_diff(&frame(&Z64, &Z64, 0.0)).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        e.step_frame_diff(&frame(&Z64, &Z64, 0.1)).unwrap();
        assert!(e.take_failed_slots().is_empty());
        // Default minimum is 60 frames even once the stepped time is exceeded.
        for i in 0..56 {
            e.step_frame_diff(&frame(&Z64, &Z64, 0.2 + i as f32 * 0.016))
                .unwrap();
        }
        assert!(e.take_failed_slots().is_empty());
        e.step_frame_diff(&frame(&Z64, &Z64, 9.0)).unwrap();
        e.step_frame_diff(&frame(&Z64, &Z64, 9.1)).unwrap();
        assert_eq!(e.take_failed_slots(), ["bad"]);
    }

    #[test]
    fn disabled_slots_come_back_with_reenable() {
        let e = engine();
        e.set_error_disable_after(Duration::from_millis(30));
        e.set_error_disable_min_frames(2);
        e.register_property_script(
            "bad",
            "L",
            "export function update(v) { throw new Error('x'); }",
            "5",
        )
        .unwrap();
        for i in 0..4 {
            e.step_frame_diff(&frame(&Z64, &Z64, i as f32 * 0.016))
                .unwrap();
        }
        assert_eq!(e.take_disabled_slots(), ["bad"]);
        assert_eq!(
            e.reenable_disabled_slots(),
            [("bad".to_string(), "5".to_string())]
        );
        assert!(e.reenable_disabled_slots().is_empty());
        let out = e.step_frame_diff(&frame(&Z64, &Z64, 1.0)).unwrap();
        assert!(out.slots.iter().any(|(k, _)| k == "bad"));
    }

    #[test]
    fn slot_that_recovers_is_not_disabled() {
        let e = engine();
        e.set_error_disable_after(Duration::from_millis(60));
        e.set_error_disable_min_frames(2);
        e.register_property_script(
            "flaky",
            "L",
            "let c = 0; export function update(v) { if (++c === 1 || c === 3 || c === 4) throw new Error('x'); return v; }",
            "0",
        )
        .unwrap();
        for i in 0..6 {
            e.step_frame_diff(&frame(&Z64, &Z64, i as f32 * 0.016))
                .unwrap();
        }
        assert!(e.take_failed_slots().is_empty());
    }

    #[test]
    fn breaker_counts_consecutive_overruns_only() {
        let mut b = Breaker::default();
        assert!(!b.overrun("x"));
        b.frame_ok();
        assert!(!b.overrun("x"));
        assert!(!b.overrun("x"));
        assert!(b.overrun("x"));
        assert_eq!(b.disabled, ["x"]);
    }

    #[test]
    fn load_budget_skips_remaining_scripts() {
        let e = engine();
        e.begin_load(Duration::ZERO);
        let err = e
            .register_property_script("a", "L", "export function update(v) { return 1; }", "0")
            .unwrap_err();
        assert!(err.to_string().contains("budget"), "{err}");
        e.end_load();
        e.register_property_script("a", "L", "export function update(v) { return 1; }", "0")
            .unwrap();
    }

    #[test]
    fn cursor_handler_detection_is_per_kind() {
        let e = engine();
        assert!(!e.has_cursor_handler(CursorEventKind::Move));
        e.register_property_script(
            "c",
            "L",
            "export function cursorClick() {} export function update(v) { return v; }",
            "0",
        )
        .unwrap();
        assert!(e.has_cursor_handler(CursorEventKind::Click));
        assert!(!e.has_cursor_handler(CursorEventKind::Move));
        // Without a handler the event is still accepted.
        e.dispatch_cursor_event(CursorEventKind::Move, 9.0, 8.0)
            .unwrap();
    }

    #[test]
    fn host_internals_are_locked() {
        let e = engine();
        let out = run(
            &e,
            "lock",
            "L",
            "export function update() {
                let r = [];
                for (const n of ['__layers', '__scripts', '__log', '__step']) {
                    try { globalThis[n] = {}; r.push('w'); } catch (err) { r.push('x'); }
                }
                // Public globals stay writable: Workshop scripts reassign them.
                for (const n of ['shared', 'console']) {
                    try { globalThis[n] = globalThis[n]; r.push('w'); } catch (err) { r.push('x'); }
                }
                try { __WEMath.PI = 3; r.push('w'); } catch (err) { r.push('x'); }
                return r.join('');
            }",
        );
        assert_eq!(out, "xxxxwwx");
    }

    #[test]
    fn log_messages_are_sanitized_and_truncated() {
        assert_eq!(sanitize_log_message("a\nb\r\tc\u{7}"), "a\\nb\\r c?");
        let long = "x".repeat(LOG_MAX_CHARS + 40);
        let out = sanitize_log_message(&long);
        assert!(out.starts_with(&"x".repeat(LOG_MAX_CHARS)));
        assert!(out.ends_with("...[40 more chars]"));
        assert_eq!(sanitize_log_message("héllo"), "héllo");
    }

    #[test]
    fn log_limiter_caps_lines_per_window() {
        let mut l = LogLimiter::default();
        let t0 = Instant::now();
        let admitted = (0..100).filter(|_| l.admit(t0).is_some()).count();
        assert_eq!(admitted, LOG_MAX_PER_WINDOW as usize);
        // Next window reports how many were dropped.
        assert_eq!(l.admit(t0 + LOG_WINDOW), Some(80));
        assert_eq!(l.admit(t0 + LOG_WINDOW), Some(0));
    }

    #[test]
    fn log_flood_from_a_script_does_not_break_the_frame() {
        let e = engine();
        e.register_property_script(
            "spam",
            "L",
            "export function update(v) { for (let i = 0; i < 500; i++) console.log('line\\nx' + i); return v; }",
            "0",
        )
        .unwrap();
        step(&e);
    }
}
