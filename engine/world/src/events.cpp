#include <pocket/world/events.hpp>

#include <algorithm>
#include <map>

namespace pocket::world {

Json event_to_json(const Event& e) {
    Json j;
    j["seq"] = e.seq;
    j["tick"] = e.tick;
    j["type"] = e.type;
    if (e.subject != 0) j["subject"] = e.subject;
    if (!e.data.is_null()) j["data"] = e.data;
    if (e.cause != 0) j["cause"] = e.cause;
    j["source"] = e.source;
    return j;
}

std::uint64_t EventLog::emit(std::int64_t tick, std::string_view type, std::uint64_t subject, Json data, std::uint64_t cause, std::string_view source) {
    Event e;
    e.seq = next_seq_++;
    e.tick = tick;
    e.type = std::string(type);
    e.subject = subject;
    e.data = std::move(data);
    e.cause = cause;
    e.source = std::string(source);
    events_.push_back(std::move(e));
    while (events_.size() > capacity_) events_.pop_front();
    return next_seq_ - 1;
}

// The stored events have consecutive sequence numbers (find() relies on it too): the first one
// after `seq` is found by its offset, not by walking a log of up to a hundred thousand.
std::size_t EventLog::index_after(std::uint64_t seq) const {
    if (events_.empty()) return 0;
    const std::uint64_t first = events_.front().seq;
    if (seq < first) return 0;
    return static_cast<std::size_t>(std::min<std::uint64_t>(seq - first + 1, events_.size()));
}

std::vector<Event> EventLog::since(std::uint64_t since_seq, std::size_t limit, std::string_view type_prefix) const {
    std::vector<Event> out;
    for (std::size_t i = index_after(since_seq); i < events_.size(); ++i) {
        const Event& e = events_[i];
        if (!type_prefix.empty() && e.type.compare(0, type_prefix.size(), type_prefix) != 0) continue;
        out.push_back(e);
        if (out.size() >= limit) break;
    }
    return out;
}

std::vector<Event> EventLog::recent(std::size_t n) const {
    std::vector<Event> out;
    std::size_t start = events_.size() > n ? events_.size() - n : 0;
    for (std::size_t i = start; i < events_.size(); ++i) out.push_back(events_[i]);
    return out;
}

const Event* EventLog::find(std::uint64_t seq) const {
    if (events_.empty() || seq == 0) return nullptr;
    std::uint64_t first = events_.front().seq;
    if (seq < first || seq > events_.back().seq) return nullptr;
    const Event& e = events_[static_cast<std::size_t>(seq - first)];
    return e.seq == seq ? &e : nullptr;
}

std::vector<Event> EventLog::why(std::uint64_t seq, std::size_t limit) const {
    std::vector<Event> chain;
    const Event* e = find(seq);
    while (e && chain.size() < limit) {
        chain.push_back(*e);
        e = e->cause != 0 ? find(e->cause) : nullptr;
    }
    return chain;
}

Json EventLog::histogram(std::uint64_t since_seq) const {
    std::map<std::string, std::uint64_t> counts;
    for (std::size_t i = index_after(since_seq); i < events_.size(); ++i) counts[events_[i].type]++;
    Json j = Json::object();
    for (auto& [k, v] : counts) j[k] = v;
    return j;
}

void EventLog::clear() {
    events_.clear();
}

}  // namespace pocket::world
