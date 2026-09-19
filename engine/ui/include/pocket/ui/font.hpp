// Glyph rasterization and text measurement on FreeType.
//
// One Font owns one face and one R8 atlas texture shared by every size. Glyphs are rasterized
// on demand at the requested pixel size (already multiplied by the display scale) and packed
// into shelves; the atlas grows in place until it is full, then a larger one replaces it.
#pragma once

#include <pocket/core/result.hpp>
#include <pocket/rhi/device.hpp>

#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::ui {

struct Glyph {
    float u0 = 0, v0 = 0, u1 = 0, v1 = 0;  // atlas uv
    float width = 0, height = 0;            // bitmap size in pixels
    float bearing_x = 0, bearing_y = 0;     // from the pen position to the bitmap's top-left
    float advance = 0;                      // pen advance in pixels
    bool empty = true;                      // whitespace or unrasterizable
};

struct ShapedGlyph {
    std::uint32_t codepoint = 0;
    std::uint32_t glyph_index = 0;  // glyph id in the font after shaping (ligatures, contextual forms)
    Glyph glyph;
    float x = 0;   // pen offset within the run
    float y = 0;   // vertical offset (marks), screen down
    std::size_t byte_offset = 0;  // start of the cluster in the source string
};

struct TextMetrics {
    float width = 0;
    float ascent = 0;    // above the baseline
    float descent = 0;   // below the baseline (positive)
    float line_height = 0;
};

// Decode one UTF-8 codepoint; advances `i`. Invalid bytes decode as U+FFFD.
std::uint32_t decode_utf8(std::string_view s, std::size_t& i);

class Font {
   public:
    static Result<std::unique_ptr<Font>> load(rhi::Device& device, const std::string& path);
    ~Font();
    Font(const Font&) = delete;
    Font& operator=(const Font&) = delete;

    // Metrics for a pixel size.
    [[nodiscard]] TextMetrics metrics(float px_size);
    // Glyph for a codepoint at a pixel size, rasterized and uploaded if needed.
    const Glyph& glyph(std::uint32_t codepoint, float px_size);
    // Glyph by font glyph index (what shaping produces).
    const Glyph& glyph_by_index(std::uint32_t index, float px_size);
    // Shape a run of text on one line with HarfBuzz (OpenType: contextual forms, ligatures,
    // kerning, marks; right-to-left runs come out in visual order); positions are in pixels.
    std::vector<ShapedGlyph> shape(std::string_view text, float px_size);
    [[nodiscard]] float measure(std::string_view text, float px_size);
    // Upload pending atlas changes; returns the texture view to bind.
    WGPUTextureView atlas_view();
    [[nodiscard]] std::uint32_t atlas_size() const;
    [[nodiscard]] std::size_t glyph_count() const;
    [[nodiscard]] const std::string& family() const;

   private:
    Font();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::ui
