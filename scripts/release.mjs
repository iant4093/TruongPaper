#!/usr/bin/env node
// scripts/release.mjs
// One-command release build:
//   1. bundles DepotDownloader sidecar into src-tauri/bin/
//   2. builds with Tauri, signing the updater archive with your private key
//   3. writes latest.json (the manifest the in-app updater reads)
//
// Usage: npm run app:build [-- --notes "What changed"] [-- <extra tauri build args>]
// Signing key: $TAURI_SIGNING_PRIVATE_KEY, or ~/.tauri/truongpaper.key (created by `tauri signer generate`).

import { execFileSync, spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const notesIdx = args.indexOf('--notes');
const notes = notesIdx >= 0 ? args.splice(notesIdx, 2)[1] : '';

const conf = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'));
const cfgIdx = args.indexOf('--config');
const override = cfgIdx >= 0 && args[cfgIdx + 1]?.trim().startsWith('{') ? JSON.parse(args[cfgIdx + 1]) : {};
const version = override.version ?? conf.version;
const endpoint = override.plugins?.updater?.endpoints?.[0] ?? conf.plugins?.updater?.endpoints?.[0] ?? '';

// 1. Bundle DepotDownloader sidecar
execFileSync(process.execPath, [join(root, 'scripts', 'bundle-depotdownloader.mjs')], { stdio: 'inherit' });

// 2. Signing key for updater artifacts
const env = { ...process.env };
if (!env.TAURI_SIGNING_PRIVATE_KEY) {
  const keyFile = join(homedir(), '.tauri', 'truongpaper.key');
  if (!existsSync(keyFile)) {
    console.error('[release] No signing key. Run: npx tauri signer generate -w ~/.tauri/truongpaper.key');
    process.exit(1);
  }
  env.TAURI_SIGNING_PRIVATE_KEY = readFileSync(keyFile, 'utf8');
  env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= '';
}

const build = spawnSync('npx', ['tauri', 'build', ...args], { cwd: root, env, stdio: 'inherit' });
if (build.status !== 0) process.exit(build.status ?? 1);

// Catch signing/runtime failures in the actual bundled downloader before delivery.
execFileSync(process.execPath, [join(root, 'scripts', 'verify-depotdownloader.mjs')], { cwd: root, stdio: 'inherit' });

// 3. latest.json — upload it plus the .tar.gz/.exe next to it to the GitHub release v<version>.
const bundleDir = join(root, 'src-tauri', 'target', 'release', 'bundle');
const platformKey = `${process.platform === 'darwin' ? 'darwin' : process.platform === 'win32' ? 'windows' : 'linux'}-${
  process.arch === 'arm64' ? 'aarch64' : 'x86_64'
}`;
const downloadBase = endpoint.includes('/releases/latest/download/')
  ? endpoint.replace(/\/releases\/latest\/download\/.*$/, `/releases/download/v${version}/`)
  : '';

const archives = [];
for (const sub of ['macos', 'nsis', 'msi', 'appimage']) {
  const dir = join(bundleDir, sub);
  if (!existsSync(dir)) continue;
  for (const f of readdirSync(dir)) {
    if (f.endsWith('.sig')) archives.push({ file: f.slice(0, -4), dir, sig: readFileSync(join(dir, f), 'utf8').trim() });
  }
}
if (!archives.length) {
  console.warn('[release] No signed updater archives found (is createUpdaterArtifacts enabled?)');
  process.exit(0);
}
const pick = archives[0];
const manifest = {
  version,
  notes: notes || `TruongPaper ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    [platformKey]: { signature: pick.sig, url: `${downloadBase}${encodeURIComponent(pick.file)}` },
  },
};
const out = join(bundleDir, 'latest.json');
writeFileSync(out, JSON.stringify(manifest, null, 2) + '\n');
console.log(`\n[release] ${pick.file} (+ .sig) and latest.json are in ${bundleDir}`);
console.log(`[release] Upload both to the GitHub release “v${version}”.`);
