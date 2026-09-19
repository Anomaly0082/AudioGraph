#pragma once

#include "audioprocess/audio_node.h"

#include <cstdint>
#include <memory>
#include <vector>

namespace audioprocess {

class ProcessingChain {
public:
    void add_node(std::unique_ptr<IAudioNode> node);
    void prepare(const AudioFormat& format, std::uint32_t maximum_block_frames);
    void process(AudioBlock& block) noexcept;
    void reset() noexcept;

    [[nodiscard]] std::uint32_t latency_frames() const noexcept;
    [[nodiscard]] std::size_t node_count() const noexcept { return nodes_.size(); }

private:
    std::vector<std::unique_ptr<IAudioNode>> nodes_;
    AudioFormat format_{};
    std::uint32_t maximum_block_frames_{};
    bool prepared_{};
};

}  // namespace audioprocess

