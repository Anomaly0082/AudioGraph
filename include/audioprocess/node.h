#pragma once

#include "audioprocess/data_value.h"

#include <atomic>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <unordered_map>
#include <variant>
#include <vector>

namespace audioprocess {

enum class ExecutionDomain {
    Synchronous,
    Realtime,
    Asynchronous,
    Streaming,
};

enum class ParameterType {
    Number,
    Text,
    Boolean,
    FilePath,
};

using ParameterValue = std::variant<double, std::string, bool, std::filesystem::path>;
using ParameterMap = std::unordered_map<std::string, ParameterValue>;
using InputValues = std::unordered_map<std::string, DataValue>;
using OutputValues = std::unordered_map<std::string, DataValue>;

struct PortDescriptor {
    std::string id;
    DataType type;
    bool required{true};
};

struct ParameterDescriptor {
    std::string id;
    ParameterType type;
    std::string description;
    bool required{false};
    std::optional<ParameterValue> default_value;
    std::optional<double> minimum;
    std::optional<double> maximum;
    std::string unit;
    std::vector<std::string> enum_values;
};

struct NodeDescriptor {
    std::string type_id;
    std::string display_name;
    std::string description;
    ExecutionDomain execution_domain{ExecutionDomain::Synchronous};
    std::vector<PortDescriptor> inputs;
    std::vector<PortDescriptor> outputs;
    std::vector<ParameterDescriptor> parameters;
};

struct ExecutionContext {
    std::atomic_bool* cancellation_requested{};

    [[nodiscard]] bool cancelled() const noexcept {
        return cancellation_requested != nullptr && cancellation_requested->load();
    }
};

class ISyncNode {
public:
    virtual ~ISyncNode() = default;

    [[nodiscard]] virtual const NodeDescriptor& descriptor() const noexcept = 0;
    [[nodiscard]] virtual OutputValues execute(
        const InputValues& inputs,
        ExecutionContext& context) = 0;
};

class NodeRegistry {
public:
    using Factory = std::function<std::unique_ptr<ISyncNode>(const ParameterMap&)>;

    void register_type(NodeDescriptor descriptor, Factory factory);

    [[nodiscard]] const NodeDescriptor& descriptor(const std::string& type_id) const;
    // 仅检查配置和补齐默认值，不创建节点，也不访问文件或设备。
    [[nodiscard]] ParameterMap normalize_parameters(
        const std::string& type_id,
        const ParameterMap& parameters) const;
    [[nodiscard]] std::unique_ptr<ISyncNode> create(
        const std::string& type_id,
        const ParameterMap& parameters) const;
    [[nodiscard]] std::vector<NodeDescriptor> descriptors() const;

private:
    struct Entry {
        NodeDescriptor descriptor;
        Factory factory;
    };

    std::unordered_map<std::string, Entry> entries_;
};

}  // namespace audioprocess
