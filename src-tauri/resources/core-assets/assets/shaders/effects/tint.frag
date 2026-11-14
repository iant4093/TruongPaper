// TruongPaper clean-room implementation of the stock "tint" effect interface.
// [COMBO] {"material":"ui_editor_properties_blend_mode","combo":"BLENDMODE","type":"imageblending","default":0}

#include "common_blending.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform vec3 g_TintColor; // {"material":"color","label":"ui_editor_properties_color","default":"1 0 0","type":"color"}
uniform float g_TintAlpha; // {"material":"alpha","label":"ui_editor_properties_alpha","default":1,"range":[0,1]}

void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord);
	float amount = g_TintAlpha * texSample2D(g_Texture1, v_TexCoord).r;
	color.rgb = ApplyBlending(BLENDMODE, color.rgb, g_TintColor, amount);
	gl_FragColor = color;
}
