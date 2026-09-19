#include "audioprocess/sync_graph_executor.h"

#include <algorithm>
#include <queue>
#include <stdexcept>
#include <unordered_set>

namespace audioprocess {
namespace {

const PortDescriptor& find_port(
    const std::vector<PortDescriptor>& ports,
    const std::string& port_id,
    const std::string& node_id) {
    const auto iterator = std::ranges::find(ports, port_id, &PortDescriptor::id);
    if (iterator == ports.end()) {
        throw std::invalid_argument(
            "Node '" + node_id + "' does not have port '" + port_id + "'");
    }
    return *iterator;
}

std::string input_key(const std::string& node_id, const std::string& port_id) {
    return node_id + "\n" + port_id;
}

}  // namespace

const DataValue& GraphExecutionResult::value(
    const std::string& node_id,
    const std::string& port_id) const {
    const auto node = node_outputs_.find(node_id);
    if (node == node_outputs_.end()) {
        throw std::out_of_range("No result for node: " + node_id);
    }
    const auto port = node->second.find(port_id);
    if (port == node->second.end()) {
        throw std::out_of_range("No result for port: " + node_id + "." + port_id);
    }
    return port->second;
}

SyncGraphExecutor SyncGraphExecutor::compile(
    const GraphDefinition& graph,
    const NodeRegistry& registry) {
    if (graph.nodes.empty()) {
        throw std::invalid_argument("Graph must contain at least one node");
    }

    std::unordered_map<std::string, std::size_t> node_indices;
    std::vector<NodeDescriptor> descriptors;
    descriptors.reserve(graph.nodes.size());

    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        const auto& definition = graph.nodes[index];
        if (definition.id.empty()) {
            throw std::invalid_argument("Graph node id cannot be empty");
        }
        if (!node_indices.emplace(definition.id, index).second) {
            throw std::invalid_argument("Duplicate graph node id: " + definition.id);
        }

        const auto& descriptor = registry.descriptor(definition.type_id);
        if (descriptor.execution_domain != ExecutionDomain::Synchronous) {
            throw std::invalid_argument(
                "SyncGraphExecutor cannot run node type: " + definition.type_id);
        }
        descriptors.push_back(descriptor);
    }

    std::vector<std::size_t> indegree(graph.nodes.size(), 0);
    std::vector<std::vector<std::size_t>> outgoing(graph.nodes.size());
    std::unordered_set<std::string> connected_inputs;

    for (const auto& connection : graph.connections) {
        const auto source = node_indices.find(connection.source_node);
        const auto target = node_indices.find(connection.target_node);
        if (source == node_indices.end() || target == node_indices.end()) {
            throw std::invalid_argument("Connection references an unknown node");
        }

        const auto& source_port = find_port(
            descriptors[source->second].outputs,
            connection.source_port,
            connection.source_node);
        const auto& target_port = find_port(
            descriptors[target->second].inputs,
            connection.target_port,
            connection.target_node);

        if (source_port.type != target_port.type) {
            throw std::invalid_argument(
                "Port type mismatch: " + connection.source_node + "." +
                connection.source_port + " (" + data_type_name(source_port.type) +
                ") -> " + connection.target_node + "." + connection.target_port +
                " (" + data_type_name(target_port.type) + ")");
        }

        if (!connected_inputs.insert(
                input_key(connection.target_node, connection.target_port)).second) {
            throw std::invalid_argument(
                "Input port has more than one connection: " +
                connection.target_node + "." + connection.target_port);
        }

        outgoing[source->second].push_back(target->second);
        ++indegree[target->second];
    }

    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        for (const auto& input : descriptors[index].inputs) {
            if (input.required && !connected_inputs.contains(
                    input_key(graph.nodes[index].id, input.id))) {
                throw std::invalid_argument(
                    "Required input is not connected: " + graph.nodes[index].id +
                    "." + input.id);
            }
        }
    }

    std::queue<std::size_t> ready;
    for (std::size_t index = 0; index < indegree.size(); ++index) {
        if (indegree[index] == 0) {
            ready.push(index);
        }
    }

    std::vector<std::size_t> order;
    order.reserve(graph.nodes.size());
    while (!ready.empty()) {
        const auto index = ready.front();
        ready.pop();
        order.push_back(index);
        for (const auto target : outgoing[index]) {
            if (--indegree[target] == 0) {
                ready.push(target);
            }
        }
    }

    if (order.size() != graph.nodes.size()) {
        throw std::invalid_argument("Graph contains a cycle");
    }

    SyncGraphExecutor executor;
    executor.execution_plan_.reserve(order.size());
    for (const auto index : order) {
        const auto& definition = graph.nodes[index];
        CompiledNode compiled{
            definition.id,
            descriptors[index],
            registry.create(definition.type_id, definition.parameters),
            {},
        };
        for (const auto& connection : graph.connections) {
            if (connection.target_node == definition.id) {
                compiled.inbound.push_back(InboundConnection{
                    connection.target_port,
                    connection.source_node,
                    connection.source_port,
                });
            }
        }
        executor.execution_plan_.push_back(std::move(compiled));
    }
    return executor;
}

GraphExecutionResult SyncGraphExecutor::execute(ExecutionContext context) {
    GraphExecutionResult result;

    for (auto& compiled : execution_plan_) {
        if (context.cancelled()) {
            throw std::runtime_error("Graph execution was cancelled");
        }

        InputValues inputs;
        for (const auto& inbound : compiled.inbound) {
            const auto source = result.node_outputs_.find(inbound.source_node);
            if (source == result.node_outputs_.end()) {
                throw std::logic_error("Execution plan referenced unavailable node output");
            }
            const auto value = source->second.find(inbound.source_port);
            if (value == source->second.end()) {
                throw std::logic_error("Execution plan referenced unavailable port output");
            }
            inputs.emplace(inbound.target_port, value->second);
        }

        auto outputs = compiled.instance->execute(inputs, context);
        for (const auto& output_port : compiled.descriptor.outputs) {
            const auto value = outputs.find(output_port.id);
            if (value == outputs.end()) {
                throw std::runtime_error(
                    "Node did not produce required output: " + compiled.id + "." +
                    output_port.id);
            }
            if (data_type_of(value->second) != output_port.type) {
                throw std::runtime_error(
                    "Node produced the wrong type for output: " + compiled.id + "." +
                    output_port.id);
            }
        }
        result.node_outputs_.emplace(compiled.id, std::move(outputs));
    }

    return result;
}

}  // namespace audioprocess

