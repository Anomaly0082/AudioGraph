#include "audioprocess/control_protocol.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/realtime_graph_executor.h"
#include <nlohmann/json.hpp>
#include <chrono>
#include <fstream>
#include <iostream>
#include <thread>

using Json = nlohmann::json;
namespace ap = audioprocess;
namespace {
void require(bool condition, const char* message) { if (!condition) throw std::runtime_error(message); }
struct Workspace {
    std::filesystem::path parent, root;
    Workspace() {
        parent = std::filesystem::temp_directory_path() / ("audio-control-" +
            std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
        require(std::filesystem::create_directory(parent), "Test directory collision");
        root = parent / "workspace";
        std::filesystem::create_directory(root);
        root = std::filesystem::canonical(root);
    }
    ~Workspace() { std::error_code error; std::filesystem::remove_all(parent, error); }
};
Json request(const char* operation) { return {{"schema_version", 1}, {"id", "test-request"}, {"op", operation}}; }
Json send(ap::ControlProtocol& protocol, const Json& message) { return Json::parse(protocol.handle(message.dump())); }
Json text_graph() {
    return {{"schema_version", 1}, {"nodes", Json::array({{{"id", "source"}, {"type", "text_input"},
        {"parameters", {{"text", "你好，接口"}}}}})}, {"connections", Json::array()},
        {"exports", Json::array({{{"name", "message"}, {"node", "source"}, {"port", "text"}}})}};
}
Json file_graph(const std::string& path) {
    auto graph = text_graph();
    graph["nodes"].push_back({{"id", "sink"}, {"type", "text_output"}, {"parameters", {{"path", path}}}});
    graph["connections"].push_back({{"from", {{"node", "source"}, {"port", "text"}}},
                                    {"to", {{"node", "sink"}, {"port", "text"}}}});
    return graph;
}
Json graph_command(const char* op, const Json& graph, const char* mode = "offline") {
    auto command = request(op); command["mode"] = mode; command["graph"] = graph; return command;
}
void error(ap::ControlProtocol& protocol, const Json& message, const std::string& expected) {
    const auto response = send(protocol, message);
    require(!response.at("success").get<bool>(), "Invalid request accepted");
    require(response.at("errors").at(0).at("code") == expected, "Unexpected request error code");
}
Json wait_result(ap::ControlProtocol& protocol, const std::string& id) {
    auto command = request("tasks.result"); command["task_id"] = id;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(3);
    while (std::chrono::steady_clock::now() < deadline) {
        auto result = send(protocol, command);
        if (result.at("success").get<bool>()) return result;
        require(result.at("errors").at(0).at("code") == "task_not_finished", "Unexpected pending task response");
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    throw std::runtime_error("Timed out waiting for controlled task");
}
void test_discovery_and_lifecycle(const Workspace& work) {
    ap::ControlProtocol protocol({work.root});
    const auto capabilities = send(protocol, request("capabilities"));
    require(capabilities.at("data").at("limits").at("retained_tasks") == 16, "Missing service capacity");
    require(!capabilities.at("data").at("policy").at("allow_devices").get<bool>(), "Devices not opt-in");
    auto description = request("nodes.describe"); description["type"] = "realtime_gain";
    require(send(protocol, description).at("data").at("node").contains("realtime_capabilities"), "Capability metadata lost");
    const auto validation = send(protocol, graph_command("graph.validate", text_graph()));
    require(validation.at("success").get<bool>() && !validation.at("data").at("device_access").get<bool>(), "Graph validation failed");
    const auto started = send(protocol, graph_command("tasks.start", text_graph()));
    require(started.at("success").get<bool>(), "Start failed");
    const auto id = started.at("data").at("task_id").get<std::string>();
    const auto result = wait_result(protocol, id);
    require(result.at("data").at("state") == "succeeded", "Text task did not succeed");
    require(result.at("data").at("result").at("outputs").at("message").at("value") == "你好，接口", "Task text result was corrupted");
    auto cancel = request("tasks.cancel"); cancel["task_id"] = id;
    require(send(protocol, cancel).at("data").at("state") == "succeeded", "Late cancel changed a completed task");
    auto release = request("tasks.release"); release["task_id"] = id;
    require(send(protocol, release).at("success").get<bool>(), "Release failed");
    auto status = request("tasks.status"); status["task_id"] = id;
    error(protocol, status, "unknown_job");
}
void test_protocol_and_permissions(const Workspace& work) {
    ap::ControlProtocol protocol({work.root});
    error(protocol, request("python.execute"), "unknown_operation");
    error(protocol, request("devices.list"), "device_access_denied");
    auto unknown = request("capabilities"); unknown["shell"] = "echo no";
    error(protocol, unknown, "invalid_request");
    auto missing = request("capabilities"); missing.erase("id");
    error(protocol, missing, "invalid_request");
    auto bad_version = request("capabilities"); bad_version["schema_version"] = 1.0;
    error(protocol, bad_version, "invalid_request");
    for (const auto raw : {"{", "[]", R"({"schema_version":1,"id":"a","op":"capabilities","op":"nodes.list"})"}) {
        require(!Json::parse(protocol.handle(raw)).at("success").get<bool>(), "Malformed/duplicate request accepted");
    }
    require(!Json::parse(protocol.handle(std::string(4 * 1024 * 1024 + 1, ' '))).at("success").get<bool>(), "Oversized request accepted");
    auto wrong_options = graph_command("graph.validate", text_graph());
    wrong_options["options"] = {{"duration_seconds", 10}};
    error(protocol, wrong_options, "invalid_request");
    auto rt = Json::parse(ap::graph_to_json(ap::make_realtime_graph("fake-in", "fake-out")));
    require(send(protocol, graph_command("graph.validate", rt, "realtime")).at("success").get<bool>(), "Pure realtime validation required devices");
    error(protocol, graph_command("tasks.start", rt, "realtime"), "device_access_denied");
    ap::ControlProtocol devices_only({work.root, true, false});
    auto audible = graph_command("tasks.start", rt, "realtime"); audible["options"] = {{"probe", false}};
    error(devices_only, audible, "monitor_access_denied"); // 假ID也不能进入设备枚举。
    require(send(protocol, request("capabilities")).at("success").get<bool>(), "Service unusable after malformed requests");
}
void test_file_boundary(const Workspace& work) {
    ap::ControlProtocol protocol({work.root});
    for (const auto& path : {std::string("../escape.txt"), ap::path_to_utf8(work.parent / "workspace-other" / "out.txt"),
                             ap::path_to_utf8(work.parent / "outside.txt")}) {
        error(protocol, graph_command("graph.validate", file_graph(path)), "path_not_allowed");
        error(protocol, graph_command("tasks.start", file_graph(path)), "path_not_allowed");
    }
    const auto target = work.root / ap::path_from_utf8("新结果.txt");
    require(send(protocol, graph_command("graph.validate", file_graph("新结果.txt"))).at("success").get<bool>(), "In-workspace output rejected");
    require(!std::filesystem::exists(target), "Validation wrote an output file");
    auto started = send(protocol, graph_command("tasks.start", file_graph("新结果.txt")));
    auto id = started.at("data").at("task_id").get<std::string>();
    require(wait_result(protocol, id).at("data").at("state") == "succeeded", "In-workspace file task failed");
    std::ifstream input(target, std::ios::binary);
    std::string content((std::istreambuf_iterator<char>(input)), {});
    require(content == "你好，接口", "Output file content mismatch");
    started = send(protocol, graph_command("tasks.start", file_graph("新结果.txt")));
    id = started.at("data").at("task_id").get<std::string>();
    require(wait_result(protocol, id).at("data").at("state") == "failed", "Existing output was overwritten");
}
} // namespace

int main() {
    try {
        Workspace work;
        test_discovery_and_lifecycle(work);
        test_protocol_and_permissions(work);
        test_file_boundary(work);
        std::cout << "Control protocol tests passed without device access.\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
