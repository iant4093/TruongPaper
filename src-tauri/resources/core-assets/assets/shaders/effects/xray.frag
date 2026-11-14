// TruongPaper clean-room implementation of the stock "xray" effect interface.
// Reveals texture 1 (blended with BLENDMODE) in a sprite-shaped spot that follows the cursor,
// scaled by `size`, strengthened by `multiply` and limited by the optional opacity mask (slot 3).

// [COMBO] {"material":"ui_editor_properties_blend_mode","combo":"BLENDMODE","type":"imageblending","default":0}

#include "common_blending.h"

varying vec4 v_TexCoord;
varying vec4 v_Pointer;

uniform float g_Multiply; // {"material":"multiply","label":"ui_editor_properties_multiply","default":1,"range":[0.0, 10.0]}

#if OPACITYMASK == 1
varying vec2 v_TexCoordOpacity;
#endif

uniform sampler2D g_Texture0; // {"hidden":true}
uniform sampler2D g_Texture1; // {"label":"ui_editor_properties_blend_texture","default":"util/white"}
uniform sampler2D g_Texture2; // {"label":"ui_editor_properties_sprite","default":"particle/halo_6"}
uniform sampler2D g_Texture3; // {"label":"ui_editor_properties_opacity_mask","mode":"opacitymask","combo":"OPACITYMASK","paintdefaultcolor":"0 0 0 1"}

void main() {
	vec4 base = texSample2D(g_Texture0, v_TexCoord.xy);
	vec4 layer = texSample2D(g_Texture1, v_TexCoord.zw);
	float amount = layer.a * g_Multiply;
#if OPACITYMASK == 1
	amount *= texSample2D(g_Texture3, v_TexCoordOpacity).r;
#endif

	// Sprite centred on the pointer; its width is `size` of the layer width, kept square.
	vec2 local = (v_TexCoord.xy - v_Pointer.xy) * v_Pointer.z * vec2(1.0, v_Pointer.w);
	vec2 spriteUV = local + 0.5;
	vec4 sprite = texSample2D(g_Texture2, clamp(spriteUV, 0.0, 1.0));
	// Halo sprites are R8/RG88 (r, a); outside the sprite rect nothing is revealed.
	float inside = step(0.0, spriteUV.x) * step(spriteUV.x, 1.0) * step(0.0, spriteUV.y) * step(spriteUV.y, 1.0);
	amount *= sprite.r * sprite.a * inside;

	base.rgb = ApplyBlending(BLENDMODE, base.rgb, layer.rgb, amount);
	gl_FragColor = base;
}
