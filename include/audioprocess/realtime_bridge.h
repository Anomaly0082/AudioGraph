#pragma once

#include <atomic>
#include <cstdint>
#include <vector>

namespace audioprocess {

struct RealtimeBridgeConfig {
    std::uint32_t sample_rate{48000};
    std::uint32_t capacity_frames{4096};
    std::uint32_t target_frames{960};
    float gain_db{0.0F};
    bool muted{false};
};

struct RealtimeBridgeStats {
    std::uint64_t capture_frames{};
    std::uint64_t render_frames{};
    std::uint64_t dropped_frames{};
    std::uint64_t underflow_frames{};
    std::uint64_t buffering_silence_frames{};
    std::uint64_t sanitized_samples{};
    std::uint64_t clipped_samples{}; // 单声道处理帧数，不因复制为双声道而翻倍。
    std::uint64_t invalid_render_calls{};
    std::uint32_t queued_frames{};
    double queue_latency_ms{}; // 仅软件环形队列，绝不是设备端到端延迟。
    float resample_ratio{1.0F}; // 每个输出帧消耗的输入帧，范围 [0.995, 1.005]。
    float capture_peak{};      // 最近一次 capture 调用，净化非有限值后的输入峰值。
    float output_peak{};       // 最近一次 render 调用，静音/限幅后的实际输出峰值。
};

// 固定 48kHz 单声道输入；render 可输出单声道或双声道复制。
// capture 与 render 各自只能有一个调用线程；控制线程可更新 gain/mute 或读取统计。
// 构造验证配置并预分配。只有设备回调完全停止后才可 reset 或析构。
class RealtimeBridge {
public:
    explicit RealtimeBridge(RealtimeBridgeConfig config = {});
    RealtimeBridge(const RealtimeBridge&) = delete;
    RealtimeBridge& operator=(const RealtimeBridge&) = delete;

    // null 输入表示 frames 帧静音。溢出时只接受可写前缀，不修改消费者游标。
    void capture(const float* mono, std::uint32_t frames) noexcept;
    // output 需容纳 frames*channels 个 float；channels 仅允许 1/2。
    // frames=0 是空操作；其他非法参数计数后返回，不写 output。
    void render(float* output, std::uint32_t frames, std::uint32_t channels = 2) noexcept;
    [[nodiscard]] bool set_gain_db(float gain_db) noexcept;
    void set_muted(bool muted) noexcept;
    [[nodiscard]] RealtimeBridgeStats stats() const noexcept;
    void reset() noexcept;

private:
    // 独立的逐帧增益处理器；所有状态由 render 线程独占。
    class RealtimeGainProcessor {
    public:
        void reset(float gain) noexcept;
        void set_target(float gain, std::uint32_t ramp_frames) noexcept;
        [[nodiscard]] float process(float sample) noexcept;
    private:
        float current_{1.0F}, target_{1.0F}, step_{};
        std::uint32_t remaining_{};
    };

    RealtimeBridgeConfig config_;
    std::vector<float> ring_;
    std::uint64_t mask_{};
    alignas(64) std::atomic<std::uint64_t> write_position_{0};
    alignas(64) std::atomic<std::uint64_t> read_position_{0};
    std::atomic<float> target_gain_{1.0F};
    std::atomic<bool> muted_{false};
    RealtimeGainProcessor gain_;
    double phase_{};
    float ratio_{1.0F};
    bool buffering_{true};

    // 每个计数只有一个回调写入；第三线程快照不要求各字段同一时刻。
    std::atomic<std::uint64_t> capture_frames_{0}, dropped_frames_{0}, capture_sanitized_{0};
    std::atomic<std::uint64_t> render_frames_{0}, underflow_frames_{0}, buffering_frames_{0};
    std::atomic<std::uint64_t> render_sanitized_{0}, clipped_samples_{0}, invalid_render_calls_{0};
    std::atomic<float> published_ratio_{1.0F}, capture_peak_{0.0F}, output_peak_{0.0F};
};

static_assert(std::atomic<std::uint64_t>::is_always_lock_free);
static_assert(std::atomic<float>::is_always_lock_free);
static_assert(std::atomic<bool>::is_always_lock_free);

} // namespace audioprocess
