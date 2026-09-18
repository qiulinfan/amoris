// Hashing for state fingerprints and content ids.
//
// StateHasher folds values in a fixed order into a 64-bit FNV-1a style digest. Floats are hashed
// by their IEEE bits, so two states hash equal only when they are bit-identical, which is the
// contract for deterministic replay.
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

    void bytes(const void* data, std::size_t n) {
        auto* p = static_cast<const unsigned char*>(data);
        for (std::size_t i = 0; i < n; ++i) {
            h ^= p[i];
            h *= kFnvPrime;
        }
    }
    void u8(std::uint8_t v) { bytes(&v, 1); }
    void u32(std::uint32_t v) { bytes(&v, 4); }
    void u64(std::uint64_t v) { bytes(&v, 8); }
    void i64(std::int64_t v) { bytes(&v, 8); }
    void f32(float v) { u32(std::bit_cast<std::uint32_t>(v)); }
    void f64(double v) { u64(std::bit_cast<std::uint64_t>(v)); }
    void str(std::string_view s) {
        u64(s.size());
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
