# Integration for the unmodified, pinned RNNoise v0.2 source/model subset.
# x86-64 uses baseline SSE2, not AVX2 or runtime CPU dispatch. No network steps.
if(NOT CMAKE_SIZEOF_VOID_P EQUAL 8 OR
   NOT CMAKE_SYSTEM_PROCESSOR MATCHES "^(AMD64|amd64|x86_64|X86_64)$" OR
   CMAKE_GENERATOR_PLATFORM MATCHES "ARM")
    message(FATAL_ERROR "The RNNoise v0.2 integration currently supports x86-64 targets only")
endif()

foreach(model_file IN ITEMS rnnoise_data.c rnnoise_data.h)
    if(NOT EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/src/${model_file}")
        message(FATAL_ERROR "RNNoise model is a local, Git-ignored dependency. Read third_party/rnnoise/README.integration.md and run scripts/prepare-rnnoise-model.ps1 explicitly before configuring.")
    endif()
endforeach()
file(SHA256 "${CMAKE_CURRENT_SOURCE_DIR}/src/rnnoise_data.c" model_c_hash)
file(SHA256 "${CMAKE_CURRENT_SOURCE_DIR}/src/rnnoise_data.h" model_h_hash)
if(NOT model_c_hash STREQUAL "522b6a64fded05bf85e58c06206eafe57ce7d94f3af58c725b17628b481d7890" OR
   NOT model_h_hash STREQUAL "09ff880bddd0fc74a2ae0e5ec6c8d65714031b08d0c3f672493acd9e189c5855")
    message(FATAL_ERROR "RNNoise local model checksum mismatch; refusing to build unverified weights")
endif()

add_library(rnnoise STATIC
    src/denoise.c
    src/rnn.c
    src/pitch.c
    src/kiss_fft.c
    src/celt_lpc.c
    src/nnet.c
    src/nnet_default.c
    src/parse_lpcnet_weights.c
    src/rnnoise_data.c
    src/rnnoise_tables.c
)
set_target_properties(rnnoise PROPERTIES C_STANDARD 11 C_STANDARD_REQUIRED YES C_EXTENSIONS NO)
target_include_directories(rnnoise SYSTEM PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_include_directories(rnnoise PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/src")
target_compile_definitions(rnnoise PRIVATE RNNOISE_BUILD)
if(MSVC)
    # The upstream MSVC compatibility header enables its SSE2 implementation.
    target_compile_definitions(rnnoise PRIVATE OPUS_X86_MAY_HAVE_SSE OPUS_X86_MAY_HAVE_SSE2)
    target_compile_options(rnnoise PRIVATE /W0)
else()
    target_link_libraries(rnnoise PRIVATE m)
endif()
