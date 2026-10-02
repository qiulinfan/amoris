// 2D rigid bodies on Box2D 3.1 (docs/design/physics2d.md). The components are the truth: every tick
// the Box2D world is brought in line with them (bodies, shapes and joints made, changed or let go;
// a Transform or a velocity a script wrote is taken as a teleport or a new speed), stepped, and what
// moved is written back. Everything runs in entity order on one thread, so a run repeats exactly.
#include <pocket/physics/rigid2d.hpp>

#include <pocket/assets/assets.hpp>

#include <box2d/box2d.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <format>
#include <map>
#include <set>

namespace pocket::physics {

namespace {

b2Vec2 bv(Vec2 v) { return b2Vec2{v.x, v.y}; }

// A turn about Z from a rotation (the Transform's), and back.
float angle_of(const Quat& q) { return std::atan2(2.0f * (q.w * q.z + q.x * q.y), 1.0f - 2.0f * (q.y * q.y + q.z * q.z)); }
Quat about_z(float a) { return Quat{0, 0, std::sin(a * 0.5f), std::cos(a * 0.5f)}; }

void* tag(world::EntityId id) { return reinterpret_cast<void*>(static_cast<std::uintptr_t>(id)); }
world::EntityId untag(void* p) { return static_cast<world::EntityId>(reinterpret_cast<std::uintptr_t>(p)); }

b2BodyType body_type(int kind) { return kind == 1 ? b2_staticBody : kind == 2 ? b2_kinematicBody : b2_dynamicBody; }

}  // namespace

struct Rigid2D::Impl {
    b2WorldId world = b2_nullWorldId;
    struct Body {
        b2BodyId id = b2_nullBodyId;
        bool collider_only = false;           // a Collider2D without a RigidBody2D: a static body of its own
        world::RigidBody2D rb;                // the settings it was made or last set with
        world::Collider2D col;                // the shape it carries
        bool has_shape = false;
        Vec2 scale{1, 1};
        Vec2 pos;                             // what the engine last wrote or read
        float angle = 0;
        Vec2 vel;
        float spin = 0;
        std::vector<b2ShapeId> shapes;
    };
    std::map<world::EntityId, Body> bodies;
    struct Joint {
        b2JointId id = b2_nullJointId;
        world::Joint2D def;
        b2BodyId a = b2_nullBodyId, b = b2_nullBodyId;
    };
    std::map<world::EntityId, Joint> joints;
    b2BodyId ground = b2_nullBodyId;          // what a joint to the world holds on to
    struct Tiles {
        b2BodyId id = b2_nullBodyId;
        std::string key;
        int shapes = 0;
    };
    std::map<world::EntityId, Tiles> tiles;
    std::map<std::pair<world::EntityId, world::EntityId>, std::uint64_t> touching;   // pair -> its begin event
    int steps = 0, begun = 0, tile_shapes = 0;
    double step_ms = 0;
    std::string skipped_maps;

    ~Impl() {
        if (b2World_IsValid(world)) b2DestroyWorld(world);
    }

    void ensure_world(Vec2 gravity) {
        if (b2World_IsValid(world)) {
            b2World_SetGravity(world, bv(gravity));
            return;
        }
        b2WorldDef def = b2DefaultWorldDef();
        def.gravity = bv(gravity);
        world = b2CreateWorld(&def);
        b2BodyDef gd = b2DefaultBodyDef();
        gd.type = b2_staticBody;
        ground = b2CreateBody(world, &gd);
    }

    static b2ShapeDef shape_def(const world::Collider2D& c, world::EntityId id) {
        b2ShapeDef sd = b2DefaultShapeDef();
        sd.density = std::max(c.density, 0.0f);
        sd.material.friction = std::max(c.friction, 0.0f);
        sd.material.restitution = std::clamp(c.restitution, 0.0f, 1.0f);
        sd.isSensor = c.sensor;
        sd.enableSensorEvents = true;
        sd.enableContactEvents = !c.sensor;
        sd.filter.categoryBits = c.layer;
        sd.filter.maskBits = c.mask;
        sd.userData = tag(id);
        return sd;
    }

    void make_shapes(Body& b, world::EntityId id) {
        for (b2ShapeId s : b.shapes) if (b2Shape_IsValid(s)) b2DestroyShape(s, true);
        b.shapes.clear();
        if (!b.has_shape) return;
        const world::Collider2D& c = b.col;
        const b2ShapeDef sd = shape_def(c, id);
        const float sx = std::fabs(b.scale.x), sy = std::fabs(b.scale.y);
        const b2Vec2 center{c.offset.x * b.scale.x, c.offset.y * b.scale.y};
        const float rs = std::max(sx, sy);
        if (c.shape == 1) {
            b2Circle circle{center, std::max(c.radius * rs, 1e-3f)};
            b.shapes.push_back(b2CreateCircleShape(b.id, &sd, &circle));
        } else if (c.shape == 2) {
            const float half = c.size.y * sy;
            b2Capsule cap{b2Vec2{center.x, center.y - half}, b2Vec2{center.x, center.y + half}, std::max(c.radius * rs, 1e-3f)};
            b.shapes.push_back(b2CreateCapsuleShape(b.id, &sd, &cap));
        } else if (c.shape == 3) {
            std::vector<b2Vec2> pts;
            for (const world::Point2D& p : c.points) {
                if (pts.size() == B2_MAX_POLYGON_VERTICES) break;
                pts.push_back(b2Vec2{p.x * b.scale.x, p.y * b.scale.y});
            }
            if (pts.size() >= 3) {
                const b2Hull hull = b2ComputeHull(pts.data(), static_cast<int>(pts.size()));
                if (hull.count >= 3) {
                    const b2Polygon poly = b2MakeOffsetRoundedPolygon(&hull, center, b2MakeRot(c.angle), std::max(c.radius < 0.5f ? c.radius * rs : 0.0f, 0.0f));
                    b.shapes.push_back(b2CreatePolygonShape(b.id, &sd, &poly));
                }
            }
        } else {
            const float hx = std::max(c.size.x * sx, 1e-3f), hy = std::max(c.size.y * sy, 1e-3f);
            // A box's rounding only when asked below its half size (the default radius is a circle's).
            const float round = c.radius < std::min(hx, hy) && c.radius < 0.5f ? c.radius : 0.0f;
            const b2Polygon box = round > 0 ? b2MakeOffsetRoundedBox(hx - round, hy - round, center, b2MakeRot(c.angle), round) : b2MakeOffsetBox(hx, hy, center, b2MakeRot(c.angle));
            b.shapes.push_back(b2CreatePolygonShape(b.id, &sd, &box));
        }
    }

    void apply_settings(Body& b, const world::RigidBody2D& rb) {
        if (b2Body_GetType(b.id) != body_type(rb.kind)) b2Body_SetType(b.id, body_type(rb.kind));
        b2Body_SetGravityScale(b.id, rb.gravity_scale);
        b2Body_SetLinearDamping(b.id, std::max(rb.linear_damping, 0.0f));
        b2Body_SetAngularDamping(b.id, std::max(rb.angular_damping, 0.0f));
        b2Body_SetFixedRotation(b.id, rb.fixed_rotation);
        b2Body_SetBullet(b.id, rb.bullet);
        if (rb.enabled != b2Body_IsEnabled(b.id)) {
            if (rb.enabled) b2Body_Enable(b.id);
            else b2Body_Disable(b.id);
        }
    }

    static bool same_settings(const world::RigidBody2D& a, const world::RigidBody2D& b) {
        return a.kind == b.kind && a.gravity_scale == b.gravity_scale && a.linear_damping == b.linear_damping && a.angular_damping == b.angular_damping && a.fixed_rotation == b.fixed_rotation && a.bullet == b.bullet && a.enabled == b.enabled;
    }

    // The solid cells of an orthogonal map as static boxes, runs of a row joined and equal runs of
    // rows below joined into one, the cells with collision shapes as those boxes, slopes as ramps.
    void build_tiles(Tiles& t, world::EntityId id, const assets::TileMap& map, Vec3 origin, float ts) {
        if (b2Body_IsValid(t.id)) b2DestroyBody(t.id);
        b2BodyDef bd = b2DefaultBodyDef();
        bd.type = b2_staticBody;
        bd.userData = tag(id);
        t.id = b2CreateBody(world, &bd);
        t.shapes = 0;
        world::Collider2D c;
        c.friction = 0.6f;
        c.layer = 1;
        b2ShapeDef sd = shape_def(c, id);
        auto left = [&](int x) { return origin.x + static_cast<float>(x) * ts; };
        auto top = [&](int y) { return origin.y - static_cast<float>(y) * ts; };
        auto add_box = [&](float l, float r, float b, float tp) {
            const b2Polygon box = b2MakeOffsetBox((r - l) * 0.5f, (tp - b) * 0.5f, b2Vec2{(l + r) * 0.5f, (b + tp) * 0.5f}, b2Rot_identity);
            b2CreatePolygonShape(t.id, &sd, &box);
            ++t.shapes;
        };
        // Whole solid cells (no shapes of their own) per row as runs [x0, x1).
        std::vector<std::vector<std::pair<int, int>>> runs(static_cast<std::size_t>(map.height));
        std::vector<assets::TileSet::Shape> shapes;
        for (int y = 0; y < map.height; ++y) {
            int start = -1;
            for (int x = 0; x <= map.width; ++x) {
                bool whole = false;
                if (x < map.width) {
                    const int kind = map.solidity_at(x, y);
                    if (kind == 1) {
                        map.solid_boxes(x, y, shapes);
                        whole = shapes.size() == 1 && shapes[0].x0 <= 0 && shapes[0].y0 <= 0 && shapes[0].x1 >= 1 && shapes[0].y1 >= 1;
                        if (!whole) for (const auto& s : shapes) add_box(left(x) + s.x0 * ts, left(x) + s.x1 * ts, top(y) - s.y1 * ts, top(y) - s.y0 * ts);
                    } else if (kind == 3) {
                        // A ramp: rising to the right or to the left across the cell.
                        const int dir = map.slope_at(x, y);
                        const b2Vec2 pts[3] = {{left(x), top(y + 1)}, {left(x + 1), top(y + 1)}, dir >= 0 ? b2Vec2{left(x + 1), top(y)} : b2Vec2{left(x), top(y)}};
                        const b2Hull hull = b2ComputeHull(pts, 3);
                        if (hull.count == 3) {
                            const b2Polygon ramp = b2MakePolygon(&hull, 0);
                            b2CreatePolygonShape(t.id, &sd, &ramp);
                            ++t.shapes;
                        }
                    }
                }
                if (whole && start < 0) start = x;
                if (!whole && start >= 0) {
                    runs[static_cast<std::size_t>(y)].push_back({start, x});
                    start = -1;
                }
            }
        }
        // Join each run with the same run in the rows below it.
        for (int y = 0; y < map.height; ++y) {
            for (const auto& run : runs[static_cast<std::size_t>(y)]) {
                if (run.first < 0) continue;   // taken into a box above
                int y1 = y + 1;
                while (y1 < map.height) {
                    auto& below = runs[static_cast<std::size_t>(y1)];
                    auto it = std::find(below.begin(), below.end(), run);
                    if (it == below.end()) break;
                    it->first = -1;
                    ++y1;
                }
                add_box(left(run.first), left(run.second), top(y1), top(y));
            }
        }
    }
};

Rigid2D::Rigid2D() : impl_(std::make_unique<Impl>()) {}
Rigid2D::~Rigid2D() = default;

bool Rigid2D::active() const { return b2World_IsValid(impl_->world); }

void Rigid2D::step(world::World& w, assets::AssetStore& assets, float dt, Vec2 gravity) {
    Impl& im = *impl_;
    // Who has a 2D body or shape, in entity order.
    std::vector<world::EntityId> ids;
    w.ecs().each([&](flecs::entity e, const world::RigidBody2D&) { ids.push_back(e.id()); });
    w.ecs().each([&](flecs::entity e, const world::Collider2D&) { if (!e.has<world::RigidBody2D>()) ids.push_back(e.id()); });
    if (ids.empty() && !im.bodies.empty()) {
        // Every 2D body gone: the world goes too.
        b2DestroyWorld(im.world);
        im.world = b2_nullWorldId;
        im.bodies.clear();
        im.joints.clear();
        im.tiles.clear();
        im.touching.clear();
        return;
    }
    if (ids.empty()) return;
    std::sort(ids.begin(), ids.end());
    const auto t0 = std::chrono::steady_clock::now();
    im.ensure_world(gravity);

    // Bodies and their shapes.
    std::set<world::EntityId> seen;
    for (world::EntityId id : ids) {
        const auto* tr = w.try_get<world::Transform>(id);
        if (!tr) continue;
        seen.insert(id);
        const auto* rbp = w.try_get<world::RigidBody2D>(id);
        world::RigidBody2D rb = rbp ? *rbp : world::RigidBody2D{};
        if (!rbp) rb.kind = 1;
        const auto* col = w.try_get<world::Collider2D>(id);
        Vec3 wp = tr->position;
        Quat wq = tr->rotation;
        if (!rbp) {
            // A static shape may sit under a parent: placed where its parents put it.
            if (const auto* wt = w.try_get<world::WorldTransform>(id)) { wp = wt->position; wq = wt->rotation; }
        }
        const Vec2 pos{wp.x, wp.y};
        const float ang = angle_of(wq);
        const Vec2 scale{tr->scale.x, tr->scale.y};
        auto it = im.bodies.find(id);
        if (it == im.bodies.end() || !b2Body_IsValid(it->second.id)) {
            Impl::Body b;
            b2BodyDef bd = b2DefaultBodyDef();
            bd.type = body_type(rb.kind);
            bd.position = bv(pos);
            bd.rotation = b2MakeRot(ang);
            bd.linearVelocity = bv(rb.velocity);
            bd.angularVelocity = rb.angular_velocity;
            bd.gravityScale = rb.gravity_scale;
            bd.linearDamping = std::max(rb.linear_damping, 0.0f);
            bd.angularDamping = std::max(rb.angular_damping, 0.0f);
            bd.fixedRotation = rb.fixed_rotation;
            bd.isBullet = rb.bullet;
            bd.isAwake = rb.awake;
            bd.isEnabled = rb.enabled;
            bd.userData = tag(id);
            b.id = b2CreateBody(im.world, &bd);
            b.collider_only = !rbp;
            b.rb = rb;
            b.pos = pos;
            b.angle = ang;
            b.vel = rb.velocity;
            b.spin = rb.angular_velocity;
            b.scale = scale;
            if (col) {
                b.col = *col;
                b.has_shape = true;
            }
            im.make_shapes(b, id);
            im.bodies[id] = std::move(b);
            continue;
        }
        Impl::Body& b = it->second;
        if (b.collider_only != !rbp) {
            // Gained or lost its RigidBody2D: made again next tick as what it is now.
            b2DestroyBody(b.id);
            im.bodies.erase(it);
            seen.erase(id);
            continue;
        }
        if (!Impl::same_settings(b.rb, rb)) {
            im.apply_settings(b, rb);
        }
        // A Transform or velocity written since the engine wrote it: taken as the script's.
        if (pos.x != b.pos.x || pos.y != b.pos.y || ang != b.angle) {
            b2Body_SetTransform(b.id, bv(pos), b2MakeRot(ang));
            b.pos = pos;
            b.angle = ang;
        }
        if (rb.velocity.x != b.vel.x || rb.velocity.y != b.vel.y) {
            b2Body_SetLinearVelocity(b.id, bv(rb.velocity));
            b.vel = rb.velocity;
        }
        if (rb.angular_velocity != b.spin) {
            b2Body_SetAngularVelocity(b.id, rb.angular_velocity);
            b.spin = rb.angular_velocity;
        }
        if (rb.awake != b.rb.awake) b2Body_SetAwake(b.id, rb.awake);
        b.rb = rb;
        const bool want_shape = col != nullptr;
        if (want_shape != b.has_shape || (col && !(b.col == *col)) || !(scale == b.scale)) {
            b.has_shape = want_shape;
            if (col) b.col = *col;
            b.scale = scale;
            im.make_shapes(b, id);
        }
    }
    for (auto it = im.bodies.begin(); it != im.bodies.end();) {
        if (seen.contains(it->first)) { ++it; continue; }
        if (b2Body_IsValid(it->second.id)) b2DestroyBody(it->second.id);
        it = im.bodies.erase(it);
    }

    // Tile maps: their solid cells, made again when the map, its place or its tile size changes.
    std::set<world::EntityId> maps_seen;
    im.skipped_maps.clear();
    im.tile_shapes = 0;
    w.ecs().each([&](flecs::entity e, const world::TileMap& tm) {
        if (tm.map.empty()) return;
        auto m = assets.tilemap(tm.map);
        if (!m) return;
        if ((*m)->orientation != "orthogonal") {
            im.skipped_maps += (im.skipped_maps.empty() ? "" : ", ") + tm.map;
            return;
        }
        Vec3 origin{0, 0, 0};
        if (const auto* wt = e.try_get<world::WorldTransform>()) origin = wt->position;
        else if (const auto* t = e.try_get<world::Transform>()) origin = t->position;
        const float ts = tm.tile_size > 0 ? tm.tile_size : 1.0f;
        const std::string key = std::format("{}#{}@{},{}x{}", tm.map, (*m)->revision, origin.x, origin.y, ts);
        Impl::Tiles& t = im.tiles[e.id()];
        maps_seen.insert(e.id());
        if (t.key != key || !b2Body_IsValid(t.id)) {
            im.build_tiles(t, e.id(), **m, origin, ts);
            t.key = key;
        }
        im.tile_shapes += t.shapes;
    });
    for (auto it = im.tiles.begin(); it != im.tiles.end();) {
        if (maps_seen.contains(it->first)) { ++it; continue; }
        if (b2Body_IsValid(it->second.id)) b2DestroyBody(it->second.id);
        it = im.tiles.erase(it);
    }

    // Joints: made when their bodies are, made again when they change or a body is made again.
    std::set<world::EntityId> joints_seen;
    w.ecs().each([&](flecs::entity e, const world::Joint2D& j) {
        auto a = im.bodies.find(e.id());
        if (a == im.bodies.end() || a->second.collider_only) return;
        b2BodyId other = im.ground;
        if (j.body) {
            auto b = im.bodies.find(static_cast<world::EntityId>(j.body));
            if (b == im.bodies.end() || b->first == e.id()) return;
            other = b->second.id;
        }
        joints_seen.insert(e.id());
        Impl::Joint& jr = im.joints[e.id()];
        const bool same_bodies = b2Joint_IsValid(jr.id) && B2_ID_EQUALS(jr.a, a->second.id) && B2_ID_EQUALS(jr.b, other);
        if (same_bodies && jr.def == j) return;
        // Only its motor, limits or spring changed (a script driving a wheel every tick): set in
        // place, so the joint keeps what it has solved so far.
        auto structure = [](world::Joint2D d) {
            d.enable_motor = false; d.motor_speed = 0; d.max_motor_force = 0;
            d.enable_limit = false; d.lower = 0; d.upper = 0; d.min_length = 0; d.max_length = 0;
            d.enable_spring = false; d.hertz = 0; d.damping_ratio = 0; d.break_force = 0;
            return d;
        };
        if (same_bodies && structure(jr.def) == structure(j)) {
            const b2JointId id = jr.id;
            switch (j.kind) {
                case 1:
                    b2DistanceJoint_EnableMotor(id, j.enable_motor); b2DistanceJoint_SetMotorSpeed(id, j.motor_speed); b2DistanceJoint_SetMaxMotorForce(id, j.max_motor_force);
                    b2DistanceJoint_EnableLimit(id, j.enable_limit); b2DistanceJoint_SetLengthRange(id, j.min_length, j.max_length);
                    b2DistanceJoint_EnableSpring(id, j.enable_spring); b2DistanceJoint_SetSpringHertz(id, j.hertz); b2DistanceJoint_SetSpringDampingRatio(id, j.damping_ratio);
                    break;
                case 2:
                    b2PrismaticJoint_EnableMotor(id, j.enable_motor); b2PrismaticJoint_SetMotorSpeed(id, j.motor_speed); b2PrismaticJoint_SetMaxMotorForce(id, j.max_motor_force);
                    b2PrismaticJoint_EnableLimit(id, j.enable_limit); b2PrismaticJoint_SetLimits(id, j.lower, j.upper);
                    b2PrismaticJoint_EnableSpring(id, j.enable_spring); b2PrismaticJoint_SetSpringHertz(id, j.hertz); b2PrismaticJoint_SetSpringDampingRatio(id, j.damping_ratio);
                    break;
                case 3:
                    b2WeldJoint_SetLinearHertz(id, j.enable_spring ? j.hertz : 0.0f); b2WeldJoint_SetAngularHertz(id, j.enable_spring ? j.hertz : 0.0f);
                    b2WeldJoint_SetLinearDampingRatio(id, j.damping_ratio); b2WeldJoint_SetAngularDampingRatio(id, j.damping_ratio);
                    break;
                case 4:
                    b2WheelJoint_EnableMotor(id, j.enable_motor); b2WheelJoint_SetMotorSpeed(id, j.motor_speed); b2WheelJoint_SetMaxMotorTorque(id, j.max_motor_force);
                    b2WheelJoint_EnableLimit(id, j.enable_limit); b2WheelJoint_SetLimits(id, j.lower, j.upper);
                    b2WheelJoint_EnableSpring(id, j.enable_spring); b2WheelJoint_SetSpringHertz(id, j.hertz); b2WheelJoint_SetSpringDampingRatio(id, j.damping_ratio);
                    break;
                default:
                    b2RevoluteJoint_EnableMotor(id, j.enable_motor); b2RevoluteJoint_SetMotorSpeed(id, j.motor_speed); b2RevoluteJoint_SetMaxMotorTorque(id, j.max_motor_force);
                    b2RevoluteJoint_EnableLimit(id, j.enable_limit); b2RevoluteJoint_SetLimits(id, j.lower, j.upper);
                    b2RevoluteJoint_EnableSpring(id, j.enable_spring); b2RevoluteJoint_SetSpringHertz(id, j.hertz); b2RevoluteJoint_SetSpringDampingRatio(id, j.damping_ratio);
                    break;
            }
            if (j.enable_motor && j.motor_speed != jr.def.motor_speed) {
                // A motor told to turn wakes what it turns.
                b2Body_SetAwake(jr.a, true);
                if (b2Body_GetType(jr.b) != b2_staticBody) b2Body_SetAwake(jr.b, true);
            }
            jr.def = j;
            return;
        }
        if (b2Joint_IsValid(jr.id)) b2DestroyJoint(jr.id);
        jr.def = j;
        jr.a = a->second.id;
        jr.b = other;
        const b2Vec2 la = bv(j.anchor), lb = bv(j.other_anchor);
        const float ref = b2Rot_GetAngle(b2Body_GetRotation(other)) * -1.0f + b2Rot_GetAngle(b2Body_GetRotation(a->second.id));
        switch (j.kind) {
            case 1: {
                b2DistanceJointDef d = b2DefaultDistanceJointDef();
                d.bodyIdA = jr.a;
                d.bodyIdB = jr.b;
                d.localAnchorA = la;
                d.localAnchorB = lb;
                const b2Vec2 pa = b2Body_GetWorldPoint(jr.a, la), pb = b2Body_GetWorldPoint(jr.b, lb);
                d.length = j.length >= 0 ? j.length : b2Distance(pa, pb);
                d.enableSpring = j.enable_spring;
                d.hertz = j.hertz;
                d.dampingRatio = j.damping_ratio;
                d.enableLimit = j.enable_limit;
                d.minLength = j.min_length;
                d.maxLength = j.max_length;
                d.enableMotor = j.enable_motor;
                d.maxMotorForce = j.max_motor_force;
                d.motorSpeed = j.motor_speed;
                d.collideConnected = j.collide_connected;
                jr.id = b2CreateDistanceJoint(im.world, &d);
                break;
            }
            case 2: {
                // A slide along `axis` in the other body's space (the world's when there is none):
                // Box2D's body A is the other one here.
                b2PrismaticJointDef d = b2DefaultPrismaticJointDef();
                d.bodyIdA = jr.b;
                d.bodyIdB = jr.a;
                d.localAnchorA = lb;
                d.localAnchorB = la;
                d.localAxisA = b2Normalize(bv(j.axis));
                d.referenceAngle = ref;
                d.enableSpring = j.enable_spring;
                d.hertz = j.hertz;
                d.dampingRatio = j.damping_ratio;
                d.enableLimit = j.enable_limit;
                d.lowerTranslation = j.lower;
                d.upperTranslation = j.upper;
                d.enableMotor = j.enable_motor;
                d.maxMotorForce = j.max_motor_force;
                d.motorSpeed = j.motor_speed;
                d.collideConnected = j.collide_connected;
                jr.id = b2CreatePrismaticJoint(im.world, &d);
                break;
            }
            case 3: {
                b2WeldJointDef d = b2DefaultWeldJointDef();
                d.bodyIdA = jr.a;
                d.bodyIdB = jr.b;
                d.localAnchorA = la;
                d.localAnchorB = lb;
                d.referenceAngle = -ref;
                d.linearHertz = j.enable_spring ? j.hertz : 0.0f;
                d.angularHertz = j.enable_spring ? j.hertz : 0.0f;
                d.linearDampingRatio = j.damping_ratio;
                d.angularDampingRatio = j.damping_ratio;
                d.collideConnected = j.collide_connected;
                jr.id = b2CreateWeldJoint(im.world, &d);
                break;
            }
            case 4: {
                // A wheel on the other body (the chassis): its suspension along `axis` in the
                // chassis's space; the motor turns this body, the wheel.
                b2WheelJointDef d = b2DefaultWheelJointDef();
                d.bodyIdA = jr.b;
                d.bodyIdB = jr.a;
                d.localAnchorA = lb;
                d.localAnchorB = la;
                d.localAxisA = b2Normalize(bv(j.axis));
                d.enableSpring = j.enable_spring;
                d.hertz = j.hertz;
                d.dampingRatio = j.damping_ratio;
                d.enableLimit = j.enable_limit;
                d.lowerTranslation = j.lower;
                d.upperTranslation = j.upper;
                d.enableMotor = j.enable_motor;
                d.maxMotorTorque = j.max_motor_force;
                d.motorSpeed = j.motor_speed;
                d.collideConnected = j.collide_connected;
                jr.id = b2CreateWheelJoint(im.world, &d);
                break;
            }
            default: {
                b2RevoluteJointDef d = b2DefaultRevoluteJointDef();
                d.bodyIdA = jr.a;
                d.bodyIdB = jr.b;
                d.localAnchorA = la;
                d.localAnchorB = lb;
                d.referenceAngle = -ref;
                d.enableSpring = j.enable_spring;
                d.hertz = j.hertz;
                d.dampingRatio = j.damping_ratio;
                d.enableLimit = j.enable_limit;
                d.lowerAngle = j.lower;
                d.upperAngle = j.upper;
                d.enableMotor = j.enable_motor;
                d.maxMotorTorque = j.max_motor_force;
                d.motorSpeed = j.motor_speed;
                d.collideConnected = j.collide_connected;
                jr.id = b2CreateRevoluteJoint(im.world, &d);
                break;
            }
        }
    });
    for (auto it = im.joints.begin(); it != im.joints.end();) {
        if (joints_seen.contains(it->first)) { ++it; continue; }
        if (b2Joint_IsValid(it->second.id)) b2DestroyJoint(it->second.id);
        it = im.joints.erase(it);
    }

    b2World_Step(im.world, dt, 4);
    ++im.steps;

    // What moved, written back.
    const b2BodyEvents moved = b2World_GetBodyEvents(im.world);
    std::vector<std::pair<world::EntityId, const b2BodyMoveEvent*>> writes;
    for (int i = 0; i < moved.moveCount; ++i) writes.push_back({untag(moved.moveEvents[i].userData), &moved.moveEvents[i]});
    std::sort(writes.begin(), writes.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
    for (const auto& [id, ev] : writes) {
        auto it = im.bodies.find(id);
        if (it == im.bodies.end() || it->second.collider_only || !w.alive(id)) continue;
        Impl::Body& b = it->second;
        world::Transform tr = *w.try_get<world::Transform>(id);
        const Vec2 pos{ev->transform.p.x, ev->transform.p.y};
        const float ang = b2Rot_GetAngle(ev->transform.q);
        tr.position.x = pos.x;
        tr.position.y = pos.y;
        tr.rotation = about_z(ang);
        w.set_typed<world::Transform>(id, tr);
        world::RigidBody2D rb = *w.try_get<world::RigidBody2D>(id);
        const b2Vec2 v = b2Body_GetLinearVelocity(b.id);
        rb.velocity = Vec2{v.x, v.y};
        rb.angular_velocity = b2Body_GetAngularVelocity(b.id);
        rb.awake = !ev->fellAsleep;
        w.set_typed<world::RigidBody2D>(id, rb);
        // What the engine wrote, so it is not taken for the script's next tick (read back, as the
        // components hold it).
        const world::Transform& kept = *w.try_get<world::Transform>(id);
        b.pos = Vec2{kept.position.x, kept.position.y};
        b.angle = angle_of(kept.rotation);
        b.vel = rb.velocity;
        b.spin = rb.angular_velocity;
        b.rb = rb;
    }

    // Touches, in the order Box2D reports them.
    const std::int64_t tick = w.tick_index();
    auto entity_of = [](b2ShapeId s) { return b2Shape_IsValid(s) ? untag(b2Shape_GetUserData(s)) : world::EntityId{0}; };
    const b2ContactEvents contacts = b2World_GetContactEvents(im.world);
    im.begun = contacts.beginCount;
    for (int i = 0; i < contacts.beginCount; ++i) {
        const b2ContactBeginTouchEvent& e = contacts.beginEvents[i];
        const world::EntityId a = entity_of(e.shapeIdA), b = entity_of(e.shapeIdB);
        if (!a || !b || !w.alive(a) || !w.alive(b)) continue;
        const b2Vec2 n = e.manifold.normal;
        const b2Vec2 p = e.manifold.pointCount > 0 ? e.manifold.points[0].point : b2Body_GetPosition(b2Shape_GetBody(e.shapeIdA));
        const b2Vec2 va = b2Body_GetLinearVelocity(b2Shape_GetBody(e.shapeIdA)), vb = b2Body_GetLinearVelocity(b2Shape_GetBody(e.shapeIdB));
        const float speed = std::fabs((va.x - vb.x) * n.x + (va.y - vb.y) * n.y);
        Json data{{"a", w.path(a)}, {"b", w.path(b)}, {"point", Json{{"x", p.x}, {"y", p.y}, {"z", 0.0}}}, {"normal", Json{{"x", n.x}, {"y", n.y}, {"z", 0.0}}}, {"speed", speed}, {"space", "2d"}};
        im.touching[{a, b}] = w.events().emit(tick, "collision.begin", a, std::move(data));
    }
    for (int i = 0; i < contacts.endCount; ++i) {
        const b2ContactEndTouchEvent& e = contacts.endEvents[i];
        const world::EntityId a = entity_of(e.shapeIdA), b = entity_of(e.shapeIdB);
        if (!a || !b) continue;
        auto it = im.touching.find({a, b});
        const std::uint64_t cause = it == im.touching.end() ? 0 : it->second;
        if (it != im.touching.end()) im.touching.erase(it);
        if (!w.alive(a) || !w.alive(b)) continue;
        w.events().emit(tick, "collision.end", a, Json{{"a", w.path(a)}, {"b", w.path(b)}, {"space", "2d"}}, cause);
    }
    const b2SensorEvents sensors = b2World_GetSensorEvents(im.world);
    for (int i = 0; i < sensors.beginCount; ++i) {
        const world::EntityId a = entity_of(sensors.beginEvents[i].sensorShapeId), b = entity_of(sensors.beginEvents[i].visitorShapeId);
        if (!a || !b || a == b || !w.alive(a) || !w.alive(b)) continue;
        im.touching[{a, b}] = w.events().emit(tick, "trigger.enter", a, Json{{"a", w.path(a)}, {"b", w.path(b)}, {"space", "2d"}});
    }
    for (int i = 0; i < sensors.endCount; ++i) {
        const world::EntityId a = entity_of(sensors.endEvents[i].sensorShapeId), b = entity_of(sensors.endEvents[i].visitorShapeId);
        if (!a || !b || a == b) continue;
        auto it = im.touching.find({a, b});
        const std::uint64_t cause = it == im.touching.end() ? 0 : it->second;
        if (it != im.touching.end()) im.touching.erase(it);
        if (!w.alive(a) || !w.alive(b)) continue;
        w.events().emit(tick, "trigger.exit", a, Json{{"a", w.path(a)}, {"b", w.path(b)}, {"space", "2d"}}, cause);
    }

    // Joints pulled past their strength break.
    std::vector<world::EntityId> broken;
    for (auto& [id, jr] : im.joints) {
        if (jr.def.break_force <= 0 || !b2Joint_IsValid(jr.id)) continue;
        const b2Vec2 f = b2Joint_GetConstraintForce(jr.id);
        const float force = std::sqrt(f.x * f.x + f.y * f.y);
        if (force > jr.def.break_force) {
            broken.push_back(id);
            w.events().emit(tick, "joint2d.broken", id, Json{{"path", w.path(id)}, {"force", force}});
        }
    }
    for (world::EntityId id : broken) {
        b2DestroyJoint(im.joints[id].id);
        im.joints.erase(id);
        (void)w.remove(id, "Joint2D");
    }
    im.step_ms = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
}

std::optional<Hit2D> Rigid2D::raycast(Vec2 from, Vec2 to, std::uint32_t mask) const {
    if (!b2World_IsValid(impl_->world)) return std::nullopt;
    b2QueryFilter filter = b2DefaultQueryFilter();
    filter.maskBits = mask;
    const b2RayResult r = b2World_CastRayClosest(impl_->world, bv(from), b2Vec2{to.x - from.x, to.y - from.y}, filter);
    if (!r.hit) return std::nullopt;
    Hit2D h;
    h.entity = untag(b2Shape_GetUserData(r.shapeId));
    h.point = Vec2{r.point.x, r.point.y};
    h.normal = Vec2{r.normal.x, r.normal.y};
    h.fraction = r.fraction;
    return h;
}

std::vector<world::EntityId> Rigid2D::overlap(Vec2 center, Vec2 half, float angle, float radius, std::uint32_t mask) const {
    std::vector<world::EntityId> found;
    if (!b2World_IsValid(impl_->world)) return found;
    b2QueryFilter filter = b2DefaultQueryFilter();
    filter.maskBits = mask;
    b2ShapeProxy proxy;
    if (radius > 0) {
        const b2Vec2 c = bv(center);
        proxy = b2MakeProxy(&c, 1, radius);
    } else {
        const b2Vec2 corners[4] = {{-half.x, -half.y}, {half.x, -half.y}, {half.x, half.y}, {-half.x, half.y}};
        proxy = b2MakeOffsetProxy(corners, 4, 0.0f, bv(center), b2MakeRot(angle));
    }
    auto collect = [](b2ShapeId s, void* ctx) -> bool {
        auto* out = static_cast<std::vector<world::EntityId>*>(ctx);
        const world::EntityId id = untag(b2Shape_GetUserData(s));
        if (std::find(out->begin(), out->end(), id) == out->end()) out->push_back(id);
        return true;
    };
    b2World_OverlapShape(impl_->world, &proxy, filter, collect, &found);
    std::sort(found.begin(), found.end());
    return found;
}

Result<std::pair<Vec2, float>> Rigid2D::impulse(world::EntityId id, Vec2 impulse, std::optional<Vec2> point, float angular) {
    auto it = impl_->bodies.find(id);
    if (it == impl_->bodies.end() || !b2Body_IsValid(it->second.id)) return fail("no_body", "entity {} has no 2D rigid body in the simulation yet (a RigidBody2D is made on the next step)", id);
    const b2BodyId b = it->second.id;
    if (point) b2Body_ApplyLinearImpulse(b, bv(impulse), bv(*point), true);
    else b2Body_ApplyLinearImpulseToCenter(b, bv(impulse), true);
    if (angular != 0) b2Body_ApplyAngularImpulse(b, angular, true);
    const b2Vec2 v = b2Body_GetLinearVelocity(b);
    return std::pair{Vec2{v.x, v.y}, b2Body_GetAngularVelocity(b)};
}

std::vector<Box2DView> Rigid2D::boxes() const {
    std::vector<Box2DView> out;
    if (!b2World_IsValid(impl_->world)) return out;
    for (const auto& [id, body] : impl_->bodies) {
        if (!b2Body_IsValid(body.id) || !body.has_shape) continue;
        const b2AABB box = b2Body_ComputeAABB(body.id);
        const b2Vec2 v = b2Body_GetLinearVelocity(body.id);
        out.push_back({id, Vec2{box.lowerBound.x, box.lowerBound.y}, Vec2{box.upperBound.x, box.upperBound.y}, Vec2{v.x, v.y}, b2Body_GetType(body.id) == b2_dynamicBody, body.col.sensor});
    }
    return out;
}

std::optional<Vec2> Rigid2D::push(world::EntityId id, Vec2 want) {
    auto it = impl_->bodies.find(id);
    if (it == impl_->bodies.end() || !b2Body_IsValid(it->second.id) || b2Body_GetType(it->second.id) != b2_dynamicBody) return std::nullopt;
    b2Vec2 v = b2Body_GetLinearVelocity(it->second.id);
    bool changed = false;
    if ((want.x > 0 && v.x < want.x) || (want.x < 0 && v.x > want.x)) { v.x = want.x; changed = true; }
    if ((want.y > 0 && v.y < want.y) || (want.y < 0 && v.y > want.y)) { v.y = want.y; changed = true; }
    if (changed) {
        b2Body_SetLinearVelocity(it->second.id, v);
        b2Body_SetAwake(it->second.id, true);
    }
    return Vec2{v.x, v.y};
}

Json Rigid2D::describe() const {
    const Impl& im = *impl_;
    Json j{{"active", b2World_IsValid(im.world)}, {"bodies", im.bodies.size()}, {"joints", im.joints.size()}, {"maps", im.tiles.size()}, {"tile_shapes", im.tile_shapes}, {"steps", im.steps}, {"contacts_begun", im.begun}, {"step_ms", im.step_ms}};
    if (b2World_IsValid(im.world)) {
        j["awake"] = b2World_GetAwakeBodyCount(im.world);
        const b2Counters c = b2World_GetCounters(im.world);
        j["shapes"] = c.shapeCount;
        j["contacts"] = c.contactCount;
    }
    if (!im.skipped_maps.empty()) j["maps_skipped"] = im.skipped_maps + " (only orthogonal maps collide with 2D rigid bodies)";
    return j;
}

}  // namespace pocket::physics
