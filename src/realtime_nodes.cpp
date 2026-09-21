#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/execution_error.h"

#include <cmath>
#include <utility>

namespace audioprocess {
namespace {

NodeDescriptor endpoint_descriptor(const std::string& type, RealtimeRole role) {
    NodeDescriptor descriptor;
    descriptor.type_id = type;
    descriptor.display_name = role == RealtimeRole::Source ? "Realtime Device Input" : "Realtime Device Output";
    descriptor.description = "Explicit device binding; opened by the control-thread session, not a node factory.";
    descriptor.execution_domain = ExecutionDomain::Realtime;
    descriptor.realtime_role = role;
    descriptor.realtime_capabilities = RealtimeCapabilities{};
    descriptor.realtime_capabilities->offline_drivable = false;
    descriptor.parameters = {{"device_id", ParameterType::Text, "Explicit WASAPI endpoint ID.", true}};
    if (role == RealtimeRole::Source) descriptor.outputs = {{"audio", DataType::AudioStream, true}};
    else descriptor.inputs = {{"audio", DataType::AudioStream, true}};
    return descriptor;
}

class RealtimeGainNode final : public IRealtimeProcessor {
public:
    explicit RealtimeGainNode(const ParameterMap& parameters)
        : gain_(static_cast<float>(std::pow(10.0, std::get<double>(parameters.at("gain_db")) / 20.0))) {}

    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    void prepare(AudioFormat format, std::uint32_t maximum_frames) override {
        if (format != AudioFormat{48000, 1} || maximum_frames == 0 || maximum_frames > 65536) {
            throw ExecutionError("unsupported_realtime_format", "Realtime Gain requires 48kHz mono and 1..65536 maximum frames");
        }
        maximum_frames_ = maximum_frames;
    }
    RealtimeProcessStatus process(std::span<float> samples) noexcept override {
        if (maximum_frames_ == 0) return RealtimeProcessStatus::NotPrepared;
        if (samples.empty() || samples.size() > maximum_frames_) return RealtimeProcessStatus::InvalidBlock;
        for (float& sample : samples) {
            if (!std::isfinite(sample)) return RealtimeProcessStatus::NonFiniteInput;
            sample *= gain_;
            if (!std::isfinite(sample)) return RealtimeProcessStatus::NonFiniteOutput;
        }
        return RealtimeProcessStatus::Ok;
    }
    static NodeDescriptor make_descriptor() {
        NodeDescriptor descriptor;
        descriptor.type_id = "realtime_gain";
        descriptor.display_name = "Realtime Gain";
        descriptor.description = "Prepared, static in-place gain; bounded noexcept processing, no callback allocation.";
        descriptor.execution_domain = ExecutionDomain::Realtime;
        descriptor.inputs = {{"audio", DataType::AudioStream, true}};
        descriptor.outputs = {{"audio", DataType::AudioStream, true}};
        descriptor.parameters = {{"gain_db", ParameterType::Number, "Static gain in decibels.", false,
            ParameterValue{0.0}, -24.0, 12.0, "dB", {}}};
        descriptor.realtime_role = RealtimeRole::Processor;
        descriptor.realtime_capabilities = RealtimeCapabilities{};
        return descriptor;
    }
private:
    float gain_{};
    std::uint32_t maximum_frames_{};
    NodeDescriptor descriptor_{make_descriptor()};
};

} // namespace

void register_realtime_nodes(NodeRegistry& registry) {
    registry.register_realtime_endpoint(endpoint_descriptor("realtime_input", RealtimeRole::Source));
    registry.register_realtime_type(RealtimeGainNode::make_descriptor(), [](const ParameterMap& parameters) {
        return std::make_unique<RealtimeGainNode>(parameters);
    });
    registry.register_realtime_endpoint(endpoint_descriptor("realtime_output", RealtimeRole::Sink));
}

GraphDefinition make_realtime_graph(const std::string& input_device, const std::string& output_device,
                                    std::optional<float> gain_db) {
    GraphDefinition graph;
    graph.nodes.push_back({"input", "realtime_input", {{"device_id", input_device}}});
    if (gain_db) graph.nodes.push_back({"gain", "realtime_gain", {{"gain_db", static_cast<double>(*gain_db)}}});
    graph.nodes.push_back({"output", "realtime_output", {{"device_id", output_device}}});
    if (gain_db) {
        graph.connections = {{"input", "audio", "gain", "audio"}, {"gain", "audio", "output", "audio"}};
    } else {
        graph.connections = {{"input", "audio", "output", "audio"}};
    }
    return graph;
}

} // namespace audioprocess
