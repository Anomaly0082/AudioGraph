#pragma once

#include "audioprocess/audio_block.h"
#include "audioprocess/audio_buffer.h"
#include "audioprocess/audio_format.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <optional>
#include <vector>

namespace audioprocess {

class WavFileSource {
public:
    WavFileSource(const std::filesystem::path& path, std::uint32_t maximum_block_frames);

    WavFileSource(const WavFileSource&) = delete;
    WavFileSource& operator=(const WavFileSource&) = delete;

    [[nodiscard]] const AudioFormat& format() const noexcept { return format_; }
    [[nodiscard]] std::uint64_t total_frames() const noexcept { return total_frames_; }
    [[nodiscard]] std::optional<AudioBlock> read(AudioBuffer& destination);

private:
    std::ifstream stream_;
    AudioFormat format_{};
    std::uint16_t block_align_{};
    std::uint64_t data_offset_{};
    std::uint64_t data_bytes_{};
    std::uint64_t total_frames_{};
    std::uint64_t frames_read_{};
    std::uint32_t maximum_block_frames_{};
    std::vector<std::byte> scratch_;
};

class WavFileSink {
public:
    WavFileSink(
        const std::filesystem::path& path,
        AudioFormat format,
        std::uint32_t maximum_block_frames);
    ~WavFileSink();

    WavFileSink(const WavFileSink&) = delete;
    WavFileSink& operator=(const WavFileSink&) = delete;

    void write(const AudioBlock& block);
    void finalize();

    [[nodiscard]] std::uint64_t frames_written() const noexcept { return frames_written_; }

private:
    std::ofstream stream_;
    AudioFormat format_{};
    std::uint32_t maximum_block_frames_{};
    std::uint64_t frames_written_{};
    std::uint64_t data_bytes_written_{};
    std::vector<std::byte> scratch_;
    bool finalized_{};
};

}  // namespace audioprocess

