#pragma once

#include "audioprocess/node.h"

#include <cstdint>
#include <span>

namespace audioprocess {

enum class RealtimeProcessStatus : std::uint32_t {
    Ok,
    NotPrepared,
    InvalidBlock,
    NonFiniteInput,
    NodeFailed,
    NonFiniteOutput,
};

// 实时处理器：准备阶段允许分配/抛异常；process 原地处理，保持格式和帧数。
// process 必须有界、不分配、不等待锁/网络/设备、不抛异常，不能保存借用 span。
// 输入块允许 1..prepare(maximum_frames) 帧；同一实例只由一个执行线程调用。
class IRealtimeProcessor {
public:
    virtual ~IRealtimeProcessor() = default;
    [[nodiscard]] virtual const NodeDescriptor& descriptor() const noexcept = 0;
    virtual void prepare(AudioFormat format, std::uint32_t maximum_frames) = 0;
    [[nodiscard]] virtual RealtimeProcessStatus process(std::span<float> samples) noexcept = 0;
};

} // namespace audioprocess
