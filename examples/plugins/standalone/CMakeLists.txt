cmake_minimum_required(VERSION 3.25)
project(AudioGraphStandaloneExample VERSION 0.1.0 LANGUAGES CXX)
if(MSVC)
    # Deliberately use a private CRT to demonstrate that the host never frees
    # plugin allocations and owns copies of every emitted temporary result.
    set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

# This project may be copied outside the host repository before configuring.
# The installed public SDK is its only AudioGraph build dependency.
find_package(AudioGraphPluginSDK 0.1.0 EXACT CONFIG REQUIRED)
if(NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
    message(FATAL_ERROR "The experimental ABI 0.1 example requires a 64-bit toolchain")
endif()
if(MSVC)
    string(TOLOWER "${CMAKE_CXX_COMPILER_ARCHITECTURE_ID}" EXAMPLE_TARGET_ARCH)
else()
    string(TOLOWER "${CMAKE_SYSTEM_PROCESSOR}" EXAMPLE_TARGET_ARCH)
endif()
if(NOT EXAMPLE_TARGET_ARCH MATCHES "^(x64|x86_64|amd64)$")
    message(FATAL_ERROR "This prototype supports x86_64 only; ARM64 and other targets are not supported")
endif()

add_library(example_algorithms STATIC src/example_algorithms.cpp)
target_compile_features(example_algorithms PUBLIC cxx_std_20)
target_include_directories(example_algorithms PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}/src)
set_target_properties(example_algorithms PROPERTIES POSITION_INDEPENDENT_CODE ON)

file(READ "${CMAKE_CURRENT_SOURCE_DIR}/nodes.json" EXAMPLE_NODES_JSON)
string(REPLACE "\r\n" "\n" EXAMPLE_NODES_JSON "${EXAMPLE_NODES_JSON}")
string(REPLACE "\r" "\n" EXAMPLE_NODES_JSON "${EXAMPLE_NODES_JSON}")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/nodes.json")
configure_file(cmake/node_description.h.in generated/node_description.h @ONLY NEWLINE_STYLE UNIX)
# Embed and package identical LF-normalized bytes even after a CRLF checkout.
file(GENERATE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/generated/nodes.json"
    CONTENT "${EXAMPLE_NODES_JSON}" NEWLINE_STYLE UNIX)
add_library(ag_example_plugin SHARED src/plugin.cpp)
target_compile_features(ag_example_plugin PRIVATE cxx_std_20)
target_compile_definitions(ag_example_plugin PRIVATE AG_PLUGIN_BUILD)
target_include_directories(ag_example_plugin PRIVATE ${CMAKE_CURRENT_BINARY_DIR}/generated)
target_link_libraries(ag_example_plugin PRIVATE AudioGraph::PluginSDK example_algorithms)
set_target_properties(ag_example_plugin PROPERTIES PREFIX "" CXX_EXTENSIONS OFF)
set_target_properties(example_algorithms PROPERTIES CXX_EXTENSIONS OFF)
if(MSVC)
    target_compile_options(example_algorithms PRIVATE /W4 /permissive- /utf-8)
    target_compile_options(ag_example_plugin PRIVATE /W4 /permissive- /utf-8)
else()
    target_compile_options(example_algorithms PRIVATE -Wall -Wextra -Wpedantic)
    target_compile_options(ag_example_plugin PRIVATE -Wall -Wextra -Wpedantic)
endif()
set_property(TARGET ag_example_plugin APPEND PROPERTY LINK_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/cmake/package.cmake"
    "${CMAKE_CURRENT_SOURCE_DIR}/cmake/manifest.json.in")

add_custom_command(TARGET ag_example_plugin POST_BUILD
    COMMAND ${CMAKE_COMMAND}
        "-DPLUGIN_FILE=$<TARGET_FILE:ag_example_plugin>"
        "-DPACKAGE_DIR=${CMAKE_CURRENT_BINARY_DIR}/package/$<CONFIG>"
        "-DNODES_FILE=${CMAKE_CURRENT_BINARY_DIR}/generated/nodes.json"
        "-DPLATFORM=${CMAKE_SYSTEM_NAME}"
        -P ${CMAKE_CURRENT_SOURCE_DIR}/cmake/package.cmake
    VERBATIM
    COMMENT "Packaging experimental standalone plugin with SHA-256 file identities")
