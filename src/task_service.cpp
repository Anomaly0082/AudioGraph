#include "audioprocess/task_service.h"
#include "audioprocess/execution_error.h"

#include <condition_variable>
#include <limits>
#include <mutex>
#include <thread>
#include <unordered_map>
#include <utility>

namespace audioprocess {
namespace {

constexpr std::size_t kMaximumRetainedTasks = 16;
constexpr std::size_t kMaximumResultBytes = 4 * 1024 * 1024;

bool terminal(TaskState state) noexcept {
    return state == TaskState::Succeeded || state == TaskState::Failed || state == TaskState::Cancelled;
}

TaskError error_from(const ExecutionError& error) {
    return {error.code, error.what(), error.node_id, error.port_id, error.parameter_id, error.field_path};
}

TaskOutcome cancelled_outcome() {
    return {{}, TaskError{"cancelled", "Task cancelled", {}, {}, {}, {}}};
}

void validate_request(const TaskRequest& request) {
    if (request.mode != ExecutionDomain::Synchronous && request.mode != ExecutionDomain::Streaming &&
        request.mode != ExecutionDomain::Realtime) {
        throw ExecutionError("invalid_task_request", "Unsupported task execution mode", {}, {}, {}, "/mode");
    }
    if (request.block_frames == 0 || request.block_frames > 65536) {
        throw ExecutionError("invalid_task_request", "Task block frames must be between 1 and 65536", {}, {}, {}, "/block_frames");
    }
    if (request.duration_seconds == 0 || request.duration_seconds > 3600) {
        throw ExecutionError("invalid_task_request", "Task duration must be between 1 and 3600 seconds", {}, {}, {}, "/duration_seconds");
    }
}

} // namespace

const char* task_state_name(TaskState state) noexcept {
    switch (state) {
    case TaskState::Queued: return "queued";
    case TaskState::Running: return "running";
    case TaskState::Cancelling: return "cancelling";
    case TaskState::Succeeded: return "succeeded";
    case TaskState::Failed: return "failed";
    case TaskState::Cancelled: return "cancelled";
    }
    return "unknown";
}

class TaskService::Impl {
public:
    explicit Impl(Runner runner) : runner_(std::move(runner)) {
        if (!runner_) throw ExecutionError("invalid_runner", "Task service requires a runner");
        worker_ = std::thread([this] { work(); });
    }

    ~Impl() {
        {
            std::lock_guard lock(mutex_);
            stopping_ = true;
            if (active_) {
                active_->cancel_requested.store(true, std::memory_order_release);
                active_->state = TaskState::Cancelling;
            }
        }
        available_.notify_one();
        if (worker_.joinable()) worker_.join();
    }

    std::string submit(TaskRequest request) {
        validate_request(request);
        // 请求可能含较大的 Graph；构造/移动放在状态锁之外，避免阻塞查询。
        auto task = std::make_shared<Record>(std::move(request));
        std::string id;
        {
            std::lock_guard lock(mutex_);
            if (stopping_) throw ExecutionError("task_service_stopping", "Task service is stopping");
            if (active_) throw ExecutionError("task_busy", "One task is already active; no additional task is queued");
            if (records_.size() >= kMaximumRetainedTasks) {
                throw ExecutionError("task_capacity", "Release a terminal task before retaining more than 16 tasks");
            }
            if (next_id_ == std::numeric_limits<std::uint64_t>::max()) {
                throw ExecutionError("task_id_exhausted", "Task IDs are exhausted for this session");
            }
            id = "task-" + std::to_string(next_id_++);
            task->id = id;
            records_.emplace(id, task);
            active_ = task;
            pending_ = task;
        }
        available_.notify_one();
        return id;
    }

    TaskSnapshot status(const std::string& id) const {
        std::shared_ptr<Record> task;
        TaskState state;
        {
            std::lock_guard lock(mutex_);
            task = find(id);
            state = task->state;
        }
        return snapshot(task, state);
    }

    TaskSnapshot cancel(const std::string& id) {
        std::shared_ptr<Record> task;
        TaskState state;
        {
            std::lock_guard lock(mutex_);
            task = find(id);
            if (!terminal(task->state)) {
                // 取消和完成发布持有同一把锁：取消先到，即使 Runner 随后成功也判取消。
                task->cancel_requested.store(true, std::memory_order_release);
                task->state = TaskState::Cancelling;
            }
            state = task->state;
        }
        available_.notify_one();
        return snapshot(task, state);
    }

    TaskOutcome result(const std::string& id) const {
        std::shared_ptr<Record> task;
        {
            std::lock_guard lock(mutex_);
            task = find(id);
            if (!terminal(task->state)) throw ExecutionError("task_not_finished", "Task result is not available before completion");
        }
        // 终态结果不可变；引用保证 release 后仍有效。大字符串复制不占用状态锁。
        return task->outcome;
    }

    void release(const std::string& id) {
        std::shared_ptr<Record> removed;
        {
            std::lock_guard lock(mutex_);
            removed = find(id);
            if (!terminal(removed->state)) throw ExecutionError("task_not_finished", "Only terminal tasks may be released");
            records_.erase(id);
        }
        // 图/结果的最后一次释放也在状态锁之外，status 不等待大对象析构。
    }

private:
    struct Record {
        explicit Record(TaskRequest value) : request(std::move(value)) {}
        std::string id;
        TaskRequest request;
        TaskState state{TaskState::Queued};
        std::atomic_bool cancel_requested{false};
        TaskOutcome outcome;
    };

    std::shared_ptr<Record> find(const std::string& id) const {
        const auto found = records_.find(id);
        if (found == records_.end()) throw ExecutionError("unknown_job", "Unknown task ID: " + id, {}, {}, {}, "/task_id");
        return found->second;
    }

    static TaskSnapshot snapshot(const std::shared_ptr<Record>& task, TaskState state) {
        TaskSnapshot result{task->id, state, {}};
        // captured state 非终态时不触碰可能正由 worker 发布的 outcome。
        if (terminal(state)) result.error = task->outcome.error;
        return result;
    }

    TaskOutcome run(const std::shared_ptr<Record>& task) {
        try {
            auto outcome = runner_(task->request, task->cancel_requested);
            if (outcome.result_json.size() > kMaximumResultBytes) {
                return {{}, TaskError{"result_too_large", "Task result exceeds 4 MiB", {}, {}, {}, {}}};
            }
            if (outcome.error) {
                // 失败可携带统计等诊断负载；状态由 error 决定，不能把有JSON等同于成功。
                if (outcome.error->code.empty()) outcome.error->code = "runner_failed";
            }
            return outcome;
        } catch (const ExecutionError& error) {
            return {{}, error_from(error)};
        } catch (const std::exception& error) {
            return {{}, TaskError{"task_execution_failed", error.what(), {}, {}, {}, {}}};
        } catch (...) {
            return {{}, TaskError{"task_execution_failed", "Runner threw an unknown exception", {}, {}, {}, {}}};
        }
    }

    void work() {
        while (true) {
            std::shared_ptr<Record> task;
            bool execute = false;
            {
                std::unique_lock lock(mutex_);
                available_.wait(lock, [this] { return stopping_ || pending_ != nullptr; });
                if (!pending_) return; // 停止且没有待执行任务。
                task = std::exchange(pending_, {});
                execute = !stopping_ && !task->cancel_requested.load(std::memory_order_acquire);
                if (execute) task->state = TaskState::Running;
            }

            // 唯一执行 Runner 的位置，绝不持有 mutex_；status/cancel 可立即处理。
            auto outcome = execute ? run(task) : cancelled_outcome();
            {
                std::lock_guard lock(mutex_);
                if (task->state == TaskState::Cancelling ||
                    task->cancel_requested.load(std::memory_order_acquire) || stopping_) {
                    task->outcome = cancelled_outcome();
                    task->state = TaskState::Cancelled;
                } else {
                    task->outcome = std::move(outcome);
                    task->state = task->outcome.error ? TaskState::Failed : TaskState::Succeeded;
                }
                // 终态发布与释放活动槽是原子状态变更，submit不必join上一次线程。
                active_.reset();
            }
        }
    }

    Runner runner_;
    mutable std::mutex mutex_;
    std::condition_variable available_;
    std::unordered_map<std::string, std::shared_ptr<Record>> records_;
    std::shared_ptr<Record> active_, pending_;
    std::uint64_t next_id_{1};
    bool stopping_{};
    std::thread worker_;
};

TaskService::TaskService(Runner runner) : impl_(std::make_unique<Impl>(std::move(runner))) {}
TaskService::~TaskService() = default;
std::string TaskService::submit(TaskRequest request) { return impl_->submit(std::move(request)); }
TaskSnapshot TaskService::status(const std::string& id) const { return impl_->status(id); }
TaskSnapshot TaskService::cancel(const std::string& id) { return impl_->cancel(id); }
TaskOutcome TaskService::result(const std::string& id) const { return impl_->result(id); }
void TaskService::release(const std::string& id) { impl_->release(id); }

} // namespace audioprocess
