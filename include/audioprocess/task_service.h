#pragma once

#include "audioprocess/graph.h"

#include <atomic>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>

namespace audioprocess {

enum class TaskState { Queued, Running, Cancelling, Succeeded, Failed, Cancelled };
[[nodiscard]] const char* task_state_name(TaskState state) noexcept;

struct TaskError {
    std::string code;
    std::string message;
    std::string node_id;
    std::string port_id;
    std::string parameter_id;
    std::string field_path;
};

struct TaskRequest {
    GraphDefinition graph;
    ExecutionDomain mode{ExecutionDomain::Synchronous};
    std::uint32_t block_frames{256};
    bool probe{true};
    std::uint32_t duration_seconds{10};
};

// result_json 是 Runner 生成的不透明协议负载；Service 不解析或拼装业务 JSON。
struct TaskOutcome {
    std::string result_json;
    std::optional<TaskError> error;
};

struct TaskSnapshot {
    std::string id;
    TaskState state{TaskState::Queued};
    std::optional<TaskError> error;
};

// 传输无关的单活动任务服务，最多保留16条记录，不创建无界等待队列。
// submit复制/接收请求快照；Runner在唯一worker线程执行，status/cancel不等待Runner结束。
// 所有权：取消标志仅在Runner调用期间有效，Runner不能保留它或请求引用供后台继续使用。
class TaskService {
public:
    using Runner = std::function<TaskOutcome(const TaskRequest&, std::atomic_bool&)>;

    explicit TaskService(Runner runner);
    // 析构请求取消并join。Runner不响应取消时可能等待；不强制抢占。
    // 析构必须由外部所有者执行，此时不能再并发调用公共方法。
    ~TaskService();
    TaskService(const TaskService&) = delete;
    TaskService& operator=(const TaskService&) = delete;

    [[nodiscard]] std::string submit(TaskRequest request);
    [[nodiscard]] TaskSnapshot status(const std::string& id) const;
    // 与终态发布共用锁：cancel先获得锁则取消优先（包括后续错误），终态后取消不改变结果。
    [[nodiscard]] TaskSnapshot cancel(const std::string& id);
    // 仅终态可取；取消结果包含 cancelled 错误且不保留成功负载。
    [[nodiscard]] TaskOutcome result(const std::string& id) const;
    // 仅终态可释放；释放后ID不可查询或复用。ID仅当前Service会话内有意义。
    void release(const std::string& id);

private:
    class Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace audioprocess
