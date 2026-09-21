#include "audioprocess/realtime_bridge.h"

#include <algorithm>
#include <cmath>
#include <limits>
#include <stdexcept>

namespace audioprocess {
namespace {

void add_owned(std::atomic<std::uint64_t>& counter, std::uint64_t amount) noexcept {
    // 只有所属回调线程写入，不需要跨线程 read-modify-write 或 CAS 重试。
    counter.store(counter.load(std::memory_order_relaxed) + amount, std::memory_order_relaxed);
}

} // namespace

RealtimeBridge::RealtimeBridge(RealtimeBridgeConfig config, RealtimeGraphExecutor* plan)
    : config_(config), plan_(plan) {
    const auto capacity = config.capacity_frames;
    if (config.sample_rate != 48000 || capacity < 4 || capacity > (1U << 20U) ||
        (capacity & (capacity - 1U)) != 0 || config.target_frames < 2 || config.target_frames > capacity - 2) {
        throw std::invalid_argument("Realtime bridge requires 48kHz, power-of-two capacity 4..1048576 and target 2..capacity-2");
    }
    processing_frames_ = plan_ ? plan_->max_block_frames() : 256;
    if (processing_frames_ == 0 || processing_frames_ > 65536) {
        throw std::invalid_argument("Realtime bridge requires a prepared plan with maximum block size 1..65536");
    }
    ring_.resize(capacity); // 所有音频缓存分配发生在设备启动前。
    processing_buffer_.resize(processing_frames_);
    mask_ = static_cast<std::uint64_t>(capacity - 1U);
    muted_.store(config.muted, std::memory_order_relaxed);
    reset();
}

void RealtimeBridge::set_muted(bool muted) noexcept {
    muted_.store(muted, std::memory_order_relaxed);
}

void RealtimeBridge::capture(const float* mono, std::uint32_t frames) noexcept {
    if (frames == 0) return;
    const auto write = write_position_.load(std::memory_order_relaxed);
    const auto read = read_position_.load(std::memory_order_acquire);
    const auto occupied = std::min<std::uint64_t>(write - read, config_.capacity_frames);
    const auto accepted = std::min<std::uint64_t>(frames, config_.capacity_frames - occupied);
    std::uint64_t sanitized = 0;
    float peak = 0.0F;
    for (std::uint64_t index = 0; index < frames; ++index) {
        float sample = mono ? mono[index] : 0.0F;
        if (!std::isfinite(sample)) { sample = 0.0F; ++sanitized; }
        peak = std::max(peak, std::abs(sample));
        if (index < accepted) ring_[static_cast<std::size_t>((write + index) & mask_)] = sample;
    }
    // 先完成样本写入，再发布写游标，消费者 acquire 后才读这些样本。
    write_position_.store(write + accepted, std::memory_order_release);
    add_owned(capture_frames_, frames);
    add_owned(dropped_frames_, static_cast<std::uint64_t>(frames) - accepted);
    add_owned(capture_sanitized_, sanitized);
    capture_peak_.store(peak, std::memory_order_relaxed);
}

void RealtimeBridge::render(float* output, std::uint32_t frames, std::uint32_t channels) noexcept {
    if (frames == 0) return;
    if (!output || (channels != 1 && channels != 2) ||
        frames > std::numeric_limits<std::size_t>::max() / channels) {
        add_owned(invalid_render_calls_, 1);
        return;
    }
    add_owned(render_frames_, frames);
    if (faulted()) {
        std::fill_n(output, static_cast<std::size_t>(frames) * channels, 0.0F);
        output_peak_.store(0.0F, std::memory_order_relaxed);
        return;
    }
    auto read = read_position_.load(std::memory_order_relaxed);
    const auto write = write_position_.load(std::memory_order_acquire);
    const auto available = write - read;
    const bool muted = muted_.load(std::memory_order_relaxed);

    if (buffering_) {
        if (available < config_.target_frames) {
            std::fill_n(output, static_cast<std::size_t>(frames) * channels, 0.0F);
            add_owned(buffering_frames_, frames);
            output_peak_.store(0.0F, std::memory_order_relaxed);
            return;
        }
        buffering_ = false;
    }

    // 调节渲染后预计剩余队列量，纠正两个设备的独立时钟。
    // 这是轻量线性插值/比例控制，不替代高质量通用重采样器，也不是实时期限保证。
    const auto error = static_cast<double>(available) - frames - config_.target_frames;
    const auto adjustment = std::clamp(error * 0.01 / config_.target_frames, -0.005, 0.005);
    const auto desired_ratio = static_cast<float>(1.0 + adjustment);
    ratio_ = std::clamp(ratio_ + (desired_ratio - ratio_) * 0.05F, 0.995F, 1.005F);
    published_ratio_.store(ratio_, std::memory_order_relaxed);

    std::uint64_t sanitized = 0;
    std::uint64_t clipped = 0;
    float peak = 0.0F;
    std::uint32_t output_frame{};
    while (output_frame < frames) {
        const auto requested = std::min(processing_frames_, frames - output_frame);
        std::uint32_t produced{};
        for (; produced < requested; ++produced) {
            if (write - read < 2) {
                // 缺音不假造输入推进有状态节点：只处理有效前缀，其余设备输出补零。
                buffering_ = true;
                phase_ = 0.0;
                break;
            }
            const float left = ring_[static_cast<std::size_t>(read & mask_)];
            const float right = ring_[static_cast<std::size_t>((read + 1U) & mask_)];
            // double 中间值避免相反的大幅有限采样相减溢出。
            float sample = static_cast<float>(static_cast<double>(left) +
                (static_cast<double>(right) - left) * phase_);
            if (!std::isfinite(sample)) { sample = 0.0F; ++sanitized; }
            processing_buffer_[produced] = sample;
            phase_ += ratio_;
            const auto consumed = static_cast<std::uint32_t>(phase_);
            phase_ -= consumed;
            read += consumed;
        }

        if (produced != 0 && plan_) {
            const auto result = plan_->process(std::span<float>{processing_buffer_.data(), produced});
            if (result.status != RealtimeProcessStatus::Ok) {
                // 单一 render 写者：先发布索引，再以 release 发布锁存状态。回调不构造文本。
                failed_node_.store(result.node_index, std::memory_order_relaxed);
                process_status_.store(result.status, std::memory_order_release);
                std::fill_n(output, static_cast<std::size_t>(frames) * channels, 0.0F);
                peak = 0.0F;
                break;
            }
        }

        for (std::uint32_t index = 0; index < produced; ++index) {
            float sample = processing_buffer_[index];
            if (!std::isfinite(sample)) { sample = 0.0F; ++sanitized; }
            if (sample < -1.0F || sample > 1.0F) { ++clipped; sample = std::clamp(sample, -1.0F, 1.0F); }
            if (muted) sample = 0.0F; // Probe 仍消费并执行 Graph，只有最终设备输出静音。
            peak = std::max(peak, std::abs(sample));
            const auto output_index = static_cast<std::size_t>(output_frame + index) * channels;
            output[output_index] = sample;
            if (channels == 2) output[output_index + 1] = sample;
        }
        output_frame += produced;
        if (produced < requested) {
            std::fill_n(output + static_cast<std::size_t>(output_frame) * channels,
                        static_cast<std::size_t>(frames - output_frame) * channels, 0.0F);
            add_owned(underflow_frames_, frames - output_frame);
            break;
        }
    }
    // 回调读完后才发布可重用区间，capture 不会覆盖本回调仍在使用的样本。
    read_position_.store(read, std::memory_order_release);
    add_owned(render_sanitized_, sanitized);
    add_owned(clipped_samples_, clipped);
    output_peak_.store(peak, std::memory_order_relaxed);
}

bool RealtimeBridge::faulted() const noexcept {
    return process_status_.load(std::memory_order_acquire) != RealtimeProcessStatus::Ok;
}

RealtimeProcessResult RealtimeBridge::process_fault() const noexcept {
    const auto status = process_status_.load(std::memory_order_acquire);
    if (status == RealtimeProcessStatus::Ok) { return {}; }
    return {status, failed_node_.load(std::memory_order_relaxed)};
}

RealtimeBridgeStats RealtimeBridge::stats() const noexcept {
    RealtimeBridgeStats result;
    result.capture_frames = capture_frames_.load(std::memory_order_relaxed);
    result.render_frames = render_frames_.load(std::memory_order_relaxed);
    result.dropped_frames = dropped_frames_.load(std::memory_order_relaxed);
    result.underflow_frames = underflow_frames_.load(std::memory_order_relaxed);
    result.buffering_silence_frames = buffering_frames_.load(std::memory_order_relaxed);
    result.sanitized_samples = capture_sanitized_.load(std::memory_order_relaxed) + render_sanitized_.load(std::memory_order_relaxed);
    result.clipped_samples = clipped_samples_.load(std::memory_order_relaxed);
    result.invalid_render_calls = invalid_render_calls_.load(std::memory_order_relaxed);
    const auto read = read_position_.load(std::memory_order_acquire);
    const auto write = write_position_.load(std::memory_order_acquire);
    result.queued_frames = static_cast<std::uint32_t>(std::min<std::uint64_t>(write - read, config_.capacity_frames));
    result.queue_latency_ms = 1000.0 * result.queued_frames / config_.sample_rate;
    result.resample_ratio = published_ratio_.load(std::memory_order_relaxed);
    result.capture_peak = capture_peak_.load(std::memory_order_relaxed);
    result.output_peak = output_peak_.load(std::memory_order_relaxed);
    return result;
}

void RealtimeBridge::reset() noexcept {
    // 前置条件：capture/render 均已停止。仅清传输历史；节点状态由调用方重新 prepare。
    write_position_.store(0, std::memory_order_relaxed);
    read_position_.store(0, std::memory_order_relaxed);
    phase_ = 0.0;
    ratio_ = 1.0F;
    buffering_ = true;
    failed_node_.store(UINT32_MAX, std::memory_order_relaxed);
    process_status_.store(RealtimeProcessStatus::Ok, std::memory_order_relaxed);
    capture_frames_.store(0, std::memory_order_relaxed);
    dropped_frames_.store(0, std::memory_order_relaxed);
    capture_sanitized_.store(0, std::memory_order_relaxed);
    render_frames_.store(0, std::memory_order_relaxed);
    underflow_frames_.store(0, std::memory_order_relaxed);
    buffering_frames_.store(0, std::memory_order_relaxed);
    render_sanitized_.store(0, std::memory_order_relaxed);
    clipped_samples_.store(0, std::memory_order_relaxed);
    invalid_render_calls_.store(0, std::memory_order_relaxed);
    published_ratio_.store(1.0F, std::memory_order_relaxed);
    capture_peak_.store(0.0F, std::memory_order_relaxed);
    output_peak_.store(0.0F, std::memory_order_relaxed);
}

} // namespace audioprocess
