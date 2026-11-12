// TruongPaper clean-room implementation of the stock "colorkey" effect interface.
// [COMBO] {"material":"ui_editor_properties_invert","combo":"INVERT","type":"options","default":0}
// [COMBO] {"material":"ui_editor_properties_flatten","combo":"FLATTEN","type":"options","default":0}

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform float g_KeyAlpha; // {"material":"alpha","label":"ui_editor_properties_write_alpha","default":0,"range":[0,1]}
uniform float g_KeyFuzz; // {"material":"fuzziness","label":"ui_editor_properties_fuzziness","default":0,"range":[0,3]}
uniform float g_KeyTolerance; // {"material":"tolerance","label":"ui_editor_properties_tolerance","default":0.1,"range":[0,3]}
uniform vec3 g_KeyColor; // {"material":"color","label":"ui_editor_properties_color","type":"color","default":"1 1 1"}

// Pixels within `tolerance` (summed per-channel distance) of the key colour get the key alpha;
// `fuzziness` widens the soft edge.
void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord);
	vec3 d = abs(color.rgb - g_KeyColor);
	float distance = d.r + d.g + d.b;
	float keep = smoothstep(g_KeyTolerance, g_KeyTolerance + g_KeyFuzz + 0.001, distance);
#if INVERT == 1
	keep = 1.0 - keep;
#endif
	color.a *= mix(g_KeyAlpha, 1.0, keep);
#if FLATTEN == 1
	color.rgb *= color.a;
#endif
	gl_FragColor = color;
}
