#include <pocket/app/runtime.hpp>
#include <pocket/app/server.hpp>
#include <pocket/app/session.hpp>
#include <pocket/assets/assets.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Weverything"
#include <httplib.h>
#pragma clang diagnostic pop

#include <chrono>
#include <cmath>
#include <cstdlib>
#include <filesystem>
#include <thread>

using namespace pocket;

namespace {

std::filesystem::path root() {
    const char* r = std::getenv("POCKET_ROOT");
    REQUIRE(r != nullptr);
    return r;
}

app::Options hello_options(int frames) {
    app::Options o;
    o.project_dir = root() / "samples" / "hello";
    o.bundle = root() / "build" / "ts" / "hello.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = frames;
    o.width = 128;
    o.height = 72;
    o.log_level = "warn";
    return o;
}

}  // namespace

TEST_CASE("hello runs headless and reports state", "[runtime]") {
    auto o = hello_options(120);
    o.capture = root() / "build" / "test-out" / "hello-120.png";
    auto r = app::run(o);
    REQUIRE(r.has_value());
    const Json& rep = *r;
    INFO(rep.dump(2));
    REQUIRE(rep["ok"] == true);
    REQUIRE(rep["frames"] == 120);
    REQUIRE(rep["ticks"] == 120);
    REQUIRE(rep["state"].contains("bounces"));
    REQUIRE(rep["state"]["bounces"].get<int>() >= 1);
    REQUIRE(rep["state"]["ball.y"].get<double>() >= 0.0);
    REQUIRE(rep["state_hash"].get<std::string>().size() == 16);
    REQUIRE(rep["gpu"]["backend"] == "metal");
    REQUIRE(rep["script"]["engine"] == "JavaScriptCore");
    // The top-left pixel is sky: it must show the clear color the script set on the last tick.
    // The center shows the ball or the ground, so it must differ from the sky.
    auto corner = rep["capture"]["corner_pixel"];
    auto center = rep["capture"]["center_pixel"];
    auto cc = rep["clear_color"];
    bool center_differs = false;
    for (int i = 0; i < 3; ++i) {
        int expected = static_cast<int>(std::lround(cc[i].get<double>() * 255.0));
        REQUIRE(std::abs(corner[i].get<int>() - expected) <= 1);
        if (std::abs(center[i].get<int>() - expected) > 2) center_differs = true;
    }
    REQUIRE(center_differs);
    REQUIRE(corner[3] == 255);
    REQUIRE(rep["render"]["meshes"] == 2);
    REQUIRE(std::filesystem::exists(o.capture));
}

TEST_CASE("hello is deterministic across runs", "[runtime]") {
    auto a = app::run(hello_options(90));
    auto b = app::run(hello_options(90));
    REQUIRE(a.has_value());
    REQUIRE(b.has_value());
    REQUIRE((*a)["state_hash"] == (*b)["state_hash"]);
    REQUIRE((*a)["state"] == (*b)["state"]);
    auto c = hello_options(90);
    c.seed = 2;
    auto cr = app::run(c);
    REQUIRE(cr.has_value());
    // The seed only feeds random(); hello's exposed state does not depend on it, so the hash
    // must still match. This pins down that the hash covers state, not incidental values.
    REQUIRE((*cr)["state_hash"] == (*a)["state_hash"]);
}

TEST_CASE("golden state hash for hello", "[runtime]") {
    auto golden_path = root() / "tests" / "evidence" / "hello-golden.json";
    auto r = app::run(hello_options(120));
    REQUIRE(r.has_value());
    std::string hash = (*r)["state_hash"].get<std::string>();
    if (std::filesystem::exists(golden_path)) {
        Json golden = Json::parse(fs::read_text(golden_path).value());
        INFO("golden " << golden.dump() << " actual " << (*r)["state"].dump());
        REQUIRE(golden["state_hash"] == hash);
        REQUIRE(golden["state"] == (*r)["state"]);
    } else {
        Json golden;
        golden["note"] = "state hash of samples/hello after 120 headless ticks at 60 Hz; regenerate deliberately when the sample or the hash contract changes";
        golden["frames"] = 120;
        golden["state_hash"] = hash;
        golden["state"] = (*r)["state"];
        REQUIRE(fs::write_text(golden_path, golden.dump(2) + "\n").has_value());
        WARN("wrote new golden file " << golden_path.string());
    }
}

TEST_CASE("script errors are reported, not fatal", "[runtime]") {
    auto o = hello_options(3);
    auto broken = root() / "build" / "test-out" / "broken.js";
    REQUIRE(fs::write_text(broken, "globalThis.__pocket_dispatch = function(kind){ if (kind === 'tick') throw new Error('boom'); };").has_value());
    o.bundle = broken;
    o.project_config = "";
    o.project_config = broken.string() + ".project.json";
    auto r = app::run(o);
    REQUIRE(r.has_value());
    REQUIRE((*r)["ok"] == false);
    REQUIRE((*r)["errors"].size() >= 1);
    REQUIRE((*r)["errors"][0]["code"] == "script_error");
    REQUIRE((*r)["errors"][0]["message"].get<std::string>().find("boom") != std::string::npos);
}

TEST_CASE("prefab files instantiate, save and reload through the session", "[runtime][prefab]") {
    app::Options o;
    o.project_dir = root() / "samples" / "playground";
    o.bundle = root() / "build" / "ts" / "playground.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.width = 64;
    o.height = 64;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    auto inst = s.command("world.instantiate", Json{{"prefab", "prefabs/enemy.json"}, {"parent", "/Level"}, {"name", "Boss"}, {"components", Json{{"Health", Json{{"max", 300}, {"current", 300}}}}}});
    REQUIRE(inst.has_value());
    REQUIRE((*inst)["roots"].size() == 1);
    auto boss = (*inst)["roots"][0].get<world::EntityId>();
    Json d = s.command("world.describe", Json{{"entity", boss}}).value();
    REQUIRE(d["path"] == "/Level/Boss");
    REQUIRE(d["children"].size() == 1);
    REQUIRE(d["children"][0]["name"] == "Marker");
    REQUIRE(d["components"]["Health"]["max"] == 300);
    REQUIRE(d["components"]["MeshRenderer"]["mesh"] == "sphere");
    // Save the boss as a new prefab (under the project), instantiate it back, then remove the file.
    std::string rel = "prefabs/_boss_test.json";
    Json saved = s.command("world.save_prefab", Json{{"entity", boss}, {"path", rel}}).value();
    REQUIRE(saved["entities"] == 2);
    auto again = s.command("world.instantiate", Json{{"prefab", rel}}).value();
    REQUIRE(again["roots"].size() == 1);
    REQUIRE(s.command("world.describe", Json{{"entity", again["roots"][0]}}).value()["components"]["Health"]["max"] == 300);
    std::filesystem::remove(o.project_dir / rel);
    // Bad inputs are errors, not crashes.
    REQUIRE_FALSE(s.command("world.instantiate", Json{{"prefab", "../../README.md"}}).has_value());
    REQUIRE_FALSE(s.command("world.instantiate", Json{{"prefab", "prefabs/nope.json"}}).has_value());
    REQUIRE_FALSE(s.command("world.instantiate", Json{{"scene", Json{{"entities", 5}}}}).has_value());
    // Loading a scene by path replaces the world and emits scene.loaded.
    std::size_t before = s.world().entity_count();
    REQUIRE(before > 4);
    Json loaded = s.command("world.load", Json{{"path", "scene.json"}}).value();
    REQUIRE(loaded["entities"].get<std::size_t>() < before);
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["scene.loaded"] == 1);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("input actions carry edges across frames and release held keys", "[runtime][input]") {
    auto o = hello_options(1000);
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("input.map", Json{{"actions", Json{{"move_x", Json{{"negative", Json::array({"A"})}, {"positive", Json::array({"D"})}}}, {"jump", Json::array({"Space", "pad:a"})}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json before = s.command("input.actions", Json::object()).value();
    REQUIRE(before["move_x"]["down"] == false);
    // Hold D for 3 ticks: down with a pressed edge now, down during the next three ticks,
    // released at the start of the frame after that.
    Json held = s.command("input.hold", Json{{"action", "move_x"}, {"ticks", 3}}).value();
    REQUIRE(held["keys"][0] == "D");
    REQUIRE(held["actions"]["move_x"]["value"].get<double>() == Catch::Approx(1.0));
    REQUIRE(held["actions"]["move_x"]["pressed"] == true);
    for (int i = 0; i < 3; ++i) {
        REQUIRE(s.frame().has_value());
        Json mid = s.command("input.actions", Json::object()).value();
        INFO("tick " << i << ": " << mid.dump());
        REQUIRE(mid["move_x"]["down"] == true);
        REQUIRE(mid["move_x"]["pressed"] == false);
    }
    REQUIRE(s.frame().has_value());
    Json after = s.command("input.actions", Json::object()).value();
    INFO(after.dump());
    REQUIRE(after["move_x"]["down"] == false);
    // The released edge was consumed by the tick that saw it (see input_map_tests for edges).
    REQUIRE(after["move_x"]["released"] == false);
    REQUIRE(s.command("input.state", Json::object()).value()["held"].empty());
    // Pad events map like keys; a pad axis within the deadzone is nothing.
    Json st = s.command("input.state", Json::object()).value();
    REQUIRE(st["pads"] == 0);
    Json d = s.command("input.describe", Json::object()).value();
    REQUIRE(d["jump"]["positive"].size() == 2);
    REQUIRE_FALSE(s.command("input.hold", Json{{"action", "fly"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("skeletal animation poses a skinned mesh and moves its vertices", "[runtime][animation]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json clips = s.command("animation.clips", Json{{"entity", "Arm"}}).value();
    INFO(clips.dump());
    REQUIRE(clips["skinned"] == true);
    REQUIRE(clips["clips"].size() == 5);
    REQUIRE(clips["skins"][0]["joints"] == Json::array({"root", "tip"}));
    REQUIRE(s.frame().has_value());
    // Frame 1 sampled time 1/60: the tip joint leans about -45 degrees around Z (axis toward +X).
    Json pose0 = s.command("animation.pose", Json{{"entity", "Arm"}}).value();
    INFO(pose0.dump());
    REQUIRE(pose0["posed"] == true);
    REQUIRE(pose0["joints"].size() == 2);
    REQUIRE(pose0["joints"][1]["name"] == "tip");
    REQUIRE(pose0["joints"][1]["position"]["y"].get<double>() == Catch::Approx(1.0).margin(0.01));
    REQUIRE(pose0["joints"][1]["axis_y"]["x"].get<double>() > 0.6);
    Json rs = s.command("render.stats", Json::object()).value();
    REQUIRE(rs["skinned"].get<int>() == 6);  // the Arm, the Walker and the Pulse arm
    // Half a second later the swing has reached +45 degrees: the axis points toward -X.
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    Json pose1 = s.command("animation.pose", Json{{"entity", "Arm"}}).value();
    REQUIRE(pose1["joints"][1]["axis_y"]["x"].get<double>() < -0.6);
    Json anim = s.command("world.get", Json{{"entity", "Arm"}, {"component", "Animator"}}).value();
    REQUIRE(anim["time"].get<double>() == Catch::Approx(31.0 / 60.0).margin(1e-4));
    // The mesh really bends: the top of the arm (bind space (0,2,0)) is now near (-0.7, 1.7, 0) in
    // model space, and picking there hits the Arm while the bind-space top is empty.
    Json proj_bent = s.command("render.project", Json{{"point", {{"x", 0.2 - 0.68}, {"y", 1.68}, {"z", 1.6}}}}).value();
    REQUIRE(proj_bent["visible"] == true);
    Json pick_bent = s.command("render.pick", Json{{"x", proj_bent["x"]}, {"y", proj_bent["y"]}}).value();
    INFO(pick_bent.dump());
    REQUIRE(pick_bent["name"] == "Arm");
    Json proj_straight = s.command("render.project", Json{{"point", {{"x", 0.2}, {"y", 1.95}, {"z", 1.6}}}}).value();
    Json pick_straight = s.command("render.pick", Json{{"x", proj_straight["x"]}, {"y", proj_straight["y"]}}).value();
    REQUIRE(pick_straight["name"] != "Arm");
    // A one-shot clip stops at its end and says so; play with a bad name fails.
    REQUIRE(s.command("animation.play", Json{{"entity", "Arm"}, {"clip", "nod"}, {"loop", false}, {"speed", 4.0}}).has_value());
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    Json done = s.command("world.get", Json{{"entity", "Arm"}, {"component", "Animator"}}).value();
    REQUIRE(done["finished"] == true);
    REQUIRE(done["time"].get<double>() == Catch::Approx(1.5));
    Json ev = s.command("events.recent", Json{{"limit", 50}, {"type", "animation.finished"}}).value();
    REQUIRE(ev.size() >= 1);
    REQUIRE(s.command("animation.play", Json{{"entity", "Arm"}, {"clip", "sprint"}}).has_value() == false);
    REQUIRE(s.command("animation.play", Json{{"entity", "Crate"}, {"clip", "wave"}}).has_value() == false);
}

TEST_CASE("clips cross-fade and land on the new clip", "[runtime][animation]") {
    auto make = [] {
        app::Options o;
        o.project_dir = root() / "samples" / "assets";
        o.bundle = root() / "build" / "ts" / "assets.js";
        o.project_config = o.bundle.string() + ".project.json";
        o.headless = true;
        o.frames = 1000;
        o.width = 160;
        o.height = 90;
        o.log_level = "warn";
        return o;
    };
    auto tip_axis_x = [](app::Session& s) { return s.command("animation.pose", Json{{"entity", "Arm"}}).value()["joints"][1]["axis_y"]["x"].get<double>(); };
    // Two sessions at the same point of the wave (its +45 degree extreme): one cuts to nod, one
    // fades over a fifth of a second, while the wave swings back through +27 degrees.
    app::Session cut(make()), fade(make());
    REQUIRE(cut.start().has_value());
    REQUIRE(fade.start().has_value());
    for (int i = 0; i < 30; ++i) {
        REQUIRE(cut.frame().has_value());
        REQUIRE(fade.frame().has_value());
    }
    REQUIRE(cut.command("animation.play", Json{{"entity", "Arm"}, {"clip", "nod"}}).has_value());
    Json fading = fade.command("animation.play", Json{{"entity", "Arm"}, {"clip", "nod"}, {"fade", 0.2}}).value();
    REQUIRE(fading["from_clip"] == "wave");
    REQUIRE(fading["from_time"].get<double>() == Catch::Approx(0.5).margin(1e-4));
    REQUIRE(fading["fade"].get<double>() == Catch::Approx(0.2));
    // Halfway through the fade the pose sits between the two clips and says so.
    for (int i = 0; i < 6; ++i) {
        REQUIRE(cut.frame().has_value());
        REQUIRE(fade.frame().has_value());
    }
    Json mid = fade.command("animation.pose", Json{{"entity", "Arm"}}).value();
    INFO(mid.dump());
    REQUIRE(mid["blend"]["from"] == "wave");
    REQUIRE(mid["blend"]["weight"].get<double>() == Catch::Approx(0.5).margin(0.05));
    double x_cut = tip_axis_x(cut), x_fade = tip_axis_x(fade);
    INFO("cut " << x_cut << " fade " << x_fade);
    REQUIRE(std::abs(x_cut - x_fade) > 0.1);  // still carrying half of the wave's lean
    // After the fade both sessions agree exactly: the new clip has run the same time in both.
    for (int i = 0; i < 20; ++i) {
        REQUIRE(cut.frame().has_value());
        REQUIRE(fade.frame().has_value());
    }
    Json after = fade.command("world.get", Json{{"entity", "Arm"}, {"component", "Animator"}}).value();
    REQUIRE(after["from_clip"] == "");
    REQUIRE(after["fade"].get<double>() == 0.0);
    REQUIRE(after["clip"] == "nod");
    Json pa = cut.command("animation.pose", Json{{"entity", "Arm"}}).value(), pb = fade.command("animation.pose", Json{{"entity", "Arm"}}).value();
    for (int j = 0; j < 2; ++j) {
        for (const char* k : {"x", "y", "z"}) {
            REQUIRE(pa["joints"][j]["position"][k].get<double>() == Catch::Approx(pb["joints"][j]["position"][k].get<double>()).margin(1e-4));
            REQUIRE(pa["joints"][j]["axis_y"][k].get<double>() == Catch::Approx(pb["joints"][j]["axis_y"][k].get<double>()).margin(1e-4));
        }
    }
    REQUIRE_FALSE(pb.contains("blend"));
}

TEST_CASE("animation layers mask, weigh and add clips over the base", "[runtime][animation][layers]") {
    auto make = [] {
        app::Options o;
        o.project_dir = root() / "samples" / "assets";
        o.bundle = root() / "build" / "ts" / "assets.js";
        o.project_config = o.bundle.string() + ".project.json";
        o.headless = true;
        o.frames = 1000;
        o.width = 160;
        o.height = 90;
        o.log_level = "warn";
        return o;
    };
    auto axis = [](app::Session& s, int joint) { return s.command("animation.pose", Json{{"entity", "Arm"}}).value()["joints"][joint]["axis_y"]; };
    // The scene plays "wave" (the tip about Z). "nod" tilts the root about X: 30 degrees at 0.75 s.
    app::Session plain(make()), masked(make()), half(make()), full(make());
    for (app::Session* s : {&plain, &masked, &half, &full}) REQUIRE(s->start().has_value());
    // Errors: unknown clips and mask nodes are refused before anything changes.
    REQUIRE(masked.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "sprint"}}).has_value() == false);
    REQUIRE(masked.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"mask", "elbow"}}).error().code == "no_such_node");
    REQUIRE(masked.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"remove", true}}).error().code == "no_such_layer");
    // A mask of "tip" keeps nod (which moves the root) from moving anything; "root" lets it through.
    Json m = masked.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"mask", "tip"}}).value();
    REQUIRE(m["layers"].size() == 1);
    REQUIRE(m["layers"][0]["clip"] == "nod");
    REQUIRE(m["layers"][0]["mask"] == "tip");
    REQUIRE(half.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"mask", "root"}, {"weight", 0.5}}).has_value());
    REQUIRE(full.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"mask", " root "}}).has_value());
    for (int i = 0; i < 45; ++i) {
        for (app::Session* s : {&plain, &masked, &half, &full}) REQUIRE(s->frame().has_value());
    }
    Json root_plain = axis(plain, 0), root_masked = axis(masked, 0), root_half = axis(half, 0), root_full = axis(full, 0);
    INFO("root plain " << root_plain.dump() << " masked " << root_masked.dump() << " half " << root_half.dump() << " full " << root_full.dump());
    REQUIRE(root_plain["y"].get<double>() == Catch::Approx(1.0).margin(1e-3));
    REQUIRE(root_masked["y"].get<double>() == Catch::Approx(1.0).margin(1e-3));
    REQUIRE(root_full["y"].get<double>() == Catch::Approx(std::cos(30.0 * M_PI / 180.0)).margin(0.02));
    REQUIRE(std::abs(root_full["z"].get<double>()) == Catch::Approx(std::sin(30.0 * M_PI / 180.0)).margin(0.02));
    REQUIRE(root_half["y"].get<double>() == Catch::Approx(std::cos(15.0 * M_PI / 180.0)).margin(0.02));
    // The base wave keeps its time underneath the layer (at 0.75 s it passes through 0 degrees),
    // so the tip's axis follows the root's tilt; the pose lists the layer.
    Json pose = full.command("animation.pose", Json{{"entity", "Arm"}}).value();
    REQUIRE(pose["layers"].size() == 1);
    REQUIRE(pose["layers"][0]["time"].get<double>() == Catch::Approx(0.75).margin(1e-3));
    REQUIRE(pose["clip"] == "wave");
    REQUIRE(pose["time"].get<double>() == Catch::Approx(0.75).margin(1e-3));
    REQUIRE(std::abs(axis(full, 1)["z"].get<double>()) == Catch::Approx(0.5).margin(0.02));
    REQUIRE(std::abs(axis(plain, 1)["z"].get<double>()) < 0.02);
    // Additive: nod on top of nod doubles the tilt (the change since the clip's first frame).
    app::Session once(make()), twice(make());
    REQUIRE(once.start().has_value());
    REQUIRE(twice.start().has_value());
    REQUIRE(once.command("animation.play", Json{{"entity", "Arm"}, {"clip", "nod"}}).has_value());
    REQUIRE(twice.command("animation.play", Json{{"entity", "Arm"}, {"clip", "nod"}}).has_value());
    REQUIRE(twice.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"additive", true}}).has_value());
    for (int i = 0; i < 45; ++i) {
        REQUIRE(once.frame().has_value());
        REQUIRE(twice.frame().has_value());
    }
    Json r1 = axis(once, 0), r2 = axis(twice, 0);
    INFO("once " << r1.dump() << " twice " << r2.dump());
    REQUIRE(r1["y"].get<double>() == Catch::Approx(std::cos(30.0 * M_PI / 180.0)).margin(0.02));
    REQUIRE(r2["y"].get<double>() == Catch::Approx(std::cos(60.0 * M_PI / 180.0)).margin(0.02));
    REQUIRE(std::abs(r2["z"].get<double>()) == Catch::Approx(std::sin(60.0 * M_PI / 180.0)).margin(0.02));
    // Layer bookkeeping: a one-shot layer finishes with its index, stop pauses layers, remove drops them.
    REQUIRE(twice.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "wave"}, {"loop", false}, {"speed", 4.0}, {"mask", "tip"}}).value()["layers"].size() == 2);
    for (int i = 0; i < 20; ++i) REQUIRE(twice.frame().has_value());
    Json a = twice.command("world.get", Json{{"entity", "Arm"}, {"component", "Animator"}}).value();
    REQUIRE(a["layers"][1]["playing"] == false);
    REQUIRE(a["layers"][1]["time"].get<double>() == Catch::Approx(1.0));
    Json ev = twice.command("events.recent", Json{{"limit", 20}, {"type", "animation.finished"}}).value();
    REQUIRE(ev.size() >= 1);
    REQUIRE(ev.back()["data"]["layer"].get<int>() == 1);
    REQUIRE(ev.back()["data"]["clip"] == "wave");
    Json stopped = twice.command("animation.stop", Json{{"entity", "Arm"}}).value();
    REQUIRE(stopped["layers"][0]["playing"] == false);
    // Updating by clip name changes the layer in place; index 0 removal shifts the rest down.
    REQUIRE(twice.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"weight", 0.25}}).value()["layers"][0]["weight"].get<double>() == Catch::Approx(0.25));
    Json fewer = twice.command("animation.layer", Json{{"entity", "Arm"}, {"index", 0}, {"remove", true}}).value();
    REQUIRE(fewer["layers"].size() == 1);
    REQUIRE(fewer["layers"][0]["clip"] == "wave");
    REQUIRE(twice.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "wave"}, {"remove", true}}).value()["layers"].empty());
    // Typed access reaches into the list: layers.0.weight is a numeric path.
    REQUIRE(twice.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"weight", 0.5}}).has_value());
    Json packed = twice.command("world.pack", Json{{"component", "Animator"}, {"fields", Json::array({"layers.0.weight"})}, {"name", "Arm"}}).value();
    INFO(packed.dump());
    REQUIRE(packed["count"].get<int>() == 1);
    REQUIRE(packed["stride"].get<int>() == 1);
    REQUIRE(packed["layout"]["layers.0.weight"].get<int>() == 0);
}

TEST_CASE("particles spawn, draw, burst and hash deterministically", "[runtime][particles]") {
    auto make = [](std::uint64_t seed) {
        app::Options o;
        o.project_dir = root() / "samples" / "playground";
        o.bundle = root() / "build" / "ts" / "playground.js";
        o.project_config = o.bundle.string() + ".project.json";
        o.headless = true;
        o.frames = 1000;
        o.width = 320;
        o.height = 180;
        o.seed = seed;
        o.log_level = "warn";
        return o;
    };
    app::Session s(make(7));
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json stats = s.command("particles.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["emitters"].get<int>() >= 1);
    REQUIRE(stats["alive"].get<int>() >= 30);                 // 40/s for a second, lives of 1.2 s and more
    Json rs = s.command("render.stats", Json::object()).value();
    REQUIRE(rs["particles"].get<int>() == stats["alive"].get<int>());
    REQUIRE(rs["draw_calls"].get<int>() >= 2);
    // The particles are drawn where the fountain rises: a pick above the emitter finds it.
    Json fountain = s.command("world.describe", Json{{"entity", "/Level/Fountain"}}).value();
    REQUIRE(fountain["components"].contains("ParticleEmitter"));
    bool found = false;
    for (float y = 0.4f; y <= 2.4f && !found; y += 0.2f) {
        Json pr = s.command("render.project", Json{{"point", {{"x", -4.0}, {"y", y}, {"z", -4.0}}}}).value();
        if (pr["visible"] != true) continue;
        for (int dx = -3; dx <= 3 && !found; ++dx) {
            for (int dy = -3; dy <= 3 && !found; ++dy) {
                Json pick = s.command("render.pick", Json{{"x", pr["x"].get<double>() + dx}, {"y", pr["y"].get<double>() + dy}}).value();
                if (pick["name"] == "Fountain") found = true;
            }
        }
    }
    REQUIRE(found);
    // A burst adds at once, capped by max; clear removes everything.
    Json burst = s.command("particles.burst", Json{{"entity", "/Level/Fountain"}, {"count", 500}}).value();
    REQUIRE(burst["alive"].get<int>() == 200);
    REQUIRE(s.command("particles.burst", Json{{"entity", "/Level/Player"}, {"count", 5}}).has_value() == false);
    Json cleared = s.command("particles.clear", Json::object()).value();
    REQUIRE(cleared["cleared"].get<int>() == 200);
    // Same seed, same particles: the state hash covers them.
    app::Session a(make(11)), b(make(11));
    REQUIRE(a.start().has_value());
    REQUIRE(b.start().has_value());
    for (int i = 0; i < 45; ++i) {
        REQUIRE(a.frame().has_value());
        REQUIRE(b.frame().has_value());
    }
    REQUIRE(a.command("state", Json::object()).value()["state_hash"] == b.command("state", Json::object()).value()["state_hash"]);
    REQUIRE(a.command("particles.stats", Json::object()).value()["alive"].get<int>() > 0);
}

TEST_CASE("joints hold a pendulum chain and a rope snaps under load", "[runtime][joints]") {
    app::Options o;
    o.project_dir = root() / "samples" / "physics";
    o.bundle = root() / "build" / "ts" / "physics.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.seed = 3;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto position = [&](const char* path) {
        Json t = s.command("world.get", Json{{"entity", path}, {"component", "Transform"}}).value();
        return Vec3{t["position"]["x"].get<float>(), t["position"]["y"].get<float>(), t["position"]["z"].get<float>()};
    };
    float worst = 0;
    for (int i = 0; i < 200; ++i) {
        REQUIRE(s.frame().has_value());
        worst = std::max(worst, std::fabs(length(position("/Link1") - position("/Hook")) - 1.0f));
        worst = std::max(worst, std::fabs(length(position("/Link2") - position("/Link1")) - 1.0f));
        worst = std::max(worst, std::fabs(length(position("/Link3") - position("/Link2")) - 1.0f));
    }
    REQUIRE(worst < 0.06f);
    Json joints = s.command("physics.joints", Json::object()).value();
    INFO(joints.dump());
    REQUIRE(joints.size() == 8);  // three links, the lantern's rope, the hatch, the paddle, the lift and the bob
    bool chain_loaded = false;
    for (const auto& j : joints) {
        if (j["path"] == "/Link1") {
            chain_loaded = j["force"].get<float>() > 5.0f;  // carries the two links below (about 1 kg)
            REQUIRE(j["target"] == "/Hook");
            REQUIRE(j["length"].get<float>() == Catch::Approx(1.0f));
        }
    }
    REQUIRE(chain_loaded);
    REQUIRE(s.command("world.has", Json{{"entity", "/Lantern"}, {"component", "Joint"}}).value() == true);
    REQUIRE(s.command("physics.stats", Json::object()).value()["joints"].get<int>() == 8);
    // The kick at tick 240 snaps the rope: the joint is gone, the event says so, the lantern falls.
    for (int i = 0; i < 120; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("world.has", Json{{"entity", "/Lantern"}, {"component", "Joint"}}).value() == false);
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["joint.broken"].get<int>() == 1);
    REQUIRE(s.command("physics.joints", Json::object()).value().size() == 7);  // the rope is gone; the hinges, the slider and the spring stay
    REQUIRE(length(position("/Lantern") - position("/Beam")) > 2.0f);  // flew off, no longer tethered at 1.5
    Json state = s.command("state", Json::object()).value()["state"];
    REQUIRE(state["ropeIntact"] == false);
    REQUIRE(state["joints"].get<int>() == 7);
    // The capsule log came to rest on its side.
    REQUIRE(position("/Log").y == Catch::Approx(0.35f).margin(0.05f));
}

TEST_CASE("the recorder, events.why and render.visible explain a run", "[runtime][recorder]") {
    app::Options o;
    o.project_dir = root() / "samples" / "physics";
    o.bundle = root() / "build" / "ts" / "physics.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.seed = 3;
    o.log_level = "warn";
    o.history = 400;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 320; ++i) REQUIRE(s.frame().has_value());
    Json st = s.command("recorder.status", Json::object()).value();
    INFO(st.dump());
    REQUIRE(st["recording"] == true);
    REQUIRE(st["frames"].get<int>() == 320);
    REQUIRE(st["from"].get<int>() == 0);
    // The chain's last link was kicked at tick 60: the first tick it moved past z = -4.5.
    Json first = s.command("recorder.first", Json{{"entity", "/Link3"}, {"component", "Transform"}, {"field", "position.z"}, {"op", ">"}, {"value", -4.5}}).value();
    REQUIRE(first["tick"].get<int>() > 60);
    REQUIRE(first["tick"].get<int>() < 90);
    Json track = s.command("recorder.track", Json{{"entity", "/Link3"}, {"component", "Transform"}, {"field", "position.z"}, {"from", 0}, {"to", 59}}).value();
    REQUIRE(track["ticks"].size() == 60);
    for (const auto& v : track["values"]) REQUIRE(v.get<double>() == Catch::Approx(-5.0).margin(0.01));  // hung still before the kick
    // The lantern's rope broke after the kick at tick 240: it exists at tick 200 with its Joint, and without it at tick 300.
    REQUIRE(s.command("recorder.at", Json{{"tick", 200}, {"entity", "/Lantern"}}).value()["entity"]["components"].contains("Joint"));
    REQUIRE_FALSE(s.command("recorder.at", Json{{"tick", 300}, {"entity", "/Lantern"}}).value()["entity"]["components"].contains("Joint"));
    Json diff = s.command("recorder.diff", Json{{"from", 200}, {"to", 300}, {"entity", "/Lantern"}}).value();
    bool joint_gone = false;
    for (const auto& c : diff["changed"]) if (c["field"] == "Joint" && c["to"].is_null()) joint_gone = true;
    REQUIRE(joint_gone);
    REQUIRE(diff["spawned"].empty());
    // Why did the Joint component go? Because the joint broke.
    Json removed = s.command("events.since", Json{{"seq", 0}, {"type", "component.removed"}}).value();
    std::uint64_t seq = 0;
    for (const auto& e : removed["events"]) if (e["data"]["component"] == "Joint") seq = e["seq"].get<std::uint64_t>();
    REQUIRE(seq != 0);
    Json why = s.command("events.why", Json{{"seq", seq}}).value();
    INFO(why.dump());
    REQUIRE(why["chain"].size() == 2);
    REQUIRE(why["chain"][1]["type"] == "joint.broken");
    REQUIRE(why["complete"] == true);
    REQUIRE(why["story"].get<std::string>().find("component.removed(/Lantern)") == 0);
    // What the camera sees: the ground covers most of the frame, the ramp is in it, with bounds inside the image.
    Json vis = s.command("render.visible", Json{{"limit", 5}}).value();
    INFO(vis.dump());
    REQUIRE(vis["visible"].size() == 5);
    REQUIRE(vis["visible"][0]["name"] == "Ground");
    REQUIRE(vis["visible"][0]["coverage"].get<double>() > 0.3);
    bool ramp = false;
    for (const auto& e : vis["visible"]) {
        REQUIRE(e["bounds"]["x"].get<int>() + e["bounds"]["width"].get<int>() <= 320);
        REQUIRE(e["center"]["x"].get<double>() <= 1.0);
        if (e["name"] == "Ramp") ramp = true;
    }
    REQUIRE(ramp);
    REQUIRE(vis["count"].get<int>() >= 5);
    // Stopping keeps what was recorded; clearing drops it.
    REQUIRE(s.command("recorder.stop", Json::object()).value()["recording"] == false);
    for (int i = 0; i < 5; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("recorder.status", Json::object()).value()["to"].get<int>() == 319);
    REQUIRE(s.command("recorder.clear", Json::object()).value()["frames"].get<int>() == 0);
    REQUIRE_FALSE(s.command("recorder.at", Json{{"tick", 100}}).has_value());
}

TEST_CASE("a scenario bundle plays the game and reports its verdict", "[runtime][scenario]") {
    auto make = [](const char* name) {
        app::Options o;
        o.project_dir = root() / "samples" / "sprites";
        o.bundle = root() / "build" / "ts" / "sprites.js";
        o.project_config = o.bundle.string() + ".project.json";
        o.scenario_bundle = root() / "build" / "ts" / "sprites.scenarios.coins.js";
        o.scenario = name;
        o.headless = true;
        o.frames = 1000;
        o.width = 160;
        o.height = 90;
        o.seed = 2;
        o.log_level = "warn";
        return o;
    };
    app::Session s(make("walking right collects the coin ahead within a second"));
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());  // exposed state is sampled at the end of a tick
    Json state = s.command("state", Json::object()).value()["state"];
    INFO(state.dump());
    REQUIRE(state["__scenarios"].size() == 10);
    REQUIRE(state["__scenario"]["status"] == "running");
    REQUIRE_FALSE(s.quit_requested());
    int frames = 1;
    while (!s.quit_requested() && frames < 200) {
        REQUIRE(s.frame().has_value());
        ++frames;
    }
    Json done = s.command("state", Json::object()).value()["state"];
    INFO(done.dump());
    REQUIRE(done["__scenario"]["status"] == "passed");
    REQUIRE(done["score"].get<int>() == 1);
    REQUIRE(done["__scenario"]["ticks"].get<int>() < 60);
    REQUIRE(s.quit_requested());  // the scenario quits the run when it decides
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["scenario.started"].get<int>() == 1);
    REQUIRE(hist["scenario.finished"].get<int>() == 1);
    // A scenario that cannot be satisfied fails at the step it was on, with a reason.
    app::Session f(make("walking right collects the coin ahead within a second"));
    REQUIRE(f.start().has_value());
    REQUIRE(f.command("world.set", Json{{"entity", "Coin3"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 8.5}, {"y", -3}}}}}}).has_value());  // the coin ahead moved out of reach
    frames = 0;
    while (!f.quit_requested() && frames < 200) {
        REQUIRE(f.frame().has_value());
        ++frames;
    }
    Json failed = f.command("state", Json::object()).value()["state"]["__scenario"];
    INFO(failed.dump());
    REQUIRE(failed["status"] == "failed");
    REQUIRE(failed["label"] == "first coin");
    REQUIRE(failed["error"].get<std::string>().find("not within") != std::string::npos);
    // An unknown name fails at once: the run quits before its first tick.
    app::Session u(make("no such scenario"));
    REQUIRE(u.start().has_value());
    REQUIRE(u.quit_requested());
}

TEST_CASE("Body2D falls onto tiles, is stopped by walls and passes one-way planks from below", "[runtime][body2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto body = [&](const char* path) { return s.command("world.get", Json{{"entity", path}, {"component", "Body2D"}}).value(); };
    auto pos = [&](const char* path) { Json t = s.command("world.get", Json{{"entity", path}, {"component", "Transform"}}).value(); return Vec3{t["position"]["x"].get<float>(), t["position"]["y"].get<float>(), 0}; };
    // A crate dropped from the air lands on the ground row (top at y = -3.5) and reports it
    // (x -7: between the sample's lift, on the rail at x -8.5, and the plank over cells 4 to 6).
    REQUIRE(s.command("world.spawn", Json{{"name", "Crate"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -7}, {"y", 2}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}}}}}}).has_value());
    for (int i = 0; i < 90; ++i) REQUIRE(s.frame().has_value());
    Json crate = body("Crate");
    INFO(crate.dump());
    REQUIRE(crate["grounded"] == true);
    REQUIRE(crate["velocity"]["y"].get<double>() == 0.0);
    REQUIRE(pos("Crate").y == Catch::Approx(-3.2f).margin(0.01f));  // ground top + half size
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["body2d.landed"].get<int>() >= 1);
    Json stats = s.command("physics.stats", Json::object()).value();
    REQUIRE(stats["tiles"]["bodies"].get<int>() == 5);   // the player, the crate, the lift, and the sample's ball and puck
    REQUIRE(stats["tiles"]["grounded"].get<int>() >= 3);   // the player, the crate, the sample's puck (its ball may be in the air)
    // Pushed right along the ground, it stops at the ledge (cells 13-14: x from 3 to 5).
    REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 1.0}, {"y", -3.2}}}}}}).has_value());
    for (int i = 0; i < 60; ++i) {
        REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Body2D"}, {"value", Json{{"velocity", Json{{"x", 6.0}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
    }
    // The ledge sits at rows 6 (y -2.5..-1.5): a crate on the ground (top at -3.5, so its box spans -3.5..-2.9) passes under it. Raise it onto the ledge's row first.
    REQUIRE(pos("Crate").x > 4.0f);  // walked under the ledge
    REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 1.0}, {"y", -2.0}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Body2D"}, {"value", Json{{"gravity", 0.0}, {"velocity", Json{{"x", 6.0}, {"y", 0.0}}}}}}).has_value());
    for (int i = 0; i < 40; ++i) {
        REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Body2D"}, {"value", Json{{"velocity", Json{{"x", 6.0}, {"y", 0.0}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
    }
    crate = body("Crate");
    INFO(crate.dump());
    REQUIRE(crate["on_wall"].get<int>() == 1);
    REQUIRE(pos("Crate").x == Catch::Approx(3.0f - 0.3f).margin(0.01f));
    // The one-way plank (cells 4-6 of row 6: top at y = -1.5) lets a body rise through it and catches it falling.
    REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", -5.0}, {"y", -2.0}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Body2D"}, {"value", Json{{"gravity", -24.0}, {"velocity", Json{{"x", 0.0}, {"y", 12.0}}}}}}).has_value());
    float top = -10;
    bool ceiling = false;
    for (int i = 0; i < 120; ++i) {
        REQUIRE(s.frame().has_value());
        top = std::max(top, pos("Crate").y);
        if (body("Crate")["on_ceiling"] == true) ceiling = true;
    }
    REQUIRE_FALSE(ceiling);
    REQUIRE(top > -0.7f);                                       // rose through the plank (its top is at -1.5)
    crate = body("Crate");
    INFO(crate.dump());
    REQUIRE(crate["grounded"] == true);
    REQUIRE(pos("Crate").y == Catch::Approx(-1.5f + 0.3f).margin(0.01f));  // resting on the plank
    // From the side, a plank is nothing: the crate walks through it.
    REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", -7.0}, {"y", -2.0}}}}}}).has_value());
    for (int i = 0; i < 30; ++i) {
        REQUIRE(s.command("world.set", Json{{"entity", "Crate"}, {"component", "Body2D"}, {"value", Json{{"gravity", 0.0}, {"velocity", Json{{"x", 6.0}, {"y", 0.0}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
    }
    REQUIRE(body("Crate")["on_wall"].get<int>() == 0);
    REQUIRE(pos("Crate").x > -4.5f);
    // The player is a Body2D too: the sample's exposed state says it stands on the ground.
    Json st = s.command("state", Json::object()).value()["state"];
    REQUIRE(st["player.grounded"] == true);
    REQUIRE(st["player.y"].get<double>() == Catch::Approx(-3.0).margin(0.01));
}

TEST_CASE("sprite clips from project.toml play through commands", "[runtime][sprites]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json clips = s.command("sprite.clips", Json::object()).value();
    REQUIRE(clips.contains("walk"));
    REQUIRE(clips["walk"]["frames"].size() == 4);
    REQUIRE(s.frame().has_value());
    // Idle at rest, walking while an action is held, idle again afterwards; the coins spin on their own.
    Json idle = s.command("world.get", Json{{"entity", "Player"}, {"component", "SpriteAnimation"}}).value();
    REQUIRE(idle["clip"] == "idle");
    REQUIRE(s.command("input.hold", Json{{"action", "move_x"}, {"ticks", 30}}).has_value());
    for (int i = 0; i < 10; ++i) REQUIRE(s.frame().has_value());
    Json walking = s.command("world.get", Json{{"entity", "Player"}, {"component", "SpriteAnimation"}}).value();
    REQUIRE(walking["clip"] == "walk");
    REQUIRE(walking["playing"] == true);
    Json coin = s.command("world.get", Json{{"entity", "Coin5"}, {"component", "SpriteAnimation"}}).value();
    REQUIRE(coin["clip"] == "coin");
    Json sprite = s.command("world.get", Json{{"entity", "Coin5"}, {"component", "Sprite"}}).value();
    REQUIRE(sprite["uv"]["z"].get<double>() - sprite["uv"]["x"].get<double>() == Catch::Approx(0.25));
    for (int i = 0; i < 40; ++i) REQUIRE(s.frame().has_value());
    Json again = s.command("world.get", Json{{"entity", "Player"}, {"component", "SpriteAnimation"}}).value();
    REQUIRE(again["clip"] == "idle");
    // A one-shot clip defined on the fly stops and reports.
    REQUIRE(s.command("sprite.clip", Json{{"name", "pop"}, {"columns", 4}, {"rows", 1}, {"fps", 60}, {"loop", false}}).has_value());
    REQUIRE(s.command("sprite.play", Json{{"entity", "Player"}, {"clip", "pop"}}).has_value());
    for (int i = 0; i < 10; ++i) REQUIRE(s.frame().has_value());
    Json popped = s.command("world.get", Json{{"entity", "Player"}, {"component", "SpriteAnimation"}}).value();
    REQUIRE(popped["finished"] == true);
    REQUIRE(popped["frame"] == 3);
    Json ev = s.command("events.recent", Json{{"limit", 50}, {"type", "sprite.finished"}}).value();
    INFO(ev.dump());
    REQUIRE(ev.size() >= 1);
    REQUIRE(s.command("sprite.play", Json{{"entity", "Player"}, {"clip", "nope"}}).has_value() == false);
}

TEST_CASE("sprites draw unlit through an orthographic camera and are picked by shape", "[runtime][sprites]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["sprites"].get<int>() == 10);  // the player, six coins, the lift, the ball and the puck; the ground is a tile map
    REQUIRE(stats["tile_layers"].get<int>() == 4);         // ground, deco, platforms and water layers of level.tmj
    REQUIRE(stats["image_layers"].get<int>() == 1);        // the sky, repeated across the level in one run of quads
    REQUIRE(stats["draw_calls"].get<int>() <= 8);           // runs per texture, split by layer (four tile layers and the sky among them)
    // The map answers what is where: solid ground under the player, air above, the ledge at (13,6).
    Json below = s.command("tilemap.solid", Json{{"entity", "Level"}, {"x", 0.0}, {"y", -3.6}}).value();
    REQUIRE(below["solid"] == true);
    REQUIRE(below["tile_y"] == 8);
    Json above = s.command("tilemap.solid", Json{{"entity", "Level"}, {"x", 0.0}, {"y", -2.0}}).value();
    REQUIRE(above["solid"] == false);
    Json ledge = s.command("tilemap.tile", Json{{"entity", "Level"}, {"tile_x", 13}, {"tile_y", 6}}).value();
    REQUIRE(ledge["solid"] == true);
    REQUIRE(ledge["layers"][1]["flip_h"] == true);
    REQUIRE(ledge["layers"][1]["properties"]["solid"] == true);
    Json objects = s.command("tilemap.objects", Json{{"entity", "Level"}, {"layer", "spawns"}}).value();
    REQUIRE(objects.size() == 7);
    REQUIRE(objects[0]["name"] == "player");
    REQUIRE(objects[0]["x"].get<double>() == Catch::Approx(0.0));
    REQUIRE(objects[0]["y"].get<double>() == Catch::Approx(-3.0));
    // Picking the ground finds the map entity.
    Json gp = s.command("render.project", Json{{"point", {{"x", 0.0}, {"y", -4.0}, {"z", 0.0}}}}).value();
    Json gpick = s.command("render.pick", Json{{"x", gp["x"]}, {"y", gp["y"]}}).value();
    REQUIRE(gpick["name"] == "Level");
    // The player sits at (0,-3); with ortho_size 5 over 180 px that is 90 + 3/5*90 = 144 px down.
    Json pr = s.command("render.project", Json{{"entity", "Player"}}).value();
    REQUIRE(pr["visible"] == true);
    REQUIRE(pr["x"].get<double>() == Catch::Approx(160).margin(1));
    REQUIRE(pr["y"].get<double>() == Catch::Approx(144).margin(1));
    Json pick = s.command("render.pick", Json{{"x", 160}, {"y", 144}}).value();
    REQUIRE(pick["name"] == "Player");
    // The transparent corner of the player's square shows what is behind it (the sky: nothing).
    Json corner = s.command("render.pick", Json{{"x", 160 - 8}, {"y", 144 - 8}}).value();
    REQUIRE(corner["id"] == 0);
    // Walk right for a second: the player moves, a coin is collected, the HUD says so.
    REQUIRE(s.command("input.hold", Json{{"action", "move_x"}, {"ticks", 60}}).has_value());
    for (int i = 0; i < 70; ++i) REQUIRE(s.frame().has_value());
    Json st = s.command("state", Json::object()).value();
    INFO(st.dump());
    REQUIRE(st["state"]["player.x"].get<double>() > 4.0);
    REQUIRE(st["state"]["score"].get<int>() >= 1);
    std::string snap = s.command("ui.snapshot", Json{{"depth", 4}}).value()["text"].get<std::string>();
    REQUIRE(snap.find("Score 1") != std::string::npos);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("save slots hold the world and script state and load back exactly", "[runtime][saves]") {
    auto o = hello_options(1000);
    o.save_dir = root() / "build" / "test-out" / "saves";
    std::filesystem::remove_all(o.save_dir);
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json before = s.command("state", Json::object()).value();
    Json w = s.command("save.write", Json{{"slot", "checkpoint-1"}, {"data", Json{{"label", "after one second"}}}}).value();
    INFO(w.dump());
    REQUIRE(std::filesystem::exists(o.save_dir / "checkpoint-1.json"));
    REQUIRE(w["entities"] == 4);
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json later = s.command("state", Json::object()).value();
    REQUIRE(later["world_hash"] != before["world_hash"]);
    REQUIRE(later["state"]["bounces"] != before["state"]["bounces"]);
    Json l = s.command("save.read", Json{{"slot", "checkpoint-1"}}).value();
    INFO(l.dump());
    REQUIRE(l["saved_tick"] == 60);
    REQUIRE(l["data"]["label"] == "after one second");
    Json after = s.command("state", Json::object()).value();
    INFO(after.dump());
    REQUIRE(after["world_hash"] == before["world_hash"]);
    REQUIRE(after["state"] == before["state"]);
    // The game goes on from the loaded point: the next second bounces the same number of times.
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json resumed = s.command("state", Json::object()).value();
    REQUIRE(resumed["state"]["bounces"] == later["state"]["bounces"]);
    REQUIRE(resumed["state"]["ball.y"] == later["state"]["ball.y"]);
    Json list = s.command("save.list", Json::object()).value();
    REQUIRE(list.size() == 1);
    REQUIRE(list[0]["slot"] == "checkpoint-1");
    REQUIRE(list[0]["tick"] == 60);
    REQUIRE(list[0]["data"]["label"] == "after one second");
    REQUIRE_FALSE(s.command("save.read", Json{{"slot", "nope"}}).has_value());
    REQUIRE_FALSE(s.command("save.write", Json{{"slot", "../escape"}}).has_value());
    REQUIRE(s.command("save.delete", Json{{"slot", "checkpoint-1"}}).value()["deleted"] == true);
    REQUIRE(s.command("save.list", Json::object()).value().empty());
    Json events = s.command("events.recent", Json{{"n", 50}}).value();
    bool saw_written = false, saw_loaded = false;
    for (const auto& e : events) { if (e["type"] == "save.written") saw_written = true; if (e["type"] == "save.loaded") saw_loaded = true; }
    REQUIRE(saw_written);
    REQUIRE(saw_loaded);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("animated tiles show the frame the simulation clock is at", "[runtime][tilemap][animtiles]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    auto water = [&]() {
        for (const Json& l : s.command("tilemap.tile", Json{{"entity", "Level"}, {"tile_x", 0}, {"tile_y", 7}}).value()["layers"]) if (l["layer"] == "water") return l;
        return Json(nullptr);
    };
    auto frames = [&]() { return s.command("render.stats", Json::object()).value()["tile_frames"].get<int>(); };
    // The pool's cell holds tile 5 (gid 6), which shows itself in its first 400 ms and tile 6 after.
    Json w = water();
    INFO(w.dump());
    REQUIRE(w["id"] == 5);
    REQUIRE(w["frame"] == 5);
    REQUIRE(w["solid"] == false);
    const int built = frames();
    for (int i = 0; i < 12; ++i) REQUIRE(s.frame().has_value());   // 0.2 s: the same frame, nothing rebuilt
    REQUIRE(water()["frame"] == 5);
    REQUIRE(frames() == built);
    for (int i = 0; i < 15; ++i) REQUIRE(s.frame().has_value());   // past 0.4 s: the second frame, one rebuild of the animated cells
    REQUIRE(water()["frame"] == 6);
    REQUIRE(frames() == built + 1);
    for (int i = 0; i < 24; ++i) REQUIRE(s.frame().has_value());   // past 0.8 s: round again
    REQUIRE(water()["frame"] == 5);
    REQUIRE(frames() == built + 2);
    // A cell with a still tile has no frame.
    for (const Json& l : s.command("tilemap.tile", Json{{"entity", "Level"}, {"tile_x", 0}, {"tile_y", 8}}).value()["layers"]) if (l["layer"] == "ground") REQUIRE_FALSE(l.contains("frame"));
}

TEST_CASE("an isometric map is drawn, asked, walked by the grid and left alone by bodies", "[runtime][tilemap][isomap]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    // A 4 by 3 isometric map of 32 by 16 diamonds drawn from the sample's 16-pixel tiles, written
    // beside the sample's map for the test and removed after it.
    const std::filesystem::path file = o.project_dir / "assets" / "iso-test.tmj";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    {
        std::ofstream out(file);
        out << R"({"width":4,"height":3,"tilewidth":32,"tileheight":16,"orientation":"isometric","tilesets":[{"firstgid":1,"name":"tiles","image":"tiles.png","imagewidth":80,"imageheight":16,"tilewidth":16,"tileheight":16,"columns":5,"tilecount":5,"tiles":[{"id":0,"properties":[{"name":"solid","type":"bool","value":true}]}]}],"layers":[{"id":1,"type":"tilelayer","name":"floor","width":4,"height":3,"data":[1,2,3,4,1,1,1,1,2,2,2,2]}]})";
    }
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    const int drawn_before = s.command("render.stats", Json::object()).value()["tile_layers"].get<int>();
    // Far from the sample's level, so nothing of it is under the bodies here.
    REQUIRE(s.command("world.spawn", Json{{"name", "Iso"}, {"components", Json{{"Transform", Json{{"position", {{"x", 100}, {"y", 0}, {"z", 0}}}}}, {"TileMap", Json{{"map", "assets/iso-test.tmj"}, {"tile_size", 1.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_layers"].get<int>() == drawn_before + 1);
    Json info = s.command("tilemap.info", Json{{"entity", "Iso"}}).value();
    INFO(info.dump());
    REQUIRE(info["orientation"] == "isometric");
    REQUIRE(info["bounds"]["max"]["x"].get<double>() == Catch::Approx(103.5));   // (4 + 3) diamonds' half widths at a unit per 32 pixels
    REQUIRE(info["bounds"]["min"]["y"].get<double>() == Catch::Approx(-1.75));
    // The center of tile (1, 1): its box at pixel (32, 16), so its center at (48, 24), a unit and a half in and three quarters down.
    Json cell = s.command("tilemap.cell", Json{{"entity", "Iso"}, {"x", 101.5}, {"y", -0.75}}).value();
    INFO(cell.dump());
    REQUIRE(cell["tile_x"] == 1);
    REQUIRE(cell["tile_y"] == 1);
    REQUIRE(cell["inside"] == true);
    REQUIRE(cell["center"]["x"].get<double>() == Catch::Approx(101.5));
    REQUIRE(cell["center"]["y"].get<double>() == Catch::Approx(-0.75));
    REQUIRE(s.command("tilemap.cell", Json{{"entity", "Iso"}, {"x", 100.1}, {"y", -0.05}}).value()["inside"] == false);   // the top-left corner, outside the diamonds
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Iso"}, {"tile_x", 0}, {"tile_y", 0}}).value()["solid"] == true);
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Iso"}, {"tile_x", 1}, {"tile_y", 0}}).value()["solid"] == false);
    // A body over the map falls through it: the platformer knows orthogonal maps only.
    REQUIRE(s.command("world.spawn", Json{{"name", "Drop"}, {"components", Json{{"Transform", Json{{"position", {{"x", 101.5}, {"y", -0.2}, {"z", 0}}}}}, {"Body2D", Json{{"size", {{"x", 0.2}, {"y", 0.2}}}}}}}}).has_value());
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json drop = s.command("world.get", Json{{"entity", "Drop"}, {"component", "Transform"}}).value();
    REQUIRE(drop["position"]["y"].get<double>() < -1.75);
    REQUIRE(s.command("world.get", Json{{"entity", "Drop"}, {"component", "Body2D"}}).value()["grounded"] == false);
    // The top-down grid bakes from it as a grid of diamonds (the platformer mode refuses).
    Json baked = s.command("nav.bake", Json{{"entity", "Iso"}, {"mode", "topdown"}}).value();
    REQUIRE(baked["layout"] == "isometric");
    REQUIRE(baked["walkable"].get<int>() == 7);   // five cells hold the solid tile
    REQUIRE(s.command("nav.bake", Json{{"entity", "Iso"}, {"mode", "platformer"}}).error().code == "bad_tilemap");
}

TEST_CASE("an entity's copy of a map is edited apart from the map and outlives a reload", "[runtime][tilemap][mapcopy]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    const std::string map = s.command("world.get", Json{{"entity", "Level"}, {"component", "TileMap"}}).value()["map"].get<std::string>();
    // A second entity draws the same file, off to the side.
    REQUIRE(s.command("world.spawn", Json{{"name", "Annex"}, {"components", Json{{"Transform", Json{{"position", {{"x", 40}, {"y", 0}, {"z", 0}}}}}, {"TileMap", Json{{"map", map}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const int drawn_shared = s.command("render.stats", Json::object()).value()["tile_layers"].get<int>();
    auto gid_at = [&](const char* entity, int x, int y) {
        for (const Json& l : s.command("tilemap.tile", Json{{"entity", entity}, {"tile_x", x}, {"tile_y", y}}).value()["layers"]) if (l["gid"].get<std::uint32_t>() != 0) return l["gid"].get<std::uint32_t>();
        return std::uint32_t{0};
    };
    // A cell in the air is empty on both; an edit through the annex, before the copy, is the map's.
    REQUIRE(gid_at("Level", 3, 2) == 0);
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Annex"}, {"tile_x", 3}, {"tile_y", 2}, {"gid", 1}}).has_value());
    REQUIRE(gid_at("Level", 3, 2) == 1);
    Json copy = s.command("tilemap.copy", Json{{"entity", "Annex"}}).value();
    INFO(copy.dump());
    REQUIRE(copy["map"] == map + "@Annex");
    REQUIRE(copy["source"] == map);
    REQUIRE(s.command("world.get", Json{{"entity", "Annex"}, {"component", "TileMap"}}).value()["map"] == map + "@Annex");
    REQUIRE(s.command("tilemap.copy", Json{{"entity", "Annex"}, {"name", map + "@Annex"}}).error().code == "duplicate_map");   // a name in use; a copy of the copy would be fine
    // From here the annex's edits are its own: the level keeps the tile the annex clears.
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Annex"}, {"tile_x", 3}, {"tile_y", 2}, {"clear", true}}).has_value());
    REQUIRE(gid_at("Annex", 3, 2) == 0);
    REQUIRE(gid_at("Level", 3, 2) == 1);
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Annex"}, {"tile_x", 4}, {"tile_y", 2}, {"gid", 1}}).has_value());
    REQUIRE(gid_at("Level", 4, 2) == 0);
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Annex"}, {"tile_x", 4}, {"tile_y", 2}}).value()["solid"] == true);
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Level"}, {"tile_x", 4}, {"tile_y", 2}}).value()["solid"] == false);
    // Both are drawn, each its own layers.
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_layers"].get<int>() == drawn_shared);
    // The copy has no file: saving needs a path; with one it writes a map that reads back.
    REQUIRE(s.command("tilemap.save", Json{{"entity", "Annex"}}).error().code == "no_file");
    Json saved = s.command("tilemap.save", Json{{"entity", "Annex"}, {"path", "assets/annex-test.tmj"}}).value();
    REQUIRE(saved["bytes"].get<std::size_t>() > 100);
    std::filesystem::remove(o.project_dir / "assets" / "annex-test.tmj");
    // Reloading the assets forgets the level's unsaved edit and keeps the copy with its own.
    REQUIRE(s.command("assets.reload", Json::object()).has_value());
    REQUIRE(gid_at("Level", 3, 2) == 0);
    REQUIRE(gid_at("Annex", 3, 2) == 0);
    REQUIRE(gid_at("Annex", 4, 2) == 1);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_layers"].get<int>() == drawn_shared);
    REQUIRE(s.command("events.since", Json{{"since", 0}, {"limit", 200}, {"type", "tilemap.copied"}}).value()["events"].size() == 1);
}

TEST_CASE("layers and tilesets are added at runtime: drawn, solid, saved and removed", "[runtime][tilemap][layers]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    Json info = s.command("tilemap.info", Json{{"entity", "Level"}}).value();
    const std::size_t layers_before = info["layers"].size(), sets_before = info["tilesets"].size();
    const int drawn_before = s.command("render.stats", Json::object()).value()["tile_layers"].get<int>();
    // A cell in the air holds nothing on any layer.
    auto tile_at = [&](int x, int y) { return s.command("tilemap.tile", Json{{"entity", "Level"}, {"tile_x", x}, {"tile_y", y}}).value(); };
    auto solid_at = [&](int x, int y) { return s.command("tilemap.solid", Json{{"entity", "Level"}, {"tile_x", x}, {"tile_y", y}}).value()["solid"].get<bool>(); };
    // tilemap.tile lists every layer; the ones holding a tile there have a gid.
    auto filled = [](const Json& t) { Json out = Json::array(); for (const Json& l : t["layers"]) if (l["gid"].get<std::uint32_t>() != 0) out.push_back(l); return out; };
    auto has_layer = [](const Json& t, const char* name) { for (const Json& l : t["layers"]) if (l["layer"] == name) return true; return false; };
    REQUIRE(filled(tile_at(3, 1)).empty());
    REQUIRE_FALSE(has_layer(tile_at(3, 1), "extra"));
    REQUIRE_FALSE(solid_at(3, 1));
    // A solid layer on top, three tiles of the first tileset on it: seen, solid and drawn.
    Json added = s.command("tilemap.add_layer", Json{{"entity", "Level"}, {"name", "extra"}, {"solid", true}}).value();
    INFO(added.dump());
    REQUIRE(added["layer"] == "extra");
    REQUIRE(added["index"].get<std::size_t>() == layers_before);
    REQUIRE(added["solid"] == true);
    REQUIRE(added["layers"].get<std::size_t>() == layers_before + 1);
    REQUIRE(s.command("tilemap.add_layer", Json{{"entity", "Level"}, {"name", "extra"}}).error().code == "duplicate_layer");
    REQUIRE(s.command("tilemap.fill", Json{{"entity", "Level"}, {"layer", "extra"}, {"tile_x", 2}, {"tile_y", 1}, {"width", 3}, {"height", 1}, {"id", 0}}).value()["changed"] == 3);
    Json t = filled(tile_at(3, 1));
    REQUIRE(t.size() == 1);
    REQUIRE(t[0]["layer"] == "extra");
    REQUIRE(t[0]["gid"] == 1);
    REQUIRE(t[0]["solid"] == true);
    REQUIRE(solid_at(3, 1));
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_layers"].get<int>() == drawn_before + 1);
    // A second tileset from another image of the project: its ids follow the first's.
    Json set = s.command("tilemap.add_tileset", Json{{"entity", "Level"}, {"name", "coins"}, {"image", "assets/coin.png"}, {"tiles", Json{{"1", Json{{"solid", true}}}}}}).value();
    INFO(set.dump());
    REQUIRE(set["tile_count"] == 4);
    REQUIRE(set["columns"] == 4);
    REQUIRE(set["first_gid"].get<std::uint32_t>() == 1 + info["tilesets"][0]["tile_count"].get<std::uint32_t>());
    REQUIRE(set["tilesets"].get<std::size_t>() == sets_before + 1);
    REQUIRE(s.command("tilemap.add_tileset", Json{{"entity", "Level"}, {"name", "coins"}, {"image", "assets/coin.png"}}).error().code == "duplicate_tileset");
    REQUIRE(s.command("tilemap.add_tileset", Json{{"entity", "Level"}, {"name", "nope"}, {"image", "assets/missing.png"}}).error().code == "bad_image");
    const std::uint32_t coin_gid = set["first_gid"].get<std::uint32_t>() + 1;
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Level"}, {"layer", "extra"}, {"tile_x", 6}, {"tile_y", 1}, {"id", 1}, {"tileset", "coins"}}).value()["gid"].get<std::uint32_t>() == coin_gid);
    t = filled(tile_at(6, 1));
    REQUIRE(t.size() == 1);
    REQUIRE(t[0]["tileset"] == "coins");
    REQUIRE(t[0]["id"] == 1);
    REQUIRE(t[0]["solid"] == true);
    // Hidden, the layer is neither drawn nor solid; shown again, it is.
    REQUIRE(s.command("tilemap.layer", Json{{"entity", "Level"}, {"name", "extra"}, {"visible", false}}).value()["changed"] == true);
    REQUIRE_FALSE(solid_at(3, 1));
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_layers"].get<int>() == drawn_before);
    REQUIRE(s.command("tilemap.layer", Json{{"entity", "Level"}, {"name", "extra"}, {"visible", true}, {"opacity", 0.5}}).value()["opacity"].get<double>() == Catch::Approx(0.5));
    REQUIRE(solid_at(3, 1));
    REQUIRE(s.command("tilemap.layer", Json{{"entity", "Level"}, {"name", "extra"}}).value()["changed"] == false);
    REQUIRE(s.command("tilemap.layer", Json{{"entity", "Level"}, {"name", "nope"}}).error().code == "unknown_layer");
    // Moved under the others, it is drawn first and saved first; moved back, last.
    Json moved = s.command("tilemap.layer", Json{{"entity", "Level"}, {"name", "extra"}, {"index", 0}}).value();
    REQUIRE(moved["index"] == 0);
    REQUIRE(moved["changed"] == true);
    REQUIRE(s.command("tilemap.info", Json{{"entity", "Level"}}).value()["layers"][0]["name"] == "extra");
    REQUIRE(s.command("tilemap.layer", Json{{"entity", "Level"}, {"name", "extra"}, {"index", 9}}).error().code == "bad_args");
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_layers"].get<int>() == drawn_before + 1);
    {
        REQUIRE(s.command("tilemap.save", Json{{"entity", "Level"}, {"path", "assets/level-layers.tmj"}}).has_value());
        auto first = fs::read_text(root() / "samples" / "sprites" / "assets" / "level-layers.tmj");
        REQUIRE(first.has_value());
        auto parsed = assets::parse_tilemap(*first, "assets/level-layers.tmj");
        REQUIRE(parsed.has_value());
        REQUIRE(parsed->layers.front().name == "extra");
        REQUIRE(parsed->layers.front().gids[1 * parsed->width + 3] == 1);
    }
    REQUIRE(s.command("tilemap.layer", Json{{"entity", "Level"}, {"name", "extra"}, {"index", layers_before}}).value()["index"].get<std::size_t>() == layers_before);
    // Saved with the map: the new layer, its tiles and the new tileset come back from the file.
    const auto saved_path = root() / "samples" / "sprites" / "assets" / "level-layers.tmj";
    REQUIRE(s.command("tilemap.save", Json{{"entity", "Level"}, {"path", "assets/level-layers.tmj"}}).value()["layers"].get<std::size_t>() == layers_before + 1);
    auto text = fs::read_text(saved_path);
    REQUIRE(text.has_value());
    auto again = assets::parse_tilemap(*text, "assets/level-layers.tmj");
    REQUIRE(again.has_value());
    REQUIRE(again->layers.size() == layers_before + 1);
    REQUIRE(again->layers.back().name == "extra");
    REQUIRE(again->layers.back().solid_layer());
    REQUIRE(again->layers.back().opacity == Catch::Approx(0.5));
    REQUIRE(again->layers.back().gids[1 * again->width + 3] == 1);
    REQUIRE(again->layers.back().gids[1 * again->width + 6] == coin_gid);
    REQUIRE(again->tilesets.size() == sets_before + 1);
    REQUIRE(again->tilesets.back().name == "coins");
    REQUIRE(again->tilesets.back().first_gid == set["first_gid"].get<std::uint32_t>());
    REQUIRE(again->tilesets.back().tile_count == 4);
    REQUIRE(again->tilesets.back().solid(1));
    REQUIRE(again->tilesets.back().image == "assets/coin.png");
    std::filesystem::remove(saved_path);
    // A tileset with tiles on a layer stays; the layer gone, it can go, and the file loses it.
    REQUIRE(s.command("tilemap.remove_tileset", Json{{"entity", "Level"}, {"name", "coins"}}).error().code == "tileset_in_use");
    REQUIRE(s.command("tilemap.remove_tileset", Json{{"entity", "Level"}, {"name", "nope"}}).error().code == "unknown_tileset");
    // Removed, the layer and its tiles are gone, from the map and from the next save.
    REQUIRE(s.command("tilemap.remove_layer", Json{{"entity", "Level"}, {"name", "extra"}}).value()["layers"].get<std::size_t>() == layers_before);
    REQUIRE(s.command("tilemap.remove_tileset", Json{{"entity", "Level"}, {"name", "coins"}}).value()["tilesets"].get<std::size_t>() == sets_before);
    REQUIRE(s.command("tilemap.info", Json{{"entity", "Level"}}).value()["tilesets"].size() == sets_before);
    REQUIRE(s.command("tilemap.remove_layer", Json{{"entity", "Level"}, {"name", "extra"}}).error().code == "unknown_layer");
    REQUIRE(filled(tile_at(3, 1)).empty());
    REQUIRE_FALSE(has_layer(tile_at(3, 1), "extra"));
    REQUIRE_FALSE(solid_at(3, 1));
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_layers"].get<int>() == drawn_before);
    REQUIRE(s.command("tilemap.save", Json{{"entity", "Level"}, {"path", "assets/level-layers.tmj"}}).has_value());
    text = fs::read_text(saved_path);
    REQUIRE(text.has_value());
    again = assets::parse_tilemap(*text, "assets/level-layers.tmj");
    REQUIRE(again.has_value());
    REQUIRE(again->layers.size() == layers_before);
    REQUIRE(again->tilesets.size() == sets_before);
    std::filesystem::remove(saved_path);
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["tilemap.layer"].get<int>() == 6);   // added, hidden, shown (a read without changes emits nothing), moved twice, removed
    REQUIRE(hist["tilemap.tileset"].get<int>() == 2);   // added, removed
    REQUIRE(s.finish().has_value());
}

TEST_CASE("tiles are edited at runtime: drawn, solid, felt by bodies and saved", "[runtime][tilemap]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    const std::filesystem::path edited = o.project_dir / "assets" / "level-edited.tmj";
    std::filesystem::remove(edited);
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // Tile (10, 5) is sky: world x 0..1, y -1.5..-0.5 with the map's corner at (-10, 4.5).
    REQUIRE(s.command("tilemap.tile", Json{{"entity", "Level"}, {"tile_x", 10}, {"tile_y", 5}}).value()["solid"] == false);
    Json pr = s.command("render.project", Json{{"point", {{"x", 0.5}, {"y", -1.0}, {"z", 0.0}}}}).value();
    REQUIRE(pr["visible"] == true);
    REQUIRE(s.command("render.pick", Json{{"x", pr["x"]}, {"y", pr["y"]}}).value()["id"] == 0);
    // The pixel maps back to the point: render.unproject is the inverse of render.project.
    Json un = s.command("render.unproject", Json{{"x", pr["x"]}, {"y", pr["y"]}, {"plane", "xy"}, {"at", 0.0}}).value();
    INFO(un.dump());
    REQUIRE(un["hit"] == true);
    REQUIRE(un["point"]["x"].get<double>() == Catch::Approx(0.5).margin(0.05));
    REQUIRE(un["point"]["y"].get<double>() == Catch::Approx(-1.0).margin(0.05));
    REQUIRE(un["point"]["z"].get<double>() == Catch::Approx(0.0).margin(1e-3));
    // Put the ground tile there: solid at once, drawn on the next frame, one event.
    Json set = s.command("tilemap.set", Json{{"entity", "Level"}, {"tile_x", 10}, {"tile_y", 5}, {"id", 0}}).value();
    INFO(set.dump());
    REQUIRE(set["was"] == 0);
    REQUIRE(set["gid"] == 1);
    REQUIRE(set["changed"] == true);
    REQUIRE(set["layer"] == "ground");
    REQUIRE(set["revision"] == 1);
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Level"}, {"x", 0.5}, {"y", -1.0}}).value()["solid"] == true);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.pick", Json{{"x", pr["x"]}, {"y", pr["y"]}}).value()["name"] == "Level");
    Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["tile_layers"].get<int>() == 4);
    REQUIRE(stats["tile_rebuilds"].get<int>() == 1);
    bool saw = false;
    for (const Json& e : s.command("events.recent", Json{{"n", 50}}).value()) saw = saw || (e["type"] == "tilemap.changed" && e["data"]["tile_x"] == 10);
    REQUIRE(saw);
    // The same tile again changes nothing; bad layers, cells and gids are refused by name.
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Level"}, {"tile_x", 10}, {"tile_y", 5}, {"gid", 1}}).value()["changed"] == false);
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Level"}, {"tile_x", 10}, {"tile_y", 5}, {"gid", 1}, {"layer", "nope"}}).error().code == "unknown_layer");
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Level"}, {"tile_x", 50}, {"tile_y", 5}, {"gid", 1}}).error().code == "out_of_map");
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Level"}, {"tile_x", 1}, {"tile_y", 1}, {"gid", 99}}).error().code == "bad_gid");
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Level"}, {"tile_x", 1}, {"tile_y", 1}, {"id", 9}}).error().code == "bad_tile");
    // Clearing the ground under the player drops it: Body2D reads the edited map.
    Json fill = s.command("tilemap.fill", Json{{"entity", "Level"}, {"layer", "ground"}, {"tile_x", 9}, {"tile_y", 8}, {"width", 2}, {"height", 2}, {"clear", true}}).value();
    REQUIRE(fill["changed"] == 4);
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    Json st = s.command("state", Json::object()).value();
    INFO(st.dump());
    REQUIRE(st["state"]["player.y"].get<double>() < -4.0);
    REQUIRE(st["state"]["player.grounded"] == false);
    REQUIRE(s.command("render.stats", Json::object()).value()["tile_rebuilds"].get<int>() == 2);
    // Saved as Tiled JSON with the edits and everything else intact; escapes are refused.
    Json saved = s.command("tilemap.save", Json{{"entity", "Level"}, {"path", "assets/level-edited.tmj"}}).value();
    REQUIRE(saved["layers"] == 4);
    REQUIRE(std::filesystem::exists(edited));
    auto text = fs::read_text(edited);
    REQUIRE(text.has_value());
    auto again = assets::parse_tilemap(*text, "level-edited.tmj");
    REQUIRE(again.has_value());
    REQUIRE(again->layers.size() == 4);
    REQUIRE(again->layers[0].gids[5 * 20 + 10] == 1);
    REQUIRE(again->layers[0].gids[8 * 20 + 9] == 0);
    REQUIRE(again->layers[0].gids[8 * 20 + 8] == 1);
    REQUIRE(again->object_layers[0].objects.size() == 7);
    REQUIRE(again->properties["title"] == "coins");
    REQUIRE(again->tilesets[0].one_way(2));
    REQUIRE(s.command("tilemap.save", Json{{"entity", "Level"}, {"path", "../outside.tmj"}}).error().code == "forbidden");
    std::filesystem::remove(edited);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a perception benchmark answers through the instruments and meters what it cost", "[runtime][bench]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.scenario_bundle = root() / "build" / "ts" / "sprites.benches.perception.js";
    o.scenario = "how high does the player jump";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.seed = 1;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    int frames = 0;
    while (!s.quit_requested() && frames < 400) {
        REQUIRE(s.frame().has_value());
        ++frames;
    }
    Json st = s.command("state", Json::object()).value()["state"];
    INFO(st.dump());
    REQUIRE(st["__scenario"]["status"] == "passed");
    REQUIRE(st["__benches"].size() == 4);
    Json b = st["__bench"];
    REQUIRE(b["correct"] == true);
    REQUIRE(b["answer"].get<double>() == Catch::Approx(2.25).margin(0.15));   // v^2 / 2g, a step less
    REQUIRE(b["commands"].size() == 2);   // recorder.start, recorder.track
    REQUIRE(b["commands"][1]["method"] == "recorder.track");
    REQUIRE(b["tokens"].get<int>() > 0);
    REQUIRE(b["tokens"].get<int>() < 1500);
    REQUIRE(b["ticks"].get<int>() > 60);
    REQUIRE(b["frame_tokens"].get<int>() == b["ticks"].get<int>() * 692);   // 960x540 images at a token per 750 pixels
    REQUIRE(b["ratio"].get<double>() > 10.0);
}

TEST_CASE("the playground bakes a navigation grid and its enemies path around the pillars", "[runtime][nav]") {
    app::Options o;
    o.project_dir = root() / "samples" / "playground";
    o.bundle = root() / "build" / "ts" / "playground.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 300; ++i) REQUIRE(s.frame().has_value());   // long enough for an enemy to have gone around something
    Json info = s.command("nav.info", Json::object()).value();
    INFO(info.dump());
    REQUIRE(info["baked"] == true);
    REQUIRE(info["source"] == "colliders");
    REQUIRE(info["walkable"].get<int>() > 1000);
    REQUIRE(info["walkable"].get<int>() < info["cells"].get<int>());
    // A straight line from the south-east corner to the player crosses the south-east pillar; the
    // path bends around it and never enters one (the pillars stand on the diagonals at (+-2.5, +-2.5)).
    Json path = s.command("nav.path", Json{{"from", Json{{"x", 6.0}, {"y", 0.0}, {"z", 6.0}}}, {"to", "/Level/Player"}}).value();
    INFO(path.dump());
    REQUIRE(path["partial"] == false);
    REQUIRE(path["points"].size() > 2);
    for (const Json& pt : path["points"]) {
        double x = pt["x"].get<double>(), z = pt["z"].get<double>();
        bool in_pillar = std::fabs(std::fabs(x) - 2.5) < 0.95 && std::fabs(std::fabs(z) - 2.5) < 0.95;
        REQUIRE_FALSE(in_pillar);
    }
    REQUIRE(path["length"].get<double>() > 8.5);
    REQUIRE(s.command("nav.reachable", Json{{"from", Json{{"x", 6.0}, {"y", 0.0}, {"z", 6.0}}}, {"to", Json{{"x", 2.5}, {"y", 0.0}, {"z", 2.5}}}}).value()["reachable"] == false);
    REQUIRE(s.command("nav.reachable", Json{{"from", Json{{"x", 6.0}, {"y", 0.0}, {"z", 6.0}}}, {"to", Json{{"x", -6.0}, {"y", 0.0}, {"z", -6.0}}}}).value()["reachable"] == true);
    // The nearest walkable ground to a pillar's center is beside it, not its top (1.5 up).
    Json near = s.command("nav.nearest", Json{{"point", Json{{"x", 2.5}, {"y", 0.0}, {"z", 2.5}}}, {"radius", 2.0}}).value();
    REQUIRE_FALSE(near.is_null());
    REQUIRE(std::hypot(near["x"].get<double>() - 2.5, near["z"].get<double>() - 2.5) > 0.9);
    REQUIRE(near["y"].get<double>() == Catch::Approx(0.0).margin(0.01));
    REQUIRE(s.command("nav.path", Json{{"from", Json{{"x", 60.0}, {"y", 0.0}, {"z", 0.0}}}, {"to", "/Level/Player"}}).error().code == "outside");
    // The navmesh over the baked grid: rectangles with shared edges, a path across the arena over
    // it (far fewer nodes than the cells), the cells on request.
    REQUIRE(info["mesh"]["polygons"].get<int>() >= 4);
    Json mesh = s.command("nav.mesh", Json::object()).value();
    INFO(mesh.dump().substr(0, 400));
    REQUIRE(mesh["count"] == info["mesh"]["polygons"]);
    REQUIRE(mesh["polygons"][0].contains("neighbours"));
    Json over = s.command("nav.path", Json{{"from", Json{{"x", -8.0}, {"y", 0.0}, {"z", -8.0}}}, {"to", Json{{"x", 8.0}, {"y", 0.0}, {"z", 8.0}}}}).value();
    Json by_cells = s.command("nav.path", Json{{"from", Json{{"x", -8.0}, {"y", 0.0}, {"z", -8.0}}}, {"to", Json{{"x", 8.0}, {"y", 0.0}, {"z", 8.0}}}, {"mesh", false}}).value();
    INFO(over.dump() << "\n" << by_cells.dump());
    REQUIRE(over["mesh"] == true);
    REQUIRE(by_cells["mesh"] == false);
    REQUIRE(over["partial"] == false);
    REQUIRE(over["expanded"].get<int>() < by_cells["expanded"].get<int>());
    REQUIRE(over["length"].get<double>() <= by_cells["length"].get<double>() + 0.05);
    // The script steered enemies along paths with corners, and the overlay draws the grid and the paths.
    Json st = s.command("state", Json::object()).value()["state"];
    REQUIRE(st["nav.cells"].get<int>() == info["walkable"].get<int>());
    REQUIRE(st["nav.detours"].get<int>() > 0);
    // The enemies are agents following the player; the cart is an obstacle blocking the cells under it.
    REQUIRE(info["obstacles"].get<int>() == 1);
    REQUIRE(info["blocked"].get<int>() > 0);
    REQUIRE(info["agents"].get<int>() > 0);
    REQUIRE(st["nav.blocked"].get<int>() == info["blocked"].get<int>());
    Json agents = s.command("nav.agents", Json::object()).value();
    REQUIRE(agents.is_array());
    REQUIRE(agents.size() >= 1);
    for (const Json& a : agents) {
        REQUIRE(a["mode"].get<int>() == 2);
        REQUIRE(a["state"].get<int>() >= 1);
        REQUIRE(a["path"].get<std::string>().starts_with("/Level/Enemy"));
    }
    REQUIRE(st["nav.min_gap"].is_number());
    REQUIRE(st["nav.min_gap"].get<double>() > 0.5);
    // A path straight across the cart's track bends around the cart, wherever it has rolled to.
    Json cart_t = s.command("world.get", Json{{"entity", "/Level/Cart"}, {"component", "Transform"}}).value();
    const double cart_x = cart_t["position"]["x"].get<double>(), cart_z = cart_t["position"]["z"].get<double>();
    Json cart_path = s.command("nav.path", Json{{"from", Json{{"x", cart_x}, {"y", 0.0}, {"z", 7.5}}}, {"to", Json{{"x", cart_x}, {"y", 0.0}, {"z", 0.5}}}}).value();
    INFO(cart_path.dump() << " cart at " << cart_x << "," << cart_z);
    REQUIRE(cart_path["partial"] == false);
    REQUIRE(cart_path["points"].size() > 2);
    for (const Json& pt : cart_path["points"]) REQUIRE(std::hypot(pt["x"].get<double>() - cart_x, pt["z"].get<double>() - cart_z) > 1.1);
    REQUIRE(s.command("render.debug", Json{{"nav", true}}).value()["nav"] == true);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() > 1000);
    bool baked_event = false;
    for (const Json& e : s.command("events.since", Json{{"seq", 0}, {"type", "nav.baked"}, {"limit", 5}}).value()["events"]) baked_event = baked_event || e["data"]["source"] == "colliders";
    REQUIRE(baked_event);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("physics layers are named by the project and used by queries", "[runtime][layers]") {
    app::Options o;
    o.project_dir = root() / "samples" / "physics";
    o.bundle = root() / "build" / "ts" / "physics.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json layers = s.command("physics.layers", Json::object()).value();
    REQUIRE(layers["names"] == Json::array({"arena", "bodies", "marble"}));
    REQUIRE(layers["bits"]["marble"].get<int>() == 4);
    REQUIRE(s.command("physics.stats", Json::object()).value()["layers"].size() == 3);
    // Put the marble on its layer and ask for it by name.
    REQUIRE(s.command("world.set", Json{{"entity", "Marble"}, {"component", "Collider"}, {"value", {{"layer", 4}}}}).has_value());
    for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
    Json hit = s.command("physics.raycast", Json{{"origin", {{"x", 1.6}, {"y", 6}, {"z", 8}}}, {"direction", {{"x", 0}, {"y", -1}, {"z", 0}}}, {"mask", Json::array({"marble"})}}).value();
    INFO(hit.dump());
    REQUIRE(hit["path"] == "/Marble");
    Json bowl = s.command("physics.raycast", Json{{"origin", {{"x", 1.6}, {"y", 6}, {"z", 8}}}, {"direction", {{"x", 0}, {"y", -1}, {"z", 0}}}, {"mask", 1}}).value();
    REQUIRE(bowl["path"] == "/Bowl");
    Json near = s.command("physics.overlap", Json{{"center", {{"x", 1.6}, {"y", 4}, {"z", 8}}}, {"radius", 0.5}, {"mask", Json::array({"arena"})}}).value();
    REQUIRE(near.empty());
}

TEST_CASE("Body2D walks slopes and steps without leaving the ground and rides kinematic platforms", "[runtime][body2d][platforms]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto body = [&](const char* path) { return s.command("world.get", Json{{"entity", path}, {"component", "Body2D"}}).value(); };
    auto pos = [&](const char* path) { Json t = s.command("world.get", Json{{"entity", path}, {"component", "Transform"}}).value(); return Vec3{t["position"]["x"].get<float>(), t["position"]["y"].get<float>(), 0}; };
    auto push = [&](const char* path, double vx) { REQUIRE(s.command("world.set", Json{{"entity", path}, {"component", "Body2D"}, {"value", Json{{"velocity", Json{{"x", vx}}}}}}).has_value()); };
    // The hill: a slope up at cell 16, a block at 17 (top at y -2.5), a slope down at 18, all on row 7.
    // A crate on the ground at x 5.5 walks right at three units per second and stays grounded throughout.
    REQUIRE(s.command("world.spawn", Json{{"name", "Walker"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 5.5}, {"y", -3.2}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}}}}}}).has_value());
    REQUIRE(s.frame().has_value());  // the spawn's landing
    float top = -10;
    int airborne = 0;
    bool up = false, down = false;
    for (int i = 0; i < 60; ++i) {
        push("Walker", 3.0);
        REQUIRE(s.frame().has_value());
        Json b = body("Walker");
        if (b["grounded"] != true) airborne++;
        if (b["on_slope"].get<int>() == 1) up = true;
        if (b["on_slope"].get<int>() == -1) down = true;
        top = std::max(top, pos("Walker").y);
    }
    INFO(body("Walker").dump() << " at " << pos("Walker").x << "," << pos("Walker").y);
    REQUIRE(airborne == 0);
    REQUIRE(up);
    REQUIRE(down);
    REQUIRE(top == Catch::Approx(-2.2f).margin(0.02f));           // stood on the block
    REQUIRE(pos("Walker").x == Catch::Approx(8.5f).margin(0.1f));
    REQUIRE(pos("Walker").y == Catch::Approx(-2.7f).margin(0.06f));  // halfway down the far slope
    REQUIRE(body("Walker")["on_slope"].get<int>() == -1);
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["body2d.landed"].get<int>() == 2);                   // the player's and the walker's spawn landings only
    Json tstats = s.command("physics.stats", Json::object()).value()["tiles"];
    INFO(tstats.dump());
    REQUIRE(tstats["platforms"].get<int>() == 1);                     // the sample's lift
    // A solid platform moving right carries the crate that lands on it.
    REQUIRE(s.command("world.spawn", Json{{"name", "Mover"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -8}, {"y", -1.0}, {"z", 0}}}}}, {"Body2D", Json{{"kinematic", true}, {"size", Json{{"x", 0.6}, {"y", 0.15}}}, {"velocity", Json{{"x", 2.0}, {"y", 0.0}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Rider"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -8}, {"y", 0}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}}}}}}).has_value());
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json rider = body("Rider");
    INFO(rider.dump() << " rider at " << pos("Rider").x << "," << pos("Rider").y << " mover at " << pos("Mover").x);
    REQUIRE(rider["grounded"] == true);
    REQUIRE(rider["riding"].get<std::uint64_t>() == s.command("world.find", Json{{"path", "Mover"}}).value().get<std::uint64_t>());
    REQUIRE(pos("Mover").x == Catch::Approx(-6.0f).margin(0.01f));
    REQUIRE(pos("Rider").x > -7.0f);                                  // carried along since it landed
    REQUIRE(pos("Rider").y == Catch::Approx(-0.85f + 0.3f).margin(0.01f));
    REQUIRE(s.command("physics.stats", Json::object()).value()["tiles"]["riding"].get<int>() >= 1);
    // A one-way platform is passed from below and landed on (at x 1, under open sky and out of the
    // mover's way); a solid one blocks sideways.
    REQUIRE(s.command("world.spawn", Json{{"name", "OneWay"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 1}, {"y", -1.5}, {"z", 0}}}}}, {"Body2D", Json{{"kinematic", true}, {"one_way", true}, {"size", Json{{"x", 0.6}, {"y", 0.15}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Jumper"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 1}, {"y", -3.2}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}, {"velocity", Json{{"x", 0.0}, {"y", 12.0}}}}}}}}).has_value());
    bool ceiling = false;
    for (int i = 0; i < 120; ++i) {
        REQUIRE(s.frame().has_value());
        if (body("Jumper")["on_ceiling"] == true) ceiling = true;
    }
    REQUIRE_FALSE(ceiling);
    REQUIRE(pos("Jumper").y == Catch::Approx(-1.35f + 0.3f).margin(0.01f));
    REQUIRE(body("Jumper")["riding"].get<std::uint64_t>() == s.command("world.find", Json{{"path", "OneWay"}}).value().get<std::uint64_t>());
    REQUIRE(s.command("world.spawn", Json{{"name", "Wall"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 2.0}, {"y", -3.0}, {"z", 0}}}}}, {"Body2D", Json{{"kinematic", true}, {"size", Json{{"x", 0.2}, {"y", 0.5}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Pusher"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0.5}, {"y", -3.2}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}}}}}}).has_value());
    for (int i = 0; i < 40; ++i) {
        push("Pusher", 6.0);
        REQUIRE(s.frame().has_value());
    }
    REQUIRE(body("Pusher")["on_wall"].get<int>() == 1);
    REQUIRE(pos("Pusher").x == Catch::Approx(1.8f - 0.3f).margin(0.01f));
}

TEST_CASE("the environment interface resets, acts and observes the sprites sample", "[runtime][env]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 5; ++i) REQUIRE(s.frame().has_value());  // some history before the first episode
    Json d = s.command("env.describe", Json::object()).value();
    INFO(d.dump());
    REQUIRE(d["actions"].contains("move_x"));
    REQUIRE(d["actions"].contains("jump"));
    REQUIRE(d["score_key"] == "score");
    REQUIRE(d["has_done"] == true);
    REQUIRE(d["episode"] == 0);
    Json first = s.command("env.reset", Json{{"seed", 5}, {"max_ticks", 400}}).value();
    INFO(first.dump());
    REQUIRE(first["episode"] == 1);
    REQUIRE(first["t"] == 0);
    REQUIRE(first["tick"] == 5);
    REQUIRE(first["done"] == false);
    REQUIRE(first["score"] == 0);
    REQUIRE(first["reward"] == 0.0);
    REQUIRE(first["state"]["player.x"] == 0);
    REQUIRE(first["events"].empty());
    Json walked = s.command("env.step", Json{{"actions", Json{{"move_x", 1}}}, {"ticks", 30}}).value();
    INFO(walked.dump());
    REQUIRE(walked["t"] == 30);
    REQUIRE(walked["tick"] == 35);
    REQUIRE(walked["state"]["player.x"].get<double>() > 0.5);
    REQUIRE(walked["score"].get<double>() >= 1);
    REQUIRE(walked["reward"].get<double>() == walked["score"].get<double>());
    bool collected = false;
    for (const Json& e : walked["events"]) collected = collected || e["type"] == "coin.collected";
    REQUIRE(collected);
    REQUIRE(walked["actions"]["move_x"]["down"] == true);  // still held on the step's last tick
    Json jumped = s.command("env.step", Json{{"actions", Json{{"jump", true}}}, {"ticks", 60}}).value();
    REQUIRE(jumped["t"] == 90);
    bool landed = false, jumped_ev = false;
    for (const Json& e : jumped["events"]) { landed = landed || e["type"] == "body2d.landed"; jumped_ev = jumped_ev || e["type"] == "player.jumped"; }
    REQUIRE(jumped_ev);
    REQUIRE(landed);
    REQUIRE(jumped["reward"].get<double>() == 0.0);
    REQUIRE(s.command("env.observe", Json::object()).value()["t"] == 90);
    REQUIRE(s.command("env.observe", Json::object()).value()["events"].empty());  // nothing new since
    // The same seed and acts give the same episode again; the tick counter carries on.
    Json again = s.command("env.reset", Json{{"seed", 5}}).value();
    REQUIRE(again["episode"] == 2);
    REQUIRE(again["t"] == 0);
    REQUIRE(again["tick"] == 95);
    REQUIRE(again["state"]["player.x"] == 0);
    REQUIRE(again["state"]["score"] == 0);
    Json walked2 = s.command("env.step", Json{{"actions", Json{{"move_x", 1}}}, {"ticks", 30}}).value();
    REQUIRE(walked2["state"] == walked["state"]);
    // Bad actions are refused before anything runs; max_ticks ends an episode; a capture rides along.
    REQUIRE(s.command("env.step", Json{{"actions", Json{{"fly", 1}}}}).error().code == "no_such_action");
    REQUIRE(s.command("env.step", Json{{"actions", Json{{"jump", "hard"}}}}).error().code == "bad_args");
    REQUIRE(s.command("env.observe", Json::object()).value()["t"] == 30);
    REQUIRE(s.command("env.reset", Json{{"max_ticks", 10}}).value()["episode"] == 3);
    Json ended = s.command("env.step", Json{{"ticks", 10}, {"capture", (root() / "build" / "test-out" / "env-capture.png").string()}}).value();
    REQUIRE(ended["done"] == true);
    REQUIRE(ended["t"] == 10);
    REQUIRE(std::filesystem::exists(root() / "build" / "test-out" / "env-capture.png"));
    Json events = s.command("events.since", Json{{"seq", 0}, {"type", "env.reset"}, {"limit", 10}}).value()["events"];
    REQUIRE(events.size() == 3);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("morph targets weigh vertices from a clip's weights track and from the Morph component", "[runtime][animation][morph]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json clips = s.command("animation.clips", Json{{"entity", "Pulse"}}).value();
    INFO(clips.dump());
    REQUIRE(clips["targets"] == Json::array({"bulge", "lean"}));
    REQUIRE(clips["clips"].size() == 5);
    Json described = s.command("assets.describe", Json{{"path", "assets/arm.glb"}}).value();
    REQUIRE(described["targets"] == Json::array({"bulge", "lean"}));
    // Half a second into the pulse clip the bulge weight is at its peak.
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    Json pose = s.command("animation.pose", Json{{"entity", "Pulse"}}).value();
    INFO(pose.dump());
    REQUIRE(pose["weights"].size() == 2);
    REQUIRE(pose["weights"][0]["target"] == "bulge");
    REQUIRE(pose["weights"][0]["weight"].get<double>() > 0.9);
    REQUIRE(pose["weights"][1]["weight"].get<double>() == 0.0);
    Json rs = s.command("render.stats", Json::object()).value();
    REQUIRE(rs["morphed"].get<int>() >= 1);
    Json st = s.command("state", Json::object()).value()["state"];
    REQUIRE(st["pulse.bulge"].get<double>() > 0.9);
    // The Morph component sets a target by name over the clip: the lean moves the arm's top toward +X.
    REQUIRE(s.command("world.set", Json{{"entity", "Pulse"}, {"component", "Animator"}, {"value", Json{{"clip", ""}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Pulse"}, {"component", "Morph"}, {"value", Json{{"weights", Json::array({Json{{"target", "lean"}, {"weight", 1.0}}})}}}}).has_value());
    REQUIRE(s.frame().has_value());
    pose = s.command("animation.pose", Json{{"entity", "Pulse"}}).value();
    REQUIRE(pose["weights"][0]["weight"].get<double>() == 0.0);
    REQUIRE(pose["weights"][1]["weight"].get<double>() == 1.0);
    // The mesh really leans: the top (bind space (0, 2, 0)) sits at +0.5 in X; picking there hits Pulse and the old top is empty.
    Json proj_lean = s.command("render.project", Json{{"point", {{"x", 3.2 + 0.5}, {"y", 1.95}, {"z", 0.4}}}}).value();
    REQUIRE(proj_lean["visible"] == true);
    Json pick_lean = s.command("render.pick", Json{{"x", proj_lean["x"]}, {"y", proj_lean["y"]}}).value();
    INFO(pick_lean.dump());
    REQUIRE(pick_lean["name"] == "Pulse");
    Json proj_old = s.command("render.project", Json{{"point", {{"x", 3.2 - 0.05}, {"y", 1.95}, {"z", 0.4}}}}).value();
    Json pick_old = s.command("render.pick", Json{{"x", proj_old["x"]}, {"y", proj_old["y"]}}).value();
    REQUIRE(pick_old["name"] != "Pulse");
    // An unknown target is ignored; the weight by index works too.
    REQUIRE(s.command("world.set", Json{{"entity", "Pulse"}, {"component", "Morph"}, {"value", Json{{"weights", Json::array({Json{{"target", "smile"}, {"weight", 1.0}}, Json{{"target", "0"}, {"weight", 0.25}}})}}}}).has_value());
    REQUIRE(s.frame().has_value());
    pose = s.command("animation.pose", Json{{"entity", "Pulse"}}).value();
    REQUIRE(pose["weights"][0]["weight"].get<double>() == 0.25);
    REQUIRE(pose["weights"][1]["weight"].get<double>() == 0.0);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("root motion carries the entity by the clip's root translation and pins the pose", "[runtime][animation][root]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto z = [&]() { return s.command("world.get", Json{{"entity", "Walker"}, {"component", "Transform"}}).value()["position"]["z"].get<double>(); };
    REQUIRE(z() == Catch::Approx(-1.5));
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    // One second of the walk clip: one unit along the entity's +Z; the root joint stays with the entity.
    REQUIRE(z() == Catch::Approx(-0.5).margin(0.02));
    Json anim = s.command("world.get", Json{{"entity", "Walker"}, {"component", "Animator"}}).value();
    REQUIRE(anim["root_delta"]["z"].get<double>() == Catch::Approx(1.0 / 60.0).margin(1e-4));
    Json pose = s.command("animation.pose", Json{{"entity", "Walker"}}).value();
    INFO(pose.dump());
    REQUIRE(pose["root"] == "root");
    REQUIRE(pose["root_motion"] == 1);
    REQUIRE(pose["joints"][0]["position"]["z"].get<double>() == Catch::Approx(z()).margin(0.02));
    // Across the loop's wrap the motion is continuous: half a second more is half a unit more.
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(z() == Catch::Approx(0.0).margin(0.03));
    // Mode 2 pins the root and reports the delta without moving the entity.
    REQUIRE(s.command("world.set", Json{{"entity", "Walker"}, {"component", "Animator"}, {"value", Json{{"root_motion", 2}}}}).has_value());
    const double before = z();
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(z() == Catch::Approx(before));
    anim = s.command("world.get", Json{{"entity", "Walker"}, {"component", "Animator"}}).value();
    REQUIRE(anim["root_delta"]["z"].get<double>() == Catch::Approx(1.0 / 60.0).margin(1e-4));
    // Off: the root's translation moves the mesh again and the delta is zero.
    REQUIRE(s.command("world.set", Json{{"entity", "Walker"}, {"component", "Animator"}, {"value", Json{{"root_motion", 0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    anim = s.command("world.get", Json{{"entity", "Walker"}, {"component", "Animator"}}).value();
    REQUIRE(anim["root_delta"]["z"].get<double>() == 0.0);
    // Turned around by the sample script at tick 180, the walker comes back the other way.
    REQUIRE(s.command("world.set", Json{{"entity", "Walker"}, {"component", "Animator"}, {"value", Json{{"root_motion", 1}}}}).has_value());
    for (int i = 0; i < 70; ++i) REQUIRE(s.frame().has_value());  // past tick 180
    const double turned = z();
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(z() < turned - 0.4);
    REQUIRE(s.command("state", Json::object()).value()["state"]["walker.turns"] == 1);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a bend limit keeps every joint of an IK chain within its cone", "[runtime][animation][ik][iklimit]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto v = [](const Json& j) { return Vec3{j["x"].get<float>(), j["y"].get<float>(), j["z"].get<float>()}; };
    auto pose = [&]() { return s.command("animation.pose", Json{{"entity", "Limb"}}).value(); };
    // A target close under the base needs the elbow folded to 141 degrees; free, the arm reaches it.
    Json spawn = Json{{"name", "Limb"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"IK", Json{{"end", "tip"}, {"bones", 2}, {"tip", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"target", {{"x", 0.0}, {"y", 0.6}, {"z", 0.3}}}}}}}};
    REQUIRE(s.command("world.spawn", spawn).has_value());
    REQUIRE(s.frame().has_value());
    Json p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(p["ik"]["bend"].get<double>() == Catch::Approx(140.8).margin(1.0));
    REQUIRE(p["ik"]["max_bend"].get<double>() == 180.0);
    // Limited to 90 degrees, the elbow stops at a right angle: the effector stays root 2 from the
    // base, the target is missed by the difference, and the report says so.
    REQUIRE(s.command("world.set", Json{{"entity", "Limb"}, {"component", "IK"}, {"value", Json{{"max_bend", 90.0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == false);
    REQUIRE(p["ik"]["bend"].get<double>() <= 90.5);
    REQUIRE(p["ik"]["bend"].get<double>() >= 85.0);
    const float reach = length(v(p["ik"]["effector"]));
    REQUIRE(reach == Catch::Approx(std::sqrt(2.0)).margin(0.05));
    REQUIRE(p["ik"]["error"].get<double>() > 0.5);
    REQUIRE(p["ik"]["error"].get<double>() < 1.0);
    REQUIRE(length(v(p["joints"][1]["position"])) == Catch::Approx(1.0).margin(0.01));   // bone lengths kept
    // The first joint is limited against the posed direction (+Y here, the root has no parent bone):
    // a target far along +X, out of reach, tilts the upper bone 45 degrees and the lower one 45 more.
    REQUIRE(s.command("world.set", Json{{"entity", "Limb"}, {"component", "IK"}, {"value", Json{{"max_bend", 45.0}, {"target", {{"x", 3.0}, {"y", 0.0}, {"z", 0.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(length(v(p["joints"][1]["position"]) - Vec3{0.7071f, 0.7071f, 0.0f}) < 0.02f);
    REQUIRE(length(v(p["ik"]["effector"]) - Vec3{1.7071f, 0.7071f, 0.0f}) < 0.03f);
    REQUIRE(p["ik"]["bend"].get<double>() == Catch::Approx(45.0).margin(1.0));
    REQUIRE(p["ik"]["reached"] == false);
    // Freed again, the chain stretches straight along +X.
    REQUIRE(s.command("world.set", Json{{"entity", "Limb"}, {"component", "IK"}, {"value", Json{{"max_bend", 180.0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(length(v(p["ik"]["effector"]) - Vec3{2.0f, 0.0f, 0.0f}) < 0.02f);
    REQUIRE(p["ik"]["bend"].get<double>() == Catch::Approx(90.0).margin(1.0));
}

TEST_CASE("per-joint IK limits give one joint its own most and least bend", "[runtime][animation][ik][iklimit][ikjoint]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto v = [](const Json& j) { return Vec3{j["x"].get<float>(), j["y"].get<float>(), j["z"].get<float>()}; };
    auto pose = [&]() { return s.command("animation.pose", Json{{"entity", "Knee"}}).value(); };
    // The chain is free (max_bend 180) but the tip joint alone may bend 45 degrees at most: the
    // folded target under the base is missed, the fold stopping at the tip's own limit.
    Json spawn = Json{{"name", "Knee"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"IK", Json{{"end", "tip"}, {"bones", 2}, {"tip", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"target", {{"x", 0.0}, {"y", 0.6}, {"z", 0.3}}}, {"limits", Json::array({Json{{"joint", "tip"}, {"max_bend", 45.0}}})}}}}}};
    REQUIRE(s.command("world.spawn", spawn).has_value());
    REQUIRE(s.frame().has_value());
    Json p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["limits"] == 1);
    REQUIRE(p["ik"]["reached"] == false);
    REQUIRE(p["ik"]["bend"].get<double>() <= 45.5);
    REQUIRE(p["ik"]["bend"].get<double>() >= 40.0);
    REQUIRE(length(v(p["ik"]["effector"])) == Catch::Approx(2.0 * std::cos(45.0 * 3.14159265 / 360.0)).margin(0.05));   // two unit bones 45 degrees apart
    // A least bend: the target almost straight up is within reach, but the tip must keep 30
    // degrees, so the effector stops short by the difference.
    REQUIRE(s.command("world.set", Json{{"entity", "Knee"}, {"component", "IK"}, {"value", Json{{"target", {{"x", 0.0}, {"y", 1.99}, {"z", 0.0}}}, {"limits", Json::array({Json{{"joint", "tip"}, {"min_bend", 30.0}}})}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["bend"].get<double>() == Catch::Approx(30.0).margin(1.0));
    REQUIRE(length(v(p["ik"]["effector"])) == Catch::Approx(2.0 * std::cos(30.0 * 3.14159265 / 360.0)).margin(0.02));
    REQUIRE(p["ik"]["error"].get<double>() == Catch::Approx(1.99 - 2.0 * std::cos(30.0 * 3.14159265 / 360.0)).margin(0.02));
    REQUIRE(p["ik"]["reached"] == false);
    // Without the entry the same target is reached with an eleven-degree bend (two unit bones
    // spanning 1.99: cos of the fold is (1.99^2 - 2) / 2).
    REQUIRE(s.command("world.set", Json{{"entity", "Knee"}, {"component", "IK"}, {"value", Json{{"limits", Json::array()}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(p["ik"]["bend"].get<double>() == Catch::Approx(11.46).margin(0.5));
}

TEST_CASE("a spatial source is heard from where its entity is: quieter with distance, panned to its side", "[runtime][audio][spatial]") {
    app::Options o;
    o.project_dir = root() / "samples" / "audio";
    o.bundle = root() / "build" / "ts" / "audio.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The camera at the origin looking down -Z: +X is its right.
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"rotation", {{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Ping"}, {"components", Json{{"Transform", Json{{"position", {{"x", 8}, {"y", 0}, {"z", 0}}}}}, {"AudioSource", Json{{"clip", "assets/hum.wav"}, {"autoplay", true}, {"loop", true}, {"volume", 1.0}, {"spatial", true}, {"near", 1.0}, {"range", 10.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    auto voice = [&]() {
        for (const Json& v : s.command("audio.list", Json::object()).value()) if (v["clip"] == "assets/hum.wav" && v["entity"] != 0) return v;
        return Json(nullptr);
    };
    Json v = voice();
    INFO(v.dump());
    REQUIRE(!v.is_null());
    // Eight units off to the right of a ten-unit range: about a fifth of the volume, panned right.
    REQUIRE(v["volume"].get<double>() == Catch::Approx((10.0 - 8.0) / 9.0).margin(0.02));
    REQUIRE(v["pan"].get<double>() > 0.5);
    // Two units to the left: nearly full, panned left.
    REQUIRE(s.command("world.set", Json{{"entity", "Ping"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", -2}, {"y", 0}, {"z", 0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    v = voice();
    REQUIRE(v["volume"].get<double>() == Catch::Approx((10.0 - 2.0) / 9.0).margin(0.02));
    REQUIRE(v["pan"].get<double>() < -0.5);
    // Within `near`, straight ahead: full and centered; beyond `range`: silent.
    REQUIRE(s.command("world.set", Json{{"entity", "Ping"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0}, {"z", -0.5}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    v = voice();
    REQUIRE(v["volume"].get<double>() == Catch::Approx(1.0).margin(0.01));
    REQUIRE(std::fabs(v["pan"].get<double>()) < 0.05);
    REQUIRE(s.command("world.set", Json{{"entity", "Ping"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0}, {"z", -40}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(voice()["volume"].get<double>() == Catch::Approx(0.0).margin(0.001));
    // An AudioListener entity stands in for the camera: at the source it hears it full and centered,
    // two units to its right it hears it on the left; disabled, the camera listens again.
    REQUIRE(s.command("world.spawn", Json{{"name", "Ear"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", -40}}}}}, {"AudioListener", Json::object()}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    v = voice();
    INFO(v.dump());
    REQUIRE(v["volume"].get<double>() == Catch::Approx(1.0).margin(0.01));
    REQUIRE(std::fabs(v["pan"].get<double>()) < 0.05);
    REQUIRE(s.command("world.set", Json{{"entity", "Ear"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 2}, {"y", 0}, {"z", -40}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    v = voice();
    REQUIRE(v["volume"].get<double>() == Catch::Approx((10.0 - 2.0) / 9.0).margin(0.02));
    REQUIRE(v["pan"].get<double>() < -0.5);
    REQUIRE(s.command("world.set", Json{{"entity", "Ear"}, {"component", "AudioListener"}, {"value", Json{{"enabled", false}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(voice()["volume"].get<double>() == Catch::Approx(0.0).margin(0.001));   // forty units from the camera again
    // A spatial one-shot follows its entity too, and needs one.
    REQUIRE(s.command("audio.play", Json{{"clip", "assets/beep.wav"}, {"spatial", true}}).error().code == "bad_args");
    REQUIRE(s.command("world.set", Json{{"entity", "Ping"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 4}, {"y", 0}, {"z", 0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json shot = s.command("audio.play", Json{{"clip", "assets/beep.wav"}, {"entity", "Ping"}, {"spatial", true}, {"range", 8.0}, {"volume", 0.5}}).value();
    REQUIRE(s.frame().has_value());
    Json bv(nullptr);
    for (const Json& x : s.command("audio.list", Json::object()).value()) if (x["id"] == shot["voice"]) bv = x;
    INFO(bv.dump());
    REQUIRE(!bv.is_null());
    REQUIRE(bv["volume"].get<double>() == Catch::Approx(0.5 * (8.0 - 4.0) / 7.0).margin(0.02));
    REQUIRE(bv["pan"].get<double>() > 0.5);
}

TEST_CASE("particles land on a floor, burst a child where they die and stretch along their motion", "[runtime][particles][floor][child][stretch]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto emitter = [&](const char* name, Vec3 at, Json fields) {
        fields["emitting"] = false;
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", {{"x", at.x}, {"y", at.y}, {"z", at.z}}}}}, {"ParticleEmitter", fields}}}}).has_value());
        REQUIRE(s.frame().has_value());   // its world transform is computed on the tick
        return s.command("world.find", Json{{"path", name}}).value().get<world::EntityId>();
    };
    auto pool = [&](world::EntityId id) {
        for (const Json& pl : s.command("particles.stats", Json::object()).value()["pools"]) if (pl["entity"].get<world::EntityId>() == id) return pl;
        return Json(nullptr);
    };
    auto list = [&](const char* name) { return s.command("particles.list", Json{{"entity", name}, {"limit", 1000}}).value()["particles"]; };
    // Sparks thrown sideways from two units up with a floor at half a unit: they fall, bounce, come
    // to rest on it and slide to a stop, never below it.
    const world::EntityId sparks = emitter("Sparks", {40, 2, 0}, Json{{"gravity", {{"x", 0}, {"y", -10}, {"z", 0}}}, {"lifetime", {{"x", 6}, {"y", 6}}}, {"speed", {{"x", 2}, {"y", 2}}}, {"direction", {{"x", 1}, {"y", 0}, {"z", 0}}}, {"spread", 0}, {"floor", 0.5}, {"bounce", 0.5}, {"floor_friction", 2.0}});
    REQUIRE(s.command("particles.burst", Json{{"entity", "Sparks"}, {"count", 20}}).value()["alive"] == 20);
    float lowest = 1e9f;
    int bounced = 0;
    for (int i = 0; i < 180; ++i) {
        REQUIRE(s.frame().has_value());
        for (const Json& p : list("Sparks")) {
            lowest = std::min(lowest, p["position"]["y"].get<float>());
            if (p["velocity"]["y"].get<float>() > 0.5f && p["position"]["y"].get<float>() < 0.6f) ++bounced;
        }
    }
    INFO("lowest " << lowest << " bounced " << bounced);
    REQUIRE(lowest >= 0.499f);
    REQUIRE(bounced > 0);   // seen going back up off the floor
    REQUIRE(pool(sparks)["landed"] == 20);
    Json rested = list("Sparks");
    REQUIRE(rested.size() == 20);
    for (const Json& p : rested) {
        REQUIRE(p["resting"] == true);
        REQUIRE(p["position"]["y"].get<float>() == Catch::Approx(0.5f).margin(1e-3f));
        REQUIRE(std::fabs(p["velocity"]["x"].get<float>()) < 0.1f);   // slid to a stop
        REQUIRE(p["position"]["x"].get<float>() > 40.0f);           // after sliding some way
    }
    // Shells that die after half a second, each bursting ten of a child emitter's particles where it died.
    const world::EntityId burst = emitter("Burst", {40, 0, 0}, Json{{"max", 500}, {"speed", {{"x", 1}, {"y", 2}}}, {"spread", 180}, {"gravity", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"lifetime", {{"x", 3}, {"y", 3}}}});
    const world::EntityId shells = emitter("Shells", {40, 5, 0}, Json{{"lifetime", {{"x", 0.5}, {"y", 0.5}}}, {"speed", {{"x", 3}, {"y", 3}}}, {"spread", 0}, {"gravity", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"child", burst}, {"child_count", 10}});
    REQUIRE(s.command("particles.burst", Json{{"entity", "Shells"}, {"count", 3}}).has_value());
    for (int i = 0; i < 40; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(pool(shells)["died"] == 3);
    REQUIRE(pool(burst)["spawned"] == 30);
    REQUIRE(pool(burst)["alive"] == 30);
    // They burst where the shells were: a unit and a half up the shells' way (3 m/s for half a second), give or take their own flight since.
    for (const Json& p : list("Burst")) REQUIRE(std::fabs(p["position"]["y"].get<float>() - 6.5f) < 1.2f);
    // A streak: a particle flying +X at ten a second drawn stretched by a fifth of a second covers a
    // unit ahead of itself, where a square one does not. Seen head-on from +Z, high up and off to the side.
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 60}, {"y", 3.5}, {"z", 6}}}, {"rotation", {{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
    const world::EntityId streak = emitter("Streak", {59, 3.5, 0}, Json{{"gravity", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"lifetime", {{"x", 10}, {"y", 10}}}, {"speed", {{"x", 10}, {"y", 10}}}, {"direction", {{"x", 1}, {"y", 0}, {"z", 0}}}, {"spread", 0}, {"size", {{"x", 0.2}, {"y", 0.2}}}, {"stretch", 0.2}});
    const world::EntityId dot = emitter("Dot", {59, 2.5, 0}, Json{{"gravity", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"lifetime", {{"x", 10}, {"y", 10}}}, {"speed", {{"x", 10}, {"y", 10}}}, {"direction", {{"x", 1}, {"y", 0}, {"z", 0}}}, {"spread", 0}, {"size", {{"x", 0.2}, {"y", 0.2}}}});
    REQUIRE(s.command("particles.burst", Json{{"entity", "Streak"}, {"count", 1}}).has_value());
    REQUIRE(s.command("particles.burst", Json{{"entity", "Dot"}, {"count", 1}}).has_value());
    REQUIRE(s.frame().has_value());
    auto pick_at = [&](double x, double y, double z) {
        Json pr = s.command("render.project", Json{{"point", {{"x", x}, {"y", y}, {"z", z}}}}).value();
        REQUIRE(pr["visible"] == true);
        return s.command("render.pick", Json{{"x", pr["x"].get<double>()}, {"y", pr["y"].get<double>()}}).value()["id"].get<world::EntityId>();
    };
    // After one tick both particles are a sixth of a unit along; the streak reaches a unit further.
    REQUIRE(pick_at(59.17, 3.5, 0.0) == streak);
    REQUIRE(pick_at(59.17, 2.5, 0.0) == dot);
    REQUIRE(pick_at(59.9, 3.5, 0.0) == streak);
    REQUIRE(pick_at(59.9, 2.5, 0.0) != dot);
}

TEST_CASE("a clip turns an unskinned part: the blade is drawn where its node puts it", "[runtime][animation][parts]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The camera straight in front of the fan, which stands at the origin with its blade half a unit up.
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0.5}, {"z", 6}}}, {"rotation", {{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Fan"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/fan.glb"}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const world::EntityId fan = s.command("world.find", Json{{"path", "Fan"}}).value().get<world::EntityId>();
    auto pick_at = [&](double x, double y, double z) {
        Json pr = s.command("render.project", Json{{"point", {{"x", x}, {"y", y}, {"z", z}}}}).value();
        INFO(pr.dump());
        REQUIRE(pr["visible"] == true);
        return s.command("render.pick", Json{{"x", pr["x"].get<double>()}, {"y", pr["y"].get<double>()}}).value()["id"].get<world::EntityId>();
    };
    auto parts = [&]() { return s.command("animation.pose", Json{{"entity", "Fan"}}).value()["parts"]; };
    // At rest the blade lies along +X: its tip at (0.7, 0.5, 0) is the fan, and above the tip is empty.
    Json p = parts();
    INFO(p.dump());
    REQUIRE(p.size() == 1);
    REQUIRE(p[0]["name"] == "Blade");
    REQUIRE(p[0]["position"]["y"].get<double>() == Catch::Approx(0.5));
    REQUIRE(p[0]["axis_x"]["x"].get<double>() == Catch::Approx(1.0).margin(1e-3));
    REQUIRE(pick_at(0.7, 0.5, 0.0) == fan);
    REQUIRE(pick_at(0.7, 0.9, 0.0) != fan);
    REQUIRE(s.command("render.stats", Json::object()).value()["moving_parts"].get<int>() == 1);
    // A quarter of the spin clip later the blade points down -Z (a quarter turn about Y): the tip
    // has left +X for -Z, and the pose says so.
    REQUIRE(s.command("animation.play", Json{{"entity", "Fan"}, {"clip", "spin"}, {"loop", true}}).has_value());
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());   // half a second: 90 degrees
    p = parts();
    INFO(p.dump());
    REQUIRE(p[0]["axis_x"]["z"].get<double>() == Catch::Approx(-1.0).margin(0.05));
    REQUIRE(p[0]["position"]["y"].get<double>() == Catch::Approx(0.5));
    REQUIRE(pick_at(0.7, 0.5, 0.0) != fan);
    // Seen from +Z the turned blade is a short bar at the center: the point a little in front of the hub is it.
    REQUIRE(pick_at(0.0, 0.5, 0.7) == fan);
    REQUIRE(s.command("render.stats", Json::object()).value()["moving_parts"].get<int>() == 1);
}

TEST_CASE("a hinge bends one way only: the chain folds on the hinge's side or misses", "[runtime][animation][ik][iklimit][ikhinge]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto v = [](const Json& j) { return Vec3{j["x"].get<float>(), j["y"].get<float>(), j["z"].get<float>()}; };
    auto pose = [&]() { return s.command("animation.pose", Json{{"entity", "Leg"}}).value(); };
    auto joint = [&](const Json& p, const char* name) {
        for (const Json& j : p["joints"]) if (j["name"] == name) return v(j["position"]);
        return Vec3{0, 0, 0};
    };
    auto limits = [](Json lim) { return Json{{"limits", Json::array({std::move(lim)})}}; };
    // Two unit bones up +Y, the target a bone's length out and back (0, 1, -1): the fold is a right
    // angle either way, and the free chain folds on the side the pose leans to, with the tip
    // joint staying put at (0, 1, 0) and the last bone swinging back.
    Json spawn = Json{{"name", "Leg"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"IK", Json{{"end", "tip"}, {"bones", 2}, {"tip", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"target", {{"x", 0.0}, {"y", 1.0}, {"z", -1.0}}}}}}}};
    REQUIRE(s.command("world.spawn", spawn).has_value());
    REQUIRE(s.frame().has_value());
    Json p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(length(joint(p, "tip") - Vec3{0, 1, 0}) < 0.05f);
    // The tip joint a hinge bending toward +Z only: the same target is reached by the mirror fold,
    // the first bone swinging back and the last one bending forward from it.
    REQUIRE(s.command("world.set", Json{{"entity", "Leg"}, {"component", "IK"}, {"value", limits(Json{{"joint", "tip"}, {"side", {{"x", 0}, {"y", 0}, {"z", 1}}}})}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["limits"] == 1);
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(length(joint(p, "tip") - Vec3{0, 0, -1}) < 0.05f);
    REQUIRE(length(v(p["ik"]["effector"]) - Vec3{0, 1, -1}) < 0.02f);
    REQUIRE(p["ik"]["bend"].get<double>() == Catch::Approx(90.0).margin(1.0));
    // Bending toward -Z only: the first fold again.
    REQUIRE(s.command("world.set", Json{{"entity", "Leg"}, {"component", "IK"}, {"value", limits(Json{{"joint", "tip"}, {"side", {{"x", 0}, {"y", 0}, {"z", -1}}}})}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(length(joint(p, "tip") - Vec3{0, 1, 0}) < 0.05f);
    // Both joints hinges toward +Z: no fold reaches a point behind, the chain stays as near as it
    // can (straight up, a bone's length away).
    REQUIRE(s.command("world.set", Json{{"entity", "Leg"}, {"component", "IK"}, {"value", Json{{"limits", Json::array({Json{{"joint", "root"}, {"side", {{"x", 0}, {"y", 0}, {"z", 1}}}}, Json{{"joint", "tip"}, {"side", {{"x", 0}, {"y", 0}, {"z", 1}}}}})}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == false);
    REQUIRE(p["ik"]["error"].get<double>() > 1.3);
    // A hinge's least bend may be negative: the root locked straight (a hinge with no bend at
    // all) and the tip bending toward +Z, a target a bone's length from the tip joint and a
    // little behind needs the tip to go 17 degrees back; min_bend -30 allows it, 0 does not.
    Json both = Json{{"target", {{"x", 0.0}, {"y", 1.0 + std::cos(17.0 * 3.14159265 / 180.0)}, {"z", -std::sin(17.0 * 3.14159265 / 180.0)}}}, {"limits", Json::array({Json{{"joint", "root"}, {"side", {{"x", 0}, {"y", 0}, {"z", 1}}}, {"max_bend", 0.0}}, Json{{"joint", "tip"}, {"side", {{"x", 0}, {"y", 0}, {"z", 1}}}, {"min_bend", -30.0}}})}};
    REQUIRE(s.command("world.set", Json{{"entity", "Leg"}, {"component", "IK"}, {"value", both}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(length(joint(p, "tip") - Vec3{0, 1, 0}) < 0.02f);   // the root did not move
    REQUIRE(p["ik"]["bend"].get<double>() == Catch::Approx(17.0).margin(1.5));
    both["limits"][1]["min_bend"] = 0.0;
    REQUIRE(s.command("world.set", Json{{"entity", "Leg"}, {"component", "IK"}, {"value", both}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == false);
    REQUIRE(length(v(p["ik"]["effector"]) - Vec3{0, 2, 0}) < 0.02f);   // held straight
}

TEST_CASE("a look-at with a speed turns toward its target a little each tick", "[runtime][animation][lookat][lookatspeed]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The tip joint points +Y; the target lies 90 degrees away along +X; 90 degrees per second is
    // a degree and a half per tick.
    Json spawn = Json{{"name", "Turret"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"LookAt", Json{{"node", "tip"}, {"forward", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"target", {{"x", 2.0}, {"y", 1.0}, {"z", 0.0}}}, {"speed", 90.0}}}}}};
    REQUIRE(s.command("world.spawn", spawn).has_value());
    REQUIRE(s.frame().has_value());
    auto pose = [&]() { return s.command("animation.pose", Json{{"entity", "Turret"}}).value(); };
    Json p = pose();
    INFO(p.dump());
    REQUIRE(p["look_at"]["speed"].get<double>() == 90.0);
    REQUIRE(p["look_at"]["angle"].get<double>() == Catch::Approx(1.5).margin(0.05));
    for (int i = 0; i < 20; ++i) REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(p["look_at"]["angle"].get<double>() == Catch::Approx(31.5).margin(0.2));
    const Vec3 aim{p["look_at"]["aim"]["x"].get<float>(), p["look_at"]["aim"]["y"].get<float>(), p["look_at"]["aim"]["z"].get<float>()};
    REQUIRE(aim.x > 0.5f);   // part way from +Y toward +X
    REQUIRE(aim.y > 0.8f);
    for (int i = 0; i < 50; ++i) REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(p["look_at"]["angle"].get<double>() == Catch::Approx(90.0).margin(0.1));   // arrived, held by max_angle 90
    // Speed 0 aims at once: the target moved behind, the turn is the full limit next tick.
    REQUIRE(s.command("world.set", Json{{"entity", "Turret"}, {"component", "LookAt"}, {"value", Json{{"speed", 0.0}, {"target", {{"x", -2.0}, {"y", 1.0}, {"z", 0.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(p["look_at"]["angle"].get<double>() == Catch::Approx(90.0).margin(0.1));
    REQUIRE(p["look_at"]["aim"]["x"].get<double>() < -0.99);
}

TEST_CASE("inverse kinematics bends a chain to its target, with a pole, a weight and out of reach", "[runtime][animation][ik]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // An arm at the origin: two unit bones along +Y (root at 0, tip joint at 1, the effector at 2).
    Json spawn = Json{{"name", "Reach"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"IK", Json{{"end", "tip"}, {"bones", 2}, {"tip", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"target", {{"x", 1.0}, {"y", 1.2}, {"z", 0.3}}}}}}}};
    REQUIRE(s.command("world.spawn", spawn).has_value());
    REQUIRE(s.frame().has_value());
    auto pose = [&]() { return s.command("animation.pose", Json{{"entity", "Reach"}}).value(); };
    auto v = [](const Json& j) { return Vec3{j["x"].get<float>(), j["y"].get<float>(), j["z"].get<float>()}; };
    Json p = pose();
    INFO(p.dump());
    REQUIRE(p["posed"] == true);
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(p["ik"]["error"].get<double>() < 0.01);
    REQUIRE(length(v(p["ik"]["effector"]) - Vec3{1.0f, 1.2f, 0.3f}) < 0.01f);
    // The bones keep their lengths: the tip joint is one unit from the root and from the effector.
    Vec3 root_pos = v(p["joints"][0]["position"]), tip_pos = v(p["joints"][1]["position"]);
    REQUIRE(length(root_pos) < 1e-4f);
    REQUIRE(length(tip_pos) == Catch::Approx(1.0).margin(0.01));
    REQUIRE(length(v(p["ik"]["effector"]) - tip_pos) == Catch::Approx(1.0).margin(0.01));
    // Out of reach: the chain points straight at the target and the error is the shortfall.
    REQUIRE(s.command("world.set", Json{{"entity", "Reach"}, {"component", "IK"}, {"value", Json{{"target", {{"x", 3.0}, {"y", 0.0}, {"z", 0.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(p["ik"]["reached"] == false);
    REQUIRE(p["ik"]["error"].get<double>() == Catch::Approx(1.0).margin(0.01));
    REQUIRE(length(v(p["joints"][1]["position"]) - Vec3{1, 0, 0}) < 0.01f);
    REQUIRE(length(v(p["ik"]["effector"]) - Vec3{2, 0, 0}) < 0.01f);
    // A pole entity decides which way the elbow bends.
    REQUIRE(s.command("world.spawn", Json{{"name", "Pole"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 1}, {"z", 2}}}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Reach"}, {"component", "IK"}, {"value", Json{{"target", {{"x", 0.0}, {"y", 1.5}, {"z", 0.0}}}, {"pole_entity", "Pole"}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(v(p["joints"][1]["position"]).z > 0.3f);
    REQUIRE(s.command("world.set", Json{{"entity", "Pole"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 1}, {"z", -2}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());   // the pole's world transform lands a tick later
    p = pose();
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(v(p["joints"][1]["position"]).z < -0.3f);
    // Weight 0 leaves the pose alone and reports the error the pose has.
    REQUIRE(s.command("world.set", Json{{"entity", "Reach"}, {"component", "IK"}, {"value", Json{{"weight", 0.0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pose();
    REQUIRE(length(v(p["joints"][1]["position"]) - Vec3{0, 1, 0}) < 0.01f);
    REQUIRE(p["ik"]["error"].get<double>() == Catch::Approx(0.5).margin(0.01));
    // A target entity is followed, in the entity's own space: the arm moved and rotated still reaches it.
    REQUIRE(s.command("world.set", Json{{"entity", "Reach"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 2}, {"y", 0}, {"z", -1}}}, {"rotation", {{"x", 0}, {"y", 0.7071068}, {"z", 0}, {"w", 0.7071068}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Reach"}, {"component", "IK"}, {"value", Json{{"weight", 1.0}, {"target_entity", "Pole"}, {"pole_entity", ""}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Pole"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 2.8}, {"y", 1.0}, {"z", 0.2}}}}}}).has_value());
    for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
    p = pose();
    INFO(p.dump());
    REQUIRE(p["ik"]["reached"] == true);
    REQUIRE(length(v(p["ik"]["effector"]) - Vec3{2.8f, 1.0f, 0.2f}) < 0.01f);
    // The sample's Reacher follows the circling Orb: within reach most of the time.
    int reached = 0;
    for (int i = 0; i < 120; ++i) {
        REQUIRE(s.frame().has_value());
        if (s.command("world.get", Json{{"entity", "Reacher"}, {"component", "IK"}}).value()["reached"] == true) ++reached;
    }
    REQUIRE(reached > 60);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("look-at turns a node toward its target within its limit", "[runtime][animation][lookat]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The arm's tip node sits at (0, 1, 0) pointing +Y; the target lies straight to its +X.
    Json spawn = Json{{"name", "Gaze"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"LookAt", Json{{"node", "tip"}, {"forward", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"target", {{"x", 2.0}, {"y", 1.0}, {"z", 0.0}}}}}}}};
    REQUIRE(s.command("world.spawn", spawn).has_value());
    REQUIRE(s.frame().has_value());
    auto v = [](const Json& j) { return Vec3{j["x"].get<float>(), j["y"].get<float>(), j["z"].get<float>()}; };
    Json p = s.command("animation.pose", Json{{"entity", "Gaze"}}).value();
    INFO(p.dump());
    REQUIRE(p["look_at"]["angle"].get<double>() == Catch::Approx(90.0).margin(0.1));
    REQUIRE(length(v(p["joints"][1]["axis_y"]) - Vec3{1, 0, 0}) < 0.01f);
    REQUIRE(length(v(p["joints"][1]["position"]) - Vec3{0, 1, 0}) < 1e-3f);   // the node turns in place
    // A limit: 30 degrees toward the target, no more.
    REQUIRE(s.command("world.set", Json{{"entity", "Gaze"}, {"component", "LookAt"}, {"value", Json{{"max_angle", 30.0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = s.command("animation.pose", Json{{"entity", "Gaze"}}).value();
    REQUIRE(p["look_at"]["angle"].get<double>() == Catch::Approx(30.0).margin(0.1));
    REQUIRE(length(v(p["joints"][1]["axis_y"]) - Vec3{0.5f, 0.8660254f, 0}) < 0.01f);
    // Half the weight: half the turn.
    REQUIRE(s.command("world.set", Json{{"entity", "Gaze"}, {"component", "LookAt"}, {"value", Json{{"max_angle", 90.0}, {"weight", 0.5}}}}).has_value());
    REQUIRE(s.frame().has_value());
    p = s.command("animation.pose", Json{{"entity", "Gaze"}}).value();
    REQUIRE(p["look_at"]["angle"].get<double>() == Catch::Approx(45.0).margin(0.1));
    // The sample's Gazer aims at the circling Orb and reports the turn it makes.
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    Json gazer = s.command("world.get", Json{{"entity", "Gazer"}, {"component", "LookAt"}}).value();
    REQUIRE(gazer["angle"].get<double>() > 5.0);
    REQUIRE(gazer["angle"].get<double>() <= 70.0 + 1e-3);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("root rotation turns the entity with the clip's heading and follows the arc", "[runtime][animation][root][rotation]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The "turn" clip walks a quarter circle of radius one, turning 90 degrees, each second.
    Json spawn = Json{{"name", "Turn"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"Animator", Json{{"clip", "turn"}, {"root_motion", 1}, {"root_rotation", true}}}}}};
    REQUIRE(s.command("world.spawn", spawn).has_value());
    auto transform = [&]() { return s.command("world.get", Json{{"entity", "Turn"}, {"component", "Transform"}}).value(); };
    auto yaw = [&](const Json& t) { return 2.0 * std::atan2(t["rotation"]["y"].get<double>(), t["rotation"]["w"].get<double>()) * 180.0 / 3.14159265; };
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json t = transform();
    INFO(t.dump());
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(1.0).margin(0.03));
    REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(1.0).margin(0.03));
    REQUIRE(yaw(t) == Catch::Approx(90.0).margin(1.0));
    // The pose's root keeps the first frame's heading and place: the root joint stays with the entity.
    Json pose = s.command("animation.pose", Json{{"entity", "Turn"}}).value();
    INFO(pose.dump());
    // (the pose is reported through the world transform of the tick before, one step behind)
    REQUIRE(pose["joints"][0]["position"]["x"].get<double>() == Catch::Approx(t["position"]["x"].get<double>()).margin(0.05));
    REQUIRE(pose["joints"][0]["position"]["z"].get<double>() == Catch::Approx(t["position"]["z"].get<double>()).margin(0.05));
    // Across the wrap the arc continues: a second quarter lands at (2, 0, 0) facing -Z, a full circle at the start.
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    t = transform();
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(2.0).margin(0.05));
    REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(0.0).margin(0.05));
    REQUIRE(std::abs(yaw(t)) == Catch::Approx(180.0).margin(1.5));
    for (int i = 0; i < 120; ++i) REQUIRE(s.frame().has_value());
    t = transform();
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(0.0).margin(0.08));
    REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(0.0).margin(0.08));
    Json anim = s.command("world.get", Json{{"entity", "Turn"}, {"component", "Animator"}}).value();
    REQUIRE(anim["root_delta_yaw"].get<double>() == Catch::Approx(3.14159265 / 2.0 / 60.0).margin(1e-3));
    // Mode 2 reports the yaw without turning the entity.
    REQUIRE(s.command("world.set", Json{{"entity", "Turn"}, {"component", "Animator"}, {"value", Json{{"root_motion", 2}}}}).has_value());
    const double before = yaw(transform());
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(yaw(transform()) == Catch::Approx(before).margin(1e-6));
    anim = s.command("world.get", Json{{"entity", "Turn"}, {"component", "Animator"}}).value();
    REQUIRE(anim["root_delta_yaw"].get<double>() == Catch::Approx(3.14159265 / 2.0 / 60.0).margin(1e-3));
    // The sample's Turner has walked its circle too.
    REQUIRE(s.command("state", Json::object()).value()["state"].contains("turner.yaw"));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("2D bodies bounce by their restitution and slide to a stop by friction", "[runtime][body2d][bounce]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto spawn = [&](const char* name, double x, double y, Json body) {
        Json b = Json{{"size", Json{{"x", 0.2}, {"y", 0.2}}}, {"collide_bodies", false}};
        for (auto& [k, v] : body.items()) b[k] = v;
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", 0}}}}}, {"Body2D", b}}}}).has_value());
    };
    auto body = [&](const char* path) { return s.command("world.get", Json{{"entity", path}, {"component", "Body2D"}}).value(); };
    auto pos = [&](const char* path) { Json t = s.command("world.get", Json{{"entity", path}, {"component", "Transform"}}).value(); return Vec3{t["position"]["x"].get<float>(), t["position"]["y"].get<float>(), 0}; };
    auto bounces = [&](const char* path) {
        int n = 0;
        for (const Json& ev : s.command("events.since", Json{{"seq", 0}, {"type", "body2d.bounced"}, {"limit", 200}}).value()["events"]) if (ev["data"]["path"] == path) ++n;
        return n;
    };
    // A ball dropped from 2.8 units above the ground (top at -3.5) with restitution 0.7.
    spawn("Bouncer", 0.9, -0.5, Json{{"restitution", 0.7}});
    // A puck shoved at 4 units per second with friction 5: it slides 1.6 units and stops.
    spawn("Slider", -2.0, -3.3, Json{{"friction", 5.0}, {"velocity", Json{{"x", 4.0}, {"y", 0.0}}}});
    // A stone with no restitution lands and stays; a dead-slow bounce lands too.
    spawn("Pebble", 1.4, -0.5, Json::object());
    bool bounced_up = false;
    float apex = -10;
    for (int i = 0; i < 60; ++i) {
        REQUIRE(s.frame().has_value());
        Json b = body("Bouncer");
        if (bounces("/Bouncer") >= 1 && b["velocity"]["y"].get<double>() > 0 && b["grounded"] == false) bounced_up = true;
        if (bounces("/Bouncer") >= 1) apex = std::max(apex, pos("Bouncer").y);   // the first bounce's top
    }
    REQUIRE(bounced_up);
    INFO(body("Bouncer").dump() << " at " << pos("Bouncer").y << " apex " << apex);
    REQUIRE(bounces("/Bouncer") >= 1);
    Json first;
    for (const Json& ev : s.command("events.since", Json{{"seq", 0}, {"type", "body2d.bounced"}, {"limit", 200}}).value()["events"]) { if (ev["data"]["path"] == "/Bouncer") { first = ev["data"]; break; } }
    REQUIRE(first["path"] == "/Bouncer");
    REQUIRE(first["side"] == "floor");
    REQUIRE(first["speed"].get<double>() > 10.0);   // sqrt(2 g h) with g 24 and h 2.8 is 11.6
    // The first bounce rises to about half the drop (0.49 of it): well above the ground, well below the start.
    REQUIRE(apex > -2.6f);
    REQUIRE(apex < -1.4f);
    for (int i = 0; i < 180; ++i) REQUIRE(s.frame().has_value());
    Json ball = body("Bouncer");
    INFO(ball.dump());
    REQUIRE(ball["grounded"] == true);                       // settled after the bounces died out
    REQUIRE(bounces("/Bouncer") >= 5);
    REQUIRE(pos("Bouncer").y == Catch::Approx(-3.3f).margin(0.01f));
    REQUIRE(bounces("/Pebble") == 0);
    REQUIRE(body("Pebble")["grounded"] == true);
    // The puck slid and stopped: v*v / (2 a) = 16 / 10 = 1.6 units, grounded all along.
    Json puck = body("Slider");
    INFO(puck.dump() << " at " << pos("Slider").x);
    REQUIRE(puck["velocity"]["x"].get<double>() == 0.0);
    REQUIRE(puck["grounded"] == true);
    REQUIRE(pos("Slider").x == Catch::Approx(-0.4f).margin(0.08f));
    // A wall: a ball thrown at a kinematic block comes back with 0.7 of its speed, and the
    // block's top bounces a ball dropped on it (a platform is a floor).
    REQUIRE(s.command("world.spawn", Json{{"name", "Block"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 2.0}, {"y", -3.0}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.3}, {"y", 0.5}}}, {"kinematic", true}}}}}}).has_value());
    spawn("Shot", 0.6, -3.3, Json{{"restitution", 0.7}, {"velocity", Json{{"x", 6.0}, {"y", 0.0}}}});
    for (int i = 0; i < 15; ++i) REQUIRE(s.frame().has_value());
    Json shot = body("Shot");
    INFO(shot.dump() << " at " << pos("Shot").x);
    REQUIRE(bounces("/Shot") >= 1);
    REQUIRE(shot["velocity"]["x"].get<double>() == Catch::Approx(-4.2).margin(0.05));
    REQUIRE(pos("Shot").x < 1.5f);
    // A bounce off another body: a ball dropped onto a crate that collides with bodies.
    REQUIRE(s.command("world.spawn", Json{{"name", "Crate"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -0.3}, {"y", -3.2}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Drop"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -0.3}, {"y", -1.0}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.2}, {"y", 0.2}}}, {"restitution", 0.6}}}}}}).has_value());
    int body_bounces = 0;
    for (int i = 0; i < 60; ++i) {
        REQUIRE(s.frame().has_value());
        for (const Json& ev : s.command("events.since", Json{{"seq", 0}, {"type", "body2d.bounced"}, {"limit", 400}}).value()["events"]) if (ev["data"]["path"] == "/Drop" && ev["data"]["side"] == "body") body_bounces = std::max(body_bounces, 1);
    }
    REQUIRE(body_bounces == 1);
    REQUIRE(s.command("physics.stats", Json::object()).value()["tiles"].contains("bounces"));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("physics exceptions are commands and the pellet with ccd is held by the pane", "[runtime][physics][ccd]") {
    app::Options o;
    o.project_dir = root() / "samples" / "physics";
    o.bundle = root() / "build" / "ts" / "physics.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json ig = s.command("physics.ignore", Json{{"a", "Link1"}, {"b", "Link2"}}).value();
    REQUIRE(ig["ignored"] == true);
    REQUIRE(ig["exceptions"] == 1);
    Json list = s.command("physics.ignored", Json::object()).value();
    REQUIRE(list.size() == 1);
    REQUIRE(list[0]["a"] == "/Link1");
    REQUIRE(s.command("physics.stats", Json::object()).value()["exceptions"] == 1);
    REQUIRE(s.command("physics.ignore", Json{{"a", "Link1"}, {"b", "Link2"}, {"ignore", false}}).value()["exceptions"] == 0);
    REQUIRE(s.command("physics.ignore", Json{{"a", "Link1"}, {"b", "Link1"}}).error().code == "bad_args");
    REQUIRE(s.command("physics.ignore", Json{{"a", "Link1"}, {"b", "Nobody"}}).error().code == "no_such_entity");
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json st = s.command("state", Json::object()).value()["state"];
    INFO(st.dump());
    REQUIRE(st["pelletX"].get<double>() < 9.0);
    REQUIRE(st["pelletX"].get<double>() > 8.5);
    REQUIRE(st["dudCrossed"] == true);
    REQUIRE(st["ccdHits"] == 2);   // the pellet at the pane, and the clash pair meeting
    REQUIRE(st["ccdDynamic"] == 1);
    REQUIRE(st["clashGap"].get<double>() < 0.2);
    REQUIRE(st["clashGap"].get<double>() > 0.05);
    Json hits = s.command("events.since", Json{{"seq", 0}, {"type", "physics.ccd"}, {"limit", 10}}).value()["events"];
    REQUIRE(hits.size() == 2);
    int pane = 0, pair = 0;
    for (const Json& h : hits) {
        if (h["data"]["other"] == "/Pane") ++pane;
        if (h["data"]["dynamic"] == true) ++pair;
    }
    REQUIRE(pane == 1);
    REQUIRE(pair == 1);
    // The sweep query through the command: a sphere cast along the pellet's lane meets the pane.
    Json sw = s.command("physics.sweep", Json{{"origin", {{"x", 5}, {"y", 1.2}, {"z", 5}}}, {"direction", {{"x", 1}, {"y", 0}, {"z", 0}}}, {"radius", 0.1}}).value();
    INFO(sw.dump());
    REQUIRE(sw["path"].get<std::string>().ends_with("Pane"));
    REQUIRE(sw["distance"].get<double>() == Catch::Approx(3.89).margin(0.01));
    REQUIRE(sw["normal"]["x"].get<double>() == Catch::Approx(-1.0));
    REQUIRE(sw["radius"].get<double>() == Catch::Approx(0.1));
    REQUIRE(s.command("physics.sweep", Json{{"origin", {{"x", 5}, {"y", 4}, {"z", 5}}}, {"direction", {{"x", 1}, {"y", 0}, {"z", 0}}}, {"radius", 0.1}, {"max_distance", 3.0}}).value().is_null());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("2D bodies stack on each other, push each other by mass and stop at walls", "[runtime][body2d][bodies2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    auto spawn = [&](const char* name, double x, double y, Json body) {
        Json b = Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}};
        for (auto& [k, v] : body.items()) b[k] = v;
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", 0}}}}}, {"Body2D", b}}}}).has_value());
    };
    auto body = [&](const char* path) { return s.command("world.get", Json{{"entity", path}, {"component", "Body2D"}}).value(); };
    auto pos = [&](const char* path) { Json t = s.command("world.get", Json{{"entity", path}, {"component", "Transform"}}).value(); return Vec3{t["position"]["x"].get<float>(), t["position"]["y"].get<float>(), 0}; };
    auto push = [&](const char* path, double vx) { REQUIRE(s.command("world.set", Json{{"entity", path}, {"component", "Body2D"}, {"value", Json{{"velocity", Json{{"x", vx}}}}}}).has_value()); };
    // A crate dropped onto another stands on it, once, and rides it when it moves.
    spawn("Lower", 1.5, -3.2, Json::object());
    spawn("Upper", 1.5, -1.0, Json::object());
    // A ghost falls through the body under it.
    spawn("Base", -1.0, -3.2, Json::object());
    spawn("Ghost", -1.0, -1.0, Json{{"collide_bodies", false}});
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    INFO(body("Upper").dump() << " at " << pos("Upper").x << "," << pos("Upper").y);
    REQUIRE(pos("Upper").y == Catch::Approx(-2.6f).margin(0.02f));  // on the lower crate's top (-2.9) plus its half height
    REQUIRE(body("Upper")["grounded"] == true);
    REQUIRE(body("Upper")["riding"].get<std::uint64_t>() == s.command("world.find", Json{{"path", "Lower"}}).value().get<std::uint64_t>());
    REQUIRE(pos("Lower").y == Catch::Approx(-3.2f).margin(0.02f));
    REQUIRE(pos("Ghost").y == Catch::Approx(-3.2f).margin(0.02f));  // through the base, on the ground
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["body2d.landed"].get<int>() == 5);                  // the player's and the four crates' landings, once each
    Json st = s.command("physics.stats", Json::object()).value()["tiles"];
    INFO(st.dump());
    REQUIRE(st["stacked"].get<int>() == 1);
    REQUIRE(st["pairs"].get<int>() >= 1);
    for (int i = 0; i < 30; ++i) {
        push("Lower", 2.0);
        REQUIRE(s.frame().has_value());
    }
    INFO("lower " << pos("Lower").x << " upper " << pos("Upper").x);
    REQUIRE(pos("Lower").x > 2.3f);
    REQUIRE(std::fabs(pos("Upper").x - pos("Lower").x) < 0.08f);     // carried along
    push("Lower", 0.0);
    REQUIRE(hist["body2d.landed"].get<int>() == s.command("events.histogram", Json::object()).value()["body2d.landed"].get<int>());  // no landings while riding
    // A light pusher moves a light crate ahead of it, barely moves a heavy one, and stops when the crate is against a wall.
    spawn("Crate", -4.0, -3.2, Json::object());
    spawn("Pusher", -5.0, -3.2, Json::object());
    spawn("Heavy", 5.0, -3.2, Json{{"mass", 100.0}, {"friction", 6.0}});   // on a floor with friction: the light pusher's ticks of impulse are eaten
    spawn("Pusher2", 4.0, -3.2, Json::object());
    // (On the flat ground left of the start: the hill and the ledge are to the right, the lift's rail further left.)
    REQUIRE(s.command("world.spawn", Json{{"name", "Wall"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -6.0}, {"y", -3.0}, {"z", 0}}}}}, {"Body2D", Json{{"kinematic", true}, {"size", Json{{"x", 0.2}, {"y", 0.5}}}}}}}}).has_value());
    spawn("Cornered", -6.55, -3.2, Json::object());                  // its right side a twentieth from the wall's face at x -6.2
    spawn("Pusher3", -7.4, -3.2, Json::object());
    REQUIRE(s.frame().has_value());
    for (int i = 0; i < 60; ++i) {
        push("Pusher", 3.0);
        push("Pusher2", 3.0);
        push("Pusher3", 3.0);
        REQUIRE(s.frame().has_value());
    }
    INFO("crate " << pos("Crate").x << " pusher " << pos("Pusher").x << " heavy " << pos("Heavy").x << " pusher2 " << pos("Pusher2").x << " cornered " << pos("Cornered").x << " pusher3 " << pos("Pusher3").x);
    REQUIRE(pos("Crate").x > -2.8f);                                  // taken along: the pusher's speed passes to it within a few ticks
    REQUIRE(pos("Pusher").x < pos("Crate").x - 0.55f);               // behind it, touching
    REQUIRE(pos("Heavy").x < 5.15f);                                  // a hundred times heavier: barely moved
    REQUIRE(pos("Pusher2").x == Catch::Approx(pos("Heavy").x - 0.6f).margin(0.02f));
    REQUIRE(pos("Cornered").x < -6.499f);                             // held by the wall (its face at -6.2, the crate's half width 0.3)
    REQUIRE(pos("Cornered").x > -6.56f);
    REQUIRE(pos("Pusher3").x == Catch::Approx(pos("Cornered").x - 0.6f).margin(0.02f));
    REQUIRE(body("Cornered")["on_wall"].get<int>() == 1);
    REQUIRE(s.command("physics.stats", Json::object()).value()["tiles"]["pushed"].get<int>() >= 2);
    REQUIRE(s.finish().has_value());
}


TEST_CASE("2D bodies exchange momentum sideways: inelastic, elastic and by mass", "[runtime][body2d][momentum2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // Lanes in the air (no gravity) over the flat ground left of the start, one pair per lane, the
    // mover a unit behind the target and heading right.
    auto spawn = [&](const char* name, double x, double y, Json body) {
        Json b = Json{{"size", Json{{"x", 0.3}, {"y", 0.3}}}, {"gravity", 0.0}};
        for (auto& [k, v] : body.items()) b[k] = v;
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", 0}}}}}, {"Body2D", b}}}}).has_value());
    };
    auto vx = [&](const char* path) { return s.command("world.get", Json{{"entity", path}, {"component", "Body2D"}}).value()["velocity"]["x"].get<double>(); };
    auto px = [&](const char* path) { return s.command("world.get", Json{{"entity", path}, {"component", "Transform"}}).value()["position"]["x"].get<double>(); };
    spawn("Cue", -6.5, -1.0, Json{{"velocity", Json{{"x", 4.0}, {"y", 0.0}}}});
    spawn("Target", -5.5, -1.0, Json::object());
    spawn("Cue2", -6.5, 0.0, Json{{"velocity", Json{{"x", 4.0}, {"y", 0.0}}}, {"restitution", 1.0}});
    spawn("Target2", -5.5, 0.0, Json::object());
    spawn("Train", -6.5, 1.0, Json{{"velocity", Json{{"x", 2.0}, {"y", 0.0}}}, {"mass", 10.0}});
    spawn("Pebble2", -5.5, 1.0, Json::object());
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    INFO("cue " << vx("Cue") << " target " << vx("Target") << " cue2 " << vx("Cue2") << " target2 " << vx("Target2") << " train " << vx("Train") << " pebble " << vx("Pebble2"));
    // Equal masses, no restitution: both leave at the mean, momentum kept.
    REQUIRE(vx("Cue") == Catch::Approx(2.0).margin(0.05));
    REQUIRE(vx("Target") == Catch::Approx(2.0).margin(0.05));
    REQUIRE(px("Target") > px("Cue") + 0.55);
    // Restitution 1: the cue stops and the target leaves at the cue's speed, and the cue reports its bounce.
    REQUIRE(vx("Cue2") == Catch::Approx(0.0).margin(0.05));
    REQUIRE(vx("Target2") == Catch::Approx(4.0).margin(0.05));
    int cue_bounces = 0;
    for (const Json& ev : s.command("events.since", Json{{"seq", 0}, {"type", "body2d.bounced"}, {"limit", 400}}).value()["events"]) if (ev["data"]["path"] == "/Cue2" && ev["data"]["side"] == "body") cue_bounces++;
    REQUIRE(cue_bounces == 1);
    // Ten times the mass: the train keeps most of its speed and the pebble is taken along at it.
    REQUIRE(vx("Train") == Catch::Approx(2.0 * 10.0 / 11.0).margin(0.05));
    REQUIRE(vx("Pebble2") == Catch::Approx(2.0 * 10.0 / 11.0).margin(0.05));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("the control server answers over HTTP, runs past 3600 frames and fails requests left at shutdown", "[runtime][serve]") {
    auto o = hello_options(-1);
    o.paused = true;
    o.serve = 0;
    app::Session s(o);
    REQUIRE(s.start());
    auto server = std::make_unique<app::ControlServer>(s, 0);
    REQUIRE(server->start());
    REQUIRE(server->port() > 0);
    httplib::Client client("127.0.0.1", server->port());
    client.set_read_timeout(120, 0);
    auto post = [&](const char* body) {
        auto res = client.Post("/rpc", body, "application/json");
        return res ? Json::parse(res->body, nullptr, false) : Json{{"transport", "no reply"}};
    };
    // A request runs on the main thread when it pumps. 3601 ticks pass the cap that ends a
    // headless run without a controller: a served run is its controller's to end.
    Json reply;
    std::thread t([&] { reply = post(R"({"id":1,"method":"step","params":{"ticks":3601}})"); });
    while (server->describe()["handled"].get<std::uint64_t>() < 1) server->pump(50);
    t.join();
    INFO(reply.dump());
    REQUIRE(reply["result"]["frames"] == 3601);
    REQUIRE_FALSE(s.finished());
    // A request queued after the last pump: shutting the server down answers it with an error
    // instead of hanging the client (and the shutdown, which joins the handler threads).
    Json late;
    std::thread t2([&] { late = post(R"({"id":2,"method":"state","params":{}})"); });
    for (int i = 0; i < 2000 && server->describe()["queued"].get<std::size_t>() < 1; ++i) std::this_thread::sleep_for(std::chrono::milliseconds(5));
    REQUIRE(server->describe()["queued"] == 1);
    server.reset();
    t2.join();
    INFO(late.dump());
    REQUIRE(late["error"]["data"]["code"] == "shutting_down");
    REQUIRE(s.command("quit", Json::object()).has_value());
    REQUIRE(s.finished());
    REQUIRE(s.finish());

    // Without a controller the cap still ends a headless run, so no run hangs forever.
    auto o2 = hello_options(-1);
    app::Session s2(o2);
    REQUIRE(s2.start());
    REQUIRE(s2.command("step", Json{{"ticks", 3599}}, "test").has_value());
    REQUIRE_FALSE(s2.finished());
    REQUIRE(s2.command("step", Json{{"ticks", 1}}, "test").has_value());
    REQUIRE(s2.finished());
    REQUIRE(s2.finish());
}

TEST_CASE("render.compare records a reference frame, then tells a matching frame from a changed one", "[runtime][compare]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    const std::filesystem::path scratch = o.project_dir / ".pocket" / "compare";
    std::filesystem::remove_all(scratch);
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // The first call writes the reference; the same frame then matches it exactly.
    Json first = s.command("render.compare", Json{{"path", ".pocket/compare/ref.png"}}).value();
    INFO(first.dump());
    REQUIRE(first["written"] == true);
    REQUIRE(first["match"] == true);
    REQUIRE(std::filesystem::exists(scratch / "ref.png"));
    Json same = s.command("render.compare", Json{{"path", ".pocket/compare/ref.png"}}).value();
    INFO(same.dump());
    REQUIRE(same["written"] == false);
    REQUIRE(same["match"] == true);
    REQUIRE(same["differing"] == 0);
    REQUIRE(same["width"] == 320);
    // A big sprite over the middle of the frame: many pixels differ, in a rectangle the answer names.
    Json block;
    block["name"] = "Block";
    block["components"]["Transform"]["position"] = Json{{"x", 0.0}, {"y", 0.0}, {"z", 0.5}};
    block["components"]["Sprite"] = Json{{"size", {{"x", 6.0}, {"y", 3.0}}}, {"color", {{"r", 1.0}, {"g", 0.0}, {"b", 1.0}, {"a", 1.0}}}, {"layer", 50}};
    REQUIRE(s.command("world.spawn", block).has_value());
    REQUIRE(s.frame().has_value());
    Json changed = s.command("render.compare", Json{{"path", ".pocket/compare/ref.png"}, {"diff", ".pocket/compare/diff.png"}}).value();
    INFO(changed.dump());
    REQUIRE(changed["match"] == false);
    REQUIRE(changed["fraction"].get<double>() > 0.05);
    REQUIRE(changed["fraction"].get<double>() < 0.9);
    REQUIRE(changed["bounds"]["x1"].get<int>() > changed["bounds"]["x0"].get<int>());
    REQUIRE(changed["bounds"]["y1"].get<int>() > changed["bounds"]["y0"].get<int>());
    REQUIRE(std::filesystem::exists(scratch / "diff.png"));
    // A tolerance above the change accepts it; update rewrites the reference so it matches again.
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/compare/ref.png"}, {"tolerance", 0.95}}).value()["match"] == true);
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/compare/ref.png"}, {"update", true}}).value()["written"] == true);
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/compare/ref.png"}}).value()["match"] == true);
    // A reference of another size is a mismatch by size, and a path outside the project is refused.
    REQUIRE(s.command("render.compare", Json{{"path", "assets/coin.png"}}).value()["reason"] == "size");
    REQUIRE(s.command("render.compare", Json{{"path", "../outside.png"}}).error().code == "forbidden");
    std::filesystem::remove_all(scratch);
}

TEST_CASE("an interface box draws a project image, fitted, cropped or stretched, and a missing one draws nothing", "[runtime][ui][image]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    const Json plain = s.command("capture", Json::object()).value()["center_pixel"];
    // A box over the middle of the window showing the top of the sky column (its darkest rows).
    Json ops = Json::array();
    ops.push_back(Json::array({"create", 50, "box"}));
    ops.push_back(Json::array({"set", 50, Json{{"position", "absolute"}, {"left", 140}, {"top", 70}, {"width", 40}, {"height", 40}, {"image", "assets/sky.png"}, {"fit", "fill"}, {"uv", Json::array({0.0, 0.0, 1.0, 0.05})}, {"name", "picture"}}}));
    ops.push_back(Json::array({"append", 1, 50}));
    REQUIRE(s.command("ui.apply", Json{{"ops", ops}}).has_value());
    REQUIRE(s.frame().has_value());
    Json c = s.command("capture", Json::object()).value();
    INFO(c["center_pixel"].dump() << " over " << plain.dump());
    const auto px = [&](const Json& p, int i) { return p[i].get<int>(); };
    REQUIRE(px(c["center_pixel"], 0) == Catch::Approx(58).margin(14));    // the sky's top color (58, 95, 154)
    REQUIRE(px(c["center_pixel"], 1) == Catch::Approx(95).margin(14));
    REQUIRE(px(c["center_pixel"], 2) == Catch::Approx(154).margin(14));
    Json d = s.command("ui.describe", Json{{"id", 50}}).value();
    REQUIRE(d["image"] == "assets/sky.png");
    REQUIRE(d["fit"] == "fill");
    // Contain keeps the column's proportions: a 32x8 strip in a 40x40 box is 40x10 across the middle, the rest of the box empty.
    REQUIRE(s.command("ui.apply", Json{{"ops", Json::array({Json::array({"set", 50, Json{{"fit", "contain"}}})})}}).has_value());
    REQUIRE(s.frame().has_value());
    c = s.command("capture", Json::object()).value();
    REQUIRE(px(c["center_pixel"], 2) == Catch::Approx(154).margin(14));   // the strip runs through the center
    // A picture that does not exist draws nothing: the scene shows through again.
    REQUIRE(s.command("ui.apply", Json{{"ops", Json::array({Json::array({"set", 50, Json{{"image", "assets/nothing-here.png"}}})})}}).has_value());
    REQUIRE(s.frame().has_value());
    c = s.command("capture", Json::object()).value();
    INFO(c["center_pixel"].dump() << " vs plain " << plain.dump());
    for (int i = 0; i < 3; ++i) REQUIRE(px(c["center_pixel"], i) == Catch::Approx(px(plain, i)).margin(2));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a texture drawn small is sampled from its mip chain, unless it is pixel art", "[runtime][render][mips]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    // A 64 by 64 checkerboard of 2-pixel cells, written as a BMP beside the sample's assets and removed after.
    const std::filesystem::path file = o.project_dir / "assets" / "checker-test.bmp";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    {
        const int w = 64, h = 64;
        std::vector<std::uint8_t> bmp(54 + static_cast<std::size_t>(w) * h * 3, 0);
        auto put32 = [&](std::size_t at, std::uint32_t v) { for (int i = 0; i < 4; ++i) bmp[at + static_cast<std::size_t>(i)] = static_cast<std::uint8_t>(v >> (8 * i)); };
        bmp[0] = 'B';
        bmp[1] = 'M';
        put32(2, static_cast<std::uint32_t>(bmp.size()));
        put32(10, 54);
        put32(14, 40);
        put32(18, w);
        put32(22, h);
        bmp[26] = 1;
        bmp[28] = 24;
        put32(34, static_cast<std::uint32_t>(w * h * 3));
        for (int y = 0; y < h; ++y) {
            for (int x = 0; x < w; ++x) {
                const std::uint8_t v = ((x / 2 + y / 2) % 2) ? 255 : 0;
                const std::size_t at = 54 + (static_cast<std::size_t>(y) * w + static_cast<std::size_t>(x)) * 3;
                bmp[at] = bmp[at + 1] = bmp[at + 2] = v;
            }
        }
        std::ofstream out(file, std::ios::binary);
        out.write(reinterpret_cast<const char*>(bmp.data()), static_cast<std::streamsize>(bmp.size()));
    }
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // At the frame's center, half a unit wide: nine pixels for 64 texels, the cells far below pixel size.
    Json sp;
    sp["name"] = "Checker";
    sp["components"]["Transform"] = Json{{"position", {{"x", 0.0}, {"y", 0.0}, {"z", 1.0}}}};
    sp["components"]["Sprite"] = Json{{"texture", "assets/checker-test.bmp"}, {"size", {{"x", 0.5}, {"y", 0.5}}}, {"layer", 9}};
    REQUIRE(s.command("world.spawn", sp).has_value());
    REQUIRE(s.frame().has_value());
    const auto px = [](const Json& p, int i) { return p[i].get<int>(); };
    Json smooth = s.command("capture", Json::object()).value();
    INFO("smooth " << smooth["center_pixel"].dump() << " stats " << smooth["render"].dump());
    // Filtered through the chain: an even gray, neither cell's color.
    for (int i = 0; i < 3; ++i) {
        REQUIRE(px(smooth["center_pixel"], i) > 80);
        REQUIRE(px(smooth["center_pixel"], i) < 176);
    }
    // Nearest sampling stays on the full image: the pixel is one cell or the other.
    REQUIRE(s.command("world.set", Json{{"entity", "Checker"}, {"component", "Sprite"}, {"value", Json{{"filter", "nearest"}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json crisp = s.command("capture", Json::object()).value();
    INFO("crisp " << crisp["center_pixel"].dump());
    const int v = px(crisp["center_pixel"], 0);
    const bool one_cell = v < 40 || v > 215;
    REQUIRE(one_cell);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a textured mesh with a cutoff is cut out where its picture is transparent", "[runtime][render][cutout]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    // A 64 by 64 green picture with a transparent 32 by 32 hole in the middle, as a TGA beside the sample's assets, removed after.
    const std::filesystem::path file = o.project_dir / "assets" / "hole-test.tga";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    {
        const int w = 64, h = 64;
        std::vector<std::uint8_t> tga(18 + static_cast<std::size_t>(w) * h * 4, 0);
        tga[2] = 2;   // uncompressed true color
        tga[12] = w;
        tga[14] = h;
        tga[16] = 32;
        tga[17] = 0x28;   // 8 alpha bits, rows from the top
        for (int y = 0; y < h; ++y) {
            for (int x = 0; x < w; ++x) {
                const bool hole = x >= 16 && x < 48 && y >= 16 && y < 48;
                std::uint8_t* px = &tga[18 + (static_cast<std::size_t>(y) * w + static_cast<std::size_t>(x)) * 4];
                px[0] = 0;
                px[1] = 255;
                px[2] = 0;
                px[3] = hole ? 0 : 255;
            }
        }
        std::ofstream out(file, std::ios::binary);
        out.write(reinterpret_cast<const char*>(tga.data()), static_cast<std::streamsize>(tga.size()));
    }
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    const auto px = [](const Json& p, int i) { return p[i].get<int>(); };
    const Json plain = s.command("capture", Json::object()).value()["center_pixel"];
    // A quad over the frame's center, two units wide, glowing green through the picture and lit by nothing (black base color).
    Json q;
    q["name"] = "Grate";
    q["components"]["Transform"] = Json{{"position", {{"x", 0.0}, {"y", 0.0}, {"z", 1.0}}}, {"scale", {{"x", 2.0}, {"y", 2.0}, {"z", 1.0}}}};
    q["components"]["MeshRenderer"] = Json{{"mesh", "quad"}, {"texture", "assets/hole-test.tga"}, {"color", {{"r", 0.0}, {"g", 0.0}, {"b", 0.0}, {"a", 1.0}}}, {"emissive", {{"r", 0.0}, {"g", 1.0}, {"b", 0.0}, {"a", 1.0}}}};
    REQUIRE(s.command("world.spawn", q).has_value());
    REQUIRE(s.frame().has_value());
    Json solid = s.command("capture", Json::object()).value();
    INFO("plain " << plain.dump() << " solid " << solid["center_pixel"].dump());
    // Without a cutoff the hole is drawn like the rest: green, and the quad is picked there.
    REQUIRE(px(solid["center_pixel"], 1) > 200);
    REQUIRE(px(solid["center_pixel"], 0) < 40);
    REQUIRE(s.command("render.pick", Json{{"x", 160}, {"y", 90}}).value()["name"] == "Grate");
    // With one, the hole shows what is behind and picks nothing of the quad.
    REQUIRE(s.command("world.set", Json{{"entity", "Grate"}, {"component", "MeshRenderer"}, {"value", Json{{"cutoff", 0.5}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json cut = s.command("capture", Json::object()).value();
    INFO("cut " << cut["center_pixel"].dump());
    for (int i = 0; i < 3; ++i) REQUIRE(px(cut["center_pixel"], i) == Catch::Approx(px(plain, i)).margin(2));
    REQUIRE(s.command("render.pick", Json{{"x", 160}, {"y", 90}}).value()["name"] != "Grate");
    REQUIRE(s.command("world.get", Json{{"entity", "Grate"}, {"component", "MeshRenderer"}}).value()["cutoff"].get<double>() == Catch::Approx(0.5));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a mesh with alpha under one is drawn translucent over what is behind it, after the opaque meshes", "[runtime][render][translucent]") {
    app::Session s(hello_options(200));
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    const Json plain = s.command("capture", Json::object()).value()["center_pixel"];
    const auto px = [](const Json& p, int i) { return p[i].get<int>(); };
    // A red pane between the camera (0, 2.5, 7, looking at the ball) and the scene, over the middle of the frame.
    Json pane;
    pane["name"] = "Pane";
    pane["components"]["Transform"] = Json{{"position", {{"x", 0.0}, {"y", 1.5}, {"z", 3.0}}}, {"scale", {{"x", 3.0}, {"y", 3.0}, {"z", 0.05}}}};
    pane["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", {{"r", 1.0}, {"g", 0.0}, {"b", 0.0}, {"a", 0.5}}}};
    REQUIRE(s.command("world.spawn", pane).has_value());
    REQUIRE(s.frame().has_value());
    Json half = s.command("capture", Json::object()).value();
    INFO("plain " << plain.dump() << " half " << half["center_pixel"].dump() << " stats " << half["render"].dump());
    REQUIRE(half["render"]["translucent"] == 1);
    // Half red over the scene: redder than the scene, but the scene still shows (green and blue not gone).
    REQUIRE(px(half["center_pixel"], 0) > px(plain, 0) + 20);
    REQUIRE(px(half["center_pixel"], 1) > 8);
    REQUIRE(px(half["center_pixel"], 2) > 8);
    // Opaque, the pane hides the scene: red, nothing of the scene's green and blue.
    REQUIRE(s.command("world.set", Json{{"entity", "Pane"}, {"component", "MeshRenderer"}, {"value", Json{{"color", {{"r", 1.0}, {"g", 0.0}, {"b", 0.0}, {"a", 1.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json opaque = s.command("capture", Json::object()).value();
    INFO("opaque " << opaque["center_pixel"].dump());
    REQUIRE(opaque["render"]["translucent"] == 0);
    REQUIRE(px(opaque["center_pixel"], 0) > px(half["center_pixel"], 0));
    REQUIRE(px(opaque["center_pixel"], 1) < 8);
    REQUIRE(px(opaque["center_pixel"], 2) < 8);
    // Behind an opaque wall, a translucent mesh does not show: the depth test still applies to it.
    REQUIRE(s.command("world.set", Json{{"entity", "Pane"}, {"component", "MeshRenderer"}, {"value", Json{{"color", {{"r", 1.0}, {"g", 0.0}, {"b", 0.0}, {"a", 0.5}}}}}}).has_value());
    Json wall;
    wall["name"] = "Wall";
    wall["components"]["Transform"] = Json{{"position", {{"x", 0.0}, {"y", 1.5}, {"z", 4.0}}}, {"scale", {{"x", 3.0}, {"y", 3.0}, {"z", 0.05}}}};
    wall["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", {{"r", 0.0}, {"g", 0.0}, {"b", 1.0}, {"a", 1.0}}}};
    REQUIRE(s.command("world.spawn", wall).has_value());
    REQUIRE(s.frame().has_value());
    Json walled = s.command("capture", Json::object()).value();
    INFO("walled " << walled["center_pixel"].dump());
    REQUIRE(px(walled["center_pixel"], 2) > px(walled["center_pixel"], 0) + 20);   // the blue wall, no red over it
    // The pane is still picked through its id.
    Json pick = s.command("render.pick", Json{{"x", 64}, {"y", 36}}).value();   // the frame is 128 by 72
    REQUIRE(pick["name"] == "Wall");
    REQUIRE(s.finish().has_value());
}

TEST_CASE("colliding particles land on the physics bodies and the rest fall through", "[runtime][particles][collide]") {
    app::Options o;
    o.project_dir = root() / "samples" / "physics";
    o.bundle = root() / "build" / "ts" / "physics.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // The ground's top under (2, 3, 2): what the sparks should come to rest on.
    Json ray = s.command("physics.raycast", Json{{"origin", {{"x", 2.0}, {"y", 3.0}, {"z", 2.0}}}, {"direction", {{"x", 0.0}, {"y", -1.0}, {"z", 0.0}}}, {"max_distance", 10.0}}).value();
    INFO(ray.dump());
    REQUIRE(ray["path"] == "/Ground");
    const double top = ray["point"]["y"].get<double>();
    auto emitter = [&](const char* name, bool collide) {
        Json fields = Json{{"emitting", false}, {"gravity", {{"x", 0}, {"y", -10}, {"z", 0}}}, {"lifetime", {{"x", 8}, {"y", 8}}}, {"speed", {{"x", 0.5}, {"y", 0.5}}}, {"direction", {{"x", 0}, {"y", -1}, {"z", 0}}}, {"spread", 10}, {"bounce", 0.2}, {"collide", collide}};
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", {{"x", 2.0}, {"y", 3.0}, {"z", 2.0}}}}}, {"ParticleEmitter", fields}}}}).has_value());
        REQUIRE(s.frame().has_value());
        REQUIRE(s.command("particles.burst", Json{{"entity", name}, {"count", 30}}).has_value());
    };
    emitter("Dust", true);
    emitter("Ghost", false);
    for (int i = 0; i < 120; ++i) REQUIRE(s.frame().has_value());   // two seconds: a fall of three units takes under one
    Json dust = s.command("particles.list", Json{{"entity", "Dust"}, {"limit", 100}}).value()["particles"];
    Json ghost = s.command("particles.list", Json{{"entity", "Ghost"}, {"limit", 100}}).value()["particles"];
    REQUIRE(dust.size() == 30);
    REQUIRE(ghost.size() == 30);
    int resting = 0;
    for (const Json& p : dust) {
        INFO(p.dump());
        REQUIRE(p["position"]["y"].get<double>() >= top - 0.02);   // never through the ground
        REQUIRE(p["position"]["y"].get<double>() <= top + 0.3);
        resting += p["resting"] == true;
    }
    REQUIRE(resting == 30);
    for (const Json& p : ghost) REQUIRE(p["position"]["y"].get<double>() < top - 2.0);   // through it, still falling
    Json stats = s.command("particles.stats", Json::object()).value();
    for (const Json& pl : stats["pools"]) {
        if (pl["entity"] == s.command("world.find", Json{{"path", "Dust"}}).value()) REQUIRE(pl["landed"] == 30);
    }
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a sliced image keeps its corners and stretches its middle", "[runtime][ui][image][slice]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // The first two tiles of the sheet (grass, dirt: 32 by 16 px) over a 200 by 16 box across the
    // middle of the window, sliced 4 px on the left and 20 on the right: the middle 8 px of grass
    // stretch over 176 points, so the window's center shows grass; the dirt tile filled instead
    // shows dirt there.
    Json ops = Json::array();
    ops.push_back(Json::array({"create", 60, "box"}));
    ops.push_back(Json::array({"set", 60, Json{{"position", "absolute"}, {"left", 60}, {"top", 82}, {"width", 200}, {"height", 16}, {"image", "assets/tiles.png"}, {"uv", Json::array({0.0, 0.0, 2.0 / 7.0, 1.0})}, {"slice", Json::array({4, 0, 20, 0})}, {"name", "frame"}}}));
    ops.push_back(Json::array({"append", 1, 60}));
    REQUIRE(s.command("ui.apply", Json{{"ops", ops}}).has_value());
    REQUIRE(s.frame().has_value());
    Json c = s.command("capture", Json::object()).value();
    const auto px = [&](const Json& p, int i) { return p[i].get<int>(); };
    INFO("sliced " << c["center_pixel"].dump());
    REQUIRE(px(c["center_pixel"], 0) < 110);   // grass: (70..90, 160..180, 70)
    REQUIRE(px(c["center_pixel"], 1) > 140);
    Json d = s.command("ui.describe", Json{{"id", 60}}).value();
    REQUIRE(d["slice"] == Json::array({4, 0, 20, 0}));
    REQUIRE(s.command("ui.apply", Json{{"ops", Json::array({Json::array({"set", 60, Json{{"slice", 0}, {"fit", "fill"}, {"uv", Json::array({1.0 / 7.0, 0.0, 2.0 / 7.0, 1.0})}}})})}}).has_value());
    REQUIRE(s.frame().has_value());
    c = s.command("capture", Json::object()).value();
    INFO("filled " << c["center_pixel"].dump());
    REQUIRE(px(c["center_pixel"], 0) > 115);   // dirt: (130..145, 90..105, 50)
    REQUIRE(px(c["center_pixel"], 1) < 120);
    REQUIRE(s.finish().has_value());
}
