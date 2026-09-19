#include <pocket/world/events.hpp>

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

std::vector<Event> EventLog::since(std::uint64_t since_seq, std::size_t limit, std::string_view type_prefix) const {
    std::vector<Event> out;
    for (const Event& e : events_) {
        if (e.seq <= since_seq) continue;
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

Json EventLog::histogram(std::uint64_t since_seq) const {
    std::map<std::string, std::uint64_t> counts;
    for (const Event& e : events_) {
        if (e.seq > since_seq) counts[e.type]++;
    }
    Json j = Json::object();
    for (auto& [k, v] : counts) j[k] = v;
    return j;
}

void EventLog::clear() {
    events_.clear();
}

}  // namespace pocket::world
