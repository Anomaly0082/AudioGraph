#include "audioprocess/execution_error.h"
#include "audioprocess/prototype_nodes.h"

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <iostream>
#include <limits>
#include <memory>
#include <numeric>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
using namespace audioprocess;
constexpr double pi = 3.14159265358979323846;

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

AudioClipPtr clip(AudioFormat format, std::vector<float> samples) {
    auto value = std::make_shared<AudioClip>();
    value->format = format;
    value->samples = std::move(samples);
    return value;
}

AudioClipPtr run(ISyncNode& node, const AudioClipPtr& input, ExecutionContext& context) {
    const auto values = node.execute({{"audio", input}}, context);
    require(values.size() == 1 && values.contains("audio"), "format node returned unexpected outputs");
    const auto output = std::get<AudioClipPtr>(values.at("audio"));
    require(output != nullptr, "format node returned null audio");
    return output;
}

std::vector<float> tone(std::uint32_t rate, std::size_t frames, double frequency,
                        std::uint16_t channels = 1) {
    std::vector<float> samples(frames * channels);
    for (std::size_t frame = 0; frame < frames; ++frame) {
        const auto value = static_cast<float>(0.5 * std::sin(2.0 * pi * frequency * frame / rate));
        for (std::uint16_t channel = 0; channel < channels; ++channel) {
            samples[frame * channels + channel] = value;
        }
    }
    return samples;
}

double rms_middle(const std::vector<float>& samples) {
    if (samples.size() < 8) return 0.0;
    const auto margin = samples.size() / 8;
    const auto begin = samples.begin() + static_cast<std::ptrdiff_t>(margin);
    const auto end = samples.end() - static_cast<std::ptrdiff_t>(margin);
    const auto energy = std::accumulate(begin, end, 0.0,
        [](double sum, float sample) { return sum + static_cast<double>(sample) * sample; });
    return std::sqrt(energy / static_cast<double>(std::distance(begin, end)));
}

void test_descriptors_and_parameters() {
    const auto registry = create_prototype_node_registry();
    const auto& downmix = registry.descriptor("audio_downmix_mono");
    require(downmix.execution_domain == ExecutionDomain::Synchronous && downmix.parameters.empty(),
            "downmix descriptor contract mismatch");
    const auto& resample = registry.descriptor("audio_resample");
    require(resample.execution_domain == ExecutionDomain::Synchronous && resample.parameters.size() == 1 &&
            resample.parameters[0].id == "sample_rate" && resample.parameters[0].minimum == 8000.0 &&
            resample.parameters[0].maximum == 192000.0 && resample.parameters[0].integer_only,
            "resample descriptor contract mismatch");
    for (const double rate : {7999.0, 192001.0, 48000.5}) {
        rejects([&] { static_cast<void>(registry.create("audio_resample", {{"sample_rate", rate}})); },
                "resample accepted an out-of-contract rate");
    }
    rejects([&] { static_cast<void>(registry.create("audio_resample", {})); },
            "resample accepted a missing rate");
    rejects([&] { static_cast<void>(registry.create("audio_downmix_mono", {{"gain", 1.0}})); },
            "downmix accepted an undocumented parameter");
}

void test_downmix_exactness_and_sharing() {
    const auto registry = create_prototype_node_registry();
    ExecutionContext context;
    auto node = registry.create("audio_downmix_mono", {});
    const auto mono = clip({44100, 1}, {0.25F, -0.5F});
    require(run(*node, mono, context) == mono, "mono downmix did not share immutable input");
    const auto stereo = clip({44100, 2}, {1.0F, -1.0F, 0.75F, 0.25F, -0.5F, -0.25F, 2.0F, 2.0F});
    const auto mixed = run(*node, stereo, context);
    require(mixed != stereo && mixed->format == AudioFormat{44100, 1}, "stereo downmix format mismatch");
    require(mixed->samples == std::vector<float>({0.0F, 0.5F, -0.375F, 2.0F}),
            "stereo average mismatch or undocumented clipping occurred");
    require(stereo->samples == std::vector<float>({1.0F, -1.0F, 0.75F, 0.25F, -0.5F, -0.25F, 2.0F, 2.0F}),
            "downmix modified shared input");
}

void test_resample_frame_policy_boundaries_and_finiteness() {
    const auto registry = create_prototype_node_registry();
    ExecutionContext context;
    for (const auto frames : {0U, 1U, 2U, 146U, 147U, 148U, 440U, 441U, 442U, 1001U}) {
        auto node = registry.create("audio_resample", {{"sample_rate", 48000.0}});
        const auto input = clip({44100, 2}, tone(44100, frames, 997.0, 2));
        const auto output = run(*node, input, context);
        const auto expected = (static_cast<std::uint64_t>(frames) * 48000U + 44100U - 1U) / 44100U;
        require(output->format == AudioFormat{48000, 2}, "resample changed channel count");
        require(output->samples.size() == expected * 2, "resample ceil frame policy mismatch");
        require(std::all_of(output->samples.begin(), output->samples.end(),
            [](float sample) { return std::isfinite(sample); }), "resample produced a non-finite sample");
    }
    auto identity = registry.create("audio_resample", {{"sample_rate", 44100.0}});
    const auto input = clip({44100, 1}, tone(44100, 64, 1000.0));
    require(run(*identity, input, context) == input, "same-rate resample did not share immutable input");
}

void test_alias_suppression_and_state_isolation() {
    const auto registry = create_prototype_node_registry();
    ExecutionContext context;
    const auto low = clip({48000, 1}, tone(48000, 48000, 1000.0));
    const auto high = clip({48000, 1}, tone(48000, 48000, 12000.0));
    auto low_node = registry.create("audio_resample", {{"sample_rate", 16000.0}});
    auto high_node = registry.create("audio_resample", {{"sample_rate", 16000.0}});
    const auto low_rms = rms_middle(run(*low_node, low, context)->samples);
    const auto high_rms = rms_middle(run(*high_node, high, context)->samples);
    require(low_rms > 0.2, "downsample removed pass-band tone");
    require(high_rms < low_rms * 0.35, "downsample did not sufficiently suppress an alias-band tone");

    auto reused = registry.create("audio_resample", {{"sample_rate", 48000.0}});
    static_cast<void>(run(*reused, clip({44100, 1}, tone(44100, 731, 321.0)), context));
    const auto probe = clip({44100, 1}, tone(44100, 997, 713.0));
    const auto after_warmup = run(*reused, probe, context);
    const auto repeated = run(*reused, probe, context);
    auto fresh = registry.create("audio_resample", {{"sample_rate", 48000.0}});
    const auto baseline = run(*fresh, probe, context);
    require(after_warmup->samples == baseline->samples && repeated->samples == baseline->samples,
            "resampler state leaked across execute calls");
}

void test_tail_flush_and_stereo_channel_isolation() {
    const auto registry = create_prototype_node_registry();
    ExecutionContext context;
    auto tail_node = registry.create("audio_resample", {{"sample_rate", 48000.0}});
    std::vector<float> impulse(441, 0.0F);
    impulse.back() = 0.5F;
    const auto tail = run(*tail_node, clip({44100, 1}, impulse), context);
    require(tail->samples.size() == 480, "tail impulse resample length mismatch");
    const auto tail_begin = tail->samples.begin() + 360;
    const auto tail_peak = std::accumulate(tail_begin, tail->samples.end(), 0.0F,
        [](float peak, float sample) { return std::max(peak, std::abs(sample)); });
    require(tail_peak > 1e-4F, "resampler dropped the final input impulse instead of flushing its tail");

    auto extreme_node = registry.create("audio_resample", {{"sample_rate", 8000.0}});
    const auto extreme = run(*extreme_node, clip({192000, 1}, {0.5F}), context);
    require(extreme->samples.size() == 1 && std::isfinite(extreme->samples[0]) &&
            std::abs(extreme->samples[0]) > 1e-8F,
            "192 kHz to 8 kHz single-frame input was replaced by a startup zero");

    std::vector<float> stereo(4410 * 2, 0.0F);
    for (std::size_t frame = 0; frame < 4410; ++frame) {
        stereo[frame * 2] = static_cast<float>(0.5 * std::sin(2.0 * pi * 997.0 * frame / 44100.0));
    }
    auto stereo_node = registry.create("audio_resample", {{"sample_rate", 48000.0}});
    const auto converted = run(*stereo_node, clip({44100, 2}, stereo), context);
    double left_energy = 0.0, right_energy = 0.0;
    for (std::size_t frame = 0; frame < converted->frame_count(); ++frame) {
        left_energy += static_cast<double>(converted->samples[frame * 2]) * converted->samples[frame * 2];
        right_energy += static_cast<double>(converted->samples[frame * 2 + 1]) * converted->samples[frame * 2 + 1];
    }
    require(left_energy > 1.0 && right_energy < 1e-12, "stereo resampling leaked one channel into the other");
}

void test_invalid_inputs_and_cancellation() {
    const auto registry = create_prototype_node_registry();
    ExecutionContext context;
    for (const auto type : {std::string("audio_downmix_mono"), std::string("audio_resample")}) {
        const ParameterMap parameters = type == "audio_resample" ? ParameterMap{{"sample_rate", 48000.0}} : ParameterMap{};
        auto node = registry.create(type, parameters);
        rejects([&] { static_cast<void>(node->execute({}, context)); }, "node accepted missing input");
        rejects([&] { static_cast<void>(node->execute({{"audio", 1.0}}, context)); }, "node accepted a non-Audio input");
        rejects([&] { static_cast<void>(node->execute({{"audio", AudioClipPtr{}}}, context)); }, "node accepted null input");
        rejects([&] { static_cast<void>(run(*node, clip({7999, 1}, {0.0F}), context)); }, "node accepted unsupported rate");
        rejects([&] { static_cast<void>(run(*node, clip({48000, 3}, {0.0F, 0.0F, 0.0F}), context)); }, "node accepted 3 channels");
        rejects([&] { static_cast<void>(run(*node, clip({48000, 2}, {0.0F}), context)); }, "node accepted partial frame");
        for (const auto sample : {std::numeric_limits<float>::quiet_NaN(), std::numeric_limits<float>::infinity()}) {
            rejects([&] { static_cast<void>(run(*node, clip({48000, 1}, {sample}), context)); },
                    "node accepted non-finite input");
        }
        std::atomic_bool cancelled{true};
        ExecutionContext cancelled_context{&cancelled};
        bool reported = false;
        try { static_cast<void>(run(*node, clip({48000, 1}, tone(48000, 9000, 1000.0)), cancelled_context)); }
        catch (const ExecutionError& error) { reported = error.code == "cancelled"; }
        require(reported, "node did not report cooperative cancellation");
    }
}
}

int main() {
    try {
        test_descriptors_and_parameters();
        test_downmix_exactness_and_sharing();
        test_resample_frame_policy_boundaries_and_finiteness();
        test_alias_suppression_and_state_isolation();
        test_tail_flush_and_stereo_channel_isolation();
        test_invalid_inputs_and_cancellation();
        std::cout << "Format conversion node tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Format conversion node test failure: " << error.what() << '\n';
        return 1;
    }
}
