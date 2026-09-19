cmake_minimum_required(VERSION 3.25)

project(
    AudioProcess
    VERSION 0.1.0
    DESCRIPTION "Experimental block-based audio processing framework"
    LANGUAGES CXX
)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_library(audio_core STATIC
    src/node.cpp
    src/processing_chain.cpp
    src/prototype_nodes.cpp
    src/sync_graph_executor.cpp
    src/wav_file.cpp
)

target_include_directories(audio_core
    PUBLIC
        ${CMAKE_CURRENT_SOURCE_DIR}/include
)

target_compile_features(audio_core PUBLIC cxx_std_20)

if(MSVC)
    target_compile_options(audio_core PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
else()
    target_compile_options(audio_core PRIVATE -Wall -Wextra -Wpedantic)
endif()

add_executable(audio-cli apps/audio_cli/main.cpp)
target_link_libraries(audio-cli PRIVATE audio_core)

add_executable(graph-demo apps/graph_demo/main.cpp)
target_link_libraries(graph-demo PRIVATE audio_core)

if(MSVC)
    target_compile_options(audio-cli PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
    target_compile_options(graph-demo PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
else()
    target_compile_options(audio-cli PRIVATE -Wall -Wextra -Wpedantic)
    target_compile_options(graph-demo PRIVATE -Wall -Wextra -Wpedantic)
endif()

set(TAURI_SIDECAR_DIR "${CMAKE_CURRENT_SOURCE_DIR}/apps/desktop/src-tauri/binaries")
file(MAKE_DIRECTORY "${TAURI_SIDECAR_DIR}")
add_custom_command(TARGET graph-demo POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
        $<TARGET_FILE:graph-demo>
        "${TAURI_SIDECAR_DIR}/graph-demo-x86_64-pc-windows-msvc.exe"
    COMMENT "Copying graph-demo into the Tauri sidecar directory"
)

include(CTest)

if(BUILD_TESTING)
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
endif()
