#include <pocket/physics/tiles.hpp>

#include <algorithm>
#include <cmath>
#include <vector>

namespace pocket::physics {

namespace {

struct MapView {
    const assets::TileMap* map = nullptr;
    Vec3 origin;
    float ts = 1;
    // Tile column/row covering a world coordinate (rows go down from the origin).
    [[nodiscard]] int col(float x) const { return static_cast<int>(std::floor((x - origin.x) / ts)); }
    [[nodiscard]] int row(float y) const { return static_cast<int>(std::floor((origin.y - y) / ts)); }
    [[nodiscard]] float left(int c) const { return origin.x + static_cast<float>(c) * ts; }
    [[nodiscard]] float right(int c) const { return origin.x + static_cast<float>(c + 1) * ts; }
    [[nodiscard]] float top(int r) const { return origin.y - static_cast<float>(r) * ts; }
    [[nodiscard]] float bottom(int r) const { return origin.y - static_cast<float>(r + 1) * ts; }
    [[nodiscard]] int solidity(int c, int r) const { return map->solidity_at(c, r); }
    [[nodiscard]] int slope_at_cell(int c, int r) const { return map->slope_at(c, r); }
    // The solid boxes of a cell in world units: the whole cell, or the tile's collision shapes.
    struct Box { float l = 0, r = 0, b = 0, t = 0; };
    mutable std::vector<assets::TileSet::Shape> shapes;
    template <class F>
    void boxes(int c, int r, F&& fn) const {
        map->solid_boxes(c, r, shapes);
        for (const assets::TileSet::Shape& s : shapes) fn(Box{left(c) + s.x0 * ts, left(c) + s.x1 * ts, top(r) - s.y1 * ts, top(r) - s.y0 * ts});
    }
};

constexpr float kSkin = 1e-3f;

}  // namespace

// A kinematic body's box after its move this step, with the move itself (what it carries riders by).
struct Rect {
    world::EntityId id = 0;
    float l = 0, r = 0, b = 0, t = 0;
    float dx = 0, dy = 0;
    bool one_way = false;
};

void Physics2D::step(world::World& w, assets::AssetStore& assets, float dt) {
    stats_ = Stats2D{};
    // Maps by entity: resolved once per step.
    struct MapEntry { world::EntityId id; MapView view; };
    std::vector<MapEntry> maps, all_maps;   // the platformer's orthogonal maps; every map for the top-down movers
    w.ecs().each([&](flecs::entity e, const world::TileMap& tm) {
        auto m = assets.tilemap(tm.map);
        if (!m) return;
        MapView v;
        v.map = *m;
        v.ts = tm.tile_size > 0 ? tm.tile_size : 1.0f;
        if (const auto* wt = e.try_get<world::WorldTransform>()) v.origin = wt->position;
        else if (const auto* t = e.try_get<world::Transform>()) v.origin = t->position;
        all_maps.push_back({e.id(), v});
        if (!(*m)->orthogonal()) return;   // the platformer sees orthogonal maps only (docs/design/tilemaps.md, Orientations)
        maps.push_back({e.id(), v});
    });
    // Top-down movers: a point with a radius, moved by its velocity in steps no longer than the
    // radius, each step along X then Y dropped when the cell ahead of the center is solid. The
    // cell under a point comes from the map's own geometry, so any orientation works.
    auto cell_under = [](const MapView& v, float wx, float wy, int& cx, int& cy) {
        if (v.map->orthogonal()) {
            cx = v.col(wx);
            cy = v.row(wy);
        } else {
            const float sx = v.ts / static_cast<float>(v.map->tile_width);
            (void)v.map->cell_at_pixel((wx - v.origin.x) / sx, (v.origin.y - wy) / sx, cx, cy);
        }
        return cx >= 0 && cy >= 0 && cx < v.map->width && cy < v.map->height;
    };
    auto solid_under = [&](const MapView& v, float wx, float wy) {
        int cx = 0, cy = 0;
        return cell_under(v, wx, wy, cx, cy) && v.map->solid_at(cx, cy);
    };
    std::vector<std::pair<world::EntityId, world::TopDown2D>> mover_writes;
    std::vector<std::pair<world::EntityId, Vec3>> mover_positions;
    w.ecs().each([&](flecs::entity e, const world::TopDown2D& mover_in, const world::Transform& t) {
        world::TopDown2D b = mover_in;
        stats_.movers++;
        const MapView* mv = nullptr;
        if (!b.map.empty()) {
            world::EntityId mid = w.find(b.map);
            for (const MapEntry& m : all_maps) if (m.id == mid) mv = &m.view;
        } else if (!all_maps.empty()) {
            mv = &all_maps.front().view;
        }
        Vec3 pos = t.position;
        b.blocked_x = false;
        b.blocked_y = false;
        const float radius = std::max(b.radius, 0.0f);
        const bool stuck = mv && solid_under(*mv, pos.x, pos.y);   // placed inside a wall: free to move out
        const float dx = b.velocity.x * dt, dy = b.velocity.y * dt;
        const int steps = std::max(1, static_cast<int>(std::ceil(std::max(std::fabs(dx), std::fabs(dy)) / std::max(radius, 0.05f))));
        const float sx = dx / static_cast<float>(steps), sy = dy / static_cast<float>(steps);
        for (int i = 0; i < steps; ++i) {
            if (sx != 0 && !b.blocked_x) {
                if (mv && !stuck && solid_under(*mv, pos.x + sx + (sx > 0 ? radius : -radius), pos.y)) { b.blocked_x = true; stats_.blocked++; }
                else pos.x += sx;
            }
            if (sy != 0 && !b.blocked_y) {
                if (mv && !stuck && solid_under(*mv, pos.x, pos.y + sy + (sy > 0 ? radius : -radius))) { b.blocked_y = true; stats_.blocked++; }
                else pos.y += sy;
            }
        }
        int cx = -1, cy = -1;
        if (!mv || !cell_under(*mv, pos.x, pos.y, cx, cy)) { cx = -1; cy = -1; }
        b.tile_x = cx;
        b.tile_y = cy;
        mover_positions.emplace_back(e.id(), pos);
        mover_writes.emplace_back(e.id(), b);
    });
    struct Landing { world::EntityId id; float speed; };
    std::vector<Landing> landings;
    struct Bounce { world::EntityId id; float speed; const char* side; };
    std::vector<Bounce> bounces;
    constexpr float kBounceMin = 0.5f;   // slower impacts land or stop instead of bouncing
    std::vector<std::pair<world::EntityId, world::Body2D>> writes;
    std::vector<std::pair<world::EntityId, Vec3>> positions;
    std::vector<std::pair<world::EntityId, bool>> dyn_was_grounded;  // per dynamic body: grounded before this step
    // Platforms first: kinematic bodies move by their velocity, nothing stops them, and the
    // dynamic bodies below see where they are now and how far they moved.
    std::vector<Rect> rects;
    w.ecs().each([&](flecs::entity e, const world::Body2D& body_in, const world::Transform& t) {
        if (!body_in.kinematic) return;
        world::Body2D b = body_in;
        stats_.bodies++;
        stats_.platforms++;
        Vec3 pos = t.position;
        Rect rc;
        rc.id = e.id();
        rc.dx = b.velocity.x * dt;
        rc.dy = b.velocity.y * dt;
        pos.x += rc.dx;
        pos.y += rc.dy;
        const float cx = pos.x + b.offset.x, cy = pos.y + b.offset.y;
        const float hx = std::max(b.size.x, 0.01f), hy = std::max(b.size.y, 0.01f);
        rc.l = cx - hx;
        rc.r = cx + hx;
        rc.b = cy - hy;
        rc.t = cy + hy;
        rc.one_way = b.one_way;
        rects.push_back(rc);
        b.grounded = false;
        b.on_wall = 0;
        b.on_ceiling = false;
        b.riding = 0;
        b.on_slope = 0;
        positions.emplace_back(e.id(), pos);
        writes.emplace_back(e.id(), b);
    });
    w.ecs().each([&](flecs::entity e, const world::Body2D& body_in, const world::Transform& t) {
        if (body_in.kinematic) return;
        world::Body2D b = body_in;
        stats_.bodies++;
        const MapView* mv = nullptr;
        if (!b.map.empty()) {
            world::EntityId mid = w.find(b.map);
            for (const MapEntry& m : maps) if (m.id == mid) mv = &m.view;
        } else if (!maps.empty()) {
            mv = &maps.front().view;
        }
        // Gravity, clamped fall.
        b.velocity.y += b.gravity * dt;
        if (b.velocity.y < -b.max_fall) b.velocity.y = -b.max_fall;
        Vec3 pos = t.position;
        float cx = pos.x + b.offset.x, cy = pos.y + b.offset.y;
        const float hx = std::max(b.size.x, 0.01f), hy = std::max(b.size.y, 0.01f);
        const bool was_grounded = b.grounded;
        const float step = std::max(b.step, 0.0f);
        // Carried: a body standing on a platform moves with it before its own move; one standing
        // on another dynamic body moves with that body's sideways velocity.
        if (b.riding != 0) {
            bool carried = false;
            for (const Rect& rc : rects) {
                if (rc.id != b.riding) continue;
                cx += rc.dx;
                cy += rc.dy;
                carried = true;
            }
            if (!carried) {
                // A dynamic carrier's velocity is relative to what carries it in turn, so the chain is summed.
                float carrier_vx = 0;
                bool found = false;
                world::EntityId cid = b.riding;
                for (int guard = 0; cid != 0 && guard < 8; ++guard) {
                    bool on_rect = false;
                    for (const Rect& rc : rects) if (rc.id == cid) { carrier_vx += rc.dx / dt; on_rect = true; break; }
                    if (on_rect) break;
                    const auto* c = w.try_get<world::Body2D>(cid);
                    if (!c || c->kinematic) break;
                    carrier_vx += c->velocity.x;
                    found = true;
                    cid = c->riding;
                }
                if (found) {
                    cx += carrier_vx * dt;
                    carried = true;
                }
            }
            if (carried) stats_.riding++;
        }
        const bool was_riding = b.riding != 0;
        b.grounded = false;
        b.on_wall = 0;
        b.on_ceiling = false;
        b.on_slope = 0;
        world::EntityId riding = 0;
        bool stepped_tick = false;
        // The height of a slope's floor at x inside its cell.
        auto slope_height = [&](int c, int r, float x) {
            const int sl = mv->slope_at_cell(c, r);
            float tt = std::clamp((x - mv->left(c)) / mv->ts, 0.0f, 1.0f);
            if (sl < 0) tt = 1.0f - tt;
            return mv->bottom(r) + tt * mv->ts;
        };
        // X: sweep the box to its new column range; stop at the first solid column or platform,
        // unless the obstacle is low enough to step onto while grounded.
        float dx = b.velocity.x * dt;
        if (dx != 0) {
            float nx = cx + dx;
            const float bottom = cy - hy;
            int r0 = mv ? mv->row(cy + hy - kSkin) : 0, r1 = mv ? mv->row(bottom + kSkin) : -1;
            float block_x = 0;      // the x the box stops at
            float top_of_block = -1e30f; // the highest solid top in the blocking column, for stepping up
            bool blocked = false;
            if (mv) {
                // The boxes of the solid cells ahead (a tile's collision shapes, or the whole cell)
                // that the box's height crosses; the nearest one stops the move.
                if (dx > 0) {
                    int c_from = mv->col(cx + hx), c_to = mv->col(nx + hx - kSkin);
                    for (int c = c_from; c <= c_to && !blocked; ++c) {
                        for (int r = r0; r <= r1; ++r) {
                            if (mv->solidity(c, r) != 1) continue;
                            mv->boxes(c, r, [&](const MapView::Box& bx) {
                                if (bx.b >= cy + hy - kSkin || bx.t <= bottom + kSkin || bx.r <= cx + hx - kSkin || bx.l >= nx + hx) return;
                                const float x = bx.l - hx - kSkin;
                                if (!blocked || x < block_x) { blocked = true; block_x = x; }
                                top_of_block = std::max(top_of_block, bx.t);
                            });
                        }
                    }
                } else {
                    int c_from = mv->col(cx - hx), c_to = mv->col(nx - hx + kSkin);
                    for (int c = c_from; c >= c_to && !blocked; --c) {
                        for (int r = r0; r <= r1; ++r) {
                            if (mv->solidity(c, r) != 1) continue;
                            mv->boxes(c, r, [&](const MapView::Box& bx) {
                                if (bx.b >= cy + hy - kSkin || bx.t <= bottom + kSkin || bx.l >= cx - hx + kSkin || bx.r <= nx - hx) return;
                                const float x = bx.r + hx + kSkin;
                                if (!blocked || x > block_x) { blocked = true; block_x = x; }
                                top_of_block = std::max(top_of_block, bx.t);
                            });
                        }
                    }
                }
            }
            // Solid platforms block sideways too (one-way ones are nothing from the side).
            for (const Rect& rc : rects) {
                if (rc.one_way || rc.b >= cy + hy - kSkin || rc.t <= bottom + kSkin) continue;
                if (dx > 0 && rc.l >= cx + hx - kSkin && rc.l < nx + hx) {
                    float x = rc.l - hx - kSkin;
                    if (!blocked || x < block_x) { blocked = true; block_x = x; top_of_block = rc.t; }
                } else if (dx < 0 && rc.r <= cx - hx + kSkin && rc.r > nx - hx) {
                    float x = rc.r + hx + kSkin;
                    if (!blocked || x > block_x) { blocked = true; block_x = x; top_of_block = rc.t; }
                }
            }
            bool stepped = false;
            if (blocked && was_grounded && mv && top_of_block - bottom <= step + kSkin) {
                // A step: the box fits above the obstacle at the new height, so climb it.
                const float ny = top_of_block + hy + kSkin;
                bool fits = true;
                int rr0 = mv->row(ny + hy - kSkin), rr1 = mv->row(ny - hy + kSkin);
                int cc0 = mv->col(nx - hx + kSkin), cc1 = mv->col(nx + hx - kSkin);
                for (int r = rr0; r <= rr1 && fits; ++r) {
                    for (int c = cc0; c <= cc1 && fits; ++c) {
                        if (mv->solidity(c, r) != 1) continue;
                        mv->boxes(c, r, [&](const MapView::Box& bx) { if (bx.l < nx + hx && bx.r > nx - hx && bx.b < ny + hy && bx.t > ny - hy + kSkin) fits = false; });
                    }
                }
                for (const Rect& rc : rects) if (!rc.one_way && rc.l < nx + hx && rc.r > nx - hx && rc.b < ny + hy && rc.t > ny - hy + kSkin) fits = false;
                if (fits) {
                    cy = ny;
                    blocked = false;
                    stepped = true;
                    stats_.stepped++;
                }
            }
            if (blocked) {
                nx = block_x;
                b.on_wall = dx > 0 ? 1 : -1;
                if (b.restitution > 0 && std::fabs(b.velocity.x) > kBounceMin) {
                    bounces.push_back({e.id(), std::fabs(b.velocity.x), "wall"});
                    b.velocity.x = -b.velocity.x * std::min(b.restitution, 1.0f);
                } else {
                    b.velocity.x = 0;
                }
                stats_.blocked++;
            }
            cx = nx;
            if (stepped) {
                // Standing on the edge just climbed; the floor search below would only find the
                // lower floor the body came from and pull it back down.
                b.grounded = true;
                b.velocity.y = 0;
                stepped_tick = true;
            }
        }
        // Y: down onto solid tiles, one-way tiles and platforms from above, and slope floors; up
        // against solid tiles and solid platforms. A grounded body that only sinks by gravity may
        // also reach down a step for the floor, so it walks down slopes and stairs without a hop.
        float dy = b.velocity.y * dt;
        const bool just_gravity = was_grounded && !was_riding && b.velocity.y <= 0 && b.velocity.y >= b.gravity * dt * 1.5f;
        const float reach = just_gravity ? step : 0.0f;  // riders never reach down: their floor moves with them
        if (!stepped_tick && (dy != 0 || reach > 0)) {
            float ny = cy + dy;
            int c0 = mv ? mv->col(cx - hx + kSkin) : 0, c1 = mv ? mv->col(cx + hx - kSkin) : -1;
            bool hit = false;
            if (dy <= 0) {
                const float bottom_before = cy - hy;
                const float lowest = ny - hy - reach;   // how far down a floor may be
                float best = -1e30f;
                bool found = false;
                int best_slope = 0;
                world::EntityId best_rect = 0;
                auto offer = [&](float h, int slope, world::EntityId rect) {
                    if (h < lowest - kSkin) return;
                    if (h > best) { best = h; found = true; best_slope = slope; best_rect = rect; }
                };
                if (mv) {
                    int r_from = mv->row(bottom_before), r_to = mv->row(lowest + kSkin);
                    for (int r = r_from; r <= r_to; ++r) {
                        for (int c = c0; c <= c1; ++c) {
                            const int sv = mv->solidity(c, r);
                            if (sv != 1 && sv != 2) continue;
                            mv->boxes(c, r, [&](const MapView::Box& bx) {
                                if (bx.l >= cx + hx - kSkin || bx.r <= cx - hx + kSkin) return;   // beside the body
                                if (sv == 1) offer(bx.t, 0, 0);
                                else if (bottom_before >= bx.t - kSkin) offer(bx.t, 0, 0);
                            });
                        }
                    }
                    // Slopes: the floor under the box's center, in the cells its bottom passes through
                    // and the one above (walking up, the floor rises past the current bottom).
                    int cs = mv->col(cx);
                    for (int r = std::max(r_from - 1, mv->row(bottom_before + step)); r <= r_to; ++r) {
                        if (mv->solidity(cs, r) != 3) continue;
                        float h = slope_height(cs, r, cx);
                        if (h <= bottom_before + step + kSkin) offer(h, mv->slope_at_cell(cs, r), 0);
                    }
                }
                for (const Rect& rc : rects) {
                    if (rc.r <= cx - hx + kSkin || rc.l >= cx + hx - kSkin) continue;
                    if (rc.t > bottom_before + kSkin) continue;   // platforms are stood on from above only
                    offer(rc.t, 0, rc.id);
                }
                if (found) {
                    ny = best + hy + kSkin;
                    hit = true;
                    b.grounded = true;
                    b.on_slope = best_slope;
                    riding = best_rect;
                }
            } else {
                int r_from = mv ? mv->row(cy + hy) : 0, r_to = mv ? mv->row(ny + hy - kSkin) : 1;
                if (mv) {
                    for (int r = r_from; r >= r_to && !hit; --r) {
                        for (int c = c0; c <= c1; ++c) {
                            if (mv->solidity(c, r) != 1) continue;
                            mv->boxes(c, r, [&](const MapView::Box& bx) {
                                if (bx.l >= cx + hx - kSkin || bx.r <= cx - hx + kSkin || bx.t <= cy + hy - kSkin || bx.b >= ny + hy) return;
                                const float y = bx.b - hy - kSkin;
                                if (!hit || y < ny) { ny = y; hit = true; b.on_ceiling = true; }
                            });
                        }
                    }
                }
                for (const Rect& rc : rects) {
                    if (rc.one_way || rc.r <= cx - hx + kSkin || rc.l >= cx + hx - kSkin) continue;
                    if (rc.b >= cy + hy - kSkin && rc.b < ny + hy) {
                        float y = rc.b - hy - kSkin;
                        if (!hit || y < ny) { ny = y; hit = true; b.on_ceiling = true; }
                    }
                }
            }
            if (hit) {
                if (b.restitution > 0 && std::fabs(b.velocity.y) > kBounceMin) {
                    // A bounce: the speed comes back reversed and scaled, and the body is in the air again.
                    bounces.push_back({e.id(), std::fabs(b.velocity.y), b.grounded ? "floor" : "ceiling"});
                    b.velocity.y = -b.velocity.y * std::min(b.restitution, 1.0f);
                    b.grounded = false;
                    b.on_ceiling = false;
                    b.on_slope = 0;
                    riding = 0;
                } else {
                    if (b.grounded && !was_grounded) landings.push_back({e.id(), -b.velocity.y});
                    b.velocity.y = 0;
                }
                stats_.blocked++;
            }
            cy = ny;
        }
        b.riding = riding;
        pos.x = cx - b.offset.x;
        pos.y = cy - b.offset.y;
        positions.emplace_back(e.id(), pos);
        writes.emplace_back(e.id(), b);
        dyn_was_grounded.emplace_back(e.id(), was_grounded);
    });
    // Bodies against bodies: dynamic boxes that overlap are pushed apart along their smaller
    // overlap. Sideways, each gives way by the other's share of the mass (a body already against
    // a wall on that side gives none); vertically, the upper one is lifted onto the lower and
    // stands on it. Two passes settle small stacks; a body pushed into a solid tile is set back.
    struct Dyn {
        std::size_t i;  // index into writes and positions
        float cx, cy, hx, hy;
        bool was_grounded;
        const MapView* mv;
        world::EntityId prev_riding;   // what it rode last tick: this tick's riding is settled by the vertical pass below
    };
    std::vector<Dyn> dyn;
    for (std::size_t i = 0; i < writes.size(); ++i) {
        const world::Body2D& b = writes[i].second;
        if (b.kinematic || !b.collide_bodies) continue;
        const MapView* mv = nullptr;
        if (!b.map.empty()) {
            world::EntityId mid = w.find(b.map);
            for (const MapEntry& m : maps) if (m.id == mid) mv = &m.view;
        } else if (!maps.empty()) {
            mv = &maps.front().view;
        }
        bool wg = false;
        for (const auto& [id, g] : dyn_was_grounded) if (id == writes[i].first) wg = g;
        const auto* prev = w.try_get<world::Body2D>(writes[i].first);
        dyn.push_back({i, positions[i].second.x + b.offset.x, positions[i].second.y + b.offset.y, std::max(b.size.x, 0.01f), std::max(b.size.y, 0.01f), wg, mv, prev ? prev->riding : 0});
    }
    // For the sideways exchange of speeds a stack is one body (its riders move with it): the
    // bottom body's velocity, the sum of the masses. A rider's own velocity is relative to its
    // carrier and stays what it is.
    std::vector<std::size_t> root(dyn.size());
    std::vector<float> stack_mass(dyn.size()), absv(dyn.size());
    auto dyn_index = [&](world::EntityId id) -> std::size_t {
        for (std::size_t k = 0; k < dyn.size(); ++k) if (writes[dyn[k].i].first == id) return k;
        return dyn.size();
    };
    // What a body rides: settled by this tick's vertical pass once it has run, else last tick's.
    auto riding_of = [&](std::size_t k) {
        const world::EntityId now = writes[dyn[k].i].second.riding;
        return now != 0 ? now : dyn[k].prev_riding;
    };
    auto rect_vx = [&](world::EntityId id) {
        for (const Rect& rc : rects) if (rc.id == id) return rc.dx / dt;
        return 0.0f;
    };
    auto set_abs = [&](std::size_t r, float v) {
        absv[r] = v;
        writes[dyn[r].i].second.velocity.x = v - rect_vx(writes[dyn[r].i].second.riding);
    };
    for (int pass = 0; pass < 2 && dyn.size() > 1; ++pass) {
        std::fill(stack_mass.begin(), stack_mass.end(), 0.0f);
        for (std::size_t k = 0; k < dyn.size(); ++k) {
            std::size_t r = k;
            for (int guard = 0; guard < 8; ++guard) {
                const std::size_t next = dyn_index(riding_of(r));
                if (next == dyn.size() || next == r) break;
                r = next;
            }
            root[k] = r;
        }
        for (std::size_t k = 0; k < dyn.size(); ++k) stack_mass[root[k]] += std::max(writes[dyn[k].i].second.mass, 1e-3f);
        for (std::size_t k = 0; k < dyn.size(); ++k) {
            if (root[k] == k) absv[k] = writes[dyn[k].i].second.velocity.x + rect_vx(writes[dyn[k].i].second.riding);
        }
        for (std::size_t p = 0; p < dyn.size(); ++p) {
            for (std::size_t q = p + 1; q < dyn.size(); ++q) {
                Dyn& A = dyn[p];
                Dyn& B = dyn[q];
                const float ox = (A.hx + B.hx) - std::fabs(A.cx - B.cx);
                const float oy = (A.hy + B.hy) - std::fabs(A.cy - B.cy);
                if (ox <= kSkin || oy <= kSkin) continue;
                if (pass == 0) stats_.pairs++;
                world::Body2D& a = writes[A.i].second;
                world::Body2D& b = writes[B.i].second;
                if (oy < ox) {
                    Dyn& up = A.cy >= B.cy ? A : B;
                    Dyn& low = A.cy >= B.cy ? B : A;
                    world::Body2D& ub = writes[up.i].second;
                    world::Body2D& lb = writes[low.i].second;
                    if (ub.on_ceiling && !lb.grounded) {
                        low.cy -= oy + kSkin;  // squeezed under a ceiling: the lower one yields
                        if (lb.velocity.y > 0) lb.velocity.y = 0;
                    } else {
                        up.cy += oy + kSkin;
                        if (ub.restitution > 0 && ub.velocity.y < -kBounceMin) {
                            bounces.push_back({writes[up.i].first, -ub.velocity.y, "body"});
                            ub.velocity.y = -ub.velocity.y * std::min(ub.restitution, 1.0f);
                            ub.grounded = false;
                            ub.on_slope = 0;
                            ub.riding = 0;
                        } else {
                            if (!up.was_grounded && !ub.grounded && ub.velocity.y < 0) landings.push_back({writes[up.i].first, -ub.velocity.y});
                            if (ub.velocity.y < 0) ub.velocity.y = 0;
                            ub.grounded = true;
                            ub.on_slope = 0;
                            ub.riding = writes[low.i].first;
                        }
                        if (pass == 0) stats_.stacked++;
                    }
                } else {
                    const int dir = A.cx <= B.cx ? -1 : 1;  // the way A gives way
                    float sa = std::max(b.mass, 1e-3f), sb = std::max(a.mass, 1e-3f);  // each gives way by the other's mass
                    if (a.on_wall == dir) sa = 0;
                    if (b.on_wall == -dir) sb = 0;
                    if (sa + sb <= 0) continue;
                    const float total = ox + kSkin;
                    A.cx += static_cast<float>(dir) * total * sa / (sa + sb);
                    B.cx -= static_cast<float>(dir) * total * sb / (sa + sb);
                    // The speeds into each other are exchanged as a collision of the two stacks (a
                    // body held by a wall on its side counts as immovable): with no restitution both
                    // leave at the mass-weighted mean, with restitution (the larger of the two) they
                    // rebound, momentum kept either way.
                    const std::size_t rA = root[p], rB = root[q];
                    const float n = static_cast<float>(-dir);   // from A toward B
                    const float closing = rA != rB ? (absv[rA] - absv[rB]) * n : 0.0f;
                    if (closing > 0) {
                        const float inv_a = sa > 0 ? 1.0f / stack_mass[rA] : 0.0f;
                        const float inv_b = sb > 0 ? 1.0f / stack_mass[rB] : 0.0f;
                        const float e = closing > kBounceMin ? std::min(std::max(a.restitution, b.restitution), 1.0f) : 0.0f;
                        const float j = (1.0f + e) * closing / (inv_a + inv_b);
                        set_abs(rA, absv[rA] - j * inv_a * n);
                        set_abs(rB, absv[rB] + j * inv_b * n);
                        if (e > 0) {
                            if (a.restitution > 0) bounces.push_back({writes[A.i].first, closing, "body"});
                            if (b.restitution > 0) bounces.push_back({writes[B.i].first, closing, "body"});
                        }
                    }
                    if (pass == 0) stats_.pushed++;
                }
            }
        }
        // Back out of solid tiles and solid platforms a push may have moved a body into.
        for (Dyn& d : dyn) {
            world::Body2D& b = writes[d.i].second;
            auto out_of = [&](float left, float right) {
                if (d.cx <= (left + right) * 0.5f) { d.cx = left - d.hx - kSkin; b.on_wall = 1; if (b.velocity.x > 0) b.velocity.x = 0; }
                else { d.cx = right + d.hx + kSkin; b.on_wall = -1; if (b.velocity.x < 0) b.velocity.x = 0; }
            };
            if (d.mv) {
                const int r0 = d.mv->row(d.cy + d.hy - kSkin), r1 = d.mv->row(d.cy - d.hy + kSkin);
                const int c0 = d.mv->col(d.cx - d.hx + kSkin), c1 = d.mv->col(d.cx + d.hx - kSkin);
                for (int r = r0; r <= r1; ++r) {
                    for (int c = c0; c <= c1; ++c) {
                        if (d.mv->solidity(c, r) != 1) continue;
                        d.mv->boxes(c, r, [&](const MapView::Box& bx) {   // the tile's shapes, or the whole cell
                            if (bx.l < d.cx + d.hx - kSkin && bx.r > d.cx - d.hx + kSkin && bx.b < d.cy + d.hy - kSkin && bx.t > d.cy - d.hy + kSkin) out_of(bx.l, bx.r);
                        });
                    }
                }
            }
            for (const Rect& rc : rects) {
                if (rc.one_way) continue;
                if (rc.l < d.cx + d.hx - kSkin && rc.r > d.cx - d.hx + kSkin && rc.b < d.cy + d.hy - kSkin && rc.t > d.cy - d.hy + kSkin) out_of(rc.l, rc.r);
            }
        }
    }
    for (const Dyn& d : dyn) {
        positions[d.i].second.x = d.cx - writes[d.i].second.offset.x;
        positions[d.i].second.y = d.cy - writes[d.i].second.offset.y;
    }
    // Ground friction: a grounded body's sideways speed, relative to what carries it, falls toward
    // zero by `friction` per second, after the move, so what a script drives each tick is untouched.
    for (auto& [id, b] : writes) {
        if (b.kinematic || !b.grounded || b.friction <= 0) continue;
        const float rel = b.velocity.x;   // a rider's velocity is relative to its carrier already
        const float decel = b.friction * dt;
        b.velocity.x = std::fabs(rel) <= decel ? 0.0f : rel - (rel > 0 ? decel : -decel);
    }
    for (const auto& [id, b] : writes) if (!b.kinematic && b.grounded) stats_.grounded++;
    for (auto& [id, b] : writes) w.set_typed<world::Body2D>(id, b);
    for (auto& [id, pos] : positions) {
        world::Transform t = *w.try_get<world::Transform>(id);
        t.position = pos;
        w.set_typed<world::Transform>(id, t);
    }
    for (auto& [id, b] : mover_writes) w.set_typed<world::TopDown2D>(id, b);
    for (auto& [id, pos] : mover_positions) {
        world::Transform t = *w.try_get<world::Transform>(id);
        t.position = pos;
        w.set_typed<world::Transform>(id, t);
    }
    for (const Landing& l : landings) {
        stats_.landings++;
        w.events().emit(w.tick_index(), "body2d.landed", l.id, Json{{"path", w.path(l.id)}, {"speed", l.speed}});
    }
    for (const Bounce& bn : bounces) {
        stats_.bounces++;
        w.events().emit(w.tick_index(), "body2d.bounced", bn.id, Json{{"path", w.path(bn.id)}, {"speed", bn.speed}, {"side", bn.side}});
    }
}

Json Physics2D::describe() const {
    return Json{{"bodies", stats_.bodies}, {"grounded", stats_.grounded}, {"landings", stats_.landings}, {"blocked", stats_.blocked}, {"platforms", stats_.platforms}, {"riding", stats_.riding}, {"stepped", stats_.stepped}, {"pairs", stats_.pairs}, {"stacked", stats_.stacked}, {"pushed", stats_.pushed}, {"bounces", stats_.bounces}, {"movers", stats_.movers}};
}

}  // namespace pocket::physics
