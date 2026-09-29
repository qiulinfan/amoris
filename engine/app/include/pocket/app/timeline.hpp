// Timelines (docs/design/timelines.md): a project file of keyed tracks that move the fields of
// entities' components over time, set others at moments, and fire events at times; the Timeline
// component plays one on the simulation clock, so a cutscene, a door's swing or a day's end is
// data an agent writes and reads rather than code.
#pragma once

#include <pocket/core/core.hpp>
#include <pocket/world/world.hpp>

#include <filesystem>
#include <map>
#include <string>
#include <vector>

namespace pocket::app {

struct TimelineKey {
    float time = 0;
    Json value;
    std::string ease;   // how it is reached from the key before: linear, step, or an easing name
};

struct TimelineTrack {
    std::string entity;      // a name or path; empty or "." for the Timeline's own entity
    std::string component;
    std::string field;       // a field path: "position", "position.y", "layers.0.weight"
    std::vector<TimelineKey> keys;   // in time order
};

struct TimelineEvent {
    float time = 0;
    std::string type;
    Json data;
};

struct TimelineFile {
    float duration = 0;      // the file's, else its last key's or event's time
    std::vector<TimelineTrack> tracks;
    std::vector<TimelineEvent> events;
};

// A timeline document read and checked for shape (keys, times, easing names); what it refers to
// in a world is checked by Timelines::check.
Result<TimelineFile> parse_timeline(const Json& doc, const std::string& display_path);

// The value of an easing at t (0..1); false for a name it does not know.
bool timeline_ease(std::string_view name, float t, float& out);

class Timelines {
   public:
    explicit Timelines(std::filesystem::path project_dir);

    // Every playing Timeline: its time advanced by dt times its speed, its tracks applied, the
    // events it passed emitted, timeline.finished at the end of one that does not loop.
    void step(world::World& w, float dt);
    // A timeline file and what is wrong with it against this world: the tracks' entities,
    // components and fields, and each track's values.
    Result<Json> info(const world::World& w, const std::string& path, world::EntityId self);
    // Files are read again when they change on disk; this reads every one again now.
    void forget();

   private:
    struct Cached {
        TimelineFile file;
        std::filesystem::file_time_type stamp{};
    };
    Result<const TimelineFile*> load(const std::string& path);
    std::filesystem::path project_dir_;
    std::map<std::string, Cached> files_;
};

}  // namespace pocket::app
