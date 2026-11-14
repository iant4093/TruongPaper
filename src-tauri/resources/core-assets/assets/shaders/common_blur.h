// TruongPaper clean-room replacement for the Wallpaper Engine "common_blur.h" include.
// Separable Gaussian blurs of g_Texture0. `step` is the per-tap UV offset (already
// scaled by direction, e.g. v_TexCoord.zw), so the same call blurs along X or Y.
// blurN returns rgb, blurNa returns rgba (alpha blurred too).
#ifndef TP_COMMON_BLUR_H
#define TP_COMMON_BLUR_H

// Normalised Gaussian over taps -R..R with the given sigma.
vec4 tpGaussianBlur(vec2 uv, vec2 step, int radius, float sigma) {
    vec4 sum = vec4(0.0);
    float total = 0.0;
    float denom = 2.0 * sigma * sigma;
    for (int i = -12; i <= 12; ++i) {
        if (i < -radius || i > radius) {
            continue;
        }
        float fi = float(i);
        float w = exp(-(fi * fi) / denom);
        sum += texSample2D(g_Texture0, uv + step * fi) * w;
        total += w;
    }
    return sum / total;
}

vec4 blur3a(vec2 uv, vec2 step)  { return tpGaussianBlur(uv, step, 1, 0.85); }
vec4 blur5a(vec2 uv, vec2 step)  { return tpGaussianBlur(uv, step, 2, 1.2); }
vec4 blur7a(vec2 uv, vec2 step)  { return tpGaussianBlur(uv, step, 3, 1.6); }
vec4 blur9a(vec2 uv, vec2 step)  { return tpGaussianBlur(uv, step, 4, 2.1); }
vec4 blur13a(vec2 uv, vec2 step) { return tpGaussianBlur(uv, step, 6, 3.0); }

vec3 blur3(vec2 uv, vec2 step)  { return blur3a(uv, step).rgb; }
vec3 blur5(vec2 uv, vec2 step)  { return blur5a(uv, step).rgb; }
vec3 blur7(vec2 uv, vec2 step)  { return blur7a(uv, step).rgb; }
vec3 blur9(vec2 uv, vec2 step)  { return blur9a(uv, step).rgb; }
vec3 blur13(vec2 uv, vec2 step) { return blur13a(uv, step).rgb; }

// Radial blur of g_Texture0 toward/away from `center`: taps run along the line through `uv` and
// `center`, spread by `scale`.
vec4 tpRadialBlur(vec2 uv, vec2 center, float scale, int radius, float sigma) {
    vec2 dir = (uv - center) * scale;
    vec4 sum = vec4(0.0);
    float total = 0.0;
    float denom = 2.0 * sigma * sigma;
    for (int i = -12; i <= 12; ++i) {
        if (i < -radius || i > radius) {
            continue;
        }
        float fi = float(i);
        float w = exp(-(fi * fi) / denom);
        sum += texSample2D(g_Texture0, uv + dir * (fi / float(radius))) * w;
        total += w;
    }
    return sum / total;
}

vec4 blurRadial3a(vec2 uv, vec2 center, float scale)  { return tpRadialBlur(uv, center, scale, 1, 0.85); }
vec4 blurRadial7a(vec2 uv, vec2 center, float scale)  { return tpRadialBlur(uv, center, scale, 3, 1.6); }
vec4 blurRadial13a(vec2 uv, vec2 center, float scale) { return tpRadialBlur(uv, center, scale, 6, 3.0); }
vec3 blurRadial3(vec2 uv, vec2 center, float scale)   { return blurRadial3a(uv, center, scale).rgb; }
vec3 blurRadial7(vec2 uv, vec2 center, float scale)   { return blurRadial7a(uv, center, scale).rgb; }
vec3 blurRadial13(vec2 uv, vec2 center, float scale)  { return blurRadial13a(uv, center, scale).rgb; }

#endif
