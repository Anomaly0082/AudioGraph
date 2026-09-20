#pragma once

#include "audioprocess/graph_validator.h"
#include "audioprocess/streaming_node.h"
#include "audioprocess/sync_graph_executor.h"

namespace audioprocess {

// 纯校验：只允许 1 个 Source -> 0..N 个 Processor -> 1 个 Sink，最多 128 节点。
// 流块不能导出为 DataValue，只有 Sink 的最终摘要可通过 Graph exports 返回。
[[nodiscard]] GraphValidationResult validate_stream_graph(
    const GraphDefinition& graph, const NodeRegistry& registry);

// 同步拉取、按块调用的离线执行器；无异步队列，不承诺实时设备的时限。
class StreamingGraphExecutor {
public:
    [[nodiscard]] static StreamingGraphExecutor compile(
        const GraphDefinition& graph, const NodeRegistry& registry);
    // 块长是运行选项，允许 1..65536 帧，不写入图定义。
    [[nodiscard]] GraphExecutionResult execute(
        std::uint32_t maximum_frames = 256, ExecutionContext context = {});

private:
    StreamingGraphExecutor() = default;
    NodeRegistry registry_;
    GraphDefinition graph_;
    std::vector<std::size_t> order_;
};

}  // namespace audioprocess
