// Animated GIFs of play (docs/mcp.md, capture.gif): frames made smaller by box filtering, one
// palette of 256 colours for all of them by median cut, each pixel its nearest colour, and the
// GIF89a stream with LZW-compressed frames that loops for ever.
#include "gif.hpp"

#include <algorithm>
#include <array>
#include <cstdint>
#include <unordered_map>
#include <string>
#include <vector>

namespace pocket::app {

namespace {

struct Box {
    std::vector<std::uint32_t> colours;   // 0xRRGGBB
};

// The palette by median cut: the box with the widest channel is split at its median until there
// are `want` boxes; each gives its mean.
std::vector<std::array<std::uint8_t, 3>> median_cut(std::vector<std::uint32_t> colours, std::size_t want) {
    std::vector<Box> boxes{{std::move(colours)}};
    auto channel = [](std::uint32_t c, int k) { return static_cast<int>((c >> (16 - 8 * k)) & 0xFF); };
    while (boxes.size() < want) {
        std::size_t pick = boxes.size();
        int widest = -1, along = 0;
        for (std::size_t b = 0; b < boxes.size(); ++b) {
            if (boxes[b].colours.size() < 2) continue;
            for (int k = 0; k < 3; ++k) {
                int lo = 255, hi = 0;
                for (std::uint32_t c : boxes[b].colours) { lo = std::min(lo, channel(c, k)); hi = std::max(hi, channel(c, k)); }
                if (hi - lo > widest) { widest = hi - lo; pick = b; along = k; }
            }
        }
        if (pick == boxes.size() || widest <= 0) break;
        auto& v = boxes[pick].colours;
        std::sort(v.begin(), v.end(), [&](std::uint32_t a, std::uint32_t b) { return channel(a, along) < channel(b, along); });
        Box upper{std::vector<std::uint32_t>(v.begin() + static_cast<std::ptrdiff_t>(v.size() / 2), v.end())};
        v.resize(v.size() / 2);
        boxes.push_back(std::move(upper));
    }
    std::vector<std::array<std::uint8_t, 3>> palette;
    for (const Box& b : boxes) {
        if (b.colours.empty()) continue;
        std::uint64_t s[3]{};
        for (std::uint32_t c : b.colours) for (int k = 0; k < 3; ++k) s[k] += static_cast<std::uint64_t>(channel(c, k));
        const auto n = static_cast<std::uint64_t>(b.colours.size());
        palette.push_back({static_cast<std::uint8_t>(s[0] / n), static_cast<std::uint8_t>(s[1] / n), static_cast<std::uint8_t>(s[2] / n)});
    }
    while (palette.size() < 256) palette.push_back({0, 0, 0});
    return palette;
}

// GIF's LZW: codes of growing width (from min + 1 bits to 12), a clear when the table is full.
void lzw(const std::vector<std::uint8_t>& indices, std::string& out) {
    constexpr int kMin = 8;
    const int clear = 1 << kMin, end = clear + 1;
    std::string block;
    std::uint32_t bits = 0;
    int nbits = 0, width = kMin + 1, next = end + 1;
    auto emit = [&](int code) {
        bits |= static_cast<std::uint32_t>(code) << nbits;
        nbits += width;
        while (nbits >= 8) {
            block.push_back(static_cast<char>(bits & 0xFF));
            bits >>= 8;
            nbits -= 8;
            if (block.size() == 255) { out.push_back(static_cast<char>(255)); out += block; block.clear(); }
        }
    };
    std::unordered_map<std::uint32_t, int> table;   // (prefix << 8 | byte) -> code
    table.reserve(8192);
    out.push_back(static_cast<char>(kMin));
    emit(clear);
    int prefix = -1;
    for (const std::uint8_t c : indices) {
        if (prefix < 0) { prefix = c; continue; }
        const std::uint32_t key = (static_cast<std::uint32_t>(prefix) << 8) | c;
        if (const auto it = table.find(key); it != table.end()) { prefix = it->second; continue; }
        emit(prefix);
        if (next < 4096) {
            table[key] = next++;
            if (next > (1 << width) && width < 12) ++width;
        } else {
            emit(clear);
            table.clear();
            next = end + 1;
            width = kMin + 1;
        }
        prefix = c;
    }
    if (prefix >= 0) emit(prefix);
    emit(end);
    if (nbits > 0) block.push_back(static_cast<char>(bits & 0xFF));
    if (!block.empty()) { out.push_back(static_cast<char>(block.size())); out += block; }
    out.push_back('\0');
}

}  // namespace

std::vector<std::uint8_t> shrink_rgba(const std::vector<std::uint8_t>& rgba, int w, int h, int to_w, int to_h) {
    std::vector<std::uint8_t> out(static_cast<std::size_t>(to_w) * static_cast<std::size_t>(to_h) * 3);
    for (int y = 0; y < to_h; ++y)
        for (int x = 0; x < to_w; ++x) {
            const int x0 = x * w / to_w, x1 = std::max(x0 + 1, (x + 1) * w / to_w), y0 = y * h / to_h, y1 = std::max(y0 + 1, (y + 1) * h / to_h);
            std::uint32_t s[3]{}, n = 0;
            for (int yy = y0; yy < y1; ++yy)
                for (int xx = x0; xx < x1; ++xx) {
                    const std::size_t at = (static_cast<std::size_t>(yy) * static_cast<std::size_t>(w) + static_cast<std::size_t>(xx)) * 4;
                    for (int k = 0; k < 3; ++k) s[k] += rgba[at + static_cast<std::size_t>(k)];
                    ++n;
                }
            const std::size_t o = (static_cast<std::size_t>(y) * static_cast<std::size_t>(to_w) + static_cast<std::size_t>(x)) * 3;
            for (int k = 0; k < 3; ++k) out[o + static_cast<std::size_t>(k)] = static_cast<std::uint8_t>(s[k] / std::max(n, 1u));
        }
    return out;
}

std::string encode_gif(const std::vector<std::vector<std::uint8_t>>& frames, int w, int h, int delay_cs) {
    // One palette for every frame, from a sample of their pixels.
    std::vector<std::uint32_t> sample;
    const std::size_t pixels = static_cast<std::size_t>(w) * static_cast<std::size_t>(h);
    const std::size_t stride = std::max<std::size_t>(1, pixels * frames.size() / 60000);
    std::size_t seen = 0;
    for (const auto& f : frames)
        for (std::size_t i = 0; i < pixels; ++i, ++seen)
            if (seen % stride == 0) sample.push_back((static_cast<std::uint32_t>(f[i * 3]) << 16) | (static_cast<std::uint32_t>(f[i * 3 + 1]) << 8) | f[i * 3 + 2]);
    const auto palette = median_cut(std::move(sample), 256);
    // Nearest colour, remembered for each 5-bit-a-channel cell of colour.
    std::vector<int> cache(32 * 32 * 32, -1);
    auto nearest = [&](std::uint8_t r, std::uint8_t g, std::uint8_t b) {
        const std::size_t key = (static_cast<std::size_t>(r >> 3) << 10) | (static_cast<std::size_t>(g >> 3) << 5) | (b >> 3);
        if (cache[key] >= 0) return static_cast<std::uint8_t>(cache[key]);
        int best = 0, best_d = 1 << 30;
        for (int k = 0; k < 256; ++k) {
            const int dr = r - palette[static_cast<std::size_t>(k)][0], dg = g - palette[static_cast<std::size_t>(k)][1], db = b - palette[static_cast<std::size_t>(k)][2];
            const int d = 2 * dr * dr + 4 * dg * dg + 3 * db * db;
            if (d < best_d) { best_d = d; best = k; }
        }
        cache[key] = best;
        return static_cast<std::uint8_t>(best);
    };
    std::string out = "GIF89a";
    auto u16 = [&](int v) { out.push_back(static_cast<char>(v & 0xFF)); out.push_back(static_cast<char>((v >> 8) & 0xFF)); };
    u16(w);
    u16(h);
    out.push_back(static_cast<char>(0xF7));   // a global table of 256 colours
    out.push_back('\0');
    out.push_back('\0');
    for (const auto& c : palette) for (int k = 0; k < 3; ++k) out.push_back(static_cast<char>(c[static_cast<std::size_t>(k)]));
    out += std::string("\x21\xFF\x0BNETSCAPE2.0\x03\x01\x00\x00\x00", 19);   // loop for ever
    std::vector<std::uint8_t> indices(pixels);
    for (const auto& f : frames) {
        out += std::string("\x21\xF9\x04\x00", 4);   // graphic control: no disposal, no transparency
        u16(delay_cs);
        out += std::string("\x00\x00", 2);
        out.push_back(',');
        u16(0);
        u16(0);
        u16(w);
        u16(h);
        out.push_back('\0');
        for (std::size_t i = 0; i < pixels; ++i) indices[i] = nearest(f[i * 3], f[i * 3 + 1], f[i * 3 + 2]);
        lzw(indices, out);
    }
    out.push_back(';');
    return out;
}

}  // namespace pocket::app
