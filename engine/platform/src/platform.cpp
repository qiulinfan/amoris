#include <pocket/platform/platform.hpp>

#include <algorithm>
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
    }
    return "unknown";
}

Event event_from_json(const Json& j) {
    Event e;
    std::string t = j.is_object() ? j.value("type", "quit") : "quit";
    e.type = t == "key_down" ? EventType::KeyDown : t == "key_up" ? EventType::KeyUp : t == "mouse_move" ? EventType::MouseMove : t == "mouse_down" ? EventType::MouseDown : t == "mouse_up" ? EventType::MouseUp : t == "mouse_wheel" ? EventType::MouseWheel : t == "resize" ? EventType::Resize : t == "text" ? EventType::Text : t == "pad_added" ? EventType::PadAdded : t == "pad_removed" ? EventType::PadRemoved : t == "pad_button" ? EventType::PadButton : t == "pad_axis" ? EventType::PadAxis : EventType::Quit;
    if (!j.is_object()) return e;
    e.key_name = j.contains("key") && j["key"].is_string() ? j["key"].get<std::string>() : "";
    e.key = j.contains("code") && j["code"].is_number() ? j["code"].get<int>() : 0;
    e.repeat = j.contains("repeat") && j["repeat"].is_boolean() && j["repeat"].get<bool>();
    e.mods = j.contains("mods") ? mods_from_json(j["mods"]) : 0;
    auto num = [&](const char* k) -> float { return j.contains(k) && j[k].is_number() ? j[k].get<float>() : 0.0f; };
    e.x = num("x"); e.y = num("y"); e.dx = num("dx"); e.dy = num("dy");
    e.button = j.contains("button") && j["button"].is_number() ? j["button"].get<int>() : 0;
    e.width = static_cast<int>(num("width")); e.height = static_cast<int>(num("height"));
    e.text = j.contains("text") && j["text"].is_string() ? j["text"].get<std::string>() : "";
    e.pad = j.contains("pad") && j["pad"].is_number() ? j["pad"].get<int>() : 0;
    e.pressed = j.contains("pressed") && j["pressed"].is_boolean() && j["pressed"].get<bool>();
    e.value = num("value");
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
    SDL_MetalView metal_view = nullptr;
    void* layer = nullptr;
    bool sdl_initialized = false;
    bool quit = false;
    InputState input;
    int pixel_w = 0, pixel_h = 0;
    bool text_input = false;
    std::map<SDL_JoystickID, SDL_Gamepad*> pads;  // open gamepads by instance id
    int pad_index(SDL_JoystickID id) const {
        int i = 0;
        for (auto& [jid, pad] : pads) { if (jid == id) return i; ++i; }
        return 0;
    }

    ~Impl() {
#ifndef __EMSCRIPTEN__
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
    if (config.headless) {
        p->impl_->pixel_w = config.width;
        p->impl_->pixel_h = config.height;
        log::debug("platform", "headless, no window");
        return p;
    }
    SDL_SetHint(SDL_HINT_VIDEO_ALLOW_SCREENSAVER, "1");
    if (!SDL_Init(SDL_INIT_VIDEO | SDL_INIT_GAMEPAD)) {
        return fail("platform_init_failed", "SDL_Init failed: {}", SDL_GetError());
    }
    p->impl_->sdl_initialized = true;
#ifdef __EMSCRIPTEN__
    // The page sizes the canvas (CSS); SDL follows it and reports resizes, so the window is
    // always resizable in the browser.
    SDL_WindowFlags flags = SDL_WINDOW_HIGH_PIXEL_DENSITY | SDL_WINDOW_RESIZABLE;
#else
    SDL_WindowFlags flags = SDL_WINDOW_METAL | SDL_WINDOW_HIGH_PIXEL_DENSITY;
#endif
    if (config.resizable) flags |= SDL_WINDOW_RESIZABLE;
    if (!config.visible) flags |= SDL_WINDOW_HIDDEN;
    SDL_Window* w = SDL_CreateWindow(config.title.c_str(), config.width, config.height, flags);
    if (!w) return fail("window_create_failed", "SDL_CreateWindow failed: {}", SDL_GetError());
    p->impl_->window = w;
#ifndef __EMSCRIPTEN__
    p->impl_->metal_view = SDL_Metal_CreateView(w);
    if (!p->impl_->metal_view) return fail("metal_view_failed", "SDL_Metal_CreateView failed: {}", SDL_GetError());
    p->impl_->layer = SDL_Metal_GetLayer(p->impl_->metal_view);
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
                out.push_back(ev);
                break;
            }
            case SDL_EVENT_MOUSE_MOTION:
                ev.type = EventType::MouseMove;
                ev.x = e.motion.x; ev.y = e.motion.y; ev.dx = e.motion.xrel; ev.dy = e.motion.yrel;
                impl_->input.mouse_x = ev.x; impl_->input.mouse_y = ev.y;
                out.push_back(ev);
                break;
            case SDL_EVENT_MOUSE_BUTTON_DOWN:
            case SDL_EVENT_MOUSE_BUTTON_UP:
                ev.type = e.type == SDL_EVENT_MOUSE_BUTTON_DOWN ? EventType::MouseDown : EventType::MouseUp;
                ev.button = e.button.button; ev.x = e.button.x; ev.y = e.button.y;
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
            default:
                break;
        }
    }
    return out;
}

bool Platform::quit_requested() const { return impl_->quit; }
const InputState& Platform::input() const { return impl_->input; }
bool Platform::headless() const { return impl_->config.headless; }
void* Platform::metal_layer() const { return impl_->layer; }
std::string Platform::canvas_selector() const {
#ifdef __EMSCRIPTEN__
    return impl_->window ? "#canvas" : "";
#else
    return "";
#endif
}
int Platform::pixel_width() const { return impl_->pixel_w; }
int Platform::pixel_height() const { return impl_->pixel_h; }
float Platform::pixel_density() const { return impl_->window ? SDL_GetWindowPixelDensity(impl_->window) : 1.0f; }

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
