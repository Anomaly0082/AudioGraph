#pragma once

#include "audioprocess/graph_validator.h"
#include "audioprocess/realtime_node.h"

#include <limits>
#include <optional>
#include <string_view>

namespace audioprocess {

struct RealtimeProcessResult {
    RealtimeProcessStatus status{RealtimeProcessStatus::Ok};
    // 处理器执行计划下标，不是 JSON nodes 下标。无特定节点时使用此哨兵。
    std::uint32_t node_index{std::numeric_limits<std::uint32_t>::max()};
};

[[nodiscard]] GraphValidationResult validate_realtime_graph(
    const GraphDefinition& graph, const NodeRegistry& registry,
    AudioFormat format = {48000, 1}, std::uint32_t maximum_frames = 256);

// 控制线程 compile/prepare；之后音频线程只使用预先准备的处理器数组。
// 首版实时图只接受一个设备源、线性处理器链和一个设备终点，不运行离线节点。
// 离线供块前调用方须检查各 Processor 的 offline_drivable；设备端点不因此变成文件节点。
class RealtimeGraphExecutor {
public:
    [[nodiscard]] static RealtimeGraphExecutor compile(const GraphDefinition& graph, const NodeRegistry& registry);
    // 仅在 process 未运行时调用。成功时全部换成新实例，等价于新建处理会话。
    void prepare(AudioFormat format = {48000, 1}, std::uint32_t maximum_frames = 256);
    [[nodiscard]] RealtimeProcessResult process(std::span<float> samples) noexcept;
    [[nodiscard]] std::string_view node_id(std::uint32_t index) const noexcept;
    [[nodiscard]] const std::string& input_device() const noexcept { return input_device_; }
    [[nodiscard]] const std::string& output_device() const noexcept { return output_device_; }
    [[nodiscard]] std::uint32_t max_block_frames() const noexcept { return maximum_frames_; }

private:
    RealtimeGraphExecutor() = default;
    struct PreparedProcessor {
        std::string id;
        std::unique_ptr<IRealtimeProcessor> instance;
    };
    NodeRegistry registry_;
    GraphDefinition graph_;
    std::vector<std::size_t> order_;
    std::vector<PreparedProcessor> processors_;
    std::string input_device_, output_device_;
    std::uint32_t maximum_frames_{};
    bool prepared_{};
};

void register_realtime_nodes(NodeRegistry& registry);
[[nodiscard]] GraphDefinition make_realtime_graph(
    const std::string& input_device, const std::string& output_device,
    std::optional<float> gain_db = std::nullopt);

} // namespace audioprocess
