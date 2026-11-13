//! Source-level compatibility fixes for the HLSL-flavoured GLSL that Wallpaper Engine shaders are
//! written in. Wallpaper Engine's own compiler is lenient about things `naga`'s GLSL front end
//! rejects; this module rewrites the offending constructs before translation:
//!
//! - user functions that share a name with one of our preamble macros (`float log10(float x)`);
//! - a local variable named `main`;
//! - object-like numeric macros with a trailing semicolon (`#define mask 1.0;`);
//! - unsized local array aliases (`float spectrum[] = g_AudioSpectrum16Left;`);
//! - `const` declarations that depend on uniforms, which GLSL treats as non-constant;
//! - arithmetic on comparison results (`(depth < limit) * 6.0`), which HLSL converts to float;
//! - mixed-width arguments to `mix` / `step` / `clamp` / ... (HLSL truncates to the shortest
//!   vector), through an overload set that is only added when the plain translation fails.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

macro_rules! re {
    ($pattern:expr) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pattern).expect("valid regex"))
    }};
}

/// Preamble macros that a shader may legitimately re-define as functions.
const PREAMBLE_NAMES: &[&str] = &[
    "log10", "saturate", "fmod", "mul", "ddx", "ddy", "atan2", "frac", "lerp",
];

/// Applies every source-level fix to an include-expanded shader body.
pub fn fixup_hlsl_source(body: &str) -> String {
    let uniforms: HashSet<String> = re!(r"(?m)^\s*uniform\s+\w+\s+(\w+)")
        .captures_iter(body)
        .map(|c| c[1].to_string())
        .collect();
    let main_variable = re!(r"\b(?:vec[234]|float[234]?|int)\s+main\b").is_match(body);

    let mut out_lines: Vec<String> = Vec::new();
    for name in PREAMBLE_NAMES {
        let defines_function = Regex::new(&format!(
            r"(?m)^\s*(?:float|int|bool|void|vec[234]|float[234]|mat[234])\s+{name}\s*\("
        ))
        .map(|r| r.is_match(body))
        .unwrap_or(false);
        if defines_function {
            out_lines.push(format!("#undef {name}"));
        }
    }

    let mut depth = 0i32;
    let mut conditionals = 0i32;
    for line in body.lines() {
        let trimmed = line.trim_start();
        // A stray `#endif` / `#else` (the shader author's typo) is ignored by Wallpaper Engine.
        if re!(r"^#\s*if").is_match(trimmed) {
            conditionals += 1;
        } else if re!(r"^#\s*(?:endif|else|elif)\b").is_match(trimmed) {
            if conditionals == 0 {
                continue;
            }
            if re!(r"^#\s*endif\b").is_match(trimmed) {
                conditionals -= 1;
            }
        }
        let mut l = line.to_string();
        if let Some(c) =
            re!(r"^(\s*)float\s+(\w+)\s*\[\s*\]\s*=\s*(\w+)\s*;").captures(&l.clone())
        {
            l = format!("{}#define {} {}", &c[1], &c[2], &c[3]);
        } else if let Some(c) =
            re!(r"^(\s*#\s*define\s+\w+\s+[-+]?[0-9.]+f?)\s*;(.*)$").captures(&l.clone())
        {
            l = format!("{}{}", &c[1], &c[2]);
        }
        if main_variable && !re!(r"\bvoid\s+main\b").is_match(&l) {
            l = re!(r"\bmain\b").replace_all(&l, "tp_main_var").into_owned();
        }
        if let Some(c) =
            re!(r"^(\s*)const\s+(\w+)\s+(\w+)\s*=\s*(.+?)\s*;\s*(?://.*)?$").captures(&l.clone())
        {
            let (indent, ty, name, rhs) = (&c[1], &c[2], &c[3], &c[4]);
            if !is_constant_initializer(rhs, &uniforms) {
                // GLSL (and naga) only allow constant expressions here; HLSL does not mind.
                // A global becomes a macro so every function sees the value; a local loses
                // its `const`.
                l = if depth == 0 {
                    format!("{indent}#define {name} ({rhs})")
                } else {
                    format!("{indent}{ty} {name} = {rhs};")
                };
            }
        }
        l = comparison_arithmetic(&l);

        let code = l.split("//").next().unwrap_or("");
        depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
        out_lines.push(l);
    }

    prune_unused_blend_helpers(&mut out_lines);
    let mut out = out_lines.join("\n");
    out.push('\n');
    out
}

/// Drops the single-line `Blend*` helper functions of the bundled `common_blending.h` that the
/// shader never calls (directly or through other helpers), so including the header does not
/// bloat every translated module with ~130 overloads.
fn prune_unused_blend_helpers(lines: &mut Vec<String>) {
    let def = re!(r"^\s*(?:float|vec[234])\s+(Blend\w+)\s*\(.*\)\s*\{.*\}\s*$");
    if !lines.iter().any(|l| l.starts_with("float BlendNormalf(")) {
        return;
    }
    loop {
        let defined: HashSet<String> = lines
            .iter()
            .filter_map(|l| def.captures(l).map(|c| c[1].to_string()))
            .collect();
        let mut used: HashSet<String> = HashSet::new();
        for l in lines.iter() {
            let body = match def.captures(l) {
                // A helper's own name does not count as a use.
                Some(c) => l.replacen(&c[1], "", 1),
                None => l.clone(),
            };
            for m in re!(r"\bBlend\w+").find_iter(&body) {
                used.insert(m.as_str().to_string());
            }
        }
        let before = lines.len();
        lines.retain(|l| match def.captures(l) {
            Some(c) => used.contains(&c[1]) || !defined.contains(&c[1]),
            None => true,
        });
        if lines.len() == before {
            break;
        }
    }
}

/// Whether a `const` initializer is a constant expression: no uniform and no function call other
/// than type constructors.
fn is_constant_initializer(rhs: &str, uniforms: &HashSet<String>) -> bool {
    const CONSTRUCTORS: &[&str] = &[
        "float", "int", "uint", "bool", "vec2", "vec3", "vec4", "ivec2", "ivec3", "ivec4", "mat2",
        "mat3", "mat4", "float2", "float3", "float4", "int2", "int3", "int4", "CAST2", "CAST3",
        "CAST4",
    ];
    if re!(r"\w+")
        .find_iter(rhs)
        .any(|m| m.as_str().starts_with("g_") || uniforms.contains(m.as_str()))
    {
        return false;
    }
    re!(r"(\w+)\s*\(")
        .captures_iter(rhs)
        .all(|c| CONSTRUCTORS.contains(&&c[1]))
}

/// `(a < b) * x` / `x * (a < b)` -> `float(a < b) * x`; HLSL converts the bool implicitly.
fn comparison_arithmetic(line: &str) -> String {
    let before = re!(r"(^|[^\w])\(([^()=<>!&|?:;]+?(?:<=|>=|==|!=|<|>)[^()=<>!&|?:;]+?)\)(\s*\*)");
    let after = re!(r"(\*\s*)\(([^()=<>!&|?:;]+?(?:<=|>=|==|!=|<|>)[^()=<>!&|?:;]+?)\)");
    let l = before.replace_all(line, "${1}float(${2})${3}");
    after.replace_all(&l, "${1}float(${2})").into_owned()
}

/// Scalar and vector widths an HLSL-style overload may mix.
/// Width 0 stands for an `int` scalar (`step(x, 1)`), offered for two-argument functions only.
const WIDTHS: [usize; 5] = [1, 2, 3, 4, 0];

fn glsl_type(width: usize) -> String {
    if width == 0 {
        "int".to_string()
    } else if width == 1 {
        "float".to_string()
    } else {
        format!("vec{width}")
    }
}

/// Overloads `tpMix` / `tpStep` / ... of the GLSL builtins for every mix of scalar and vector
/// arguments; the result has the width of the shortest vector argument (HLSL truncation). Only
/// the functions `source` calls are emitted. Returns the definitions followed by the `#define`s
/// that route the builtin names to them.
pub fn lax_builtin_overloads(source: &str) -> String {
    const FUNCTIONS: &[(&str, usize)] = &[
        ("mix", 3),
        ("clamp", 3),
        ("smoothstep", 3),
        ("step", 2),
        ("min", 2),
        ("max", 2),
        ("pow", 2),
    ];
    let mut defs = String::new();
    let mut routes = String::new();
    for &(name, arity) in FUNCTIONS {
        let called = Regex::new(&format!(r"\b{name}\s*\("))
            .map(|r| r.is_match(source))
            .unwrap_or(false);
        if !called {
            continue;
        }
        let tp = format!("tp_{name}");
        let kinds: usize = if arity == 2 { 5 } else { 4 };
        let combos = kinds.pow(arity as u32);
        for code in 0..combos {
            let widths: Vec<usize> = (0..arity)
                .map(|k| WIDTHS[(code / kinds.pow(k as u32)) % kinds])
                .collect();
            let w = widths.iter().copied().filter(|&w| w > 1).min().unwrap_or(1);
            let params: Vec<String> = widths
                .iter()
                .enumerate()
                .map(|(k, &wk)| format!("{} a{k}", glsl_type(wk)))
                .collect();
            let args: Vec<String> = widths
                .iter()
                .enumerate()
                .map(|(k, &wk)| match (wk, w) {
                    (a, b) if a == b => format!("a{k}"),
                    (0, 1) => format!("float(a{k})"),
                    (0 | 1, b) => format!("vec{b}(a{k})"),
                    (_, b) => format!("a{k}.{}", &"xyzw"[..b]),
                })
                .collect();
            defs.push_str(&format!(
                "{} {tp}({}) {{ return {name}({}); }}\n",
                glsl_type(w),
                params.join(", "),
                args.join(", ")
            ));
        }
        routes.push_str(&format!("#define {name} {tp}\n"));
    }
    format!("{defs}{routes}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_function_named_like_a_preamble_macro_gets_an_undef() {
        let out = fixup_hlsl_source("float log10(float x) { return log(x) / 2.3; }\n");
        assert!(out.starts_with("#undef log10\n"), "{out}");
    }

    #[test]
    fn local_variable_named_main_is_renamed_but_entry_point_is_not() {
        let out = fixup_hlsl_source(
            "void main() {\n\tvec2 main = uv - pos;\n\tfloat d = length(main);\n}\n",
        );
        assert!(out.contains("void main()"), "{out}");
        assert!(out.contains("vec2 tp_main_var = uv - pos;"), "{out}");
        assert!(out.contains("length(tp_main_var)"), "{out}");
    }

    #[test]
    fn numeric_define_loses_its_trailing_semicolon() {
        let out = fixup_hlsl_source("#define mask 1.0; // full\n#define DO x = 1;\n");
        assert!(out.contains("#define mask 1.0 // full"), "{out}");
        assert!(out.contains("#define DO x = 1;"), "{out}");
    }

    #[test]
    fn unsized_array_alias_becomes_a_define() {
        let out = fixup_hlsl_source("\tfloat spectrum[] = g_AudioSpectrum16Left;\n");
        assert!(out.contains("#define spectrum g_AudioSpectrum16Left"), "{out}");
    }

    #[test]
    fn non_constant_global_const_becomes_a_macro_and_local_const_is_stripped() {
        let out = fixup_hlsl_source(
            "uniform float u_strength;\nconst float k = -u_strength * 0.1;\nconst vec3 n = normalize(vec3(1.0));\nconst float pi = 3.14;\nconst vec2 v = vec2(1.0, 2.0);\nvoid main() {\n\tconst float z = u_strength;\n}\n",
        );
        assert!(out.contains("#define k (-u_strength * 0.1)"), "{out}");
        assert!(out.contains("#define n (normalize(vec3(1.0)))"), "{out}");
        assert!(out.contains("const float pi = 3.14;"), "{out}");
        assert!(out.contains("const vec2 v = vec2(1.0, 2.0);"), "{out}");
        assert!(out.contains("\tfloat z = u_strength;"), "{out}");
    }

    #[test]
    fn stray_endif_is_dropped_but_balanced_ones_stay() {
        let out = fixup_hlsl_source("#if A\nx;\n#endif\n#endif\n#else\ny;\n");
        assert_eq!(out, "#if A\nx;\n#endif\ny;\n");
    }

    #[test]
    fn comparison_results_are_converted_before_arithmetic() {
        let out = fixup_hlsl_source("\tdepth *= (depth < limit) * 6.0;\n\tx = 2.0 * (a >= b);\n");
        assert!(out.contains("depth *= float(depth < limit) * 6.0;"), "{out}");
        assert!(out.contains("x = 2.0 * float(a >= b);"), "{out}");
        let call = fixup_hlsl_source("\tx = foo(a < b) * 2.0;\n");
        assert!(call.contains("foo(a < b) * 2.0"), "{call}");
    }

    #[test]
    fn unused_blend_helpers_are_pruned_and_used_ones_kept_transitively() {
        let src = "float BlendNormalf(float b, float l) { return l; }\n\
                   float BlendAddf(float b, float l) { return min(b + l, 1.0); }\n\
                   float BlendOverlayf(float b, float l) { return l; }\n\
                   float BlendHardLightf(float b, float l) { return BlendOverlayf(l, b); }\n\
                   float BlendUnusedf(float b, float l) { return b; }\n\
                   void main() { x = BlendHardLightf(a, b); }\n";
        let out = fixup_hlsl_source(src);
        assert!(out.contains("BlendHardLightf(float"), "{out}");
        assert!(out.contains("BlendOverlayf(float"), "{out}");
        assert!(!out.contains("BlendAddf"), "{out}");
        assert!(!out.contains("BlendUnusedf"), "{out}");
    }

    #[test]
    fn lax_overloads_cover_mixed_widths_and_only_called_functions() {
        let out = lax_builtin_overloads("x = mix(a, b, t);");
        assert!(out.contains("vec3 tp_mix(vec4 a0, vec3 a1, float a2)"), "{out}");
        assert!(out.contains("#define mix tp_mix"), "{out}");
        assert!(!out.contains("tp_step"), "{out}");
        let step = lax_builtin_overloads("v = step(a, 1);");
        assert!(step.contains("vec2 tp_step(vec2 a0, int a1)"), "{step}");
        assert!(step.contains("float tp_step(int a0, float a1)"), "{step}");
    }
}
