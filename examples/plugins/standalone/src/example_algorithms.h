#pragma once

#include <cstdint>
#include <span>
#include <string>

namespace example_algorithms {
// Private C++ types never cross the plugin's public C ABI.
bool gain_block(std::span<const float> input, double factor, std::span<float> output) noexcept;
bool finite_block(std::span<const float> input) noexcept;
std::string mock_transcript(std::uint64_t frames, std::uint32_t channels, std::uint32_t sample_rate);
}
