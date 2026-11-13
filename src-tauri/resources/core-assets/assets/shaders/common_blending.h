#ifndef TRUONGPAPER_BLEND_GLSL
#define TRUONGPAPER_BLEND_GLSL
// Non-separable HSL helpers (hue / saturation / color / luminosity modes 26-29).
float TpBlendLum(vec3 c) { return dot(c, vec3(0.3, 0.59, 0.11)); }
vec3 TpBlendClipColor(vec3 c) {
    float l = TpBlendLum(c);
    float n = min(min(c.r, c.g), c.b);
    float x = max(max(c.r, c.g), c.b);
    if (n < 0.0) c = l + (c - l) * l / max(l - n, 1e-6);
    if (x > 1.0) c = l + (c - l) * (1.0 - l) / max(x - l, 1e-6);
    return c;
}
vec3 TpBlendSetLum(vec3 c, float l) { return TpBlendClipColor(c + (l - TpBlendLum(c))); }
float TpBlendSat(vec3 c) { return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b); }
vec3 TpBlendSetSat(vec3 c, float s) {
    float n = min(min(c.r, c.g), c.b);
    float x = max(max(c.r, c.g), c.b);
    return x > n ? (c - n) * s / (x - n) : vec3(0.0);
}
vec3 ApplyBlending(int mode, vec3 base, vec3 layer, float amount) {
    vec3 result = layer;
    if (mode == 1 || mode == 5) result = min(base, layer);
    else if (mode == 2) result = base * layer;
    else if (mode == 4 || mode == 20) result = max(base - layer, vec3(0.0));
    else if (mode == 6 || mode == 10) result = max(base, layer);
    else if (mode == 7) result = base + layer - base * layer;
    else if (mode == 9) result = min(base + layer, vec3(1.0));
    else if (mode == 11) result = mix(2.0 * base * layer, 1.0 - 2.0 * (1.0 - base) * (1.0 - layer), step(vec3(0.5), base));
    else if (mode == 12) result = mix(base - (1.0 - 2.0 * layer) * base * (1.0 - base), base + (2.0 * layer - 1.0) * (sqrt(max(base, vec3(0.0))) - base), step(vec3(0.5), layer));
    else if (mode == 18) result = abs(base - layer);
    else if (mode == 19) result = base + layer - 2.0 * base * layer;
    else if (mode == 24) result = (base + layer) * 0.5;
    else if (mode == 25) result = 1.0 - abs(1.0 - base - layer);
    else if (mode == 26) result = TpBlendSetLum(TpBlendSetSat(layer, TpBlendSat(base)), TpBlendLum(base));
    else if (mode == 27) result = TpBlendSetLum(TpBlendSetSat(base, TpBlendSat(layer)), TpBlendLum(base));
    else if (mode == 28) result = TpBlendSetLum(layer, TpBlendLum(base));
    else if (mode == 29) result = TpBlendSetLum(base, TpBlendLum(layer));
    // 30 is the effect default of `tint`: the colour multiplies the image (white = no change).
    else if (mode == 30) result = base * layer;
    if (mode == 5 || mode == 10) return result;
    return mix(base, result, amount);
}
// Per-mode blend functions (WE common_blending.h interface): Blend<Mode>f works on floats,
// Blend<Mode> on vec2/vec3/vec4 per channel; a float blend operand is splatted.
float BlendNormalf(float b, float l) { return l; }
float BlendLightenf(float b, float l) { return max(b, l); }
float BlendDarkenf(float b, float l) { return min(b, l); }
float BlendMultiplyf(float b, float l) { return b * l; }
float BlendAveragef(float b, float l) { return (b + l) * 0.5; }
float BlendAddf(float b, float l) { return min(b + l, 1.0); }
float BlendSubstractf(float b, float l) { return max(b + l - 1.0, 0.0); }
float BlendDifferencef(float b, float l) { return abs(b - l); }
float BlendNegationf(float b, float l) { return 1.0 - abs(1.0 - b - l); }
float BlendExclusionf(float b, float l) { return b + l - 2.0 * b * l; }
float BlendScreenf(float b, float l) { return 1.0 - (1.0 - b) * (1.0 - l); }
float BlendOverlayf(float b, float l) { return b < 0.5 ? 2.0 * b * l : 1.0 - 2.0 * (1.0 - b) * (1.0 - l); }
float BlendSoftLightf(float b, float l) { return l < 0.5 ? 2.0 * b * l + b * b * (1.0 - 2.0 * l) : sqrt(b) * (2.0 * l - 1.0) + 2.0 * b * (1.0 - l); }
float BlendHardLightf(float b, float l) { return BlendOverlayf(l, b); }
float BlendColorDodgef(float b, float l) { return l >= 1.0 ? l : min(b / (1.0 - l), 1.0); }
float BlendColorBurnf(float b, float l) { return l <= 0.0 ? l : max(1.0 - (1.0 - b) / l, 0.0); }
float BlendLinearLightf(float b, float l) { return l < 0.5 ? BlendSubstractf(b, 2.0 * l) : BlendAddf(b, 2.0 * (l - 0.5)); }
float BlendVividLightf(float b, float l) { return l < 0.5 ? BlendColorBurnf(b, 2.0 * l) : BlendColorDodgef(b, 2.0 * (l - 0.5)); }
float BlendPinLightf(float b, float l) { return l < 0.5 ? BlendDarkenf(b, 2.0 * l) : BlendLightenf(b, 2.0 * (l - 0.5)); }
float BlendHardMixf(float b, float l) { return BlendVividLightf(b, l) < 0.5 ? 0.0 : 1.0; }
float BlendReflectf(float b, float l) { return l >= 1.0 ? l : min(b * b / (1.0 - l), 1.0); }
float BlendGlowf(float b, float l) { return BlendReflectf(l, b); }
float BlendPhoenixf(float b, float l) { return min(b, l) - max(b, l) + 1.0; }
#define BlendLinearDodgef BlendAddf
#define BlendLinearBurnf BlendSubstractf
vec2 BlendNormal(vec2 b, vec2 l) { return vec2(BlendNormalf(b.x, l.x), BlendNormalf(b.y, l.y)); }
vec3 BlendNormal(vec3 b, vec3 l) { return vec3(BlendNormalf(b.x, l.x), BlendNormalf(b.y, l.y), BlendNormalf(b.z, l.z)); }
vec4 BlendNormal(vec4 b, vec4 l) { return vec4(BlendNormalf(b.x, l.x), BlendNormalf(b.y, l.y), BlendNormalf(b.z, l.z), BlendNormalf(b.w, l.w)); }
vec2 BlendNormal(vec2 b, float l) { return BlendNormal(b, vec2(l)); }
vec3 BlendNormal(vec3 b, float l) { return BlendNormal(b, vec3(l)); }
vec4 BlendNormal(vec4 b, float l) { return BlendNormal(b, vec4(l)); }
vec2 BlendLighten(vec2 b, vec2 l) { return vec2(BlendLightenf(b.x, l.x), BlendLightenf(b.y, l.y)); }
vec3 BlendLighten(vec3 b, vec3 l) { return vec3(BlendLightenf(b.x, l.x), BlendLightenf(b.y, l.y), BlendLightenf(b.z, l.z)); }
vec4 BlendLighten(vec4 b, vec4 l) { return vec4(BlendLightenf(b.x, l.x), BlendLightenf(b.y, l.y), BlendLightenf(b.z, l.z), BlendLightenf(b.w, l.w)); }
vec2 BlendLighten(vec2 b, float l) { return BlendLighten(b, vec2(l)); }
vec3 BlendLighten(vec3 b, float l) { return BlendLighten(b, vec3(l)); }
vec4 BlendLighten(vec4 b, float l) { return BlendLighten(b, vec4(l)); }
vec2 BlendDarken(vec2 b, vec2 l) { return vec2(BlendDarkenf(b.x, l.x), BlendDarkenf(b.y, l.y)); }
vec3 BlendDarken(vec3 b, vec3 l) { return vec3(BlendDarkenf(b.x, l.x), BlendDarkenf(b.y, l.y), BlendDarkenf(b.z, l.z)); }
vec4 BlendDarken(vec4 b, vec4 l) { return vec4(BlendDarkenf(b.x, l.x), BlendDarkenf(b.y, l.y), BlendDarkenf(b.z, l.z), BlendDarkenf(b.w, l.w)); }
vec2 BlendDarken(vec2 b, float l) { return BlendDarken(b, vec2(l)); }
vec3 BlendDarken(vec3 b, float l) { return BlendDarken(b, vec3(l)); }
vec4 BlendDarken(vec4 b, float l) { return BlendDarken(b, vec4(l)); }
vec2 BlendMultiply(vec2 b, vec2 l) { return vec2(BlendMultiplyf(b.x, l.x), BlendMultiplyf(b.y, l.y)); }
vec3 BlendMultiply(vec3 b, vec3 l) { return vec3(BlendMultiplyf(b.x, l.x), BlendMultiplyf(b.y, l.y), BlendMultiplyf(b.z, l.z)); }
vec4 BlendMultiply(vec4 b, vec4 l) { return vec4(BlendMultiplyf(b.x, l.x), BlendMultiplyf(b.y, l.y), BlendMultiplyf(b.z, l.z), BlendMultiplyf(b.w, l.w)); }
vec2 BlendMultiply(vec2 b, float l) { return BlendMultiply(b, vec2(l)); }
vec3 BlendMultiply(vec3 b, float l) { return BlendMultiply(b, vec3(l)); }
vec4 BlendMultiply(vec4 b, float l) { return BlendMultiply(b, vec4(l)); }
vec2 BlendAverage(vec2 b, vec2 l) { return vec2(BlendAveragef(b.x, l.x), BlendAveragef(b.y, l.y)); }
vec3 BlendAverage(vec3 b, vec3 l) { return vec3(BlendAveragef(b.x, l.x), BlendAveragef(b.y, l.y), BlendAveragef(b.z, l.z)); }
vec4 BlendAverage(vec4 b, vec4 l) { return vec4(BlendAveragef(b.x, l.x), BlendAveragef(b.y, l.y), BlendAveragef(b.z, l.z), BlendAveragef(b.w, l.w)); }
vec2 BlendAverage(vec2 b, float l) { return BlendAverage(b, vec2(l)); }
vec3 BlendAverage(vec3 b, float l) { return BlendAverage(b, vec3(l)); }
vec4 BlendAverage(vec4 b, float l) { return BlendAverage(b, vec4(l)); }
vec2 BlendAdd(vec2 b, vec2 l) { return vec2(BlendAddf(b.x, l.x), BlendAddf(b.y, l.y)); }
vec3 BlendAdd(vec3 b, vec3 l) { return vec3(BlendAddf(b.x, l.x), BlendAddf(b.y, l.y), BlendAddf(b.z, l.z)); }
vec4 BlendAdd(vec4 b, vec4 l) { return vec4(BlendAddf(b.x, l.x), BlendAddf(b.y, l.y), BlendAddf(b.z, l.z), BlendAddf(b.w, l.w)); }
vec2 BlendAdd(vec2 b, float l) { return BlendAdd(b, vec2(l)); }
vec3 BlendAdd(vec3 b, float l) { return BlendAdd(b, vec3(l)); }
vec4 BlendAdd(vec4 b, float l) { return BlendAdd(b, vec4(l)); }
vec2 BlendSubstract(vec2 b, vec2 l) { return vec2(BlendSubstractf(b.x, l.x), BlendSubstractf(b.y, l.y)); }
vec3 BlendSubstract(vec3 b, vec3 l) { return vec3(BlendSubstractf(b.x, l.x), BlendSubstractf(b.y, l.y), BlendSubstractf(b.z, l.z)); }
vec4 BlendSubstract(vec4 b, vec4 l) { return vec4(BlendSubstractf(b.x, l.x), BlendSubstractf(b.y, l.y), BlendSubstractf(b.z, l.z), BlendSubstractf(b.w, l.w)); }
vec2 BlendSubstract(vec2 b, float l) { return BlendSubstract(b, vec2(l)); }
vec3 BlendSubstract(vec3 b, float l) { return BlendSubstract(b, vec3(l)); }
vec4 BlendSubstract(vec4 b, float l) { return BlendSubstract(b, vec4(l)); }
vec2 BlendDifference(vec2 b, vec2 l) { return vec2(BlendDifferencef(b.x, l.x), BlendDifferencef(b.y, l.y)); }
vec3 BlendDifference(vec3 b, vec3 l) { return vec3(BlendDifferencef(b.x, l.x), BlendDifferencef(b.y, l.y), BlendDifferencef(b.z, l.z)); }
vec4 BlendDifference(vec4 b, vec4 l) { return vec4(BlendDifferencef(b.x, l.x), BlendDifferencef(b.y, l.y), BlendDifferencef(b.z, l.z), BlendDifferencef(b.w, l.w)); }
vec2 BlendDifference(vec2 b, float l) { return BlendDifference(b, vec2(l)); }
vec3 BlendDifference(vec3 b, float l) { return BlendDifference(b, vec3(l)); }
vec4 BlendDifference(vec4 b, float l) { return BlendDifference(b, vec4(l)); }
vec2 BlendNegation(vec2 b, vec2 l) { return vec2(BlendNegationf(b.x, l.x), BlendNegationf(b.y, l.y)); }
vec3 BlendNegation(vec3 b, vec3 l) { return vec3(BlendNegationf(b.x, l.x), BlendNegationf(b.y, l.y), BlendNegationf(b.z, l.z)); }
vec4 BlendNegation(vec4 b, vec4 l) { return vec4(BlendNegationf(b.x, l.x), BlendNegationf(b.y, l.y), BlendNegationf(b.z, l.z), BlendNegationf(b.w, l.w)); }
vec2 BlendNegation(vec2 b, float l) { return BlendNegation(b, vec2(l)); }
vec3 BlendNegation(vec3 b, float l) { return BlendNegation(b, vec3(l)); }
vec4 BlendNegation(vec4 b, float l) { return BlendNegation(b, vec4(l)); }
vec2 BlendExclusion(vec2 b, vec2 l) { return vec2(BlendExclusionf(b.x, l.x), BlendExclusionf(b.y, l.y)); }
vec3 BlendExclusion(vec3 b, vec3 l) { return vec3(BlendExclusionf(b.x, l.x), BlendExclusionf(b.y, l.y), BlendExclusionf(b.z, l.z)); }
vec4 BlendExclusion(vec4 b, vec4 l) { return vec4(BlendExclusionf(b.x, l.x), BlendExclusionf(b.y, l.y), BlendExclusionf(b.z, l.z), BlendExclusionf(b.w, l.w)); }
vec2 BlendExclusion(vec2 b, float l) { return BlendExclusion(b, vec2(l)); }
vec3 BlendExclusion(vec3 b, float l) { return BlendExclusion(b, vec3(l)); }
vec4 BlendExclusion(vec4 b, float l) { return BlendExclusion(b, vec4(l)); }
vec2 BlendScreen(vec2 b, vec2 l) { return vec2(BlendScreenf(b.x, l.x), BlendScreenf(b.y, l.y)); }
vec3 BlendScreen(vec3 b, vec3 l) { return vec3(BlendScreenf(b.x, l.x), BlendScreenf(b.y, l.y), BlendScreenf(b.z, l.z)); }
vec4 BlendScreen(vec4 b, vec4 l) { return vec4(BlendScreenf(b.x, l.x), BlendScreenf(b.y, l.y), BlendScreenf(b.z, l.z), BlendScreenf(b.w, l.w)); }
vec2 BlendScreen(vec2 b, float l) { return BlendScreen(b, vec2(l)); }
vec3 BlendScreen(vec3 b, float l) { return BlendScreen(b, vec3(l)); }
vec4 BlendScreen(vec4 b, float l) { return BlendScreen(b, vec4(l)); }
vec2 BlendOverlay(vec2 b, vec2 l) { return vec2(BlendOverlayf(b.x, l.x), BlendOverlayf(b.y, l.y)); }
vec3 BlendOverlay(vec3 b, vec3 l) { return vec3(BlendOverlayf(b.x, l.x), BlendOverlayf(b.y, l.y), BlendOverlayf(b.z, l.z)); }
vec4 BlendOverlay(vec4 b, vec4 l) { return vec4(BlendOverlayf(b.x, l.x), BlendOverlayf(b.y, l.y), BlendOverlayf(b.z, l.z), BlendOverlayf(b.w, l.w)); }
vec2 BlendOverlay(vec2 b, float l) { return BlendOverlay(b, vec2(l)); }
vec3 BlendOverlay(vec3 b, float l) { return BlendOverlay(b, vec3(l)); }
vec4 BlendOverlay(vec4 b, float l) { return BlendOverlay(b, vec4(l)); }
vec2 BlendSoftLight(vec2 b, vec2 l) { return vec2(BlendSoftLightf(b.x, l.x), BlendSoftLightf(b.y, l.y)); }
vec3 BlendSoftLight(vec3 b, vec3 l) { return vec3(BlendSoftLightf(b.x, l.x), BlendSoftLightf(b.y, l.y), BlendSoftLightf(b.z, l.z)); }
vec4 BlendSoftLight(vec4 b, vec4 l) { return vec4(BlendSoftLightf(b.x, l.x), BlendSoftLightf(b.y, l.y), BlendSoftLightf(b.z, l.z), BlendSoftLightf(b.w, l.w)); }
vec2 BlendSoftLight(vec2 b, float l) { return BlendSoftLight(b, vec2(l)); }
vec3 BlendSoftLight(vec3 b, float l) { return BlendSoftLight(b, vec3(l)); }
vec4 BlendSoftLight(vec4 b, float l) { return BlendSoftLight(b, vec4(l)); }
vec2 BlendHardLight(vec2 b, vec2 l) { return vec2(BlendHardLightf(b.x, l.x), BlendHardLightf(b.y, l.y)); }
vec3 BlendHardLight(vec3 b, vec3 l) { return vec3(BlendHardLightf(b.x, l.x), BlendHardLightf(b.y, l.y), BlendHardLightf(b.z, l.z)); }
vec4 BlendHardLight(vec4 b, vec4 l) { return vec4(BlendHardLightf(b.x, l.x), BlendHardLightf(b.y, l.y), BlendHardLightf(b.z, l.z), BlendHardLightf(b.w, l.w)); }
vec2 BlendHardLight(vec2 b, float l) { return BlendHardLight(b, vec2(l)); }
vec3 BlendHardLight(vec3 b, float l) { return BlendHardLight(b, vec3(l)); }
vec4 BlendHardLight(vec4 b, float l) { return BlendHardLight(b, vec4(l)); }
vec2 BlendColorDodge(vec2 b, vec2 l) { return vec2(BlendColorDodgef(b.x, l.x), BlendColorDodgef(b.y, l.y)); }
vec3 BlendColorDodge(vec3 b, vec3 l) { return vec3(BlendColorDodgef(b.x, l.x), BlendColorDodgef(b.y, l.y), BlendColorDodgef(b.z, l.z)); }
vec4 BlendColorDodge(vec4 b, vec4 l) { return vec4(BlendColorDodgef(b.x, l.x), BlendColorDodgef(b.y, l.y), BlendColorDodgef(b.z, l.z), BlendColorDodgef(b.w, l.w)); }
vec2 BlendColorDodge(vec2 b, float l) { return BlendColorDodge(b, vec2(l)); }
vec3 BlendColorDodge(vec3 b, float l) { return BlendColorDodge(b, vec3(l)); }
vec4 BlendColorDodge(vec4 b, float l) { return BlendColorDodge(b, vec4(l)); }
vec2 BlendColorBurn(vec2 b, vec2 l) { return vec2(BlendColorBurnf(b.x, l.x), BlendColorBurnf(b.y, l.y)); }
vec3 BlendColorBurn(vec3 b, vec3 l) { return vec3(BlendColorBurnf(b.x, l.x), BlendColorBurnf(b.y, l.y), BlendColorBurnf(b.z, l.z)); }
vec4 BlendColorBurn(vec4 b, vec4 l) { return vec4(BlendColorBurnf(b.x, l.x), BlendColorBurnf(b.y, l.y), BlendColorBurnf(b.z, l.z), BlendColorBurnf(b.w, l.w)); }
vec2 BlendColorBurn(vec2 b, float l) { return BlendColorBurn(b, vec2(l)); }
vec3 BlendColorBurn(vec3 b, float l) { return BlendColorBurn(b, vec3(l)); }
vec4 BlendColorBurn(vec4 b, float l) { return BlendColorBurn(b, vec4(l)); }
vec2 BlendLinearLight(vec2 b, vec2 l) { return vec2(BlendLinearLightf(b.x, l.x), BlendLinearLightf(b.y, l.y)); }
vec3 BlendLinearLight(vec3 b, vec3 l) { return vec3(BlendLinearLightf(b.x, l.x), BlendLinearLightf(b.y, l.y), BlendLinearLightf(b.z, l.z)); }
vec4 BlendLinearLight(vec4 b, vec4 l) { return vec4(BlendLinearLightf(b.x, l.x), BlendLinearLightf(b.y, l.y), BlendLinearLightf(b.z, l.z), BlendLinearLightf(b.w, l.w)); }
vec2 BlendLinearLight(vec2 b, float l) { return BlendLinearLight(b, vec2(l)); }
vec3 BlendLinearLight(vec3 b, float l) { return BlendLinearLight(b, vec3(l)); }
vec4 BlendLinearLight(vec4 b, float l) { return BlendLinearLight(b, vec4(l)); }
vec2 BlendVividLight(vec2 b, vec2 l) { return vec2(BlendVividLightf(b.x, l.x), BlendVividLightf(b.y, l.y)); }
vec3 BlendVividLight(vec3 b, vec3 l) { return vec3(BlendVividLightf(b.x, l.x), BlendVividLightf(b.y, l.y), BlendVividLightf(b.z, l.z)); }
vec4 BlendVividLight(vec4 b, vec4 l) { return vec4(BlendVividLightf(b.x, l.x), BlendVividLightf(b.y, l.y), BlendVividLightf(b.z, l.z), BlendVividLightf(b.w, l.w)); }
vec2 BlendVividLight(vec2 b, float l) { return BlendVividLight(b, vec2(l)); }
vec3 BlendVividLight(vec3 b, float l) { return BlendVividLight(b, vec3(l)); }
vec4 BlendVividLight(vec4 b, float l) { return BlendVividLight(b, vec4(l)); }
vec2 BlendPinLight(vec2 b, vec2 l) { return vec2(BlendPinLightf(b.x, l.x), BlendPinLightf(b.y, l.y)); }
vec3 BlendPinLight(vec3 b, vec3 l) { return vec3(BlendPinLightf(b.x, l.x), BlendPinLightf(b.y, l.y), BlendPinLightf(b.z, l.z)); }
vec4 BlendPinLight(vec4 b, vec4 l) { return vec4(BlendPinLightf(b.x, l.x), BlendPinLightf(b.y, l.y), BlendPinLightf(b.z, l.z), BlendPinLightf(b.w, l.w)); }
vec2 BlendPinLight(vec2 b, float l) { return BlendPinLight(b, vec2(l)); }
vec3 BlendPinLight(vec3 b, float l) { return BlendPinLight(b, vec3(l)); }
vec4 BlendPinLight(vec4 b, float l) { return BlendPinLight(b, vec4(l)); }
vec2 BlendHardMix(vec2 b, vec2 l) { return vec2(BlendHardMixf(b.x, l.x), BlendHardMixf(b.y, l.y)); }
vec3 BlendHardMix(vec3 b, vec3 l) { return vec3(BlendHardMixf(b.x, l.x), BlendHardMixf(b.y, l.y), BlendHardMixf(b.z, l.z)); }
vec4 BlendHardMix(vec4 b, vec4 l) { return vec4(BlendHardMixf(b.x, l.x), BlendHardMixf(b.y, l.y), BlendHardMixf(b.z, l.z), BlendHardMixf(b.w, l.w)); }
vec2 BlendHardMix(vec2 b, float l) { return BlendHardMix(b, vec2(l)); }
vec3 BlendHardMix(vec3 b, float l) { return BlendHardMix(b, vec3(l)); }
vec4 BlendHardMix(vec4 b, float l) { return BlendHardMix(b, vec4(l)); }
vec2 BlendReflect(vec2 b, vec2 l) { return vec2(BlendReflectf(b.x, l.x), BlendReflectf(b.y, l.y)); }
vec3 BlendReflect(vec3 b, vec3 l) { return vec3(BlendReflectf(b.x, l.x), BlendReflectf(b.y, l.y), BlendReflectf(b.z, l.z)); }
vec4 BlendReflect(vec4 b, vec4 l) { return vec4(BlendReflectf(b.x, l.x), BlendReflectf(b.y, l.y), BlendReflectf(b.z, l.z), BlendReflectf(b.w, l.w)); }
vec2 BlendReflect(vec2 b, float l) { return BlendReflect(b, vec2(l)); }
vec3 BlendReflect(vec3 b, float l) { return BlendReflect(b, vec3(l)); }
vec4 BlendReflect(vec4 b, float l) { return BlendReflect(b, vec4(l)); }
vec2 BlendGlow(vec2 b, vec2 l) { return vec2(BlendGlowf(b.x, l.x), BlendGlowf(b.y, l.y)); }
vec3 BlendGlow(vec3 b, vec3 l) { return vec3(BlendGlowf(b.x, l.x), BlendGlowf(b.y, l.y), BlendGlowf(b.z, l.z)); }
vec4 BlendGlow(vec4 b, vec4 l) { return vec4(BlendGlowf(b.x, l.x), BlendGlowf(b.y, l.y), BlendGlowf(b.z, l.z), BlendGlowf(b.w, l.w)); }
vec2 BlendGlow(vec2 b, float l) { return BlendGlow(b, vec2(l)); }
vec3 BlendGlow(vec3 b, float l) { return BlendGlow(b, vec3(l)); }
vec4 BlendGlow(vec4 b, float l) { return BlendGlow(b, vec4(l)); }
vec2 BlendPhoenix(vec2 b, vec2 l) { return vec2(BlendPhoenixf(b.x, l.x), BlendPhoenixf(b.y, l.y)); }
vec3 BlendPhoenix(vec3 b, vec3 l) { return vec3(BlendPhoenixf(b.x, l.x), BlendPhoenixf(b.y, l.y), BlendPhoenixf(b.z, l.z)); }
vec4 BlendPhoenix(vec4 b, vec4 l) { return vec4(BlendPhoenixf(b.x, l.x), BlendPhoenixf(b.y, l.y), BlendPhoenixf(b.z, l.z), BlendPhoenixf(b.w, l.w)); }
vec2 BlendPhoenix(vec2 b, float l) { return BlendPhoenix(b, vec2(l)); }
vec3 BlendPhoenix(vec3 b, float l) { return BlendPhoenix(b, vec3(l)); }
vec4 BlendPhoenix(vec4 b, float l) { return BlendPhoenix(b, vec4(l)); }
vec2 BlendLinearDodge(vec2 b, vec2 l) { return vec2(BlendLinearDodgef(b.x, l.x), BlendLinearDodgef(b.y, l.y)); }
vec3 BlendLinearDodge(vec3 b, vec3 l) { return vec3(BlendLinearDodgef(b.x, l.x), BlendLinearDodgef(b.y, l.y), BlendLinearDodgef(b.z, l.z)); }
vec4 BlendLinearDodge(vec4 b, vec4 l) { return vec4(BlendLinearDodgef(b.x, l.x), BlendLinearDodgef(b.y, l.y), BlendLinearDodgef(b.z, l.z), BlendLinearDodgef(b.w, l.w)); }
vec2 BlendLinearDodge(vec2 b, float l) { return BlendLinearDodge(b, vec2(l)); }
vec3 BlendLinearDodge(vec3 b, float l) { return BlendLinearDodge(b, vec3(l)); }
vec4 BlendLinearDodge(vec4 b, float l) { return BlendLinearDodge(b, vec4(l)); }
vec2 BlendLinearBurn(vec2 b, vec2 l) { return vec2(BlendLinearBurnf(b.x, l.x), BlendLinearBurnf(b.y, l.y)); }
vec3 BlendLinearBurn(vec3 b, vec3 l) { return vec3(BlendLinearBurnf(b.x, l.x), BlendLinearBurnf(b.y, l.y), BlendLinearBurnf(b.z, l.z)); }
vec4 BlendLinearBurn(vec4 b, vec4 l) { return vec4(BlendLinearBurnf(b.x, l.x), BlendLinearBurnf(b.y, l.y), BlendLinearBurnf(b.z, l.z), BlendLinearBurnf(b.w, l.w)); }
vec2 BlendLinearBurn(vec2 b, float l) { return BlendLinearBurn(b, vec2(l)); }
vec3 BlendLinearBurn(vec3 b, float l) { return BlendLinearBurn(b, vec3(l)); }
vec4 BlendLinearBurn(vec4 b, float l) { return BlendLinearBurn(b, vec4(l)); }
#define BlendOpacity(base, blend, F, opacity) mix(base, F(base, blend), opacity)
#endif
