#include "audioprocess/graph_codec.h"
#include "audioprocess/graph_validator.h"
#include "audioprocess/prototype_nodes.h"

#include <charconv>
#include <cmath>
#include <filesystem>
#include <iostream>
#include <optional>
#include <set>
#include <string>
#include <vector>

namespace ap = audioprocess;
namespace {

// CLI 只负责协议适配。Graph 数据、参数检查与执行规则属于核心库。
struct Options {
    std::filesystem::path graph_file, input, output;
    std::string describe;
    double gain_db{};
    bool list_nodes{}, validate{}, help{};
};

Options parse_options(const std::vector<std::string>& args) {
    Options options;
    std::set<std::string> seen;
    for (std::size_t i = 1; i < args.size(); ++i) {
        const auto& key = args[i];
        if (!seen.insert(key).second)
            throw ap::ExecutionError("invalid_arguments", "Duplicate option: " + key);
        if (key == "--help" || key == "-h") { options.help = true; continue; }
        if (key == "--list-nodes") { options.list_nodes = true; continue; }
        if (key == "--validate") { options.validate = true; continue; }
        if (key != "--graph" && key != "--describe-node" && key != "--input" &&
            key != "--output" && key != "--gain-db")
            throw ap::ExecutionError("invalid_arguments", "Unknown option: " + key);
        if (++i == args.size())
            throw ap::ExecutionError("invalid_arguments", "Missing value for: " + key);
        const auto& value = args[i];
        if (value.empty()) throw ap::ExecutionError("invalid_arguments", "Empty value for: " + key);
        if (key == "--graph") options.graph_file = ap::path_from_utf8(value);
        else if (key == "--describe-node") options.describe = value;
        else if (key == "--input") options.input = ap::path_from_utf8(value);
        else if (key == "--output") options.output = ap::path_from_utf8(value);
        else {
            const auto parsed = std::from_chars(value.data(), value.data() + value.size(), options.gain_db);
            if (parsed.ec != std::errc{} || parsed.ptr != value.data() + value.size() || !std::isfinite(options.gain_db))
                throw ap::ExecutionError("invalid_arguments", "Gain must be a finite number");
        }
    }
    if (options.help) return options;
    const bool legacy = seen.contains("--input") || seen.contains("--output") || seen.contains("--gain-db");
    const int modes = static_cast<int>(options.list_nodes) + static_cast<int>(!options.describe.empty()) +
                      static_cast<int>(!options.graph_file.empty()) + static_cast<int>(legacy);
    if (modes != 1) throw ap::ExecutionError("invalid_arguments", "Choose exactly one of --graph, --list-nodes, --describe-node or legacy --input/--output");
    if (options.validate && options.graph_file.empty())
        throw ap::ExecutionError("invalid_arguments", "--validate requires --graph");
    if (legacy && (options.input.empty() || options.output.empty()))
        throw ap::ExecutionError("invalid_arguments", "Both --input and --output are required");
    return options;
}

int run(const std::vector<std::string>& args) {
    try {
        const auto options = parse_options(args);
        if (options.help) {
            std::cout << "graph-demo --list-nodes\n"
                         "graph-demo --describe-node <type>\n"
                         "graph-demo --graph <graph.json> [--validate]\n"
                         "graph-demo --input <in.wav> --output <out.wav> [--gain-db <dB>]\n";
            return 0;
        }
        const auto registry = ap::create_prototype_node_registry();
        if (options.list_nodes) { std::cout << ap::node_catalog_json(registry) << '\n'; return 0; }
        if (!options.describe.empty()) {
            std::cout << ap::node_description_json(registry.descriptor(options.describe)) << '\n';
            return 0;
        }

        // 新协议接受外部 Graph；保留旧入口让当前 Tauri 演示继续可用。
        const bool legacy = options.graph_file.empty();
        const auto graph = legacy
            ? ap::create_prototype_graph(std::filesystem::absolute(options.input),
                                         std::filesystem::absolute(options.output), options.gain_db)
            : ap::load_graph_json(options.graph_file, registry);
        const auto validated = ap::validate_graph(graph, registry);
        if (options.validate) {
            // 这里只验证配置，不创建节点、不打开音频、不检查运行时文件存在性。
            std::cout << "{\"schema_version\":1,\"success\":true,\"valid\":true,\"node_count\":"
                      << validated.graph.nodes.size() << "}\n";
            return 0;
        }
        ap::validate_prototype_file_targets(validated.graph);
        auto executor = ap::SyncGraphExecutor::compile(validated.graph, registry);
        const auto result = executor.execute();
        if (legacy) {
            // 兼容现有 Rust RunResult；任意 Graph 使用 exports 返回通用结果。
            std::cout << "{\"schema_version\":1,\"success\":true,\"peak\":"
                      << std::get<double>(result.value("peak", "peak"))
                      << ",\"gainDb\":" << options.gain_db << "}\n";
        } else {
            std::cout << ap::execution_result_json(validated.graph, result) << '\n';
        }
        return 0;
    } catch (const ap::ExecutionError& error) {
        std::cout << ap::error_to_json(error) << '\n';
        // 当前 Rust 桥接从 stderr 读取失败文本，保留兼容性。
        std::cerr << error.what() << '\n';
    } catch (const std::exception& error) {
        std::cout << ap::error_to_json(ap::ExecutionError("execution_failed", error.what())) << '\n';
        std::cerr << error.what() << '\n';
    }
    return 1;
}
} // namespace

#ifdef _WIN32
// Windows 命令行使用宽字符，避免中文路径先被系统代码页破坏。
int wmain(int argc, wchar_t* argv[]) {
    std::vector<std::string> args;
    for (int i = 0; i < argc; ++i) args.push_back(ap::path_to_utf8(std::filesystem::path(argv[i])));
    return run(args);
}
#else
int main(int argc, char* argv[]) {
    return run(std::vector<std::string>(argv, argv + argc));
}
#endif
