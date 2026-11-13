// TruongPaper clean-room implementation of the stock "clouds" effect interface.
#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Time;
uniform vec4 g_Texture0Resolution;
uniform vec3 g_CloudColor; // {"material":"color","label":"ui_editor_properties_color","default":"1 1 1","type":"color"}
uniform float g_Density; // {"material":"density","label":"ui_editor_properties_density","default":0.5,"range":[0, 1]}
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":0.02,"range":[0, 0.5]}
uniform float g_Scale; // {"material":"scale","label":"ui_editor_properties_scale","default":3,"range":[0.1, 20]}
uniform float g_CloudAlpha; // {"material":"alpha","label":"ui_editor_properties_alpha","default":1,"range":[0, 1]}

// Procedural drifting cloud layer painted over the image.
void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord);
	float aspect = g_Texture0Resolution.z / max(g_Texture0Resolution.w, 1.0);
	vec2 p = v_TexCoord * vec2(aspect, 1.0) * g_Scale + vec2(g_Time * g_Speed, 0.0);
	float n = tpFbmNoise(p) * 0.6 + tpFbmNoise(p * 2.3 - vec2(g_Time * g_Speed * 0.5, 0.0)) * 0.4;
	float cover = smoothstep(1.0 - g_Density, 1.0 - g_Density + 0.35, n);
	float amount = cover * g_CloudAlpha * texSample2D(g_Texture1, v_TexCoord).r;
	color.rgb = mix(color.rgb, g_CloudColor, amount);
	color.a = max(color.a, amount);
	gl_FragColor = color;
}
