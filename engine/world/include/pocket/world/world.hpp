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

    // Simulation -----------------------------------------------------------------------------
    // Runs the built-in systems (motion, lifetime, transform propagation) for one tick.
    void tick(double dt);
    // Local bounds of an asset mesh (by MeshRenderer.mesh path) so Bounds can be computed for it.
    void set_mesh_bounds(std::string_view mesh, Vec3 min, Vec3 max);
    [[nodiscard]] std::int64_t tick_index() const;
    void set_tick_index(std::int64_t tick);
    // Deterministic hash of every entity path and component value in tree order.
    [[nodiscard]] std::uint64_t hash() const;

    // Scenes ---------------------------------------------------------------------------------
    [[nodiscard]] Json save() const;
    Status load(const Json& scene, bool clear_first = true);
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
