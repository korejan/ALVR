#pragma once

#include <vulkan/vk_layer.h>

// VK_LAYER_EXPORT has been removed since 08/03/2003:
//    ref: https://github.com/KhronosGroup/Vulkan-Headers/commit/e8b8e06d092ab406b097907ecaae1a8aae9c7d53
#if !defined(VK_LAYER_EXPORT)
#if defined(__GNUC__) && __GNUC__ >= 4
#define VK_LAYER_EXPORT __attribute__((visibility("default")))
#elif defined(__SUNPRO_C) && (__SUNPRO_C >= 0x590)
#define VK_LAYER_EXPORT __attribute__((visibility("default")))
#else
#define VK_LAYER_EXPORT
#endif
#endif

extern "C" const char *g_sessionPath;

extern "C" VK_LAYER_EXPORT PFN_vkVoidFunction VKAPI_CALL wsi_layer_vkGetDeviceProcAddr(VkDevice device, const char *funcName);
extern "C" VK_LAYER_EXPORT VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL wsi_layer_vkGetInstanceProcAddr(VkInstance instance, const char *funcName);
