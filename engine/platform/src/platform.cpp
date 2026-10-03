#include <pocket/platform/platform.hpp>

#include <algorithm>
#include <cmath>
#include <cstdlib>
#include <map>

#include <pocket/core/log.hpp>

#include <SDL3/SDL.h>
#ifdef __EMSCRIPTEN__
#include <emscripten.h>
// clang-format off
EM_JS(void, pocket_web_text_input, (int enabled), {
    if (typeof Module !== "undefined" && typeof Module.pocketTextInput === "function") Module.pocketTextInput(!!enabled);
});
EM_JS(int, pocket_web_pointer_locked, (), {
    return document.pointerLockElement ? 1 : 0;
});
// clang-format on
#endif

namespace pocket::platform {

const char* event_type_name(EventType type) {
    switch (type) {
        case EventType::Quit: return "quit";
        case EventType::KeyDown: return "key_down";
        case EventType::KeyUp: return "key_up";
        case EventType::MouseMove: return "mouse_move";
        case EventType::MouseDown: return "mouse_down";
        case EventType::MouseUp: return "mouse_up";
        case EventType::MouseWheel: return "mouse_wheel";
        case EventType::Resize: return "resize";
        case EventType::Text: return "text";
        case EventType::PadAdded: return "pad_added";
        case EventType::PadRemoved: return "pad_removed";
        case EventType::PadButton: return "pad_button";
        case EventType::PadAxis: return "pad_axis";
        case EventType::TouchDown: return "touch_down";
        case EventType::TouchUp: return "touch_up";
        case EventType::TouchMove: return "touch_move";
    }
    return "unknown";
}

Event event_from_json(const Json& j) {
    Event e;
    std::string t = j.is_object() ? j.value("type", "quit") : "quit";
    e.type = t == "key_down" ? EventType::KeyDown : t == "key_up" ? EventType::KeyUp : t == "mouse_move" ? EventType::MouseMove : t == "mouse_down" ? EventType::MouseDown : t == "mouse_up" ? EventType::MouseUp : t == "mouse_wheel" ? EventType::MouseWheel : t == "resize" ? EventType::Resize : t == "text" ? EventType::Text : t == "pad_added" ? EventType::PadAdded : t == "pad_removed" ? EventType::PadRemoved : t == "pad_button" ? EventType::PadButton : t == "pad_axis" ? EventType::PadAxis : EventType::Quit;
    if (t == "touch_down") e.type = EventType::TouchDown;
    else if (t == "touch_up") e.type = EventType::TouchUp;
    else if (t == "touch_move") e.type = EventType::TouchMove;
    if (!j.is_object()) return e;
    if (j.contains("finger") && j["finger"].is_number()) e.pad = j["finger"].get<int>();
    e.key_name = j.contains("key") && j["key"].is_string() ? j["key"].get<std::string>() : "";
    e.key = j.contains("code") && j["code"].is_number() ? j["code"].get<int>() : 0;
    e.repeat = j.contains("repeat") && j["repeat"].is_boolean() && j["repeat"].get<bool>();
    e.mods = j.contains("mods") ? mods_from_json(j["mods"]) : 0;
    auto num = [&](const char* k) -> float { return j.contains(k) && j[k].is_number() ? j[k].get<float>() : 0.0f; };
    e.x = num("x"); e.y = num("y"); e.dx = num("dx"); e.dy = num("dy");
    e.button = j.contains("button") && j["button"].is_number() ? j["button"].get<int>() : 0;
    e.clicks = j.contains("clicks") && j["clicks"].is_number() ? std::max(1, j["clicks"].get<int>()) : 1;
    e.width = static_cast<int>(num("width")); e.height = static_cast<int>(num("height"));
    e.text = j.contains("text") && j["text"].is_string() ? j["text"].get<std::string>() : "";
    if (j.contains("pad") && j["pad"].is_number()) e.pad = j["pad"].get<int>();
    e.pressed = j.contains("pressed") && j["pressed"].is_boolean() && j["pressed"].get<bool>();
    e.value = num("value");
    if (e.type == EventType::TouchDown || e.type == EventType::TouchUp || e.type == EventType::TouchMove) {
        // A finger without a pressure (a journal from before it was kept) presses fully while down.
        e.value = j.contains("pressure") && j["pressure"].is_number() ? j["pressure"].get<float>() : e.type == EventType::TouchUp ? 0.0f : 1.0f;
    }
    if (j.contains("button") && j["button"].is_string()) e.key_name = j["button"].get<std::string>();
    if (j.contains("axis") && j["axis"].is_string()) e.key_name = j["axis"].get<std::string>();
    if (j.contains("name") && j["name"].is_string()) e.text = j["name"].get<std::string>();
    return e;
}

std::filesystem::path user_data_dir(const std::string& org, const std::string& app) {
    if (char* p = SDL_GetPrefPath(org.c_str(), app.c_str())) {
        std::filesystem::path out(p);
        SDL_free(p);
        return out;
    }
    const char* home = std::getenv("HOME");
    std::filesystem::path base = home ? std::filesystem::path(home) / ".local" / "share" : std::filesystem::temp_directory_path();
    std::filesystem::path out = base / org / app;
    std::error_code ec;
    std::filesystem::create_directories(out, ec);
    return out;
}

Json mods_to_json(int mods) {
    Json j = Json::array();
    if (mods & kModShift) j.push_back("shift");
    if (mods & kModCtrl) j.push_back("ctrl");
    if (mods & kModAlt) j.push_back("alt");
    if (mods & kModMeta) j.push_back("meta");
    return j;
}

int mods_from_json(const Json& j) {
    if (j.is_number()) return j.get<int>();
    int out = 0;
    if (j.is_array()) {
        for (const auto& m : j) {
            if (!m.is_string()) continue;
            std::string s = m.get<std::string>();
            if (s == "shift") out |= kModShift;
            else if (s == "ctrl" || s == "control") out |= kModCtrl;
            else if (s == "alt" || s == "option") out |= kModAlt;
            else if (s == "meta" || s == "cmd" || s == "super" || s == "gui") out |= kModMeta;
        }
    }
    return out;
}

namespace {
int mods_from_sdl(SDL_Keymod m) {
    int out = 0;
    if (m & SDL_KMOD_SHIFT) out |= kModShift;
    if (m & SDL_KMOD_CTRL) out |= kModCtrl;
    if (m & SDL_KMOD_ALT) out |= kModAlt;
    if (m & SDL_KMOD_GUI) out |= kModMeta;
    return out;
}
}  // namespace

Json event_to_json(const Event& e) {
    Json j;
    j["type"] = event_type_name(e.type);
    if (e.mods && (e.type == EventType::KeyDown || e.type == EventType::KeyUp || e.type == EventType::MouseDown || e.type == EventType::MouseUp)) j["mods"] = mods_to_json(e.mods);
    switch (e.type) {
        case EventType::KeyDown:
        case EventType::KeyUp:
            j["key"] = e.key_name;
            j["code"] = e.key;
            if (e.repeat) j["repeat"] = true;
            break;
        case EventType::MouseMove:
            j["x"] = e.x; j["y"] = e.y; j["dx"] = e.dx; j["dy"] = e.dy;
            break;
        case EventType::MouseDown:
        case EventType::MouseUp:
            j["button"] = e.button; j["x"] = e.x; j["y"] = e.y;
            if (e.clicks > 1) j["clicks"] = e.clicks;
            break;
        case EventType::MouseWheel:
            j["dx"] = e.dx; j["dy"] = e.dy;
            break;
        case EventType::Resize:
            j["width"] = e.width; j["height"] = e.height;
            break;
        case EventType::Text:
            j["text"] = e.text;
            break;
        case EventType::PadAdded:
        case EventType::PadRemoved:
            j["pad"] = e.pad; j["name"] = e.text;
            break;
        case EventType::PadButton:
            j["pad"] = e.pad; j["button"] = e.key_name; j["pressed"] = e.pressed;
            break;
        case EventType::TouchDown:
        case EventType::TouchUp:
        case EventType::TouchMove:
            j["finger"] = e.pad; j["x"] = e.x; j["y"] = e.y; j["dx"] = e.dx; j["dy"] = e.dy; j["pressure"] = e.value;
            break;
        case EventType::PadAxis:
            j["pad"] = e.pad; j["axis"] = e.key_name; j["value"] = e.value;
            break;
        case EventType::Quit:
            break;
    }
    return j;
}

struct Platform::Impl {
    Config config;
    SDL_Window* window = nullptr;
#ifdef __APPLE__
    SDL_MetalView metal_view = nullptr;
#endif
    Platform::NativeWindow native;
    bool sdl_initialized = false;
    bool quit = false;
    InputState input;
    std::vector<SDL_FingerID> fingers;   // active fingers by index (0 is the one that acts as the mouse); 0 when a slot is free
    int finger_slot(SDL_FingerID id, bool take) {
        for (std::size_t i = 0; i < fingers.size(); ++i) if (fingers[i] == id) return static_cast<int>(i);
        if (!take) return -1;
        for (std::size_t i = 0; i < fingers.size(); ++i) if (fingers[i] == 0) { fingers[i] = id; return static_cast<int>(i); }
        fingers.push_back(id);
        return static_cast<int>(fingers.size() - 1);
    }
    int pixel_w = 0, pixel_h = 0;
    // SDL's window coordinates per point (the unit the engine's windows, pointers and UI are in):
    // 1 where SDL counts points (macOS, Linux, the web), the display's scale on Windows, where it
    // counts pixels (a 960x540 window on a 200% display is 1920x1080 there, as on a Retina Mac).
    float units = 1.0f;
    float to_points(float v) const { return v / units; }
    int to_units(int v) const { return static_cast<int>(std::lround(static_cast<float>(v) * units)); }
    Platform::WindowState asked;   // what was asked of the window (all there is headless)
    bool text_input = false;
    bool cursor_locked = false, cursor_visible = true;
    bool cursor_let_go = false;   // Escape released a locked pointer; a click takes it again
    std::map<SDL_JoystickID, SDL_Gamepad*> pads;  // open gamepads by instance id
    int pad_index(SDL_JoystickID id) const {
        int i = 0;
        for (auto& [jid, pad] : pads) { if (jid == id) return i; ++i; }
        return 0;
    }

    ~Impl() {
#ifdef __APPLE__
        if (metal_view) SDL_Metal_DestroyView(metal_view);
#endif
        if (window) SDL_DestroyWindow(window);
        for (auto& [id, pad] : pads) SDL_CloseGamepad(pad);
        pads.clear();
        if (sdl_initialized) SDL_Quit();
    }
};

Platform::Platform() : impl_(std::make_unique<Impl>()) {}
Platform::~Platform() = default;

Result<std::unique_ptr<Platform>> Platform::create(const Config& config) {
    std::unique_ptr<Platform> p(new Platform());
    p->impl_->config = config;
    p->impl_->asked = WindowState{config.width, config.height, config.fullscreen, config.title};
    if (config.headless) {
        p->impl_->pixel_w = config.width;
        p->impl_->pixel_h = config.height;
        log::debug("platform", "headless, no window");
        return p;
    }
    SDL_SetHint(SDL_HINT_VIDEO_ALLOW_SCREENSAVER, "1");
    // Touch reaches the engine as its own events; the first finger is turned into mouse events
    // here (below), not by SDL, so the stream is one thing to record and replay.
    SDL_SetHint(SDL_HINT_TOUCH_MOUSE_EVENTS, "0");
    SDL_SetHint(SDL_HINT_MOUSE_TOUCH_EVENTS, "0");
    if (!SDL_Init(SDL_INIT_VIDEO | SDL_INIT_GAMEPAD)) {
        return fail("platform_init_failed", "SDL_Init failed: {}", SDL_GetError());
    }
    p->impl_->sdl_initialized = true;
#ifdef __EMSCRIPTEN__
    // The page sizes the canvas (CSS); SDL follows it and reports resizes, so the window is
    // always resizable in the browser.
    SDL_WindowFlags flags = SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_RESIZABLE;
#elif defined(__APPLE__)
    SDL_WindowFlags flags = SDL_WINDOW_METAL | SDL_WINDOW_HIGH_PIXEL_DENSITY;
#elif defined(_WIN32)
    SDL_WindowFlags flags = SDL_WINDOW_HIGH_PIXEL_DENSITY;   // Direct3D 12 through the window's HWND
#else
    SDL_WindowFlags flags = SDL_WINDOW_HIGH_PIXEL_DENSITY;   // Vulkan through the window's X11 or Wayland handles
#endif
    if (config.resizable) flags |= SDL_WINDOW_RESIZABLE;
    if (!config.visible) flags |= SDL_WINDOW_HIDDEN;
    if (config.fullscreen) flags |= SDL_WINDOW_FULLSCREEN;
#if defined(_WIN32)
    if (const float s = SDL_GetDisplayContentScale(SDL_GetPrimaryDisplay()); s > 0) p->impl_->units = s;
#endif
    SDL_Window* w = SDL_CreateWindow(config.title.c_str(), p->impl_->to_units(config.width), p->impl_->to_units(config.height), flags);
    if (!w) return fail("window_create_failed", "SDL_CreateWindow failed: {}", SDL_GetError());
    p->impl_->window = w;
#if defined(_WIN32)
    if (const float s = SDL_GetWindowDisplayScale(w); s > 0) p->impl_->units = s;
#endif
#if defined(__APPLE__)
    p->impl_->metal_view = SDL_Metal_CreateView(w);
    if (!p->impl_->metal_view) return fail("metal_view_failed", "SDL_Metal_CreateView failed: {}", SDL_GetError());
    p->impl_->native.metal_layer = SDL_Metal_GetLayer(p->impl_->metal_view);
#elif defined(_WIN32)
    // Direct3D 12 presents into the window's HWND (docs/decisions/0008-windows.md).
    const SDL_PropertiesID props = SDL_GetWindowProperties(w);
    p->impl_->native.win32_hwnd = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WIN32_HWND_POINTER, nullptr);
    p->impl_->native.win32_hinstance = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WIN32_INSTANCE_POINTER, nullptr);
    if (!p->impl_->native.win32_hwnd) return fail("window_handles_missing", "the window has no HWND (video driver {})", SDL_GetCurrentVideoDriver());
#elif !defined(__EMSCRIPTEN__)
    const SDL_PropertiesID props = SDL_GetWindowProperties(w);
    p->impl_->native.wayland_display = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_DISPLAY_POINTER, nullptr);
    p->impl_->native.wayland_surface = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_WAYLAND_SURFACE_POINTER, nullptr);
    p->impl_->native.x11_display = SDL_GetPointerProperty(props, SDL_PROP_WINDOW_X11_DISPLAY_POINTER, nullptr);
    p->impl_->native.x11_window = static_cast<std::uint64_t>(SDL_GetNumberProperty(props, SDL_PROP_WINDOW_X11_WINDOW_NUMBER, 0));
    if (!p->impl_->native.wayland_surface && !p->impl_->native.x11_window) return fail("window_handles_missing", "the window has neither a Wayland surface nor an X11 window (video driver {})", SDL_GetCurrentVideoDriver());
#endif
    SDL_GetWindowSizeInPixels(w, &p->impl_->pixel_w, &p->impl_->pixel_h);
    log::info("platform", "window {}x{} points, {}x{} pixels, driver {}", config.width, config.height, p->impl_->pixel_w, p->impl_->pixel_h, SDL_GetCurrentVideoDriver());
    return p;
}

std::vector<Event> Platform::poll() {
    std::vector<Event> out;
    if (impl_->config.headless) return out;
    SDL_Event e;
    while (SDL_PollEvent(&e)) {
        Event ev;
        switch (e.type) {
            case SDL_EVENT_QUIT:
                impl_->quit = true;
                ev.type = EventType::Quit;
                out.push_back(ev);
                break;
            case SDL_EVENT_KEY_DOWN:
            case SDL_EVENT_KEY_UP: {
                ev.type = e.type == SDL_EVENT_KEY_DOWN ? EventType::KeyDown : EventType::KeyUp;
                ev.key = static_cast<int>(e.key.scancode);
                ev.key_name = SDL_GetScancodeName(e.key.scancode);
                ev.repeat = e.key.repeat;
                ev.mods = mods_from_sdl(e.key.mod);
                if (ev.key >= 0 && ev.key < static_cast<int>(impl_->input.keys.size())) {
                    impl_->input.keys[static_cast<std::size_t>(ev.key)] = e.type == SDL_EVENT_KEY_DOWN;
                }
#ifndef __EMSCRIPTEN__
                // Escape lets a locked pointer go (a browser does this itself).
                if (e.type == SDL_EVENT_KEY_DOWN && e.key.scancode == SDL_SCANCODE_ESCAPE && impl_->cursor_locked && !impl_->cursor_let_go) {
                    impl_->cursor_let_go = true;
                    SDL_SetWindowRelativeMouseMode(impl_->window, false);
                }
#endif
                out.push_back(ev);
                break;
            }
            case SDL_EVENT_MOUSE_MOTION:
                ev.type = EventType::MouseMove;
                ev.x = impl_->to_points(e.motion.x); ev.y = impl_->to_points(e.motion.y);
                ev.dx = impl_->to_points(e.motion.xrel); ev.dy = impl_->to_points(e.motion.yrel);
                impl_->input.mouse_x = ev.x; impl_->input.mouse_y = ev.y;
                out.push_back(ev);
                break;
            case SDL_EVENT_MOUSE_BUTTON_DOWN:
            case SDL_EVENT_MOUSE_BUTTON_UP:
                ev.type = e.type == SDL_EVENT_MOUSE_BUTTON_DOWN ? EventType::MouseDown : EventType::MouseUp;
#ifndef __EMSCRIPTEN__
                // A click takes a pointer Escape let go back (in a browser SDL asks for the lock again).
                if (e.type == SDL_EVENT_MOUSE_BUTTON_DOWN && impl_->cursor_locked && impl_->cursor_let_go) {
                    impl_->cursor_let_go = false;
                    SDL_SetWindowRelativeMouseMode(impl_->window, true);
                }
#endif
                ev.button = e.button.button; ev.x = impl_->to_points(e.button.x); ev.y = impl_->to_points(e.button.y);
                ev.clicks = std::max(1, static_cast<int>(e.button.clicks));
                ev.mods = mods_from_sdl(SDL_GetModState());
                if (ev.button >= 0 && ev.button < static_cast<int>(impl_->input.buttons.size())) {
                    impl_->input.buttons[static_cast<std::size_t>(ev.button)] = e.type == SDL_EVENT_MOUSE_BUTTON_DOWN;
                }
                out.push_back(ev);
                break;
            case SDL_EVENT_MOUSE_WHEEL:
                ev.type = EventType::MouseWheel;
                ev.dx = e.wheel.x; ev.dy = e.wheel.y;
                out.push_back(ev);
                break;
            case SDL_EVENT_WINDOW_DISPLAY_SCALE_CHANGED:
#if defined(_WIN32)
                // Moved to a display with another scale: SDL resizes the window to keep its points.
                if (const float s = SDL_GetWindowDisplayScale(impl_->window); s > 0) impl_->units = s;
#endif
                break;
            case SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED:
                ev.type = EventType::Resize;
                ev.width = e.window.data1; ev.height = e.window.data2;
                impl_->pixel_w = ev.width; impl_->pixel_h = ev.height;
                out.push_back(ev);
                break;
            case SDL_EVENT_TEXT_INPUT:
#ifndef __EMSCRIPTEN__
                // On the web the page delivers text (see set_text_input); SDL's keypress path would double it.
                ev.type = EventType::Text;
                ev.text = e.text.text;
                out.push_back(ev);
#endif
                break;
            case SDL_EVENT_GAMEPAD_ADDED: {
                SDL_Gamepad* pad = SDL_OpenGamepad(e.gdevice.which);
                if (!pad) break;
                impl_->pads[e.gdevice.which] = pad;
                impl_->input.pads = static_cast<int>(impl_->pads.size());
                ev.type = EventType::PadAdded;
                ev.pad = impl_->pad_index(e.gdevice.which);
                const char* name = SDL_GetGamepadName(pad);
                ev.text = name ? name : "gamepad";
                log::info("platform", "gamepad {} connected: {}", ev.pad, ev.text);
                out.push_back(ev);
                break;
            }
            case SDL_EVENT_GAMEPAD_REMOVED: {
                auto it = impl_->pads.find(e.gdevice.which);
                if (it == impl_->pads.end()) break;
                ev.type = EventType::PadRemoved;
                ev.pad = impl_->pad_index(e.gdevice.which);
                SDL_CloseGamepad(it->second);
                impl_->pads.erase(it);
                impl_->input.pads = static_cast<int>(impl_->pads.size());
                out.push_back(ev);
                break;
            }
            case SDL_EVENT_GAMEPAD_BUTTON_DOWN:
            case SDL_EVENT_GAMEPAD_BUTTON_UP: {
                ev.type = EventType::PadButton;
                ev.pad = impl_->pad_index(e.gbutton.which);
                const char* name = SDL_GetGamepadStringForButton(static_cast<SDL_GamepadButton>(e.gbutton.button));
                ev.key_name = name ? name : "unknown";
                ev.pressed = e.type == SDL_EVENT_GAMEPAD_BUTTON_DOWN;
                out.push_back(ev);
                break;
            }
            case SDL_EVENT_GAMEPAD_AXIS_MOTION: {
                ev.type = EventType::PadAxis;
                ev.pad = impl_->pad_index(e.gaxis.which);
                const char* name = SDL_GetGamepadStringForAxis(static_cast<SDL_GamepadAxis>(e.gaxis.axis));
                ev.key_name = name ? name : "unknown";
                ev.value = std::clamp(static_cast<float>(e.gaxis.value) / 32767.0f, -1.0f, 1.0f);
                out.push_back(ev);
                break;
            }
            case SDL_EVENT_FINGER_DOWN:
            case SDL_EVENT_FINGER_UP:
            case SDL_EVENT_FINGER_MOTION:
            case SDL_EVENT_FINGER_CANCELED: {
                // Fingers by index in window points; the first one also acts as the mouse.
                const bool down = e.type == SDL_EVENT_FINGER_DOWN, up = e.type == SDL_EVENT_FINGER_UP || e.type == SDL_EVENT_FINGER_CANCELED;
                const int slot = impl_->finger_slot(e.tfinger.fingerID, down);
                if (slot < 0) break;
                int w = 0, h = 0;
                SDL_GetWindowSize(impl_->window, &w, &h);
                const float pw = impl_->to_points(static_cast<float>(w)), ph = impl_->to_points(static_cast<float>(h));
                ev.type = down ? EventType::TouchDown : up ? EventType::TouchUp : EventType::TouchMove;
                ev.pad = slot;
                ev.x = e.tfinger.x * pw;
                ev.y = e.tfinger.y * ph;
                ev.dx = e.tfinger.dx * pw;
                ev.dy = e.tfinger.dy * ph;
                ev.value = std::clamp(e.tfinger.pressure, 0.0f, 1.0f);
                ev.pressed = !up;
                if (down) impl_->input.fingers++;
                if (up) { impl_->input.fingers = std::max(0, impl_->input.fingers - 1); impl_->fingers[static_cast<std::size_t>(slot)] = 0; }
                out.push_back(ev);
                if (slot == 0) {
                    Event m;
                    m.type = down ? EventType::MouseDown : up ? EventType::MouseUp : EventType::MouseMove;
                    m.button = 1;
                    m.x = ev.x; m.y = ev.y; m.dx = ev.dx; m.dy = ev.dy;
                    m.mods = mods_from_sdl(SDL_GetModState());
                    impl_->input.mouse_x = m.x; impl_->input.mouse_y = m.y;
                    if (down || up) impl_->input.buttons[1] = down;
                    out.push_back(m);
                }
                break;
            }
            default:
                break;
        }
    }
    return out;
}

bool Platform::quit_requested() const { return impl_->quit; }
const InputState& Platform::input() const { return impl_->input; }
bool Platform::headless() const { return impl_->config.headless; }
Platform::NativeWindow Platform::native_window() const { return impl_->native; }
std::string Platform::canvas_selector() const {
#ifdef __EMSCRIPTEN__
    return impl_->window ? "#canvas" : "";
#else
    return "";
#endif
}
int Platform::pixel_width() const { return impl_->pixel_w; }
int Platform::pixel_height() const { return impl_->pixel_h; }
float Platform::pixel_density() const {
    if (!impl_->window) return 1.0f;
#if defined(_WIN32)
    return impl_->units;   // SDL counts pixels: its density is 1, and the display's scale is the ratio
#else
    return SDL_GetWindowPixelDensity(impl_->window);
#endif
}

void Platform::set_text_input(bool enabled) {
    if (impl_->text_input == enabled) return;
    impl_->text_input = enabled;
    if (!impl_->window) return;
    if (enabled) SDL_StartTextInput(impl_->window);
    else SDL_StopTextInput(impl_->window);
#ifdef __EMSCRIPTEN__
    // The page's hidden text field takes over: it receives what the keyboard, an IME or a phone's
    // keyboard produce and hands it to the runtime through ui.type (docs/web.md).
    pocket_web_text_input(enabled ? 1 : 0);
#endif
}

bool Platform::text_input() const { return impl_->text_input; }

void Platform::set_text_input_area(int x, int y, int w, int h) {
    if (!impl_->window) return;
    const SDL_Rect r{impl_->to_units(x), impl_->to_units(y), std::max(1, impl_->to_units(w)), std::max(1, impl_->to_units(h))};
    SDL_SetTextInputArea(impl_->window, &r, 0);
}

void Platform::set_cursor(bool locked, bool visible) {
    impl_->cursor_locked = locked;
    impl_->cursor_visible = visible;
    impl_->cursor_let_go = false;
    if (!impl_->window) return;
    SDL_SetWindowRelativeMouseMode(impl_->window, locked);
    if (visible) SDL_ShowCursor();
    else SDL_HideCursor();
}

Platform::Cursor Platform::cursor() const {
    Cursor c;
    c.locked = impl_->cursor_locked;
    c.visible = impl_->cursor_visible;
#ifdef __EMSCRIPTEN__
    c.held = impl_->window && c.locked && pocket_web_pointer_locked() != 0;
#else
    c.held = impl_->window && c.locked && !impl_->cursor_let_go && SDL_GetWindowRelativeMouseMode(impl_->window);
#endif
    return c;
}

bool Platform::rumble(int pad, float low, float high, int ms) {
    int i = 0;
    for (auto& [jid, gp] : impl_->pads) {
        if (i++ != pad) continue;
        const auto motor = [](float v) { return static_cast<Uint16>(std::clamp(v, 0.0f, 1.0f) * 65535.0f); };
        return SDL_RumbleGamepad(gp, motor(low), motor(high), static_cast<Uint32>(std::max(0, ms)));
    }
    return false;
}

std::string Platform::clipboard_text() const {
    if (impl_->config.headless) return {};
    char* text = SDL_GetClipboardText();
    std::string out = text ? text : "";
    SDL_free(text);
    return out;
}

void Platform::set_clipboard_text(const std::string& text) {
    if (impl_->config.headless) return;
    SDL_SetClipboardText(text.c_str());
}

void Platform::set_fullscreen(bool on) {
    impl_->asked.fullscreen = on;
    if (impl_->window) SDL_SetWindowFullscreen(impl_->window, on);
}

void Platform::set_window_size(int width, int height) {
    impl_->asked.width = width;
    impl_->asked.height = height;
    if (impl_->window) SDL_SetWindowSize(impl_->window, impl_->to_units(width), impl_->to_units(height));
}

void Platform::set_title(const std::string& title) {
    impl_->asked.title = title;
    if (impl_->window) SDL_SetWindowTitle(impl_->window, title.c_str());
}

Platform::WindowState Platform::window_state() const {
    WindowState s = impl_->asked;
    if (impl_->window) {
        SDL_GetWindowSize(impl_->window, &s.width, &s.height);
        s.width = static_cast<int>(std::lround(impl_->to_points(static_cast<float>(s.width))));
        s.height = static_cast<int>(std::lround(impl_->to_points(static_cast<float>(s.height))));
        s.fullscreen = (SDL_GetWindowFlags(impl_->window) & SDL_WINDOW_FULLSCREEN) != 0;
        const char* t = SDL_GetWindowTitle(impl_->window);
        s.title = t ? t : "";
    }
    return s;
}

Json Platform::describe() const {
    Json j;
    j["headless"] = impl_->config.headless;
    j["pixel_width"] = impl_->pixel_w;
    j["pixel_height"] = impl_->pixel_h;
    if (!impl_->config.headless) {
        j["driver"] = SDL_GetCurrentVideoDriver() ? SDL_GetCurrentVideoDriver() : "";
        j["sdl"] = std::format("{}.{}.{}", SDL_MAJOR_VERSION, SDL_MINOR_VERSION, SDL_MICRO_VERSION);
    }
    return j;
}

}  // namespace pocket::platform
