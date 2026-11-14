// TruongPaper clean-room implementation of the stock "foliagesway" effect interface.
#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Time;
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":1,"range":[0, 10]}
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.01,"range":[0, 0.1]}
uniform float g_Scale; // {"material":"scale","label":"ui_editor_properties_scale","default":3,"range":[0.1, 20]}
uniform float g_Direction; // {"material":"direction","label":"ui_editor_properties_direction","default":1.57079632679,"direction":true}

// Gusts of wind: neighbouring leaves swing with slightly different timing (noise phase).
void main() {
	float mask = texSample2D(g_Texture1, v_TexCoord).r;
	float phase = tpValueNoise(v_TexCoord * g_Scale) * M_PI_2;
	float t = g_Time * g_Speed;
	float sway = sin(t + phase) * 0.7 + sin(t * 2.3 + phase * 1.7) * 0.3;
	vec2 dir = rotateVec2(vec2(0.0, 1.0), g_Direction);
	gl_FragColor = texSample2D(g_Texture0, v_TexCoord + dir * (sway * g_Strength * mask));
}
