#pragma once

#include "audioprocess/audio_block.h"
#include "audioprocess/audio_format.h"

#include <cstddef>
#include <cstdint>
#include <stdexcept>
#include <vector>

namespace audioprocess {

class AudioBuffer {
public:
    AudioBuffer(AudioFormat format, std::uint32_t maximum_frames)
        : format_(format),
          maximum_frames_(maximum_frames),
          samples_(static_cast<std::size_t>(maximum_frames) * format.channel_count) {
        if (!format.valid()) {
            throw std::invalid_argument("AudioBuffer requires a valid audio format");
        }
        if (maximum_frames == 0) {
            throw std::invalid_argument("AudioBuffer maximum frame count must be positive");
        }
    }

    [[nodiscard]] AudioBlock block(
        std::uint32_t frame_count,
        std::uint64_t frame_position = 0) {
        if (frame_count > maximum_frames_) {
            throw std::out_of_range("AudioBlock exceeds AudioBuffer capacity");
        }

        const auto count = static_cast<std::size_t>(frame_count) * format_.channel_count;
        return AudioBlock{
            std::span<float>{samples_.data(), count},
            frame_count,
            format_.channel_count,
            frame_position,
        };
    }

    [[nodiscard]] const AudioFormat& format() const noexcept { return format_; }
    [[nodiscard]] std::uint32_t maximum_frames() const noexcept { return maximum_frames_; }

private:
    AudioFormat format_;
    std::uint32_t maximum_frames_;
    std::vector<float> samples_;
};

}  // namespace audioprocess

