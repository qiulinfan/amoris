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
    World w;
    Json s = w.schema();
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
    REQUIRE(w.known_component("Velocity"));
    REQUIRE_FALSE(w.known_component("Nope"));
}

TEST_CASE("a project declares components of its own: set, queried, hashed, saved and checked like the engine's", "[world][project]") {
    const Json enemy = Json::parse(R"([{"name": "Enemy", "doc": "A foe.", "fields": [
        {"name": "hp", "type": "f32", "default": 10},
        {"name": "speed", "type": "f32", "default": 2.5},
        {"name": "kind", "type": "i32", "names": ["grunt", "boss"]},
        {"name": "home", "type": "vec3"},
        {"name": "tint", "type": "color"},
        {"name": "angry", "type": "bool"},
        {"name": "label", "type": "string", "default": "foe"}]}])");
    World w;
    REQUIRE(w.declare_components(enemy).has_value());
    REQUIRE(w.known_component("Enemy"));
    REQUIRE(w.project_component("Enemy"));
    const std::uint64_t before = w.hash();
    auto id = w.spawn("Orc", 0, Json{{"Enemy", Json{{"kind", "boss"}, {"home", Json::array({1, 2, 3})}}}});
    REQUIRE(id.has_value());
    Json v = w.get(*id, "Enemy").value();
    INFO(v.dump());
    REQUIRE(v["hp"] == 10.0);
    REQUIRE(v["kind"] == 1);
    REQUIRE(v["home"] == Json{{"x", 1.0}, {"y", 2.0}, {"z", 3.0}});
    REQUIRE(v["tint"]["a"] == 1.0);
    REQUIRE(v["label"] == "foe");
    REQUIRE(w.set(*id, "Enemy", Json{{"hp", 4}, {"angry", true}}).has_value());
    REQUIRE(w.get(*id, "Enemy").value()["hp"] == 4.0);
    REQUIRE(w.get(*id, "Enemy").value()["speed"] == 2.5);   // merged, not replaced
    REQUIRE(w.hash() != before);
    const std::uint64_t after = w.hash();
    REQUIRE(w.set(*id, "Enemy", Json{{"hp", 5}}).has_value());
    REQUIRE(w.hash() != after);   // its fields are in the hash
    QueryOptions q;
    q.with = {"Enemy"};
    REQUIRE(w.query(q)["count"] == 1);
    REQUIRE(w.components_of(*id) == std::vector<std::string>{"Enemy"});
    // Checked like an engine component's patch.
    auto bad = w.check_patch("Enemy", Json{{"hpp", 3}});
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().message.find("did you mean 'hp'") != std::string::npos);
    REQUIRE_FALSE(w.check_patch("Enemy", Json{{"kind", "dragon"}}).has_value());
    REQUIRE(w.check_patch("Enemy", Json{{"kind", "grunt"}, {"tint", Json{{"r", 0.5}}}}).has_value());
    // The schema has it, marked as the project's.
    bool listed = false;
    for (const Json& c : w.schema()["components"]) if (c["name"] == "Enemy") listed = c.value("project", false) && c["fields"][2]["names"] == Json::array({"grunt", "boss"});
    REQUIRE(listed);
    // Saved and loaded into a world that declares it too: the same world, the same hash.
    Json saved = w.save();
    World w2;
    REQUIRE(w2.declare_components(enemy).has_value());
    REQUIRE(w2.load(saved).has_value());
    REQUIRE(w2.get(w2.find("Orc"), "Enemy").value() == w.get(*id, "Enemy").value());
    REQUIRE(w2.hash() == w.hash());
    // Declaring again (a reload) keeps the values; an engine name cannot be taken.
    REQUIRE(w.declare_components(enemy).has_value());
    REQUIRE(w.get(*id, "Enemy").value()["hp"] == 5.0);
    REQUIRE_FALSE(w.declare_components(Json::parse(R"([{"name": "Transform", "fields": []}])")).has_value());
    REQUIRE(w.destroy(*id).has_value());
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

#include <pocket/world/recorder.hpp>

TEST_CASE("the recorder replays the world at any kept tick", "[world][recorder]") {
    World w;
    Recorder r;
    r.start(5);
    EntityId ball = w.spawn("Ball", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 10}, {"z", 0}}}}}, {"Health", {{"current", 100}}}}).value();
    for (int tick = 1; tick <= 12; ++tick) {
        w.set_tick_index(tick);
        REQUIRE(w.set(ball, "Transform", Json{{"position", {{"y", 10 - tick}}}}).has_value());
        if (tick == 4) REQUIRE(w.spawn("Late", 0, Json{{"Transform", Json::object()}}).has_value());
        if (tick == 6) REQUIRE(w.set(ball, "Health", Json{{"current", 40}}).has_value());
        if (tick == 8) REQUIRE(w.remove(ball, "Health").has_value());
        if (tick == 9) REQUIRE(w.rename(ball, "Orb").has_value());
        r.record(w, tick);
    }
    Json st = r.status();
    REQUIRE(st["frames"].get<int>() == 5);   // the ring keeps ticks 8..12
    REQUIRE(st["from"].get<int>() == 8);
    REQUIRE(st["to"].get<int>() == 12);
    REQUIRE_FALSE(r.at(3).has_value());       // evicted
    // Tick 8: the health is gone, the name is still Ball, y is 2.
    Json at8 = r.at(8, ball).value();
    REQUIRE(at8["entity"]["path"] == "/Ball");
    REQUIRE_FALSE(at8["entity"]["components"].contains("Health"));
    REQUIRE(at8["entity"]["components"]["Transform"]["position"]["y"].get<double>() == Catch::Approx(2.0));
    Json at10 = r.at(10).value();
    REQUIRE(at10["entities"].size() == 2);
    REQUIRE(at10["entities"][0]["path"] == "/Orb");
    // Base folding kept the history that was evicted: Late (spawned at tick 4) exists at tick 8.
    bool late = false;
    for (const auto& e : r.at(8).value()["entities"]) if (e["path"] == "/Late") late = true;
    REQUIRE(late);
    Json d = r.diff(8, 12).value();
    REQUIRE(d["renamed"].size() == 1);
    REQUIRE(d["renamed"][0]["to"] == "/Orb");
    REQUIRE(d["changed"].size() == 1);
    REQUIRE(d["changed"][0]["field"] == "Transform.position.y");
    REQUIRE(d["changed"][0]["from"].get<double>() == Catch::Approx(2.0));
    REQUIRE(d["changed"][0]["to"].get<double>() == Catch::Approx(-2.0));
    Json tr = r.track(ball, "Transform", "position.y").value();
    REQUIRE(tr["ticks"] == Json::array({8, 9, 10, 11, 12}));
    REQUIRE(tr["values"][0].get<double>() == Catch::Approx(2.0));
    REQUIRE(tr["values"][4].get<double>() == Catch::Approx(-2.0));
    Json f = r.first(ball, "Transform", "position.y", "<", 0).value();
    REQUIRE(f["tick"].get<int>() == 11);
    REQUIRE(r.first(ball, "Transform", "position.y", "<", -100).value().is_null());
    REQUIRE_FALSE(r.first(ball, "Transform", "position.y", "~", 0).has_value());
    REQUIRE(Recorder::field_of(Json{{"a", {{"b", 3}}}}, "a.b") == 3);
    REQUIRE(Recorder::field_of(Json{{"a", 1}}, "a.b").is_null());
    // Destruction shows in a diff and the entity vanishes from later ticks.
    w.set_tick_index(13);
    REQUIRE(w.destroy(ball).has_value());
    r.record(w, 13);
    REQUIRE(r.diff(12, 13).value()["destroyed"].size() == 1);
    REQUIRE_FALSE(r.at(13, ball).has_value());
    r.stop();
    REQUIRE_FALSE(r.recording());
}

TEST_CASE("events.why walks cause links", "[world][events]") {
    World w;
    auto& ev = w.events();
    std::uint64_t a = ev.emit(1, "input.jump", 0, nullptr);
    std::uint64_t b = ev.emit(1, "player.jumped", 0, nullptr, a, "script");
    std::uint64_t c = ev.emit(3, "coin.collected", 0, nullptr, b, "script");
    auto chain = ev.why(c);
    REQUIRE(chain.size() == 3);
    REQUIRE(chain[0].type == "coin.collected");
    REQUIRE(chain[2].type == "input.jump");
    REQUIRE(ev.why(a).size() == 1);
    REQUIRE(ev.why(999).empty());
    REQUIRE(ev.find(b)->cause == a);
}

TEST_CASE("a bare name finds the first entity so named in tree order, as the tree changes", "[world][find]") {
    World w;
    const EntityId a = w.spawn("A", 0, Json::object()).value();
    const EntityId b = w.spawn("B", 0, Json::object()).value();
    const EntityId leaf = w.spawn("Leaf", b, Json::object()).value();
    REQUIRE(w.find("Leaf") == leaf);
    REQUIRE(w.find("Leaf") == leaf);   // the second time from the kept answer
    // One under the earlier root comes first in tree order.
    const EntityId earlier = w.spawn("Leaf", a, Json::object()).value();
    REQUIRE(w.find("Leaf") == earlier);
    REQUIRE(w.rename(earlier, "Twig").has_value());
    REQUIRE(w.find("Leaf") == leaf);
    REQUIRE(w.find("Twig") == earlier);
    REQUIRE(w.reparent(leaf, a).has_value());
    REQUIRE(w.find("Leaf") == leaf);
    REQUIRE(w.find("/A/Leaf") == leaf);
    REQUIRE(w.destroy(leaf).has_value());
    REQUIRE(w.find("Leaf") == 0);
    REQUIRE(w.find("Nothing") == 0);
    const EntityId late = w.spawn("Nothing", b, Json::object()).value();
    REQUIRE(w.find("Nothing") == late);   // a miss kept before is forgotten when the tree changes
    w.clear();
    REQUIRE(w.find("Twig") == 0);
}
