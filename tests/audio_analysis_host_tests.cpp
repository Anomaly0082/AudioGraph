#include "audioprocess/plugin_host.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/sync_graph_executor.h"
#include "audioprocess/wav_file.h"
#include <nlohmann/json.hpp>
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <bcrypt.h>
#include <array>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <vector>

namespace {
using namespace audioprocess;
using Json = nlohmann::json;
namespace fs = std::filesystem;
constexpr char type[] = "org.audiograph.analysis.audio_metrics_v1";
constexpr char dll_name[] = "org.audiograph.analysis.plugin.dll";
void check(bool value, const char* message) { if (!value) throw std::runtime_error(message); }
void require_close(double actual, double expected, const char* message) {
    check(std::isfinite(actual) && std::abs(actual - expected) < 1e-9, message);
}
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
    check(bool(input), "Hash input missing");
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
    fs::path root = fs::temp_directory_path() / ("analysis-host-" + std::to_string(GetCurrentProcessId()) + "-" +
        std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
    Temp() { fs::create_directories(root / "workspace"); fs::create_directories(root / "data"); }
    ~Temp() { std::error_code error; fs::remove_all(root, error); }
};
void snapshot(const fs::path& destination, const fs::path& package, const std::string& manifest_hash) {
    std::ofstream output(destination, std::ios::binary);
    output << Json{{"schema_version", 1}, {"packages", Json::array({{
        {"root", utf8(package)}, {"manifest_sha256", manifest_hash}}})}}.dump();
    check(bool(output), "Snapshot write");
}
GraphDefinition graph(const fs::path& input, const fs::path& output) {
    GraphDefinition definition{
        {{"input", "wav_input", {{"path", input}}},
         {"gain", "gain", {{"gain_db", 12.0}}},
         {"analysis", type, {}},
         {"output", "wav_output", {{"path", output}}}},
        {{"input", "audio", "gain", "audio"},
         {"gain", "audio", "analysis", "audio"},
         {"gain", "audio", "output", "audio"}},
        {{"rms", "analysis", "rms"}, {"rms_dbfs", "analysis", "rms_dbfs"},
         {"clipping_ratio", "analysis", "clipping_ratio"}, {"silence_ratio", "analysis", "silence_ratio"},
         {"output_file", "output", "path"}, {"clipped_samples", "output", "clipped_samples"}}};
    return definition;
}
void run(const fs::path& package) {
    Temp temp;
    const auto dll = package / dll_name;
    check(GetModuleHandleW(dll.filename().c_str()) == nullptr, "Analysis DLL loaded before discovery");
    const auto missing_snapshot = temp.root / "missing.json";
    snapshot(missing_snapshot, temp.root / "missing-package", std::string(64, '0'));
    auto missing_registry = create_prototype_node_registry();
    const auto missing_report = Json::parse(register_plugin_nodes(missing_registry,
        {missing_snapshot, hash(missing_snapshot), temp.root / "data", temp.root / "workspace"}));
    check(missing_report["available"].empty() && !missing_report["errors"].empty(), "Missing package silently accepted");
    const auto registry_snapshot = temp.root / "snapshot.json";
    snapshot(registry_snapshot, package, hash(package / "manifest.json"));
    auto registry = create_prototype_node_registry();
    const auto report = Json::parse(register_plugin_nodes(registry,
        {registry_snapshot, hash(registry_snapshot), temp.root / "data", temp.root / "workspace"}));
    check(report["errors"].empty() && report["available"].size() == 1, "Analysis static discovery failed");
    check(GetModuleHandleW(dll.filename().c_str()) == nullptr, "Static discovery loaded Analysis DLL");
    const auto& descriptor = registry.descriptor(type);
    check(descriptor.execution_domain == ExecutionDomain::Synchronous && descriptor.inputs.size() == 1 &&
        descriptor.inputs[0].type == DataType::Audio && descriptor.outputs.size() == 4 && descriptor.plugin &&
        descriptor.plugin->plugin_id == "org.audiograph.analysis" && descriptor.plugin->capability_id == "ag.whole_sync/1",
        "Analysis catalog contract mismatch");
    for (const auto& port : descriptor.outputs) check(port.type == DataType::Number, "Analysis output must be Number");
    const auto defaults = registry.normalize_parameters(type, {});
    require_close(std::get<double>(defaults.at("silence_threshold_db")), -60.0, "Analysis threshold default");

    std::vector<float> samples(3000, 0.0f);
    for (std::size_t i = 960; i < 1920; ++i) samples[i] = i % 2 == 0 ? 0.5f : -0.5f;
    for (std::size_t i = 2880; i < samples.size(); ++i) samples[i] = i % 2 == 0 ? 0.25f : -0.25f;
    const auto input = temp.root / "workspace/input.wav";
    const auto output = temp.root / "workspace/output.wav";
    { WavFileSink sink(input, {48000, 1}, 3000); sink.write(samples, 3000); sink.finalize(); }
    const auto input_hash = hash(input);
    auto definition = graph(input, output);
    auto invalid = definition;
    invalid.nodes[2].parameters = {{"silence_threshold_db", 1.0}};
    bool rejected = false;
    try { (void)SyncGraphExecutor::compile(invalid, registry); } catch (const std::exception&) { rejected = true; }
    check(rejected, "Invalid analysis threshold accepted by graph compile");
    auto executor = SyncGraphExecutor::compile(definition, registry);
    check(GetModuleHandleW(dll.filename().c_str()) == nullptr, "Graph compile loaded Analysis DLL");
    const auto result = executor.execute();
    check(GetModuleHandleW(dll.filename().c_str()) != nullptr, "Graph execution did not load Analysis DLL");
    check(fs::exists(output), "Analysis graph did not execute WAV output branch");
    const auto source = std::get<AudioClipPtr>(result.value("input", "audio"));
    const auto gained = std::get<AudioClipPtr>(result.value("gain", "audio"));
    check(source->format == AudioFormat{48000, 1} && source->samples == samples && hash(input) == input_hash,
        "Analysis graph changed source audio");
    const float multiplier = static_cast<float>(std::pow(10.0, 12.0 / 20.0));
    double squares = 0;
    std::size_t clipped = 0;
    for (std::size_t i = 0; i < samples.size(); ++i) {
        check(gained->samples[i] == samples[i] * multiplier, "Analysis changed shared gain branch audio");
        const double value = gained->samples[i];
        squares += value * value;
        if (value < -1.0 || value > 32767.0 / 32768.0) ++clipped;
    }
    const double rms = std::sqrt(squares / samples.size());
    require_close(std::get<double>(result.value("analysis", "rms")), rms, "RMS missed weighted 120-frame tail");
    require_close(std::get<double>(result.value("analysis", "rms_dbfs")), 20.0 * std::log10(rms), "RMS dBFS mismatch");
    require_close(std::get<double>(result.value("analysis", "silence_ratio")), 1920.0 / 3000.0, "Silence ratio denominator mismatch");
    require_close(std::get<double>(result.value("analysis", "clipping_ratio")), static_cast<double>(clipped) / 3000.0,
        "Clipping ratio mismatch");
    require_close(std::get<double>(result.value("output", "clipped_samples")), static_cast<double>(clipped),
        "Analysis clipping ratio disagrees with WAV output clipped sample count");
    check(clipped == 960 && std::get<fs::path>(result.value("output", "path")) == output, "Expected clipped branch output");
    WavFileSource written(output, 1024);
    check(written.format() == AudioFormat{48000, 1} && written.total_frames() == 3000, "Output WAV dropped tail frames");
}
}
int main(int argc, char** argv) {
    try {
        check(argc == 2, "Usage: audio-analysis-host-tests PACKAGE_DIRECTORY");
        run(fs::absolute(path_from_utf8(argv[1])));
        std::cout << "Audio analysis host graph tests passed\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
