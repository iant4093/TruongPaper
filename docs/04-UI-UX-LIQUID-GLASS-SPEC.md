# 04 — Apple Liquid Glass UI/UX & Tauri IPC Specification

> **2026-11-10 user override:** The original Liquid Glass appearance is superseded by a simple, neutral dark desktop theme inspired by Codex. Use `src/styles/theme.css`, a flat sidebar, compact toolbar/playback controls, and a contextual inspector. Existing IPC contracts and view functionality below remain authoritative.

## 1. Cross-Platform "Apple Liquid Glass" Design System

To guarantee that TruongPaper looks **100% identical on macOS and Windows** (rather than relying on OS-divergent Windows 11 Mica/Acrylic vs. macOS `NSVisualEffectView`), TruongPaper renders its own GPU-accelerated **Liquid Glass** surface hierarchy inside a frameless transparent Tauri v2 window (`decorations: false`, `transparent: true`).

---

### 1.1 Complete SVG Refraction Filter (`LiquidGlassFilters.tsx`)

Include this hidden SVG filter definition once at the root of both `index.html` (Main Studio Window) and `tray.html` (Menu Bar / System Tray Popover). It combines dual-frequency Fractal Perlin noise (`feTurbulence`), edge-weighted lens refraction (`feDisplacementMap`), and chromatic specular rim lighting (`feSpecularLighting`) referenced by `backdrop-filter: url(#liquid-refraction)`:

```tsx
// src/components/LiquidGlassFilters.tsx
import React from 'react';

export const LiquidGlassFilters: React.FC = () => (
  <svg
    aria-hidden="true"
    style={{ position: 'absolute', width: 0, height: 0, pointerEvents: 'none' }}
  >
    <defs>
      <filter
        id="liquid-refraction"
        x="-10%"
        y="-10%"
        width="120%"
        height="120%"
        colorInterpolationFilters="sRGB"
      >
        {/* Subtle organic glass surface curvature noise */}
        <feTurbulence
          type="fractalNoise"
          baseFrequency="0.012 0.018"
          numOctaves={2}
          seed={42}
          result="noise"
        />
        {/* Smooth the noise field so refraction bends cleanly like polished optical glass */}
        <feGaussianBlur in="noise" stdDeviation="3.5" result="smoothNoise" />
        {/* Refract the backdrop graphic using the smoothed normal field */}
        <feDisplacementMap
          in="SourceGraphic"
          in2="smoothNoise"
          scale={11}
          xChannelSelector="R"
          yChannelSelector="G"
          result="refracted"
        />
        {/* Specular highlight layer for liquid glass sheen */}
        <feSpecularLighting
          in="smoothNoise"
          surfaceScale={2.2}
          specularConstant={0.55}
          specularExponent={28}
          lightingColor="#ffffff"
          result="specular"
        >
          <fePointLight x={-200} y={-300} z={260} />
        </feSpecularLighting>
        <feComposite
          in="specular"
          in2="SourceAlpha"
          operator="in"
          result="clippedSpecular"
        />
        <feBlend
          mode="screen"
          in="clippedSpecular"
          in2="refracted"
          result="finalGlass"
        />
      </filter>
    </defs>
  </svg>
);
```

---

### 1.2 Ambient Wallpaper Backlight Canvas (`AmbientBacklightCanvas.tsx`)

Because desktop translucency APIs differ wildly between Windows 10, Windows 11, and macOS, the bottom-most layer of the TruongPaper UI window renders a deeply blurred ($\sigma = 64\text{px}$), slowly breathing 30 FPS ambient canvas sampled from the active desktop wallpaper's thumbnail. Every floating Liquid Glass sidebar, card, modal, and popover refracts this unified internal ambient layer via `backdrop-filter`, guaranteeing identical glass depth, color bleed, and luminance on both operating systems:

```tsx
// src/components/AmbientBacklightCanvas.tsx
import React, { useEffect, useRef } from 'react';

interface AmbientBacklightProps {
  activeThumbnailUrl: string | null;
}

export const AmbientBacklightCanvas: React.FC<AmbientBacklightProps> = ({
  activeThumbnailUrl,
}) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const img = new Image();
    img.crossOrigin = 'anonymous';
    if (activeThumbnailUrl) {
      img.src = activeThumbnailUrl;
    }

    let animId = 0;
    let phase = 0;

    const render = () => {
      phase += 0.004;
      const w = canvas.width;
      const h = canvas.height;
      ctx.clearRect(0, 0, w, h);

      // Base dark obsidian tint
      ctx.fillStyle = '#090b12';
      ctx.fillRect(0, 0, w, h);

      if (img.complete && img.naturalWidth > 0) {
        ctx.save();
        ctx.globalAlpha = 0.48;
        ctx.filter = 'blur(56px) saturate(185%)';
        const offsetX = Math.sin(phase) * 28;
        const offsetY = Math.cos(phase * 0.8) * 22;
        ctx.drawImage(img, -40 + offsetX, -40 + offsetY, w + 80, h + 80);
        ctx.restore();
      } else {
        // Fallback aurora gradient when no thumbnail is loaded yet
        const grad = ctx.createRadialGradient(
          w * (0.5 + 0.2 * Math.sin(phase)),
          h * (0.4 + 0.15 * Math.cos(phase)),
          20,
          w * 0.5,
          h * 0.5,
          w * 0.8
        );
        grad.addColorStop(0, 'rgba(56, 128, 255, 0.32)');
        grad.addColorStop(0.5, 'rgba(132, 68, 255, 0.18)');
        grad.addColorStop(1, 'rgba(9, 11, 18, 0.95)');
        ctx.fillStyle = grad;
        ctx.fillRect(0, 0, w, h);
      }

      animId = window.setTimeout(() => requestAnimationFrame(render), 33);
    };

    render();
    return () => clearTimeout(animId);
  }, [activeThumbnailUrl]);

  return (
    <canvas
      ref={canvasRef}
      width={320}
      height={200}
      className="ambient-backlight-canvas"
    />
  );
};
```

---

### 1.3 Complete `liquid-glass.css` Stylesheet

```css
/* src/styles/liquid-glass.css */
:root {
  --font-sans: 'Inter Variable', -apple-system, BlinkMacSystemFont, 'SF Pro Display', sans-serif;
  --font-mono: 'JetBrains Mono Variable', ui-monospace, SFMono-Regular, monospace;

  --glass-bg-primary: linear-gradient(
    135deg,
    rgba(255, 255, 255, 0.16) 0%,
    rgba(255, 255, 255, 0.06) 50%,
    rgba(255, 255, 255, 0.11) 100%
  );
  --glass-bg-elevated: linear-gradient(
    135deg,
    rgba(255, 255, 255, 0.22) 0%,
    rgba(255, 255, 255, 0.09) 60%,
    rgba(255, 255, 255, 0.15) 100%
  );
  --glass-border-rim:
    0 20px 50px rgba(0, 0, 0, 0.42),
    inset 0 1px 0 0 rgba(255, 255, 255, 0.44),
    inset 0 -1px 0 0 rgba(255, 255, 255, 0.10),
    inset 1px 0 0 0 rgba(255, 255, 255, 0.18),
    inset -1px 0 0 0 rgba(255, 255, 255, 0.18);

  --text-primary: rgba(255, 255, 255, 0.96);
  --text-secondary: rgba(255, 255, 255, 0.68);
  --text-muted: rgba(255, 255, 255, 0.44);
  --accent-blue: #3b82f6;
  --accent-cyan: #38bdf8;
  --accent-glow: rgba(56, 189, 248, 0.38);
}

* {
  box-sizing: border-box;
  user-select: none;
  -webkit-user-select: none;
}

html, body, #root {
  margin: 0;
  padding: 0;
  width: 100%;
  height: 100%;
  background: transparent;
  font-family: var(--font-sans);
  color: var(--text-primary);
  overflow: hidden;
  -webkit-font-smoothing: antialiased;
}

.app-window-frame {
  position: relative;
  width: 100vw;
  height: 100vh;
  border-radius: 24px;
  overflow: hidden;
  background: rgba(10, 12, 20, 0.72);
  box-shadow:
    0 32px 80px rgba(0, 0, 0, 0.65),
    inset 0 1px 0 0 rgba(255, 255, 255, 0.35),
    inset 0 0 0 1px rgba(255, 255, 255, 0.14);
}

.ambient-backlight-canvas {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
  pointer-events: none;
  z-index: 0;
}

/* Primary Liquid Glass Surface */
.glass-panel {
  position: relative;
  background: var(--glass-bg-primary);
  backdrop-filter: url(#liquid-refraction) blur(28px) saturate(190%) brightness(1.06);
  -webkit-backdrop-filter: url(#liquid-refraction) blur(28px) saturate(190%) brightness(1.06);
  border-radius: 22px;
  box-shadow: var(--glass-border-rim);
  transition: transform 0.22s cubic-bezier(0.22, 1, 0.36, 1),
              box-shadow 0.22s cubic-bezier(0.22, 1, 0.36, 1),
              background 0.22s ease;
}

/* Interactive Specular Pointer Spotlight on Cards & Buttons */
.glass-card {
  position: relative;
  background: var(--glass-bg-primary);
  backdrop-filter: blur(24px) saturate(180%);
  -webkit-backdrop-filter: blur(24px) saturate(180%);
  border-radius: 18px;
  box-shadow: var(--glass-border-rim);
  overflow: hidden;
  transition: transform 0.2s cubic-bezier(0.22, 1, 0.36, 1),
              box-shadow 0.2s ease;
}

.glass-card::before {
  content: '';
  position: absolute;
  inset: 0;
  background: radial-gradient(
    420px circle at var(--mouse-x, 50%) var(--mouse-y, 0%),
    rgba(255, 255, 255, 0.22),
    transparent 45%
  );
  opacity: 0;
  transition: opacity 0.25s ease;
  pointer-events: none;
}

.glass-card:hover {
  transform: translateY(-2px);
}

.glass-card:hover::before {
  opacity: 1;
}

/* Liquid Glass Pill Button */
.glass-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 8px 16px;
  border-radius: 9999px;
  border: none;
  background: var(--glass-bg-elevated);
  color: var(--text-primary);
  font-family: var(--font-sans);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  box-shadow:
    0 6px 18px rgba(0, 0, 0, 0.24),
    inset 0 1px 0 0 rgba(255, 255, 255, 0.45),
    inset 0 0 0 1px rgba(255, 255, 255, 0.16);
  transition: all 0.18s cubic-bezier(0.22, 1, 0.36, 1);
}

.glass-button:hover {
  background: linear-gradient(
    135deg,
    rgba(255, 255, 255, 0.30) 0%,
    rgba(255, 255, 255, 0.14) 100%
  );
  transform: translateY(-1px);
}

.glass-button:active {
  transform: translateY(0) scale(0.97);
}

.glass-button-primary {
  background: linear-gradient(135deg, rgba(59, 130, 246, 0.78), rgba(37, 99, 235, 0.62));
  box-shadow:
    0 8px 24px rgba(37, 99, 235, 0.42),
    inset 0 1px 0 0 rgba(255, 255, 255, 0.52),
    inset 0 0 0 1px rgba(147, 197, 253, 0.35);
}

/* Liquid Glass Slider */
.glass-slider {
  -webkit-appearance: none;
  appearance: none;
  width: 100%;
  height: 6px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.16);
  box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.45);
  outline: none;
}

.glass-slider::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: radial-gradient(circle at 35% 30%, #ffffff, #dbeafe 70%, #93c5fd 100%);
  box-shadow:
    0 2px 8px rgba(0, 0, 0, 0.45),
    0 0 0 1px rgba(255, 255, 255, 0.8);
  cursor: pointer;
  transition: transform 0.15s ease;
}

.glass-slider::-webkit-slider-thumb:hover {
  transform: scale(1.14);
}
```

---

## 2. Main Application Window Architecture (`1280px × 820px` Default, Resizable)

```text
+--------------------------------------------------------------------------------------------------+
| [ o o o ]   [  Paste Steam Workshop URL or Ian's Photo Album URL / Slug...        ] [ + Import ] |
|             [  Monitor 1: 27" 4K (Active)  |  Monitor 2: 24" Vertical  |  Sync All  ]  [ Next > ]|
+----------------------+--------------------------------------------------+------------------------+
|                      |                                                  |                        |
|  LIQUID GLASS NAV    |  MAIN CONTENT VIEWPORT                           |  LIVE INSPECTOR        |
|                      |                                                  |  DRAWER                |
|  * All Wallpapers    |  +--------------+ +--------------+ +-----------+ |                        |
|  * Ian's Photos API  |  | [Preview]    | | [Preview]    | | [Preview] | |  [ Animated Preview ]  |
|  * Steam Workshop    |  |              | |              | |           | |                        |
|  * Unified Playlists |  | Bald Mtn #4  | | Cyberpunk 2D | | Audio Orb | |  Title & Badges        |
|  * Monitors & Fit    |  | Photo • 4K   | | Scene • Audio| | Web • 60Hz| |  [Preset: Default v]   |
|  * Settings          |  +--------------+ +--------------+ +-----------+ |                        |
|                      |                                                  |  -- User Properties -- |
|  ------------------  |  -- Active Queue (5/5 Ready Offline) ----------  |  Rain Speed  [====o--] |
|  Photo Queue: 5/5    |  [Thumb 1] [Thumb 2] [Thumb 3] [Thumb 4] [Thumb5]|  Scheme Color [ #2673D9]|
|  Cache: 92 / 250 MB  |                                                  |  Show Clock   [  ON  ] |
|  Steam: Connected    |                                                  |                        |
|  Audio FFT: Active   |                                                  |  -- Color & Audio ---- |
|                      |                                                  |  Volume      [===o---] |
+----------------------+--------------------------------------------------+------------------------+
```

### 2.1 Universal Omnibar (Paste Anywhere)
Located in the top glass header (and also triggered via `Cmd+V` / `Ctrl+V` anywhere in the window when no text input is focused):
- **Paste a Steam Workshop URL or ID** (`https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371` or numeric ID):
  - Calls `invoke<WorkshopPreview>('steam_inspect_url', { urlOrId })`.
  - Opens a floating Liquid Glass **Workshop Preview & Download Modal** showing thumbnail, title, author, type badge (`Scene` / `Video` / `Web` / `Application`), file size, and live download progress bar (`Downloading chunks... 42% (68 MB / 160 MB)`), then automatically applies or adds it to the library upon completion.
  - If the user has not logged into Steam yet, pops up the **Steam Sign-In Sheet** (QR Code tab rendering the `Vec<Vec<bool>>` matrix from `DepotDownloader -qr` + Username/Password/2FA tab).
- **Paste an Ian Truong Photography Album URL or Slug** (`https://iantruongphotography.com/album/bald-mountain-11-1` or `bald-mountain-11-1`):
  - Calls `invoke<SavedAlbum>('photo_add_album', { urlOrSlug })`, which validates the slug against `GET /v1/photos/random?album=bald-mountain-11-1`, saves it to **Saved Albums**, shows a sample thumbnail, and offers *"Activate Album & Refill Queue"*.

### 2.2 View 1: `Ian's Photos API` Studio View
- **Filter Controls Card**:
  - Pool mode selector: `All Public Photos` | `Featured Photos Only` | `Selected Saved Albums` (with optional `Featured Only` sub-toggle).
  - **Browse Official Album Catalog Drawer**: Calls `invoke<PublicCatalogAlbum[]>('photo_fetch_public_catalog')` (`GET https://iantruongphotography.com/api/public/albums?type=photo&limit=100`) to display visual cards with `title`, `category` (`Landscape`, `Street`, `Sports`, `Astro`), `imageCount`, `coverThumbnailUrl`, and one-click `"Save & Enable Album"`.
  - **Saved Albums Grid**: Interactive glass cards for each saved album slug with checkboxes to enable **multiple albums simultaneously**, quick `"Open on Website"` external link button, and `"Add Album URL/Slug"` input.
- **Live Queue & History Inspector**:
  - **Up Next Queue (`5 Unused Photos on Disk`)**: Shows the 5 pre-downloaded wallpapers ready for offline zero-API-call rotation, with dimensions (`6000 × 4000`), file size (`18.4 MB`), album badge, and per-item `Apply Now` or `Skip/Blacklist` actions.
  - **Currently Displayed Photo Card**: Large preview with quick actions:
    - `Favorite / Keep on Disk` (heart icon — sets `is_favorite = 1`, exempting it from disk cache eviction)
    - `Hide / Never Show Again` (eye-off icon — adds `id` to `photos_blacklist` and advances queue)
    - `View Album on iantruongphotography.com` (opens `albumUrl` in default browser)
  - **Storage & History Bar**:
    - Visual progress bar of current cache usage (`92 MB / 250 MB`).
    - **Cache Size Limit Slider**: `100 MB` to `10 GB` (default `250 MB`).
    - **Exhaustion Behavior Toggle**: `Auto-reset album/filter history when all photos have been shown (Recommended)` vs. `Hold on last photo`.
    - Action buttons: `Refill Queue Now`, `Clear Image Cache (Keep History)`, `Reset Shown History`.

### 2.3 View 2: `Steam Workshop` Library View
- Filter pills: `All`, `Scene`, `Video`, `Web`, `Application`, `Audio Reactive`, `Local Steam Synced`, `Downloaded`.
- Search bar (filters by title, author, tags, workshop ID).
- **Steam Connection Pill**: Shows `Steam: Connected (@username)` + `Local Steam Library: Synced` with a button to manage Steam account or trigger manual sync.
- **Wallpaper Grid Cards**:
  - Animated WebP/GIF/video hover preview (`preview.jpg` / `preview.gif`).
  - Type badge (`SCENE`, `VIDEO`, `WEB`, or `EXE (Windows Only)`).
  - Audio reactivity icon if `supportsaudioprocessing` or `wallpaperRegisterAudioListener` is detected.

### 2.4 View 3: `Unified Playlists & Rotation` View
- **Mode Selector (Per Monitor or Global)**:
  1. `Ian's Photo API Only` (rotates photos from the active Photo API filter queue)
  2. `Single Wallpaper Engine Item` (runs one static/dynamic Workshop wallpaper continuously)
  3. `Unified Mixed Playlist` (interleaves Photo API draws with selected Workshop wallpapers).
- **Rotation Interval & Trigger Controls**:
  - Timer intervals: `1 min`, `5 min`, `15 min`, `30 min`, `1 hour`, `4 hours`, `12 hours`, `24 hours`, `Never (Manual Only)`.
  - Additional triggers: `Rotate on App Launch`, `Rotate on Display Wake / Unlock`, and customizable Global Hotkey (`Cmd+Shift+N` / `Ctrl+Shift+N` for Next Wallpaper).
- **GPU Transition Selector**:
  - Transition Type: `Smooth Crossfade`, `Blur-Dissolve`, `Liquid Glass Morph`.
  - Transition Duration Slider: `0.3s` to `3.0s` (default `1.2s`).
- **Import / Export Playlist**: Export/import `.json` playlist configurations (including Workshop IDs, saved presets, and Photo API album slugs).

### 2.5 View 4: `Monitors & Fit` View
- Interactive visual diagram of all connected displays showing their exact resolution, DPI scale, refresh rate, and orientation (`Landscape` vs. `Portrait`).
- Per-monitor settings:
  - **Orientation Matching for Photo API**: `[x] Skip portrait photos on landscape monitors (and vice-versa)` (Default ON) + fallback fit mode (`Fit + Blurred Backdrop`, `Fill / Center Crop`, `Fit + Black Bars`, `Stretch`).
  - **Multi-Monitor Mode**: `Independent per monitor` vs. `Clone same wallpaper across all monitors` vs. `Span single wallpaper across all monitors`.

---

## 3. Complete Reference Implementation: Dynamic `project.json` Property Inspector (`PropertyInspector.tsx`)

Wallpaper Engine `project.json` files define arbitrary user properties under `general.properties`, along with localization dictionaries under `general.localization` and visibility expressions under `prop.condition` (e.g., `"user_enable_rain.value == true && user_weather_mode.value == 2"`).

Below is the complete, copy-paste-ready TypeScript implementation including:
1. **Safe Tokenized Condition Evaluator (`evaluatePropertyCondition`)** — evaluates `project.json` visibility conditions **without** `eval()` or `new Function()`.
2. **Normalized Float Color Converter (`weColorToHex` / `hexToWeColor`)** — converts Wallpaper Engine's `"0.149 0.451 0.851"` float strings to/from `#RRGGBB`.
3. **Localization Resolver (`resolveLocalizedText`)** — resolves `ui_browse_properties_*` keys against `general.localization`.

```tsx
// src/components/PropertyInspector.tsx
import React, { useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type {
  ProjectJsonSchema,
  ProjectJsonProperty,
  UniversalAdjustments,
  WallpaperPreset,
} from '../types/ipc';

/**
 * Converts Wallpaper Engine normalized float color string ("0.15 0.45 0.85")
 * into standard 6-digit hex ("#2673d9").
 */
export function weColorToHex(weColor: string | undefined): string {
  if (!weColor) return '#ffffff';
  const trimmed = weColor.trim();
  if (trimmed.startsWith('#')) return trimmed;
  const parts = trimmed.split(/\s+/).map(Number);
  if (parts.length < 3 || parts.some(isNaN)) return '#ffffff';
  const toByte = (n: number) => {
    // Some older wallpapers store 0..255 integers instead of 0..1 floats
    const normalized = parts.some((p) => p > 1.0) ? n / 255.0 : n;
    return Math.max(0, Math.min(255, Math.round(normalized * 255)))
      .toString(16)
      .padStart(2, '0');
  };
  return `#${toByte(parts[0])}${toByte(parts[1])}${toByte(parts[2])}`;
}

/**
 * Converts "#2673d9" back to Wallpaper Engine's "0.14902 0.45098 0.85098" string.
 */
export function hexToWeColor(hex: string): string {
  const clean = hex.replace('#', '');
  if (clean.length !== 6) return '1.00000 1.00000 1.00000';
  const r = (parseInt(clean.slice(0, 2), 16) / 255).toFixed(5);
  const g = (parseInt(clean.slice(2, 4), 16) / 255).toFixed(5);
  const b = (parseInt(clean.slice(4, 6), 16) / 255).toFixed(5);
  return `${r} ${g} ${b}`;
}

/**
 * Safely evaluates Wallpaper Engine property visibility conditions such as:
 *   "schemecolor.value == true"
 *   "weather_mode.value == 2 && rain_intensity.value > 0.5"
 *   "!disable_particles.value"
 * without using eval() or new Function().
 */
export function evaluatePropertyCondition(
  condition: string | undefined,
  currentValues: Record<string, string | number | boolean>
): boolean {
  if (!condition || condition.trim() === '') return true;

  // Split by logical OR (||) first
  const orClauses = condition.split('||');
  return orClauses.some((orClause) => {
    // Split by logical AND (&&)
    const andClauses = orClause.split('&&');
    return andClauses.every((rawAtom) => {
      let atom = rawAtom.trim();
      // Strip outer parentheses if present
      while (atom.startsWith('(') && atom.endsWith(')')) {
        atom = atom.slice(1, -1).trim();
      }
      if (atom === 'true' || atom === '1') return true;
      if (atom === 'false' || atom === '0') return false;

      // Match comparison operators: ==, !=, >=, <=, >, <
      const match = atom.match(
        /^(!?)([a-zA-Z0-9_]+)(?:\.value)?\s*(?:(==|!=|>=|<=|>|<)\s*(.+))?$/
      );
      if (!match) return true; // Fail open so controls are never accidentally hidden

      const [, negated, propKey, op, rhsRaw] = match;
      const lhsVal = currentValues[propKey];

      if (!op) {
        const truthy = Boolean(lhsVal) && lhsVal !== 'false' && lhsVal !== 0;
        return negated === '!' ? !truthy : truthy;
      }

      const rhsClean = rhsRaw.trim().replace(/^['"]|['"]$/g, '');
      let rhsVal: string | number | boolean = rhsClean;
      if (rhsClean === 'true') rhsVal = true;
      else if (rhsClean === 'false') rhsVal = false;
      else if (!isNaN(Number(rhsClean)) && rhsClean !== '') rhsVal = Number(rhsClean);

      let result = false;
      switch (op) {
        case '==':
          result = String(lhsVal) === String(rhsVal);
          break;
        case '!=':
          result = String(lhsVal) !== String(rhsVal);
          break;
        case '>=':
          result = Number(lhsVal) >= Number(rhsVal);
          break;
        case '<=':
          result = Number(lhsVal) <= Number(rhsVal);
          break;
        case '>':
          result = Number(lhsVal) > Number(rhsVal);
          break;
        case '<':
          result = Number(lhsVal) < Number(rhsVal);
          break;
      }
      return negated === '!' ? !result : result;
    });
  });
}

/**
 * Resolves localized labels (e.g. "ui_browse_properties_scheme_color") from project.json.
 */
export function resolveLocalizedText(
  rawText: string | undefined,
  project: ProjectJsonSchema,
  locale = 'en-us'
): string {
  if (!rawText) return '';
  const locTable =
    project.general?.localization?.[locale] ??
    project.general?.localization?.['en-us'];
  if (locTable && locTable[rawText]) {
    return locTable[rawText];
  }
  return rawText.replace(/<[^>]+>/g, ''); // Strip basic HTML tags often used in WE labels
}

interface PropertyInspectorProps {
  wallpaperId: string;
  project: ProjectJsonSchema;
  values: Record<string, string | number | boolean>;
  adjustments: UniversalAdjustments;
  presets: WallpaperPreset[];
  onPropertyChange: (key: string, value: string | number | boolean) => void;
  onAdjustmentChange: (next: UniversalAdjustments) => void;
}

export const PropertyInspector: React.FC<PropertyInspectorProps> = ({
  wallpaperId,
  project,
  values,
  adjustments,
  onPropertyChange,
  onAdjustmentChange,
}) => {
  const sortedProperties = useMemo(() => {
    const propsMap = project.general?.properties ?? {};
    return Object.entries(propsMap)
      .map(([key, prop]) => ({ key, ...prop }))
      .sort((a, b) => (a.order ?? 999) - (b.order ?? 999));
  }, [project]);

  const handleChange = async (key: string, val: string | number | boolean) => {
    onPropertyChange(key, val);
    await invoke('monitor_update_property', {
      wallpaperId,
      propKey: key,
      propValue: val,
    });
  };

  return (
    <aside className="glass-panel" style={{ padding: 20, overflowY: 'auto' }}>
      <h3 style={{ margin: '0 0 12px', fontSize: 16, fontWeight: 600 }}>
        {project.title}
      </h3>

      {/* Universal Color & Audio Adjustments */}
      <section style={{ marginBottom: 24 }}>
        <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 8 }}>
          UNIVERSAL PLAYBACK & AUDIO
        </div>
        <label style={{ display: 'block', fontSize: 13, marginBottom: 10 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <span>Master Volume</span>
            <span>{Math.round(adjustments.volume * 100)}%</span>
          </div>
          <input
            type="range"
            className="glass-slider"
            min={0}
            max={1}
            step={0.01}
            value={adjustments.volume}
            onChange={(e) => {
              const next = { ...adjustments, volume: Number(e.target.value) };
              onAdjustmentChange(next);
              invoke('monitor_update_adjustments', { wallpaperId, adjustments: next });
            }}
          />
        </label>
        <label style={{ display: 'block', fontSize: 13, marginBottom: 10 }}>
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <span>Playback Speed</span>
            <span>{adjustments.playbackSpeed.toFixed(2)}x</span>
          </div>
          <input
            type="range"
            className="glass-slider"
            min={0.1}
            max={3.0}
            step={0.05}
            value={adjustments.playbackSpeed}
            onChange={(e) => {
              const next = { ...adjustments, playbackSpeed: Number(e.target.value) };
              onAdjustmentChange(next);
              invoke('monitor_update_adjustments', { wallpaperId, adjustments: next });
            }}
          />
        </label>
      </section>

      {/* Auto-Generated project.json Properties */}
      <section>
        <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginBottom: 12 }}>
          WALLPAPER PROPERTIES
        </div>
        {sortedProperties.map((prop) => {
          const isVisible = evaluatePropertyCondition(prop.condition, values);
          if (!isVisible) return null;

          const label = resolveLocalizedText(prop.text, project) || prop.key;
          const currentVal = values[prop.key] ?? prop.value;

          if (prop.type === 'group') {
            return (
              <div
                key={prop.key}
                style={{
                  marginTop: 16,
                  marginBottom: 8,
                  fontSize: 12,
                  fontWeight: 600,
                  textTransform: 'uppercase',
                  letterSpacing: '0.04em',
                  color: 'var(--accent-cyan)',
                }}
              >
                {label}
              </div>
            );
          }

          if (prop.type === 'slider') {
            const min = prop.min ?? 0;
            const max = prop.max ?? 100;
            const step = prop.step ?? ( prop.fraction ? (max - min) / 100 : 1 );
            return (
              <div key={prop.key} style={{ marginBottom: 14 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 13 }}>
                  <span>{label}</span>
                  <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                    {Number(currentVal).toFixed(prop.fraction ? 2 : 0)}
                  </span>
                </div>
                <input
                  type="range"
                  className="glass-slider"
                  min={min}
                  max={max}
                  step={step}
                  value={Number(currentVal)}
                  onChange={(e) => handleChange(prop.key, Number(e.target.value))}
                />
              </div>
            );
          }

          if (prop.type === 'bool') {
            const checked = Boolean(currentVal) && currentVal !== 'false';
            return (
              <div
                key={prop.key}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  marginBottom: 12,
                  fontSize: 13,
                }}
              >
                <span>{label}</span>
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(e) => handleChange(prop.key, e.target.checked)}
                />
              </div>
            );
          }

          if (prop.type === 'color') {
            const hexVal = weColorToHex(String(currentVal));
            return (
              <div
                key={prop.key}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  marginBottom: 12,
                  fontSize: 13,
                }}
              >
                <span>{label}</span>
                <input
                  type="color"
                  value={hexVal}
                  onChange={(e) => handleChange(prop.key, hexToWeColor(e.target.value))}
                />
              </div>
            );
          }

          if (prop.type === 'combo') {
            return (
              <div key={prop.key} style={{ marginBottom: 14 }}>
                <div style={{ fontSize: 13, marginBottom: 4 }}>{label}</div>
                <select
                  className="glass-button"
                  style={{ width: '100%' }}
                  value={String(currentVal)}
                  onChange={(e) => handleChange(prop.key, e.target.value)}
                >
                  {(prop.options ?? []).map((opt) => (
                    <option key={String(opt.value)} value={String(opt.value)}>
                      {resolveLocalizedText(opt.label, project)}
                    </option>
                  ))}
                </select>
              </div>
            );
          }

          if (prop.type === 'file' || prop.type === 'scenetexture' || prop.type === 'directory') {
            return (
              <div key={prop.key} style={{ marginBottom: 14 }}>
                <div style={{ fontSize: 13, marginBottom: 4 }}>{label}</div>
                <button
                  type="button"
                  className="glass-button"
                  onClick={async () => {
                    const selected = await open({
                      directory: prop.type === 'directory',
                      multiple: false,
                    });
                    if (typeof selected === 'string') {
                      handleChange(prop.key, selected);
                    }
                  }}
                >
                  {currentVal ? String(currentVal).split(/[\\/]/).pop() : 'Choose File...'}
                </button>
              </div>
            );
          }

          return (
            <div key={prop.key} style={{ marginBottom: 14 }}>
              <div style={{ fontSize: 13, marginBottom: 4 }}>{label}</div>
              <input
                type="text"
                className="glass-button"
                style={{ width: '100%', textAlign: 'left' }}
                value={String(currentVal ?? '')}
                onChange={(e) => handleChange(prop.key, e.target.value)}
              />
            </div>
          );
        })}
      </section>
    </aside>
  );
};
```

---

## 4. Menu Bar (macOS) / System Tray (Windows) Liquid Glass Popover & Core UI Components

Clicking the TruongPaper icon in the macOS Menu Bar or Windows System Tray toggles the frameless `360px × 480px` `tray-popover` window positioned directly beneath (macOS) or above (Windows) the tray icon:

```text
+------------------------------------------+
|  TruongPaper              [Studio] [Gear]|
+------------------------------------------+
|  [ Paste Workshop or Photo Album Link ]  |
+------------------------------------------+
|  [ Monitor 1: 4K ]   [ Monitor 2: Vert ] |
+------------------------------------------+
|  +------------------------------------+  |
|  |         CURRENT WALLPAPER          |  |
|  |       Bald Mountain 11-1 (#84)     |  |
|  |     Ian Truong Photo API • 4K      |  |
|  +------------------------------------+  |
|  [ || Pause ]  [ >> Next Wallpaper ]     |
|  [ <3 Favorite ] [ Hide ] [ Open Album ] |
+------------------------------------------+
|  Volume       [========o-----]     65%   |
|  Transition   [ Liquid Glass Morph   v]  |
+------------------------------------------+
|  Queue: 5/5 Offline Ready • 92/250 MB    |
+------------------------------------------+
```

### 4.1 Complete Tauri v2 System Tray & Popover Positioning (`src-tauri/src/tray.rs`)

```rust
// src-tauri/src/tray.rs
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Rect, WindowEvent,
};

pub fn setup_system_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_studio = MenuItem::with_id(app, "open_studio", "Open TruongPaper Studio", true, None::<&str>)?;
    let next_wallpaper = MenuItem::with_id(app, "next_wallpaper", "Next Wallpaper", true, None::<&str>)?;
    let toggle_pause = MenuItem::with_id(app, "toggle_pause", "Pause / Resume Wallpapers", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit TruongPaper", true, None::<&str>)?;

    let context_menu = Menu::with_items(
        app,
        &[&open_studio, &next_wallpaper, &toggle_pause, &sep, &quit],
    )?;

    // Auto-hide the tray-popover window whenever it loses focus (blur)
    if let Some(popover) = app.get_webview_window("tray-popover") {
        let pop_clone = popover.clone();
        popover.on_window_event(move |event| {
            if let WindowEvent::Focused(false) = event {
                let _ = pop_clone.hide();
            }
        });
    }

    let icon = app
        .default_window_icon()
        .cloned()
        .unwrap_or_else(|| Image::new(&[], 0, 0));

    TrayIconBuilder::with_id("truongpaper-tray")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("TruongPaper")
        .menu(&context_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open_studio" => {
                if let Some(main_win) = app.get_webview_window("main") {
                    let _ = main_win.unminimize();
                    let _ = main_win.show();
                    let _ = main_win.set_focus();
                }
            }
            "next_wallpaper" => {
                let _ = app.emit("tray://next-requested", ());
            }
            "toggle_pause" => {
                let _ = app.emit("tray://toggle-pause-requested", ());
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                toggle_tray_popover(tray.app_handle(), rect);
            }
        })
        .build(app)?;

    Ok(())
}

fn toggle_tray_popover(app: &AppHandle, tray_rect: Rect) {
    let Some(popover) = app.get_webview_window("tray-popover") else {
        return;
    };

    if popover.is_visible().unwrap_or(false) {
        let _ = popover.hide();
        return;
    }

    let scale_factor = popover.scale_factor().unwrap_or(1.0);
    let win_size: PhysicalSize<u32> = popover
        .outer_size()
        .unwrap_or_else(|_| PhysicalSize::new((360.0 * scale_factor) as u32, (480.0 * scale_factor) as u32));

    let (icon_x, icon_y) = match tray_rect.position {
        Position::Physical(p) => (p.x as f64, p.y as f64),
        Position::Logical(l) => (l.x * scale_factor, l.y * scale_factor),
    };
    let (icon_w, icon_h) = match tray_rect.size {
        tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
        tauri::Size::Logical(l) => (l.width * scale_factor, l.height * scale_factor),
    };

    // Center horizontally on the tray icon
    let mut target_x = (icon_x + (icon_w / 2.0) - (win_size.width as f64 / 2.0)).round() as i32;

    // On macOS, open directly below the Menu Bar icon; on Windows, open directly above the System Tray icon
    let margin_px = (6.0 * scale_factor).round() as i32;
    let mut target_y = if cfg!(target_os = "macos") {
        (icon_y + icon_h).round() as i32 + margin_px
    } else {
        icon_y.round() as i32 - (win_size.height as i32) - margin_px
    };

    // Clamp to the active monitor bounds so the popover never clips off-screen
    if let Ok(Some(monitor)) = popover.current_monitor() {
        let mon_pos = monitor.position();
        let mon_size = monitor.size();
        let min_x = mon_pos.x + margin_px;
        let max_x = mon_pos.x + (mon_size.width as i32) - (win_size.width as i32) - margin_px;
        let min_y = mon_pos.y + margin_px;
        let max_y = mon_pos.y + (mon_size.height as i32) - (win_size.height as i32) - margin_px;

        target_x = target_x.clamp(min_x, max_x.max(min_x));
        target_y = target_y.clamp(min_y, max_y.max(min_y));
    }

    let _ = popover.set_position(Position::Physical(PhysicalPosition::new(target_x, target_y)));
    let _ = popover.show();
    let _ = popover.set_focus();
}
```

### 4.2 Complete Zustand Store (`src/store/useAppStore.ts`)

```typescript
// src/store/useAppStore.ts
import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AppSettings,
  FilterMode,
  MonitorInfo,
  PhotoItem,
  PhotoStatus,
  Playlist,
  PublicCatalogAlbum,
  SavedAlbum,
  TransitionType,
  UniversalAdjustments,
  WallpaperItem,
  WallpaperSource,
  WorkshopPreview,
} from '../types/ipc';

export interface SteamDownloadProgress {
  workshopId: string;
  title: string;
  progressPercent: number;
  state: 'downloading' | 'completed' | 'error';
  error?: string;
}

export interface Steam2FaPrompt {
  kind: 'totp' | 'email';
  prompt: string;
}

export interface AppState {
  photoStatus: PhotoStatus | null;
  publicCatalog: PublicCatalogAlbum[];
  savedAlbums: SavedAlbum[];
  wallpapers: WallpaperItem[];
  monitors: MonitorInfo[];
  selectedMonitorId: string | null;
  selectedWallpaperId: string | null;
  playlists: Playlist[];
  settings: AppSettings | null;
  isPaused: boolean;
  policyStateByMonitor: Record<string, string>;

  inspectedWorkshop: WorkshopPreview | null;
  downloadsById: Record<string, SteamDownloadProgress>;
  authModalOpen: boolean;
  qrMatrix: boolean[][] | null;
  need2Fa: Steam2FaPrompt | null;

  initAppListeners: () => Promise<UnlistenFn>;
  refreshAll: () => Promise<void>;
  rotateNextPhoto: (monitorId?: string) => Promise<void>;
  triggerPhotoRefill: () => Promise<void>;
  addAlbum: (urlOrSlug: string, displayName?: string) => Promise<SavedAlbum>;
  toggleAlbum: (albumSlug: string, enabled: boolean) => Promise<void>;
  setPhotoFilter: (mode: FilterMode, featuredOnly: boolean) => Promise<void>;
  setPhotoFavorite: (photoId: string, favorite: boolean) => Promise<void>;
  blacklistAndSkipPhoto: (photoId: string) => Promise<void>;
  setCacheLimit: (maxBytes: number) => Promise<void>;
  clearPhotoCache: () => Promise<void>;
  resetPhotoHistory: (albumSlug?: string) => Promise<void>;

  inspectWorkshopUrl: (urlOrId: string) => Promise<WorkshopPreview>;
  downloadWorkshopItem: (urlOrId: string) => Promise<void>;
  syncLocalSteamFolders: () => Promise<void>;
  openSteamAuthModal: () => void;
  closeSteamAuthModal: () => void;
  startSteamQrLogin: () => Promise<void>;
  submitSteamCredentials: (username: string, password: string) => Promise<void>;
  submitSteam2FaCode: (code: string) => Promise<void>;

  applyMonitorSource: (monitorId: string, source: WallpaperSource) => Promise<void>;
  updateWallpaperProperty: (wallpaperId: string, key: string, value: string | number | boolean) => Promise<void>;
  updateWallpaperAdjustments: (wallpaperId: string, adjustments: UniversalAdjustments) => Promise<void>;
  setPlaybackPaused: (paused: boolean, monitorId?: string) => Promise<void>;
  updateSettings: (partial: Partial<AppSettings>) => Promise<void>;
}

export const useAppStore = create<AppState>((set, get) => ({
  photoStatus: null,
  publicCatalog: [],
  savedAlbums: [],
  wallpapers: [],
  monitors: [],
  selectedMonitorId: null,
  selectedWallpaperId: null,
  playlists: [],
  settings: null,
  isPaused: false,
  policyStateByMonitor: {},

  inspectedWorkshop: null,
  downloadsById: {},
  authModalOpen: false,
  qrMatrix: null,
  need2Fa: null,

  refreshAll: async () => {
    const [photoStatus, monitors, wallpapers, settings, playlists] = await Promise.all([
      invoke<PhotoStatus>('photo_get_status'),
      invoke<MonitorInfo[]>('monitors_list'),
      invoke<WallpaperItem[]>('library_list_wallpapers'),
      invoke<AppSettings>('settings_get'),
      invoke<Playlist[]>('playlist_list').catch(() => [] as Playlist[]),
    ]);
    set((state) => ({
      photoStatus,
      monitors,
      selectedMonitorId: state.selectedMonitorId ?? monitors[0]?.monitorId ?? null,
      wallpapers,
      selectedWallpaperId: state.selectedWallpaperId ?? wallpapers[0]?.wallpaperId ?? null,
      settings,
      playlists,
    }));
  },

  initAppListeners: async () => {
    await get().refreshAll();

    const unlisteners = await Promise.all([
      listen<{
        monitorId: string;
        currentItem: WallpaperItem | PhotoItem;
        transitionUsed: TransitionType;
      }>('wallpaper://changed', (event) => {
        const { monitorId, currentItem } = event.payload;
        if ('albumSlug' in currentItem) {
          set((state) =>
            state.photoStatus
              ? {
                  photoStatus: {
                    ...state.photoStatus,
                    currentPhotoByMonitor: {
                      ...state.photoStatus.currentPhotoByMonitor,
                      [monitorId]: currentItem,
                    },
                  },
                }
              : state
          );
        }
      }),

      listen<PhotoStatus>('photo-queue://updated', (event) => {
        set({ photoStatus: event.payload });
      }),

      listen<SteamDownloadProgress>('steam-download://progress', (event) => {
        const prog = event.payload;
        set((state) => ({
          downloadsById: { ...state.downloadsById, [prog.workshopId]: prog },
        }));
        if (prog.state === 'completed') {
          void invoke<WallpaperItem[]>('library_list_wallpapers').then((wallpapers) =>
            set({ wallpapers })
          );
        }
      }),

      listen<{ matrix: boolean[][] }>('steam-auth://qr-matrix', (event) => {
        set({ qrMatrix: event.payload.matrix, authModalOpen: true });
      }),

      listen<Steam2FaPrompt>('steam-auth://need-2fa', (event) => {
        set({ need2Fa: event.payload, authModalOpen: true });
      }),

      listen<{ monitorId: string; state: string }>('policy://state-changed', (event) => {
        set((state) => ({
          policyStateByMonitor: {
            ...state.policyStateByMonitor,
            [event.payload.monitorId]: event.payload.state,
          },
        }));
      }),

      listen<{ kind: 'pause' | 'resume'; reason: string }>('system-power', (event) => {
        set({ isPaused: event.payload.kind === 'pause' });
      }),
    ]);

    return () => {
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  },

  rotateNextPhoto: async (monitorId) => {
    await invoke<PhotoItem>('photo_rotate_next', { monitorId });
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  },

  triggerPhotoRefill: async () => {
    await invoke('photo_trigger_refill');
  },

  addAlbum: async (urlOrSlug, displayName) => {
    const album = await invoke<SavedAlbum>('photo_add_album', { urlOrSlug, displayName });
    set((state) => ({
      savedAlbums: [
        ...state.savedAlbums.filter((a) => a.albumSlug !== album.albumSlug),
        album,
      ],
    }));
    return album;
  },

  toggleAlbum: async (albumSlug, enabled) => {
    await invoke('photo_toggle_album', { albumSlug, enabled });
    set((state) => ({
      savedAlbums: state.savedAlbums.map((a) =>
        a.albumSlug === albumSlug ? { ...a, isEnabled: enabled } : a
      ),
    }));
  },

  setPhotoFilter: async (mode, featuredOnly) => {
    await invoke('photo_set_filter', { mode, featuredOnly });
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  },

  setPhotoFavorite: async (photoId, favorite) => {
    await invoke('photo_set_favorite', { photoId, favorite });
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  },

  blacklistAndSkipPhoto: async (photoId) => {
    await invoke('photo_blacklist_and_skip', { photoId });
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  },

  setCacheLimit: async (maxBytes) => {
    await invoke('photo_set_cache_limit', { maxBytes });
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  },

  clearPhotoCache: async () => {
    await invoke('photo_clear_cache');
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  },

  resetPhotoHistory: async (albumSlug) => {
    await invoke('photo_reset_history', { albumSlug });
    const photoStatus = await invoke<PhotoStatus>('photo_get_status');
    set({ photoStatus });
  },

  inspectWorkshopUrl: async (urlOrId) => {
    const preview = await invoke<WorkshopPreview>('steam_inspect_url', { urlOrId });
    set({ inspectedWorkshop: preview });
    return preview;
  },

  downloadWorkshopItem: async (urlOrId) => {
    await invoke<string>('steam_download_workshop_item', { urlOrId });
  },

  syncLocalSteamFolders: async () => {
    const wallpapers = await invoke<WallpaperItem[]>('steam_sync_local_folders');
    set({ wallpapers });
  },

  openSteamAuthModal: () => set({ authModalOpen: true }),
  closeSteamAuthModal: () => set({ authModalOpen: false, qrMatrix: null, need2Fa: null }),

  startSteamQrLogin: async () => {
    set({ qrMatrix: null, need2Fa: null, authModalOpen: true });
    await invoke('steam_login_qr');
  },

  submitSteamCredentials: async (username, password) => {
    set({ need2Fa: null });
    await invoke('steam_login_credentials', { username, password });
  },

  submitSteam2FaCode: async (code) => {
    await invoke('steam_submit_2fa_code', { code });
    set({ need2Fa: null });
  },

  applyMonitorSource: async (monitorId, source) => {
    await invoke('monitor_apply_wallpaper', { monitorId, source });
    const monitors = await invoke<MonitorInfo[]>('monitors_list');
    set({ monitors });
  },

  updateWallpaperProperty: async (wallpaperId, propKey, propValue) => {
    set((state) => ({
      wallpapers: state.wallpapers.map((w) =>
        w.wallpaperId === wallpaperId
          ? { ...w, userProperties: { ...w.userProperties, [propKey]: propValue } }
          : w
      ),
    }));
    await invoke('monitor_update_property', { wallpaperId, propKey, propValue });
  },

  updateWallpaperAdjustments: async (wallpaperId, adjustments) => {
    set((state) => ({
      wallpapers: state.wallpapers.map((w) =>
        w.wallpaperId === wallpaperId ? { ...w, adjustments } : w
      ),
    }));
    await invoke('monitor_update_adjustments', { wallpaperId, adjustments });
  },

  setPlaybackPaused: async (paused, monitorId) => {
    await invoke('playback_set_paused', { monitorId, paused });
    set({ isPaused: paused });
  },

  updateSettings: async (partial) => {
    await invoke('settings_update', { settings: partial });
    const settings = await invoke<AppSettings>('settings_get');
    set({ settings });
  },
}));

export const useWallpaperStore = useAppStore;
```

### 4.3 Universal Paste Omnibar (`src/components/UniversalOmnibar.tsx`)

```tsx
// src/components/UniversalOmnibar.tsx
import React, { useEffect, useState } from 'react';
import { useAppStore } from '../store/useAppStore';

export type ParsedOmnibarTarget =
  | { kind: 'steam_workshop'; workshopId: string }
  | { kind: 'photo_album'; albumSlug: string }
  | null;

/**
 * Classifies any pasted URL, Workshop numeric ID, or Ian Truong Photography album slug.
 */
export function parseOmnibarInput(raw: string): ParsedOmnibarTarget {
  const trimmed = raw.trim();
  if (!trimmed) return null;

  const steamUrlMatch =
    trimmed.match(/steamcommunity\.com\/(?:sharedfiles|workshop)\/filedetails\/?\?.*?\bid=(\d{6,12})/i) ??
    trimmed.match(/steam:\/\/url\/CommunityFilePage\/(\d{6,12})/i);
  if (steamUrlMatch) {
    return { kind: 'steam_workshop', workshopId: steamUrlMatch[1] };
  }

  if (/^\d{7,12}$/.test(trimmed)) {
    return { kind: 'steam_workshop', workshopId: trimmed };
  }

  const albumUrlMatch = trimmed.match(
    /(?:https?:\/\/)?(?:www\.)?iantruongphotography\.com\/album\/([a-z0-9-]+)\/?/i
  );
  if (albumUrlMatch) {
    return { kind: 'photo_album', albumSlug: albumUrlMatch[1].toLowerCase() };
  }

  if (/^[a-z0-9]+(?:-[a-z0-9]+)+$/i.test(trimmed)) {
    return { kind: 'photo_album', albumSlug: trimmed.toLowerCase() };
  }

  return null;
}

export const UniversalOmnibar: React.FC = () => {
  const [input, setInput] = useState('');
  const [statusMsg, setStatusMsg] = useState<string | null>(null);
  const inspectWorkshopUrl = useAppStore((s) => s.inspectWorkshopUrl);
  const downloadWorkshopItem = useAppStore((s) => s.downloadWorkshopItem);
  const addAlbum = useAppStore((s) => s.addAlbum);
  const openSteamAuthModal = useAppStore((s) => s.openSteamAuthModal);

  const handleResolvedInput = async (rawText: string) => {
    const parsed = parseOmnibarInput(rawText);
    if (!parsed) {
      setStatusMsg('Paste a valid Steam Workshop URL/ID or iantruongphotography.com album link/slug');
      return;
    }

    try {
      if (parsed.kind === 'steam_workshop') {
        setStatusMsg(`Inspecting Steam Workshop #${parsed.workshopId}...`);
        const preview = await inspectWorkshopUrl(parsed.workshopId);
        setStatusMsg(`Downloading "${preview.title}"...`);
        await downloadWorkshopItem(parsed.workshopId);
        setInput('');
      } else {
        setStatusMsg(`Adding album "${parsed.albumSlug}"...`);
        const saved = await addAlbum(parsed.albumSlug);
        setStatusMsg(`Added album "${saved.displayName}" to Saved Albums`);
        setInput('');
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      if (msg.toLowerCase().includes('auth') || msg.toLowerCase().includes('login')) {
        openSteamAuthModal();
      }
      setStatusMsg(msg);
    }
  };

  useEffect(() => {
    const onWindowPaste = (e: ClipboardEvent) => {
      const active = document.activeElement;
      if (
        active instanceof HTMLInputElement ||
        active instanceof HTMLTextAreaElement ||
        (active instanceof HTMLElement && active.isContentEditable)
      ) {
        return;
      }
      const pasted = e.clipboardData?.getData('text/plain')?.trim();
      if (pasted && parseOmnibarInput(pasted)) {
        e.preventDefault();
        setInput(pasted);
        void handleResolvedInput(pasted);
      }
    };

    window.addEventListener('paste', onWindowPaste);
    return () => window.removeEventListener('paste', onWindowPaste);
  }, []);

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void handleResolvedInput(input);
      }}
      style={{ display: 'flex', alignItems: 'center', gap: 10, flex: 1 }}
    >
      <input
        type="text"
        value={input}
        onChange={(e) => setInput(e.target.value)}
        placeholder="Paste Steam Workshop URL / ID or Ian's Photo Album URL / Slug (Cmd/Ctrl+V anywhere)..."
        className="glass-button"
        style={{
          flex: 1,
          justifyContent: 'flex-start',
          textAlign: 'left',
          padding: '10px 16px',
          cursor: 'text',
        }}
      />
      <button type="submit" className="glass-button glass-button-primary">
        + Import
      </button>
      {statusMsg && (
        <span style={{ fontSize: 12, color: 'var(--text-secondary)', whiteSpace: 'nowrap' }}>
          {statusMsg}
        </span>
      )}
    </form>
  );
};
```

### 4.4 Steam QR Code & 2FA Authentication Modal (`src/components/SteamAuthModal.tsx`)

```tsx
// src/components/SteamAuthModal.tsx
import React, { useEffect, useRef, useState } from 'react';
import { useAppStore } from '../store/useAppStore';

export const SteamAuthModal: React.FC = () => {
  const authModalOpen = useAppStore((s) => s.authModalOpen);
  const qrMatrix = useAppStore((s) => s.qrMatrix);
  const need2Fa = useAppStore((s) => s.need2Fa);
  const closeSteamAuthModal = useAppStore((s) => s.closeSteamAuthModal);
  const startSteamQrLogin = useAppStore((s) => s.startSteamQrLogin);
  const submitSteamCredentials = useAppStore((s) => s.submitSteamCredentials);
  const submitSteam2FaCode = useAppStore((s) => s.submitSteam2FaCode);

  const [tab, setTab] = useState<'qr' | 'password'>('qr');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [twoFaCode, setTwoFaCode] = useState('');
  const qrCanvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    if (authModalOpen && tab === 'qr' && !qrMatrix) {
      void startSteamQrLogin();
    }
  }, [authModalOpen, tab]);

  useEffect(() => {
    const canvas = qrCanvasRef.current;
    if (!canvas || !qrMatrix || qrMatrix.length === 0) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const rows = qrMatrix.length;
    const cols = qrMatrix[0].length;
    const quietZone = 4;
    const modulePx = 5;
    canvas.width = (cols + quietZone * 2) * modulePx;
    canvas.height = (rows + quietZone * 2) * modulePx;

    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, canvas.width, canvas.height);

    ctx.fillStyle = '#090b12';
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        if (qrMatrix[r][c]) {
          ctx.fillRect(
            (c + quietZone) * modulePx,
            (r + quietZone) * modulePx,
            modulePx,
            modulePx
          );
        }
      }
    }
  }, [qrMatrix]);

  if (!authModalOpen) return null;

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 100,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'rgba(5, 7, 14, 0.65)',
        backdropFilter: 'blur(12px)',
      }}
    >
      <div className="glass-panel" style={{ width: 420, padding: 24 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 16 }}>
          <h3 style={{ margin: 0, fontSize: 18 }}>Sign in to Steam Workshop</h3>
          <button type="button" className="glass-button" onClick={closeSteamAuthModal}>
            Close
          </button>
        </div>

        <div style={{ display: 'flex', gap: 8, marginBottom: 18 }}>
          <button
            type="button"
            className={`glass-button ${tab === 'qr' ? 'glass-button-primary' : ''}`}
            style={{ flex: 1 }}
            onClick={() => setTab('qr')}
          >
            Steam Mobile QR
          </button>
          <button
            type="button"
            className={`glass-button ${tab === 'password' ? 'glass-button-primary' : ''}`}
            style={{ flex: 1 }}
            onClick={() => setTab('password')}
          >
            Username & 2FA
          </button>
        </div>

        {tab === 'qr' ? (
          <div style={{ textAlign: 'center' }}>
            <p style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 14 }}>
              Scan with the Steam Mobile App to authenticate DepotDownloader and save a persistent refresh token in your OS Keychain.
            </p>
            {qrMatrix ? (
              <canvas
                ref={qrCanvasRef}
                style={{
                  borderRadius: 14,
                  boxShadow: '0 12px 32px rgba(0, 0, 0, 0.45)',
                  maxWidth: '100%',
                }}
              />
            ) : (
              <div style={{ padding: '48px 0', color: 'var(--text-muted)', fontSize: 13 }}>
                Generating Steam QR session...
              </div>
            )}
          </div>
        ) : (
          <div>
            {!need2Fa ? (
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void submitSteamCredentials(username, password);
                }}
                style={{ display: 'flex', flexDirection: 'column', gap: 12 }}
              >
                <input
                  type="text"
                  placeholder="Steam Username"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  className="glass-button"
                  style={{ width: '100%', textAlign: 'left', cursor: 'text' }}
                  required
                />
                <input
                  type="password"
                  placeholder="Steam Password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  className="glass-button"
                  style={{ width: '100%', textAlign: 'left', cursor: 'text' }}
                  required
                />
                <button type="submit" className="glass-button glass-button-primary">
                  Sign In
                </button>
              </form>
            ) : (
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  void submitSteam2FaCode(twoFaCode);
                }}
                style={{ display: 'flex', flexDirection: 'column', gap: 12 }}
              >
                <div style={{ fontSize: 13, color: 'var(--accent-cyan)' }}>{need2Fa.prompt}</div>
                <input
                  type="text"
                  placeholder={need2Fa.kind === 'email' ? 'Email Steam Guard Code' : 'Authenticator 2FA Code'}
                  value={twoFaCode}
                  onChange={(e) => setTwoFaCode(e.target.value)}
                  className="glass-button"
                  style={{ width: '100%', textAlign: 'center', fontFamily: 'var(--font-mono)', fontSize: 16 }}
                  autoFocus
                  required
                />
                <button type="submit" className="glass-button glass-button-primary">
                  Verify Steam Guard Code
                </button>
              </form>
            )}
          </div>
        )}
      </div>
    </div>
  );
};
```

### 4.5 Menu Bar / System Tray Liquid Glass Popover (`src/tray/TrayApp.tsx` & `src/tray/tray-main.tsx`)

```tsx
// src/tray/TrayApp.tsx
import React, { useEffect, useState } from 'react';
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { open } from '@tauri-apps/plugin-shell';
import { AmbientBacklightCanvas } from '../components/AmbientBacklightCanvas';
import { LiquidGlassFilters } from '../components/LiquidGlassFilters';
import { parseOmnibarInput } from '../components/UniversalOmnibar';
import { useAppStore } from '../store/useAppStore';
import type { TransitionType } from '../types/ipc';

export const TrayApp: React.FC = () => {
  const initAppListeners = useAppStore((s) => s.initAppListeners);
  const photoStatus = useAppStore((s) => s.photoStatus);
  const monitors = useAppStore((s) => s.monitors);
  const selectedMonitorId = useAppStore((s) => s.selectedMonitorId);
  const settings = useAppStore((s) => s.settings);
  const isPaused = useAppStore((s) => s.isPaused);
  const rotateNextPhoto = useAppStore((s) => s.rotateNextPhoto);
  const setPhotoFavorite = useAppStore((s) => s.setPhotoFavorite);
  const blacklistAndSkipPhoto = useAppStore((s) => s.blacklistAndSkipPhoto);
  const setPlaybackPaused = useAppStore((s) => s.setPlaybackPaused);
  const updateSettings = useAppStore((s) => s.updateSettings);
  const addAlbum = useAppStore((s) => s.addAlbum);
  const downloadWorkshopItem = useAppStore((s) => s.downloadWorkshopItem);

  const [quickLink, setQuickLink] = useState('');

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void initAppListeners().then((fn) => {
      cleanup = fn;
    });
    return () => cleanup?.();
  }, []);

  const activeMonitorId = selectedMonitorId ?? monitors[0]?.monitorId ?? 'DISPLAY-1';
  const currentPhoto = photoStatus?.currentPhotoByMonitor?.[activeMonitorId] ?? photoStatus?.unusedQueue?.[0] ?? null;

  const handleQuickPaste = async (e: React.FormEvent) => {
    e.preventDefault();
    const parsed = parseOmnibarInput(quickLink);
    if (!parsed) return;
    if (parsed.kind === 'steam_workshop') {
      await downloadWorkshopItem(parsed.workshopId);
    } else {
      await addAlbum(parsed.albumSlug);
    }
    setQuickLink('');
  };

  return (
    <div className="app-window-frame" style={{ width: 360, height: 480, padding: 14, display: 'flex', flexDirection: 'column', gap: 10 }}>
      <LiquidGlassFilters />
      <AmbientBacklightCanvas activeThumbnailUrl={currentPhoto?.filePath ?? null} />

      <div style={{ position: 'relative', zIndex: 1, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <strong style={{ fontSize: 15, letterSpacing: '-0.01em' }}>TruongPaper</strong>
        <div style={{ display: 'flex', gap: 6 }}>
          <button
            type="button"
            className="glass-button"
            style={{ padding: '5px 12px', fontSize: 12 }}
            onClick={async () => {
              const studio = await getCurrentWebviewWindow();
              await studio.hide();
            }}
          >
            Studio
          </button>
        </div>
      </div>

      <form onSubmit={handleQuickPaste} style={{ position: 'relative', zIndex: 1 }}>
        <input
          type="text"
          value={quickLink}
          onChange={(e) => setQuickLink(e.target.value)}
          placeholder="Paste Workshop or Photo Album Link..."
          className="glass-button"
          style={{ width: '100%', textAlign: 'left', fontSize: 12, padding: '7px 12px', cursor: 'text' }}
        />
      </form>

      <div style={{ position: 'relative', zIndex: 1, display: 'flex', gap: 6, overflowX: 'auto' }}>
        {monitors.map((m) => (
          <button
            key={m.monitorId}
            type="button"
            className={`glass-button ${m.monitorId === activeMonitorId ? 'glass-button-primary' : ''}`}
            style={{ fontSize: 11, padding: '4px 10px' }}
            onClick={() => useAppStore.setState({ selectedMonitorId: m.monitorId })}
          >
            {m.name}
          </button>
        ))}
      </div>

      <div className="glass-card" style={{ position: 'relative', zIndex: 1, padding: 14, flex: 1, display: 'flex', flexDirection: 'column', justifyContent: 'space-between' }}>
        <div>
          <div style={{ fontSize: 11, color: 'var(--accent-cyan)', textTransform: 'uppercase', letterSpacing: '0.05em' }}>
            Current Wallpaper
          </div>
          <div style={{ fontSize: 15, fontWeight: 600, marginTop: 4 }}>
            {currentPhoto ? currentPhoto.albumSlug : 'Ian Truong Photo Pool'}
          </div>
          <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 2 }}>
            {currentPhoto?.width && currentPhoto?.height
              ? `${currentPhoto.width} × ${currentPhoto.height} • ${(currentPhoto.bytes / (1024 * 1024)).toFixed(1)} MB`
              : 'Ready for instant offline rotation'}
          </div>
        </div>

        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', gap: 8 }}>
            <button
              type="button"
              className="glass-button"
              style={{ flex: 1 }}
              onClick={() => void setPlaybackPaused(!isPaused, activeMonitorId)}
            >
              {isPaused ? 'Resume' : 'Pause'}
            </button>
            <button
              type="button"
              className="glass-button glass-button-primary"
              style={{ flex: 1 }}
              onClick={() => void rotateNextPhoto(activeMonitorId)}
            >
              Next Wallpaper
            </button>
          </div>

          {currentPhoto && (
            <div style={{ display: 'flex', gap: 6 }}>
              <button
                type="button"
                className="glass-button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void setPhotoFavorite(currentPhoto.id, !currentPhoto.isFavorite)}
              >
                {currentPhoto.isFavorite ? '★ Favorited' : '☆ Favorite'}
              </button>
              <button
                type="button"
                className="glass-button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void blacklistAndSkipPhoto(currentPhoto.id)}
              >
                Hide
              </button>
              <button
                type="button"
                className="glass-button"
                style={{ flex: 1, fontSize: 11, padding: '6px 8px' }}
                onClick={() => void open(currentPhoto.albumUrl)}
              >
                Open Album
              </button>
            </div>
          )}
        </div>
      </div>

      <div style={{ position: 'relative', zIndex: 1, display: 'flex', alignItems: 'center', justifyContent: 'space-between', fontSize: 12 }}>
        <span style={{ color: 'var(--text-secondary)' }}>GPU Transition</span>
        <select
          className="glass-button"
          style={{ fontSize: 12, padding: '4px 10px' }}
          value={settings?.transitionType ?? 'liquid_glass'}
          onChange={(e) => void updateSettings({ transitionType: e.target.value as TransitionType })}
        >
          <option value="liquid_glass">Liquid Glass Morph</option>
          <option value="blur_dissolve">Blur-Dissolve</option>
          <option value="crossfade">Smooth Crossfade</option>
        </select>
      </div>

      <div style={{ position: 'relative', zIndex: 1, fontSize: 11, color: 'var(--text-muted)', textAlign: 'center' }}>
        Queue: {photoStatus?.unusedQueue.length ?? 0}/5 Offline Ready •{' '}
        {Math.round((photoStatus?.cachedBytes ?? 0) / (1024 * 1024))} /{' '}
        {Math.round((photoStatus?.maxCacheBytes ?? 250 * 1024 * 1024) / (1024 * 1024))} MB
      </div>
    </div>
  );
};
```

```tsx
// src/tray/tray-main.tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import { installBrowserPreviewMocksIfNeeded } from '../dev/mockTauri';
import { TrayApp } from './TrayApp';
import '../styles/liquid-glass.css';

installBrowserPreviewMocksIfNeeded();

ReactDOM.createRoot(document.getElementById('tray-root') as HTMLElement).render(
  <React.StrictMode>
    <TrayApp />
  </React.StrictMode>
);
```

---

## 5. Complete TypeScript IPC Contract (`src/types/ipc.ts`)

```typescript
// src/types/ipc.ts

export type FilterMode = 'all' | 'featured' | 'albums';
export type WallpaperType = 'photo' | 'scene' | 'video' | 'web' | 'application';
export type FitMode = 'fill' | 'fit_blur' | 'fit_black' | 'stretch';
export type TransitionType = 'crossfade' | 'blur_dissolve' | 'liquid_glass';

export interface PhotoItem {
  id: string;
  albumSlug: string;
  albumUrl: string;
  filePath: string;
  width: number | null;
  height: number | null;
  bytes: number;
  featured: boolean;
  isFavorite: boolean;
  downloadedAt: number;
  lastShownAt: number | null;
}

export interface SavedAlbum {
  albumSlug: string;
  displayName: string;
  albumUrl: string;
  isEnabled: boolean;
  coverThumbnailUrl?: string;
  imageCount?: number;
  addedAt: number;
}

export interface PublicCatalogAlbum {
  albumId: string;
  type: 'photo';
  title: string;
  description: string;
  category: string;
  imageCount: number;
  coverImageUrl: string;
  coverThumbnailUrl: string;
  coverBlurhash: string;
  slug: string;
}

export interface PhotoStatus {
  filterMode: FilterMode;
  featuredOnly: boolean;
  unusedQueue: PhotoItem[];
  currentPhotoByMonitor: Record<string, PhotoItem | null>;
  cachedBytes: number;
  maxCacheBytes: number;
  historyCount: number;
  blacklistCount: number;
  isRefilling: boolean;
  rateLimitRemaining: number;
}

export interface ProjectJsonComboOption {
  label: string;
  value: string | number;
}

export interface ProjectJsonProperty {
  type: 'slider' | 'bool' | 'color' | 'combo' | 'text' | 'file' | 'scenetexture' | 'directory' | 'group';
  text?: string;
  value?: string | number | boolean;
  order?: number;
  min?: number;
  max?: number;
  step?: number;
  fraction?: boolean;
  options?: ProjectJsonComboOption[];
  condition?: string;
}

export interface ProjectJsonSchema {
  title: string;
  type: 'scene' | 'video' | 'web' | 'application';
  file: string;
  preview?: string;
  description?: string;
  tags?: string[];
  workshopid?: string | number;
  general?: {
    supportsaudioprocessing?: boolean;
    properties?: Record<string, ProjectJsonProperty>;
    localization?: Record<string, Record<string, string>>;
  };
}

export interface UniversalAdjustments {
  volume: number;         // 0.0 .. 1.0
  muted: boolean;
  audioSensitivity: number; // 0.0 .. 5.0
  playbackSpeed: number;  // 0.1 .. 3.0
  fitMode: FitMode;
  zoom: number;           // 0.5 .. 2.0
  offsetX: number;        // -1.0 .. 1.0
  offsetY: number;        // -1.0 .. 1.0
  brightness: number;     // -0.5 .. 0.5 (default 0.0)
  contrast: number;       // 0.5 .. 2.0  (default 1.0)
  saturation: number;     // 0.0 .. 2.0  (default 1.0)
  hueShift: number;       // -3.14159 .. 3.14159 (default 0.0)
}

export interface WallpaperItem {
  wallpaperId: string;    // "workshop:<id>" or "local:<hash>"
  workshopId?: string;
  title: string;
  type: WallpaperType;
  previewPath: string;
  installDir: string;
  bytes: number;
  audioReactive: boolean;
  windowsOnly: boolean;
  projectJson: ProjectJsonSchema;
  userProperties: Record<string, string | number | boolean>;
  adjustments: UniversalAdjustments;
}

export interface WallpaperPreset {
  presetId: string;
  wallpaperId: string;
  name: string;
  properties: Record<string, string | number | boolean>;
  adjustments: UniversalAdjustments;
}

export interface WorkshopPreview {
  workshopId: string;
  title: string;
  description: string;
  previewUrl: string;
  fileSizeBytes: number;
  tags: string[];
  timeUpdated: number;
}

export interface MonitorInfo {
  monitorId: string;
  name: string;
  width: number;
  height: number;
  scaleFactor: number;
  refreshRateHz: number;
  isPrimary: boolean;
  isPortrait: boolean;
  activeSource: WallpaperSource;
  activeWallpaperId: string | null;
}

export type WallpaperSource =
  | { kind: 'photo_api' }
  | { kind: 'workshop'; wallpaperId: string }
  | { kind: 'playlist'; playlistId: string };

export interface Playlist {
  playlistId: string;
  name: string;
  mode: 'photos_only' | 'workshop_only' | 'mixed';
  intervalSeconds: number;
  items: Array<{ kind: 'photo_api_draw' } | { kind: 'workshop'; wallpaperId: string }>;
}

export interface AppSettings {
  rotationIntervalSeconds: number;
  transitionType: TransitionType;
  transitionDurationSeconds: number;
  skipMismatchedOrientation: boolean;
  defaultFitMode: FitMode;
  exhaustionAutoReset: boolean;
  maxPhotoCacheBytes: number;
  fpsCap: 30 | 60 | 120 | 'display_native';
  qualityPreset: 'low' | 'medium' | 'high' | 'ultra';
  pauseOnFullscreen: boolean;
  pauseOnMaximized: boolean;
  batteryPolicy: 'continue' | 'pause_animations' | 'switch_to_photo';
  launchAtLogin: boolean;
}
```

### 5.1 Complete Tauri v2 IPC Commands & Real-Time Events

```typescript
// --- Photo API & Cache Commands ---
invoke<PhotoStatus>('photo_get_status');
invoke<PublicCatalogAlbum[]>('photo_fetch_public_catalog');
invoke<PhotoItem>('photo_rotate_next', { monitorId?: string });
invoke<void>('photo_trigger_refill');
invoke<SavedAlbum>('photo_add_album', { urlOrSlug: string, displayName?: string });
invoke<void>('photo_toggle_album', { albumSlug: string, enabled: boolean });
invoke<void>('photo_remove_album', { albumSlug: string });
invoke<void>('photo_set_filter', { mode: FilterMode, featuredOnly: boolean });
invoke<void>('photo_set_favorite', { photoId: string, favorite: boolean });
invoke<void>('photo_blacklist_and_skip', { photoId: string });
invoke<void>('photo_set_cache_limit', { maxBytes: number });
invoke<void>('photo_clear_cache'); // Deletes non-favorited .jpg files, NEVER deletes photos_history
invoke<void>('photo_reset_history', { albumSlug?: string });

// --- Steam Workshop & Wallpaper Engine Commands ---
invoke<WorkshopBrowsePage>('steam_browse_workshop', { query: WorkshopBrowseQuery }); // Guest public catalog: search, sort, page, days, tags
invoke<WorkshopPreview>('steam_inspect_url', { urlOrId: string });
invoke<string>('steam_download_workshop_item', { urlOrId: string, source?: 'steam' | 'ggnetwork' });
invoke<void>('steam_login_qr');
invoke<void>('steam_login_credentials', { username: string, password: string });
invoke<void>('steam_submit_2fa_code', { code: string });
invoke<void>('steam_logout');
invoke<WallpaperItem[]>('steam_sync_local_folders');
invoke<WallpaperItem[]>('library_list_wallpapers');
invoke<void>('library_delete_wallpaper', { wallpaperId: string });

// --- Monitor, Playback & Property Customization Commands ---
invoke<MonitorInfo[]>('monitors_list');
invoke<void>('monitor_apply_wallpaper', { monitorId: string, source: WallpaperSource });
invoke<void>('monitor_update_property', { wallpaperId: string, propKey: string, propValue: string | number | boolean });
invoke<void>('monitor_update_adjustments', { wallpaperId: string, adjustments: UniversalAdjustments });
invoke<WallpaperPreset>('wallpaper_save_preset', { wallpaperId: string, presetName: string });
invoke<void>('wallpaper_apply_preset', { wallpaperId: string, presetId: string });
invoke<void>('playback_set_paused', { monitorId?: string, paused: boolean });

// --- Playlists, Auto-Updater & Global Settings Commands ---
invoke<Playlist[]>('playlist_list');
invoke<Playlist>('playlist_save', { playlist: Playlist });
invoke<AppSettings>('settings_get');
invoke<void>('settings_update', { settings: Partial<AppSettings> });
invoke<string>('update_feed_status');
invoke<void>('runtime_shutdown_for_update');
invoke<RuntimeMemoryUsage>('runtime_get_memory_usage'); // { residentBytes: number }, main process RAM only
```

#### Rust $\to$ Frontend Event Channels (`app.emit(...)`)
- `wallpaper://changed`: `{ monitorId: string, source: WallpaperSource, currentItem: WallpaperItem | PhotoItem | null, transitionUsed: TransitionType }`
- `photo-queue://updated`: `PhotoStatus`
- `steam-download://progress`: `{ workshopId: string, title: string, progressPercent: number, state: 'downloading' | 'completed' | 'error', error?: string }`
- `steam-auth://qr-matrix`: `{ matrix: boolean[][] }`
- `steam-auth://need-2fa`: `{ kind: 'totp' | 'email', prompt: string }`
- `policy://state-changed`: `{ monitorId: string, state: 'playing' | 'paused_fullscreen' | 'paused_maximized' | 'paused_battery' | 'paused_display_sleep' }`
- `system-power`: `{ kind: 'pause' | 'resume', reason: 'sleep' | 'display' | 'lock' }`

---

## 6. Complete `src-tauri/tauri.conf.json` & `src-tauri/capabilities/default.json` Configuration

### 6.1 `src-tauri/tauri.conf.json` (Aligned with `TruongFlix`)

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "TruongPaper",
  "version": "1.0.0",
  "identifier": "com.iantruong.truongpaper",
  "build": {
    "beforeDevCommand": "npm run bundle:depotdownloader && npm run dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "npm run bundle:depotdownloader && npm run build",
    "frontendDist": "../dist"
  },
  "app": {
    "withGlobalTauri": false,
    "macOSPrivateApi": true,
    "windows": [
      {
        "label": "main",
        "title": "TruongPaper",
        "url": "index.html",
        "width": 1280,
        "height": 820,
        "minWidth": 980,
        "minHeight": 640,
        "decorations": false,
        "transparent": true,
        "shadow": true,
        "center": true,
        "theme": "Dark"
      },
      {
        "label": "tray-popover",
        "title": "TruongPaper Quick Controls",
        "url": "tray.html",
        "width": 360,
        "height": 480,
        "resizable": false,
        "decorations": false,
        "transparent": true,
        "alwaysOnTop": true,
        "skipTaskbar": true,
        "visible": false,
        "theme": "Dark"
      }
    ],
    "security": {
      "csp": "default-src 'self' ipc: http://ipc.localhost; img-src 'self' asset: http://asset.localhost https://*.iantruongphotography.com https://*.cloudfront.net https://*.steamstatic.com https://*.akamaihd.net data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost https://photos-api.iantruongphotography.com https://iantruongphotography.com https://api.steampowered.com",
      "assetProtocol": {
        "enable": true,
        "scope": [
          "$APPDATA/TruongPaper/**",
          "$HOME/Library/Application Support/com.iantruong.truongpaper/**"
        ]
      }
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "category": "GraphicsAndDesign",
    "shortDescription": "Dynamic desktop wallpaper engine & Ian Truong Photography API client",
    "externalBin": [
      "bin/DepotDownloader"
    ],
    "resources": {
      "bin/licenses/": "sidecar-licenses/"
    },
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ],
    "createUpdaterArtifacts": true,
    "macOS": {
      "signingIdentity": "-",
      "entitlements": "./Entitlements.plist"
    }
  },
  "plugins": {
    "updater": {
      "pubkey": "REPLACE_WITH_MINISIGN_PUBKEY_FROM_TAURI_SIGNER_GENERATE",
      "endpoints": [
        "https://github.com/iant4093/TruongPaper/releases/latest/download/latest.json"
      ]
    }
  }
}
```

### 6.2 `src-tauri/capabilities/default.json`

> [!IMPORTANT]
> Tauri v2 blocks window dragging (`data-tauri-drag-region`), file dialogs, autostart, auto-updater, process restart, and sidecar execution unless explicitly listed in `capabilities/default.json` for **both** `"main"` and `"tray-popover"` windows!

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Default capabilities for TruongPaper Main Studio and Tray Popover windows",
  "windows": ["main", "tray-popover"],
  "permissions": [
    "core:default",
    "core:window:allow-start-dragging",
    "core:window:allow-minimize",
    "core:window:allow-unminimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-close",
    "core:window:allow-hide",
    "core:window:allow-show",
    "core:window:allow-set-focus",
    "dialog:default",
    "dialog:allow-open",
    "dialog:allow-save",
    "dialog:allow-message",
    "dialog:allow-ask",
    "autostart:default",
    "updater:default",
    "process:allow-restart",
    "shell:default",
    "shell:allow-open",
    {
      "identifier": "shell:allow-execute",
      "allow": [
        {
          "name": "bin/DepotDownloader",
          "sidecar": true,
          "args": true
        }
      ]
    }
  ]
}
```
