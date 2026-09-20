#include "audioprocess/execution_error.h"
#include "audioprocess/streaming_graph_executor.h"
#include "audioprocess/streaming_node.h"

#include <algorithm>
#include <atomic>
#include <functional>
#include <iostream>
#include <limits>
#include <memory>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>

namespace {
using namespace audioprocess;

static_assert(std::is_const_v<decltype(AudioStreamBlock::samples)::element_type>);

void require(bool condition, const std::string& message) {
    if (!condition) throw std::runtime_error(message);
}
template<class Action>
ExecutionError rejects(Action action) {
    try { action(); }
    catch (const ExecutionError& error) {
        require(!error.code.empty(), "Stream error has no machine-readable code");
        return error;
    }
    throw std::runtime_error("Invalid stream operation was accepted");
}

enum class BadBlock { None, Empty, TooLarge, WrongSpan, Channels, Position, NonFinite };
struct Observation {
    std::vector<float> input{0, 1, 2, 3, 4, 5, 6, 7, 8};
    std::vector<float> received;
    std::vector<std::string> finishes;
    AudioFormat sink_format{};
    int live{};
    int factories{};
    int reads{};
    int sink_finishes{};
    BadBlock source_error{BadBlock::None};
    bool cancel_on_push{};
    bool cancel_on_finish{};
    bool bad_sink_output{};
    bool throw_in_sink{};
    bool invalid_source_format{};
};

NodeDescriptor descriptor(std::string id, StreamRole role) {
    NodeDescriptor result;
    result.type_id = std::move(id);
    result.display_name = result.type_id;
    result.execution_domain = ExecutionDomain::Streaming;
    result.stream_role = role;
    if (role != StreamRole::Source) result.inputs = {{"audio", DataType::AudioStream}};
    if (role != StreamRole::Sink) result.outputs = {{"audio", DataType::AudioStream}};
    else result.outputs = {{"frames", DataType::Number}};
    return result;
}

// 构造/析构计数独立于Executor，用来检查失败与取消路径的实例释放。
class Probe {
public:
    Probe(NodeDescriptor desc, std::shared_ptr<Observation> state)
        : desc_(std::move(desc)), state_(std::move(state)) { ++state_->live; }
    ~Probe() { --state_->live; }
    Probe(const Probe&) = delete;
    Probe& operator=(const Probe&) = delete;
protected:
    NodeDescriptor desc_;
    std::shared_ptr<Observation> state_;
};

class Source final : public IAudioStreamSource, private Probe {
public:
    Source(NodeDescriptor desc, std::shared_ptr<Observation> state)
        : Probe(std::move(desc), std::move(state)) {}
    const NodeDescriptor& descriptor() const noexcept override { return desc_; }
    AudioFormat open(std::uint32_t max_frames, ExecutionContext&) override {
        limit_ = max_frames;
        if (state_->invalid_source_format) return {};
        return {48'000, 1};
    }
    std::optional<AudioStreamBlock> read(ExecutionContext&) override {
        ++state_->reads;
        if (offset_ == state_->input.size()) return std::nullopt;
        const auto count = std::min<std::size_t>(limit_, state_->input.size() - offset_);
        // 下次read复用该缓冲，任何保存借用引用的下游都会在内容验收中暴露。
        scratch_.assign(state_->input.begin() + static_cast<std::ptrdiff_t>(offset_),
            state_->input.begin() + static_cast<std::ptrdiff_t>(offset_ + count));
        AudioStreamBlock block{scratch_, static_cast<std::uint32_t>(count), 1, offset_};
        offset_ += count;
        switch (state_->source_error) {
        case BadBlock::Empty: block.frame_count = 0; block.samples = {}; break;
        case BadBlock::TooLarge:
            scratch_.assign(static_cast<std::size_t>(limit_) + 1, 0.0F);
            block.samples = scratch_; block.frame_count = limit_ + 1; break;
        case BadBlock::WrongSpan: block.samples = block.samples.first(block.samples.size() - 1); break;
        case BadBlock::Channels: block.channel_count = 2; break;
        case BadBlock::Position: block.frame_position = std::numeric_limits<std::uint64_t>::max(); break;
        case BadBlock::NonFinite: scratch_[0] = std::numeric_limits<float>::quiet_NaN(); break;
        case BadBlock::None: break;
        }
        return block;
    }
private:
    std::uint32_t limit_{};
    std::size_t offset_{};
    std::vector<float> scratch_;
};

// 故意每积累2块才输出2块，覆盖零输出、多输出、内部复制以及尾部排空。
class Buffered final : public IAudioStreamProcessor, private Probe {
public:
    Buffered(NodeDescriptor desc, std::shared_ptr<Observation> state)
        : Probe(std::move(desc), std::move(state)) {}
    const NodeDescriptor& descriptor() const noexcept override { return desc_; }
    AudioFormat prepare(AudioFormat format, std::uint32_t max_frames, ExecutionContext&) override {
        format_ = format; limit_ = max_frames; return format;
    }
    void push(const AudioStreamBlock& block, const StreamEmit& emit, ExecutionContext& context) override {
        if (state_->cancel_on_push && context.cancellation_requested) {
            context.cancellation_requested->store(true);
        }
        pending_.insert(pending_.end(), block.samples.begin(), block.samples.end());
        const auto count = static_cast<std::size_t>(limit_) * format_.channel_count;
        if (pending_.size() >= 2 * count) {
            emit_frames(limit_, emit);
            emit_frames(limit_, emit);
        }
    }
    void finish(const StreamEmit& emit, ExecutionContext& context) override {
        state_->finishes.push_back(desc_.type_id);
        if (state_->cancel_on_finish && context.cancellation_requested) {
            context.cancellation_requested->store(true);
        }
        while (!pending_.empty()) {
            const auto frames = std::min<std::size_t>(limit_, pending_.size() / format_.channel_count);
            emit_frames(static_cast<std::uint32_t>(frames), emit);
        }
    }
private:
    void emit_frames(std::uint32_t frames, const StreamEmit& emit) {
        const auto count = static_cast<std::size_t>(frames) * format_.channel_count;
        const AudioStreamBlock output{std::span<const float>(pending_).first(count), frames,
            format_.channel_count, position_};
        emit(output);
        position_ += frames;
        pending_.erase(pending_.begin(), pending_.begin() + static_cast<std::ptrdiff_t>(count));
    }
    AudioFormat format_{};
    std::uint32_t limit_{};
    std::uint64_t position_{};
    std::vector<float> pending_;
};

// 只验证格式协商/每条边的格式传播，不把该fixture当成真实重采样算法。
class StereoMetadata final : public IAudioStreamProcessor, private Probe {
public:
    StereoMetadata(NodeDescriptor desc, std::shared_ptr<Observation> state, bool invalid)
        : Probe(std::move(desc), std::move(state)), invalid_(invalid) {}
    const NodeDescriptor& descriptor() const noexcept override { return desc_; }
    AudioFormat prepare(AudioFormat input, std::uint32_t, ExecutionContext&) override {
        require(input == AudioFormat{48'000, 1}, "Converter received the wrong input format");
        return {24'000, 2};
    }
    void push(const AudioStreamBlock& block, const StreamEmit& emit, ExecutionContext&) override {
        output_.clear();
        for (auto sample : block.samples) { output_.push_back(sample); output_.push_back(-sample); }
        emit(AudioStreamBlock{output_, block.frame_count,
            static_cast<std::uint16_t>(invalid_ ? 1 : 2), block.frame_position});
    }
    void finish(const StreamEmit&, ExecutionContext&) override { state_->finishes.push_back(desc_.type_id); }
private:
    bool invalid_{};
    std::vector<float> output_;
};

class Sink final : public IAudioStreamSink, private Probe {
public:
    Sink(NodeDescriptor desc, std::shared_ptr<Observation> state)
        : Probe(std::move(desc), std::move(state)) {}
    const NodeDescriptor& descriptor() const noexcept override { return desc_; }
    void prepare(AudioFormat format, std::uint32_t, ExecutionContext&) override {
        state_->sink_format = format;
    }
    void push(const AudioStreamBlock& block, ExecutionContext&) override {
        if (state_->throw_in_sink) throw std::runtime_error("Simulated sink write failure");
        state_->received.insert(state_->received.end(), block.samples.begin(), block.samples.end());
        frames_ += block.frame_count;
    }
    OutputValues finish(ExecutionContext&) override {
        state_->finishes.push_back("sink");
        ++state_->sink_finishes;
        if (state_->bad_sink_output) return {{"frames", std::string("wrong")}};
        return {{"frames", static_cast<double>(frames_)}};
    }
private:
    std::uint64_t frames_{};
};

NodeRegistry registry_for(const std::shared_ptr<Observation>& state) {
    NodeRegistry registry;
    const auto source = descriptor("source", StreamRole::Source);
    registry.register_stream_type(source, [source, state](const ParameterMap&) {
        ++state->factories; return std::make_unique<Source>(source, state);
    });
    for (const std::string id : {"buffer_a", "buffer_b"}) {
        const auto desc = descriptor(id, StreamRole::Processor);
        registry.register_stream_type(desc, [desc, state](const ParameterMap&) {
            ++state->factories; return std::make_unique<Buffered>(desc, state);
        });
    }
    for (const bool invalid : {false, true}) {
        const auto desc = descriptor(invalid ? "bad_format" : "convert", StreamRole::Processor);
        registry.register_stream_type(desc, [desc, state, invalid](const ParameterMap&) {
            ++state->factories; return std::make_unique<StereoMetadata>(desc, state, invalid);
        });
    }
    const auto sink = descriptor("sink", StreamRole::Sink);
    registry.register_stream_type(sink, [sink, state](const ParameterMap&) {
        ++state->factories; return std::make_unique<Sink>(sink, state);
    });
    return registry;
}

GraphDefinition graph_for(std::vector<std::string> processors = {}) {
    GraphDefinition graph;
    graph.nodes = {{"src", "source", {}}, {"dst", "sink", {}}};
    std::string previous = "src";
    for (const auto& id : processors) {
        graph.nodes.push_back({id, id, {}});
        graph.connections.push_back({previous, "audio", id, "audio"});
        previous = id;
    }
    graph.connections.push_back({previous, "audio", "dst", "audio"});
    graph.exports = {{"frames", "dst", "frames"}};
    // 配置顺序倒置，确保运行遵守连接而不是数组顺序。
    std::reverse(graph.nodes.begin(), graph.nodes.end());
    return graph;
}

void test_buffering_finish_and_new_tasks() {
    for (const std::uint32_t size : {1U, 3U, 7U}) {
        auto state = std::make_shared<Observation>();
        const auto registry = registry_for(state);
        const auto graph = graph_for({"buffer_a", "buffer_b"});
        static_cast<void>(validate_stream_graph(graph, registry));
        auto executor = StreamingGraphExecutor::compile(graph, registry);
        require(state->factories == 0, "Stream validate/compile invoked node factories");
        const auto result = executor.execute(size);
        require(state->received == state->input, "Buffered stream lost, duplicated or changed samples");
        require(std::get<double>(result.value("dst", "frames")) == 9.0, "Wrong stream result");
        require(state->finishes == std::vector<std::string>{"buffer_a", "buffer_b", "sink"},
            "Finish order did not allow upstream tails to reach downstream buffers");
        require(state->live == 0, "Completed stream retained task node instances");
        state->received.clear(); state->finishes.clear();
        const auto second = executor.execute(size);
        require(state->received == state->input &&
            std::get<double>(second.value("dst", "frames")) == 9.0, "New task inherited node state");
    }
    auto empty = std::make_shared<Observation>();
    empty->input.clear();
    auto executor = StreamingGraphExecutor::compile(graph_for({"buffer_a"}), registry_for(empty));
    require(std::get<double>(executor.execute().value("dst", "frames")) == 0.0 &&
        empty->sink_finishes == 1, "Empty file EOS did not finish cleanly");
}

void test_format_propagation_and_block_validation() {
    auto state = std::make_shared<Observation>();
    auto executor = StreamingGraphExecutor::compile(graph_for({"convert", "buffer_a"}), registry_for(state));
    static_cast<void>(executor.execute(3));
    require(state->sink_format == AudioFormat{24'000, 2}, "Downstream preparation lost converted format");
    std::vector<float> expected;
    for (auto sample : state->input) { expected.push_back(sample); expected.push_back(-sample); }
    require(state->received == expected, "Per-edge format propagation lost converted samples");

    for (const auto bad : {BadBlock::Empty, BadBlock::TooLarge, BadBlock::WrongSpan,
                          BadBlock::Channels, BadBlock::Position, BadBlock::NonFinite}) {
        auto broken = std::make_shared<Observation>();
        broken->source_error = bad;
        auto run = StreamingGraphExecutor::compile(graph_for(), registry_for(broken));
        const auto error = rejects([&] { static_cast<void>(run.execute(3)); });
        require(error.node_id == "src", "Malformed source block lacks source node location");
        require(broken->received.empty() && broken->sink_finishes == 0 && broken->live == 0,
            "Malformed source data reached sink or resources were retained");
    }
    auto broken = std::make_shared<Observation>();
    auto run = StreamingGraphExecutor::compile(graph_for({"bad_format"}), registry_for(broken));
    const auto error = rejects([&] { static_cast<void>(run.execute(3)); });
    require(error.node_id == "bad_format" && broken->received.empty(), "Bad processor format reached sink");
    auto bad_result = std::make_shared<Observation>();
    bad_result->bad_sink_output = true;
    auto result_run = StreamingGraphExecutor::compile(graph_for(), registry_for(bad_result));
    require(rejects([&] { static_cast<void>(result_run.execute(3)); }).node_id == "dst",
        "Sink scalar output type was not validated");

    auto bad_format = std::make_shared<Observation>();
    bad_format->invalid_source_format = true;
    auto invalid_open = StreamingGraphExecutor::compile(graph_for(), registry_for(bad_format));
    require(rejects([&] { static_cast<void>(invalid_open.execute(3)); }).node_id == "src" &&
        bad_format->reads == 0 && bad_format->sink_format == AudioFormat{} && bad_format->live == 0,
        "Invalid source format reached sink preparation or retained resources");

    auto sink_failure = std::make_shared<Observation>();
    sink_failure->throw_in_sink = true;
    auto nested = StreamingGraphExecutor::compile(graph_for({"buffer_a", "buffer_b"}), registry_for(sink_failure));
    require(rejects([&] { static_cast<void>(nested.execute(3)); }).node_id == "dst" &&
        sink_failure->live == 0 && sink_failure->sink_finishes == 0,
        "Nested sink failure was attributed to upstream or incorrectly finished");
}

void test_cancel_does_not_drain() {
    for (const int stage : {0, 1, 2}) {
        auto state = std::make_shared<Observation>();
        state->input.resize(1); // 只在finish时排出，可精确区分取消与排尾。
        state->cancel_on_push = stage == 1;
        state->cancel_on_finish = stage == 2;
        std::atomic_bool cancellation{stage == 0};
        auto executor = StreamingGraphExecutor::compile(graph_for({"buffer_a", "buffer_b"}), registry_for(state));
        const auto error = rejects([&] { static_cast<void>(executor.execute(3, {&cancellation})); });
        require(error.code == "cancelled", "Cancellation did not produce a cancelled error");
        require(state->received.empty() && state->sink_finishes == 0 && state->live == 0,
            "Cancelled stream drained or retained task resources");
        if (stage == 0) require(state->reads == 0, "Pre-cancelled stream read input");
        if (stage == 1) require(state->finishes.empty(), "Cancelled push still called finish");
        if (stage == 2) require(state->finishes == std::vector<std::string>{"buffer_a"},
            "Cancel during tail emission continued downstream finish");
    }
}

void test_linear_validation_limits() {
    auto state = std::make_shared<Observation>();
    const auto registry = registry_for(state);
    const auto base = graph_for({"buffer_a"});
    auto invalid = base;
    invalid.connections.pop_back();
    static_cast<void>(rejects([&] { static_cast<void>(validate_stream_graph(invalid, registry)); }));
    invalid = base;
    invalid.nodes.push_back({"other", "sink", {}});
    invalid.connections.push_back({"src", "audio", "other", "audio"});
    static_cast<void>(rejects([&] { static_cast<void>(validate_stream_graph(invalid, registry)); }));
    invalid = base;
    invalid.nodes.push_back({"buffer_b", "buffer_b", {}});
    // 每个端口都只连接一次，但另外两个节点组成孤立环，不能被执行计划静默忽略。
    invalid.connections = {{"src", "audio", "dst", "audio"},
        {"buffer_a", "audio", "buffer_b", "audio"},
        {"buffer_b", "audio", "buffer_a", "audio"}};
    static_cast<void>(rejects([&] { static_cast<void>(validate_stream_graph(invalid, registry)); }));
    invalid = base;
    invalid.exports = {{"stream", "src", "audio"}};
    static_cast<void>(rejects([&] { static_cast<void>(validate_stream_graph(invalid, registry)); }));

    GraphDefinition long_chain;
    long_chain.nodes = {{"src", "source", {}}, {"dst", "sink", {}}};
    std::string previous = "src";
    for (int index = 0; index < 127; ++index) {
        const auto id = "p" + std::to_string(index);
        long_chain.nodes.push_back({id, "buffer_a", {}});
        long_chain.connections.push_back({previous, "audio", id, "audio"});
        previous = id;
    }
    long_chain.connections.push_back({previous, "audio", "dst", "audio"});
    static_cast<void>(rejects([&] { static_cast<void>(validate_stream_graph(long_chain, registry)); }));
    require(state->factories == 0, "Invalid stream graphs instantiated nodes");
    auto executor = StreamingGraphExecutor::compile(base, registry);
    for (const std::uint32_t size : {0U, 65537U}) {
        static_cast<void>(rejects([&] { static_cast<void>(executor.execute(size)); }));
    }
    require(state->reads == 0, "Invalid block size started source IO");
}

void test_registry_and_mixed_domains() {
    auto state = std::make_shared<Observation>();
    auto registry = registry_for(state);
    auto wrong_role = descriptor("lying_source", StreamRole::Source);
    registry.register_stream_type(wrong_role, [wrong_role, state](const ParameterMap&) {
        // 元数据声明Source，实际仅实现Sink接口，不能等到执行时才做错误的强转。
        return std::make_unique<Sink>(wrong_role, state);
    });
    require(rejects([&] { static_cast<void>(registry.create_stream("lying_source", {})); }).code ==
        "invalid_factory", "Registry accepted a factory with the wrong C++ role interface");
    require(state->live == 0, "Rejected factory retained its instance");

    NodeDescriptor whole;
    whole.type_id = "whole_audio";
    whole.outputs = {{"audio", DataType::Audio}};
    registry.register_type(whole, [](const ParameterMap&) -> std::unique_ptr<ISyncNode> {
        throw std::runtime_error("Mixed graph validation must not create nodes");
    });
    auto mixed = graph_for();
    for (auto& node : mixed.nodes) if (node.id == "src") node.type_id = "whole_audio";
    static_cast<void>(rejects([&] { static_cast<void>(validate_stream_graph(mixed, registry)); }));
}

} // namespace

int main() {
    try {
        test_buffering_finish_and_new_tasks();
        test_format_propagation_and_block_validation();
        test_cancel_does_not_drain();
        test_linear_validation_limits();
        test_registry_and_mixed_domains();
        std::cout << "Independent streaming graph contract tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Streaming contract failure: " << error.what() << '\n';
        return 1;
    }
}
