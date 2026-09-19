#pragma once

#include "audioprocess/graph.h"

#include <cstddef>
#include <vector>

namespace audioprocess {

struct GraphValidationResult {
    GraphDefinition graph;  // 已补齐默认参数，未创建节点实例。
    std::vector<std::size_t> topological_order;  // graph.nodes 中的下标。
};

// P1 校验同步 DAG；未来的异步/流式执行能力需独立契约。
[[nodiscard]] GraphValidationResult validate_graph(
    const GraphDefinition& graph, const NodeRegistry& registry);

}  // namespace audioprocess
