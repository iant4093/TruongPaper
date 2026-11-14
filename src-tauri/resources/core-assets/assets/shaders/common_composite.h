#ifndef TRUONGPAPER_COMPOSITE_GLSL
#define TRUONGPAPER_COMPOSITE_GLSL
// Clean-room composite helpers used by two-stage effects (blur/combine): the processed result
// ("layer") is merged with the original image ("base") according to the COMPOSITE combo.
#include "common_blending.h"

#ifndef COMPOSITE
#define COMPOSITE 0
#endif
#ifndef COMPOSITEMONO
#define COMPOSITEMONO 0
#endif
#ifndef BLENDMODE
#define BLENDMODE 0
#endif

// Sampling offset for the processed texture; both halves share one coordinate space here.
vec2 ApplyCompositeOffset(vec2 coords, vec2 resolution) {
    return coords;
}

vec4 ApplyComposite(vec4 base, vec4 layer) {
#if COMPOSITEMONO
    layer.rgb = vec3(dot(layer.rgb, vec3(0.299, 0.587, 0.114)));
#endif
#if COMPOSITE == 1
    vec3 rgb = ApplyBlending(BLENDMODE, base.rgb, layer.rgb, 1.0);
    return vec4(mix(base.rgb, rgb, layer.a), max(base.a, layer.a));
#elif COMPOSITE == 2
    float a = base.a + layer.a * (1.0 - base.a);
    vec3 rgb = (base.rgb * base.a + layer.rgb * layer.a * (1.0 - base.a)) / max(a, 0.0001);
    return vec4(rgb, a);
#elif COMPOSITE == 3
    return vec4(base.rgb, base.a * layer.a);
#else
    return layer;
#endif
}
#endif
