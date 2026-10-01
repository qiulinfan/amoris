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
#include <span>
#include <string_view>
#include <vector>

namespace pocket::world {

using EntityId = std::uint64_t;

// What a command was given for a component, checked against the metadata (docs/generated/
// components.md) before it is applied: every key a field, every value of the field's type, a
// named value one of the field's names. bad_args names the field, and the nearest one when a key
// is misspelt. Scene, prefab and model loads are not held to it.
Status check_component_patch(std::span<const ComponentInfo> known, std::string_view component, const Json& patch);
Status check_components(std::span<const ComponentInfo> known, const Json& components);   // {Name: fields, ...}

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
    // A component's fields as plain numbers (generated read_numbers / write_numbers; the scripts'
    // path without JSON): the count, or -1 when the entity lacks it, -2 when it is not all numbers
    // (or unknown), -3 when the entity is gone, and for set -4 when the count is not the component's.
    long get_numbers(EntityId id, std::string_view component, double* out) const;
    // The same by the component's index (component_index), without finding it by name: -2 for an
    // index that names none. The index is fixed for the world's life.
    [[nodiscard]] int component_index(std::string_view component) const;
    long get_numbers(EntityId id, int component, double* out) const;
    long set_numbers(EntityId id, int component, const double* in, std::size_t n);
    // Every entity's saved components (the derived ones, WorldTransform and Bounds, left out) by
    // id, with its path: what world.mark keeps and world.diff compares.
    [[nodiscard]] Json snapshot() const;
    long set_numbers(EntityId id, std::string_view component, const double* in, std::size_t n);
    [[nodiscard]] std::vector<std::string> components_of(EntityId id) const;
    // Every serialized component of an entity as {name: value} (what scenes and the recorder store).
    [[nodiscard]] Json components_json(EntityId id) const;
    // The hash of each serialized component of an entity (cheap change detection without JSON).
    void component_hashes(EntityId id, const std::function<void(std::string_view name, std::uint64_t hash)>& fn) const;
    // Every entity in tree order (parents before children, siblings in creation order).
    void visit_all(const std::function<void(EntityId id, EntityId parent, int depth)>& fn) const;
    // One serialized component's hash, by its index among the world's components (component_name).
    struct ComponentHash {
        std::uint32_t component = 0;
        std::uint64_t hash = 0;
        bool operator==(const ComponentHash&) const = default;
    };
    // Every entity in tree order with its parent, its name and the hashes of the serialized
    // components it carries (ascending component index), in one walk: what a recorder compares
    // tick to tick. Which components an entity carries is asked once per flecs table.
    void scan_hashes(const std::function<void(EntityId id, EntityId parent, std::string_view name, const ComponentHash* hashes, std::size_t count)>& fn) const;
    [[nodiscard]] std::string_view component_name(std::uint32_t component) const;
    [[nodiscard]] std::size_t component_count() const;
    [[nodiscard]] bool known_component(std::string_view component) const;
    // The project's own components (its components.toml, as the tool hands it over: [{name, doc,
    // fields: [{name, type, default?, doc?, names?}]}]), beside the engine's; declaring again
    // replaces the list. Their values are JSON objects kept to the declared types (docs/decisions/0007).
    Status declare_components(const Json& list);
    [[nodiscard]] bool project_component(std::string_view component) const;
    // Every component this world knows, the engine's then the project's.
    [[nodiscard]] std::span<const ComponentInfo> component_infos_all() const;
    // A command's component values checked against them (see check_component_patch below).
    Status check_patch(std::string_view component, const Json& patch) const;
    Status check_components(const Json& components) const;

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
    [[nodiscard]] Json schema() const;  // component metadata as JSON

    // Sprite clips ---------------------------------------------------------------------------
    // A run of frames on a sheet laid out as a grid: frame i is cell frames[i] (column-major
    // index: cell = row * columns + column). SpriteAnimation names a clip; the tick writes Sprite.uv.
    struct SpriteClip {
        std::string texture;          // sheet image; empty keeps the Sprite's own texture
        int columns = 1;
        int rows = 1;
        std::vector<int> frames;      // cell indices in play order (indices into `rects` when it has any)
        float fps = 8.0f;
        bool loop = true;
        std::vector<Vec4> rects;      // a packed sheet's frames (u0, v0, u1, v1), in place of the grid
        std::vector<float> durations; // seconds each entry of `frames` shows, in place of 1 / fps
        [[nodiscard]] Json to_json() const;
        static Result<SpriteClip> from_json(const Json& j);
    };
    void define_clip(const std::string& name, SpriteClip clip);
    [[nodiscard]] const SpriteClip* clip(std::string_view name) const;
    [[nodiscard]] const std::map<std::string, SpriteClip>& clips() const;
    // uv rectangle (u0, v0, u1, v1) of one cell of a clip's grid, or of one of its packed rects.
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
    // Where bounds come from for a mesh path not yet set (the asset store), so an entity's Bounds
    // do not wait for the mesh to be drawn: true with the local bounds, false when it has none.
    void set_mesh_bounds_source(std::function<bool(const std::string& mesh, Vec3& min, Vec3& max)> source);
    // The mesh the engine made for an entity (a Terrain's, docs/design/terrain.md): the renderer
    // draws it and a mesh collider collides as it in place of MeshRenderer.mesh and Collider.mesh.
    // An empty path clears it.
    void set_derived_mesh(EntityId id, std::string path);
    [[nodiscard]] const std::string* derived_mesh(EntityId id) const;
    // Copies of an entity's MeshRenderer the engine placed (a Scatter's): each drawn at its own
    // world matrix, its colour scaled by `shade`, in place of the entity's one draw.
    struct Instance {
        Mat4 model;
        float shade = 1;
    };
    void set_derived_instances(EntityId id, std::vector<Instance> instances);   // an empty list draws nothing
    void clear_derived_instances(EntityId id);                                   // back to the entity's own draw
    [[nodiscard]] const std::vector<Instance>* derived_instances(EntityId id) const;
    [[nodiscard]] std::int64_t tick_index() const;
    // Changes whenever a body may have moved or changed shape through the world's writes (a
    // Transform, RigidBody or Collider set, added or removed; the tree changed): the key of a
    // cache of where the colliders are, with tick_index for what systems move in a tick.
    [[nodiscard]] std::uint64_t placement_version() const;
    void set_tick_index(std::int64_t tick);
    // Simulated time since the run began (the tick it began on set by set_run_start: a restarted
    // project's wind, waves and cloth start over as a fresh run's do), in ticks times the length
    // of the last tick.
    [[nodiscard]] double seconds() const;
    void set_run_start(std::int64_t tick);
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
std::string format_component_compact(std::string_view component, const Json& value, bool only_non_default, const Json& defaults);

}  // namespace pocket::world
