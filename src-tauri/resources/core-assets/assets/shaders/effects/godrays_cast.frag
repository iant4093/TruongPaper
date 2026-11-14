// TruongPaper clean-room implementation of the stock "godrays" effect interface.
// [COMBO] {"material":"ui_editor_properties_cast_type","combo":"CASTER","type":"options","default":0,"options":{"Radial":0,"Directional":1}}
// [COMBO] {"material":"ui_editor_properties_quality","combo":"SAMPLES","type":"options","default":0,"options":{"30":0,"50":1}}

#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform float g_Length; // {"material":"raylength","label":"ui_editor_properties_ray_length","default":0.5,"range":[0.01, 1]}
uniform float g_Intensity; // {"material":"rayintensity","label":"ui_editor_properties_ray_intensity","default":1,"range":[0.01, 2.0]}
uniform vec3 g_ColorRays; // {"material":"color","label":"ui_editor_properties_color","default":"1 1 1","type":"color"}
#if CASTER == 0
uniform vec2 g_Center; // {"material":"center","label":"ui_editor_properties_center","default":"0.5 0.5","position":true}
#else
uniform float g_Direction; // {"material":"direction","label":"ui_editor_properties_direction","default":3.14159265358,"range":[0,6.28],"direction":true}
#endif

// Gathers light along the line towards the light source; nearer samples weigh more, so the
// bright areas smear outwards into rays that fade with distance.
void main() {
#if SAMPLES == 1
	const int taps = 50;
#else
	const int taps = 30;
#endif
#if CASTER == 0
	vec2 toLight = g_Center - v_TexCoord;
#else
	vec2 toLight = rotateVec2(vec2(0.0, 0.5), g_Direction);
#endif
	vec2 reach = toLight * g_Length;
	vec4 sum = vec4(0.0);
	float total = 0.0;
	for (int i = 0; i < taps; ++i) {
		float t = float(i) / float(taps - 1);
		float w = 1.0 - t;
		sum += texSample2D(g_Texture0, v_TexCoord + reach * t) * w;
		total += w;
	}
	sum *= 1.5 / total;
	gl_FragColor = vec4(sum.rgb * g_ColorRays * g_Intensity, saturate(sum.a * g_Intensity));
}
