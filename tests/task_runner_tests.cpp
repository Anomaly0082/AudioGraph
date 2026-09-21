#include "audioprocess/task_runner.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/wav_file.h"

#include <nlohmann/json.hpp>
#include <atomic>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>
#include <stdexcept>
#include <string>

namespace {
using namespace audioprocess;
using Json = nlohmann::json;

void require(bool condition, const char* message) {
    if (!condition) throw std::runtime_error(message);
}
TaskOutcome run(const TaskRequest& request, const NodeRegistry& registry, std::atomic_bool& cancelled) {
    try { return execute_task_request(request, registry, cancelled); }
    catch (const ExecutionError& error) {
        return {"", TaskError{error.code, error.what(), error.node_id, error.port_id, error.parameter_id, error.field_path}};
    }
}
std::string bytes(const std::filesystem::path& path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) throw std::runtime_error("Cannot read test output");
    return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}

class TemporaryDirectory {
public:
    TemporaryDirectory() {
        const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
        for (unsigned attempt = 0; attempt < 100; ++attempt) {
            auto candidate = std::filesystem::temp_directory_path() /
                ("audioprocess_task_runner_" + std::to_string(stamp) + "_" + std::to_string(attempt));
            if (std::filesystem::create_directory(candidate)) { path = std::move(candidate); return; }
        }
        throw std::runtime_error("Cannot create isolated test directory");
    }
    ~TemporaryDirectory() {
        std::error_code ignored;
        std::filesystem::remove_all(path, ignored); // 只删除本测试原子创建的目录。
    }
    std::filesystem::path path;
};

NodeRegistry registry() {
    auto nodes = create_prototype_node_registry();
    register_realtime_nodes(nodes);
    return nodes;
}

void test_text_output_and_pre_cancel(const std::filesystem::path& directory) {
    const auto nodes = registry();
    const auto path = directory / std::filesystem::path(u8"文本结果.txt");
    const auto encoded = u8"任务接口的文本测试\nText → file\n";
    const std::string text(reinterpret_cast<const char*>(encoded));
    TaskRequest request;
    request.graph = {{{"text", "text_input", {{"text", text}}},
                      {"file", "text_output", {{"path", path}}}},
                     {{"text", "text", "file", "text"}}, {{"file", "file", "path"}}};
    validate_task_request(request, nodes);
    require(!std::filesystem::exists(path), "Pure task validation wrote a text file");
    std::atomic_bool cancelled{false};
    const auto result = run(request, nodes, cancelled);
    require(!result.error && Json::parse(result.result_json).at("success").get<bool>(), "Text runner failed");
    require(bytes(path) == text, "Task runner changed UTF-8 text bytes");
    const auto repeated = run(request, nodes, cancelled);
    require(repeated.error && repeated.error->node_id == "file" && bytes(path) == text,
        "Runner overwrote an existing output or lost file-node failure location");

    const auto cancelled_file = directory / "cancelled.txt";
    request.graph.nodes[1].parameters["path"] = cancelled_file;
    cancelled.store(true);
    const auto stopped = run(request, nodes, cancelled);
    require(stopped.error && stopped.error->code == "cancelled" && !std::filesystem::exists(cancelled_file),
        "Pre-cancelled task wrote output");
}

void test_whole_and_stream_wav(const std::filesystem::path& directory) {
    const auto nodes = registry();
    const auto input = directory / "input.wav";
    {
        AudioBuffer buffer({48000, 1}, 301);
        auto block = buffer.block(301);
        for (std::size_t i = 0; i < block.samples.size(); ++i)
            block.samples[i] = i % 2 == 0 ? 0.125F : -0.125F;
        WavFileSink sink(input, {48000, 1}, 301);
        sink.write(block); sink.finalize();
    }
    std::atomic_bool cancelled{false};
    TaskRequest whole;
    const auto whole_path = directory / "whole.wav";
    whole.graph = create_prototype_graph(input, whole_path, 6.020599913);
    const auto first = run(whole, nodes, cancelled);
    require(!first.error, "Whole-value WAV runner failed");
    const auto first_json = Json::parse(first.result_json);
    require(std::abs(first_json.at("outputs").at("peak").at("value").get<double>() - 0.25) < 0.00001,
        "Whole-value task result did not expose its expected peak");

    TaskRequest stream;
    stream.mode = ExecutionDomain::Streaming;
    stream.block_frames = 7;
    const auto stream_path = directory / "stream.wav";
    stream.graph = {{{"src", "wav_stream_input", {{"path", input}}},
                     {"gain", "stream_gain", {{"gain_db", 6.020599913}}},
                     {"dst", "wav_stream_output", {{"path", stream_path}}}},
                    {{"src", "audio", "gain", "audio"}, {"gain", "audio", "dst", "audio"}},
                    {{"frames", "dst", "frames_written"}}};
    const auto second = run(stream, nodes, cancelled);
    require(!second.error, "Streaming WAV runner failed");
    require(Json::parse(second.result_json).at("outputs").at("frames").at("value").get<double>() == 301,
        "Streaming task lost its final partial block");
    require(bytes(whole_path) == bytes(stream_path), "Whole/stream task adapters produced different PCM16 data");

    TaskRequest missing;
    missing.graph = create_prototype_graph(directory / "missing.wav", directory / "not_written.wav", 0);
    validate_task_request(missing, nodes); // 文件可用性属于运行阶段，不应由纯验证打开。
    require(!std::filesystem::exists(directory / "not_written.wav"), "Validation touched a file output");
}

struct Counts { int factories{}, prepared{}, live{}; };
class FailBeforeDevices final : public IRealtimeProcessor {
public:
    FailBeforeDevices(NodeDescriptor descriptor, std::shared_ptr<Counts> counts)
        : descriptor_(std::move(descriptor)), counts_(std::move(counts)) { ++counts_->live; }
    ~FailBeforeDevices() override { --counts_->live; }
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    void prepare(AudioFormat, std::uint32_t) override {
        ++counts_->prepared;
        throw ExecutionError("intentional_prepare_failure", "Do not open any device");
    }
    RealtimeProcessStatus process(std::span<float>) noexcept override { return RealtimeProcessStatus::NodeFailed; }
private:
    NodeDescriptor descriptor_;
    std::shared_ptr<Counts> counts_;
};

void test_realtime_rejected_before_device_access() {
    auto nodes = registry();
    auto counts = std::make_shared<Counts>();
    NodeDescriptor descriptor;
    descriptor.type_id = "fail_before_device";
    descriptor.execution_domain = ExecutionDomain::Realtime;
    descriptor.realtime_role = RealtimeRole::Processor;
    descriptor.realtime_capabilities = RealtimeCapabilities{};
    descriptor.inputs = {{"audio", DataType::AudioStream}};
    descriptor.outputs = {{"audio", DataType::AudioStream}};
    nodes.register_realtime_type(descriptor, [descriptor, counts](const ParameterMap&) {
        ++counts->factories;
        return std::make_unique<FailBeforeDevices>(descriptor, counts);
    });
    TaskRequest request;
    request.mode = ExecutionDomain::Realtime;
    request.graph = {{{"input", "realtime_input", {{"device_id", std::string("must-not-open")}}},
                      {"broken", "fail_before_device", {}},
                      {"output", "realtime_output", {{"device_id", std::string("must-not-open")}}}},
                     {{"input", "audio", "broken", "audio"}, {"broken", "audio", "output", "audio"}}};
    validate_task_request(request, nodes);
    require(counts->factories == 0, "Task validation instantiated a realtime processor");
    std::atomic_bool cancelled{true};
    const auto precancel = run(request, nodes, cancelled);
    require(precancel.error && precancel.error->code == "cancelled" && counts->factories == 0,
        "Pre-cancelled realtime task reached preparation");

    auto malformed = request;
    malformed.graph.connections[0].source_port = "missing";
    cancelled.store(false);
    const auto bad_graph = run(malformed, nodes, cancelled);
    require(bad_graph.error && bad_graph.error->code == "unknown_port" && counts->factories == 0,
        "Invalid realtime graph reached device preparation");
#ifdef _WIN32
    // 永远在processor.prepare抛错，故意不可达设备初始化；不做静音探测或设备枚举。
    const auto failed = run(request, nodes, cancelled);
    require(failed.error && failed.error->code == "intentional_prepare_failure" &&
        failed.error->node_id == "broken" && counts->factories == 1 && counts->prepared == 1 && counts->live == 0,
        "Realtime runner lost preparation failure location or retained resources");
    const auto json = Json::parse(failed.result_json);
    require(!json.at("success").get<bool>() && json.at("stopped_by") == "startup_failed" &&
        json.at("stats").at("capture_frames").get<std::uint64_t>() == 0 &&
        json.at("stats").at("render_frames").get<std::uint64_t>() == 0,
        "Failed preparation reported audio callbacks or lost diagnostic payload");
#endif
}
} // namespace

int main() {
    try {
        TemporaryDirectory directory;
        test_text_output_and_pre_cancel(directory.path);
        test_whole_and_stream_wav(directory.path);
        test_realtime_rejected_before_device_access();
        std::cout << "Independent task runner tests passed (no devices accessed).\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Task runner test failure: " << error.what() << '\n';
        return 1;
    }
}
