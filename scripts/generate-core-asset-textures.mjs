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
function texFile(width, height, rgba, flags = 0) {
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

for (const [dir, entries] of [
  [outDir, files],
  [join(outDir, '../effects'), effectFiles],
]) {
  mkdirSync(dir, { recursive: true });
  for (const [name, bytes] of Object.entries(entries)) {
    writeFileSync(join(dir, name), bytes);
    console.log(`wrote ${name} (${bytes.length} bytes)`);
  }
}
