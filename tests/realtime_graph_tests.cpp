#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/realtime_node.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/wav_file.h"

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <limits>
#include <memory>
#include <new>
#include <stdexcept>
#include <string>
#include <vector>
#ifdef _WIN32
#include <malloc.h>
#endif

// 只监测当前线程的 C++ new/new[]（包括对齐分配）；不声称拦截 OS/malloc 分配。
// 缓冲、线程、断言和日志在检测范围之外创建，回调由测试预分配的数据驱动。
namespace allocation_probe {
thread_local bool enabled = false;
thread_local std::size_t count = 0;
void mark() noexcept { if (enabled) ++count; }
void* allocate(std::size_t size) {
    mark();
    if (void* memory = std::malloc(size == 0 ? 1 : size)) return memory;
    throw std::bad_alloc{};
}
void* aligned_allocate(std::size_t size, std::size_t alignment) {
    mark();
#ifdef _WIN32
    if (void* memory = _aligned_malloc(size == 0 ? 1 : size, alignment)) return memory;
#else
    if (size > std::numeric_limits<std::size_t>::max() - alignment) throw std::bad_alloc{};
    const auto padded = ((std::max<std::size_t>(size, 1) + alignment - 1) / alignment) * alignment;
    if (void* memory = std::aligned_alloc(alignment, padded)) return memory;
#endif
    throw std::bad_alloc{};
}
void aligned_free(void* memory) noexcept {
#ifdef _WIN32
    _aligned_free(memory);
#else
    std::free(memory);
#endif
}
} // namespace allocation_probe

void* operator new(std::size_t size) { return allocation_probe::allocate(size); }
void* operator new[](std::size_t size) { return allocation_probe::allocate(size); }
void operator delete(void* memory) noexcept { std::free(memory); }
void operator delete[](void* memory) noexcept { std::free(memory); }
void operator delete(void* memory, std::size_t) noexcept { std::free(memory); }
void operator delete[](void* memory, std::size_t) noexcept { std::free(memory); }
void* operator new(std::size_t size, std::align_val_t align) {
    return allocation_probe::aligned_allocate(size, static_cast<std::size_t>(align));
}
void* operator new[](std::size_t size, std::align_val_t align) {
    return allocation_probe::aligned_allocate(size, static_cast<std::size_t>(align));
}
void operator delete(void* memory, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void operator delete[](void* memory, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void operator delete(void* memory, std::size_t, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void operator delete[](void* memory, std::size_t, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
    try { return ::operator new(size); } catch (...) { return nullptr; }
}
void* operator new[](std::size_t size, const std::nothrow_t&) noexcept {
    try { return ::operator new[](size); } catch (...) { return nullptr; }
}
void operator delete(void* memory, const std::nothrow_t&) noexcept { std::free(memory); }
void operator delete[](void* memory, const std::nothrow_t&) noexcept { std::free(memory); }
void* operator new(std::size_t size, std::align_val_t align, const std::nothrow_t&) noexcept {
    try { return ::operator new(size, align); } catch (...) { return nullptr; }
}
void* operator new[](std::size_t size, std::align_val_t align, const std::nothrow_t&) noexcept {
    try { return ::operator new[](size, align); } catch (...) { return nullptr; }
}
void operator delete(void* memory, std::align_val_t, const std::nothrow_t&) noexcept {
    allocation_probe::aligned_free(memory);
}
void operator delete[](void* memory, std::align_val_t, const std::nothrow_t&) noexcept {
    allocation_probe::aligned_free(memory);
}


namespace {
using namespace audioprocess;

void require(bool condition, const std::string& message) {
    if (!condition) throw std::runtime_error(message);
}
template<class Action>
ExecutionError rejects(Action action) {
    try { action(); }
    catch (const ExecutionError& error) {
        require(!error.code.empty(), "Realtime error has no machine-readable code");
        return error;
    }
    throw std::runtime_error("Invalid realtime operation was accepted");
}
bool near(float actual, float expected) { return std::abs(actual - expected) < 0.00001F; }

enum class Behavior { History, PrepareFailure, ProcessFailure, NonFiniteOutput, InvalidStatus };
struct Counters { int factories{}, prepares{}, processes{}, live{}; bool fail_prepare{}; };

NodeDescriptor history_descriptor(std::string id = "history") {
    NodeDescriptor descriptor;
    descriptor.type_id = std::move(id);
    descriptor.display_name = "Independent realtime test processor";
    descriptor.execution_domain = ExecutionDomain::Realtime;
    descriptor.realtime_role = RealtimeRole::Processor;
    descriptor.realtime_capabilities = RealtimeCapabilities{};
    descriptor.inputs = {{"audio", DataType::AudioStream}};
    descriptor.outputs = {{"audio", DataType::AudioStream}};
    return descriptor;
}

// 第三个节点由测试独立注册：每帧加上累计序号，验证跨块历史与调用顺序。
class HistoryNode final : public IRealtimeProcessor {
public:
    HistoryNode(NodeDescriptor descriptor, std::shared_ptr<Counters> counters, Behavior behavior)
        : descriptor_(std::move(descriptor)), counters_(std::move(counters)), behavior_(behavior) {
        ++counters_->live;
    }
    ~HistoryNode() override { --counters_->live; }
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    void prepare(AudioFormat, std::uint32_t) override {
        ++counters_->prepares;
        if (behavior_ == Behavior::PrepareFailure || counters_->fail_prepare)
            throw std::runtime_error("Simulated preparation failure");
        frames_ = 0;
    }
    RealtimeProcessStatus process(std::span<float> samples) noexcept override {
        ++counters_->processes;
        if (behavior_ == Behavior::ProcessFailure) return RealtimeProcessStatus::NodeFailed;
        if (behavior_ == Behavior::InvalidStatus) return static_cast<RealtimeProcessStatus>(0xffffU);
        if (behavior_ == Behavior::NonFiniteOutput) {
            samples[0] = std::numeric_limits<float>::quiet_NaN();
            return RealtimeProcessStatus::Ok;
        }
        for (auto& sample : samples) sample += static_cast<float>(++frames_) * 0.125F;
        return RealtimeProcessStatus::Ok;
    }
private:
    NodeDescriptor descriptor_;
    std::shared_ptr<Counters> counters_;
    Behavior behavior_;
    std::uint64_t frames_{};
};

NodeRegistry registry_with_history(const std::shared_ptr<Counters>& counters,
                                   Behavior behavior = Behavior::History,
                                   NodeDescriptor descriptor = history_descriptor()) {
    NodeRegistry registry;
    register_realtime_nodes(registry);
    const auto factory_descriptor = descriptor;
    registry.register_realtime_type(std::move(descriptor),
        [factory_descriptor, counters, behavior](const ParameterMap&) {
            ++counters->factories;
            return std::make_unique<HistoryNode>(factory_descriptor, counters, behavior);
        });
    return registry;
}

GraphDefinition with_history(std::optional<float> gain = std::nullopt) {
    auto graph = make_realtime_graph("test-input", "test-output", gain);
    // helper生成的链最后一条边通向设备输出；在该边插入第三方处理器。
    auto& last = graph.connections.back();
    const auto sink = last.target_node;
    const auto sink_port = last.target_port;
    last.target_node = "custom";
    last.target_port = "audio";
    graph.nodes.push_back({"custom", "history", {}});
    graph.connections.push_back({"custom", "audio", sink, sink_port});
    std::reverse(graph.nodes.begin(), graph.nodes.end());
    return graph;
}

void test_identity_and_gain_arithmetic() {
    NodeRegistry registry;
    register_realtime_nodes(registry);
    for (const auto gain : {std::optional<float>{}, std::optional<float>{0.0F},
                           std::optional<float>{6.020599913F}, std::optional<float>{-24.0F},
                           std::optional<float>{12.0F}}) {
        auto plan = RealtimeGraphExecutor::compile(make_realtime_graph("in", "out", gain), registry);
        std::array<float, 4> samples{0.125F, -0.25F, 0.0F, 0.03125F};
        const auto original = samples;
        plan.prepare({48000, 1}, 256);
        require(plan.process(samples).status == RealtimeProcessStatus::Ok, "Valid realtime graph failed");
        const auto multiplier = gain ? std::pow(10.0F, *gain / 20.0F) : 1.0F;
        for (std::size_t i = 0; i < samples.size(); ++i) {
            require(near(samples[i], original[i] * multiplier), "Graph gain or bypass changed samples incorrectly");
        }
    }
    for (const float gain : {-24.01F, 12.01F, std::numeric_limits<float>::quiet_NaN(),
                            std::numeric_limits<float>::infinity()}) {
        static_cast<void>(rejects([&] {
            static_cast<void>(RealtimeGraphExecutor::compile(make_realtime_graph("in", "out", gain), registry));
        }));
    }
}

void test_extension_order_state_and_reprepare() {
    auto counts = std::make_shared<Counters>();
    const auto registry = registry_with_history(counts);
    const auto graph = with_history(6.020599913F);
    static_cast<void>(validate_realtime_graph(graph, registry));
    auto plan = RealtimeGraphExecutor::compile(graph, registry);
    require(counts->factories == 0, "Realtime validation/compile invoked a processor factory");
    std::array<float, 1> samples{0.25F};
    require(plan.process(samples).status == RealtimeProcessStatus::NotPrepared,
        "Unprepared graph was executed");
    plan.prepare({48000, 1}, 256);
    require(plan.process(samples).status == RealtimeProcessStatus::Ok && near(samples[0], 0.625F),
        "Graph used node array order instead of Gain -> third-party processor connections");
    samples[0] = 0.25F;
    require(plan.process(samples).status == RealtimeProcessStatus::Ok && near(samples[0], 0.75F),
        "Processor lost history across blocks");
    plan.prepare({48000, 1}, 256); // 停回调时重建，相当于新任务/状态重置。
    samples[0] = 0.25F;
    require(plan.process(samples).status == RealtimeProcessStatus::Ok && near(samples[0], 0.625F),
        "Reprepare retained previous task history");
    require(counts->live == 1 && counts->factories == 2, "Reprepare did not replace its processor instance");
}

void test_validation_before_factory() {
    auto counts = std::make_shared<Counters>();
    auto registry = registry_with_history(counts);
    const auto graph = with_history();
    auto malformed = graph;
    malformed.connections.clear();
    static_cast<void>(rejects([&] { static_cast<void>(validate_realtime_graph(malformed, registry)); }));
    malformed = graph;
    malformed.connections.push_back(malformed.connections.front());
    static_cast<void>(rejects([&] { static_cast<void>(validate_realtime_graph(malformed, registry)); }));
    malformed = graph;
    malformed.exports = {{"audio", "custom", "audio"}};
    static_cast<void>(rejects([&] { static_cast<void>(validate_realtime_graph(malformed, registry)); }));
    malformed = graph;
    for (auto& node : malformed.nodes) if (node.id == "custom") node.parameters["typo"] = 1.0;
    static_cast<void>(rejects([&] { static_cast<void>(validate_realtime_graph(malformed, registry)); }));

    NodeDescriptor offline;
    offline.type_id = "offline";
    offline.outputs = {{"audio", DataType::Audio}};
    registry.register_type(offline, [](const ParameterMap&) -> std::unique_ptr<ISyncNode> {
        throw std::runtime_error("Offline factory must not run during realtime validation");
    });
    malformed = graph;
    for (auto& node : malformed.nodes) if (node.id == "custom") node.type_id = "offline";
    static_cast<void>(rejects([&] { static_cast<void>(validate_realtime_graph(malformed, registry)); }));

    auto plan = RealtimeGraphExecutor::compile(graph, registry);
    for (const auto format : {AudioFormat{44100, 1}, AudioFormat{48000, 2}}) {
        static_cast<void>(rejects([&] { plan.prepare(format, 256); }));
    }
    for (const std::uint32_t frames : {0U, 65537U}) {
        static_cast<void>(rejects([&] { plan.prepare({48000, 1}, frames); }));
    }
    require(counts->factories == 0, "Invalid graph or format created runtime nodes");
}

void test_prepare_failure_and_callback_fault_location() {
    auto counts = std::make_shared<Counters>();
    {
        auto plan = RealtimeGraphExecutor::compile(with_history(), registry_with_history(counts, Behavior::PrepareFailure));
        const auto error = rejects([&] { plan.prepare({48000, 1}, 256); });
        require(error.node_id == "custom", "Prepare error was not located at its node");
    }
    require(counts->live == 0, "Failed preparation leaked processor instances");
    for (const auto behavior : {Behavior::ProcessFailure, Behavior::NonFiniteOutput, Behavior::InvalidStatus}) {
        auto plan = RealtimeGraphExecutor::compile(with_history(), registry_with_history(counts, behavior));
        plan.prepare({48000, 1}, 256);
        std::array<float, 4> samples{};
        const auto result = plan.process(samples);
        require(result.status == (behavior == Behavior::NonFiniteOutput ?
            RealtimeProcessStatus::NonFiniteOutput : RealtimeProcessStatus::NodeFailed),
            "Process failure or non-finite node output was not detected");
        require(plan.node_id(result.node_index) == "custom", "Callback failure has the wrong processor index");
    }
    auto plan = RealtimeGraphExecutor::compile(with_history(), registry_with_history(counts));
    plan.prepare({48000, 1}, 4);
    const auto prior = counts->processes;
    std::array<float, 5> too_large{};
    require(plan.process(too_large).status == RealtimeProcessStatus::InvalidBlock,
        "Realtime callback accepted a block larger than prepared capacity");
    std::array<float, 1> invalid{std::numeric_limits<float>::quiet_NaN()};
    require(plan.process(invalid).status == RealtimeProcessStatus::NonFiniteInput && counts->processes == prior,
        "Invalid input reached a processor");
}

void test_per_node_capacity_and_failed_reprepare() {
    auto counts = std::make_shared<Counters>();
    auto descriptor = history_descriptor();
    descriptor.realtime_capabilities->maximum_block_frames = 64;
    const auto registry = registry_with_history(counts, Behavior::History, descriptor);
    const auto graph = with_history();
    static_cast<void>(validate_realtime_graph(graph, registry, {48000, 1}, 64));
    auto plan = RealtimeGraphExecutor::compile(graph, registry);
    require(counts->factories == 0, "Compile created the limited-capacity processor");
    static_cast<void>(rejects([&] { plan.prepare({48000, 1}, 65); }));
    require(counts->factories == 0, "Unsupported block budget reached a factory");
    plan.prepare({48000, 1}, 64);
    std::array<float, 1> samples{0.125F};
    require(plan.process(samples).status == RealtimeProcessStatus::Ok && near(samples[0], 0.25F),
        "Supported smaller callback budget was unusable");
    counts->fail_prepare = true;
    static_cast<void>(rejects([&] { plan.prepare({48000, 1}, 64); }));
    require(counts->live == 1, "Failed reprepare leaked the failed replacement instance");
    samples[0] = 0.125F;
    require(plan.process(samples).status == RealtimeProcessStatus::Ok && near(samples[0], 0.375F),
        "Failed reprepare destroyed the previous valid plan/state");
}

void test_registry_rejects_incompatible_realtime_contracts() {
    auto counts = std::make_shared<Counters>();
    const auto descriptor = history_descriptor();
    for (int failure = 0; failure < 3; ++failure) {
        NodeRegistry registry;
        auto actual = descriptor;
        if (failure == 1) actual.realtime_capabilities->maximum_block_frames = 512;
        if (failure == 2) actual.realtime_role = RealtimeRole::Sink;
        registry.register_realtime_type(descriptor,
            [actual, counts, failure](const ParameterMap&) -> std::unique_ptr<IRealtimeProcessor> {
                ++counts->factories;
                if (failure == 0) return {};
                return std::make_unique<HistoryNode>(actual, counts, Behavior::History);
            });
        require(rejects([&] { static_cast<void>(registry.create_realtime("history", {})); }).code ==
            "invalid_factory", "Null/mismatching realtime factory instance was accepted");
        require(counts->live == 0, "Rejected realtime factory retained its instance");
    }
    NodeRegistry registry;
    static_cast<void>(rejects([&] { registry.register_realtime_type(descriptor, {}); }));
    auto no_capabilities = descriptor;
    no_capabilities.realtime_capabilities.reset();
    const auto factory_count = counts->factories;
    static_cast<void>(rejects([&] {
        registry.register_realtime_type(no_capabilities,
            [descriptor, counts](const ParameterMap&) -> std::unique_ptr<IRealtimeProcessor> {
                ++counts->factories;
                return std::make_unique<HistoryNode>(descriptor, counts, Behavior::History);
            });
    }));
    require(counts->factories == factory_count, "Invalid descriptor invoked a factory");

    auto fixed_blocks = descriptor;
    fixed_blocks.realtime_capabilities->supports_variable_blocks = false;
    const auto fixed_registry = registry_with_history(counts, Behavior::History, fixed_blocks);
    static_cast<void>(rejects([&] { static_cast<void>(validate_realtime_graph(with_history(), fixed_registry)); }));
    require(counts->factories == factory_count, "Unsupported variable-block contract invoked a factory");
}

class TemporaryDirectory {
public:
    TemporaryDirectory() {
        const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
        for (int attempt = 0; attempt < 100; ++attempt) {
            auto candidate = std::filesystem::temp_directory_path() /
                ("audioprocess_rt_graph_" + std::to_string(stamp) + "_" + std::to_string(attempt));
            if (std::filesystem::create_directory(candidate)) { path = std::move(candidate); return; }
        }
        throw std::runtime_error("Cannot create test directory");
    }
    ~TemporaryDirectory() {
        std::error_code ignored;
        std::filesystem::remove_all(path, ignored); // 仅删除本测试成功原子创建的目录。
    }
    std::filesystem::path path;
};

void test_same_processor_plan_driven_from_file_blocks() {
    TemporaryDirectory directory;
    const auto file = directory.path / "input.wav";
    {
        AudioBuffer buffer({48000, 1}, 17);
        auto block = buffer.block(17);
        std::fill(block.samples.begin(), block.samples.end(), 0.125F);
        WavFileSink sink(file, {48000, 1}, 17);
        sink.write(block); sink.finalize();
    }
    auto counts = std::make_shared<Counters>();
    const auto registry = registry_with_history(counts);
    require(registry.descriptor("history").realtime_capabilities->offline_drivable,
        "File-driving test requires a processor that declares offline driving support");
    auto plan = RealtimeGraphExecutor::compile(with_history(), registry);
    for (const std::uint32_t frames : {1U, 3U, 7U, 16U}) {
        plan.prepare({48000, 1}, frames);
        WavFileSource source(file, frames);
        AudioBuffer buffer(source.format(), frames);
        std::vector<float> actual;
        while (auto block = source.read(buffer)) {
            require(plan.process(block->samples).status == RealtimeProcessStatus::Ok,
                "Prepared realtime processor rejected a valid file block");
            actual.insert(actual.end(), block->samples.begin(), block->samples.end());
        }
        require(actual.size() == 17, "File-driven processing changed frame count");
        for (std::size_t i = 0; i < actual.size(); ++i) {
            require(near(actual[i], 0.125F + static_cast<float>(i + 1) * 0.125F),
                "File block size changed node history or reset failed between files");
        }
    }
}

void test_callback_no_new_allocations() {
    auto counts = std::make_shared<Counters>();
    auto plan = RealtimeGraphExecutor::compile(with_history(0.0F), registry_with_history(counts));
    plan.prepare({48000, 1}, 256);
    std::array<float, 8> samples{};
    const auto before = allocation_probe::count;
    bool okay = true;
    allocation_probe::enabled = true;
    for (int iteration = 0; iteration < 2000; ++iteration) {
        samples.fill(0.125F);
        if (plan.process(samples).status != RealtimeProcessStatus::Ok) okay = false;
    }
    allocation_probe::enabled = false;
    require(okay && allocation_probe::count == before,
        "Realtime plan/registered third-party processor allocated through C++ new");

    auto broken = RealtimeGraphExecutor::compile(with_history(), registry_with_history(counts, Behavior::NonFiniteOutput));
    broken.prepare({48000, 1}, 256);
    const auto before_failure = allocation_probe::count;
    allocation_probe::enabled = true;
    const auto failure = broken.process(samples);
    allocation_probe::enabled = false;
    require(failure.status == RealtimeProcessStatus::NonFiniteOutput &&
        allocation_probe::count == before_failure, "Callback error reporting allocated memory");
}

} // namespace

int main() {
    try {
        test_identity_and_gain_arithmetic();
        test_extension_order_state_and_reprepare();
        test_validation_before_factory();
        test_prepare_failure_and_callback_fault_location();
        test_per_node_capacity_and_failed_reprepare();
        test_registry_rejects_incompatible_realtime_contracts();
        test_same_processor_plan_driven_from_file_blocks();
        test_callback_no_new_allocations();
        std::cout << "Independent realtime Graph tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        allocation_probe::enabled = false;
        std::cerr << "Realtime Graph test failure: " << error.what() << '\n';
        return 1;
    }
}
