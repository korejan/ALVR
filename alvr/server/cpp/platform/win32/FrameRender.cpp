#include "FrameRender.h"
#include "alvr_server/Utils.h"
#include "alvr_server/Logger.h"
#include "alvr_server/Settings.h"
#include "alvr_server/bindings.h"

extern uint64_t g_DriverTestMode;

using namespace d3d_render_utils;


FrameRender::FrameRender(std::shared_ptr<CD3DRender> pD3DRender)
	: m_pD3DRender(pD3DRender)
{
		FrameRender::SetGpuPriority(m_pD3DRender->GetDevice());
}


FrameRender::~FrameRender()
{
}

bool FrameRender::Startup()
{
	if (m_pStagingTexture) {
		return true;
	}

	//
	// Create staging texture
	// This is input texture of Video Encoder and is render target of both eyes.
	//

	D3D11_TEXTURE2D_DESC compositionTextureDesc;
	ZeroMemory(&compositionTextureDesc, sizeof(compositionTextureDesc));
	compositionTextureDesc.Width = Settings::Instance().m_renderWidth;
	compositionTextureDesc.Height = Settings::Instance().m_renderHeight;
	compositionTextureDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;
	compositionTextureDesc.MipLevels = 1;
	compositionTextureDesc.ArraySize = 1;
	compositionTextureDesc.SampleDesc.Count = 1;
	compositionTextureDesc.Usage = D3D11_USAGE_DEFAULT;
	compositionTextureDesc.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;

	ComPtr<ID3D11Texture2D> compositionTexture;

	if (FAILED(m_pD3DRender->GetDevice()->CreateTexture2D(&compositionTextureDesc, NULL, &compositionTexture)))
	{
		Error("Failed to create staging texture!\n");
		return false;
	}

	HRESULT hr = m_pD3DRender->GetDevice()->CreateRenderTargetView(compositionTexture.Get(), NULL, &m_pRenderTargetView);
	if (FAILED(hr)) {
		Error("CreateRenderTargetView %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	// Create depth stencil texture
	D3D11_TEXTURE2D_DESC descDepth;
	ZeroMemory(&descDepth, sizeof(descDepth));
	descDepth.Width = compositionTextureDesc.Width;
	descDepth.Height = compositionTextureDesc.Height;
	descDepth.MipLevels = 1;
	descDepth.ArraySize = 1;
	descDepth.Format = DXGI_FORMAT_D24_UNORM_S8_UINT;
	descDepth.SampleDesc.Count = 1;
	descDepth.SampleDesc.Quality = 0;
	descDepth.Usage = D3D11_USAGE_DEFAULT;
	descDepth.BindFlags = D3D11_BIND_DEPTH_STENCIL;
	descDepth.CPUAccessFlags = 0;
	descDepth.MiscFlags = 0;
	hr = m_pD3DRender->GetDevice()->CreateTexture2D(&descDepth, nullptr, &m_pDepthStencil);
	if (FAILED(hr)) {
		Error("CreateTexture2D %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}


	// Create the depth stencil view
	D3D11_DEPTH_STENCIL_VIEW_DESC descDSV;
	ZeroMemory(&descDSV, sizeof(descDSV));
	descDSV.Format = descDepth.Format;
	descDSV.ViewDimension = D3D11_DSV_DIMENSION_TEXTURE2D;
	descDSV.Texture2D.MipSlice = 0;
	hr = m_pD3DRender->GetDevice()->CreateDepthStencilView(m_pDepthStencil.Get(), &descDSV, &m_pDepthStencilView);
	if (FAILED(hr)) {
		Error("CreateDepthStencilView %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	m_pD3DRender->GetContext()->OMSetRenderTargets(1, m_pRenderTargetView.GetAddressOf(), m_pDepthStencilView.Get());

	D3D11_VIEWPORT viewport;
	viewport.Width = (float)Settings::Instance().m_renderWidth;
	viewport.Height = (float)Settings::Instance().m_renderHeight;
	viewport.MinDepth = 0.0f;
	viewport.MaxDepth = 1.0f;
	viewport.TopLeftX = 0;
	viewport.TopLeftY = 0;
	m_pD3DRender->GetContext()->RSSetViewports(1, &viewport);

	//
	// Compile shaders
	//

	std::vector<uint8_t> vshader(FRAME_RENDER_VS_CSO_PTR, FRAME_RENDER_VS_CSO_PTR + FRAME_RENDER_VS_CSO_LEN);
	hr = m_pD3DRender->GetDevice()->CreateVertexShader((const DWORD*)&vshader[0], vshader.size(), NULL, &m_pVertexShader);
	if (FAILED(hr)) {
		Error("CreateVertexShader %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	std::vector<uint8_t> pshader(FRAME_RENDER_PS_CSO_PTR, FRAME_RENDER_PS_CSO_PTR + FRAME_RENDER_PS_CSO_LEN);
	hr = m_pD3DRender->GetDevice()->CreatePixelShader((const DWORD*)&pshader[0], pshader.size(), NULL, &m_pPixelShader);
	if (FAILED(hr)) {
		Error("CreatePixelShader %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	//
	// Create input layout
	//

	// Define the input layout
	D3D11_INPUT_ELEMENT_DESC layout[] =
	{
		{ "POSITION", 0, DXGI_FORMAT_R32G32B32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0 },
	{ "TEXCOORD", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 12, D3D11_INPUT_PER_VERTEX_DATA, 0 },
	{ "VIEW", 0, DXGI_FORMAT_R32_UINT, 0, 20, D3D11_INPUT_PER_VERTEX_DATA, 0 },
	};
	UINT numElements = ARRAYSIZE(layout);


	// Create the input layout
	hr = m_pD3DRender->GetDevice()->CreateInputLayout(layout, numElements, &vshader[0],
		vshader.size(), &m_pVertexLayout);
	if (FAILED(hr)) {
		Error("CreateInputLayout %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	// Set the input layout
	m_pD3DRender->GetContext()->IASetInputLayout(m_pVertexLayout.Get());

	//
	// Create vertex buffer
	//

	// Src texture has various geometry and we should use the part of the textures.
	// That part are defined by uv-coordinates of "bounds" passed to IVRDriverDirectModeComponent::SubmitLayer.
	// So we should update uv-coordinates for every frames and layers.
	D3D11_BUFFER_DESC bd;
	ZeroMemory(&bd, sizeof(bd));
	bd.Usage = D3D11_USAGE_DYNAMIC;
	bd.ByteWidth = sizeof(SimpleVertex) * 8;
	bd.BindFlags = D3D11_BIND_VERTEX_BUFFER;
	bd.CPUAccessFlags = D3D11_CPU_ACCESS_WRITE;

	hr = m_pD3DRender->GetDevice()->CreateBuffer(&bd, NULL, &m_pVertexBuffer);
	if (FAILED(hr)) {
		Error("CreateBuffer 1 %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	// Set vertex buffer
	UINT stride = sizeof(SimpleVertex);
	UINT offset = 0;
	m_pD3DRender->GetContext()->IASetVertexBuffers(0, 1, m_pVertexBuffer.GetAddressOf(), &stride, &offset);
	
	//
	// Create index buffer
	//

	WORD indices[] =
	{
		0,1,2,
		0,3,1,

		4,5,6,
		4,7,5
	};

	bd.Usage = D3D11_USAGE_DEFAULT;
	bd.ByteWidth = sizeof(indices);
	bd.BindFlags = D3D11_BIND_INDEX_BUFFER;
	bd.CPUAccessFlags = 0;

	D3D11_SUBRESOURCE_DATA InitData;
	ZeroMemory(&InitData, sizeof(InitData));
	InitData.pSysMem = indices;

	hr = m_pD3DRender->GetDevice()->CreateBuffer(&bd, &InitData, &m_pIndexBuffer);
	if (FAILED(hr)) {
		Error("CreateBuffer 2 %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	// Set index buffer
	m_pD3DRender->GetContext()->IASetIndexBuffer(m_pIndexBuffer.Get(), DXGI_FORMAT_R16_UINT, 0);

	// Set primitive topology
	m_pD3DRender->GetContext()->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

	// Create the sample state
	D3D11_SAMPLER_DESC sampDesc;
	ZeroMemory(&sampDesc, sizeof(sampDesc));
	sampDesc.Filter = D3D11_FILTER_ANISOTROPIC;
	sampDesc.AddressU = D3D11_TEXTURE_ADDRESS_WRAP;
	sampDesc.AddressV = D3D11_TEXTURE_ADDRESS_WRAP;
	sampDesc.AddressW = D3D11_TEXTURE_ADDRESS_WRAP;
	sampDesc.MaxAnisotropy = D3D11_REQ_MAXANISOTROPY;
	sampDesc.ComparisonFunc = D3D11_COMPARISON_NEVER;
	sampDesc.MinLOD = 0;
	sampDesc.MaxLOD = D3D11_FLOAT32_MAX;
	hr = m_pD3DRender->GetDevice()->CreateSamplerState(&sampDesc, &m_pSamplerLinear);
	if (FAILED(hr)) {
		Error("CreateSamplerState %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	//
	// Create alpha blend state
	// We need alpha blending to support layer.
	//

	// BlendState for first layer.
	// Some VR apps (like StreamVR Home beta) submit the texture that alpha is zero on all pixels.
	// So we need to ignore alpha of first layer.
	D3D11_BLEND_DESC BlendDesc;
	ZeroMemory(&BlendDesc, sizeof(BlendDesc));
	BlendDesc.AlphaToCoverageEnable = FALSE;
	BlendDesc.IndependentBlendEnable = FALSE;
	for (int i = 0; i < 8; i++) {
		BlendDesc.RenderTarget[i].BlendEnable = TRUE;
		BlendDesc.RenderTarget[i].SrcBlend = D3D11_BLEND_ONE;
		BlendDesc.RenderTarget[i].DestBlend = D3D11_BLEND_ZERO;
		BlendDesc.RenderTarget[i].BlendOp = D3D11_BLEND_OP_ADD;
		BlendDesc.RenderTarget[i].SrcBlendAlpha = D3D11_BLEND_ONE;
		BlendDesc.RenderTarget[i].DestBlendAlpha = D3D11_BLEND_ZERO;
		BlendDesc.RenderTarget[i].BlendOpAlpha = D3D11_BLEND_OP_ADD;
		BlendDesc.RenderTarget[i].RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_RED | D3D11_COLOR_WRITE_ENABLE_GREEN | D3D11_COLOR_WRITE_ENABLE_BLUE;
	}

	hr = m_pD3DRender->GetDevice()->CreateBlendState(&BlendDesc, &m_pBlendStateFirst);
	if (FAILED(hr)) {
		Error("CreateBlendState %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	// BleandState for other layers than first.
	BlendDesc.AlphaToCoverageEnable = FALSE;
	BlendDesc.IndependentBlendEnable = FALSE;
	for (int i = 0; i < 8; i++) {
		BlendDesc.RenderTarget[i].BlendEnable = TRUE;
		BlendDesc.RenderTarget[i].SrcBlend = D3D11_BLEND_SRC_ALPHA;
		BlendDesc.RenderTarget[i].DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
		BlendDesc.RenderTarget[i].BlendOp = D3D11_BLEND_OP_ADD;
		BlendDesc.RenderTarget[i].SrcBlendAlpha = D3D11_BLEND_ONE;
		BlendDesc.RenderTarget[i].DestBlendAlpha = D3D11_BLEND_ZERO;
		BlendDesc.RenderTarget[i].BlendOpAlpha = D3D11_BLEND_OP_ADD;
		BlendDesc.RenderTarget[i].RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL;
	}

	hr = m_pD3DRender->GetDevice()->CreateBlendState(&BlendDesc, &m_pBlendState);
	if (FAILED(hr)) {
		Error("CreateBlendState %p %ls\n", hr, GetErrorStr(hr).c_str());
		return false;
	}

	m_pStagingTexture = compositionTexture;

	std::vector<uint8_t> quadShaderCSO(QUAD_SHADER_CSO_PTR, QUAD_SHADER_CSO_PTR + QUAD_SHADER_CSO_LEN);
	ComPtr<ID3D11VertexShader> quadVertexShader = CreateVertexShader(m_pD3DRender->GetDevice(), quadShaderCSO);

	enableColorCorrection = Settings::Instance().m_enableColorCorrection;
	if (enableColorCorrection) {
		std::vector<uint8_t> colorCorrectionShaderCSO(COLOR_CORRECTION_CSO_PTR, COLOR_CORRECTION_CSO_PTR + COLOR_CORRECTION_CSO_LEN);

		ComPtr<ID3D11Texture2D> colorCorrectedTexture = CreateTexture(m_pD3DRender->GetDevice(),
			Settings::Instance().m_renderWidth, Settings::Instance().m_renderHeight,
			DXGI_FORMAT_R8G8B8A8_UNORM_SRGB);

		struct ColorCorrection {
			float renderWidth;
			float renderHeight;
			float brightness;
			float contrast;
			float saturation;
			float gamma;
			float sharpening;
			float _align;
		};
		ColorCorrection colorCorrectionStruct = { (float)Settings::Instance().m_renderWidth, (float)Settings::Instance().m_renderHeight,
												  Settings::Instance().m_brightness, Settings::Instance().m_contrast + 1.f,
												  Settings::Instance().m_saturation + 1.f, Settings::Instance().m_gamma,
												  Settings::Instance().m_sharpening };
		ComPtr<ID3D11Buffer> colorCorrectionBuffer = CreateBuffer(m_pD3DRender->GetDevice(), colorCorrectionStruct);

		m_colorCorrectionPipeline = std::make_unique<RenderPipeline>(m_pD3DRender->GetDevice());
		m_colorCorrectionPipeline->Initialize({ m_pStagingTexture.Get() }, quadVertexShader.Get(), colorCorrectionShaderCSO,
											  colorCorrectedTexture.Get(), colorCorrectionBuffer.Get());

		m_pStagingTexture = colorCorrectedTexture;
	}

	enableFFR = Settings::Instance().m_enableFoveatedRendering;
	if (enableFFR) {
		m_ffr = std::make_unique<FFR>(m_pD3DRender->GetDevice());
		m_ffr->Initialize(m_pStagingTexture.Get());

		m_pStagingTexture = m_ffr->GetOutputTexture();
	}

	Debug("Staging Texture created\n");

	return true;
}


bool FrameRender::RenderFrame(ID3D11Texture2D *pTexture[][2], vr::VRTextureBounds_t bounds[][2], int layerCount, bool recentering, const std::string &message, const std::string& debugText)
{
	// Set render target
	m_pD3DRender->GetContext()->OMSetRenderTargets(1, m_pRenderTargetView.GetAddressOf(), m_pDepthStencilView.Get());

	// Set viewport
	D3D11_VIEWPORT viewport;
	viewport.Width = (float)Settings::Instance().m_renderWidth;
	viewport.Height = (float)Settings::Instance().m_renderHeight;
	viewport.MinDepth = 0.0f;
	viewport.MaxDepth = 1.0f;
	viewport.TopLeftX = 0;
	viewport.TopLeftY = 0;
	m_pD3DRender->GetContext()->RSSetViewports(1, &viewport);

	// Clear the back buffer
	m_pD3DRender->GetContext()->ClearRenderTargetView(m_pRenderTargetView.Get(), DirectX::Colors::Black);

	RenderVisibilityMaskIfDirty();

	// Overlay recentering texture on top of all layers.
	int recenterLayer = -1;
	if (recentering) {
		recenterLayer = layerCount;
		layerCount++;
	}

	for (int i = 0; i < layerCount; i++) {
		ID3D11Texture2D *textures[2];
		vr::VRTextureBounds_t bound[2];

		if (i == recenterLayer) {
			textures[0] = (ID3D11Texture2D *)m_recenterTexture.Get();
			textures[1] = (ID3D11Texture2D *)m_recenterTexture.Get();
			bound[0].uMin = bound[0].vMin = bound[1].uMin = bound[1].vMin = 0.0f;
			bound[0].uMax = bound[0].vMax = bound[1].uMax = bound[1].vMax = 1.0f;
		}
		else {
			textures[0] = pTexture[i][0];
			textures[1] = pTexture[i][1];
			bound[0] = bounds[i][0];
			bound[1] = bounds[i][1];
		}
		if (textures[0] == NULL || textures[1] == NULL) {
			Debug("Ignore NULL layer. layer=%d/%d%s%s\n", i, layerCount
				, recentering ? L" (recentering)" : L"", !message.empty() ? L" (message)" : L"");
			continue;
		}

		D3D11_TEXTURE2D_DESC srcDesc;
		textures[0]->GetDesc(&srcDesc);

		Debug("RenderFrame layer=%d/%d %dx%d %d%s%s\n", i, layerCount, srcDesc.Width, srcDesc.Height, srcDesc.Format
			, recentering ? L" (recentering)" : L"", !message.empty() ? L" (message)" : L"");

		D3D11_SHADER_RESOURCE_VIEW_DESC SRVDesc = {};
		SRVDesc.Format = srcDesc.Format;
		SRVDesc.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
		SRVDesc.Texture2D.MostDetailedMip = 0;
		SRVDesc.Texture2D.MipLevels = 1;

		ComPtr<ID3D11ShaderResourceView> pShaderResourceView[2];

		HRESULT hr = m_pD3DRender->GetDevice()->CreateShaderResourceView(textures[0], &SRVDesc, pShaderResourceView[0].ReleaseAndGetAddressOf());
		if (FAILED(hr)) {
			Error("CreateShaderResourceView %p %ls\n", hr, GetErrorStr(hr).c_str());
			return false;
		}
		hr = m_pD3DRender->GetDevice()->CreateShaderResourceView(textures[1], &SRVDesc, pShaderResourceView[1].ReleaseAndGetAddressOf());
		if (FAILED(hr)) {
			Error("CreateShaderResourceView %p %ls\n", hr, GetErrorStr(hr).c_str());
			return false;
		}

		if (i == 0) {
			m_pD3DRender->GetContext()->OMSetBlendState(m_pBlendStateFirst.Get(), NULL, 0xffffffff);
		}
		else {
			m_pD3DRender->GetContext()->OMSetBlendState(m_pBlendState.Get(), NULL, 0xffffffff);
		}
		
		// Clear the depth buffer to 1.0 (max depth)
		// We need clear depth buffer to correctly render layers.
		m_pD3DRender->GetContext()->ClearDepthStencilView(m_pDepthStencilView.Get(), D3D11_CLEAR_DEPTH, 1.0f, 0);

		//
		// Update uv-coordinates in vertex buffer according to bounds.
		//

		SimpleVertex vertices[] =
		{
			// Left View
			{ DirectX::XMFLOAT3(-1.0f, -1.0f, 0.5f), DirectX::XMFLOAT2(bound[0].uMin, bound[0].vMax), 0 },
		{ DirectX::XMFLOAT3(0.0f,  1.0f, 0.5f), DirectX::XMFLOAT2(bound[0].uMax, bound[0].vMin), 0 },
		{ DirectX::XMFLOAT3(0.0f, -1.0f, 0.5f), DirectX::XMFLOAT2(bound[0].uMax, bound[0].vMax), 0 },
		{ DirectX::XMFLOAT3(-1.0f,  1.0f, 0.5f), DirectX::XMFLOAT2(bound[0].uMin, bound[0].vMin), 0 },
		// Right View
		{ DirectX::XMFLOAT3(0.0f, -1.0f, 0.5f), DirectX::XMFLOAT2(bound[1].uMin, bound[1].vMax), 1 },
		{ DirectX::XMFLOAT3(1.0f,  1.0f, 0.5f), DirectX::XMFLOAT2(bound[1].uMax, bound[1].vMin), 1 },
		{ DirectX::XMFLOAT3(1.0f, -1.0f, 0.5f), DirectX::XMFLOAT2(bound[1].uMax, bound[1].vMax), 1 },
		{ DirectX::XMFLOAT3(0.0f,  1.0f, 0.5f), DirectX::XMFLOAT2(bound[1].uMin, bound[1].vMin), 1 },
		};

		// TODO: Which is better? UpdateSubresource or Map
		//m_pD3DRender->GetContext()->UpdateSubresource(m_pVertexBuffer.Get(), 0, nullptr, &vertices, 0, 0);

		D3D11_MAPPED_SUBRESOURCE mapped = { 0 };
		hr = m_pD3DRender->GetContext()->Map(m_pVertexBuffer.Get(), 0, D3D11_MAP_WRITE_DISCARD, 0, &mapped);
		if (FAILED(hr)) {
			Error("Map %p %ls\n", hr, GetErrorStr(hr).c_str());
			return false;
		}
		memcpy(mapped.pData, vertices, sizeof(vertices));

		m_pD3DRender->GetContext()->Unmap(m_pVertexBuffer.Get(), 0);

		// Set the input layout
		m_pD3DRender->GetContext()->IASetInputLayout(m_pVertexLayout.Get());

		//
		// Set buffers
		//

		UINT stride = sizeof(SimpleVertex);
		UINT offset = 0;
		m_pD3DRender->GetContext()->IASetVertexBuffers(0, 1, m_pVertexBuffer.GetAddressOf(), &stride, &offset);

		m_pD3DRender->GetContext()->IASetIndexBuffer(m_pIndexBuffer.Get(), DXGI_FORMAT_R16_UINT, 0);
		m_pD3DRender->GetContext()->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);

		//
		// Set shaders
		//

		m_pD3DRender->GetContext()->VSSetShader(m_pVertexShader.Get(), nullptr, 0);
		m_pD3DRender->GetContext()->PSSetShader(m_pPixelShader.Get(), nullptr, 0);

		ID3D11ShaderResourceView *shaderResourceView[2] = { pShaderResourceView[0].Get(), pShaderResourceView[1].Get() };
		m_pD3DRender->GetContext()->PSSetShaderResources(0, 2, shaderResourceView);

		m_pD3DRender->GetContext()->PSSetSamplers(0, 1, m_pSamplerLinear.GetAddressOf());
		
		//
		// Draw
		//

		m_pD3DRender->GetContext()->DrawIndexed(VERTEX_INDEX_COUNT, 0, 0);
	}

	if (enableColorCorrection) {
		m_colorCorrectionPipeline->Render();
	}

	if (enableFFR) {
		m_ffr->Render();
	}

	m_pD3DRender->GetContext()->Flush();

	return true;
}

ComPtr<ID3D11Texture2D> FrameRender::GetTexture()
{
	return m_pStagingTexture;
}

void FrameRender::GetEncodingResolution(uint32_t *width, uint32_t *height) {
	if (enableFFR) {
		m_ffr->GetOptimizedResolution(width, height);
	}
	else {
		*width = Settings::Instance().m_renderWidth;
		*height = Settings::Instance().m_renderHeight;
	}
	
}

void FrameRender::RenderVisibilityMaskIfDirty() {
	std::scoped_lock<std::mutex> lock(m_visibilityMaskMutex);
	RenderVisibilityMaskNoLock();
	if (m_visibilityMaskState.testStencilState != nullptr) {
		m_pD3DRender->GetContext()->OMSetDepthStencilState(m_visibilityMaskState.testStencilState.Get(), 1);
	}
}

void FrameRender::RenderVisibilityMaskNoLock() {
	if (!m_visibilityMaskState.isDirty)
		return;

	auto deviceCtx = m_pD3DRender->GetContext();
	if (deviceCtx == nullptr || m_pDepthStencilView == nullptr)
		return;
	if (m_visibilityMaskState.pixelShader == nullptr ||
		m_visibilityMaskState.vertexShader == nullptr)
		return;

    deviceCtx->OMSetDepthStencilState(m_visibilityMaskState.fillStencilState.Get(), 1);  // Use stencil ref = 1
	const float blendFactor[4] = {0, 0, 0, 0};
	constexpr const UINT sampleMask = 0xFFFFFFFF;
	deviceCtx->OMSetBlendState(m_visibilityMaskState.noBlendState.Get(), blendFactor, sampleMask);
	deviceCtx->RSSetState(m_visibilityMaskState.noCullState.Get());
	deviceCtx->ClearDepthStencilView(m_pDepthStencilView.Get(), D3D11_CLEAR_STENCIL, 1.0f, 0);

	deviceCtx->VSSetShader(m_visibilityMaskState.vertexShader.Get(), nullptr, 0);
	deviceCtx->PSSetShader(m_visibilityMaskState.pixelShader.Get(), nullptr, 0);
	deviceCtx->IASetInputLayout(m_visibilityMaskState.vertexLayout.Get());

	for (size_t viewIdx = 0; viewIdx < 2; ++viewIdx) {
		const auto& vbuff = m_visibilityMaskState.vertexBuffers[viewIdx];
		if (vbuff.vb == nullptr || vbuff.vertexCount == 0)
			continue;
		D3D11_VIEWPORT viewport = {};
		viewport.Width = (float)Settings::Instance().m_renderWidth * .5f;
		viewport.Height = (float)Settings::Instance().m_renderHeight;
		viewport.MinDepth = 0.0f;
		viewport.MaxDepth = 1.0f;
		viewport.TopLeftX = (float)viewIdx * viewport.Width;
		viewport.TopLeftY = 0;
		deviceCtx->RSSetViewports(1, &viewport);

		const constexpr UINT stride = sizeof(vr::HmdVector2_t);
		const constexpr UINT offset = 0;
		deviceCtx->IASetVertexBuffers(0, 1, vbuff.vb.GetAddressOf(), &stride, &offset);
		deviceCtx->IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
		deviceCtx->Draw(vbuff.vertexCount, 0);
	}

	// Reset state
	D3D11_VIEWPORT viewport = {};
	viewport.Width = (float)Settings::Instance().m_renderWidth;
	viewport.Height = (float)Settings::Instance().m_renderHeight;
	viewport.MinDepth = 0.0f;
	viewport.MaxDepth = 1.0f;
	viewport.TopLeftX = 0;
	viewport.TopLeftY = 0;
	m_pD3DRender->GetContext()->RSSetViewports(1, &viewport);

	deviceCtx->RSSetState(m_visibilityMaskState.cullState.Get());

	m_visibilityMaskState.isDirty = false;
}

// projected triangles in NDC space.
bool FrameRender::SetVisibilityMasks(const FrameRender::HiddenAreaMeshViews& hams) {
	if (hams[0].empty() || hams[1].empty())
		return false;
	std::scoped_lock<std::mutex> lock{m_visibilityMaskMutex};

	auto device = m_pD3DRender->GetDevice();
	auto deviceCtx = m_pD3DRender->GetContext();
	if (device == nullptr || deviceCtx == nullptr)
		return false;

	constexpr const DWORD CompileFlags = D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_WARNINGS_ARE_ERRORS
#if defined(NDEBUG)
		| D3DCOMPILE_OPTIMIZATION_LEVEL3;
#else
		| D3DCOMPILE_SKIP_OPTIMIZATION | D3DCOMPILE_DEBUG;
#endif

	if (m_visibilityMaskState.vertexShader == nullptr) {
		static constexpr const char* const VShaderSrc = R"(
			struct VS_INPUT {
				float2 pos : POSITION;
			};
			struct PS_INPUT {
				float4 pos : SV_POSITION;
			};
			PS_INPUT main(VS_INPUT input) {
				PS_INPUT output;
				output.pos = float4(input.pos, 0.0, 1.0);
				return output;
			}
		)";
		ComPtr<ID3DBlob> vshaderBlob{};
		if (FAILED(D3DCompile(VShaderSrc, strlen(VShaderSrc), nullptr, nullptr, nullptr, "main", "vs_5_0", CompileFlags, 0, vshaderBlob.ReleaseAndGetAddressOf(), nullptr)))
			return false;
    	device->CreateVertexShader(vshaderBlob->GetBufferPointer(), vshaderBlob->GetBufferSize(), nullptr, m_visibilityMaskState.vertexShader.ReleaseAndGetAddressOf());

		static constexpr const std::array<D3D11_INPUT_ELEMENT_DESC,1> Vertexlayout = {
			{ "POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 0, 0, D3D11_INPUT_PER_VERTEX_DATA, 0 },
		};		
		if (FAILED(device->CreateInputLayout(
			Vertexlayout.data(), (UINT)Vertexlayout.size(),
			vshaderBlob->GetBufferPointer(), vshaderBlob->GetBufferSize(),
			m_visibilityMaskState.vertexLayout.ReleaseAndGetAddressOf()))) {
			return false;
		}
	}
	assert(m_visibilityMaskState.vertexLayout != nullptr);

	if (m_visibilityMaskState.pixelShader == nullptr) {
		// output doesn't matter, only writing to the stencil buffer
		static constexpr const char* const PShaderSrc  = R"(
			float4 main() : SV_TARGET {
				return float4(0.0, 0.0, 0.0, 0.0);
			}
		)";
		ComPtr<ID3DBlob> shaderBlob{};
		if (FAILED(D3DCompile(PShaderSrc, strlen(PShaderSrc), nullptr, nullptr, nullptr, "main", "ps_5_0", CompileFlags, 0, shaderBlob.ReleaseAndGetAddressOf(), nullptr)))
			return false;
    	device->CreatePixelShader(shaderBlob->GetBufferPointer(), shaderBlob->GetBufferSize(), nullptr, m_visibilityMaskState.pixelShader.ReleaseAndGetAddressOf());
	}

	if (m_visibilityMaskState.cullState == nullptr) {
		const CD3D11_RASTERIZER_DESC rasterizerDesc{ D3D11_DEFAULT };
		assert(rasterizerDesc.CullMode == D3D11_CULL_BACK);
		device->CreateRasterizerState(&rasterizerDesc, m_visibilityMaskState.cullState.ReleaseAndGetAddressOf());
	}

	if (m_visibilityMaskState.noCullState == nullptr) {
		CD3D11_RASTERIZER_DESC rasterizerDesc{ D3D11_DEFAULT };
		rasterizerDesc.FillMode = D3D11_FILL_SOLID;
		rasterizerDesc.CullMode = D3D11_CULL_NONE;
		device->CreateRasterizerState(&rasterizerDesc, m_visibilityMaskState.noCullState.ReleaseAndGetAddressOf());
	}

	if (m_visibilityMaskState.noBlendState == nullptr) {
		CD3D11_BLEND_DESC blendDesc{ D3D11_DEFAULT };
		for (size_t idx = 0; idx < 8; ++idx) {
			blendDesc.RenderTarget[idx].BlendEnable = FALSE;
			blendDesc.RenderTarget[idx].RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL; // Allow writing all color channels
		}
		device->CreateBlendState(&blendDesc, m_visibilityMaskState.noBlendState.ReleaseAndGetAddressOf());
	}

	if (m_visibilityMaskState.fillStencilState == nullptr) {
		D3D11_DEPTH_STENCIL_DESC stencilDesc = {};
		stencilDesc.DepthEnable = FALSE;
		stencilDesc.DepthWriteMask = D3D11_DEPTH_WRITE_MASK_ZERO;
		stencilDesc.DepthFunc = D3D11_COMPARISON_ALWAYS;
		stencilDesc.StencilEnable = TRUE;
		stencilDesc.StencilReadMask = 0xFF;  // Allow reading from all stencil bits
		stencilDesc.StencilWriteMask = 0xFF; // Allow writing to all stencil bits
		stencilDesc.FrontFace.StencilFailOp = D3D11_STENCIL_OP_KEEP;   // Replace stencil value if stencil test fails
		stencilDesc.FrontFace.StencilDepthFailOp = D3D11_STENCIL_OP_KEEP; // Replace stencil value if depth test fails (depth test disabled)
		stencilDesc.FrontFace.StencilPassOp = D3D11_STENCIL_OP_REPLACE;   // Replace stencil value if stencil test passes
		stencilDesc.FrontFace.StencilFunc = D3D11_COMPARISON_ALWAYS;      // Always pass stencil test (fill buffer)
		stencilDesc.BackFace = stencilDesc.FrontFace;
		device->CreateDepthStencilState(&stencilDesc, m_visibilityMaskState.fillStencilState.ReleaseAndGetAddressOf());
	}

	if (m_visibilityMaskState.testStencilState == nullptr) {
		D3D11_DEPTH_STENCIL_DESC stencilDesc = {};
		stencilDesc.DepthEnable = TRUE;
		stencilDesc.DepthWriteMask = D3D11_DEPTH_WRITE_MASK_ALL;
		stencilDesc.DepthFunc = D3D11_COMPARISON_LESS;
		stencilDesc.StencilEnable = TRUE;
		stencilDesc.StencilReadMask = 0xFF;  // Allow reading from all stencil bits
		stencilDesc.StencilWriteMask = 0x00; // No writing to stencil buffer (read-only)
		stencilDesc.FrontFace.StencilFailOp = D3D11_STENCIL_OP_KEEP;   // Keep stencil value if stencil test fails
		stencilDesc.FrontFace.StencilDepthFailOp = D3D11_STENCIL_OP_KEEP; // Keep stencil value if depth test fails
		stencilDesc.FrontFace.StencilPassOp = D3D11_STENCIL_OP_KEEP;   // Keep stencil value if both tests pass
		stencilDesc.FrontFace.StencilFunc = D3D11_COMPARISON_NOT_EQUAL;    // Only pass if stencil value equals reference
		stencilDesc.BackFace = stencilDesc.FrontFace;
		device->CreateDepthStencilState(&stencilDesc, m_visibilityMaskState.testStencilState.ReleaseAndGetAddressOf());
	}

	for (size_t vbIdx = 0; vbIdx < hams.size(); ++vbIdx) {
		const auto& ham = hams[vbIdx];
		auto& buff = m_visibilityMaskState.vertexBuffers[vbIdx];
		D3D11_BUFFER_DESC bufferDesc = {};
		bufferDesc.Usage = D3D11_USAGE_DEFAULT;
		bufferDesc.ByteWidth = static_cast<UINT>(sizeof(vr::HmdVector2_t) * ham.size());
		bufferDesc.BindFlags = D3D11_BIND_VERTEX_BUFFER;
		D3D11_SUBRESOURCE_DATA initData = {};
		initData.pSysMem = ham.data();
		if (FAILED(device->CreateBuffer(&bufferDesc, &initData, buff.vb.ReleaseAndGetAddressOf()))) {
			return false;
		}
		buff.vertexCount = ham.size();
	}
	return m_visibilityMaskState.isDirty = true;
}
