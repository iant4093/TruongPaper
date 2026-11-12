// TruongPaper clean-room implementation of the stock "scroll" effect interface.
// [COMBO] {"material":"ui_editor_properties_repeat","combo":"REPEAT","type":"options","default":1}

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform float g_Time;
uniform vec2 g_ScrollSpeed; // {"material":"speed","label":"ui_editor_properties_speed","default":"0.1 0","range":[-2, 2]}

void main() {
	vec2 uv = v_TexCoord - g_ScrollSpeed * g_Time;
#if REPEAT
	uv = fract(uv);
	gl_FragColor = texSample2D(g_Texture0, uv);
#else
	vec2 inside = step(vec2(0.0), uv) * step(uv, vec2(1.0));
	gl_FragColor = texSample2D(g_Texture0, uv) * (inside.x * inside.y);
#endif
}
