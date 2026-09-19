#pragma once

#include "audioprocess/graph.h"
#include "audioprocess/node.h"

#include <filesystem>

namespace audioprocess {

[[nodiscard]] NodeRegistry create_prototype_node_registry();

[[nodiscard]] GraphDefinition create_prototype_graph(
    const std::filesystem::path& input,
    const std::filesystem::path& output,
    double gain_db);

}  // namespace audioprocess

