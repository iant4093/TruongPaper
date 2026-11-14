// TruongPaper clean-room implementation of the stock "waterripple" effect interface.
#include "common.h"

uniform mat4 g_ModelViewProjectionMatrix;
uniform float g_Time;
uniform vec4 g_Texture0Resolution;
uniform float g_AnimationSpeed; // {"material":"ui_editor_properties_animation_speed","default":0.15,"range":[0,0.5]}
uniform float g_Scale; // {"material":"ui_editor_properties_ripple_scale","default":1,"range":[0,10]}
uniform float g_ScrollSpeed; // {"material":"ui_editor_properties_scroll_speed","default":0,"range":[0,0.5]}
uniform float g_Direction; // {"material":"ui_editor_properties_scroll_direction","default":0,"range":[0,6.28]}
uniform float g_Ratio; // {"material":"ui_editor_properties_ratio","default":1,"range":[0,10]}

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

varying vec2 v_TexCoord;
varying vec4 v_RippleCoord;

void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord = a_TexCoord;
	float aspect = g_Texture0Resolution.x / max(g_Texture0Resolution.y, 1.0);
	float anim = g_Time * g_AnimationSpeed * g_AnimationSpeed;
	vec2 scroll = rotateVec2(vec2(0.0, -1.0), g_Direction) * (g_ScrollSpeed * g_ScrollSpeed * g_Time);
	vec2 stretch = vec2(aspect, g_Ratio) * g_Scale;
	// Two copies of the normal map at different sizes moving against each other.
	v_RippleCoord.xy = (a_TexCoord + vec2(anim, anim * 0.5) + scroll) * stretch;
	v_RippleCoord.zw = (a_TexCoord * 1.37 - vec2(anim * 0.6, anim) + scroll) * stretch;
}
