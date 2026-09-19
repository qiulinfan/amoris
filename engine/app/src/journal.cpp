#include "journal.hpp"

namespace pocket::app {

Result<Journal> Journal::open_for_replay(const std::filesystem::path& p) {
    POCKET_TRY(text, fs::read_text(p));
    Json j = Json::parse(text, nullptr, false);
    if (j.is_discarded() || j.value("format", "") != "pocket-journal") return fail("bad_journal", "{} is not a pocket journal", p.string());
    Journal out;
    out.replaying = true;
    out.path = p;
    out.frames = j.value("frames", Json::array());
    out.seed = j.value("seed", 0ULL);
    out.tick_rate = j.value("tick_rate", 60.0);
    return out;
}

Journal Journal::open_for_record(const std::filesystem::path& p, std::uint64_t seed, double tick_rate) {
    Journal out;
    out.recording = true;
    out.path = p;
    out.seed = seed;
    out.tick_rate = tick_rate;
    return out;
}

void Journal::record_frame(int ticks, const Json& events) {
    Json f;
    f["ticks"] = ticks;
    if (events.is_array() && !events.empty()) f["events"] = events;
    frames.push_back(f);
}

bool Journal::next_frame(int& ticks, Json& events) {
    if (cursor >= frames.size()) return false;
    const Json& f = frames[cursor++];
    ticks = f.value("ticks", 1);
    events = f.contains("events") ? f["events"] : Json::array();
    return true;
}

Status Journal::close() {
    if (!recording) return {};
    Json j;
    j["format"] = "pocket-journal";
    j["version"] = 1;
    j["seed"] = seed;
    j["tick_rate"] = tick_rate;
    j["frames"] = frames;
    return fs::write_text(path, j.dump() + "\n");
}

}  // namespace pocket::app
