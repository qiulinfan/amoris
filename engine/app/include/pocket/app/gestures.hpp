#pragma once

#include <pocket/core/core.hpp>
#include <pocket/platform/platform.hpp>

#include <cstdint>
#include <map>
#include <vector>

namespace pocket::app {

// Gestures from finger events, timed in ticks so they replay exactly: a tap (with a count for
// double taps), a long press, a swipe and a pinch. What comes out are input events
// ({type: "gesture", gesture, ...}) for scripts and `input.gesture` world events for agents.
class Gestures {
   public:
    struct Settings {
        bool enabled = true;
        float slop = 12.0f;           // points a finger may wander and still be a tap or a long press
        double tap_seconds = 0.3;     // down and up within this, still: a tap; a second one within this of the first: a double tap
        double hold_seconds = 0.5;    // down and still for this long: a long press
        float swipe_points = 40.0f;   // travelled this far and lifted within swipe_seconds: a swipe
        double swipe_seconds = 0.5;
    };

    // From a project's [input] table: gestures = false, gesture_slop, gesture_tap, gesture_hold,
    // gesture_swipe (points), gesture_swipe_seconds.
    void configure(const Json& input);
    // A batch of events at `tick` (`on_ui[i]` is the interface element event i landed on, 0 for
    // none); the gestures they complete come back as input events.
    std::vector<Json> feed(const std::vector<platform::Event>& events, const std::vector<std::uint64_t>& on_ui, std::int64_t tick, double tick_seconds);
    // Time passing without events at `tick`: long presses.
    std::vector<Json> tick(std::int64_t tick, double tick_seconds);
    [[nodiscard]] const Settings& settings() const { return settings_; }
    [[nodiscard]] int fingers() const { return static_cast<int>(fingers_.size()); }

   private:
    struct Finger {
        float x0 = 0, y0 = 0, x = 0, y = 0;
        std::int64_t down_tick = 0;
        bool moved = false;    // past the slop at some point
        bool held = false;     // reported as a long press
        bool pinched = false;  // took part in a pinch (no tap or swipe on lifting)
        std::uint64_t ui = 0;  // the interface element it went down on
    };
    Settings settings_;
    std::map<int, Finger> fingers_;
    std::int64_t last_tap_tick_ = -1000000;
    float last_tap_x_ = 0, last_tap_y_ = 0;
    int last_tap_count_ = 0;
    bool pinching_ = false;
    float pinch_d0_ = 1, pinch_a0_ = 0;
    Json pinch_json(const char* phase) const;
};

}  // namespace pocket::app
