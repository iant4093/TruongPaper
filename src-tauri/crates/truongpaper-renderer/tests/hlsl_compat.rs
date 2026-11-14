//! Regression tests for HLSL-isms in Workshop effect shaders that used to skip effect passes:
//! each test feeds a minimal fragment shader through the real preprocessor (with the bundled
//! includes) and `naga`.
mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use common::TempDir;
use truongpaper_renderer::shader_preprocessor::{
    preprocess_we_shader_pair, translate_we_shader_pair,
};
use truongpaper_we_formats::WeVirtualFileSystem;

const VERT: &str = r#"
uniform mat4 g_ModelViewProjectionMatrix;
attribute vec3 a_Position;
attribute vec2 a_TexCoord;
varying vec4 v_TexCoord;
void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord = a_TexCoord.xyxy;
}
"#;

fn bundled_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/core-assets/assets")
}

/// Translates `frag_body` (a full fragment shader) with the stock vertex shader above, or the
/// given one.
fn translate_with(vert: &str, frag: &str) -> Result<(), String> {
    let t = TempDir::new("hlsl-compat");
    t.write("shaders/t.vert", vert);
    t.write("shaders/t.frag", frag);
    let vfs = WeVirtualFileSystem::mount(t.path().to_path_buf(), t.path().join("no-core"))
        .with_bundled_assets_dir(Some(bundled_dir()));
    let (v, f) = preprocess_we_shader_pair(
        &vfs,
        "shaders/t.vert",
        "shaders/t.frag",
        &HashMap::new(),
        None,
        &[true; 8],
    )
    .map_err(|e| e.to_string())?;
    translate_we_shader_pair("t", &v, &f).map(|_| ())
}

fn translate(frag: &str) -> Result<(), String> {
    translate_with(VERT, frag)
}

const FRAG_HEAD: &str = "uniform sampler2D g_Texture0;\nvarying vec4 v_TexCoord;\n";

#[test]
fn blend_mode_macros_from_common_blending_resolve() {
    translate(&format!(
        "#include \"common_blending.h\"\n{FRAG_HEAD}
void main() {{
	vec4 albedo = texSample2D(g_Texture0, v_TexCoord.xy);
	albedo.rgb = BlendOpacity(albedo.rgb, smoothstep(0.7, 1.0, albedo.r), BlendLinearDodge, 0.5);
	albedo.rgb = BlendOpacity(albedo.rgb, vec3(0.2), BlendSoftLight, 0.5);
	gl_FragColor = albedo;
}}"
    ))
    .unwrap();
}

#[test]
fn radial_blur_helpers_from_common_blur_resolve() {
    translate(&format!(
        "#include \"common_blur.h\"\n{FRAG_HEAD}
void main() {{
	gl_FragColor = blurRadial13a(v_TexCoord.xy, vec2(0.5), 0.1) + vec4(blurRadial7(v_TexCoord.xy, vec2(0.5), 0.1), 0.0);
}}"
    ))
    .unwrap();
}

#[test]
fn contrast_saturation_brightness_resolves() {
    translate(&format!(
        "#include \"common.h\"\n{FRAG_HEAD}
void main() {{
	vec4 albedo = texSample2D(g_Texture0, v_TexCoord.xy);
	albedo.rgb = ContrastSaturationBrightness(albedo.rgb, 1.0, 1.1, 0.9);
	gl_FragColor = albedo;
}}"
    ))
    .unwrap();
}

#[test]
fn shader_redefining_a_bundled_helper_does_not_clash() {
    translate(&format!(
        "#include \"common.h\"\n{FRAG_HEAD}
float hash12(vec2 p) {{ return fract(sin(dot(p, vec2(12.9, 78.2))) * 43758.5); }}
void main() {{
	gl_FragColor = vec4(hash12(v_TexCoord.xy));
}}"
    ))
    .unwrap();
}

#[test]
fn user_log10_function_and_main_named_variable() {
    translate(&format!(
        "{FRAG_HEAD}
float log10(float x) {{ return log(x) / 2.302585; }}
void main() {{
	vec2 main = v_TexCoord.xy - 0.5;
	float dist=length(main); dist = pow(dist,.1);
	gl_FragColor = vec4(log10(1.0 + dist));
}}"
    ))
    .unwrap();
}

#[test]
fn numeric_define_with_semicolon_and_stray_endif() {
    translate(&format!(
        "{FRAG_HEAD}
#define mask 1.0;
void main() {{
#if 1
	float a = mask;
#endif
#endif
	gl_FragColor = vec4(a);
}}"
    ))
    .unwrap();
}

#[test]
fn uniform_dependent_and_function_const_globals() {
    translate(&format!(
        "{FRAG_HEAD}
uniform vec4 g_Texture0Resolution;
uniform float u_strength;
const vec2 type = (vec2((g_Texture0Resolution.x / g_Texture0Resolution.y), 1.0) * vec2(u_strength, 1.0));
const vec3 lum = normalize(vec3(1.0, 2.0, 3.0));
vec2 helper(vec2 uv) {{ return uv * type; }}
void main() {{
	gl_FragColor = vec4(helper(v_TexCoord.xy), lum.x, 1.0);
}}"
    ))
    .unwrap();
}

#[test]
fn audio_spectrum_array_alias_and_scalar_texture_sample() {
    translate(&format!(
        "{FRAG_HEAD}
uniform float g_AudioSpectrum16Left[16];
uniform sampler2D g_Texture1;
void main() {{
	float g_AudioSpectrumLeft[] = g_AudioSpectrum16Left;
	uint idx = v_TexCoord.x * 16.0;
	float v = texSample2D(g_Texture1, v_TexCoord.xy);
	int bar = step(v_TexCoord.y, g_AudioSpectrumLeft[idx]);
	vec3 col = bar * vec3(1.0, 0.5, 0.2);
	gl_FragColor = vec4(col, v);
}}"
    ))
    .unwrap();
}

#[test]
fn comparison_and_bool_values_used_as_floats() {
    translate(&format!(
        "{FRAG_HEAD}
void main() {{
	float depth = v_TexCoord.x;
	depth *= (depth < 0.5) * 6.0;
	bool left = v_TexCoord.y < 0.49;
	float bar = 1.0;
	bar *= left;
	bar *= v_TexCoord.x > 0.0 && v_TexCoord.x < 1.0;
	float h = step(0.5, v_TexCoord.x);
	vec2 flags = vec2(h, !h);
	gl_FragColor = vec4(depth, bar, flags);
}}"
    ))
    .unwrap();
}

#[test]
fn mixed_vector_widths_in_builtins_and_operands() {
    translate(&format!(
        "{FRAG_HEAD}
uniform vec4 g_Tint;
void main() {{
	vec4 albedo = texSample2D(g_Texture0, v_TexCoord.xy);
	vec3 other = albedo.rgb * 0.5;
	albedo.rgb = mix(albedo, other, 0.5);
	vec2 edge = vec2(step(abs(floor(v_TexCoord.xy)) + 0.001, 1));
	vec2 da = g_Tint * vec2(2.0, 2.0) * 0.001;
	gl_FragColor = vec4(albedo.rgb, edge.x + da.x);
}}"
    ))
    .unwrap();
}

#[test]
fn scalar_literal_initialising_a_vector_in_a_declaration_list() {
    translate(&format!(
        "{FRAG_HEAD}
void main() {{
	vec2 radial = 0.0, center = (v_TexCoord.xy - 0.5);
	radial += center;
	gl_FragColor = vec4(radial, 0.0, 1.0);
}}"
    ))
    .unwrap();
}

#[test]
fn fragment_may_write_and_narrow_its_varyings() {
    // The fragment declares `vec2 v_TexCoord` against the vertex stage's `vec4`, and writes it.
    translate(
        "uniform sampler2D g_Texture0;\nvarying vec2 v_TexCoord;\nvoid main() {\n\tv_TexCoord.y = 1.0 - v_TexCoord.y;\n\tgl_FragColor = texSample2D(g_Texture0, v_TexCoord);\n}\n",
    )
    .unwrap();
}

#[test]
fn varying_arrays_take_one_location_per_element() {
    // Workshop bloom (2822917890) `light_map.vert`: `v_TexCoord[4]` used to share locations
    // with the next varying (naga `BindingCollision`).
    let vert = "attribute vec3 a_Position;\nattribute vec2 a_TexCoord;\nvarying vec2 v_TexCoord[4];\nvarying float v_Damp;\nuniform vec4 g_Texture0Resolution;\nvoid main() {\n\tv_Damp = 1.0;\n\tgl_Position = vec4(a_Position, 1.0);\n\tvec2 offsets = 1.0 / g_Texture0Resolution.xy;\n\tv_TexCoord[0] = a_TexCoord - offsets;\n\tv_TexCoord[1] = a_TexCoord;\n\tv_TexCoord[2] = a_TexCoord;\n\tv_TexCoord[3] = a_TexCoord + offsets;\n}\n";
    let frag = "uniform sampler2D g_Texture0;\nvarying vec2 v_TexCoord[4];\nvarying float v_Damp;\nvoid main() {\n\tgl_FragColor = (texSample2D(g_Texture0, v_TexCoord[0]) + texSample2D(g_Texture0, v_TexCoord[3])) * v_Damp;\n}\n";
    translate_with(vert, frag).unwrap();
}

#[test]
fn float_loop_bounds_and_float_ternary_conditions() {
    // Workshop blur_gaussian (2822917890), test_shader (2342779250), frame_builder
    // (3573886911) and sharpen_filter (2798696916).
    translate(&format!(
        "{FRAG_HEAD}uniform float u_iterations;\nuniform float u_Min;\nuniform float u_Max;\nuniform float g_AudioSpectrum16Left[16];\n#define INVERT 0\nvoid main() {{\n\tvec4 albedo = CAST4(0.0);\n\tfloat iterations = int(u_iterations);\n\tfor (int i = -iterations; i <= iterations; i++) {{\n\t\talbedo += texSample2D(g_Texture0, v_TexCoord.xy + float(i) * 0.01);\n\t}}\n\tfloat s = 0.0;\n\tfor (int i = u_Min; i < u_Max; i++) {{\n\t\ts += g_AudioSpectrum16Left[i];\n\t}}\n\tfloat mask = texSample2D(g_Texture0, v_TexCoord.xy);\n\tmask = INVERT ? 1 - mask : mask;\n\tfloat outside = mask;\n\tvec4 f = outside ? albedo : CAST4(s);\n\tgl_FragColor = f;\n}}\n"
    ))
    .unwrap();
}
