#include <pocket/physics/physics.hpp>

#include <pocket/core/log.hpp>

#include <algorithm>
#include <array>
#include <cmath>
#include <format>
#include <map>
#include <set>
#include <string>
#include <tuple>

namespace pocket::physics {

namespace {

using world::EntityId;

struct MeshShape;

struct Body {
    EntityId id = 0;
    int kind = 0;  // 0 dynamic, 1 static, 2 kinematic
    int shape = 0; // 0 box, 1 sphere, 2 capsule (half.x radius, half.y half length of the segment), 3 mesh
    const MeshShape* mesh = nullptr;  // shape 3: the triangles (null when the file is missing)
    bool trigger = false;
    bool lock_rotation = false;
    bool sleeping = false;
    bool ccd = false;      // sweep along the motion before integrating (fast small bodies)
    bool swept = false;    // this step's motion was already applied by the sweep
    int group = 0;         // same negative group: never collide; same positive: always; 0: the layers decide
    std::uint32_t layer = 1, mask = 0xFFFFFFFFu;  // collision layers: a pair interacts when each is on a layer the other's mask includes
    float inv_mass = 0;
    float restitution = 0, friction = 0;
    float linear_damping = 0, angular_damping = 0, gravity_scale = 1;
    Vec3 position, half;   // half extents (box) or radius in x (sphere)
    Quat rotation;
    Vec3 velocity, angular;
    Vec3 aabb_min, aabb_max;
    Mat4 inv_inertia_world;  // 3x3 in a Mat4
    float sleep_timer = 0;
    bool touched = false;
};

struct Manifold {
    std::size_t a = 0, b = 0;
    Vec3 normal;
    std::vector<Vec3> points;
    std::vector<float> depths;
    bool trigger = false;
};

Vec3 mul3(const Mat4& m, Vec3 v) { return m.transform_dir(v); }
Quat conj(Quat q) { return {-q.x, -q.y, -q.z, q.w}; }  // the inverse of a unit quaternion
Vec3 any_perpendicular(Vec3 v) { return normalize(cross(v, std::fabs(v.x) < 0.9f ? Vec3{1, 0, 0} : Vec3{0, 1, 0})); }

// The capsule's segment end points in world space.
void capsule_segment(const Body& b, Vec3& p0, Vec3& p1) {
    Vec3 axis = b.rotation.rotate(Vec3{0, 1, 0}) * b.half.y;
    p0 = b.position - axis;
    p1 = b.position + axis;
}

Vec3 closest_on_segment(Vec3 p, Vec3 a, Vec3 b) {
    Vec3 ab = b - a;
    float len2 = dot(ab, ab);
    if (len2 <= 1e-12f) return a;
    float t = std::clamp(dot(p - a, ab) / len2, 0.0f, 1.0f);
    return a + ab * t;
}

// Closest points between two segments (Ericson, Real-Time Collision Detection 5.1.9).
void closest_segments(Vec3 p1, Vec3 q1, Vec3 p2, Vec3 q2, Vec3& c1, Vec3& c2) {
    Vec3 d1 = q1 - p1, d2 = q2 - p2, r = p1 - p2;
    float a = dot(d1, d1), e = dot(d2, d2), f = dot(d2, r);
    float s = 0, t = 0;
    const float eps = 1e-8f;
    if (a <= eps && e <= eps) { c1 = p1; c2 = p2; return; }
    if (a <= eps) {
        t = std::clamp(f / e, 0.0f, 1.0f);
    } else {
        float c = dot(d1, r);
        if (e <= eps) {
            s = std::clamp(-c / a, 0.0f, 1.0f);
        } else {
            float b = dot(d1, d2);
            float denom = a * e - b * b;
            s = denom != 0 ? std::clamp((b * f - c * e) / denom, 0.0f, 1.0f) : 0.0f;
            t = (b * s + f) / e;
            if (t < 0) { t = 0; s = std::clamp(-c / a, 0.0f, 1.0f); }
            else if (t > 1) { t = 1; s = std::clamp((b - c) / a, 0.0f, 1.0f); }
        }
    }
    c1 = p1 + d1 * s;
    c2 = p2 + d2 * t;
}

Vec3 vmin3(Vec3 a, Vec3 b) { return {std::min(a.x, b.x), std::min(a.y, b.y), std::min(a.z, b.z)}; }
Vec3 vmax3(Vec3 a, Vec3 b) { return {std::max(a.x, b.x), std::max(a.y, b.y), std::max(a.z, b.z)}; }

// A mesh collider: the asset's triangles in world space (the entity's transform and scale applied)
// with a bounding-volume tree over them. Built once and kept until the transform or file changes.
struct MeshShape {
    std::string key;                                  // path and transform it was built for
    std::vector<Vec3> v;                              // world-space vertices
    std::vector<std::array<std::uint32_t, 3>> tri;
    std::vector<Vec3> n;                              // unit normals from the winding: the front side
    struct Node { Vec3 min, max; std::uint32_t first = 0, count = 0, left = 0, right = 0; };
    std::vector<Node> nodes;                          // nodes[0] is the root; leaves have count > 0
    std::vector<std::uint32_t> order;                 // triangle indices, leaves cover ranges of it
    Vec3 aabb_min, aabb_max;

    void clear() {
        v.clear();
        tri.clear();
        n.clear();
        nodes.clear();
        order.clear();
        aabb_min = aabb_max = {0, 0, 0};
    }

    void build(const assets::Mesh& mesh, Vec3 position, Quat rotation, Vec3 scale) {
        clear();
        v.reserve(mesh.vertices.size());
        for (const assets::MeshVertex& mv : mesh.vertices) {
            v.push_back(position + rotation.rotate(Vec3{mv.position.x * scale.x, mv.position.y * scale.y, mv.position.z * scale.z}));
        }
        for (const assets::Submesh& sm : mesh.submeshes) {
            if (sm.skin >= 0) continue;  // skinned geometry moves with its joints: no collision
            for (std::uint32_t i = sm.first_index; i + 2 < sm.first_index + sm.index_count; i += 3) {
                std::array<std::uint32_t, 3> t{mesh.indices[i], mesh.indices[i + 1], mesh.indices[i + 2]};
                if (t[0] >= v.size() || t[1] >= v.size() || t[2] >= v.size()) continue;
                Vec3 nn = cross(v[t[1]] - v[t[0]], v[t[2]] - v[t[0]]);
                float len = length(nn);
                if (len <= 1e-12f) continue;  // degenerate
                tri.push_back(t);
                n.push_back(nn * (1.0f / len));
            }
        }
        if (tri.empty()) return;
        std::vector<Vec3> centroid(tri.size());
        order.resize(tri.size());
        for (std::uint32_t i = 0; i < tri.size(); ++i) {
            order[i] = i;
            centroid[i] = (v[tri[i][0]] + v[tri[i][1]] + v[tri[i][2]]) * (1.0f / 3.0f);
        }
        nodes.reserve(tri.size() / 4 + 1);
        build_node(0, static_cast<std::uint32_t>(tri.size()), centroid);
        aabb_min = nodes[0].min;
        aabb_max = nodes[0].max;
    }

    std::uint32_t build_node(std::uint32_t first, std::uint32_t count, const std::vector<Vec3>& centroid) {
        Node node;
        node.min = {1e30f, 1e30f, 1e30f};
        node.max = {-1e30f, -1e30f, -1e30f};
        for (std::uint32_t k = first; k < first + count; ++k) {
            for (std::uint32_t j = 0; j < 3; ++j) {
                const Vec3& p = v[tri[order[k]][j]];
                node.min = vmin3(node.min, p);
                node.max = vmax3(node.max, p);
            }
        }
        auto index = static_cast<std::uint32_t>(nodes.size());
        nodes.push_back(node);
        if (count <= 8) {
            nodes[index].first = first;
            nodes[index].count = count;
            return index;
        }
        Vec3 ext = node.max - node.min;
        int axis = ext.x >= ext.y && ext.x >= ext.z ? 0 : ext.y >= ext.z ? 1 : 2;
        std::uint32_t mid = first + count / 2;
        // A strict total order (ties by index) keeps the split reproducible.
        std::nth_element(order.begin() + first, order.begin() + mid, order.begin() + first + count, [&](std::uint32_t a, std::uint32_t b) {
            float ca = (&centroid[a].x)[axis], cb = (&centroid[b].x)[axis];
            return ca < cb || (ca == cb && a < b);
        });
        std::uint32_t left = build_node(first, mid - first, centroid);
        std::uint32_t right = build_node(mid, first + count - mid, centroid);
        nodes[index].left = left;
        nodes[index].right = right;
        return index;
    }

    // Every triangle whose leaf box overlaps [qmin, qmax], in tree order.
    template <class F>
    void query(Vec3 qmin, Vec3 qmax, F&& f) const {
        if (nodes.empty()) return;
        std::vector<std::uint32_t> stack{0};
        while (!stack.empty()) {
            const Node& node = nodes[stack.back()];
            stack.pop_back();
            if (node.max.x < qmin.x || node.min.x > qmax.x || node.max.y < qmin.y || node.min.y > qmax.y || node.max.z < qmin.z || node.min.z > qmax.z) continue;
            if (node.count > 0) {
                for (std::uint32_t k = node.first; k < node.first + node.count; ++k) f(order[k]);
            } else {
                stack.push_back(node.left);
                stack.push_back(node.right);
            }
        }
    }
};

bool ray_aabb(Vec3 o, Vec3 d, Vec3 mn, Vec3 mx, float tmax) {
    float t0 = 0, t1 = tmax;
    for (int i = 0; i < 3; ++i) {
        float oi = (&o.x)[i], di = (&d.x)[i], lo = (&mn.x)[i], hi = (&mx.x)[i];
        if (std::fabs(di) < 1e-12f) {
            if (oi < lo || oi > hi) return false;
            continue;
        }
        float a = (lo - oi) / di, b = (hi - oi) / di;
        if (a > b) std::swap(a, b);
        t0 = std::max(t0, a);
        t1 = std::min(t1, b);
        if (t0 > t1) return false;
    }
    return true;
}

// Moller-Trumbore, both sides: t along the ray to the triangle's plane inside abc.
bool ray_triangle(Vec3 o, Vec3 d, Vec3 a, Vec3 b, Vec3 c, float& t) {
    Vec3 e1 = b - a, e2 = c - a;
    Vec3 p = cross(d, e2);
    float det = dot(e1, p);
    if (std::fabs(det) < 1e-12f) return false;
    float inv = 1.0f / det;
    Vec3 s = o - a;
    float u = dot(s, p) * inv;
    if (u < 0 || u > 1) return false;
    Vec3 q = cross(s, e1);
    float w = dot(d, q) * inv;
    if (w < 0 || u + w > 1) return false;
    t = dot(e2, q) * inv;
    return t >= 0;
}

// Closest point on triangle abc to p (Ericson, Real-Time Collision Detection 5.1.5).
Vec3 closest_on_triangle(Vec3 p, Vec3 a, Vec3 b, Vec3 c) {
    Vec3 ab = b - a, ac = c - a, ap = p - a;
    float d1 = dot(ab, ap), d2 = dot(ac, ap);
    if (d1 <= 0 && d2 <= 0) return a;
    Vec3 bp = p - b;
    float d3 = dot(ab, bp), d4 = dot(ac, bp);
    if (d3 >= 0 && d4 <= d3) return b;
    float vc = d1 * d4 - d3 * d2;
    if (vc <= 0 && d1 >= 0 && d3 <= 0) return a + ab * (d1 / (d1 - d3));
    Vec3 cp = p - c;
    float d5 = dot(ab, cp), d6 = dot(ac, cp);
    if (d6 >= 0 && d5 <= d6) return c;
    float vb = d5 * d2 - d1 * d6;
    if (vb <= 0 && d2 >= 0 && d6 <= 0) return a + ac * (d2 / (d2 - d6));
    float va = d3 * d6 - d5 * d4;
    if (va <= 0 && (d4 - d3) >= 0 && (d5 - d6) >= 0) return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    float denom = 1.0f / (va + vb + vc);
    return a + ab * (vb * denom) + ac * (vc * denom);
}

// Does p project into triangle abc (normal n) along n?
bool inside_prism(Vec3 p, Vec3 a, Vec3 b, Vec3 c, Vec3 n) {
    return dot(cross(b - a, p - a), n) >= -1e-6f && dot(cross(c - b, p - b), n) >= -1e-6f && dot(cross(a - c, p - c), n) >= -1e-6f;
}

struct MeshHit {
    Vec3 n, p;      // normal from the probe into the mesh, contact point on the mesh
    float depth;
    bool corner;    // a box corner under the face (preferred over fallback points)
};

// Sphere (c, r) against triangle t: the closest point and the front-side rule. A center that has
// dipped under the face by less than a radius is pushed back out along the face normal, so a fast
// marble does not fall through a floor; a center behind a face by more is on the other side.
bool sphere_triangle(Vec3 c, float r, const MeshShape& ms, std::uint32_t t, MeshHit& out) {
    const Vec3& a = ms.v[ms.tri[t][0]];
    const Vec3& b = ms.v[ms.tri[t][1]];
    const Vec3& cc = ms.v[ms.tri[t][2]];
    const Vec3& n = ms.n[t];
    float side = dot(c - a, n);
    if (side < -r) return false;
    if (side < 0) {
        if (!inside_prism(c, a, b, cc, n)) return false;
        out = {-n, c - n * side, r - side, false};
        return true;
    }
    Vec3 q = closest_on_triangle(c, a, b, cc);
    Vec3 d = c - q;
    float dist2 = dot(d, d);
    if (dist2 >= r * r) return false;
    float dist = std::sqrt(dist2);
    Vec3 away = dist > 1e-6f ? d * (1.0f / dist) : n;
    out = {-away, q, r - dist, false};
    return true;
}

// Capsule against a triangle: its ends and the segment point nearest the triangle, as spheres.
void capsule_triangle(const Body& cap, const MeshShape& ms, std::uint32_t t, std::vector<MeshHit>& hits) {
    Vec3 p0, p1;
    capsule_segment(cap, p0, p1);
    const Vec3& a = ms.v[ms.tri[t][0]];
    const Vec3& b = ms.v[ms.tri[t][1]];
    const Vec3& c = ms.v[ms.tri[t][2]];
    Vec3 cands[6] = {p0, p1};
    int nc = 2;
    const Vec3 edges[3][2] = {{a, b}, {b, c}, {c, a}};
    for (const auto& e : edges) {
        Vec3 c1, c2;
        closest_segments(p0, p1, e[0], e[1], c1, c2);
        cands[nc++] = c1;
    }
    float tt = 0;
    Vec3 d = p1 - p0;
    if (ray_triangle(p0, d, a, b, c, tt) && tt <= 1) cands[nc++] = p0 + d * tt;
    Vec3 best = cands[0];
    float best_d = 1e30f;
    for (int i = 0; i < nc; ++i) {
        Vec3 q = closest_on_triangle(cands[i], a, b, c);
        float dd = dot(cands[i] - q, cands[i] - q);
        if (dd < best_d) { best_d = dd; best = cands[i]; }
    }
    for (const Vec3& pt : {p0, p1, best}) {
        MeshHit h;
        if (sphere_triangle(pt, cap.half.x, ms, t, h)) hits.push_back(h);
    }
}

// Box against a triangle: separating axes in the box's frame, then the box corners under the face
// and inside it as contacts; failing those (a triangle corner or edge poking in), the box's
// deepest corner along the face normal.
void box_triangle(const Body& box, const MeshShape& ms, std::uint32_t t, std::vector<MeshHit>& hits) {
    Mat4 rot = Mat4::rotation(box.rotation);
    Vec3 ax[3];
    for (int i = 0; i < 3; ++i) ax[i] = {rot.at(i, 0), rot.at(i, 1), rot.at(i, 2)};
    const Vec3& n = ms.n[t];
    Vec3 lv[3];
    for (int k = 0; k < 3; ++k) {
        Vec3 rel = ms.v[ms.tri[t][k]] - box.position;
        lv[k] = {dot(rel, ax[0]), dot(rel, ax[1]), dot(rel, ax[2])};
    }
    const Vec3 ln{dot(n, ax[0]), dot(n, ax[1]), dot(n, ax[2])};
    const Vec3 h = box.half;
    const float ext_n = h.x * std::fabs(ln.x) + h.y * std::fabs(ln.y) + h.z * std::fabs(ln.z);
    const float center_side = -dot(lv[0], ln);  // the box center's height over the face
    if (center_side > ext_n || center_side < -ext_n) return;  // clear of the face, or behind it
    auto separated = [&](Vec3 axis) {
        if (dot(axis, axis) < 1e-12f) return false;
        float r = h.x * std::fabs(axis.x) + h.y * std::fabs(axis.y) + h.z * std::fabs(axis.z);
        float p0 = dot(lv[0], axis), p1 = dot(lv[1], axis), p2 = dot(lv[2], axis);
        return std::min({p0, p1, p2}) > r || std::max({p0, p1, p2}) < -r;
    };
    const Vec3 unit[3] = {{1, 0, 0}, {0, 1, 0}, {0, 0, 1}};
    for (const Vec3& u : unit) if (separated(u)) return;
    const Vec3 e[3] = {lv[1] - lv[0], lv[2] - lv[1], lv[0] - lv[2]};
    for (const Vec3& edge : e) for (const Vec3& u : unit) if (separated(cross(edge, u))) return;
    bool any = false;
    for (int k = 0; k < 8; ++k) {
        Vec3 corner{(k & 1) ? h.x : -h.x, (k & 2) ? h.y : -h.y, (k & 4) ? h.z : -h.z};
        float s = dot(corner - lv[0], ln);
        if (s >= 0 || !inside_prism(corner, lv[0], lv[1], lv[2], ln)) continue;
        Vec3 world_corner = box.position + ax[0] * corner.x + ax[1] * corner.y + ax[2] * corner.z;
        hits.push_back({-n, world_corner - n * s, -s, true});
        any = true;
    }
    if (any) return;
    float depth = ext_n - center_side;
    if (depth <= 0) return;
    Vec3 deepest = box.position;
    for (int i = 0; i < 3; ++i) deepest -= ax[i] * ((&h.x)[i] * ((&ln.x)[i] >= 0 ? 1.0f : -1.0f));
    hits.push_back({-n, deepest + n * depth, depth, false});
}

// A sphere, capsule or box (probe) against a mesh body: every triangle its box touches, merged
// into one manifold around the deepest contact (box corners first).
bool collide_mesh(const Body& probe, const Body& mb, Manifold& m, bool flip) {
    if (!mb.mesh) return false;
    const MeshShape& ms = *mb.mesh;
    std::vector<MeshHit> hits;
    ms.query(probe.aabb_min, probe.aabb_max, [&](std::uint32_t t) {
        if (probe.shape == 1) {
            MeshHit h;
            if (sphere_triangle(probe.position, probe.half.x, ms, t, h)) hits.push_back(h);
        } else if (probe.shape == 2) {
            capsule_triangle(probe, ms, t, hits);
        } else {
            box_triangle(probe, ms, t, hits);
        }
    });
    if (hits.empty()) return false;
    std::stable_sort(hits.begin(), hits.end(), [](const MeshHit& x, const MeshHit& y) { return x.corner != y.corner ? x.corner : x.depth > y.depth; });
    Vec3 n = hits[0].n;  // from the probe into the mesh
    m.normal = flip ? -n : n;
    for (const MeshHit& h : hits) {
        if (dot(h.n, n) < 0.95f) continue;
        bool duplicate = false;
        for (const Vec3& q : m.points) if (length(q - h.p) < 1e-3f) duplicate = true;
        if (duplicate) continue;
        m.points.push_back(h.p);
        m.depths.push_back(h.depth);
        if (m.points.size() >= 4) break;
    }
    return true;
}
Mat4 inertia_inverse(const Body& b) {
    Mat4 r;
    for (float& v : r.m) v = 0;
    if (b.inv_mass == 0 || b.lock_rotation) return r;
    float mass = 1.0f / b.inv_mass;
    float ix, iy, iz;
    if (b.shape == 1) {
        float rr = b.half.x * b.half.x;
        ix = iy = iz = 0.4f * mass * rr;
    } else if (b.shape == 2) {
        // A solid cylinder of the capsule's full height is close enough.
        float rr = b.half.x * b.half.x;
        float hh = 2 * (b.half.y + b.half.x);
        ix = iz = mass * (3 * rr + hh * hh) / 12.0f;
        iy = mass * rr / 2.0f;
    } else {
        Vec3 half = b.half;
        if (b.shape == 3) half = (b.aabb_max - b.aabb_min) * 0.5f;  // a mesh tumbles as its bounding box
        float w = 2 * half.x, h = 2 * half.y, d = 2 * half.z;
        ix = mass * (h * h + d * d) / 12.0f;
        iy = mass * (w * w + d * d) / 12.0f;
        iz = mass * (w * w + h * h) / 12.0f;
    }
    Mat4 local;
    for (float& v : local.m) v = 0;
    local.at(0, 0) = ix > 0 ? 1.0f / ix : 0;
    local.at(1, 1) = iy > 0 ? 1.0f / iy : 0;
    local.at(2, 2) = iz > 0 ? 1.0f / iz : 0;
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 rot_t;
    for (int c = 0; c < 4; ++c)
        for (int row = 0; row < 4; ++row) rot_t.at(c, row) = rot.at(row, c);
    return rot * local * rot_t;
}

void update_aabb(Body& b) {
    if (b.shape == 3) {
        if (b.mesh) {
            b.aabb_min = b.mesh->aabb_min;
            b.aabb_max = b.mesh->aabb_max;
        } else {
            b.aabb_min = b.aabb_max = b.position;
        }
        return;
    }
    if (b.shape == 1) {
        float r = b.half.x;
        b.aabb_min = b.position - Vec3{r, r, r};
        b.aabb_max = b.position + Vec3{r, r, r};
        return;
    }
    if (b.shape == 2) {
        Vec3 p0, p1;
        capsule_segment(b, p0, p1);
        float r = b.half.x;
        b.aabb_min = Vec3{std::min(p0.x, p1.x) - r, std::min(p0.y, p1.y) - r, std::min(p0.z, p1.z) - r};
        b.aabb_max = Vec3{std::max(p0.x, p1.x) + r, std::max(p0.y, p1.y) + r, std::max(p0.z, p1.z) + r};
        return;
    }
    Mat4 rot = Mat4::rotation(b.rotation);
    Vec3 ex{0, 0, 0};
    for (int i = 0; i < 3; ++i) {
        Vec3 axis{rot.at(i, 0), rot.at(i, 1), rot.at(i, 2)};
        float e = (&b.half.x)[i];
        ex += Vec3{std::fabs(axis.x) * e, std::fabs(axis.y) * e, std::fabs(axis.z) * e};
    }
    b.aabb_min = b.position - ex;
    b.aabb_max = b.position + ex;
}

bool aabb_overlap(const Body& a, const Body& b) {
    return a.aabb_min.x <= b.aabb_max.x && a.aabb_max.x >= b.aabb_min.x && a.aabb_min.y <= b.aabb_max.y && a.aabb_max.y >= b.aabb_min.y && a.aabb_min.z <= b.aabb_max.z && a.aabb_max.z >= b.aabb_min.z;
}

// Sphere vs sphere.
bool collide_ss(const Body& a, const Body& b, Manifold& m) {
    Vec3 d = b.position - a.position;
    float dist = length(d);
    float r = a.half.x + b.half.x;
    if (dist >= r) return false;
    m.normal = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
    m.points.push_back(a.position + m.normal * a.half.x);
    m.depths.push_back(r - dist);
    return true;
}

// A sphere (center, radius r) against box b: the normal points from the box toward the sphere.
bool sphere_box(Vec3 center, float r, const Body& b, Vec3& normal_world, Vec3& point, float& depth) {
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 inv = rot.inverse_affine();
    Vec3 local = inv.transform_dir(center - b.position);
    Vec3 clamped{std::clamp(local.x, -b.half.x, b.half.x), std::clamp(local.y, -b.half.y, b.half.y), std::clamp(local.z, -b.half.z, b.half.z)};
    Vec3 diff = local - clamped;
    float dist2 = dot(diff, diff);
    Vec3 normal_local;
    if (dist2 > r * r) return false;
    if (dist2 > 1e-8f) {
        float dist = std::sqrt(dist2);
        normal_local = diff * (1.0f / dist);
        depth = r - dist;
    } else {
        float dx = b.half.x - std::fabs(local.x), dy = b.half.y - std::fabs(local.y), dz = b.half.z - std::fabs(local.z);
        if (dx <= dy && dx <= dz) { normal_local = {local.x < 0 ? -1.0f : 1.0f, 0, 0}; depth = dx + r; }
        else if (dy <= dz) { normal_local = {0, local.y < 0 ? -1.0f : 1.0f, 0}; depth = dy + r; }
        else { normal_local = {0, 0, local.z < 0 ? -1.0f : 1.0f}; depth = dz + r; }
    }
    normal_world = normalize(rot.transform_dir(normal_local));
    point = b.position + rot.transform_dir(clamped);
    return true;
}

// Capsule (a) vs sphere (b).
bool collide_cs(const Body& a, const Body& b, Manifold& m, bool flip) {
    Vec3 p0, p1;
    capsule_segment(a, p0, p1);
    Vec3 c = closest_on_segment(b.position, p0, p1);
    Vec3 d = b.position - c;
    float dist = length(d);
    float r = a.half.x + b.half.x;
    if (dist >= r) return false;
    Vec3 n = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};  // from capsule toward sphere
    m.normal = flip ? -n : n;
    m.points.push_back(c + n * a.half.x);
    m.depths.push_back(r - dist);
    return true;
}

// Capsule vs capsule.
bool collide_cc(const Body& a, const Body& b, Manifold& m) {
    Vec3 a0, a1, b0, b1, ca, cb;
    capsule_segment(a, a0, a1);
    capsule_segment(b, b0, b1);
    closest_segments(a0, a1, b0, b1, ca, cb);
    Vec3 d = cb - ca;
    float dist = length(d);
    float r = a.half.x + b.half.x;
    if (dist >= r) return false;
    m.normal = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
    m.points.push_back(ca + m.normal * a.half.x);
    m.depths.push_back(r - dist);
    return true;
}

// Capsule (a) vs box (b): the segment's ends and its point nearest the box center, each as a
// sphere; points that agree with the deepest normal make the manifold (two when it lies flat).
bool collide_cb(const Body& a, const Body& b, Manifold& m, bool flip) {
    Vec3 p0, p1;
    capsule_segment(a, p0, p1);
    Vec3 candidates[3] = {p0, p1, closest_on_segment(b.position, p0, p1)};
    struct Hit { Vec3 n, p; float depth; };
    std::vector<Hit> hits;
    for (const Vec3& c : candidates) {
        Vec3 n, p;
        float depth;
        if (sphere_box(c, a.half.x, b, n, p, depth)) hits.push_back({n, p, depth});
    }
    if (hits.empty()) return false;
    std::size_t deepest = 0;
    for (std::size_t i = 1; i < hits.size(); ++i) if (hits[i].depth > hits[deepest].depth) deepest = i;
    Vec3 n = hits[deepest].n;  // from box toward capsule
    m.normal = flip ? n : -n;
    for (std::size_t i = 0; i < hits.size(); ++i) {
        if (i != deepest && dot(hits[i].n, n) < 0.95f) continue;
        bool duplicate = false;
        for (const Vec3& q : m.points) if (length(q - hits[i].p) < 1e-4f) duplicate = true;
        if (duplicate) continue;
        m.points.push_back(hits[i].p);
        m.depths.push_back(hits[i].depth);
    }
    return true;
}

// Sphere (a) vs box (b).
bool collide_sb(const Body& a, const Body& b, Manifold& m, bool flip) {
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 inv = rot.inverse_affine();
    Vec3 local = inv.transform_dir(a.position - b.position);
    Vec3 clamped{std::clamp(local.x, -b.half.x, b.half.x), std::clamp(local.y, -b.half.y, b.half.y), std::clamp(local.z, -b.half.z, b.half.z)};
    Vec3 diff = local - clamped;
    float dist2 = dot(diff, diff);
    float r = a.half.x;
    Vec3 normal_local;
    float depth;
    if (dist2 > r * r) return false;
    if (dist2 > 1e-8f) {
        float dist = std::sqrt(dist2);
        normal_local = diff * (1.0f / dist);
        depth = r - dist;
    } else {
        // Center inside the box: push out along the axis of least penetration.
        float dx = b.half.x - std::fabs(local.x), dy = b.half.y - std::fabs(local.y), dz = b.half.z - std::fabs(local.z);
        if (dx <= dy && dx <= dz) { normal_local = {local.x < 0 ? -1.0f : 1.0f, 0, 0}; depth = dx + r; }
        else if (dy <= dz) { normal_local = {0, local.y < 0 ? -1.0f : 1.0f, 0}; depth = dy + r; }
        else { normal_local = {0, 0, local.z < 0 ? -1.0f : 1.0f}; depth = dz + r; }
    }
    Vec3 normal_world = normalize(rot.transform_dir(normal_local));  // from box toward sphere
    Vec3 point = b.position + rot.transform_dir(clamped);
    // Manifold normal must point from a to b.
    m.normal = flip ? normal_world : -normal_world;
    m.points.push_back(point);
    m.depths.push_back(depth);
    return true;
}

// Box vs box, SAT with 15 axes; contact points from the incident face clipped against the reference face.
bool collide_bb(const Body& a, const Body& b, Manifold& m) {
    Mat4 ra = Mat4::rotation(a.rotation), rb = Mat4::rotation(b.rotation);
    Vec3 axes_a[3], axes_b[3];
    for (int i = 0; i < 3; ++i) {
        axes_a[i] = {ra.at(i, 0), ra.at(i, 1), ra.at(i, 2)};
        axes_b[i] = {rb.at(i, 0), rb.at(i, 1), rb.at(i, 2)};
    }
    Vec3 d = b.position - a.position;
    float best_depth = 1e30f;
    Vec3 best_axis;
    auto project = [](const Body& box, const Vec3 axes[3], Vec3 axis) {
        return std::fabs(dot(axes[0], axis)) * box.half.x + std::fabs(dot(axes[1], axis)) * box.half.y + std::fabs(dot(axes[2], axis)) * box.half.z;
    };
    auto test = [&](Vec3 axis) -> bool {
        float len = length(axis);
        if (len < 1e-6f) return true;
        axis = axis * (1.0f / len);
        float dist = std::fabs(dot(d, axis));
        float overlap = project(a, axes_a, axis) + project(b, axes_b, axis) - dist;
        if (overlap < 0) return false;
        if (overlap < best_depth) {
            best_depth = overlap;
            best_axis = dot(d, axis) < 0 ? -axis : axis;
        }
        return true;
    };
    for (int i = 0; i < 3; ++i) if (!test(axes_a[i])) return false;
    for (int i = 0; i < 3; ++i) if (!test(axes_b[i])) return false;
    for (int i = 0; i < 3; ++i)
        for (int j = 0; j < 3; ++j)
            if (!test(cross(axes_a[i], axes_b[j]))) return false;
    m.normal = best_axis;  // from a to b
    // Contact points: vertices of each box inside the other, in the direction of the normal.
    auto corners = [](const Body& box, const Vec3 axes[3], std::vector<Vec3>& out) {
        for (int i = 0; i < 8; ++i) {
            Vec3 c = box.position + axes[0] * ((i & 1) ? box.half.x : -box.half.x) + axes[1] * ((i & 2) ? box.half.y : -box.half.y) + axes[2] * ((i & 4) ? box.half.z : -box.half.z);
            out.push_back(c);
        }
    };
    auto inside = [&](const Body& box, const Vec3 axes[3], Vec3 p, float tol) {
        Vec3 l = p - box.position;
        return std::fabs(dot(l, axes[0])) <= box.half.x + tol && std::fabs(dot(l, axes[1])) <= box.half.y + tol && std::fabs(dot(l, axes[2])) <= box.half.z + tol;
    };
    std::vector<Vec3> ca, cb;
    corners(a, axes_a, ca);
    corners(b, axes_b, cb);
    float tol = best_depth + 1e-4f;
    for (const Vec3& p : cb) {
        if (inside(a, axes_a, p, tol)) {
            m.points.push_back(p);
            m.depths.push_back(best_depth);
        }
    }
    for (const Vec3& p : ca) {
        if (inside(b, axes_b, p, tol)) {
            m.points.push_back(p);
            m.depths.push_back(best_depth);
        }
    }
    if (m.points.empty()) {
        m.points.push_back(a.position + m.normal * project(a, axes_a, m.normal));
        m.depths.push_back(best_depth);
    }
    if (m.points.size() > 4) m.points.resize(4), m.depths.resize(4);
    return true;
}

bool ray_sphere(Vec3 o, Vec3 dir, Vec3 c, float r, float& t) {
    Vec3 oc = o - c;
    float b = dot(oc, dir);
    float cc = dot(oc, oc) - r * r;
    float disc = b * b - cc;
    if (disc < 0) return false;
    float s = std::sqrt(disc);
    float t0 = -b - s;
    if (t0 >= 0) { t = t0; return true; }
    float t1 = -b + s;
    if (t1 >= 0) { t = t1; return true; }
    return false;
}

// Ray vs the capsule of radius r around a segment: the cylinder, then the two end spheres.
bool ray_segment_capsule(Vec3 o, Vec3 dir, Vec3 p0, Vec3 p1, float r, float& t, Vec3& normal) {
    bool found = false;
    float best = 0;
    Vec3 axis = p1 - p0;
    float len2 = dot(axis, axis);
    if (len2 > 1e-10f) {
        Vec3 u = axis * (1.0f / std::sqrt(len2));
        Vec3 w = o - p0;
        Vec3 dperp = dir - u * dot(dir, u);
        Vec3 wperp = w - u * dot(w, u);
        float A = dot(dperp, dperp), B = 2 * dot(dperp, wperp), C = dot(wperp, wperp) - r * r;
        if (A > 1e-10f) {
            float disc = B * B - 4 * A * C;
            if (disc >= 0) {
                float tt = (-B - std::sqrt(disc)) / (2 * A);
                if (tt >= 0) {
                    Vec3 hit = o + dir * tt;
                    float along = dot(hit - p0, u);
                    if (along >= 0 && along * along <= len2) {
                        found = true;
                        best = tt;
                        normal = normalize(hit - (p0 + u * along));
                    }
                }
            }
        }
    }
    for (const Vec3& c : {p0, p1}) {
        float tt;
        if (ray_sphere(o, dir, c, r, tt) && (!found || tt < best)) {
            found = true;
            best = tt;
            normal = normalize(o + dir * tt - c);
        }
    }
    if (found) t = best;
    return found;
}

bool ray_capsule(Vec3 o, Vec3 dir, const Body& b, float& t, Vec3& normal) {
    Vec3 p0, p1;
    capsule_segment(b, p0, p1);
    return ray_segment_capsule(o, dir, p0, p1, b.half.x, t, normal);
}

bool ray_box(Vec3 o, Vec3 dir, const Body& b, float& t, Vec3& normal) {
    Mat4 rot = Mat4::rotation(b.rotation);
    Mat4 inv = rot.inverse_affine();
    Vec3 lo = inv.transform_dir(o - b.position);
    Vec3 ld = inv.transform_dir(dir);
    float tmin = 0, tmax = 1e30f;
    int hit_axis = -1;
    float hit_sign = 1;
    for (int i = 0; i < 3; ++i) {
        float oi = (&lo.x)[i], di = (&ld.x)[i], h = (&b.half.x)[i];
        if (std::fabs(di) < 1e-8f) {
            if (oi < -h || oi > h) return false;
            continue;
        }
        float t1 = (-h - oi) / di, t2 = (h - oi) / di;
        float sign = -1;
        if (t1 > t2) { std::swap(t1, t2); sign = 1; }
        if (t1 > tmin) { tmin = t1; hit_axis = i; hit_sign = sign; }
        tmax = std::min(tmax, t2);
        if (tmin > tmax) return false;
    }
    t = tmin;
    Vec3 n{0, 0, 0};
    if (hit_axis >= 0) (&n.x)[hit_axis] = hit_sign;
    else n = -dir;
    normal = normalize(rot.transform_dir(n));
    return true;
}

}  // namespace

struct Physics::Impl {
    Settings settings;
    std::vector<Body> bodies;
    std::vector<Contact> contacts;
    std::vector<JointInfo> joint_infos;
    std::set<std::pair<EntityId, EntityId>> touching;  // pairs in contact last step
    std::set<std::pair<EntityId, EntityId>> ignored;   // exceptions: pairs that never collide (ordered ids)
    std::set<std::pair<EntityId, EntityId>> joined;    // pairs a joint with collide_connected = false keeps apart, this step
    std::map<EntityId, float> sleep_timers;             // persists across steps (bodies are regathered)
    std::map<EntityId, int> limit_states;               // hinge limit state per joint, for joint.limit events
    std::map<std::pair<EntityId, EntityId>, std::uint64_t> pair_cause;  // begin event seq per pair
    StepStats stats;
    assets::AssetStore* assets = nullptr;
    mutable std::map<EntityId, MeshShape> mesh_shapes;  // per mesh collider, rebuilt when its key changes
    mutable std::set<std::string> warned;               // mesh files reported missing or broken

    // The triangles of a shape 3 collider, built for the entity's transform (and rebuilt when it
    // or the file changes); null when there is no store, no path, or no usable triangles.
    const MeshShape* mesh_for(EntityId id, const world::Collider& col, const world::Transform& t, const world::MeshRenderer* mr) const {
        if (!assets) return nullptr;
        const std::string path = !col.mesh.empty() ? col.mesh : (mr ? mr->mesh : std::string());
        if (path.empty()) return nullptr;
        const Vec3 pos = t.position + t.rotation.rotate(col.offset);
        const std::string key = std::format("{}|{:.6g},{:.6g},{:.6g}|{:.6g},{:.6g},{:.6g},{:.6g}|{:.6g},{:.6g},{:.6g}", path, pos.x, pos.y, pos.z, t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w, t.scale.x, t.scale.y, t.scale.z);
        MeshShape& ms = mesh_shapes[id];
        if (ms.key != key) {
            ms.key = key;
            ms.clear();
            auto m = assets->mesh(path);
            if (!m) {
                if (warned.insert(path).second) log::warn("physics", "mesh collider {}: {}", path, m.error().to_string());
            } else {
                ms.build(**m, pos, t.rotation, t.scale);
            }
        }
        return ms.tri.empty() ? nullptr : &ms;
    }

    void gather(world::World& w) {
        bodies.clear();
        std::set<EntityId> seen;
        w.ecs().each([&](flecs::entity e, const world::RigidBody& rb, const world::Collider& col, const world::Transform& t) {
            Body b;
            b.id = e.id();
            b.kind = rb.kind;
            b.shape = col.shape;
            if (col.shape == 3) {
                b.mesh = mesh_for(b.id, col, t, e.try_get<world::MeshRenderer>());
                seen.insert(b.id);
            }
            b.trigger = col.is_trigger;
            b.layer = col.layer;
            b.mask = col.mask;
            b.group = col.group;
            b.ccd = rb.ccd;
            b.sleeping = rb.sleeping;
            b.inv_mass = (rb.kind == 0 && rb.mass > 0) ? 1.0f / rb.mass : 0.0f;
            b.restitution = rb.restitution;
            b.friction = rb.friction;
            b.linear_damping = rb.linear_damping;
            b.angular_damping = rb.angular_damping;
            b.gravity_scale = rb.gravity_scale;
            b.lock_rotation = rb.lock_rotation;
            b.position = t.position + t.rotation.rotate(col.offset);
            b.rotation = t.rotation;
            b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.shape == 2 ? Vec3{col.size.x, col.size.y, col.size.x} : col.size;
            if (const world::Velocity* v = e.try_get<world::Velocity>()) {
                b.velocity = v->linear;
                b.angular = v->angular;
            }
            if (auto it = sleep_timers.find(b.id); it != sleep_timers.end()) b.sleep_timer = it->second;
            bodies.push_back(b);
        });
        std::sort(bodies.begin(), bodies.end(), [](const Body& x, const Body& y) { return x.id < y.id; });
        for (auto it = mesh_shapes.begin(); it != mesh_shapes.end();) {
            if (seen.contains(it->first)) ++it;
            else it = mesh_shapes.erase(it);  // the entity is gone or no longer a mesh collider
        }
    }

    void write_back(world::World& w, float dt) {
        for (Body& b : bodies) {
            flecs::entity e = w.entity(b.id);
            if (b.kind == 1) continue;
            world::Transform t = e.get<world::Transform>();
            const world::Collider& col = e.get<world::Collider>();
            t.position = b.position - b.rotation.rotate(col.offset);
            t.rotation = b.rotation;
            e.set<world::Transform>(t);
            world::Velocity v;
            v.linear = b.velocity;
            v.angular = b.angular;
            e.set<world::Velocity>(v);
            world::RigidBody rb = e.get<world::RigidBody>();
            if (rb.sleeping != b.sleeping) {
                rb.sleeping = b.sleeping;
                e.set<world::RigidBody>(rb);
            }
        }
        (void)dt;
    }
};

Physics::Physics(Settings settings) : impl_(std::make_unique<Impl>()) { impl_->settings = settings; }
Physics::~Physics() = default;
namespace {

float bounding_radius(const Body& b) {
    if (b.shape == 1) return b.half.x;
    if (b.shape == 2) return b.half.x + b.half.y;
    return length(b.half);
}

// The shortest way across a shape: the extent a sweep must not skip in one sample.
float thinnest_extent(const Body& b) {
    if (b.shape == 1 || b.shape == 2) return 2.0f * b.half.x;
    return 2.0f * std::min({b.half.x, b.half.y, b.half.z});
}

// A sphere of radius r cast along a ray against a box: the box grown by r has flat faces where the
// hit lands within the box's own extent, and rounded edges and corners elsewhere (the twelve edge
// capsules, whose caps are the corner spheres).
bool sphere_cast_box(Vec3 o, Vec3 dir, float r, const Body& box, float& t, Vec3& normal) {
    Body big = box;
    big.half = box.half + Vec3{r, r, r};
    float t0 = 0;
    Vec3 n0;
    if (!ray_box(o, dir, big, t0, n0)) return false;
    const Mat4 rot = Mat4::rotation(box.rotation);
    const Vec3 local = rot.inverse_affine().transform_dir(o + dir * t0 - box.position);
    int outside = 0;
    for (int i = 0; i < 3; ++i) {
        if (std::fabs((&local.x)[i]) > (&box.half.x)[i] + 1e-6f) ++outside;
    }
    if (outside <= 1) {
        t = t0;
        normal = n0;
        return true;
    }
    Vec3 corners[8];
    for (int i = 0; i < 8; ++i) corners[i] = box.position + rot.transform_dir({(i & 1) ? box.half.x : -box.half.x, (i & 2) ? box.half.y : -box.half.y, (i & 4) ? box.half.z : -box.half.z});
    static const int edges[12][2] = {{0, 1}, {2, 3}, {4, 5}, {6, 7}, {0, 2}, {1, 3}, {4, 6}, {5, 7}, {0, 4}, {1, 5}, {2, 6}, {3, 7}};
    bool found = false;
    float best = 0;
    Vec3 bn;
    for (const auto& e : edges) {
        float tt = 0;
        Vec3 n;
        if (ray_segment_capsule(o, dir, corners[e[0]], corners[e[1]], r, tt, n) && (!found || tt < best)) {
            found = true;
            best = tt;
            bn = n;
        }
    }
    if (found) {
        t = best;
        normal = bn;
    }
    return found;
}

// A sphere cast against one triangle: its face offset by r toward the sphere, then its edges as
// capsules (their caps are the corner spheres). `n` is the triangle's normal.
bool sphere_cast_triangle(Vec3 o, Vec3 dir, float r, Vec3 a, Vec3 b, Vec3 c, Vec3 n, float& t, Vec3& normal) {
    bool found = false;
    float best = 0;
    Vec3 bn;
    const Vec3 nn = dot(o - a, n) >= 0 ? n : -n;
    const float denom = dot(dir, nn);
    if (denom < -1e-8f) {
        const float tt = dot(a + nn * r - o, nn) / denom;
        if (tt >= 0 && inside_prism(o + dir * tt - nn * r, a, b, c, n)) {
            found = true;
            best = tt;
            bn = nn;
        }
    }
    const Vec3 ends[3][2] = {{a, b}, {b, c}, {c, a}};
    for (const auto& e : ends) {
        float tt = 0;
        Vec3 en;
        if (ray_segment_capsule(o, dir, e[0], e[1], r, tt, en) && (!found || tt < best)) {
            found = true;
            best = tt;
            bn = en;
        }
    }
    if (found) {
        t = best;
        normal = bn;
    }
    return found;
}

// A sphere of radius r cast against one body's exact shape: the nearest hit under `reach`, the
// normal facing the sphere.
bool sphere_cast_body(Vec3 o, Vec3 dir, float r, const Body& b, float reach, float& t, Vec3& normal) {
    if (b.shape == 3) {
        const MeshShape* ms = b.mesh;
        if (!ms) return false;
        bool hit = false;
        const Vec3 grow{r, r, r};
        std::vector<std::uint32_t> stack{0};
        while (!stack.empty()) {
            const MeshShape::Node& node = ms->nodes[stack.back()];
            stack.pop_back();
            if (!ray_aabb(o, dir, node.min - grow, node.max + grow, reach)) continue;
            if (node.count == 0) {
                stack.push_back(node.left);
                stack.push_back(node.right);
                continue;
            }
            for (std::uint32_t k = node.first; k < node.first + node.count; ++k) {
                const std::uint32_t tri = ms->order[k];
                float th = 0;
                Vec3 n;
                if (!sphere_cast_triangle(o, dir, r, ms->v[ms->tri[tri][0]], ms->v[ms->tri[tri][1]], ms->v[ms->tri[tri][2]], ms->n[tri], th, n)) continue;
                if (th >= reach) continue;
                reach = th;
                hit = true;
                t = th;
                normal = n;
            }
        }
        return hit;
    }
    if (b.shape == 2) {
        Body big = b;
        big.half.x += r;
        return ray_capsule(o, dir, big, t, normal) && t < reach;
    }
    if (b.shape == 1) {
        if (!ray_sphere(o, dir, b.position, b.half.x + r, t) || t >= reach) return false;
        normal = normalize(o + dir * t - b.position);
        return true;
    }
    return sphere_cast_box(o, dir, r, b, t, normal) && t < reach;
}

// Whether two bodies' shapes overlap, and the normal from `b` into `a` when they do.
bool overlap_pair(const Body& a, const Body& b, Vec3& normal, Vec3* point = nullptr) {
    Manifold m;
    bool hit = false;
    if (a.shape == 3 && b.shape == 3) hit = false;
    else if (b.shape == 3) hit = collide_mesh(a, b, m, false);
    else if (a.shape == 3) hit = collide_mesh(b, a, m, true);
    else if (a.shape == 1 && b.shape == 1) hit = collide_ss(a, b, m);
    else if (a.shape == 1 && b.shape == 0) hit = collide_sb(a, b, m, false);
    else if (a.shape == 0 && b.shape == 1) hit = collide_sb(b, a, m, true);
    else if (a.shape == 2 && b.shape == 1) hit = collide_cs(a, b, m, false);
    else if (a.shape == 1 && b.shape == 2) hit = collide_cs(b, a, m, true);
    else if (a.shape == 2 && b.shape == 2) hit = collide_cc(a, b, m);
    else if (a.shape == 2 && b.shape == 0) hit = collide_cb(a, b, m, false);
    else if (a.shape == 0 && b.shape == 2) hit = collide_cb(b, a, m, true);
    else hit = collide_bb(a, b, m);
    if (hit) {
        normal = -m.normal;   // the manifold's normal runs from a to b
        if (point) *point = m.points.empty() ? a.position : m.points.front();
    }
    return hit;
}

// The sweep of `a` along `motion`, turning by `spin` (an axis times an angle) meanwhile, against
// `b`, itself turning in place by `b_spin`, by samples: no sample skips more than half of the
// thinner body's thinnest extent along the farthest point's path of either, the first overlapping
// sample is bisected back to the last free one, and the fraction of the step reached is the time
// of impact, with the contact point there. `limit` bounds the search.
bool stepped_sweep(const Body& a, Vec3 motion, Vec3 spin, const Body& b, Vec3 b_spin, float limit, float& fraction, Vec3& normal, Vec3& point) {
    const float dist = length(motion);
    const float angle = length(spin), b_angle = length(b_spin);
    const float arc = angle * bounding_radius(a) + b_angle * bounding_radius(b);
    if (dist + arc <= 1e-6f) return false;
    const float thin = b_angle > 1e-6f ? std::min(thinnest_extent(a), thinnest_extent(b)) : thinnest_extent(a);
    const int samples = std::clamp(static_cast<int>(std::ceil((dist + arc) / std::max(thin * 0.5f, 1e-3f))), 1, 256);
    Body moved = a, other = b;
    auto place = [&](float f) {
        moved.position = a.position + motion * f;
        if (angle > 1e-6f) moved.rotation = normalize(Quat::from_axis_angle(spin, angle * f) * a.rotation);
        update_aabb(moved);
        if (b_angle > 1e-6f) {
            other.rotation = normalize(Quat::from_axis_angle(b_spin, b_angle * f) * b.rotation);
            update_aabb(other);
        }
    };
    Vec3 n, pt;
    if (overlap_pair(a, b, n, &pt)) {   // already touching: the impact is now
        fraction = 0;
        normal = n;
        point = pt;
        return true;
    }
    float lo = 0, hi = 0;
    bool found = false;
    for (int i = 1; i <= samples; ++i) {
        const float f = static_cast<float>(i) / static_cast<float>(samples);
        if (f > limit) break;
        place(f);
        if (overlap_pair(moved, other, n, &pt)) {
            hi = f;
            found = true;
            break;
        }
        lo = f;
    }
    if (!found) return false;
    for (int k = 0; k < 6; ++k) {
        const float mid = 0.5f * (lo + hi);
        place(mid);
        Vec3 nm, pm;
        if (overlap_pair(moved, other, nm, &pm)) { hi = mid; n = nm; pt = pm; }
        else lo = mid;
    }
    fraction = lo;
    normal = n;
    point = pt;
    return true;
}

std::pair<EntityId, EntityId> ordered(EntityId a, EntityId b) { return a < b ? std::pair{a, b} : std::pair{b, a}; }

}  // namespace

void Physics::ignore(EntityId a, EntityId b, bool ignore) {
    if (a == 0 || b == 0 || a == b) return;
    if (ignore) impl_->ignored.insert(ordered(a, b));
    else impl_->ignored.erase(ordered(a, b));
}

std::vector<std::pair<EntityId, EntityId>> Physics::ignored() const {
    return {impl_->ignored.begin(), impl_->ignored.end()};
}

Settings& Physics::settings() { return impl_->settings; }
void Physics::set_assets(assets::AssetStore* assets) {
    impl_->assets = assets;
    impl_->mesh_shapes.clear();
    impl_->warned.clear();
}
const std::vector<Contact>& Physics::contacts() const { return impl_->contacts; }
const std::vector<JointInfo>& Physics::joints() const { return impl_->joint_infos; }
const StepStats& Physics::stats() const { return impl_->stats; }

void Physics::step(world::World& w, double dt_d) {
    Impl& im = *impl_;
    const Settings& s = im.settings;
    auto dt = static_cast<float>(dt_d);
    im.gather(w);
    im.contacts.clear();
    im.stats = StepStats{};
    im.stats.bodies = static_cast<std::uint32_t>(im.bodies.size());
    for (const Body& b : im.bodies) {
        if (!b.mesh) continue;
        im.stats.meshes++;
        im.stats.triangles += static_cast<std::uint32_t>(b.mesh->tri.size());
    }
    if (im.bodies.empty()) {
        im.touching.clear();
        return;
    }
    // 1. Forces.
    for (Body& b : im.bodies) {
        if (b.kind != 0 || b.sleeping) continue;
        b.velocity += s.gravity * (b.gravity_scale * dt);
        b.velocity *= std::max(0.0f, 1.0f - b.linear_damping * dt);
        b.angular *= std::max(0.0f, 1.0f - b.angular_damping * dt);
    }
    for (Body& b : im.bodies) {
        update_aabb(b);
        b.inv_inertia_world = inertia_inverse(b);
    }
    // Pairs kept apart on purpose: exceptions whose bodies are gone are dropped; joints that say
    // their two bodies do not collide are noted for this step.
    for (auto it = im.ignored.begin(); it != im.ignored.end();) it = (w.alive(it->first) && w.alive(it->second)) ? std::next(it) : im.ignored.erase(it);
    im.joined.clear();
    w.ecs().each([&](flecs::entity e, const world::Joint& j) {
        if (j.collide_connected || j.target.empty()) return;
        const EntityId t = w.find(j.target);
        if (t != 0 && t != e.id()) im.joined.insert(ordered(e.id(), t));
    });
    // Whether two bodies may touch: the same negative group never, the same positive group always,
    // otherwise the layers; then the exceptions and the joints.
    auto allowed = [&](const Body& a, const Body& b) {
        if (a.group != 0 && a.group == b.group) {
            if (a.group < 0) return false;
        } else if (!(a.layer & b.mask) || !(b.layer & a.mask)) {
            return false;
        }
        if (!im.ignored.empty() && im.ignored.contains(ordered(a.id, b.id))) return false;
        if (!im.joined.empty() && im.joined.contains(ordered(a.id, b.id))) return false;
        return true;
    };
    // 2. Broadphase: sort and sweep on x.
    std::vector<std::size_t> order(im.bodies.size());
    for (std::size_t i = 0; i < order.size(); ++i) order[i] = i;
    std::sort(order.begin(), order.end(), [&](std::size_t x, std::size_t y) { return im.bodies[x].aabb_min.x < im.bodies[y].aabb_min.x || (im.bodies[x].aabb_min.x == im.bodies[y].aabb_min.x && im.bodies[x].id < im.bodies[y].id); });
    std::vector<Manifold> manifolds;
    for (std::size_t i = 0; i < order.size(); ++i) {
        const Body& a = im.bodies[order[i]];
        for (std::size_t j = i + 1; j < order.size(); ++j) {
            const Body& b = im.bodies[order[j]];
            if (b.aabb_min.x > a.aabb_max.x) break;
            if (a.kind != 0 && b.kind != 0) continue;  // nothing dynamic
            auto moving = [](const Body& x) { return (x.kind == 0 && !x.sleeping) || (x.kind == 2 && (x.velocity.x != 0 || x.velocity.y != 0 || x.velocity.z != 0)); };
            if (!moving(a) && !moving(b)) continue;
            if (!aabb_overlap(a, b)) continue;
            if (!allowed(a, b)) { im.stats.ignored++; continue; }  // layers, groups, exceptions or a joint keep them apart
            im.stats.pairs++;
            std::size_t ia = order[i], ib = order[j];
            if (im.bodies[ia].id > im.bodies[ib].id) std::swap(ia, ib);
            const Body& ba = im.bodies[ia];
            const Body& bb = im.bodies[ib];
            Manifold m;
            m.a = ia;
            m.b = ib;
            m.trigger = ba.trigger || bb.trigger;
            bool hit = false;
            if (ba.shape == 3 && bb.shape == 3) hit = false;  // meshes do not collide with each other
            else if (bb.shape == 3) hit = collide_mesh(ba, bb, m, false);
            else if (ba.shape == 3) hit = collide_mesh(bb, ba, m, true);
            else if (ba.shape == 1 && bb.shape == 1) hit = collide_ss(ba, bb, m);
            else if (ba.shape == 1 && bb.shape == 0) hit = collide_sb(ba, bb, m, false);
            else if (ba.shape == 0 && bb.shape == 1) hit = collide_sb(bb, ba, m, true);
            else if (ba.shape == 2 && bb.shape == 1) hit = collide_cs(ba, bb, m, false);
            else if (ba.shape == 1 && bb.shape == 2) hit = collide_cs(bb, ba, m, true);
            else if (ba.shape == 2 && bb.shape == 2) hit = collide_cc(ba, bb, m);
            else if (ba.shape == 2 && bb.shape == 0) hit = collide_cb(ba, bb, m, false);
            else if (ba.shape == 0 && bb.shape == 2) hit = collide_cb(bb, ba, m, true);
            else hit = collide_bb(ba, bb, m);
            if (hit) manifolds.push_back(std::move(m));
        }
    }
    // Wake sleeping bodies that a moving body touches, before solving, so they respond this step.
    for (const Manifold& m : manifolds) {
        if (m.trigger) continue;
        Body& a = im.bodies[m.a];
        Body& b = im.bodies[m.b];
        auto pushes = [&](const Body& x) { return (x.kind == 0 && !x.sleeping && length(x.velocity) > s.sleep_linear * 4) || (x.kind == 2 && length(x.velocity) > 0); };
        if (a.sleeping && a.kind == 0 && pushes(b)) { a.sleeping = false; a.sleep_timer = 0; }
        if (b.sleeping && b.kind == 0 && pushes(a)) { b.sleeping = false; b.sleep_timer = 0; }
    }
    // Joints: gathered from Joint components on bodies; the other side is a body, any entity as a
    // fixed point, or a world point.
    struct JointState {
        EntityId entity = 0, target = 0;
        int kind = 0;
        std::size_t a = 0;                 // body index
        bool b_is_body = false;
        std::size_t b = 0;
        Vec3 anchor_a, anchor_b;           // local anchors
        Vec3 ra, rb;                       // anchor offsets from the body centers
        Vec3 pa, pb;                       // anchor world positions
        float length = 0;
        bool rope = false;
        float break_force = 0;
        Vec3 impulse;                      // accumulated this step
        float k = 0;                       // effective mass (distance joints)
        Vec3 n;                            // constraint direction (distance joints)
        float bias = 0;
        Mat4 k_inv;                        // effective mass inverse (ball and hinge joints), 3x3 in a Mat4
        bool broken = false;
        // Hinges: the axis in each frame, a direction across it to measure the angle from, the
        // world tangents the angular constraints act along, limits and motor.
        Quat rot_b;                        // the target frame's rotation (identity for a world point)
        Vec3 axis_a, axis_b, perp_a, ref_b;
        Vec3 axis_w, t1, t2;
        float k_t1 = 0, k_t2 = 0, k_axis = 0;
        float angle = 0, speed = 0;
        bool limit = false;
        float lower = 0, upper = 0;
        float motor_speed = 0, motor_torque = 0;
        float motor_impulse = 0, limit_impulse = 0;
        int limit_state = 0;
        // Springs (distance joints with stiffness): Catto's soft constraint, a compliance (gamma)
        // and a bias toward the rest length, computed per step.
        bool spring = false;
        float stiffness = 0, damping = 0, gamma = 0, soft_bias = 0;
        // Sliders: the directions across the axis carry equality constraints on the anchors, the
        // axis carries the limit and the motor; the travel and its speed are reported.
        bool slide_limit = false;
        float slide_lower = 0, slide_upper = 0, motor_force = 0, translation = 0;
        float k_l1 = 0, k_l2 = 0, k_l_axis = 0;
        float slide_motor_impulse = 0, slide_limit_impulse = 0;
        int slide_state = 0;
    };
    std::vector<JointState> joints;
    std::vector<std::pair<EntityId, float>> lengths_to_write;
    std::vector<std::tuple<EntityId, Vec3, Vec3>> frames_to_write;
    // The hinge angle: how far the body's frame has turned about the axis relative to the target's.
    auto hinge_angle = [](const JointState& js, const Quat& rot_a, const Quat& rot_b) {
        const Vec3 axis_w = rot_a.rotate(js.axis_a);
        const Vec3 ua = rot_a.rotate(js.perp_a), ub = rot_b.rotate(js.ref_b);
        return std::atan2(dot(cross(ub, ua), axis_w), dot(ub, ua));
    };
    auto body_index = [&](EntityId id) -> std::size_t {
        auto it = std::lower_bound(im.bodies.begin(), im.bodies.end(), id, [](const Body& b, EntityId v) { return b.id < v; });
        return (it != im.bodies.end() && it->id == id) ? static_cast<std::size_t>(it - im.bodies.begin()) : static_cast<std::size_t>(-1);
    };
    w.ecs().each([&](flecs::entity e, const world::Joint& j) {
        std::size_t ia = body_index(e.id());
        if (ia == static_cast<std::size_t>(-1)) return;
        JointState js;
        js.entity = e.id();
        js.kind = j.kind;
        js.a = ia;
        js.rope = j.rope;
        js.break_force = j.break_force;
        js.anchor_a = j.anchor;
        js.anchor_b = j.target_anchor;
        const Body& a = im.bodies[ia];
        js.pa = a.position + a.rotation.rotate(j.anchor);
        js.ra = js.pa - a.position;
        if (!j.target.empty()) {
            EntityId t = w.find(j.target);
            if (!t) return;  // target gone: the joint waits
            js.target = t;
            std::size_t ib = body_index(t);
            if (ib != static_cast<std::size_t>(-1)) {
                js.b_is_body = true;
                js.b = ib;
                const Body& b = im.bodies[ib];
                js.pb = b.position + b.rotation.rotate(j.target_anchor);
                js.rb = js.pb - b.position;
                js.rot_b = b.rotation;
            } else {
                Vec3 tp{0, 0, 0};
                Quat tr;
                if (const auto* wt = w.try_get<world::WorldTransform>(t)) { tp = wt->position; tr = wt->rotation; }
                else if (const auto* tt = w.try_get<world::Transform>(t)) { tp = tt->position; tr = tt->rotation; }
                js.pb = tp + tr.rotate(j.target_anchor);
                js.rot_b = tr;
            }
        } else {
            js.pb = j.target_anchor;
        }
        float current = length(js.pb - js.pa);
        js.length = j.distance < 0 ? current : j.distance;
        if (j.distance < 0) lengths_to_write.push_back({e.id(), current});
        js.spring = j.kind == 0 && j.stiffness > 0;
        js.stiffness = j.stiffness;
        js.damping = j.damping;
        if (j.kind == 2 || j.kind == 3) {
            js.axis_a = length(j.axis) > 1e-6f ? normalize(j.axis) : Vec3{0, 0, 1};
            js.perp_a = any_perpendicular(js.axis_a);
            const Quat inv_b = conj(js.rot_b);
            bool take_frame = false;
            if (length(j.target_axis) > 1e-6f) js.axis_b = normalize(j.target_axis);
            else { js.axis_b = normalize(inv_b.rotate(a.rotation.rotate(js.axis_a))); take_frame = true; }
            if (length(j.reference) > 1e-6f) js.ref_b = normalize(j.reference);
            else { js.ref_b = normalize(inv_b.rotate(a.rotation.rotate(js.perp_a))); take_frame = true; }
            if (take_frame) frames_to_write.emplace_back(e.id(), js.axis_b, js.ref_b);
            js.limit = j.limit;
            js.lower = j.lower;
            js.upper = j.upper;
            js.motor_speed = j.motor_speed;
            js.motor_torque = j.motor_torque;
            if (j.kind == 3) {
                // A slider's limit and motor act along the axis; the turn about it is locked.
                js.slide_limit = j.limit;
                js.slide_lower = j.lower;
                js.slide_upper = j.upper;
                js.motor_force = j.motor_force;
                js.motor_torque = 0;
                js.limit = true;
                js.lower = js.upper = 0;
            }
        }
        joints.push_back(js);
    });
    std::sort(joints.begin(), joints.end(), [](const JointState& x, const JointState& y) { return x.entity < y.entity; });
    // A joint wakes what it holds: when the other side moves, or when the joint is violated
    // (an anchor moved, a length changed).
    for (JointState& js : joints) {
        Body& a = im.bodies[js.a];
        float error = js.kind == 0 ? length(js.pb - js.pa) - js.length : length(js.pb - js.pa);
        if (js.kind == 3) {
            // A slider's anchors sit apart along the axis by design; only the sideways offset counts.
            const Vec3 axis_w = a.rotation.rotate(js.axis_a);
            const Vec3 d = js.pb - js.pa;
            error = length(d - axis_w * dot(d, axis_w));
        }
        bool violated = js.rope ? error > s.slop : std::fabs(error) > s.slop;
        if (js.spring) violated = false;  // a spring at rest is stretched by design: it wakes with its neighbour or a script write
        if (js.kind == 2 || js.kind == 3) {
            // A running motor keeps its bodies awake; so does an axis that has drifted.
            const Quat rb_rot = js.b_is_body ? im.bodies[js.b].rotation : js.rot_b;
            if (js.motor_torque > 0 && js.motor_speed != 0) violated = true;
            if (js.motor_force > 0 && js.motor_speed != 0) violated = true;
            if (dot(a.rotation.rotate(js.axis_a), rb_rot.rotate(js.axis_b)) < 0.9999f) violated = true;
        }
        bool other_moving = js.b_is_body && ((im.bodies[js.b].kind == 0 && !im.bodies[js.b].sleeping) || im.bodies[js.b].kind == 2);
        if (a.sleeping && a.kind == 0 && (other_moving || violated)) { a.sleeping = false; a.sleep_timer = 0; }
        if (js.b_is_body) {
            Body& b = im.bodies[js.b];
            if (b.sleeping && b.kind == 0 && ((a.kind == 0 && !a.sleeping) || a.kind == 2 || violated)) { b.sleeping = false; b.sleep_timer = 0; }
        }
    }
    for (JointState& js : joints) {
        Body& a = im.bodies[js.a];
        float inv_a = a.kind == 0 && !a.sleeping ? a.inv_mass : 0.0f;
        float inv_b = js.b_is_body && im.bodies[js.b].kind == 0 && !im.bodies[js.b].sleeping ? im.bodies[js.b].inv_mass : 0.0f;
        const Mat4 zero = [] { Mat4 z; for (float& v : z.m) v = 0; return z; }();
        const Mat4& ia_w = inv_a > 0 ? a.inv_inertia_world : zero;
        const Mat4& ib_w = inv_b > 0 ? im.bodies[js.b].inv_inertia_world : zero;
        Vec3 d = js.pb - js.pa;
        float dist = length(d);
        if (js.kind != 0) {
            // Ball and hinge joints: K = (1/ma + 1/mb) I - [ra]x Ia^-1 [ra]x - [rb]x Ib^-1 [rb]x, inverted.
            Mat4 K = zero;
            for (int i = 0; i < 3; ++i) K.at(i, i) = inv_a + inv_b;
            auto skew_term = [&](Vec3 r, const Mat4& iw, Mat4& out) {
                // -[r]x * iw * [r]x
                float rx[3][3] = {{0, -r.z, r.y}, {r.z, 0, -r.x}, {-r.y, r.x, 0}};
                float t1[3][3] = {};
                for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) for (int k = 0; k < 3; ++k) t1[i][j] += iw.at(k, i) * rx[k][j];
                for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) { float s = 0; for (int k = 0; k < 3; ++k) s += rx[k][i] * t1[k][j]; out.at(j, i) += s; }
            };
            if (inv_a > 0) skew_term(js.ra, ia_w, K);
            if (inv_b > 0) skew_term(js.rb, ib_w, K);
            // 3x3 inverse.
            float m00 = K.at(0, 0), m01 = K.at(1, 0), m02 = K.at(2, 0), m10 = K.at(0, 1), m11 = K.at(1, 1), m12 = K.at(2, 1), m20 = K.at(0, 2), m21 = K.at(1, 2), m22 = K.at(2, 2);
            float det = m00 * (m11 * m22 - m12 * m21) - m01 * (m10 * m22 - m12 * m20) + m02 * (m10 * m21 - m11 * m20);
            js.k_inv = zero;
            if (std::fabs(det) > 1e-12f) {
                float inv = 1.0f / det;
                js.k_inv.at(0, 0) = (m11 * m22 - m12 * m21) * inv; js.k_inv.at(1, 0) = (m02 * m21 - m01 * m22) * inv; js.k_inv.at(2, 0) = (m01 * m12 - m02 * m11) * inv;
                js.k_inv.at(0, 1) = (m12 * m20 - m10 * m22) * inv; js.k_inv.at(1, 1) = (m00 * m22 - m02 * m20) * inv; js.k_inv.at(2, 1) = (m02 * m10 - m00 * m12) * inv;
                js.k_inv.at(0, 2) = (m10 * m21 - m11 * m20) * inv; js.k_inv.at(1, 2) = (m01 * m20 - m00 * m21) * inv; js.k_inv.at(2, 2) = (m00 * m11 - m01 * m10) * inv;
            }
            js.n = {};  // no velocity bias: the position pass below removes drift
            if (js.kind == 2 || js.kind == 3) {
                js.axis_w = a.rotation.rotate(js.axis_a);
                js.t1 = any_perpendicular(js.axis_w);
                js.t2 = cross(js.axis_w, js.t1);
                auto ang_k = [&](Vec3 v) {
                    float k = 0;
                    if (inv_a > 0) k += dot(v, mul3(ia_w, v));
                    if (inv_b > 0) k += dot(v, mul3(ib_w, v));
                    return k;
                };
                js.k_t1 = ang_k(js.t1);
                js.k_t2 = ang_k(js.t2);
                js.k_axis = ang_k(js.axis_w);
                const Quat rb_rot = js.b_is_body ? im.bodies[js.b].rotation : js.rot_b;
                js.angle = hinge_angle(js, a.rotation, rb_rot);
                js.speed = dot(js.axis_w, a.angular - (js.b_is_body ? im.bodies[js.b].angular : Vec3{}));
                if (js.limit) js.limit_state = js.lower >= js.upper ? 2 : js.angle <= js.lower ? -1 : js.angle >= js.upper ? 1 : 0;
                if (js.kind == 3) {
                    // The turn about the axis is locked; the slide gets effective masses of its own.
                    js.limit_state = 2;
                    auto lin_k = [&](Vec3 v) {
                        float k = inv_a + inv_b;
                        if (inv_a > 0) k += dot(v, cross(mul3(ia_w, cross(js.ra, v)), js.ra));
                        if (inv_b > 0) k += dot(v, cross(mul3(ib_w, cross(js.rb, v)), js.rb));
                        return k;
                    };
                    js.k_l1 = lin_k(js.t1);
                    js.k_l2 = lin_k(js.t2);
                    js.k_l_axis = lin_k(js.axis_w);
                    js.translation = dot(js.pa - js.pb, js.axis_w);
                    const Vec3 va = a.velocity + cross(a.angular, js.ra);
                    const Vec3 vb = js.b_is_body ? im.bodies[js.b].velocity + cross(im.bodies[js.b].angular, js.rb) : Vec3{};
                    js.speed = dot(va - vb, js.axis_w);
                    // Within a slop of a stop counts as at it, so a body pressed against a stop does
                    // not flicker between "at the limit" and "free" as the correction lands on it.
                    if (js.slide_limit) js.slide_state = js.slide_lower >= js.slide_upper ? 2 : js.translation <= js.slide_lower + s.slop ? -1 : js.translation >= js.slide_upper - s.slop ? 1 : 0;
                }
            }
        } else {
            js.n = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
            Vec3 ra_n = cross(js.ra, js.n), rb_n = cross(js.rb, js.n);
            js.k = inv_a + inv_b + (inv_a > 0 ? dot(js.n, cross(mul3(ia_w, ra_n), js.ra)) : 0.0f) + (inv_b > 0 ? dot(js.n, cross(mul3(ib_w, rb_n), js.rb)) : 0.0f);
            float c = dist - js.length;
            // A slack rope only acts when the anchors would pass its length within this step;
            // drift of taut joints is removed by the position pass below, not by a velocity bias.
            js.bias = js.rope && c < 0 ? c / dt : 0.0f;
            if (js.spring) {
                // Catto's soft constraint: stiffness and damping become a compliance and a bias
                // toward the rest length, stable at any stiffness the step can carry.
                js.gamma = 1.0f / (dt * (js.damping + dt * js.stiffness));
                js.soft_bias = dt * js.stiffness * js.gamma * c;
                js.bias = 0;
            }
        }
    }
    im.stats.joints = static_cast<std::uint32_t>(joints.size());
    // 3. Solve (non-trigger manifolds) with accumulated, clamped impulses (sequential impulses).
    struct PointState { float jn = 0; float jt1 = 0; float jt2 = 0; float vn0 = 0; float bias = 0; float k_n = 0; Vec3 ra, rb, t1, t2; float k_t1 = 0, k_t2 = 0; };
    std::vector<std::vector<PointState>> states(manifolds.size());
    for (std::size_t mi = 0; mi < manifolds.size(); ++mi) {
        Manifold& m = manifolds[mi];
        if (m.trigger) continue;
        Body& a = im.bodies[m.a];
        Body& b = im.bodies[m.b];
        float inv_mass_sum = a.inv_mass + b.inv_mass;
        if (inv_mass_sum == 0) continue;
        float e = std::max(a.restitution, b.restitution);
        // Split the manifold depth over its points so stacked corners do not over-correct.
        for (std::size_t k = 0; k < m.points.size(); ++k) {
            PointState ps;
            Vec3 p = m.points[k];
            ps.ra = p - a.position;
            ps.rb = p - b.position;
            Vec3 va = a.velocity + cross(a.angular, ps.ra);
            Vec3 vb = b.velocity + cross(b.angular, ps.rb);
            Vec3 rel = vb - va;
            ps.vn0 = dot(rel, m.normal);
            Vec3 ra_n = cross(ps.ra, m.normal), rb_n = cross(ps.rb, m.normal);
            ps.k_n = inv_mass_sum + dot(m.normal, cross(mul3(a.inv_inertia_world, ra_n), ps.ra)) + dot(m.normal, cross(mul3(b.inv_inertia_world, rb_n), ps.rb));
            // Restitution only; penetration is removed by position projection after the solve so
            // that correction never adds momentum.
            ps.bias = ps.vn0 < -1.0f ? -e * ps.vn0 : 0.0f;
            // Tangent basis.
            Vec3 t1 = std::fabs(m.normal.x) < 0.9f ? cross(m.normal, Vec3{1, 0, 0}) : cross(m.normal, Vec3{0, 1, 0});
            ps.t1 = normalize(t1);
            ps.t2 = normalize(cross(m.normal, ps.t1));
            auto k_t = [&](Vec3 t) {
                Vec3 ra_t = cross(ps.ra, t), rb_t = cross(ps.rb, t);
                return inv_mass_sum + dot(t, cross(mul3(a.inv_inertia_world, ra_t), ps.ra)) + dot(t, cross(mul3(b.inv_inertia_world, rb_t), ps.rb));
            };
            ps.k_t1 = k_t(ps.t1);
            ps.k_t2 = k_t(ps.t2);
            states[mi].push_back(ps);
        }
    }
    for (int iter = 0; iter < s.solver_iterations; ++iter) {
        for (JointState& js : joints) {
            Body& a = im.bodies[js.a];
            Body* bp = js.b_is_body ? &im.bodies[js.b] : nullptr;
            bool a_dyn = a.kind == 0 && !a.sleeping, b_dyn = bp && bp->kind == 0 && !bp->sleeping;
            if (!a_dyn && !b_dyn) continue;
            if (js.kind == 2 || js.kind == 3) {
                // Hinge, angular part: the motor drives the speed about the axis within its torque,
                // a limit only lets the angle come back inside, and the tangents across the axis
                // carry no relative spin at all. Angular impulse L goes to a (+) and b (-).
                auto rel_ang = [&]() { return a.angular - (bp ? bp->angular : Vec3{}); };
                auto apply_ang = [&](Vec3 L) {
                    if (a_dyn) a.angular += mul3(a.inv_inertia_world, L);
                    if (b_dyn) bp->angular -= mul3(bp->inv_inertia_world, L);
                };
                if (js.motor_torque > 0 && js.k_axis > 0) {
                    float dj = (js.motor_speed - dot(js.axis_w, rel_ang())) / js.k_axis;
                    const float max_impulse = js.motor_torque * static_cast<float>(dt);
                    float next = std::clamp(js.motor_impulse + dj, -max_impulse, max_impulse);
                    dj = next - js.motor_impulse;
                    js.motor_impulse = next;
                    apply_ang(js.axis_w * dj);
                }
                if (js.limit_state != 0 && js.k_axis > 0) {
                    float dj = -dot(js.axis_w, rel_ang()) / js.k_axis;
                    float next = js.limit_impulse + dj;
                    if (js.limit_state == -1) next = std::max(next, 0.0f);
                    if (js.limit_state == 1) next = std::min(next, 0.0f);
                    dj = next - js.limit_impulse;
                    js.limit_impulse = next;
                    apply_ang(js.axis_w * dj);
                }
                if (js.k_t1 > 0) apply_ang(js.t1 * (-dot(js.t1, rel_ang()) / js.k_t1));
                if (js.k_t2 > 0) apply_ang(js.t2 * (-dot(js.t2, rel_ang()) / js.k_t2));
            }
            Vec3 va = a_dyn || a.kind == 2 ? a.velocity + cross(a.angular, js.ra) : Vec3{0, 0, 0};
            Vec3 vb = bp ? (bp->kind != 1 ? bp->velocity + cross(bp->angular, js.rb) : Vec3{0, 0, 0}) : Vec3{0, 0, 0};
            Vec3 rel = vb - va;
            auto apply = [&](Vec3 impulse) {
                // Positive impulse pulls a toward b (applied +impulse to a, -impulse to b).
                if (a_dyn) { a.velocity += impulse * a.inv_mass; a.angular += mul3(a.inv_inertia_world, cross(js.ra, impulse)); }
                if (b_dyn) { bp->velocity -= impulse * bp->inv_mass; bp->angular -= mul3(bp->inv_inertia_world, cross(js.rb, impulse)); }
            };
            if (js.kind == 3) {
                // Slider: no sideways drift of the anchors, then the motor and the limit along the
                // axis (a positive impulse along the axis moves this body's anchor further along it).
                auto relv = [&]() {
                    Vec3 va2 = a_dyn || a.kind == 2 ? a.velocity + cross(a.angular, js.ra) : Vec3{0, 0, 0};
                    Vec3 vb2 = bp ? (bp->kind != 1 ? bp->velocity + cross(bp->angular, js.rb) : Vec3{0, 0, 0}) : Vec3{0, 0, 0};
                    return vb2 - va2;
                };
                for (const auto& [t, k] : {std::pair{js.t1, js.k_l1}, std::pair{js.t2, js.k_l2}}) {
                    if (k <= 0) continue;
                    float dj = dot(relv(), t) / k;
                    apply(t * dj);
                    js.impulse += t * dj;
                }
                if (js.motor_force > 0 && js.k_l_axis > 0) {
                    float dj = (js.motor_speed + dot(relv(), js.axis_w)) / js.k_l_axis;
                    const float max_impulse = js.motor_force * dt;
                    float next = std::clamp(js.slide_motor_impulse + dj, -max_impulse, max_impulse);
                    dj = next - js.slide_motor_impulse;
                    js.slide_motor_impulse = next;
                    apply(js.axis_w * dj);
                }
                if (js.slide_state != 0 && js.k_l_axis > 0) {
                    float dj = dot(relv(), js.axis_w) / js.k_l_axis;
                    float next = js.slide_limit_impulse + dj;
                    if (js.slide_state == -1) next = std::max(next, 0.0f);
                    if (js.slide_state == 1) next = std::min(next, 0.0f);
                    dj = next - js.slide_limit_impulse;
                    js.slide_limit_impulse = next;
                    apply(js.axis_w * dj);
                }
            } else if (js.kind != 0) {
                // Want rel + bias = 0: impulse = K^-1 (rel + bias).
                Vec3 target = rel + js.n;
                Vec3 imp = mul3(js.k_inv, target);
                apply(imp);
                js.impulse += imp;
            } else {
                if (js.k <= 0) continue;
                float vn = dot(rel, js.n);   // separating speed along the rod
                float acc = dot(js.impulse, js.n);
                float dj = js.spring ? (vn + js.soft_bias - js.gamma * acc) / (js.k + js.gamma) : (vn + js.bias) / js.k;
                float next = acc + dj;
                if (js.rope && next < 0) next = 0;  // a rope (or a bungee) only pulls
                dj = next - acc;
                apply(js.n * dj);
                js.impulse += js.n * dj;
            }
        }
        for (std::size_t mi = 0; mi < manifolds.size(); ++mi) {
            Manifold& m = manifolds[mi];
            if (m.trigger || states[mi].empty()) continue;
            Body& a = im.bodies[m.a];
            Body& b = im.bodies[m.b];
            float mu = std::sqrt(std::max(0.0f, a.friction * b.friction));
            for (PointState& ps : states[mi]) {
                auto apply = [&](Vec3 impulse) {
                    if (a.kind == 0) { a.velocity -= impulse * a.inv_mass; a.angular -= mul3(a.inv_inertia_world, cross(ps.ra, impulse)); }
                    if (b.kind == 0) { b.velocity += impulse * b.inv_mass; b.angular += mul3(b.inv_inertia_world, cross(ps.rb, impulse)); }
                };
                // Normal.
                Vec3 va = a.velocity + cross(a.angular, ps.ra);
                Vec3 vb = b.velocity + cross(b.angular, ps.rb);
                Vec3 rel = vb - va;
                float vn = dot(rel, m.normal);
                if (ps.k_n > 0) {
                    float dj = (-vn + ps.bias) / ps.k_n;
                    float next = std::max(ps.jn + dj, 0.0f);
                    dj = next - ps.jn;
                    ps.jn = next;
                    apply(m.normal * dj);
                }
                // Friction.
                if (mu > 0) {
                    float max_f = mu * ps.jn;
                    va = a.velocity + cross(a.angular, ps.ra);
                    vb = b.velocity + cross(b.angular, ps.rb);
                    rel = vb - va;
                    if (ps.k_t1 > 0) {
                        float dj = -dot(rel, ps.t1) / ps.k_t1;
                        float next = std::clamp(ps.jt1 + dj, -max_f, max_f);
                        dj = next - ps.jt1;
                        ps.jt1 = next;
                        apply(ps.t1 * dj);
                    }
                    va = a.velocity + cross(a.angular, ps.ra);
                    vb = b.velocity + cross(b.angular, ps.rb);
                    rel = vb - va;
                    if (ps.k_t2 > 0) {
                        float dj = -dot(rel, ps.t2) / ps.k_t2;
                        float next = std::clamp(ps.jt2 + dj, -max_f, max_f);
                        dj = next - ps.jt2;
                        ps.jt2 = next;
                        apply(ps.t2 * dj);
                    }
                }
            }
        }
    }
    // Joint forces and breaking.
    im.joint_infos.clear();
    std::vector<EntityId> broken;
    for (JointState& js : joints) {
        float force = length(js.impulse) / dt;
        if (js.spring) {
            // A spring whose bodies sleep still holds its load: report the static pull.
            const Body& a = im.bodies[js.a];
            const bool solved = (a.kind == 0 && !a.sleeping) || (js.b_is_body && im.bodies[js.b].kind == 0 && !im.bodies[js.b].sleeping);
            if (!solved) force = js.stiffness * std::fabs(length(js.pb - js.pa) - js.length);
        }
        JointInfo info;
        info.entity = js.entity;
        info.target = js.target;
        info.kind = js.kind;
        info.length = js.length;
        info.current = length(js.pb - js.pa);
        info.force = force;
        info.angle = js.angle;
        info.speed = js.speed;
        info.torque = (js.kind == 3 ? js.slide_motor_impulse : js.motor_impulse) / static_cast<float>(dt);
        info.limit_state = js.kind == 3 ? js.slide_state : js.limit_state;
        info.translation = js.translation;
        im.joint_infos.push_back(info);
        if (js.break_force > 0 && force > js.break_force) broken.push_back(js.entity);
        if (js.kind == 2 || js.kind == 3) {
            const int state = js.kind == 3 ? js.slide_state : js.limit_state;
            int& last = im.limit_states[js.entity];
            if (state != 0 && state != last) {
                Json data;
                data["path"] = w.path(js.entity);
                if (js.kind == 2) data["angle"] = js.angle;
                else data["translation"] = js.translation;
                data["limit"] = state == -1 ? "lower" : state == 1 ? "upper" : "locked";
                w.events().emit(w.tick_index(), "joint.limit", js.entity, data);
            }
            last = state;
        }
    }
    for (EntityId id : broken) {
        Json data;
        data["path"] = w.path(id);
        std::uint64_t seq = w.events().emit(w.tick_index(), "joint.broken", id, data);
        (void)w.remove(id, "Joint", seq);
        im.stats.broken++;
    }
    for (const auto& [id, len] : lengths_to_write) {
        if (w.has(id, "Joint")) (void)w.set(id, "Joint", Json{{"distance", len}});
    }
    for (const auto& [id, axis_b, ref_b] : frames_to_write) {
        if (w.has(id, "Joint")) (void)w.set(id, "Joint", Json{{"target_axis", {{"x", axis_b.x}, {"y", axis_b.y}, {"z", axis_b.z}}}, {"reference", {{"x", ref_b.x}, {"y", ref_b.y}, {"z", ref_b.z}}}});
    }
    for (const JointInfo& info : im.joint_infos) {
        if (!w.has(info.entity, "Joint")) continue;
        if (info.kind == 2) (void)w.set(info.entity, "Joint", Json{{"force", info.force}, {"angle", info.angle}, {"speed", info.speed}});
        else if (info.kind == 3) (void)w.set(info.entity, "Joint", Json{{"force", info.force}, {"translation", info.translation}, {"speed", info.speed}});
        else (void)w.set(info.entity, "Joint", Json{{"force", info.force}});
    }
    // Continuous collision: a body that asked for it is swept along this step's motion, turning
    // as it goes, relative to each shape it may touch (static, kinematic, or dynamic and moving
    // too), and stops a skin short of the first impact; the impact exchanges an impulse at the
    // contact point along the normal with the restitution (the body's spin counts, so a blade's
    // tip is stopped and the blade turned back), the rest of the step's motion is dropped for
    // both, and the contact solver takes over next step. A sphere is cast exactly against the
    // other's shape (a rounded box, a grown capsule or sphere, the triangles' offset faces and
    // edges); a box or capsule is swept by samples finer than half its thinnest extent along its
    // farthest point's path, bisected back to the impact.
    for (Body& b : im.bodies) b.swept = false;
    for (std::size_t bi = 0; bi < im.bodies.size(); ++bi) {
        Body& b = im.bodies[bi];
        if (!b.ccd || b.kind != 0 || b.sleeping || b.trigger || b.swept) continue;
        const Vec3 motion = b.velocity * dt;
        const float dist = length(motion);
        const float radius = bounding_radius(b);
        const Vec3 spin = b.shape == 1 || b.lock_rotation ? Vec3{0, 0, 0} : b.angular * dt;   // a sphere's turn moves nothing
        const float arc = length(spin) * radius;
        if (dist + arc <= radius * 0.5f) continue;  // slow for its size: the discrete step is enough
        const Vec3 swept_min{std::min(b.aabb_min.x, b.aabb_min.x + motion.x) - arc, std::min(b.aabb_min.y, b.aabb_min.y + motion.y) - arc, std::min(b.aabb_min.z, b.aabb_min.z + motion.z) - arc};
        const Vec3 swept_max{std::max(b.aabb_max.x, b.aabb_max.x + motion.x) + arc, std::max(b.aabb_max.y, b.aabb_max.y + motion.y) + arc, std::max(b.aabb_max.z, b.aabb_max.z + motion.z) + arc};
        float best_f = 1.0f;   // the fraction of the step at the first impact
        Vec3 best_n, best_p;
        bool best_exact = false;   // found by an exact cast rather than by samples
        std::size_t hit_i = im.bodies.size();
        for (std::size_t oi = 0; oi < im.bodies.size(); ++oi) {
            const Body& o = im.bodies[oi];
            if (oi == bi || o.trigger || o.swept) continue;
            const bool o_moves = (o.kind == 0 && !o.sleeping) || o.kind == 2;
            const Vec3 o_motion = o_moves ? o.velocity * dt : Vec3{0, 0, 0};
            // The other's turn over the step counts too (a spinning bar meets what flies past
            // where it will be), sampled with the body's sweep; a sphere's turn moves nothing.
            const Vec3 o_spin = o_moves && o.shape != 1 && !o.lock_rotation ? o.angular * dt : Vec3{0, 0, 0};
            const float o_arc = length(o_spin) * bounding_radius(o);
            const Vec3 o_min{std::min(o.aabb_min.x, o.aabb_min.x + o_motion.x) - o_arc, std::min(o.aabb_min.y, o.aabb_min.y + o_motion.y) - o_arc, std::min(o.aabb_min.z, o.aabb_min.z + o_motion.z) - o_arc};
            const Vec3 o_max{std::max(o.aabb_max.x, o.aabb_max.x + o_motion.x) + o_arc, std::max(o.aabb_max.y, o.aabb_max.y + o_motion.y) + o_arc, std::max(o.aabb_max.z, o.aabb_max.z + o_motion.z) + o_arc};
            if (o_min.x > swept_max.x || o_max.x < swept_min.x || o_min.y > swept_max.y || o_max.y < swept_min.y || o_min.z > swept_max.z || o_max.z < swept_min.z) continue;
            if (!allowed(b, o)) continue;
            // The other is held in place and the body moves by the motion between them.
            const Vec3 rel = motion - o_motion;
            const float rel_len = length(rel);
            if (rel_len <= 1e-6f && arc <= 1e-6f && o_arc <= 1e-6f) continue;
            float f = 1.0f;
            Vec3 n, pt;
            if (b.shape == 1 && o_arc <= 1e-6f) {
                if (rel_len <= 1e-6f) continue;
                float t = 0;
                if (!sphere_cast_body(b.position, rel * (1.0f / rel_len), b.half.x, o, rel_len * best_f, t, n)) continue;
                f = t / rel_len;
                pt = b.position + rel * f - n * b.half.x;
            } else if (!stepped_sweep(b, rel, spin, o, o_spin, best_f, f, n, pt)) {
                continue;
            }
            if (f >= best_f) continue;
            best_f = f;
            best_n = n;
            best_p = pt;
            best_exact = b.shape == 1 && o_arc <= 1e-6f;
            hit_i = oi;
        }
        if (hit_i >= im.bodies.size()) continue;
        Body& o = im.bodies[hit_i];
        const bool dynamic = o.kind == 0 && !o.sleeping;
        const Vec3 o_motion = dynamic || o.kind == 2 ? o.velocity * dt : Vec3{0, 0, 0};
        const Vec3 o_spin = (dynamic || o.kind == 2) && o.shape != 1 && !o.lock_rotation ? o.angular * dt : Vec3{0, 0, 0};
        const float o_arc = length(o_spin) * bounding_radius(o);
        const float rel_len = length(motion - o_motion);
        const float stop = std::max(best_f - s.slop / std::max(rel_len + arc + o_arc, 1e-6f), 0.0f);   // a skin short
        b.position += motion * stop;
        if (arc > 0.0f) b.rotation = normalize(Quat::from_axis_angle(spin, length(spin) * stop) * b.rotation);
        b.swept = true;
        b.touched = true;
        if (dynamic) {
            o.position += o_motion * stop;
            if (o_arc > 0.0f) o.rotation = normalize(Quat::from_axis_angle(o_spin, length(o_spin) * stop) * o.rotation);
            o.swept = true;
            o.touched = true;
        }
        // The impulse at the contact point: the spin of either body counts, so a turning body is
        // turned back as well as stopped.
        const Vec3 point = best_exact ? b.position - best_n * b.half.x : best_p + motion * (stop - best_f);
        const Vec3 rb = point - b.position, ro = point - o.position;
        const Vec3 vb = b.velocity + (b.lock_rotation ? Vec3{0, 0, 0} : cross(b.angular, rb));
        const Vec3 vo = dynamic ? o.velocity + (o.lock_rotation ? Vec3{0, 0, 0} : cross(o.angular, ro)) : (o.kind == 2 ? o.velocity : Vec3{0, 0, 0});
        const float vn = dot(vb - vo, best_n);
        if (vn < 0) {
            const float e = b.restitution;   // the swept body's: a pellet with none stays where it stopped
            float k = b.inv_mass + (dynamic ? o.inv_mass : 0.0f);
            if (!b.lock_rotation) k += dot(best_n, cross(mul3(b.inv_inertia_world, cross(rb, best_n)), rb));
            if (dynamic && !o.lock_rotation) k += dot(best_n, cross(mul3(o.inv_inertia_world, cross(ro, best_n)), ro));
            const float j = -(1.0f + e) * vn / std::max(k, 1e-9f);
            b.velocity += best_n * (j * b.inv_mass);
            if (!b.lock_rotation) b.angular += mul3(b.inv_inertia_world, cross(rb, best_n * j));
            if (dynamic) {
                o.velocity -= best_n * (j * o.inv_mass);
                if (!o.lock_rotation) o.angular -= mul3(o.inv_inertia_world, cross(ro, best_n * j));
            }
        }
        im.stats.ccd_hits++;
        if (dynamic) im.stats.ccd_dynamic++;
        w.events().emit(w.tick_index(), "physics.ccd", b.id, Json{{"path", w.path(b.id)}, {"other", w.path(o.id)}, {"dynamic", dynamic}, {"exact", best_exact}, {"point", {{"x", point.x}, {"y", point.y}, {"z", point.z}}}, {"normal", {{"x", best_n.x}, {"y", best_n.y}, {"z", best_n.z}}}, {"speed", -vn}, {"fraction", best_f}});
    }
    // 4. Integrate, project out remaining penetration, sleep.
    for (Body& b : im.bodies) {
        if (b.kind == 1) continue;
        if (b.kind == 0 && b.sleeping) continue;
        if (b.lock_rotation) b.angular = {};
        if (!b.swept) b.position += b.velocity * dt;
        float w_len = length(b.angular);
        if (w_len > 1e-6f && !b.swept) b.rotation = normalize(Quat::from_axis_angle(b.angular, w_len * dt) * b.rotation);   // a swept body turned as far as its impact
        if (b.kind == 0) {
            // Snap to rest: creep below these thresholds is solver noise, not motion, and would
            // otherwise tilt stacks and start balls rolling.
            if (w_len < 0.02f) { b.angular = {}; w_len = 0; }
            if (length(b.velocity) < 0.004f) b.velocity = {};
            if (length(b.velocity) < s.sleep_linear && w_len < s.sleep_angular) {
                b.sleep_timer += dt;
                if (b.sleep_timer >= s.sleep_seconds) {
                    b.sleeping = true;
                    b.velocity = {};
                    b.angular = {};
                }
            } else {
                b.sleep_timer = 0;
            }
        }
    }
    // Position projection: push overlapping pairs apart along the normal, split by inverse mass.
    for (const Manifold& m : manifolds) {
        if (m.trigger) continue;
        Body& a = im.bodies[m.a];
        Body& b = im.bodies[m.b];
        float inv_mass_sum = a.inv_mass + b.inv_mass;
        if (inv_mass_sum == 0) continue;
        float depth = 0;
        for (float d : m.depths) depth = std::max(depth, d);
        float correction = std::max(depth - s.slop, 0.0f) * s.baumgarte;
        if (correction <= 0) continue;
        Vec3 shift = m.normal * (correction / inv_mass_sum);
        if (a.kind == 0 && !a.sleeping) a.position -= shift * a.inv_mass;
        if (b.kind == 0 && !b.sleeping) b.position += shift * b.inv_mass;
    }
    // Joint position projection (Box2D's position solver): after integration, move the bodies so
    // the anchors meet again, splitting the correction by the same effective mass the velocity
    // pass used, so a fast pendulum neither stretches nor gains energy from a velocity bias.
    const float max_correction = 0.2f;
    auto turn = [](Body& b, Vec3 w_disp) {
        float len = length(w_disp);
        if (len > 1e-7f) b.rotation = normalize(Quat::from_axis_angle(w_disp, len) * b.rotation);
    };
    for (int iter = 0; iter < 3 && !joints.empty(); ++iter) {
        for (JointState& js : joints) {
            Body& a = im.bodies[js.a];
            Body* bp = js.b_is_body ? &im.bodies[js.b] : nullptr;
            bool a_dyn = a.kind == 0 && !a.sleeping, b_dyn = bp && bp->kind == 0 && !bp->sleeping;
            if (!a_dyn && !b_dyn) continue;
            float inv_a = a_dyn ? a.inv_mass : 0.0f, inv_b = b_dyn ? bp->inv_mass : 0.0f;
            Vec3 ra = a.rotation.rotate(js.anchor_a);
            Vec3 pa = a.position + ra;
            Vec3 rb = bp ? bp->rotation.rotate(js.anchor_b) : Vec3{};
            Vec3 pb = bp ? bp->position + rb : js.pb;
            Vec3 d = pb - pa;
            Vec3 P;
            bool has_p = false;
            if (js.kind == 1 || js.kind == 2) {
                if (length(d) >= s.slop) {
                    Mat4 K;
                    for (float& v : K.m) v = 0;
                    for (int i = 0; i < 3; ++i) K.at(i, i) = inv_a + inv_b;
                    auto add_term = [&](Vec3 r, const Mat4& iw) {
                        float rx[3][3] = {{0, -r.z, r.y}, {r.z, 0, -r.x}, {-r.y, r.x, 0}};
                        float t1[3][3] = {};
                        for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) for (int k = 0; k < 3; ++k) t1[i][j] += iw.at(k, i) * rx[k][j];
                        for (int i = 0; i < 3; ++i) for (int j = 0; j < 3; ++j) { float acc = 0; for (int k = 0; k < 3; ++k) acc += rx[k][i] * t1[k][j]; K.at(j, i) += acc; }
                    };
                    if (a_dyn) add_term(ra, a.inv_inertia_world);
                    if (b_dyn) add_term(rb, bp->inv_inertia_world);
                    float m00 = K.at(0, 0), m01 = K.at(1, 0), m02 = K.at(2, 0), m10 = K.at(0, 1), m11 = K.at(1, 1), m12 = K.at(2, 1), m20 = K.at(0, 2), m21 = K.at(1, 2), m22 = K.at(2, 2);
                    float det = m00 * (m11 * m22 - m12 * m21) - m01 * (m10 * m22 - m12 * m20) + m02 * (m10 * m21 - m11 * m20);
                    if (std::fabs(det) > 1e-12f) {
                        float inv = 1.0f / det;
                        Vec3 c0{(m11 * m22 - m12 * m21) * inv, (m12 * m20 - m10 * m22) * inv, (m10 * m21 - m11 * m20) * inv};
                        Vec3 c1{(m02 * m21 - m01 * m22) * inv, (m00 * m22 - m02 * m20) * inv, (m01 * m20 - m00 * m21) * inv};
                        Vec3 c2{(m01 * m12 - m02 * m11) * inv, (m02 * m10 - m00 * m12) * inv, (m00 * m11 - m01 * m10) * inv};
                        P = c0 * d.x + c1 * d.y + c2 * d.z;
                        has_p = true;
                    }
                }
            } else if (js.kind == 3) {
                // Slider: pull the anchors back onto the axis line (the travel along it is free).
                Vec3 axis_w = a.rotation.rotate(js.axis_a);
                Vec3 perp = d - axis_w * dot(d, axis_w);
                float dist = length(perp);
                if (dist >= s.slop) {
                    float c = std::min(dist, max_correction);
                    Vec3 n = perp * (1.0f / dist);
                    float k = inv_a + inv_b;
                    if (a_dyn) k += dot(n, cross(mul3(a.inv_inertia_world, cross(ra, n)), ra));
                    if (b_dyn) k += dot(n, cross(mul3(bp->inv_inertia_world, cross(rb, n)), rb));
                    if (k > 0) {
                        P = n * (c / k);
                        has_p = true;
                    }
                }
            } else if (!js.spring) {
                float dist = length(d);
                float c = dist - js.length;
                if (!(js.rope && c < 0) && std::fabs(c) >= s.slop) {
                    c = std::clamp(c, -max_correction, max_correction);
                    Vec3 n = dist > 1e-6f ? d * (1.0f / dist) : Vec3{0, 1, 0};
                    float k = inv_a + inv_b;
                    if (a_dyn) k += dot(n, cross(mul3(a.inv_inertia_world, cross(ra, n)), ra));
                    if (b_dyn) k += dot(n, cross(mul3(bp->inv_inertia_world, cross(rb, n)), rb));
                    if (k > 0) {
                        P = n * (c / k);
                        has_p = true;
                    }
                }
            }
            if (has_p) {
                if (a_dyn) { a.position += P * inv_a; turn(a, mul3(a.inv_inertia_world, cross(ra, P))); }
                if (b_dyn) { bp->position -= P * inv_b; turn(*bp, mul3(bp->inv_inertia_world, cross(rb, P)) * -1.0f); }
            }
            if (js.kind == 2 || js.kind == 3) {
                // Hinge: bring the axes back together, then the angle back inside its limits, turning
                // each body by its share of the inverse inertia about the correction axis. A slider
                // goes through the same with its angle limited to zero: no turn at all.
                Quat rb_rot = bp ? bp->rotation : js.rot_b;
                Vec3 axis_a_w = a.rotation.rotate(js.axis_a), axis_b_w = rb_rot.rotate(js.axis_b);
                Vec3 c = cross(axis_a_w, axis_b_w);
                float sin_err = length(c);
                if (sin_err > 1e-5f) {
                    float ang = std::asin(std::min(1.0f, sin_err));
                    if (dot(axis_a_w, axis_b_w) < 0) ang = kPi - ang;
                    Vec3 n = c * (1.0f / sin_err);
                    float ka = a_dyn ? dot(n, mul3(a.inv_inertia_world, n)) : 0.0f, kb = b_dyn ? dot(n, mul3(bp->inv_inertia_world, n)) : 0.0f;
                    if (ka + kb > 0) {
                        if (a_dyn) turn(a, n * (ang * ka / (ka + kb)));
                        if (b_dyn) turn(*bp, n * (-ang * kb / (ka + kb)));
                    }
                }
                if (js.limit) {
                    rb_rot = bp ? bp->rotation : js.rot_b;
                    float angle = hinge_angle(js, a.rotation, rb_rot);
                    float lo = js.lower, hi = std::max(js.upper, js.lower);
                    float viol = angle < lo ? angle - lo : angle > hi ? angle - hi : 0.0f;
                    if (std::fabs(viol) > 1e-4f) {
                        viol = std::clamp(viol, -max_correction, max_correction);
                        Vec3 ax = a.rotation.rotate(js.axis_a);
                        float ka = a_dyn ? dot(ax, mul3(a.inv_inertia_world, ax)) : 0.0f, kb = b_dyn ? dot(ax, mul3(bp->inv_inertia_world, ax)) : 0.0f;
                        if (ka + kb > 0) {
                            if (a_dyn) turn(a, ax * (-viol * ka / (ka + kb)));
                            if (b_dyn) turn(*bp, ax * (viol * kb / (ka + kb)));
                        }
                    }
                }
            }
            if (js.kind == 3 && js.slide_limit) {
                // Slider: push the travel back inside its limits along the axis, split by mass.
                Vec3 axis_w = a.rotation.rotate(js.axis_a);
                Vec3 pa2 = a.position + a.rotation.rotate(js.anchor_a);
                Vec3 pb2 = bp ? bp->position + bp->rotation.rotate(js.anchor_b) : js.pb;
                float x = dot(pa2 - pb2, axis_w);
                float lo = js.slide_lower, hi = std::max(js.slide_upper, js.slide_lower);
                float viol = x < lo ? x - lo : x > hi ? x - hi : 0.0f;
                if (std::fabs(viol) > 1e-4f) {
                    viol = std::clamp(viol, -max_correction, max_correction);
                    float k = inv_a + inv_b;
                    if (k > 0) {
                        if (a_dyn) a.position -= axis_w * (viol * inv_a / k);
                        if (b_dyn) bp->position += axis_w * (viol * inv_b / k);
                    }
                }
            }
        }
    }
    // 5. Contacts and events.
    std::set<std::pair<EntityId, EntityId>> now;
    for (const Manifold& m : manifolds) {
        const Body& a = im.bodies[m.a];
        const Body& b = im.bodies[m.b];
        Contact c;
        c.a = a.id;
        c.b = b.id;
        c.normal = m.normal;
        c.point = m.points.empty() ? a.position : m.points[0];
        c.depth = m.depths.empty() ? 0 : m.depths[0];
        c.trigger = m.trigger;
        im.contacts.push_back(c);
        now.insert({a.id, b.id});
        std::pair<EntityId, EntityId> key{a.id, b.id};
        if (!im.touching.contains(key)) {
            Json data;
            data["a"] = w.path(a.id);
            data["b"] = w.path(b.id);
            data["point"] = Json{{"x", c.point.x}, {"y", c.point.y}, {"z", c.point.z}};
            data["normal"] = Json{{"x", c.normal.x}, {"y", c.normal.y}, {"z", c.normal.z}};
            data["speed"] = length(b.velocity - a.velocity);
            std::uint64_t seq = w.events().emit(w.tick_index(), m.trigger ? "trigger.enter" : "collision.begin", a.id, data);
            im.pair_cause[key] = seq;
            im.stats.begins++;
        }
    }
    for (const auto& key : im.touching) {
        if (!now.contains(key)) {
            // A pair that stopped being tested because a body fell asleep is still in contact.
            bool asleep = false;
            for (const Body& b : im.bodies) if ((b.id == key.first || b.id == key.second) && b.kind == 0 && b.sleeping) asleep = true;
            if (asleep && w.alive(key.first) && w.alive(key.second)) {
                now.insert(key);
                continue;
            }
            Json data;
            data["a"] = w.alive(key.first) ? w.path(key.first) : "";
            data["b"] = w.alive(key.second) ? w.path(key.second) : "";
            bool trig = false;
            for (const Body& b : im.bodies) if ((b.id == key.first || b.id == key.second) && b.trigger) trig = true;
            w.events().emit(w.tick_index(), trig ? "trigger.exit" : "collision.end", key.first, data, im.pair_cause[key]);
            im.pair_cause.erase(key);
            im.stats.ends++;
        }
    }
    im.touching = std::move(now);
    im.stats.contacts = static_cast<std::uint32_t>(im.contacts.size());
    for (const Body& b : im.bodies) if (b.kind == 0 && !b.sleeping) im.stats.awake++;
    im.sleep_timers.clear();
    for (const Body& b : im.bodies) if (b.kind == 0) im.sleep_timers[b.id] = b.sleep_timer;
    im.write_back(w, dt);
}

Result<RayHit> Physics::raycast(const world::World& w, Vec3 origin, Vec3 direction, float max_distance, bool include_triggers) const {
    return raycast(w, origin, direction, max_distance, [include_triggers](world::EntityId, const world::RigidBody&, const world::Collider& col) { return include_triggers || !col.is_trigger; });
}

Result<RayHit> Physics::raycast(const world::World& w, Vec3 origin, Vec3 direction, float max_distance, const Filter& accept) const {
    Vec3 dir = normalize(direction);
    if (length(dir) == 0) return fail("bad_args", "direction must be non-zero");
    RayHit best;
    best.distance = max_distance;
    bool found = false;
    w.ecs().each([&](flecs::entity e, const world::RigidBody& rb, const world::Collider& col, const world::Transform& t) {
        if (!accept(e.id(), rb, col)) return;
        Body b;
        b.position = t.position + t.rotation.rotate(col.offset);
        b.rotation = t.rotation;
        b.shape = col.shape;
        b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.shape == 2 ? Vec3{col.size.x, col.size.y, col.size.x} : col.size;
        float tt = 0;
        Vec3 n;
        bool hit = false;
        if (b.shape == 3) {
            // Down the tree, nearest triangle first found by shrinking the reach; both faces hit,
            // the normal facing the ray.
            const MeshShape* ms = impl_->mesh_for(e.id(), col, t, e.try_get<world::MeshRenderer>());
            if (!ms) return;
            float reach = best.distance;
            std::vector<std::uint32_t> stack{0};
            while (!stack.empty()) {
                const MeshShape::Node& node = ms->nodes[stack.back()];
                stack.pop_back();
                if (!ray_aabb(origin, dir, node.min, node.max, reach)) continue;
                if (node.count == 0) {
                    stack.push_back(node.left);
                    stack.push_back(node.right);
                    continue;
                }
                for (std::uint32_t k = node.first; k < node.first + node.count; ++k) {
                    std::uint32_t tri = ms->order[k];
                    float th = 0;
                    if (!ray_triangle(origin, dir, ms->v[ms->tri[tri][0]], ms->v[ms->tri[tri][1]], ms->v[ms->tri[tri][2]], th)) continue;
                    if (th >= reach) continue;
                    reach = th;
                    hit = true;
                    tt = th;
                    n = dot(ms->n[tri], dir) > 0 ? -ms->n[tri] : ms->n[tri];
                }
            }
        } else if (b.shape == 2) {
            hit = ray_capsule(origin, dir, b, tt, n);
        } else if (b.shape == 1) {
            hit = ray_sphere(origin, dir, b.position, b.half.x, tt);
            if (hit) n = normalize(origin + dir * tt - b.position);
        } else {
            hit = ray_box(origin, dir, b, tt, n);
        }
        if (hit && tt < best.distance && (!found || tt < best.distance || e.id() < best.entity)) {
            found = true;
            best.entity = e.id();
            best.distance = tt;
            best.point = origin + dir * tt;
            best.normal = n;
        }
    });
    if (!found) return fail("no_hit", "nothing within {} units", max_distance);
    return best;
}

Result<RayHit> Physics::sweep(const world::World& w, Vec3 origin, Vec3 direction, float radius, float max_distance, const Filter& accept) const {
    Vec3 dir = normalize(direction);
    if (length(dir) == 0) return fail("bad_args", "direction must be non-zero");
    if (radius <= 0) return fail("bad_args", "radius must be positive");
    RayHit best;
    best.distance = max_distance;
    bool found = false;
    w.ecs().each([&](flecs::entity e, const world::RigidBody& rb, const world::Collider& col, const world::Transform& t) {
        if (!accept(e.id(), rb, col)) return;
        Body b;
        b.position = t.position + t.rotation.rotate(col.offset);
        b.rotation = t.rotation;
        b.shape = col.shape;
        b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.shape == 2 ? Vec3{col.size.x, col.size.y, col.size.x} : col.size;
        if (b.shape == 3) {
            b.mesh = impl_->mesh_for(e.id(), col, t, e.try_get<world::MeshRenderer>());
            if (!b.mesh) return;
        }
        float tt = 0;
        Vec3 n;
        if (!sphere_cast_body(origin, dir, radius, b, best.distance, tt, n)) return;
        if (tt < best.distance || (tt == best.distance && (!found || e.id() < best.entity))) {
            found = true;
            best.entity = e.id();
            best.distance = tt;
            best.point = origin + dir * tt - n * radius;   // where the sphere touches
            best.normal = n;
        }
    });
    if (!found) return fail("no_hit", "nothing within {} units", max_distance);
    return best;
}

std::vector<world::EntityId> Physics::overlap_sphere(const world::World& w, Vec3 center, float radius) const {
    return overlap_sphere(w, center, radius, [](world::EntityId, const world::RigidBody&, const world::Collider&) { return true; });
}

std::vector<world::EntityId> Physics::overlap_sphere(const world::World& w, Vec3 center, float radius, const Filter& accept) const {
    std::vector<world::EntityId> out;
    Body probe;
    probe.shape = 1;
    probe.position = center;
    probe.half = {radius, radius, radius};
    w.ecs().each([&](flecs::entity e, const world::RigidBody& rb, const world::Collider& col, const world::Transform& t) {
        if (!accept(e.id(), rb, col)) return;
        Body b;
        b.position = t.position + t.rotation.rotate(col.offset);
        b.rotation = t.rotation;
        b.shape = col.shape;
        b.half = col.shape == 1 ? Vec3{col.size.x, col.size.x, col.size.x} : col.shape == 2 ? Vec3{col.size.x, col.size.y, col.size.x} : col.size;
        Manifold m;
        bool hit = false;
        if (b.shape == 3) {
            b.mesh = impl_->mesh_for(e.id(), col, t, e.try_get<world::MeshRenderer>());
            update_aabb(probe);
            hit = collide_mesh(probe, b, m, false);
        } else {
            hit = b.shape == 1 ? collide_ss(probe, b, m) : b.shape == 2 ? collide_cs(b, probe, m, true) : collide_sb(probe, b, m, false);
        }
        if (hit) out.push_back(e.id());
    });
    std::sort(out.begin(), out.end());
    return out;
}

Json Physics::describe() const {
    const StepStats& s = impl_->stats;
    Json j;
    j["bodies"] = s.bodies;
    j["awake"] = s.awake;
    j["pairs"] = s.pairs;
    j["contacts"] = s.contacts;
    j["begins"] = s.begins;
    j["ends"] = s.ends;
    j["joints"] = s.joints;
    j["broken"] = s.broken;
    j["meshes"] = s.meshes;
    j["triangles"] = s.triangles;
    j["ccd_hits"] = s.ccd_hits;
    j["ccd_dynamic"] = s.ccd_dynamic;
    j["ignored"] = s.ignored;
    j["exceptions"] = impl_->ignored.size();
    j["gravity"] = Json{{"x", impl_->settings.gravity.x}, {"y", impl_->settings.gravity.y}, {"z", impl_->settings.gravity.z}};
    return j;
}

}  // namespace pocket::physics
