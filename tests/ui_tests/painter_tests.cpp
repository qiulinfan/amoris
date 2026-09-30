#include <pocket/core/core.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/ui/bidi.hpp>
#include <pocket/ui/font.hpp>
#include <pocket/ui/painter.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <algorithm>
#include <filesystem>
#include <set>

using namespace pocket;

namespace {

std::filesystem::path root() {
    const char* r = std::getenv("POCKET_ROOT");
    REQUIRE(r != nullptr);
    return r;
}

std::filesystem::path font_path() { return root() / ".pocket" / "deps" / "noto-sans-cjk-2.004" / "NotoSansCJKsc-Regular.otf"; }

struct Pixel { int r, g, b, a; };
Pixel at(const rhi::Image& img, std::uint32_t x, std::uint32_t y) {
    std::size_t i = (static_cast<std::size_t>(y) * img.width + x) * 4;
    return {img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]};
}

}  // namespace

TEST_CASE("utf8 decoding", "[ui]") {
    std::string s = "a\xC3\xA9\xE4\xBD\xA0\xF0\x9F\x98\x80";  // a é 你 😀
    std::size_t i = 0;
    REQUIRE(ui::decode_utf8(s, i) == 'a');
    REQUIRE(ui::decode_utf8(s, i) == 0xE9);
    REQUIRE(ui::decode_utf8(s, i) == 0x4F60);
    REQUIRE(ui::decode_utf8(s, i) == 0x1F600);
    REQUIRE(i == s.size());
    std::string bad = "\xFF";
    i = 0;
    REQUIRE(ui::decode_utf8(bad, i) == 0xFFFD);
}

TEST_CASE("font measures latin and cjk text", "[ui]") {
    rhi::Config rc;
    rc.width = 64;
    rc.height = 64;
    auto device = rhi::Device::create(rc);
    REQUIRE(device.has_value());
    auto font = ui::Font::load(**device, font_path().string());
    REQUIRE(font.has_value());
    ui::Font& f = **font;
    REQUIRE(f.family().find("Noto") != std::string::npos);
    ui::TextMetrics m = f.metrics(16);
    REQUIRE(m.ascent > 10);
    REQUIRE(m.line_height > 16);
    float latin = f.measure("Hello", 16);
    float cjk = f.measure("你好", 16);
    REQUIRE(latin > 20);
    REQUIRE(cjk == Catch::Approx(32).margin(2));  // CJK glyphs are one em wide
    REQUIRE(f.measure("Hello", 32) == Catch::Approx(latin * 2).margin(6));  // hinted advances round per glyph
    REQUIRE(f.glyph_count() >= 6);
    const ui::Glyph& g = f.glyph('H', 16);
    REQUIRE_FALSE(g.empty);
    REQUIRE(g.height > 8);
    REQUIRE(f.glyph(' ', 16).empty);
}

TEST_CASE("painter draws rectangles, borders and text into a capture", "[ui]") {
    rhi::Config rc;
    rc.width = 256;
    rc.height = 128;
    auto device = rhi::Device::create(rc);
    REQUIRE(device.has_value());
    rhi::Device& d = **device;
    auto font = ui::Font::load(d, font_path().string());
    REQUIRE(font.has_value());
    auto painter = ui::Painter::create(d, **font);
    REQUIRE(painter.has_value());
    ui::Painter& p = **painter;

    auto frame = d.begin_frame();
    REQUIRE(frame.has_value());
    // Clear to dark blue through the main pass, then paint over it.
    WGPURenderPassEncoder pass = d.begin_main_pass(*frame, {0.0f, 0.0f, 0.25f, 1.0f});
    wgpuRenderPassEncoderEnd(pass);
    wgpuRenderPassEncoderRelease(pass);
    p.begin(256, 128, 1.0f);
    p.rect({10, 10, 100, 50}, ui::Color{1, 0, 0, 1});                 // solid red
    p.rect({120, 10, 100, 50}, ui::Color{0, 1, 0, 1}, 12);            // rounded green
    p.border({10, 70, 100, 40}, ui::Color{1, 1, 1, 1}, 4, 6);         // white ring
    p.push_clip({120, 70, 40, 40});
    p.rect({100, 60, 150, 60}, ui::Color{1, 1, 0, 1});                // yellow, clipped to 40x40
    p.pop_clip();
    float w = p.text(130, 76, "Hi 你好", 18, ui::Color{1, 1, 1, 1});
    REQUIRE(w > 40);
    REQUIRE(p.flush(*frame).has_value());
    REQUIRE(d.end_frame(*frame).has_value());
    auto img = d.capture();
    REQUIRE(img.has_value());

    Pixel red = at(*img, 50, 30);
    REQUIRE(red.r > 240); REQUIRE(red.g < 10);
    Pixel green_center = at(*img, 170, 35);
    REQUIRE(green_center.g > 240);
    Pixel green_corner = at(*img, 121, 11);  // outside the rounded corner: background shows
    REQUIRE(green_corner.g < 60); REQUIRE(green_corner.b > 40);
    Pixel ring_inside = at(*img, 60, 90);   // inside the border ring: background
    REQUIRE(ring_inside.r < 40); REQUIRE(ring_inside.b > 40);
    Pixel ring_edge = at(*img, 12, 90);     // on the ring: white
    REQUIRE(ring_edge.r > 200); REQUIRE(ring_edge.g > 200);
    Pixel clipped_in = at(*img, 130, 100);  // inside the clip: yellow (text may overlap elsewhere)
    REQUIRE(clipped_in.r > 200); REQUIRE(clipped_in.g > 200); REQUIRE(clipped_in.b < 60);
    Pixel clipped_out = at(*img, 110, 100); // outside the clip but inside the yellow rect: background
    REQUIRE(clipped_out.r < 40);
    // Text: some pixel in the text box is bright.
    bool text_found = false;
    for (std::uint32_t y = 76; y < 100 && !text_found; ++y)
        for (std::uint32_t x = 165; x < 220 && !text_found; ++x) {
            Pixel px = at(*img, x, y);
            if (px.r > 150 && px.g > 150 && px.b > 150) text_found = true;
        }
    REQUIRE(text_found);
    REQUIRE(p.draw_count() >= 2);
    std::filesystem::create_directories(root() / "build" / "test-out");
    (void)fs::write_bytes(root() / "build" / "test-out" / "painter.raw", img->rgba.data(), img->rgba.size());
}

TEST_CASE("shaping joins arabic letters and ligates latin pairs", "[ui][shaping]") {
    rhi::Config rc;
    rc.width = 64;
    rc.height = 64;
    auto device = rhi::Device::create(rc);
    REQUIRE(device.has_value());
    rhi::Device& d = **device;
    // Arabic: four letters (seen, lam, alef, meem) shaped as one run take contextual forms and
    // come out right-to-left in visual order.
    std::filesystem::path arabic = root() / ".pocket" / "deps" / "noto-sans-arabic-2.010" / "NotoSansArabic-Regular.ttf";
    auto af = ui::Font::load(d, arabic.string());
    REQUIRE(af.has_value());
    const std::string word = "\xd8\xb3\xd9\x84\xd8\xa7\xd9\x85";  // سلام
    auto joined = (*af)->shape(word, 24);
    std::string dump;
    for (const auto& sg : joined) dump += "#" + std::to_string(sg.glyph_index) + "@" + std::to_string(sg.byte_offset) + " ";
    INFO("joined run: " << dump);
    REQUIRE(joined.size() <= 4);
    float isolated = 0;
    std::vector<std::uint32_t> isolated_ids;
    std::string idump;
    for (std::size_t i = 0; i < word.size(); i += 2) {
        auto one = (*af)->shape(word.substr(i, 2), 24);
        REQUIRE(one.size() == 1);
        isolated_ids.push_back(one[0].glyph_index);
        idump += "#" + std::to_string(one[0].glyph_index) + " ";
        isolated += (*af)->measure(word.substr(i, 2), 24);
    }
    INFO("isolated: " << idump);
    // Seen, lam and alef take joined forms; the meem after a non-joining alef stays isolated.
    int changed = 0;
    for (const auto& sg : joined) {
        REQUIRE(sg.glyph_index != 0);
        if (std::find(isolated_ids.begin(), isolated_ids.end(), sg.glyph_index) == isolated_ids.end()) ++changed;
    }
    REQUIRE(changed >= 3);
    REQUIRE((*af)->measure(word, 24) < isolated);
    // Right-to-left comes out in visual order: the first glyph drawn (leftmost) is the last letter (meem).
    REQUIRE(joined.front().byte_offset == 6);
    REQUIRE(joined.back().byte_offset == 0);
    // Latin in the CJK face: kerning never widens a pair, and byte offsets survive shaping.
    auto lf = ui::Font::load(d, font_path().string());
    REQUIRE(lf.has_value());
    REQUIRE((*lf)->measure("AV", 32) <= (*lf)->measure("A", 32) + (*lf)->measure("V", 32) + 0.01f);
    auto hello = (*lf)->shape("Hi 世界", 20);
    REQUIRE(hello.size() == 5);
    REQUIRE(hello[3].byte_offset == 3);
    REQUIRE(hello[4].byte_offset == 6);
    REQUIRE(hello[4].x > hello[3].x);
    // A line break inside a run shapes to nothing but keeps offsets for what follows.
    auto lines = (*lf)->shape("a\nb", 20);
    REQUIRE(lines.size() == 2);
    REQUIRE(lines[1].byte_offset == 2);
}

TEST_CASE("an image sampled by the nearest texel scales into blocks where linear sampling blends", "[ui][nearest]") {
    rhi::Config rc;
    rc.width = 256;
    rc.height = 128;
    auto device = rhi::Device::create(rc);
    REQUIRE(device.has_value());
    rhi::Device& d = **device;
    auto font = ui::Font::load(d, font_path().string());
    REQUIRE(font.has_value());
    auto painter = ui::Painter::create(d, **font);
    REQUIRE(painter.has_value());
    ui::Painter& p = **painter;
    // A 2 by 2 picture: red, green over blue, white.
    WGPUTextureDescriptor td{};
    td.label = rhi::str("test.pixels");
    td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst;
    td.dimension = WGPUTextureDimension_2D;
    td.size = {2, 2, 1};
    td.format = WGPUTextureFormat_RGBA8Unorm;
    td.mipLevelCount = 1;
    td.sampleCount = 1;
    WGPUTexture tex = wgpuDeviceCreateTexture(d.device(), &td);
    REQUIRE(tex != nullptr);
    const std::uint8_t pixels[16] = {255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255};
    WGPUTexelCopyTextureInfo dst{};
    dst.texture = tex;
    dst.aspect = WGPUTextureAspect_All;
    WGPUTexelCopyBufferLayout layout{};
    layout.bytesPerRow = 8;
    layout.rowsPerImage = 2;
    WGPUExtent3D ext{2, 2, 1};
    wgpuQueueWriteTexture(d.queue(), &dst, pixels, sizeof pixels, &layout, &ext);
    WGPUTextureViewDescriptor vd{};
    vd.format = td.format;
    vd.dimension = WGPUTextureViewDimension_2D;
    vd.mipLevelCount = 1;
    vd.arrayLayerCount = 1;
    vd.aspect = WGPUTextureAspect_All;
    vd.usage = td.usage;
    WGPUTextureView view = wgpuTextureCreateView(tex, &vd);
    REQUIRE(view != nullptr);

    auto frame = d.begin_frame();
    REQUIRE(frame.has_value());
    WGPURenderPassEncoder pass = d.begin_main_pass(*frame, {0.0f, 0.0f, 0.0f, 1.0f});
    wgpuRenderPassEncoderEnd(pass);
    wgpuRenderPassEncoderRelease(pass);
    p.begin(256, 128, 1.0f);
    p.image({0, 0, 64, 64}, view, 0, 0, 1, 1, ui::Color{1, 1, 1, 1}, 0, true);    // nearest
    p.image({64, 0, 64, 64}, view, 0, 0, 1, 1, ui::Color{1, 1, 1, 1}, 0, false);  // linear
    REQUIRE(p.flush(*frame).has_value());
    REQUIRE(d.end_frame(*frame).has_value());
    auto img = d.capture();
    REQUIRE(img.has_value());
    // Near the middle of the red block (uv 0.47): nearest keeps it red, linear blends it with its neighbours.
    Pixel nearest = at(*img, 30, 30);
    REQUIRE(nearest.r > 240);
    REQUIRE(nearest.g < 10);
    REQUIRE(nearest.b < 10);
    Pixel linear = at(*img, 94, 30);
    REQUIRE(linear.g > 60);
    REQUIRE(linear.b > 60);
    // The blocks' corners are the pure texels either way.
    Pixel corner = at(*img, 2, 61);
    REQUIRE(corner.b > 240);
    REQUIRE(corner.r < 10);
    wgpuTextureViewRelease(view);
    wgpuTextureRelease(tex);
}

TEST_CASE("right-to-left text: a line's runs in the order they are seen, each shaped its way", "[ui][bidi]") {
    const std::string salam = "\xd8\xb3\xd9\x84\xd8\xa7\xd9\x85";   // سلام, four Arabic letters of two bytes
    const std::string shalom = "\xd7\xa9\xd7\x9c\xd7\x95\xd7\x9d";  // שלום, four Hebrew letters
    auto runs = [](std::string_view t) {
        std::string out;
        for (const ui::BidiRun& r : ui::bidi_runs(t)) out += "[" + std::to_string(r.start) + "," + std::to_string(r.end) + ")" + std::to_string(r.level) + " ";
        return out;
    };
    REQUIRE_FALSE(ui::rtl_line("Score 20"));
    REQUIRE(ui::rtl_line("  " + salam));
    REQUIRE(ui::rtl_line("123 " + shalom + " abc"));   // the first strong letter decides; numbers are not strong
    REQUIRE(runs("Score 20") == "[0,8)0 ");
    // Arabic then its number: the digits after Arabic letters are an Arabic number, kept left to
    // right, and seen at the left of the right-to-left line.
    REQUIRE(runs(salam + " 123") == "[9,12)2 [0,9)1 ");
    // Arabic in an English line: in its place, turned around; the spaces round it stay English.
    REQUIRE(runs("Level 3: " + salam) == "[0,9)0 [9,17)1 ");
    REQUIRE(runs("a " + shalom + " b") == "[0,2)0 [2,10)1 [10,12)0 ");
    // Two Hebrew words with an English one between, in a Hebrew line: the Hebrew words swap sides
    // and the English stays readable in the middle.
    REQUIRE(runs(shalom + " abc " + shalom) == "[12,21)1 [9,12)2 [0,9)1 ");
    // A Hebrew line ending in spaces keeps them at the line's end, its left; an English word
    // ending a Hebrew line leaves its trailing space there too, not between the words.
    REQUIRE(runs(shalom + "  ") == "[0,10)1 ");
    REQUIRE(runs(shalom + " abc ") == "[12,13)1 [9,12)2 [0,9)1 ");
    // A right-to-left sentence ending in an English word and a full stop: the stop is the
    // sentence's, at its end, the far left.
    REQUIRE(runs(shalom + " WASD.") == "[13,14)1 [9,13)2 [0,9)1 ");
    // Types from the character database: guillemets are neutral, so a quoted Hebrew line is right
    // to left; a right-to-left mark is a strong letter of its own; format characters (a joiner, a
    // zero-width space) ride along with their neighbours and break no run.
    const std::string quoted = "\xc2\xab" + shalom + "\xc2\xbb";
    REQUIRE(ui::rtl_line(quoted));
    REQUIRE(runs(quoted) == "[0,12)1 ");
    REQUIRE(ui::rtl_line("\xe2\x80\x8fScore 20"));
    REQUIRE(runs("\xe2\x80\x8fScore 20") == "[3,11)2 [0,3)1 ");
    REQUIRE(runs(salam.substr(0, 2) + "\xe2\x80\x8d" + salam.substr(2)) == "[0,11)1 ");
    REQUIRE(runs("\xe2\x80\x8b" "abc") == "[0,6)0 ");
    // Shaped: the digits first (leftmost) in their order, then the Arabic from its last letter.
    rhi::Config rc;
    rc.width = 64;
    rc.height = 64;
    auto device = rhi::Device::create(rc);
    REQUIRE(device.has_value());
    auto af = ui::Font::load(**device, (root() / ".pocket" / "deps" / "noto-sans-arabic-2.010" / "NotoSansArabic-Regular.ttf").string());
    REQUIRE(af.has_value());
    const std::string line = salam + " 123";
    auto glyphs = (*af)->shape(line, 24);
    std::string order;
    for (const auto& g : glyphs) order += std::to_string(g.byte_offset) + " ";
    INFO("visual order: " << order);
    REQUIRE(glyphs.size() >= 7);
    REQUIRE(glyphs[0].byte_offset == 9);
    REQUIRE(glyphs[1].byte_offset == 10);
    REQUIRE(glyphs[2].byte_offset == 11);
    REQUIRE(glyphs.back().byte_offset == 0);
    for (std::size_t k = 1; k < glyphs.size(); ++k) REQUIRE(glyphs[k].x >= glyphs[k - 1].x);   // left to right on the line
    // The whole line's width is every run's advance.
    REQUIRE((*af)->measure(line, 24) == Catch::Approx((*af)->measure(salam, 24) + (*af)->measure(" 123", 24)).margin(1.0));
}

TEST_CASE("fallback fonts draw what the first font lacks: Arabic in an interface drawn with a Chinese face", "[ui][fallback]") {
    rhi::Config rc;
    rc.width = 64;
    rc.height = 64;
    auto device = rhi::Device::create(rc);
    REQUIRE(device.has_value());
    const std::string salam = "\xd8\xb3\xd9\x84\xd8\xa7\xd9\x85";
    const std::string line = "Hi " + salam + " \xe4\xb8\x96\xe7\x95\x8c";   // Hi سلام 世界
    auto count_distinct = [&](const std::vector<ui::ShapedGlyph>& gs, std::size_t from, std::size_t to) {
        std::set<std::uint32_t> ids;
        for (const auto& g : gs) if (g.byte_offset >= from && g.byte_offset < to) ids.insert(g.glyph_index);
        return ids.size();
    };
    auto cjk = ui::Font::load(**device, font_path().string());
    REQUIRE(cjk.has_value());
    // Alone, the Chinese face has no Arabic: its four letters are one replacement glyph.
    auto without = (*cjk)->shape(line, 24);
    REQUIRE(count_distinct(without, 3, 11) == 1);
    const float narrow = (*cjk)->measure(line, 24);
    // With Noto Sans Arabic after it, they are Arabic letters, joined, right to left; the Latin and
    // the Chinese stay the first face's.
    REQUIRE((*cjk)->add_fallback((root() / ".pocket" / "deps" / "noto-sans-arabic-2.010" / "NotoSansArabic-Regular.ttf").string()).has_value());
    REQUIRE((*cjk)->fallback_count() == 1);
    auto with = (*cjk)->shape(line, 24);
    std::string dump;
    for (const auto& g : with) dump += std::to_string(g.byte_offset) + ":" + std::to_string(g.glyph_index) + " ";
    INFO(dump);
    REQUIRE(count_distinct(with, 3, 11) >= 3);
    REQUIRE((*cjk)->measure(line, 24) != Catch::Approx(narrow));
    // In visual order: H, i, space, the Arabic from its last letter, space, the Chinese.
    REQUIRE(with[0].byte_offset == 0);
    REQUIRE(with[1].byte_offset == 1);
    std::vector<std::size_t> arabic;
    for (const auto& g : with) if (g.byte_offset >= 3 && g.byte_offset < 11) arabic.push_back(g.byte_offset);
    REQUIRE(arabic.front() > arabic.back());
    REQUIRE(with.back().byte_offset == 15);   // 界, last
    for (std::size_t k = 1; k < with.size(); ++k) REQUIRE(with[k].x >= with[k - 1].x);
}

TEST_CASE("a line laid out for editing puts carets on the glyphs' edges in either direction", "[ui][caretlayout]") {
    rhi::Config rc;
    rc.width = 64;
    rc.height = 64;
    auto device = rhi::Device::create(rc);
    REQUIRE(device.has_value());
    auto af = ui::Font::load(**device, (root() / ".pocket" / "deps" / "noto-sans-arabic-2.010" / "NotoSansArabic-Regular.ttf").string());
    REQUIRE(af.has_value());
    ui::Font& font = **af;
    const std::string salam = "\xd8\xb3\xd9\x84\xd8\xa7\xd9\x85";   // سلام: seen, lam, alef, meem
    using Catch::Approx;
    // Left to right: carets from the left end, one after another; a point is taken to the nearest.
    const ui::LineLayout l = font.layout("abc", 20);
    REQUIRE_FALSE(l.rtl);
    REQUIRE(l.caret_x(0) == Approx(0));
    REQUIRE(l.caret_x(3) == Approx(l.width));
    REQUIRE(l.caret_x(1) < l.caret_x(2));
    REQUIRE(l.offset_at(-5) == 0);
    REQUIRE(l.offset_at(l.width + 5) == 3);
    REQUIRE(l.offset_at(l.caret_x(1) + 0.5f) == 1);
    // Right to left: the first letter at the right end, each caret left of the one before (lam
    // and alef make one ligature, whose caret between them is shared out), a point past the
    // right end is the start.
    const ui::LineLayout r = font.layout(salam, 20);
    REQUIRE(r.rtl);
    REQUIRE(r.caret_x(0) == Approx(r.width));
    REQUIRE(r.caret_x(8) == Approx(0));
    for (std::size_t k = 2; k <= 8; k += 2) REQUIRE(r.caret_x(k) < r.caret_x(k - 2));
    REQUIRE(r.offset_at(r.width + 5) == 0);
    REQUIRE(r.offset_at(-5) == 8);
    REQUIRE(r.offset_at(r.caret_x(2) - 0.5f) == 2);
    // Mixed, in a left-to-right paragraph: the Arabic keeps its place on the right, turned around,
    // so "b", the space and the first Arabic letter are two stretches apart.
    const ui::LineLayout m = font.layout("ab " + salam, 20);
    REQUIRE(m.mixed());
    REQUIRE_FALSE(m.rtl);
    const float arabic_left = m.width - font.measure(salam, 20);
    REQUIRE(m.caret_x(3) == Approx(arabic_left).margin(0.5));   // after the space: English goes on there, left of the Arabic
    REQUIRE(m.caret_x(9) > arabic_left + 1);        // before meem: its right edge, the word's last letter at its left end
    REQUIRE(m.caret_x(9) < m.caret_x(5));
    REQUIRE(m.caret_x(11) == Approx(m.width));   // the end: the paragraph's end side, where more English goes
    REQUIRE(m.offset_at(m.width + 5) == 11);
    const auto spans = m.spans(1, 5);
    REQUIRE(spans.size() == 2);
    REQUIRE(spans[0].first == Approx(m.caret_x(1)));
    REQUIRE(spans[0].second == Approx(arabic_left).margin(0.5));
    REQUIRE(spans[1].second == Approx(m.width));
    REQUIRE(m.spans(0, 11).size() == 1);
    // The same line in a right-to-left paragraph: the Latin word goes to the right end.
    const ui::LineLayout mr = font.layout("ab " + salam, 20, ui::TextDirection::Rtl);
    REQUIRE(mr.rtl);
    REQUIRE(mr.caret_x(1) > mr.width * 0.5f);   // between a and b: on the right
    REQUIRE(mr.caret_x(2) < mr.caret_x(1) - 5);   // after "ab": left of it, where the paragraph's Arabic goes on
    REQUIRE(mr.caret_x(11) == Approx(0));
    REQUIRE(mr.offset_at(mr.width + 5) == 0);
    // Every boundary has a place of its own, so a click or an arrow can reach each one.
    std::set<int> places;
    for (std::size_t s : m.stops) places.insert(static_cast<int>(std::lround(m.caret_x(s) * 4)));
    REQUIRE(places.size() == m.stops.size());
    // In a right-to-left run a bracket or other mirrored pair is drawn mirrored (UAX #9 L4).
    REQUIRE(font.shape("<", 20, ui::TextDirection::Rtl)[0].glyph_index == font.shape(">", 20)[0].glyph_index);
}
