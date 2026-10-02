#include "example_algorithms.h"

#include <cmath>
#include <limits>

extern "C" bool example_finite_block(const float* input, std::size_t count) noexcept {
    if (count != 0 && input == nullptr) return false;
    for (std::size_t i = 0; i < count; ++i) {
        if (!std::isfinite(input[i])) return false;
    }
    return true;
}
extern "C" bool example_gain_block(const float* input, std::size_t count, double factor, float* output) noexcept {
    if ((count != 0 && (input == nullptr || output == nullptr)) || !std::isfinite(factor)) return false;
    for (std::size_t i = 0; i < count; ++i) {
        if (!std::isfinite(input[i])) return false;
        const double result = static_cast<double>(input[i])*factor;
        if (!std::isfinite(result) || std::abs(result) > std::numeric_limits<float>::max()) return false;
        output[i] = static_cast<float>(result);
    }
    return true;
}
