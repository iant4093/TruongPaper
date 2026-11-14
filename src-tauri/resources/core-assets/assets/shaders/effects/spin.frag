// TruongPaper clean-room implementation of the stock "spin" effect interface.
#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Time;
uniform vec4 g_Texture0Resolution;
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":1,"range":[-10, 10]}
uniform vec2 g_Center; // {"material":"center","label":"ui_editor_properties_center","default":"0.5 0.5","position":true}

// Rotates `uv` around `center` by `angle`, correcting for the texture aspect ratio.
vec2 rotateAround(vec2 uv, vec2 center, float angle, float aspect) {
	vec2 p = (uv - center) * vec2(aspect, 1.0);
	p = rotateVec2(p, angle);
	return p / vec2(aspect, 1.0) + center;
}

void main() {
	float aspect = g_Texture0Resolution.z / max(g_Texture0Resolution.w, 1.0);
	vec2 uv = rotateAround(v_TexCoord, g_Center, -g_Time * g_Speed, aspect);
	uv = mix(v_TexCoord, uv, texSample2D(g_Texture1, v_TexCoord).r);
	gl_FragColor = texSample2D(g_Texture0, uv);
}
