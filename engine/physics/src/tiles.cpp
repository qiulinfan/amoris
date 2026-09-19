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
};

constexpr float kSkin = 1e-3f;

}  // namespace

void Physics2D::step(world::World& w, assets::AssetStore& assets, float dt) {
    stats_ = Stats2D{};
    // Maps by entity: resolved once per step.
    struct MapEntry { world::EntityId id; MapView view; };
    std::vector<MapEntry> maps;
    w.ecs().each([&](flecs::entity e, const world::TileMap& tm) {
        auto m = assets.tilemap(tm.map);
        if (!m) return;
        MapView v;
        v.map = *m;
        v.ts = tm.tile_size > 0 ? tm.tile_size : 1.0f;
        if (const auto* wt = e.try_get<world::WorldTransform>()) v.origin = wt->position;
        else if (const auto* t = e.try_get<world::Transform>()) v.origin = t->position;
        maps.push_back({e.id(), v});
    });
    struct Landing { world::EntityId id; float speed; };
    std::vector<Landing> landings;
    std::vector<std::pair<world::EntityId, world::Body2D>> writes;
    std::vector<std::pair<world::EntityId, Vec3>> positions;
    w.ecs().each([&](flecs::entity e, const world::Body2D& body_in, const world::Transform& t) {
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
        b.grounded = false;
        b.on_wall = 0;
        b.on_ceiling = false;
        if (mv) {
            // X: sweep the box to its new column range; stop at the first solid column.
            float dx = b.velocity.x * dt;
            if (dx != 0) {
                float nx = cx + dx;
                int r0 = mv->row(cy + hy - kSkin), r1 = mv->row(cy - hy + kSkin);
                if (dx > 0) {
                    int c_from = mv->col(cx + hx), c_to = mv->col(nx + hx - kSkin);
                    for (int c = c_from; c <= c_to && !b.on_wall; ++c) {
                        for (int r = r0; r <= r1; ++r) {
                            if (mv->solidity(c, r) == 1) { nx = mv->left(c) - hx - kSkin; b.on_wall = 1; break; }
                        }
                    }
                } else {
                    int c_from = mv->col(cx - hx), c_to = mv->col(nx - hx + kSkin);
                    for (int c = c_from; c >= c_to && !b.on_wall; --c) {
                        for (int r = r0; r <= r1; ++r) {
                            if (mv->solidity(c, r) == 1) { nx = mv->right(c) + hx + kSkin; b.on_wall = -1; break; }
                        }
                    }
                }
                if (b.on_wall) { b.velocity.x = 0; stats_.blocked++; }
                cx = nx;
            }
            // Y: the same downward or upward; one-way tiles only catch a box coming from above.
            float dy = b.velocity.y * dt;
            if (dy != 0) {
                float ny = cy + dy;
                int c0 = mv->col(cx - hx + kSkin), c1 = mv->col(cx + hx - kSkin);
                bool hit = false;
                if (dy < 0) {
                    const float bottom_before = cy - hy;
                    int r_from = mv->row(cy - hy), r_to = mv->row(ny - hy + kSkin);
                    for (int r = r_from; r <= r_to && !hit; ++r) {
                        for (int c = c0; c <= c1; ++c) {
                            int s = mv->solidity(c, r);
                            if (s == 1 || (s == 2 && bottom_before >= mv->top(r) - kSkin)) {
                                ny = mv->top(r) + hy + kSkin;
                                hit = true;
                                b.grounded = true;
                                break;
                            }
                        }
                    }
                } else {
                    int r_from = mv->row(cy + hy), r_to = mv->row(ny + hy - kSkin);
                    for (int r = r_from; r >= r_to && !hit; --r) {
                        for (int c = c0; c <= c1; ++c) {
                            if (mv->solidity(c, r) == 1) { ny = mv->bottom(r) - hy - kSkin; hit = true; b.on_ceiling = true; break; }
                        }
                    }
                }
                if (hit) {
                    if (b.grounded && !was_grounded) landings.push_back({e.id(), -b.velocity.y});
                    b.velocity.y = 0;
                    stats_.blocked++;
                }
                cy = ny;
            } else if (b.velocity.y == 0) {
                // Resting: still grounded while a solid or one-way tile sits right under the box.
                int c0 = mv->col(cx - hx + kSkin), c1 = mv->col(cx + hx - kSkin);
                int r = mv->row(cy - hy - 2 * kSkin);
                for (int c = c0; c <= c1 && !b.grounded; ++c) if (mv->solidity(c, r) != 0) b.grounded = true;
            }
        } else {
            cx += b.velocity.x * dt;
            cy += b.velocity.y * dt;
        }
        if (b.grounded) stats_.grounded++;
        pos.x = cx - b.offset.x;
        pos.y = cy - b.offset.y;
        positions.emplace_back(e.id(), pos);
        writes.emplace_back(e.id(), b);
    });
    for (auto& [id, b] : writes) w.set_typed<world::Body2D>(id, b);
    for (auto& [id, pos] : positions) {
        world::Transform t = *w.try_get<world::Transform>(id);
        t.position = pos;
        w.set_typed<world::Transform>(id, t);
    }
    for (const Landing& l : landings) {
        stats_.landings++;
        w.events().emit(w.tick_index(), "body2d.landed", l.id, Json{{"path", w.path(l.id)}, {"speed", l.speed}});
    }
}

Json Physics2D::describe() const {
    return Json{{"bodies", stats_.bodies}, {"grounded", stats_.grounded}, {"landings", stats_.landings}, {"blocked", stats_.blocked}};
}

}  // namespace pocket::physics
