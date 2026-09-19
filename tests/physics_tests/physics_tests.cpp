#include <pocket/physics/physics.hpp>

#include <catch_amalgamated.hpp>

using namespace pocket;
using namespace pocket::world;

namespace {

EntityId ground(World& w) {
    return w.spawn("Ground", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", -0.5}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 20}, {"y", 0.5}, {"z", 20}}}}}}).value();
}

EntityId body(World& w, const char* name, int shape, Vec3 pos, float size = 0.5f, Json extra = Json::object()) {
    Json rb = Json{{"kind", 0}};
    for (auto& [k, v] : extra.items()) rb[k] = v;
    return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"RigidBody", rb}, {"Collider", {{"shape", shape}, {"size", {{"x", size}, {"y", size}, {"z", size}}}}}}).value();
}

void run(physics::Physics& p, World& w, int ticks) {
    for (int i = 0; i < ticks; ++i) {
        w.set_tick_index(w.tick_index());
        p.step(w, 1.0 / 60.0);
        w.tick(1.0 / 60.0);
    }
}

}  // namespace

TEST_CASE("a box falls, lands, rests and sleeps", "[physics]") {
    World w;
    physics::Physics p;
    ground(w);
    EntityId box = body(w, "Box", 0, {0, 5, 0}, 0.5f, Json{{"restitution", 0.0}});
    run(p, w, 30);
    const Transform* t = w.try_get<Transform>(box);
    REQUIRE(t->position.y < 5.0f);
    REQUIRE(t->position.y > 0.4f);
    run(p, w, 240);
    t = w.try_get<Transform>(box);
    REQUIRE(t->position.y == Catch::Approx(0.5f).margin(0.02f));
    REQUIRE(w.try_get<RigidBody>(box)->sleeping);
    auto hist = w.events().histogram();
    REQUIRE(hist["collision.begin"].get<int>() >= 1);
    REQUIRE(hist.value("collision.end", 0) < hist["collision.begin"].get<int>());  // still touching
    REQUIRE(p.stats().bodies == 2);
}

TEST_CASE("spheres bounce with restitution and stack on boxes", "[physics]") {
    World w;
    physics::Physics p;
    ground(w);
    EntityId ball = body(w, "Ball", 1, {0, 3, 0}, 0.5f, Json{{"restitution", 0.8}});
    float max_after_bounce = 0;
    bool bounced = false;
    for (int i = 0; i < 240; ++i) {
        run(p, w, 1);
        const Velocity* v = w.try_get<Velocity>(ball);
        if (v->linear.y > 0.5f) bounced = true;
        if (bounced) max_after_bounce = std::max(max_after_bounce, w.try_get<Transform>(ball)->position.y);
    }
    REQUIRE(bounced);
    REQUIRE(max_after_bounce > 1.0f);
    // A box on the ground with a ball on top: the ball rests on the box.
    World w2;
    physics::Physics p2;
    ground(w2);
    body(w2, "Crate", 0, {3, 0.5f, 0});
    EntityId top = body(w2, "Top", 1, {3, 2.5f, 0}, 0.3f, Json{{"restitution", 0.0}});
    run(p2, w2, 300);
    REQUIRE(w2.try_get<Transform>(top)->position.y == Catch::Approx(1.3f).margin(0.05f));
    REQUIRE(w2.try_get<Transform>(top)->position.x == Catch::Approx(3.0f).margin(0.05f));
}

TEST_CASE("triggers report enter and exit without pushing", "[physics]") {
    World w;
    physics::Physics p;
    ground(w);
    EntityId zone = w.spawn("Zone", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 3}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1}, {"y", 0.5}, {"z", 1}}}, {"is_trigger", true}}}}).value();
    EntityId ball = body(w, "Ball", 1, {0, 6, 0}, 0.3f);
    run(p, w, 180);
    auto enters = w.events().since(0, 100, "trigger.enter");
    auto exits = w.events().since(0, 100, "trigger.exit");
    REQUIRE(enters.size() == 1);
    REQUIRE(enters[0].data["b"] == "/Ball");
    REQUIRE(exits.size() == 1);
    REQUIRE(exits[0].cause == enters[0].seq);
    // The trigger did not stop the ball: it rests on the ground, not on the zone.
    REQUIRE(w.try_get<Transform>(ball)->position.y == Catch::Approx(0.3f).margin(0.03f));
    (void)zone;
}

TEST_CASE("kinematic bodies push dynamic ones", "[physics]") {
    World w;
    physics::Physics p;
    ground(w);
    EntityId pusher = w.spawn("Pusher", 0, Json{{"Transform", {{"position", {{"x", -3}, {"y", 0.5}, {"z", 0}}}}}, {"RigidBody", {{"kind", 2}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}, {"Velocity", {{"linear", {{"x", 2}, {"y", 0}, {"z", 0}}}}}}).value();
    EntityId crate = body(w, "Crate", 0, {0, 0.5f, 0});
    run(p, w, 180);
    REQUIRE(w.try_get<Transform>(pusher)->position.x == Catch::Approx(3.0f).margin(0.01f));
    REQUIRE(w.try_get<Transform>(crate)->position.x > 2.5f);
}

TEST_CASE("raycast and overlap find colliders", "[physics]") {
    World w;
    physics::Physics p;
    ground(w);
    EntityId ball = body(w, "Ball", 1, {2, 1, 0}, 0.5f);
    auto hit = p.raycast(w, {2, 5, 0}, {0, -1, 0});
    REQUIRE(hit.has_value());
    REQUIRE(hit->entity == ball);
    REQUIRE(hit->distance == Catch::Approx(3.5f).margin(1e-3f));
    REQUIRE(hit->normal.y == Catch::Approx(1.0f).margin(1e-3f));
    auto g = p.raycast(w, {-5, 5, 0}, {0, -1, 0});
    REQUIRE(g.has_value());
    REQUIRE(w.name(g->entity) == "Ground");
    REQUIRE(g->distance == Catch::Approx(5.0f).margin(1e-3f));
    REQUIRE_FALSE(p.raycast(w, {0, 5, 0}, {0, 1, 0}).has_value());
    auto near = p.overlap_sphere(w, {2, 1.6f, 0}, 0.2f);
    REQUIRE(near == std::vector<EntityId>{ball});
}

TEST_CASE("simulation is deterministic", "[physics]") {
    auto scenario = [](World& w, physics::Physics& p) {
        ground(w);
        for (int i = 0; i < 6; ++i) body(w, i % 2 ? "B" : "C", i % 2, {static_cast<float>(i) * 0.3f - 1, 2.0f + static_cast<float>(i), 0.1f * static_cast<float>(i)}, 0.4f);
        run(p, w, 200);
    };
    World a, b;
    physics::Physics pa, pb;
    scenario(a, pa);
    scenario(b, pb);
    REQUIRE(a.hash() == b.hash());
    REQUIRE(a.events().total() == b.events().total());
}
