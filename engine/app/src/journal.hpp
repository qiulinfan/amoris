// Input journal: records what the platform delivered each frame so a run can be replayed headless
// with the same ticks and the same input, and therefore the same state hash.
#pragma once

#include <pocket/core/core.hpp>

#include <filesystem>
#include <vector>

namespace pocket::app {

struct Journal {
    bool recording = false;
    bool replaying = false;
    std::filesystem::path path;
    Json frames = Json::array();  // recorded: appended; replayed: read
    std::size_t cursor = 0;
    std::uint64_t seed = 0;
    double tick_rate = 60.0;

    static Result<Journal> open_for_replay(const std::filesystem::path& p);
    static Journal open_for_record(const std::filesystem::path& p, std::uint64_t seed, double tick_rate);
    void record_frame(int ticks, const Json& events);
    // Next replayed frame; false at the end.
    bool next_frame(int& ticks, Json& events);
    Status close();
};

}  // namespace pocket::app
