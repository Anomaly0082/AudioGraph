#ifndef AUDIOGRAPH_PLUGIN_H
#define AUDIOGRAPH_PLUGIN_H

/* Experimental ABI 0.1: NOT a stable third-party binary compatibility promise.
 * No AudioGraph implementation headers or libraries are required. C11/C++17+.
 * First tested platform: Windows x64, default 8-byte packing, MSVC VS2022.
 * Every callback and exported function must contain its own C++ exceptions.
 * Native plugins are trusted code, not a filesystem/network security sandbox.
 * Do not write to process stdout: the production worker uses it for its control
 * protocol. Keep private diagnostics bounded, e.g. stderr or plugin data files. */
#include <stdint.h>
#include <stddef.h>
#include <float.h>

#if defined(_WIN32)
# define AG_CALL __cdecl
# if defined(AG_PLUGIN_BUILD)
#  define AG_EXPORT __declspec(dllexport)
# else
#  define AG_EXPORT
# endif
#else
# define AG_CALL
# define AG_EXPORT __attribute__((visibility("default")))
#endif
#ifdef __cplusplus
extern "C" {
#endif

#define AG_ABI_MAJOR 0u
#define AG_ABI_MINOR 1u
#define AG_WHOLE_SYNC_ID "ag.whole_sync/1"
#define AG_WHOLE_SYNC_VERSION 1u
#define AG_NO_DEADLINE UINT64_MAX

typedef uint32_t ag_status;
#define AG_OK                 0u
#define AG_CANCELLED          1u
#define AG_INVALID_ARGUMENT   2u
#define AG_UNSUPPORTED        3u
#define AG_RESOURCE_LIMIT     4u
#define AG_EXECUTION_FAILED   5u
#define AG_INTERNAL_ERROR     6u
#define AG_DEADLINE_EXCEEDED   7u

/* Tags, not compiler-sized enums. BOOLEAN is a parameter only, not a port. */
#define AG_AUDIO     1u
#define AG_NUMBER    2u
#define AG_TEXT      3u
#define AG_FILE_PATH 4u
#define AG_BOOLEAN   5u

typedef struct ag_string {
    const char* data; /* UTF-8; not necessarily NUL terminated. */
    uint64_t size;    /* Bytes. NULL is permitted only for zero length. */
} ag_string;

typedef struct ag_audio {
    uint32_t sample_rate;
    uint32_t channel_count;
    uint64_t frame_count;
    uint64_t sample_count; /* Checked product frame_count * channel_count. */
    const float* samples;  /* Read-only interleaved CPU float32; finite values. */
} ag_audio;

typedef struct ag_value {
    uint32_t type;
    uint32_t reserved; /* Must be zero, as must all reserved fields below. */
    union {
        ag_audio audio;
        double number;
        ag_string string; /* TEXT or FILE_PATH. */
        uint32_t boolean; /* Exactly 0 or 1; parameter only. */
    } data;
} ag_value;

typedef struct ag_named_value {
    ag_string id;
    ag_value value;
} ag_named_value;

typedef struct ag_instance ag_instance;

typedef struct ag_error {
    ag_string code;
    ag_string message;
    ag_string port_id;
    ag_string parameter_id;
} ag_error;
/* Host copies immediately. Borrowed only during the particular create/run call;
 * report on that call's thread, never from a worker or after returning.
 * errors may be NULL when only a status is needed. Non-NULL sinks require report.
 * Host should bound messages (suggested 4 KiB total), and must not log secrets.
 * A failing report callback never changes an original failure into success. */
typedef ag_status (AG_CALL *ag_report_error_fn)(void* user, const ag_error* error);
typedef struct ag_error_sink {
    uint32_t struct_size;
    uint32_t reserved;
    void* user;
    ag_report_error_fn report;
} ag_error_sink;

/* Paths are host-selected absolute UTF-8 paths, NOT Graph FilePath parameters.
 * Copy during create if needed later. Resource root is read-only by contract;
 * data root is plugin/workspace specific. These are not OS access restrictions. */
typedef struct ag_instance_environment {
    uint32_t struct_size;
    uint32_t reserved;
    ag_string resource_root;
    ag_string data_root;
} ag_instance_environment;

typedef struct ag_create_info {
    uint32_t struct_size;
    uint32_t reserved;
    const ag_named_value* parameters; /* Normalized, unique IDs; no Audio values. */
    uint32_t parameter_count;
    uint32_t reserved2;
    const ag_instance_environment* environment;
} ag_create_info;

/* Thread-safe host queries, valid ONLY until run returns. 0 remaining means
 * expired; AG_NO_DEADLINE means no host deadline. Cancellation is cooperative.
 * Workers may query these callbacks, but must finish using them before return. */
typedef uint32_t (AG_CALL *ag_is_cancelled_fn)(void* user);
typedef uint64_t (AG_CALL *ag_remaining_ms_fn)(void* user);
typedef struct ag_call_context {
    uint32_t struct_size;
    uint32_t reserved;
    void* user;
    ag_is_cancelled_fn is_cancelled;
    ag_remaining_ms_fn remaining_ms;
    /* Payload totals, excluding IDs/structs: audio samples*4, Number=8,
     * Text/FilePath=UTF-8 byte count. Zero is zero capacity, not unlimited. */
    uint64_t max_input_bytes;
    uint64_t max_output_bytes;
    uint32_t max_inputs;
    uint32_t max_outputs;
} ag_call_context;

/* emit copies the entire borrowed value into HOST-owned staging storage before
 * returning. Caller owns the original; host must never free caller memory.
 * Run thread only; each output ID at most once. Stop on non-OK. Host validates
 * types, sizes, finite values and workspace FilePaths. Publish staging only if
 * run succeeds AND final cancellation/deadline/output checks pass. No rollback
 * of filesystem/network effects is implied. No callback or pointer retention. */
typedef ag_status (AG_CALL *ag_emit_fn)(void* user, ag_string port, const ag_value* value);
typedef struct ag_output_sink {
    uint32_t struct_size;
    uint32_t reserved;
    void* user;
    ag_emit_fn emit;
} ag_output_sink;

/* create writes NULL to *out_instance on failure. create is lightweight;
 * heavy initialization/HTTP/GPU work belongs in run and its cancellation budget.
 * run is single-use per instance, serialized; separate instances may run in
 * parallel. Input views are read-only and borrowed for run only. Private worker
 * work may use copied inputs but must stop all access to host data before return.
 * Host calls destroy exactly once after successful create, never concurrently
 * with run. The creating plugin frees its own instance and other allocations. */
typedef ag_status (AG_CALL *ag_create_fn)(const ag_create_info* info,
    const ag_error_sink* errors, ag_instance** out_instance);
typedef ag_status (AG_CALL *ag_run_fn)(ag_instance* instance,
    const ag_named_value* inputs, uint32_t input_count,
    const ag_call_context* context, const ag_output_sink* outputs,
    const ag_error_sink* errors);
typedef void (AG_CALL *ag_destroy_fn)(ag_instance* instance);
typedef struct ag_whole_sync_api {
    uint32_t struct_size;
    uint32_t version;
    ag_create_fn create;
    ag_run_fn run;
    ag_destroy_fn destroy;
} ag_whole_sync_api;

typedef ag_status (AG_CALL *ag_receive_description_fn)(void* user, ag_string json);
typedef ag_status (AG_CALL *ag_describe_nodes_fn)(void* user, ag_receive_description_fn receive);
/* Exact capability/version selection, never silent fallback to a different
 * execution model. Unknown capability -> AG_UNSUPPORTED. ABI 0.1 supports only
 * ag.whole_sync/1. On failure, do not write output table bytes. On success write
 * only sizeof(selected table); never touch the caller's trailing guard bytes. */
typedef ag_status (AG_CALL *ag_get_node_api_fn)(ag_string type_id,
    ag_string capability, uint32_t capability_version, uint32_t out_size, void* out_api);
typedef struct ag_plugin_api {
    uint32_t struct_size;
    uint32_t abi_major;
    uint32_t abi_minor;
    uint32_t reserved;
    ag_string plugin_id;      /* Borrowed until module/process exit; host copies. */
    ag_string plugin_version; /* Implementation version, independent of ABI. */
    ag_describe_nodes_fn describe_nodes;
    ag_get_node_api_fn get_node_api;
} ag_plugin_api;

/* Export this exact unmangled symbol. 0.1 requires exact major/minor match.
 * Validate pointers, requested versions and output capacity before writing.
 * Mismatch -> AG_UNSUPPORTED; too-small buffer -> AG_INVALID_ARGUMENT.
 * All input struct_size fields must cover the selected ABI structure; extra
 * trailing bytes are ignored. Table queries never start an algorithm task.
 * Strings/descriptions are data, not instructions; describe JSON is copied
 * synchronously and compared with a static manifest by a future host adapter. */
typedef ag_status (AG_CALL *ag_get_api_fn)(uint32_t major, uint32_t minor,
    uint32_t out_size, ag_plugin_api* out_api);
AG_EXPORT ag_status AG_CALL ag_plugin_get_api(uint32_t major, uint32_t minor,
    uint32_t out_size, ag_plugin_api* out_api);

#ifdef __cplusplus
} /* extern "C" */
#endif

#if defined(__cplusplus)
# define AG_STATIC_ASSERT(c, m) static_assert(c, m)
# define AG_ALIGNOF(t) alignof(t)
#else
# define AG_STATIC_ASSERT(c, m) _Static_assert(c, m)
# define AG_ALIGNOF(t) _Alignof(t)
#endif
/* Fail fast on unsupported packing/architecture rather than silently misread
 * callback pointers. Kept usable by both a C host and a C++ plugin. */
AG_STATIC_ASSERT(sizeof(void*) == 8, "SDK prototype requires 64-bit pointers");
AG_STATIC_ASSERT(sizeof(float) == 4 && sizeof(double) == 8, "unsupported numeric layout");
AG_STATIC_ASSERT(FLT_RADIX == 2 && FLT_MANT_DIG == 24 && DBL_MANT_DIG == 53, "requires binary32/binary64");
AG_STATIC_ASSERT(AG_ALIGNOF(ag_value) == 8 && AG_ALIGNOF(ag_call_context) == 8, "unsupported structure packing");
AG_STATIC_ASSERT(sizeof(ag_string) == 16, "ag_string layout");
AG_STATIC_ASSERT(sizeof(ag_audio) == 32 && offsetof(ag_audio, samples) == 24, "ag_audio layout");
AG_STATIC_ASSERT(sizeof(ag_value) == 40 && offsetof(ag_value, data) == 8, "ag_value layout");
AG_STATIC_ASSERT(sizeof(ag_named_value) == 56, "ag_named_value layout");
AG_STATIC_ASSERT(sizeof(ag_error_sink) == 24 && sizeof(ag_output_sink) == 24, "sink layout");
AG_STATIC_ASSERT(sizeof(ag_error) == 64 && sizeof(ag_create_fn) == 8, "error/function pointer layout");
AG_STATIC_ASSERT(sizeof(ag_instance_environment) == 40, "environment layout");
AG_STATIC_ASSERT(sizeof(ag_create_info) == 32, "create layout");
AG_STATIC_ASSERT(sizeof(ag_call_context) == 56, "context layout");
AG_STATIC_ASSERT(sizeof(ag_whole_sync_api) == 32, "whole sync API layout");
AG_STATIC_ASSERT(sizeof(ag_plugin_api) == 64 && offsetof(ag_plugin_api, get_node_api) == 56, "plugin API layout");
#undef AG_STATIC_ASSERT
#undef AG_ALIGNOF
#endif /* AUDIOGRAPH_PLUGIN_H */
