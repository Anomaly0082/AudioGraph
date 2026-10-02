#include <audiograph/plugin.h>
#include "example_algorithms.h"
#include "node_description.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstring>
#include <limits>
#include <memory>
#include <new>
#include <span>
#include <stdexcept>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

enum class ExampleKind { Gain, MockAsr };

struct ag_instance {
    ExampleKind kind;
    double parameter;
    std::string resource_root;
    std::string data_root;
    std::atomic_bool used{false};

    ag_instance(ExampleKind type, double value, std::string resources, std::string data)
        : kind(type), parameter(value), resource_root(std::move(resources)), data_root(std::move(data)) {}
};

namespace {
constexpr std::string_view gain_id = "org.audiograph.example.gain_v1";
constexpr std::string_view mock_id = "org.audiograph.example.mock_asr_v1";
constexpr std::size_t chunk_samples = 4096;
constexpr std::uint64_t environment_limit = 16*1024;

ag_string borrowed(std::string_view value) noexcept { return {value.data(), static_cast<std::uint64_t>(value.size())}; }

bool valid_string(ag_string value) noexcept {
    return (value.data != nullptr || value.size == 0)
        && value.size <= static_cast<std::uint64_t>(std::numeric_limits<std::size_t>::max());
}

bool equals(ag_string value, std::string_view expected) noexcept {
    return valid_string(value) && value.size == expected.size()
        && (value.size == 0 || std::memcmp(value.data,expected.data(),expected.size()) == 0);
}

bool utf8_path(ag_string value) noexcept {
    if (!valid_string(value)) return false;
    const auto* data = reinterpret_cast<const unsigned char*>(value.data);
    for (std::uint64_t i = 0; i < value.size;) {
        const unsigned char first = data[i++];
        if (first == 0) return false;
        if (first < 0x80) continue;
        std::uint32_t code = 0;
        std::uint32_t count = 0;
        std::uint32_t minimum = 0;
        if (first >= 0xc2 && first <= 0xdf) { code = first & 0x1f; count = 1; minimum = 0x80; }
        else if (first >= 0xe0 && first <= 0xef) { code = first & 0x0f; count = 2; minimum = 0x800; }
        else if (first >= 0xf0 && first <= 0xf4) { code = first & 0x07; count = 3; minimum = 0x10000; }
        else return false;
        if (count > value.size-i) return false;
        for (std::uint32_t j = 0; j < count; ++j) {
            const unsigned char next = data[i++];
            if ((next & 0xc0) != 0x80) return false;
            code = (code << 6) | (next & 0x3f);
        }
        if (code < minimum || code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) return false;
    }
    return true;
}

bool valid_errors(const ag_error_sink* errors) noexcept {
    return errors == nullptr || (errors->struct_size >= sizeof(ag_error_sink)
        && errors->reserved == 0 && errors->report != nullptr);
}

ag_status fail(ag_status status, const ag_error_sink* errors, std::string_view code,
    std::string_view message, std::string_view port = {}, std::string_view parameter = {}) noexcept {
    // Errors are borrowed during this call only; a broken report callback cannot replace the original failure.
    if (valid_errors(errors) && errors != nullptr) {
        const ag_error error{borrowed(code),borrowed(message),borrowed(port),borrowed(parameter)};
        try { (void)errors->report(errors->user,&error); } catch (...) {}
    }
    return status;
}

ag_status poll(const ag_call_context& context) {
    if (context.is_cancelled(context.user) != 0) return AG_CANCELLED;
    if (context.remaining_ms(context.user) == 0) return AG_DEADLINE_EXCEEDED;
    return AG_OK;
}

ag_status check_interrupt(const ag_call_context& context, const ag_error_sink* errors) {
    const auto status = poll(context);
    if (status == AG_CANCELLED) return fail(status,errors,"cancelled","Plugin execution was cancelled.");
    if (status == AG_DEADLINE_EXCEEDED) return fail(status,errors,"deadline_exceeded","Plugin execution exceeded the host deadline.");
    return status;
}

ag_status create_impl(ExampleKind kind, const ag_create_info* info, const ag_error_sink* errors,
    ag_instance** out_instance) {
    if (out_instance == nullptr) return fail(AG_INVALID_ARGUMENT,errors,"invalid_instance_output","Missing instance output pointer.");
    *out_instance = nullptr;
    if (!valid_errors(errors) || info == nullptr || info->struct_size < sizeof(ag_create_info)
        || info->reserved != 0 || info->reserved2 != 0 || info->environment == nullptr
        || info->environment->struct_size < sizeof(ag_instance_environment) || info->environment->reserved != 0) {
        return fail(AG_INVALID_ARGUMENT,errors,"invalid_create_info","Create info or environment has an invalid size, pointer or reserved field.");
    }
    const auto* environment = info->environment;
    if (environment->resource_root.size > environment_limit || environment->data_root.size > environment_limit) {
        return fail(AG_RESOURCE_LIMIT,errors,"environment_limit","Environment paths exceed the example's 16 KiB-per-path limit.");
    }
    if (!utf8_path(environment->resource_root) || !utf8_path(environment->data_root)) {
        return fail(AG_INVALID_ARGUMENT,errors,"invalid_environment","Environment paths must be UTF-8 without embedded NUL.");
    }
    const auto parameter_id = kind == ExampleKind::Gain ? std::string_view{"gain_db"} : std::string_view{"delay_ms"};
    if (info->parameter_count != 1 || info->parameters == nullptr
        || !equals(info->parameters[0].id,parameter_id)
        || info->parameters[0].value.type != AG_NUMBER || info->parameters[0].value.reserved != 0) {
        return fail(AG_INVALID_ARGUMENT,errors,"invalid_parameter","Exactly one required normalized Number parameter is expected.",{},parameter_id);
    }
    const double parameter = info->parameters[0].value.data.number;
    if (!std::isfinite(parameter) || (kind == ExampleKind::Gain && (parameter < -24 || parameter > 12))
        || (kind == ExampleKind::MockAsr && (parameter < 0 || parameter > 2000 || std::trunc(parameter) != parameter))) {
        return fail(AG_INVALID_ARGUMENT,errors,"invalid_parameter","The normalized Number parameter is outside the descriptor's permitted range.",{},parameter_id);
    }
    const auto copy = [](ag_string text) {
        return text.size == 0 ? std::string{} : std::string{text.data,static_cast<std::size_t>(text.size)};
    };
    auto instance = std::make_unique<ag_instance>(kind,parameter,copy(environment->resource_root),copy(environment->data_root));
    *out_instance = instance.release();
    return AG_OK;
}

template<ExampleKind Kind>
ag_status AG_CALL create_node(const ag_create_info* info, const ag_error_sink* errors,
    ag_instance** out_instance) noexcept {
    if (out_instance != nullptr) *out_instance = nullptr;
    try { return create_impl(Kind,info,errors,out_instance); }
    catch (const std::bad_alloc&) { return fail(AG_RESOURCE_LIMIT,errors,"allocation_limit","Could not allocate the lightweight instance."); }
    catch (...) { return fail(AG_INTERNAL_ERROR,errors,"create_exception","An exception was contained during instance creation."); }
}

ag_status validate_audio(const ag_audio& audio, const ag_call_context& context,
    const ag_error_sink* errors, std::uint64_t& sample_bytes) {
    if (audio.sample_rate == 0 || audio.channel_count == 0
        || audio.frame_count > std::numeric_limits<std::uint64_t>::max()/audio.channel_count
        || audio.sample_count != audio.frame_count*audio.channel_count
        || audio.sample_count > std::numeric_limits<std::uint64_t>::max()/sizeof(float)
        || audio.sample_count > std::numeric_limits<std::size_t>::max()/sizeof(float)
        || (audio.sample_count != 0 && (audio.samples == nullptr
            || reinterpret_cast<std::uintptr_t>(audio.samples)%alignof(float) != 0))) {
        return fail(AG_INVALID_ARGUMENT,errors,"invalid_audio","Audio format, sample pointer or frame/sample arithmetic is invalid.","audio");
    }
    sample_bytes = audio.sample_count*sizeof(float);
    if (context.max_inputs < 1 || context.max_outputs < 1 || sample_bytes > context.max_input_bytes) {
        return fail(AG_RESOURCE_LIMIT,errors,"input_budget","Audio input or port count exceeds the host call budget.","audio");
    }
    return AG_OK;
}

ag_status emit_output(const ag_call_context& context, const ag_output_sink& sink,
    const ag_error_sink* errors, std::string_view port, const ag_value& value) {
    if (const auto status = check_interrupt(context,errors); status != AG_OK) return status;
    const auto emitted = sink.emit(sink.user,borrowed(port),&value);
    if (emitted != AG_OK) return fail(emitted,errors,"output_rejected","The host rejected the staged output.",port);
    // The host must discard staging if cancellation arrives in its copying callback.
    return check_interrupt(context,errors);
}

ag_status run_impl(ExampleKind expected, ag_instance* instance, const ag_named_value* inputs,
    std::uint32_t input_count, const ag_call_context* context, const ag_output_sink* outputs,
    const ag_error_sink* errors) {
    if (!valid_errors(errors) || instance == nullptr || instance->kind != expected) {
        return fail(AG_INVALID_ARGUMENT,errors,"invalid_instance","Instance or error sink is invalid for this node.");
    }
    if (instance->used.exchange(true,std::memory_order_acq_rel)) {
        return fail(AG_INVALID_ARGUMENT,errors,"instance_already_used","Each plugin instance permits exactly one run attempt.");
    }
    if (context == nullptr || context->struct_size < sizeof(ag_call_context) || context->reserved != 0
        || context->is_cancelled == nullptr || context->remaining_ms == nullptr
        || outputs == nullptr || outputs->struct_size < sizeof(ag_output_sink) || outputs->reserved != 0 || outputs->emit == nullptr
        || input_count != 1 || inputs == nullptr || !equals(inputs[0].id,"audio")
        || inputs[0].value.type != AG_AUDIO || inputs[0].value.reserved != 0) {
        return fail(AG_INVALID_ARGUMENT,errors,"invalid_run_info","Run requires one Audio input and valid context/output callbacks.","audio");
    }
    if (const auto status = check_interrupt(*context,errors); status != AG_OK) return status;
    const ag_audio& audio = inputs[0].value.data.audio;
    std::uint64_t sample_bytes = 0;
    if (const auto status = validate_audio(audio,*context,errors,sample_bytes); status != AG_OK) return status;
    const auto count = static_cast<std::size_t>(audio.sample_count);
    if (expected == ExampleKind::Gain) {
        if (sample_bytes > context->max_output_bytes) return fail(AG_RESOURCE_LIMIT,errors,"output_budget","Audio output exceeds the host byte budget.","audio");
        std::vector<float> result(count);
        const double factor = std::pow(10.0,instance->parameter/20.0);
        for (std::size_t offset = 0; offset < count;) {
            if (const auto status = check_interrupt(*context,errors); status != AG_OK) return status;
            const auto length = std::min(chunk_samples,count-offset);
            const std::span<const float> source(audio.samples+offset,length);
            if (!example_algorithms::finite_block(source)) return fail(AG_INVALID_ARGUMENT,errors,"non_finite_audio","Audio samples must be finite.","audio");
            if (!example_algorithms::gain_block(source,factor,std::span<float>{result.data()+offset,length})) {
                return fail(AG_EXECUTION_FAILED,errors,"non_finite_output","Gain would produce non-finite float32 samples.","audio");
            }
            offset += length;
        }
        ag_value output{};
        output.type = AG_AUDIO;
        output.data.audio = {audio.sample_rate,audio.channel_count,audio.frame_count,audio.sample_count,result.data()};
        return emit_output(*context,*outputs,errors,"audio",output);
    }
    for (std::size_t offset = 0; offset < count;) {
        if (const auto status = check_interrupt(*context,errors); status != AG_OK) return status;
        const auto length = std::min(chunk_samples,count-offset);
        if (!example_algorithms::finite_block(std::span<const float>{audio.samples+offset,length})) {
            return fail(AG_INVALID_ARGUMENT,errors,"non_finite_audio","Audio samples must be finite.","audio");
        }
        offset += length;
    }
    const auto end = std::chrono::steady_clock::now()+std::chrono::milliseconds{static_cast<std::int64_t>(instance->parameter)};
    while (std::chrono::steady_clock::now() < end) {
        if (const auto status = check_interrupt(*context,errors); status != AG_OK) return status;
        const auto left = end-std::chrono::steady_clock::now();
        using ClockDuration = std::chrono::steady_clock::duration;
        const auto step = std::min(left,std::chrono::duration_cast<ClockDuration>(std::chrono::milliseconds{10}));
        if (step > ClockDuration::zero()) std::this_thread::sleep_for(step);
    }
    if (const auto status = check_interrupt(*context,errors); status != AG_OK) return status;
    const std::string text = example_algorithms::mock_transcript(audio.frame_count,audio.channel_count,audio.sample_rate);
    if (text.size() > context->max_output_bytes) return fail(AG_RESOURCE_LIMIT,errors,"output_budget","Mock text exceeds the host byte budget.","text");
    ag_value output{};
    output.type = AG_TEXT;
    output.data.string = borrowed(text);
    return emit_output(*context,*outputs,errors,"text",output);
}

template<ExampleKind Kind>
ag_status AG_CALL run_node(ag_instance* instance, const ag_named_value* inputs, std::uint32_t input_count,
    const ag_call_context* context, const ag_output_sink* outputs, const ag_error_sink* errors) noexcept {
    try { return run_impl(Kind,instance,inputs,input_count,context,outputs,errors); }
    catch (const std::bad_alloc&) { return fail(AG_RESOURCE_LIMIT,errors,"allocation_limit","Plugin result allocation exceeded available resources."); }
    catch (const std::length_error&) { return fail(AG_RESOURCE_LIMIT,errors,"allocation_limit","Plugin result exceeds the container size limit."); }
    catch (...) { return fail(AG_INTERNAL_ERROR,errors,"run_exception","An exception was contained during plugin execution."); }
}

void AG_CALL destroy_node(ag_instance* instance) noexcept {
    try { delete instance; } catch (...) {}
}

ag_status AG_CALL describe_nodes(void* user, ag_receive_description_fn receive) noexcept {
    if (receive == nullptr) return AG_INVALID_ARGUMENT;
    try { return receive(user,borrowed(std::string_view{example_nodes_json,sizeof(example_nodes_json)-1})); }
    catch (...) { return AG_INTERNAL_ERROR; }
}

ag_status AG_CALL get_node_api(ag_string type_id, ag_string capability, std::uint32_t version,
    std::uint32_t out_size, void* out_api) noexcept {
    try {
        if (!valid_string(type_id) || !valid_string(capability)) return AG_INVALID_ARGUMENT;
        if (!equals(capability,AG_WHOLE_SYNC_ID) || version != AG_WHOLE_SYNC_VERSION) return AG_UNSUPPORTED;
        if (!equals(type_id,gain_id) && !equals(type_id,mock_id)) return AG_UNSUPPORTED;
        if (out_api == nullptr || out_size < sizeof(ag_whole_sync_api)) return AG_INVALID_ARGUMENT;
        const bool gain = equals(type_id,gain_id);
        const ag_whole_sync_api table{sizeof(ag_whole_sync_api),AG_WHOLE_SYNC_VERSION,
            gain ? create_node<ExampleKind::Gain> : create_node<ExampleKind::MockAsr>,
            gain ? run_node<ExampleKind::Gain> : run_node<ExampleKind::MockAsr>,destroy_node};
        std::memcpy(out_api,&table,sizeof(table));
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
}

extern "C" AG_EXPORT ag_status AG_CALL ag_plugin_get_api(std::uint32_t major, std::uint32_t minor,
    std::uint32_t out_size, ag_plugin_api* out_api) {
    try {
        if (major != AG_ABI_MAJOR || minor != AG_ABI_MINOR) return AG_UNSUPPORTED;
        if (out_api == nullptr || out_size < sizeof(ag_plugin_api)) return AG_INVALID_ARGUMENT;
        const ag_plugin_api table{sizeof(ag_plugin_api),AG_ABI_MAJOR,AG_ABI_MINOR,0,
            borrowed("org.audiograph.example"),borrowed("0.1.0"),describe_nodes,get_node_api};
        std::memcpy(out_api,&table,sizeof(table));
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
