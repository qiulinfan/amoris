#pragma once
// Skeletal animation for glTF assets (docs/design/animation.md): Animator components advance on
// the fixed tick, clips are sampled into the asset's node hierarchy, and each skinned mesh gets a
// pose (joint matrices) the renderer uploads for its vertex shader.
#include <pocket/assets/assets.hpp>
#include <pocket/core/json.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <map>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::renderer {

struct Pose {
    std::string mesh;                             // asset path the pose belongs to
    std::vector<Mat4> globals;                    // one per node of the asset (file space)
    std::vector<std::vector<Mat4>> joints;        // per skin: global * inverse bind, one per joint
};

class Animation {
   public:
    // Advance every entity with an Animator and a skinned MeshRenderer asset; poses are rebuilt.
    void step(world::World& world, assets::AssetStore& assets, float dt);
    [[nodiscard]] const Pose* pose(world::EntityId id) const;
    [[nodiscard]] std::size_t posed() const { return poses_.size(); }
    // Joints of an entity's pose in world space (position and the bone's Y axis), for agents.
    [[nodiscard]] Json describe_pose(const world::World& world, world::EntityId id, const assets::Mesh& mesh) const;
    // Sample a clip at `time` into globals (rest pose for nodes the clip leaves alone).
    static void sample(const assets::Mesh& mesh, const assets::AnimationClip* clip, float time, Pose& out);
    // Sample two clips and blend their node transforms (weight 0 = a, 1 = b) before composing.
    static void blend(const assets::Mesh& mesh, const assets::AnimationClip* a, float time_a, const assets::AnimationClip* b, float time_b, float weight, Pose& out);
    // The node names of a layer mask ("spine, head" -> spine, head); empty for an empty mask.
    static std::vector<std::string> mask_names(std::string_view mask);

   private:
    std::map<world::EntityId, Pose> poses_;
};

}  // namespace pocket::renderer
