// Ragdolls (docs/design/animation.md, Ragdolls): a skinned character's bones made into rigid
// bodies when its Ragdoll becomes active, and its pose taken from those bodies while it is.
#pragma once

#include <pocket/assets/assets.hpp>
#include <pocket/core/math.hpp>
#include <pocket/physics/physics.hpp>
#include <pocket/renderer/animation.hpp>
#include <pocket/world/world.hpp>

#include <map>
#include <set>
#include <string>
#include <vector>

namespace pocket::app {

class Ragdolls {
   public:
    // After the physics step and the animation: ragdolls that became active are made, those that
    // stopped (or whose entity went) are taken apart, and the active ones are posed from their bodies.
    void step(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, physics::Physics& physics);

   private:
    struct Part {
        int node = -1;                 // the bone
        world::EntityId body = 0;
        Mat4 offset;                   // the bone's world matrix in its body's frame, as made
    };
    struct Active {
        world::EntityId root = 0;      // the entity holding the bodies
        std::string mesh;
        std::vector<Part> parts;
        std::vector<int> part_of;      // per node: its part, or -1
        std::vector<Mat4> rest;        // per node: its matrix in its parent's (a root node's in the entity's), as made
        Mat4 entity0;                  // the entity's world matrix when made
        Vec3 hips0{0, 0, 0};           // the root body's place when made
        std::size_t hips = 0;          // the root body's part
        renderer::Pose last;           // the pose it made last (the clips have made theirs over it by the time it stops)
    };
    bool start(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, physics::Physics& physics, world::EntityId id, Active& out);
    void stop(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, world::EntityId id, Active& a);
    void pose(world::World& w, assets::AssetStore& assets, renderer::Animation& animation, world::EntityId id, Active& a);
    std::map<world::EntityId, Active> active_;
    std::set<world::EntityId> refused_;   // active but not made (no skinned mesh): not tried again until turned off
};

}  // namespace pocket::app
