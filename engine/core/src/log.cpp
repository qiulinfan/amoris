#include <pocket/core/log.hpp>
#include <pocket/core/time.hpp>

#include <cstdio>
#include <fstream>
#include <memory>
#include <print>

namespace pocket::log {

std::string_view level_name(Level level) {
    switch (level) {
        case Level::Trace: return "trace";
        case Level::Debug: return "debug";
        case Level::Info: return "info";
        case Level::Warn: return "warn";
        case Level::Error: return "error";
        case Level::Off: return "off";
    }
    return "info";
}

Level level_from_name(std::string_view name) {
    if (name == "trace") return Level::Trace;
    if (name == "debug") return Level::Debug;
    if (name == "warn" || name == "warning") return Level::Warn;
    if (name == "error") return Level::Error;
    if (name == "off") return Level::Off;
    return Level::Info;
}

void Logger::set_level(Level level) {
    std::lock_guard lock(mutex_);
    level_ = level;
}

Level Logger::level() const {
    std::lock_guard lock(mutex_);
    return level_;
}

void Logger::add_sink(Sink sink) {
    std::lock_guard lock(mutex_);
    sinks_.push_back(std::move(sink));
}

void Logger::clear_sinks() {
    std::lock_guard lock(mutex_);
    sinks_.clear();
}

void Logger::set_ring_capacity(std::size_t records) {
    std::lock_guard lock(mutex_);
    ring_.clear();
    ring_next_ = 0;
    ring_capacity_ = records;
}

void Logger::set_tick(std::int64_t tick) {
    std::lock_guard lock(mutex_);
    tick_ = tick;
}

void Logger::emit(Level level, std::string_view category, std::string_view message, Json fields) {
    Record r;
    r.level = level;
    r.category = std::string(category);
    r.message = std::string(message);
    r.fields = std::move(fields);
    r.wall_ms = process_ms();
    std::vector<Sink> sinks;
    {
        std::lock_guard lock(mutex_);
        if (level < level_) return;
        r.seq = ++seq_;
        r.tick = tick_;
        if (ring_capacity_ > 0) {
            if (ring_.size() < ring_capacity_) {
                ring_.push_back(r);
            } else {
                ring_[ring_next_] = r;
                ring_next_ = (ring_next_ + 1) % ring_capacity_;
            }
        }
        sinks = sinks_;
    }
    for (auto& s : sinks) s(r);
}

std::vector<Record> Logger::recent(std::size_t max_records, Level min_level) const {
    std::lock_guard lock(mutex_);
    std::vector<Record> out;
    if (ring_.empty()) return out;
    // ring_ is either not yet full (ordered) or full with ring_next_ as the oldest.
    std::size_t n = ring_.size();
    std::size_t start = ring_.size() < ring_capacity_ ? 0 : ring_next_;
    for (std::size_t i = 0; i < n; ++i) {
        const Record& r = ring_[(start + i) % n];
        if (r.level >= min_level) out.push_back(r);
    }
    if (out.size() > max_records) out.erase(out.begin(), out.end() - static_cast<std::ptrdiff_t>(max_records));
    return out;
}

Logger& global() {
    static Logger logger;
    return logger;
}

Json to_json(const Record& r) {
    Json j;
    j["seq"] = r.seq;
    j["t_ms"] = r.wall_ms;
    if (r.tick >= 0) j["tick"] = r.tick;
    j["level"] = level_name(r.level);
    j["cat"] = r.category;
    j["msg"] = r.message;
    if (!r.fields.is_null()) j["fields"] = r.fields;
    return j;
}

Sink stderr_sink() {
    return [](const Record& r) {
        if (r.fields.is_null()) {
            std::println(stderr, "[{}] {}: {}", level_name(r.level), r.category, r.message);
        } else {
            std::println(stderr, "[{}] {}: {} {}", level_name(r.level), r.category, r.message, r.fields.dump());
        }
    };
}

Sink jsonl_file_sink(const std::string& path) {
    auto file = std::make_shared<std::ofstream>(path, std::ios::app);
    if (!file->is_open()) return [](const Record&) {};
    return [file](const Record& r) {
        *file << to_json(r).dump() << '\n';
        file->flush();
    };
}

}  // namespace pocket::log
