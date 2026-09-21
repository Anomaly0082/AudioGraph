#include "audioprocess/task_service.h"
#include "audioprocess/execution_error.h"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <future>
#include <iostream>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
using namespace audioprocess;
using namespace std::chrono_literals;

void require(bool condition, const std::string& message) {
    if (!condition) throw std::runtime_error(message);
}
template<class Action>
void rejects(Action action, const std::string& code) {
    try { action(); }
    catch (const ExecutionError& error) {
        require(error.code == code, "Unexpected rejection code: " + error.code + ", expected " + code);
        return;
    }
    throw std::runtime_error("Expected rejection: " + code);
}
bool terminal(TaskState state) {
    return state == TaskState::Succeeded || state == TaskState::Failed || state == TaskState::Cancelled;
}

// 每个门控都有限时保险；失败断言触发Service析构时，mock runner不会永远阻塞join。
class Gate {
public:
    void open() { std::lock_guard lock(mutex_); open_ = true; condition_.notify_all(); }
    bool wait() {
        std::unique_lock lock(mutex_);
        return condition_.wait_for(lock, 3s, [&] { return open_; });
    }
private:
    std::mutex mutex_;
    std::condition_variable condition_;
    bool open_{};
};
struct OpenOnExit {
    std::shared_ptr<Gate> gate;
    ~OpenOnExit() { gate->open(); }
};

TaskSnapshot wait_terminal(TaskService& service, const std::string& id) {
    std::mutex mutex;
    std::condition_variable condition;
    std::unique_lock lock(mutex);
    const auto deadline = std::chrono::steady_clock::now() + 3s;
    while (std::chrono::steady_clock::now() < deadline) {
        auto snapshot = service.status(id);
        if (terminal(snapshot.state)) return snapshot;
        condition.wait_for(lock, 1ms);
    }
    throw std::runtime_error("Task did not reach a terminal state within the test timeout");
}
TaskOutcome success(std::string json = "{\"ok\":true}") { return {std::move(json), std::nullopt}; }

void test_nonblocking_cancel_busy_and_publication() {
    auto gate = std::make_shared<Gate>();
    auto entered = std::make_shared<std::promise<void>>();
    auto entry = entered->get_future();
    TaskService service([gate, entered](const TaskRequest&, std::atomic_bool&) {
        entered->set_value();
        if (!gate->wait()) throw std::runtime_error("Test gate was not released");
        return success(); // 故意不主动响应cancel，服务不能提前宣称worker已经停止。
    });
    OpenOnExit release{gate};
    const auto id = service.submit({});
    require(entry.wait_for(3s) == std::future_status::ready, "Submitted runner never started");
    require(service.status(id).state == TaskState::Running,
        "submit waited for runner completion or Running status was not observable");
    rejects([&] { static_cast<void>(service.result(id)); }, "task_not_finished");
    rejects([&] { service.release(id); }, "task_not_finished");
    rejects([&] { static_cast<void>(service.submit({})); }, "task_busy");
    const auto cancelled = service.cancel(id);
    require(cancelled.state == TaskState::Cancelling, "Cancellation falsely reported stopped work as terminal");
    require(service.status(id).state == TaskState::Cancelling, "Cancelling state was not published");
    rejects([&] { static_cast<void>(service.submit({})); }, "task_busy");
    gate->open();
    require(wait_terminal(service, id).state == TaskState::Cancelled,
        "Runner success overwrote cancellation requested before publication");
    const auto result = service.result(id);
    require(result.error && result.error->code == "cancelled" && result.result_json.empty(),
        "Cancelled result exposed a successful runner result");
    require(service.cancel(id).state == TaskState::Cancelled, "Repeated cancel changed terminal state");
}

void test_snapshot_ownership_and_late_cancel() {
    auto gate = std::make_shared<Gate>();
    auto entered = std::make_shared<std::promise<void>>();
    auto entry = entered->get_future();
    TaskService service([gate, entered](const TaskRequest& request, std::atomic_bool&) {
        entered->set_value();
        if (!gate->wait()) throw std::runtime_error("Test gate was not released");
        return success(std::get<std::string>(request.graph.nodes.at(0).parameters.at("text")));
    });
    OpenOnExit release{gate};
    TaskRequest request;
    request.graph.nodes = {{"text", "text_input", {{"text", std::string("{\"text\":\"original\"}")}}}};
    const auto id = service.submit(request);
    require(entry.wait_for(3s) == std::future_status::ready, "Runner never observed its owned request");
    request.graph.nodes[0].parameters["text"] = std::string("changed by caller");
    request.graph.nodes.clear();
    gate->open();
    require(wait_terminal(service, id).state == TaskState::Succeeded, "Snapshot task failed");
    auto result = service.result(id);
    require(result.result_json == "{\"text\":\"original\"}", "Runner request referenced caller-owned mutable graph state");
    result.result_json = "changed returned copy";
    require(service.result(id).result_json == "{\"text\":\"original\"}", "Caller changed stored outcome through a returned copy");
    require(service.cancel(id).state == TaskState::Succeeded && service.result(id).result_json == "{\"text\":\"original\"}",
        "Late cancellation rewrote a previously published success");
}

void test_accepted_cancel_wins_over_late_error() {
    auto gate = std::make_shared<Gate>();
    auto entered = std::make_shared<std::promise<void>>();
    auto entry = entered->get_future();
    TaskService service([gate, entered](const TaskRequest&, std::atomic_bool& cancelled) -> TaskOutcome {
        entered->set_value();
        if (!gate->wait()) throw std::runtime_error("Test gate was not released");
        // 即使runner误清借用标志，已受理的Cancelling状态也不能被撤销。
        cancelled.store(false);
        throw ExecutionError("late_failure", "Failure after cancellation was accepted");
    });
    OpenOnExit release{gate};
    const auto id = service.submit({});
    require(entry.wait_for(3s) == std::future_status::ready, "Cancellation priority runner did not start");
    require(service.cancel(id).state == TaskState::Cancelling, "Cancellation was not accepted before runner failure");
    gate->open();
    require(wait_terminal(service, id).state == TaskState::Cancelled &&
        service.result(id).error && service.result(id).error->code == "cancelled",
        "Late runner failure or cleared flag revoked an already accepted cancellation");
}

void test_error_fields_and_exception_convergence() {
    for (int behavior = 0; behavior < 4; ++behavior) {
        TaskService service([behavior](const TaskRequest&, std::atomic_bool&) -> TaskOutcome {
            if (behavior == 0) return {"{\"success\":false,\"diagnostic\":7}",
                TaskError{"fixture_error", "planned failure", "node", "port", "parameter", "/nodes/0"}};
            if (behavior == 1) throw ExecutionError("fixture_error", "planned failure", "node", "port", "parameter", "/nodes/0");
            if (behavior == 2) throw std::runtime_error("unexpected runner error");
            throw 42;
        });
        const auto id = service.submit({});
        const auto snapshot = wait_terminal(service, id);
        require(snapshot.state == TaskState::Failed && snapshot.error && !snapshot.error->code.empty(),
            "Runner failure/exception escaped the task state machine");
        const auto result = service.result(id);
        require(result.error && !result.error->code.empty(), "Failed task has no structured result error");
        if (behavior < 2) {
            const auto& error = *result.error;
            require(error.code == "fixture_error" && error.message == "planned failure" &&
                error.node_id == "node" && error.port_id == "port" && error.parameter_id == "parameter" &&
                error.field_path == "/nodes/0", "Service lost structured failure location fields");
        }
        if (behavior == 0) require(result.result_json == "{\"success\":false,\"diagnostic\":7}",
            "Failed runner result lost its bounded diagnostic payload");
        require(service.cancel(id).state == TaskState::Failed, "Late cancel rewrote a published failure");
    }
}

void test_submit_cancel_start_races() {
    TaskService service([](const TaskRequest&, std::atomic_bool&) { return success(); });
    for (int iteration = 0; iteration < 100; ++iteration) {
        const auto id = service.submit({});
        const auto cancellation = service.cancel(id);
        const auto final = wait_terminal(service, id);
        if (cancellation.state == TaskState::Succeeded) {
            require(final.state == TaskState::Succeeded, "Terminal success changed after racing cancel");
        } else {
            require(cancellation.state == TaskState::Cancelling || cancellation.state == TaskState::Cancelled,
                "Immediate cancel returned an impossible task state");
            require(final.state == TaskState::Cancelled, "Queued/running cancellation lost to late success publication");
        }
        service.release(id);
        rejects([&] { static_cast<void>(service.status(id)); }, "unknown_job");
    }
}

void test_capacity_release_and_unknown_ids() {
    TaskService service([](const TaskRequest&, std::atomic_bool&) { return success(); });
    rejects([&] { static_cast<void>(service.status("missing")); }, "unknown_job");
    rejects([&] { static_cast<void>(service.cancel("missing")); }, "unknown_job");
    rejects([&] { static_cast<void>(service.result("missing")); }, "unknown_job");
    rejects([&] { service.release("missing"); }, "unknown_job");
    std::vector<std::string> ids;
    for (int index = 0; index < 16; ++index) {
        const auto id = service.submit({});
        require(wait_terminal(service, id).state == TaskState::Succeeded, "Capacity fixture failed");
        ids.push_back(id);
    }
    rejects([&] { static_cast<void>(service.submit({})); }, "task_capacity");
    service.release(ids[0]);
    const auto next = service.submit({});
    require(next != ids[0] && wait_terminal(service, next).state == TaskState::Succeeded,
        "Release did not free capacity or reused an obsolete task identity");
    rejects([&] { static_cast<void>(service.result(ids[0])); }, "unknown_job");
    for (std::size_t i = 1; i < ids.size(); ++i) {
        require(service.status(ids[i]).state == TaskState::Succeeded, "Capacity handling evicted an unrelated record");
    }
}

void test_destructor_requests_cancel_and_joins() {
    auto entered = std::make_shared<std::promise<void>>();
    auto entry = entered->get_future();
    auto observed_cancel = std::make_shared<std::atomic_bool>(false);
    auto runner_finished = std::make_shared<std::atomic_bool>(false);
    {
        TaskService service([entered, observed_cancel, runner_finished](const TaskRequest&, std::atomic_bool& cancelled) {
            entered->set_value();
            std::mutex mutex;
            std::condition_variable condition;
            std::unique_lock lock(mutex);
            const auto deadline = std::chrono::steady_clock::now() + 3s;
            while (!cancelled.load() && std::chrono::steady_clock::now() < deadline) {
                condition.wait_for(lock, 1ms);
            }
            observed_cancel->store(cancelled.load());
            runner_finished->store(true);
            return success();
        });
        static_cast<void>(service.submit({}));
        require(entry.wait_for(3s) == std::future_status::ready, "Destructor test runner never started");
    }
    require(observed_cancel->load() && runner_finished->load(),
        "Service destructor returned before requesting cooperative cancellation and joining the worker");
}

void test_request_option_validation() {
    std::atomic_uint invocations{};
    TaskService service([&](const TaskRequest&, std::atomic_bool&) {
        invocations.fetch_add(1);
        return success();
    });
    const auto reject_any = [&](TaskRequest request) {
        bool rejected = false;
        try { static_cast<void>(service.submit(std::move(request))); }
        catch (const ExecutionError&) { rejected = true; }
        require(rejected, "Invalid task options reached the worker");
    };
    TaskRequest request;
    request.mode = ExecutionDomain::Asynchronous; reject_any(request);
    request = {}; request.block_frames = 0; reject_any(request);
    request = {}; request.block_frames = 65537; reject_any(request);
    request = {}; request.duration_seconds = 0; reject_any(request);
    request = {}; request.duration_seconds = 3601; reject_any(request);
    require(invocations.load() == 0, "Invalid task option created runnable work");
}

void test_result_retention_limit() {
    TaskService service([](const TaskRequest&, std::atomic_bool&) {
        return success("{\"padding\":\"" + std::string(4 * 1024 * 1024, 'x') + "\"}");
    });
    const auto id = service.submit({});
    const auto snapshot = wait_terminal(service, id);
    const auto result = service.result(id);
    require(snapshot.state == TaskState::Failed && result.error &&
        result.error->code == "result_too_large" && result.result_json.empty(),
        "Oversized result was retained rather than replaced with a bounded failure");
}
} // namespace

int main() {
    try {
        test_nonblocking_cancel_busy_and_publication();
        test_snapshot_ownership_and_late_cancel();
        test_accepted_cancel_wins_over_late_error();
        test_error_fields_and_exception_convergence();
        test_submit_cancel_start_races();
        test_capacity_release_and_unknown_ids();
        test_destructor_requests_cancel_and_joins();
        test_request_option_validation();
        test_result_retention_limit();
        std::cout << "Independent task service tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Task service test failure: " << error.what() << '\n';
        return 1;
    }
}
