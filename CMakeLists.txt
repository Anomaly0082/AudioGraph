cmake_minimum_required(VERSION 3.25)

project(
    AudioProcess
    VERSION 0.1.0
    DESCRIPTION "Experimental block-based audio processing framework"
    LANGUAGES C CXX
)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_subdirectory(third_party/rnnoise)

# One fixed miniaudio implementation provides both portable DSP and Windows devices.
add_library(miniaudio STATIC src/miniaudio_impl.c)
target_include_directories(miniaudio SYSTEM PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}/third_party")
target_compile_definitions(miniaudio PUBLIC MA_NO_DECODING MA_NO_ENCODING
    MA_NO_RESOURCE_MANAGER MA_NO_NODE_GRAPH MA_NO_ENGINE)
if(WIN32)
    target_compile_definitions(miniaudio PUBLIC MA_ENABLE_ONLY_SPECIFIC_BACKENDS MA_ENABLE_WASAPI)
    target_link_libraries(miniaudio PRIVATE ole32)
else()
    target_compile_definitions(miniaudio PUBLIC MA_NO_DEVICE_IO MA_NO_THREADING)
    target_link_libraries(miniaudio PRIVATE m)
endif()
if(MSVC)
    target_compile_options(miniaudio PRIVATE /W0)
endif()

add_library(audio_core STATIC
    src/graph_validator.cpp
    src/node.cpp
    src/processing_chain.cpp
    src/prototype_nodes.cpp
    src/denoise_node.cpp
    src/format_nodes.cpp
    src/sync_graph_executor.cpp
    src/streaming_graph_executor.cpp
    src/streaming_nodes.cpp
    src/wav_file.cpp
    src/realtime_bridge.cpp
    src/realtime_graph_executor.cpp
    src/realtime_nodes.cpp
)

target_include_directories(audio_core
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}/include
)

target_compile_features(audio_core PUBLIC cxx_std_20)
target_link_libraries(audio_core PRIVATE rnnoise miniaudio)

# JSON 仅用于配置/协议边界，核心节点接口保持无第三方 JSON 类型。
add_library(nlohmann_json INTERFACE)
add_library(nlohmann_json::nlohmann_json ALIAS nlohmann_json)
target_include_directories(nlohmann_json SYSTEM INTERFACE "${CMAKE_CURRENT_SOURCE_DIR}/third_party")
add_library(audio_graph_io STATIC src/graph_codec.cpp)
target_link_libraries(audio_graph_io PUBLIC audio_core PRIVATE nlohmann_json::nlohmann_json)

add_library(audio_realtime_config STATIC src/realtime_config.cpp)
target_link_libraries(audio_realtime_config PUBLIC audio_core PRIVATE nlohmann_json::nlohmann_json)

if(WIN32)
    # PUBLIC miniaudio definitions keep device structures consistent across C/C++.
    add_library(audio_device_session STATIC src/realtime_session.cpp)
    target_link_libraries(audio_device_session PUBLIC audio_core PRIVATE miniaudio)
    add_executable(realtime-cli apps/realtime_cli/main.cpp)
    target_link_libraries(realtime-cli PRIVATE audio_device_session audio_realtime_config audio_graph_io nlohmann_json::nlohmann_json)
    if(MSVC)
        target_compile_options(audio_device_session PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
        target_compile_options(realtime-cli PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    endif()
endif()

find_package(Threads REQUIRED)
add_library(audio_tasks STATIC src/task_service.cpp src/task_runner.cpp)
target_link_libraries(audio_tasks PUBLIC audio_core PRIVATE audio_graph_io Threads::Threads nlohmann_json::nlohmann_json)
if(WIN32)
    target_link_libraries(audio_tasks PRIVATE audio_device_session)
endif()
add_library(audio_control STATIC src/control_protocol.cpp)

# Native plugin IO/adaptation is outside the algorithm core and public SDK.
add_library(audio_plugins STATIC src/plugin_host.cpp)
target_link_libraries(audio_plugins PUBLIC audio_core PRIVATE audio_graph_io nlohmann_json::nlohmann_json)
target_include_directories(audio_plugins PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/sdk/plugin/include")
if(WIN32)
    target_link_libraries(audio_plugins PRIVATE bcrypt)
endif()
target_link_libraries(audio_control PUBLIC audio_tasks PRIVATE audio_graph_io audio_plugins nlohmann_json::nlohmann_json)
if(WIN32)
    target_link_libraries(audio_control PRIVATE audio_device_session)
endif()
add_executable(control-cli apps/control_cli/main.cpp)
target_link_libraries(control-cli PRIVATE audio_control audio_graph_io)
foreach(task_target IN ITEMS audio_tasks audio_control audio_plugins control-cli)
    if(MSVC)
        target_compile_options(${task_target} PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    else()
        target_compile_options(${task_target} PRIVATE -Wall -Wextra -Wpedantic)
    endif()
endforeach()

if(MSVC)
    target_compile_options(audio_core PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    target_compile_options(audio_graph_io PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    target_compile_options(audio_realtime_config PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
else()
    target_compile_options(audio_core PRIVATE -Wall -Wextra -Wpedantic)
    target_compile_options(audio_graph_io PRIVATE -Wall -Wextra -Wpedantic)
    target_compile_options(audio_realtime_config PRIVATE -Wall -Wextra -Wpedantic)
endif()

add_executable(audio-cli apps/audio_cli/main.cpp)
target_link_libraries(audio-cli PRIVATE audio_core)

add_executable(graph-demo apps/graph_demo/main.cpp)
target_link_libraries(graph-demo PRIVATE audio_graph_io)

if(MSVC)
    target_compile_options(audio-cli PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    target_compile_options(graph-demo PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
else()
    target_compile_options(audio-cli PRIVATE -Wall -Wextra -Wpedantic)
    target_compile_options(graph-demo PRIVATE -Wall -Wextra -Wpedantic)
endif()

option(AUDIOPROCESS_COPY_TAURI_SIDECAR "Copy Windows x64 engine for the desktop prototype" ON)
if(AUDIOPROCESS_COPY_TAURI_SIDECAR AND WIN32 AND MSVC AND CMAKE_SIZEOF_VOID_P EQUAL 8
   AND NOT CMAKE_GENERATOR_PLATFORM MATCHES "ARM")
set(TAURI_SIDECAR_DIR "${CMAKE_CURRENT_SOURCE_DIR}/apps/desktop/src-tauri/binaries")
file(MAKE_DIRECTORY "${TAURI_SIDECAR_DIR}")
add_custom_command(TARGET graph-demo POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
        $<TARGET_FILE:graph-demo>
        "${TAURI_SIDECAR_DIR}/graph-demo-x86_64-pc-windows-msvc.exe"
    COMMENT "Copying graph-demo into the Tauri sidecar directory"
)
add_custom_command(TARGET control-cli POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
        $<TARGET_FILE:control-cli>
        "${TAURI_SIDECAR_DIR}/control-cli-x86_64-pc-windows-msvc.exe"
    COMMENT "Copying control-cli into the Tauri sidecar directory"
)
endif()

include(CTest)

# Separately installed SDK + copied external plugin + C/C++ ABI test host.
# Does not register or load plugins in any application executable.
if(BUILD_TESTING AND WIN32 AND CMAKE_SIZEOF_VOID_P EQUAL 8 AND MSVC
   AND NOT CMAKE_GENERATOR_PLATFORM MATCHES "ARM")
    add_test(NAME plugin-sdk-standalone COMMAND ${CMAKE_COMMAND}
        "-DSOURCE_ROOT=${CMAKE_CURRENT_SOURCE_DIR}"
        "-DTEST_ROOT=${CMAKE_CURRENT_BINARY_DIR}/plugin-sdk-standalone"
        "-DGENERATOR=${CMAKE_GENERATOR}"
        "-DPLATFORM=${CMAKE_GENERATOR_PLATFORM}"
        "-DTOOLSET=${CMAKE_GENERATOR_TOOLSET}"
        "-DCONFIG=$<CONFIG>"
        -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/plugin_sdk_standalone.cmake")
    set_tests_properties(plugin-sdk-standalone PROPERTIES TIMEOUT 180)
    set_tests_properties(plugin-sdk-standalone PROPERTIES FIXTURES_SETUP plugin_sdk_package)
    add_executable(plugin-host-tests tests/plugin_host_tests.cpp)
    target_link_libraries(plugin-host-tests PRIVATE audio_control audio_plugins audio_graph_io nlohmann_json::nlohmann_json bcrypt)
    target_compile_options(plugin-host-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    add_test(NAME plugin-host-tests COMMAND plugin-host-tests
        "${CMAKE_CURRENT_BINARY_DIR}/plugin-sdk-standalone/plugin-build/package/$<CONFIG>")
    set_tests_properties(plugin-host-tests PROPERTIES TIMEOUT 60 FIXTURES_REQUIRED plugin_sdk_package)
endif()

if(BUILD_TESTING)
    add_executable(format-nodes-tests tests/format_nodes_tests.cpp)
    target_link_libraries(format-nodes-tests PRIVATE audio_core miniaudio)
    if(MSVC)
        target_compile_options(format-nodes-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    else()
        target_compile_options(format-nodes-tests PRIVATE -Wall -Wextra -Wpedantic)
    endif()
    add_test(NAME format-nodes-tests COMMAND format-nodes-tests)
    set_tests_properties(format-nodes-tests PROPERTIES TIMEOUT 60)

    add_executable(denoise-node-tests tests/denoise_node_tests.cpp)
    target_link_libraries(denoise-node-tests PRIVATE audio_core rnnoise)
    if(MSVC)
        target_compile_options(denoise-node-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    else()
        target_compile_options(denoise-node-tests PRIVATE -Wall -Wextra -Wpedantic)
    endif()
    add_test(NAME denoise-node-tests COMMAND denoise-node-tests)
    set_tests_properties(denoise-node-tests PROPERTIES TIMEOUT 60)

    add_executable(audio-tests tests/audio_tests.cpp)
    target_link_libraries(audio-tests PRIVATE audio_core)

    add_executable(graph-tests tests/graph_tests.cpp)
    target_link_libraries(graph-tests PRIVATE audio_core)

    if(MSVC)
        target_compile_options(audio-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
        target_compile_options(graph-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    else()
        target_compile_options(audio-tests PRIVATE -Wall -Wextra -Wpedantic)
        target_compile_options(graph-tests PRIVATE -Wall -Wextra -Wpedantic)
    endif()

    add_test(NAME audio-tests COMMAND audio-tests)
    add_test(NAME graph-tests COMMAND graph-tests)

    foreach(task_test IN ITEMS task-service task-runner control-protocol)
        string(REPLACE "-" "_" task_test_source "${task_test}")
        add_executable(${task_test}-tests tests/${task_test_source}_tests.cpp)
        target_link_libraries(${task_test}-tests PRIVATE audio_control audio_graph_io Threads::Threads nlohmann_json::nlohmann_json)
        if(MSVC)
            target_compile_options(${task_test}-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
        else()
            target_compile_options(${task_test}-tests PRIVATE -Wall -Wextra -Wpedantic)
        endif()
        add_test(NAME ${task_test}-tests COMMAND ${task_test}-tests)
        set_tests_properties(${task_test}-tests PROPERTIES TIMEOUT 30)
    endforeach()
    if(WIN32)
        add_test(NAME control-cli-tests COMMAND powershell -NoProfile -ExecutionPolicy Bypass
            -File ${CMAKE_CURRENT_SOURCE_DIR}/tests/control_cli_tests.ps1
            -Executable $<TARGET_FILE:control-cli> -TestRoot ${CMAKE_CURRENT_BINARY_DIR}/control-cli-tests)
        set_tests_properties(control-cli-tests PROPERTIES TIMEOUT 30)
    endif()

    find_package(Threads REQUIRED)
    foreach(test_name IN ITEMS contracts execution-contract file-boundary codec stream-contract stream-nodes realtime-bridge realtime-config realtime-graph)
        string(REPLACE "-" "_" test_source "${test_name}")
        add_executable(${test_name}-tests tests/${test_source}_tests.cpp)
        target_link_libraries(${test_name}-tests PRIVATE audio_graph_io audio_realtime_config Threads::Threads)
        if(MSVC)
            target_compile_options(${test_name}-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
        else()
            target_compile_options(${test_name}-tests PRIVATE -Wall -Wextra -Wpedantic)
        endif()
        add_test(NAME ${test_name}-tests COMMAND ${test_name}-tests)
        set_tests_properties(${test_name}-tests PROPERTIES TIMEOUT 30)
    endforeach()
    add_test(NAME cli-contract-tests COMMAND ${CMAKE_COMMAND}
        -DGRAPH_DEMO=$<TARGET_FILE:graph-demo>
        -DTEST_ROOT=${CMAKE_CURRENT_BINARY_DIR}/cli-contract-tests
        -P ${CMAKE_CURRENT_SOURCE_DIR}/tests/cli_contract_tests.cmake)

    add_executable(stream-cli-fixture tests/stream_cli_fixture.cpp)
    target_link_libraries(stream-cli-fixture PRIVATE audio_core)
    if(MSVC)
        target_compile_options(stream-cli-fixture PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    else()
        target_compile_options(stream-cli-fixture PRIVATE -Wall -Wextra -Wpedantic)
    endif()
    add_test(NAME stream-cli-tests COMMAND ${CMAKE_COMMAND}
        -DGRAPH_DEMO=$<TARGET_FILE:graph-demo>
        -DFIXTURE=$<TARGET_FILE:stream-cli-fixture>
        -DTEST_ROOT=${CMAKE_CURRENT_BINARY_DIR}/stream-cli-tests
        -P ${CMAKE_CURRENT_SOURCE_DIR}/tests/stream_cli_tests.cmake)
    set_tests_properties(stream-cli-tests PROPERTIES TIMEOUT 60)
    if(WIN32)
        add_executable(realtime-session-tests tests/realtime_session_tests.cpp)
        target_link_libraries(realtime-session-tests PRIVATE audio_device_session)
        if(MSVC)
            target_compile_options(realtime-session-tests PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
        endif()
        add_test(NAME realtime-session-tests COMMAND realtime-session-tests)
        set_tests_properties(realtime-session-tests PROPERTIES TIMEOUT 30)
        add_test(NAME realtime-cli-contract-tests COMMAND ${CMAKE_COMMAND}
            -DREALTIME_CLI=$<TARGET_FILE:realtime-cli>
            -DTEST_ROOT=${CMAKE_CURRENT_BINARY_DIR}/realtime-cli-contract-tests
            -P ${CMAKE_CURRENT_SOURCE_DIR}/tests/realtime_cli_tests.cmake)
        set_tests_properties(realtime-cli-contract-tests PROPERTIES TIMEOUT 30)
    endif()
endif()
