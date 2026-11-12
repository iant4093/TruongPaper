// TruongPaper clean-room implementation of the stock "twirl" effect interface.
#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform vec4 g_Texture0Resolution;
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":1,"range":[-10, 10]}
uniform float g_Size; // {"material":"size","label":"ui_editor_properties_size","default":0.5,"range":[0.01, 1]}
uniform vec2 g_Center; // {"material":"center","label":"ui_editor_properties_center","default":"0.5 0.5","position":true}

// Rotates `uv` around `center` by `angle`, correcting for the texture aspect ratio.
vec2 rotateAround(vec2 uv, vec2 center, float angle, float aspect) {
	vec2 p = (uv - center) * vec2(aspect, 1.0);
	p = rotateVec2(p, angle);
	return p / vec2(aspect, 1.0) + center;
}

void main() {
	float aspect = g_Texture0Resolution.z / max(g_Texture0Resolution.w, 1.0);
	float r = length((v_TexCoord - g_Center) * vec2(aspect, 1.0));
	float falloff = 1.0 - saturate(r / g_Size);
	float angle = g_Strength * falloff * falloff * texSample2D(g_Texture1, v_TexCoord).r;
	gl_FragColor = texSample2D(g_Texture0, rotateAround(v_TexCoord, g_Center, angle, aspect));
}
