cmake_minimum_required(VERSION 3.25)
project(AudioGraphPluginSDK VERSION 0.1.0 LANGUAGES C CXX)

include(GNUInstallDirs)
include(CMakePackageConfigHelpers)
add_library(audiograph_plugin_sdk INTERFACE)
add_library(AudioGraph::PluginSDK ALIAS audiograph_plugin_sdk)
set_target_properties(audiograph_plugin_sdk PROPERTIES EXPORT_NAME PluginSDK)
target_include_directories(audiograph_plugin_sdk INTERFACE
    $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
    $<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}>)
install(TARGETS audiograph_plugin_sdk EXPORT AudioGraphPluginSDKTargets)
install(DIRECTORY include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
install(EXPORT AudioGraphPluginSDKTargets NAMESPACE AudioGraph::
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/AudioGraphPluginSDK)
configure_package_config_file(cmake/AudioGraphPluginSDKConfig.cmake.in
    ${CMAKE_CURRENT_BINARY_DIR}/AudioGraphPluginSDKConfig.cmake
    INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/AudioGraphPluginSDK)
# 0.x ABI is experimental: package version compatibility is intentionally exact.
write_basic_package_version_file(${CMAKE_CURRENT_BINARY_DIR}/AudioGraphPluginSDKConfigVersion.cmake
    VERSION ${PROJECT_VERSION} COMPATIBILITY ExactVersion)
install(FILES ${CMAKE_CURRENT_BINARY_DIR}/AudioGraphPluginSDKConfig.cmake
    ${CMAKE_CURRENT_BINARY_DIR}/AudioGraphPluginSDKConfigVersion.cmake
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/AudioGraphPluginSDK)
