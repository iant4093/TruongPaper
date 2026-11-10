// Scene bloom chain (docs/05 Phase 6, linux-wallpaperengine render-target names):
//   fs_threshold_downsample: scene        -> _rt_4FrameBuffer (1/4)
//   fs_downsample_blur:      _rt_4        -> _rt_8FrameBuffer (1/8)
//   fs_blur:                 _rt_8        -> _rt_Bloom        (1/8)
//   fs_combine:              _rt_Bloom    -> scene (additive blend state on the pipeline)
// `texel_size` is 1 / source texture size.

struct BloomUniforms {
    texel_size: vec2<f32>,
    strength: f32,
    threshold: f32,
};

@group(0) @binding(0) var<uniform> u: BloomUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle; uv (0,0) is the top-left of the texture.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VsOut;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// 4-tap box downsample (samples the corners of the 2x2 texel footprint).
fn box4(uv: vec2<f32>) -> vec3<f32> {
    let o = u.texel_size;
    var c = textureSample(src_tex, src_sampler, uv + vec2<f32>(-o.x, -o.y)).rgb;
    c += textureSample(src_tex, src_sampler, uv + vec2<f32>(o.x, -o.y)).rgb;
    c += textureSample(src_tex, src_sampler, uv + vec2<f32>(-o.x, o.y)).rgb;
    c += textureSample(src_tex, src_sampler, uv + vec2<f32>(o.x, o.y)).rgb;
    return c * 0.25;
}

@fragment
fn fs_threshold_downsample(in: VsOut) -> @location(0) vec4<f32> {
    let c = box4(in.uv);
    let l = luminance(c);
    let knee = max(l - u.threshold, 0.0) / max(l, 0.0001);
    return vec4<f32>(c * knee, 1.0);
}

@fragment
fn fs_downsample_blur(in: VsOut) -> @location(0) vec4<f32> {
    let o = u.texel_size;
    var c = box4(in.uv) * 0.5;
    c += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(-2.0 * o.x, 0.0)).rgb * 0.125;
    c += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(2.0 * o.x, 0.0)).rgb * 0.125;
    c += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(0.0, -2.0 * o.y)).rgb * 0.125;
    c += textureSample(src_tex, src_sampler, in.uv + vec2<f32>(0.0, 2.0 * o.y)).rgb * 0.125;
    return vec4<f32>(c, 1.0);
}

// 9-tap separable-weight Gaussian applied in both axes (3x3 kernel 1-2-1).
@fragment
fn fs_blur(in: VsOut) -> @location(0) vec4<f32> {
    let o = u.texel_size;
    var c = vec3<f32>(0.0);
    for (var y: i32 = -1; y <= 1; y = y + 1) {
        for (var x: i32 = -1; x <= 1; x = x + 1) {
            let w = (2.0 - abs(f32(x))) * (2.0 - abs(f32(y))) / 16.0;
            let uv = in.uv + vec2<f32>(f32(x) * o.x, f32(y) * o.y);
            c += textureSample(src_tex, src_sampler, uv).rgb * w;
        }
    }
    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_combine(in: VsOut) -> @location(0) vec4<f32> {
    let bloom = textureSample(src_tex, src_sampler, in.uv).rgb;
    return vec4<f32>(bloom * u.strength, 1.0);
}
