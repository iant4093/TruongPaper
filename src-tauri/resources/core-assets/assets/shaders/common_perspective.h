#ifndef TRUONGPAPER_PERSPECTIVE_GLSL
#define TRUONGPAPER_PERSPECTIVE_GLSL
mat3 squareToQuad(vec2 topLeft, vec2 topRight, vec2 bottomRight, vec2 bottomLeft) {
    vec2 d1 = topRight - bottomRight;
    vec2 d2 = bottomLeft - bottomRight;
    vec2 correction = topLeft - topRight + bottomRight - bottomLeft;
    float denominator = d1.x * d2.y - d2.x * d1.y;
    vec2 projective = vec2(0.0);
    if (abs(denominator) > 0.000001) {
        projective = vec2(correction.x * d2.y - d2.x * correction.y,
                          d1.x * correction.y - correction.x * d1.y) / denominator;
    }
    return mat3(
        vec3(topRight * (1.0 + projective.x) - topLeft, projective.x),
        vec3(bottomLeft * (1.0 + projective.y) - topLeft, projective.y),
        vec3(topLeft, 1.0)
    );
}
#endif
