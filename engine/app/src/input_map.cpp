#include <pocket/app/input_map.hpp>

#include <algorithm>
#include <cmath>

namespace pocket::app {

namespace {

// Pad sticks and triggers; the mouse's motion and wheel are axes too, read as deltas over a
// tick: a hundred pixels of motion (or one notch of the wheel) is full deflection.
bool is_axis(const std::string& s) {
    if (s == "mouse:x" || s == "mouse:y" || s == "wheel:x" || s == "wheel:y") return true;
    return s.starts_with("pad:") && (s.ends_with("x") || s.ends_with("y") || s.find("trigger") != std::string::npos) && s != "pad:x" && s != "pad:y";
}

bool is_delta(const std::string& s) { return s.starts_with("mouse:") || s.starts_with("wheel:"); }

constexpr float kMousePixelsPerUnit = 100.0f;

}  // namespace

Status InputMap::configure(const Json& actions) {
    if (!actions.is_object()) return fail("bad_args", "actions must be an object of name -> bindings");
    std::map<std::string, Action> next;
    for (auto& [name, spec] : actions.items()) {
        Action a;
        auto add = [&](const Json& v, float sign) -> Status {
            if (!v.is_string()) return fail("bad_args", "{}: bindings are strings such as \"Space\" or \"pad:a\"", name);
            std::string s = v.get<std::string>();
            if (s.empty()) return fail("bad_args", "{}: empty binding", name);
            if (is_axis(s)) a.axes.push_back({s, sign});
            else a.buttons.push_back({s, sign});
            return {};
        };
        if (spec.is_string()) {
            POCKET_TRY_VOID(add(spec, 1.0f));
        } else if (spec.is_array()) {
            for (auto& v : spec) POCKET_TRY_VOID(add(v, 1.0f));
        } else if (spec.is_object()) {
            for (auto& v : spec.value("positive", Json::array())) POCKET_TRY_VOID(add(v, 1.0f));
            for (auto& v : spec.value("negative", Json::array())) POCKET_TRY_VOID(add(v, -1.0f));
            for (auto& v : spec.value("axis", Json::array())) POCKET_TRY_VOID(add(v, 1.0f));
            for (auto& v : spec.value("keys", Json::array())) POCKET_TRY_VOID(add(v, 1.0f));
            if (spec.contains("deadzone") && spec["deadzone"].is_number()) a.deadzone = std::clamp(spec["deadzone"].get<float>(), 0.0f, 0.9f);
        } else {
            return fail("bad_args", "{}: a binding list or an object with positive/negative/axis", name);
        }
        if (a.buttons.empty() && a.axes.empty()) return fail("bad_args", "{}: no bindings", name);
        // Keep the live state of an action that already existed.
        if (auto it = actions_.find(name); it != actions_.end()) {
            a.active = it->second.active;
            a.axis_values = it->second.axis_values;
            a.down = it->second.down;
            a.value = it->second.value;
        }
        next[name] = std::move(a);
    }
    actions_ = std::move(next);
    for (auto& [n, a] : actions_) recompute(a);
    return {};
}

void InputMap::recompute(Action& a) {
    float value = 0;
    for (auto& [src, v] : a.active) value += v;
    float axis = 0;
    for (auto& [src, v] : a.axis_values) if (std::fabs(v) > std::fabs(axis)) axis = v;
    if (std::fabs(axis) < a.deadzone) axis = 0;
    else axis = (axis - std::copysign(a.deadzone, axis)) / (1.0f - a.deadzone);
    if (std::fabs(axis) > std::fabs(value)) value = axis;
    value = std::clamp(value, -1.0f, 1.0f);
    bool down = !a.active.empty() || std::fabs(axis) > 0.5f;
    if (down && !a.down) a.pressed = true;
    if (!down && a.down) a.released = true;
    a.down = down;
    a.value = value;
}

void InputMap::apply(const platform::Event& event) {
    using platform::EventType;
    std::string source;
    bool button = false, is_down = false;
    float axis_value = 0;
    bool axis = false;
    switch (event.type) {
        case EventType::KeyDown: if (event.repeat) return; source = event.key_name; button = true; is_down = true; break;
        case EventType::KeyUp: source = event.key_name; button = true; is_down = false; break;
        case EventType::PadButton: source = "pad:" + event.key_name; button = true; is_down = event.pressed; break;
        case EventType::PadAxis: source = "pad:" + event.key_name; axis = true; axis_value = event.value; break;
        case EventType::MouseMove:
            // Two sources at once, each accumulated over the tick; +y is down the screen.
            apply_delta("mouse:x", event.dx / kMousePixelsPerUnit);
            apply_delta("mouse:y", event.dy / kMousePixelsPerUnit);
            return;
        case EventType::MouseWheel:
            apply_delta("wheel:x", event.dx);
            apply_delta("wheel:y", event.dy);
            return;
        default: return;
    }
    for (auto& [name, a] : actions_) {
        bool touched = false;
        if (button) {
            for (const Binding& b : a.buttons) {
                if (b.source != source) continue;
                if (is_down) a.active[source] = b.sign;
                else a.active.erase(source);
                touched = true;
            }
        }
        if (axis) {
            for (const Binding& b : a.axes) {
                if (b.source != source) continue;
                a.axis_values[source] = axis_value * b.sign;
                touched = true;
            }
        }
        if (touched) recompute(a);
    }
}

void InputMap::apply_delta(const std::string& source, float amount) {
    if (amount == 0.0f) return;
    for (auto& [name, a] : actions_) {
        bool touched = false;
        for (const Binding& b : a.axes) {
            if (b.source != source) continue;
            a.axis_values[source] += amount * b.sign;
            touched = true;
        }
        if (touched) recompute(a);
    }
}

void InputMap::consume_edges() {
    for (auto& [name, a] : actions_) {
        a.pressed = false;
        a.released = false;
        // A delta source is spent by the tick that read it.
        bool touched = false;
        for (auto& [src, v] : a.axis_values) if (is_delta(src) && v != 0.0f) { v = 0.0f; touched = true; }
        if (touched) recompute(a);
    }
}

Json InputMap::snapshot() const {
    Json j = Json::object();
    for (const auto& [name, a] : actions_) {
        Json s;
        s["down"] = a.down;
        s["pressed"] = a.pressed;
        s["released"] = a.released;
        s["value"] = a.value;
        j[name] = s;
    }
    return j;
}

Json InputMap::describe() const {
    Json j = Json::object();
    for (const auto& [name, a] : actions_) {
        Json s;
        Json pos = Json::array(), neg = Json::array(), ax = Json::array();
        for (const Binding& b : a.buttons) (b.sign < 0 ? neg : pos).push_back(b.source);
        for (const Binding& b : a.axes) ax.push_back(b.source);
        s["positive"] = pos;
        if (!neg.empty()) s["negative"] = neg;
        if (!ax.empty()) { s["axis"] = ax; s["deadzone"] = a.deadzone; }
        j[name] = s;
    }
    return j;
}

bool InputMap::has_action(const std::string& name) const { return actions_.contains(name); }

std::vector<std::string> InputMap::keys_of(const std::string& name, int sign) const {
    std::vector<std::string> out;
    auto it = actions_.find(name);
    if (it == actions_.end()) return out;
    for (const Binding& b : it->second.buttons) {
        if (b.source.starts_with("pad:")) continue;
        if (sign == 0 || (sign > 0 && b.sign > 0) || (sign < 0 && b.sign < 0)) out.push_back(b.source);
    }
    return out;
}

}  // namespace pocket::app
