// TruongPaper clean-room implementation of the stock "vhs" effect interface.
#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform float g_Time;
uniform vec4 g_Texture0Resolution;
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.5,"range":[0, 1]}
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":1,"range":[0, 5]}

// Tape look: line jitter, a rolling tracking band, colour bleed, scanlines and noise.
void main() {
	float t = g_Time * g_Speed;
	float lines = max(g_Texture0Resolution.w, 1.0) * 0.5;
	float row = floor(v_TexCoord.y * lines);
	float jitter = (tpHash12(vec2(row, floor(t * 30.0))) - 0.5) * 0.004;
	float band = smoothstep(0.08, 0.0, abs(fract(v_TexCoord.y - t * 0.1) - 0.5));
	vec2 uv = v_TexCoord + vec2((jitter + band * 0.01) * g_Strength, 0.0);
	float bleed = 0.003 * g_Strength;
	vec4 color = texSample2D(g_Texture0, uv);
	color.r = texSample2D(g_Texture0, uv + vec2(bleed, 0.0)).r;
	color.b = texSample2D(g_Texture0, uv - vec2(bleed, 0.0)).b;
	float scan = 0.85 + 0.15 * sin(v_TexCoord.y * lines * M_PI_2);
	float grain = tpHash12(v_TexCoord * 911.0 + t) - 0.5;
	color.rgb = mix(color.rgb, color.rgb * scan + grain * 0.15, g_Strength);
	gl_FragColor = color;
}
