# 每次使用新的目录，测试不覆盖用户文件，也不会与并行测试冲突。
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef token)
set(work "${TEST_ROOT}/${token}/中文 路径")
file(MAKE_DIRECTORY "${work}")
file(WRITE "${work}/graph.json" [=[
{"schema_version":1,"nodes":[
{"id":"out","type":"text_output","parameters":{"path":"文本.txt"}},
{"id":"in","type":"text_input","parameters":{"text":"你好，Graph"}}],
"connections":[{"from":{"node":"in","port":"text"},"to":{"node":"out","port":"text"}}],
"exports":[{"name":"text","node":"in","port":"text"},{"name":"file","node":"out","port":"path"}]}
]=])
execute_process(COMMAND "${GRAPH_DEMO}" --graph "${work}/graph.json" --validate
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(NOT code EQUAL 0 OR EXISTS "${work}/文本.txt")
    message(FATAL_ERROR "Validation executed IO or failed: ${response} ${log}")
endif()
string(JSON valid GET "${response}" valid)
if(NOT valid)
    message(FATAL_ERROR "Validation response missing valid=true")
endif()
execute_process(COMMAND "${GRAPH_DEMO}" --graph "${work}/graph.json"
    WORKING_DIRECTORY "${TEST_ROOT}" RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(NOT code EQUAL 0)
    message(FATAL_ERROR "Unicode CLI execution failed: ${response} ${log}")
endif()
string(JSON text GET "${response}" outputs text value)
file(READ "${work}/文本.txt" saved)
if(NOT text STREQUAL "你好，Graph" OR NOT saved STREQUAL text)
    message(FATAL_ERROR "UTF-8 text roundtrip failed")
endif()
execute_process(COMMAND "${GRAPH_DEMO}" --graph "${work}/graph.json"
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(code EQUAL 0)
    message(FATAL_ERROR "Output overwrite was accepted")
endif()
string(JSON success GET "${response}" success)
string(JSON error_code GET "${response}" errors 0 code)
if(success OR error_code STREQUAL "")
    message(FATAL_ERROR "Failure response is not structured JSON")
endif()
file(READ "${work}/文本.txt" unchanged)
if(NOT unchanged STREQUAL saved)
    message(FATAL_ERROR "Existing output was modified on failure")
endif()
execute_process(COMMAND "${GRAPH_DEMO}" --describe-node gain
    RESULT_VARIABLE code OUTPUT_VARIABLE response ENCODING UTF-8)
string(JSON minimum GET "${response}" node parameters 0 minimum)
if(NOT code EQUAL 0 OR NOT minimum EQUAL -24)
    message(FATAL_ERROR "Gain capability schema missing")
endif()
execute_process(COMMAND "${GRAPH_DEMO}" --unknown
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_QUIET ENCODING UTF-8)
string(JSON error_code GET "${response}" errors 0 code)
if(code EQUAL 0 OR NOT error_code STREQUAL "invalid_arguments")
    message(FATAL_ERROR "Invalid argument contract failed")
endif()
