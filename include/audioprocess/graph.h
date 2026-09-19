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

struct GraphExport {
    std::string name;
    std::string node_id;
    std::string port_id;
};

struct GraphDefinition {
    std::vector<NodeDefinition> nodes;
    std::vector<Connection> connections;
    std::vector<GraphExport> exports;
    std::uint32_t schema_version{1};
};

}  // namespace audioprocess
