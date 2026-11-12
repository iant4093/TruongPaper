//! Headless tests for `shader_preprocessor` (docs/03 §4). No GPU device is created.

mod common;

use common::TempDir;
use std::collections::HashMap;
use truongpaper_renderer::shader_preprocessor::{
    lighting_v1_stub, preprocess_we_shader_pair, we_glsl_preamble, WE_SAMPLER_BINDING_OFFSET,
};
use truongpaper_renderer::*;
use truongpaper_we_formats::{TexFormat, WeVirtualFileSystem};

const NO_SLOTS: [bool; 8] = [false; 8];

fn mount(tmp: &TempDir) -> WeVirtualFileSystem {
    WeVirtualFileSystem::mount(tmp.path().join("wall"), tmp.path().join("core"))
}

fn pp(
    vfs: &WeVirtualFileSystem,
    path: &str,
    stage: WeShaderStage,
    combos: &HashMap<String, i32>,
    tex0: Option<TexFormat>,
    slots: &[bool; 8],
) -> String {
    preprocess_we_shader(vfs, path, stage, combos, tex0, slots).expect("preprocess")
}

#[test]
fn sampler_combo_is_shared_by_both_shader_stages() {
    let tmp = TempDir::new("shared_combo");
    tmp.write(
        "wall/shaders/test.vert",
        "#if MASK\nvarying vec2 v_MaskUv;\n#endif",
    );
    tmp.write(
        "wall/shaders/test.frag",
        "uniform sampler2D g_Texture1; // {\"combo\":\"MASK\"}",
    );
    let mut slots = NO_SLOTS;
    slots[1] = true;
    let (vertex, fragment) = preprocess_we_shader_pair(
        &mount(&tmp),
        "shaders/test.vert",
        "shaders/test.frag",
        &HashMap::new(),
        None,
        &slots,
    )
    .unwrap();
    assert!(vertex.contains("#define MASK 1\n"));
    assert!(fragment.contains("#define MASK 1\n"));
}

// ------------------------------------------------------------------------------------------
// §4.1 preamble
// ------------------------------------------------------------------------------------------

#[test]
fn preamble_has_mul_macro_with_swapped_argument_order() {
    let pre = we_glsl_preamble();
    assert!(pre.starts_with("#version 330\nprecision highp float;\n"));
    // Pitfall #7: HLSL mul(v, M) == GLSL M * v, so the arguments MUST be swapped.
    assert!(pre.contains("#define mul(x, y) ((y) * (x))\n"));
    assert!(!pre.contains("((x) * (y))"));
    for m in [
        "#define max(x, y) max(y, x)",
        "#define lerp mix",
        "#define frac fract",
        "#define CAST2(x) (vec2(x))",
        "#define CAST3X3(x) (mat3(x))",
        "#define float4 vec4",
        "#define int4 ivec4",
        "#define saturate(x) (clamp(x, 0.0, 1.0))",
        "#define texSample2D texture",
        "#define texSample2DLod textureLod",
        "#define log10(x) (log2(x) * 0.301029995663981)",
        "#define atan2 atan",
        "#define fmod(x, y) ((x)-(y)*trunc((x)/(y)))",
        "#define ddx dFdx",
        "#define ddy(x) dFdy(-(x))",
        "#define GLSL 1",
    ] {
        assert!(pre.contains(m), "missing preamble macro: {m}");
    }
}

#[test]
fn preamble_precedes_body_and_stage_defines_are_applied() {
    let tmp = TempDir::new("stage");
    tmp.write(
        "wall/shaders/a.vert",
        "attribute vec3 a_Position;\nvoid main() {}\n",
    );
    tmp.write(
        "wall/shaders/a.frag",
        "void main() { gl_FragColor = vec4(1.0); }\n",
    );
    let vfs = mount(&tmp);

    let v = pp(
        &vfs,
        "shaders/a.vert",
        WeShaderStage::Vertex,
        &HashMap::new(),
        None,
        &NO_SLOTS,
    );
    let mul_at = v.find("#define mul(x, y)").unwrap();
    let body_at = v.find("attribute vec3 a_Position;").unwrap();
    assert!(mul_at < body_at);
    assert!(v.contains("#define attribute in\n#define varying out\n"));
    assert!(!v.contains("out vec4 out_FragColor;"));

    let f = pp(
        &vfs,
        "shaders/a.frag",
        WeShaderStage::Fragment,
        &HashMap::new(),
        None,
        &NO_SLOTS,
    );
    assert!(f.contains("#define varying in\nout vec4 out_FragColor;\n"));
    assert!(f.contains("out_FragColor = vec4(1.0);"));
    assert!(!f.contains("gl_FragColor"));
}

// ------------------------------------------------------------------------------------------
// #include via the 4-layer VFS
// ------------------------------------------------------------------------------------------

#[test]
fn includes_resolve_recursively_through_vfs_layers() {
    let tmp = TempDir::new("include");
    // Layer 4 (core assets) provides common.h which includes a nested header.
    tmp.write(
        "core/shaders/common.h",
        "#include \"common_inner.h\"\nfloat greyscale(vec3 c) { return dot(c, vec3(0.11, 0.59, 0.3)); }\n",
    );
    tmp.write("core/shaders/common_inner.h", "#define INNER_INCLUDED 1\n");
    // Layer 1 (loose wallpaper file) overrides a core header of the same name.
    tmp.write("core/shaders/override.h", "#define OVERRIDE_FROM_CORE 1\n");
    tmp.write(
        "wall/shaders/override.h",
        "#define OVERRIDE_FROM_WALLPAPER 1\n",
    );
    tmp.write(
        "wall/shaders/effect.frag",
        "#include \"common.h\"\n#include <override.h>\n#include \"shaders/override.h\"\n#include \"missing.h\"\nvoid main() {}\n",
    );
    let vfs = mount(&tmp);
    let f = pp(
        &vfs,
        "shaders/effect.frag",
        WeShaderStage::Fragment,
        &HashMap::new(),
        None,
        &NO_SLOTS,
    );

    assert!(f.contains("#define INNER_INCLUDED 1"));
    assert!(f.contains("float greyscale(vec3 c)"));
    assert!(f.find("INNER_INCLUDED").unwrap() < f.find("float greyscale").unwrap());
    assert_eq!(f.matches("#define OVERRIDE_FROM_WALLPAPER 1").count(), 2);
    assert!(!f.contains("OVERRIDE_FROM_CORE"));
    // Unresolvable includes are left in place (naga will report them).
    assert!(f.contains("#include \"missing.h\""));
    assert!(!f.contains("#include \"common.h\""));
}

#[test]
fn self_including_header_terminates() {
    let tmp = TempDir::new("include-loop");
    tmp.write(
        "core/shaders/loop.h",
        "#include \"loop.h\"\nfloat x_loop;\n",
    );
    tmp.write(
        "wall/shaders/a.frag",
        "#include \"loop.h\"\nvoid main() {}\n",
    );
    let vfs = mount(&tmp);
    let f = pp(
        &vfs,
        "shaders/a.frag",
        WeShaderStage::Fragment,
        &HashMap::new(),
        None,
        &NO_SLOTS,
    );
    assert!(f.matches("float x_loop;").count() >= 16);
}

// ------------------------------------------------------------------------------------------
// // [COMBO] parsing and Rules 1-3
// ------------------------------------------------------------------------------------------

#[test]
fn combo_comment_defaults_are_parsed_and_pass_combos_win() {
    let tmp = TempDir::new("combo");
    tmp.write(
        "wall/shaders/a.frag",
        "// [COMBO] {\"material\":\"ui_mode\",\"combo\":\"MODE\",\"type\":\"options\",\"default\":2}\n\
         // [COMBO] {\"combo\":\"BLENDMODE\",\"default\":0}\n\
         // [COMBO] {\"combo\":\"NODEFAULT\"}\n\
         // [COMBO] not json\n\
         void main() {}\n",
    );
    let vfs = mount(&tmp);
    let f = pp(
        &vfs,
        "shaders/a.frag",
        WeShaderStage::Fragment,
        &HashMap::new(),
        None,
        &NO_SLOTS,
    );
    assert!(f.contains("#define MODE 2\n"));
    assert!(f.contains("#define BLENDMODE 0\n"));
    assert!(!f.contains("NODEFAULT 0"));
    // Deterministic (sorted) emission order.
    assert!(f.find("#define BLENDMODE").unwrap() < f.find("#define MODE").unwrap());

    let pass: HashMap<String, i32> = [("MODE".to_string(), 5)].into_iter().collect();
    let f = pp(
        &vfs,
        "shaders/a.frag",
        WeShaderStage::Fragment,
        &pass,
        None,
        &NO_SLOTS,
    );
    assert!(f.contains("#define MODE 5\n"));
    assert!(!f.contains("#define MODE 2\n"));
}

#[test]
fn rule1_tex0format_injected_for_rg88_and_r8_only() {
    let tmp = TempDir::new("rule1");
    tmp.write("wall/shaders/a.frag", "void main() {}\n");
    let vfs = mount(&tmp);
    let none = HashMap::new();
    let path = "shaders/a.frag";
    let st = WeShaderStage::Fragment;
    assert!(pp(&vfs, path, st, &none, Some(TexFormat::RG88), &NO_SLOTS)
        .contains("#define TEX0FORMAT 8\n"));
    assert!(pp(&vfs, path, st, &none, Some(TexFormat::R8), &NO_SLOTS)
        .contains("#define TEX0FORMAT 9\n"));
    assert!(!pp(&vfs, path, st, &none, Some(TexFormat::DXT5), &NO_SLOTS).contains("TEX0FORMAT"));
    assert!(
        !pp(&vfs, path, st, &none, Some(TexFormat::RGBA8888), &NO_SLOTS).contains("TEX0FORMAT")
    );
    assert!(!pp(&vfs, path, st, &none, None, &NO_SLOTS).contains("TEX0FORMAT"));
}

#[test]
fn rule2_sampler_comment_combo_activates_only_when_slot_bound() {
    let tmp = TempDir::new("rule2");
    tmp.write(
        "wall/shaders/a.frag",
        "uniform sampler2D g_Texture0; // {\"material\":\"framebuffer\",\"hidden\":true}\n\
         uniform sampler2D g_Texture1; // {\"combo\":\"MASK\",\"default\":\"util/white\"}\n\
         uniform sampler2D g_Texture2; // {\"combo\":\"NOISE\",\"default\":\"util/noise\"}\n\
         void main() {}\n",
    );
    let vfs = mount(&tmp);
    let mut slots = [false; 8];
    slots[1] = true;
    let f = pp(
        &vfs,
        "shaders/a.frag",
        WeShaderStage::Fragment,
        &HashMap::new(),
        None,
        &slots,
    );
    assert!(f.contains("#define MASK 1\n"));
    assert!(!f.contains("#define NOISE"));
}

#[test]
fn rule3_require_lighting_v1_is_replaced_with_stub() {
    let tmp = TempDir::new("rule3");
    tmp.write(
        "wall/shaders/a.frag",
        "#require LightingV1\nvoid main() { gl_FragColor = vec4(PerformLighting_V1(vec3(0.0), vec3(1.0), vec3(0.0,0.0,1.0), vec3(0.0,0.0,1.0), vec3(1.0), vec3(0.04), 0.5, 0.0), 1.0); }\n",
    );
    let vfs = mount(&tmp);
    let f = pp(
        &vfs,
        "shaders/a.frag",
        WeShaderStage::Fragment,
        &HashMap::new(),
        None,
        &NO_SLOTS,
    );
    assert!(!f.contains("#require"));
    assert!(f.contains(lighting_v1_stub().trim()));
    assert!(f.contains(
        "vec3 PerformLighting_V1(vec3 worldPos, vec3 albedo, vec3 normal, vec3 viewDir,"
    ));
}

// ------------------------------------------------------------------------------------------
// Rules 4 & 5 (pure string rewrites)
// ------------------------------------------------------------------------------------------

#[test]
fn rule4_linked_varying_vec2_upgraded_to_vec4() {
    let vert = "varying vec2 v_TexCoord;\nvarying vec2 v_Other;\nvoid main() {\n    v_TexCoord = a_TexCoord;\n    v_Other = a_TexCoord;\n}";
    let frag = "varying vec4 v_TexCoord;\nvarying vec2 v_Other;\nvoid main() {}";
    let out = apply_linked_varying_compatibility(vert, frag);
    assert!(out.contains("varying vec4 v_TexCoord;"));
    assert!(out.contains("v_TexCoord = vec4(a_TexCoord, 0.0, 1.0);"));
    // Not linked as vec4 in the fragment shader -> untouched.
    assert!(out.contains("varying vec2 v_Other;"));
    assert!(out.contains("v_Other = a_TexCoord;"));

    // Already-vec4 assignment is not double-wrapped; `out` declarations are also handled.
    let vert2 =
        "out vec2 v_TexCoord;\nvoid main() {\n    v_TexCoord = vec4(a_TexCoord, 0.0, 1.0);\n}";
    let out2 = apply_linked_varying_compatibility(vert2, frag);
    assert!(out2.contains("out vec4 v_TexCoord;"));
    assert!(!out2.contains("vec4(vec4("));

    // No vec4 varyings in fragment -> identity.
    assert_eq!(
        apply_linked_varying_compatibility(vert, "void main() {}"),
        vert
    );
}

#[test]
fn rule5_fragment_texcoord_swizzle_fix() {
    let frag = "varying vec4 v_TexCoord;\nvoid main() {\n    vec2 a = v_TexCoord + CAST2(0.1);\n    vec2 b = v_TexCoord - vec2(0.1, 0.2);\n    vec2 c = v_TexCoord * CAST2(2.0);\n}";
    let out = apply_fragment_texcoord_compatibility(frag);
    assert!(out.contains("v_TexCoord.xy + CAST2(0.1)"));
    assert!(out.contains("v_TexCoord.xy - vec2(0.1, 0.2)"));
    assert!(out.contains("v_TexCoord.xy * CAST2(2.0)"));
    assert!(out.contains("varying vec4 v_TexCoord;"));

    let vec3_frag = "in vec3 v_TexCoord;\nvec2 a = v_TexCoord + vec2(1.0);";
    assert!(apply_fragment_texcoord_compatibility(vec3_frag).contains("v_TexCoord.xy + vec2(1.0)"));

    // vec2 texcoords are left alone.
    let vec2_frag = "varying vec2 v_TexCoord;\nvec2 a = v_TexCoord + CAST2(0.1);";
    assert_eq!(apply_fragment_texcoord_compatibility(vec2_frag), vec2_frag);
}

// ------------------------------------------------------------------------------------------
// Uniform comment metadata + builtin table
// ------------------------------------------------------------------------------------------

#[test]
fn uniform_comment_metadata_is_parsed() {
    let src = "uniform float g_Speed; // {\"material\":\"speed\",\"label\":\"ui_speed\",\"default\":1.5}\n\
               uniform vec3 g_Tint; // {\"material\":\"tint\",\"default\":\"1 0.5 0\",\"type\":\"color\"}\n\
               uniform sampler2D g_Texture1; // {\"combo\":\"MASK\",\"default\":\"util/white\"}\n\
               uniform float g_Plain;\n\
               uniform float g_NotJson; // just a comment\n";
    let meta = parse_uniform_comment_metadata(src);
    assert_eq!(meta.len(), 3);
    assert_eq!(meta[0].name, "g_Speed");
    assert_eq!(meta[0].glsl_type, "float");
    assert_eq!(meta[0].material.as_deref(), Some("speed"));
    assert_eq!(meta[0].label.as_deref(), Some("ui_speed"));
    assert_eq!(meta[0].default.as_ref().and_then(|v| v.as_f64()), Some(1.5));
    assert_eq!(
        meta[1].default.as_ref().and_then(|v| v.as_str()),
        Some("1 0.5 0")
    );
    assert_eq!(
        meta[1].raw.get("type").and_then(|v| v.as_str()),
        Some("color")
    );
    assert_eq!(meta[2].name, "g_Texture1");
    assert_eq!(meta[2].combo.as_deref(), Some("MASK"));
}

#[test]
fn builtin_uniform_table_covers_spec_rows() {
    for (name, ty) in [
        ("g_Time", "float"),
        ("g_Daytime", "float"),
        ("g_PointerPosition", "vec2"),
        ("g_TexelSizeHalf", "vec2"),
        ("g_Texture7", "sampler2D"),
        ("g_Texture7Resolution", "vec4"),
        ("g_Texture0Rotation", "vec4"),
        ("g_Texture0Translation", "vec2"),
        ("g_ModelViewProjectionMatrix", "mat4"),
        ("g_EffectModelViewProjectionMatrix", "mat4"),
        ("g_ModelViewProjectionMatrixInverse", "mat4"),
        ("g_ViewProjectionMatrix", "mat4"),
        ("g_Color4", "vec4"),
        ("g_EyePosition", "vec3"),
        ("g_RenderVar1", "vec4"),
        ("g_AudioSpectrum64Right", "float[64]"),
    ] {
        let u = builtin_uniform(name).unwrap_or_else(|| panic!("missing builtin {name}"));
        assert_eq!(u.glsl_type, ty, "{name}");
    }
    let mut names: Vec<_> = WE_BUILTIN_UNIFORMS.iter().map(|u| u.name).collect();
    let n = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), n, "duplicate builtin uniform names");
}

// ------------------------------------------------------------------------------------------
// End-to-end: WE GLSL -> preprocess -> rules 4/5 -> GLSL 450 -> naga glsl-in -> validate -> WGSL
// ------------------------------------------------------------------------------------------

const WE_VERT: &str = "\
uniform mat4 g_ModelViewProjectionMatrix;
uniform float g_Time;
attribute vec3 a_Position;
attribute vec2 a_TexCoord;
varying vec2 v_TexCoord;
void main() {
    gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
    v_TexCoord = a_TexCoord;
}
";

const WE_FRAG: &str = "\
#include \"common.h\"
// [COMBO] {\"material\":\"ui_mode\",\"combo\":\"MODE\",\"type\":\"options\",\"default\":1}
uniform sampler2D g_Texture0; // {\"material\":\"framebuffer\",\"hidden\":true}
uniform sampler2D g_Texture1; // {\"combo\":\"MASK\",\"default\":\"util/white\"}
uniform float g_Time;
uniform float g_Speed; // {\"material\":\"speed\",\"default\":1.0}
uniform float4 g_Color4;
uniform float g_AudioSpectrum16Left[16];
varying vec4 v_TexCoord;
#require LightingV1
void main() {
    vec2 uv = v_TexCoord + CAST2(0.0);
    float4 albedo = texSample2D(g_Texture0, uv);
#if MASK
    albedo.a *= texSample2D(g_Texture1, uv).r;
#endif
#if MODE == 1
    albedo.rgb = lerp(albedo.rgb, CAST3(greyscale(albedo.rgb)), saturate(frac(g_Time * g_Speed)));
#endif
    float m = max(albedo.r, 0.5) + fmod(g_Time, 2.0) + log10(10.0) + atan2(1.0, 2.0);
    albedo.rgb += PerformLighting_V1(vec3(0.0), albedo.rgb, vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, 1.0),
        vec3(1.0), vec3(0.04), 0.5, 0.0) * m * g_AudioSpectrum16Left[3];
    gl_FragColor = albedo * g_Color4;
}
";

fn we_pair_vfs(tmp: &TempDir) -> WeVirtualFileSystem {
    tmp.write(
        "core/shaders/common.h",
        "float greyscale(vec3 c) { return dot(c, vec3(0.11, 0.59, 0.3)); }\n",
    );
    tmp.write("wall/shaders/effect.vert", WE_VERT);
    tmp.write("wall/shaders/effect.frag", WE_FRAG);
    mount(tmp)
}

#[test]
fn we_shader_pair_translates_and_validates_through_naga() {
    let tmp = TempDir::new("e2e");
    let vfs = we_pair_vfs(&tmp);
    let mut slots = [false; 8];
    slots[0] = true;
    slots[1] = true;
    let combos = HashMap::new();
    let vert = pp(
        &vfs,
        "shaders/effect.vert",
        WeShaderStage::Vertex,
        &combos,
        None,
        &slots,
    );
    let frag = pp(
        &vfs,
        "shaders/effect.frag",
        WeShaderStage::Fragment,
        &combos,
        None,
        &slots,
    );
    assert!(frag.contains("#define MASK 1"));
    assert!(frag.contains("#define MODE 1"));

    let t = translate_we_shader_pair("effect", &vert, &frag).unwrap_or_else(|e| panic!("{e}"));

    // Rule 4 applied to the vertex stage, Rule 5 to the fragment stage.
    assert!(t.vertex_glsl450.contains("out vec4 v_TexCoord;"));
    assert!(t
        .vertex_glsl450
        .contains("v_TexCoord = vec4(a_TexCoord, 0.0, 1.0);"));
    assert!(t.fragment_glsl450.contains("v_TexCoord.xy + CAST2(0.0)"));
    assert!(t.vertex_glsl450.starts_with("#version 450\n"));
    assert!(t
        .fragment_glsl450
        .contains("layout(location = 0) out vec4 out_FragColor;"));

    // Varying locations agree across stages.
    assert!(t
        .vertex_glsl450
        .contains("layout(location = 0) out vec4 v_TexCoord;"));
    assert!(t
        .fragment_glsl450
        .contains("layout(location = 0) in vec4 v_TexCoord;"));

    // Samplers split into texture (binding N) + sampler (binding N + 8).
    assert!(t
        .fragment_glsl450
        .contains("layout(set = 1, binding = 1) uniform texture2D g_Texture1_WeTex;"));
    assert!(t.fragment_glsl450.contains(&format!(
        "layout(set = 1, binding = {}) uniform sampler g_Texture1_WeSmp;",
        1 + WE_SAMPLER_BINDING_OFFSET
    )));

    // std140 packing of loose uniforms across both stages (deduplicated).
    let fields: Vec<(&str, usize, usize)> = t
        .uniform_fields
        .iter()
        .map(|f| (f.name.as_str(), f.byte_offset, f.byte_size))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("g_ModelViewProjectionMatrix", 0, 64),
            ("g_Time", 64, 4),
            ("g_Speed", 68, 4),
            ("g_Color4", 80, 16),
            ("g_AudioSpectrum16Left[16]", 96, 256),
        ]
    );
    assert_eq!(t.uniform_buffer_size, 352);
    assert!(t.fragment_glsl450.contains("    vec4 g_Color4;"));

    // Entry points survived and the WGSL re-emission parses again.
    assert_eq!(t.vertex_naga.entry_points.len(), 1);
    assert_eq!(
        t.vertex_naga.entry_points[0].stage,
        naga::ShaderStage::Vertex
    );
    assert_eq!(
        t.fragment_naga.entry_points[0].stage,
        naga::ShaderStage::Fragment
    );
    naga::front::wgsl::parse_str(&t.vertex_wgsl).expect("re-emitted vertex WGSL parses");
    naga::front::wgsl::parse_str(&t.fragment_wgsl).expect("re-emitted fragment WGSL parses");
}

#[test]
fn mul_macro_argument_order_is_required_by_naga() {
    // mat2x4 * vec2 -> vec4 is legal; vec2 * mat2x4 is a type error. With the preamble's
    // swapped `mul`, HLSL-style `mul(v2, m24)` must type-check.
    let frag_ok = format!(
        "{}#define varying in\nout vec4 out_FragColor;\nvoid main() {{\n    mat2x4 m24 = mat2x4(1.0);\n    out_FragColor = mul(vec2(1.0, 0.0), m24);\n}}\n",
        we_glsl_preamble()
    );
    let vert = format!(
        "{}#define attribute in\n#define varying out\nvoid main() {{ gl_Position = vec4(0.0); }}\n",
        we_glsl_preamble()
    );
    translate_we_shader_pair("mul_ok", &vert, &frag_ok).unwrap_or_else(|e| panic!("{e}"));

    // The un-swapped expansion is rejected, proving argument order matters.
    let frag_bad = frag_ok.replace("mul(vec2(1.0, 0.0), m24)", "(vec2(1.0, 0.0) * m24)");
    assert!(translate_we_shader_pair("mul_bad", &vert, &frag_bad).is_err());
}

#[test]
fn invalid_glsl_reports_stage_and_label() {
    let vert = format!(
        "{}void main() {{ gl_Position = vec4(0.0); }}\n",
        we_glsl_preamble()
    );
    let frag = format!(
        "{}out vec4 out_FragColor;\nvoid main() {{ out_FragColor = undefined_fn(); }}\n",
        we_glsl_preamble()
    );
    let err = translate_we_shader_pair("broken", &vert, &frag).unwrap_err();
    assert!(err.contains("Fragment") && err.contains("broken"), "{err}");
}
