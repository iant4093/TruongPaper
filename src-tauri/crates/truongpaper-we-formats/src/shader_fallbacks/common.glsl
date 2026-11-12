#ifndef TRUONGPAPER_COMMON_GLSL
#define TRUONGPAPER_COMMON_GLSL
#define M_PI 3.14159265359
#define M_PI_HALF 1.57079632679
#define M_PI_2 6.28318530718
#define SQRT_2 1.41421356237
#define SQRT_3 1.73205080756

vec2 rotateVec2(vec2 v, float angle) {
    float c = cos(angle);
    float s = sin(angle);
    return vec2(c * v.x - s * v.y, s * v.x + c * v.y);
}

float greyscale(vec3 rgb) {
    return dot(rgb, vec3(0.299, 0.587, 0.114));
}
#endif
