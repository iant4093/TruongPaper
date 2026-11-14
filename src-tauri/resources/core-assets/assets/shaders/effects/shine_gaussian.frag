// TruongPaper clean-room implementation of the stock "shine" effect interface.
uniform sampler2D g_Texture0; // {"material":"framebuffer","label":"ui_editor_properties_framebuffer","hidden":true}

#include "common_blur.h"

varying vec4 v_TexCoord;

void main() {
#if KERNEL == 2
	gl_FragColor = blur3a(v_TexCoord.xy, v_TexCoord.zw);
#elif KERNEL == 1
	gl_FragColor = blur7a(v_TexCoord.xy, v_TexCoord.zw);
#else
	gl_FragColor = blur13a(v_TexCoord.xy, v_TexCoord.zw);
#endif
}
