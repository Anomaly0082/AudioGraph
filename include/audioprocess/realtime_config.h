#pragma once

#include "audioprocess/realtime_session.h"
#include <filesystem>
#include <string>
#include <string_view>

namespace audioprocess {

// P3 配置的兼容读取。CLI 将它转换为 GraphDefinition，不再有第二条固定 Gain 执行路径。
struct RealtimeRouteConfig {
    std::string input_device;
    std::string output_device;
    float gain_db{0.0F};
    RealtimeSessionConfig session;
};

[[nodiscard]] RealtimeRouteConfig parse_realtime_config(std::string_view json);
[[nodiscard]] RealtimeRouteConfig load_realtime_config(const std::filesystem::path& file);
void validate_realtime_config(const RealtimeRouteConfig& config);

} // namespace audioprocess
