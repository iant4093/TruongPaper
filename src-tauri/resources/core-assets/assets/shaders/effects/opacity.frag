// TruongPaper clean-room implementation of the stock "opacity" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white"}
uniform float g_Opacity; // {"material":"opacity","label":"ui_editor_properties_opacity","default":1,"range":[0,1]}

void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord);
	color.a *= texSample2D(g_Texture1, v_TexCoord).r * g_Opacity;
	gl_FragColor = color;
}
