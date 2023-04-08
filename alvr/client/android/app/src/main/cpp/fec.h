#ifndef ALVRCLIENT_FEC_H
#define ALVRCLIENT_FEC_H

#include <cstddef>
#include <cstdint>
#include <cassert>
#include <memory>
#include <span>
#include <vector>
#include <mutex>
#include "packet_types.h"
#include "reedsolomon/rs.h"

class FECQueue {
public:
    FECQueue();

    using VideoPacket = std::span<const std::uint8_t>;
    void addVideoPacket(const VideoFrame& header, const VideoPacket& packet, bool& fecFailure);

    void addVideoPacket(const VideoFrame* packet, std::size_t packetSize, bool& fecFailure) {
        assert(packet != nullptr && packetSize > sizeof(VideoFrame));
        addVideoPacket(*packet, {
            reinterpret_cast<const std::uint8_t*>(packet) + sizeof(VideoFrame),
            packetSize - sizeof(VideoFrame)
        }, fecFailure);
    }

    bool reconstruct();
    const std::byte *getFrameBuffer() const;
    int getFrameByteSize() const;

    bool fecFailure() const;
    void clearFecFailure();

    FECQueue(const FECQueue&) = delete;
    FECQueue& operator=(const FECQueue&) = delete;
private:

    VideoFrame m_currentFrame;
    size_t m_shardPackets;
    size_t m_blockSize;
    size_t m_totalDataShards;
    size_t m_totalParityShards;
    size_t m_totalShards;
    uint32_t m_firstPacketOfNextFrame = 0;
    std::vector<std::vector<unsigned char>> m_marks;
    std::vector<std::byte> m_frameBuffer;
    std::vector<uint32_t> m_receivedDataShards;
    std::vector<uint32_t> m_receivedParityShards;
    std::vector<bool> m_recoveredPacket;
    std::vector<std::byte *> m_shards;
    bool m_recovered;
    bool m_fecFailure;

    struct reed_solomon_deleter {
        inline void operator()(reed_solomon* rs_ptr) const {
            if (rs_ptr != nullptr) {
                reed_solomon_release(rs_ptr);
            }
        }
    };
    using reed_solomon_ptr = std::unique_ptr<reed_solomon, reed_solomon_deleter>;
    reed_solomon_ptr m_rs{ nullptr };

    static std::once_flag reed_solomon_initialized;
};

#endif //ALVRCLIENT_FEC_H
