#include "audioprocess/realtime_config.h"
#include "audioprocess/graph_codec.h"
#include <nlohmann/json.hpp>
#include <charconv>
#include <chrono>
#include <cmath>
#include <csignal>
#include <iostream>
#include <optional>
#include <set>
#include <thread>

namespace ap = audioprocess;
using Json = nlohmann::json;
namespace {
volatile std::sig_atomic_t interrupted = 0;
void on_interrupt(int) { interrupted = 1; }
struct Options {
    ap::RealtimeRouteConfig config;
    std::filesystem::path config_file;
    unsigned seconds{10};
    bool devices{}, validate{}, monitor{}, help{};
};
unsigned parse_integer(const std::string& value) {
    unsigned result{};
    const auto parsed = std::from_chars(value.data(), value.data() + value.size(), result);
    if (parsed.ec != std::errc{} || parsed.ptr != value.data() + value.size())
        throw ap::ExecutionError("invalid_arguments", "Expected an unsigned integer");
    return result;
}
Options parse_options(const std::vector<std::string>& args) {
    Options options;
    std::set<std::string> seen;
    for (std::size_t i = 1; i < args.size(); ++i) {
        const auto key = args[i];
        if (!seen.insert(key).second) throw ap::ExecutionError("invalid_arguments", "Duplicate option: " + key);
        if (key == "--help" || key == "-h") { options.help = true; continue; }
        if (key == "--list-devices") { options.devices = true; continue; }
        if (key == "--validate") { options.validate = true; continue; }
        if (key == "--probe") continue; // 默认强制静音，不能从配置文件绕过。
        if (key == "--monitor") { options.monitor = true; continue; }
        if (key != "--input" && key != "--output" && key != "--config" && key != "--gain-db" &&
            key != "--seconds" && key != "--target-frames" && key != "--period-frames")
            throw ap::ExecutionError("invalid_arguments", "Unknown option: " + key);
        if (++i == args.size() || args[i].empty()) throw ap::ExecutionError("invalid_arguments", "Missing option value");
        const auto& value = args[i];
        if (key == "--input") options.config.input_device = value;
        else if (key == "--output") options.config.output_device = value;
        else if (key == "--config") options.config_file = ap::path_from_utf8(value);
        else if (key == "--seconds") options.seconds = parse_integer(value);
        else if (key == "--target-frames") options.config.session.bridge.target_frames = parse_integer(value);
        else if (key == "--period-frames") options.config.session.device_period_frames = parse_integer(value);
        else {
            const auto parsed = std::from_chars(value.data(), value.data() + value.size(), options.config.session.bridge.gain_db);
            if (parsed.ec != std::errc{} || parsed.ptr != value.data() + value.size())
                throw ap::ExecutionError("invalid_arguments", "Expected numeric gain_db");
        }
    }
    if (options.help) return options;
    if (options.devices) {
        if (seen.size() != 1) throw ap::ExecutionError("invalid_arguments", "--list-devices is a standalone command");
        return options;
    }
    if (seen.contains("--monitor") && seen.contains("--probe"))
        throw ap::ExecutionError("invalid_arguments", "Choose --probe or --monitor, not both");
    if (options.seconds < 1 || options.seconds > 3600)
        throw ap::ExecutionError("invalid_arguments", "--seconds must be between 1 and 3600");
    if (!options.config_file.empty()) {
        for (const auto key : {"--input", "--output", "--gain-db", "--target-frames", "--period-frames"})
            if (seen.contains(key)) throw ap::ExecutionError("invalid_arguments", "Do not mix --config with direct device settings");
        options.config = ap::load_realtime_config(options.config_file);
    }
    ap::validate_realtime_config(options.config);
    options.config.session.probe = !options.monitor;
    return options;
}
Json stats_json(const ap::RealtimeBridgeStats& stats) {
    return {{"capture_frames", stats.capture_frames}, {"render_frames", stats.render_frames},
        {"dropped_frames", stats.dropped_frames}, {"underflow_frames", stats.underflow_frames},
        {"buffering_silence_frames", stats.buffering_silence_frames}, {"sanitized_samples", stats.sanitized_samples},
        {"clipped_samples", stats.clipped_samples}, {"invalid_render_calls", stats.invalid_render_calls},
        {"queued_frames", stats.queued_frames}, {"software_queue_latency_ms", stats.queue_latency_ms},
        {"resample_ratio", stats.resample_ratio}, {"capture_peak", stats.capture_peak}, {"output_peak", stats.output_peak}};
}
int run(const std::vector<std::string>& args) {
    try {
        const auto options = parse_options(args);
        if (options.help) {
            std::cout << "realtime-cli --list-devices\n"
                "realtime-cli --input <capture-id> --output <playback-id> [--probe | --monitor] [--gain-db -6] [--seconds 10]\n"
                "realtime-cli --config <session.json> [--validate] [--probe | --monitor] [--seconds 10]\n"
                "Default is a silent probe. --monitor sends microphone audio to the selected output.\n";
            return 0;
        }
        if (options.devices) {
            const auto catalog = ap::RealtimeSession::enumerate_devices();
            Json inputs = Json::array(), outputs = Json::array();
            for (const auto& item : catalog.inputs) inputs.push_back({{"id", item.id}, {"name", item.name}, {"is_default", item.is_default}});
            for (const auto& item : catalog.outputs) outputs.push_back({{"id", item.id}, {"name", item.name}, {"is_default", item.is_default}});
            std::cout << Json{{"schema_version", 1}, {"success", true}, {"backend", "wasapi"}, {"inputs", inputs}, {"outputs", outputs}}.dump() << '\n';
            return 0;
        }
        if (options.validate) {
            std::cout << "{\"schema_version\":1,\"success\":true,\"valid\":true,\"device_access\":false}\n";
            return 0;
        }
        std::signal(SIGINT, on_interrupt);
        ap::RealtimeSession session;
        session.start(options.config.input_device, options.config.output_device, options.config.session);
        const auto started = std::chrono::steady_clock::now();
        auto next_report = started + std::chrono::seconds(1);
        while (!interrupted && !session.faulted() &&
               std::chrono::steady_clock::now() - started < std::chrono::seconds(options.seconds)) {
            std::this_thread::sleep_for(std::chrono::milliseconds(50));
            if (std::chrono::steady_clock::now() >= next_report) {
                std::cerr << Json{{"event", "metrics"}, {"stats", stats_json(session.snapshot())}}.dump() << '\n';
                next_report += std::chrono::seconds(1);
            }
        }
        session.stop();
        const auto stats = session.snapshot();
        const auto info = session.session_info();
        const bool fault = session.faulted();
        // 成功启动不等于音频回调真实运行；探测需要确认两个方向都有帧。
        const bool no_frames = stats.capture_frames == 0 || stats.render_frames == 0;
        const bool success = !fault && !no_frames;
        Json result{{"schema_version", 1}, {"success", success}, {"probe", options.config.session.probe},
                    {"stopped_by", interrupted ? "interrupt" : (fault ? "device_fault" : "duration")},
                    {"stats", stats_json(stats)}};
        result["device_format"] = {
            {"capture_native_sample_rate", info.capture_native_sample_rate},
            {"capture_native_channels", info.capture_native_channels},
            {"capture_native_period_frames", info.capture_native_period_frames},
            {"playback_native_sample_rate", info.playback_native_sample_rate},
            {"playback_native_channels", info.playback_native_channels},
            {"playback_native_period_frames", info.playback_native_period_frames},
            {"internal_sample_rate", 48000}};
        if (!success) result["errors"] = Json::array({{{"code", fault ? "device_fault" : "no_audio_callbacks"},
            {"message", fault ? session.fault_message() : "No capture or render frames observed"}}});
        std::cout << result.dump() << '\n';
        return success ? 0 : 1;
    } catch (const ap::ExecutionError& error) {
        std::cout << ap::error_to_json(error) << '\n';
        std::cerr << error.what() << '\n';
    } catch (const std::exception& error) {
        std::cout << ap::error_to_json(ap::ExecutionError("realtime_failed", error.what())) << '\n';
        std::cerr << error.what() << '\n';
    }
    return 1;
}
}
#ifdef _WIN32
int wmain(int argc, wchar_t* argv[]) {
    std::vector<std::string> args;
    for (int i = 0; i < argc; ++i) args.push_back(ap::path_to_utf8(std::filesystem::path(argv[i])));
    return run(args);
}
#else
int main(int argc, char* argv[]) { return run({argv, argv + argc}); }
#endif
