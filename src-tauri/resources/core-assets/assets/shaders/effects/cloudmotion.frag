// TruongPaper clean-room implementation of the stock "cloudmotion" effect interface.
#include "common.h"

varying vec2 v_TexCoord;
varying vec2 v_NoiseCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"label":"ui_editor_properties_opacity_mask","mode":"opacitymask","combo":"MASK","paintdefaultcolor":"0 0 0 1"}
uniform sampler2D g_Texture2; // {"label":"ui_editor_properties_perlin_noise","default":"util/perlin_256"}
uniform float u_amount; // {"material":"ui_editor_properties_amount","default":0.1,"range":[0,0.2]}
uniform float u_direction; // {"material":"ui_editor_properties_direction","default":1.57079632679,"direction":true,"conversion":"rad2deg"}

// Drifting noise pushes pixels back and forth across `direction`, so clouds and smoke billow.
void main() {
	float mask = 1.0;
#if MASK
	mask = texSample2D(g_Texture1, v_TexCoord).r;
#endif
	float n = texSample2D(g_Texture2, v_NoiseCoord).r * 2.0 - 1.0;
	vec2 shift = rotateVec2(vec2(n * u_amount * mask, 0.0), u_direction + M_PI_HALF);
	gl_FragColor = texSample2D(g_Texture0, v_TexCoord + shift);
}
