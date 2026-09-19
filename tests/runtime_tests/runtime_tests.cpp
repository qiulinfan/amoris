#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>
#include <pocket/core/core.hpp>

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
    REQUIRE(clips["clips"].size() == 2);
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
    REQUIRE(rs["skinned"].get<int>() == 1);
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
    REQUIRE(joints.size() == 4);  // three links and the lantern's rope
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
    REQUIRE(s.command("physics.stats", Json::object()).value()["joints"].get<int>() == 4);
    // The kick at tick 240 snaps the rope: the joint is gone, the event says so, the lantern falls.
    for (int i = 0; i < 120; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("world.has", Json{{"entity", "/Lantern"}, {"component", "Joint"}}).value() == false);
    Json hist = s.command("events.histogram", Json::object()).value();
    REQUIRE(hist["joint.broken"].get<int>() == 1);
    REQUIRE(s.command("physics.joints", Json::object()).value().size() == 3);
    REQUIRE(length(position("/Lantern") - position("/Beam")) > 2.0f);  // flew off, no longer tethered at 1.5
    Json state = s.command("state", Json::object()).value()["state"];
    REQUIRE(state["ropeIntact"] == false);
    REQUIRE(state["joints"].get<int>() == 3);
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
    REQUIRE(stats["sprites"].get<int>() == 7);             // player + 6 coins; the ground is a tile map
    REQUIRE(stats["tile_layers"].get<int>() == 2);         // ground and deco layers of level.tmj
    REQUIRE(stats["draw_calls"].get<int>() <= 6);           // runs per texture, split by layer
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
