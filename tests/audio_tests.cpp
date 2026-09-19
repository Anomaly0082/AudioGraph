#include "audioprocess/audio_buffer.h"
#include "audioprocess/bypass_node.h"
#include "audioprocess/processing_chain.h"
#include "audioprocess/wav_file.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <iostream>
#include <memory>
#include <numbers>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

void require(bool condition, const std::string& message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

std::filesystem::path unique_test_path(std::string_view suffix) {
    const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
    return std::filesystem::temp_directory_path() /
        ("audioprocess_" + std::to_string(stamp) + std::string(suffix));
}

void create_test_wav(const std::filesystem::path& path) {
    constexpr audioprocess::AudioFormat format{48'000, 2};
    constexpr std::uint32_t block_size = 257;
    constexpr std::uint64_t total_frames = 4'321;

    audioprocess::AudioBuffer buffer(format, block_size);
    audioprocess::WavFileSink sink(path, format, block_size);

    std::uint64_t position{};
    while (position < total_frames) {
        const auto frames = static_cast<std::uint32_t>(
            std::min<std::uint64_t>(block_size, total_frames - position));
        auto block = buffer.block(frames, position);

        for (std::uint32_t frame = 0; frame < frames; ++frame) {
            const auto absolute_frame = position + frame;
            const auto phase = 2.0 * std::numbers::pi * 440.0 *
                static_cast<double>(absolute_frame) / format.sample_rate;
            block.samples[static_cast<std::size_t>(frame) * 2] =
                static_cast<float>(0.5 * std::sin(phase));
            block.samples[static_cast<std::size_t>(frame) * 2 + 1] =
                static_cast<float>(0.25 * std::cos(phase));
        }

        sink.write(block);
        position += frames;
    }
    sink.finalize();
}

void run_bypass(
    const std::filesystem::path& input,
    const std::filesystem::path& output,
    std::uint32_t block_size) {
    audioprocess::WavFileSource source(input, block_size);
    audioprocess::AudioBuffer buffer(source.format(), block_size);
    audioprocess::WavFileSink sink(output, source.format(), block_size);

    audioprocess::ProcessingChain chain;
    chain.add_node(std::make_unique<audioprocess::BypassNode>());
    chain.prepare(source.format(), block_size);

    while (auto block = source.read(buffer)) {
        chain.process(*block);
        sink.write(*block);
    }
    sink.finalize();
}

struct DecodedAudio {
    audioprocess::AudioFormat format;
    std::uint64_t frame_count{};
    std::vector<float> samples;
};

DecodedAudio decode_all(const std::filesystem::path& path, std::uint32_t block_size) {
    audioprocess::WavFileSource source(path, block_size);
    audioprocess::AudioBuffer buffer(source.format(), block_size);
    DecodedAudio decoded{source.format(), source.total_frames(), {}};
    decoded.samples.reserve(
        static_cast<std::size_t>(source.total_frames()) * source.format().channel_count);

    while (auto block = source.read(buffer)) {
        decoded.samples.insert(
            decoded.samples.end(), block->samples.begin(), block->samples.end());
    }
    return decoded;
}

void test_bypass_is_sample_exact() {
    const auto input = unique_test_path("_input.wav");
    const auto output_128 = unique_test_path("_output_128.wav");
    const auto output_512 = unique_test_path("_output_512.wav");

    create_test_wav(input);
    run_bypass(input, output_128, 128);
    run_bypass(input, output_512, 512);

    const auto original = decode_all(input, 333);
    const auto processed_128 = decode_all(output_128, 271);
    const auto processed_512 = decode_all(output_512, 509);

    require(original.format == processed_128.format, "128-frame output format changed");
    require(original.format == processed_512.format, "512-frame output format changed");
    require(original.frame_count == processed_128.frame_count, "128-frame output length changed");
    require(original.frame_count == processed_512.frame_count, "512-frame output length changed");
    require(original.samples == processed_128.samples, "Bypass changed samples at block size 128");
    require(original.samples == processed_512.samples, "Bypass changed samples at block size 512");

    std::filesystem::remove(input);
    std::filesystem::remove(output_128);
    std::filesystem::remove(output_512);
}

void test_invalid_block_size_is_rejected() {
    bool rejected = false;
    try {
        [[maybe_unused]] audioprocess::AudioBuffer buffer({48'000, 1}, 0);
    } catch (const std::invalid_argument&) {
        rejected = true;
    }
    require(rejected, "AudioBuffer accepted a zero block size");
}

}  // namespace

int main() {
    try {
        test_bypass_is_sample_exact();
        test_invalid_block_size_is_rejected();
        std::cout << "All audio framework tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Test failure: " << error.what() << '\n';
        return 1;
    }
}

