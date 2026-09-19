// Causal event log.
//
// Everything that happens in the world is an Event with a sequence number, the tick it happened
// on, a type, a subject entity, a JSON payload and an optional cause (the sequence number of the
// event that led to it). Agents read the log instead of watching frames; systems emit into it;
// scripts emit their own events with causes so that "why" is answerable after the fact.
#pragma once

#include <pocket/core/json.hpp>

#include <cstdint>
#include <deque>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::world {

struct Event {
    std::uint64_t seq = 0;
    std::int64_t tick = -1;
    std::string type;        // dotted, e.g. "entity.spawned", "script.bounce"
    std::uint64_t subject = 0;  // entity id or 0
    Json data;               // object or null
    std::uint64_t cause = 0; // seq of the causing event, 0 when unknown
    std::string source;      // "engine", "script", "agent"
};

Json event_to_json(const Event& e);

class EventLog {
   public:
    explicit EventLog(std::size_t capacity = 100000) : capacity_(capacity) {}

    std::uint64_t emit(std::int64_t tick, std::string_view type, std::uint64_t subject, Json data, std::uint64_t cause = 0, std::string_view source = "engine");
    // Events with seq > since, oldest first, at most limit. type_prefix filters "a.b" and "a.".
    [[nodiscard]] std::vector<Event> since(std::uint64_t since_seq, std::size_t limit = 1000, std::string_view type_prefix = {}) const;
    [[nodiscard]] std::vector<Event> recent(std::size_t n) const;
    [[nodiscard]] const Event* find(std::uint64_t seq) const;  // null when never emitted or evicted
    // The event and the chain of its causes, the event first; stops at an unknown cause or `limit`.
    [[nodiscard]] std::vector<Event> why(std::uint64_t seq, std::size_t limit = 32) const;
    [[nodiscard]] std::uint64_t last_seq() const { return next_seq_ - 1; }
    [[nodiscard]] std::uint64_t total() const { return next_seq_ - 1; }
    [[nodiscard]] std::size_t stored() const { return events_.size(); }
    // Counts per type since a sequence number (for the transcript and for reports).
    [[nodiscard]] Json histogram(std::uint64_t since_seq = 0) const;
    void clear();

   private:
    std::deque<Event> events_;
    std::size_t capacity_;
    std::uint64_t next_seq_ = 1;
};

}  // namespace pocket::world
