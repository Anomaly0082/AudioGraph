#pragma once

#include <cstddef>
#if defined(_WIN32)
#if defined(EXAMPLE_ALGORITHMS_BUILD)
#define EXAMPLE_API __declspec(dllexport)
#else
#define EXAMPLE_API __declspec(dllimport)
#endif
#else
#define EXAMPLE_API
#endif
// Only borrowed buffers and primitives cross the self-owned helper DLL boundary.
extern "C" {
EXAMPLE_API bool example_gain_block(const float* input, std::size_t count, double factor, float* output) noexcept;
EXAMPLE_API bool example_finite_block(const float* input, std::size_t count) noexcept;
}
