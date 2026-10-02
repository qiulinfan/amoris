#include <pocket/app/runtime.hpp>
#include <pocket/app/server.hpp>
#include <pocket/app/session.hpp>
#include <pocket/app/websocket.hpp>
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
#include <fstream>
#include <future>
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

TEST_CASE("Math.random follows the run seed, on a stream of its own", "[runtime][random]") {
    auto draws = [](std::uint64_t seed, const char* source) {
        auto o = hello_options(10);
        o.seed = seed;
        app::Session s(o);
        REQUIRE(s.start().has_value());
        Json v = s.command("script.eval", Json{{"source", source}}).value();
        REQUIRE(s.finish().has_value());
        return v;
    };
    const char* three = "[Math.random(), Math.random(), Math.random()]";
    const Json a = draws(7, three);
    INFO(a.dump());
    // The same bits a browser or V8 computes from seed 7 (sfc32 on 32-bit integer operations).
    REQUIRE(a == Json::array({0.9593075499869883, 0.7410346050746739, 0.4709692746400833}));
    REQUIRE(draws(7, three) == a);
    REQUIRE(draws(8, three) != a);
    // random() is another stream: drawing from it leaves Math.random's where it was.
    REQUIRE(draws(7, "[Math.random(), __pocket.random(), Math.random()][2]") == a[1]);
    // A new episode starts the stream again.
    auto o = hello_options(10);
    o.seed = 7;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("script.eval", Json{{"source", "Math.random()"}}).value() == a[0]);
    REQUIRE(s.command("env.reset", Json{{"seed", 7}}).has_value());
    REQUIRE(s.command("script.eval", Json{{"source", "Math.random()"}}).value() == a[0]);
    // So does a restart of the scene and the scripts (an edit applied), for both streams.
    const Json native = s.command("script.eval", Json{{"source", "__pocket.random()"}}).value();
    REQUIRE(s.command("project.reload", Json::object()).has_value());
    REQUIRE(s.command("script.eval", Json{{"source", "Math.random()"}}).value() == a[0]);
    REQUIRE(s.command("script.eval", Json{{"source", "__pocket.random()"}}).value() == native);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a component write is checked field by field, takes value names and answers the value", "[runtime][world][strict]") {
    auto o = hello_options(10);
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Lamp"}, {"components", Json{{"Light", Json{{"kind", "point"}, {"intensity", 2}}}}}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Lamp"}, {"component", "Light"}}).value()["kind"] == 1);
    Json set = s.command("world.set", Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"kind", "spot"}}}}).value();
    INFO(set.dump());
    REQUIRE(set["ok"] == true);
    REQUIRE(set["value"]["kind"] == 2);
    REQUIRE(set["value"]["intensity"] == 2.0);   // the patch merged into what was there
    auto refused = [&](const Json& params, std::string_view needle) {
        auto r = s.command("world.set", params);
        REQUIRE_FALSE(r.has_value());
        INFO(r.error().message);
        REQUIRE(r.error().message.find(needle) != std::string::npos);
    };
    refused(Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"colour", Json{{"r", 1}}}}}}, "did you mean 'color'");
    refused(Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"kind", "spotlight"}}}}, "one of directional, point, spot");
    refused(Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"intensity", "bright"}}}}, "Light.intensity is a number");
    refused(Json{{"entity", "Lamp"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 1}, {"q", 2}}}}}}, "Transform.position is a vec3 with the parts xyz, not 'q'");
    refused(Json{{"entity", "Lamp"}, {"component", "Animator"}, {"value", Json{{"layers", Json::array({Json{{"clip", "walk"}, {"wieght", 1}}})}}}}, "Animator.layers[0] has no field 'wieght'; did you mean 'weight'?");
    REQUIRE(s.command("world.get", Json{{"entity", "Lamp"}, {"component", "Light"}}).value()["kind"] == 2);   // nothing was applied
    auto spawned = s.command("world.spawn", Json{{"name", "Bad"}, {"components", Json{{"Lihgt", Json::object()}}}});
    REQUIRE_FALSE(spawned.has_value());
    REQUIRE(spawned.error().message.find("did you mean 'Light'") != std::string::npos);
    REQUIRE_FALSE(s.command("world.find", Json{{"path", "Bad"}}).value().is_number());   // refused before anything was made
    // The schema shows the names; a scene may use them too.
    Json schema = s.command("world.schema", Json{{"component", "Light"}}).value();
    REQUIRE(schema["components"][0]["fields"][0]["names"] == Json::array({"directional", "point", "spot"}));
    Json scene = {{"format", "pocket-scene"}, {"version", 1}, {"entities", Json::array({Json{{"name", "Crate"}, {"components", Json{{"RigidBody", Json{{"kind", "kinematic"}}}}}}})}};
    REQUIRE(s.command("world.instantiate", Json{{"scene", scene}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Crate"}, {"component", "RigidBody"}}).value()["kind"] == 2);
    REQUIRE(s.finish().has_value());
}

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
#ifdef __APPLE__
    REQUIRE(rep["gpu"]["backend"] == "metal");
#else
    REQUIRE(rep["gpu"]["backend"] == "vulkan");
#endif
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

TEST_CASE("script.eval has the SDK's exports as names and answers an error with its message", "[runtime][eval]") {
    app::Session s(hello_options(-1));
    REQUIRE(s.start().has_value());
    const Json made = s.command("script.eval", Json{{"source", "world.spawn('Evaled')"}}).value();
    REQUIRE(made.is_number());
    REQUIRE(s.command("script.eval", Json{{"source", "pocket.world.find('Evaled')"}}).value() == made);
    const Json bad = s.command("script.eval", Json{{"source", "nothingHere.x"}}).value();
    INFO(bad.dump());
    REQUIRE(bad["error"].get<std::string>().find("Can't find variable: nothingHere") != std::string::npos);
    // An entity that a lookup did not find is said as such, not as a missing parameter.
    const Json lost = s.command("script.eval", Json{{"source", "world.get(world.find('NoSuchThing'), 'Transform')"}}).value();
    INFO(lost.dump());
    REQUIRE(lost["error"].get<std::string>().find("the entity is undefined") != std::string::npos);
    // A var stays for the next call, as in a console.
    REQUIRE(s.command("script.eval", Json{{"source", "var kept = 41; kept + 1"}}).value() == 42);
    REQUIRE(s.command("script.eval", Json{{"source", "kept"}}).value() == 41);
}

TEST_CASE("a reload after a script error drops the old handlers before the new bundle's join", "[runtime][reload]") {
    app::Session s(hello_options(-1));
    REQUIRE(s.start().has_value());
    auto handlers = [&] { return s.command("script.eval", Json{{"source", "globalThis.__pocket_registry.contexts.get('project').tick.length"}}).value().get<int>(); };
    const int ticks = handlers();
    REQUIRE(ticks > 0);
    // hello's tick throws once its ball is gone: a script error stops the simulation.
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    (void)s.command("step", Json{{"ticks", 2}});
    REQUIRE_FALSE(s.ok());
    const Json r = s.command("project.reload", Json::object()).value();
    INFO(r.dump());
    REQUIRE(r["ok"] == true);
    REQUIRE(handlers() == ticks);
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    REQUIRE(s.ok());
}

TEST_CASE("a script error names the file and line that were written, through the bundle's line map", "[runtime][sourcelines]") {
    auto o = hello_options(3);
    auto broken = root() / "build" / "test-out" / "mapped.js";
    // Lines 2 and 3 of the bundle came from lines 40 and 41 of scripts/fake.ts.
    REQUIRE(fs::write_text(broken, "// made by hand\nglobalThis.__pocket_dispatch = function(kind){\n if (kind === 'tick') throw new Error('boom'); };\n").has_value());
    const auto fake = std::filesystem::weakly_canonical(o.project_dir) / "scripts" / "fake.ts";
    REQUIRE(fs::write_text(std::filesystem::path(broken.string() + ".lines.json"), Json{{"modules", Json::array({Json{{"source", fake.string()}, {"first", 2}, {"lines", {40, 41}}}})}}.dump()).has_value());
    o.bundle = broken;
    o.project_config = broken.string() + ".project.json";
    auto r = app::run(o);
    REQUIRE(r.has_value());
    const Json& e = (*r)["errors"][0];
    INFO(e.dump());
    REQUIRE(e.value("detail", std::string()).find("scripts/fake.ts:41") != std::string::npos);
    REQUIRE(e.value("detail", std::string()).find("mapped.js:3") == std::string::npos);
}

TEST_CASE("script.profile breaks the script time down by handler, at the line it was registered, and by command", "[runtime][profile]") {
    app::Session s(hello_options(-1));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 20}}).has_value());
    Json p = s.command("script.profile", Json{{"reset", true}}).value();
    INFO(p.dump());
    REQUIRE(p["ticks"] == 20);
    bool tick = false;
    for (const Json& h : p["handlers"]) {
        if (h["kind"] == "tick" && h["at"] == "scripts/main.ts:22") tick = h["calls"] == 20 && h["ms"].get<double>() >= 0;
    }
    REQUIRE(tick);
    REQUIRE(s.command("step", Json{{"ticks", 5}}).has_value());
    p = s.command("script.profile", Json::object()).value();
    REQUIRE(p["ticks"] == 5);
    for (const Json& h : p["handlers"]) REQUIRE(h["calls"].get<int>() <= 5);
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
    // Presses a tick apart are two presses: the second lets the key up and presses it again.
    REQUIRE(s.command("input.press", Json{{"action", "jump"}}).value()["actions"]["jump"]["pressed"] == true);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("input.press", Json{{"action", "jump"}}).value()["actions"]["jump"]["pressed"] == true);
    REQUIRE(s.command("input.hold", Json{{"action", "jump"}, {"ticks", 5}}).value()["actions"]["jump"]["pressed"] == true);   // a hold only holds longer
    REQUIRE(s.command("input.state", Json::object()).value()["held"]["Space"].get<std::int64_t>() > 0);
    for (int i = 0; i < 6; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("input.actions", Json::object()).value()["jump"]["down"] == false);
    // Pad events map like keys; a pad axis within the deadzone is nothing.
    Json st = s.command("input.state", Json::object()).value();
    REQUIRE(st["pads"] == 0);
    Json d = s.command("input.describe", Json::object()).value();
    REQUIRE(d["jump"]["positive"].size() == 2);
    REQUIRE_FALSE(s.command("input.hold", Json{{"action", "fly"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a glTF file's nodes become entities that draw their own parts", "[runtime][assets][nodetree]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    const std::filesystem::path ref = o.project_dir / ".pocket" / "crate-whole.png";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{ref};
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // The sample's camera off and one of our own far from its moving things, looking at the whole file.
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Camera"}, {"value", Json{{"active", false}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "TreeCam"}, {"components", Json{{"Transform", Json{{"position", {{"x", 100.0}, {"y", 0.6}, {"z", 4.0}}}}}, {"Camera", Json::object()}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Whole"}, {"components", Json{{"Transform", Json{{"position", {{"x", 100.0}, {"y", 0.0}, {"z", 0.0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/crate.glb"}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json vis = s.command("render.visible", Json::object()).value();
    INFO(vis.dump());
    bool whole_seen = false;
    for (const Json& v : vis["visible"]) if (v.value("name", "") == "Whole") whole_seen = true;
    REQUIRE(whole_seen);
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/crate-whole.png"}, {"update", true}}).value()["written"] == true);
    // The same file as a tree of entities: the parts by name, the small crate where the file puts it.
    REQUIRE(s.command("world.set", Json{{"entity", "Whole"}, {"component", "MeshRenderer"}, {"value", Json{{"visible", false}}}}).has_value());
    REQUIRE(s.command("assets.describe", Json{{"path", "assets/crate.glb"}}).value()["parts"] == Json::array({"crate", "crate_small"}));
    Json inst = s.command("world.instantiate", Json{{"mesh", "assets/crate.glb"}, {"name", "Tree"}, {"position", {{"x", 100.0}, {"y", 0.0}, {"z", 0.0}}}}).value();
    INFO(inst.dump());
    REQUIRE(inst["roots"].size() == 1);
    REQUIRE(s.command("world.children", Json{{"entity", "Tree"}}).value().size() == 2);
    Json small_t = s.command("world.get", Json{{"entity", "crate_small"}, {"component", "Transform"}}).value();
    REQUIRE(small_t["position"]["x"].get<double>() == Catch::Approx(1.5));
    REQUIRE(small_t["scale"]["x"].get<double>() == Catch::Approx(0.5));
    Json small_m = s.command("world.get", Json{{"entity", "crate_small"}, {"component", "MeshRenderer"}}).value();
    REQUIRE(small_m["node"] == "crate_small");
    REQUIRE(small_m["mesh"] == "assets/crate.glb");
    // It draws the same picture, each part its own entity on screen.
    REQUIRE(s.frame().has_value());
    Json same = s.command("render.compare", Json{{"path", ".pocket/crate-whole.png"}}).value();
    INFO(same.dump());
    REQUIRE(same["fraction"].get<double>() < 0.002);
    vis = s.command("render.visible", Json::object()).value();
    bool big = false, small = false;
    for (const Json& v : vis["visible"]) { if (v.value("name", "") == "crate") big = true; if (v.value("name", "") == "crate_small") small = true; }
    REQUIRE(big);
    REQUIRE(small);
    // A part moved apart changes the picture.
    REQUIRE(s.command("world.set", Json{{"entity", "crate_small"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 1.5}, {"y", 1.2}, {"z", 0.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json moved = s.command("render.compare", Json{{"path", ".pocket/crate-whole.png"}}).value();
    INFO(moved.dump());
    REQUIRE(moved["fraction"].get<double>() > 0.002);
    // A skinned file stays one drawable; an unknown node draws nothing and is reported.
    auto bad = s.command("world.instantiate", Json{{"mesh", "assets/arm.glb"}});
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().code == "unsupported");
    REQUIRE(s.command("world.set", Json{{"entity", "crate_small"}, {"component", "MeshRenderer"}, {"value", Json{{"node", "lid"}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    bool reported = false;
    for (const Json& m : stats["assets"]["missing"]) if (m.get<std::string>().find("#lid") != std::string::npos) reported = true;
    REQUIRE(reported);
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
    REQUIRE(rs["skinned"].get<int>() == 7);  // the Arm, the Walker, the Pulse arm, the Reacher, the Gazer, the Turner and the Stepper
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

TEST_CASE("a clip from another file plays on a model, matched by joint name", "[runtime][animation][library]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // arm_bow.glb: the arm's joints as "mixamorig:root" and "mixamorig:tip", one clip "mixamo.com".
    const Json lib = s.command("animation.library", Json{{"mesh", "assets/arm.glb"}, {"files", Json::array({"assets/arm_bow.glb"})}}).value();
    INFO(lib.dump());
    REQUIRE(lib["files"][0]["clips"][0]["name"] == "arm_bow");
    REQUIRE(lib["files"][0]["channels_left_out"] == 0);
    const Json clips = s.command("animation.clips", Json{{"entity", "Arm"}}).value();
    REQUIRE(clips["clips"].size() == 6);
    REQUIRE(s.command("animation.play", Json{{"entity", "Arm"}, {"clip", "arm_bow"}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    // Half a second in: the tip bowed about X to 60 degrees (its up axis turned toward Z).
    const Json pose = s.command("animation.pose", Json{{"entity", "Arm"}}).value();
    INFO(pose.dump());
    const Json tip = pose["joints"][1];
    REQUIRE(tip["name"] == "tip");
    REQUIRE(std::fabs(tip["axis_y"]["z"].get<double>()) == Catch::Approx(std::sin(60 * 3.14159265 / 180)).margin(0.05));
    REQUIRE(std::fabs(tip["axis_y"]["x"].get<double>()) < 0.05);
    // Read again, the model takes the clip again.
    REQUIRE(s.command("assets.reload", Json::object()).has_value());
    REQUIRE(s.command("animation.clips", Json{{"entity", "Arm"}}).value()["clips"].size() == 6);
    REQUIRE_FALSE(s.command("animation.library", Json{{"mesh", "assets/arm.glb"}, {"files", Json::array({"assets/none.glb"})}}).has_value());
    REQUIRE(s.finish().has_value());
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

TEST_CASE("an Area2D notices 2D bodies coming in and going out", "[runtime][sprites][area2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());   // the player has landed
    const Json at = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"];
    // A zone around where the player stands, and one far away.
    REQUIRE(s.command("world.spawn", Json{{"name", "Checkpoint"}, {"components", Json{{"Transform", Json{{"position", at}}}, {"Area2D", Json{{"size", Json{{"x", 1}, {"y", 1}}}}}}}}).has_value());
    const Json far_place = Json{{"position", Json{{"x", 50}, {"y", 50}, {"z", 0}}}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Far"}, {"components", Json{{"Transform", far_place}, {"Area2D", Json::object()}}}}).has_value());
    Json in = s.command("step", Json{{"ticks", 30}, {"until", Json{{"event", "area.entered"}}}}).value();
    INFO(in.dump());
    REQUIRE(in["until"]["met"] == true);
    REQUIRE(in["until"]["event"]["data"]["area"] == "/Checkpoint");
    REQUIRE(in["until"]["event"]["data"]["body"] == "/Player");
    const int held = s.command("world.get", Json{{"entity", "Checkpoint"}, {"component", "Area2D"}}).value()["inside"].get<int>();
    REQUIRE(held >= 1);   // the player, and whatever else stands within a unit of it
    REQUIRE(s.command("world.get", Json{{"entity", "Far"}, {"component", "Area2D"}}).value()["inside"] == 0);
    // Taken away: it leaves; switched off: nothing is inside and nothing comes in.
    REQUIRE(s.command("world.set", Json{{"entity", "Player"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", at["x"].get<double>() + 4}}}}}}).has_value());
    Json out = s.command("step", Json{{"ticks", 5}, {"until", Json{{"event", "area.exited"}}}}).value();
    REQUIRE(out["until"]["met"] == true);
    REQUIRE(out["until"]["event"]["data"]["body"] == "/Player");
    REQUIRE(s.command("world.get", Json{{"entity", "Checkpoint"}, {"component", "Area2D"}}).value()["inside"] == held - 1);
    REQUIRE(s.command("world.set", Json{{"entity", "Checkpoint"}, {"component", "Area2D"}, {"value", Json{{"enabled", false}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Checkpoint"}, {"component", "Area2D"}}).value()["inside"] == 0);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a hitbox hurts the Health that comes into it, pushes it away, waits out its guard and hits again while it stays", "[runtime][combat]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Player"}, {"component", "Health"}, {"value", Json{{"current", 30}, {"max", 30}, {"invulnerable", 0.25}, {"team", 1}}}}).has_value());
    const Json at = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"];
    // Spikes under the player: 10 a hit, a hit every 0.5 s while it stays, a push.
    REQUIRE(s.command("world.spawn", Json{{"name", "Spikes"}, {"components", Json{{"Transform", Json{{"position", at}}}, {"Area2D", Json{{"size", Json{{"x", 1}, {"y", 1}}}}}, {"Hitbox", Json{{"damage", 10}, {"repeat", 0.5}, {"knockback", 3}}}}}}).has_value());
    Json first = s.command("step", Json{{"ticks", 10}, {"until", Json{{"event", "hit"}}}}).value();
    INFO(first.dump());
    REQUIRE(first["until"]["met"] == true);
    REQUIRE(first["until"]["event"]["data"]["by"] == "/Spikes");
    REQUIRE(first["until"]["event"]["data"]["health"] == 20.0);
    REQUIRE(first["until"]["event"].contains("cause"));   // the touch that caused it (events.why)
    const Json hp = s.command("world.get", Json{{"entity", "Player"}, {"component", "Health"}}).value();
    REQUIRE(hp["current"] == 20.0);
    REQUIRE(hp["guard"].get<double>() > 0.2);
    // Held still in it: hit again half a second later, then down to nothing and dead.
    Json again = s.command("step", Json{{"ticks", 120}, {"until", Json{{"event", "health.depleted"}}}}).value();
    REQUIRE(again["until"]["met"] == true);
    REQUIRE(again["until"]["ticks"].get<int>() >= 50);   // two more hits, half a second apart
    const Json dead = s.command("world.get", Json{{"entity", "Player"}, {"component", "Health"}}).value();
    REQUIRE(dead["current"] == 0.0);
    REQUIRE(dead["dead"] == true);
    REQUIRE(s.command("world.get", Json{{"entity", "Spikes"}, {"component", "Hitbox"}}).value()["hits"] == 3);
    // Same team: no hurt. A bullet: one hit, then gone.
    REQUIRE(s.command("world.set", Json{{"entity", "Player"}, {"component", "Health"}, {"value", Json{{"current", 30}, {"guard", 0}}}}).has_value());   // revived by its hit points alone
    REQUIRE(s.command("world.destroy", Json{{"entity", "Spikes"}}).has_value());
    const Json here = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"];
    REQUIRE(s.command("world.spawn", Json{{"name", "Friendly"}, {"components", Json{{"Transform", Json{{"position", here}}}, {"Area2D", Json{{"size", Json{{"x", 1}, {"y", 1}}}}}, {"Hitbox", Json{{"team", 1}}}}}}).has_value());
    // A bullet as combat.shoot makes one in 2D: an Area2D moving by its Velocity, from two units away.
    const Json start = Json{{"position", Json{{"x", here["x"].get<double>() - 2}, {"y", here["y"]}, {"z", 0}}}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Bullet"}, {"components", Json{{"Transform", start}, {"Velocity", Json{{"linear", Json{{"x", 10}, {"y", 0}, {"z", 0}}}}}, {"Area2D", Json{{"size", Json{{"x", 0.15}, {"y", 0.15}}}}}, {"Hitbox", Json{{"damage", 5}, {"destroy", true}}}, {"Lifetime", Json{{"seconds", 2}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Player"}, {"component", "Health"}}).value()["current"] == 25.0);
    REQUIRE_FALSE(s.command("world.find", Json{{"path", "Bullet"}}).value().is_number());
    // A hitbox with nothing to notice touches is named by the lint.
    REQUIRE(s.command("world.spawn", Json{{"name", "Lonely"}, {"components", Json{{"Transform", Json::object()}, {"Hitbox", Json::object()}}}}).has_value());
    const Json lint = s.command("world.lint", Json::object()).value();
    bool named = false;
    for (const Json& pr : lint["problems"]) named = named || (pr["component"] == "Hitbox" && pr["path"] == "/Lonely");
    REQUIRE(named);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("an overlap query takes a sphere, a turned box or a capsule, and finds characters too", "[runtime][physics][overlap]") {
    app::Options o;
    o.project_dir = root() / "samples" / "walker";
    o.bundle = root() / "build" / "ts" / "walker.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 10}}).has_value());
    const Json at = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"];
    auto paths = [&](const Json& params) {
        std::vector<std::string> out;
        for (const Json& r : s.command("physics.overlap", params).value()) out.push_back(r["path"].get<std::string>());
        return out;
    };
    auto has = [](const std::vector<std::string>& v, const char* p) { return std::find(v.begin(), v.end(), p) != v.end(); };
    REQUIRE(has(paths(Json{{"center", at}, {"radius", 0.2}}), "/Player"));
    // A character with no collider of its own is found as a Character, unless characters are left out.
    const Json ghost_at = Json{{"x", at["x"].get<double>()}, {"y", at["y"].get<double>() + 6}, {"z", at["z"]}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Ghost"}, {"components", Json{{"Transform", Json{{"position", ghost_at}}}, {"Character", Json{{"gravity", 0}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    const Json ghost_now = s.command("world.get", Json{{"entity", "Ghost"}, {"component", "WorldTransform"}}).value()["position"];
    REQUIRE(has(paths(Json{{"center", ghost_now}, {"radius", 0.2}}), "/Ghost"));
    REQUIRE_FALSE(has(paths(Json{{"center", ghost_now}, {"radius", 0.2}, {"characters", false}}), "/Ghost"));
    // A thin box beside the player: square on, it misses; turned 90 degrees about Y, its long side reaches.
    const Json beside = Json{{"x", at["x"].get<double>() + 1.0}, {"y", at["y"]}, {"z", at["z"]}};
    const Json slab = Json{{"x", 0.1}, {"y", 0.5}, {"z", 1.2}};
    REQUIRE_FALSE(has(paths(Json{{"center", beside}, {"shape", "box"}, {"size", slab}}), "/Player"));
    REQUIRE(has(paths(Json{{"center", beside}, {"shape", "box"}, {"size", slab}, {"rotation", Json{{"x", 0}, {"y", 0.7071068}, {"z", 0}, {"w", 0.7071068}}}}), "/Player"));
    // A capsule standing a little off: reaches it with a radius that touches.
    const Json off = Json{{"x", at["x"].get<double>() + 0.5}, {"y", at["y"]}, {"z", at["z"]}};
    REQUIRE(has(paths(Json{{"center", off}, {"shape", "capsule"}, {"radius", 0.3}, {"height", 1.8}}), "/Player"));
    REQUIRE_FALSE(has(paths(Json{{"center", off}, {"shape", "capsule"}, {"radius", 0.1}, {"height", 1.8}}), "/Player"));
    REQUIRE_FALSE(s.command("physics.overlap", Json{{"center", at}, {"shape", "cone"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("cloth hangs at its length, streams out in the wind, keeps out of a ball and is drawn", "[runtime][cloth]") {
    auto o = hello_options(1000);
    o.paused = true;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // Away from the sample's own things (its script keeps running on them).
    auto sheet = [&](const char* name, double x, Json cloth) {
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", 3}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}, {"Cloth", cloth}}}}).has_value());
    };
    // A curtain from its top edge, a flag from its left edge, a sheet a ball pushes into.
    sheet("Curtain", 50, Json{{"size", Json{{"x", 1.0}, {"y", 2.0}}}, {"segments", Json{{"x", 8}, {"y", 12}}}});
    sheet("Flag", 54, Json{{"size", Json{{"x", 1.6}, {"y", 1.0}}}, {"segments", Json{{"x", 12}, {"y", 8}}}, {"pin", "left"}, {"weight", 0.2}});
    sheet("Screen", 58, Json{{"size", Json{{"x", 1.6}, {"y", 2.0}}}, {"segments", Json{{"x", 12}, {"y", 12}}}});
    REQUIRE(s.command("world.spawn", Json{{"name", "Ball"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 58}, {"y", 2}, {"z", 0.3}}}}}, {"RigidBody", Json{{"kind", "static"}}}, {"Collider", Json{{"shape", "sphere"}, {"size", Json{{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 120}}).has_value());
    const Json curtain = s.command("physics.cloth", Json{{"entity", "Curtain"}}).value();
    INFO(curtain.dump());
    REQUIRE(curtain["particles"] == 9 * 13);
    REQUIRE(curtain["across"] == 9);
    // Hanging straight: as long as it is, not stretched by its own weight beyond a few percent.
    REQUIRE(curtain["bounds"]["min"]["y"].get<double>() == Catch::Approx(1.0).margin(0.08));
    REQUIRE(std::abs(curtain["bounds"]["max"]["z"].get<double>()) < 0.05);
    // Without wind the flag droops: its free edge far below the pole's top.
    const Json still = s.command("physics.cloth", Json{{"entity", "Flag"}}).value();
    REQUIRE(still["lowest"]["y"].get<double>() < 2.0 - 0.3);
    // The screen keeps half a ball's width and a thread's thickness from the ball's centre.
    for (const Json& pt : s.command("physics.cloth", Json{{"entity", "Screen"}, {"points", true}}).value()["points"]) {
        const double dx = pt[0].get<double>() - 58, dy = pt[1].get<double>() - 2, dz = pt[2].get<double>() - 0.3;
        REQUIRE(std::sqrt(dx * dx + dy * dy + dz * dz) > 0.5);
    }
    // A wind along +x: the flag streams out from its pole, its far edge nearly as high as the pole's top.
    REQUIRE(s.command("world.spawn", Json{{"name", "Wind"}, {"components", Json{{"Wind", Json{{"direction", 0}, {"speed", 8}, {"gusts", 0}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 240}}).has_value());
    const Json flying = s.command("physics.cloth", Json{{"entity", "Flag"}}).value();
    INFO(flying.dump());
    REQUIRE(flying["bounds"]["max"]["x"].get<double>() > 54 - 0.8 + 1.6 * 0.8);   // the pole at 53.2
    REQUIRE(flying["lowest"]["y"].get<double>() > still["lowest"]["y"].get<double>() + 0.2);
    // Drawn as a mesh of its own, both sides: two triangles a square, twice.
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["triangles"].get<int>() >= 4 * (8 * 12 + 12 * 8 + 12 * 12));
    REQUIRE(s.command("physics.stats", Json::object()).value()["cloth"] == 3);
    // Taken off, its sheet goes.
    REQUIRE(s.command("world.remove", Json{{"entity", "Curtain"}, {"component", "Cloth"}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    REQUIRE(s.command("physics.stats", Json::object()).value()["cloth"] == 2);
    REQUIRE_FALSE(s.command("physics.cloth", Json{{"entity", "Curtain"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a cape is cloth attached to a character's spine: it goes where the walker goes", "[runtime][cloth][cape]") {
    app::Options o;
    o.project_dir = root() / "samples" / "walker";
    o.bundle = root() / "build" / "ts" / "walker.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 5}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Cape"}, {"components", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", "cube"}}},
        {"Attach", Json{{"target", "Player/Hero"}, {"joint", "Spine"}, {"offset", Json{{"x", 0}, {"y", 0.5}, {"z", -0.14}}}}},
        {"Cloth", Json{{"size", Json{{"x", 0.55}, {"y", 1.0}}}, {"segments", Json{{"x", 8}, {"y", 12}}}, {"weight", 0.6}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    const double x0 = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"]["x"].get<double>();
    REQUIRE(s.command("input.hold", Json{{"action", "move_x"}, {"ticks", 90}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 90}}).has_value());
    const Json player = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"];
    const Json cape = s.command("physics.cloth", Json{{"entity", "Cape"}, {"points", true}}).value();
    const Json top = cape["points"][4];   // the middle of its pinned top edge
    const Json at = s.command("world.get", Json{{"entity", "Cape"}, {"component", "WorldTransform"}}).value()["position"];
    INFO("player " << player.dump() << ", cape top " << top.dump() << ", the cape's place " << at.dump());
    REQUIRE(player["x"].get<double>() > x0 + 3);
    // Its top held at the place on the spine it is attached to, the rest hanging below and behind.
    REQUIRE(std::abs(top[0].get<double>() - at["x"].get<double>()) < 0.05);
    REQUIRE(std::abs(top[1].get<double>() - at["y"].get<double>()) < 0.05);
    REQUIRE(std::abs(top[2].get<double>() - at["z"].get<double>()) < 0.05);
    REQUIRE(cape["lowest"]["y"].get<double>() < at["y"].get<double>() - 0.5);
    REQUIRE(std::abs(cape["lowest"]["x"].get<double>() - player["x"].get<double>()) < 1.0);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a ragdoll makes a character's bones into bodies that fall, and gives the pose back", "[runtime][ragdoll]") {
    app::Options o;
    o.project_dir = root() / "samples" / "walker";
    o.bundle = root() / "build" / "ts" / "walker.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 20}}).has_value());
    auto head = [&]() {
        for (const Json& j : s.command("animation.pose", Json{{"entity", "Player/Hero"}}).value()["joints"]) {
            if (j["name"] == "Head") return j["position"];
        }
        return Json();
    };
    const double standing = head()["y"].get<double>();
    const Json hero_at = s.command("world.get", Json{{"entity", "Player/Hero"}, {"component", "WorldTransform"}}).value()["position"];
    REQUIRE(s.command("world.set", Json{{"entity", "Player/Hero"}, {"component", "Ragdoll"}, {"value", Json{{"active", true}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    const Json made = s.command("world.get", Json{{"entity", "Player/Hero"}, {"component", "Ragdoll"}}).value();
    INFO(made.dump());
    // A body for each of the hero's eleven bones, each named after it, under an entity of their own.
    REQUIRE(made["bodies"] == 11);
    const std::string rag = made["root"].get<std::string>();
    REQUIRE(s.command("world.children", Json{{"entity", rag}}).value().size() == 11);
    REQUIRE(s.command("world.get", Json{{"entity", rag + "/LeftForeArm"}, {"component", "Collider"}}).value()["shape"] == 2);
    REQUIRE(s.command("world.get", Json{{"entity", rag + "/LeftForeArm"}, {"component", "Joint"}}).value()["target"] == rag + "/LeftArm");
    REQUIRE(s.command("events.recent", Json{{"type", "ragdoll.started"}}).value().size() == 1);
    const Json hips_at = s.command("world.get", Json{{"entity", rag + "/Hips"}, {"component", "Transform"}}).value()["position"];
    // At first the pose is the one it stood in.
    REQUIRE(std::abs(head()["y"].get<double>() - standing) < 0.05);
    REQUIRE(s.command("step", Json{{"ticks", 150}}).has_value());
    // On the ground: every body low, the head with them, the entity moved along with the hips.
    for (const Json& b : s.command("world.children", Json{{"entity", rag}}).value()) {
        const double y = s.command("world.get", Json{{"entity", b}, {"component", "Transform"}}).value()["position"]["y"].get<double>();
        INFO(s.command("world.describe", Json{{"entity", b}}).value()["name"] << " at " << y);
        REQUIRE(y > -0.05);
        REQUIRE(y < 0.6);
    }
    const double fallen = head()["y"].get<double>();
    INFO("head standing " << standing << ", fallen " << fallen);
    REQUIRE(fallen < 0.5);
    REQUIRE(standing > 1.4);
    const Json hero_now = s.command("world.get", Json{{"entity", "Player/Hero"}, {"component", "WorldTransform"}}).value()["position"];
    const Json hips_now = s.command("world.get", Json{{"entity", rag + "/Hips"}, {"component", "Transform"}}).value()["position"];
    // Followed the hips across the ground, its height kept.
    REQUIRE(std::abs(hero_now["y"].get<double>() - hero_at["y"].get<double>()) < 0.05);
    REQUIRE(std::abs((hero_now["x"].get<double>() - hero_at["x"].get<double>()) - (hips_now["x"].get<double>() - hips_at["x"].get<double>())) < 0.02);
    REQUIRE(std::abs((hero_now["z"].get<double>() - hero_at["z"].get<double>()) - (hips_now["z"].get<double>() - hips_at["z"].get<double>())) < 0.02);
    // Off again: the bodies go and the clips have the pose back.
    REQUIRE(s.command("world.set", Json{{"entity", "Player/Hero"}, {"component", "Ragdoll"}, {"value", Json{{"active", false}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(s.command("world.find", Json{{"path", rag}}).value().is_null());
    REQUIRE(s.command("world.get", Json{{"entity", "Player/Hero"}, {"component", "Ragdoll"}}).value()["bodies"] == 0);
    REQUIRE(s.command("events.recent", Json{{"type", "ragdoll.stopped"}}).value().size() == 1);
    // Standing up is a fade from the pose it lay in: part way after two ticks, up after half a second.
    const double rising = head()["y"].get<double>();
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    const double up = head()["y"].get<double>();
    INFO("head two ticks into standing up " << rising << ", after half a second " << up);
    REQUIRE(rising < standing - 0.3);
    REQUIRE(std::abs(up - standing) < 0.15);
    // A mesh without bones cannot go limp, and says so once.
    REQUIRE(s.command("world.spawn", Json{{"name", "Crate"}, {"components", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", "cube"}}}, {"Ragdoll", Json{{"active", true}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 3}}).has_value());
    REQUIRE(s.command("events.recent", Json{{"type", "ragdoll.refused"}}).value().size() == 1);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a hitbox on a 3D trigger hurts a character that walks into it", "[runtime][combat]") {
    app::Options o;
    o.project_dir = root() / "samples" / "walker";
    o.bundle = root() / "build" / "ts" / "walker.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 20}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Player"}, {"component", "Health"}, {"value", Json{{"current", 50}, {"max", 50}}}}).has_value());
    const Json at = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"];
    const Json place = Json{{"position", Json{{"x", at["x"].get<double>() + 2.0}, {"y", at["y"]}, {"z", at["z"]}}}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Fire"}, {"components", Json{{"Transform", place}, {"RigidBody", Json{{"kind", "static"}}}, {"Collider", Json{{"shape", "box"}, {"size", Json{{"x", 0.8}, {"y", 1}, {"z", 0.8}}}, {"is_trigger", true}}}, {"Hitbox", Json{{"damage", 15}}}}}}).has_value());
    REQUIRE(s.command("input.hold", Json{{"action", "move_x"}, {"ticks", 90}}).has_value());
    Json r = s.command("step", Json{{"ticks", 90}, {"until", Json{{"event", "hit"}}}}).value();
    INFO(r.dump());
    REQUIRE(r["until"]["met"] == true);
    REQUIRE(r["until"]["event"]["data"]["by"] == "/Fire");
    REQUIRE(s.command("world.get", Json{{"entity", "Player"}, {"component", "Health"}}).value()["current"] == 35.0);
    // A bullet as combat.shoot makes one in 3D: a kinematic trigger sphere moving by its Velocity.
    const Json now = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value()["position"];
    const Json from = Json{{"x", now["x"].get<double>() - 4}, {"y", now["y"]}, {"z", now["z"]}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Shot"}, {"components", Json{
        {"Transform", Json{{"position", from}}}, {"Velocity", Json{{"linear", Json{{"x", 12}, {"y", 0}, {"z", 0}}}}},
        {"RigidBody", Json{{"kind", "kinematic"}}}, {"Collider", Json{{"shape", "sphere"}, {"size", Json{{"x", 0.15}, {"y", 0.15}, {"z", 0.15}}}, {"is_trigger", true}}},
        {"Hitbox", Json{{"damage", 5}, {"destroy", true}}}, {"Lifetime", Json{{"seconds", 2}}}}}}).has_value());
    Json shot = s.command("step", Json{{"ticks", 60}, {"until", Json{{"event", "hit"}}}}).value();
    INFO(shot.dump());
    REQUIRE(shot["until"]["met"] == true);
    REQUIRE(shot["until"]["event"]["data"]["by"] == "/Shot");
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    REQUIRE_FALSE(s.command("world.find", Json{{"path", "Shot"}}).value().is_number());   // gone on its hit
    REQUIRE(s.finish().has_value());
}

TEST_CASE("an Aseprite sheet becomes a clip per tag, each frame its own rectangle and time", "[runtime][sprites][aseprite]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    const Json sheet = s.command("sprite.sheet", Json{{"path", "assets/player.aseprite.json"}}).value();
    INFO(sheet.dump());
    const Json& clips = sheet["clips"];
    REQUIRE(clips.contains("player.idle"));
    REQUIRE(clips["player.walk"]["frames"] == Json::array({0, 1, 2, 3, 2, 1}));   // ping-pong without repeating the ends
    REQUIRE(clips["player.blink"]["frames"] == Json::array({2, 1, 2, 1}));        // reversed, played twice
    REQUIRE(clips["player.blink"]["loop"] == false);
    REQUIRE(clips["player.walk"]["texture"] == "assets/player.png");
    REQUIRE(clips["player.walk"]["rects"][1] == Json::array({0.25, 0.0, 0.5, 1.0}));
    REQUIRE(clips["player.walk"]["durations"][0].get<double>() == Catch::Approx(0.3));
    // Played on a sprite: the first frame holds 0.3 s (18 ticks), the next 0.1 s (6 ticks).
    REQUIRE(s.command("world.spawn", Json{{"name", "Dancer"}, {"components", Json{{"Transform", Json::object()}, {"Sprite", Json::object()}, {"SpriteAnimation", Json{{"clip", "player.walk"}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 17}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Dancer"}, {"component", "SpriteAnimation"}}).value()["frame"] == 0);
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Dancer"}, {"component", "SpriteAnimation"}}).value()["frame"] == 1);
    const Json uv = s.command("world.get", Json{{"entity", "Dancer"}, {"component", "Sprite"}}).value()["uv"];
    REQUIRE(uv["x"].get<double>() == Catch::Approx(0.25));
    REQUIRE(uv["z"].get<double>() == Catch::Approx(0.5));
    REQUIRE(s.command("step", Json{{"ticks", 6}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Dancer"}, {"component", "SpriteAnimation"}}).value()["frame"] == 2);
    REQUIRE_FALSE(s.command("sprite.sheet", Json{{"path", "../../etc/passwd"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("an additive sprite adds its light to what is behind instead of covering it", "[runtime][sprites][additive]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // A red square over the middle of the view, in front of everything: covering, then adding.
    const Json cam = s.command("world.get", Json{{"entity", "Camera"}, {"component", "Transform"}}).value()["position"];
    REQUIRE(s.command("world.spawn", Json{{"name", "Glow"}, {"components", Json{
        {"Transform", Json{{"position", Json{{"x", cam["x"]}, {"y", cam["y"]}, {"z", 0.5}}}}},
        {"Sprite", Json{{"size", Json{{"x", 4}, {"y", 4}}}, {"color", Json{{"r", 0.6}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"layer", 100}}}}}}).has_value());
    auto center = [&] {
        REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
        return s.command("capture", Json{{"pixel", Json{{"x", 160}, {"y", 90}}}}).value()["pixel"];
    };
    const Json covered = center();
    INFO(covered.dump());
    REQUIRE(covered[1].get<int>() <= 2);   // red over everything: no green from behind
    REQUIRE(s.command("world.set", Json{{"entity", "Glow"}, {"component", "Sprite"}, {"value", Json{{"additive", true}}}}).has_value());
    const Json added = center();
    INFO(added.dump());
    REQUIRE(added[1].get<int>() > 10);     // what is behind shows through, its green kept
    REQUIRE(added[0].get<int>() > covered[0].get<int>() - 2);   // and its red brightened, not replaced
    REQUIRE(s.finish().has_value());
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

TEST_CASE("a top-down mover is stopped by solid cells on isometric and orthogonal maps", "[runtime][tilemap][topdown]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    // The same 4 by 3 isometric map as above: row 0 is open but its first cell, row 1 solid, row 2 open.
    const std::filesystem::path file = o.project_dir / "assets" / "iso-test.tmj";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    {
        std::ofstream out(file);
        out << R"({"width":4,"height":3,"tilewidth":32,"tileheight":16,"orientation":"isometric","tilesets":[{"firstgid":1,"name":"tiles","image":"tiles.png","imagewidth":80,"imageheight":16,"tilewidth":16,"tileheight":16,"columns":5,"tilecount":5,"tiles":[{"id":0,"properties":[{"name":"solid","type":"bool","value":true}]}]}],"layers":[{"id":1,"type":"tilelayer","name":"floor","width":4,"height":3,"data":[1,2,3,4,1,1,1,1,2,2,2,2]}]})";
    }
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Iso"}, {"components", Json{{"Transform", Json{{"position", {{"x", 100}, {"y", 0}, {"z", 0}}}}}, {"TileMap", Json{{"map", "assets/iso-test.tmj"}, {"tile_size", 1.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    // The center of cell (1, 0), from the map's own geometry.
    Json cell = s.command("tilemap.cell", Json{{"entity", "Iso"}, {"x", 102.0}, {"y", -0.5}}).value();
    INFO(cell.dump());
    REQUIRE(cell["tile_x"] == 1);
    REQUIRE(cell["tile_y"] == 0);
    const double cx = cell["center"]["x"].get<double>(), cy = cell["center"]["y"].get<double>();
    // From the upper part of the diamond, down lies the solid row: the walker moves, stops short of it (its
    // radius touching the cell below) and reports the block and its cell.
    const double y0 = cy + 0.2;
    REQUIRE(s.command("world.spawn", Json{{"name", "Walker"}, {"components", Json{{"Transform", Json{{"position", {{"x", cx}, {"y", y0}, {"z", 0}}}}}, {"TopDown2D", Json{{"velocity", {{"x", 0.0}, {"y", -3.0}}}, {"radius", 0.2}, {"map", "Iso"}}}}}}).has_value());
    // To the right the row is open: the runner keeps going.
    REQUIRE(s.command("world.spawn", Json{{"name", "Runner"}, {"components", Json{{"Transform", Json{{"position", {{"x", cx}, {"y", y0}, {"z", 0}}}}}, {"TopDown2D", Json{{"velocity", {{"x", 3.0}, {"y", 0.0}}}, {"radius", 0.2}, {"map", "Iso"}}}}}}).has_value());
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json walker = s.command("world.get", Json{{"entity", "Walker"}, {"component", "Transform"}}).value();
    Json wb = s.command("world.get", Json{{"entity", "Walker"}, {"component", "TopDown2D"}}).value();
    INFO("walker " << walker.dump() << " " << wb.dump());
    REQUIRE(wb["blocked_y"] == true);
    REQUIRE(wb["blocked_x"] == false);
    REQUIRE(walker["position"]["y"].get<double>() > cy - 0.5);   // stopped within its own cell
    REQUIRE(walker["position"]["y"].get<double>() < y0 - 0.1);   // but it did move down first
    REQUIRE(wb["tile_x"] == 1);
    REQUIRE(wb["tile_y"] == 0);
    REQUIRE(s.command("tilemap.cell", Json{{"entity", "Iso"}, {"x", walker["position"]["x"]}, {"y", walker["position"]["y"]}}).value()["tile_y"] == 0);
    Json runner = s.command("world.get", Json{{"entity", "Runner"}, {"component", "Transform"}}).value();
    Json rb = s.command("world.get", Json{{"entity", "Runner"}, {"component", "TopDown2D"}}).value();
    INFO("runner " << runner.dump() << " " << rb.dump());
    REQUIRE(rb["blocked_x"] == false);
    REQUIRE(runner["position"]["x"].get<double>() == Catch::Approx(cx + 3.0).margin(0.06));
    REQUIRE(rb["tile_x"] == -1);   // off the map by now
    // On the sample's orthogonal level a cart driven into the ground below the player stops on it.
    Json player = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value();
    REQUIRE(s.command("world.spawn", Json{{"name", "Cart"}, {"components", Json{{"Transform", Json{{"position", {{"x", player["position"]["x"]}, {"y", player["position"]["y"]}, {"z", 0}}}}}, {"TopDown2D", Json{{"velocity", {{"x", 0.0}, {"y", -3.0}}}, {"radius", 0.25}, {"map", "Level"}}}}}}).has_value());
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json cart = s.command("world.get", Json{{"entity", "Cart"}, {"component", "Transform"}}).value();
    Json cb = s.command("world.get", Json{{"entity", "Cart"}, {"component", "TopDown2D"}}).value();
    INFO("cart " << cart.dump() << " " << cb.dump());
    REQUIRE(cb["blocked_y"] == true);
    REQUIRE(cart["position"]["y"].get<double>() > player["position"]["y"].get<double>() - 1.5);
    REQUIRE(cb["tile_x"].get<int>() >= 0);
    REQUIRE(cb["tile_y"].get<int>() >= 0);
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Level"}, {"tile_x", cb["tile_x"]}, {"tile_y", cb["tile_y"]}}).value()["solid"] == false);
    REQUIRE(s.command("physics.stats", Json::object()).value()["tiles"]["movers"] == 3);
    REQUIRE(s.finish().has_value());
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
    // The tools' shape for several calls, sent to the server as it is: each answered in order.
    Json many;
    std::thread tc([&] { many = post(R"({"id":3,"calls":[{"method":"world.summary"},{"method":"no.such"},{"method":"state"}]})"); });
    while (server->describe()["handled"].get<std::uint64_t>() < 4) server->pump(50);
    tc.join();
    INFO(many.dump());
    REQUIRE(many["result"].size() == 3);
    REQUIRE(many["result"][0]["result"]["entities"].get<int>() > 0);
    REQUIRE(many["result"][1].contains("error"));
    REQUIRE(many["result"][2]["result"]["frames"] == 3601);
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

TEST_CASE("pad bindings with a player index answer that pad only", "[runtime][input][players]") {
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
    Json map;
    map["p1_jump"] = Json{{"positive", Json::array({"pad0:a"})}};
    map["p2_jump"] = Json{{"positive", Json::array({"pad1:a"})}};
    map["any_jump"] = Json::array({"pad:a"});
    map["p2_move"] = Json{{"axis", Json::array({"pad1:leftx"})}};
    REQUIRE(s.command("input.map", Json{{"actions", map}}).has_value());
    REQUIRE(s.command("input.describe", Json::object()).value()["p2_move"]["axis"] == Json::array({"pad1:leftx"}));
    // Pad 1's button: the second player's action and the any-pad action, not the first player's.
    Json down = s.command("input.pad", Json{{"pad", 1}, {"button", "a"}, {"pressed", true}}).value();
    INFO(down.dump());
    REQUIRE(down["input"][0]["type"] == "pad_button");
    REQUIRE(down["input"][0]["pad"] == 1);
    REQUIRE(down["actions"]["p2_jump"]["down"] == true);
    REQUIRE(down["actions"]["any_jump"]["down"] == true);
    REQUIRE(down["actions"]["p1_jump"]["down"] == false);
    Json up = s.command("input.pad", Json{{"pad", 1}, {"button", "a"}, {"pressed", false}}).value();
    REQUIRE(up["actions"]["p2_jump"]["down"] == false);
    REQUIRE(up["actions"]["any_jump"]["down"] == false);
    // Pad 1's stick moves the second player; pad 0's does not.
    Json stick = s.command("input.pad", Json{{"pad", 1}, {"axis", "leftx"}, {"value", 0.8}}).value();
    REQUIRE(stick["actions"]["p2_move"]["value"].get<double>() == Catch::Approx((0.8 - 0.15) / 0.85).margin(1e-4));
    REQUIRE(s.command("input.pad", Json{{"pad", 1}, {"axis", "leftx"}, {"value", 0.0}}).value()["actions"]["p2_move"]["value"].get<double>() == Catch::Approx(0.0));
    REQUIRE(s.command("input.pad", Json{{"pad", 0}, {"axis", "leftx"}, {"value", 0.8}}).value()["actions"]["p2_move"]["value"].get<double>() == Catch::Approx(0.0));
    REQUIRE_FALSE(s.command("input.pad", Json{{"pad", 0}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a finger presses interface buttons, drives mouse bindings and reaches scripts as touch events", "[runtime][input][touch]") {
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
    // A button over the top-left corner of the window.
    Json ops = Json::array();
    ops.push_back(Json::array({"create", 70, "box"}));
    ops.push_back(Json::array({"set", 70, Json{{"position", "absolute"}, {"left", 10}, {"top", 10}, {"width", 100}, {"height", 40}, {"name", "tap"}, {"on", Json::array({"click"})}}}));
    ops.push_back(Json::array({"append", 1, 70}));
    REQUIRE(s.command("ui.apply", Json{{"ops", ops}}).has_value());
    REQUIRE(s.frame().has_value());
    // The first finger down and up on it is a click; the touch events carry the finger and land on the element.
    Json down = s.command("input.touch", Json{{"x", 60}, {"y", 30}, {"phase", "down"}}).value();
    INFO(down.dump());
    REQUIRE(down["input"].size() == 2);
    REQUIRE(down["input"][0]["type"] == "touch_down");
    REQUIRE(down["input"][0]["finger"] == 0);
    REQUIRE(down["input"][0]["ui"] == 70);
    REQUIRE(down["input"][0]["pressure"] == 1.0);
    REQUIRE(down["input"][1]["type"] == "mouse_down");
    Json up = s.command("input.touch", Json{{"x", 62}, {"y", 31}, {"phase", "up"}}).value();
    INFO(up.dump());
    bool clicked = false;
    for (const Json& e : up["events"]) if (e["type"] == "click" && e.value("name", "") == "tap") clicked = true;
    REQUIRE(clicked);
    // A second finger is not the mouse: no mouse events, its own index.
    Json second = s.command("input.touch", Json{{"finger", 1}, {"x", 200}, {"y", 100}, {"phase", "down"}, {"pressure", 0.25}}).value();
    REQUIRE(second["input"].size() == 1);
    REQUIRE(second["input"][0]["finger"] == 1);
    REQUIRE(second["input"][0]["pressure"].get<double>() == Catch::Approx(0.25));
    // The journal's form of a touch keeps its finger and pressure.
    const platform::Event back = platform::event_from_json(second["input"][0]);
    REQUIRE(back.pad == 1);
    REQUIRE(back.value == Catch::Approx(0.25));
    REQUIRE(s.command("input.state", Json::object()).value()["fingers"] == 1);
    REQUIRE(s.command("input.touch", Json{{"finger", 1}, {"x", 200}, {"y", 100}, {"phase", "up"}}).has_value());
    REQUIRE(s.command("input.state", Json::object()).value()["fingers"] == 0);
    REQUIRE_FALSE(s.command("input.touch", Json{{"finger", 3}, {"x", 0}, {"y", 0}, {"phase", "move"}}).has_value());
    // Moving the first finger drives a mouse axis binding like a mouse move.
    REQUIRE(s.command("input.map", Json{{"actions", Json{{"look", Json{{"axis", Json::array({"mouse:x"})}}}}}}).has_value());
    REQUIRE(s.command("input.touch", Json{{"x", 100}, {"y", 100}, {"phase", "down"}}).has_value());
    Json move = s.command("input.touch", Json{{"x", 160}, {"y", 100}, {"phase", "move"}}).value();
    INFO(move.dump());
    REQUIRE(move["input"][0]["type"] == "touch_move");
    REQUIRE(move["input"][0]["dx"].get<double>() == Catch::Approx(60.0));
    REQUIRE(move["input"][1]["type"] == "mouse_move");
    REQUIRE(s.command("input.state", Json::object()).value()["actions"]["look"]["value"].get<double>() > 0.0);
    REQUIRE(s.command("input.touch", Json{{"x", 160}, {"y", 100}, {"phase", "up"}}).has_value());
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
    // Only the pane's own highlight is left in green and blue (a dielectric reflects a little white,
    // which the sRGB encoding of linear light makes visible): far under the scene showing through.
    REQUIRE(px(opaque["center_pixel"], 1) < 40);
    REQUIRE(px(opaque["center_pixel"], 2) < 40);
    REQUIRE(px(opaque["center_pixel"], 1) * 2 < px(half["center_pixel"], 1));
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

TEST_CASE("colliding particles land on a level's solid tiles", "[runtime][particles][collide][tiles]") {
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
    // The ground under the player: the solid cell below its feet, and that cell's top edge.
    Json player = s.command("world.get", Json{{"entity", "Player"}, {"component", "Transform"}}).value();
    const double px = player["position"]["x"].get<double>(), py = player["position"]["y"].get<double>();
    Json ground = s.command("tilemap.solid", Json{{"entity", "Level"}, {"x", px}, {"y", py - 0.6}}).value();
    INFO(ground.dump());
    REQUIRE(ground["solid"] == true);
    Json cell = s.command("tilemap.cell", Json{{"entity", "Level"}, {"x", px}, {"y", py - 0.6}}).value();
    const double top = cell["center"]["y"].get<double>() + 0.5;
    // Sparks dropped straight down from above the player, with and without collision.
    auto emitter = [&](const char* name, bool collide) {
        Json fields = Json{{"emitting", false}, {"gravity", {{"x", 0}, {"y", -10}, {"z", 0}}}, {"lifetime", {{"x", 8}, {"y", 8}}}, {"speed", {{"x", 0.5}, {"y", 0.5}}}, {"direction", {{"x", 0}, {"y", -1}, {"z", 0}}}, {"spread", 0}, {"size", {{"x", 0.05}, {"y", 0.05}}}, {"collide", collide}, {"bounce", 0.3}};
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", {{"x", px}, {"y", py + 1.5}, {"z", 0.0}}}}}, {"ParticleEmitter", fields}}}}).has_value());
        REQUIRE(s.frame().has_value());
        REQUIRE(s.command("particles.burst", Json{{"entity", name}, {"count", 20}}).has_value());
    };
    emitter("Sparks", true);
    emitter("Ghosts", false);
    for (int i = 0; i < 120; ++i) REQUIRE(s.frame().has_value());
    Json sparks = s.command("particles.list", Json{{"entity", "Sparks"}, {"limit", 100}}).value()["particles"];
    Json ghosts = s.command("particles.list", Json{{"entity", "Ghosts"}, {"limit", 100}}).value()["particles"];
    REQUIRE(sparks.size() == 20);
    REQUIRE(ghosts.size() == 20);
    int resting = 0;
    for (const Json& p : sparks) {
        INFO(p.dump() << " top " << top);
        REQUIRE(p["position"]["y"].get<double>() >= top - 0.02);   // never into the ground
        REQUIRE(p["position"]["y"].get<double>() <= top + 0.3);
        resting += p["resting"] == true;
    }
    REQUIRE(resting == 20);
    for (const Json& p : ghosts) REQUIRE(p["position"]["y"].get<double>() < top - 1.0);   // through the tiles, still falling
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

TEST_CASE("an anchored element follows its entity's projection and hides when the entity is gone", "[runtime][ui][anchor]") {
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
    const auto player = s.command("world.find", Json{{"path", "Player"}}).value().get<std::uint64_t>();
    // A name tag standing 6 points over the player's origin.
    Json ops = Json::array();
    ops.push_back(Json::array({"create", 61, "text"}));
    ops.push_back(Json::array({"set", 61, Json{{"anchor", player}, {"anchorOffset", Json::array({0, -6})}, {"fontSize", 12}, {"name", "tag"}}}));
    ops.push_back(Json::array({"text", 61, "Hero"}));
    ops.push_back(Json::array({"append", 1, 61}));
    REQUIRE(s.command("ui.apply", Json{{"ops", ops}}).has_value());
    REQUIRE(s.frame().has_value());
    auto placed = [&]() {
        Json d = s.command("ui.describe", Json{{"id", 61}}).value();
        Json pr = s.command("render.project", Json{{"entity", player}}).value();
        INFO(d.dump() << " " << pr.dump());
        REQUIRE(pr["visible"] == true);
        REQUIRE(d["visible"] == true);
        REQUIRE(d["anchor"].get<std::uint64_t>() == player);
        REQUIRE(d["rect"]["w"].get<double>() > 10);
        // Bottom center over the point, 6 points up (headless: one pixel per point).
        REQUIRE(d["rect"]["x"].get<double>() + d["rect"]["w"].get<double>() / 2 == Catch::Approx(pr["x"].get<double>()).margin(1.0));
        REQUIRE(d["rect"]["y"].get<double>() + d["rect"]["h"].get<double>() == Catch::Approx(pr["y"].get<double>() - 6).margin(1.0));
        return d["rect"]["x"].get<double>();
    };
    const double x0 = placed();
    // The player moves: the tag follows on the next frame.
    Json t = s.command("world.get", Json{{"entity", player}, {"component", "Transform"}}).value();
    REQUIRE(s.command("world.set", Json{{"entity", player}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", t["position"]["x"].get<double>() + 1.0}, {"y", t["position"]["y"]}, {"z", t["position"]["z"]}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(placed() > x0 + 5);
    // The snapshot names the anchor; a hit at the tag finds it.
    Json snap = s.command("ui.snapshot", Json{{"root", 61}}).value();
    REQUIRE(snap["text"].get<std::string>().find("anchor=") != std::string::npos);
    Json d = s.command("ui.describe", Json{{"id", 61}}).value();
    Json hit = s.command("ui.hit", Json{{"x", d["rect"]["x"].get<double>() + 2}, {"y", d["rect"]["y"].get<double>() + 2}}).value();
    REQUIRE(hit["id"] == 61);
    // Anchored to nothing that exists, it hides; back on the player, it shows again.
    REQUIRE(s.command("ui.apply", Json{{"ops", Json::array({Json::array({"set", 61, Json{{"anchor", 987654321}}})})}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("ui.describe", Json{{"id", 61}}).value()["visible"] == false);
    REQUIRE(s.command("ui.hit", Json{{"x", d["rect"]["x"].get<double>() + 2}, {"y", d["rect"]["y"].get<double>() + 2}}).value()["id"] != 61);
    REQUIRE(s.command("ui.apply", Json{{"ops", Json::array({Json::array({"set", 61, Json{{"anchor", player}}})})}}).has_value());
    REQUIRE(s.frame().has_value());
    placed();
    REQUIRE(s.finish().has_value());
}

TEST_CASE("project.brief says what the project is made of and what it is doing, as one text", "[runtime][brief]") {
    app::Options o;
    o.project_dir = root() / "samples" / "playground";
    o.bundle = root() / "build" / "ts" / "playground.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 120}}).has_value());
    const std::string t = s.command("project.brief", Json::object()).value()["text"].get<std::string>();
    INFO(t);
    for (std::string_view want : {"project playground", "scripts: scripts/main.ts", "scenarios: scenarios/crowd.ts", "roots: Level (+", "components in use: Transform", "project component Enemy {damage f32, kind i32: grunt|brute", "exposed state: enemies=", "events: ", "enemy.spawned x", "lint: 0 errors", "next: "}) {
        REQUIRE(t.find(want) != std::string::npos);
    }
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a clip's cues are announced as it passes them, every loop, either way", "[runtime][animation][cues]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The arm waves (a one-second loop): a cue half way, and one for a clip it is not playing.
    const Json cues = Json::array({Json{{"time", 0.5}, {"name", "mid"}}, Json{{"clip", "nod"}, {"time", 0.2}, {"name", "other"}}});
    REQUIRE(s.command("world.set", Json{{"entity", "Arm"}, {"component", "Animator"}, {"value", Json{{"cues", cues}, {"time", 0}}}}).has_value());
    const auto since = s.command("events.last_seq", Json::object()).value();
    Json first = s.command("step", Json{{"ticks", 60}, {"until", Json{{"event", "animation.cue"}}}}).value();
    INFO(first.dump());
    REQUIRE(first["until"]["met"] == true);
    REQUIRE(first["until"]["event"]["data"]["name"] == "mid");
    REQUIRE(first["until"]["event"]["data"]["clip"] == "wave");
    REQUIRE(first["until"]["ticks"].get<int>() >= 29);
    REQUIRE(first["until"]["ticks"].get<int>() <= 31);
    REQUIRE(s.command("step", Json{{"ticks", 125}}).has_value());   // two more loops (and a little: sixtieths add up short of 2.5)
    const Json all = s.command("events.since", Json{{"seq", since["seq"]}, {"type", "animation.cue"}}).value();
    REQUIRE(all["events"].size() == 3);
    for (const Json& e : all["events"]) REQUIRE(e["data"]["name"] == "mid");
    // Played backwards, the cue is passed going back.
    REQUIRE(s.command("world.set", Json{{"entity", "Arm"}, {"component", "Animator"}, {"value", Json{{"speed", -1}}}}).has_value());
    Json back = s.command("step", Json{{"ticks", 70}, {"until", Json{{"event", "animation.cue"}}}}).value();
    REQUIRE(back["until"]["met"] == true);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("an attached entity follows a joint of an animated model", "[runtime][animation][attach]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json parts;
    parts["Transform"] = Json{{"scale", Json{{"x", 0.1}, {"y", 0.6}, {"z", 0.1}}}};
    parts["MeshRenderer"] = Json{{"mesh", "cube"}};
    parts["Attach"] = Json{{"target", "Arm"}, {"joint", "tip"}, {"offset", Json{{"x", 0}, {"y", 0.5}, {"z", 0}}}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Sword"}, {"components", parts}}).has_value());
    for (int round = 0; round < 3; ++round) {
        REQUIRE(s.command("step", Json{{"ticks", 17}}).has_value());
        const Json pose = s.command("animation.pose", Json{{"entity", "Arm"}}).value();
        Json tip;
        for (const Json& j : pose["joints"]) if (j["name"] == "tip") tip = j;
        REQUIRE(tip.is_object());
        const Json sword = s.command("world.get", Json{{"entity", "Sword"}, {"component", "WorldTransform"}}).value();
        INFO(tip.dump() << " " << sword.dump());
        // Half a unit along the joint's own Y from the joint (the arm is at unit scale).
        for (const char* k : {"x", "y", "z"}) {
            REQUIRE(sword["position"][k].get<double>() == Catch::Approx(tip["position"][k].get<double>() + 0.5 * tip["axis_y"][k].get<double>()).margin(1e-3));
        }
        REQUIRE(sword["scale"]["y"].get<double>() == Catch::Approx(0.6));   // its own scale stays
    }
    REQUIRE(s.command("world.get", Json{{"entity", "Sword"}, {"component", "Attach"}}).value()["found"] == true);
    REQUIRE(s.command("world.set", Json{{"entity", "Sword"}, {"component", "Attach"}, {"value", Json{{"joint", "elbow"}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Sword"}, {"component", "Attach"}}).value()["found"] == false);   // no such joint
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a step watches values for one who would otherwise step a tick at a time", "[runtime][step][watch]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    // A jump: the player's height rises and falls back, its top and the tick of it in one answer.
    REQUIRE(s.command("input.press", Json{{"action", "jump"}}).has_value());
    Json a = s.command("step", Json{{"ticks", 90}, {"watch", Json::array({"Player:Transform.position.y", "score", Json{{"entity", "Player"}, {"component", "Body2D"}, {"field", "grounded"}}})}, {"every", 30}, {"keys", Json::array({"score", "nothing"})}}).value();
    INFO(a.dump());
    const Json& y = a["watch"]["Player:Transform.position.y"];
    REQUIRE(y["max"].get<double>() > y["first"].get<double>() + 1.5);
    REQUIRE(y["max_tick"].get<int>() > 30);
    REQUIRE(y["max_tick"].get<int>() < a["tick"].get<int>() - 1);
    REQUIRE(y["last"].get<double>() == Catch::Approx(y["first"].get<double>()).margin(0.05));
    REQUIRE(y["changes"].get<int>() > 10);
    REQUIRE(y["series"].size() == 3);
    REQUIRE(a["watch"]["score"]["changes"] == 0);
    const Json& grounded = a["watch"][Json{{"entity", "Player"}, {"component", "Body2D"}, {"field", "grounded"}}.dump()];
    REQUIRE(grounded["changes"].get<int>() >= 2);   // left the ground and came back
    REQUIRE_FALSE(grounded.contains("min"));
    // keys: only the values asked for, and the names that are not exposed.
    REQUIRE(a["state"].size() == 1);
    REQUIRE(a["missing"] == Json::array({"nothing"}));
    REQUIRE_FALSE(a.contains("world_hash"));
    REQUIRE(s.command("state", Json{{"keys", Json::array({"player.x"})}}).value()["state"].contains("player.x"));
    REQUIRE_FALSE(s.command("step", Json{{"watch", "Nobody:Transform.position.y"}}).has_value());
    REQUIRE_FALSE(s.command("step", Json{{"watch", "Player:Nothing.x"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a step runs until an event, a state value or a field says so", "[runtime][step][until]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // Walking right collects the coin ahead within a second (the sprites scenario): one call.
    REQUIRE(s.command("input.hold", Json{{"action", "move_x"}, {"ticks", 600}}).has_value());
    Json a = s.command("step", Json{{"ticks", 600}, {"until", Json{{"event", "coin."}}}}).value();
    INFO(a.dump());
    REQUIRE(a["until"]["met"] == true);
    REQUIRE(a["until"]["ticks"].get<int>() < 60);
    REQUIRE(a["until"]["event"]["type"] == "coin.collected");
    REQUIRE(a["state"]["score"] == 1);
    REQUIRE(a["until"]["tick"] == a["tick"].get<int>() - 1);
    // An exposed value, and a field of a component.
    Json b = s.command("step", Json{{"ticks", 600}, {"until", Json{{"state", "player.x"}, {"above", 2}}}}).value();
    REQUIRE(b["until"]["met"] == true);
    REQUIRE(b["until"]["value"].get<double>() > 2);
    Json c = s.command("step", Json{{"ticks", 600}, {"until", Json{{"entity", "Player"}, {"component", "Transform"}, {"field", "position.x"}, {"at_least", 3}}}}).value();
    REQUIRE(c["until"]["met"] == true);
    REQUIRE(c["until"]["value"].get<double>() >= 3);
    // Not met: the whole step runs and says so.
    Json d = s.command("step", Json{{"ticks", 20}, {"until", Json{{"state", "score"}, {"equals", 99}}}}).value();
    REQUIRE(d["until"]["met"] == false);
    REQUIRE(d["until"]["ticks"] == 20);
    REQUIRE_FALSE(s.command("step", Json{{"ticks", 5}, {"until", Json{{"state", "score"}}}}).has_value());   // no comparison
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a click on the interface is the interface's; elsewhere it presses the action bound to the button", "[runtime][input][mouse]") {
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
    REQUIRE(s.command("input.map", Json{{"actions", Json{{"fire", Json::array({"mouse:left"})}}}}).has_value());
    Json ops = Json::array();
    ops.push_back(Json::array({"create", 71, "box"}));
    ops.push_back(Json::array({"set", 71, Json{{"position", "absolute"}, {"left", 10}, {"top", 10}, {"width", 100}, {"height", 40}, {"name", "menu"}, {"on", Json::array({"click"})}}}));
    ops.push_back(Json::array({"append", 1, 71}));
    REQUIRE(s.command("ui.apply", Json{{"ops", ops}}).has_value());
    REQUIRE(s.frame().has_value());
    auto fire = [&] { return s.command("input.actions", Json::object()).value()["fire"]; };
    // On the button: a click for the interface, and the action stays at rest.
    Json on = s.command("ui.click", Json{{"x", 60}, {"y", 30}}).value();
    INFO(on.dump());
    REQUIRE(on["input"][1]["ui"] == 71);
    REQUIRE(fire()["pressed"] == false);
    // Off it: the game's click, pressed (and released) for the next tick to see.
    REQUIRE(s.command("ui.click", Json{{"x", 250}, {"y", 120}}).has_value());
    REQUIRE(fire()["pressed"] == true);
    REQUIRE(s.frame().has_value());
    REQUIRE(fire()["pressed"] == false);
    // An agent presses the action by name; the press is the button's, away from the interface.
    Json held = s.command("input.hold", Json{{"action", "fire"}, {"ticks", 3}}).value();
    REQUIRE(held["keys"] == Json::array({"mouse:left"}));
    REQUIRE(fire()["down"] == true);
    for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(fire()["down"] == false);
    // The cursor: asked for and remembered headless, never held without a window.
    Json c = s.command("input.cursor", Json{{"locked", true}}).value();
    REQUIRE(c["locked"] == true);
    REQUIRE(c["held"] == false);
    REQUIRE(s.command("input.state", Json::object()).value()["cursor"]["locked"] == true);
    REQUIRE_FALSE(s.command("input.cursor", Json{{"locked", "yes"}}).has_value());
    REQUIRE(s.command("input.cursor", Json{{"visible", false}}).value() == Json{{"locked", true}, {"held", false}, {"visible", false}});
    REQUIRE(s.finish().has_value());
}

TEST_CASE("rumble answers false without a pad and plays patterns on the tick clock", "[runtime][input][rumble]") {
    app::Options o;
    o.project_dir = root() / "samples" / "hello";
    o.bundle = root() / "build" / "ts" / "hello.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 100;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json r = s.command("input.rumble", Json{{"pad", 0}, {"low", 1.0}, {"high", 0.5}, {"ms", 100}}).value();
    REQUIRE(r["rumbled"] == false);
    // A pattern plays its steps on the tick clock, each an input.rumble event, and ends.
    REQUIRE(s.frame().has_value());
    const std::int64_t seq = s.command("events.last_seq", Json::object()).value()["seq"].get<std::int64_t>();
    r = s.command("input.rumble", Json{{"pad", 1}, {"pattern", Json::array({Json{{"low", 1}, {"high", 0}, {"ms", 50}}, Json{{"low", 0}, {"high", 0}, {"ms", 100}}, Json{{"low", 0}, {"high", 1}, {"ms", 50}}})}, {"repeat", 2}}).value();
    INFO(r.dump());
    REQUIRE(r["steps"] == 3);
    REQUIRE(r["seconds"].get<double>() == Catch::Approx(0.4));
    REQUIRE(s.command("input.state", Json::object()).value()["rumble"]["1"]["steps"] == 3);
    auto rumbles = [&]() {
        Json out = Json::array();
        for (const Json& e : s.command("events.since", Json{{"seq", seq}, {"limit", 50}}).value()["events"]) if (e["type"] == "input.rumble") out.push_back(e);
        return out;
    };
    REQUIRE(rumbles().size() == 1);   // the first step starts at once
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    Json played = rumbles();
    INFO(played.dump());
    REQUIRE(played.size() == 6);      // three steps twice, 0.4 s at 60 ticks a second
    REQUIRE(played[1]["data"]["low"] == 0);
    REQUIRE(played[2]["data"]["high"] == 1);
    REQUIRE(played[3]["data"]["step"] == 0);
    REQUIRE(played[1]["tick"].get<std::int64_t>() - played[0]["tick"].get<std::int64_t>() == 3);   // 50 ms is three ticks
    REQUIRE(played[2]["tick"].get<std::int64_t>() - played[1]["tick"].get<std::int64_t>() == 6);
    REQUIRE(s.command("input.state", Json::object()).value()["rumble"].empty());
    // Stopping ends a pattern before its steps are through.
    REQUIRE(s.command("input.rumble", Json{{"pad", 0}, {"pattern", Json::array({Json{{"ms", 1000}}, Json{{"ms", 1000}}})}}).value()["steps"] == 2);
    REQUIRE(s.command("input.rumble", Json{{"pad", 0}, {"stop", true}}).value()["stopped"] == true);
    REQUIRE(s.command("input.state", Json::object()).value()["rumble"].empty());
    REQUIRE_FALSE(s.command("input.rumble", Json{{"pattern", Json::array()}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a platformer body lands on a tile's collision shape rather than its cell", "[runtime][tilemap][shapes]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    // A map beside the sample's assets, removed after: a ground row of solid tiles and, over it,
    // one tile whose collision shape is its bottom half.
    const std::filesystem::path file = o.project_dir / "assets" / "shapes-test.tmj";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    {
        std::ofstream out(file);
        out << R"({"type":"map","orientation":"orthogonal","renderorder":"right-down","width":6,"height":4,"tilewidth":16,"tileheight":16,"infinite":false,"nextlayerid":2,"nextobjectid":1,
            "tilesets":[{"firstgid":1,"name":"tiles","image":"tiles.png","imagewidth":112,"imageheight":16,"tilewidth":16,"tileheight":16,"columns":7,"tilecount":7,"spacing":0,"margin":0,
                "tiles":[{"id":0,"objectgroup":{"type":"objectgroup","objects":[{"id":1,"x":0,"y":8,"width":16,"height":8}]}},{"id":1,"properties":[{"name":"solid","type":"bool","value":true}]}]}],
            "layers":[{"id":1,"type":"tilelayer","name":"ground","width":6,"height":4,"x":0,"y":0,"opacity":1,"visible":true,"data":[0,0,0,0,0,0, 0,0,0,0,0,0, 0,0,1,0,0,0, 2,2,2,2,2,2]}]})";
    }
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The map far from the sample's level (whose script keeps running), at x = 100.
    Json map = s.command("world.spawn", Json{{"name", "Shapes"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 100}, {"y", 0}, {"z", 0}}}}}, {"TileMap", Json{{"map", "assets/shapes-test.tmj"}, {"tile_size", 1.0}}}}}}).value();
    REQUIRE(s.command("world.update_transforms", Json::object()).has_value());
    // Two small bodies: one over the half block, one over open ground.
    REQUIRE(s.command("world.spawn", Json{{"name", "OverBlock"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 102.5}, {"y", -1.5}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.2}, {"y", 0.2}}}, {"map", "Shapes"}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "OverGround"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 100.5}, {"y", -1.5}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.2}, {"y", 0.2}}}, {"map", "Shapes"}, {"step", 0.1}}}}}}).has_value());   // a low step, so the half block is a wall to it
    for (int i = 0; i < 90; ++i) REQUIRE(s.frame().has_value());
    Json block = s.command("world.get", Json{{"entity", "OverBlock"}, {"component", "Transform"}}).value();
    Json ground = s.command("world.get", Json{{"entity", "OverGround"}, {"component", "Transform"}}).value();
    INFO(block.dump() << " " << ground.dump());
    // Cell (2, 2) spans y -3..-2; its block is the bottom half, so the body rests at -2.5 + 0.2.
    REQUIRE(block["position"]["y"].get<double>() == Catch::Approx(-2.3).margin(0.01));
    REQUIRE(s.command("world.get", Json{{"entity", "OverBlock"}, {"component", "Body2D"}}).value()["grounded"] == true);
    // The ground row's top is -3: the other body rests at -2.8.
    REQUIRE(ground["position"]["y"].get<double>() == Catch::Approx(-2.8).margin(0.01));
    // The cell is solid as a whole, but the point in its top half is not inside the shape.
    Json above = s.command("tilemap.solid", Json{{"entity", map["id"]}, {"x", 102.5}, {"y", -2.25}}).value();
    Json within = s.command("tilemap.solid", Json{{"entity", map["id"]}, {"x", 102.5}, {"y", -2.75}}).value();
    INFO(above.dump() << " " << within.dump());
    REQUIRE(above["solid"] == true);
    REQUIRE(above["inside"] == false);
    REQUIRE(within["inside"] == true);
    Json tile = s.command("tilemap.tile", Json{{"entity", map["id"]}, {"tile_x", 2}, {"tile_y", 2}}).value();
    INFO(tile.dump());
    REQUIRE(tile["layers"][0]["shapes"] == Json::array({Json::array({0.0, 0.5, 1.0, 1.0})}));
    // Walking into the block from the ground is stopped by its side; the body stands beside it.
    REQUIRE(s.command("world.set", Json{{"entity", "OverGround"}, {"component", "Body2D"}, {"value", Json{{"velocity", Json{{"x", 3.0}, {"y", 0.0}}}}}}).has_value());
    for (int i = 0; i < 60; ++i) {
        REQUIRE(s.command("world.set", Json{{"entity", "OverGround"}, {"component", "Body2D"}, {"value", Json{{"velocity", Json{{"x", 3.0}, {"y", s.command("world.get", Json{{"entity", "OverGround"}, {"component", "Body2D"}}).value()["velocity"]["y"]}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
    }
    Json walked = s.command("world.get", Json{{"entity", "OverGround"}, {"component", "Transform"}}).value();
    INFO(walked.dump());
    REQUIRE(walked["position"]["x"].get<double>() == Catch::Approx(102.0 - 0.2).margin(0.02));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("y-sorted sprites draw what is lower on the screen on top", "[runtime][sprites][sorty]") {
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
    const auto cam = s.command("render.stats", Json::object()).value()["camera"].get<std::uint64_t>();
    const Json ct = s.command("world.get", Json{{"entity", cam}, {"component", "WorldTransform"}}).value();
    const double cx = ct["position"]["x"].get<double>(), cy = ct["position"]["y"].get<double>();
    // A red square half a unit lower than a blue one, both on layer 9 and y-sorted: red, lower on
    // the screen, is drawn later where they overlap, although it was spawned first.
    auto square = [&](const char* name, double y, double r, double g, double b) {
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", cx}, {"y", y}, {"z", 0.0}}}}}, {"Sprite", Json{{"size", Json{{"x", 1.0}, {"y", 1.0}}}, {"layer", 9}, {"sort_y", true}, {"color", Json{{"r", r}, {"g", g}, {"b", b}, {"a", 1.0}}}}}}}}).has_value());
    };
    square("Low", cy - 0.5, 1, 0, 0);
    square("High", cy, 0, 0, 1);
    REQUIRE(s.frame().has_value());
    Json pr = s.command("render.project", Json{{"point", Json{{"x", cx}, {"y", cy - 0.25}, {"z", 0.0}}}}).value();
    REQUIRE(pr["visible"] == true);
    const Json at = Json{{"x", pr["x"]}, {"y", pr["y"]}};
    Json sorted = s.command("capture", Json{{"pixel", at}}).value();
    INFO("y-sorted " << sorted["pixel"].dump());
    REQUIRE(sorted["pixel"][0].get<int>() > 150);
    REQUIRE(sorted["pixel"][2].get<int>() < 80);
    // Sorted by distance again (both on the same plane, so spawn order): blue, spawned later, wins.
    for (const char* name : {"Low", "High"}) REQUIRE(s.command("world.set", Json{{"entity", name}, {"component", "Sprite"}, {"value", Json{{"sort_y", false}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json plain = s.command("capture", Json{{"pixel", at}}).value();
    INFO("plain " << plain["pixel"].dump());
    REQUIRE(plain["pixel"][2].get<int>() > 150);
    REQUIRE(plain["pixel"][0].get<int>() < 80);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("tilemap.spawn puts prefabs at a map's objects with their properties applied", "[runtime][tilemap][spawn]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    // A prefab and a map beside the sample's files, removed after.
    const std::filesystem::path prefab_dir = o.project_dir / "prefabs";
    const bool had_prefabs = std::filesystem::exists(prefab_dir);
    const std::filesystem::path prefab = prefab_dir / "marker-test.json";
    const std::filesystem::path mapfile = o.project_dir / "assets" / "spawn-test.tmj";
    struct Cleanup {
        std::filesystem::path prefab, dir, map;
        bool keep_dir;
        ~Cleanup() { std::filesystem::remove(prefab); std::filesystem::remove(map); if (!keep_dir) std::filesystem::remove(dir); }
    } cleanup{prefab, prefab_dir, mapfile, had_prefabs};
    std::filesystem::create_directories(prefab_dir);
    {
        std::ofstream out(prefab);
        out << R"({"format":"pocket-scene","entities":[{"name":"Marker","components":{"Transform":{},"Sprite":{"size":{"x":0.5,"y":0.5},"layer":3}}}]})";
        std::ofstream map(mapfile);
        map << R"({"type":"map","orientation":"orthogonal","renderorder":"right-down","width":4,"height":4,"tilewidth":16,"tileheight":16,"infinite":false,"nextlayerid":2,"nextobjectid":3,"tilesets":[],
            "layers":[{"id":1,"type":"objectgroup","name":"things","objects":[
                {"id":1,"name":"hero","type":"marker","point":true,"x":16,"y":16,"width":0,"height":0,"properties":[{"name":"Sprite.layer","type":"int","value":7},{"name":"note","type":"string","value":"kept aside"}]},
                {"id":2,"name":"box","type":"marker","x":32,"y":32,"width":16,"height":16},
                {"id":3,"name":"other","type":"decor","point":true,"x":48,"y":48,"width":0,"height":0}]}]})";
    }
    app::Session s(o);
    REQUIRE(s.start().has_value());
    Json map = s.command("world.spawn", Json{{"name", "Things"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 100}, {"y", 0}, {"z", 0}}}}}, {"TileMap", Json{{"map", "assets/spawn-test.tmj"}, {"tile_size", 1.0}}}}}}).value();
    REQUIRE(s.command("world.update_transforms", Json::object()).has_value());
    const auto before = s.command("world.summary", Json::object()).value()["entities"].get<int>();
    Json r = s.command("tilemap.spawn", Json{{"entity", map["id"]}, {"layer", "things"}, {"prefabs", Json{{"marker", "prefabs/marker-test.json"}}}}).value();
    INFO(r.dump());
    REQUIRE(r["count"] == 2);   // the decor object is not listed
    REQUIRE(s.command("world.summary", Json::object()).value()["entities"].get<int>() == before + 2);
    // Named after the objects, at a point's spot and a rectangle's center, the dotted property applied.
    Json hero = s.command("world.get", Json{{"entity", "hero"}, {"component", "Transform"}}).value();
    Json box = s.command("world.get", Json{{"entity", "box"}, {"component", "Transform"}}).value();
    INFO(hero.dump() << " " << box.dump());
    REQUIRE(hero["position"]["x"].get<double>() == Catch::Approx(101.0));
    REQUIRE(hero["position"]["y"].get<double>() == Catch::Approx(-1.0));
    REQUIRE(box["position"]["x"].get<double>() == Catch::Approx(102.5));
    REQUIRE(box["position"]["y"].get<double>() == Catch::Approx(-2.5));
    REQUIRE(s.command("world.get", Json{{"entity", "hero"}, {"component", "Sprite"}}).value()["layer"] == 7);
    REQUIRE(s.command("world.get", Json{{"entity", "box"}, {"component", "Sprite"}}).value()["layer"] == 3);
    REQUIRE(r["spawned"][0]["entity"].get<std::uint64_t>() == s.command("world.find", Json{{"path", "hero"}}).value().get<std::uint64_t>());
    // No prefabs: refused.
    REQUIRE_FALSE(s.command("tilemap.spawn", Json{{"entity", map["id"]}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a wall between the listener and a source turns it down and muffles it", "[runtime][audio][occlusion]") {
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
    // The camera at the origin looking down -Z; the hum six units ahead, seven tenths taken by a wall.
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"rotation", {{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Hum"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", -6}}}}}, {"AudioSource", Json{{"clip", "assets/hum.wav"}, {"autoplay", true}, {"loop", true}, {"spatial", true}, {"near", 1.0}, {"range", 20.0}, {"occlusion", 0.7}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    auto voice = [&]() {
        for (const Json& v : s.command("audio.list", Json::object()).value()) if (v["clip"] == "assets/hum.wav" && v["entity"] != 0) return v;
        return Json(nullptr);
    };
    auto source = [&]() { return s.command("world.get", Json{{"entity", "Hum"}, {"component", "AudioSource"}}).value(); };
    const double open = (20.0 - 6.0) / 19.0;
    Json v = voice();
    INFO(v.dump());
    REQUIRE(!v.is_null());
    REQUIRE(v["volume"].get<double>() == Catch::Approx(open).margin(0.02));
    REQUIRE(v["lowpass"].get<double>() == Catch::Approx(1.0).margin(0.01));
    REQUIRE(source()["occluded"] == false);
    // A static wall across the line: three tenths of the volume and of the high end, and the event says by what.
    REQUIRE(s.command("world.spawn", Json{{"name", "Wall"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", -3}}}}}, {"RigidBody", Json{{"kind", 1}}}, {"Collider", Json{{"shape", 0}, {"size", {{"x", 2}, {"y", 2}, {"z", 0.25}}}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    v = voice();
    INFO(v.dump());
    REQUIRE(v["volume"].get<double>() == Catch::Approx(open * 0.3).margin(0.02));
    REQUIRE(v["lowpass"].get<double>() == Catch::Approx(0.3).margin(0.01));
    REQUIRE(source()["occluded"] == true);
    Json ev = s.command("events.recent", Json{{"limit", 20}, {"type", "audio.occluded"}}).value();
    INFO(ev.dump());
    REQUIRE(ev.size() >= 1);
    REQUIRE(ev.back()["data"]["blocked"] == true);
    REQUIRE(ev.back()["data"]["by"] == "/Wall");
    // A trigger does not block.
    REQUIRE(s.command("world.set", Json{{"entity", "Wall"}, {"component", "Collider"}, {"value", Json{{"is_trigger", true}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    v = voice();
    REQUIRE(v["volume"].get<double>() == Catch::Approx(open).margin(0.02));
    REQUIRE(v["lowpass"].get<double>() == Catch::Approx(1.0).margin(0.01));
    REQUIRE(source()["occluded"] == false);
    ev = s.command("events.recent", Json{{"limit", 20}, {"type", "audio.occluded"}}).value();
    REQUIRE(ev.back()["data"]["blocked"] == false);
    // Solid again but moved aside: open.
    REQUIRE(s.command("world.set", Json{{"entity", "Wall"}, {"component", "Collider"}, {"value", Json{{"is_trigger", false}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Wall"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 10}, {"y", 0}, {"z", -3}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(voice()["volume"].get<double>() == Catch::Approx(open).margin(0.02));
    REQUIRE(source()["occluded"] == false);
    // A one-shot with its own occlusion and low-pass, behind the wall put back: half of each.
    REQUIRE(s.command("world.set", Json{{"entity", "Wall"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0}, {"z", -3}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json shot = s.command("audio.play", Json{{"clip", "assets/beep.wav"}, {"entity", "Hum"}, {"spatial", true}, {"occlusion", 0.5}, {"lowpass", 0.8}}).value();
    REQUIRE(s.frame().has_value());
    Json bv(nullptr);
    for (const Json& x : s.command("audio.list", Json::object()).value()) if (x["id"] == shot["voice"]) bv = x;
    INFO(bv.dump());
    REQUIRE(!bv.is_null());
    REQUIRE(bv["volume"].get<double>() == Catch::Approx(open * 0.5).margin(0.02));
    REQUIRE(bv["lowpass"].get<double>() == Catch::Approx(0.4).margin(0.01));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("the time scale slows, speeds and stops the simulation against the frames, and a step is exact", "[runtime][time]") {
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
    const std::int64_t t0 = s.tick();
    Json st = s.command("time.scale", Json::object()).value();
    REQUIRE(st["scale"].get<double>() == Catch::Approx(1.0));
    REQUIRE(st["seconds"].get<double>() == Catch::Approx(0.0));
    // Half speed: a tick every other frame.
    REQUIRE(s.command("time.scale", Json{{"scale", 0.5}}).value()["scale"].get<double>() == Catch::Approx(0.5));
    for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.tick() == t0 + 2);
    // Double speed: two ticks a frame.
    REQUIRE(s.command("time.scale", Json{{"scale", 2}}).has_value());
    for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.tick() == t0 + 10);
    // A step is its ticks whatever the scale.
    Json stepped = s.command("step", Json{{"ticks", 3}}).value();
    REQUIRE(s.tick() == t0 + 13);
    REQUIRE(stepped["tick"].get<std::int64_t>() == t0 + 13);
    // A hit-stop: no tick for a twentieth of a second (three frames), then time runs by itself.
    Json stop = s.command("time.scale", Json{{"scale", 0}, {"seconds", 0.05}}).value();
    REQUIRE(stop["scale"].get<double>() == Catch::Approx(0.0));
    REQUIRE(stop["seconds"].get<double>() == Catch::Approx(0.05));
    const std::int64_t t1 = s.tick();
    for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.tick() == t1);
    REQUIRE(s.command("time.scale", Json::object()).value()["scale"].get<double>() == Catch::Approx(1.0));
    for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.tick() == t1 + 2);
    // Out of range is clamped; a non-number is refused.
    REQUIRE(s.command("time.scale", Json{{"scale", 50}}).value()["scale"].get<double>() == Catch::Approx(8.0));
    REQUIRE(s.command("time.scale", Json{{"scale", "fast"}}).error().code == "bad_args");
    REQUIRE(s.command("time.scale", Json{{"scale", 1}}).value()["seconds"].get<double>() == Catch::Approx(0.0));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("fingers make taps, double taps, long presses, swipes and pinches", "[runtime][input][gesture]") {
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
    REQUIRE(s.frame().has_value());
    auto gestures = [](const Json& reply) {
        Json out = Json::array();
        for (const Json& e : reply["input"]) if (e["type"] == "gesture") out.push_back(e);
        return out;
    };
    auto touch = [&](int finger, const char* phase, double x, double y) {
        Json r = s.command("input.touch", Json{{"finger", finger}, {"x", x}, {"y", y}, {"phase", phase}}).value();
        return gestures(r);
    };
    auto last_event = [&]() {
        Json ev = s.command("events.recent", Json{{"limit", 5}, {"type", "input.gesture"}}).value();
        return ev.empty() ? Json(nullptr) : ev.back()["data"];
    };
    REQUIRE(s.command("input.state", Json::object()).value()["gestures"]["enabled"] == true);
    // A tap: down and up a few ticks apart, barely moving.
    REQUIRE(touch(0, "down", 100, 100).empty());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    Json g = touch(0, "up", 103, 101);
    INFO(g.dump());
    REQUIRE(g.size() == 1);
    REQUIRE(g[0]["gesture"] == "tap");
    REQUIRE(g[0]["count"] == 1);
    REQUIRE(g[0]["finger"] == 0);
    REQUIRE(g[0]["x"].get<double>() == Catch::Approx(103));
    // Another right away in the same place: a double tap, and the world event says so.
    for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(touch(0, "down", 101, 100).empty());
    REQUIRE(s.frame().has_value());
    g = touch(0, "up", 101, 100);
    REQUIRE(g.size() == 1);
    REQUIRE(g[0]["count"] == 2);
    REQUIRE(last_event()["gesture"] == "tap");
    REQUIRE(last_event()["count"] == 2);
    // A long press: held still for half a second is reported while down; lifting it is no tap.
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(touch(0, "down", 50, 50).empty());
    for (int i = 0; i < 31; ++i) REQUIRE(s.frame().has_value());
    Json held = last_event();
    INFO(held.dump());
    REQUIRE(held["gesture"] == "long_press");
    REQUIRE(held["x"].get<double>() == Catch::Approx(50));
    REQUIRE(touch(0, "up", 51, 50).empty());
    // A swipe: a finger that travels right and lifts within a few frames.
    REQUIRE(touch(0, "down", 100, 100).empty());
    REQUIRE(s.frame().has_value());
    REQUIRE(touch(0, "move", 140, 100).empty());
    REQUIRE(s.frame().has_value());
    REQUIRE(touch(0, "move", 180, 102).empty());
    REQUIRE(s.frame().has_value());
    g = touch(0, "up", 200, 103);
    INFO(g.dump());
    REQUIRE(g.size() == 1);
    REQUIRE(g[0]["gesture"] == "swipe");
    REQUIRE(g[0]["direction"] == "right");
    REQUIRE(g[0]["dx"].get<double>() == Catch::Approx(100));
    REQUIRE(g[0]["seconds"].get<double>() == Catch::Approx(3.0 / 60.0).margin(0.001));
    REQUIRE_FALSE(g[0].contains("edge"));
    // From the right side of the view inwards: an edge swipe.
    REQUIRE(touch(0, "down", 315, 90).empty());
    REQUIRE(s.frame().has_value());
    g = touch(0, "up", 250, 92);
    INFO(g.dump());
    REQUIRE(g.size() == 1);
    REQUIRE(g[0]["direction"] == "left");
    REQUIRE(g[0]["edge"] == "right");
    REQUIRE(s.command("input.state", Json::object()).value()["gestures"]["edge"].get<double>() == Catch::Approx(24));
    // A pinch: two fingers; the second moving away doubles the spread; neither lifts as a tap.
    REQUIRE(touch(0, "down", 100, 100).empty());
    g = touch(1, "down", 200, 100);
    INFO(g.dump());
    REQUIRE(g.size() == 1);
    REQUIRE(g[0]["gesture"] == "pinch");
    REQUIRE(g[0]["phase"] == "begin");
    REQUIRE(g[0]["scale"].get<double>() == Catch::Approx(1.0));
    REQUIRE(g[0]["x"].get<double>() == Catch::Approx(150));
    g = touch(1, "move", 300, 100);
    REQUIRE(g.size() == 1);
    REQUIRE(g[0]["phase"] == "move");
    REQUIRE(g[0]["scale"].get<double>() == Catch::Approx(2.0));
    REQUIRE(g[0]["rotation"].get<double>() == Catch::Approx(0.0).margin(0.01));
    REQUIRE(g[0]["x"].get<double>() == Catch::Approx(200));
    g = touch(1, "move", 100, 200);   // straight below the first finger: a quarter turn
    REQUIRE(g[0]["rotation"].get<double>() == Catch::Approx(90.0).margin(0.01));
    g = touch(1, "up", 100, 200);
    REQUIRE(g.size() == 1);
    REQUIRE(g[0]["phase"] == "end");
    REQUIRE(touch(0, "up", 100, 100).empty());
    // A drift past the slop that is too short for a swipe is nothing.
    REQUIRE(touch(0, "down", 10, 10).empty());
    REQUIRE(s.frame().has_value());
    REQUIRE(touch(0, "move", 40, 10).empty());
    REQUIRE(s.frame().has_value());
    REQUIRE(touch(0, "up", 40, 10).empty());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("several cameras each draw their part of the window: split screen and a minimap", "[runtime][cameras][views]") {
    app::Session s(hello_options(100000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    // Two blocks far apart, a red and a blue, and a camera in front of each, the left half and the right.
    auto block = [&](const std::string& name, float x, Json color) {
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", 1}, {"z", -40}}}, {"scale", Json{{"x", 3}, {"y", 3}, {"z", 1}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", color}}}}}}).has_value());
    };
    block("RedBlock", -50, Json{{"r", 1}, {"g", 0.05}, {"b", 0.05}, {"a", 1}});
    block("BlueBlock", 50, Json{{"r", 0.05}, {"g", 0.05}, {"b", 1}, {"a", 1}});
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Camera"}, {"value", Json{{"active", false}}}}).has_value());
    auto camera = [&](const std::string& name, float x, Json viewport, int order) {
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", 1}, {"z", -34}}}}}, {"Camera", Json{{"viewport", viewport}, {"order", order}}}}}}).has_value());
    };
    camera("Left", -50, Json{{"x", 0}, {"y", 0}, {"z", 0.5}, {"w", 1}}, 0);
    camera("Right", 50, Json{{"x", 0.5}, {"y", 0}, {"z", 0.5}, {"w", 1}}, 0);
    auto at = [&](int x, int y) {
        return s.command("capture", Json{{"pixel", Json{{"x", x}, {"y", y}}}}).value()["pixel"];
    };
    REQUIRE(s.frame().has_value());
    const Json w = s.command("window.info", Json::object()).value();
    const int W = w["pixel_width"].get<int>(), H = w["pixel_height"].get<int>();
    const Json left = at(W / 4, H / 2), right = at(3 * W / 4, H / 2);
    INFO("left " << left.dump() << " right " << right.dump());
    REQUIRE(left[0].get<int>() > left[2].get<int>() + 60);    // red in the left half
    REQUIRE(right[2].get<int>() > right[0].get<int>() + 60);  // blue in the right half
    // A minimap over the left half's corner, drawn after it by its order, sees the blue block.
    camera("Minimap", 50, Json{{"x", 0.02}, {"y", 0.02}, {"z", 0.2}, {"w", 0.3}}, 1);
    REQUIRE(s.frame().has_value());
    const Json corner = at(static_cast<int>(W * 0.12), static_cast<int>(H * 0.17));
    INFO("corner " << corner.dump());
    REQUIRE(corner[2].get<int>() > corner[0].get<int>() + 60);
    REQUIRE(at(W / 4, H / 2)[0].get<int>() > 150);   // the left half still red where the minimap is not
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a camera draws into a texture that a sprite in the world and the interface show", "[runtime][cameras][views][texture]") {
    app::Session s(hello_options(100000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("render.taa", Json{{"enabled", false}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    // Far off, a red block filling the view of a camera in front of it, which draws into "spy".
    REQUIRE(s.command("world.spawn", Json{{"name", "Red"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 80}, {"y", 1}, {"z", -40}}}, {"scale", Json{{"x", 8}, {"y", 8}, {"z", 1}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", "#ff1010"}, {"unlit", true}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Spy"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 80}, {"y", 1}, {"z", -34}}}}}, {"Camera", Json{{"target", "spy"}, {"target_size", Json{{"x", 64}, {"y", 48}}}}}}}}).has_value());
    // A screen in front of the window's camera shows it; another screen in front of the spy's own
    // camera shows it too, which that camera must not draw into its own picture.
    REQUIRE(s.command("world.spawn", Json{{"name", "Screen"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 2}, {"z", 0}}}}}, {"Sprite", Json{{"texture", "view:spy"}, {"size", Json{{"x", 2}, {"y", 1.5}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Mirror"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 80}, {"y", 1}, {"z", -36}}}}}, {"Sprite", Json{{"texture", "view:spy"}, {"size", Json{{"x", 0.5}, {"y", 0.5}}}}}}}}).has_value());
    auto pixel = [&](const char* entity) {
        REQUIRE(s.frame().has_value());
        const Json at = s.command("world.get", Json{{"entity", entity}, {"component", "Transform"}}).value()["position"];
        const Json r = s.command("render.project", Json{{"point", at}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
    };
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    const Json seen = pixel("Screen");
    INFO("screen " << seen.dump());
    REQUIRE(seen[0].get<int>() > 180);
    REQUIRE(seen[1].get<int>() < 80);
    REQUIRE(seen[2].get<int>() < 80);
    // The window's camera is still the window's: the spy's camera draws only into its texture.
    REQUIRE(s.command("render.stats", Json::object()).value()["camera"] != s.command("world.find", Json{{"path", "Spy"}}).value());
    // A new size makes the texture again; the screen shows it as before.
    REQUIRE(s.command("world.set", Json{{"entity", "Spy"}, {"component", "Camera"}, {"value", Json{{"target_size", Json{{"x", 32}, {"y", 32}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    const Json again = pixel("Screen");
    INFO("again " << again.dump());
    REQUIRE(again[0].get<int>() > 180);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a mesh drawn through a material the project wrote over the lit colour", "[runtime][material]") {
    app::Session s(hello_options(100000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("render.taa", Json{{"enabled", false}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    // The sample's toon material, with params.x 1 laying a flat colour (params.yzw) over it.
    REQUIRE(s.command("world.spawn", Json{{"name", "Toon"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 2}}}, {"scale", Json{{"x", 2}, {"y", 2}, {"z", 0.2}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"material", "materials/toon.wgsl"}, {"material_params", Json{{"x", 1}, {"y", 0}, {"z", 1}, {"w", 0}}}}}}}}).has_value());
    auto pixel = [&]() {
        REQUIRE(s.frame().has_value());
        const Json r = s.command("render.project", Json{{"point", Json{{"x", 0}, {"y", 1}, {"z", 2.1}}}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
    };
    const Json green = pixel();
    INFO("green " << green.dump());
    REQUIRE(green[1].get<int>() > 150);
    REQUIRE(green[0].get<int>() < 60);
    REQUIRE(s.command("world.set", Json{{"entity", "Toon"}, {"component", "MeshRenderer"}, {"value", Json{{"material_params", Json{{"y", 1}, {"z", 0}}}}}}).has_value());
    const Json red = pixel();
    INFO("red " << red.dump());
    REQUIRE(red[0].get<int>() > 150);
    REQUIRE(red[1].get<int>() < 60);
    // With params.x 0 the toon steps show: lit, not flat red.
    REQUIRE(s.command("world.set", Json{{"entity", "Toon"}, {"component", "MeshRenderer"}, {"value", Json{{"material_params", Json{{"x", 0}}}}}}).has_value());
    const Json toon = pixel();
    REQUIRE(std::abs(toon[0].get<int>() - toon[1].get<int>()) < 40);   // the cube's grey, stepped
    // One that is not there is drawn plain and named by the lint.
    REQUIRE(s.command("world.spawn", Json{{"name", "Plain"}, {"components", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"material", "materials/none.wgsl"}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    bool named = false;
    for (const Json& pr : s.command("world.lint", Json::object()).value()["problems"]) named |= pr.value("component", std::string()) == "MeshRenderer" && pr.value("severity", std::string()) == "error";
    REQUIRE(named);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a sprite drawn through a material the project wrote, its numbers per sprite", "[runtime][sprites][material]") {
    app::Options o;
    o.project_dir = root() / "samples" / "crates";
    o.bundle = root() / "build" / "ts" / "crates.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 960;
    o.height = 540;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("render.post", Json{{"effects", Json::array()}}).has_value());   // the sample's scanlines off
    REQUIRE(s.command("render.taa", Json{{"enabled", false}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Flash"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 8}, {"y", 9}, {"z", 1}}}}}, {"Sprite", Json{{"size", Json{{"x", 2}, {"y", 2}}}, {"color", Json{{"r", 1}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"material", "materials/flash.wgsl"}, {"params", Json{{"x", 1}, {"y", 0}, {"z", 0}, {"w", 0}}}}}}}}).has_value());
    auto pixel = [&]() {
        REQUIRE(s.frame().has_value());
        const Json r = s.command("render.project", Json{{"point", Json{{"x", 8}, {"y", 9}, {"z", 1}}}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
    };
    const Json white = pixel();
    INFO("white " << white.dump());
    REQUIRE(white[1].get<int>() > 230);
    REQUIRE(white[2].get<int>() > 230);
    REQUIRE(s.command("world.set", Json{{"entity", "Flash"}, {"component", "Sprite"}, {"value", Json{{"params", Json{{"x", 0}}}}}}).has_value());
    const Json red = pixel();
    INFO("red " << red.dump());
    REQUIRE(red[0].get<int>() > 200);
    REQUIRE(red[1].get<int>() < 40);
    // A material that is not there, or does not compile, is drawn plain and named by the lint.
    const auto broken = root() / "samples" / "crates" / "materials" / "broken.wgsl";
    REQUIRE(fs::write_text(broken, "fn material(texel: vec4f) -> vec4f { return texel +; }\n").has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Broken"}, {"components", Json{{"Transform", Json::object()}, {"Sprite", Json{{"material", "materials/broken.wgsl"}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Missing"}, {"components", Json{{"Transform", Json::object()}, {"Sprite", Json{{"material", "materials/none.wgsl"}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    std::filesystem::remove(broken);
    const Json lint = s.command("world.lint", Json::object()).value();
    INFO(lint.dump());
    int named = 0;
    for (const Json& pr : lint["problems"]) if (pr.value("component", std::string()) == "Sprite" && pr.value("severity", std::string()) == "error") ++named;
    REQUIRE(named == 2);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a mark and a diff say what an edit and some ticks changed", "[runtime][world][diff]") {
    app::Session s(hello_options(100000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(s.command("world.mark", Json::object()).value()["entities"].get<int>() >= 4);
    // Nothing yet.
    Json none = s.command("world.diff", Json::object()).value();
    REQUIRE(none["counts"] == Json{{"spawned", 0}, {"destroyed", 0}, {"changed", 0}});
    REQUIRE(s.command("world.set", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}, {"value", Json{{"color", Json{{"r", 0}, {"g", 1}, {"b", 0}, {"a", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Beacon"}, {"components", Json{{"Transform", Json::object()}, {"Light", Json::object()}}}}).has_value());
    REQUIRE(s.command("world.destroy", Json{{"entity", "Sun"}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    const Json d = s.command("world.diff", Json{{"since", "default"}}).value();
    INFO(d.dump());
    REQUIRE(d["spawned"][0]["path"] == "/Beacon");
    REQUIRE(d["spawned"][0]["components"] == Json::array({"Transform", "Light"}));
    REQUIRE(d["destroyed"][0]["path"] == "/Sun");
    bool ball = false;
    for (const Json& c : d["changed"]) {
        if (c["path"] != "/Ball") continue;
        ball = true;
        REQUIRE(c["changes"]["MeshRenderer"]["fields"]["color"][1]["g"] == 1);
        REQUIRE(c["changes"]["MeshRenderer"]["fields"].size() == 1);   // only what changed
        REQUIRE(c["changes"].contains("Transform"));                    // the ball fell
    }
    REQUIRE(ball);
    REQUIRE(d["since_tick"] == 2);
    REQUIRE_FALSE(s.command("world.diff", Json{{"since", "nope"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a project's post effects run after the tonemap, with parameters, and say why they do not compile", "[runtime][post]") {
    app::Session s(hello_options(100000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 5}}).has_value());
    // A still, coloured block in the middle of the view (the sample's ball bounces).
    REQUIRE(s.command("world.spawn", Json{{"name", "Block"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 2}}}, {"scale", Json{{"x", 2}, {"y", 2}, {"z", 0.2}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0.9}, {"g", 0.4}, {"b", 0.1}, {"a", 1}}}}}}}}).has_value());
    const Json ball = Json{{"x", 0}, {"y", 1}, {"z", 2.1}};
    auto pixel = [&]() {
        REQUIRE(s.frame().has_value());
        const Json r = s.command("render.project", Json{{"point", ball}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
    };
    REQUIRE(s.command("pause", Json::object()).has_value());
    REQUIRE(s.command("render.taa", Json{{"enabled", false}}).has_value());   // every frame the same: not jittered,
    REQUIRE(s.command("render.tonemap", Json{{"auto_exposure", false}}).has_value());   // nor metered
    REQUIRE(s.frame().has_value());
    const Json before = pixel();
    INFO("before " << before.dump());
    REQUIRE(std::abs(before[0].get<int>() - before[2].get<int>()) > 20);   // the ball has a colour
    // Grey, then grey turned over by a parameter.
    const std::string grey = "fn effect(uv: vec2f) -> vec4f { let c = sample_frame(uv); let g = dot(c.rgb, vec3f(0.299, 0.587, 0.114)); return vec4f(mix(vec3f(g), vec3f(1.0 - g), param(0)), 1.0); }";
    Json set = s.command("render.post", Json{{"effects", Json::array({Json{{"code", grey}, {"name", "grey"}}})}}).value();
    INFO(set.dump());
    REQUIRE(set["ok"] == true);
    const Json g = pixel();
    INFO("grey " << g.dump());
    REQUIRE(std::abs(g[0].get<int>() - g[1].get<int>()) <= 1);
    REQUIRE(std::abs(g[1].get<int>() - g[2].get<int>()) <= 1);
    const double lum = 0.299 * before[0].get<double>() + 0.587 * before[1].get<double>() + 0.114 * before[2].get<double>();
    REQUIRE(g[0].get<double>() == Catch::Approx(lum).margin(5));   // the tonemap dithers before it rounds to 8 bits
    REQUIRE(s.command("render.stats", Json::object()).value()["post_effects"] == 1);
    REQUIRE(s.command("render.post", Json{{"effects", Json::array({Json{{"code", grey}, {"params", Json::array({1})}}})}}).value()["ok"] == true);
    const Json turned = pixel();
    REQUIRE(turned[0].get<double>() == Catch::Approx(255 - g[0].get<double>()).margin(2));
    // Two at once, in order: grey then turned over by the second.
    const std::string invert = "fn effect(uv: vec2f) -> vec4f { let c = sample_frame(uv); return vec4f(1.0 - c.rgb, 1.0); }";
    REQUIRE(s.command("render.post", Json{{"effects", Json::array({Json{{"code", grey}}, Json{{"code", invert}}})}}).value()["ok"] == true);
    REQUIRE(pixel()[0].get<double>() == Catch::Approx(255 - g[0].get<double>()).margin(2));
    // A mistake is answered with the compiler's words, and the others still run.
    const Json bad = s.command("render.post", Json{{"effects", Json::array({Json{{"code", "fn effect(uv: vec2f) -> vec4f { return sample_frame(uv) +; }"}, {"name", "broken"}}, Json{{"code", grey}}})}}).value();
    INFO(bad.dump());
    REQUIRE(bad["ok"] == false);
    REQUIRE(bad["effects"][0]["ok"] == false);
    REQUIRE(bad["effects"][0]["error"].get<std::string>().find("broken:1:") != std::string::npos);   // the line in the effect's own code
    REQUIRE(bad["effects"][1]["ok"] == true);
    REQUIRE(s.command("render.post", Json::object()).value()["effects"].size() == 1);
    // None: the frame as before.
    REQUIRE(s.command("render.post", Json{{"effects", Json::array()}}).has_value());
    const Json after = pixel();
    REQUIRE(std::abs(after[0].get<int>() - before[0].get<int>()) <= 2);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("followers run along paths once, round and back and forth, and paths answer where", "[runtime][paths]") {
    app::Session s(hello_options(100000));
    REQUIRE(s.start().has_value());
    auto spawn = [&](const std::string& name, Json comps) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", comps}}).has_value()); };
    auto pt = [](float x, float y, float z) { return Json{{"x", x}, {"y", y}, {"z", z}}; };
    // A straight track ten units long, lifted one unit by its Transform.
    spawn("Track", Json{{"Transform", Json{{"position", pt(0, 1, 0)}}}, {"Path", Json{{"points", Json::array({pt(0, 0, 0), pt(10, 0, 0)})}, {"smooth", false}}}});
    spawn("Once", Json{{"Transform", Json::object()}, {"PathFollower", Json{{"path", "Track"}, {"speed", 5}, {"orient", "flat"}}}});
    spawn("Back", Json{{"Transform", Json::object()}, {"PathFollower", Json{{"path", "Track"}, {"speed", 4}, {"mode", "pingpong"}}}});
    // A closed, smooth loop through the corners of a square: round and round.
    spawn("Loop", Json{{"Transform", Json{{"position", pt(0, 0, 20)}}}, {"Path", Json{{"points", Json::array({pt(-2, 0, -2), pt(2, 0, -2), pt(2, 0, 2), pt(-2, 0, 2)})}, {"closed", true}}}});
    spawn("Lapper", Json{{"Transform", Json::object()}, {"PathFollower", Json{{"path", "Loop"}, {"speed", 6}, {"orient", "forward"}}}});
    REQUIRE(s.command("step", Json{{"ticks", 60}}).has_value());
    auto pos = [&](const std::string& e) { return s.command("world.get", Json{{"entity", e}, {"component", "Transform"}}).value()["position"]; };
    auto fol = [&](const std::string& e) { return s.command("world.get", Json{{"entity", e}, {"component", "PathFollower"}}).value(); };
    REQUIRE(pos("Once")["x"].get<double>() == Catch::Approx(5).margin(0.01));
    REQUIRE(pos("Once")["y"].get<double>() == Catch::Approx(1).margin(0.01));
    REQUIRE(s.command("world.get", Json{{"entity", "Track"}, {"component", "Path"}}).value()["length"].get<double>() == Catch::Approx(10));
    REQUIRE(s.command("step", Json{{"ticks", 120}, {"until", Json{{"event", "path.arrived"}}}}).value()["until"]["met"] == true);
    // Once: at the end, finished; ping-pong came back (12 units of travel in the three seconds: out and 2 back).
    REQUIRE(s.command("step", Json{{"ticks", 60}}).has_value());
    REQUIRE(pos("Once")["x"].get<double>() == Catch::Approx(10).margin(0.01));
    REQUIRE(fol("Once")["finished"] == true);
    REQUIRE(fol("Back")["speed"].get<double>() < 0);
    REQUIRE(pos("Back")["x"].get<double>() == Catch::Approx(8).margin(0.15));
    // Round the loop: still on it; the curve through the square's corners swells a little past its 16.
    const double loop_len = s.command("path.info", Json{{"entity", "Loop"}}).value()["length"].get<double>();
    REQUIRE(loop_len > 16);
    REQUIRE(loop_len < 18);
    const Json near = s.command("path.nearest", Json{{"entity", "Loop"}, {"point", pos("Lapper")}}).value();
    REQUIRE(near["away"].get<double>() < 0.01);
    // Questions: a point along the track, and how far along a point beside it lies.
    const Json at = s.command("path.sample", Json{{"entity", "Track"}, {"fraction", 0.25}}).value();
    REQUIRE(at["point"]["x"].get<double>() == Catch::Approx(2.5));
    REQUIRE(at["direction"]["x"].get<double>() == Catch::Approx(1));
    REQUIRE(s.command("path.nearest", Json{{"entity", "Track"}, {"point", pt(3, 5, 0)}}).value()["distance"].get<double>() == Catch::Approx(3));
    REQUIRE_FALSE(s.command("path.info", Json{{"entity", "Once"}}).has_value());
    REQUIRE(s.command("render.debug", Json{{"paths", true}}).value()["paths"] == true);
    REQUIRE(s.command("events.since", Json{{"seq", 0}, {"type", "path.looped"}}).value()["events"].size() == 0);   // a closed path goes round without a jump
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a Tiled polyline becomes a Path a follower runs along", "[runtime][paths][tiled]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    const Json objs = s.command("tilemap.objects", Json{{"entity", "Level"}, {"layer", "routes"}}).value();
    INFO(objs.dump());
    REQUIRE(objs[0]["polyline"] == true);
    REQUIRE(objs[0]["points"].size() == 3);
    const Json made = s.command("tilemap.paths", Json{{"entity", "Level"}, {"layer", "routes"}}).value();
    REQUIRE(made["paths"][0]["path"] == "/patrol");
    // The level's top-left is at (-10, 4.5), a tile a unit: the route starts at (-7, -3), runs 4
    // right and 2 up.
    const Json info = s.command("path.info", Json{{"entity", "patrol"}}).value();
    REQUIRE(info["length"].get<double>() == Catch::Approx(6));
    REQUIRE(info["start"]["x"].get<double>() == Catch::Approx(-7));
    REQUIRE(info["start"]["y"].get<double>() == Catch::Approx(-3));
    REQUIRE(info["end"]["y"].get<double>() == Catch::Approx(-1));
    REQUIRE(s.command("world.spawn", Json{{"name", "Guard"}, {"components", Json{{"Transform", Json::object()}, {"Sprite", Json::object()}, {"PathFollower", Json{{"path", "patrol"}, {"speed", 3}, {"mode", "pingpong"}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 60}}).has_value());
    const Json at = s.command("world.get", Json{{"entity", "Guard"}, {"component", "Transform"}}).value()["position"];
    REQUIRE(at["x"].get<double>() == Catch::Approx(-4).margin(0.05));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a game without on-screen controls gets them from its input map at the first touch", "[runtime][input][touchcontrols][autotouch]") {
    app::Options o;
    o.project_dir = root() / "samples" / "walker";
    o.bundle = root() / "build" / "ts" / "walker.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 960;
    o.height = 540;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    auto value = [&](const std::string& name) { return s.command("input.actions", Json::object()).value()[name]["value"].get<double>(); };
    // The stick sits 110 points from the left and the bottom; a finger put down on it and moved
    // 50 points right and 50 up walks right and forward (move_z's negative side is W, up).
    REQUIRE(s.command("input.touch", Json{{"x", 110}, {"y", 430}, {"phase", "down"}}).has_value());
    REQUIRE(s.command("input.touch", Json{{"x", 160}, {"y", 380}, {"phase", "move"}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    INFO(s.command("input.actions", Json::object()).value().dump());
    REQUIRE(value("move_x") > 0.5);
    REQUIRE(value("move_z") < -0.5);
    REQUIRE_FALSE(s.command("ui.query", Json{{"name", "touch-controls"}}).value().empty());
    REQUIRE(s.command("input.touch", Json{{"x", 160}, {"y", 380}, {"phase", "up"}}).has_value());
    // A button for each other action, by name (crouch first, 80 points from the right and 90 from the bottom).
    REQUIRE(s.command("input.touch", Json{{"x", 880}, {"y", 450}, {"phase", "down"}, {"finger", 1}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    REQUIRE(value("crouch") == 1.0);
}

TEST_CASE("an action's value set directly, and an on-screen stick and button that set it from fingers", "[runtime][input][touchcontrols]") {
    app::Options o;
    o.project_dir = root() / "samples" / "crates";
    o.bundle = root() / "build" / "ts" / "crates.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 960;
    o.height = 540;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 1}}).has_value());
    auto action = [&](const std::string& name) { return s.command("input.actions", Json::object()).value()[name]; };
    // input.axis: part way is a value without a press; past 0.5 it is down; 0 lets go.
    REQUIRE(s.command("input.axis", Json{{"action", "move_x"}, {"value", 0.4}}).has_value());
    REQUIRE(action("move_x")["value"].get<double>() == Catch::Approx(0.4));
    REQUIRE(action("move_x")["down"] == false);
    REQUIRE(s.command("input.axis", Json{{"action", "move_x"}, {"value", -0.8}}).has_value());
    REQUIRE(action("move_x")["down"] == true);
    REQUIRE(s.command("input.axis", Json{{"action", "move_x"}, {"value", 0}}).has_value());
    REQUIRE(action("move_x")["value"].get<double>() == 0);
    REQUIRE_FALSE(s.command("input.axis", Json{{"action", "nope"}, {"value", 1}}).has_value());
    // The sample's stick (100 points from the left and the bottom): a finger down on it and moved
    // 50 points right tilts move_x most of the way; lifted, it lets go.
    REQUIRE(s.command("input.touch", Json{{"x", 100}, {"y", 440}, {"phase", "down"}}).has_value());
    REQUIRE(s.command("input.touch", Json{{"x", 150}, {"y", 440}, {"phase", "move"}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    const double tilt = action("move_x")["value"].get<double>();
    REQUIRE(tilt == Catch::Approx((50.0 / 60.0 - 0.15) / 0.85).margin(0.01));
    REQUIRE(s.command("ui.query", Json{{"name", "touch-controls"}}).value().size() == 1);   // drawn since the first touch
    // A second finger on the button holds fire while the first still tilts the stick.
    REQUIRE(s.command("input.touch", Json{{"x", 870}, {"y", 450}, {"phase", "down"}, {"finger", 1}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(action("fire")["down"] == true);
    REQUIRE(action("move_x")["value"].get<double>() == Catch::Approx(tilt));
    REQUIRE(s.command("input.touch", Json{{"x", 870}, {"y", 450}, {"phase", "up"}, {"finger", 1}}).has_value());
    REQUIRE(s.command("input.touch", Json{{"x", 150}, {"y", 440}, {"phase", "up"}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(action("fire")["down"] == false);
    REQUIRE(action("move_x")["value"].get<double>() == 0);
    REQUIRE(s.command("state", Json::object()).value()["state"]["released"] == true);   // the button let the ball go
    // The window as it is, and a request remembered headless.
    REQUIRE(s.command("window.info", Json::object()).value()["width"] == 960);
    REQUIRE(s.command("window.set", Json{{"fullscreen", true}}).value()["fullscreen"] == true);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("2D rigid bodies stack, swing on joints, hit through sensors, land on tiles and repeat exactly", "[runtime][physics2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    auto spawn = [](app::Session& s, const std::string& name, Json comps) {
        auto r = s.command("world.spawn", Json{{"name", name}, {"components", comps}});
        INFO(name);
        REQUIRE(r.has_value());
    };
    auto at = [](float x, float y) { return Json{{"position", Json{{"x", x}, {"y", y}, {"z", 0}}}}; };
    auto build = [&](app::Session& s) {
        // A floor of its own far from the sample's level, three boxes over it, a pendulum, a hazard.
        spawn(s, "Floor", Json{{"Transform", at(60, 0)}, {"Collider2D", Json{{"size", Json{{"x", 10}, {"y", 0.5}}}}}});
        for (int i = 0; i < 3; ++i) spawn(s, "Box" + std::to_string(i), Json{{"Transform", at(60, 1.2f + 1.1f * static_cast<float>(i))}, {"RigidBody2D", Json::object()}, {"Collider2D", Json{{"size", Json{{"x", 0.5}, {"y", 0.5}}}}}});
        spawn(s, "Bob", Json{{"Transform", at(80, 10)}, {"RigidBody2D", Json::object()}, {"Collider2D", Json{{"shape", "circle"}, {"radius", 0.25}}}, {"Joint2D", Json{{"kind", "revolute"}, {"anchor", Json{{"x", -2}, {"y", 0}}}, {"other_anchor", Json{{"x", 78}, {"y", 10}}}}}});   // hinged two units to its left
        spawn(s, "Spikes", Json{{"Transform", at(70, 3)}, {"Collider2D", Json{{"size", Json{{"x", 1}, {"y", 0.2}}}, {"sensor", true}}}, {"Hitbox", Json{{"damage", 10}}}});
        spawn(s, "Victim", Json{{"Transform", at(70, 6)}, {"RigidBody2D", Json::object()}, {"Collider2D", Json{{"shape", "circle"}, {"radius", 0.3}}}, {"Health", Json{{"current", 30}, {"max", 30}}}});
        // A ball over the sample's own tile level: it lands on the solid tiles.
        spawn(s, "Marble", Json{{"Transform", at(0, 2)}, {"RigidBody2D", Json::object()}, {"Collider2D", Json{{"shape", "circle"}, {"radius", 0.3}}}});
    };
    app::Session s(o);
    REQUIRE(s.start().has_value());
    build(s);
    REQUIRE(s.command("step", Json{{"ticks", 240}}).has_value());
    auto get = [&](app::Session& ss, const std::string& e, const std::string& c) { return ss.command("world.get", Json{{"entity", e}, {"component", c}}).value(); };
    // The stack stands: each box a unit above the one under it, asleep.
    for (int i = 0; i < 3; ++i) {
        const Json t = get(s, "Box" + std::to_string(i), "Transform");
        INFO(t.dump());
        REQUIRE(t["position"]["y"].get<double>() == Catch::Approx(1.0 + i).margin(0.03));
        REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(60).margin(0.03));
    }
    REQUIRE(get(s, "Box2", "RigidBody2D")["awake"] == false);
    // The pendulum swings at its length from the pivot.
    const Json bob = get(s, "Bob", "Transform")["position"];
    REQUIRE(std::hypot(bob["x"].get<double>() - 78, bob["y"].get<double>() - 10) == Catch::Approx(2).margin(0.02));
    REQUIRE(bob["y"].get<double>() < 9.9);
    // The sensor stopped nothing and its hitbox hurt what fell through it.
    REQUIRE(get(s, "Victim", "Health")["current"].get<double>() == Catch::Approx(20));
    REQUIRE(get(s, "Victim", "Transform")["position"]["y"].get<double>() < 2);
    // The marble rests on the level's tiles.
    const Json ball = get(s, "Marble", "Transform")["position"];
    INFO(ball.dump() << " " << get(s, "Marble", "RigidBody2D").dump());
    const Json below = s.command("tilemap.solid", Json{{"entity", "Level"}, {"x", ball["x"]}, {"y", ball["y"].get<double>() - 0.5}}).value();
    REQUIRE(below["solid"] == true);
    REQUIRE(std::fabs(get(s, "Marble", "RigidBody2D")["velocity"]["y"].get<double>()) < 0.05);
    // Questions: a ray down onto the top box, an overlap at the stack, a push.
    const Json ray = s.command("physics2d.raycast", Json{{"from", Json{{"x", 60}, {"y", 10}}}, {"direction", Json{{"x", 0}, {"y", -1}}}}).value();
    REQUIRE(ray["path"] == "/Box2");
    REQUIRE(ray["point"]["y"].get<double>() == Catch::Approx(3.5).margin(0.03));
    REQUIRE(s.command("physics2d.overlap", Json{{"center", Json{{"x", 60}, {"y", 1.5}}}, {"half", Json{{"x", 0.2}, {"y", 0.8}}}}).value()["entities"].size() == 2);
    REQUIRE(s.command("physics2d.impulse", Json{{"entity", "Box2"}, {"impulse", Json{{"x", 3}, {"y", 0}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 30}}).has_value());
    REQUIRE(get(s, "Box2", "Transform")["position"]["x"].get<double>() > 60.5);
    const Json stats = s.command("physics2d.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["bodies"] == 8);   // the floor and the spikes are static bodies of their own
    REQUIRE(stats["joints"] == 1);
    REQUIRE(stats["tile_shapes"].get<int>() > 0);
    const std::string hash = s.command("state", Json::object()).value()["world_hash"].get<std::string>();
    // A field holding an entity takes its name.
    const Json box0 = s.command("world.find", Json{{"path", "Box0"}}).value();
    REQUIRE(s.command("world.spawn", Json{{"name", "Tag"}, {"components", Json{{"Transform", at(90, 0)}, {"Joint2D", Json{{"body", "Box0"}}}}}}).has_value());
    REQUIRE(get(s, "Tag", "Joint2D")["body"] == box0);
    REQUIRE_FALSE(s.command("world.set", Json{{"entity", "Tag"}, {"component", "Joint2D"}, {"value", Json{{"body", "Nobody"}}}}).has_value());
    REQUIRE(s.finish().has_value());
    // The same again, step for step: the same world.
    app::Session t(o);
    REQUIRE(t.start().has_value());
    build(t);
    REQUIRE(t.command("step", Json{{"ticks", 240}}).has_value());
    REQUIRE(t.command("physics2d.impulse", Json{{"entity", "Box2"}, {"impulse", Json{{"x", 3}, {"y", 0}}}}).has_value());
    REQUIRE(t.command("step", Json{{"ticks", 30}}).has_value());
    REQUIRE(t.command("state", Json::object()).value()["world_hash"] == hash);
    REQUIRE(t.finish().has_value());
}

TEST_CASE("a tile map made by code is filled, collided with, and saved with the scene", "[runtime][tilemap][made]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    Json scene;
    {
        app::Session s(o);
        REQUIRE(s.start().has_value());
        const Json made = s.command("tilemap.create", Json{{"name", "maps/dungeon.tmj"}, {"width", 12}, {"height", 6}, {"layers", Json::array({Json{{"name", "walls"}, {"solid", true}}, "floor"})}, {"tilesets", Json::array({Json{{"image", "assets/tiles.png"}}})}}).value();
        INFO(made.dump());
        REQUIRE(made["layers"] == Json::array({"walls", "floor"}));
        REQUIRE(made["tilesets"][0]["tiles"] == 7);   // a 112 by 16 strip of 16-pixel tiles
        // Drawn far from the sample's level, a wall along its bottom row, and a box dropped on it.
        REQUIRE(s.command("world.spawn", Json{{"name", "Dungeon"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 100}, {"y", 0}, {"z", 0}}}}}, {"TileMap", Json{{"map", "maps/dungeon.tmj"}}}}}}).has_value());
        REQUIRE(s.command("tilemap.fill", Json{{"entity", "Dungeon"}, {"tile_x", 0}, {"tile_y", 5}, {"width", 12}, {"height", 1}, {"layer", "walls"}, {"id", 1}}).has_value());
        REQUIRE(s.command("tilemap.solid", Json{{"entity", "Dungeon"}, {"tile_x", 3}, {"tile_y", 5}}).value()["solid"] == true);
        REQUIRE(s.command("tilemap.solid", Json{{"entity", "Dungeon"}, {"tile_x", 3}, {"tile_y", 4}}).value()["solid"] == false);
        REQUIRE(s.command("world.spawn", Json{{"name", "Crate"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 104}, {"y", -2}, {"z", 0}}}}}, {"Body2D", Json{{"size", Json{{"x", 0.4}, {"y", 0.4}}}, {"map", "Dungeon"}}}}}}).has_value());
        REQUIRE(s.command("step", Json{{"ticks", 120}}).has_value());
        const Json body = s.command("world.get", Json{{"entity", "Crate"}, {"component", "Body2D"}}).value();
        REQUIRE(body["grounded"] == true);
        REQUIRE(s.command("world.get", Json{{"entity", "Crate"}, {"component", "Transform"}}).value()["position"]["y"].get<double>() == Catch::Approx(-4.6).margin(0.05));
        scene = s.command("world.save", Json::object()).value();
        REQUIRE(scene["tilemaps"].contains("maps/dungeon.tmj"));
        REQUIRE_FALSE(s.command("tilemap.create", Json{{"name", "maps/x.tmj"}, {"width", 0}, {"height", 3}}).has_value());
        REQUIRE(s.finish().has_value());
    }
    // A session that never made it draws and collides with it from the saved scene.
    app::Session t(o);
    REQUIRE(t.start().has_value());
    REQUIRE(t.command("world.load", Json{{"scene", scene}}).has_value());
    REQUIRE(t.command("tilemap.solid", Json{{"entity", "Dungeon"}, {"tile_x", 11}, {"tile_y", 5}}).value()["solid"] == true);
    REQUIRE(t.finish().has_value());
}

TEST_CASE("sight and a field of view over a tile map stop at walls and at the layers named", "[runtime][tilemap][sight]") {
    app::Options o;
    o.project_dir = root() / "samples" / "sprites";
    o.bundle = root() / "build" / "ts" / "sprites.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // A 12 by 6 room at x 100: a wall down column 6 from row 0 to row 4, a tuft of grass at (3, 2).
    REQUIRE(s.command("tilemap.create", Json{{"name", "maps/room.tmj"}, {"width", 12}, {"height", 6}, {"layers", Json::array({Json{{"name", "walls"}, {"solid", true}}, "grass"})}, {"tilesets", Json::array({Json{{"image", "assets/tiles.png"}}})}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Room"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 100}, {"y", 0}, {"z", 0}}}}}, {"TileMap", Json{{"map", "maps/room.tmj"}}}}}}).has_value());
    REQUIRE(s.command("tilemap.fill", Json{{"entity", "Room"}, {"tile_x", 6}, {"tile_y", 0}, {"width", 1}, {"height", 5}, {"layer", "walls"}, {"id", 1}}).has_value());
    REQUIRE(s.command("tilemap.set", Json{{"entity", "Room"}, {"tile_x", 3}, {"tile_y", 2}, {"layer", "grass"}, {"id", 2}}).has_value());
    const Json eye{{"x", 102.5}, {"y", -2.5}};
    auto sight = [&](double x, double y, Json layers = nullptr, Json from = nullptr) {
        Json args{{"entity", "Room"}, {"from", from.is_null() ? eye : from}, {"to", Json{{"x", x}, {"y", y}}}};
        if (!layers.is_null()) args["layers"] = layers;
        return s.command("tilemap.sight", args).value();
    };
    // Through the wall: stopped at its near face, three and a half units on.
    Json r = sight(109.5, -2.5);
    INFO(r.dump());
    REQUIRE(r["visible"] == false);
    REQUIRE(r["blocked_at"]["tile_x"] == 6);
    REQUIRE(r["blocked_at"]["tile_y"] == 2);
    REQUIRE(r["blocked_at"]["point"]["x"].get<double>() == Catch::Approx(106.0).margin(1e-6));
    REQUIRE(r["blocked_at"]["distance"].get<double>() == Catch::Approx(3.5).margin(1e-6));
    // Short of it, clear; under its end (row 5 is open), clear; the grass hides nothing by default.
    REQUIRE(sight(104.5, -2.5)["visible"] == true);
    REQUIRE(sight(109.5, -5.5)["visible"] == false);   // the line clips the wall's foot at row 4
    REQUIRE(sight(102.5, -5.5)["visible"] == true);
    // Naming the layers that hide: the grass stops the line, the wall no longer does.
    REQUIRE(sight(104.5, -2.5, Json::array({"grass"}))["blocked_at"]["tile_x"] == 3);
    REQUIRE(sight(109.5, -0.5)["visible"] == false);
    REQUIRE(sight(109.5, -0.5, Json::array({"grass"}), Json{{"x", 102.5}, {"y", -0.5}})["visible"] == true);   // along row 0, through the wall
    REQUIRE_FALSE(s.command("tilemap.sight", Json{{"entity", "Room"}, {"from", eye}, {"to", eye}, {"layers", Json::array({"nope"})}}).has_value());
    // The field of view: the near side and the wall's face, nothing behind the wall.
    const Json fov = s.command("tilemap.fov", Json{{"entity", "Room"}, {"from", eye}, {"radius", 8}}).value();
    auto has = [&](int x, int y) { for (const Json& c : fov["cells"]) if (c[0] == x && c[1] == y) return true; return false; };
    REQUIRE(has(2, 2));
    REQUIRE(has(4, 2));
    REQUIRE(has(6, 2));
    REQUIRE(has(6, 0));
    REQUIRE_FALSE(has(8, 2));
    REQUIRE_FALSE(has(7, 1));
    REQUIRE(fov["walls"].get<int>() == 5);
    REQUIRE(fov["count"] == fov["cells"].size());
    // A point as [x, y] is the same point; a value of the wrong type is a bad_args answer, not a crash.
    REQUIRE(s.command("tilemap.fov", Json{{"entity", "Room"}, {"from", Json::array({eye["x"], eye["y"]})}, {"radius", 8}}).value()["count"] == fov["count"]);
    auto wrong = s.command("tilemap.fov", Json{{"entity", "Room"}, {"from", Json::array({"a", "b"})}, {"radius", 8}});
    REQUIRE_FALSE(wrong.has_value());
    REQUIRE(wrong.error().code == "bad_args");
    REQUIRE(s.command("tilemap.fov", Json{{"entity", "Room"}, {"from", eye}, {"radius", 8}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("lit sprites and maps take the scene's lights and ambient, normal maps bend them, unlit ones stay", "[runtime][sprites][lit2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "dungeon";
    o.bundle = root() / "build" / "ts" / "dungeon.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("render.taa", Json{{"enabled", false}}).has_value());
    REQUIRE(s.command("render.bloom", Json{{"enabled", false}}).has_value());
    // Held still (a hit-stop: frames draw, no tick runs): the script's ticks would move the camera
    // back to the player and flicker the torches.
    REQUIRE(s.command("time.scale", Json{{"scale", 0}}).has_value());
    // Light alone here: the map's shadows (its own test below) off.
    REQUIRE(s.command("world.set", Json{{"entity", "Level"}, {"component", "TileMap"}, {"value", Json{{"shadows", false}}}}).has_value());
    // Every light out, the camera close over the first crate (at 9.5, -7.5; 0.9 across).
    for (const Json& e : s.command("world.query", Json{{"with", Json::array({"Light"})}}).value()["entities"]) {
        REQUIRE(s.command("world.set", Json{{"entity", e["id"]}, {"component", "Light"}, {"value", Json{{"intensity", 0}}}}).has_value());
    }
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 9.5}, {"y", -7.5}, {"z", 10}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Camera"}, {"value", Json{{"ortho_size", 1.5}}}}).has_value());
    // Paused, so the moves are put into the world transforms by hand before each frame.
    auto pixel = [&](double x, double y) {
        REQUIRE(s.command("world.update_transforms", Json::object()).has_value());
        REQUIRE(s.frame().has_value());
        const Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", 0}}}}).value();
        const Json px = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        return px[0].get<int>() + px[1].get<int>() + px[2].get<int>();
    };
    // No light at all (and no sun: the key light a scene without one gets is not for sprites): the
    // crate and the floor beside it are black.
    Json amb = s.command("render.ambient", Json{{"color", "#ffffff"}, {"intensity", 0}}).value();
    REQUIRE(amb["intensity"] == 0.0);
    REQUIRE(pixel(9.5, -7.5) < 10);
    REQUIRE(pixel(9.5, -6.5) < 10);
    // White ambient: the crate shows its own colours, as an unlit sprite would.
    REQUIRE(s.command("render.ambient", Json{{"intensity", 1}}).has_value());
    const int ambient_lit = pixel(9.5, -7.5);
    INFO("ambient " << ambient_lit);
    REQUIRE(ambient_lit > 150);
    // In the dark again, a lamp near the crate lights it, and the floor more where it is nearer.
    REQUIRE(s.command("render.ambient", Json{{"intensity", 0}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Lamp"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 9.5}, {"y", -6.5}, {"z", 0.6}}}}}, {"Light", Json{{"kind", "point"}, {"color", "#ffffff"}, {"intensity", 2}, {"range", 4}}}}}}).has_value());
    const int near_floor = pixel(9.25, -6.3), far_floor = pixel(10.75, -6.3);   // on stones, off the grout
    INFO("floor near " << near_floor << " far " << far_floor);
    REQUIRE(pixel(9.5, -7.5) > 30);
    REQUIRE(near_floor > far_floor + 20);
    // A light low on the right: the frame's inner edges of the normal map face each other, so the
    // left one (facing right) takes more light than the right one (facing left), nearer as it is.
    REQUIRE(s.command("world.set", Json{{"entity", "Lamp"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 15.5}, {"y", -7.4}, {"z", 0.25}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"intensity", 6}, {"range", 30}}}}).has_value());
    const double row = -7.5 + (0.5 - 10.5 / 32) * 0.9;
    const int left_edge = pixel(9.5 - (0.5 - 2.5 / 32) * 0.9, row), right_edge = pixel(9.5 + (0.5 - 2.5 / 32) * 0.9 - 0.03, row);
    INFO("left edge " << left_edge << " right edge " << right_edge);
    REQUIRE(left_edge > right_edge + 15);
    // Without the normal map the two are alike (the right one a little nearer the light).
    REQUIRE(s.command("world.set", Json{{"entity", "Crate_1"}, {"component", "Sprite"}, {"value", Json{{"normal_map", ""}}}}).has_value());
    const int flat_left = pixel(9.5 - (0.5 - 2.5 / 32) * 0.9, row), flat_right = pixel(9.5 + (0.5 - 2.5 / 32) * 0.9 - 0.03, row);
    INFO("flat left " << flat_left << " flat right " << flat_right);
    REQUIRE(std::abs(flat_left - flat_right) < std::abs(left_edge - right_edge));
    // An unlit sprite (the flames) is as bright in the dark as it was drawn.
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 1.5}, {"y", -1.2}, {"z", 10}}}}}}).has_value());
    const Json flame = s.command("world.get", Json{{"entity", "Torch_1/Flame"}, {"component", "WorldTransform"}}).value();
    REQUIRE(pixel(flame["position"]["x"].get<double>(), flame["position"]["y"].get<double>()) > 300);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a tile map's solid cells cast 2D shadows from the lights onto lit sprites and maps", "[runtime][sprites][lit2d][shadows2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "dungeon";
    o.bundle = root() / "build" / "ts" / "dungeon.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("render.taa", Json{{"enabled", false}}).has_value());
    REQUIRE(s.command("render.bloom", Json{{"enabled", false}}).has_value());
    REQUIRE(s.command("time.scale", Json{{"scale", 0}}).has_value());
    for (const Json& e : s.command("world.query", Json{{"with", Json::array({"Light"})}}).value()["entities"]) {
        REQUIRE(s.command("world.set", Json{{"entity", e["id"]}, {"component", "Light"}, {"value", Json{{"intensity", 0}}}}).has_value());
    }
    REQUIRE(s.command("render.ambient", Json{{"intensity", 0}}).has_value());
    // Fog off, the camera over the wall between the first room (x 1 to 10) and the second (15 to 30).
    REQUIRE(s.command("tilemap.fill", Json{{"entity", "Level"}, {"tile_x", 0}, {"tile_y", 0}, {"width", 32}, {"height", 20}, {"layer", "fog"}, {"id", nullptr}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 13}, {"y", -2.5}, {"z", 10}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Camera"}, {"value", Json{{"ortho_size", 4}}}}).has_value());
    // A lamp by the first room's east wall, reaching well into the second room.
    REQUIRE(s.command("world.spawn", Json{{"name", "Lamp"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 10.5}, {"y", -2.5}, {"z", 0.6}}}}}, {"Light", Json{{"kind", "point"}, {"color", "#ffffff"}, {"intensity", 3}, {"range", 9}}}}}}).has_value());
    auto pixel = [&](double x, double y) {
        REQUIRE(s.command("world.update_transforms", Json::object()).has_value());
        REQUIRE(s.frame().has_value());
        const Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", 0}}}}).value();
        const Json px = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        return px[0].get<int>() + px[1].get<int>() + px[2].get<int>();
    };
    // The sample's map casts: the second room's floor behind four cells of wall gets nothing; the
    // first room's floor by the lamp and the wall's face toward it are lit.
    const int behind = pixel(15.25, -2.3), near_floor = pixel(9.25, -2.3), face = pixel(11.1, -2.3);
    INFO("behind " << behind << " near " << near_floor << " face " << face);
    REQUIRE(behind < 10);
    REQUIRE(near_floor > 60);
    REQUIRE(face > 30);
    // Without shadows the light goes through the wall.
    REQUIRE(s.command("world.set", Json{{"entity", "Level"}, {"component", "TileMap"}, {"value", Json{{"shadows", false}}}}).has_value());
    const int through = pixel(15.25, -2.3);
    INFO("through " << through);
    REQUIRE(through > 30);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a capture looks at the frame in characters for a model that reads", "[runtime][capture][look]") {
    app::Session s(hello_options(100000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(s.frame().has_value());
    const Json c = s.command("capture", Json{{"ascii", 32}}).value();
    REQUIRE(c.contains("look"));
    const Json& look = c["look"];
    INFO(look.dump());
    // 32 across; rows for the frame's shape at half height per character.
    REQUIRE(look["ascii"][0].get<std::string>().size() == 32);
    REQUIRE(look["ascii"].size() == look["ascii_colours"].size());
    REQUIRE(look["ascii"].size() == static_cast<std::size_t>(std::lround(32.0 * c["height"].get<double>() / c["width"].get<double>() * 0.5)));
    REQUIRE_FALSE(look["colours"].empty());
    REQUIRE_FALSE(look.contains("coverage"));   // a frame is drawn everywhere
    REQUIRE_FALSE(s.command("capture", Json::object()).value().contains("look"));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a map drawn in characters: tiles on layers, objects at cell centers, walls solid", "[runtime][tilemap][text]") {
    app::Options o;
    o.project_dir = root() / "samples" / "dungeon";
    o.bundle = root() / "build" / "ts" / "dungeon.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.paused = true;
    o.frames = 100000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    const Json rows = Json::array({"##########", "#@..#....#", "#...#.c..#", "#.c....###", "##########"});
    const Json legend{{"*", Json{{"layer", "floor"}, {"tile", 0}}}, {"#", Json{{"layer", "walls"}, {"tile", 1}}}, {".", nullptr}, {"@", Json{{"object", "start"}}}, {"c", Json{{"object", "coin"}}}};
    const Json made = s.command("tilemap.text", Json{{"name", "maps/text.tmj"}, {"rows", rows}, {"legend", legend}, {"tilesets", Json::array({Json{{"image", "assets/dungeon_tiles.png"}}})}, {"layers", Json::array({"floor", Json{{"name", "walls"}, {"solid", true}}})}}).value();
    INFO(made.dump());
    REQUIRE(made["width"] == 10);
    REQUIRE(made["height"] == 5);
    REQUIRE(made["layers"] == Json::array({"floor", "walls"}));
    REQUIRE(made["objects"]["coin"] == 2);
    REQUIRE(made["objects"]["start"] == 1);
    REQUIRE(s.command("world.spawn", Json{{"name", "Text"}, {"components", Json{{"Transform", Json::object()}, {"TileMap", Json{{"map", "maps/text.tmj"}}}}}}).has_value());
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Text"}, {"tile_x", 4}, {"tile_y", 1}}).value()["solid"] == true);
    REQUIRE(s.command("tilemap.solid", Json{{"entity", "Text"}, {"tile_x", 3}, {"tile_y", 1}}).value()["solid"] == false);
    // The floor is under every cell, walls included.
    const Json tile = s.command("tilemap.tile", Json{{"entity", "Text"}, {"tile_x", 0}, {"tile_y", 0}, {"layer", "floor"}}).value();
    REQUIRE(tile["layers"][0]["id"] == 0);
    const Json objs = s.command("tilemap.objects", Json{{"entity", "Text"}}).value();
    REQUIRE(objs[0]["name"] == "Start_1");
    REQUIRE(objs[0]["center"]["x"].get<double>() == Catch::Approx(1.5));
    REQUIRE(objs[0]["center"]["y"].get<double>() == Catch::Approx(-1.5));
    // A character the legend lacks, and a tile the tileset lacks, are named.
    auto bad = s.command("tilemap.text", Json{{"name", "maps/bad.tmj"}, {"rows", Json::array({"#?#"})}, {"legend", legend}, {"tilesets", Json::array({Json{{"image", "assets/dungeon_tiles.png"}}})}});
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().message.find("'?'") != std::string::npos);
    auto far = s.command("tilemap.text", Json{{"name", "maps/bad.tmj"}, {"rows", Json::array({"#"})}, {"legend", Json{{"#", Json{{"layer", "walls"}, {"tile", 9}}}}}, {"tilesets", Json::array({Json{{"image", "assets/dungeon_tiles.png"}}})}});
    REQUIRE_FALSE(far.has_value());
    REQUIRE(far.error().message.find("outside tileset") != std::string::npos);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a mesh made from numbers is drawn, collided with, and saved with the scene", "[runtime][mesh][made]") {
    app::Session s(hello_options(1000));
    REQUIRE(s.start().has_value());
    // A 4 by 4 floor, its triangles wound to face up, its normals made from them.
    const Json floor{{"name", "floor"}, {"positions", Json::array({Json::array({-2, 0, -2}), Json::array({2, 0, -2}), Json::array({2, 0, 2}), Json::array({-2, 0, 2})})}, {"indices", Json::array({0, 2, 1, 0, 3, 2})}, {"uvs", Json::array({0, 0, 1, 0, 1, 1, 0, 1})}};
    const Json made = s.command("mesh.create", floor).value();
    INFO(made.dump());
    REQUIRE(made["mesh"] == "mesh:floor");
    REQUIRE(made["triangles"] == 2);
    REQUIRE(made["bounds"]["max"][0].get<double>() == Catch::Approx(2));
    // A static body on it as a mesh collider, high above the sample's own ground, and a ball dropped on it.
    REQUIRE(s.command("world.spawn", Json{{"name", "Made"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 40}, {"y", 5}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "mesh:floor"}, {"color", Json{{"r", 1}, {"g", 0}, {"b", 0}, {"a", 1}}}}}, {"RigidBody", Json{{"kind", "static"}}}, {"Collider", Json{{"shape", "mesh"}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Drop"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 40}, {"y", 7}, {"z", 0}}}}}, {"RigidBody", Json::object()}, {"Collider", Json{{"shape", "sphere"}, {"size", Json{{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}}}}).has_value());
    REQUIRE(s.command("step", Json{{"ticks", 180}}).has_value());
    const double y = s.command("world.get", Json{{"entity", "Drop"}, {"component", "Transform"}}).value()["position"]["y"].get<double>();
    REQUIRE(y == Catch::Approx(5.5).margin(0.1));
    // Its bounds are the mesh's, known without drawing it.
    const Json b = s.command("world.get", Json{{"entity", "Made"}, {"component", "Bounds"}}).value();
    REQUIRE(b["max"]["x"].get<double>() == Catch::Approx(42).margin(0.01));
    // Saved with the scene, and made again when the scene loads into a session that never had it.
    const Json scene = s.command("world.save", Json::object()).value();
    REQUIRE(scene["meshes"]["floor"]["indices"].size() == 6);
    REQUIRE(s.command("mesh.list", Json::object()).value()["meshes"].size() == 1);
    REQUIRE_FALSE(s.command("mesh.create", Json{{"name", "bad"}, {"positions", Json::array({0, 0, 0, 1, 0, 0, 0, 1, 0})}, {"indices", Json::array({0, 1, 5})}}).has_value());
    REQUIRE_FALSE(s.command("mesh.create", Json{{"name", "no/slash"}, {"positions", Json::array({0, 0, 0, 1, 0, 0, 0, 1, 0})}}).has_value());
    REQUIRE(s.finish().has_value());
    app::Session t(hello_options(1000));
    REQUIRE(t.start().has_value());
    REQUIRE(t.command("world.load", Json{{"scene", scene}}).has_value());
    REQUIRE(t.command("mesh.list", Json::object()).value()["meshes"][0]["mesh"] == "mesh:floor");
    REQUIRE(t.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(t.command("world.get", Json{{"entity", "Made"}, {"component", "Bounds"}}).value()["max"]["x"].get<double>() == Catch::Approx(42).margin(0.01));
    REQUIRE(t.command("mesh.remove", Json{{"name", "floor"}}).has_value());
    REQUIRE(t.finish().has_value());
}

TEST_CASE("a Blender level: custom properties become components, a node's mesh collides in its place, glass and lacquer come along", "[runtime][import][blenderlevel]") {
    const std::string blender = assets::find_blender("");
    if (blender.empty()) SKIP("Blender is not installed");
    const std::filesystem::path dir = root() / "samples" / "assets" / "assets" / "import-test";
    const std::filesystem::path out = root() / "build" / "test-out";
    std::filesystem::create_directories(dir);
    std::filesystem::create_directories(out);
    // A floor that is level geometry, a pillar with a box collider and health, a pane of glass, a
    // lacquered ball, and an empty asking for a component there is none of, all set in Blender's
    // custom properties (Object Properties > Custom Properties).
    std::ofstream(out / "make-level.py") << R"PY(
import bpy, sys
argv = sys.argv[sys.argv.index('--') + 1:]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.mesh.primitive_plane_add(size=10, location=(0, 0, 1))
floor = bpy.context.active_object
floor.name = 'Floor'
floor['pocket'] = '{"RigidBody": {"kind": 1}, "Collider": {"shape": 3}}'
bpy.ops.mesh.primitive_cube_add(size=1, location=(3, 0, 2.5))
pillar = bpy.context.active_object
pillar.name = 'Pillar'
pillar.scale = (1, 1, 3)
pillar['pocket.RigidBody'] = '{"kind": 1}'
pillar['pocket.Collider'] = '{"shape": 0, "size": {"x": 0.5, "y": 1.5, "z": 0.5}}'
pillar['pocket.Health'] = '{"max": 50, "current": 50}'
pillar['note'] = 'not ours'
def principled(name):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    return m, m.node_tree.nodes['Principled BSDF']
def set_input(b, names, value):
    for n in names:
        if n in b.inputs:
            b.inputs[n].default_value = value
            return
bpy.ops.mesh.primitive_cube_add(size=1, location=(-2, -2, 2))
pane = bpy.context.active_object
pane.name = 'Pane'
pane.scale = (1.5, 0.05, 1)
glass, b = principled('Glass')
set_input(b, ['Transmission Weight', 'Transmission'], 1.0)
set_input(b, ['IOR'], 1.45)
set_input(b, ['Roughness'], 0.05)
pane.data.materials.append(glass)
bpy.ops.mesh.primitive_uv_sphere_add(radius=0.5, location=(-2, 2, 1.5))
ball = bpy.context.active_object
ball.name = 'Lacquered'
paint, b = principled('CarPaint')
set_input(b, ['Base Color'], (0.6, 0.02, 0.02, 1))
set_input(b, ['Coat Weight', 'Clearcoat'], 1.0)
set_input(b, ['Coat Roughness', 'Clearcoat Roughness'], 0.05)
ball.data.materials.append(paint)
velvet, b = principled('Velvet')
set_input(b, ['Sheen Weight', 'Sheen'], 1.0)
set_input(b, ['Sheen Roughness'], 0.4)
floor.data.materials.append(velvet)
brushed, b = principled('Brushed')
set_input(b, ['Metallic'], 1.0)
set_input(b, ['Anisotropic'], 0.8)
set_input(b, ['Specular IOR Level'], 0.25)
pillar.data.materials.append(brushed)
bpy.ops.object.empty_add(location=(0, 3, 1))
marker = bpy.context.active_object
marker.name = 'Marker'
marker['pocket.Lamp'] = '{}'
bpy.ops.wm.save_as_mainfile(filepath=argv[0])
)PY";
    const std::string cmd = "'" + blender + "' -b --factory-startup --python '" + (out / "make-level.py").string() + "' -- '" + (dir / "level.blend").string() + "' > '" + (out / "make-level.log").string() + "' 2>&1";
    REQUIRE(std::system(cmd.c_str()) == 0);
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The file as the store sees it: the custom properties by object, the glass and the lacquer.
    const Json d = s.command("assets.describe", Json{{"path", "assets/import-test/level.blend"}}).value();
    INFO(d.dump());
    REQUIRE(d["properties"]["Floor"]["pocket"].is_string());
    REQUIRE(d["tangents"] == "file");   // Blender's own tangents, as its normal maps were baked against
    REQUIRE(d["properties"]["Pillar"]["note"] == "not ours");
    double transmission = 0, ior = 0, coat = 0;
    for (const Json& m : d["materials"]) {
        if (m["name"] == "Glass") { transmission = m.value("transmission", 0.0); ior = m.value("ior", 0.0); }
        if (m["name"] == "CarPaint") coat = m.value("clearcoat", 0.0);
    }
    REQUIRE(transmission == Catch::Approx(1.0));
    REQUIRE(ior == Catch::Approx(1.45).margin(0.01));
    REQUIRE(coat == Catch::Approx(1.0));
    // Cloth and brushed metal (Blender's Sheen, Anisotropic and Specular IOR Level).
    Json velvet, brushed;
    for (const Json& m : d["materials"]) {
        if (m["name"] == "Velvet") velvet = m;
        if (m["name"] == "Brushed") brushed = m;
    }
    REQUIRE(velvet.contains("sheen"));
    REQUIRE(velvet["sheen"]["color"][0].get<double>() > 0.5);
    REQUIRE(velvet["sheen"]["roughness"].get<double>() == Catch::Approx(0.4).margin(0.01));
    REQUIRE(brushed.contains("anisotropy"));
    REQUIRE(brushed["anisotropy"]["strength"].get<double>() == Catch::Approx(0.8).margin(0.01));
    REQUIRE(brushed.contains("specular"));
    REQUIRE(brushed["specular"]["factor"].get<double>() < 0.9);
    // Instantiated far from the sample's own things: the components from the properties, and a
    // warning for the one that names nothing.
    Json inst = s.command("world.instantiate", Json{{"mesh", "assets/import-test/level.blend"}, {"position", Json{{"x", 60}, {"y", 0}, {"z", 0}}}}).value();
    INFO(inst.dump());
    REQUIRE(inst["warnings"].size() == 1);
    REQUIRE(inst["warnings"][0].get<std::string>().find("Lamp") != std::string::npos);
    const Json root_id = inst["roots"][0];
    auto child = [&](const char* name) {
        for (const Json& c : s.command("world.children", Json{{"entity", root_id}}).value())
            if (s.command("world.describe", Json{{"entity", c}}).value().value("name", "") == name) return c;
        return Json(nullptr);
    };
    const Json floor = child("Floor"), pillar = child("Pillar");
    REQUIRE(!floor.is_null());
    REQUIRE(!pillar.is_null());
    REQUIRE(s.command("world.get", Json{{"entity", floor}, {"component", "RigidBody"}}).value()["kind"] == 1);
    REQUIRE(s.command("world.get", Json{{"entity", floor}, {"component", "Collider"}}).value()["shape"] == 3);
    REQUIRE(s.command("world.get", Json{{"entity", floor}, {"component", "MeshRenderer"}}).value()["node"] == "Floor");
    REQUIRE(s.command("world.get", Json{{"entity", pillar}, {"component", "Health"}}).value()["max"].get<double>() == Catch::Approx(50));
    REQUIRE(s.command("world.get", Json{{"entity", pillar}, {"component", "Collider"}}).value()["size"]["y"].get<double>() == Catch::Approx(1.5));
    // Balls dropped at the level's place: one lands on the floor's own triangles (a unit up, where
    // the file puts it, the model at x 60), one on the pillar's box (4 up).
    auto ball = [&](const char* name, double x, double z) {
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", 8}, {"z", z}}}}}, {"MeshRenderer", Json{{"mesh", "sphere"}}}, {"RigidBody", Json{{"kind", 0}, {"mass", 1}}}, {"Collider", Json{{"shape", 1}, {"size", Json{{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}}}}).has_value());
    };
    ball("OnFloor", 58, 1);    // Blender's y is the engine's -z
    ball("OnPillar", 63, 0);
    for (int i = 0; i < 180; ++i) REQUIRE(s.frame().has_value());
    // The floor's collider keeps its own plane (two triangles, four corners), not the whole file.
    const Json stats = s.command("physics.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["meshes"] == 1);
    REQUIRE(stats["triangles"] == 2);
    REQUIRE(stats["mesh_vertices"] == 4);
    // Live models: the root remembers its file. A second copy is told not to follow it; then the
    // level is changed in Blender (the pillar moved to the other side, a crate with 20 health
    // added) and the store reloaded: the live copy is made again in place, the frozen one is not.
    const Json model = s.command("world.get", Json{{"entity", root_id}, {"component", "Model"}}).value();
    REQUIRE(model["path"] == "assets/import-test/level.blend");
    REQUIRE(model["live"] == true);
    REQUIRE(model["hash"].get<std::string>().size() == 16);
    const Json frozen = s.command("world.instantiate", Json{{"mesh", "assets/import-test/level.blend"}, {"position", Json{{"x", -60}, {"y", 0}, {"z", 0}}}, {"name", "Frozen"}, {"components", Json{{"Model", Json{{"live", false}}}}}}).value()["roots"][0];
    REQUIRE(s.command("assets.reload", Json::object()).value().contains("relinked") == false);   // nothing changed yet
    std::ofstream(out / "change-level.py") << R"PY(
import bpy, sys
argv = sys.argv[sys.argv.index('--') + 1:]
bpy.ops.wm.open_mainfile(filepath=argv[0])
bpy.data.objects['Pillar'].location = (-3, 0, 2.5)
bpy.ops.mesh.primitive_cube_add(size=1, location=(0, -3, 1.5))
crate = bpy.context.active_object
crate.name = 'Crate'
crate['pocket.Health'] = '{"max": 20, "current": 20}'
bpy.ops.wm.save_as_mainfile(filepath=argv[0])
)PY";
    const std::string change = "'" + blender + "' -b --factory-startup --python '" + (out / "change-level.py").string() + "' -- '" + (dir / "level.blend").string() + "' > '" + (out / "change-level.log").string() + "' 2>&1";
    REQUIRE(std::system(change.c_str()) == 0);
    const std::uint64_t seq = s.command("events.last_seq", Json::object()).value()["seq"].get<std::uint64_t>();
    const Json reloaded = s.command("assets.reload", Json::object()).value();
    INFO(reloaded.dump());
    REQUIRE(reloaded["relinked"].size() == 1);
    REQUIRE(reloaded["relinked"][0]["entity"] == root_id);
    REQUIRE(s.command("world.get", Json{{"entity", root_id}, {"component", "Model"}}).value()["hash"] != model["hash"]);
    const Json crate = child("Crate");
    REQUIRE(!crate.is_null());
    REQUIRE(s.command("world.get", Json{{"entity", crate}, {"component", "Health"}}).value()["max"].get<double>() == Catch::Approx(20));
    REQUIRE(s.command("world.get", Json{{"entity", child("Pillar")}, {"component", "Transform"}}).value()["position"]["x"].get<double>() == Catch::Approx(-3).margin(1e-3));
    bool relinked_event = false;
    for (const Json& e : s.command("events.since", Json{{"seq", seq}}).value()["events"]) relinked_event = relinked_event || e["type"] == "model.relinked";
    REQUIRE(relinked_event);
    bool frozen_crate = false;
    for (const Json& c : s.command("world.children", Json{{"entity", frozen}}).value())
        frozen_crate = frozen_crate || s.command("world.describe", Json{{"entity", c}}).value().value("name", "") == "Crate";
    REQUIRE_FALSE(frozen_crate);
    const double on_floor = s.command("world.get", Json{{"entity", "OnFloor"}, {"component", "Transform"}}).value()["position"]["y"].get<double>();
    const double on_pillar = s.command("world.get", Json{{"entity", "OnPillar"}, {"component", "Transform"}}).value()["position"]["y"].get<double>();
    INFO("on the floor at " << on_floor << ", on the pillar at " << on_pillar);
    REQUIRE(on_floor == Catch::Approx(1.5).margin(0.05));
    REQUIRE(on_pillar == Catch::Approx(4.5).margin(0.05));
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(dir / "level.blend");
    std::filesystem::remove(dir / "level.blend1");   // Blender's backup of the file it saved over
    std::filesystem::remove_all(root() / "samples" / "assets" / ".imported" / "assets" / "import-test" / "level.blend.glb");
    std::filesystem::remove(root() / "samples" / "assets" / ".imported" / "assets" / "import-test" / "level.blend.glb.stamp");
    std::filesystem::remove(root() / "samples" / "assets" / ".imported" / "assets" / "import-test" / "level.blend.glb.log");
    std::error_code ec;
    std::filesystem::remove(dir, ec);   // only when nothing else is in it
}

TEST_CASE("Blender files, FBX and OBJ come in as scenes: converted once, instantiated with their lights and cameras", "[runtime][import]") {
    const std::string blender = assets::find_blender("");
    if (blender.empty()) SKIP("Blender is not installed");
    const std::filesystem::path dir = root() / "samples" / "assets" / "assets" / "import-test";
    const std::filesystem::path out = root() / "build" / "test-out";
    std::filesystem::create_directories(dir);
    std::filesystem::create_directories(out);
    // A Blender scene made by Blender: a beveled red cube, a warm point lamp and a camera, saved as
    // .blend and exported as .fbx.
    std::ofstream(out / "make-scene.py") << R"PY(
import bpy, sys
argv = sys.argv[sys.argv.index('--') + 1:]
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.mesh.primitive_cube_add(size=1.0, location=(0, 0, 0.5))
crate = bpy.context.active_object
crate.name = 'Crate'
mat = bpy.data.materials.new('RedPaint')
mat.use_nodes = True
bsdf = mat.node_tree.nodes['Principled BSDF']
bsdf.inputs['Base Color'].default_value = (0.8, 0.05, 0.05, 1)
bsdf.inputs['Roughness'].default_value = 0.3
crate.data.materials.append(mat)
bpy.ops.object.modifier_add(type='BEVEL')
crate.modifiers[-1].width = 0.1
bpy.ops.object.light_add(type='POINT', location=(2, -2, 3))
bpy.context.active_object.name = 'Lamp'
bpy.context.active_object.data.energy = 1000
bpy.ops.object.camera_add(location=(0, -6, 2))
bpy.context.active_object.name = 'Eye'
bpy.ops.object.light_add(type='SPOT', location=(-2, 0, 4))
bpy.context.active_object.name = 'Beam'
bpy.context.active_object.data.energy = 500
bpy.context.active_object.data.spot_size = 0.6981317   # forty degrees across
bpy.ops.wm.save_as_mainfile(filepath=argv[0])
bpy.ops.export_scene.fbx(filepath=argv[1])
)PY";
    const std::string cmd = "'" + blender + "' -b --factory-startup --python '" + (out / "make-scene.py").string() + "' -- '" + (dir / "scene.blend").string() + "' '" + (dir / "scene.fbx").string() + "' > '" + (out / "make-scene.log").string() + "' 2>&1";
    REQUIRE(std::system(cmd.c_str()) == 0);
    REQUIRE(std::filesystem::is_regular_file(dir / "scene.blend"));
    std::ofstream(dir / "post.obj") << "o Post\nv 0 0 0\nv 0.2 0 0\nv 0.2 2 0\nv 0 2 0\nf 1 2 3 4\n";
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The first import runs Blender; the same content again is the cached conversion.
    Json first = s.command("assets.import", Json{{"path", "assets/import-test/scene.blend"}}).value();
    INFO(first.dump());
    REQUIRE(first["importer"] == "blender");
    REQUIRE(first["cached"] == false);
    REQUIRE(first["blender_found"] == true);
    REQUIRE(first["converted"] == ".imported/assets/import-test/scene.blend.glb");
    REQUIRE(first["mesh"]["lights"].size() == 2);
    REQUIRE(first["mesh"]["cameras"].size() == 1);
    REQUIRE(first["mesh"]["materials"][0]["name"] == "RedPaint");
    REQUIRE(first["mesh"]["materials"][0]["roughness"].get<double>() == Catch::Approx(0.3).margin(0.01));
    REQUIRE(first["mesh"]["vertices"].get<int>() > 24);   // the bevel modifier was applied
    Json again = s.command("assets.import", Json{{"path", "assets/import-test/scene.blend"}}).value();
    REQUIRE(again["cached"] == true);
    // Instantiated: the file's objects as entities, the lamp a point light, the camera inactive.
    Json inst = s.command("world.instantiate", Json{{"mesh", "assets/import-test/scene.blend"}}).value();
    const Json root_id = inst["roots"][0];
    auto child = [&](const char* name) {
        for (const Json& c : s.command("world.children", Json{{"entity", root_id}}).value()) {
            if (s.command("world.describe", Json{{"entity", c}}).value().value("name", "") == name) return c;
        }
        return Json(nullptr);
    };
    const Json crate = child("Crate"), lamp = child("Lamp"), eye = child("Eye");
    REQUIRE(!crate.is_null());
    REQUIRE(!lamp.is_null());
    REQUIRE(!eye.is_null());
    Json light = s.command("world.get", Json{{"entity", lamp}, {"component", "Light"}}).value();
    REQUIRE(light["kind"] == 1);
    REQUIRE(light["intensity"].get<double>() > 1.0);
    // The spot keeps its cone: forty degrees across is twenty each side of its axis, pointing down.
    const Json beam = child("Beam");
    REQUIRE(!beam.is_null());
    Json spot = s.command("world.get", Json{{"entity", beam}, {"component", "Light"}}).value();
    INFO(spot.dump());
    REQUIRE(spot["kind"] == 2);
    REQUIRE(spot["outer_angle"].get<double>() == Catch::Approx(20.0).margin(0.1));
    REQUIRE(spot["inner_angle"].get<double>() < spot["outer_angle"].get<double>());
    Json cam = s.command("world.get", Json{{"entity", eye}, {"component", "Camera"}}).value();
    REQUIRE(cam["active"] == false);
    // Blender's Z up is Y up here: the lamp three units above the ground.
    Json lamp_t = s.command("world.get", Json{{"entity", lamp}, {"component", "Transform"}}).value();
    REQUIRE(lamp_t["position"]["y"].get<double>() == Catch::Approx(3.0).margin(1e-3));
    REQUIRE(lamp_t["position"]["z"].get<double>() == Catch::Approx(2.0).margin(1e-3));
    REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    for (const Json& m : stats["assets"].value("missing", Json::array())) REQUIRE(m.get<std::string>().find("import-test") == std::string::npos);   // the sample's own Missing entity aside
    // An FBX goes the same way, and an OBJ needs no Blender at all.
    Json fbx = s.command("assets.import", Json{{"path", "assets/import-test/scene.fbx"}}).value();
    REQUIRE(fbx["importer"] == "blender");
    REQUIRE(fbx["mesh"]["materials"][0]["name"] == "RedPaint");
    Json obj = s.command("world.instantiate", Json{{"mesh", "assets/import-test/post.obj"}}).value();
    REQUIRE(obj["roots"].size() == 1);
    REQUIRE(s.command("assets.describe", Json{{"path", "assets/import-test/post.obj"}}).value()["importer"] == "obj");
    REQUIRE(s.finish().has_value());
    std::filesystem::remove_all(dir);
    std::filesystem::remove_all(root() / "samples" / "assets" / ".imported");
}

TEST_CASE("the engine says how to call its commands and refuses parameters they do not take", "[runtime][help]") {
    app::Session s(hello_options(10));
    REQUIRE(s.start().has_value());
    // Every command it lists has help, with a usage line and a summary.
    const Json listed = s.command("commands", Json::object()).value();
    for (const Json& c : listed) {
        auto h = s.command("help", Json{{"command", c}});
        INFO(c.dump());
        REQUIRE(h.has_value());
        REQUIRE_FALSE((*h)["summary"].get<std::string>().empty());
    }
    // The SDK's exports, generated from its sources: one by name, those holding a text, the parts.
    Json after = s.command("help", Json{{"sdk", "timer.after"}}).value();
    REQUIRE(after["signature"] == "timer.after(seconds: number, fn: () => void): TimerHandle");
    REQUIRE(after["file"] == "sdk/runtime/timer.ts");
    REQUIRE_FALSE(after["doc"].get<std::string>().empty());
    REQUIRE(s.command("help", Json{{"sdk", "RayHit"}}).value()["signature"].get<std::string>().starts_with("interface RayHit { entity: Entity;"));
    REQUIRE(s.command("help", Json{{"sdk", "timer"}}).value()["exports"].size() >= 4);
    REQUIRE(s.command("help", Json{{"sdk", ""}}).value()["parts"]["world"].size() > 10);
    REQUIRE_FALSE(s.command("help", Json{{"sdk", "nothing.like.this"}}).has_value());
    Json set = s.command("help", Json{{"command", "world.set"}}).value();
    REQUIRE(set["usage"] == "world.set {entity, component, value, cause?, quiet?}");
    REQUIRE(set["params"] == Json::array({"entity", "component", "value", "cause", "quiet"}));
    Json usage = s.command("commands", Json{{"usage", true}}).value();
    REQUIRE(usage.size() == listed.size());
    // Narrowed for an agent's context: a family, a word, one line each, one component's schema.
    const Json world_family = s.command("commands", Json{{"family", "world"}}).value();
    REQUIRE(world_family.size() > 20);
    for (const Json& c : world_family) REQUIRE(c.get<std::string>().starts_with("world."));
    REQUIRE(s.command("commands", Json{{"search", "raycast"}}).value() == Json::array({"physics.raycast", "physics2d.raycast"}));
    // Without a family or a word, the text is an index of the names by family; with one, a line a command.
    const std::string text = s.command("commands", Json{{"text", true}}).value()["text"].get<std::string>();
    REQUIRE(text.size() < 4000);
    for (const Json& c : listed) {
        const std::string n = c.get<std::string>();
        REQUIRE(text.find(n.substr(n.find('.') + 1)) != std::string::npos);
    }
    const std::string world_text = s.command("commands", Json{{"text", true}, {"family", "world"}}).value()["text"].get<std::string>();
    REQUIRE(std::count(world_text.begin(), world_text.end(), '\n') == static_cast<long>(world_family.size()) + 1);
    const Json light = s.command("world.schema", Json{{"component", "Light"}}).value();
    REQUIRE(light["components"].size() == 1);
    REQUIRE(light.dump().size() < 4000);
    REQUIRE_FALSE(s.command("world.schema", Json{{"component", "Lite"}}).has_value());
    REQUIRE(s.command("help", Json{{"family", "nav"}}).value()["commands"].size() >= 5);
    // A mistyped command is answered with the names it is close to.
    auto typo = s.command("world.spwan", Json::object());
    REQUIRE_FALSE(typo.has_value());
    REQUIRE(typo.error().message.find("world.spawn") != std::string::npos);
    // A key a command does not take is refused, with the ones it takes, instead of being ignored.
    auto wrong = s.command("world.set", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}, {"fields", Json{{"color", Json{{"r", 0}, {"g", 1}, {"b", 0}, {"a", 1}}}}}});
    REQUIRE_FALSE(wrong.has_value());
    INFO(wrong.error().message);
    REQUIRE(wrong.error().code == "bad_args");
    REQUIRE(wrong.error().message.find("'fields'") != std::string::npos);
    REQUIRE(wrong.error().message.find("entity, component, value") != std::string::npos);
    auto pattern = s.command("world.query", Json{{"pattern", "*Ball*"}});
    REQUIRE_FALSE(pattern.has_value());
    REQUIRE(pattern.error().message.find("name?") != std::string::npos);
    // world.set without a value, or without a component, says what it needs.
    auto empty = s.command("world.set", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}});
    REQUIRE_FALSE(empty.has_value());
    REQUIRE(empty.error().message.find("value") != std::string::npos);
    // `path` stands for `entity` where a command takes an entity.
    Json described = s.command("world.describe", Json{{"path", "Ball"}}).value();
    REQUIRE(described["name"] == "Ball");
    REQUIRE(s.command("world.set", Json{{"path", "/Ball"}, {"component", "MeshRenderer"}, {"value", Json{{"color", Json{{"r", 0}, {"g", 1}, {"b", 0}, {"a", 1}}}}}}).has_value());
    REQUIRE(s.command("world.get", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}}).value()["color"]["g"].get<double>() == Catch::Approx(1.0));
    // A command whose parameters run on into another's (env.step) is not held to the list.
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a script error stops the simulation, says so in state and in step, and a reload starts over", "[runtime][script_error]") {
    app::Session s(hello_options(1000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    // The hello script colors the Ball every tick: without the Ball, its next tick throws.
    REQUIRE(s.command("world.destroy", Json{{"entity", "Ball"}}).has_value());
    (void)s.frame();
    Json st = s.command("state", Json::object()).value();
    INFO(st.dump());
    REQUIRE(st["ok"] == false);
    REQUIRE(st["errors"].size() >= 1);
    REQUIRE(st["errors"].back()["message"].get<std::string>().find("no entity") != std::string::npos);
    REQUIRE(st.contains("hint"));
    auto step = s.command("step", Json{{"ticks", 1}});
    REQUIRE_FALSE(step.has_value());
    REQUIRE(step.error().code == "script_error");
    REQUIRE(s.command("resume", Json::object()).error().code == "script_error");
    // Commands still answer: the world can be read and fixed.
    REQUIRE(s.command("world.find", Json{{"path", "Ball"}}).value().is_null());
    Json reloaded = s.command("project.reload", Json::object()).value();
    REQUIRE(reloaded["ok"] == true);
    REQUIRE(s.command("state", Json::object()).value()["ok"] == true);
    REQUIRE(s.command("step", Json{{"ticks", 2}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a spatial source rises in pitch coming at the listener and falls going away (Doppler)", "[runtime][audio][doppler]") {
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
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"rotation", {{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Car"}, {"components", Json{{"Transform", Json{{"position", {{"x", 0}, {"y", 0}, {"z", -15}}}}}, {"AudioSource", Json{{"clip", "assets/hum.wav"}, {"autoplay", true}, {"loop", true}, {"spatial", true}, {"near", 1.0}, {"range", 40.0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    auto pitch = [&]() {
        for (const Json& v : s.command("audio.list", Json::object()).value()) if (v["clip"] == "assets/hum.wav" && v["entity"] != 0) return v["pitch"].get<double>();
        return -1.0;
    };
    REQUIRE(pitch() == Catch::Approx(1.0).margin(1e-3));   // standing still
    // Thirty units a second (half a unit a tick) toward the camera, then away.
    double z = -15;
    auto drive = [&](double step, int ticks) {
        for (int i = 0; i < ticks; ++i) {
            z += step;
            REQUIRE(s.command("world.set", Json{{"entity", "Car"}, {"component", "Transform"}, {"value", Json{{"position", {{"x", 0}, {"y", 0}, {"z", z}}}}}}).has_value());
            REQUIRE(s.frame().has_value());
        }
    };
    drive(0.5, 4);
    const double coming = pitch();
    drive(-0.5, 4);
    const double going = pitch();
    INFO("coming " << coming << ", going " << going);
    REQUIRE(coming == Catch::Approx(343.0 / 313.0).margin(0.01));
    REQUIRE(going == Catch::Approx(343.0 / 373.0).margin(0.01));
    // Parked: the pitch returns; with doppler 0 motion leaves it alone.
    drive(0.0, 2);
    REQUIRE(pitch() == Catch::Approx(1.0).margin(1e-3));
    REQUIRE(s.command("world.set", Json{{"entity", "Car"}, {"component", "AudioSource"}, {"value", Json{{"doppler", 0.0}}}}).has_value());
    drive(0.5, 3);
    REQUIRE(pitch() == Catch::Approx(1.0).margin(1e-3));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("assets.preview draws a model on its own without touching the scene's frame", "[runtime][assets][preview]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "error";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    const Json before = s.command("render.stats", Json::object()).value();
    const std::filesystem::path out = root() / "build" / "test-out" / "crate-preview.png";
    std::filesystem::remove(out);
    Json p = s.command("assets.preview", Json{{"path", "assets/crate.glb"}, {"size", 96}, {"out", out.string()}, {"image", true}}).value();
    INFO(p.dump().substr(0, 400));
    REQUIRE(p["width"] == 96);
    REQUIRE(p["vertices"].get<int>() > 0);
    REQUIRE(p["png"].get<std::string>().rfind("iVBORw0KGgo", 0) == 0);   // a PNG's signature, in base64
    REQUIRE(std::filesystem::is_regular_file(out));
    auto bytes = fs::read_text(out);
    REQUIRE(bytes.has_value());
    auto img = assets::decode_image(*bytes, "preview");
    REQUIRE(img.has_value());
    REQUIRE(img->width == 96);
    // The model fills the middle; the corners are the background.
    auto at = [&](std::uint32_t x, std::uint32_t y) { const std::size_t i = (static_cast<std::size_t>(y) * img->width + x) * 4; return std::array<int, 3>{img->rgba[i], img->rgba[i + 1], img->rgba[i + 2]}; };
    const auto corner = at(2, 2), middle = at(48, 52);
    INFO("corner " << corner[0] << "," << corner[1] << "," << corner[2] << " middle " << middle[0] << "," << middle[1] << "," << middle[2]);
    REQUIRE(std::abs(middle[0] - corner[0]) + std::abs(middle[1] - corner[1]) + std::abs(middle[2] - corner[2]) > 40);
    // The scene's renderer did not see any of it.
    REQUIRE(s.frame().has_value());
    const Json after = s.command("render.stats", Json::object()).value();
    REQUIRE(after["camera"] == before["camera"]);
    REQUIRE(after["instances"] == before["instances"]);
    REQUIRE(after["draw_calls"] == before["draw_calls"]);
    // A path that is not a model is refused.
    REQUIRE_FALSE(s.command("assets.preview", Json{{"path", "assets/nothing.glb"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a pad opens the UI sample's menu, sets its volume and difficulty, and closes it", "[runtime][ui][pad]") {
    app::Options o;
    o.project_dir = root() / "samples" / "ui";
    o.bundle = root() / "build" / "ts" / "ui.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 960;
    o.height = 540;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    auto press = [&](const char* button) {
        REQUIRE(s.command("input.pad", Json{{"pad", 0}, {"button", button}, {"pressed", true}}).has_value());
        REQUIRE(s.command("input.pad", Json{{"pad", 0}, {"button", button}, {"pressed", false}}).has_value());
        REQUIRE(s.frame().has_value());
    };
    auto state = [&]() { return s.command("state", Json::object()).value()["state"]; };
    // Start opens the menu; its volume slider takes the focus as it appears.
    press("start");
    REQUIRE(state()["paused"] == true);
    press("dpad_left");
    press("dpad_left");
    INFO(state().dump());
    REQUIRE(state()["volume"].get<double>() == Catch::Approx(0.9));
    for (const Json& b : s.command("audio.buses", Json::object()).value()) if (b["name"] == "main") REQUIRE(b["volume"].get<double>() == Catch::Approx(0.9));
    // Down to the difficulty, Right steps it; down to the checkbox, A flips it.
    press("dpad_down");
    press("dpad_right");
    REQUIRE(state()["difficulty"] == "hard");
    press("dpad_down");
    press("a");
    REQUIRE(state()["show_bar"] == false);
    // B leaves the menu and the game closes it.
    press("b");
    REQUIRE(state()["paused"] == false);
    REQUIRE(s.command("ui.query", Json{{"name", "pause-menu"}}).value().empty());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("project.reload reads project.toml's settings again: the input map, buses and render settings", "[runtime][reload][settings]") {
    // The settings as the tool bundles them, in a copy the test edits the way `pocket ts` would rewrite it.
    const std::filesystem::path config = root() / "build" / "test-out" / "reload-settings.project.json";
    std::filesystem::create_directories(config.parent_path());
    Json settings = Json::parse(std::ifstream(root() / "build" / "ts" / "hello.js.project.json"));
    settings["input"] = Json{{"actions", Json{{"jump", Json::array({"Space"})}}}};
    std::ofstream(config) << settings.dump(2);
    app::Options o;
    o.project_dir = root() / "samples" / "hello";
    o.bundle = root() / "build" / "ts" / "hello.js";
    o.project_config = config;
    o.headless = true;
    o.frames = 100;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("input.describe", Json::object()).value().contains("jump"));
    REQUIRE_FALSE(s.command("input.describe", Json::object()).value().contains("dash"));
    // An action added, a bus and a render setting changed in the file: a reload applies them.
    settings["input"]["actions"]["dash"] = Json::array({"LShift"});
    settings["audio"] = Json{{"buses", Json{{"music", Json{{"volume", 0.4}}}}}};
    settings["render"] = Json{{"bloom", true}};
    std::ofstream(config) << settings.dump(2);
    Json r = s.command("project.reload", Json::object()).value();
    REQUIRE(r["settings"] == true);
    Json actions = s.command("input.describe", Json::object()).value();
    INFO(actions.dump());
    REQUIRE(actions.contains("dash"));
    REQUIRE(actions.contains("jump"));
    bool music = false;
    for (const Json& b : s.command("audio.buses", Json::object()).value()) if (b["name"] == "music") { music = true; REQUIRE(b["volume"].get<double>() == Catch::Approx(0.4)); }
    REQUIRE(music);
    REQUIRE(s.command("render.bloom", Json::object()).value()["enabled"] == true);
    // settings: false keeps what is running.
    settings["input"]["actions"].erase("dash");
    std::ofstream(config) << settings.dump(2);
    REQUIRE(s.command("project.reload", Json{{"settings", false}}).value()["settings"] == false);
    REQUIRE(s.command("input.describe", Json::object()).value().contains("dash"));
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(config);
}

TEST_CASE("a terrain is drawn, stood on and collided with; sculpting reshapes it under the player; it saves as a heightmap", "[runtime][terrain]") {
    const std::filesystem::path saved = root() / "samples" / "hills" / "assets" / "test-heights.png";
    std::filesystem::remove(saved);
    app::Options o;
    o.project_dir = root() / "samples" / "hills";
    o.bundle = root() / "build" / "ts" / "hills.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    auto state = [&]() { return s.command("state", Json::object()).value()["state"]; };
    Json info = s.command("terrain.info", Json::object()).value();
    INFO(info.dump());
    REQUIRE(info["source"] == "noise");
    REQUIRE(info["resolution"] == 129);
    REQUIRE(info["highest"].get<double>() == Catch::Approx(12));
    // Drawn as a mesh the engine made, and a mesh collider of its triangles.
    Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["assets"]["meshes"].get<int>() >= 1);
    Json ph = s.command("physics.stats", Json::object()).value();
    REQUIRE(ph["meshes"].get<int>() >= 1);
    REQUIRE(ph["triangles"].get<int>() >= 128 * 128 * 2);
    // The player stands on it: its centre half its height above the ground under it (a little more
    // on a slope, where the round foot touches the ground uphill of the centre).
    Json st = state();
    INFO(st.dump());
    REQUIRE(st["player.grounded"] == true);
    REQUIRE(st["player.y"].get<double>() - st["ground.y"].get<double>() > 0.89);
    REQUIRE(st["player.y"].get<double>() - st["ground.y"].get<double>() < 1.0);
    // A ray down from above finds the ground where terrain.height says it is.
    const double px = st["player.x"].get<double>() + 3, pz = st["player.z"].get<double>();
    Json ground = s.command("terrain.height", Json{{"x", px}, {"z", pz}}).value();
    Json hit = s.command("physics.raycast", Json{{"origin", Json{{"x", px}, {"y", 50}, {"z", pz}}}, {"direction", Json{{"x", 0}, {"y", -1}, {"z", 0}}}}).value();
    REQUIRE(hit["entity"] == info["entity"]);
    REQUIRE(hit["point"]["y"].get<double>() == Catch::Approx(ground["height"].get<double>()).margin(0.01));
    // Raising the ground under the player lifts the player with it.
    const double before = st["player.y"].get<double>();
    Json sculpt = s.command("terrain.sculpt", Json{{"x", st["player.x"]}, {"z", st["player.z"]}, {"radius", 4}, {"amount", 1.5}}).value();
    REQUIRE(sculpt["samples"].get<int>() > 10);
    REQUIRE(sculpt["revision"].get<int>() == info["revision"].get<int>() + 1);
    for (int i = 0; i < 20; ++i) REQUIRE(s.frame().has_value());
    st = state();
    REQUIRE(st["player.y"].get<double>() == Catch::Approx(before + 1.5).margin(0.1));
    REQUIRE(st["player.grounded"] == true);
    REQUIRE(s.command("terrain.info", Json::object()).value()["edited"] == true);
    REQUIRE(s.command("events.recent", Json{{"n", 50}, {"type", "terrain.sculpted"}}).value().size() == 1);
    // Flatten toward a height; smooth; bad modes are refused.
    Json flat = s.command("terrain.sculpt", Json{{"x", px + 10}, {"z", pz}, {"radius", 3}, {"mode", "flatten"}, {"target", 6}, {"amount", 1}}).value();
    REQUIRE(flat["height"].get<double>() == Catch::Approx(6).margin(0.05));
    REQUIRE(s.command("terrain.sculpt", Json{{"x", px}, {"z", pz}, {"mode", "smooth"}}).has_value());
    REQUIRE_FALSE(s.command("terrain.sculpt", Json{{"x", px}, {"z", pz}, {"mode", "dig"}}).has_value());
    // Saved as a 16-bit heightmap, the terrain reads its heights back from it, sculpting kept.
    const double lifted = s.command("terrain.height", Json{{"x", st["player.x"]}, {"z", st["player.z"]}}).value()["height"].get<double>();
    REQUIRE(s.command("terrain.save", Json{{"path", "assets/test-heights.png"}}).has_value());
    REQUIRE(std::filesystem::exists(saved));
    info = s.command("terrain.info", Json::object()).value();
    REQUIRE(info["source"] == "assets/test-heights.png");
    REQUIRE(s.command("world.get", Json{{"entity", "Hills"}, {"component", "Terrain"}}).value()["heightmap"] == "assets/test-heights.png");
    REQUIRE(s.command("terrain.height", Json{{"x", st["player.x"]}, {"z", st["player.z"]}}).value()["height"].get<double>() == Catch::Approx(lifted).margin(0.002));
    // Back to noise: the heightmap cleared, the hills as they were.
    REQUIRE(s.command("world.set", Json{{"entity", "Hills"}, {"component", "Terrain"}, {"value", Json{{"heightmap", ""}}}}).has_value());
    REQUIRE(s.command("terrain.height", Json{{"x", st["player.x"]}, {"z", st["player.z"]}}).value()["height"].get<double>() == Catch::Approx(before - 0.9).margin(0.06));
    REQUIRE_FALSE(s.command("terrain.info", Json{{"entity", "Player"}}).has_value());
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(saved);
}

TEST_CASE("a Scatter strews copies over the ground within its limits, the same every run, and follows the ground when it is sculpted", "[runtime][terrain][scatter]") {
    app::Options o;
    o.project_dir = root() / "samples" / "hills";
    o.bundle = root() / "build" / "ts" / "hills.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    auto run = [&](auto&& body) {
        app::Session s(o);
        REQUIRE(s.start().has_value());
        for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
        body(s);
        REQUIRE(s.finish().has_value());
    };
    Json first;
    run([&](app::Session& s) {
        Json bushes = s.command("scatter.copies", Json{{"entity", "Bushes"}, {"limit", 20000}}).value();
        const int placed = bushes["placed"].get<int>();
        INFO(placed);
        REQUIRE(placed > 500);
        REQUIRE(s.command("world.get", Json{{"entity", "Bushes"}, {"component", "Scatter"}}).value()["placed"] == placed);
        // Every copy on gentle ground between the water and the snow, sunk by 0.08, none closer than the spacing.
        const double cos_max = std::cos(28.0 * 3.14159265 / 180.0);
        const Json& copies = bushes["copies"];
        for (std::size_t k = 0; k < copies.size(); ++k) {
            const Json& c = copies[k];
            Json g = s.command("terrain.height", Json{{"x", c["x"]}, {"z", c["z"]}}).value();
            INFO(c.dump() << " on " << g.dump());
            REQUIRE(c["y"].get<double>() == Catch::Approx(g["height"].get<double>() - 0.08).margin(1e-3));
            REQUIRE(g["normal"]["y"].get<double>() >= cos_max - 1e-4);
            REQUIRE(g["height"].get<double>() >= 3.6);
            REQUIRE(g["height"].get<double>() <= 8.5);
            if (k > 0) {
                const Json& d = copies[k - 1];
                REQUIRE(std::hypot(c["x"].get<double>() - d["x"].get<double>(), c["z"].get<double>() - d["z"].get<double>()) >= 0.9 - 1e-4);
            }
        }
        // Drawn as copies: many objects, few draw calls.
        Json stats = s.command("render.stats", Json::object()).value();
        REQUIRE(stats["scattered"].get<int>() >= placed);
        REQUIRE(stats["draw_calls"].get<int>() < 40);
        first = copies;
        // Raising the ground under a copy lifts it; lowering the scatter's bounds re-places it.
        const Json c0 = copies[0];
        s.command("terrain.sculpt", Json{{"x", c0["x"]}, {"z", c0["z"]}, {"radius", 2}, {"amount", 0.5}}).value();
        Json after = s.command("scatter.copies", Json{{"entity", "Bushes"}, {"limit", 20000}}).value()["copies"];
        bool lifted = false;
        for (const Json& c : after) if (std::hypot(c["x"].get<double>() - c0["x"].get<double>(), c["z"].get<double>() - c0["z"].get<double>()) < 1e-4 && c["y"].get<double>() > c0["y"].get<double>() + 0.3) lifted = true;
        REQUIRE(lifted);
        REQUIRE(s.command("world.set", Json{{"entity", "Bushes"}, {"component", "Scatter"}, {"value", Json{{"count", 0}}}}).has_value());
        REQUIRE(s.command("scatter.copies", Json{{"entity", "Bushes"}}).value()["placed"] == 0);
        REQUIRE_FALSE(s.command("scatter.copies", Json{{"entity", "Hills"}}).has_value());
    });
    // Another run places the same copies.
    run([&](app::Session& s) {
        REQUIRE(s.command("scatter.copies", Json{{"entity", "Bushes"}, {"limit", 20000}}).value()["copies"] == first);
    });
}

TEST_CASE("the hills' boulders are scattered copies that collide: counted by the physics, hit by rays, and in the player's way", "[runtime][terrain][scatter][collide]") {
    app::Options o;
    o.project_dir = root() / "samples" / "hills";
    o.bundle = root() / "build" / "ts" / "hills.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 5; ++i) REQUIRE(s.frame().has_value());
    const Json boulders = s.command("scatter.copies", Json{{"entity", "Boulders"}, {"limit", 1000}}).value();
    const int placed = boulders["placed"].get<int>();
    INFO(placed);
    REQUIRE(placed > 20);
    REQUIRE(s.command("physics.stats", Json::object()).value()["scattered"] == placed);
    const auto id = s.command("world.find", Json{{"path", "Boulders"}}).value().get<world::EntityId>();
    // A ray down onto each of the first ten meets the Boulders above the ground there.
    for (int k = 0; k < 10; ++k) {
        const Json& c = boulders["copies"][k];
        const Json hit = s.command("physics.raycast", Json{{"origin", Json{{"x", c["x"]}, {"y", 60}, {"z", c["z"]}}}, {"direction", Json{{"x", 0}, {"y", -1}, {"z", 0}}}}).value();
        REQUIRE(hit["entity"].get<world::EntityId>() == id);
        REQUIRE(hit["point"]["y"].get<double>() > c["y"].get<double>() + 0.5);
    }
    // Navigation baked around one leaves the ground under it out: a path from one side to the
    // other through its centre goes round it.
    {
        const Json& b = boulders["copies"][1];
        const double bx = b["x"].get<double>(), bz = b["z"].get<double>(), r = 0.42 * b["size"].get<double>();
        REQUIRE(s.command("nav.bake", Json{{"min", Json{{"x", bx - 8}, {"y", 0}, {"z", bz - 8}}}, {"max", Json{{"x", bx + 8}, {"y", 14}, {"z", bz + 8}}}, {"cell", 0.25}, {"agent_radius", 0.3}, {"max_slope", 50}}).has_value());
        auto at = [&](double x, double z) { return Json{{"x", x}, {"y", s.command("terrain.height", Json{{"x", x}, {"z", z}}).value()["height"].get<double>()}, {"z", z}}; };
        const Json path = s.command("nav.path", Json{{"from", at(bx - 4, bz)}, {"to", at(bx + 4, bz)}}).value();
        INFO(path.dump().substr(0, 600) << " boulder " << b.dump());
        REQUIRE(path["partial"] == false);
        const Json& pts = path["points"];
        REQUIRE(pts.size() >= 2);
        for (std::size_t k = 0; k + 1 < pts.size(); ++k) {
            for (int q = 0; q <= 8; ++q) {
                const double x = pts[k]["x"].get<double>() + (pts[k + 1]["x"].get<double>() - pts[k]["x"].get<double>()) * q / 8;
                const double z = pts[k]["z"].get<double>() + (pts[k + 1]["z"].get<double>() - pts[k]["z"].get<double>()) * q / 8;
                REQUIRE(std::hypot(x - bx, z - bz) > r);
            }
        }
    }
    // The player put beside one and walked at it never gets closer than their two radii: it is
    // stopped, or slides round it (the boulder is round) off the line it walked.
    const Json& c = boulders["copies"][0];
    const Json g = s.command("terrain.height", Json{{"x", c["x"].get<double>() - 4}, {"z", c["z"]}}).value();
    const auto player = s.command("world.find", Json{{"path", "Player"}}).value().get<world::EntityId>();
    REQUIRE(s.command("world.set", Json{{"entity", player}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", c["x"].get<double>() - 4}, {"y", g["height"].get<double>() + 1.2}, {"z", c["z"]}}}}}}).has_value());
    REQUIRE(s.command("input.hold", Json{{"action", "move_x"}, {"ticks", 90}}).has_value());
    const double reach = 0.42 * c["size"].get<double>() + 0.3;   // the boulder's capsule (entity scale x 1) and the player's
    double closest = 1e9;
    Json st;
    for (int i = 0; i < 90; ++i) {
        REQUIRE(s.frame().has_value());
        st = s.command("state", Json::object()).value()["state"];
        closest = std::min(closest, std::hypot(st["player.x"].get<double>() - c["x"].get<double>(), st["player.z"].get<double>() - c["z"].get<double>()));
    }
    INFO(st.dump() << " boulder " << c.dump() << " closest " << closest);
    REQUIRE(closest > reach - 0.05);
    REQUIRE((st["player.x"].get<double>() < c["x"].get<double>() || std::fabs(st["player.z"].get<double>() - c["z"].get<double>()) > 0.3));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("textured layers on a terrain: laid by their rules, painted and erased by name or index, saved as a layermap and read back", "[runtime][terrain][layers]") {
    const std::filesystem::path saved = root() / "samples" / "hills" / "assets" / "test-layers.png";
    std::filesystem::remove(saved);
    app::Options o;
    o.project_dir = root() / "samples" / "hills";
    o.bundle = root() / "build" / "ts" / "hills.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 5; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("pause", Json::object()).has_value());
    Json info = s.command("terrain.info", Json::object()).value();
    REQUIRE(info["layers"] == Json::array({"grass", "sand", "rock", "dirt"}));
    auto share = [&](double x, double z, const char* name) {
        double sum = 0, found = -1;
        for (const Json& l : s.command("terrain.height", Json{{"x", x}, {"z", z}}).value()["layers"]) {
            sum += l["share"].get<double>();
            if (l["name"] == name) found = l["share"].get<double>();
        }
        REQUIRE(sum == Catch::Approx(1).margin(1e-3));   // the shares always add up
        return found;
    };
    // The sample's rules: sand low by the water, rock on the steep slopes, grass elsewhere.
    Json steep, low, mid;
    for (double x = -44; x <= 44; x += 2) {
        for (double z = -44; z <= 44; z += 2) {
            const Json g = s.command("terrain.height", Json{{"x", x}, {"z", z}}).value();
            const double slope = std::acos(std::clamp(g["normal"]["y"].get<double>(), -1.0, 1.0)) * 180.0 / 3.14159265;
            const double h = g["height"].get<double>();
            const Json at{{"x", x}, {"z", z}};
            if (steep.is_null() && slope > 45) steep = at;
            if (low.is_null() && h < 3.0 && slope < 15) low = at;
            if (mid.is_null() && h > 5 && h < 8 && slope < 15 && share(x, z, "dirt") < 0.01 && std::abs(x) < 36 && std::abs(z) < 36) mid = at;
        }
    }
    INFO("steep " << steep.dump() << " low " << low.dump() << " mid " << mid.dump());
    REQUIRE_FALSE(steep.is_null());
    REQUIRE_FALSE(low.is_null());
    REQUIRE_FALSE(mid.is_null());
    REQUIRE(share(steep["x"].get<double>(), steep["z"].get<double>(), "rock") > 0.9);
    REQUIRE(share(low["x"].get<double>(), low["z"].get<double>(), "sand") > 0.9);
    const double mx = mid["x"].get<double>(), mz = mid["z"].get<double>();
    REQUIRE(share(mx, mz, "grass") > 0.9);
    // Sand painted on the grass by name at full strength covers it; erased, the grass is back.
    const double beyond = share(mx + 6.8, mz, "sand");
    Json painted = s.command("terrain.paint", Json{{"x", mx}, {"z", mz}, {"layer", "sand"}, {"radius", 6}, {"amount", 1}}).value();
    INFO(painted.dump());
    REQUIRE(painted["layer"] == 1);
    REQUIRE(painted["samples"].get<int>() > 20);
    REQUIRE(share(mx, mz, "sand") == Catch::Approx(1).margin(0.05));
    REQUIRE(share(mx, mz, "grass") < 0.05);
    REQUIRE(share(mx + 6.8, mz, "sand") == Catch::Approx(beyond).margin(1e-3));   // past the radius, as it was
    REQUIRE(s.command("terrain.paint", Json{{"x", mx}, {"z", mz}, {"layer", "sand"}, {"mode", "erase"}, {"radius", 6}, {"amount", 1}}).has_value());
    REQUIRE(share(mx, mz, "grass") > 0.9);
    // Rock by its index, half over.
    REQUIRE(s.command("terrain.paint", Json{{"x", mx}, {"z", mz}, {"layer", 2}, {"radius", 6}, {"amount", 0.5}}).has_value());
    const double half = share(mx, mz, "rock");
    REQUIRE(half == Catch::Approx(0.5).margin(0.05));
    // Layers it does not have are refused, naming the ones it has.
    auto bad = s.command("terrain.paint", Json{{"x", mx}, {"z", mz}, {"layer", "lava"}});
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().code == "bad_args");
    REQUIRE(bad.error().message.find("grass") != std::string::npos);
    REQUIRE(s.command("terrain.paint", Json{{"x", mx}, {"z", mz}, {"layer", 4}}).error().code == "bad_args");
    // The paint grid, saved as a layermap, cleared, and read back from the file.
    const Json grid = s.command("terrain.paints", Json{{"layers", true}}).value();
    REQUIRE(grid["paint"].size() == 129u * 129u * 4u);
    REQUIRE(s.command("terrain.save", Json{{"path", "assets/test-layers.png"}, {"layers", true}}).has_value());
    REQUIRE(std::filesystem::exists(saved));
    REQUIRE(s.command("terrain.info", Json::object()).value()["layermap"] == "assets/test-layers.png");
    REQUIRE(s.command("terrain.paints", Json{{"layers", true}, {"paint", Json::array()}}).has_value());
    REQUIRE(share(mx, mz, "rock") < 0.01);
    REQUIRE(s.command("terrain.reset", Json{{"layers", true}}).has_value());
    REQUIRE(share(mx, mz, "rock") == Catch::Approx(half).margin(0.01));
    std::filesystem::remove(saved);
    // A terrain without layers has none to paint.
    REQUIRE(s.command("world.set", Json{{"entity", "Hills"}, {"component", "Terrain"}, {"value", Json{{"layers", Json::array()}}}}).has_value());
    REQUIRE(s.command("terrain.paint", Json{{"x", mx}, {"z", mz}, {"layer", 0}}).error().code == "no_layers");
    REQUIRE_FALSE(s.command("terrain.height", Json{{"x", mx}, {"z", mz}}).value().contains("layers"));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("paint on a terrain: laid on and erased by a brush, drawn, answered by terrain.height, saved as a paintmap and kept through a new shape", "[runtime][terrain][paint]") {
    const std::filesystem::path saved = root() / "samples" / "hills" / "assets" / "test-paint.png";
    std::filesystem::remove(saved);
    app::Options o;
    o.project_dir = root() / "samples" / "hills";
    o.bundle = root() / "build" / "ts" / "hills.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("pause", Json::object()).has_value());
    const Json st = s.command("state", Json::object()).value()["state"];
    const double px = st["player.x"].get<double>() + 2.5, pz = st["player.z"].get<double>() - 1.5;
    auto ground = [&](double x, double z) { return s.command("terrain.height", Json{{"x", x}, {"z", z}}).value(); };
    // The sample paints a dirt path with its dirt layer from the player to the beacon at its start;
    // the player stands on it.
    auto dirt = [&](double x, double z) {
        for (const Json& l : ground(x, z)["layers"]) if (l["name"] == "dirt") return l["share"].get<double>();
        return -1.0;
    };
    REQUIRE(st["on_path"] == true);
    REQUIRE(dirt(st["player.x"].get<double>(), st["player.z"].get<double>()) > 0.5);
    // The sample's Scatters (max_paint 0.3) keep their bushes and stones off the path.
    for (const char* name : {"Bushes", "Stones"}) {
        const Json copies = s.command("scatter.copies", Json{{"entity", name}, {"limit", 20000}}).value()["copies"];
        REQUIRE(copies.size() > 100);
        for (const Json& c : copies) REQUIRE(dirt(c["x"].get<double>(), c["z"].get<double>()) <= 0.31);
    }
    REQUIRE(s.command("terrain.paints", Json{{"paint", Json::array()}, {"layers", true}}).has_value());
    // Cleared, the ground has no paint to answer.
    REQUIRE(s.command("terrain.paints", Json{{"paint", Json::array()}}).has_value());
    REQUIRE_FALSE(ground(px, pz).contains("paint"));
    // What the camera sees there before and after red is painted on it.
    auto pixel = [&]() {
        REQUIRE(s.frame().has_value());
        const Json g = ground(px, pz);
        const Json r = s.command("render.project", Json{{"point", g["point"]}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
    };
    const Json before = pixel();
    Json painted = s.command("terrain.paint", Json{{"x", px}, {"z", pz}, {"color", Json{{"r", 1}, {"g", 0}, {"b", 0}}}, {"radius", 3}, {"amount", 1}}).value();
    INFO(painted.dump());
    REQUIRE(painted["samples"].get<int>() > 20);
    REQUIRE(painted["paint"]["a"].get<double>() == Catch::Approx(1).margin(0.05));   // between samples, a little under full
    const Json after = pixel();
    INFO("before " << before.dump() << " after " << after.dump());
    REQUIRE(after[0].get<int>() > before[0].get<int>() + 30);
    REQUIRE(after[1].get<int>() < before[1].get<int>());
    // The brush fades: full at the centre, about half halfway out, nothing past the radius.
    Json centre = ground(px, pz)["paint"];
    REQUIRE(centre["r"].get<double>() == Catch::Approx(1).margin(0.02));
    REQUIRE(centre["g"].get<double>() == Catch::Approx(0).margin(0.02));
    const double half = ground(px + 1.5, pz)["paint"]["a"].get<double>();
    REQUIRE(half > 0.3);
    REQUIRE(half < 0.7);
    REQUIRE(ground(px + 3.5, pz)["paint"]["a"].get<double>() == 0);
    Json info = s.command("terrain.info", Json::object()).value();
    REQUIRE(info["painted"].get<double>() > 0);
    REQUIRE(info["painted"].get<double>() < 0.05);
    // Blue laid half over the red mixes them; erasing at full strength takes the paint away.
    s.command("terrain.paint", Json{{"x", px}, {"z", pz}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 1}}}, {"radius", 3}, {"amount", 0.5}}).value();
    centre = ground(px, pz)["paint"];
    REQUIRE(centre["r"].get<double>() == Catch::Approx(0.5).margin(0.03));
    REQUIRE(centre["b"].get<double>() == Catch::Approx(0.5).margin(0.03));
    REQUIRE(centre["a"].get<double>() == Catch::Approx(1).margin(0.03));
    REQUIRE(s.command("terrain.paint", Json{{"x", px}, {"z", pz}, {"mode", "erase"}, {"radius", 3}, {"amount", 1}}).value()["paint"]["a"].get<double>() < 0.03);
    REQUIRE_FALSE(s.command("terrain.paint", Json{{"x", px}, {"z", pz}}).has_value());   // painting needs a colour
    s.command("terrain.paint", Json{{"x", px}, {"z", pz}, {"color", Json{{"r", 0.2}, {"g", 0.8}, {"b", 0.3}}}, {"radius", 3}, {"amount", 1}}).value();
    // The grid read and set back whole (the editor's undo) is the same paint.
    const Json grid = s.command("terrain.paints", Json::object()).value();
    REQUIRE(grid["paint"].size() == 129u * 129u * 4u);
    const double at_half = ground(px + 1.5, pz)["paint"]["a"].get<double>();
    REQUIRE(at_half > 0.3);
    REQUIRE(s.command("terrain.paints", Json{{"paint", Json::array()}}).has_value());
    REQUIRE_FALSE(ground(px, pz).contains("paint"));
    REQUIRE(s.command("terrain.paints", Json{{"paint", grid["paint"]}}).has_value());
    REQUIRE(ground(px + 1.5, pz)["paint"]["a"].get<double>() == Catch::Approx(at_half).margin(1e-3));
    // Saved as an RGBA PNG it becomes the paintmap and reads back to within a step of 8 bits.
    Json sv = s.command("terrain.save", Json{{"path", "assets/test-paint.png"}, {"paint", true}}).value();
    REQUIRE(std::filesystem::exists(saved));
    const Json tc = s.command("world.get", Json{{"entity", info["entity"]}, {"component", "Terrain"}}).value();
    REQUIRE(tc["paintmap"] == "assets/test-paint.png");
    REQUIRE(s.command("terrain.paints", Json{{"paint", Json::array()}}).has_value());
    REQUIRE(s.command("terrain.reset", Json{{"paint", true}}).has_value());   // back to the paintmap's
    REQUIRE(ground(px + 1.5, pz)["paint"]["a"].get<double>() == Catch::Approx(at_half).margin(0.01));
    // A new shape (another seed) keeps the paint; sculpting keeps it too.
    Json next = tc;
    next["seed"] = tc["seed"].get<int>() + 1;
    REQUIRE(s.command("world.set", Json{{"entity", info["entity"]}, {"component", "Terrain"}, {"value", next}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(ground(px + 1.5, pz)["paint"]["a"].get<double>() == Catch::Approx(at_half).margin(0.01));
    REQUIRE(s.command("terrain.sculpt", Json{{"x", px}, {"z", pz}, {"amount", 1}}).has_value());
    REQUIRE(ground(px + 1.5, pz)["paint"]["a"].get<double>() == Catch::Approx(at_half).margin(0.01));
    // A stroke through points paints the whole line between them in one change, and nothing off it.
    const int rev = s.command("terrain.info", Json::object()).value()["revision"].get<int>();
    REQUIRE(s.command("terrain.paints", Json{{"paint", Json::array()}}).has_value());
    const Json line = s.command("terrain.paint", Json{{"points", Json::array({Json{{"x", px - 6}, {"z", pz}}, Json{{"x", px + 6}, {"z", pz}}})}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 0}}}, {"radius", 1}, {"amount", 0.8}}).value();
    REQUIRE(line["revision"].get<int>() == rev + 2);   // the clearing, then the stroke
    for (double x = px - 5.5; x <= px + 5.5; x += 0.37) REQUIRE(ground(x, pz)["paint"]["a"].get<double>() > 0.5);
    REQUIRE(ground(px, pz + 2)["paint"]["a"].get<double>() == 0);   // the nearest samples are past the radius
    REQUIRE_FALSE(s.command("terrain.paint", Json{{"points", Json::array()}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 0}}}}).has_value());
    Json evs = s.command("events.since", Json{{"since", 0}, {"limit", 1000}}).value()["events"];
    int count = 0;
    for (const Json& e : evs) count += e["type"] == "terrain.painted" ? 1 : 0;
    REQUIRE(count >= 5);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(saved);
}

namespace {

// One peer of a lockstep arena game on its own thread: run until `until` ticks, pressing its move
// action at tick 20, then report its state.
struct Peer {
    Json state, net;
    std::string error;
};

void play_peer(app::Options o, int until, const char* action, int sign, std::promise<int>* port, Peer& out, int perturb_at = -1, int hold_at = 20, int pace_ms = 0) {
    app::Session s(o);
    if (auto r = s.start(); !r) {
        out.error = r.error().to_string();
        if (port) port->set_value(-1);
        return;
    }
    if (port) port->set_value(s.command("net.info", Json::object()).value()["port"].get<int>());
    bool held = false, perturbed = false;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(60);
    for (;;) {
        const std::int64_t tick = s.command("state", Json::object()).value()["tick"].get<std::int64_t>();
        if (tick >= until || std::chrono::steady_clock::now() > deadline) break;
        if (!held && tick >= hold_at) {
            s.command("input.hold", Json{{"action", action}, {"ticks", 60}, {"sign", sign}}).value();
            held = true;
        }
        if (perturb_at >= 0 && !perturbed && tick >= perturb_at) {
            s.command("world.set", Json{{"entity", "Ball"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 3}}}}}}).value();
            perturbed = true;
        }
        if (auto r = s.frame(); !r) { out.error = r.error().to_string(); break; }
        if (pace_ms > 0) std::this_thread::sleep_for(std::chrono::milliseconds(pace_ms));
    }
    // A little longer so the last hashes and inputs reach the host before anyone leaves.
    for (int i = 0; i < 30; ++i) (void)s.idle_frame();
    out.state = s.command("state", Json::object()).value();
    out.net = s.command("net.info", Json::object()).value();
    (void)s.finish();
}

app::Options arena_options() {
    app::Options o;
    o.project_dir = root() / "samples" / "arena";
    o.bundle = root() / "build" / "ts" / "arena.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 100000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    return o;
}

}  // namespace

TEST_CASE("two peers play a lockstep game: each moves its own player, and their worlds stay the same", "[runtime][net]") {
    app::Options ho = arena_options();
    ho.net_host = 0;
    ho.net_players = 2;
    std::promise<int> port;
    auto port_ready = port.get_future();
    Peer host, player;
    std::thread host_thread([&] { play_peer(ho, 240, "move_x", 1, &port, host); });
    const int p = port_ready.get();
    REQUIRE(p > 0);
    app::Options po = arena_options();
    po.net_join = "127.0.0.1:" + std::to_string(p);
    std::thread player_thread([&] { play_peer(po, 240, "move_z", 1, nullptr, player); });
    host_thread.join();
    player_thread.join();
    INFO("host " << host.error << " " << host.state.dump() << " " << host.net.dump());
    INFO("player " << player.error << " " << player.state.dump() << " " << player.net.dump());
    REQUIRE(host.error.empty());
    REQUIRE(player.error.empty());
    REQUIRE(host.net["mode"] == "host");
    REQUIRE(player.net["mode"] == "player");
    REQUIRE(player.net["player"] == 1);
    REQUIRE(host.state["tick"] == 240);
    REQUIRE(player.state["tick"] == 240);
    // The same game on both: every exposed value and the whole run's hash chain.
    REQUIRE(host.state["state"] == player.state["state"]);
    REQUIRE(host.state["state_hash"] == player.state["state_hash"]);
    // Each peer's key moved its own player: Red (the host's) to the right, Blue (the other's) south.
    REQUIRE(host.state["state"]["red.x"].get<double>() > 3.0);
    REQUIRE(host.state["state"]["blue.z"].get<double>() > -6.0 + 3.0);
    REQUIRE(host.state["state"]["red.z"].get<double>() == Catch::Approx(6).margin(0.05));
    REQUIRE(host.state["state"]["blue.x"].get<double>() == Catch::Approx(0).margin(0.05));
    REQUIRE(host.net["desyncs"] == 0);
}

TEST_CASE("a peer whose world drifts from the others' is caught by the host's hash check", "[runtime][net]") {
    app::Options ho = arena_options();
    ho.net_host = 0;
    std::promise<int> port;
    auto port_ready = port.get_future();
    Peer host, player;
    std::thread host_thread([&] { play_peer(ho, 150, "move_x", 1, &port, host); });
    const int p = port_ready.get();
    REQUIRE(p > 0);
    app::Options po = arena_options();
    po.net_join = "127.0.0.1:" + std::to_string(p);
    // The joining peer moves the ball on its own at tick 70: its world is no longer the host's.
    std::thread player_thread([&] { play_peer(po, 150, "move_z", 1, nullptr, player, 70); });
    host_thread.join();
    player_thread.join();
    INFO(host.net.dump() << " / " << player.net.dump());
    REQUIRE(host.error.empty());
    REQUIRE(host.net["desyncs"].get<int>() >= 1);
    REQUIRE(host.net["first_desync"].get<int>() >= 70);
    REQUIRE(host.net["first_desync"].get<int>() <= 90);
}

TEST_CASE("a player who left comes back into the running game: it replays what it missed, is let back in, and plays in step", "[runtime][net][rejoin]") {
    app::Options ho = arena_options();
    ho.net_host = 0;
    ho.net_players = 2;
    std::promise<int> port;
    auto port_ready = port.get_future();
    Peer host, first, back;
    // The host plays to tick 900 at a walking pace (a few ms a frame), so the game is still running
    // when the other player comes back.
    std::thread host_thread([&] { play_peer(ho, 900, "move_x", 1, &port, host, -1, 20, 4); });
    const int p = port_ready.get();
    REQUIRE(p > 0);
    app::Options po = arena_options();
    po.net_join = "127.0.0.1:" + std::to_string(p);
    // Player 1 plays until tick 150 and leaves (its Blue walks south first).
    std::thread first_thread([&] { play_peer(po, 150, "move_z", 1, nullptr, first); });
    first_thread.join();
    REQUIRE(first.error.empty());
    // Someone comes back into its place: welcomed late, it replays from tick 0 and then plays,
    // walking Blue west once it is back.
    std::this_thread::sleep_for(std::chrono::milliseconds(400));
    std::thread back_thread([&] { play_peer(po, 900, "move_x", -1, nullptr, back, -1, 700); });
    host_thread.join();
    back_thread.join();
    INFO("host " << host.error << " " << host.state.dump() << " " << host.net.dump());
    INFO("back " << back.error << " " << back.state.dump() << " " << back.net.dump());
    REQUIRE(host.error.empty());
    REQUIRE(back.error.empty());
    REQUIRE(back.net["player"] == 1);
    REQUIRE(back.net["late"] == true);
    REQUIRE(back.net["catching_up"] == false);
    const std::int64_t back_at = back.net["back_at"].get<std::int64_t>();
    REQUIRE(back_at > 150);
    REQUIRE(back_at < 700);   // back in before its key was pressed
    // Away once, from after its last input until it was back.
    REQUIRE(host.net["away"]["1"].size() == 1);
    REQUIRE(host.net["away"]["1"][0][1].get<std::int64_t>() == back_at);
    // The same game on both, through the absence and the replay: every exposed value and the whole
    // run's hash chain; the hashes it reported while replaying matched the host's.
    REQUIRE(host.state["tick"] == 900);
    REQUIRE(back.state["tick"] == 900);
    REQUIRE(host.state["state"] == back.state["state"]);
    REQUIRE(host.state["state_hash"] == back.state["state_hash"]);
    REQUIRE(host.net["replay_checks"].get<int>() >= 4);
    REQUIRE(host.net["desyncs"] == 0);
    // Blue walked west under the one who came back: its key counts again.
    REQUIRE(host.state["state"]["blue.x"].get<double>() < -2.0);
}

TEST_CASE("a burst comes from the emitter or from any point, faster or slower", "[runtime][particles][burst]") {
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
    auto emitter = [&](const char* name, Vec3 at, bool world_space) {
        const Json fields{{"emitting", false}, {"world_space", world_space}, {"gravity", {{"x", 0}, {"y", 0}, {"z", 0}}}, {"speed", {{"x", 2}, {"y", 2}}}, {"lifetime", {{"x", 5}, {"y", 5}}}};
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", {{"x", at.x}, {"y", at.y}, {"z", at.z}}}}}, {"ParticleEmitter", fields}}}}).has_value());
        REQUIRE(s.frame().has_value());
    };
    auto list = [&](const char* name) { return s.command("particles.list", Json{{"entity", name}, {"limit", 100}}).value()["particles"]; };
    auto speed_of = [](const Json& p) { return std::hypot(p["velocity"]["x"].get<double>(), p["velocity"]["y"].get<double>(), p["velocity"]["z"].get<double>()); };
    emitter("Spray", {0, 0, 0}, true);
    REQUIRE(s.command("particles.burst", Json{{"entity", "Spray"}, {"count", 3}}).value()["alive"] == 3);
    REQUIRE(s.command("particles.burst", Json{{"entity", "Spray"}, {"count", 5}, {"at", {10, 4, -2}}, {"speed", 0.5}}).value()["alive"] == 8);
    const Json spray = list("Spray");
    for (std::size_t i = 0; i < spray.size(); ++i) {
        const Json& p = spray[i];
        INFO(p.dump());
        if (i < 3) {   // from the emitter at its speed
            REQUIRE(p["position"]["x"].get<double>() == Catch::Approx(0.0));
            REQUIRE(speed_of(p) == Catch::Approx(2.0));
        } else {       // from the point at half of it
            REQUIRE(p["position"]["x"].get<double>() == Catch::Approx(10.0));
            REQUIRE(p["position"]["y"].get<double>() == Catch::Approx(4.0));
            REQUIRE(p["position"]["z"].get<double>() == Catch::Approx(-2.0));
            REQUIRE(speed_of(p) == Catch::Approx(1.0));
        }
    }
    // A local emitter's particles live relative to it: the point is where they are in the world all the same.
    emitter("Fountain", {3, 1, 0}, false);
    REQUIRE(s.command("particles.burst", Json{{"entity", "Fountain"}, {"count", 2}, {"at", Json{{"x", 3}, {"y", 6}, {"z", 1}}}}).has_value());
    for (const Json& p : list("Fountain")) {
        REQUIRE(p["position"]["x"].get<double>() == Catch::Approx(0.0));
        REQUIRE(p["position"]["y"].get<double>() == Catch::Approx(5.0));
        REQUIRE(p["position"]["z"].get<double>() == Catch::Approx(1.0));
    }
    REQUIRE(s.command("particles.burst", Json{{"entity", "Spray"}, {"at", "here"}}).error().code == "bad_args");
    REQUIRE(s.command("particles.burst", Json{{"entity", "Spray"}, {"speed", -1}}).error().code == "bad_args");
    REQUIRE(s.finish().has_value());
}

TEST_CASE("pictures written to a relative path land under the project, and never out of it", "[runtime][capture]") {
    app::Options o;
    o.project_dir = root() / "samples" / "hello";
    o.bundle = root() / "build" / "ts" / "hello.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    const std::filesystem::path shot = o.project_dir / ".pocket" / "test-capture" / "shot.png";
    std::filesystem::remove_all(shot.parent_path());
    const Json cap = s.command("capture", Json{{"path", ".pocket/test-capture/shot.png"}, {"ids", ".pocket/test-capture/ids.png"}}).value();
    REQUIRE(std::filesystem::exists(shot));   // its directory made
    REQUIRE(std::filesystem::equivalent(std::filesystem::path(cap["path"].get<std::string>()), shot));
    REQUIRE(std::filesystem::exists(o.project_dir / ".pocket" / "test-capture" / "ids.png"));
    const Json views = s.command("render.views", Json{{"path", ".pocket/test-capture/views.png"}}).value();
    REQUIRE(std::filesystem::exists(o.project_dir / ".pocket" / "test-capture" / "views.png"));
    REQUIRE(std::filesystem::path(views["path"].get<std::string>()).is_absolute());
    REQUIRE(s.command("capture", Json{{"path", "../escaped.png"}}).error().code == "forbidden");
    REQUIRE_FALSE(std::filesystem::exists(o.project_dir.parent_path() / "escaped.png"));
    std::filesystem::remove_all(shot.parent_path());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("water.height answers the moving surface, and a crate dropped in the lake floats on it", "[runtime][water]") {
    app::Options o;
    o.project_dir = root() / "samples" / "hills";
    o.bundle = root() / "build" / "ts" / "hills.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 5; ++i) REQUIRE(s.frame().has_value());
    // A deep spot of the lake: the lowest ground on a coarse look.
    float bx = 0, bz = 0, low = 1e9f;
    for (int i = -20; i <= 20; ++i) {
        for (int j = -20; j <= 20; ++j) {
            const float h = s.command("terrain.height", Json{{"x", i * 2.2}, {"z", j * 2.2}}).value()["height"].get<float>();
            if (h < low) { low = h; bx = i * 2.2f; bz = j * 2.2f; }
        }
    }
    INFO("deepest ground " << low << " at " << bx << ", " << bz);
    REQUIRE(low < 3.2f - 1.5f);
    Json w0 = s.command("water.height", Json{{"x", bx}, {"z", bz}}).value();
    INFO(w0.dump());
    REQUIRE(w0["path"] == "/Lake");
    REQUIRE(w0["inside"] == true);
    REQUIRE(w0["level"].get<double>() == Catch::Approx(3.2));
    REQUIRE(std::fabs(w0["height"].get<double>() - 3.2) < 0.25);            // waves 0.22 high
    REQUIRE(w0["bottom"].get<double>() == Catch::Approx(3.2 - 6));
    REQUIRE(s.command("water.height", Json{{"x", 500}, {"z", 0}}).value()["entity"].is_null());
    // The surface moves: over a second it is not where it was.
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("water.height", Json{{"x", bx}, {"z", bz}}).value()["height"].get<double>() != Catch::Approx(w0["height"].get<double>()).margin(1e-4));
    // A crate 0.5 units a side of mass 0.1: the 0.125 cubic units of water it could displace weigh 0.25.
    REQUIRE(s.command("world.spawn", Json{{"name", "Crate"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", bx}, {"y", 5.5}, {"z", bz}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}, {"RigidBody", Json{{"kind", 0}, {"mass", 0.1}}}, {"Collider", Json{{"shape", 0}, {"size", Json{{"x", 0.25}, {"y", 0.25}, {"z", 0.25}}}}}}}}).has_value());
    for (int i = 0; i < 240; ++i) REQUIRE(s.frame().has_value());
    const Json crate = s.command("world.get", Json{{"entity", "Crate"}, {"component", "Transform"}}).value();
    const double cy = crate["position"]["y"].get<double>();
    const Json here = s.command("water.height", Json{{"x", crate["position"]["x"]}, {"z", crate["position"]["z"]}}).value();
    INFO("crate at " << cy << ", the surface at " << here["height"]);
    REQUIRE(std::fabs(cy - here["height"].get<double>()) < 0.3);
    REQUIRE(s.command("physics.stats", Json::object()).value()["floating"].get<int>() >= 1);
    REQUIRE(s.command("events.histogram", Json::object()).value()["water.entered"].get<int>() >= 1);
    // It splashed where it came down: the lake's Splash emitter threw as many as its speed asks.
    Json crate_in;
    for (const Json& e : s.command("events.since", Json{{"type", "water.entered"}}).value()["events"])
        if (e["data"]["path"] == "/Crate") crate_in = e["data"];
    INFO(crate_in.dump());
    REQUIRE(crate_in["point"]["x"].get<double>() == Catch::Approx(bx).margin(0.3));
    REQUIRE(crate_in["point"]["y"].get<double>() == Catch::Approx(3.2).margin(0.25));
    REQUIRE(crate_in["speed"].get<double>() > 4.0);   // from two units up
    const auto splash = s.command("world.find", Json{{"path", "Splash"}}).value().get<world::EntityId>();
    std::int64_t spawned = 0;
    for (const Json& pl : s.command("particles.stats", Json::object()).value()["pools"])
        if (pl["entity"].get<world::EntityId>() == splash) spawned = pl["spawned"].get<std::int64_t>();
    REQUIRE(spawned >= std::lround(24 * crate_in["speed"].get<double>() / 8));
    REQUIRE(s.finish().has_value());
}

TEST_CASE("an AnimationGraph moves between states on its parameters, blends along one, fires triggers and reports what is wrong", "[runtime][animation][animgraph]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // The arm's clips: nod (1.5 s), wave, walk and turn (1 s each).
    const Json graph = Json{
        {"states", Json::array({Json{{"name", "idle"}, {"clip", "nod"}},
                                Json{{"name", "move"}, {"blend", "speed"}, {"clips", "wave 0, walk 2"}},
                                Json{{"name", "hop"}, {"clip", "turn"}, {"loop", false}}})},
        {"transitions", Json::array({Json{{"from", "idle"}, {"to", "move"}, {"when", "speed > 0.1"}, {"fade", 0.2}},
                                     Json{{"from", "move"}, {"to", "idle"}, {"when", "speed <= 0.1"}, {"fade", 0.3}},
                                     Json{{"from", "*"}, {"to", "hop"}, {"when", "jump and not tired"}, {"fade", 0.1}},
                                     Json{{"from", "hop"}, {"to", "idle"}, {"after", 1.0}, {"fade", 0.2}}})},
        {"params", Json::array({Json{{"name", "speed"}, {"value", 0}}, Json{{"name", "jump"}, {"trigger", true}}, Json{{"name", "tired"}, {"value", 0}}})}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Actor"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -3}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"Animator", Json::object()}, {"AnimationGraph", graph}}}}).has_value());
    auto get = [&](const char* component) { return s.command("world.get", Json{{"entity", "Actor"}, {"component", component}}).value(); };
    auto frames = [&](int n) { for (int i = 0; i < n; ++i) REQUIRE(s.frame().has_value()); };
    frames(2);
    Json g = get("AnimationGraph"), a = get("Animator");
    INFO(g.dump());
    REQUIRE(g["state"] == "idle");
    REQUIRE(g["error"] == "");
    REQUIRE(a["clip"] == "nod");
    // Moving: into the blend space, halfway between wave and walk at speed 1, the idle clip fading out.
    Json r = s.command("animation.param", Json{{"entity", "Actor"}, {"name", "speed"}, {"value", 1.0}}).value();
    REQUIRE(r["params"]["speed"] == 1.0);
    frames(1);
    g = get("AnimationGraph");
    a = get("Animator");
    INFO(a.dump());
    REQUIRE(g["state"] == "move");
    REQUIRE(a["clip"] == "wave");
    REQUIRE(a["blends"].size() == 1);
    REQUIRE(a["blends"][0]["clip"] == "walk");
    REQUIRE(a["blends"][0]["weight"].get<double>() == Catch::Approx(0.5));
    REQUIRE(a["from_clip"] == "nod");
    REQUIRE(a["fade"].get<double>() == Catch::Approx(0.2));
    const Json moving = s.command("animation.pose", Json{{"entity", "Actor"}}).value();
    // At speed 2 the walk alone plays, in the phase the wave had reached.
    const double phase = a["time"].get<double>();
    REQUIRE(s.command("animation.param", Json{{"entity", "Actor"}, {"name", "speed"}, {"value", 2.0}}).has_value());
    frames(1);
    a = get("Animator");
    REQUIRE(a["clip"] == "walk");
    REQUIRE(a["blends"].empty());
    REQUIRE(a["time"].get<double>() == Catch::Approx(phase + 1.0 / 60).margin(0.02));
    REQUIRE(s.command("animation.pose", Json{{"entity", "Actor"}}).value() != moving);
    // A trigger from any state: the hop, the trigger spent, then back to idle when it has played
    // through, and on into move since the speed is still 2.
    REQUIRE(s.command("animation.trigger", Json{{"entity", "Actor"}, {"name", "jump"}}).has_value());
    frames(1);
    g = get("AnimationGraph");
    REQUIRE(g["state"] == "hop");
    REQUIRE(get("Animator")["clip"] == "turn");
    REQUIRE(g["params"][1]["value"] == 0.0);
    frames(70);
    std::vector<std::string> path;
    for (const Json& e : s.command("events.since", Json{{"since", 0}, {"type", "animation.state"}}).value()["events"]) {
        if (e["data"]["path"] == "/Actor") path.push_back(e["data"]["from"].get<std::string>() + ">" + e["data"]["to"].get<std::string>());
    }
    std::string joined;
    for (const auto& p : path) joined += p + " ";
    INFO(joined);
    REQUIRE(path == std::vector<std::string>{">idle", "idle>move", "move>hop", "hop>idle", "idle>move"});
    // A flag blocks the trigger: jump and not tired.
    REQUIRE(s.command("animation.param", Json{{"entity", "Actor"}, {"name", "tired"}, {"value", 1}}).has_value());
    REQUIRE(s.command("animation.trigger", Json{{"entity", "Actor"}, {"name", "jump"}}).has_value());
    frames(2);
    REQUIRE(get("AnimationGraph")["state"] == "move");
    // What is wrong is said: a parameter that is not there, a condition that ends too soon, a missing state.
    REQUIRE_FALSE(s.command("animation.param", Json{{"entity", "Actor"}, {"name", "sped"}, {"value", 1}}).has_value());
    Json broken = graph;
    broken["transitions"][0]["when"] = "speed >";
    REQUIRE(s.command("world.set", Json{{"entity", "Actor"}, {"component", "AnimationGraph"}, {"value", Json{{"transitions", broken["transitions"]}, {"state", "idle"}}}}).has_value());
    frames(1);
    INFO(get("AnimationGraph").dump());
    REQUIRE(get("AnimationGraph")["error"].get<std::string>().find("ends too soon") != std::string::npos);
    REQUIRE(get("AnimationGraph")["state"] == "idle");                  // the broken way out is never taken
    broken["transitions"][0]["when"] = "speed > 0.1";
    broken["transitions"][1]["to"] = "nowhere";
    REQUIRE(s.command("world.set", Json{{"entity", "Actor"}, {"component", "AnimationGraph"}, {"value", Json{{"transitions", broken["transitions"]}}}}).has_value());
    frames(1);
    REQUIRE(get("AnimationGraph")["error"].get<std::string>().find("'nowhere'") != std::string::npos);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a blend space on two parameters mixes the clips around the point", "[runtime][animation][animgraph][blend2d]") {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 1000;
    o.width = 160;
    o.height = 90;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    // A plane of the arm's clips: nod at the middle, wave to the right, walk ahead, turn to the left.
    const Json graph = Json{
        {"states", Json::array({Json{{"name", "strafe"}, {"blend", "mx, my"}, {"clips", "nod 0 0, wave 1 0, walk 0 1, turn -1 0"}}})},
        {"params", Json::array({Json{{"name", "mx"}, {"value", 1}}, Json{{"name", "my"}, {"value", 0}}})}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Actor"}, {"components", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", "assets/arm.glb"}}}, {"Animator", Json::object()}, {"AnimationGraph", graph}}}}).has_value());
    auto get = [&](const char* component) { return s.command("world.get", Json{{"entity", "Actor"}, {"component", component}}).value(); };
    auto frames = [&](int n) { for (int i = 0; i < n; ++i) REQUIRE(s.frame().has_value()); };
    frames(1);
    Json a = get("Animator");
    INFO(a.dump());
    REQUIRE(get("AnimationGraph")["error"] == "");
    // On a clip's own point it plays alone.
    REQUIRE(a["clip"] == "wave");
    REQUIRE(a["blends"].empty());
    // Between the middle, the right and ahead: those three share it, the left has none.
    REQUIRE(s.command("animation.param", Json{{"entity", "Actor"}, {"name", "mx"}, {"value", 0.5}}).has_value());
    REQUIRE(s.command("animation.param", Json{{"entity", "Actor"}, {"name", "my"}, {"value", 0.5}}).has_value());
    frames(1);
    a = get("Animator");
    REQUIRE(a["blends"].size() == 2);
    double sum = 0;
    for (const Json& b : a["blends"]) {
        REQUIRE(b["clip"] != "turn");
        REQUIRE(b["weight"].get<double>() == Catch::Approx(1.0 / 3).margin(1e-4));
        sum += b["weight"].get<double>();
    }
    REQUIRE(sum == Catch::Approx(2.0 / 3).margin(1e-4));
    REQUIRE(s.command("animation.pose", Json{{"entity", "Actor"}}).value()["blends"].size() == 2);
    // Past the outer clips, the nearest plays.
    REQUIRE(s.command("animation.param", Json{{"entity", "Actor"}, {"name", "mx"}, {"value", -3}}).has_value());
    REQUIRE(s.command("animation.param", Json{{"entity", "Actor"}, {"name", "my"}, {"value", 0}}).has_value());
    frames(1);
    a = get("Animator");
    REQUIRE(a["clip"] == "turn");
    REQUIRE(a["blends"].empty());
    // A parameter the graph does not have, and a clip with one value on a plane, are named.
    Json bad = graph;
    bad["states"][0]["blend"] = "mx, nope";
    REQUIRE(s.command("world.set", Json{{"entity", "Actor"}, {"component", "AnimationGraph"}, {"value", bad}}).has_value());
    frames(1);
    REQUIRE(get("AnimationGraph")["error"].get<std::string>().find("'nope', which is not a parameter") != std::string::npos);
    bad = graph;
    bad["states"][0]["clips"] = "nod 0, wave 1 0";
    REQUIRE(s.command("world.set", Json{{"entity", "Actor"}, {"component", "AnimationGraph"}, {"value", bad}}).has_value());
    frames(1);
    REQUIRE(get("AnimationGraph")["error"].get<std::string>().find("'nod 0' in the clips needs a clip and two values") != std::string::npos);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a timeline moves fields along its keys and easings, sets others at moments, fires its events, loops, seeks and says what it cannot do", "[runtime][timeline]") {
    const std::filesystem::path dir = root() / "samples" / "hello" / "timelines";
    std::filesystem::create_directories(dir);
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove_all(p); } } cleanup{dir};
    std::ofstream(dir / "door.json") << R"({
      "duration": 3,
      "tracks": [
        {"entity": "Door", "component": "Transform", "field": "position.y", "keys": [[0, 0], [1, 2], [2, 2], [3, 0, "cubicOut"]]},
        {"entity": "Door", "component": "Transform", "field": "rotation", "keys": [[0, [0, 0, 0]], [2, [0, 90, 0]]]},
        {"entity": "Lamp", "component": "Light", "field": "intensity", "keys": [[0, 0], [2, 4, "quadIn"]]},
        {"entity": "Lamp", "component": "Light", "field": "color", "keys": [[0, [1, 1, 1, 1]], [2, [1, 0.5, 0.25, 1]]]},
        {"entity": "Door", "component": "MeshRenderer", "field": "cast_shadows", "keys": [[0, true], [1.5, false]]}
      ],
      "events": [[0.5, "door.creak", {"loud": 2}], [3, "door.shut"]]
    })";
    std::ofstream(dir / "broken.json") << R"({"tracks": [{"entity": "Door", "component": "Transform", "field": "position.y", "keys": [[0, 0], [1, 2, "wobbly"]]}]})";
    std::ofstream(dir / "stray.json") << R"({"tracks": [{"entity": "Nobody", "component": "Transform", "field": "position.y", "keys": [[0, 0], [1, 1]]},
        {"entity": "Door", "component": "Transform", "field": "colour", "keys": [[0, 0]]},
        {"entity": "Door", "component": "Transform", "field": "position", "keys": [[0, [1, 2]]]}]})";
    app::Session s(hello_options(10000));
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Door"}, {"components", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", "cube"}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Lamp"}, {"components", Json{{"Transform", Json::object()}, {"Light", Json{{"kind", 1}, {"intensity", 0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Director"}, {"components", Json{{"Transform", Json::object()}}}}).has_value());
    auto ticks = [&](int n) { REQUIRE(s.command("step", Json{{"ticks", n}}).has_value()); };
    auto door = [&]() { return s.command("world.get", Json{{"entity", "Door"}, {"component", "Transform"}}).value(); };
    auto lamp = [&]() { return s.command("world.get", Json{{"entity", "Lamp"}, {"component", "Light"}}).value(); };
    // Read and checked before it plays.
    Json info = s.command("timeline.info", Json{{"path", "timelines/door.json"}}).value();
    INFO(info.dump());
    REQUIRE(info["duration"] == 3.0);
    REQUIRE(info["tracks"].size() == 5);
    REQUIRE(info["problems"].empty());
    REQUIRE(info["tracks"][0]["target"] == "/Door");
    // Half a second in: the door halfway up (linear), the lamp at a sixteenth of 4 (quadIn at a quarter), the creak once.
    Json play = s.command("timeline.play", Json{{"entity", "Director"}, {"path", "timelines/door.json"}}).value();
    REQUIRE(play["duration"] == 3.0);
    const Json last = s.command("events.last_seq", Json::object()).value();
    const std::uint64_t seq0 = last["seq"].get<std::uint64_t>();
    ticks(30);
    REQUIRE(s.command("world.get", Json{{"entity", "Director"}, {"component", "Timeline"}}).value()["time"].get<double>() == Catch::Approx(0.5).margin(1e-4));
    REQUIRE(door()["position"]["y"].get<double>() == Catch::Approx(1.0).margin(1e-3));
    REQUIRE(lamp()["intensity"].get<double>() == Catch::Approx(0.25).margin(1e-3));
    ticks(1);
    auto count = [&](const char* type) {
        int n = 0;
        for (const Json& e : s.command("events.since", Json{{"seq", seq0}, {"type", type}}).value()["events"]) { (void)e; ++n; }
        return n;
    };
    REQUIRE(count("door.creak") == 1);
    const Json creak = s.command("events.since", Json{{"seq", seq0}, {"type", "door.creak"}}).value()["events"][0];
    REQUIRE(creak["data"]["loud"] == 2);
    REQUIRE(creak["data"]["timeline"] == "timelines/door.json");
    // One second in: the door turned 45 degrees about y (degrees in the keys), the colour on its way.
    ticks(29);
    REQUIRE(door()["rotation"]["y"].get<double>() == Catch::Approx(std::sin(3.14159265 / 8)).margin(1e-3));
    REQUIRE(lamp()["color"]["g"].get<double>() == Catch::Approx(0.75).margin(1e-2));
    REQUIRE(s.command("world.get", Json{{"entity", "Door"}, {"component", "MeshRenderer"}}).value()["cast_shadows"] == true);
    // Past 1.5 s the shadows step off.
    ticks(40);
    REQUIRE(s.command("world.get", Json{{"entity", "Door"}, {"component", "MeshRenderer"}}).value()["cast_shadows"] == false);
    // At the end: down again (cubicOut), stopped, finished, the shut event and timeline.finished.
    ticks(85);
    Json t = s.command("world.get", Json{{"entity", "Director"}, {"component", "Timeline"}}).value();
    REQUIRE(t["finished"] == true);
    REQUIRE(t["playing"] == false);
    REQUIRE(t["time"] == 3.0);
    REQUIRE(door()["position"]["y"].get<double>() == Catch::Approx(0.0).margin(1e-4));
    REQUIRE(count("door.shut") == 1);
    REQUIRE(count("timeline.finished") == 1);
    // Looping from 2.5 s: past the end into the next lap, the creak again at 0.5.
    REQUIRE(s.command("timeline.play", Json{{"entity", "Director"}, {"time", 2.5}, {"loop", true}}).has_value());
    ticks(31);
    t = s.command("world.get", Json{{"entity", "Director"}, {"component", "Timeline"}}).value();
    REQUIRE(t["playing"] == true);
    REQUIRE(t["time"].get<double>() == Catch::Approx(0.0167).margin(0.01));
    REQUIRE(count("door.shut") == 2);
    ticks(30);
    REQUIRE(count("door.creak") == 2);
    // Backward and stopped.
    REQUIRE(s.command("timeline.play", Json{{"entity", "Director"}, {"time", 1.0}, {"speed", -1}, {"loop", false}}).has_value());
    ticks(30);
    REQUIRE(door()["position"]["y"].get<double>() == Catch::Approx(1.0).margin(1e-3));
    REQUIRE(s.command("timeline.stop", Json{{"entity", "Director"}}).has_value());
    ticks(10);
    REQUIRE(door()["position"]["y"].get<double>() == Catch::Approx(1.0).margin(1e-3));
    // What is wrong is said: an easing that does not exist is refused; tracks that cannot apply are named.
    auto broken = s.command("timeline.play", Json{{"entity", "Director"}, {"path", "timelines/broken.json"}});
    REQUIRE_FALSE(broken.has_value());
    REQUIRE(broken.error().message.find("wobbly") != std::string::npos);
    Json stray = s.command("timeline.info", Json{{"path", "timelines/stray.json"}}).value();
    INFO(stray.dump());
    REQUIRE(stray["problems"].size() == 3);
    REQUIRE(stray["tracks"][0]["problem"].get<std::string>().find("Nobody") != std::string::npos);
    REQUIRE(stray["tracks"][1]["problem"].get<std::string>().find("colour") != std::string::npos);
    REQUIRE(stray["tracks"][2]["problem"].get<std::string>().find("parts") != std::string::npos);
    REQUIRE(s.command("timeline.play", Json{{"entity", "Director"}, {"path", "timelines/stray.json"}}).has_value());
    ticks(2);
    REQUIRE(s.command("world.get", Json{{"entity", "Director"}, {"component", "Timeline"}}).value()["error"].get<std::string>().find("Nobody") != std::string::npos);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a camera rig follows its target: orbit, easing, input turning, chase round a heading, in front of walls, a fixed offset, and a shake that fades", "[runtime][camerarig]") {
    app::Session s(hello_options(10000));
    REQUIRE(s.start().has_value());
    auto xyz = [](double x, double y, double z) { return Json{{"x", x}, {"y", y}, {"z", z}}; };
    REQUIRE(s.command("world.spawn", Json{{"name", "Target"}, {"components", Json{{"Transform", Json::object()}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Eye"}, {"components", Json{{"Transform", Json::object()}, {"CameraRig", Json{{"target", "Target"}, {"mode", 1}, {"distance", 5}, {"height", 1}, {"pitch", -30}, {"yaw", 0}, {"follow", 0}, {"collide", false}}}}}}).has_value());
    auto ticks = [&](int n) { REQUIRE(s.command("step", Json{{"ticks", n}}).has_value()); };
    auto eye = [&]() { return s.command("world.get", Json{{"entity", "Eye"}, {"component", "Transform"}}).value(); };
    auto rig = [&](Json v) { REQUIRE(s.command("world.set", Json{{"entity", "Eye"}, {"component", "CameraRig"}, {"value", v}}).has_value()); };
    auto forward = [&](const Json& t) {
        const Quat q{t["rotation"]["x"].get<float>(), t["rotation"]["y"].get<float>(), t["rotation"]["z"].get<float>(), t["rotation"]["w"].get<float>()};
        return q.rotate(Vec3{0, 0, -1});
    };
    ticks(1);
    // Orbit at pitch -30, distance 5, from +z: up 2.5 and back 4.33 from the pivot a unit over the target, looking at it.
    Json t = eye();
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(0).margin(1e-4));
    REQUIRE(t["position"]["y"].get<double>() == Catch::Approx(3.5).margin(1e-4));
    REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(4.330).margin(1e-3));
    REQUIRE(forward(t).y == Catch::Approx(-0.5).margin(1e-4));
    REQUIRE(forward(t).z == Catch::Approx(-0.866).margin(1e-3));
    // Easing: the target jumps 10 along x; a tick closes 1 - e^(-1/30) of it at follow 0.5, three seconds nearly all.
    rig(Json{{"follow", 0.5}});
    REQUIRE(s.command("world.set", Json{{"entity", "Target"}, {"component", "Transform"}, {"value", Json{{"position", xyz(10, 0, 0)}}}}).has_value());
    ticks(1);
    REQUIRE(eye()["position"]["x"].get<double>() == Catch::Approx(10 * (1 - std::exp(-1.0 / 30))).margin(1e-3));
    ticks(179);
    REQUIRE(eye()["position"]["x"].get<double>() == Catch::Approx(10 * (1 - std::exp(-6.0))).margin(0.02));
    // Turning by input: an action held for half a second at 120 degrees a second turns the orbit 60.
    REQUIRE(s.command("input.map", Json{{"actions", Json{{"look", Json{{"negative", Json::array({"Q"})}, {"positive", Json::array({"E"})}}}}}}).has_value());
    rig(Json{{"orbit_x", "look"}, {"follow", 0}});
    REQUIRE(s.command("input.hold", Json{{"action", "look"}, {"ticks", 30}}).has_value());
    ticks(40);
    const Json r = s.command("world.get", Json{{"entity", "Eye"}, {"component", "CameraRig"}}).value();
    INFO(r.dump());
    REQUIRE(r["yaw"].get<double>() == Catch::Approx(60).margin(2.5));
    // Chase: behind a target turned to face -x (heading 90), so on its +x side.
    rig(Json{{"mode", 0}, {"yaw", 0}, {"turn", 0}, {"orbit_x", ""}});
    REQUIRE(s.command("world.set", Json{{"entity", "Target"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"x", 0}, {"y", 0.7071068}, {"z", 0}, {"w", 0.7071068}}}}}}).has_value());
    ticks(1);
    t = eye();
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(10 + 4.330).margin(1e-3));
    REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(0).margin(1e-3));
    REQUIRE(forward(t).x < -0.8f);
    // Turning back, it swings round over its turn time rather than at once.
    rig(Json{{"turn", 0.5}});
    REQUIRE(s.command("world.set", Json{{"entity", "Target"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
    ticks(1);
    const double heading = s.command("world.get", Json{{"entity", "Eye"}, {"component", "CameraRig"}}).value()["heading"].get<double>();
    REQUIRE(heading == Catch::Approx(90 * std::exp(-1.0 / 30)).margin(0.05));
    ticks(240);
    REQUIRE(std::fabs(s.command("world.get", Json{{"entity", "Eye"}, {"component", "CameraRig"}}).value()["heading"].get<double>()) < 0.05);
    // A wall between the pivot and where it would stand (orbit from +z): it comes in front of it.
    rig(Json{{"mode", 1}, {"yaw", 0}, {"collide", true}});
    REQUIRE(s.command("world.spawn", Json{{"name", "Wall"}, {"components", Json{{"Transform", Json{{"position", xyz(10, 2, 2.5)}}}, {"RigidBody", Json{{"kind", 1}}}, {"Collider", Json{{"shape", 0}, {"size", xyz(3, 3, 0.2)}}}}}}).has_value());
    ticks(1);
    t = eye();
    INFO(t.dump());
    REQUIRE(t["position"]["z"].get<double>() < 2.3);
    REQUIRE(t["position"]["z"].get<double>() > 1.9);
    REQUIRE(s.command("world.destroy", Json{{"entity", "Wall"}}).has_value());
    // A fixed offset in the world, looking back at the pivot.
    rig(Json{{"mode", 2}, {"offset", xyz(0, 10, 8)}});
    ticks(1);
    t = eye();
    REQUIRE(t["position"]["y"].get<double>() == Catch::Approx(11).margin(1e-4));
    REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(8).margin(1e-4));
    const Vec3 still = forward(t);
    // A shake: the view trembles, the trauma drains at shake_decay a second, and it is still again.
    rig(Json{{"shake", 1.0}});
    ticks(1);
    const Vec3 shaken = forward(eye());
    REQUIRE(length(shaken - still) > 1e-3f);
    REQUIRE(s.command("world.get", Json{{"entity", "Eye"}, {"component", "CameraRig"}}).value()["shake"].get<double>() == Catch::Approx(1 - 1.5 / 60).margin(1e-4));
    ticks(60);
    REQUIRE(s.command("world.get", Json{{"entity", "Eye"}, {"component", "CameraRig"}}).value()["shake"] == 0.0);
    REQUIRE(length(forward(eye()) - still) < 1e-5f);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("locale commands read the language files, switch the language and check the translations", "[runtime][locale]") {
    const std::filesystem::path dir = root() / "samples" / "hello" / "locales";
    std::filesystem::create_directories(dir);
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove_all(p); } } cleanup{dir};
    std::ofstream(dir / "en.json") << R"({"menu": {"start": "Start", "hello": "Hello, {name}"}, "coins": "{n, plural, one {# coin} other {# coins}}"})";
    std::ofstream(dir / "fr.json") << R"({"menu": {"start": "Commencer", "hello": "Bonjour"}, "extra": "en trop"})";
    std::ofstream(dir / "bad.json") << R"({"menu": {"start": 3}})";
    app::Session s(hello_options(1000));
    REQUIRE(s.start().has_value());
    Json get = s.command("locale.get", Json::object()).value();
    REQUIRE(get["lang"] == "en");
    REQUIRE(get["default"] == "en");
    REQUIRE(get["languages"] == Json::array({"bad", "en", "fr"}));
    // Nested groups flattened to dotted keys.
    Json table = s.command("locale.table", Json{{"lang", "en"}}).value();
    REQUIRE(table["keys"] == 3);
    REQUIRE(table["strings"]["menu.hello"] == "Hello, {name}");
    // The check: French lacks the plural, has an extra key, and dropped a placeholder; the bad file says why.
    Json check = s.command("locale.check", Json::object()).value();
    INFO(check.dump());
    REQUIRE(check["complete"] == false);
    REQUIRE(check["languages"]["fr"]["missing"] == Json::array({"coins"}));
    REQUIRE(check["languages"]["fr"]["extra"] == Json::array({"extra"}));
    REQUIRE(check["languages"]["fr"]["placeholders"] == Json::array({"menu.hello"}));
    REQUIRE(check["languages"]["bad"]["error"].get<std::string>().find("'menu.start' is not a text") != std::string::npos);
    // Switching: to a language with a file, the tick tells the scripts, locale.changed is emitted; not to one without.
    REQUIRE(s.command("locale.set", Json{{"lang", "fr"}}).value()["lang"] == "fr");
    REQUIRE(s.command("locale.get", Json::object()).value()["lang"] == "fr");
    REQUIRE(s.command("events.histogram", Json::object()).value()["locale.changed"] == 1);
    auto missing = s.command("locale.set", Json{{"lang", "de"}});
    REQUIRE_FALSE(missing.has_value());
    REQUIRE(missing.error().message.find("the project has: bad, en, fr") != std::string::npos);
    REQUIRE_FALSE(s.command("locale.set", Json{{"lang", "../en"}}).has_value());
    REQUIRE(s.finish().has_value());
}

TEST_CASE("WebSocket pieces: SHA-1, the RFC's accept key, frames of every length form, masked and not, arriving in parts", "[runtime][net][websocket]") {
    namespace ws = app::ws;
    // SHA-1 of "abc" (FIPS 180), and RFC 6455's example handshake.
    const auto d = ws::sha1("abc");
    REQUIRE(ws::base64(d.data(), d.size()) == "qZk+NkcGgWq6PiVxeFDCbJzQ2J0=");
    REQUIRE(ws::accept_key("dGhlIHNhbXBsZSBub25jZQ==") == "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    const std::uint8_t mask[4] = {0x37, 0xfa, 0x21, 0x3d};
    // RFC 6455 5.7: a masked "Hello" from a client.
    const std::string hello = ws::frame("Hello", ws::Text, mask);
    REQUIRE(hello == std::string("\x81\x85\x37\xfa\x21\x3d\x7f\x9f\x4d\x51\x58", 11));
    for (std::size_t size : {std::size_t{5}, std::size_t{125}, std::size_t{126}, std::size_t{300}, std::size_t{65535}, std::size_t{70000}}) {
        std::string payload(size, 'x');
        for (std::size_t i = 0; i < size; ++i) payload[i] = static_cast<char>('a' + i % 26);
        for (const std::uint8_t* m : {static_cast<const std::uint8_t*>(nullptr), mask}) {
            std::string wire = ws::frame(payload, ws::Text, m) + ws::frame("next", ws::Ping, m);
            // Arriving a few bytes at a time: nothing until it is whole, then exactly it.
            std::string in;
            int op = 0;
            std::string got;
            std::size_t fed = 0;
            bool whole = false;
            while (!whole) {
                const std::size_t take = std::min<std::size_t>(size < 1000 ? 3 : 4099, wire.size() - fed);
                in.append(wire, fed, take);
                fed += take;
                whole = ws::take(in, op, got);
                if (!whole) REQUIRE(fed < wire.size());
            }
            INFO(size << (m ? " masked" : ""));
            REQUIRE(op == ws::Text);
            REQUIRE(got == payload);
            in.append(wire, fed, std::string::npos);
            REQUIRE(ws::take(in, op, got));
            REQUIRE(op == ws::Ping);
            REQUIRE(got == "next");
            REQUIRE(in.empty());
        }
    }
}

TEST_CASE("a peer joins a lockstep game over a WebSocket (as a browser does) and plays in step", "[runtime][net][websocket]") {
    app::Options ho = arena_options();
    ho.net_host = 0;
    ho.net_players = 2;
    std::promise<int> port;
    auto port_ready = port.get_future();
    Peer host, player;
    std::thread host_thread([&] { play_peer(ho, 180, "move_x", 1, &port, host); });
    const int p = port_ready.get();
    REQUIRE(p > 0);
    app::Options po = arena_options();
    po.net_join = "ws://127.0.0.1:" + std::to_string(p) + "/";
    std::thread player_thread([&] { play_peer(po, 180, "move_z", 1, nullptr, player); });
    host_thread.join();
    player_thread.join();
    INFO("host " << host.error << " " << host.net.dump());
    INFO("player " << player.error << " " << player.net.dump());
    REQUIRE(host.error.empty());
    REQUIRE(player.error.empty());
    REQUIRE(player.net["player"] == 1);
    REQUIRE(host.state["tick"] == 180);
    REQUIRE(player.state["tick"] == 180);
    REQUIRE(host.state["state_hash"] == player.state["state_hash"]);
    REQUIRE(host.state["state"]["blue.z"].get<double>() > -6.0 + 2.0);    // the WebSocket peer's key moved its player
    REQUIRE(host.net["desyncs"] == 0);
}

TEST_CASE("world.lint names what is likely wrong, says how to fix it, and every sample is clean but for its deliberate miss", "[runtime][lint]") {
    {
        app::Options o;
        o.project_dir = root() / "samples" / "playground";
        o.bundle = root() / "build" / "ts" / "playground.js";
        o.project_config = o.bundle.string() + ".project.json";
        o.headless = true;
        o.frames = 100;
        o.width = 160;
        o.height = 90;
        o.log_level = "warn";
        app::Session s(o);
        REQUIRE(s.start().has_value());
        REQUIRE(s.command("world.clear", Json::object()).has_value());
        auto spawn = [&](const char* name, Json parts) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", std::move(parts)}}).has_value()); };
        const Json box{{"shape", 0}, {"size", Json{{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}};
        spawn("Loose", Json{{"Transform", Json::object()}, {"Collider", box}});
        spawn("Weightless", Json{{"Transform", Json::object()}, {"RigidBody", Json{{"kind", 0}, {"mass", 0}}}, {"Collider", box}});
        spawn("Ghost", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", "assets/nope.glb"}}}});
        Json flat = Json::object();
        flat["Transform"]["scale"] = Json{{"x", 1}, {"y", 0}, {"z", 1}};
        flat["MeshRenderer"]["mesh"] = "cube";
        spawn("Flat", flat);
        spawn("Strewn", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", "sphere"}}}, {"Scatter", Json{{"on", "Nobody"}, {"count", 5}}}});
        spawn("Eye", Json{{"Transform", Json::object()}, {"Camera", Json{{"active", false}}}, {"CameraRig", Json{{"target", "Nobody"}}}});
        auto lint = [&]() { return s.command("world.lint", Json::object()).value(); };
        auto has = [](const Json& l, const char* path, const char* component, const char* severity) {
            for (const Json& q : l["problems"]) {
                if (q.value("path", "") == path && q["component"] == component && q["severity"] == severity) return true;
            }
            return false;
        };
        Json l = lint();
        INFO(l.dump(1));
        REQUIRE(l["ok"] == false);
        REQUIRE(has(l, "/Loose", "Collider", "warning"));
        REQUIRE(has(l, "/Weightless", "RigidBody", "error"));
        REQUIRE(has(l, "/Ghost", "MeshRenderer", "error"));
        REQUIRE(has(l, "/Flat", "Transform", "warning"));
        REQUIRE(has(l, "/Strewn", "Scatter", "error"));
        REQUIRE(has(l, "/Eye", "CameraRig", "error"));
        bool camera = false;
        for (const Json& q : l["problems"]) camera = camera || (q["component"] == "Camera" && q["severity"] == "error");
        REQUIRE(camera);   // a camera, none active
        for (const Json& q : l["problems"]) REQUIRE_FALSE(q["fix"].get<std::string>().empty());
        // Fixed, each goes away; with all of them fixed the world is clean.
        REQUIRE(s.command("world.set", Json{{"entity", "Loose"}, {"component", "RigidBody"}, {"value", Json{{"kind", 1}}}}).has_value());
        REQUIRE(s.command("world.set", Json{{"entity", "Weightless"}, {"component", "RigidBody"}, {"value", Json{{"mass", 1}}}}).has_value());
        REQUIRE(s.command("world.set", Json{{"entity", "Ghost"}, {"component", "MeshRenderer"}, {"value", Json{{"mesh", "cube"}}}}).has_value());
        REQUIRE(s.command("world.set", Json{{"entity", "Flat"}, {"component", "Transform"}, {"value", Json{{"scale", Json{{"x", 1}, {"y", 1}, {"z", 1}}}}}}).has_value());
        REQUIRE(s.command("world.set", Json{{"entity", "Strewn"}, {"component", "Scatter"}, {"value", Json{{"on", ""}}}}).has_value());
        REQUIRE(s.command("world.set", Json{{"entity", "Eye"}, {"component", "Camera"}, {"value", Json{{"active", true}}}}).has_value());
        REQUIRE(s.command("world.set", Json{{"entity", "Eye"}, {"component", "CameraRig"}, {"value", Json{{"target", "Flat"}}}}).has_value());
        l = lint();
        INFO(l.dump(1));
        REQUIRE(l["ok"] == true);
        REQUIRE(l["errors"] == 0);
        REQUIRE(l["warnings"] == 0);
        REQUIRE(s.finish().has_value());
    }
    // Every sample as it ships: no errors but the assets sample's Missing, which shows the fallback.
    for (const auto& entry : std::filesystem::directory_iterator(root() / "samples")) {
        const std::string name = entry.path().filename().string();
        const std::filesystem::path bundle = root() / "build" / "ts" / (name + ".js");
        if (!std::filesystem::exists(bundle)) continue;
        app::Options o;
        o.project_dir = entry.path();
        o.bundle = bundle;
        if (std::filesystem::exists(bundle.string() + ".project.json")) o.project_config = bundle.string() + ".project.json";
        o.headless = true;
        o.frames = 100;
        o.width = 160;
        o.height = 90;
        o.log_level = "error";
        app::Session s(o);
        REQUIRE(s.start().has_value());
        for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
        const Json l = s.command("world.lint", Json::object()).value();
        INFO(name << ": " << l.dump(1));
        for (const Json& q : l["problems"]) {
            if (q["severity"] != "error") continue;
            REQUIRE((name == "assets" && q.value("path", "") == "/Missing"));
        }
        REQUIRE(l["warnings"] == 0);
        REQUIRE(s.finish().has_value());
    }
}

TEST_CASE("render.views draws the scene or an entity from standard views into one sheet without touching the world", "[runtime][views]") {
    const std::filesystem::path out = root() / "build" / "test-out";
    std::filesystem::create_directories(out);
    app::Options o;
    o.project_dir = root() / "samples" / "physics";
    o.bundle = root() / "build" / "ts" / "physics.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 100;
    o.width = 240;
    o.height = 150;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 5; ++i) REQUIRE(s.frame().has_value());
    const Json before = s.command("state", Json::object()).value();
    const auto camera = s.command("render.stats", Json::object()).value()["camera"];
    // The whole scene from six sides: a sheet of three by two tiles of the frame's size.
    Json v = s.command("render.views", Json{{"path", (out / "views.png").string()}}).value();
    INFO(v.dump());
    REQUIRE(std::filesystem::exists(out / "views.png"));
    REQUIRE(v["views"].size() == 6);
    REQUIRE(v["columns"] == 3);
    REQUIRE(v["rows"] == 2);
    REQUIRE(v["width"] == 240 * 3);
    REQUIRE(v["height"] == 150 * 2);
    // One entity from four: two by two, framed on it (the eye of the top view straight over its centre).
    v = s.command("render.views", Json{{"path", (out / "views-ramp.png").string()}, {"entity", "Ramp"}, {"views", Json::array({"front", "right", "top", "perspective"})}}).value();
    REQUIRE(v["columns"] == 2);
    REQUIRE(v["rows"] == 2);
    REQUIRE(v["entity"] == "/Ramp");
    REQUIRE(v["views"][2]["eye"]["x"].get<double>() == Catch::Approx(v["center"]["x"].get<double>()).margin(1e-3));
    REQUIRE(v["views"][2]["eye"]["y"].get<double>() > v["center"]["y"].get<double>() + 1);
    // Nothing moved and the scene's own camera is back.
    const Json after = s.command("state", Json::object()).value();
    REQUIRE(after["tick"] == before["tick"]);
    REQUIRE(after["world_hash"] == before["world_hash"]);
    REQUIRE(s.command("render.stats", Json::object()).value()["camera"] == camera);
    // Asked wrongly, it says so.
    REQUIRE_FALSE(s.command("render.views", Json{{"path", (out / "x.png").string()}, {"views", Json::array({"sideways"})}}).has_value());
    REQUIRE_FALSE(s.command("render.views", Json::object()).has_value());
    REQUIRE(s.finish().has_value());
}
