// The editor is a TypeScript program on Pocket UI; these tests open it headless beside a sample
// project and operate it the way an agent would: read the interface as text, click its buttons.
#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cmath>
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
    REQUIRE(s.world().entity_count() == 19);
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
    // The Overlays button draws colliders and joints as lines in the scene pane.
    REQUIRE(ok(s.command("render.stats", Json::object()))["debug_lines"].get<int>() == 0);
    ok(s.command("ui.click", Json{{"id", find_named(s, "overlays")}}));
    ok(s.idle_frame());
    Json ds = ok(s.command("debug.stats", Json::object()));
    REQUIRE(ds["colliders"] == true);
    REQUIRE(ds["joints"] == true);
    REQUIRE(ok(s.command("render.stats", Json::object()))["debug_lines"].get<int>() > 12 * 7);  // seven boxes, spheres, a capsule, joints
    ok(s.command("ui.click", Json{{"id", find_named(s, "overlays")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("debug.stats", Json::object()))["colliders"] == false);
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
    REQUIRE(s.world().entity_count() == 19);
    t = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(1.25));
    Json contexts = ok(s.command("script.contexts", Json::object()));
    REQUIRE(contexts.size() == 2);
    // The project's exposed state is gone until the next Play (state is sampled per tick).
    ok(s.command("step", Json{{"ticks", 1}}));
    st = ok(s.command("state", Json::object()));
    REQUIRE_FALSE(st["state"].contains("dropped"));
    REQUIRE(s.world().entity_count() == 19);  // stepping without the project spawns nothing
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

TEST_CASE("editor edits are undoable and redoable", "[editor]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    Json before = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    double x0 = before["position"]["x"].get<double>();

    // Inspector edit, then Cmd+Z / Cmd+Shift+Z through the keyboard path, then the toolbar buttons.
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ramp")}}));
    ok(s.idle_frame());
    int input = find_named(s, "Transform.position.x");
    ok(s.command("ui.click", Json{{"id", input}}));
    ok(s.command("ui.key", Json{{"key", "End"}}));
    for (int i = 0; i < 10; ++i) ok(s.command("ui.key", Json{{"key", "Backspace"}}));
    ok(s.command("ui.type", Json{{"text", "2.5"}}));
    ok(s.command("ui.key", Json{{"key", "Return"}}));
    REQUIRE(ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}))["position"]["x"].get<double>() == Catch::Approx(2.5));
    ok(s.command("ui.key", Json{{"key", "Escape"}}));  // drop focus so shortcuts reach the editor
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    REQUIRE(ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}))["position"]["x"].get<double>() == Catch::Approx(x0));
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta", "shift"})}}));
    REQUIRE(ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}))["position"]["x"].get<double>() == Catch::Approx(2.5));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}))["position"]["x"].get<double>() == Catch::Approx(x0));

    // Spawn, delete and duplicate come back exactly.
    std::size_t n = s.world().entity_count();
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "spawn")}}));
    REQUIRE(s.world().entity_count() == n + 1);
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(s.world().entity_count() == n);
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "redo")}}));
    REQUIRE(s.world().entity_count() == n + 1);
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(s.world().entity_count() == n);

    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ramp")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "delete")}}));
    REQUIRE(s.world().entity_count() < n);
    REQUIRE(ok(s.command("world.find", Json{{"path", "/Ramp"}})).is_null());
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(s.world().entity_count() == n);
    Json restored = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(restored["position"]["x"].get<double>() == Catch::Approx(x0));
    REQUIRE(restored["scale"] == before["scale"]);

    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ramp")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "duplicate")}}));
    REQUIRE(s.world().entity_count() == n + 1);
    REQUIRE_FALSE(ok(s.command("world.find", Json{{"path", "/Ramp 2"}})).is_null());
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(ok(s.command("world.find", Json{{"path", "/Ramp 2"}})).is_null());
    REQUIRE(s.world().entity_count() == n);
    ok(s.finish());
}

TEST_CASE("editor gizmo drags the selection along a world axis", "[editor]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ramp")}}));
    ok(s.idle_frame());
    ok(s.idle_frame());
    Json before = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    // The X handle sits where the axis tip projects; drag it further along the axis' screen direction.
    Json cq = ok(s.command("ui.query", Json{{"name", "gizmo:plane"}}));
    Json xq = ok(s.command("ui.query", Json{{"name", "gizmo:x"}}));
    REQUIRE(cq.size() == 1);
    REQUIRE(xq.size() == 1);
    double cx = cq[0]["rect"]["x"].get<double>(), cy = cq[0]["rect"]["y"].get<double>();
    double tx = xq[0]["rect"]["x"].get<double>(), ty = xq[0]["rect"]["y"].get<double>();
    double dx = tx - cx, dy = ty - cy;
    REQUIRE(std::hypot(dx, dy) > 5);
    ok(s.command("ui.drag", Json{{"id", xq[0]["id"]}, {"dx", dx}, {"dy", dy}, {"steps", 5}}));
    Json after = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    INFO(before.dump() << " -> " << after.dump());
    // One axis length further along +X, nothing else touched.
    REQUIRE(after["position"]["x"].get<double>() > before["position"]["x"].get<double>() + 0.1);
    REQUIRE(after["position"]["y"].get<double>() == Catch::Approx(before["position"]["y"].get<double>()).margin(1e-4));
    REQUIRE(after["position"]["z"].get<double>() == Catch::Approx(before["position"]["z"].get<double>()).margin(1e-4));
    // The move is one undo step, and the handles followed the entity.
    ok(s.idle_frame());
    Json xq2 = ok(s.command("ui.query", Json{{"name", "gizmo:x"}}));
    REQUIRE(std::abs(xq2[0]["rect"]["x"].get<double>() - tx) + std::abs(xq2[0]["rect"]["y"].get<double>() - ty) > 1);
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    Json undone = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(undone["position"]["x"].get<double>() == Catch::Approx(before["position"]["x"].get<double>()).margin(1e-5));

    // The rotate handle turns around world Y (100 px per radian); the scale handle grows uniformly.
    ok(s.idle_frame());
    Json rq = ok(s.command("ui.query", Json{{"name", "gizmo:rotate"}}));
    REQUIRE(rq.size() == 1);
    ok(s.command("ui.drag", Json{{"id", rq[0]["id"]}, {"dx", 314}, {"dy", 0}, {"steps", 5}}));
    Json turned = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    INFO(turned.dump());
    // Ramp starts rolled about Z (w = 0.966); a half turn about world Y moves that weight into y.
    REQUIRE(std::abs(turned["rotation"]["y"].get<double>()) > 0.9);
    REQUIRE(turned["position"] == undone["position"]);
    ok(s.idle_frame());
    Json sq = ok(s.command("ui.query", Json{{"name", "gizmo:scale"}}));
    REQUIRE(sq.size() == 1);
    ok(s.command("ui.drag", Json{{"id", sq[0]["id"]}, {"dx", 139}, {"dy", 0}, {"steps", 5}}));
    Json grown = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(grown["scale"]["x"].get<double>() == Catch::Approx(before["scale"]["x"].get<double>() * 2.0).epsilon(0.02));
    REQUIRE(grown["scale"]["y"].get<double>() == Catch::Approx(before["scale"]["y"].get<double>() * 2.0).epsilon(0.02));
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    Json back = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(back["scale"] == before["scale"]);
    REQUIRE(back["rotation"]["y"].get<double>() == Catch::Approx(before["rotation"]["y"].get<double>()).margin(1e-5));

    // Shift-click extends the selection; the toolbar reports the count through the inspector title.
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ground")}, {"mods", Json::array({"shift"})}}));
    ok(s.idle_frame());
    std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 8}, {"max_nodes", 2000}}))["text"].get<std::string>();
    INFO(snap);
    REQUIRE(snap.find("(+1 more)") != std::string::npos);
    ok(s.finish());
}

TEST_CASE("editor remembers its layout in the project", "[editor]") {
    std::filesystem::path saved = root() / "samples" / "physics" / ".pocket" / "editor.json";
    std::filesystem::remove(saved);
    {
        app::Session s(editor_options("physics"));
        ok(s.start());
        s.set_paused(true);
        for (int i = 0; i < 2; ++i) ok(s.idle_frame());
        Json h = ok(s.command("ui.query", Json{{"name", "hierarchy"}}));
        double w0 = h[0]["rect"]["w"].get<double>();
        ok(s.command("ui.drag", Json{{"id", find_named(s, "split:hierarchy")}, {"dx", 60}, {"dy", 0}}));
        ok(s.idle_frame());
        h = ok(s.command("ui.query", Json{{"name", "hierarchy"}}));
        REQUIRE(h[0]["rect"]["w"].get<double>() == Catch::Approx(w0 + 60).margin(1));
        ok(s.command("ui.click", Json{{"id", find_named(s, "tab:events")}}));
        ok(s.finish());
    }
    REQUIRE(std::filesystem::exists(saved));
    {
        app::Session s(editor_options("physics"));
        ok(s.start());
        s.set_paused(true);
        for (int i = 0; i < 2; ++i) ok(s.idle_frame());
        Json h = ok(s.command("ui.query", Json{{"name", "hierarchy"}}));
        REQUIRE(h[0]["rect"]["w"].get<double>() == Catch::Approx(300).margin(1));
        std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 4}}))["text"].get<std::string>();
        REQUIRE(snap.find("bottom-body") != std::string::npos);
        ok(s.finish());
    }
    std::filesystem::remove(saved);
}
