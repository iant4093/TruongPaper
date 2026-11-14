// TruongPaper clean-room implementation of the stock "perspective" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}

void main() {
	gl_FragColor = texSample2D(g_Texture0, v_TexCoord);
}
