// Cloth (docs/design/physics.md, Cloth).
#include <pocket/physics/cloth.hpp>
#include <pocket/core/parallel.hpp>
#include <pocket/world/wind.hpp>

#include <algorithm>
#include <cmath>

namespace pocket::physics {

namespace {

constexpr int kSubsteps = 2;
constexpr int kPasses = 6;   // constraint passes a substep

// A collider the sheet keeps out of, placed in the world.
struct Solid {
    int shape = 0;        // 0 box, 1 sphere, 2 capsule
    Vec3 center;
    Quat turn;
    Vec3 half;            // box half extents; radius in x; capsule half length in y
    Vec3 lo, hi;          // the axis-aligned box around it
};

// `p` pushed out of the solid to `margin` beyond its surface (unchanged when outside).
Vec3 push_out(const Solid& s, Vec3 p, float margin) {
    const Quat inv{-s.turn.x, -s.turn.y, -s.turn.z, s.turn.w};
    const Vec3 local = inv.rotate(p - s.center);
    if (s.shape == 1 || s.shape == 2) {
        const float r = s.half.x + margin;
        Vec3 axis{0, 0, 0};
        if (s.shape == 2) axis.y = std::clamp(local.y, -s.half.y, s.half.y);
        const Vec3 d = local - axis;
        const float len = length(d);
        if (len >= r) return p;
        const Vec3 out = len > 1e-6f ? d * (r / len) : Vec3{0, r, 0};
        return s.center + s.turn.rotate(axis + out);
    }
    const Vec3 e = s.half + Vec3{margin, margin, margin};
    if (std::fabs(local.x) >= e.x || std::fabs(local.y) >= e.y || std::fabs(local.z) >= e.z) return p;
    // Out through the nearest face.
    Vec3 q = local;
    const float dx = e.x - std::fabs(local.x), dy = e.y - std::fabs(local.y), dz = e.z - std::fabs(local.z);
    if (dx <= dy && dx <= dz) q.x = std::copysign(e.x, local.x);
    else if (dy <= dz) q.y = std::copysign(e.y, local.y);
    else q.z = std::copysign(e.z, local.z);
    return s.center + s.turn.rotate(q);
}

}  // namespace

void ClothRunner::make(world::World& w, assets::AssetStore& assets, world::EntityId id, const world::Cloth& c, const Mat4& placed, Sheet& s) {
    s = Sheet{};
    s.size = Vec2{std::max(c.size.x, 0.01f), std::max(c.size.y, 0.01f)};
    s.across = std::clamp(static_cast<int>(std::lround(c.segments.x)), 1, 48);
    s.down = std::clamp(static_cast<int>(std::lround(c.segments.y)), 1, 48);
    s.pin = c.pin;
    s.nx = s.across + 1;
    s.ny = s.down + 1;
    const std::size_t n = static_cast<std::size_t>(s.nx * s.ny);
    s.rest.resize(n);
    s.at.resize(n);
    s.pinned.assign(n, 0);
    for (int j = 0; j < s.ny; ++j) {
        for (int i = 0; i < s.nx; ++i) {
            const std::size_t k = static_cast<std::size_t>(j * s.nx + i);
            s.rest[k] = Vec3{-s.size.x * 0.5f + s.size.x * static_cast<float>(i) / static_cast<float>(s.across), -s.size.y * static_cast<float>(j) / static_cast<float>(s.down), 0};
            // A wrinkle of a millimetre out of the sheet's plane, the same every time: a sheet made
            // perfectly flat stays balanced in its plane and never folds over its pins.
            const Vec3 wrinkled = s.rest[k] + Vec3{0, 0, 0.001f * std::sin(1.7f * static_cast<float>(i) + 2.3f * static_cast<float>(j))};
            s.at[k] = placed.transform_point(wrinkled);
            const bool top = j == 0;
            s.pinned[k] = c.pin == 0 ? top : c.pin == 1 ? (top && (i == 0 || i == s.across)) : c.pin == 2 ? i == 0 : 0;
        }
    }
    s.was = s.at;
    // Along the weave, across it and two apart: it neither stretches, shears nor folds sharply.
    auto link = [&](int i0, int j0, int i1, int j1, float give) {
        if (i1 < 0 || i1 >= s.nx || j1 < 0 || j1 >= s.ny) return;
        const auto a = static_cast<std::uint32_t>(j0 * s.nx + i0), b = static_cast<std::uint32_t>(j1 * s.nx + i1);
        s.links.push_back(Link{a, b, length(placed.transform_point(s.rest[b]) - placed.transform_point(s.rest[a])), give});
    };
    // Cloth hardly stretches along its threads, shears more easily and bends easily. A pass's share
    // compounds over the passes (kPasses a substep): across the threads and over two, a few hundredths
    // a pass keep a third and a tenth of the shape a tick; along them, nearly all of it.
    for (int j = 0; j < s.ny; ++j) {
        for (int i = 0; i < s.nx; ++i) {
            link(i, j, i + 1, j, 1.0f);
            link(i, j, i, j + 1, 1.0f);
            link(i, j, i + 1, j + 1, 0.03f);
            link(i + 1, j, i, j + 1, 0.03f);
            link(i, j, i + 2, j, 0.01f);
            link(i, j, i, j + 2, 0.01f);
        }
    }
    s.mesh = "cloth:" + std::to_string(id);
    // Bounds wide enough for wherever it swings, given once: the boxes around it need not be made
    // again every tick it moves.
    const float reach = std::max(s.size.x, s.size.y);
    w.set_mesh_bounds(s.mesh, Vec3{-reach, -2.0f * reach, -reach}, Vec3{reach, reach, reach});
    w.set_derived_mesh(id, s.mesh);
    (void)assets;
}

void ClothRunner::draw(world::World& w, assets::AssetStore& assets, world::EntityId id, const Mat4& placed, const Sheet& s) {
    // Both sides, so it is seen and lit from either: the back a copy with its normals turned over
    // and its triangles wound the other way.
    const Mat4 into = placed.inverse_affine();
    const std::size_t n = s.at.size();
    assets::Mesh mesh;
    mesh.vertices.resize(2 * n);
    std::vector<Vec3> normal(n, Vec3{0, 0, 0});
    std::vector<Vec3> local(n);
    for (std::size_t k = 0; k < n; ++k) local[k] = into.transform_point(s.at[k]);
    std::vector<std::uint32_t> idx;
    idx.reserve(static_cast<std::size_t>(s.across * s.down) * 12);
    for (int j = 0; j < s.down; ++j) {
        for (int i = 0; i < s.across; ++i) {
            const auto a = static_cast<std::uint32_t>(j * s.nx + i), b = a + 1, c = a + static_cast<std::uint32_t>(s.nx), d = c + 1;
            const std::uint32_t tris[2][3] = {{a, c, b}, {b, c, d}};
            for (const auto& t : tris) {
                const Vec3 f = cross(local[t[1]] - local[t[0]], local[t[2]] - local[t[0]]);
                for (std::uint32_t v : t) normal[v] = normal[v] + f;
                idx.insert(idx.end(), {t[0], t[1], t[2]});
                idx.insert(idx.end(), {t[0] + static_cast<std::uint32_t>(n), t[2] + static_cast<std::uint32_t>(n), t[1] + static_cast<std::uint32_t>(n)});
            }
        }
    }
    mesh.aabb_min = mesh.aabb_max = local.empty() ? Vec3{0, 0, 0} : local[0];
    for (std::size_t k = 0; k < n; ++k) {
        const Vec3 nk = length(normal[k]) > 1e-12f ? normalize(normal[k]) : Vec3{0, 0, 1};
        const int i = static_cast<int>(k) % s.nx, j = static_cast<int>(k) / s.nx;
        const Vec2 uv{static_cast<float>(i) / static_cast<float>(s.across), static_cast<float>(j) / static_cast<float>(s.down)};
        assets::MeshVertex& front = mesh.vertices[k];
        front.position = local[k];
        front.normal = nk;
        front.uv = uv;
        assets::MeshVertex& back = mesh.vertices[k + n];
        back = front;
        back.normal = nk * -1.0f;
        mesh.aabb_min = Vec3{std::min(mesh.aabb_min.x, local[k].x), std::min(mesh.aabb_min.y, local[k].y), std::min(mesh.aabb_min.z, local[k].z)};
        mesh.aabb_max = Vec3{std::max(mesh.aabb_max.x, local[k].x), std::max(mesh.aabb_max.y, local[k].y), std::max(mesh.aabb_max.z, local[k].z)};
    }
    mesh.indices = std::move(idx);
    assets::Material m;
    m.name = "cloth";
    mesh.materials.push_back(m);
    assets::Submesh sm;
    sm.index_count = static_cast<std::uint32_t>(mesh.indices.size());
    mesh.submeshes.push_back(sm);
    assets::fill_tangents(mesh);
    assets.put_mesh(s.mesh, std::move(mesh));
    (void)w;
    (void)id;
}

void ClothRunner::step(world::World& w, assets::AssetStore& assets, float dt, Vec3 gravity) {
    std::vector<world::EntityId> ids;
    w.ecs().each([&](flecs::entity e, const world::Cloth&, const world::WorldTransform&) { ids.push_back(e.id()); });
    std::sort(ids.begin(), ids.end());
    // Sheets whose entity or Cloth went: their meshes go too.
    for (auto it = sheets_.begin(); it != sheets_.end();) {
        if (std::binary_search(ids.begin(), ids.end(), it->first)) {
            ++it;
            continue;
        }
        if (w.alive(it->first)) w.set_derived_mesh(it->first, "");
        assets.forget_mesh(it->second.mesh);
        it = sheets_.erase(it);
    }
    if (ids.empty()) return;
    const world::WindField wind = world::wind_field(w);
    const auto now = static_cast<float>(w.seconds());
    // The colliders around, gathered once for every sheet.
    std::vector<Solid> solids;
    bool solids_made = false;
    auto gather = [&]() {
        solids_made = true;
        w.ecs().each([&](flecs::entity e, const world::Collider& col, const world::WorldTransform& t) {
            if (col.is_trigger || col.shape > 2 || e.has<world::Cloth>()) return;
            Solid s;
            s.shape = col.shape;
            s.turn = t.rotation;
            s.center = t.position + t.rotation.rotate(col.offset);
            s.half = col.shape == 0 ? col.size : col.shape == 1 ? Vec3{col.size.x, 0, 0} : Vec3{col.size.x, col.size.y, 0};
            // The turned shape's own extent along each world axis (a wide floor is wide and thin,
            // not a ball as wide as it is).
            auto abs3 = [](Vec3 v) { return Vec3{std::fabs(v.x), std::fabs(v.y), std::fabs(v.z)}; };
            const Vec3 ax = abs3(s.turn.rotate(Vec3{1, 0, 0})), ay = abs3(s.turn.rotate(Vec3{0, 1, 0})), az = abs3(s.turn.rotate(Vec3{0, 0, 1}));
            const float r = col.size.x;
            const Vec3 ext = col.shape == 0 ? ax * col.size.x + ay * col.size.y + az * col.size.z : col.shape == 1 ? Vec3{r, r, r} : ay * col.size.y + Vec3{r, r, r};
            s.lo = s.center - ext;
            s.hi = s.center + ext;
            solids.push_back(s);
        });
    };
    // Made or remade where the Cloth changed, on this thread (the world and the assets are written);
    // then every sheet moved on its own, on several threads (each touches only its own particles
    // and reads what nothing writes meanwhile, so the result is the same whichever thread ran it);
    // then drawn, on this thread again.
    struct Job {
        world::EntityId id;
        const world::Cloth* c;
        Mat4 placed;
        Sheet* s;
        bool move;
    };
    std::vector<Job> jobs;
    for (world::EntityId id : ids) {
        const world::Cloth* c = w.try_get<world::Cloth>(id);
        const world::WorldTransform* wt = w.try_get<world::WorldTransform>(id);
        if (!c || !wt) continue;
        const Mat4 placed = Mat4::trs(wt->position, wt->rotation, wt->scale);
        Sheet& s = sheets_[id];
        const int across = std::clamp(static_cast<int>(std::lround(c->segments.x)), 1, 48), down = std::clamp(static_cast<int>(std::lround(c->segments.y)), 1, 48);
        if (s.nx == 0 || s.across != across || s.down != down || s.pin != c->pin || std::fabs(s.size.x - std::max(c->size.x, 0.01f)) > 1e-5f || std::fabs(s.size.y - std::max(c->size.y, 0.01f)) > 1e-5f) make(w, assets, id, *c, placed, s);
        const bool move = c->enabled && dt > 0;
        if (move && c->collide && !solids_made) gather();
        jobs.push_back(Job{id, c, placed, &s, move});
    }
    parallel_for(jobs.size(), [&](std::size_t j) {
        const Job& job = jobs[j];
        if (!job.move) return;
        const world::Cloth* c = job.c;
        const Mat4& placed = job.placed;
        Sheet& s = *job.s;
        const std::size_t n = s.at.size();
        const float h = dt / static_cast<float>(kSubsteps);
        const float mass = std::max(c->weight, 0.001f) / static_cast<float>(n);
        const float area = s.size.x * s.size.y / static_cast<float>(n);
        const float keep = 1.0f - std::clamp(c->damping, 0.0f, 1.0f) / static_cast<float>(kSubsteps);
        const float stiff = std::clamp(c->stiffness, 0.0f, 1.0f);
        // The solids near it this tick.
        Vec3 lo = s.at[0], hi = s.at[0];
        for (const Vec3& p : s.at) {
            lo = Vec3{std::min(lo.x, p.x), std::min(lo.y, p.y), std::min(lo.z, p.z)};
            hi = Vec3{std::max(hi.x, p.x), std::max(hi.y, p.y), std::max(hi.z, p.z)};
        }
        const float pad = std::max(s.size.x, s.size.y) * 0.5f + c->thickness;
        std::vector<const Solid*> near;
        if (c->collide) {
            for (const Solid& so : solids) {
                if (so.hi.x < lo.x - pad || so.lo.x > hi.x + pad || so.hi.y < lo.y - pad || so.lo.y > hi.y + pad || so.hi.z < lo.z - pad || so.lo.z > hi.z + pad) continue;
                near.push_back(&so);
            }
        }
        // A particle further than this outside a solid's box is outside the solid and its margin,
        // turned as it may be (twice the margin covers a turned box's corners).
        const float reach = 2.0f * c->thickness;
        std::vector<Vec3> normal(n);
        for (int sub = 0; sub < kSubsteps; ++sub) {
            // Normals for the wind: the sheet's surface at each particle.
            std::fill(normal.begin(), normal.end(), Vec3{0, 0, 0});
            for (int j = 0; j < s.down; ++j) {
                for (int i = 0; i < s.across; ++i) {
                    const std::size_t a = static_cast<std::size_t>(j * s.nx + i), b = a + 1, cc = a + static_cast<std::size_t>(s.nx), d = cc + 1;
                    const Vec3 f1 = cross(s.at[cc] - s.at[a], s.at[b] - s.at[a]), f2 = cross(s.at[cc] - s.at[b], s.at[d] - s.at[b]);
                    normal[a] = normal[a] + f1;
                    normal[b] = normal[b] + f1 + f2;
                    normal[cc] = normal[cc] + f1 + f2;
                    normal[d] = normal[d] + f2;
                }
            }
            for (std::size_t k = 0; k < n; ++k) {
                if (s.pinned[k]) {
                    s.was[k] = s.at[k];
                    s.at[k] = placed.transform_point(s.rest[k]);
                    continue;
                }
                const Vec3 v = (s.at[k] - s.was[k]) * (1.0f / h);
                Vec3 a = gravity;
                if (wind.on && c->wind > 0) {
                    // The air pushes on the face it meets, by how squarely it meets it (a sail), and
                    // drags along the face it slides over (what makes a flag stream out from its pole).
                    const Vec3 air = world::wind_velocity(wind, s.at[k].x, s.at[k].z, now) - v;
                    const Vec3 nk = length(normal[k]) > 1e-12f ? normalize(normal[k]) : Vec3{0, 0, 1};
                    const float across_face = dot(air, nk);
                    const Vec3 along_face = air - nk * across_face;
                    a = a + (nk * (across_face * std::fabs(across_face)) * 0.6f + along_face * (length(along_face) * 0.03f)) * (area * c->wind / mass);
                }
                const Vec3 next = s.at[k] + (s.at[k] - s.was[k]) * keep + a * (h * h);
                s.was[k] = s.at[k];
                s.at[k] = next;
            }
            for (int pass = 0; pass < kPasses; ++pass) {
                for (const Link& l : s.links) {
                    const Vec3 d = s.at[l.b] - s.at[l.a];
                    const float len = length(d);
                    if (len < 1e-9f) continue;
                    const Vec3 fix = d * ((len - l.rest) / len * stiff * l.give);
                    const bool pa = s.pinned[l.a] != 0, pb = s.pinned[l.b] != 0;
                    if (pa && pb) continue;
                    if (pa) s.at[l.b] = s.at[l.b] - fix;
                    else if (pb) s.at[l.a] = s.at[l.a] + fix;
                    else {
                        s.at[l.a] = s.at[l.a] + fix * 0.5f;
                        s.at[l.b] = s.at[l.b] - fix * 0.5f;
                    }
                }
                for (const Solid* so : near) {
                    for (std::size_t k = 0; k < n; ++k) {
                        const Vec3& q = s.at[k];
                        if (s.pinned[k] || q.x < so->lo.x - reach || q.x > so->hi.x + reach || q.y < so->lo.y - reach || q.y > so->hi.y + reach || q.z < so->lo.z - reach || q.z > so->hi.z + reach) continue;
                        s.at[k] = push_out(*so, q, c->thickness);
                    }
                }
            }
        }
    });
    for (const Job& job : jobs) draw(w, assets, job.id, job.placed, *job.s);
}

std::vector<Vec3> ClothRunner::particles(world::EntityId id) const {
    auto it = sheets_.find(id);
    return it == sheets_.end() ? std::vector<Vec3>{} : it->second.at;
}

}  // namespace pocket::physics
