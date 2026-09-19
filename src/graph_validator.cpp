#include "audioprocess/graph_validator.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/detail/json_pointer.h"

#include <algorithm>
#include <queue>
#include <unordered_map>
#include <unordered_set>

namespace audioprocess {
namespace {

const PortDescriptor& find_port(const std::vector<PortDescriptor>& ports,
                                const std::string& port_id, const std::string& node_id,
                                const std::string& field) {
    const auto found = std::ranges::find(ports, port_id, &PortDescriptor::id);
    if (found == ports.end()) {
        throw ExecutionError("unknown_port", "Unknown port: " + node_id + "." + port_id,
                             node_id, port_id, {}, field);
    }
    return *found;
}

}  // namespace

GraphValidationResult validate_graph(const GraphDefinition& graph, const NodeRegistry& registry) {
    if (graph.schema_version != 1) {
        throw ExecutionError("unsupported_schema_version", "Only graph schema version 1 is supported",
                             {}, {}, {}, "/schema_version");
    }
    if (graph.nodes.empty()) {
        throw ExecutionError("empty_graph", "Graph must contain at least one node", {}, {}, {}, "/nodes");
    }
    GraphValidationResult result{graph, {}};
    std::unordered_map<std::string, std::size_t> indices;
    std::vector<const NodeDescriptor*> descriptors;
    descriptors.reserve(graph.nodes.size());
    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        auto& definition = result.graph.nodes[index];
        const auto field = "/nodes/" + std::to_string(index);
        if (definition.id.empty() || !indices.emplace(definition.id, index).second) {
            throw ExecutionError("invalid_node_id", "Graph node ids must be nonempty and unique",
                                 definition.id, {}, {}, field + "/id");
        }
        try {
            const auto& descriptor = registry.descriptor(definition.type_id);
            if (descriptor.execution_domain != ExecutionDomain::Synchronous) {
                throw ExecutionError("unsupported_execution_domain", "This executor supports synchronous nodes only");
            }
            definition.parameters = registry.normalize_parameters(definition.type_id, definition.parameters);
            descriptors.push_back(&descriptor);
        } catch (const ExecutionError& error) {
            throw ExecutionError(error.code, error.what(), definition.id, error.port_id, error.parameter_id,
                                 (error.code == "unknown_node_type" || error.code == "unsupported_execution_domain")
                                     ? field + "/type"
                                     : detail::json_pointer_append(field + "/parameters", error.parameter_id));
        }
    }

    std::vector<std::size_t> indegree(graph.nodes.size(), 0);
    std::vector<std::vector<std::size_t>> outgoing(graph.nodes.size());
    // 使用每节点独立集合，避免拼接 ID 时分隔符碰撞。
    std::vector<std::unordered_set<std::string>> connected_inputs(graph.nodes.size());
    for (std::size_t index = 0; index < graph.connections.size(); ++index) {
        const auto& connection = graph.connections[index];
        const auto field = "/connections/" + std::to_string(index);
        const auto source = indices.find(connection.source_node);
        const auto target = indices.find(connection.target_node);
        if (source == indices.end()) {
            throw ExecutionError("unknown_node", "Connection references an unknown source node",
                                 connection.source_node, connection.source_port, {}, field + "/from/node");
        }
        if (target == indices.end()) {
            throw ExecutionError("unknown_node", "Connection references an unknown target node",
                                 connection.target_node, connection.target_port, {}, field + "/to/node");
        }
        const auto& output = find_port(descriptors[source->second]->outputs, connection.source_port,
                                       connection.source_node, field + "/from/port");
        const auto& input = find_port(descriptors[target->second]->inputs, connection.target_port,
                                      connection.target_node, field + "/to/port");
        if (output.type != input.type) {
            throw ExecutionError("port_type_mismatch", "Connection ports have different data types",
                                 connection.target_node, connection.target_port, {}, field);
        }
        if (!connected_inputs[target->second].insert(connection.target_port).second) {
            throw ExecutionError("duplicate_input_connection", "Input port has more than one connection",
                                 connection.target_node, connection.target_port, {}, field);
        }
        outgoing[source->second].push_back(target->second);
        ++indegree[target->second];
    }
    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        for (const auto& input : descriptors[index]->inputs) {
            if (input.required && !connected_inputs[index].contains(input.id)) {
                throw ExecutionError("missing_input", "Required input is not connected: " + input.id,
                                     graph.nodes[index].id, input.id, {}, "/connections");
            }
        }
    }

    std::unordered_set<std::string> export_names;
    for (std::size_t index = 0; index < graph.exports.size(); ++index) {
        const auto& output = graph.exports[index];
        const auto field = "/exports/" + std::to_string(index);
        if (output.name.empty() || !export_names.insert(output.name).second) {
            throw ExecutionError("invalid_export", "Export names must be nonempty and unique",
                                 output.node_id, output.port_id, {}, field + "/name");
        }
        const auto node = indices.find(output.node_id);
        if (node == indices.end()) {
            throw ExecutionError("unknown_node", "Export references an unknown node",
                                 output.node_id, output.port_id, {}, field + "/node");
        }
        (void)find_port(descriptors[node->second]->outputs, output.port_id, output.node_id, field + "/port");
    }

    std::queue<std::size_t> ready;
    for (std::size_t index = 0; index < indegree.size(); ++index) {
        if (indegree[index] == 0) ready.push(index);
    }
    while (!ready.empty()) {
        const auto index = ready.front();
        ready.pop();
        result.topological_order.push_back(index);
        for (const auto target : outgoing[index]) {
            if (--indegree[target] == 0) ready.push(target);
        }
    }
    if (result.topological_order.size() != graph.nodes.size()) {
        throw ExecutionError("graph_cycle", "Graph contains a cycle", {}, {}, {}, "/connections");
    }
    return result;
}

}  // namespace audioprocess
