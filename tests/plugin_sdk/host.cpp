#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <audiograph/plugin.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <initializer_list>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

extern "C" int ag_c11_gain_smoke(ag_get_api_fn entry, const char* resource_root,
    uint64_t resource_size, const char* data_root, uint64_t data_size);

namespace {
using namespace std::chrono_literals;
namespace fs = std::filesystem;
constexpr char gain_type[] = "org.audiograph.example.gain_v1";
constexpr char asr_type[] = "org.audiograph.example.mock_asr_v1";

void check(bool condition, const char* message) {
    if (!condition) throw std::runtime_error(message);
}
ag_string view(const char* text) { return {text, static_cast<uint64_t>(std::strlen(text))}; }
bool equal(ag_string value, const char* expected) {
    return value.data && value.size == std::strlen(expected) &&
        std::memcmp(value.data,expected,static_cast<size_t>(value.size)) == 0;
}
bool valid_utf8(ag_string value, uint64_t limit) {
    if (value.size > limit || (value.size && !value.data)) return false;
    const auto* bytes = reinterpret_cast<const unsigned char*>(value.data);
    for (uint64_t i=0; i<value.size;) {
        const auto first = bytes[i++];
        if (first < 0x80) continue; // Includes embedded NUL: length, not terminator, owns semantics.
        uint64_t following{};
        unsigned char low = 0x80, high = 0xBF;
        if (first >= 0xC2 && first <= 0xDF) following = 1;
        else if (first >= 0xE0 && first <= 0xEF) {
            following = 2;
            if (first == 0xE0) low = 0xA0;
            if (first == 0xED) high = 0x9F;
        } else if (first >= 0xF0 && first <= 0xF4) {
            following = 3;
            if (first == 0xF0) low = 0x90;
            if (first == 0xF4) high = 0x8F;
        } else return false;
        if (following > value.size-i || bytes[i] < low || bytes[i] > high) return false;
        ++i;
        for (uint64_t part=1;part<following;++part,++i)
            if (bytes[i] < 0x80 || bytes[i] > 0xBF) return false;
    }
    return true;
}
std::string copy(ag_string value) {
    if (!value.data && value.size) throw std::runtime_error("Invalid borrowed string");
    if (value.size > 4096) throw std::runtime_error("Oversized borrowed string");
    return value.size ? std::string(value.data,static_cast<size_t>(value.size)) : std::string();
}
std::string utf8(const fs::path& path) {
    const auto bytes = path.u8string();
    return {reinterpret_cast<const char*>(bytes.data()),bytes.size()};
}

struct Module {
    HMODULE handle{};
    explicit Module(const fs::path& dll) {
        check(dll.is_absolute() && fs::is_regular_file(dll),"Fixture DLL must be an existing absolute path");
        handle = LoadLibraryExW(dll.c_str(),nullptr,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        check(handle != nullptr,"LoadLibraryExW failed for absolute fixture DLL");
    }
    ~Module() { if (handle) FreeLibrary(handle); }
    Module(const Module&) = delete;
    Module& operator=(const Module&) = delete;
    ag_get_api_fn entry() const {
        auto address = GetProcAddress(handle,"ag_plugin_get_api");
        check(address != nullptr,"Fixture DLL does not export ag_plugin_get_api");
        return reinterpret_cast<ag_get_api_fn>(address);
    }
};

struct TemporaryDirectory {
    fs::path path;
    TemporaryDirectory() {
        const auto stamp = std::chrono::steady_clock::now().time_since_epoch().count();
        path = fs::temp_directory_path() / ("ag-plugin-sdk-host-" + std::to_string(GetCurrentProcessId()) + "-" + std::to_string(stamp));
        check(fs::create_directory(path),"Cannot create isolated host data directory");
    }
    ~TemporaryDirectory() { std::error_code error; fs::remove_all(path,error); }
};

struct ErrorLog { unsigned calls{}; bool after_return{}; unsigned late{}; std::string code; };
extern "C" ag_status AG_CALL report_error(void* user, const ag_error* error) {
    try {
        auto& log = *static_cast<ErrorLog*>(user);
        if (log.after_return) { ++log.late; return AG_INVALID_ARGUMENT; }
        if (!error) return AG_INVALID_ARGUMENT;
        uint64_t remaining = 4096;
        for (auto field : {error->code,error->message,error->port_id,error->parameter_id}) {
            if (field.size > remaining) return AG_RESOURCE_LIMIT;
            if (!valid_utf8(field,remaining)) return AG_INVALID_ARGUMENT;
            remaining -= field.size;
        }
        log.code = copy(error->code);
        (void)copy(error->message); (void)copy(error->port_id); (void)copy(error->parameter_id);
        ++log.calls;
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}

struct HostContext {
    std::atomic<bool> cancelled{false};
    std::chrono::steady_clock::time_point start{std::chrono::steady_clock::now()};
    uint64_t cancel_after_ms{AG_NO_DEADLINE};
    uint64_t deadline_ms{AG_NO_DEADLINE};
    uint64_t elapsed_ms() const {
        return static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::steady_clock::now()-start).count());
    }
};
extern "C" uint32_t AG_CALL is_cancelled(void* user) {
    try {
        auto& context = *static_cast<HostContext*>(user);
        return context.cancelled.load() || context.elapsed_ms() >= context.cancel_after_ms ? 1u : 0u;
    } catch (...) { return 1u; }
}
extern "C" uint64_t AG_CALL remaining_ms(void* user) {
    try {
        const auto& context = *static_cast<HostContext*>(user);
        if (context.deadline_ms == AG_NO_DEADLINE) return AG_NO_DEADLINE;
        const auto elapsed = context.elapsed_ms();
        return elapsed < context.deadline_ms ? context.deadline_ms-elapsed : 0u;
    } catch (...) { return 0u; }
}

struct Staging {
    uint32_t expected_type{};
    uint64_t max_bytes{4096};
    uint32_t max_outputs{1};
    bool reject{};
    HostContext* cancel_on_emit{};
    bool returned{};
    bool callback_failure{};
    unsigned calls{};
    unsigned late{};
    bool has_value{};
    std::vector<float> audio;
    std::string text;
};

extern "C" ag_status AG_CALL stage_output(void* user, ag_string port, const ag_value* value) {
    try {
        auto& stage = *static_cast<Staging*>(user);
        if (stage.returned) { ++stage.late; return AG_INVALID_ARGUMENT; }
        ++stage.calls;
        if (stage.reject) return AG_RESOURCE_LIMIT;
        if (!value || !port.data || port.size > 64 || stage.has_value || stage.calls > stage.max_outputs ||
            value->reserved != 0u || value->type != stage.expected_type) return AG_INVALID_ARGUMENT;
        if (stage.expected_type == AG_AUDIO) {
            if (!equal(port,"audio")) return AG_INVALID_ARGUMENT;
            const auto& audio = value->data.audio;
            if (!audio.sample_rate || !audio.channel_count ||
                audio.frame_count > UINT64_MAX/audio.channel_count ||
                audio.sample_count != audio.frame_count*audio.channel_count ||
                audio.sample_count > stage.max_bytes/sizeof(float) ||
                (audio.sample_count && !audio.samples)) return AG_RESOURCE_LIMIT;
            std::vector<float> copied;
            copied.reserve(static_cast<size_t>(audio.sample_count));
            for (uint64_t i=0; i<audio.sample_count; ++i) {
                if (!std::isfinite(audio.samples[i])) return AG_INVALID_ARGUMENT;
                copied.push_back(audio.samples[i]);
            }
            stage.audio = std::move(copied);
        } else if (stage.expected_type == AG_TEXT) {
            if (!equal(port,"text")) return AG_INVALID_ARGUMENT;
            const auto& string = value->data.string;
            if (string.size > stage.max_bytes) return AG_RESOURCE_LIMIT;
            if (!valid_utf8(string,stage.max_bytes)) return AG_INVALID_ARGUMENT;
            stage.text.assign(string.data ? string.data : "",static_cast<size_t>(string.size));
        } else return AG_INVALID_ARGUMENT;
        stage.has_value = true;
        if (stage.cancel_on_emit) stage.cancel_on_emit->cancelled.store(true);
        return AG_OK;
    } catch (...) {
        if (user) static_cast<Staging*>(user)->callback_failure = true;
        return AG_INTERNAL_ERROR;
    }
}

struct Instance {
    ag_whole_sync_api api{};
    ag_instance* handle{};
    Instance(ag_whole_sync_api selected, ag_instance* value):api(selected),handle(value) {}
    ~Instance() { if (handle) api.destroy(handle); }
    Instance(const Instance&) = delete;
    Instance& operator=(const Instance&) = delete;
};

ag_instance_environment environment(const std::string& resource, const std::string& data) {
    ag_instance_environment result{};
    result.struct_size = sizeof(result);
    result.resource_root = {resource.data(),resource.size()};
    result.data_root = {data.data(),data.size()};
    return result;
}
ag_named_value number_parameter(const char* id, double number) {
    ag_named_value result{};
    result.id = view(id);
    result.value.type = AG_NUMBER;
    result.value.data.number = number;
    return result;
}
ag_create_info create_info(const ag_named_value* parameters, uint32_t count, const ag_instance_environment* env) {
    ag_create_info result{};
    result.struct_size = sizeof(result);
    result.parameters = parameters; result.parameter_count = count; result.environment = env;
    return result;
}
ag_call_context call_context(HostContext& host, uint64_t input_bytes, uint64_t output_bytes) {
    ag_call_context result{};
    result.struct_size = sizeof(result);
    result.user = &host; result.is_cancelled = is_cancelled; result.remaining_ms = remaining_ms;
    result.max_input_bytes = input_bytes; result.max_output_bytes = output_bytes;
    result.max_inputs = 1; result.max_outputs = 1;
    return result;
}

struct RunResult {
    ag_status status{};
    bool published{};
    std::vector<float> audio;
    std::string text;
    unsigned emits{};
    unsigned late{};
    bool callback_failure{};
};

RunResult run(ag_whole_sync_api api, ag_instance* instance, const std::vector<float>& source,
    uint32_t expected_type, HostContext& host, uint64_t input_budget = 4096,
    uint64_t output_budget = 4096, bool reject = false, bool cancel_on_emit = false) {
    check(source.size() <= UINT64_MAX/sizeof(float),"Host input product overflow");
    check(input_budget >= source.size()*sizeof(float),"Host refused oversized input before plugin call");
    check(std::all_of(source.begin(),source.end(),[](float sample){return std::isfinite(sample);}),
        "Host refused non-finite input before plugin call");
    ag_named_value input{};
    input.id = view("audio"); input.value.type = AG_AUDIO;
    input.value.data.audio = {44100u,1u,source.size(),source.size(),source.data()};
    auto context = call_context(host,input_budget,output_budget);
    Staging stage{}; stage.expected_type = expected_type; stage.max_bytes = output_budget;
    stage.reject = reject; stage.cancel_on_emit = cancel_on_emit ? &host : nullptr;
    ag_output_sink output{}; output.struct_size = sizeof(output); output.user = &stage; output.emit = stage_output;
    ErrorLog errors{};
    ag_error_sink error_sink{}; error_sink.struct_size = sizeof(error_sink);
    error_sink.user = &errors; error_sink.report = report_error;
    const auto status = api.run(instance,&input,1u,&context,&output,&error_sink);
    stage.returned = true; errors.after_return = true;
    RunResult result{};
    result.status = status; result.emits = stage.calls; result.callback_failure = stage.callback_failure;
    // This is the test host's publication gate: borrowed data lives in staging
    // until the plugin has returned and final cooperative checks have passed.
    result.published = status == AG_OK && !is_cancelled(&host) && remaining_ms(&host) != 0 &&
        !stage.callback_failure && stage.has_value && stage.calls == 1u;
    if (result.published) { result.audio = std::move(stage.audio); result.text = std::move(stage.text); }
    std::this_thread::sleep_for(30ms);
    result.late = stage.late + errors.late;
    return result;
}

void test_callback_validation() {
    float samples[] = {0.25f,-0.5f};
    ag_value value{}; value.type = AG_AUDIO;
    value.data.audio = {44100u,1u,2u,2u,samples};
    Staging stage{}; stage.expected_type = AG_AUDIO; stage.max_bytes = 8;
    check(stage_output(&stage,view("audio"),&value) == AG_OK,"Host staging did not accept valid audio");
    samples[0] = 0.9f;
    check(stage.audio[0] == 0.25f,"Host staging retained borrowed audio memory");
    check(stage_output(&stage,view("audio"),&value) == AG_INVALID_ARGUMENT,"Host accepted duplicate port");
    stage = {}; stage.expected_type = AG_AUDIO; stage.max_bytes = 7;
    check(stage_output(&stage,view("audio"),&value) == AG_RESOURCE_LIMIT,"Host accepted output over byte budget");
    stage = {}; stage.expected_type = AG_AUDIO; stage.max_bytes = 8;
    value.reserved = 1;
    check(stage_output(&stage,view("audio"),&value) == AG_INVALID_ARGUMENT,"Host accepted nonzero reserved field");
    value.reserved = 0; value.type = AG_NUMBER;
    check(stage_output(&stage,view("audio"),&value) == AG_INVALID_ARGUMENT,"Host accepted wrong output tag");
    value.type = AG_AUDIO; value.data.audio.samples = samples;
    samples[1] = std::numeric_limits<float>::infinity();
    check(stage_output(&stage,view("audio"),&value) == AG_INVALID_ARGUMENT,"Host accepted non-finite audio");
    std::string message = "[MOCK ASR] no speech recognition performed.";
    ag_value text{}; text.type = AG_TEXT; text.data.string = {message.data(),message.size()};
    stage = {}; stage.expected_type = AG_TEXT; stage.max_bytes = 4096;
    check(stage_output(&stage,view("text"),&text) == AG_OK,"Host staging did not accept text");
    message[0] = 'X';
    check(stage.text.front() == '[',"Host staging retained borrowed text memory");
    const char invalid_utf8[] = {static_cast<char>(0xC0),static_cast<char>(0xAF)};
    text.data.string = {invalid_utf8,sizeof(invalid_utf8)};
    stage = {}; stage.expected_type = AG_TEXT; stage.max_bytes = 4096;
    check(stage_output(&stage,view("text"),&text) == AG_INVALID_ARGUMENT && !stage.has_value,
        "Host accepted malformed UTF-8 Text");
    const char with_nul[] = {'o','k','\0','x'};
    text.data.string = {with_nul,sizeof(with_nul)};
    stage = {}; stage.expected_type = AG_TEXT; stage.max_bytes = 4096;
    check(stage_output(&stage,view("text"),&text) == AG_OK && stage.text.size() == 4 && stage.text[2] == '\0',
        "Host treated a valid embedded NUL as a terminator");
    ErrorLog error_log{};
    ag_error malformed_error{};
    malformed_error.code = {invalid_utf8,sizeof(invalid_utf8)};
    check(report_error(&error_log,&malformed_error) == AG_INVALID_ARGUMENT && error_log.calls == 0,
        "Host accepted malformed UTF-8 error data");
    const char valid_error[] = {'e','\0','r'};
    malformed_error.code = {valid_error,sizeof(valid_error)};
    check(report_error(&error_log,&malformed_error) == AG_OK && error_log.calls == 1 &&
        error_log.code.size() == 3 && error_log.code[1] == '\0',
        "Host rejected a length-delimited valid error containing NUL");
}

void test_tables(ag_get_api_fn entry, ag_plugin_api& plugin, ag_whole_sync_api& gain, ag_whole_sync_api& asr) {
    alignas(ag_plugin_api) std::array<unsigned char,sizeof(ag_plugin_api)+16> table{};
    table.fill(0xA5);
    check(entry(AG_ABI_MAJOR+1,AG_ABI_MINOR,sizeof(ag_plugin_api),reinterpret_cast<ag_plugin_api*>(table.data())) == AG_UNSUPPORTED,
        "Major ABI mismatch was accepted");
    check(std::all_of(table.begin(),table.end(),[](auto byte){return byte == 0xA5;}),"Failed ABI query wrote output bytes");
    check(entry(AG_ABI_MAJOR,AG_ABI_MINOR+1,sizeof(ag_plugin_api),reinterpret_cast<ag_plugin_api*>(table.data())) == AG_UNSUPPORTED,
        "Minor ABI mismatch was accepted");
    check(entry(AG_ABI_MAJOR,AG_ABI_MINOR,sizeof(ag_plugin_api)-1,reinterpret_cast<ag_plugin_api*>(table.data())) == AG_INVALID_ARGUMENT,
        "Undersized plugin table was accepted");
    check(std::all_of(table.begin(),table.end(),[](auto byte){return byte == 0xA5;}),"Rejected plugin table query wrote bytes");
    check(entry(AG_ABI_MAJOR,AG_ABI_MINOR,sizeof(ag_plugin_api),reinterpret_cast<ag_plugin_api*>(table.data())) == AG_OK,
        "Exact plugin ABI query failed");
    check(std::all_of(table.begin()+sizeof(ag_plugin_api),table.end(),[](auto byte){return byte == 0xA5;}),
        "Plugin table query touched trailing guard bytes");
    std::memcpy(&plugin,table.data(),sizeof(plugin));
    check(plugin.struct_size == sizeof(plugin) && plugin.abi_major == AG_ABI_MAJOR &&
        plugin.abi_minor == AG_ABI_MINOR && plugin.reserved == 0 &&
        equal(plugin.plugin_id,"org.audiograph.example") && plugin.describe_nodes && plugin.get_node_api,
        "Plugin table contains invalid metadata or functions");

    alignas(ag_whole_sync_api) std::array<unsigned char,sizeof(ag_whole_sync_api)+16> selected{};
    auto query = [&](const char* type, const char* capability, uint32_t version, uint32_t size) {
        selected.fill(0xA5);
        return plugin.get_node_api(view(type),view(capability),version,size,selected.data());
    };
    check(query("org.audiograph.example.unknown",AG_WHOLE_SYNC_ID,1,sizeof(gain)) == AG_UNSUPPORTED,
        "Unknown node type was accepted");
    check(std::all_of(selected.begin(),selected.end(),[](auto byte){return byte == 0xA5;}),"Unknown node query wrote bytes");
    check(query(gain_type,"ag.realtime/1",1,sizeof(gain)) == AG_UNSUPPORTED,"Unknown capability was accepted");
    check(query(gain_type,AG_WHOLE_SYNC_ID,2,sizeof(gain)) == AG_UNSUPPORTED,"Unknown capability version was accepted");
    check(query(gain_type,AG_WHOLE_SYNC_ID,1,sizeof(gain)-1) == AG_INVALID_ARGUMENT,"Small node table was accepted");
    check(std::all_of(selected.begin(),selected.end(),[](auto byte){return byte == 0xA5;}),"Rejected node query wrote bytes");
    check(query(gain_type,AG_WHOLE_SYNC_ID,1,sizeof(gain)) == AG_OK,"Gain capability selection failed");
    check(std::all_of(selected.begin()+sizeof(gain),selected.end(),[](auto byte){return byte == 0xA5;}),
        "Node API touched trailing guard bytes");
    std::memcpy(&gain,selected.data(),sizeof(gain));
    check(gain.struct_size == sizeof(gain) && gain.version == 1 && gain.create && gain.run && gain.destroy,
        "Gain function table invalid");
    check(query(asr_type,AG_WHOLE_SYNC_ID,1,sizeof(asr)) == AG_OK,"Mock ASR capability selection failed");
    std::memcpy(&asr,selected.data(),sizeof(asr));
    check(asr.struct_size == sizeof(asr) && asr.version == 1 && asr.create && asr.run && asr.destroy,
        "Mock ASR function table invalid");
}

struct Description { std::string json; unsigned calls{}; };
extern "C" ag_status AG_CALL receive_description(void* user, ag_string json) {
    try {
        auto& description = *static_cast<Description*>(user);
        if (description.calls++ != 0 || json.size > 64*1024 || (json.size && !json.data)) return AG_INVALID_ARGUMENT;
        description.json.assign(json.data ? json.data : "",static_cast<size_t>(json.size));
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
void test_description(const ag_plugin_api& plugin, const fs::path& dll) {
    Description description{};
    check(plugin.describe_nodes(&description,receive_description) == AG_OK,"describe_nodes failed");
    check(description.calls == 1 && description.json.find(gain_type) != std::string::npos &&
        description.json.find(asr_type) != std::string::npos &&
        description.json.find(AG_WHOLE_SYNC_ID) != std::string::npos,
        "Description omitted fixture nodes or capability");
    std::ifstream manifest(dll.parent_path() / "nodes.json",std::ios::binary);
    check(static_cast<bool>(manifest),"Package nodes.json is missing");
    const std::string static_json((std::istreambuf_iterator<char>(manifest)),std::istreambuf_iterator<char>());
    // The sample embeds nodes.json verbatim. Any BOM is therefore part of both
    // byte streams; no normalization can hide a static/runtime mismatch.
    check(description.json == static_json,"describe_nodes differs byte-for-byte from packaged nodes.json");
}

void expect_create_failure(ag_whole_sync_api api, ag_create_info info, ag_status expected) {
    ag_instance* handle = reinterpret_cast<ag_instance*>(static_cast<uintptr_t>(1));
    ErrorLog errors{}; ag_error_sink sink{};
    sink.struct_size = sizeof(sink); sink.user = &errors; sink.report = report_error;
    check(api.create(&info,&sink,&handle) == expected,"Create returned an unexpected status");
    check(handle == nullptr,"Failed create did not null its output handle");
    errors.after_return = true;
    std::this_thread::sleep_for(10ms);
    check(errors.late == 0,"Error callback occurred after create returned");
}

void test_create_boundaries(ag_whole_sync_api gain, ag_whole_sync_api asr,
    const ag_instance_environment& env) {
    auto parameter = number_parameter("gain_db",0);
    auto info = create_info(&parameter,1,&env);
    expect_create_failure(gain,create_info(nullptr,0,&env),AG_INVALID_ARGUMENT);
    parameter.value.data.number = 12.1; expect_create_failure(gain,info,AG_INVALID_ARGUMENT);
    parameter.value.data.number = -24.1; expect_create_failure(gain,info,AG_INVALID_ARGUMENT);
    parameter.value.data.number = std::numeric_limits<double>::quiet_NaN();
    expect_create_failure(gain,info,AG_INVALID_ARGUMENT);
    parameter.value.type = AG_TEXT; expect_create_failure(gain,info,AG_INVALID_ARGUMENT);
    parameter = number_parameter("unknown",0); expect_create_failure(gain,info,AG_INVALID_ARGUMENT);
    parameter = number_parameter("gain_db",0);
    std::array<ag_named_value,2> duplicate{parameter,parameter};
    expect_create_failure(gain,create_info(duplicate.data(),2,&env),AG_INVALID_ARGUMENT);
    info = create_info(&parameter,1,&env); info.reserved = 1;
    expect_create_failure(gain,info,AG_INVALID_ARGUMENT);
    auto delay = number_parameter("delay_ms",2001);
    auto asr_info = create_info(&delay,1,&env);
    expect_create_failure(asr,asr_info,AG_INVALID_ARGUMENT);
    delay.value.data.number = 1.5; expect_create_failure(asr,asr_info,AG_INVALID_ARGUMENT);
    delay.value.data.number = -1; expect_create_failure(asr,asr_info,AG_INVALID_ARGUMENT);
    ag_instance* null_handle = reinterpret_cast<ag_instance*>(static_cast<uintptr_t>(1));
    check(gain.create(nullptr,nullptr,&null_handle) == AG_INVALID_ARGUMENT && null_handle == nullptr,
        "Null create info did not fail and clear handle");
}

Instance create(ag_whole_sync_api api, const char* id, double value,
    const ag_instance_environment& env) {
    auto parameter = number_parameter(id,value);
    auto info = create_info(&parameter,1,&env);
    ag_instance* handle{};
    check(api.create(&info,nullptr,&handle) == AG_OK && handle,"Valid node create failed");
    return Instance{api,handle};
}

void test_execution(ag_whole_sync_api gain, ag_whole_sync_api asr,
    const ag_instance_environment& env) {
    const std::vector<float> source{0.25f,-0.5f,0.75f,-1.0f};
    auto half = create(gain,"gain_db",-6.020599913279624,env);
    auto plus = create(gain,"gain_db",6.020599913279624,env);
    HostContext first_context{}, second_context{};
    const auto first = run(gain,half.handle,source,AG_AUDIO,first_context);
    const auto second = run(gain,plus.handle,source,AG_AUDIO,second_context);
    check(first.status == AG_OK && first.published && first.emits == 1 && first.late == 0,
        "Gain run did not publish exactly one complete output");
    check(second.status == AG_OK && second.published && second.late == 0,
        "Independent gain instance failed");
    check(first.audio.size() == source.size() && second.audio.size() == source.size(),"Gain output size wrong");
    for (size_t i=0;i<source.size();++i) {
        check(std::fabs(first.audio[i]-source[i]*0.5f) < 0.0005f,"-6.0206 dB did not halve audio");
        check(std::fabs(second.audio[i]-source[i]*2.0f) < 0.001f,"Second instance contaminated first gain");
    }
    check(source[0] == 0.25f && source[3] == -1.0f,"Plugin modified borrowed source samples");
    HostContext reuse_context{};
    const auto reused = run(gain,half.handle,source,AG_AUDIO,reuse_context);
    check(reused.status == AG_INVALID_ARGUMENT && !reused.published && reused.emits == 0,
        "Instance allowed a second run");
    HostContext null_context{};
    const auto null_run = run(gain,nullptr,source,AG_AUDIO,null_context);
    check(null_run.status == AG_INVALID_ARGUMENT && !null_run.published,"Null instance was accepted");

    auto mock = create(asr,"delay_ms",0,env);
    HostContext mock_context{};
    const auto transcript = run(asr,mock.handle,source,AG_TEXT,mock_context);
    check(transcript.status == AG_OK && transcript.published && transcript.emits == 1 && transcript.late == 0,
        "Mock ASR did not publish one text output");
    check(transcript.text.find("[MOCK ASR]") != std::string::npos &&
        transcript.text.find("no speech recognition performed") != std::string::npos,
        "Mock ASR was not clearly marked as simulated text");

    auto pre_cancel = create(asr,"delay_ms",100,env);
    HostContext cancelled{}; cancelled.cancelled = true;
    const auto cancelled_result = run(asr,pre_cancel.handle,source,AG_TEXT,cancelled);
    check(cancelled_result.status == AG_CANCELLED && !cancelled_result.published && cancelled_result.late == 0,
        "Pre-cancelled run published data");
    auto during_cancel = create(asr,"delay_ms",120,env);
    HostContext stopping{}; stopping.cancel_after_ms = 15;
    const auto stopped = run(asr,during_cancel.handle,source,AG_TEXT,stopping);
    check(stopped.status == AG_CANCELLED && !stopped.published && stopped.late == 0,
        "Cooperative mid-run cancellation failed");
    auto during_timeout = create(asr,"delay_ms",120,env);
    HostContext expiring{}; expiring.deadline_ms = 15;
    const auto expired = run(asr,during_timeout.handle,source,AG_TEXT,expiring);
    check(expired.status == AG_DEADLINE_EXCEEDED && !expired.published && expired.late == 0,
        "Cooperative deadline failed");
    auto rejected = create(gain,"gain_db",0,env);
    HostContext reject_context{};
    const auto denied = run(gain,rejected.handle,source,AG_AUDIO,reject_context,4096,4096,true);
    check(denied.status == AG_RESOURCE_LIMIT && !denied.published && denied.emits == 1 && denied.late == 0,
        "Rejected output writer was treated as success or published data");
    auto output_limited = create(gain,"gain_db",0,env);
    HostContext limit_context{};
    const auto limit = run(gain,output_limited.handle,source,AG_AUDIO,limit_context,4096,1);
    check(limit.status == AG_RESOURCE_LIMIT && !limit.published && limit.late == 0,
        "Output byte budget was ignored");
    auto cancelled_in_emit = create(gain,"gain_db",0,env);
    HostContext emit_context{};
    const auto staged_then_cancelled = run(gain,cancelled_in_emit.handle,source,AG_AUDIO,
        emit_context,4096,4096,false,true);
    check(staged_then_cancelled.status == AG_CANCELLED && staged_then_cancelled.emits == 1 &&
        !staged_then_cancelled.published && staged_then_cancelled.late == 0,
        "Post-emit cancellation published staged data or returned success");
}

void test_direct_invalid_run(ag_whole_sync_api gain, const ag_instance_environment& env) {
    for (int scenario=0; scenario<3; ++scenario) {
        auto instance = create(gain,"gain_db",0,env);
        std::array<float,4> source{0.25f,-0.5f,0.75f,-1.0f};
        HostContext host{};
        auto context = call_context(host,4096,4096);
        ag_named_value input{};
        input.id = view("audio"); input.value.type = AG_AUDIO;
        input.value.data.audio = {44100u,1u,4u,4u,source.data()};
        if (scenario == 0) {
            input.value.data.audio.channel_count = 2u;
            input.value.data.audio.frame_count = UINT64_MAX;
        } else if (scenario == 1) {
            source[1] = std::numeric_limits<float>::quiet_NaN();
        } else {
            context.struct_size = sizeof(context)-1u;
        }
        Staging staged{}; staged.expected_type = AG_AUDIO;
        ag_output_sink sink{}; sink.struct_size = sizeof(sink); sink.user = &staged; sink.emit = stage_output;
        const auto status = gain.run(instance.handle,&input,1u,&context,&sink,nullptr);
        staged.returned = true;
        check(status == AG_INVALID_ARGUMENT && staged.calls == 0 && !staged.has_value,
            "Malformed direct plugin run emitted output or returned the wrong status");
    }
}
} // namespace

int main(int argc, char** argv) {
    try {
        check(argc == 2,"Usage: ag_plugin_sdk_host ABSOLUTE_FIXTURE_DLL");
        const auto dll = fs::absolute(fs::u8path(argv[1]));
        check(fs::u8path(argv[1]).is_absolute(),"DLL argument must already be absolute");
        Module module(dll);
        TemporaryDirectory data;
        const auto resource_utf8 = utf8(dll.parent_path());
        const auto data_utf8 = utf8(data.path);
        auto entry = module.entry();
        ag_plugin_api plugin{}; ag_whole_sync_api gain{},asr{};
        test_tables(entry,plugin,gain,asr);
        test_description(plugin,dll);
        test_callback_validation();
        auto env = environment(resource_utf8,data_utf8);
        test_create_boundaries(gain,asr,env);
        const int c_result = ag_c11_gain_smoke(entry,resource_utf8.data(),resource_utf8.size(),
            data_utf8.data(),data_utf8.size());
        check(c_result == 0,"C11 function-table gain smoke failed");
        test_execution(gain,asr,env);
        test_direct_invalid_run(gain,env);
        std::cout << "Plugin SDK standalone ABI host tests passed\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "Plugin SDK standalone ABI host test failed: " << error.what() << '\n';
        return 1;
    }
}
