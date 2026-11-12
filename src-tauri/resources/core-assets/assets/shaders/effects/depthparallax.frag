// TruongPaper clean-room implementation of the stock "depthparallax" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"depth","label":"ui_editor_properties_depth_map","default":"util/black"}
uniform vec2 g_PointerPosition;
uniform vec2 g_Scale; // {"material":"scale","label":"ui_editor_properties_scale","default":"0.02 0.02","range":[0, 0.2]}
uniform float g_Center; // {"material":"center","label":"ui_editor_properties_center","default":0.5,"range":[0, 1]}

// Near pixels (bright in the depth map) shift with the cursor more than far ones.
void main() {
	float depth = texSample2D(g_Texture1, v_TexCoord).r - g_Center;
	vec2 look = (g_PointerPosition - 0.5) * 2.0;
	gl_FragColor = texSample2D(g_Texture0, v_TexCoord - look * g_Scale * depth);
}
