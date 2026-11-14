// TruongPaper clean-room implementation of the stock "iris" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform vec2 g_PointerPosition;
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.02,"range":[0, 0.2]}
uniform vec2 g_Bounds; // {"material":"bounds","label":"ui_editor_properties_bounds","default":"1 1"}

// The masked area (an iris) shifts towards the cursor.
void main() {
	float mask = texSample2D(g_Texture1, v_TexCoord).r;
	vec2 look = clamp((g_PointerPosition - 0.5) * 2.0, -1.0, 1.0) * g_Bounds;
	gl_FragColor = texSample2D(g_Texture0, v_TexCoord - look * (g_Strength * mask));
}
