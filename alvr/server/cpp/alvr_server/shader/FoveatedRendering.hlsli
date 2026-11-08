// Axis-aligned foveated encoding: x and y are foveated independently, each with the 1-D mapping below.
//
//   One source eye, in eye UV. loBound and hiBound split each axis into an edge band, the
//   fovea and a second edge band. The edge bands are squeezed along their axis, the corners
//   along both (sizes here are illustrative; centerShift moves the fovea):
//
//             x=0         loBound.x                          hiBound.x         x=1
//          y=0 +--------------+----------------------------------+--------------+
//              |    corner    |             top band             |    corner    |
//              | x+y squeezed |            y squeezed            | x+y squeezed |
//    loBound.y +--------------+----------------------------------+--------------+
//              |              |                                  |              |
//              |   lo band    |              FOVEA               |   hi band    |
//              |  x squeezed  |           copied 1:1,            |  x squeezed  |
//              |  (temporal)  |         pixel for pixel          |   (nasal)    |
//              |              |                                  |              |
//    hiBound.y +--------------+----------------------------------+--------------+
//              |    corner    |           bottom band            |    corner    |
//          y=1 +--------------+----------------------------------+--------------+
//
//   The encoded video frame packs both eyes side by side. Edge bands shrink to 1/r of their
//   size on average, the fovea keeps its size. The right eye is mirrored, so the nasal sides
//   (eye x = 1) meet in the middle. Each eye is padded to a multiple of 32 pixels (::):
//
//   texture x   0                                 0.5                                  1
//               +--+------------------------+--+::::::::+--+------------------------+--+
//               |  |        top band        |  |::::::::|  |        top band        |  |
//               +--+------------------------+--+::::::::+--+------------------------+--+
//               |lo|                        |hi|::::::::|hi|                        |lo|
//               |  |         FOVEA          |  |::::::::|  |         FOVEA          |  |
//               |  |  same size in pixels   |  |::::::::|  |  same size in pixels   |  |
//               +--+------------------------+--+::::::::+--+------------------------+--+
//               |  |      bottom band       |  |::::::::|  |      bottom band       |  |
//               +--+------------------------+--+::::::::+--+------------------------+--+
//               :::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::::: <- padding rows
//               |<------------ left eye ----------->|<----- right eye (mirrored) ----->|
//                  ^                        ^  ^
//                  encLoBound.x
//                                           encHiBound.x
//                                              eyeSizeRatio.x  (end of content)
//
//   Source pixels per encoded pixel, along the encoded axis (edgeRatio = r):
//
//        2r-1 |\                                                   /|
//             | \                                                 / |
//           r |  \ . . . . band average . . . . . . . . . . . . ./  |
//             |   \                                             /   |
//           1 |    \___________________________________________/    |
//             +-----+-------------------------------------------+---+--> encoded (u)
//             0  encLoBound             fovea              encHiBound  1
//
//   In an edge band, t runs from the band's outer border (0) to the fovea (1):
//     EdgeToSource(t) = t * ((2r - 1) - (r - 1) * t) / r     (fraction of the band's source width)
//   That is 2r - 1 source pixels per encoded pixel at the border and exactly 1 at the fovea, so
//   the bands join the fovea without a visible seam.
//
// See FoveationVars (FFR.cpp) and FoveationAxisLayout (FoveationLayout.h).
cbuffer FoveationVars {
	float2 eyeSizeRatio;     // encoded content size / encoded eye size (rounded up to a multiple of 32)
	float2 edgeRatio;        // average compression of the edge bands
	float2 loBound;          // fovea bounds in eye UV
	float2 hiBound;
	float2 encLoBound;       // fovea bounds in encoded content UV
	float2 encHiBound;
	float2 centerScale;      // eye UV per encoded content UV inside the fovea
	float2 targetEyeSize;    // unfoveated eye size, pixels
	float2 optimizedEyeSize; // encoded eye size, pixels
};

float2 TextureToEyeUV(float2 textureUV, bool isRightEye) {
	// flip distortion horizontally for right eye
	// left: x * 2; right: (1 - x) * 2
	return float2((textureUV.x + float(isRightEye) * (1. - 2. * textureUV.x)) * 2., textureUV.y);
}

float2 EyeToTextureUV(float2 eyeUV, bool isRightEye) {
	// left: x / 2; right 1 - (x / 2)
	return float2(eyeUV.x / 2. + float(isRightEye) * (1. - eyeUV.x), eyeUV.y);
}

// The edge band profile above.
float2 EdgeToSource(float2 t) {
	return t * ((2. * edgeRatio - 1.) - (edgeRatio - 1.) * t) / edgeRatio;
}

// Encoded eye UV -> source eye UV. The alignment padding past the encoded content repeats its edge.
float2 EncodedToEyeUV(float2 encodedEyeUV) {
	float2 u = saturate(encodedEyeUV / eyeSizeRatio);
	bool2 inLoBand = u < encLoBound;
	// At most one band applies per axis, so the profile is evaluated once, from that band's outer
	// border. Outside both bands this may divide by a zero-width band; the selects ignore it.
	float2 bandX = EdgeToSource((inLoBand ? u : 1. - u) / (inLoBand ? encLoBound : 1. - encHiBound));
	float2 center = loBound + (u - encLoBound) * centerScale;
	return inLoBand ? loBound * bandX : (u > encHiBound ? 1. - (1. - hiBound) * bandX : center);
}
