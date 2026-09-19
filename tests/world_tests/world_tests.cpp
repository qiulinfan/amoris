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

TEST_CASE("sprite clips advance frames, loop, finish and write uv", "[world][sprites]") {
    World w;
    World::SpriteClip spin = World::SpriteClip::from_json(Json{{"texture", "sheet.png"}, {"columns", 4}, {"rows", 2}, {"first", 4}, {"count", 4}, {"fps", 10}}).value();
    REQUIRE(spin.frames == std::vector<int>{4, 5, 6, 7});
    w.define_clip("spin", spin);
    REQUIRE(w.clip("spin") != nullptr);
    auto e = w.spawn("Coin", 0, Json{{"Transform", Json::object()}, {"Sprite", Json::object()}, {"SpriteAnimation", {{"clip", "spin"}}}}).value();
    w.tick(0.0);
    const Sprite* s = w.try_get<Sprite>(e);
    REQUIRE(s != nullptr);
    REQUIRE(s->texture == "sheet.png");                       // the clip's sheet replaces the sprite's texture
    REQUIRE(s->uv.x == Catch::Approx(0.0f));                  // cell 4: column 0, row 1 of a 4x2 grid
    REQUIRE(s->uv.y == Catch::Approx(0.5f));
    REQUIRE(s->uv.z == Catch::Approx(0.25f));
    REQUIRE(s->uv.w == Catch::Approx(1.0f));
    for (int i = 0; i < 6; ++i) w.tick(1.0 / 60.0);            // 0.1 s at 10 fps: one frame
    REQUIRE(w.try_get<SpriteAnimation>(e)->frame == 1);
    REQUIRE(w.try_get<Sprite>(e)->uv.x == Catch::Approx(0.25f));
    for (int i = 0; i < 18; ++i) w.tick(1.0 / 60.0);           // three more: wraps to frame 0
    REQUIRE(w.try_get<SpriteAnimation>(e)->frame == 0);
    REQUIRE(w.try_get<SpriteAnimation>(e)->playing == true);
    // A non-looping clip stops on its last frame and says so once.
    REQUIRE(w.set(e, "SpriteAnimation", Json{{"loop", false}, {"frame", 0}, {"time", 0.0}, {"speed", 2.0}}).has_value());
    for (int i = 0; i < 30; ++i) w.tick(1.0 / 60.0);           // 0.5 s at 20 fps is plenty
    const SpriteAnimation* a = w.try_get<SpriteAnimation>(e);
    REQUIRE(a->finished == true);
    REQUIRE(a->playing == false);
    REQUIRE(a->frame == 3);
    int finished_events = 0;
    for (const auto& ev : w.events().recent(100)) {
        if (ev.type == "sprite.finished") ++finished_events;
    }
    REQUIRE(finished_events == 1);
    // Clips travel with scenes.
    Json scene = w.save();
    REQUIRE(scene["sprite_clips"]["spin"]["fps"] == 10);
    World w2;
    REQUIRE(w2.load(scene).has_value());
    REQUIRE(w2.clip("spin") != nullptr);
    REQUIRE(w2.clip("spin")->frames.size() == 4);
    REQUIRE(World::SpriteClip::from_json(Json{{"columns", 2}, {"frames", {0, 5}}}).has_value() == false);
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

#include <pocket/world/transcript.hpp>

TEST_CASE("transcript segments trends and groups events", "[world]") {
    std::vector<StateSample> samples;
    EventLog log;
    // 30 ticks falling, a bounce event, 20 ticks rising, then 30 ticks constant with a string change.
    double y = 3.0;
    for (int t = 0; t < 80; ++t) {
        if (t < 30) y -= 0.1;
        else if (t < 50) y += 0.05;
        Json s;
        s["y"] = y;
        s["count"] = t < 30 ? 0 : 1;
        s["label"] = t < 60 ? "air" : "ground";
        samples.push_back({t, s});
        if (t == 30) log.emit(t, "bounce", 0, Json{{"path", "/Ball"}});
        if (t >= 60 && t % 5 == 0) log.emit(t, "tick.mark", 0, nullptr);
    }
    TranscriptOptions o;
    Transcript tr = build_transcript(samples, log, o);
    INFO(tr.text);
    REQUIRE(tr.segments.size() >= 3);
    REQUIRE(tr.segments.front().trends["y"]["trend"] == "falling");
    REQUIRE(tr.segments.front().to <= 30);
    bool rising = false, changed = false, bounce = false;
    for (auto& seg : tr.segments) {
        if (seg.trends.contains("y") && seg.trends["y"]["trend"] == "rising") rising = true;
        if (seg.trends.contains("label") && seg.trends["label"]["trend"] == "changed") changed = true;
        if (seg.events.contains("bounce")) bounce = true;
    }
    REQUIRE(rising);
    REQUIRE(changed);
    REQUIRE(bounce);
    REQUIRE(tr.text.find("tick.markx4") != std::string::npos);
    // The line budget coarsens without losing events.
    o.max_lines = 2;
    Transcript small = build_transcript(samples, log, o);
    REQUIRE(small.segments.size() == 1);
    REQUIRE(small.segments[0].events["tick.mark"]["count"] == 4);
    REQUIRE(small.segments[0].events["bounce"]["count"] == 1);
}
