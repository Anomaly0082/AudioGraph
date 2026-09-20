// 仅为 CLI 集成测试生成/核对音频，无需依赖 Python 或系统编码器。
#include "audioprocess/wav_file.h"
#include <cmath>
#include <filesystem>
#include <iostream>
#include <string>
#include <vector>

namespace {
// 两声道均随帧变化，避免等长重复/错序被恒定信号掩盖；数值可被PCM16精确表示。
float sample_at(std::uint32_t frame, unsigned channel) {
    const auto value = static_cast<int>((frame * (channel == 0 ? 17U : 31U)) % 2048U) - 1024;
    return static_cast<float>(value) / 8192.0F;
}
int run(const std::vector<std::filesystem::path>& args) {
    try {
        if (args.size() != 3) return 2;
        const auto mode = args[1].string();
        const auto& file = args[2];
        if (mode == "generate") {
            audioprocess::AudioBuffer buffer({48000, 2}, 1041);
            auto block = buffer.block(1041);
            for (std::size_t i = 0; i < block.samples.size(); i += 2) {
                block.samples[i] = sample_at(static_cast<std::uint32_t>(i / 2), 0);
                block.samples[i + 1] = sample_at(static_cast<std::uint32_t>(i / 2), 1);
            }
            audioprocess::WavFileSink sink(file, {48000, 2}, 1041);
            sink.write(block);
            sink.finalize();
        } else if (mode == "check-half") {
            audioprocess::WavFileSource source(file, 37);
            if (source.total_frames() != 1041 || source.format() != audioprocess::AudioFormat{48000, 2}) return 1;
            audioprocess::AudioBuffer buffer(source.format(), 37);
            while (auto block = source.read(buffer)) {
                for (std::size_t i = 0; i < block->samples.size(); i += 2) {
                    const auto frame = static_cast<std::uint32_t>(block->frame_position + i / 2);
                    if (std::abs(block->samples[i] - sample_at(frame, 0) * 0.5F) > 1e-5F ||
                        std::abs(block->samples[i + 1] - sample_at(frame, 1) * 0.5F) > 1e-5F) return 1;
                }
            }
        } else return 2;
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
}
#ifdef _WIN32
int wmain(int argc, wchar_t* argv[]) { return run({argv, argv + argc}); }
#else
int main(int argc, char* argv[]) { return run({argv, argv + argc}); }
#endif
