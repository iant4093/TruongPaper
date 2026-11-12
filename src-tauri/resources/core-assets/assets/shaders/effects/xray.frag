// TruongPaper clean-room implementation of the stock "xray" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"albedo","label":"ui_editor_properties_albedo","default":"util/black"}
uniform vec2 g_PointerPosition;
uniform vec4 g_Texture0Resolution;
uniform float g_Radius; // {"material":"radius","label":"ui_editor_properties_radius","default":0.15,"range":[0, 1]}
uniform float g_Feather; // {"material":"feather","label":"ui_editor_properties_feather","default":0.05,"range":[0, 0.5]}

// Reveals the second texture in a soft circle around the cursor.
void main() {
	float aspect = g_Texture0Resolution.z / max(g_Texture0Resolution.w, 1.0);
	float d = length((v_TexCoord - g_PointerPosition) * vec2(aspect, 1.0));
	float reveal = 1.0 - smoothstep(g_Radius, g_Radius + g_Feather + 0.0001, d);
	gl_FragColor = mix(texSample2D(g_Texture0, v_TexCoord), texSample2D(g_Texture1, v_TexCoord), reveal);
}
