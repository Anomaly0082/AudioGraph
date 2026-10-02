#pragma once

#include "audioprocess/task_service.h"

#include <filesystem>
#include <string>
#include <string_view>

namespace audioprocess {

// 由宿主进程启动时设置，JSON 请求不能扩大权限。路径策略不是操作系统沙箱。
struct ControlPolicy {
    std::filesystem::path workspace;
    bool allow_devices{false};
    bool allow_monitor{false};
    // Host-only startup options; never accepted from a Graph or JSON request.
    std::filesystem::path plugin_snapshot_path;
    std::string plugin_snapshot_sha256;
    std::filesystem::path plugin_data_root;
};

// 只检查声明为 FilePath 的参数；在启动前和 worker 执行前重复检查。
void validate_control_paths(const GraphDefinition& graph, const ControlPolicy& policy);

// 同一个实例的 handle 由单一控制线程调用；任务在 TaskService 的 worker 中运行。
// 不启动 HTTP 服务、不执行脚本/命令、不把实时音频带到协议线程。
class ControlProtocol {
public:
    explicit ControlProtocol(ControlPolicy policy);
    [[nodiscard]] std::string handle(std::string_view request);
private:
    ControlPolicy policy_;
    std::string plugin_report_json_;
    NodeRegistry registry_;
    TaskService tasks_;
};

} // namespace audioprocess
