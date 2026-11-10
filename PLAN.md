# TruongPaper — Master Architecture & Implementation Blueprint

> **Status**: Ready for Implementation (Zero Ambiguities — Copy-Paste-Ready Specifications)  
> **Reference Repo Standard**: [`https://github.com/iant4093/TruongFlix`](https://github.com/iant4093/TruongFlix) (Identical repository layout, `README.md` structure, sidecar bundler pattern, GitHub Actions CI/Release workflow, and signed auto-updater)  
> **Target Platforms**: macOS (`14.2+` Apple Silicon & Intel) & Windows (`10 1809+`, `11 23H2`, and `11 24H2` `x64` & `ARM64`)  
> **Core Stack**: Rust (`Tauri v2.2`, `wgpu 23`, `naga 23`, `tokio`, `rusqlite`, `rustfft`, `rquickjs`, `DepotDownloader v3.4.0` sidecar) + TypeScript / React 19 (Custom Cross-Platform "Apple Liquid Glass" Design System)

---

## 1. Executive Summary

**TruongPaper** is a high-performance, lightweight, cross-platform desktop wallpaper application for **macOS** and **Windows** featuring an identical, modern **Apple Liquid Glass** user interface across both operating systems. It unifies two core engines into one cohesive experience:

1. **Ian Truong Photography API Engine**: Connects directly to the public random endpoint (`GET https://photos-api.iantruongphotography.com/v1/photos/random`) and official album catalog endpoint (`GET https://iantruongphotography.com/api/public/albums?type=photo&limit=100`) with an offline-first local queue of 5 unused photos, persistent SQLite history to guarantee zero unwanted repeats even after disk cache eviction, multi-album filtering, small-album deadlock protection, aspect-ratio/orientation intelligence, and GPU-accelerated wallpaper transitions.
2. **Full Wallpaper Engine Compatibility & Steam Workshop Downloader**: Paste any Steam Workshop link (or auto-sync with a local Steam installation) to automatically download, unpack, render, and customize any Wallpaper Engine wallpaper (`Scene`, `Video`, `Web`, and Windows `Application` types) with full system audio reactivity, desktop cursor interaction, custom HLSL/GLSL shaders, particle systems, `.mdl` puppet warp, SceneScript execution, and auto-generated user property controls.

---

## 2. Locked-In User Decisions & Product Requirements

| Area | Finalized Decision |
| :--- | :--- |
| **Steam Workshop Downloading** | **Dual Support**: (1) **Built-in standalone Steam downloader** via bundled `DepotDownloader` v3.4.0 native sidecar (`macos-arm64`, `macos-x64`, `windows-x64`, `windows-arm64`) with QR Code & 2FA login, single-vault OS Keychain persistence, and automatic first-run bootstrap of Wallpaper Engine core `assets/` (`Depot 431961`) **AND** (2) **Local Steam folder auto-sync** (`steamapps/workshop/content/431960`). |
| **Wallpaper Engine Types** | **All 4 Types Supported**: <br>1. `scene` (4-layer VFS, `.pkg` unpacker, `.tex` decoder, custom shaders/effects, particles, `.mdl` puppet warp, SceneScript, audio & cursor reactivity)<br>2. `video` (hardware-accelerated looping MP4/WebM/MKV/MOV + `.tex` `FIF_MP4` video textures with volume & playback speed)<br>3. `web` (HTML/Canvas/WebGL with `wallpaperPropertyListener` & `wallpaperRegisterAudioListener` 128-float stereo FFT)<br>4. `application` (Windows application wallpapers launched with desktop bounds; flagged with a `Windows Only` badge on macOS). |
| **Photo Orientation & Aspect Ratios** | **Smart Orientation Matching**: By default, skip portrait photos (`height > width`) on landscape monitors (and vice-versa) during queue selection, with a per-monitor option to allow mismatched orientations using a **GPU Kawase-blurred ambient backdrop (`Fit + Blur`)**, `Fill (Center Crop)`, `Fit (Black Bars)`, or `Stretch`. |
| **Photo API + Workshop Coexistence** | **Unified Playlist & Per-Monitor Assignment**: Each monitor can independently run Photo API rotation, a single Workshop wallpaper, or a **Unified Playlist** that rotates only Photos, only Workshop wallpapers, or mixes both on a custom schedule. |
| **GPU Wallpaper Transitions** | Custom `wgpu` desktop transition shaders: **Smooth Crossfade / Dissolve** ($0.3\text{s}$–$3.0\text{s}$), **Blur-Dissolve** (dual-pass Kawase blur out/in), and **Liquid Glass Morph / Chromatic Refraction** shader. |
| **Photo Queue & History Exhaustion** | Maintain **5 unused photos** on disk (`wallpapers/photos/<id>.jpg`), refill in background when queue drops to **2**, cap retry attempts at **12** per refill, and exit cleanly after **4** consecutive `AlreadyInUnusedQueue` hits for small albums ($< 5$ photos). When all photos in a selected album/filter have been shown, **automatically reset the shown history for that specific album/filter** and loop seamlessly (with a toggle to hold on the last photo instead). |
| **Photo API UX & Cache Controls** | Browse official public albums (`GET /api/public/albums?type=photo&limit=100`) or paste full album URLs (`https://iantruongphotography.com/album/...`) / slugs, save favorite albums, enable **multiple saved albums at once**, and per-photo actions: **Favorite/Keep on disk**, **Hide/Never show again**, and **View Album on Website** (`albumUrl`). Configurable cache size limit (**250 MB default**, slider **100 MB – 10 GB**) + manual **Clear Cache** (never deletes `photos_history`) and **Reset History** buttons. |
| **Wallpaper Engine Controls** | Auto-generate Liquid Glass UI controls for all `project.json` user properties (sliders, toggles, color pickers, dropdowns, file/image overrides) with safe `condition` evaluation and localization lookup, global/per-wallpaper adjustments (Volume, Playback Speed, Alignment/Zoom, Color Grading: Brightness, Contrast, Saturation, Hue), Audio input device selector + sensitivity/gain, and **named configuration presets** + playlist import/export. |
| **Performance & Smart Pause** | Pause/mute when a fullscreen app or game is active on that monitor; pause/mute when any window is maximized covering the desktop; pause on OS sleep, display sleep, or screen lock (`power.rs`); pause animations or switch to static photo on laptop battery / Low Power Mode; configurable FPS cap (`30` / `60` / `120` / `Match Display Refresh Rate`) and quality presets (`Low`, `Medium`, `High`, `Ultra`). |
| **UI Layout & Visual Style** | **Apple Liquid Glass UI** (`LiquidGlassFilters.tsx` + `AmbientBacklightCanvas.tsx` + `liquid-glass.css`) rendered identically on macOS and Windows. Includes **both** a full **Library & Studio Window** and a lightweight **Menu Bar (macOS) / System Tray (Windows) Quick-Control Popover**. |
| **Repository, `README.md` & Releases** | **100% Aligned with [`TruongFlix`](https://github.com/iant4093/TruongFlix)**: Root React/Vite/Vitest structure + `src-tauri/` Rust workspace, `scripts/bundle-depotdownloader.mjs`, `scripts/release.mjs`, `.github/workflows/ci.yml` publishing `TruongPaper-macOS.dmg`, `TruongPaper-Windows-Setup.exe`, `TruongPaper-macOS.app.tar.gz`, `latest.json`, and `SHA256SUMS`, plus in-app signed auto-updates (`src/services/updates.ts`). |

---

## 3. Specification Map (Read All Six Documents Before Writing Code)

Every subsystem has been researched, tested against live endpoints and reference source code, and specified down to copy-pasteable Rust, WGSL, SQL, TypeScript, CSS, and YAML code inside [`docs/`](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs):

1. [**`docs/01-SYSTEM-ARCHITECTURE.md`**](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs/01-SYSTEM-ARCHITECTURE.md)
   - Complete Rust desktop attachment code for **Windows 10/11 23H2** AND **Windows 11 24H2 (`WS_EX_NOREDIRECTIONBITMAP`)**, **macOS `NSWindow` (`kCGDesktopWindowLevel`)**, complete **WGSL shaders** (`kawase_blur.wgsl`, `compositor.wgsl` with all 4 fit modes, 3 transitions, and color grading), **2048-point Hann-windowed `rustfft` audio spectrum analyzer + WASAPI silence watchdog**, passive cursor hit-testing, Smart Pause occlusion monitor, native OS sleep/lock watcher (`power.rs`), and single-item OS Keychain vault (`secrets.rs`).
2. [**`docs/02-PHOTO-API-AND-CACHE-SPEC.md`**](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs/02-PHOTO-API-AND-CACHE-SPEC.md)
   - Verified HTTP/2 API wire formats (`200 OK`, `400`, `404`), official Public Album Catalog endpoint (`GET /api/public/albums`), complete **9-table SQLite schema (`truongpaper.db`)**, and complete copy-pasteable Rust implementation for the 100 req / 5 min sliding-window rate limiter, zero-API-call queue rotation, small-album-safe background refill, exhaustion auto-reset, and history-preserving LRU disk eviction.
3. [**`docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md`**](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md)
   - Complete **`DepotDownloader` v3.4.0 Tauri sidecar** integration (including ASCII QR code matrix parser and non-newline `stderr` 2FA prompt handler), **4-Layer VFS** + `Depot 431961` core `assets/` bootstrap, byte-accurate Rust parsers for **`.pkg`** (`read_string_i32_size`), **`.tex`** (`read_n_string`, `TEXV0005`, `TEXI0001`, `TEXB0001..0004` including the `FIF_MP4` `Version4` downgrade rule, `TEXS0001..0003`), and **`.mdl`** (80-byte skeletal vertex layout), complete **Shader Preprocessor**, **`17-float` / `26-float` particle layouts**, **`scene.json` `UserBoundValue<T>` deserializer**, **`rquickjs` SceneScript** bridge, **`web-wallpaper-preload.js`**, and Windows `Application` runtime.
4. [**`docs/04-UI-UX-LIQUID-GLASS-SPEC.md`**](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs/04-UI-UX-LIQUID-GLASS-SPEC.md)
   - Complete **`LiquidGlassFilters.tsx`** SVG refraction filter, **`AmbientBacklightCanvas.tsx`** 30 FPS ambient backlight, **`liquid-glass.css`**, **`PropertyInspector.tsx`** (including the safe `evaluatePropertyCondition` parser, `weColorToHex` / `hexToWeColor` converter, and localization resolver), complete **`src/types/ipc.ts`**, **`src-tauri/tauri.conf.json`**, and **`src-tauri/capabilities/default.json`**.
5. [**`docs/05-IMPLEMENTATION-ROADMAP.md`**](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs/05-IMPLEMENTATION-ROADMAP.md)
   - Ordered **7-Phase Implementation Plan** with per-phase verification commands, exact **`src-tauri/Cargo.toml`**, **`package.json`**, and **`vite.config.ts`** manifests, the **Top 15 Engineering Pitfalls & Do-Not-Do Table**, and the final Acceptance Criteria Checklist.
6. [**`docs/06-REPO-STRUCTURE-README-AND-RELEASES-SPEC.md`**](file:///Users/iantruong/Desktop/Fun/TruongPaper/docs/06-REPO-STRUCTURE-README-AND-RELEASES-SPEC.md)
   - Complete **`TruongFlix`-aligned Repository Layout**, copy-paste-ready **`README.md`**, **`scripts/bundle-depotdownloader.mjs`**, **`scripts/release.mjs`**, **`.github/workflows/ci.yml`**, **`.github/dependabot.yml`**, **`src/services/updates.ts`**, **`src-tauri/build.rs`**, **`src-tauri/windows-app-manifest.xml`**, and **`src-tauri/Entitlements.plist`**.

---

## 4. Target Repository Structure (Aligned with `TruongFlix`)

```text
.
├── PLAN.md                         Master architecture blueprint (this file)
├── README.md                       TruongFlix-style README with badges, screenshots, install & CI docs
├── package.json                    Frontend & release scripts (dev, app:dev, app:build, test, bundle:depotdownloader)
├── vite.config.ts                  Multi-window Vite config (index.html + tray.html)
├── vitest.config.ts                Vitest unit & component test config
├── index.html                      Main Studio window entry
├── tray.html                       Menu Bar / System Tray popover entry
├── src/                            React 19 + TypeScript application
│   ├── components/                 LiquidGlassFilters, AmbientBacklightCanvas, PropertyInspector, views, modals
│   ├── services/                   Photo API, Steam Workshop, playlists, monitors, presets, updates.ts
│   ├── store/                      Zustand store (useWallpaperStore.ts) and selectors
│   ├── tray/                       Menu Bar (macOS) / System Tray (Windows) quick-control popover
│   ├── types/                      Complete IPC contract (ipc.ts)
│   └── dev/                        Browser-preview mock of the Tauri backend (mockTauri.ts)
├── src-tauri/                      Rust backend & Cargo workspace
│   ├── Cargo.toml                  Workspace manifest + release profile overrides
│   ├── build.rs                    Embeds windows-app-manifest.xml so `cargo test` works on Windows
│   ├── windows-app-manifest.xml    Common Controls v6 manifest
│   ├── Entitlements.plist          macOS ad-hoc signing entitlements
│   ├── tauri.conf.json             Tauri v2 dual-window, CSP, sidecar & updater config
│   ├── capabilities/default.json   Permissions for "main" and "tray-popover" windows
│   ├── bin/                        Bundled DepotDownloader sidecar & licenses (populated by script)
│   ├── src/                        main.rs, lib.rs, power.rs, secrets.rs, window.rs
│   └── crates/
│       ├── truongpaper-core/
│       ├── truongpaper-photo-api/
│       ├── truongpaper-steam/
│       ├── truongpaper-we-formats/
│       ├── truongpaper-renderer/
│       ├── truongpaper-scenescript/
│       ├── truongpaper-audio/
│       └── truongpaper-platform/
├── tests/                          Vitest frontend unit & component tests
├── scripts/
│   ├── bundle-depotdownloader.mjs  Downloads & SHA-256 verifies DepotDownloader v3.4.0 sidecar
│   └── release.mjs                 One-command local signed build + latest.json generator
├── docs/                           Architecture specs (01..06), app-icon.png & screenshots/*.webp
└── .github/
    ├── workflows/ci.yml            Automated test, matrix build, smoke test & GitHub Release publisher
    └── dependabot.yml              Weekly SHA-pinned GitHub Actions updates
```
