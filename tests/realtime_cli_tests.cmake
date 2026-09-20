# 仅验证配置协议，不在自动CTest中启动麦克风或扬声器。
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef token)
set(work "${TEST_ROOT}/${token}/实时 配置")
file(MAKE_DIRECTORY "${work}")
file(WRITE "${work}/session.json" [=[
{"schema_version":1,"input_device":"not-a-real-capture-id","output_device":"not-a-real-playback-id","gain_db":-6}
]=])
execute_process(COMMAND "${REALTIME_CLI}" --config "${work}/session.json" --validate
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(NOT code EQUAL 0)
    message(FATAL_ERROR "Realtime validation failed: ${response} ${log}")
endif()
string(JSON success GET "${response}" success)
string(JSON accessed GET "${response}" device_access)
if(NOT success OR accessed)
    message(FATAL_ERROR "--validate must not access audio devices")
endif()
foreach(bad IN ITEMS 0 -1 3601 nonsense)
    execute_process(COMMAND "${REALTIME_CLI}" --config "${work}/session.json" --validate --seconds "${bad}"
        RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_QUIET ENCODING UTF-8)
    string(JSON error_code GET "${response}" errors 0 code)
    if(code EQUAL 0 OR NOT error_code STREQUAL "invalid_arguments")
        message(FATAL_ERROR "Invalid seconds accepted")
    endif()
endforeach()
execute_process(COMMAND "${REALTIME_CLI}" --config "${work}/session.json" --validate --probe --monitor
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_QUIET ENCODING UTF-8)
if(code EQUAL 0)
    message(FATAL_ERROR "Contradictory output modes accepted")
endif()
