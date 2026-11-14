// TruongPaper clean-room implementation of the stock "xray" effect interface.
// Inputs: size (g_PointerScale), multiply, blend texture (1), reveal sprite (2), opacity mask (3).
uniform mat4 g_ModelViewProjectionMatrix;
uniform vec4 g_Texture0Resolution;
uniform vec4 g_Texture1Resolution;
uniform vec2 g_PointerPosition;

uniform float g_PointerScale; // {"material":"size","label":"ui_editor_properties_size","default":0.2,"range":[0.0, 1.0]}

#if OPACITYMASK == 1
uniform vec4 g_Texture3Resolution;
varying vec2 v_TexCoordOpacity;
#endif

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

varying vec4 v_TexCoord;
varying vec4 v_Pointer;

// Padded textures keep their picture in the top-left: .zw is the used/allocated size ratio.
vec2 padded(vec2 uv, vec4 res) {
	return uv * res.zw / res.xy;
}

void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord.xy = a_TexCoord;
	v_TexCoord.zw = padded(a_TexCoord, g_Texture1Resolution);
#if OPACITYMASK == 1
	v_TexCoordOpacity = padded(a_TexCoord, g_Texture3Resolution);
#endif
	// Pointer in layer texture space, and the sprite scale; size 0 pushes the sprite off screen.
	v_Pointer.xy = g_PointerPosition;
	v_Pointer.z = mix(999.0, 1.0 / max(g_PointerScale, 0.001), step(0.001, g_PointerScale));
	v_Pointer.w = g_Texture0Resolution.y / max(g_Texture0Resolution.x, 1.0);
}
