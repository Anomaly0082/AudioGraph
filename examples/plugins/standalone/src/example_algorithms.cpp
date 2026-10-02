#include "example_algorithms.h"

#include <cmath>
#include <limits>

namespace example_algorithms {
bool finite_block(std::span<const float> input) noexcept {
    for (const auto sample : input) {
        if (!std::isfinite(sample)) return false;
    }
    return true;
}

bool gain_block(std::span<const float> input, double factor, std::span<float> output) noexcept {
    if (input.size() != output.size() || !std::isfinite(factor)) return false;
    for (std::size_t i = 0; i < input.size(); ++i) {
        if (!std::isfinite(input[i])) return false;
        const double result = static_cast<double>(input[i])*factor;
        if (!std::isfinite(result) || std::abs(result) > std::numeric_limits<float>::max()) return false;
        output[i] = static_cast<float>(result);
    }
    return true;
}

std::string mock_transcript(std::uint64_t frames, std::uint32_t channels, std::uint32_t sample_rate) {
    return "[MOCK ASR] frames="+std::to_string(frames)+", channels="+std::to_string(channels)
        +", sample_rate="+std::to_string(sample_rate)+"; no speech recognition performed.";
}
}
