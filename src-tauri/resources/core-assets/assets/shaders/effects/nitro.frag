// TruongPaper clean-room implementation of the stock "nitro" effect interface.
#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Time;
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.5,"range":[0, 1]}
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":1,"range":[0, 10]}
uniform float g_Scale; // {"material":"scale","label":"ui_editor_properties_scale","default":20,"range":[1, 100]}

// Digital glitch: random horizontal slices jump sideways and split their colour channels.
void main() {
	float mask = texSample2D(g_Texture1, v_TexCoord).r;
	float tick = floor(g_Time * g_Speed * 8.0);
	float slice = floor(v_TexCoord.y * g_Scale);
	float pick = tpHash12(vec2(slice, tick));
	float active = step(1.0 - g_Strength * 0.35, pick) * mask;
	float shift = (tpHash12(vec2(tick, slice + 7.0)) - 0.5) * 0.1 * g_Strength * active;
	vec2 uv = v_TexCoord + vec2(shift, 0.0);
	vec4 color = texSample2D(g_Texture0, uv);
	color.r = texSample2D(g_Texture0, uv + vec2(shift * 0.5, 0.0)).r;
	color.b = texSample2D(g_Texture0, uv - vec2(shift * 0.5, 0.0)).b;
	gl_FragColor = color;
}
