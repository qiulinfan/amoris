// 2D batch painter.
//
// Coordinates are logical points (window points); the painter multiplies by the display scale
// so retina displays get crisp geometry and glyphs. Draws are batched into one vertex buffer and
// split only when the scissor rectangle changes.
#pragma once

#include <pocket/core/result.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/ui/font.hpp>

#include <memory>
#include <string_view>
#include <vector>

namespace pocket::ui {

struct Color {
    float r = 0, g = 0, b = 0, a = 1;
    static Color rgba8(std::uint8_t r, std::uint8_t g, std::uint8_t b, std::uint8_t a = 255) { return {r / 255.0f, g / 255.0f, b / 255.0f, a / 255.0f}; }
    [[nodiscard]] Color with_alpha(float alpha) const { return {r, g, b, a * alpha}; }
};

struct Rect {
    float x = 0, y = 0, w = 0, h = 0;
    [[nodiscard]] bool contains(float px, float py) const { return px >= x && py >= y && px < x + w && py < y + h; }
    [[nodiscard]] Rect intersect(const Rect& o) const {
        float x0 = std::max(x, o.x), y0 = std::max(y, o.y);
        float x1 = std::min(x + w, o.x + o.w), y1 = std::min(y + h, o.y + o.h);
        return {x0, y0, std::max(0.0f, x1 - x0), std::max(0.0f, y1 - y0)};
    }
};

enum class TextAlign { Left, Center, Right };

class Painter {
   public:
    static Result<std::unique_ptr<Painter>> create(rhi::Device& device, Font& font);
    ~Painter();
    Painter(const Painter&) = delete;
    Painter& operator=(const Painter&) = delete;

    // Start a frame: logical size in points and the pixel scale.
    void begin(float width_points, float height_points, float scale);
    void rect(const Rect& r, Color color, float radius = 0);
    void border(const Rect& r, Color color, float thickness, float radius = 0);
    // Draw an image: a texture view (RGBA, sampled linearly, or by the nearest texel with `nearest`
    // for pixel art) stretched over the rect, u0..u1 and v0..v1 (0..1, v down) picking the part of
    // it shown, tinted by color, corners rounded by radius.
    void image(const Rect& r, WGPUTextureView view, float u0, float v0, float u1, float v1, Color tint, float radius = 0, bool nearest = false);
    // Draw an image in nine slices: the part of the texture from (u0, v0) to (u1, v1), `pw` by
    // `ph` pixels, split `left`, `top`, `right` and `bottom` pixels from its edges; the corners
    // keep their pixel size (one point each), the edges stretch along, the middle fills the rest.
    void image_sliced(const Rect& r, WGPUTextureView view, float u0, float v0, float u1, float v1, float pw, float ph, float left, float top, float right, float bottom, Color tint, bool nearest = false);
    // Draw one line of text with its baseline placed so the text box starts at (x, y).
    // Returns the advance width in points.
    float text(float x, float y, std::string_view text, float size_points, Color color);
    float text_aligned(const Rect& box, std::string_view text, float size_points, Color color, TextAlign align, bool vcenter = true);
    [[nodiscard]] float measure(std::string_view text, float size_points);
    [[nodiscard]] float line_height(float size_points);
    void push_clip(const Rect& r);
    void pop_clip();
    [[nodiscard]] Rect current_clip() const;
    // Draw a solid-color line (thin quad).
    void line(float x0, float y0, float x1, float y1, Color color, float thickness = 1);
    // Submit everything drawn since begin() over the frame's color target.
    Status flush(rhi::Frame& frame);
    [[nodiscard]] std::uint32_t vertex_count() const;
    [[nodiscard]] std::uint32_t draw_count() const;
    [[nodiscard]] float scale() const;

   private:
    Painter();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::ui
