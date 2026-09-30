// Mixed-direction text (docs/design/pocket-ui.md, Right-to-left text): the Unicode bidirectional
// algorithm (UAX #9) for one line of a paragraph, without explicit embeddings or isolates. A line
// becomes runs of one direction in the order they are seen, left to right; shaping lays each out
// in its direction.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string_view>
#include <vector>

namespace pocket::ui {

struct BidiRun {
    std::size_t start = 0, end = 0;   // bytes of the line, logical order within the run
    int level = 0;                    // odd: right to left
    [[nodiscard]] bool rtl() const { return (level & 1) != 0; }
};

// A paragraph's direction: given (an element's `dir` style), or found from the text.
enum class TextDirection : std::uint8_t { Auto, Ltr, Rtl };

// Whether a paragraph's direction is right to left: its first strong letter is Hebrew or Arabic
// (or another right-to-left script); a paragraph with none is left to right.
bool rtl_line(std::string_view text);
// Whether a line of a paragraph going `dir` is right to left: Auto finds it from the line itself.
inline bool rtl_line(std::string_view text, TextDirection dir) { return dir == TextDirection::Auto ? rtl_line(text) : dir == TextDirection::Rtl; }
// The line's directional runs in visual order, left to right, in a paragraph going `dir` (a
// wrapped paragraph's lines all take the paragraph's; Auto: the line is the paragraph).
std::vector<BidiRun> bidi_runs(std::string_view text, TextDirection dir = TextDirection::Auto);

}  // namespace pocket::ui
