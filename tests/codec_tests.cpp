#include "audioprocess/graph_codec.h"
#include "audioprocess/graph_validator.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/wav_file.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>

namespace ap = audioprocess;
namespace {
void require(bool condition, const char* message) {
    if (!condition) throw std::runtime_error(message);
}
struct Workspace {
    std::filesystem::path root = std::filesystem::temp_directory_path() /
        ("audio-codec-" + std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
    Workspace() {
        if (!std::filesystem::create_directory(root))
            throw std::runtime_error("Test workspace already exists; refusing to use it");
    }
    ~Workspace() { std::error_code ec; std::filesystem::remove_all(root, ec); }
};
template<class F> void rejected(F&& action) {
    try { action(); }
    catch (const ap::ExecutionError& error) { require(!error.code.empty(), "Missing error code"); return; }
    throw std::runtime_error("Expected rejection");
}
const std::string text_graph = R"({"schema_version":1,"nodes":[
 {"id":"source","type":"text_input","parameters":{"text":"中文 \"测试\"\n下一行"}}],
 "connections":[],"exports":[{"name":"message","node":"source","port":"text"}]})";

void test_codec(const Workspace& work) {
    const auto registry = ap::create_prototype_node_registry();
    const auto graph = ap::parse_graph_json(text_graph, registry, work.root);
    const auto roundtrip = ap::parse_graph_json(ap::graph_to_json(graph), registry, work.root);
    require(ap::graph_to_json(graph) == ap::graph_to_json(roundtrip), "Graph semantic roundtrip failed");
    auto executor = ap::SyncGraphExecutor::compile(roundtrip, registry);
    const auto result = executor.execute();
    require(std::get<std::string>(result.value("source", "text")) == "中文 \"测试\"\n下一行", "Text changed");
    const auto response = ap::execution_result_json(graph, result);
    require(response.find("message") != std::string::npos, "Missing exported text");
    for (const auto& bad : std::vector<std::string>{
        "{}", "[]", "{", text_graph + " garbage",
        R"({"schema_version":1,"schema_version":1,"nodes":[],"connections":[]})",
        R"({"schema_version":2,"nodes":[],"connections":[]})",
        R"({"schema_version":1.0,"nodes":[],"connections":[]})",
        R"({"schema_version":1,"nodes":[{"id":"s","type":"text_input","parameters":{"text":"ok","typo":1}}],"connections":[]})",
        R"({"schema_version":1,"nodes":[{"id":"s","type":"text_input","parameters":{"text":3}}],"connections":[]})",
        R"({"schema_version":1,"nodes":[{"id":"s","type":"gain","parameters":{"gain_db":1e999}}],"connections":[]})"
    }) rejected([&] { (void)ap::parse_graph_json(bad, registry, work.root); });
    rejected([&] { (void)ap::parse_graph_json(text_graph, registry, "relative"); });
    try {
        (void)ap::parse_graph_json(
            R"({"schema_version":1,"nodes":[{"id":"s","type":"gain","parameters":{"bad~/key":3}}],"connections":[]})",
            registry, work.root);
        throw std::runtime_error("Unknown escaped parameter accepted");
    } catch (const ap::ExecutionError& error) {
        require(error.field_path == "/nodes/0/parameters/bad~0~1key", "JSON Pointer escaping failed");
    }
    auto bad_edge_graph = graph;
    bad_edge_graph.connections.push_back({"missing", "text", "source", "text"});
    try {
        (void)ap::validate_graph(bad_edge_graph, registry);
        throw std::runtime_error("Unknown source accepted");
    } catch (const ap::ExecutionError& error) {
        require(error.field_path == "/connections/0/from/node", "Validator location differs from Graph JSON");
    }
    const auto paths = ap::parse_graph_json(
        R"({"schema_version":1,"nodes":[{"id":"in","type":"wav_input","parameters":{"path":"中文 input.wav"}}],"connections":[]})",
        registry, work.root);
    require(std::get<std::filesystem::path>(paths.nodes[0].parameters.at("path")) ==
            work.root / ap::path_from_utf8("中文 input.wav"), "Relative UTF-8 path resolution failed");
#ifdef _WIN32
    for (const auto bad : {
        R"({"schema_version":1,"nodes":[{"id":"in","type":"wav_input","parameters":{"path":"D:input.wav"}}],"connections":[]})",
        R"({"schema_version":1,"nodes":[{"id":"in","type":"wav_input","parameters":{"path":"\\input.wav"}}],"connections":[]})"
    }) rejected([&] { (void)ap::parse_graph_json(bad, registry, work.root); });
#endif
}

void test_audio_graphs(const Workspace& work) {
    const auto registry = ap::create_prototype_node_registry();
    const auto input = work.root / "input.wav";
    {
        ap::AudioBuffer buffer({48000, 1}, 17);
        auto block = buffer.block(17);
        std::fill(block.samples.begin(), block.samples.end(), 0.25F);
        ap::WavFileSink sink(input, {48000, 1}, 17);
        sink.write(block);
        sink.finalize();
    }
    // 三种配置在同一执行器入口运行，既检查图序列化，也检查保存音频结果。
    for (int mode = 0; mode < 3; ++mode) {
        const auto output = work.root / ("output-" + std::to_string(mode) + ".wav");
        auto graph = ap::create_prototype_graph(input, output, -6.020599913);
        if (mode == 0) {
            graph.nodes.erase(graph.nodes.begin() + 1); // 删除 gain
            for (auto& edge : graph.connections) if (edge.source_node == "gain") edge.source_node = "input";
            graph.connections.erase(graph.connections.begin());
        }
        if (mode < 2) {
            std::erase_if(graph.nodes, [](const auto& node) { return node.id == "peak"; });
            std::erase_if(graph.connections, [](const auto& edge) { return edge.target_node == "peak"; });
            std::erase_if(graph.exports, [](const auto& item) { return item.node_id == "peak"; });
        }
        std::reverse(graph.nodes.begin(), graph.nodes.end());
        const auto config = work.root / ("graph-" + std::to_string(mode) + ".json");
        { std::ofstream file(config); file << ap::graph_to_json(graph); }
        const auto loaded = ap::load_graph_json(config, registry);
        (void)ap::validate_graph(loaded, registry);
        require(!std::filesystem::exists(output), "Validation produced output");
        ap::validate_prototype_file_targets(loaded);
        auto executor = ap::SyncGraphExecutor::compile(loaded, registry);
        const auto result = executor.execute();
        if (mode == 2) require(std::abs(std::get<double>(result.value("peak", "peak")) - 0.125) < 1e-5, "Peak wrong");
        ap::WavFileSource source(output, 32);
        ap::AudioBuffer buffer(source.format(), 32);
        auto block = source.read(buffer);
        require(block && block->frame_count == 17, "Audio length changed");
        require(std::abs(block->samples[0] - (mode == 0 ? 0.25F : 0.125F)) < 0.0001F, "Written audio wrong");
    }
}
} // namespace
int main() {
    try {
        Workspace workspace;
        test_codec(workspace);
        test_audio_graphs(workspace);
        std::cout << "Graph codec and configuration tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
