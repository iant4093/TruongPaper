struct CompositorUniforms {
    // progress in [0.0, 1.0], already cubic-bezier eased by CPU or eased in shader
    progress: f32,
    // 0 = Smooth Crossfade, 1 = Blur-Dissolve, 2 = Liquid Glass Morph
    transition_mode: u32,
    // 0 = Fill (Cover), 1 = Fit + Blurred Backdrop, 2 = Fit + Black Bars, 3 = Stretch
    from_fit_mode: u32,
    to_fit_mode: u32,
    // Aspect ratios (width / height)
    monitor_aspect: f32,
    from_image_aspect: f32,
    to_image_aspect: f32,
    zoom: f32,          // default 1.0
    offset_x: f32,      // [-1.0, 1.0], default 0.0
    offset_y: f32,      // [-1.0, 1.0], default 0.0
    // Color grading
    brightness: f32,    // [-1.0, 1.0], default 0.0
    contrast: f32,      // [0.0, 2.0], default 1.0
    saturation: f32,    // [0.0, 2.0], default 1.0
    hue_shift: f32,     // [-3.14159, 3.14159], default 0.0
    time: f32,
    _pad: f32,
    // Span mode sub-rectangle of the shared canvas: canvas_uv = offset + uv * scale.
    span_offset_x: f32,
    span_offset_y: f32,
    span_scale_x: f32,
    span_scale_y: f32,
};

@group(0) @binding(0) var t_from: texture_2d<f32>;
@group(0) @binding(1) var t_to: texture_2d<f32>;
@group(0) @binding(2) var t_from_blurred: texture_2d<f32>;
@group(0) @binding(3) var t_to_blurred: texture_2d<f32>;
@group(0) @binding(4) var s_linear: sampler;
@group(0) @binding(5) var<uniform> u: CompositorUniforms;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOutput {
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    var out: VertexOutput;
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

// Maps monitor-local UV to the (possibly shared, span mode) canvas UV.
fn canvas_uv(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(u.span_offset_x, u.span_offset_y) + uv * vec2<f32>(u.span_scale_x, u.span_scale_y);
}

// Computes aspect-corrected UV and samples sharp vs blurred/black bars for a layer
fn sample_fitted_layer(
    local_uv: vec2<f32>,
    img_aspect: f32,
    fit_mode: u32,
    t_sharp: texture_2d<f32>,
    t_blur: texture_2d<f32>
) -> vec3<f32> {
    let screen_uv = canvas_uv(local_uv);
    let centered = (screen_uv - vec2<f32>(0.5, 0.5)) / max(u.zoom, 0.05)
                   - vec2<f32>(u.offset_x * 0.5, u.offset_y * 0.5);

    // Stretch mode (3): map [0,1] directly
    if (fit_mode == 3u) {
        return textureSampleLevel(t_sharp, s_linear, clamp(centered + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
    }

    let ratio = u.monitor_aspect / max(img_aspect, 0.001);

    // Fill / Center-Crop mode (0)
    if (fit_mode == 0u) {
        var scale = vec2<f32>(1.0, 1.0);
        if (ratio > 1.0) {
            // Monitor is wider than image: crop top/bottom
            scale.y = 1.0 / ratio;
        } else {
            // Monitor is taller than image: crop left/right
            scale.x = ratio;
        }
        let uv_fill = clamp(centered * scale + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
        return textureSampleLevel(t_sharp, s_linear, uv_fill, 0.0).rgb;
    }

    // Fit + Blurred Backdrop (1) or Fit + Black Bars (2)
    var fit_scale = vec2<f32>(1.0, 1.0);
    if (ratio > 1.0) {
        // Monitor is wider than image (e.g., portrait photo on landscape monitor): pillarbox X
        fit_scale.x = ratio;
    } else {
        // Monitor is taller than image: letterbox Y
        fit_scale.y = 1.0 / ratio;
    }

    let uv_fit = centered * fit_scale + 0.5;
    let in_bounds = uv_fit.x >= 0.0 && uv_fit.x <= 1.0 && uv_fit.y >= 0.0 && uv_fit.y <= 1.0;

    if (in_bounds) {
        return textureSampleLevel(t_sharp, s_linear, uv_fit, 0.0).rgb;
    } else if (fit_mode == 1u) {
        // Sample cover-scaled Kawase-blurred backdrop in pillarbox/letterbox bars (dimmed to 78% for depth)
        var cover_scale = vec2<f32>(1.0, 1.0);
        if (ratio > 1.0) { cover_scale.y = 1.0 / ratio; } else { cover_scale.x = ratio; }
        let uv_cover = clamp((screen_uv - 0.5) * cover_scale + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
        return textureSampleLevel(t_blur, s_linear, uv_cover, 0.0).rgb * 0.78;
    } else {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
}

// 2D Simplex/Gradient noise for Liquid Glass refraction
fn hash22(p: vec2<f32>) -> vec2<f32> {
    var p3 = fract(vec3<f32>(p.xyx) * vec3<f32>(0.1031, 0.1030, 0.0973));
    p3 += dot(p3, p3.yzx + 33.33);
    return -1.0 + 2.0 * fract((p3.xx + p3.yz) * p3.zy);
}

fn gradient_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u_f = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(dot(hash22(i + vec2<f32>(0.0, 0.0)), f - vec2<f32>(0.0, 0.0)),
            dot(hash22(i + vec2<f32>(1.0, 0.0)), f - vec2<f32>(1.0, 0.0)), u_f.x),
        mix(dot(hash22(i + vec2<f32>(0.0, 1.0)), f - vec2<f32>(0.0, 1.0)),
            dot(hash22(i + vec2<f32>(1.0, 1.0)), f - vec2<f32>(1.0, 1.0)), u_f.x),
        u_f.y
    );
}

fn apply_color_grading(rgb: vec3<f32>) -> vec3<f32> {
    // 1. Brightness & Contrast
    var c = (rgb - 0.5) * u.contrast + 0.5 + u.brightness;
    // 2. Hue Shift (YIQ / Rodrigues rotation around luma axis)
    let k = vec3<f32>(0.57735, 0.57735, 0.57735);
    let cos_angle = cos(u.hue_shift);
    c = c * cos_angle + cross(k, c) * sin(u.hue_shift) + k * dot(k, c) * (1.0 - cos_angle);
    // 3. Saturation
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = mix(vec3<f32>(luma), c, u.saturation);
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = smoothstep(0.0, 1.0, clamp(u.progress, 0.0, 1.0));
    var final_rgb: vec3<f32>;

    if (u.transition_mode == 0u) {
        // Mode 0: Smooth Crossfade
        let col_from = sample_fitted_layer(in.uv, u.from_image_aspect, u.from_fit_mode, t_from, t_from_blurred);
        let col_to = sample_fitted_layer(in.uv, u.to_image_aspect, u.to_fit_mode, t_to, t_to_blurred);
        final_rgb = mix(col_from, col_to, t);
    } else if (u.transition_mode == 1u) {
        // Mode 1: Blur-Dissolve (sharp -> blurred midpoint -> sharp)
        let col_from_sharp = sample_fitted_layer(in.uv, u.from_image_aspect, u.from_fit_mode, t_from, t_from_blurred);
        let col_from_blur = textureSampleLevel(t_from_blurred, s_linear, canvas_uv(in.uv), 0.0).rgb;
        let col_to_sharp = sample_fitted_layer(in.uv, u.to_image_aspect, u.to_fit_mode, t_to, t_to_blurred);
        let col_to_blur = textureSampleLevel(t_to_blurred, s_linear, canvas_uv(in.uv), 0.0).rgb;

        let from_mixed = mix(col_from_sharp, col_from_blur, smoothstep(0.0, 0.7, t));
        let to_mixed = mix(col_to_blur, col_to_sharp, smoothstep(0.3, 1.0, t));
        final_rgb = mix(from_mixed, to_mixed, t);
    } else {
        // Mode 2: Liquid Glass Morph / Refractive Distortion
        let envelope = sin(t * 3.14159265);
        let scaled_uv = vec2<f32>(in.uv.x * u.monitor_aspect, in.uv.y) * 3.5;
        let n1 = gradient_noise(scaled_uv + vec2<f32>(t * 1.4, -t * 0.9));
        let n2 = gradient_noise(scaled_uv * 2.1 - vec2<f32>(t * 1.1, t * 1.6));
        let displacement = vec2<f32>(n1, n2) * 0.045 * envelope;

        // Chromatic dispersion offsets for R, G, B channels
        let dispersion = 0.0045 * envelope;
        let uv_from = in.uv + displacement * t;
        let uv_to = in.uv - displacement * (1.0 - t);

        let from_r = sample_fitted_layer(uv_from + vec2<f32>(dispersion, 0.0), u.from_image_aspect, u.from_fit_mode, t_from, t_from_blurred).r;
        let from_g = sample_fitted_layer(uv_from, u.from_image_aspect, u.from_fit_mode, t_from, t_from_blurred).g;
        let from_b = sample_fitted_layer(uv_from - vec2<f32>(dispersion, 0.0), u.from_image_aspect, u.from_fit_mode, t_from, t_from_blurred).b;

        let to_r = sample_fitted_layer(uv_to + vec2<f32>(dispersion, 0.0), u.to_image_aspect, u.to_fit_mode, t_to, t_to_blurred).r;
        let to_g = sample_fitted_layer(uv_to, u.to_image_aspect, u.to_fit_mode, t_to, t_to_blurred).g;
        let to_b = sample_fitted_layer(uv_to - vec2<f32>(dispersion, 0.0), u.to_image_aspect, u.to_fit_mode, t_to, t_to_blurred).b;

        let refracted_mix = mix(vec3<f32>(from_r, from_g, from_b), vec3<f32>(to_r, to_g, to_b), t);

        // Specular diagonal glass sweep highlight
        let sweep_pos = (in.uv.x * 0.75 + in.uv.y * 0.25) - (t * 1.4 - 0.2);
        let specular = exp(-sweep_pos * sweep_pos * 60.0) * 0.14 * envelope;
        final_rgb = clamp(refracted_mix + vec3<f32>(specular), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    return vec4<f32>(apply_color_grading(final_rgb), 1.0);
}
