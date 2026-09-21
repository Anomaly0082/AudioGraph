#include "audioprocess/task_runner.h"

#include "audioprocess/execution_error.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/graph_validator.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/streaming_graph_executor.h"
#include <utility>

#ifdef _WIN32
#include "audioprocess/realtime_session.h"
#include <nlohmann/json.hpp>
#include <chrono>
#include <thread>
#endif

namespace audioprocess {
namespace {

void check_cancelled(const std::atomic_bool& cancellation_requested) {
    if (cancellation_requested.load(std::memory_order_acquire)) {
        throw ExecutionError("cancelled", "Task cancellation requested; partial output files may remain");
    }
}

GraphDefinition validated_graph(const TaskRequest& request, const NodeRegistry& registry) {
    if (request.block_frames == 0 || request.block_frames > 65536) {
        throw ExecutionError("invalid_block_size", "Task block_frames must be between 1 and 65536", {}, {}, {}, "/block_frames");
    }
    switch (request.mode) {
    case ExecutionDomain::Synchronous:
    case ExecutionDomain::Streaming:
        if (!request.probe || request.duration_seconds != 10) {
            throw ExecutionError("invalid_task_options", "probe and duration_seconds apply only to realtime tasks");
        }
        if (request.mode == ExecutionDomain::Synchronous) {
            if (request.block_frames != 256) {
                throw ExecutionError("invalid_task_options", "Synchronous whole-value tasks do not accept a custom block_frames",
                                     {}, {}, {}, "/block_frames");
            }
            return validate_graph(request.graph, registry).graph;
        }
        return validate_stream_graph(request.graph, registry).graph;
    case ExecutionDomain::Realtime:
        if (request.duration_seconds < 1 || request.duration_seconds > 3600) {
            throw ExecutionError("invalid_task_options", "Realtime duration_seconds must be between 1 and 3600",
                                 {}, {}, {}, "/duration_seconds");
        }
        return validate_realtime_graph(request.graph, registry, {48000, 1}, request.block_frames).graph;
    case ExecutionDomain::Asynchronous:
        break;
    }
    throw ExecutionError("unsupported_task_mode", "Supported task modes are synchronous, streaming and realtime", {}, {}, {}, "/mode");
}

#ifdef _WIN32
using Json = nlohmann::json;

TaskError task_error(const ExecutionError& error) {
    return {error.code, error.what(), error.node_id, error.port_id, error.parameter_id, error.field_path};
}

Json error_json(const TaskError& error) {
    Json detail{{"code", error.code}, {"message", error.message}};
    if (!error.node_id.empty()) { detail["node_id"] = error.node_id; }
    if (!error.port_id.empty()) { detail["port_id"] = error.port_id; }
    if (!error.parameter_id.empty()) { detail["parameter_id"] = error.parameter_id; }
    if (!error.field_path.empty()) { detail["field_path"] = error.field_path; }
    return detail;
}

TaskOutcome realtime_outcome(const RealtimeSession& session, bool probe, const char* stopped_by,
                              std::optional<TaskError> error = {}) {
    const auto stats = session.snapshot();
    const auto info = session.session_info();
    Json result{{"schema_version", 1}, {"success", !error.has_value()}, {"mode", "realtime"},
                {"probe", probe}, {"stopped_by", stopped_by}};
    result["stats"] = {
        {"capture_frames", stats.capture_frames}, {"render_frames", stats.render_frames},
        {"dropped_frames", stats.dropped_frames}, {"underflow_frames", stats.underflow_frames},
        {"buffering_silence_frames", stats.buffering_silence_frames}, {"sanitized_samples", stats.sanitized_samples},
        {"clipped_samples", stats.clipped_samples}, {"invalid_render_calls", stats.invalid_render_calls},
        {"queued_frames", stats.queued_frames}, {"software_queue_latency_ms", stats.queue_latency_ms},
        {"resample_ratio", stats.resample_ratio}, {"capture_peak", stats.capture_peak}, {"output_peak", stats.output_peak}};
    result["device_format"] = {
        {"capture_native_sample_rate", info.capture_native_sample_rate},
        {"capture_native_channels", info.capture_native_channels},
        {"capture_native_period_frames", info.capture_native_period_frames},
        {"playback_native_sample_rate", info.playback_native_sample_rate},
        {"playback_native_channels", info.playback_native_channels},
        {"playback_native_period_frames", info.playback_native_period_frames},
        {"internal_sample_rate", 48000}, {"internal_capture_channels", 1}, {"internal_playback_channels", 2}};
    if (error) { result["errors"] = Json::array({error_json(*error)}); }
    // 系统异常文本可能使用本地编码；替换非法 UTF-8，保持结果可供机器解析。
    return {result.dump(-1, ' ', false, Json::error_handler_t::replace), std::move(error)};
}

TaskOutcome execute_realtime(const TaskRequest& request, const GraphDefinition& graph,
                             const NodeRegistry& registry, std::atomic_bool& cancellation_requested) {
    check_cancelled(cancellation_requested);
    RealtimeSessionConfig config;
    config.probe = request.probe;
    config.graph_block_frames = request.block_frames;
    RealtimeSession session;
    try {
        check_cancelled(cancellation_requested);
        // Session 自己先完成所有 processor.prepare，之后才打开显式设备。
        session.start(graph, registry, config);
    } catch (const ExecutionError& error) {
        session.stop();
        return realtime_outcome(session, config.probe, "startup_failed", task_error(error));
    }

    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(request.duration_seconds);
    while (!cancellation_requested.load(std::memory_order_acquire) && !session.faulted() &&
           std::chrono::steady_clock::now() < deadline) {
        // 只阻塞执行此任务的 worker；TaskService 的 status/cancel 不等待这个循环。
        std::this_thread::sleep_for(std::chrono::milliseconds(20));
    }
    session.stop();
    if (cancellation_requested.load(std::memory_order_acquire)) {
        return realtime_outcome(session, config.probe, "cancelled",
            TaskError{"cancelled", "Realtime task cancellation requested", {}, {}, {}, {}});
    }
    if (session.faulted()) {
        const auto code = session.fault_code();
        return realtime_outcome(session, config.probe, code == "device_fault" ? "device_fault" : "graph_fault",
            TaskError{code, session.fault_message(), session.fault_node_id(), {}, {}, {}});
    }
    const auto stats = session.snapshot();
    if (stats.capture_frames == 0 || stats.render_frames == 0) {
        return realtime_outcome(session, config.probe, "duration",
            TaskError{"no_audio_callbacks", "No capture or render frames observed", {}, {}, {}, {}});
    }
    return realtime_outcome(session, config.probe, "duration");
}
#endif

} // namespace

void validate_task_request(const TaskRequest& request, const NodeRegistry& registry) {
    (void)validated_graph(request, registry);
}

TaskOutcome execute_task_request(const TaskRequest& request, const NodeRegistry& registry,
                                  std::atomic_bool& cancellation_requested) {
    check_cancelled(cancellation_requested);
    const auto graph = validated_graph(request, registry);
    check_cancelled(cancellation_requested);
    if (request.mode == ExecutionDomain::Realtime) {
#ifdef _WIN32
        return execute_realtime(request, graph, registry, cancellation_requested);
#else
        throw ExecutionError("unsupported_platform", "Realtime device tasks currently require Windows WASAPI");
#endif
    }

    // 运行阶段才检查文件系统目标；纯 validate 不触及这些 IO 或创建节点实例。
    validate_prototype_file_targets(graph);
    check_cancelled(cancellation_requested);
    const ExecutionContext context{&cancellation_requested};
    if (request.mode == ExecutionDomain::Synchronous) {
        auto executor = SyncGraphExecutor::compile(graph, registry);
        const auto result = executor.execute(context);
        return {execution_result_json(graph, result), {}};
    }
    auto executor = StreamingGraphExecutor::compile(graph, registry);
    const auto result = executor.execute(request.block_frames, context);
    return {execution_result_json(graph, result), {}};
}

} // namespace audioprocess
