// TruongPaper clean-room implementation of the stock "godrays" effect interface.
// [COMBO] {"material":"ui_editor_properties_blend_mode","combo":"BLENDMODE","type":"imageblending","default":9}

#include "common_blending.h"

varying vec4 v_TexCoord;

uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}
uniform sampler2D g_Texture1; // {"material":"previous","label":"Prev","hidden":true}

// Blends the blurred rays (texture 0, half resolution) over the original image (texture 1).
void main() {
	vec4 rays = texSample2D(g_Texture0, v_TexCoord.zw);
	vec4 image = texSample2D(g_Texture1, v_TexCoord.xy);
	image.rgb = ApplyBlending(BLENDMODE, image.rgb, rays.rgb, rays.a);
	image.a = saturate(image.a + rays.a);
	gl_FragColor = image;
}
