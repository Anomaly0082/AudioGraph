#pragma once

#include "audioprocess/node.h"

namespace audioprocess {

// RNNoise 的整段离线适配器。底层帧缓冲和模型状态只在一次 execute 中存活；
// 不向 Graph/Executor 暴露第三方库类型，也不承担文件读取或格式转换。
class RnnNoiseDenoiseNode final : public ISyncNode {
public:
    [[nodiscard]] const NodeDescriptor& descriptor() const noexcept override;
    [[nodiscard]] OutputValues execute(
        const InputValues& inputs, ExecutionContext& context) override;

    [[nodiscard]] static NodeDescriptor make_descriptor();

private:
    NodeDescriptor descriptor_{make_descriptor()};
};

void register_denoise_node_type(NodeRegistry& registry);

}  // namespace audioprocess
