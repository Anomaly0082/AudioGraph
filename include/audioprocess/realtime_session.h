#pragma once

#include "audioprocess/realtime_bridge.h"

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace audioprocess {

struct RealtimeDeviceInfo {
    std::string id;   // 稳定的 WASAPI endpoint ID，UTF-8；不是临时列表下标。
    std::string name;
    bool is_default{};
};

struct RealtimeDeviceCatalog {
    std::vector<RealtimeDeviceInfo> inputs;
    std::vector<RealtimeDeviceInfo> outputs;
};

struct RealtimeSessionConfig {
    RealtimeBridgeConfig bridge;
    bool probe{true}; // 探测强制静音，不能被 bridge.muted=false 绕过。
    std::uint32_t device_period_frames{256};
    std::uint32_t graph_block_frames{256}; // 计划工作缓冲上限，独立于设备的原生回调长度。
};

struct RealtimeSessionInfo {
    std::uint32_t capture_native_sample_rate{};
    std::uint32_t capture_native_channels{};
    std::uint32_t capture_native_period_frames{};
    std::uint32_t playback_native_sample_rate{};
    std::uint32_t playback_native_channels{};
    std::uint32_t playback_native_period_frames{};
};

// 控制线程使用的 WASAPI 会话；执行已准备的实时 Graph，不自动选择/替换端点。
// start/stop/snapshot 等公共方法由同一控制线程调用。内部音频回调仅操作 Bridge/原子状态。
class RealtimeSession {
public:
    RealtimeSession();
    ~RealtimeSession();
    RealtimeSession(const RealtimeSession&) = delete;
    RealtimeSession& operator=(const RealtimeSession&) = delete;

    // 只枚举设备；不启动音频采集或播放。
    [[nodiscard]] static RealtimeDeviceCatalog enumerate_devices();
    void start(const GraphDefinition& graph, const NodeRegistry& registry,
               const RealtimeSessionConfig& config = {});
    void stop() noexcept;
    [[nodiscard]] RealtimeBridgeStats snapshot() const noexcept;
    // native 指 WASAPI 协商格式；period 为设备周期提示，不代表端到端延迟。
    [[nodiscard]] RealtimeSessionInfo session_info() const noexcept;
    [[nodiscard]] bool faulted() const noexcept;
    [[nodiscard]] bool is_running() const noexcept;
    [[nodiscard]] std::string fault_message() const;
    [[nodiscard]] std::string fault_code() const;
    [[nodiscard]] std::string fault_node_id() const;

private:
    class Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace audioprocess
