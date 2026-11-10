struct BlurUniforms {
    texel_size: vec2<f32>,
    offset: f32,
    _pad: f32,
};

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var s_linear: sampler;
@group(0) @binding(2) var<uniform> u_blur: BlurUniforms;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOutput {
    // Fullscreen triangle
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    var out: VertexOutput;
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_downsample(in: VertexOutput) -> @location(0) vec4<f32> {
    let halfpixel = 0.5 * u_blur.texel_size * u_blur.offset;
    var sum = textureSample(t_input, s_linear, in.uv) * 4.0;
    sum += textureSample(t_input, s_linear, in.uv - halfpixel.xy);
    sum += textureSample(t_input, s_linear, in.uv + halfpixel.xy);
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(halfpixel.x, -halfpixel.y));
    sum += textureSample(t_input, s_linear, in.uv - vec2<f32>(halfpixel.x, -halfpixel.y));
    return sum / 8.0;
}

@fragment
fn fs_upsample(in: VertexOutput) -> @location(0) vec4<f32> {
    let halfpixel = 0.5 * u_blur.texel_size * u_blur.offset;
    var sum = textureSample(t_input, s_linear, in.uv + vec2<f32>(-halfpixel.x * 2.0, 0.0));
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(-halfpixel.x, halfpixel.y)) * 2.0;
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(0.0, halfpixel.y * 2.0));
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(halfpixel.x, halfpixel.y)) * 2.0;
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(halfpixel.x * 2.0, 0.0));
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(halfpixel.x, -halfpixel.y)) * 2.0;
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(0.0, -halfpixel.y * 2.0));
    sum += textureSample(t_input, s_linear, in.uv + vec2<f32>(-halfpixel.x, -halfpixel.y)) * 2.0;
    return sum / 12.0;
}
