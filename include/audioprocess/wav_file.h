#pragma once

#include "audioprocess/audio_block.h"
#include "audioprocess/audio_buffer.h"
#include "audioprocess/audio_format.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <optional>
#include <memory>
#include <span>
#include <vector>

namespace audioprocess {

namespace detail { class ExclusiveFile; }

class WavFileSource {
public:
    WavFileSource(const std::filesystem::path& path, std::uint32_t maximum_block_frames,
                  std::optional<std::size_t> maximum_header_chunks = std::nullopt);

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
    // 仅创建新文件，不覆盖已有目标；写入失败或取消时可能留下部分文件。
    WavFileSink(
        const std::filesystem::path& path,
        AudioFormat format,
        std::uint32_t maximum_block_frames);
    ~WavFileSink();

    WavFileSink(const WavFileSink&) = delete;
    WavFileSink& operator=(const WavFileSink&) = delete;

    void write(const AudioBlock& block);
    // 只读交错 PCM 视图；允许流式节点写入，不需要 const_cast 或第二份音频缓冲。
    void write(std::span<const float> samples, std::uint32_t frame_count);
    void finalize();

    [[nodiscard]] std::uint64_t frames_written() const noexcept { return frames_written_; }

private:
    std::unique_ptr<detail::ExclusiveFile> stream_;
    AudioFormat format_{};
    std::uint32_t maximum_block_frames_{};
    std::uint64_t frames_written_{};
    std::uint64_t data_bytes_written_{};
    std::vector<std::byte> scratch_;
    bool finalized_{};
};

}  // namespace audioprocess
