#pragma once

#include "audioprocess/node.h"

namespace audioprocess {

// 整段离线格式适配。不涉及文件、设备或 Executor 调度；第三方 DSP 状态仅属于一次 execute。
class AudioResampleNode final : public ISyncNode {
public:
    explicit AudioResampleNode(const ParameterMap& parameters);
    [[nodiscard]] const NodeDescriptor& descriptor() const noexcept override;
    [[nodiscard]] OutputValues execute(
        const InputValues& inputs, ExecutionContext& context) override;
    [[nodiscard]] static NodeDescriptor make_descriptor();

private:
    std::uint32_t sample_rate_;
    NodeDescriptor descriptor_{make_descriptor()};
};

class AudioDownmixMonoNode final : public ISyncNode {
public:
    [[nodiscard]] const NodeDescriptor& descriptor() const noexcept override;
    [[nodiscard]] OutputValues execute(
        const InputValues& inputs, ExecutionContext& context) override;
    [[nodiscard]] static NodeDescriptor make_descriptor();

private:
    NodeDescriptor descriptor_{make_descriptor()};
};

void register_format_node_types(NodeRegistry& registry);

}  // namespace audioprocess
