#include <pocket/physics/physics.hpp>
#include <pocket/world/water.hpp>

#include <pocket/assets/assets.hpp>

#include <catch_amalgamated.hpp>

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <filesystem>

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

// ---- capsules, rotation locks and joints ----------------------------------------------------

namespace {

EntityId capsule(World& w, const char* name, Vec3 pos, float radius, float half, Quat rot = {}, Json extra = Json::object(), int kind = 0) {
    Json rb = Json{{"kind", kind}};
    for (auto& [k, v] : extra.items()) rb[k] = v;
    return w.spawn(name, 0,
                   Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}, {"rotation", {{"x", rot.x}, {"y", rot.y}, {"z", rot.z}, {"w", rot.w}}}}},
                        {"RigidBody", rb},
                        {"Collider", {{"shape", 2}, {"size", {{"x", radius}, {"y", half}, {"z", radius}}}}}})
        .value();
}

EntityId point(World& w, const char* name, Vec3 pos) {
    return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}}).value();
}

}  // namespace

TEST_CASE("capsules rest on the ground upright and lying", "[physics][capsule]") {
    World w;
    physics::Physics p;
    ground(w);
    EntityId upright = capsule(w, "Upright", {0, 3, 0}, 0.3f, 0.5f, {}, Json{{"lock_rotation", true}, {"restitution", 0.0}});
    Quat lying_rot = Quat::from_axis_angle({0, 0, 1}, kPi / 2);
    EntityId lying = capsule(w, "Lying", {3, 3, 0}, 0.3f, 0.5f, lying_rot, Json{{"restitution", 0.0}});
    run(p, w, 360);
    REQUIRE(w.try_get<Transform>(upright)->position.y == Catch::Approx(0.8f).margin(0.03f));  // half + radius
    REQUIRE(w.try_get<Transform>(lying)->position.y == Catch::Approx(0.3f).margin(0.03f));    // radius
    const Quat& q = w.try_get<Transform>(lying)->rotation;
    REQUIRE(std::fabs(q.x * lying_rot.x + q.y * lying_rot.y + q.z * lying_rot.z + q.w * lying_rot.w) > 0.999f);  // still flat
    REQUIRE(w.try_get<RigidBody>(upright)->sleeping);
    REQUIRE(w.try_get<RigidBody>(lying)->sleeping);
}

TEST_CASE("spheres and capsules rest on capsules", "[physics][capsule]") {
    World w;
    physics::Physics p;
    ground(w);
    Quat along_x = Quat::from_axis_angle({0, 0, 1}, kPi / 2);
    capsule(w, "Log", {0, 1, 0}, 0.3f, 1.0f, along_x, {}, 1);  // static, from x=-1.3 to 1.3
    EntityId ball = body(w, "Ball", 1, {0.5f, 3, 0}, 0.25f, Json{{"restitution", 0.0}});
    Quat along_z = Quat::from_axis_angle({1, 0, 0}, kPi / 2);
    EntityId cross = capsule(w, "Cross", {-0.5f, 3, 0}, 0.2f, 0.5f, along_z, Json{{"lock_rotation", true}, {"restitution", 0.0}});
    run(p, w, 360);
    REQUIRE(w.try_get<Transform>(ball)->position.y == Catch::Approx(1.55f).margin(0.03f));
    REQUIRE(w.try_get<Transform>(cross)->position.y == Catch::Approx(1.5f).margin(0.03f));
    REQUIRE(w.try_get<Transform>(cross)->position.x == Catch::Approx(-0.5f).margin(0.05f));
}

TEST_CASE("lock_rotation keeps a body's orientation", "[physics]") {
    World w;
    physics::Physics p;
    ground(w);
    Quat tilt = Quat::from_axis_angle({0, 0, 1}, 0.4f);
    EntityId box = w.spawn("Box", 0,
                           Json{{"Transform", {{"position", {{"x", 0}, {"y", 3}, {"z", 0}}}, {"rotation", {{"x", tilt.x}, {"y", tilt.y}, {"z", tilt.z}, {"w", tilt.w}}}}},
                                {"RigidBody", {{"kind", 0}, {"lock_rotation", true}, {"restitution", 0.0}}},
                                {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}})
                       .value();
    run(p, w, 300);
    const Transform* t = w.try_get<Transform>(box);
    REQUIRE(t->rotation.z == Catch::Approx(tilt.z).margin(1e-4f));
    REQUIRE(t->rotation.w == Catch::Approx(tilt.w).margin(1e-4f));
    // Resting on its lowest edge, higher than a flat box would.
    REQUIRE(t->position.y == Catch::Approx(0.5f * (std::cos(0.4f) + std::sin(0.4f))).margin(0.03f));
    REQUIRE(w.try_get<Velocity>(box)->angular.z == 0.0f);
}

TEST_CASE("a distance joint swings a pendulum at its rod length", "[physics][joint]") {
    World w;
    physics::Physics p;
    point(w, "Hook", {0, 5, 0});
    EntityId bob = body(w, "Bob", 1, {2, 5, 0}, 0.2f);
    REQUIRE(w.set(bob, "Joint", Json{{"target", "/Hook"}}).has_value());
    float worst = 0, min_y = 5;
    for (int i = 0; i < 180; ++i) {
        run(p, w, 1);
        Vec3 pos = w.try_get<Transform>(bob)->position;
        worst = std::max(worst, std::fabs(length(pos - Vec3{0, 5, 0}) - 2.0f));
        min_y = std::min(min_y, pos.y);
    }
    REQUIRE(worst < 0.05f);
    REQUIRE(min_y < 3.2f);  // swung through the bottom
    REQUIRE(p.stats().joints == 1);
    REQUIRE(p.joints().size() == 1);
    REQUIRE(p.joints()[0].force > 0.0f);
    REQUIRE(p.joints()[0].length == Catch::Approx(2.0f).margin(1e-4f));
    REQUIRE(w.get(bob, "Joint").value()["distance"].get<float>() == Catch::Approx(2.0f).margin(1e-4f));  // auto length written back
    REQUIRE(w.get(bob, "Joint").value()["force"].get<float>() > 0.0f);
}

TEST_CASE("a rope pulls only when taut", "[physics][joint]") {
    World w;
    physics::Physics p;
    ground(w);
    point(w, "Hook", {0, 6, 0});
    EntityId ball = body(w, "Ball", 1, {0, 5, 0}, 0.2f, Json{{"restitution", 0.0}});
    REQUIRE(w.set(ball, "Joint", Json{{"target", "/Hook"}, {"rope", true}, {"distance", 3.0}}).has_value());
    run(p, w, 30);  // half a second of free fall while the rope is slack
    float y = w.try_get<Transform>(ball)->position.y;
    REQUIRE(y < 4.0f);
    REQUIRE(y > 3.5f);
    REQUIRE(p.joints()[0].force == Catch::Approx(0.0f).margin(1e-3f));
    run(p, w, 300);
    REQUIRE(w.try_get<Transform>(ball)->position.y == Catch::Approx(3.0f).margin(0.05f));  // hangs at the rope's length
}

TEST_CASE("a ball joint pins two bodies at their anchors", "[physics][joint]") {
    World w;
    physics::Physics p;
    w.spawn("Post", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 4}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.25}, {"y", 0.25}, {"z", 0.25}}}}}}).value();
    EntityId arm = w.spawn("Arm", 0, Json{{"Transform", {{"position", {{"x", 1.5}, {"y", 4}, {"z", 0}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1.0}, {"y", 0.1}, {"z", 0.1}}}}}}).value();
    REQUIRE(w.set(arm, "Joint", Json{{"kind", 1}, {"target", "/Post"}, {"anchor", {{"x", -1}, {"y", 0}, {"z", 0}}}, {"target_anchor", {{"x", 0.5}, {"y", 0}, {"z", 0}}}}).has_value());
    const Vec3 pin{0.5f, 4, 0};
    float worst = 0, min_y = 4, max_turn = 0;
    for (int i = 0; i < 240; ++i) {
        run(p, w, 1);
        const Transform* t = w.try_get<Transform>(arm);
        Vec3 anchor = t->position + t->rotation.rotate(Vec3{-1, 0, 0});
        worst = std::max(worst, length(anchor - pin));
        min_y = std::min(min_y, t->position.y);
        max_turn = std::max(max_turn, std::fabs(t->rotation.z));
    }
    REQUIRE(worst < 0.05f);
    REQUIRE(min_y < 3.2f);     // the arm swung down around the pin
    REQUIRE(max_turn > 0.5f);  // by rotating, not by sliding
    REQUIRE(p.joints()[0].kind == 1);
}

TEST_CASE("a joint breaks above its break force", "[physics][joint]") {
    World w;
    physics::Physics p;
    ground(w);
    point(w, "Hook", {0, 5, 0});
    EntityId heavy = body(w, "Heavy", 1, {0, 4, 0}, 0.3f, Json{{"mass", 100.0}, {"restitution", 0.0}});
    REQUIRE(w.set(heavy, "Joint", Json{{"target", "/Hook"}, {"distance", 1.0}, {"break_force", 500.0}}).has_value());
    EntityId light = body(w, "Light", 1, {3, 4, 0}, 0.3f, Json{{"restitution", 0.0}});
    REQUIRE(w.set(light, "Joint", Json{{"target", "/Hook"}, {"break_force", 500.0}}).has_value());
    run(p, w, 120);
    REQUIRE_FALSE(w.has(heavy, "Joint"));
    REQUIRE(w.has(light, "Joint"));
    REQUIRE(w.events().histogram()["joint.broken"].get<int>() == 1);
    REQUIRE(w.try_get<Transform>(heavy)->position.y < 1.0f);  // fell once the joint gave
    bool seen = false;
    for (const auto& e : w.events().recent(200)) {
        if (e.type == "joint.broken") {
            seen = true;
            REQUIRE(e.subject == heavy);
            REQUIRE(e.data["path"] == "/Heavy");
        }
    }
    REQUIRE(seen);
}

TEST_CASE("rays and overlaps see capsules", "[physics][capsule]") {
    World w;
    physics::Physics p;
    capsule(w, "Post", {0, 1, 0}, 0.3f, 0.5f, {}, {}, 1);
    auto side = p.raycast(w, {5, 1, 0}, {-1, 0, 0});
    REQUIRE(side.has_value());
    REQUIRE(side->distance == Catch::Approx(4.7f).margin(1e-3f));
    REQUIRE(side->normal.x == Catch::Approx(1.0f).margin(1e-3f));
    auto top = p.raycast(w, {0, 5, 0}, {0, -1, 0});
    REQUIRE(top.has_value());
    REQUIRE(top->distance == Catch::Approx(3.2f).margin(1e-3f));
    REQUIRE(top->normal.y == Catch::Approx(1.0f).margin(1e-3f));
    auto cap = p.raycast(w, {0.2f, 5, 0}, {0, -1, 0});  // through the rounded top, off center
    REQUIRE(cap.has_value());
    REQUIRE(cap->point.y == Catch::Approx(1.5f + std::sqrt(0.09f - 0.04f)).margin(1e-3f));
    REQUIRE_FALSE(p.raycast(w, {0.5f, 5, 0}, {0, -1, 0}).has_value());
    REQUIRE(p.overlap_sphere(w, {0, 2.0f, 0}, 0.3f).size() == 1);
    REQUIRE(p.overlap_sphere(w, {0, 2.5f, 0}, 0.3f).empty());
}

TEST_CASE("a hinge keeps its axis, stops at its limit and reports the angle", "[physics][joint]") {
    World w;
    physics::Physics p;
    // A hatch hinged along its back edge to a world point; gravity swings the free edge down.
    EntityId hatch = w.spawn("Hatch", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 3}, {"z", 0}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1.0}, {"y", 0.05}, {"z", 0.6}}}}}}).value();
    REQUIRE(w.set(hatch, "Joint", Json{{"kind", 2}, {"anchor", {{"x", 0}, {"y", 0}, {"z", -0.6}}}, {"target_anchor", {{"x", 0}, {"y", 3}, {"z", -0.6}}}, {"axis", {{"x", 1}, {"y", 0}, {"z", 0}}}, {"limit", true}, {"lower", 0.0}, {"upper", 1.2}}).has_value());
    float worst = 0, max_angle = 0, min_axis = 1;
    for (int i = 0; i < 240; ++i) {
        run(p, w, 1);
        const Transform* t = w.try_get<Transform>(hatch);
        Vec3 pin = t->position + t->rotation.rotate(Vec3{0, 0, -0.6f});
        worst = std::max(worst, length(pin - Vec3{0, 3, -0.6f}));
        min_axis = std::min(min_axis, t->rotation.rotate(Vec3{1, 0, 0}).x);  // the hinge axis stays world X
        max_angle = std::max(max_angle, p.joints()[0].angle);
    }
    REQUIRE(worst < 0.05f);
    REQUIRE(min_axis > 0.999f);
    REQUIRE(max_angle < 1.25f);
    const physics::JointInfo& j = p.joints()[0];
    REQUIRE(j.kind == 2);
    REQUIRE(j.angle == Catch::Approx(1.2f).margin(0.05f));
    REQUIRE(j.limit_state == 1);
    // The component carries the angle and the frame the engine took at the first step; the stop was an event.
    const Joint* jc = w.try_get<Joint>(hatch);
    REQUIRE(jc->angle == Catch::Approx(1.2f).margin(0.05f));
    REQUIRE(jc->target_axis.x == Catch::Approx(1.0f).margin(1e-4f));
    REQUIRE(length(jc->reference) == Catch::Approx(1.0f).margin(1e-4f));
    bool limited = false;
    for (const auto& e : w.events().recent(500)) limited = limited || e.type == "joint.limit";
    REQUIRE(limited);
}

TEST_CASE("a hinge motor turns a wheel at its speed within its torque", "[physics][joint]") {
    World w;
    physics::Physics p;
    EntityId wheel = w.spawn("Wheel", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 2}, {"z", 0}}}}}, {"RigidBody", {{"kind", 0}, {"gravity_scale", 0.0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.9}, {"y", 0.05}, {"z", 0.15}}}}}}).value();
    REQUIRE(w.set(wheel, "Joint", Json{{"kind", 2}, {"target_anchor", {{"x", 0}, {"y", 2}, {"z", 0}}}, {"axis", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"motor_speed", 3.0}, {"motor_torque", 4.0}}).has_value());
    run(p, w, 60);
    REQUIRE(p.joints()[0].speed == Catch::Approx(3.0f).margin(0.1f));
    const Transform* t = w.try_get<Transform>(wheel);
    REQUIRE(length(t->position - Vec3{0, 2, 0}) < 0.02f);
    REQUIRE(std::fabs(t->rotation.x) < 0.01f);  // turning about Y only
    REQUIRE(std::fabs(t->rotation.z) < 0.01f);
    // Reversing the motor reverses the wheel; a weak motor cannot reach its speed and reports what it applies.
    REQUIRE(w.set(wheel, "Joint", Json{{"motor_speed", -3.0}}).has_value());
    run(p, w, 60);
    REQUIRE(p.joints()[0].speed == Catch::Approx(-3.0f).margin(0.1f));
    REQUIRE(w.set(wheel, "Joint", Json{{"motor_torque", 0.01}, {"motor_speed", 30.0}}).has_value());
    run(p, w, 60);
    REQUIRE(p.joints()[0].speed < 10.0f);
    REQUIRE(p.joints()[0].torque == Catch::Approx(0.01f).margin(0.002f));
}

TEST_CASE("a hinge between two dynamic bodies keeps their axes aligned through a swing", "[physics][joint]") {
    World w;
    physics::Physics p;
    ground(w);
    // A heavy block on the ground with a bar hinged (axis Z) in front of it: the bar swings in the XY plane.
    EntityId block = body(w, "Block", 0, {0, 0.5f, 0}, 0.5f, Json{{"mass", 50.0}});
    EntityId bar = w.spawn("Bar", 0, Json{{"Transform", {{"position", {{"x", 0.4}, {"y", 1.0}, {"z", 0.6}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.4}, {"y", 0.05}, {"z", 0.05}}}}}}).value();
    REQUIRE(w.set(bar, "Joint", Json{{"kind", 2}, {"target", "/Block"}, {"anchor", {{"x", -0.4}, {"y", 0}, {"z", 0}}}, {"target_anchor", {{"x", 0}, {"y", 0.5}, {"z", 0.6}}}, {"axis", {{"x", 0}, {"y", 0}, {"z", 1}}}}).has_value());
    float worst = 0, min_align = 1, min_y = 1;
    for (int i = 0; i < 240; ++i) {
        run(p, w, 1);
        const Transform* tb = w.try_get<Transform>(bar);
        const Transform* tk = w.try_get<Transform>(block);
        Vec3 pa = tb->position + tb->rotation.rotate(Vec3{-0.4f, 0, 0});
        Vec3 pb = tk->position + tk->rotation.rotate(Vec3{0, 0.5f, 0.6f});
        worst = std::max(worst, length(pa - pb));
        min_align = std::min(min_align, dot(tb->rotation.rotate(Vec3{0, 0, 1}), tk->rotation.rotate(Vec3{0, 0, 1})));
        min_y = std::min(min_y, tb->position.y);
    }
    REQUIRE(worst < 0.05f);
    REQUIRE(min_align > 0.999f);
    REQUIRE(min_y < 0.7f);  // it swung down
    REQUIRE(p.joints()[0].target == block);
}

TEST_CASE("mesh colliders: a marble rolls to the bottom of a bowl, rays and overlaps see the triangles", "[physics][mesh]") {
    const char* root = std::getenv("POCKET_ROOT");
    REQUIRE(root != nullptr);
    assets::AssetStore store(std::filesystem::path(root) / "samples" / "physics");
    World w;
    physics::Physics p;
    p.set_assets(&store);
    EntityId bowl = w.spawn("Bowl", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", {{"mesh", "assets/bowl.glb"}}}, {"RigidBody", {{"kind", 1}, {"friction", 0.4}}}, {"Collider", {{"shape", 3}}}}).value();
    // Some linear damping stands in for rolling resistance, or the marble would swing in the bowl for a minute.
    EntityId ball = body(w, "Ball", 1, {1.5f, 3.0f, 0.0f}, 0.25f, Json{{"restitution", 0.1}, {"friction", 0.4}, {"linear_damping", 0.6}});
    run(p, w, 60);
    REQUIRE(p.stats().meshes == 1);
    REQUIRE(p.stats().triangles > 1000);
    // After a second it sits on the slope (the surface is y = r^2 / 4), not under it.
    const Transform* t = w.try_get<Transform>(ball);
    INFO(t->position.x << " " << t->position.y << " " << t->position.z);
    REQUIRE(t->position.y > 0.2f);
    REQUIRE(t->position.y < 1.0f);
    run(p, w, 540);
    t = w.try_get<Transform>(ball);
    INFO(t->position.x << " " << t->position.y << " " << t->position.z);
    REQUIRE(std::hypot(t->position.x, t->position.z) < 0.35f);
    REQUIRE(t->position.y == Catch::Approx(0.25f).margin(0.1f));
    REQUIRE(w.events().histogram()["collision.begin"].get<int>() >= 1);
    // Rays hit the surface where the bowl is: at r = 2 it is at y = 1, facing up and inward.
    auto hit = p.raycast(w, {2, 5, 0}, {0, -1, 0}, 10);
    REQUIRE(hit.has_value());
    REQUIRE(hit->entity == bowl);
    REQUIRE(hit->point.y == Catch::Approx(1.0f).margin(0.06f));
    REQUIRE(hit->normal.y > 0.5f);
    REQUIRE(hit->normal.x < -0.3f);
    REQUIRE(p.raycast(w, {4, 5, 0}, {0, -1, 0}, 10).has_value() == false);  // beyond the rim
    auto over = p.overlap_sphere(w, {2, 1.1f, 0}, 0.3f);
    REQUIRE(std::find(over.begin(), over.end(), bowl) != over.end());
    REQUIRE(p.overlap_sphere(w, {2, 3, 0}, 0.3f).empty());
    // A box dropped on the slope slides down too and rests on the triangles.
    EntityId box = body(w, "Box", 0, {-1.2f, 3.0f, 0.8f}, 0.3f, Json{{"restitution", 0.0}, {"friction", 0.2}, {"linear_damping", 0.3}});
    run(p, w, 600);
    t = w.try_get<Transform>(box);
    INFO("box " << t->position.x << " " << t->position.y << " " << t->position.z);
    REQUIRE(t->position.y > 0.2f);
    REQUIRE(t->position.y < 1.2f);
    REQUIRE(std::hypot(t->position.x, t->position.z) < 1.5f);
    // A missing mesh file collides with nothing and is not counted.
    w.spawn("Ghost", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 0}, {"z", 5}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 3}, {"mesh", "assets/nope.glb"}}}});
    run(p, w, 1);
    REQUIRE(p.stats().meshes == 1);
}

TEST_CASE("a spring stretches under its load, swings and settles at its weight over its stiffness", "[physics][joint][spring]") {
    World w;
    physics::Physics p;
    EntityId bob = body(w, "Bob", 1, {0, 4, 0}, 0.2f, Json{{"mass", 1.0}});
    REQUIRE(w.set(bob, "Joint", Json{{"kind", 0}, {"target_anchor", {{"x", 0}, {"y", 5}, {"z", 0}}}, {"distance", 1.0}, {"stiffness", 50.0}, {"damping", 2.0}}).has_value());
    float lowest = 10, highest = 0;
    for (int i = 0; i < 120; ++i) {
        run(p, w, 1);
        float y = w.try_get<Transform>(bob)->position.y;
        lowest = std::min(lowest, y);
        highest = std::max(highest, y);
    }
    // Let go at the rest length it swings past the equilibrium (m g / k = 0.196 below) and back up.
    INFO("lowest " << lowest << " highest " << highest);
    REQUIRE(lowest < 4.0f - 0.25f);
    REQUIRE(highest > 4.0f - 0.15f);
    run(p, w, 600);
    const Transform* t = w.try_get<Transform>(bob);
    REQUIRE(t->position.y == Catch::Approx(4.0f - 9.81f / 50.0f).margin(0.02f));
    REQUIRE(p.joints()[0].force == Catch::Approx(9.81f).margin(0.5f));
    REQUIRE(w.try_get<Joint>(bob)->force == Catch::Approx(9.81f).margin(0.5f));
    // A bungee (rope with stiffness) never pushes: started above its rest length it falls freely.
    EntityId lifted = body(w, "Lifted", 1, {3, 4.5f, 0}, 0.2f, Json{{"mass", 1.0}});
    REQUIRE(w.set(lifted, "Joint", Json{{"kind", 0}, {"rope", true}, {"target_anchor", {{"x", 3}, {"y", 5}, {"z", 0}}}, {"distance", 1.0}, {"stiffness", 50.0}, {"damping", 2.0}}).has_value());
    run(p, w, 6);
    REQUIRE(w.try_get<Velocity>(lifted)->linear.y == Catch::Approx(-9.81f * 0.1f).margin(0.05f));
}

TEST_CASE("a slider moves along its axis only, its motor lifts it to the stop and it reports the travel", "[physics][joint][slider]") {
    World w;
    physics::Physics p;
    EntityId lift = w.spawn("Lift", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 1}, {"z", 0}}}}}, {"RigidBody", {{"kind", 0}, {"mass", 2.0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.6}, {"y", 0.1}, {"z", 0.6}}}}}}).value();
    REQUIRE(w.set(lift, "Joint", Json{{"kind", 3}, {"target_anchor", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"axis", {{"x", 0}, {"y", 1}, {"z", 0}}}, {"limit", true}, {"lower", 0.0}, {"upper", 2.0}, {"motor_speed", 1.0}, {"motor_force", 100.0}}).has_value());
    // A crate sits on the lift and rides up with it.
    EntityId crate = body(w, "Crate", 0, {0.2f, 1.4f, 0}, 0.3f, Json{{"mass", 1.0}});
    run(p, w, 60);
    const Transform* t = w.try_get<Transform>(lift);
    INFO(t->position.x << " " << t->position.y << " " << t->position.z);
    REQUIRE(t->position.y == Catch::Approx(2.0f).margin(0.15f));  // one meter per second, a second in
    REQUIRE(std::fabs(t->position.x) < 0.01f);
    REQUIRE(std::fabs(t->position.z) < 0.01f);
    REQUIRE(t->rotation.w > 0.9999f);
    run(p, w, 120);
    t = w.try_get<Transform>(lift);
    REQUIRE(t->position.y == Catch::Approx(3.0f).margin(0.05f));  // stopped at the upper limit
    const physics::JointInfo& j = p.joints()[0];
    REQUIRE(j.kind == 3);
    REQUIRE(j.translation == Catch::Approx(2.0f).margin(0.05f));
    REQUIRE(j.limit_state == 1);
    REQUIRE(w.try_get<Joint>(lift)->translation == Catch::Approx(2.0f).margin(0.05f));
    REQUIRE(w.try_get<Transform>(crate)->position.y > 3.2f);  // rode up on the platform
    REQUIRE(w.events().histogram()["joint.limit"].get<int>() >= 1);
    // Reverse the motor: down to the lower stop; with the motor off the stop still holds it.
    REQUIRE(w.set(lift, "Joint", Json{{"motor_speed", -1.0}}).has_value());
    run(p, w, 180);
    t = w.try_get<Transform>(lift);
    INFO(t->position.x << " " << t->position.y << " " << t->position.z);
    REQUIRE(t->position.y == Catch::Approx(1.0f).margin(0.05f));
    REQUIRE(p.joints()[0].limit_state == -1);
    REQUIRE(w.set(lift, "Joint", Json{{"motor_force", 0.0}}).has_value());
    run(p, w, 60);
    REQUIRE(w.try_get<Transform>(lift)->position.y == Catch::Approx(1.0f).margin(0.05f));
    REQUIRE(std::fabs(w.try_get<Transform>(lift)->position.x) < 0.01f);
}

TEST_CASE("collision layers decide which pairs collide, trigger and answer queries", "[physics][layers]") {
    World w;
    physics::Physics p;
    ground(w);  // layer 1, mask all
    // Two boxes stacked in the same column: the upper one on layer 2 with a mask that leaves out
    // layer 2... and the lower one on layer 2 as well, so they pass through each other and both
    // land on the ground.
    EntityId lower = w.spawn("Lower", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 1}, {"z", 0}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}, {"layer", 2}, {"mask", 1}}}}).value();
    EntityId upper = w.spawn("Upper", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 3}, {"z", 0}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}, {"layer", 2}, {"mask", 1}}}}).value();
    run(p, w, 240);
    REQUIRE(w.try_get<Transform>(lower)->position.y == Catch::Approx(0.5f).margin(0.03f));
    REQUIRE(w.try_get<Transform>(upper)->position.y == Catch::Approx(0.5f).margin(0.03f));  // inside the lower one, not on it
    // A third box on layer 1 lands on top of the pile: it collides with the layer-2 boxes (its mask
    // is all) and they collide with it (their mask includes layer 1).
    EntityId top = body(w, "Top", 0, {0, 3, 0}, 0.5f);
    run(p, w, 240);
    REQUIRE(w.try_get<Transform>(top)->position.y == Catch::Approx(1.5f).margin(0.05f));
    // Queries take a mask: a ray with mask 1 passes the layer-2 boxes and hits the ground.
    auto any = p.raycast(w, {0, 5, 0}, {0, -1, 0}, 10, physics::Physics::Filter{[](EntityId, const RigidBody&, const Collider& c) { return (c.layer & 0xFFFFFFFFu) != 0; }});
    REQUIRE(any.has_value());
    REQUIRE(any->entity == top);
    auto only1 = p.raycast(w, {0.7f, 5, 0}, {0, -1, 0}, 10, physics::Physics::Filter{[](EntityId, const RigidBody&, const Collider& c) { return (c.layer & 1u) != 0; }});
    REQUIRE(only1.has_value());
    REQUIRE(only1->point.y == Catch::Approx(0.0f).margin(0.01f));  // the ground, through the layer-2 boxes
    auto over = p.overlap_sphere(w, {0, 0.5f, 0}, 0.2f, physics::Physics::Filter{[](EntityId, const RigidBody&, const Collider& c) { return (c.layer & 2u) != 0; }});
    REQUIRE(over.size() == 2);
    // A trigger on layer 4 with mask 2 only notices layer-2 bodies.
    w.spawn("Gate", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 0.5}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1}, {"y", 1}, {"z", 1}}}, {"is_trigger", true}, {"layer", 4}, {"mask", 2}}}});
    body(w, "Visitor1", 1, {5, 0.5f, 0}, 0.3f);                                     // layer 1: ignored by the gate
    w.spawn("Visitor2", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 0.5}, {"z", 0.5}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 1}, {"size", {{"x", 0.3}, {"y", 0.3}, {"z", 0.3}}}, {"layer", 2}}}});
    run(p, w, 5);
    auto enters = w.events().since(0, 1000, "trigger.enter");
    REQUIRE(enters.size() == 1);
    REQUIRE(w.name(enters[0].subject) == "Gate");
}

TEST_CASE("continuous collision holds a fast small sphere at a thin wall that a discrete one crosses", "[physics][ccd]") {
    World w;
    physics::Physics p;
    ground(w);
    // A pane 4 cm thick and 5 cm pellets fired at 80 m/s from 3 m away: 1.33 m per step.
    w.spawn("Pane", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 1}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.02}, {"y", 1}, {"z", 2}}}}}});
    auto pellet = [&](const char* name, float z, bool ccd, double restitution) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", 2}, {"y", 1}, {"z", z}}}}}, {"RigidBody", {{"kind", 0}, {"ccd", ccd}, {"gravity_scale", 0.0}, {"restitution", restitution}}}, {"Collider", {{"shape", 1}, {"size", {{"x", 0.05}, {"y", 0.05}, {"z", 0.05}}}}}, {"Velocity", {{"linear", {{"x", 80}, {"y", 0}, {"z", 0}}}}}}).value();
    };
    EntityId held = pellet("Held", 0.5f, true, 0.0);
    EntityId dud = pellet("Dud", -0.5f, false, 0.0);
    EntityId bouncy = pellet("Bouncy", 1.5f, true, 0.9);
    auto x = [&](EntityId id) { return w.try_get<Transform>(id)->position.x; };
    run(p, w, 30);
    INFO("held " << x(held) << " dud " << x(dud) << " bouncy " << x(bouncy));
    REQUIRE(x(held) < 4.98f);       // stopped a skin short of the pane's near face
    REQUIRE(x(held) > 4.85f);
    REQUIRE(x(dud) > 6.0f);         // crossed the pane between two steps
    REQUIRE(x(bouncy) < 2.0f);      // reflected by its restitution and gone back the way it came
    auto hits = w.events().since(0, 100, "physics.ccd");
    REQUIRE(hits.size() == 2);
    REQUIRE(hits[0].subject == held);
    REQUIRE(hits[0].data["other"] == "/Pane");
    REQUIRE(hits[0].data["normal"]["x"].get<double>() == Catch::Approx(-1.0));
    REQUIRE(hits[0].data["speed"].get<double>() == Catch::Approx(80.0).margin(0.1));  // a step of damping off 80
    // A slow body with ccd is not swept (the discrete step is enough) and settles like any other.
    EntityId slow = w.spawn("Slow", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 2}, {"z", 3}}}}}, {"RigidBody", {{"kind", 0}, {"ccd", true}}}, {"Collider", {{"shape", 1}, {"size", {{"x", 0.3}, {"y", 0.3}, {"z", 0.3}}}}}}).value();
    run(p, w, 240);
    REQUIRE(w.try_get<Transform>(slow)->position.y == Catch::Approx(0.3f).margin(0.03f));
    REQUIRE(w.events().since(0, 100, "physics.ccd").size() == 2);
}

TEST_CASE("continuous collision sweeps a pellet into a spinning bar that turns into its path", "[physics][ccd][spin][otherspin]") {
    World w;
    physics::Physics p;
    ground(w);
    // A bar four meters long lies along x at height 3, spinning about z at -20 radians a second
    // (its +x end sweeps down); a pellet a meter and a half out flies up at 24 m/s from 0.7 below
    // the bar's line. In one tick the pellet rises 0.4 and the bar's line there drops 0.52: they
    // cross mid-tick, where the bar was not at the start of the tick and the pellet is not at its
    // end. Without the bar's turn in the sweep the pellet passes through it.
    auto bar = [&](const char* name, float z) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 3}, {"z", z}}}}}, {"RigidBody", {{"kind", 0}, {"mass", 50.0}, {"gravity_scale", 0.0}, {"restitution", 0.0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 2.0}, {"y", 0.1}, {"z", 0.1}}}}}, {"Velocity", {{"angular", {{"x", 0}, {"y", 0}, {"z", -20.0}}}}}}).value();
    };
    auto pellet = [&](const char* name, float z, bool ccd) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", 1.5}, {"y", 2.3}, {"z", z}}}}}, {"RigidBody", {{"kind", 0}, {"ccd", ccd}, {"mass", 0.01}, {"gravity_scale", 0.0}, {"restitution", 0.0}}}, {"Collider", {{"shape", 1}, {"size", {{"x", 0.05}, {"y", 0.05}, {"z", 0.05}}}}}, {"Velocity", {{"linear", {{"x", 0}, {"y", 24.0}, {"z", 0}}}}}}).value();
    };
    const EntityId blade = bar("Blade", 0.0f);
    const EntityId swept = pellet("Swept", 0.0f, true);
    (void)bar("Dud", 6.0f);
    const EntityId dud = pellet("Dud pellet", 6.0f, false);
    run(p, w, 1);
    INFO("swept " << w.try_get<Transform>(swept)->position.y << " v " << w.try_get<Velocity>(swept)->linear.y << " dud " << w.try_get<Transform>(dud)->position.y << " blade spin " << w.try_get<Velocity>(blade)->angular.z);
    // The unswept pellet went through the bar; the swept one met it and was knocked back down.
    REQUIRE(w.try_get<Transform>(dud)->position.y == Catch::Approx(2.7).margin(0.02));
    REQUIRE(w.try_get<Velocity>(swept)->linear.y < 0.0f);
    REQUIRE(w.try_get<Transform>(swept)->position.y < 2.65f);
    auto hits = w.events().since(0, 100, "physics.ccd");
    Json hit(nullptr);
    for (const auto& h : hits) if (h.subject == swept) { hit = h.data; break; }
    REQUIRE(!hit.is_null());
    REQUIRE(hit["dynamic"] == true);
    REQUIRE(hit["exact"] == false);   // sampled: the other's turn has no exact cast
    REQUIRE(hit["fraction"].get<double>() > 0.3);
    REQUIRE(hit["fraction"].get<double>() < 0.95);
    // The bar turned as far as the impact, not the whole tick, and kept most of its spin.
    const Quat q = w.try_get<Transform>(blade)->rotation;
    const float turned = 2.0f * std::atan2(std::fabs(q.z), q.w);
    REQUIRE(turned > 0.05f);
    REQUIRE(turned < 0.33f);
    REQUIRE(std::fabs(w.try_get<Velocity>(blade)->angular.z) > 15.0f);
}

TEST_CASE("continuous collision sweeps a spinning plank's tip into a pane its center never nears", "[physics][ccd][spin]") {
    World w;
    physics::Physics p;
    ground(w);
    // The pane: 4 cm thick at x 5. Two planks two meters long lie along z with their centers at
    // x 4.2, not moving, spinning at 40 radians a second so a tip swings 0.67 m a tick: within one
    // tick it crosses the pane's x, which the plank's translation (none) would never notice.
    w.spawn("Pane", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 1}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.02}, {"y", 1}, {"z", 4}}}}}});
    auto plank = [&](const char* name, float z, bool ccd) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", 4.2}, {"y", 1}, {"z", z}}}, {"rotation", {{"x", 0}, {"y", 0.7071068}, {"z", 0}, {"w", 0.7071068}}}}}, {"RigidBody", {{"kind", 0}, {"ccd", ccd}, {"gravity_scale", 0.0}, {"restitution", 0.0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1.0}, {"y", 0.05}, {"z", 0.05}}}}}, {"Velocity", {{"angular", {{"x", 0}, {"y", -40.0}, {"z", 0}}}}}}).value();
    };
    const EntityId blade = plank("Blade", 0.0f, true);
    const EntityId dud = plank("Dud", 6.0f, false);   // beyond the pane's end
    auto tip_x = [&](EntityId id) {
        const auto* t = w.try_get<Transform>(id);
        return t->position.x + t->rotation.rotate({1, 0, 0}).x;
    };
    float blade_max = 0, dud_max = 0;
    for (int i = 0; i < 30; ++i) {
        run(p, w, 1);
        blade_max = std::max(blade_max, tip_x(blade));
        dud_max = std::max(dud_max, tip_x(dud));
    }
    INFO("blade tip " << blade_max << " dud tip " << dud_max << " blade spin " << w.try_get<Velocity>(blade)->angular.y);
    // The dud's tip swung through the pane's x; the blade's tip never reached the pane's near face.
    REQUIRE(dud_max > 5.05f);
    REQUIRE(blade_max < 5.0f);
    REQUIRE(blade_max > 4.8f);   // it did get close: the sweep stopped it at the pane, not at the start
    auto hits = w.events().since(0, 100, "physics.ccd");
    Json hit(nullptr);
    for (const auto& h : hits) if (h.subject == blade) { hit = h.data; break; }
    REQUIRE(!hit.is_null());
    REQUIRE(hit["exact"] == false);
    REQUIRE(hit["normal"]["x"].get<double>() == Catch::Approx(-1.0).margin(0.05));
    REQUIRE(hit["point"]["x"].get<double>() == Catch::Approx(4.98).margin(0.05));
    // The impulse at the tip both slowed the spin and pushed the blade's center back from the pane
    // (a tip hit turns and shoves a free body, as it should): the spin lost at least a third and
    // the blade recoiled along -x.
    REQUIRE(std::fabs(w.try_get<Velocity>(blade)->angular.y) < 30.0f);
    REQUIRE(w.try_get<Velocity>(blade)->linear.x < -3.0f);
    REQUIRE(w.try_get<Transform>(blade)->position.x < 4.2f);
}

TEST_CASE("continuous collision casts exact shapes, sweeps boxes and capsules by samples and stops dynamic pairs", "[physics][ccd][sweep]") {
    World w;
    physics::Physics p;
    ground(w);
    // The pane: 4 cm thick at x 5, from y 0 to 2 and z -2 to 2.
    w.spawn("Pane", 0, Json{{"Transform", {{"position", {{"x", 5}, {"y", 1}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.02}, {"y", 1}, {"z", 2}}}}}});
    auto fire = [&](const char* name, Vec3 pos, Json collider, Vec3 vel) {
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"RigidBody", {{"kind", 0}, {"ccd", true}, {"gravity_scale", 0.0}, {"restitution", 0.0}}}, {"Collider", collider}, {"Velocity", {{"linear", {{"x", vel.x}, {"y", vel.y}, {"z", vel.z}}}}}}).value();
    };
    const Json sphere{{"shape", 1}, {"size", {{"x", 0.05}, {"y", 0.05}, {"z", 0.05}}}};
    // A pellet skimming the pane's top edge: its center passes 3 cm above the top face, where a ray
    // from the center misses the pane; the exact cast meets the edge, rounded by the radius.
    EntityId edge = fire("Edge", {2, 2.03f, 0.5f}, sphere, {80, 0, 0});
    // A box and a capsule pellet, swept by samples and bisected to the impact.
    EntityId box = fire("Box", {2, 1, -0.5f}, Json{{"shape", 0}, {"size", {{"x", 0.05}, {"y", 0.05}, {"z", 0.05}}}}, {80, 0, 0});
    EntityId cap = fire("Cap", {2, 1, -1.5f}, Json{{"shape", 2}, {"size", {{"x", 0.05}, {"y", 0.1}, {"z", 0.05}}}}, {80, 0, 0});
    // Two pellets fired at each other past the pane's end, 160 m/s between them: both dynamic, both stop.
    EntityId a = fire("A", {2, 1, 3.0f}, sphere, {80, 0, 0});
    EntityId b = fire("B", {8, 1, 3.0f}, sphere, {-80, 0, 0});
    auto pos = [&](EntityId id) { return w.try_get<Transform>(id)->position; };
    std::uint32_t dynamic_hits = 0;
    for (int i = 0; i < 30; ++i) {
        run(p, w, 1);
        dynamic_hits += p.stats().ccd_dynamic;
    }
    INFO("edge " << pos(edge).x << "," << pos(edge).y << " box " << pos(box).x << " cap " << pos(cap).x << " a " << pos(a).x << " b " << pos(b).x);
    REQUIRE(pos(box).x < 4.98f);
    REQUIRE(pos(box).x > 4.85f);
    REQUIRE(pos(cap).x < 4.98f);
    REQUIRE(pos(cap).x > 4.85f);
    // The edge pellet was deflected up over the pane by the edge's normal, not passed through nor stopped flat.
    auto hits = w.events().since(0, 100, "physics.ccd");
    auto find = [&](EntityId id) { for (const auto& h : hits) if (h.subject == id) return h.data; return Json(nullptr); };
    Json e = find(edge);
    REQUIRE(!e.is_null());
    REQUIRE(e["exact"] == true);
    REQUIRE(e["normal"]["y"].get<double>() > 0.5);
    REQUIRE(e["normal"]["x"].get<double>() < -0.5);
    REQUIRE(pos(edge).y > 2.1f);
    REQUIRE(find(box)["exact"] == false);
    REQUIRE(find(box)["normal"]["x"].get<double>() == Catch::Approx(-1.0).margin(0.01));
    REQUIRE(find(cap)["normal"]["x"].get<double>() == Catch::Approx(-1.0).margin(0.01));
    // The pair met a skin apart and lost their speed into each other (no restitution).
    REQUIRE(dynamic_hits >= 1);
    Json pair = find(a).is_null() ? find(b) : find(a);
    REQUIRE(!pair.is_null());
    REQUIRE(pair["dynamic"] == true);
    REQUIRE(pos(a).x < 5.0f);
    REQUIRE(pos(b).x > 5.0f);
    REQUIRE(pos(b).x - pos(a).x == Catch::Approx(0.1).margin(0.02));
    REQUIRE(std::fabs(w.try_get<Velocity>(a)->linear.x) < 1.0f);
    REQUIRE(std::fabs(w.try_get<Velocity>(b)->linear.x) < 1.0f);
    // The sweep query: a sphere cast to the pane's face, its edge (rounded), and from above onto its edge.
    auto all = [](EntityId, const RigidBody&, const Collider&) { return true; };
    auto face = p.sweep(w, {2, 1, 0}, {1, 0, 0}, 0.05f, 10.0f, all);
    REQUIRE(face.has_value());
    REQUIRE(face->distance == Catch::Approx(2.93).margin(1e-3));
    REQUIRE(face->point.x == Catch::Approx(4.98).margin(1e-3));
    REQUIRE(face->normal.x == Catch::Approx(-1.0));
    auto rim = p.sweep(w, {2, 2.03f, 0}, {1, 0, 0}, 0.05f, 10.0f, all);
    REQUIRE(rim.has_value());
    REQUIRE(rim->distance == Catch::Approx(2.94).margin(2e-3));
    REQUIRE(rim->normal.y == Catch::Approx(0.6).margin(0.02));
    auto down = p.sweep(w, {4.95f, 5, 0}, {0, -1, 0}, 0.5f, 10.0f, all);
    REQUIRE(down.has_value());
    REQUIRE(down->distance == Catch::Approx(2.501).margin(2e-3));   // the corner is rounded: a little past the flat 2.5
    REQUIRE(p.sweep(w, {2, 4, 0}, {1, 0, 0}, 0.05f, 10.0f, all).has_value() == false);   // over the pane, nothing (the ground is below)
    REQUIRE(p.sweep(w, {2, 1, 0}, {1, 0, 0}, 0.05f, 2.0f, all).has_value() == false);    // out of range
}

TEST_CASE("groups, exceptions and joints keep chosen pairs apart", "[physics][groups]") {
    World w;
    physics::Physics p;
    ground(w);
    auto sphere = [&](const char* name, Vec3 pos, Json collider) {
        Json col = Json{{"shape", 1}, {"size", {{"x", 0.3}, {"y", 0.3}, {"z", 0.3}}}};
        for (auto& [k, v] : collider.items()) col[k] = v;
        return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", col}}).value();
    };
    auto y = [&](EntityId id) { return w.try_get<Transform>(id)->position.y; };
    // The same negative group: dropped onto each other, they pass through and both rest on the ground.
    EntityId n1 = sphere("N1", {0, 1, 0}, Json{{"group", -2}});
    EntityId n2 = sphere("N2", {0, 3, 0}, Json{{"group", -2}});
    // The same positive group: layers that would keep them apart are overruled and the upper rests on the lower.
    EntityId p1 = sphere("P1", {3, 1, 0}, Json{{"group", 3}, {"layer", 2}, {"mask", 1}});
    EntityId p2 = sphere("P2", {3, 3, 0}, Json{{"group", 3}, {"layer", 2}, {"mask", 1}});
    // The same layers without a group pass through (the control).
    EntityId c1 = sphere("C1", {6, 1, 0}, Json{{"layer", 2}, {"mask", 1}});
    EntityId c2 = sphere("C2", {6, 3, 0}, Json{{"layer", 2}, {"mask", 1}});
    bool kept_apart = false;  // pairs the rules skipped while the bodies still moved through each other
    for (int i = 0; i < 240; ++i) {
        run(p, w, 1);
        kept_apart = kept_apart || p.stats().ignored > 0;
    }
    INFO("n " << y(n1) << "," << y(n2) << " p " << y(p1) << "," << y(p2) << " c " << y(c1) << "," << y(c2));
    REQUIRE(y(n1) == Catch::Approx(0.3f).margin(0.03f));
    REQUIRE(y(n2) == Catch::Approx(0.3f).margin(0.03f));
    REQUIRE(y(p1) == Catch::Approx(0.3f).margin(0.03f));
    REQUIRE(y(p2) == Catch::Approx(0.9f).margin(0.05f));
    REQUIRE(y(c2) == Catch::Approx(0.3f).margin(0.03f));
    REQUIRE(kept_apart);
    // An exception: two boxes stacked in the same column pass through while it stands, and stack again once lifted.
    EntityId b1 = body(w, "B1", 0, {-3, 0.5f, 0}, 0.5f);
    EntityId b2 = body(w, "B2", 0, {-3, 3, 0}, 0.5f);
    p.ignore(b1, b2);
    REQUIRE(p.ignored().size() == 1);
    run(p, w, 240);
    REQUIRE(y(b2) == Catch::Approx(0.5f).margin(0.03f));
    p.ignore(b1, b2, false);
    REQUIRE(p.ignored().empty());
    REQUIRE(w.set(b2, "Transform", Json{{"position", {{"x", -3}, {"y", 3}, {"z", 0}}}}).has_value());
    REQUIRE(w.set(b2, "RigidBody", Json{{"sleeping", false}}).has_value());
    run(p, w, 240);
    REQUIRE(y(b2) == Catch::Approx(1.5f).margin(0.05f));
    // An exception dies with one of its bodies.
    p.ignore(b1, b2);
    REQUIRE(w.destroy(b2).has_value());
    run(p, w, 1);
    REQUIRE(p.ignored().empty());
    // A joint whose bodies do not collide: a box hanging by a rod inside a fixed one stays there
    // without a contact; the same box with collide_connected is pushed by the contact.
    EntityId post = w.spawn("Post", 0, Json{{"Transform", {{"position", {{"x", 8}, {"y", 3}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}}).value();
    EntityId sunk = w.spawn("Sunk", 0, Json{{"Transform", {{"position", {{"x", 8}, {"y", 2.4}, {"z", 0}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}, {"Joint", {{"kind", 0}, {"target", "Post"}, {"distance", -1}, {"collide_connected", false}}}}).value();
    EntityId post2 = w.spawn("Post2", 0, Json{{"Transform", {{"position", {{"x", 8}, {"y", 3}, {"z", 3}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}}).value();
    EntityId kept = w.spawn("Kept", 0, Json{{"Transform", {{"position", {{"x", 8}, {"y", 2.4}, {"z", 3}}}}}, {"RigidBody", {{"kind", 0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 0.5}, {"y", 0.5}, {"z", 0.5}}}}}, {"Joint", {{"kind", 0}, {"target", "Post2"}, {"distance", -1}}}}).value();
    bool sunk_contact = false, kept_contact = false;
    for (int i = 0; i < 120; ++i) {
        run(p, w, 1);
        for (const physics::Contact& c : p.contacts()) {
            if ((c.a == sunk && c.b == post) || (c.a == post && c.b == sunk)) sunk_contact = true;
            if ((c.a == kept && c.b == post2) || (c.a == post2 && c.b == kept)) kept_contact = true;
        }
    }
    INFO("sunk " << y(sunk) << " kept " << y(kept));
    REQUIRE_FALSE(sunk_contact);
    REQUIRE(kept_contact);
    REQUIRE(y(sunk) == Catch::Approx(2.4f).margin(0.05f));  // hanging on its rod, inside the post
}


namespace {

EntityId character(World& w, const char* name, Vec3 pos, Json extra = Json::object()) {
    Json c = Json::object();
    for (auto& [k, v] : extra.items()) c[k] = v;
    return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"Character", c}}).value();
}

EntityId solid(World& w, const char* name, Vec3 pos, Vec3 half, Quat rot = Quat{}, int kind = 1) {
    return w.spawn(name, 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}, {"rotation", {{"x", rot.x}, {"y", rot.y}, {"z", rot.z}, {"w", rot.w}}}}}, {"RigidBody", {{"kind", kind}}}, {"Collider", {{"shape", 0}, {"size", {{"x", half.x}, {"y", half.y}, {"z", half.z}}}}}}).value();
}

void walk(physics::Physics& p, World& w, EntityId who, Vec3 velocity, int ticks, int* airborne = nullptr) {
    for (int i = 0; i < ticks; ++i) {
        Character c = *w.try_get<Character>(who);
        c.velocity.x = velocity.x;
        c.velocity.z = velocity.z;
        if (velocity.y != 0) c.velocity.y = velocity.y;
        w.ecs().entity(who).set(c);
        w.set_tick_index(w.tick_index());
        p.step(w, 1.0 / 60.0);
        p.move_characters(w, 1.0 / 60.0);
        w.tick(1.0 / 60.0);
        if (airborne && !w.try_get<Character>(who)->grounded) ++*airborne;
    }
}

}  // namespace

TEST_CASE("a character lands, walks, slides along walls and jumps", "[physics][character]") {
    World w;
    physics::Physics p;
    ground(w);
    const EntityId hero = character(w, "Hero", {0, 3, 0});
    walk(p, w, hero, {}, 90);
    const Character* c = w.try_get<Character>(hero);
    REQUIRE(c->grounded);
    REQUIRE(c->velocity.y == 0.0f);
    REQUIRE(w.try_get<Transform>(hero)->position.y == Catch::Approx(0.9).margin(0.02));   // centred, half its height above the floor
    REQUIRE(w.events().histogram()["character.landed"] == 1);
    REQUIRE(p.stats().characters == 1);
    // Standing still stays put: no creep, no sinking.
    walk(p, w, hero, {}, 120);
    REQUIRE(w.try_get<Transform>(hero)->position.x == Catch::Approx(0).margin(1e-4));
    REQUIRE(w.try_get<Transform>(hero)->position.y == Catch::Approx(0.9).margin(0.02));
    // Into a wall along x: stopped at the wall; along it diagonally: slides on in z.
    solid(w, "Wall", {3, 1, 0}, {0.5f, 1, 5});
    walk(p, w, hero, {4, 0, 0}, 60);
    const Transform* t = w.try_get<Transform>(hero);
    REQUIRE(t->position.x == Catch::Approx(2.5 - 0.3).margin(0.03));
    REQUIRE(w.try_get<Character>(hero)->on_wall);
    REQUIRE(w.try_get<Character>(hero)->wall_normal.x == Catch::Approx(-1).margin(0.01));
    REQUIRE(w.try_get<Character>(hero)->velocity.x == Catch::Approx(0).margin(1e-4));   // the part into the wall is gone
    walk(p, w, hero, {4, 0, 4}, 30);
    t = w.try_get<Transform>(hero);
    REQUIRE(t->position.x == Catch::Approx(2.2).margin(0.03));
    REQUIRE(t->position.z == Catch::Approx(2.0).margin(0.05));   // half a second at 4 along the wall
    // A jump leaves the ground and comes back down onto it.
    Character jump = *w.try_get<Character>(hero);
    jump.velocity = {0, 6, 0};
    w.ecs().entity(hero).set(jump);
    int airborne = 0;
    walk(p, w, hero, {}, 60, &airborne);
    REQUIRE(airborne > 30);   // 6 up against 20 down: 0.6 s in the air
    REQUIRE(w.try_get<Character>(hero)->grounded);
    REQUIRE(w.events().histogram()["character.landed"] == 2);
}

TEST_CASE("a character walks up steps and slopes it can climb and not up those it cannot", "[physics][character]") {
    World w;
    physics::Physics p;
    ground(w);
    // A kerb 0.25 high (under the 0.3 step) and a ledge 0.6 high across the path in z = 0..
    solid(w, "Kerb", {2, 0.125f, 0}, {0.5f, 0.125f, 1});
    solid(w, "Ledge", {2, 0.3f, 4}, {0.5f, 0.3f, 1});
    const EntityId up = character(w, "Up", {0, 0.9f, 0});
    const EntityId stuck = character(w, "Stuck", {0, 0.9f, 4});
    walk(p, w, up, {2, 0, 0}, 1);
    walk(p, w, stuck, {2, 0, 0}, 1);
    for (int i = 0; i < 60; ++i) {
        walk(p, w, up, {2, 0, 0}, 1);
        walk(p, w, stuck, {2, 0, 0}, 0);
        Character s = *w.try_get<Character>(stuck);
        s.velocity.x = 2;
        w.ecs().entity(stuck).set(s);
    }
    INFO("up at " << w.try_get<Transform>(up)->position.x << ", " << w.try_get<Transform>(up)->position.y);
    REQUIRE(w.try_get<Transform>(up)->position.y == Catch::Approx(0.25 + 0.9).margin(0.03));   // on the kerb
    REQUIRE(w.try_get<Transform>(up)->position.x > 1.8f);
    REQUIRE(p.stats().characters == 2);
    REQUIRE(w.try_get<Transform>(stuck)->position.x < 1.5f - 0.3f + 0.05f);   // at the ledge's face
    REQUIRE(w.try_get<Transform>(stuck)->position.y == Catch::Approx(0.9).margin(0.03));

    // Ramps: 30 degrees is walked up, 60 degrees is a wall.
    World r;
    physics::Physics rp;
    ground(r);
    const float a30 = 30.0f * 3.14159265f / 180.0f, a60 = 60.0f * 3.14159265f / 180.0f;
    solid(r, "Ramp30", {4, 0, 0}, {3, 0.1f, 1}, Quat::from_axis_angle({0, 0, 1}, a30));
    solid(r, "Ramp60", {4, 0, 5}, {3, 0.1f, 1}, Quat::from_axis_angle({0, 0, 1}, a60));
    const EntityId climber = character(r, "Climber", {0, 0.9f, 0});
    const EntityId blocked = character(r, "Blocked", {0, 0.9f, 5});
    for (int i = 0; i < 100; ++i) {
        for (EntityId who : {climber, blocked}) {
            Character c = *r.try_get<Character>(who);
            c.velocity.x = 3;
            r.ecs().entity(who).set(c);
        }
        r.set_tick_index(r.tick_index());
        rp.step(r, 1.0 / 60.0);
        rp.move_characters(r, 1.0 / 60.0);
        r.tick(1.0 / 60.0);
    }
    const Transform* tc = r.try_get<Transform>(climber);
    INFO("climber at " << tc->position.x << ", " << tc->position.y);
    REQUIRE(tc->position.x == Catch::Approx(5.0).margin(0.05));   // 3 across for 100 ticks: the slope keeps the pace
    REQUIRE(tc->position.y > 0.9f + (tc->position.x - 4.0f - 0.3f) * std::tan(a30));   // up on the ramp
    REQUIRE(r.try_get<Character>(climber)->grounded);
    REQUIRE(r.try_get<Character>(climber)->ground_normal.y == Catch::Approx(std::cos(a30)).margin(0.02));
    const Transform* tb = r.try_get<Transform>(blocked);
    INFO("blocked at " << tb->position.x << ", " << tb->position.y);
    REQUIRE(tb->position.y == Catch::Approx(0.9).margin(0.02));   // at the steep ramp's foot (its surface meets the ground at x 3.88)
    REQUIRE(tb->position.x < 3.7f);
    REQUIRE(r.try_get<Character>(blocked)->on_wall);
    // Walking back down the 30 degree ramp keeps to it: never off the ground.
    int airborne = 0;
    walk(rp, r, climber, {-3, 0, 0}, 60, &airborne);
    REQUIRE(airborne == 0);
    REQUIRE(r.try_get<Transform>(climber)->position.y == Catch::Approx(0.9).margin(0.05));
}

TEST_CASE("a character rides a moving platform and pushes a crate", "[physics][character]") {
    World w;
    physics::Physics p;
    ground(w);
    const EntityId lift = solid(w, "Lift", {0, 0.5f, 0}, {1, 0.1f, 1}, Quat{}, 2);
    const EntityId rider = character(w, "Rider", {0, 1.6f, 0});
    walk(p, w, rider, {}, 20);   // lands on the lift standing still
    REQUIRE(w.try_get<Character>(rider)->grounded);
    Velocity lv;
    lv.linear = {1, 0.5f, 0};
    w.ecs().entity(lift).set(lv);
    walk(p, w, rider, {}, 60);
    const Transform* lt = w.try_get<Transform>(lift);
    const Transform* rt = w.try_get<Transform>(rider);
    INFO("lift " << lt->position.x << "," << lt->position.y << " rider " << rt->position.x << "," << rt->position.y);
    REQUIRE(w.try_get<Character>(rider)->grounded);
    REQUIRE(w.try_get<Character>(rider)->ground == lift);
    REQUIRE(rt->position.x == Catch::Approx(lt->position.x).margin(0.05));   // carried along x
    REQUIRE(rt->position.y == Catch::Approx(lt->position.y + 0.1 + 0.9).margin(0.05));   // and up with it

    // A crate in the way is pushed along at the walking speed.
    World c;
    physics::Physics cp;
    ground(c);
    const EntityId crate = body(c, "Crate", 0, {2, 0.5f, 0}, 0.5f, Json{{"friction", 0.2}});
    const EntityId pusher = character(c, "Pusher", {0, 0.9f, 0});
    walk(cp, c, pusher, {2, 0, 0}, 90);
    INFO("crate at " << c.try_get<Transform>(crate)->position.x << ", pusher at " << c.try_get<Transform>(pusher)->position.x);
    REQUIRE(c.try_get<Transform>(crate)->position.x > 3.0f);
    REQUIRE(c.try_get<Transform>(pusher)->position.x > 1.5f);
    // With push 0 the crate stays where it is.
    Character still = *c.try_get<Character>(pusher);
    still.push = 0;
    c.ecs().entity(pusher).set(still);
    Velocity stop;
    c.ecs().entity(crate).set(stop);
    walk(cp, c, pusher, {}, 60);
    const float before = c.try_get<Transform>(crate)->position.x;
    walk(cp, c, pusher, {2, 0, 0}, 60);
    REQUIRE(c.try_get<Transform>(crate)->position.x == Catch::Approx(before).margin(0.05));
}

TEST_CASE("a character walks down stairs on the ground and falls off a ledge", "[physics][character]") {
    World w;
    physics::Physics p;
    ground(w);
    // Four stairs of 0.2 going down along x from a landing 0.8 high, and a 1.2 high block beside them.
    solid(w, "Landing", {-1, 0.4f, 0}, {1, 0.4f, 1});
    for (int i = 0; i < 3; ++i) {
        const float top = 0.6f - 0.2f * static_cast<float>(i);   // 0.6, 0.4, 0.2, then the ground
        solid(w, "Stair", {0.25f + 0.5f * static_cast<float>(i), top * 0.5f, 0}, {0.25f, top * 0.5f, 1});
    }
    solid(w, "Block", {-1, 0.6f, 4}, {1, 0.6f, 1});
    const EntityId down = character(w, "Down", {-1.5f, 0.8f + 0.9f + 0.05f, 0});
    const EntityId off = character(w, "Off", {-1.5f, 1.2f + 0.9f + 0.05f, 4});
    walk(p, w, down, {}, 30);
    REQUIRE(w.try_get<Character>(down)->grounded);
    REQUIRE(w.try_get<Character>(off)->grounded);
    const std::int64_t landings = w.events().histogram()["character.landed"].get<std::int64_t>();
    int airborne = 0;
    for (int i = 0; i < 120; ++i) {
        Character o = *w.try_get<Character>(off);
        o.velocity.x = 2;
        w.ecs().entity(off).set(o);
        walk(p, w, down, {2, 0, 0}, 1, &airborne);
    }
    INFO("down at " << w.try_get<Transform>(down)->position.x << ", " << w.try_get<Transform>(down)->position.y);
    REQUIRE(airborne == 0);   // every stair followed without leaving the ground
    REQUIRE(w.try_get<Transform>(down)->position.x == Catch::Approx(2.5).margin(0.02));   // four across at 2, past the last stair
    REQUIRE(w.try_get<Transform>(down)->position.y == Catch::Approx(0.9).margin(0.02));
    // Off the 1.2 block's edge: a fall of more than a step, and a landing below.
    REQUIRE(w.try_get<Transform>(off)->position.y == Catch::Approx(0.9).margin(0.02));
    REQUIRE(w.events().histogram()["character.landed"].get<std::int64_t>() == landings + 1);
}

TEST_CASE("a character walking through a trigger enters and leaves it", "[physics][character]") {
    World w;
    physics::Physics p;
    ground(w);
    w.spawn("Zone", 0, Json{{"Transform", {{"position", {{"x", 2}, {"y", 1}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"is_trigger", true}, {"size", {{"x", 0.5}, {"y", 1}, {"z", 1}}}}}}).value();
    const EntityId hero = character(w, "Hero", {0, 0.9f, 0});
    walk(p, w, hero, {3, 0, 0}, 40);   // two units along: in the middle of the zone
    auto hist = w.events().histogram();
    REQUIRE(hist["trigger.enter"] == 1);
    REQUIRE_FALSE(hist.contains("trigger.exit"));
    REQUIRE(w.try_get<Transform>(hero)->position.x == Catch::Approx(2.0).margin(0.02));   // triggers stop nothing
    walk(p, w, hero, {3, 0, 0}, 40);   // and out the far side
    hist = w.events().histogram();
    REQUIRE(hist["trigger.enter"] == 1);
    REQUIRE(hist["trigger.exit"] == 1);
}

namespace {

EntityId car(World& w, Vec3 pos) {
    Json wheels = Json::array();
    for (int k = 0; k < 4; ++k) {
        const float x = (k % 2 == 0) ? -0.8f : 0.8f, z = k < 2 ? -1.3f : 1.3f;   // front wheels at -z
        wheels.push_back(Json{{"offset", {{"x", x}, {"y", -0.2}, {"z", z}}}, {"radius", 0.35}, {"rest", 0.3}, {"steer", k < 2}, {"drive", k >= 2}});
    }
    return w.spawn("Car", 0, Json{{"Transform", {{"position", {{"x", pos.x}, {"y", pos.y}, {"z", pos.z}}}}}, {"RigidBody", {{"kind", 0}, {"mass", 1200}, {"friction", 0.3}}},
        {"Collider", {{"shape", 0}, {"size", {{"x", 0.9}, {"y", 0.3}, {"z", 2.0}}}}}, {"Vehicle", {{"wheels", wheels}}}}).value();
}

void drive(physics::Physics& p, World& w, EntityId who, float throttle, float steer, float brake, int ticks) {
    for (int i = 0; i < ticks; ++i) {
        Vehicle v = *w.try_get<Vehicle>(who);
        v.throttle = throttle;
        v.steer = steer;
        v.brake = brake;
        w.ecs().entity(who).set(v);
        w.set_tick_index(w.tick_index());
        p.step(w, 1.0 / 60.0);
        w.tick(1.0 / 60.0);
    }
}

float yaw_of(const World& w, EntityId who) {
    const Vec3 f = w.try_get<Transform>(who)->rotation.rotate(Vec3{0, 0, -1});
    return std::atan2(-f.x, -f.z);
}

}  // namespace

TEST_CASE("a vehicle settles on its springs, drives to its top speed, turns and brakes", "[physics][vehicle]") {
    World w;
    physics::Physics p;
    w.spawn("Ground", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", -0.5}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 400}, {"y", 0.5}, {"z", 400}}}}}}).value();
    const EntityId c = car(w, {0, 1.2f, 0});
    drive(p, w, c, 0, 0, 0, 180);
    const Vehicle* v = w.try_get<Vehicle>(c);
    REQUIRE(v->grounded == 4);
    for (const Wheel& wh : v->wheels) REQUIRE(wh.contact);
    // The springs hold the weight: compressed by g / (2 pi f)^2 = 0.051 at 2.2 Hz, the body level.
    const float sag = 9.81f / std::pow(2.0f * 3.14159265f * 2.2f, 2.0f);
    REQUIRE(v->wheels[0].compression == Catch::Approx(sag).margin(0.01));
    REQUIRE(w.try_get<Transform>(c)->position.y == Catch::Approx(0.2 + 0.3 - sag + 0.35).margin(0.02));
    REQUIRE(std::fabs(v->speed) < 0.05f);
    REQUIRE(std::fabs(w.try_get<Transform>(c)->rotation.rotate(Vec3{0, 1, 0}).y - 1.0f) < 0.001f);
    // Full throttle asks 10 units per second squared of two driven wheels carrying half the weight:
    // their grip (1.4 times their load) allows about 0.7 g, so the rear tyres set the pace; then up
    // to the top speed of 25.
    drive(p, w, c, 1, 0, 0, 60);
    INFO("after a second " << w.try_get<Vehicle>(c)->speed);
    REQUIRE(w.try_get<Vehicle>(c)->speed == Catch::Approx(1.4 * 9.81 * 0.5).margin(1.0));
    drive(p, w, c, 1, 0, 0, 240);
    REQUIRE(w.try_get<Vehicle>(c)->speed == Catch::Approx(25).margin(1.0));
    REQUIRE(w.try_get<Transform>(c)->position.z < -60.0f);                  // forward is -z
    REQUIRE(std::fabs(w.try_get<Transform>(c)->position.x) < 0.5f);        // straight
    REQUIRE(w.try_get<Vehicle>(c)->wheels[2].spin > 100.0f);               // the wheels rolled forward
    // Steering right turns it to the right (clockwise seen from above) while it stays upright.
    const float yaw0 = yaw_of(w, c);
    drive(p, w, c, 0.4f, 1, 0, 60);
    const float turned = yaw_of(w, c) - yaw0;
    INFO("turned " << turned);
    REQUIRE(turned < -0.3f);
    REQUIRE(w.try_get<Transform>(c)->rotation.rotate(Vec3{0, 1, 0}).y > 0.9f);
    // Braking stops it, and it stays stopped.
    drive(p, w, c, 0, 0, 1, 180);
    REQUIRE(std::fabs(w.try_get<Vehicle>(c)->speed) < 0.1f);
    const Vec3 at = w.try_get<Transform>(c)->position;
    drive(p, w, c, 0, 0, 1, 60);
    REQUIRE(length(w.try_get<Transform>(c)->position - at) < 0.05f);
    // Reverse backs it up.
    drive(p, w, c, -1, 0, 0, 60);
    REQUIRE(w.try_get<Vehicle>(c)->speed < -5.0f);
}

namespace {

EntityId lake(World& w, Json water) {
    Json wa = Json{{"size", {{"x", 20}, {"y", 20}}}, {"depth", 5}, {"wave_height", 0}};
    for (auto& [k, v] : water.items()) wa[k] = v;
    return w.spawn("Lake", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"Water", wa}}).value();
}

}  // namespace

TEST_CASE("what is lighter than water floats at its level and what is heavier sinks", "[physics][water]") {
    World w;
    physics::Physics p;
    w.spawn("Bottom", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", -4.5}, {"z", 0}}}}}, {"RigidBody", {{"kind", 1}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 20}, {"y", 0.5}, {"z", 20}}}}}}).value();
    const EntityId water = lake(w, Json::object());
    // A unit cube of mass 1 in water of density 2 displaces its weight half under: centred on the surface.
    const EntityId cork = body(w, "Cork", 0, {-3, 2, 0}, 0.5f, Json{{"mass", 1.0}});
    const EntityId stone = body(w, "Stone", 0, {3, 2, 0}, 0.5f, Json{{"mass", 3.0}});
    const EntityId ball = body(w, "Ball", 1, {0, 2, 3}, 0.5f, Json{{"mass", 0.25}});
    // A raft (a flat board) dropped tipped over rights itself.
    const EntityId raft = w.spawn("Raft", 0, Json{{"Transform", {{"position", {{"x", 0}, {"y", 1.5}, {"z", -4}}}, {"rotation", {{"x", 0.5}, {"y", 0}, {"z", 0}, {"w", 0.866}}}}}, {"RigidBody", {{"kind", 0}, {"mass", 1.0}}}, {"Collider", {{"shape", 0}, {"size", {{"x", 1.0}, {"y", 0.15}, {"z", 0.8}}}}}}).value();
    run(p, w, 600);
    const float cork_y = w.try_get<Transform>(cork)->position.y;
    const float ball_y = w.try_get<Transform>(ball)->position.y;
    const Vec3 raft_up = w.try_get<Transform>(raft)->rotation.rotate(Vec3{0, 1, 0});
    INFO("cork " << cork_y << " stone " << w.try_get<Transform>(stone)->position.y << " ball " << ball_y << " raft up " << raft_up.y << " at " << w.try_get<Transform>(raft)->position.y);
    REQUIRE(cork_y == Catch::Approx(0.0).margin(0.08));
    REQUIRE(w.try_get<Transform>(stone)->position.y == Catch::Approx(-3.5).margin(0.05));   // on the bottom
    // A quarter of the water its whole volume weighs (0.25 of 1.05): mostly out of the water.
    REQUIRE(ball_y > 0.05f);
    REQUIRE(ball_y < 0.4f);
    REQUIRE(raft_up.y > 0.98f);
    // Its 0.96 cubic units weigh about half as much as that much water: half under, centred on the surface.
    REQUIRE(w.try_get<Transform>(raft)->position.y == Catch::Approx(0.0).margin(0.05));
    REQUIRE(std::fabs(w.try_get<Velocity>(cork)->linear.y) < 0.05f);
    auto hist = w.events().histogram();
    REQUIRE(hist["water.entered"].get<int>() == 4);
    REQUIRE(p.stats().floating == 4);
    // Lifted out, it leaves; dropped back, it enters again.
    REQUIRE(w.set(cork, "Transform", Json{{"position", {{"x", -3}, {"y", 6}, {"z", 0}}}}).has_value());
    REQUIRE(w.set(cork, "RigidBody", Json{{"sleeping", false}}).has_value());
    run(p, w, 2);
    REQUIRE(w.events().histogram()["water.left"].get<int>() == 1);
    run(p, w, 120);
    REQUIRE(w.events().histogram()["water.entered"].get<int>() == 5);
    // Disabled, the water holds nothing up.
    REQUIRE(w.set(water, "Water", Json{{"enabled", false}}).has_value());
    run(p, w, 120);
    REQUIRE(w.try_get<Transform>(cork)->position.y < -2.0f);
}

TEST_CASE("floating bodies ride the waves and drift with the current", "[physics][water]") {
    World w;
    physics::Physics p;
    const EntityId water = lake(w, Json{{"wave_height", 0.8}, {"wave_length", 10}, {"choppiness", 0.8}});
    const Water* wa = w.try_get<Water>(water);
    // The surface over a point: the waves' sideways motion undone.
    for (float x : {-3.0f, 0.4f, 2.7f}) {
        const WaterPoint at = water_at(*wa, 0.0f, x, 1.5f, 2.3f);
        REQUIRE(at.position.x == Catch::Approx(x).margin(1e-3));
        REQUIRE(at.position.z == Catch::Approx(1.5).margin(1e-3));
        REQUIRE(at.normal.y > 0.5f);
    }
    const EntityId cork = body(w, "Cork", 0, {0, 0.5f, 0}, 0.3f, Json{{"mass", 0.1}});
    run(p, w, 120);
    float lo = 1e9f, hi = -1e9f, worst = 0, mean = 0;
    for (int i = 0; i < 180; ++i) {
        run(p, w, 1);
        const Vec3 at = w.try_get<Transform>(cork)->position;
        lo = std::min(lo, at.y);
        hi = std::max(hi, at.y);
        const float level = water_at(*w.try_get<Water>(water), 0.0f, at.x, at.z, static_cast<float>(w.seconds())).position.y;
        worst = std::max(worst, std::fabs(at.y - level));
        mean += (at.y - level) / 180.0f;
    }
    INFO("cork between " << lo << " and " << hi << ", at most " << worst << " off the surface, " << mean << " above it on average");
    REQUIRE(hi - lo > 0.4f);     // it rises and falls with waves 0.8 high
    // A quarter of it under (0.1 of the 0.43 its volume of water weighs), its centre rides about
    // 0.16 over the surface, pulled a little off it by the waves but never tossed out or pulled under.
    REQUIRE(mean == Catch::Approx(0.16).margin(0.05));
    REQUIRE(worst < 0.45f);
    // A current of one unit a second along x carries it along, near its speed.
    REQUIRE(w.set(water, "Water", Json{{"wave_height", 0}, {"flow", {{"x", 1}, {"y", 0}}}}).has_value());
    const float x0 = w.try_get<Transform>(cork)->position.x;
    run(p, w, 180);
    INFO("drifted " << w.try_get<Transform>(cork)->position.x - x0);
    REQUIRE(w.try_get<Transform>(cork)->position.x - x0 > 1.8f);
    REQUIRE(w.try_get<Velocity>(cork)->linear.x == Catch::Approx(1.0).margin(0.15));
}

TEST_CASE("a character walks down a beach into deep water, swims with its head out and walks back out", "[physics][character][water]") {
    World w;
    physics::Physics p;
    // A beach at 0 for x < -6, a 20 degree slope down to the bottom at -3 from x = 2.25 on, and water at 0.
    solid(w, "Beach", {-13, -0.5f, 0}, {7, 0.5f, 6});
    solid(w, "Slope", {-1.96f, -1.736f, 0}, {4.39f, 0.25f, 6}, Quat::from_axis_angle({0, 0, 1}, -20.0f * 3.14159265f / 180.0f));
    solid(w, "Bottom", {11, -3.5f, 0}, {9, 0.5f, 6});
    lake(w, Json{{"size", {{"x", 60}, {"y", 20}}}, {"depth", 6}});
    const EntityId hero = character(w, "Hero", {-10, 1, 0});
    walk(p, w, hero, {}, 30);
    REQUIRE(w.try_get<Character>(hero)->grounded);
    REQUIRE_FALSE(w.try_get<Character>(hero)->swimming);
    REQUIRE(w.try_get<Character>(hero)->submerged == 0.0f);
    // Walking out at 4: down the slope, wading, then swimming once the water is over its chest.
    walk(p, w, hero, {4, 0, 0}, 300);
    const Character* c = w.try_get<Character>(hero);
    const Vec3 at = w.try_get<Transform>(hero)->position;
    INFO("at " << at.x << ", " << at.y << " swimming " << c->swimming << " submerged " << c->submerged);
    REQUIRE(c->swimming);
    REQUIRE_FALSE(c->grounded);
    REQUIRE(at.y == Catch::Approx(-0.36).margin(0.05));             // its centre a fifth of its height under the surface
    REQUIRE(c->submerged == Catch::Approx(0.7).margin(0.03));
    // To x = -2.5 at walking pace (1.9 s), then 3.1 s at 0.6 of it.
    REQUIRE(at.x == Catch::Approx(-2.5 + 3.1 * 2.4).margin(0.8));
    // It stays at the surface: no sinking, no bobbing up out.
    walk(p, w, hero, {}, 120);
    REQUIRE(w.try_get<Transform>(hero)->position.y == Catch::Approx(-0.36).margin(0.03));
    // Back to the beach: it swims to the slope, finds its feet and walks up out of the water.
    walk(p, w, hero, {-4, 0, 0}, 300);
    c = w.try_get<Character>(hero);
    INFO("back at " << w.try_get<Transform>(hero)->position.x << ", " << w.try_get<Transform>(hero)->position.y);
    REQUIRE(c->grounded);
    REQUIRE_FALSE(c->swimming);
    REQUIRE(w.try_get<Transform>(hero)->position.x < -7.0f);
    REQUIRE(w.try_get<Transform>(hero)->position.y == Catch::Approx(0.9).margin(0.03));
    REQUIRE(c->submerged == 0.0f);
    const Json hist = w.events().histogram();
    REQUIRE(hist["water.entered"] == 1);
    REQUIRE(hist["water.left"] == 1);
}

TEST_CASE("scattered copies that collide stop characters, bodies, rays and sweeps; without collide they are only drawn", "[physics][scatter]") {
    World w;
    physics::Physics p;
    ground(w);
    // Three posts a Scatter placed (the session's placement stands in: the copies' matrices are set
    // directly), 0.4 across times each copy's size and 2 tall; the middle one twice the size.
    const EntityId posts = w.spawn("Posts", 0, Json{{"Transform", Json::object()}, {"Scatter", {{"count", 0}, {"collide", 0.4}, {"collide_height", 2}}}}).value();
    std::vector<World::Instance> copies;
    copies.push_back({Mat4::trs(Vec3{0, 0, 0}, Quat{}, Vec3{1, 1, 1}), 1});
    copies.push_back({Mat4::trs(Vec3{0, 0, 4}, Quat{}, Vec3{2, 2, 2}), 1});
    copies.push_back({Mat4::trs(Vec3{0, 0, -4}, Quat{}, Vec3{1, 1, 1}), 1});
    w.set_derived_instances(posts, copies);
    p.step(w, 1.0 / 60.0);
    REQUIRE(p.stats().scattered == 3);
    // A ray across meets the first post's side at x -0.4; one from above meets its top at y 2.
    auto ray = p.raycast(w, Vec3{-5, 1, 0}, Vec3{1, 0, 0}, 20.0f, false);
    REQUIRE(ray.has_value());
    REQUIRE(ray->entity == posts);
    REQUIRE(ray->point.x == Catch::Approx(-0.4).margin(1e-3));
    REQUIRE(ray->normal.x == Catch::Approx(-1).margin(1e-3));
    ray = p.raycast(w, Vec3{0, 10, 0}, Vec3{0, -1, 0}, 20.0f, false);
    REQUIRE(ray.has_value());
    REQUIRE(ray->entity == posts);
    REQUIRE(ray->point.y == Catch::Approx(2).margin(1e-3));
    // The bigger one is 0.8 across and 4 tall.
    ray = p.raycast(w, Vec3{-5, 3, 4}, Vec3{1, 0, 0}, 20.0f, false);
    REQUIRE(ray.has_value());
    REQUIRE(ray->point.x == Catch::Approx(-0.8).margin(1e-3));
    // A sphere swept at it stops a radius short; an overlap names the Scatter once.
    auto sweep = p.sweep(w, Vec3{-5, 1, -4}, Vec3{1, 0, 0}, 0.5f, 20.0f, [](EntityId, const RigidBody&, const Collider&) { return true; });
    REQUIRE(sweep.has_value());
    REQUIRE(sweep->entity == posts);
    REQUIRE(sweep->distance == Catch::Approx(5 - 0.4 - 0.5).margin(1e-3));
    const auto near = p.overlap_sphere(w, Vec3{0, 1, 2}, 2.5f);
    REQUIRE(std::count(near.begin(), near.end(), posts) == 1);
    // A character walking at the first post is held at its side, its capsule's radius away.
    const EntityId hero = character(w, "Hero", {-3, 1, 0});
    walk(p, w, hero, {}, 30);
    walk(p, w, hero, {4, 0, 0}, 90);
    REQUIRE(w.try_get<Transform>(hero)->position.x == Catch::Approx(-0.4 - 0.3).margin(0.03));
    REQUIRE(w.try_get<Character>(hero)->on_wall);
    // A ball rolled at the last one bounces back off it.
    const EntityId ball = body(w, "Ball", 1, Vec3{-3, 0.5, -4}, 0.5f, Json{{"restitution", 0.5}});
    w.ecs().entity(ball).set(Velocity{Vec3{6, 0, 0}, Vec3{}});
    float nearest = -10;
    for (int i = 0; i < 90; ++i) {
        p.step(w, 1.0 / 60.0);
        w.tick(1.0 / 60.0);
        nearest = std::max(nearest, w.try_get<Transform>(ball)->position.x);
    }
    REQUIRE(nearest < -0.4 - 0.5 + 0.05);
    REQUIRE(w.try_get<Velocity>(ball)->linear.x < 0);
    // Without collide the copies are only drawn: nothing counts them, the ray goes through.
    Scatter sc = *w.try_get<Scatter>(posts);
    sc.collide = 0;
    w.ecs().entity(posts).set(sc);
    p.step(w, 1.0 / 60.0);
    REQUIRE(p.stats().scattered == 0);
    ray = p.raycast(w, Vec3{-5, 1, 0}, Vec3{1, 0, 0}, 20.0f, false);
    REQUIRE((!ray.has_value() || ray->entity != posts));
}
