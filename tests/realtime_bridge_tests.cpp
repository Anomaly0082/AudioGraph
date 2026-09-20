#include "audioprocess/realtime_bridge.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdlib>
#include <iostream>
#include <limits>
#include <new>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>
#ifdef _WIN32
#include <malloc.h>
#endif

// 只监测当前线程的 C++ new/new[]（包括对齐分配）；不声称拦截 OS/malloc 分配。
// 缓冲、线程、断言和日志在检测范围之外创建，回调由测试预分配的数据驱动。
namespace allocation_probe {
thread_local bool enabled = false;
thread_local std::size_t count = 0;
void mark() noexcept { if (enabled) ++count; }
void* allocate(std::size_t size) {
    mark();
    if (void* memory = std::malloc(size == 0 ? 1 : size)) return memory;
    throw std::bad_alloc{};
}
void* aligned_allocate(std::size_t size, std::size_t alignment) {
    mark();
#ifdef _WIN32
    if (void* memory = _aligned_malloc(size == 0 ? 1 : size, alignment)) return memory;
#else
    if (size > std::numeric_limits<std::size_t>::max() - alignment) throw std::bad_alloc{};
    const auto padded = ((std::max<std::size_t>(size, 1) + alignment - 1) / alignment) * alignment;
    if (void* memory = std::aligned_alloc(alignment, padded)) return memory;
#endif
    throw std::bad_alloc{};
}
void aligned_free(void* memory) noexcept {
#ifdef _WIN32
    _aligned_free(memory);
#else
    std::free(memory);
#endif
}
} // namespace allocation_probe

void* operator new(std::size_t size) { return allocation_probe::allocate(size); }
void* operator new[](std::size_t size) { return allocation_probe::allocate(size); }
void operator delete(void* memory) noexcept { std::free(memory); }
void operator delete[](void* memory) noexcept { std::free(memory); }
void operator delete(void* memory, std::size_t) noexcept { std::free(memory); }
void operator delete[](void* memory, std::size_t) noexcept { std::free(memory); }
void* operator new(std::size_t size, std::align_val_t align) {
    return allocation_probe::aligned_allocate(size, static_cast<std::size_t>(align));
}
void* operator new[](std::size_t size, std::align_val_t align) {
    return allocation_probe::aligned_allocate(size, static_cast<std::size_t>(align));
}
void operator delete(void* memory, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void operator delete[](void* memory, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void operator delete(void* memory, std::size_t, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void operator delete[](void* memory, std::size_t, std::align_val_t) noexcept { allocation_probe::aligned_free(memory); }
void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
    try { return ::operator new(size); } catch (...) { return nullptr; }
}
void* operator new[](std::size_t size, const std::nothrow_t&) noexcept {
    try { return ::operator new[](size); } catch (...) { return nullptr; }
}
void operator delete(void* memory, const std::nothrow_t&) noexcept { std::free(memory); }
void operator delete[](void* memory, const std::nothrow_t&) noexcept { std::free(memory); }
void* operator new(std::size_t size, std::align_val_t align, const std::nothrow_t&) noexcept {
    try { return ::operator new(size, align); } catch (...) { return nullptr; }
}
void* operator new[](std::size_t size, std::align_val_t align, const std::nothrow_t&) noexcept {
    try { return ::operator new[](size, align); } catch (...) { return nullptr; }
}
void operator delete(void* memory, std::align_val_t, const std::nothrow_t&) noexcept {
    allocation_probe::aligned_free(memory);
}
void operator delete[](void* memory, std::align_val_t, const std::nothrow_t&) noexcept {
    allocation_probe::aligned_free(memory);
}

namespace {
using namespace audioprocess;

void require(bool condition, const std::string& message) {
    if (!condition) throw std::runtime_error(message);
}
bool near(float value, float expected, float tolerance = 0.00001F) {
    return std::abs(value - expected) <= tolerance;
}
bool silent(const std::vector<float>& values) {
    return std::ranges::all_of(values, [](float value) { return value == 0.0F; });
}
void valid_stereo(const float* output, std::size_t frames) {
    for (std::size_t frame = 0; frame < frames; ++frame) {
        require(std::isfinite(output[2 * frame]) && std::abs(output[2 * frame]) <= 1.0F,
            "Output sample is non-finite or outside the output ceiling");
        require(output[2 * frame] == output[2 * frame + 1], "Mono output was not duplicated to stereo");
    }
}

void test_startup_underflow_and_recovery() {
    RealtimeBridge bridge;
    std::vector<float> output(256 * 2, 0.75F);
    bridge.render(output.data(), 256);
    require(silent(output), "Unprimed playback did not clear output to silence");
    std::vector<float> input(4096, 0.25F);
    bridge.capture(input.data(), 400);
    bridge.render(output.data(), 256);
    require(silent(output), "Playback started before target prebuffer was reached");
    bridge.capture(input.data(), 1200);
    bridge.render(output.data(), 256);
    valid_stereo(output.data(), 256);
    require(near(output.back(), 0.25F), "Primed bridge did not produce captured samples");
    // 消费到队列枯竭；一次回调尾部及后续回调都必须补零。
    std::vector<float> exhausted(8192 * 2, 0.75F);
    bridge.render(exhausted.data(), 8192);
    require(exhausted.back() == 0.0F, "Underflow exposed stale output samples");
    require(bridge.stats().underflow_frames > 0 && bridge.stats().buffering_silence_frames >= 512,
        "Underflow and startup buffering were not reported separately");
    bridge.capture(input.data(), 100);
    std::fill(output.begin(), output.end(), 0.75F);
    bridge.render(output.data(), 256);
    require(silent(output), "Underflow recovery skipped rebuffer threshold");
    bridge.capture(input.data(), 1600);
    bridge.render(output.data(), 256);
    require(near(output.back(), 0.25F), "Bridge failed to recover after rebuffering");
}

void test_drop_new_and_mute() {
    RealtimeBridgeConfig config;
    config.capacity_frames = 1024;
    config.target_frames = 256;
    RealtimeBridge bridge(config);
    std::vector<float> original(1024, 0.2F);
    std::vector<float> discarded(1024, -0.8F);
    std::vector<float> output(128 * 2);
    bridge.capture(original.data(), 1024);
    bridge.capture(discarded.data(), 1024);
    require(bridge.stats().dropped_frames == 1024 && bridge.stats().queued_frames == 1024,
        "Overflow counters do not match accepted prefix/drop-new behavior");
    bridge.render(output.data(), 128);
    require(std::ranges::all_of(output, [](float value) { return near(value, 0.2F); }),
        "Overflow overwrote queued reader data rather than dropping new input");
    bridge.set_muted(true);
    const auto before = bridge.stats().queued_frames;
    bridge.render(output.data(), 128);
    const auto after = bridge.stats().queued_frames;
    require(silent(output) && after < before, "Mute did not silence output while consuming the queue");
    bridge.set_muted(false);
    bridge.render(output.data(), 128);
    require(near(output.back(), 0.2F), "Unmute did not resume valid captured audio");
}

void test_gain_ramp_clipping_and_invalid_values() {
    RealtimeBridge bridge;
    std::vector<float> input(4096, 0.25F);
    std::vector<float> output(600 * 2);
    bridge.capture(input.data(), 4096);
    require(bridge.set_gain_db(-6.0F), "Valid gain was rejected");
    require(!bridge.set_gain_db(std::numeric_limits<float>::quiet_NaN()) &&
        !bridge.set_gain_db(std::numeric_limits<float>::infinity()) &&
        !bridge.set_gain_db(-24.01F) && !bridge.set_gain_db(12.01F), "Invalid gain was accepted");
    bridge.render(output.data(), 600);
    const float target = 0.25F * std::pow(10.0F, -6.0F / 20.0F);
    require(output[0] > 0.24F && output[0] <= 0.25F,
        "Gain switched abruptly instead of beginning a ramp");
    require(near(output[2 * 500], target), "Gain did not reach target after its 10 ms ramp");
    for (std::size_t i = 1; i < 480; ++i) {
        require(output[2 * i] <= output[2 * (i - 1)] &&
            std::abs(output[2 * i] - output[2 * (i - 1)]) < 0.001F, "Gain ramp is discontinuous");
    }
    valid_stereo(output.data(), 600);

    RealtimeBridgeConfig loud;
    loud.gain_db = 12.0F;
    RealtimeBridge clipping(loud);
    std::fill(input.begin(), input.end(), 0.75F);
    clipping.capture(input.data(), 4096);
    clipping.render(output.data(), 600);
    require(std::ranges::all_of(output, [](float value) { return value == 1.0F; }),
        "Output ceiling did not saturate amplified audio");
    require(clipping.stats().clipped_samples == 600, "Clipping should count mono frames, not stereo copies");

    RealtimeBridge sanitized;
    input[0] = std::numeric_limits<float>::quiet_NaN();
    input[1] = std::numeric_limits<float>::infinity();
    input[2] = -std::numeric_limits<float>::infinity();
    sanitized.capture(input.data(), 4096);
    sanitized.render(output.data(), 600);
    valid_stereo(output.data(), 600);
    require(sanitized.stats().sanitized_samples == 3, "Non-finite capture samples were not counted");
    RealtimeBridge null_input;
    null_input.capture(nullptr, 2048);
    null_input.render(output.data(), 600);
    require(silent(output), "Null capture input was not treated as silence");
}

void test_configuration_and_render_arguments() {
    const auto reject = [](RealtimeBridgeConfig config) {
        bool rejected = false;
        try { RealtimeBridge bridge(config); }
        catch (const std::invalid_argument&) { rejected = true; }
        require(rejected, "Invalid bridge configuration was accepted");
    };
    RealtimeBridgeConfig config;
    config.sample_rate = 44100; reject(config);
    config = {}; config.capacity_frames = 1000; reject(config);
    config = {}; config.capacity_frames = 2; reject(config);
    config = {}; config.target_frames = 1; reject(config);
    config = {}; config.target_frames = 4095; reject(config);
    config = {}; config.gain_db = std::numeric_limits<float>::quiet_NaN(); reject(config);
    config = {}; config.gain_db = 13.0F; reject(config);

    RealtimeBridge bridge;
    std::array<float, 32> output{};
    output.fill(0.75F);
    bridge.render(nullptr, 0, 0); // 零帧无操作，不访问指针。
    bridge.render(nullptr, 16, 2);
    bridge.render(output.data(), 16, 3);
    const auto stats = bridge.stats();
    require(stats.invalid_render_calls == 2 && stats.render_frames == 0 &&
        std::ranges::all_of(output, [](float value) { return value == 0.75F; }),
        "Invalid render arguments touched the output or corrupted counters");
    std::vector<float> input(1024, 0.3F);
    bridge.capture(input.data(), 1024);
    bridge.render(output.data(), 16, 1);
    require(near(output[0], 0.3F) && near(output[15], 0.3F) && output[16] == 0.75F,
        "Mono render wrote the wrong channel count");
}

void test_callback_new_allocations() {
    RealtimeBridge bridge;
    std::array<float, 256> input{};
    std::array<float, 512> output{};
    input.fill(0.25F);
    const auto before = allocation_probe::count;
    allocation_probe::enabled = true;
    for (int iteration = 0; iteration < 2000; ++iteration) {
        bridge.capture(input.data(), 256);
        bridge.render(output.data(), 256);
        if (iteration % 100 == 0) {
            static_cast<void>(bridge.set_gain_db(iteration % 200 == 0 ? -6.0F : 0.0F));
            bridge.set_muted(iteration % 400 == 0);
        }
        static_cast<void>(bridge.stats());
    }
    allocation_probe::enabled = false;
    require(allocation_probe::count == before, "Bridge callback/control methods allocated through C++ new");
}

void test_concurrent_spsc_callbacks() {
    RealtimeBridge bridge;
    std::atomic<int> ready{};
    std::atomic_bool valid{true};
    auto rendezvous = [&] {
        ready.fetch_add(1, std::memory_order_release);
        while (ready.load(std::memory_order_acquire) != 2) std::this_thread::yield();
    };
    std::thread capture([&] {
        std::array<float, 256> input{};
        input.fill(0.125F);
        std::uint32_t random = 7;
        rendezvous();
        const auto before = allocation_probe::count;
        allocation_probe::enabled = true;
        for (int i = 0; i < 20000; ++i) {
            random = random * 1664525U + 1013904223U;
            bridge.capture(input.data(), 1U + random % 256U);
        }
        allocation_probe::enabled = false;
        if (allocation_probe::count != before) valid.store(false);
    });
    std::thread playback([&] {
        std::array<float, 512> output{};
        std::uint32_t random = 17;
        rendezvous();
        const auto before = allocation_probe::count;
        allocation_probe::enabled = true;
        for (int i = 0; i < 20000; ++i) {
            random = random * 1664525U + 1013904223U;
            const auto frames = 1U + random % 256U;
            bridge.render(output.data(), frames);
            for (std::uint32_t frame = 0; frame < frames; ++frame) {
                const auto sample = output[2 * frame];
                if (!std::isfinite(sample) || sample < 0.0F || sample > 0.12501F ||
                    sample != output[2 * frame + 1]) valid.store(false);
            }
        }
        allocation_probe::enabled = false;
        if (allocation_probe::count != before) valid.store(false);
    });
    std::thread control([&] {
        for (int iteration = 0; iteration < 10000; ++iteration) {
            if (!bridge.set_gain_db(iteration % 2 == 0 ? -6.0F : 0.0F)) valid.store(false);
            bridge.set_muted(iteration % 3 == 0);
            const auto snapshot = bridge.stats();
            if (snapshot.queued_frames > 4096 || !std::isfinite(snapshot.queue_latency_ms)) valid.store(false);
        }
    });
    capture.join(); playback.join(); control.join();
    require(valid.load(), "Concurrent variable-size callbacks corrupted data or allocated");
    require(bridge.stats().queued_frames <= 4096, "Queue metric exceeded ring capacity");
    // 这只是并发压力与不变量检查，不把无TSan的运行声称为数据竞争形式证明。
}

void test_drift_simulation() {
    for (const double drift : {-0.003, 0.003}) {
        RealtimeBridge bridge;
        std::vector<float> seed(960, 0.25F);
        std::array<float, 256> input{};
        std::array<float, 256> output{};
        input.fill(0.25F);
        bridge.capture(seed.data(), static_cast<std::uint32_t>(seed.size()));
        double capture_budget = 0;
        std::size_t silent_frames = 0;
        std::uint32_t minimum_queue = 4096;
        std::uint32_t maximum_queue = 0;
        // 模拟80秒，不sleep、不接硬件。capture时钟每次比playback快/慢0.3%。
        for (int step = 0; step < 30000; ++step) {
            capture_budget += 128.0 * (1.0 + drift);
            const auto frames = static_cast<std::uint32_t>(capture_budget);
            capture_budget -= frames;
            bridge.capture(input.data(), frames);
            bridge.render(output.data(), 128);
            const auto metrics = bridge.stats();
            require(metrics.queued_frames <= 4096, "Clock correction overflowed queue bounds");
            require(metrics.resample_ratio >= 0.994999F && metrics.resample_ratio <= 1.005001F,
                "Clock correction exceeded its allowed +/-0.5% range");
            if (step >= 3000) {
                minimum_queue = std::min(minimum_queue, static_cast<std::uint32_t>(metrics.queued_frames));
                maximum_queue = std::max(maximum_queue, static_cast<std::uint32_t>(metrics.queued_frames));
                for (std::size_t sample = 0; sample < output.size(); sample += 2)
                    if (output[sample] == 0.0F) ++silent_frames;
            }
        }
        require(minimum_queue > 100 && maximum_queue < 3900,
            "Clock correction did not stabilize away from empty/full queue");
        require(silent_frames < 128, "Clock mismatch caused repeated audible underflows after warmup");
        const auto final = bridge.stats();
        require(final.dropped_frames == 0, "Clock mismatch filled the ring and dropped data");
        require(drift > 0 ? final.resample_ratio > 1.0F : final.resample_ratio < 1.0F,
            "Clock correction consumed samples in the wrong direction");
        require(std::abs(final.queue_latency_ms - final.queued_frames / 48.0) < 0.00001,
            "Queue latency does not describe software queued frames at 48 kHz");
    }
}

void test_stopped_reset() {
    RealtimeBridge bridge;
    std::vector<float> input(2048, 0.3F);
    std::vector<float> output(512, 1.0F);
    bridge.capture(input.data(), 2048);
    bridge.render(output.data(), 256);
    bridge.reset(); // 本测试没有任何并发回调；运行中reset不属于契约。
    require(bridge.stats().queued_frames == 0, "Stopped reset retained queued audio");
    bridge.render(output.data(), 256);
    require(silent(output), "Reset replayed stale audio from previous session");
}
} // namespace

int main() {
    try {
        test_startup_underflow_and_recovery();
        test_drop_new_and_mute();
        test_gain_ramp_clipping_and_invalid_values();
        test_configuration_and_render_arguments();
        test_callback_new_allocations();
        test_concurrent_spsc_callbacks();
        test_drift_simulation();
        test_stopped_reset();
        std::cout << "Independent realtime bridge tests passed.\n";
        return 0;
    } catch (const std::exception& error) {
        allocation_probe::enabled = false;
        std::cerr << "Realtime bridge test failure: " << error.what() << '\n';
        return 1;
    }
}
