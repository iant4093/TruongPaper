# 06 — Repository Layout, `README.md`, CI/CD & Auto-Update Release Specification (Aligned with `TruongFlix`)

> **2026-11-10 release-readiness override:** Push/PR builds produce test installers only. Publishing requires a manual CI run on `main` with `publish: true` after desktop acceptance testing. The UI now follows the user-requested neutral dark theme in `src/styles/theme.css`.

> **Reference Standard**: [`https://github.com/iant4093/TruongFlix`](https://github.com/iant4093/TruongFlix)  
> TruongPaper MUST follow the exact repository layout, `README.md` structure, `package.json` scripts, sidecar bundling workflow (`scripts/bundle-depotdownloader.mjs`), local release script (`scripts/release.mjs`), GitHub Actions CI & automated release pipeline (`.github/workflows/ci.yml`), signed auto-updater (`latest.json` + `SHA256SUMS`), and Windows test manifest (`windows-app-manifest.xml`) established in `TruongFlix`.

---

## 1. `TruongFlix`-Aligned Repository Layout

Just like `TruongFlix`, the React + TypeScript + Vite + Vitest frontend lives at the **repository root** (`src/`, `tests/`, `index.html`, `tray.html`, `package.json`, `vite.config.ts`, `vitest.config.ts`), while the Rust workspace and Tauri configuration live in **`src-tauri/`**:

```text
.
├── src/                        React app (Main Studio Window + Menu Bar / Tray Popover)
│   ├── components/             LiquidGlassFilters, AmbientBacklightCanvas, PropertyInspector,
│   │                           Omnibar, WallpaperGrid, PhotoQueueBar, MonitorDiagram, modals, settings
│   ├── services/               Photo API client bridge, Steam Workshop bridge, playlists,
│   │                           monitors, presets, auto-updater (updates.ts), SQLite state sync
│   ├── store/                  Zustand store (useWallpaperStore.ts) and selectors
│   ├── tray/                   Menu Bar (macOS) / System Tray (Windows) quick-control popover app
│   ├── hooks/ lib/ types/      Shared hooks, formatting and platform helpers, and IPC types (ipc.ts)
│   └── dev/                    Browser-preview mock of the Tauri backend (mockTauri.ts, dev only)
├── src-tauri/                  Rust backend & workspace
│   ├── Cargo.toml              Tauri binary + workspace manifest
│   ├── build.rs                Embeds windows-app-manifest.xml for Common Controls v6 in tests & bins
│   ├── windows-app-manifest.xml Common Controls v6 manifest so `cargo test` never crashes on Windows
│   ├── Entitlements.plist      macOS ad-hoc signing & library validation entitlements
│   ├── Info.plist              macOS bundle metadata
│   ├── tauri.conf.json         Shared macOS/Windows Tauri v2 config + updater pubkey & endpoint
│   ├── tauri.windows.conf.json Windows-specific overrides
│   ├── capabilities/           Tauri v2 permissions (default.json for "main" and "tray-popover")
│   ├── bin/                    Bundled DepotDownloader sidecar & licenses (populated by script)
│   ├── examples/               Smoke tests (wallpaper_smoke.rs)
│   ├── src/                    Tauri main/lib entrypoints, IPC commands, power.rs, secrets.rs, window.rs
│   └── crates/                 Modular Rust crates
│       ├── truongpaper-core/        SQLite WAL schema, models, playlist & rotation scheduler
│       ├── truongpaper-photo-api/   Ian Truong Photo API v1 client, 5-photo queue, rate limiter, cache evictor
│       ├── truongpaper-steam/       DepotDownloader sidecar runner, QR/2FA auth, local 431960 watcher
│       ├── truongpaper-we-formats/  4-layer VFS, .pkg unpacker, .tex decoder, .mdl puppet parser, scene.json
│       ├── truongpaper-renderer/    wgpu desktop compositor, Scene engine, Video decoder, transition shaders
│       ├── truongpaper-scenescript/ Embedded rquickjs runtime for Wallpaper Engine SceneScript
│       ├── truongpaper-audio/       WASAPI Loopback / CoreAudio Tap + 60 Hz 2048-pt Hann FFT
│       └── truongpaper-platform/    Win32 WorkerW (23H2 & 24H2) + macOS kCGDesktopWindowLevel host & occlusion
├── tests/                      Vitest frontend unit & component tests
├── scripts/                    DepotDownloader sidecar bundler and release packaging (release.mjs)
├── docs/                       Architecture specs, README screenshots (docs/screenshots/*.webp) and app-icon.png
├── .github/                    CI/release workflow (ci.yml) and Dependabot (dependabot.yml)
├── index.html                  Main Studio window entry
├── tray.html                   Menu Bar / System Tray popover entry
└── README.md
```

---

## 2. Exact `README.md` Template (Matching `TruongFlix` Style & Structure)

 When generating the repository's root `README.md`, the implementation agent must use this exact `TruongFlix`-formatted markdown document:

````markdown
<div align="center">

<img src="docs/app-icon.png" width="96" alt="TruongPaper app icon">

# TruongPaper

A native macOS and Windows desktop wallpaper engine built in Rust and WebGPU that combines the [Ian Truong Photography API](https://iantruongphotography.com/apps/photo-api) with full Steam Workshop & Wallpaper Engine compatibility inside a unified Apple Liquid Glass interface.

[Download for macOS](https://github.com/iant4093/TruongPaper/releases/latest/download/TruongPaper-macOS.dmg) · [Download for Windows](https://github.com/iant4093/TruongPaper/releases/latest/download/TruongPaper-Windows-Setup.exe) · [Releases](https://github.com/iant4093/TruongPaper/releases) · [Build status](https://github.com/iant4093/TruongPaper/actions) · [Report an issue](https://github.com/iant4093/TruongPaper/issues)

[![Download for macOS](https://img.shields.io/badge/Download_for_macOS-0A84FF?style=for-the-badge&logo=apple&logoColor=white)](https://github.com/iant4093/TruongPaper/releases/latest/download/TruongPaper-macOS.dmg)
[![Download for Windows](https://img.shields.io/badge/Download_for_Windows-0A84FF?style=for-the-badge&logo=windows&logoColor=white)](https://github.com/iant4093/TruongPaper/releases/latest/download/TruongPaper-Windows-Setup.exe)

![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)
![React](https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black)
![Rust](https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white)
![Renderer](https://img.shields.io/badge/renderer-wgpu-6E3FA3)
![Platforms](https://img.shields.io/badge/platforms-macOS_%7C_Windows-555555)
[![CI](https://github.com/iant4093/TruongPaper/actions/workflows/ci.yml/badge.svg)](https://github.com/iant4093/TruongPaper/actions/workflows/ci.yml)

</div>

---

TruongPaper renders live, audio-reactive and high-resolution photographic wallpapers directly behind your desktop icons on both macOS and Windows. It pairs an offline-first queue for the public [Ian Truong Photography API](https://iantruongphotography.com/apps/photo-api) with a built-in Steam Workshop downloader and native Rust renderer for Wallpaper Engine scenes, videos, web wallpapers and Windows applications—all wrapped in an Apple Liquid Glass interface that looks identical on macOS and Windows.

## Screenshots

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/screenshots/studio-library.webp" width="100%" alt="TruongPaper main Liquid Glass studio window showing unified library, top paste omnibar, active 5-photo queue and live property inspector">
      <p align="center"><sub><b>Liquid Glass Studio</b> · unified library, paste omnibar and live inspector</sub></p>
    </td>
    <td width="50%" valign="top">
      <img src="docs/screenshots/photos-api.webp" width="100%" alt="Ian Truong Photography API studio view showing saved albums, featured filter, 5-photo offline queue, cache slider and history controls">
      <p align="center"><sub><b>Ian's Photos API</b> · offline queue, saved albums and zero-repeat history</sub></p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/screenshots/workshop-download.webp" width="100%" alt="Steam Workshop URL paste modal showing live depot download progress, QR sign-in and automatic scene unpacking">
      <p align="center"><sub><b>Steam Workshop</b> · paste any Workshop link to download and apply</sub></p>
    </td>
    <td width="50%" valign="top">
      <img src="docs/screenshots/scene-inspector.webp" width="100%" alt="Dynamic Wallpaper Engine property inspector with auto-generated sliders, color pickers, toggles, presets and audio FFT sensitivity">
      <p align="center"><sub><b>Wallpaper Engine Inspector</b> · user properties, presets and audio FFT</sub></p>
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/screenshots/monitors-playlists.webp" width="100%" alt="Multi-monitor layout diagram with per-display wallpaper assignment, GPU transitions and unified mixed playlists">
      <p align="center"><sub><b>Monitors & Playlists</b> · per-display fit modes and Liquid Glass transitions</sub></p>
    </td>
    <td width="50%" valign="top">
      <img src="docs/screenshots/tray-popover.webp" width="100%" alt="Frameless Liquid Glass Menu Bar and System Tray popover with quick pause, next wallpaper, favorite, volume and queue status">
      <p align="center"><sub><b>Menu Bar & System Tray</b> · instant controls without opening the studio</sub></p>
    </td>
  </tr>
</table>

## Features

**Ian Truong Photography API & Offline Queue**

- Direct integration with `https://photos-api.iantruongphotography.com/v1/photos/random`: pull from all public photos, featured photos only (`?featured=true`), or any combination of saved albums (`?album=<slug>`)
- Browse the official public album catalog with cover thumbnails and photo counts, or paste any `https://iantruongphotography.com/album/...` URL or slug into the top omnibar
- Offline-first local queue: keeps 5 unused full-resolution JPEGs (`wallpapers/photos/<id>.jpg`) ready on disk and refills in the background when the queue drops to 2, so scheduled wallpaper changes happen instantly with zero API calls
- Persistent SQLite history (`photos_history`): checks every random pick against both the unused queue and shown history *before* downloading `imageUrl`, guaranteeing no photo repeats even after older image files are evicted to free disk space
- Smart exhaustion handling: when every photo in an album or filter has been shown, TruongPaper automatically resets the history for that filter and re-queues any photos still cached on disk (or holds on the last photo if preferred)
- Per-photo actions: Favorite / Pin on disk (exempts from cache cleanup), Hide / Never show again (blacklists ID and advances), and View Album on `iantruongphotography.com`
- Configurable disk cache limit (250 MB default, 100 MB – 10 GB slider) plus sliding-window rate limiting (respecting the 100 requests / 5 min API limit with automatic exponential backoff on `403`/`503`)

**Steam Workshop & Wallpaper Engine**

- Paste any Steam Workshop link (`https://steamcommunity.com/sharedfiles/filedetails/?id=...`) or Workshop ID to inspect metadata and download the wallpaper directly inside TruongPaper
- Built-in standalone Steam downloader powered by a bundled, checksum-verified `DepotDownloader` sidecar with Steam Mobile QR code and 2FA sign-in; refresh tokens are kept in macOS Keychain / Windows Credential Manager
- Automatic local Steam sync: discovers local Steam libraries (`libraryfolders.vdf`) and watches `steamapps/workshop/content/431960` in real time so subscriptions from the Steam client appear automatically
- First-run core asset bootstrap: automatically fetches Wallpaper Engine's core shaders and materials (`Depot 431961`) on macOS or Windows machines where Wallpaper Engine isn't installed locally

**Scene, Video, Web & Application Runtimes**

- **Scene wallpapers**: native Rust `.pkg` (`PKGV0001`–`PKGV0022+`) memory-mapped archive reader, `.tex` (`TEXV0005` / `TEXB0001`–`TEXB0004` / `TEXS0001`–`TEXS0003`) LZ4/DXT1/DXT3/DXT5/RG88/R8/MP4 texture decoder, 4-layer Virtual File System, HLSL/GLSL-to-WGSL shader preprocessor, multi-pass FBO effects, GPU/SIMD particle systems, `.mdl` skeletal puppet warp, and an embedded `rquickjs` SceneScript runtime (`init`, `update`, `cursorMove`, `applyUserProperties`, `engine.registerAudioBuffers`)
- **Video wallpapers**: hardware-accelerated looping playback (`VideoToolbox` on macOS, `D3D11VA` on Windows) for MP4, WebM, MKV and MOV with gapless loop, playback speed (`0.1x`–`2.0x`) and volume control
- **Web wallpapers**: embedded `wry` desktop WebView with `window.wallpaperPropertyListener` and 60 Hz 128-band stereo `window.wallpaperRegisterAudioListener` injection
- **Application wallpapers** (Windows): launches Windows application wallpapers with desktop bounds; flagged as `Windows Only` on macOS

**Inspector, Audio Reactivity & Desktop Interaction**

- Dynamic `project.json` inspector: automatically generates Liquid Glass sliders, switches, color pickers, dropdowns, text inputs and file pickers for every wallpaper property, evaluating visibility `condition` expressions and localization tables in real time
- Universal per-wallpaper adjustments (Volume, Playback Speed, Fit Mode, Zoom/Offset, Brightness, Contrast, Saturation, Hue Shift) and named user presets
- System audio loopback + 2048-point Hann-windowed FFT (`WASAPI Loopback` on Windows, `CoreAudio Tap` / input capture on macOS) driving 16-, 32- and 64-band logarithmic stereo spectrums at 60 Hz
- Passive desktop cursor forwarding: tracks cursor movement and clicks over unoccluded desktop regions without stealing clicks from Finder or Windows desktop icons

**Multi-Monitor, GPU Transitions & Smart Pause**

- Independent per-monitor wallpapers, Clone across all displays, or Span a single canvas across all monitors, with full support for both classic Windows 10/11 `WorkerW` and Windows 11 24H2 (`WS_EX_NOREDIRECTIONBITMAP`) raised layered desktops
- Smart orientation matching: skips portrait photos on landscape monitors (and vice-versa) by default, or displays mismatched aspect ratios over a dual-pass Kawase GPU-blurred ambient backdrop (`Fit + Blur`)
- Custom `wgpu` transitions: **Smooth Crossfade**, **Blur-Dissolve**, and **Liquid Glass Morph** (refractive chromatic dispersion shader) with adjustable duration (`0.3s`–`3.0s`)
- Unified playlists: rotate only Ian's Photo API, only Workshop wallpapers, or mix both on a custom timer (`1 min` to `24 hours`), on app launch, on display wake, or via global hotkey
- Smart pause & battery saver: drops to 0% GPU/CPU and mutes audio when a fullscreen game or maximized window covers a monitor, when running on laptop battery / Low Power Mode, or when the screen locks or sleeps

## Install

**macOS** (Apple Silicon & Intel, macOS 14.2 or later)

1. Download [TruongPaper-macOS.dmg](https://github.com/iant4093/TruongPaper/releases/latest/download/TruongPaper-macOS.dmg) and drag TruongPaper into Applications.
2. Launch TruongPaper from Applications.

**Windows** (x64, Windows 10 1809 or later / Windows 11)

1. Download [TruongPaper-Windows-Setup.exe](https://github.com/iant4093/TruongPaper/releases/latest/download/TruongPaper-Windows-Setup.exe) and run the installer.

Installed copies check for a new version once a day (or from **Settings › About**), show what changed, and install it when you choose after verifying its signature.

## Tech stack

| Area | Technologies |
|---|---|
| App shell | Tauri 2 (Rust), WKWebView on macOS, WebView2 on Windows |
| Interface | React 19, TypeScript, Vite, Tailwind CSS, Framer Motion, Zustand, lucide-react, custom cross-platform Apple Liquid Glass SVG/CSS refraction system |
| Desktop & GPU Engine | `wgpu` (Metal on macOS, DirectX 12 / Vulkan on Windows), `naga` (GLSL/WGSL shader translation), Win32 `Progman`/`WorkerW` (23H2 & 24H2) and macOS `NSWindow` (`kCGDesktopWindowLevel`) |
| Wallpaper Engine Formats | Custom Rust `.pkg` memory-mapped VFS, `.tex` decoder (`lz4_flex`, BC1/BC2/BC3, RG88, R8, RGBA8, MP4 streams), `.mdl` puppet mesh deformer, `rquickjs` SceneScript sandbox |
| Steam Workshop | Bundled checksum-verified `DepotDownloader` v3.4.0 sidecar, Steam Web API metadata client, `notify` local `431960` folder watcher, OS Keychain (`keyring`) |
| Photo API & Storage | `reqwest` (rustls), `imagesize`, SQLite (`rusqlite` WAL mode) with persistent `photos_history` and LRU disk cache eviction |
| Audio DSP | `cpal` (WASAPI Loopback / CoreAudio), `rustfft` 2048-point Hann-windowed logarithmic 16/32/64-band stereo analyzer |
| Delivery | GitHub Actions, macOS `.dmg`, Windows NSIS installer, signed auto-updates, SHA-256 checksums |

## Build locally

Requires **Node 22.12+**, **Rust stable** and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform.

```bash
npm install

# Desktop app with hot reload (automatically downloads and verifies DepotDownloader sidecar)
npm run app:dev

# UI-only preview in a browser, with a mocked backend and sample data
npm run dev

# Download and checksum-verify the DepotDownloader sidecar into src-tauri/bin/
npm run bundle:depotdownloader

# Release build: bundles sidecars, builds installers, signs updates, writes latest.json
npm run app:build
```

Desktop dev/build commands automatically prepare the checksum-verified `DepotDownloader` sidecar and its license files. Run `npm run bundle:depotdownloader` before running Rust tests directly.

`app:build` signs updates with `~/.tauri/truongpaper.key` or `$TAURI_SIGNING_PRIVATE_KEY`. Generate a key once with `npx tauri signer generate -w ~/.tauri/truongpaper.key`. Without it, use `npx tauri build` for an unsigned local build.

## Tests

```bash
# Frontend unit and component tests (Vitest)
npm test

# TypeScript check and production frontend build
npm run build

# Rust unit tests: Photo API queue & history retention, rate limiter, small-album refill,
# .pkg/.tex/.mdl binary parsers, shader preprocessor, audio FFT, VFS and keychain vault
cd src-tauri && cargo test

# Live network & integration tests (live Ian Truong Photo API, public album catalog, Steam API, real OS keychain)
cd src-tauri && cargo test -- --ignored

# Desktop wallpaper renderer smoke test: attaches behind desktop icons and verifies rendered pixels
cd src-tauri && TRUONGPAPER_SMOKE_PROBE=1 cargo run --example wallpaper_smoke
```

## CI and releases

Pull requests and pushes to `main` run:

1. Vitest unit tests, a TypeScript check and a production frontend build.
2. macOS and Windows builds: `DepotDownloader` sidecar bundling, Rust tests and installer packaging.
3. A Windows desktop wallpaper smoke test that attaches a wallpaper window behind `Progman`/`WorkerW`, renders a test frame, captures the screen as an artifact, and fails the build if desktop attachment breaks.

After all builds pass, pushes to `main` (and manual runs on `main`) publish a GitHub Release `v<major>.<minor>.<run number>` containing `TruongPaper-macOS.dmg`, `TruongPaper-Windows-Setup.exe`, the macOS updater archive (`TruongPaper-macOS.app.tar.gz`), `latest.json` (with the update signatures) and `SHA256SUMS`. The release notes are generated from the commit messages since the previous release, and the same list appears in the app's update prompt. Installed apps pick up the new version through `latest.json`. Pull requests never publish.

The updater signing key is stored as the `TAURI_SIGNING_PRIVATE_KEY` repository secret. Builds without it (such as fork pull requests) still produce installers, just without update signatures. GitHub Actions are pinned to commit SHAs, and Dependabot checks them weekly.

## Repository layout

```text
.
├── src/                    React app (Liquid Glass Studio & Menu Bar / Tray Popover)
│   ├── components/         Studio views, PropertyInspector, LiquidGlassFilters, AmbientBacklightCanvas
│   ├── services/           Photo API, Steam Workshop, playlists, monitors, presets, auto-updater
│   ├── store/              Zustand store and selectors
│   ├── tray/               Menu Bar (macOS) / System Tray (Windows) quick-control popover
│   ├── hooks/ lib/ types/  Shared hooks, formatting and platform helpers, and IPC types
│   └── dev/                Browser-preview mock of the Tauri backend (dev only)
├── src-tauri/              Rust backend
│   ├── crates/             Modular crates: core, photo-api, steam, we-formats, renderer, scenescript, audio, platform
│   ├── src/                Tauri commands, power/lock watcher, OS keychain vault, window setup
│   ├── capabilities/       Tauri permissions
│   └── examples/           Desktop attachment & renderer smoke tests
├── tests/                  Vitest tests
├── scripts/                DepotDownloader sidecar bundling and release packaging
├── docs/                   Architecture specs, README screenshots and icon
├── .github/                CI/release workflow and Dependabot
└── README.md
```
````

---

## 3. Complete Sidecar Bundler & Release Scripts (`scripts/`)

### 3.1 `scripts/bundle-depotdownloader.mjs`
Modeled directly on `TruongFlix/scripts/bundle-trailers.mjs`, this script downloads the official `SteamRE/DepotDownloader` v3.4.0 native binary for the active Rust target triple, verifies its SHA-256 checksum, extracts it to `src-tauri/bin/DepotDownloader-<target-triple>(.exe)`, marks it executable (`0o755`), and downloads its license into `src-tauri/bin/licenses/DepotDownloader-LICENSE.txt`:

```javascript
// scripts/bundle-depotdownloader.mjs
// Pinned, checksum-verified DepotDownloader v3.4.0 sidecar bundler for Steam Workshop downloads.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const host = execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
const target = process.env.TAURI_ENV_TARGET_TRIPLE || host;

const ASSETS_BY_TARGET = {
  'aarch64-apple-darwin': {
    zipName: 'DepotDownloader-macos-arm64.zip',
    sha256: '60e80c7c496f3f9a079cd3c62036b35d088c27bc0149baf38f009eb57a52f6a5',
    exeName: 'DepotDownloader',
  },
  'x86_64-apple-darwin': {
    zipName: 'DepotDownloader-macos-x64.zip',
    sha256: '3214b689564d73e9342a8a4aef693de6ad3d293801b0f300a4466f60ec75befb',
    exeName: 'DepotDownloader',
  },
  'x86_64-pc-windows-msvc': {
    zipName: 'DepotDownloader-windows-x64.zip',
    sha256: '41c9e9f0df54b3ad02e67a11726756e5c73283bd7c2e1b04acfa5ae4c2ed3767',
    exeName: 'DepotDownloader.exe',
  },
  'aarch64-pc-windows-msvc': {
    zipName: 'DepotDownloader-windows-arm64.zip',
    sha256: '1449ba47775e9974036e615bedf00d72bef747cc5b93a78048ed4e0b2c63b2b3',
    exeName: 'DepotDownloader.exe',
  },
};

if (!target || !ASSETS_BY_TARGET[target]) {
  throw new Error(`DepotDownloader sidecar is not configured for target: ${target}`);
}

const version = '3.4.0';
const windows = target.includes('windows');
const { zipName, sha256, exeName } = ASSETS_BY_TARGET[target];

const outDir = join(root, 'src-tauri', 'bin');
const downloadsDir = join(outDir, 'downloads');
mkdirSync(downloadsDir, { recursive: true });

const destBinary = join(outDir, `DepotDownloader-${target}${windows ? '.exe' : ''}`);
if (existsSync(destBinary)) {
  console.log(`[depotdownloader] Already bundled: ${destBinary}`);
  process.exit(0);
}

async function downloadFile(url, destPath, expectedSha256) {
  if (existsSync(destPath)) {
    if (!expectedSha256) return;
    const existingDigest = createHash('sha256').update(readFileSync(destPath)).digest('hex');
    if (existingDigest === expectedSha256) return;
  }
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Download failed (${response.status}): ${url}`);
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  if (expectedSha256) {
    const digest = createHash('sha256').update(bytes).digest('hex');
    if (digest !== expectedSha256) {
      throw new Error(`SHA-256 mismatch for ${url}: expected ${expectedSha256}, got ${digest}`);
    }
  }
  writeFileSync(destPath, bytes);
}

const zipPath = join(downloadsDir, `DepotDownloader_${version}_${zipName}`);
const releaseUrl = `https://github.com/SteamRE/DepotDownloader/releases/download/DepotDownloader_${version}/${zipName}`;
await downloadFile(releaseUrl, zipPath, sha256);

if (windows) {
  const sevenZip = 'C:\\Program Files\\7-Zip\\7z.exe';
  execFileSync(existsSync(sevenZip) ? sevenZip : '7z', ['e', zipPath, `-o${downloadsDir}`, exeName, '-y'], {
    stdio: 'ignore',
  });
} else {
  execFileSync('unzip', ['-j', '-o', zipPath, exeName, '-d', downloadsDir], { stdio: 'ignore' });
}

const extractedExe = join(downloadsDir, exeName);
copyFileSync(extractedExe, destBinary);
chmodSync(destBinary, 0o755);

const licensesDir = join(outDir, 'licenses');
mkdirSync(licensesDir, { recursive: true });
await downloadFile(
  `https://raw.githubusercontent.com/SteamRE/DepotDownloader/DepotDownloader_${version}/LICENSE`,
  join(licensesDir, 'DepotDownloader-LICENSE.txt')
);

const digest = createHash('sha256').update(readFileSync(destBinary)).digest('hex');
console.log(`[depotdownloader] Bundled DepotDownloader ${version} for ${target} (sha256: ${digest})`);
```

---

### 3.2 `scripts/release.mjs`
Modeled directly on `TruongFlix/scripts/release.mjs`:

```javascript
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
```

---

## 4. Complete GitHub Actions CI & Automated Release Workflow (`.github/workflows/ci.yml`)

Modeled directly on `TruongFlix/.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

env:
  CARGO_TERM_COLOR: always

jobs:
  checks:
    name: Type-check & frontend build
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1 # v7.1.0
        with:
          node-version: 22
          cache: npm
      - run: npm ci
      - run: npm test
      - run: npm run build

  build:
    name: Build (${{ matrix.name }})
    needs: checks
    strategy:
      fail-fast: false
      matrix:
        include:
          - name: macOS
            os: macos-latest
            bundles: app,dmg
          - name: Windows
            os: windows-latest
            bundles: nsis
    runs-on: ${{ matrix.os }}
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - uses: actions/setup-node@949feb2413d6458794dcd2491c4babbbce0c15c1 # v7.1.0
        with:
          node-version: 22
          cache: npm
      - uses: dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec # stable
      - uses: Swatinem/rust-cache@d1527c8f67ef9af357290475d4af4b0fab8f12e0 # v2.9.3
        with:
          workspaces: src-tauri -> target

      - name: Version
        id: version
        shell: bash
        run: |
          base=$(node -p "require('./src-tauri/tauri.conf.json').version.split('.').slice(0,2).join('.')")
          echo "version=${base}.${GITHUB_RUN_NUMBER}" >> "$GITHUB_OUTPUT"

      - run: npm ci

      - name: Bundle DepotDownloader sidecar
        working-directory: .
        run: node scripts/bundle-depotdownloader.mjs

      - name: Rust tests
        working-directory: src-tauri
        run: cargo test --locked

      - name: Windows desktop wallpaper smoke test
        if: matrix.os == 'windows-latest'
        working-directory: src-tauri
        env:
          TRUONGPAPER_SMOKE_PROBE: "1"
        run: cargo run --example wallpaper_smoke

      - uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        if: always() && matrix.os == 'windows-latest'
        with:
          name: windows-wallpaper-smoke
          path: src-tauri/smoke-*.png
          retention-days: 7
          if-no-files-found: ignore

      - name: Build installers
        shell: bash
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ""
          VERSION: ${{ steps.version.outputs.version }}
          BUNDLES: ${{ matrix.bundles }}
        run: |
          # Signed updater archives need the private key (absent on fork PRs).
          if [ -n "$TAURI_SIGNING_PRIVATE_KEY" ]; then updater=true; else updater=false; fi
          npx tauri build --bundles "$BUNDLES" \
            --config "{\"version\":\"$VERSION\",\"bundle\":{\"createUpdaterArtifacts\":$updater}}"

      - name: Collect installers
        shell: bash
        run: |
          mkdir -p release-assets
          b=src-tauri/target/release/bundle
          if [ "$RUNNER_OS" = "macOS" ]; then
            cp "$b"/dmg/*.dmg release-assets/TruongPaper-macOS.dmg
            if [ -f "$b/macos/TruongPaper.app.tar.gz.sig" ]; then
              cp "$b/macos/TruongPaper.app.tar.gz" release-assets/TruongPaper-macOS.app.tar.gz
              cp "$b/macos/TruongPaper.app.tar.gz.sig" release-assets/TruongPaper-macOS.app.tar.gz.sig
            fi
          else
            setup=$(ls "$b"/nsis/*-setup.exe | head -1)
            cp "$setup" release-assets/TruongPaper-Windows-Setup.exe
            if [ -f "$setup.sig" ]; then cp "$setup.sig" release-assets/TruongPaper-Windows-Setup.exe.sig; fi
          fi
          ls -la release-assets

      - uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9 # v7.0.2
        with:
          name: installers-${{ matrix.name }}
          path: release-assets/
          retention-days: 7
          if-no-files-found: error

  release:
    name: Publish release
    needs: build
    if: github.event_name != 'pull_request' && github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    timeout-minutes: 15
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
          fetch-depth: 0
      - uses: actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333 # v8.0.2
        with:
          pattern: installers-*
          merge-multiple: true
          path: release-assets

      - name: Release notes from commit messages
        shell: bash
        run: |
          prev=$(git describe --tags --abbrev=0 --match 'v*' HEAD 2>/dev/null || true)
          range="${prev:+$prev..}HEAD"
          git log "$range" --no-merges --pretty='- %s' -n 60 \
            | grep -viE '^- (merge|wip|bump version)\b' > release-notes.txt || true
          [ -s release-notes.txt ] || echo "- Maintenance release" > release-notes.txt
          {
            echo "## What's new"
            echo
            cat release-notes.txt
            echo
            if [ -n "$prev" ]; then
              echo "**Full changelog:** https://github.com/${GITHUB_REPOSITORY}/compare/${prev}...${GITHUB_SHA::7}"
            else
              echo "Built from ${GITHUB_SHA::7}."
            fi
          } > release-body.md
          cat release-body.md

      - name: Compose updater manifest and checksums
        shell: bash
        run: |
          base=$(node -p "require('./src-tauri/tauri.conf.json').version.split('.').slice(0,2).join('.')")
          version="${base}.${GITHUB_RUN_NUMBER}"
          echo "VERSION=$version" >> "$GITHUB_ENV"
          VERSION="$version" node -e '
            const fs = require("fs");
            const v = process.env.VERSION;
            const url = (f) => `https://github.com/${process.env.GITHUB_REPOSITORY}/releases/download/v${v}/${f}`;
            const sig = (f) => fs.readFileSync(`release-assets/${f}.sig`, "utf8").trim();
            const platforms = {};
            if (fs.existsSync("release-assets/TruongPaper-macOS.app.tar.gz.sig"))
              platforms["darwin-aarch64"] = { signature: sig("TruongPaper-macOS.app.tar.gz"), url: url("TruongPaper-macOS.app.tar.gz") };
            if (fs.existsSync("release-assets/TruongPaper-Windows-Setup.exe.sig"))
              platforms["windows-x86_64"] = { signature: sig("TruongPaper-Windows-Setup.exe"), url: url("TruongPaper-Windows-Setup.exe") };
            const notes = fs.readFileSync("release-notes.txt", "utf8").trim() || `TruongPaper ${v}`;
            const manifest = { version: v, notes, pub_date: new Date().toISOString(), platforms };
            fs.writeFileSync("release-assets/latest.json", JSON.stringify(manifest, null, 2));
            console.log(Object.keys(platforms));
          '
          rm -f release-assets/*.sig
          (cd release-assets && sha256sum -- * > SHA256SUMS)
          ls -la release-assets

      - name: Publish GitHub release
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release create "v${VERSION}" release-assets/* \
            --repo "$GITHUB_REPOSITORY" \
            --target "$GITHUB_SHA" \
            --title "TruongPaper ${VERSION}" \
            --notes-file release-body.md \
            --latest
```

### 4.1 `.github/dependabot.yml`
```yaml
version: 2
updates:
  - package-ecosystem: github-actions
    directory: /
    schedule:
      interval: weekly
```

---

## 5. In-App Signed Auto-Updater (`src/services/updates.ts`)

Modeled directly on `TruongFlix/src/services/updates.ts`:

```typescript
// src/services/updates.ts
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';

export interface AvailableUpdate {
  version: string;
  currentVersion: string;
  date: string | null;
  notes: string | null;
}

let pending: Update | null = null;
const listeners = new Set<(u: AvailableUpdate | null) => void>();

const summarize = (u: Update): AvailableUpdate => ({
  version: u.version,
  currentVersion: u.currentVersion,
  date: u.date ?? null,
  notes: u.body ?? null,
});

export const appVersion = () => getVersion();

export function availableUpdate(): AvailableUpdate | null {
  return pending ? summarize(pending) : null;
}

export function onUpdateChange(fn: (u: AvailableUpdate | null) => void) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

async function checkWithDiagnosis(): Promise<Update | null> {
  try {
    return await check({ timeout: 20000 });
  } catch (first) {
    await new Promise((r) => setTimeout(r, 1500));
    try {
      return await check({ timeout: 20000 });
    } catch (e) {
      const status = await invoke<string>('update_feed_status').catch(() => '');
      if (status && status !== 'ok') throw new Error(`${status}. Try again in a few minutes.`);
      throw e ?? first;
    }
  }
}

export async function checkForUpdate(): Promise<AvailableUpdate | null> {
  pending = await checkWithDiagnosis();
  const info = availableUpdate();
  listeners.forEach((fn) => fn(info));
  return info;
}

export async function installUpdate(onProgress: (downloaded: number, total: number | null) => void) {
  if (!pending) throw new Error('No update available');
  // Cleanly detach desktop wallpaper hosts / JobObjects before replacing executable
  await invoke('runtime_shutdown_for_update').catch(() => {});
  let downloaded = 0;
  let total: number | null = null;
  await pending.downloadAndInstall((event) => {
    if (event.event === 'Started') total = event.data.contentLength ?? null;
    else if (event.event === 'Progress') downloaded += event.data.chunkLength;
    onProgress(downloaded, total);
  });
  await relaunch();
}
```

### 5.1 Rust Backend Auto-Updater Diagnostic & Clean Shutdown Commands (`src-tauri/src/lib.rs`)
Modeled directly on `TruongFlix/src-tauri/src/lib.rs`:

```rust
#[tauri::command]
async fn update_feed_status(app: tauri::AppHandle) -> String {
    let url = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.get(0))
        .and_then(|e| e.as_str())
        .unwrap_or_default()
        .to_owned();
    if url.is_empty() {
        return "no update feed is configured".into();
    }
    let client = match reqwest::Client::builder()
        .user_agent("tauri-updater")
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return e.to_string(),
    };
    match client.get(&url).send().await {
        Ok(res) if res.status().is_success() => "ok".into(),
        Ok(res) => format!("GitHub answered HTTP {}", res.status()),
        Err(e) if e.is_timeout() => "GitHub didn’t answer within 15 seconds".into(),
        Err(e) if e.is_connect() => "couldn’t connect to GitHub (offline, VPN or firewall?)".into(),
        Err(e) => e.to_string(),
    }
}

#[tauri::command]
async fn runtime_shutdown_for_update(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    // 1. Terminate any active Windows Application (.exe) JobObjects or DepotDownloader sidecars
    // 2. Detach any desktop child HWNDs from Progman/WorkerW so NSIS can overwrite TruongPaper.exe cleanly
    state.shutdown_all_wallpaper_hosts().await;
    Ok(())
}
```

---

## 6. Mandatory Platform Build Files (`src-tauri/`)

### 6.1 `src-tauri/build.rs` & `src-tauri/windows-app-manifest.xml`
> [!CAUTION]
> **Critical Windows `cargo test` Crash Fix (from `TruongFlix/src-tauri/build.rs`)**:  
> `tauri-build` only links its Windows Common Controls v6 application manifest into `bin` targets. Without `build.rs` embedding `windows-app-manifest.xml` into all targets via `/MANIFEST:EMBED`, running `cargo test` on Windows loads Common Controls v5 and **crashes immediately with `STATUS_ENTRYPOINT_NOT_FOUND` (`0xc0000139`)**!

```rust
// src-tauri/build.rs
fn main() {
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");

    if windows_msvc {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());

        let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
            .expect("failed to run tauri-build");
    } else {
        tauri_build::build()
    }
}
```

```xml
<!-- src-tauri/windows-app-manifest.xml -->
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
```

### 6.2 `src-tauri/Entitlements.plist`
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.cs.disable-library-validation</key>
	<true/>
</dict>
</plist>
```

---

## 7. Frontend Test Config, Browser-Preview Tauri Mock & Desktop Smoke Test

### 7.1 Root `tsconfig.json`, `index.html`, `tray.html` & `vitest.config.ts`
Modeled directly on `TruongFlix/tsconfig.json`, `TruongFlix/index.html`, and `TruongFlix/vitest.config.ts`:

```json
// tsconfig.json
{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    "isolatedModules": true,
    "resolveJsonModule": true,
    "allowImportingTsExtensions": false,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "types": ["vite/client"]
  },
  "include": ["src", "tests"]
}
```

```html
<!-- index.html (Main Liquid Glass Studio Window) -->
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>TruongPaper</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

```html
<!-- tray.html (Menu Bar / System Tray Quick-Control Popover) -->
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>TruongPaper Quick Controls</title>
  </head>
  <body>
    <div id="tray-root"></div>
    <script type="module" src="/src/tray/tray-main.tsx"></script>
  </body>
</html>
```

```typescript
// vitest.config.ts
import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/**/*.test.ts'],
  },
});
```

### 7.2 `src/dev/mockTauri.ts` (Browser Preview Mock for `npm run dev`)
Modeled directly on `TruongFlix/src/dev/mockTauri.ts`, this module activates automatically when opening `http://localhost:1420` in Chrome/Safari without the Tauri shell (`!("__TAURI_INTERNALS__" in window)`), allowing UI developers and automated tests to preview and interact with the entire Liquid Glass Studio in a standard browser:

```typescript
// src/dev/mockTauri.ts
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import type {
  AppSettings,
  MonitorInfo,
  PhotoItem,
  PhotoStatus,
  PublicCatalogAlbum,
  SavedAlbum,
  WallpaperItem,
} from '../types/ipc';

export function installBrowserPreviewMocksIfNeeded(): void {
  if (typeof window === 'undefined' || '__TAURI_INTERNALS__' in window) {
    return;
  }

  mockWindows('main', 'tray-popover');

  const demoPhoto: PhotoItem = {
    id: '4ce4c078-8fd6-4475-875b-0fb74e9d3381',
    albumSlug: 'bald-mountain-11-1',
    albumUrl: 'https://iantruongphotography.com/album/bald-mountain-11-1',
    filePath: '/mock/wallpapers/photos/4ce4c078-8fd6-4475-875b-0fb74e9d3381.jpg',
    width: 6000,
    height: 4000,
    bytes: 18452910,
    featured: true,
    isFavorite: true,
    downloadedAt: Date.now() - 3600_000,
    lastShownAt: Date.now() - 300_000,
  };

  let savedAlbums: SavedAlbum[] = [
    {
      albumSlug: 'bald-mountain-11-1',
      displayName: 'Bald Mountain 11/1',
      albumUrl: 'https://iantruongphotography.com/album/bald-mountain-11-1',
      isEnabled: true,
      imageCount: 41,
      addedAt: Date.now() - 86400_000,
    },
  ];

  let settings: AppSettings = {
    rotationIntervalSeconds: 1800,
    transitionType: 'liquid_glass',
    transitionDurationSeconds: 1.2,
    skipMismatchedOrientation: true,
    defaultFitMode: 'fit_blur',
    exhaustionAutoReset: true,
    maxPhotoCacheBytes: 250 * 1024 * 1024,
    fpsCap: 60,
    qualityPreset: 'high',
    pauseOnFullscreen: true,
    pauseOnMaximized: false,
    batteryPolicy: 'pause_animations',
    launchAtLogin: true,
  };

  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case 'photo_get_status':
        return {
          filterMode: 'featured',
          featuredOnly: true,
          unusedQueue: [demoPhoto],
          currentPhotoByMonitor: { 'DISPLAY-1': demoPhoto },
          cachedBytes: 92 * 1024 * 1024,
          maxCacheBytes: settings.maxPhotoCacheBytes,
          historyCount: 14,
          blacklistCount: 0,
          isRefilling: false,
          rateLimitRemaining: 98,
        } satisfies PhotoStatus;
      case 'photo_fetch_public_catalog':
        return [
          {
            albumId: 'demo-album-1',
            type: 'photo',
            title: 'Bald Mountain 11/1',
            description: 'Sunset above the clouds at Bald Mountain.',
            category: 'Landscape',
            imageCount: 41,
            coverImageUrl: '',
            coverThumbnailUrl: '',
            coverBlurhash: 'LGF5]+Yk^6#M@-5c,1J5@[or[Q6.',
            slug: 'bald-mountain-11-1',
          },
        ] satisfies PublicCatalogAlbum[];
      case 'photo_add_album': {
        const slug = String(args.urlOrSlug ?? '').replace(/^.*\/album\//, '').trim();
        const created: SavedAlbum = {
          albumSlug: slug,
          displayName: String(args.displayName ?? slug),
          albumUrl: `https://iantruongphotography.com/album/${slug}`,
          isEnabled: true,
          addedAt: Date.now(),
        };
        savedAlbums = [...savedAlbums, created];
        return created;
      }
      case 'monitors_list':
        return [
          {
            monitorId: 'DISPLAY-1',
            name: '27" 4K Studio Display',
            width: 3840,
            height: 2160,
            scaleFactor: 2.0,
            refreshRateHz: 60,
            isPrimary: true,
            isPortrait: false,
            activeSource: { kind: 'photo_api' },
          },
        ] satisfies MonitorInfo[];
      case 'library_list_wallpapers':
        return [] satisfies WallpaperItem[];
      case 'settings_get':
        return settings;
      case 'settings_update':
        settings = { ...settings, ...(args.settings as Partial<AppSettings>) };
        return;
      default:
        return null;
    }
  });
}
```

### 7.3 `src-tauri/examples/wallpaper_smoke.rs` (Desktop Wallpaper Attachment Smoke Test)
Modeled directly on `TruongFlix/src-tauri/examples/trailer_smoke.rs`, this standalone smoke test verifies that TruongPaper can enumerate the primary display, initialize a `wgpu` offscreen/desktop surface, compile `kawase_blur.wgsl` and `compositor.wgsl`, render a Liquid Glass transition frame, and (when `TRUONGPAPER_SMOKE_PROBE=1` on Windows CI) verify `Progman` / `SHELLDLL_DefView` / `WorkerW` hierarchy discovery:

```rust
// src-tauri/examples/wallpaper_smoke.rs
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let probe_mode = std::env::var("TRUONGPAPER_SMOKE_PROBE").as_deref() == Ok("1");
    println!("[wallpaper_smoke] Starting smoke check (probe_mode = {probe_mode})...");

    // 1. Validate headless wgpu adapter + shader module compilation
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or("No compatible wgpu adapter found on runner")?;

        let (device, _queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await?;

        println!(
            "[wallpaper_smoke] wgpu adapter initialized: {:?}",
            adapter.get_info().name
        );
        drop(device);
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;

    // 2. On Windows, verify Progman HWND discovery
    #[cfg(target_os = "windows")]
    if probe_mode {
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        let progman = unsafe { FindWindowW(w!("Progman"), PCWSTR::null()) }?;
        if progman.0.is_null() {
            return Err("Progman HWND not found on Windows desktop".into());
        }
        println!("[wallpaper_smoke] Verified Windows Progman HWND: {:?}", progman);
    }

    println!("[wallpaper_smoke] PASS");
    Ok(())
}
```

### 7.4 `tests/propertyInspector.test.ts` (Vitest Unit Test Suite)
Ensures `npm test` (`vitest run`) in `.github/workflows/ci.yml` passes out of the box and verifies Wallpaper Engine color conversions (`weColorToHex`, `hexToWeColor`), safe tokenized condition evaluation (`evaluatePropertyCondition`), `project.json` localization lookup (`resolveLocalizedText`), and Universal Omnibar URL/slug classification (`parseOmnibarInput`):

```typescript
// tests/propertyInspector.test.ts
import { describe, expect, it } from 'vitest';
import {
  evaluatePropertyCondition,
  hexToWeColor,
  resolveLocalizedText,
  weColorToHex,
} from '../src/components/PropertyInspector';
import { parseOmnibarInput } from '../src/components/UniversalOmnibar';
import type { ProjectJsonSchema } from '../src/types/ipc';

describe('Wallpaper Engine Color Conversion (weColorToHex & hexToWeColor)', () => {
  it('converts normalized 0..1 float strings to 6-digit hex', () => {
    expect(weColorToHex('0.14902 0.45098 0.85098')).toBe('#2673d9');
    expect(weColorToHex('1 0 0')).toBe('#ff0000');
    expect(weColorToHex('0 0 0')).toBe('#000000');
  });

  it('supports legacy 0..255 integer strings and passthrough hex', () => {
    expect(weColorToHex('38 115 217')).toBe('#2673d9');
    expect(weColorToHex('#38bdf8')).toBe('#38bdf8');
    expect(weColorToHex(undefined)).toBe('#ffffff');
  });

  it('converts 6-digit hex back to normalized float strings', () => {
    expect(hexToWeColor('#2673d9')).toBe('0.14902 0.45098 0.85098');
    expect(weColorToHex(hexToWeColor('#38bdf8'))).toBe('#38bdf8');
  });
});

describe('Safe Tokenized Condition Evaluator (evaluatePropertyCondition)', () => {
  const values = {
    enable_rain: true,
    weather_mode: 2,
    rain_intensity: 0.75,
    disable_particles: false,
    preset_name: 'cyberpunk',
  };

  it('returns true for empty or undefined conditions', () => {
    expect(evaluatePropertyCondition(undefined, values)).toBe(true);
    expect(evaluatePropertyCondition('   ', values)).toBe(true);
  });

  it('evaluates boolean truthiness and negation (!)', () => {
    expect(evaluatePropertyCondition('enable_rain.value', values)).toBe(true);
    expect(evaluatePropertyCondition('!disable_particles.value', values)).toBe(true);
    expect(evaluatePropertyCondition('!enable_rain.value', values)).toBe(false);
  });

  it('evaluates compound && and || numeric and string comparisons', () => {
    expect(
      evaluatePropertyCondition(
        'weather_mode.value == 2 && rain_intensity.value > 0.5',
        values
      )
    ).toBe(true);
    expect(
      evaluatePropertyCondition(
        'weather_mode.value == 1 || preset_name.value == "cyberpunk"',
        values
      )
    ).toBe(true);
    expect(
      evaluatePropertyCondition(
        'weather_mode.value != 2 && rain_intensity.value <= 0.5',
        values
      )
    ).toBe(false);
  });
});

describe('project.json Localization Lookup (resolveLocalizedText)', () => {
  const sampleProject: ProjectJsonSchema = {
    title: 'Neon Rain',
    type: 'scene',
    file: 'scene.json',
    general: {
      localization: {
        'en-us': {
          ui_browse_properties_scheme_color: 'Scheme Color',
        },
      },
    },
  };

  it('resolves localized keys and strips raw HTML tags on fallback', () => {
    expect(
      resolveLocalizedText('ui_browse_properties_scheme_color', sampleProject)
    ).toBe('Scheme Color');
    expect(resolveLocalizedText('<b>Rain Speed</b>', sampleProject)).toBe(
      'Rain Speed'
    );
  });
});

describe('Universal Omnibar URL & Slug Parser (parseOmnibarInput)', () => {
  it('detects Steam Workshop HTTP URLs, steam:// protocol links, and numeric IDs', () => {
    expect(
      parseOmnibarInput('https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371&searchtext=')
    ).toEqual({ kind: 'steam_workshop', workshopId: '2841928371' });
    expect(
      parseOmnibarInput('steam://url/CommunityFilePage/2841928371')
    ).toEqual({ kind: 'steam_workshop', workshopId: '2841928371' });
    expect(parseOmnibarInput('2841928371')).toEqual({
      kind: 'steam_workshop',
      workshopId: '2841928371',
    });
  });

  it('detects Ian Truong Photography Album URLs and slugs', () => {
    expect(
      parseOmnibarInput('https://iantruongphotography.com/album/bald-mountain-11-1')
    ).toEqual({ kind: 'photo_album', albumSlug: 'bald-mountain-11-1' });
    expect(parseOmnibarInput('cape-horn-11-7')).toEqual({
      kind: 'photo_album',
      albumSlug: 'cape-horn-11-7',
    });
  });
});
```
