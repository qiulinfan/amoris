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
