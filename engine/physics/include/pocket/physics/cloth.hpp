// Cloth (docs/design/physics.md, Cloth): a sheet of particles in a grid, held to each other at their
// rest distances, the pinned ones carried by the entity, the rest pulled by gravity, pushed by the
// Wind and kept out of the bodies' colliders; drawn as a mesh made every tick.
#pragma once

#include <pocket/assets/assets.hpp>
#include <pocket/core/math.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <map>
#include <string>
#include <vector>

namespace pocket::physics {

class ClothRunner {
   public:
    // Every Cloth moved on by `dt` (after the bodies and the world transforms of this tick), and
    // its mesh made: `cloth:<entity>` in the asset store, the entity's derived mesh.
    void step(world::World& w, assets::AssetStore& assets, float dt, Vec3 gravity);
    [[nodiscard]] std::size_t sheets() const { return sheets_.size(); }
    // Where a sheet's particles are now, row by row from the top-left (empty for an entity without one).
    [[nodiscard]] std::vector<Vec3> particles(world::EntityId id) const;

   private:
    struct Link {
        std::uint32_t a = 0, b = 0;
        float rest = 0;
        float give = 1;   // of the sheet's stiffness: 1 along the weave, less across it and over two
    };
    struct Sheet {
        Vec2 size{0, 0};
        int across = 0, down = 0, pin = -1;   // what it was made from
        int nx = 0, ny = 0;                   // particles across and down
        std::vector<Vec3> at, was, rest;      // now, a substep ago, and in the entity's space at rest
        std::vector<char> pinned;
        std::vector<Link> links;
        std::string mesh;
    };
    void make(world::World& w, assets::AssetStore& assets, world::EntityId id, const world::Cloth& c, const Mat4& placed, Sheet& s);
    void draw(world::World& w, assets::AssetStore& assets, world::EntityId id, const Mat4& placed, const Sheet& s);
    std::map<world::EntityId, Sheet> sheets_;
};

}  // namespace pocket::physics
