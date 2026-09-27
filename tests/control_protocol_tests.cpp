#include "audioprocess/control_protocol.h"
#include "audioprocess/audio_buffer.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/wav_file.h"
#include <nlohmann/json.hpp>
#include <algorithm>
#include <chrono>
#include <cmath>
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
Json denoise_graph() {
    return {{"schema_version", 1}, {"nodes", Json::array({
        {{"id", "input"}, {"type", "wav_input"}, {"parameters", {{"path", "noisy.wav"}}}},
        {{"id", "denoise"}, {"type", "rnnoise_denoise"}, {"parameters", Json::object()}},
        {{"id", "output"}, {"type", "wav_output"}, {"parameters", {{"path", "clean.wav"}}}}
    })}, {"connections", Json::array({
        {{"from", {{"node", "input"}, {"port", "audio"}}},
         {"to", {{"node", "denoise"}, {"port", "audio"}}}},
        {{"from", {{"node", "denoise"}, {"port", "audio"}}},
         {"to", {{"node", "output"}, {"port", "audio"}}}}
    })}, {"exports", Json::array({
        {{"name", "output_file"}, {"node", "output"}, {"port", "path"}}
    })}};
}
Json converted_denoise_graph(bool include_downmix = true, bool include_resample = true) {
    Json nodes = Json::array({
        {{"id", "input"}, {"type", "wav_input"}, {"parameters", {{"path", "stereo-44k.wav"}}}}
    });
    Json connections = Json::array();
    std::string previous = "input";
    if (include_downmix) {
        nodes.push_back({{"id", "downmix"}, {"type", "audio_downmix_mono"}, {"parameters", Json::object()}});
        connections.push_back({{"from", {{"node", previous}, {"port", "audio"}}},
                               {"to", {{"node", "downmix"}, {"port", "audio"}}}});
        previous = "downmix";
    }
    if (include_resample) {
        nodes.push_back({{"id", "resample"}, {"type", "audio_resample"},
                         {"parameters", {{"sample_rate", 48000}}}});
        connections.push_back({{"from", {{"node", previous}, {"port", "audio"}}},
                               {"to", {{"node", "resample"}, {"port", "audio"}}}});
        previous = "resample";
    }
    nodes.push_back({{"id", "denoise"}, {"type", "rnnoise_denoise"}, {"parameters", Json::object()}});
    nodes.push_back({{"id", "output"}, {"type", "wav_output"}, {"parameters", {{"path", "converted-clean.wav"}}}});
    connections.push_back({{"from", {{"node", previous}, {"port", "audio"}}},
                           {"to", {{"node", "denoise"}, {"port", "audio"}}}});
    connections.push_back({{"from", {{"node", "denoise"}, {"port", "audio"}}},
                           {"to", {{"node", "output"}, {"port", "audio"}}}});
    return {{"schema_version", 1}, {"nodes", nodes}, {"connections", connections},
        {"exports", Json::array({{{"name", "file"}, {"node", "output"}, {"port", "path"}}})}};
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
void write_denoise_fixture(const std::filesystem::path& path) {
    constexpr std::uint32_t frame_count = 961;
    ap::AudioBuffer buffer({48'000, 1}, frame_count);
    auto block = buffer.block(frame_count);
    std::uint32_t noise = 0x93d765ddU;
    for (std::uint32_t index = 0; index < frame_count; ++index) {
        noise = noise * 1664525U + 1013904223U;
        const auto random = static_cast<float>((noise >> 8U) * (1.0 / 16777215.0));
        block.samples[index] = static_cast<float>(0.2 * std::sin(2.0 * 3.141592653589793 * 220.0 * index / 48'000.0) +
            0.05 * (2.0 * random - 1.0));
    }
    ap::WavFileSink sink(path, {48'000, 1}, frame_count);
    sink.write(block);
    sink.finalize();
}
void write_stereo_44k_fixture(const std::filesystem::path& path, std::uint32_t frames = 4411) {
    ap::AudioBuffer buffer({44'100, 2}, frames);
    auto block = buffer.block(frames);
    for (std::uint32_t frame = 0; frame < frames; ++frame) {
        block.samples[frame * 2] = static_cast<float>(0.18 * std::sin(2.0 * 3.141592653589793 * 330.0 * frame / 44'100.0));
        block.samples[frame * 2 + 1] = static_cast<float>(0.12 * std::sin(2.0 * 3.141592653589793 * 770.0 * frame / 44'100.0));
    }
    ap::WavFileSink sink(path, {44'100, 2}, frames);
    sink.write(block); sink.finalize();
}
void write_chunk_flood_wav(const std::filesystem::path& path) {
    std::ofstream output(path, std::ios::binary);
    const auto u16 = [&](std::uint16_t value) { output.write(reinterpret_cast<const char*>(&value), 2); };
    const auto u32 = [&](std::uint32_t value) { output.write(reinterpret_cast<const char*>(&value), 4); };
    output.write("RIFF", 4); u32(4 + 1025 * 8 + 24 + 8); output.write("WAVE", 4);
    for (int index = 0; index < 1025; ++index) { output.write("JUNK", 4); u32(0); }
    output.write("fmt ", 4); u32(16); u16(1); u16(1); u32(48000); u32(96000); u16(2); u16(16);
    output.write("data", 4); u32(0);
}
void test_discovery_and_lifecycle(const Workspace& work) {
    ap::ControlProtocol protocol({work.root});
    const auto capabilities = send(protocol, request("capabilities"));
    require(capabilities.at("data").at("limits").at("retained_tasks") == 16, "Missing service capacity");
    require(!capabilities.at("data").at("policy").at("allow_devices").get<bool>(), "Devices not opt-in");
    const auto listed = send(protocol, request("nodes.list"));
    const auto resample = std::find_if(listed.at("data").at("nodes").begin(),
        listed.at("data").at("nodes").end(), [](const Json& node) { return node.at("typeId") == "audio_resample"; });
    require(resample != listed.at("data").at("nodes").end() &&
            resample->at("parameters").at(0).at("integer_only") == true,
            "audio_resample catalog omitted the integer-only sample rate contract");
    const auto denoise = std::find_if(listed.at("data").at("nodes").begin(),
        listed.at("data").at("nodes").end(), [](const Json& node) {
            return node.at("typeId") == "rnnoise_denoise";
        });
    require(denoise != listed.at("data").at("nodes").end() &&
            denoise->at("execution_domain") == "synchronous" &&
            denoise->at("parameters").empty(), "RNNoise denoise is missing from controlled discovery");
    auto denoise_description = request("nodes.describe");
    denoise_description["type"] = "rnnoise_denoise";
    require(send(protocol, denoise_description).at("data").at("node").at("typeId") == "rnnoise_denoise",
            "Controlled node description cannot resolve RNNoise denoise");
    const auto denoise_validation = send(protocol, graph_command("graph.validate", denoise_graph()));
    require(denoise_validation.at("success").get<bool>() &&
            denoise_validation.at("data").at("node_count") == 3,
            "Controlled graph validation rejected the offline RNNoise WAV graph");
    require(!std::filesystem::exists(work.root / "clean.wav"),
            "Controlled RNNoise graph validation created an output file");
    auto wrong_mode = send(protocol, graph_command("graph.validate", denoise_graph(), "streaming"));
    require(!wrong_mode.at("success").get<bool>() &&
            wrong_mode.at("errors").at(0).at("code") == "unsupported_execution_domain",
            "Controlled API accepted RNNoise as a streaming node");
    auto realtime_graph = denoise_graph();
    realtime_graph["exports"] = Json::array();
    wrong_mode = send(protocol, graph_command("graph.validate", realtime_graph, "realtime"));
    require(!wrong_mode.at("success").get<bool>() &&
            wrong_mode.at("errors").at(0).at("code") == "unsupported_execution_domain",
            "Controlled API accepted RNNoise as a realtime node");

    write_denoise_fixture(work.root / "noisy.wav");
    const auto denoise_started = send(protocol, graph_command("tasks.start", denoise_graph()));
    require(denoise_started.at("success").get<bool>(), "Controlled RNNoise WAV task did not start");
    const auto denoise_id = denoise_started.at("data").at("task_id").get<std::string>();
    const auto denoise_result = wait_result(protocol, denoise_id);
    require(denoise_result.at("data").at("state") == "succeeded" &&
            denoise_result.at("data").at("result").at("outputs").at("output_file").at("value") ==
                ap::path_to_utf8(work.root / "clean.wav"),
            "Controlled RNNoise WAV task did not publish its output file");
    ap::WavFileSource denoised(work.root / "clean.wav", 257);
    require(denoised.format() == ap::AudioFormat{48'000, 1} && denoised.total_frames() == 961,
            "Controlled RNNoise WAV task changed format or frame count");
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
void test_audio_inspection(const Workspace& work) {
    ap::ControlProtocol protocol({work.root});
    const auto wav = work.root / "Inspect.WAV";
    write_stereo_44k_fixture(wav, 4410);
    auto inspect = request("audio.inspect"); inspect["path"] = "Inspect.WAV";
    const auto response = send(protocol, inspect);
    require(response.at("success").get<bool>(), "audio.inspect rejected an in-workspace uppercase WAV");
    const auto& data = response.at("data");
    require(data.at("path") == "Inspect.WAV" &&
            data.at("sample_rate") == 44100 && data.at("channels") == 2 &&
            data.at("frame_count") == 4410 && data.at("duration_seconds") == 0.1 &&
            data.at("encoding") == "pcm_s16le", "audio.inspect metadata mismatch");

    for (const auto& path : {std::string("../outside.wav"), ap::path_to_utf8(work.parent / "outside.wav")}) {
        inspect["path"] = path; error(protocol, inspect, "path_not_allowed");
    }
    inspect["path"] = "."; error(protocol, inspect, "audio_not_file");
    inspect["path"] = "missing.wav"; error(protocol, inspect, "audio_not_file");
    std::ofstream(work.root / "not-audio.txt") << "RIFF";
    inspect["path"] = "not-audio.txt"; error(protocol, inspect, "unsupported_audio_format");
    std::ofstream(work.root / "bad.wav", std::ios::binary) << "RIFFbad";
    inspect["path"] = "bad.wav"; error(protocol, inspect, "audio_inspect_failed");
    write_chunk_flood_wav(work.root / "chunk-flood.wav");
    inspect["path"] = "chunk-flood.wav"; error(protocol, inspect, "audio_inspect_failed");
    for (const auto invalid : {Json(), Json(""), Json(std::string(4097, 'x')), Json(7)}) {
        inspect["path"] = invalid; error(protocol, inspect, "invalid_request");
    }

    const auto outside = work.parent / "outside.wav";
    write_stereo_44k_fixture(outside, 10);
    std::error_code link_error;
    std::filesystem::create_symlink(outside, work.root / "escape-link.wav", link_error);
    if (!link_error) {
        inspect["path"] = "escape-link.wav"; error(protocol, inspect, "path_not_allowed");
    }
}

void test_format_conversion_denoise_graph(const Workspace& work) {
    write_stereo_44k_fixture(work.root / "stereo-44k.wav");
    ap::ControlProtocol protocol({work.root});
    const auto graph = converted_denoise_graph();
    const auto validation = send(protocol, graph_command("graph.validate", graph));
    require(validation.at("success").get<bool>() && !std::filesystem::exists(work.root / "converted-clean.wav"),
            "conversion graph validation failed or created output");
    auto fractional = graph;
    for (auto& node : fractional["nodes"]) {
        if (node["type"] == "audio_resample") node["parameters"]["sample_rate"] = 8000.5;
    }
    error(protocol, graph_command("graph.validate", fractional), "invalid_parameter");
    const auto started = send(protocol, graph_command("tasks.start", graph));
    require(started.at("success").get<bool>(), "44.1 kHz stereo conversion graph did not start");
    const auto result = wait_result(protocol, started.at("data").at("task_id").get<std::string>());
    require(result.at("data").at("state") == "succeeded", "converted RNNoise graph failed");
    ap::WavFileSource output(work.root / "converted-clean.wav", 127);
    const auto expected_frames = (4411ULL * 48000ULL + 44100ULL - 1ULL) / 44100ULL;
    require(output.format() == ap::AudioFormat{48000, 1} && output.total_frames() == expected_frames,
            "converted RNNoise WAV format or ceil duration policy mismatch");

    for (const auto bad_graph : {converted_denoise_graph(false, true), converted_denoise_graph(true, false)}) {
        std::error_code ignored; std::filesystem::remove(work.root / "converted-clean.wav", ignored);
        const auto bad_started = send(protocol, graph_command("tasks.start", bad_graph));
        require(bad_started.at("success").get<bool>(), "format-error graph was rejected before task diagnostics");
        const auto bad_result = wait_result(protocol, bad_started.at("data").at("task_id").get<std::string>());
        require(bad_result.at("data").at("state") == "failed", "RNNoise accepted an unconverted channel/rate format");
    }
}
} // namespace

int main() {
    try {
        Workspace work;
        test_discovery_and_lifecycle(work);
        test_protocol_and_permissions(work);
        test_file_boundary(work);
        test_audio_inspection(work);
        test_format_conversion_denoise_graph(work);
        std::cout << "Control protocol tests passed without device access.\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
