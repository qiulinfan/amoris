// Window and input.
//
// The platform layer produces a normalized event stream (Event) so that input can be journaled
// and replayed independently of the OS. Headless mode is a first-class configuration: it creates
// no window, initializes no video driver, and yields no events.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>

#include <array>
#include <filesystem>
#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace pocket::platform {

struct Config {
    std::string title = "Pocket";
    int width = 1280;
    int height = 720;
    bool headless = false;
    bool visible = true;
    bool resizable = true;
    bool fullscreen = false;
};

enum class EventType : std::uint8_t { Quit, KeyDown, KeyUp, MouseMove, MouseDown, MouseUp, MouseWheel, Resize, Text, PadAdded, PadRemoved, PadButton, PadAxis, TouchDown, TouchUp, TouchMove };

struct Event {
    EventType type = EventType::Quit;
    int key = 0;             // scancode (layout independent)
    std::string key_name;    // human name of the scancode
    bool repeat = false;
    float x = 0, y = 0;      // mouse position in window points
    float dx = 0, dy = 0;    // motion delta / wheel delta
    int button = 0;          // 1 left, 2 middle, 3 right
    int width = 0, height = 0;  // pixel size for Resize
    std::string text;        // for Text
    int pad = 0;             // gamepad index (PadAdded/Removed/Button/Axis); key_name carries the button/axis name; the finger index for Touch events
    int mods = 0;            // modifier bits (kModShift | ...) for keys and mouse buttons
    int clicks = 1;          // MouseDown/Up: 2 for a double-click, 3 for a triple
    bool pressed = false;    // PadButton
    float value = 0;         // PadAxis, -1..1; a touch's pressure, 0..1
};

enum Mod : int { kModShift = 1, kModCtrl = 2, kModAlt = 4, kModMeta = 8 };
Json mods_to_json(int mods);              // ["shift", "meta"]
int mods_from_json(const Json& json);     // names array or bit number

const char* event_type_name(EventType type);
// Per-user writable directory for an application (SDL's preference path), created if needed.
std::filesystem::path user_data_dir(const std::string& org, const std::string& app);
Json event_to_json(const Event& event);
Event event_from_json(const Json& json);  // inverse of event_to_json (journals, synthetic input)

struct InputState {
    std::array<bool, 512> keys{};
    std::array<bool, 8> buttons{};
    float mouse_x = 0, mouse_y = 0;
    int pads = 0;  // connected gamepads
    int fingers = 0;  // fingers on the screen
};

class Platform {
   public:
    static Result<std::unique_ptr<Platform>> create(const Config& config);
    ~Platform();
    Platform(const Platform&) = delete;
    Platform& operator=(const Platform&) = delete;

    // Drain OS events. Updates input(). Quit events are also remembered in quit_requested().
    std::vector<Event> poll();
    [[nodiscard]] bool quit_requested() const;
    [[nodiscard]] const InputState& input() const;

    [[nodiscard]] bool headless() const;
    // What a GPU surface is made from: the window's CAMetalLayer on macOS, its X11 display and
    // window or its Wayland display and surface on Linux; all null when headless.
    struct NativeWindow {
        void* metal_layer = nullptr;
        void* x11_display = nullptr;
        std::uint64_t x11_window = 0;
        void* wayland_display = nullptr;
        void* wayland_surface = nullptr;
    };
    [[nodiscard]] NativeWindow native_window() const;
    [[nodiscard]] std::string canvas_selector() const;  // web: the window's <canvas>; empty elsewhere
    [[nodiscard]] int pixel_width() const;
    [[nodiscard]] int pixel_height() const;
    [[nodiscard]] float pixel_density() const;
    [[nodiscard]] Json describe() const;
    // Enable or disable OS text input (composition, Text events).
    void set_text_input(bool enabled);
    // Where the text being edited is, in window points: an IME opens its candidate window beside it.
    void set_text_input_area(int x, int y, int w, int h);
    [[nodiscard]] bool text_input() const;
    // The OS clipboard as text (empty when headless or when it holds no text).
    [[nodiscard]] std::string clipboard_text() const;
    void set_clipboard_text(const std::string& text);
    // Shake gamepad `pad` (its index) for `ms` milliseconds, the low and high motors in 0..1.
    // False when there is no such pad or it cannot rumble.
    bool rumble(int pad, float low, float high, int ms);
    // The pointer. `locked` captures it: hidden, held in the window, motion as unbounded deltas.
    // Escape lets it go and the next click in the window takes it again (in a browser the page's
    // pointer lock does the same). `visible` false hides an uncaptured pointer. Headless, the
    // request is only remembered.
    void set_cursor(bool locked, bool visible);
    struct Cursor {
        bool locked = false;    // asked for
        bool held = false;      // captured now
        bool visible = true;
    };
    [[nodiscard]] Cursor cursor() const;
    // The window: fullscreen (the desktop's resolution; in a browser the page's fullscreen, which
    // the browser grants only during a click or a key press), its size in points when windowed,
    // its title. Headless, the requests are only remembered.
    void set_fullscreen(bool on);
    void set_window_size(int width, int height);
    void set_title(const std::string& title);
    struct WindowState {
        int width = 0, height = 0;   // points
        bool fullscreen = false;
        std::string title;
    };
    [[nodiscard]] WindowState window_state() const;

   private:
    Platform();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::platform
