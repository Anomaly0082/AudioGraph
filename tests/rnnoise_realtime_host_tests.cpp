#include "audioprocess/plugin_host.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/realtime_bridge.h"
#include <rnnoise.h>
#include <nlohmann/json.hpp>
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <bcrypt.h>
#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <vector>

namespace {
using namespace audioprocess;
using Json = nlohmann::json;
namespace fs = std::filesystem;
constexpr char type[] = "org.audiograph.rnnoise.realtime_v1";
void check(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
std::string utf8(const fs::path& path) {
    const auto value = path.u8string();
    return {reinterpret_cast<const char*>(value.data()), value.size()};
}
std::string hash(const fs::path& path) {
    BCRYPT_ALG_HANDLE algorithm{};
    BCRYPT_HASH_HANDLE state{};
    check(BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) >= 0, "SHA256 provider");
    check(BCryptCreateHash(algorithm, &state, nullptr, 0, nullptr, 0, 0) >= 0, "SHA256 state");
    std::ifstream input(path, std::ios::binary);
    check(bool(input), "Snapshot input");
    std::array<unsigned char, 4096> bytes{};
    while (input) {
        input.read(reinterpret_cast<char*>(bytes.data()), bytes.size());
        if (input.gcount()) check(BCryptHashData(state, bytes.data(), static_cast<ULONG>(input.gcount()), 0) >= 0, "SHA256 update");
    }
    std::array<unsigned char, 32> digest{};
    check(BCryptFinishHash(state, digest.data(), static_cast<ULONG>(digest.size()), 0) >= 0, "SHA256 finish");
    BCryptDestroyHash(state);
    BCryptCloseAlgorithmProvider(algorithm, 0);
    std::string output;
    constexpr char hex[] = "0123456789abcdef";
    for (auto byte : digest) { output += hex[byte >> 4]; output += hex[byte & 15]; }
    return output;
}
struct Temp {
    fs::path root = fs::temp_directory_path() / ("rnnoise-host-" + std::to_string(GetCurrentProcessId()) + "-" +
        std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
    Temp() { fs::create_directories(root / "workspace"); fs::create_directories(root / "data"); }
    ~Temp() { std::error_code error; fs::remove_all(root, error); }
};
GraphDefinition graph() {
    auto definition = make_realtime_graph("synthetic-input", "synthetic-output", 0.0f);
    definition.nodes[1].type_id = type;
    definition.nodes[1].parameters.clear();
    return definition;
}
std::vector<float> signal() {
    std::vector<float> samples(480 * 16);
    for (std::size_t i = 0; i < samples.size(); ++i) {
        const auto t = static_cast<double>(i) / 48000.0;
        samples[i] = static_cast<float>(0.2 * std::sin(2.0 * 3.141592653589793 * 220.0 * t) +
            0.025 * std::sin(2.0 * 3.141592653589793 * 7103.0 * t));
    }
    return samples;
}
std::vector<float> oracle(const std::vector<float>& input) {
    std::unique_ptr<DenoiseState, decltype(&rnnoise_destroy)> state(rnnoise_create(nullptr), &rnnoise_destroy);
    check(bool(state) && rnnoise_get_frame_size() == 480, "RNNoise oracle initialization");
    std::vector<float> expected(input.size(), 0.0f);
    std::array<float, 480> frame{}, output{};
    for (std::size_t offset = 0; offset < input.size(); offset += frame.size()) {
        for (std::size_t i = 0; i < frame.size(); ++i) frame[i] = input[offset + i] * 32768.0f;
        (void)rnnoise_process_frame(state.get(), output.data(), frame.data());
        if (offset + 480 < input.size()) {
            for (std::size_t i = 0; i < frame.size(); ++i) expected[offset + 480 + i] = output[i] / 32768.0f;
        }
    }
    return expected;
}
void compare(RealtimeGraphExecutor& plan, const std::vector<float>& input, const std::vector<float>& expected,
             const std::vector<std::size_t>& sizes) {
    auto actual = input;
    for (std::size_t offset = 0, index = 0; offset < actual.size(); ++index) {
        const auto count = std::min(sizes[index % sizes.size()], actual.size() - offset);
        check(plan.process(std::span<float>(actual).subspan(offset, count)).status == RealtimeProcessStatus::Ok,
              "Host RNNoise process failed");
        offset += count;
    }
    for (std::size_t i = 0; i < actual.size(); ++i) {
        check(std::isfinite(actual[i]) && std::abs(actual[i] - expected[i]) <= 2.0e-5f,
              "Host output differs from public RNNoise oracle plus 480-frame cache delay");
    }
}
void run(const fs::path& package) {
    Temp temp;
    const auto snapshot = temp.root / "snapshot.json";
    {
        std::ofstream output(snapshot, std::ios::binary);
        output << Json{{"schema_version", 1}, {"packages", Json::array({{
            {"root", utf8(package)}, {"manifest_sha256", hash(package / "manifest.json")}}})}}.dump();
        check(bool(output), "Snapshot write");
    }
    const auto dll = package / "org.audiograph.rnnoise.plugin.dll";
    check(GetModuleHandleW(dll.filename().c_str()) == nullptr, "RNNoise module loaded before discovery");
    NodeRegistry registry;
    register_realtime_nodes(registry);
    const auto report = Json::parse(register_plugin_nodes(registry,
        {snapshot, hash(snapshot), temp.root / "data", temp.root / "workspace"}));
    check(report["errors"].empty() && report["available"].size() == 1, "RNNoise discovery failed");
    check(GetModuleHandleW(dll.filename().c_str()) == nullptr, "Discovery loaded RNNoise DLL");
    const auto& descriptor = registry.descriptor(type);
    check(descriptor.parameters.empty() && descriptor.execution_domain == ExecutionDomain::Realtime &&
          descriptor.realtime_role == RealtimeRole::Processor && descriptor.realtime_capabilities &&
          descriptor.realtime_capabilities->format == AudioFormat{48000, 1} &&
          descriptor.realtime_capabilities->maximum_block_frames == 65536 &&
          descriptor.realtime_capabilities->offline_drivable && descriptor.realtime_capabilities->supports_variable_blocks,
          "RNNoise realtime catalog metadata mismatch");
    const auto metadata = Json::parse(node_description_json(descriptor));
    check(metadata["node"]["plugin"]["capabilities"][0]["id"] == "ag.realtime_audio/1", "RNNoise capability provenance");
    auto plan = RealtimeGraphExecutor::compile(graph(), registry);
    check(GetModuleHandleW(dll.filename().c_str()) == nullptr, "Compile loaded RNNoise DLL");
    plan.prepare({48000, 1}, 1024);
    check(GetModuleHandleW(dll.filename().c_str()) != nullptr, "Prepare did not load RNNoise DLL");
    const auto input = signal();
    const auto expected = oracle(input);
    check(std::any_of(expected.begin(), expected.end(), [](float value) { return std::abs(value) > 1e-3f; }),
          "Non-silent oracle required to reject all-zero implementations");
    compare(plan, input, expected, {1, 7, 127, 256, 479, 480, 481, 1024});
    plan.prepare({48000, 1}, 1024);
    compare(plan, input, expected, {1024, 480, 1, 479});
    // Same format/capacity prepare must recreate all DSP and partial frame history.
    plan.prepare({48000, 1}, 1024);
    std::array<float, 17> partial{};
    partial.fill(0.4f);
    check(plan.process(partial).status == RealtimeProcessStatus::Ok, "Partial host frame failed");
    plan.prepare({48000, 1}, 1024);
    compare(plan, input, expected, {480});
    plan.prepare({48000, 1}, 256);
    RealtimeBridge bridge({48000, 4096, 256, false}, &plan);
    std::array<float, 1024> captured{};
    captured.fill(1.25f);
    std::array<float, 256> rendered{};
    rendered.fill(1.0f);
    bridge.capture(captured.data(), static_cast<std::uint32_t>(captured.size()));
    bridge.render(rendered.data(), static_cast<std::uint32_t>(rendered.size()), 1);
    check(bridge.faulted() && bridge.process_fault().status == RealtimeProcessStatus::NodeFailed,
          "RNNoise finite out-of-range failure not mapped/latching as NodeFailed");
    check(std::all_of(rendered.begin(), rendered.end(), [](float sample) { return sample == 0.0f; }),
          "RNNoise failure did not mute whole render block");
    captured.fill(0.1f);
    rendered.fill(1.0f);
    bridge.capture(captured.data(), static_cast<std::uint32_t>(captured.size()));
    bridge.render(rendered.data(), static_cast<std::uint32_t>(rendered.size()), 1);
    check(bridge.faulted() && std::all_of(rendered.begin(), rendered.end(), [](float sample) { return sample == 0.0f; }),
          "RNNoise bridge fault did not remain latched");
}
}
int main(int argc, char** argv) {
    try {
        check(argc == 2, "Usage: rnnoise-realtime-host-tests PACKAGE_DIRECTORY");
        run(fs::absolute(path_from_utf8(argv[1])));
        std::cout << "RNNoise realtime host tests passed\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
