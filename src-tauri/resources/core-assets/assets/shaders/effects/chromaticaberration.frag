// TruongPaper clean-room implementation of the stock "chromaticaberration" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.01,"range":[0, 0.1]}
uniform vec2 g_Center; // {"material":"center","label":"ui_editor_properties_center","default":"0.5 0.5","position":true}

// Red and blue are scaled away from / towards the centre, like a cheap lens.
void main() {
	vec2 d = (v_TexCoord - g_Center) * (g_Strength * texSample2D(g_Texture1, v_TexCoord).r);
	vec4 g = texSample2D(g_Texture0, v_TexCoord);
	float r = texSample2D(g_Texture0, v_TexCoord + d).r;
	float b = texSample2D(g_Texture0, v_TexCoord - d).b;
	gl_FragColor = vec4(r, g.g, b, g.a);
}
