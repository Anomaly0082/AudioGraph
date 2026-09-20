#pragma once

#include "audioprocess/node.h"

#include <cstdint>
#include <functional>
#include <optional>
#include <span>

namespace audioprocess {

// 只读借用的交错 PCM。格式由 open/prepare 固定，position 是该输出流的帧位置。
struct AudioStreamBlock {
    std::span<const float> samples;
    std::uint32_t frame_count{};
    std::uint16_t channel_count{};
    std::uint64_t frame_position{};
};

// 必须在 push/finish 内同步调用，不能保存回调，也不能在后台线程调用。
// 回调返回前会消费完当前输出块，因此可复用工作缓冲而不复制整段音频。
using StreamEmit = std::function<void(const AudioStreamBlock&)>;

class IStreamNode {
public:
    virtual ~IStreamNode() = default;
    [[nodiscard]] virtual const NodeDescriptor& descriptor() const noexcept = 0;
};

class IAudioStreamSource : public IStreamNode {
public:
    [[nodiscard]] virtual AudioFormat open(std::uint32_t maximum_frames, ExecutionContext& context) = 0;
    // 返回块有效到下一次 read 或实例销毁；nullopt 表示永久 EOS，不用空块表示。
    [[nodiscard]] virtual std::optional<AudioStreamBlock> read(ExecutionContext& context) = 0;
};

class IAudioStreamProcessor : public IStreamNode {
public:
    // 返回固定输出格式，允许重采样等节点改变格式；输出仍不得超过 maximum_frames。
    [[nodiscard]] virtual AudioFormat prepare(
        AudioFormat input_format, std::uint32_t maximum_frames, ExecutionContext& context) = 0;
    // 输入和 emit 仅在本次调用内有效。需要跨块历史时节点须自行复制到有界状态中。
    virtual void push(const AudioStreamBlock& input, const StreamEmit& emit, ExecutionContext& context) = 0;
    // 输入结束后调用一次，排空残留；完成后不能再 push。
    virtual void finish(const StreamEmit& emit, ExecutionContext& context) = 0;
};

class IAudioStreamSink : public IStreamNode {
public:
    virtual void prepare(AudioFormat input_format, std::uint32_t maximum_frames, ExecutionContext& context) = 0;
    virtual void push(const AudioStreamBlock& input, ExecutionContext& context) = 0;
    // 正常 EOS 才发布最终摘要。失败/取消由析构释放资源，不执行成功结束逻辑。
    [[nodiscard]] virtual OutputValues finish(ExecutionContext& context) = 0;
};

}  // namespace audioprocess
