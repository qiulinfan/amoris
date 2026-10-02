#include <pocket/nav/nav.hpp>

#include <algorithm>
#include <set>
#include <cmath>
#include <limits>
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
    return repro::hypot(static_cast<float>(g.x_of(b) - g.x_of(a)), static_cast<float>(g.y_of(b) - g.y_of(a))) + 1.0f;  // a jump costs a little more than walking
}

// Orthogonal neighbours, diagonals between two free orthogonal cells (no corner cutting), the
// cell's extra links.
// Whether a row (or column) of a staggered or hexagonal map is the shifted one.
bool stagger_index(const Grid& g, int i) { return (i % 2 != 0) == g.stagger_odd; }

// The distance between two cells' centers in cells: the step cost on grids whose cells are not squares.
float center_distance(const Grid& g, int ax, int ay, int bx, int by) {
    const Vec3 a = g.center_of(ax, ay), b = g.center_of(bx, by);
    const float dx = b.x - a.x, dy = g.plane == 0 ? b.z - a.z : b.y - a.y;
    return repro::hypot(dx, dy) / g.cell;
}

template <typename F>
void for_neighbours(const Grid& g, std::size_t from, F&& f) {
    const int x = g.x_of(from), y = g.y_of(from);
    if (g.layout >= 2) {
        // Staggered diamonds and hexagons: the two cells across an edge in each of the rows (or
        // columns) beside this one, picked by the stagger; hexagons add the two along the row (or
        // column), which share an edge too; diamonds touch those only at a corner, so they and the
        // two straight across two rows join with `diagonal`, and only between two free edge
        // neighbours (no corner cutting). Costs are the distances between centers.
        auto visit = [&](int nx, int ny) {
            if (!g.walkable_at(nx, ny)) return;
            const std::size_t to = g.index(nx, ny);
            if (!step_ok(g, from, to)) return;
            f(to, center_distance(g, x, y, nx, ny));
        };
        const bool shifted = g.stagger_y ? stagger_index(g, y) : stagger_index(g, x);
        int ex[4], ey[4];
        if (g.stagger_y) {
            const int a = shifted ? x : x - 1, b = shifted ? x + 1 : x;
            ex[0] = a; ey[0] = y - 1; ex[1] = b; ey[1] = y - 1; ex[2] = a; ey[2] = y + 1; ex[3] = b; ey[3] = y + 1;
        } else {
            const int a = shifted ? y : y - 1, b = shifted ? y + 1 : y;
            ex[0] = x - 1; ey[0] = a; ex[1] = x - 1; ey[1] = b; ex[2] = x + 1; ey[2] = a; ex[3] = x + 1; ey[3] = b;
        }
        for (int i = 0; i < 4; ++i) visit(ex[i], ey[i]);
        const int ax = g.stagger_y ? 1 : 0, ay = g.stagger_y ? 0 : 1;
        if (g.layout == 3) {
            visit(x + ax, y + ay);
            visit(x - ax, y - ay);
        } else if (g.diagonal) {
            auto corner = [&](int nx, int ny, int i0, int i1) {
                if (g.walkable_at(ex[i0], ey[i0]) && g.walkable_at(ex[i1], ey[i1])) visit(nx, ny);
            };
            if (g.stagger_y) {
                corner(x + 1, y, 1, 3);
                corner(x - 1, y, 0, 2);
                corner(x, y - 2, 0, 1);
                corner(x, y + 2, 2, 3);
            } else {
                corner(x, y + 1, 1, 3);
                corner(x, y - 1, 0, 2);
                corner(x - 2, y, 0, 1);
                corner(x + 2, y, 2, 3);
            }
        }
        if (!g.links.empty()) for (int to : g.links[from]) f(static_cast<std::size_t>(to), link_cost(g, from, static_cast<std::size_t>(to)));
        return;
    }
    // Squares: the eight around, on whichever floor of each column is a step away (on a stack of
    // floors, the one the walker can step onto), diagonals only past two such orthogonal cells.
    static constexpr int dx[8] = {1, -1, 0, 0, 1, 1, -1, -1};
    static constexpr int dy[8] = {0, 0, 1, -1, 1, -1, 1, -1};
    for (int i = 0; i < (g.diagonal ? 8 : 4); ++i) {
        const int nx = x + dx[i], ny = y + dy[i];
        const std::optional<std::size_t> to = g.step_to(from, nx, ny);
        if (!to) continue;
        if (i >= 4 && (!g.step_to(from, x + dx[i], y) || !g.step_to(from, x, y + dy[i]))) continue;
        f(*to, g.layout == 1 ? center_distance(g, x, y, nx, ny) : (i >= 4 ? kSqrt2 : 1.0f));
    }
    if (!g.links.empty()) for (int to : g.links[from]) f(static_cast<std::size_t>(to), link_cost(g, from, static_cast<std::size_t>(to)));
}

float heuristic(const Grid& g, std::size_t a, std::size_t b) {
    const int ax = g.x_of(a), ay = g.y_of(a), bx = g.x_of(b), by = g.y_of(b);
    if (g.layout != 0) return center_distance(g, ax, ay, bx, by);   // the straight line, never more than the steps
    const float dx = static_cast<float>(std::abs(bx - ax)), dy = static_cast<float>(std::abs(by - ay));
    if (g.diagonal) return std::max(dx, dy) + (kSqrt2 - 1.0f) * std::min(dx, dy);
    return dx + dy;
}

// Whether a straight segment between two cells' centers stays on walkable ground: sampled every
// quarter cell, each sample on the floor of its column a step from the previous sample's (so a line
// on an upper floor stays on it, and one up a ramp climbs it), ending on the second cell.
bool line_of_sight(const Grid& g, std::size_t from, std::size_t to) {
    const Vec3 a = g.center_at(from), b = g.center_at(to);
    const float dx = b.x - a.x, dy = g.plane == 0 ? b.z - a.z : b.y - a.y;
    const float dist = repro::hypot(dx, dy);
    const int samples = std::max(1, static_cast<int>(std::ceil(dist / (g.cell * 0.25f))));
    std::size_t prev = from;
    for (int i = 1; i <= samples; ++i) {
        const float t = static_cast<float>(i) / static_cast<float>(samples);
        Vec3 p = a;
        p.x += dx * t;
        if (g.plane == 0) p.z += dy * t; else p.y += dy * t;
        int cx, cy;
        if (!g.cell_of(p, cx, cy)) return false;
        if (cx == g.x_of(prev) && cy == g.y_of(prev)) continue;
        const std::optional<std::size_t> cur = g.step_to(prev, cx, cy);
        if (!cur) return false;
        // A diagonal crossing between two cells must have a free orthogonal neighbour on each side.
        const int px = g.x_of(prev), py = g.y_of(prev);
        if (g.layout == 0 && px != cx && py != cy && !g.step_to(prev, px, cy) && !g.step_to(prev, cx, py)) return false;
        prev = *cur;
    }
    return prev == to;
}

}  // namespace

namespace {
// Tile maps that are not orthogonal keep their lattice but place cells as the map draws them
// (docs/design/tilemaps.md, Orientations), in map units: x right and y down from the map's top-left.

Vec2 layout_corner(const Grid& g, int x, int y) {   // the top-left of a cell's box
    if (g.layout == 1) return {(static_cast<float>(x - y) + static_cast<float>(g.height - 1)) * g.tile_w * 0.5f, static_cast<float>(x + y) * g.tile_h * 0.5f};
    if (g.stagger_y) return {static_cast<float>(x) * g.tile_w + (stagger_index(g, y) ? g.tile_w * 0.5f : 0.0f), static_cast<float>(y) * (g.tile_h + g.hex_side) * 0.5f};
    return {static_cast<float>(x) * (g.tile_w + g.hex_side) * 0.5f, static_cast<float>(y) * g.tile_h + (stagger_index(g, x) ? g.tile_h * 0.5f : 0.0f)};
}

void layout_cell(const Grid& g, float px, float py, int& x, int& y) {   // the cell under a point, allowed outside the grid
    if (g.layout == 1) {
        const float ux = (px - static_cast<float>(g.height) * g.tile_w * 0.5f) / g.tile_w, uy = py / g.tile_h;
        x = static_cast<int>(std::floor(uy + ux));
        y = static_cast<int>(std::floor(uy - ux));
        return;
    }
    // The cell whose center is nearest among the box grid's guess and its neighbours; a diamond's
    // distances are measured with the axes scaled to the tile.
    const float sx = g.layout == 3 ? 1.0f : 1.0f / g.tile_w, sy = g.layout == 3 ? 1.0f : 1.0f / g.tile_h;
    int gx, gy;
    if (g.stagger_y) { gy = static_cast<int>(std::floor(py / ((g.tile_h + g.hex_side) * 0.5f))); gx = static_cast<int>(std::floor((px - (stagger_index(g, gy) ? g.tile_w * 0.5f : 0.0f)) / g.tile_w)); }
    else { gx = static_cast<int>(std::floor(px / ((g.tile_w + g.hex_side) * 0.5f))); gy = static_cast<int>(std::floor((py - (stagger_index(g, gx) ? g.tile_h * 0.5f : 0.0f)) / g.tile_h)); }
    float best = std::numeric_limits<float>::max();
    x = gx;
    y = gy;
    for (int cy = gy - 1; cy <= gy + 1; ++cy) {
        for (int cx = gx - 1; cx <= gx + 1; ++cx) {
            const Vec2 c = layout_corner(g, cx, cy);
            const float dx = (px - (c.x + g.tile_w * 0.5f)) * sx, dy = (py - (c.y + g.tile_h * 0.5f)) * sy;
            const float d = dx * dx + dy * dy;
            if (d < best) { best = d; x = cx; y = cy; }
        }
    }
}
}  // namespace

Vec3 Grid::center_of(int x, int y) const {
    if (layout != 0 && plane == 1) {
        const Vec2 c = layout_corner(*this, x, y);
        return {origin.x + c.x + tile_w * 0.5f, origin.y - (c.y + tile_h * 0.5f), depth};
    }
    const float cx = origin.x + (static_cast<float>(x) + 0.5f) * cell;
    if (plane == 0) return {cx, ground.empty() ? origin.y : ground[index(x, y)], origin.z + (static_cast<float>(y) + 0.5f) * cell};
    return {cx, origin.y - (static_cast<float>(y) + 0.5f) * cell, depth};
}

Vec3 Grid::center_at(std::size_t i) const {
    Vec3 c = center_of(x_of(i), y_of(i));
    if (plane == 0 && i < ground.size()) c.y = ground[i];
    return c;
}

std::optional<std::size_t> Grid::step_to(std::size_t from, int x, int y) const {
    if (!inside(x, y)) return std::nullopt;
    if (layers == 1) {
        const std::size_t to = index(x, y);
        if (!walkable_cell(to) || !step_ok(*this, from, to)) return std::nullopt;
        return to;
    }
    // Floors are far more than a step apart, so at most one is within a step; the nearest wins.
    std::optional<std::size_t> best;
    float best_d = max_step + 1e-4f;
    for (int l = 0; l < layers; ++l) {
        const std::size_t to = index(x, y, l);
        if (!walkable_cell(to)) continue;
        const float d = std::fabs(ground[to] - ground[from]);
        if (d <= best_d) { best_d = d; best = to; }
    }
    return best;
}

std::optional<std::size_t> Grid::cell_at(Vec3 p) const {
    int x, y;
    if (!cell_of(p, x, y)) return std::nullopt;
    if (layers == 1) return walkable_at(x, y) ? std::optional<std::size_t>(index(x, y)) : std::nullopt;
    // The floor it stands on: the highest at or below a step over its feet; else the lowest above.
    std::optional<std::size_t> below, above;
    for (int l = 0; l < layers; ++l) {
        const std::size_t i = index(x, y, l);
        if (!walkable_cell(i)) continue;
        if (ground[i] <= p.y + max_step + 1e-4f) {
            if (!below || ground[i] > ground[*below]) below = i;
        } else if (!above || ground[i] < ground[*above]) {
            above = i;
        }
    }
    return below ? below : above;
}

bool Grid::cell_of(Vec3 p, int& x, int& y) const {
    if (cell <= 0 || width <= 0) return false;
    if (layout != 0 && plane == 1) {
        layout_cell(*this, p.x - origin.x, origin.y - p.y, x, y);
        return inside(x, y);
    }
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

std::size_t Grid::blocked_count() const {
    std::size_t n = 0;
    for (std::uint8_t b : blocked) n += b != 0;
    return n;
}

void Nav::set_grid(Grid grid) {
    mesh_ = NavMesh{};
    grid_ = std::move(grid);
    runs_.clear();
    build_mesh();
    reapply_obstacles();
}

void Nav::clear() {
    mesh_ = NavMesh{};
    grid_ = Grid{};
    runs_.clear();
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
    const int max_layers = std::clamp(p.layers, 1, 16);
    g.layers = max_layers;
    g.walkable.assign(n * static_cast<std::size_t>(max_layers), 0);
    g.ground.assign(n * static_cast<std::size_t>(max_layers), 0.0f);
    const float cos_slope = repro::cos(radians(p.max_slope_degrees));
    const physics::Physics::Filter static_only = [](EntityId, const world::RigidBody& rb, const world::Collider& col) { return rb.kind == 1 && !col.is_trigger; };
    const float top = p.max.y + 1.0f, bottom = p.min.y - 1.0f;
    int used = 1;
    std::vector<EntityId> passed;   // this column's convex shapes already gone through
    const physics::Physics::Filter below = [&](EntityId id, const world::RigidBody& rb, const world::Collider& col) {
        return rb.kind == 1 && !col.is_trigger && std::find(passed.begin(), passed.end(), id) == passed.end();
    };
    for (int y = 0; y < g.height; ++y) {
        for (int x = 0; x < g.width; ++x) {
            // Down the column: every surface met, the top floor first. A convex shape (box, sphere,
            // capsule) has one top, so the next ray leaves it out; inside a mesh, the next starts
            // just under the surface, and the underside it meets has no headroom above it.
            passed.clear();
            float from = top;
            int layer = 0;
            for (int tries = 0; tries < 4 * max_layers && layer < max_layers && from > bottom; ++tries) {
                const Vec3 c{g.origin.x + (static_cast<float>(x) + 0.5f) * p.cell, from, g.origin.z + (static_cast<float>(y) + 0.5f) * p.cell};
                auto hit = ph.raycast(w, c, Vec3{0, -1, 0}, from - bottom, below);
                if (!hit) break;
                const world::Collider* col = w.try_get<world::Collider>(hit->entity);
                const bool convex = col && col->shape <= 2;
                if (convex) passed.push_back(hit->entity);
                from = convex ? from : hit->point.y - 0.02f;
                if (max_layers == 1) tries = 4 * max_layers;   // one layer: the first surface only, walkable or not, as before layers
                if (hit->normal.y < cos_slope || hit->point.y < p.min.y || hit->point.y > p.max.y) continue;
                // The feet probe clears a step's height: a riser the agent can climb (the next stair)
                // beside the cell does not take the cell away.
                const Vec3 feet{c.x, hit->point.y + std::max(p.max_step, 0.05f) + p.agent_radius, c.z};
                const Vec3 head{c.x, std::max(hit->point.y + p.agent_height - p.agent_radius, feet.y), c.z};
                bool blocked = false;
                for (const Vec3& probe : {feet, head}) {
                    for (EntityId id : ph.overlap_sphere(w, probe, p.agent_radius, static_only)) if (id != hit->entity) blocked = true;
                }
                // Below the top floor, open air above it as well (not the underside of what is over it).
                if (!blocked && layer > 0) {
                    const Vec3 up_from{c.x, hit->point.y + 0.02f, c.z};
                    if (ph.raycast(w, up_from, Vec3{0, 1, 0}, std::max(p.agent_height - 0.02f, 0.05f), static_only)) blocked = true;
                }
                if (blocked) continue;
                const std::size_t i = g.index(x, y, layer);
                g.walkable[i] = 1;
                g.ground[i] = hit->point.y;
                ++layer;
                used = std::max(used, layer);
            }
        }
    }
    // As many layers as some column has floors.
    g.layers = used;
    g.walkable.resize(n * static_cast<std::size_t>(used));
    g.ground.resize(n * static_cast<std::size_t>(used));
    g.source = "colliders";
    g.baked_tick = tick;
    g.agent_radius = p.agent_radius;
    grid_ = std::move(g);
    build_mesh();
    runs_.clear();
    reapply_obstacles();
    return {};
}

Status Nav::bake_tilemap(const world::World& w, assets::AssetStore& assets, EntityId map_entity, const TileBakeParams& p, std::uint64_t tick) {
    const auto* tmc = w.try_get<world::TileMap>(map_entity);
    if (!tmc) return fail("no_tilemap", "entity {} has no TileMap", map_entity);
    if (p.mode != "topdown" && p.mode != "platformer") return fail("bad_args", "mode must be topdown or platformer");
    POCKET_TRY(map, assets.tilemap(tmc->map));
    if (!map->orthogonal() && p.mode == "platformer") return fail("bad_tilemap", "{} is {}: platformer grids need an orthogonal map (rows of floor and gravity down its columns)", tmc->map, map->orientation);
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
    // Cells that are not squares keep the map's geometry, in world units (the map draws with a
    // uniform scale of tile_size per tile width).
    g.layout = map->orthogonal() ? 0 : map->orientation == "isometric" ? 1 : map->orientation == "staggered" ? 2 : 3;
    const float unit = g.cell / static_cast<float>(std::max(map->tile_width, 1));
    g.tile_w = static_cast<float>(map->tile_width) * unit;
    g.tile_h = static_cast<float>(map->tile_height) * unit;
    g.hex_side = static_cast<float>(map->hex_side) * unit;
    g.stagger_y = map->stagger_y;
    g.stagger_odd = map->stagger_odd;
    const bool platformer = p.mode == "platformer";
    g.diagonal = !platformer && p.diagonal;
    const std::size_t n = static_cast<std::size_t>(g.width) * static_cast<std::size_t>(g.height);
    g.walkable.assign(n, 0);
    auto free = [&](int x, int y) { const int s = map->solidity_at(x, y); return (s == 0 || s == 3) && x >= 0 && y >= 0 && x < map->width && y < map->height; };  // a slope cell is a floor to walk
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
    build_mesh();
    runs_.clear();
    reapply_obstacles();
    return {};
}

std::optional<Vec3> Nav::nearest(Vec3 p, float max_radius) const {
    if (!baked()) return std::nullopt;
    const Grid& g = grid_;
    int cx, cy;
    // Cell coordinates of the point, allowed outside the grid for the scan.
    if (g.layout != 0 && g.plane == 1) {
        layout_cell(g, p.x - g.origin.x, g.origin.y - p.y, cx, cy);
    } else {
        cx = static_cast<int>(std::floor((p.x - g.origin.x) / g.cell));
        cy = g.plane == 0 ? static_cast<int>(std::floor((p.z - g.origin.z) / g.cell)) : static_cast<int>(std::floor((g.origin.y - p.y) / g.cell));
    }
    const int r = std::max(0, static_cast<int>(std::ceil(max_radius / g.cell)));
    float best = max_radius * max_radius;
    std::optional<Vec3> out;
    for (int y = cy - r; y <= cy + r; ++y) {
        for (int x = cx - r; x <= cx + r; ++x) {
            for (int l = 0; l < g.layers; ++l) {
                if (!g.walkable_at(x, y, l)) continue;
                const Vec3 c = g.center_at(g.index(x, y, l));
                // Distance in the plane plus, on ground grids, the height difference: a point beside
                // a pillar's foot is nearer to the ground cell next to it than to the pillar's top.
                const float dx = c.x - p.x, dy = g.plane == 0 ? c.z - p.z : c.y - p.y;
                const float dh = g.plane == 0 ? c.y - p.y : 0.0f;
                const float d2 = dx * dx + dy * dy + dh * dh;
                if (d2 <= best + 1e-6f && (!out || d2 < best)) { best = d2; out = c; }
            }
        }
    }
    return out;
}

Result<Path> Nav::grid_path(Vec3 from, Vec3 to, bool smooth) const {
    if (!baked()) return fail("not_baked", "no navigation grid yet: bake one with nav.bake");
    const Grid& g = grid_;
    Path out;
    // The cell a point stands on (its floor, on a stack of floors), else the nearest within two cells.
    auto locate = [&](Vec3 p, const char* what, std::size_t& cell) -> Status {
        int x, y;
        const bool inside = g.cell_of(p, x, y);
        if (inside) {
            if (auto c = g.cell_at(p); c && (g.plane != 0 || std::fabs(g.ground[*c] - p.y) <= g.max_step + 2.5f || g.layers == 1)) {
                cell = *c;
                return {};
            }
        }
        auto near = nearest(p, g.cell * 2.0f);
        if (!near) return inside ? fail("blocked", "the {} is not on walkable ground and nothing walkable is within two cells", what) : fail("outside", "the {} is outside the navigation grid", what);
        out.snapped = true;
        auto c = g.cell_at(*near);
        if (!c) return fail("blocked", "the {} is not on walkable ground", what);
        cell = *c;
        return {};
    };
    std::size_t start = 0, goal = 0;
    POCKET_TRY_VOID(locate(from, "start", start));
    POCKET_TRY_VOID(locate(to, "goal", goal));
    const std::size_t n = g.walkable.size();
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
        for_neighbours(g, cur.cell, [&](std::size_t to_cell, float step) {
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
    std::vector<std::size_t> kept;
    if (smooth && g.links.empty() && cells.size() > 2) {
        // String pulling: keep a corner only when the next cell is out of sight of the last kept one.
        kept.push_back(cells[0]);
        std::size_t anchor = 0;
        for (std::size_t i = 2; i < cells.size(); ++i) {
            if (!line_of_sight(g, cells[anchor], cells[i])) {
                kept.push_back(cells[i - 1]);
                anchor = i - 1;
            }
        }
        kept.push_back(cells.back());
    } else {
        kept = cells;
    }
    for (std::size_t c : kept) out.points.push_back(g.center_at(c));
    for (std::size_t i = 1; i < out.points.size(); ++i) out.length += length(out.points[i] - out.points[i - 1]);
    return out;
}

bool Nav::reachable(Vec3 from, Vec3 to) const {
    if (!baked()) return false;
    if (!grid_.cell_at(from) || !grid_.cell_at(to)) return false;
    auto p = path(from, to, false);
    return p.has_value() && !p->partial && !p->snapped;
}

Json Nav::describe() const {
    Json j;
    j["baked"] = baked();
    j["obstacles"] = obstacles_.size();
    j["blocked"] = grid_.blocked_count();
    j["agents"] = crowd_.agents;
    j["moving"] = crowd_.moving;
    j["arrived"] = crowd_.arrived;
    j["stuck"] = crowd_.stuck;
    j["replans"] = crowd_.replans;
    j["avoiding"] = crowd_.avoiding;
    j["queuing"] = crowd_.queuing;
    j["detours"] = crowd_.detours;
    j["moving_obstacles"] = crowd_.moving_obstacles;
    if (!baked()) return j;
    const Grid& g = grid_;
    j["agent_radius"] = g.agent_radius;
    j["plane"] = g.plane == 0 ? "xz" : "xy";
    j["layout"] = g.layout == 0 ? "square" : g.layout == 1 ? "isometric" : g.layout == 2 ? "staggered" : "hexagonal";
    j["width"] = g.width;
    j["height"] = g.height;
    j["layers"] = g.layers;
    j["cell"] = g.cell;
    j["origin"] = Json{{"x", g.origin.x}, {"y", g.origin.y}, {"z", g.origin.z}};
    j["cells"] = g.walkable.size();
    j["walkable"] = g.walkable_count();
    j["links"] = g.link_count();
    j["diagonal"] = g.diagonal;
    j["max_step"] = g.max_step;
    j["source"] = g.source;
    j["baked_tick"] = g.baked_tick;
    j["mesh"] = Json{{"polygons", mesh_.polys.size()}, {"portals", mesh_.portal_count()}};
    return j;
}

// ---- Obstacles and agents (docs/design/navigation.md, Obstacles and Agents) ----

namespace {

// A point in the grid's plane: (x, z) on ground grids, (x, y) on tile grids.
struct P2 {
    float u = 0, v = 0;
};
P2 p2(int plane, Vec3 p) { return plane == 0 ? P2{p.x, p.z} : P2{p.x, p.y}; }
Vec3 v3(int plane, P2 q, float other) { return plane == 0 ? Vec3{q.u, other, q.v} : Vec3{q.u, q.v, other}; }
P2 sub(P2 a, P2 b) { return {a.u - b.u, a.v - b.v}; }
P2 add(P2 a, P2 b) { return {a.u + b.u, a.v + b.v}; }
P2 mul(P2 a, float s) { return {a.u * s, a.v * s}; }
float dot2(P2 a, P2 b) { return a.u * b.u + a.v * b.v; }
float len2(P2 a) { return std::sqrt(dot2(a, a)); }

constexpr float kHorizon = 1.0f;          // seconds ahead a collision counts against a velocity
constexpr float kLookahead = 0.3f;        // seconds ahead a velocity must keep the agent on walkable ground
constexpr float kWeightDesired = 2.0f;    // deviation from the desired velocity, per unit of top speed
constexpr float kWeightCollision = 4.0f;  // a collision at the horizon's edge counts nothing, one now counts this: more than a right-angle turn
constexpr float kWeightOffGrid = 3.0f;    // leaving walkable ground
constexpr float kFormationGain = 4.0f;    // per second: how fast a follower closes on its slot beyond matching the leader
constexpr float kWeightAcross = 4.0f;     // at queue 1, sideways deviation costs this much more than slowing while someone ahead goes the agent's way
constexpr int kMaxNeighbours = 8;

// Seconds until two discs touch, given the other's position and velocity relative to this one;
// nullopt when they never do, 0 when they already overlap.
std::optional<float> time_to_collision(P2 rel, P2 rel_vel, float radius) {
    const float c = dot2(rel, rel) - radius * radius;
    if (c <= 0) return 0.0f;
    const float a = dot2(rel_vel, rel_vel);
    if (a < 1e-8f) return std::nullopt;
    const float b = 2.0f * dot2(rel, rel_vel);
    const float disc = b * b - 4.0f * a * c;
    if (disc < 0) return std::nullopt;
    const float t = (-b - std::sqrt(disc)) / (2.0f * a);
    if (t < 0) return std::nullopt;
    return t;
}

bool on_ground(const Grid& g, Vec3 p) {
    return g.cell_at(p).has_value();
}

Json json_of_vec(Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; }

}  // namespace

void Nav::set_obstacles(std::vector<Obstacle> obstacles) {
    std::sort(obstacles.begin(), obstacles.end(), [](const Obstacle& a, const Obstacle& b) { return a.id < b.id; });
    bool same = obstacles.size() == obstacles_.size();
    for (std::size_t i = 0; same && i < obstacles.size(); ++i) {
        const Obstacle &a = obstacles[i], &b = obstacles_[i];
        same = a.id == b.id && a.position.x == b.position.x && a.position.y == b.position.y && a.position.z == b.position.z && a.radius == b.radius;
    }
    const bool applied = obstacles_.empty() ? grid_.blocked.empty() : grid_.blocked.size() == grid_.walkable.size();
    if (same && applied) return;
    obstacles_ = std::move(obstacles);
    reapply_obstacles();
}

void Nav::reapply_obstacles() {
    Grid& g = grid_;
    g.blocked.clear();
    if (!baked() || obstacles_.empty()) return;
    g.blocked.assign(g.walkable.size(), 0);
    for (const Obstacle& o : obstacles_) {
        const float r = std::max(o.radius, 0.0f) + g.agent_radius;
        if (r <= 0) continue;
        const P2 c = p2(g.plane, o.position);
        // The columns and rows the disc can touch, clipped to the grid.
        const int x0 = static_cast<int>(std::floor((c.u - r - g.origin.x) / g.cell)), x1 = static_cast<int>(std::floor((c.u + r - g.origin.x) / g.cell));
        int y0, y1;
        if (g.plane == 0) {
            y0 = static_cast<int>(std::floor((c.v - r - g.origin.z) / g.cell));
            y1 = static_cast<int>(std::floor((c.v + r - g.origin.z) / g.cell));
        } else {
            y0 = static_cast<int>(std::floor((g.origin.y - (c.v + r)) / g.cell));
            y1 = static_cast<int>(std::floor((g.origin.y - (c.v - r)) / g.cell));
        }
        // On a stack of floors, the floor it stands on (within a body's height of its position).
        for (int y = std::max(y0, 0); y <= std::min(y1, g.height - 1); ++y) {
            for (int x = std::max(x0, 0); x <= std::min(x1, g.width - 1); ++x) {
                const P2 d = sub(p2(g.plane, g.center_of(x, y)), c);
                if (dot2(d, d) > r * r) continue;
                for (int l = 0; l < g.layers; ++l) {
                    const std::size_t i = g.index(x, y, l);
                    if (g.layers == 1 || (g.ground[i] <= o.position.y + g.max_step + 0.01f && g.ground[i] >= o.position.y - 2.5f)) g.blocked[i] = 1;
                }
            }
        }
    }
}

void Nav::step(world::World& w, float dt) {
    const std::uint64_t tick = static_cast<std::uint64_t>(std::max<std::int64_t>(w.tick_index(), 0));
    crowd_ = CrowdStats{};
    // The obstacles of this tick: every enabled NavObstacle at its world position, with the velocity
    // the agents should expect of it: its Velocity component when it has one, else where it went
    // since last tick (a cart moved by a script or a tween), so a moving obstacle is met where it
    // will be, not where it is.
    std::vector<Obstacle> obstacles;
    std::map<world::EntityId, Vec3> was;
    crowd_.moving_obstacles = 0;
    w.ecs().each([&](flecs::entity e, const world::NavObstacle& o, const world::Transform& t) {
        Vec3 p = t.position;
        if (const auto* wt = w.try_get<world::WorldTransform>(e.id())) p = wt->position;
        was[e.id()] = p;
        if (!o.enabled) return;
        Vec3 v;
        if (const auto* vel = w.try_get<world::Velocity>(e.id())) v = vel->linear;
        else if (const auto it = obstacle_was_.find(e.id()); it != obstacle_was_.end() && dt > 0) v = (p - it->second) * (1.0f / dt);
        if (length(v) > 1e-4f) crowd_.moving_obstacles++;
        else v = Vec3{};
        obstacles.push_back({e.id(), p, std::max(o.radius, 0.0f), v});
    });
    obstacle_was_ = std::move(was);
    set_obstacles(std::move(obstacles));
    crowd_.obstacles = static_cast<int>(obstacles_.size());
    crowd_.blocked = static_cast<int>(grid_.blocked_count());

    struct Item {
        world::EntityId id;
        world::NavAgent agent;
        Vec3 pos;
        P2 vel_prev;  // the velocity chosen last tick: what the others avoid, whatever the entity order
    };
    const int plane = baked() ? grid_.plane : 0;
    std::vector<Item> items;
    w.ecs().each([&](flecs::entity e, const world::NavAgent& a, const world::Transform& t) { items.push_back({e.id(), a, t.position, p2(plane, a.velocity)}); });
    std::sort(items.begin(), items.end(), [](const Item& a, const Item& b) { return a.id < b.id; });
    crowd_.agents = static_cast<int>(items.size());
    for (auto it = runs_.begin(); it != runs_.end();) {
        const world::EntityId id = it->first;
        const bool alive = std::any_of(items.begin(), items.end(), [&](const Item& i) { return i.id == id; });
        it = alive ? std::next(it) : runs_.erase(it);
    }
    if (items.empty() || dt <= 0) return;
    const float cell = baked() ? grid_.cell : 0.5f;
    // Formation leaders: their heading is the way they last moved, their speed the move over the tick.
    {
        std::set<world::EntityId> led;
        for (const Item& it : items) {
            if (it.agent.mode != 3 || it.agent.target == 0 || !w.alive(it.agent.target)) continue;
            Vec3 lp;
            if (const auto* wt = w.try_get<world::WorldTransform>(it.agent.target)) lp = wt->position;
            else if (const auto* t = w.try_get<world::Transform>(it.agent.target)) lp = t->position;
            else continue;
            led.insert(it.agent.target);
            Lead& L = leads_[it.agent.target];
            if (L.tick == tick && L.seen) continue;   // several followers, one leader: once a tick
            if (L.seen) {
                const P2 d = sub(p2(plane, lp), p2(plane, L.last));
                const float dl = len2(d);
                if (dl > 1e-5f) {
                    L.hu = d.u / dl;
                    L.hv = d.v / dl;
                    L.speed = dl / dt;
                } else {
                    L.speed = 0;
                }
            }
            L.last = lp;
            L.seen = true;
            L.tick = tick;
        }
        for (auto it = leads_.begin(); it != leads_.end();) it = led.contains(it->first) ? std::next(it) : leads_.erase(it);
    }
    const float other_axis = 0.0f;
    (void)other_axis;

    struct Plan {
        P2 desired, chosen;
        int state = 0;
        Vec3 corner;
        float distance = 0;
        int neighbours = 0;
        bool queued = false;
        bool has_goal = false;
        Vec3 goal;
    };
    std::vector<Plan> plans(items.size());
    // 1. Where each agent wants to go: the next corner of its path, replanned when due.
    for (std::size_t i = 0; i < items.size(); ++i) {
        const Item& it = items[i];
        const world::NavAgent& a = it.agent;
        Plan& pl = plans[i];
        pl.corner = it.pos;
        if (a.mode == 0) {
            runs_.erase(it.id);
            continue;
        }
        std::optional<Vec3> goal;
        if (a.mode == 1) {
            goal = a.goal;
        } else if (a.mode == 2 && a.target != 0 && w.alive(a.target)) {
            if (const auto* wt = w.try_get<world::WorldTransform>(a.target)) goal = wt->position;
            else if (const auto* t = w.try_get<world::Transform>(a.target)) goal = t->position;
        } else if (a.mode == 3) {
            if (auto lead = leads_.find(a.target); lead != leads_.end()) {
                const Lead& L = lead->second;
                const P2 fwd{L.hu, L.hv}, right{-L.hv, L.hu};
                const float side = plane == 0 ? a.offset.z : a.offset.y;
                const P2 slot = add(p2(plane, L.last), add(mul(fwd, a.offset.x), mul(right, side)));
                goal = v3(plane, slot, plane == 0 ? it.pos.y : it.pos.z);
            }
        }
        if (!goal) {
            pl.state = 3;
            runs_.erase(it.id);
            continue;
        }
        pl.has_goal = true;
        pl.goal = *goal;
        pl.corner = *goal;
        const P2 p = p2(plane, it.pos), gq = p2(plane, *goal);
        const float dgoal = len2(sub(gq, p));
        pl.distance = dgoal;
        if (a.mode == 3) {
            // A follower with its slot in sight (a straight line over walkable ground, or no grid)
            // steers to it, no path: the leader's velocity plus a pull toward the slot, capped at
            // its speed; arrived only once the leader stands and it is in place. A slot out of
            // sight, behind a wall or a pillar, is walked to along a path like a goal, below.
            bool in_sight = true;
            if (baked()) {
                const auto ac = grid_.cell_at(it.pos), bc = grid_.cell_at(*goal);
                in_sight = ac && bc && line_of_sight(grid_, *ac, *bc);
            }
            if (in_sight) {
                const Lead& L = leads_[a.target];
                runs_.erase(it.id);
                if (L.speed < 1e-3f && dgoal <= a.arrive) {
                    pl.state = 2;
                    continue;
                }
                pl.state = 1;
                P2 want = add(mul(P2{L.hu, L.hv}, L.speed), mul(sub(gq, p), kFormationGain));
                const float wl = len2(want), top = std::max(a.speed, 0.0f);
                if (wl > top && wl > 1e-6f) want = mul(want, top / wl);
                pl.desired = want;
                pl.chosen = want;
                continue;
            }
            crowd_.detours++;
        }
        // On a stack of floors the goal right over (or under) the agent is not reached until the
        // agent stands on its floor.
        bool same_floor = true;
        if (plane == 0 && grid_.layers > 1) {
            const auto ac = grid_.cell_at(it.pos), bc = grid_.cell_at(*goal);
            same_floor = !ac || !bc || std::fabs(grid_.ground[*ac] - grid_.ground[*bc]) <= grid_.max_step + 1e-4f;
        }
        if (dgoal <= a.arrive && same_floor) {
            pl.state = 2;
            runs_.erase(it.id);
            continue;
        }
        pl.state = 1;
        P2 corner = gq;
        if (baked()) {
            AgentRun& run = runs_[it.id];
            const std::uint64_t every = static_cast<std::uint64_t>(std::max(a.replan, 0));
            bool replan = !run.planned || tick >= run.planned_tick + every || len2(sub(p2(plane, run.goal), gq)) > cell * 0.5f || (plane == 0 && std::fabs(run.goal.y - goal->y) > grid_.max_step);
            if (!replan && run.next < run.path.size() && !on_ground(grid_, run.path[run.next])) replan = true;  // the corner got blocked
            if (replan) {
                auto r = path(it.pos, *goal, true);
                crowd_.replans++;
                run.planned = true;
                run.planned_tick = tick;
                run.goal = *goal;
                if (r) {
                    run.path = r->points;
                    run.partial = r->partial;
                    run.next = run.path.size() > 1 && !r->snapped ? 1 : 0;   // off walkable ground: back onto it first
                } else {
                    run.path.clear();  // off the grid: straight at the goal
                    run.partial = false;
                }
            }
            if (run.path.size() >= 2) {
                // On to the next corner near this one, once the way to the one after is in sight
                // (or the corner is reached): cutting in early would leave walkable ground.
                const auto here = grid_.cell_at(it.pos);
                auto sees = [&](const Vec3& q) {
                    const auto c = grid_.cell_at(q);
                    return here && c && line_of_sight(grid_, *here, *c);
                };
                while (run.next + 1 < run.path.size()) {
                    const float dc = len2(sub(p2(plane, run.path[run.next]), p));
                    if (dc >= cell * 0.6f || (dc >= cell * 0.1f && !sees(run.path[run.next + 1]))) break;
                    run.next++;
                }
                const bool last = run.next + 1 >= run.path.size();
                if (last && !run.partial) {
                    corner = gq;  // the goal itself, not its cell's center
                } else {
                    corner = p2(plane, run.path[run.next]);
                    float remaining = len2(sub(corner, p));
                    for (std::size_t k = run.next + 1; k < run.path.size(); ++k) remaining += len2(sub(p2(plane, run.path[k]), p2(plane, run.path[k - 1])));
                    if (!run.partial) remaining += len2(sub(gq, p2(plane, run.path.back())));
                    pl.distance = remaining;
                    // The path stops short of the goal and the agent stands at its end: stuck.
                    if (run.partial && last && len2(sub(corner, p)) <= a.arrive) pl.state = 3;
                }
            } else if (run.partial) {
                // Already at the closest cell the grid offers: nowhere further to walk.
                corner = run.path.empty() ? p : p2(plane, run.path.front());
                pl.state = 3;
            }
        }
        pl.corner = v3(plane, corner, plane == 0 ? it.pos.y : it.pos.z);
        if (pl.state != 1) continue;
        const P2 d = sub(corner, p);
        const float len = len2(d);
        const float speed = same_floor ? std::min(std::max(a.speed, 0.0f), std::max(dgoal - a.arrive * 0.5f, 0.0f) / dt) : std::max(a.speed, 0.0f);
        if (len > 1e-6f && speed > 0) pl.desired = mul(d, speed / len);
        pl.chosen = pl.desired;
    }
    // 2. The velocity each moving agent takes: the desired one, or the candidate that best keeps
    // clear of the others and the obstacles over the next second while staying on walkable ground.
    for (std::size_t i = 0; i < items.size(); ++i) {
        Plan& pl = plans[i];
        const world::NavAgent& a = items[i].agent;
        if (pl.state != 1 || a.avoidance <= 0) continue;
        const P2 p = p2(plane, items[i].pos);
        const float top = std::max(a.speed, 1e-3f);
        struct Near {
            P2 rel, vel;
            float radius, dist;
            bool agent;
        };
        std::vector<Near> near;
        const float reach = a.radius + top * kHorizon + cell;
        for (std::size_t j = 0; j < items.size(); ++j) {
            if (j == i || items[j].agent.mode == 0) continue;
            if (items[j].agent.priority < a.priority) continue;   // it gets out of this one's way
            if (items[j].agent.mode == 3 && items[j].agent.target == items[i].id) continue;   // its own followers keep their slots; a leader that fled them would stall
            const P2 rel = sub(p2(plane, items[j].pos), p);
            const float dist = len2(rel);
            if (dist > reach + items[j].agent.radius) continue;
            near.push_back({rel, items[j].vel_prev, a.radius + items[j].agent.radius, dist, true});
        }
        for (const Obstacle& o : obstacles_) {
            const P2 rel = sub(p2(plane, o.position), p);
            const P2 ov = p2(plane, o.velocity);
            const float dist = len2(rel);
            if (dist > reach + o.radius + len2(ov) * kHorizon) continue;   // a fast one arrives from further away
            near.push_back({rel, ov, a.radius + o.radius, dist, false});
        }
        if (near.empty()) continue;
        std::sort(near.begin(), near.end(), [](const Near& x, const Near& y) { return x.dist < y.dist; });
        if (near.size() > kMaxNeighbours) near.resize(kMaxNeighbours);
        pl.neighbours = static_cast<int>(near.size());
        // Candidates: the desired velocity, for a queuing agent the same direction slowed, then turns
        // in steps of 22.5 degrees either way at full and half speed, then standing still. Ties go
        // to the earlier candidate, so every agent turns the same way first and two that meet
        // head-on pass on the same side.
        std::vector<P2> candidates;
        candidates.push_back(pl.desired);
        const float dlen = len2(pl.desired);
        const float queue = std::clamp(a.queue, 0.0f, 1.0f);
        const P2 ddir = dlen > 1e-6f ? mul(pl.desired, 1.0f / dlen) : P2{};
        // Someone ahead going this agent's way, or standing there, whom the desired velocity would
        // run into: the queue applies to them, never to crossing or oncoming traffic.
        bool line = false;
        if (queue > 0 && dlen > 1e-6f) {
            for (const Near& n : near) {
                if (!n.agent || dot2(n.rel, ddir) <= 0) continue;
                const float nl = len2(n.vel);
                if (nl > 0.1f * top && dot2(n.vel, ddir) < 0.5f * nl) continue;
                if (const auto t = time_to_collision(n.rel, sub(n.vel, pl.desired), n.radius); t && *t < kHorizon) {
                    line = true;
                    break;
                }
            }
        }
        if (dlen > 1e-6f && queue > 0) {
            for (float f : {0.75f, 0.5f, 0.25f}) candidates.push_back(mul(pl.desired, f));
        }
        if (dlen > 1e-6f) {
            for (int k = 1; k <= 8; ++k) {
                for (int side : {1, -1}) {
                    const float ang = static_cast<float>(side * k) * (kPi / 8.0f);
                    const float c = repro::cos(ang), s = repro::sin(ang);
                    const P2 dir{(pl.desired.u * c - pl.desired.v * s) / dlen, (pl.desired.u * s + pl.desired.v * c) / dlen};
                    candidates.push_back(mul(dir, dlen));
                    candidates.push_back(mul(dir, dlen * 0.5f));
                }
            }
        }
        candidates.push_back(P2{});
        float best = std::numeric_limits<float>::infinity();
        P2 chosen = pl.desired;
        for (std::size_t c = 0; c < candidates.size(); ++c) {
            const P2 v = candidates[c];
            float deviation = len2(sub(v, pl.desired));
            if (line) {
                // Behind someone going the same way, slowing is the cheap deviation and stepping aside the dear one.
                const float along = dot2(v, ddir);
                const float across = len2(sub(v, mul(ddir, along)));
                deviation = std::fabs(along - dlen) + across * (1.0f + kWeightAcross * queue);
            }
            float score = kWeightDesired * deviation / top + 0.001f * static_cast<float>(c);
            for (const Near& n : near) {
                const auto t = time_to_collision(n.rel, sub(n.vel, v), n.radius);
                if (!t) continue;
                if (*t <= 0) {
                    // Already overlapping: what counts is this agent moving away, whatever the other does.
                    const float toward = n.dist > 1e-6f ? dot2(v, n.rel) / (n.dist * top) : 0.0f;
                    score += a.avoidance * kWeightCollision * (1.0f + std::clamp(toward, -1.0f, 1.0f));
                } else if (*t < kHorizon) {
                    score += a.avoidance * kWeightCollision * (kHorizon - *t) / kHorizon;
                }
            }
            if (baked()) {
                for (float f : {0.5f, 1.0f}) {
                    const Vec3 ahead = v3(plane, add(p, mul(v, kLookahead * f)), plane == 0 ? items[i].pos.y : items[i].pos.z);
                    if (!on_ground(grid_, ahead)) {
                        score += kWeightOffGrid;
                        break;
                    }
                }
            }
            if (score < best) {
                best = score;
                chosen = v;
            }
        }
        pl.chosen = chosen;
        if (len2(sub(chosen, pl.desired)) > 1e-4f) crowd_.avoiding++;
        pl.queued = line && dot2(chosen, ddir) < dlen - 1e-4f;
        if (pl.queued) crowd_.queuing++;
    }
    // 3. Overlapping agents are pushed apart, half each, from where they stood at the start of the tick.
    std::vector<P2> push(items.size());
    for (std::size_t i = 0; i < items.size(); ++i) {
        if (items[i].agent.mode == 0) continue;
        for (std::size_t j = i + 1; j < items.size(); ++j) {
            if (items[j].agent.mode == 0) continue;
            const P2 rel = sub(p2(plane, items[j].pos), p2(plane, items[i].pos));
            const float dist = len2(rel), radius = items[i].agent.radius + items[j].agent.radius;
            if (radius <= 0 || dist >= radius) continue;
            const P2 n = dist > 1e-6f ? mul(rel, 1.0f / dist) : P2{1, 0};
            const float half = std::min((radius - dist) * 0.5f, radius * 0.25f);
            push[i] = sub(push[i], mul(n, half));
            push[j] = add(push[j], mul(n, half));
        }
    }
    // 4. Write the agents back and move them.
    for (std::size_t i = 0; i < items.size(); ++i) {
        const Item& it = items[i];
        const Plan& pl = plans[i];
        world::NavAgent a = it.agent;
        const int old_state = a.state;
        if (a.mode == 0) {
            if (a.state != 0 || a.velocity.x != 0 || a.velocity.y != 0 || a.velocity.z != 0 || a.neighbours != 0 || a.distance != 0 || a.queued) {
                a.state = 0;
                a.velocity = {};
                a.neighbours = 0;
                a.queued = false;
                a.distance = 0;
                a.corner = it.pos;
                w.set_typed<world::NavAgent>(it.id, a);
            }
            continue;
        }
        const bool pushed = push[i].u != 0 || push[i].v != 0;
        const P2 vel = add(pl.state == 1 ? pl.chosen : P2{}, mul(push[i], 1.0f / dt));
        a.state = pl.state;
        a.velocity = v3(plane, vel, 0.0f);
        a.corner = pl.corner;
        a.distance = pl.state == 1 ? pl.distance : 0.0f;
        a.neighbours = pl.neighbours;
        a.queued = pl.queued;
        if (pl.state == 1) crowd_.moving++;
        else if (pl.state == 2) crowd_.arrived++;
        else if (pl.state == 3) crowd_.stuck++;
        // Through the Velocity component when the entity has one (the motion system or the
        // physics integrates it), else the transform itself.
        if (const auto* v = w.try_get<world::Velocity>(it.id)) {
            world::Velocity nv = *v;
            if (plane == 0) {
                nv.linear.x = vel.u;
                nv.linear.z = vel.v;
            } else {
                nv.linear.x = vel.u;
                nv.linear.y = vel.v;
            }
            w.set_typed<world::Velocity>(it.id, nv);
        } else if (pl.state == 1 || pushed) {
            world::Transform t = *w.try_get<world::Transform>(it.id);
            const Vec3 next = t.position + v3(plane, mul(vel, dt), 0.0f);
            // On ground, the agent climbs and descends with the floor it walks onto, keeping its
            // height over it (a ramp, stairs, or onto the deck above the road it came from).
            if (plane == 0 && !grid_.ground.empty()) {
                int nx, ny;
                if (auto from = grid_.cell_at(t.position); from && grid_.cell_of(next, nx, ny)) {
                    // A step from its floor, else (a corner cut onto a taller step) the floor
                    // nearest in height.
                    std::optional<std::size_t> to = grid_.step_to(*from, nx, ny);
                    const bool stepped = to.has_value();
                    float best = std::numeric_limits<float>::infinity();
                    for (int l = 0; !stepped && l < grid_.layers; ++l) {
                        const std::size_t c = grid_.index(nx, ny, l);
                        const float d = std::fabs(grid_.ground[c] - grid_.ground[*from]);
                        if (grid_.walkable_cell(c) && d < best) {
                            best = d;
                            to = c;
                        }
                    }
                    if (to) t.position.y += grid_.ground[*to] - grid_.ground[*from];
                }
            }
            t.position.x = next.x;
            t.position.z = next.z;
            if (plane != 0) t.position.y = next.y;
            w.set_typed<world::Transform>(it.id, t);
        }
        // Facing where it walks: the yaw turned toward the velocity, at most turn_speed a second.
        if (a.face && plane == 0 && len2(vel) > 0.05f) {
            world::Transform t = *w.try_get<world::Transform>(it.id);
            const Vec3 f = t.rotation.rotate({0, 0, -1});
            const float now = repro::atan2(-f.x, -f.z), want = repro::atan2(-vel.u, -vel.v);
            float turn = want - now;
            while (turn > kPi) turn -= 2 * kPi;
            while (turn < -kPi) turn += 2 * kPi;
            const float most = std::max(a.turn_speed, 0.0f) * kPi / 180.0f * dt;
            const float yaw = now + std::clamp(turn, -most, most);
            t.rotation = Quat{0, repro::sin(yaw * 0.5f), 0, repro::cos(yaw * 0.5f)};
            w.set_typed<world::Transform>(it.id, t);
        }
        w.set_typed<world::NavAgent>(it.id, a);
        if (a.state == 2 && old_state != 2) w.events().emit(w.tick_index(), "nav.arrived", it.id, Json{{"path", w.path(it.id)}, {"goal", json_of_vec(pl.goal)}}, 0, "nav");
        if (a.state == 3 && old_state != 3) w.events().emit(w.tick_index(), "nav.stuck", it.id, Json{{"path", w.path(it.id)}, {"reason", pl.has_goal ? "no path" : "no target"}}, 0, "nav");
    }
}

Json Nav::agents(world::World& w) const {
    std::vector<std::pair<world::EntityId, world::NavAgent>> list;
    w.ecs().each([&](flecs::entity e, const world::NavAgent& a) { list.emplace_back(e.id(), a); });
    std::sort(list.begin(), list.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
    Json out = Json::array();
    for (const auto& [id, a] : list) {
        Json j;
        j["id"] = id;
        j["path"] = w.path(id);
        j["mode"] = a.mode;
        j["state"] = a.state;
        j["speed"] = a.speed;
        j["radius"] = a.radius;
        j["velocity"] = json_of_vec(a.velocity);
        j["corner"] = json_of_vec(a.corner);
        j["distance"] = a.distance;
        j["neighbours"] = a.neighbours;
        j["offset"] = json_of_vec(a.offset);
        j["queue"] = a.queue;
        j["priority"] = a.priority;
        j["queued"] = a.queued;
        if (auto it = runs_.find(id); it != runs_.end() && !it->second.path.empty()) {
            j["corners"] = it->second.path.size() > it->second.next ? it->second.path.size() - it->second.next : 0;
            j["partial"] = it->second.partial;
            j["planned_tick"] = it->second.planned_tick;
        }
        out.push_back(j);
    }
    return out;
}


// ---- Navmesh (docs/design/navigation.md, Navmesh) ----

namespace {

// The signed double area of a triangle in the plane: positive when c lies to the right of a->b
// (the funnel below keeps left and right by this sign).
float triarea2(P2 a, P2 b, P2 c) { return (c.u - a.u) * (b.v - a.v) - (b.u - a.u) * (c.v - a.v); }
bool same2(P2 a, P2 b) { return std::fabs(a.u - b.u) < 1e-5f && std::fabs(a.v - b.v) < 1e-5f; }
// The plane coordinate of a column's left edge, and of a row's near edge (rows run +Z on ground
// grids and -Y on tile grids).
float col_edge(const Grid& g, int c) { return g.origin.x + static_cast<float>(c) * g.cell; }
float row_edge(const Grid& g, int r) { return g.plane == 0 ? g.origin.z + static_cast<float>(r) * g.cell : g.origin.y - static_cast<float>(r) * g.cell; }
P2 poly_center(const Grid& g, const NavMesh::Poly& p) {
    return {col_edge(g, p.x0) + (static_cast<float>(p.x1 - p.x0 + 1) * 0.5f) * g.cell, g.plane == 0 ? row_edge(g, p.y0) + (static_cast<float>(p.y1 - p.y0 + 1) * 0.5f) * g.cell : row_edge(g, p.y0) - (static_cast<float>(p.y1 - p.y0 + 1) * 0.5f) * g.cell};
}
// The cell under a plane point, clamped into the grid: on a stack of floors, the walkable one
// nearest the height `near_y` (the way's height where it comes from), else layer 0.
std::size_t cell_under(const Grid& g, P2 q, float near_y = 0) {
    int x = static_cast<int>(std::floor((q.u - g.origin.x) / g.cell));
    int y = g.plane == 0 ? static_cast<int>(std::floor((q.v - g.origin.z) / g.cell)) : static_cast<int>(std::floor((g.origin.y - q.v) / g.cell));
    x = std::clamp(x, 0, g.width - 1);
    y = std::clamp(y, 0, g.height - 1);
    std::size_t best = g.index(x, y);
    if (g.layers > 1) {
        float best_d = std::numeric_limits<float>::infinity();
        for (int l = 0; l < g.layers; ++l) {
            const std::size_t i = g.index(x, y, l);
            if (g.walkable[i] == 0) continue;
            const float d = std::fabs(g.ground[i] - near_y);
            if (d < best_d) { best_d = d; best = i; }
        }
    }
    return best;
}
// A plane point lifted into the world: the ground under it on ground grids (the floor nearest
// near_y on a stack of floors), the map's depth otherwise.
Vec3 lift(const Grid& g, P2 q, float near_y = 0) {
    if (g.plane == 0) return {q.u, g.ground.empty() ? g.origin.y : g.ground[cell_under(g, q, near_y)], q.v};
    return {q.u, q.v, g.depth};
}

// The simple stupid funnel: the shortest way through a sequence of portals (left, right) from the
// start to the goal, corners only where the funnel closes.
std::vector<P2> funnel(P2 start, P2 goal, const std::vector<std::pair<P2, P2>>& portals) {
    std::vector<std::pair<P2, P2>> ports;
    ports.reserve(portals.size() + 2);
    ports.emplace_back(start, start);
    for (const auto& p : portals) ports.push_back(p);
    ports.emplace_back(goal, goal);
    std::vector<P2> out{start};
    P2 apex = start, left = start, right = start;
    std::size_t apex_i = 0, left_i = 0, right_i = 0;
    for (std::size_t i = 1; i < ports.size(); ++i) {
        const P2 pl = ports[i].first, pr = ports[i].second;
        if (triarea2(apex, right, pr) <= 0.0f) {
            if (same2(apex, right) || triarea2(apex, left, pr) > 0.0f) {
                right = pr;
                right_i = i;
            } else {
                out.push_back(left);
                apex = left;
                apex_i = left_i;
                left = right = apex;
                left_i = right_i = apex_i;
                i = apex_i;
                continue;
            }
        }
        if (triarea2(apex, left, pl) >= 0.0f) {
            if (same2(apex, left) || triarea2(apex, right, pl) < 0.0f) {
                left = pl;
                left_i = i;
            } else {
                out.push_back(right);
                apex = right;
                apex_i = right_i;
                left = right = apex;
                left_i = right_i = apex_i;
                i = apex_i;
                continue;
            }
        }
    }
    if (!same2(out.back(), goal)) out.push_back(goal);
    return out;
}

}  // namespace

std::size_t NavMesh::portal_count() const {
    std::size_t n = 0;
    for (const auto& p : portals) n += p.size();
    return n;
}

// Rectangles of walkable cells, greedy row by row (as wide as the row allows, then as tall as
// every column allows), each cell within a step of its neighbours in the rectangle; then a portal
// along every run of a rectangle's side that touches another rectangle within a step. Platformer
// grids (jump links) get no mesh.
void Nav::build_mesh() {
    const Grid& g = grid_;
    NavMesh m;
    if (g.width <= 0 || g.height <= 0 || !g.links.empty() || g.layout != 0) { mesh_ = std::move(m); return; }   // rectangles cover square lattices only
    m.cell_poly.assign(g.walkable.size(), -1);
    for (int l = 0; l < g.layers; ++l) {
    auto ok = [&](int x, int y) { return g.inside(x, y) && g.walkable[g.index(x, y, l)] != 0; };
    for (int y = 0; y < g.height; ++y) {
        for (int x = 0; x < g.width; ++x) {
            if (!ok(x, y) || m.cell_poly[g.index(x, y, l)] >= 0) continue;
            int x1 = x;
            while (x1 + 1 < g.width && ok(x1 + 1, y) && m.cell_poly[g.index(x1 + 1, y, l)] < 0 && step_ok(g, g.index(x1, y, l), g.index(x1 + 1, y, l))) ++x1;
            int y1 = y;
            while (y1 + 1 < g.height) {
                bool fits = true;
                for (int xx = x; xx <= x1 && fits; ++xx) {
                    const std::size_t below = g.index(xx, y1 + 1, l);
                    if (!ok(xx, y1 + 1) || m.cell_poly[below] >= 0 || !step_ok(g, g.index(xx, y1, l), below)) fits = false;
                    else if (xx > x && !step_ok(g, g.index(xx - 1, y1 + 1, l), below)) fits = false;
                }
                if (!fits) break;
                ++y1;
            }
            const int id = static_cast<int>(m.polys.size());
            m.polys.push_back({x, y, x1, y1, l});
            for (int yy = y; yy <= y1; ++yy) for (int xx = x; xx <= x1; ++xx) m.cell_poly[g.index(xx, yy, l)] = id;
        }
    }
    }
    m.portals.assign(m.polys.size(), {});
    for (int id = 0; id < static_cast<int>(m.polys.size()); ++id) {
        const NavMesh::Poly& P = m.polys[static_cast<std::size_t>(id)];
        // Each side: the cells along it, their neighbours across, the runs of one neighbour polygon.
        auto side = [&](int dx, int dy) {
            const int n = dx != 0 ? P.y1 - P.y0 + 1 : P.x1 - P.x0 + 1;
            auto mine = [&](int t, int& cx, int& cy) {
                if (dx != 0) { cx = dx > 0 ? P.x1 : P.x0; cy = P.y0 + t; }
                else { cx = P.x0 + t; cy = dy > 0 ? P.y1 : P.y0; }
            };
            int run_start = -1, run_poly = -1;
            auto flush = [&](int t_end) {
                if (run_start < 0) return;
                NavMesh::Portal pt;
                pt.to = run_poly;
                if (dx != 0) {
                    const float u = col_edge(g, dx > 0 ? P.x1 + 1 : P.x0);
                    pt.au = u; pt.av = row_edge(g, P.y0 + run_start);
                    pt.bu = u; pt.bv = row_edge(g, P.y0 + t_end + 1);
                } else {
                    const float v = row_edge(g, dy > 0 ? P.y1 + 1 : P.y0);
                    pt.au = col_edge(g, P.x0 + run_start); pt.av = v;
                    pt.bu = col_edge(g, P.x0 + t_end + 1); pt.bv = v;
                }
                m.portals[static_cast<std::size_t>(id)].push_back(pt);
                run_start = -1;
                run_poly = -1;
            };
            for (int t = 0; t < n; ++t) {
                int cx, cy;
                mine(t, cx, cy);
                const int nx = cx + dx, ny = cy + dy;
                int other = -1;
                // Across the side, the floor a step away, whichever layer it is on.
                if (const auto across = g.step_to(g.index(cx, cy, P.layer), nx, ny)) other = m.cell_poly[*across];
                if (other != run_poly) { flush(t - 1); if (other >= 0) { run_start = t; run_poly = other; } }
            }
            flush(n - 1);
        };
        side(1, 0);
        side(-1, 0);
        side(0, 1);
        side(0, -1);
    }
    mesh_ = std::move(m);
}

Json Nav::mesh_json() const {
    Json j;
    j["polygons"] = Json::array();
    const Grid& g = grid_;
    for (std::size_t i = 0; i < mesh_.polys.size(); ++i) {
        const NavMesh::Poly& p = mesh_.polys[i];
        Json neighbours = Json::array();
        for (const NavMesh::Portal& pt : mesh_.portals[i]) neighbours.push_back(pt.to);
        const float h = g.plane == 0 && !g.ground.empty() ? g.ground[g.index(p.x0, p.y0, p.layer)] : 0.0f;
        const Vec3 lo = lift(g, {col_edge(g, p.x0), g.plane == 0 ? row_edge(g, p.y0) : row_edge(g, p.y1 + 1)}, h);
        const Vec3 hi = lift(g, {col_edge(g, p.x1 + 1), g.plane == 0 ? row_edge(g, p.y1 + 1) : row_edge(g, p.y0)}, h);
        j["polygons"].push_back(Json{{"id", i}, {"cells", (p.x1 - p.x0 + 1) * (p.y1 - p.y0 + 1)}, {"min", {{"x", lo.x}, {"y", lo.y}, {"z", lo.z}}}, {"max", {{"x", hi.x}, {"y", hi.y}, {"z", hi.z}}}, {"neighbours", neighbours}});
    }
    j["count"] = mesh_.polys.size();
    j["portals"] = mesh_.portal_count();
    return j;
}

std::optional<Path> Nav::mesh_path(Vec3 from, Vec3 to) const {
    const Grid& g = grid_;
    const NavMesh& m = mesh_;
    if (m.empty()) return std::nullopt;
    Path out;
    out.mesh = true;
    auto locate = [&](Vec3 p, P2& q, std::size_t& cell) -> bool {
        if (auto c = g.cell_at(p)) {
            q = p2(g.plane, p);
            cell = *c;
            return true;
        }
        auto near = nearest(p, g.cell * 2.0f);
        if (!near) return false;
        auto c = g.cell_at(*near);
        if (!c) return false;
        out.snapped = true;
        q = p2(g.plane, *near);
        cell = *c;
        return true;
    };
    P2 start, goal;
    std::size_t start_cell, goal_cell;
    if (!locate(from, start, start_cell) || !locate(to, goal, goal_cell)) return std::nullopt;
    const int sp = m.cell_poly[start_cell], gp = m.cell_poly[goal_cell];
    if (sp < 0 || gp < 0) return std::nullopt;
    // A* over the polygons: the cost to a polygon is the distance walked to the middle of the
    // portal it is entered by; ties break deterministically.
    const std::size_t n = m.polys.size();
    struct Node { float f, gcost; int poly; std::uint32_t order; };
    struct Less { bool operator()(const Node& a, const Node& b) const { if (a.f != b.f) return a.f > b.f; if (a.gcost != b.gcost) return a.gcost < b.gcost; return a.order > b.order; } };
    std::priority_queue<Node, std::vector<Node>, Less> open;
    std::vector<float> cost(n, std::numeric_limits<float>::infinity());
    std::vector<P2> entry(n, start);
    std::vector<std::pair<int, int>> came(n, {-1, -1});   // previous polygon and the portal index into this one
    std::vector<std::uint8_t> closed(n, 0);
    std::uint32_t order = 0;
    cost[static_cast<std::size_t>(sp)] = 0;
    open.push({len2(sub(goal, start)), 0, sp, order++});
    bool reached = false;
    while (!open.empty()) {
        const Node cur = open.top();
        open.pop();
        const auto ci = static_cast<std::size_t>(cur.poly);
        if (closed[ci]) continue;
        closed[ci] = 1;
        ++out.expanded;
        if (cur.poly == gp) { reached = true; break; }
        const auto& ports = m.portals[ci];
        for (std::size_t k = 0; k < ports.size(); ++k) {
            const NavMesh::Portal& pt = ports[k];
            const auto ti = static_cast<std::size_t>(pt.to);
            if (closed[ti]) continue;
            const P2 mid{(pt.au + pt.bu) * 0.5f, (pt.av + pt.bv) * 0.5f};
            const float c = cost[ci] + len2(sub(mid, entry[ci]));
            if (c < cost[ti]) {
                cost[ti] = c;
                entry[ti] = mid;
                came[ti] = {cur.poly, static_cast<int>(k)};
                open.push({c + len2(sub(goal, mid)), c, pt.to, order++});
            }
        }
    }
    if (!reached) return std::nullopt;
    // The portals in travel order, each oriented left and right of the way through it.
    std::vector<std::pair<int, int>> steps;   // (from polygon, portal index)
    for (int p = gp; p != sp;) {
        const auto [prev, k] = came[static_cast<std::size_t>(p)];
        steps.emplace_back(prev, k);
        p = prev;
    }
    std::reverse(steps.begin(), steps.end());
    // The cell at a plane point on a layer, if inside the grid.
    auto cell_in = [&](P2 q, int layer) -> std::optional<std::size_t> {
        const int x = static_cast<int>(std::floor((q.u - g.origin.x) / g.cell));
        const int y = g.plane == 0 ? static_cast<int>(std::floor((q.v - g.origin.z) / g.cell)) : static_cast<int>(std::floor((g.origin.y - q.v) / g.cell));
        if (!g.inside(x, y)) return std::nullopt;
        return g.index(x, y, layer);
    };
    // A portal's end is hard when the ground does not go on past it on both sides (a wall's
    // corner, a riser too tall to climb, the edge of a drop): a way round it keeps off the corner.
    auto hard_end = [&](P2 end, P2 along, int pa, int pb) {
        const float hc = g.cell * 0.5f;
        const P2 across{-along.v, along.u};
        for (float side : {1.0f, -1.0f}) {
            // The end cell on this side of the portal, in whichever polygon it belongs to.
            const P2 inner = add(end, add(mul(along, -hc), mul(across, side * hc)));
            std::optional<std::size_t> in;
            for (int poly : {pa, pb}) {
                const auto c = cell_in(inner, m.polys[static_cast<std::size_t>(poly)].layer);
                if (c && m.cell_poly[*c] == poly) in = c;
            }
            const auto past = cell_in(add(end, add(mul(along, hc), mul(across, side * hc))), 0);
            if (!in || !past || !g.step_to(*in, g.x_of(*past), g.y_of(*past))) return true;
        }
        return false;
    };
    std::vector<std::pair<P2, P2>> lr;
    for (const auto& [from_poly, k] : steps) {
        const NavMesh::Portal& pt = m.portals[static_cast<std::size_t>(from_poly)][static_cast<std::size_t>(k)];
        P2 a{pt.au, pt.av}, b{pt.bu, pt.bv};
        // A hard end drawn in by half a cell (less on a short portal).
        const float span = len2(sub(b, a));
        if (span > 1e-6f) {
            const P2 dir = mul(sub(b, a), 1.0f / span);
            const float in = std::min(g.cell * 0.5f, span * 0.45f);
            const bool hard_a = hard_end(a, mul(dir, -1.0f), from_poly, pt.to), hard_b = hard_end(b, dir, from_poly, pt.to);
            if (hard_a) a = add(a, mul(dir, in));
            if (hard_b) b = sub(b, mul(dir, in));
        }
        const P2 ca = poly_center(g, m.polys[static_cast<std::size_t>(from_poly)]), cb = poly_center(g, m.polys[static_cast<std::size_t>(pt.to)]);
        if (triarea2(ca, cb, a) >= 0.0f) lr.emplace_back(b, a);   // a lies to the right of the way: right is a
        else lr.emplace_back(a, b);
    }
    std::vector<P2> corners = funnel(start, goal, lr);
    // The corners in the world. Each leg is walked every quarter cell on the floor nearest the
    // last (the way up a ramp climbs it; a way under a bridge stays under), and an obstacle across
    // a leg leaves the way to the cell path.
    const bool ground = g.plane == 0 && !g.ground.empty();
    float h = ground ? g.ground[start_cell] : 0.0f;
    out.points.push_back(ground ? Vec3{corners.front().u, h, corners.front().v} : lift(g, corners.front(), h));
    for (std::size_t i = 1; i < corners.size(); ++i) {
        const P2 a = corners[i - 1], b = corners[i];
        if (ground || !g.blocked.empty()) {
            const float d = len2(sub(b, a));
            const int samples = std::max(1, static_cast<int>(std::ceil(d / (g.cell * 0.25f))));
            for (int s = i == 1 ? 0 : 1; s <= samples; ++s) {
                const P2 q = add(a, mul(sub(b, a), static_cast<float>(s) / static_cast<float>(samples)));
                const std::size_t c = cell_under(g, q, h);
                if (ground && g.walkable[c] != 0) h = g.ground[c];
                if (!g.blocked.empty() && g.blocked[c] != 0) return std::nullopt;
            }
        }
        if (i + 1 == corners.size() && ground) h = g.ground[goal_cell];
        out.points.push_back(ground ? Vec3{b.u, h, b.v} : lift(g, b, h));
    }
    for (std::size_t i = 1; i < out.points.size(); ++i) out.length += length(out.points[i] - out.points[i - 1]);
    out.polys = static_cast<int>(steps.size()) + 1;
    return out;
}

Result<Path> Nav::path(Vec3 from, Vec3 to, bool smooth, bool use_mesh) const {
    if (!baked()) return fail("not_baked", "no navigation grid yet: bake one with nav.bake");
    if (use_mesh && smooth && !mesh_.empty()) {
        if (auto r = mesh_path(from, to)) return *r;
    }
    return grid_path(from, to, smooth);
}

}  // namespace pocket::nav
