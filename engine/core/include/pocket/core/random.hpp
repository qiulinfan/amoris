// Seeded deterministic random numbers (PCG32). Gameplay code must use this, never std::rand.
#pragma once

#include <cstdint>

namespace pocket {

struct Random {
    std::uint64_t state = 0x853c49e6748fea9bULL;
    std::uint64_t inc = 0xda3e39cb94b95bdbULL;

    Random() = default;
    explicit Random(std::uint64_t seed, std::uint64_t stream = 1) { reseed(seed, stream); }

    void reseed(std::uint64_t seed, std::uint64_t stream = 1) {
        state = 0;
        inc = (stream << 1u) | 1u;
        next_u32();
        state += seed;
        next_u32();
    }

    std::uint32_t next_u32() {
        std::uint64_t old = state;
        state = old * 6364136223846793005ULL + inc;
        auto xorshifted = static_cast<std::uint32_t>(((old >> 18u) ^ old) >> 27u);
        auto rot = static_cast<std::uint32_t>(old >> 59u);
        return (xorshifted >> rot) | (xorshifted << ((-rot) & 31u));
    }

    std::uint64_t next_u64() { return (static_cast<std::uint64_t>(next_u32()) << 32) | next_u32(); }

    // Uniform in [0, 1).
    double next_double() { return static_cast<double>(next_u32() >> 5) * (1.0 / 134217728.0); }
    float next_float() { return static_cast<float>(next_u32() >> 8) * (1.0f / 16777216.0f); }

    // Uniform integer in [0, bound). Unbiased.
    std::uint32_t next_below(std::uint32_t bound) {
        if (bound == 0) return 0;
        std::uint32_t threshold = (-bound) % bound;
        for (;;) {
            std::uint32_t r = next_u32();
            if (r >= threshold) return r % bound;
        }
    }

    double range(double lo, double hi) { return lo + (hi - lo) * next_double(); }
};

}  // namespace pocket
