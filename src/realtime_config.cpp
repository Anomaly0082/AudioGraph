#include "audioprocess/realtime_config.h"
#include "audioprocess/execution_error.h"
#include <nlohmann/json.hpp>
#include <algorithm>
#include <fstream>
#include <set>
#include <vector>

namespace audioprocess {
namespace {
using Json = nlohmann::json;
constexpr std::size_t maximum_bytes = 65536;
[[noreturn]] void invalid(const std::string& message, const std::string& field = {}) {
    throw ExecutionError("invalid_realtime_config", message, {}, {}, {}, field);
}
std::uint32_t integer(const Json& item, const char* key, std::uint32_t fallback) {
    if (!item.contains(key)) return fallback;
    const auto& value = item.at(key);
    if (!value.is_number_unsigned() || value.get<std::uint64_t>() > UINT32_MAX)
        invalid("Expected an unsigned 32-bit integer", "/" + std::string(key));
    return value.get<std::uint32_t>();
}
std::string device(const Json& item, const char* key) {
    if (!item.contains(key) || !item.at(key).is_string()) invalid("Device ID is required", "/" + std::string(key));
    auto value = item.at(key).get<std::string>();
    if (value.empty() || value.find('\0') != std::string::npos) invalid("Device ID must be nonempty and contain no NUL", "/" + std::string(key));
    return value;
}
}

void validate_realtime_config(const RealtimeRouteConfig& config) {
    for (const auto* id : {&config.input_device, &config.output_device})
        if (id->empty() || id->find('\0') != std::string::npos) invalid("Explicit input and output device IDs are required");
    if (config.session.device_period_frames < 32 || config.session.device_period_frames > 2048)
        invalid("Device period must be between 32 and 2048 frames", "/period_frames");
    try { const RealtimeBridge validation(config.session.bridge); }
    catch (const std::exception& error) { invalid(error.what()); }
}

RealtimeRouteConfig parse_realtime_config(std::string_view text) {
    if (text.size() > maximum_bytes) invalid("Realtime config exceeds 64 KiB");
    std::vector<std::set<std::string>> keys;
    const auto callback = [&keys](int depth, Json::parse_event_t event, Json& value) {
        if (depth > 8) invalid("Realtime config nesting exceeds 8");
        if (event == Json::parse_event_t::object_start) keys.emplace_back();
        if (event == Json::parse_event_t::key && !keys.back().insert(value.get<std::string>()).second)
            invalid("Duplicate JSON key");
        if (event == Json::parse_event_t::object_end) keys.pop_back();
        return true;
    };
    Json root;
    try { root = Json::parse(text.begin(), text.end(), callback); }
    catch (const Json::exception& error) { invalid(error.what()); }
    if (!root.is_object()) invalid("Expected a configuration object");
    const std::set<std::string> fields{"schema_version", "input_device", "output_device", "gain_db",
        "target_frames", "capacity_frames", "period_frames"};
    for (const auto& [key, value] : root.items()) {
        (void)value;
        if (!fields.contains(key)) invalid("Unknown field: " + key);
    }
    if (!root.contains("schema_version") || !root.at("schema_version").is_number_integer() || root.at("schema_version") != 1)
        invalid("Expected schema_version integer 1", "/schema_version");
    RealtimeRouteConfig config;
    config.input_device = device(root, "input_device");
    config.output_device = device(root, "output_device");
    if (root.contains("gain_db")) {
        if (!root.at("gain_db").is_number()) invalid("Expected numeric gain_db", "/gain_db");
        config.session.bridge.gain_db = root.at("gain_db").get<float>();
    }
    config.session.bridge.capacity_frames = integer(root, "capacity_frames", config.session.bridge.capacity_frames);
    config.session.bridge.target_frames = integer(root, "target_frames", config.session.bridge.target_frames);
    config.session.device_period_frames = integer(root, "period_frames", config.session.device_period_frames);
    validate_realtime_config(config);
    return config;
}

RealtimeRouteConfig load_realtime_config(const std::filesystem::path& path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) invalid("Cannot open realtime config");
    char buffer[4096];
    std::string text;
    while (file.read(buffer, sizeof(buffer)) || file.gcount() > 0) {
        text.append(buffer, static_cast<std::size_t>(file.gcount()));
        if (text.size() > maximum_bytes) invalid("Realtime config exceeds 64 KiB");
    }
    if (file.bad()) invalid("Failed to read realtime config");
    return parse_realtime_config(text);
}
} // namespace audioprocess
