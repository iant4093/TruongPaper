// TruongPaper clean-room implementation of the stock "refraction" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"normal","label":"ui_editor_properties_normal_map","default":"util/flatnormal"}
uniform sampler2D g_Texture2; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white"}
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.05,"range":[0, 0.5]}

void main() {
	vec2 bend = texSample2D(g_Texture1, v_TexCoord).xy * 2.0 - 1.0;
	float mask = texSample2D(g_Texture2, v_TexCoord).r;
	gl_FragColor = texSample2D(g_Texture0, v_TexCoord + bend * (g_Strength * mask));
}
