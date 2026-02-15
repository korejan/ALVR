#include "ClientConnection.h"
#include <mutex>
#include <string.h>

#include "rs_auto.h"

#include "Logger.h"
#include "Settings.h"
#include "Statistics.h"
#include "Utils.h"
#include "bindings.h"

const int64_t STATISTICS_TIMEOUT_US = 100 * 1000;

ClientConnection::ClientConnection() : m_LastStatisticsUpdate(0) {

    m_Statistics = std::make_shared<Statistics>();

    reed_solomon_init();

    videoPacketCounter = 0;
    m_fecPercentage = INITIAL_FEC_PERCENTAGE;
    memset(&m_reportedStatistics, 0, sizeof(m_reportedStatistics));
    m_Statistics->ResetAll();
}

void ClientConnection::FECSend(uint8_t *buf,
                               uint32_t len,
                               uint64_t targetTimestampNs,
                               uint64_t videoFrameIndex) {
    const uint32_t shardPackets = CalculateFECShardPackets(len, m_fecPercentage);

    const uint32_t blockSize = shardPackets * ALVR_MAX_VIDEO_BUFFER_SIZE;

    const uint32_t dataShards = (len + blockSize - 1) / blockSize;
    const uint32_t totalParityShards = CalculateParityShards(dataShards, m_fecPercentage);
    const uint32_t totalShards = dataShards + totalParityShards;

    assert(totalShards <= DATA_SHARDS_MAX);

    Debug("reed_solomon_new. dataShards=%d totalParityShards=%d totalShards=%d blockSize=%d "
          "shardPackets=%d\n",
          dataShards,
          totalParityShards,
          totalShards,
          blockSize,
          shardPackets);

    uint8_t *shards[DATA_SHARDS_MAX];
    // Data shards point directly into input buffer
    for (uint32_t i = 0; i < dataShards; ++i) {
        shards[i] = buf + i * blockSize;
    }

    // Handle padding for last data shard if needed
    if (len % blockSize != 0) {
        const size_t requiredSize = static_cast<size_t>(blockSize);
        if (m_fecPaddingBuffer.size() < requiredSize) {
            m_fecPaddingBuffer.resize(requiredSize);
        }
        const size_t lastShardOffset = (dataShards - 1) * blockSize;
        const size_t remainingBytes = len % blockSize;
        memcpy(m_fecPaddingBuffer.data(), buf + lastShardOffset, remainingBytes);
        memset(m_fecPaddingBuffer.data() + remainingBytes, 0, blockSize - remainingBytes);
        shards[dataShards - 1] = m_fecPaddingBuffer.data();
    }

    // Ensure parity buffer is large enough
    const size_t requiredParitySize = static_cast<size_t>(totalParityShards) * blockSize;
    if (m_fecParityBuffer.size() < requiredParitySize) {
        m_fecParityBuffer.resize(requiredParitySize);
    }

    // Parity shards point into contiguous parity buffer
    for (uint32_t i = 0; i < totalParityShards; ++i) {
        shards[dataShards + i] = m_fecParityBuffer.data() + i * blockSize;
    }

    const size_t rsBufSize = reed_solomon_bufsize(dataShards, totalParityShards);
    if (m_rsBuffer.size() < rsBufSize) {
        m_rsBuffer.resize(rsBufSize);
    }
    reed_solomon *rs =
        reed_solomon_new_static(m_rsBuffer.data(), rsBufSize, dataShards, totalParityShards);

    const int ret = reed_solomon_encode(rs, shards, totalShards, blockSize);
    assert(ret == 0);

    int32_t dataRemain = static_cast<int32_t>(len);

    VideoFrame header = {
        .type = ALVR_PACKET_TYPE_VIDEO_FRAME,
        .trackingFrameIndex = targetTimestampNs,
        .videoFrameIndex = videoFrameIndex,
        .sentTime = GetSystemTimestampUs(),
        .frameByteSize = len,
        .fecIndex = 0,
        .fecPercentage = (uint16_t)m_fecPercentage,
    };
    for (uint32_t i = 0; i < dataShards; ++i) {
        for (uint32_t j = 0; j < shardPackets; ++j) {
            const int32_t copyLength =
                std::min(static_cast<int32_t>(ALVR_MAX_VIDEO_BUFFER_SIZE), dataRemain);
            if (copyLength <= 0) {
                break;
            }

            const uint8_t *payload = shards[i] + j * ALVR_MAX_VIDEO_BUFFER_SIZE;
            dataRemain -= ALVR_MAX_VIDEO_BUFFER_SIZE;
            header.packetCounter = videoPacketCounter;
            ++videoPacketCounter;

            VideoSend(&header, payload, copyLength);
            m_Statistics->CountPacket(sizeof(VideoFrame) + copyLength);
            ++header.fecIndex;
        }
    }
    header.fecIndex = dataShards * shardPackets;
    for (uint32_t i = 0; i < totalParityShards; ++i) {
        for (uint32_t j = 0; j < shardPackets; ++j) {
            const uint32_t copyLength = ALVR_MAX_VIDEO_BUFFER_SIZE;

            const uint8_t *payload = shards[dataShards + i] + j * ALVR_MAX_VIDEO_BUFFER_SIZE;
            header.packetCounter = videoPacketCounter;
            ++videoPacketCounter;

            VideoSend(&header, payload, copyLength);
            m_Statistics->CountPacket(sizeof(VideoFrame) + copyLength);
            ++header.fecIndex;
        }
    }
}

void ClientConnection::SendVideo(uint8_t *buf, uint32_t len, uint64_t targetTimestampNs) {
    if (Settings::Instance().m_enableFec) {
        FECSend(buf, len, targetTimestampNs, mVideoFrameIndex);
    } else {
        const VideoFrame header = {
            .packetCounter = this->videoPacketCounter,
            .trackingFrameIndex = targetTimestampNs,
            .videoFrameIndex = mVideoFrameIndex,
            .sentTime = GetSystemTimestampUs(),
            .frameByteSize = len,
        };
        VideoSend(&header, buf, len);

        m_Statistics->CountPacket(sizeof(VideoFrame) + len);

        this->videoPacketCounter++;
    }

    mVideoFrameIndex++;
}

void ClientConnection::ProcessTimeSync(const TimeSync &data) {
    m_Statistics->CountPacket(sizeof(TrackingInfo));

    const TimeSync *const timeSync = &data;
    const std::uint64_t Current = GetSystemTimestampUs();

    if (timeSync->mode == 0) {
        // timings might be a little incorrect since it is a mix from a previous sent frame and
        // latest frame

        vr::Compositor_FrameTiming timing[2];
        timing[0].m_nSize = sizeof(vr::Compositor_FrameTiming);
        vr::VRServerDriverHost()->GetFrameTimings(&timing[0], 2);

        m_reportedStatistics = *timeSync;
        TimeSync sendBuf = *timeSync;
        sendBuf.mode = 1;
        sendBuf.serverTime = Current;
        sendBuf.serverTotalLatency =
            (int)(m_reportedStatistics.averageSendLatency +
                  (timing[0].m_flPreSubmitGpuMs + timing[0].m_flPostSubmitGpuMs +
                   timing[0].m_flTotalRenderGpuMs + timing[0].m_flCompositorRenderGpuMs +
                   timing[0].m_flCompositorRenderCpuMs + timing[0].m_flCompositorIdleCpuMs +
                   timing[0].m_flClientFrameIntervalMs + timing[0].m_flPresentCallCpuMs +
                   timing[0].m_flWaitForPresentCpuMs + timing[0].m_flSubmitFrameMs) *
                      1000 +
                  m_Statistics->GetEncodeLatencyAverage() +
                  m_reportedStatistics.averageTransportLatency +
                  m_reportedStatistics.averageDecodeLatency + m_reportedStatistics.idleTime);
        TimeSyncSend(&sendBuf);

        m_Statistics->NetworkTotal(sendBuf.serverTotalLatency);
        m_Statistics->NetworkSend(m_reportedStatistics.averageTransportLatency);

        float renderTime = timing[0].m_flPreSubmitGpuMs + timing[0].m_flPostSubmitGpuMs +
                           timing[0].m_flTotalRenderGpuMs + timing[0].m_flCompositorRenderGpuMs +
                           timing[0].m_flCompositorRenderCpuMs;
        float idleTime = timing[0].m_flCompositorIdleCpuMs;
        float waitTime = timing[0].m_flClientFrameIntervalMs + timing[0].m_flPresentCallCpuMs +
                         timing[0].m_flWaitForPresentCpuMs + timing[0].m_flSubmitFrameMs;

        if (timeSync->fecFailure) {
            OnFecFailure();
        }

        m_Statistics->Add(sendBuf.serverTotalLatency / 1000.0,
                          (double)(m_Statistics->GetEncodeLatencyAverage()) / US_TO_MS,
                          m_reportedStatistics.averageTransportLatency / 1000.0,
                          m_reportedStatistics.averageDecodeLatency / 1000.0,
                          m_reportedStatistics.fps,
                          m_RTT / 2. / 1000.);

        const std::uint64_t now = GetSteadyTimeStampUS();
        if (now - m_LastStatisticsUpdate > STATISTICS_TIMEOUT_US) {
            // Text statistics only, some values averaged
            Info("#{ \"id\": \"Statistics\", \"data\": {"
                 "\"totalPackets\": %llu, "
                 "\"packetRate\": %llu, "
                 "\"packetsLostTotal\": %llu, "
                 "\"packetsLostPerSecond\": %llu, "
                 "\"totalSent\": %llu, "
                 "\"sentRate\": %.3f, "
                 "\"bitrate\": %llu, "
                 "\"ping\": %.3f, "
                 "\"totalLatency\": %.3f, "
                 "\"encodeLatency\": %.3f, "
                 "\"sendLatency\": %.3f, "
                 "\"decodeLatency\": %.3f, "
                 "\"fecPercentage\": %d, "
                 "\"fecFailureTotal\": %llu, "
                 "\"fecFailureInSecond\": %llu, "
                 "\"clientFPS\": %.3f, "
                 "\"serverFPS\": %.3f, "
                 "\"batteryHMD\": %d, "
                 "\"batteryLeft\": %d, "
                 "\"batteryRight\": %d"
                 "} }#\n",
                 m_Statistics->GetPacketsSentTotal(),
                 m_Statistics->GetPacketsSentInSecond(),
                 m_reportedStatistics.packetsLostTotal,
                 m_reportedStatistics.packetsLostInSecond,
                 m_Statistics->GetBitsSentTotal() / 8 / 1000 / 1000,
                 m_Statistics->GetBitsSentInSecond() / 1000. / 1000.0,
                 m_Statistics->GetBitrate(),
                 m_Statistics->Get(5), // ping
                 m_Statistics->Get(0), // totalLatency
                 m_Statistics->Get(1), // encodeLatency
                 m_Statistics->Get(2), // sendLatency
                 m_Statistics->Get(3), // decodeLatency
                 m_fecPercentage,
                 m_reportedStatistics.fecFailureTotal,
                 m_reportedStatistics.fecFailureInSecond,
                 m_Statistics->Get(4), // clientFPS
                 m_Statistics->GetFPS(),
                 (int)(m_Statistics->m_hmdBattery * 100),
                 (int)(m_Statistics->m_leftControllerBattery * 100),
                 (int)(m_Statistics->m_rightControllerBattery * 100));

            m_LastStatisticsUpdate = now;
            m_Statistics->Reset();
        };

        // Continously send statistics info for updating graphs
        Info("#{ \"id\": \"GraphStatistics\", \"data\": "
             "[%llu,%.3f,%.3f,%.3f,%.3f,%.3f,%.3f,%.3f,%.3f,%.3f,%.3f,%.3f] }#\n",
             Current / 1000,                                               // time
             sendBuf.serverTotalLatency / 1000.0,                          // totalLatency
             m_reportedStatistics.averageSendLatency / 1000.0,             // receiveLatency
             renderTime,                                                   // renderTime
             idleTime,                                                     // idleTime
             waitTime,                                                     // waitTime
             (double)(m_Statistics->GetEncodeLatencyAverage()) / US_TO_MS, // encodeLatency
             m_reportedStatistics.averageTransportLatency / 1000.0,        // sendLatency
             m_reportedStatistics.averageDecodeLatency / 1000.0,           // decodeLatency
             m_reportedStatistics.idleTime / 1000.0,                       // clientIdleTime
             m_reportedStatistics.fps,                                     // clientFPS
             m_Statistics->GetFPS());                                      // serverFPS

    } else if (timeSync->mode == 2) {
        // Calclate RTT
        uint64_t RTT = Current - timeSync->serverTime;
        m_RTT = RTT;
        // Estimated difference between server and client clock
        int64_t TimeDiff = Current - (timeSync->clientTime + RTT / 2);
        m_TimeDiff = TimeDiff;
        Debug("TimeSync: server - client = %lld us RTT = %lld us\n", TimeDiff, RTT);
    }
}

float ClientConnection::GetPoseTimeOffset() {
    return -(double)(m_Statistics->GetTotalLatencyAverage()) / 1000.0 / 1000.0;
}

void ClientConnection::OnFecFailure() {
    Debug("Listener::OnFecFailure()\n");
    const std::uint64_t timestamp = GetSteadyTimeStampUS();
    if (timestamp - m_lastFecFailure < CONTINUOUS_FEC_FAILURE) {
        if (m_fecPercentage < MAX_FEC_PERCENTAGE) {
            m_fecPercentage += 5;
        }
    }
    m_lastFecFailure = timestamp;
}

std::shared_ptr<Statistics> ClientConnection::GetStatistics() { return m_Statistics; }
