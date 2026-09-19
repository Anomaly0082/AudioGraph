#pragma once

#include <cstddef>
#include <cstdint>
#include <span>

namespace audioprocess {

struct AudioBlock {
    std::span<float> samples;
    std::uint32_t frame_count{};
    std::uint16_t channel_count{};
    std::uint64_t frame_position{};

    [[nodiscard]] constexpr std::size_t sample_count() const noexcept {
        return static_cast<std::size_t>(frame_count) * channel_count;
    }

    [[nodiscard]] constexpr bool valid() const noexcept {
        return channel_count > 0 && samples.size() == sample_count();
    }
};

}  // namespace audioprocess

