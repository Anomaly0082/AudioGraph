#include "audioprocess/streaming_graph_executor.h"
#include "audioprocess/detail/json_pointer.h"
#include "audioprocess/execution_error.h"

#include <algorithm>
#include <cmath>
#include <limits>
#include <optional>
#include <unordered_map>
#include <unordered_set>
#include <utility>

namespace audioprocess {
namespace {

constexpr std::size_t kMaximumStreamNodes = 128;

const PortDescriptor& find_port(const std::vector<PortDescriptor>& ports, const std::string& id,
                                const std::string& node, const std::string& field) {
    const auto found = std::ranges::find(ports, id, &PortDescriptor::id);
    if (found == ports.end()) throw ExecutionError("unknown_port", "Unknown stream graph port", node, id, {}, field);
    return *found;
}

void check_cancelled(const ExecutionContext& context, const std::string& node) {
    if (context.cancelled()) throw ExecutionError("cancelled", "Stream execution was cancelled", node);
}

// 回调会嵌套进入下游节点；已定位的异常保留最初失败节点，不能被上游覆盖。
template<class Action>
decltype(auto) call_node(const std::string& id, ExecutionContext& context, Action&& action) {
    check_cancelled(context, id);
    try {
        return std::forward<Action>(action)();
    } catch (const ExecutionError& error) {
        if (!error.node_id.empty()) throw;
        throw ExecutionError(error.code, error.what(), id, error.port_id, error.parameter_id, error.field_path);
    } catch (const std::exception& error) {
        throw ExecutionError("node_execution_failed", "Node '" + id + "': " + error.what(), id);
    } catch (...) {
        throw ExecutionError("node_execution_failed", "Node threw an unknown exception", id);
    }
}

void validate_format(AudioFormat format, const std::string& node) {
    if (!format.valid()) throw ExecutionError("invalid_stream_format", "Stream node returned an invalid audio format", node);
}

void validate_block(const AudioStreamBlock& block, AudioFormat expected_format, std::uint32_t maximum_frames,
                    std::uint64_t& next_position, const std::string& node, const std::string& port) {
    if (block.frame_count == 0 || block.frame_count > maximum_frames ||
        block.channel_count != expected_format.channel_count ||
        block.frame_count > std::numeric_limits<std::size_t>::max() / expected_format.channel_count ||
        block.samples.size() != static_cast<std::size_t>(block.frame_count) * expected_format.channel_count) {
        throw ExecutionError("invalid_stream_block", "Stream block has invalid size or channels", node, port);
    }
    if (block.frame_position != next_position ||
        block.frame_count > std::numeric_limits<std::uint64_t>::max() - next_position) {
        throw ExecutionError("invalid_stream_position", "Stream output positions must be continuous from zero", node, port);
    }
    if (!std::ranges::all_of(block.samples, [](float sample) { return std::isfinite(sample); })) {
        throw ExecutionError("invalid_stream_block", "Stream block contains non-finite samples", node, port);
    }
    next_position += block.frame_count;
}

void validate_summary(const OutputValues& values, const NodeDescriptor& descriptor, const std::string& id) {
    for (const auto& [port_id, value] : values) {
        const auto port = std::ranges::find(descriptor.outputs, port_id, &PortDescriptor::id);
        if (port == descriptor.outputs.end()) throw ExecutionError("unknown_output", "Sink produced an undeclared summary", id, port_id);
        if (data_type_of(value) != port->type) throw ExecutionError("output_type_mismatch", "Sink summary type mismatch", id, port_id);
        if (const auto* number = std::get_if<double>(&value); number && !std::isfinite(*number)) {
            throw ExecutionError("invalid_output", "Sink summary must contain finite numbers", id, port_id);
        }
    }
    for (const auto& port : descriptor.outputs) {
        if (port.required && !values.contains(port.id)) throw ExecutionError("missing_output", "Sink omitted a required summary", id, port.id);
    }
}

}  // namespace

GraphValidationResult validate_stream_graph(const GraphDefinition& graph, const NodeRegistry& registry) {
    if (graph.schema_version != 1) throw ExecutionError("unsupported_schema_version", "Only schema version 1 is supported", {}, {}, {}, "/schema_version");
    if (graph.nodes.size() < 2 || graph.nodes.size() > kMaximumStreamNodes) {
        throw ExecutionError("invalid_stream_graph", "A stream graph requires 2 to 128 nodes", {}, {}, {}, "/nodes");
    }
    GraphValidationResult result{graph, {}};
    std::unordered_map<std::string, std::size_t> indices;
    std::vector<const NodeDescriptor*> descriptors;
    std::optional<std::size_t> source;
    std::optional<std::size_t> sink;
    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        auto& node = result.graph.nodes[index];
        const auto field = "/nodes/" + std::to_string(index);
        if (node.id.empty() || !indices.emplace(node.id, index).second) {
            throw ExecutionError("invalid_node_id", "Graph node ids must be nonempty and unique", node.id, {}, {}, field + "/id");
        }
        try {
            const auto& descriptor = registry.descriptor(node.type_id);
            if (descriptor.execution_domain != ExecutionDomain::Streaming || descriptor.stream_role == StreamRole::None) {
                throw ExecutionError("unsupported_execution_domain", "Stream graphs require registered streaming nodes");
            }
            node.parameters = registry.normalize_parameters(node.type_id, node.parameters);
            descriptors.push_back(&descriptor);
        } catch (const ExecutionError& error) {
            const auto location = (error.code == "unknown_node_type" || error.code == "unsupported_execution_domain")
                ? field + "/type" : detail::json_pointer_append(field + "/parameters", error.parameter_id);
            throw ExecutionError(error.code, error.what(), node.id, error.port_id, error.parameter_id, location);
        }
        if (descriptors.back()->stream_role == StreamRole::Source) {
            if (source) throw ExecutionError("invalid_stream_graph", "Stream graph has multiple sources", node.id, {}, {}, field);
            source = index;
        } else if (descriptors.back()->stream_role == StreamRole::Sink) {
            if (sink) throw ExecutionError("invalid_stream_graph", "Stream graph has multiple sinks", node.id, {}, {}, field);
            sink = index;
        }
    }
    if (!source || !sink) throw ExecutionError("invalid_stream_graph", "Stream graph needs one source and one sink", {}, {}, {}, "/nodes");

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
            throw ExecutionError("port_type_mismatch", "Only AudioStream connections are allowed in a stream graph", connection.target_node, connection.target_port, {}, field);
        }
        if (next[from->second] || has_input[to->second]) {
            throw ExecutionError("invalid_stream_graph", "Branching and merging are not supported by the linear stream executor",
                                 connection.target_node, connection.target_port, {}, field);
        }
        next[from->second] = to->second;
        has_input[to->second] = true;
    }
    // 每个非源节点有输入、每个非终点节点有输出，随后遍历排除孤立环。
    for (std::size_t index = 0; index < graph.nodes.size(); ++index) {
        if ((index != *source && !has_input[index]) || (index != *sink && !next[index])) {
            throw ExecutionError("missing_input", "Every stream node must belong to the source-to-sink chain", graph.nodes[index].id, {}, {}, "/connections");
        }
    }
    std::vector<bool> visited(graph.nodes.size(), false);
    auto cursor = source;
    while (cursor) {
        if (visited[*cursor]) throw ExecutionError("graph_cycle", "Stream graph contains a cycle", graph.nodes[*cursor].id, {}, {}, "/connections");
        visited[*cursor] = true;
        result.topological_order.push_back(*cursor);
        cursor = next[*cursor];
    }
    if (result.topological_order.size() != graph.nodes.size() || result.topological_order.back() != *sink) {
        throw ExecutionError("invalid_stream_graph", "Stream graph contains a disconnected component or cycle", {}, {}, {}, "/connections");
    }
    std::unordered_set<std::string> export_names;
    for (std::size_t index = 0; index < graph.exports.size(); ++index) {
        const auto& item = graph.exports[index];
        const auto field = "/exports/" + std::to_string(index);
        if (item.name.empty() || !export_names.insert(item.name).second) {
            throw ExecutionError("invalid_export", "Export names must be nonempty and unique", item.node_id, item.port_id, {}, field + "/name");
        }
        if (item.node_id != graph.nodes[*sink].id) {
            throw ExecutionError("invalid_export", "Only sink summaries may be exported from a stream graph", item.node_id, item.port_id, {}, field + "/node");
        }
        (void)find_port(descriptors[*sink]->outputs, item.port_id, item.node_id, field + "/port");
    }
    return result;
}

StreamingGraphExecutor StreamingGraphExecutor::compile(const GraphDefinition& graph, const NodeRegistry& registry) {
    auto validated = validate_stream_graph(graph, registry);
    StreamingGraphExecutor executor;
    executor.registry_ = registry;
    executor.graph_ = std::move(validated.graph);
    executor.order_ = std::move(validated.topological_order);
    return executor;
}

GraphExecutionResult StreamingGraphExecutor::execute(std::uint32_t maximum_frames, ExecutionContext context) {
    if (maximum_frames == 0 || maximum_frames > 65536) {
        throw ExecutionError("invalid_block_size", "Maximum stream block frames must be between 1 and 65536");
    }
    // 节点实例归本次执行拥有；失败或取消时 RAII 释放，不调用成功结束方法。
    std::vector<std::unique_ptr<IStreamNode>> instances;
    instances.reserve(order_.size());
    for (const auto index : order_) {
        const auto& definition = graph_.nodes[index];
        instances.push_back(call_node(definition.id, context, [&] {
            return registry_.create_stream(definition.type_id, definition.parameters);
        }));
        check_cancelled(context, definition.id);
    }
    const auto id_at = [&](std::size_t position) -> const std::string& { return graph_.nodes[order_[position]].id; };
    auto& source = *dynamic_cast<IAudioStreamSource*>(instances.front().get());
    auto& sink = *dynamic_cast<IAudioStreamSink*>(instances.back().get());
    std::vector<IAudioStreamProcessor*> processors(instances.size(), nullptr);
    std::vector<AudioFormat> output_formats(instances.size());
    output_formats[0] = call_node(id_at(0), context, [&] { return source.open(maximum_frames, context); });
    check_cancelled(context, id_at(0));
    validate_format(output_formats[0], id_at(0));
    for (std::size_t index = 1; index + 1 < instances.size(); ++index) {
        processors[index] = dynamic_cast<IAudioStreamProcessor*>(instances[index].get());
        output_formats[index] = call_node(id_at(index), context, [&] {
            return processors[index]->prepare(output_formats[index - 1], maximum_frames, context);
        });
        check_cancelled(context, id_at(index));
        validate_format(output_formats[index], id_at(index));
    }
    const auto sink_index = instances.size() - 1;
    // 最后准备 Sink：文件创建前，源格式与所有 Processor 格式均已协商成功。
    call_node(id_at(sink_index), context, [&] { sink.prepare(output_formats[sink_index - 1], maximum_frames, context); });
    check_cancelled(context, id_at(sink_index));

    std::vector<std::uint64_t> next_positions(instances.size(), 0);
    std::vector<StreamEmit> emitters(sink_index);
    for (std::size_t index = 0; index < sink_index; ++index) {
        emitters[index] = [&, index](const AudioStreamBlock& block) {
            check_cancelled(context, id_at(index));
            const auto& descriptor = instances[index]->descriptor();
            validate_block(block, output_formats[index], maximum_frames, next_positions[index], id_at(index), descriptor.outputs[0].id);
            const auto downstream = index + 1;
            if (downstream == sink_index) {
                call_node(id_at(downstream), context, [&] { sink.push(block, context); });
            } else {
                call_node(id_at(downstream), context, [&] { processors[downstream]->push(block, emitters[downstream], context); });
            }
            check_cancelled(context, id_at(downstream));
        };
    }
    while (true) {
        const auto block = call_node(id_at(0), context, [&] { return source.read(context); });
        check_cancelled(context, id_at(0));
        if (!block) break;
        emitters[0](*block);
    }
    // 先排空上游，尾部仍按普通 push 送入尚未 finish 的下游，再逐级结束。
    for (std::size_t index = 1; index < sink_index; ++index) {
        call_node(id_at(index), context, [&] { processors[index]->finish(emitters[index], context); });
        check_cancelled(context, id_at(index));
    }
    auto summary = call_node(id_at(sink_index), context, [&] { return sink.finish(context); });
    check_cancelled(context, id_at(sink_index));
    validate_summary(summary, instances.back()->descriptor(), id_at(sink_index));
    GraphExecutionResult result;
    result.node_outputs_.emplace(id_at(sink_index), std::move(summary));
    return result;
}

}  // namespace audioprocess
