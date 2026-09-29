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

std::array<float, 4> Terrain::paint_at(float x, float z) const {
    if (n < 2 || paint.size() != static_cast<std::size_t>(n) * static_cast<std::size_t>(n)) return {0, 0, 0, 0};
    int i = 0, j = 0;
    float u = 0, v = 0;
    locate(*this, x, z, i, j, u, v);
    auto p = [&](int a, int b) { return paint[static_cast<std::size_t>(b) * static_cast<std::size_t>(n) + static_cast<std::size_t>(a)]; };
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

Terrain terrain_from_noise(std::uint32_t seed, float scale, int octaves, int n, Vec2 size, float height) {
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
    return t;
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
    auto smooth = [](float a, float b, float x) {
        const float k = std::clamp((x - a) / std::max(b - a, 1e-6f), 0.0f, 1.0f);
        return k * k * (3 - 2 * k);
    };
    float lo = 1e30f, hi = -1e30f;
    for (int j = 0; j < n; ++j) {
        for (int i = 0; i < n; ++i) {
            const float x = -t.size_x * 0.5f + static_cast<float>(i) * t.cell_x();
            const float z = -t.size_z * 0.5f + static_cast<float>(j) * t.cell_z();
            const float y = t.at(i, j);
            lo = std::min(lo, y);
            hi = std::max(hi, y);
            // The vertex normal from its neighbours (central differences, one-sided at the edges).
            const float dx = (t.at(i + 1, j) - t.at(i - 1, j)) / (t.cell_x() * static_cast<float>((i > 0 && i < n - 1) ? 2 : 1));
            const float dz = (t.at(i, j + 1) - t.at(i, j - 1)) / (t.cell_z() * static_cast<float>((j > 0 && j < n - 1) ? 2 : 1));
            const Vec3 nrm = normalize(Vec3{-dx, 1, -dz});
            // Grass, then rock where it is steep, then snow high up (not on the steepest rock).
            const float steep = 1.0f - smooth(cos_rock - 0.08f, cos_rock + 0.08f, nrm.y);
            const float high = look.snow_line < 1.0f ? smooth(look.snow_line - 0.05f, look.snow_line + 0.05f, t.height > 0 ? y / t.height : 0.0f) : 0.0f;
            Vec3 c = look.grass + (look.rock - look.grass) * steep;
            c = c + (look.snow - c) * (high * (1.0f - steep * 0.7f));
            // Paint over it by its weight (painted in sRGB, drawn in linear light like the rest).
            if (!t.paint.empty()) {
                const auto& p = t.paint[static_cast<std::size_t>(j) * static_cast<std::size_t>(n) + static_cast<std::size_t>(i)];
                auto lin = [](float s) { s = std::clamp(s, 0.0f, 1.0f); return s <= 0.04045f ? s / 12.92f : std::pow((s + 0.055f) / 1.055f, 2.4f); };
                const float w = std::clamp(p[3], 0.0f, 1.0f);
                c = c + (Vec3{lin(p[0]), lin(p[1]), lin(p[2])} - c) * w;
            }
            MeshVertex v;
            v.position = {x, y, z};
            v.normal = nrm;
            v.uv = {x / tile, z / tile};
            v.color = {c.x, c.y, c.z, 1};
            m.vertices.push_back(v);
        }
    }
    m.indices.reserve(static_cast<std::size_t>(n - 1) * static_cast<std::size_t>(n - 1) * 6);
    for (int j = 0; j + 1 < n; ++j) {
        for (int i = 0; i + 1 < n; ++i) {
            const auto a = static_cast<std::uint32_t>(j * n + i), b = a + 1, c = a + static_cast<std::uint32_t>(n), d = c + 1;
            // Split along a-d (the diagonal sample() follows), wound counter-clockwise seen from above.
            m.indices.insert(m.indices.end(), {a, d, b, a, c, d});
        }
    }
    Submesh sm;
    sm.index_count = static_cast<std::uint32_t>(m.indices.size());
    m.submeshes.push_back(sm);
    Material mat;
    mat.name = "terrain";
    mat.roughness = 0.95f;
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

std::string terrain_paint_png(const Terrain& t) {
    std::string raw;
    raw.reserve(static_cast<std::size_t>(t.n) * (static_cast<std::size_t>(t.n) * 4 + 1));
    const bool painted = t.paint.size() == static_cast<std::size_t>(t.n) * static_cast<std::size_t>(t.n);
    for (int j = 0; j < t.n; ++j) {
        raw.push_back(0);
        for (int i = 0; i < t.n; ++i) {
            const std::array<float, 4> p = painted ? t.paint[static_cast<std::size_t>(j) * static_cast<std::size_t>(t.n) + static_cast<std::size_t>(i)] : std::array<float, 4>{0, 0, 0, 0};
            for (int c = 0; c < 4; ++c) raw.push_back(static_cast<char>(std::lround(std::clamp(p[c], 0.0f, 1.0f) * 255.0f)));
        }
    }
    int zlen = 0;
    unsigned char* z = stbi_zlib_compress(reinterpret_cast<unsigned char*>(raw.data()), static_cast<int>(raw.size()), &zlen, 8);
    std::string out("\x89PNG\r\n\x1a\n", 8);
    std::string ihdr;
    put_u32(ihdr, static_cast<std::uint32_t>(t.n));
    put_u32(ihdr, static_cast<std::uint32_t>(t.n));
    ihdr += std::string("\x08\x06\x00\x00\x00", 5);   // 8 bits, RGBA, deflate, no filter method, no interlace
    chunk(out, "IHDR", ihdr);
    chunk(out, "IDAT", std::string(reinterpret_cast<const char*>(z), static_cast<std::size_t>(zlen)));
    std::free(z);
    chunk(out, "IEND", "");
    return out;
}

}  // namespace pocket::assets
