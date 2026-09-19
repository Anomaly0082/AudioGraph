#include "audioprocess/audio_buffer.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/sync_graph_executor.h"
#include "audioprocess/wav_file.h"

#include <chrono>
#include <cmath>
#include <filesystem>
#include <iostream>
#include <stdexcept>
#include <string>

namespace {

void require(bool condition, const std::string& message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

std::filesystem::path unique_path(std::string_view suffix) {
    const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
    return std::filesystem::temp_directory_path() /
        ("audioprocess_graph_" + std::to_string(stamp) + std::string(suffix));
}

void create_constant_wav(const std::filesystem::path& path) {
    constexpr audioprocess::AudioFormat format{48'000, 1};
    constexpr std::uint32_t frames = 1024;
    audioprocess::AudioBuffer buffer(format, frames);
    auto block = buffer.block(frames);
    std::fill(block.samples.begin(), block.samples.end(), 0.25F);
    audioprocess::WavFileSink sink(path, format, frames);
    sink.write(block);
    sink.finalize();
}

void test_prototype_graph() {
    const auto input = unique_path("_input.wav");
    const auto output = unique_path("_output.wav");
    create_constant_wav(input);

    const auto registry = audioprocess::create_prototype_node_registry();
    const auto graph = audioprocess::create_prototype_graph(input, output, 6.020599913);
    auto executor = audioprocess::SyncGraphExecutor::compile(graph, registry);
    const auto result = executor.execute();
    const auto peak = std::get<double>(result.value("peak", "peak"));

    require(peak > 0.49 && peak < 0.51, "Gain or peak meter produced an unexpected result");
    require(std::filesystem::exists(output), "WAV output node did not create a file");

    {
        audioprocess::WavFileSource output_source(output, 256);
        require(output_source.total_frames() == 1024, "WAV output node changed frame count");
    }

    std::filesystem::remove(input);
    std::filesystem::remove(output);
}

void test_type_mismatch_is_rejected() {
    const auto registry = audioprocess::create_prototype_node_registry();
    audioprocess::GraphDefinition graph{
        {
            {"input", "wav_input", {{"path", std::filesystem::path{"input.wav"}}}},
            {"peak", "peak_meter", {}},
            {"gain", "gain", {{"gain_db", 0.0}}},
        },
        {
            {"input", "audio", "peak", "audio"},
            {"peak", "peak", "gain", "audio"},
        },
    };

    bool rejected = false;
    try {
        [[maybe_unused]] auto executor =
            audioprocess::SyncGraphExecutor::compile(graph, registry);
    } catch (const std::invalid_argument&) {
        rejected = true;
    }
    require(rejected, "Executor accepted a Number-to-Audio connection");
}

}  // namespace

int main() {
    try {
        test_prototype_graph();
        test_type_mismatch_is_rejected();
        std::cout << "All graph executor tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Test failure: " << error.what() << '\n';
        return 1;
    }
}
