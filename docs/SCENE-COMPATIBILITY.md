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
| Effects (GLSL → WGSL translation) | Partial. Passes whose shader fails to translate are skipped (warnings in diagnostics). Many stock effects (godrays, blur precise, shine, water ripple) rely on core-asset shader includes; without the Wallpaper Engine core assets installed they are skipped and their properties cannot change pixels |
| Particles (sprite, rope) | Supported for common initializers/operators; others ignored |
| Text layers | Supported (system-font fallback when the WE font is not shipped) |
| Puppet / skeletal animation | Partial |
| Embedded video textures (macOS) | Supported |
| Bloom, camera parallax / shake | Supported |
| 3D models, light/shadow, depth effects | Not supported |

## Scripts, audio, input

| Feature | Status |
|---|---|
| SceneScript property scripts (`update`, `init`, `applyUserProperties`, `scriptProperties`) | Supported (QuickJS sandbox). Full property set is sent once after scripts load, then only changed properties. Script bodies must be valid multi-line modules; `export var scriptProperties = createScriptProperties()…` works |
| SceneScript APIs | Partial: layers, engine, input, audio buffers; several `thisScene`/`engine` helpers missing |
| Sound layers, audio-reactive uniforms | Supported |
| Cursor events / parallax | Supported |
| Web / Video / Application wallpaper types | Handled outside the scene renderer (Application: Windows only) |

## Known limits (this build)

- Visual verification of effect-bound properties (blur, rays) requires the Wallpaper Engine core assets; they are installed by the app's bootstrap, not available to the offline `scene_probe` example on a machine without them.
- `engine.screenResolution` is the monitor size, so scripts that position layers as a percentage of it use that size, as in Wallpaper Engine.
