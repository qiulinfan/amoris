// Terrains (docs/design/terrain.md): height fields from images or noise, their meshes and their
// 16-bit PNGs.
#include <pocket/assets/assets.hpp>

#include <stb_image.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdlib>
#include <numbers>

// stb_image_write's deflate (compiled with its implementation in third_party/stb/src/stb_impl.c).
extern "C" unsigned char* stbi_zlib_compress(unsigned char* data, int data_len, int* out_len, int quality);

namespace pocket::assets {

float Terrain::at(int i, int j) const {
    if (n <= 0) return 0;
    i = std::clamp(i, 0, n - 1);
    j = std::clamp(j, 0, n - 1);
    return h[static_cast<std::size_t>(j) * static_cast<std::size_t>(n) + static_cast<std::size_t>(i)];
}

namespace {

// The cell and the place within it of local (x, z), clamped to the grid.
void locate(const Terrain& t, float x, float z, int& i, int& j, float& u, float& v) {
    const float gx = std::clamp((x + t.size_x * 0.5f) / t.cell_x(), 0.0f, static_cast<float>(t.n - 1));
    const float gz = std::clamp((z + t.size_z * 0.5f) / t.cell_z(), 0.0f, static_cast<float>(t.n - 1));
    i = std::min(static_cast<int>(gx), t.n - 2);
    j = std::min(static_cast<int>(gz), t.n - 2);
    u = gx - static_cast<float>(i);
    v = gz - static_cast<float>(j);
}

}  // namespace

float Terrain::sample(float x, float z) const {
    if (n < 2) return n == 1 ? h[0] : 0.0f;
    int i = 0, j = 0;
    float u = 0, v = 0;
    locate(*this, x, z, i, j, u, v);
    const float h00 = at(i, j), h10 = at(i + 1, j), h01 = at(i, j + 1), h11 = at(i + 1, j + 1);
    // The triangle of the cell the point is in: (00, 11, 10) below the diagonal, (00, 01, 11) above.
    if (u >= v) return h00 + (h10 - h00) * u + (h11 - h10) * v;
    return h00 + (h11 - h01) * u + (h01 - h00) * v;
}

std::array<float, 4> Terrain::paint_at(float x, float z) const { return grid_at(paint, x, z); }

std::array<float, 4> Terrain::grid_at(const std::vector<std::array<float, 4>>& grid, float x, float z) const {
    if (n < 2 || grid.size() != static_cast<std::size_t>(n) * static_cast<std::size_t>(n)) return {0, 0, 0, 0};
    int i = 0, j = 0;
    float u = 0, v = 0;
    locate(*this, x, z, i, j, u, v);
    auto p = [&](int a, int b) { return grid[static_cast<std::size_t>(b) * static_cast<std::size_t>(n) + static_cast<std::size_t>(a)]; };
    std::array<float, 4> out{};
    for (int c = 0; c < 4; ++c) {
        const float top = p(i, j)[c] * (1 - u) + p(i + 1, j)[c] * u;
        const float bottom = p(i, j + 1)[c] * (1 - u) + p(i + 1, j + 1)[c] * u;
        out[c] = top * (1 - v) + bottom * v;
    }
    return out;
}

Vec3 Terrain::normal(float x, float z) const {
    if (n < 2) return {0, 1, 0};
    int i = 0, j = 0;
    float u = 0, v = 0;
    locate(*this, x, z, i, j, u, v);
    const float h00 = at(i, j), h10 = at(i + 1, j), h01 = at(i, j + 1), h11 = at(i + 1, j + 1);
    const float cx = cell_x(), cz = cell_z();
    const float dx = u >= v ? (h10 - h00) / cx : (h11 - h01) / cx;   // rise per unit along x in that triangle
    const float dz = u >= v ? (h11 - h10) / cz : (h01 - h00) / cz;
    return normalize(Vec3{-dx, 1, -dz});
}

Result<Terrain> terrain_from_image(const std::string& bytes, const std::string& display_path, int n, Vec2 size, float height) {
    const auto* data = reinterpret_cast<const stbi_uc*>(bytes.data());
    const int len = static_cast<int>(bytes.size());
    int w = 0, hgt = 0, channels = 0;
    std::vector<float> grey;
    if (stbi_is_16_bit_from_memory(data, len)) {
        stbi_us* px = stbi_load_16_from_memory(data, len, &w, &hgt, &channels, 1);
        if (!px) return fail("bad_image", "{}: {}", display_path, stbi_failure_reason());
        grey.resize(static_cast<std::size_t>(w) * static_cast<std::size_t>(hgt));
        for (std::size_t k = 0; k < grey.size(); ++k) grey[k] = static_cast<float>(px[k]) / 65535.0f;
        stbi_image_free(px);
    } else {
        stbi_uc* px = stbi_load_from_memory(data, len, &w, &hgt, &channels, 1);
        if (!px) return fail("bad_image", "{}: {}", display_path, stbi_failure_reason());
        grey.resize(static_cast<std::size_t>(w) * static_cast<std::size_t>(hgt));
        for (std::size_t k = 0; k < grey.size(); ++k) grey[k] = static_cast<float>(px[k]) / 255.0f;
        stbi_image_free(px);
    }
    if (w < 2 || hgt < 2) return fail("bad_image", "{}: a heightmap needs at least 2 by 2 pixels", display_path);
    Terrain t;
    t.n = std::clamp(n, 2, 1025);
    t.size_x = size.x;
    t.size_z = size.y;
    t.height = height;
    t.h.resize(static_cast<std::size_t>(t.n) * static_cast<std::size_t>(t.n));
    // Bilinear from the image, its top row at -z.
    for (int j = 0; j < t.n; ++j) {
        const float py = static_cast<float>(j) / static_cast<float>(t.n - 1) * static_cast<float>(hgt - 1);
        const int y0 = std::min(static_cast<int>(py), hgt - 2);
        const float fy = py - static_cast<float>(y0);
        for (int i = 0; i < t.n; ++i) {
            const float px = static_cast<float>(i) / static_cast<float>(t.n - 1) * static_cast<float>(w - 1);
            const int x0 = std::min(static_cast<int>(px), w - 2);
            const float fx = px - static_cast<float>(x0);
            auto g = [&](int x, int y) { return grey[static_cast<std::size_t>(y) * static_cast<std::size_t>(w) + static_cast<std::size_t>(x)]; };
            const float top = g(x0, y0) * (1 - fx) + g(x0 + 1, y0) * fx;
            const float bottom = g(x0, y0 + 1) * (1 - fx) + g(x0 + 1, y0 + 1) * fx;
            t.h[static_cast<std::size_t>(j) * static_cast<std::size_t>(t.n) + static_cast<std::size_t>(i)] = (top * (1 - fy) + bottom * fy) * height;
        }
    }
    return t;
}

namespace {

// Gradient noise on the integer lattice: a gradient per lattice point from a hash of its
// coordinates and the seed, faded between them (a quintic ease); about -0.7..0.7.
float gradient_noise(float x, float y, std::uint32_t seed) {
    auto hash = [seed](int ix, int iy) {
        std::uint32_t h = static_cast<std::uint32_t>(ix) * 0x8da6b343u ^ static_cast<std::uint32_t>(iy) * 0xd8163841u ^ seed * 0xcb1ab31fu;
        h ^= h >> 15;
        h *= 0x2c1b3c6du;
        h ^= h >> 12;
        h *= 0x297a2d39u;
        h ^= h >> 15;
        return h;
    };
    auto grad = [&](int ix, int iy, float dx, float dy) {
        const float a = static_cast<float>(hash(ix, iy) & 0xFFFFu) / 65536.0f * 2.0f * std::numbers::pi_v<float>;
        return std::cos(a) * dx + std::sin(a) * dy;
    };
    const int x0 = static_cast<int>(std::floor(x)), y0 = static_cast<int>(std::floor(y));
    const float fx = x - static_cast<float>(x0), fy = y - static_cast<float>(y0);
    auto fade = [](float t) { return t * t * t * (t * (t * 6 - 15) + 10); };
    const float u = fade(fx), v = fade(fy);
    const float n00 = grad(x0, y0, fx, fy), n10 = grad(x0 + 1, y0, fx - 1, fy);
    const float n01 = grad(x0, y0 + 1, fx, fy - 1), n11 = grad(x0 + 1, y0 + 1, fx - 1, fy - 1);
    const float a = n00 + (n10 - n00) * u, b = n01 + (n11 - n01) * u;
    return a + (b - a) * v;
}

}  // namespace

Terrain terrain_from_noise(std::uint32_t seed, float scale, int octaves, int n, Vec2 size, float height, float island) {
    Terrain t;
    t.n = std::clamp(n, 2, 1025);
    t.size_x = size.x;
    t.size_z = size.y;
    t.height = height;
    t.h.resize(static_cast<std::size_t>(t.n) * static_cast<std::size_t>(t.n));
    octaves = std::clamp(octaves, 1, 10);
    scale = std::max(scale, 1e-3f);
    float lo = 1e30f, hi = -1e30f;
    for (int j = 0; j < t.n; ++j) {
        for (int i = 0; i < t.n; ++i) {
            const float x = -t.size_x * 0.5f + static_cast<float>(i) * t.cell_x();
            const float z = -t.size_z * 0.5f + static_cast<float>(j) * t.cell_z();
            float sum = 0, amp = 1, freq = 1.0f / scale;
            for (int o = 0; o < octaves; ++o) {
                sum += gradient_noise(x * freq, z * freq, seed + static_cast<std::uint32_t>(o) * 1013u) * amp;
                amp *= 0.5f;
                freq *= 2.0f;
            }
            t.h[static_cast<std::size_t>(j) * static_cast<std::size_t>(t.n) + static_cast<std::size_t>(i)] = sum;
            lo = std::min(lo, sum);
            hi = std::max(hi, sum);
        }
    }
    // Spread over 0..height: the lowest sample at 0, the highest at the top.
    const float span = hi > lo ? hi - lo : 1.0f;
    for (float& v : t.h) v = (v - lo) / span * height;
    // An island: the ground falls from halfway out (by distance from the middle, in the extent's
    // own proportions) to the edges, so under water it ends in a shore all round.
    island = std::clamp(island, 0.0f, 1.0f);
    if (island > 0) {
        for (int j = 0; j < t.n; ++j) {
            for (int i = 0; i < t.n; ++i) {
                const float u = static_cast<float>(i) / static_cast<float>(t.n - 1) * 2 - 1, w = static_cast<float>(j) / static_cast<float>(t.n - 1) * 2 - 1;
                const float r = std::sqrt(u * u + w * w);
                const float k = std::clamp((r - 0.45f) / 0.5f, 0.0f, 1.0f);
                t.h[static_cast<std::size_t>(j) * static_cast<std::size_t>(t.n) + static_cast<std::size_t>(i)] *= 1.0f - island * k * k * (3 - 2 * k);
            }
        }
    }
    return t;
}

namespace {

// The vertex normal at sample (i, j) from its neighbours (central differences, one-sided at the edges).
Vec3 sample_normal(const Terrain& t, int i, int j) {
    const int n = t.n;
    const float dx = (t.at(i + 1, j) - t.at(i - 1, j)) / (t.cell_x() * static_cast<float>((i > 0 && i < n - 1) ? 2 : 1));
    const float dz = (t.at(i, j + 1) - t.at(i, j - 1)) / (t.cell_z() * static_cast<float>((j > 0 && j < n - 1) ? 2 : 1));
    return normalize(Vec3{-dx, 1, -dz});
}

float smooth_step(float a, float b, float x) {
    const float k = std::clamp((x - a) / std::max(b - a, 1e-6f), 0.0f, 1.0f);
    return k * k * (3 - 2 * k);
}

// 1 within lo..hi, falling to 0 across `soft` centred on each end that is a limit (lo above `least`, hi below `most`).
float window(float v, float lo, float hi, float soft, float least, float most) {
    float w = 1;
    if (lo > least) w *= smooth_step(lo - soft * 0.5f, lo + soft * 0.5f, v);
    if (hi < most) w *= 1.0f - smooth_step(hi - soft * 0.5f, hi + soft * 0.5f, v);
    return w;
}

}  // namespace

std::vector<std::array<float, 4>> terrain_layer_weights(const Terrain& t, const TerrainLook& look) {
    const int count = static_cast<int>(std::min<std::size_t>(look.layers.size(), 4));
    if (count == 0 || t.n < 1) return {};
    const std::size_t total = static_cast<std::size_t>(t.n) * static_cast<std::size_t>(t.n);
    const bool painted = t.layer_paint.size() == total;
    std::vector<std::array<float, 4>> out(total);
    for (int j = 0; j < t.n; ++j) {
        for (int i = 0; i < t.n; ++i) {
            const std::size_t k = static_cast<std::size_t>(j) * static_cast<std::size_t>(t.n) + static_cast<std::size_t>(i);
            const float slope = std::acos(std::clamp(sample_normal(t, i, j).y, -1.0f, 1.0f)) * 180.0f / std::numbers::pi_v<float>;
            const float high = t.height > 0 ? t.at(i, j) / t.height : 0.0f;
            // The first layer everywhere, each next over those before it where its rules let it lie.
            std::array<float, 4> w{1, 0, 0, 0};
            for (int l = 1; l < count; ++l) {
                const TerrainLook::Layer& ly = look.layers[static_cast<std::size_t>(l)];
                const float r = std::clamp(ly.cover, 0.0f, 1.0f) * window(slope, ly.slope.x, ly.slope.y, 4.0f, 0.0f, 90.0f) * window(high, ly.height.x, ly.height.y, 0.03f, 0.0f, 1.0f);
                for (float& c : w) c *= 1 - r;
                w[static_cast<std::size_t>(l)] += r;
            }
            // The paint over the rules, as much as it covers.
            if (painted) {
                std::array<float, 4> p = t.layer_paint[k];
                float sum = 0;
                for (int l = 0; l < 4; ++l) {
                    p[static_cast<std::size_t>(l)] = l < count ? std::clamp(p[static_cast<std::size_t>(l)], 0.0f, 1.0f) : 0.0f;
                    sum += p[static_cast<std::size_t>(l)];
                }
                if (sum > 1) {
                    for (float& c : p) c /= sum;
                    sum = 1;
                }
                for (int l = 0; l < 4; ++l) w[static_cast<std::size_t>(l)] = w[static_cast<std::size_t>(l)] * (1 - sum) + p[static_cast<std::size_t>(l)];
            }
            out[k] = w;
        }
    }
    return out;
}

Mesh terrain_mesh(const Terrain& t, const TerrainLook& look, const std::string& path) {
    Mesh m;
    m.path = path;
    m.importer = "terrain";
    m.vertex_colors = true;
    const int n = t.n;
    m.vertices.reserve(static_cast<std::size_t>(n) * static_cast<std::size_t>(n));
    const float cos_rock = std::cos(std::clamp(look.rock_slope, 0.0f, 89.0f) * std::numbers::pi_v<float> / 180.0f);
    const float tile = std::max(look.texture_tile, 1e-3f);
    const bool layered = !look.layers.empty();
    auto smooth = smooth_step;
    float lo = 1e30f, hi = -1e30f;
    for (int j = 0; j < n; ++j) {
        for (int i = 0; i < n; ++i) {
            const float x = -t.size_x * 0.5f + static_cast<float>(i) * t.cell_x();
            const float z = -t.size_z * 0.5f + static_cast<float>(j) * t.cell_z();
            const float y = t.at(i, j);
            lo = std::min(lo, y);
            hi = std::max(hi, y);
            const Vec3 nrm = sample_normal(t, i, j);
            // Grass, then rock where it is steep, then snow high up (not on the steepest rock); white
            // under textured layers, which bring their own.
            Vec3 c{1, 1, 1};
            if (!layered) {
                const float steep = 1.0f - smooth(cos_rock - 0.08f, cos_rock + 0.08f, nrm.y);
                const float high = look.snow_line < 1.0f ? smooth(look.snow_line - 0.05f, look.snow_line + 0.05f, t.height > 0 ? y / t.height : 0.0f) : 0.0f;
                c = look.grass + (look.rock - look.grass) * steep;
                c = c + (look.snow - c) * (high * (1.0f - steep * 0.7f));
            }
            // Paint over it by its weight (painted in sRGB, drawn in linear light like the rest). Over
            // textured layers the colour goes with its weight in the alpha, and the renderer lays it
            // over the layers' images.
            float cover = layered ? 0.0f : 1.0f;
            if (!t.paint.empty()) {
                const auto& p = t.paint[static_cast<std::size_t>(j) * static_cast<std::size_t>(n) + static_cast<std::size_t>(i)];
                auto lin = [](float s) { s = std::clamp(s, 0.0f, 1.0f); return s <= 0.04045f ? s / 12.92f : std::pow((s + 0.055f) / 1.055f, 2.4f); };
                const float w = std::clamp(p[3], 0.0f, 1.0f);
                if (layered) {
                    c = Vec3{lin(p[0]), lin(p[1]), lin(p[2])};
                    cover = w;
                } else {
                    c = c + (Vec3{lin(p[0]), lin(p[1]), lin(p[2])} - c) * w;
                }
            }
            MeshVertex v;
            v.position = {x, y, z};
            v.normal = nrm;
            v.uv = {x / tile, z / tile};
            v.color = {c.x, c.y, c.z, cover};
            m.vertices.push_back(v);
        }
    }
    auto idx = [n](int i, int j) { return static_cast<std::uint32_t>(j * n + i); };
    // The samples along a side from a to b every s, b always among them.
    auto samples = [](int a, int b, int s) {
        std::vector<int> v;
        for (int i = a; i < b; i += s) v.push_back(i);
        v.push_back(b);
        return v;
    };
    // The cells between those samples, each split along a-d (the diagonal sample() follows), wound
    // counter-clockwise seen from above.
    auto grid = [&](std::vector<std::uint32_t>& out, const std::vector<int>& xs, const std::vector<int>& zs) {
        for (std::size_t b = 0; b + 1 < zs.size(); ++b)
            for (std::size_t a = 0; a + 1 < xs.size(); ++a) {
                const std::uint32_t p = idx(xs[a], zs[b]), q = idx(xs[a + 1], zs[b]), r = idx(xs[a], zs[b + 1]), s = idx(xs[a + 1], zs[b + 1]);
                out.insert(out.end(), {p, s, q, p, r, s});
            }
    };
    m.indices.reserve(static_cast<std::size_t>(n - 1) * static_cast<std::size_t>(n - 1) * 6);
    grid(m.indices, samples(0, n - 1, 1), samples(0, n - 1, 1));
    fill_tangents(m);   // from the full grid alone: the skirts and the coarser levels add nothing to them
    m.indices.clear();
    // Chunks (docs/design/terrain.md, Levels of detail): a grid of 256 cells or more across is cut
    // into squares of an eighth of it (32 to 128 cells), a smaller one is a single square; each
    // square has levels taking every 2nd, 4th ... sample, down to its corners alone.
    const int cells = n - 1;
    const int side = cells >= 256 ? std::clamp(cells / 16, 32, 64) : std::max(cells, 1);
    auto h = [&](int i, int j) { return m.vertices[idx(i, j)].position.y; };
    // How far the full grid's samples are from a level's surface over a square.
    auto level_error = [&](const std::vector<int>& xs, const std::vector<int>& zs) {
        float e = 0;
        for (std::size_t b = 0; b + 1 < zs.size(); ++b)
            for (std::size_t a = 0; a + 1 < xs.size(); ++a) {
                const int xa = xs[a], xb = xs[a + 1], za = zs[b], zb = zs[b + 1];
                if (xb - xa < 2 && zb - za < 2) continue;
                const float h00 = h(xa, za), h10 = h(xb, za), h01 = h(xa, zb), h11 = h(xb, zb);
                for (int j = za; j <= zb; ++j)
                    for (int i = xa; i <= xb; ++i) {
                        const float u = static_cast<float>(i - xa) / static_cast<float>(xb - xa), v = static_cast<float>(j - za) / static_cast<float>(zb - za);
                        const float on = u >= v ? h00 + u * (h10 - h00) + v * (h11 - h10) : h00 + v * (h01 - h00) + u * (h11 - h01);
                        e = std::max(e, std::fabs(h(i, j) - on));
                    }
            }
        return e;
    };
    struct Square { int i0, i1, j0, j1; std::vector<int> strides; std::vector<float> errors; };
    std::vector<Square> squares;
    float deepest = 0;
    for (int j0 = 0; j0 < cells; j0 += side)
        for (int i0 = 0; i0 < cells; i0 += side) {
            Square sq{i0, std::min(i0 + side, cells), j0, std::min(j0 + side, cells), {}, {}};
            float worst = 0;
            for (int s = 1;; s *= 2) {
                worst = std::max(worst, s == 1 ? 0.0f : level_error(samples(sq.i0, sq.i1, s), samples(sq.j0, sq.j1, s)));
                sq.strides.push_back(s);
                sq.errors.push_back(worst);
                if (s >= sq.i1 - sq.i0 && s >= sq.j1 - sq.j0) break;
            }
            deepest = std::max(deepest, worst);
            squares.push_back(std::move(sq));
        }
    // Skirts: every sample on a line between squares has a twin below it, as deep as the worst
    // error of any level plus a little, so a crack between two levels shows the skirt, not the sky.
    const float cell = std::max(t.cell_x(), t.cell_z());
    const float depth = deepest + 0.05f * cell;
    std::vector<std::uint32_t> twin(static_cast<std::size_t>(n) * static_cast<std::size_t>(n), 0);
    for (int j = 0; j < n; ++j)
        for (int i = 0; i < n; ++i) {
            const bool on_line = (i % side == 0 && i > 0 && i < cells) || (j % side == 0 && j > 0 && j < cells);
            if (!on_line) continue;
            MeshVertex v = m.vertices[idx(i, j)];
            v.position.y -= depth;
            twin[idx(i, j)] = static_cast<std::uint32_t>(m.vertices.size());
            m.vertices.push_back(v);
        }
    // A square's skirts at a stride, on its edges inside the grid, seen from both sides.
    auto skirts = [&](std::vector<std::uint32_t>& out, const Square& sq, int s) {
        auto edge = [&](bool along_x, int fixed, int a, int b) {
            const std::vector<int> at = samples(a, b, s);
            for (std::size_t k = 0; k + 1 < at.size(); ++k) {
                const std::uint32_t p = along_x ? idx(at[k], fixed) : idx(fixed, at[k]), q = along_x ? idx(at[k + 1], fixed) : idx(fixed, at[k + 1]);
                const std::uint32_t pl = twin[p], ql = twin[q];
                out.insert(out.end(), {p, q, ql, p, ql, pl, p, ql, q, p, pl, ql});
            }
        };
        if (sq.i0 > 0) edge(false, sq.i0, sq.j0, sq.j1);
        if (sq.i1 < cells) edge(false, sq.i1, sq.j0, sq.j1);
        if (sq.j0 > 0) edge(true, sq.j0, sq.i0, sq.i1);
        if (sq.j1 < cells) edge(true, sq.j1, sq.i0, sq.i1);
    };
    // The full grids first, each square's after its skirts (the grids are the submeshes, which
    // collide; level 0 draws both), then every coarser level with its skirts.
    for (const Square& sq : squares) {
        TerrainChunk ch;
        ch.cell = cell;
        float y0 = 1e30f, y1 = -1e30f;
        for (int j = sq.j0; j <= sq.j1; ++j)
            for (int i = sq.i0; i <= sq.i1; ++i) { y0 = std::min(y0, h(i, j)); y1 = std::max(y1, h(i, j)); }
        y0 -= depth;
        const Vec3 lo3{m.vertices[idx(sq.i0, sq.j0)].position.x, y0, m.vertices[idx(sq.i0, sq.j0)].position.z};
        const Vec3 hi3{m.vertices[idx(sq.i1, sq.j1)].position.x, y1, m.vertices[idx(sq.i1, sq.j1)].position.z};
        ch.center = (lo3 + hi3) * 0.5f;
        ch.radius = length(hi3 - lo3) * 0.5f;
        const auto first = static_cast<std::uint32_t>(m.indices.size());
        skirts(m.indices, sq, 1);
        Submesh sm;
        sm.first_index = static_cast<std::uint32_t>(m.indices.size());
        grid(m.indices, samples(sq.i0, sq.i1, 1), samples(sq.j0, sq.j1, 1));
        sm.index_count = static_cast<std::uint32_t>(m.indices.size()) - sm.first_index;
        m.submeshes.push_back(sm);
        ch.levels.push_back({first, static_cast<std::uint32_t>(m.indices.size()) - first, 1, 0.0f});
        m.chunks.push_back(std::move(ch));
    }
    for (std::size_t c = 0; c < squares.size(); ++c) {
        const Square& sq = squares[c];
        for (std::size_t k = 1; k < sq.strides.size(); ++k) {
            const auto first = static_cast<std::uint32_t>(m.indices.size());
            grid(m.indices, samples(sq.i0, sq.i1, sq.strides[k]), samples(sq.j0, sq.j1, sq.strides[k]));
            skirts(m.indices, sq, sq.strides[k]);
            m.chunks[c].levels.push_back({first, static_cast<std::uint32_t>(m.indices.size()) - first, sq.strides[k], sq.errors[k]});
        }
    }
    Material mat;
    mat.name = "terrain";
    mat.roughness = 0.95f;
    if (layered) {
        auto tl = std::make_shared<TerrainLayers>();
        for (std::size_t l = 0; l < std::min<std::size_t>(look.layers.size(), 4); ++l) tl->layers.push_back({look.layers[l].texture, look.layers[l].color, std::max(look.layers[l].tile, 1e-3f)});
        tl->n = n;
        tl->size = {t.size_x, t.size_z};
        tl->texture_tile = tile;
        const auto shares = terrain_layer_weights(t, look);
        tl->weights.resize(shares.size() * 4);
        for (std::size_t k = 0; k < shares.size(); ++k)
            for (std::size_t c = 0; c < 4; ++c) tl->weights[k * 4 + c] = static_cast<std::uint8_t>(std::lround(std::clamp(shares[k][c], 0.0f, 1.0f) * 255.0f));
        mat.terrain = std::move(tl);
    }
    m.materials.push_back(mat);
    m.aabb_min = {-t.size_x * 0.5f, lo, -t.size_z * 0.5f};
    m.aabb_max = {t.size_x * 0.5f, hi, t.size_z * 0.5f};
    m.node_count = 1;
    return m;
}

namespace {

std::uint32_t crc32(const unsigned char* data, std::size_t len, std::uint32_t crc = 0) {
    static const std::array<std::uint32_t, 256> table = [] {
        std::array<std::uint32_t, 256> tab{};
        for (std::uint32_t k = 0; k < 256; ++k) {
            std::uint32_t c = k;
            for (int b = 0; b < 8; ++b) c = (c & 1) ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            tab[k] = c;
        }
        return tab;
    }();
    crc = ~crc;
    for (std::size_t k = 0; k < len; ++k) crc = table[(crc ^ data[k]) & 0xFFu] ^ (crc >> 8);
    return ~crc;
}

void put_u32(std::string& out, std::uint32_t v) {
    out.push_back(static_cast<char>((v >> 24) & 0xFF));
    out.push_back(static_cast<char>((v >> 16) & 0xFF));
    out.push_back(static_cast<char>((v >> 8) & 0xFF));
    out.push_back(static_cast<char>(v & 0xFF));
}

void chunk(std::string& out, const char* type, const std::string& body) {
    put_u32(out, static_cast<std::uint32_t>(body.size()));
    std::string typed = std::string(type, 4) + body;
    out += typed;
    put_u32(out, crc32(reinterpret_cast<const unsigned char*>(typed.data()), typed.size()));
}

}  // namespace

std::string terrain_png16(const Terrain& t) {
    // Rows of big-endian 16-bit grey samples, each after a filter byte of 0, deflated by stb.
    std::string raw;
    raw.reserve(static_cast<std::size_t>(t.n) * (static_cast<std::size_t>(t.n) * 2 + 1));
    for (int j = 0; j < t.n; ++j) {
        raw.push_back(0);
        for (int i = 0; i < t.n; ++i) {
            const float k = t.height > 0 ? std::clamp(t.at(i, j) / t.height, 0.0f, 1.0f) : 0.0f;
            const auto s = static_cast<std::uint16_t>(std::lround(k * 65535.0f));
            raw.push_back(static_cast<char>(s >> 8));
            raw.push_back(static_cast<char>(s & 0xFF));
        }
    }
    int zlen = 0;
    unsigned char* z = stbi_zlib_compress(reinterpret_cast<unsigned char*>(raw.data()), static_cast<int>(raw.size()), &zlen, 8);
    std::string out("\x89PNG\r\n\x1a\n", 8);
    std::string ihdr;
    put_u32(ihdr, static_cast<std::uint32_t>(t.n));
    put_u32(ihdr, static_cast<std::uint32_t>(t.n));
    ihdr += std::string("\x10\x00\x00\x00\x00", 5);   // 16 bits, greyscale, deflate, no filter method, no interlace
    chunk(out, "IHDR", ihdr);
    chunk(out, "IDAT", std::string(reinterpret_cast<const char*>(z), static_cast<std::size_t>(zlen)));
    std::free(z);
    chunk(out, "IEND", "");
    return out;
}

Result<std::vector<std::array<float, 4>>> terrain_paint_from_image(const std::string& bytes, const std::string& display_path, int n) {
    int w = 0, hgt = 0, channels = 0;
    stbi_uc* px = stbi_load_from_memory(reinterpret_cast<const stbi_uc*>(bytes.data()), static_cast<int>(bytes.size()), &w, &hgt, &channels, 4);
    if (!px) return fail("bad_image", "{}: {}", display_path, stbi_failure_reason());
    if (w < 2 || hgt < 2) {
        stbi_image_free(px);
        return fail("bad_image", "{}: a paint map needs at least 2 by 2 pixels", display_path);
    }
    n = std::clamp(n, 2, 1025);
    std::vector<std::array<float, 4>> out(static_cast<std::size_t>(n) * static_cast<std::size_t>(n));
    auto g = [&](int x, int y, int c) { return static_cast<float>(px[(static_cast<std::size_t>(y) * static_cast<std::size_t>(w) + static_cast<std::size_t>(x)) * 4 + static_cast<std::size_t>(c)]) / 255.0f; };
    // Bilinear, as the heights; the colour weighed by its alpha so an unpainted neighbour's colour does not bleed in.
    for (int j = 0; j < n; ++j) {
        const float py = static_cast<float>(j) / static_cast<float>(n - 1) * static_cast<float>(hgt - 1);
        const int y0 = std::min(static_cast<int>(py), hgt - 2);
        const float fy = py - static_cast<float>(y0);
        for (int i = 0; i < n; ++i) {
            const float pxf = static_cast<float>(i) / static_cast<float>(n - 1) * static_cast<float>(w - 1);
            const int x0 = std::min(static_cast<int>(pxf), w - 2);
            const float fx = pxf - static_cast<float>(x0);
            const float k[4] = {(1 - fx) * (1 - fy), fx * (1 - fy), (1 - fx) * fy, fx * fy};
            const int xs[4] = {x0, x0 + 1, x0, x0 + 1}, ys[4] = {y0, y0, y0 + 1, y0 + 1};
            std::array<float, 4> s{};
            for (int q = 0; q < 4; ++q) {
                const float a = g(xs[q], ys[q], 3) * k[q];
                for (int c = 0; c < 3; ++c) s[c] += g(xs[q], ys[q], c) * a;
                s[3] += a;
            }
            if (s[3] > 1e-6f) for (int c = 0; c < 3; ++c) s[c] /= s[3];
            out[static_cast<std::size_t>(j) * static_cast<std::size_t>(n) + static_cast<std::size_t>(i)] = s;
        }
    }
    stbi_image_free(px);
    return out;
}

namespace {

// n by n samples of four numbers 0..1 as an 8-bit RGBA PNG (zeros when the grid is not n * n).
std::string rgba_png(int n, const std::vector<std::array<float, 4>>& grid) {
    std::string raw;
    raw.reserve(static_cast<std::size_t>(n) * (static_cast<std::size_t>(n) * 4 + 1));
    const bool painted = grid.size() == static_cast<std::size_t>(n) * static_cast<std::size_t>(n);
    for (int j = 0; j < n; ++j) {
        raw.push_back(0);
        for (int i = 0; i < n; ++i) {
            const std::array<float, 4> p = painted ? grid[static_cast<std::size_t>(j) * static_cast<std::size_t>(n) + static_cast<std::size_t>(i)] : std::array<float, 4>{0, 0, 0, 0};
            for (int c = 0; c < 4; ++c) raw.push_back(static_cast<char>(std::lround(std::clamp(p[c], 0.0f, 1.0f) * 255.0f)));
        }
    }
    int zlen = 0;
    unsigned char* z = stbi_zlib_compress(reinterpret_cast<unsigned char*>(raw.data()), static_cast<int>(raw.size()), &zlen, 8);
    std::string out("\x89PNG\r\n\x1a\n", 8);
    std::string ihdr;
    put_u32(ihdr, static_cast<std::uint32_t>(n));
    put_u32(ihdr, static_cast<std::uint32_t>(n));
    ihdr += std::string("\x08\x06\x00\x00\x00", 5);   // 8 bits, RGBA, deflate, no filter method, no interlace
    chunk(out, "IHDR", ihdr);
    chunk(out, "IDAT", std::string(reinterpret_cast<const char*>(z), static_cast<std::size_t>(zlen)));
    std::free(z);
    chunk(out, "IEND", "");
    return out;
}

}  // namespace

std::string terrain_paint_png(const Terrain& t) { return rgba_png(t.n, t.paint); }

std::string terrain_layers_png(const Terrain& t) { return rgba_png(t.n, t.layer_paint); }

Result<std::vector<std::array<float, 4>>> terrain_layers_from_image(const std::string& bytes, const std::string& display_path, int n) {
    int w = 0, hgt = 0, channels = 0;
    stbi_uc* px = stbi_load_from_memory(reinterpret_cast<const stbi_uc*>(bytes.data()), static_cast<int>(bytes.size()), &w, &hgt, &channels, 4);
    if (!px) return fail("bad_image", "{}: {}", display_path, stbi_failure_reason());
    if (w < 2 || hgt < 2) {
        stbi_image_free(px);
        return fail("bad_image", "{}: a layer map needs at least 2 by 2 pixels", display_path);
    }
    n = std::clamp(n, 2, 1025);
    std::vector<std::array<float, 4>> out(static_cast<std::size_t>(n) * static_cast<std::size_t>(n));
    auto g = [&](int x, int y, int c) { return static_cast<float>(px[(static_cast<std::size_t>(y) * static_cast<std::size_t>(w) + static_cast<std::size_t>(x)) * 4 + static_cast<std::size_t>(c)]) / 255.0f; };
    for (int j = 0; j < n; ++j) {
        const float py = static_cast<float>(j) / static_cast<float>(n - 1) * static_cast<float>(hgt - 1);
        const int y0 = std::min(static_cast<int>(py), hgt - 2);
        const float fy = py - static_cast<float>(y0);
        for (int i = 0; i < n; ++i) {
            const float pxf = static_cast<float>(i) / static_cast<float>(n - 1) * static_cast<float>(w - 1);
            const int x0 = std::min(static_cast<int>(pxf), w - 2);
            const float fx = pxf - static_cast<float>(x0);
            std::array<float, 4> s{};
            float sum = 0;
            for (int c = 0; c < 4; ++c) {
                s[static_cast<std::size_t>(c)] = (g(x0, y0, c) * (1 - fx) + g(x0 + 1, y0, c) * fx) * (1 - fy) + (g(x0, y0 + 1, c) * (1 - fx) + g(x0 + 1, y0 + 1, c) * fx) * fy;
                sum += s[static_cast<std::size_t>(c)];
            }
            if (sum > 1) for (float& v : s) v /= sum;
            out[static_cast<std::size_t>(j) * static_cast<std::size_t>(n) + static_cast<std::size_t>(i)] = s;
        }
    }
    stbi_image_free(px);
    return out;
}

}  // namespace pocket::assets
