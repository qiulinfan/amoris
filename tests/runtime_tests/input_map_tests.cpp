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

}  // namespace

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
