# Release readiness — 2026-11-10

TruongPaper is a preview implementation. Unit tests and packaged builds do not establish full Wallpaper Engine compatibility. Follow [MANUAL-TESTING.md](MANUAL-TESTING.md) before publishing.

## Completed in this pass

- Replaced the Liquid Glass design with a simple neutral dark theme, flat sidebar, compact import/display toolbar, wallpaper grid, contextual inspector, and bottom playback bar. The tray, settings, dialogs, and shared controls use the same theme.
- Library selection no longer automatically opens an unrelated first item's inspector. Added title/type filtering and keyboard-accessible thumbnail selection separate from Apply.
- Removed unused ambient/refraction components and unused Tailwind/PostCSS/Autoprefixer direct dependencies. Upgraded Vitest to 5.1 and kept Vite on its patched 6.4 line; Node 22.12+ is required. `npm audit` reported zero vulnerabilities after the upgrade.
- Wallpaper webviews cannot fall back to another wallpaper's shared VFS after their mount disappears. Such requests return 404.
- Photo Next/Hide and cloned scheduler draws use the same per-monitor apply locks as Workshop applies, with stable multi-monitor lock ordering and source revalidation after waiting. Scheduler helpers already holding a lock avoid reentrant locking.
- Scene reapplication compares asset metadata revisions as well as paths; normal Workshop file updates reload the scene. Runtime extraction cache changes do not invalidate it. Embedded MP4 caches verify payload contents instead of assuming equal length means equal data.
- Display disconnect/reconnect preserves an in-memory playlist position and restores the current item. Playlist positions still do not persist across app restarts.
- Audio capture has an explicit stop handle and releases the capture backend when no playing audio-reactive wallpaper needs it.
- Local Steam preview files can be served outside app data using narrowly granted per-file asset access, only when the file resolves inside its wallpaper folder.
- System sleep/lock state is separate from explicit user pause in the UI, so a resume event cannot erase a manual pause.
- Startup/runtime logging is initialized for terminal-driven troubleshooting.
- Generated an updater key, restricted the local private key file to mode 0600, configured the public key, and installed the GitHub `TAURI_SIGNING_PRIVATE_KEY` secret. The private key is outside the repository at `~/.tauri/truongpaper.key`; keep a secure backup.
- Pushed the repository and started real macOS/Windows CI. The first Windows build compiled and reached tests; a CRLF-sensitive shader/doc test stopped it. Added LF checkout rules and normalized the test comparison. CI retains caches on failure for follow-up builds.
- CI builds test installers by default. Publishing requires a manual run on `main` with `publish` enabled. No public release is published by an ordinary push.
- Added browser-preview screenshots and a real native macOS photo-library screenshot. README claims were narrowed to match current limitations.

## Verification performed locally

| Check | Result |
|---|---|
| Frontend tests | 149 passed |
| TypeScript / Vite build | Passed |
| Rust workspace tests (`--locked`) | 396 passed |
| Clippy (workspace, all targets, locked) | Passed without warnings |
| Rust formatting | Passed |
| GPU smoke example | Passed on Apple M3 Max; initializes adapter and compiles shaders |
| AVFoundation video probe | Passed; ten 1920×1080 BGRA frames decoded |
| Public random-photo API and album catalog | HTTP 200 with expected response shapes |
| Native macOS app | Opened with real photo data; Next rotated through the backend |
| Browser visual checks | Library, Photography, Workshop, inspector, Displays, Playlists, Settings, download dialog, tray; minimum 980×640 layout checked |

Installer build results and current CI status are available in [GitHub Actions](https://github.com/iant4093/TruongPaper/actions/workflows/ci.yml). Use the latest successful run for the current `main` commit, not the first run or an older artifact.

## Follow-up to the first desktop test

The user observed Fit+Blur edge strips, repeated gray-screen flashing, and the inherited TruongFlix application icon.

- Fill / Center Crop is now the default for fresh settings. Changing photo fit/transition settings updates active photo compositor uniforms immediately.
- macOS desktop surfaces explicitly host a `CAMetalLayer` as the view's root layer, so AppKit does not repaint a default backing layer under the Metal surface.
- Repeated screen-parameter notifications with unchanged display geometry no longer destroy and recreate desktop windows. Windows still rebuilds after Explorer restarts, regardless of geometry.
- Static wallpapers repaint once when rendering resumes after sleep/occlusion/user pause.
- A new charcoal/ivory layered-photo icon is used for macOS, Windows, documentation, sidebar branding, and browser favicon. The macOS tray has its own transparent template mark. The master and exact built-in ImageGen prompt are in `assets/`.
- The user confirmed the corrected test build was stable and filled the screen after minimizing and observing the desktop. Longer sleep/wake and repeated-rotation checks remain in the priority checklist; shader smoke tests alone cannot establish WindowServer stability.

Lock-screen support was also requested for feasibility review. Windows supports still images/snapshots through its personalization API, but not arbitrary live wallpaper on its secure lock screen. macOS needs separate native wallpaper/screensaver integration; the current desktop overlay is not lock-screen support. No lock-screen implementation is included in this build.

## Remaining code limitations

- Windows embedded scene MP4 decoding through Media Foundation/D3D11 is not implemented. Standalone video wallpapers use WebView2.
- Native macOS system-audio capture through a process tap/ScreenCaptureKit is not integrated. The current backend uses a virtual loopback audio device and never falls back to the hardware microphone.
- Sound-layer minimum/maximum repeat times are parsed but not scheduled; random mode plays one selected sound. Pausing the sound-owning display can transfer ownership and restart sound layers.
- Morph targets lack a documented binary layout; caller-supplied vertex deltas are supported.
- A spanning canvas and editable global hotkeys are not implemented. Their UI/README claims were removed or clarified. Applying a photo source to multiple displays does not synchronize individual draws.
- Albums and preset lists still use UI localStorage because corresponding list IPC commands do not exist. Steam login status has no dedicated IPC command.
- Steam logout cannot remove DepotDownloader's independently saved login data.
- Initial scene setup still runs on the render thread and can briefly stall other displays. The same scene on multiple displays still has separate embedded-video decoders.
- Asset revisions use paths, sizes, and modification times rather than hashing every large file. Replacing an asset while preserving all metadata may need a full app restart.
- macOS builds are ad-hoc signed, not Apple-notarized; Windows installers are not Authenticode-signed. The updater signature is a separate integrity mechanism.

## Needs desktop/account testing

Steam QR/password/Steam Guard login and real downloads; core-asset bootstrap; real scene/web/video playback and looping; desktop placement and click-through; Smart Pause, sleep/lock, battery policy; multiple displays; Windows desktop attachment on both old and 24H2+ paths; Explorer restart recovery; and a two-version signed updater installation. The exact steps and report template are in [MANUAL-TESTING.md](MANUAL-TESTING.md).

## Spec deviations

The user's request for a simple Codex-like dark UI supersedes the original Liquid Glass visual specification. Runtime ownership, revision checks, preview scoping, line-ending portability, patched test tooling, and manual release publication are implementation refinements to address observed or reviewed gaps. Existing IPC command names and capability boundaries are preserved. No Rust dependency or Windows feature expansion was introduced in this pass.
