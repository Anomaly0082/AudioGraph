#include "audioprocess/prototype_nodes.h"

#include "audioprocess/audio_buffer.h"
#include "audioprocess/wav_file.h"

#include <algorithm>
#include <cmath>
#include <memory>
#include <stdexcept>
#include <utility>

namespace audioprocess {
namespace {

constexpr std::uint32_t kIoBlockFrames = 1024;

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
    return std::get<std::filesystem::path>(iterator->second);
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

AudioClipPtr load_wav(const std::filesystem::path& path) {
    WavFileSource source(path, kIoBlockFrames);
    AudioBuffer buffer(source.format(), kIoBlockFrames);
    auto clip = std::make_shared<AudioClip>();
    clip->format = source.format();
    clip->samples.reserve(
        static_cast<std::size_t>(source.total_frames()) * source.format().channel_count);

    while (auto block = source.read(buffer)) {
        clip->samples.insert(clip->samples.end(), block->samples.begin(), block->samples.end());
    }
    return clip;
}

void write_wav(const std::filesystem::path& path, const AudioClip& clip) {
    WavFileSink sink(path, clip.format, kIoBlockFrames);
    AudioBuffer buffer(clip.format, kIoBlockFrames);
    const auto total_frames = clip.frame_count();

    std::uint64_t frame_position{};
    while (frame_position < total_frames) {
        const auto frames = static_cast<std::uint32_t>(
            std::min<std::uint64_t>(kIoBlockFrames, total_frames - frame_position));
        auto block = buffer.block(frames, frame_position);
        const auto sample_offset = static_cast<std::size_t>(frame_position) *
            clip.format.channel_count;
        std::copy_n(
            clip.samples.begin() + static_cast<std::ptrdiff_t>(sample_offset),
            block.sample_count(),
            block.samples.begin());
        sink.write(block);
        frame_position += frames;
    }
    sink.finalize();
}

class WavInputNode final : public ISyncNode {
public:
    explicit WavInputNode(const ParameterMap& parameters)
        : path_(path_parameter(parameters, "path")) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues&, ExecutionContext&) override {
        return {{"audio", load_wav(path_)}};
    }

    static NodeDescriptor make_descriptor() {
        return NodeDescriptor{
            "wav_input",
            "WAV Input",
            "Loads a PCM16 WAV file into an Audio value.",
            ExecutionDomain::Synchronous,
            {},
            {{"audio", DataType::Audio, true}},
            {{"path", ParameterType::FilePath, "Input PCM16 WAV file."}},
        };
    }

private:
    std::filesystem::path path_;
    NodeDescriptor descriptor_{make_descriptor()};
};

class GainNode final : public ISyncNode {
public:
    explicit GainNode(const ParameterMap& parameters)
        : gain_db_(number_parameter(parameters, "gain_db", 0.0)) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues& inputs, ExecutionContext&) override {
        const auto& value = required_input(inputs, "audio");
        const auto& input = std::get<AudioClipPtr>(value);
        if (!input) {
            throw std::invalid_argument("Gain node received a null Audio value");
        }

        auto output = std::make_shared<AudioClip>(*input);
        const auto linear_gain = static_cast<float>(std::pow(10.0, gain_db_ / 20.0));
        for (auto& sample : output->samples) {
            sample *= linear_gain;
        }
        return {{"audio", std::move(output)}};
    }

    static NodeDescriptor make_descriptor() {
        return NodeDescriptor{
            "gain",
            "Gain",
            "Applies a constant gain in decibels.",
            ExecutionDomain::Synchronous,
            {{"audio", DataType::Audio, true}},
            {{"audio", DataType::Audio, true}},
            {{"gain_db", ParameterType::Number, "Gain in decibels."}},
        };
    }

private:
    double gain_db_{};
    NodeDescriptor descriptor_{make_descriptor()};
};

class PeakMeterNode final : public ISyncNode {
public:
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues& inputs, ExecutionContext&) override {
        const auto& input = std::get<AudioClipPtr>(required_input(inputs, "audio"));
        if (!input) {
            throw std::invalid_argument("Peak meter received a null Audio value");
        }
        double peak{};
        for (const auto sample : input->samples) {
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

class WavOutputNode final : public ISyncNode {
public:
    explicit WavOutputNode(const ParameterMap& parameters)
        : path_(path_parameter(parameters, "path")) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    OutputValues execute(const InputValues& inputs, ExecutionContext&) override {
        const auto& input = std::get<AudioClipPtr>(required_input(inputs, "audio"));
        if (!input) {
            throw std::invalid_argument("WAV output received a null Audio value");
        }
        write_wav(path_, *input);
        return {{"path", path_}};
    }

    static NodeDescriptor make_descriptor() {
        return NodeDescriptor{
            "wav_output",
            "WAV Output",
            "Writes an Audio value to a PCM16 WAV file.",
            ExecutionDomain::Synchronous,
            {{"audio", DataType::Audio, true}},
            {{"path", DataType::FilePath, true}},
            {{"path", ParameterType::FilePath, "Output PCM16 WAV file."}},
        };
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
    return registry;
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
    };
}

}  // namespace audioprocess

