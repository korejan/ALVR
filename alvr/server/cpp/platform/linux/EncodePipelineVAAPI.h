#pragma once

#include "EncodePipeline.h"

extern "C" struct AVBufferRef;
extern "C" struct AVCodecContext;
extern "C" struct AVFilterContext;
extern "C" struct AVFilterGraph;
extern "C" struct AVFrame;

namespace alvr
{

#define PRESET_MODE_SPEED   (0)
#define PRESET_MODE_BALANCE (1)
#define PRESET_MODE_QUALITY (2)

class EncodePipelineVAAPI: public EncodePipeline
{
public:
  ~EncodePipelineVAAPI();
  EncodePipelineVAAPI(std::vector<VkFrame> &input_frames, VkFrameCtx& vk_frame_ctx);

  void PushFrame(uint32_t frame_index, uint64_t targetTimestampNs, bool idr) override;

private:
  AVBufferRef *hw_ctx = nullptr;
  std::vector<AVFrame *> mapped_frames;
  AVFilterGraph *filter_graph = nullptr;
  AVFilterContext *filter_in = nullptr;
  AVFilterContext *filter_out = nullptr;

   union vlVaQualityBits {
      unsigned int quality;
      struct {
         unsigned int valid_setting: 1;
         unsigned int preset_mode: 2;
         unsigned int pre_encode_mode: 1;
         unsigned int vbaq_mode: 1;
         unsigned int reservered: 27;
      };
   };

};
}
