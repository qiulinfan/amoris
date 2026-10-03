#include <pocket/nav/nav.hpp>
#include <pocket/physics/physics.hpp>

#include <fstream>
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
    (void)w.spawn("Goal", 0, Json{{"Transform", {{"position", {{"x", -5}, {"y", 0.5}, {"z", -5}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1}, {"y", 1}, {"z", 1}}}, {"is_trigger", true}}}}).value();
    (void)w.spawn("Crate", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 0.5}, {"z", -5}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}}).value();
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

TEST_CASE("obstacles block the cells under them until they move, and paths and nearest go around", "[nav][obstacles]") {
    nav::Nav n;
    n.set_grid(open_grid(12, 12));
    const Vec3 from{0.5f, 0, 5.5f}, to{11.5f, 0, 5.5f};
    auto direct = n.path(from, to, true).value();
    REQUIRE(direct.points.size() == 2);
    n.set_obstacles({{7, {6.0f, 0, 5.5f}, 1.2f}});
    REQUIRE(n.grid().blocked_count() > 0);
    REQUIRE(n.grid().walkable_count() == 144);   // the level is untouched
    REQUIRE_FALSE(n.grid().walkable_at(5, 5));   // its center is half a cell from the obstacle
    REQUIRE(n.grid().walkable_at(0, 0));
    auto around = n.path(from, to, true).value();
    REQUIRE_FALSE(around.partial);
    REQUIRE(around.points.size() > 2);
    REQUIRE(around.length > direct.length + 0.5f);
    for (const Vec3& p : around.points) REQUIRE(std::hypot(p.x - 6.0f, p.z - 5.5f) > 1.2f);
    auto near = n.nearest({6.0f, 0, 5.5f}, 3.0f);
    REQUIRE(near.has_value());
    REQUIRE(std::hypot(near->x - 6.0f, near->z - 5.5f) > 1.2f);
    REQUIRE_FALSE(n.reachable(from, {6.5f, 0, 5.5f}));
    REQUIRE(n.describe()["blocked"].get<int>() == static_cast<int>(n.grid().blocked_count()));
    REQUIRE(n.describe()["obstacles"].get<int>() == 1);
    // A radius of zero blocks nothing; the obstacle moved north opens the straight run again.
    n.set_obstacles({{7, {6.0f, 0, 5.5f}, 0.0f}});
    REQUIRE(n.grid().blocked_count() == 0);
    n.set_obstacles({{7, {6.0f, 0, 1.5f}, 1.2f}});
    REQUIRE(n.grid().blocked_count() > 0);
    REQUIRE(n.path(from, to, true).value().points.size() == 2);
    n.set_obstacles({});
    REQUIRE(n.grid().blocked.empty());
    // Outside the grid: nothing to block. The grid's agent radius widens every obstacle.
    n.set_obstacles({{8, {30.0f, 0, 30.0f}, 2.0f}});
    REQUIRE(n.grid().blocked_count() == 0);
    nav::Grid wide = open_grid(12, 12);
    wide.agent_radius = 1.0f;
    n.set_grid(wide);
    n.set_obstacles({{7, {6.0f, 0, 5.5f}, 1.2f}});
    const std::size_t widened = n.grid().blocked_count();
    wide.agent_radius = 0.0f;
    n.set_grid(wide);
    REQUIRE(n.grid().blocked_count() < widened);  // set_grid keeps the obstacles and reapplies them
    REQUIRE(n.grid().blocked_count() > 0);
}

TEST_CASE("a queuing agent falls in behind one going its way instead of passing", "[nav][agents][queue]") {
    // Runs until one of the two arrives: the leader when the follower queues behind it, the follower
    // when it passes (the goal is then taken, so the other cannot arrive).
    auto scenario = [](float queue, bool& passed, bool& ever_queued, float& min_gap, float& follower_x, int& queuing_ticks, bool& leader_arrived) {
        World w;
        nav::Nav n;
        n.set_grid(open_grid(12, 12));
        // A slow leader and a fast follower one unit behind, both bound for the far end of the row.
        const EntityId leader = w.spawn("Leader", 0, Json{{"Transform", {{"position", {{"x", 1.5f}, {"y", 0}, {"z", 8.5f}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", 10.5f}, {"y", 0}, {"z", 8.5f}}}, {"speed", 1.0}, {"radius", 0.4}}}}).value();
        const EntityId follower = w.spawn("Follower", 0, Json{{"Transform", {{"position", {{"x", 0.5f}, {"y", 0}, {"z", 8.5f}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", 10.5f}, {"y", 0}, {"z", 8.5f}}}, {"speed", 2.0}, {"radius", 0.4}, {"queue", queue}}}}).value();
        auto pos = [&](EntityId id) { return w.try_get<Transform>(id)->position; };
        passed = false;
        ever_queued = false;
        min_gap = 1e9f;
        queuing_ticks = 0;
        for (int tick = 0; tick < 900 && w.try_get<NavAgent>(leader)->state != 2 && w.try_get<NavAgent>(follower)->state != 2; ++tick) {   // nine units at one per second, plus the arrival
            w.set_tick_index(tick);
            n.step(w, 1.0f / 60.0f);
            if (pos(follower).x > pos(leader).x) passed = true;
            if (w.try_get<NavAgent>(follower)->queued) ever_queued = true;
            queuing_ticks += n.crowd_stats().queuing;
            min_gap = std::min(min_gap, length(pos(follower) - pos(leader)));
        }
        follower_x = pos(follower).x;
        leader_arrived = w.try_get<NavAgent>(leader)->state == 2;
    };
    bool passed = false, ever_queued = false, leader_arrived = false;
    float min_gap = 0, follower_x = 0;
    int queuing_ticks = 0;
    // With queue 1 the follower matches the leader's pace, keeps its distance and never passes.
    scenario(1.0f, passed, ever_queued, min_gap, follower_x, queuing_ticks, leader_arrived);
    INFO("queue 1: passed " << passed << " queued " << ever_queued << " gap " << min_gap << " x " << follower_x << " queuing ticks " << queuing_ticks);
    REQUIRE(leader_arrived);
    REQUIRE_FALSE(passed);
    REQUIRE(ever_queued);
    REQUIRE(queuing_ticks > 100);
    REQUIRE(min_gap > 0.7f);
    REQUIRE(follower_x > 7.0f);   // it followed the leader down the row rather than standing
    // Without queue the follower steps aside and passes.
    scenario(0.0f, passed, ever_queued, min_gap, follower_x, queuing_ticks, leader_arrived);
    INFO("queue 0: passed " << passed << " queued " << ever_queued << " gap " << min_gap << " x " << follower_x);
    REQUIRE_FALSE(leader_arrived);   // the follower got there first
    REQUIRE(passed);
    REQUIRE_FALSE(ever_queued);
    REQUIRE(queuing_ticks == 0);
    REQUIRE(min_gap > 0.7f);
}

TEST_CASE("formation followers keep their slots behind and beside a leader as it walks and turns", "[nav][agents][formation]") {
    World w;
    nav::Nav n;
    n.set_grid(open_grid(12, 12));
    auto spawn = [&](const char* name, Vec3 pos, Json agent) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"NavAgent", agent}}).value();
    };
    auto pos = [&](EntityId id) { return w.try_get<Transform>(id)->position; };
    auto state = [&](EntityId id) { return w.try_get<NavAgent>(id)->state; };
    // The leader walks +X along a row; one follower a unit behind, one two behind, one behind and to the right (+Z).
    const EntityId leader = spawn("Leader", {2.5f, 0, 6.5f}, Json{{"mode", 1}, {"goal", {{"x", 9.5f}, {"y", 0}, {"z", 6.5f}}}, {"speed", 2.0}, {"radius", 0.3}});
    const EntityId f1 = spawn("F1", {1.5f, 0, 6.5f}, Json{{"mode", 3}, {"target", leader}, {"offset", {{"x", -1.0f}, {"y", 0}, {"z", 0}}}, {"speed", 3.0}, {"radius", 0.3}});
    const EntityId f2 = spawn("F2", {0.5f, 0, 6.5f}, Json{{"mode", 3}, {"target", leader}, {"offset", {{"x", -2.0f}, {"y", 0}, {"z", 0}}}, {"speed", 3.0}, {"radius", 0.3}});
    const EntityId f3 = spawn("F3", {1.5f, 0, 7.5f}, Json{{"mode", 3}, {"target", leader}, {"offset", {{"x", -1.0f}, {"y", 0}, {"z", 1.0f}}}, {"speed", 3.0}, {"radius", 0.3}});
    int tick = 0;
    float worst = 0;   // the largest slot error of F1 once the group is under way
    for (; tick < 600 && state(leader) != 2; ++tick) {
        w.set_tick_index(tick);
        n.step(w, 1.0f / 60.0f);
        if (tick > 60) worst = std::max(worst, std::hypot(pos(f1).x - (pos(leader).x - 1.0f), pos(f1).z - 6.5f));
    }
    // Under way, no follower reported an arrival: the three at tick 0 (they start in their slots
    // with the leader still standing) and the leader's are all there is when the leader stops.
    const std::size_t arrived_at_stop = w.events().since(0, 100, "nav.arrived").size();
    for (int i = 0; i < 90; ++i) {
        w.set_tick_index(tick++);
        n.step(w, 1.0f / 60.0f);
    }
    INFO("leader " << pos(leader).x << "," << pos(leader).z << " f1 " << pos(f1).x << "," << pos(f1).z << " f2 " << pos(f2).x << "," << pos(f2).z << " f3 " << pos(f3).x << "," << pos(f3).z << " worst " << worst << " arrived at stop " << arrived_at_stop);
    REQUIRE(state(leader) == 2);
    REQUIRE(arrived_at_stop <= 4);
    REQUIRE(worst < 0.5f);   // it kept up while the leader walked
    REQUIRE(std::hypot(pos(f1).x - (pos(leader).x - 1.0f), pos(f1).z - pos(leader).z) < 0.25f);
    REQUIRE(std::hypot(pos(f2).x - (pos(leader).x - 2.0f), pos(f2).z - pos(leader).z) < 0.25f);
    REQUIRE(std::hypot(pos(f3).x - (pos(leader).x - 1.0f), pos(f3).z - (pos(leader).z + 1.0f)) < 0.3f);
    REQUIRE(state(f1) == 2);   // in place with the leader standing
    REQUIRE(w.try_get<NavAgent>(f1)->distance < 0.3f);
    // Arrival was reported once per follower, when the leader stopped.
    REQUIRE(w.events().since(0, 100, "nav.arrived").size() == arrived_at_stop + 3);
    // The leader turns down the column (-Z): the slots swing round behind it and to its new right (+X).
    REQUIRE(w.set(leader, "NavAgent", Json{{"goal", {{"x", 9.5f}, {"y", 0}, {"z", 1.5f}}}}).has_value());
    w.set_tick_index(tick++);
    n.step(w, 1.0f / 60.0f);   // the state leaves 2 on the first step toward the new goal
    for (int i = 0; i < 600 && state(leader) != 2; ++i) {
        w.set_tick_index(tick++);
        n.step(w, 1.0f / 60.0f);
    }
    for (int i = 0; i < 90; ++i) {
        w.set_tick_index(tick++);
        n.step(w, 1.0f / 60.0f);
    }
    INFO("after the turn: leader " << pos(leader).x << "," << pos(leader).z << " f1 " << pos(f1).x << "," << pos(f1).z << " f3 " << pos(f3).x << "," << pos(f3).z);
    REQUIRE(state(leader) == 2);
    REQUIRE(std::hypot(pos(f1).x - pos(leader).x, pos(f1).z - (pos(leader).z + 1.0f)) < 0.3f);
    REQUIRE(std::hypot(pos(f2).x - pos(leader).x, pos(f2).z - (pos(leader).z + 2.0f)) < 0.3f);
    REQUIRE(std::hypot(pos(f3).x - (pos(leader).x + 1.0f), pos(f3).z - (pos(leader).z + 1.0f)) < 0.35f);
    // A leader that vanishes leaves its followers stuck.
    REQUIRE(w.destroy(leader).has_value());
    w.set_tick_index(tick++);
    n.step(w, 1.0f / 60.0f);
    REQUIRE(state(f1) == 3);
}

TEST_CASE("a follower whose slot is behind a wall paths around it and re-forms", "[nav][agents][formation][formationpath]") {
    World w;
    nav::Nav n;
    nav::Grid g = open_grid(12, 12);
    for (int y = 3; y <= 9; ++y) g.walkable[g.index(4, y)] = 0;   // a wall across the middle, open at both ends
    n.set_grid(g);
    auto spawn = [&](const char* name, Vec3 pos, Json agent) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"NavAgent", agent}}).value();
    };
    auto pos = [&](EntityId id) { return w.try_get<Transform>(id)->position; };
    auto state = [&](EntityId id) { return w.try_get<NavAgent>(id)->state; };
    // The leader stands east of the wall; its follower starts west of it, its slot a unit behind
    // the leader (heading +X until it moves), right through the wall from where it stands.
    const EntityId leader = spawn("Leader", {7.5f, 0, 6.5f}, Json{{"mode", 1}, {"goal", {{"x", 7.5f}, {"y", 0}, {"z", 6.5f}}}, {"speed", 2.0}, {"radius", 0.3}});
    const EntityId f1 = spawn("F1", {2.5f, 0, 6.5f}, Json{{"mode", 3}, {"target", leader}, {"offset", {{"x", -1.0f}, {"y", 0}, {"z", 0}}}, {"speed", 3.0}, {"radius", 0.3}});
    float detour = 0;   // how far off the row the follower went
    int detours = 0;
    int tick = 0;
    for (; tick < 900 && state(f1) != 2; ++tick) {
        w.set_tick_index(tick);
        n.step(w, 1.0f / 60.0f);
        detour = std::max(detour, std::fabs(pos(f1).z - 6.5f));
        detours += n.crowd_stats().detours;
    }
    INFO("f1 " << pos(f1).x << "," << pos(f1).z << " detour " << detour << " ticks " << tick << " detour ticks " << detours);
    REQUIRE(state(f1) == 2);
    REQUIRE(std::hypot(pos(f1).x - 6.5f, pos(f1).z - 6.5f) < 0.3f);   // in its slot, east of the wall
    REQUIRE(detour > 2.5f);   // around the wall's end, not through it
    REQUIRE(detours > 30);    // pathing while out of sight
    REQUIRE(n.crowd_stats().detours == 0);   // in sight once in place
    REQUIRE(w.try_get<NavAgent>(f1)->corner.x < 7.0f);
}

TEST_CASE("a higher-priority agent walks straight while the lower one yields", "[nav][agents][priority]") {
    World w;
    nav::Nav n;
    n.set_grid(open_grid(12, 12));
    const EntityId boss = w.spawn("Boss", 0, Json{{"Transform", {{"position", {{"x", 1.5f}, {"y", 0}, {"z", 5.5f}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", 10.5f}, {"y", 0}, {"z", 5.5f}}}, {"speed", 2.0}, {"radius", 0.4}, {"priority", 1}}}}).value();
    const EntityId minion = w.spawn("Minion", 0, Json{{"Transform", {{"position", {{"x", 10.5f}, {"y", 0}, {"z", 5.5f}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", 1.5f}, {"y", 0}, {"z", 5.5f}}}, {"speed", 2.0}, {"radius", 0.4}}}}).value();
    auto pos = [&](EntityId id) { return w.try_get<Transform>(id)->position; };
    float boss_off = 0, minion_off = 0, min_gap = 1e9f;
    int boss_neighbours = 0;
    for (int tick = 0; tick < 600 && !(w.try_get<NavAgent>(boss)->state == 2 && w.try_get<NavAgent>(minion)->state == 2); ++tick) {
        w.set_tick_index(tick);
        n.step(w, 1.0f / 60.0f);
        boss_off = std::max(boss_off, std::fabs(pos(boss).z - 5.5f));
        minion_off = std::max(minion_off, std::fabs(pos(minion).z - 5.5f));
        min_gap = std::min(min_gap, length(pos(boss) - pos(minion)));
        boss_neighbours = std::max(boss_neighbours, w.try_get<NavAgent>(boss)->neighbours);
    }
    INFO("boss off " << boss_off << " minion off " << minion_off << " gap " << min_gap);
    REQUIRE(w.try_get<NavAgent>(boss)->state == 2);
    REQUIRE(w.try_get<NavAgent>(minion)->state == 2);
    REQUIRE(boss_neighbours == 0);       // the boss's avoidance never considered the minion
    REQUIRE(boss_off < 0.05f);           // and walked its line
    REQUIRE(minion_off > 0.3f);          // the minion stepped aside
    REQUIRE(min_gap > 0.7f);
}

TEST_CASE("agents walk to their goals, pass each other, go around obstacles and report arrival or stuck", "[nav][agents]") {
    World w;
    nav::Nav n;
    n.set_grid(open_grid(12, 12));
    auto agent = [&](const char* name, Vec3 pos, Vec3 goal) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", goal.x}, {"y", goal.y}, {"z", goal.z}}}, {"speed", 2.0}, {"radius", 0.4}}}}).value();
    };
    auto pos = [&](EntityId id) { return w.try_get<Transform>(id)->position; };
    auto state = [&](EntityId id) { return w.try_get<NavAgent>(id)->state; };
    int tick = 0;
    auto run = [&](int ticks, auto&& done) {
        for (int i = 0; i < ticks && !done(); ++i) {
            w.set_tick_index(tick++);
            n.step(w, 1.0f / 60.0f);
        }
    };
    // Two agents on the same row walking toward each other's start pass without overlapping.
    const EntityId a = agent("A", {1.5f, 0, 5.5f}, {10.5f, 0, 5.5f});
    const EntityId b = agent("B", {10.5f, 0, 5.5f}, {1.5f, 0, 5.5f});
    float min_gap = 1e9f;
    int ticks = 0;
    for (; ticks < 600 && !(state(a) == 2 && state(b) == 2); ++ticks) {
        w.set_tick_index(tick++);
        n.step(w, 1.0f / 60.0f);
        min_gap = std::min(min_gap, length(pos(a) - pos(b)));
    }
    INFO("ticks " << ticks << " gap " << min_gap << " a " << pos(a).x << "," << pos(a).z << " b " << pos(b).x << "," << pos(b).z);
    REQUIRE(state(a) == 2);
    REQUIRE(state(b) == 2);
    REQUIRE(ticks < 450);                    // 9 units at 2 per second is 270 ticks; the detour costs a little
    REQUIRE(min_gap > 0.7f);                 // two radii of 0.4: never overlapped
    REQUIRE(length(pos(a) - Vec3{10.5f, 0, 5.5f}) <= 0.35f);
    REQUIRE(length(pos(b) - Vec3{1.5f, 0, 5.5f}) <= 0.35f);
    REQUIRE(pos(a).y == 0);                  // the plane is XZ: y is left alone
    REQUIRE(n.crowd_stats().arrived == 2);
    REQUIRE(w.events().since(0, 100, "nav.arrived").size() == 2);
    REQUIRE(w.try_get<NavAgent>(a)->velocity.x == 0);
    // Idle agents are left alone.
    const EntityId idle = agent("Idle", {0.5f, 0, 0.5f}, {10.5f, 0, 10.5f});
    REQUIRE(w.set(idle, "NavAgent", Json{{"mode", 0}}).has_value());
    run(60, [] { return false; });
    REQUIRE(pos(idle).x == 0.5f);
    REQUIRE(state(idle) == 0);
    // An obstacle in the corridor: the agent goes around it and still arrives.
    const EntityId cart = w.spawn("Cart", 0, Json{{"Transform", {{"position", {{"x", 6.0f}, {"y", 0}, {"z", 1.5f}}}}}, {"NavObstacle", {{"radius", 1.0}}}}).value();
    const EntityId c = agent("C", {1.5f, 0, 1.5f}, {10.5f, 0, 1.5f});
    float nearest_cart = 1e9f;
    run(600, [&] {
        nearest_cart = std::min(nearest_cart, std::hypot(pos(c).x - 6.0f, pos(c).z - 1.5f));
        return state(c) == 2;
    });
    INFO("c at " << pos(c).x << "," << pos(c).z << " nearest " << nearest_cart);
    REQUIRE(state(c) == 2);
    REQUIRE(nearest_cart > 1.0f);
    REQUIRE(n.describe()["obstacles"].get<int>() == 1);
    REQUIRE(n.describe()["blocked"].get<int>() > 0);
    REQUIRE(w.set(cart, "NavObstacle", Json{{"enabled", false}}).has_value());
    run(1, [] { return false; });
    REQUIRE(n.describe()["blocked"].get<int>() == 0);
    // A goal on the far side of a wall: the agent walks to the closest cell and reports stuck.
    nav::Grid walled = open_grid(12, 12);
    for (int x = 0; x < 12; ++x) walled.walkable[walled.index(x, 8)] = 0;
    n.set_grid(walled);
    const EntityId d = agent("D", {5.5f, 0, 2.5f}, {5.5f, 0, 10.5f});
    run(400, [&] { return state(d) == 3; });
    INFO("d at " << pos(d).x << "," << pos(d).z);
    REQUIRE(state(d) == 3);
    REQUIRE(pos(d).z > 6.8f);               // up to the wall's row
    REQUIRE(w.events().since(0, 100, "nav.stuck").size() == 1);
    // A target that is gone leaves the agent stuck too; a target that exists is followed.
    const EntityId e = agent("E", {1.5f, 0, 1.5f}, {0, 0, 0});
    REQUIRE(w.set(e, "NavAgent", Json{{"mode", 2}, {"target", d}}).has_value());
    run(120, [] { return false; });
    REQUIRE(state(e) == 1);
    REQUIRE(pos(e).x > 1.5f);
    REQUIRE(w.destroy(d).has_value());
    run(1, [] { return false; });
    REQUIRE(state(e) == 3);
    Json list = n.agents(w);
    REQUIRE(list.size() == 5);
    REQUIRE(list[0]["path"] == "/A");
    REQUIRE(list[0]["state"].get<int>() == 2);
}

TEST_CASE("an agent steps out of a moving cart's way before it arrives", "[nav][agents][obstacles][moving]") {
    // A cart crosses the corridor at 3 per second on X while the agent walks up Z through its path.
    // With the cart's velocity in the avoidance the two never overlap; the cart is moved by hand
    // (its Transform written each tick), so the velocity comes from where it went since last tick.
    World w;
    nav::Nav n;
    n.set_grid(open_grid(16, 16));
    const EntityId walker = w.spawn("Walker", 0, Json{{"Transform", {{"position", {{"x", 8.0f}, {"y", 0}, {"z", 1.5f}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", 8.0f}, {"y", 0}, {"z", 14.5f}}}, {"speed", 2.0}, {"radius", 0.4}}}}).value();
    const EntityId cart = w.spawn("Cart", 0, Json{{"Transform", {{"position", {{"x", 1.0f}, {"y", 0}, {"z", 6.0f}}}}}, {"NavObstacle", {{"radius", 0.8}}}}).value();
    auto pos = [&](EntityId id) { return w.try_get<Transform>(id)->position; };
    float nearest = 1e9f;
    int tick = 0, moving_seen = 0;
    for (; tick < 600 && w.try_get<NavAgent>(walker)->state != 2; ++tick) {
        // The cart drives across at 3 per second; it meets the agent's line at x = 8 after about 2.3 s,
        // when the walker (2 per second from z 1.5) is at about z 6, right in its path.
        Transform ct = *w.try_get<Transform>(cart);
        ct.position.x = 1.0f + 3.0f * static_cast<float>(tick) / 60.0f;
        w.set_typed<Transform>(cart, ct);
        w.set_tick_index(tick);
        n.step(w, 1.0f / 60.0f);
        moving_seen += n.crowd_stats().moving_obstacles;
        nearest = std::min(nearest, std::hypot(pos(walker).x - pos(cart).x, pos(walker).z - pos(cart).z));
    }
    INFO("ticks " << tick << " nearest " << nearest << " walker " << pos(walker).x << "," << pos(walker).z << " cart x " << pos(cart).x);
    REQUIRE(w.try_get<NavAgent>(walker)->state == 2);
    REQUIRE(nearest > 1.2f);                 // radii 0.4 + 0.8: never overlapped
    REQUIRE(moving_seen > 100);              // the cart's velocity was seen on nearly every tick
    REQUIRE(n.describe()["moving_obstacles"].is_number());
    // The same crossing with the cart's velocity hidden (a Velocity component reading zero while the
    // Transform is written by hand, so the obstacle reads as standing still) comes closer: the
    // anticipation, not the blocked cells, is what keeps them apart.
    World w2;
    nav::Nav n2;
    n2.set_grid(open_grid(16, 16));
    const EntityId walker2 = w2.spawn("Walker", 0, Json{{"Transform", {{"position", {{"x", 8.0f}, {"y", 0}, {"z", 1.5f}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", 8.0f}, {"y", 0}, {"z", 14.5f}}}, {"speed", 2.0}, {"radius", 0.4}}}}).value();
    const EntityId cart2 = w2.spawn("Cart", 0, Json{{"Transform", {{"position", {{"x", 1.0f}, {"y", 0}, {"z", 6.0f}}}}}, {"NavObstacle", {{"radius", 0.8}}}, {"Velocity", Json::object()}}).value();
    float nearest2 = 1e9f;
    for (int t = 0; t < 600 && w2.try_get<NavAgent>(walker2)->state != 2; ++t) {
        Transform ct = *w2.try_get<Transform>(cart2);
        ct.position.x = 1.0f + 3.0f * static_cast<float>(t) / 60.0f;
        w2.set_typed<Transform>(cart2, ct);
        w2.set_tick_index(t);
        n2.step(w2, 1.0f / 60.0f);
        const auto& p = w2.try_get<Transform>(walker2)->position;
        nearest2 = std::min(nearest2, std::hypot(p.x - ct.position.x, p.z - ct.position.z));
    }
    INFO("without anticipation nearest " << nearest2);
    REQUIRE(nearest2 < nearest);
}

TEST_CASE("the navmesh covers open ground with one rectangle, routes through a gap and yields to obstacles and steps", "[nav][mesh]") {
    // Open ground: one polygon, a straight path, one node expanded.
    nav::Nav nav;
    nav.set_grid(open_grid(30, 30));
    REQUIRE(nav.mesh().polys.size() == 1);
    REQUIRE(nav.mesh().portal_count() == 0);
    auto straight = nav.path({0.5f, 0, 0.5f}, {29.5f, 0, 29.5f});
    REQUIRE(straight.has_value());
    REQUIRE(straight->mesh == true);
    REQUIRE(straight->polys == 1);
    REQUIRE(straight->expanded == 1);
    REQUIRE(straight->points.size() == 2);
    REQUIRE(straight->length == Catch::Approx(std::hypot(29.0f, 29.0f)).margin(1e-3));
    // A wall down the middle with one gap: a few rectangles, a path through the gap about as long
    // as the cell path's (which grazes the wall's corners; the mesh path keeps off them), the cell
    // path still there on request.
    nav::Grid g = open_grid(30, 30);
    for (int y = 0; y < 30; ++y) if (y != 14) g.walkable[g.index(15, y)] = 0;
    nav.set_grid(g);
    INFO("polygons " << nav.mesh().polys.size() << " portals " << nav.mesh().portal_count());
    REQUIRE(nav.mesh().polys.size() >= 3);
    REQUIRE(nav.mesh().polys.size() <= 8);
    REQUIRE(nav.mesh().portal_count() >= 4);
    auto through = nav.path({2.5f, 0, 2.5f}, {27.5f, 0, 27.5f});
    auto cells = nav.path({2.5f, 0, 2.5f}, {27.5f, 0, 27.5f}, true, false);
    REQUIRE(through.has_value());
    REQUIRE(cells.has_value());
    REQUIRE(through->mesh == true);
    REQUIRE(cells->mesh == false);
    REQUIRE(through->partial == false);
    REQUIRE(through->polys >= 2);
    REQUIRE(through->length <= cells->length + 0.5f);
    REQUIRE(through->length >= std::hypot(25.0f, 25.0f) - 1e-3f);
    bool passes_gap = false;
    for (const Vec3& pt : through->points) if (std::fabs(pt.x - 15.5f) < 1.01f && std::fabs(pt.z - 14.5f) < 1.01f) passes_gap = true;
    REQUIRE(passes_gap);
    REQUIRE(through->expanded < cells->expanded);
    // An obstacle in the gap: the mesh path would cross it, so the cells decide (and find no way).
    nav.set_obstacles({{1, {15.5f, 0, 14.5f}, 0.4f}});
    auto blocked = nav.path({2.5f, 0, 2.5f}, {27.5f, 0, 27.5f});
    REQUIRE(blocked.has_value());
    REQUIRE(blocked->mesh == false);
    REQUIRE(blocked->partial == true);
    nav.set_obstacles({});
    REQUIRE(nav.path({2.5f, 0, 2.5f}, {27.5f, 0, 27.5f})->mesh == true);
    // A ledge above the step: two rectangles without a portal, so the way between them is the
    // cells' partial path.
    nav::Grid ledge = open_grid(10, 4);
    for (int y = 0; y < 4; ++y) for (int x = 5; x < 10; ++x) ledge.ground[ledge.index(x, y)] = 1.0f;
    nav.set_grid(ledge);
    REQUIRE(nav.mesh().polys.size() == 2);
    REQUIRE(nav.mesh().portal_count() == 0);
    auto up = nav.path({1.5f, 0, 1.5f}, {8.5f, 1, 1.5f});
    REQUIRE(up.has_value());
    REQUIRE(up->mesh == false);
    REQUIRE(up->partial == true);
    // Along the ledge the mesh serves: one rectangle, height carried in the points.
    auto along = nav.path({5.5f, 1, 0.5f}, {9.5f, 1, 3.5f});
    REQUIRE(along->mesh == true);
    REQUIRE(along->points.front().y == Catch::Approx(1.0f));
    REQUIRE(along->points.back().y == Catch::Approx(1.0f));
    // A platformer grid (links) has no mesh.
    nav::Grid pf = open_grid(6, 3);
    pf.plane = 1;
    pf.links.assign(pf.walkable.size(), {});
    pf.links[0].push_back(2);
    nav.set_grid(pf);
    REQUIRE(nav.mesh().empty());
    REQUIRE(nav.path({0.5f, -0.5f, 0}, {5.5f, -2.5f, 0})->mesh == false);
    REQUIRE(nav.describe()["mesh"]["polygons"] == 0);
}

namespace {
// A small map of one orientation with a wall of solid tiles down column 3, open in the last `open_rows` rows.
std::string wall_map(const char* orientation, int w, int h, int tile_w, int tile_h, int hex_side, const char* stagger_axis, const char* stagger_index, int open_rows) {
    Json data = Json::array();
    for (int y = 0; y < h; ++y) for (int x = 0; x < w; ++x) data.push_back(x == 3 && y < h - open_rows ? 1 : 0);
    Json doc = {{"type", "map"}, {"version", "1.10"}, {"orientation", orientation}, {"renderorder", "right-down"}, {"width", w}, {"height", h}, {"tilewidth", tile_w}, {"tileheight", tile_h}, {"infinite", false}, {"nextlayerid", 2}, {"nextobjectid", 1},
                {"tilesets", Json::array({{{"firstgid", 1}, {"name", "t"}, {"image", "t.png"}, {"imagewidth", tile_w}, {"imageheight", tile_h}, {"tilewidth", tile_w}, {"tileheight", tile_h}, {"columns", 1}, {"tilecount", 1},
                                           {"tiles", Json::array({{{"id", 0}, {"properties", Json::array({{{"name", "solid"}, {"type", "bool"}, {"value", true}}})}}})}}})},
                {"layers", Json::array({{{"id", 1}, {"type", "tilelayer"}, {"name", "walls"}, {"width", w}, {"height", h}, {"x", 0}, {"y", 0}, {"opacity", 1}, {"visible", true}, {"data", data}}})}};
    if (hex_side > 0) doc["hexsidelength"] = hex_side;
    if (stagger_axis) { doc["staggeraxis"] = stagger_axis; doc["staggerindex"] = stagger_index; }
    return doc.dump();
}
}  // namespace

TEST_CASE("hexagonal, staggered and isometric maps bake to grids whose cells sit where the map draws them", "[nav][hexgrid]") {
    const std::filesystem::path dir = root() / "build" / "test-out" / "nav-layouts";
    std::filesystem::create_directories(dir);
    // Staggered diamonds touch along a row only at a corner, and a corner is never cut past a
    // wall, so their wall opens three rows: the way around goes up through the diamonds' edges.
    struct Case { const char* file; std::string text; const char* layout; int open_rows; };
    const Case cases[] = {
        {"hex.tmj", wall_map("hexagonal", 7, 6, 32, 32, 16, "y", "odd", 1), "hexagonal", 1},
        {"hexcol.tmj", wall_map("hexagonal", 7, 6, 32, 32, 16, "x", "even", 1), "hexagonal", 1},
        {"stag.tmj", wall_map("staggered", 7, 6, 64, 32, 0, "y", "odd", 3), "staggered", 3},
        {"iso.tmj", wall_map("isometric", 7, 6, 64, 32, 0, nullptr, nullptr, 1), "isometric", 1},
    };
    for (const Case& c : cases) {
        DYNAMIC_SECTION(c.file) {
            {
                std::ofstream f(dir / c.file, std::ios::binary);
                f << c.text;
            }
            World w;
            assets::AssetStore store(dir);
            const EntityId map = w.spawn("Map", 0, Json{{"Transform", {{"position", {{"x", 1.0f}, {"y", 2.0f}, {"z", 0}}}}}, {"TileMap", {{"map", c.file}, {"tile_size", 0.5}}}}).value();
            nav::Nav n;
            nav::TileBakeParams tp;
            tp.mode = "topdown";
            REQUIRE(n.bake_tilemap(w, store, map, tp, 1).has_value());
            const nav::Grid& g = n.grid();
            REQUIRE(n.describe()["layout"] == c.layout);
            REQUIRE(g.cell == Catch::Approx(0.5f));
            REQUIRE(g.walkable_count() == static_cast<std::size_t>(7 * 6 - (6 - c.open_rows)));
            REQUIRE(n.mesh().polys.empty());   // rectangles cover square lattices only
            // Every cell's center maps back to the cell, and centers of neighbours in a row are a tile apart.
            for (int y = 0; y < g.height; ++y) {
                for (int x = 0; x < g.width; ++x) {
                    int cx = -1, cy = -1;
                    REQUIRE(g.cell_of(g.center_of(x, y), cx, cy));
                    REQUIRE(cx == x);
                    REQUIRE(cy == y);
                }
            }
            const Vec3 c00 = g.center_of(0, 0), c10 = g.center_of(1, 0), c01 = g.center_of(0, 1);
            INFO(c.file << " c00 " << c00.x << "," << c00.y << " c10 " << c10.x << "," << c10.y << " c01 " << c01.x << "," << c01.y);
            REQUIRE(c00.y <= 2.0f);
            REQUIRE(c10.x > c00.x);
            REQUIRE(c01.y < c00.y);
            // Across the wall: only the last row is open, so the path goes down, across and up, in steps no longer than a neighbour's.
            auto path = n.path(g.center_of(0, 0), g.center_of(6, 0)).value();
            INFO(c.file << " length " << path.length << " points " << path.points.size() << " expanded " << path.expanded);
            REQUIRE_FALSE(path.partial);
            REQUIRE(path.points.size() >= 3);
            const float straight = length(g.center_of(6, 0) - g.center_of(0, 0));
            REQUIRE(path.length > straight);
            REQUIRE(path.length < straight * 4.0f);
            auto cells = n.path(g.center_of(0, 0), g.center_of(6, 0), false).value();   // unsmoothed: every step a neighbour
            const float neighbour = std::max({length(g.center_of(1, 0) - g.center_of(0, 0)), length(g.center_of(0, 1) - g.center_of(0, 0)), length(g.center_of(1, 1) - g.center_of(0, 0)), length(g.center_of(0, 2) - g.center_of(0, 0))});
            for (std::size_t i = 1; i < cells.points.size(); ++i) REQUIRE(length(cells.points[i] - cells.points[i - 1]) <= neighbour * 1.01f);
            REQUIRE(n.reachable(g.center_of(0, 0), g.center_of(6, 5)));
            REQUIRE(n.nearest(Vec3{-5, 2, 0}, 20.0f).has_value());
            // Platformer grids need rows of floor: refused here.
            nav::TileBakeParams pf;
            pf.mode = "platformer";
            REQUIRE(n.bake_tilemap(w, store, map, pf, 2).error().code == "bad_tilemap");
        }
    }
}

TEST_CASE("a bridge over a road bakes two floors: paths under it stay under, paths onto it climb the stairs", "[nav][layers]") {
    World w;
    physics::Physics p;
    static_box(w, "Ground", {0, -0.5f, 0}, {12, 0.5f, 12});
    static_box(w, "Deck", {0, 1.95f, 0}, {3, 0.15f, 1});   // x -3..3, z -1..1, top at 2.1, 1.8 of room under it
    for (int k = 1; k <= 6; ++k) {                           // stairs down the east end, 0.3 a step
        const float top = 2.1f - 0.3f * static_cast<float>(k);
        static_box(w, "Step", {3.25f + 0.5f * static_cast<float>(k - 1), top * 0.5f, 0}, {0.25f, top * 0.5f, 1});
    }
    nav::BakeParams bp;
    bp.min = {-8, -1, -8};
    bp.max = {8, 4, 8};
    bp.cell = 0.5f;
    nav::Nav n;
    REQUIRE(n.bake_colliders(w, p, bp, 1).has_value());
    const nav::Grid& g = n.grid();
    REQUIRE(g.layers == 2);
    REQUIRE(n.describe()["layers"] == 2);
    // The column under the deck holds both floors; the height picks one.
    auto low = g.cell_at({0.25f, 0, 0.25f}), high = g.cell_at({0.25f, 2.1f, 0.25f});
    REQUIRE(low.has_value());
    REQUIRE(high.has_value());
    REQUIRE(g.ground[*low] == Catch::Approx(0.0f).margin(0.01f));
    REQUIRE(g.ground[*high] == Catch::Approx(2.1f).margin(0.01f));
    for (bool mesh : {false, true}) {
        INFO("mesh " << mesh);
        // Along the road under the bridge: straight through, on the ground.
        auto under = n.path({0.25f, 0, -6}, {0.25f, 0, 6}, true, mesh).value();
        REQUIRE_FALSE(under.partial);
        REQUIRE(under.length == Catch::Approx(12.0f).margin(0.3f));
        for (const Vec3& pt : under.points) REQUIRE(pt.y == Catch::Approx(0.0f).margin(0.01f));
        // From the road to the deck: round to the stairs and up them, a step at a time.
        auto up = n.path({-6, 0, 0.25f}, {0.25f, 2.1f, 0.25f}, false, mesh).value();
        REQUIRE_FALSE(up.partial);
        REQUIRE(up.points.back().y == Catch::Approx(2.1f).margin(0.01f));
        float east = -1e9f;
        for (const Vec3& pt : up.points) east = std::max(east, pt.x);
        REQUIRE(east > 4.5f);
        if (!mesh) {
            for (std::size_t i = 1; i < up.points.size(); ++i) REQUIRE(std::fabs(up.points[i].y - up.points[i - 1].y) <= g.max_step + 1e-3f);
        }
    }
    REQUIRE(n.reachable({-6, 0, 0.25f}, {0.25f, 2.1f, 0.25f}));
    // nearest() keeps to the floor the point is on.
    REQUIRE(n.nearest({0.25f, 2.2f, 0.25f}, 1.0f)->y == Catch::Approx(2.1f).margin(0.01f));
    REQUIRE(n.nearest({0.25f, 0.1f, 0.25f}, 1.0f)->y == Catch::Approx(0.0f).margin(0.01f));
    // An agent moved by the nav walks from the road up the stairs onto the deck, rising with them.
    const EntityId a = w.spawn("Walker", 0, Json{{"Transform", {{"position", {{"x", -6}, {"y", 0}, {"z", 0.25f}}}}}, {"NavAgent", {{"mode", 1}, {"goal", {{"x", 0.25f}, {"y", 2.1f}, {"z", 0.25f}}}, {"speed", 3.0}, {"radius", 0.3}}}}).value();
    for (int t = 0; t < 900 && w.try_get<NavAgent>(a)->state != 2; ++t) {
        w.set_tick_index(t);
        n.step(w, 1.0f / 60.0f);
    }
    const Vec3 at = w.try_get<Transform>(a)->position;
    INFO(at.x << "," << at.y << "," << at.z);
    REQUIRE(w.try_get<NavAgent>(a)->state == 2);
    REQUIRE(at.y == Catch::Approx(2.1f).margin(0.05f));
    // The top floor alone: the road under the deck is gone, so the way along it goes round.
    bp.layers = 1;
    REQUIRE(n.bake_colliders(w, p, bp, 2).has_value());
    REQUIRE(n.grid().layers == 1);
    REQUIRE(n.grid().ground[*n.grid().cell_at({0.25f, 0, 0.25f})] == Catch::Approx(2.1f).margin(0.01f));
    REQUIRE(n.path({0.25f, 0, -6}, {0.25f, 0, 6}).value().length > 14.0f);
}
