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

enum class StreamRole { None, Source, Processor, Sink };
class IStreamNode;

enum class RealtimeRole { None, Source, Processor, Sink };
struct RealtimeCapabilities {
    AudioFormat format{48000, 1};
    std::uint32_t maximum_block_frames{65536};
    bool supports_variable_blocks{true};
    bool offline_drivable{true};
    friend constexpr bool operator==(const RealtimeCapabilities&, const RealtimeCapabilities&) = default;
};
class IRealtimeProcessor;

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
    // Number 参数的可选约束，统一在配置预检阶段验证；不引入第二种数值类型。
    bool integer_only{false};
};

// Provenance is metadata, not a dependency on the plugin implementation or ABI.
struct PluginOrigin {
    std::string plugin_id;
    std::string plugin_version;
    std::string package_sha256;
    std::uint32_t abi_major{0};
    std::uint32_t abi_minor{1};
    friend bool operator==(const PluginOrigin&, const PluginOrigin&) = default;
};

struct NodeDescriptor {
    std::string type_id;
    std::string display_name;
    std::string description;
    ExecutionDomain execution_domain{ExecutionDomain::Synchronous};
    std::vector<PortDescriptor> inputs;
    std::vector<PortDescriptor> outputs;
    std::vector<ParameterDescriptor> parameters;
    StreamRole stream_role{StreamRole::None};
    RealtimeRole realtime_role{RealtimeRole::None};
    std::optional<RealtimeCapabilities> realtime_capabilities;
    std::optional<PluginOrigin> plugin;
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
    using StreamFactory = std::function<std::unique_ptr<IStreamNode>(const ParameterMap&)>;
    using RealtimeFactory = std::function<std::unique_ptr<IRealtimeProcessor>(const ParameterMap&)>;

    void register_type(NodeDescriptor descriptor, Factory factory);
    void register_stream_type(NodeDescriptor descriptor, StreamFactory factory);
    // 端点只声明设备 binding，不能通过工厂在编译/音频回调中打开设备。
    void register_realtime_endpoint(NodeDescriptor descriptor);
    void register_realtime_type(NodeDescriptor descriptor, RealtimeFactory factory);

    [[nodiscard]] const NodeDescriptor& descriptor(const std::string& type_id) const;
    // 仅检查配置和补齐默认值，不创建节点，也不访问文件或设备。
    [[nodiscard]] ParameterMap normalize_parameters(
        const std::string& type_id,
        const ParameterMap& parameters) const;
    [[nodiscard]] std::unique_ptr<IStreamNode> create_stream(
        const std::string& type_id, const ParameterMap& parameters) const;
    [[nodiscard]] std::unique_ptr<IRealtimeProcessor> create_realtime(
        const std::string& type_id, const ParameterMap& parameters) const;
    [[nodiscard]] std::unique_ptr<ISyncNode> create(
        const std::string& type_id,
        const ParameterMap& parameters) const;
    [[nodiscard]] std::vector<NodeDescriptor> descriptors() const;

private:
    struct Entry {
        NodeDescriptor descriptor;
        Factory factory;
        StreamFactory stream_factory;
        RealtimeFactory realtime_factory;
    };

    std::unordered_map<std::string, Entry> entries_;
};

}  // namespace audioprocess
