// Refracting sprite particles (the `REFRACT` combo of `genericparticle`).
// The colour texture is only a mask; the sprite shows the scene behind it, shifted by a
// normal map. Output alpha is the mask, so flat normals leave the scene unchanged.
struct SimpleUniforms {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
};
struct RefractParams {
    amount: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: SimpleUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;
@group(1) @binding(0) var normal_tex: texture_2d<f32>;
@group(1) @binding(1) var scene_backdrop: texture_2d<f32>;
@group(1) @binding(2) var<uniform> rp: RefractParams;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) texcoord_rot_size: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) velocity_life: vec4<f32>,
    @location(4) rotation_xy: vec2<f32>,
};
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) rot: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let uv = v.texcoord_rot_size.xy;
    let half_size = v.texcoord_rot_size.w;
    let corner = vec2<f32>(uv.x - 0.5, 0.5 - uv.y) * 2.0 * half_size;
    let c = cos(v.texcoord_rot_size.z);
    let s = sin(v.texcoord_rot_size.z);
    let rotated = vec2<f32>(corner.x * c - corner.y * s, corner.x * s + corner.y * c);
    var out: VsOut;
    out.position = u.mvp * vec4<f32>(v.position + vec3<f32>(rotated, 0.0), 1.0);
    out.uv = uv;
    out.color = v.color * u.color;
    out.rot = vec2<f32>(c, s);
    return out;
}

fn refract_offset(n: vec2<f32>, amount: f32, strength: f32) -> vec2<f32> {
    let o = n * amount * strength * 0.1;
    let len = length(o);
    return select(o, o * (0.15 / max(len, 1e-6)), len > 0.15);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let mask = textureSample(src_tex, src_sampler, in.uv);
    let strength = clamp(mask.a * mask.r * in.color.a, 0.0, 1.0);
    let n = textureSample(normal_tex, src_sampler, in.uv).xy * 2.0 - 1.0;
    // Normal maps are authored in sprite space (y up); the sprite is rotated, screen y is down.
    let nw = vec2<f32>(n.x * in.rot.x - n.y * in.rot.y, -(n.x * in.rot.y + n.y * in.rot.x));
    let dims = vec2<f32>(textureDimensions(scene_backdrop));
    let off = refract_offset(nw, rp.amount.x, strength);
    let pix = clamp(in.position.xy + off * dims, vec2<f32>(0.0), dims - vec2<f32>(1.0));
    let scene = textureLoad(scene_backdrop, vec2<i32>(pix), 0);
    return vec4<f32>(scene.rgb, strength);
}
