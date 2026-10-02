// Images made by the engine (docs/design/assets.md, Patterns): "pattern:<name>?<settings>" names a
// texture that wraps at its edges (bricks, tiles, planks, a prototyping grid, noise, ground and
// metal), drawn here from its settings, with `map=normal` its normal map and `map=height` its
// heights, so a level built from boxes can be dressed without a single file.
#include <pocket/assets/assets.hpp>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdint>
#include <functional>
#include <map>
#include <string>
#include <vector>

namespace pocket::assets {

namespace {

struct Rgb {
    float r = 0, g = 0, b = 0;   // sRGB, 0..1
};
Rgb operator*(Rgb a, float k) { return {a.r * k, a.g * k, a.b * k}; }
Rgb operator+(Rgb a, Rgb b) { return {a.r + b.r, a.g + b.g, a.b + b.b}; }
Rgb mix(Rgb a, Rgb b, float t) { return a * (1 - t) + b * t; }

// "#rgb", "#rrggbb" or a name.
bool read_colour(std::string v, Rgb& out) {
    std::transform(v.begin(), v.end(), v.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    static const std::map<std::string, std::string> names = {
        {"red", "#c83c32"}, {"green", "#3c9a46"}, {"blue", "#3264c8"}, {"yellow", "#e6c83c"}, {"orange", "#e68232"}, {"purple", "#7846a0"},
        {"pink", "#e68caa"}, {"brown", "#785032"}, {"black", "#202020"}, {"white", "#ebebeb"}, {"grey", "#808080"}, {"gray", "#808080"},
        {"tan", "#d2aa82"}, {"navy", "#283c6e"}, {"teal", "#329696"}, {"olive", "#6e7837"}, {"sand", "#d9c28f"}, {"stone", "#8c8a84"},
        {"wood", "#9a6b3f"}, {"brick", "#a0522d"}, {"concrete", "#9a9a96"}, {"grass", "#4f7d35"}, {"dirt", "#6b4f35"}, {"steel", "#a7abb0"}};
    if (const auto it = names.find(v); it != names.end()) v = it->second;
    if (v.starts_with("#")) v = v.substr(1);
    if (v.size() == 3) v = std::string{v[0], v[0], v[1], v[1], v[2], v[2]};
    if (v.size() != 6 || !std::all_of(v.begin(), v.end(), [](unsigned char c) { return std::isxdigit(c) != 0; })) return false;
    for (int k = 0; k < 3; ++k) (&out.r)[k] = static_cast<float>(std::stoi(v.substr(static_cast<std::size_t>(k) * 2, 2), nullptr, 16)) / 255.0f;
    return true;
}

std::uint32_t hash(std::int32_t x, std::int32_t y, std::uint32_t seed) {
    std::uint32_t h = static_cast<std::uint32_t>(x) * 0x8da6b343u ^ static_cast<std::uint32_t>(y) * 0xd8163841u ^ seed * 0xcb1ab31fu;
    h ^= h >> 15;
    h *= 0x2c1b3c6du;
    h ^= h >> 12;
    h *= 0x297a2d39u;
    h ^= h >> 15;
    return h;
}
float unit(std::int32_t x, std::int32_t y, std::uint32_t seed) { return static_cast<float>(hash(x, y, seed) & 0xFFFFFFu) / 16777216.0f; }

// Value noise on a lattice of `period` cells that wraps, at (u, v) in 0..1: smooth, 0..1.
float wrap_noise(float u, float v, int period, std::uint32_t seed) {
    const float x = u * static_cast<float>(period), y = v * static_cast<float>(period);
    const int x0 = static_cast<int>(std::floor(x)), y0 = static_cast<int>(std::floor(y));
    const float fx = x - static_cast<float>(x0), fy = y - static_cast<float>(y0);
    auto at = [&](int i, int j) { return unit(((i % period) + period) % period, ((j % period) + period) % period, seed); };
    auto fade = [](float t) { return t * t * t * (t * (t * 6 - 15) + 10); };
    const float a = at(x0, y0), b = at(x0 + 1, y0), c = at(x0, y0 + 1), d = at(x0 + 1, y0 + 1);
    const float sx = fade(fx), sy = fade(fy);
    return (a + (b - a) * sx) + ((c + (d - c) * sx) - (a + (b - a) * sx)) * sy;
}

// Octaves of it, each twice as fine and half as strong: 0..1.
float fbm(float u, float v, int period, int octaves, std::uint32_t seed) {
    float sum = 0, amp = 0.5f, total = 0;
    for (int o = 0; o < octaves; ++o) {
        sum += wrap_noise(u, v, period << o, seed + static_cast<std::uint32_t>(o) * 977u) * amp;
        total += amp;
        amp *= 0.5f;
    }
    return sum / total;
}

float smooth(float a, float b, float x) {
    const float t = std::clamp((x - a) / (b - a), 0.0f, 1.0f);
    return t * t * (3 - 2 * t);
}
float fract(float x) { return x - std::floor(x); }

struct Settings {
    std::map<std::string, std::string> values;
    std::string error;
    std::string text(const std::string& k, const std::string& d) const {
        const auto it = values.find(k);
        return it == values.end() ? d : it->second;
    }
    float number(const std::string& k, float d) {
        const auto it = values.find(k);
        if (it == values.end()) return d;
        char* end = nullptr;
        const float v = std::strtof(it->second.c_str(), &end);
        if (end == it->second.c_str() || *end != '\0') {
            if (error.empty()) error = k + "=" + it->second + " is not a number";
            return d;
        }
        return v;
    }
    Rgb colour(const std::string& k, const std::string& d) {
        Rgb c;
        const std::string v = text(k, d);
        if (!read_colour(v, c)) {
            if (error.empty()) error = k + "=" + v + " is not a colour (\"#rrggbb\", \"#rgb\" or a name such as red, tan, stone, wood)";
            (void)read_colour(d, c);
        }
        return c;
    }
};

struct Texel {
    Rgb colour;
    float height = 0.5f;   // 0..1, for the normal map
};

using Draw = std::function<Texel(float u, float v)>;

}  // namespace

Result<Image> pattern_image(const std::string& spec) {
    // "pattern:<name>?<k=v>&...".
    std::string body = spec.starts_with("pattern:") ? spec.substr(8) : spec;
    const std::size_t q = body.find('?');
    const std::string name = body.substr(0, q);
    Settings s;
    if (q != std::string::npos) {
        std::string rest = body.substr(q + 1);
        std::size_t at = 0;
        while (at <= rest.size()) {
            const std::size_t amp = rest.find('&', at);
            const std::string pair = rest.substr(at, amp == std::string::npos ? std::string::npos : amp - at);
            if (!pair.empty()) {
                const std::size_t eq = pair.find('=');
                s.values[pair.substr(0, eq)] = eq == std::string::npos ? std::string() : pair.substr(eq + 1);
            }
            if (amp == std::string::npos) break;
            at = amp + 1;
        }
    }
    const int size = std::clamp(static_cast<int>(s.number("size", 512)), 16, 2048);
    const auto seed = static_cast<std::uint32_t>(s.number("seed", 1));
    const float vary = std::clamp(s.number("vary", 0.12f), 0.0f, 1.0f);
    Draw draw;
    if (name == "checker") {
        const Rgb a = s.colour("a", "#d0d0d0"), b = s.colour("b", "#5a5a5a");
        const int n = std::max(1, static_cast<int>(s.number("count", 8)));
        draw = [=](float u, float v) {
            const bool odd = ((static_cast<int>(u * static_cast<float>(n)) + static_cast<int>(v * static_cast<float>(n))) & 1) != 0;
            return Texel{odd ? b : a, 0.5f};
        };
    } else if (name == "stripes") {
        const Rgb a = s.colour("a", "#d0d0d0"), b = s.colour("b", "#5a5a5a");
        const int n = std::max(1, static_cast<int>(s.number("count", 8)));
        draw = [=](float u, float) { return Texel{fract(u * static_cast<float>(n)) < 0.5f ? a : b, 0.5f}; };
    } else if (name == "grid") {
        // A prototyping grid: cells with a strong line every cell and faint lines between.
        const Rgb base = s.colour("color", "#7d8a96"), line = s.colour("line", "#2d3540");
        const int n = std::max(1, static_cast<int>(s.number("count", 4)));
        const int sub = std::max(1, static_cast<int>(s.number("sub", 4)));
        draw = [=](float u, float v) {
            auto edge = [](float x, float w) { const float f = fract(x); return std::max(1.0f - smooth(0.0f, w, f), smooth(1.0f - w, 1.0f, f)); };
            const float major = std::max(edge(u * static_cast<float>(n), 0.012f * static_cast<float>(n)), edge(v * static_cast<float>(n), 0.012f * static_cast<float>(n)));
            const float minor = std::max(edge(u * static_cast<float>(n * sub), 0.01f * static_cast<float>(n * sub)), edge(v * static_cast<float>(n * sub), 0.01f * static_cast<float>(n * sub))) * 0.35f;
            return Texel{mix(base, line, std::max(major, minor)), 0.5f};
        };
    } else if (name == "bricks" || name == "tiles") {
        // Rows of blocks with mortar between: bricks offset by half a block every other row,
        // tiles square and in line.
        const bool bricks = name == "bricks";
        const Rgb colour = s.colour("color", bricks ? "#a0522d" : "#d8d4cc"), mortar = s.colour(bricks ? "mortar" : "grout", bricks ? "#c8c0b0" : "#8a8580");
        const int rows = std::max(1, static_cast<int>(s.number(bricks ? "rows" : "count", bricks ? 8 : 4)));
        const int cols = std::max(1, static_cast<int>(s.number(bricks ? "columns" : "count", bricks ? 4 : 4)));
        const float gap = std::clamp(s.number("gap", bricks ? 0.08f : 0.04f), 0.0f, 0.45f);
        draw = [=](float u, float v) {
            const float y = v * static_cast<float>(rows);
            const int row = static_cast<int>(std::floor(y));
            const float shift = bricks && (row & 1) ? 0.5f : 0.0f;
            const float x = u * static_cast<float>(cols) + shift;
            const int col = static_cast<int>(std::floor(x)) % cols;
            const float fx = fract(x), fy = fract(y);
            // Distance to the block's edge in its own height (bricks are wider than tall).
            const float aspect = static_cast<float>(rows) / static_cast<float>(cols);
            const float ex = std::min(fx, 1 - fx) * aspect, ey = std::min(fy, 1 - fy);
            const float edge = std::min(ex, ey);
            const float inside = smooth(gap * 0.5f, gap * 0.5f + 0.04f, edge);
            const float tone = (unit(col, row, seed) - 0.5f) * 2 * vary;
            const float grain = (fbm(u, v, 16, 4, seed + 7) - 0.5f) * 0.25f;
            Rgb block = colour * (1 + tone + grain);
            Rgb gapc = mortar * (1 + (fbm(u, v, 32, 3, seed + 11) - 0.5f) * 0.3f);
            const float h = inside * (0.75f + grain * 0.4f + 0.25f * smooth(0.0f, 0.12f, edge - gap * 0.5f));
            return Texel{mix(gapc, block, inside), h};
        };
    } else if (name == "planks") {
        // Boards side by side along v, each cut across at its own place, with grain along it.
        const Rgb colour = s.colour("color", "#9a6b3f");
        const int n = std::max(1, static_cast<int>(s.number("count", 5)));
        draw = [=](float u, float v) {
            const float x = u * static_cast<float>(n);
            const int board = static_cast<int>(std::floor(x));
            const float fx = fract(x);
            const float cut = unit(board, 0, seed);
            const float along = fract(v + cut);
            const int piece = static_cast<int>(std::floor(v + cut)) + board * 7;
            const float seam = std::min({fx, 1 - fx, along * 4, (1 - along) * 4});
            const float inside = smooth(0.0f, 0.03f, seam);
            const float grain = fbm(u * 0.25f, v, 8, 5, seed + static_cast<std::uint32_t>(board) * 31u);
            const float rings = 0.5f + 0.5f * std::sin((grain * 14.0f + fx * 3.0f) * 3.14159f);
            const float tone = (unit(piece, 3, seed) - 0.5f) * 2 * vary;
            const Rgb wood = colour * (0.82f + 0.22f * rings + tone);
            return Texel{mix(colour * 0.35f, wood, inside), inside * (0.7f + 0.3f * rings)};
        };
    } else if (name == "noise" || name == "concrete" || name == "sand" || name == "dirt" || name == "rock" || name == "grass") {
        // Ground and walls: noise in a colour's shades, each with its own features.
        const char* def = name == "concrete" ? "#9a9a96" : name == "sand" ? "#d9c28f" : name == "dirt" ? "#6b4f35" : name == "rock" ? "#7a7670" : name == "grass" ? "#4f7d35" : "#808080";
        const Rgb a = s.colour(name == "noise" ? "a" : "color", def);
        const Rgb b = name == "noise" ? s.colour("b", "#303030") : a * 0.55f;
        const int scale = std::max(1, static_cast<int>(s.number("scale", name == "sand" ? 6 : name == "rock" ? 3 : 4)));
        const std::string kind = name;
        draw = [=](float u, float v) {
            float n = fbm(u, v, scale, 6, seed);
            float h = n;
            Rgb c = mix(b, a, std::clamp(0.35f + n * 0.9f, 0.0f, 1.0f));
            if (kind == "concrete") {
                const float pits = smooth(0.78f, 0.83f, wrap_noise(u, v, 96, seed + 5));
                c = mix(a * (0.9f + (n - 0.5f) * 0.25f), a * 0.55f, pits * 0.6f);
                h = 0.6f + (n - 0.5f) * 0.3f - pits * 0.3f;
            } else if (kind == "sand") {
                const float ripple = 0.5f + 0.5f * std::sin((v * 16.0f + (n - 0.5f) * 2.0f) * 2 * 3.14159f);
                const float grains = wrap_noise(u, v, 256, seed + 3);
                c = a * (0.9f + (n - 0.5f) * 0.18f + ripple * 0.035f + (grains - 0.5f) * 0.16f);
                h = ripple * 0.15f + n * 0.5f + grains * 0.35f;
            } else if (kind == "dirt") {
                const float pebble = smooth(0.72f, 0.76f, wrap_noise(u, v, 40, seed + 9));
                c = mix(a * (0.75f + n * 0.45f), Rgb{0.55f, 0.52f, 0.48f}, pebble * 0.7f);
                h = n * 0.6f + pebble * 0.4f;
            } else if (kind == "rock") {
                const float ridge = 1 - std::fabs(fbm(u, v, scale * 2, 4, seed + 13) * 2 - 1);
                const float crack = smooth(0.965f, 0.995f, ridge) * smooth(0.35f, 0.6f, fbm(u, v, scale, 3, seed + 23));
                const float grit = wrap_noise(u, v, 128, seed + 29);
                c = mix(a * (0.68f + n * 0.5f + (grit - 0.5f) * 0.12f), a * 0.5f, crack);
                h = n * 0.75f + grit * 0.1f + (1 - crack) * 0.15f;
            } else if (kind == "grass") {
                const float blades = wrap_noise(u * 4, v, 64, seed + 17);
                const float tint = fbm(u, v, 3, 3, seed + 19);
                c = mix(a * 0.6f, mix(a, Rgb{a.r * 1.25f, a.g * 1.15f, a.b * 0.8f}, tint), std::clamp(blades * 0.7f + n * 0.5f, 0.0f, 1.0f));
                h = blades * 0.6f + n * 0.4f;
            }
            return Texel{c, h};
        };
    } else if (name == "metal") {
        // Brushed metal: fine streaks along u, a little broad unevenness.
        const Rgb colour = s.colour("color", "#a7abb0");
        draw = [=](float u, float v) {
            const float streak = wrap_noise(u * 0.02f, v, 256, seed) * 0.6f + wrap_noise(u * 0.05f, v, 128, seed + 1) * 0.4f;
            const float broad = fbm(u, v, 4, 3, seed + 2);
            return Texel{colour * (0.86f + streak * 0.16f + (broad - 0.5f) * 0.1f), 0.5f + (streak - 0.5f) * 0.2f};
        };
    } else {
        return fail("bad_asset", "{}: no pattern '{}' (checker, stripes, grid, bricks, tiles, planks, noise, concrete, sand, dirt, rock, grass, metal)", spec, name);
    }
    const std::string map = s.text("map", "color");
    if (map != "color" && map != "normal" && map != "height") return fail("bad_asset", "{}: map={} (color, normal or height)", spec, map);
    const float bump = s.number("bump", 3.0f);
    if (!s.error.empty()) return fail("bad_asset", "{}: {}", spec, s.error);
    Image img;
    img.path = spec;
    img.width = static_cast<std::uint32_t>(size);
    img.height = static_cast<std::uint32_t>(size);
    img.rgba.resize(static_cast<std::size_t>(size) * static_cast<std::size_t>(size) * 4);
    std::vector<float> heights(static_cast<std::size_t>(size) * static_cast<std::size_t>(size));
    const float inv = 1.0f / static_cast<float>(size);
    for (int y = 0; y < size; ++y) {
        for (int x = 0; x < size; ++x) {
            const Texel t = draw((static_cast<float>(x) + 0.5f) * inv, (static_cast<float>(y) + 0.5f) * inv);
            const std::size_t i = static_cast<std::size_t>(y) * static_cast<std::size_t>(size) + static_cast<std::size_t>(x);
            heights[i] = t.height;
            auto byte = [](float c) { return static_cast<std::uint8_t>(std::lround(std::clamp(c, 0.0f, 1.0f) * 255.0f)); };
            img.rgba[i * 4 + 0] = byte(t.colour.r);
            img.rgba[i * 4 + 1] = byte(t.colour.g);
            img.rgba[i * 4 + 2] = byte(t.colour.b);
            img.rgba[i * 4 + 3] = 255;
        }
    }
    if (map != "color") {
        // The heights' slopes across the wrapping image, as glTF's normal maps hold them (+y up the image).
        const float k = bump * static_cast<float>(size) / 512.0f;
        for (int y = 0; y < size; ++y) {
            for (int x = 0; x < size; ++x) {
                auto h = [&](int i, int j) { return heights[static_cast<std::size_t>((j + size) % size) * static_cast<std::size_t>(size) + static_cast<std::size_t>((i + size) % size)]; };
                const std::size_t i = static_cast<std::size_t>(y) * static_cast<std::size_t>(size) + static_cast<std::size_t>(x);
                if (map == "height") {
                    const auto g = static_cast<std::uint8_t>(std::lround(std::clamp(heights[i], 0.0f, 1.0f) * 255.0f));
                    img.rgba[i * 4 + 0] = img.rgba[i * 4 + 1] = img.rgba[i * 4 + 2] = g;
                    continue;
                }
                const float dx = (h(x + 1, y) - h(x - 1, y)) * 0.5f * k, dy = (h(x, y + 1) - h(x, y - 1)) * 0.5f * k;
                float nx = -dx, ny = dy, nz = 1;
                const float len = std::sqrt(nx * nx + ny * ny + nz * nz);
                nx /= len; ny /= len; nz /= len;
                img.rgba[i * 4 + 0] = static_cast<std::uint8_t>(std::lround((nx * 0.5f + 0.5f) * 255.0f));
                img.rgba[i * 4 + 1] = static_cast<std::uint8_t>(std::lround((ny * 0.5f + 0.5f) * 255.0f));
                img.rgba[i * 4 + 2] = static_cast<std::uint8_t>(std::lround((nz * 0.5f + 0.5f) * 255.0f));
            }
        }
    }
    return img;
}

}  // namespace pocket::assets
