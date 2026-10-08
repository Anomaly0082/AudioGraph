#include <audiograph/plugin.h>
#include "node_description.h"
#include <cmath>
#include <cstring>
#include <new>

struct ag_instance { float gain; uint32_t maximum_frames; };
namespace {
template<size_t N> constexpr ag_string str(const char (&s)[N]) { return {s, N - 1}; }
template<size_t N> bool equals(ag_string s, const char (&text)[N]) noexcept {
    return s.data && s.size == N - 1 && std::memcmp(s.data, text, N - 1) == 0;
}
bool valid_sink(const ag_error_sink* s) noexcept {
    return !s || (s->struct_size >= sizeof(*s) && !s->reserved && s->report);
}
ag_status report(const ag_error_sink* sink, const char* message) noexcept {
    if (sink) { try { ag_error e{str("invalid_argument"), {message, std::strlen(message)}, {}, {}}; sink->report(sink->user, &e); } catch (...) {} }
    return AG_INVALID_ARGUMENT;
}
ag_status AG_CALL create(const ag_create_info* info, const ag_error_sink* errors, ag_instance** out) noexcept {
    if (!out) return AG_INVALID_ARGUMENT;
    *out = nullptr;
    try {
        if (!valid_sink(errors)) return AG_INVALID_ARGUMENT;
        if (!info || info->struct_size < sizeof(*info) || info->reserved || info->reserved2 ||
            info->parameter_count > 1 || (info->parameter_count && !info->parameters)) return report(errors, "Invalid create information");
        if (info->environment && (info->environment->struct_size < sizeof(ag_instance_environment) || info->environment->reserved ||
            (!info->environment->resource_root.data && info->environment->resource_root.size) ||
            (!info->environment->data_root.data && info->environment->data_root.size))) return report(errors, "Invalid environment");
        double db = 0;
        if (info->parameter_count) {
            const auto& p = info->parameters[0];
            if (!equals(p.id, "gain_db") || p.value.type != AG_NUMBER || p.value.reserved) return report(errors, "Invalid gain parameter");
            db = p.value.data.number;
        }
        if (!std::isfinite(db) || db < -24 || db > 12) return report(errors, "Gain must be finite and in [-24,12]");
        *out = new ag_instance{static_cast<float>(std::pow(10.0, db / 20.0)), 0};
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
ag_status AG_CALL prepare(ag_instance* instance, const ag_realtime_prepare_info* info, const ag_error_sink* errors) noexcept {
    try {
        if (!valid_sink(errors)) return AG_INVALID_ARGUMENT;
        if (!instance || !info || info->struct_size < sizeof(*info) || info->reserved || info->reserved2 ||
            info->sample_rate != 48000 || info->channel_count != 1 || !info->maximum_frames || info->maximum_frames > 65536)
            return report(errors, "Prepare requires 48 kHz mono and 1..65536 maximum frames");
        if (instance->maximum_frames) return report(errors, "A new session requires a fresh instance");
        instance->maximum_frames = info->maximum_frames;
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
ag_realtime_status AG_CALL process(ag_instance* instance, const ag_realtime_block* block) noexcept {
    if (!instance || !instance->maximum_frames) return AG_RT_NOT_PREPARED;
    if (!block || block->struct_size < sizeof(*block) || block->reserved || !block->samples ||
        !block->frame_count || block->frame_count > instance->maximum_frames || block->sample_count != block->frame_count) return AG_RT_INVALID_BLOCK;
    for (uint64_t i = 0; i < block->sample_count; ++i)
        if (!std::isfinite(block->samples[i])) return AG_RT_NON_FINITE_INPUT;
    for (uint64_t i = 0; i < block->sample_count; ++i) {
        const float value = block->samples[i] * instance->gain;
        if (!std::isfinite(value)) return AG_RT_NON_FINITE_OUTPUT;
        block->samples[i] = value;
    }
    return AG_RT_OK;
}
void AG_CALL destroy(ag_instance* instance) noexcept { delete instance; }
ag_status AG_CALL describe(void* user, ag_receive_description_fn receive) noexcept {
    if (!receive) return AG_INVALID_ARGUMENT;
    try { return receive(user, str(node_description)); } catch (...) { return AG_INTERNAL_ERROR; }
}
ag_status AG_CALL get_node(ag_string type, ag_string cap, uint32_t version, uint32_t size, void* out) noexcept {
    if (!equals(type, "org.audiograph.realtime_example.gain_v1") || !equals(cap, AG_REALTIME_AUDIO_ID) || version != AG_REALTIME_AUDIO_VERSION) return AG_UNSUPPORTED;
    if (!out || size < sizeof(ag_realtime_audio_api)) return AG_INVALID_ARGUMENT;
    const ag_realtime_audio_api api{sizeof(api), AG_REALTIME_AUDIO_VERSION, create, prepare, process, destroy};
    std::memcpy(out, &api, sizeof(api)); return AG_OK;
}
}
extern "C" AG_EXPORT ag_status AG_CALL ag_plugin_get_api(uint32_t major, uint32_t minor, uint32_t size, ag_plugin_api* out) {
    if (major != AG_ABI_MAJOR || minor != AG_ABI_MINOR) return AG_UNSUPPORTED;
    if (!out || size < sizeof(ag_plugin_api)) return AG_INVALID_ARGUMENT;
    const ag_plugin_api api{sizeof(api), AG_ABI_MAJOR, AG_ABI_MINOR, 0, str("org.audiograph.realtime_example"), str("0.1.0"), describe, get_node};
    std::memcpy(out, &api, sizeof(api)); return AG_OK;
}
