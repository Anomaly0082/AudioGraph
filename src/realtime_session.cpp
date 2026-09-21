#include "audioprocess/realtime_session.h"

#include "audioprocess/execution_error.h"

#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <Windows.h>
#include <miniaudio/miniaudio.h>

#include <algorithm>
#include <atomic>
#include <optional>
#include <utility>

namespace audioprocess {
namespace {

[[noreturn]] void backend_error(const char* stage, ma_result result) {
    throw ExecutionError("realtime_backend_error", std::string(stage) + ": " +
        ma_result_description(result) + " (" + std::to_string(result) + ")");
}

// miniaudio 的 WASAPI ID 是原生宽字符串；协议边界一律转换为 UTF-8。
std::string device_id_utf8(const ma_device_id& id) {
    const auto count = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, id.wasapi, -1,
                                          nullptr, 0, nullptr, nullptr);
    if (count <= 1) { throw ExecutionError("invalid_device_id", "WASAPI returned an invalid endpoint ID"); }
    std::string result(static_cast<std::size_t>(count), '\0');
    if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, id.wasapi, -1,
                            result.data(), count, nullptr, nullptr) != count) {
        throw ExecutionError("invalid_device_id", "Failed to encode WASAPI endpoint ID as UTF-8");
    }
    result.resize(static_cast<std::size_t>(count - 1));
    return result;
}

class WasapiContext {
public:
    ~WasapiContext() { close(); }
    WasapiContext() = default;
    WasapiContext(const WasapiContext&) = delete;
    WasapiContext& operator=(const WasapiContext&) = delete;

    void open() {
        const ma_backend backend = ma_backend_wasapi;
        // 只提供一个 backend，初始化失败时绝不能回退到别的音频接口。
        const auto result = ma_context_init(&backend, 1, nullptr, &value_);
        if (result != MA_SUCCESS) { backend_error("initialize WASAPI context", result); }
        initialized_ = true;
    }
    void close() noexcept {
        if (initialized_) {
            ma_context_uninit(&value_);
            initialized_ = false;
        }
    }
    ma_context* get() noexcept { return &value_; }
private:
    ma_context value_{};
    bool initialized_{};
};

struct EnumeratedDevices {
    ma_device_info* outputs{};
    ma_uint32 output_count{};
    ma_device_info* inputs{};
    ma_uint32 input_count{};
};

EnumeratedDevices enumerate_native(ma_context* context) {
    EnumeratedDevices devices;
    const auto result = ma_context_get_devices(context, &devices.outputs, &devices.output_count,
                                               &devices.inputs, &devices.input_count);
    if (result != MA_SUCCESS) { backend_error("enumerate WASAPI devices", result); }
    return devices;
}

ma_device_id select_device(const ma_device_info* devices, ma_uint32 count,
                          const std::string& requested, const char* direction) {
    for (ma_uint32 i = 0; i < count; ++i) {
        if (device_id_utf8(devices[i].id) == requested) { return devices[i].id; }
    }
    throw ExecutionError("device_not_found", std::string("Selected ") + direction +
        " device is not available; no default fallback: " + requested);
}

} // namespace

class RealtimeSession::Impl {
public:
    ~Impl() { stop(); }

    void start(const GraphDefinition& graph, const NodeRegistry& registry,
               const RealtimeSessionConfig& config) {
        if (running_ || capture_initialized_ || playback_initialized_) {
            throw ExecutionError("invalid_lifecycle", "Realtime session is already started");
        }
        if (config.bridge.sample_rate != 48000 || config.device_period_frames < 32 ||
            config.device_period_frames > 2048) {
            throw ExecutionError("invalid_realtime_config", "Session requires 48000 Hz and device period 32..2048 frames");
        }
        if (config.graph_block_frames == 0 || config.graph_block_frames > 65536) {
            throw ExecutionError("invalid_block_size", "Realtime graph block frames must be between 1 and 65536");
        }
        // 必须先完成所有图校验、工厂创建和 prepare，再建立任何设备上下文或打开端点。
        auto next_plan = std::make_unique<RealtimeGraphExecutor>(RealtimeGraphExecutor::compile(graph, registry));
        next_plan->prepare({48000, 1}, config.graph_block_frames);
        std::vector<std::string> next_processor_ids;
        for (std::uint32_t index = 0; !next_plan->node_id(index).empty(); ++index) {
            next_processor_ids.emplace_back(next_plan->node_id(index));
        }
        const auto& input_id = next_plan->input_device();
        const auto& output_id = next_plan->output_device();
        if (input_id.empty() || output_id.empty() || input_id.find('\0') != std::string::npos ||
            output_id.find('\0') != std::string::npos) {
            throw ExecutionError("invalid_device_id", "Explicit nonempty input and output IDs are required");
        }
        auto bridge_config = config.bridge;
        bridge_config.muted = config.probe || bridge_config.muted;
        std::unique_ptr<RealtimeBridge> next_bridge;
        try { next_bridge = std::make_unique<RealtimeBridge>(bridge_config, next_plan.get()); }
        catch (const std::invalid_argument& error) {
            throw ExecutionError("invalid_realtime_config", error.what());
        }
        // move unique_ptr 不移动计划对象本身，已绑定的 Bridge 借用指针保持有效。
        plan_ = std::move(next_plan);
        bridge_ = std::move(next_bridge);
        processor_ids_ = std::move(next_processor_ids);
        last_stats_ = {};
        last_process_fault_ = {};
        info_ = {};
        fault_.store(false, std::memory_order_relaxed);
        expected_stop_.store(false, std::memory_order_release);
        try {
            context_.open();
            const auto devices = enumerate_native(context_.get());
            const auto input = select_device(devices.inputs, devices.input_count, input_id, "input");
            const auto output = select_device(devices.outputs, devices.output_count, output_id, "output");

            auto capture_config = ma_device_config_init(ma_device_type_capture);
            capture_config.capture.pDeviceID = &input;
            capture_config.capture.format = ma_format_f32;
            capture_config.capture.channels = 1;
            capture_config.capture.shareMode = ma_share_mode_shared;
            configure_common(capture_config, config.device_period_frames);
            capture_config.dataCallback = capture_callback;
            auto result = ma_device_init(context_.get(), &capture_config, &capture_);
            if (result != MA_SUCCESS) { backend_error("initialize selected input device", result); }
            capture_initialized_ = true;

            auto playback_config = ma_device_config_init(ma_device_type_playback);
            playback_config.playback.pDeviceID = &output;
            playback_config.playback.format = ma_format_f32;
            playback_config.playback.channels = 2;
            playback_config.playback.shareMode = ma_share_mode_shared;
            configure_common(playback_config, config.device_period_frames);
            playback_config.dataCallback = playback_callback;
            result = ma_device_init(context_.get(), &playback_config, &playback_);
            if (result != MA_SUCCESS) { backend_error("initialize selected output device", result); }
            playback_initialized_ = true;
            info_ = {capture_.capture.internalSampleRate, capture_.capture.internalChannels,
                     capture_.capture.internalPeriodSizeInFrames, playback_.playback.internalSampleRate,
                     playback_.playback.internalChannels, playback_.playback.internalPeriodSizeInFrames};

            // 先启动输出，它会在桥接队列积累目标水位前输出静音。
            result = ma_device_start(&playback_);
            if (result != MA_SUCCESS) { backend_error("start selected output device", result); }
            result = ma_device_start(&capture_);
            if (result != MA_SUCCESS) { backend_error("start selected input device", result); }
            if (fault_.load(std::memory_order_acquire)) {
                throw ExecutionError("device_stopped", "Selected audio device stopped during session startup");
            }
            running_ = true;
        } catch (...) {
            stop(); // 任何初始化/启动阶段失败都先停止并销毁已成功创建的设备。
            throw;
        }
    }

    void stop() noexcept {
        // 通知回调可能在 stop/uninit 期间执行；期望中的停止不能被误报为拔插故障。
        expected_stop_.store(true, std::memory_order_release);
        if (bridge_) { bridge_->set_muted(true); }
        if (capture_initialized_) { (void)ma_device_stop(&capture_); }
        if (playback_initialized_) { (void)ma_device_stop(&playback_); }
        // 两个设备全部 uninit 后，回调才不再访问 bridge 和 pUserData。
        if (capture_initialized_) { ma_device_uninit(&capture_); capture_initialized_ = false; }
        if (playback_initialized_) { ma_device_uninit(&playback_); playback_initialized_ = false; }
        running_ = false;
        if (bridge_) {
            last_stats_ = bridge_->stats();
            last_process_fault_ = bridge_->process_fault();
            bridge_.reset();
        }
        // 节点 ID 已在启动前复制为独立元数据；stop 不分配字符串，并及时销毁节点资源。
        plan_.reset();
        context_.close();
    }

    RealtimeBridgeStats snapshot() const noexcept { return bridge_ ? bridge_->stats() : last_stats_; }
    RealtimeSessionInfo session_info() const noexcept { return info_; }
    RealtimeProcessResult process_fault() const noexcept {
        return bridge_ ? bridge_->process_fault() : last_process_fault_;
    }
    bool faulted() const noexcept {
        return fault_.load(std::memory_order_acquire) || process_fault().status != RealtimeProcessStatus::Ok;
    }
    bool is_running() const noexcept { return running_; }
    std::string fault_code() const {
        switch (process_fault().status) {
        case RealtimeProcessStatus::NotPrepared: return "realtime_plan_not_prepared";
        case RealtimeProcessStatus::InvalidBlock: return "invalid_realtime_block";
        case RealtimeProcessStatus::NonFiniteInput: return "non_finite_realtime_input";
        case RealtimeProcessStatus::NodeFailed: return "realtime_node_failed";
        case RealtimeProcessStatus::NonFiniteOutput: return "non_finite_realtime_output";
        case RealtimeProcessStatus::Ok: break;
        }
        return fault_.load(std::memory_order_acquire) ? "device_fault" : "";
    }
    std::string fault_node_id() const {
        const auto index = process_fault().node_index;
        return index < processor_ids_.size() ? processor_ids_[index] : "";
    }
    std::string fault_message() const {
        const auto code = fault_code();
        if (code.empty()) { return {}; }
        if (code == "device_fault") {
            return "Selected WASAPI device stopped, rerouted or was interrupted; automatic fallback is disabled";
        }
        const auto node = fault_node_id();
        return "Realtime graph stopped with " + code + (node.empty() ? "" : " at node '" + node + "'");
    }

private:
    void configure_common(ma_device_config& config, std::uint32_t period_frames) {
        config.sampleRate = 48000;
        config.periodSizeInFrames = period_frames; // 设备可以协商其他原生周期，不假设回调等长。
        config.noFixedSizedCallback = MA_TRUE;
        config.wasapi.noAutoStreamRouting = MA_TRUE;
        config.wasapi.noAutoConvertSRC = MA_TRUE; // 原生采样率/声道由 miniaudio converter 适配。
        config.notificationCallback = notification_callback;
        config.pUserData = this;
    }

    static void capture_callback(ma_device* device, void*, const void* input, ma_uint32 frames) noexcept {
        auto& self = *static_cast<Impl*>(device->pUserData);
        self.bridge_->capture(static_cast<const float*>(input), frames);
    }
    static void playback_callback(ma_device* device, void* output, const void*, ma_uint32 frames) noexcept {
        auto& self = *static_cast<Impl*>(device->pUserData);
        if (self.fault_.load(std::memory_order_acquire)) { self.bridge_->set_muted(true); }
        self.bridge_->render(static_cast<float*>(output), frames, 2);
    }
    static void notification_callback(const ma_device_notification* notification) noexcept {
        auto& self = *static_cast<Impl*>(notification->pDevice->pUserData);
        if (self.expected_stop_.load(std::memory_order_acquire)) { return; }
        if (notification->type == ma_device_notification_type_stopped ||
            notification->type == ma_device_notification_type_rerouted ||
            notification->type == ma_device_notification_type_interruption_began) {
            // 不在设备通知线程停止/重启设备或分配错误文本，由控制线程轮询后 stop。
            self.fault_.store(true, std::memory_order_release);
        }
    }

    WasapiContext context_;
    ma_device capture_{};
    ma_device playback_{};
    std::unique_ptr<RealtimeGraphExecutor> plan_;
    std::vector<std::string> processor_ids_;
    std::unique_ptr<RealtimeBridge> bridge_;
    RealtimeBridgeStats last_stats_{};
    RealtimeProcessResult last_process_fault_{};
    RealtimeSessionInfo info_{};
    std::atomic<bool> expected_stop_{true};
    std::atomic<bool> fault_{false};
    bool capture_initialized_{};
    bool playback_initialized_{};
    bool running_{};
};

RealtimeSession::RealtimeSession() : impl_(std::make_unique<Impl>()) {}
RealtimeSession::~RealtimeSession() = default;

RealtimeDeviceCatalog RealtimeSession::enumerate_devices() {
    WasapiContext context;
    context.open();
    const auto native = enumerate_native(context.get());
    RealtimeDeviceCatalog catalog;
    const auto append = [](std::vector<RealtimeDeviceInfo>& output, const ma_device_info* devices, ma_uint32 count) {
        output.reserve(count);
        for (ma_uint32 i = 0; i < count; ++i) {
            output.push_back({device_id_utf8(devices[i].id), devices[i].name, devices[i].isDefault != MA_FALSE});
        }
    };
    append(catalog.inputs, native.inputs, native.input_count);
    append(catalog.outputs, native.outputs, native.output_count);
    return catalog;
}

void RealtimeSession::start(const GraphDefinition& graph, const NodeRegistry& registry,
                            const RealtimeSessionConfig& config) { impl_->start(graph, registry, config); }
void RealtimeSession::stop() noexcept { impl_->stop(); }
RealtimeBridgeStats RealtimeSession::snapshot() const noexcept { return impl_->snapshot(); }
RealtimeSessionInfo RealtimeSession::session_info() const noexcept { return impl_->session_info(); }
bool RealtimeSession::faulted() const noexcept { return impl_->faulted(); }
bool RealtimeSession::is_running() const noexcept { return impl_->is_running(); }
std::string RealtimeSession::fault_message() const { return impl_->fault_message(); }
std::string RealtimeSession::fault_code() const { return impl_->fault_code(); }
std::string RealtimeSession::fault_node_id() const { return impl_->fault_node_id(); }

} // namespace audioprocess
