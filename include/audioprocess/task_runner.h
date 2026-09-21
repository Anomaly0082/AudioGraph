#pragma once

#include "audioprocess/task_service.h"

namespace audioprocess {

// 纯业务校验：显式选择整段/离线分块/实时模式，不创建节点、不访问文件或设备。
// 设备授权及文件路径边界由宿主在调用前检查；此层不隐式授予权限。
void validate_task_request(const TaskRequest& request, const NodeRegistry& registry);

// 在调用方提供的 worker 上执行；不自行创建全局调度线程。
// 取消标志仅在本次调用中借用。Runtime 错误可返回 error+详情，也可抛 ExecutionError。
[[nodiscard]] TaskOutcome execute_task_request(
    const TaskRequest& request, const NodeRegistry& registry, std::atomic_bool& cancellation_requested);

} // namespace audioprocess
