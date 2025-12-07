#pragma once

#include <vulkan/vk_layer.h>
#include <vulkan/vulkan.h>

#ifndef VK_LAYER_EXPORT
#if defined(__GNUC__) && __GNUC__ >= 4
#define VK_LAYER_EXPORT __attribute__((visibility("default")))
#elif defined(_WIN32)
#define VK_LAYER_EXPORT __declspec(dllexport)
#else
#define VK_LAYER_EXPORT
#endif
#endif

extern "C" const char *g_sessionPath;

extern "C" VK_LAYER_EXPORT PFN_vkVoidFunction VKAPI_CALL wsi_layer_vkGetDeviceProcAddr(VkDevice device, const char *funcName);
extern "C" VK_LAYER_EXPORT VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL wsi_layer_vkGetInstanceProcAddr(VkInstance instance, const char *funcName);
