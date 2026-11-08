#include "FFR.h"

#include "FoveationLayout.h"
#include "alvr_server/Settings.h"
#include "alvr_server/Utils.h"
#include "alvr_server/bindings.h"

using Microsoft::WRL::ComPtr;
using namespace d3d_render_utils;

namespace {

	// Matches the FoveationVars cbuffer in FoveatedRendering.hlsli
	struct alignas(16) FoveationVars {
		float eyeSizeRatioX;
		float eyeSizeRatioY;
		float edgeRatioX;
		float edgeRatioY;
		float loBoundX;
		float loBoundY;
		float hiBoundX;
		float hiBoundY;
		float encLoBoundX;
		float encLoBoundY;
		float encHiBoundX;
		float encHiBoundY;
		float centerScaleX;
		float centerScaleY;
		float targetEyeWidth;
		float targetEyeHeight;
		float optimizedEyeWidth;
		float optimizedEyeHeight;
	};

	struct FoveationLayout {
		FoveationAxisLayout x;
		FoveationAxisLayout y;
	};

	FoveationLayout CalculateFoveationLayout() {
		const auto &settings = Settings::Instance();
		return {
			MakeFoveationAxisLayout((float)settings.m_renderWidth / 2, settings.m_foveationCenterSizeX,
				settings.m_foveationCenterShiftX, settings.m_foveationEdgeRatioX),
			MakeFoveationAxisLayout((float)settings.m_renderHeight, settings.m_foveationCenterSizeY,
				settings.m_foveationCenterShiftY, settings.m_foveationEdgeRatioY)
		};
	}

	FoveationVars CalculateFoveationVars(const FoveationLayout &layout) {
		const auto x = MakeFoveationAxisParams(layout.x);
		const auto y = MakeFoveationAxisParams(layout.y);
		return {
			x.eyeSizeRatio, y.eyeSizeRatio,
			x.edgeRatio, y.edgeRatio,
			x.loBound, y.loBound,
			x.hiBound, y.hiBound,
			x.encLoBound, y.encLoBound,
			x.encHiBound, y.encHiBound,
			x.centerScale, y.centerScale,
			layout.x.eyeSize, layout.y.eyeSize,
			(float)layout.x.alignedSize, (float)layout.y.alignedSize
		};
	}
}


void FFR::GetOptimizedResolution(uint32_t* width, uint32_t* height) {
	const auto layout = CalculateFoveationLayout();
	*width = layout.x.alignedSize * 2;
	*height = layout.y.alignedSize;
}

FFR::FFR(ID3D11Device* device) : mDevice(device) {}

void FFR::Initialize(ID3D11Texture2D* compositionTexture) {
	const auto layout = CalculateFoveationLayout();
	ComPtr<ID3D11Buffer> foveatedRenderingBuffer = CreateBuffer(mDevice.Get(), CalculateFoveationVars(layout));

	std::vector<uint8_t> quadShaderCSO(QUAD_SHADER_CSO_PTR, QUAD_SHADER_CSO_PTR + QUAD_SHADER_CSO_LEN);
	mQuadVertexShader = CreateVertexShader(mDevice.Get(), quadShaderCSO);

	mOptimizedTexture = CreateTexture(mDevice.Get(), layout.x.alignedSize * 2,
		layout.y.alignedSize, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);

	if (Settings::Instance().m_enableFoveatedRendering) {
		std::vector<uint8_t> compressAxisAlignedShaderCSO(COMPRESS_AXIS_ALIGNED_CSO_PTR, COMPRESS_AXIS_ALIGNED_CSO_PTR + COMPRESS_AXIS_ALIGNED_CSO_LEN);
		auto compressAxisAlignedPipeline = RenderPipeline(mDevice.Get());
		compressAxisAlignedPipeline.Initialize({ compositionTexture }, mQuadVertexShader.Get(),
			compressAxisAlignedShaderCSO, mOptimizedTexture.Get(), foveatedRenderingBuffer.Get());
		// The shader filters each footprint itself from bilinear taps; don't inherit the last pass's sampler.
		compressAxisAlignedPipeline.SetSamplerState(CreateLinearClampSampler(mDevice.Get()).Get());

		mPipelines.push_back(compressAxisAlignedPipeline);
	} else {
		mOptimizedTexture = compositionTexture;
	}
}

void FFR::Render() {
	for (auto &p : mPipelines) {
		p.Render();
	}
}

ID3D11Texture2D* FFR::GetOutputTexture() {
	return mOptimizedTexture.Get();
}
