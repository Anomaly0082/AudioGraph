#pragma once

#include <cstdint>

namespace audioprocess {

struct AudioFormat {
    std::uint32_t sample_rate{};
    std::uint16_t channel_count{};

    [[nodiscard]] constexpr bool valid() const noexcept {
        return sample_rate > 0 && channel_count > 0;
    }

    friend constexpr bool operator==(const AudioFormat&, const AudioFormat&) = default;
};

}  // namespace audioprocess

