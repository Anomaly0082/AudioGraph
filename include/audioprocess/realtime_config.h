#pragma once

#include "audioprocess/realtime_session.h"
#include <filesystem>
#include <string>
#include <string_view>

namespace audioprocess {

// P3 专用实时会话配置，不是 GraphDefinition；设备回调不执行离线文件图。
struct RealtimeRouteConfig {
    std::string input_device;
    std::string output_device;
    RealtimeSessionConfig session;
};

[[nodiscard]] RealtimeRouteConfig parse_realtime_config(std::string_view json);
[[nodiscard]] RealtimeRouteConfig load_realtime_config(const std::filesystem::path& file);
void validate_realtime_config(const RealtimeRouteConfig& config);

} // namespace audioprocess
