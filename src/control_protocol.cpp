#include "audioprocess/control_protocol.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/task_runner.h"
#include "audioprocess/wav_file.h"
#include "audioprocess/plugin_host.h"
#ifdef _WIN32
#include "audioprocess/realtime_session.h"
#endif

#include <nlohmann/json.hpp>
#include <algorithm>
#include <cctype>
#include <set>

namespace audioprocess {
namespace {
using Json = nlohmann::json;
constexpr std::size_t maximum_request_bytes = 4 * 1024 * 1024;

[[noreturn]] void invalid(const std::string& message, const std::string& field = {}) {
    throw ExecutionError("invalid_request", message, {}, {}, {}, field);
}

void fields(const Json& value, std::initializer_list<std::string_view> allowed,
            const std::string& field = {}) {
    if (!value.is_object()) invalid("Expected an object", field);
    for (const auto& [key, item] : value.items()) {
        (void)item;
        if (std::find(allowed.begin(), allowed.end(), key) == allowed.end())
            invalid("Unknown field: " + key, field + "/" + key);
    }
}

std::string text_field(const Json& object, const char* key, std::size_t maximum = 128) {
    const auto field = "/" + std::string(key);
    if (!object.contains(key) || !object.at(key).is_string()) invalid("Required string field", field);
    auto text = object.at(key).get<std::string>();
    if (text.empty() || text.size() > maximum || text.find('\0') != std::string::npos)
        invalid("String must be nonempty, bounded and contain no NUL", field);
    return text;
}

std::uint32_t integer(const Json& object, const char* key, std::uint32_t fallback,
                      std::uint32_t maximum) {
    if (!object.contains(key)) return fallback;
    const auto& value = object.at(key);
    if (!value.is_number_unsigned() || value.get<std::uint64_t>() == 0 || value.get<std::uint64_t>() > maximum)
        invalid("Expected a positive bounded integer", "/options/" + std::string(key));
    return value.get<std::uint32_t>();
}

Json parse_request(std::string_view input) {
    if (input.size() > maximum_request_bytes) invalid("Request exceeds 4 MiB");
    std::vector<std::set<std::string>> keys;
    auto callback = [&keys](int depth, Json::parse_event_t event, Json& value) {
        if (depth > 64) invalid("Request nesting exceeds 64");
        if (event == Json::parse_event_t::object_start) keys.emplace_back();
        if (event == Json::parse_event_t::key && !keys.back().insert(value.get<std::string>()).second)
            invalid("Duplicate JSON key");
        if (event == Json::parse_event_t::object_end) keys.pop_back();
        return true;
    };
    try { return Json::parse(input.begin(), input.end(), callback); }
    catch (const Json::exception& error) { invalid(error.what()); }
}

ControlPolicy normalize_policy(ControlPolicy policy) {
    std::error_code error;
    policy.workspace = std::filesystem::canonical(policy.workspace, error);
    if (error || !std::filesystem::is_directory(policy.workspace))
        throw ExecutionError("invalid_workspace", "Workspace must be an existing directory");
    if (policy.allow_monitor && !policy.allow_devices)
        throw ExecutionError("invalid_policy", "Monitor permission requires device permission");
    return policy;
}

NodeRegistry control_registry(const ControlPolicy& policy, std::string& plugin_report) {
    auto registry = create_prototype_node_registry();
    register_realtime_nodes(registry);
    plugin_report = register_plugin_nodes(registry, PluginHostOptions{
        policy.plugin_snapshot_path, policy.plugin_snapshot_sha256, policy.plugin_data_root, policy.workspace});
    return registry;
}

Json task_error_json(const TaskError& error) {
    Json result{{"code", error.code}, {"message", error.message}};
    if (!error.node_id.empty()) result["node_id"] = error.node_id;
    if (!error.port_id.empty()) result["port_id"] = error.port_id;
    if (!error.parameter_id.empty()) result["parameter_id"] = error.parameter_id;
    if (!error.field_path.empty()) result["field_path"] = error.field_path;
    return result;
}

Json snapshot_json(const TaskSnapshot& snapshot) {
    Json result{{"task_id", snapshot.id}, {"state", task_state_name(snapshot.state)}};
    if (snapshot.error) result["errors"] = Json::array({task_error_json(*snapshot.error)});
    return result;
}

TaskRequest graph_request(const Json& request, const NodeRegistry& registry, const ControlPolicy& policy) {
    fields(request, {"schema_version", "id", "op", "mode", "graph", "options"});
    TaskRequest task;
    const auto mode = text_field(request, "mode");
    if (mode == "offline") task.mode = ExecutionDomain::Synchronous;
    else if (mode == "streaming") task.mode = ExecutionDomain::Streaming;
    else if (mode == "realtime") task.mode = ExecutionDomain::Realtime;
    else invalid("Mode must be offline, streaming or realtime", "/mode");
    const auto options = request.value("options", Json::object());
    if (task.mode == ExecutionDomain::Synchronous) fields(options, {}, "/options");
    else if (task.mode == ExecutionDomain::Streaming) fields(options, {"block_frames"}, "/options");
    else fields(options, {"block_frames", "probe", "duration_seconds"}, "/options");
    task.block_frames = integer(options, "block_frames", 256, 65536);
    task.duration_seconds = integer(options, "duration_seconds", 10, 3600);
    if (options.contains("probe")) {
        if (!options.at("probe").is_boolean()) invalid("Expected boolean probe", "/options/probe");
        task.probe = options.at("probe").get<bool>();
    }
    if (!request.contains("graph")) invalid("Graph is required", "/graph");
    task.graph = parse_graph_json(request.at("graph").dump(), registry, policy.workspace);
    validate_task_request(task, registry);
    validate_control_paths(task.graph, policy);
    return task;
}

std::filesystem::path resolve_control_path(const std::filesystem::path& path, const ControlPolicy& policy) {
#ifdef _WIN32
    if (!path.is_absolute() && (path.has_root_name() || path.has_root_directory()))
        throw ExecutionError("invalid_path", "Partially qualified Windows paths are not allowed", {}, {}, {}, "/path");
#endif
    const auto candidate = path.is_absolute() ? path : policy.workspace / path;
    std::error_code error;
    const auto resolved = std::filesystem::weakly_canonical(candidate, error);
    bool inside = !error && resolved.is_absolute();
    auto target_part = resolved.begin();
    for (auto root_part = policy.workspace.begin(); inside && root_part != policy.workspace.end(); ++root_part) {
        if (target_part == resolved.end() || *root_part != *target_part) { inside = false; break; }
        ++target_part;
    }
    if (!inside)
        throw ExecutionError("path_not_allowed", "File path escapes the host workspace or cannot be resolved", {}, {}, {}, "/path");
    return resolved;
}

Json inspect_audio(const Json& request, const ControlPolicy& policy) {
    fields(request, {"schema_version", "id", "op", "path"});
    const auto requested = path_from_utf8(text_field(request, "path", 4096));
    const auto path = resolve_control_path(requested, policy);
    std::error_code error;
    const auto status = std::filesystem::status(path, error);
    if (error || !std::filesystem::exists(status) || !std::filesystem::is_regular_file(status))
        throw ExecutionError("audio_not_file", "Audio inspection requires an existing regular file", {}, {}, {}, "/path");
    auto extension = path.extension().string();
    std::ranges::transform(extension, extension.begin(), [](unsigned char character) {
        return static_cast<char>(std::tolower(character));
    });
    if (extension != ".wav")
        throw ExecutionError("unsupported_audio_format", "Audio inspection only accepts PCM16 WAV files", {}, {}, {}, "/path");
    try {
        WavFileSource source(path, 1, 1024);
        const auto& format = source.format();
        const auto frames = source.total_frames();
        return {{"path", path_to_utf8(path.lexically_relative(policy.workspace))}, {"sample_rate", format.sample_rate},
                {"channels", format.channel_count}, {"frame_count", frames},
                {"duration_seconds", static_cast<double>(frames) / static_cast<double>(format.sample_rate)},
                {"encoding", "pcm_s16le"}};
    } catch (const ExecutionError&) {
        throw;
    } catch (const std::exception& error) {
        throw ExecutionError("audio_inspect_failed", error.what(), {}, {}, {}, "/path");
    }
}

} // namespace

void validate_control_paths(const GraphDefinition& graph, const ControlPolicy& policy) {
    // canonical/weakly_canonical 会解析已存在的链接；逐组件比较避免 root 与 root-other 前缀混淆。
    // 不把原始字符串的大小写/前缀当权限依据，无法解析的路径一律拒绝。
    // 根目录在宿主启动时已 canonical；不要在每次请求重新解析根、意外扩大授权范围。
    if (!policy.workspace.is_absolute()) throw ExecutionError("invalid_workspace", "Host workspace must be canonical and absolute");
    for (const auto& node : graph.nodes) {
        for (const auto& [id, value] : node.parameters) {
            const auto* path = std::get_if<std::filesystem::path>(&value);
            if (!path) continue;
            try { (void)resolve_control_path(*path, policy); }
            catch (const ExecutionError& error) {
                throw ExecutionError(error.code, "File parameter escapes the host workspace or cannot be resolved", node.id, {}, id);
            }
        }
    }
}

ControlProtocol::ControlProtocol(ControlPolicy policy)
    : policy_(normalize_policy(std::move(policy))), registry_(control_registry(policy_, plugin_report_json_)),
      tasks_([this](const TaskRequest& request, std::atomic_bool& cancellation) {
          validate_control_paths(request.graph, policy_);
          return execute_task_request(request, registry_, cancellation);
      }) {}

std::string ControlProtocol::handle(std::string_view input) {
    Json request_id = nullptr;
    try {
        const auto request = parse_request(input);
        if (!request.is_object()) invalid("Request must be an object");
        request_id = text_field(request, "id");
        if (!request.contains("schema_version") || !request.at("schema_version").is_number_integer() || request.at("schema_version") != 1)
            invalid("Expected integer schema_version 1", "/schema_version");
        const auto operation = text_field(request, "op");
        Json data;
        if (operation == "capabilities" || operation == "nodes.list") {
            fields(request, {"schema_version", "id", "op"});
            data = Json::parse(node_catalog_json(registry_));
            data["plugins"] = Json::parse(plugin_report_json_);
            if (operation == "capabilities") {
                data["operations"] = {"capabilities", "nodes.list", "nodes.describe", "devices.list", "audio.inspect", "graph.validate",
                                      "tasks.start", "tasks.status", "tasks.cancel", "tasks.result", "tasks.release"};
                data["modes"] = {"offline", "streaming", "realtime"};
                data["policy"] = {{"workspace", path_to_utf8(policy_.workspace)}, {"allow_devices", policy_.allow_devices},
                                   {"allow_monitor", policy_.allow_monitor}};
#ifdef _WIN32
                data["device_backend_available"] = true;
#else
                data["device_backend_available"] = false;
#endif
                data["limits"] = {{"active_tasks", 1}, {"retained_tasks", 16}, {"request_bytes", maximum_request_bytes},
                                   {"result_bytes", maximum_request_bytes}, {"request_depth", 64}};
            }
        } else if (operation == "audio.inspect") {
            data = inspect_audio(request, policy_);
        } else if (operation == "nodes.describe") {
            fields(request, {"schema_version", "id", "op", "type"});
            data = Json::parse(node_description_json(registry_.descriptor(text_field(request, "type"))));
        } else if (operation == "devices.list") {
            fields(request, {"schema_version", "id", "op"});
            if (!policy_.allow_devices) throw ExecutionError("device_access_denied", "Device access was not enabled by the host");
#ifdef _WIN32
            const auto catalog = RealtimeSession::enumerate_devices();
            data = {{"inputs", Json::array()}, {"outputs", Json::array()}};
            for (const auto& item : catalog.inputs) data["inputs"].push_back({{"id", item.id}, {"name", item.name}, {"is_default", item.is_default}});
            for (const auto& item : catalog.outputs) data["outputs"].push_back({{"id", item.id}, {"name", item.name}, {"is_default", item.is_default}});
#else
            throw ExecutionError("unsupported_platform", "This build has no device backend");
#endif
        } else if (operation == "graph.validate" || operation == "tasks.start") {
            auto task = graph_request(request, registry_, policy_);
            if (operation == "graph.validate") {
                data = {{"valid", true}, {"device_access", false}, {"node_count", task.graph.nodes.size()}};
            } else {
                if (task.mode == ExecutionDomain::Realtime) {
                    if (!policy_.allow_devices) throw ExecutionError("device_access_denied", "Realtime tasks require host device permission");
                    if (!task.probe && !policy_.allow_monitor) throw ExecutionError("monitor_access_denied", "Audible output was not enabled by the host");
                }
                const auto id = tasks_.submit(std::move(task));
                data = snapshot_json(tasks_.status(id));
            }
        } else if (operation == "tasks.status" || operation == "tasks.cancel" || operation == "tasks.result" || operation == "tasks.release") {
            fields(request, {"schema_version", "id", "op", "task_id"});
            const auto id = text_field(request, "task_id");
            if (operation == "tasks.release") {
                tasks_.release(id);
                data = {{"task_id", id}, {"released", true}};
            } else if (operation == "tasks.cancel") data = snapshot_json(tasks_.cancel(id));
            else if (operation == "tasks.status") data = snapshot_json(tasks_.status(id));
            else {
                const auto outcome = tasks_.result(id);
                data = snapshot_json(tasks_.status(id));
                if (!outcome.result_json.empty()) data["result"] = Json::parse(outcome.result_json);
                if (outcome.error) data["errors"] = Json::array({task_error_json(*outcome.error)});
            }
        } else throw ExecutionError("unknown_operation", "Operation is not supported: " + operation);
        // Windows 后端异常文本可能不是 UTF-8；替换坏字节而不丢失任务状态/节点位置。
        return Json{{"schema_version", 1}, {"id", request_id}, {"success", true}, {"data", std::move(data)}}
            .dump(-1, ' ', false, Json::error_handler_t::replace);
    } catch (const ExecutionError& error) {
        auto result = Json::parse(error_to_json(error));
        result["id"] = request_id;
        return result.dump();
    } catch (const std::exception& error) {
        auto result = Json::parse(error_to_json(ExecutionError("control_failed", error.what())));
        result["id"] = request_id;
        return result.dump();
    }
}

} // namespace audioprocess
