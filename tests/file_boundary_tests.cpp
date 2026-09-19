#include "audioprocess/audio_buffer.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/sync_graph_executor.h"
#include "audioprocess/wav_file.h"

#include <atomic>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
using namespace audioprocess;

void require(bool condition, const std::string& message) {
    if (!condition) { throw std::runtime_error(message); }
}

// 只清理本测试原子创建的独立目录，不触碰工作区或预先存在的文件。
class TemporaryDirectory {
public:
    TemporaryDirectory() {
        const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
        for (unsigned attempt = 0; attempt < 100; ++attempt) {
            auto candidate = std::filesystem::temp_directory_path() /
                ("audioprocess_file_boundaries_" + std::to_string(stamp) + "_" + std::to_string(attempt));
            if (std::filesystem::create_directory(candidate)) { path = std::move(candidate); return; }
        }
        throw std::runtime_error("Unable to create unique test directory");
    }
    ~TemporaryDirectory() {
        std::error_code ignored;
        if (!path.empty()) { std::filesystem::remove_all(path, ignored); }
    }
    TemporaryDirectory(const TemporaryDirectory&) = delete;
    TemporaryDirectory& operator=(const TemporaryDirectory&) = delete;
    std::filesystem::path path;
};

void write_bytes(const std::filesystem::path& path, const std::string& bytes) {
    std::ofstream file(path, std::ios::binary);
    file.write(bytes.data(), static_cast<std::streamsize>(bytes.size()));
    if (!file) { throw std::runtime_error("Fixture write failed"); }
}
std::string read_bytes(const std::filesystem::path& path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) { throw std::runtime_error("Fixture read failed"); }
    return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}
void set_u16(std::string& bytes, std::size_t offset, std::uint16_t value) {
    bytes[offset] = static_cast<char>(value & 0xffU);
    bytes[offset + 1] = static_cast<char>((value >> 8U) & 0xffU);
}
void set_u32(std::string& bytes, std::size_t offset, std::uint32_t value) {
    for (unsigned i = 0; i < 4; ++i) {
        bytes[offset + i] = static_cast<char>((value >> (i * 8U)) & 0xffU);
    }
}

// 手写标准 PCM16 单声道单帧 fixture，不依赖被测试的 WavFileSink。
std::string pcm16_fixture() {
    std::string bytes(46, '\0');
    bytes.replace(0, 4, "RIFF"); set_u32(bytes, 4, 38);
    bytes.replace(8, 4, "WAVE"); bytes.replace(12, 4, "fmt ");
    set_u32(bytes, 16, 16); set_u16(bytes, 20, 1); set_u16(bytes, 22, 1);
    set_u32(bytes, 24, 48000); set_u32(bytes, 28, 96000);
    set_u16(bytes, 32, 2); set_u16(bytes, 34, 16);
    bytes.replace(36, 4, "data"); set_u32(bytes, 40, 2); set_u16(bytes, 44, 8192);
    return bytes;
}

template<class Exception, class Function>
void rejects(Function action, const std::string& message) {
    bool rejected = false;
    try { action(); }
    catch (const Exception&) { rejected = true; }
    require(rejected, message);
}

void test_pcm16_boundaries(const std::filesystem::path& directory) {
    const auto path = directory / "near_full_scale.wav";
    {
        AudioBuffer buffer({48000, 1}, 4);
        auto block = buffer.block(4);
        block.samples[0] = 0.999999F; block.samples[1] = 1.0F;
        block.samples[2] = -1.0F; block.samples[3] = 0.25F;
        WavFileSink sink(path, {48000, 1}, 4);
        sink.write(block); sink.finalize(); sink.finalize();
    }
    const auto bytes = read_bytes(path);
    require(bytes.size() == 52, "Unexpected PCM16 file size");
    require(static_cast<unsigned char>(bytes[44]) == 0xffU &&
            static_cast<unsigned char>(bytes[45]) == 0x7fU,
            "Near positive full scale wrapped to a negative value");
    WavFileSource source(path, 4);
    AudioBuffer buffer(source.format(), 4);
    const auto block = source.read(buffer);
    require(block && block->samples[0] > 0.99F && block->samples[1] > 0.99F &&
            block->samples[2] == -1.0F && block->samples[3] == 0.25F,
            "PCM16 boundary samples did not decode correctly");
}

void test_output_preservation(const std::filesystem::path& directory) {
    const auto path = directory / "keep_existing.wav";
    const std::string original = "Existing user content";
    write_bytes(path, original);
    rejects<std::runtime_error>([&] { WavFileSink sink(path, {48000, 1}, 64); },
                               "Existing output was accepted");
    rejects<std::invalid_argument>([&] { WavFileSink sink(path, {48000, 1}, 0); },
                                  "Invalid block size was accepted");
    rejects<std::invalid_argument>([&] { WavFileSink sink(path, {48000, 40000}, 64); },
                                  "Unrepresentable block alignment was accepted");
    rejects<std::invalid_argument>([&] { WavFileSink sink(path, {0xffffffffU, 2}, 64); },
                                  "Overflowing byte rate was accepted");
    require(read_bytes(path) == original, "Failed sink construction modified existing file");

    const auto fresh = directory / "must_not_be_created.wav";
    rejects<std::invalid_argument>([&] { WavFileSink sink(fresh, {0, 1}, 64); },
                                  "Invalid sample rate was accepted");
    require(!std::filesystem::exists(fresh), "Invalid format created a partial output");

    auto native = (directory / "nul_prefix.wav").native();
    native.push_back(std::filesystem::path::value_type{});
    native += std::filesystem::path{"suffix"}.native();
    const std::filesystem::path nul_path{native};
    rejects<std::invalid_argument>([&] { WavFileSink sink(nul_path, {48000, 1}, 64); },
                                  "Embedded NUL output path was accepted");
    rejects<std::invalid_argument>([&] { WavFileSource source(nul_path, 64); },
                                  "Embedded NUL input path was accepted");
    require(!std::filesystem::exists(directory / "nul_prefix.wav"), "NUL path created truncated-prefix file");
}

void test_output_clipping_count(const std::filesystem::path& directory) {
    const auto path = directory / "clipped_stereo.wav";
    auto mutable_clip = std::make_shared<AudioClip>();
    mutable_clip->format = {48000, 2};
    mutable_clip->samples.resize(2050, 0.0F); // 跨过 1024 帧写入块边界，保留尾块。
    mutable_clip->samples[0] = 1.5F;
    mutable_clip->samples[1] = -1.5F;
    mutable_clip->samples[2] = 0.999999F;
    mutable_clip->samples[3] = 32767.0F / 32768.0F; // 正边界不计削波。
    mutable_clip->samples[4] = -1.0F;              // 负边界不计削波。
    mutable_clip->samples[5] = 0.25F;
    mutable_clip->samples[6] = 1.0F;
    mutable_clip->samples[7] = -1.000001F;
    mutable_clip->samples[2048] = 2.0F;
    mutable_clip->samples[2049] = -2.0F;
    AudioClipPtr clip = std::move(mutable_clip);

    const auto registry = create_prototype_node_registry();
    auto output = registry.create("wav_output", {{"path", path}});
    ExecutionContext context;
    const auto result = output->execute({{"audio", clip}}, context);
    require(std::get<double>(result.at("clipped_samples")) == 7.0,
            "Clipping count must include each out-of-range sample, including the final block");
    auto meter = registry.create("peak_meter", {});
    const auto measured = meter->execute({{"audio", clip}}, context);
    require(std::get<double>(measured.at("peak")) == 2.0,
            "Output encoding modified shared audio or pre-encoding peak");

    WavFileSource source(path, 1024);
    AudioBuffer buffer(source.format(), 1024);
    require(source.total_frames() == 1025 && source.format().channel_count == 2,
            "Clipping changed the stereo file length or format");
    const auto first = source.read(buffer);
    const auto positive_limit = 32767.0F / 32768.0F;
    require(first && first->samples[0] == positive_limit && first->samples[1] == -1.0F &&
            first->samples[2] == positive_limit && first->samples[3] == positive_limit &&
            first->samples[4] == -1.0F && first->samples[5] == 0.25F &&
            first->samples[6] == positive_limit && first->samples[7] == -1.0F,
            "PCM16 output did not saturate boundary and out-of-range samples correctly");
    const auto last = source.read(buffer);
    require(last && last->frame_count == 1 && last->samples[0] == positive_limit &&
            last->samples[1] == -1.0F, "Final stereo frame did not saturate correctly");
}

void test_riff_validation(const std::filesystem::path& directory) {
    auto bytes = pcm16_fixture();
    const auto valid = directory / "fixture.wav";
    write_bytes(valid, bytes);
    { WavFileSource source(valid, 64); require(source.total_frames() == 1, "Valid byte fixture failed"); }
    auto bad = [&](const std::string& name, std::string fixture) {
        const auto path = directory / name;
        write_bytes(path, fixture);
        rejects<std::runtime_error>([&] { WavFileSource source(path, 64); },
                                    "Invalid RIFF accepted: " + name);
    };
    auto changed = bytes; set_u32(changed, 4, 100); bad("riff_past_eof.wav", changed);
    changed = bytes; set_u32(changed, 4, 37); bad("chunk_past_riff.wav", changed);
    changed = bytes; set_u32(changed, 40, 100); bad("data_past_riff.wav", changed);
    changed = bytes; set_u32(changed, 40, 1); bad("partial_frame.wav", changed);
    changed = bytes; set_u32(changed, 28, 123); bad("wrong_byte_rate.wav", changed);
    changed = bytes; set_u16(changed, 32, 1); bad("wrong_block_alignment.wav", changed);
    changed = bytes; set_u16(changed, 22, 32768); set_u16(changed, 32, 0);
    bad("overflow_block_alignment.wav", changed);
    changed = bytes; changed.pop_back(); bad("truncated_data.wav", changed);
}

void test_target_preflight(const std::filesystem::path& directory) {
    const auto preflight_rejects = [](const GraphDefinition& graph, const std::string& node,
                                     const std::string& code) {
        bool rejected = false;
        try { validate_prototype_file_targets(graph); }
        catch (const ExecutionError& error) {
            rejected = error.code == code && error.node_id == node && error.parameter_id == "path";
        }
        require(rejected, "Preflight failed to provide structured path error: " + code);
    };
    const auto input = directory / "source.wav";
    write_bytes(input, pcm16_fixture());
    preflight_rejects(create_prototype_graph(input, input, 0.0), "output", "output_exists");
    GraphDefinition graph{{
        {"a", "text_output", {{"path", directory / "new.txt"}}},
        {"b", "text_output", {{"path", directory / "." / "new.txt"}}}}, {}};
    preflight_rejects(graph, "b", "file_target_conflict");
#ifdef _WIN32
    graph.nodes[1].parameters["path"] = directory / "NEW.TXT";
    preflight_rejects(graph, "b", "file_target_conflict");
#endif
    graph.nodes.resize(1);
    graph.nodes[0].parameters["path"] = directory / "missing" / "out.txt";
    preflight_rejects(graph, "a", "output_directory_missing");
    auto native = (directory / "nul_target").native();
    native.push_back(std::filesystem::path::value_type{});
    graph.nodes[0].parameters["path"] = std::filesystem::path{native};
    preflight_rejects(graph, "a", "invalid_path");
    require(!std::filesystem::exists(directory / "new.txt"), "Preflight created an output file");
}

void test_text_io_and_cancel(const std::filesystem::path& directory) {
    const auto path = directory / std::filesystem::path{u8"文本输出.txt"};
    const auto utf8 = u8"语音处理\nText → TTS\n";
    const std::string text(reinterpret_cast<const char*>(utf8));
    auto registry = create_prototype_node_registry();
    GraphDefinition graph{{
        {"text", "text_input", {{"text", text}}},
        {"file", "text_output", {{"path", path}}}},
        {{"text", "text", "file", "text"}}};
    auto executor = SyncGraphExecutor::compile(graph, registry);
    const auto result = executor.execute();
    require(std::get<std::filesystem::path>(result.value("file", "path")) == path,
            "Text output did not return its path");
    require(read_bytes(path) == text, "UTF-8 text bytes or line endings changed");
    bool existing_rejected = false;
    try { (void)executor.execute(); }
    catch (const ExecutionError& error) { existing_rejected = error.code == "node_execution_failed"; }
    require(existing_rejected && read_bytes(path) == text, "Text output overwrote existing file");

    const auto cancelled_path = directory / "cancelled.txt";
    auto output = registry.create("text_output", {{"path", cancelled_path}});
    std::atomic_bool cancelled{true};
    ExecutionContext context{&cancelled};
    bool cancellation_reported = false;
    try { (void)output->execute({{"text", text}}, context); }
    catch (const ExecutionError& error) { cancellation_reported = error.code == "cancelled"; }
    require(cancellation_reported && !std::filesystem::exists(cancelled_path),
            "Pre-cancelled text output created a file");
}
} // namespace

int main() {
    try {
        TemporaryDirectory temporary;
        test_pcm16_boundaries(temporary.path);
        test_output_preservation(temporary.path);
        test_output_clipping_count(temporary.path);
        test_riff_validation(temporary.path);
        test_target_preflight(temporary.path);
        test_text_io_and_cancel(temporary.path);
        std::cout << "All file boundary tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "File boundary test failure: " << error.what() << '\n';
        return 1;
    }
}
