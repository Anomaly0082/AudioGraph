#pragma once

#include "audioprocess/node.h"

namespace audioprocess {

// 注册 PCM16 文件源、分块 Gain 和 PCM16 文件终点；工厂构造阶段不执行文件 IO。
void register_streaming_node_types(NodeRegistry& registry);

} // namespace audioprocess
