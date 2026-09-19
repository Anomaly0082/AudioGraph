#pragma once

#include "audioprocess/graph.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/sync_graph_executor.h"

#include <filesystem>
#include <string>
#include <string_view>

namespace audioprocess {

// JSON 是边界协议；Node 与 Executor 不依赖 JSON 库。
// 纯解析不执行节点。相对路径必须通过显式 base_directory 解析。
[[nodiscard]] GraphDefinition parse_graph_json(
    std::string_view text, const NodeRegistry& registry,
    const std::filesystem::path& base_directory);
[[nodiscard]] GraphDefinition load_graph_json(
    const std::filesystem::path& file, const NodeRegistry& registry);
[[nodiscard]] std::string graph_to_json(const GraphDefinition& graph);
[[nodiscard]] std::string node_catalog_json(const NodeRegistry& registry);
[[nodiscard]] std::string node_description_json(const NodeDescriptor& descriptor);
[[nodiscard]] std::string execution_result_json(
    const GraphDefinition& graph, const GraphExecutionResult& result);
[[nodiscard]] std::string error_to_json(const ExecutionError& error);

// 明确使用 UTF-8 作为 JSON/进程协议编码；Windows 文件操作仍使用原生宽路径。
[[nodiscard]] std::string path_to_utf8(const std::filesystem::path& path);
[[nodiscard]] std::filesystem::path path_from_utf8(std::string_view text);

} // namespace audioprocess
