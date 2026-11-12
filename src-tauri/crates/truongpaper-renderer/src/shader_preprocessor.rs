//! Wallpaper Engine GLSL shader preprocessor & `naga` transpiler
//! (`docs/03-WALLPAPER-ENGINE-COMPATIBILITY-SPEC.md` §4).
//!
//! Deviations from the doc listing (all required to compile/run against the real crate set):
//! - `serde_json::Value` -> `crate::json::Value` (`serde_json` is not a dependency of this crate).
//! - `wgpu` 23 is built without the `naga-ir` feature, so `wgpu::ShaderSource::Naga` is not
//!   available. `compile_we_shader_pair` validates the `naga` modules and re-emits them as WGSL via
//!   `naga::back::wgsl` (the `wgsl-out` feature is enabled) before `create_shader_module`.
//!   The device-free half lives in [`translate_we_shader_pair`] so it can be tested headless.
//! - Combo `#define`s are emitted in sorted order (the doc iterated a `HashMap`, which made the
//!   output non-deterministic and therefore uncacheable).
//! - HLSL-style type aliases (`float2`, `float4`, ...) in loose uniforms are normalised to GLSL
//!   names when emitted into the std140 block, because the block is emitted *before* the preamble
//!   `#define float2 vec2` lines and would otherwise fail to parse.
//! - `naga::front::glsl` does not support combined `uniform sampler2D` declarations. Each
//!   `uniform sampler2D g_TextureN;` is emitted as `texture2D g_TextureN_WeTex` (set 1, binding N,
//!   same as the doc) plus `sampler g_TextureN_WeSmp` (set 1, binding N + 8) and a
//!   `#define g_TextureN sampler2D(g_TextureN_WeTex, g_TextureN_WeSmp)` so `texture(g_TextureN, uv)`
//!   call sites are unchanged.
//! - Loose-uniform de-duplication compared the bracketed array name against the bare name, so
//!   array uniforms declared in both stages were emitted twice; it now compares bare names.

use std::collections::HashMap;
use std::io::Result as IoResult;
use truongpaper_we_formats::binary::TexFormat;
use truongpaper_we_formats::vfs::WallpaperVfs;

use crate::json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeShaderStage {
    Vertex,
    Fragment,
}

const WE_GLSL_PREAMBLE: &str = r#"#version 330
precision highp float;
#define mul(x, y) ((y) * (x))
#define max(x, y) max(y, x)
#define lerp mix
#define frac fract
#define CAST2(x) (vec2(x))
#define CAST3(x) (vec3(x))
#define CAST4(x) (vec4(x))
#define CAST3X3(x) (mat3(x))
#define float2 vec2
#define float3 vec3
#define float4 vec4
#define int2 ivec2
#define int3 ivec3
#define int4 ivec4
#define saturate(x) (clamp(x, 0.0, 1.0))
#define texSample2D texture
#define texSample2DLod textureLod
#define log10(x) (log2(x) * 0.301029995663981)
#define atan2 atan
#define fmod(x, y) ((x)-(y)*trunc((x)/(y)))
#define ddx dFdx
#define ddy(x) dFdy(-(x))
#define GLSL 1
#define sample we_sample
"#;

const LIGHTING_V1_STUB: &str = r#"
vec3 PerformLighting_V1(vec3 worldPos, vec3 albedo, vec3 normal, vec3 viewDir,
    vec3 specularTint, vec3 baseReflectance, float roughness, float metallic) {
    return vec3(0.0);
}
"#;

pub fn preprocess_we_shader(
    vfs: &WallpaperVfs,
    shader_virtual_path: &str,
    stage: WeShaderStage,
    pass_combos: &HashMap<String, i32>,
    tex0_format: Option<TexFormat>,
    bound_texture_slots: &[bool; 8],
) -> IoResult<String> {
    let raw_source = vfs.read_string(shader_virtual_path)?;
    let combos = shader_combos(
        &[&raw_source],
        pass_combos,
        tex0_format,
        bound_texture_slots,
    );
    prepare_shader_stage(vfs, &raw_source, stage, &combos)
}

pub fn preprocess_we_shader_pair(
    vfs: &WallpaperVfs,
    vertex_path: &str,
    fragment_path: &str,
    pass_combos: &HashMap<String, i32>,
    tex0_format: Option<TexFormat>,
    bound_texture_slots: &[bool; 8],
) -> IoResult<(String, String)> {
    let vertex = vfs.read_string(vertex_path)?;
    let fragment = vfs.read_string(fragment_path)?;
    let combos = shader_combos(
        &[&vertex, &fragment],
        pass_combos,
        tex0_format,
        bound_texture_slots,
    );
    Ok((
        prepare_shader_stage(vfs, &vertex, WeShaderStage::Vertex, &combos)?,
        prepare_shader_stage(vfs, &fragment, WeShaderStage::Fragment, &combos)?,
    ))
}

fn shader_combos(
    sources: &[&str],
    pass_combos: &HashMap<String, i32>,
    tex0_format: Option<TexFormat>,
    bound_texture_slots: &[bool; 8],
) -> HashMap<String, i32> {
    let mut combos = pass_combos.clone();

    // Rule 1: Automatic TEX0FORMAT injection for RG88 (8) and R8 (9)
    if let Some(fmt) = tex0_format {
        match fmt {
            TexFormat::RG88 => {
                combos.entry("TEX0FORMAT".into()).or_insert(8);
            }
            TexFormat::R8 => {
                combos.entry("TEX0FORMAT".into()).or_insert(9);
            }
            _ => {}
        }
    }

    // Parse // [COMBO] JSON lines & Rule 2: Sampler comment combo auto-activation
    for line in sources.iter().flat_map(|source| source.lines()) {
        let trimmed = line.trim();
        if let Some(json_str) = trimmed.strip_prefix("// [COMBO]") {
            if let Ok(val) = Value::parse(json_str.trim()) {
                if let (Some(name), Some(def)) = (
                    val.get("combo").and_then(|v| v.as_str()),
                    val.get("default").and_then(|v| v.as_i64()),
                ) {
                    combos.entry(name.to_string()).or_insert(def as i32);
                }
            }
        } else if trimmed.starts_with("uniform sampler2D g_Texture") {
            if let Some((decl, comment)) = trimmed.split_once("//") {
                for (slot, &bound) in bound_texture_slots.iter().enumerate() {
                    if decl.contains(&format!("g_Texture{slot}")) && bound {
                        if let Ok(val) = Value::parse(comment.trim()) {
                            if let Some(combo_name) = val.get("combo").and_then(|v| v.as_str()) {
                                combos.entry(combo_name.to_string()).or_insert(1);
                            }
                        }
                    }
                }
            }
        }
    }
    combos
}

fn prepare_shader_stage(
    vfs: &WallpaperVfs,
    raw_source: &str,
    stage: WeShaderStage,
    combos: &HashMap<String, i32>,
) -> IoResult<String> {
    // Recursively expand #include "..." directives via 4-layer VFS
    let hoisted = hoist_sampler_declarations(raw_source);
    let expanded_body = expand_includes_recursive(vfs, &hoisted, 0)?;

    let mut out = String::with_capacity(expanded_body.len() + 2048);
    out.push_str(WE_GLSL_PREAMBLE);

    let mut sorted_combos: Vec<(&String, &i32)> = combos.iter().collect();
    sorted_combos.sort();
    for (k, v) in sorted_combos {
        out.push_str(&format!("#define {k} {v}\n"));
    }

    match stage {
        WeShaderStage::Vertex => {
            out.push_str("#define attribute in\n#define varying out\n");
        }
        WeShaderStage::Fragment => {
            out.push_str("#define varying in\nout vec4 out_FragColor;\n");
        }
    }

    // Rule 3: Replace #require LightingV1 and gl_FragColor -> out_FragColor
    for line in expanded_body.lines() {
        if line.trim().starts_with("#require LightingV1") {
            out.push_str(LIGHTING_V1_STUB);
            out.push('\n');
        } else if stage == WeShaderStage::Fragment {
            out.push_str(&line.replace("gl_FragColor", "out_FragColor"));
            out.push('\n');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    Ok(out)
}

/// Moves the unconditional `uniform sampler2D` declarations of a shader above its first
/// `#include`. Include headers (e.g. the blur helpers) sample `g_Texture0` themselves, but stock
/// shaders declare their samplers after the `#include` line, which GLSL would reject.
fn hoist_sampler_declarations(source: &str) -> String {
    if !source.contains("#include") {
        return source.to_string();
    }
    let mut depth = 0i32;
    let (mut hoisted, mut rest) = (String::new(), String::new());
    for line in source.lines() {
        let t = line.trim();
        if t.starts_with("#if") {
            depth += 1;
        } else if t.starts_with("#endif") {
            depth -= 1;
        }
        let target = if depth == 0 && t.starts_with("uniform sampler2D") {
            &mut hoisted
        } else {
            &mut rest
        };
        target.push_str(line);
        target.push('\n');
    }
    hoisted.push_str(&rest);
    hoisted
}

fn expand_includes_recursive(vfs: &WallpaperVfs, source: &str, depth: usize) -> IoResult<String> {
    if depth > 16 {
        return Ok(source.to_string());
    }
    let mut result = String::with_capacity(source.len());
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#include") {
            let inc_path = rest
                .trim()
                .trim_matches('"')
                .trim_matches('<')
                .trim_matches('>');
            let full_inc = if inc_path.starts_with("shaders/") {
                inc_path.to_string()
            } else {
                format!("shaders/{inc_path}")
            };
            if let Ok(inc_content) = vfs.read_string(&full_inc) {
                let expanded = expand_includes_recursive(vfs, &inc_content, depth + 1)?;
                result.push_str(&expanded);
                result.push('\n');
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    Ok(result)
}

/// Rule 4: Upgrades `varying vec2 <name>;` in the vertex shader to `varying vec4 <name>;`
/// whenever the fragment shader declares `varying vec4 <name>;`, and wraps bare assignments
/// `<name> = <rhs>;` into `<name> = vec4(<rhs>, 0.0, 1.0);`.
pub fn apply_linked_varying_compatibility(vert_src: &str, frag_src: &str) -> String {
    let mut frag_vec4_varyings = Vec::new();
    for line in frag_src.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed
            .strip_prefix("varying vec4 ")
            .or_else(|| trimmed.strip_prefix("in vec4 "))
        {
            if let Some(name) = rest.trim_end_matches(';').split_whitespace().next() {
                frag_vec4_varyings.push(name.to_string());
            }
        }
    }

    if frag_vec4_varyings.is_empty() {
        return vert_src.to_string();
    }

    let mut upgraded_names = Vec::new();
    let mut out_lines = Vec::new();

    for line in vert_src.lines() {
        let trimmed = line.trim();
        let mut replaced_decl = false;
        for var_name in &frag_vec4_varyings {
            let decl_varying = format!("varying vec2 {var_name};");
            let decl_out = format!("out vec2 {var_name};");
            if trimmed == decl_varying {
                out_lines.push(line.replace(&decl_varying, &format!("varying vec4 {var_name};")));
                upgraded_names.push(var_name.clone());
                replaced_decl = true;
                break;
            } else if trimmed == decl_out {
                out_lines.push(line.replace(&decl_out, &format!("out vec4 {var_name};")));
                upgraded_names.push(var_name.clone());
                replaced_decl = true;
                break;
            }
        }
        if replaced_decl {
            continue;
        }

        let mut modified_line = line.to_string();
        for var_name in &upgraded_names {
            if let Some(eq_idx) = trimmed.find('=') {
                let lhs = trimmed[..eq_idx].trim();
                let rhs = trimmed[eq_idx + 1..].trim().trim_end_matches(';').trim();
                if lhs == var_name && !rhs.starts_with("vec4(") && !rhs.starts_with("CAST4(") {
                    modified_line = format!("    {var_name} = vec4({rhs}, 0.0, 1.0);");
                    break;
                }
            }
        }
        out_lines.push(modified_line);
    }

    out_lines.join("\n")
}

/// Rule 5: When the fragment shader declares `varying vec4 v_TexCoord;` (or `vec3`)
/// and performs `v_TexCoord + CAST2(...)`, rewrites bare `v_TexCoord` in binary vec2 arithmetic to `v_TexCoord.xy`.
pub fn apply_fragment_texcoord_compatibility(frag_src: &str) -> String {
    let is_wide_texcoord = frag_src.lines().any(|l| {
        let t = l.trim();
        t.starts_with("varying vec4 v_TexCoord")
            || t.starts_with("varying vec3 v_TexCoord")
            || t.starts_with("in vec4 v_TexCoord")
            || t.starts_with("in vec3 v_TexCoord")
    });

    if !is_wide_texcoord {
        return frag_src.to_string();
    }

    let replaced = frag_src
        .replace("v_TexCoord + CAST2(", "v_TexCoord.xy + CAST2(")
        .replace("v_TexCoord - CAST2(", "v_TexCoord.xy - CAST2(")
        .replace("v_TexCoord * CAST2(", "v_TexCoord.xy * CAST2(")
        .replace("v_TexCoord + vec2(", "v_TexCoord.xy + vec2(")
        .replace("v_TexCoord - vec2(", "v_TexCoord.xy - vec2(");
    truncate_arithmetic_texcoord(&replaced)
}

/// HLSL truncates a wide `v_TexCoord` implicitly when it is an operand of vec2 arithmetic
/// (`(v_TexCoord * 2.0 - 1.0 - center)`); GLSL rejects the mixed widths. Appends `.xy` to every
/// bare `v_TexCoord` that sits directly next to an arithmetic operator.
fn truncate_arithmetic_texcoord(src: &str) -> String {
    const NAME: &str = "v_TexCoord";
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let is_op = |c: char| matches!(c, '*' | '/' | '+' | '-');
    let mut out = String::with_capacity(src.len() + 16);
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("varying") || t.starts_with("in ") || t.starts_with("//") {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut rest = line;
        let mut consumed = String::new();
        while let Some(idx) = rest.find(NAME) {
            let before = &rest[..idx];
            let after = &rest[idx + NAME.len()..];
            let word_start = !before.chars().next_back().is_some_and(|c| is_ident(c) || c == '.');
            let word_end = !after.chars().next().is_some_and(is_ident);
            let next = after.trim_start().chars().next();
            let prev = before.trim_end().chars().next_back();
            // `a - b` and unary minus both count; `a-- ` never occurs in shaders.
            let arithmetic = next.is_some_and(is_op) || prev.is_some_and(is_op);
            consumed.push_str(before);
            consumed.push_str(NAME);
            if word_start && word_end && next != Some('.') && arithmetic {
                consumed.push_str(".xy");
            }
            rest = after;
        }
        consumed.push_str(rest);
        out.push_str(&consumed);
        out.push('\n');
    }
    out
}

/// Rule 6: Wallpaper Engine compiles shaders with HLSL-style implicit truncation, so stock
/// shaders assign a wider vector to a narrower one (`v_NoiseCoord = v_TexCoord;` with a `vec2`
/// and a `vec4`). For whole-statement assignments between two declared global vectors, adds the
/// swizzle GLSL needs (`v_NoiseCoord = v_TexCoord.xy;`).
pub fn apply_truncating_assignment_compatibility(src: &str) -> String {
    let mut widths: HashMap<String, usize> = HashMap::new();
    for line in src.lines() {
        let t = line.trim();
        let decl = ["varying ", "uniform ", "in ", "out ", "attribute "]
            .iter()
            .find_map(|p| t.strip_prefix(p));
        let Some(decl) = decl else { continue };
        let decl = decl.split("//").next().unwrap_or(decl);
        let mut parts = decl.split_whitespace();
        let (Some(ty), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let width = match ty {
            "vec2" | "float2" => 2,
            "vec3" | "float3" => 3,
            "vec4" | "float4" => 4,
            _ => continue,
        };
        widths.insert(name.trim_end_matches(';').to_string(), width);
    }
    if widths.is_empty() {
        return src.to_string();
    }
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim();
        let rewritten = t
            .strip_suffix(';')
            .and_then(|stmt| stmt.split_once('='))
            .and_then(|(lhs, rhs)| {
                let (lhs, rhs) = (lhs.trim(), rhs.trim());
                if !ident(lhs) || !ident(rhs) {
                    return None;
                }
                let (&lw, &rw) = (widths.get(lhs)?, widths.get(rhs)?);
                (rw > lw).then(|| format!("{lhs} = {rhs}.{};", &"xyzw"[..lw]))
            });
        match rewritten {
            Some(stmt) => out.push_str(&stmt),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// HLSL converts scalars implicitly (`int hit = step(a, b);`, `uint i = x % 32;` with a float
/// `x`); GLSL (and naga) reject those stores. Wraps the initializer of single-declaration
/// `int` / `uint` / `float` statements in the matching constructor and turns `a % b` inside
/// them into `mod(float(a), float(b))`.
pub fn apply_scalar_conversion_compatibility(src: &str) -> String {
    let mut out = String::with_capacity(src.len() + 64);
    for line in src.lines() {
        out.push_str(&convert_scalar_decl(line).unwrap_or_else(|| line.to_string()));
        out.push('\n');
    }
    out
}

fn convert_scalar_decl(line: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let t = line.trim();
    let (ty, rest) = ["uint ", "int ", "float "]
        .iter()
        .find_map(|p| t.strip_prefix(p).map(|r| (p.trim(), r)))?;
    let (name, init) = rest.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if init.starts_with('=') {
        return None;
    }
    let init = init.trim().strip_suffix(';')?.trim();
    if init.split("//").nth(1).is_some() || init.is_empty() {
        return None;
    }
    let mut depth = 0i32;
    for c in init.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => return None,
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    // Already an explicit conversion of the whole initializer.
    let already = init.starts_with(&format!("{ty}(")) && {
        let mut d = 0i32;
        let mut whole = true;
        for (i, c) in init.char_indices() {
            match c {
                '(' => d += 1,
                ')' => {
                    d -= 1;
                    if d == 0 && i + 1 != init.len() {
                        whole = false;
                    }
                }
                _ => {}
            }
        }
        whole
    };
    if already {
        return None;
    }
    let init = rewrite_modulo(init);
    Some(format!("{}{ty} {name} = {ty}({init});", &line[..indent_len]))
}

/// `a % b` (simple operands) -> `mod(float(a), float(b))`.
fn rewrite_modulo(expr: &str) -> String {
    let mut s = expr.to_string();
    // A few passes cover chained uses; each pass rewrites the leftmost `%`.
    for _ in 0..4 {
        let Some(pos) = s.find('%') else { break };
        let (left, right) = (&s[..pos], &s[pos + 1..]);
        let l = left.trim_end();
        let lstart = if l.ends_with(')') {
            let mut d = 0i32;
            let mut start = 0;
            for (i, c) in l.char_indices().rev() {
                match c {
                    ')' => d += 1,
                    '(' => {
                        d -= 1;
                        if d == 0 {
                            start = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            start
        } else {
            l.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .map_or(0, |i| i + 1)
        };
        let r = right.trim_start();
        let rlen = r
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(r.len());
        if rlen == 0 {
            break;
        }
        s = format!(
            "{}mod(float({}), float({})){}",
            &l[..lstart],
            &l[lstart..],
            &r[..rlen],
            &r[rlen..]
        );
    }
    s
}

#[derive(Debug, Clone)]
pub struct ReflectedUniformField {
    pub name: String,
    pub glsl_type: String,
    pub byte_offset: usize,
    pub byte_size: usize,
}

pub struct CompiledWeShaderPair {
    pub vertex_module: wgpu::ShaderModule,
    pub fragment_module: wgpu::ShaderModule,
    pub uniform_fields: Vec<ReflectedUniformField>,
    pub uniform_buffer_size: usize,
}

/// Device-free output of the GLSL -> `naga` translation step (validated IR + emitted WGSL).
#[derive(Debug)]
pub struct TranslatedWeShaderPair {
    /// `#version 450` GLSL actually fed to `naga::front::glsl` (useful for diagnostics).
    pub vertex_glsl450: String,
    pub fragment_glsl450: String,
    pub vertex_naga: naga::Module,
    pub fragment_naga: naga::Module,
    /// WGSL re-emitted from the validated `naga` modules (what gets handed to `wgpu`).
    pub vertex_wgsl: String,
    pub fragment_wgsl: String,
    pub uniform_fields: Vec<ReflectedUniformField>,
    pub uniform_buffer_size: usize,
}

/// Applies Rules 4 and 5, assigns deterministic locations, packs loose non-sampler uniforms into
/// a std140 UBO at (set = 0, binding = 0), splits each `sampler2D g_TextureN` into a
/// `texture2D` at (set = 1, binding = N) and a `sampler` at (set = 1, binding = N + 8),
/// parses both stages via `naga::front::glsl::Frontend`, validates them with
/// `naga::valid::Validator`, and re-emits WGSL. Pure CPU; no `wgpu::Device` required.
pub fn translate_we_shader_pair(
    label: &str,
    raw_vert_preprocessed: &str,
    raw_frag_preprocessed: &str,
) -> Result<TranslatedWeShaderPair, String> {
    let linked_vert =
        apply_linked_varying_compatibility(raw_vert_preprocessed, raw_frag_preprocessed);
    let linked_frag = apply_fragment_texcoord_compatibility(raw_frag_preprocessed);
    let linked_vert = apply_truncating_assignment_compatibility(&linked_vert);
    let linked_frag = apply_truncating_assignment_compatibility(&linked_frag);
    let linked_vert = apply_scalar_conversion_compatibility(&linked_vert);
    let linked_frag = apply_scalar_conversion_compatibility(&linked_frag);

    let (vert_450, frag_450, uniform_fields, ubo_size) =
        upgrade_to_naga_glsl450(&linked_vert, &linked_frag);

    let mut frontend = naga::front::glsl::Frontend::default();

    let vert_naga = frontend
        .parse(
            &naga::front::glsl::Options::from(naga::ShaderStage::Vertex),
            &vert_450,
        )
        .map_err(|errs| format!("Vertex GLSL parse error in {label}: {errs:?}"))?;

    let frag_450 = drop_unwritten_fragment_inputs(&frag_450, &vert_naga);
    let frag_naga = frontend
        .parse(
            &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
            &frag_450,
        )
        .map_err(|errs| format!("Fragment GLSL parse error in {label}: {errs:?}"))?;

    let vertex_wgsl = naga_module_to_wgsl(&vert_naga)
        .map_err(|e| format!("Vertex shader validation error in {label}: {e}"))?;
    let fragment_wgsl = naga_module_to_wgsl(&frag_naga)
        .map_err(|e| format!("Fragment shader validation error in {label}: {e}"))?;

    Ok(TranslatedWeShaderPair {
        vertex_glsl450: vert_450,
        fragment_glsl450: frag_450,
        vertex_naga: vert_naga,
        fragment_naga: frag_naga,
        vertex_wgsl,
        fragment_wgsl,
        uniform_fields,
        uniform_buffer_size: ubo_size.max(16),
    })
}

/// Stock shaders leave fragment varyings behind that the vertex stage does not write (unused
/// leftovers such as `v_Scroll`, or ones the vertex stage only declares under an inactive combo).
/// A pipeline rejects such an unmatched input, so each becomes a zero-initialised private global.
fn drop_unwritten_fragment_inputs(frag_450: &str, vert: &naga::Module) -> String {
    let written: std::collections::HashSet<u32> = vert
        .entry_points
        .iter()
        .filter_map(|ep| ep.function.result.as_ref())
        .flat_map(|result| {
            let mut locations = Vec::new();
            let mut push = |binding: &Option<naga::Binding>| {
                if let Some(naga::Binding::Location { location, .. }) = binding {
                    locations.push(*location);
                }
            };
            push(&result.binding);
            if let naga::TypeInner::Struct { members, .. } = &vert.types[result.ty].inner {
                members.iter().for_each(|m| push(&m.binding));
            }
            locations
        })
        .collect();
    let mut out = String::with_capacity(frag_450.len());
    for line in frag_450.lines() {
        let unwritten = line
            .trim()
            .strip_prefix("layout(location = ")
            .and_then(|rest| rest.split_once(") in "))
            .and_then(|(loc, decl)| {
                let loc: u32 = loc.trim().parse().ok()?;
                (!written.contains(&loc)).then_some(decl)
            });
        match unwritten {
            Some(decl) => out.push_str(decl),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Validates a `naga::Module` and writes it back out as WGSL.
pub fn naga_module_to_wgsl(module: &naga::Module) -> Result<String, String> {
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(module)
    .map_err(|e| format!("{:?}", e.into_inner()))?;
    let wgsl =
        naga::back::wgsl::write_string(module, &info, naga::back::wgsl::WriterFlags::empty())
            .map_err(|e| e.to_string())?;
    // `wgpu` re-parses the WGSL, and WGSL is stricter than the GLSL-derived IR (for example
    // uniform array strides), so check the emitted text the same way before handing it over.
    let reparsed = naga::front::wgsl::parse_str(&wgsl).map_err(|e| e.emit_to_string(&wgsl))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&reparsed)
    .map_err(|e| format!("{:?}", e.into_inner()))?;
    Ok(wgsl)
}

/// Translates via [`translate_we_shader_pair`] and creates the two `wgpu::ShaderModule`s.
pub fn compile_we_shader_pair(
    device: &wgpu::Device,
    label: &str,
    raw_vert_preprocessed: &str,
    raw_frag_preprocessed: &str,
) -> Result<CompiledWeShaderPair, String> {
    let translated = translate_we_shader_pair(label, raw_vert_preprocessed, raw_frag_preprocessed)?;

    let vertex_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{label}_vert")),
        source: wgpu::ShaderSource::Wgsl(translated.vertex_wgsl.into()),
    });

    let fragment_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{label}_frag")),
        source: wgpu::ShaderSource::Wgsl(translated.fragment_wgsl.into()),
    });

    Ok(CompiledWeShaderPair {
        vertex_module,
        fragment_module,
        uniform_fields: translated.uniform_fields,
        uniform_buffer_size: translated.uniform_buffer_size,
    })
}

/// Suffix of the `texture2D` half of a split WE `sampler2D` (set 1, binding N).
pub const WE_TEXTURE_SUFFIX: &str = "_WeTex";
/// Suffix of the `sampler` half of a split WE `sampler2D` (set 1, binding N + 8).
pub const WE_SAMPLER_SUFFIX: &str = "_WeSmp";
/// Binding offset of the `sampler` half of `g_TextureN` within set 1.
pub const WE_SAMPLER_BINDING_OFFSET: u32 = 8;

fn upgrade_to_naga_glsl450(
    vert_src: &str,
    frag_src: &str,
) -> (String, String, Vec<ReflectedUniformField>, usize) {
    let mut uniform_fields: Vec<ReflectedUniformField> = Vec::new();
    let mut current_offset = 0usize;
    let mut varying_locations: HashMap<String, u32> = HashMap::new();
    let mut next_varying_loc = 0u32;

    for src in [vert_src, frag_src] {
        for line in src.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("uniform ") {
                let decl = rest
                    .split("//")
                    .next()
                    .unwrap_or(rest)
                    .trim()
                    .trim_end_matches(';');
                let parts: Vec<&str> = decl.split_whitespace().collect();
                if parts.len() >= 2 && parts[0] != "sampler2D" {
                    let ty = parts[0].to_string();
                    let raw_name = parts[1].split('=').next().unwrap().trim().to_string();
                    let clean_name = raw_name.split('[').next().unwrap().to_string();
                    if !uniform_fields
                        .iter()
                        .any(|f| f.name.split('[').next() == Some(clean_name.as_str()))
                    {
                        let (align, size) = std140_align_and_size(&ty, &raw_name);
                        current_offset = (current_offset + align - 1) & !(align - 1);
                        uniform_fields.push(ReflectedUniformField {
                            name: raw_name,
                            glsl_type: ty,
                            byte_offset: current_offset,
                            byte_size: size,
                        });
                        current_offset += size;
                    }
                }
            }
        }
    }
    let total_ubo_size = (current_offset + 15) & !15;

    // std140 gives every array element a 16-byte stride, which `naga` only accepts for vector
    // elements. Scalar arrays (`float g_AudioSpectrum16Left[16]`) live in the block as
    // `vec4 <name>_WeArr[N]` and are copied into a private `float <name>[N]` at the top of
    // `main`, so indexing and passing the array keep working. Byte layout is unchanged.
    let scalar_arrays: Vec<(String, String, String)> = uniform_fields
        .iter()
        .filter_map(|f| {
            let (name, rest) = f.name.split_once('[')?;
            let len = rest.trim_end_matches(']').trim().to_string();
            let vec = match normalize_glsl_type(&f.glsl_type) {
                "float" => "vec4",
                "int" => "ivec4",
                _ => return None,
            };
            Some((name.to_string(), vec.to_string(), len))
        })
        .collect();

    let rewrite_stage = |src: &str,
                         is_vert: bool,
                         varying_locs: &mut HashMap<String, u32>,
                         next_loc: &mut u32|
     -> String {
        let mut out = String::from("#version 450\n");
        if !uniform_fields.is_empty() {
            out.push_str("layout(std140, set = 0, binding = 0) uniform WePassUniformBlock {\n");
            for f in &uniform_fields {
                let packed = scalar_arrays
                    .iter()
                    .find(|(name, _, _)| f.name.split('[').next() == Some(name.as_str()));
                match packed {
                    Some((name, vec, len)) => {
                        out.push_str(&format!("    {vec} {name}_WeArr[{len}];\n"));
                    }
                    None => out.push_str(&format!(
                        "    {} {};\n",
                        normalize_glsl_type(&f.glsl_type),
                        f.name
                    )),
                }
            }
            out.push_str("};\n");
            for (name, vec, len) in &scalar_arrays {
                let scalar = if vec == "ivec4" { "int" } else { "float" };
                out.push_str(&format!("{scalar} {name}[{len}];\n"));
            }
        }
        let array_init: String = scalar_arrays
            .iter()
            .map(|(name, _, len)| {
                format!(
                    "for (int we_i = 0; we_i < {len}; ++we_i) {{ {name}[we_i] = {name}_WeArr[we_i].x; }}\n"
                )
            })
            .collect();
        let mut main_pending = !array_init.is_empty();
        let mut in_main_signature = false;

        let mut attr_loc = 0u32;
        for line in src.lines() {
            let t = line.trim();
            if t.starts_with("#version")
                || t == "#define attribute in"
                || t == "#define varying out"
                || t == "#define varying in"
            {
                continue;
            }
            if t == "out vec4 out_FragColor;" {
                out.push_str("layout(location = 0) out vec4 out_FragColor;\n");
                continue;
            }
            if let Some(rest) = t.strip_prefix("attribute ") {
                out.push_str(&format!("layout(location = {attr_loc}) in {rest}\n"));
                attr_loc += 1;
                continue;
            }
            if main_pending && (in_main_signature || t.starts_with("void main(")) {
                in_main_signature = true;
                if let Some(brace) = line.find('{') {
                    out.push_str(&line[..=brace]);
                    out.push('\n');
                    out.push_str(&array_init);
                    out.push_str(&line[brace + 1..]);
                    out.push('\n');
                    main_pending = false;
                    in_main_signature = false;
                    continue;
                }
            }
            if let Some(rest) = t.strip_prefix("varying ") {
                let decl = rest.split(';').next().unwrap_or(rest);
                let var_name = decl.split_whitespace().last().unwrap_or("").to_string();
                let loc = *varying_locs.entry(var_name).or_insert_with(|| {
                    let l = *next_loc;
                    *next_loc += 1;
                    l
                });
                let qualifier = if is_vert { "out" } else { "in" };
                out.push_str(&format!("layout(location = {loc}) {qualifier} {rest}\n"));
                continue;
            }
            if let Some(rest) = t.strip_prefix("uniform ") {
                let decl = rest.split("//").next().unwrap_or(rest).trim();
                if let Some(sampler_decl) = decl.strip_prefix("sampler2D ") {
                    let slot = (0..8)
                        .find(|s| decl.contains(&format!("g_Texture{s}")))
                        .unwrap_or(0);
                    // `naga::front::glsl` has no combined `sampler2D` uniform type, so each WE
                    // sampler is split into a `texture2D` at (set 1, binding N) and a `sampler`
                    // at (set 1, binding N + 8); a macro re-combines them at every use site.
                    let name = sampler_decl.trim().trim_end_matches(';').trim();
                    out.push_str(&format!(
                        "layout(set = 1, binding = {slot}) uniform texture2D {name}{WE_TEXTURE_SUFFIX};\n\
                         layout(set = 1, binding = {}) uniform sampler {name}{WE_SAMPLER_SUFFIX};\n\
                         #define {name} sampler2D({name}{WE_TEXTURE_SUFFIX}, {name}{WE_SAMPLER_SUFFIX})\n",
                        slot + WE_SAMPLER_BINDING_OFFSET,
                    ));
                }
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    };

    let v450 = rewrite_stage(
        vert_src,
        true,
        &mut varying_locations,
        &mut next_varying_loc,
    );
    let f450 = rewrite_stage(
        frag_src,
        false,
        &mut varying_locations,
        &mut next_varying_loc,
    );
    (v450, f450, uniform_fields, total_ubo_size)
}

fn std140_align_and_size(glsl_type: &str, raw_name: &str) -> (usize, usize) {
    if let Some(bracket_idx) = raw_name.find('[') {
        let len_str = raw_name[bracket_idx + 1..].trim_end_matches(']');
        let count = len_str.parse::<usize>().unwrap_or(1);
        return (16, 16 * count);
    }
    match glsl_type {
        "float" | "int" | "uint" | "bool" => (4, 4),
        "vec2" | "ivec2" | "float2" => (8, 8),
        "vec3" | "ivec3" | "float3" => (16, 12),
        "vec4" | "ivec4" | "float4" => (16, 16),
        "mat3" => (16, 48),
        "mat4" => (16, 64),
        _ => (16, 16),
    }
}

/// Maps the HLSL-style aliases from the preamble (`float2` .. `int4`) to their GLSL type names.
fn normalize_glsl_type(ty: &str) -> &str {
    match ty {
        "float2" => "vec2",
        "float3" => "vec3",
        "float4" => "vec4",
        "int2" => "ivec2",
        "int3" => "ivec3",
        "int4" => "ivec4",
        other => other,
    }
}

/// Public accessor for the exact preamble prepended to every WE shader (§4.1).
pub fn we_glsl_preamble() -> &'static str {
    WE_GLSL_PREAMBLE
}

/// The `#require LightingV1` replacement stub (§4.2 Rule 3).
pub fn lighting_v1_stub() -> &'static str {
    LIGHTING_V1_STUB
}

/// Metadata carried in the trailing JSON comment of a WE uniform declaration, e.g.
/// `uniform float g_Speed; // {"material":"speed","label":"ui_speed","default":1.0}` or
/// `uniform sampler2D g_Texture1; // {"combo":"MASK","default":"util/white"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct UniformCommentMetadata {
    pub glsl_type: String,
    pub name: String,
    /// `material` key: the material/user-property name this uniform is bound from.
    pub material: Option<String>,
    pub label: Option<String>,
    /// `combo` key (samplers only): combo auto-activated when the slot is bound (Rule 2).
    pub combo: Option<String>,
    /// Raw `default` value (number, `"x y z"` string, texture path, ...).
    pub default: Option<Value>,
    /// The full parsed comment object.
    pub raw: Value,
}

/// Parses every `uniform <type> <name>; // {json}` line in `source`. Lines whose comment is not a
/// JSON object are skipped.
pub fn parse_uniform_comment_metadata(source: &str) -> Vec<UniformCommentMetadata> {
    let mut out = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("uniform ") else {
            continue;
        };
        let Some((decl, comment)) = rest.split_once("//") else {
            continue;
        };
        let Ok(raw) = Value::parse(comment.trim()) else {
            continue;
        };
        if !matches!(raw, Value::Object(_)) {
            continue;
        }
        let decl = decl.trim().trim_end_matches(';').trim();
        let mut parts = decl.split_whitespace();
        let (Some(ty), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        let name = name.split(['=', '[']).next().unwrap_or(name).trim();
        let get_str = |k: &str| raw.get(k).and_then(Value::as_str).map(str::to_string);
        out.push(UniformCommentMetadata {
            glsl_type: ty.to_string(),
            name: name.to_string(),
            material: get_str("material"),
            label: get_str("label"),
            combo: get_str("combo"),
            default: raw.get("default").cloned(),
            raw: raw.clone(),
        });
    }
    out
}

/// One row of the built-in uniform table (§4.3, `CPass::setupUniforms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinUniform {
    pub name: &'static str,
    pub glsl_type: &'static str,
    pub source: &'static str,
}

const fn bu(name: &'static str, glsl_type: &'static str, source: &'static str) -> BuiltinUniform {
    BuiltinUniform {
        name,
        glsl_type,
        source,
    }
}

/// Complete built-in uniform table (§4.3). Rows that the doc collapses (`g_Texture0..7`,
/// `Left`/`Right`, `A` / `B`) are expanded into one entry per uniform name.
pub const WE_BUILTIN_UNIFORMS: &[BuiltinUniform] = &[
    bu(
        "g_Time",
        "float",
        "Elapsed seconds since wallpaper start (scaled by playback speed)",
    ),
    bu(
        "g_Daytime",
        "float",
        "Normalized local time of day [0,1] (0.5 = 12:00)",
    ),
    bu(
        "g_PointerPosition",
        "vec2",
        "Normalized desktop cursor position [0,1] on that monitor",
    ),
    bu(
        "g_PointerPositionLast",
        "vec2",
        "Previous frame cursor position",
    ),
    bu("g_TexelSize", "vec2", "(1/scene_width, 1/scene_height)"),
    bu(
        "g_TexelSizeHalf",
        "vec2",
        "(0.5/scene_width, 0.5/scene_height)",
    ),
    bu("g_Texture0", "sampler2D", "Bound texture unit 0"),
    bu("g_Texture1", "sampler2D", "Bound texture unit 1"),
    bu("g_Texture2", "sampler2D", "Bound texture unit 2"),
    bu("g_Texture3", "sampler2D", "Bound texture unit 3"),
    bu("g_Texture4", "sampler2D", "Bound texture unit 4"),
    bu("g_Texture5", "sampler2D", "Bound texture unit 5"),
    bu("g_Texture6", "sampler2D", "Bound texture unit 6"),
    bu("g_Texture7", "sampler2D", "Bound texture unit 7"),
    bu(
        "g_Texture0Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture1Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture2Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture3Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture4Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture5Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture6Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture7Resolution",
        "vec4",
        "(texture_w, texture_h, image_w, image_h)",
    ),
    bu(
        "g_Texture0Rotation",
        "vec4",
        "Sprite sheet frame rotation/size (w1/tw, w2/tw, h2/th, h1/th)",
    ),
    bu(
        "g_Texture0Translation",
        "vec2",
        "Sprite sheet frame UV offset (frame.x/tw, frame.y/th)",
    ),
    bu("g_ModelViewProjectionMatrix", "mat4", "Pass MVP matrix"),
    bu(
        "g_EffectModelViewProjectionMatrix",
        "mat4",
        "Alias of g_ModelViewProjectionMatrix",
    ),
    bu(
        "g_ModelViewProjectionMatrixInverse",
        "mat4",
        "Inverse of pass MVP matrix",
    ),
    bu("g_ModelMatrix", "mat4", "Model matrix"),
    bu("g_ModelMatrixInverse", "mat4", "Inverse model matrix"),
    bu(
        "g_ViewProjectionMatrix",
        "mat4",
        "Camera view-projection matrix",
    ),
    bu("g_LightAmbientColor", "vec3", "scene.general.ambientcolor"),
    bu(
        "g_LightSkylightColor",
        "vec3",
        "scene.general.skylightcolor",
    ),
    bu("g_Alpha", "float", "Layer alpha"),
    bu("g_UserAlpha", "float", "Layer alpha"),
    bu("g_Brightness", "float", "Layer brightness"),
    bu("g_Color", "vec3", "Layer RGB color"),
    bu("g_CompositeColor", "vec3", "Layer RGB color"),
    bu("g_Color4", "vec4", "Layer RGBA color"),
    bu("g_OrientationUp", "vec3", "(0,1,0)"),
    bu("g_OrientationRight", "vec3", "(1,0,0)"),
    bu("g_OrientationForward", "vec3", "(0,0,1)"),
    bu("g_ViewUp", "vec3", "(0,1,0)"),
    bu("g_ViewRight", "vec3", "(1,0,0)"),
    bu("g_EyePosition", "vec3", "(0,0,1000)"),
    bu(
        "g_RenderVar0",
        "vec4",
        "Particle (trail_length, trail_max_length, trail_min_length, 0)",
    ),
    bu(
        "g_RenderVar1",
        "vec4",
        "Particle (frame_w, frame_h, num_frames, texture_aspect_ratio)",
    ),
    bu(
        "g_AudioSpectrum16Left",
        "float[16]",
        "16-band FFT left (truongpaper-audio)",
    ),
    bu(
        "g_AudioSpectrum16Right",
        "float[16]",
        "16-band FFT right (truongpaper-audio)",
    ),
    bu(
        "g_AudioSpectrum32Left",
        "float[32]",
        "32-band FFT left (truongpaper-audio)",
    ),
    bu(
        "g_AudioSpectrum32Right",
        "float[32]",
        "32-band FFT right (truongpaper-audio)",
    ),
    bu(
        "g_AudioSpectrum64Left",
        "float[64]",
        "64-band FFT left (truongpaper-audio)",
    ),
    bu(
        "g_AudioSpectrum64Right",
        "float[64]",
        "64-band FFT right (truongpaper-audio)",
    ),
];

/// Looks up a built-in uniform by exact name.
pub fn builtin_uniform(name: &str) -> Option<&'static BuiltinUniform> {
    WE_BUILTIN_UNIFORMS.iter().find(|u| u.name == name)
}

#[cfg(test)]
mod compat_tests {
    use super::*;

    #[test]
    fn scalar_declarations_get_explicit_conversions() {
        let out = apply_scalar_conversion_compatibility(
            "\tint hit = step(a, b);\n\tuint f = (g + 1) % RES;\n\tfloat x = max(p, q);\nfloat y = float(z);\nint a = 1, b = 2;\n",
        );
        assert!(out.contains("int hit = int(step(a, b));"), "{out}");
        assert!(out.contains("uint f = uint(mod(float((g + 1)), float(RES)));"), "{out}");
        assert!(out.contains("float x = float(max(p, q));"), "{out}");
        assert!(out.contains("float y = float(z);\n"), "{out}");
        assert!(out.contains("int a = 1, b = 2;"), "{out}");
    }

    #[test]
    fn wide_texcoord_in_arithmetic_is_truncated() {
        let out = apply_fragment_texcoord_compatibility(
            "varying vec4 v_TexCoord;\nvec2 uv = (v_TexCoord * 2.0 - 1.0) / 2.0;\nvec4 c = texSample2D(t, v_TexCoord);\nfloat a = v_TexCoord.z * 2.0;\n",
        );
        assert!(out.contains("(v_TexCoord.xy * 2.0 - 1.0)"), "{out}");
        assert!(out.contains("texSample2D(t, v_TexCoord)"), "{out}");
        assert!(out.contains("v_TexCoord.z * 2.0"), "{out}");
    }
}
