#include "audioprocess/audio_buffer.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/sync_graph_executor.h"
#include "audioprocess/wav_file.h"

#include <rnnoise.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <filesystem>
#include <iostream>
#include <limits>
#include <memory>
#include <numeric>
#include <span>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {
using namespace audioprocess;

constexpr std::size_t kRnnoiseFrame = 480;
constexpr std::size_t kRnnoiseDelay = 2 * kRnnoiseFrame;

void require(bool condition, const std::string& message) {
    if (!condition) throw std::runtime_error(message);
}

template<class Function>
void rejects(Function action, const std::string& message) {
    bool rejected = false;
    try { action(); }
    catch (const std::exception&) { rejected = true; }
    require(rejected, message);
}

class TemporaryDirectory {
public:
    TemporaryDirectory() {
        const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
        for (unsigned attempt = 0; attempt < 100; ++attempt) {
            auto candidate = std::filesystem::temp_directory_path() /
                ("audioprocess_denoise_" + std::to_string(stamp) + "_" +
                 std::to_string(attempt));
            if (std::filesystem::create_directory(candidate)) {
                path = std::move(candidate);
                return;
            }
        }
        throw std::runtime_error("Unable to create denoise test directory");
    }
    ~TemporaryDirectory() {
        std::error_code ignored;
        if (!path.empty()) std::filesystem::remove_all(path, ignored);
    }
    TemporaryDirectory(const TemporaryDirectory&) = delete;
    TemporaryDirectory& operator=(const TemporaryDirectory&) = delete;
    std::filesystem::path path;
};

AudioClipPtr clip(std::vector<float> samples, AudioFormat format = {48'000, 1}) {
    auto value = std::make_shared<AudioClip>();
    value->format = format;
    value->samples = std::move(samples);
    return value;
}

std::vector<float> signal(std::size_t frames) {
    std::vector<float> result(frames);
    std::uint32_t noise = 0x6d2b79f5U;
    for (std::size_t i = 0; i < frames; ++i) {
        noise = noise * 1664525U + 1013904223U;
        const auto unit = static_cast<float>((noise >> 8U) * (1.0 / 16777215.0));
        const auto t = static_cast<double>(i) / 48'000.0;
        result[i] = static_cast<float>(
            0.22 * std::sin(2.0 * 3.141592653589793 * 233.0 * t) +
            0.11 * std::sin(2.0 * 3.141592653589793 * 701.0 * t) +
            0.06 * (2.0 * unit - 1.0));
    }
    return result;
}

// 独立 oracle 只使用 RNNoise 的公开 C API。v0.2 的 delayed_X 加 50% OLA
// 共引入 960 个采样；补两个静音帧排空后丢掉前 960 个输出采样。
std::vector<float> native_reference(const std::vector<float>& input) {
    require(rnnoise_get_frame_size() == static_cast<int>(kRnnoiseFrame),
            "Unexpected RNNoise frame size");
    if (input.empty()) return {};

    std::unique_ptr<DenoiseState, decltype(&rnnoise_destroy)> state(
        rnnoise_create(nullptr), &rnnoise_destroy);
    require(state != nullptr, "RNNoise failed to create a reference state");

    const auto input_frames = (input.size() + kRnnoiseFrame - 1) / kRnnoiseFrame;
    std::vector<float> processed((input_frames + 2) * kRnnoiseFrame);
    std::vector<float> in(kRnnoiseFrame);
    std::vector<float> out(kRnnoiseFrame);
    for (std::size_t frame = 0; frame < input_frames + 2; ++frame) {
        std::fill(in.begin(), in.end(), 0.0F);
        if (frame < input_frames) {
            const auto offset = frame * kRnnoiseFrame;
            const auto count = std::min(kRnnoiseFrame, input.size() - offset);
            for (std::size_t i = 0; i < count; ++i) in[i] = input[offset + i] * 32768.0F;
        }
        static_cast<void>(rnnoise_process_frame(state.get(), out.data(), in.data()));
        std::copy(out.begin(), out.end(), processed.begin() + frame * kRnnoiseFrame);
    }

    std::vector<float> result(input.size());
    std::transform(processed.begin() + static_cast<std::ptrdiff_t>(kRnnoiseDelay),
                   processed.begin() + static_cast<std::ptrdiff_t>(kRnnoiseDelay + input.size()),
                   result.begin(), [](float sample) { return sample / 32768.0F; });
    return result;
}

std::vector<float> run(ISyncNode& node, const AudioClipPtr& input) {
    ExecutionContext context;
    const auto output = node.execute({{"audio", input}}, context);
    require(output.size() == 1 && output.contains("audio"),
            "Denoise node returned an unexpected output set");
    const auto result = std::get<AudioClipPtr>(output.at("audio"));
    require(result != nullptr, "Denoise node returned null audio");
    require(result->format == input->format, "Denoise node changed the audio format");
    return result->samples;
}

void require_close(const std::vector<float>& actual, const std::vector<float>& expected,
                   const std::string& message, float tolerance = 2e-6F) {
    require(actual.size() == expected.size(), message + ": frame count differs");
    for (std::size_t i = 0; i < actual.size(); ++i) {
        if (!std::isfinite(actual[i]) || std::abs(actual[i] - expected[i]) > tolerance) {
            throw std::runtime_error(message + " at sample " + std::to_string(i) +
                                     ": actual=" + std::to_string(actual[i]) +
                                     ", expected=" + std::to_string(expected[i]));
        }
    }
}

void test_descriptor_and_registration() {
    const auto registry = create_prototype_node_registry();
    const auto& descriptor = registry.descriptor("rnnoise_denoise");
    require(descriptor.execution_domain == ExecutionDomain::Synchronous,
            "RNNoise denoise is not an offline synchronous node");
    require(descriptor.parameters.empty(), "RNNoise denoise unexpectedly exposes parameters");
    require(descriptor.inputs.size() == 1 && descriptor.inputs[0].id == "audio" &&
            descriptor.inputs[0].type == DataType::Audio && descriptor.inputs[0].required,
            "RNNoise denoise input contract differs from Audio audio");
    require(descriptor.outputs.size() == 1 && descriptor.outputs[0].id == "audio" &&
            descriptor.outputs[0].type == DataType::Audio && descriptor.outputs[0].required,
            "RNNoise denoise output contract differs from Audio audio");
    rejects([&] { static_cast<void>(registry.create("rnnoise_denoise", {{"strength", 0.5}})); },
            "RNNoise denoise accepted an undocumented parameter");
}

void test_native_alignment_and_boundaries() {
    const auto registry = create_prototype_node_registry();
    for (const std::size_t frames : {0U, 1U, 479U, 480U, 481U, 959U, 960U, 961U, 1441U}) {
        const auto input_samples = signal(frames);
        const auto input = clip(input_samples);
        auto node = registry.create("rnnoise_denoise", {});
        const auto output = run(*node, input);
        require_close(output, native_reference(input_samples),
                      "Node diverged from native RNNoise for " + std::to_string(frames) + " frames");
        require(input->samples == input_samples, "Denoise node modified its shared input");
    }
}

void test_execute_state_isolation() {
    const auto registry = create_prototype_node_registry();
    auto reused = registry.create("rnnoise_denoise", {});
    const auto warmup = clip(signal(3 * kRnnoiseFrame + 37));
    static_cast<void>(run(*reused, warmup));

    auto second_samples = signal(kRnnoiseFrame + 19);
    std::reverse(second_samples.begin(), second_samples.end());
    const auto second = clip(second_samples);
    const auto after_warmup = run(*reused, second);
    const auto repeated = run(*reused, second);
    auto fresh = registry.create("rnnoise_denoise", {});
    const auto fresh_result = run(*fresh, second);
    const auto expected = native_reference(second_samples);
    require_close(after_warmup, expected, "A prior execute leaked RNNoise state");
    require_close(repeated, expected, "Repeated execute leaked RNNoise state");
    require_close(fresh_result, expected, "Fresh node differs from native RNNoise");
}

void test_rejections_and_cancellation() {
    const auto registry = create_prototype_node_registry();
    auto node = registry.create("rnnoise_denoise", {});
    ExecutionContext context;
    rejects([&] { static_cast<void>(node->execute({{"audio", clip(signal(480), {44'100, 1})}}, context)); },
            "RNNoise denoise accepted 44.1 kHz audio");
    rejects([&] { static_cast<void>(node->execute({{"audio", clip(signal(960), {48'000, 2})}}, context)); },
            "RNNoise denoise accepted stereo audio");
    rejects([&] { static_cast<void>(node->execute({{"audio", clip({0.0F}, {0, 1})}}, context)); },
            "RNNoise denoise accepted an invalid format");
    rejects([&] { static_cast<void>(node->execute({{"audio", AudioClipPtr{}}}, context)); },
            "RNNoise denoise accepted null audio");
    for (const auto invalid : {std::numeric_limits<float>::quiet_NaN(),
                               std::numeric_limits<float>::infinity(), 1.0001F, -1.0001F}) {
        rejects([&] { static_cast<void>(node->execute({{"audio", clip({invalid})}}, context)); },
                "RNNoise denoise accepted a non-finite or out-of-range sample");
    }

    std::atomic_bool cancelled{true};
    ExecutionContext cancelled_context{&cancelled};
    bool reported = false;
    try { static_cast<void>(node->execute({{"audio", clip(signal(2 * kRnnoiseFrame))}}, cancelled_context)); }
    catch (const ExecutionError& error) { reported = error.code == "cancelled"; }
    require(reported, "RNNoise denoise did not report cooperative cancellation");
}

double rms(const std::vector<float>& samples) {
    const auto energy = std::accumulate(samples.begin(), samples.end(), 0.0,
        [](double sum, float sample) { return sum + static_cast<double>(sample) * sample; });
    return samples.empty() ? 0.0 : std::sqrt(energy / static_cast<double>(samples.size()));
}

void test_synthetic_noise_fixture() {
    std::vector<float> noise(100 * kRnnoiseFrame);
    std::uint32_t state = 0x12345678U;
    for (auto& sample : noise) {
        state = state * 1664525U + 1013904223U;
        sample = 0.12F * (2.0F * static_cast<float>((state >> 8U) * (1.0 / 16777215.0)) - 1.0F);
    }
    const auto registry = create_prototype_node_registry();
    auto node = registry.create("rnnoise_denoise", {});
    const auto output = run(*node, clip(noise));
    const auto input_rms = rms(noise);
    const auto output_rms = rms(output);
    require(output_rms < input_rms, "RNNoise did not reduce RMS for the synthetic noise fixture");
    std::cout << "Synthetic-noise fixture RMS: " << input_rms << " -> " << output_rms
              << " (fixture observation only; not a human-speech quality claim).\n";
}

std::shared_ptr<AudioClip> read_wav(const std::filesystem::path& path) {
    WavFileSource source(path, 257);
    AudioBuffer buffer(source.format(), 257);
    auto result = std::make_shared<AudioClip>();
    result->format = source.format();
    while (const auto block = source.read(buffer)) {
        result->samples.insert(result->samples.end(), block->samples.begin(), block->samples.end());
    }
    return result;
}

float pcm16_roundtrip(float sample) {
    const auto clamped = std::clamp(sample, -1.0F, 1.0F);
    if (clamped <= -1.0F) return -1.0F;
    if (clamped >= 1.0F) return 32767.0F / 32768.0F;
    const auto encoded = std::clamp(std::lrint(clamped * 32768.0F), -32768L, 32767L);
    return static_cast<float>(encoded) / 32768.0F;
}

void test_wav_graph_integration(const std::filesystem::path& directory) {
    const auto input_path = directory / "noisy_input.wav";
    const auto output_path = directory / "denoised_output.wav";
    const auto source_samples = signal(3 * kRnnoiseFrame + 17);
    {
        WavFileSink sink(input_path, {48'000, 1}, 233);
        for (std::size_t offset = 0; offset < source_samples.size(); offset += 233) {
            const auto count = static_cast<std::uint32_t>(
                std::min<std::size_t>(233, source_samples.size() - offset));
            sink.write(std::span<const float>{source_samples}.subspan(offset, count), count);
        }
        sink.finalize();
    }
    const auto decoded_input = read_wav(input_path);

    const auto registry = create_prototype_node_registry();
    GraphDefinition graph{
        {{"input", "wav_input", {{"path", input_path}}},
         {"denoise", "rnnoise_denoise", {}},
         {"output", "wav_output", {{"path", output_path}}}},
        {{"input", "audio", "denoise", "audio"},
         {"denoise", "audio", "output", "audio"}},
        {{"audio", "denoise", "audio"}, {"path", "output", "path"}}};
    validate_prototype_file_targets(graph);
    const auto result = SyncGraphExecutor::compile(graph, registry).execute();
    require(std::get<std::filesystem::path>(result.value("output", "path")) == output_path,
            "WAV graph returned the wrong output path");
    const auto graph_audio = std::get<AudioClipPtr>(result.value("denoise", "audio"));
    const auto expected = native_reference(decoded_input->samples);
    require_close(graph_audio->samples, expected, "WAV graph differs from native RNNoise");

    const auto written = read_wav(output_path);
    require(written->format == AudioFormat{48'000, 1} &&
            written->samples.size() == decoded_input->samples.size(),
            "Denoised WAV changed format or frame count");
    std::vector<float> encoded_expected(expected.size());
    std::transform(expected.begin(), expected.end(), encoded_expected.begin(), pcm16_roundtrip);
    require_close(written->samples, encoded_expected, "Denoised WAV contains unexpected PCM16 samples", 0.0F);
}

}  // namespace

int main() {
    try {
        TemporaryDirectory temporary;
        test_descriptor_and_registration();
        test_native_alignment_and_boundaries();
        test_execute_state_isolation();
        test_rejections_and_cancellation();
        test_synthetic_noise_fixture();
        test_wav_graph_integration(temporary.path);
        std::cout << "RNNoise denoise node tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "RNNoise denoise test failure: " << error.what() << '\n';
        return 1;
    }
}
