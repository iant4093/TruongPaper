// TruongPaper clean-room implementation of the stock "perspective" effect interface.
#include "common_perspective.h"

uniform vec2 g_Point0; // {"material":"point0","label":"p0","default":"0 0"}
uniform vec2 g_Point1; // {"material":"point1","label":"p1","default":"1 0"}
uniform vec2 g_Point2; // {"material":"point2","label":"p2","default":"1 1"}
uniform vec2 g_Point3; // {"material":"point3","label":"p3","default":"0 1"}

attribute vec3 a_Position;
attribute vec2 a_TexCoord;

varying vec2 v_TexCoord;

// Moves the quad's corners onto the four points with a projective mapping. Writing the
// homogeneous w lets the rasteriser interpolate the texture coordinate perspective-correctly.
void main() {
	mat3 xform = squareToQuad(g_Point0, g_Point1, g_Point2, g_Point3);
	vec3 p = xform * vec3(a_TexCoord, 1.0);
	vec2 corner = p.xy / p.z;
	gl_Position = vec4(vec2(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0) * p.z, 0.0, p.z);
	v_TexCoord = a_TexCoord;
}
