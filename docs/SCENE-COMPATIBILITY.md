# Scene compatibility (Wallpaper Engine `scene.pkg`)

Honest status of TruongPaper's own scene renderer, most commonly used features first.
"Live" = applies immediately when a user property changes; "reload" = the renderer's GPU plans
are rebuilt in place (`SceneRenderer::load`, textures stay cached).

## User-property bindings

| Binding kind | Status |
|---|---|
| `visible`, `alpha`, `brightness`, `color`, `origin`, `scale`, `angles`, `size`, text `pointsize`/`padding`, `text` | Supported, live |
| Condition form `{"user":{"name":..,"condition":".."},"value":..}` | Supported for booleans (visible etc.); bool/number/string property values all compare against the condition string (`true`≙`"1"`). For non-boolean fields a conditional binding resolves to its default |
| Effect `visible` | Supported, reload |
| Effect pass `constantshadervalues` | Supported, reload |
| Scene `general` (bloom, parallax, clear colour, camera) | Supported, live (bloom: reload) |
| Particle `instanceoverride` (`count` ≤1, `rate`, `size`, `alpha`, `lifetime`, `speed`, `brightness`, `colorn` as tint) | Supported, live for newly spawned particles (alive ones age out). `count` >1 is clamped to the preset's `maxcount` |
| SceneScript `scriptproperties` bound to a user property | Supported, live |
| Effect pass `combos`, `textures` bound to user properties | Not supported (never bound in practice) |
| Saved values at load | Merged over `project.json` defaults; bool, slider number, combo, colour `"r g b"` and text are passed in the shape scripts expect |

## Layers and rendering

| Feature | Status |
|---|---|
| Image layers, parenting, transforms, blend modes | Supported |
| Effects (GLSL → WGSL translation) | Supported for the stock effects below and for workshop effects that translate. Passes whose shader still fails to translate are skipped (warnings in diagnostics). Effect passes draw a clip-space quad with an identity model-view-projection, as stock passes expect |
| Particles (sprite, rope) | Supported for common initializers/operators; others ignored |
| Text layers | Supported (system-font fallback when the WE font is not shipped) |
| Puppet / skeletal animation | Partial |
| Embedded video textures (macOS) | Supported |
| Bloom, camera parallax / shake | Supported |
| 3D models, light/shadow, depth effects | Not supported |

## Effects and core assets

Wallpaper Engine scenes reference stock effects and shader includes that ship only with Wallpaper
Engine ("core assets"). TruongPaper bundles its **own clean-room implementations** of them in
`src-tauri/resources/core-assets/assets` (shipped as the `core-assets` app resource). They keep the
public interface — effect folder and material file names, uniform names, material constant keys
(`constantshadervalues`), combo names and include function names — so existing `scene.json`
references keep working, but every shader is original code. No Wallpaper Engine file is copied.

Resolution order in the scene VFS: wallpaper folder → `scene.pkg` → `gifscene.pkg` → real Wallpaper
Engine core assets (if installed) → bundled clean-room assets → built-in virtual files. Workshop
packages normally embed the stock effect shaders they use, so the bundled versions mostly supply the
includes, base image shaders and utility textures; they are used in full when a scene references an
effect it does not ship.

| Bundled | Contents |
|---|---|
| Includes | `common.h` (`rotateVec2`, `greyscale`, `rgb2hsv`/`hsv2rgb`, noise helpers, `M_PI*`), `common_blur.h` (`blur3/5/7/9/13[a]` separable Gaussian), `common_blending.h` (`ApplyBlending`), `common_perspective.h` (`squareToQuad`), `common_fragment.h` |
| Base shaders | `genericimage`, `genericimage2`, `genericimage3`, `genericimage4` |
| Utility textures | `util/white`, `black`, `noflow`, `noise`, `clouds_256`, `perlin_256`, `flatnormal`; `effects/waterripplenormal`, `effects/waterflowphase` (procedural, `scripts/generate-core-asset-textures.mjs`) |
| Multi-pass effects | `godrays` (downsample → cast radial/directional → Gaussian x/y → combine), `shine` (star streaks), `blurprecise`, `blur` |
| Single-pass effects | `opacity`, `tint`, `colorkey`, `filmgrain`, `pulse`, `waterwaves`, `waterripple`, `waterflow`, `shake`, `foliagesway`, `cloudmotion`, `scroll`, `spin`, `swing`, `twirl`, `fisheye`, `iris`, `chromaticaberration`, `vhs`, `nitro`, `xray`, `perspective`, `depthparallax`, `reflection`, `refraction`, `clouds` |

Translation fixes that stock shaders need (all in `shader_preprocessor`): sampler declarations are
hoisted above `#include` so helper headers can sample `g_Texture0`; `sample` is usable as a variable
name; fragment varyings the vertex stage never writes become zeroed private globals; whole-statement
assignments of a wider vector to a narrower one get a swizzle; scalar uniform arrays
(`g_AudioSpectrum16Left[16]`) are packed with a 16-byte stride; emitted WGSL is re-validated before
`wgpu` sees it. A scalar material constant bound to a vector uniform (a slider driving `vec2 scale`)
fills every component.

Not yet bundled: `lightshafts`, audio-responsive variants (`AUDIOPROCESSING`) of `shake`/`pulse`,
3D/lighting effects, particle textures (`materials/particle/*`) and fonts.

## Scripts, audio, input

| Feature | Status |
|---|---|
| SceneScript property scripts (`update`, `init`, `applyUserProperties`, `scriptProperties`) | Supported (QuickJS sandbox). Full property set is sent once after scripts load, then only changed properties. Script bodies must be valid multi-line modules; `export var scriptProperties = createScriptProperties()…` works |
| SceneScript APIs | Partial: layers, engine, input, audio buffers; several `thisScene`/`engine` helpers missing |
| Sound layers, audio-reactive uniforms | Supported |
| Cursor events / parallax | Supported |
| Web / Video / Application wallpaper types | Handled outside the scene renderer (Application: Windows only) |

## Known limits (this build)

- The bundled effects are written to the documented interface, not pixel-matched to Wallpaper Engine, so strengths and noise patterns differ slightly from the originals. `scene_probe` uses the bundled tree by default (`PROBE_NO_BUNDLED=1` disables it).
- Particle textures and fonts that only ship with Wallpaper Engine are still replaced by fallbacks.
- `engine.screenResolution` is the monitor size, so scripts that position layers as a percentage of it use that size, as in Wallpaper Engine.
