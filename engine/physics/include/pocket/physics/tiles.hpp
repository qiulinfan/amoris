// 2D platformer physics against tile maps (docs/design/tilemaps.md): Body2D boxes fall, move
// along X then Y, and stop at the solid cells of a TileMap; one-way tiles stop them from above
// only. Deterministic, in entity order, on the fixed tick.
#pragma once

#include <pocket/assets/assets.hpp>
#include <pocket/core/json.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>

namespace pocket::physics {

struct Stats2D {
    std::uint32_t bodies = 0;
    std::uint32_t grounded = 0;
    std::uint32_t landings = 0;   // this step
    std::uint32_t blocked = 0;    // axis moves stopped by a tile this step
};

class Physics2D {
   public:
    void step(world::World& world, assets::AssetStore& assets, float dt);
    [[nodiscard]] const Stats2D& stats() const { return stats_; }
    [[nodiscard]] Json describe() const;

   private:
    Stats2D stats_;
};

}  // namespace pocket::physics
