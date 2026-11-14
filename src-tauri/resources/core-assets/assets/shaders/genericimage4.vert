// TruongPaper clean-room base shader for image layers ("genericimage4" interface).
uniform mat4 g_ModelViewProjectionMatrix;

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

varying vec2 v_TexCoord;

// Spritesheet frame mapping (identity unless the base texture carries a TEXS frame table).
uniform vec4 g_Texture0Rotation;
uniform vec2 g_Texture0Translation;

void main() {
	gl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);
	v_TexCoord = vec2(dot(a_TexCoord, g_Texture0Rotation.xy), dot(a_TexCoord, g_Texture0Rotation.zw)) + g_Texture0Translation;
}
