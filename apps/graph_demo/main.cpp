#include "audioprocess/prototype_nodes.h"
#include "audioprocess/sync_graph_executor.h"

#include <cerrno>
#include <cstdlib>
#include <filesystem>
#include <iomanip>
#include <iostream>
#include <stdexcept>
#include <string>
#include <string_view>

namespace {

// graph-demo 是 P0 阶段的 C++ Sidecar，也是通用节点图原型的入口。
// Rust/Tauri 通过命令行参数调用它，并从 stdout 读取结构化 JSON 结果。
struct Options {
    std::filesystem::path input;
    std::filesystem::path output;
    double gain_db{};
    bool list_nodes{};
};

double parse_number(const char* text) {
    // strtod 允许验证 gain 参数是否被完整解析，避免静默接受非法后缀。
    char* end{};
    errno = 0;
    const auto value = std::strtod(text, &end);
    if (errno != 0 || end == text || *end != '\0') {
        throw std::invalid_argument("Invalid numeric value: " + std::string(text));
    }
    return value;
}

Options parse_options(int argc, char* argv[]) {
    // 两种工作模式：列出已注册节点，或执行预定义的演示 Graph。
    Options options;
    for (int index = 1; index < argc; ++index) {
        const std::string_view argument{argv[index]};
        if (argument == "--list-nodes") {
            options.list_nodes = true;
            continue;
        }
        if (index + 1 >= argc) {
            throw std::invalid_argument("Missing value after " + std::string(argument));
        }
        const char* value = argv[++index];
        if (argument == "--input") {
            options.input = value;
        } else if (argument == "--output") {
            options.output = value;
        } else if (argument == "--gain-db") {
            options.gain_db = parse_number(value);
        } else {
            throw std::invalid_argument("Unknown argument: " + std::string(argument));
        }
    }

    if (!options.list_nodes && (options.input.empty() || options.output.empty())) {
        throw std::invalid_argument("--input and --output are required");
    }
    return options;
}

void print_node_descriptors(const audioprocess::NodeRegistry& registry) {
    // 能力发现接口的最小实现。stdout 只输出 JSON，供 Rust 直接反序列化。
    std::cout << "{\"nodes\":[";
    bool first_node = true;
    for (const auto& node : registry.descriptors()) {
        if (!first_node) {
            std::cout << ',';
        }
        first_node = false;
        std::cout << "{\"typeId\":\"" << node.type_id << "\",\"displayName\":\""
                  << node.display_name << "\",\"inputs\":[";
        bool first_port = true;
        for (const auto& port : node.inputs) {
            if (!first_port) {
                std::cout << ',';
            }
            first_port = false;
            std::cout << "{\"id\":\"" << port.id << "\",\"type\":\""
                      << audioprocess::data_type_name(port.type) << "\"}";
        }
        std::cout << "],\"outputs\":[";
        first_port = true;
        for (const auto& port : node.outputs) {
            if (!first_port) {
                std::cout << ',';
            }
            first_port = false;
            std::cout << "{\"id\":\"" << port.id << "\",\"type\":\""
                      << audioprocess::data_type_name(port.type) << "\"}";
        }
        std::cout << "]}";
    }
    std::cout << "]}\n";
}

}  // namespace

int main(int argc, char* argv[]) {
    try {
        const auto options = parse_options(argc, argv);

        // Registry 保存节点类型、端口描述和创建具体节点实例的工厂。
        const auto registry = audioprocess::create_prototype_node_registry();
        if (options.list_nodes) {
            print_node_descriptors(registry);
            return 0;
        }

        // 演示 Graph：WAV Input -> Gain -> WAV Output，并从 Gain 分支到 Peak Meter。
        const auto graph = audioprocess::create_prototype_graph(
            options.input, options.output, options.gain_db);

        // compile() 验证节点、端口类型、必要输入和环路，并生成拓扑执行计划。
        auto executor = audioprocess::SyncGraphExecutor::compile(graph, registry);

        // execute() 按执行计划调用节点，并保存每个输出端口产生的 DataValue。
        const auto result = executor.execute();
        const auto peak = std::get<double>(result.value("peak", "peak"));

        // 这是 Sidecar 与 Rust 之间的成功响应协议；错误写入 stderr 并返回非零码。
        std::cout << std::fixed << std::setprecision(8)
                  << "{\"success\":true,\"peak\":" << peak
                  << ",\"gainDb\":" << options.gain_db << "}\n";
        return 0;
    } catch (const std::exception& error) {
        // 保持 stdout 只承载成功 JSON，避免 Rust 解析到混合内容。
        std::cerr << error.what() << '\n';
        return 1;
    }
}
