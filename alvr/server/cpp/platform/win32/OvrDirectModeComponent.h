#pragma once
#include <cstdint>
#include <atomic>

#include "openvr_driver.h"
#include "alvr_server/ClientConnection.h"
#include "alvr_server/Utils.h"
#include "CEncoder.h"
#include "alvr_server/PoseHistory.h"

#include "alvr_server/Settings.h"


class OvrDirectModeComponent final : public vr::IVRDriverDirectModeComponent
{
public:
	OvrDirectModeComponent(std::shared_ptr<CD3DRender> pD3DRender, std::shared_ptr<PoseHistory> poseHistory);

	void SetEncoder(std::shared_ptr<CEncoder> pEncoder);

	/** Specific to Oculus compositor support, textures supplied must be created using this method. */
	virtual void CreateSwapTextureSet( uint32_t unPid, const SwapTextureSetDesc_t *pSwapTextureSetDesc, SwapTextureSet_t *pOutSwapTextureSet ) override;

	/** Used to textures created using CreateSwapTextureSet.  Only one of the set's handles needs to be used to destroy the entire set. */
	virtual void DestroySwapTextureSet(vr::SharedTextureHandle_t sharedTextureHandle) override;

	/** Used to purge all texture sets for a given process. */
	virtual void DestroyAllSwapTextureSets(uint32_t unPid) override;

	/** After Present returns, calls this to get the next index to use for rendering. */
	virtual void GetNextSwapTextureSetIndex(vr::SharedTextureHandle_t sharedTextureHandles[2], uint32_t(*pIndices)[2]) override;

	/** Call once per layer to draw for this frame.  One shared texture handle per eye.  Textures must be created
	* using CreateSwapTextureSet and should be alternated per frame.  Call Present once all layers have been submitted. */
	virtual void SubmitLayer(const SubmitLayerPerEye_t(&perEye)[2]) override;

	/** Submits queued layers for display. */
	virtual void Present(vr::SharedTextureHandle_t syncTexture) override;

	void CopyTexture(uint32_t layerCount);

	void Pause() { m_paused = true; }
	void Resume() {
		if (const auto encoder = m_pEncoder)
			encoder->InsertIDR();
		m_paused = false;
	}

private:
	std::shared_ptr<CD3DRender> m_pD3DRender;
	std::shared_ptr<CEncoder> m_pEncoder;
	std::shared_ptr<ClientConnection> m_Listener;
	std::shared_ptr<PoseHistory> m_poseHistory;

	// Resource for each process
	struct ProcessResource {
		ComPtr<ID3D11Texture2D> textures[3];
		HANDLE sharedHandles[3];
		uint32_t pid;
	};
	std::map<HANDLE, std::pair<ProcessResource *, int> > m_handleMap;

	static constexpr const uint32_t MAX_LAYERS = 10;
	uint32_t m_submitLayer;
	SubmitLayerPerEye_t m_submitLayers[MAX_LAYERS][2];
	uint64_t m_targetTimestampNs;
	uint64_t m_prevTargetTimestampNs;
	std::atomic_bool m_paused{false};
};
