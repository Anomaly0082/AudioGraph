#pragma once

#include "audioprocess/audio_node.h"

#include <cstdint>
#include <stdexcept>
#include <string_view>

namespace audioprocess {

class BypassNode final : public IAudioNode {
public:
    void prepare(const AudioFormat& format, std::uint32_t maximum_block_frames) override {
        if (!format.valid() || maximum_block_frames == 0) {
            throw std::invalid_argument("BypassNode received an invalid processing configuration");
        }
    }

    void process(AudioBlock& block) noexcept override {
        (void)block;
    }

    void reset() noexcept override {}

    [[nodiscard]] std::uint32_t latency_frames() const noexcept override { return 0; }
    [[nodiscard]] std::string_view name() const noexcept override { return "Bypass"; }
};

}  // namespace audioprocess

