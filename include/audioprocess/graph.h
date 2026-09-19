#pragma once

#include "audioprocess/node.h"

#include <string>
#include <vector>

namespace audioprocess {

struct NodeDefinition {
    std::string id;
    std::string type_id;
    ParameterMap parameters;
};

struct Connection {
    std::string source_node;
    std::string source_port;
    std::string target_node;
    std::string target_port;
};

struct GraphDefinition {
    std::vector<NodeDefinition> nodes;
    std::vector<Connection> connections;
};

}  // namespace audioprocess

