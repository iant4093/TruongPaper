// TruongPaper clean-room implementation of the stock "pulse" effect interface.
// [COMBO] {"material":"ui_editor_properties_mode","combo":"MODE","type":"options","default":0,"options":{"Alpha":0,"Brightness":1,"Color":2}}

#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Time;
uniform float g_PulseSpeed; // {"material":"speed","label":"ui_editor_properties_speed","default":1,"range":[0.01, 10]}
uniform float g_PulseAmount; // {"material":"amount","label":"ui_editor_properties_amount","default":0.5,"range":[0, 1]}
uniform float g_PulseExponent; // {"material":"exponent","label":"ui_editor_properties_exponent","default":1,"range":[0.1, 8]}
uniform vec3 g_PulseColor; // {"material":"color","label":"ui_editor_properties_color","default":"1 1 1","type":"color"}

void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord);
	float mask = texSample2D(g_Texture1, v_TexCoord).r;
	float wave = pow(0.5 + 0.5 * sin(g_Time * g_PulseSpeed * M_PI_2), g_PulseExponent) * g_PulseAmount * mask;
#if MODE == 1
	color.rgb *= 1.0 + wave;
#elif MODE == 2
	color.rgb = mix(color.rgb, g_PulseColor * greyscale(color.rgb) * 2.0, wave);
#else
	color.a *= 1.0 - wave;
#endif
	gl_FragColor = color;
}
