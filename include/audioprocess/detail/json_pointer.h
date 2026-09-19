#pragma once

#include <string>
#include <string_view>

namespace audioprocess::detail {

// RFC 6901：每个对象键先转义 ~ 和 /，再拼接到 JSON Pointer。
// 不依赖 JSON 库，核心校验与协议边界使用相同的定位规则。
[[nodiscard]] inline std::string json_pointer_token(std::string_view token) {
    std::string escaped;
    escaped.reserve(token.size());
    for (const char character : token) {
        if (character == '~') escaped += "~0";
        else if (character == '/') escaped += "~1";
        else escaped += character;
    }
    return escaped;
}

[[nodiscard]] inline std::string json_pointer_append(
    std::string_view pointer, std::string_view token) {
    return std::string(pointer) + "/" + json_pointer_token(token);
}

}  // namespace audioprocess::detail
