#include "audioprocess/node.h"

#include <algorithm>
#include <stdexcept>
#include <utility>

namespace audioprocess {

void NodeRegistry::register_type(NodeDescriptor descriptor, Factory factory) {
    if (descriptor.type_id.empty()) {
        throw std::invalid_argument("Node type id cannot be empty");
    }
    if (!factory) {
        throw std::invalid_argument("Node factory cannot be empty");
    }

    const auto type_id = descriptor.type_id;
    const auto [iterator, inserted] = entries_.emplace(
        type_id, Entry{std::move(descriptor), std::move(factory)});
    (void)iterator;
    if (!inserted) {
        throw std::invalid_argument("Duplicate node type id: " + type_id);
    }
}

const NodeDescriptor& NodeRegistry::descriptor(const std::string& type_id) const {
    const auto iterator = entries_.find(type_id);
    if (iterator == entries_.end()) {
        throw std::invalid_argument("Unknown node type: " + type_id);
    }
    return iterator->second.descriptor;
}

std::unique_ptr<ISyncNode> NodeRegistry::create(
    const std::string& type_id,
    const ParameterMap& parameters) const {
    const auto iterator = entries_.find(type_id);
    if (iterator == entries_.end()) {
        throw std::invalid_argument("Unknown node type: " + type_id);
    }
    return iterator->second.factory(parameters);
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

