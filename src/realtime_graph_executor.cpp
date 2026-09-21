#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/detail/json_pointer.h"
#include "audioprocess/execution_error.h"

#include <algorithm>
#include <cmath>
#include <unordered_map>
#include <utility>

namespace audioprocess {
namespace {

const PortDescriptor& find_port(const std::vector<PortDescriptor>& ports, const std::string& id,
                                const std::string& node, const std::string& field) {
    const auto port = std::ranges::find(ports, id, &PortDescriptor::id);
    if (port == ports.end()) throw ExecutionError("unknown_port", "Unknown realtime graph port", node, id, {}, field);
    return *port;
}

bool all_finite(std::span<const float> samples) noexcept {
    for (const float sample : samples) if (!std::isfinite(sample)) return false;
    return true;
}

} // namespace

GraphValidationResult validate_realtime_graph(const GraphDefinition& graph, const NodeRegistry& registry,
                                              AudioFormat format, std::uint32_t maximum_frames) {
    if (format != AudioFormat{48000, 1}) {
        throw ExecutionError("unsupported_realtime_format", "Realtime plan currently requires 48kHz mono");
    }
    if (maximum_frames == 0 || maximum_frames > 65536) {
        throw ExecutionError("invalid_block_size", "Realtime maximum block frames must be between 1 and 65536");
    }
    if (graph.schema_version != 1) throw ExecutionError("unsupported_schema_version", "Only graph schema 1 is supported", {}, {}, {}, "/schema_version");
    if (graph.nodes.size() < 2 || graph.nodes.size() > 128) {
        throw ExecutionError("invalid_realtime_graph", "Realtime graph requires 2 to 128 nodes", {}, {}, {}, "/nodes");
    }
    if (!graph.exports.empty()) throw ExecutionError("invalid_export", "Realtime audio streams do not expose offline Graph exports", {}, {}, {}, "/exports");
    GraphValidationResult result{graph, {}};
    std::unordered_map<std::string, std::size_t> indices;
    std::vector<const NodeDescriptor*> descriptors;
    std::optional<std::size_t> source, sink;
    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        auto& node = result.graph.nodes[index];
        const auto field = "/nodes/" + std::to_string(index);
        if (node.id.empty() || !indices.emplace(node.id, index).second) {
            throw ExecutionError("invalid_node_id", "Graph node ids must be nonempty and unique", node.id, {}, {}, field + "/id");
        }
        try {
            const auto& descriptor = registry.descriptor(node.type_id);
            if (descriptor.execution_domain != ExecutionDomain::Realtime ||
                descriptor.realtime_role == RealtimeRole::None || !descriptor.realtime_capabilities) {
                throw ExecutionError("unsupported_execution_domain", "Every node must declare the realtime execution contract");
            }
            const auto& capabilities = *descriptor.realtime_capabilities;
            if (capabilities.format != format || capabilities.maximum_block_frames < maximum_frames ||
                !capabilities.supports_variable_blocks) {
                throw ExecutionError("unsupported_realtime_capability", "Node does not support the requested format, block size or variable blocks");
            }
            node.parameters = registry.normalize_parameters(node.type_id, node.parameters);
            descriptors.push_back(&descriptor);
        } catch (const ExecutionError& error) {
            const auto location = error.parameter_id.empty() && error.code != "unknown_parameter"
                ? field + "/type" : detail::json_pointer_append(field + "/parameters", error.parameter_id);
            throw ExecutionError(error.code, error.what(), node.id, error.port_id, error.parameter_id, location);
        }
        const auto role = descriptors.back()->realtime_role;
        if (role == RealtimeRole::Source || role == RealtimeRole::Sink) {
            const auto& device = std::get<std::string>(node.parameters.at("device_id"));
            if (device.empty() || device.find('\0') != std::string::npos) {
                throw ExecutionError("invalid_device_id", "Device binding must be nonempty and contain no NUL", node.id, {}, "device_id", field + "/parameters/device_id");
            }
            auto& slot = role == RealtimeRole::Source ? source : sink;
            if (slot) throw ExecutionError("invalid_realtime_graph", "Realtime graph needs exactly one source and one sink", node.id, {}, {}, field);
            slot = index;
        }
    }
    if (!source || !sink) throw ExecutionError("invalid_realtime_graph", "Realtime graph needs one source and one sink", {}, {}, {}, "/nodes");
    std::vector<std::optional<std::size_t>> next(graph.nodes.size());
    std::vector<bool> has_input(graph.nodes.size(), false);
    for (std::size_t index = 0; index < graph.connections.size(); ++index) {
        const auto& connection = graph.connections[index];
        const auto field = "/connections/" + std::to_string(index);
        const auto from = indices.find(connection.source_node);
        const auto to = indices.find(connection.target_node);
        if (from == indices.end()) throw ExecutionError("unknown_node", "Unknown source node", connection.source_node, {}, {}, field + "/from/node");
        if (to == indices.end()) throw ExecutionError("unknown_node", "Unknown target node", connection.target_node, {}, {}, field + "/to/node");
        const auto& output = find_port(descriptors[from->second]->outputs, connection.source_port, connection.source_node, field + "/from/port");
        const auto& input = find_port(descriptors[to->second]->inputs, connection.target_port, connection.target_node, field + "/to/port");
        if (output.type != DataType::AudioStream || input.type != DataType::AudioStream) {
            throw ExecutionError("port_type_mismatch", "Realtime connections must carry AudioStream", connection.target_node, connection.target_port, {}, field);
        }
        if (next[from->second] || has_input[to->second]) {
            throw ExecutionError("invalid_realtime_graph", "Realtime branching and merging are not supported", connection.target_node, connection.target_port, {}, field);
        }
        next[from->second] = to->second;
        has_input[to->second] = true;
    }
    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        if ((index != *source && !has_input[index]) || (index != *sink && !next[index])) {
            throw ExecutionError("missing_input", "All nodes must belong to the realtime source-to-sink chain", graph.nodes[index].id, {}, {}, "/connections");
        }
    }
    std::vector<bool> visited(graph.nodes.size(), false);
    auto cursor = source;
    while (cursor) {
        if (visited[*cursor]) throw ExecutionError("graph_cycle", "Realtime graph contains a cycle", graph.nodes[*cursor].id, {}, {}, "/connections");
        visited[*cursor] = true;
        result.topological_order.push_back(*cursor);
        cursor = next[*cursor];
    }
    if (result.topological_order.size() != graph.nodes.size() || result.topological_order.back() != *sink) {
        throw ExecutionError("invalid_realtime_graph", "Realtime graph has a disconnected component or cycle", {}, {}, {}, "/connections");
    }
    return result;
}

RealtimeGraphExecutor RealtimeGraphExecutor::compile(const GraphDefinition& graph, const NodeRegistry& registry) {
    // compile 不预先选定回调块预算；支持变长块的节点至少须接受 1 帧。
    // 实际 maximum_frames 在 prepare 中检查，允许仅支持小块的处理器注册使用。
    auto validated = validate_realtime_graph(graph, registry, {48000, 1}, 1);
    RealtimeGraphExecutor executor;
    executor.registry_ = registry;
    executor.graph_ = std::move(validated.graph);
    executor.order_ = std::move(validated.topological_order);
    executor.input_device_ = std::get<std::string>(executor.graph_.nodes[executor.order_.front()].parameters.at("device_id"));
    executor.output_device_ = std::get<std::string>(executor.graph_.nodes[executor.order_.back()].parameters.at("device_id"));
    return executor;
}

void RealtimeGraphExecutor::prepare(AudioFormat format, std::uint32_t maximum_frames) {
    // 请求格式与能力在任何工厂调用前统一验证；失败不会动到原先的有效计划。
    (void)validate_realtime_graph(graph_, registry_, format, maximum_frames);
    std::vector<PreparedProcessor> prepared;
    prepared.reserve(order_.size() - 2);
    for (std::size_t index = 1; index + 1 < order_.size(); ++index) {
        const auto& node = graph_.nodes[order_[index]];
        try {
            auto instance = registry_.create_realtime(node.type_id, node.parameters);
            instance->prepare(format, maximum_frames);
            prepared.push_back({node.id, std::move(instance)});
        } catch (const ExecutionError& error) {
            throw ExecutionError(error.code, error.what(), node.id, error.port_id, error.parameter_id, error.field_path);
        } catch (const std::exception& error) {
            throw ExecutionError("node_prepare_failed", error.what(), node.id);
        } catch (...) {
            throw ExecutionError("node_prepare_failed", "Realtime node prepare threw an unknown exception", node.id);
        }
    }
    processors_ = std::move(prepared);
    maximum_frames_ = maximum_frames;
    prepared_ = true;
}

RealtimeProcessResult RealtimeGraphExecutor::process(std::span<float> samples) noexcept {
    if (!prepared_) return {RealtimeProcessStatus::NotPrepared};
    if (samples.empty() || samples.size() > maximum_frames_) return {RealtimeProcessStatus::InvalidBlock};
    if (!all_finite(samples)) return {RealtimeProcessStatus::NonFiniteInput};
    for (std::uint32_t index = 0; index < processors_.size(); ++index) {
        const auto status = processors_[index].instance->process(samples);
        if (status != RealtimeProcessStatus::Ok) {
            switch (status) {
            case RealtimeProcessStatus::NotPrepared: case RealtimeProcessStatus::InvalidBlock:
            case RealtimeProcessStatus::NonFiniteInput: case RealtimeProcessStatus::NodeFailed:
            case RealtimeProcessStatus::NonFiniteOutput: return {status, index};
            default: return {RealtimeProcessStatus::NodeFailed, index};
            }
        }
        if (!all_finite(samples)) return {RealtimeProcessStatus::NonFiniteOutput, index};
    }
    return {};
}

std::string_view RealtimeGraphExecutor::node_id(std::uint32_t index) const noexcept {
    return index < processors_.size() ? std::string_view{processors_[index].id} : std::string_view{};
}

} // namespace audioprocess
