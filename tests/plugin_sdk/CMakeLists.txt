cmake_minimum_required(VERSION 3.25)
project(AudioGraphPluginSDKHarness LANGUAGES C CXX)
if(NOT WIN32)
    message(FATAL_ERROR "This prototype harness currently exercises LoadLibraryExW on Windows x64")
endif()

# Standalone consumer: only the installed SDK package and standard libraries.
find_package(AudioGraphPluginSDK 0.1.0 EXACT CONFIG REQUIRED)
add_executable(ag_plugin_sdk_host host.cpp abi_c11.c)
target_link_libraries(ag_plugin_sdk_host PRIVATE AudioGraph::PluginSDK)
set_target_properties(ag_plugin_sdk_host PROPERTIES C_STANDARD 11 C_STANDARD_REQUIRED YES
    CXX_STANDARD 20 CXX_STANDARD_REQUIRED YES CXX_EXTENSIONS NO)
if(MSVC)
    target_compile_options(ag_plugin_sdk_host PRIVATE /W4 /permissive- /EHsc /utf-8 /Zc:__cplusplus)
else()
    target_compile_options(ag_plugin_sdk_host PRIVATE -Wall -Wextra -Wpedantic)
endif()

enable_testing()
if(DEFINED AG_TEST_PLUGIN_PATH AND NOT AG_TEST_PLUGIN_PATH STREQUAL "")
    add_test(NAME ag_plugin_sdk_host COMMAND ag_plugin_sdk_host "${AG_TEST_PLUGIN_PATH}")
    set_tests_properties(ag_plugin_sdk_host PROPERTIES TIMEOUT 30)
endif()
