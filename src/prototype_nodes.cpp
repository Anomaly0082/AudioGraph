#include "audioprocess/prototype_nodes.h"

#include "audioprocess/audio_buffer.h"
#include "audioprocess/wav_file.h"
#include "audioprocess/detail/exclusive_file.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/streaming_nodes.h"

#include <algorithm>
#include <cmath>
#include <memory>
#include <stdexcept>
#include <utility>

namespace audioprocess {
namespace {

constexpr std::uint32_t kIoBlockFrames = 1024;

void check_cancelled(const ExecutionContext& context) {
    if (context.cancelled()) {
        throw ExecutionError("cancelled", "Node cancelled; previously written files may remain");
    }
}

const DataValue& required_input(const InputValues& inputs, const std::string& id) {
    const auto iterator = inputs.find(id);
    if (iterator == inputs.end()) {
        throw std::invalid_argument("Missing required node input: " + id);
    }
    return iterator->second;
}

std::filesystem::path path_parameter(
    const ParameterMap& parameters,
    const std::string& id) {
    const auto iterator = parameters.find(id);
    if (iterator == parameters.end() ||
        !std::holds_alternative<std::filesystem::path>(iterator->second)) {
        throw std::invalid_argument("Missing file path parameter: " + id);
    }
    auto path = std::get<std::filesystem::path>(iterator->second);
    if (path.empty() || path.native().find(std::filesystem::path::value_type{}) !=
                            std::filesystem::path::string_type::npos) {
        throw ExecutionError("invalid_path", "File path must be nonempty and contain no NUL characters", {}, {}, id);
    }
    return path;
}

double number_parameter(
    const ParameterMap& parameters,
    const std::string& id,
    double default_value) {
    const auto iterator = parameters.find(id);
    if (iterator == parameters.end()) {
        return default_value;
    }
    if (!std::holds_alternative<double>(iterator->second)) {
        throw std::invalid_argument("Parameter must be numeric: " + id);
    }
    return std::get<double>(iterator->second);
}

AudioClipPtr load_wav(const std::filesystem::path& path, ExecutionContext& context) {
    check_cancelled(context);
    WavFileSource source(path, kIoBlockFrames);
    AudioBuffer buffer(source.format(), kIoBlockFrames);
    auto clip = std::make_shared<AudioClip>();
    clip->format = source.format();
    clip->samples.reserve(
        static_cast<std::size_t>(source.total_frames()) * source.format().channel_count);

    while (auto block = source.read(buffer)) {
        check_cancelled(context);
        clip->samples.insert(clip->samples.end(), block->samples.begin(), block->samples.end());
    }
    return clip;
}

std::uint64_t write_wav(const std::filesystem::path& path, const AudioClip& clip, ExecutionContext& context) {
    check_cancelled(context);
    const auto total_frames = clip.frame_count();
    AudioBuffer buffer(clip.format, kIoBlockFrames);
    WavFileSink sink(path, clip.format, kIoBlockFrames);

    std::uint64_t frame_position{};
    std::uint64_t clipped_samples{};
    while (frame_position < total_frames) {
        check_cancelled(context);
        const auto frames = static_cast<std::uint32_t>(
            std::min<std::uint64_t>(kIoBlockFrames, total_frames - frame_position));
        auto block = buffer.block(frames, frame_position);
        const auto sample_offset = static_cast<std::size_t>(frame_position) *
            clip.format.channel_count;
        for (std::size_t sample_index = 0; sample_index < block.sample_count(); ++sample_index) {
            if (sample_index % 4096U == 0) { check_cancelled(context); }
            const auto sample = clip.samples[sample_offset + sample_index];
            block.samples[sample_index] = sample;
            // PCM16 的正满幅小于 1；统计编码前超出可表示范围的采样，而不是帧。
            if (sample < -1.0F || sample > 32767.0F / 32768.0F) { ++clipped_samples; }
        }
        sink.write(block);
        frame_position += frames;
    }
    sink.finalize();
    return clipped_samples;
}

// 文件节点将 PCM16 解码为只读整段音频；不包含实时设备逻辑。
class WavInputNode final : public ISyncNode {
public:
    explicit WavInputNode(const ParameterMap& parameters)
        : path_(path_parameter(parameters, "path")) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues&, ExecutionContext& context) override {
        return {{"audio", load_wav(path_, context)}};
    }

    static NodeDescriptor make_descriptor() {
        return NodeDescriptor{
            "wav_input",
            "WAV Input",
            "Loads a PCM16 WAV file into an Audio value.",
            ExecutionDomain::Synchronous,
            {},
            {{"audio", DataType::Audio, true}},
            {{"path", ParameterType::FilePath, "Input PCM16 WAV file.", true}},
        };
    }

private:
    std::filesystem::path path_;
    NodeDescriptor descriptor_{make_descriptor()};
};

// 分支共享输入时只读访问；增益节点新建输出，避免影响另一条分支。
class GainNode final : public ISyncNode {
public:
    explicit GainNode(const ParameterMap& parameters)
        : gain_db_(number_parameter(parameters, "gain_db", 0.0)) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues& inputs, ExecutionContext& context) override {
        check_cancelled(context);
        const auto& value = required_input(inputs, "audio");
        const auto& input = std::get<AudioClipPtr>(value);
        if (!input) {
            throw std::invalid_argument("Gain node received a null Audio value");
        }

        auto output = std::make_shared<AudioClip>();
        output->format = input->format;
        output->samples.reserve(input->samples.size());
        const auto linear_gain = static_cast<float>(std::pow(10.0, gain_db_ / 20.0));
        for (std::size_t index = 0; index < input->samples.size(); ++index) {
            if (index % 4096U == 0) { check_cancelled(context); }
            const auto sample = input->samples[index] * linear_gain;
            if (!std::isfinite(sample)) {
                throw std::runtime_error("Gain produced a non-finite audio sample");
            }
            output->samples.push_back(sample);
        }
        return {{"audio", AudioClipPtr{std::move(output)}}};
    }

    static NodeDescriptor make_descriptor() {
        return NodeDescriptor{
            "gain",
            "Gain",
            "Applies a constant gain in decibels.",
            ExecutionDomain::Synchronous,
            {{"audio", DataType::Audio, true}},
            {{"audio", DataType::Audio, true}},
            {{"gain_db", ParameterType::Number, "Gain in decibels.", false,
                ParameterValue{0.0}, -24.0, 12.0, "dB", {}}},
        };
    }

private:
    double gain_db_{};
    NodeDescriptor descriptor_{make_descriptor()};
};

// 检测节点只读共享音频，输出数值而不是新的音频。
class PeakMeterNode final : public ISyncNode {
public:
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues& inputs, ExecutionContext& context) override {
        check_cancelled(context);
        const auto& input = std::get<AudioClipPtr>(required_input(inputs, "audio"));
        if (!input) {
            throw std::invalid_argument("Peak meter received a null Audio value");
        }
        double peak{};
        for (std::size_t index = 0; index < input->samples.size(); ++index) {
            if (index % 4096U == 0) { check_cancelled(context); }
            const auto sample = input->samples[index];
            if (!std::isfinite(sample)) {
                throw std::invalid_argument("Peak meter received a non-finite audio sample");
            }
            peak = std::max(peak, static_cast<double>(std::abs(sample)));
        }
        return {{"peak", peak}};
    }

    static NodeDescriptor make_descriptor() {
        return NodeDescriptor{
            "peak_meter",
            "Peak Meter",
            "Calculates the absolute peak of an Audio value.",
            ExecutionDomain::Synchronous,
            {{"audio", DataType::Audio, true}},
            {{"peak", DataType::Number, true}},
            {},
        };
    }

private:
    NodeDescriptor descriptor_{make_descriptor()};
};

// 输出节点是有副作用的终点：仅创建新文件；图失败不会回滚已经写出的产物。
class WavOutputNode final : public ISyncNode {
public:
    explicit WavOutputNode(const ParameterMap& parameters)
        : path_(path_parameter(parameters, "path")) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues& inputs, ExecutionContext& context) override {
        const auto& input = std::get<AudioClipPtr>(required_input(inputs, "audio"));
        if (!input) {
            throw std::invalid_argument("WAV output received a null Audio value");
        }
        const auto clipped_samples = write_wav(path_, *input, context);
        return {{"path", path_}, {"clipped_samples", static_cast<double>(clipped_samples)}};
    }

    static NodeDescriptor make_descriptor() {
        return NodeDescriptor{
            "wav_output",
            "WAV Output",
            "Writes PCM16 WAV; clipped_samples counts inputs outside [-1, 32767/32768].",
            ExecutionDomain::Synchronous,
            {{"audio", DataType::Audio, true}},
            {{"path", DataType::FilePath, true}, {"clipped_samples", DataType::Number, true}},
            {{"path", ParameterType::FilePath, "New output PCM16 WAV file; no overwrite.", true}},
        };
    }

private:
    std::filesystem::path path_;
    NodeDescriptor descriptor_{make_descriptor()};
};

// 文本源验证异构数据端口：文本由配置提供，后续可连接 TTS 等扩展节点。
class TextInputNode final : public ISyncNode {
public:
    explicit TextInputNode(const ParameterMap& parameters)
        : text_(std::get<std::string>(parameters.at("text"))) {}
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    OutputValues execute(const InputValues&, ExecutionContext& context) override {
        check_cancelled(context);
        return {{"text", text_}};
    }
    static NodeDescriptor make_descriptor() {
        return {"text_input", "Text Input", "Provides UTF-8 text from configuration.",
            ExecutionDomain::Synchronous, {}, {{"text", DataType::Text, true}},
            {{"text", ParameterType::Text, "UTF-8 text, including empty text.", true}}};
    }
private:
    std::string text_;
    NodeDescriptor descriptor_{make_descriptor()};
};

// UTF-8 文本文件输出不添加 BOM、不改换行；按块检查取消，已有文件始终受保护。
class TextOutputNode final : public ISyncNode {
public:
    explicit TextOutputNode(const ParameterMap& parameters)
        : path_(path_parameter(parameters, "path")) {}
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    OutputValues execute(const InputValues& inputs, ExecutionContext& context) override {
        check_cancelled(context);
        const auto& text = std::get<std::string>(required_input(inputs, "text"));
        detail::ExclusiveFile file(path_);
        for (std::size_t offset = 0; offset < text.size();) {
            check_cancelled(context);
            const auto count = std::min<std::size_t>(4096, text.size() - offset);
            file.write(text.data() + offset, count);
            offset += count;
        }
        file.flush();
        return {{"path", path_}};
    }
    static NodeDescriptor make_descriptor() {
        return {"text_output", "Text Output", "Writes UTF-8 text to a new file.",
            ExecutionDomain::Synchronous, {{"text", DataType::Text, true}},
            {{"path", DataType::FilePath, true}},
            {{"path", ParameterType::FilePath, "New UTF-8 output file; no overwrite.", true}}};
    }
private:
    std::filesystem::path path_;
    NodeDescriptor descriptor_{make_descriptor()};
};

}  // namespace

NodeRegistry create_prototype_node_registry() {
    NodeRegistry registry;
    registry.register_type(
        WavInputNode::make_descriptor(),
        [](const ParameterMap& parameters) {
            return std::make_unique<WavInputNode>(parameters);
        });
    registry.register_type(
        GainNode::make_descriptor(),
        [](const ParameterMap& parameters) {
            return std::make_unique<GainNode>(parameters);
        });
    registry.register_type(
        PeakMeterNode::make_descriptor(),
        [](const ParameterMap&) {
            return std::make_unique<PeakMeterNode>();
        });
    registry.register_type(
        WavOutputNode::make_descriptor(),
        [](const ParameterMap& parameters) {
            return std::make_unique<WavOutputNode>(parameters);
        });
    registry.register_type(TextInputNode::make_descriptor(), [](const ParameterMap& parameters) {
        return std::make_unique<TextInputNode>(parameters);
    });
    registry.register_type(TextOutputNode::make_descriptor(), [](const ParameterMap& parameters) {
        return std::make_unique<TextOutputNode>(parameters);
    });
    register_streaming_node_types(registry);
    return registry;
}

void validate_prototype_file_targets(const GraphDefinition& graph) {
    struct Target { std::string node_id; std::filesystem::path path; };
    std::vector<Target> inputs;
    std::vector<Target> outputs;
    for (const auto& node : graph.nodes) {
        if (node.type_id != "wav_input" && node.type_id != "wav_output" &&
            node.type_id != "text_output" && node.type_id != "wav_stream_input" &&
            node.type_id != "wav_stream_output") { continue; }
        try {
            const auto path = path_parameter(node.parameters, "path");
            const auto normalized = std::filesystem::weakly_canonical(std::filesystem::absolute(path));
            if (node.type_id == "wav_input" || node.type_id == "wav_stream_input") {
                inputs.push_back({node.id, normalized});
            } else {
                // symlink_status 也拒绝悬空符号链接；真正创建时仍使用 O_EXCL 防止竞态。
                if (std::filesystem::exists(std::filesystem::symlink_status(path))) {
                    throw ExecutionError("output_exists", "Output already exists: " + detail::path_utf8(path), node.id, {}, "path");
                }
                if (!std::filesystem::is_directory(normalized.parent_path())) {
                    throw ExecutionError("output_directory_missing", "Output parent directory does not exist: " + detail::path_utf8(path), node.id, {}, "path");
                }
                outputs.push_back({node.id, normalized});
            }
        } catch (const ExecutionError& error) {
            throw ExecutionError(error.code, error.what(), node.id, error.port_id, "path", error.field_path);
        } catch (const std::exception& error) {
            throw ExecutionError("invalid_file_target", error.what(), node.id, {}, "path");
        }
    }
    const auto same_path = [](const auto& left, const auto& right) {
#ifdef _WIN32
        return _wcsicmp(left.c_str(), right.c_str()) == 0;
#else
        return left == right;
#endif
    };
    for (std::size_t i = 0; i < outputs.size(); ++i) {
        for (const auto& input : inputs) {
            if (same_path(outputs[i].path, input.path)) {
                throw ExecutionError("file_target_conflict", "Input and output file paths must differ", outputs[i].node_id, {}, "path");
            }
        }
        for (std::size_t j = 0; j < i; ++j) {
            if (same_path(outputs[i].path, outputs[j].path)) {
                throw ExecutionError("file_target_conflict", "Multiple output nodes target the same file", outputs[i].node_id, {}, "path");
            }
        }
    }
}

GraphDefinition create_prototype_graph(
    const std::filesystem::path& input,
    const std::filesystem::path& output,
    double gain_db) {
    return GraphDefinition{
        {
            {"input", "wav_input", {{"path", input}}},
            {"gain", "gain", {{"gain_db", gain_db}}},
            {"output", "wav_output", {{"path", output}}},
            {"peak", "peak_meter", {}},
        },
        {
            {"input", "audio", "gain", "audio"},
            {"gain", "audio", "output", "audio"},
            {"gain", "audio", "peak", "audio"},
        },
        {
            {"output_file", "output", "path"},
            {"peak", "peak", "peak"},
            {"clipped_samples", "output", "clipped_samples"},
        },
    };
}

}  // namespace audioprocess
