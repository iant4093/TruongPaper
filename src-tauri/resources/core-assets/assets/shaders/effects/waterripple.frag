// TruongPaper clean-room implementation of the stock "waterripple" effect interface.
// [COMBO] {"material":"ui_editor_properties_specular","combo":"SPECULAR","type":"options","default":0}

varying vec2 v_TexCoord;
varying vec4 v_RippleCoord;

uniform sampler2D g_Texture0; // {"material":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"ui_editor_properties_water_normal","default":"effects/waterripplenormal"}
uniform sampler2D g_Texture2; // {"material":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white"}
uniform float g_Strength; // {"material":"ui_editor_properties_ripple_strength","default":0.1,"range":[0,1]}
uniform float g_SpecularPower; // {"material":"ui_editor_properties_ripple_specular_power","default":1.0,"range":[0,100]}
uniform float g_SpecularStrength; // {"material":"ui_editor_properties_ripple_specular_strength","default":1.0,"range":[0,10]}
uniform vec3 g_SpecularColor; // {"material":"ui_editor_properties_ripple_specular_color","default":"1 1 1","type":"color"}

void main() {
	float mask = texSample2D(g_Texture2, v_TexCoord).r;
	vec2 a = texSample2D(g_Texture1, v_RippleCoord.xy).xy * 2.0 - 1.0;
	vec2 b = texSample2D(g_Texture1, v_RippleCoord.zw).xy * 2.0 - 1.0;
	vec2 bend = (a + b) * 0.5;
	vec4 color = texSample2D(g_Texture0, v_TexCoord + bend * (g_Strength * g_Strength * mask));
#if SPECULAR == 1
	// Highlights on ripple slopes that face up towards the sky.
	float facing = max(0.0, -bend.y);
	color.rgb += g_SpecularColor * (pow(facing, max(g_SpecularPower, 0.001)) * g_SpecularStrength * mask * color.a);
#endif
	gl_FragColor = color;
}
