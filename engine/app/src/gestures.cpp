#include <pocket/app/gestures.hpp>

#include <algorithm>
#include <cmath>
#include <numbers>
#include <string>

namespace pocket::app {

namespace {

float distance(float ax, float ay, float bx, float by) { return std::hypot(bx - ax, by - ay); }

}  // namespace

void Gestures::configure(const Json& input) {
    if (!input.is_object()) return;
    if (input.contains("gestures") && input["gestures"].is_boolean()) settings_.enabled = input["gestures"].get<bool>();
    auto num = [&](const char* key, auto& field) {
        if (input.contains(key) && input[key].is_number()) field = static_cast<std::remove_reference_t<decltype(field)>>(input[key].get<double>());
    };
    num("gesture_slop", settings_.slop);
    num("gesture_tap", settings_.tap_seconds);
    num("gesture_hold", settings_.hold_seconds);
    num("gesture_swipe", settings_.swipe_points);
    num("gesture_swipe_seconds", settings_.swipe_seconds);
    num("gesture_edge", settings_.edge);
    settings_.slop = std::max(settings_.slop, 0.0f);
    settings_.tap_seconds = std::max(settings_.tap_seconds, 0.0);
    settings_.hold_seconds = std::max(settings_.hold_seconds, 0.0);
    settings_.swipe_points = std::max(settings_.swipe_points, 0.0f);
    settings_.swipe_seconds = std::max(settings_.swipe_seconds, 0.0);
    settings_.edge = std::max(settings_.edge, 0.0f);
}

// The two fingers of a pinch: the first two by index. The centre, the distance and the angle
// between them, the last two against the pinch's start.
Json Gestures::pinch_json(const char* phase) const {
    auto it = fingers_.begin();
    const Finger& a = it->second;
    ++it;
    const Finger& b = it->second;
    const float d = std::max(distance(a.x, a.y, b.x, b.y), 1e-3f);
    const float angle = std::atan2(b.y - a.y, b.x - a.x);
    float rotation = (angle - pinch_a0_) * 180.0f / std::numbers::pi_v<float>;
    while (rotation > 180.0f) rotation -= 360.0f;
    while (rotation <= -180.0f) rotation += 360.0f;
    Json j;
    j["type"] = "gesture";
    j["gesture"] = "pinch";
    j["phase"] = phase;
    j["x"] = (a.x + b.x) * 0.5f;
    j["y"] = (a.y + b.y) * 0.5f;
    j["scale"] = d / pinch_d0_;
    j["rotation"] = rotation;
    j["distance"] = d;
    if (a.ui) j["ui"] = a.ui;
    return j;
}

std::vector<Json> Gestures::feed(const std::vector<platform::Event>& events, const std::vector<std::uint64_t>& on_ui, std::int64_t tick, double tick_seconds) {
    std::vector<Json> out;
    if (!settings_.enabled) return out;
    for (std::size_t i = 0; i < events.size(); ++i) {
        const platform::Event& e = events[i];
        const int finger = e.pad;
        if (e.type == platform::EventType::TouchDown) {
            Finger f;
            f.x0 = f.x = e.x;
            f.y0 = f.y = e.y;
            f.down_tick = tick;
            f.ui = i < on_ui.size() ? on_ui[i] : 0;
            fingers_[finger] = f;
            if (fingers_.size() == 2 && !pinching_) {
                // Two fingers down: a pinch begins from their present spread and angle.
                pinching_ = true;
                auto it = fingers_.begin();
                Finger& a = it->second;
                ++it;
                Finger& b = it->second;
                a.pinched = b.pinched = true;
                pinch_d0_ = std::max(distance(a.x, a.y, b.x, b.y), 1e-3f);
                pinch_a0_ = std::atan2(b.y - a.y, b.x - a.x);
                out.push_back(pinch_json("begin"));
            }
        } else if (e.type == platform::EventType::TouchMove) {
            auto it = fingers_.find(finger);
            if (it == fingers_.end()) continue;
            Finger& f = it->second;
            f.x = e.x;
            f.y = e.y;
            if (distance(f.x0, f.y0, f.x, f.y) > settings_.slop) f.moved = true;
            if (pinching_ && fingers_.size() >= 2) out.push_back(pinch_json("move"));
        } else if (e.type == platform::EventType::TouchUp) {
            auto it = fingers_.find(finger);
            if (it == fingers_.end()) continue;
            Finger f = it->second;
            f.x = e.x;
            f.y = e.y;
            if (pinching_ && fingers_.size() >= 2) {
                it->second.x = e.x;
                it->second.y = e.y;
                out.push_back(pinch_json("end"));
                pinching_ = false;
            }
            fingers_.erase(it);
            if (f.pinched || f.held) continue;
            const double seconds = static_cast<double>(tick - f.down_tick) * tick_seconds;
            const float travelled = distance(f.x0, f.y0, f.x, f.y);
            const bool still = !f.moved && travelled <= settings_.slop;
            if (still && seconds <= settings_.tap_seconds) {
                // A tap; one right after another in the same place counts up (a double tap is count 2).
                const bool again = static_cast<double>(tick - last_tap_tick_) * tick_seconds <= settings_.tap_seconds && distance(last_tap_x_, last_tap_y_, f.x, f.y) <= settings_.slop * 2;
                const int count = again ? last_tap_count_ + 1 : 1;
                last_tap_tick_ = tick;
                last_tap_x_ = f.x;
                last_tap_y_ = f.y;
                last_tap_count_ = count;
                Json j;
                j["type"] = "gesture";
                j["gesture"] = "tap";
                j["finger"] = finger;
                j["x"] = f.x;
                j["y"] = f.y;
                j["count"] = count;
                if (f.ui) j["ui"] = f.ui;
                out.push_back(std::move(j));
            } else if (travelled >= settings_.swipe_points && seconds <= settings_.swipe_seconds) {
                const float dx = f.x - f.x0, dy = f.y - f.y0;
                Json j;
                j["type"] = "gesture";
                j["gesture"] = "swipe";
                j["finger"] = finger;
                j["x"] = f.x;
                j["y"] = f.y;
                j["dx"] = dx;
                j["dy"] = dy;
                const std::string direction = std::fabs(dx) >= std::fabs(dy) ? (dx > 0 ? "right" : "left") : (dy > 0 ? "down" : "up");
                j["direction"] = direction;
                j["seconds"] = seconds;
                // From a side of the view inwards: the edge it came from (a drawer, a back gesture).
                const float m = settings_.edge;
                if (m > 0 && view_w_ > 0 && view_h_ > 0) {
                    if (direction == "right" && f.x0 <= m) j["edge"] = "left";
                    else if (direction == "left" && f.x0 >= view_w_ - m) j["edge"] = "right";
                    else if (direction == "down" && f.y0 <= m) j["edge"] = "top";
                    else if (direction == "up" && f.y0 >= view_h_ - m) j["edge"] = "bottom";
                }
                if (f.ui) j["ui"] = f.ui;
                out.push_back(std::move(j));
            }
        }
    }
    return out;
}

std::vector<Json> Gestures::tick(std::int64_t tick, double tick_seconds) {
    std::vector<Json> out;
    if (!settings_.enabled) return out;
    for (auto& [finger, f] : fingers_) {
        if (f.held || f.moved || f.pinched) continue;
        if (static_cast<double>(tick - f.down_tick) * tick_seconds < settings_.hold_seconds) continue;
        f.held = true;
        Json j;
        j["type"] = "gesture";
        j["gesture"] = "long_press";
        j["finger"] = finger;
        j["x"] = f.x;
        j["y"] = f.y;
        if (f.ui) j["ui"] = f.ui;
        out.push_back(std::move(j));
    }
    return out;
}

}  // namespace pocket::app
