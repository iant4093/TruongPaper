mod common;

use common::TempDir;
use glam::Vec2;
use truongpaper_renderer::{
    fbo_size, fbo_texture_format, json::Value, BlendMode, BloomPlan, BloomStage, EffectChainPlan,
    EffectStep, RenderTargetId, SceneGraph, TextureBinding, BLOOM_WGSL, RT_4_FRAME_BUFFER,
    RT_8_FRAME_BUFFER, RT_BLOOM, RT_SCENE,
};
use truongpaper_we_formats::WeVirtualFileSystem;

fn setup(t: &TempDir) {
    t.write(
        "models/sq.json",
        r#"{ "material": "materials/sq.json", "width": 400, "height": 200 }"#,
    );
    t.write(
        "materials/sq.json",
        r#"{ "passes": [ { "shader": "genericimage2", "blending": "translucent", "textures": [null] } ] }"#,
    );
    for name in ["a1", "a2", "b1", "b2", "blur"] {
        t.write(
            &format!("materials/fx/{name}.json"),
            &format!(r#"{{ "passes": [ {{ "shader": "effects/{name}", "textures": [null] }} ] }}"#),
        );
    }
    t.write(
        "materials/fx/combo.json",
        r#"{ "passes": [ { "shader": "effects/combo", "blending": "additive",
            "combos": { "A": 1, "B": 2 }, "constantshadervalues": { "speed": 1.0, "strength": 0.5 },
            "textures": [null, "util/white", "mask"] } ] }"#,
    );
    t.write(
        "effects/a/effect.json",
        r#"{ "name": "a", "passes": [ { "material": "materials/fx/a1.json" }, { "material": "materials/fx/a2.json" } ] }"#,
    );
    t.write(
        "effects/b/effect.json",
        r#"{ "name": "b", "passes": [ { "material": "materials/fx/b1.json" }, { "material": "materials/fx/b2.json" } ] }"#,
    );
    t.write(
        "effects/named/effect.json",
        r#"{ "name": "named",
             "fbos": [ { "name": "_rt_Half", "scale": 2, "format": "r8" }, { "name": "_rt_Q", "scale": 4, "format": "rg88" } ],
             "passes": [
                { "material": "materials/fx/blur.json", "target": "_rt_Half" },
                { "command": "copy", "source": "previous", "target": "_rt_Q" },
                { "material": "materials/fx/blur.json",
                  "bind": [ { "name": "previous", "index": 0 }, { "name": "_rt_Half", "index": 1 },
                            { "name": "_rt_FullFrameBuffer", "index": 2 } ] }
             ] }"#,
    );
    t.write(
        "effects/combo/effect.json",
        r#"{ "passes": [ { "material": "materials/fx/combo.json" } ] }"#,
    );
}

fn load(t: &TempDir, effects: &str, props: Option<&str>) -> (WeVirtualFileSystem, SceneGraph) {
    setup(t);
    t.write(
        "scene.json",
        &format!(
            r#"{{ "general": {{ "orthogonalprojection": {{ "width": 1920, "height": 1080 }},
                  "bloom": {{ "user": "bloom", "value": true }}, "bloomstrength": 1.5, "bloomthreshold": 0.8 }},
                  "objects": [ {{ "name": "L", "image": "models/sq.json", "effects": {effects} }} ] }}"#
        ),
    );
    let vfs = WeVirtualFileSystem::mount(t.path().to_path_buf(), t.path().join("core"));
    let g = SceneGraph::load(&vfs, "scene.json", props, Vec2::new(1920.0, 1080.0)).unwrap();
    (vfs, g)
}

fn plan(t: &TempDir, effects: &str, props: Option<&str>) -> EffectChainPlan {
    let (vfs, g) = load(t, effects, props);
    EffectChainPlan::build(&vfs, &g.layers[0], &g.user_props)
}

/// `(shader, target, slot0, swaps)` per material pass.
fn pass_summary(p: &EffectChainPlan) -> Vec<(String, RenderTargetId, TextureBinding, bool)> {
    p.passes()
        .map(|p| {
            (
                p.shader.clone(),
                p.target.clone(),
                p.textures[&0].clone(),
                p.swaps,
            )
        })
        .collect()
}

#[test]
fn ping_pong_order_for_two_effects_with_two_passes() {
    let t = TempDir::new("fx-pingpong");
    let p = plan(
        &t,
        r#"[ { "file": "effects/a/effect.json" }, { "file": "effects/b/effect.json" } ]"#,
        None,
    );
    assert_eq!((p.width, p.height), (400, 200));
    assert_eq!(p.ping_pong[0].id, RenderTargetId::FboA);
    assert_eq!((p.ping_pong[1].width, p.ping_pong[1].height), (400, 200));
    assert_eq!(p.ping_pong[1].format, wgpu::TextureFormat::Rgba8Unorm);
    assert_eq!(
        p.steps.first(),
        Some(&EffectStep::DrawBase {
            into: RenderTargetId::FboA
        })
    );
    use RenderTargetId::{FboA, FboB};
    let t = |id| TextureBinding::Target(id);
    assert_eq!(
        pass_summary(&p),
        vec![
            ("effects/a1".into(), FboB, t(FboA), true),
            ("effects/a2".into(), FboA, t(FboB), true),
            ("effects/b1".into(), FboB, t(FboA), true),
            ("effects/b2".into(), FboA, t(FboB), true),
        ]
    );
    assert_eq!(
        p.steps.last(),
        Some(&EffectStep::Composite {
            source: FboA,
            blending: BlendMode::Translucent
        })
    );
    assert_eq!(p.final_source(), &FboA);
    assert!(!p.uses_full_frame_buffer);
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
}

#[test]
fn invisible_and_missing_effects_are_skipped() {
    let t = TempDir::new("fx-invisible");
    let effects = r#"[ { "file": "effects/a/effect.json", "visible": false },
                       { "file": "effects/b/effect.json", "visible": { "user": "fx_b", "value": true } },
                       { "file": "effects/missing/effect.json" } ]"#;
    let p = plan(&t, effects, Some(r#"{ "fx_b": false }"#));
    assert_eq!(p.passes().count(), 0);
    assert_eq!(p.steps.len(), 2); // DrawBase + Composite
    assert_eq!(p.final_source(), &RenderTargetId::FboA);

    let p = plan(&t, effects, None);
    let shaders: Vec<String> = p.passes().map(|p| p.shader.clone()).collect();
    assert_eq!(shaders, ["effects/b1", "effects/b2"]);
    assert!(p
        .warnings
        .iter()
        .any(|w| w.contains("effects/missing/effect.json")));
}

#[test]
fn named_fbo_target_copy_and_full_frame_bind() {
    let t = TempDir::new("fx-named");
    let p = plan(&t, r#"[ { "file": "effects/named/effect.json" } ]"#, None);
    let half = RenderTargetId::Named {
        effect: 0,
        name: "_rt_Half".into(),
    };
    let q = RenderTargetId::Named {
        effect: 0,
        name: "_rt_Q".into(),
    };
    // FBO sizes are layer size / scale, formats mapped.
    assert_eq!(p.fbos.len(), 2);
    assert_eq!(p.fbos[0].id, half);
    assert_eq!((p.fbos[0].width, p.fbos[0].height), (200, 100));
    assert_eq!(p.fbos[0].format, wgpu::TextureFormat::R8Unorm);
    assert_eq!((p.fbos[1].width, p.fbos[1].height), (100, 50));
    assert_eq!(p.fbos[1].format, wgpu::TextureFormat::Rg8Unorm);
    assert_eq!(p.all_fbos().count(), 4);

    // DrawBase, Pass(-> _rt_Half, no swap), Copy(FboA -> _rt_Q), Pass(-> FboB, swap), Composite.
    assert_eq!(p.steps.len(), 5);
    let first = p.passes().next().unwrap();
    assert_eq!(first.target, half);
    assert!(!first.swaps);
    assert_eq!(
        first.textures[&0],
        TextureBinding::Target(RenderTargetId::FboA)
    );
    assert_eq!(
        p.steps[2],
        EffectStep::Copy {
            source: RenderTargetId::FboA,
            target: q
        }
    );
    let second = p.passes().nth(1).unwrap();
    assert_eq!(second.target, RenderTargetId::FboB);
    assert!(second.swaps);
    assert_eq!(
        second.textures[&0],
        TextureBinding::Target(RenderTargetId::FboA)
    );
    assert_eq!(second.textures[&1], TextureBinding::Target(half));
    assert_eq!(
        second.textures[&2],
        TextureBinding::Target(RenderTargetId::FullFrame)
    );
    assert!(p.uses_full_frame_buffer);
    assert_eq!(p.final_source(), &RenderTargetId::FboB);
}

#[test]
fn combos_and_constants_override_merge() {
    let t = TempDir::new("fx-merge");
    let p = plan(
        &t,
        r#"[ { "file": "effects/combo/effect.json", "passes": [ {
               "combos": { "B": 5, "C": 1 },
               "constantshadervalues": { "strength": { "user": "str", "value": 0.7 }, "color": "1 0 0" },
               "textures": [null, null, "custommask"] } ] } ]"#,
        Some(r#"{ "str": 0.9 }"#),
    );
    let pass = p.passes().next().unwrap();
    assert_eq!(pass.combos.get("A"), Some(&1));
    assert_eq!(pass.combos.get("B"), Some(&5));
    assert_eq!(pass.combos.get("C"), Some(&1));
    assert_eq!(pass.constants.get("speed"), Some(&Value::Number(1.0)));
    let strength = pass
        .constants
        .get("strength")
        .and_then(Value::as_f64)
        .unwrap();
    assert!((strength - 0.9).abs() < 1e-6);
    assert_eq!(
        pass.constants.get("color"),
        Some(&Value::String("1 0 0".into()))
    );
    assert_eq!(pass.blending, BlendMode::Additive);
    // Material slot 1, override slot 2 wins over the material's "mask".
    assert_eq!(
        pass.textures[&1],
        TextureBinding::Asset("materials/util/white.tex".into())
    );
    assert_eq!(
        pass.textures[&2],
        TextureBinding::Asset("materials/custommask.tex".into())
    );

    // Without the user property the bound default is used.
    let p = plan(
        &t,
        r#"[ { "file": "effects/combo/effect.json", "passes": [ {
               "constantshadervalues": { "strength": { "user": "str", "value": 0.7 } } } ] } ]"#,
        None,
    );
    let s = p.passes().next().unwrap().constants["strength"]
        .as_f64()
        .unwrap();
    assert!((s - 0.7).abs() < 1e-6);
}

#[test]
fn blend_mode_mapping() {
    use wgpu::BlendFactor as F;
    assert_eq!(BlendMode::parse("normal"), BlendMode::Normal);
    assert_eq!(BlendMode::parse("Translucent"), BlendMode::Translucent);
    assert_eq!(BlendMode::parse("additive"), BlendMode::Additive);
    assert_eq!(BlendMode::parse("multiply"), BlendMode::Multiply);
    assert_eq!(BlendMode::parse("screen"), BlendMode::Screen);
    assert_eq!(BlendMode::parse("weird"), BlendMode::Normal);
    assert_eq!(BlendMode::parse("disabled"), BlendMode::Normal);
    assert_eq!(BlendMode::Normal.to_wgpu_blend(), None);
    let factors = |m: BlendMode| {
        let b = m.to_wgpu_blend().unwrap();
        assert_eq!(b.color, b.alpha);
        assert_eq!(b.color.operation, wgpu::BlendOperation::Add);
        (b.color.src_factor, b.color.dst_factor)
    };
    assert_eq!(
        factors(BlendMode::Translucent),
        (F::SrcAlpha, F::OneMinusSrcAlpha)
    );
    assert_eq!(factors(BlendMode::Additive), (F::SrcAlpha, F::One));
    assert_eq!(factors(BlendMode::Multiply), (F::Dst, F::Zero));
    assert_eq!(factors(BlendMode::Screen), (F::One, F::OneMinusSrc));
}

#[test]
fn fbo_helpers() {
    assert_eq!(fbo_size(400, 200, 2), (200, 100));
    assert_eq!(fbo_size(3, 3, 8), (1, 1));
    assert_eq!(fbo_size(400, 200, 0), (400, 200));
    assert_eq!(
        fbo_texture_format("rgba8888"),
        wgpu::TextureFormat::Rgba8Unorm
    );
    assert_eq!(
        fbo_texture_format("rgba_backbuffer"),
        wgpu::TextureFormat::Rgba8Unorm
    );
    assert_eq!(fbo_texture_format("r8"), wgpu::TextureFormat::R8Unorm);
    assert_eq!(fbo_texture_format("rg88"), wgpu::TextureFormat::Rg8Unorm);
    assert_eq!(fbo_texture_format("?"), wgpu::TextureFormat::Rgba8Unorm);
}

#[test]
fn bloom_plan_sizes_and_settings() {
    let t = TempDir::new("fx-bloom");
    let (_vfs, g) = load(&t, "[]", None);
    let b = BloomPlan::from_general(&g.scene.general, &g.user_props, g.scene_size);
    assert!(b.enabled);
    assert_eq!((b.strength, b.threshold), (1.5, 0.8));
    let summary: Vec<_> = b
        .passes
        .iter()
        .map(|p| (p.stage, p.source, p.target, p.width, p.height, p.blending))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                BloomStage::ThresholdDownsample,
                RT_SCENE,
                RT_4_FRAME_BUFFER,
                480,
                270,
                BlendMode::Normal
            ),
            (
                BloomStage::DownsampleBlur,
                RT_4_FRAME_BUFFER,
                RT_8_FRAME_BUFFER,
                240,
                135,
                BlendMode::Normal
            ),
            (
                BloomStage::Blur,
                RT_8_FRAME_BUFFER,
                RT_BLOOM,
                240,
                135,
                BlendMode::Normal
            ),
            (
                BloomStage::Combine,
                RT_BLOOM,
                RT_SCENE,
                1920,
                1080,
                BlendMode::Additive
            ),
        ]
    );

    // User property turns bloom off.
    let (_vfs, g) = load(&t, "[]", Some(r#"{ "bloom": false }"#));
    let b = BloomPlan::from_general(&g.scene.general, &g.user_props, g.scene_size);
    assert!(!b.enabled);
    assert!(b.passes.is_empty());
}

#[test]
fn bloom_wgsl_validates_with_naga() {
    let module = naga::front::wgsl::parse_str(BLOOM_WGSL)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(BLOOM_WGSL)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .expect("bloom.wgsl validates");
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for stage in [
        BloomStage::ThresholdDownsample,
        BloomStage::DownsampleBlur,
        BloomStage::Blur,
        BloomStage::Combine,
    ] {
        assert!(names.contains(&stage.fragment_entry_point()), "{names:?}");
    }
    assert!(names.contains(&"vs_main"));
}

#[test]
fn unknown_render_targets_skip_the_effect_and_are_reported() {
    let t = TempDir::new("fx-unknown-target");
    t.write(
        "effects/badtarget/effect.json",
        r#"{ "passes": [ { "material": "materials/fx/a1.json", "target": "_rt_Nope" } ] }"#,
    );
    t.write(
        "effects/badbind/effect.json",
        r#"{ "passes": [ { "material": "materials/fx/b1.json" },
                         { "material": "materials/fx/b2.json",
                           "bind": [ { "name": "_rt_Missing", "index": 1 } ] } ] }"#,
    );
    let p = plan(
        &t,
        r#"[ { "file": "effects/a/effect.json" }, { "file": "effects/badtarget/effect.json" },
             { "file": "effects/badbind/effect.json" }, { "file": "effects/missing/effect.json" } ]"#,
        None,
    );
    // Only the good effect's passes survive; the chain is not corrupted by guessed targets.
    let shaders: Vec<String> = p.passes().map(|p| p.shader.clone()).collect();
    assert_eq!(shaders, ["effects/a1", "effects/a2"]);
    assert_eq!(p.final_source(), &RenderTargetId::FboA);
    assert_eq!(
        p.diagnostics,
        [
            "Effect badtarget/effect.json skipped: unknown render target '_rt_Nope'",
            "Effect badbind/effect.json skipped: unknown render target '_rt_Missing'",
            "Effect missing/effect.json skipped: file missing or invalid",
        ]
    );
}

#[test]
fn unknown_blend_modes_are_reported() {
    assert_eq!(BlendMode::try_parse("weird"), None);
    assert_eq!(BlendMode::try_parse("Additive"), Some(BlendMode::Additive));
    let t = TempDir::new("fx-blend");
    t.write(
        "materials/fx/wobble.json",
        r#"{ "passes": [ { "shader": "effects/w", "blending": "wobble", "textures": [null] } ] }"#,
    );
    t.write(
        "effects/wobble/effect.json",
        r#"{ "passes": [ { "material": "materials/fx/wobble.json" } ] }"#,
    );
    let p = plan(&t, r#"[ { "file": "effects/wobble/effect.json" } ]"#, None);
    assert!(p
        .diagnostics
        .iter()
        .any(|d| d == "Effect wobble/effect.json: unknown blend mode 'wobble' treated as normal"));
}

/// Regression (1444077782, 2269193950): pass overrides naming a unique effect FBO as
/// `<fbo>_<kind>_<id>` and another layer's `_b` composite resolve instead of skipping the effect.
#[test]
fn unique_fbo_override_names_and_layer_composite_b_resolve() {
    let t = TempDir::new("fx-unique");
    setup(&t);
    t.write(
        "effects/acc/effect.json",
        r#"{ "fbos": [ { "name": "_rt_Acc", "scale": 1, "unique": true } ],
             "passes": [ { "material": "materials/fx/combo.json" } ] }"#,
    );
    let p = plan(
        &t,
        r#"[ { "file": "effects/acc/effect.json", "passes": [
              { "textures": [ null, "_rt_Acc_fullscreen_90", "_rt_imageLayerComposite_13_b" ] } ] } ]"#,
        None,
    );
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    let pass = p.passes().next().unwrap();
    assert_eq!(
        pass.textures[&1],
        TextureBinding::Target(RenderTargetId::Named {
            effect: 0,
            name: "_rt_Acc".into()
        })
    );
    assert_eq!(
        pass.textures[&2],
        TextureBinding::Target(RenderTargetId::LayerOutput(13))
    );
}

#[test]
fn own_layer_composite_binds_the_current_source() {
    // 3152793212: a CRT effect binds `_rt_imageLayerComposite_<own id>_b` as its input; that is
    // the layer's own chain, not a finished layer output (which is still empty at that point).
    let t = TempDir::new("fx-own-composite");
    setup(&t);
    t.write(
        "scene.json",
        r#"{ "general": { "orthogonalprojection": { "width": 1920, "height": 1080 } },
              "objects": [ { "id": 246, "name": "L", "image": "models/sq.json", "effects": [
                { "file": "effects/combo/effect.json",
                  "passes": [ { "textures": [ "_rt_imageLayerComposite_246_b" ] } ] },
                { "file": "effects/combo/effect.json",
                  "passes": [ { "textures": [ "_rt_imageLayerComposite_13_a" ] } ] } ] } ] }"#,
    );
    let vfs = WeVirtualFileSystem::mount(t.path().to_path_buf(), t.path().join("core"));
    let g = SceneGraph::load(&vfs, "scene.json", None, Vec2::new(1920.0, 1080.0)).unwrap();
    let p = EffectChainPlan::build(&vfs, &g.layers[0], &g.user_props);
    let passes: Vec<_> = p.passes().collect();
    assert_eq!(
        passes[0].textures[&0],
        TextureBinding::Target(RenderTargetId::FboA)
    );
    assert_eq!(
        passes[1].textures[&0],
        TextureBinding::Target(RenderTargetId::LayerOutput(13))
    );
}
