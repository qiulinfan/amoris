// Frame recorder: time travel over the world.
//
// While recording, every tick's world (each entity's path and serialized components) is compared
// with the previous one and only the changes are kept, in a ring of the last N ticks. Agents ask
// what the world looked like at a tick, what changed between two ticks, how one field moved over
// time, or the first tick a field crossed a value, instead of watching frames.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>
#include <pocket/world/world.hpp>

#include <cstdint>
#include <deque>
#include <map>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

namespace pocket::world {

class Recorder {
   public:
    struct EntityState {
        std::string path;
        std::map<std::string, Json> components;
    };
    using State = std::map<EntityId, EntityState>;  // ordered by id: deterministic output

    void start(std::size_t ticks);  // keep the last `ticks` ticks (a new start clears)
    void stop();
    void clear();
    [[nodiscard]] bool recording() const { return recording_; }
    [[nodiscard]] std::size_t capacity() const { return capacity_; }
    [[nodiscard]] std::int64_t first_tick() const;  // -1 when empty
    [[nodiscard]] std::int64_t last_tick() const;   // -1 when empty
    [[nodiscard]] std::size_t frames() const { return deltas_.size(); }

    // Record the world after its tick. Cheap when little moved: only differences are stored.
    void record(const World& w, std::int64_t tick);

    [[nodiscard]] Json status() const;
    // The world at a tick (every entity, or one), as {tick, entities: [{id, path, components}]}.
    [[nodiscard]] Result<Json> at(std::int64_t tick, EntityId entity = 0) const;
    // What changed between two ticks: spawned, destroyed, renamed, and per component field values
    // before and after; `entity` narrows to one entity, `limit` caps the change list.
    [[nodiscard]] Result<Json> diff(std::int64_t from, std::int64_t to, EntityId entity = 0, std::size_t limit = 200) const;
    // One field of one component over time: {ticks: [...], values: [...]} (null while the entity or
    // component is absent), every `every` ticks.
    [[nodiscard]] Result<Json> track(EntityId entity, std::string_view component, std::string_view field, std::int64_t from = -1, std::int64_t to = -1, int every = 1) const;
    // The first tick at or after `from` where `field <op> value` holds (op: < <= > >= == !=), or null.
    [[nodiscard]] Result<Json> first(EntityId entity, std::string_view component, std::string_view field, std::string_view op, const Json& value, std::int64_t from = -1) const;

    // A dotted field ("position.y") of a JSON value; null when absent.
    static Json field_of(const Json& value, std::string_view field);
    static bool compare(const Json& lhs, std::string_view op, const Json& rhs);

   private:
    struct Delta {
        std::int64_t tick = 0;
        std::vector<std::pair<EntityId, EntityState>> spawned;
        std::vector<EntityId> destroyed;
        std::vector<std::pair<EntityId, std::string>> renamed;                          // new path
        std::vector<std::tuple<EntityId, std::string, Json>> changed;                   // component -> partial (changed top-level fields), or the whole value when added
        std::vector<std::pair<EntityId, std::string>> removed;                          // component removed
    };
    static void apply(State& state, const Delta& d);
    [[nodiscard]] State state_at(std::int64_t tick) const;

    bool recording_ = false;
    std::size_t capacity_ = 600;
    State base_;                // the world before deltas_.front()
    std::int64_t base_tick_ = -1;
    State last_;                // the world after the last recorded tick
    struct Seen {               // what was hashed last time, so unchanged components cost no JSON
        std::string name;
        EntityId parent = 0;
        std::vector<World::ComponentHash> hashes;   // ascending component index
        bool hashes_valid = false;                  // false after the world's components changed
        std::uint64_t stamp = 0;                    // the record that last saw it
        EntityState* state = nullptr;               // its entry in last_ (map nodes stay put)
    };
    std::unordered_map<EntityId, Seen> seen_;
    std::uint64_t stamp_ = 0;
    std::vector<std::string> names_;   // the world's component names by index, as last recorded
    std::deque<Delta> deltas_;
};

}  // namespace pocket::world
