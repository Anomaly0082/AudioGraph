#include "audioprocess/sync_graph_executor.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/graph_validator.h"

#include <algorithm>
#include <cmath>
#include <utility>

namespace audioprocess {
namespace {

// 类型匹配不等于数据有效：拒绝空音频引用、损坏格式和非有限值。
void validate_output_value(const DataValue& value, const std::string& node_id, const std::string& port_id) {
    const auto invalid = [&](const std::string& message) {
        throw ExecutionError("invalid_output", message, node_id, port_id);
    };
    if (const auto* number = std::get_if<double>(&value)) {
        if (!std::isfinite(*number)) invalid("Node produced a non-finite number");
    } else if (const auto* audio = std::get_if<AudioClipPtr>(&value)) {
        if (!*audio) invalid("Node produced a null audio clip");
        if (!(*audio)->format.valid() ||
            (*audio)->samples.size() % (*audio)->format.channel_count != 0) {
            invalid("Node produced an invalid audio format or sample count");
        }
        if (!std::ranges::all_of((*audio)->samples, [](float sample) { return std::isfinite(sample); })) {
            invalid("Node produced non-finite audio samples");
        }
    }
}

void check_cancelled(const ExecutionContext& context, const std::string& node_id) {
    if (context.cancelled()) {
        throw ExecutionError("cancelled", "Graph execution was cancelled", node_id);
    }
}

}  // namespace

const DataValue& GraphExecutionResult::value(const std::string& node_id, const std::string& port_id) const {
    const auto node = node_outputs_.find(node_id);
    if (node == node_outputs_.end()) {
        throw ExecutionError("missing_result", "No result for node: " + node_id, node_id, port_id);
    }
    const auto port = node->second.find(port_id);
    if (port == node->second.end()) {
        throw ExecutionError("missing_result", "No result for port: " + node_id + "." + port_id, node_id, port_id);
    }
    return port->second;
}

SyncGraphExecutor SyncGraphExecutor::compile(const GraphDefinition& graph, const NodeRegistry& registry) {
    const auto validated = validate_graph(graph, registry);
    SyncGraphExecutor executor;
    executor.registry_ = registry;
    executor.execution_plan_.reserve(validated.topological_order.size());
    for (const auto index : validated.topological_order) {
        const auto& definition = validated.graph.nodes[index];
        CompiledNode compiled{definition.id, registry.descriptor(definition.type_id),
                              definition.type_id, definition.parameters, {}};
        for (const auto& connection : graph.connections) {
            if (connection.target_node == definition.id) {
                compiled.inbound.push_back(InboundConnection{
                    connection.target_port, connection.source_node, connection.source_port});
            }
        }
        executor.execution_plan_.push_back(std::move(compiled));
    }
    return executor;
}

GraphExecutionResult SyncGraphExecutor::execute(ExecutionContext context) {
    GraphExecutionResult result;
    for (const auto& compiled : execution_plan_) {
        check_cancelled(context, compiled.id);
        InputValues inputs;
        for (const auto& inbound : compiled.inbound) {
            const auto source = result.node_outputs_.find(inbound.source_node);
            if (source == result.node_outputs_.end()) {
                throw ExecutionError("missing_input", "Required upstream node output is unavailable",
                                     compiled.id, inbound.target_port);
            }
            const auto value = source->second.find(inbound.source_port);
            if (value == source->second.end()) {
                const auto port = std::ranges::find(compiled.descriptor.inputs, inbound.target_port, &PortDescriptor::id);
                // 上游可以省略 optional 输出；仅当下游输入也 optional 时才能继续。
                if (port != compiled.descriptor.inputs.end() && !port->required) continue;
                throw ExecutionError("missing_input", "Required upstream port output is unavailable",
                                     compiled.id, inbound.target_port);
            }
            inputs.emplace(inbound.target_port, value->second);
        }

        OutputValues outputs;
        try {
            // 每次执行创建新实例，节点状态不会泄漏到下一次任务。
            auto instance = registry_.create(compiled.type_id, compiled.parameters);
            check_cancelled(context, compiled.id);
            outputs = instance->execute(inputs, context);
        } catch (const ExecutionError& error) {
            throw ExecutionError(error.code, "Node '" + compiled.id + "': " + error.what(),
                                 compiled.id, error.port_id, error.parameter_id, error.field_path);
        } catch (const std::exception& error) {
            throw ExecutionError("node_execution_failed", "Node '" + compiled.id + "': " + error.what(), compiled.id);
        } catch (...) {
            throw ExecutionError("node_execution_failed", "Node threw an unknown exception", compiled.id);
        }
        // 节点可能在执行中收到取消请求，包括图中最后一个节点。
        check_cancelled(context, compiled.id);
        for (const auto& [port_id, value] : outputs) {
            const auto port = std::ranges::find(compiled.descriptor.outputs, port_id, &PortDescriptor::id);
            if (port == compiled.descriptor.outputs.end()) {
                throw ExecutionError("unknown_output", "Node produced an undeclared output", compiled.id, port_id);
            }
            if (data_type_of(value) != port->type) {
                throw ExecutionError("output_type_mismatch", "Node produced the wrong output type", compiled.id, port_id);
            }
            validate_output_value(value, compiled.id, port_id);
        }
        for (const auto& output_port : compiled.descriptor.outputs) {
            if (output_port.required && !outputs.contains(output_port.id)) {
                throw ExecutionError("missing_output", "Node did not produce a required output", compiled.id, output_port.id);
            }
        }
        result.node_outputs_.emplace(compiled.id, std::move(outputs));
    }
    return result;
}

}  // namespace audioprocess
