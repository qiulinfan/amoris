// The editor is a TypeScript program on Pocket UI; these tests open it headless beside a sample
// project and operate it the way an agent would: read the interface as text, click its buttons.
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

app::Options editor_options(const char* sample) {
    app::Options o;
    o.project_dir = root() / "samples" / sample;
    o.bundle = root() / "build" / "ts" / (std::string(sample) + ".js");
    o.project_config = o.bundle.string() + ".project.json";
    o.editor_bundle = root() / "build" / "ts" / "editor.js";
    o.headless = true;
    o.paused = true;
    o.width = 1024;
    o.height = 640;
    o.log_level = "warn";
    return o;
}

Json ok(Result<Json> r) {
    std::string err = r ? std::string() : r.error().to_string();
    INFO(err);
    REQUIRE(r.has_value());
    return *r;
}

void ok(Status r) {
    std::string err = r ? std::string() : r.error().to_string();
    INFO(err);
    REQUIRE(r.has_value());
}

int find_named(app::Session& s, const char* name) {
    Json q = ok(s.command("ui.query", Json{{"name", name}}));
    REQUIRE(q.size() == 1);
    return q[0]["id"].get<int>();
}

}  // namespace

TEST_CASE("editor opens a project paused and shows its world", "[editor]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    REQUIRE(s.tick() == 0);  // the project has not started: no ticks, no crates
    REQUIRE(s.world().entity_count() == 9);
    std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 3}}))["text"].get<std::string>();
    INFO(snap);
    REQUIRE(snap.find("toolbar") != std::string::npos);
    REQUIRE(snap.find("hierarchy") != std::string::npos);
    REQUIRE(snap.find("viewport") != std::string::npos);
    REQUIRE(snap.find("inspector") != std::string::npos);
    // The hierarchy lists the scene's entities by name.
    Json rows = ok(s.command("ui.query", Json{{"name", "entity:Ramp"}}));
    REQUIRE(rows.size() == 1);
    // The scene pane confines the renderer.
    Json vp = ok(s.command("render.viewport", Json::object()));
    REQUIRE(vp["full"] == false);
    REQUIRE(vp["w"].get<double>() > 300);
    ok(s.finish());
}

TEST_CASE("editor inspector edits components and play/stop restores the scene", "[editor]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ramp")}}));
    ok(s.idle_frame());
    int input = find_named(s, "Transform.position.x");
    ok(s.command("ui.click", Json{{"id", input}}));
    ok(s.command("ui.key", Json{{"key", "End"}}));
    for (int i = 0; i < 10; ++i) ok(s.command("ui.key", Json{{"key", "Backspace"}}));
    ok(s.command("ui.type", Json{{"text", "1.25"}}));
    ok(s.command("ui.key", Json{{"key", "Return"}}));
    Json t = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(1.25));

    // Play starts the project's scripts (they drop crates) and resumes the simulation.
    ok(s.command("ui.click", Json{{"id", find_named(s, "play")}}));
    REQUIRE_FALSE(s.paused());
    for (int i = 0; i < 30; ++i) ok(s.frame());
    REQUIRE(s.tick() == 30);
    REQUIRE(s.world().entity_count() > 9);
    Json st = ok(s.command("state", Json::object()));
    REQUIRE(st["state"].contains("dropped"));

    // Pause, then Stop: scripts unloaded, the edited scene comes back, the tick counter keeps counting frames but no crates remain.
    ok(s.command("ui.click", Json{{"id", find_named(s, "pause")}}));
    REQUIRE(s.paused());
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "stop")}}));
    ok(s.idle_frame());
    REQUIRE(s.world().entity_count() == 9);
    t = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(1.25));
    Json contexts = ok(s.command("script.contexts", Json::object()));
    REQUIRE(contexts.size() == 2);
    // The project's exposed state is gone until the next Play (state is sampled per tick).
    ok(s.command("step", Json{{"ticks", 1}}));
    st = ok(s.command("state", Json::object()));
    REQUIRE_FALSE(st["state"].contains("dropped"));
    REQUIRE(s.world().entity_count() == 9);  // stepping without the project spawns nothing
    ok(s.finish());
}

TEST_CASE("editor hosts the project's interface inside the scene pane", "[editor]") {
    app::Session s(editor_options("ui"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    int viewport = find_named(s, "viewport");
    int hud = find_named(s, "hud");
    Json d = ok(s.command("ui.describe", Json{{"id", hud}}));
    // The HUD's parent chain reaches the viewport, and it sits inside the pane's rectangle.
    Json vp = ok(s.command("ui.describe", Json{{"id", viewport}}));
    REQUIRE(d["rect"]["x"].get<double>() >= vp["rect"]["x"].get<double>());
    REQUIRE(d["rect"]["y"].get<double>() >= vp["rect"]["y"].get<double>());
    int parent = d["parent"].get<int>();
    int hops = 0;
    while (parent != 0 && parent != viewport && hops < 10) {
        parent = ok(s.command("ui.describe", Json{{"id", parent}}))["parent"].get<int>();
        ++hops;
    }
    REQUIRE(parent == viewport);
    ok(s.finish());
}
