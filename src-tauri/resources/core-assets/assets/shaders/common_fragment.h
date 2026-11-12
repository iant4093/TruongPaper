// TruongPaper clean-room replacement for the Wallpaper Engine "common_fragment.h" include:
// small fragment-stage helpers shared by effects.
#ifndef TP_COMMON_FRAGMENT_H
#define TP_COMMON_FRAGMENT_H

// Perceptual luminance of a colour.
float luminance(vec3 rgb) {
    return dot(rgb, vec3(0.2126, 0.7152, 0.0722));
}

// Maps a 0..1 value through a two-colour gradient.
vec3 gradient2(vec3 a, vec3 b, float t) {
    return mix(a, b, clamp(t, 0.0, 1.0));
}

#endif
