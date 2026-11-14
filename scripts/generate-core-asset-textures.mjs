#!/usr/bin/env node
// Generates the utility textures of TruongPaper's bundled core assets
// (src-tauri/resources/core-assets/assets/materials/util/*.tex). They stand in for the
// "util/..." textures stock Wallpaper Engine effects reference by name (white, black, noflow,
// noise, clouds_256, perlin_256). Everything is procedurally generated here, deterministic,
// and written as uncompressed RGBA8888 TEXV0005 / TEXB0003 files.
//
// Usage: node scripts/generate-core-asset-textures.mjs
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const outDir = join(root, 'src-tauri/resources/core-assets/assets/materials/util');

const CLAMP_UVS = 2;

function cstr(s) {
  return Buffer.from(`${s}\0`, 'latin1');
}

function i32(v) {
  const b = Buffer.alloc(4);
  b.writeInt32LE(v);
  return b;
}

/** One-mip RGBA8888 `.tex` file. */
function texFile(width, height, rgba, flags = 0, trailer = null) {
  return Buffer.concat([
    cstr('TEXV0005'),
    cstr('TEXI0001'),
    i32(0), // format RGBA8888
    i32(flags),
    i32(width),
    i32(height),
    i32(width),
    i32(height),
    i32(0),
    cstr('TEXB0003'),
    i32(1), // image count
    i32(-1), // FreeImage format: none (raw pixels)
    i32(1), // mip count
    i32(width),
    i32(height),
    i32(0), // not LZ4
    i32(0),
    i32(rgba.length),
    Buffer.from(rgba),
    ...(trailer ? [trailer] : []),
  ]);
}

function solid(r, g, b, a) {
  const px = new Uint8Array(4 * 4 * 4);
  for (let i = 0; i < 16; i++) px.set([r, g, b, a], i * 4);
  return texFile(4, 4, px, CLAMP_UVS);
}

// Small deterministic PRNG (mulberry32).
function rng(seed) {
  let t = seed >>> 0;
  return () => {
    t = (t + 0x6d2b79f5) >>> 0;
    let x = t;
    x = Math.imul(x ^ (x >>> 15), x | 1);
    x ^= x + Math.imul(x ^ (x >>> 7), x | 61);
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
  };
}

/** Tileable value noise lattice of `cells` x `cells`, sampled smoothly. */
function lattice(cells, seed) {
  const rand = rng(seed);
  const v = Float32Array.from({ length: cells * cells }, rand);
  const at = (x, y) => v[((y % cells) + cells) % cells * cells + (((x % cells) + cells) % cells)];
  return (u, w) => {
    const x = u * cells;
    const y = w * cells;
    const x0 = Math.floor(x);
    const y0 = Math.floor(y);
    const fx = x - x0;
    const fy = y - y0;
    const sx = fx * fx * (3 - 2 * fx);
    const sy = fy * fy * (3 - 2 * fy);
    const top = at(x0, y0) * (1 - sx) + at(x0 + 1, y0) * sx;
    const bottom = at(x0, y0 + 1) * (1 - sx) + at(x0 + 1, y0 + 1) * sx;
    return top * (1 - sy) + bottom * sy;
  };
}

/** Tileable fractal noise in 0..1 from `octaves` lattices doubling in frequency. */
function fractal(size, baseCells, octaves, seed) {
  const layers = Array.from({ length: octaves }, (_, i) => lattice(baseCells << i, seed + i * 101));
  const out = new Float32Array(size * size);
  let min = Infinity;
  let max = -Infinity;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let sum = 0;
      let amp = 1;
      let norm = 0;
      for (const layer of layers) {
        sum += layer(x / size, y / size) * amp;
        norm += amp;
        amp *= 0.5;
      }
      const v = sum / norm;
      out[y * size + x] = v;
      min = Math.min(min, v);
      max = Math.max(max, v);
    }
  }
  return out.map((v) => (v - min) / (max - min || 1));
}

function toRgba(size, channels) {
  const px = new Uint8Array(size * size * 4);
  for (let i = 0; i < size * size; i++) {
    px[i * 4] = Math.round(channels[0][i] * 255);
    px[i * 4 + 1] = Math.round(channels[1][i] * 255);
    px[i * 4 + 2] = Math.round(channels[2][i] * 255);
    px[i * 4 + 3] = 255;
  }
  return px;
}

const files = {
  'white.tex': solid(255, 255, 255, 255),
  'black.tex': solid(0, 0, 0, 255),
  // Flow / direction maps encode "no motion" as 0.5 in R and G.
  'noflow.tex': solid(127, 127, 0, 255),
};

{
  const size = 256;
  const rand = rng(7);
  const white = () => Float32Array.from({ length: size * size }, rand);
  files['noise.tex'] = texFile(size, size, toRgba(size, [white(), white(), white()]));
}
{
  const size = 256;
  const c = fractal(size, 4, 5, 11);
  files['clouds_256.tex'] = texFile(size, size, toRgba(size, [c, c, c]));
}
{
  const size = 256;
  files['perlin_256.tex'] = texFile(
    size,
    size,
    toRgba(size, [fractal(size, 8, 3, 21), fractal(size, 8, 3, 31), fractal(size, 8, 3, 41)]),
  );
}

// Tangent-space "flat" normal (no bending) for refraction-style effects.
files['flatnormal.tex'] = solid(128, 128, 255, 255);

const effectFiles = {};
{
  // Tileable water normal map: slopes of a fractal height field, packed as 0.5 + n / 2.
  const size = 256;
  const h = fractal(size, 6, 4, 51);
  const at = (x, y) => h[((y + size) % size) * size + ((x + size) % size)];
  const nx = new Float32Array(size * size);
  const ny = new Float32Array(size * size);
  const nz = new Float32Array(size * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = (at(x + 1, y) - at(x - 1, y)) * 6;
      const dy = (at(x, y + 1) - at(x, y - 1)) * 6;
      const len = Math.hypot(dx, dy, 1);
      const i = y * size + x;
      nx[i] = 0.5 - dx / len / 2;
      ny[i] = 0.5 - dy / len / 2;
      nz[i] = 0.5 + 1 / len / 2;
    }
  }
  effectFiles['waterripplenormal.tex'] = texFile(size, size, toRgba(size, [nx, ny, nz]));
}
{
  // Smooth per-area time offsets for the two-phase flow map.
  const size = 128;
  const p = fractal(size, 4, 2, 61);
  effectFiles['waterflowphase.tex'] = texFile(size, size, toRgba(size, [p, p, p]));
}


// ---------------------------------------------------------------------------------------------
// Original procedural particle sprites (materials/particle/...). White colour, shaped alpha: the
// particle system tints them. Soft shapes only; nothing here is derived from stock artwork.
const particleFiles = {};
const smooth = (a, b, x) => {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a || 1)));
  return t * t * (3 - 2 * t);
};

/** Builds a w x h white sprite whose alpha is `alphaAt(u, v, x, y)` in 0..1 (u, v in -1..1). */
function sprite(w, h, alphaAt, flags = CLAMP_UVS) {
  const px = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const u = ((x + 0.5) / w) * 2 - 1;
      const v = ((y + 0.5) / h) * 2 - 1;
      const a = Math.min(1, Math.max(0, alphaAt(u, v, x, y)));
      px.set([255, 255, 255, Math.round(a * 255)], (y * w + x) * 4);
    }
  }
  return texFile(w, h, px, flags);
}

// halo family: soft glows of rising sharpness, halo_N with a faint ring.
particleFiles['halo.tex'] = sprite(128, 128, (u, v) => Math.pow(1 - smooth(0, 1, Math.hypot(u, v)), 2));
// No rim: stock halos are plain glows, and a ring at r=0.7 shows up as a hard circle once
// scenes scale the sprite to 1000+ px (Pac-Man glow children, rain bokeh).
for (let n = 1; n <= 8; n++) {
  const core = 0.1 * n;
  particleFiles[`halo_${n}.tex`] = sprite(128, 128, (u, v) => {
    const r = Math.hypot(u, v);
    // halo_4 is the stock glow sprite of neon scenes (Pac-Man scales it to 1200 px): a short,
    // steep falloff keeps the visible glow near the subject instead of a screen-wide wash.
    if (n === 4) return Math.pow(1 - smooth(0, 1, r), 5) * 0.5;
    const glow = Math.pow(1 - smooth(0, 1, r), 1.5) * (0.6 - 0.04 * n);
    const disk = 1 - smooth(core * 0.5, core * 0.5 + 0.25, r);
    return Math.max(glow, disk * (n > 4 ? 0.9 : 0.5));
  });
}
// Raindrop / water drop: teardrop with a bright centre.
particleFiles['drop.tex'] = sprite(64, 128, (u, v) => {
  const y = v * 0.5 + 0.5; // 0 top .. 1 bottom
  const wdt = 0.1 + 0.8 * Math.pow(y, 1.6) * (1 - smooth(0.7, 1, y) * 0.9);
  const edge = 1 - smooth(wdt * 0.55, wdt, Math.abs(u));
  return edge * smooth(0, 0.08, y) * (1 - smooth(0.93, 1, y));
});
// Ripple ring.
particleFiles['misc/wave.tex'] = sprite(128, 128, (u, v) => {
  const r = Math.hypot(u, v);
  return Math.exp(-(((r - 0.75) / 0.12) ** 2)) * (1 - smooth(0.85, 1, r));
});
// Dot with a slight chromatic fringe look: bright core and soft halo.
particleFiles['chromaticdot.tex'] = sprite(64, 64, (u, v) => {
  const r = Math.hypot(u, v);
  return Math.pow(1 - smooth(0, 1, r), 2.5) * 0.35;
});
// Light shafts: tall soft beams with different widths.
for (let n = 0; n < 5; n++) {
  const wdt = 0.3 + 0.1 * n;
  particleFiles[`light/light_shafts_${n}.tex`] = sprite(128, 256, (u, v) => {
    const across = Math.exp(-((u / wdt) ** 2) * 1.5) * (1 - smooth(0.6, 1, Math.abs(u)));
    const along = smooth(-1, -0.3, v) * (1 - smooth(-0.1, 1, v));
    return across * along * 0.17;
  });
}
// Fog puffs: low-frequency cloudy blobs.
for (let n = 1; n <= 3; n++) {
  const noise = fractal(128, 3 + n, 3, 700 + n * 13);
  particleFiles[`fog/fog${n}.tex`] = sprite(128, 128, (u, v, x, y) => {
    const m = 1 - smooth(0.25, 1, Math.hypot(u, v));
    return m * noise[y * 128 + x] * 1.1;
  });
}
// Smoke wisps.
for (let n = 1; n <= 3; n++) {
  const noise = fractal(128, 2 + n, 4, 900 + n * 17);
  particleFiles[`smoke/smoke${n}.tex`] = sprite(128, 128, (u, v, x, y) => {
    const m = 1 - smooth(0.1, 0.95, Math.hypot(u * 1.1, v));
    return m * smooth(0.25, 0.8, noise[y * 128 + x]);
  });
}
// Bubbles: soft translucent discs (bokeh-like) with a faint rim and highlight. Scenes scale them
// to 50-200 px and stack hundreds additively, so the rim must stay far below the body+glow.
for (let n = 1; n <= 4; n++) {
  particleFiles[`bubbles/bubble${n}.tex`] = sprite(64, 64, (u, v) => {
    const r = Math.hypot(u, v);
    const rim = Math.exp(-(((r - 0.85) / 0.1) ** 2)) * 0.08;
    const body = (1 - smooth(0.5, 1, r)) * 0.2;
    const hl = Math.exp(-((((u + 0.35) ** 2 + (v + 0.35) ** 2) / 0.03))) * 0.15;
    return (rim + body + hl) * (1 - smooth(0.95, 1, r));
  });
}
// Beams: horizontal soft-edged bars; *_fade tapers towards one end.
for (const n of [1, 2]) {
  const thick = n === 1 ? 0.18 : 0.3;
  particleFiles[`beam/beam_${n}.tex`] = sprite(256, 32, (u, v) =>
    Math.exp(-((v / thick) ** 2) * 2) * (1 - smooth(0.9, 1, Math.abs(u))));
  particleFiles[`beam/beam_${n}_fade.tex`] = sprite(256, 32, (u, v) =>
    Math.exp(-((v / thick) ** 2) * 2) * (1 - smooth(-1, 1, u)) * smooth(-1, -0.9, u));
}
// Nature: snow flake (soft dot with a faint cross) and rose petal (pointed ellipse).
particleFiles['nature/snow.tex'] = sprite(64, 64, (u, v) => {
  const r = Math.hypot(u, v);
  const cross = Math.max(Math.exp(-((u / 0.06) ** 2)), Math.exp(-((v / 0.06) ** 2))) * (1 - smooth(0.2, 0.9, r)) * 0.35;
  return Math.max(1 - smooth(0.2, 0.6, r), cross);
});
particleFiles['nature/rosepetals.tex'] = sprite(64, 64, (u, v) => {
  const a = Math.PI / 5;
  const x = u * Math.cos(a) + v * Math.sin(a);
  const y = -u * Math.sin(a) + v * Math.cos(a);
  const body = (x / 0.9) ** 2 + (y / (0.5 * (1 - 0.35 * x))) ** 2;
  return (1 - smooth(0.7, 1, body)) * (0.75 + 0.25 * x);
});

// ---- Extended particle families -------------------------------------------------------------
const segDist = (px, py, ax, ay, bx, by) => {
  const dx = bx - ax;
  const dy = by - ay;
  const t = Math.min(1, Math.max(0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy || 1)));
  return Math.hypot(px - (ax + dx * t), py - (ay + dy * t));
};
const polyDist = (u, v, pts) => {
  let d = Infinity;
  for (let i = 0; i + 1 < pts.length; i++) d = Math.min(d, segDist(u, v, pts[i][0], pts[i][1], pts[i + 1][0], pts[i + 1][1]));
  return d;
};
const rot = (u, v, a) => [u * Math.cos(a) + v * Math.sin(a), -u * Math.sin(a) + v * Math.cos(a)];
const glowDot = (r, k) => Math.pow(1 - smooth(0, 1, r), k);

// Light shafts 5..8 (wider / softer continuation of the 0..4 set).
for (let n = 5; n <= 8; n++) {
  const wdt = 0.15 + 0.1 * n;
  particleFiles[`light/light_shafts_${n}.tex`] = sprite(128, 256, (u, v) => {
    const across = Math.exp(-((u / wdt) ** 2) * 1.5);
    const along = smooth(-1, -0.5, v) * (1 - smooth(-0.2, 1, v));
    return across * along * 0.17;
  });
}
// Flares: soft radial core with N rays (flare_0 has no rays, 1..6 add more).
for (let n = 0; n <= 6; n++) {
  const rays = n === 0 ? 0 : 2 + n;
  particleFiles[`light/flare_${n}.tex`] = sprite(128, 128, (u, v) => {
    const r = Math.hypot(u, v);
    const ang = Math.atan2(v, u);
    const core = glowDot(r, 3) * 0.9;
    const ray = rays
      ? Math.pow(Math.abs(Math.cos((ang * rays) / 2)), 24) * Math.pow(1 - smooth(0, 1, r), 2) * 0.8
      : 0;
    return Math.max(core, ray);
  });
}
// Rain: a 4x2 sprite sheet of short drops (rain0..5). WE rain presets use `randomframe` with
// particles 800-1600 units tall, so every frame is one thin streak with a fading tail, of a
// different length and slant, centred in a square cell. The frame table is TEXS0003.
function spriteSheet(cols, rows, cell, alphaAt, flags = CLAMP_UVS) {
  const w = cols * cell;
  const h = rows * cell;
  const px = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const f = Math.floor(y / cell) * cols + Math.floor(x / cell);
      const u = (((x % cell) + 0.5) / cell) * 2 - 1;
      const v = (((y % cell) + 0.5) / cell) * 2 - 1;
      const a = Math.min(1, Math.max(0, alphaAt(f, u, v)));
      px.set([255, 255, 255, Math.round(a * 255)], (y * w + x) * 4);
    }
  }
  const frames = [];
  for (let f = 0; f < cols * rows; f++) {
    frames.push([(f % cols) * cell, Math.floor(f / cols) * cell]);
  }
  const head = Buffer.concat([cstr('TEXS0003'), i32(frames.length), i32(w), i32(h)]);
  const body = frames.map(([fx, fy]) => {
    const b = Buffer.alloc(32);
    b.writeInt32LE(0, 0); // image id
    b.writeFloatLE(0.1, 4); // frame time
    [fx, fy, cell, 0, 0, cell].forEach((val, i) => b.writeFloatLE(val, 8 + i * 4));
    return b;
  });
  return texFile(w, h, px, flags | 4, Buffer.concat([head, ...body]));
}
for (let n = 0; n <= 5; n++) {
  const wdt = 0.011 + 0.001 * n;
  particleFiles[`nature/rain${n}.tex`] = spriteSheet(4, 2, 128, (f, u, v) => {
    const rand = rng(900 + n * 16 + f);
    const len = 0.8 + 0.15 * rand(); // half-length of the drop
    // Same axis in every frame: systems that cycle the sheet instead of picking one random
    // frame (randomframe) then only change the drop's length, never its position.
    const cx = 0;
    const slant = 0;
    const off = 0;
    const x = u - cx - slant * (v - off) * 6;
    const t = (v - off) / len; // -1 tail .. 1 head
    if (Math.abs(t) > 1) return 0;
    const fade = smooth(-1, 0.3, t) * (1 - smooth(0.85, 1, t));
    return Math.exp(-((x / wdt) ** 2) * 2) * fade * 0.6;
  });
}
// Leaves: white silhouettes with a midrib; shape varies with n.
for (let n = 0; n <= 10; n++) {
  const rand = rng(1300 + n);
  const len = 0.75 + 0.2 * rand();
  const wid = 0.28 + 0.25 * rand();
  const bend = (rand() - 0.5) * 0.5;
  const tilt = rand() * Math.PI;
  const pointy = 1 + 1.5 * rand();
  particleFiles[`nature/leaves${n}.tex`] = sprite(64, 64, (u, v) => {
    const [x, yy] = rot(u, v, tilt);
    const t = x / len; // -1..1 along the leaf
    if (Math.abs(t) > 1) return 0;
    const y = yy - bend * (1 - t * t);
    const half = wid * Math.pow(1 - Math.abs(t), 0.8) * (t > 0 ? 1 : 1 / pointy * 1.4) * Math.sqrt(1 - t * t * 0.3);
    const body = 1 - smooth(half * 0.75, half, Math.abs(y));
    const vein = Math.exp(-((y / 0.025) ** 2)) * 0.25;
    return body * (0.85 - vein);
  });
}
// Lightning: jagged vertical bolts with glow and a side branch.
for (let n = 1; n <= 4; n++) {
  const rand = rng(1700 + n);
  const pts = [];
  const steps = 10 + n * 2;
  let x = (rand() - 0.5) * 0.3;
  for (let i = 0; i <= steps; i++) {
    pts.push([x, -0.95 + (1.9 * i) / steps]);
    x = Math.max(-0.5, Math.min(0.5, x + (rand() - 0.5) * 0.35));
  }
  const mid = pts[Math.floor(steps / 2)];
  const branch = [mid, [mid[0] + (rand() - 0.5) * 0.5, mid[1] + 0.25], [mid[0] + (rand() - 0.5) * 0.9, mid[1] + 0.55]];
  particleFiles[`lightning/lightning${n}.tex`] = sprite(64, 256, (u, v) => {
    const d = Math.min(polyDist(u, v, pts), polyDist(u, v, branch) * 1.6);
    const core = 1 - smooth(0.012, 0.04, d);
    const glow = Math.exp(-((d / 0.12) ** 2)) * 0.45;
    return Math.max(core, glow);
  });
}
// Debris: small irregular chunks (noisy radial outline).
for (let n = 0; n <= 5; n++) {
  const rand = rng(1900 + n);
  const lobes = Array.from({ length: 7 }, () => 0.55 + 0.4 * rand());
  particleFiles[`debris/debris${n}.tex`] = sprite(32, 32, (u, v) => {
    const r = Math.hypot(u, v);
    const f = ((Math.atan2(v, u) + Math.PI) / (2 * Math.PI)) * lobes.length;
    const i = Math.floor(f) % lobes.length;
    const t = f - Math.floor(f);
    const edge = lobes[i] * (1 - t) + lobes[(i + 1) % lobes.length] * t;
    return (1 - smooth(edge - 0.1, edge, r)) * (0.7 + 0.3 * (1 - r));
  });
}
// Rings: plain alpha ring, and normal-map style rings (RGB encodes the surface slope).
particleFiles['ring.tex'] = sprite(128, 128, (u, v) => {
  const r = Math.hypot(u, v);
  return Math.exp(-(((r - 0.7) / 0.08) ** 2)) * (1 - smooth(0.9, 1, r));
});
function normalRing(profile) {
  const w = 128;
  const px = new Uint8Array(w * w * 4);
  for (let y = 0; y < w; y++) {
    for (let x = 0; x < w; x++) {
      const u = ((x + 0.5) / w) * 2 - 1;
      const v = ((y + 0.5) / w) * 2 - 1;
      const r = Math.hypot(u, v) || 1e-6;
      const slope = profile(r); // d(height)/dr
      const nx = (-slope * u) / r;
      const ny = (-slope * v) / r;
      const len = Math.hypot(nx, ny, 1);
      const a = Math.round(255 * (1 - smooth(0.92, 1, r)));
      px.set([Math.round((0.5 + nx / len / 2) * 255), Math.round((0.5 + ny / len / 2) * 255), Math.round((0.5 + 1 / len / 2) * 255), a], (y * w + x) * 4);
    }
  }
  return texFile(w, w, px, CLAMP_UVS);
}
const bump = (r, c, wd) => Math.exp(-(((r - c) / wd) ** 2));
particleFiles['normal_ring.tex'] = normalRing((r) => -2 * ((r - 0.7) / 0.1 ** 2) * bump(r, 0.7, 0.1) * 0.12);
particleFiles['normal_ring_smooth.tex'] = normalRing((r) => -2 * ((r - 0.6) / 0.25 ** 2) * bump(r, 0.6, 0.25) * 0.1);
/** Tangent-space normal sprite (xy encode the slope, y up in sprite space) on a w x h grid. */
function normalSprite(w, h, field, flags = CLAMP_UVS) {
  const px = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const u = ((x + 0.5) / w) * 2 - 1;
      const v = ((y + 0.5) / h) * 2 - 1;
      const [nx, ny, a] = field(u, v);
      const len = Math.hypot(nx, ny, 1);
      px.set([Math.round((0.5 + nx / len / 2) * 255), Math.round((0.5 + ny / len / 2) * 255), Math.round((0.5 + 1 / len / 2) * 255), Math.round(Math.min(1, Math.max(0, a)) * 255)], (y * w + x) * 4);
    }
  }
  return texFile(w, h, px, flags);
}
// Splash: a bright crest ring with a weaker trailing ring inside it (water impact).
particleFiles['normal_splash.tex'] = normalSprite(128, 128, (u, v) => {
  const r = Math.hypot(u, v) || 1e-6;
  const slope = -2 * ((r - 0.72) / 0.09 ** 2) * bump(r, 0.72, 0.09) * 0.1 - 2 * ((r - 0.45) / 0.12 ** 2) * bump(r, 0.45, 0.12) * 0.05;
  return [(-slope * u) / r, (-slope * v) / r, 1 - smooth(0.92, 1, r)];
});
// Pinch / rotate: a swirl (tangential push) around a pinched centre (radial pull).
particleFiles['normal_pinch_rotate.tex'] = normalSprite(128, 128, (u, v) => {
  const r = Math.hypot(u, v) || 1e-6;
  const fall = (1 - smooth(0, 1, r)) * smooth(0, 0.15, r);
  const swirl = 0.7 * fall;
  const pinch = -0.5 * fall;
  return [(-v / r) * swirl + (u / r) * pinch, (u / r) * swirl + (v / r) * pinch, 1 - smooth(0.92, 1, r)];
});
// Droplet normal: dome over the teardrop silhouette of drop.tex, steep at the outline.
particleFiles['drop_normal.tex'] = normalSprite(64, 128, (u, v) => {
  const y = v * 0.5 + 0.5;
  const wdt = 0.1 + 0.8 * Math.pow(y, 1.6) * (1 - smooth(0.7, 1, y) * 0.9);
  const e = Math.min(1, Math.abs(u) / (wdt || 1e-6));
  const lens = Math.pow(e, 3) * 0.9;
  const ey = Math.pow(Math.abs(v), 4) * 0.5;
  const body = 1 - smooth(wdt * 0.55, wdt, Math.abs(u));
  return [Math.sign(u) * lens, Math.sign(v) * ey, body * smooth(0, 0.08, y) * (1 - smooth(0.93, 1, y))];
});
// Bubble normals: a spherical shell, flat in the middle and bending hard towards the rim.
for (let n = 1; n <= 4; n++) {
  particleFiles[`bubbles/bubble${n}normal.tex`] = normalSprite(64, 64, (u, v) => {
    const r = Math.hypot(u, v) || 1e-6;
    const k = Math.pow(Math.min(1, r / 0.9), 2 + n * 0.5) * (0.7 + 0.15 * n);
    return [(u / r) * k, (v / r) * k, 1 - smooth(0.92, 1, r)];
  });
}
// Generic 4-point star sprite (particle/star): bright core, thin spikes.
particleFiles['star.tex'] = sprite(64, 64, (u, v) => {
  const r = Math.hypot(u, v);
  const spike = Math.pow(Math.abs(Math.cos(Math.atan2(v, u) * 2)), 24) * Math.pow(1 - smooth(0, 1, r), 1.8);
  return Math.max(spike, glowDot(r, 3) * 0.8);
});
// Smoke variant for lit smoke: softer and more transparent than smoke2.
{
  const noise = fractal(128, 4, 3, 2101);
  particleFiles['smoke/smoke2light.tex'] = sprite(128, 128, (u, v, x, y) =>
    (1 - smooth(0.05, 1, Math.hypot(u, v))) * (0.25 + 0.5 * noise[y * 128 + x]) * 0.8);
}
// Stars: N-point stars with a glow, N = 4..6.
for (let n = 1; n <= 3; n++) {
  const pts = n + 3;
  particleFiles[`star${n}.tex`] = sprite(64, 64, (u, v) => {
    const r = Math.hypot(u, v);
    const spike = Math.pow(Math.abs(Math.cos((Math.atan2(v, u) * pts) / 2)), 14) * Math.pow(1 - smooth(0, 1, r), 1.6);
    return Math.max(spike, glowDot(r, 3) * 0.8);
  });
}
// Sparkles: thin cross with bright centre.
for (let n = 1; n <= 3; n++) {
  const thin = 0.03 + 0.02 * n;
  particleFiles[`sparkle${n}.tex`] = sprite(64, 64, (u, v) => {
    const r = Math.hypot(u, v);
    const cross = Math.max(Math.exp(-((u / thin) ** 2)) * (1 - smooth(0, 1, Math.abs(v))), Math.exp(-((v / thin) ** 2)) * (1 - smooth(0, 1, Math.abs(u))));
    return Math.max(cross * 0.9, glowDot(r, 4));
  });
}
// Dust: tiny soft specks / mote clusters.
for (let n = 1; n <= 3; n++) {
  const rand = rng(2300 + n);
  const motes = Array.from({ length: n * 3 }, () => [rand() * 1.4 - 0.7, rand() * 1.4 - 0.7, 0.08 + 0.12 * rand()]);
  particleFiles[`dust${n}.tex`] = sprite(32, 32, (u, v) =>
    Math.max(...motes.map(([mx, my, s]) => glowDot(Math.hypot(u - mx, v - my) / s, 2) * 0.8)));
}
// Bokeh: disc with brighter rim; later ones have a harder rim.
for (let n = 1; n <= 3; n++) {
  particleFiles[`bokeh${n}.tex`] = sprite(128, 128, (u, v) => {
    const r = Math.hypot(u, v);
    const disc = 1 - smooth(0.85, 0.95, r);
    return disc * (0.35 + 0.4 * n * smooth(0.55, 0.9, r) * 0.5);
  });
}
// Petals (generic): rounded tip with notch; sakura-like.
for (let n = 1; n <= 3; n++) {
  const wid = 0.35 + 0.1 * n;
  particleFiles[`nature/petal${n}.tex`] = sprite(64, 64, (u, v) => {
    const [x, y] = rot(u, v, Math.PI / 3);
    const t = (x + 0.8) / 1.6;
    if (t < 0 || t > 1) return 0;
    const half = wid * Math.sin(Math.PI * Math.pow(t, 0.7)) * (1 - 0.15 * Math.exp(-(((t - 1) / 0.08) ** 2)));
    return 1 - smooth(half * 0.7, half, Math.abs(y));
  });
}
// Feathers: curved quill with barbs fading outward.
for (let n = 1; n <= 3; n++) {
  const rand = rng(2500 + n);
  const curve = 0.15 + 0.15 * rand();
  const wid = 0.16 + 0.05 * n;
  particleFiles[`feather${n}.tex`] = sprite(32, 128, (u, v) => {
    const t = v; // -1..1 along the feather
    const cx = curve * (1 - t * t);
    const half = wid * Math.sqrt(Math.max(0, 1 - t * t)) * (1 - 0.2 * smooth(-0.8, 0.8, t));
    const dx = Math.abs(u - cx);
    const barbs = 0.8 + 0.2 * Math.sin((dx / (half || 1)) * 0 + t * 60);
    const quill = Math.exp(-((dx / 0.03) ** 2));
    return Math.max((1 - smooth(half * 0.8, half, dx)) * barbs * (1 - 0.4 * smooth(0, 1, dx / (half || 1))), quill * 0.9 * (Math.abs(t) < 0.97 ? 1 : 0));
  });
}
// Snow 0..5: flakes of increasing detail (0 = soft dot, 5 = six-armed crystal).
for (let n = 0; n <= 5; n++) {
  particleFiles[`nature/snow${n}.tex`] = sprite(64, 64, (u, v) => {
    const r = Math.hypot(u, v);
    const dot = 1 - smooth(0.2 + 0.05 * n, 0.65 - 0.03 * n, r);
    if (n < 2) return dot;
    const arms = Math.pow(Math.abs(Math.cos(Math.atan2(v, u) * 3)), 40 - n * 4) * (1 - smooth(0.2, 0.9, r)) * (0.4 + 0.1 * n);
    return Math.max(dot * (1 - 0.12 * n), arms);
  });
}
// Fire: flame tongue, vertical and flickering via noise.
for (let n = 1; n <= 3; n++) {
  const noise = fractal(128, 3 + n, 3, 2700 + n * 7);
  particleFiles[`fire${n}.tex`] = sprite(64, 128, (u, v, x, y) => {
    const t = v * 0.5 + 0.5; // 0 top .. 1 bottom
    const half = 0.65 * Math.pow(t, 0.7) * (1 - 0.3 * t);
    const wobble = (noise[y * 128 + Math.min(127, x * 2)] - 0.5) * 0.25 * (1 - t);
    // Soft, peaked cross-section (not a flat plateau) so overlapping additive flames keep
    // internal gradation instead of saturating into one orange block; noise breaks up the body
    // into licks of flame.
    const d = Math.abs(u + wobble) / (half + 0.001);
    const across = Math.pow(Math.max(0, 1 - d), 1.5);
    const lick = 0.35 + 0.65 * noise[(y * 128 + Math.min(127, x * 2 + 17 * n)) % (128 * 128)];
    return across * smooth(0, 0.25, t) * (1 - smooth(0.85, 1, t)) * (0.4 + 0.6 * t) * lick;
  });
}
// Stock scenes reference the flame sprites as particle/fire/fireN: same shapes under that path.
for (let n = 1; n <= 3; n++) particleFiles[`fire/fire${n}.tex`] = particleFiles[`fire${n}.tex`];
// Sickle: a crescent slash (outer disc minus an offset disc) with a bright inner edge.
particleFiles['sickle.tex'] = sprite(128, 128, (u, v) => {
  const outer = 1 - smooth(0.82, 0.9, Math.hypot(u, v));
  const d = Math.hypot(u - 0.38, v + 0.12);
  const cut = smooth(0.74, 0.8, d);
  const edge = 0.7 + 0.3 * Math.exp(-(((d - 0.8) / 0.1) ** 2));
  return outer * cut * edge;
});
// Embers: small hot cores with a falloff glow.
for (let n = 1; n <= 3; n++) {
  particleFiles[`ember${n}.tex`] = sprite(32, 32, (u, v) =>
    Math.max(1 - smooth(0.1 * n, 0.2 * n + 0.1, Math.hypot(u, v)), glowDot(Math.hypot(u, v), 3) * 0.5));
}
// Glows: soft round glows of increasing size.
for (let n = 1; n <= 3; n++) {
  particleFiles[`glow${n}.tex`] = sprite(128, 128, (u, v) => glowDot(Math.hypot(u, v), 1 + n * 0.8));
}

// Vertical black-to-white ramp used as a mask by blur / blend effects (materials/gradient/...).
const gradientFiles = {};
{
  const w = 16;
  const h = 256;
  const px = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    const g = Math.round((y / (h - 1)) * 255);
    for (let x = 0; x < w; x++) px.set([g, g, g, 255], (y * w + x) * 4);
  }
  gradientFiles['blend_gradient.tex'] = texFile(w, h, px, CLAMP_UVS);
}

// Colour ramps (materials/gradient/...), sampled along x. Our own palettes.
function ramp(stops) {
  const w = 256;
  const h = 4;
  const px = new Uint8Array(w * h * 4);
  for (let x = 0; x < w; x++) {
    const t = x / (w - 1);
    let i = 0;
    while (i < stops.length - 2 && t > stops[i + 1][0]) i++;
    const [t0, c0] = stops[i];
    const [t1, c1] = stops[i + 1];
    const k = Math.min(1, Math.max(0, (t - t0) / (t1 - t0)));
    const c = c0.map((v, j) => Math.round(v + (c1[j] - v) * k));
    for (let y = 0; y < h; y++) px.set([c[0], c[1], c[2], 255], (y * w + x) * 4);
  }
  return texFile(w, h, px, CLAMP_UVS);
}
gradientFiles['gradient_fire.tex'] = ramp([
  [0, [0, 0, 0]],
  [0.25, [120, 10, 0]],
  [0.5, [235, 70, 5]],
  [0.75, [255, 190, 40]],
  [1, [255, 255, 220]],
]);
gradientFiles['gradient_iridescent.tex'] = ramp(
  Array.from({ length: 9 }, (_, i) => {
    const t = i / 8;
    const c = [0, 2, 4].map((o) => Math.round(150 + 105 * Math.cos(6.28318 * (t + o / 6))));
    return [t, c];
  }),
);

// Tileable cell patterns (materials/pattern/...), used by caustics-style effects. `voronoi`
// is a soft glow that peaks on the cell borders; `voronoi_local` is the same network with
// thinner, sharper lines. Both are our own procedural textures.
const patternFiles = {};
{
  const size = 256;
  const cells = 8;
  const rand = rng(61);
  const pts = Array.from({ length: cells * cells }, () => [rand(), rand()]);
  const border = new Float32Array(size * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const u = (x / size) * cells;
      const v = (y / size) * cells;
      const cx = Math.floor(u);
      const cy = Math.floor(v);
      let f1 = Infinity;
      let f2 = Infinity;
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const gx = cx + dx;
          const gy = cy + dy;
          const p = pts[(((gy % cells) + cells) % cells) * cells + (((gx % cells) + cells) % cells)];
          const d = Math.hypot(gx + p[0] - u, gy + p[1] - v);
          if (d < f1) {
            f2 = f1;
            f1 = d;
          } else if (d < f2) {
            f2 = d;
          }
        }
      }
      border[y * size + x] = f2 - f1;
    }
  }
  const glow = border.map((d) => Math.pow(1 - smooth(0, 0.25, d), 2));
  const lines = border.map((d) => Math.pow(1 - smooth(0, 0.08, d), 2));
  patternFiles['voronoi.tex'] = texFile(size, size, toRgba(size, [glow, glow, glow]));
  patternFiles['voronoi_local.tex'] = texFile(size, size, toRgba(size, [lines, lines, lines]));
}

// Uniform white noise, independent per channel (util/uniform_256).
{
  const size = 256;
  const rand = rng(71);
  const ch = () => Float32Array.from({ length: size * size }, rand);
  files['uniform_256.tex'] = texFile(size, size, toRgba(size, [ch(), ch(), ch()]));
}

for (const [dir, entries] of [
  [join(outDir, '../gradient'), gradientFiles],
  [join(outDir, '../pattern'), patternFiles],
  [outDir, files],
  [join(outDir, '../effects'), effectFiles],
  [join(outDir, '../particle'), particleFiles],
]) {
  mkdirSync(dir, { recursive: true });
  for (const [name, bytes] of Object.entries(entries)) {
    mkdirSync(dirname(join(dir, name)), { recursive: true });
    writeFileSync(join(dir, name), bytes);
    console.log(`wrote ${name} (${bytes.length} bytes)`);
  }
}
