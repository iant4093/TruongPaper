// TruongPaper clean-room implementation of the stock "filmgrain" effect interface.
uniform mat4 g_ModelViewProjectionMatrix;
uniform vec4 g_Texture0Resolution;
uniform float g_Time;
uniform float g_NoiseScale; // {"material":"scale","label":"ui_editor_properties_scale","default":10,"range":[0.0, 20.0]}

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

varying vec2 v_TexCoord;
varying vec4 v_NoiseCoord;

void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord = a_TexCoord;
	float aspect = g_Texture0Resolution.z / max(g_Texture0Resolution.w, 1.0);
	// Jump to a new random offset 24 times a second, like grain on film frames.
	float frame = floor(g_Time * 24.0);
	vec2 jitter = fract(vec2(frame * 0.6180339, frame * 0.7548776));
	vec2 grainUv = a_TexCoord * vec2(aspect, 1.0) * g_NoiseScale;
	v_NoiseCoord.xy = grainUv + jitter;
	v_NoiseCoord.zw = grainUv * 0.57 - jitter.yx;
}
