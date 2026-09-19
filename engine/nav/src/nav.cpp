#include <pocket/nav/nav.hpp>

#include <algorithm>
#include <cmath>
#include <queue>

namespace pocket::nav {

using world::EntityId;

namespace {

constexpr float kSqrt2 = 1.41421356f;

bool step_ok(const Grid& g, std::size_t a, std::size_t b) {
    if (g.plane != 0 || g.ground.empty()) return true;
    return std::fabs(g.ground[a] - g.ground[b]) <= g.max_step + 1e-4f;
}

float link_cost(const Grid& g, std::size_t a, std::size_t b) {
    const int ax = static_cast<int>(a % static_cast<std::size_t>(g.width)), ay = static_cast<int>(a / static_cast<std::size_t>(g.width));
    const int bx = static_cast<int>(b % static_cast<std::size_t>(g.width)), by = static_cast<int>(b / static_cast<std::size_t>(g.width));
    return std::hypot(static_cast<float>(bx - ax), static_cast<float>(by - ay)) + 1.0f;  // a jump costs a little more than walking
}

// Orthogonal neighbours, diagonals between two free orthogonal cells (no corner cutting), the
// cell's extra links.
template <typename F>
void for_neighbours(const Grid& g, int x, int y, F&& f) {
    static constexpr int dx[8] = {1, -1, 0, 0, 1, 1, -1, -1};
    static constexpr int dy[8] = {0, 0, 1, -1, 1, -1, 1, -1};
    const std::size_t from = g.index(x, y);
    for (int i = 0; i < (g.diagonal ? 8 : 4); ++i) {
        const int nx = x + dx[i], ny = y + dy[i];
        if (!g.walkable_at(nx, ny)) continue;
        const std::size_t to = g.index(nx, ny);
        if (!step_ok(g, from, to)) continue;
        if (i >= 4) {
            if (!g.walkable_at(x + dx[i], y) || !g.walkable_at(x, y + dy[i])) continue;
            if (!step_ok(g, from, g.index(x + dx[i], y)) || !step_ok(g, from, g.index(x, y + dy[i]))) continue;
        }
        f(to, i >= 4 ? kSqrt2 : 1.0f);
    }
    if (!g.links.empty()) for (int to : g.links[from]) f(static_cast<std::size_t>(to), link_cost(g, from, static_cast<std::size_t>(to)));
}

float heuristic(const Grid& g, std::size_t a, std::size_t b) {
    const int ax = static_cast<int>(a % static_cast<std::size_t>(g.width)), ay = static_cast<int>(a / static_cast<std::size_t>(g.width));
    const int bx = static_cast<int>(b % static_cast<std::size_t>(g.width)), by = static_cast<int>(b / static_cast<std::size_t>(g.width));
    const float dx = static_cast<float>(std::abs(bx - ax)), dy = static_cast<float>(std::abs(by - ay));
    if (g.diagonal) return std::max(dx, dy) + (kSqrt2 - 1.0f) * std::min(dx, dy);
    return dx + dy;
}

// Whether a straight segment between two cell centers stays on walkable ground: sampled every
// quarter cell, each sample's cell walkable and within a step of the previous one.
bool line_of_sight(const Grid& g, int ax, int ay, int bx, int by) {
    const Vec3 a = g.center_of(ax, ay), b = g.center_of(bx, by);
    const float dx = b.x - a.x, dy = g.plane == 0 ? b.z - a.z : b.y - a.y;
    const float dist = std::hypot(dx, dy);
    const int samples = std::max(1, static_cast<int>(std::ceil(dist / (g.cell * 0.25f))));
    std::size_t prev = g.index(ax, ay);
    for (int i = 1; i <= samples; ++i) {
        const float t = static_cast<float>(i) / static_cast<float>(samples);
        Vec3 p = a;
        p.x += dx * t;
        if (g.plane == 0) p.z += dy * t; else p.y += dy * t;
        int cx, cy;
        if (!g.cell_of(p, cx, cy) || !g.walkable_at(cx, cy)) return false;
        const std::size_t cur = g.index(cx, cy);
        if (cur != prev) {
            if (!step_ok(g, prev, cur)) return false;
            // A diagonal crossing between two cells must have a free orthogonal neighbour on each side.
            const int px = static_cast<int>(prev % static_cast<std::size_t>(g.width)), py = static_cast<int>(prev / static_cast<std::size_t>(g.width));
            if (px != cx && py != cy && !g.walkable_at(px, cy) && !g.walkable_at(cx, py)) return false;
            prev = cur;
        }
    }
    return true;
}

}  // namespace

Vec3 Grid::center_of(int x, int y) const {
    const float cx = origin.x + (static_cast<float>(x) + 0.5f) * cell;
    if (plane == 0) return {cx, ground.empty() ? origin.y : ground[index(x, y)], origin.z + (static_cast<float>(y) + 0.5f) * cell};
    return {cx, origin.y - (static_cast<float>(y) + 0.5f) * cell, depth};
}

bool Grid::cell_of(Vec3 p, int& x, int& y) const {
    if (cell <= 0 || width <= 0) return false;
    x = static_cast<int>(std::floor((p.x - origin.x) / cell));
    y = plane == 0 ? static_cast<int>(std::floor((p.z - origin.z) / cell)) : static_cast<int>(std::floor((origin.y - p.y) / cell));
    return inside(x, y);
}

std::size_t Grid::walkable_count() const {
    std::size_t n = 0;
    for (std::uint8_t w : walkable) n += w != 0;
    return n;
}

std::size_t Grid::link_count() const {
    std::size_t n = 0;
    for (const auto& l : links) n += l.size();
    return n;
}

Status Nav::bake_colliders(const world::World& w, const physics::Physics& ph, const BakeParams& p, std::uint64_t tick) {
    if (p.cell <= 0) return fail("bad_args", "cell must be positive");
    if (p.max.x <= p.min.x || p.max.z <= p.min.z || p.max.y <= p.min.y) return fail("bad_args", "max must be above min on every axis");
    Grid g;
    g.cell = p.cell;
    g.plane = 0;
    g.diagonal = p.diagonal;
    g.max_step = p.max_step;
    g.width = std::max(1, static_cast<int>(std::ceil((p.max.x - p.min.x) / p.cell)));
    g.height = std::max(1, static_cast<int>(std::ceil((p.max.z - p.min.z) / p.cell)));
    if (static_cast<std::size_t>(g.width) * static_cast<std::size_t>(g.height) > 1000000) return fail("too_big", "the grid would have more than a million cells; use a larger cell or a smaller area");
    g.origin = {p.min.x, p.min.y, p.min.z};
    const std::size_t n = static_cast<std::size_t>(g.width) * static_cast<std::size_t>(g.height);
    g.walkable.assign(n, 0);
    g.ground.assign(n, 0.0f);
    const float cos_slope = std::cos(radians(p.max_slope_degrees));
    const physics::Physics::Filter static_only = [](EntityId, const world::RigidBody& rb, const world::Collider& col) { return rb.kind == 1 && !col.is_trigger; };
    const float top = p.max.y + 1.0f, span = (p.max.y - p.min.y) + 2.0f;
    for (int y = 0; y < g.height; ++y) {
        for (int x = 0; x < g.width; ++x) {
            const Vec3 c{g.origin.x + (static_cast<float>(x) + 0.5f) * p.cell, top, g.origin.z + (static_cast<float>(y) + 0.5f) * p.cell};
            auto hit = ph.raycast(w, c, Vec3{0, -1, 0}, span, static_only);
            if (!hit || hit->normal.y < cos_slope || hit->point.y < p.min.y || hit->point.y > p.max.y) continue;
            const Vec3 feet{c.x, hit->point.y + p.agent_radius + 0.05f, c.z};
            const Vec3 head{c.x, hit->point.y + std::max(p.agent_height - p.agent_radius, p.agent_radius + 0.05f), c.z};
            bool blocked = false;
            for (const Vec3& probe : {feet, head}) {
                for (EntityId id : ph.overlap_sphere(w, probe, p.agent_radius, static_only)) if (id != hit->entity) blocked = true;
            }
            if (blocked) continue;
            const std::size_t i = g.index(x, y);
            g.walkable[i] = 1;
            g.ground[i] = hit->point.y;
        }
    }
    g.source = "colliders";
    g.baked_tick = tick;
    grid_ = std::move(g);
    return {};
}

Status Nav::bake_tilemap(const world::World& w, assets::AssetStore& assets, EntityId map_entity, const TileBakeParams& p, std::uint64_t tick) {
    const auto* tmc = w.try_get<world::TileMap>(map_entity);
    if (!tmc) return fail("no_tilemap", "entity {} has no TileMap", map_entity);
    if (p.mode != "topdown" && p.mode != "platformer") return fail("bad_args", "mode must be topdown or platformer");
    POCKET_TRY(map, assets.tilemap(tmc->map));
    Vec3 origin{0, 0, 0};
    if (const auto* wt = w.try_get<world::WorldTransform>(map_entity)) origin = wt->position;
    else if (const auto* t = w.try_get<world::Transform>(map_entity)) origin = t->position;
    Grid g;
    g.plane = 1;
    g.cell = tmc->tile_size > 0 ? tmc->tile_size : 1.0f;
    g.width = map->width;
    g.height = map->height;
    g.origin = origin;
    g.depth = origin.z;
    const bool platformer = p.mode == "platformer";
    g.diagonal = !platformer && p.diagonal;
    const std::size_t n = static_cast<std::size_t>(g.width) * static_cast<std::size_t>(g.height);
    g.walkable.assign(n, 0);
    auto free = [&](int x, int y) { return map->solidity_at(x, y) == 0 && x >= 0 && y >= 0 && x < map->width && y < map->height; };
    auto clear_above = [&](int x, int y) {
        for (int k = 0; k < std::max(1, p.agent_height); ++k) if (!free(x, y - k)) return false;
        return true;
    };
    for (int y = 0; y < g.height; ++y) {
        for (int x = 0; x < g.width; ++x) {
            if (!clear_above(x, y)) continue;
            if (platformer && map->solidity_at(x, y + 1) == 0) continue;  // nothing to stand on
            g.walkable[g.index(x, y)] = 1;
        }
    }
    if (platformer) {
        // Jumps rise then cross; drops cross then fall; both need empty cells along the way.
        g.links.assign(n, {});
        auto column_free = [&](int x, int y0, int y1) {  // y0 > y1: upward; cells y0-1 .. y1 (inclusive)
            for (int y = std::min(y0, y1); y <= std::max(y0, y1); ++y) if (!free(x, y)) return false;
            return true;
        };
        auto row_free = [&](int y, int x0, int x1) {
            for (int x = std::min(x0, x1); x <= std::max(x0, x1); ++x) if (!free(x, y)) return false;
            return true;
        };
        for (int y = 0; y < g.height; ++y) {
            for (int x = 0; x < g.width; ++x) {
                if (!g.walkable_at(x, y)) continue;
                auto& out = g.links[g.index(x, y)];
                for (int dx = -p.jump_range; dx <= p.jump_range; ++dx) {
                    // Up: rise in place, then cross at the top row.
                    for (int up = 1; up <= p.jump_height; ++up) {
                        const int tx = x + dx, ty = y - up;
                        if (!g.walkable_at(tx, ty) || !column_free(x, y - 1, ty) || !row_free(ty, x, tx)) continue;
                        out.push_back(static_cast<int>(g.index(tx, ty)));
                    }
                    if (dx == 0) continue;
                    // Across: a gap in the same row (adjacent cells are ordinary neighbours).
                    if (std::abs(dx) >= 2 && g.walkable_at(x + dx, y) && row_free(y, x, x + dx)) out.push_back(static_cast<int>(g.index(x + dx, y)));
                    // Down: cross at this row, then fall.
                    for (int down = 1; down <= p.max_drop; ++down) {
                        const int tx = x + dx, ty = y + down;
                        if (!g.walkable_at(tx, ty) || !row_free(y, x, tx) || !column_free(tx, y, ty)) continue;
                        out.push_back(static_cast<int>(g.index(tx, ty)));
                        break;  // the first landing below is where a drop ends
                    }
                }
            }
        }
    }
    g.source = "tilemap:" + tmc->map + ":" + p.mode;
    g.baked_tick = tick;
    grid_ = std::move(g);
    return {};
}

std::optional<Vec3> Nav::nearest(Vec3 p, float max_radius) const {
    if (!baked()) return std::nullopt;
    const Grid& g = grid_;
    int cx, cy;
    // Cell coordinates of the point, allowed outside the grid for the scan.
    cx = static_cast<int>(std::floor((p.x - g.origin.x) / g.cell));
    cy = g.plane == 0 ? static_cast<int>(std::floor((p.z - g.origin.z) / g.cell)) : static_cast<int>(std::floor((g.origin.y - p.y) / g.cell));
    const int r = std::max(0, static_cast<int>(std::ceil(max_radius / g.cell)));
    float best = max_radius * max_radius;
    std::optional<Vec3> out;
    for (int y = cy - r; y <= cy + r; ++y) {
        for (int x = cx - r; x <= cx + r; ++x) {
            if (!g.walkable_at(x, y)) continue;
            const Vec3 c = g.center_of(x, y);
            // Distance in the plane plus, on ground grids, the height difference: a point beside a
            // pillar's foot is nearer to the ground cell next to it than to the pillar's top.
            const float dx = c.x - p.x, dy = g.plane == 0 ? c.z - p.z : c.y - p.y;
            const float dh = g.plane == 0 ? c.y - p.y : 0.0f;
            const float d2 = dx * dx + dy * dy + dh * dh;
            if (d2 <= best + 1e-6f && (!out || d2 < best)) { best = d2; out = c; }
        }
    }
    return out;
}

Result<Path> Nav::path(Vec3 from, Vec3 to, bool smooth) const {
    if (!baked()) return fail("not_baked", "no navigation grid yet: bake one with nav.bake");
    const Grid& g = grid_;
    Path out;
    auto locate = [&](Vec3 p, const char* what, int& x, int& y) -> Status {
        if (!g.cell_of(p, x, y)) {
            auto near = nearest(p, g.cell * 2.0f);
            if (!near) return fail("outside", "the {} is outside the navigation grid", what);
            out.snapped = true;
            (void)g.cell_of(*near, x, y);
            return {};
        }
        if (g.walkable_at(x, y)) return {};
        auto near = nearest(p, g.cell * 2.0f);
        if (!near) return fail("blocked", "the {} is not on walkable ground and nothing walkable is within two cells", what);
        out.snapped = true;
        (void)g.cell_of(*near, x, y);
        return {};
    };
    int sx, sy, gx, gy;
    POCKET_TRY_VOID(locate(from, "start", sx, sy));
    POCKET_TRY_VOID(locate(to, "goal", gx, gy));
    const std::size_t n = g.walkable.size();
    const std::size_t start = g.index(sx, sy), goal = g.index(gx, gy);
    struct Node { float f, gcost; std::size_t cell; std::uint32_t order; };
    struct Less { bool operator()(const Node& a, const Node& b) const { if (a.f != b.f) return a.f > b.f; if (a.gcost != b.gcost) return a.gcost < b.gcost; return a.order > b.order; } };
    std::priority_queue<Node, std::vector<Node>, Less> open;
    std::vector<float> cost(n, std::numeric_limits<float>::infinity());
    std::vector<int> came(n, -1);
    std::vector<std::uint8_t> closed(n, 0);
    std::uint32_t order = 0;
    cost[start] = 0;
    open.push({heuristic(g, start, goal), 0, start, order++});
    std::size_t best = start;
    float best_h = heuristic(g, start, goal);
    bool reached = false;
    while (!open.empty()) {
        const Node cur = open.top();
        open.pop();
        if (closed[cur.cell]) continue;
        closed[cur.cell] = 1;
        ++out.expanded;
        if (cur.cell == goal) { reached = true; break; }
        const float h = heuristic(g, cur.cell, goal);
        if (h < best_h) { best_h = h; best = cur.cell; }
        const int cx = static_cast<int>(cur.cell % static_cast<std::size_t>(g.width)), cy = static_cast<int>(cur.cell / static_cast<std::size_t>(g.width));
        for_neighbours(g, cx, cy, [&](std::size_t to_cell, float step) {
            if (closed[to_cell]) return;
            const float c = cost[cur.cell] + step;
            if (c < cost[to_cell]) {
                cost[to_cell] = c;
                came[to_cell] = static_cast<int>(cur.cell);
                open.push({c + heuristic(g, to_cell, goal), c, to_cell, order++});
            }
        });
    }
    const std::size_t end = reached ? goal : best;
    out.partial = !reached;
    std::vector<std::size_t> cells;
    for (int c = static_cast<int>(end); c >= 0; c = came[static_cast<std::size_t>(c)]) {
        cells.push_back(static_cast<std::size_t>(c));
        if (static_cast<std::size_t>(c) == start) break;
    }
    std::reverse(cells.begin(), cells.end());
    out.cells = static_cast<int>(cells.size());
    auto xy = [&](std::size_t c, int& x, int& y) { x = static_cast<int>(c % static_cast<std::size_t>(g.width)); y = static_cast<int>(c / static_cast<std::size_t>(g.width)); };
    std::vector<std::size_t> kept;
    if (smooth && g.links.empty() && cells.size() > 2) {
        // String pulling: keep a corner only when the next cell is out of sight of the last kept one.
        kept.push_back(cells[0]);
        std::size_t anchor = 0;
        for (std::size_t i = 2; i < cells.size(); ++i) {
            int ax, ay, bx, by;
            xy(cells[anchor], ax, ay);
            xy(cells[i], bx, by);
            if (!line_of_sight(g, ax, ay, bx, by)) {
                kept.push_back(cells[i - 1]);
                anchor = i - 1;
            }
        }
        kept.push_back(cells.back());
    } else {
        kept = cells;
    }
    for (std::size_t c : kept) {
        int x, y;
        xy(c, x, y);
        out.points.push_back(g.center_of(x, y));
    }
    for (std::size_t i = 1; i < out.points.size(); ++i) out.length += length(out.points[i] - out.points[i - 1]);
    return out;
}

bool Nav::reachable(Vec3 from, Vec3 to) const {
    if (!baked()) return false;
    int sx, sy, gx, gy;
    if (!grid_.cell_of(from, sx, sy) || !grid_.cell_of(to, gx, gy)) return false;
    if (!grid_.walkable_at(sx, sy) || !grid_.walkable_at(gx, gy)) return false;
    auto p = path(from, to, false);
    return p.has_value() && !p->partial && !p->snapped;
}

Json Nav::describe() const {
    Json j;
    j["baked"] = baked();
    if (!baked()) return j;
    const Grid& g = grid_;
    j["plane"] = g.plane == 0 ? "xz" : "xy";
    j["width"] = g.width;
    j["height"] = g.height;
    j["cell"] = g.cell;
    j["origin"] = Json{{"x", g.origin.x}, {"y", g.origin.y}, {"z", g.origin.z}};
    j["cells"] = g.walkable.size();
    j["walkable"] = g.walkable_count();
    j["links"] = g.link_count();
    j["diagonal"] = g.diagonal;
    j["max_step"] = g.max_step;
    j["source"] = g.source;
    j["baked_tick"] = g.baked_tick;
    return j;
}

}  // namespace pocket::nav
