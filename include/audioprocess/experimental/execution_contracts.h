#pragma once

#include "audioprocess/node.h"

#include <future>
#include <span>
#include <stop_token>

// 这些接口用于验证 F12/F15/F16，未接入正式 Graph/Registry，也未实现实时运行时。
// 保持独立，避免把调用方式、输入粒度和实时期限混为一个互斥枚举。
namespace audioprocess::experimental {

enum class InputGranularity { WholeValue, Stream };
enum class InvocationStyle { Synchronous, Asynchronous };
enum class DeadlinePolicy { None, Realtime };

struct ExecutionTraits {
    InputGranularity input_granularity{InputGranularity::WholeValue};
    InvocationStyle invocation_style{InvocationStyle::Synchronous};
    DeadlinePolicy deadline_policy{DeadlinePolicy::None};
};

struct AsyncContext {
    // token 共享取消状态的生命周期，不借用调用方栈上的 atomic_bool。
    // 取消是协作请求；外部服务可能仍会返回，执行器必须屏蔽迟到结果。
    std::stop_token stop;
};

class IAsyncNode {
public:
    virtual ~IAsyncNode() = default;

    // 输入容器按值转移；AudioClipPtr 持有只读音频，不能保存借用 AudioBlock。
    // 实现必须尽快返回。等待/销毁 future 可能阻塞，只允许在非实时管理线程做。
    // 本实验接口不承诺线程池、排队容量、进度或可强制取消远端操作。
    [[nodiscard]] virtual std::future<OutputValues> submit(
        InputValues inputs,
        std::shared_ptr<const AsyncContext> context) = 0;
};

class IStreamingAudioNode {
public:
    virtual ~IStreamingAudioNode() = default;

    virtual void prepare(AudioFormat format) = 0;

    // 交错 float32 PCM；samples 只在本次调用内有效。需保留的样本必须复制。
    // 允许一次输入产生零到多个块，输出拥有内存。此版本允许分配，不是实时接口。
    [[nodiscard]] virtual std::vector<AudioClipPtr> push(
        std::span<const float> samples) = 0;

    // 通知输入结束并排空尾部；finish 后不能继续 push，直到 reset/prepare。
    [[nodiscard]] virtual std::vector<AudioClipPtr> finish() = 0;

    // 丢弃内部历史/尾部并开启同格式的新会话，不隐式输出旧任务内容。
    virtual void reset() noexcept = 0;
};

}  // namespace audioprocess::experimental
