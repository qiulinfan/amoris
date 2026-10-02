#pragma once
// Behaviors (docs/design/behavior.md): the Behavior component's state machines, stepped every
// tick after the hits and before the navigation agents move.

#include <pocket/core/condition.hpp>
#include <pocket/physics/physics.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <functional>
#include <map>
#include <string>
#include <vector>

namespace pocket::app {

class Behaviors {
   public:
    // Each enabled Behavior: perceive (distance, sight, health, hits, events since the last tick),
    // take the first transition that holds, and drive its NavAgent for the state it is in.
    // `hidden(self, target, from, to)` says whether what the 3D ray does not see blocks the line
    // between the two in the XY plane (a tile map's walls, the 2D bodies).
    using Hidden = std::function<bool(world::EntityId, world::EntityId, Vec3, Vec3)>;
    void step(world::World& w, const physics::Physics* physics, float dt, std::uint64_t seed, const Hidden& hidden = {});

   private:
    struct Compiled {
        std::vector<world::BehaviorTransition> transitions;   // what the programs were read from
        std::vector<ConditionProgram> programs;
        std::string error;
    };
    std::map<world::EntityId, Compiled> compiled_;
    std::map<world::EntityId, std::string> entered_;   // the state each was last seen in, to notice a state written from outside
    std::uint64_t seen_seq_ = 0;                        // events up to here were looked at
};

}  // namespace pocket::app
