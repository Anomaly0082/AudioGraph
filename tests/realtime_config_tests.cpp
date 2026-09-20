#include "audioprocess/realtime_config.h"
#include "audioprocess/execution_error.h"
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>

int main() {
    try {
        using namespace audioprocess;
        const auto config = parse_realtime_config(R"({"schema_version":1,"input_device":"capture-id","output_device":"render-id","gain_db":-6})");
        if (config.input_device != "capture-id" || config.session.bridge.gain_db != -6 ||
            config.session.bridge.target_frames != 960 || !config.session.probe)
            throw std::runtime_error("Default configuration contract changed");
        const std::vector<std::string> invalid{
            "{}", "[]", "{", R"({"schema_version":1,"input_device":"i","output_device":"o","probe":false})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","gain_db":13})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","gain_db":1e100})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","target_frames":0})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","capacity_frames":2000})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","capacity_frames":2147483648})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","period_frames":0})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","target_frames":4096})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","period_frames":256.0})",
            R"({"schema_version":1,"input_device":"i","output_device":"o","period_frames":-1})",
            R"({"schema_version":2,"input_device":"i","output_device":"o"})",
            R"({"schema_version":1.0,"input_device":"i","output_device":"o"})",
            R"({"schema_version":1,"input_device":"i","input_device":"j","output_device":"o"})",
            R"({"schema_version":1,"input_device":"","output_device":"o"})",
            R"({"schema_version":1,"input_device":"i\u0000x","output_device":"o"})"
        };
        for (const auto& value : invalid) {
            bool rejected{};
            try { (void)parse_realtime_config(value); }
            catch (const ExecutionError& error) { rejected = error.code == "invalid_realtime_config"; }
            if (!rejected) throw std::runtime_error("Invalid config accepted: " + value);
        }
        std::cout << "Realtime config tests passed (no device access).\n";
        return 0;
    } catch (const std::exception& error) { std::cerr << error.what() << '\n'; return 1; }
}
