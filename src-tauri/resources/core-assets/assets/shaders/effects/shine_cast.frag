// TruongPaper clean-room implementation of the stock "shine" effect interface.
// [COMBO] {"material":"ui_editor_properties_quality","combo":"SAMPLES","type":"options","default":1,"options":{"4":0,"8":1,"15":2,"30":3}}
// [COMBO] {"material":"ui_editor_properties_edges","combo":"EDGES","type":"options","default":4,"options":{"2":2,"3":3,"4":4,"5":5}}

#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform float g_Time;
uniform float g_Length; // {"material":"raylength","label":"ui_editor_properties_ray_length","default":0.1,"range":[0.01, 1]}
uniform float g_Intensity; // {"material":"rayintensity","label":"ui_editor_properties_ray_intensity","default":1,"range":[0.01, 2.0]}
uniform vec3 g_ColorRays; // {"material":"color","label":"ui_editor_properties_color","default":"1 1 1","type":"color"}
uniform float g_Direction; // {"material":"direction","label":"ui_editor_properties_direction","default":0,"direction":true}
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":0,"range":[-1,1]}

// Star-shaped glints: every bright spot is smeared along EDGES evenly spaced directions.
void main() {
#if SAMPLES == 0
	const int taps = 4;
#elif SAMPLES == 2
	const int taps = 15;
#elif SAMPLES == 3
	const int taps = 30;
#else
	const int taps = 8;
#endif
	float angle0 = g_Direction + g_Time * g_Speed;
	vec4 sum = vec4(0.0);
	float total = 0.0;
	for (int e = 0; e < EDGES * 2; ++e) {
		vec2 dir = rotateVec2(vec2(0.0, g_Length), angle0 + M_PI * float(e) / float(EDGES));
		for (int i = 1; i <= taps; ++i) {
			float t = float(i) / float(taps);
			float w = 1.0 - t;
			sum += texSample2D(g_Texture0, v_TexCoord + dir * t) * w;
			total += w;
		}
	}
	sum *= float(EDGES) / max(total, 0.0001);
	gl_FragColor = vec4(sum.rgb * g_ColorRays * g_Intensity, saturate(sum.a * g_Intensity));
}
