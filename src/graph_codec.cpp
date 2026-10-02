#include "audioprocess/graph_codec.h"
#include "audioprocess/detail/json_pointer.h"

#include <nlohmann/json.hpp>
#include <algorithm>
#include <cmath>
#include <fstream>
#include <iterator>
#include <set>
#include <type_traits>

namespace audioprocess {
namespace {
using Json = nlohmann::json;
constexpr std::size_t kMaxDocumentBytes = 4 * 1024 * 1024;

std::filesystem::path resolve_path(const std::filesystem::path& path,
                                   const std::filesystem::path& base_directory) {
#ifdef _WIN32
    if (!path.is_absolute() && (path.has_root_name() || path.has_root_directory()))
        throw ExecutionError("invalid_path", "Partially qualified Windows paths are not allowed");
#endif
    const auto resolved = (path.is_absolute() ? path : base_directory / path).lexically_normal();
    if (!resolved.is_absolute()) throw ExecutionError("invalid_path", "Path could not be resolved to an absolute path");
    return resolved;
}

[[noreturn]] void invalid(const std::string& message, const std::string& field) {
    throw ExecutionError("invalid_graph_json", message, "", "", "", field);
}

void object_fields(const Json& value, std::initializer_list<std::string_view> allowed,
                   const std::string& field) {
    if (!value.is_object()) invalid("Expected an object", field);
    for (const auto& [key, item] : value.items()) {
        (void)item;
        if (std::find(allowed.begin(), allowed.end(), key) == allowed.end())
            invalid("Unknown field: " + key, detail::json_pointer_append(field, key));
    }
}

const Json& required(const Json& object, const char* key, const std::string& field) {
    if (!object.contains(key)) invalid("Missing field: " + std::string(key), field + "/" + key);
    return object.at(key);
}

std::string string_value(const Json& value, const std::string& field, bool allow_empty = false) {
    if (!value.is_string()) invalid("Expected a string", field);
    auto text = value.get<std::string>();
    if (!allow_empty && text.empty()) invalid("String cannot be empty", field);
    if (text.find('\0') != std::string::npos) invalid("Embedded NUL is not allowed", field);
    return text;
}

const char* parameter_type_name(ParameterType type) {
    switch (type) {
    case ParameterType::Number: return "number";
    case ParameterType::Text: return "text";
    case ParameterType::Boolean: return "boolean";
    case ParameterType::FilePath: return "file_path";
    }
    return "unknown";
}

Json parameter_json(const ParameterValue& parameter) {
    return std::visit([](const auto& value) -> Json {
        using T = std::decay_t<decltype(value)>;
        if constexpr (std::is_same_v<T, std::filesystem::path>) return path_to_utf8(value);
        else if constexpr (std::is_same_v<T, double>) {
            if (!std::isfinite(value)) throw ExecutionError("invalid_parameter", "Non-finite parameter");
            return value;
        } else return value;
    }, parameter);
}

Json descriptor_json(const NodeDescriptor& node) {
    Json inputs = Json::array(), outputs = Json::array(), parameters = Json::array();
    auto ports = [](const std::vector<PortDescriptor>& source, Json& destination) {
        for (const auto& port : source)
            destination.push_back({{"id", port.id}, {"type", data_type_name(port.type)}, {"required", port.required}});
    };
    ports(node.inputs, inputs);
    ports(node.outputs, outputs);
    for (const auto& parameter : node.parameters) {
        Json item{{"id", parameter.id}, {"type", parameter_type_name(parameter.type)},
                  {"description", parameter.description}, {"required", parameter.required}};
        if (parameter.default_value) item["default"] = parameter_json(*parameter.default_value);
        if (parameter.minimum) item["minimum"] = *parameter.minimum;
        if (parameter.maximum) item["maximum"] = *parameter.maximum;
        if (!parameter.unit.empty()) item["unit"] = parameter.unit;
        if (!parameter.enum_values.empty()) item["enum"] = parameter.enum_values;
        if (parameter.integer_only) item["integer_only"] = true;
        parameters.push_back(std::move(item));
    }
    const char* domain = "unsupported";
    switch (node.execution_domain) {
    case ExecutionDomain::Synchronous: domain = "synchronous"; break;
    case ExecutionDomain::Realtime: domain = "realtime"; break;
    case ExecutionDomain::Asynchronous: domain = "asynchronous"; break;
    case ExecutionDomain::Streaming: domain = "streaming"; break;
    }
    const char* role = "none";
    switch (node.stream_role) {
    case StreamRole::None: break;
    case StreamRole::Source: role = "source"; break;
    case StreamRole::Processor: role = "processor"; break;
    case StreamRole::Sink: role = "sink"; break;
    }
    Json result{{"typeId", node.type_id}, {"displayName", node.display_name},
            {"description", node.description}, {"execution_domain", domain},
            {"stream_role", role},
            {"inputs", inputs}, {"outputs", outputs}, {"parameters", parameters}};
    const char* realtime_role = "none";
    switch (node.realtime_role) {
    case RealtimeRole::None: break;
    case RealtimeRole::Source: realtime_role = "source"; break;
    case RealtimeRole::Processor: realtime_role = "processor"; break;
    case RealtimeRole::Sink: realtime_role = "sink"; break;
    }
    result["realtime_role"] = realtime_role;
    if (node.plugin) {
        const auto& plugin = *node.plugin;
        result["plugin"] = {{"id", plugin.plugin_id}, {"implementation_version", plugin.plugin_version},
            {"package_sha256", plugin.package_sha256},
            {"abi", {{"major", plugin.abi_major}, {"minor", plugin.abi_minor}}},
            {"capabilities", Json::array({{{"id", "ag.whole_sync/1"}, {"version", 1}}})}};
    }
    if (node.realtime_capabilities) {
        const auto& capability = *node.realtime_capabilities;
        result["realtime_capabilities"] = {
            {"format", {{"sample_rate", capability.format.sample_rate}, {"channels", capability.format.channel_count},
                        {"sample_type", "float32"}, {"layout", "interleaved"}}},
            {"maximum_block_frames", capability.maximum_block_frames},
            {"supports_variable_blocks", capability.supports_variable_blocks},
            {"offline_drivable", capability.offline_drivable}};
    }
    return result;
}

Json value_json(const DataValue& value) {
    return std::visit([](const auto& item) -> Json {
        using T = std::decay_t<decltype(item)>;
        if constexpr (std::is_same_v<T, AudioClipPtr>) {
            if (!item) throw ExecutionError("invalid_output", "Null audio output");
            return {{"type", "Audio"}, {"sample_rate", item->format.sample_rate},
                    {"channels", item->format.channel_count}, {"frames", item->frame_count()}};
        } else if constexpr (std::is_same_v<T, std::filesystem::path>) {
            return {{"type", "FilePath"}, {"value", path_to_utf8(item)}};
        } else {
            return {{"type", data_type_name(data_type_of(DataValue{item}))}, {"value", item}};
        }
    }, value);
}
} // namespace

std::string path_to_utf8(const std::filesystem::path& path) {
    const auto text = path.u8string();
    return {reinterpret_cast<const char*>(text.data()), text.size()};
}

std::filesystem::path path_from_utf8(std::string_view text) {
    if (text.find('\0') != std::string_view::npos)
        throw ExecutionError("invalid_path", "Embedded NUL in path");
    std::u8string utf8;
    utf8.reserve(text.size());
    for (const unsigned char character : text) utf8.push_back(static_cast<char8_t>(character));
    return std::filesystem::path(utf8);
}

GraphDefinition parse_graph_json(std::string_view text, const NodeRegistry& registry,
                                const std::filesystem::path& base_directory) {
    if (base_directory.empty() || !base_directory.is_absolute())
        throw ExecutionError("invalid_base_directory", "An absolute base directory is required");
    if (text.size() > kMaxDocumentBytes) invalid("Graph JSON exceeds 4 MiB", "");
    Json root;
    // 拒绝重复键，避免用户看到的值与执行值存在歧义；限制文档嵌套深度。
    std::vector<std::set<std::string>> keys;
    auto callback = [&keys](int depth, Json::parse_event_t event, Json& parsed) {
        if (depth > 64) invalid("Graph JSON nesting exceeds 64", "");
        if (event == Json::parse_event_t::object_start) keys.emplace_back();
        if (event == Json::parse_event_t::key && !keys.back().insert(parsed.get<std::string>()).second)
            invalid("Duplicate JSON key: " + parsed.get<std::string>(), "");
        if (event == Json::parse_event_t::object_end) keys.pop_back();
        return true;
    };
    try { root = Json::parse(text.begin(), text.end(), callback); }
    catch (const Json::exception& error) { invalid(error.what(), ""); }
    object_fields(root, {"schema_version", "nodes", "connections", "exports"}, "");
    const auto& version = required(root, "schema_version", "");
    if (!version.is_number_integer() || version != 1) invalid("Unsupported schema_version; expected integer 1", "/schema_version");
    const auto& nodes = required(root, "nodes", "");
    const auto& edges = required(root, "connections", "");
    if (!nodes.is_array() || nodes.empty()) invalid("Expected a nonempty node array", "/nodes");
    if (!edges.is_array()) invalid("Expected an array", "/connections");
    GraphDefinition graph;
    for (std::size_t i = 0; i < nodes.size(); ++i) {
        const auto field = "/nodes/" + std::to_string(i);
        const auto& item = nodes[i];
        object_fields(item, {"id", "type", "parameters"}, field);
        NodeDefinition node{string_value(required(item, "id", field), field + "/id"),
                            string_value(required(item, "type", field), field + "/type"), {}};
        const NodeDescriptor* descriptor{};
        try { descriptor = &registry.descriptor(node.type_id); }
        catch (const std::exception& error) {
            throw ExecutionError("unknown_node_type", error.what(), node.id, "", "", field + "/type");
        }
        if (item.contains("parameters")) {
            const auto& parameters = item.at("parameters");
            if (!parameters.is_object()) invalid("Expected parameter object", field + "/parameters");
            for (const auto& [id, value] : parameters.items()) {
                const auto location = detail::json_pointer_append(field + "/parameters", id);
                const auto found = std::ranges::find(descriptor->parameters, id, &ParameterDescriptor::id);
                if (found == descriptor->parameters.end())
                    throw ExecutionError("unknown_parameter", "Unknown parameter: " + id, node.id, "", id, location);
                switch (found->type) {
                case ParameterType::Number:
                    if (!value.is_number()) invalid("Expected number", location);
                    node.parameters[id] = value.get<double>(); break;
                case ParameterType::Boolean:
                    if (!value.is_boolean()) invalid("Expected boolean", location);
                    node.parameters[id] = value.get<bool>(); break;
                case ParameterType::Text:
                    node.parameters[id] = string_value(value, location, true); break;
                case ParameterType::FilePath: {
                    auto path = path_from_utf8(string_value(value, location));
                    try { node.parameters[id] = resolve_path(path, base_directory); }
                    catch (const ExecutionError& error) {
                        throw ExecutionError(error.code, error.what(), node.id, "", id, location);
                    }
                    break;
                }
                }
            }
        }
        // 默认值和业务约束由 Registry 统一负责。解析与 Graph 依赖校验分离。
        try { node.parameters = registry.normalize_parameters(node.type_id, node.parameters); }
        catch (const ExecutionError& error) {
            throw ExecutionError(error.code, error.what(), node.id, error.port_id,
                                 error.parameter_id, detail::json_pointer_append(field + "/parameters", error.parameter_id));
        }
        // FilePath 默认值也采用同样的基准目录，不随进程工作目录漂移。
        for (auto& [id, value] : node.parameters) {
            (void)id;
            if (auto* path = std::get_if<std::filesystem::path>(&value); path && path->is_relative())
                *path = resolve_path(*path, base_directory);
        }
        graph.nodes.push_back(std::move(node));
    }
    for (std::size_t i = 0; i < edges.size(); ++i) {
        const auto field = "/connections/" + std::to_string(i);
        const auto& edge = edges[i];
        object_fields(edge, {"from", "to"}, field);
        const auto& from = required(edge, "from", field);
        const auto& to = required(edge, "to", field);
        object_fields(from, {"node", "port"}, field + "/from");
        object_fields(to, {"node", "port"}, field + "/to");
        graph.connections.push_back({
            string_value(required(from, "node", field + "/from"), field + "/from/node"),
            string_value(required(from, "port", field + "/from"), field + "/from/port"),
            string_value(required(to, "node", field + "/to"), field + "/to/node"),
            string_value(required(to, "port", field + "/to"), field + "/to/port")});
    }
    if (root.contains("exports")) {
        const auto& exports = root.at("exports");
        if (!exports.is_array()) invalid("Expected an array", "/exports");
        for (std::size_t i = 0; i < exports.size(); ++i) {
            const auto field = "/exports/" + std::to_string(i);
            const auto& item = exports[i];
            object_fields(item, {"name", "node", "port"}, field);
            graph.exports.push_back({string_value(required(item, "name", field), field + "/name"),
                                     string_value(required(item, "node", field), field + "/node"),
                                     string_value(required(item, "port", field), field + "/port")});
        }
    }
    return graph;
}

GraphDefinition load_graph_json(const std::filesystem::path& file, const NodeRegistry& registry) {
    const auto absolute = std::filesystem::absolute(file).lexically_normal();
    std::ifstream input(absolute, std::ios::binary);
    if (!input) throw ExecutionError("graph_read_failed", "Cannot open Graph JSON: " + path_to_utf8(absolute));
    // 读取上限也适用于读取期间增长的文件。
    std::string text;
    char buffer[4096];
    while (input.read(buffer, sizeof(buffer)) || input.gcount() > 0) {
        text.append(buffer, static_cast<std::size_t>(input.gcount()));
        if (text.size() > kMaxDocumentBytes) invalid("Graph JSON exceeds 4 MiB", "");
    }
    if (input.bad()) throw ExecutionError("graph_read_failed", "Cannot read Graph JSON");
    return parse_graph_json(text, registry, absolute.parent_path());
}

std::string graph_to_json(const GraphDefinition& graph) {
    Json root{{"schema_version", graph.schema_version}, {"nodes", Json::array()},
              {"connections", Json::array()}, {"exports", Json::array()}};
    for (const auto& node : graph.nodes) {
        Json parameters = Json::object();
        for (const auto& [id, value] : node.parameters) parameters[id] = parameter_json(value);
        root["nodes"].push_back({{"id", node.id}, {"type", node.type_id}, {"parameters", parameters}});
    }
    for (const auto& edge : graph.connections)
        root["connections"].push_back({{"from", {{"node", edge.source_node}, {"port", edge.source_port}}},
                                       {"to", {{"node", edge.target_node}, {"port", edge.target_port}}}});
    for (const auto& item : graph.exports)
        root["exports"].push_back({{"name", item.name}, {"node", item.node_id}, {"port", item.port_id}});
    return root.dump(2);
}

std::string node_catalog_json(const NodeRegistry& registry) {
    Json nodes = Json::array();
    for (const auto& descriptor : registry.descriptors()) nodes.push_back(descriptor_json(descriptor));
    return Json{{"schema_version", 1}, {"success", true}, {"nodes", nodes}}.dump();
}
std::string node_description_json(const NodeDescriptor& descriptor) {
    return Json{{"schema_version", 1}, {"success", true}, {"node", descriptor_json(descriptor)}}.dump();
}
std::string execution_result_json(const GraphDefinition& graph, const GraphExecutionResult& result) {
    Json outputs = Json::object();
    for (const auto& item : graph.exports) outputs[item.name] = value_json(result.value(item.node_id, item.port_id));
    return Json{{"schema_version", 1}, {"success", true}, {"outputs", outputs}}.dump();
}
std::string error_to_json(const ExecutionError& error) {
    Json detail{{"code", error.code}, {"message", error.what()}};
    if (!error.node_id.empty()) detail["node_id"] = error.node_id;
    if (!error.port_id.empty()) detail["port_id"] = error.port_id;
    if (!error.parameter_id.empty()) detail["parameter_id"] = error.parameter_id;
    if (!error.field_path.empty()) detail["field_path"] = error.field_path;
    return Json{{"schema_version", 1}, {"success", false}, {"errors", Json::array({detail})}}.dump(-1, ' ', false, Json::error_handler_t::replace);
}
} // namespace audioprocess
