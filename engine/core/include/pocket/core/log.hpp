// Structured logging.
//
// Every log record has a level, a category, a message and optional structured fields. Sinks are
// pluggable; the default sink prints one human line per record to stderr, the JSONL sink writes one
// JSON object per line. Records are also kept in a bounded ring so that the runtime can return
// "what happened" to agents without them scraping a terminal.
#pragma once

#include <pocket/core/json.hpp>

#include <cstdint>
#include <format>
#include <functional>
#include <mutex>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::log {

enum class Level : std::uint8_t { Trace = 0, Debug = 1, Info = 2, Warn = 3, Error = 4, Off = 5 };

[[nodiscard]] std::string_view level_name(Level level);
[[nodiscard]] Level level_from_name(std::string_view name);

struct Record {
    Level level = Level::Info;
    std::string category;
    std::string message;
    Json fields;        // object or null
    std::uint64_t seq = 0;
    double wall_ms = 0; // milliseconds since process start (wall clock)
    std::int64_t tick = -1; // simulation tick if known
};

using Sink = std::function<void(const Record&)>;

struct Logger {
    void set_level(Level level);
    [[nodiscard]] Level level() const;
    void add_sink(Sink sink);
    void clear_sinks();
    void set_ring_capacity(std::size_t records);
    void set_tick(std::int64_t tick);

    void emit(Level level, std::string_view category, std::string_view message, Json fields = nullptr);
    [[nodiscard]] std::vector<Record> recent(std::size_t max_records, Level min_level = Level::Trace) const;
    [[nodiscard]] bool enabled(Level level) const { return level >= level_; }

   private:
    mutable std::mutex mutex_;
    Level level_ = Level::Info;
    std::vector<Sink> sinks_;
    std::vector<Record> ring_;
    std::size_t ring_capacity_ = 4096;
    std::size_t ring_next_ = 0;
    std::uint64_t seq_ = 0;
    std::int64_t tick_ = -1;
};

Logger& global();

// Human sink to stderr: "[level] category: message {fields}"
Sink stderr_sink();
// JSON lines sink to a file (appends). Returns a no-op sink when the file cannot be opened.
Sink jsonl_file_sink(const std::string& path);

Json to_json(const Record& record);

template <class... Args>
void trace(std::string_view category, std::format_string<Args...> fmt, Args&&... args) {
    if (global().enabled(Level::Trace)) global().emit(Level::Trace, category, std::format(fmt, std::forward<Args>(args)...));
}
template <class... Args>
void debug(std::string_view category, std::format_string<Args...> fmt, Args&&... args) {
    if (global().enabled(Level::Debug)) global().emit(Level::Debug, category, std::format(fmt, std::forward<Args>(args)...));
}
template <class... Args>
void info(std::string_view category, std::format_string<Args...> fmt, Args&&... args) {
    if (global().enabled(Level::Info)) global().emit(Level::Info, category, std::format(fmt, std::forward<Args>(args)...));
}
template <class... Args>
void warn(std::string_view category, std::format_string<Args...> fmt, Args&&... args) {
    if (global().enabled(Level::Warn)) global().emit(Level::Warn, category, std::format(fmt, std::forward<Args>(args)...));
}
template <class... Args>
void error(std::string_view category, std::format_string<Args...> fmt, Args&&... args) {
    if (global().enabled(Level::Error)) global().emit(Level::Error, category, std::format(fmt, std::forward<Args>(args)...));
}

}  // namespace pocket::log
