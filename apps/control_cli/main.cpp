#include "audioprocess/control_protocol.h"
#include "audioprocess/graph_codec.h"

#include <iostream>
#include <set>

namespace ap = audioprocess;
namespace {
constexpr std::size_t maximum_line_bytes = 4 * 1024 * 1024;

int run(const std::vector<std::string>& arguments) {
    try {
        ap::ControlPolicy policy{std::filesystem::current_path()};
        std::set<std::string> seen;
        for (std::size_t i = 1; i < arguments.size(); ++i) {
            const auto& key = arguments[i];
            if (!seen.insert(key).second) throw ap::ExecutionError("invalid_arguments", "Duplicate host option");
            if (key == "--help") {
                std::cout << "control-cli [--workspace <directory>] [--allow-devices] [--allow-monitor]\n"
                    "One versioned JSON request per stdin line; one response per stdout line. EOF cancels and joins the active task.\n";
                return 0;
            }
            if (key == "--workspace" && i + 1 < arguments.size()) policy.workspace = ap::path_from_utf8(arguments[++i]);
            else if (key == "--allow-devices") policy.allow_devices = true;
            else if (key == "--allow-monitor") policy.allow_monitor = true;
            else throw ap::ExecutionError("invalid_arguments", "Unknown or incomplete host option: " + key);
        }
        ap::ControlProtocol protocol(std::move(policy));
        // 不用无限 getline：超过预算的行继续丢弃至换行，返回错误后仍接受下一条请求。
        std::string line;
        bool oversized = false;
        char character{};
        const auto respond = [&] {
            if (oversized) {
                std::cout << "{\"schema_version\":1,\"id\":null,\"success\":false,\"errors\":[{\"code\":\"invalid_request\",\"message\":\"Request exceeds 4 MiB\"}]}\n";
            } else {
                std::cout << protocol.handle(line) << '\n';
            }
            std::cout.flush();
            line.clear();
            oversized = false;
        };
        while (std::cin.get(character)) {
            if (character == '\n') { respond(); continue; }
            if (!oversized) {
                if (line.size() == maximum_line_bytes) { oversized = true; line.clear(); }
                else line.push_back(character);
            }
        }
        if (!line.empty() || oversized) respond();
        // protocol 析构会请求取消并等待清理；不能强制抢占用户 C++ 节点/系统阻塞调用。
        return std::cin.bad() || !std::cout ? 1 : 0;
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
}

#ifdef _WIN32
int wmain(int argc, wchar_t* argv[]) {
    std::vector<std::string> arguments;
    for (int i = 0; i < argc; ++i) arguments.push_back(ap::path_to_utf8(std::filesystem::path(argv[i])));
    return run(arguments);
}
#else
int main(int argc, char* argv[]) { return run({argv, argv + argc}); }
#endif
