cmake_minimum_required(VERSION 3.22...4.0)

# C must be enabled here: the fetched nanors target exports c_std_11 as a
# PUBLIC compile feature, which consumers can only resolve with a known C
# compiler.
project(alvr_server_cpp
    VERSION     18.26.0
    DESCRIPTION "C++ core of the ALVR SteamVR (OpenVR) driver"
    LANGUAGES   C CXX
)

# ── Options ────────────────────────────────────────────────────
option(ALVR_BUNDLED_FFMPEG
    "Load a bundled FFmpeg with dlopen at runtime instead of linking the system FFmpeg (Linux only)"
    OFF)
option(ALVR_GPL
    "Enable GPL-licensed features (FFmpeg software encoding on Windows)"
    OFF)
set(ALVR_FFMPEG_DIR "" CACHE PATH
    "Root of a prebuilt shared FFmpeg (containing include/ and lib/). \
Required for ALVR_BUNDLED_FFMPEG and for ALVR_GPL on Windows.")

# ── Global build settings ──────────────────────────────────────
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# C++20 modules are not used; skip the dependency-scanning pass.
set(CMAKE_CXX_SCAN_FOR_MODULES OFF)

# The library is linked into a Rust cdylib, which is always
# position-independent code.
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# Everything here is consumed as static archives by the Rust link
# (build.rs assumes `rustc-link-lib=static=...`); never let an inherited
# BUILD_SHARED_LIBS turn the unqualified add_library() in fetched
# dependencies (nanors) into shared libraries.
set(BUILD_SHARED_LIBS OFF)

# rustc always links the release (non-debug) MSVC CRT, so the C++ side
# must never use the debug runtime, even in Debug builds.
if(MSVC)
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreadedDLL")

    # Strip /RTC1 from CMake's Debug defaults: a runtime-check failure
    # fail-fasts the whole vrserver process with no diagnostics. Re-add
    # manually when hunting memory bugs under an attached debugger.
    string(REPLACE "/RTC1" "" CMAKE_CXX_FLAGS_DEBUG "${CMAKE_CXX_FLAGS_DEBUG}")
    string(REPLACE "/RTC1" "" CMAKE_C_FLAGS_DEBUG "${CMAKE_C_FLAGS_DEBUG}")
endif()

# ── Dependencies ───────────────────────────────────────────────
include(FetchContent)
FetchContent_Declare(nanors
    GIT_REPOSITORY https://github.com/korejan/nanors.git
    GIT_TAG korejan/main
    GIT_SHALLOW TRUE
    GIT_PROGRESS TRUE
)
set(NANORS_BUILD_TESTS OFF)
set(NANORS_INSTALL OFF)
FetchContent_MakeAvailable(nanors)

add_subdirectory(openvr)

# ── Driver library ─────────────────────────────────────────────
add_library(alvr_server_cpp STATIC)
add_library(alvr::server_cpp ALIAS alvr_server_cpp)

target_sources(alvr_server_cpp PRIVATE
    ALVR-common/common-utils.cpp
    ALVR-common/exception.cpp
    alvr_server/alvr_server.cpp
    alvr_server/ChaperoneUpdater.cpp
    alvr_server/ClientConnection.cpp
    alvr_server/driverlog.cpp
    alvr_server/IDRScheduler.cpp
    alvr_server/Logger.cpp
    alvr_server/OvrController.cpp
    alvr_server/OvrHMD.cpp
    alvr_server/OvrViveTrackerProxy.cpp
    alvr_server/Paths.cpp
    alvr_server/PoseHistory.cpp
    alvr_server/Settings.cpp
    alvr_server/TrackedDevice.cpp
    alvr_server/Utils.cpp
    alvr_server/VSyncThread.cpp
    shared/threadtools.cpp
)

target_compile_features(alvr_server_cpp PUBLIC c_std_11 cxx_std_20)

# Sources use include paths relative to this directory
# (e.g. "alvr_server/bindings.h", "platform/linux/CEncoder.h").
target_include_directories(alvr_server_cpp PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})

target_link_libraries(alvr_server_cpp PRIVATE
    nanors::nanors
    openvr::openvr_api
)

if(MSVC)
    target_compile_options(alvr_server_cpp PRIVATE /W4 /permissive-)
    # Allow FP contraction in optimized builds (never on by default with
    # MSVC); optimization levels themselves come from the CMake build type.
    target_compile_options(alvr_server_cpp PRIVATE
        $<$<NOT:$<CONFIG:Debug>>:/fp:contract>)
else()
    target_compile_options(alvr_server_cpp PRIVATE -Wall -Wextra)
endif()

# ── FFmpeg helper ──────────────────────────────────────────────
# Adds FFmpeg headers (and, for standalone CMake builds, link info) to
# the target. ALVR_FFMPEG_DIR takes precedence over pkg-config.
function(alvr_use_ffmpeg target)
    if(ALVR_FFMPEG_DIR)
        if(NOT EXISTS "${ALVR_FFMPEG_DIR}/include")
            message(FATAL_ERROR "ALVR_FFMPEG_DIR does not contain include/: ${ALVR_FFMPEG_DIR}")
        endif()
        target_include_directories(${target} SYSTEM PRIVATE "${ALVR_FFMPEG_DIR}/include")
    else()
        find_package(PkgConfig REQUIRED)
        pkg_check_modules(FFMPEG REQUIRED IMPORTED_TARGET
            libavutil libavcodec libavfilter libswscale)
        target_link_libraries(${target} PRIVATE PkgConfig::FFMPEG)
    endif()
endfunction()

# ── Platform-specific parts ────────────────────────────────────
if(WIN32)
    target_sources(alvr_server_cpp PRIVATE
        platform/win32/CEncoder.cpp
        platform/win32/FFR.cpp
        platform/win32/FrameRender.cpp
        platform/win32/NvEncoder.cpp
        platform/win32/NvEncoderD3D11.cpp
        platform/win32/OvrDirectModeComponent.cpp
        platform/win32/VideoEncoder.cpp
        platform/win32/VideoEncoderNVENC.cpp
        platform/win32/VideoEncoderSW.cpp
        platform/win32/VideoEncoderVCE.cpp
        platform/win32/d3d-render-utils/RenderPipeline.cpp
        platform/win32/d3d-render-utils/RenderUtils.cpp
        platform/win32/shared/d3drender.cpp
        platform/win32/shared/systemtime.cpp
        # vendored AMD AMF SDK
        platform/win32/amf/public/common/AMFFactory.cpp
        platform/win32/amf/public/common/AMFSTL.cpp
        platform/win32/amf/public/common/CurrentTimeImpl.cpp
        platform/win32/amf/public/common/DataStreamFactory.cpp
        platform/win32/amf/public/common/DataStreamFile.cpp
        platform/win32/amf/public/common/DataStreamMemory.cpp
        platform/win32/amf/public/common/IOCapsImpl.cpp
        platform/win32/amf/public/common/PropertyStorageExImpl.cpp
        platform/win32/amf/public/common/Thread.cpp
        platform/win32/amf/public/common/TraceAdapter.cpp
        platform/win32/amf/public/common/Windows/ThreadWindows.cpp
    )

    target_compile_definitions(alvr_server_cpp PRIVATE
        NOMINMAX
        _WINSOCKAPI_
    )

    if(ALVR_GPL)
        target_compile_definitions(alvr_server_cpp PRIVATE ALVR_GPL)
        alvr_use_ffmpeg(alvr_server_cpp)
    endif()
elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux")
    target_sources(alvr_server_cpp PRIVATE
        platform/linux/CEncoder.cpp
        platform/linux/EncodePipeline.cpp
        platform/linux/EncodePipelineNvEnc.cpp
        platform/linux/EncodePipelineSW.cpp
        platform/linux/EncodePipelineVAAPI.cpp
        platform/linux/ffmpeg_helper.cpp
        platform/linux/generated/avcodec_loader.cpp
        platform/linux/generated/avfilter_loader.cpp
        platform/linux/generated/avutil_loader.cpp
        platform/linux/generated/swscale_loader.cpp
    )

    if(ALVR_BUNDLED_FFMPEG AND NOT ALVR_FFMPEG_DIR)
        # Without this, the build would silently take the FFmpeg major
        # versions from the system headers while dlopening the (potentially
        # different) bundled libraries at runtime.
        message(FATAL_ERROR
            "ALVR_BUNDLED_FFMPEG requires ALVR_FFMPEG_DIR to point at the "
            "prebuilt FFmpeg that will be bundled with the driver.")
    endif()

    alvr_use_ffmpeg(alvr_server_cpp)
    if(ALVR_BUNDLED_FFMPEG)
        # The generated loaders dlopen the bundled FFmpeg libraries at
        # runtime instead of binding to symbols resolved at link time.
        target_compile_definitions(alvr_server_cpp PRIVATE
            LIBRARY_LOADER_AVCODEC_LOADER_H_DLOPEN
            LIBRARY_LOADER_AVUTIL_LOADER_H_DLOPEN
            LIBRARY_LOADER_AVFILTER_LOADER_H_DLOPEN
            LIBRARY_LOADER_SWSCALE_LOADER_H_DLOPEN
        )
    endif()

    find_package(Vulkan REQUIRED)
    target_link_libraries(alvr_server_cpp PRIVATE Vulkan::Vulkan)
endif()
# macOS: no platform sources; platform/macos/CEncoder.h is header-only.

# ── Install ────────────────────────────────────────────────────
# A fixed lib/ destination keeps the layout predictable for build.rs.
install(TARGETS alvr_server_cpp nanors ARCHIVE DESTINATION lib)
