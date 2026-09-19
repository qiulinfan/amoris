#pragma once
// Navigation (docs/design/navigation.md): a walkability grid over a plane, baked from what the
// physics sees (static colliders) or from a tile map, and A* over it. Scripts steer enemies with
// it; agents ask "can it get there" and "which way" without vision.
#include <pocket/assets/assets.hpp>
#include <pocket/core/json.hpp>
#include <pocket/core/math.hpp>
#include <pocket/core/result.hpp>
#include <pocket/physics/physics.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace pocket::nav {

struct BakeParams {
    Vec3 min{-10, -1, -10}, max{10, 3, 10};  // the XZ rectangle and the height band to bake
    float cell = 0.5f;
    float agent_radius = 0.3f;
    float agent_height = 1.6f;
    float max_step = 0.4f;          // height difference two neighbouring cells may have
    float max_slope_degrees = 45.0f;
    bool diagonal = true;
};

struct TileBakeParams {
    std::string mode = "topdown";  // "topdown": every empty cell; "platformer": empty cells with support below, linked by jumps and drops
    int agent_height = 1;          // cells that must be empty above a walkable cell (the cell included)
    int jump_height = 2;           // platformer: cells a jump link may rise
    int jump_range = 3;            // platformer: cells a jump or drop link may cross sideways
    int max_drop = 6;              // platformer: cells a drop link may fall
    bool diagonal = true;          // topdown only
};

struct Grid {
    int width = 0, height = 0;
    float cell = 0.5f;
    Vec3 origin;             // world position of cell (0, 0)'s corner
    int plane = 0;           // 0: XZ (rows along +Z, ground heights in y), 1: XY (rows go down, -Y)
    float depth = 0;         // XY grids: the z of every point
    bool diagonal = true;
    float max_step = 0.4f;
    std::vector<std::uint8_t> walkable;      // row-major
    std::vector<float> ground;               // XZ grids: ground height per cell
    std::vector<std::vector<int>> links;     // platformer: extra directed moves per cell (jumps, drops), as cell indices
    std::string source;
    std::uint64_t baked_tick = 0;
    [[nodiscard]] bool inside(int x, int y) const { return x >= 0 && y >= 0 && x < width && y < height; }
    [[nodiscard]] std::size_t index(int x, int y) const { return static_cast<std::size_t>(y) * static_cast<std::size_t>(width) + static_cast<std::size_t>(x); }
    [[nodiscard]] bool walkable_at(int x, int y) const { return inside(x, y) && walkable[index(x, y)] != 0; }
    [[nodiscard]] Vec3 center_of(int x, int y) const;
    [[nodiscard]] bool cell_of(Vec3 p, int& x, int& y) const;  // false when the point is outside the grid
    [[nodiscard]] std::size_t walkable_count() const;
    [[nodiscard]] std::size_t link_count() const;
};

struct Path {
    std::vector<Vec3> points;  // from the start cell's center to the goal's
    float length = 0;
    int expanded = 0;          // A* nodes expanded
    int cells = 0;             // cells along the path before smoothing
    bool partial = false;      // the goal was unreachable: the path ends at the closest cell A* reached
    bool snapped = false;      // the start or the goal was off walkable ground and moved to the nearest cell
};

class Nav {
   public:
    // Bake a grid over the XZ rectangle from the static colliders: a cell is walkable where a ray
    // down finds static ground flatter than max_slope, inside the height band, with room for the
    // agent (spheres at its feet and head hit nothing static).
    Status bake_colliders(const world::World& world, const physics::Physics& physics, const BakeParams& params, std::uint64_t tick);
    // Bake from a TileMap entity in its XY plane (docs/design/tilemaps.md).
    Status bake_tilemap(const world::World& world, assets::AssetStore& assets, world::EntityId map_entity, const TileBakeParams& params, std::uint64_t tick);
    void set_grid(Grid grid) { grid_ = std::move(grid); }
    void clear() { grid_ = Grid{}; }
    [[nodiscard]] bool baked() const { return grid_.width > 0 && grid_.height > 0; }
    [[nodiscard]] const Grid& grid() const { return grid_; }
    // A* between the cells under two points (each snapped to the nearest walkable cell within two
    // cells), the result string-pulled when smooth. Unreachable goals give a partial path.
    [[nodiscard]] Result<Path> path(Vec3 from, Vec3 to, bool smooth = true) const;
    // Strict: both points on walkable cells and a complete path between them.
    [[nodiscard]] bool reachable(Vec3 from, Vec3 to) const;
    // The nearest walkable cell center within max_radius of a point (on ground grids the height
    // difference counts too, so a foot of a pillar snaps to the ground beside it, not its top).
    [[nodiscard]] std::optional<Vec3> nearest(Vec3 p, float max_radius) const;
    [[nodiscard]] Json describe() const;

   private:
    Grid grid_;
};

}  // namespace pocket::nav
