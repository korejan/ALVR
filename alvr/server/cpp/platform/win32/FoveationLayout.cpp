// The layout must come out identical on the server and every client. Its products are exact in a double,
// so each step rounds at most once however it is compiled; the pragmas rule out fast-math rewriting it.
#include "FoveationLayout.h"

#include <algorithm>
#include <cmath>

#if defined(_MSC_VER) || defined(__clang__)
#pragma float_control(precise, on, push)
#elif defined(__GNUC__)
#pragma GCC push_options
#pragma GCC optimize("no-fast-math")
#endif

FoveationAxisLayout MakeFoveationAxisLayout(float eyeSize, float centerSize, float centerShift, float edgeRatio) {
	// Whole ratios keep the fovea texel-aligned; the encoder handles up to 16 (the dashboard offers 1 to 10).
	const float ratio = std::clamp(std::round(edgeRatio), 1.0f, 16.0f);
	const double blockSize = ratio * 2.; // source pixels per block of 2 encoded pixels

	// The settings are decimals that floats only approximate (0.4f * 55 == 22.0000003), so a result at
	// most Slack above a whole number counts as that number. The products are exact in a double.
	constexpr double Slack = 1e-4;
	const double edgeSize = eyeSize - double(centerSize) * eyeSize;
	// A center size of 0 would otherwise round to more edge blocks than fit in the eye.
	const double edgeBlocks = std::clamp(std::ceil(edgeSize / blockSize - Slack), 0., std::floor(eyeSize / blockSize));
	// A hand-edited shift beyond +-1 would leave a band of negative width.
	const double shiftBlocks = std::clamp(std::ceil(centerShift * edgeBlocks - Slack), -edgeBlocks, edgeBlocks);

	const double encodedSize = eyeSize - edgeBlocks * blockSize + edgeBlocks * 2.;
	return {
		eyeSize,
		ratio,
		edgeBlocks + shiftBlocks,
		edgeBlocks - shiftBlocks,
		encodedSize,
		static_cast<uint32_t>(std::ceil(encodedSize / 32.)) * 32
	};
}

#if defined(_MSC_VER) || defined(__clang__)
#pragma float_control(pop)
#elif defined(__GNUC__)
#pragma GCC pop_options
#endif
