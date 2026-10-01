// Hashing for state fingerprints and content ids.
//
// StateHasher folds values in a fixed order into a 64-bit digest. Floats are hashed by their IEEE
// bits, so two states hash equal only when they are bit-identical, which is the contract for
// deterministic replay. A value is one mixing step (a string its length, then a step per eight
// bytes): the world is hashed every tick, and a step per byte cost a millisecond a tick at three
// thousand entities. Only 64-bit integer operations, so native and web builds agree.
#pragma once

#include <bit>
#include <cstdint>
#include <cstring>
#include <string_view>

namespace pocket {

constexpr std::uint64_t kFnvOffset = 14695981039346656037ULL;
constexpr std::uint64_t kFnvPrime = 1099511628211ULL;

constexpr std::uint64_t fnv1a(std::string_view s, std::uint64_t h = kFnvOffset) {
    for (unsigned char c : s) {
        h ^= c;
        h *= kFnvPrime;
    }
    return h;
}

struct StateHasher {
    std::uint64_t h = kFnvOffset;

    // In, multiplied by an odd constant, and the high half folded down so the next step spreads it.
    void mix(std::uint64_t v) {
        h = (h ^ v) * 0x9e3779b97f4a7c15ULL;
        h ^= h >> 29;
    }
    void bytes(const void* data, std::size_t n) {
        auto* p = static_cast<const unsigned char*>(data);
        for (; n >= 8; p += 8, n -= 8) {
            std::uint64_t w;
            std::memcpy(&w, p, 8);   // little-endian on every target (arm64, x86-64, wasm)
            mix(w);
        }
        if (n) {
            std::uint64_t w = 0;
            std::memcpy(&w, p, n);
            mix(w ^ (static_cast<std::uint64_t>(n) << 56));
        }
    }
    void u8(std::uint8_t v) { mix(v); }
    void u32(std::uint32_t v) { mix(v); }
    void u64(std::uint64_t v) { mix(v); }
    void i64(std::int64_t v) { mix(static_cast<std::uint64_t>(v)); }
    void f32(float v) { mix(std::bit_cast<std::uint32_t>(v)); }
    void f64(double v) { mix(std::bit_cast<std::uint64_t>(v)); }
    void str(std::string_view s) {
        mix(s.size());
        bytes(s.data(), s.size());
    }
    [[nodiscard]] std::uint64_t digest() const { return h; }
};

// Hex string of a 64-bit digest, 16 lowercase characters.
inline std::string hex64(std::uint64_t v) {
    static constexpr char digits[] = "0123456789abcdef";
    std::string out(16, '0');
    for (int i = 15; i >= 0; --i) {
        out[static_cast<std::size_t>(i)] = digits[v & 0xf];
        v >>= 4;
    }
    return out;
}

}  // namespace pocket
