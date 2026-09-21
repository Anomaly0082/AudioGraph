#include "audioprocess/realtime_session.h"
#include "audioprocess/execution_error.h"

#include <iostream>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
using namespace audioprocess;

void require(bool condition, const char* message) {
    if (!condition) { throw std::runtime_error(message); }
}

struct Counts {
    int factories{};
    int live{};
    int preparations{};
    int destructions{};
};

class PreparationProbe final : public IRealtimeProcessor {
public:
    PreparationProbe(NodeDescriptor descriptor, std::shared_ptr<Counts> counts, bool fail)
        : descriptor_(std::move(descriptor)), counts_(std::move(counts)), fail_(fail) {
        ++counts_->live;
    }
    ~PreparationProbe() override { --counts_->live; ++counts_->destructions; }
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    void prepare(AudioFormat, std::uint32_t maximum_frames) override {
        ++counts_->preparations;
        storage_.resize(maximum_frames, 0.0F);
        if (fail_) { throw std::runtime_error("intentional prepare failure before device access"); }
    }
    RealtimeProcessStatus process(std::span<float>) noexcept override {
        return RealtimeProcessStatus::Ok;
    }
private:
    NodeDescriptor descriptor_;
    std::shared_ptr<Counts> counts_;
    bool fail_{};
    std::vector<float> storage_;
};

NodeRegistry make_registry(const std::shared_ptr<Counts>& counts) {
    NodeRegistry registry;
    register_realtime_nodes(registry);
    for (const bool fail : {false, true}) {
        NodeDescriptor descriptor;
        descriptor.type_id = fail ? "prepare_failure" : "prepare_success";
        descriptor.display_name = descriptor.type_id;
        descriptor.execution_domain = ExecutionDomain::Realtime;
        descriptor.realtime_role = RealtimeRole::Processor;
        descriptor.realtime_capabilities = RealtimeCapabilities{};
        descriptor.inputs = {{"audio", DataType::AudioStream, true}};
        descriptor.outputs = {{"audio", DataType::AudioStream, true}};
        registry.register_realtime_type(descriptor, [descriptor, counts, fail](const ParameterMap&) {
            ++counts->factories;
            return std::make_unique<PreparationProbe>(descriptor, counts, fail);
        });
    }
    return registry;
}

GraphDefinition make_failing_graph() {
    return {{{"input", "realtime_input", {{"device_id", std::string("must-not-open-capture")}}},
             {"first", "prepare_success", {}},
             {"reject_prepare", "prepare_failure", {}},
             {"output", "realtime_output", {{"device_id", std::string("must-not-open-playback")}}}},
            {{"input", "audio", "first", "audio"},
             {"first", "audio", "reject_prepare", "audio"},
             {"reject_prepare", "audio", "output", "audio"}}};
}

void test_prepare_failure_releases_all_nodes_without_devices() {
    auto counts = std::make_shared<Counts>();
    const auto registry = make_registry(counts);
    const auto graph = make_failing_graph();
    RealtimeSession session;
    for (int task = 1; task <= 2; ++task) {
        bool failed_in_prepare = false;
        try { session.start(graph, registry); }
        catch (const ExecutionError& error) {
            failed_in_prepare = error.code == "node_prepare_failed" && error.node_id == "reject_prepare";
        }
        require(failed_in_prepare, "Session did not reject processor preparation before opening fake devices");
        require(counts->factories == task * 2 && counts->preparations == task * 2 &&
                counts->live == 0 && counts->destructions == task * 2,
                "Failed startup retained the failed or already-prepared processor instance");
        require(!session.is_running() && !session.faulted(), "Failed preparation left an active device session");
        require(session.snapshot().capture_frames == 0 && session.snapshot().render_frames == 0,
                "Failed preparation unexpectedly processed device frames");
        session.stop();
        session.stop();
    }
}

void test_validation_precedes_factories_and_devices() {
    auto counts = std::make_shared<Counts>();
    const auto registry = make_registry(counts);
    auto graph = make_failing_graph();
    graph.connections[0].source_port = "missing";
    RealtimeSession session;
    bool bad_port = false;
    try { session.start(graph, registry); }
    catch (const ExecutionError& error) { bad_port = error.code == "unknown_port"; }
    require(bad_port && counts->factories == 0, "Invalid topology invoked processor factories");

    const auto valid_topology = make_failing_graph();
    for (const auto size : {0U, 65537U}) {
        RealtimeSessionConfig config;
        config.graph_block_frames = size;
        bool bad_budget = false;
        try { session.start(valid_topology, registry, config); }
        catch (const ExecutionError& error) { bad_budget = error.code == "invalid_block_size"; }
        require(bad_budget && counts->factories == 0, "Invalid graph block budget reached node preparation");
    }
    session.stop();
}
} // namespace

int main() {
    try {
        test_prepare_failure_releases_all_nodes_without_devices();
        test_validation_precedes_factories_and_devices();
        std::cout << "Realtime Session pre-device boundary tests passed (no hardware access).\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
