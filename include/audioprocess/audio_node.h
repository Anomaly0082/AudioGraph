#pragma once

#include "audioprocess/audio_block.h"
#include "audioprocess/audio_format.h"

#include <cstdint>
#include <string_view>

namespace audioprocess {

class IAudioNode {
public:
    virtual ~IAudioNode() = default;

    virtual void prepare(const AudioFormat& format, std::uint32_t maximum_block_frames) = 0;
    virtual void process(AudioBlock& block) noexcept = 0;
    virtual void reset() noexcept = 0;

    [[nodiscard]] virtual std::uint32_t latency_frames() const noexcept = 0;
    [[nodiscard]] virtual std::string_view name() const noexcept = 0;
};

}  // namespace audioprocess

