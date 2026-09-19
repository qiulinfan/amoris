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
#include <map>
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
    float agent_radius = 0;                  // the radius the grid was baked for; obstacles grow by it
    std::vector<std::uint8_t> walkable;      // row-major: the level
    std::vector<std::uint8_t> blocked;       // row-major: the moment (cells under obstacles); empty when none
    std::vector<float> ground;               // XZ grids: ground height per cell
    std::vector<std::vector<int>> links;     // platformer: extra directed moves per cell (jumps, drops), as cell indices
    std::string source;
    std::uint64_t baked_tick = 0;
    [[nodiscard]] bool inside(int x, int y) const { return x >= 0 && y >= 0 && x < width && y < height; }
    [[nodiscard]] std::size_t index(int x, int y) const { return static_cast<std::size_t>(y) * static_cast<std::size_t>(width) + static_cast<std::size_t>(x); }
    // Walkable in the level and not under an obstacle right now.
    [[nodiscard]] bool walkable_at(int x, int y) const { return inside(x, y) && walkable[index(x, y)] != 0 && (blocked.empty() || blocked[index(x, y)] == 0); }
    [[nodiscard]] std::size_t blocked_count() const;
    [[nodiscard]] Vec3 center_of(int x, int y) const;
    [[nodiscard]] bool cell_of(Vec3 p, int& x, int& y) const;  // false when the point is outside the grid
    [[nodiscard]] std::size_t walkable_count() const;
    [[nodiscard]] std::size_t link_count() const;
};

// A disc in the grid's plane that blocks the cells under it (docs/design/navigation.md, Obstacles).
struct Obstacle {
    world::EntityId id = 0;
    Vec3 position;
    float radius = 0.5f;
};

// What the last step() did with the world's NavAgent and NavObstacle entities.
struct CrowdStats {
    int agents = 0, moving = 0, arrived = 0, stuck = 0, obstacles = 0, blocked = 0, replans = 0, avoiding = 0, queuing = 0;
};

// The walkable cells covered by rectangles that share edges (docs/design/navigation.md, Navmesh):
// long paths are searched over the rectangles and pulled through the shared edges.
struct NavMesh {
    struct Poly { int x0 = 0, y0 = 0, x1 = 0, y1 = 0; };      // cell ranges, inclusive
    struct Portal { int to = -1; float au = 0, av = 0, bu = 0, bv = 0; };  // the shared edge in the grid's plane
    std::vector<Poly> polys;
    std::vector<std::vector<Portal>> portals;                 // per polygon
    std::vector<int> cell_poly;                               // per cell: its polygon, -1 when not walkable
    [[nodiscard]] bool empty() const { return polys.empty(); }
    [[nodiscard]] std::size_t portal_count() const;
};

struct Path {
    std::vector<Vec3> points;  // from the start cell's center to the goal's
    float length = 0;
    int expanded = 0;          // A* nodes expanded (cells, or polygons on a navmesh path)
    int cells = 0;             // cells along the path before smoothing (0 on a navmesh path)
    int polys = 0;             // polygons crossed on a navmesh path
    bool partial = false;      // the goal was unreachable: the path ends at the closest cell A* reached
    bool snapped = false;      // the start or the goal was off walkable ground and moved to the nearest cell
    bool mesh = false;         // found over the navmesh (else over the cells)
};

class Nav {
   public:
    // Bake a grid over the XZ rectangle from the static colliders: a cell is walkable where a ray
    // down finds static ground flatter than max_slope, inside the height band, with room for the
    // agent (spheres at its feet and head hit nothing static).
    Status bake_colliders(const world::World& world, const physics::Physics& physics, const BakeParams& params, std::uint64_t tick);
    // Bake from a TileMap entity in its XY plane (docs/design/tilemaps.md).
    Status bake_tilemap(const world::World& world, assets::AssetStore& assets, world::EntityId map_entity, const TileBakeParams& params, std::uint64_t tick);
    void set_grid(Grid grid);
    void clear();
    [[nodiscard]] bool baked() const { return grid_.width > 0 && grid_.height > 0; }
    [[nodiscard]] const Grid& grid() const { return grid_; }
    [[nodiscard]] const NavMesh& mesh() const { return mesh_; }
    [[nodiscard]] Json mesh_json() const;
    // The way between two points (each snapped to the nearest walkable cell within two cells): over
    // the navmesh when there is one, the path is smooth and the mesh path crosses no obstacle, else
    // A* over the cells, string-pulled when smooth. Unreachable goals give a partial cell path.
    [[nodiscard]] Result<Path> path(Vec3 from, Vec3 to, bool smooth = true, bool use_mesh = true) const;
    // Strict: both points on walkable cells and a complete path between them.
    [[nodiscard]] bool reachable(Vec3 from, Vec3 to) const;
    // The nearest walkable cell center within max_radius of a point (on ground grids the height
    // difference counts too, so a foot of a pillar snaps to the ground beside it, not its top).
    [[nodiscard]] std::optional<Vec3> nearest(Vec3 p, float max_radius) const;
    [[nodiscard]] Json describe() const;
    // Block the cells within each obstacle's radius (plus the grid's agent radius) of its position
    // for every query until the next call. step() applies the world's NavObstacle entities.
    void set_obstacles(std::vector<Obstacle> obstacles);
    [[nodiscard]] const std::vector<Obstacle>& obstacles() const { return obstacles_; }
    // Move the world's NavAgent entities one tick: paths around the obstacles, local avoidance
    // between agents, arrival and stuck events (docs/design/navigation.md, Agents).
    void step(world::World& w, float dt);
    [[nodiscard]] const CrowdStats& crowd_stats() const { return crowd_; }
    // Every agent with its state and plan, ordered by entity id.
    [[nodiscard]] Json agents(world::World& w) const;

   private:
    struct AgentRun {
        std::vector<Vec3> path;   // the corners planned last, the first being the start cell's center
        std::size_t next = 1;     // the corner being headed for
        std::uint64_t planned_tick = 0;
        Vec3 goal;                // the goal the path was planned for
        bool partial = false;
        bool planned = false;
    };
    // A formation leader as its followers see it: where it was last tick, the way it last moved and how fast.
    struct Lead {
        Vec3 last;
        float hu = 1, hv = 0, speed = 0;
        std::uint64_t tick = 0;
        bool seen = false;
    };
    void reapply_obstacles();
    void build_mesh();
    [[nodiscard]] Result<Path> grid_path(Vec3 from, Vec3 to, bool smooth) const;
    [[nodiscard]] std::optional<Path> mesh_path(Vec3 from, Vec3 to) const;
    Grid grid_;
    NavMesh mesh_;
    std::vector<Obstacle> obstacles_;
    std::map<world::EntityId, AgentRun> runs_;
    std::map<world::EntityId, Lead> leads_;
    CrowdStats crowd_;
};

}  // namespace pocket::nav
