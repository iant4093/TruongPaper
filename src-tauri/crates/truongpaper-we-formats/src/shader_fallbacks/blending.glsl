#ifndef TRUONGPAPER_BLEND_GLSL
#define TRUONGPAPER_BLEND_GLSL
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
    // 30 is the effect default of `tint`: the colour multiplies the image (white = no change).
    else if (mode == 30) result = base * layer;
    if (mode == 5 || mode == 10) return result;
    return mix(base, result, amount);
}
#endif
