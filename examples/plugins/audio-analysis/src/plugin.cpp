#include <audiograph/plugin.h>
#include "node_description.h"
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstring>
#include <limits>
#include <new>
#include <string_view>

struct ag_instance {
    double threshold;
    std::atomic_bool used{false};
    explicit ag_instance(double db) : threshold(std::pow(10.0, db / 20.0)) {}
};

namespace {
constexpr std::string_view type_id = "org.audiograph.analysis.audio_metrics_v1";
ag_string borrowed(std::string_view s) noexcept { return {s.data(), static_cast<std::uint64_t>(s.size())}; }
template<class T> bool aligned(const T* p) noexcept {
    const auto address = reinterpret_cast<std::uintptr_t>(p);
    return p != nullptr && address % alignof(T) == 0
        && sizeof(T) <= std::numeric_limits<std::uintptr_t>::max() - address;
}
bool valid_string(ag_string s) noexcept {
    return (s.data != nullptr || s.size == 0) && s.size <= std::numeric_limits<std::size_t>::max()
        && s.size <= std::numeric_limits<std::uintptr_t>::max() - reinterpret_cast<std::uintptr_t>(s.data);
}
bool equals(ag_string s, std::string_view expected) noexcept {
    return valid_string(s) && s.size == expected.size()
        && (s.size == 0 || std::memcmp(s.data, expected.data(), expected.size()) == 0);
}
bool valid_errors(const ag_error_sink* e) noexcept {
    return e == nullptr || (aligned(e) && e->struct_size >= sizeof(*e) && e->reserved == 0 && e->report != nullptr);
}
ag_status fail(ag_status status, const ag_error_sink* e, std::string_view code,
    std::string_view message, std::string_view port = {}, std::string_view parameter = {}) noexcept {
    if (e != nullptr && valid_errors(e)) {
        const ag_error error{borrowed(code), borrowed(message), borrowed(port), borrowed(parameter)};
        try { (void)e->report(e->user, &error); } catch (...) {}
    }
    return status;
}
ag_status poll(const ag_call_context& c, const ag_error_sink* e) {
    if (c.is_cancelled(c.user)) return fail(AG_CANCELLED,e,"cancelled","Audio analysis was cancelled.");
    if (c.remaining_ms(c.user) == 0) return fail(AG_DEADLINE_EXCEEDED,e,"deadline_exceeded","Audio analysis exceeded its deadline.");
    return AG_OK;
}
// Environment strings are unused, but still reject malformed UTF-8 and embedded NUL.
bool valid_path(ag_string s) noexcept {
    if (!valid_string(s) || s.size > 16384) return false;
    const auto* p = reinterpret_cast<const unsigned char*>(s.data);
    for (std::uint64_t i = 0; i < s.size;) {
        const auto first = p[i++];
        if (first == 0) return false;
        if (first < 128) continue;
        std::uint32_t code = 0, count = 0, minimum = 0;
        if (first >= 0xc2 && first <= 0xdf) { code = first & 31; count = 1; minimum = 128; }
        else if (first >= 0xe0 && first <= 0xef) { code = first & 15; count = 2; minimum = 2048; }
        else if (first >= 0xf0 && first <= 0xf4) { code = first & 7; count = 3; minimum = 65536; }
        else return false;
        if (count > s.size - i) return false;
        for (std::uint32_t j = 0; j < count; ++j) {
            const auto next = p[i++];
            if ((next & 0xc0) != 0x80) return false;
            code = (code << 6) | (next & 63);
        }
        if (code < minimum || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) return false;
    }
    return true;
}
ag_status AG_CALL create_node(const ag_create_info* info, const ag_error_sink* e, ag_instance** out) noexcept {
    if (!aligned(out)) return fail(AG_INVALID_ARGUMENT,e,"invalid_instance_output","Missing or misaligned instance output pointer.");
    *out = nullptr;
    try {
        if (!valid_errors(e) || !aligned(info) || info->struct_size < sizeof(*info) || info->reserved || info->reserved2
            || !aligned(info->environment) || info->environment->struct_size < sizeof(ag_instance_environment)
            || info->environment->reserved || !valid_path(info->environment->resource_root) || !valid_path(info->environment->data_root))
            return fail(AG_INVALID_ARGUMENT,e,"invalid_create_info","Invalid create info or environment.");
        double db = -60;
        if (info->parameter_count != 0) {
            if (info->parameter_count != 1 || !aligned(info->parameters)
                || !equals(info->parameters[0].id,"silence_threshold_db")
                || info->parameters[0].value.type != AG_NUMBER || info->parameters[0].value.reserved)
                return fail(AG_INVALID_ARGUMENT,e,"invalid_parameter","Expected only silence_threshold_db as Number.",{},"silence_threshold_db");
            db = info->parameters[0].value.data.number;
            if (!std::isfinite(db) || db < -120 || db > 0)
                return fail(AG_INVALID_ARGUMENT,e,"invalid_parameter","Threshold must be finite in [-120, 0].",{},"silence_threshold_db");
        }
        *out = new ag_instance(db);
        return AG_OK;
    } catch (const std::bad_alloc&) { return fail(AG_RESOURCE_LIMIT,e,"allocation_limit","Could not allocate instance."); }
    catch (...) { return fail(AG_INTERNAL_ERROR,e,"create_exception","Exception contained during creation."); }
}
ag_status run_impl(ag_instance* instance, const ag_named_value* inputs, std::uint32_t count,
    const ag_call_context* c, const ag_output_sink* outputs, const ag_error_sink* e) {
    if (!valid_errors(e) || !aligned(instance)) return fail(AG_INVALID_ARGUMENT,e,"invalid_instance","Invalid instance or error sink.");
    if (instance->used.exchange(true,std::memory_order_acq_rel))
        return fail(AG_INVALID_ARGUMENT,e,"instance_already_used","Each instance permits only one run attempt.");
    if (!aligned(c) || c->struct_size < sizeof(*c) || c->reserved || !c->is_cancelled || !c->remaining_ms
        || !aligned(outputs) || outputs->struct_size < sizeof(*outputs) || outputs->reserved || !outputs->emit
        || count != 1 || !aligned(inputs) || !equals(inputs[0].id,"audio")
        || inputs[0].value.type != AG_AUDIO || inputs[0].value.reserved)
        return fail(AG_INVALID_ARGUMENT,e,"invalid_run_info","Expected one Audio input and valid call context and sinks.","audio");
    if (auto status = poll(*c,e); status != AG_OK) return status;
    const auto& a = inputs[0].value.data.audio;
    if (a.sample_rate == 0 || a.channel_count == 0 || a.frame_count == 0
        || a.frame_count > std::numeric_limits<std::uint64_t>::max()/a.channel_count
        || a.sample_count != a.frame_count*a.channel_count
        || a.sample_count > std::numeric_limits<std::uint64_t>::max()/sizeof(float)
        || a.sample_count > std::numeric_limits<std::size_t>::max()/sizeof(float) || !aligned(a.samples)
        || a.sample_count*sizeof(float) > std::numeric_limits<std::uintptr_t>::max()-reinterpret_cast<std::uintptr_t>(a.samples))
        return fail(AG_INVALID_ARGUMENT,e,"invalid_audio","Audio must have nonempty frames, valid format and aligned samples with checked counts.","audio");
    if (c->max_inputs < 1 || c->max_outputs < 4 || a.sample_count*sizeof(float) > c->max_input_bytes || c->max_output_bytes < 32)
        return fail(AG_RESOURCE_LIMIT,e,"call_budget","Audio input or four Number outputs exceed the call budget.");
    const std::uint64_t window_frames = std::max<std::uint64_t>(1,(static_cast<std::uint64_t>(a.sample_rate)+25)/50);
    double sum = 0;
    std::uint64_t clipped = 0, quiet_frames = 0, sample_offset = 0;
    for (std::uint64_t frame_offset = 0; frame_offset < a.frame_count;) {
        const auto frames = std::min(window_frames,a.frame_count-frame_offset);
        const auto samples = frames*a.channel_count; // Bounded by validated total sample count.
        double window_sum = 0;
        for (std::uint64_t local = 0; local < samples; ++local) {
            if ((sample_offset & 4095) == 0) { if (auto status = poll(*c,e); status != AG_OK) return status; }
            const double x = a.samples[static_cast<std::size_t>(sample_offset++)];
            if (!std::isfinite(x)) return fail(AG_INVALID_ARGUMENT,e,"non_finite_audio","Audio samples must be finite.","audio");
            const double square = x*x;
            sum += square;
            window_sum += square;
            if (x < -1.0 || x > 32767.0/32768.0) ++clipped;
        }
        if (std::sqrt(window_sum/static_cast<double>(samples)) <= instance->threshold) quiet_frames += frames;
        frame_offset += frames;
    }
    const double rms = std::sqrt(sum/static_cast<double>(a.sample_count));
    const double values[]{rms, rms == 0 ? -120.0 : std::max(-120.0,20.0*std::log10(rms)),
        static_cast<double>(clipped)/static_cast<double>(a.sample_count),
        static_cast<double>(quiet_frames)/static_cast<double>(a.frame_count)};
    constexpr std::string_view ports[]{"rms","rms_dbfs","clipping_ratio","silence_ratio"};
    for (std::size_t i = 0; i < 4; ++i) {
        if (!std::isfinite(values[i])) return fail(AG_EXECUTION_FAILED,e,"non_finite_output","Analysis produced a non-finite metric.");
    }
    for (std::size_t i = 0; i < 4; ++i) {
        if (auto status = poll(*c,e); status != AG_OK) return status;
        ag_value output{};
        output.type = AG_NUMBER;
        output.data.number = values[i];
        const auto status = outputs->emit(outputs->user,borrowed(ports[i]),&output);
        if (status != AG_OK) return fail(status,e,"output_rejected","Host rejected staged metric.",ports[i]);
        if (auto interrupted = poll(*c,e); interrupted != AG_OK) return interrupted;
    }
    return AG_OK;
}
ag_status AG_CALL run_node(ag_instance* instance, const ag_named_value* inputs, std::uint32_t count,
    const ag_call_context* c, const ag_output_sink* outputs, const ag_error_sink* e) noexcept {
    try { return run_impl(instance,inputs,count,c,outputs,e); }
    catch (const std::bad_alloc&) { return fail(AG_RESOURCE_LIMIT,e,"allocation_limit","Allocation failed during analysis."); }
    catch (...) { return fail(AG_INTERNAL_ERROR,e,"run_exception","Exception contained during analysis."); }
}
void AG_CALL destroy_node(ag_instance* instance) noexcept { try { delete instance; } catch (...) {} }
ag_status AG_CALL describe_nodes(void* user, ag_receive_description_fn receive) noexcept {
    if (!receive) return AG_INVALID_ARGUMENT;
    try { return receive(user,borrowed({node_description,sizeof(node_description)-1})); }
    catch (...) { return AG_INTERNAL_ERROR; }
}
ag_status AG_CALL get_node_api(ag_string type, ag_string capability, std::uint32_t version, std::uint32_t size, void* out) noexcept {
    try {
        if (!valid_string(type) || !valid_string(capability)) return AG_INVALID_ARGUMENT;
        if (!equals(type,type_id) || !equals(capability,AG_WHOLE_SYNC_ID) || version != AG_WHOLE_SYNC_VERSION) return AG_UNSUPPORTED;
        if (!out || size < sizeof(ag_whole_sync_api)) return AG_INVALID_ARGUMENT;
        const ag_whole_sync_api api{sizeof(ag_whole_sync_api),AG_WHOLE_SYNC_VERSION,create_node,run_node,destroy_node};
        std::memcpy(out,&api,sizeof(api));
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
}
extern "C" AG_EXPORT ag_status AG_CALL ag_plugin_get_api(std::uint32_t major, std::uint32_t minor, std::uint32_t size, ag_plugin_api* out) {
    try {
        if (major != AG_ABI_MAJOR || minor != AG_ABI_MINOR) return AG_UNSUPPORTED;
        if (!out || size < sizeof(ag_plugin_api)) return AG_INVALID_ARGUMENT;
        const ag_plugin_api api{sizeof(ag_plugin_api),AG_ABI_MAJOR,AG_ABI_MINOR,0,borrowed("org.audiograph.analysis"),borrowed("0.1.0"),describe_nodes,get_node_api};
        std::memcpy(out,&api,sizeof(api));
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
