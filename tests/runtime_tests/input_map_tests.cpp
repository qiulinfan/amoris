#include <catch_amalgamated.hpp>
#include <pocket/app/input_map.hpp>

using namespace pocket;

namespace {

platform::Event key(platform::EventType type, const char* name) {
    platform::Event e;
    e.type = type;
    e.key_name = name;
    return e;
}

platform::Event pad_button(const char* name, bool pressed) {
    platform::Event e;
    e.type = platform::EventType::PadButton;
    e.key_name = name;
    e.pressed = pressed;
    return e;
}

platform::Event pad_axis(const char* name, float value) {
    platform::Event e;
    e.type = platform::EventType::PadAxis;
    e.key_name = name;
    e.value = value;
    return e;
}

platform::Event mouse_move(float dx, float dy) {
    platform::Event e;
    e.type = platform::EventType::MouseMove;
    e.dx = dx;
    e.dy = dy;
    return e;
}

platform::Event wheel(float dx, float dy) {
    platform::Event e;
    e.type = platform::EventType::MouseWheel;
    e.dx = dx;
    e.dy = dy;
    return e;
}

}  // namespace

TEST_CASE("mouse motion and wheel are axes read as deltas over a tick", "[input][mouse]") {
    app::InputMap m;
    REQUIRE(m.configure(Json{{"look_x", Json{{"axis", Json::array({"mouse:x", "pad:rightx"})}, {"deadzone", 0.0}}}, {"look_y", Json{{"axis", Json::array({"mouse:y"})}, {"deadzone", 0.0}}}, {"zoom", Json{{"axis", Json::array({"wheel:y"})}, {"deadzone", 0.0}}}}).has_value());
    // Two moves in one tick add up: 30 + 20 pixels right is half deflection; 10 pixels down is a tenth.
    m.apply(mouse_move(30, 10));
    m.apply(mouse_move(20, 0));
    Json s = m.snapshot();
    REQUIRE(s["look_x"]["value"].get<double>() == Catch::Approx(0.5));
    REQUIRE(s["look_y"]["value"].get<double>() == Catch::Approx(0.1));
    REQUIRE(s["look_x"]["down"] == false);
    // The tick that read them spends them.
    m.consume_edges();
    s = m.snapshot();
    REQUIRE(s["look_x"]["value"].get<double>() == 0.0);
    REQUIRE(s["look_y"]["value"].get<double>() == 0.0);
    // Past a hundred pixels the value is clamped, and `down` follows past half; the pad's stick
    // competes by magnitude and holds its value across ticks.
    m.apply(mouse_move(-250, 0));
    s = m.snapshot();
    REQUIRE(s["look_x"]["value"].get<double>() == Catch::Approx(-1.0));
    REQUIRE(s["look_x"]["down"] == true);
    REQUIRE(s["look_x"]["pressed"] == true);
    m.consume_edges();
    m.apply(pad_axis("rightx", 0.3f));
    m.apply(mouse_move(10, 0));
    s = m.snapshot();
    REQUIRE(s["look_x"]["value"].get<double>() == Catch::Approx(0.3));
    m.consume_edges();
    REQUIRE(m.snapshot()["look_x"]["value"].get<double>() == Catch::Approx(0.3));   // the stick stays where it is
    // A wheel notch is full deflection, spent the same way.
    m.apply(wheel(0, 1));
    REQUIRE(m.snapshot()["zoom"]["value"].get<double>() == Catch::Approx(1.0));
    m.consume_edges();
    REQUIRE(m.snapshot()["zoom"]["value"].get<double>() == 0.0);
    REQUIRE(m.describe()["look_x"]["axis"] == Json::array({"mouse:x", "pad:rightx"}));
}

TEST_CASE("input map edges last until consumed", "[input]") {
    app::InputMap m;
    REQUIRE(m.configure(Json{{"jump", Json::array({"Space", "pad:a"})}}).has_value());
    REQUIRE(m.size() == 1);
    m.apply(key(platform::EventType::KeyDown, "Space"));
    Json s = m.snapshot();
    REQUIRE(s["jump"]["down"] == true);
    REQUIRE(s["jump"]["pressed"] == true);
    m.consume_edges();
    s = m.snapshot();
    REQUIRE(s["jump"]["down"] == true);
    REQUIRE(s["jump"]["pressed"] == false);
    // A second source keeps the action down after the first releases; no edge either way.
    m.apply(pad_button("a", true));
    m.apply(key(platform::EventType::KeyUp, "Space"));
    s = m.snapshot();
    REQUIRE(s["jump"]["down"] == true);
    REQUIRE(s["jump"]["released"] == false);
    m.apply(pad_button("a", false));
    s = m.snapshot();
    REQUIRE(s["jump"]["down"] == false);
    REQUIRE(s["jump"]["released"] == true);
    m.consume_edges();
    REQUIRE(m.snapshot()["jump"]["released"] == false);
    // Key repeat is not a new press.
    m.apply(key(platform::EventType::KeyDown, "Space"));
    m.consume_edges();
    platform::Event rep = key(platform::EventType::KeyDown, "Space");
    rep.repeat = true;
    m.apply(rep);
    REQUIRE(m.snapshot()["jump"]["pressed"] == false);
}

TEST_CASE("input map axes combine keys and sticks with a dead zone", "[input]") {
    app::InputMap m;
    Json actions;
    actions["move_x"] = Json{{"negative", Json::array({"A", "pad:dpad_left"})}, {"positive", Json::array({"D"})}, {"axis", Json::array({"pad:leftx"})}, {"deadzone", 0.2}};
    REQUIRE(m.configure(actions).has_value());
    m.apply(key(platform::EventType::KeyDown, "D"));
    REQUIRE(m.snapshot()["move_x"]["value"].get<double>() == Catch::Approx(1.0));
    m.apply(key(platform::EventType::KeyDown, "A"));
    REQUIRE(m.snapshot()["move_x"]["value"].get<double>() == Catch::Approx(0.0));
    m.apply(key(platform::EventType::KeyUp, "D"));
    REQUIRE(m.snapshot()["move_x"]["value"].get<double>() == Catch::Approx(-1.0));
    m.apply(key(platform::EventType::KeyUp, "A"));
    // Inside the dead zone the stick is nothing; outside it is rescaled to 0..1.
    m.apply(pad_axis("leftx", 0.1f));
    REQUIRE(m.snapshot()["move_x"]["value"].get<double>() == Catch::Approx(0.0));
    REQUIRE(m.snapshot()["move_x"]["down"] == false);
    m.apply(pad_axis("leftx", 0.6f));
    REQUIRE(m.snapshot()["move_x"]["value"].get<double>() == Catch::Approx(0.5));
    REQUIRE(m.snapshot()["move_x"]["down"] == true);
    m.apply(pad_axis("leftx", -1.0f));
    REQUIRE(m.snapshot()["move_x"]["value"].get<double>() == Catch::Approx(-1.0));
    // A held key outweighs a smaller stick deflection.
    m.apply(pad_axis("leftx", 0.3f));
    m.apply(pad_button("dpad_left", true));
    REQUIRE(m.snapshot()["move_x"]["value"].get<double>() == Catch::Approx(-1.0));
    Json d = m.describe();
    REQUIRE(d["move_x"]["deadzone"].get<double>() == Catch::Approx(0.2));
    REQUIRE(m.keys_of("move_x") == std::vector<std::string>{"D"});
    REQUIRE(m.keys_of("move_x", -1) == std::vector<std::string>{"A"});
    REQUIRE(m.keys_of("move_x", 0) == std::vector<std::string>{"D", "A"});
    REQUIRE_FALSE(m.configure(Json{{"bad", 42}}).has_value());
}
