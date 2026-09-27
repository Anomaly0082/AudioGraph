#include "audioprocess/node.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/streaming_node.h"
#include "audioprocess/realtime_node.h"

#include <algorithm>
#include <cmath>
#include <unordered_set>
#include <utility>

namespace audioprocess {
namespace {

bool parameter_type_matches(ParameterType type, const ParameterValue& value) {
    switch (type) {
    case ParameterType::Number: return std::holds_alternative<double>(value);
    case ParameterType::Text: return std::holds_alternative<std::string>(value);
    case ParameterType::Boolean: return std::holds_alternative<bool>(value);
    case ParameterType::FilePath: return std::holds_alternative<std::filesystem::path>(value);
    }
    return false;
}

void validate_parameter_value(const ParameterDescriptor& descriptor, const ParameterValue& value) {
    const auto fail = [&](const std::string& code, const std::string& message) {
        throw ExecutionError(code, message + ": " + descriptor.id, {}, {}, descriptor.id);
    };
    if (!parameter_type_matches(descriptor.type, value)) {
        fail("parameter_type_mismatch", "Parameter has the wrong type");
    }
    if (const auto* number = std::get_if<double>(&value)) {
        if (!std::isfinite(*number)) fail("invalid_parameter", "Parameter must be finite");
        if (descriptor.integer_only && std::floor(*number) != *number) {
            fail("invalid_parameter", "Parameter must be an integer");
        }
        if ((descriptor.minimum && *number < *descriptor.minimum) ||
            (descriptor.maximum && *number > *descriptor.maximum)) {
            fail("parameter_out_of_range", "Parameter is outside the permitted range");
        }
    }
    if (!descriptor.enum_values.empty()) {
        const auto& text = std::get<std::string>(value);
        if (std::ranges::find(descriptor.enum_values, text) == descriptor.enum_values.end()) {
            fail("invalid_parameter", "Parameter is not an allowed enum value");
        }
    }
}

void validate_ports(const std::vector<PortDescriptor>& ports) {
    std::unordered_set<std::string> ids;
    for (const auto& port : ports) {
        if (port.id.empty() || !ids.insert(port.id).second) {
            throw ExecutionError("invalid_descriptor", "Port ids must be nonempty and unique", {}, port.id);
        }
        switch (port.type) {
        case DataType::Audio: case DataType::Number: case DataType::Text: case DataType::FilePath:
        case DataType::AudioStream: break;
        default: throw ExecutionError("invalid_descriptor", "Unknown port data type", {}, port.id);
        }
    }
}

void validate_descriptor(const NodeDescriptor& descriptor) {
    if (descriptor.type_id.empty()) throw ExecutionError("invalid_descriptor", "Node type id must be provided");
    switch (descriptor.execution_domain) {
    case ExecutionDomain::Synchronous: case ExecutionDomain::Realtime:
    case ExecutionDomain::Asynchronous: case ExecutionDomain::Streaming: break;
    default: throw ExecutionError("invalid_descriptor", "Unknown execution domain");
    }
    if (descriptor.execution_domain != ExecutionDomain::Realtime &&
        (descriptor.realtime_role != RealtimeRole::None || descriptor.realtime_capabilities)) {
        throw ExecutionError("invalid_descriptor", "Realtime metadata requires the Realtime execution domain");
    }
    if (descriptor.execution_domain == ExecutionDomain::Realtime) {
        if (!descriptor.realtime_capabilities || !descriptor.realtime_capabilities->format.valid() ||
            descriptor.realtime_capabilities->maximum_block_frames == 0 || descriptor.stream_role != StreamRole::None) {
            throw ExecutionError("invalid_descriptor", "Realtime nodes require valid format/block capabilities and no streaming role");
        }
        switch (descriptor.realtime_role) {
        case RealtimeRole::Source: case RealtimeRole::Processor: case RealtimeRole::Sink: break;
        default: throw ExecutionError("invalid_descriptor", "Realtime nodes require an explicit role");
        }
    }
    validate_ports(descriptor.inputs);
    validate_ports(descriptor.outputs);
    std::unordered_set<std::string> ids;
    for (const auto& parameter : descriptor.parameters) {
        if (parameter.id.empty() || !ids.insert(parameter.id).second) {
            throw ExecutionError("invalid_descriptor", "Parameter ids must be nonempty and unique", {}, {}, parameter.id);
        }
        switch (parameter.type) {
        case ParameterType::Number: case ParameterType::Text:
        case ParameterType::Boolean: case ParameterType::FilePath: break;
        default: throw ExecutionError("invalid_descriptor", "Unknown parameter type", {}, {}, parameter.id);
        }
        if ((parameter.minimum || parameter.maximum) && parameter.type != ParameterType::Number) {
            throw ExecutionError("invalid_descriptor", "Numeric bounds require a Number parameter", {}, {}, parameter.id);
        }
        if (parameter.integer_only && parameter.type != ParameterType::Number) {
            throw ExecutionError("invalid_descriptor", "Integer constraint requires a Number parameter", {}, {}, parameter.id);
        }
        if ((parameter.minimum && !std::isfinite(*parameter.minimum)) ||
            (parameter.maximum && !std::isfinite(*parameter.maximum)) ||
            (parameter.minimum && parameter.maximum && *parameter.minimum > *parameter.maximum)) {
            throw ExecutionError("invalid_descriptor", "Invalid parameter range", {}, {}, parameter.id);
        }
        if (!parameter.enum_values.empty() && parameter.type != ParameterType::Text) {
            throw ExecutionError("invalid_descriptor", "Enum values require a Text parameter", {}, {}, parameter.id);
        }
        std::unordered_set<std::string> values;
        for (const auto& value : parameter.enum_values) {
            if (!values.insert(value).second) {
                throw ExecutionError("invalid_descriptor", "Duplicate enum value", {}, {}, parameter.id);
            }
        }
        if (parameter.default_value) validate_parameter_value(parameter, *parameter.default_value);
    }
}

bool matching_ports(const std::vector<PortDescriptor>& expected, const std::vector<PortDescriptor>& received) {
    if (expected.size() != received.size()) return false;
    for (const auto& port : expected) {
        const auto found = std::ranges::find(received, port.id, &PortDescriptor::id);
        if (found == received.end() || found->type != port.type || found->required != port.required) return false;
    }
    return true;
}

void validate_instance_descriptor(const NodeDescriptor& expected, const NodeDescriptor& actual) {
    if (actual.type_id != expected.type_id || actual.execution_domain != expected.execution_domain ||
        actual.stream_role != expected.stream_role || !matching_ports(expected.inputs, actual.inputs) ||
        !matching_ports(expected.outputs, actual.outputs) || actual.realtime_role != expected.realtime_role ||
        actual.realtime_capabilities != expected.realtime_capabilities) {
        throw ExecutionError("invalid_factory", "Node factory returned a different execution contract: " + expected.type_id);
    }
}

bool only_stream_port(const std::vector<PortDescriptor>& ports) {
    return ports.size() == 1 && ports[0].type == DataType::AudioStream && ports[0].required;
}

}  // namespace

void NodeRegistry::register_type(NodeDescriptor descriptor, Factory factory) {
    validate_descriptor(descriptor);
    if (!factory) throw ExecutionError("invalid_descriptor", "Node factory must be provided");
    if (entries_.contains(descriptor.type_id)) {
        throw ExecutionError("duplicate_node_type", "Duplicate node type id: " + descriptor.type_id);
    }
    const auto is_stream = [](const PortDescriptor& port) { return port.type == DataType::AudioStream; };
    if (descriptor.execution_domain == ExecutionDomain::Streaming || descriptor.execution_domain == ExecutionDomain::Realtime ||
        descriptor.stream_role != StreamRole::None ||
        std::ranges::any_of(descriptor.inputs, is_stream) || std::ranges::any_of(descriptor.outputs, is_stream)) {
        throw ExecutionError("invalid_descriptor", "Streaming and realtime nodes require their dedicated registration methods");
    }
    const auto type_id = descriptor.type_id;
    entries_.emplace(type_id, Entry{std::move(descriptor), std::move(factory), {}, {}});
}

void NodeRegistry::register_stream_type(NodeDescriptor descriptor, StreamFactory factory) {
    validate_descriptor(descriptor);
    if (!factory || descriptor.execution_domain != ExecutionDomain::Streaming) {
        throw ExecutionError("invalid_descriptor", "A Streaming descriptor and stream factory are required");
    }
    if (entries_.contains(descriptor.type_id)) {
        throw ExecutionError("duplicate_node_type", "Duplicate node type id: " + descriptor.type_id);
    }
    bool valid_role = false;
    switch (descriptor.stream_role) {
    case StreamRole::Source:
        valid_role = descriptor.inputs.empty() && only_stream_port(descriptor.outputs); break;
    case StreamRole::Processor:
        valid_role = only_stream_port(descriptor.inputs) && only_stream_port(descriptor.outputs); break;
    case StreamRole::Sink:
        valid_role = only_stream_port(descriptor.inputs) && std::ranges::all_of(descriptor.outputs, [](const auto& port) {
            return port.type == DataType::Number || port.type == DataType::Text || port.type == DataType::FilePath;
        });
        break;
    case StreamRole::None: break;
    }
    if (!valid_role) throw ExecutionError("invalid_descriptor", "Stream role and ports do not match the linear stream contract");
    const auto type_id = descriptor.type_id;
    entries_.emplace(type_id, Entry{std::move(descriptor), {}, std::move(factory), {}});
}

void NodeRegistry::register_realtime_endpoint(NodeDescriptor descriptor) {
    validate_descriptor(descriptor);
    if (entries_.contains(descriptor.type_id)) {
        throw ExecutionError("duplicate_node_type", "Duplicate node type id: " + descriptor.type_id);
    }
    const bool source = descriptor.realtime_role == RealtimeRole::Source &&
        descriptor.inputs.empty() && only_stream_port(descriptor.outputs);
    const bool sink = descriptor.realtime_role == RealtimeRole::Sink &&
        only_stream_port(descriptor.inputs) && descriptor.outputs.empty();
    const auto device = std::ranges::find(descriptor.parameters, std::string("device_id"), &ParameterDescriptor::id);
    if (descriptor.execution_domain != ExecutionDomain::Realtime || (!source && !sink) ||
        device == descriptor.parameters.end() || device->type != ParameterType::Text || !device->required) {
        throw ExecutionError("invalid_descriptor", "Realtime endpoints require source/sink ports and required Text device_id");
    }
    const auto type_id = descriptor.type_id;
    entries_.emplace(type_id, Entry{std::move(descriptor), {}, {}, {}});
}

void NodeRegistry::register_realtime_type(NodeDescriptor descriptor, RealtimeFactory factory) {
    validate_descriptor(descriptor);
    if (entries_.contains(descriptor.type_id)) {
        throw ExecutionError("duplicate_node_type", "Duplicate node type id: " + descriptor.type_id);
    }
    if (!factory || descriptor.execution_domain != ExecutionDomain::Realtime ||
        descriptor.realtime_role != RealtimeRole::Processor || !only_stream_port(descriptor.inputs) ||
        !only_stream_port(descriptor.outputs)) {
        throw ExecutionError("invalid_descriptor", "Realtime processors require one audio stream input/output and a factory");
    }
    const auto type_id = descriptor.type_id;
    entries_.emplace(type_id, Entry{std::move(descriptor), {}, {}, std::move(factory)});
}

const NodeDescriptor& NodeRegistry::descriptor(const std::string& type_id) const {
    const auto iterator = entries_.find(type_id);
    if (iterator == entries_.end()) {
        throw ExecutionError("unknown_node_type", "Unknown node type: " + type_id);
    }
    return iterator->second.descriptor;
}

ParameterMap NodeRegistry::normalize_parameters(const std::string& type_id, const ParameterMap& parameters) const {
    const auto& schema = descriptor(type_id).parameters;
    ParameterMap normalized = parameters;
    for (const auto& [id, value] : parameters) {
        const auto found = std::ranges::find(schema, id, &ParameterDescriptor::id);
        if (found == schema.end()) {
            throw ExecutionError("unknown_parameter", "Unknown parameter: " + id, {}, {}, id);
        }
        validate_parameter_value(*found, value);
    }
    for (const auto& parameter : schema) {
        if (normalized.contains(parameter.id)) continue;
        if (parameter.default_value) normalized.emplace(parameter.id, *parameter.default_value);
        else if (parameter.required) {
            throw ExecutionError("missing_parameter", "Required parameter is missing: " + parameter.id, {}, {}, parameter.id);
        }
    }
    return normalized;
}

std::unique_ptr<ISyncNode> NodeRegistry::create(const std::string& type_id, const ParameterMap& parameters) const {
    auto normalized = normalize_parameters(type_id, parameters);
    const auto& entry = entries_.at(type_id);
    if (!entry.factory || entry.descriptor.execution_domain != ExecutionDomain::Synchronous) {
        throw ExecutionError("unsupported_execution_domain", "Node does not support synchronous whole-value creation: " + type_id);
    }
    auto instance = entry.factory(normalized);
    if (!instance) throw ExecutionError("invalid_factory", "Node factory returned null: " + type_id);
    validate_instance_descriptor(entry.descriptor, instance->descriptor());
    // 参数 Schema 以 Registry 为唯一权威；实例描述中的展示文案不参与运行时校验。
    return instance;
}

std::unique_ptr<IStreamNode> NodeRegistry::create_stream(const std::string& type_id, const ParameterMap& parameters) const {
    auto normalized = normalize_parameters(type_id, parameters);
    const auto& entry = entries_.at(type_id);
    if (!entry.stream_factory || entry.descriptor.execution_domain != ExecutionDomain::Streaming) {
        throw ExecutionError("unsupported_execution_domain", "Node does not support streaming creation: " + type_id);
    }
    auto instance = entry.stream_factory(normalized);
    if (!instance) throw ExecutionError("invalid_factory", "Stream factory returned null: " + type_id);
    validate_instance_descriptor(entry.descriptor, instance->descriptor());
    bool role_matches = false;
    switch (entry.descriptor.stream_role) {
    case StreamRole::Source: role_matches = dynamic_cast<IAudioStreamSource*>(instance.get()) != nullptr; break;
    case StreamRole::Processor: role_matches = dynamic_cast<IAudioStreamProcessor*>(instance.get()) != nullptr; break;
    case StreamRole::Sink: role_matches = dynamic_cast<IAudioStreamSink*>(instance.get()) != nullptr; break;
    case StreamRole::None: break;
    }
    if (!role_matches) throw ExecutionError("invalid_factory", "Stream factory did not implement the declared role: " + type_id);
    return instance;
}

std::unique_ptr<IRealtimeProcessor> NodeRegistry::create_realtime(const std::string& type_id, const ParameterMap& parameters) const {
    auto normalized = normalize_parameters(type_id, parameters);
    const auto& entry = entries_.at(type_id);
    if (!entry.realtime_factory || entry.descriptor.execution_domain != ExecutionDomain::Realtime ||
        entry.descriptor.realtime_role != RealtimeRole::Processor) {
        throw ExecutionError("unsupported_execution_domain", "Node does not support realtime processor creation: " + type_id);
    }
    auto instance = entry.realtime_factory(normalized);
    if (!instance) throw ExecutionError("invalid_factory", "Realtime factory returned null: " + type_id);
    validate_instance_descriptor(entry.descriptor, instance->descriptor());
    return instance;
}

std::vector<NodeDescriptor> NodeRegistry::descriptors() const {
    std::vector<NodeDescriptor> result;
    result.reserve(entries_.size());
    for (const auto& [type_id, entry] : entries_) {
        (void)type_id;
        result.push_back(entry.descriptor);
    }
    std::ranges::sort(result, {}, &NodeDescriptor::type_id);
    return result;
}

}  // namespace audioprocess
