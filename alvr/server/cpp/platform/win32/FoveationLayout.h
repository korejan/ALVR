#pragma once

#include <cstdint>

// Foveated layout of one axis in whole encoded pixels: a 1:1 fovea between two edge bands.
// Each block of 2 * edgeRatio source pixels in a band becomes 2 encoded pixels.
struct FoveationAxisLayout {
	float eyeSize;        // unfoveated eye size, pixels
	float edgeRatio;
	double loBand;        // encoded pixels before the fovea
	double hiBand;        // encoded pixels after the fovea
	double encodedSize;   // encoded content size, pixels
	uint32_t alignedSize; // encoded eye size, rounded up to a multiple of 32
};

FoveationAxisLayout MakeFoveationAxisLayout(float eyeSize, float centerSize, float centerShift, float edgeRatio);

struct FoveationAxisParams {
	float eyeSizeRatio; // encoded content size / encoded eye size
	float edgeRatio;
	float loBound;      // fovea bounds in eye UV
	float hiBound;
	float encLoBound;   // fovea bounds in encoded content UV
	float encHiBound;
	float centerScale;  // eye UV per encoded content UV inside the fovea
};

inline FoveationAxisParams MakeFoveationAxisParams(const FoveationAxisLayout &l) {
	return {
		static_cast<float>(l.encodedSize / l.alignedSize),
		l.edgeRatio,
		static_cast<float>(l.loBand * l.edgeRatio / l.eyeSize),
		static_cast<float>(1. - l.hiBand * l.edgeRatio / l.eyeSize),
		static_cast<float>(l.loBand / l.encodedSize),
		static_cast<float>(1. - l.hiBand / l.encodedSize),
		static_cast<float>(l.encodedSize / l.eyeSize),
	};
}
