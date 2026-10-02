#pragma once

#include "audioprocess/node.h"

#include <filesystem>
#include <cstdint>
#include <string>

namespace audioprocess {

// Captured once by the launching host. A missing snapshot means built-in nodes only.
struct PluginHostOptions {
    std::filesystem::path snapshot_path;
    std::string snapshot_sha256;
    std::filesystem::path data_root;
    std::filesystem::path workspace;
};

namespace detail {
inline constexpr std::uint64_t plugin_text_output_limit = 64ull * 1024;
[[nodiscard]] constexpr bool plugin_text_output_fits(std::uint64_t used, std::uint64_t next) noexcept {
    return used <= plugin_text_output_limit && next <= plugin_text_output_limit-used;
}
[[nodiscard]] constexpr std::uint64_t plugin_call_output_limit(bool has_audio_output) noexcept {
    return has_audio_output ? 256ull*1024*1024 : plugin_text_output_limit;
}
} // namespace detail

// Registers only statically validated whole-value node contracts. The plugin
// DLL is first loaded by a node factory at execution time, after file identity
// is checked again. Native plugins are trusted code, not a process sandbox.
[[nodiscard]] std::string register_plugin_nodes(NodeRegistry& registry, const PluginHostOptions& options);

} // namespace audioprocess
