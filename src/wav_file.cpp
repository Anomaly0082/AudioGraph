#include "audioprocess/wav_file.h"

#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>
#include <limits>
#include <stdexcept>
#include <string>

namespace audioprocess {
namespace {

void read_exact(std::istream& stream, char* destination, std::streamsize size) {
    stream.read(destination, size);
    if (stream.gcount() != size) {
        throw std::runtime_error("Unexpected end of WAV file");
    }
}

std::uint16_t read_u16(std::istream& stream) {
    std::array<unsigned char, 2> bytes{};
    read_exact(stream, reinterpret_cast<char*>(bytes.data()), 2);
    return static_cast<std::uint16_t>(bytes[0]) |
        (static_cast<std::uint16_t>(bytes[1]) << 8U);
}

std::uint32_t read_u32(std::istream& stream) {
    std::array<unsigned char, 4> bytes{};
    read_exact(stream, reinterpret_cast<char*>(bytes.data()), 4);
    return static_cast<std::uint32_t>(bytes[0]) |
        (static_cast<std::uint32_t>(bytes[1]) << 8U) |
        (static_cast<std::uint32_t>(bytes[2]) << 16U) |
        (static_cast<std::uint32_t>(bytes[3]) << 24U);
}

void write_u16(std::ostream& stream, std::uint16_t value) {
    const std::array<unsigned char, 2> bytes{
        static_cast<unsigned char>(value & 0xFFU),
        static_cast<unsigned char>((value >> 8U) & 0xFFU),
    };
    stream.write(reinterpret_cast<const char*>(bytes.data()), 2);
}

void write_u32(std::ostream& stream, std::uint32_t value) {
    const std::array<unsigned char, 4> bytes{
        static_cast<unsigned char>(value & 0xFFU),
        static_cast<unsigned char>((value >> 8U) & 0xFFU),
        static_cast<unsigned char>((value >> 16U) & 0xFFU),
        static_cast<unsigned char>((value >> 24U) & 0xFFU),
    };
    stream.write(reinterpret_cast<const char*>(bytes.data()), 4);
}

bool chunk_is(const std::array<char, 4>& actual, const char (&expected)[5]) {
    return std::memcmp(actual.data(), expected, 4) == 0;
}

void seek_forward(std::istream& stream, std::uint64_t byte_count) {
    if (byte_count > static_cast<std::uint64_t>(std::numeric_limits<std::streamoff>::max())) {
        throw std::runtime_error("WAV chunk is too large to seek");
    }
    stream.seekg(static_cast<std::streamoff>(byte_count), std::ios::cur);
    if (!stream) {
        throw std::runtime_error("Failed to seek through WAV chunk");
    }
}

std::int16_t float_to_pcm16(float sample) noexcept {
    if (!std::isfinite(sample)) {
        sample = 0.0F;
    }
    const float clamped = std::clamp(sample, -1.0F, 1.0F);
    if (clamped <= -1.0F) {
        return std::numeric_limits<std::int16_t>::min();
    }
    if (clamped >= 1.0F) {
        return std::numeric_limits<std::int16_t>::max();
    }
    return static_cast<std::int16_t>(std::lrint(clamped * 32768.0F));
}

}  // namespace

WavFileSource::WavFileSource(
    const std::filesystem::path& path,
    std::uint32_t maximum_block_frames)
    : stream_(path, std::ios::binary), maximum_block_frames_(maximum_block_frames) {
    if (maximum_block_frames == 0) {
        throw std::invalid_argument("WAV source block size must be positive");
    }
    if (!stream_) {
        throw std::runtime_error("Unable to open input WAV file: " + path.string());
    }

    std::array<char, 4> id{};
    read_exact(stream_, id.data(), 4);
    if (!chunk_is(id, "RIFF")) {
        throw std::runtime_error("Input file is not a RIFF file");
    }
    (void)read_u32(stream_);
    read_exact(stream_, id.data(), 4);
    if (!chunk_is(id, "WAVE")) {
        throw std::runtime_error("Input RIFF file is not WAVE audio");
    }

    bool found_format = false;
    bool found_data = false;

    while (stream_ && !(found_format && found_data)) {
        stream_.read(id.data(), 4);
        if (stream_.gcount() == 0) {
            break;
        }
        if (stream_.gcount() != 4) {
            throw std::runtime_error("Truncated WAV chunk identifier");
        }

        const auto chunk_size = read_u32(stream_);
        if (chunk_is(id, "fmt ")) {
            if (chunk_size < 16 || chunk_size > 1024) {
                throw std::runtime_error("Unsupported WAV format chunk size");
            }

            const auto audio_format = read_u16(stream_);
            format_.channel_count = read_u16(stream_);
            format_.sample_rate = read_u32(stream_);
            (void)read_u32(stream_);
            block_align_ = read_u16(stream_);
            const auto bits_per_sample = read_u16(stream_);

            if (audio_format != 1 || bits_per_sample != 16) {
                throw std::runtime_error("M0 supports only PCM16 WAV input");
            }
            if (!format_.valid() ||
                block_align_ != static_cast<std::uint16_t>(format_.channel_count * 2U)) {
                throw std::runtime_error("Invalid PCM16 WAV format fields");
            }

            seek_forward(stream_, chunk_size - 16U);
            found_format = true;
        } else if (chunk_is(id, "data")) {
            data_offset_ = static_cast<std::uint64_t>(stream_.tellg());
            data_bytes_ = chunk_size;
            found_data = true;
            if (!found_format) {
                seek_forward(stream_, chunk_size);
            }
        } else {
            seek_forward(stream_, chunk_size);
        }

        if ((chunk_size & 1U) != 0U && !(found_format && found_data)) {
            seek_forward(stream_, 1);
        }
    }

    if (!found_format || !found_data) {
        throw std::runtime_error("WAV file is missing a format or data chunk");
    }
    if (data_bytes_ % block_align_ != 0) {
        throw std::runtime_error("WAV data size is not aligned to complete frames");
    }

    total_frames_ = data_bytes_ / block_align_;
    scratch_.resize(static_cast<std::size_t>(maximum_block_frames_) * block_align_);

    stream_.clear();
    stream_.seekg(static_cast<std::streamoff>(data_offset_), std::ios::beg);
    if (!stream_) {
        throw std::runtime_error("Unable to seek to WAV audio data");
    }
}

std::optional<AudioBlock> WavFileSource::read(AudioBuffer& destination) {
    if (destination.format() != format_) {
        throw std::invalid_argument("WAV source and destination buffer formats do not match");
    }
    if (destination.maximum_frames() > maximum_block_frames_) {
        throw std::invalid_argument("Destination buffer is larger than WAV source capacity");
    }
    if (frames_read_ >= total_frames_) {
        return std::nullopt;
    }

    const auto remaining = total_frames_ - frames_read_;
    const auto frames = static_cast<std::uint32_t>(
        std::min<std::uint64_t>(remaining, destination.maximum_frames()));
    const auto bytes_to_read = static_cast<std::size_t>(frames) * block_align_;

    stream_.read(reinterpret_cast<char*>(scratch_.data()), static_cast<std::streamsize>(bytes_to_read));
    if (stream_.gcount() != static_cast<std::streamsize>(bytes_to_read)) {
        throw std::runtime_error("Unexpected end of PCM16 WAV data");
    }

    auto block = destination.block(frames, frames_read_);
    for (std::size_t index = 0; index < block.sample_count(); ++index) {
        const auto low = std::to_integer<std::uint8_t>(scratch_[index * 2]);
        const auto high = std::to_integer<std::uint8_t>(scratch_[index * 2 + 1]);
        const auto value = static_cast<std::int16_t>(
            static_cast<std::uint16_t>(low) |
            (static_cast<std::uint16_t>(high) << 8U));
        block.samples[index] = static_cast<float>(value) / 32768.0F;
    }

    frames_read_ += frames;
    return block;
}

WavFileSink::WavFileSink(
    const std::filesystem::path& path,
    AudioFormat format,
    std::uint32_t maximum_block_frames)
    : stream_(path, std::ios::binary | std::ios::trunc),
      format_(format),
      maximum_block_frames_(maximum_block_frames) {
    if (!format.valid()) {
        throw std::invalid_argument("WAV sink requires a valid audio format");
    }
    if (maximum_block_frames == 0) {
        throw std::invalid_argument("WAV sink block size must be positive");
    }
    if (!stream_) {
        throw std::runtime_error("Unable to open output WAV file: " + path.string());
    }

    scratch_.resize(
        static_cast<std::size_t>(maximum_block_frames_) * format_.channel_count * 2U);

    stream_.write("RIFF", 4);
    write_u32(stream_, 0);
    stream_.write("WAVE", 4);
    stream_.write("fmt ", 4);
    write_u32(stream_, 16);
    write_u16(stream_, 1);
    write_u16(stream_, format_.channel_count);
    write_u32(stream_, format_.sample_rate);
    const auto block_align = static_cast<std::uint16_t>(format_.channel_count * 2U);
    write_u32(stream_, format_.sample_rate * block_align);
    write_u16(stream_, block_align);
    write_u16(stream_, 16);
    stream_.write("data", 4);
    write_u32(stream_, 0);

    if (!stream_) {
        throw std::runtime_error("Unable to write PCM16 WAV header");
    }
}

WavFileSink::~WavFileSink() {
    try {
        finalize();
    } catch (...) {
    }
}

void WavFileSink::write(const AudioBlock& block) {
    if (finalized_) {
        throw std::logic_error("Cannot write to a finalized WAV file");
    }
    if (!block.valid() || block.channel_count != format_.channel_count) {
        throw std::invalid_argument("WAV sink received an invalid or incompatible AudioBlock");
    }
    if (block.frame_count > maximum_block_frames_) {
        throw std::invalid_argument("AudioBlock exceeds WAV sink capacity");
    }

    const auto byte_count = block.sample_count() * 2U;
    if (data_bytes_written_ + byte_count > std::numeric_limits<std::uint32_t>::max()) {
        throw std::runtime_error("M0 WAV writer does not support files larger than 4 GiB");
    }

    for (std::size_t index = 0; index < block.sample_count(); ++index) {
        const auto value = static_cast<std::uint16_t>(float_to_pcm16(block.samples[index]));
        scratch_[index * 2] = static_cast<std::byte>(value & 0xFFU);
        scratch_[index * 2 + 1] = static_cast<std::byte>((value >> 8U) & 0xFFU);
    }

    stream_.write(
        reinterpret_cast<const char*>(scratch_.data()),
        static_cast<std::streamsize>(byte_count));
    if (!stream_) {
        throw std::runtime_error("Unable to write PCM16 WAV audio data");
    }

    frames_written_ += block.frame_count;
    data_bytes_written_ += byte_count;
}

void WavFileSink::finalize() {
    if (finalized_) {
        return;
    }

    if (data_bytes_written_ > std::numeric_limits<std::uint32_t>::max() - 36U) {
        throw std::runtime_error("M0 WAV writer cannot finalize a file larger than 4 GiB");
    }

    stream_.seekp(4, std::ios::beg);
    write_u32(stream_, static_cast<std::uint32_t>(36U + data_bytes_written_));
    stream_.seekp(40, std::ios::beg);
    write_u32(stream_, static_cast<std::uint32_t>(data_bytes_written_));
    stream_.flush();
    if (!stream_) {
        throw std::runtime_error("Unable to finalize PCM16 WAV header");
    }

    finalized_ = true;
}

}  // namespace audioprocess

