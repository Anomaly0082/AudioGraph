#include "audioprocess/streaming_nodes.h"

#include "audioprocess/audio_buffer.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/streaming_node.h"
#include "audioprocess/wav_file.h"

#include <cmath>
#include <memory>
#include <utility>

namespace audioprocess {
namespace {

void check_cancelled(const ExecutionContext& context) {
    if (context.cancelled()) {
        throw ExecutionError("cancelled", "Stream processing cancelled; partial output files may remain");
    }
}

std::filesystem::path file_parameter(const ParameterMap& parameters) {
    const auto found = parameters.find("path");
    if (found == parameters.end() || !std::holds_alternative<std::filesystem::path>(found->second)) {
        throw ExecutionError("invalid_parameter", "Expected a file path", {}, {}, "path");
    }
    const auto path = std::get<std::filesystem::path>(found->second);
    if (path.empty() || path.native().find(std::filesystem::path::value_type{}) !=
                            std::filesystem::path::string_type::npos) {
        throw ExecutionError("invalid_path", "File path must be nonempty and contain no NUL", {}, {}, "path");
    }
    return path;
}

void validate_block(const AudioStreamBlock& block, AudioFormat format, std::uint32_t maximum_frames) {
    if (block.frame_count == 0 || block.frame_count > maximum_frames ||
        block.channel_count != format.channel_count ||
        block.samples.size() != static_cast<std::size_t>(block.frame_count) * block.channel_count) {
        throw ExecutionError("invalid_stream_block", "Invalid or incompatible audio stream block");
    }
}

// 每次 read 只解码一个块；借用视图指向节点工作缓冲，下一次 read 才覆盖。
class WavStreamInputNode final : public IAudioStreamSource {
public:
    explicit WavStreamInputNode(const ParameterMap& parameters) : path_(file_parameter(parameters)) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    AudioFormat open(std::uint32_t maximum_frames, ExecutionContext& context) override {
        check_cancelled(context);
        if (source_) { throw ExecutionError("invalid_lifecycle", "Stream source is already open"); }
        auto source = std::make_unique<WavFileSource>(path_, maximum_frames);
        auto buffer = std::make_unique<AudioBuffer>(source->format(), maximum_frames);
        check_cancelled(context);
        source_ = std::move(source);
        buffer_ = std::move(buffer);
        return source_->format();
    }

    std::optional<AudioStreamBlock> read(ExecutionContext& context) override {
        check_cancelled(context);
        if (!source_) { throw ExecutionError("invalid_lifecycle", "Stream source must be opened before read"); }
        const auto block = source_->read(*buffer_);
        check_cancelled(context);
        if (!block) { return std::nullopt; }
        return AudioStreamBlock{block->samples, block->frame_count, block->channel_count, block->frame_position};
    }

    static NodeDescriptor make_descriptor() {
        return {"wav_stream_input", "WAV Stream Input", "Reads PCM16 WAV in bounded, borrowed audio blocks.",
            ExecutionDomain::Streaming, {}, {{"audio", DataType::AudioStream, true}},
            {{"path", ParameterType::FilePath, "Input PCM16 WAV file.", true}}, StreamRole::Source};
    }

private:
    std::filesystem::path path_;
    std::unique_ptr<WavFileSource> source_;
    std::unique_ptr<AudioBuffer> buffer_;
    NodeDescriptor descriptor_{make_descriptor()};
};

// 输入始终只读。prepare 分配固定容量输出，push 内计算并同步交给下游。
class StreamGainNode final : public IAudioStreamProcessor {
public:
    explicit StreamGainNode(const ParameterMap& parameters)
        : gain_(static_cast<float>(std::pow(10.0, std::get<double>(parameters.at("gain_db")) / 20.0))) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    AudioFormat prepare(AudioFormat format, std::uint32_t maximum_frames, ExecutionContext& context) override {
        check_cancelled(context);
        if (buffer_) { throw ExecutionError("invalid_lifecycle", "Stream Gain is already prepared"); }
        buffer_ = std::make_unique<AudioBuffer>(format, maximum_frames);
        return format;
    }

    void push(const AudioStreamBlock& input, const StreamEmit& emit, ExecutionContext& context) override {
        check_cancelled(context);
        require_active();
        if (!emit) { throw ExecutionError("invalid_stream_callback", "Stream Gain requires an output callback"); }
        validate_block(input, buffer_->format(), buffer_->maximum_frames());
        auto output = buffer_->block(input.frame_count, input.frame_position);
        for (std::size_t index = 0; index < input.samples.size(); ++index) {
            if (index % 4096U == 0) { check_cancelled(context); }
            const auto sample = input.samples[index] * gain_;
            if (!std::isfinite(sample)) {
                throw ExecutionError("invalid_stream_block", "Stream Gain encountered a non-finite audio sample");
            }
            output.samples[index] = sample;
        }
        check_cancelled(context);
        emit({output.samples, output.frame_count, output.channel_count, output.frame_position});
    }

    void finish(const StreamEmit&, ExecutionContext& context) override {
        check_cancelled(context);
        require_active();
        // 常量 Gain 没有延迟状态，因此正常 EOS 不产生额外输出块。
        finished_ = true;
    }

    static NodeDescriptor make_descriptor() {
        return {"stream_gain", "Stream Gain", "Applies gain to each block without retaining the complete recording.",
            ExecutionDomain::Streaming, {{"audio", DataType::AudioStream, true}},
            {{"audio", DataType::AudioStream, true}},
            {{"gain_db", ParameterType::Number, "Gain in decibels.", false,
                ParameterValue{0.0}, -24.0, 12.0, "dB", {}}}, StreamRole::Processor};
    }

private:
    void require_active() const {
        if (!buffer_ || finished_) {
            throw ExecutionError("invalid_lifecycle", "Stream Gain requires prepare and must not be finished");
        }
    }
    float gain_{};
    std::unique_ptr<AudioBuffer> buffer_;
    bool finished_{};
    NodeDescriptor descriptor_{make_descriptor()};
};

// 只读写出流块；正常结束才返回摘要。取消/异常时 RAII 关闭文件，部分产物保留。
class WavStreamOutputNode final : public IAudioStreamSink {
public:
    explicit WavStreamOutputNode(const ParameterMap& parameters) : path_(file_parameter(parameters)) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }

    void prepare(AudioFormat format, std::uint32_t maximum_frames, ExecutionContext& context) override {
        check_cancelled(context);
        if (sink_ || finished_) { throw ExecutionError("invalid_lifecycle", "Stream output is already prepared or finished"); }
        sink_ = std::make_unique<WavFileSink>(path_, format, maximum_frames);
        format_ = format;
        maximum_frames_ = maximum_frames;
    }

    void push(const AudioStreamBlock& input, ExecutionContext& context) override {
        check_cancelled(context);
        require_active();
        validate_block(input, format_, maximum_frames_);
        std::uint64_t block_clipped{};
        for (std::size_t index = 0; index < input.samples.size(); ++index) {
            if (index % 4096U == 0) { check_cancelled(context); }
            const auto sample = input.samples[index];
            if (!std::isfinite(sample)) {
                throw ExecutionError("invalid_stream_block", "WAV stream output received a non-finite audio sample");
            }
            if (sample < -1.0F || sample > 32767.0F / 32768.0F) { ++block_clipped; }
        }
        check_cancelled(context);
        sink_->write(input.samples, input.frame_count);
        clipped_samples_ += block_clipped;
    }

    OutputValues finish(ExecutionContext& context) override {
        check_cancelled(context);
        require_active();
        sink_->finalize();
        const auto frames_written = sink_->frames_written();
        sink_.reset(); // 及时关闭输出，外部可立即重新打开并比较文件。
        finished_ = true;
        return {{"path", path_}, {"frames_written", static_cast<double>(frames_written)},
                {"clipped_samples", static_cast<double>(clipped_samples_)}};
    }

    static NodeDescriptor make_descriptor() {
        return {"wav_stream_output", "WAV Stream Output",
            "Writes PCM16 blocks to a new file; reports frame count and samples outside [-1, 32767/32768].",
            ExecutionDomain::Streaming, {{"audio", DataType::AudioStream, true}},
            {{"path", DataType::FilePath, true}, {"frames_written", DataType::Number, true},
                {"clipped_samples", DataType::Number, true}},
            {{"path", ParameterType::FilePath, "New PCM16 WAV output; existing files are never overwritten.", true}},
            StreamRole::Sink};
    }

private:
    void require_active() const {
        if (!sink_ || finished_) {
            throw ExecutionError("invalid_lifecycle", "Stream output requires prepare and must not be finished");
        }
    }
    std::filesystem::path path_;
    AudioFormat format_{};
    std::uint32_t maximum_frames_{};
    std::uint64_t clipped_samples_{};
    std::unique_ptr<WavFileSink> sink_;
    bool finished_{};
    NodeDescriptor descriptor_{make_descriptor()};
};

} // namespace

void register_streaming_node_types(NodeRegistry& registry) {
    registry.register_stream_type(WavStreamInputNode::make_descriptor(), [](const ParameterMap& parameters) {
        return std::make_unique<WavStreamInputNode>(parameters);
    });
    registry.register_stream_type(StreamGainNode::make_descriptor(), [](const ParameterMap& parameters) {
        return std::make_unique<StreamGainNode>(parameters);
    });
    registry.register_stream_type(WavStreamOutputNode::make_descriptor(), [](const ParameterMap& parameters) {
        return std::make_unique<WavStreamOutputNode>(parameters);
    });
}

} // namespace audioprocess
