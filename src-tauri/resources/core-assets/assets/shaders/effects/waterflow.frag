// TruongPaper clean-room implementation of the stock "waterflow" effect interface.
varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"ui_editor_properties_flow_map","mode":"flowmask","default":"util/noflow"}
uniform sampler2D g_Texture2; // {"material":"ui_editor_properties_flow_phase","default":"effects/waterflowphase"}
uniform float g_Time;
uniform float g_FlowSpeed; // {"material":"ui_editor_properties_speed","default":1,"range":[0.01, 1]}
uniform float g_FlowAmp; // {"material":"ui_editor_properties_strength","default":1,"range":[0.01, 1]}
uniform float g_FlowPhaseScale; // {"material":"ui_editor_properties_phase_scale","default":1,"range":[0.01, 10]}

// Classic two-phase flow mapping: two copies of the image slide along the painted flow vectors,
// each restarting while the other is fully visible, so the motion loops without a visible jump.
void main() {
	vec2 flow = (texSample2D(g_Texture1, v_TexCoord).rg - 0.5) * 2.0;
	float offset = texSample2D(g_Texture2, v_TexCoord * g_FlowPhaseScale).r;
	float t = g_Time * g_FlowSpeed + offset;
	float phaseA = fract(t);
	float phaseB = fract(t + 0.5);
	float weightA = 1.0 - abs(phaseA * 2.0 - 1.0);
	vec2 reach = flow * (g_FlowAmp * 0.1);
	vec4 a = texSample2D(g_Texture0, v_TexCoord - reach * phaseA);
	vec4 b = texSample2D(g_Texture0, v_TexCoord - reach * phaseB);
	gl_FragColor = mix(b, a, weightA);
}
