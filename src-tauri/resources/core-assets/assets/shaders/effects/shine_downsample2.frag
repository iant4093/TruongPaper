// TruongPaper clean-room implementation of the stock "shine" effect interface.
// [COMBO] {"material":"ui_editor_properties_noise","combo":"NOISE","type":"options","default":1}

varying vec4 v_TexCoord;
varying vec4 v_NoiseTexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform sampler2D g_Texture2; // {"material":"albedo","label":"ui_editor_properties_albedo","default":"util/clouds_256"}
uniform float g_Threshold; // {"material":"raythreshold","label":"ui_editor_properties_ray_threshold","default":0.5,"range":[0, 1]}
uniform float g_NoiseAmount; // {"material":"noiseamount","label":"ui_editor_properties_noise_amount","default":0.4,"range":[0.01, 1]}
uniform float g_NoiseSmoothness; // {"material":"noisesmoothness","label":"ui_editor_properties_noise_smoothness","default":0.2,"range":[0.01, 0.5]}

// Keeps only the bright parts of the image: they are the light sources the rays are cast from.
void main() {
	vec4 color = texSample2D(g_Texture0, v_TexCoord.xy);
	float mask = texSample2D(g_Texture1, v_TexCoord.zw).r;
	float luma = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
	float weight = color.a * mask * smoothstep(g_Threshold, g_Threshold + 0.04, luma);
#if NOISE == 1
	float n = sqrt(texSample2D(g_Texture2, v_NoiseTexCoord.xy).r * texSample2D(g_Texture2, v_NoiseTexCoord.zw).r);
	float gate = smoothstep(0.5 - g_NoiseSmoothness, 0.5 + g_NoiseSmoothness, n);
	weight *= mix(1.0, gate, g_NoiseAmount);
#endif
	gl_FragColor = vec4(color.rgb * weight, weight);
}
