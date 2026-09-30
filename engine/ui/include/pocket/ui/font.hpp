// Glyph rasterization and text measurement on FreeType.
//
// One Font owns one face and one R8 atlas texture shared by every size. Glyphs are rasterized
// on demand at the requested pixel size (already multiplied by the display scale) and packed
// into shelves; the atlas grows in place until it is full, then a larger one replaces it.
#pragma once

#include <pocket/core/result.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/ui/bidi.hpp>

#include <cstdint>
#include <memory>
#include <string>
#include <string_view>
#include <utility>
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
    std::uint32_t glyph_index = 0;  // glyph id in the face that drew it: this font or a fallback
    Glyph glyph;                    // already rasterized from that face
    float x = 0;   // pen offset within the run
    float y = 0;   // vertical offset (marks), screen down
    float advance = 0;            // the pen's advance after it, as shaped (kerning included)
    std::size_t byte_offset = 0;  // start of the cluster in the source string
    bool rtl = false;             // in a right-to-left run
};

// One line laid out for editing (docs/design/pocket-ui.md, Right-to-left text): its characters'
// clusters in visual order with their extents, in pixels from the line's left end. A caret
// between two characters sits on the leading edge of the one after it (its left in a
// left-to-right run, its right in a right-to-left one), so it follows the glyphs in mixed text;
// between runs of the two directions it takes the side of the character going the paragraph's
// way, and at the line's ends a character going the other way leaves it at the paragraph's side.
struct LineLayout {
    struct Cluster {
        std::size_t start = 0, end = 0;   // the bytes it draws
        float x0 = 0, x1 = 0;             // its extent
        bool rtl = false;
    };
    std::vector<Cluster> clusters;        // left to right
    std::vector<std::size_t> stops;       // every character's start, then the line's size
    static constexpr std::uint32_t kNone = 0xFFFFFFFFu;
    std::vector<std::uint32_t> owner;     // each character's cluster (kNone: drew nothing)
    float width = 0;
    bool rtl = false;                     // the paragraph's direction
    // Where a caret at a byte offset stands.
    [[nodiscard]] float caret_x(std::size_t offset) const;
    // The byte offset whose caret is nearest an x.
    [[nodiscard]] std::size_t offset_at(float x) const;
    // The extents covering the characters in [a, b), left to right (more than one in mixed text).
    [[nodiscard]] std::vector<std::pair<float, float>> spans(std::size_t a, std::size_t b) const;
    // Whether it holds runs of both directions.
    [[nodiscard]] bool mixed() const;
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
    // Glyph by glyph index in this font's own face (not a fallback's; shape() returns glyphs
    // already rasterized from whichever face drew them).
    const Glyph& glyph_by_index(std::uint32_t index, float px_size);
    // Shape a line of text with HarfBuzz (OpenType: contextual forms, ligatures, kerning, marks),
    // its runs in visual order for a paragraph going `dir`; positions are in pixels.
    std::vector<ShapedGlyph> shape(std::string_view text, float px_size, TextDirection dir = TextDirection::Auto);
    [[nodiscard]] float measure(std::string_view text, float px_size, TextDirection dir = TextDirection::Auto);
    // A line laid out for carets, selections and clicks.
    [[nodiscard]] LineLayout layout(std::string_view text, float px_size, TextDirection dir = TextDirection::Auto);
    // Upload pending atlas changes; returns the texture view to bind.
    WGPUTextureView atlas_view();
    [[nodiscard]] std::uint32_t atlas_size() const;
    [[nodiscard]] std::size_t glyph_count() const;
    [[nodiscard]] const std::string& family() const;
    // Another font for the characters this one lacks (docs/design/pocket-ui.md, Fallback fonts):
    // Arabic letters in an interface drawn with a Chinese face. Fallbacks are tried in the order
    // added.
    Status add_fallback(const std::string& path);
    [[nodiscard]] std::size_t fallback_count() const;

   private:
    const Glyph& glyph_in(int face, std::uint32_t index, float px_size);
    Font();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::ui
