// TruongPaper clean-room implementation of the stock "blur" effect interface.
// [COMBO] {"material":"ui_editor_properties_kernel_size","combo":"KERNEL","type":"options","default":0,"options":{"13x13":0,"7x7":1,"3x3":2}}

uniform mat4 g_ModelViewProjectionMatrix;
uniform vec4 g_Texture0Resolution;
uniform vec2 g_Scale; // {"material":"scale","label":"ui_editor_properties_scale","default":"2 2","linked":true,"range":[0.01, 2.0]}

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

// xy: texture coordinate, zw: one blur tap in UV units along the pass direction.
varying vec4 v_TexCoord;

void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord.xy = a_TexCoord;
#if VERTICAL
	v_TexCoord.zw = vec2(0.0, g_Scale.y / max(g_Texture0Resolution.w, 1.0));
#else
	v_TexCoord.zw = vec2(g_Scale.x / max(g_Texture0Resolution.z, 1.0), 0.0);
#endif
}
