// TruongPaper clean-room implementation of the stock "shake" effect interface.
// [COMBO] {"material":"ui_editor_properties_noise","combo":"NOISE","type":"options","default":0}
// [COMBO] {"material":"ui_editor_properties_direction","combo":"DIRECTION","type":"options","default":0,"options":{"ui_editor_properties_center":0,"ui_editor_properties_left":1,"ui_editor_properties_right":2}}

#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"hidden":true}
uniform sampler2D g_Texture1; // {"label":"ui_editor_properties_shake_direction_map","mode":"flowmask","default":"util/noflow"}
uniform sampler2D g_Texture2; // {"label":"ui_editor_properties_time_offset","mode":"opacitymask","default":"util/black","combo":"TIMEOFFSET"}
uniform sampler2D g_Texture3; // {"label":"ui_editor_properties_opacity","mode":"opacitymask","combo":"MASK"}
uniform float g_Time;
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":1,"range":[0.0, 10]}
uniform float g_Amp; // {"material":"strength","label":"ui_editor_properties_strength","default":0.1,"range":[0.01, 0.5]}
uniform vec2 g_Friction; // {"material":"friction","label":"ui_editor_properties_friction","default":"1 1","linked":true,"range":[0.01, 10.0]}
uniform vec2 g_Bounds; // {"material":"bounds","label":"ui_editor_properties_bounds","default":"0 1"}

// Pixels oscillate along the painted direction map. Friction eases the two halves of the swing
// (x: moving out, y: coming back); bounds clip the swing range.
void main() {
	float phase = 0.0;
#if TIMEOFFSET
	phase = texSample2D(g_Texture2, v_TexCoord).r * M_PI_2;
#endif
	vec2 flow = (texSample2D(g_Texture1, v_TexCoord).rg - 0.5) * 2.0;
	float t = g_Speed * g_Time + phase;
#if NOISE
	float wave = (sin(t) + 0.5 * sin(t * 2.17 + 1.3) + 0.25 * sin(t * 4.31 + 2.1)) / 1.75;
#else
	float wave = sin(t);
#endif
	float s = wave * 0.5 + 0.5;
	s = mix(1.0 - pow(1.0 - s, g_Friction.x), pow(s, g_Friction.y), step(0.0, cos(t)));
	s = saturate((s - g_Bounds.x) / max(g_Bounds.y - g_Bounds.x, 0.0001));
#if DIRECTION == 1
	float offset = s;
#elif DIRECTION == 2
	float offset = s - 1.0;
#else
	float offset = s * 2.0 - 1.0;
#endif
	vec2 shift = flow * (offset * g_Amp * g_Amp);
	vec4 color = texSample2D(g_Texture0, v_TexCoord + shift);
#if MASK
	float mask = texSample2D(g_Texture3, v_TexCoord + shift).r;
	color = mix(texSample2D(g_Texture0, v_TexCoord), color, mask);
#endif
	gl_FragColor = color;
}
