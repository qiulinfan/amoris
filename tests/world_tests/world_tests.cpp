#include <pocket/world/world.hpp>

#include <catch_amalgamated.hpp>

using namespace pocket;
using namespace pocket::world;

TEST_CASE("spawn, hierarchy, paths and names", "[world]") {
    World w;
    auto root = w.spawn("Level").value();
    auto player = w.spawn("Player", root, Json{{"Transform", {{"position", {{"x", 1}, {"y", 2}, {"z", 3}}}}}, {"Health", {{"current", 50}}}}).value();
    auto weapon = w.spawn("Weapon", player).value();
    REQUIRE(w.path(weapon) == "/Level/Player/Weapon");
    REQUIRE(w.find("/Level/Player") == player);
    REQUIRE(w.find("Level/Player/Weapon") == weapon);
    REQUIRE(w.find("Weapon") == weapon);
    REQUIRE(w.find("/Nope") == 0);
    REQUIRE(w.parent(weapon) == player);
    REQUIRE(w.children(root) == std::vector<EntityId>{player});
    REQUIRE(w.entity_count() == 3);
    // Duplicate sibling names get a suffix.
    auto p2 = w.spawn("Player", root).value();
    REQUIRE(w.name(p2) == "Player_2");
    // Partial set merges onto defaults.
    Json h = w.get(player, "Health").value();
    REQUIRE(h["current"] == 50.0);
    REQUIRE(h["max"] == 100.0);
    REQUIRE(w.set(player, "Health", Json{{"max", 200}}).has_value());
    REQUIRE(w.get(player, "Health").value()["current"] == 50.0);
    REQUIRE(w.get(player, "Health").value()["max"] == 200.0);
    REQUIRE_FALSE(w.get(player, "Nope").has_value());
    REQUIRE(w.get(player, "Nope").error().code == "unknown_component");
    REQUIRE_FALSE(w.get(weapon, "Health").has_value());
    // Destroy removes the subtree.
    REQUIRE(w.destroy(player).has_value());
    REQUIRE_FALSE(w.alive(weapon));
    REQUIRE(w.entity_count() == 2);
}

TEST_CASE("events carry causes and ticks", "[world]") {
    World w;
    w.set_tick_index(7);
    auto e = w.spawn("Bomb", 0, Json{{"Lifetime", {{"seconds", 0.05}}}}).value();
    auto spawned = w.events().recent(1);
    REQUIRE(spawned.size() == 1);
    REQUIRE(spawned[0].type == "entity.spawned");
    REQUIRE(spawned[0].tick == 7);
    REQUIRE(spawned[0].subject == e);
    for (int i = 0; i < 5; ++i) w.tick(1.0 / 60.0);
    REQUIRE_FALSE(w.alive(e));
    auto tail = w.events().since(spawned[0].seq);
    REQUIRE(tail.size() == 2);
    REQUIRE(tail[0].type == "lifetime.expired");
    REQUIRE(tail[1].type == "entity.destroyed");
    REQUIRE(tail[1].cause == tail[0].seq);
    REQUIRE(w.events().histogram()["entity.destroyed"] == 1);
}

TEST_CASE("motion and transform propagation", "[world]") {
    World w;
    auto parent = w.spawn("Parent", 0, Json{{"Transform", {{"position", {{"x", 10}, {"y", 0}, {"z", 0}}}, {"scale", {{"x", 2}, {"y", 2}, {"z", 2}}}}}, {"Velocity", {{"linear", {{"x", 1}, {"y", 0}, {"z", 0}}}}}}).value();
    auto child = w.spawn("Child", parent, Json{{"Transform", {{"position", {{"x", 1}, {"y", 0}, {"z", 0}}}}}}).value();
    w.tick(0.5);
    const Transform* pt = w.try_get<Transform>(parent);
    REQUIRE(pt != nullptr);
    REQUIRE(pt->position.x == Catch::Approx(10.5));
    const WorldTransform* wt = w.try_get<WorldTransform>(child);
    REQUIRE(wt != nullptr);
    REQUIRE(wt->position.x == Catch::Approx(12.5));
    REQUIRE(wt->scale.x == Catch::Approx(2));
    Json d = w.describe(child);
    REQUIRE(d["path"] == "/Parent/Child");
    REQUIRE(d["components"].contains("WorldTransform"));
}

TEST_CASE("tree text, queries and hashing are deterministic", "[world]") {
    auto build = [](World& w) {
        auto level = w.spawn("Level", 0, Json{{"Transform", Json::object()}}).value();
        for (int i = 0; i < 3; ++i) {
            w.spawn("Enemy", level, Json{{"Transform", {{"position", {{"x", i}, {"y", 0}, {"z", 0}}}}}, {"Health", {{"current", 10 * (i + 1)}}}});
        }
        w.spawn("Camera", 0, Json{{"Camera", Json::object()}});
        w.tick(1.0 / 60.0);
    };
    World a, b;
    build(a);
    build(b);
    REQUIRE(a.hash() == b.hash());
    TreeOptions to;
    std::string t = a.tree(to);
    INFO(t);
    REQUIRE(t == b.tree(to));
    REQUIRE(t.find("- Level #") != std::string::npos);
    REQUIRE(t.find("Enemy_2") != std::string::npos);
    REQUIRE(t.find("Health current=20") != std::string::npos);
    REQUIRE(t.find("WorldTransform") != std::string::npos);
    to.depth = 0;
    std::string collapsed = a.tree(to);
    REQUIRE(collapsed.find("(3 children)") != std::string::npos);
    to.depth = -1;
    to.max_entities = 2;
    REQUIRE(a.tree(to).find("more") != std::string::npos);

    QueryOptions q;
    q.with = {"Health"};
    q.name = "Enemy*";
    Json r = a.query(q);
    REQUIRE(r["count"] == 3);
    REQUIRE(r["entities"][1]["Health"]["current"] == 20.0);
    q.without = {"Health"};
    q.with = {};
    q.name = "";
    REQUIRE(a.query(q)["count"] == 2);

    REQUIRE(a.set(a.find("Level/Enemy"), "Health", Json{{"current", 1}}).has_value());
    REQUIRE(a.hash() != b.hash());
}

TEST_CASE("scene save and load round trip", "[world]") {
    World w;
    auto level = w.spawn("Level", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 1}, {"z", 0}}}}}}).value();
    w.spawn("Light", level, Json{{"Light", {{"intensity", 2.5}}}});
    w.spawn("Cam", 0, Json{{"Camera", {{"fov_degrees", 45}}}});
    w.tick(0.1);
    Json scene = w.save();
    REQUIRE(scene["format"] == "pocket-scene");
    REQUIRE(scene["entities"].size() == 2);
    REQUIRE_FALSE(scene["entities"][0]["components"].contains("WorldTransform"));
    World w2;
    REQUIRE(w2.load(scene).has_value());
    REQUIRE(w2.entity_count() == 3);
    REQUIRE(w2.get(w2.find("/Level/Light"), "Light").value()["intensity"] == 2.5);
    REQUIRE(w2.save() == scene);
    REQUIRE(w2.hash() == w.hash());
    // Reparent moves a subtree and keeps paths consistent.
    REQUIRE(w2.reparent(w2.find("Cam"), w2.find("Level")).has_value());
    REQUIRE(w2.path(w2.find("Cam")) == "/Level/Cam");
    REQUIRE_FALSE(w2.reparent(w2.find("Level"), w2.find("Cam")).has_value());
    REQUIRE(w2.roots().size() == 1);
}

TEST_CASE("schema lists every component with defaults", "[world]") {
    Json s = World::schema();
    REQUIRE(s["components"].size() == component_infos().size());
    bool found = false;
    for (auto& c : s["components"]) {
        if (c["name"] == "Transform") {
            found = true;
            REQUIRE(c["default"]["scale"]["x"] == 1.0);
            REQUIRE(c["fields"][0]["name"] == "position");
        }
    }
    REQUIRE(found);
    REQUIRE(World::known_component("Velocity"));
    REQUIRE_FALSE(World::known_component("Nope"));
}
