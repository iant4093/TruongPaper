//! The bundled clean-room core assets (`src-tauri/resources/core-assets/assets`): every stock
//! effect resolves through the VFS, and every shader translates through the same preprocessor +
//! `naga` path the renderer uses, with default combos and with each combo option switched on.
mod common;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use common::TempDir;
use truongpaper_renderer::json::Value;
use truongpaper_renderer::scene_assets::{shader_stage_paths, EffectFile, MaterialFile};
use truongpaper_renderer::shader_preprocessor::{
    preprocess_we_shader_pair, translate_we_shader_pair,
};
use truongpaper_we_formats::WeVirtualFileSystem;

fn bundled_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/core-assets/assets")
}

/// A VFS with an empty wallpaper, no real Wallpaper Engine assets, and the bundled tree.
fn vfs(t: &TempDir) -> WeVirtualFileSystem {
    WeVirtualFileSystem::mount(t.path().join("wall"), t.path().join("no-core"))
        .with_bundled_assets_dir(Some(bundled_dir()))
}

fn effect_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(bundled_dir().join("effects"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("effect.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `(combo, option values)` declared by `// [COMBO]` lines in the given sources.
fn declared_combos(sources: &[&str]) -> Vec<(String, BTreeSet<i32>)> {
    let mut out: Vec<(String, BTreeSet<i32>)> = Vec::new();
    for line in sources.iter().flat_map(|s| s.lines()) {
        let Some(json) = line.trim().strip_prefix("// [COMBO]") else {
            continue;
        };
        let v = Value::parse(json.trim()).expect("combo json");
        let name = v.get("combo").and_then(Value::as_str).unwrap().to_string();
        let mut values: BTreeSet<i32> = [0, 1].into();
        if let Some(Value::Object(options)) = v.get("options") {
            values = options
                .iter()
                .filter_map(|(_, o)| o.as_i64().map(|n| n as i32))
                .collect();
        }
        if v.get("type").and_then(Value::as_str) == Some("imageblending") {
            values = [0, 2, 9, 12].into();
        }
        out.push((name, values));
    }
    out
}

fn translate(vfs: &WeVirtualFileSystem, shader: &str, combos: &HashMap<String, i32>) {
    let (vpath, fpath) = shader_stage_paths(shader);
    let (v, f) = preprocess_we_shader_pair(vfs, &vpath, &fpath, combos, None, &[true; 8])
        .unwrap_or_else(|e| panic!("{shader}: preprocess: {e}"));
    translate_we_shader_pair(shader, &v, &f)
        .unwrap_or_else(|e| panic!("{shader} with {combos:?}: {e}"));
}

#[test]
fn every_bundled_effect_shader_translates_for_every_combo_option() {
    let t = TempDir::new("bundled-translate");
    let vfs = vfs(&t);
    let names = effect_names();
    assert!(names.len() >= 25, "only {} bundled effects", names.len());
    let mut translated = 0;
    for name in &names {
        let effect_path = format!("effects/{name}/effect.json");
        let effect = EffectFile::from_json_str(&vfs.read_string(&effect_path).unwrap())
            .unwrap_or_else(|e| panic!("{effect_path}: {e:?}"));
        assert!(!effect.passes.is_empty(), "{effect_path} has no passes");
        for pass in &effect.passes {
            let material_path = pass.material.as_deref().expect("material pass");
            let material = MaterialFile::from_json_str(&vfs.read_string(material_path).unwrap())
                .unwrap_or_else(|e| panic!("{material_path}: {e:?}"));
            let mpass = &material.passes[0];
            for texture in mpass.textures.iter().flatten() {
                let path = format!("materials/{texture}.tex");
                assert!(vfs.exists(&path), "{material_path}: missing {path}");
            }
            let (vpath, fpath) = shader_stage_paths(&mpass.shader);
            let sources = [
                vfs.read_string(&vpath).unwrap(),
                vfs.read_string(&fpath).unwrap(),
            ];
            translate(&vfs, &mpass.shader, &mpass.combos);
            translated += 1;
            for (combo, values) in declared_combos(&[&sources[0], &sources[1]]) {
                for value in values {
                    let mut combos = mpass.combos.clone();
                    combos.insert(combo.clone(), value);
                    translate(&vfs, &mpass.shader, &combos);
                    translated += 1;
                }
            }
        }
    }
    eprintln!("{} effects, {translated} shader translations", names.len());
    assert!(translated > 100, "{translated} translations");
}

#[test]
fn bundled_base_image_shaders_translate() {
    let t = TempDir::new("bundled-generic");
    let vfs = vfs(&t);
    for shader in [
        "genericimage",
        "genericimage2",
        "genericimage3",
        "genericimage4",
    ] {
        translate(&vfs, shader, &HashMap::new());
    }
}

#[test]
fn bundled_headers_and_utility_textures_resolve() {
    let t = TempDir::new("bundled-headers");
    let vfs = vfs(&t);
    for header in [
        "common.h",
        "common_blending.h",
        "common_blur.h",
        "common_composite.h",
        "common_fragment.h",
        "common_perspective.h",
        "common_vertex.h",
    ] {
        assert!(vfs.exists(&format!("shaders/{header}")), "{header}");
    }
    for tex in [
        "util/white",
        "util/black",
        "util/noflow",
        "util/noise",
        "util/clouds_256",
        "util/perlin_256",
        "util/flatnormal",
        "effects/waterripplenormal",
        "effects/waterflowphase",
    ] {
        let bytes = vfs.read_bytes(&format!("materials/{tex}.tex")).unwrap();
        let parsed =
            truongpaper_we_formats::parse_tex(&bytes).unwrap_or_else(|e| panic!("{tex}: {e}"));
        assert!(parsed.header.texture_width > 0, "{tex}");
    }
}

#[test]
fn wallpaper_files_still_win_over_bundled_effects() {
    let t = TempDir::new("bundled-priority");
    t.write("wall/shaders/effects/tint.frag", "// wallpaper's own");
    let vfs = vfs(&t);
    assert_eq!(
        vfs.read_string("shaders/effects/tint.frag").unwrap(),
        "// wallpaper's own"
    );
    assert!(vfs
        .read_string("shaders/effects/tint.vert")
        .unwrap()
        .contains("TruongPaper clean-room"));
}

#[test]
fn bundled_particle_sprites_resolve_and_have_shaped_alpha() {
    let t = TempDir::new("bundled-particles");
    let vfs = vfs(&t);
    for tex in [
        "halo",
        "halo_2",
        "halo_3",
        "halo_6",
        "drop",
        "misc/wave",
        "chromaticdot",
        "light/light_shafts_0",
        "fog/fog1",
        "smoke/smoke2",
        "bubbles/bubble3",
        "beam/beam_1",
        "beam/beam_2_fade",
        "nature/snow",
        "nature/rosepetals",
        "light/light_shafts_6",
        "light/light_shafts_8",
        "light/flare_0",
        "light/flare_1",
        "light/flare_6",
        "nature/rain0",
        "nature/rain1",
        "nature/rain5",
        "nature/leaves0",
        "nature/leaves1",
        "nature/leaves7",
        "nature/leaves10",
        "lightning/lightning1",
        "lightning/lightning2",
        "lightning/lightning4",
        "debris/debris0",
        "debris/debris1",
        "debris/debris5",
        "normal_ring",
        "normal_ring_smooth",
        "ring",
        "smoke/smoke2light",
        "star1",
        "sparkle2",
        "dust3",
        "bokeh1",
        "nature/petal2",
        "feather1",
        "nature/snow0",
        "nature/snow5",
        "fire1",
        "ember2",
        "glow3",
    ] {
        let bytes = vfs
            .read_bytes(&format!("materials/particle/{tex}.tex"))
            .unwrap_or_else(|e| panic!("{tex}: {e}"));
        let parsed =
            truongpaper_we_formats::parse_tex(&bytes).unwrap_or_else(|e| panic!("{tex}: {e}"));
        assert!(parsed.header.texture_width >= 32, "{tex}");
        assert!(parsed.header.texture_height >= 32, "{tex}");
    }
}

/// Regression (1444077782, 1823900922): scenes draw one rain sprite 800-1600 px tall, so the
/// bundled streak has to stay a hairline. The old 32 px wide, ~13 %-wide streak turned the
/// additive rain layers into wide white light bars that blew out the whole frame.
#[test]
fn bundled_rain_sprites_are_hairline_streaks() {
    let t = TempDir::new("bundled-rain");
    let vfs = vfs(&t);
    for n in 0..=5 {
        let bytes = vfs
            .read_bytes(&format!("materials/particle/nature/rain{n}.tex"))
            .unwrap();
        let tex = truongpaper_we_formats::parse_tex(&bytes).unwrap();
        let mip = &tex.images[0][0];
        let rgba = truongpaper_we_formats::decode_tex_mipmap_to_rgba8(
            tex.header.format,
            tex.free_image_format,
            mip,
        )
        .unwrap();
        let (w, h) = (mip.width as usize, mip.height as usize);
        let mid = h * 3 / 5;
        let row = &rgba[mid * w * 4..(mid + 1) * w * 4];
        let covered = row.chunks(4).filter(|p| p[3] > 32).count();
        let peak = rgba.chunks(4).map(|p| p[3]).max().unwrap();
        assert!(peak <= 140, "rain{n}: peak alpha {peak} too strong");
        assert!(covered >= 1, "rain{n}: streak vanished");
        assert!(
            covered * 100 <= w * 6,
            "rain{n}: {covered}/{w} texels opaque, wider than a hairline"
        );
    }
}

/// Regression (1851524277, 2282120494, 1959836574, 2269193950, 2887099508): stock light-shaft
/// presets spawn up to 16 huge (350-1000 px) additive sprites. Their texture must stay a faint
/// veil (peak alpha <= ~45 %) or overlapping shafts saturate into glare.
#[test]
fn bundled_light_shafts_are_faint_veils() {
    let t = TempDir::new("bundled-shafts");
    let vfs = vfs(&t);
    for n in 0..=8 {
        let bytes = vfs
            .read_bytes(&format!("materials/particle/light/light_shafts_{n}.tex"))
            .unwrap();
        let tex = truongpaper_we_formats::parse_tex(&bytes).unwrap();
        let mip = &tex.images[0][0];
        let rgba = truongpaper_we_formats::decode_tex_mipmap_to_rgba8(
            tex.header.format,
            tex.free_image_format,
            mip,
        )
        .unwrap();
        let peak = rgba.chunks(4).map(|p| p[3]).max().unwrap();
        assert!(peak > 40, "light_shafts_{n}: vanished (peak {peak})");
        assert!(
            peak <= 115,
            "light_shafts_{n}: peak alpha {peak} too strong"
        );
    }
}

/// Regression (2250930375): the stock `tint` effect leaves BLENDMODE at its default 30. That
/// mode must multiply (white tint = unchanged); falling through to "replace" painted the
/// whole layer in the tint colour.
#[test]
fn blend_mode_30_tint_multiplies_instead_of_replacing() {
    let src = std::fs::read_to_string(bundled_dir().join("shaders/common_blending.h")).unwrap();
    assert!(
        src.contains("mode == 30) result = base * layer"),
        "common_blending.h lacks the tint blend mode"
    );
    let fallback = include_str!("../../truongpaper-we-formats/src/shader_fallbacks/blending.glsl");
    assert!(fallback.contains("mode == 30) result = base * layer"));
}
