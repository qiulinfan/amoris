// The editor is a TypeScript program on Pocket UI; these tests open it headless beside a sample
// project and operate it the way an agent would: read the interface as text, click its buttons.
#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cmath>
#include <cstdlib>
#include <filesystem>
#include <fstream>

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
    INFO("looking for " << name << ": " << q.dump());
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
    REQUIRE(s.world().entity_count() == 20);
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
    REQUIRE(s.world().entity_count() == 20);
    t = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(1.25));
    Json contexts = ok(s.command("script.contexts", Json::object()));
    REQUIRE(contexts.size() == 2);
    // The project's exposed state is gone until the next Play (state is sampled per tick).
    ok(s.command("step", Json{{"ticks", 1}}));
    st = ok(s.command("state", Json::object()));
    REQUIRE_FALSE(st["state"].contains("dropped"));
    REQUIRE(s.world().entity_count() == 20);  // stepping without the project spawns nothing
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
    // One axis at a time: SX doubles x alone; RZ turns a half turn about world Z, which flips the
    // ramp's roll (it starts rolled about Z) into a rotation whose z weight is large.
    ok(s.idle_frame());
    Json sxq = ok(s.command("ui.query", Json{{"name", "gizmo:scale_x"}}));
    REQUIRE(sxq.size() == 1);
    ok(s.command("ui.drag", Json{{"id", sxq[0]["id"]}, {"dx", 139}, {"dy", 0}, {"steps", 5}}));
    Json wide = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(wide["scale"]["x"].get<double>() == Catch::Approx(before["scale"]["x"].get<double>() * 2.0).epsilon(0.02));
    REQUIRE(wide["scale"]["y"].get<double>() == Catch::Approx(before["scale"]["y"].get<double>()).margin(1e-6));
    REQUIRE(wide["scale"]["z"].get<double>() == Catch::Approx(before["scale"]["z"].get<double>()).margin(1e-6));
    ok(s.idle_frame());
    Json rzq = ok(s.command("ui.query", Json{{"name", "gizmo:rotate_z"}}));
    REQUIRE(rzq.size() == 1);
    ok(s.command("ui.drag", Json{{"id", rzq[0]["id"]}, {"dx", 314}, {"dy", 0}, {"steps", 5}}));
    Json rolled = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    INFO(rolled.dump());
    REQUIRE(std::abs(rolled["rotation"]["z"].get<double>()) > 0.9);
    REQUIRE(std::abs(rolled["rotation"]["y"].get<double>()) < 0.1);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    REQUIRE(ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}))["scale"] == before["scale"]);

    // Shift-click extends the selection; the toolbar reports the count through the inspector title.
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ground")}, {"mods", Json::array({"shift"})}}));
    ok(s.idle_frame());
    std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 8}, {"max_nodes", 2000}}))["text"].get<std::string>();
    INFO(snap);
    REQUIRE(snap.find("(+1 more)") != std::string::npos);
    ok(s.finish());
}

TEST_CASE("editor reparents by dragging a hierarchy row onto another", "[editor][reparent]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    Json ramp = ok(s.command("ui.query", Json{{"name", "entity:Ramp"}}));
    Json ground = ok(s.command("ui.query", Json{{"name", "entity:Ground"}}));
    REQUIRE(ramp.size() == 1);
    REQUIRE(ground.size() == 1);
    const world::EntityId ramp_id = ok(s.command("world.find", Json{{"path", "Ramp"}})).get<world::EntityId>();
    const world::EntityId ground_id = ok(s.command("world.find", Json{{"path", "Ground"}})).get<world::EntityId>();
    REQUIRE(ok(s.command("world.describe", Json{{"entity", ramp_id}})).value("parent", world::EntityId{0}) == 0);
    // Dragged from its row's center onto the Ground row's center.
    const double dy = (ground[0]["rect"]["y"].get<double>() + ground[0]["rect"]["h"].get<double>() / 2) - (ramp[0]["rect"]["y"].get<double>() + ramp[0]["rect"]["h"].get<double>() / 2);
    ok(s.command("ui.drag", Json{{"id", ramp[0]["id"]}, {"dx", 0}, {"dy", dy}, {"steps", 6}}));
    ok(s.idle_frame());
    Json d = ok(s.command("world.describe", Json{{"entity", ramp_id}}));
    INFO(d.dump());
    REQUIRE(d.value("parent", world::EntityId{0}) == ground_id);
    REQUIRE(d["path"] == "/Ground/Ramp");
    // Its world placement stayed where it was: the local transform absorbed Ground's scale and position.
    {
        const Json wt = ok(s.command("world.get", Json{{"entity", ramp_id}, {"component", "WorldTransform"}}));
        INFO(wt.dump());
        REQUIRE(wt["position"]["x"].get<double>() == Catch::Approx(-4.0).margin(1e-3));
        REQUIRE(wt["position"]["y"].get<double>() == Catch::Approx(0.3).margin(1e-3));
        REQUIRE(wt["scale"]["x"].get<double>() == Catch::Approx(6.0).margin(1e-3));
    }
    // Undo puts it back at the root; redo under Ground again.
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("world.describe", Json{{"entity", ramp_id}})).value("parent", world::EntityId{0}) == 0);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta", "shift"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("world.describe", Json{{"entity", ramp_id}})).value("parent", world::EntityId{0}) == ground_id);
    // Ground cannot go under its own child.
    ok(s.idle_frame());
    ground = ok(s.command("ui.query", Json{{"name", "entity:Ground"}}));
    ramp = ok(s.command("ui.query", Json{{"name", "entity:Ramp"}}));
    const double back = (ramp[0]["rect"]["y"].get<double>() + ramp[0]["rect"]["h"].get<double>() / 2) - (ground[0]["rect"]["y"].get<double>() + ground[0]["rect"]["h"].get<double>() / 2);
    ok(s.command("ui.drag", Json{{"id", ground[0]["id"]}, {"dx", 0}, {"dy", back}, {"steps", 6}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("world.describe", Json{{"entity", ground_id}})).value("parent", world::EntityId{0}) == 0);
}

TEST_CASE("editor lists the project's assets and places one dropped on the scene", "[editor][assets]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "tab:assets")}}));
    ok(s.idle_frame());
    Json row = ok(s.command("ui.query", Json{{"name", "asset:assets/bowl.glb"}}));
    REQUIRE(row.size() == 1);
    // A model's row carries a thumbnail drawn by assets.preview into the project's .pocket/thumbs.
    REQUIRE(ok(s.command("ui.query", Json{{"name", "thumb:assets/bowl.glb"}})).size() == 1);
    REQUIRE(std::filesystem::is_regular_file(root() / "samples" / "physics" / ".pocket" / "thumbs" / "assets_bowl.glb.png"));
    // A click describes the file.
    ok(s.command("ui.click", Json{{"id", row[0]["id"]}}));
    ok(s.idle_frame());
    {
        std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 12}}))["text"].get<std::string>();
        INFO(snap);
        REQUIRE(snap.find("assets/bowl.glb:") != std::string::npos);
        REQUIRE(snap.find("vertices") != std::string::npos);
    }
    REQUIRE_FALSE(ok(s.command("world.find", Json{{"path", "bowl"}})).is_number());
    // Dragged onto the middle of the scene pane, it becomes an entity on the ground under the pointer.
    Json vp = ok(s.command("ui.query", Json{{"name", "viewport"}}));
    REQUIRE(vp.size() == 1);
    row = ok(s.command("ui.query", Json{{"name", "asset:assets/bowl.glb"}}));
    const double cx = vp[0]["rect"]["x"].get<double>() + vp[0]["rect"]["w"].get<double>() / 2, cy = vp[0]["rect"]["y"].get<double>() + vp[0]["rect"]["h"].get<double>() / 2;
    const double dx = cx - (row[0]["rect"]["x"].get<double>() + row[0]["rect"]["w"].get<double>() / 2), dy = cy - (row[0]["rect"]["y"].get<double>() + row[0]["rect"]["h"].get<double>() / 2);
    ok(s.command("ui.drag", Json{{"id", row[0]["id"]}, {"dx", dx}, {"dy", dy}, {"steps", 6}}));
    ok(s.idle_frame());
    Json found = ok(s.command("world.find", Json{{"path", "bowl"}}));
    INFO(found.dump());
    REQUIRE(found.is_number());
    const world::EntityId id = found.get<world::EntityId>();
    REQUIRE(ok(s.command("world.get", Json{{"entity", id}, {"component", "MeshRenderer"}}))["mesh"] == "assets/bowl.glb");
    {
        Json root = ok(s.command("ui.describe", Json{{"id", 1}}));
        const double scale = root["rect"]["w"].get<double>() > 0 ? 1024.0 / root["rect"]["w"].get<double>() : 1.0;
        Json ray = ok(s.command("render.unproject", Json{{"x", cx * scale}, {"y", cy * scale}, {"plane", "xz"}, {"at", 0}}));
        INFO(ray.dump());
        REQUIRE(ray["hit"] == true);
        Json t = ok(s.command("world.get", Json{{"entity", id}, {"component", "Transform"}}));
        INFO(t.dump());
        REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(ray["point"]["x"].get<double>()).margin(1e-3));
        REQUIRE(t["position"]["y"].get<double>() == Catch::Approx(0.0).margin(1e-3));
        REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(ray["point"]["z"].get<double>()).margin(1e-3));
    }
    // The hierarchy shows it selected; undo removes it, redo brings it back with the same mesh.
    REQUIRE(ok(s.command("ui.query", Json{{"name", "entity:bowl"}})).size() == 1);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    REQUIRE_FALSE(ok(s.command("world.find", Json{{"path", "bowl"}})).is_number());
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta", "shift"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("world.get", Json{{"entity", "bowl"}, {"component", "MeshRenderer"}}))["mesh"] == "assets/bowl.glb");
    // A second drop gets a name of its own.
    row = ok(s.command("ui.query", Json{{"name", "asset:assets/bowl.glb"}}));
    const double dx2 = cx - (row[0]["rect"]["x"].get<double>() + row[0]["rect"]["w"].get<double>() / 2), dy2 = cy - (row[0]["rect"]["y"].get<double>() + row[0]["rect"]["h"].get<double>() / 2);
    ok(s.command("ui.drag", Json{{"id", row[0]["id"]}, {"dx", dx2}, {"dy", dy2}, {"steps", 6}}));
    ok(s.idle_frame());
    INFO(ok(s.command("ui.query", Json{{"name", "notice"}})).dump());
    REQUIRE(ok(s.command("world.find", Json{{"path", "bowl 2"}})).is_number());
    // Dropped back on the list, nothing is placed.
    ok(s.command("ui.drag", Json{{"id", row[0]["id"]}, {"dx", 0}, {"dy", -4}, {"steps", 3}}));
    ok(s.idle_frame());
    REQUIRE_FALSE(ok(s.command("world.find", Json{{"path", "bowl 3"}})).is_number());
    ok(s.finish());
    std::filesystem::remove_all(root() / "samples" / "physics" / ".pocket");   // the layout file the tab click wrote
}

TEST_CASE("editor's Input tab rebinds actions live, undoably, and saves them to input.json", "[editor][input]") {
    const std::filesystem::path saved = root() / "samples" / "sprites" / "input.json";
    std::filesystem::remove(saved);
    {
        app::Session s(editor_options("sprites"));
        ok(s.start());
        s.set_paused(true);
        for (int i = 0; i < 2; ++i) ok(s.idle_frame());
        ok(s.command("ui.click", Json{{"id", find_named(s, "tab:input")}}));
        ok(s.idle_frame());
        Json jump = ok(s.command("ui.query", Json{{"name", "action:jump:positive"}}));
        REQUIRE(jump.size() == 1);
        REQUIRE(ok(s.command("ui.describe", Json{{"id", jump[0]["id"]}}))["value"] == "Space, W, Up, pad:a");
        // A key added at the end of the field binds at once.
        ok(s.command("ui.click", Json{{"id", jump[0]["id"]}}));
        ok(s.command("ui.key", Json{{"key", "End"}}));
        ok(s.command("ui.type", Json{{"text", ", Q"}}));
        ok(s.command("ui.key", Json{{"key", "Return"}}));
        ok(s.idle_frame());
        Json d = ok(s.command("input.describe", Json::object()));
        INFO(d.dump());
        REQUIRE(d["jump"]["positive"].size() == 5);
        REQUIRE(d["jump"]["positive"][4] == "Q");
        ok(s.command("input.hold", Json{{"key", "Q"}, {"ticks", 3}}));
        ok(s.frame());
        REQUIRE(ok(s.command("input.actions", Json::object()))["jump"]["down"] == true);
        // Undo takes it back; redo binds it again (Escape first: shortcuts wait while a field has focus).
        ok(s.command("ui.key", Json{{"key", "Escape"}}));
        ok(s.idle_frame());
        ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
        ok(s.idle_frame());
        REQUIRE(ok(s.command("input.describe", Json::object()))["jump"]["positive"].size() == 4);
        ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta", "shift"})}}));
        ok(s.idle_frame());
        REQUIRE(ok(s.command("input.describe", Json::object()))["jump"]["positive"].size() == 5);
        // A new action from its name and keys (an action without keys is refused); Save writes input.json.
        ok(s.command("ui.click", Json{{"id", find_named(s, "action:new")}}));
        ok(s.command("ui.type", Json{{"text", "dash"}}));
        ok(s.command("ui.key", Json{{"key", "Return"}}));
        ok(s.idle_frame());
        ok(s.command("ui.click", Json{{"id", find_named(s, "action:add")}}));
        ok(s.idle_frame());
        REQUIRE_FALSE(ok(s.command("input.describe", Json::object())).contains("dash"));
        ok(s.command("ui.click", Json{{"id", find_named(s, "action:new:keys")}}));
        ok(s.command("ui.type", Json{{"text", "LShift pad:b"}}));
        ok(s.command("ui.key", Json{{"key", "Return"}}));
        ok(s.idle_frame());
        REQUIRE(ok(s.command("input.describe", Json::object()))["dash"]["positive"] == Json::array({"LShift", "pad:b"}));
        REQUIRE(ok(s.command("ui.query", Json{{"name", "action:dash:positive"}})).size() == 1);
        ok(s.command("ui.click", Json{{"id", find_named(s, "save_bindings")}}));
        ok(s.idle_frame());
        REQUIRE(std::filesystem::exists(saved));
        Json file = Json::parse(ok(s.command("project.read", Json{{"path", "input.json"}}))["text"].get<std::string>());
        REQUIRE(file["actions"]["dash"]["positive"] == Json::array({"LShift", "pad:b"}));
        REQUIRE(file["actions"]["jump"]["positive"].size() == 5);
        // Remove takes an action out.
        ok(s.command("ui.click", Json{{"id", find_named(s, "action:dash:remove")}}));
        ok(s.idle_frame());
        REQUIRE_FALSE(ok(s.command("input.describe", Json::object())).contains("dash"));
        // Press takes the next key as the part's keyboard binding; pad bindings stay; Escape cancels; undo restores.
        Json was = ok(s.command("input.describe", Json::object()))["jump"]["positive"];
        ok(s.command("ui.click", Json{{"id", find_named(s, "action:jump:positive:press")}}));
        ok(s.idle_frame());
        ok(s.command("ui.key", Json{{"key", "K"}}));
        ok(s.idle_frame());
        Json now = ok(s.command("input.describe", Json::object()))["jump"]["positive"];
        INFO(was.dump() << " -> " << now.dump());
        REQUIRE(now == Json::array({"K", "pad:a"}));
        REQUIRE(ok(s.command("ui.describe", Json{{"id", find_named(s, "action:jump:positive")}}))["value"] == "K, pad:a");
        ok(s.command("ui.click", Json{{"id", find_named(s, "action:jump:negative:press")}}));
        ok(s.idle_frame());
        ok(s.command("ui.key", Json{{"key", "Escape"}}));
        ok(s.idle_frame());
        REQUIRE_FALSE(ok(s.command("input.describe", Json::object()))["jump"].contains("negative"));
        ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
        ok(s.idle_frame());
        REQUIRE(ok(s.command("input.describe", Json::object()))["jump"]["positive"] == was);
        ok(s.finish());
    }
    // The saved file is the map of the next run, in place of project.toml's.
    {
        app::Session s(editor_options("sprites"));
        ok(s.start());
        Json d = ok(s.command("input.describe", Json::object()));
        INFO(d.dump());
        REQUIRE(d.contains("dash"));
        REQUIRE(d["jump"]["positive"].size() == 5);
        ok(s.finish());
    }
    std::filesystem::remove(saved);
    std::filesystem::remove_all(root() / "samples" / "sprites" / ".pocket");
}

TEST_CASE("editor snaps gizmo moves, turns and scales to the grid", "[editor][snap]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ramp")}}));
    ok(s.idle_frame());
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "snap")}}));
    ok(s.idle_frame());
    auto transform = [&]() { return ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}})); };
    auto multiple_of = [](double v, double step) { return std::abs(v / step - std::round(v / step)) < 1e-4; };
    const Json before = transform();
    // The step cycles 0.1, 0.25, 0.5, 1, 2 and is kept with the layout; two clicks from the default half unit make it two units.
    ok(s.command("ui.click", Json{{"id", find_named(s, "snap_step")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "snap_step")}}));
    ok(s.idle_frame());
    {
        Json bq = ok(s.command("ui.query", Json{{"name", "snap_step"}}));
        REQUIRE(bq.size() == 1);
        Json saved = ok(s.command("project.read", Json{{"path", ".pocket/editor.json"}}));
        INFO(bq.dump());
        INFO(saved.dump());
        INFO(ok(s.command("ui.query", Json{{"name", "snap"}})).dump());
        REQUIRE(Json::parse(saved["text"].get<std::string>())["snap_step"] == 2);
    }
    for (int i = 0; i < 3; ++i) { ok(s.command("ui.click", Json{{"id", find_named(s, "snap_step")}})); ok(s.idle_frame()); }   // round to the half unit again
    // A move along X lands on a half unit; the other axes are left alone.
    Json cq = ok(s.command("ui.query", Json{{"name", "gizmo:plane"}}));
    Json xq = ok(s.command("ui.query", Json{{"name", "gizmo:x"}}));
    REQUIRE(cq.size() == 1);
    REQUIRE(xq.size() == 1);
    const double dx = xq[0]["rect"]["x"].get<double>() - cq[0]["rect"]["x"].get<double>(), dy = xq[0]["rect"]["y"].get<double>() - cq[0]["rect"]["y"].get<double>();
    ok(s.command("ui.drag", Json{{"id", xq[0]["id"]}, {"dx", dx * 1.3}, {"dy", dy * 1.3}, {"steps", 5}}));
    Json t = transform();
    INFO(before.dump() << " -> " << t.dump());
    REQUIRE(t["position"]["x"].get<double>() > before["position"]["x"].get<double>() + 0.1);
    REQUIRE(multiple_of(t["position"]["x"].get<double>(), 0.5));
    REQUIRE(t["position"]["y"].get<double>() == Catch::Approx(before["position"]["y"].get<double>()));
    REQUIRE(t["position"]["z"].get<double>() == Catch::Approx(before["position"]["z"].get<double>()));
    // A turn of a radian (100 px) lands on 60 degrees about world Y, applied over the ramp's own roll.
    ok(s.idle_frame());
    Json rq = ok(s.command("ui.query", Json{{"name", "gizmo:rotate"}}));
    REQUIRE(rq.size() == 1);
    ok(s.command("ui.drag", Json{{"id", rq[0]["id"]}, {"dx", 100}, {"dy", 0}, {"steps", 5}}));
    t = transform();
    INFO(t.dump());
    {
        const Json& r0 = before["rotation"];
        const double ax = 0, ay = std::sin(M_PI / 6), az = 0, aw = std::cos(M_PI / 6);   // yaw 60 degrees
        const double bx = r0["x"].get<double>(), by = r0["y"].get<double>(), bz = r0["z"].get<double>(), bw = r0["w"].get<double>();
        const double ex = aw * bx + ax * bw + ay * bz - az * by, ey = aw * by - ax * bz + ay * bw + az * bx, ez = aw * bz + ax * by - ay * bx + az * bw, ew = aw * bw - ax * bx - ay * by - az * bz;
        REQUIRE(t["rotation"]["x"].get<double>() == Catch::Approx(ex).margin(1e-3));
        REQUIRE(t["rotation"]["y"].get<double>() == Catch::Approx(ey).margin(1e-3));
        REQUIRE(t["rotation"]["z"].get<double>() == Catch::Approx(ez).margin(1e-3));
        REQUIRE(t["rotation"]["w"].get<double>() == Catch::Approx(ew).margin(1e-3));
    }
    // A scale drag of 100 px (times e^0.5) lands on quarters.
    ok(s.idle_frame());
    Json sq = ok(s.command("ui.query", Json{{"name", "gizmo:scale"}}));
    REQUIRE(sq.size() == 1);
    ok(s.command("ui.drag", Json{{"id", sq[0]["id"]}, {"dx", 100}, {"dy", 0}, {"steps", 5}}));
    t = transform();
    INFO(t.dump());
    for (const char* a : {"x", "y", "z"}) {
        const double v = before["scale"][a].get<double>() * std::exp(0.5);
        REQUIRE(t["scale"][a].get<double>() == Catch::Approx(std::max(0.25, std::round(v / 0.25) * 0.25)).margin(1e-4));
    }
    // The setting is kept with the layout; off again, a drag is free.
    Json saved = ok(s.command("project.read", Json{{"path", ".pocket/editor.json"}}));
    REQUIRE(Json::parse(saved["text"].get<std::string>())["snap"] == true);
    ok(s.command("ui.click", Json{{"id", find_named(s, "snap")}}));
    ok(s.idle_frame());
    xq = ok(s.command("ui.query", Json{{"name", "gizmo:x"}}));
    cq = ok(s.command("ui.query", Json{{"name", "gizmo:plane"}}));
    const double dx2 = xq[0]["rect"]["x"].get<double>() - cq[0]["rect"]["x"].get<double>(), dy2 = xq[0]["rect"]["y"].get<double>() - cq[0]["rect"]["y"].get<double>();
    const double snapped_x = t["position"]["x"].get<double>();
    ok(s.command("ui.drag", Json{{"id", xq[0]["id"]}, {"dx", dx2 * 0.37}, {"dy", dy2 * 0.37}, {"steps", 5}}));
    t = transform();
    INFO(t.dump());
    REQUIRE(t["position"]["x"].get<double>() > snapped_x + 0.01);
    REQUIRE_FALSE(multiple_of(t["position"]["x"].get<double>(), 0.5));
    REQUIRE(Json::parse(ok(s.command("project.read", Json{{"path", ".pocket/editor.json"}}))["text"].get<std::string>())["snap"] == false);
    // Four undo steps put everything back.
    for (int i = 0; i < 4; ++i) ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    t = transform();
    REQUIRE(t["position"]["x"].get<double>() == Catch::Approx(before["position"]["x"].get<double>()).margin(1e-5));
    REQUIRE(t["scale"] == before["scale"]);
    REQUIRE(t["rotation"]["y"].get<double>() == Catch::Approx(before["rotation"]["y"].get<double>()).margin(1e-5));
    ok(s.finish());
    std::filesystem::remove_all(root() / "samples" / "physics" / ".pocket");   // the layout file the toggle wrote
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

TEST_CASE("editor paints tiles in the scene pane and undoes the stroke", "[editor]") {
    app::Session s(editor_options("sprites"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    // The sprites sample builds its world in its script: Play, let it settle, Pause, then edit.
    ok(s.command("ui.click", Json{{"id", find_named(s, "play")}}));
    for (int i = 0; i < 10; ++i) ok(s.frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "pause")}}));
    REQUIRE(s.paused());
    // The hierarchy refreshes every 15 frames; wait for the spawned Level to show up.
    for (int i = 0; i < 40 && ok(s.command("ui.query", Json{{"name", "entity:Level"}})).empty(); ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Level")}}));
    ok(s.idle_frame());
    // Picking the ground layer and the ground tile (gid 1) turns the brush on.
    ok(s.command("ui.click", Json{{"id", find_named(s, "layer:ground")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "tile:1")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.snapshot", Json{{"depth", 12}}))["text"].get<std::string>().find("Stop painting") != std::string::npos);
    auto gid_at = [&](int x, int y) { return ok(s.command("tilemap.tile", Json{{"entity", "Level"}, {"tile_x", x}, {"tile_y", y}, {"layer", "ground"}}))["layers"][0]["gid"].get<int>(); };
    REQUIRE(gid_at(10, 5) == 0);
    // A click in the sky where world (0.5, -1) projects paints cell (10, 5); undo clears it.
    Json pr = ok(s.command("render.project", Json{{"point", {{"x", 0.5}, {"y", -1.0}, {"z", 0.0}}}}));
    REQUIRE(pr["visible"] == true);
    Json root = ok(s.command("ui.describe", Json{{"id", 1}}));
    double scale = root["rect"]["w"].get<double>() > 0 ? 1024.0 / root["rect"]["w"].get<double>() : 1.0;
    double px = pr["x"].get<double>() / scale, py = pr["y"].get<double>() / scale;
    ok(s.command("ui.click", Json{{"x", px}, {"y", py}}));
    REQUIRE(gid_at(10, 5) == 1);
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(gid_at(10, 5) == 0);
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "redo")}}));
    REQUIRE(gid_at(10, 5) == 1);
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(gid_at(10, 5) == 0);
    // A drag paints every cell it crosses as one stroke: one undo step takes all of them back.
    ok(s.idle_frame());
    ok(s.command("ui.drag", Json{{"x", px}, {"y", py}, {"dx", 70.0 / scale}, {"dy", 0}, {"steps", 6}}));
    REQUIRE(gid_at(10, 5) == 1);
    REQUIRE(gid_at(11, 5) == 1);
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "undo")}}));
    REQUIRE(gid_at(10, 5) == 0);
    REQUIRE(gid_at(11, 5) == 0);
    // Stop painting: the same click selects again instead of painting.
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "paint")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"x", px}, {"y", py}}));
    REQUIRE(gid_at(10, 5) == 0);
    ok(s.finish());
}

TEST_CASE("inspector steps a named code through its names", "[editor][names]") {
    app::Session s(editor_options("assets"));
    ok(s.start());
    s.set_paused(true);
    // An entity of its own with a light, so its few fields sit at the top of the inspector.
    ok(s.command("world.spawn", Json{{"name", "Aa"}, {"components", Json{{"Transform", Json::object()}, {"Light", Json{{"kind", "point"}}}}}}));
    for (int i = 0; i < 20; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Aa")}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 18}, {"max_nodes", 3000}}))["text"].get<std::string>();
    INFO(snap);
    REQUIRE(snap.find("point") != std::string::npos);   // the kind shown by its name
    // The light's rows are at the foot of the pane: scroll it down, then step the kind on.
    ok(s.command("ui.wheel", Json{{"id", find_named(s, "inspector")}, {"dy", -6}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "Light.kind:next")}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    REQUIRE(ok(s.command("world.get", Json{{"entity", "Aa"}, {"component", "Light"}}))["kind"] == 2);   // point -> spot
}

TEST_CASE("inspector puts a pattern on a mesh: its texture, normal map and tile in one edit", "[editor][pattern]") {
    app::Session s(editor_options("assets"));
    ok(s.start());
    s.set_paused(true);
    ok(s.command("world.spawn", Json{{"name", "Aa"}, {"components", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", "cube"}}}}}}));
    for (int i = 0; i < 20; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Aa")}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    // The mesh's rows run past the pane: scroll down to the texture's.
    ok(s.command("ui.wheel", Json{{"id", find_named(s, "inspector")}, {"dy", -6}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "MeshRenderer.pattern:next")}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    Json mr = ok(s.command("world.get", Json{{"entity", "Aa"}, {"component", "MeshRenderer"}}));
    INFO(mr.dump());
    REQUIRE(mr["texture"] == "pattern:bricks");
    REQUIRE(mr["normal_map"] == "pattern:bricks?map=normal");
    REQUIRE(mr["texture_tile"].get<double>() == Catch::Approx(2));
    // One edit: undone, the mesh is as it was.
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    REQUIRE(ok(s.command("world.get", Json{{"entity", "Aa"}, {"component", "MeshRenderer"}}))["texture"] == "");
}

TEST_CASE("inspector shows list fields as JSON and takes them back", "[editor][layers]") {
    app::Session s(editor_options("assets"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 20; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Arm")}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 16}, {"max_nodes", 2000}}))["text"].get<std::string>();
    INFO(snap);
    REQUIRE(snap.find("layers (0)") != std::string::npos);
    // A layer added through the command shows up in the field once the inspector refreshes.
    ok(s.command("animation.layer", Json{{"entity", "Arm"}, {"clip", "nod"}, {"mask", "root"}}));
    for (int i = 0; i < 20; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Arm")}}));
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    snap = ok(s.command("ui.snapshot", Json{{"depth", 16}, {"max_nodes", 2000}}))["text"].get<std::string>();
    INFO(snap);
    REQUIRE(snap.find("layers (1)") != std::string::npos);
    REQUIRE(snap.find("\"clip\":\"nod\"") != std::string::npos);
}

TEST_CASE("editor opens a project script in a text area and saves it", "[editor][script]") {
    // A scratch script beside the sample's, removed after the test.
    const std::filesystem::path file = root() / "samples" / "physics" / "scripts" / "note-test.ts";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    { std::ofstream out(file); out << "export const note = 1;\n"; }
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "tab:script")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.query", Json{{"name", "script:scripts/main.ts"}})).size() == 1);
    Json row = ok(s.command("ui.query", Json{{"name", "script:scripts/note-test.ts"}}));
    REQUIRE(row.size() == 1);
    ok(s.command("ui.click", Json{{"id", row[0]["id"]}}));
    ok(s.idle_frame());
    Json area = ok(s.command("ui.query", Json{{"name", "script:text"}}));
    REQUIRE(area.size() == 1);
    const int area_id = area[0]["id"].get<int>();
    Json d = ok(s.command("ui.describe", Json{{"id", area_id}}));
    REQUIRE(d["value"] == "export const note = 1;\n");
    REQUIRE(d["multiline"] == true);
    REQUIRE(d["rect"]["h"].get<double>() > 60);   // it fills the panel, not one line
    // Typing at the end adds a line; the header shows the file modified; Save writes it.
    ok(s.command("ui.focus", Json{{"id", area_id}}));
    ok(s.command("ui.type", Json{{"text", "export const two = 2;"}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.describe", Json{{"id", area_id}}))["value"] == "export const note = 1;\nexport const two = 2;");
    {
        std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 12}}))["text"].get<std::string>();
        INFO(snap);
        REQUIRE(snap.find("note-test.ts (modified)") != std::string::npos);
    }
    ok(s.command("ui.click", Json{{"id", find_named(s, "script:save")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("project.read", Json{{"path", "scripts/note-test.ts"}}))["text"] == "export const note = 1;\nexport const two = 2;");
    {
        std::string snap = ok(s.command("ui.snapshot", Json{{"depth", 12}}))["text"].get<std::string>();
        REQUIRE(snap.find("(modified)") == std::string::npos);
    }
    // Return inside the area is a new line; Cmd+Return saves without leaving it.
    ok(s.command("ui.focus", Json{{"id", area_id}}));
    ok(s.command("ui.key", Json{{"key", "Return"}}));
    ok(s.command("ui.type", Json{{"text", "// three"}}));
    ok(s.command("ui.key", Json{{"key", "Return"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("project.read", Json{{"path", "scripts/note-test.ts"}}))["text"] == "export const note = 1;\nexport const two = 2;\n// three");
    REQUIRE(ok(s.command("ui.describe", Json{{"id", area_id}}))["caret"] == 53);
    ok(s.finish());
}

TEST_CASE("editor's Local toggle moves and turns along the entity's own axes", "[editor][local]") {
    // The layout file carries the toggle; start and end without one so other tests see the defaults.
    const std::filesystem::path layout_file = root() / "samples" / "physics" / ".pocket" / "editor.json";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{layout_file};
    std::filesystem::remove(layout_file);
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Ramp")}}));
    ok(s.idle_frame());
    ok(s.idle_frame());
    // The ramp is rolled about Z, so its own X points right and, with the roll's sign, up or down.
    Json before = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(std::abs(before["rotation"]["z"].get<double>()) > 0.1);
    const double roll = before["rotation"]["z"].get<double>() > 0 ? 1.0 : -1.0;
    ok(s.command("ui.click", Json{{"id", find_named(s, "axes")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.query", Json{{"text", "Local"}})).size() >= 1);   // the view bar's toggle reads Local
    auto drag_x = [&]() {
        Json cq = ok(s.command("ui.query", Json{{"name", "gizmo:plane"}}));
        Json xq = ok(s.command("ui.query", Json{{"name", "gizmo:x"}}));
        REQUIRE(cq.size() == 1);
        REQUIRE(xq.size() == 1);
        const double dx = xq[0]["rect"]["x"].get<double>() - cq[0]["rect"]["x"].get<double>(), dy = xq[0]["rect"]["y"].get<double>() - cq[0]["rect"]["y"].get<double>();
        ok(s.command("ui.drag", Json{{"id", xq[0]["id"]}, {"dx", dx}, {"dy", dy}, {"steps", 5}}));
        ok(s.idle_frame());
    };
    drag_x();
    Json local = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    INFO(before.dump() << " -> " << local.dump());
    REQUIRE(local["position"]["x"].get<double>() > before["position"]["x"].get<double>() + 0.1);
    REQUIRE((local["position"]["y"].get<double>() - before["position"]["y"].get<double>()) * roll > 0.05);   // along the rolled axis, off the level too
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    // A half turn about the ramp's own X: the roll's weight lands in +y; about the world's X it lands in -y.
    auto drag_rx = [&]() {
        Json rq = ok(s.command("ui.query", Json{{"name", "gizmo:rotate_x"}}));
        REQUIRE(rq.size() == 1);
        ok(s.command("ui.drag", Json{{"id", rq[0]["id"]}, {"dx", 314}, {"dy", 0}, {"steps", 5}}));
        ok(s.idle_frame());
    };
    drag_rx();
    Json own = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    INFO(own.dump());
    REQUIRE(std::abs(own["rotation"]["x"].get<double>()) > 0.9);
    REQUIRE(own["rotation"]["y"].get<double>() * before["rotation"]["z"].get<double>() > 0.05);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    // World again: the same drags move along world X only and turn about world X.
    ok(s.command("ui.click", Json{{"id", find_named(s, "axes")}}));
    ok(s.idle_frame());
    drag_x();
    Json world_move = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    REQUIRE(world_move["position"]["x"].get<double>() > before["position"]["x"].get<double>() + 0.1);
    REQUIRE(world_move["position"]["y"].get<double>() == Catch::Approx(before["position"]["y"].get<double>()).margin(1e-4));
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    drag_rx();
    Json world_turn = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "Transform"}}));
    INFO(world_turn.dump());
    REQUIRE(world_turn["rotation"]["y"].get<double>() * before["rotation"]["z"].get<double>() < -0.05);
    // The choice is kept with the layout.
    ok(s.command("ui.click", Json{{"id", find_named(s, "axes")}}));
    ok(s.idle_frame());
    Json saved = Json::parse(ok(s.command("project.read", Json{{"path", ".pocket/editor.json"}}))["text"].get<std::string>());
    REQUIRE(saved["local"] == true);
    ok(s.command("ui.click", Json{{"id", find_named(s, "axes")}}));
    ok(s.idle_frame());
    ok(s.finish());
}

TEST_CASE("editor gizmo moves and turns a child exactly under a turned, scaled parent", "[editor][parent]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    // An arm where the ramp is (in view), turned a quarter about Y and doubled, with a pawn at its
    // origin; named to sort to the top of the hierarchy, where its rows are in view.
    const Json ramp = ok(s.command("world.get", Json{{"entity", "Ramp"}, {"component", "WorldTransform"}}));
    const double h = std::sqrt(0.5);
    Json rig = ok(s.command("world.spawn", Json{{"name", "Arm"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", ramp["position"]["x"]}, {"y", ramp["position"]["y"].get<double>() + 1.0}, {"z", ramp["position"]["z"]}}}, {"rotation", Json{{"x", 0}, {"y", h}, {"z", 0}, {"w", h}}}, {"scale", Json{{"x", 2}, {"y", 2}, {"z", 2}}}}}}}}));
    Json pawn = ok(s.command("world.spawn", Json{{"name", "Pawn"}, {"parent", rig["id"]}, {"components", Json{{"Transform", Json::object()}}}}));
    ok(s.command("world.update_transforms", Json::object()));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Pawn")}}));
    ok(s.idle_frame());
    ok(s.idle_frame());
    const Json before = ok(s.command("world.get", Json{{"entity", pawn["id"]}, {"component", "WorldTransform"}}));
    // The X handle pulled by its own length: the pawn goes that far along world X and nowhere else,
    // which in the arm's frame is a move along its local Z of half the length.
    Json cq = ok(s.command("ui.query", Json{{"name", "gizmo:plane"}}));
    Json xq = ok(s.command("ui.query", Json{{"name", "gizmo:x"}}));
    INFO("pawn " << before.dump() << " projected " << s.command("render.project", Json{{"point", before["position"]}}).value_or(Json()).dump());
    REQUIRE(cq.size() == 1);
    REQUIRE(xq.size() == 1);
    const double dx = xq[0]["rect"]["x"].get<double>() - cq[0]["rect"]["x"].get<double>(), dy = xq[0]["rect"]["y"].get<double>() - cq[0]["rect"]["y"].get<double>();
    ok(s.command("ui.drag", Json{{"id", xq[0]["id"]}, {"dx", dx}, {"dy", dy}, {"steps", 5}}));
    ok(s.idle_frame());
    const Json after = ok(s.command("world.get", Json{{"entity", pawn["id"]}, {"component", "WorldTransform"}}));
    const Json local = ok(s.command("world.get", Json{{"entity", pawn["id"]}, {"component", "Transform"}}));
    INFO(before.dump() << " -> " << after.dump() << " local " << local.dump());
    const double moved = after["position"]["x"].get<double>() - before["position"]["x"].get<double>();
    REQUIRE(moved > 0.1);
    REQUIRE(after["position"]["y"].get<double>() == Catch::Approx(before["position"]["y"].get<double>()).margin(1e-3));
    REQUIRE(after["position"]["z"].get<double>() == Catch::Approx(before["position"]["z"].get<double>()).margin(1e-3));
    REQUIRE(local["position"]["x"].get<double>() == Catch::Approx(0.0).margin(1e-3));
    REQUIRE(local["position"]["z"].get<double>() == Catch::Approx(moved / 2).margin(1e-3));
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    // A half turn about world X: the pawn's world rotation is that turn after the arm's, whose
    // x and z parts then share a sign (turning about the arm's own X would give them opposite signs).
    Json rq = ok(s.command("ui.query", Json{{"name", "gizmo:rotate_x"}}));
    REQUIRE(rq.size() == 1);
    ok(s.command("ui.drag", Json{{"id", rq[0]["id"]}, {"dx", 314}, {"dy", 0}, {"steps", 5}}));
    ok(s.idle_frame());
    const Json turned = ok(s.command("world.get", Json{{"entity", pawn["id"]}, {"component", "WorldTransform"}}));
    INFO(turned.dump());
    REQUIRE(turned["rotation"]["x"].get<double>() * turned["rotation"]["z"].get<double>() > 0.4);
    REQUIRE(std::abs(turned["rotation"]["y"].get<double>()) < 0.1);
    ok(s.finish());
}

TEST_CASE("editor places a model with a light as its node tree, from the Place button, and reimports it", "[editor][assets][import]") {
    // A glTF lamp: a shade (one triangle) and a point light two units above it.
    const std::filesystem::path lamp = root() / "samples" / "physics" / "assets" / "lamp-test.gltf";
    std::ofstream(lamp) << R"({"asset": {"version": "2.0"}, "extensionsUsed": ["KHR_lights_punctual"],
        "extensions": {"KHR_lights_punctual": {"lights": [{"type": "point", "color": [1, 0.8, 0.6], "intensity": 100}]}},
        "scene": 0, "scenes": [{"nodes": [0, 1]}],
        "nodes": [{"name": "Shade", "mesh": 0}, {"name": "Bulb", "translation": [0, 2, 0], "extensions": {"KHR_lights_punctual": {"light": 0}}}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]}],
        "bufferViews": [{"buffer": 0, "byteLength": 36}],
        "buffers": [{"byteLength": 36, "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"}]})";
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "tab:assets")}}));
    ok(s.idle_frame());
    // Picked, it says what it carries, and Place and Reimport appear.
    ok(s.command("ui.click", Json{{"id", find_named(s, "asset:assets/lamp-test.gltf")}}));
    ok(s.idle_frame());
    {
        const std::string info = ok(s.command("ui.describe", Json{{"id", find_named(s, "asset-info")}})).value("text", "");
        INFO(info);
        REQUIRE(info.find(", 1 light") != std::string::npos);
    }
    ok(s.command("ui.click", Json{{"id", find_named(s, "asset:place")}}));
    ok(s.idle_frame());
    INFO(ok(s.command("ui.describe", Json{{"id", find_named(s, "notice")}})).dump());
    const Json root_id = ok(s.command("world.find", Json{{"path", "lamp-test"}}));
    REQUIRE(root_id.is_number());
    const Json bulb = ok(s.command("world.find", Json{{"path", "lamp-test/Bulb"}}));
    REQUIRE(bulb.is_number());
    REQUIRE(ok(s.command("world.get", Json{{"entity", bulb}, {"component", "Light"}}))["kind"] == 1);
    REQUIRE(ok(s.command("world.get", Json{{"entity", "lamp-test/Shade"}, {"component", "MeshRenderer"}}))["mesh"] == "assets/lamp-test.gltf");
    // One edit: undo takes the whole tree away, redo brings it back with its light.
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    REQUIRE_FALSE(ok(s.command("world.find", Json{{"path", "lamp-test"}})).is_number());
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta", "shift"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("world.get", Json{{"entity", "lamp-test/Bulb"}, {"component", "Light"}}))["kind"] == 1);
    // A model without lights or cameras stays one drawable.
    ok(s.command("ui.click", Json{{"id", find_named(s, "asset:assets/bowl.glb")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "asset:place")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("world.get", Json{{"entity", "bowl"}, {"component", "MeshRenderer"}}))["mesh"] == "assets/bowl.glb");
    REQUIRE(ok(s.command("world.children", Json{{"entity", "bowl"}})).empty());
    // Reimport reads the file again.
    ok(s.command("ui.click", Json{{"id", find_named(s, "asset:reimport")}}));
    ok(s.idle_frame());
    {
        const std::string notice = ok(s.command("ui.describe", Json{{"id", find_named(s, "notice")}})).dump();
        INFO(notice);
        REQUIRE(notice.find("imported again") != std::string::npos);
    }
    // A built-in mesh, listed with the files, placed the same way.
    ok(s.command("ui.click", Json{{"id", find_named(s, "asset:tree")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "asset:place")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("world.get", Json{{"entity", "tree"}, {"component", "MeshRenderer"}}))["mesh"] == "tree");
    ok(s.finish());
    std::filesystem::remove(lamp);
    std::filesystem::remove_all(root() / "samples" / "physics" / ".pocket");   // the layout file the tab click wrote
}

TEST_CASE("editor's Audio tab sets buses live and saves them to audio.json for the next run", "[editor][audio]") {
    const std::filesystem::path saved = root() / "samples" / "audio" / "audio.json";
    std::filesystem::remove(saved);
    {
        app::Session s(editor_options("audio"));
        ok(s.start());
        s.set_paused(true);
        for (int i = 0; i < 2; ++i) ok(s.idle_frame());
        ok(s.command("ui.click", Json{{"id", find_named(s, "tab:audio")}}));
        ok(s.idle_frame());
        // The project's ambience bus is listed with its ducking; a click on its slider sets its volume.
        Json row = ok(s.command("ui.query", Json{{"name", "bus:ambience:volume"}}));
        REQUIRE(row.size() == 1);
        const Json rect = row[0]["rect"];
        const double x = rect["x"].get<double>() + 7 + (rect["w"].get<double>() - 14) * 0.25;   // a quarter of 0..2
        ok(s.command("ui.click", Json{{"x", x}, {"y", rect["y"].get<double>() + rect["h"].get<double>() / 2}}));
        ok(s.idle_frame());
        auto bus = [&](const char* name) {
            for (const Json& b : ok(s.command("audio.buses", Json::object()))) if (b["name"] == name) return b;
            FAIL("no bus " << name);
            return Json();
        };
        REQUIRE(bus("ambience")["volume"].get<double>() == Catch::Approx(0.5));
        REQUIRE(bus("ambience")["duck_by"] == "fx");
        ok(s.command("ui.click", Json{{"id", find_named(s, "bus:ambience:muted")}}));
        ok(s.idle_frame());
        REQUIRE(bus("ambience")["muted"] == true);
        ok(s.command("ui.click", Json{{"id", find_named(s, "bus:ambience:muted")}}));
        ok(s.command("ui.click", Json{{"id", find_named(s, "mixer:save")}}));
        ok(s.idle_frame());
        REQUIRE(std::filesystem::exists(saved));
        Json file = Json::parse(ok(s.command("project.read", Json{{"path", "audio.json"}}))["text"].get<std::string>());
        INFO(file.dump());
        REQUIRE(file["buses"]["ambience"]["volume"].get<double>() == Catch::Approx(0.5));
        REQUIRE(file["buses"]["ambience"]["muted"] == false);
        REQUIRE(file["buses"]["ambience"]["duck_by"] == "fx");
        ok(s.finish());
    }
    // The saved mixer applies over project.toml's buses in the next run.
    {
        app::Session s(editor_options("audio"));
        ok(s.start());
        bool found = false;
        for (const Json& b : ok(s.command("audio.buses", Json::object()))) {
            if (b["name"] != "ambience") continue;
            found = true;
            REQUIRE(b["volume"].get<double>() == Catch::Approx(0.5));
            REQUIRE(b["duck_amount"].get<double>() == Catch::Approx(0.5));
        }
        REQUIRE(found);
        ok(s.finish());
    }
    std::filesystem::remove(saved);
    std::filesystem::remove_all(root() / "samples" / "audio" / ".pocket");
}

TEST_CASE("editor's Script tab lists the type errors a watched rebundle found", "[editor][script][types]") {
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    // What `pocket editor --watch` sends after checking a rebundle.
    REQUIRE(ok(s.command("script.diagnostics", Json::object()))["count"] == 0);
    const Json found = Json::array({Json{{"file", "scripts/main.ts"}, {"line", 19}, {"column", 50}, {"severity", "error"}, {"message", "TS2561: 'positon' does not exist in type 'DeepPartial<Transform>'. Did you mean to write 'position'?"}}});
    REQUIRE(ok(s.command("script.diagnostics", Json{{"diagnostics", found}}))["count"] == 1);
    REQUIRE(ok(s.command("script.diagnostics", Json::object()))["diagnostics"] == found);
    REQUIRE_FALSE(s.command("script.diagnostics", Json{{"diagnostics", "not a list"}}).has_value());
    // The Console has it as a warning under "types".
    bool logged = false;
    for (const Json& l : ok(s.command("log.tail", Json{{"n", 20}}))) if (l.value("cat", "") == "types" && l.value("msg", "").find("scripts/main.ts:19:50") != std::string::npos) logged = true;
    REQUIRE(logged);
    // The Script tab lists it under the text area, with its place.
    ok(s.command("ui.click", Json{{"id", find_named(s, "tab:script")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "script:scripts/main.ts")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.query", Json{{"name", "script:errors"}})).size() == 1);
    REQUIRE(ok(s.command("ui.query", Json{{"text", "scripts/main.ts:19:50  TS2561"}})).size() == 1);
    // Cleared by the next clean check.
    ok(s.command("script.diagnostics", Json{{"diagnostics", Json::array()}}));
    for (int i = 0; i < 21; ++i) ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.query", Json{{"name", "script:errors"}})).empty());
    ok(s.finish());
    std::filesystem::remove_all(root() / "samples" / "physics" / ".pocket");
}

TEST_CASE("editor sculpts a terrain in the scene pane and undoes the stroke", "[editor][terrain]") {
    app::Session s(editor_options("hills"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 3; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:Hills")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.query", Json{{"name", "terrain-brush"}})).size() == 1);
    ok(s.command("ui.click", Json{{"id", find_named(s, "sculpt")}}));
    ok(s.idle_frame());
    const Json before = ok(s.command("terrain.heights", Json::object()))["heights"];
    const int revision = ok(s.command("terrain.info", Json::object()))["revision"].get<int>();
    // A drag across the middle of the scene pane raises the ground under it.
    Json pane = ok(s.command("ui.query", Json{{"name", "viewport"}}));
    REQUIRE(pane.size() == 1);
    const Json r = pane[0]["rect"];
    const double cx = r["x"].get<double>() + r["w"].get<double>() * 0.5, cy = r["y"].get<double>() + r["h"].get<double>() * 0.6;
    ok(s.command("ui.drag", Json{{"x", cx}, {"y", cy}, {"dx", 40}, {"dy", 0}, {"steps", 4}}));
    ok(s.idle_frame());
    Json info = ok(s.command("terrain.info", Json::object()));
    INFO(info.dump());
    REQUIRE(info["edited"] == true);
    REQUIRE(info["revision"].get<int>() > revision);
    const Json after = ok(s.command("terrain.heights", Json::object()))["heights"];
    double raised = 0;
    for (std::size_t k = 0; k < after.size(); ++k) raised += after[k].get<double>() - before[k].get<double>();
    REQUIRE(raised > 1.0);
    // Undo puts every height back; redo raises them again.
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("terrain.heights", Json::object()))["heights"] == before);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta", "shift"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("terrain.heights", Json::object()))["heights"] == after);
    // Lower mode takes ground away.
    ok(s.command("ui.click", Json{{"id", find_named(s, "sculpt:lower")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"x", cx}, {"y", cy}}));
    ok(s.idle_frame());
    const Json lowered = ok(s.command("terrain.heights", Json::object()))["heights"];
    double sum_after = 0, sum_lowered = 0;
    for (std::size_t k = 0; k < after.size(); ++k) { sum_after += after[k].get<double>(); sum_lowered += lowered[k].get<double>(); }
    REQUIRE(sum_lowered < sum_after);
    // Paint mode lays the chosen colour (sand) along a drag; undo takes it away, redo lays it again.
    ok(s.command("terrain.paints", Json{{"paint", Json::array()}}));   // the sample's own path cleared first
    ok(s.command("ui.click", Json{{"id", find_named(s, "sculpt:paint")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "paint:sand")}}));
    ok(s.idle_frame());
    ok(s.command("ui.drag", Json{{"x", cx}, {"y", cy}, {"dx", -40}, {"dy", 0}, {"steps", 4}}));
    ok(s.idle_frame());
    info = ok(s.command("terrain.info", Json::object()));
    INFO(info.dump());
    REQUIRE(info["painted"].get<double>() > 0);
    const Json paint = ok(s.command("terrain.paints", Json::object()))["paint"];
    double best = 0, red = 0;
    for (std::size_t k = 3; k < paint.size(); k += 4) if (paint[k].get<double>() > best) { best = paint[k].get<double>(); red = paint[k - 3].get<double>(); }
    REQUIRE(best > 0.3);
    REQUIRE(red == Catch::Approx(0.82).margin(0.02));
    REQUIRE(ok(s.command("ui.query", Json{{"name", "terrain:save-paint"}})).size() == 1);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("terrain.info", Json::object()))["painted"].get<double>() == 0);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta", "shift"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("terrain.paints", Json::object()))["paint"] == paint);
    // With a textured layer chosen (the sample's terrain has four), the brush lays that layer; undo
    // takes it away and leaves the sample's own dirt path.
    const Json layers_before = ok(s.command("terrain.paints", Json{{"layers", true}}))["paint"];
    ok(s.command("ui.click", Json{{"id", find_named(s, "paint-layer:rock")}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("ui.query", Json{{"name", "paint:sand"}})).empty());   // no colours while a layer is chosen
    ok(s.command("ui.drag", Json{{"x", cx}, {"y", cy}, {"dx", 40}, {"dy", 0}, {"steps", 4}}));
    ok(s.idle_frame());
    const Json layered = ok(s.command("terrain.paints", Json{{"layers", true}}))["paint"];
    double rock = 0;
    for (std::size_t k = 2; k < layered.size(); k += 4) rock = std::max(rock, layered[k].get<double>());
    REQUIRE(rock > 0.3);
    REQUIRE(ok(s.command("ui.query", Json{{"name", "terrain:save-layers"}})).size() == 1);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    ok(s.idle_frame());
    REQUIRE(ok(s.command("terrain.paints", Json{{"layers", true}}))["paint"] == layers_before);
    ok(s.finish());
    std::filesystem::remove_all(root() / "samples" / "hills" / ".pocket");
}

TEST_CASE("editor docks: a tab dragged to another dock moves its pane there, an emptied dock gives the scene its room, and the layout comes back", "[editor][docks]") {
    std::filesystem::path saved = root() / "samples" / "physics" / ".pocket" / "editor.json";
    std::filesystem::remove(saved);
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{saved};
    auto rect = [](app::Session& s, const char* name) {
        Json q = ok(s.command("ui.query", Json{{"name", name}}));
        INFO("looking for " << name << ": " << q.dump());
        REQUIRE(q.size() == 1);
        return q[0]["rect"];
    };
    auto drag_onto = [&](app::Session& s, const char* tab, const Json& target) {
        const Json from = rect(s, tab);
        const double dx = target["x"].get<double>() + target["w"].get<double>() / 2 - (from["x"].get<double>() + from["w"].get<double>() / 2);
        const double dy = target["y"].get<double>() + target["h"].get<double>() / 2 - (from["y"].get<double>() + from["h"].get<double>() / 2);
        ok(s.command("ui.drag", Json{{"id", find_named(s, tab)}, {"dx", dx}, {"dy", dy}}));
        for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    };
    {
        app::Session s(editor_options("physics"));
        ok(s.start());
        s.set_paused(true);
        for (int i = 0; i < 2; ++i) ok(s.idle_frame());
        // The default: the hierarchy on the left, the inspector on the right, the tabs at the bottom.
        REQUIRE(rect(s, "tab:audio")["y"].get<double>() > rect(s, "viewport")["y"].get<double>() + rect(s, "viewport")["h"].get<double>());
        const double scene_w = rect(s, "viewport")["w"].get<double>();
        // Audio onto the left dock: its tab and pane there, in front; the hierarchy a tab beside it.
        drag_onto(s, "tab:audio", rect(s, "hierarchy"));
        REQUIRE(rect(s, "tab:audio")["x"].get<double>() < 240);
        REQUIRE(rect(s, "tab:audio")["y"].get<double>() < rect(s, "viewport")["y"].get<double>() + 40);
        REQUIRE(rect(s, "pane:audio")["x"].get<double>() < 240);
        REQUIRE(ok(s.command("ui.query", Json{{"name", "hierarchy"}})).empty());
        ok(s.command("ui.click", Json{{"id", find_named(s, "tab:hierarchy")}}));
        ok(s.idle_frame());
        REQUIRE(rect(s, "hierarchy")["x"].get<double>() < 240);
        REQUIRE(ok(s.command("ui.query", Json{{"name", "pane:audio"}})).empty());
        // The inspector down to the bottom dock: the right dock is empty and gone, the scene wider.
        drag_onto(s, "tab:inspector", rect(s, "bottom"));
        REQUIRE(ok(s.command("ui.query", Json{{"name", "dock:right"}})).empty());
        REQUIRE(rect(s, "viewport")["w"].get<double>() > scene_w + 300);
        REQUIRE(rect(s, "inspector")["y"].get<double>() > rect(s, "viewport")["y"].get<double>() + rect(s, "viewport")["h"].get<double>());
        // A tab dropped in the middle of the scene stays where it was.
        drag_onto(s, "tab:console", rect(s, "viewport"));
        REQUIRE(rect(s, "tab:console")["y"].get<double>() > rect(s, "viewport")["y"].get<double>() + rect(s, "viewport")["h"].get<double>());
        ok(s.finish());
    }
    REQUIRE(std::filesystem::exists(saved));
    {
        // Opened again: the panes where they were left.
        app::Session s(editor_options("physics"));
        ok(s.start());
        s.set_paused(true);
        for (int i = 0; i < 2; ++i) ok(s.idle_frame());
        REQUIRE(rect(s, "tab:audio")["x"].get<double>() < 240);
        REQUIRE(ok(s.command("ui.query", Json{{"name", "dock:right"}})).empty());
        REQUIRE(rect(s, "tab:inspector")["y"].get<double>() > rect(s, "viewport")["y"].get<double>() + rect(s, "viewport")["h"].get<double>());
        ok(s.finish());
    }
}

TEST_CASE("editor timeline pane: tracks and keys on a ruler, a playhead that scrubs, keys added, deleted and undone, a new track", "[editor][timeline]") {
    const std::filesystem::path file = root() / "samples" / "physics" / "timelines" / "test-door.json";
    std::filesystem::remove(file);
    app::Session s(editor_options("physics"));
    ok(s.start());
    s.set_paused(true);
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("project.write", Json{{"path", "timelines/test-door.json"}, {"json", Json{{"duration", 2}, {"tracks", Json::array({Json{{"component", "Transform"}, {"field", "position.y"}, {"keys", Json::array({Json::array({0, 0}), Json::array({2, 3})})}}})}}}}));
    Json door = Json::object();
    door["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 0}}}};
    door["MeshRenderer"] = Json{{"mesh", "cube"}};
    door["Timeline"] = Json{{"path", "timelines/test-door.json"}, {"playing", false}};
    ok(s.command("world.spawn", Json{{"name", "ADoor"}, {"components", door}}));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "entity:ADoor")}}));
    ok(s.command("ui.click", Json{{"id", find_named(s, "tab:timeline")}}));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    // One track with its two keys on the ruler.
    REQUIRE(ok(s.command("ui.query", Json{{"name", "tl:track:0"}})).size() == 1);
    REQUIRE(ok(s.command("ui.query", Json{{"name", "tl:key:0:0"}})).size() == 1);
    REQUIRE(ok(s.command("ui.query", Json{{"name", "tl:key:0:1"}})).size() == 1);
    // The playhead dragged to the middle puts the door halfway up, with no tick.
    const Json slider = ok(s.command("ui.query", Json{{"name", "tl:time"}}))[0]["rect"];
    ok(s.command("ui.click", Json{{"x", slider["x"].get<double>() + slider["w"].get<double>() * 0.5}, {"y", slider["y"].get<double>() + slider["h"].get<double>() * 0.5}}));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    const double t = ok(s.command("world.get", Json{{"entity", "ADoor"}, {"component", "Timeline"}}))["time"].get<double>();
    INFO("time " << t);
    REQUIRE(t == Catch::Approx(1).margin(0.15));
    REQUIRE(ok(s.command("world.get", Json{{"entity", "ADoor"}, {"component", "Transform"}}))["position"]["y"].get<double>() == Catch::Approx(1.5 * t).margin(0.01));
    // The door raised to 5 and keyed there: three keys, the middle one 5.
    ok(s.command("world.set", Json{{"entity", "ADoor"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"y", 5}}}}}}));
    ok(s.command("ui.click", Json{{"id", find_named(s, "tl:track:0:key")}}));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    auto keys = [&]() { return Json::parse(ok(s.command("project.read", Json{{"path", "timelines/test-door.json"}}))["text"].get<std::string>())["tracks"][0]["keys"]; };
    Json k = keys();
    INFO(k.dump());
    REQUIRE(k.size() == 3);
    REQUIRE(k[1][1].get<double>() == Catch::Approx(5));
    // The last key picked and deleted; undone, it is back.
    ok(s.command("ui.click", Json{{"id", find_named(s, "tl:key:0:2")}}));
    ok(s.idle_frame());
    ok(s.command("ui.click", Json{{"id", find_named(s, "tl:key:delete")}}));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    REQUIRE(keys().size() == 2);
    ok(s.command("ui.key", Json{{"key", "Z"}, {"mods", Json::array({"meta"})}}));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    REQUIRE(keys().size() == 3);
    // A new track on scale.x starts with the value now at the playhead.
    const int field = find_named(s, "tl:new:field");
    ok(s.command("ui.click", Json{{"id", field}}));
    ok(s.command("ui.key", Json{{"key", "End"}}));
    for (int i = 0; i < 12; ++i) ok(s.command("ui.key", Json{{"key", "Backspace"}}));
    ok(s.command("ui.type", Json{{"text", "scale.x"}}));
    ok(s.command("ui.key", Json{{"key", "Return"}}));
    ok(s.command("ui.click", Json{{"id", find_named(s, "tl:new:add")}}));
    for (int i = 0; i < 2; ++i) ok(s.idle_frame());
    const Json doc = Json::parse(ok(s.command("project.read", Json{{"path", "timelines/test-door.json"}}))["text"].get<std::string>());
    INFO(doc.dump());
    REQUIRE(doc["tracks"].size() == 2);
    REQUIRE(doc["tracks"][1]["field"] == "scale.x");
    REQUIRE(doc["tracks"][1]["keys"][0][1].get<double>() == Catch::Approx(1));
    for (int i = 0; i < 25; ++i) ok(s.idle_frame());   // the bottom dock redraws its tab every twenty frames
    REQUIRE(ok(s.command("ui.query", Json{{"name", "tl:track:1"}})).size() == 1);
    ok(s.finish());
    std::filesystem::remove(file);
    std::filesystem::remove_all(root() / "samples" / "physics" / ".pocket");
}
