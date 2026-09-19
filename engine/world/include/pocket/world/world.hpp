// The world: entities, hierarchy, components, views for agents.
//
// Entities are flecs entities with names and a ChildOf hierarchy; children keep creation order so
// every traversal (tree text, hashing, scene files) is deterministic. Components come from the
// metadata in engine/world/meta and are accessed generically by name with JSON, or typed from C++.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>
#include <pocket/world/components.gen.hpp>
#include <pocket/world/events.hpp>
#include <pocket/world/flecs.hpp>

#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::world {

using EntityId = std::uint64_t;

struct TreeOptions {
    EntityId root = 0;          // 0: all roots
    int depth = -1;             // -1: unlimited
    int max_entities = 200;     // budget; siblings beyond it collapse into "(+N more)"
    bool values = true;         // show component fields that differ from defaults
    std::vector<std::string> components;  // restrict shown components (empty: all)
};

struct QueryOptions {
    std::vector<std::string> with;     // components the entity must have
    std::vector<std::string> without;  // components the entity must not have
    std::string name;                  // glob on the entity name ("*" and "?")
    EntityId under = 0;                // restrict to descendants of this entity
    std::vector<std::string> fields;   // components to include in results (empty: those in `with`)
    int limit = 1000;
};

class World {
   public:
    World();
    ~World();
    World(const World&) = delete;
    World& operator=(const World&) = delete;

    // Entities -------------------------------------------------------------------------------
    Result<EntityId> spawn(std::string_view name, EntityId parent = 0, const Json& components = Json::object(), std::uint64_t cause = 0);
    Status destroy(EntityId id, std::uint64_t cause = 0);
    [[nodiscard]] bool alive(EntityId id) const;
    // The live entity whose index (low 32 bits) matches, e.g. from the renderer's id buffer; 0 if none.
    [[nodiscard]] EntityId from_index(std::uint32_t index) const;
    [[nodiscard]] EntityId find(std::string_view path) const;  // "/A/B", "A/B" or a bare name searched from the roots
    [[nodiscard]] std::string path(EntityId id) const;
    [[nodiscard]] std::string name(EntityId id) const;
    Status rename(EntityId id, std::string_view name);
    Status reparent(EntityId id, EntityId new_parent);
    [[nodiscard]] EntityId parent(EntityId id) const;
    [[nodiscard]] std::vector<EntityId> children(EntityId id) const;  // creation order
    [[nodiscard]] std::vector<EntityId> roots() const;
    [[nodiscard]] std::size_t entity_count() const;

    // Components (generic) -------------------------------------------------------------------
    [[nodiscard]] bool has(EntityId id, std::string_view component) const;
    [[nodiscard]] Result<Json> get(EntityId id, std::string_view component) const;
    // Merge `partial` onto the current value (or the default when absent), then store.
    Status set(EntityId id, std::string_view component, const Json& partial, std::uint64_t cause = 0);
    Status remove(EntityId id, std::string_view component, std::uint64_t cause = 0);
    [[nodiscard]] std::vector<std::string> components_of(EntityId id) const;
    // Every serialized component of an entity as {name: value} (what scenes and the recorder store).
    [[nodiscard]] Json components_json(EntityId id) const;
    // The hash of each serialized component of an entity (cheap change detection without JSON).
    void component_hashes(EntityId id, const std::function<void(std::string_view name, std::uint64_t hash)>& fn) const;
    // Every entity in tree order (parents before children, siblings in creation order).
    void visit_all(const std::function<void(EntityId id, EntityId parent, int depth)>& fn) const;
    [[nodiscard]] static bool known_component(std::string_view component);

    // Components (typed, for engine systems) -------------------------------------------------
    template <class T>
    [[nodiscard]] const T* try_get(EntityId id) const {
        flecs::entity e = entity(id);
        return (id != 0 && e.is_alive()) ? e.try_get<T>() : nullptr;
    }
    template <class T>
    void set_typed(EntityId id, const T& value) {
        entity(id).set<T>(value);
    }
    [[nodiscard]] flecs::entity entity(EntityId id) const;
    [[nodiscard]] flecs::world& ecs();
    [[nodiscard]] const flecs::world& ecs() const;

    // Views ----------------------------------------------------------------------------------
    [[nodiscard]] Json describe(EntityId id) const;
    [[nodiscard]] std::string tree(const TreeOptions& options) const;
    [[nodiscard]] Json query(const QueryOptions& options) const;
    [[nodiscard]] Json summary() const;
    [[nodiscard]] static Json schema();  // component metadata as JSON

    // Sprite clips ---------------------------------------------------------------------------
    // A run of frames on a sheet laid out as a grid: frame i is cell frames[i] (column-major
    // index: cell = row * columns + column). SpriteAnimation names a clip; the tick writes Sprite.uv.
    struct SpriteClip {
        std::string texture;          // sheet image; empty keeps the Sprite's own texture
        int columns = 1;
        int rows = 1;
        std::vector<int> frames;      // cell indices in play order
        float fps = 8.0f;
        bool loop = true;
        [[nodiscard]] Json to_json() const;
        static Result<SpriteClip> from_json(const Json& j);
    };
    void define_clip(const std::string& name, SpriteClip clip);
    [[nodiscard]] const SpriteClip* clip(std::string_view name) const;
    [[nodiscard]] const std::map<std::string, SpriteClip>& clips() const;
    // uv rectangle (u0, v0, u1, v1) of one cell of a clip's grid.
    [[nodiscard]] static Vec4 cell_uv(const SpriteClip& clip, int cell);

    // Simulation -----------------------------------------------------------------------------
    // Runs the built-in systems (motion, lifetime, sprite animation, transform propagation) for one tick.
    void tick(double dt);
    // Recompute WorldTransform and Bounds from the Transform hierarchy without ticking (edits while paused, loads).
    void update_transforms();
    void update_bounds();
    // Typed-array access for hot loops: copy numeric fields of one component for every matching
    // entity into a flat float array (stride = sum of field sizes) plus their ids, and back.
    struct PackInfo {
        std::size_t count = 0;
        std::size_t stride = 0;
        std::vector<std::pair<std::string, std::size_t>> layout;  // field -> offset within the stride
    };
    Result<PackInfo> pack(std::string_view component, const std::vector<std::string>& fields, const QueryOptions& options, std::vector<float>& data, std::vector<double>& ids) const;
    Status unpack(std::string_view component, const std::vector<std::string>& fields, const double* ids, std::size_t count, const float* data);
    // Local bounds of an asset mesh (by MeshRenderer.mesh path) so Bounds can be computed for it.
    void set_mesh_bounds(std::string_view mesh, Vec3 min, Vec3 max);
    [[nodiscard]] std::int64_t tick_index() const;
    void set_tick_index(std::int64_t tick);
    // Deterministic hash of every entity path and component value in tree order.
    [[nodiscard]] std::uint64_t hash() const;

    // Scenes ---------------------------------------------------------------------------------
    [[nodiscard]] Json save() const;
    Status load(const Json& scene, bool clear_first = true);
    // Spawn a scene fragment (a prefab) under `parent`; `overrides` merge onto each root's components
    // and `root_name` renames a single root. Returns the created roots.
    Result<std::vector<EntityId>> instantiate(const Json& fragment, EntityId parent = 0, const Json& overrides = Json::object(), std::string_view root_name = "", std::uint64_t cause = 0);
    // One entity with its descendants as a scene fragment (a prefab file's content).
    [[nodiscard]] Json save_subtree(EntityId id) const;
    [[nodiscard]] Json save_entity_json(EntityId id) const;
    void clear();

    [[nodiscard]] EventLog& events();
    [[nodiscard]] const EventLog& events() const;

   private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

// Compact human/agent-readable form of a component value: "pos=(0,1,0) rot=(0,0,0,1)".
std::string format_component_compact(std::string_view component, const Json& value, bool only_non_default);

}  // namespace pocket::world
