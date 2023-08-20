#pragma once

#ifdef __cplusplus
extern "C" {;
#endif

#include <stdint.h>

typedef struct EyeFov {
    float left = 49.;
    float right = 45.;
    float top = 50.;
    float bottom = 48.;
} EyeFov;

typedef struct TrackingQuat {
    float x;
    float y;
    float z;
    float w;
} TrackingQuat;
typedef struct TrackingVector3 {
    float x;
    float y;
    float z;
} TrackingVector3;
typedef struct TrackingVector2 {
    float x;
    float y;
} TrackingVector2;

typedef struct TrackingPosef {
    TrackingQuat    orientation;
    TrackingVector3 position;
} TrackingPosef;

typedef struct TrackingInfo {
    static constexpr const uint32_t MAX_CONTROLLERS = 2;
    static constexpr const uint32_t BONE_COUNT = 19;

    struct Controller {
        // Tracking info of hand. A3
        TrackingQuat    boneRotations[BONE_COUNT];
        TrackingVector3 bonePositionsBase[BONE_COUNT];
        TrackingPosef   boneRootPose;

        // Tracking info of controller. (float * 19 = 76 bytes)
        TrackingPosef pose;
        TrackingVector3 angularVelocity;
        TrackingVector3 linearVelocity;

        TrackingVector2 joystickPosition;
        TrackingVector2 trackpadPosition;

        uint64_t buttons;

        float triggerValue;
        float gripValue;

        uint32_t handFingerConfidences;

        bool enabled;
        bool isHand;
    } controller[MAX_CONTROLLERS];
    
    TrackingPosef headPose;
    uint64_t      targetTimestampNs;
    uint8_t       mounted;
} TrackingInfo;
// Client >----(mode 0)----> Server
// Client <----(mode 1)----< Server
// Client >----(mode 2)----> Server
// Client <----(mode 3)----< Server
typedef struct TimeSync {
    uint32_t mode; // 0,1,2,3
    uint64_t sequence;
    uint64_t serverTime;
    uint64_t clientTime;

    // Following value are filled by client only when mode=0.
    uint64_t packetsLostTotal;
    uint64_t packetsLostInSecond;

    uint64_t averageDecodeLatency;

    uint32_t averageTotalLatency;

    uint32_t averageSendLatency;

    uint32_t averageTransportLatency;
    
    uint32_t idleTime;

    uint64_t fecFailureInSecond;
    uint64_t fecFailureTotal;
    uint32_t fecFailure;

    float fps;

    // Following value are filled by server only when mode=3.
    uint64_t trackingRecvFrameIndex;

    // Following value are filled by server only when mode=1.
    uint32_t serverTotalLatency;
} TimeSync;
typedef struct VideoFrame {
    uint32_t type; // ALVR_PACKET_TYPE_VIDEO_FRAME
    uint32_t packetCounter;
    uint64_t trackingFrameIndex;
    // FEC decoder needs some value for identify video frame number to detect new frame.
    // trackingFrameIndex becomes sometimes same value as previous video frame (in case of low
    // tracking rate).
    uint64_t videoFrameIndex;
    uint64_t sentTime;
    uint32_t frameByteSize;
    uint32_t fecIndex;
    uint16_t fecPercentage;
    // char frameBuffer[];
} VideoFrame;
enum OpenvrPropertyType {
    Bool,
    Float,
    Int32,
    Uint64,
    Vector3,
    Double,
    String,
};

union OpenvrPropertyValue {
    bool bool_;
    float float_;
    int32_t int32;
    uint64_t uint64;
    float vector3[3];
    double double_;
    char string[64];
};

struct OpenvrProperty {
    uint32_t key;
    OpenvrPropertyType type;
    OpenvrPropertyValue value;
};

typedef struct HiddenAreaMesh {
    const TrackingVector2* vertices;
    unsigned int vertexCount;
    const unsigned int* indices;
    unsigned int indexCount;
} HiddenAreaMesh;

struct ViewsConfigData {
    EyeFov fov[2];
    float ipd_m;
    HiddenAreaMesh hidden_area_mesh[2];
};

extern "C" const unsigned char *FRAME_RENDER_VS_CSO_PTR;
extern "C" unsigned int FRAME_RENDER_VS_CSO_LEN;
extern "C" const unsigned char *FRAME_RENDER_PS_CSO_PTR;
extern "C" unsigned int FRAME_RENDER_PS_CSO_LEN;
extern "C" const unsigned char *QUAD_SHADER_CSO_PTR;
extern "C" unsigned int QUAD_SHADER_CSO_LEN;
extern "C" const unsigned char *COMPRESS_AXIS_ALIGNED_CSO_PTR;
extern "C" unsigned int COMPRESS_AXIS_ALIGNED_CSO_LEN;
extern "C" const unsigned char *COLOR_CORRECTION_CSO_PTR;
extern "C" unsigned int COLOR_CORRECTION_CSO_LEN;

extern "C" const char *g_sessionPath;
extern "C" const char *g_driverRootDir;

extern "C" void (*LogError)(const char *stringPtr);
extern "C" void (*LogWarn)(const char *stringPtr);
extern "C" void (*LogInfo)(const char *stringPtr);
extern "C" void (*LogDebug)(const char *stringPtr);
extern "C" void (*DriverReadyIdle)(bool setDefaultChaprone);
extern "C" void (*VideoSend)(const VideoFrame* header, const uint8_t *buf, uint32_t len);
extern "C" void (*HapticsSend)(unsigned long long path,
                               float duration_s,
                               float frequency,
                               float amplitude);
extern "C" void (*TimeSyncSend)(const TimeSync* packet);
extern "C" void (*ShutdownRuntime)();
extern "C" unsigned long long (*PathStringToHash)(const char *path);

extern "C" void *CppEntryPoint(const char *pInterfaceName, int *pReturnCode);
extern "C" void InitializeStreaming();
extern "C" void DeinitializeStreaming();
extern "C" void RequestIDR();
extern "C" void SetChaperone(float areaWidth, float areaHeight);
extern "C" void InputReceive(const TrackingInfo* data);
extern "C" void TimeSyncReceive(const TimeSync* data);
extern "C" void VideoErrorReportReceive();
extern "C" void ShutdownSteamvr();

extern "C" void SetOpenvrProperty(uint64_t topLevelPath, OpenvrProperty prop);
extern "C" void SetViewsConfig(const ViewsConfigData* config);
extern "C" void SetBattery(uint64_t topLevelPath, float gauge_value, bool is_plugged);

#ifdef __cplusplus
}
#endif
