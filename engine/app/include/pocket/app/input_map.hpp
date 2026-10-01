// Action maps: named actions bound to keys, gamepad buttons and axes, with per-frame edges.
//
// Gameplay reads actions ("jump", "move_x"), not scancodes, so a project can be played with a
// keyboard or a pad, an agent can hold an action for N ticks through `input.hold`, and the
// same snapshot is part of every tick a script sees.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>
#include <pocket/platform/platform.hpp>

#include <map>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::app {

// A mouse button's binding name ("mouse:left" for 1, middle 2, right 3, x1 4, x2 5; null otherwise),
// and back (0 for anything that is not one).
const char* mouse_button_source(int button);
int mouse_button_of(std::string_view source);

class InputMap {
   public:
    // {"jump": ["Space", "pad:a"], "move_x": {"negative": ["A"], "positive": ["D"], "axis": ["pad:leftx"], "deadzone": 0.15}}
    Status configure(const Json& actions);
    void apply(const platform::Event& event);
    // Clear pressed/released edges; call after a tick has seen them, so edges are never lost
    // between frames and are true for exactly one tick.
    void consume_edges();
    // {"jump": {"down": true, "pressed": true, "released": false, "value": 1.0}, ...}
    [[nodiscard]] Json snapshot() const;
    [[nodiscard]] Json describe() const;
    [[nodiscard]] bool has_action(const std::string& name) const;
    // Bindings of an action that are keys (for synthetic holds).
    // sign +1: positive keys, -1: negative keys, 0: all keys.
    [[nodiscard]] std::vector<std::string> keys_of(const std::string& name, int sign = 1) const;
    [[nodiscard]] std::size_t size() const { return actions_.size(); }

   private:
    struct Binding {
        std::string source;  // "Space", "pad:a", "pad:leftx", "mouse:x", "wheel:y"
        float sign = 1.0f;   // +1 positive, -1 negative
    };
    struct Action {
        std::vector<Binding> buttons;  // keys and pad buttons
        std::vector<Binding> axes;     // pad axes, and the mouse's motion and wheel (deltas over the tick)
        float deadzone = 0.15f;
        // State
        std::map<std::string, float> active;  // source -> contribution while held
        std::map<std::string, float> axis_values;
        bool down = false, pressed = false, released = false;
        float value = 0;
    };
    void recompute(Action& a);
    void apply_delta(const std::string& source, float amount);
    std::map<std::string, Action> actions_;
};

}  // namespace pocket::app
