// Gameplay transcript: the run compressed into stable regimes and grouped events.
//
// Agents should not read ticks; they should read "the ball fell for 45 ticks, bounced twice,
// then rested" and "three enemies spawned, one hit the player at tick 233". The transcript
// segments the exposed numeric state into runs where every value keeps its trend (rising,
// falling, constant), debounced so noise does not fragment it, and lists the events of each
// segment run-length compressed by type. Output is text with a line budget.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/world/events.hpp>

#include <cstdint>
#include <string>
#include <vector>

namespace pocket::world {

struct StateSample {
    std::int64_t tick = 0;
    Json state;  // the exposed state after that tick
};

struct TranscriptOptions {
    std::int64_t since_tick = 0;
    std::int64_t until_tick = -1;  // -1: latest
    int max_lines = 40;
    double tolerance = 1e-4;       // |delta| below this counts as constant
    int debounce = 3;              // ticks a new trend must persist to open a segment
};

struct TranscriptSegment {
    std::int64_t from = 0, to = 0;
    Json trends;   // key -> {"trend": "rising"|"falling"|"constant", "from": v, "to": v}
    Json events;   // type -> {"count": n, "first": tick, "last": tick, "subjects": [...]}
};

struct Transcript {
    std::vector<TranscriptSegment> segments;
    std::string text;
    Json to_json() const;
};

Transcript build_transcript(const std::vector<StateSample>& samples, const EventLog& events, const TranscriptOptions& options);

}  // namespace pocket::world
