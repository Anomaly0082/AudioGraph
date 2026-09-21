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

# 同一图协议；假设备ID确保 --validate 不因枚举/打开设备而失败。
set(graph [=[
{"schema_version":1,"nodes":[
 {"id":"out","type":"realtime_output","parameters":{"device_id":"fake-render"}},
 {"id":"gain","type":"realtime_gain","parameters":{"gain_db":-6}},
 {"id":"in","type":"realtime_input","parameters":{"device_id":"fake-capture"}}
],"connections":[
 {"from":{"node":"in","port":"audio"},"to":{"node":"gain","port":"audio"}},
 {"from":{"node":"gain","port":"audio"},"to":{"node":"out","port":"audio"}}
]}
]=])
file(WRITE "${work}/graph.json" "${graph}")
execute_process(COMMAND "${REALTIME_CLI}" --graph "${work}/graph.json" --validate
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(NOT code EQUAL 0)
    message(FATAL_ERROR "Realtime graph validation failed: ${response} ${log}")
endif()
string(JSON accessed GET "${response}" device_access)
string(JSON count GET "${response}" node_count)
if(accessed OR NOT count EQUAL 3)
    message(FATAL_ERROR "Realtime graph validation contract changed")
endif()
execute_process(COMMAND "${REALTIME_CLI}" --describe-node realtime_gain
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(NOT code EQUAL 0)
    message(FATAL_ERROR "Node description failed: ${log}")
endif()
string(JSON role GET "${response}" node realtime_role)
string(JSON rate GET "${response}" node realtime_capabilities format sample_rate)
string(JSON channels GET "${response}" node realtime_capabilities format channels)
string(JSON offline GET "${response}" node realtime_capabilities offline_drivable)
string(JSON variable GET "${response}" node realtime_capabilities supports_variable_blocks)
if(NOT role STREQUAL "processor" OR NOT rate EQUAL 48000 OR NOT channels EQUAL 1 OR NOT offline OR NOT variable)
    message(FATAL_ERROR "Realtime capabilities are not discoverable")
endif()
execute_process(COMMAND "${REALTIME_CLI}" --list-nodes
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(NOT code EQUAL 0 OR NOT response MATCHES "realtime_gain")
    message(FATAL_ERROR "Realtime registry discovery failed")
endif()

# 离线节点不能仅因有音频端口就混入实时图。
string(REPLACE "realtime_gain" "stream_gain" offline_graph "${graph}")
file(WRITE "${work}/offline.json" "${offline_graph}")
execute_process(COMMAND "${REALTIME_CLI}" --graph "${work}/offline.json" --validate
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_QUIET ENCODING UTF-8)
string(JSON success GET "${response}" success)
if(code EQUAL 0 OR success)
    message(FATAL_ERROR "Offline streaming node accepted in realtime graph")
endif()

foreach(extra IN ITEMS --input --gain-db --config)
    execute_process(COMMAND "${REALTIME_CLI}" --graph "${work}/graph.json" --validate "${extra}" "x"
        RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_QUIET ENCODING UTF-8)
    if(code EQUAL 0)
        message(FATAL_ERROR "Graph and legacy configuration mixed")
    endif()
endforeach()

# 端口/参数错误必须在probe（非validate）入口也先拒绝，不能尝试打开假设备。
string(REPLACE "\"port\":\"audio\"" "\"port\":\"missing\"" invalid_graph "${graph}")
file(WRITE "${work}/invalid.json" "${invalid_graph}")
execute_process(COMMAND "${REALTIME_CLI}" --graph "${work}/invalid.json" --probe --seconds 1
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_QUIET ENCODING UTF-8)
string(JSON error_code GET "${response}" errors 0 code)
string(JSON node_id GET "${response}" errors 0 node_id)
if(code EQUAL 0 OR error_code STREQUAL "realtime_backend_error" OR error_code STREQUAL "device_not_found" OR node_id STREQUAL "")
    message(FATAL_ERROR "Invalid graph did not fail before device startup with a node location")
endif()
