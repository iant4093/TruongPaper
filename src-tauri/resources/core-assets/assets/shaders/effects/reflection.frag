// TruongPaper clean-room implementation of the stock "reflection" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Offset; // {"material":"offset","label":"ui_editor_properties_offset","default":0.5,"range":[0, 1]}
uniform float g_ReflectionAlpha; // {"material":"alpha","label":"ui_editor_properties_alpha","default":0.5,"range":[0, 1]}
uniform float g_Fade; // {"material":"fade","label":"ui_editor_properties_fade","default":1,"range":[0, 1]}

// Mirrors the image above `offset` into the area below it, fading with distance.
void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord);
	float below = v_TexCoord.y - g_Offset;
	if (below > 0.0) {
		vec4 mirrored = texSample2D(g_Texture0, vec2(v_TexCoord.x, g_Offset - below));
		float fade = 1.0 - saturate(below / max(1.0 - g_Offset, 0.0001)) * g_Fade;
		float amount = g_ReflectionAlpha * fade * texSample2D(g_Texture1, v_TexCoord).r;
		color = mix(color, mirrored, amount);
	}
	gl_FragColor = color;
}
