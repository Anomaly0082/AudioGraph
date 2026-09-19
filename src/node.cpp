#include "audioprocess/node.h"
#include "audioprocess/execution_error.h"

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
        case DataType::Audio: case DataType::Number: case DataType::Text: case DataType::FilePath: break;
        default: throw ExecutionError("invalid_descriptor", "Unknown port data type", {}, port.id);
        }
    }
}

}  // namespace

void NodeRegistry::register_type(NodeDescriptor descriptor, Factory factory) {
    if (descriptor.type_id.empty() || !factory) {
        throw ExecutionError("invalid_descriptor", "Node type id and factory must be provided");
    }
    if (entries_.contains(descriptor.type_id)) {
        throw ExecutionError("duplicate_node_type", "Duplicate node type id: " + descriptor.type_id);
    }
    switch (descriptor.execution_domain) {
    case ExecutionDomain::Synchronous: case ExecutionDomain::Realtime:
    case ExecutionDomain::Asynchronous: case ExecutionDomain::Streaming: break;
    default: throw ExecutionError("invalid_descriptor", "Unknown execution domain");
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
    const auto type_id = descriptor.type_id;
    entries_.emplace(type_id, Entry{std::move(descriptor), std::move(factory)});
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
    auto instance = entry.factory(normalized);
    if (!instance) throw ExecutionError("invalid_factory", "Node factory returned null: " + type_id);
    const auto& actual = instance->descriptor();
    if (actual.type_id != type_id) {
        throw ExecutionError("invalid_factory", "Node factory returned a different node type: " + type_id);
    }
    const auto matching_ports = [](const auto& expected, const auto& received) {
        if (expected.size() != received.size()) return false;
        for (const auto& port : expected) {
            const auto found = std::ranges::find(received, port.id, &PortDescriptor::id);
            if (found == received.end() || found->type != port.type || found->required != port.required) return false;
        }
        return true;
    };
    if (actual.execution_domain != entry.descriptor.execution_domain ||
        !matching_ports(entry.descriptor.inputs, actual.inputs) ||
        !matching_ports(entry.descriptor.outputs, actual.outputs)) {
        throw ExecutionError("invalid_factory", "Node factory returned a different execution contract: " + type_id);
    }
    // 参数 Schema 以 Registry 为唯一权威；实例描述中的展示文案不参与运行时校验。
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
