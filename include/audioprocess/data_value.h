#pragma once

#include "audioprocess/audio_format.h"

#include <cstdint>
#include <filesystem>
#include <memory>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <variant>
#include <vector>

namespace audioprocess {

enum class DataType {
    Audio,
    Number,
    Text,
    FilePath,
    AudioStream,  // 流端口的连接类型；借用块通过流接口传递，不存入 DataValue。
};

[[nodiscard]] constexpr const char* data_type_name(DataType type) noexcept {
    switch (type) {
    case DataType::Audio:
        return "Audio";
    case DataType::Number:
        return "Number";
    case DataType::Text:
        return "Text";
    case DataType::FilePath:
        return "FilePath";
    case DataType::AudioStream:
        return "AudioStream";
    }
    return "Unknown";
}

struct AudioClip {
    AudioFormat format;
    std::vector<float> samples;

    [[nodiscard]] std::uint64_t frame_count() const {
        if (!format.valid() || samples.size() % format.channel_count != 0) {
            throw std::logic_error("AudioClip contains an invalid format or sample count");
        }
        return samples.size() / format.channel_count;
    }
};

// 图中的分支共享只读音频；修改音频的节点必须创建自己的输出缓冲区。
using AudioClipPtr = std::shared_ptr<const AudioClip>;
using DataValue = std::variant<AudioClipPtr, double, std::string, std::filesystem::path>;

[[nodiscard]] inline DataType data_type_of(const DataValue& value) {
    return std::visit(
        [](const auto& item) -> DataType {
            using Value = std::decay_t<decltype(item)>;
            if constexpr (std::is_same_v<Value, AudioClipPtr>) {
                return DataType::Audio;
            } else if constexpr (std::is_same_v<Value, double>) {
                return DataType::Number;
            } else if constexpr (std::is_same_v<Value, std::string>) {
                return DataType::Text;
            } else {
                return DataType::FilePath;
            }
        },
        value);
}

}  // namespace audioprocess
