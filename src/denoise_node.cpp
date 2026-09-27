#include "audioprocess/denoise_node.h"

#include "audioprocess/execution_error.h"

#include <rnnoise.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdlib>
#include <memory>
#include <utility>

namespace audioprocess {
namespace {

constexpr std::size_t kFrameSamples = 480;
// 固定的 RNNoise v0.2：20 ms 窗口 overlap-add 延迟一帧，delayed_X 又延迟一帧。
// 升级第三方库/模型时必须重新核对，不能把本常量视为通用 RNNoise API 保证。
constexpr std::size_t kDelayFrames = 2;
constexpr float kPcmScale = 32768.0F;

void check_cancelled(const ExecutionContext& context) {
    if (context.cancelled()) {
        throw ExecutionError("cancelled", "RNNoise denoising was cancelled");
    }
}

const AudioClip& input_audio(const InputValues& inputs) {
    const auto found = inputs.find("audio");
    if (found == inputs.end()) {
        throw ExecutionError("missing_input", "RNNoise requires the audio input", {}, "audio");
    }
    const auto* audio = std::get_if<AudioClipPtr>(&found->second);
    if (!audio || !*audio) {
        throw ExecutionError("invalid_input", "RNNoise requires a non-null Audio value", {}, "audio");
    }
    if ((*audio)->format != AudioFormat{48000, 1}) {
        throw ExecutionError("unsupported_audio_format",
            "RNNoise requires 48000 Hz mono audio; resampling and downmixing are not automatic",
            {}, "audio");
    }
    return **audio;
}

struct StateDeleter {
    void operator()(DenoiseState* state) const noexcept { std::free(state); }
};

std::unique_ptr<DenoiseState, StateDeleter> create_state() {
    if (rnnoise_get_frame_size() != static_cast<int>(kFrameSamples)) {
        throw ExecutionError("denoise_initialization_failed", "Unexpected RNNoise frame size");
    }
    const auto bytes = rnnoise_get_size();
    if (bytes <= 0) {
        throw ExecutionError("denoise_initialization_failed", "Invalid RNNoise state size");
    }
    // v0.2 的 rnnoise_create 在 malloc 之后未判空；使用公开的预分配初始化接口。
    // 默认模型仅引用编译期权重，无额外模型对象或嵌套资源需要释放。
    std::unique_ptr<DenoiseState, StateDeleter> state{
        static_cast<DenoiseState*>(std::malloc(static_cast<std::size_t>(bytes)))};
    if (!state || rnnoise_init(state.get(), nullptr) != 0) {
        throw ExecutionError("denoise_initialization_failed", "Could not initialize RNNoise");
    }
    return state;
}

}  // namespace

const NodeDescriptor& RnnNoiseDenoiseNode::descriptor() const noexcept { return descriptor_; }

NodeDescriptor RnnNoiseDenoiseNode::make_descriptor() {
    return {"rnnoise_denoise", "RNNoise Speech Denoise",
        "Offline speech denoising with the bundled RNNoise v0.2 default model. Requires 48000 Hz "
        "mono Audio with finite samples in [-1,1]; lower gain first if necessary. No automatic "
        "resampling/downmixing and no strength parameter. Preserves frame count and compensates "
        "960 samples of algorithmic delay. Not a music denoiser or streaming/realtime node.",
        ExecutionDomain::Synchronous,
        {{"audio", DataType::Audio, true}},
        {{"audio", DataType::Audio, true}}, {}};
}

OutputValues RnnNoiseDenoiseNode::execute(const InputValues& inputs, ExecutionContext& context) {
    check_cancelled(context);
    const auto& input = input_audio(inputs);
    auto output = std::make_shared<AudioClip>();
    output->format = input.format;
    output->samples.resize(input.samples.size());
    if (input.samples.empty()) {
        return {{"audio", AudioClipPtr{std::move(output)}}};
    }

    // 状态属于本次 execute；重复执行同一节点也不延续上次的滤波/RNN 历史。
    auto state = create_state();
    std::array<float, kFrameSamples> frame_in{};
    std::array<float, kFrameSamples> frame_out{};
    const auto input_frames = input.samples.size() / kFrameSamples +
        (input.samples.size() % kFrameSamples != 0 ? 1U : 0U);

    for (std::size_t frame = 0; frame < input_frames + kDelayFrames; ++frame) {
        check_cancelled(context);
        frame_in.fill(0.0F);
        if (frame < input_frames) {
            const auto offset = frame * kFrameSamples;
            const auto count = std::min(kFrameSamples, input.samples.size() - offset);
            for (std::size_t index = 0; index < count; ++index) {
                const auto sample = input.samples[offset + index];
                if (!std::isfinite(sample) || sample < -1.0F || sample > 1.0F) {
                    throw ExecutionError("invalid_audio_sample",
                        "RNNoise requires finite samples in [-1,1]; lower gain before denoising",
                        {}, "audio");
                }
                // AudioClip 使用归一化 float；RNNoise 使用 float 承载 PCM16 量级。
                frame_in[index] = sample * kPcmScale;
            }
        }

        (void)rnnoise_process_frame(state.get(), frame_out.data(), frame_in.data());
        check_cancelled(context);
        if (!std::ranges::all_of(frame_out, [](float sample) { return std::isfinite(sample); })) {
            throw ExecutionError("invalid_output", "RNNoise produced non-finite audio", {}, "audio");
        }
        // 丢弃启动延迟。尾部不足一帧先补零，再送两个完整零帧排空延迟，最终裁至原长。
        if (frame >= kDelayFrames) {
            const auto offset = (frame - kDelayFrames) * kFrameSamples;
            const auto count = std::min(kFrameSamples, output->samples.size() - offset);
            for (std::size_t index = 0; index < count; ++index) {
                output->samples[offset + index] = frame_out[index] / kPcmScale;
            }
        }
    }
    check_cancelled(context);
    return {{"audio", AudioClipPtr{std::move(output)}}};
}

void register_denoise_node_type(NodeRegistry& registry) {
    registry.register_type(RnnNoiseDenoiseNode::make_descriptor(), [](const ParameterMap&) {
        return std::make_unique<RnnNoiseDenoiseNode>();
    });
}

}  // namespace audioprocess
