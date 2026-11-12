// TruongPaper clean-room implementation of the stock "godrays" effect interface.
uniform mat4 g_ModelViewProjectionMatrix;
uniform float g_Time;
uniform float g_NoiseSpeed; // {"material":"noisespeed","label":"ui_editor_properties_noise_speed","default":0.15,"range":[0.01, 1]}
uniform float g_NoiseScale; // {"material":"noisescale","label":"ui_editor_properties_noise_scale","default":3,"range":[0.01, 10]}

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

varying vec4 v_TexCoord;
varying vec4 v_NoiseTexCoord;

void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord = a_TexCoord.xyxy;
	// Two noise layers drifting in different directions so the gaps between rays move.
	float drift = g_Time * g_NoiseSpeed;
	v_NoiseTexCoord.xy = a_TexCoord * g_NoiseScale + vec2(drift, drift * 0.37);
	v_NoiseTexCoord.zw = a_TexCoord * (g_NoiseScale * 0.71) + vec2(-drift * 0.53, drift * 0.81);
}
