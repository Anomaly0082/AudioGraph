#include "audioprocess/processing_chain.h"

#include <limits>
#include <stdexcept>
#include <utility>

namespace audioprocess {

void ProcessingChain::add_node(std::unique_ptr<IAudioNode> node) {
    if (!node) {
        throw std::invalid_argument("ProcessingChain cannot add a null node");
    }
    if (prepared_) {
        throw std::logic_error("Nodes cannot be added after ProcessingChain::prepare");
    }
    nodes_.push_back(std::move(node));
}

void ProcessingChain::prepare(const AudioFormat& format, std::uint32_t maximum_block_frames) {
    if (!format.valid()) {
        throw std::invalid_argument("ProcessingChain requires a valid audio format");
    }
    if (maximum_block_frames == 0) {
        throw std::invalid_argument("ProcessingChain block size must be positive");
    }

    for (auto& node : nodes_) {
        node->prepare(format, maximum_block_frames);
    }

    format_ = format;
    maximum_block_frames_ = maximum_block_frames;
    prepared_ = true;
}

void ProcessingChain::process(AudioBlock& block) noexcept {
    if (!prepared_ || !block.valid() || block.channel_count != format_.channel_count ||
        block.frame_count > maximum_block_frames_) {
        return;
    }

    for (auto& node : nodes_) {
        node->process(block);
    }
}

void ProcessingChain::reset() noexcept {
    for (auto& node : nodes_) {
        node->reset();
    }
}

std::uint32_t ProcessingChain::latency_frames() const noexcept {
    std::uint64_t total{};
    for (const auto& node : nodes_) {
        total += node->latency_frames();
    }
    return total > std::numeric_limits<std::uint32_t>::max()
        ? std::numeric_limits<std::uint32_t>::max()
        : static_cast<std::uint32_t>(total);
}

}  // namespace audioprocess

