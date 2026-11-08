// Foveated encode: writes one pixel of the encoded frame (layout in FoveatedRendering.hlsli).
//
//   Each encoded pixel covers a rectangle of the source eye (its edges eyeUV +- halfPixel mapped
//   through EncodedToEyeUV) and is the box average of the texels under it, so detail is averaged
//   rather than skipped, which would alias. A bilinear tap between two texels can return any
//   weighted average of them, so each tap covers a pair of texels per axis with their exact weights.
//
//      encoded pixel             source footprint (a 5 x 1 example, in a horizontal edge band)
//      +-------+                 lo                      hi
//      |   *   |   -------->     +----+----+----+----+----+
//      +-------+                 |    *    |    *    |  * |  colour = sum(weight * tap) / sum(weight)
//                                +----+----+----+----+----+
//                                 pair 0    pair 1    pair 2 (its second texel has weight 0)
//
//   In the fovea the footprint is one source texel, copied. Weighted taps only read texels under the
//   footprint, which stays inside this eye, so the eyes never blend.

#include "FoveatedRendering.hlsli"

// Texel pairs per axis: a footprint covers at most edgeRatio pairs, and the layout clamps the ratio to 16.
#define MAX_PAIRS 16

Texture2D<float4> compositionTexture : register(t0);
// Linear, clamped; bound by FFR.cpp.
SamplerState bilinearSampler : register(s0);

float4 main(float2 uv : TEXCOORD0) : SV_Target{
	bool isRightEye = uv.x > 0.5;
	float2 eyeUV = TextureToEyeUV(uv, isRightEye);
	float2 halfPixel = 0.5 / optimizedEyeSize;

	// The fovea, most of the frame: copy the pixel's one source texel.
	float2 u = eyeUV / eyeSizeRatio;
	float2 halfU = halfPixel / eyeSizeRatio;
	[branch] if (all(u - halfU >= encLoBound) && all(u + halfU <= encHiBound)) {
		float2 texel = floor((loBound + (u - encLoBound) * centerScale) * targetEyeSize) + 0.5;
		return compositionTexture.SampleLevel(bilinearSampler, EyeToTextureUV(texel / targetEyeSize, isRightEye), 0);
	}

	// The footprint in source texels. The padding's zero-width footprint at the eye's edge becomes the
	// last half texel, replicating it, and rounding must not carry hiT past the edge into the other eye.
	float2 loT = min(EncodedToEyeUV(eyeUV - halfPixel) * targetEyeSize, targetEyeSize - 0.5);
	float2 hiT = min(EncodedToEyeUV(eyeUV + halfPixel) * targetEyeSize, targetEyeSize);
	
	// The texels it covers, in pairs. Slivers under 0.1% of a texel, mostly float error at a texel
	// edge, are skipped rather than cost a pair.
	float2 first = floor(loT + 0.001);
	float2 last = max(ceil(hiT - 0.001) - 1., first);
	float2 pairs = min(floor((last - first) * 0.5) + 1., MAX_PAIRS);

	// x fastest, four taps per iteration so they are fetched together; taps past the last pair get
	// weight 0 (measured no slower than a separate remainder loop).
	float count = pairs.x * pairs.y;
	float2 pair = 0;
	float4 sum = 0;
	float total = 0;
	[loop] for (float i = 0; i < count; i += 4.) {
		[unroll] for (uint k = 0; k < 4; ++k) {
			// The pair's two texels are weighted by their overlap with the footprint, per axis.
			float2 a = first + 2. * pair;
			float2 wa = saturate(min(hiT, a + 1.) - max(loT, a));
			float2 wb = saturate(min(hiT, a + 2.) - max(loT, a + 1.));
			float2 w = wa + wb;
			float2 p = (a + 0.5 + wb / max(w, 1e-6)) / targetEyeSize;
			float weight = i + k < count ? w.x * w.y : 0.;
			sum += weight * compositionTexture.SampleLevel(bilinearSampler, EyeToTextureUV(p, isRightEye), 0);
			total += weight;
			bool wrap = pair.x + 1. == pairs.x;
			pair = wrap ? float2(0., pair.y + 1.) : float2(pair.x + 1., pair.y);
		}
	}
	return sum / total;
}
