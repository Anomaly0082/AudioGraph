#include "audioprocess/execution_error.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/streaming_node.h"
#include "audioprocess/wav_file.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <limits>
#include <string>
#include <vector>

namespace {
using namespace audioprocess;

void require(bool condition, const std::string& message) {
    if (!condition) { throw std::runtime_error(message); }
}

class TestDirectory {
public:
    TestDirectory() {
        const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
        for (unsigned i = 0; i < 100; ++i) {
            auto candidate = std::filesystem::temp_directory_path() /
                ("audio-stream-nodes-" + std::to_string(stamp) + "-" + std::to_string(i));
            if (std::filesystem::create_directory(candidate)) { path = std::move(candidate); return; }
        }
        throw std::runtime_error("Unable to create isolated stream-node test directory");
    }
    ~TestDirectory() {
        std::error_code ignored;
        if (!path.empty()) { std::filesystem::remove_all(path, ignored); }
    }
    TestDirectory(const TestDirectory&) = delete;
    TestDirectory& operator=(const TestDirectory&) = delete;
    std::filesystem::path path;
};

template<class Function>
void requires_error(const std::string& code, Function action) {
    try { action(); }
    catch (const ExecutionError& error) {
        require(error.code == code, "Unexpected error code: " + error.code + ", expected " + code);
        return;
    }
    throw std::runtime_error("Expected failure: " + code);
}

std::string bytes(const std::filesystem::path& path) {
    std::ifstream file(path, std::ios::binary);
    require(static_cast<bool>(file), "Unable to inspect test file");
    return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}

void test_source_gain_sink(const std::filesystem::path& directory) {
    const auto registry = create_prototype_node_registry();
    const auto input = directory / std::filesystem::path{u8"分块输入.wav"};
    const auto output = directory / std::filesystem::path{u8"分块输出.wav"};
    constexpr AudioFormat format{48000, 2};
    constexpr std::uint32_t total_frames = 1031;
    std::vector<float> original(static_cast<std::size_t>(total_frames) * 2U);
    for (std::size_t i = 0; i < original.size(); ++i) {
        original[i] = (static_cast<float>(i % 31U) - 15.0F) / 32.0F;
    }
    {
        WavFileSink fixture(input, format, total_frames);
        fixture.write(std::span<const float>{original}, total_frames);
        fixture.finalize();
    }

    auto source_owner = registry.create_stream("wav_stream_input", {{"path", input}});
    auto gain_owner = registry.create_stream("stream_gain", {});
    auto sink_owner = registry.create_stream("wav_stream_output", {{"path", output}});
    auto& source = dynamic_cast<IAudioStreamSource&>(*source_owner);
    auto& gain = dynamic_cast<IAudioStreamProcessor&>(*gain_owner);
    auto& sink = dynamic_cast<IAudioStreamSink&>(*sink_owner);
    ExecutionContext context;
    require(!std::filesystem::exists(output), "Constructing a stream sink performed IO");
    requires_error("invalid_lifecycle", [&] { (void)source.read(context); });
    require(source.open(256, context) == format, "Source format changed");
    require(gain.prepare(format, 256, context) == format, "Gain format changed");
    sink.prepare(format, 256, context);

    std::uint64_t position{};
    unsigned callbacks{};
    while (auto block = source.read(context)) {
        require(block->frame_count <= 256 && block->frame_position == position,
                "Source violated block capacity or stream position");
        const auto snapshot = std::vector<float>(block->samples.begin(), block->samples.end());
        gain.push(*block, [&](const AudioStreamBlock& transformed) {
            ++callbacks;
            require(transformed.frame_count == block->frame_count &&
                    transformed.frame_position == block->frame_position &&
                    std::equal(transformed.samples.begin(), transformed.samples.end(), snapshot.begin(), snapshot.end()),
                    "Default Gain changed audio samples or timing");
            sink.push(transformed, context);
        }, context);
        require(std::equal(block->samples.begin(), block->samples.end(), snapshot.begin(), snapshot.end()),
                "Processor changed its borrowed source input");
        position += block->frame_count;
    }
    require(position == total_frames && callbacks == 5, "Source dropped the partial final block");
    require(!source.read(context), "Source produced more audio after EOS");
    gain.finish([&](const AudioStreamBlock&) { throw std::runtime_error("Gain emitted unexpected tail audio"); }, context);
    const auto result = sink.finish(context);
    require(std::get<double>(result.at("frames_written")) == total_frames &&
            std::get<double>(result.at("clipped_samples")) == 0 &&
            std::get<std::filesystem::path>(result.at("path")) == output,
            "Sink summary was incorrect");
    require(bytes(input) == bytes(output), "Default streaming Gain was not PCM16 sample-exact");
    requires_error("invalid_lifecycle", [&] { (void)sink.finish(context); });
    requires_error("invalid_lifecycle", [&] { gain.push({original, total_frames, 2, 0}, {}, context); });
    // finish 必须关闭文件，即使节点对象仍然存活，Windows 下也应可改名。
    const auto renamed = directory / "closed.wav";
    std::filesystem::rename(output, renamed);
    require(std::filesystem::exists(renamed), "Sink did not close its output on finish");
}

void test_gain_parameters_and_sink_clipping(const std::filesystem::path& directory) {
    const auto registry = create_prototype_node_registry();
    auto gain_owner = registry.create_stream("stream_gain", {{"gain_db", 6.020599913279624}});
    auto sink_owner = registry.create_stream("wav_stream_output", {{"path", directory / "clipped.wav"}});
    auto& gain = dynamic_cast<IAudioStreamProcessor&>(*gain_owner);
    auto& sink = dynamic_cast<IAudioStreamSink&>(*sink_owner);
    ExecutionContext context;
    (void)gain.prepare({48000, 1}, 5, context);
    sink.prepare({48000, 1}, 5, context);
    const std::vector<float> samples{0.75F, -0.75F, 0.5F, -0.5F, 0.125F};
    gain.push({samples, 5, 1, 0}, [&](const AudioStreamBlock& block) {
        require(block.samples[0] == 1.5F && block.samples[1] == -1.5F && block.samples[4] == 0.25F,
                "Gain did not apply its configured decibel value");
        sink.push(block, context);
    }, context);
    gain.finish({}, context);
    const auto result = sink.finish(context);
    require(std::get<double>(result.at("clipped_samples")) == 3.0, "Stream clipping count was incorrect");
    WavFileSource decoded(directory / "clipped.wav", 5);
    AudioBuffer buffer(decoded.format(), 5);
    const auto block = decoded.read(buffer);
    require(block && block->samples[0] == 32767.0F / 32768.0F && block->samples[1] == -1.0F &&
            block->samples[2] == 32767.0F / 32768.0F && block->samples[3] == -1.0F &&
            block->samples[4] == 0.25F, "Stream output did not saturate PCM16 correctly");
}

void test_empty_stream_and_cancellation(const std::filesystem::path& directory) {
    const auto registry = create_prototype_node_registry();
    const auto empty_input = directory / "empty-input.wav";
    const auto empty_output = directory / "empty-output.wav";
    { WavFileSink file(empty_input, {44100, 1}, 1); file.finalize(); }
    ExecutionContext context;
    auto source_owner = registry.create_stream("wav_stream_input", {{"path", empty_input}});
    auto sink_owner = registry.create_stream("wav_stream_output", {{"path", empty_output}});
    auto& source = dynamic_cast<IAudioStreamSource&>(*source_owner);
    auto& sink = dynamic_cast<IAudioStreamSink&>(*sink_owner);
    sink.prepare(source.open(1, context), 1, context);
    require(!source.read(context), "Empty source emitted an audio block");
    const auto result = sink.finish(context);
    require(std::get<double>(result.at("frames_written")) == 0 && bytes(empty_input) == bytes(empty_output),
            "Empty stream did not produce a valid empty PCM16 WAV");

    const auto cancelled_output = directory / "cancelled.wav";
    auto cancelled_owner = registry.create_stream("wav_stream_output", {{"path", cancelled_output}});
    auto& cancelled_sink = dynamic_cast<IAudioStreamSink&>(*cancelled_owner);
    std::atomic_bool cancel{true};
    ExecutionContext cancelled_context{&cancel};
    requires_error("cancelled", [&] { cancelled_sink.prepare({48000, 1}, 2, cancelled_context); });
    require(!std::filesystem::exists(cancelled_output), "Pre-cancelled sink created a file");
    cancel.store(false);
    cancelled_sink.prepare({48000, 1}, 2, cancelled_context);
    const std::vector<float> sample{0.25F};
    cancelled_sink.push({sample, 1, 1, 0}, cancelled_context);
    cancel.store(true);
    requires_error("cancelled", [&] { (void)cancelled_sink.finish(cancelled_context); });
    cancelled_owner.reset(); // 异常/取消时由析构关闭，保留可检查的部分文件。
    std::filesystem::rename(cancelled_output, directory / "partial.wav");
    WavFileSource partial(directory / "partial.wav", 2);
    require(partial.total_frames() == 1, "Cancellation cleanup lost the successfully written frame");
}

void test_invalid_blocks_and_protection(const std::filesystem::path& directory) {
    const auto registry = create_prototype_node_registry();
    ExecutionContext context;
    auto gain_owner = registry.create_stream("stream_gain", {});
    auto& gain = dynamic_cast<IAudioStreamProcessor&>(*gain_owner);
    const StreamEmit discard = [](const AudioStreamBlock&) {};
    const std::vector<float> one{0.5F};
    requires_error("invalid_lifecycle", [&] { gain.push({one, 1, 1, 0}, discard, context); });
    (void)gain.prepare({48000, 1}, 2, context);
    requires_error("invalid_stream_block", [&] { gain.push({one, 2, 1, 0}, discard, context); });
    requires_error("invalid_stream_callback", [&] { gain.push({one, 1, 1, 0}, {}, context); });
    const std::vector<float> nan{std::numeric_limits<float>::quiet_NaN()};
    requires_error("invalid_stream_block", [&] { gain.push({nan, 1, 1, 0}, discard, context); });

    const auto existing = directory / "keep.wav";
    { WavFileSink original(existing, {48000, 1}, 1); original.write(one, 1); original.finalize(); }
    const auto before = bytes(existing);
    auto sink_owner = registry.create_stream("wav_stream_output", {{"path", existing}});
    auto& sink = dynamic_cast<IAudioStreamSink&>(*sink_owner);
    bool refused = false;
    try { sink.prepare({48000, 1}, 1, context); }
    catch (const std::runtime_error&) { refused = true; }
    require(refused && bytes(existing) == before, "Stream sink overwrote an existing file");
    GraphDefinition graph{{{"in", "wav_stream_input", {{"path", existing}}},
                           {"out", "wav_stream_output", {{"path", existing}}}}, {}};
    requires_error("output_exists", [&] { validate_prototype_file_targets(graph); });
}
} // namespace

int main() {
    try {
        TestDirectory directory;
        test_source_gain_sink(directory.path);
        test_gain_parameters_and_sink_clipping(directory.path);
        test_empty_stream_and_cancellation(directory.path);
        test_invalid_blocks_and_protection(directory.path);
        std::cout << "Streaming audio node tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
