#include <pocket/core/core.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/ui/document.hpp>
#include <pocket/ui/font.hpp>
#include <pocket/ui/painter.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <filesystem>

using namespace pocket;

namespace {

std::filesystem::path root() {
    const char* r = std::getenv("POCKET_ROOT");
    REQUIRE(r != nullptr);
    return r;
}

std::filesystem::path font_path() { return root() / ".pocket" / "deps" / "noto-sans-cjk-2.004" / "NotoSansCJKsc-Regular.otf"; }

struct Fixture {
    std::unique_ptr<rhi::Device> device;
    std::unique_ptr<ui::Font> font;
    std::unique_ptr<ui::Document> doc;
    Fixture(std::uint32_t w = 320, std::uint32_t h = 200) {
        rhi::Config rc;
        rc.width = w;
        rc.height = h;
        auto d = rhi::Device::create(rc);
        REQUIRE(d.has_value());
        device = std::move(*d);
        auto f = ui::Font::load(*device, font_path().string());
        REQUIRE(f.has_value());
        font = std::move(*f);
        doc = std::make_unique<ui::Document>(*font);
    }
    void apply(const Json& ops) {
        auto r = doc->apply(ops);
        std::string err = r ? std::string() : r.error().to_string();
        INFO(err);
        REQUIRE(r.has_value());
    }
    void layout() { doc->layout(320, 200, 1.0f); }
};

platform::Event mouse(platform::EventType t, float x, float y, int button = 1) {
    platform::Event e;
    e.type = t;
    e.x = x;
    e.y = y;
    e.button = button;
    return e;
}

}  // namespace

TEST_CASE("flex layout: row with fixed and flexible children", "[ui]") {
    Fixture f;
    f.apply(Json::parse(R"([
        ["create", 10, "box"], ["set", 10, {"direction": "row", "width": "100%", "height": 50, "padding": 5, "gap": 10}],
        ["append", 1, 10],
        ["create", 11, "box"], ["set", 11, {"width": 60, "height": "100%"}], ["append", 10, 11],
        ["create", 12, "box"], ["set", 12, {"flex": 1}], ["append", 10, 12],
        ["create", 13, "box"], ["set", 13, {"width": 40}], ["append", 10, 13]
    ])"));
    f.layout();
    ui::Rect a = f.doc->rect_of(11), b = f.doc->rect_of(12), c = f.doc->rect_of(13);
    REQUIRE(a.x == 5); REQUIRE(a.y == 5); REQUIRE(a.w == 60); REQUIRE(a.h == 40);
    REQUIRE(b.x == 75); REQUIRE(b.w == Catch::Approx(320 - 5 - 60 - 10 - 10 - 40 - 5));
    REQUIRE(c.x == Catch::Approx(320 - 5 - 40)); REQUIRE(c.w == 40);
    REQUIRE(c.h == 40);  // stretch by default
}

TEST_CASE("text measures and wraps", "[ui]") {
    Fixture f;
    f.apply(Json::parse(R"([
        ["create", 20, "text"], ["set", 20, {"fontSize": 16, "alignSelf": "start"}], ["text", 20, "Hello world"], ["append", 1, 20],
        ["create", 21, "box"], ["set", 21, {"width": 80}], ["append", 1, 21],
        ["create", 22, "text"], ["set", 22, {"fontSize": 16, "textWrap": true}], ["text", 22, "one two three four five six"], ["append", 21, 22],
        ["create", 23, "text"], ["set", 23, {"fontSize": 16, "textWrap": true}], ["text", 23, "你好世界你好世界你好世界"], ["append", 21, 23]
    ])"));
    f.layout();
    ui::Rect single = f.doc->rect_of(20);
    REQUIRE(single.w > 60);
    REQUIRE(single.w < 120);
    REQUIRE(single.h >= 16);
    ui::Rect wrapped = f.doc->rect_of(22);
    REQUIRE(wrapped.w <= 80);
    REQUIRE(wrapped.h > single.h * 2.5);  // at least three lines
    ui::Rect cjk = f.doc->rect_of(23);
    REQUIRE(cjk.w <= 80);
    REQUIRE(cjk.h > single.h * 1.5);  // per-character breaks: 12 chars at 16px in 80px => 3 lines
    Json d = f.doc->describe(22);
    REQUIRE(d["text"] == "one two three four five six");
}

TEST_CASE("hit testing, clicks and listeners", "[ui]") {
    Fixture f;
    f.apply(Json::parse(R"([
        ["create", 30, "box"], ["set", 30, {"position": "absolute", "left": 10, "top": 10, "width": 100, "height": 40, "on": ["click"], "name": "play"}], ["append", 1, 30],
        ["create", 31, "text"], ["text", 31, "Play"], ["append", 30, 31],
        ["create", 32, "box"], ["set", 32, {"position": "absolute", "left": 150, "top": 10, "width": 100, "height": 40, "display": "none"}], ["append", 1, 32],
        ["create", 33, "box"], ["set", 33, {"position": "absolute", "left": 10, "top": 100, "width": 100, "height": 40, "overflow": "hidden"}], ["append", 1, 33],
        ["create", 34, "box"], ["set", 34, {"width": 300, "height": 20, "on": ["click"]}], ["append", 33, 34]
    ])"));
    f.layout();
    REQUIRE(f.doc->hit_test(15, 15) == 31);   // the text inside the button is on top
    REQUIRE(f.doc->hit_test(100, 45) == 30);
    REQUIRE(f.doc->hit_test(200, 20) == 0);   // hidden box does not hit
    REQUIRE(f.doc->hit_test(300, 190) == 0);  // root is not reported
    REQUIRE(f.doc->hit_test(50, 105) == 34);
    REQUIRE(f.doc->hit_test(150, 105) == 0);  // clipped by the overflow:hidden parent

    bool text_wanted = false;
    auto events = f.doc->handle_events({mouse(platform::EventType::MouseMove, 20, 20), mouse(platform::EventType::MouseDown, 20, 20), mouse(platform::EventType::MouseUp, 20, 20)}, text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "click");
    REQUIRE(events[0]["id"] == 30);  // bubbled from the text to the listening box
    REQUIRE(events[0]["name"] == "play");
    REQUIRE_FALSE(text_wanted);

    // Press on one element, release on another: no click.
    events = f.doc->handle_events({mouse(platform::EventType::MouseDown, 20, 20), mouse(platform::EventType::MouseMove, 20, 110), mouse(platform::EventType::MouseUp, 20, 110)}, text_wanted);
    for (auto& e : events) REQUIRE(e["type"] != "click");
    // An element without listeners produces nothing.
    events = f.doc->handle_events({mouse(platform::EventType::MouseDown, 300, 190), mouse(platform::EventType::MouseUp, 300, 190)}, text_wanted);
    REQUIRE(events.empty());
}

TEST_CASE("inputs take focus, text and keys", "[ui]") {
    Fixture f;
    f.apply(Json::parse(R"([
        ["create", 40, "input"], ["set", 40, {"position": "absolute", "left": 10, "top": 10, "width": 150, "placeholder": "name", "on": ["input", "change", "focus"]}], ["append", 1, 40]
    ])"));
    f.layout();
    ui::Rect r = f.doc->rect_of(40);
    REQUIRE(r.h > 16);
    bool text_wanted = false;
    auto events = f.doc->handle_events({mouse(platform::EventType::MouseDown, 20, 20), mouse(platform::EventType::MouseUp, 20, 20)}, text_wanted);
    REQUIRE(text_wanted);
    REQUIRE(f.doc->focused() == 40);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "focus");

    platform::Event t;
    t.type = platform::EventType::Text;
    t.text = "ab你";
    events = f.doc->handle_events({t}, text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "input");
    REQUIRE(events[0]["value"] == "ab你");

    platform::Event back;
    back.type = platform::EventType::KeyDown;
    back.key_name = "Backspace";
    events = f.doc->handle_events({back}, text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["value"] == "ab");  // one code point removed, not one byte

    platform::Event left = back;
    left.key_name = "Left";
    t.text = "X";
    events = f.doc->handle_events({left, t}, text_wanted);
    REQUIRE(events.back()["value"] == "aXb");

    platform::Event enter = back;
    enter.key_name = "Return";
    events = f.doc->handle_events({enter}, text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "change");
    REQUIRE(events[0]["value"] == "aXb");

    // Clicking elsewhere blurs and commits.
    events = f.doc->handle_events({mouse(platform::EventType::MouseDown, 300, 180), mouse(platform::EventType::MouseUp, 300, 180)}, text_wanted);
    REQUIRE_FALSE(text_wanted);
    REQUIRE(f.doc->focused() == 0);
    bool saw_blur = false, saw_change = false;
    for (auto& e : events) { if (e["type"] == "blur") saw_blur = true; if (e["type"] == "change") saw_change = true; }
    REQUIRE(saw_blur);
    REQUIRE(saw_change);
    REQUIRE(f.doc->describe(40)["value"] == "aXb");
}

TEST_CASE("scrolling clamps and moves children", "[ui]") {
    Fixture f;
    Json ops = Json::parse(R"([
        ["create", 50, "box"], ["set", 50, {"position": "absolute", "left": 0, "top": 0, "width": 100, "height": 100, "overflow": "scroll"}], ["append", 1, 50]
    ])");
    for (int i = 0; i < 10; ++i) {
        ops.push_back(Json::array({"create", 51 + i, "box"}));
        ops.push_back(Json::array({"set", 51 + i, Json{{"height", 30}}}));
        ops.push_back(Json::array({"append", 50, 51 + i}));
    }
    f.apply(ops);
    f.layout();
    REQUIRE(f.doc->rect_of(51).y == 0);
    REQUIRE(f.doc->describe(50)["contentHeight"].get<float>() == Catch::Approx(300));
    bool tw = false;
    platform::Event wheel;
    wheel.type = platform::EventType::MouseWheel;
    wheel.dy = -2;  // scroll down 48 points
    (void)f.doc->handle_events({mouse(platform::EventType::MouseMove, 50, 50), wheel}, tw);
    f.layout();
    REQUIRE(f.doc->rect_of(51).y == Catch::Approx(-48));
    f.apply(Json::parse(R"([["set", 50, {"scrollTop": 10000}]])"));
    f.layout();
    REQUIRE(f.doc->describe(50)["scrollTop"].get<float>() == Catch::Approx(200));  // clamped to content - height
    REQUIRE(f.doc->hit_test(50, 50) == 59);  // child 9 (y 240..270 - 200 = 40..70)
}

TEST_CASE("snapshot, query and removal", "[ui]") {
    Fixture f;
    f.apply(Json::parse(R"([
        ["create", 60, "box"], ["set", 60, {"name": "toolbar", "direction": "row", "height": 30}], ["append", 1, 60],
        ["create", 61, "box"], ["set", 61, {"on": ["click"], "name": "play", "width": 60}], ["append", 60, 61],
        ["create", 62, "text"], ["text", 62, "Play"], ["append", 61, 62],
        ["create", 63, "input"], ["set", 63, {"value": "hello", "width": 80}], ["append", 60, 63]
    ])"));
    f.layout();
    std::string snap = f.doc->snapshot(ui::SnapshotOptions{});
    INFO(snap);
    REQUIRE(snap.find("box#60 toolbar") != std::string::npos);
    REQUIRE(snap.find("box#61 play") != std::string::npos);
    REQUIRE(snap.find("on=click") != std::string::npos);
    REQUIRE(snap.find("text#62") != std::string::npos);
    REQUIRE(snap.find("\"Play\"") != std::string::npos);
    REQUIRE(snap.find("input#63") != std::string::npos);
    REQUIRE(snap.find("value=\"hello\"") != std::string::npos);
    ui::SnapshotOptions shallow;
    shallow.depth = 1;
    std::string s2 = f.doc->snapshot(shallow);
    REQUIRE(s2.find("(2 children)") != std::string::npos);
    REQUIRE(s2.find("text#62") == std::string::npos);

    Json q = f.doc->query(Json{{"text", "Play"}});
    REQUIRE(q.size() == 1);
    REQUIRE(q[0]["id"] == 62);
    q = f.doc->query(Json{{"name", "play"}});
    REQUIRE(q.size() == 1);
    REQUIRE(q[0]["id"] == 61);
    q = f.doc->query(Json{{"type", "input"}});
    REQUIRE(q.size() == 1);

    f.apply(Json::parse(R"([["remove", 61]])"));
    REQUIRE_FALSE(f.doc->exists(61));
    REQUIRE_FALSE(f.doc->exists(62));  // subtree freed
    REQUIRE(f.doc->node_count() == 3);  // root, toolbar, input
    auto bad = f.doc->apply(Json::parse(R"([["append", 60, 999]])"));
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().code == "ui_no_such_node");
    auto cycle = f.doc->apply(Json::parse(R"([["append", 60, 1]])"));
    REQUIRE_FALSE(cycle.has_value());
}

TEST_CASE("document paints boxes and text", "[ui]") {
    Fixture f(256, 128);
    auto painter = ui::Painter::create(*f.device, *f.font);
    REQUIRE(painter.has_value());
    f.apply(Json::parse(R"([
        ["create", 70, "box"], ["set", 70, {"position": "absolute", "left": 8, "top": 8, "width": 120, "height": 40, "background": "#ff0000", "radius": 6}], ["append", 1, 70],
        ["create", 71, "text"], ["set", 71, {"color": "#ffffff", "fontSize": 18, "position": "absolute", "left": 16, "top": 64}], ["text", 71, "Score 42"], ["append", 1, 71],
        ["create", 72, "box"], ["set", 72, {"position": "absolute", "left": 150, "top": 8, "width": 60, "height": 60, "background": "#00ff00", "opacity": 0.5}], ["append", 1, 72]
    ])"));
    f.doc->layout(256, 128, 1.0f);
    auto frame = f.device->begin_frame();
    REQUIRE(frame.has_value());
    WGPURenderPassEncoder pass = f.device->begin_main_pass(*frame, {0.0f, 0.0f, 0.0f, 1.0f});
    wgpuRenderPassEncoderEnd(pass);
    wgpuRenderPassEncoderRelease(pass);
    (*painter)->begin(256, 128, 1.0f);
    f.doc->paint(**painter);
    REQUIRE((*painter)->flush(*frame).has_value());
    REQUIRE(f.device->end_frame(*frame).has_value());
    auto img = f.device->capture();
    REQUIRE(img.has_value());
    auto px = [&](std::uint32_t x, std::uint32_t y) { std::size_t i = (static_cast<std::size_t>(y) * img->width + x) * 4; return std::array<int, 3>{img->rgba[i], img->rgba[i + 1], img->rgba[i + 2]}; };
    auto red = px(60, 28);
    REQUIRE(red[0] > 240); REQUIRE(red[1] < 10);
    auto half_green = px(180, 38);
    REQUIRE(half_green[1] > 100); REQUIRE(half_green[1] < 160);  // 50% opacity over black
    bool text_found = false;
    for (std::uint32_t y = 64; y < 90 && !text_found; ++y)
        for (std::uint32_t x = 16; x < 120 && !text_found; ++x) {
            auto p = px(x, y);
            if (p[0] > 150 && p[1] > 150 && p[2] > 150) text_found = true;
        }
    REQUIRE(text_found);
    std::filesystem::create_directories(root() / "build" / "test-out");
    (void)fs::write_bytes(root() / "build" / "test-out" / "document.raw", img->rgba.data(), img->rgba.size());
}

TEST_CASE("tab walks the focus through inputs and buttons, and Return presses the focused button", "[ui][focus]") {
    Fixture f;
    f.apply(Json::parse(R"([
        ["create", 40, "input"], ["set", 40, {"position": "absolute", "left": 10, "top": 10, "width": 100, "name": "first"}], ["append", 1, 40],
        ["create", 41, "box"], ["set", 41, {"position": "absolute", "left": 10, "top": 50, "width": 60, "height": 20, "name": "ok", "on": ["click", "focus"]}], ["append", 1, 41],
        ["create", 42, "box"], ["set", 42, {"position": "absolute", "left": 10, "top": 80, "width": 60, "height": 20, "name": "plain"}], ["append", 1, 42],
        ["create", 43, "input"], ["set", 43, {"position": "absolute", "left": 10, "top": 110, "width": 100, "name": "last"}], ["append", 1, 43]
    ])"));
    f.layout();
    bool text_wanted = false;
    platform::Event tab;
    tab.type = platform::EventType::KeyDown;
    tab.key_name = "Tab";
    // From nothing, Tab lands on the first input; then the button (a plain box is skipped); then the last input; then round again.
    auto events = f.doc->handle_events({tab}, text_wanted);
    REQUIRE(f.doc->focused() == 40);
    REQUIRE(text_wanted);
    events = f.doc->handle_events({tab}, text_wanted);
    REQUIRE(f.doc->focused() == 41);
    REQUIRE_FALSE(text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "focus");
    REQUIRE(events[0]["name"] == "ok");
    events = f.doc->handle_events({tab}, text_wanted);
    REQUIRE(f.doc->focused() == 43);
    events = f.doc->handle_events({tab}, text_wanted);
    REQUIRE(f.doc->focused() == 40);
    // Shift+Tab walks back, wrapping to the end.
    platform::Event back = tab;
    back.mods = platform::mods_from_json(Json::array({"shift"}));
    events = f.doc->handle_events({back}, text_wanted);
    REQUIRE(f.doc->focused() == 43);
    events = f.doc->handle_events({back}, text_wanted);
    REQUIRE(f.doc->focused() == 41);
    // Return on the focused button is a click at its center; on an input it is a change, not a click.
    platform::Event enter = tab;
    enter.key_name = "Return";
    events = f.doc->handle_events({enter}, text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "click");
    REQUIRE(events[0]["name"] == "ok");
    REQUIRE(events[0]["keyboard"] == true);
    REQUIRE(events[0]["x"].get<double>() == Catch::Approx(40).margin(1));
    platform::Event space = enter;
    space.key_name = "Space";
    events = f.doc->handle_events({space}, text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "click");
    // A disabled button is skipped.
    f.apply(Json::parse(R"([["set", 41, {"disabled": true}]])"));
    f.layout();
    f.doc->set_focus(40);
    events = f.doc->handle_events({tab}, text_wanted);
    REQUIRE(f.doc->focused() == 43);
}

TEST_CASE("a multi-line input takes Return as a new line, moves by lines and commits with the modifier", "[ui][multiline]") {
    Fixture f;
    f.apply(Json::parse(R"([
        ["create", 45, "input"], ["set", 45, {"position": "absolute", "left": 10, "top": 10, "width": 150, "height": 60, "multiline": true, "name": "notes", "on": ["input", "change"]}], ["append", 1, 45]
    ])"));
    f.layout();
    bool text_wanted = false;
    auto events = f.doc->handle_events({mouse(platform::EventType::MouseDown, 20, 20), mouse(platform::EventType::MouseUp, 20, 20)}, text_wanted);
    REQUIRE(f.doc->focused() == 45);
    platform::Event t;
    t.type = platform::EventType::Text;
    t.text = "ab";
    platform::Event key;
    key.type = platform::EventType::KeyDown;
    key.key_name = "Return";
    events = f.doc->handle_events({t, key}, text_wanted);
    REQUIRE(events.size() == 2);
    REQUIRE(events[1]["type"] == "input");
    REQUIRE(events[1]["value"] == "ab\n");
    t.text = "cd";
    f.doc->handle_events({t}, text_wanted);
    Json d = f.doc->describe(45);
    REQUIRE(d["value"] == "ab\ncd");
    REQUIRE(d["caret"] == 5);
    REQUIRE(d["multiline"] == true);
    // Up keeps the column on the line above; Home and End work along the line; Down comes back.
    key.key_name = "Up";
    f.doc->handle_events({key}, text_wanted);
    REQUIRE(f.doc->describe(45)["caret"] == 2);
    key.key_name = "Home";
    f.doc->handle_events({key}, text_wanted);
    REQUIRE(f.doc->describe(45)["caret"] == 0);
    key.key_name = "End";
    f.doc->handle_events({key}, text_wanted);
    REQUIRE(f.doc->describe(45)["caret"] == 2);
    key.key_name = "Down";
    f.doc->handle_events({key}, text_wanted);
    REQUIRE(f.doc->describe(45)["caret"] == 5);
    // The modifier commits; the snapshot shows the newline escaped.
    key.key_name = "Return";
    key.mods = platform::mods_from_json(Json::array({"meta"}));
    events = f.doc->handle_events({key}, text_wanted);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0]["type"] == "change");
    REQUIRE(events[0]["value"] == "ab\ncd");
    ui::SnapshotOptions so;
    std::string snap = f.doc->snapshot(so);
    INFO(snap);
    REQUIRE(snap.find("value=\"ab\\ncd\"") != std::string::npos);
    REQUIRE(snap.find("multiline") != std::string::npos);
    // A click lands on the line under it, at the nearest boundary; the same value set again keeps the caret.
    f.doc->handle_events({mouse(platform::EventType::MouseDown, 12, 38), mouse(platform::EventType::MouseUp, 12, 38)}, text_wanted);
    REQUIRE(f.doc->describe(45)["caret"] == 3);
    f.apply(Json::parse(R"([["set", 45, {"value": "ab\ncd"}]])"));
    REQUIRE(f.doc->describe(45)["caret"] == 3);
    f.apply(Json::parse(R"([["set", 45, {"value": "ab\ncd\n"}]])"));
    REQUIRE(f.doc->describe(45)["caret"] == 6);
    // Measured height grows with the lines when no height is set.
    f.apply(Json::parse(R"([["create", 46, "input"], ["set", 46, {"position": "absolute", "left": 10, "top": 100, "width": 150, "multiline": true, "value": "one\ntwo\nthree"}], ["append", 1, 46]])"));
    f.layout();
    REQUIRE(f.doc->rect_of(46).h > f.doc->rect_of(45).h * 0.5f);
    REQUIRE(f.doc->rect_of(46).h >= 3 * 13);
}
