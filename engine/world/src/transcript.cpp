#include <pocket/world/transcript.hpp>

#include <algorithm>
#include <cmath>
#include <map>
#include <set>
#include <sstream>

namespace pocket::world {

namespace {

std::string fmt(double v) {
    if (std::fabs(v - std::round(v)) < 1e-6) return std::to_string(static_cast<long long>(std::llround(v)));
    char buf[32];
    std::snprintf(buf, sizeof buf, "%.3f", v);
    std::string s = buf;
    while (!s.empty() && s.back() == '0') s.pop_back();
    if (!s.empty() && s.back() == '.') s.pop_back();
    return s;
}

// Flatten an exposed state into numeric series (booleans become 0/1; strings are tracked as
// discrete values by hashing their change into a separate key).
void numeric_view(const Json& state, std::map<std::string, double>& out, std::map<std::string, std::string>& strings) {
    if (!state.is_object()) return;
    for (auto& [k, v] : state.items()) {
        if (v.is_number()) out[k] = v.get<double>();
        else if (v.is_boolean()) out[k] = v.get<bool>() ? 1.0 : 0.0;
        else if (v.is_string()) strings[k] = v.get<std::string>();
        else if (!v.is_null()) strings[k] = v.dump();
    }
}

}  // namespace

Json Transcript::to_json() const {
    Json j;
    Json segs = Json::array();
    for (const auto& s : segments) {
        Json sj;
        sj["from"] = s.from;
        sj["to"] = s.to;
        sj["trends"] = s.trends;
        sj["events"] = s.events;
        segs.push_back(sj);
    }
    j["segments"] = segs;
    j["text"] = text;
    return j;
}

Transcript build_transcript(const std::vector<StateSample>& samples, const EventLog& events, const TranscriptOptions& o) {
    Transcript out;
    if (samples.empty()) {
        out.text = "no samples";
        return out;
    }
    std::int64_t first = std::max(o.since_tick, samples.front().tick);
    std::int64_t last = o.until_tick < 0 ? samples.back().tick : std::min(o.until_tick, samples.back().tick);
    if (last < first) {
        out.text = "empty range";
        return out;
    }
    // Index samples by tick.
    std::vector<const StateSample*> range;
    for (const auto& s : samples) {
        if (s.tick >= first && s.tick <= last) range.push_back(&s);
    }
    if (range.empty()) {
        out.text = "no samples in range";
        return out;
    }
    // Per-tick trend vector.
    std::set<std::string> keys;
    std::vector<std::map<std::string, double>> nums(range.size());
    std::vector<std::map<std::string, std::string>> strs(range.size());
    for (std::size_t i = 0; i < range.size(); ++i) {
        numeric_view(range[i]->state, nums[i], strs[i]);
        for (auto& [k, v] : nums[i]) keys.insert(k);
    }
    auto trend_at = [&](std::size_t i, const std::string& k) -> int {
        if (i == 0) return 0;
        auto a = nums[i - 1].find(k);
        auto b = nums[i].find(k);
        if (a == nums[i - 1].end() || b == nums[i].end()) return 0;
        double d = b->second - a->second;
        if (d > o.tolerance) return 1;
        if (d < -o.tolerance) return -1;
        return 0;
    };
    // Segment boundaries: a trend change for any key that persists for `debounce` ticks.
    std::vector<std::size_t> starts{0};
    std::map<std::string, int> current;
    for (const auto& k : keys) current[k] = trend_at(std::min<std::size_t>(1, range.size() - 1), k);
    for (std::size_t i = 1; i < range.size(); ++i) {
        bool cut = false;
        for (const auto& k : keys) {
            int t = trend_at(i, k);
            if (t == current[k]) continue;
            // Persisting?
            bool persists = true;
            for (int d = 1; d < o.debounce && i + static_cast<std::size_t>(d) < range.size(); ++d) {
                if (trend_at(i + static_cast<std::size_t>(d), k) != t) { persists = false; break; }
            }
            if (persists) {
                current[k] = t;
                cut = true;
            }
        }
        if (cut && i > starts.back()) starts.push_back(i);
    }
    starts.push_back(range.size());
    // Build segments.
    std::map<std::string, double> prev_end;
    for (std::size_t si = 0; si + 1 < starts.size(); ++si) {
        std::size_t a = starts[si], b = starts[si + 1] - 1;
        TranscriptSegment seg;
        seg.from = range[a]->tick;
        seg.to = range[b]->tick;
        seg.trends = Json::object();
        for (const auto& k : keys) {
            auto va = nums[a].find(k), vb = nums[b].find(k);
            if (va == nums[a].end() || vb == nums[b].end()) continue;
            double delta = vb->second - va->second;
            std::string trend = delta > o.tolerance ? "rising" : (delta < -o.tolerance ? "falling" : "constant");
            // Salience: skip constants that did not change since the previous segment.
            if (trend == "constant" && prev_end.contains(k) && std::fabs(prev_end[k] - va->second) <= o.tolerance) continue;
            Json t;
            t["trend"] = trend;
            t["from"] = va->second;
            t["to"] = vb->second;
            seg.trends[k] = t;
        }
        for (const auto& k : keys) {
            if (auto it = nums[b].find(k); it != nums[b].end()) prev_end[k] = it->second;
        }
        // String changes inside the segment.
        for (std::size_t i = a + 1; i <= b; ++i) {
            for (auto& [k, v] : strs[i]) {
                auto p = strs[i - 1].find(k);
                if (p == strs[i - 1].end() || p->second != v) {
                    Json t;
                    t["trend"] = "changed";
                    t["to"] = v;
                    t["tick"] = range[i]->tick;
                    seg.trends[k] = t;
                }
            }
        }
        // Events in [from, to].
        std::map<std::string, Json> by_type;
        for (const Event& e : events.since(0, 1000000)) {
            if (e.tick < seg.from || e.tick > seg.to) continue;
            Json& g = by_type[e.type];
            if (g.is_null()) {
                g = Json::object();
                g["count"] = 0;
                g["first"] = e.tick;
                g["subjects"] = Json::array();
            }
            g["count"] = g["count"].get<int>() + 1;
            g["last"] = e.tick;
            if (g["subjects"].size() < 4 && !e.data.is_null()) {
                std::string label;
                if (e.data.contains("path") && e.data["path"].is_string()) label = e.data["path"].get<std::string>();
                else if (e.data.contains("b") && e.data["b"].is_string()) label = e.data["b"].get<std::string>();
                if (!label.empty()) {
                    bool dup = false;
                    for (auto& s : g["subjects"]) if (s == label) dup = true;
                    if (!dup) g["subjects"].push_back(label);
                }
            }
        }
        seg.events = Json::object();
        for (auto& [k, v] : by_type) seg.events[k] = v;
        out.segments.push_back(seg);
    }
    // Coarsen to the line budget: merge the shortest adjacent segments (keeping event counts).
    auto merge = [&](std::size_t i) {
        TranscriptSegment& x = out.segments[i];
        TranscriptSegment& y = out.segments[i + 1];
        x.to = y.to;
        for (auto& [k, v] : y.trends.items()) {
            if (x.trends.contains(k) && x.trends[k].contains("from") && v.contains("to") && v["to"].is_number()) {
                double from = x.trends[k]["from"].get<double>(), to = v["to"].get<double>();
                x.trends[k]["to"] = to;
                double delta = to - from;
                x.trends[k]["trend"] = delta > o.tolerance ? "rising" : (delta < -o.tolerance ? "falling" : "varying");
            } else {
                x.trends[k] = v;
            }
        }
        for (auto& [k, v] : y.events.items()) {
            if (x.events.contains(k)) {
                x.events[k]["count"] = x.events[k]["count"].get<int>() + v["count"].get<int>();
                x.events[k]["last"] = v["last"];
            } else {
                x.events[k] = v;
            }
        }
        out.segments.erase(out.segments.begin() + static_cast<std::ptrdiff_t>(i) + 1);
    };
    int budget = std::max(o.max_lines - 1, 1);
    while (static_cast<int>(out.segments.size()) > budget && out.segments.size() > 1) {
        std::size_t shortest = 0;
        std::int64_t best = -1;
        for (std::size_t i = 0; i + 1 < out.segments.size(); ++i) {
            std::int64_t len = (out.segments[i].to - out.segments[i].from) + (out.segments[i + 1].to - out.segments[i + 1].from);
            if (best < 0 || len < best) { best = len; shortest = i; }
        }
        merge(shortest);
    }
    // Text.
    std::ostringstream text;
    text << "transcript ticks " << first << "-" << last << " (" << out.segments.size() << " segments)\n";
    for (const auto& s : out.segments) {
        text << "t" << s.from << "-" << s.to << " (" << (s.to - s.from + 1) << " ticks):";
        bool any = false;
        for (auto& [k, v] : s.trends.items()) {
            std::string trend = v["trend"].get<std::string>();
            text << (any ? ", " : " ") << k << " ";
            if (trend == "changed") text << "-> " << v["to"].dump();
            else if (trend == "constant") text << "= " << fmt(v["from"].get<double>());
            else text << trend << " " << fmt(v["from"].get<double>()) << "->" << fmt(v["to"].get<double>());
            any = true;
        }
        if (!any) text << " (no state change)";
        if (!s.events.empty()) {
            text << " | events:";
            for (auto& [k, v] : s.events.items()) {
                text << " " << k << "x" << v["count"].get<int>();
                if (v["count"].get<int>() == 1) text << "@t" << v["first"].get<std::int64_t>();
                if (v["subjects"].size() > 0) {
                    text << "(";
                    for (std::size_t i = 0; i < v["subjects"].size(); ++i) text << (i ? "," : "") << v["subjects"][i].get<std::string>();
                    text << ")";
                }
            }
        }
        text << "\n";
    }
    out.text = text.str();
    return out;
}

}  // namespace pocket::world
