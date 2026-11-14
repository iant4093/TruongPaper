// TruongPaper clean-room implementation of the stock "filmgrain" effect interface.
// [COMBO] {"material":"ui_editor_properties_blend_mode","combo":"BLENDMODE","type":"imageblending","default":12}
// [COMBO] {"material":"ui_editor_properties_greyscale","combo":"GREYSCALE","type":"options","default":1}

#include "common.h"
#include "common_blending.h"

varying vec2 v_TexCoord;
varying vec4 v_NoiseCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"noise","label":"ui_editor_properties_noise","default":"util/noise"}
uniform sampler2D g_Texture2; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","combo":"MASK","paintdefaultcolor":"0 0 0 1"}
uniform float g_NoiseAlpha; // {"material":"strength","label":"ui_editor_properties_strength","default":2,"range":[0.0, 5.0]}
uniform float g_NoisePower; // {"material":"exponent","label":"ui_editor_properties_power","default":0.5,"range":[0.0, 5.0]}

void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord);
	vec3 grain = texSample2D(g_Texture1, v_NoiseCoord.xy).rgb * texSample2D(g_Texture1, v_NoiseCoord.zw).brg;
#if GREYSCALE == 1
	grain = vec3(greyscale(grain));
#endif
	grain = pow(saturate(grain * 2.0), vec3(g_NoisePower));
	float amount = saturate(g_NoiseAlpha * 0.25);
#if MASK == 1
	amount *= texSample2D(g_Texture2, v_TexCoord).r;
#endif
	color.rgb = ApplyBlending(BLENDMODE, color.rgb, grain, amount);
	gl_FragColor = color;
}
