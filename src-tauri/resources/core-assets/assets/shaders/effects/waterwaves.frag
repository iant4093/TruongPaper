// TruongPaper clean-room implementation of the stock "waterwaves" effect interface.
#include "common.h"

varying vec2 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"mask","label":"ui_editor_properties_opacity_mask","mode":"opacitymask","default":"util/white","paintdefaultcolor":"0 0 0 1"}
uniform float g_Time;
uniform float g_Speed; // {"material":"speed","label":"ui_editor_properties_speed","default":5,"range":[0.01,50]}
uniform float g_Scale; // {"material":"scale","label":"ui_editor_properties_scale","default":200,"range":[0.01,1000]}
uniform float g_Strength; // {"material":"strength","label":"ui_editor_properties_strength","default":0.1,"range":[0.01,1]}
uniform float g_Perspective; // {"material":"perspective","label":"ui_editor_properties_perspective","default":0,"range":[0,0.2]}
uniform float g_Direction; // {"material":"direction","label":"ui_editor_properties_direction","default":0,"range":[0,6.28],"direction":true}

// Sine waves travelling along `direction`; pixels shift sideways to the wave front.
void main() {
	vec2 along = rotateVec2(vec2(0.0, 1.0), g_Direction);
	vec2 side = vec2(along.y, -along.x);
	float depth = abs(dot(v_TexCoord - 0.5, along));
	float phase = g_Time * g_Speed + dot(v_TexCoord, along) * (g_Scale + g_Perspective * depth);
	float amplitude = (g_Strength * g_Strength + g_Perspective * depth) * texSample2D(g_Texture1, v_TexCoord).r;
	gl_FragColor = texSample2D(g_Texture0, v_TexCoord + side * sin(phase) * amplitude);
}
