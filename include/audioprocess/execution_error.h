#pragma once

#include <stdexcept>
#include <string>
#include <utility>

namespace audioprocess {

// code 用于机器判断，what() 面向用户；定位字段没有对应对象时为空。
class ExecutionError : public std::runtime_error {
public:
    ExecutionError(std::string code_value, std::string message,
                   std::string node = {}, std::string port = {},
                   std::string parameter = {}, std::string field = {})
        : std::runtime_error(std::move(message)), code(std::move(code_value)),
          node_id(std::move(node)), port_id(std::move(port)),
          parameter_id(std::move(parameter)), field_path(std::move(field)) {}

    std::string code;
    std::string node_id;
    std::string port_id;
    std::string parameter_id;
    std::string field_path;
};

}  // namespace audioprocess
