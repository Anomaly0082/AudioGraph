#pragma once

#include "audioprocess/graph.h"
#include "audioprocess/node.h"

#include <filesystem>

namespace audioprocess {

[[nodiscard]] NodeRegistry create_prototype_node_registry();

// CLI 运行前的内置文件节点策略校验。它不是 Executor 的通用节点类型分支。
// 仅做预检，不创建文件；输出节点还会独占创建文件，避免检查/写入间的竞态。
void validate_prototype_file_targets(const GraphDefinition& graph);

[[nodiscard]] GraphDefinition create_prototype_graph(
    const std::filesystem::path& input,
    const std::filesystem::path& output,
    double gain_db);

}  // namespace audioprocess
