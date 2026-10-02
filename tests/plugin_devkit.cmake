cmake_minimum_required(VERSION 3.25)
foreach(required SOURCE_ROOT TEST_ROOT GENERATOR)
    if(NOT DEFINED ${required} OR "${${required}}" STREQUAL "")
        message(FATAL_ERROR "Missing ${required}")
    endif()
endforeach()
function(checked)
    execute_process(COMMAND ${ARGV} RESULT_VARIABLE result OUTPUT_VARIABLE output ERROR_VARIABLE error)
    if(NOT result EQUAL 0)
        message(FATAL_ERROR "Devkit command failed (${result})\n${output}\n${error}")
    endif()
    message(STATUS "${output}")
endfunction()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef nonce)
set(run_root "${TEST_ROOT}/run-${nonce}")
set(exported "${run_root}/exported")
set(relocated "${run_root}/relocated 插件 devkit")
set(options "-DGENERATOR=${GENERATOR}" "-DPLATFORM=${PLATFORM}" "-DTOOLSET=${TOOLSET}")
set(forbidden "${SOURCE_ROOT}/sdk/plugin/include/devkit-forbidden-${nonce}")
execute_process(COMMAND "${CMAKE_COMMAND}" "-DOUTPUT_DIR=${forbidden}" ${options}
    -P "${SOURCE_ROOT}/scripts/export_plugin_devkit.cmake"
    RESULT_VARIABLE source_refusal OUTPUT_VARIABLE output ERROR_VARIABLE error)
if(source_refusal EQUAL 0 OR EXISTS "${forbidden}"
   OR NOT "${output}${error}" MATCHES "Refusing source-tree OUTPUT_DIR")
    message(FATAL_ERROR "Export must refuse source-tree output before creating it")
endif()
checked("${CMAKE_COMMAND}" "-DOUTPUT_DIR=${exported}" ${options}
    -P "${SOURCE_ROOT}/scripts/export_plugin_devkit.cmake")
file(SHA256 "${exported}/README.md" readme_before)
file(GLOB_RECURSE files_before RELATIVE "${exported}" "${exported}/*")
execute_process(COMMAND "${CMAKE_COMMAND}" "-DOUTPUT_DIR=${exported}" ${options}
    -P "${SOURCE_ROOT}/scripts/export_plugin_devkit.cmake"
    RESULT_VARIABLE refusal OUTPUT_VARIABLE output ERROR_VARIABLE error)
if(refusal EQUAL 0 OR NOT "${output}${error}" MATCHES "Refusing existing OUTPUT_DIR")
    message(FATAL_ERROR "Export must explicitly refuse an existing destination")
endif()
file(SHA256 "${exported}/README.md" readme_after)
file(GLOB_RECURSE files_after RELATIVE "${exported}" "${exported}/*")
if(NOT readme_before STREQUAL readme_after OR NOT files_before STREQUAL files_after)
    message(FATAL_ERROR "Refused export changed existing output")
endif()
set(incomplete "${run_root}/incomplete")
file(MAKE_DIRECTORY "${incomplete}")
file(WRITE "${incomplete}/sentinel.txt" "existing incomplete output\n")
execute_process(COMMAND "${CMAKE_COMMAND}" "-DOUTPUT_DIR=${incomplete}" ${options}
    -P "${SOURCE_ROOT}/scripts/export_plugin_devkit.cmake"
    RESULT_VARIABLE refusal OUTPUT_VARIABLE output ERROR_VARIABLE error)
file(READ "${incomplete}/sentinel.txt" sentinel)
file(GLOB_RECURSE incomplete_files RELATIVE "${incomplete}" "${incomplete}/*")
if(refusal EQUAL 0 OR NOT "${output}${error}" MATCHES "Refusing existing OUTPUT_DIR"
   OR NOT sentinel STREQUAL "existing incomplete output\n" OR NOT incomplete_files STREQUAL "sentinel.txt")
    message(FATAL_ERROR "Export must preserve and refuse an incomplete existing destination")
endif()
set(allowed README.md docs/plugin-packages.md docs/plugin-development.md
    examples/graphs/plugin-gain.json scripts/verify-example.cmake
    include/audiograph/plugin.h
    lib/cmake/AudioGraphPluginSDK/AudioGraphPluginSDKTargets.cmake
    lib/cmake/AudioGraphPluginSDK/AudioGraphPluginSDKConfig.cmake
    lib/cmake/AudioGraphPluginSDK/AudioGraphPluginSDKConfigVersion.cmake)
foreach(relative IN ITEMS CMakeLists.txt nodes.json cmake/manifest.json.in
    cmake/node_description.h.in cmake/package.cmake src/plugin.cpp
    src/example_algorithms.cpp src/example_algorithms.h resources/config/gain-scale.txt)
    list(APPEND allowed "examples/standalone/${relative}")
endforeach()
foreach(relative IN ITEMS CMakeLists.txt host.cpp abi_c11.c)
    list(APPEND allowed "tests/sdk-host/${relative}")
endforeach()
list(SORT allowed)
list(SORT files_after)
if(NOT files_after STREQUAL allowed)
    message(FATAL_ERROR "Export differs from the public asset allowlist: ${files_after}")
endif()
file(GLOB metadata "${exported}/lib/cmake/AudioGraphPluginSDK/*.cmake")
foreach(path IN LISTS metadata)
    file(READ "${path}" content)
    string(FIND "${content}" "${SOURCE_ROOT}" source_position)
    string(FIND "${content}" "${run_root}" build_position)
    if(NOT source_position EQUAL -1 OR NOT build_position EQUAL -1)
        message(FATAL_ERROR "Installed SDK metadata contains a source/build absolute path: ${path}")
    endif()
endforeach()
file(RENAME "${exported}" "${relocated}" NO_REPLACE)
checked("${CMAKE_COMMAND}" "-DWORK_ROOT=${run_root}/verification"
    ${options} "-DCONFIG=${CONFIG}" -P "${relocated}/scripts/verify-example.cmake")
