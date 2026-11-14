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
| Particles (sprite and rope renderers) | Supported. Emitters: `boxrandom`; every other emitter name uses the sphere/random-direction fallback. Initializers: `lifetimerandom`, `sizerandom`, `alpharandom`, `colorrandom`, `velocityrandom`, `rotationrandom`, `angularvelocityrandom`. Operators: `movement` (gravity, drag), `angularmovement`, `alphafade`, `sizechange`, `colorchange`, `oscillateposition`, `turbulence`, `vortex`, `controlpointattract`. Control points, sprite-sheet frame animation (`sequencemultiplier`, `animationmode: "randomframe"`), world-space flag and `instanceoverride` are honoured. Any other initializer or operator name is ignored |
| Particle children | `static`, `eventfollow`, `eventspawn` and `eventdeath` child systems render, with a shared instance budget (2048 per scene particle tree, 1024 per `children` entry). A child's `angles` and `probability` are ignored, `scale` is a uniform size multiplier, and burst children emit for 0.1 s |
| Refracting particles | `REFRACT` sprite materials sample the scene behind the sprite through a normal map (`ui_editor_properties_refract_amount`, default 0.05). Each particle draw set keeps its own refraction state |
| Text layers | Supported. Fonts come from the scene package or a VFS path; `systemfont_*` names resolve to a system font (Helvetica/Arial on macOS). Characters the font lacks are drawn from a CJK system font (Hiragino, PingFang, Arial Unicode on macOS; Microsoft YaHei, Yu Gothic, MS Gothic, SimSun on Windows; Noto Sans CJK on Linux) when one is installed |
| Puppet / skeletal animation | Supported for `.mdl` meshes (`MDLV0005`/`0013`/`0021`/`0023`, plus unskinned 20-byte records) with `MDLS` skeletons and `MDLA` animations: CPU bone skinning, animation layers with blending, mirrored playback and puppet-attached child layers at the parent's `MDAT` attachment point. Morph-target files have no documented layout, so morph weights are not read from a scene |
| 3D models | Static `.mdl` meshes render with a perspective camera, a depth buffer and a basic material (`texture * (ambient + lambert)` plus a masked emissive term), with `solidlayer` helper materials. The light is one fixed directional light, plus the scene's ambient and skylight colours. Lights and shadows defined in `scene.json` are not read |
| Embedded video textures (macOS) | Supported |
| Bloom, camera parallax / shake | Supported |
| Sprite sheets, blend modes | Sprite-sheet frame animation, HSL blend modes and blend mode 31 (additive light) and 32 (caustics) are supported |
| Depth effects | `depthparallax` is bundled; scene lights, shadows and true depth-of-field are not supported |

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

Bundled textures (procedurally generated, original artwork):

| Group | Contents |
|---|---|
| Utility and pattern | `util/white`, `black`, `noflow`, `noise`, `clouds_256`, `perlin_256`, `flatnormal`, `uniform_256`; `pattern/voronoi`, `voronoi_local`; `gradient/blend_gradient`, `gradient_fire`, `gradient_iridescent` |
| Effect support | `effects/waterripplenormal`, `effects/waterflowphase` |
| Particle sprites | `halo` (1-8), `glow` (1-3), `sparkle` (1-3), `star` (1-3), `bokeh` (1-3), `dust` (1-3), `ember` (1-3), `fire` (shaped flame sprites), `smoke`, `fog`, `rain` (sprite sheet of short drops), `snow`, `leaves`, `petal`/`rosepetals`, `feather`, `debris`, `bubbles`, `lightning`, `beam`, `light/light_shafts` and `light/flare`, `ring`, `drop`, `sickle`, `chromaticdot`, plus the refraction normal maps (`normal_ring`, `normal_ring_smooth`, `normal_splash`, `normal_pinch_rotate`, `drop_normal`, bubble normals) |

Fonts are not bundled. Text layers use the fonts a scene ships, or system fonts (see Text layers above).

Not yet bundled: `lightshafts` as an effect, audio-responsive variants (`AUDIOPROCESSING`) of `shake`/`pulse`, and 3D/lighting effects.

## Scripts, audio, input

| Feature | Status |
|---|---|
| SceneScript property scripts (`update`, `init`, `applyUserProperties`, `cursorMove/Down/Up/Click`, `scriptProperties`) | Supported (QuickJS sandbox, ES modules). Full property set is sent once after scripts load, then only changed properties. `export var scriptProperties = createScriptProperties()…` works. `import` is limited to the WE modules `WEMath`, `WEVector` and `WEColor` |
| SceneScript APIs | Supported: `engine` (`frametime`, `runtime`, `timeOfDay` as local time, `screenResolution`, `canvasSize`, `userProperties`, `registerAudioBuffers` at 16/32/64, `setTimeout`/`setInterval` and their clear functions, `isDesktopDevice`, `isWallpaper`, `isScreensaver`, `isRunningInEditor`, `isPortrait`, `isLandscape`), `input` (cursor world/screen position, left button), `thisLayer` and `thisScene.getLayer`/`getLayerByID`/`enumerateLayers` (origin, scale, angles, size, color, alpha, visible, text, parent/children, transform matrix), `Vec2`/`Vec3`/`Vec4`/`Mat4`, `shared`, `localStorage` (in memory, per script runtime), `MediaPlaybackEvent` and the other media event classes |
| SceneScript APIs that are accepted but inert | Layer and scene animation (`getAnimation`, `getAnimationLayer`, `getTextureAnimation`: `play`/`pause`/`stop` do nothing, `setFrame`/`getFrame` only store a number), `thisLayer.play`/`stop`/`pause`/`setVolume`, video textures, `thisScene.createLayer` (returns a detached stub), `destroyLayer`, `sortLayer`, `getEffect` (returns `{ visible: true }`), `setParent`, `getBoundingBox` (returns `null`), `engine.openUserShortcut`. The renderer drives animation and sound itself |
| SceneScript limits | 64 MiB memory and 512 KiB native stack per runtime. `init`, events and `applyUserProperties` get a 2 s budget; all scripts together get 3 s at load (later scripts are skipped). A frame step is interrupted after 250 ms. A script slot is disabled after 10 consecutive frames over 50 ms, 3 frames that hit the hard limit, or 3 s of throwing every frame. Script logging is limited to 20 lines per second and 512 characters per line. The per-frame step runs from compiled slots, so a failing script never stops the others |
| Sound layers, audio-reactive uniforms | Supported |
| Cursor events / parallax | Supported |
| Web / Video / Application wallpaper types | Handled outside the scene renderer (Application: Windows only) |

## Known limits (this build)

- The bundled effects are written to the documented interface, not pixel-matched to Wallpaper Engine, so strengths and noise patterns differ slightly from the originals. `scene_probe` uses the bundled tree by default (`PROBE_NO_BUNDLED=1` disables it).
- Particle sprites are bundled stand-ins, not Wallpaper Engine's artwork, and fonts that only ship with Wallpaper Engine are replaced by system fonts.
- `engine.screenResolution` is the monitor size, so scripts that position layers as a percentage of it use that size, as in Wallpaper Engine.

## Render grading vs. Workshop previews (round 12 build)

Offscreen `scene_probe` renders of Workshop scenes (default properties, 120 frames at 1920x1080) were graded by eye against each scene's Workshop preview image. Previews are often promo art, a close crop or another animation frame, so "mostly" means the same scene content with visible differences, and "yes" means the same content with only crop, framing or time-of-day differences.

| Grade | Count |
|---|---|
| yes | 77 |
| mostly | 55 |
| no | 2 |

The gradings come from three sets of 40, 43 and 51 renders (134 rows covering 124 distinct scenes; ten scenes appear in two sets). The last two rounds had no regressions. Scenes are listed by Workshop id.

The two `no` grades:

- `1444077782`: rain streaks are too long and bright, and pink blobs appear at the bottom, where the preview is soft bokeh.
- `2938612768`: a grey background with a small music widget renders, but the main artwork is missing.

Remaining causes, most common first:

1. **Extra or oversized overlay layers.** Clocks, weekday/date text, audio bars, notice panels and extra sprites that the preview does not show. Examples: `3292361861`, `3326873240`, `3151551777`, `3195212886`, `3078285611`, `2983846453`, `3122339805`, `2134765860`, and about 17 scenes in set C (`1921212781`, `2212279721`, `3413921910` and others).
2. **Over-strong rain, light-shaft and particle effects.** `1444077782`, `1823900922`, `1851524277`, `1959836574`, `2282120494`, `2230132661`, `1582915184`, `833227004`, `1994794519`, and particle misplacement in `1039919954`, `1468116360`, `2504353624`, `3152793212`.
3. **Background and lighting differences.** Hazy or dark backgrounds, or a different hair colour or glow: `2419444134`, `973101892`, `2761155250`, `2885492021`, `3118949804`, `3509243656`.
4. **Missing background art or text overlays.** `2938612768`, `2250930375` (edge blur, no badges), `2998757800` (promo text), `3147346398` (text missing).
5. **Framing and crop.** The render shows a wider or different camera view than the preview: `1210752799`, `1364019990`, `1375031246`, `1433247985`, `2405309541`, `3479521040`, `1636394814`.
6. **Wrong layer scale or missing layers.** `1425503532`, `2149068390`, `3577990983`, `3463520581`.
7. **Blocky sampling and stray artifacts.** `3141421197`, `3147346398`, `3146507587` (blocky); `3022080536` (red dot), `3146507587` (red streaks).
8. **Script errors.** The only warnings in the logs: one script on `3047405322` throws every frame and is disabled, a `myLayer/visible` script is skipped in `3326873240` and `3470764447`, and rate-limited script logging drops lines in `3195212886` and `3233141951`.
