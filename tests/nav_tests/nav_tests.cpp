#include <pocket/nav/nav.hpp>
#include <pocket/physics/physics.hpp>

#include <catch_amalgamated.hpp>

#include <cmath>
#include <cstdlib>
#include <filesystem>

using namespace pocket;
using namespace pocket::world;

namespace {

std::filesystem::path root() {
    const char* r = std::getenv("POCKET_ROOT");
    REQUIRE(r != nullptr);
    return r;
}

nav::Grid open_grid(int w, int h) {
    nav::Grid g;
    g.width = w;
    g.height = h;
    g.cell = 1.0f;
    g.origin = {0, 0, 0};
    g.plane = 0;
    g.walkable.assign(static_cast<std::size_t>(w * h), 1);
    g.ground.assign(static_cast<std::size_t>(w * h), 0.0f);
    return g;
}

EntityId static_box(World& w, const char* name, Vec3 pos, Vec3 half) {
    return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", half.x}, {"y", half.y}, {"z", half.z}}}}}}).value();
}

}  // namespace

TEST_CASE("A* goes through the gap, never cuts a corner, and says when the goal is out of reach", "[nav]") {
    nav::Grid g = open_grid(10, 10);
    for (int y = 0; y < 10; ++y) if (y != 8) g.walkable[g.index(5, y)] = 0;  // a wall with one gap
    nav::Nav n;
    n.set_grid(g);
    auto raw = n.path({0.5f, 0, 0.5f}, {9.5f, 0, 0.5f}, false).value();
    REQUIRE_FALSE(raw.partial);
    REQUIRE_FALSE(raw.snapped);
    REQUIRE(raw.points.front().x == Catch::Approx(0.5f));
    REQUIRE(raw.points.back().x == Catch::Approx(9.5f));
    for (const Vec3& p : raw.points) {
        int x, y;
        REQUIRE(g.cell_of(p, x, y));
        REQUIRE(g.walkable_at(x, y));
        if (x == 5) REQUIRE(y == 8);
    }
    REQUIRE(raw.length > 16.0f);  // down to the gap and back up
    auto smooth = n.path({0.5f, 0, 0.5f}, {9.5f, 0, 0.5f}, true).value();
    REQUIRE(smooth.points.size() < raw.points.size());
    REQUIRE(smooth.length <= raw.length + 1e-3f);
    REQUIRE(smooth.points.size() >= 3);  // the gap forces at least one corner
    REQUIRE(n.reachable({0.5f, 0, 0.5f}, {9.5f, 0, 0.5f}));
    // Diagonals do not squeeze between two blocked cells.
    nav::Grid c = open_grid(3, 3);
    c.walkable[c.index(1, 0)] = 0;
    c.walkable[c.index(0, 1)] = 0;
    n.set_grid(c);
    auto corner = n.path({0.5f, 0, 0.5f}, {1.5f, 0, 1.5f}, false).value();
    REQUIRE(corner.partial);
    REQUIRE(corner.points.size() == 1);
    // Closing the gap leaves a partial path that ends as close as it can.
    g.walkable[g.index(5, 8)] = 0;
    n.set_grid(g);
    auto part = n.path({0.5f, 0, 0.5f}, {9.5f, 0, 0.5f}, false).value();
    REQUIRE(part.partial);
    REQUIRE(part.points.back().x < 5.0f);
    REQUIRE_FALSE(n.reachable({0.5f, 0, 0.5f}, {9.5f, 0, 0.5f}));
    // Steps above max_step are not crossed; the point of a blocked cell snaps to the nearest walkable one.
    nav::Grid s = open_grid(4, 1);
    s.ground[s.index(2, 0)] = 1.0f;
    s.ground[s.index(3, 0)] = 1.0f;
    s.max_step = 0.4f;
    n.set_grid(s);
    REQUIRE(n.path({0.5f, 0, 0.5f}, {3.5f, 1, 0.5f}, false).value().partial);
    s.ground[s.index(2, 0)] = 0.3f;
    s.ground[s.index(3, 0)] = 0.6f;
    n.set_grid(s);
    REQUIRE_FALSE(n.path({0.5f, 0, 0.5f}, {3.5f, 0.6f, 0.5f}, false).value().partial);
    s.walkable[s.index(3, 0)] = 0;
    n.set_grid(s);
    auto snapped = n.path({0.5f, 0, 0.5f}, {3.5f, 0.6f, 0.5f}, false).value();
    REQUIRE(snapped.snapped);
    REQUIRE(snapped.points.back().x == Catch::Approx(2.5f));
    REQUIRE_FALSE(n.reachable({0.5f, 0, 0.5f}, {3.5f, 0.6f, 0.5f}));
    REQUIRE(n.nearest({3.5f, 0.6f, 0.5f}, 2.0f)->x == Catch::Approx(2.5f));
    REQUIRE_FALSE(n.nearest({30.0f, 0, 0.5f}, 2.0f).has_value());
    REQUIRE(n.path({50, 0, 50}, {0.5f, 0, 0.5f}).error().code == "outside");
    REQUIRE(nav::Nav().path({0, 0, 0}, {1, 0, 1}).error().code == "not_baked");
}

TEST_CASE("a grid baked from colliders walks around a wall, keeps off it, and finds no way up a ledge", "[nav]") {
    World w;
    physics::Physics p;
    static_box(w, "Ground", {0, -0.5f, 0}, {20, 0.5f, 20});
    static_box(w, "Wall", {0, 1, 0}, {0.25f, 1, 3});     // across the middle, z -3..3
    static_box(w, "Ledge", {5, 0.5f, 5}, {1, 0.5f, 1});   // top at 1.0: above max_step
    w.spawn("Goal", 0, Json{{"Transform", {{"position", {{"x", -5}, {"y", 0.5}, {"z", -5}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1}, {"y", 1}, {"z", 1}}}, {"is_trigger", true}}}}).value();
    w.spawn("Crate", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 0.5}, {"z", -5}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}}).value();
    nav::Nav n;
    nav::BakeParams bp;
    bp.min = {-8, -1, -8};
    bp.max = {8, 3, 8};
    bp.cell = 0.5f;
    REQUIRE(n.bake_colliders(w, p, bp, 7).has_value());
    const nav::Grid& g = n.grid();
    REQUIRE(g.width == 32);
    REQUIRE(g.baked_tick == 7);
    REQUIRE(g.walkable_count() > 800);
    auto path = n.path({-4, 0, 0}, {4, 0, 0}).value();
    INFO(path.length);
    REQUIRE_FALSE(path.partial);
    REQUIRE(path.length > 10.0f);  // around the wall's end at z = 3 or -3
    for (const Vec3& pt : path.points) REQUIRE((std::fabs(pt.x) > 0.5f || std::fabs(pt.z) > 3.2f));
    // Cells hugging the wall are not ground the agent can stand on next to it: the agent's radius
    // keeps it off (a cell whose ray lands on the wall's top is walkable up there, two units up).
    int x, y;
    REQUIRE(g.cell_of({0.4f, 0, 0}, x, y));
    REQUIRE((!g.walkable_at(x, y) || g.ground[g.index(x, y)] > 1.5f));
    REQUIRE(g.cell_of({1.4f, 0, 0}, x, y));
    REQUIRE(g.walkable_at(x, y));
    REQUIRE(g.ground[g.index(x, y)] == Catch::Approx(0.0f).margin(0.01f));
    // The ledge top is walkable ground of its own, but no path climbs the step.
    REQUIRE(g.cell_of({5, 1, 5}, x, y));
    REQUIRE(g.walkable_at(x, y));
    REQUIRE(g.ground[g.index(x, y)] == Catch::Approx(1.0f).margin(0.01f));
    REQUIRE_FALSE(n.reachable({-4, 0, 0}, {5, 1, 5}));
    // Triggers and dynamic bodies are not obstacles for the bake.
    REQUIRE(g.cell_of({-5, 0, -5}, x, y));
    REQUIRE(g.walkable_at(x, y));
    REQUIRE(g.cell_of({5, 0, -5}, x, y));
    REQUIRE(g.walkable_at(x, y));
    REQUIRE(g.ground[g.index(x, y)] == Catch::Approx(0.0f).margin(0.01f));
    Json d = n.describe();
    REQUIRE(d["source"] == "colliders");
    REQUIRE(d["plane"] == "xz");
}

TEST_CASE("a platformer grid from the sprites level links the ground, the plank and the ledge by jumps", "[nav]") {
    World w;
    assets::AssetStore store(root() / "samples" / "sprites");
    EntityId level = w.spawn("Level", 0, Json{{"Transform", {{"position", {{"x", -10}, {"y", 4.5}, {"z", 0}}}}}, {"TileMap", {{"map", "assets/level.tmj"}}}}).value();
    nav::Nav n;
    nav::TileBakeParams tp;
    tp.mode = "platformer";
    REQUIRE(n.bake_tilemap(w, store, level, tp, 3).has_value());
    const nav::Grid& g = n.grid();
    REQUIRE(g.plane == 1);
    REQUIRE(g.width == 20);
    REQUIRE(g.walkable_at(10, 7));       // standing on the ground
    REQUIRE(g.walkable_at(13, 5));       // on the ledge
    REQUIRE(g.walkable_at(5, 5));        // on the one-way plank
    REQUIRE_FALSE(g.walkable_at(10, 6)); // air
    REQUIRE_FALSE(g.walkable_at(10, 8)); // inside the ground
    REQUIRE(g.link_count() > 0);
    auto ground_to_ledge = n.path(g.center_of(2, 7), g.center_of(13, 5)).value();
    INFO(ground_to_ledge.length);
    REQUIRE_FALSE(ground_to_ledge.partial);
    REQUIRE(ground_to_ledge.points.size() > 5);
    REQUIRE(g.center_of(2, 7).y == Catch::Approx(-3.0f));
    REQUIRE(n.reachable(g.center_of(2, 7), g.center_of(5, 5)));
    // The same level top-down: every empty cell walks, even the sky.
    tp.mode = "topdown";
    REQUIRE(n.bake_tilemap(w, store, level, tp, 4).has_value());
    REQUIRE(n.grid().walkable_at(10, 2));
    REQUIRE(n.grid().link_count() == 0);
    REQUIRE(n.reachable(n.grid().center_of(0, 0), n.grid().center_of(19, 0)));
    REQUIRE(n.describe()["source"] == "tilemap:assets/level.tmj:topdown");
    REQUIRE(n.bake_tilemap(w, store, level, nav::TileBakeParams{"sideways"}, 5).error().code == "bad_args");
}
