#include "audioprocess/experimental/execution_contracts.h"

#include <algorithm>
#include <chrono>
#include <iostream>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>

namespace {
using namespace audioprocess;
using namespace audioprocess::experimental;

void require(bool condition, const std::string& message) {
    if (!condition) {
        throw std::runtime_error(message);
    }
}

// 由测试控制完成时机，不依赖 sleep 或机器速度，也不在实时线程上等待。
class DelayedEcho final : public IAsyncNode {
public:
    DelayedEcho(std::shared_future<void> release, bool cooperative)
        : release_(std::move(release)), cooperative_(cooperative) {}

    std::future<OutputValues> submit(
        InputValues inputs,
        std::shared_ptr<const AsyncContext> context) override {
        return std::async(std::launch::async,
            [inputs = std::move(inputs), context = std::move(context),
             release = release_, cooperative = cooperative_]() mutable {
                release.wait();
                if (cooperative && context->stop.stop_requested()) {
                    throw std::runtime_error("cancelled");
                }
                return OutputValues{{"audio", std::move(inputs.at("audio"))}};
            });
    }

private:
    std::shared_future<void> release_;
    bool cooperative_;
};

// 仅验证一种候选调度策略，不代表正式 Executor 已支持异步任务。
// 取消/替换任务后，旧任务即使成功完成也不再发布。
class CompletionGate {
public:
    void start(std::uint64_t id) { active_id_ = id; result_.clear(); }
    void cancel() { active_id_.reset(); result_.clear(); }
    bool deliver(std::uint64_t id, OutputValues output) {
        if (!active_id_ || *active_id_ != id) {
            return false;
        }
        result_ = std::move(output);
        return true;
    }
    const OutputValues& result() const { return result_; }

private:
    std::optional<std::uint64_t> active_id_;
    OutputValues result_;
};

AudioClipPtr clip(float sample) {
    auto value = std::make_shared<AudioClip>();
    value->format = {48'000, 1};
    value->samples = {sample};
    return value;
}

void test_async_owns_inputs_and_context() {
    std::promise<void> release;
    DelayedEcho node(release.get_future().share(), false);
    std::stop_source cancellation;
    auto context = std::make_shared<AsyncContext>(cancellation.get_token());
    std::weak_ptr<const AsyncContext> context_observer = context;
    auto audio = clip(0.25F);
    std::weak_ptr<const AudioClip> audio_observer = audio;
    InputValues input{{"audio", audio}};

    auto future = node.submit(std::move(input), context);
    audio.reset();
    context.reset();
    const bool input_alive = !audio_observer.expired();
    const bool context_alive = !context_observer.expired();
    const bool pending = future.wait_for(std::chrono::milliseconds(0)) ==
        std::future_status::timeout;
    // 先打开 gate，再断言：即使断言失败也不会卡在 async future 析构上。
    release.set_value();
    auto output = future.get();
    require(input_alive && context_alive, "Async task lost owned input/context");
    require(pending, "submit did not return before completion");
    require(std::get<AudioClipPtr>(output.at("audio"))->samples[0] == 0.25F,
        "Async data changed after caller released its references");
    output.clear();
    require(audio_observer.expired() && context_observer.expired(),
        "Finished async operation retained task resources");
}

void test_cooperative_cancellation_and_late_result_isolation() {
    std::promise<void> release;
    DelayedEcho node(release.get_future().share(), true);
    std::stop_source cancellation;
    auto context = std::make_shared<AsyncContext>(cancellation.get_token());
    auto future = node.submit({{"audio", clip(0.5F)}}, context);
    cancellation.request_stop();
    release.set_value();
    bool cancelled = false;
    try {
        static_cast<void>(future.get());
    } catch (const std::runtime_error& error) {
        cancelled = std::string(error.what()) == "cancelled";
    }
    require(cancelled, "Async task ignored cooperative cancellation");

    // 模拟不能即时取消的 HTTP/GPU 请求：即使返回正常结果也不能污染新任务。
    std::promise<void> late_release;
    DelayedEcho uninterruptible(late_release.get_future().share(), false);
    auto late = uninterruptible.submit({{"audio", clip(0.75F)}}, context);
    CompletionGate gate;
    gate.start(1);
    const bool first_delivered = gate.deliver(1, {{"text", std::string("old task")}});
    gate.cancel();
    const bool cleared_on_cancel = gate.result().empty();
    gate.start(2);
    const bool second_delivered = gate.deliver(2, {{"text", std::string("new task")}});
    late_release.set_value();
    require(first_delivered && second_delivered, "Active task rejected");
    require(cleared_on_cancel, "Cancelled task exposed its old result");
    require(!gate.deliver(1, late.get()), "Late result triggered a replaced task");
    require(std::get<std::string>(gate.result().at("text")) == "new task",
        "Late result overwrote the active task");
    gate.start(3);
    require(gate.result().empty(), "New task inherited a prior task's published result");
}

// 4帧聚合模拟需要跨块上下文的算法。调用者可用任意块大小喂入同一文件。
class FourFrameBuffer final : public IStreamingAudioNode {
public:
    void prepare(AudioFormat format) override {
        if (!format.valid()) {
            throw std::invalid_argument("invalid audio format");
        }
        format_ = format;
        prepared_ = true;
        reset();
    }

    std::vector<AudioClipPtr> push(std::span<const float> samples) override {
        if (!prepared_ || ended_ || samples.size() % format_.channel_count != 0) {
            throw std::logic_error("invalid stream input or lifecycle");
        }
        pending_.insert(pending_.end(), samples.begin(), samples.end());
        std::vector<AudioClipPtr> result;
        const auto count = static_cast<std::size_t>(4) * format_.channel_count;
        while (pending_.size() >= count) {
            result.push_back(emit(count));
        }
        return result;
    }

    std::vector<AudioClipPtr> finish() override {
        if (!prepared_ || ended_) {
            throw std::logic_error("stream already ended or not prepared");
        }
        ended_ = true;
        if (pending_.empty()) {
            return {};
        }
        return {emit(pending_.size())};
    }

    void reset() noexcept override {
        pending_.clear();
        ended_ = false;
    }

private:
    AudioClipPtr emit(std::size_t count) {
        auto output = std::make_shared<AudioClip>();
        output->format = format_;
        const auto end = pending_.begin() + static_cast<std::ptrdiff_t>(count);
        output->samples.assign(pending_.begin(), end);
        pending_.erase(pending_.begin(), end);
        return output;
    }

    AudioFormat format_{};
    bool prepared_{};
    bool ended_{};
    std::vector<float> pending_;
};

std::vector<float> run_in_chunks(std::span<const float> input, std::size_t size) {
    FourFrameBuffer node;
    node.prepare({48'000, 1});
    std::vector<float> output;
    auto collect = [&](const std::vector<AudioClipPtr>& chunks) {
        for (const auto& chunk : chunks) {
            output.insert(output.end(), chunk->samples.begin(), chunk->samples.end());
        }
    };
    for (std::size_t offset = 0; offset < input.size(); offset += size) {
        collect(node.push(input.subspan(offset, std::min(size, input.size() - offset))));
    }
    collect(node.finish());
    return output;
}

void test_stream_drain_and_reset() {
    const std::vector<float> input{0, 1, 2, 3, 4, 5, 6};
    require(run_in_chunks(input, 1) == input && run_in_chunks(input, 5) == input,
        "File chunk size changed stream output or dropped final frames");

    FourFrameBuffer node;
    node.prepare({48'000, 1});
    std::vector<float> borrowed{0.25F, 0.5F};
    require(node.push(borrowed).empty(), "Partial batch should be buffered");
    borrowed.assign(2, -1.0F);
    const auto tail = node.finish();
    require(tail.size() == 1 && tail[0]->samples == std::vector<float>{0.25F, 0.5F},
        "Stream retained borrowed memory instead of copying pending samples");
    bool rejected = false;
    try {
        static_cast<void>(node.push(borrowed));
    } catch (const std::logic_error&) {
        rejected = true;
    }
    require(rejected, "Stream accepted input after EOS without reset");
    node.reset();
    static_cast<void>(node.push(borrowed));
    node.reset(); // unfinished task discarded before a new file
    require(node.finish().empty(), "New stream inherited previous task samples");
}

}  // namespace

int main() {
    try {
        // 同步流式、异步整段均成立；实时期限不能从前两个维度推导。
        constexpr ExecutionTraits realtime_dsp{
            InputGranularity::Stream, InvocationStyle::Synchronous, DeadlinePolicy::Realtime};
        constexpr ExecutionTraits offline_asr{
            InputGranularity::WholeValue, InvocationStyle::Asynchronous, DeadlinePolicy::None};
        static_assert(realtime_dsp.invocation_style != offline_asr.invocation_style);
        test_async_owns_inputs_and_context();
        test_cooperative_cancellation_and_late_result_isolation();
        test_stream_drain_and_reset();
        std::cout << "Experimental async/stream contract tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Contract test failure: " << error.what() << '\n';
        return 1;
    }
}
