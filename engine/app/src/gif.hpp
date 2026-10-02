#pragma once
// Animated GIFs of play (docs/mcp.md, capture.gif).

#include <cstdint>
#include <string>
#include <vector>

namespace pocket::app {

// An RGBA frame of w by h box-filtered down to to_w by to_h, as RGB.
std::vector<std::uint8_t> shrink_rgba(const std::vector<std::uint8_t>& rgba, int w, int h, int to_w, int to_h);
// RGB frames of w by h as one looping GIF89a, each shown delay_cs hundredths of a second.
std::string encode_gif(const std::vector<std::vector<std::uint8_t>>& frames, int w, int h, int delay_cs);

}  // namespace pocket::app
