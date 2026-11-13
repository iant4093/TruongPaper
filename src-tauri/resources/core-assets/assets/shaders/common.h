// TruongPaper clean-room replacement for the Wallpaper Engine "common.h" include.
// Written from the public shader interface; not derived from Wallpaper Engine's files.
#ifndef TP_COMMON_H
#define TP_COMMON_H

#define M_PI 3.14159265359
#define M_PI_HALF 1.57079632679
#define M_PI_2 6.28318530718
#define SQRT_2 1.41421356237
#define SQRT_3 1.73205080756

// Rotates a 2D vector counter-clockwise by `angle` radians.
vec2 rotateVec2(vec2 v, float angle) {
    float c = cos(angle);
    float s = sin(angle);
    return vec2(c * v.x - s * v.y, s * v.x + c * v.y);
}

float greyscale(vec3 rgb) {
    return dot(rgb, vec3(0.299, 0.587, 0.114));
}

vec3 rgb2hsv(vec3 c) {
    vec4 k = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    vec4 p = mix(vec4(c.bg, k.wz), vec4(c.gb, k.xy), step(c.b, c.g));
    vec4 q = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
    float d = q.x - min(q.w, q.y);
    float e = 1.0e-10;
    return vec3(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

vec3 hsv2rgb(vec3 c) {
    vec3 p = abs(fract(c.xxx + vec3(1.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0);
    return c.z * mix(vec3(1.0), clamp(p - 1.0, 0.0, 1.0), c.y);
}

// Brightness scales, saturation blends from grey, contrast blends from mid grey.
vec3 ContrastSaturationBrightness(vec3 color, float brt, float sat, float con) {
    vec3 brtColor = color * brt;
    vec3 intensity = vec3(dot(brtColor, vec3(0.2125, 0.7154, 0.0721)));
    vec3 satColor = mix(intensity, brtColor, sat);
    return mix(vec3(0.5), satColor, con);
}

// Cheap hash noise in [0, 1).
float tpHash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Smooth value noise in [0, 1], continuous in `p`.
float tpValueNoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = tpHash12(i);
    float b = tpHash12(i + vec2(1.0, 0.0));
    float c = tpHash12(i + vec2(0.0, 1.0));
    float d = tpHash12(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Two-octave value noise in [0, 1], used where stock shaders sample a tiling noise texture.
float tpFbmNoise(vec2 p) {
    return 0.65 * tpValueNoise(p) + 0.35 * tpValueNoise(p * 2.07 + 17.3);
}

#endif
