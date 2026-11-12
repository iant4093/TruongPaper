// TruongPaper clean-room implementation of the stock "cloudmotion" effect interface.
uniform mat4 g_ModelViewProjectionMatrix;
uniform float g_Time;
uniform vec4 g_Texture0Resolution;
uniform float u_speed; // {"material":"ui_editor_properties_speed","default":0.02,"range":[0.01,0.1]}
uniform float u_scale; // {"material":"ui_editor_properties_granularity","default":2,"range":[0.5,4]}
uniform float u_scaleX; // {"material":"ui_editor_properties_granularity_horizontal","default":0.5,"range":[0.1,1]}

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

varying vec2 v_TexCoord;
varying vec2 v_NoiseCoord;

void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord = a_TexCoord;
	float aspect = g_Texture0Resolution.x / max(g_Texture0Resolution.y, 1.0);
	v_NoiseCoord = a_TexCoord * vec2(aspect * u_scaleX, 1.0) * u_scale + vec2(g_Time * u_speed, 0.0);
}
