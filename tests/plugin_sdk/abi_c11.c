#include <audiograph/plugin.h>
#include <math.h>
#include <string.h>

/* Compiled as C11. The SDK header validates layout from this compiler, while
 * this test actually creates and runs a plugin instance through C tables. */
_Static_assert(offsetof(ag_plugin_api, get_node_api) == 56, "C table offset");
_Static_assert(sizeof(ag_call_context) == 56, "C callback layout");

typedef struct c_output {
    unsigned calls;
    float samples[4];
} c_output;

static uint32_t AG_CALL never_cancelled(void* user) { (void)user; return 0u; }
static uint64_t AG_CALL no_deadline(void* user) { (void)user; return AG_NO_DEADLINE; }
static ag_status AG_CALL capture_audio(void* user, ag_string port, const ag_value* value) {
    c_output* output = (c_output*)user;
    if (!output || !value || !port.data || port.size != 5u || memcmp(port.data,"audio",5u) != 0)
        return AG_INVALID_ARGUMENT;
    if (output->calls++ != 0u || value->type != AG_AUDIO || value->reserved != 0u ||
        value->data.audio.sample_rate != 44100u || value->data.audio.channel_count != 1u ||
        value->data.audio.frame_count != 4u || value->data.audio.sample_count != 4u ||
        !value->data.audio.samples) return AG_INVALID_ARGUMENT;
    for (unsigned i = 0; i < 4u; ++i) {
        if (!isfinite(value->data.audio.samples[i])) return AG_INVALID_ARGUMENT;
        output->samples[i] = value->data.audio.samples[i];
    }
    return AG_OK;
}

int ag_c11_gain_smoke(ag_get_api_fn entry, const char* resource_root, uint64_t resource_size,
    const char* data_root, uint64_t data_size) {
    if (!entry || !resource_root || !data_root) return 1;
    ag_plugin_api plugin = {0};
    if (entry(AG_ABI_MAJOR,AG_ABI_MINOR,(uint32_t)sizeof(plugin),&plugin) != AG_OK ||
        plugin.struct_size != sizeof(plugin) || !plugin.get_node_api) return 2;
    const ag_string type_id = {"org.audiograph.example.gain_v1",sizeof("org.audiograph.example.gain_v1")-1u};
    const ag_string capability = {AG_WHOLE_SYNC_ID,sizeof(AG_WHOLE_SYNC_ID)-1u};
    ag_whole_sync_api api = {0};
    if (plugin.get_node_api(type_id,capability,AG_WHOLE_SYNC_VERSION,(uint32_t)sizeof(api),&api) != AG_OK ||
        api.struct_size != sizeof(api) || api.version != AG_WHOLE_SYNC_VERSION ||
        !api.create || !api.run || !api.destroy) return 3;

    ag_named_value parameter = {0};
    parameter.id.data = "gain_db"; parameter.id.size = 7u;
    parameter.value.type = AG_NUMBER; parameter.value.data.number = -6.020599913279624;
    ag_instance_environment environment = {0};
    environment.struct_size = (uint32_t)sizeof(environment);
    environment.resource_root.data = resource_root; environment.resource_root.size = resource_size;
    environment.data_root.data = data_root; environment.data_root.size = data_size;
    ag_create_info info = {0};
    info.struct_size = (uint32_t)sizeof(info);
    info.parameters = &parameter; info.parameter_count = 1u; info.environment = &environment;
    ag_instance* instance = NULL;
    if (api.create(&info,NULL,&instance) != AG_OK || !instance) return 4;

    const float original[4] = {0.25f,-0.5f,0.75f,-1.0f};
    float input_samples[4]; memcpy(input_samples,original,sizeof(original));
    ag_named_value input = {0};
    input.id.data = "audio"; input.id.size = 5u;
    input.value.type = AG_AUDIO;
    input.value.data.audio.sample_rate = 44100u;
    input.value.data.audio.channel_count = 1u;
    input.value.data.audio.frame_count = 4u;
    input.value.data.audio.sample_count = 4u;
    input.value.data.audio.samples = input_samples;
    ag_call_context context = {0};
    context.struct_size = (uint32_t)sizeof(context);
    context.is_cancelled = never_cancelled; context.remaining_ms = no_deadline;
    context.max_input_bytes = 16u; context.max_output_bytes = 16u;
    context.max_inputs = 1u; context.max_outputs = 1u;
    c_output captured = {0};
    ag_output_sink sink = {0};
    sink.struct_size = (uint32_t)sizeof(sink); sink.user = &captured; sink.emit = capture_audio;
    const ag_status result = api.run(instance,&input,1u,&context,&sink,NULL);
    api.destroy(instance);
    if (result != AG_OK || captured.calls != 1u || memcmp(input_samples,original,sizeof(original)) != 0) return 5;
    for (unsigned i = 0; i < 4u; ++i)
        if (fabsf(captured.samples[i]-original[i]*0.5f) > 0.0005f) return 6;
    return 0;
}
