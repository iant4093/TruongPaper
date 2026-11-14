// TruongPaper clean-room implementation of the stock "fisheye" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform vec4 g_Texture0Resolution;
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.5,"range":[-1, 1]}
uniform float g_Size; // {"material":"size","label":"ui_editor_properties_size","default":0.5,"range":[0.01, 1]}
uniform vec2 g_Center; // {"material":"center","label":"ui_editor_properties_center","default":"0.5 0.5","position":true}

// Lens bulge: inside `size` the image is magnified towards the centre (negative strength pinches).
void main() {
	float aspect = g_Texture0Resolution.z / max(g_Texture0Resolution.w, 1.0);
	vec2 d = (v_TexCoord - g_Center) * vec2(aspect, 1.0);
	float k = saturate(length(d) / g_Size);
	float scale = mix(1.0, k, g_Strength * (1.0 - k * k) * texSample2D(g_Texture1, v_TexCoord).r);
	vec2 uv = g_Center + d * scale / vec2(aspect, 1.0);
	gl_FragColor = texSample2D(g_Texture0, uv);
}
