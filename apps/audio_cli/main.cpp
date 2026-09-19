#include "audioprocess/audio_buffer.h"
#include "audioprocess/bypass_node.h"
#include "audioprocess/processing_chain.h"
#include "audioprocess/wav_file.h"

#include <charconv>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <memory>
#include <stdexcept>
#include <string_view>

namespace {

// audio-cli 是 M0 阶段的底层音频通路实验：
// PCM16 WAV -> 分块 AudioBlock -> ProcessingChain -> PCM16 WAV。
// 它不使用通用 GraphExecutor，目的是单独验证块式音频处理接口。
struct Options {
    std::filesystem::path input;
    std::filesystem::path output;
    std::uint32_t block_size{256};
};

void print_usage() {
    std::cout
        << "Usage:\n"
        << "  audio-cli --input <input.wav> --output <output.wav> "
           "[--block-size <frames>]\n";
}

std::uint32_t parse_block_size(std::string_view text) {
    // from_chars 不会抛异常，适合严格检查整个字符串是否都是合法整数。
    std::uint32_t value{};
    const auto result = std::from_chars(text.data(), text.data() + text.size(), value);
    if (result.ec != std::errc{} || result.ptr != text.data() + text.size() ||
        value == 0 || value > 65'536) {
        throw std::invalid_argument("Block size must be an integer between 1 and 65536");
    }
    return value;
}

Options parse_options(int argc, char* argv[]) {
    // 原型阶段使用简单的成对命令行参数，避免引入额外参数解析依赖。
    Options options;

    for (int index = 1; index < argc; ++index) {
        const std::string_view argument{argv[index]};
        if (argument == "--help" || argument == "-h") {
            print_usage();
            std::exit(0);
        }
        if (index + 1 >= argc) {
            throw std::invalid_argument("Missing value after " + std::string(argument));
        }

        const std::string_view value{argv[++index]};
        if (argument == "--input") {
            options.input = value;
        } else if (argument == "--output") {
            options.output = value;
        } else if (argument == "--block-size") {
            options.block_size = parse_block_size(value);
        } else {
            throw std::invalid_argument("Unknown argument: " + std::string(argument));
        }
    }

    if (options.input.empty() || options.output.empty()) {
        throw std::invalid_argument("Both --input and --output are required");
    }
    return options;
}

}  // namespace

int main(int argc, char* argv[]) {
    try {
        const auto options = parse_options(argc, argv);

        // Source 和 Sink 负责 PCM16 与内部 float32 格式之间的转换。
        // AudioBuffer 只分配一次，循环中的 AudioBlock 是它的非拥有视图。
        audioprocess::WavFileSource source(options.input, options.block_size);
        audioprocess::AudioBuffer buffer(source.format(), options.block_size);
        audioprocess::WavFileSink sink(options.output, source.format(), options.block_size);

        // 当前处理链只有 BypassNode。后续实时 DSP 节点仍可复用这套块式接口。
        audioprocess::ProcessingChain chain;
        chain.add_node(std::make_unique<audioprocess::BypassNode>());
        chain.prepare(source.format(), options.block_size);

        // 同一个缓冲区被重复填充；每个有效数据块依次经过全部节点后写出。
        while (auto block = source.read(buffer)) {
            chain.process(*block);
            sink.write(*block);
        }
        // finalize() 回填 RIFF 和 data chunk 的最终长度。
        sink.finalize();

        std::cout
            << "Processed " << sink.frames_written() << " frames at "
            << source.format().sample_rate << " Hz with "
            << source.format().channel_count << " channel(s).\n";
        return 0;
    } catch (const std::exception& error) {
        // CLI 用非零退出码向脚本或调用者报告失败。
        std::cerr << "audio-cli: " << error.what() << '\n';
        print_usage();
        return 1;
    }
}
