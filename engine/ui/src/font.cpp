#include <pocket/ui/font.hpp>

#include <pocket/core/log.hpp>

#include <ft2build.h>
#include FT_FREETYPE_H
#include <hb.h>
#include <hb-ot.h>

#include <cstring>
#include <map>
#include <unordered_map>

namespace pocket::ui {

std::uint32_t decode_utf8(std::string_view s, std::size_t& i) {
    auto byte = [&](std::size_t k) -> std::uint32_t { return k < s.size() ? static_cast<unsigned char>(s[k]) : 0; };
    std::uint32_t c = byte(i);
    if (c < 0x80) { i += 1; return c; }
    if ((c & 0xE0) == 0xC0 && (byte(i + 1) & 0xC0) == 0x80) {
        std::uint32_t r = ((c & 0x1F) << 6) | (byte(i + 1) & 0x3F);
        i += 2;
        return r;
    }
    if ((c & 0xF0) == 0xE0 && (byte(i + 1) & 0xC0) == 0x80 && (byte(i + 2) & 0xC0) == 0x80) {
        std::uint32_t r = ((c & 0x0F) << 12) | ((byte(i + 1) & 0x3F) << 6) | (byte(i + 2) & 0x3F);
        i += 3;
        return r;
    }
    if ((c & 0xF8) == 0xF0 && (byte(i + 1) & 0xC0) == 0x80 && (byte(i + 2) & 0xC0) == 0x80 && (byte(i + 3) & 0xC0) == 0x80) {
        std::uint32_t r = ((c & 0x07) << 18) | ((byte(i + 1) & 0x3F) << 12) | ((byte(i + 2) & 0x3F) << 6) | (byte(i + 3) & 0x3F);
        i += 4;
        return r;
    }
    i += 1;
    return 0xFFFD;
}

namespace {

struct Shelf {
    std::uint32_t y = 0, height = 0, x = 0;
};

}  // namespace

struct Font::Impl {
    rhi::Device* device = nullptr;
    FT_Library library = nullptr;
    FT_Face face = nullptr;
    hb_blob_t* hb_blob = nullptr;
    hb_face_t* hb_face = nullptr;
    hb_font_t* hb_font = nullptr;
    hb_buffer_t* hb_buf = nullptr;
    std::string family;
    std::uint32_t atlas_size = 1024;
    std::vector<std::uint8_t> atlas;  // CPU copy
    WGPUTexture texture = nullptr;
    WGPUTextureView view = nullptr;
    std::vector<Shelf> shelves;
    bool dirty = false;
    std::uint32_t dirty_y0 = 0, dirty_y1 = 0;
    std::map<std::pair<std::uint32_t, int>, Glyph> glyphs;  // (glyph index, size*64)
    std::map<int, TextMetrics> metrics_cache;
    std::unordered_map<std::string, float> measure_cache;   // text + size key -> width
    int current_size = -1;

    ~Impl() {
        if (view) wgpuTextureViewRelease(view);
        if (texture) wgpuTextureRelease(texture);
        if (hb_buf) hb_buffer_destroy(hb_buf);
        if (hb_font) hb_font_destroy(hb_font);
        if (hb_face) hb_face_destroy(hb_face);
        if (hb_blob) hb_blob_destroy(hb_blob);
        if (face) FT_Done_Face(face);
        if (library) FT_Done_FreeType(library);
    }

    void set_size(float px) {
        int key = static_cast<int>(px * 64.0f);
        if (key == current_size) return;
        FT_Set_Char_Size(face, 0, static_cast<FT_F26Dot6>(key), 72, 72);
        // HarfBuzz positions come back in 26.6 pixels when the scale is px * 64.
        if (hb_font) {
            hb_font_set_scale(hb_font, key, key);
            hb_font_set_ppem(hb_font, static_cast<unsigned>(px), static_cast<unsigned>(px));
        }
        current_size = key;
    }

    Status create_texture(std::uint32_t size) {
        if (view) { wgpuTextureViewRelease(view); view = nullptr; }
        if (texture) { wgpuTextureRelease(texture); texture = nullptr; }
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.font.atlas");
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {size, size, 1};
        td.format = WGPUTextureFormat_R8Unorm;
        td.mipLevelCount = 1;
        td.sampleCount = 1;
        texture = wgpuDeviceCreateTexture(device->device(), &td);
        if (!texture) return fail("gpu_texture_failed", "cannot create font atlas {}x{}", size, size);
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = 1;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        view = wgpuTextureCreateView(texture, &vd);
        atlas_size = size;
        atlas.assign(static_cast<std::size_t>(size) * size, 0);
        shelves.clear();
        glyphs.clear();
        dirty = true;
        dirty_y0 = 0;
        dirty_y1 = size;
        // A white pixel at (0,0) for solid fills: reserve the first shelf row.
        atlas[0] = 255;
        atlas[1] = 255;
        atlas[atlas_size] = 255;
        atlas[atlas_size + 1] = 255;
        shelves.push_back({0, 4, 4});
        return {};
    }

    bool pack(std::uint32_t w, std::uint32_t h, std::uint32_t& out_x, std::uint32_t& out_y) {
        const std::uint32_t pad = 1;
        w += pad;
        h += pad;
        for (Shelf& s : shelves) {
            if (h <= s.height && s.x + w <= atlas_size) {
                out_x = s.x;
                out_y = s.y;
                s.x += w;
                return true;
            }
        }
        std::uint32_t next_y = shelves.empty() ? 0 : shelves.back().y + shelves.back().height;
        std::uint32_t shelf_h = (h + 7) / 8 * 8;
        if (next_y + shelf_h > atlas_size || w > atlas_size) return false;
        shelves.push_back({next_y, shelf_h, w});
        out_x = 0;
        out_y = next_y;
        return true;
    }

    void upload() {
        if (!dirty) return;
        std::uint32_t y0 = dirty_y0, y1 = std::min(dirty_y1, atlas_size);
        if (y1 <= y0) { dirty = false; return; }
        WGPUTexelCopyTextureInfo dst{};
        dst.texture = texture;
        dst.origin = {0, y0, 0};
        dst.aspect = WGPUTextureAspect_All;
        WGPUTexelCopyBufferLayout layout{};
        layout.offset = 0;
        layout.bytesPerRow = atlas_size;
        layout.rowsPerImage = y1 - y0;
        WGPUExtent3D ext{atlas_size, y1 - y0, 1};
        wgpuQueueWriteTexture(device->queue(), &dst, atlas.data() + static_cast<std::size_t>(y0) * atlas_size, static_cast<std::size_t>(y1 - y0) * atlas_size, &layout, &ext);
        dirty = false;
        dirty_y0 = atlas_size;
        dirty_y1 = 0;
    }
};

Font::Font() : impl_(std::make_unique<Impl>()) {}
Font::~Font() = default;

Result<std::unique_ptr<Font>> Font::load(rhi::Device& device, const std::string& path) {
    std::unique_ptr<Font> f(new Font());
    Impl& im = *f->impl_;
    im.device = &device;
    if (FT_Init_FreeType(&im.library) != 0) return fail("font_init_failed", "FT_Init_FreeType failed");
    if (FT_New_Face(im.library, path.c_str(), 0, &im.face) != 0) return fail("font_load_failed", "cannot load font {}", path);
    im.family = im.face->family_name ? im.face->family_name : "unknown";
    // HarfBuzz reads the same file with its own OpenType functions; FreeType only rasterizes.
    im.hb_blob = hb_blob_create_from_file_or_fail(path.c_str());
    if (!im.hb_blob) return fail("font_load_failed", "cannot read font {} for shaping", path);
    im.hb_face = hb_face_create(im.hb_blob, 0);
    im.hb_font = hb_font_create(im.hb_face);
    hb_ot_font_set_funcs(im.hb_font);
    im.hb_buf = hb_buffer_create();
    POCKET_TRY_VOID(im.create_texture(1024));
    im.dirty_y0 = 0;
    im.dirty_y1 = im.atlas_size;
    log::info("ui", "font {} ({} glyphs in face)", im.family, static_cast<long>(im.face->num_glyphs));
    return f;
}

TextMetrics Font::metrics(float px_size) {
    Impl& im = *impl_;
    int key = static_cast<int>(px_size * 64.0f);
    if (auto it = im.metrics_cache.find(key); it != im.metrics_cache.end()) return it->second;
    im.set_size(px_size);
    TextMetrics m;
    m.ascent = static_cast<float>(im.face->size->metrics.ascender) / 64.0f;
    m.descent = -static_cast<float>(im.face->size->metrics.descender) / 64.0f;
    m.line_height = static_cast<float>(im.face->size->metrics.height) / 64.0f;
    im.metrics_cache[key] = m;
    return m;
}

const Glyph& Font::glyph(std::uint32_t codepoint, float px_size) {
    Impl& im = *impl_;
    FT_UInt index = FT_Get_Char_Index(im.face, codepoint);
    if (index == 0 && codepoint != ' ') index = FT_Get_Char_Index(im.face, 0xFFFD);
    return glyph_by_index(index, px_size);
}

const Glyph& Font::glyph_by_index(std::uint32_t index, float px_size) {
    Impl& im = *impl_;
    int key = static_cast<int>(px_size * 64.0f);
    auto k = std::make_pair(index, key);
    if (auto it = im.glyphs.find(k); it != im.glyphs.end()) return it->second;
    im.set_size(px_size);
    Glyph g;
    if (FT_Load_Glyph(im.face, index, FT_LOAD_RENDER | FT_LOAD_TARGET_LIGHT) == 0) {
        FT_GlyphSlot slot = im.face->glyph;
        g.advance = static_cast<float>(slot->advance.x) / 64.0f;
        g.width = static_cast<float>(slot->bitmap.width);
        g.height = static_cast<float>(slot->bitmap.rows);
        g.bearing_x = static_cast<float>(slot->bitmap_left);
        g.bearing_y = static_cast<float>(slot->bitmap_top);
        if (slot->bitmap.width > 0 && slot->bitmap.rows > 0 && slot->bitmap.buffer) {
            std::uint32_t x = 0, y = 0;
            bool packed = im.pack(slot->bitmap.width, slot->bitmap.rows, x, y);
            if (!packed && im.atlas_size < 4096) {
                // Grow the atlas: every cached glyph is invalidated and re-rasterized on demand.
                std::uint32_t next = im.atlas_size * 2;
                if (auto r = im.create_texture(next); r) {
                    im.set_size(px_size);
                    FT_Load_Glyph(im.face, index, FT_LOAD_RENDER | FT_LOAD_TARGET_LIGHT);
                    slot = im.face->glyph;
                    packed = im.pack(slot->bitmap.width, slot->bitmap.rows, x, y);
                }
            }
            if (packed) {
                for (unsigned row = 0; row < slot->bitmap.rows; ++row) {
                    std::memcpy(im.atlas.data() + static_cast<std::size_t>(y + row) * im.atlas_size + x, slot->bitmap.buffer + static_cast<std::size_t>(row) * static_cast<std::size_t>(slot->bitmap.pitch), slot->bitmap.width);
                }
                im.dirty = true;
                im.dirty_y0 = std::min(im.dirty_y0, y);
                im.dirty_y1 = std::max(im.dirty_y1, y + slot->bitmap.rows + 1);
                float s = static_cast<float>(im.atlas_size);
                g.u0 = static_cast<float>(x) / s;
                g.v0 = static_cast<float>(y) / s;
                g.u1 = static_cast<float>(x + slot->bitmap.width) / s;
                g.v1 = static_cast<float>(y + slot->bitmap.rows) / s;
                g.empty = false;
            } else {
                log::warn("ui", "font atlas full; glyph #{} dropped", index);
            }
        }
    }
    return im.glyphs.emplace(k, g).first->second;
}

std::vector<ShapedGlyph> Font::shape(std::string_view text, float px_size) {
    Impl& im = *impl_;
    std::vector<ShapedGlyph> out;
    if (text.empty()) return out;
    im.set_size(px_size);
    // Line breaks and tabs keep their byte offsets (callers index the source string) but shape
    // as spaces; a line break then advances nothing.
    std::string cleaned(text);
    for (char& c : cleaned) if (c == '\t' || c == '\n' || c == '\r') c = ' ';
    hb_buffer_clear_contents(im.hb_buf);
    hb_buffer_add_utf8(im.hb_buf, cleaned.data(), static_cast<int>(cleaned.size()), 0, static_cast<int>(cleaned.size()));
    hb_buffer_guess_segment_properties(im.hb_buf);
    hb_buffer_set_cluster_level(im.hb_buf, HB_BUFFER_CLUSTER_LEVEL_MONOTONE_CHARACTERS);
    hb_shape(im.hb_font, im.hb_buf, nullptr, 0);
    unsigned n = 0;
    hb_glyph_info_t* infos = hb_buffer_get_glyph_infos(im.hb_buf, &n);
    hb_glyph_position_t* pos = hb_buffer_get_glyph_positions(im.hb_buf, &n);
    FT_UInt fallback = FT_Get_Char_Index(im.face, 0xFFFD);
    float pen = 0;
    out.reserve(n);
    for (unsigned i = 0; i < n; ++i) {
        std::size_t cluster = infos[i].cluster;
        ShapedGlyph sg;
        std::size_t at = cluster;
        sg.codepoint = at < text.size() ? decode_utf8(text, at) : 0;
        if (sg.codepoint == '\n' || sg.codepoint == '\r') continue;
        sg.glyph_index = infos[i].codepoint;
        if (sg.glyph_index == 0 && sg.codepoint != ' ' && fallback != 0) sg.glyph_index = fallback;
        sg.glyph = glyph_by_index(sg.glyph_index, px_size);
        sg.x = pen + static_cast<float>(pos[i].x_offset) / 64.0f;
        sg.y = -static_cast<float>(pos[i].y_offset) / 64.0f;
        sg.byte_offset = cluster;
        pen += static_cast<float>(pos[i].x_advance) / 64.0f;
        out.push_back(sg);
    }
    return out;
}

float Font::measure(std::string_view text, float px_size) {
    Impl& im = *impl_;
    if (text.empty()) return 0;
    std::string key(text);
    key.push_back('\0');
    key += std::to_string(static_cast<int>(px_size * 64.0f));
    if (auto it = im.measure_cache.find(key); it != im.measure_cache.end()) return it->second;
    std::vector<ShapedGlyph> run = shape(text, px_size);
    float width = 0;
    if (!run.empty()) {
        // The run's advance: pen position after the last glyph.
        im.set_size(px_size);
        unsigned n = 0;
        hb_glyph_position_t* pos = hb_buffer_get_glyph_positions(im.hb_buf, &n);
        for (unsigned i = 0; i < n; ++i) width += static_cast<float>(pos[i].x_advance) / 64.0f;
    }
    if (im.measure_cache.size() > 8192) im.measure_cache.clear();
    im.measure_cache.emplace(std::move(key), width);
    return width;
}

WGPUTextureView Font::atlas_view() {
    impl_->upload();
    return impl_->view;
}

std::uint32_t Font::atlas_size() const { return impl_->atlas_size; }
std::size_t Font::glyph_count() const { return impl_->glyphs.size(); }
const std::string& Font::family() const { return impl_->family; }

}  // namespace pocket::ui
