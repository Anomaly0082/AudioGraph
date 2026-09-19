#pragma once

#include "audioprocess/graph.h"

#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

namespace audioprocess {

class GraphExecutionResult {
public:
    [[nodiscard]] const DataValue& value(
        const std::string& node_id,
        const std::string& port_id) const;

private:
    friend class SyncGraphExecutor;
    std::unordered_map<std::string, OutputValues> node_outputs_;
};

class SyncGraphExecutor {
public:
    [[nodiscard]] static SyncGraphExecutor compile(
        const GraphDefinition& graph,
        const NodeRegistry& registry);

    [[nodiscard]] GraphExecutionResult execute(ExecutionContext context = {});

private:
    struct InboundConnection {
        std::string target_port;
        std::string source_node;
        std::string source_port;
    };

    struct CompiledNode {
        std::string id;
        NodeDescriptor descriptor;
        std::unique_ptr<ISyncNode> instance;
        std::vector<InboundConnection> inbound;
    };

    std::vector<CompiledNode> execution_plan_;
};

}  // namespace audioprocess

