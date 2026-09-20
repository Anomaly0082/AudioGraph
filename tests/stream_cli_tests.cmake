string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef token)
set(work "${TEST_ROOT}/${token}/分块 测试")
file(MAKE_DIRECTORY "${work}")
execute_process(COMMAND "${FIXTURE}" generate "${work}/输入.wav" RESULT_VARIABLE code)
if(NOT code EQUAL 0)
    message(FATAL_ERROR "Failed to generate streaming fixture")
endif()
file(SHA256 "${work}/输入.wav" input_hash)
set(previous_hash "")
foreach(block_size IN ITEMS 1 7 128 256 512 4096 65536)
    file(WRITE "${work}/graph.json" "{
      \"schema_version\":1,
      \"nodes\":[
        {\"id\":\"out\",\"type\":\"wav_stream_output\",\"parameters\":{\"path\":\"输出-${block_size}.wav\"}},
        {\"id\":\"gain\",\"type\":\"stream_gain\",\"parameters\":{\"gain_db\":-6.020599913}},
        {\"id\":\"in\",\"type\":\"wav_stream_input\",\"parameters\":{\"path\":\"输入.wav\"}}],
      \"connections\":[
        {\"from\":{\"node\":\"in\",\"port\":\"audio\"},\"to\":{\"node\":\"gain\",\"port\":\"audio\"}},
        {\"from\":{\"node\":\"gain\",\"port\":\"audio\"},\"to\":{\"node\":\"out\",\"port\":\"audio\"}}],
      \"exports\":[{\"name\":\"frames\",\"node\":\"out\",\"port\":\"frames_written\"},
        {\"name\":\"clipped\",\"node\":\"out\",\"port\":\"clipped_samples\"}]}")
    execute_process(COMMAND "${GRAPH_DEMO}" --graph "${work}/graph.json" --validate --block-size "${block_size}"
        RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
    if(NOT code EQUAL 0 OR EXISTS "${work}/输出-${block_size}.wav")
        message(FATAL_ERROR "Stream validation failed or wrote output: ${response} ${log}")
    endif()
    execute_process(COMMAND "${GRAPH_DEMO}" --graph "${work}/graph.json" --block-size "${block_size}"
        WORKING_DIRECTORY "${TEST_ROOT}" RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
    if(NOT code EQUAL 0)
        message(FATAL_ERROR "Stream CLI failed: ${response} ${log}")
    endif()
    string(JSON frames GET "${response}" outputs frames value)
    string(JSON clipped GET "${response}" outputs clipped value)
    if(NOT frames EQUAL 1041 OR NOT clipped EQUAL 0)
        message(FATAL_ERROR "Incorrect stream result: ${response}")
    endif()
    execute_process(COMMAND "${FIXTURE}" check-half "${work}/输出-${block_size}.wav" RESULT_VARIABLE code)
    if(NOT code EQUAL 0)
        message(FATAL_ERROR "Decoded output differs from expected samples")
    endif()
    file(SHA256 "${work}/输出-${block_size}.wav" output_hash)
    if(NOT previous_hash STREQUAL "" AND NOT previous_hash STREQUAL output_hash)
        message(FATAL_ERROR "Changing block size changed the audio output")
    endif()
    set(previous_hash "${output_hash}")
endforeach()
file(SHA256 "${work}/输入.wav" final_input_hash)
if(NOT input_hash STREQUAL final_input_hash)
    message(FATAL_ERROR "Input file changed")
endif()
execute_process(COMMAND "${GRAPH_DEMO}" --graph "${work}/graph.json"
    RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_VARIABLE log ENCODING UTF-8)
if(code EQUAL 0)
    message(FATAL_ERROR "Existing output was overwritten")
endif()
file(SHA256 "${work}/输出-65536.wav" after_refusal_hash)
if(NOT after_refusal_hash STREQUAL previous_hash)
    message(FATAL_ERROR "Existing output changed despite failure")
endif()
foreach(bad_size IN ITEMS 0 -1 65537 1.5)
    execute_process(COMMAND "${GRAPH_DEMO}" --graph "${work}/graph.json" --block-size "${bad_size}"
        RESULT_VARIABLE code OUTPUT_VARIABLE response ERROR_QUIET ENCODING UTF-8)
    string(JSON error_code GET "${response}" errors 0 code)
    if(code EQUAL 0 OR NOT error_code STREQUAL "invalid_arguments")
        message(FATAL_ERROR "Invalid block size was accepted")
    endif()
endforeach()
execute_process(COMMAND "${GRAPH_DEMO}" --describe-node stream_gain
    RESULT_VARIABLE code OUTPUT_VARIABLE response ENCODING UTF-8)
string(JSON type GET "${response}" node inputs 0 type)
string(JSON role GET "${response}" node stream_role)
if(NOT code EQUAL 0 OR NOT type STREQUAL "AudioStream" OR NOT role STREQUAL "processor")
    message(FATAL_ERROR "Missing streaming capability information")
endif()
