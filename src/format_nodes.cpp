#include "audioprocess/format_nodes.h"

#include "audioprocess/execution_error.h"

#include <miniaudio/miniaudio.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <limits>
#include <memory>
#include <utility>

namespace audioprocess {
namespace {

constexpr std::uint32_t kMinimumRate = 8000;
constexpr std::uint32_t kMaximumRate = 192000;
constexpr std::size_t kChunkFrames = 4096;

void check_cancelled(const ExecutionContext& context) {
    if (context.cancelled()) {
        throw ExecutionError("cancelled", "Audio format conversion was cancelled");
    }
}

AudioClipPtr input_audio(const InputValues& inputs, const ExecutionContext& context) {
    check_cancelled(context);
    const auto found = inputs.find("audio");
    if (found == inputs.end()) {
        throw ExecutionError("missing_input", "Format conversion requires the audio input", {}, "audio");
    }
    const auto* audio = std::get_if<AudioClipPtr>(&found->second);
    if (audio == nullptr || !*audio) {
        throw ExecutionError("invalid_input", "Format conversion requires a non-null Audio value", {}, "audio");
    }
    const auto& format = (*audio)->format;
    if (format.sample_rate < kMinimumRate || format.sample_rate > kMaximumRate ||
        (format.channel_count != 1 && format.channel_count != 2)) {
        throw ExecutionError("unsupported_audio_format",
            "Format conversion supports 8000..192000 Hz mono or stereo audio only", {}, "audio");
    }
    if ((*audio)->samples.size() % format.channel_count != 0) {
        throw ExecutionError("invalid_input", "Audio must contain complete interleaved frames", {}, "audio");
    }
    for (std::size_t index = 0; index < (*audio)->samples.size(); ++index) {
        if (index % kChunkFrames == 0) { check_cancelled(context); }
        if (!std::isfinite((*audio)->samples[index])) {
            throw ExecutionError("invalid_audio_sample", "Format conversion requires finite audio samples", {}, "audio");
        }
    }
    check_cancelled(context);
    return *audio;
}

std::uint32_t target_rate(const ParameterMap& parameters) {
    const auto found = parameters.find("sample_rate");
    const auto* rate = found == parameters.end() ? nullptr : std::get_if<double>(&found->second);
    if (rate == nullptr || !std::isfinite(*rate) || *rate < kMinimumRate ||
        *rate > kMaximumRate || std::floor(*rate) != *rate) {
        throw ExecutionError("invalid_parameter", "sample_rate must be an integer from 8000 to 192000",
            {}, {}, "sample_rate");
    }
    return static_cast<std::uint32_t>(*rate);
}

// 分拆商和余数计算 ceil(N * out / in)，避免直接乘 N 导致整数溢出。
std::size_t output_frames(std::uint64_t frames, std::uint32_t input_rate,
                          std::uint32_t output_rate, std::uint16_t channels) {
    const auto maximum = std::vector<float>{}.max_size() / channels;
    const auto whole = frames / input_rate;
    const auto fraction = (frames % input_rate * output_rate + input_rate - 1U) / input_rate;
    if (whole > maximum / output_rate || fraction > maximum - whole * output_rate) {
        throw ExecutionError("audio_too_large", "Resampled audio exceeds the buffer size limit");
    }
    return static_cast<std::size_t>(whole * output_rate + fraction);
}

class Resampler final {
public:
    explicit Resampler(const AudioFormat& input_format, std::uint32_t output_rate) {
        auto config = ma_resampler_config_init(ma_format_f32, input_format.channel_count,
            input_format.sample_rate, output_rate, ma_resample_algorithm_linear);
        // 固定四阶低通：降采样前抗混叠、升采样后抑制镜像。非高保真 sinc 重采样。
        config.linear.lpfOrder = 4;
        if (ma_resampler_init(&config, nullptr, &state_) != MA_SUCCESS) {
            throw ExecutionError("resample_initialization_failed", "Could not initialize the audio resampler");
        }
    }
    ~Resampler() { ma_resampler_uninit(&state_, nullptr); }
    Resampler(const Resampler&) = delete;
    Resampler& operator=(const Resampler&) = delete;
    ma_resampler& state() noexcept { return state_; }

private:
    ma_resampler state_{};
};

}  // namespace

AudioResampleNode::AudioResampleNode(const ParameterMap& parameters) : sample_rate_(target_rate(parameters)) {}

const NodeDescriptor& AudioResampleNode::descriptor() const noexcept { return descriptor_; }

NodeDescriptor AudioResampleNode::make_descriptor() {
    return {"audio_resample", "Audio Resample",
        "Offline mono/stereo resampling from 8000..192000 Hz using miniaudio linear interpolation "
        "with a fourth-order low-pass filter (not high-fidelity sinc). Preserves channels; outputs "
        "ceil(input_frames * sample_rate / input_rate) frames. Drops library-reported whole-frame "
        "startup latency (minimum one output frame) and zero-pads the end; the one-frame floor can "
        "advance timing by one target sample and IIR phase/group delay is not fully compensated. "
        "Short clips and boundaries may be attenuated. Same-rate input is unchanged. Finite float "
        "samples are required; no normalization or clipping, and filter overshoot may exceed [-1,1]. "
        "For RNNoise use 48000 Hz and audio_downmix_mono; lower gain first if samples exceed [-1,1].",
        ExecutionDomain::Synchronous,
        {{"audio", DataType::Audio, true}}, {{"audio", DataType::Audio, true}},
        {{"sample_rate", ParameterType::Number, "Target sample rate, integer Hz.", true,
            {}, static_cast<double>(kMinimumRate), static_cast<double>(kMaximumRate), "Hz", {}, true}}};
}

OutputValues AudioResampleNode::execute(const InputValues& inputs, ExecutionContext& context) {
    const auto input = input_audio(inputs, context);
    if (input->format.sample_rate == sample_rate_) { return {{"audio", input}}; }
    const auto channels = input->format.channel_count;
    const auto frame_count = input->frame_count();
    const auto target_frames = output_frames(frame_count, input->format.sample_rate, sample_rate_, channels);
    auto output = std::make_shared<AudioClip>();
    output->format = {sample_rate_, channels};
    output->samples.reserve(target_frames * channels);
    if (target_frames == 0) { return {{"audio", AudioClipPtr{std::move(output)}}}; }

    Resampler resampler(input->format, sample_rate_);
    auto& state = resampler.state();
    // 高倍率降采样时库的整帧延迟会向下取整成 0，但第一个插值仍是启动零。
    // 至少丢一帧，避免极短输入被这个启动零完全取代；代价是最多提前一个目标采样。
    auto discard = std::max<ma_uint64>(1, ma_resampler_get_output_latency(&state));
    std::uint64_t consumed{};
    std::size_t written{};
    std::array<float, kChunkFrames * 2> converted{};
    while (written < target_frames) {
        check_cancelled(context);
        auto in_frames = static_cast<ma_uint64>(std::min<std::uint64_t>(kChunkFrames, frame_count - consumed));
        // 输入耗尽后使用有界零帧排空，不延长输出文件。miniaudio 没有单独的 EOS 标记。
        const float* source = in_frames == 0 ? nullptr : input->samples.data() + consumed * channels;
        if (source == nullptr) { in_frames = kChunkFrames; }
        const auto requested_input = in_frames;
        const auto wanted = discard > kChunkFrames ? kChunkFrames :
            std::min<std::uint64_t>(kChunkFrames, discard + target_frames - written);
        auto out_frames = static_cast<ma_uint64>(wanted);
        if (ma_resampler_process_pcm_frames(&state, source, &in_frames, converted.data(), &out_frames) != MA_SUCCESS ||
            in_frames > requested_input || out_frames > wanted || (in_frames == 0 && out_frames == 0)) {
            throw ExecutionError("resample_failed", "Audio resampler failed to make valid progress");
        }
        if (source != nullptr) { consumed += in_frames; }
        check_cancelled(context);
        const auto skipped = std::min(discard, out_frames);
        discard -= skipped;
        for (ma_uint64 frame = skipped; frame < out_frames; ++frame) {
            for (std::uint16_t channel = 0; channel < channels; ++channel) {
                const auto sample = converted[static_cast<std::size_t>(frame) * channels + channel];
                if (!std::isfinite(sample)) {
                    throw ExecutionError("invalid_output", "Resampling produced non-finite audio", {}, "audio");
                }
                output->samples.push_back(sample);
            }
        }
        written += static_cast<std::size_t>(out_frames - skipped);
    }
    check_cancelled(context);
    return {{"audio", AudioClipPtr{std::move(output)}}};
}

const NodeDescriptor& AudioDownmixMonoNode::descriptor() const noexcept { return descriptor_; }

NodeDescriptor AudioDownmixMonoNode::make_descriptor() {
    return {"audio_downmix_mono", "Audio Downmix to Mono",
        "Offline mono/stereo to mono conversion for 8000..192000 Hz finite float Audio. "
        "Stereo uses (left + right) / 2 without normalization or clipping; opposite-phase content "
        "can cancel. Mono input is unchanged. Preserves sample rate and frame count. "
        "More than two channels are rejected because no channel layout is declared.",
        ExecutionDomain::Synchronous,
        {{"audio", DataType::Audio, true}}, {{"audio", DataType::Audio, true}}, {}};
}

OutputValues AudioDownmixMonoNode::execute(const InputValues& inputs, ExecutionContext& context) {
    const auto input = input_audio(inputs, context);
    if (input->format.channel_count == 1) { return {{"audio", input}}; }
    auto output = std::make_shared<AudioClip>();
    output->format = {input->format.sample_rate, 1};
    output->samples.reserve(input->samples.size() / 2);
    for (std::size_t frame = 0; frame < input->samples.size() / 2; ++frame) {
        if (frame % kChunkFrames == 0) { check_cancelled(context); }
        // double 中间值避免有限大幅值相加溢出；没有偷偷削波到 PCM16 范围。
        output->samples.push_back(static_cast<float>((static_cast<double>(input->samples[frame * 2]) +
            static_cast<double>(input->samples[frame * 2 + 1])) * 0.5));
    }
    check_cancelled(context);
    return {{"audio", AudioClipPtr{std::move(output)}}};
}

void register_format_node_types(NodeRegistry& registry) {
    registry.register_type(AudioResampleNode::make_descriptor(), [](const ParameterMap& parameters) {
        return std::make_unique<AudioResampleNode>(parameters);
    });
    registry.register_type(AudioDownmixMonoNode::make_descriptor(), [](const ParameterMap&) {
        return std::make_unique<AudioDownmixMonoNode>();
    });
}

}  // namespace audioprocess
