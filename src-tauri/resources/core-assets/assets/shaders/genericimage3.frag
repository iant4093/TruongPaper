// TruongPaper clean-room base shader for image layers ("genericimage3" interface):
// the layer texture tinted by the layer colour and brightness, faded by its alpha.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"albedo","label":"ui_editor_properties_albedo","default":"util/white"}
uniform float g_Alpha;
uniform float g_Brightness;
uniform vec3 g_Color;

void main() {
	vec4 albedo = texSample2D(g_Texture0, v_TexCoord);
	albedo.rgb *= g_Color * g_Brightness;
	albedo.a *= g_Alpha;
	gl_FragColor = albedo;
}
