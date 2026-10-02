#include <pocket/app/session.hpp>
#include <pocket/core/hash.hpp>
#include <pocket/world/component_list.gen.hpp>

#include "command_help.hpp"
#include "journal.hpp"
#include "behavior.hpp"
#include "gif.hpp"
#include "ragdoll.hpp"
#include "web_fs.hpp"
#include <pocket/world/water.hpp>
#include <pocket/world/wind.hpp>

#include <stb_image_write.h>

#include <algorithm>
#include <charconv>
#include <cctype>
#include <chrono>
#include <cstdlib>
#include <cstring>
#include <format>
#include <limits>
#include <map>
#include <numbers>
#include <functional>
#include <set>
#include <unordered_map>
#include <thread>

#if !defined(__EMSCRIPTEN__) && !defined(_WIN32)
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>
extern char** environ;
#endif

namespace pocket::app {

namespace {

// What a hold presses: a key, or a mouse button, which a hold presses away from the interface (it
// presses the action bound to it; ui.click is what clicks an element).
platform::Event press_event(const std::string& key, bool down) {
    platform::Event e;
    if (const int b = mouse_button_of(key)) {
        e.type = down ? platform::EventType::MouseDown : platform::EventType::MouseUp;
        e.button = b;
        e.x = e.y = -1;
    } else {
        e.type = down ? platform::EventType::KeyDown : platform::EventType::KeyUp;
        e.key_name = key;
    }
    return e;
}

Status write_png(const std::filesystem::path& path, const rhi::Image& img) {
    if (path.has_parent_path()) POCKET_TRY_VOID(fs::ensure_dir(path.parent_path()));
    int ok = stbi_write_png(path.string().c_str(), static_cast<int>(img.width), static_cast<int>(img.height), 4, img.rgba.data(), static_cast<int>(img.width * 4));
    if (!ok) return fail("io_error", "cannot write PNG {}", path.string());
    return {};
}

// A PNG in memory, and base64 for handing one over in JSON.
std::string png_bytes(const rhi::Image& img) {
    std::string out;
    stbi_write_png_to_func([](void* ctx, void* data, int size) { static_cast<std::string*>(ctx)->append(static_cast<const char*>(data), static_cast<std::size_t>(size)); },
                           &out, static_cast<int>(img.width), static_cast<int>(img.height), 4, img.rgba.data(), static_cast<int>(img.width * 4));
    return out;
}

std::string base64(const std::string& bytes) {
    static constexpr char kAlphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string out;
    out.reserve((bytes.size() + 2) / 3 * 4);
    for (std::size_t i = 0; i < bytes.size(); i += 3) {
        const std::uint32_t n = (static_cast<std::uint8_t>(bytes[i]) << 16) | (i + 1 < bytes.size() ? static_cast<std::uint8_t>(bytes[i + 1]) << 8 : 0) | (i + 2 < bytes.size() ? static_cast<std::uint8_t>(bytes[i + 2]) : 0);
        out += kAlphabet[(n >> 18) & 63];
        out += kAlphabet[(n >> 12) & 63];
        out += i + 1 < bytes.size() ? kAlphabet[(n >> 6) & 63] : '=';
        out += i + 2 < bytes.size() ? kAlphabet[n & 63] : '=';
    }
    return out;
}

// The tick's systems as perf names them, in Session::System's order.
constexpr const char* kSystemNames[] = {"timelines", "paths", "bodies", "characters", "water", "contacts", "tiles_2d", "bodies_2d", "hits", "behaviors", "navigation", "cameras", "world", "particles", "animation", "ragdolls", "attachments", "cloth", "audio", "interface", "recorder"};

Json error_json(const Error& e) {
    Json j;
    j["code"] = e.code;
    j["message"] = e.message;
    if (!e.detail.empty()) j["detail"] = e.detail;
    return j;
}

template <class T>
T opt(const Json& p, const char* key, T fallback) {
    if (!p.is_object() || !p.contains(key)) return fallback;
    const Json& v = p[key];
    if constexpr (std::is_same_v<T, std::string>) {
        return v.is_string() ? v.get<std::string>() : fallback;
    } else if constexpr (std::is_same_v<T, bool>) {
        return v.is_boolean() ? v.get<bool>() : fallback;
    } else {
        return v.is_number() ? v.get<T>() : fallback;
    }
}

// A bus's settings with the fields `p` gives over `s` (audio.bus, [audio.buses]); duck_by "" or null stops ducking.
audio::BusSettings bus_settings(audio::BusSettings s, const Json& p) {
    s.volume = static_cast<float>(opt<double>(p, "volume", s.volume));
    s.muted = opt<bool>(p, "muted", s.muted);
    s.lowpass = static_cast<float>(opt<double>(p, "lowpass", s.lowpass));
    s.highpass = static_cast<float>(opt<double>(p, "highpass", s.highpass));
    s.echo = static_cast<float>(opt<double>(p, "echo", s.echo));
    s.echo_feedback = static_cast<float>(opt<double>(p, "echo_feedback", s.echo_feedback));
    s.echo_mix = static_cast<float>(opt<double>(p, "echo_mix", s.echo_mix));
    s.reverb = static_cast<float>(opt<double>(p, "reverb", s.reverb));
    if (p.contains("duck_by")) s.duck_by = p["duck_by"].is_string() ? p["duck_by"].get<std::string>() : std::string();
    s.duck_amount = static_cast<float>(opt<double>(p, "duck_amount", s.duck_amount));
    s.duck_seconds = static_cast<float>(opt<double>(p, "duck_seconds", s.duck_seconds));
    return s;
}

// A color for the grade's tint: {r, g, b}, [r, g, b] or "#rrggbb" (also "#rgb").
bool read_tint(const Json& v, rhi::Color& out) {
    if (v.is_object()) {
        out = {opt<float>(v, "r", out.r), opt<float>(v, "g", out.g), opt<float>(v, "b", out.b), 1.0f};
        return true;
    }
    if (v.is_array() && v.size() >= 3 && v[0].is_number() && v[1].is_number() && v[2].is_number()) {
        out = {v[0].get<float>(), v[1].get<float>(), v[2].get<float>(), 1.0f};
        return true;
    }
    if (v.is_string()) {
        std::string s = v.get<std::string>();
        if (!s.empty() && s[0] == '#') s.erase(0, 1);
        if (s.size() == 3) s = {s[0], s[0], s[1], s[1], s[2], s[2]};
        if (s.size() != 6) return false;
        float c[3];
        for (int i = 0; i < 3; ++i) {
            char* end = nullptr;
            const std::string part = s.substr(static_cast<std::size_t>(i) * 2, 2);
            const long n = std::strtol(part.c_str(), &end, 16);
            if (end == nullptr || *end != '\0') return false;
            c[i] = static_cast<float>(n) / 255.0f;
        }
        out = {c[0], c[1], c[2], 1.0f};
        return true;
    }
    return false;
}

// The grade's fields from a command's params or a project's [render.grade] table.
void read_grade(renderer::GradeSettings& g, const Json& p) {
    g.exposure = opt<float>(p, "exposure", g.exposure);
    g.filmic = opt<bool>(p, "filmic", g.filmic);
    g.temperature = opt<float>(p, "temperature", g.temperature);
    g.contrast = opt<float>(p, "contrast", g.contrast);
    g.saturation = opt<float>(p, "saturation", g.saturation);
    g.vignette = opt<float>(p, "vignette", g.vignette);
    if (p.is_object() && p.contains("tint")) read_tint(p["tint"], g.tint);
    g.lut = opt<std::string>(p, "lut", g.lut);
    g.lut_strength = opt<float>(p, "lut_strength", g.lut_strength);
}

// The tone-mapping settings from a command's params or a project's [render.tonemap] table; false
// with `why` set when the operator is not one of the names.
bool read_tonemap(renderer::TonemapSettings& t, const Json& p, std::string& why) {
    if (p.is_object() && p.contains("operator")) {
        if (!p["operator"].is_string() || !renderer::tonemap_from_name(p["operator"].get<std::string>(), t.op)) {
            why = "operator is none, aces, agx or neutral";
            return false;
        }
    }
    t.exposure = opt<float>(p, "exposure", t.exposure);
    t.auto_exposure = opt<bool>(p, "auto_exposure", t.auto_exposure);
    t.compensation = opt<float>(p, "compensation", t.compensation);
    t.min_ev = opt<float>(p, "min_ev", t.min_ev);
    t.max_ev = opt<float>(p, "max_ev", t.max_ev);
    t.speed = opt<float>(p, "speed", t.speed);
    return true;
}

Json tonemap_json(const renderer::TonemapSettings& t) {
    return Json{{"operator", renderer::tonemap_name(t.op)}, {"exposure", t.exposure}, {"auto_exposure", t.auto_exposure}, {"compensation", t.compensation}, {"min_ev", t.min_ev}, {"max_ev", t.max_ev}, {"speed", t.speed}};
}

Json grade_json(const renderer::GradeSettings& g) {
    return Json{{"enabled", g.enabled}, {"exposure", g.exposure}, {"filmic", g.filmic}, {"temperature", g.temperature}, {"contrast", g.contrast}, {"saturation", g.saturation}, {"tint", Json{{"r", g.tint.r}, {"g", g.tint.g}, {"b", g.tint.b}}}, {"vignette", g.vignette}, {"lut", g.lut}, {"lut_strength", g.lut_strength}};
}

std::vector<std::string> string_list(const Json& p, const char* key) {
    std::vector<std::string> out;
    if (!p.is_object() || !p.contains(key)) return out;
    const Json& v = p[key];
    if (v.is_string()) {
        out.push_back(v.get<std::string>());
    } else if (v.is_array()) {
        for (const auto& x : v) if (x.is_string()) out.push_back(x.get<std::string>());
    }
    return out;
}

}  // namespace

Session::Session(const Options& options) : options_(options) {}
Session::~Session() = default;

void Session::record_error(const Error& e) {
    Error mapped = e;
    mapped.message = source_lines(e.message);
    mapped.detail = source_lines(e.detail);
    log::error("runtime", "{}", mapped.to_string());
    errors_.push_back(error_json(mapped));
}

void Session::read_bundle_lines(const std::filesystem::path& bundle) {
    const std::string url = bundle.string();
    std::erase_if(bundle_lines_, [&](const BundleLines& b) { return b.url == url; });
    auto text = fs::read_text(std::filesystem::path(url + ".lines.json"));
    if (!text) return;   // a bundle made by hand or by an older tool: its own lines are shown
    const Json j = Json::parse(*text, nullptr, false);
    if (!j.is_object() || !j.contains("modules") || !j["modules"].is_array()) return;
    std::error_code ec;
    const auto project = std::filesystem::weakly_canonical(options_.project_dir, ec);
    BundleLines b;
    b.url = url;
    for (const Json& m : j["modules"]) {
        if (!m.is_object() || !m.contains("lines") || !m["lines"].is_array()) continue;
        BundleLines::Module mod;
        const std::filesystem::path source = m.value("source", std::string());
        const auto rel = source.lexically_relative(project);
        mod.source = !project.empty() && !rel.empty() && *rel.begin() != ".." ? rel.generic_string() : source.generic_string();
        mod.first = m.value("first", std::size_t{0});
        mod.lines = m["lines"].get<std::vector<std::uint32_t>>();
        b.modules.push_back(std::move(mod));
    }
    bundle_lines_.push_back(std::move(b));
}

std::string Session::source_lines(std::string_view text) const {
    std::string out;
    std::size_t from = 0;
    while (from < text.size()) {
        // The earliest place a bundle is named, followed by :<line>.
        std::size_t best = std::string_view::npos;
        const BundleLines* in = nullptr;
        for (const BundleLines& b : bundle_lines_) {
            const std::size_t at = text.find(b.url, from);
            if (at != std::string_view::npos && (best == std::string_view::npos || at < best)) {
                best = at;
                in = &b;
            }
        }
        if (!in) break;
        std::size_t i = best + in->url.size();
        auto digits = [&](std::size_t& k) {
            std::size_t n = 0;
            const std::size_t start = k;
            while (k < text.size() && text[k] >= '0' && text[k] <= '9') n = n * 10 + static_cast<std::size_t>(text[k++] - '0');
            return k > start ? n : std::string_view::npos;
        };
        const BundleLines::Module* mod = nullptr;
        std::size_t line = 0, end = i;
        if (i < text.size() && text[i] == ':') {
            std::size_t k = i + 1;
            line = digits(k);
            if (line != std::string_view::npos) {
                end = k;
                if (k < text.size() && text[k] == ':') {
                    std::size_t c = k + 1;
                    if (digits(c) != std::string_view::npos) end = c;
                }
                for (const auto& m : in->modules) {
                    if (line >= m.first && line < m.first + m.lines.size()) mod = &m;
                }
            }
        }
        out.append(text.substr(from, best - from));
        if (mod) out += std::format("{}:{}", mod->source, mod->lines[line - mod->first]);
        else out.append(text.substr(best, end - best));   // the bundle's own lines (its prelude)
        from = std::max(end, best + in->url.size());
    }
    out.append(text.substr(std::min(from, text.size())));
    return out;
}

bool Session::finished() const {
    if (quit_) return true;
    if (platform_ && platform_->quit_requested()) return true;
    if (journal_ && journal_->replaying && journal_->cursor >= journal_->frames.size()) return true;
    if (options_.frames >= 0 && frames_ >= static_cast<std::uint64_t>(options_.frames)) return true;
    return false;
}

// The project's settings that can change while it runs: the audio room and buses (and the
// editor's audio.json), gestures and the input map (and input.json), sprite clips, render and
// physics settings. At start, and again when project.reload finds project.toml changed.
void Session::apply_project_settings() {
    // [locale] default = "en": the language the game starts in and falls back to.
    {
        std::string def;
        if (project_.contains("locale") && project_["locale"].is_object()) def = project_["locale"].value("default", std::string());
        if (def.empty()) {
            def = "en";
            std::error_code ec;
            const auto dir = options_.project_dir / "locales";
            if (!std::filesystem::exists(dir / "en.json", ec) && std::filesystem::is_directory(dir, ec)) {
                std::vector<std::string> found;
                for (const auto& f : std::filesystem::directory_iterator(dir, ec)) if (f.path().extension() == ".json") found.push_back(f.path().stem().string());
                std::sort(found.begin(), found.end());
                if (!found.empty()) def = found.front();
            }
        }
        if (locale_.empty() || locale_ == locale_default_) locale_ = def;
        locale_default_ = def;
        ++locale_rev_;
    }
    // [audio.reverb] room = 0.5, damping, mix: the project's room.
    if (project_.contains("audio") && project_["audio"].is_object() && project_["audio"].contains("reverb") && project_["audio"]["reverb"].is_object()) {
        const Json& rj = project_["audio"]["reverb"];
        audio::ReverbSettings r = audio_->reverb();
        r.room = static_cast<float>(opt<double>(rj, "room", r.room));
        r.damping = static_cast<float>(opt<double>(rj, "damping", r.damping));
        r.mix = static_cast<float>(opt<double>(rj, "mix", r.mix));
        audio_->set_reverb(r);
    }
    // [audio.buses] music = { volume = 0.6, duck_by = "dialogue" }: the project's buses.
    if (project_.contains("audio") && project_["audio"].is_object() && project_["audio"].contains("buses") && project_["audio"]["buses"].is_object()) {
        for (const auto& [name, bj] : project_["audio"]["buses"].items()) {
            if (bj.is_object()) audio_->set_bus(name, bus_settings(audio_->bus(name), bj));
        }
    }
    // audio.json beside project.toml, written by the editor's Audio tab: {"buses": {...}} over those.
    if (const std::filesystem::path mixer = options_.project_dir / "audio.json"; std::filesystem::exists(mixer)) {
        auto text = fs::read_text(mixer);
        Json j = text ? Json::parse(*text, nullptr, false) : Json();
        if (!text || j.is_discarded() || !j.is_object() || !j.contains("buses") || !j["buses"].is_object()) log::warn("runtime", "audio.json is not an object with \"buses\"; keeping the project.toml buses");
        else for (const auto& [name, bj] : j["buses"].items()) if (bj.is_object()) audio_->set_bus(name, bus_settings(audio_->bus(name), bj));
    }
    // The project's own components (components.toml beside project.toml, handed over by the tool)
    // before anything is loaded that may carry them.
    if (auto r = world_->declare_components(project_.value("components", Json::array())); !r) log::warn("runtime", "components.toml: {}", r.error().to_string());
    if (project_.contains("input") && project_["input"].is_object()) gestures_.configure(project_["input"]);
    // [input] cursor: how the pointer starts, "visible" (the default), "hidden" or "locked" (docs/design/input.md, The cursor).
    {
        const Json* c = project_.contains("input") && project_["input"].is_object() && project_["input"].contains("cursor") ? &project_["input"]["cursor"] : nullptr;
        const std::string mode = c && c->is_string() ? c->get<std::string>() : "visible";
        if (mode != "visible" && mode != "hidden" && mode != "locked") log::warn("runtime", "[input] cursor is \"visible\", \"hidden\" or \"locked\", not {}", c->dump());
        set_cursor(mode == "locked", mode != "hidden");
    }
    if (project_.contains("input") && project_["input"].is_object() && project_["input"].contains("actions")) {
        if (auto r = input_map_.configure(project_["input"]["actions"]); !r) log::warn("runtime", "project input map: {}", r.error().to_string());
        else log::info("runtime", "input map: {} actions from project.toml", input_map_.size());
    }
    // input.json beside project.toml, written by the editor's Input tab, replaces that map
    // (docs/design/input.md, Map): {"actions": {...}} in the same shape.
    if (const std::filesystem::path override = options_.project_dir / "input.json"; std::filesystem::exists(override)) {
        auto text = fs::read_text(override);
        Json j = text ? Json::parse(*text, nullptr, false) : Json();
        if (!text || j.is_discarded() || !j.is_object() || !j.contains("actions") || !j["actions"].is_object()) log::warn("runtime", "input.json is not an object with \"actions\"; keeping the project.toml map");
        else if (auto r = input_map_.configure(j["actions"]); !r) log::warn("runtime", "input.json: {}", r.error().to_string());
        else log::info("runtime", "input map: {} actions from input.json", input_map_.size());
    }
    if (project_.contains("sprite_clips") && project_["sprite_clips"].is_object()) {
        for (const auto& [name, j] : project_["sprite_clips"].items()) {
            if (auto clip = world::World::SpriteClip::from_json(j)) world_->define_clip(name, std::move(*clip));
            else log::warn("runtime", "project sprite clip '{}': {}", name, clip.error().to_string());
        }
    }
    // [sprite_sheets] name = "assets/hero.json": an Aseprite sheet, a clip per tag named name.tag.
    if (project_.contains("sprite_sheets") && project_["sprite_sheets"].is_object()) {
        for (const auto& [name, path] : project_["sprite_sheets"].items()) {
            if (!path.is_string()) continue;
            if (auto r = load_sprite_sheet(path.get<std::string>(), name); !r) log::warn("runtime", "sprite sheet '{}': {}", name, r.error().to_string());
        }
    }
    if (project_.contains("animations") && project_["animations"].is_object() && assets_) {
        // [animations] "assets/hero.glb" = ["assets/anims/run.glb", ...]: clips from other files.
        for (const auto& [model, files] : project_["animations"].items()) {
            if (!files.is_array()) continue;
            if (auto r = add_animation_library(model, files, false); !r) log::warn("runtime", "[animations] {}: {}", model, r.error().message);
        }
    }
    if (project_.contains("render") && project_["render"].is_object() && project_["render"].contains("post")) {
        // [render] post = ["effects/crt.wgsl", {shader = "...", params = [...]}]: the project's
        // post effects (docs/design/rendering.md, Post effects), read again on project.reload.
        auto r = set_post_effects(project_["render"]["post"]);
        if (!r) log::warn("runtime", "[render] post: {}", r.error().message);
        else for (const Json& e : (*r)["effects"]) if (!e.value("ok", false)) log::warn("runtime", "post effect {}: {}", e.value("name", std::string()), e.value("error", std::string()));
    }
    if (project_.contains("render") && project_["render"].is_object()) {
        renderer::ShadowSettings s = renderer_->shadows();
        const Json& r = project_["render"];
        if (r.contains("shadows") && r["shadows"].is_boolean()) s.enabled = r["shadows"].get<bool>();
        if (r.contains("msaa") && r["msaa"].is_number()) renderer_->set_msaa(r["msaa"].get<int>());
        if (r.contains("shadow_strength") && r["shadow_strength"].is_number()) s.strength = std::clamp(r["shadow_strength"].get<float>(), 0.0f, 1.0f);
        if (r.contains("shadow_cascades") && r["shadow_cascades"].is_number()) s.cascades = r["shadow_cascades"].get<int>();
        if (r.contains("shadow_distance") && r["shadow_distance"].is_number()) s.distance = r["shadow_distance"].get<float>();
        if (r.contains("shadow_softness") && r["shadow_softness"].is_number()) s.softness = r["shadow_softness"].get<float>();
        if (r.contains("contact_shadows") && r["contact_shadows"].is_boolean()) s.contact = r["contact_shadows"].get<bool>();
        if (r.contains("contact_shadow_length") && r["contact_shadow_length"].is_number()) s.contact_length = r["contact_shadow_length"].get<float>();
        renderer_->set_shadows(s);
        renderer::BloomSettings b = renderer_->bloom();
        if (r.contains("bloom") && r["bloom"].is_boolean()) b.enabled = r["bloom"].get<bool>();
        if (r.contains("bloom_threshold") && r["bloom_threshold"].is_number()) b.threshold = r["bloom_threshold"].get<float>();
        if (r.contains("bloom_strength") && r["bloom_strength"].is_number()) b.strength = r["bloom_strength"].get<float>();
        if (r.contains("bloom_radius") && r["bloom_radius"].is_number()) b.radius = r["bloom_radius"].get<float>();
        renderer_->set_bloom(b);
        // [render] ambient = "#202030", or {color, intensity}.
        if (r.contains("ambient")) {
            renderer::AmbientSettings a = renderer_->ambient();
            const Json& aj = r["ambient"];
            if (aj.is_object()) {
                if (aj.contains("color")) (void)read_tint(aj["color"], a.color);
                a.intensity = opt<float>(aj, "intensity", a.intensity);
            } else {
                (void)read_tint(aj, a.color);
            }
            renderer_->set_ambient(a);
        }
        // [render] grade = true, or a [render.grade] table with the settings (on unless it says enabled = false).
        if (r.contains("grade")) {
            renderer::GradeSettings g = renderer_->grade();
            const Json& gj = r["grade"];
            if (gj.is_boolean()) {
                g.enabled = gj.get<bool>();
            } else if (gj.is_object()) {
                g.enabled = opt<bool>(gj, "enabled", true);
                read_grade(g, gj);
            }
            renderer_->set_grade(g);
        }
        // [render] ssr = true, or [render.ssr] max_distance, max_roughness, steps, thickness, intensity.
        if (r.contains("ssr") && (r["ssr"].is_object() || r["ssr"].is_boolean())) {
            renderer::SsrSettings s = renderer_->ssr();
            if (r["ssr"].is_boolean()) {
                s.enabled = r["ssr"].get<bool>();
            } else {
                s.enabled = opt<bool>(r["ssr"], "enabled", true);
                s.max_distance = opt<float>(r["ssr"], "max_distance", s.max_distance);
                s.max_roughness = opt<float>(r["ssr"], "max_roughness", s.max_roughness);
                s.steps = opt<int>(r["ssr"], "steps", s.steps);
                s.thickness = opt<float>(r["ssr"], "thickness", s.thickness);
                s.intensity = opt<float>(r["ssr"], "intensity", s.intensity);
            }
            renderer_->set_ssr(s);
        }
        // [render] toon = true, or [render.toon] bands, outline, outline_color, opacity.
        if (r.contains("toon") && (r["toon"].is_boolean() || r["toon"].is_object())) {
            const Json args = r["toon"].is_boolean() ? Json{{"enabled", r["toon"].get<bool>()}} : r["toon"];
            if (auto done = render_command("toon", args); !done) log::warn("runtime", "[render] toon: {}", done.error().message);
        }
        // [render] colorblind = "deuteranopia", or [render.colorblind] mode, simulate, strength.
        if (r.contains("colorblind") && (r["colorblind"].is_string() || r["colorblind"].is_object())) {
            const Json args = r["colorblind"].is_string() ? Json{{"mode", r["colorblind"]}} : r["colorblind"];
            if (auto done = render_command("colorblind", args); !done) log::warn("runtime", "[render] colorblind: {}", done.error().message);
        }
        // [render] scale = 0.75, or [render.scale] scale, dynamic, target_ms, least, sharpen.
        if (r.contains("scale") && (r["scale"].is_number() || r["scale"].is_object())) {
            renderer::RenderScaleSettings s = renderer_->render_scale();
            if (r["scale"].is_number()) {
                s.scale = r["scale"].get<float>();
            } else {
                s.scale = opt<float>(r["scale"], "scale", s.scale);
                s.dynamic = opt<bool>(r["scale"], "dynamic", s.dynamic);
                s.target_ms = opt<float>(r["scale"], "target_ms", s.target_ms);
                s.least = opt<float>(r["scale"], "least", s.least);
                s.sharpen = opt<float>(r["scale"], "sharpen", s.sharpen);
                s.pixelated = opt<bool>(r["scale"], "pixelated", s.pixelated);
            }
            renderer_->set_render_scale(s);
        }
        // [render] ssgi = true, or [render.ssgi] distance, rays, steps, thickness, intensity.
        if (r.contains("ssgi") && (r["ssgi"].is_object() || r["ssgi"].is_boolean())) {
            renderer::SsgiSettings s = renderer_->ssgi();
            if (r["ssgi"].is_boolean()) {
                s.enabled = r["ssgi"].get<bool>();
            } else {
                s.enabled = opt<bool>(r["ssgi"], "enabled", true);
                s.distance = opt<float>(r["ssgi"], "distance", s.distance);
                s.rays = opt<int>(r["ssgi"], "rays", s.rays);
                s.steps = opt<int>(r["ssgi"], "steps", s.steps);
                s.thickness = opt<float>(r["ssgi"], "thickness", s.thickness);
                s.intensity = opt<float>(r["ssgi"], "intensity", s.intensity);
            }
            renderer_->set_ssgi(s);
        }
        // [render.dof] focus, aperture, max_blur (on unless enabled = false); [render.motion_blur] strength, samples.
        if (r.contains("dof") && (r["dof"].is_object() || r["dof"].is_boolean())) {
            renderer::DofSettings d = renderer_->dof();
            if (r["dof"].is_boolean()) {
                d.enabled = r["dof"].get<bool>();
            } else {
                d.enabled = opt<bool>(r["dof"], "enabled", true);
                d.focus = opt<float>(r["dof"], "focus", d.focus);
                d.aperture = opt<float>(r["dof"], "aperture", d.aperture);
                d.max_blur = opt<float>(r["dof"], "max_blur", d.max_blur);
            }
            renderer_->set_dof(d);
        }
        if (r.contains("motion_blur") && (r["motion_blur"].is_object() || r["motion_blur"].is_boolean())) {
            renderer::MotionBlurSettings m = renderer_->motion_blur();
            if (r["motion_blur"].is_boolean()) {
                m.enabled = r["motion_blur"].get<bool>();
            } else {
                m.enabled = opt<bool>(r["motion_blur"], "enabled", true);
                m.strength = opt<float>(r["motion_blur"], "strength", m.strength);
                m.samples = opt<int>(r["motion_blur"], "samples", m.samples);
            }
            renderer_->set_motion_blur(m);
        }
        // [render] oit = true
        if (r.contains("oit") && r["oit"].is_boolean()) renderer_->set_oit(r["oit"].get<bool>());
        // [render] taa = true, or [render.taa] enabled, feedback.
        if (r.contains("taa") && (r["taa"].is_object() || r["taa"].is_boolean())) {
            renderer::TaaSettings t = renderer_->taa();
            if (r["taa"].is_boolean()) {
                t.enabled = r["taa"].get<bool>();
            } else {
                t.enabled = opt<bool>(r["taa"], "enabled", true);
                t.feedback = opt<float>(r["taa"], "feedback", t.feedback);
            }
            renderer_->set_taa(t);
        }
        // [render.ao] enabled = true, radius, intensity, samples (a table: on unless it says enabled = false).
        if (r.contains("ao") && (r["ao"].is_object() || r["ao"].is_boolean())) {
            renderer::AoSettings a = renderer_->ao();
            if (r["ao"].is_boolean()) {
                a.enabled = r["ao"].get<bool>();
            } else {
                a.enabled = opt<bool>(r["ao"], "enabled", true);
                a.radius = opt<float>(r["ao"], "radius", a.radius);
                a.intensity = opt<float>(r["ao"], "intensity", a.intensity);
                a.samples = opt<int>(r["ao"], "samples", a.samples);
            }
            renderer_->set_ao(a);
        }
        // [render.tonemap] operator = "agx", exposure, auto_exposure, compensation, min_ev, max_ev, speed.
        if (r.contains("tonemap") && r["tonemap"].is_object()) {
            renderer::TonemapSettings t = renderer_->tonemap();
            std::string why;
            if (read_tonemap(t, r["tonemap"], why)) renderer_->set_tonemap(t);
            else log::warn("runtime", "project [render.tonemap]: {}", why);
        }
    }
    if (project_.contains("physics") && project_["physics"].is_object()) {
        const Json& ph = project_["physics"];
        if (ph.contains("gravity") && ph["gravity"].is_array() && ph["gravity"].size() == 3) {
            physics_->settings().gravity = {ph["gravity"][0].get<float>(), ph["gravity"][1].get<float>(), ph["gravity"][2].get<float>()};
        }
        // Layer names, bit 0 first: documentation for agents and scripts (physics.layers).
        physics_layers_.clear();
        if (ph.contains("layers") && ph["layers"].is_array()) {
            for (const Json& n : ph["layers"]) if (n.is_string() && physics_layers_.size() < 32) physics_layers_.push_back(n.get<std::string>());
        }
    }
}

Status Session::start() {
    auto& logger = log::global();
    logger.set_level(log::level_from_name(options_.log_level));
    logger.clear_sinks();
    logger.add_sink(log::stderr_sink());
    if (!options_.log_file.empty()) logger.add_sink(log::jsonl_file_sink(options_.log_file.string()));

    // Lockstep: a joining peer takes the host's seed before anything is made from it, so every
    // peer's world starts the same (docs/design/networking.md).
    if (!options_.net_join.empty()) {
        POCKET_TRY(peer, Net::join(options_.net_join, 20.0));
        options_.seed = peer->seed();
        net_ = std::move(peer);
    } else if (options_.net_host >= 0) {
        POCKET_TRY(peer, Net::host(options_.net_host, options_.net_players, options_.net_delay, options_.seed, options_.net_dedicated));
        net_ = std::move(peer);
    }

    if (std::filesystem::exists(options_.project_config)) {
        POCKET_TRY(text, fs::read_text(options_.project_config));
        project_ = Json::parse(text, nullptr, false);
        if (project_.is_discarded()) return fail("bad_project_config", "{} is not valid JSON", options_.project_config.string());
    }
    name_ = project_.contains("name") && project_["name"].is_string() ? project_["name"].get<std::string>() : options_.project_dir.filename().string();
    if (options_.history > 0) recorder_.start(static_cast<std::size_t>(options_.history));
    Json window = project_.contains("window") && project_["window"].is_object() ? project_["window"] : Json::object();
    int width = options_.width > 0 ? options_.width : window.value("width", 960);
    int height = options_.height > 0 ? options_.height : window.value("height", 540);
    std::string title = !options_.title.empty() ? options_.title : (window.contains("title") && window["title"].is_string() ? window["title"].get<std::string>() : name_);

    if (!options_.replay.empty()) {
        POCKET_TRY(j, Journal::open_for_replay(options_.replay));
        journal_ = std::make_unique<Journal>(std::move(j));
        options_.seed = journal_->seed;
        options_.tick_rate = journal_->tick_rate;
        log::info("runtime", "replaying {} ({} frames)", options_.replay.string(), journal_->frames.size());
    } else if (!options_.record.empty()) {
        journal_ = std::make_unique<Journal>(Journal::open_for_record(options_.record, options_.seed, options_.tick_rate));
    }

    POCKET_TRY(bundle_src, fs::read_text(options_.bundle));

#ifdef __EMSCRIPTEN__
    if (!options_.save_dir.empty()) {
        log::warn("runtime", "--save-dir is ignored in the browser; saves live in IndexedDB");
        options_.save_dir.clear();
    }
    if (!web_mount_saves(save_dir().string())) log::warn("runtime", "saves will not persist: the browser refused IndexedDB");
#endif

    platform::Config pc;
    pc.title = title.empty() ? "Pocket" : title;
    pc.width = width;
    pc.height = height;
    pc.headless = options_.headless;
    pc.visible = options_.visible;
    pc.fullscreen = window.value("fullscreen", false);
    POCKET_TRY(platform, platform::Platform::create(pc));
    platform_ = std::move(platform);

    rhi::Config rc;
    const platform::Platform::NativeWindow native = platform_->native_window();
    rc.metal_layer = native.metal_layer;
    rc.x11_display = native.x11_display;
    rc.x11_window = native.x11_window;
    rc.wayland_display = native.wayland_display;
    rc.wayland_surface = native.wayland_surface;
    rc.canvas_selector = platform_->canvas_selector();
    rc.width = static_cast<std::uint32_t>(platform_->pixel_width());
    rc.height = static_cast<std::uint32_t>(platform_->pixel_height());
    POCKET_TRY(device, rhi::Device::create(rc));
    device_ = std::move(device);

    script::Config sc;
    sc.inspectable = options_.inspectable;
    sc.console_text = [this](std::string text) { return bundle_lines_.empty() ? text : source_lines(text); };
    POCKET_TRY(host, script::ScriptHost::create(sc));
    host_ = std::move(host);

    POCKET_TRY(renderer, renderer::Renderer::create(*device_));
    renderer_ = std::move(renderer);
    renderer_->set_time_step(static_cast<float>(1.0 / options_.tick_rate));
    particles_ = std::make_unique<renderer::Particles>();
    particles_->set_collider([this](Vec3 from, Vec3 to) -> std::optional<renderer::ParticleContact> {
        const Vec3 d = to - from;
        const float len = length(d);
        if (len < 1e-6f) return std::nullopt;
        if (physics_) {
            auto hit = physics_->raycast(*world_, from, d * (1.0f / len), len, false);
            if (hit && hit->entity != 0) return renderer::ParticleContact{hit->point, hit->normal};
        }
        // The solid tiles of orthogonal maps: when the particle would end in one, the contact is
        // on the face it crossed to get there (the top or bottom when it changed row, else a side).
        std::optional<renderer::ParticleContact> tile;
        if (assets_) {
            world_->ecs().each([&](flecs::entity e, const world::TileMap& tm) {
                if (tile) return;
                auto m = assets_->tilemap(tm.map);
                if (!m || !(*m)->orthogonal()) return;
                const float ts = tm.tile_size > 0 ? tm.tile_size : 1.0f;
                Vec3 origin{0, 0, 0};
                if (const auto* wt = e.try_get<world::WorldTransform>()) origin = wt->position;
                else if (const auto* t = e.try_get<world::Transform>()) origin = t->position;
                auto col = [&](float x) { return static_cast<int>(std::floor((x - origin.x) / ts)); };
                auto row = [&](float y) { return static_cast<int>(std::floor((origin.y - y) / ts)); };
                const int c1 = col(to.x), r1 = row(to.y);
                if (c1 < 0 || r1 < 0 || c1 >= (*m)->width || r1 >= (*m)->height || (*m)->solidity_at(c1, r1) != 1) return;
                const int c0 = col(from.x), r0 = row(from.y);
                Vec3 point = from;
                Vec3 normal{0, 1, 0};
                if (r0 != r1) {
                    const float face = r1 > r0 ? origin.y - static_cast<float>(r1) * ts : origin.y - static_cast<float>(r1 + 1) * ts;
                    const float t = std::fabs(d.y) > 1e-6f ? (face - from.y) / d.y : 0.0f;
                    point = from + d * std::clamp(t, 0.0f, 1.0f);
                    normal = {0, r1 > r0 ? 1.0f : -1.0f, 0};
                } else if (c0 != c1) {
                    const float face = c1 > c0 ? origin.x + static_cast<float>(c1) * ts : origin.x + static_cast<float>(c1 + 1) * ts;
                    const float t = std::fabs(d.x) > 1e-6f ? (face - from.x) / d.x : 0.0f;
                    point = from + d * std::clamp(t, 0.0f, 1.0f);
                    normal = {c1 > c0 ? -1.0f : 1.0f, 0, 0};
                }
                tile = renderer::ParticleContact{point, normal};
            });
        }
        return tile;
    });
    animation_ = std::make_unique<renderer::Animation>();

    // Pocket UI needs a font. `pocket` records the path of the bundled Noto Sans CJK in the
    // project config; POCKET_FONT overrides it. Without a font the ui.* commands report
    // ui_unavailable and everything else works.
    std::string font = project_.contains("font") && project_["font"].is_string() ? project_["font"].get<std::string>() : "";
    // A relative font path is relative to the project config (packed games ship the font next to it).
    if (!font.empty() && std::filesystem::path(font).is_relative() && !options_.project_config.empty()) font = (options_.project_config.parent_path() / font).string();
    // A project's own interface font (`[ui] font` in project.toml, a path in the project): the
    // bundled one then stands behind it for what it lacks (Chinese, say, under a pixel font).
    std::vector<std::string> fallbacks;
    const Json ui_settings = project_.contains("ui") && project_["ui"].is_object() ? project_["ui"] : Json::object();
    auto in_project = [this](std::string fp) {
        if (std::filesystem::path(fp).is_relative()) {
            // A project's own (project.toml) is in the project; a packed game's next to its config.
            const std::filesystem::path here = options_.project_dir / fp;
            if (std::filesystem::exists(here)) fp = here.string();
            else if (!options_.project_config.empty()) fp = (options_.project_config.parent_path() / fp).string();
        }
        return fp;
    };
    if (ui_settings.contains("font") && ui_settings["font"].is_string()) {
        const std::string own = in_project(ui_settings["font"].get<std::string>());
        if (std::filesystem::exists(own)) {
            if (!font.empty()) fallbacks.push_back(font);
            font = own;
        } else {
            log::warn("runtime", "[ui] font {} not found; the bundled font is used", own);
        }
    }
    if (const char* env = std::getenv("POCKET_FONT"); env && *env) font = env;
    if (!options_.font.empty()) font = options_.font.string();
    // Fonts for what the first lacks (docs/design/pocket-ui.md, Fallback fonts): the project config's
    // `font_fallbacks`, which `pocket` fills with the bundled Noto Sans Arabic unless project.toml
    // names its own (paths relative to the project).
    if (project_.contains("font_fallbacks") && project_["font_fallbacks"].is_array()) {
        for (const Json& fb : project_["font_fallbacks"]) {
            if (!fb.is_string()) continue;
            const std::string fp = in_project(fb.get<std::string>());
            if (!std::filesystem::exists(fp)) {
                log::warn("runtime", "fallback font {} not found", fp);
                continue;
            }
            fallbacks.push_back(fp);
        }
    }
    auto with_fallbacks = [&](ui::Font& f) {
        for (const std::string& fp : fallbacks) {
            if (auto r = f.add_fallback(fp); !r) log::warn("runtime", "fallback font {}: {}", fp, r.error().message);
        }
    };
    if (!font.empty() && std::filesystem::exists(font)) {
        POCKET_TRY(loaded_font, ui::Font::load(*device_, font));
        font_ = std::move(loaded_font);
        with_fallbacks(*font_);
        POCKET_TRY(painter, ui::Painter::create(*device_, *font_));
        painter_ = std::move(painter);
        ui_ = std::make_unique<ui::Document>(*font_);
        // The project's named fonts (`[ui.fonts] title = "assets/fonts/Title.ttf"`), for the
        // `font` style; each with the same fonts behind it.
        if (ui_settings.contains("fonts") && ui_settings["fonts"].is_object()) {
            for (const auto& [name, path] : ui_settings["fonts"].items()) {
                if (!path.is_string()) continue;
                const std::string fp = in_project(path.get<std::string>());
                if (!std::filesystem::exists(fp)) {
                    log::warn("runtime", "[ui.fonts] {}: {} not found", name, fp);
                    continue;
                }
                auto loaded = ui::Font::load(*device_, fp);
                if (!loaded) {
                    log::warn("runtime", "[ui.fonts] {}: {}", name, loaded.error().message);
                    continue;
                }
                with_fallbacks(**loaded);
                ui_->add_font(name, **loaded);
                named_fonts_.push_back({name, fp, std::move(*loaded)});
            }
        }
        if (platform_ && !platform_->headless()) {
            ui_->set_clipboard([this] { return platform_->clipboard_text(); }, [this](const std::string& text) { platform_->set_clipboard_text(text); });
        }
        ui_->set_anchor_source([this](std::uint64_t entity, float& x, float& y) {
            // An anchored element follows its entity's projection, in points.
            if (!renderer_ || !world_ || !world_->alive(static_cast<world::EntityId>(entity))) return false;
            const world::WorldTransform* wt = world_->try_get<world::WorldTransform>(static_cast<world::EntityId>(entity));
            if (!wt) return false;
            float px = 0, py = 0;
            if (!renderer_->project(wt->position, px, py)) return false;
            float w = 0, h = 0, scale = 1;
            ui_size(w, h, scale);
            x = px / scale;
            y = py / scale;
            return true;
        });
        ui_->set_image_source([this](const std::string& path) {
            ui::Document::ImageSource src;
            if (!renderer_) return src;
            const auto v = renderer_->image_view(path);
            src.view = v.view;
            src.width = static_cast<float>(v.width);
            src.height = static_cast<float>(v.height);
            return src;
        });
        font_path_ = font;
    } else {
        log::warn("runtime", "ui disabled: no font found (project config 'font' or POCKET_FONT)");
    }

    world_ = std::make_unique<world::World>();
    physics_ = std::make_unique<physics::Physics>();
    physics2d_ = std::make_unique<physics::Physics2D>();
    rigid2d_ = std::make_unique<physics::Rigid2D>();
    timelines_ = std::make_unique<Timelines>(options_.project_dir);
    assets_ = std::make_unique<assets::AssetStore>(options_.project_dir);
    // [assets] blender = "/path/to/blender": where Blender is for the formats it imports.
    if (project_.contains("assets") && project_["assets"].is_object() && project_["assets"].contains("blender") && project_["assets"]["blender"].is_string()) assets_->set_blender(project_["assets"]["blender"].get<std::string>());
    physics_->set_assets(assets_.get());
    renderer_->set_assets(assets_.get());
    // An asset mesh's bounds from the store, so Bounds do not wait for the renderer to draw it
    // (a headless step draws only its last tick).
    world_->set_mesh_bounds_source([this](const std::string& mesh, Vec3& lo, Vec3& hi) {
        auto m = assets_->mesh(mesh);
        if (!m) return false;
        lo = (*m)->aabb_min;
        hi = (*m)->aabb_max;
        return true;
    });
    audio::Config ac;
    ac.project_dir = options_.project_dir;
    ac.headless = options_.headless;
    if (project_.contains("audio") && project_["audio"].is_object() && project_["audio"].contains("stream_seconds") && project_["audio"]["stream_seconds"].is_number()) {
        ac.stream_seconds = std::max(0.0, project_["audio"]["stream_seconds"].get<double>());
    }
    POCKET_TRY(audio, audio::Audio::create(ac));
    audio_ = std::move(audio);
    apply_project_settings();
    if (net_) player_maps_.assign(static_cast<std::size_t>(std::max(net_->players() - 1, 0)), input_map_);
    rng_.reseed(options_.seed);
    clock_.tick_seconds = 1.0 / options_.tick_rate;

    POCKET_TRY_VOID(load_scene_file());
    update_terrains();

    bind_natives();
    started_ = true;
    (void)bundle_src;
    // script.eval helper: indirect eval in the global scope, errors reported as values.
    // The SDK's exports are names of their own in what script.eval runs (world, events, input, ...,
    // and pocket for all of them; kept current across reloads, never over a name the page or a game
    // made global), and an error answers with its message as well as its frames.
    if (auto r = host_->evaluate(R"(globalThis.__pocket_eval = function (src) {
    const sdk = typeof globalThis.__pocket_sdk === "function" ? globalThis.__pocket_sdk() : null;
    if (sdk) {
        const ours = globalThis.__pocket_eval_names || (globalThis.__pocket_eval_names = new Set());
        for (const k of ["pocket", ...Object.keys(sdk)]) {
            if (!/^[A-Za-z_$][\w$]*$/.test(k) || (k in globalThis && !ours.has(k))) continue;
            globalThis[k] = k === "pocket" ? sdk : sdk[k];
            ours.add(k);
        }
    }
    try { const v = (0, eval)(src); return v === undefined ? null : v; }
    catch (e) { return { error: String(e) + (e && e.stack ? "\n" + e.stack : "") }; }
};)", "<pocket:eval>"); !r) record_error(r.error());
    // Math.random from the run's seed, so code that reaches for it (a library, an agent's habit)
    // replays, runs a scenario at many seeds and plays in lockstep like random() does. sfc32 on
    // 32-bit integer operations gives the same bits on every JavaScript engine; it is a stream of
    // its own, so calling one does not move the other.
    if (auto r = host_->evaluate(R"(globalThis.__pocket_seed_random = function (lo, hi) {
    let a = lo | 0, b = hi | 0, c = 0x6a09e667 | 0, d = 1;
    const next = function random() {
        const t = (((a + b) | 0) + d) | 0;
        d = (d + 1) | 0;
        a = b ^ (b >>> 9);
        b = (c + (c << 3)) | 0;
        c = (c << 21) | (c >>> 11);
        c = (c + t) | 0;
        return (t >>> 0) / 4294967296;
    };
    for (let i = 0; i < 15; i++) next();
    Math.random = next;
};)", "<pocket:random>"); !r) record_error(r.error());
    seed_math_random();
    if (!options_.editor_bundle.empty()) {
        if (auto r = load_bundle(options_.editor_bundle, "editor"); !r) record_error(r.error());
    }
    if (auto r = load_bundle(options_.bundle, "project"); !r) record_error(r.error());
    if (!options_.scenario_bundle.empty()) {
        if (auto r = load_bundle(options_.scenario_bundle, "scenario"); !r) record_error(r.error());
    }
    has_dispatch_ = host_->has_function("__pocket_dispatch");
    if (!has_dispatch_) log::warn("runtime", "bundle does not define __pocket_dispatch; scripts will not receive ticks");
    // With an editor, the project stays dormant (its bundle registered handlers, nothing ran)
    // until the editor starts it with script.start; the editor itself starts now.
    if (!options_.editor_bundle.empty()) dispatch("start", Json::object(), "editor");
    else dispatch("start", Json::object());
    return {};
}

void Session::seed_math_random() {
    const auto lo = static_cast<std::uint32_t>(options_.seed) ^ 0x9e3779b9u, hi = static_cast<std::uint32_t>(options_.seed >> 32) ^ 0x85ebca6bu;
    if (auto r = host_->evaluate(std::format("__pocket_seed_random({}, {});", lo, hi), "<pocket:random>"); !r) record_error(r.error());
}

void Session::bind_natives() {
    host_->bind("log", [this](const Json& args) -> Result<Json> {
        std::string level = args.size() > 0 && args[0].is_string() ? args[0].get<std::string>() : "info";
        std::string msg = source_lines(args.size() > 1 && args[1].is_string() ? args[1].get<std::string>() : (args.size() > 1 ? args[1].dump() : ""));
        Json fields = nullptr;
        if (args.size() > 2 && args[2].is_string()) fields = Json::parse(args[2].get<std::string>(), nullptr, false);
        else if (args.size() > 2) fields = args[2];
        log::global().emit(log::level_from_name(level), "script", msg, fields);
        return nullptr;
    });
    host_->bind("setClearColor", [this](const Json& args) -> Result<Json> {
        if (args.size() < 3) return fail("bad_args", "setClearColor(r, g, b, a?)");
        clear_ = {args[0].get<float>(), args[1].get<float>(), args[2].get<float>(), args.size() > 3 ? args[3].get<float>() : 1.0f};
        return nullptr;
    });
    host_->bind("random", [this](const Json&) -> Result<Json> { return rng_.next_double(); });
    // Wall-clock milliseconds since the session started, for measuring script cost (never for gameplay).
    host_->bind("now", [this](const Json&) -> Result<Json> { return total_.ms(); });
    host_->bind_numbers("clock", [this](const double*, std::size_t) -> double { return total_.ms(); });   // the same without JSON
    host_->bind("info", [this](const Json&) -> Result<Json> {
        Json j;
        j["tickRate"] = options_.tick_rate;
        j["headless"] = options_.headless;
        j["width"] = device_->width();
        j["height"] = device_->height();
        j["seed"] = options_.seed;
        j["project"] = name_;
        j["version"] = POCKET_VERSION;
        j["scenario"] = options_.scenario;
        return j;
    });
    // world.get and world.set of a component whose fields are all numbers, without JSON (sdk
    // world.ts): the values cross in the shared Float64Array __pocket.__nums, in the generated
    // order; the answer is their count, or below 0 for the SDK to take the JSON path instead.
    nums_.assign(256, 0.0);
    host_->share_f64("__nums", nums_.data(), nums_.size());
    host_->bind("get_nums", [this](const Json& args) -> Result<Json> {
        if (args.size() < 2 || !args[1].is_string()) return -2;
        const world::EntityId id = args[0].is_number_unsigned() || args[0].is_number_integer() ? args[0].get<world::EntityId>() : resolve_entity(args[0]);
        return world_->get_numbers(id, args[1].get_ref<const std::string&>(), nums_.data());
    });
    host_->bind("set_nums", [this](const Json& args) -> Result<Json> {
        if (args.size() < 3 || !args[1].is_string() || !args[2].is_number()) return -2;
        const world::EntityId id = args[0].is_number_unsigned() || args[0].is_number_integer() ? args[0].get<world::EntityId>() : resolve_entity(args[0]);
        const auto n = static_cast<std::size_t>(std::clamp<std::int64_t>(args[2].get<std::int64_t>(), 0, static_cast<std::int64_t>(nums_.size())));
        return world_->set_numbers(id, args[1].get_ref<const std::string&>(), nums_.data(), n);
    });
    // The same by component index and entity id, numbers only (no JSON either way): what the SDK
    // uses once it has the index of a component (component_index, asked once per name).
    host_->bind("component_index", [this](const Json& args) -> Result<Json> {
        if (args.empty() || !args[0].is_string()) return -1;
        return world_->component_index(args[0].get_ref<const std::string&>());
    });
    host_->bind_numbers("get_nums_at", [this](const double* a, std::size_t n) -> double {
        ++script_numbers_[0];
        if (n < 2 || !(a[0] >= 0) || !(a[1] >= 0)) return -2;
        return static_cast<double>(world_->get_numbers(static_cast<world::EntityId>(a[0]), static_cast<int>(a[1]), nums_.data()));
    });
    host_->bind_numbers("set_nums_at", [this](const double* a, std::size_t n) -> double {
        ++script_numbers_[1];
        if (n < 3 || !(a[0] >= 0) || !(a[1] >= 0) || !(a[2] >= 0)) return -2;
        const auto count = static_cast<std::size_t>(std::min(a[2], static_cast<double>(nums_.size())));
        return static_cast<double>(world_->set_numbers(static_cast<world::EntityId>(a[0]), static_cast<int>(a[1]), nums_.data(), count));
    });
    // A patch: the numbers whose bits are set in the mask, laid over the component as it is.
    host_->bind_numbers("patch_nums_at", [this](const double* a, std::size_t n) -> double {
        ++script_numbers_[1];
        if (n < 3 || !(a[0] >= 0) || !(a[1] >= 0) || !(a[2] >= 1) || a[2] > 2147483647.0) return -2;
        const auto id = static_cast<world::EntityId>(a[0]);
        const int component = static_cast<int>(a[1]);
        const auto mask = static_cast<std::uint32_t>(a[2]);
        double current[256];
        const long count = world_->get_numbers(id, component, current);
        if (count < 0) return static_cast<double>(count);
        for (long i = 0; i < count && i < 32; ++i) if (mask & (1u << i)) current[i] = nums_[static_cast<std::size_t>(i)];
        return static_cast<double>(world_->set_numbers(id, component, current, static_cast<std::size_t>(count)));
    });
    host_->bind("command", [this](const Json& args) -> Result<Json> {
        if (args.empty() || !args[0].is_string()) return fail("bad_args", "command(name, params?)");
        static const Json kNone = Json::object();
        const Json& given = args.size() > 1 ? args[1] : kNone;
        const std::string& method = args[0].get_ref<const std::string&>();
        Stopwatch took;
        auto r = given.is_string() ? command(method, Json::parse(given.get<std::string>(), nullptr, false), "script") : command(method, given, "script");
        CallCost& c = script_calls_[method];   // script.profile
        c.calls += 1;
        c.ms += took.ms();
        return r;
    });
}

namespace {
// Where a file stops being JSON, for the one who wrote it: the parser's line, column and reason, and
// that line of the file ("line 4, column 9: syntax error ... near `"Transform": {"pos" 1}`").
// Read through the parser's events (no exceptions: the web build has none), keeping its error.
struct ProblemSax : nlohmann::json_sax<Json> {
    std::size_t at = 0;
    std::string why;
    bool null() override { return true; }
    bool boolean(bool) override { return true; }
    bool number_integer(number_integer_t) override { return true; }
    bool number_unsigned(number_unsigned_t) override { return true; }
    bool number_float(number_float_t, const string_t&) override { return true; }
    bool string(string_t&) override { return true; }
    bool binary(binary_t&) override { return true; }
    bool start_object(std::size_t) override { return true; }
    bool key(string_t&) override { return true; }
    bool end_object() override { return true; }
    bool start_array(std::size_t) override { return true; }
    bool end_array() override { return true; }
    bool parse_error(std::size_t position, const std::string&, const nlohmann::detail::exception& ex) override {
        at = position;
        why = ex.what();
        return false;
    }
};
std::string json_problem(const std::string& text) {
    ProblemSax sax;
    (void)Json::sax_parse(text, &sax);
    if (sax.why.empty()) return "not valid JSON";
    std::string why = sax.why;
    if (const auto at = why.find("parse error at "); at != std::string::npos) why = why.substr(at + 15);
    std::size_t line_start = 0, line_no = 1;
    const std::size_t byte = std::min<std::size_t>(sax.at > 0 ? sax.at - 1 : 0, text.size());
    for (std::size_t i = 0; i < byte; ++i) if (text[i] == '\n') { line_start = i + 1; ++line_no; }
    std::string line = text.substr(line_start, text.find('\n', line_start) - line_start);
    if (line.size() > 160) line = line.substr(0, 160) + "...";
    return std::format("{} (line {}: `{}`)", why, line_no, line);
}
}  // namespace

// Scene from project.toml `scene = "..."` (relative to the project directory).
Status Session::load_scene_file() {
    std::string scene_rel = project_.contains("scene") && project_["scene"].is_string() ? project_["scene"].get<std::string>() : "";
    if (scene_rel.empty()) return {};
    std::filesystem::path scene_path = options_.project_dir / scene_rel;
    POCKET_TRY(text, fs::read_text(scene_path));
    Json scene = Json::parse(text, nullptr, false);
    if (scene.is_discarded()) return fail("bad_scene", "{} is not valid JSON: {}", scene_path.string(), json_problem(text));
    POCKET_TRY_VOID(world_->load(scene));
    log::info("runtime", "loaded scene {} ({} entities)", scene_rel, world_->entity_count());
    return {};
}

bool Session::context_active(const std::string& name) {
    if (!has_dispatch_) return false;
    auto r = host_->call("__pocket_dispatch", Json::array({"active", name}));
    return r && r->is_boolean() && r->get<bool>();
}

Status Session::load_bundle(const std::filesystem::path& path, const std::string& name) {
    POCKET_TRY(src, fs::read_text(path));
    read_bundle_lines(path);
    // The SDK reads __pocket_bundle while the bundle evaluates and registers its handlers under
    // that context, so several bundles (editor + project) share one script host.
    POCKET_TRY_VOID(host_->evaluate(std::format("globalThis.__pocket_bundle = {};", Json(name).dump()), "<pocket:bundle>"));
    POCKET_TRY_VOID(host_->evaluate(src, path.string()));
    if (std::find(bundle_names_.begin(), bundle_names_.end(), name) == bundle_names_.end()) bundle_names_.push_back(name);
    log::info("runtime", "loaded bundle {} as context '{}'", path.filename().string(), name);
    return {};
}

Json Session::dispatch(const char* kind, Json arg, std::string_view context) {
    if (!has_dispatch_ || !errors_.empty()) return nullptr;
    Json args = Json::array({kind, std::move(arg)});
    if (!context.empty()) args.push_back(std::string(context));
    auto r = host_->call("__pocket_dispatch", args);
    if (!r) {
        record_error(r.error());
        return nullptr;
    }
    return *r;
}

void Session::run_tick() {
    std::int64_t tick = clock_.tick;
    log::global().set_tick(tick);
    world_->set_tick_index(tick);
    // Holds a script asked for during the previous tick press now, so their edge is what this
    // tick's scripts see and a one-tick press lasts through a whole tick.
    if (!pending_holds_.empty()) {
        std::vector<platform::Event> downs;
        for (const PendingHold& h : pending_holds_) {
            if (held_keys_.contains(h.key)) {
                held_keys_[h.key] = std::max(held_keys_[h.key], tick + h.ticks);
                if (!h.press) continue;
                downs.push_back(press_event(h.key, false));
            }
            downs.push_back(press_event(h.key, true));
            held_keys_.try_emplace(h.key, tick + h.ticks);
        }
        pending_holds_.clear();
        if (!downs.empty()) inject_events(std::move(downs));
    }
    if (!pending_events_.empty()) {
        std::vector<platform::Event> due;
        due.swap(pending_events_);
        inject_events(std::move(due));
    }
    in_tick_ = true;
    if (assets_) assets_->set_tile_time(static_cast<std::uint64_t>(static_cast<double>(tick - run_start_) * clock_.tick_seconds * 1000.0));   // the animated tiles' clock
    // The scripts' clock counts from the run's start (a restart begins at tick 0, as a fresh run
    // does); the session's tick, which events, the journal and the recorder go by, carries on.
    Json t;
    t["tick"] = tick - run_start_;
    t["dt"] = clock_.tick_seconds;
    t["time"] = static_cast<double>(tick - run_start_) * clock_.tick_seconds;
    if (input_map_.size() > 0) t["actions"] = input_map_.snapshot();
    t["locale"] = locale_;
    t["locale_rev"] = locale_rev_;
    if (net_) {
        Json players = Json::array({input_map_.snapshot()});
        for (const InputMap& m : player_maps_) players.push_back(m.snapshot());
        t["players"] = std::move(players);
    }
    Stopwatch tick_sw;
    Stopwatch sw;
    dispatch("tick", t);
    perf_script_.add(sw.ms());
    input_map_.consume_edges();
    for (InputMap& m : player_maps_) m.consume_edges();
    in_tick_ = false;
    sw = Stopwatch{};
    // Each system's time on its own as well (perf's systems): the lap since the last mark.
    Stopwatch lap;
    auto mark = [&](System s) { perf_systems_[static_cast<int>(s)].add(lap.ms()); lap = Stopwatch{}; };
    update_terrains();
    timelines_->step(*world_, static_cast<float>(clock_.tick_seconds));   // before physics: a moved platform is where the bodies meet it (docs/design/timelines.md)
    mark(System::Timelines);
    paths_.step(*world_, static_cast<float>(clock_.tick_seconds));        // the same for what follows a path (docs/design/paths.md)
    mark(System::Paths);
    const std::uint64_t before_physics = world_->events().last_seq();
    physics_->step(*world_, clock_.tick_seconds);
    mark(System::Bodies);
    physics_->move_characters(*world_, clock_.tick_seconds);   // after the bodies: platforms have moved (docs/design/physics.md, Characters)
    mark(System::Characters);
    splash_water(before_physics);
    mark(System::Water);
    // Contacts become JSON for the scripts only when one of them listens (onContacts): a pile of
    // resting bodies touches every tick, and nothing need be made of it otherwise.
    if (!physics_->contacts().empty() && dispatch("wants", "contacts") == Json(true)) {
        Json contacts = Json::array();
        for (const auto& c : physics_->contacts()) {
            Json cj;
            cj["a"] = c.a;
            cj["b"] = c.b;
            cj["normal"] = Json{{"x", c.normal.x}, {"y", c.normal.y}, {"z", c.normal.z}};
            cj["point"] = Json{{"x", c.point.x}, {"y", c.point.y}, {"z", c.point.z}};
            cj["depth"] = c.depth;
            cj["trigger"] = c.trigger;
            contacts.push_back(cj);
        }
        dispatch("contacts", contacts);
    }
    mark(System::Contacts);
    if (physics2d_ && assets_) {
        // The platformer bodies stand on the 2D rigid bodies and shove the dynamic ones they walk into.
        const bool rigid = rigid2d_ && rigid2d_->active();
        std::vector<physics::Box2DView> boxes;
        std::vector<std::pair<world::EntityId, Vec2>> pushes;
        if (rigid) boxes = rigid2d_->boxes();
        physics2d_->step(*world_, *assets_, static_cast<float>(clock_.tick_seconds), rigid ? &boxes : nullptr, rigid ? &pushes : nullptr);
        for (const auto& [body, shove] : pushes) {
            if (const auto v = rigid2d_->push(body, shove)) {
                if (const auto* rb = world_->try_get<world::RigidBody2D>(body)) {
                    world::RigidBody2D next = *rb;
                    next.velocity = Vec2{v->x, v->y};
                    world_->set_typed<world::RigidBody2D>(body, next);
                }
            }
        }
    }
    mark(System::Tiles2D);
    if (rigid2d_ && assets_) {
        const Vec3 g = physics_->settings().gravity;
        rigid2d_->step(*world_, *assets_, static_cast<float>(clock_.tick_seconds), Vec2{g.x, g.y});
    }
    mark(System::Bodies2D);
    update_hits(before_physics);   // the touches the 3D and 2D steps just reported
    mark(System::Hits);
    if (!behaviors_) behaviors_ = std::make_unique<Behaviors>();
    // In the XY plane, between the two positions: an orthogonal tile map's cells that hide (solid,
    // or an `opaque` tile), and the 2D bodies but the two's own.
    auto hidden = [this](world::EntityId self, world::EntityId other, Vec3 a, Vec3 b) {
        auto& w = *world_;
        bool hid = false;
        if (assets_) {
            w.ecs().each([&](flecs::entity e, const world::TileMap& tm) {
                if (hid) return;
                auto m = assets_->tilemap(tm.map);
                if (!m || !(*m)->orthogonal()) return;
                const double ts = tm.tile_size > 0 ? tm.tile_size : 1.0;
                Vec3 origin{0, 0, 0};
                if (const auto* wt = e.try_get<world::WorldTransform>()) origin = wt->position;
                const assets::TileMap& map = **m;
                hid = !map.line_clear((a.x - origin.x) / ts, (origin.y - a.y) / ts, (b.x - origin.x) / ts, (origin.y - b.y) / ts, [&](int x, int y) { return map.hides(x, y); });
            });
        }
        if (hid || !rigid2d_ || !rigid2d_->active()) return hid;
        auto own = [&](world::EntityId e, world::EntityId of) { return e == of || w.parent(e) == of; };
        Vec2 from{a.x, a.y};
        const Vec2 to{b.x, b.y};
        for (int i = 0; i < 16; ++i) {   // past its own shapes and the map's tiles (looked at above)
            const auto hit = rigid2d_->raycast(from, to);
            if (!hit || own(hit->entity, other)) return false;
            if (!own(hit->entity, self) && !w.try_get<world::TileMap>(hit->entity)) return true;
            const Vec2 d{to.x - from.x, to.y - from.y};
            const float len = std::sqrt(d.x * d.x + d.y * d.y);
            if (len < 1e-4f) return false;
            from = Vec2{hit->point.x + d.x / len * 1e-3f, hit->point.y + d.y / len * 1e-3f};
        }
        return false;
    };
    behaviors_->step(*world_, physics_.get(), static_cast<float>(clock_.tick_seconds), options_.seed, hidden);   // after the hits, before the agents move
    mark(System::Behaviors);
    nav_.step(*world_, static_cast<float>(clock_.tick_seconds));  // obstacles, then the agents (docs/design/navigation.md)
    mark(System::Navigation);
    update_camera_rigs(static_cast<float>(clock_.tick_seconds));   // after everything that moves what they follow
    mark(System::Cameras);
    update_day(static_cast<float>(clock_.tick_seconds));
    update_weather(static_cast<float>(clock_.tick_seconds));
    perf_physics_.add(sw.ms());
    sw = Stopwatch{};
    world_->tick(clock_.tick_seconds);
    mark(System::World);
    particles_->step(*world_, static_cast<float>(clock_.tick_seconds));
    mark(System::Particles);
    if (assets_) animation_->step(*world_, *assets_, static_cast<float>(clock_.tick_seconds));
    update_footprints(static_cast<float>(clock_.tick_seconds));   // after the gait has planted this tick's feet
    update_water_rings(static_cast<float>(clock_.tick_seconds));
    mark(System::Animation);
    if (assets_ && physics_) {
        if (!ragdolls_) ragdolls_ = std::make_unique<Ragdolls>();
        ragdolls_->step(*world_, *assets_, *animation_, *physics_);   // after the clips: a limp body's pose is its bodies'
    }
    mark(System::Ragdolls);
    update_attachments();   // after the poses: what is held follows the joint as it is now
    mark(System::Attachments);
    // Cloth after everything that places its entity: the world transforms, the poses and what is
    // attached to a joint (a cape on a character's back moves with the spine).
    if (assets_ && physics_) cloth_.step(*world_, *assets_, static_cast<float>(clock_.tick_seconds), physics_->settings().gravity);
    mark(System::Cloth);
    if (audio_) tick_audio(clock_.tick_seconds);
    mark(System::Audio);
    if (ui_) ui_->advance(static_cast<float>(clock_.tick_seconds));   // the interface's transitions run on the simulation clock
    mark(System::Interface);
    if (recorder_.recording()) recorder_.record(*world_, tick);
    mark(System::Recorder);
    perf_world_.add(sw.ms());
    sw = Stopwatch{};
    Json s = dispatch("state", nullptr);
    if (!s.is_object()) s = Json::object();
    last_state_ = s;
    if (state_history_.size() < 200000) state_history_.push_back(world::StateSample{tick, s});
    if (options_.hash_every_tick) {
        hasher_.i64(tick);
        hasher_.str(s.dump());
        hasher_.u64(world_->hash());
        hasher_.u64(particles_->hash(*world_));
        hasher_.f32(clear_.r);
        hasher_.f32(clear_.g);
        hasher_.f32(clear_.b);
        tick_hashes_.push_back(hasher_.digest());
    }
    perf_state_.add(sw.ms());
    perf_tick_.add(tick_sw.ms());
    clock_.tick++;
    ticks_++;
}

Json Session::PhaseStats::json() const {
    Json j;
    j["avg_ms"] = samples ? total_ms / static_cast<double>(samples) : 0.0;
    j["max_ms"] = max_ms;
    j["last_ms"] = last_ms;
    j["total_ms"] = total_ms;
    j["samples"] = samples;
    return j;
}

Json Session::perf() const {
    Json j;
    j["frames"] = frames_;
    j["ticks"] = ticks_;
    j["elapsed_ms"] = total_.ms();
    j["frame"] = perf_frame_.json();
    j["poll"] = perf_poll_.json();
    j["render"] = perf_render_.json();
    j["tick"] = perf_tick_.json();
    j["script"] = perf_script_.json();
    j["physics"] = perf_physics_.json();
    j["world"] = perf_world_.json();
    j["state"] = perf_state_.json();
    // The tick's systems, the costliest first (what physics and world above are made of).
    static_assert(std::size(kSystemNames) == static_cast<std::size_t>(System::Count));
    std::vector<std::pair<double, int>> order;
    for (int i = 0; i < static_cast<int>(System::Count); ++i) {
        const PhaseStats& s = perf_systems_[i];
        if (s.samples) order.emplace_back(s.total_ms / static_cast<double>(s.samples), i);
    }
    std::stable_sort(order.begin(), order.end(), [](const auto& a, const auto& b) { return a.first > b.first; });
    Json systems = Json::array();
    for (const auto& [avg, i] : order) systems.push_back(Json{{"system", kSystemNames[i]}, {"avg_ms", std::round(avg * 1e4) / 1e4}, {"max_ms", std::round(perf_systems_[i].max_ms * 1e4) / 1e4}});
    j["systems"] = std::move(systems);
    return j;
}

Status Session::run_ticks(int ticks) {
    Stopwatch slice;
    for (int i = 0; i < ticks && errors_.empty(); ++i) {
        if (net_ && !net_tick_ready()) break;
        if (net_ && net_->catching_up() && i > 0 && slice.ms() > 30) break;   // replaying: a slice a frame
        run_tick();
        if (net_) {
            // Every thirtieth tick each peer tells the host what its world is; the host compares.
            if (clock_.tick % 30 == 0) net_->report_hash(clock_.tick - 1, hex64(world_->hash() ^ (particles_->hash(*world_) * 1099511628211ull)));
            net_->release(clock_.tick - 1);
        }
    }
    if (net_) net_pump();
    host_->drain_microtasks();
    return {};
}

// Bytes in and out, and what the network did into the event log (this peer's view: when a player
// joined, left or a desync was found; the game's own logic must not depend on these).
void Session::net_pump() {
    net_->pump();
    for (Json& n : net_->take_notes()) {
        const std::string type = n.value("type", "net");
        n.erase("type");
        world_->events().emit(clock_.tick, type, 0, std::move(n), 0, "net");
    }
}

// Whether the next tick may run: this peer's input for the tick `delay` ahead goes out first (once
// per tick), then every player's input for this tick must be in; it is then applied, each player's
// through their own map, and handed to the scripts tagged with the player.
bool Session::net_tick_ready() {
    net_pump();
    if (!net_->started()) return false;
    const std::int64_t tick = clock_.tick;
    const std::int64_t ahead = tick + net_->delay();
    if (net_committed_ < ahead) {
        for (std::int64_t t = std::max(net_committed_ + 1, static_cast<std::int64_t>(net_->delay())); t <= ahead; ++t) {
            net_->commit(t, t == ahead ? net_queue_ : Json::array());
        }
        net_queue_ = Json::array();
        net_committed_ = ahead;
    }
    if (!net_->ready(tick)) return false;
    Json tagged = Json::array();
    const std::vector<Json> all = net_->inputs(tick);
    for (std::size_t p = 0; p < all.size(); ++p) {
        InputMap* map = p == 0 ? &input_map_ : (p - 1 < player_maps_.size() ? &player_maps_[p - 1] : nullptr);
        if (!map || !all[p].is_array()) continue;
        for (const Json& ev : all[p]) {
            if (!(ev.contains("ui") && ev.value("type", "") == "mouse_down")) map->apply(platform::event_from_json(ev));   // a press its sender's interface took
            Json j = ev;
            j["player"] = p;
            tagged.push_back(std::move(j));
        }
    }
    if (!tagged.empty()) dispatch("input", tagged);
    return true;
}

namespace {

float srgb_to_linear(float c) { return c <= 0.04045f ? c / 12.92f : std::pow((c + 0.055f) / 1.055f, 2.4f); }
Result<std::filesystem::path> inside_dir(const std::filesystem::path& project_dir, const std::string& rel);   // below

}  // namespace

// Water splashes (docs/design/water.md, Splashes): each water.entered event after `since` bursts
// its water's splash emitter where it met the surface, by how fast it came.
void Session::splash_water(std::uint64_t since) {
    // Slower than this is a body set down in the water or lapped by a wave, not a splash.
    constexpr float kReference = 8.0f, kSlowest = 1.0f;
    for (const world::Event& ev : world_->events().since(since, 256, "water.entered")) {
        if (!ev.data.is_object() || !ev.data.contains("point") || !ev.data.contains("speed")) continue;
        const world::EntityId water = world_->find(ev.data.value("water", std::string()));
        const auto* wa = water ? world_->try_get<world::Water>(water) : nullptr;
        if (!wa || wa->splash.empty() || wa->splash_count <= 0) continue;
        const float speed = ev.data["speed"].get<float>();
        if (!(speed >= kSlowest)) continue;
        const world::EntityId emitter = world_->find(wa->splash);
        if (!emitter || !world_->try_get<world::ParticleEmitter>(emitter)) continue;
        const float k = std::min(speed / kReference, 2.0f);
        const int count = std::max(1, static_cast<int>(static_cast<float>(wa->splash_count) * k + 0.5f));
        const Json& pt = ev.data["point"];
        const Vec3 at{pt.value("x", 0.0f), pt.value("y", 0.0f), pt.value("z", 0.0f)};
        (void)particles_->burst(*world_, emitter, count, &at, std::clamp(k, 0.5f, 1.5f));
    }
}

// Terrains (docs/design/terrain.md): heights from the entity's heightmap or its noise, made again
// when those settings change (which drops sculpted edits), meshed again when only the look does.
void Session::update_terrains() {
    if (!assets_) return;
    std::vector<std::pair<world::EntityId, world::Terrain>> list;
    world_->ecs().each([&](flecs::entity e, const world::Terrain& tc) { list.emplace_back(e.id(), tc); });
    std::set<world::EntityId> seen;
    for (auto& [id, tc] : list) {
        seen.insert(id);
        TerrainState& st = terrains_[id];
        const std::string shape = std::format("{}|{}|{:.6g}|{}|{}|{:.6g}x{:.6g}|{:.6g}", tc.heightmap, tc.seed, tc.scale, tc.octaves, tc.resolution, tc.size.x, tc.size.y, tc.height);
        std::string look = std::format("{:.4g},{:.4g},{:.4g}|{:.4g},{:.4g},{:.4g}|{:.4g},{:.4g},{:.4g}|{:.4g}|{:.4g}|{:.4g}", tc.grass.r, tc.grass.g, tc.grass.b, tc.rock.r, tc.rock.g, tc.rock.b, tc.snow.r, tc.snow.g, tc.snow.b, tc.rock_slope, tc.snow_line, tc.texture_tile);
        for (const world::TerrainLayer& ly : tc.layers)
            look += std::format("|{}:{:.4g},{:.4g},{:.4g}:{:.4g}:{:.4g}..{:.4g}:{:.4g}..{:.4g}:{:.4g}", ly.texture, ly.color.r, ly.color.g, ly.color.b, ly.tile, ly.slope.x, ly.slope.y, ly.height.x, ly.height.y, ly.cover);
        bool remesh = false;
        if (st.shape_key != shape) {
            st.shape_key = shape;
            std::vector<std::array<float, 4>> kept = std::move(st.grid.paint), kept_layers = std::move(st.grid.layer_paint);
            const Vec2 size{std::max(tc.size.x, 0.01f), std::max(tc.size.y, 0.01f)};
            const int n = std::clamp(tc.resolution, 2, 1025);
            if (!tc.heightmap.empty()) {
                auto full = inside_dir(options_.project_dir, tc.heightmap);
                auto bytes = full ? fs::read_text(*full) : Result<std::string>(std::unexpected(full.error()));
                auto grid = bytes ? assets::terrain_from_image(*bytes, tc.heightmap, n, size, tc.height) : Result<assets::Terrain>(std::unexpected(bytes.error()));
                if (!grid) {
                    const std::string msg = grid.error().to_string();
                    if (st.error != msg) log::warn("terrain", "{}: {}", world_->path(id), msg);
                    st.error = msg;
                    if (st.grid.h.empty()) st.grid = assets::terrain_from_noise(tc.seed, tc.scale, tc.octaves, n, size, 0.0f);   // flat until the file reads
                } else {
                    st.grid = std::move(*grid);
                    st.error.clear();
                }
            } else {
                st.grid = assets::terrain_from_noise(tc.seed, tc.scale, tc.octaves, n, size, tc.height);
                st.error.clear();
            }
            if (kept.size() == st.grid.h.size()) st.grid.paint = std::move(kept);
            if (kept_layers.size() == st.grid.h.size()) st.grid.layer_paint = std::move(kept_layers);
            st.edited = false;
            remesh = true;
        }
        // The paint: read from the paintmap when it (or the grid's size) changes, kept otherwise, so a
        // new heightmap or a sculpt does not wash it away.
        const std::string paint = std::format("{}|{}", tc.paintmap, st.grid.n);
        if (st.paint_key != paint) {
            st.paint_key = paint;
            st.grid.paint.clear();
            if (!tc.paintmap.empty()) {
                auto full = inside_dir(options_.project_dir, tc.paintmap);
                auto bytes = full ? fs::read_text(*full) : Result<std::string>(std::unexpected(full.error()));
                auto grid = bytes ? assets::terrain_paint_from_image(*bytes, tc.paintmap, st.grid.n) : Result<std::vector<std::array<float, 4>>>(std::unexpected(bytes.error()));
                if (grid) {
                    st.grid.paint = std::move(*grid);
                } else {
                    const std::string msg = grid.error().to_string();
                    if (st.error != msg) log::warn("terrain", "{}: {}", world_->path(id), msg);
                    st.error = msg;
                }
            }
            remesh = true;
        }
        // The layers' paint the same way, from the layermap.
        const std::string layer = std::format("{}|{}", tc.layermap, st.grid.n);
        if (st.layer_key != layer) {
            st.layer_key = layer;
            st.grid.layer_paint.clear();
            if (!tc.layermap.empty()) {
                auto full = inside_dir(options_.project_dir, tc.layermap);
                auto bytes = full ? fs::read_text(*full) : Result<std::string>(std::unexpected(full.error()));
                auto grid = bytes ? assets::terrain_layers_from_image(*bytes, tc.layermap, st.grid.n) : Result<std::vector<std::array<float, 4>>>(std::unexpected(bytes.error()));
                if (grid) {
                    st.grid.layer_paint = std::move(*grid);
                } else {
                    const std::string msg = grid.error().to_string();
                    if (st.error != msg) log::warn("terrain", "{}: {}", world_->path(id), msg);
                    st.error = msg;
                }
            }
            remesh = true;
        }
        if (st.look_key != look) {
            st.look_key = look;
            remesh = true;
        }
        if (remesh) remesh_terrain(id, st, tc);
    }
    for (auto it = terrains_.begin(); it != terrains_.end();) {
        if (seen.contains(it->first)) { ++it; continue; }
        if (!it->second.mesh.empty()) assets_->forget_mesh(it->second.mesh);
        world_->set_derived_mesh(it->first, "");
        it = terrains_.erase(it);
    }
    update_scatters();   // they stand on the ground as it is now
}

void Session::update_scatters() {
    std::vector<std::pair<world::EntityId, world::Scatter>> list;
    world_->ecs().each([&](flecs::entity e, const world::Scatter& sc) { list.emplace_back(e.id(), sc); });
    std::set<world::EntityId> seen;
    if (!list.empty()) {
        // The ground's version: every terrain's revision (a sculpted terrain places its copies again).
        std::uint64_t ground = 0;
        for (const auto& [tid, st] : terrains_) ground = ground * 1000003u + tid * 31u + st.revision;
        for (auto& [id, sc] : list) {
            seen.insert(id);
            const auto* wt = world_->try_get<world::WorldTransform>(id);
            if (!wt) {
                world_->update_transforms();   // spawned since the last tick: placed at once all the same
                wt = world_->try_get<world::WorldTransform>(id);
            }
            if (!wt) continue;
            const std::string key = std::format("{}|{:.5g}x{:.5g}|{}|{}|{:.4g}..{:.4g}|{:.4g}|{:.4g}|{:.4g}|{:.4g}|{:.4g}|{:.5g}..{:.5g}|{:.4g}|{:.4g}|{:.5g},{:.5g},{:.5g}|{:.4g},{:.4g},{:.4g}|{}",
                sc.count, sc.area.x, sc.area.y, sc.seed, sc.on, sc.scale.x, sc.scale.y, sc.yaw, sc.align, sc.sink, sc.spacing, sc.max_slope, sc.min_height, sc.max_height, sc.max_paint, sc.shade,
                wt->position.x, wt->position.y, wt->position.z, wt->scale.x, wt->scale.y, wt->scale.z, ground);
            if (scatter_keys_[id] == key) continue;
            scatter_keys_[id] = key;
            // What they stand on: a terrain's grid when `on` names one (exact and quick), otherwise the
            // first static collider under each place (only `on`'s when it names another entity).
            world::EntityId target = 0;
            if (!sc.on.empty()) target = resolve_entity(Json(sc.on));
            const TerrainState* land = target && terrains_.contains(target) ? &terrains_[target] : nullptr;
            const world::WorldTransform* land_t = land ? world_->try_get<world::WorldTransform>(target) : nullptr;
            const Mat4 land_m = land_t ? Mat4::trs(land_t->position, land_t->rotation, land_t->scale) : Mat4::identity();
            const Mat4 land_inv = land_m.inverse_affine();
            const physics::Physics::Filter below = [&](world::EntityId other, const world::RigidBody& rb, const world::Collider& col) {
                return rb.kind == 1 && !col.is_trigger && (target == 0 || other == target) && !world_->ecs().entity(other).has<world::Scatter>();   // not on copies
            };
            // One stream from the seed, five numbers a place whether it is kept or not, so the same
            // seed and settings always place the same copies.
            std::uint64_t state = 0x9E3779B97F4A7C15ull ^ (static_cast<std::uint64_t>(sc.seed) * 0xBF58476D1CE4E5B9ull);
            auto next = [&]() {
                state += 0x9E3779B97F4A7C15ull;
                std::uint64_t z = state;
                z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
                z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
                z ^= z >> 31;
                return static_cast<float>(z >> 40) / static_cast<float>(1ull << 24);
            };
            const float cos_max = repro::cos(std::clamp(sc.max_slope, 0.0f, 90.0f) * std::numbers::pi_v<float> / 180.0f);
            const float cell = std::max(sc.spacing, 1e-3f);
            std::unordered_map<std::int64_t, std::vector<Vec2>> near;
            std::vector<world::World::Instance> copies;
            const int tries = std::clamp(sc.count, 0, 20000);
            for (int k = 0; k < tries; ++k) {
                const float rx = next(), rz = next(), ryaw = next(), rscale = next(), rshade = next();
                const float x = wt->position.x + (rx - 0.5f) * sc.area.x, z = wt->position.z + (rz - 0.5f) * sc.area.y;
                Vec3 point, normal;
                if (land) {
                    const Vec3 l = land_inv.transform_point(Vec3{x, 0, z});
                    if (std::fabs(l.x) > land->grid.size_x * 0.5f || std::fabs(l.z) > land->grid.size_z * 0.5f) continue;
                    if (sc.max_paint < 1.0f) {
                        // A painted path kept clear, painted in a colour or with a textured layer.
                        const auto layers = land->grid.grid_at(land->grid.layer_paint, l.x, l.z);
                        if (std::max(land->grid.paint_at(l.x, l.z)[3], layers[0] + layers[1] + layers[2] + layers[3]) > sc.max_paint) continue;
                    }
                    point = land_m.transform_point(Vec3{l.x, land->grid.sample(l.x, l.z), l.z});
                    const Vec3 n = land->grid.normal(l.x, l.z);
                    normal = land_t ? normalize(land_t->rotation.rotate(Vec3{n.x / land_t->scale.x, n.y / std::max(std::fabs(land_t->scale.y), 1e-6f), n.z / land_t->scale.z})) : n;
                } else {
                    auto hit = physics_->raycast(*world_, Vec3{x, wt->position.y + 1000.0f, z}, Vec3{0, -1, 0}, 3000.0f, below);
                    if (!hit) continue;
                    point = hit->point;
                    normal = hit->normal;
                }
                if (normal.y < cos_max || point.y < sc.min_height || point.y > sc.max_height) continue;
                if (sc.spacing > 0) {
                    const auto cx = static_cast<std::int64_t>(std::floor(x / cell)), cz = static_cast<std::int64_t>(std::floor(z / cell));
                    bool crowded = false;
                    for (std::int64_t dx = -1; dx <= 1 && !crowded; ++dx) {
                        for (std::int64_t dz = -1; dz <= 1 && !crowded; ++dz) {
                            auto it = near.find((cx + dx) * 1000003 + (cz + dz));
                            if (it == near.end()) continue;
                            for (const Vec2& q : it->second) if (repro::hypot(q.x - x, q.y - z) < sc.spacing) { crowded = true; break; }
                        }
                    }
                    if (crowded) continue;
                    near[cx * 1000003 + cz].push_back(Vec2{x, z});
                }
                // Turned about the vertical, leaned toward the ground's slope by `align`, sized and shaded.
                Quat rot = Quat::from_axis_angle(Vec3{0, 1, 0}, (ryaw - 0.5f) * sc.yaw * std::numbers::pi_v<float> / 180.0f);
                const Vec3 axis = cross(Vec3{0, 1, 0}, normal);
                if (sc.align > 0 && length(axis) > 1e-5f) rot = Quat::from_axis_angle(normalize(axis), repro::acos(std::clamp(normal.y, -1.0f, 1.0f)) * std::clamp(sc.align, 0.0f, 1.0f)) * rot;
                const float size = sc.scale.x + (sc.scale.y - sc.scale.x) * rscale;
                const Vec3 at{point.x, point.y - sc.sink, point.z};
                copies.push_back({Mat4::trs(at, normalize(rot), wt->scale * size), std::max(0.0f, 1.0f + (rshade * 2.0f - 1.0f) * sc.shade)});
            }
            const int placed = static_cast<int>(copies.size());
            world_->set_derived_instances(id, std::move(copies));
            if (sc.placed != placed) {
                world::Scatter next_sc = sc;
                next_sc.placed = placed;
                world_->ecs().entity(id).set<world::Scatter>(next_sc);
            }
        }
    }
    for (auto it = scatter_keys_.begin(); it != scatter_keys_.end();) {
        if (seen.contains(it->first)) { ++it; continue; }
        world_->clear_derived_instances(it->first);
        it = scatter_keys_.erase(it);
    }
}

void Session::remesh_terrain(world::EntityId id, TerrainState& st, const world::Terrain& tc) {
    assets::TerrainLook look;
    auto lin = [](const world::Color4& c) { return Vec3{srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b)}; };
    look.grass = lin(tc.grass);
    look.rock = lin(tc.rock);
    look.snow = lin(tc.snow);
    look.rock_slope = tc.rock_slope;
    look.snow_line = tc.snow_line;
    look.texture_tile = tc.texture_tile;
    for (const world::TerrainLayer& ly : tc.layers) {
        if (look.layers.size() == 4) break;
        look.layers.push_back({ly.texture, lin(ly.color), ly.tile, ly.slope, ly.height, ly.cover});
    }
    st.shares = assets::terrain_layer_weights(st.grid, look);
    const std::string path = std::format("terrain:{}@{}", id, ++st.revision);
    assets::Mesh mesh = assets::terrain_mesh(st.grid, look, path);
    world_->set_mesh_bounds(path, mesh.aabb_min, mesh.aabb_max);
    assets_->put_mesh(path, std::move(mesh));
    if (!st.mesh.empty()) assets_->forget_mesh(st.mesh);
    st.mesh = path;
    world_->set_derived_mesh(id, path);
}

// Maps a saved scene or slot carries (docs/design/tilemaps.md, Maps made by code): those made or
// copied at runtime, which have no file, and with `edited` also those changed since they were read.
Json Session::saved_maps(bool edited) const {
    Json maps = Json::object();
    if (!assets_) return maps;
    for (const assets::TileMap* m : assets_->tilemaps()) {
        if (m->file && !(edited && m->revision > 0)) continue;
        maps[m->path] = Json{{"file", m->file}, {"map", m->to_json()}};
    }
    return maps;
}

Status Session::restore_maps(const Json& maps) {
    if (!assets_ || !maps.is_object()) return {};
    for (const auto& [path, entry] : maps.items()) {
        POCKET_TRY(map, assets::parse_tilemap(entry.value("map", Json::object()).dump(), path));
        map.file = entry.value("file", false);
        if (map.file) map.revision = std::max<std::uint64_t>(map.revision, 1);   // edited since it was read, as it was when saved
        assets_->put_tilemap(std::move(map));
    }
    return {};
}

// A project's post effects from a list (render.post, [render] post): each a shader file in the
// project or code given whole, with its parameters; answered per effect, with the compiler's message
// for one that did not compile (it is left out; the rest run).
Result<Json> Session::set_post_effects(const Json& list) {
    if (!list.is_array()) return fail("bad_args", "effects is a list: [\"effects/crt.wgsl\", {{shader, params}}, {{code, name}}]");
    std::vector<renderer::Renderer::PostEffect> defs;
    for (std::size_t i = 0; i < list.size(); ++i) {
        const Json& e = list[i];
        renderer::Renderer::PostEffect d;
        std::string shader = e.is_string() ? e.get<std::string>() : e.value("shader", std::string());
        if (!shader.empty()) {
            POCKET_TRY(full, inside_dir(options_.project_dir, shader));
            POCKET_TRY(text, fs::read_text(full));
            d.wgsl = text;
            d.name = std::filesystem::path(shader).stem().string();
        } else if (e.is_object() && e.contains("code") && e["code"].is_string()) {
            d.wgsl = e["code"].get<std::string>();
            d.name = std::format("effect{}", i + 1);
        } else {
            return fail("bad_args", "effect {} has neither a shader file nor code", i + 1);
        }
        if (e.is_object()) {
            d.name = e.value("name", d.name);
            d.enabled = e.value("enabled", true);
            if (e.contains("params") && e["params"].is_array())
                for (std::size_t k = 0; k < std::min<std::size_t>(8, e["params"].size()); ++k) d.params[k] = e["params"][k].is_number() ? e["params"][k].get<float>() : 0.0f;
        }
        defs.push_back(std::move(d));
    }
    const std::vector<std::string> errors = renderer_->set_post_effects(defs);
    Json out = Json::array();
    bool all = true;
    for (std::size_t i = 0; i < defs.size(); ++i) {
        Json r{{"name", defs[i].name}, {"ok", errors[i].empty()}};
        if (!errors[i].empty()) {
            r["error"] = errors[i];
            all = false;
        }
        out.push_back(r);
    }
    world_->events().emit(clock_.tick, "render.post", 0, Json{{"effects", defs.size()}, {"ok", all}}, 0, "engine");
    return Json{{"effects", out}, {"ok", all}};
}

// Meshes made by code (docs/design/assets.md, Meshes made by code): vertices and triangles given as
// numbers, kept by the asset store under "mesh:<name>" for a MeshRenderer and a mesh Collider to
// name, their bounds known at once, and the request kept so a saved scene or slot carries them.
Result<Json> Session::make_mesh(const std::string& name, const Json& spec) {
    if (name.empty() || name.size() > 64 || !std::all_of(name.begin(), name.end(), [](char c) { return std::isalnum(static_cast<unsigned char>(c)) || c == '_' || c == '-' || c == '.'; }))
        return fail("bad_args", "a mesh's name is 1 to 64 letters, digits, '_', '-' or '.' (it is drawn as \"mesh:<name>\")");
    // Numbers from a flat list, or a list of [x, y, z] or {x, y, z}.
    auto numbers = [](const Json& v, int per, const char* what, std::vector<float>& out) -> Status {
        out.clear();
        if (v.is_null()) return {};
        if (!v.is_array()) return fail("bad_args", "{} is a list of numbers", what);
        const char* keys[] = {"x", "y", "z", "w"};
        for (const Json& e : v) {
            if (e.is_number()) out.push_back(e.get<float>());
            else if (e.is_array()) for (const Json& c : e) { if (!c.is_number()) return fail("bad_args", "{} holds numbers", what); out.push_back(c.get<float>()); }
            else if (e.is_object()) {
                if (per == 2 && e.contains("u")) { out.push_back(e.value("u", 0.0f)); out.push_back(e.value("v", 0.0f)); continue; }
                if (e.contains("r")) { out.push_back(e.value("r", 1.0f)); out.push_back(e.value("g", 1.0f)); out.push_back(e.value("b", 1.0f)); if (per == 4) out.push_back(e.value("a", 1.0f)); continue; }
                for (int k = 0; k < per; ++k) out.push_back(e.value(keys[k], k == 3 ? 1.0f : 0.0f));
            } else return fail("bad_args", "{} holds numbers, [x, y, z] or {{x, y, z}}", what);
        }
        if (out.size() % static_cast<std::size_t>(per) != 0) return fail("bad_args", "{} has {} numbers, not a multiple of {}", what, out.size(), per);
        for (float f : out) if (!std::isfinite(f)) return fail("bad_args", "{} holds a number that is not finite", what);
        return {};
    };
    std::vector<float> pos, nrm, uv, col;
    POCKET_TRY_VOID(numbers(spec.value("positions", Json()), 3, "positions", pos));
    POCKET_TRY_VOID(numbers(spec.value("normals", Json()), 3, "normals", nrm));
    POCKET_TRY_VOID(numbers(spec.value("uvs", Json()), 2, "uvs", uv));
    const Json colors = spec.value("colors", Json());
    // Colors: three or four numbers a vertex, sRGB like MeshRenderer.color.
    const int per_color = colors.is_array() && !colors.empty() && ((colors[0].is_array() && colors[0].size() == 4) || (colors[0].is_object() && colors[0].contains("a"))) ? 4 : 3;
    POCKET_TRY_VOID(numbers(colors, per_color, "colors", col));
    const std::size_t n = pos.size() / 3;
    if (n < 3) return fail("bad_args", "positions needs at least three vertices (x, y, z each)");
    if (n > 1000000) return fail("bad_args", "a made mesh holds at most a million vertices");
    if (!nrm.empty() && nrm.size() != pos.size()) return fail("bad_args", "normals has {} vertices for {} positions", nrm.size() / 3, n);
    if (!uv.empty() && uv.size() / 2 != n) return fail("bad_args", "uvs has {} vertices for {} positions", uv.size() / 2, n);
    if (!col.empty() && col.size() / static_cast<std::size_t>(per_color) != n) return fail("bad_args", "colors has {} vertices for {} positions", col.size() / static_cast<std::size_t>(per_color), n);
    std::vector<std::uint32_t> idx;
    if (spec.contains("indices") && !spec["indices"].is_null()) {
        if (!spec["indices"].is_array()) return fail("bad_args", "indices is a list of vertex numbers, three a triangle");
        for (const Json& e : spec["indices"]) {
            const auto add = [&](const Json& v) -> Status {
                if (!v.is_number_integer() || v.get<std::int64_t>() < 0 || static_cast<std::size_t>(v.get<std::int64_t>()) >= n) return fail("bad_args", "an index {} is not one of the {} vertices", v.dump(), n);
                idx.push_back(v.get<std::uint32_t>());
                return {};
            };
            if (e.is_array()) for (const Json& v : e) POCKET_TRY_VOID(add(v));
            else POCKET_TRY_VOID(add(e));
        }
    } else {
        for (std::uint32_t i = 0; i < n; ++i) idx.push_back(i);
    }
    if (idx.empty() || idx.size() % 3 != 0) return fail("bad_args", "indices has {} entries; triangles take three each", idx.size());
    assets::Mesh mesh;
    const std::string path = "mesh:" + name;
    mesh.path = path;
    mesh.importer = "code";
    mesh.vertices.resize(n);
    for (std::size_t i = 0; i < n; ++i) {
        assets::MeshVertex& v = mesh.vertices[i];
        v.position = Vec3{pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]};
        if (!uv.empty()) v.uv = Vec2{uv[i * 2], uv[i * 2 + 1]};
        if (!col.empty()) {
            const std::size_t c = i * static_cast<std::size_t>(per_color);
            v.color = Vec4{srgb_to_linear(col[c]), srgb_to_linear(col[c + 1]), srgb_to_linear(col[c + 2]), per_color == 4 ? col[c + 3] : 1.0f};
        }
    }
    if (!nrm.empty()) {
        for (std::size_t i = 0; i < n; ++i) mesh.vertices[i].normal = normalize(Vec3{nrm[i * 3], nrm[i * 3 + 1], nrm[i * 3 + 2]});
    } else {
        // Each vertex's normal from the triangles that share it, weighed by their area: shared
        // vertices round a surface, separate ones keep its faces flat.
        std::vector<Vec3> acc(n, Vec3{0, 0, 0});
        for (std::size_t t = 0; t + 2 < idx.size(); t += 3) {
            const Vec3 a = mesh.vertices[idx[t]].position, b = mesh.vertices[idx[t + 1]].position, c = mesh.vertices[idx[t + 2]].position;
            const Vec3 f = cross(b - a, c - a);
            for (int k = 0; k < 3; ++k) acc[idx[t + k]] = acc[idx[t + k]] + f;
        }
        for (std::size_t i = 0; i < n; ++i) mesh.vertices[i].normal = length(acc[i]) > 1e-12f ? normalize(acc[i]) : Vec3{0, 1, 0};
    }
    mesh.indices = std::move(idx);
    mesh.vertex_colors = !col.empty();
    assets::Material m;
    m.name = name;
    m.double_sided = spec.value("double_sided", false);
    mesh.materials.push_back(m);
    assets::Submesh sm;
    sm.index_count = static_cast<std::uint32_t>(mesh.indices.size());
    mesh.submeshes.push_back(sm);
    mesh.aabb_min = mesh.aabb_max = mesh.vertices[0].position;
    for (const assets::MeshVertex& v : mesh.vertices) {
        mesh.aabb_min = Vec3{std::min(mesh.aabb_min.x, v.position.x), std::min(mesh.aabb_min.y, v.position.y), std::min(mesh.aabb_min.z, v.position.z)};
        mesh.aabb_max = Vec3{std::max(mesh.aabb_max.x, v.position.x), std::max(mesh.aabb_max.y, v.position.y), std::max(mesh.aabb_max.z, v.position.z)};
    }
    assets::add_back_faces(mesh);
    if (!uv.empty()) assets::fill_tangents(mesh);
    const bool again = made_meshes_.contains(name);
    const Vec3 lo = mesh.aabb_min, hi = mesh.aabb_max;
    Json info{{"mesh", path}, {"vertices", n}, {"triangles", mesh.indices.size() / 3}, {"bounds", Json{{"min", Json::array({lo.x, lo.y, lo.z})}, {"max", Json::array({hi.x, hi.y, hi.z})}}}};
    world_->set_mesh_bounds(path, lo, hi);
    assets_->put_mesh(path, std::move(mesh));
    if (again) {
        // The same name again: what was drawn and collided from the old one is let go.
        if (renderer_) renderer_->drop_asset_cache();
        if (physics_) physics_->drop_mesh_cache();
    }
    Json kept = Json::object();
    for (const char* k : {"positions", "indices", "normals", "uvs", "colors", "double_sided"}) if (spec.contains(k) && !spec[k].is_null()) kept[k] = spec[k];
    made_meshes_[name] = std::move(kept);
    return info;
}

Result<Json> Session::mesh_command(std::string_view op, const Json& p) {
    if (op == "create") {
        POCKET_TRY(info, make_mesh(opt<std::string>(p, "name", ""), p));
        world_->events().emit(clock_.tick, "mesh.created", 0, Json{{"mesh", info["mesh"]}, {"vertices", info["vertices"]}, {"triangles", info["triangles"]}}, 0, "engine");
        return info;
    }
    if (op == "list") {
        Json all = Json::array();
        for (const auto& [name, spec] : made_meshes_) {
            const std::size_t verts = spec.contains("positions") ? spec["positions"].size() : 0;
            all.push_back(Json{{"mesh", "mesh:" + name}, {"entries", Json{{"positions", verts}, {"indices", spec.contains("indices") ? spec["indices"].size() : 0}}}});
        }
        return Json{{"meshes", all}};
    }
    if (op == "remove") {
        const std::string name = opt<std::string>(p, "name", "");
        if (!made_meshes_.erase(name)) return fail("no_such_mesh", "no made mesh named '{}' (mesh.list)", name);
        assets_->forget_mesh("mesh:" + name);
        if (renderer_) renderer_->drop_asset_cache();
        if (physics_) physics_->drop_mesh_cache();
        return Json{{"removed", "mesh:" + name}};
    }
    return fail("unknown_command", "mesh.{} is not a command (create, list, remove)", op);
}

Result<Json> Session::net_command(std::string_view op, const Json& p) {
    (void)p;
    if (op == "info") {
        if (!net_) return Json{{"mode", "off"}, {"player", 0}, {"players", 1}, {"started", true}};
        Json j = net_->info();
        j["tick"] = clock_.tick;
        j["waiting"] = !net_->started() || !net_->ready(clock_.tick);
        j["queued"] = net_queue_.size();
        return j;
    }
    return fail("unknown_command", "unknown net command '{}'", op);
}

// Localization (docs/design/localization.md): the project's words, one file per language in
// locales/ (nested objects flattened to dotted keys); the language scripts' `t` reads.
Result<Json> Session::locale_command(std::string_view op, const Json& p) {
    const std::filesystem::path dir = options_.project_dir / "locales";
    auto languages = [&]() {
        std::vector<std::string> out;
        std::error_code ec;
        if (std::filesystem::is_directory(dir, ec))
            for (const auto& f : std::filesystem::directory_iterator(dir, ec)) if (f.path().extension() == ".json") out.push_back(f.path().stem().string());
        std::sort(out.begin(), out.end());
        return out;
    };
    auto load = [&](const std::string& lang) -> Result<std::map<std::string, std::string>> {
        const std::filesystem::path file = dir / (lang + ".json");
        if (lang.empty() || lang.find('/') != std::string::npos || lang.find("..") != std::string::npos || !std::filesystem::exists(file)) {
            std::string have;
            for (const auto& l : languages()) have += (have.empty() ? "" : ", ") + l;
            return fail("no_locale", "no locales/{}.json (the project has: {})", lang, have.empty() ? "none" : have);
        }
        POCKET_TRY(text, fs::read_text(file));
        Json doc = Json::parse(text, nullptr, false);
        if (doc.is_discarded() || !doc.is_object()) return fail("bad_locale", "locales/{}.json is not a JSON object of texts", lang);
        std::map<std::string, std::string> out;
        std::string bad;
        std::function<void(const Json&, const std::string&)> walk = [&](const Json& j, const std::string& prefix) {
            for (auto it = j.begin(); it != j.end(); ++it) {
                const std::string key = prefix.empty() ? it.key() : prefix + "." + it.key();
                if (it->is_object()) walk(*it, key);
                else if (it->is_string()) out[key] = it->get<std::string>();
                else if (bad.empty()) bad = key;
            }
        };
        walk(doc, "");
        if (!bad.empty()) return fail("bad_locale", "locales/{}.json: '{}' is not a text (texts are strings, groups are objects)", lang, bad);
        return out;
    };
    auto list = [](const std::vector<std::string>& v) { Json a = Json::array(); for (const auto& s : v) a.push_back(s); return a; };
    if (op == "get") return Json{{"lang", locale_}, {"default", locale_default_}, {"languages", list(languages())}};
    if (op == "set") {
        if (!p.contains("lang")) return fail("bad_args", "locale.set needs a 'lang'");
        const std::string lang = opt<std::string>(p, "lang", "");
        POCKET_TRY(table, load(lang));
        locale_ = lang;
        world_->events().emit(world_->tick_index(), "locale.changed", 0, Json{{"lang", lang}});
        return Json{{"lang", lang}, {"keys", table.size()}, {"languages", list(languages())}};
    }
    if (op == "table") {
        const std::string lang = p.contains("lang") ? opt<std::string>(p, "lang", "") : locale_;
        POCKET_TRY(table, load(lang));
        Json strings = Json::object();
        for (const auto& [k, v] : table) strings[k] = v;
        return Json{{"lang", lang}, {"keys", table.size()}, {"strings", strings}};
    }
    if (op == "check") {
        // Every language against the base: keys it lacks, keys only it has, and texts whose
        // placeholders ({name}) differ from the base's.
        const std::string base = p.contains("base") ? opt<std::string>(p, "base", "") : locale_default_;
        POCKET_TRY(ref, load(base));
        auto names = [](const std::string& text) {
            std::set<std::string> out;
            for (std::size_t i = text.find('{'); i != std::string::npos; i = text.find('{', i + 1)) {
                std::size_t j = i + 1;
                while (j < text.size() && (std::isalnum(static_cast<unsigned char>(text[j])) || text[j] == '_')) ++j;
                if (j > i + 1 && j < text.size() && (text[j] == '}' || text[j] == ',')) out.insert(text.substr(i + 1, j - i - 1));
            }
            return out;
        };
        Json result = Json::object();
        bool complete = true;
        for (const std::string& lang : languages()) {
            if (lang == base) continue;
            auto table = load(lang);
            if (!table) { result[lang] = Json{{"error", table.error().message}}; complete = false; continue; }
            Json missing = Json::array(), extra = Json::array(), placeholders = Json::array();
            for (const auto& [k, v] : ref) {
                auto it = table->find(k);
                if (it == table->end()) missing.push_back(k);
                else if (names(it->second) != names(v)) placeholders.push_back(k);
            }
            for (const auto& [k, v] : *table) if (!ref.contains(k)) extra.push_back(k);
            if (!missing.empty() || !placeholders.empty()) complete = false;
            result[lang] = Json{{"keys", table->size()}, {"missing", missing}, {"extra", extra}, {"placeholders", placeholders}};
        }
        return Json{{"base", base}, {"keys", ref.size()}, {"languages", result}, {"complete", complete}};
    }
    return fail("unknown_command", "locale.{} is not a command (get, set, table, check)", op);
}

// Camera rigs (docs/design/cameras.md): each camera with a CameraRig is moved with its target after
// the bodies, the characters and the agents moved: to where its mode puts it, eased there, in
// front of walls between it and the pivot, looking at the pivot, shaken by its trauma.
// A day (docs/design/rendering.md, A day): the first enabled Sky with a time of day runs it on by its
// day length and stands the first directional light where the sun is at that hour: up in the east at
// 6, at `sun_height` degrees to the south (+z) at 12, down in the west at 18, under the ground at
// night (an atmosphere goes dark with it). repro's sine and cosine: the same sun on every machine.
void Session::update_day(float dt) {
    world::EntityId sky_id = 0;
    world::Sky sky;
    world_->ecs().each([&](flecs::entity e, const world::Sky& s) {
        if (!sky_id && s.enabled && s.time_of_day >= 0.0f) { sky_id = e.id(); sky = s; }
    });
    if (!sky_id) return;
    if (sky.day_length > 0.0f) {
        sky.time_of_day = std::fmod(sky.time_of_day + dt * 24.0f / sky.day_length, 24.0f);
        world_->set_typed<world::Sky>(sky_id, sky);
    }
    world::EntityId sun = 0;
    world_->ecs().each([&](flecs::entity e, const world::Light& l) {
        if (!sun && l.kind == 0) sun = e.id();
    });
    const auto* t = sun ? world_->try_get<world::Transform>(sun) : nullptr;
    if (!t) return;
    const float a = (std::fmod(sky.time_of_day, 24.0f) - 6.0f) / 12.0f * kPi;   // 0 at sunrise, pi at sunset
    const float tilt = std::clamp(sky.sun_height, 1.0f, 90.0f) * kPi / 180.0f;
    const Vec3 to_sun = normalize(Vec3{repro::cos(a), repro::sin(a) * repro::sin(tilt), repro::sin(a) * repro::cos(tilt)});
    // The light's -Z along the sunlight (away from the sun): the turn that takes -Z there.
    const Vec3 from{0, 0, -1}, to = to_sun * -1.0f;
    const float d = dot(from, to);
    Quat q;
    if (d < -0.9999f) {
        q = Quat{0, 1, 0, 0};
    } else {
        const Vec3 c = cross(from, to);
        q = normalize(Quat{c.x, c.y, c.z, 1.0f + d});
    }
    if (!(q == t->rotation)) {
        world::Transform nt = *t;
        nt.rotation = q;
        world_->set_typed<world::Transform>(sun, nt);
    }
}

// The weather's lasting part (docs/design/rendering.md, Weather): the first enabled Weather's
// wetness runs toward its rain (up in 15 seconds, down in 90) and its snow lying builds while it
// snows (in 40 seconds at full) and melts in four minutes once it stops, faster in rain; both are
// written back, so they are saved, hashed and read like any other field.
void Session::update_weather(float dt) {
    world::EntityId id = 0;
    world::Weather w;
    world_->ecs().each([&](flecs::entity e, const world::Weather& x) {
        if (x.enabled && (!id || e.id() < id)) { id = e.id(); w = x; }
    });
    if (!id) return;
    const float rain = std::clamp(w.rain, 0.0f, 1.0f), snow = std::clamp(w.snow, 0.0f, 1.0f);
    float wet = std::clamp(w.wet, 0.0f, 1.0f), cover = std::clamp(w.cover, 0.0f, 1.0f);
    wet = wet < rain ? std::min(rain, wet + dt / 15.0f) : std::max(rain, wet - dt / 90.0f);
    if (snow > 0) cover = std::min(1.0f, cover + dt * snow / 40.0f);
    else cover = std::max(0.0f, cover - dt * (1.0f / 240.0f + rain / 30.0f));
    // Lightning: a flash every few seconds in a full storm (four to fourteen, drawn from the run's
    // seed and the tick, so a replay storms alike), each two pulses over a third of a second, and its
    // thunder one to three seconds after, as loud as the storm.
    const float storm = std::clamp(w.storm, 0.0f, 1.0f);
    float flash = 0;
    if (storm > 0) {
        auto draw = [&](std::uint64_t salt) {
            std::uint64_t h = (options_.seed + 0x9e3779b97f4a7c15ull) ^ (static_cast<std::uint64_t>(clock_.tick) * 0xbf58476d1ce4e5b9ull) ^ salt;
            h = (h ^ (h >> 31)) * 0x94d049bb133111ebull;
            return static_cast<float>((h ^ (h >> 29)) >> 40) / static_cast<float>(1ull << 24);
        };
        if (storm_wait_ < 0) storm_wait_ = (2.0f + 6.0f * draw(1)) / storm;
        storm_wait_ -= dt;
        if (storm_wait_ <= 0) {
            flash_age_ = 0;
            storm_wait_ = (4.0f + 10.0f * draw(2)) / storm;
            thunder_in_ = 1.0f + 2.0f * draw(3);
            thunder_volume_ = 0.5f + 0.4f * storm;
            world_->events().emit(clock_.tick, "weather.lightning", id, Json{{"storm", storm}, {"thunder_in", thunder_in_}}, 0, "engine");
        }
        flash_age_ += dt;
        const float a = flash_age_;
        flash = a < 0.06f ? 1.0f : a < 0.12f ? 0.25f : a < 0.18f ? 0.85f : a < 0.36f ? 0.85f * (1.0f - (a - 0.18f) / 0.18f) : 0.0f;
    } else {
        storm_wait_ = -1;
        flash_age_ = 99;
    }
    if (thunder_in_ >= 0) {
        thunder_in_ -= dt;
        if (thunder_in_ < 0 && audio_ && w.sound) {
            audio::PlayOptions o;
            o.volume = thunder_volume_;
            o.tag = "thunder";
            (void)audio_->play("sfx:thunder?seed=" + std::to_string(clock_.tick % 7), o);
        }
    }
    if (wet != w.wet || cover != w.cover || flash != w.flash) {
        w.wet = wet;
        w.cover = cover;
        w.flash = flash;
        world_->set_typed<world::Weather>(id, w);
    }
}

// Footprints in lying snow (docs/design/rendering.md, Weather): each footfall (animation.footstep)
// where snow lies leaves a print a little to its side of the walker, along its heading; prints fill
// in over a minute and a half, faster while it snows, and the newest 64 are kept. Render-only: the
// world and its hash do not hold them.
void Session::update_footprints(float dt) {
    world::World& w = *world_;
    world::EntityId wid = 0;
    world::Weather wx;
    w.ecs().each([&](flecs::entity e, const world::Weather& x) {
        if (x.enabled && (!wid || e.id() < wid)) { wid = e.id(); wx = x; }
    });
    const float fill = dt * (1.0f / 90.0f + std::clamp(wx.snow, 0.0f, 1.0f) / 15.0f);
    for (auto& f : footprints_) f.depth -= fill;
    std::erase_if(footprints_, [](const renderer::Renderer::Footprint& f) { return f.depth <= 0; });
    if (!wid || wx.cover < 0.15f) {
        footprints_seen_ = w.events().last_seq();
        if (!wid) footprints_.clear();
    } else {
        for (const world::Event& e : w.events().since(footprints_seen_, 256, "animation.footstep")) {
            footprints_seen_ = e.seq;
            const auto* wt = w.alive(e.subject) ? w.try_get<world::WorldTransform>(e.subject) : nullptr;
            if (!wt) continue;
            const Vec3 fwd = wt->rotation.rotate({0, 0, -1});
            const float len = std::hypot(fwd.x, fwd.z);
            if (len < 1e-4f) continue;
            const float fx = fwd.x / len, fz = fwd.z / len;
            const float side = e.data.value("foot", "") == "left" ? -0.11f : 0.11f;   // right of the heading is (-fz, fx)
            footprints_.push_back({wt->position.x - fz * side, wt->position.z + fx * side, std::atan2(fx, fz), std::min(1.0f, wx.cover)});
        }
        footprints_seen_ = std::max(footprints_seen_, w.events().last_seq());
        if (footprints_.size() > 64) footprints_.erase(footprints_.begin(), footprints_.end() - 64);
    }
    if (renderer_) renderer_->set_footprints(footprints_);
}

// Rings on water (docs/design/water.md, Rings): a strong one where something falls in (each
// water.entered, by how fast it came), and a wake of small ones behind what moves through the water
// (a dynamic body or a character at the surface moving faster than half a unit a second, one each
// sixty centimetres or so); each spreads for three and a half seconds, the newest 32 drawn.
// Render-only, like footprints.
void Session::update_water_rings(float dt) {
    world::World& w = *world_;
    for (auto& r : water_rings_) r.age += dt;
    std::erase_if(water_rings_, [](const renderer::Renderer::WaterRing& r) { return r.age > 3.5f; });
    struct Pool { world::Water water; Vec3 center; };
    std::vector<Pool> pools;
    w.ecs().each([&](flecs::entity, const world::Water& wa, const world::WorldTransform& t) {
        if (wa.enabled && wa.size.x > 0 && wa.size.y > 0) pools.push_back({wa, t.position});
    });
    if (pools.empty()) {
        water_rings_.clear();
        wakes_.clear();
    } else {
        for (const world::Event& ev : w.events().since(water_rings_seen_, 256, "water.entered")) {
            water_rings_seen_ = ev.seq;
            if (!ev.data.is_object() || !ev.data.contains("point")) continue;
            const Json& pt = ev.data["point"];
            const float speed = ev.data.value("speed", 2.0f);
            water_rings_.push_back({pt.value("x", 0.0f), pt.value("z", 0.0f), 0.0f, std::clamp(speed / 5.0f, 0.4f, 1.5f)});
        }
        const auto time = static_cast<float>(w.seconds());
        std::set<world::EntityId> seen;
        auto wake = [&](world::EntityId id, Vec3 at) {
            seen.insert(id);
            for (const Pool& pool : pools) {
                if (!world::water_covers(pool.water, pool.center, at.x, at.z)) continue;
                const float level = world::water_at(pool.water, pool.center.y, at.x, at.z, time).position.y;
                if (std::fabs(at.y - level) > 1.0f) continue;   // under it, or above it
                auto [it, fresh] = wakes_.try_emplace(id, std::pair{at, 0.0f});
                it->second.second += dt;
                const float moved = std::hypot(at.x - it->second.first.x, at.z - it->second.first.z);
                if (!fresh && moved > 0.6f && moved / std::max(it->second.second, 1e-3f) > 0.5f) {
                    water_rings_.push_back({at.x, at.z, 0.0f, 0.35f});
                    it->second = {at, 0.0f};
                } else if (fresh || it->second.second > 1.5f) {
                    it->second = {at, 0.0f};
                }
                return;
            }
        };
        w.ecs().each([&](flecs::entity e, const world::RigidBody& rb, const world::WorldTransform& t) {
            if (rb.kind == 0) wake(e.id(), t.position);   // dynamic
        });
        w.ecs().each([&](flecs::entity e, const world::Character&, const world::WorldTransform& t) { wake(e.id(), t.position); });
        for (auto it = wakes_.begin(); it != wakes_.end();) it = seen.contains(it->first) ? std::next(it) : wakes_.erase(it);
        if (water_rings_.size() > 32) water_rings_.erase(water_rings_.begin(), water_rings_.end() - 32);
    }
    water_rings_seen_ = std::max(water_rings_seen_, w.events().last_seq());
    if (renderer_) renderer_->set_water_rings(water_rings_);
}

void Session::update_camera_rigs(float dt) {
    std::vector<world::EntityId> ids;
    world_->ecs().each([&](flecs::entity e, const world::CameraRig&, const world::Transform&) { ids.push_back(e.id()); });
    std::sort(ids.begin(), ids.end());
    for (auto it = rig_base_.begin(); it != rig_base_.end();) it = std::binary_search(ids.begin(), ids.end(), it->first) ? std::next(it) : rig_base_.erase(it);
    for (auto it = rig_motion_.begin(); it != rig_motion_.end();) it = std::binary_search(ids.begin(), ids.end(), it->first) ? std::next(it) : rig_motion_.erase(it);
    if (ids.empty()) return;
    constexpr float kDeg = std::numbers::pi_v<float> / 180.0f;
    Json actions;
    auto action = [&](const std::string& name) {
        if (name.empty()) return 0.0f;
        if (actions.is_null()) actions = input_map_.snapshot();
        return actions.contains(name) ? actions[name].value("value", 0.0f) : 0.0f;
    };
    const auto time = static_cast<float>(world_->seconds());
    for (world::EntityId id : ids) {
        flecs::entity e = world_->ecs().entity(id);
        world::CameraRig rig = e.get<world::CameraRig>();
        world::Transform tr = e.get<world::Transform>();
        // The target and those framed with it: where each is now (its Transform for a root, already
        // moved this tick, else the world transform of the tick before).
        std::vector<world::EntityId> group;
        if (!rig.target.empty()) group.push_back(world_->find(rig.target));
        for (std::size_t from = 0; from < rig.targets.size();) {
            std::size_t comma = rig.targets.find(',', from);
            if (comma == std::string::npos) comma = rig.targets.size();
            std::string name = rig.targets.substr(from, comma - from);
            name.erase(0, name.find_first_not_of(' '));
            name.erase(name.find_last_not_of(' ') + 1);
            if (!name.empty()) group.push_back(world_->find(name));
            from = comma + 1;
        }
        std::erase_if(group, [&](world::EntityId g) { return g == 0 || g == id; });
        if (group.empty()) continue;
        const world::EntityId target = group.front();
        auto place = [&](world::EntityId g, Vec3& p, Quat& q) {
            if (world_->parent(g) == 0) {
                const auto* t = world_->try_get<world::Transform>(g);
                if (!t) return false;
                p = t->position;
                q = t->rotation;
            } else {
                const auto* t = world_->try_get<world::WorldTransform>(g);
                if (!t) return false;
                p = t->position;
                q = t->rotation;
            }
            return true;
        };
        Vec3 at;
        Quat turned;
        if (!place(target, at, turned)) continue;
        // Several: their middle, and how far it must stand back for all of them to be in view.
        float fit = 0;
        if (group.size() > 1) {
            std::vector<Vec3> spots;
            Vec3 sum{0, 0, 0};
            for (world::EntityId g : group) {
                Vec3 p;
                Quat q;
                if (!place(g, p, q)) continue;
                spots.push_back(p);
                sum += p;
            }
            if (!spots.empty()) {
                at = sum * (1.0f / static_cast<float>(spots.size()));
                float reach = 0;
                for (const Vec3& p : spots) reach = std::max(reach, length(p - at));
                reach += std::max(rig.margin, 0.0f);
                float vfov = 60.0f;
                if (const auto* cam = e.try_get<world::Camera>()) vfov = cam->fov_degrees;
                const float aspect = device_ && device_->height() > 0 ? static_cast<float>(device_->width()) / static_cast<float>(device_->height()) : 16.0f / 9.0f;
                const float half_v = std::clamp(vfov, 1.0f, 170.0f) * 0.5f * kDeg;
                const float half_h = repro::atan(repro::tan(half_v) * aspect);
                fit = reach / std::max(repro::sin(std::min(half_v, half_h)), 0.05f);
            }
        }
        const bool fresh = !rig_base_.contains(id);
        auto ease = [&](float seconds) { return seconds > 0 ? 1.0f - repro::exp(-dt / seconds) : 1.0f; };
        // Looking ahead: the middle's velocity, eased over a third of a second, times look_ahead; a
        // jump faster than 30 units a second (a respawn, a teleport, a change of targets) is not a
        // motion to lead.
        Vec3 lead{0, 0, 0};
        {
            auto& [last, vel] = rig_motion_[id];
            if (!fresh && dt > 0) {
                const Vec3 raw = (at - last) * (1.0f / dt);
                vel = length(raw) > 30.0f ? Vec3{0, 0, 0} : vel + (raw - vel) * ease(0.33f);
            }
            last = at;
            if (rig.look_ahead > 0) lead = Vec3{vel.x, 0, vel.z} * rig.look_ahead;
        }
        const Vec3 pivot = at + Vec3{0, rig.height, 0} + lead;
        Vec3 want;
        if (rig.mode == 3) {
            // Rail: the point of the path nearest the pivot (the targets' middle).
            const world::EntityId rail = rig.rail.empty() ? 0 : world_->find(rig.rail);
            const auto* path = rail ? world_->try_get<world::Path>(rail) : nullptr;
            if (!path || path->points.empty()) continue;
            const auto* placed = world_->try_get<world::WorldTransform>(rail);
            const world::PathCurve curve = world::make_curve(*path, placed ? *placed : world::WorldTransform{});
            Vec3 on = pivot;
            (void)curve.nearest(pivot, &on);
            want = on;
        } else if (rig.mode == 2) {
            const float len = length(rig.offset);
            want = pivot + (len > 1e-4f && fit > len ? rig.offset * (fit / len) : rig.offset);
        } else {
            float yaw = rig.yaw;
            if (rig.mode == 0) {
                const Vec3 f = turned.rotate(Vec3{0, 0, -1});
                const float heading = repro::atan2(-f.x, -f.z) / kDeg;
                float gap = std::fmod(heading - rig.heading + 540.0f, 360.0f) - 180.0f;
                rig.heading = fresh ? heading : rig.heading + gap * ease(rig.turn);
                rig.heading = std::fmod(rig.heading + 540.0f, 360.0f) - 180.0f;
                yaw = rig.heading + rig.yaw;
            } else {
                // Orbit: the actions turn it, the pitch within its limits.
                rig.yaw += action(rig.orbit_x) * rig.orbit_speed * dt;
                rig.yaw = std::fmod(rig.yaw + 540.0f, 360.0f) - 180.0f;
                rig.pitch = std::clamp(rig.pitch + action(rig.orbit_y) * rig.orbit_speed * dt, std::min(rig.pitch_min, rig.pitch_max), std::max(rig.pitch_min, rig.pitch_max));
                yaw = rig.yaw;
            }
            const float y = yaw * kDeg, p = rig.pitch * kDeg;
            const Vec3 look{-repro::sin(y) * repro::cos(p), repro::sin(p), -repro::cos(y) * repro::cos(p)};
            want = pivot - look * std::max(std::max(rig.distance, 0.0f), fit);
        }
        const Vec3 base = fresh ? want : rig_base_[id];
        Vec3 pos = base + (want - base) * ease(rig.follow);
        // In front of what stands between it and the pivot.
        if (rig.collide) {
            const Vec3 d = pos - pivot;
            const float len = length(d);
            if (len > 0.3f) {
                auto hit = physics_->raycast(*world_, pivot, d * (1.0f / len), len, [&](world::EntityId other, const world::RigidBody& rb, const world::Collider& col) {
                    return other != target && other != id && !col.is_trigger && rb.kind != 0 && world_->parent(other) != target;
                });
                if (hit) pos = pivot + d * (std::max(hit->distance - 0.2f, 0.2f) / len);
            }
        }
        rig_base_[id] = pos;
        // Looking at the pivot, trembling by the trauma's square.
        const Vec3 to = pivot - pos;
        float yaw = repro::atan2(-to.x, -to.z), pitch = repro::atan2(to.y, std::sqrt(to.x * to.x + to.z * to.z)), roll = 0;
        const float s = std::clamp(rig.shake, 0.0f, 1.0f);
        if (s > 0) {
            const float k = s * s;
            auto wave = [&](float a, float b, float c) { return 0.6f * repro::sin(time * a + c) + 0.4f * repro::sin(time * b + 2.0f * c); };
            yaw += k * 4.0f * kDeg * wave(37.1f, 23.3f, 0.3f);
            pitch += k * 4.0f * kDeg * wave(31.7f, 19.9f, 1.7f);
            roll += k * 2.0f * kDeg * wave(29.3f, 41.1f, 2.9f);
            pos += Vec3{wave(33.3f, 21.7f, 4.1f), wave(27.9f, 17.3f, 5.3f), wave(35.9f, 25.1f, 6.7f)} * (k * 0.15f);
            rig.shake = std::max(0.0f, s - std::max(rig.shake_decay, 0.0f) * dt);
        }
        tr.position = pos;
        tr.rotation = Quat::from_euler(Vec3{pitch, yaw, roll});
        e.set<world::Transform>(tr);
        e.set<world::CameraRig>(rig);
    }
}

// Timelines (docs/design/timelines.md): play one on an entity, stop it, or read a file and what
// in this world it would move.
Result<Json> Session::timeline_command(std::string_view op, const Json& p) {
    auto& w = *world_;
    if (op == "info") {
        if (!p.contains("path")) return fail("bad_args", "timeline.info needs a 'path'");
        world::EntityId self = p.contains("entity") ? resolve_entity(p["entity"]) : 0;
        return timelines_->info(w, opt<std::string>(p, "path", ""), self);
    }
    if (op != "play" && op != "stop" && op != "seek") return fail("unknown_command", "timeline.{} is not a command (play, stop, seek, info)", op);
    if (!p.contains("entity")) return fail("bad_args", "timeline.{} needs an 'entity'", op);
    const world::EntityId id = resolve_entity(p["entity"]);
    if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
    if (op == "seek") {
        if (!p.contains("time")) return fail("bad_args", "timeline.seek needs a 'time'");
        return timelines_->seek(w, id, opt<float>(p, "time", 0.0f));
    }
    world::Timeline t = w.try_get<world::Timeline>(id) ? *w.try_get<world::Timeline>(id) : world::Timeline{};
    if (op == "stop") {
        t.playing = false;
    } else {
        if (p.contains("path")) t.path = opt<std::string>(p, "path", "");
        if (t.path.empty()) return fail("bad_args", "timeline.play needs a 'path' (the entity has no Timeline yet)");
        POCKET_TRY(info, timelines_->info(w, t.path, id));
        t.time = opt<float>(p, "time", 0.0f);
        t.speed = opt<float>(p, "speed", t.speed);
        t.loop = opt<bool>(p, "loop", t.loop);
        t.playing = true;
        t.finished = false;
        t.error.clear();
        w.ecs().entity(id).set<world::Timeline>(t);
        return Json{{"entity", id}, {"path", t.path}, {"duration", info["duration"]}, {"problems", info["problems"]}};
    }
    w.ecs().entity(id).set<world::Timeline>(t);
    return Json{{"entity", id}, {"time", t.time}};
}

// Water (docs/design/water.md): the surface anywhere, moved by the same waves the renderer draws
// and the physics floats things on.
Result<Json> Session::water_command(std::string_view op, const Json& p) {
    if (op != "height") return fail("unknown_command", "water.{} is not a command (water.height is)", op);
    if (!p.contains("x") || !p.contains("z")) return fail("bad_args", "water.height needs x and z");
    const float x = opt<float>(p, "x", 0.0f), z = opt<float>(p, "z", 0.0f);
    auto vec = [](Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; };
    // The water asked for, or the first by id (enabled) whose extent covers the point.
    world::EntityId id = 0;
    if (p.contains("entity") && !p["entity"].is_null()) {
        id = resolve_entity(p["entity"]);
        if (!id || !world_->try_get<world::Water>(id)) return fail("no_water", "{} has no Water", p["entity"].dump());
    } else {
        std::vector<world::EntityId> ids;
        world_->ecs().each([&](flecs::entity e, const world::Water& wa, const world::WorldTransform& t) {
            if (wa.enabled && world::water_covers(wa, t.position, x, z)) ids.push_back(e.id());
        });
        if (ids.empty()) return Json{{"entity", nullptr}, {"inside", false}};
        id = *std::min_element(ids.begin(), ids.end());
    }
    const world::Water wa = world_->ecs().entity(id).get<world::Water>();
    const world::WorldTransform* wt = world_->try_get<world::WorldTransform>(id);
    const Vec3 c = wt ? wt->position : Vec3{0, 0, 0};
    const world::WaterPoint s = world::water_at(wa, c.y, x, z, static_cast<float>(world_->seconds()));
    return Json{{"entity", id}, {"path", world_->path(id)}, {"height", s.position.y}, {"point", vec(s.position)}, {"normal", vec(s.normal)}, {"velocity", vec(s.velocity)},
                {"level", c.y}, {"bottom", c.y - std::max(wa.depth, 0.0f)}, {"inside", world::water_covers(wa, c, x, z)}};
}

Result<Json> Session::terrain_command(std::string_view op, const Json& p) {
    update_terrains();
    // The terrain asked for, or the first by id; its world transform maps world points to the grid.
    world::EntityId id = 0;
    if (p.contains("entity") && !p["entity"].is_null()) {
        id = resolve_entity(p["entity"]);
        if (!id || !terrains_.contains(id)) return fail("no_terrain", "{} has no Terrain", p["entity"].dump());
    } else if (!terrains_.empty()) {
        id = terrains_.begin()->first;
    } else {
        return fail("no_terrain", "no entity has a Terrain");
    }
    TerrainState& st = terrains_[id];
    const world::Terrain tc = world_->ecs().entity(id).get<world::Terrain>();
    const world::WorldTransform* wt = world_->try_get<world::WorldTransform>(id);
    const Mat4 m = wt ? Mat4::trs(wt->position, wt->rotation, wt->scale) : Mat4::identity();
    const Mat4 inv = m.inverse_affine();
    const float scale_xz = wt ? std::max(std::fabs(wt->scale.x), 1e-6f) : 1.0f, scale_y = wt ? wt->scale.y : 1.0f;
    auto local = [&](float x, float z) { return inv.transform_point(Vec3{x, 0, z}); };
    auto vec = [](Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; };
    auto inside = [&](Vec3 l) { return std::fabs(l.x) <= st.grid.size_x * 0.5f && std::fabs(l.z) <= st.grid.size_z * 0.5f; };
    // The textured layers' shares somewhere, each with its index and name (docs/design/terrain.md, Layers).
    auto layer_shares = [&](const std::array<float, 4>& w) {
        Json arr = Json::array();
        for (std::size_t l = 0; l < std::min<std::size_t>(tc.layers.size(), 4); ++l) arr.push_back(Json{{"layer", l}, {"name", tc.layers[l].name}, {"share", w[l]}});
        return arr;
    };
    if (op == "info") {
        float lo = 1e30f, hi = -1e30f;
        for (float v : st.grid.h) { lo = std::min(lo, v); hi = std::max(hi, v); }
        Json j;
        j["entity"] = id;
        j["path"] = world_->path(id);
        j["size"] = Json{{"x", st.grid.size_x}, {"z", st.grid.size_z}};
        j["height"] = st.grid.height;
        j["resolution"] = st.grid.n;
        j["source"] = tc.heightmap.empty() ? "noise" : tc.heightmap;
        j["edited"] = st.edited;
        std::size_t painted = 0;
        for (const auto& q : st.grid.paint) painted += q[3] > 0.01f ? 1 : 0;
        j["painted"] = st.grid.h.empty() ? 0.0 : static_cast<double>(painted) / static_cast<double>(st.grid.h.size());
        if (!tc.paintmap.empty()) j["paintmap"] = tc.paintmap;
        if (!tc.layers.empty()) {
            Json names = Json::array();
            for (std::size_t l = 0; l < std::min<std::size_t>(tc.layers.size(), 4); ++l) names.push_back(tc.layers[l].name);
            j["layers"] = names;
            if (tc.layers.size() > 4) j["layers_unused"] = tc.layers.size() - 4;
        }
        if (!tc.layermap.empty()) j["layermap"] = tc.layermap;
        j["revision"] = st.revision;
        j["lowest"] = st.grid.h.empty() ? 0.0f : lo;
        j["highest"] = st.grid.h.empty() ? 0.0f : hi;
        if (!st.error.empty()) j["error"] = st.error;
        return j;
    }
    if (op == "height") {
        if (!p.contains("x") || !p.contains("z")) return fail("bad_args", "terrain.height needs x and z");
        const float x = opt<float>(p, "x", 0.0f), z = opt<float>(p, "z", 0.0f);
        const Vec3 l = local(x, z);
        const float h = st.grid.sample(l.x, l.z);
        const Vec3 world_point = m.transform_point(Vec3{l.x, h, l.z});
        Vec3 n = st.grid.normal(l.x, l.z);
        if (wt) n = normalize(wt->rotation.rotate(Vec3{n.x / wt->scale.x, n.y / std::max(std::fabs(wt->scale.y), 1e-6f), n.z / wt->scale.z}));
        Json out{{"entity", id}, {"height", world_point.y}, {"point", vec(world_point)}, {"normal", vec(n)}, {"inside", inside(l)}};
        if (!st.grid.paint.empty()) {
            const auto c = st.grid.paint_at(l.x, l.z);
            out["paint"] = Json{{"r", c[0]}, {"g", c[1]}, {"b", c[2]}, {"a", c[3]}};
        }
        if (!st.shares.empty()) out["layers"] = layer_shares(st.grid.grid_at(st.shares, l.x, l.z));
        return out;
    }
    if (op == "paint") {
        // A brush at (x, z), or dabbed along a stroke through `points`, laying a colour over the
        // ground's own or taking paint away, `amount` at the centre fading to nothing at `radius` (a
        // cosine, as the sculpting brush).
        const bool stroke = p.contains("points");
        if (stroke && (!p["points"].is_array() || p["points"].empty())) return fail("bad_args", "points is an array of {{x, z}}");
        if (!stroke && (!p.contains("x") || !p.contains("z"))) return fail("bad_args", "terrain.paint needs x and z, or points");
        const std::string mode = opt<std::string>(p, "mode", "paint");
        if (mode != "paint" && mode != "erase") return fail("bad_args", "mode is paint or erase");
        // A textured layer (by index or name) instead of a colour: its share of the ground laid on.
        int layer = -1;
        if (p.contains("layer") && !p["layer"].is_null()) {
            const std::size_t count = std::min<std::size_t>(tc.layers.size(), 4);
            if (count == 0) return fail("no_layers", "{} has no textured layers (Terrain.layers)", world_->path(id));
            if (p["layer"].is_number_integer()) layer = p["layer"].get<int>();
            else if (p["layer"].is_string())
                for (std::size_t l = 0; l < count; ++l) if (tc.layers[l].name == p["layer"].get<std::string>()) layer = static_cast<int>(l);
            if (layer < 0 || static_cast<std::size_t>(layer) >= count) {
                Json names = Json::array();
                for (std::size_t l = 0; l < count; ++l) names.push_back(tc.layers[l].name);
                return fail("bad_args", "no layer {} (the layers: {}, or 0 to {})", p["layer"].dump(), names.dump(), count - 1);
            }
        }
        std::array<float, 3> color{0, 0, 0};
        if (mode == "paint" && layer < 0) {
            if (!p.contains("color") || !p["color"].is_object()) return fail("bad_args", "terrain.paint needs a color {{r, g, b}} (sRGB, 0..1)");
            color = {std::clamp(p["color"].value("r", 0.0f), 0.0f, 1.0f), std::clamp(p["color"].value("g", 0.0f), 0.0f, 1.0f), std::clamp(p["color"].value("b", 0.0f), 0.0f, 1.0f)};
        }
        const float radius = std::max(opt<float>(p, "radius", 3.0f), 1e-3f) / scale_xz;
        const float amount = std::clamp(opt<float>(p, "amount", 0.5f), 0.0f, 1.0f);
        assets::Terrain& g = st.grid;
        // The dabs: the point, or every point of the stroke and enough between them (a quarter of
        // the radius apart, at least a cell) that it is one line.
        std::vector<Vec3> dabs;
        if (stroke) {
            Vec3 last{};
            for (std::size_t k = 0; k < p["points"].size(); ++k) {
                const Json& q = p["points"][k];
                if (!q.is_object() || !q.contains("x") || !q.contains("z")) return fail("bad_args", "points[{}] is not {{x, z}}", k);
                const Vec3 at = local(q.value("x", 0.0f), q.value("z", 0.0f));
                if (k > 0) {
                    const float len = repro::hypot(at.x - last.x, at.z - last.z);
                    const float step = std::max(radius * 0.25f, std::min(g.cell_x(), g.cell_z()));
                    for (int s = 1; static_cast<float>(s) * step < len; ++s) dabs.push_back(last + (at - last) * (static_cast<float>(s) * step / len));
                }
                dabs.push_back(at);
                last = at;
            }
        } else {
            dabs.push_back(local(opt<float>(p, "x", 0.0f), opt<float>(p, "z", 0.0f)));
        }
        if (layer < 0 && g.paint.size() != g.h.size()) g.paint.assign(g.h.size(), {0, 0, 0, 0});
        if (layer >= 0 && g.layer_paint.size() != g.h.size()) g.layer_paint.assign(g.h.size(), {0, 0, 0, 0});
        std::vector<char> touched(g.h.size(), 0);
        for (const Vec3& c : dabs) {
            // Only the samples under the brush.
            const int i0 = std::max(0, static_cast<int>(std::floor((c.x - radius + g.size_x * 0.5f) / g.cell_x())));
            const int i1 = std::min(g.n - 1, static_cast<int>(std::ceil((c.x + radius + g.size_x * 0.5f) / g.cell_x())));
            const int j0 = std::max(0, static_cast<int>(std::floor((c.z - radius + g.size_z * 0.5f) / g.cell_z())));
            const int j1 = std::min(g.n - 1, static_cast<int>(std::ceil((c.z + radius + g.size_z * 0.5f) / g.cell_z())));
            for (int j = j0; j <= j1; ++j) {
                for (int i = i0; i <= i1; ++i) {
                    const float x = -g.size_x * 0.5f + static_cast<float>(i) * g.cell_x(), z = -g.size_z * 0.5f + static_cast<float>(j) * g.cell_z();
                    const float d = repro::hypot(x - c.x, z - c.z);
                    if (d >= radius) continue;
                    const float s = amount * (0.5f + 0.5f * repro::cos(d / radius * std::numbers::pi_v<float>));
                    const std::size_t k = static_cast<std::size_t>(j) * static_cast<std::size_t>(g.n) + static_cast<std::size_t>(i);
                    if (layer >= 0) {
                        // The layer's share laid over the others' paint, or taken away.
                        auto& q = g.layer_paint[k];
                        const auto was = q;
                        const auto li = static_cast<std::size_t>(layer);
                        if (mode == "paint") {
                            for (float& v : q) v *= 1 - s;
                            q[li] += s;
                        } else {
                            q[li] *= 1 - s;
                        }
                        if (q != was) touched[k] = 1;
                        continue;
                    }
                    auto& q = g.paint[k];
                    const auto was = q;
                    if (mode == "paint") {
                        // The colour laid over what was painted: coverage s over the old coverage.
                        const float a = s + q[3] * (1 - s);
                        for (int ch = 0; ch < 3; ++ch) q[ch] = a > 1e-6f ? (color[ch] * s + q[ch] * q[3] * (1 - s)) / a : color[ch];
                        q[3] = a;
                    } else {
                        q[3] *= 1 - s;
                    }
                    if (q != was) touched[k] = 1;
                }
            }
        }
        int changed = 0;
        for (char t : touched) changed += t;
        const Vec3 c = dabs.back();
        if (changed > 0) {
            remesh_terrain(id, st, tc);
            const Vec3 at = m.transform_point(Vec3{c.x, 0, c.z});
            Json data{{"mode", mode}, {"x", at.x}, {"z", at.z}, {"dabs", dabs.size()}, {"radius", radius * scale_xz}, {"samples", changed}, {"revision", st.revision}};
            if (layer >= 0) data["layer"] = layer;
            world_->events().emit(clock_.tick, "terrain.painted", id, std::move(data), 0, "terrain");
        }
        if (layer >= 0) return Json{{"entity", id}, {"mode", mode}, {"layer", layer}, {"samples", changed}, {"revision", st.revision}, {"layers", layer_shares(g.grid_at(st.shares, c.x, c.z))}};
        const auto at = g.paint_at(c.x, c.z);
        return Json{{"entity", id}, {"mode", mode}, {"samples", changed}, {"revision", st.revision}, {"paint", Json{{"r", at[0]}, {"g", at[1]}, {"b", at[2]}, {"a", at[3]}}}};
    }
    if (op == "paints") {
        // The paint grid, four numbers a sample in the heights' order: read, or set whole (the
        // editor's undo; an agent's own painter); an empty array clears it.
        // With layers: true, the textured layers' paint (a share for each of four layers a sample).
        const bool layers = opt<bool>(p, "layers", false);
        auto& grid = layers ? st.grid.layer_paint : st.grid.paint;
        if (p.contains("paint")) {
            const Json& ps = p["paint"];
            const std::size_t want = st.grid.h.size() * 4;
            if (!ps.is_array() || (!ps.empty() && ps.size() != want)) return fail("bad_args", "paint is an array of {} numbers ({} by {} samples, {} each), or empty", want, st.grid.n, st.grid.n, layers ? "four layers' shares" : "r g b a");
            std::vector<std::array<float, 4>> next(ps.empty() ? 0 : st.grid.h.size());
            for (std::size_t k = 0; k < ps.size(); ++k) {
                if (!ps[k].is_number()) return fail("bad_args", "paint[{}] is not a number", k);
                next[k / 4][k % 4] = std::clamp(ps[k].get<float>(), 0.0f, 1.0f);
            }
            grid = std::move(next);
            remesh_terrain(id, st, tc);
            world_->events().emit(clock_.tick, "terrain.painted", id, Json{{"mode", "set"}, {"layers", layers}, {"samples", grid.size()}, {"revision", st.revision}}, 0, "terrain");
            return Json{{"entity", id}, {"resolution", st.grid.n}, {"revision", st.revision}};
        }
        Json arr = Json::array();
        for (const auto& q : grid) for (float v : q) arr.push_back(std::round(static_cast<double>(v) * 10000.0) / 10000.0);
        return Json{{"entity", id}, {"resolution", st.grid.n}, {"paint", std::move(arr)}};
    }
    if (op == "sculpt") {
        // A brush at (x, z): raise, lower, flatten toward a height, or smooth, fading from full at the
        // centre to nothing at `radius` (a cosine), `amount` units at the centre (a fraction for smooth).
        if (!p.contains("x") || !p.contains("z")) return fail("bad_args", "terrain.sculpt needs x and z");
        const std::string mode = opt<std::string>(p, "mode", "raise");
        if (mode != "raise" && mode != "lower" && mode != "flatten" && mode != "smooth") return fail("bad_args", "mode is raise, lower, flatten or smooth");
        const float radius = std::max(opt<float>(p, "radius", 3.0f), 1e-3f) / scale_xz;
        const float amount = opt<float>(p, "amount", mode == "smooth" ? 0.5f : 0.5f);
        const Vec3 c = local(opt<float>(p, "x", 0.0f), opt<float>(p, "z", 0.0f));
        const float target = p.contains("target") ? (opt<float>(p, "target", 0.0f) - (wt ? wt->position.y : 0.0f)) / (std::fabs(scale_y) > 1e-6f ? scale_y : 1.0f) : st.grid.sample(c.x, c.z);
        assets::Terrain& g = st.grid;
        const std::vector<float> before = g.h;
        int changed = 0;
        for (int j = 0; j < g.n; ++j) {
            for (int i = 0; i < g.n; ++i) {
                const float x = -g.size_x * 0.5f + static_cast<float>(i) * g.cell_x(), z = -g.size_z * 0.5f + static_cast<float>(j) * g.cell_z();
                const float d = repro::hypot(x - c.x, z - c.z);
                if (d >= radius) continue;
                const float w = 0.5f + 0.5f * repro::cos(d / radius * std::numbers::pi_v<float>);
                float& h = g.h[static_cast<std::size_t>(j) * static_cast<std::size_t>(g.n) + static_cast<std::size_t>(i)];
                const float was = h;
                if (mode == "raise") h += amount / scale_y * w;
                else if (mode == "lower") h -= amount / scale_y * w;
                else if (mode == "flatten") h += (target - h) * std::clamp(w * std::max(amount, 0.0f), 0.0f, 1.0f);
                else {
                    float sum = 0;
                    int count = 0;
                    for (int dj = -1; dj <= 1; ++dj) for (int di = -1; di <= 1; ++di) { sum += before[static_cast<std::size_t>(std::clamp(j + dj, 0, g.n - 1)) * static_cast<std::size_t>(g.n) + static_cast<std::size_t>(std::clamp(i + di, 0, g.n - 1))]; ++count; }
                    h += (sum / static_cast<float>(count) - h) * std::clamp(w * amount, 0.0f, 1.0f);
                }
                h = std::clamp(h, 0.0f, g.height);
                if (h != was) ++changed;
            }
        }
        if (changed > 0) {
            st.edited = true;
            remesh_terrain(id, st, tc);
            world_->events().emit(clock_.tick, "terrain.sculpted", id, Json{{"mode", mode}, {"x", opt<float>(p, "x", 0.0f)}, {"z", opt<float>(p, "z", 0.0f)}, {"radius", radius * scale_xz}, {"samples", changed}, {"revision", st.revision}}, 0, "terrain");
        }
        return Json{{"entity", id}, {"mode", mode}, {"samples", changed}, {"revision", st.revision}, {"height", m.transform_point(Vec3{c.x, g.sample(c.x, c.z), c.z}).y}};
    }
    if (op == "save") {
        // The heights as a 16-bit PNG in the project; the Terrain then reads its heightmap from it.
        const std::string rel = opt<std::string>(p, "path", "");
        if (rel.empty()) return fail("bad_args", "terrain.save needs a project-relative path such as assets/island.png");
        const bool paint = opt<bool>(p, "paint", false), layers = opt<bool>(p, "layers", false);
        if (paint && layers) return fail("bad_args", "save the paint or the layers, one at a time");
        POCKET_TRY(full, inside_dir(options_.project_dir, rel));
        std::filesystem::create_directories(full.parent_path());
        POCKET_TRY_VOID(fs::write_text(full, paint ? assets::terrain_paint_png(st.grid) : layers ? assets::terrain_layers_png(st.grid) : assets::terrain_png16(st.grid)));
        world::Terrain next = tc;
        (paint ? next.paintmap : layers ? next.layermap : next.heightmap) = rel;
        world_->ecs().entity(id).set<world::Terrain>(next);
        assets_->invalidate(rel);
        update_terrains();
        return Json{{"entity", id}, {"path", rel}, {"resolution", st.grid.n}, {"revision", st.revision}};
    }
    if (op == "heights") {
        // The grid itself, row after row along z: read, or set whole (an editor's undo, an agent's
        // own generator). Heights are in the terrain's units, 0..height.
        if (p.contains("heights")) {
            const Json& hs = p["heights"];
            const std::size_t want = static_cast<std::size_t>(st.grid.n) * static_cast<std::size_t>(st.grid.n);
            if (!hs.is_array() || hs.size() != want) return fail("bad_args", "heights is an array of {} numbers ({} by {})", want, st.grid.n, st.grid.n);
            for (std::size_t k = 0; k < want; ++k) {
                if (!hs[k].is_number()) return fail("bad_args", "heights[{}] is not a number", k);
                st.grid.h[k] = std::clamp(hs[k].get<float>(), 0.0f, st.grid.height);
            }
            st.edited = true;
            remesh_terrain(id, st, tc);
            world_->events().emit(clock_.tick, "terrain.sculpted", id, Json{{"mode", "set"}, {"samples", want}, {"revision", st.revision}}, 0, "terrain");
            return Json{{"entity", id}, {"resolution", st.grid.n}, {"revision", st.revision}};
        }
        Json arr = Json::array();
        for (float v : st.grid.h) arr.push_back(std::round(static_cast<double>(v) * 10000.0) / 10000.0);
        return Json{{"entity", id}, {"resolution", st.grid.n}, {"height", st.grid.height}, {"heights", std::move(arr)}};
    }
    if (op == "reset") {
        // Back to what the heightmap or the noise makes, the edits dropped; or the paint back to its
        // paintmap's (none without one).
        if (opt<bool>(p, "paint", false)) {
            st.paint_key.clear();
        } else if (opt<bool>(p, "layers", false)) {
            st.layer_key.clear();
        } else {
            st.shape_key.clear();
        }
        update_terrains();
        return Json{{"entity", id}, {"revision", terrains_[id].revision}};
    }
    return fail("unknown_command", "unknown terrain command '{}'", op);
}

// Sprite materials the sprites name, read and compiled the first time one is named (and again after
// assets.reload); a material that does not compile keeps its message for world.lint and is drawn plain.
void Session::sync_sprite_materials() {
    if (!renderer_) return;
    world_->ecs().each([&](const world::MeshRenderer& mr) {
        if (mr.material.empty() || renderer_->has_mesh_material(mr.material) || sprite_material_errors_.contains(mr.material)) return;
        auto full = inside_dir(options_.project_dir, mr.material);
        auto text = full ? fs::read_text(*full) : Result<std::string>(fail("bad_path", "{}", full.error().message));
        const std::string error = text ? renderer_->set_mesh_material(mr.material, *text) : text.error().message;
        if (!error.empty()) {
            sprite_material_errors_[mr.material] = error;
            log::warn("runtime", "mesh material {}: {}", mr.material, error);
        }
    });
    world_->ecs().each([&](const world::Sprite& sp) {
        if (sp.material.empty() || renderer_->has_sprite_material(sp.material) || sprite_material_errors_.contains(sp.material)) return;
        auto full = inside_dir(options_.project_dir, sp.material);
        if (!full) {
            sprite_material_errors_[sp.material] = full.error().message;
            return;
        }
        auto text = fs::read_text(*full);
        if (!text) {
            sprite_material_errors_[sp.material] = text.error().message;
            return;
        }
        const std::string error = renderer_->set_sprite_material(sp.material, *text);
        if (!error.empty()) {
            sprite_material_errors_[sp.material] = error;
            log::warn("runtime", "sprite material {}: {}", sp.material, error);
        }
    });
}

Status Session::render_frame() {
    sync_sprite_materials();
    if (audio_) audio_->pump();
    update_terrains();   // a terrain spawned or changed since the last tick shows in this frame
    auto frame = device_->begin_frame();
    if (!frame) return fail(frame.error());
    build_debug_draw();
    // Several cameras (docs/design/cameras.md, Several cameras): active cameras with a viewport of
    // their own each draw their part of the window, lowest order first; otherwise the one camera.
    struct View { int order; world::EntityId id; Vec4 viewport; };
    std::vector<View> views;
    // Cameras that draw into a texture (Camera.target) go first, so what shows their pictures in
    // the window's views sees this frame's (docs/design/cameras.md, Into a texture).
    struct Target { int order; world::EntityId id; std::string name; Vec2 size; };
    std::vector<Target> targets;
    world_->ecs().each([&](flecs::entity e, const world::Camera& c) {
        if (!c.active) return;
        if (!c.target.empty()) targets.push_back({c.order, e.id(), c.target, c.target_size});
        else views.push_back({c.order, e.id(), c.viewport});
    });
    std::sort(targets.begin(), targets.end(), [](const Target& a, const Target& b) { return a.order != b.order ? a.order < b.order : a.id < b.id; });
    for (const Target& t : targets) {
        const auto w = static_cast<std::uint32_t>(std::clamp(std::round(t.size.x), 1.0f, static_cast<float>(frame->width)));
        const auto h = static_cast<std::uint32_t>(std::clamp(std::round(t.size.y), 1.0f, static_cast<float>(frame->height)));
        auto tex = renderer_->view_texture(t.name, w, h);
        if (!tex) { record_error(tex.error()); continue; }
        rhi::Frame into = *frame;
        into.color = tex->view;
        into.color_texture = tex->texture;
        renderer::Renderer::RenderView rv;
        rv.camera = t.id;
        rv.secondary = true;
        rv.target = t.name;
        rv.viewport = renderer::Viewport{0, 0, w, h};
        if (auto r = renderer_->render(into, *world_, clear_, particles_.get(), animation_.get(), nullptr, &rv); !r) {
            (void)device_->end_frame(*frame);
            return fail(r.error());
        }
        device_->submit_so_far(*frame);
    }
    const bool split = views.size() > 1 && std::any_of(views.begin(), views.end(), [](const View& v) { return v.viewport.x != 0 || v.viewport.y != 0 || v.viewport.z != 1 || v.viewport.w != 1; });
    if (split) {
        std::sort(views.begin(), views.end(), [](const View& a, const View& b) { return a.order != b.order ? a.order < b.order : a.id < b.id; });
        const auto fw = static_cast<float>(frame->width), fh = static_cast<float>(frame->height);
        for (std::size_t i = 0; i < views.size(); ++i) {
            const Vec4 v = views[i].viewport;
            renderer::Renderer::RenderView rv;
            rv.camera = views[i].id;
            rv.secondary = i > 0;
            const int x0 = static_cast<int>(std::round(std::clamp(v.x, 0.0f, 1.0f) * fw)), y0 = static_cast<int>(std::round(std::clamp(v.y, 0.0f, 1.0f) * fh));
            const int x1 = static_cast<int>(std::round(std::clamp(v.x + v.z, 0.0f, 1.0f) * fw)), y1 = static_cast<int>(std::round(std::clamp(v.y + v.w, 0.0f, 1.0f) * fh));
            if (x1 <= x0 || y1 <= y0) continue;
            rv.viewport = renderer::Viewport{x0, y0, static_cast<std::uint32_t>(x1 - x0), static_cast<std::uint32_t>(y1 - y0)};
            // Each view's uniforms are written before its passes run: what the last one recorded goes first.
            if (i > 0) device_->submit_so_far(*frame);
            if (auto r = renderer_->render(*frame, *world_, clear_, particles_.get(), animation_.get(), debug_draw_.lines() ? &debug_draw_ : nullptr, &rv); !r) {
                (void)device_->end_frame(*frame);
                return fail(r.error());
            }
        }
    } else if (auto r = renderer_->render(*frame, *world_, clear_, particles_.get(), animation_.get(), debug_draw_.lines() ? &debug_draw_ : nullptr); !r) {
        // Still submit the encoder so the device stays consistent, then report.
        (void)device_->end_frame(*frame);
        return fail(r.error());
    }
    for (const auto& [path, box] : renderer_->take_new_bounds()) world_->set_mesh_bounds(path, box.first, box.second);
    if (ui_ && painter_ && ui_->node_count() > 1) {
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        ui_->layout(w, h, scale);
        painter_->begin(w, h, scale);
        ui_->paint(*painter_);
        if (auto r = painter_->flush(*frame); !r) {
            (void)device_->end_frame(*frame);
            return fail(r.error());
        }
    }
    POCKET_TRY_VOID(device_->end_frame(*frame));
    return {};
}

void Session::ui_size(float& width, float& height, float& scale) const {
    scale = platform_ ? platform_->pixel_density() : 1.0f;
    if (!(scale > 0)) scale = 1.0f;
    width = static_cast<float>(device_->width()) / scale;
    height = static_cast<float>(device_->height()) / scale;
}

Json Session::frame_info() const {
    Json j;
    j["frame"] = frames_;
    j["tick"] = clock_.tick - run_start_;
    j["time"] = static_cast<double>(clock_.tick - run_start_) * clock_.tick_seconds;
    j["dt"] = clock_.tick_seconds;
    j["paused"] = paused_;
    j["time_scale"] = time_scale_;
    return j;
}

Result<bool> Session::poll_input(Json& input_events, int& ticks, bool simulating) {
    input_events = Json::array();
    ticks = simulating ? 1 : 0;
    std::vector<platform::Event> events;
    if (journal_ && journal_->replaying) {
        Json recorded = Json::array();
        if (!journal_->next_frame(ticks, recorded)) return false;
        (void)platform_->poll();
        for (const Json& j : recorded) events.push_back(platform::event_from_json(j));
        if (!simulating) ticks = 0;
    } else {
        events = platform_->poll();
        for (auto& e : events) {
            if (e.type == platform::EventType::Resize) {
                if (auto r = device_->resize(static_cast<std::uint32_t>(e.width), static_cast<std::uint32_t>(e.height)); !r) record_error(r.error());
            }
        }
        if (simulating) {
            // Real time into ticks through the time scale: a windowed run by the wall clock, a
            // headless one a tick's worth per frame; a step is one tick whatever the scale.
            double elapsed = clock_.tick_seconds;
            if (!options_.headless) {
                elapsed = frame_timer_.lap();
                if (frames_ == 0) elapsed = clock_.tick_seconds;
            }
            if (stepping_ || (options_.headless && time_scale_ == 1.0)) {
                ticks = 1;
            } else {
                ticks = clock_.advance(elapsed * time_scale_);
            }
            if (scale_left_ > 0) {
                scale_left_ -= elapsed;
                if (scale_left_ <= 1e-9) {
                    scale_left_ = 0;
                    time_scale_ = 1.0;
                }
            }
        }
        if (journal_ && journal_->recording) {
            Json raw = Json::array();
            for (auto& e : events) raw.push_back(platform::event_to_json(e));
            journal_->record_frame(ticks, raw);
        }
    }
    release_expired_holds();
    advance_rumble();
    if (net_) {
        // Lockstep: this peer's input acts on the tick it is committed for, for everyone at once.
        for (auto& e : events) {
            if (e.type == platform::EventType::Resize || e.type == platform::EventType::Quit) continue;
            Json j = platform::event_to_json(e);
            if (const ui::NodeId on = ui_press_target(e)) j["ui"] = on;   // every peer leaves the action alone
            net_queue_.push_back(std::move(j));
        }
    } else {
        for (auto& e : events) if (!ui_press_target(e)) input_map_.apply(e);
        for (auto& e : events) input_events.push_back(platform::event_to_json(e));
    }
    // While the pointer is captured, its presses and motion are the game's, not the interface's.
    const bool held = cursor_held();
    auto pointer = [](const platform::Event& e) { return e.type == platform::EventType::MouseMove || e.type == platform::EventType::MouseDown || e.type == platform::EventType::MouseUp || e.type == platform::EventType::MouseWheel; };
    if (ui_) {
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        ui_->layout(w, h, scale);
        bool wants_text = false;
        std::vector<platform::Event> unheld;
        if (held) for (const auto& e : events) if (!pointer(e)) unheld.push_back(e);
        std::vector<Json> ui_events = ui_->handle_events(held ? unheld : events, wants_text);
        platform_->set_text_input(wants_text);
        if (wants_text) { const ui::Rect c = ui_->caret_rect(); platform_->set_text_input_area(static_cast<int>(c.x), static_cast<int>(c.y), static_cast<int>(c.w), static_cast<int>(c.h)); }
        // Tag input events that landed on the interface so gameplay code can ignore them.
        for (std::size_t i = 0; i < events.size(); ++i) {
            const platform::Event& e = events[i];
            if (held && pointer(e)) continue;
            ui::NodeId target = 0;
            switch (e.type) {
                case platform::EventType::MouseMove:
                case platform::EventType::MouseDown:
                case platform::EventType::MouseUp: target = ui_->hit_test(e.x, e.y); break;
                case platform::EventType::TouchDown:
                case platform::EventType::TouchUp:
                case platform::EventType::TouchMove: target = ui_->hit_test(e.x, e.y); break;
                case platform::EventType::MouseWheel: target = ui_->hit_test(platform_->input().mouse_x, platform_->input().mouse_y); break;
                case platform::EventType::KeyDown:
                case platform::EventType::KeyUp:
                case platform::EventType::Text:
                case platform::EventType::PadButton: target = ui_->focused(); break;   // a pad drives a focused interface
                default: break;
            }
            if (target) input_events[i]["ui"] = target;
        }
        if (!ui_events.empty()) dispatch("ui", Json(ui_events));
    }
    if (!net_) recognize_gestures(events, input_events, simulating);
    return true;
}

void Session::recognize_gestures(const std::vector<platform::Event>& events, Json& input_events, bool time_passes) {
    std::vector<std::uint64_t> on_ui(events.size(), 0);
    for (std::size_t i = 0; i < events.size() && i < input_events.size(); ++i) {
        if (input_events[i].is_object() && input_events[i].contains("ui") && input_events[i]["ui"].is_number()) on_ui[i] = input_events[i]["ui"].get<std::uint64_t>();
    }
    {
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        gestures_.set_view(w, h);
    }
    std::vector<Json> found = gestures_.feed(events, on_ui, clock_.tick, clock_.tick_seconds);
    if (time_passes) for (Json& g : gestures_.tick(clock_.tick, clock_.tick_seconds)) found.push_back(std::move(g));
    for (Json& g : found) {
        Json data = g;
        data.erase("type");
        world_->events().emit(clock_.tick, "input.gesture", 0, std::move(data), 0, "input");
        input_events.push_back(std::move(g));
    }
}

Json Session::inject_events(std::vector<platform::Event> events) {
    Json out;
    Json input_events = Json::array();
    if (net_) {
        for (auto& e : events) net_queue_.push_back(platform::event_to_json(e));   // for the tick it is committed for
    } else {
        for (auto& e : events) { if (!ui_press_target(e)) input_map_.apply(e); input_events.push_back(platform::event_to_json(e)); }
    }
    // Synthetic input is part of the run: record it so a replay reproduces it.
    if (journal_ && journal_->recording && !journal_->frames.empty()) {
        Json& last = journal_->frames.back();
        if (last.is_object() && last.contains("events") && last["events"].is_array()) for (auto& e : input_events) last["events"].push_back(e);
    }
    Json ui_events = Json::array();
    if (ui_) {
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        ui_->layout(w, h, scale);
        bool wants_text = false;
        for (Json& e : ui_->handle_events(events, wants_text)) ui_events.push_back(std::move(e));
        platform_->set_text_input(wants_text);
        if (wants_text) { const ui::Rect c = ui_->caret_rect(); platform_->set_text_input_area(static_cast<int>(c.x), static_cast<int>(c.y), static_cast<int>(c.w), static_cast<int>(c.h)); }
        for (std::size_t i = 0; i < events.size(); ++i) {
            const platform::Event& e = events[i];
            ui::NodeId target = 0;
            if (e.type == platform::EventType::MouseMove || e.type == platform::EventType::MouseDown || e.type == platform::EventType::MouseUp || e.type == platform::EventType::TouchDown || e.type == platform::EventType::TouchUp || e.type == platform::EventType::TouchMove) target = ui_->hit_test(e.x, e.y);
            else if (e.type == platform::EventType::KeyDown || e.type == platform::EventType::KeyUp || e.type == platform::EventType::Text) target = ui_->focused();
            if (target) input_events[i]["ui"] = target;
        }
        if (!ui_events.empty()) dispatch("ui", ui_events);
    }
    if (!net_) recognize_gestures(events, input_events, false);
    if (!input_events.empty()) dispatch("input", input_events);
    host_->drain_microtasks();
    out["events"] = ui_events;
    out["input"] = input_events;
    if (ui_) out["focused"] = ui_->focused();
    return out;
}

Status Session::idle_frame() {
    if (!started_) return fail("not_started", "Session::start() was not called");
    Json input_events;
    int ticks = 0;
    POCKET_TRY(has_frame, poll_input(input_events, ticks, false));
    (void)has_frame;
    if (platform_->quit_requested()) return {};
    if (!input_events.empty()) dispatch("input", input_events);
    if (net_) net_pump();   // a paused peer still greets players and keeps its connections moving
    // No tick ran: edits made since the last one (inspector, gizmo, agents) still need world transforms.
    world_->update_transforms();
    dispatch("frame", frame_info());
    host_->drain_microtasks();
    world_->update_transforms();
    if (errors_.empty()) {
        Stopwatch render_sw;
        auto r = render_frame();
        perf_render_.add(render_sw.ms());
        if (!r) {
            record_error(r.error());
            return fail(r.error());
        }
    }
    frames_++;
    // Nothing else throttles a paused window: pace it at the tick rate. (The browser paces its
    // own frame callback.)
#ifndef __EMSCRIPTEN__
    double spent = pace_timer_.seconds();
    double budget = clock_.tick_seconds;
    if (spent < budget) std::this_thread::sleep_for(std::chrono::duration<double>(budget - spent));
#endif
    pace_timer_.lap();
    frame_timer_.lap();  // resuming must not simulate the time spent paused
    return {};
}

namespace {

// Visualize an id buffer: each id gets a stable pseudo-random color, background stays black.
rhi::Image ids_to_image(const renderer::IdImage& ids) {
    rhi::Image img;
    img.width = ids.width;
    img.height = ids.height;
    img.rgba.resize(static_cast<std::size_t>(ids.width) * ids.height * 4);
    for (std::size_t i = 0; i < ids.ids.size(); ++i) {
        std::uint32_t id = ids.ids[i];
        std::uint8_t* px = img.rgba.data() + i * 4;
        if (id == 0) {
            px[0] = px[1] = px[2] = 0;
        } else {
            std::uint32_t h = id * 2654435761u;
            px[0] = static_cast<std::uint8_t>(64 + (h & 0x7F));
            px[1] = static_cast<std::uint8_t>(64 + ((h >> 8) & 0x7F));
            px[2] = static_cast<std::uint8_t>(64 + ((h >> 16) & 0x7F));
        }
        px[3] = 255;
    }
    return img;
}

}  // namespace

namespace {
Vec3 vec3_of(const Json& j, Vec3 fallback) {
    if (j.is_array() && j.size() >= 3) return {j[0].get<float>(), j[1].get<float>(), j[2].get<float>()};
    if (j.is_object()) return {j.value("x", fallback.x), j.value("y", fallback.y), j.value("z", fallback.z)};
    return fallback;
}
Json json_of(Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; }

// A project-relative path resolved under the project directory; escapes are refused.
Result<std::filesystem::path> inside_dir(const std::filesystem::path& project_dir, const std::string& rel) {
    if (rel.empty()) return fail("bad_args", "path is required");
    std::filesystem::path base = std::filesystem::weakly_canonical(project_dir);
    std::filesystem::path full = std::filesystem::weakly_canonical(base / rel);
    auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
    if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", rel);
    return full;
}

// Where a picture a command writes goes: a relative path under the project (not out of it), an
// absolute one as given; its directory is made.
Result<std::filesystem::path> output_path(const std::filesystem::path& project_dir, const std::string& path) {
    std::filesystem::path full(path);
    if (full.is_relative()) {
        POCKET_TRY(inside, inside_dir(project_dir, path));
        full = inside;
    }
    std::error_code ec;
    if (full.has_parent_path()) std::filesystem::create_directories(full.parent_path(), ec);
    return full;
}
}  // namespace

Result<std::vector<std::string>> Session::load_sprite_sheet(const std::string& rel, const std::string& prefix) {
    if (!inside_dir(options_.project_dir, rel)) return fail("forbidden", "{} is outside the project directory", rel);
    const std::filesystem::path file = options_.project_dir / rel;
    POCKET_TRY(text, fs::read_text(file));
    const Json doc = Json::parse(text, nullptr, false);
    if (doc.is_discarded() || !doc.is_object() || !doc.contains("frames") || !doc.contains("meta")) return fail("bad_sheet", "{} is not a sprite sheet's JSON (frames and meta)", rel);
    const Json& meta = doc["meta"];
    const float sw = meta.contains("size") ? meta["size"].value("w", 0.0f) : 0.0f, sh = meta.contains("size") ? meta["size"].value("h", 0.0f) : 0.0f;
    if (sw <= 0 || sh <= 0) return fail("bad_sheet", "{}: meta.size gives no size", rel);
    // The frames in their order (an object's keys in the file's order, or an array).
    std::vector<Vec4> rects;
    std::vector<float> seconds;
    auto take = [&](const Json& f) {
        const Json& r = f.value("frame", Json::object());
        const float x = r.value("x", 0.0f), y = r.value("y", 0.0f), fw = r.value("w", 0.0f), fh = r.value("h", 0.0f);
        rects.push_back(Vec4{x / sw, y / sh, (x + fw) / sw, (y + fh) / sh});
        seconds.push_back(std::max(f.value("duration", 100.0f), 1.0f) / 1000.0f);
    };
    if (doc["frames"].is_array()) for (const Json& f : doc["frames"]) take(f);
    else if (doc["frames"].is_object()) for (const auto& [k, f] : doc["frames"].items()) take(f);
    if (rects.empty()) return fail("bad_sheet", "{} has no frames", rel);
    // The image beside the JSON, named by the sheet.
    const std::string image = meta.value("image", "");
    const std::string texture = image.empty() ? std::string() : (std::filesystem::path(rel).parent_path() / image).lexically_normal().generic_string();
    std::vector<std::string> names;
    auto define = [&](const std::string& name, std::vector<int> order, bool loop) {
        world::World::SpriteClip c;
        c.texture = texture;
        c.rects = rects;
        c.frames = std::move(order);
        for (int f : c.frames) c.durations.push_back(seconds[static_cast<std::size_t>(f)]);
        c.loop = loop;
        world_->define_clip(name, std::move(c));
        names.push_back(name);
    };
    const Json tags = meta.value("frameTags", Json::array());
    const int last = static_cast<int>(rects.size()) - 1;
    if (!tags.is_array() || tags.empty()) {
        std::vector<int> all;
        for (int i = 0; i <= last; ++i) all.push_back(i);
        define(prefix, all, true);
        return names;
    }
    for (const Json& t : tags) {
        const int from = std::clamp(t.value("from", 0), 0, last), to = std::clamp(t.value("to", last), from, last);
        const std::string dir = t.value("direction", "forward");
        std::vector<int> run;
        for (int i = from; i <= to; ++i) run.push_back(i);
        if (dir == "reverse") std::reverse(run.begin(), run.end());
        else if (dir == "pingpong" || dir == "pingpong_reverse") {
            for (int i = to - 1; i > from; --i) run.push_back(i);   // back down without repeating the ends
            if (dir == "pingpong_reverse") std::rotate(run.begin(), run.begin() + (to - from), run.end());
        }
        // "repeat" (Aseprite 1.3): a number of plays; absent or 0 loops for ever.
        int repeat = 0;
        if (t.contains("repeat")) repeat = t["repeat"].is_string() ? std::atoi(t["repeat"].get<std::string>().c_str()) : t["repeat"].get<int>();
        std::vector<int> order;
        for (int k = 0; k < std::max(repeat, 1); ++k) order.insert(order.end(), run.begin(), run.end());
        define(prefix.empty() ? t.value("name", "clip") : prefix + "." + t.value("name", "clip"), order, repeat <= 0);
    }
    return names;
}

Result<Json> Session::sprite_command(std::string_view op, const Json& p) {
    // Sprite clips and playback (docs/design/sprites.md): the same calls for scripts and agents.
    auto& w = *world_;
    if (op == "clip") {
        std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "clip needs a name");
        POCKET_TRY(clip, world::World::SpriteClip::from_json(p));
        Json j = clip.to_json();
        w.define_clip(name, std::move(clip));
        j["name"] = name;
        return j;
    }
    if (op == "clips") {
        Json out = Json::object();
        for (const auto& [name, clip] : w.clips()) out[name] = clip.to_json();
        return out;
    }
    if (op == "sheet") {
        // A sheet exported by Aseprite (JSON, hash or array frames): a clip per tag.
        const std::string path = opt<std::string>(p, "path", "");
        if (path.empty()) return fail("bad_args", "sheet needs the path of the JSON Aseprite exported (File > Export Sprite Sheet, with JSON data)");
        POCKET_TRY(names, load_sprite_sheet(path, opt<std::string>(p, "prefix", std::filesystem::path(path).stem().stem().string())));
        Json out = Json::object();
        for (const std::string& n : names) out[n] = w.clip(n)->to_json();
        return Json{{"clips", out}};
    }
    if (op == "play" || op == "stop") {
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        std::uint64_t cause = opt<std::uint64_t>(p, "cause", 0);
        Json patch = Json::object();
        if (op == "play") {
            std::string clip = opt<std::string>(p, "clip", "");
            if (!clip.empty()) {
                const world::World::SpriteClip* c = w.clip(clip);
                if (!c) return fail("no_such_clip", "no sprite clip named '{}' (define it with sprite.clip or [sprite_clips])", clip);
                patch["clip"] = clip;
                patch["loop"] = c->loop;   // the clip's setting unless the call says otherwise (below)
            } else if (!w.has(id, "SpriteAnimation")) {
                return fail("bad_args", "play needs a clip name");
            }
            if (!w.has(id, "Sprite")) {
                // A sprite the clip can draw on; the clip's texture fills it on the first tick.
                POCKET_TRY_VOID(w.set(id, "Sprite", Json::object(), cause));
            }
            const bool restart = opt<bool>(p, "restart", true);
            patch["playing"] = true;
            patch["finished"] = false;
            if (restart) {
                patch["frame"] = 0;
                patch["time"] = 0.0;
            }
            for (const char* k : {"loop", "speed", "fps"}) {
                if (p.contains(k)) patch[k] = p[k];
            }
        } else {
            patch["playing"] = false;
            if (opt<bool>(p, "reset", false)) {
                patch["frame"] = 0;
                patch["time"] = 0.0;
            }
        }
        POCKET_TRY_VOID(w.set(id, "SpriteAnimation", patch, cause));
        return w.get(id, "SpriteAnimation");
    }
    return fail("unknown_command", "unknown sprite command '{}'", op);
}

Result<Json> Session::nav_command(std::string_view op, const Json& p) {
    // Navigation (docs/design/navigation.md): a walkability grid baked from the static colliders
    // or a tile map, and paths over it. Every call is a command, so it replays and is journaled.
    auto& w = *world_;
    auto point_of = [&](const Json& v, const char* what) -> Result<Vec3> {
        if (v.is_object() && (v.contains("x") || v.contains("y") || v.contains("z"))) return vec3_of(v, {0, 0, 0});
        if (v.is_array() && v.size() == 3) return Vec3{v[0].get<float>(), v[1].get<float>(), v[2].get<float>()};
        if (v.is_string() || v.is_number()) {
            world::EntityId id = resolve_entity(v);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {} ({})", v.dump(), what);
            if (const auto* wt = w.try_get<world::WorldTransform>(id)) return wt->position;
            if (const auto* t = w.try_get<world::Transform>(id)) return t->position;
            return fail("no_transform", "entity {} has no transform ({})", w.path(id), what);
        }
        return fail("bad_args", "{} must be a point {{x, y, z}} or an entity", what);
    };
    if (op == "bake") {
        if (p.contains("entity")) {
            world::EntityId id = resolve_entity(p["entity"]);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
            if (!assets_) return fail("no_assets", "no asset store");
            nav::TileBakeParams tp;
            tp.mode = opt<std::string>(p, "mode", "topdown");
            tp.agent_height = opt<int>(p, "agent_height", 1);
            tp.jump_height = opt<int>(p, "jump_height", 2);
            tp.jump_range = opt<int>(p, "jump_range", 3);
            tp.max_drop = opt<int>(p, "max_drop", 6);
            tp.diagonal = opt<bool>(p, "diagonal", true);
            POCKET_TRY_VOID(nav_.bake_tilemap(w, *assets_, id, tp, static_cast<std::uint64_t>(clock_.tick)));
        } else {
            nav::BakeParams bp;
            bp.min = vec3_of(p.value("min", Json(nullptr)), bp.min);
            bp.max = vec3_of(p.value("max", Json(nullptr)), bp.max);
            bp.cell = opt<float>(p, "cell", bp.cell);
            bp.agent_radius = opt<float>(p, "agent_radius", bp.agent_radius);
            bp.agent_height = opt<float>(p, "agent_height", bp.agent_height);
            bp.max_step = opt<float>(p, "max_step", bp.max_step);
            bp.max_slope_degrees = opt<float>(p, "max_slope", bp.max_slope_degrees);
            bp.diagonal = opt<bool>(p, "diagonal", true);
            bp.layers = opt<int>(p, "layers", bp.layers);
            POCKET_TRY_VOID(nav_.bake_colliders(w, *physics_, bp, static_cast<std::uint64_t>(clock_.tick)));
        }
        nav_paths_.clear();
        Json d = nav_.describe();
        w.events().emit(clock_.tick, "nav.baked", 0, Json{{"source", d["source"]}, {"walkable", d["walkable"]}, {"cells", d["cells"]}}, 0, "nav");
        return d;
    }
    if (op == "info") return nav_.describe();
    if (op == "mesh") {
        if (!nav_.baked()) return fail("not_baked", "no navigation grid yet: bake one with nav.bake");
        return nav_.mesh_json();
    }
    if (op == "agents") return nav_.agents(w);
    if (op == "clear") {
        nav_.clear();
        nav_paths_.clear();
        return Json{{"baked", false}};
    }
    if (op == "path") {
        if (!p.contains("from") || !p.contains("to")) return fail("bad_args", "'from' and 'to' are required (points or entities)");
        POCKET_TRY(from, point_of(p["from"], "from"));
        POCKET_TRY(to, point_of(p["to"], "to"));
        POCKET_TRY(path, nav_.path(from, to, opt<bool>(p, "smooth", true), opt<bool>(p, "mesh", true)));
        Json pts = Json::array();
        for (const Vec3& v : path.points) pts.push_back(json_of(v));
        if (nav_paths_.size() >= 16) nav_paths_.erase(nav_paths_.begin());
        nav_paths_.push_back(path.points);
        return Json{{"points", pts}, {"length", path.length}, {"partial", path.partial}, {"snapped", path.snapped}, {"expanded", path.expanded}, {"cells", path.cells}, {"mesh", path.mesh}, {"polys", path.polys}};
    }
    if (op == "reachable") {
        if (!p.contains("from") || !p.contains("to")) return fail("bad_args", "'from' and 'to' are required (points or entities)");
        POCKET_TRY(from, point_of(p["from"], "from"));
        POCKET_TRY(to, point_of(p["to"], "to"));
        return Json{{"reachable", nav_.reachable(from, to)}};
    }
    if (op == "nearest") {
        POCKET_TRY(at, point_of(p.contains("point") ? p["point"] : p.value("entity", Json(nullptr)), "point"));
        auto near = nav_.nearest(at, opt<float>(p, "radius", 2.0f));
        if (!near) return nullptr;
        return json_of(*near);
    }
    return fail("unknown_command", "unknown nav command '{}'", op);
}

Result<Json> Session::env_observation(const Json& p, bool first) {
    // What a player sees between two acts: the exposed state, the score and its change, whether
    // the episode is over, the events since the last observation, and the input as it stands.
    Json o;
    const std::int64_t t = clock_.tick - env_start_tick_;
    o["episode"] = env_episode_;
    o["t"] = t;
    o["tick"] = clock_.tick;
    o["time"] = static_cast<double>(t) * clock_.tick_seconds;
    o["state"] = last_state_.is_object() ? last_state_ : Json::object();
    double score = 0;
    bool scored = false;
    if (last_state_.is_object()) {
        if (last_state_.contains("reward") && last_state_["reward"].is_number()) { score = last_state_["reward"].get<double>(); scored = true; }
        else if (last_state_.contains("score") && last_state_["score"].is_number()) { score = last_state_["score"].get<double>(); scored = true; }
    }
    o["score"] = scored ? Json(score) : Json(nullptr);
    o["reward"] = first || !scored ? 0.0 : score - env_last_score_;
    env_last_score_ = score;
    bool done = last_state_.is_object() && last_state_.contains("done") && last_state_["done"].is_boolean() && last_state_["done"].get<bool>();
    if (env_max_ticks_ > 0 && t >= env_max_ticks_) done = true;
    o["done"] = done;
    Json events = Json::array();
    for (const world::Event& e : world_->events().since(env_last_seq_, 200)) {
        events.push_back(Json{{"seq", e.seq}, {"tick", e.tick}, {"type", e.type}, {"subject", e.subject}, {"data", e.data}, {"cause", e.cause}});
        env_last_seq_ = e.seq;
    }
    o["events"] = events;
    o["actions"] = input_map_.snapshot();
    o["world_hash"] = hex64(world_->hash());
    o["errors"] = errors_.size();
    if (p.contains("capture") && p["capture"].is_string()) {
        Json cp = Json{{"path", p["capture"]}};
        if (p.contains("size")) cp["size"] = p["size"];
        if (auto r = command("capture", cp, "env"); !r) return fail(r.error());
        else o["capture"] = *r;
    }
    return o;
}

Result<Json> Session::env_command(std::string_view op, const Json& p) {
    // The environment interface (docs/design/environment.md): reset, act, observe, for bots,
    // learned players and agent evaluations, built on the same commands as everything else.
    if (op == "describe") {
        Json j;
        j["actions"] = input_map_.describe();
        Json keys = Json::array();
        std::string score_key;
        bool has_done = false;
        if (last_state_.is_object()) {
            for (const auto& [k, v] : last_state_.items()) if (!k.starts_with("__")) keys.push_back(k);
            if (last_state_.contains("reward") && last_state_["reward"].is_number()) score_key = "reward";
            else if (last_state_.contains("score") && last_state_["score"].is_number()) score_key = "score";
            has_done = last_state_.contains("done") && last_state_["done"].is_boolean();
        }
        j["observation"] = keys;
        j["score_key"] = score_key.empty() ? Json(nullptr) : Json(score_key);
        j["has_done"] = has_done;
        j["tick_rate"] = options_.tick_rate;
        j["seed"] = options_.seed;
        j["episode"] = env_episode_;
        j["t"] = clock_.tick - env_start_tick_;
        j["max_ticks"] = env_max_ticks_;
        return j;
    }
    if (op == "reset") {
        // A fresh episode: the seed, the scene and the scripts start again; the tick counter, the
        // journal and the event bus carry on, so a session's whole history stays one story.
        if (p.contains("seed") && p["seed"].is_number()) options_.seed = static_cast<std::uint64_t>(p["seed"].get<double>());
        if (p.contains("max_ticks") && p["max_ticks"].is_number()) env_max_ticks_ = std::max(p["max_ticks"].get<int>(), 0);
        for (auto& [key, until] : held_keys_) until = clock_.tick;
        release_expired_holds();
        pending_holds_.clear();
        nav_.clear();
        nav_paths_.clear();
        debug_shapes_.clear();
        recorder_.clear();
        errors_.clear();
        rng_.reseed(options_.seed);
        seed_math_random();
        if (auto r = command("project.reload", Json{{"scene", true}, {"scripts", true}}, "env"); !r) return fail(r.error());
        Json s = dispatch("state", nullptr);
        last_state_ = s.is_object() ? s : Json::object();
        env_episode_++;
        env_start_tick_ = clock_.tick;
        world_->events().emit(clock_.tick, "env.reset", 0, Json{{"episode", env_episode_}, {"seed", options_.seed}, {"max_ticks", env_max_ticks_}}, 0, "env");
        env_last_seq_ = world_->events().last_seq();  // the first observation carries only what the episode does
        return env_observation(p, true);
    }
    if (op == "step") {
        // Act, then run: every action given is held for the step's ticks (a number: its sign is
        // the direction) or pressed once (true), then the ticks run and the observation follows.
        const int ticks = std::clamp(opt<int>(p, "ticks", 1), 1, 100000);
        const Json acts = p.value("actions", Json::object());
        std::vector<std::pair<std::string, Json>> list;
        if (acts.is_object()) {
            for (const auto& [k, v] : acts.items()) list.emplace_back(k, v);
        } else if (acts.is_array()) {
            for (const Json& a : acts) if (a.is_object() && a.contains("action") && a["action"].is_string()) list.emplace_back(a["action"].get<std::string>(), a.value("value", Json(true)));
        } else {
            return fail("bad_args", "actions must be an object {{action: value}} or an array of {{action, value}}");
        }
        for (const auto& [name, v] : list) {
            if (!input_map_.has_action(name)) return fail("no_such_action", "no action named '{}'", name);
            Result<Json> r;
            if (v.is_boolean()) {
                if (!v.get<bool>()) continue;
                r = command("input.press", Json{{"action", name}}, "env");
            } else if (v.is_number()) {
                const double x = v.get<double>();
                if (x == 0) continue;
                r = command("input.hold", Json{{"action", name}, {"ticks", ticks}, {"sign", x < 0 ? -1 : 1}}, "env");
            } else {
                return fail("bad_args", "action '{}' must be true (press) or a number (hold, its sign the direction)", name);
            }
            if (!r) return fail(r.error());
        }
        if (auto r = command("step", Json{{"ticks", ticks}}, "env"); !r) return fail(r.error());
        return env_observation(p, false);
    }
    if (op == "observe") return env_observation(p, false);
    return fail("unknown_command", "unknown env command '{}'", op);
}

Result<Json> Session::tilemap_command(std::string_view op, const Json& p) {
    // Tile maps (docs/design/tilemaps.md): what is where, in tiles and in world units.
    auto& w = *world_;
    // The tilesets of a map made by code: {image, name?, tile_width?, tile_height?, spacing?, margin?,
    // solid?: [local ids]}, numbered on from gid 1.
    auto made_tilesets = [&](const Json& list, const std::string& name, int tw, int th) -> Result<Json> {
        Json tilesets = Json::array();
        std::uint32_t gid = 1;
        const std::filesystem::path map_dir = std::filesystem::path(name).parent_path();
        for (const Json& t : list) {
            const std::string image = t.value("image", std::string());
            if (image.empty()) return fail("bad_args", "a tileset needs an image (project-relative)");
            auto img = assets_->image(image);
            if (!img) return fail("bad_image", "{}: {}", image, img.error().message);
            const int stw = t.value("tile_width", tw), sth = t.value("tile_height", th), spacing = t.value("spacing", 0), margin = t.value("margin", 0);
            const int iw = static_cast<int>((*img)->width), ih = static_cast<int>((*img)->height);
            const int columns = (iw - 2 * margin + spacing) / (stw + spacing), rows = (ih - 2 * margin + spacing) / (sth + spacing);
            if (columns <= 0 || rows <= 0) return fail("bad_args", "{} ({}x{}) does not hold a single {}x{} tile", image, iw, ih, stw, sth);
            Json ts{{"firstgid", gid}, {"name", t.value("name", std::filesystem::path(image).stem().string())}, {"image", std::filesystem::path(image).lexically_relative(map_dir.empty() ? std::filesystem::path(".") : map_dir).generic_string()},
                    {"imagewidth", iw}, {"imageheight", ih}, {"tilewidth", stw}, {"tileheight", sth}, {"columns", columns}, {"tilecount", columns * rows}, {"spacing", spacing}, {"margin", margin}};
            if (t.contains("solid") && t["solid"].is_array()) {
                Json tiles = Json::array();
                for (const Json& sid : t["solid"]) if (sid.is_number_integer()) tiles.push_back(Json{{"id", sid}, {"properties", Json::array({Json{{"name", "solid"}, {"type", "bool"}, {"value", true}}})}});
                ts["tiles"] = tiles;
            }
            tilesets.push_back(ts);
            gid += static_cast<std::uint32_t>(columns * rows);
        }
        return tilesets;
    };
    auto keep_made = [&](Json doc, const std::string& name, int width, int height) -> Result<Json> {
        POCKET_TRY(map, assets::parse_tilemap(doc.dump(), name));
        map.file = false;
        const assets::TileMap* made = assets_->put_tilemap(std::move(map));
        world_->events().emit(clock_.tick, "tilemap.created", 0, Json{{"path", name}, {"width", width}, {"height", height}}, 0, "tilemap");
        Json sets = Json::array();
        for (const assets::TileSet& st : made->tilesets) sets.push_back(Json{{"name", st.name}, {"first_gid", st.first_gid}, {"tiles", st.tile_count}, {"columns", st.columns}});
        Json ls = Json::array();
        for (const assets::TileLayer& l : made->layers) ls.push_back(l.name);
        return Json{{"map", name}, {"width", width}, {"height", height}, {"layers", ls}, {"tilesets", sets}};
    };
    if (op == "create") {
        // A map made by code: its size in tiles, its tiles' size in pixels, its tile layers (empty)
        // and tilesets, kept under `name` for a TileMap to draw and tilemap.set to fill; no file
        // until tilemap.save gives it one (docs/design/tilemaps.md, Maps made by code).
        if (!assets_) return fail("no_assets", "no asset store");
        const std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "a map needs a name (a project-relative path such as maps/dungeon.tmj, where tilemap.save would write it)");
        const int width = opt<int>(p, "width", 0), height = opt<int>(p, "height", 0);
        if (width <= 0 || height <= 0 || width > 4096 || height > 4096 || static_cast<long long>(width) * height > 4'000'000) return fail("bad_args", "width and height are tiles, 1 to 4096 each and at most four million together");
        const int tw = opt<int>(p, "tile_width", 16), th = opt<int>(p, "tile_height", tw);
        if (tw <= 0 || th <= 0) return fail("bad_args", "tile_width and tile_height are pixels");
        const std::string orientation = opt<std::string>(p, "orientation", "orthogonal");
        if (orientation != "orthogonal" && orientation != "isometric" && orientation != "staggered" && orientation != "hexagonal") return fail("bad_args", "orientation is orthogonal, isometric, staggered or hexagonal");
        Json doc{{"type", "map"}, {"version", "1.10"}, {"orientation", orientation}, {"renderorder", "right-down"}, {"width", width}, {"height", height}, {"tilewidth", tw}, {"tileheight", th}, {"infinite", false}};
        if (orientation == "staggered" || orientation == "hexagonal") {
            doc["staggeraxis"] = opt<std::string>(p, "stagger_axis", "y");
            doc["staggerindex"] = opt<std::string>(p, "stagger_index", "odd");
            if (orientation == "hexagonal") doc["hexsidelength"] = opt<int>(p, "hex_side", th / 2);
        }
        Json layers = Json::array();
        const Json names = p.contains("layers") && p["layers"].is_array() ? p["layers"] : Json::array({"ground"});
        int lid = 1;
        for (const Json& l : names) {
            // A layer's name, or {name, solid?, visible?}.
            Json layer{{"type", "tilelayer"}, {"id", lid++}, {"name", l.is_string() ? l.get<std::string>() : l.value("name", std::string("layer"))}, {"width", width}, {"height", height}, {"x", 0}, {"y", 0}, {"opacity", 1}, {"visible", l.is_object() ? l.value("visible", true) : true}, {"data", Json(std::vector<int>(static_cast<std::size_t>(width) * height, 0))}};
            if (l.is_object() && l.value("solid", false)) layer["properties"] = Json::array({Json{{"name", "solid"}, {"type", "bool"}, {"value", true}}});
            layers.push_back(layer);
        }
        doc["layers"] = layers;
        doc["nextlayerid"] = lid;
        POCKET_TRY(tilesets, made_tilesets(p.value("tilesets", Json::array()), name, tw, th));
        doc["tilesets"] = tilesets;
        return keep_made(std::move(doc), name, width, height);
    }
    if (op == "text") {
        // A map drawn in characters (docs/design/tilemaps.md, Maps in characters): rows of text, a
        // legend from a character to a tile on a layer (or several, bottom first) or to an object,
        // "*" a tile under every cell; layers in the order they first appear, or as `layers` says.
        if (!assets_) return fail("no_assets", "no asset store");
        const std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "a map needs a name (a project-relative path such as maps/level1.tmj)");
        if (!p.contains("rows") || !p["rows"].is_array() || p["rows"].empty()) return fail("bad_args", "rows: [\"#####\", \"#@.c#\", ...], one string a row of cells");
        if (!p.contains("legend") || !p["legend"].is_object()) return fail("bad_args", "legend: {{\"#\": {{layer: \"walls\", tile: 1}}, \"c\": {{object: \"coin\"}}, ...}}");
        std::vector<std::string> rows;
        int width = 0;
        for (const Json& r : p["rows"]) {
            if (!r.is_string()) return fail("bad_args", "every row is a string");
            rows.push_back(r.get<std::string>());
            width = std::max(width, static_cast<int>(rows.back().size()));
        }
        const int height = static_cast<int>(rows.size());
        if (width <= 0 || width > 4096 || height > 4096) return fail("bad_args", "rows make a map 1 to 4096 cells each way");
        const int tw = opt<int>(p, "tile_width", 16), th = opt<int>(p, "tile_height", tw);
        POCKET_TRY(tilesets, made_tilesets(p.value("tilesets", Json::array()), name, tw, th));
        // What each character puts: tiles {layer, tile} (tile a local id of `tileset`, the first by default) and objects.
        struct Put { std::string layer; std::uint32_t gid = 0; };
        struct Mark { std::vector<Put> tiles; std::string object, object_name; };
        std::map<std::string, Mark> marks;
        std::vector<std::string> order;   // layers as they first appear
        auto gid_of = [&](const Json& e) -> Result<std::uint32_t> {
            if (e.contains("gid")) return e["gid"].get<std::uint32_t>();
            const int local = e.value("tile", -1);
            if (local < 0) return fail("bad_args", "a tile in the legend needs tile (a local id) or gid");
            const std::string set = e.value("tileset", std::string());
            for (const Json& ts : tilesets) {
                if (!set.empty() && ts["name"] != set) continue;
                if (local >= ts["tilecount"].get<int>()) return fail("bad_args", "tile {} is outside tileset '{}' (0..{})", local, ts["name"].get<std::string>(), ts["tilecount"].get<int>() - 1);
                return ts["firstgid"].get<std::uint32_t>() + static_cast<std::uint32_t>(local);
            }
            return fail("bad_args", "no tileset {} for tile {}", set.empty() ? std::string("at all (give tilesets)") : "'" + set + "'", local);
        };
        for (const auto& [key, e] : p["legend"].items()) {
            if (key.size() != 1 && key != "*") return fail("bad_args", "legend key '{}' is one character (or * for every cell)", key);
            Mark m;
            auto put = [&](const Json& t) -> Status {
                if (!t.contains("layer")) return fail("bad_args", "legend '{}': a tile needs its layer", key);
                POCKET_TRY(g, gid_of(t));
                const std::string layer = t["layer"].get<std::string>();
                m.tiles.push_back({layer, g});
                if (std::find(order.begin(), order.end(), layer) == order.end()) order.push_back(layer);
                return {};
            };
            if (e.is_null()) { marks[key] = m; continue; }
            if (e.contains("layers") && e["layers"].is_array()) for (const Json& t : e["layers"]) POCKET_TRY_VOID(put(t));
            else if (e.contains("layer")) POCKET_TRY_VOID(put(e));
            if (e.contains("object")) {
                m.object = e["object"].get<std::string>();
                m.object_name = e.value("name", std::string(1, static_cast<char>(std::toupper(static_cast<unsigned char>(m.object.empty() ? 'o' : m.object[0])))) + m.object.substr(m.object.empty() ? 0 : 1));
                if (e.contains("under")) POCKET_TRY_VOID(put(e["under"]));
            }
            if (m.tiles.empty() && m.object.empty()) return fail("bad_args", "legend '{}' puts nothing: give {{layer, tile}}, {{layers: [...]}} or {{object}}", key);
            marks[key] = m;
        }
        // The "*" tiles first, at the bottom.
        if (auto star = marks.find("*"); star != marks.end()) {
            std::vector<std::string> first;
            for (const Put& t : star->second.tiles) if (std::find(first.begin(), first.end(), t.layer) == first.end()) first.push_back(t.layer);
            for (const std::string& l : order) if (std::find(first.begin(), first.end(), l) == first.end()) first.push_back(l);
            order = first;
        }
        std::map<std::string, bool> solid;
        if (p.contains("layers") && p["layers"].is_array()) {
            std::vector<std::string> given;
            for (const Json& l : p["layers"]) {
                const std::string ln = l.is_string() ? l.get<std::string>() : l.value("name", std::string());
                given.push_back(ln);
                if (l.is_object() && l.value("solid", false)) solid[ln] = true;
            }
            for (const std::string& l : order) if (std::find(given.begin(), given.end(), l) == given.end()) given.push_back(l);
            order = given;
        }
        if (order.empty()) order.push_back("ground");
        std::map<std::string, std::vector<std::uint32_t>> data;
        for (const std::string& l : order) data[l].assign(static_cast<std::size_t>(width) * height, 0);
        Json objects = Json::array();
        std::map<std::string, int> counts;
        int oid = 1;
        std::string unknown;
        for (int y = 0; y < height; ++y) {
            for (int x = 0; x < width; ++x) {
                const char c = x < static_cast<int>(rows[y].size()) ? rows[y][static_cast<std::size_t>(x)] : ' ';
                const std::size_t at = static_cast<std::size_t>(y) * width + x;
                if (auto star = marks.find("*"); star != marks.end()) for (const Put& t : star->second.tiles) data[t.layer][at] = t.gid;
                if (c == ' ') continue;
                const auto it = marks.find(std::string(1, c));
                if (it == marks.end()) {
                    if (unknown.find(c) == std::string::npos) unknown += c;
                    continue;
                }
                for (const Put& t : it->second.tiles) data[t.layer][at] = t.gid;
                if (!it->second.object.empty()) {
                    const int n = ++counts[it->second.object];
                    objects.push_back(Json{{"id", oid++}, {"name", std::format("{}_{}", it->second.object_name, n)}, {"type", it->second.object}, {"x", (x + 0.5) * tw}, {"y", (y + 0.5) * th}, {"width", 0}, {"height", 0}, {"point", true}, {"rotation", 0}, {"visible", true}});
                }
            }
        }
        if (!unknown.empty()) return fail("bad_args", "the rows use characters the legend does not have: '{}' (a space is an empty cell)", unknown);
        Json doc{{"type", "map"}, {"version", "1.10"}, {"orientation", "orthogonal"}, {"renderorder", "right-down"}, {"width", width}, {"height", height}, {"tilewidth", tw}, {"tileheight", th}, {"infinite", false}};
        Json layers = Json::array();
        int lid = 1;
        for (const std::string& l : order) {
            Json layer{{"type", "tilelayer"}, {"id", lid++}, {"name", l}, {"width", width}, {"height", height}, {"x", 0}, {"y", 0}, {"opacity", 1}, {"visible", true}, {"data", data[l]}};
            if (solid[l]) layer["properties"] = Json::array({Json{{"name", "solid"}, {"type", "bool"}, {"value", true}}});
            layers.push_back(layer);
        }
        if (!objects.empty()) layers.push_back(Json{{"type", "objectgroup"}, {"id", lid++}, {"name", "objects"}, {"x", 0}, {"y", 0}, {"opacity", 1}, {"visible", true}, {"draworder", "topdown"}, {"objects", objects}});
        doc["layers"] = layers;
        doc["nextlayerid"] = lid;
        doc["nextobjectid"] = oid;
        doc["tilesets"] = tilesets;
        POCKET_TRY(made, keep_made(std::move(doc), name, width, height));
        made["objects"] = counts;
        return made;
    }
    if (!p.contains("entity")) return fail("bad_args", "missing 'entity' (a TileMap's entity; tilemap.create makes a map)");
    world::EntityId id = resolve_entity(p["entity"]);
    if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
    const auto* tmc = w.try_get<world::TileMap>(id);
    if (!tmc) return fail("no_tilemap", "entity {} has no TileMap", id);
    if (!assets_) return fail("no_assets", "no asset store");
    POCKET_TRY(map, assets_->tilemap_mut(tmc->map));
    const float ts = tmc->tile_size > 0 ? tmc->tile_size : 1.0f;
    Vec3 origin{0, 0, 0};
    if (const auto* wt = w.try_get<world::WorldTransform>(id)) origin = wt->position;
    else if (const auto* t = w.try_get<world::Transform>(id)) origin = t->position;
    // World <-> tile: the entity sits at the map's top-left corner; rows go down (-Y). An
    // orthogonal map's cells are tile_size square; the others are read through the map's own
    // pixel geometry at tile_size per cell width (docs/design/tilemaps.md, Orientations).
    const double sx = ts / map->tile_width, sy = map->orthogonal() ? ts / map->tile_height : sx;
    auto to_cell = [&](double wx, double wy, int& cx, int& cy) {
        if (map->orthogonal()) {
            cx = static_cast<int>(std::floor((wx - origin.x) / ts));
            cy = static_cast<int>(std::floor((origin.y - wy) / ts));
        } else {
            (void)map->cell_at_pixel(static_cast<float>((wx - origin.x) / sx), static_cast<float>((origin.y - wy) / sy), cx, cy);
        }
    };
    auto cell_center = [&](int cx, int cy) {
        if (map->orthogonal()) return Json{{"x", origin.x + (cx + 0.5) * ts}, {"y", origin.y - (cy + 0.5) * ts}};
        const Vec2 c = map->tile_pixel(cx, cy);
        return Json{{"x", origin.x + (c.x + map->tile_width * 0.5) * sx}, {"y", origin.y - (c.y + map->tile_height * 0.5) * sy}};
    };
    if (op == "info") {
        Json j = map->describe();
        j["entity"] = id;
        j["tile_size"] = ts;
        j["origin"] = Json{{"x", origin.x}, {"y", origin.y}};
        const Vec2 px = map->pixel_size();
        j["bounds"] = Json{{"min", {{"x", origin.x}, {"y", origin.y - px.y * sy}}}, {"max", {{"x", origin.x + px.x * sx}, {"y", origin.y}}}};
        return j;
    }
    if (op == "cell") {
        int cx, cy;
        to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        Json j = Json{{"tile_x", cx}, {"tile_y", cy}, {"inside", cx >= 0 && cy >= 0 && cx < map->width && cy < map->height}};
        j["center"] = cell_center(cx, cy);
        return j;
    }
    if (op == "rows") {
        // The map (or a window of it) as rows of characters, the reverse of tilemap.text: per tile
        // layer, a character for each tile used ('.' empty) and a legend from character to tile; with
        // solid, the collision instead ('#' solid, '-' one way, '/' a slope, '.' free).
        const int x0 = std::max(opt<int>(p, "tile_x", 0), 0), y0 = std::max(opt<int>(p, "tile_y", 0), 0);
        const int w = std::clamp(opt<int>(p, "width", map->width - x0), 0, std::max(map->width - x0, 0));
        const int h = std::clamp(opt<int>(p, "height", map->height - y0), 0, std::max(map->height - y0, 0));
        Json j{{"tile_x", x0}, {"tile_y", y0}, {"width", w}, {"height", h}};
        if (opt<bool>(p, "solid", false)) {
            Json rows = Json::array();
            for (int y = y0; y < y0 + h; ++y) {
                std::string row;
                for (int x = x0; x < x0 + w; ++x) {
                    const int s = map->solidity_at(x, y);
                    row += s == 1 ? '#' : s == 2 ? '-' : s == 3 ? '/' : '.';
                }
                rows.push_back(row);
            }
            j["rows"] = rows;
            return j;
        }
        static constexpr std::string_view kMarks = "#abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789@$%&*+=?";
        const std::string layer_name = opt<std::string>(p, "layer", "");
        Json layers = Json::array();
        for (const assets::TileLayer& l : map->layers) {
            if (!layer_name.empty() && l.name != layer_name) continue;
            std::map<std::uint32_t, char> marks;
            Json legend = Json::object(), rows = Json::array();
            bool more = false;
            for (int y = y0; y < y0 + h; ++y) {
                std::string row;
                for (int x = x0; x < x0 + w; ++x) {
                    const std::uint32_t gid = x < l.width && y < l.height ? l.gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(l.width) + static_cast<std::size_t>(x)] & assets::TileMap::kIdMask : 0;
                    if (!gid) { row += '.'; continue; }
                    auto it = marks.find(gid);
                    if (it == marks.end()) {
                        if (marks.size() >= kMarks.size()) { row += '?'; more = true; continue; }
                        const char c = kMarks[marks.size()];
                        it = marks.emplace(gid, c).first;
                        Json entry{{"gid", gid}};
                        if (const assets::TileSet* set = map->tileset_for(gid)) {
                            entry["tileset"] = set->name;
                            entry["id"] = static_cast<int>(gid - set->first_gid);
                        }
                        legend[std::string(1, c)] = entry;
                    }
                    row += it->second;
                }
                rows.push_back(row);
            }
            Json lj{{"layer", l.name}, {"rows", rows}, {"legend", legend}};
            if (more) lj["more"] = "more tiles than characters: the rest show as ?";
            layers.push_back(lj);
        }
        if (!layer_name.empty() && layers.empty()) return fail("no_layer", "no tile layer {}", layer_name);
        j["layers"] = layers;
        return j;
    }
    if (op == "tile") {
        int cx, cy;
        if (p.contains("tile_x") || p.contains("tile_y")) {
            cx = opt<int>(p, "tile_x", 0);
            cy = opt<int>(p, "tile_y", 0);
        } else {
            to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        }
        std::string layer_name = opt<std::string>(p, "layer", "");
        Json layers = Json::array();
        for (const assets::TileLayer& l : map->layers) {
            if (!layer_name.empty() && l.name != layer_name) continue;
            Json lj{{"layer", l.name}, {"gid", 0}, {"id", nullptr}, {"tileset", nullptr}, {"solid", false}, {"properties", Json::object()}};
            if (cx >= 0 && cy >= 0 && cx < l.width && cy < l.height) {
                std::uint32_t gid = l.gids[static_cast<std::size_t>(cy) * static_cast<std::size_t>(l.width) + static_cast<std::size_t>(cx)];
                lj["gid"] = gid & assets::TileMap::kIdMask;
                lj["flip_h"] = (gid & assets::TileMap::kFlipH) != 0;
                lj["flip_v"] = (gid & assets::TileMap::kFlipV) != 0;
                if (const assets::TileSet* set = gid ? map->tileset_for(gid) : nullptr) {
                    int local = static_cast<int>((gid & assets::TileMap::kIdMask) - set->first_gid);
                    lj["id"] = local;
                    lj["tileset"] = set->name;
                    if (set->animations.contains(local)) lj["frame"] = set->frame_at(local, assets_->tile_time());   // the id drawn now
                    lj["solid"] = l.solid_layer() || set->solid(local);
                    lj["one_way"] = set->one_way(local);
                    if (const auto* sh = set->shapes_of(local)) {
                        Json shapes = Json::array();
                        for (const assets::TileSet::Shape& s : *sh) shapes.push_back(Json::array({s.x0, s.y0, s.x1, s.y1}));
                        lj["shapes"] = shapes;   // fractions of the tile, unflipped
                    }
                    if (auto it = set->tile_properties.find(local); it != set->tile_properties.end()) lj["properties"] = it->second;
                }
            }
            layers.push_back(lj);
        }
        Json j{{"tile_x", cx}, {"tile_y", cy}, {"solid", map->solid_at(cx, cy)}, {"one_way", map->solidity_at(cx, cy) == 2}, {"slope", map->slope_at(cx, cy)}, {"layers", layers}};
        j["center"] = cell_center(cx, cy);
        return j;
    }
    if (op == "sight" || op == "fov") {
        // Line of sight and field of view over the cells (docs/design/tilemaps.md, Sight): a cell
        // hides what is behind it when it is solid (not one-way), or, with `layers`, when one of
        // those layers has a tile there; a tile's `opaque` property says otherwise either way.
        if (!map->orthogonal()) return fail("bad_map", "tilemap.{} needs an orthogonal map; this one is {}", op, map->describe().value("orientation", std::string("not")));
        std::vector<const assets::TileLayer*> blockers;
        if (p.contains("layers")) {
            if (!p["layers"].is_array()) return fail("bad_args", "layers: [\"walls\", ...]");
            for (const Json& n : p["layers"]) {
                const auto it = std::find_if(map->layers.begin(), map->layers.end(), [&](const assets::TileLayer& l) { return n.is_string() && l.name == n.get<std::string>(); });
                if (it == map->layers.end()) {
                    std::string names;
                    for (const auto& l : map->layers) names += (names.empty() ? "" : ", ") + l.name;
                    return fail("no_layer", "no tile layer {} (the map has {})", n.dump(), names);
                }
                blockers.push_back(&*it);
            }
        }
        std::vector<signed char> memo(static_cast<std::size_t>(std::max(map->width, 0)) * static_cast<std::size_t>(std::max(map->height, 0)), -1);
        auto opaque = [&](int cx, int cy) -> bool {
            if (cx < 0 || cy < 0 || cx >= map->width || cy >= map->height) return false;
            signed char& m = memo[static_cast<std::size_t>(cy) * static_cast<std::size_t>(map->width) + static_cast<std::size_t>(cx)];
            if (m >= 0) return m != 0;
            bool hides = false, has = false;
            if (blockers.empty()) hides = map->solidity_at(cx, cy) == 1;
            for (const assets::TileLayer& l : map->layers) {
                if (cx >= l.width || cy >= l.height) continue;
                const std::uint32_t gid = l.gids[static_cast<std::size_t>(cy) * static_cast<std::size_t>(l.width) + static_cast<std::size_t>(cx)];
                if (!gid) continue;
                if (!blockers.empty() && std::find(blockers.begin(), blockers.end(), &l) != blockers.end()) hides = true;
                if (const assets::TileSet* set = map->tileset_for(gid)) {
                    const int local = static_cast<int>((gid & assets::TileMap::kIdMask) - set->first_gid);
                    if (auto it = set->tile_properties.find(local); it != set->tile_properties.end() && it->second.contains("opaque") && it->second["opaque"].is_boolean()) {
                        hides = it->second["opaque"].get<bool>();
                        has = true;
                    }
                }
                if (has) break;
            }
            m = hides ? 1 : 0;
            return hides;
        };
        // In cell units, y down the map.
        auto to_grid = [&](const Json& v, double& gx, double& gy) {
            const bool pair = v.is_array() && v.size() >= 2;   // [x, y] as well as {x, y}
            gx = ((pair ? v[0].get<double>() : v.value("x", 0.0)) - origin.x) / ts;
            gy = (origin.y - (pair ? v[1].get<double>() : v.value("y", 0.0))) / ts;
        };
        // The cells a segment crosses, in order (Amanatides and Woo); the first opaque one past the
        // start stops it. Answers whether it got through, and where it stopped.
        auto trace = [&](double ax, double ay, double bx, double by, int& hx, int& hy, double& t_hit) {
            return map->line_clear(ax, ay, bx, by, opaque, &hx, &hy, &t_hit);
        };
        double ax, ay;
        if (!p.contains("from")) return fail("bad_args", "tilemap.{} needs from: {{x, y}}", op);
        to_grid(p["from"], ax, ay);
        if (op == "sight") {
            if (!p.contains("to")) return fail("bad_args", "tilemap.sight needs to: {{x, y}}");
            double bx, by;
            to_grid(p["to"], bx, by);
            int hx = 0, hy = 0;
            double t = 1.0;
            const bool clear = trace(ax, ay, bx, by, hx, hy, t);
            Json j{{"visible", clear}, {"distance", std::hypot(bx - ax, by - ay) * ts}};
            if (!clear) {
                j["blocked_at"] = Json{{"tile_x", hx}, {"tile_y", hy}, {"point", Json{{"x", origin.x + (ax + (bx - ax) * t) * ts}, {"y", origin.y - (ay + (by - ay) * t) * ts}}}, {"distance", std::hypot(bx - ax, by - ay) * t * ts}};
            }
            return j;
        }
        // Field of view: every cell within `radius` cells whose center or one of four points near its
        // corners can be seen from `from`. Walls that face the viewer are seen; what is behind is not.
        const double radius = std::clamp(opt<double>(p, "radius", 8.0), 0.0, 256.0);
        const int ox = static_cast<int>(std::floor(ax)), oy = static_cast<int>(std::floor(ay)), r = static_cast<int>(std::ceil(radius));
        Json cells = Json::array();
        int seen_walls = 0;
        for (int cy = oy - r; cy <= oy + r; ++cy) {
            for (int cx = ox - r; cx <= ox + r; ++cx) {
                if (cx < 0 || cy < 0 || cx >= map->width || cy >= map->height) continue;
                const double ddx = cx + 0.5 - ax, ddy = cy + 0.5 - ay;
                if (ddx * ddx + ddy * ddy > radius * radius) continue;
                bool seen = cx == ox && cy == oy;
                static constexpr double kPoints[5][2] = {{0.5, 0.5}, {0.1, 0.1}, {0.9, 0.1}, {0.1, 0.9}, {0.9, 0.9}};
                for (int k = 0; k < 5 && !seen; ++k) {
                    int hx = 0, hy = 0;
                    double t = 1.0;
                    seen = trace(ax, ay, cx + kPoints[k][0], cy + kPoints[k][1], hx, hy, t) || (hx == cx && hy == cy);
                }
                if (!seen) continue;
                cells.push_back(Json::array({cx, cy}));
                if (opaque(cx, cy)) seen_walls++;
            }
        }
        return Json{{"from", Json{{"tile_x", ox}, {"tile_y", oy}}}, {"radius", radius}, {"count", cells.size()}, {"walls", seen_walls}, {"cells", cells}};
    }
    if (op == "solid") {
        int cx, cy;
        if (p.contains("tile_x") || p.contains("tile_y")) {
            cx = opt<int>(p, "tile_x", 0);
            cy = opt<int>(p, "tile_y", 0);
        } else {
            to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        }
        Json j{{"solid", map->solid_at(cx, cy)}, {"one_way", map->solidity_at(cx, cy) == 2}, {"slope", map->slope_at(cx, cy)}, {"tile_x", cx}, {"tile_y", cy}};
        if (!p.contains("tile_x") && !p.contains("tile_y") && map->orthogonal()) {
            // The point itself, against the tile's collision shapes (the whole cell without them).
            const double fx = (opt<double>(p, "x", 0.0) - origin.x) / ts - cx, fy = (origin.y - opt<double>(p, "y", 0.0)) / ts - cy;
            j["inside"] = map->solid_at_point(cx, cy, static_cast<float>(fx), static_cast<float>(fy));
        }
        return j;
    }
    if (op == "spawn") {
        // Prefabs at Tiled's objects: `prefabs` maps an object type to a prefab path; every object
        // of a listed type (in one layer, or all) becomes an instance named after the object at its
        // center (a point's spot), with its properties named Component.field applied on top.
        if (!p.contains("prefabs") || !p["prefabs"].is_object()) return fail("bad_args", "give prefabs: {{type: \"prefabs/x.json\", ...}}");
        const std::string layer_name = opt<std::string>(p, "layer", "");
        Json spawned = Json::array();
        for (const assets::ObjectLayer& ol : map->object_layers) {
            if (!layer_name.empty() && ol.name != layer_name) continue;
            for (const assets::MapObject& o : ol.objects) {
                if (!p["prefabs"].contains(o.type) || !p["prefabs"][o.type].is_string()) continue;
                const Vec2 opx = map->object_pixel(o.x, o.y);
                const double wx = origin.x + opx.x * sx, wy = origin.y - opx.y * sy, ww = o.width * sx, wh = o.height * sy;
                const double cxw = wx + ww / 2, cyw = o.gid ? wy + wh / 2 : wy - wh / 2;
                Json components = Json::object();
                components["Transform"] = Json{{"position", Json{{"x", cxw}, {"y", cyw}, {"z", origin.z}}}};
                if (o.properties.is_object()) {
                    for (const auto& [key, value] : o.properties.items()) {
                        const std::size_t dot = key.find('.');
                        if (dot == std::string::npos || dot == 0 || dot + 1 >= key.size()) continue;
                        components[key.substr(0, dot)][key.substr(dot + 1)] = value;
                    }
                }
                Json args{{"prefab", p["prefabs"][o.type]}, {"components", components}};
                if (!o.name.empty()) args["name"] = o.name;
                if (p.contains("parent") && !p["parent"].is_null()) args["parent"] = p["parent"];
                POCKET_TRY(made, command("world.instantiate", args, "tilemap.spawn"));
                Json row{{"object", o.name}, {"type", o.type}, {"layer", ol.name}, {"x", cxw}, {"y", cyw}};
                if (made.is_object() && made.contains("roots") && made["roots"].is_array() && !made["roots"].empty()) row["entity"] = made["roots"][0];
                spawned.push_back(std::move(row));
            }
        }
        return Json{{"spawned", spawned}, {"count", spawned.size()}};
    }
    if (op == "paths") {
        // Tiled's polylines and polygons (of one layer, or all) as Paths a PathFollower can run
        // along (docs/design/paths.md): one entity each, named after the object, its points in the
        // world where the map draws them; a polygon closed. Straight from point to point unless smooth.
        const std::string layer_name = opt<std::string>(p, "layer", "");
        const bool smooth = opt<bool>(p, "smooth", false);
        Json made = Json::array();
        int n = 0;
        for (const assets::ObjectLayer& ol : map->object_layers) {
            if (!layer_name.empty() && ol.name != layer_name) continue;
            for (const assets::MapObject& o : ol.objects) {
                if (o.points.size() < 2) continue;
                Json pts = Json::array();
                for (const Vec2& q : o.points) {
                    const Vec2 qp = map->object_pixel(o.x + q.x, o.y + q.y);
                    pts.push_back(Json{{"x", origin.x + qp.x * sx}, {"y", origin.y - qp.y * sy}, {"z", origin.z}});
                }
                const std::string name = o.name.empty() ? std::format("Path_{}", ++n) : o.name;
                POCKET_TRY(spawned, command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json::object()}, {"Path", Json{{"points", pts}, {"closed", o.closed}, {"smooth", smooth}}}}}}, "tilemap"));
                made.push_back(Json{{"object", o.name}, {"layer", ol.name}, {"entity", spawned["id"]}, {"path", spawned["path"]}, {"points", pts.size()}, {"closed", o.closed}});
            }
        }
        return Json{{"paths", made}};
    }
    if (op == "objects") {
        std::string layer_name = opt<std::string>(p, "layer", "");
        Json out = Json::array();
        for (const assets::ObjectLayer& ol : map->object_layers) {
            if (!layer_name.empty() && ol.name != layer_name) continue;
            for (const assets::MapObject& o : ol.objects) {
                // Tiled objects are in pixels from the top-left (isometric ones in the unprojected
                // tile space, taken through the projection); tile objects (gid) anchor bottom-left.
                const Vec2 op = map->object_pixel(o.x, o.y);
                double wx = origin.x + op.x * sx;
                double wy = origin.y - op.y * sy;
                double ww = o.width * sx, wh = o.height * sy;
                Json oj{{"layer", ol.name}, {"name", o.name}, {"type", o.type}, {"properties", o.properties}, {"point", o.point}};
                oj["x"] = wx;
                oj["y"] = wy;
                oj["width"] = ww;
                oj["height"] = wh;
                oj["center"] = Json{{"x", wx + ww / 2}, {"y", o.gid ? wy + wh / 2 : wy - wh / 2}};
                if (o.gid) oj["gid"] = o.gid & assets::TileMap::kIdMask;
                if (!o.points.empty()) {
                    // A polyline's or polygon's points in the world.
                    Json pts = Json::array();
                    for (const Vec2& q : o.points) {
                        const Vec2 qp = map->object_pixel(o.x + q.x, o.y + q.y);
                        pts.push_back(Json{{"x", origin.x + qp.x * sx}, {"y", origin.y - qp.y * sy}});
                    }
                    oj["points"] = pts;
                    oj[o.closed ? "polygon" : "polyline"] = true;
                }
                out.push_back(oj);
            }
        }
        return out;
    }
    // Editing (docs/design/tilemaps.md, Editing): the map asset changes for every entity that
    // draws it, the layer mesh is rebuilt on the next frame, Body2D and tilemap.solid see it at once.
    auto cell_of = [&](int& cx, int& cy) {
        if (p.contains("tile_x") || p.contains("tile_y")) {
            cx = opt<int>(p, "tile_x", 0);
            cy = opt<int>(p, "tile_y", 0);
        } else {
            to_cell(opt<double>(p, "x", 0.0), opt<double>(p, "y", 0.0), cx, cy);
        }
    };
    auto layer_of = [&]() -> Result<std::string> {
        std::string name = opt<std::string>(p, "layer", "");
        if (name.empty()) name = tmc->layer;
        if (name.empty() && !map->layers.empty()) name = map->layers.front().name;
        if (name.empty()) return fail("no_layers", "{} has no tile layers", map->path);
        return name;
    };
    // The tile to put: gid (global id, 0 clears), or id in a tileset (the first by default), or clear.
    auto gid_of = [&]() -> Result<std::uint32_t> {
        std::uint32_t gid = 0;
        if (p.contains("gid")) {
            gid = static_cast<std::uint32_t>(opt<double>(p, "gid", 0.0));
        } else if (p.contains("id")) {
            const int local = opt<int>(p, "id", 0);
            const std::string set_name = opt<std::string>(p, "tileset", "");
            const assets::TileSet* set = nullptr;
            for (const assets::TileSet& t : map->tilesets) if (set_name.empty() || t.name == set_name) { set = &t; break; }
            if (!set) return fail("unknown_tileset", "{} has no tileset '{}'", map->path, set_name);
            if (local < 0 || (set->tile_count > 0 && local >= set->tile_count)) return fail("bad_tile", "tile id {} is outside tileset '{}' (0..{})", local, set->name, set->tile_count - 1);
            gid = set->first_gid + static_cast<std::uint32_t>(local);
        } else if (!opt<bool>(p, "clear", false)) {
            return fail("bad_args", "give 'gid', 'id' (with an optional 'tileset') or 'clear': true");
        }
        if (gid != 0) {
            if (!map->tileset_for(gid)) return fail("bad_gid", "gid {} is not covered by any tileset of {}", gid & assets::TileMap::kIdMask, map->path);
            if (opt<bool>(p, "flip_h", false)) gid |= assets::TileMap::kFlipH;
            if (opt<bool>(p, "flip_v", false)) gid |= assets::TileMap::kFlipV;
        }
        return gid;
    };
    if (op == "set") {
        int cx, cy;
        cell_of(cx, cy);
        POCKET_TRY(layer, layer_of());
        POCKET_TRY(gid, gid_of());
        POCKET_TRY(was, map->set(layer, cx, cy, gid));
        const bool changed = was != gid;
        if (changed) world_->events().emit(clock_.tick, "tilemap.changed", id, Json{{"path", map->path}, {"layer", layer}, {"tile_x", cx}, {"tile_y", cy}, {"width", 1}, {"height", 1}, {"gid", gid}, {"was", was}, {"count", 1}}, 0, "tilemap");
        Json j{{"tile_x", cx}, {"tile_y", cy}, {"layer", layer}, {"gid", gid}, {"was", was}, {"changed", changed}, {"revision", map->revision}};
        j["center"] = cell_center(cx, cy);
        return j;
    }
    if (op == "fill") {
        POCKET_TRY(layer, layer_of());
        POCKET_TRY(gid, gid_of());
        int x0 = opt<int>(p, "tile_x", 0), y0 = opt<int>(p, "tile_y", 0);
        const int fw = opt<int>(p, "width", 1), fh = opt<int>(p, "height", 1);
        if (fw <= 0 || fh <= 0) return fail("bad_args", "width and height must be positive");
        const int x1 = std::min(x0 + fw, map->width), y1 = std::min(y0 + fh, map->height);
        x0 = std::max(x0, 0);
        y0 = std::max(y0, 0);
        int changed = 0;
        for (int y = y0; y < y1; ++y) {
            for (int x = x0; x < x1; ++x) {
                POCKET_TRY(was, map->set(layer, x, y, gid));
                changed += was != gid;
            }
        }
        if (changed > 0) world_->events().emit(clock_.tick, "tilemap.changed", id, Json{{"path", map->path}, {"layer", layer}, {"tile_x", x0}, {"tile_y", y0}, {"width", std::max(0, x1 - x0)}, {"height", std::max(0, y1 - y0)}, {"gid", gid}, {"count", changed}}, 0, "tilemap");
        return Json{{"layer", layer}, {"gid", gid}, {"changed", changed}, {"revision", map->revision}, {"tile_x", x0}, {"tile_y", y0}, {"width", std::max(0, x1 - x0)}, {"height", std::max(0, y1 - y0)}};
    }
    // Layers and tilesets at runtime: a layer is added last (drawn on top), empty, the map's size;
    // a tileset after the last; both are saved with the map.
    auto layer_json = [&](const assets::TileLayer& l) {
        std::size_t index = 0;
        for (; index < map->layers.size(); ++index) if (&map->layers[index] == &l) break;
        std::size_t filled = 0;
        for (std::uint32_t g : l.gids) filled += g != 0;
        return Json{{"layer", l.name}, {"id", l.id}, {"index", index}, {"width", l.width}, {"height", l.height}, {"visible", l.visible}, {"opacity", l.opacity}, {"solid", l.solid_layer()}, {"tiles", filled}, {"properties", l.properties}, {"layers", map->layers.size()}, {"revision", map->revision}};
    };
    if (op == "add_layer") {
        const std::string name = opt<std::string>(p, "name", "");
        Json props = p.contains("properties") && p["properties"].is_object() ? p["properties"] : Json::object();
        if (p.contains("solid")) props["solid"] = opt<bool>(p, "solid", false);
        POCKET_TRY(layer, map->add_layer(name, opt<bool>(p, "visible", true), static_cast<float>(opt<double>(p, "opacity", 1.0)), props));
        world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", layer->name}, {"op", "added"}}, 0, "tilemap");
        return layer_json(*layer);
    }
    if (op == "remove_layer") {
        const std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "missing 'name'");
        POCKET_TRY_VOID(map->remove_layer(name));
        world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", name}, {"op", "removed"}}, 0, "tilemap");
        return Json{{"layer", name}, {"removed", true}, {"layers", map->layers.size()}, {"revision", map->revision}};
    }
    if (op == "layer") {
        // Read a layer, or change its visibility, opacity, solidity, properties or name.
        const std::string name = opt<std::string>(p, "name", "");
        assets::TileLayer* layer = map->layer_mut(name);
        if (!layer) return fail("unknown_layer", "{} has no tile layer '{}'", map->path, name);
        bool changed = false;
        if (p.contains("visible")) { const bool v = opt<bool>(p, "visible", true); changed |= v != layer->visible; layer->visible = v; }
        if (p.contains("opacity")) { const float o = std::clamp(static_cast<float>(opt<double>(p, "opacity", 1.0)), 0.0f, 1.0f); changed |= o != layer->opacity; layer->opacity = o; }
        if (p.contains("solid")) { if (!layer->properties.is_object()) layer->properties = Json::object(); const bool sd = opt<bool>(p, "solid", false); changed |= layer->solid_layer() != sd; layer->properties["solid"] = sd; }
        if (p.contains("properties") && p["properties"].is_object()) { if (!layer->properties.is_object()) layer->properties = Json::object(); for (auto& [k, v] : p["properties"].items()) { changed |= layer->properties.value(k, Json()) != v; layer->properties[k] = v; } }
        if (p.contains("rename")) {
            const std::string to = opt<std::string>(p, "rename", "");
            if (to.empty()) return fail("bad_args", "'rename' needs a name");
            if (to != layer->name && map->layer(to)) return fail("duplicate_layer", "{} already has a tile layer '{}'", map->path, to);
            changed |= to != layer->name;
            layer->name = to;
        }
        bool moved = false;
        if (p.contains("index")) {
            const int index = opt<int>(p, "index", 0);
            if (index < 0) return fail("bad_args", "index must not be negative");
            std::size_t was = 0;
            for (; was < map->layers.size(); ++was) if (&map->layers[was] == layer) break;
            const std::string moved_name = layer->name;   // the pointer is stale once the vector moves
            POCKET_TRY_VOID(map->move_layer(moved_name, static_cast<std::size_t>(index)));
            moved = static_cast<std::size_t>(index) != was;
            layer = map->layer_mut(moved_name);
        }
        if (changed) {
            ++map->revision;
            ++layer->revision;
            // The document keeps the layer's attributes too: patch the k-th tile layer.
            std::size_t k = 0;
            for (; k < map->layers.size(); ++k) if (&map->layers[k] == layer) break;
            if (map->source.is_object() && map->source.contains("layers") && map->source["layers"].is_array()) {
                std::size_t seen = 0;
                std::function<bool(Json&)> patch_kth = [&](Json& list) -> bool {
                    for (Json& l : list) {
                        if (!l.is_object()) continue;
                        const std::string type = l.value("type", "tilelayer");
                        if (type == "group") { if (l.contains("layers") && l["layers"].is_array() && patch_kth(l["layers"])) return true; continue; }
                        if (type != "tilelayer") continue;
                        if (seen++ != k) continue;
                        l["name"] = layer->name;
                        l["visible"] = layer->visible;
                        l["opacity"] = layer->opacity;
                        if (layer->properties.is_object() && !layer->properties.empty()) {
                            Json props = Json::array();
                            for (auto it = layer->properties.begin(); it != layer->properties.end(); ++it) {
                                const Json& v = it.value();
                                const char* type_name = v.is_boolean() ? "bool" : v.is_number_integer() ? "int" : v.is_number() ? "float" : "string";
                                props.push_back(Json{{"name", it.key()}, {"type", type_name}, {"value", v.is_string() || v.is_number() || v.is_boolean() ? v : Json(v.dump())}});
                            }
                            l["properties"] = props;
                        }
                        return true;
                    }
                    return false;
                };
                patch_kth(map->source["layers"]);
            }
            world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", layer->name}, {"op", "changed"}}, 0, "tilemap");
        } else if (moved) {
            world_->events().emit(clock_.tick, "tilemap.layer", id, Json{{"path", map->path}, {"layer", layer->name}, {"op", "moved"}}, 0, "tilemap");
        }
        Json j = layer_json(*layer);
        j["changed"] = changed || moved;
        return j;
    }
    if (op == "add_tileset") {
        assets::TileSet set;
        set.name = opt<std::string>(p, "name", "");
        const std::string image = opt<std::string>(p, "image", "");
        if (image.empty()) return fail("bad_args", "missing 'image' (a project-relative image path)");
        auto img = assets_->image(image);
        if (!img) return fail("bad_image", "{}: {}", image, img.error().message);
        set.image = image;
        set.image_width = static_cast<int>((*img)->width);
        set.image_height = static_cast<int>((*img)->height);
        set.tile_width = opt<int>(p, "tile_width", map->tile_width);
        set.tile_height = opt<int>(p, "tile_height", map->tile_height);
        set.spacing = opt<int>(p, "spacing", 0);
        set.margin = opt<int>(p, "margin", 0);
        if (set.tile_width <= 0 || set.tile_height <= 0) return fail("bad_args", "tile_width and tile_height must be positive");
        set.columns = (set.image_width - 2 * set.margin + set.spacing) / (set.tile_width + set.spacing);
        if (set.columns <= 0) return fail("bad_args", "{} ({}x{}) does not hold a single {}x{} tile", image, set.image_width, set.image_height, set.tile_width, set.tile_height);
        set.tile_count = set.columns * ((set.image_height - 2 * set.margin + set.spacing) / (set.tile_height + set.spacing));
        if (p.contains("tiles") && p["tiles"].is_object()) {
            for (auto& [key, props] : p["tiles"].items()) {
                if (!props.is_object()) continue;
                int tile_id = 0;
                const auto [end, ec] = std::from_chars(key.data(), key.data() + key.size(), tile_id);
                if (ec != std::errc() || end != key.data() + key.size()) return fail("bad_args", "tile ids in 'tiles' must be numbers, not '{}'", key);
                set.tile_properties[tile_id] = props;
            }
        }
        POCKET_TRY(added, map->add_tileset(std::move(set)));
        world_->events().emit(clock_.tick, "tilemap.tileset", id, Json{{"path", map->path}, {"tileset", added->name}, {"first_gid", added->first_gid}, {"op", "added"}}, 0, "tilemap");
        return Json{{"tileset", added->name}, {"first_gid", added->first_gid}, {"tile_count", added->tile_count}, {"columns", added->columns}, {"image", added->image}, {"tile_width", added->tile_width}, {"tile_height", added->tile_height}, {"tilesets", map->tilesets.size()}, {"revision", map->revision}};
    }
    if (op == "remove_tileset") {
        const std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "missing 'name'");
        POCKET_TRY_VOID(map->remove_tileset(name));
        world_->events().emit(clock_.tick, "tilemap.tileset", id, Json{{"path", map->path}, {"tileset", name}, {"op", "removed"}}, 0, "tilemap");
        return Json{{"tileset", name}, {"removed", true}, {"tilesets", map->tilesets.size()}, {"revision", map->revision}};
    }
    if (op == "copy") {
        // The entity's own copy of its map, under a new name: edits to it leave the map the other
        // entities draw alone. Named after the entity unless told otherwise.
        const std::string name = opt<std::string>(p, "name", tmc->map + "@" + w.name(id));
        POCKET_TRY(copy, assets_->copy_tilemap(tmc->map, name));
        POCKET_TRY_VOID(w.set(id, "TileMap", Json{{"map", name}}));
        world_->events().emit(clock_.tick, "tilemap.copied", id, Json{{"path", name}, {"source", map->path}, {"layers", copy->layers.size()}}, 0, "tilemap");
        return Json{{"map", name}, {"source", map->path}, {"layers", copy->layers.size()}, {"revision", copy->revision}};
    }
    if (op == "save") {
        // Write the map back as Tiled JSON, to its own file or another path in the project.
        if (!map->file && !p.contains("path")) return fail("no_file", "{} is a copy made at runtime with no file of its own; give a path to save it to", map->path);
        const std::string rel = opt<std::string>(p, "path", map->path);
        POCKET_TRY(full, inside_dir(options_.project_dir, rel));
        const std::string text = map->to_json().dump() + "\n";
        std::error_code ec;
        std::filesystem::create_directories(full.parent_path(), ec);
        POCKET_TRY_VOID(fs::write_text(full, text));
        world_->events().emit(clock_.tick, "tilemap.saved", id, Json{{"path", rel}, {"revision", map->revision}, {"bytes", text.size()}}, 0, "tilemap");
        return Json{{"path", full.string()}, {"bytes", text.size()}, {"revision", map->revision}, {"layers", map->layers.size()}};
    }
    return fail("unknown_command", "unknown tilemap command '{}'", op);
}

// Clips from other files onto a model, kept for the session so a reload of the model takes them again.
Result<Json> Session::add_animation_library(const std::string& model, const Json& files, bool root_only) {
    if (!assets_) return fail("no_assets", "no asset store");
    if (model.empty()) return fail("bad_args", "library needs mesh: the model the clips go onto");
    if (!files.is_array()) return fail("bad_args", "library needs files: [\"assets/anims/run.glb\", ...]");
    Json results = Json::array();
    for (const Json& f : files) {
        if (!f.is_string()) continue;
        POCKET_TRY(r, assets_->add_clips(model, f.get<std::string>(), root_only));
        results.push_back(r);
        auto& lib = anim_libraries_[model];
        if (std::find(lib.files.begin(), lib.files.end(), f.get<std::string>()) == lib.files.end()) lib.files.push_back(f.get<std::string>());
        lib.root_only = root_only;
    }
    return Json{{"mesh", model}, {"files", results}};
}

Result<Json> Session::animation_command(std::string_view op, const Json& p) {
    // Skeletal animation of glTF assets (docs/design/animation.md).
    auto& w = *world_;
    auto mesh_of = [&](world::EntityId id) -> Result<const assets::Mesh*> {
        const auto* mr = w.try_get<world::MeshRenderer>(id);
        if (!mr) return fail("no_mesh", "entity {} has no MeshRenderer", id);
        if (!assets_) return fail("no_assets", "no asset store");
        POCKET_TRY(m, assets_->mesh(mr->mesh));
        return m;
    };
    if (op == "library") {
        // Clips from other files onto a model (docs/design/animation.md, Clips from other files):
        // matched by joint name, kept for the session so a reload of the model takes them again.
        return add_animation_library(opt<std::string>(p, "mesh", ""), p.contains("files") ? p["files"] : Json(), opt<std::string>(p, "translations", "all") == "root");
    }
    if (op == "clips") {
        std::string path = opt<std::string>(p, "mesh", "");
        const assets::Mesh* mesh = nullptr;
        if (!path.empty()) {
            if (!assets_) return fail("no_assets", "no asset store");
            POCKET_TRY(m, assets_->mesh(path));
            mesh = m;
        } else {
            if (!p.contains("entity")) return fail("bad_args", "clips needs 'entity' or 'mesh'");
            world::EntityId id = resolve_entity(p["entity"]);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
            POCKET_TRY(m, mesh_of(id));
            mesh = m;
        }
        Json clips = Json::array();
        for (const auto& c : mesh->animations) clips.push_back(Json{{"name", c.name}, {"duration", c.duration}, {"channels", c.channels.size()}});
        Json skins = Json::array();
        for (const auto& s : mesh->skins) {
            Json joints = Json::array();
            for (int j : s.joints) joints.push_back(mesh->nodes[static_cast<std::size_t>(j)].name);
            skins.push_back(Json{{"name", s.name}, {"joints", joints}});
        }
        Json targets = Json::array();
        for (const auto& t : mesh->morph_targets) targets.push_back(t.name);
        return Json{{"mesh", mesh->path}, {"clips", clips}, {"skins", skins}, {"skinned", mesh->skinned()}, {"targets", targets}};
    }
    if (op == "param" || op == "trigger") {
        // A parameter of the entity's AnimationGraph (docs/design/animation.md, State machines).
        // `values` sets several parameters at once ({speed: 2, grounded: true}): all or none.
        const bool many = op == "param" && p.contains("values") && p["values"].is_object();
        if (!p.contains("entity") || (!many && !p.contains("name"))) return fail("bad_args", "animation.{} needs 'entity' and 'name'{}", op, op == "param" ? " (or 'values': {name: value})" : "");
        if (op == "param" && !many && !p.contains("value")) return fail("bad_args", "animation.param needs a 'value'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        const auto* current = w.try_get<world::AnimationGraph>(id);
        if (!current) return fail("no_graph", "{} has no AnimationGraph", w.path(id));
        world::AnimationGraph g = *current;
        auto number = [](const Json& v) { return v.is_boolean() ? (v.get<bool>() ? 1.0f : 0.0f) : v.is_number() ? v.get<float>() : 0.0f; };
        auto set_one = [&](const std::string& name, const Json* value) -> Result<void> {
            auto it = std::find_if(g.params.begin(), g.params.end(), [&](const world::AnimationParam& a) { return a.name == name; });
            if (it == g.params.end()) {
                std::string names;
                for (const auto& a : g.params) names += (names.empty() ? "" : ", ") + a.name;
                return fail("no_param", "{} has no parameter '{}' (it has: {})", w.path(id), name, names.empty() ? "none" : names);
            }
            if (!value) {
                it->value = 1;
                it->trigger = true;
            } else {
                it->value = number(*value);
            }
            return {};
        };
        if (many) {
            for (const auto& [name, value] : p["values"].items()) POCKET_TRY_VOID(set_one(name, &value));
        } else {
            POCKET_TRY_VOID(set_one(opt<std::string>(p, "name", ""), op == "trigger" ? nullptr : &p["value"]));
        }
        w.ecs().entity(id).set<world::AnimationGraph>(g);
        Json params = Json::object();
        for (const auto& a : g.params) params[a.name] = a.value;
        return Json{{"entity", id}, {"state", g.state}, {"params", params}};
    }
    if (op == "play" || op == "stop" || op == "pose" || op == "layer") {
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        POCKET_TRY(mesh, mesh_of(id));
        if (op == "pose") return animation_->describe_pose(w, id, *mesh);
        std::uint64_t cause = opt<std::uint64_t>(p, "cause", 0);
        Json patch = Json::object();
        if (op == "layer") {
            // A layer over the base clip: by index, or by the clip's name (an existing layer of
            // that clip is updated, else one is appended); remove takes it out again.
            const auto* current = w.try_get<world::Animator>(id);
            std::vector<world::AnimationLayer> layers = current ? current->layers : std::vector<world::AnimationLayer>{};
            const bool remove = opt<bool>(p, "remove", false);
            int index = opt<int>(p, "index", -1);
            const std::string clip = opt<std::string>(p, "clip", "");
            if (index < 0) {
                for (std::size_t i = 0; i < layers.size(); ++i) {
                    if (!clip.empty() && layers[i].clip == clip) { index = static_cast<int>(i); break; }
                }
                if (index < 0) {
                    if (remove) return fail("no_such_layer", "entity {} has no layer playing '{}'", id, clip);
                    index = static_cast<int>(layers.size());
                }
            }
            if (index > static_cast<int>(layers.size()) || (remove && index == static_cast<int>(layers.size()))) {
                return fail("no_such_layer", "entity {} has {} layers; there is no layer {}", id, layers.size(), index);
            }
            if (remove) {
                layers.erase(layers.begin() + index);
            } else {
                if (index == static_cast<int>(layers.size())) layers.emplace_back();
                world::AnimationLayer& L = layers[static_cast<std::size_t>(index)];
                from_json(p, L);  // only the layer's own fields are read
                if (L.clip.empty()) return fail("bad_args", "a layer needs a 'clip'");
                if (!mesh->clip(L.clip)) return fail("no_such_clip", "'{}' has no animation named '{}' (animation.clips lists them)", mesh->path, L.clip);
                for (const std::string& name : renderer::Animation::mask_names(L.mask)) {
                    bool found = false;
                    for (const auto& n : mesh->nodes) found = found || n.name == name;
                    if (!found) return fail("no_such_node", "'{}' has no node named '{}' (animation.clips lists the joints)", mesh->path, name);
                }
            }
            patch["layers"] = layers;
            POCKET_TRY_VOID(w.set(id, "Animator", patch, cause));
            return w.get(id, "Animator");
        }
        if (op == "play") {
            std::string clip = opt<std::string>(p, "clip", "");
            if (!clip.empty()) {
                if (!mesh->clip(clip)) return fail("no_such_clip", "'{}' has no animation named '{}' (animation.clips lists them)", mesh->path, clip);
                patch["clip"] = clip;
            } else if (!w.has(id, "Animator")) {
                if (mesh->animations.empty()) return fail("no_such_clip", "'{}' has no animations", mesh->path);
                patch["clip"] = mesh->animations.front().name;
            }
            patch["playing"] = true;
            patch["finished"] = false;
            if (opt<bool>(p, "restart", true)) patch["time"] = 0.0;
            for (const char* k : {"loop", "speed", "time"}) {
                if (p.contains(k)) patch[k] = p[k];
            }
            // A cross-fade from what plays now: the current clip becomes from_clip at its time.
            float fade = opt<float>(p, "fade", 0.0f);
            const auto* current = w.try_get<world::Animator>(id);
            if (fade > 0 && current && !current->clip.empty()) {
                patch["from_clip"] = current->clip;
                patch["from_time"] = current->time;
                patch["fade"] = fade;
                patch["fade_time"] = 0.0;
            } else {
                patch["from_clip"] = "";
                patch["fade"] = 0.0;
                patch["fade_time"] = 0.0;
            }
        } else {
            patch["playing"] = false;
            if (opt<bool>(p, "reset", false)) patch["time"] = 0.0;
            // The layers stop with the base clip.
            if (const auto* current = w.try_get<world::Animator>(id); current && !current->layers.empty()) {
                std::vector<world::AnimationLayer> layers = current->layers;
                for (world::AnimationLayer& L : layers) {
                    L.playing = false;
                    if (opt<bool>(p, "reset", false)) L.time = 0;
                }
                patch["layers"] = layers;
            }
        }
        POCKET_TRY_VOID(w.set(id, "Animator", patch, cause));
        return w.get(id, "Animator");
    }
    return fail("unknown_command", "unknown animation command '{}'", op);
}

Result<Json> Session::particles_command(std::string_view op, const Json& p) {
    if (op == "stats") return particles_->stats();
    if (op == "clear") {
        std::size_t n = particles_->alive();
        particles_->clear();
        return Json{{"cleared", n}};
    }
    if (op == "list") {
        // The live particles of one emitter: where they are and how they move.
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        // A few of them (10 unless asked: a hundred were 20 KB an agent then carried), and all of them
        // summed up: where they are, how fast they go, how old.
        const auto limit = static_cast<std::size_t>(std::clamp(opt<int>(p, "limit", 10), 0, 10000));
        Json arr = limit > 0 ? particles_->list(id, limit) : Json::array();
        std::size_t alive = 0;
        Json j{{"entity", id}};
        if (auto it = particles_->pools().find(id); it != particles_->pools().end() && !it->second.alive.empty()) {
            const auto& all = it->second.alive;
            alive = all.size();
            Vec3 lo = all.front().position, hi = lo;
            double speed = 0, age = 0;
            for (const renderer::Particle& q : all) {
                lo = Vec3{std::min(lo.x, q.position.x), std::min(lo.y, q.position.y), std::min(lo.z, q.position.z)};
                hi = Vec3{std::max(hi.x, q.position.x), std::max(hi.y, q.position.y), std::max(hi.z, q.position.z)};
                speed += length(q.velocity);
                age += q.age;
            }
            auto r3 = [](float v) { return std::round(v * 1000.0) / 1000.0; };
            j["bounds"] = Json{{"min", {{"x", r3(lo.x)}, {"y", r3(lo.y)}, {"z", r3(lo.z)}}}, {"max", {{"x", r3(hi.x)}, {"y", r3(hi.y)}, {"z", r3(hi.z)}}}};
            j["mean_speed"] = r3(static_cast<float>(speed / static_cast<double>(alive)));
            j["mean_age"] = r3(static_cast<float>(age / static_cast<double>(alive)));
        }
        j["alive"] = alive;
        j["particles"] = arr;
        return j;
    }
    if (op == "preset") {
        // A tuned ParticleEmitter by name (docs/design/particles.md, Presets): its fields written
        // onto the entity (added when it has none), `set` over them; the answer is the emitter.
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        const std::string name = opt<std::string>(p, "name", "");
        auto c = [](double r, double g, double b, double a) { return Json{{"r", r}, {"g", g}, {"b", b}, {"a", a}}; };
        auto v2 = [](double x, double y) { return Json{{"x", x}, {"y", y}}; };
        auto v3 = [](double x, double y, double z) { return Json{{"x", x}, {"y", y}, {"z", z}}; };
        static const char* kNames = "fire, smoke, sparks, explosion, rain, snow, dust, fireflies, magic";
        Json e;
        if (name == "fire") e = {{"emitting", true}, {"rate", 70}, {"max", 300}, {"lifetime", v2(0.45, 0.9)}, {"speed", v2(0.6, 1.4)}, {"direction", v3(0, 1, 0)}, {"spread", 12}, {"gravity", v3(0, 1.6, 0)}, {"drag", 0.6}, {"size", v2(0.55, 0.08)}, {"color", c(1, 0.78, 0.32, 1)}, {"color_end", c(0.85, 0.18, 0.04, 0)}, {"additive", true}, {"turbulence", 0.7}, {"turbulence_scale", 1.2}, {"area", v3(0.22, 0.02, 0.22)}, {"stretch", 0}};
        else if (name == "smoke") e = {{"emitting", true}, {"rate", 14}, {"max", 120}, {"lifetime", v2(2.5, 4.0)}, {"speed", v2(0.35, 0.8)}, {"direction", v3(0, 1, 0)}, {"spread", 18}, {"gravity", v3(0, 0.35, 0)}, {"drag", 0.35}, {"size", v2(0.45, 1.8)}, {"color", c(0.32, 0.32, 0.32, 0.55)}, {"color_end", c(0.55, 0.55, 0.55, 0)}, {"additive", false}, {"turbulence", 0.45}, {"turbulence_scale", 2.0}, {"area", v3(0.15, 0, 0.15)}, {"stretch", 0}};
        else if (name == "sparks") e = {{"emitting", false}, {"rate", 0}, {"max", 300}, {"lifetime", v2(0.2, 0.55)}, {"speed", v2(3, 7)}, {"direction", v3(0, 1, 0)}, {"spread", 65}, {"gravity", v3(0, -9, 0)}, {"drag", 0.2}, {"size", v2(0.05, 0.01)}, {"color", c(1, 0.82, 0.4, 1)}, {"color_end", c(1, 0.3, 0.08, 0)}, {"additive", true}, {"turbulence", 0}, {"stretch", 0.04}, {"area", v3(0, 0, 0)}};
        else if (name == "explosion") e = {{"emitting", false}, {"rate", 0}, {"max", 400}, {"lifetime", v2(0.3, 0.75)}, {"speed", v2(3, 9)}, {"direction", v3(0, 1, 0)}, {"spread", 180}, {"gravity", v3(0, -1.5, 0)}, {"drag", 2.5}, {"size", v2(1.2, 0.2)}, {"color", c(1, 0.95, 0.7, 1)}, {"color_end", c(1, 0.45, 0.1, 0)}, {"additive", true}, {"turbulence", 0.8}, {"turbulence_scale", 1.5}, {"stretch", 0}, {"area", v3(0.2, 0.2, 0.2)}};
        else if (name == "rain") e = {{"emitting", true}, {"rate", 900}, {"max", 6000}, {"lifetime", v2(0.9, 1.1)}, {"speed", v2(12, 14)}, {"direction", v3(0.05, -1, 0)}, {"spread", 2}, {"gravity", v3(0, -4, 0)}, {"drag", 0}, {"size", v2(0.025, 0.025)}, {"color", c(0.72, 0.78, 0.88, 0.45)}, {"color_end", c(0.72, 0.78, 0.88, 0.3)}, {"additive", false}, {"turbulence", 0}, {"stretch", 0.06}, {"area", v3(15, 0, 15)}, {"gpu", true}};
        else if (name == "snow") e = {{"emitting", true}, {"rate", 260}, {"max", 4000}, {"lifetime", v2(6, 9)}, {"speed", v2(0.3, 0.7)}, {"direction", v3(0, -1, 0)}, {"spread", 25}, {"gravity", v3(0, -0.5, 0)}, {"drag", 0.6}, {"size", v2(0.07, 0.06)}, {"color", c(1, 1, 1, 0.9)}, {"color_end", c(1, 1, 1, 0.7)}, {"additive", false}, {"turbulence", 0.5}, {"turbulence_scale", 3.0}, {"stretch", 0}, {"area", v3(15, 0, 15)}, {"gpu", true}};
        else if (name == "dust") e = {{"emitting", true}, {"rate", 10}, {"max", 200}, {"lifetime", v2(3, 6)}, {"speed", v2(0.03, 0.15)}, {"direction", v3(0, 1, 0)}, {"spread", 180}, {"gravity", v3(0, 0, 0)}, {"drag", 0.2}, {"size", v2(0.035, 0.035)}, {"color", c(0.95, 0.9, 0.75, 0.35)}, {"color_end", c(0.95, 0.9, 0.75, 0)}, {"additive", true}, {"turbulence", 0.25}, {"turbulence_scale", 2.5}, {"stretch", 0}, {"area", v3(3, 1.5, 3)}};
        else if (name == "fireflies") e = {{"emitting", true}, {"rate", 5}, {"max", 60}, {"lifetime", v2(3, 6)}, {"speed", v2(0.1, 0.35)}, {"direction", v3(0, 1, 0)}, {"spread", 180}, {"gravity", v3(0, 0, 0)}, {"drag", 0.1}, {"size", v2(0.09, 0.07)}, {"color", c(0.85, 1, 0.45, 1)}, {"color_end", c(0.6, 1, 0.3, 0)}, {"additive", true}, {"turbulence", 0.9}, {"turbulence_scale", 1.5}, {"stretch", 0}, {"area", v3(4, 0.8, 4)}};
        else if (name == "magic") e = {{"emitting", true}, {"rate", 45}, {"max", 200}, {"lifetime", v2(0.7, 1.3)}, {"speed", v2(0.4, 1.1)}, {"direction", v3(0, 1, 0)}, {"spread", 180}, {"gravity", v3(0, 0.7, 0)}, {"drag", 0.8}, {"size", v2(0.16, 0.0)}, {"color", c(0.65, 0.45, 1, 1)}, {"color_end", c(0.25, 0.85, 1, 0)}, {"additive", true}, {"turbulence", 1.1}, {"turbulence_scale", 1.0}, {"stretch", 0}, {"area", v3(0.3, 0.3, 0.3)}};
        else return fail("bad_args", "no preset '{}' ({})", name, kNames);
        if (p.contains("set")) {
            if (!p["set"].is_object()) return fail("bad_args", "'set' must be an object of ParticleEmitter fields");
            for (const auto& [k, v] : p["set"].items()) e[k] = v;
        }
        POCKET_TRY_VOID(world_->set(id, "ParticleEmitter", e, 0));
        return world_->get(id, "ParticleEmitter");
    }
    if (op == "burst") {
        if (!p.contains("entity")) return fail("bad_args", "missing 'entity'");
        world::EntityId id = resolve_entity(p["entity"]);
        if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
        int count = opt<int>(p, "count", 10);
        std::optional<Vec3> at;
        if (p.contains("at")) {
            if (!(p["at"].is_array() && p["at"].size() == 3) && !p["at"].is_object()) return fail("bad_args", "'at' must be a point [x, y, z] or {{x, y, z}}");
            at = vec3_of(p["at"], {0, 0, 0});
        }
        POCKET_TRY_VOID(particles_->burst(*world_, id, count, at ? &*at : nullptr, opt<float>(p, "speed", 1.0f)));
        std::size_t alive = 0;
        if (auto it = particles_->pools().find(id); it != particles_->pools().end()) alive = it->second.alive.size();
        return Json{{"entity", id}, {"count", count}, {"alive", alive}};
    }
    return fail("unknown_command", "unknown particles command '{}'", op);
}

// 2D rigid bodies' questions and pushes (docs/design/physics2d.md).
Result<Json> Session::physics2d_command(std::string_view op, const Json& p) {
    auto& w = *world_;
    auto v2 = [](const Json& j, Vec2 d) {
        if (j.is_array() && j.size() >= 2) return Vec2{j[0].get<float>(), j[1].get<float>()};
        if (j.is_object()) return Vec2{j.value("x", d.x), j.value("y", d.y)};
        return d;
    };
    const auto mask = static_cast<std::uint32_t>(opt<std::int64_t>(p, "mask", 0xFFFFFFFFll));
    if (op == "stats") return rigid2d_->describe();
    if (op == "raycast") {
        // From `from` to `to`, or along `direction` for `distance`.
        const Vec2 from = v2(p.value("from", Json()), Vec2{0, 0});
        Vec2 to = v2(p.value("to", Json()), from);
        if (!p.contains("to")) {
            Vec2 d = v2(p.value("direction", Json()), Vec2{0, -1});
            const float len = std::sqrt(d.x * d.x + d.y * d.y);
            if (len <= 0) return fail("bad_args", "direction must not be zero");
            const float dist = static_cast<float>(opt<double>(p, "distance", 100.0));
            to = Vec2{from.x + d.x / len * dist, from.y + d.y / len * dist};
        }
        const auto hit = rigid2d_->raycast(from, to, mask);
        if (!hit) return Json{{"hit", false}};
        const float len = std::sqrt((to.x - from.x) * (to.x - from.x) + (to.y - from.y) * (to.y - from.y));
        return Json{{"hit", true}, {"entity", hit->entity}, {"path", w.alive(hit->entity) ? w.path(hit->entity) : std::string()}, {"point", Json{{"x", hit->point.x}, {"y", hit->point.y}}}, {"normal", Json{{"x", hit->normal.x}, {"y", hit->normal.y}}}, {"distance", hit->fraction * len}};
    }
    if (op == "overlap") {
        // A box (half, angle) or a circle (radius) at `center`: the bodies whose shapes it touches.
        const Vec2 c = v2(p.value("center", Json()), Vec2{0, 0});
        const Vec2 half = v2(p.value("half", Json()), Vec2{0.5f, 0.5f});
        const float radius = static_cast<float>(opt<double>(p, "radius", 0.0));
        Json found = Json::array();
        for (world::EntityId id : rigid2d_->overlap(c, half, static_cast<float>(opt<double>(p, "angle", 0.0)), radius, mask))
            found.push_back(Json{{"entity", id}, {"path", w.alive(id) ? w.path(id) : std::string()}});
        return Json{{"entities", found}};
    }
    if (op == "impulse") {
        const world::EntityId id = resolve_entity(p.value("entity", Json()));
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p.value("entity", Json()).dump());
        std::optional<Vec2> at;
        if (p.contains("point")) at = v2(p["point"], Vec2{0, 0});
        POCKET_TRY(now, rigid2d_->impulse(id, v2(p.value("impulse", Json()), Vec2{0, 0}), at, static_cast<float>(opt<double>(p, "angular", 0.0))));
        // The new speed in the component at once (the next step sets the same in the body).
        if (const auto* rb = w.try_get<world::RigidBody2D>(id)) {
            world::RigidBody2D next = *rb;
            next.velocity = now.first;
            next.angular_velocity = now.second;
            next.awake = true;
            w.set_typed<world::RigidBody2D>(id, next);
        }
        return Json{{"velocity", Json{{"x", now.first.x}, {"y", now.first.y}}}, {"angular_velocity", now.second}};
    }
    return fail("unknown_command", "physics2d.{} is not a command (raycast, overlap, impulse, stats)", op);
}

Result<Json> Session::physics_command(std::string_view op, const Json& p) {
    if (op == "stats") {
        Json j = physics_->describe();
        if (physics2d_) j["tiles"] = physics2d_->describe();
        j["layers"] = physics_layers_;
        j["cloth"] = cloth_.sheets();
        return j;
    }
    if (op == "cloth") {
        // Where a sheet of cloth is now: its box, its lowest point, and the particles row by row when asked.
        const world::EntityId id = p.contains("entity") ? resolve_entity(p["entity"]) : 0;
        if (!id) return fail("no_such_entity", "physics.cloth needs an entity with a Cloth");
        const std::vector<Vec3> at = cloth_.particles(id);
        if (at.empty()) return fail("no_cloth", "{} has no Cloth the engine has stepped yet", world_->path(id));
        const auto* c = world_->try_get<world::Cloth>(id);
        Vec3 lo = at[0], hi = at[0], lowest = at[0];
        for (const Vec3& v : at) {
            lo = Vec3{std::min(lo.x, v.x), std::min(lo.y, v.y), std::min(lo.z, v.z)};
            hi = Vec3{std::max(hi.x, v.x), std::max(hi.y, v.y), std::max(hi.z, v.z)};
            if (v.y < lowest.y) lowest = v;
        }
        auto vec = [](Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; };
        const int across = c ? std::clamp(static_cast<int>(std::lround(c->segments.x)), 1, 48) + 1 : 0;
        Json j{{"entity", id}, {"path", world_->path(id)}, {"particles", at.size()}, {"across", across}, {"down", across ? static_cast<int>(at.size()) / across : 0},
               {"bounds", Json{{"min", vec(lo)}, {"max", vec(hi)}}}, {"lowest", vec(lowest)}, {"mesh", "cloth:" + std::to_string(id)}};
        if (opt<bool>(p, "points", false)) {
            Json pts = Json::array();
            for (const Vec3& v : at) pts.push_back(Json::array({v.x, v.y, v.z}));
            j["points"] = pts;
        }
        return j;
    }
    if (op == "layers") {
        Json bits = Json::object();
        for (std::size_t i = 0; i < physics_layers_.size(); ++i) bits[physics_layers_[i]] = 1u << i;
        return Json{{"names", physics_layers_}, {"bits", bits}};
    }
    if (op == "ignore") {
        // An exception for one pair: they never collide, until told otherwise or one is gone.
        if (!p.contains("a") || !p.contains("b")) return fail("bad_args", "'a' and 'b' are required (entities)");
        const world::EntityId a = resolve_entity(p["a"]), b = resolve_entity(p["b"]);
        if (!world_->alive(a) || !world_->alive(b)) return fail("no_such_entity", "no entity for {} or {}", p["a"].dump(), p["b"].dump());
        if (a == b) return fail("bad_args", "a pair needs two different entities");
        const bool ignore = opt<bool>(p, "ignore", true);
        physics_->ignore(a, b, ignore);
        return Json{{"a", world_->path(a)}, {"b", world_->path(b)}, {"ignored", ignore}, {"exceptions", physics_->ignored().size()}};
    }
    if (op == "ignored") {
        Json out = Json::array();
        for (const auto& [a, b] : physics_->ignored()) out.push_back(Json{{"a", world_->path(a)}, {"b", world_->path(b)}, {"a_id", a}, {"b_id", b}});
        return out;
    }
    // Queries take a layer mask: only shapes on a layer in it answer (all layers by default).
    auto mask_of = [&]() -> std::uint32_t {
        if (!p.contains("mask")) return 0xFFFFFFFFu;
        if (p["mask"].is_number()) return static_cast<std::uint32_t>(p["mask"].get<double>());
        std::uint32_t m = 0;
        if (p["mask"].is_array()) {
            for (const Json& n : p["mask"]) {
                if (n.is_number()) m |= static_cast<std::uint32_t>(n.get<double>());
                else if (n.is_string()) {
                    for (std::size_t i = 0; i < physics_layers_.size(); ++i) if (physics_layers_[i] == n.get<std::string>()) m |= 1u << i;
                }
            }
        }
        return m;
    };
    if (op == "raycast") {
        Vec3 origin = vec3_of(p.value("origin", Json(nullptr)), {0, 0, 0});
        Vec3 dir = vec3_of(p.value("direction", Json(nullptr)), {0, -1, 0});
        const bool include_triggers = opt<bool>(p, "include_triggers", false);
        const std::uint32_t mask = mask_of();
        auto hit = physics_->raycast(*world_, origin, dir, opt<float>(p, "max_distance", 1000.0f), [include_triggers, mask](world::EntityId, const world::RigidBody&, const world::Collider& col) { return (include_triggers || !col.is_trigger) && (col.layer & mask) != 0; });
        if (!hit) {
            if (hit.error().code == "no_hit") return nullptr;
            return fail(hit.error());
        }
        Json j;
        j["entity"] = hit->entity;
        j["path"] = world_->path(hit->entity);
        j["point"] = json_of(hit->point);
        j["normal"] = json_of(hit->normal);
        j["distance"] = hit->distance;
        return j;
    }
    if (op == "sweep") {
        Vec3 origin = vec3_of(p.value("origin", Json(nullptr)), {0, 0, 0});
        Vec3 dir = vec3_of(p.value("direction", Json(nullptr)), {0, -1, 0});
        const bool include_triggers = opt<bool>(p, "include_triggers", false);
        const std::uint32_t mask = mask_of();
        const float radius = opt<float>(p, "radius", 0.5f);
        auto hit = physics_->sweep(*world_, origin, dir, radius, opt<float>(p, "max_distance", 1000.0f), [include_triggers, mask](world::EntityId, const world::RigidBody&, const world::Collider& col) { return (include_triggers || !col.is_trigger) && (col.layer & mask) != 0; });
        if (!hit) {
            if (hit.error().code == "no_hit") return nullptr;
            return fail(hit.error());
        }
        Json j;
        j["entity"] = hit->entity;
        j["path"] = world_->path(hit->entity);
        j["point"] = json_of(hit->point);
        j["normal"] = json_of(hit->normal);
        j["distance"] = hit->distance;
        j["radius"] = radius;
        return j;
    }
    if (op == "overlap") {
        // A sphere by default; a box (size: half extents) or a capsule (radius and height, like a
        // Character's), turned by rotation, for a sword's arc or a body's reach.
        physics::Physics::Probe probe;
        probe.position = vec3_of(p.value("center", Json(nullptr)), {0, 0, 0});
        const std::string shape = opt<std::string>(p, "shape", "sphere");
        const float radius = opt<float>(p, "radius", 1.0f);
        if (shape == "sphere") { probe.shape = 1; probe.half = {radius, radius, radius}; }
        else if (shape == "box") { probe.shape = 0; probe.half = vec3_of(p.value("size", Json(nullptr)), {0.5f, 0.5f, 0.5f}); }
        else if (shape == "capsule") { probe.shape = 2; probe.half = {radius, std::max(opt<float>(p, "height", 2.0f) * 0.5f - radius, 0.0f), radius}; }
        else return fail("bad_args", "shape is sphere, box or capsule, not '{}'", shape);
        if (p.contains("rotation") && p["rotation"].is_object()) {
            const Json& q = p["rotation"];
            probe.rotation = Quat{q.value("x", 0.0f), q.value("y", 0.0f), q.value("z", 0.0f), q.value("w", 1.0f)};
        }
        const std::uint32_t mask = mask_of();
        auto ids = physics_->overlap(*world_, probe, [mask](world::EntityId, const world::RigidBody&, const world::Collider& col) { return (col.layer & mask) != 0; }, opt<bool>(p, "characters", true), mask);
        Json arr = Json::array();
        for (auto id : ids) arr.push_back(Json{{"id", id}, {"path", world_->path(id)}});
        return arr;
    }
    if (op == "contacts") {
        Json arr = Json::array();
        for (const auto& c : physics_->contacts()) {
            Json cj;
            cj["a"] = world_->path(c.a);
            cj["b"] = world_->path(c.b);
            cj["point"] = json_of(c.point);
            cj["normal"] = json_of(c.normal);
            cj["depth"] = c.depth;
            cj["trigger"] = c.trigger;
            arr.push_back(cj);
        }
        return arr;
    }
    if (op == "gravity") {
        if (p.contains("gravity")) physics_->settings().gravity = vec3_of(p["gravity"], physics_->settings().gravity);
        return json_of(physics_->settings().gravity);
    }
    if (op == "joints") {
        Json arr = Json::array();
        for (const auto& j : physics_->joints()) {
            Json jj;
            jj["entity"] = j.entity;
            jj["path"] = world_->path(j.entity);
            jj["target"] = j.target ? world_->path(j.target) : "";
            jj["kind"] = j.kind;
            jj["length"] = j.length;
            jj["current"] = j.current;
            jj["force"] = j.force;
            if (j.kind == 2) {
                jj["angle"] = j.angle;
                jj["speed"] = j.speed;
                jj["torque"] = j.torque;
                jj["at_limit"] = j.limit_state;
            } else if (j.kind == 3) {
                jj["translation"] = j.translation;
                jj["speed"] = j.speed;
                jj["motor_force"] = j.torque;
                jj["at_limit"] = j.limit_state;
            }
            arr.push_back(jj);
        }
        return arr;
    }
    return fail("unknown_command", "unknown physics command '{}'", op);
}

Status Session::ensure_drawn() {
    if (!frame_stale_ || !errors_.empty() || !renderer_) return {};
    renderer_->cut();
    frame_stale_ = false;
    return render_frame();
}

Result<Json> Session::render_command(std::string_view op, const Json& p) {
    if (op == "stats") return renderer_->describe();
    if (op == "pick") {
        auto x = static_cast<std::uint32_t>(opt<double>(p, "x", -1));
        auto y = static_cast<std::uint32_t>(opt<double>(p, "y", -1));
        POCKET_TRY(index, renderer_->pick(x, y));
        world::EntityId id = world_->from_index(static_cast<std::uint32_t>(index));
        Json j;
        j["id"] = id;
        if (id != 0) {
            j["path"] = world_->path(id);
            j["name"] = world_->name(id);
        } else if (index != 0) {
            j["stale"] = true;
        }
        return j;
    }
    if (op == "project") {
        // An entity's world position, or any world point {x, y, z}.
        Vec3 pos;
        if (p.contains("point") && p["point"].is_object()) {
            const Json& pt = p["point"];
            pos = Vec3{opt<float>(pt, "x", 0.0f), opt<float>(pt, "y", 0.0f), opt<float>(pt, "z", 0.0f)};
        } else {
            world::EntityId id = p.contains("entity") ? resolve_entity(p["entity"]) : 0;
            if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p.value("entity", Json(nullptr)).dump());
            const world::WorldTransform* wt = world_->try_get<world::WorldTransform>(id);
            if (!wt) return fail("no_transform", "entity {} has no WorldTransform", world_->path(id));
            pos = wt->position;
        }
        float x = 0, y = 0;
        bool visible = renderer_->project(pos, x, y);
        Json j;
        j["visible"] = visible;
        if (visible) {
            j["x"] = x;
            j["y"] = y;
            j["inside"] = x >= 0 && y >= 0 && x < static_cast<float>(device_->width()) && y < static_cast<float>(device_->height());
        }
        return j;
    }
    if (op == "unproject") {
        // The world ray under a pixel, and where it meets an axis plane: "xy" at z = at (2D scenes),
        // "xz" at y = at (a ground plane), "yz" at x = at.
        Vec3 origin, dir;
        const float px = opt<float>(p, "x", 0.0f), py = opt<float>(p, "y", 0.0f);
        if (!renderer_->unproject(px, py, origin, dir)) {
            // Nothing drawn yet (a script's first tick): the window's camera as the scene has it,
            // the active one of highest priority, over the whole window.
            world::EntityId cam_id = 0;
            world::Camera cam;
            world_->ecs().each([&](flecs::entity e, const world::Camera& c) {
                if (c.active && c.target.empty() && (cam_id == 0 || c.priority > cam.priority)) { cam_id = e.id(); cam = c; }
            });
            const auto* wt = cam_id ? world_->try_get<world::WorldTransform>(cam_id) : nullptr;
            const auto* t = cam_id ? world_->try_get<world::Transform>(cam_id) : nullptr;
            const float w = static_cast<float>(std::max(device_->width(), 1u)), h = static_cast<float>(std::max(device_->height(), 1u));
            if (!cam_id || (!wt && !t)) return fail("no_view", "no camera to unproject with: the scene has no active Camera");
            const Vec3 at = wt ? wt->position : t->position;
            const Quat turn = wt ? wt->rotation : t->rotation;
            const float nx = px / w * 2.0f - 1.0f, ny = 1.0f - py / h * 2.0f;
            const Vec3 right = turn.rotate({1, 0, 0}), up = turn.rotate({0, 1, 0}), fwd = turn.rotate({0, 0, -1});
            if (cam.orthographic) {
                origin = at + right * (nx * cam.ortho_size * w / h) + up * (ny * cam.ortho_size);
                dir = fwd;
            } else {
                const float k = std::tan(radians(cam.fov_degrees) * 0.5f);
                origin = at;
                dir = normalize(fwd + right * (nx * k * w / h) + up * (ny * k));
            }
        }
        Json j;
        j["origin"] = json_of(origin);
        j["direction"] = json_of(dir);
        const std::string plane = opt<std::string>(p, "plane", "xy");
        if (plane != "xy" && plane != "xz" && plane != "yz") return fail("bad_args", "plane must be xy, xz or yz");
        const float at = opt<float>(p, "at", 0.0f);
        const int axis = plane == "yz" ? 0 : plane == "xz" ? 1 : 2;
        const float o = axis == 0 ? origin.x : axis == 1 ? origin.y : origin.z;
        const float d = axis == 0 ? dir.x : axis == 1 ? dir.y : dir.z;
        j["plane"] = plane;
        j["at"] = at;
        j["hit"] = false;
        if (std::abs(d) > 1e-8f) {
            const float t = (at - o) / d;
            if (t >= 0) {
                j["hit"] = true;
                j["point"] = json_of(origin + dir * t);
                j["distance"] = t;
            }
        }
        return j;
    }
    if (op == "compare") {
        // The last frame against a reference PNG in the project (docs/design/rendering.md,
        // Comparing frames): a pixel differs when a channel is off by more than `threshold`
        // (16 of 255), and the frame matches when at most `tolerance` (0.01) of the pixels
        // differ. A missing reference is written from this frame and reported as `written`
        // (delete it to record again); `update: true` rewrites it; `diff` names a PNG to write,
        // when the frame does not match, with the differing pixels in red over the dimmed reference.
        const std::string rel = opt<std::string>(p, "path", "");
        POCKET_TRY(path, inside_dir(options_.project_dir, rel));
        POCKET_TRY(img, device_->capture());
        const double tolerance = std::clamp(opt<double>(p, "tolerance", 0.01), 0.0, 1.0);
        const int threshold = std::clamp(opt<int>(p, "threshold", 16), 0, 255);
        Json j;
        j["path"] = rel;
        j["width"] = img.width;
        j["height"] = img.height;
        j["tolerance"] = tolerance;
        j["threshold"] = threshold;
        std::error_code ec;
        if (!std::filesystem::exists(path, ec) || opt<bool>(p, "update", false)) {
            std::filesystem::create_directories(path.parent_path(), ec);
            POCKET_TRY_VOID(write_png(path, img));
            j["written"] = true;
            j["match"] = true;
            j["differing"] = 0;
            j["fraction"] = 0.0;
            return j;
        }
        POCKET_TRY(bytes, fs::read_bytes(path));
        POCKET_TRY(ref, assets::decode_image(std::string(bytes.begin(), bytes.end()), rel));
        j["written"] = false;
        j["ref_width"] = ref.width;
        j["ref_height"] = ref.height;
        if (ref.width != img.width || ref.height != img.height) {
            j["match"] = false;
            j["differing"] = static_cast<std::uint64_t>(img.width) * img.height;
            j["fraction"] = 1.0;
            j["reason"] = "size";
            return j;
        }
        const std::string diff_rel = opt<std::string>(p, "diff", "");
        rhi::Image diff;
        if (!diff_rel.empty()) {
            diff.width = img.width;
            diff.height = img.height;
            diff.rgba.resize(img.rgba.size());
        }
        std::uint64_t differing = 0;
        std::uint32_t x0 = img.width, y0 = img.height, x1 = 0, y1 = 0;
        for (std::uint32_t y = 0; y < img.height; ++y) {
            for (std::uint32_t x = 0; x < img.width; ++x) {
                const std::size_t i = (static_cast<std::size_t>(y) * img.width + x) * 4;
                bool off = false;
                for (std::size_t c = 0; c < 4 && !off; ++c) off = std::abs(static_cast<int>(img.rgba[i + c]) - static_cast<int>(ref.rgba[i + c])) > threshold;
                if (off) {
                    ++differing;
                    x0 = std::min(x0, x); y0 = std::min(y0, y); x1 = std::max(x1, x); y1 = std::max(y1, y);
                }
                if (!diff_rel.empty()) {
                    if (off) { diff.rgba[i] = 255; diff.rgba[i + 1] = 0; diff.rgba[i + 2] = 0; }
                    else { diff.rgba[i] = ref.rgba[i] / 3; diff.rgba[i + 1] = ref.rgba[i + 1] / 3; diff.rgba[i + 2] = ref.rgba[i + 2] / 3; }
                    diff.rgba[i + 3] = 255;
                }
            }
        }
        const std::uint64_t total = static_cast<std::uint64_t>(img.width) * img.height;
        const double fraction = total > 0 ? static_cast<double>(differing) / static_cast<double>(total) : 0.0;
        j["differing"] = differing;
        j["fraction"] = fraction;
        j["match"] = fraction <= tolerance;
        if (differing > 0) j["bounds"] = Json{{"x0", x0}, {"y0", y0}, {"x1", x1 + 1}, {"y1", y1 + 1}};
        if (!diff_rel.empty() && fraction > tolerance) {   // the picture of what went wrong, only when something did
            POCKET_TRY(diff_path, inside_dir(options_.project_dir, diff_rel));
            std::filesystem::create_directories(diff_path.parent_path(), ec);
            POCKET_TRY_VOID(write_png(diff_path, diff));
            j["diff"] = diff_rel;
        }
        return j;
    }
    if (op == "ids" || op == "visible") {
        POCKET_TRY(img, renderer_->read_ids());
        struct Span { std::uint64_t pixels = 0; std::uint32_t x0 = ~0u, y0 = ~0u, x1 = 0, y1 = 0; };
        std::map<std::uint32_t, Span> spans;
        for (std::uint32_t y = 0; y < img.height; ++y) {
            for (std::uint32_t x = 0; x < img.width; ++x) {
                Span& sp = spans[img.ids[static_cast<std::size_t>(y) * img.width + x]];
                sp.pixels++;
                sp.x0 = std::min(sp.x0, x);
                sp.y0 = std::min(sp.y0, y);
                sp.x1 = std::max(sp.x1, x);
                sp.y1 = std::max(sp.y1, y);
            }
        }
        const double total = static_cast<double>(img.width) * static_cast<double>(img.height);
        std::vector<Json> entities;
        for (auto& [index, sp] : spans) {
            if (op == "visible" && index == 0) continue;  // the background is not an entity
            Json e;
            world::EntityId id = world_->from_index(index);
            e["id"] = id;
            e["pixels"] = sp.pixels;
            e["coverage"] = total > 0 ? static_cast<double>(sp.pixels) / total : 0.0;
            e["bounds"] = Json{{"x", sp.x0}, {"y", sp.y0}, {"width", sp.x1 - sp.x0 + 1}, {"height", sp.y1 - sp.y0 + 1}};
            e["center"] = Json{{"x", img.width ? (sp.x0 + sp.x1 + 1) * 0.5 / img.width : 0.0}, {"y", img.height ? (sp.y0 + sp.y1 + 1) * 0.5 / img.height : 0.0}};
            if (id != 0) {
                e["path"] = world_->path(id);
                e["name"] = world_->name(id);
            } else if (index != 0) {
                e["stale"] = true;
            }
            entities.push_back(e);
        }
        if (op == "visible") {
            std::stable_sort(entities.begin(), entities.end(), [](const Json& a, const Json& b) { return a["pixels"].get<std::uint64_t>() > b["pixels"].get<std::uint64_t>(); });
            auto limit = static_cast<std::size_t>(std::max(opt<int>(p, "limit", 50), 0));
            if (entities.size() > limit) entities.resize(limit);
        }
        Json j;
        j["width"] = img.width;
        j["height"] = img.height;
        j["visible"] = Json(entities);
        j["count"] = spans.size() - (spans.contains(0) ? 1 : 0);
        const std::string path = op == "ids" ? opt<std::string>(p, "path", "") : "";
        if (!path.empty()) {
            POCKET_TRY(out, output_path(options_.project_dir, path));
            POCKET_TRY_VOID(write_png(out, ids_to_image(img)));
            j["path"] = out.string();
        }
        return j;
    }
    if (op == "debug") {
        if (p.contains("colliders") && p["colliders"].is_boolean()) debug_flags_.colliders = p["colliders"].get<bool>();
        if (p.contains("paths") && p["paths"].is_boolean()) debug_flags_.paths = p["paths"].get<bool>();
        if (p.contains("joints") && p["joints"].is_boolean()) debug_flags_.joints = p["joints"].get<bool>();
        if (p.contains("lights") && p["lights"].is_boolean()) debug_flags_.lights = p["lights"].get<bool>();
        if (p.contains("nav") && p["nav"].is_boolean()) debug_flags_.nav = p["nav"].get<bool>();
        if (p.contains("bounds") && p["bounds"].is_boolean()) debug_flags_.bounds = p["bounds"].get<bool>();
        if (p.contains("axes") && p["axes"].is_boolean()) debug_flags_.axes = p["axes"].get<bool>();
        if (p.contains("all") && p["all"].is_boolean()) debug_flags_.colliders = debug_flags_.joints = debug_flags_.bounds = debug_flags_.axes = debug_flags_.nav = debug_flags_.lights = debug_flags_.paths = p["all"].get<bool>();
        return Json{{"colliders", debug_flags_.colliders}, {"joints", debug_flags_.joints}, {"bounds", debug_flags_.bounds}, {"axes", debug_flags_.axes}, {"nav", debug_flags_.nav}, {"lights", debug_flags_.lights}, {"paths", debug_flags_.paths}, {"lines", renderer_->stats().debug_lines}};
    }
    if (op == "msaa") {
        // 1 or 4 samples per pixel; anything above one means four.
        if (p.contains("samples")) renderer_->set_msaa(opt<int>(p, "samples", 1));
        return Json{{"msaa", renderer_->msaa()}};
    }
    if (op == "shadows") {
        renderer::ShadowSettings s = renderer_->shadows();
        if (p.contains("enabled") && p["enabled"].is_boolean()) s.enabled = p["enabled"].get<bool>();
        if (p.contains("strength") && p["strength"].is_number()) s.strength = std::clamp(p["strength"].get<float>(), 0.0f, 1.0f);
        if (p.contains("bias") && p["bias"].is_number()) s.bias = std::max(0.0f, p["bias"].get<float>());
        if (p.contains("cascades") && p["cascades"].is_number()) s.cascades = p["cascades"].get<int>();
        if (p.contains("distance") && p["distance"].is_number()) s.distance = p["distance"].get<float>();
        if (p.contains("softness") && p["softness"].is_number()) s.softness = p["softness"].get<float>();
        if (p.contains("contact") && p["contact"].is_boolean()) s.contact = p["contact"].get<bool>();
        if (p.contains("contact_length") && p["contact_length"].is_number()) s.contact_length = p["contact_length"].get<float>();
        renderer_->set_shadows(s);
        s = renderer_->shadows();
        return Json{{"enabled", s.enabled}, {"strength", s.strength}, {"bias", s.bias}, {"cascades", s.cascades}, {"distance", s.distance}, {"softness", s.softness}, {"contact", s.contact}, {"contact_length", s.contact_length}};
    }
    if (op == "bloom") {
        renderer::BloomSettings b = renderer_->bloom();
        if (p.contains("enabled") && p["enabled"].is_boolean()) b.enabled = p["enabled"].get<bool>();
        if (p.contains("threshold") && p["threshold"].is_number()) b.threshold = p["threshold"].get<float>();
        if (p.contains("strength") && p["strength"].is_number()) b.strength = p["strength"].get<float>();
        if (p.contains("radius") && p["radius"].is_number()) b.radius = p["radius"].get<float>();
        renderer_->set_bloom(b);
        b = renderer_->bloom();
        return Json{{"enabled", b.enabled}, {"threshold", b.threshold}, {"strength", b.strength}, {"radius", b.radius}};
    }
    if (op == "ambient") {
        // The flat light from all around where there is no Sky: dark for a dungeon between its torches.
        renderer::AmbientSettings a = renderer_->ambient();
        if (p.is_object() && p.contains("color") && !read_tint(p["color"], a.color)) return fail("bad_args", "ambient color: {{r, g, b}}, [r, g, b] or \"#rrggbb\"");
        a.intensity = opt<float>(p, "intensity", a.intensity);
        renderer_->set_ambient(a);
        a = renderer_->ambient();
        return Json{{"color", Json{{"r", a.color.r}, {"g", a.color.g}, {"b", a.color.b}}}, {"intensity", a.intensity}};
    }
    if (op == "grade") {
        renderer::GradeSettings g = renderer_->grade();
        g.enabled = opt<bool>(p, "enabled", g.enabled);
        if (p.is_object() && p.contains("tint") && !read_tint(p["tint"], g.tint)) return fail("bad_args", "grade tint: {{r, g, b}}, [r, g, b] or \"#rrggbb\"");
        read_grade(g, p);
        renderer_->set_grade(g);
        return grade_json(renderer_->grade());
    }
    if (op == "probes") {
        // Reflection probes in use, and a request to capture every one again (a room rearranged).
        if (opt<bool>(p, "refresh", false)) renderer_->refresh_probes();
        return renderer_->probes();
    }
    if (op == "ssr") {
        // Screen-space reflections: glossy surfaces reflect what is on screen.
        renderer::SsrSettings r = renderer_->ssr();
        r.enabled = opt<bool>(p, "enabled", r.enabled);
        r.max_distance = opt<float>(p, "max_distance", r.max_distance);
        r.max_roughness = opt<float>(p, "max_roughness", r.max_roughness);
        r.steps = opt<int>(p, "steps", r.steps);
        r.thickness = opt<float>(p, "thickness", r.thickness);
        r.intensity = opt<float>(p, "intensity", r.intensity);
        renderer_->set_ssr(r);
        r = renderer_->ssr();
        return Json{{"enabled", r.enabled}, {"max_distance", r.max_distance}, {"max_roughness", r.max_roughness}, {"steps", r.steps}, {"thickness", r.thickness}, {"intensity", r.intensity}};
    }
    if (op == "ssgi") {
        // Screen-space global illumination: light bounced off what is on screen onto what is near it.
        renderer::SsgiSettings g = renderer_->ssgi();
        g.enabled = opt<bool>(p, "enabled", g.enabled);
        g.distance = opt<float>(p, "distance", g.distance);
        g.rays = opt<int>(p, "rays", g.rays);
        g.steps = opt<int>(p, "steps", g.steps);
        g.thickness = opt<float>(p, "thickness", g.thickness);
        g.intensity = opt<float>(p, "intensity", g.intensity);
        renderer_->set_ssgi(g);
        g = renderer_->ssgi();
        return Json{{"enabled", g.enabled}, {"distance", g.distance}, {"rays", g.rays}, {"steps", g.steps}, {"thickness", g.thickness}, {"intensity", g.intensity}};
    }
    if (op == "toon") {
        // The cel look (docs/design/rendering.md, Toon): lights in flat bands, outlines round each entity.
        renderer::ToonSettings t = renderer_->toon();
        t.enabled = opt<bool>(p, "enabled", p.empty() ? t.enabled : true);
        t.bands = opt<int>(p, "bands", t.bands);
        t.softness = opt<float>(p, "softness", t.softness);
        t.outline = opt<float>(p, "outline", t.outline);
        t.opacity = opt<float>(p, "opacity", t.opacity);
        if (p.contains("outline_color")) {
            const Json& c = p["outline_color"];
            if (c.is_string()) {
                std::string s = c.get<std::string>();
                if (s.starts_with("#")) s = s.substr(1);
                if (s.size() == 3) s = std::string{s[0], s[0], s[1], s[1], s[2], s[2]};
                if (s.size() != 6 || !std::all_of(s.begin(), s.end(), [](unsigned char ch) { return std::isxdigit(ch) != 0; })) return fail("bad_args", "outline_color is \"#rrggbb\" or {{r, g, b}} (0..1), not {}", c.dump());
                for (int k = 0; k < 3; ++k) t.outline_color[k] = static_cast<float>(std::stoi(s.substr(static_cast<std::size_t>(k) * 2, 2), nullptr, 16)) / 255.0f;
            } else if (c.is_object()) {
                t.outline_color[0] = c.value("r", t.outline_color[0]);
                t.outline_color[1] = c.value("g", t.outline_color[1]);
                t.outline_color[2] = c.value("b", t.outline_color[2]);
            } else {
                return fail("bad_args", "outline_color is \"#rrggbb\" or {{r, g, b}} (0..1), not {}", c.dump());
            }
        }
        renderer_->set_toon(t);
        t = renderer_->toon();
        return Json{{"enabled", t.enabled}, {"bands", t.bands}, {"softness", t.softness}, {"outline", t.outline}, {"outline_color", Json{{"r", t.outline_color[0]}, {"g", t.outline_color[1]}, {"b", t.outline_color[2]}}}, {"opacity", t.opacity}};
    }
    if (op == "colorblind" || op == "colour_vision") {
        // Colour vision: the frame as a dichromat sees it (simulate), or corrected for them.
        renderer::ColourVisionSettings c = renderer_->colour_vision();
        static const std::array<std::string_view, 4> modes = {"off", "protanopia", "deuteranopia", "tritanopia"};
        if (p.contains("mode")) {
            const Json& m = p["mode"];
            if (m.is_number_integer()) {
                c.mode = m.get<int>();
            } else if (m.is_string()) {
                const auto it = std::find(modes.begin(), modes.end(), m.get<std::string>());
                if (it == modes.end()) return fail("bad_args", "mode is off, protanopia (red), deuteranopia (green) or tritanopia (blue), not {}", m.dump());
                c.mode = static_cast<int>(it - modes.begin());
            }
        }
        c.simulate = opt<bool>(p, "simulate", c.simulate);
        c.strength = opt<float>(p, "strength", c.strength);
        renderer_->set_colour_vision(c);
        c = renderer_->colour_vision();
        return Json{{"mode", modes[static_cast<std::size_t>(c.mode)]}, {"simulate", c.simulate}, {"strength", c.strength}};
    }
    if (op == "scale") {
        // Render scale: the window's view drawn at a fraction of its pixels and stretched up;
        // dynamic moves the fraction to keep the GPU's frame under target_ms.
        renderer::RenderScaleSettings s = renderer_->render_scale();
        s.scale = opt<float>(p, "scale", s.scale);
        s.dynamic = opt<bool>(p, "dynamic", s.dynamic);
        s.target_ms = opt<float>(p, "target_ms", s.target_ms);
        s.least = opt<float>(p, "least", s.least);
        s.sharpen = opt<float>(p, "sharpen", s.sharpen);
        s.pixelated = opt<bool>(p, "pixelated", s.pixelated);
        renderer_->set_render_scale(s);
        s = renderer_->render_scale();
        const renderer::RenderStats& st = renderer_->stats();
        return Json{{"scale", s.scale}, {"dynamic", s.dynamic}, {"target_ms", s.target_ms}, {"least", s.least}, {"sharpen", s.sharpen}, {"pixelated", s.pixelated},
                    {"drawn", Json{{"scale", st.render_scale}, {"width", st.render_width}, {"height", st.render_height}}}};
    }
    if (op == "dof") {
        // Depth of field: a lens focused at `focus`, blurring what is nearer or farther.
        renderer::DofSettings d = renderer_->dof();
        d.enabled = opt<bool>(p, "enabled", d.enabled);
        d.focus = opt<float>(p, "focus", d.focus);
        d.aperture = opt<float>(p, "aperture", d.aperture);
        d.max_blur = opt<float>(p, "max_blur", d.max_blur);
        renderer_->set_dof(d);
        d = renderer_->dof();
        return Json{{"enabled", d.enabled}, {"focus", d.focus}, {"aperture", d.aperture}, {"max_blur", d.max_blur}};
    }
    if (op == "motion_blur") {
        renderer::MotionBlurSettings m = renderer_->motion_blur();
        m.enabled = opt<bool>(p, "enabled", m.enabled);
        m.strength = opt<float>(p, "strength", m.strength);
        m.samples = opt<int>(p, "samples", m.samples);
        renderer_->set_motion_blur(m);
        m = renderer_->motion_blur();
        return Json{{"enabled", m.enabled}, {"strength", m.strength}, {"samples", m.samples}};
    }
    if (op == "oit") {
        // Order-independent transparency: translucent meshes blended without sorting.
        renderer_->set_oit(opt<bool>(p, "enabled", renderer_->oit()));
        return Json{{"enabled", renderer_->oit()}};
    }
    if (op == "taa") {
        // Temporal anti-aliasing: the view jittered inside the pixel, frames blended through motion.
        renderer::TaaSettings t = renderer_->taa();
        t.enabled = opt<bool>(p, "enabled", t.enabled);
        t.feedback = opt<float>(p, "feedback", t.feedback);
        renderer_->set_taa(t);
        t = renderer_->taa();
        return Json{{"enabled", t.enabled}, {"feedback", t.feedback}};
    }
    if (op == "ao") {
        renderer::AoSettings a = renderer_->ao();
        a.enabled = opt<bool>(p, "enabled", a.enabled);
        a.radius = opt<float>(p, "radius", a.radius);
        a.intensity = opt<float>(p, "intensity", a.intensity);
        a.samples = opt<int>(p, "samples", a.samples);
        renderer_->set_ao(a);
        a = renderer_->ao();
        return Json{{"enabled", a.enabled}, {"radius", a.radius}, {"intensity", a.intensity}, {"samples", a.samples}};
    }
    if (op == "tonemap") {
        // How the HDR scene becomes the frame: exposure (fixed or metered), the operator. The
        // answer carries what the meter settled on after the last frame when it is on.
        renderer::TonemapSettings t = renderer_->tonemap();
        std::string why;
        if (!read_tonemap(t, p, why)) return fail("bad_args", "tonemap: {}", why);
        renderer_->set_tonemap(t);
        Json j = tonemap_json(renderer_->tonemap());
        if (renderer_->tonemap().auto_exposure) {
            POCKET_TRY(m, renderer_->metering());
            if (m.valid) j["metered"] = Json{{"exposure_ev", m.exposure_ev}, {"average_ev", m.average_ev}};
        }
        return j;
    }
    if (op == "viewport") {
        // Points in, points out; the renderer works in pixels.
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        if (p.contains("w") || p.contains("width")) {
            renderer::Viewport v;
            v.x = static_cast<std::int32_t>(std::lround(opt<double>(p, "x", 0) * scale));
            v.y = static_cast<std::int32_t>(std::lround(opt<double>(p, "y", 0) * scale));
            v.w = static_cast<std::uint32_t>(std::max(0.0, std::lround(opt<double>(p, "w", opt<double>(p, "width", 0)) * scale) * 1.0));
            v.h = static_cast<std::uint32_t>(std::max(0.0, std::lround(opt<double>(p, "h", opt<double>(p, "height", 0)) * scale) * 1.0));
            renderer_->set_viewport(v);
        } else if (opt<bool>(p, "reset", false)) {
            renderer_->set_viewport(renderer::Viewport{});
        }
        renderer::Viewport v = renderer_->viewport();
        Json j;
        j["x"] = v.x / scale;
        j["y"] = v.y / scale;
        j["w"] = v.w / scale;
        j["h"] = v.h / scale;
        j["pixels"] = Json{{"x", v.x}, {"y", v.y}, {"w", v.w}, {"h", v.h}};
        j["full"] = v.w == 0 || v.h == 0;
        return j;
    }
    return fail("unknown_command", "unknown render command '{}'", op);
}

Result<Json> Session::assets_command(std::string_view op, const Json& p) {
    if (op == "list") return assets_->list();
    if (op == "describe") {
        // An image in characters too: ascii true (32 across) or a width.
        int ascii = 0;
        if (p.contains("ascii")) ascii = p["ascii"].is_boolean() ? (p["ascii"].get<bool>() ? 32 : 0) : opt<int>(p, "ascii", 0);
        return assets_->describe(opt<std::string>(p, "path", ""), ascii);
    }
    if (op == "stats") {
        Json j = assets_->stats();
        j["render"] = renderer_->describe().value("assets", Json::object());
        return j;
    }
    if (op == "reload") {
        // Forget decoded data (one path or everything); the renderer re-uploads on the next frame.
        std::string path = opt<std::string>(p, "path", "");
        if (path.empty()) assets_->invalidate_all();
        else assets_->invalidate(path);
        if (audio_) audio_->forget(path);   // sounds read again on their next play (an edited recipe or score)
        renderer_->drop_asset_cache();
        renderer_->forget_sprite_materials();   // read and compiled again when next drawn
        sprite_material_errors_.clear();
        // Meshes made by code were let go with everything else: made again.
        if (path.empty())
            for (const auto& [mname, spec] : Json(made_meshes_).items()) if (auto r = make_mesh(mname, spec); !r) log::warn("runtime", "made mesh {}: {}", mname, r.error().message);
        // Models read again lost the clips added from other files: added again.
        for (const auto& [model, lib] : anim_libraries_)
            for (const std::string& f : lib.files) if (auto r = assets_->add_clips(model, f, lib.root_only); !r) log::warn("runtime", "animation library {} on {}: {}", f, model, r.error().message);
        if (physics_) physics_->drop_mesh_cache();
        // Terrains are made again: their heightmaps may have changed, and their meshes were forgotten.
        for (auto& [id, st] : terrains_) { st.shape_key.clear(); st.look_key.clear(); }
        update_terrains();
        // Live model instances whose file changed are made again in place.
        Json relinked = relink_models();
        Json j{{"ok", true}, {"version", assets_->version()}};
        if (!relinked.empty()) j["relinked"] = relinked;
        return j;
    }
    if (op == "preview") {
        // A model on its own, framed from three quarters above under a sky and a sun, drawn off
        // screen by a renderer of its own (the scene's frames, cameras and history untouched):
        // written to `out` (project-relative, or absolute), and as base64 PNG with `image`.
        const std::string path = opt<std::string>(p, "path", "");
        if (path.empty()) return fail("bad_args", "preview needs a project-relative model path");
        const auto size = static_cast<std::uint32_t>(std::clamp(opt<int>(p, "size", 256), 16, 1024));
        POCKET_TRY(mesh, assets_->mesh(path));
        if (!preview_renderer_) {
            POCKET_TRY(pr, renderer::Renderer::create(*device_));
            preview_renderer_ = std::move(pr);
            preview_renderer_->set_assets(assets_.get());
            renderer::TonemapSettings t = preview_renderer_->tonemap();
            t.op = renderer::Tonemap::Agx;
            preview_renderer_->set_tonemap(t);
        }
        const Vec3 lo = mesh->aabb_min, hi = mesh->aabb_max;
        const Vec3 center = (lo + hi) * 0.5f;
        const float radius = std::max(length(hi - lo) * 0.5f, 1e-3f);
        constexpr float kFov = 35.0f;
        const float dist = radius / std::sin(radians(kFov * 0.5f)) * 1.05f;
        const float yaw = radians(35.0f), pitch = radians(-25.0f);
        const Quat look = Quat::from_axis_angle({0, 1, 0}, yaw) * Quat::from_axis_angle({1, 0, 0}, pitch);
        const Vec3 eye = center - look.rotate({0, 0, -1}) * dist;
        world::World pw;
        auto put = [&](const char* name, const Json& comps) -> Status {
            auto r = pw.spawn(name, 0, comps);
            if (!r) return std::unexpected(r.error());
            return {};
        };
        POCKET_TRY_VOID(put("Model", Json{{"Transform", Json::object()}, {"MeshRenderer", Json{{"mesh", path}}}}));
        POCKET_TRY_VOID(put("Camera", Json{{"Transform", Json{{"position", Json{{"x", eye.x}, {"y", eye.y}, {"z", eye.z}}}, {"rotation", Json{{"x", look.x}, {"y", look.y}, {"z", look.z}, {"w", look.w}}}}},
                                           {"Camera", Json{{"fov_degrees", kFov}, {"near", std::max(dist - radius * 2.0f, 0.01f)}, {"far", dist + radius * 4.0f}}}}));
        const Quat sun = Quat::from_axis_angle({0, 1, 0}, radians(-40.0f)) * Quat::from_axis_angle({1, 0, 0}, radians(-50.0f));
        POCKET_TRY_VOID(put("Sun", Json{{"Transform", Json{{"rotation", Json{{"x", sun.x}, {"y", sun.y}, {"z", sun.z}, {"w", sun.w}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2.0}}}}));
        POCKET_TRY_VOID(put("Sky", Json{{"Sky", Json{{"mode", 1}, {"intensity", 0.9}}}}));
        pw.update_transforms();
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.preview");
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_CopySrc;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {size, size, 1};
        td.format = device_->color_format();
        td.mipLevelCount = 1;
        td.sampleCount = 1;
        WGPUTexture color = wgpuDeviceCreateTexture(device_->device(), &td);
        td.label = rhi::str("pocket.preview.depth");
        td.usage = WGPUTextureUsage_RenderAttachment;
        td.format = device_->depth_format();
        WGPUTexture depth = wgpuDeviceCreateTexture(device_->device(), &td);
        if (!color || !depth) {
            if (color) wgpuTextureRelease(color);
            if (depth) wgpuTextureRelease(depth);
            return fail("gpu_texture_failed", "cannot create the preview targets");
        }
        WGPUTextureView color_view = wgpuTextureCreateView(color, nullptr);
        WGPUTextureView depth_view = wgpuTextureCreateView(depth, nullptr);
        rhi::Frame f;
        f.encoder = wgpuDeviceCreateCommandEncoder(device_->device(), nullptr);
        f.color = color_view;
        f.color_texture = color;
        f.depth = depth_view;
        f.width = size;
        f.height = size;
        const Status drawn = preview_renderer_->render(f, pw, rhi::Color{0.16f, 0.17f, 0.2f, 1.0f}, nullptr, nullptr, nullptr);
        WGPUCommandBuffer cb = wgpuCommandEncoderFinish(f.encoder, nullptr);
        wgpuQueueSubmit(device_->queue(), 1, &cb);
        wgpuCommandBufferRelease(cb);
        wgpuCommandEncoderRelease(f.encoder);
        auto img = drawn ? device_->read_texture(color, size, size) : Result<rhi::Image>(std::unexpected(drawn.error()));
        wgpuTextureViewRelease(color_view);
        wgpuTextureViewRelease(depth_view);
        wgpuTextureRelease(color);
        wgpuTextureRelease(depth);
        if (!img) return std::unexpected(img.error());
        Json j{{"path", path}, {"width", size}, {"height", size}, {"vertices", mesh->vertices.size()}, {"triangles", mesh->indices.size() / 3}};
        const std::string out = opt<std::string>(p, "out", "");
        if (!out.empty()) {
            std::filesystem::path to = out;
            if (to.is_relative()) to = options_.project_dir / to;
            POCKET_TRY_VOID(write_png(to, *img));
            j["out"] = to.string();
        }
        if (opt<bool>(p, "image", false)) j["png"] = base64(png_bytes(*img));
        return j;
    }
    if (op == "import") {
        // Read a model now: an OBJ or STL natively, a Blender-read format converted through Blender
        // (again with force); the answer describes what came out. The renderer uploads it anew.
        const std::string path = opt<std::string>(p, "path", "");
        if (path.empty()) return fail("bad_args", "import needs a project-relative path");
        POCKET_TRY(j, assets_->import(path, opt<bool>(p, "force", false)));
        renderer_->drop_asset_cache();
        if (physics_) physics_->drop_mesh_cache();
        if (Json relinked = relink_models(); !relinked.empty()) j["relinked"] = relinked;
        j["blender_found"] = !assets_->blender().empty();
        return j;
    }
    return fail("unknown_command", "unknown assets command '{}'", op);
}

bool Session::advance_rumble() {
    bool any = false;
    for (auto it = rumble_.begin(); it != rumble_.end();) {
        RumblePattern& r = it->second;
        if (clock_.tick < r.next_tick) { ++it; continue; }
        if (r.next >= r.steps.size()) {
            if (--r.repeat <= 0) { it = rumble_.erase(it); continue; }
            r.next = 0;
        }
        const RumbleStep& step = r.steps[r.next];
        const bool rumbled = platform_ && platform_->rumble(it->first, step.low, step.high, step.ms);
        any = any || rumbled;
        // Seen by agents and tests whether or not a motor answered (headless runs have none).
        world_->events().emit(clock_.tick, "input.rumble", 0, Json{{"pad", it->first}, {"low", step.low}, {"high", step.high}, {"ms", step.ms}, {"step", r.next}, {"steps", r.steps.size()}, {"rumbled", rumbled}}, 0, "input");
        r.next_tick = clock_.tick + std::max<std::int64_t>(1, static_cast<std::int64_t>(std::ceil(step.ms / 1000.0 / clock_.tick_seconds - 1e-9)));
        ++r.next;
        ++it;
    }
    return any;
}

// One hit of a hitbox on a Health: teams, invulnerability, damage, knockback, events. Answers whether it landed.
bool Session::apply_hit(world::EntityId box, world::EntityId target, std::uint64_t cause, std::vector<world::EntityId>& spent) {
    const auto* hb_now = world_->try_get<world::Hitbox>(box);
    if (!hb_now || !world_->try_get<world::Health>(target) || !hb_now->enabled || box == target) return false;
    world::Hitbox hb = *hb_now;
    Vec3 away{0, 0, 0};
    if (hb.knockback != 0.0f) {
        // Away from the hitbox, along the ground for bodies that walk.
        const auto* tb = world_->try_get<world::WorldTransform>(box);
        const auto* tt = world_->try_get<world::WorldTransform>(target);
        away = tb && tt ? tt->position - tb->position : Vec3{0, 0, 0};
        if (world_->try_get<world::Body2D>(target)) away.z = 0;
        else away.y = 0;
        const float len = length(away);
        away = len > 1e-5f ? away * (hb.knockback / len) : Vec3{0, 0, 0};
    }
    if (land_damage(box, target, hb.damage, hb.team, away, cause, Json::object()) == 0) return false;
    hb.hits++;
    world_->set_typed<world::Hitbox>(box, hb);
    if (hb.destroy) spent.push_back(box);
    return true;
}

std::uint64_t Session::land_damage(world::EntityId by, world::EntityId target, float damage, std::int32_t team, Vec3 push, std::uint64_t cause, const Json& extra) {
    const auto* hp_now = world_->try_get<world::Health>(target);
    if (!hp_now) return 0;
    world::Health hp = *hp_now;
    if (team != 0 && team == hp.team) return 0;     // a team does not hurt its own
    if (hp.current > 0.0f) hp.dead = false;         // a script that set the hit points back revived it
    if (hp.guard > 0.0f || (hp.current <= 0.0f && damage > 0.0f)) return 0;
    hp.current = damage >= 0.0f ? std::max(0.0f, hp.current - damage) : std::min(hp.max, hp.current - damage);
    hp.guard = std::max(hp.invulnerable, 0.0f);
    const std::string by_path = by && world_->alive(by) ? world_->path(by) : std::string();
    Json data{{"by", by_path}, {"to", world_->path(target)}, {"damage", damage}, {"health", hp.current}};
    for (const auto& [k, v] : extra.items()) data[k] = v;
    const std::uint64_t seq = world_->events().emit(clock_.tick, "hit", target, data, cause, "engine");
    if (hp.current <= 0.0f && !hp.dead) {
        hp.dead = true;
        world_->events().emit(clock_.tick, "health.depleted", target, Json{{"by", by_path}, {"path", world_->path(target)}}, seq, "engine");
    } else if (hp.current > 0.0f) {
        hp.dead = false;
    }
    world_->set_typed<world::Health>(target, hp);
    if (push.x != 0.0f || push.y != 0.0f || push.z != 0.0f) {
        if (const auto* v = world_->try_get<world::Velocity>(target)) { world::Velocity nv = *v; nv.linear = nv.linear + push; world_->set_typed<world::Velocity>(target, nv); }
        if (const auto* c = world_->try_get<world::Character>(target)) { world::Character nc = *c; nc.velocity = nc.velocity + push; world_->set_typed<world::Character>(target, nc); }
        if (const auto* b = world_->try_get<world::Body2D>(target)) { world::Body2D nb = *b; nb.velocity = {nb.velocity.x + push.x, nb.velocity.y + push.y}; world_->set_typed<world::Body2D>(target, nb); }
    }
    return seq;
}

// A shot that arrives at once (docs/design/combat.md, Hitscan): the nearest solid collider or
// character capsule along the ray, the Health on it or on the nearest ancestor with one hurt.
Result<Json> Session::hitscan_command(const Json& p) {
    if (!world_) return fail("no_world", "no world");
    const Vec3 from = vec3_of(p.value("from", Json(nullptr)), {0, 0, 0});
    Vec3 dir = vec3_of(p.value("direction", Json(nullptr)), {0, 0, -1});
    const float dlen = length(dir);
    if (dlen <= 1e-6f) return fail("bad_args", "direction must not be zero");
    dir = dir * (1.0f / dlen);
    const float range = std::max(static_cast<float>(opt<double>(p, "range", 100.0)), 0.0f);
    const float damage = static_cast<float>(opt<double>(p, "damage", 10.0));
    const float knockback = static_cast<float>(opt<double>(p, "knockback", 0.0));
    const auto team = static_cast<std::int32_t>(opt<double>(p, "team", 0.0));
    const auto cause = static_cast<std::uint64_t>(opt<double>(p, "cause", 0.0));
    world::EntityId shooter = 0;
    if (p.contains("shooter") && !p["shooter"].is_null()) {
        shooter = resolve_entity(p["shooter"]);
        if (!world_->alive(shooter)) return fail("no_such_entity", "no shooter {}", p["shooter"].dump());
    }
    // The shooter and what hangs under it are not in the way.
    auto mine = [&](world::EntityId e) {
        for (world::EntityId at = e; at != 0 && shooter != 0; at = world_->parent(at)) if (at == shooter) return true;
        return false;
    };
    world::EntityId hit = 0;
    Vec3 point{}, normal{};
    float best = range;
    if (physics_) {
        auto ray = physics_->raycast(*world_, from, dir, range, [&](world::EntityId e, const world::RigidBody&, const world::Collider& col) { return !col.is_trigger && !mine(e); });
        if (ray) {
            hit = ray->entity;
            point = ray->point;
            normal = ray->normal;
            best = ray->distance;
        }
    }
    // Characters' capsules: the round body between two half spheres, upright about the entity.
    world_->ecs().each([&](flecs::entity e, const world::Character& c, const world::WorldTransform& t) {
        if (mine(e.id())) return;
        const float r = std::max(c.radius, 0.01f);
        const float half = std::max(c.height * 0.5f - r, 0.0f);
        const Vec3 pa = t.position - Vec3{0, half, 0}, pb = t.position + Vec3{0, half, 0};
        const Vec3 ba = pb - pa, oa = from - pa;
        const float baba = dot(ba, ba), bard = dot(ba, dir), baoa = dot(ba, oa), rdoa = dot(dir, oa), oaoa = dot(oa, oa);
        float tt = -1;
        Vec3 n{};
        const float a = baba - bard * bard;
        if (a > 1e-8f) {
            const float b = baba * rdoa - baoa * bard, cc = baba * oaoa - baoa * baoa - r * r * baba, h = b * b - a * cc;
            if (h >= 0) {
                const float tb = (-b - std::sqrt(h)) / a, y = baoa + tb * bard;
                if (tb >= 0 && y > 0 && y < baba) {
                    tt = tb;
                    const Vec3 q = from + dir * tb;
                    n = (q - (pa + ba * (y / baba))) * (1.0f / r);
                }
            }
        }
        if (tt < 0) {
            for (const Vec3 centre : {pa, pb}) {
                const Vec3 oc = from - centre;
                const float b = dot(dir, oc), h = b * b - (dot(oc, oc) - r * r);
                if (h < 0) continue;
                const float ts = -b - std::sqrt(h);
                if (ts >= 0 && (tt < 0 || ts < tt)) {
                    tt = ts;
                    n = (from + dir * ts - centre) * (1.0f / r);
                }
            }
        }
        if (tt >= 0 && tt < best) {
            best = tt;
            hit = e.id();
            point = from + dir * tt;
            normal = n;
        }
    });
    if (hit == 0) return Json{{"hit", false}};
    Json out{{"hit", true}, {"entity", hit}, {"path", world_->path(hit)}, {"point", json_of(point)}, {"normal", json_of(normal)}, {"distance", best}};
    world::EntityId target = hit;
    while (target != 0 && !world_->try_get<world::Health>(target)) target = world_->parent(target);
    out["landed"] = false;
    if (target != 0) {
        // Pushed along the shot: along the ground for what walks, as a hitbox pushes.
        Vec3 push = dir * knockback;
        if (world_->try_get<world::Character>(target) || world_->try_get<world::NavAgent>(target)) push.y = 0;
        if (world_->try_get<world::Body2D>(target)) push.z = 0;
        const std::uint64_t seq = land_damage(shooter, target, damage, team, push, cause, Json{{"point", json_of(point)}, {"hitscan", true}});
        out["target"] = target;
        out["target_path"] = world_->path(target);
        out["landed"] = seq != 0;
        if (seq) out["seq"] = seq;
        out["health"] = world_->try_get<world::Health>(target)->current;
    }
    return out;
}

void Session::update_hits(std::uint64_t since_seq) {
    const double now = clock_.sim_seconds();
    const float dt = static_cast<float>(clock_.tick_seconds);
    std::vector<world::EntityId> spent;
    // Invulnerability runs out.
    std::vector<std::pair<world::EntityId, world::Health>> cooled;
    world_->ecs().each([&](flecs::entity e, const world::Health& h) {
        if (h.guard > 0.0f) { world::Health n = h; n.guard = std::max(0.0f, h.guard - dt); cooled.emplace_back(e.id(), n); }
    });
    for (auto& [id, h] : cooled) world_->set_typed<world::Health>(id, h);
    // Touches begun and ended this tick, by 3D triggers and 2D areas, in the order they were reported.
    for (const world::Event& ev : world_->events().since(since_seq, 100000)) {
        const bool begin = ev.type == "trigger.enter" || ev.type == "area.entered";
        const bool end = ev.type == "trigger.exit" || ev.type == "area.exited";
        if (!begin && !end) continue;
        world::EntityId a = 0, b = 0;
        if (ev.type.starts_with("area.")) { a = ev.subject; b = world_->find(ev.data.value("area", "")); }
        else { a = world_->find(ev.data.value("a", "")); b = world_->find(ev.data.value("b", "")); }
        if (!a || !b) continue;
        for (const auto& [box, target] : {std::pair{a, b}, std::pair{b, a}}) {
            if (!world_->try_get<world::Hitbox>(box) || !world_->try_get<world::Health>(target)) continue;
            if (end) {
                std::erase_if(touches_, [&](const Touch& t) { return t.box == box && t.target == target; });
                continue;
            }
            apply_hit(box, target, ev.seq, spent);
            const float repeat = world_->try_get<world::Hitbox>(box)->repeat;
            if (repeat > 0.0f) touches_.push_back(Touch{box, target, now + repeat});
        }
    }
    // Hitboxes that hit again while their targets stay.
    for (Touch& t : touches_) {
        if (!world_->alive(t.box) || !world_->alive(t.target) || now + 1e-9 < t.next) continue;
        const auto* hb = world_->try_get<world::Hitbox>(t.box);
        if (!hb) continue;
        apply_hit(t.box, t.target, 0, spent);
        t.next = now + std::max(hb->repeat, static_cast<float>(clock_.tick_seconds));
    }
    std::erase_if(touches_, [&](const Touch& t) { return !world_->alive(t.box) || !world_->alive(t.target); });
    for (world::EntityId id : spent) if (world_->alive(id)) (void)world_->destroy(id);
}

void Session::update_attachments() {
    // Each Attach: the joint's frame in the world (the target's place times the pose's node), the
    // offset in it, and that brought into the attached entity's parent's space.
    struct Placed { world::EntityId id; Vec3 position; Quat rotation; bool found; };
    std::vector<Placed> placed;
    world_->ecs().each([&](flecs::entity e, const world::Attach& a) {
        Placed out{e.id(), {}, {}, false};
        const world::EntityId target = a.target.empty() ? 0 : world_->find(a.target);
        const auto* mr = target ? world_->try_get<world::MeshRenderer>(target) : nullptr;
        const auto* wt = target ? world_->try_get<world::WorldTransform>(target) : nullptr;
        if (mr && wt && assets_ && target != e.id()) {
            if (auto mesh = assets_->mesh(mr->mesh)) {
                const assets::Mesh& m = **mesh;
                int node = -1;
                for (std::size_t i = 0; i < m.nodes.size(); ++i) if (m.nodes[i].name == a.joint) { node = static_cast<int>(i); break; }
                if (node >= 0) {
                    const renderer::Pose* pose = animation_->pose(target);
                    const Mat4 g = pose && static_cast<std::size_t>(node) < pose->globals.size() ? pose->globals[static_cast<std::size_t>(node)] : m.rest_global(node);
                    Mat4 at = Mat4::trs(wt->position, wt->rotation, wt->scale) * g * Mat4::trs(a.offset, a.rotation, {1, 1, 1});
                    if (const world::EntityId parent = world_->parent(e.id())) {
                        if (const auto* pw = world_->try_get<world::WorldTransform>(parent)) at = Mat4::trs(pw->position, pw->rotation, pw->scale).inverse_affine() * at;
                    }
                    Vec3 scale;
                    decompose(at, out.position, out.rotation, scale);
                    out.rotation = normalize(out.rotation);
                    out.found = true;
                }
            }
        }
        placed.push_back(out);
    });
    if (placed.empty()) return;
    for (const Placed& p : placed) {
        world::Attach a = *world_->try_get<world::Attach>(p.id);
        if (a.found != p.found) { a.found = p.found; world_->set_typed<world::Attach>(p.id, a); }
        if (!p.found) continue;
        world::Transform t = world_->try_get<world::Transform>(p.id) ? *world_->try_get<world::Transform>(p.id) : world::Transform{};
        t.position = p.position;
        t.rotation = p.rotation;   // its own scale stays
        world_->set_typed<world::Transform>(p.id, t);
    }
    world_->update_transforms();   // so this tick's frame draws it where the joint is
}

void Session::set_cursor(bool locked, bool visible) {
    cursor_locked_ = locked;
    cursor_visible_ = visible;
    // The editor never captures or hides the pointer: its panels need it (a run of the game does).
    if (platform_ && options_.editor_bundle.empty()) platform_->set_cursor(locked, visible);
}

bool Session::cursor_held() const { return platform_ && options_.editor_bundle.empty() && platform_->cursor().held; }

ui::NodeId Session::ui_press_target(const platform::Event& e) const {
    // A press on the interface is the interface's: the action bound to that button stays where it was.
    if (!ui_ || e.type != platform::EventType::MouseDown || cursor_held()) return 0;
    return ui_->hit_test(e.x, e.y);
}

void Session::release_expired_holds() {
    if (held_keys_.empty()) return;
    std::vector<platform::Event> ups;
    for (auto it = held_keys_.begin(); it != held_keys_.end();) {
        if (clock_.tick >= it->second) {
            ups.push_back(press_event(it->first, false));
            it = held_keys_.erase(it);
        } else {
            ++it;
        }
    }
    if (!ups.empty()) inject_events(std::move(ups));
}

std::filesystem::path Session::save_dir() const {
    if (!options_.save_dir.empty()) return options_.save_dir;
    if (const char* env = std::getenv("POCKET_SAVE_DIR"); env && *env) return env;
    if (project_.contains("saves") && project_["saves"].is_string()) return options_.project_dir / project_["saves"].get<std::string>();
#ifdef __EMSCRIPTEN__
    // The browser's persistent store (IndexedDB through IDBFS), mounted in start().
    return std::filesystem::path("/saves") / (name_.empty() ? "project" : name_);
#else
    return platform::user_data_dir("pocket", name_.empty() ? "project" : name_);
#endif
}

Result<Json> Session::save_command(std::string_view op, const Json& p) {
    // Save slots: the world as a scene plus whatever scripts return from onSave, one JSON file
    // per slot in the project's user data directory (or --save-dir / POCKET_SAVE_DIR).
    std::filesystem::path dir = save_dir();
    auto slot_path = [&](std::string& slot) -> Result<std::filesystem::path> {
        slot = opt<std::string>(p, "slot", "");
        if (slot.empty() || slot.size() > 64) return fail("bad_args", "save needs a slot name (1-64 characters: letters, digits, '-', '_')");
        for (char c : slot) if (!(std::isalnum(static_cast<unsigned char>(c)) || c == '-' || c == '_')) return fail("bad_args", "slot '{}' has characters other than letters, digits, '-', '_'", slot);
        return dir / (slot + ".json");
    };
    if (op == "dir") return Json{{"path", dir.string()}};
    if (op == "write") {
        std::string slot;
        POCKET_TRY(path, slot_path(slot));
        Json save;
        save["format"] = "pocket-save";
        save["version"] = 1;
        save["project"] = name_;
        save["tick"] = clock_.tick;
        save["sim_seconds"] = clock_.sim_seconds();
        save["env_t"] = clock_.tick - env_start_tick_;   // the episode's clock, so a load continues it (env.step's t and done)
        save["env_max_ticks"] = env_max_ticks_;
        save["scene"] = world_->save();
        if (!made_meshes_.empty()) save["scene"]["meshes"] = made_meshes_;
        if (Json maps = saved_maps(true); !maps.empty()) save["scene"]["tilemaps"] = maps;
        Json script = dispatch("save", nullptr);
        save["script"] = script.is_object() ? script : Json::object();
        if (p.contains("data")) save["data"] = p["data"];
        std::error_code ec;
        std::filesystem::create_directories(dir, ec);
        std::string text = save.dump(2) + "\n";
        POCKET_TRY_VOID(fs::write_text(path, text));
#ifdef __EMSCRIPTEN__
        web_sync_saves();
#endif
        world_->events().emit(clock_.tick, "save.written", 0, Json{{"slot", slot}, {"entities", world_->entity_count()}}, 0, "save");
        return Json{{"slot", slot}, {"path", path.string()}, {"bytes", text.size()}, {"entities", world_->entity_count()}};
    }
    if (op == "read") {
        std::string slot;
        POCKET_TRY(path, slot_path(slot));
        if (!std::filesystem::exists(path)) return fail("no_such_save", "no save slot '{}' in {}", slot, dir.string());
        POCKET_TRY(text, fs::read_text(path));
        Json save = Json::parse(text, nullptr, false);
        if (save.is_discarded() || !save.is_object() || save.value("format", "") != "pocket-save") return fail("bad_save", "{} is not a Pocket save", path.string());
        const Json scene = save.value("scene", Json::object());
        if (scene.contains("meshes") && scene["meshes"].is_object())
            for (const auto& [mname, spec] : scene["meshes"].items()) POCKET_TRY_VOID(make_mesh(mname, spec));
        if (scene.contains("tilemaps")) POCKET_TRY_VOID(restore_maps(scene["tilemaps"]));
        POCKET_TRY_VOID(world_->load(scene, true));
        world_->update_transforms();
        dispatch("load", save.value("script", Json::object()));
        if (save.contains("env_t") && save["env_t"].is_number()) {
            env_start_tick_ = clock_.tick - save["env_t"].get<std::int64_t>();
            if (save.contains("env_max_ticks") && save["env_max_ticks"].is_number()) env_max_ticks_ = save["env_max_ticks"].get<int>();
        }
        Json s = dispatch("state", nullptr);
        if (s.is_object()) last_state_ = s;
        world_->events().emit(clock_.tick, "save.loaded", 0, Json{{"slot", slot}, {"entities", world_->entity_count()}, {"saved_tick", save.value("tick", 0)}}, 0, "save");
        Json j;
        j["slot"] = slot;
        j["entities"] = world_->entity_count();
        j["saved_tick"] = save.value("tick", 0);
        j["saved_sim_seconds"] = save.value("sim_seconds", 0.0);
        if (save.contains("env_t")) j["env_t"] = save["env_t"];
        if (save.contains("data")) j["data"] = save["data"];
        return j;
    }
    if (op == "list") {
        Json out = Json::array();
        std::error_code ec;
        if (std::filesystem::exists(dir, ec)) {
            std::vector<std::filesystem::path> files;
            for (const auto& e : std::filesystem::directory_iterator(dir, ec)) if (e.path().extension() == ".json") files.push_back(e.path());
            std::sort(files.begin(), files.end());
            for (const auto& f : files) {
                Json j;
                j["slot"] = f.stem().string();
                j["bytes"] = std::filesystem::file_size(f, ec);
                auto t = std::filesystem::last_write_time(f, ec);
                j["modified"] = std::chrono::duration_cast<std::chrono::seconds>(t.time_since_epoch()).count();
                if (auto text = fs::read_text(f)) {
                    Json save = Json::parse(*text, nullptr, false);
                    if (save.is_object()) {
                        j["tick"] = save.value("tick", 0);
                        j["entities"] = save.contains("scene") && save["scene"].is_object() && save["scene"].contains("entities") ? save["scene"]["entities"].size() : 0;
                        if (save.contains("data")) j["data"] = save["data"];
                    }
                }
                out.push_back(std::move(j));
            }
        }
        return out;
    }
    if (op == "delete") {
        std::string slot;
        POCKET_TRY(path, slot_path(slot));
        std::error_code ec;
        bool existed = std::filesystem::remove(path, ec);
#ifdef __EMSCRIPTEN__
        if (existed) web_sync_saves();
#endif
        return Json{{"slot", slot}, {"deleted", existed}};
    }
    return fail("unknown_command", "unknown save command '{}'", op);
}

Result<Json> Session::input_command(std::string_view op, const Json& p) {
    if (op == "map") {
        POCKET_TRY_VOID(input_map_.configure(p.contains("actions") ? p["actions"] : p));
        return Json{{"actions", input_map_.size()}};
    }
    if (op == "actions") return input_map_.snapshot();
    if (op == "describe") return input_map_.describe();
    if (op == "cursor") {
        // Capture the pointer (first-person, mouse orbit) or hide it; Escape lets it go and a click
        // takes it again (docs/design/input.md, The cursor).
        for (const char* k : {"locked", "visible"}) if (p.contains(k) && !p[k].is_boolean()) return fail("bad_args", "{} is true or false", k);
        if (p.contains("locked") || p.contains("visible")) set_cursor(opt<bool>(p, "locked", cursor_locked_), opt<bool>(p, "visible", cursor_visible_));
        // Where the pointer is, in the window's pixels (render.unproject and render.pick take them).
        float w = 0, h = 0, scale = 1;
        ui_size(w, h, scale);
        return Json{{"locked", cursor_locked_}, {"held", cursor_held()}, {"visible", cursor_visible_}, {"x", platform_->input().mouse_x * scale}, {"y", platform_->input().mouse_y * scale}};
    }
    if (op == "state") {
        Json j;
        j["platform"] = platform_->describe();
        j["pads"] = platform_->input().pads;
        j["fingers"] = platform_->input().fingers + static_cast<int>(touch_last_.size());   // real fingers and synthetic ones
        j["gestures"] = Json{{"enabled", gestures_.settings().enabled}, {"slop", gestures_.settings().slop}, {"tap_seconds", gestures_.settings().tap_seconds}, {"hold_seconds", gestures_.settings().hold_seconds}, {"swipe_points", gestures_.settings().swipe_points}, {"swipe_seconds", gestures_.settings().swipe_seconds}, {"edge", gestures_.settings().edge}};
        j["rumble"] = Json::object();
        for (const auto& [pad, r] : rumble_) j["rumble"][std::to_string(pad)] = Json{{"step", r.next}, {"steps", r.steps.size()}, {"repeat", r.repeat}};
        j["held"] = Json::object();
        for (auto& [k, until] : held_keys_) j["held"][k] = until;
        j["actions"] = input_map_.snapshot();
        j["cursor"] = Json{{"locked", cursor_locked_}, {"held", cursor_held()}, {"visible", cursor_visible_}};
        return j;
    }
    if (op == "rumble") {
        // Shake a gamepad: one step of motor strengths for `ms`, or a `pattern` of steps played
        // one after another on the tick clock (`repeat` times), replacing what the pad was playing;
        // `stop` silences it. `rumbled` is false without a pad that can (headless runs); the steps
        // still play as `input.rumble` world events.
        const int pad = std::clamp(opt<int>(p, "pad", 0), 0, 15);
        if (opt<bool>(p, "stop", false)) {
            const bool playing = rumble_.erase(pad) > 0;
            if (platform_) platform_->rumble(pad, 0, 0, 0);
            return Json{{"stopped", playing}};
        }
        RumblePattern r;
        auto read_step = [&](const Json& s) {
            RumbleStep step;
            step.low = std::clamp(opt<float>(s, "low", 1.0f), 0.0f, 1.0f);
            step.high = std::clamp(opt<float>(s, "high", 1.0f), 0.0f, 1.0f);
            step.ms = std::clamp(opt<int>(s, "ms", 200), 0, 10000);
            return step;
        };
        if (p.contains("pattern")) {
            if (!p["pattern"].is_array() || p["pattern"].empty() || p["pattern"].size() > 64) return fail("bad_args", "pattern is an array of 1 to 64 steps {{low, high, ms}}");
            for (const Json& s : p["pattern"]) {
                if (!s.is_object()) return fail("bad_args", "a pattern step is {{low, high, ms}}");
                r.steps.push_back(read_step(s));
            }
            r.repeat = std::clamp(opt<int>(p, "repeat", 1), 1, 100);
        } else {
            r.steps.push_back(read_step(p));
        }
        int total = 0;
        for (const RumbleStep& s : r.steps) total += s.ms;
        const std::size_t steps = r.steps.size();
        const int repeat = r.repeat;
        r.next_tick = clock_.tick;
        rumble_[pad] = std::move(r);
        const bool rumbled = advance_rumble();
        return Json{{"rumbled", rumbled}, {"pad", pad}, {"steps", steps}, {"seconds", total * repeat / 1000.0}};
    }
    if (op == "pad") {
        // A gamepad button or axis for tests and agents, by pad index, through the same path as a
        // real pad (the map, the journal, the scripts' input events).
        const int pad = std::clamp(opt<int>(p, "pad", 0), 0, 15);
        platform::Event e;
        e.pad = pad;
        if (p.contains("button") && p["button"].is_string()) {
            e.type = platform::EventType::PadButton;
            e.key_name = p["button"].get<std::string>();
            e.pressed = opt<bool>(p, "pressed", true);
        } else if (p.contains("axis") && p["axis"].is_string()) {
            e.type = platform::EventType::PadAxis;
            e.key_name = p["axis"].get<std::string>();
            e.value = std::clamp(opt<float>(p, "value", 0.0f), -1.0f, 1.0f);
        } else {
            return fail("bad_args", "pad needs a button (with pressed) or an axis (with value)");
        }
        Json j = inject_events({e});
        j["pad"] = pad;
        j["actions"] = input_map_.snapshot();
        return j;
    }
    if (op == "touch") {
        // A finger on the screen at window points, for tests and agents: down, move or up. Like a
        // real touch, the first finger also acts as the mouse (buttons, drags, mouse bindings).
        const std::string phase = opt<std::string>(p, "phase", "down");
        if (phase != "down" && phase != "move" && phase != "up") return fail("bad_args", "phase is down, move or up");
        const int finger = std::clamp(opt<int>(p, "finger", 0), 0, 9);
        const float x = opt<float>(p, "x", 0.0f), y = opt<float>(p, "y", 0.0f);
        const float pressure = std::clamp(opt<float>(p, "pressure", phase == "up" ? 0.0f : 1.0f), 0.0f, 1.0f);
        const auto last = touch_last_.find(finger);
        if (phase != "down" && last == touch_last_.end()) return fail("bad_args", "finger {} is not down", finger);
        std::vector<platform::Event> evs;
        platform::Event t;
        t.type = phase == "down" ? platform::EventType::TouchDown : phase == "up" ? platform::EventType::TouchUp : platform::EventType::TouchMove;
        t.pad = finger;
        t.x = x;
        t.y = y;
        t.dx = last == touch_last_.end() ? 0.0f : x - last->second.first;
        t.dy = last == touch_last_.end() ? 0.0f : y - last->second.second;
        t.pressed = phase != "up";
        t.value = pressure;
        evs.push_back(t);
        if (finger == 0) {
            platform::Event m;
            m.type = phase == "down" ? platform::EventType::MouseDown : phase == "up" ? platform::EventType::MouseUp : platform::EventType::MouseMove;
            m.button = 1;
            m.x = x;
            m.y = y;
            m.dx = t.dx;
            m.dy = t.dy;
            evs.push_back(m);
        }
        if (phase == "up") touch_last_.erase(finger);
        else touch_last_[finger] = {x, y};
        Json j = inject_events(std::move(evs));
        j["finger"] = finger;
        j["phase"] = phase;
        return j;
    }
    if (op == "axis") {
        // An action's value set directly, -1..1, until set again (0 lets go): an on-screen stick's
        // tilt, or an agent pushing a stick halfway. Past 0.5 either way the action is down.
        const std::string action = opt<std::string>(p, "action", "");
        if (!input_map_.has_action(action)) return fail("no_such_action", "no action named '{}' (input.actions lists them)", action);
        if (!p.contains("value") || !p["value"].is_number()) return fail("bad_args", "input.axis needs value, -1 to 1");
        platform::Event e;
        e.type = platform::EventType::PadAxis;
        e.pad = -1;
        e.key_name = "@" + action;
        e.value = std::clamp(p["value"].get<float>(), -1.0f, 1.0f);
        if (in_tick_) {
            // From a script mid-tick: from the next tick, as a hold would.
            pending_events_.push_back(e);
            return Json{{"action", action}, {"value", e.value}, {"from_tick", clock_.tick + 1}};
        }
        inject_events({e});
        return Json{{"action", action}, {"value", e.value}, {"from_tick", clock_.tick}};
    }
    if (op == "hold" || op == "press") {
        // Press a key (or every key bound to an action) now and release it after `ticks` ticks
        // (1 for press). Goes through the same path as real input, including the journal.
        int ticks = op == "press" ? 1 : std::clamp(opt<int>(p, "ticks", 1), 1, 100000);
        std::vector<std::string> keys;
        if (p.contains("key") && p["key"].is_string()) keys.push_back(p["key"].get<std::string>());
        else if (p.contains("action") && p["action"].is_string()) {
            std::string action = p["action"].get<std::string>();
            if (!input_map_.has_action(action)) return fail("no_such_action", "no action named '{}'", action);
            // sign -1 (or a negative value) holds the action's negative direction.
            int sign = opt<int>(p, "sign", 1);
            if (p.contains("value") && p["value"].is_number() && p["value"].get<double>() < 0) sign = -1;
            keys = input_map_.keys_of(action, sign < 0 ? -1 : 1);
            if (keys.empty()) return fail("bad_args", "action '{}' has no {} key bindings to hold", action, sign < 0 ? "negative" : "positive");
            keys.resize(1);
        } else {
            return fail("bad_args", "hold needs a key name or an action");
        }
        if (in_tick_) {
            // Asked for by a script mid-tick: the press lands at the start of the next tick, where
            // its pressed edge is seen by every script of that tick (this tick's edges are spent).
            for (const auto& k : keys) pending_holds_.push_back(PendingHold{k, ticks, op == "press"});
            Json j;
            j["keys"] = keys;
            j["from_tick"] = clock_.tick + 1;
            j["until_tick"] = clock_.tick + 1 + ticks;
            j["actions"] = input_map_.snapshot();
            return j;
        }
        std::vector<platform::Event> downs;
        for (const auto& k : keys) {
            if (held_keys_.contains(k)) {
                // Held already: a hold goes on longer; a press is a new press, so the key goes up
                // and down again and the next tick sees its edge (presses a tick apart are two).
                held_keys_[k] = std::max(held_keys_[k], clock_.tick + ticks);
                if (op != "press") continue;
                downs.push_back(press_event(k, false));
            }
            downs.push_back(press_event(k, true));
            held_keys_.try_emplace(k, clock_.tick + ticks);
        }
        if (!downs.empty()) inject_events(std::move(downs));
        Json j;
        j["keys"] = keys;
        j["until_tick"] = clock_.tick + ticks;
        j["actions"] = input_map_.snapshot();
        return j;
    }
    if (op == "release") {
        // Let go now of what input.hold holds: a key, an action's keys both ways, or every key held.
        std::vector<std::string> keys;
        if (p.contains("key") && p["key"].is_string()) keys.push_back(p["key"].get<std::string>());
        else if (p.contains("action") && p["action"].is_string()) {
            const std::string action = p["action"].get<std::string>();
            if (!input_map_.has_action(action)) return fail("no_such_action", "no action named '{}'", action);
            for (int sign : {1, -1}) for (const auto& k : input_map_.keys_of(action, sign)) keys.push_back(k);
        } else {
            for (const auto& [k, until] : held_keys_) keys.push_back(k);
        }
        std::vector<platform::Event> ups;
        Json released = Json::array();
        for (const auto& k : keys) {
            if (!held_keys_.contains(k)) continue;
            held_keys_.erase(k);
            ups.push_back(press_event(k, false));
            released.push_back(k);
        }
        std::erase_if(pending_holds_, [&](const PendingHold& h) { return std::find(keys.begin(), keys.end(), h.key) != keys.end(); });
        if (!ups.empty()) inject_events(std::move(ups));
        return Json{{"released", released}, {"actions", input_map_.snapshot()}};
    }
    return fail("unknown_command", "unknown input command '{}'", op);
}

Result<Json> Session::audio_command(std::string_view op, const Json& p) {
    audio::Audio& a = *audio_;
    auto voice_json = [](const audio::VoiceInfo& v) {
        Json j;
        j["id"] = v.id;
        j["clip"] = v.clip;
        j["position"] = v.position;
        j["duration"] = v.duration;
        j["volume"] = v.volume;
        j["pitch"] = v.pitch;
        j["pan"] = v.pan;
        j["lowpass"] = v.lowpass;
        j["reverb"] = v.reverb;
        j["loop"] = v.loop;
        j["entity"] = v.entity;
        j["tag"] = v.tag;
        j["bus"] = v.bus;
        j["loops_done"] = v.loops_done;
        return j;
    };
    auto bus_json = [](const audio::BusInfo& b) {
        // Settings as they were written (0.1, not the float's 0.10000000149).
        const auto round = [](float v) { return std::round(static_cast<double>(v) * 10000.0) / 10000.0; };
        Json j;
        j["name"] = b.name;
        j["volume"] = round(b.settings.volume);
        j["muted"] = b.settings.muted;
        j["lowpass"] = round(b.settings.lowpass);
        j["highpass"] = round(b.settings.highpass);
        j["echo"] = round(b.settings.echo);
        j["echo_feedback"] = round(b.settings.echo_feedback);
        j["echo_mix"] = round(b.settings.echo_mix);
        j["reverb"] = round(b.settings.reverb);
        j["duck_by"] = b.settings.duck_by;
        j["duck_amount"] = round(b.settings.duck_amount);
        j["duck_seconds"] = round(b.settings.duck_seconds);
        j["duck"] = round(b.duck);
        j["ducked"] = b.ducked;
        j["voices"] = b.voices;
        return j;
    };
    if (op == "play") {
        std::string clip = opt<std::string>(p, "clip", "");
        if (clip.empty()) return fail("bad_args", "play needs a clip path");
        audio::PlayOptions o;
        o.volume = static_cast<float>(opt<double>(p, "volume", 1.0));
        o.pitch = static_cast<float>(opt<double>(p, "pitch", 1.0));
        o.pan = static_cast<float>(opt<double>(p, "pan", 0.0));
        o.lowpass = static_cast<float>(opt<double>(p, "lowpass", 1.0));
        o.reverb = static_cast<float>(opt<double>(p, "reverb", 1.0));
        o.loop = opt<bool>(p, "loop", false);
        o.tag = opt<std::string>(p, "tag", "");
        o.bus = opt<std::string>(p, "bus", "main");
        if (p.contains("entity") && !p["entity"].is_null()) o.entity = resolve_entity(p["entity"]);
        // A spatial one-shot follows its entity: placed now and every tick until it ends.
        const bool spatial = opt<bool>(p, "spatial", false);
        if (spatial && o.entity == 0) return fail("bad_args", "a spatial voice needs an entity to be heard from");
        POCKET_TRY(id, a.play(clip, o));
        if (spatial) {
            spatial_voices_[id] = SpatialVoice{o.entity, o.volume, static_cast<float>(opt<double>(p, "near", 1.0)), static_cast<float>(opt<double>(p, "range", 20.0)), static_cast<float>(opt<double>(p, "occlusion", 0.0)), o.lowpass, o.pitch, static_cast<float>(opt<double>(p, "doppler", 1.0))};
            const SpatialVoice& sv = spatial_voices_[id];
            place_voice(id, o.entity, o.volume, sv.near, sv.range, sv.occlusion, sv.lowpass);
        }
        world_->events().emit(clock_.tick, "audio.started", o.entity, Json{{"clip", clip}, {"voice", id}, {"loop", o.loop}, {"spatial", spatial}}, 0, "audio");
        return Json{{"voice", id}, {"clip", clip}};
    }
    if (op == "stop") {
        std::uint32_t n = 0;
        if (p.contains("voice") && p["voice"].is_number()) n = a.stop(p["voice"].get<std::uint32_t>());
        else if (p.contains("clip") && p["clip"].is_string()) n = a.stop_clip(p["clip"].get<std::string>());
        else if (p.contains("tag") && p["tag"].is_string()) n = a.stop_tag(p["tag"].get<std::string>());
        else if (p.contains("bus") && p["bus"].is_string()) n = a.stop_bus(p["bus"].get<std::string>());
        else n = a.stop_all();
        return Json{{"stopped", n}};
    }
    if (op == "set") {
        POCKET_TRY_VOID(a.set(static_cast<std::uint32_t>(opt<int>(p, "voice", 0)), p));
        return Json{{"ok", true}};
    }
    if (op == "list") {
        Json arr = Json::array();
        for (const auto& v : a.voices()) arr.push_back(voice_json(v));
        return arr;
    }
    if (op == "clips") return a.clips();
    if (op == "analyze") {
        // What a clip sounds like, in numbers and words, for someone who cannot hear it.
        const std::string clip = opt<std::string>(p, "clip", "");
        if (clip.empty()) return fail("bad_args", "audio.analyze needs clip: a project path (assets/coin.sfx, assets/theme.song, a WAV, Ogg or MP3)");
        return a.analyze(clip);
    }
    if (op == "stats") return a.describe();
    if (op == "bus") {
        // A group of voices set as one: volume, mute, a low-pass over its mix, ducking under another bus.
        const std::string name = opt<std::string>(p, "name", "");
        if (name.empty()) return fail("bad_args", "audio.bus needs a bus name");
        a.set_bus(name, bus_settings(a.bus(name), p));
        for (const audio::BusInfo& b : a.buses()) if (b.name == name) return bus_json(b);
        return fail("internal", "bus '{}' was not made", name);
    }
    if (op == "buses") {
        Json arr = Json::array();
        for (const audio::BusInfo& b : a.buses()) arr.push_back(bus_json(b));
        return arr;
    }
    if (op == "master") {
        if (p.contains("volume") && p["volume"].is_number()) a.set_master_volume(p["volume"].get<float>());
        if (p.contains("muted") && p["muted"].is_boolean()) a.set_muted(p["muted"].get<bool>());
        return Json{{"master_volume", a.master_volume()}, {"muted", a.muted()}};
    }
    if (op == "reverb") {
        // The room every voice plays in: how long the tail rings, how fast its high end dies,
        // how loud it is against the dry voices. Room 0 is no reverb.
        audio::ReverbSettings r = a.reverb();
        r.room = static_cast<float>(opt<double>(p, "room", r.room));
        r.damping = static_cast<float>(opt<double>(p, "damping", r.damping));
        r.mix = static_cast<float>(opt<double>(p, "mix", r.mix));
        a.set_reverb(r);
        r = a.reverb();
        return Json{{"room", r.room}, {"damping", r.damping}, {"mix", r.mix}};
    }
    return fail("unknown_command", "unknown audio command '{}'", op);
}

// The listener: the first enabled AudioListener entity, else the camera of the last frame.
Vec3 Session::listener_position(Vec3* forward_out, world::EntityId* listener_out) const {
    const renderer::CameraView& cam = renderer_->camera();
    Vec3 ear = cam.position, forward = cam.forward;
    world::EntityId listener = 0;
    world_->ecs().each([&](flecs::entity e, const world::AudioListener& l) {
        if (l.enabled && (listener == 0 || e.id() < listener) && world_->try_get<world::WorldTransform>(e.id())) listener = e.id();
    });
    if (listener != 0) {
        const auto* lt = world_->try_get<world::WorldTransform>(listener);
        ear = lt->position;
        forward = lt->rotation.rotate(Vec3{0, 0, -1});
    }
    if (forward_out) *forward_out = forward;
    if (listener_out) *listener_out = listener;
    return ear;
}

// The listener is the camera of the last frame; a spatial voice's gain falls from full within
// `near` to nothing at `range`, and it pans toward the side its entity is on, most of the way.
bool Session::place_voice(std::uint32_t voice, world::EntityId entity, float base_volume, float near, float range, float occlusion, float base_lowpass, world::EntityId* blocker, float base_pitch, float doppler, double dt) {
    const auto* wt = world_->try_get<world::WorldTransform>(entity);
    if (!wt) return false;
    occlusion = std::clamp(occlusion, 0.0f, 1.0f);
    Vec3 forward;
    world::EntityId listener = 0;
    const Vec3 ear = listener_position(&forward, &listener);
    const Vec3 to = wt->position - ear;
    const float d = length(to);
    const float span = std::max(range - near, 1e-3f);
    const float gain = std::clamp((range - d) / span, 0.0f, 1.0f);
    Vec3 right = cross(forward, Vec3{0, 1, 0});
    if (length(right) < 1e-4f) right = Vec3{1, 0, 0};
    right = normalize(right);
    const float pan = d > 1e-4f ? std::clamp(dot(to * (1.0f / d), right), -1.0f, 1.0f) * 0.8f : 0.0f;
    // A wall in the way: one ray from the ear to the source, past the listener's own collider and the
    // source's, through triggers.
    bool blocked = false;
    world::EntityId by = 0;
    if (occlusion > 0 && physics_ && d > 1e-3f) {
        auto hit = physics_->raycast(*world_, ear, to * (1.0f / d), d, [entity, listener](world::EntityId e, const world::RigidBody&, const world::Collider& col) { return !col.is_trigger && e != entity && e != listener; });
        if (hit) {
            blocked = true;
            by = hit->entity;
        }
    }
    const float keep = blocked ? 1.0f - occlusion : 1.0f;
    Json params{{"volume", base_volume * gain * keep}, {"pan", pan}};
    if (occlusion > 0) params["lowpass"] = base_lowpass * keep;
    if (doppler > 0 && dt > 0) {
        // The Doppler effect: the source's and the listener's speeds along the line between them,
        // this tick's motion, against sound's 343 m/s. A source first heard this tick has not moved.
        const Vec3 source_vel = voice_prev_pos_.contains(voice) ? (wt->position - voice_prev_pos_[voice]) * static_cast<float>(1.0 / dt) : Vec3{0, 0, 0};
        voice_prev_pos_[voice] = wt->position;
        const Vec3 dir = d > 1e-4f ? to * (1.0f / d) : Vec3{0, 0, 0};
        constexpr float kSound = 343.0f;
        const float away = dot(source_vel, dir) * doppler;        // the source moving off
        const float toward = dot(listener_vel_, dir) * doppler;   // the listener moving at it
        const float factor = std::clamp((kSound + toward) / std::max(kSound + away, 1.0f), 0.5f, 2.0f);
        params["pitch"] = base_pitch * factor;
    }
    (void)audio_->set(voice, params);
    if (blocker) *blocker = by;
    return blocked;
}

// AudioSource components start their voices; voices report back; finished voices clear `playing`.
void Session::tick_audio(double dt) {
    world::World& w = *world_;
    // Footsteps (Animator.footsteps): each footfall's sound from where its entity stands, a little
    // varied (pitch and, for an sfx: recipe, its seed, by the event's number), louder running.
    for (const world::Event& e : w.events().since(footsteps_seen_, 256, "animation.footstep")) {
        footsteps_seen_ = e.seq;
        const std::string sound = e.data.value("sound", "");
        if (sound.empty() || !w.alive(e.subject)) continue;
        const bool run = e.data.value("clip", "") == "run";
        audio::PlayOptions o;
        o.volume = run ? 0.7f : 0.45f;
        o.pitch = 0.92f + 0.16f * static_cast<float>(e.seq % 7) / 6.0f;
        o.entity = e.subject;
        o.tag = "footstep";
        const std::string clip = sound.starts_with("sfx:") && sound.find("seed=") == std::string::npos ? sound + (sound.find('?') == std::string::npos ? "?" : "&") + "seed=" + std::to_string(e.seq % 5) : sound;
        if (auto id = audio_->play(clip, o)) place_voice(*id, e.subject, o.volume, 2.0f, 25.0f, 0.0f, 1.0f, nullptr, o.pitch, 0.0f, dt);
    }
    footsteps_seen_ = std::max(footsteps_seen_, w.events().last_seq());
    // The weather's sound: its rain and wind beds looping as loud as it rains and snows, started
    // when it begins and stopped when it ends (Weather.sound false leaves them to the game).
    {
        world::EntityId wid = 0;
        world::Weather wx;
        w.ecs().each([&](flecs::entity e, const world::Weather& x) {
            if (x.enabled && (!wid || e.id() < wid)) { wid = e.id(); wx = x; }
        });
        const bool on = wid && wx.sound;
        const float want[2] = {on ? 0.6f * std::clamp(wx.rain, 0.0f, 1.0f) : 0.0f, on ? 0.35f * std::clamp(wx.snow, 0.0f, 1.0f) : 0.0f};
        const char* clips[2] = {"sfx:rain", "sfx:wind"};
        for (int k = 0; k < 2; ++k) {
            if (want[k] < 0.01f) {
                if (weather_voice_[k]) audio_->stop(weather_voice_[k]);
                weather_voice_[k] = 0;
                weather_volume_[k] = 0;
                continue;
            }
            if (!weather_voice_[k]) {
                audio::PlayOptions o;
                o.volume = want[k];
                o.loop = true;
                if (auto id = audio_->play(clips[k], o)) weather_voice_[k] = *id;
                weather_volume_[k] = want[k];
            } else if (std::fabs(want[k] - weather_volume_[k]) > 0.01f) {
                if (!audio_->set(weather_voice_[k], Json{{"volume", want[k]}})) weather_voice_[k] = 0;   // gone (stopped by the game): started again next tick
                weather_volume_[k] = want[k];
            }
        }
    }
    // The listener's velocity this tick, for the Doppler effect.
    {
        const Vec3 ear = listener_position();
        listener_vel_ = listener_prev_set_ && dt > 0 ? (ear - listener_prev_) * static_cast<float>(1.0 / dt) : Vec3{0, 0, 0};
        listener_prev_ = ear;
        listener_prev_set_ = true;
    }
    std::vector<std::pair<world::EntityId, world::AudioSource>> updates;
    w.ecs().each([&](flecs::entity e, const world::AudioSource& src) {
        if (src.autoplay && !src.playing && src.voice == 0 && !src.clip.empty()) {
            audio::PlayOptions o;
            o.volume = src.volume;
            o.pitch = src.pitch;
            o.lowpass = src.lowpass;
            o.reverb = src.reverb;
            o.bus = src.bus;
            o.loop = src.loop;
            o.entity = e.id();
            auto id = audio_->play(src.clip, o);
            world::AudioSource next = src;
            if (id) {
                next.playing = true;
                next.voice = *id;
                w.events().emit(clock_.tick, "audio.started", e.id(), Json{{"clip", src.clip}, {"voice", *id}, {"loop", src.loop}}, 0, "audio");
            } else {
                next.autoplay = false;  // do not retry every tick; the error is logged once
                log::warn("audio", "{}: {}", w.path(e.id()), id.error().to_string());
            }
            updates.emplace_back(e.id(), next);
        } else if (src.spatial && src.playing && src.voice != 0) {
            world::EntityId by = 0;
            const bool blocked = place_voice(src.voice, e.id(), src.volume, src.near, src.range, src.occlusion, src.lowpass, &by, src.pitch, src.doppler, dt);
            if (src.occlusion > 0 && blocked != src.occluded) {
                world::AudioSource next = src;
                next.occluded = blocked;
                updates.emplace_back(e.id(), next);
                w.events().emit(clock_.tick, "audio.occluded", e.id(), Json{{"clip", src.clip}, {"voice", src.voice}, {"blocked", blocked}, {"by", by ? w.path(by) : std::string()}}, 0, "audio");
            }
        }
    });
    for (auto& [id, next] : updates) {
        w.ecs().entity(id).set<world::AudioSource>(next);
        if (next.spatial && next.playing && next.voice != 0) place_voice(next.voice, id, next.volume, next.near, next.range, next.occlusion, next.lowpass);
    }
    for (auto& [voice, sp] : spatial_voices_) place_voice(voice, sp.entity, sp.volume, sp.near, sp.range, sp.occlusion, sp.lowpass, nullptr, sp.pitch, sp.doppler, dt);
    for (const audio::VoiceEvent& ev : audio_->tick(dt)) {
        if (ev.type == "audio.ducked") {
            w.events().emit(clock_.tick, ev.type, 0, Json{{"bus", ev.bus}, {"ducked", ev.ducked}, {"by", audio_->bus(ev.bus).duck_by}}, 0, "audio");
            continue;
        }
        if (ev.type == "audio.finished") voice_prev_pos_.erase(ev.voice.id);
        w.events().emit(clock_.tick, ev.type, ev.voice.entity, Json{{"clip", ev.voice.clip}, {"voice", ev.voice.id}, {"loops", ev.voice.loops_done}}, 0, "audio");
        if (ev.type == "audio.finished") spatial_voices_.erase(ev.voice.id);
        if (ev.type == "audio.finished" && ev.voice.entity) {
            flecs::entity e = w.ecs().entity(ev.voice.entity);
            if (e.is_alive() && e.has<world::AudioSource>()) {
                world::AudioSource next = e.get<world::AudioSource>();
                if (next.voice == ev.voice.id) { next.playing = false; next.voice = 0; e.set<world::AudioSource>(next); }
            }
        }
    }
}

Result<Json> Session::ui_command(std::string_view op, const Json& params) {
    if (!ui_) return fail("ui_unavailable", "Pocket UI is disabled: no font was found (run `pocket setup`)");
    ui::Document& d = *ui_;
    // An element given by its name (a Button's or a box's `name`) is the first made with it.
    Json named;
    for (const char* key : {"id", "root"}) {
        if (!params.contains(key) || !params[key].is_string()) continue;
        if (named.is_null()) named = params;
        const std::string want = params[key].get<std::string>();
        const Json found = d.query(Json{{"name", want}});
        if (found.empty()) return fail("ui_no_such_node", "no element named '{}' (ui.query {{name}} finds one, ui.snapshot shows them with their names)", want);
        named[key] = found[0]["id"];
    }
    const Json& p = named.is_null() ? params : named;
    float w = 0, h = 0, scale = 1;
    ui_size(w, h, scale);
    auto layout = [&]() { d.layout(w, h, scale); };
    auto node_id = [&](const char* key) -> ui::NodeId { return p.contains(key) && p[key].is_number() ? p[key].get<ui::NodeId>() : 0; };
    if (op == "apply") {
        POCKET_TRY_VOID(d.apply(p.contains("ops") ? p["ops"] : Json::array()));
        return Json{{"nodes", d.node_count()}};
    }
    if (op == "snapshot") {
        ui::SnapshotOptions so;
        so.root = node_id("root");
        so.depth = opt<int>(p, "depth", -1);
        so.max_nodes = opt<int>(p, "max_nodes", 300);
        so.layout = opt<bool>(p, "layout", true);
        so.styles = opt<bool>(p, "styles", false);
        layout();
        return Json{{"text", d.snapshot(so)}, {"nodes", d.node_count()}};
    }
    if (op == "query") { layout(); return d.query(p); }
    if (op == "describe") { layout(); return d.describe(node_id("id")); }
    if (op == "hit") {
        layout();
        ui::NodeId id = d.hit_test(static_cast<float>(opt<double>(p, "x", 0)), static_cast<float>(opt<double>(p, "y", 0)));
        Json j;
        j["id"] = id;
        if (id) j["node"] = d.describe(id);
        return j;
    }
    if (op == "focus") { d.set_focus(node_id("id")); return Json{{"focused", d.focused()}}; }
    if (op == "stats") { layout(); return d.stats(); }
    // Synthetic input: the same path real input takes, so agents can drive the interface.
    auto point = [&](float& x, float& y) -> Status {
        layout();
        if (ui::NodeId id = node_id("id")) {
            if (!d.exists(id)) return fail("ui_no_such_node", "node {} does not exist", id);
            ui::Rect r = d.rect_of(id);
            x = r.x + r.w * 0.5f;
            y = r.y + r.h * 0.5f;
            return {};
        }
        if (!p.contains("x") || !p.contains("y")) return fail("bad_args", "give an element id or x/y in points");
        x = static_cast<float>(p["x"].get<double>());
        y = static_cast<float>(p["y"].get<double>());
        return {};
    };
    if (op == "click") {
        float x = 0, y = 0;
        POCKET_TRY_VOID(point(x, y));
        int button = opt<int>(p, "button", 1);
        platform::Event move; move.type = platform::EventType::MouseMove; move.x = x; move.y = y;
        platform::Event down; down.type = platform::EventType::MouseDown; down.x = x; down.y = y; down.button = button;
        down.mods = p.contains("mods") ? platform::mods_from_json(p["mods"]) : 0;
        down.clicks = std::clamp(opt<int>(p, "clicks", 1), 1, 3);   // 2 selects the word under the point in an input, 3 the line
        platform::Event up = down; up.type = platform::EventType::MouseUp;
        return inject_events({move, down, up});
    }
    if (op == "drag") {
        // Press at the element (or point), move by dx/dy in a few steps, release.
        float x = 0, y = 0;
        POCKET_TRY_VOID(point(x, y));
        float dx = static_cast<float>(opt<double>(p, "dx", 0)), dy = static_cast<float>(opt<double>(p, "dy", 0));
        int steps = std::clamp(opt<int>(p, "steps", 4), 1, 64);
        std::vector<platform::Event> evs;
        platform::Event move; move.type = platform::EventType::MouseMove; move.x = x; move.y = y;
        evs.push_back(move);
        platform::Event down; down.type = platform::EventType::MouseDown; down.x = x; down.y = y; down.button = opt<int>(p, "button", 1);
        evs.push_back(down);
        for (int i = 1; i <= steps; ++i) {
            platform::Event m; m.type = platform::EventType::MouseMove;
            m.x = x + dx * static_cast<float>(i) / static_cast<float>(steps);
            m.y = y + dy * static_cast<float>(i) / static_cast<float>(steps);
            m.dx = dx / static_cast<float>(steps);
            m.dy = dy / static_cast<float>(steps);
            evs.push_back(m);
        }
        platform::Event up = down; up.type = platform::EventType::MouseUp; up.x = x + dx; up.y = y + dy;
        evs.push_back(up);
        return inject_events(std::move(evs));
    }
    if (op == "wheel") {
        float x = 0, y = 0;
        POCKET_TRY_VOID(point(x, y));
        platform::Event move; move.type = platform::EventType::MouseMove; move.x = x; move.y = y;
        platform::Event wheel; wheel.type = platform::EventType::MouseWheel; wheel.dx = static_cast<float>(opt<double>(p, "dx", 0)); wheel.dy = static_cast<float>(opt<double>(p, "dy", -1));
        return inject_events({move, wheel});
    }
    if (op == "type") {
        std::string text = opt<std::string>(p, "text", "");
        if (text.empty()) return fail("bad_args", "type needs text");
        platform::Event t; t.type = platform::EventType::Text; t.text = text;
        return inject_events({t});
    }
    if (op == "key") {
        std::string key = opt<std::string>(p, "key", "");
        if (key.empty()) return fail("bad_args", "key needs a key name such as Return, Backspace, Left");
        platform::Event down; down.type = platform::EventType::KeyDown; down.key_name = key;
        down.mods = p.contains("mods") ? platform::mods_from_json(p["mods"]) : 0;
        platform::Event up = down; up.type = platform::EventType::KeyUp;
        return inject_events({down, up});
    }
    return fail("unknown_command", "unknown ui command '{}'", op);
}

Result<Json> Session::script_command(std::string_view op, const Json& p) {
    if (op == "contexts") return Json(bundle_names_);
    if (op == "diagnostics") {
        // The project's type errors: set by `pocket run/editor --watch` after each rebundle (the
        // runtime does not type-check), read by the editor's Script tab and by agents. Each set is
        // also logged, so the Console shows it.
        if (p.contains("diagnostics")) {
            if (!p["diagnostics"].is_array()) return fail("bad_args", "diagnostics is an array of {{file, line, column, severity, message}}");
            script_diagnostics_ = p["diagnostics"];
            if (script_diagnostics_.empty()) log::info("types", "no type errors");
            for (const Json& d : script_diagnostics_) {
                log::warn("types", "{}:{}:{}: {}", opt<std::string>(d, "file", "?"), opt<int>(d, "line", 0), opt<int>(d, "column", 0), opt<std::string>(d, "message", ""));
            }
        }
        return Json{{"diagnostics", script_diagnostics_}, {"count", script_diagnostics_.size()}};
    }
    if (op == "start") {
        std::string name = opt<std::string>(p, "name", "project");
        if (std::find(bundle_names_.begin(), bundle_names_.end(), name) == bundle_names_.end()) return fail("bad_args", "unknown script context '{}'", name);
        dispatch("start", Json::object(), name);
        host_->drain_microtasks();
        return Json{{"name", name}, {"ok", errors_.empty()}};
    }
    if (op == "reload") {
        // Drop the context's handlers, re-evaluate its bundle from disk and (unless start is
        // false) start it again. The world is untouched: callers reload a scene first when they
        // want a fresh start.
        std::string name = opt<std::string>(p, "name", "project");
        bool start = opt<bool>(p, "start", true);
        std::filesystem::path path = name == "editor" ? options_.editor_bundle : name == "scenario" ? options_.scenario_bundle : options_.bundle;
        if (name != "editor" && name != "project" && name != "scenario") return fail("bad_args", "unknown script context '{}'", name);
        if (path.empty()) return fail("bad_args", "no bundle for context '{}'", name);
        // The errors go first: dispatch runs nothing while a script error stands, and the old
        // handlers must be dropped, or the new bundle's would join them (the old tick still throwing).
        errors_.clear();
        dispatch("unload", name, name);
        if (auto r = load_bundle(path, name); !r) { record_error(r.error()); return fail(r.error()); }
        has_dispatch_ = host_->has_function("__pocket_dispatch");
        if (start) dispatch("start", Json::object(), name);
        host_->drain_microtasks();
        Json j;
        j["name"] = name;
        j["bundle"] = path.string();
        j["started"] = start;
        j["ok"] = errors_.empty();
        return j;
    }
    if (op == "profile") {
        // Where the scripts' time goes since the profile last started over: each handler (with the
        // file and line it was registered at) and each command the scripts called.
        const bool reset = opt<bool>(p, "reset", false);
        const std::int64_t ticks = clock_.tick - profile_from_tick_;
        const std::uint64_t frames = frames_ - profile_from_frame_;
        Json handlers = Json::array();
        double total = 0;
        if (has_dispatch_) {
            auto r = host_->call("__pocket_dispatch", Json::array({"profile", Json{{"reset", reset}}}));
            if (r && r->is_array()) handlers = std::move(*r);
        }
        for (const Json& h : handlers) total += h.value("ms", 0.0);
        auto round3 = [](double v) { return std::round(v * 1000.0) / 1000.0; };
        for (Json& h : handlers) {
            const double ms = h.value("ms", 0.0);
            const auto calls = h.value("calls", std::uint64_t{1});
            h["at"] = source_lines(h.value("at", std::string()));
            h["ms"] = round3(ms);
            h["max_ms"] = round3(h.value("max", 0.0));
            h.erase("max");
            h["ms_per_call"] = round3(ms / static_cast<double>(std::max<std::uint64_t>(calls, 1)));
            h["share"] = total > 0 ? std::round(ms / total * 100.0) / 100.0 : 0.0;
        }
        std::vector<std::pair<std::string, CallCost>> calls(script_calls_.begin(), script_calls_.end());
        std::sort(calls.begin(), calls.end(), [](const auto& a, const auto& b) { return a.second.ms > b.second.ms; });
        Json commands = Json::array();
        const double per = static_cast<double>(std::max<std::int64_t>(ticks, 1));
        for (const auto& [method, c] : calls) {
            commands.push_back(Json{{"method", method}, {"calls", c.calls}, {"ms", round3(c.ms)}, {"calls_per_tick", std::round(static_cast<double>(c.calls) / per * 10.0) / 10.0}});
        }
        Json j;
        j["ticks"] = ticks;
        j["frames"] = frames;
        j["script_ms"] = round3(total);
        j["script_ms_per_tick"] = round3(total / per);
        j["handlers"] = std::move(handlers);
        j["commands"] = std::move(commands);
        j["component_reads"] = script_numbers_[0];
        j["component_writes"] = script_numbers_[1];
        if (reset) {
            script_calls_.clear();
            script_numbers_[0] = script_numbers_[1] = 0;
            profile_from_tick_ = clock_.tick;
            profile_from_frame_ = frames_;
        }
        return j;
    }
    if (op == "eval") {
        std::string source = opt<std::string>(p, "source", "");
        if (source.empty()) return fail("bad_args", "eval needs source");
        // Evaluated by a helper installed before the bundles so results come back as JSON.
        POCKET_TRY(v, host_->call("__pocket_eval", Json::array({source})));
        host_->drain_microtasks();
        if (v.is_object() && v.contains("error") && v["error"].is_string()) v["error"] = source_lines(v["error"].get<std::string>());
        return v;
    }
    return fail("unknown_command", "unknown script command '{}'", op);
}

Result<Json> Session::project_command(std::string_view op, const Json& p) {
    auto inside_project = [&](const std::string& rel) { return inside_dir(options_.project_dir, rel); };
    if (op == "info") {
        Json j;
        j["name"] = name_;
        j["dir"] = options_.project_dir.string();
        j["bundle"] = options_.bundle.string();
        j["scene"] = project_.contains("scene") && project_["scene"].is_string() ? project_["scene"] : Json(nullptr);
        j["font"] = font_path_.string();
        if (!named_fonts_.empty()) {
            Json fonts = Json::object();
            for (const NamedFont& f : named_fonts_) fonts[f.name] = f.path;
            j["fonts"] = fonts;
        }
        j["editor_bundle"] = options_.editor_bundle.string();
        j["contexts"] = bundle_names_;
        j["headless"] = options_.headless;
        j["window"] = Json{{"width", device_->width()}, {"height", device_->height()}};
        return j;
    }
    if (op == "brief") {
        // What an agent needs first, in a screenful of text: the files, the scene's top, the
        // components in use and the project's own, the actions, the exposed state, what has
        // happened, what is wrong, and where to look next.
        std::string t;
        auto line = [&](const std::string& s) { t += s; t += '\n'; };
        auto join = [](const std::vector<std::string>& v, std::size_t most) {
            std::string s;
            for (std::size_t i = 0; i < v.size() && i < most; ++i) s += (i ? ", " : "") + v[i];
            if (v.size() > most) s += std::format(", +{} more", v.size() - most);
            return s;
        };
        line(std::format("project {} ({}), tick {} ({} ticks a second, {:.4g} s each), {}, seed {}{}", name_, options_.project_dir.filename().string(), clock_.tick, std::lround(1.0 / clock_.tick_seconds), clock_.tick_seconds, paused_ ? "paused" : "running", options_.seed, options_.headless ? ", headless" : ""));
        // Files by kind, the build's and the tools' left out.
        std::map<std::string, std::vector<std::string>> files;
        std::error_code ec;
        for (auto it = std::filesystem::recursive_directory_iterator(options_.project_dir, ec); !ec && it != std::filesystem::recursive_directory_iterator(); it.increment(ec)) {
            const std::string fn = it->path().filename().string();
            if (it->is_directory() && (fn == "build" || fn == "dist" || fn == "node_modules" || fn.starts_with("."))) { it.disable_recursion_pending(); continue; }
            if (!it->is_regular_file()) continue;
            const std::string rel = std::filesystem::relative(it->path(), options_.project_dir, ec).generic_string();
            const std::string top = rel.find('/') == std::string::npos ? "." : rel.substr(0, rel.find('/'));
            files[top].push_back(rel);
        }
        for (auto& [dir, list] : files) {
            std::sort(list.begin(), list.end());
            if (dir == "." || dir == "scripts" || dir == "scenarios" || dir == "prefabs" || dir == "benches") line(std::format("  {}: {}", dir == "." ? "files" : dir, join(list, 12)));
            else line(std::format("  {}/: {} files", dir, list.size()));
        }
        // The scene's roots, with how much hangs under each.
        std::vector<std::string> roots;
        for (world::EntityId r : world_->roots()) {
            std::size_t under = 0;
            std::function<void(world::EntityId)> count = [&](world::EntityId e) { for (world::EntityId c : world_->children(e)) { ++under; count(c); } };
            count(r);
            roots.push_back(under ? std::format("{} (+{})", world_->name(r), under) : world_->name(r));
        }
        const Json summary = world_->summary();
        line(std::format("world: {} entities; roots: {}", summary.value("entities", 0), join(roots, 12)));
        std::vector<std::pair<int, std::string>> used;
        for (const auto& [c, n] : summary.value("components", Json::object()).items()) if (c != "WorldTransform" && c != "Bounds") used.emplace_back(n.get<int>(), c);
        std::sort(used.begin(), used.end(), [](const auto& a, const auto& b) { return a.first != b.first ? a.first > b.first : a.second < b.second; });
        std::vector<std::string> used_text;
        for (const auto& [n, c] : used) used_text.push_back(std::format("{} {}", c, n));
        line("components in use: " + join(used_text, 16));
        // The project's own components (components.toml), with their fields.
        for (const auto& info : world_->component_infos_all()) {
            if (!world_->project_component(info.name)) continue;
            std::vector<std::string> fields;
            for (const auto& f : info.fields) {
                std::string names;
                for (std::string_view n : f.names) names += (names.empty() ? ": " : "|") + std::string(n);
                fields.push_back(std::format("{} {}{}", f.name, f.type, names));
            }
            line(std::format("project component {} {{{}}}", info.name, join(fields, 12)));
        }
        std::vector<std::string> actions;
        for (const auto& [a, spec] : input_map_.describe().items()) {
            std::vector<std::string> keys;
            for (const char* side : {"positive", "negative", "axis"}) for (const Json& k : spec.value(side, Json::array())) keys.push_back(k.get<std::string>());
            actions.push_back(std::format("{} ({})", a, join(keys, 4)));
        }
        if (!actions.empty()) line("input actions: " + join(actions, 10));
        std::vector<std::string> state;
        if (last_state_.is_object()) for (const auto& [k, v] : last_state_.items()) state.push_back(std::format("{}={}", k, v.dump()));
        line(state.empty() ? "exposed state: none (scripts expose values with expose())" : "exposed state: " + join(state, 16));
        std::vector<std::pair<std::uint64_t, std::string>> kinds;
        for (const auto& [k, v] : world_->events().histogram().items()) kinds.emplace_back(v.get<std::uint64_t>(), k);
        std::sort(kinds.begin(), kinds.end(), [](const auto& a, const auto& b) { return a.first != b.first ? a.first > b.first : a.second < b.second; });
        std::vector<std::string> kinds_text;
        for (const auto& [n, k] : kinds) kinds_text.push_back(std::format("{} x{}", k, n));
        line(std::format("events: {} so far; {}", world_->events().total(), join(kinds_text, 10)));
        POCKET_TRY(lint, world_lint(Json{{"limit", 3}}));
        std::string wrong = std::format("lint: {} errors, {} warnings", lint.value("errors", 0), lint.value("warnings", 0));
        for (const Json& pr : lint.value("problems", Json::array())) {
            if (pr.value("severity", "") == "info") continue;
            wrong += std::format("; {}: {}", pr.value("path", pr.value("component", "")), pr.value("problem", ""));
        }
        line(wrong);
        if (!script_diagnostics_.empty()) line(std::format("type errors: {} (script.diagnostics lists them)", script_diagnostics_.size()));
        // A slow tick said where it shows, with where to look (a sixtieth of a second is the budget).
        if (perf_tick_.samples >= 10) {
            const double avg = perf_tick_.total_ms / static_cast<double>(perf_tick_.samples);
            if (avg > 4.0) {
                const double script = perf_script_.samples ? perf_script_.total_ms / static_cast<double>(perf_script_.samples) : 0.0;
                const double state = perf_state_.samples ? perf_state_.total_ms / static_cast<double>(perf_state_.samples) : 0.0;
                int top = 0;
                double top_ms = 0;
                for (int i = 0; i < static_cast<int>(System::Count); ++i) {
                    const PhaseStats& s = perf_systems_[i];
                    const double ms = s.samples ? s.total_ms / static_cast<double>(s.samples) : 0.0;
                    if (ms > top_ms) { top_ms = ms; top = i; }
                }
                std::vector<std::string> parts;   // what is a tenth of it or more
                if (script >= avg * 0.1) parts.push_back(std::format("{:.1f} the scripts' handlers", script));
                if (state >= avg * 0.1) parts.push_back(std::format("{:.1f} reading their exposed values", state));
                if (top_ms >= avg * 0.1) parts.push_back(std::format("{:.1f} {}", top_ms, kSystemNames[top]));
                line(std::format("slow: a tick takes {:.1f} ms on average{}{}; perf breaks it down by system, script.profile the scripts' part by handler and command", avg, parts.empty() ? "" : ": ", join(parts, 3)));
            }
        }
        line("next: world.tree {depth}, world.query {with}, world.describe {entity}, transcript, help {command}, commands {family | search, text: true}; edit scripts then project.apply");
        return Json{{"text", t}};
    }
    if (op == "reload") {
        // Hot reload: the scene from disk and/or the project's scripts. A running project starts
        // again; one the editor holds dormant stays dormant. `pocket run --watch` calls this.
        bool scene = opt<bool>(p, "scene", true);
        bool scripts = opt<bool>(p, "scripts", true);
        const bool settings = opt<bool>(p, "settings", scripts);
        bool was_active = context_active("project");
        Json j;
        if (settings && std::filesystem::exists(options_.project_config)) {
            // project.toml as the tool last bundled it: an edited input map, bus or render
            // setting takes effect without a restart.
            auto text = fs::read_text(options_.project_config);
            Json fresh = text ? Json::parse(*text, nullptr, false) : Json();
            if (!text || fresh.is_discarded() || !fresh.is_object()) return fail("bad_project_config", "{} is not valid JSON", options_.project_config.string());
            project_ = std::move(fresh);
            apply_project_settings();
        }
        timelines_->forget();
        j["settings"] = settings;
        if (scripts) {
            // Errors first, so the unload runs (see script.reload).
            errors_.clear();
            dispatch("unload", "project", "project");
            if (auto r = load_bundle(options_.bundle, "project"); !r) { record_error(r.error()); return fail(r.error()); }
            has_dispatch_ = host_->has_function("__pocket_dispatch");
        }
        if (scene) {
            // A fresh world: the scene file when there is one, otherwise empty, so a restarted
            // project spawns into what it expects instead of on top of its previous run.
            world_->clear();
            if (auto r = load_scene_file(); !r) { record_error(r.error()); return fail(r.error()); }
            update_terrains();
            j["entities"] = world_->entity_count();
        }
        bool start = scripts && (was_active || options_.editor_bundle.empty());
        if (start && scene) {
            // A restart plays as a fresh run would: random() and Math.random from the run's seed
            // again, so a game edited and reloaded meets the same rocks, cards and rolls; and in
            // silence, the last run's sounds stopped (the scene's own start again).
            rng_.reseed(options_.seed);
            seed_math_random();
            // And on a clock of its own from tick 0: what the scripts see as the tick and the time,
            // and the wind, waves and cloth.
            run_start_ = clock_.tick;
            world_->set_run_start(run_start_);
            if (audio_) audio_->stop_all();
            // Nothing held over either: an action an agent held for the last run is let go.
            for (auto& [key, until] : held_keys_) until = clock_.tick;
            release_expired_holds();
            pending_holds_.clear();
        }
        if (start) dispatch("start", Json::object(), "project");
        host_->drain_microtasks();
        world_->events().emit(clock_.tick, "project.reloaded", 0, Json{{"scene", scene}, {"scripts", scripts}, {"started", start}}, 0, "runtime");
        log::info("runtime", "project reloaded (scene {}, scripts {}, started {})", scene, scripts, start);
        j["scene"] = scene;
        j["scripts"] = scripts;
        j["started"] = start;
        j["ok"] = errors_.empty();
        // Sources edited since the bundle was made are not in what was just loaded.
        if (scripts) {
            Json stale = Json::array();
            std::error_code ec;
            const auto made = std::filesystem::last_write_time(options_.bundle, ec);
            if (!ec) {
                auto newer = [&](const std::filesystem::path& f) {
                    std::error_code e2;
                    const auto t = std::filesystem::last_write_time(f, e2);
                    if (!e2 && t > made) stale.push_back(std::filesystem::relative(f, options_.project_dir, e2).generic_string());
                };
                newer(options_.project_dir / "project.toml");
                newer(options_.project_dir / "components.toml");
                for (auto it = std::filesystem::recursive_directory_iterator(options_.project_dir / "scripts", ec); !ec && it != std::filesystem::recursive_directory_iterator(); it.increment(ec)) {
                    const std::string ext = it->path().extension().string();
                    if (it->is_regular_file() && (ext == ".ts" || ext == ".tsx" || ext == ".js")) newer(it->path());
                }
            }
            if (!stale.empty()) {
                j["stale"] = stale;
                j["hint"] = "these sources are newer than the bundle just loaded: project.apply bundles them and reloads";
            }
        }
        return j;
    }
    if (op == "apply") {
        // Edit, then one call (docs/mcp.md, Applying an edit): the project's scripts bundled again
        // by the pocket tool, type-checked, the project reloaded and stepped, and what came of it.
        return apply_project(opt<int>(p, "ticks", 1));
    }
    if (op == "save_scene") {
        std::string rel = opt<std::string>(p, "path", project_.contains("scene") && project_["scene"].is_string() ? project_["scene"].get<std::string>() : "");
        if (rel.empty()) return fail("bad_args", "the project has no scene file; give a path");
        POCKET_TRY(full, inside_project(rel));
        Json scene = world_->save();
        std::filesystem::create_directories(full.parent_path());
        POCKET_TRY_VOID(fs::write_text(full, scene.dump(2) + "\n"));
        world_->events().emit(clock_.tick, "scene.saved", 0, Json{{"path", rel}, {"entities", world_->entity_count()}}, 0, "editor");
        return Json{{"path", full.string()}, {"entities", world_->entity_count()}};
    }
    if (op == "write") {
        POCKET_TRY(full, inside_project(opt<std::string>(p, "path", "")));
        std::string text = p.contains("text") && p["text"].is_string() ? p["text"].get<std::string>() : (p.contains("json") ? p["json"].dump(2) + "\n" : "");
        std::filesystem::create_directories(full.parent_path());
        POCKET_TRY_VOID(fs::write_text(full, text));
        if (opt<std::string>(p, "path", "").starts_with("locales/")) ++locale_rev_;   // scripts read the language files again
        return Json{{"path", full.string()}, {"bytes", text.size()}};
    }
    if (op == "read") {
        POCKET_TRY(full, inside_project(opt<std::string>(p, "path", "")));
        POCKET_TRY(text, fs::read_text(full));
        return Json{{"path", full.string()}, {"text", text}};
    }
    return fail("unknown_command", "unknown project command '{}'", op);
}

Status Session::frame() {
    if (!started_) return fail("not_started", "Session::start() was not called");
    // A window is not drawable until the OS maps it. Until the first present succeeds (or a
    // grace period passes), keep rendering but do not count frames or accumulate simulation
    // time, so frame budgets and real-time ticks start when something is visible.
    if (!warmed_up_ && device_->has_surface() && !(journal_ && journal_->replaying)) {
        if (device_->presented_frames() > 0 || warmup_timer_.seconds() > 0.5) {
            warmed_up_ = true;
            frame_timer_.lap();
        } else {
            (void)platform_->poll();
            if (auto r = render_frame(); !r) { record_error(r.error()); return fail(r.error()); }
#ifndef __EMSCRIPTEN__
            std::this_thread::sleep_for(std::chrono::milliseconds(2));
#endif
            return {};
        }
    }
    // A lockstep game that has not started (players still joining) draws and waits; those frames do
    // not count against a frame budget.
    if (net_ && !net_->started()) {
        (void)platform_->poll();
        net_pump();
        if (!options_.headless) {
            if (auto r = render_frame(); !r) { record_error(r.error()); return fail(r.error()); }
        }
#ifndef __EMSCRIPTEN__
        std::this_thread::sleep_for(std::chrono::milliseconds(options_.headless ? 1 : 2));
#endif
        return {};
    }
    Stopwatch frame_sw;
    Json input_events = Json::array();
    int ticks = 1;
    Stopwatch poll_sw;
    POCKET_TRY(has_frame, poll_input(input_events, ticks, true));
    perf_poll_.add(poll_sw.ms());
    if (!has_frame) return {};
    if (platform_->quit_requested()) return {};
    if (!input_events.empty()) dispatch("input", input_events);
    // A player who came back into a running game replays what it missed as fast as it goes.
    if (net_ && net_->catching_up()) ticks = 1000000;
    POCKET_TRY_VOID(run_ticks(ticks));
    dispatch("frame", frame_info());
    host_->drain_microtasks();
    std::uint64_t presented_before = device_->presented_frames();
    // --render last: a headless run with a frame budget draws only its last frame.
    const bool skip = skip_render_ || (options_.render_last && options_.headless && options_.frames > 0 && frames_ + 1 < static_cast<std::uint64_t>(options_.frames));
    if (skip) {
        frame_stale_ = true;
        // Not drawn (a headless step's earlier ticks): what drawing does besides the GPU's work.
        if (audio_) audio_->pump();
        update_terrains();
        if (ui_ && ui_->node_count() > 1) {
            float w = 0, h = 0, scale = 1;
            ui_size(w, h, scale);
            ui_->layout(w, h, scale);
        }
    } else if (errors_.empty()) {
        if (frame_stale_) renderer_->cut();
        frame_stale_ = false;
        Stopwatch render_sw;
        auto r = render_frame();
        perf_render_.add(render_sw.ms());
        if (!r) {
            record_error(r.error());
            return fail(r.error());
        }
    }
    frames_++;
    perf_frame_.add(frame_sw.ms());
#ifndef __EMSCRIPTEN__
    if (device_->has_surface() && device_->presented_frames() == presented_before) {
        // Occluded or hidden window: nothing throttles the loop, so pace it at the tick rate to
        // keep real-time simulation sensible and the CPU idle.
        double spent = pace_timer_.seconds();
        double budget = clock_.tick_seconds;
        if (spent < budget) std::this_thread::sleep_for(std::chrono::duration<double>(budget - spent));
    }
#else
    (void)presented_before;
#endif
    pace_timer_.lap();
    // A headless run without a frame budget and without a controller would run forever: stop it
    // after a minute of simulated time. A served run belongs to its controller (`quit` ends it).
    if (options_.headless && options_.frames < 0 && options_.serve < 0 && frames_ >= 3600) quit_ = true;
    return {};
}

Status Session::finish() {
    if (stopped_) return {};
    stopped_ = true;
    dispatch("stop", Json::object());
    if (!options_.capture.empty()) {
        auto r = command("capture", Json{{"path", std::filesystem::absolute(options_.capture).string()}}, "runtime");   // a command line's path is from where it ran
        if (!r) record_error(r.error());
    }
    if (journal_) {
        if (auto r = journal_->close(); !r) record_error(r.error());
    }
    log::global().set_tick(-1);
    return {};
}

world::EntityId Session::resolve_entity(const Json& v) const {
    if (v.is_number_unsigned() || v.is_number_integer()) return v.get<world::EntityId>();
    if (v.is_number()) return static_cast<world::EntityId>(v.get<double>());
    if (v.is_string()) {
        const std::string s = v.get<std::string>();
        if (const world::EntityId id = world_->find(s)) return id;
        // An id written as a string ("492"), when no entity is named so.
        if (!s.empty() && s.size() <= 20 && std::all_of(s.begin(), s.end(), [](char c) { return c >= '0' && c <= '9'; })) {
            const world::EntityId id = std::stoull(s);
            if (world_->alive(id)) return id;
        }
    }
    return 0;
}

// What is likely wrong with the world as it stands (docs/design/world-model.md, Linting): parts
// that do nothing together, files that are not there, names that name nothing. Each problem says
// what it does to the game and what would fix it, for an agent to act on.
Result<Json> Session::world_lint(const Json& p) {
    const std::size_t limit = static_cast<std::size_t>(std::clamp(opt<int>(p, "limit", 200), 1, 5000));
    Json problems = Json::array();
    std::map<std::string, int> counts;
    auto add = [&](const char* severity, world::EntityId id, const char* component, std::string problem, std::string fix) {
        counts[severity]++;
        if (problems.size() >= limit) return;
        Json j{{"severity", severity}, {"component", component}, {"problem", std::move(problem)}, {"fix", std::move(fix)}};
        if (id) {
            j["entity"] = id;
            j["path"] = world_->path(id);
        }
        problems.push_back(std::move(j));
    };
    // A project file named by a field: whether it is there.
    auto file_missing = [&](const std::string& rel) {
        if (rel.empty() || rel.starts_with("pattern:") || rel.starts_with("sfx:")) return false;   // made by the engine; a bad pattern is said below
        auto full = inside_dir(options_.project_dir, rel);
        std::error_code ec;
        return !full || !std::filesystem::exists(*full, ec);
    };
    auto names_nothing = [&](const std::string& name) { return !name.empty() && resolve_entity(Json(name)) == 0; };
    auto& ecs = world_->ecs();
    // Physics: parts that need each other.
    ecs.each([&](flecs::entity e, const world::Collider& col) {
        if (!e.has<world::RigidBody>() && !e.has<world::Character>()) add("warning", e.id(), "Collider", "a Collider without a RigidBody is ignored by the physics: nothing stands on it or hits it", "add a RigidBody (kind 1 for something that never moves) or remove the Collider");
        // A box or sphere collider far from the size of the cube or sphere drawn with it: Collider.size
        // is half extents in world units, which the Transform's scale does not stretch.
        if ((col.shape == 0 || col.shape == 1) && !col.is_trigger) {
            const auto* mr = e.try_get<world::MeshRenderer>();
            const auto* tr = e.try_get<world::Transform>();
            if (mr && tr && (mr->mesh == (col.shape == 0 ? "cube" : "sphere"))) {
                const Vec3 drawn{std::fabs(tr->scale.x) * 0.5f, std::fabs(tr->scale.y) * 0.5f, std::fabs(tr->scale.z) * 0.5f};
                const Vec3 shape = col.shape == 0 ? col.size : Vec3{col.size.x, col.size.x, col.size.x};
                bool off = false;
                for (int k = 0; k < 3; ++k) {
                    const float d = (&drawn.x)[k], s = (&shape.x)[k];
                    if (d > 0 && (s < d / 3 || s > d * 3)) off = true;
                }
                if (off) add("warning", e.id(), "Collider", std::format("its collider ({} {:.2f} x {:.2f} x {:.2f} half extents) is far from the {} drawn ({:.2f} x {:.2f} x {:.2f}): Collider.size is in world units and the Transform's scale does not stretch it", col.shape == 0 ? "box" : "sphere", shape.x, shape.y, shape.z, mr->mesh, drawn.x, drawn.y, drawn.z), std::format("set Collider.size to {{x: {:.2f}, y: {:.2f}, z: {:.2f}}}", drawn.x, drawn.y, drawn.z));
            }
        }
        if (col.shape == 3 && !col.mesh.empty() && file_missing(col.mesh)) add("error", e.id(), "Collider", std::format("the mesh collider's file {} is not in the project", col.mesh), "import it (assets.import) or point Collider.mesh at a file that is there");
    });
    ecs.each([&](flecs::entity e, const world::Behavior& b) {
        if (!b.error.empty()) add("warning", e.id(), "Behavior", b.error, "docs/design/behavior.md says what a Behavior reads and needs");
    });
    ecs.each([&](flecs::entity e, const world::RigidBody& rb) {
        if (!e.has<world::Collider>()) add("warning", e.id(), "RigidBody", rb.kind == 0 ? "a dynamic RigidBody without a Collider falls through everything and nothing can touch it" : "a RigidBody without a Collider does nothing", "add a Collider (a box, sphere or capsule the size of what is drawn)");
        if (rb.kind == 0 && !(rb.mass > 0)) add("error", e.id(), "RigidBody", "a dynamic body with no mass cannot be simulated", "give RigidBody.mass a value above 0 (1 for a crate)");
        if (rb.kind == 0 && e.has<world::Character>()) add("warning", e.id(), "RigidBody", "a Character moves itself; a dynamic RigidBody on it fights that", "remove the RigidBody, or make it kinematic (kind 2)");
    });
    ecs.each([&](flecs::entity e, const world::Transform& t) {
        if (t.scale.x == 0 || t.scale.y == 0 || t.scale.z == 0) add("warning", e.id(), "Transform", "a scale of 0 along an axis makes it flat: not drawn, and its collider has no size", "set every scale component above 0");
        if (std::fabs(t.position.x) > 1e5f || std::fabs(t.position.y) > 1e5f || std::fabs(t.position.z) > 1e5f) add("warning", e.id(), "Transform", "this far from the origin floats lose precision: it jitters and collides roughly", "keep the game within some ten thousand units of the origin");
    });
    // Files the scene names.
    ecs.each([&](flecs::entity e, const world::MeshRenderer& mr) {
        const bool primitive = mr.mesh.empty() || mr.mesh == "cube" || mr.mesh == "sphere" || mr.mesh == "plane" || mr.mesh == "cylinder" || mr.mesh == "quad" || mr.mesh == "capsule";
        const bool builtin = mr.mesh == "humanoid" || mr.mesh.starts_with("humanoid?") || assets::is_prop(mr.mesh);
        if (!primitive && !builtin && !e.has<world::Terrain>() && file_missing(mr.mesh)) add("error", e.id(), "MeshRenderer", std::format("the mesh file {} is not in the project, so nothing is drawn", mr.mesh), "import it (assets.import), fix the path, or use a primitive (cube, sphere, plane, cylinder, quad, capsule), the built-in humanoid or a prop (tree, pine, rock, bush, barrel, lamp, fence, house, crate, chest, torch, bench, table, chair, well, sign, tower, crop)");
        if (mr.mesh.starts_with("humanoid?") && assets_) {
            if (auto m = assets_->mesh(mr.mesh); !m) add("error", e.id(), "MeshRenderer", m.error().message, "skin, shirt, trousers, shoes and hair take \"#rrggbb\" or a colour's name; hair=none leaves the head bare");
        }
        if (assets::is_prop(mr.mesh) && mr.mesh.find('?') != std::string::npos && assets_) {
            if (auto m = assets_->mesh(mr.mesh); !m) add("error", e.id(), "MeshRenderer", m.error().message, "a prop's settings are its sizes, colours (\"#rrggbb\" or a name) and seed (docs/design/assets.md, Props)");
        }
        if (file_missing(mr.texture)) add("error", e.id(), "MeshRenderer", std::format("the texture {} is not in the project", mr.texture), "point MeshRenderer.texture at an image in the project, or clear it");
        if (file_missing(mr.normal_map)) add("error", e.id(), "MeshRenderer", std::format("the normal map {} is not in the project", mr.normal_map), "point MeshRenderer.normal_map at an image in the project, or clear it");
        for (const std::string* image : {&mr.texture, &mr.normal_map}) {
            if (!image->starts_with("pattern:") || !assets_) continue;
            if (auto img = assets_->image(*image); !img) add("error", e.id(), "MeshRenderer", img.error().message, "a pattern is pattern:<name>?<settings>: checker, stripes, grid, bricks, tiles, planks, noise, concrete, sand, dirt, rock, grass or metal (docs/design/assets.md, Patterns)");
        }
    });
    ecs.each([&](flecs::entity e, const world::Sprite& s) {
        if (file_missing(s.texture)) add("error", e.id(), "Sprite", std::format("the sprite's texture {} is not in the project", s.texture), "point Sprite.texture at an image in the project");
    });
    ecs.each([&](flecs::entity e, const world::TileMap& m) {
        if (file_missing(m.map)) add("error", e.id(), "TileMap", std::format("the map file {} is not in the project", m.map), "point TileMap.map at a .tmj file in the project");
    });
    ecs.each([&](flecs::entity e, const world::AudioSource& a) {
        if (file_missing(a.clip)) add("error", e.id(), "AudioSource", std::format("the sound {} is not in the project, so it plays nothing", a.clip), "point AudioSource.clip at a .wav, .ogg, .mp3 or .flac in the project");
    });
    ecs.each([&](flecs::entity e, const world::Timeline& t) {
        if (file_missing(t.path)) add("error", e.id(), "Timeline", std::format("the timeline file {} is not in the project", t.path), "write it (timeline.write, project.write) or point Timeline.path at one");
        else if (!t.error.empty()) add("warning", e.id(), "Timeline", std::format("a track does not apply: {}", t.error), "timeline.info {path} lists every track's problem");
    });
    // Hits and zones (docs/design/combat.md): a hitbox needs something that notices touches.
    ecs.each([&](flecs::entity e, const world::Hitbox& hb) {
        const auto* col = e.try_get<world::Collider>();
        const bool trigger = col && col->is_trigger && e.has<world::RigidBody>();
        const auto* col2 = e.try_get<world::Collider2D>();
        if (!trigger && !e.has<world::Area2D>() && !(col2 && col2->sensor)) add("warning", e.id(), "Hitbox", "a Hitbox notices nothing on its own, so it hurts nothing", "add a trigger Collider (is_trigger, with a static or kinematic RigidBody) for 3D bodies, an Area2D for Body2D and TopDown2D movers, or a sensor Collider2D for 2D rigid bodies");
        if (col2 && !col2->sensor && !col) add("warning", e.id(), "Hitbox", "its Collider2D is solid: bodies stop at it instead of coming in, so it never hits", "set Collider2D.sensor true");
        if (col && !col->is_trigger) add("warning", e.id(), "Hitbox", "its Collider is solid: bodies stop at it instead of coming in, so it never hits", "set Collider.is_trigger true");
        if (hb.damage == 0 && hb.knockback == 0) add("info", e.id(), "Hitbox", "it hits for nothing: no damage and no knockback", "set Hitbox.damage (negative heals)");
    });
    ecs.each([&](flecs::entity e, const world::Attach& a) {
        const world::EntityId target = a.target.empty() ? 0 : world_->find(a.target);
        if (!target) add("warning", e.id(), "Attach", std::format("Attach.target '{}' names no entity, so it stays where it is", a.target), "name the animated entity (its name or path)");
        else if (!world_->try_get<world::MeshRenderer>(target)) add("warning", e.id(), "Attach", std::format("{} draws no model, so it has no joints to hold this at", a.target), "attach to an entity whose MeshRenderer draws a model with that joint");
        else if (!a.found) add("info", e.id(), "Attach", std::format("no joint named '{}' was found on {} (yet)", a.joint, a.target), "animation.pose {entity} lists the joints by name");
    });
    ecs.each([&](flecs::entity e, const world::Area2D& a) {
        if (a.size.x <= 0 || a.size.y <= 0) add("warning", e.id(), "Area2D", "an area with no size notices nothing", "give Area2D.size half extents above 0");
    });
    ecs.each([&](flecs::entity e, const world::MeshRenderer& mr) {
        if (auto it = sprite_material_errors_.find(mr.material); !mr.material.empty() && it != sprite_material_errors_.end())
            add("error", e.id(), "MeshRenderer", std::format("its material {} is drawn plain: {}", mr.material, it->second), "fix the WGSL (it defines fn material(lit: vec4f, s: Surface) -> vec4f), then assets.reload");
    });
    ecs.each([&](flecs::entity e, const world::Sprite& sp) {
        if (auto it = sprite_material_errors_.find(sp.material); !sp.material.empty() && it != sprite_material_errors_.end())
            add("error", e.id(), "Sprite", std::format("its material {} is drawn plain: {}", sp.material, it->second), "fix the WGSL (it defines fn material(texel: vec4f, tint: vec4f, uv: vec2f, params: vec4f, time: f32) -> vec4f), then assets.reload");
        // Light on sprites (docs/design/sprites.md, Light).
        if (!sp.normal_map.empty() && !sp.lit) add("info", e.id(), "Sprite", "a normal map on a sprite that is not lit does nothing", "set Sprite.lit, or leave normal_map empty");
        if (sp.lit && sp.additive) add("info", e.id(), "Sprite", "an additive sprite is a light of its own and is drawn unlit", "leave lit off for glows and flames");
        if (sp.lit && !sp.material.empty()) add("info", e.id(), "Sprite", std::format("a sprite drawn through its material {} is not lit", sp.material), "light it in the material, or drop the material");
    });
    // 2D rigid bodies (docs/design/physics2d.md).
    ecs.each([&](flecs::entity e, const world::RigidBody2D&) {
        if (!e.has<world::Collider2D>()) add("warning", e.id(), "RigidBody2D", "a RigidBody2D with no Collider2D has no shape: it falls through everything and nothing touches it", "add a Collider2D (box, circle, capsule or polygon)");
        if (e.has<world::Body2D>() || e.has<world::TopDown2D>()) add("error", e.id(), "RigidBody2D", "a RigidBody2D and a Body2D or TopDown2D both move the Transform, each its own way", "keep one: RigidBody2D for physics objects, Body2D for a platformer's character");
        if (world_->parent(e.id()) != 0) add("warning", e.id(), "RigidBody2D", "a 2D rigid body under a parent: the step writes its Transform as its place in the world", "make it a root (world.reparent to none)");
    });
    ecs.each([&](flecs::entity e, const world::Joint2D& j) {
        if (!e.has<world::RigidBody2D>()) add("warning", e.id(), "Joint2D", "a Joint2D holds this entity's RigidBody2D, and it has none", "add a RigidBody2D (and a Collider2D) to it");
        if (j.body && !world_->alive(static_cast<world::EntityId>(j.body))) add("warning", e.id(), "Joint2D", "Joint2D.body names an entity that is gone", "set body to another body's entity, or 0 to hold it to the world");
        else if (j.body && !world_->try_get<world::RigidBody2D>(static_cast<world::EntityId>(j.body))) add("warning", e.id(), "Joint2D", "Joint2D.body has no RigidBody2D, so the joint is not made", "give the other entity a RigidBody2D, or set body to 0 for a point in the world");
    });
    ecs.each([&](flecs::entity e, const world::Decal& d) {
        if (file_missing(d.texture)) add("error", e.id(), "Decal", std::format("the decal's image {} is not in the project", d.texture), "point Decal.texture at an image in the project, or clear it for the built-in spot");
    });
    ecs.each([&](flecs::entity e, const world::ParticleEmitter& pe) {
        if (file_missing(pe.texture)) add("error", e.id(), "ParticleEmitter", std::format("the particles' texture {} is not in the project", pe.texture), "point ParticleEmitter.texture at an image in the project, or clear it");
    });
    ecs.each([&](flecs::entity e, const world::Terrain& t) {
        if (file_missing(t.heightmap)) add("error", e.id(), "Terrain", std::format("the heightmap {} is not in the project: the ground is flat", t.heightmap), "save one (terrain.save) or clear Terrain.heightmap for noise");
        if (file_missing(t.paintmap)) add("error", e.id(), "Terrain", std::format("the paint map {} is not in the project", t.paintmap), "save the paint (terrain.save {paint: true}) or clear Terrain.paintmap");
    });
    ecs.each([&](flecs::entity e, const world::Sky& s) {
        if (s.enabled && s.mode == 2 && (s.image.empty() || file_missing(s.image))) add("error", e.id(), "Sky", std::format("the sky's panorama {} is not in the project, so no sky is drawn", s.image), "point Sky.image at an .hdr, .png or .jpg panorama, or use mode 1 or 3");
    });
    // Names that name nothing.
    ecs.each([&](flecs::entity e, const world::Scatter& sc) {
        if (names_nothing(sc.on)) add("error", e.id(), "Scatter", std::format("Scatter.on names {}, which is not in the world", sc.on), "name the terrain or collider to strew over, or clear it for any static collider");
        if (!e.has<world::MeshRenderer>()) add("warning", e.id(), "Scatter", "a Scatter draws its entity's MeshRenderer, and this entity has none", "add a MeshRenderer with the mesh to strew");
    });
    ecs.each([&](flecs::entity e, const world::CameraRig& r) {
        if (names_nothing(r.target)) add("error", e.id(), "CameraRig", std::format("the rig follows {}, which is not in the world", r.target), "name the entity to follow (by name or path)");
        if (!e.has<world::Camera>()) add("warning", e.id(), "CameraRig", "a CameraRig moves a camera, and this entity has no Camera", "put the rig on the camera's entity");
    });
    ecs.each([&](flecs::entity e, const world::Joint& j) {
        if (names_nothing(j.target)) add("error", e.id(), "Joint", std::format("the joint ties to {}, which is not in the world", j.target), "name the other body, or clear the target to tie it to a point in the world");
        if (!e.has<world::RigidBody>()) add("warning", e.id(), "Joint", "a Joint works on a body, and this entity has no RigidBody", "add a RigidBody and a Collider");
    });
    // The view: something must be drawn from a camera.
    int cameras = 0, active = 0, drawn = 0, skies = 0, winds = 0;
    ecs.each([&](flecs::entity, const world::Camera& c) { ++cameras; active += c.active ? 1 : 0; });
    ecs.each([&](flecs::entity, const world::MeshRenderer&) { ++drawn; });
    ecs.each([&](flecs::entity, const world::Sprite&) { ++drawn; });
    ecs.each([&](flecs::entity, const world::Sky& s) { skies += s.enabled ? 1 : 0; });
    ecs.each([&](flecs::entity, const world::Wind& w) { winds += w.enabled ? 1 : 0; });
    if (drawn > 0 && active == 0) add(cameras == 0 ? "warning" : "error", 0, "Camera", cameras == 0 ? "no Camera: the renderer looks from a default place" : "no Camera is active, so none is looked through", cameras == 0 ? "spawn an entity with a Transform and a Camera where the view should be" : "set Camera.active on the one to look through");
    if (active > 1) add("info", 0, "Camera", std::format("{} cameras are active; the first by id is looked through", active), "set Camera.active false on the others");
    if (skies > 1) add("info", 0, "Sky", std::format("{} skies are enabled; only the first by id is drawn", skies), "disable the others");
    if (winds > 1) add("info", 0, "Wind", std::format("{} winds are enabled; only the first by id blows", winds), "disable the others");
    // Scripts that failed.
    for (const Json& err : errors_) add("error", 0, "script", err.is_object() && err.contains("message") && err["message"].is_string() ? err["message"].get<std::string>() : err.dump(), "fix the script (log.tail and script.diagnostics say where)");
    Json j{{"problems", std::move(problems)}, {"errors", counts["error"]}, {"warnings", counts["warning"]}, {"infos", counts["info"]}};
    j["ok"] = counts["error"] == 0;
    return j;
}

// The pocket tool run with arguments (`project.apply`): its exit code and what it printed.
Result<std::pair<int, std::string>> Session::run_tool(const std::vector<std::string>& args) const {
#if defined(__EMSCRIPTEN__) || defined(_WIN32)
    (void)args;
    return fail("unsupported", "running the pocket tool is not available on this platform");
#else
    const char* root = std::getenv("POCKET_ROOT");
    const char* tool_env = std::getenv("POCKET_TOOL");
    const std::string tool = tool_env && *tool_env ? tool_env : root && *root ? (std::filesystem::path(root) / ".pocket" / "pocket").string() : "";
    std::error_code ec;
    if (tool.empty() || !std::filesystem::is_regular_file(tool, ec)) return fail("unavailable", "no pocket tool to bundle with: start the runtime through pocket (pocket run, pocket editor, the MCP server), which sets POCKET_ROOT and POCKET_TOOL");
    std::vector<std::string> all{tool};
    if (root && *root) { all.push_back("--root"); all.push_back(root); }
    all.insert(all.end(), args.begin(), args.end());
    std::vector<char*> argv;
    for (std::string& a : all) argv.push_back(a.data());
    argv.push_back(nullptr);
    const std::filesystem::path log_path = std::filesystem::temp_directory_path(ec) / std::format("pocket-apply-{}.log", static_cast<long>(getpid()));
    const std::string log_file = log_path.string();
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, STDOUT_FILENO, log_file.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
    posix_spawn_file_actions_adddup2(&actions, STDOUT_FILENO, STDERR_FILENO);
    pid_t pid = 0;
    const int rc = posix_spawn(&pid, tool.c_str(), &actions, nullptr, argv.data(), environ);
    posix_spawn_file_actions_destroy(&actions);
    if (rc != 0) return fail("unavailable", "cannot start {}: {}", tool, std::strerror(rc));
    int status = 0;
    while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
    auto text = fs::read_text(log_path);
    std::filesystem::remove(log_path, ec);
    return std::make_pair(WIFEXITED(status) ? WEXITSTATUS(status) : -1, text ? *text : std::string());
#endif
}

Result<Json> Session::apply_project(int ticks) {
    if (ticks < 0 || ticks > 100000) return fail("bad_args", "ticks must be in [0, 100000]");
    if (options_.bundle.empty()) return fail("unavailable", "this runtime was started without a bundle path to write");
    const std::string dir = options_.project_dir.string(), bundle = options_.bundle.string();
    Json j;
    // Bundled: a failure leaves the running project as it is.
    Stopwatch sw;
    POCKET_TRY(bundled, run_tool({"ts", dir, "--out", bundle}));
    j["bundle_ms"] = std::round(sw.ms());
    if (bundled.first != 0) {
        std::string why = bundled.second;
        if (why.size() > 2000) why = why.substr(why.size() - 2000);
        return fail("bundle_failed", "the scripts did not bundle, the running project is unchanged:\n{}", why);
    }
    // Type-checked: the errors go where `pocket run --watch` puts them (script.diagnostics, the
    // editor's Script tab), files relative to the project; they do not stop the reload.
    sw = Stopwatch{};
    POCKET_TRY(checked, run_tool({"--json", "check", dir}));
    j["check_ms"] = std::round(sw.ms());
    Json report;
    if (const std::size_t at = checked.second.find('{'); at != std::string::npos) report = Json::parse(checked.second.substr(at), nullptr, false);
    Json diagnostics = Json::array();
    if (report.is_object() && report.contains("diagnostics") && report["diagnostics"].is_array()) {
        std::error_code ec;
        const std::string prefix = std::filesystem::weakly_canonical(options_.project_dir, ec).generic_string() + "/";
        for (Json d : report["diagnostics"]) {
            if (d.contains("file") && d["file"].is_string()) {
                std::string f = d["file"].get<std::string>();
                const std::string abs = std::filesystem::weakly_canonical(f, ec).generic_string();
                if (abs.starts_with(prefix)) d["file"] = abs.substr(prefix.size());
                else if (const std::size_t s = f.find(options_.project_dir.filename().string() + "/"); s != std::string::npos) d["file"] = f.substr(s + options_.project_dir.filename().string().size() + 1);
            }
            diagnostics.push_back(d);
        }
        (void)script_command("diagnostics", Json{{"diagnostics", diagnostics}});
    }
    j["type_errors"] = diagnostics.size();
    if (!diagnostics.empty()) {
        Json first = Json::array();
        for (std::size_t i = 0; i < std::min<std::size_t>(diagnostics.size(), 10); ++i) first.push_back(diagnostics[i]);
        j["types"] = first;
    }
    POCKET_TRY(reloaded, command("project.reload", Json::object(), "agent"));
    j["reload"] = reloaded;
    if (ticks > 0 && errors_.empty()) {
        POCKET_TRY(state, command("step", Json{{"ticks", ticks}}, "agent"));
        j["state"] = state;
    }
    if (!errors_.empty()) j["script_errors"] = Json(errors_);
    j["ok"] = errors_.empty();
    return j;
}

// A model file's node tree as entity descriptions, the children of the root world.instantiate
// {mesh} makes (docs/design/assets.md, Live models): one per node with the node's
// own transform and, when it carries geometry, a MeshRenderer drawing that node alone.
Result<Json> Session::model_children(const std::string& mesh_path, std::vector<std::string>& warnings) {
    if (!assets_) return fail("no_assets", "no asset store");
    POCKET_TRY(mesh, assets_->mesh(mesh_path));
    if (mesh->skinned()) return fail("unsupported", "{} is skinned: its joints place it, so it stays one drawable", mesh_path);
    const std::size_t count = mesh->nodes.size();
    std::vector<int> uses(count, 0);
    std::map<std::string, int> name_count;
    for (const assets::Node& n : mesh->nodes) name_count[n.name]++;
    for (const assets::Submesh& sm : mesh->submeshes) if (sm.origin >= 0 && static_cast<std::size_t>(sm.origin) < count) uses[static_cast<std::size_t>(sm.origin)]++;
    // A node authored as a matrix has default TRS fields: take the matrix apart.
    std::function<Json(int)> entity_of = [&](int ni) -> Json {
        const assets::Node& n = mesh->nodes[static_cast<std::size_t>(ni)];
        const bool named = !n.name.empty() && name_count[n.name] == 1;
        Json e;
        e["name"] = named ? n.name : (n.name.empty() ? "node" + std::to_string(ni) : n.name + "_" + std::to_string(ni));
        Vec3 t = n.translation, s = n.scale;
        Quat r = n.rotation;
        const bool trs_default = t.x == 0 && t.y == 0 && t.z == 0 && s.x == 1 && s.y == 1 && s.z == 1 && r.x == 0 && r.y == 0 && r.z == 0 && r.w == 1;
        if (trs_default) decompose(n.rest, t, r, s);
        e["components"]["Transform"] = Json{{"position", {{"x", t.x}, {"y", t.y}, {"z", t.z}}}, {"rotation", {{"x", r.x}, {"y", r.y}, {"z", r.z}, {"w", r.w}}}, {"scale", {{"x", s.x}, {"y", s.y}, {"z", s.z}}}};
        if (uses[static_cast<std::size_t>(ni)] > 0) e["components"]["MeshRenderer"] = Json{{"mesh", mesh_path}, {"node", named ? n.name : std::to_string(ni)}};
        // The file's lights and cameras come along: a light keeps its color (encoded for the
        // component's sRGB) and intensity (point and spot lights scaled to the engine's falloff);
        // a camera arrives inactive, so it does not take the view from the scene's own.
        if (n.light >= 0 && static_cast<std::size_t>(n.light) < mesh->lights.size()) {
            const assets::LightDef& l = mesh->lights[static_cast<std::size_t>(n.light)];
            auto enc = [](float c) { c = std::clamp(c, 0.0f, 1.0f); return c <= 0.0031308f ? c * 12.92f : 1.055f * std::pow(c, 1.0f / 2.4f) - 0.055f; };
            const float m = std::max({l.color.x, l.color.y, l.color.z, 1e-6f});
            Json light{{"kind", l.type == 0 ? 0 : l.type == 2 ? 2 : 1}, {"color", {{"r", enc(l.color.x / m)}, {"g", enc(l.color.y / m)}, {"b", enc(l.color.z / m)}, {"a", 1.0}}}};
            if (l.type == 2) {
                light["inner_angle"] = l.inner_cone * 180.0f / 3.14159265f;
                light["outer_angle"] = l.outer_cone * 180.0f / 3.14159265f;
            }
            if (l.type == 0) {
                light["intensity"] = l.intensity * m;
            } else {
                const float k = l.intensity * m / 20.0f;
                light["intensity"] = k;
                light["range"] = l.range > 0 ? l.range : std::max(5.0f, 2.0f * std::sqrt(l.intensity * m));
            }
            e["components"]["Light"] = light;
        }
        if (n.camera >= 0 && static_cast<std::size_t>(n.camera) < mesh->cameras.size()) {
            const assets::CameraDef& c = mesh->cameras[static_cast<std::size_t>(n.camera)];
            e["components"]["Camera"] = Json{{"active", false}, {"orthographic", c.orthographic}, {"fov_degrees", c.yfov * 180.0f / 3.14159265f}, {"ortho_size", c.ymag}, {"near", c.znear}, {"far", c.zfar}};
        }
        // Components from the node's custom properties (docs/design/assets.md, Components from
        // Blender): "pocket" holds several, "pocket.<Component>" one, each an object or its
        // JSON text; they are merged (as a JSON merge patch) over what the node brings, its light and
        // camera too (a mesh collider with no file of its own collides with the node's
        // triangles, the MeshRenderer's).
        if (n.extras.is_object()) {
            for (const auto& [key, value] : n.extras.items()) {
                if (key != "pocket" && !key.starts_with("pocket.")) continue;
                Json obj = value;
                if (obj.is_string()) obj = Json::parse(obj.get<std::string>(), nullptr, false);
                if (!obj.is_object()) {
                    warnings.push_back(std::format("{}: {} is not an object or the JSON text of one", n.name, key));
                    continue;
                }
                const Json given = key == "pocket" ? obj : Json{{key.substr(7), obj}};
                for (const auto& [component, fields] : given.items()) {
                    if (!world_->known_component(component)) {   // the engine's and the project's
                        warnings.push_back(std::format("{}: no component named {} (world.schema lists them)", n.name, component));
                        continue;
                    }
                    if (!fields.is_object()) {
                        warnings.push_back(std::format("{}: {} is not an object of fields", n.name, component));
                        continue;
                    }
                    Json& into = e["components"][component];
                    if (!into.is_object()) into = Json::object();
                    into.merge_patch(fields);
                }
            }
        }
        Json kids = Json::array();
        for (int c : n.children) if (c >= 0 && static_cast<std::size_t>(c) < count) kids.push_back(entity_of(c));
        if (!kids.empty()) e["children"] = kids;
        return e;
    };
    Json kids = Json::array();
    for (std::size_t i = 0; i < count; ++i) if (mesh->nodes[i].parent < 0) kids.push_back(entity_of(static_cast<int>(i)));
    return kids;
}

std::string Session::file_hash(const std::string& path) const {
    auto full = inside_dir(options_.project_dir, path);
    if (!full) return {};
    auto bytes = fs::read_text(*full);
    if (!bytes) return {};
    return std::format("{:016x}", fnv1a(*bytes));
}

Json Session::relink_models() {
    Json done = Json::array();
    if (!world_ || !assets_) return done;
    std::vector<std::pair<world::EntityId, world::Model>> models;
    world_->ecs().each([&](flecs::entity e, const world::Model& m) { if (m.live && !m.path.empty()) models.emplace_back(e.id(), m); });
    for (const auto& [id, m] : models) {
        const std::string now = file_hash(m.path);
        if (now.empty() || now == m.hash) continue;
        std::vector<std::string> warnings;
        auto kids = model_children(m.path, warnings);
        if (!kids) {
            log::warn("runtime", "model {}: {}", m.path, kids.error().to_string());
            continue;
        }
        // The children made from the file go, and the file's nodes come again under the same root.
        for (world::EntityId c : world_->children(id)) (void)world_->destroy(c);
        auto made = world_->instantiate(Json{{"format", "pocket-scene"}, {"version", 1}, {"entities", *kids}}, id);
        if (!made) {
            log::warn("runtime", "model {}: {}", m.path, made.error().to_string());
            continue;
        }
        world::Model next = m;
        next.hash = now;
        world_->ecs().entity(id).set<world::Model>(next);
        world_->update_transforms();
        Json info{{"entity", id}, {"path", m.path}, {"children", made->size()}};
        if (!warnings.empty()) info["warnings"] = warnings;
        world_->events().emit(world_->tick_index(), "model.relinked", id, info, 0, "assets");
        done.push_back(std::move(info));
    }
    return done;
}

Result<Json> Session::world_command(std::string_view op, const Json& p, std::string_view source) {
    if (op == "lint") return world_lint(p);
    auto& w = *world_;
    auto need_entity = [&](const char* key) -> Result<world::EntityId> {
        if (!p.is_object() || !p.contains(key)) return fail("bad_args", "missing '{}'", key);
        world::EntityId id = resolve_entity(p[key]);
        if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p[key].dump());
        return id;
    };
    std::uint64_t cause = opt<std::uint64_t>(p, "cause", 0);
    // A field that holds an entity (Joint2D.body, Joint.target...) may name it: "Plank0" or a path,
    // turned into its id here, so a caller need not look ids up first.
    // A field named by its only start of three letters or more ("pos" for "position") or by a common
    // short form ("scl") is that field; the answer's `renamed` says what was read as what.
    Json renamed = Json::object();
    auto expand = [&](const std::string& comp, Json& value) {
        if (!value.is_object()) return;
        const auto infos = w.component_infos_all();
        const auto ci = std::find_if(infos.begin(), infos.end(), [&](const world::ComponentInfo& c) { return c.name == comp; });
        if (ci == infos.end()) return;
        auto field = [&](std::string_view n) { return std::any_of(ci->fields.begin(), ci->fields.end(), [&](const world::FieldInfo& f) { return f.name == n; }); };
        static const std::map<std::string_view, std::string_view> shorts = {{"pos", "position"}, {"rot", "rotation"}, {"scl", "scale"}, {"col", "color"}, {"colour", "color"}};
        Json out = Json::object();
        for (const auto& [k, v] : value.items()) {
            std::string to;
            if (!field(k)) {
                if (const auto s = shorts.find(k); s != shorts.end() && field(s->second)) to = s->second;
                else if (k.size() >= 3) {
                    int starts = 0;
                    for (const world::FieldInfo& f : ci->fields)
                        if (f.name.size() > k.size() && f.name.substr(0, k.size()) == k) { to = f.name; ++starts; }
                    if (starts != 1) to.clear();
                }
            }
            if (to.empty() || value.contains(to)) { out[k] = v; continue; }   // left for the check to refuse
            out[to] = v;
            renamed[comp + "." + k] = to;
        }
        value = std::move(out);
    };
    // A key that is a path into a field ("layers.1.height", "position.y") changes only that part:
    // the field is read as it is now (or from the patch, when it sets the field too), the path walked
    // through objects and lists (a number is an index), the part set, and the whole field written.
    auto paths = [&](world::EntityId id, const std::string& comp, Json& value) -> Status {
        if (!value.is_object()) return {};
        std::vector<std::string> dotted;
        for (const auto& [k, v] : value.items()) if (k.find('.') != std::string::npos) dotted.push_back(k);
        if (dotted.empty()) return {};
        Json now = Json::object();
        if (w.known_component(comp) && w.has(id, comp)) {
            POCKET_TRY(got, w.get(id, comp));
            now = got;
        }
        for (const std::string& key : dotted) {
            std::vector<std::string> parts;
            for (std::size_t at = 0; at <= key.size();) {
                const std::size_t dot = std::min(key.find('.', at), key.size());
                parts.push_back(key.substr(at, dot - at));
                at = dot + 1;
            }
            const std::string& top = parts.front();
            Json field = value.contains(top) ? value[top] : now.value(top, Json());
            if (field.is_null()) return fail("bad_args", "{}.{}: {} has no field '{}' to go into", comp, key, comp, top);
            Json* at = &field;
            for (std::size_t i = 1; i < parts.size(); ++i) {
                const std::string& part = parts[i];
                if (at->is_array()) {
                    const bool number = !part.empty() && std::all_of(part.begin(), part.end(), [](char c) { return c >= '0' && c <= '9'; });
                    if (!number) return fail("bad_args", "{}.{}: '{}' is a list; a number picks one of its {}", comp, key, part, at->size());
                    const std::size_t n = std::stoul(part);
                    if (n >= at->size()) return fail("bad_args", "{}.{}: the list has {} (0 to {})", comp, key, at->size(), at->size() == 0 ? 0 : at->size() - 1);
                    at = &(*at)[n];
                } else if (at->is_object() || at->is_null()) {
                    at = &(*at)[part];
                } else {
                    return fail("bad_args", "{}.{}: '{}' is a value, not something with parts", comp, key, parts[i - 1]);
                }
            }
            if (at->is_object() && value[key].is_object()) at->update(value[key]);
            else *at = value[key];
            value[top] = field;
            value.erase(key);
        }
        return {};
    };
    // Turning a Transform the ways agents reach for (docs/design/world-model.md, Turning): rotation as
    // {yaw, pitch, roll} in degrees, or as {x, y, z} without a w when a part is more than 1 (angles,
    // not a quaternion: x pitch, y yaw, z roll), and look_at, a point or an entity its -Z (its
    // forward) turns to, keeping +Y up. Each is said in `renamed`.
    auto turning = [&](world::EntityId id, const std::string& comp, Json& value) -> Status {
        if (comp != "Transform" || !value.is_object()) return {};
        constexpr float kDeg = std::numbers::pi_v<float> / 180.0f;
        auto num = [](const Json& o, const char* k) { return o.contains(k) && o[k].is_number() ? o[k].get<float>() : 0.0f; };
        auto quat_json = [](Quat q) { return Json{{"x", q.x}, {"y", q.y}, {"z", q.z}, {"w", q.w}}; };
        if (value.contains("rotation") && value["rotation"].is_object()) {
            const Json& r = value["rotation"];
            if (r.contains("yaw") || r.contains("pitch") || r.contains("roll")) {
                value["rotation"] = quat_json(Quat::from_euler(Vec3{num(r, "pitch") * kDeg, num(r, "yaw") * kDeg, num(r, "roll") * kDeg}));
                renamed["Transform.rotation"] = "yaw, pitch and roll in degrees, as a quaternion";
            } else if (!r.contains("w") && (std::fabs(num(r, "x")) > 1.0001f || std::fabs(num(r, "y")) > 1.0001f || std::fabs(num(r, "z")) > 1.0001f)) {
                value["rotation"] = quat_json(Quat::from_euler(Vec3{num(r, "x") * kDeg, num(r, "y") * kDeg, num(r, "z") * kDeg}));
                renamed["Transform.rotation"] = "x, y, z read as degrees (pitch, yaw, roll), as a quaternion";
            }
        }
        if (value.contains("look_at")) {
            const Json& t = value["look_at"];
            Vec3 to;
            if (t.is_object()) {
                to = Vec3{num(t, "x"), num(t, "y"), num(t, "z")};
            } else {
                const world::EntityId target = resolve_entity(t);
                w.update_transforms();   // where it is now, after any set this tick
                const auto* tw = w.alive(target) ? w.try_get<world::WorldTransform>(target) : nullptr;
                if (!tw) return fail("no_such_entity", "Transform.look_at: no entity for {}", t.dump());
                to = tw->position;
            }
            Vec3 from{0, 0, 0};
            if (value.contains("position") && value["position"].is_object()) from = Vec3{num(value["position"], "x"), num(value["position"], "y"), num(value["position"], "z")};
            else if (id && w.alive(id)) {
                w.update_transforms();
                if (const auto* own = w.try_get<world::WorldTransform>(id)) from = own->position;
            }
            const Vec3 d = to - from;
            const float flat = std::hypot(d.x, d.z);
            if (flat < 1e-6f && std::fabs(d.y) < 1e-6f) return fail("bad_args", "Transform.look_at: the point is where the entity stands");
            value["rotation"] = quat_json(Quat::from_euler(Vec3{std::atan2(d.y, flat), std::atan2(-d.x, -d.z), 0.0f}));
            value.erase("look_at");
            renamed["Transform.look_at"] = "rotation";
        }
        return {};
    };
    auto name_entities = [&](const std::string& comp, Json& value) -> Status {
        expand(comp, value);
        if (!value.is_object()) return {};
        for (const world::ComponentInfo& info : w.component_infos_all()) {
            if (info.name != comp) continue;
            for (const world::FieldInfo& f : info.fields) {
                if (f.type != "entity") continue;
                const std::string key(f.name);
                if (!value.contains(key) || !value[key].is_string()) continue;
                const world::EntityId target = resolve_entity(value[key]);
                if (!w.alive(target)) return fail("no_such_entity", "{}.{}: no entity named {}", comp, key, value[key].dump());
                value[key] = target;
            }
            break;
        }
        return {};
    };
    if (op == "spawn") {
        world::EntityId parent = 0;
        if (p.contains("parent") && !p["parent"].is_null()) {
            parent = resolve_entity(p["parent"]);
            if (!w.alive(parent)) return fail("no_such_entity", "no parent for {}", p["parent"].dump());
        }
        Json comps = p.value("components", Json::object());
        if (comps.is_object()) for (auto& [cname, cvalue] : comps.items()) {
            POCKET_TRY_VOID(name_entities(cname, cvalue));
            POCKET_TRY_VOID(turning(0, cname, cvalue));
        }
        POCKET_TRY_VOID(w.check_components(comps));
        POCKET_TRY(id, w.spawn(opt<std::string>(p, "name", ""), parent, comps, cause));
        Json j;
        j["id"] = id;
        j["path"] = w.path(id);
        if (!renamed.empty()) j["renamed"] = renamed;
        return j;
    }
    if (op == "destroy") {
        POCKET_TRY(id, need_entity("entity"));
        POCKET_TRY_VOID(w.destroy(id, cause));
        return Json{{"ok", true}};
    }
    if (op == "get") {
        POCKET_TRY(id, need_entity("entity"));
        std::string comp = opt<std::string>(p, "component", "");
        if (comp.empty() && p.contains("components") && p["components"].is_array()) {
            // Several: each by name, as world.get without a component answers all of them.
            Json some = Json::object();
            for (const std::string& n : string_list(p, "components")) {
                if (!w.known_component(n)) return fail("unknown_component", "unknown component '{}'", n);
                if (!w.has(id, n)) { some[n] = nullptr; continue; }
                POCKET_TRY(v, w.get(id, n));
                some[n] = v;
            }
            return some;
        }
        if (!comp.empty() && p.contains("field") && p["field"].is_string()) {
            // One field, or a path into it ("position.y", "layers.1.height").
            if (!w.known_component(comp)) return fail("unknown_component", "unknown component '{}'", comp);
            if (!w.has(id, comp)) return nullptr;
            POCKET_TRY(v, w.get(id, comp));
            const std::string path = p["field"].get<std::string>();
            const Json* at = &v;
            for (std::size_t from = 0; from <= path.size();) {
                const std::size_t dot = std::min(path.find('.', from), path.size());
                const std::string seg = path.substr(from, dot - from);
                if (at->is_object() && at->contains(seg)) at = &(*at)[seg];
                else if (at->is_array() && !seg.empty() && std::all_of(seg.begin(), seg.end(), [](char c) { return c >= '0' && c <= '9'; }) && std::stoul(seg) < at->size()) at = &(*at)[std::stoul(seg)];
                else return fail("bad_args", "{} has no field '{}' ({})", comp, seg, path);
                from = dot + 1;
            }
            return *at;
        }
        if (comp.empty()) {
            // Without a component: every component the entity has, by name.
            Json all = Json::object();
            for (const world::ComponentInfo& info : w.component_infos_all()) {
                const std::string n(info.name);
                if (w.has(id, n)) {
                    POCKET_TRY(v, w.get(id, n));
                    all[n] = v;
                }
            }
            return all;
        }
        if (!w.known_component(comp)) return fail("unknown_component", "unknown component '{}'", comp);
        if (!w.has(id, comp)) return nullptr;
        return w.get(id, comp);
    }
    if (op == "set" && p.contains("components")) {
        // Several components at once, each a patch as `value` is for one: all checked before any is set.
        POCKET_TRY(id, need_entity("entity"));
        if (!p["components"].is_object() || p["components"].empty()) return fail("bad_args", "components is an object of component name -> the fields to change, e.g. {{Transform: {{position: {{x: 1}}}}, MeshRenderer: {{color: \"#ff0000\"}}}}");
        Json comps = p["components"];
        if (p.contains("component") && p.contains("value")) comps[p["component"].get<std::string>()] = p["value"];
        for (auto& [cname, cvalue] : comps.items()) {
            if (!cvalue.is_object()) return fail("bad_args", "components.{}: the fields to change as an object", cname);
            POCKET_TRY_VOID(paths(id, cname, cvalue));
            POCKET_TRY_VOID(name_entities(cname, cvalue));
            POCKET_TRY_VOID(turning(id, cname, cvalue));
            POCKET_TRY_VOID(w.check_patch(cname, cvalue));
        }
        Json values = Json::object();
        for (const auto& [cname, cvalue] : comps.items()) POCKET_TRY_VOID(w.set(id, cname, cvalue, cause));
        if (opt<bool>(p, "quiet", false)) return source == "script" ? Json() : Json{{"ok", true}};
        for (const auto& [cname, cvalue] : comps.items()) {
            POCKET_TRY(now, w.get(id, cname));
            values[cname] = now;
        }
        Json j{{"ok", true}, {"values", values}};
        if (!renamed.empty()) j["renamed"] = renamed;
        return j;
    }
    if (op == "set") {
        POCKET_TRY(id, need_entity("entity"));
        if (!p.contains("value") || !p["value"].is_object()) return fail("bad_args", "world.set needs value: the fields to change as an object, e.g. {{entity: \"Ball\", component: \"MeshRenderer\", value: {{color: {{r: 0, g: 1, b: 0, a: 1}}}}}} (or components: {{MeshRenderer: {{...}}, Transform: {{...}}}} for several)");
        if (!p.contains("component") || !p["component"].is_string()) return fail("bad_args", "world.set needs component: the component's name, e.g. \"MeshRenderer\" (world.schema lists them)");
        const std::string comp = p["component"].get<std::string>();
        Json value = p["value"];
        POCKET_TRY_VOID(paths(id, comp, value));
        POCKET_TRY_VOID(name_entities(comp, value));
        POCKET_TRY_VOID(turning(id, comp, value));
        POCKET_TRY_VOID(w.check_patch(comp, value));
        POCKET_TRY_VOID(w.set(id, comp, value, cause));
        if (opt<bool>(p, "quiet", false)) return source == "script" ? Json() : Json{{"ok", true}};   // a caller that does not read it back (the SDK: nothing to turn into a value)
        POCKET_TRY(now, w.get(id, comp));
        Json j{{"ok", true}, {"value", now}};   // the component as it is now, the patch merged in
        if (!renamed.empty()) j["renamed"] = renamed;
        return j;
    }
    if (op == "remove") {
        POCKET_TRY(id, need_entity("entity"));
        if (opt<std::string>(p, "component", "").empty()) return fail("bad_args", "world.remove takes a component off an entity (component: \"Velocity\"); world.destroy removes the entity itself");
        POCKET_TRY_VOID(w.remove(id, opt<std::string>(p, "component", ""), cause));
        return Json{{"ok", true}};
    }
    if (op == "has") {
        POCKET_TRY(id, need_entity("entity"));
        return w.has(id, opt<std::string>(p, "component", ""));
    }
    if (op == "describe") {
        POCKET_TRY(id, need_entity("entity"));
        return w.describe(id);
    }
    if (op == "find") {
        world::EntityId id = w.find(opt<std::string>(p, "path", ""));
        if (!id) return nullptr;
        return id;
    }
    if (op == "children") {
        POCKET_TRY(id, need_entity("entity"));
        Json arr = Json::array();
        for (auto c : w.children(id)) arr.push_back(c);
        return arr;
    }
    if (op == "roots") {
        Json arr = Json::array();
        for (auto c : w.roots()) arr.push_back(c);
        return arr;
    }
    if (op == "components") {
        POCKET_TRY(id, need_entity("entity"));
        return Json(w.components_of(id));
    }
    if (op == "reparent") {
        POCKET_TRY(id, need_entity("entity"));
        world::EntityId parent = 0;
        if (p.contains("parent") && !p["parent"].is_null()) {
            parent = resolve_entity(p["parent"]);
            if (!w.alive(parent)) return fail("no_such_entity", "no parent for {}", p["parent"].dump());
        }
        // keep_world: the entity stays where it is in the world, its local transform taking up the
        // difference between the old parent's frame and the new one's (the editor's way).
        if (opt<bool>(p, "keep_world", false) && w.has(id, "Transform")) {
            w.update_transforms();
            const auto* wt = w.try_get<world::WorldTransform>(id);
            const auto* pw = parent != 0 ? w.try_get<world::WorldTransform>(parent) : nullptr;
            if (wt) {
                world::Transform local;
                if (pw) {
                    const Quat inv{-pw->rotation.x, -pw->rotation.y, -pw->rotation.z, pw->rotation.w};   // the parent's rotation undone
                    const Vec3 rel = inv.rotate(wt->position - pw->position);
                    auto safe = [](float v) { return std::fabs(v) > 1e-8f ? v : 1e-8f; };
                    local.position = {rel.x / safe(pw->scale.x), rel.y / safe(pw->scale.y), rel.z / safe(pw->scale.z)};
                    local.rotation = normalize(inv * wt->rotation);
                    local.scale = {wt->scale.x / safe(pw->scale.x), wt->scale.y / safe(pw->scale.y), wt->scale.z / safe(pw->scale.z)};
                } else {
                    local.position = wt->position;
                    local.rotation = wt->rotation;
                    local.scale = wt->scale;
                }
                w.ecs().entity(id).set<world::Transform>(local);
            }
        }
        POCKET_TRY_VOID(w.reparent(id, parent));
        w.update_transforms();
        return Json{{"path", w.path(id)}};
    }
    if (op == "rename") {
        POCKET_TRY(id, need_entity("entity"));
        POCKET_TRY_VOID(w.rename(id, opt<std::string>(p, "name", "")));
        return Json{{"path", w.path(id)}};
    }
    if (op == "tree") {
        world::TreeOptions to;
        if (p.contains("root") && !p["root"].is_null()) {
            to.root = resolve_entity(p["root"]);
            if (!w.alive(to.root)) return fail("no_such_entity", "no entity for {}", p["root"].dump());
        }
        to.depth = opt<int>(p, "depth", -1);
        to.max_entities = opt<int>(p, "max_entities", 200);
        to.values = opt<bool>(p, "values", true);
        to.components = string_list(p, "components");
        return Json{{"text", w.tree(to)}};
    }
    if (op == "query") {
        world::QueryOptions q;
        q.with = string_list(p, "with");
        q.without = string_list(p, "without");
        q.fields = string_list(p, "fields");
        q.name = opt<std::string>(p, "name", "");
        q.limit = opt<int>(p, "limit", 1000);
        if (p.contains("where")) {
            POCKET_TRY(conditions, world::parse_where(p["where"]));
            q.where = std::move(conditions);
        }
        if (p.contains("under") && !p["under"].is_null()) {
            q.under = resolve_entity(p["under"]);
            if (!w.alive(q.under)) return fail("no_such_entity", "no entity for {}", p["under"].dump());
        }
        Json r = w.query(q);
        if (r.contains("error")) return fail("bad_query", "{}", r["error"].get<std::string>());
        return r;
    }
    if (op == "summary") return w.summary();
    if (op == "schema") {
        // Every component, or the ones asked for: by name (component, components) or by a word in
        // the name, a field or the docs (search).
        Json all = w.schema();
        const std::string one = opt<std::string>(p, "component", "");
        std::set<std::string> names;
        if (!one.empty()) names.insert(one);
        for (const std::string& n : string_list(p, "components")) names.insert(n);
        std::string find = opt<std::string>(p, "search", "");
        for (char& c : find) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
        if (names.empty() && find.empty()) return all;
        Json picked = Json::array();
        for (const Json& c : all["components"]) {
            const std::string n = c["name"].get<std::string>();
            bool take = names.contains(n);
            if (!take && !find.empty()) {
                std::string hay = c.dump();
                for (char& ch : hay) ch = static_cast<char>(std::tolower(static_cast<unsigned char>(ch)));
                take = hay.find(find) != std::string::npos;
            }
            if (take) picked.push_back(c);
        }
        // Names that are no component: answered beside the ones that are, each with what it might
        // have meant (a component whose name holds it or is held in it, BoxCollider -> Collider; a
        // field of that name, Snow -> Weather.snow; a record type and where it lives,
        // TerrainLayer -> Terrain.layers), rather than failing the whole call.
        Json unknown = Json::object();
        for (const std::string& n : names) {
            bool found = false;
            for (const Json& c : picked) found = found || c["name"] == n;
            if (found) continue;
            std::string low = n;
            for (char& ch : low) ch = static_cast<char>(std::tolower(static_cast<unsigned char>(ch)));
            Json maybe = Json::array();
            for (const Json& c : all["components"]) {
                const std::string cn = c["name"].get<std::string>();
                std::string cl = cn;
                for (char& ch : cl) ch = static_cast<char>(std::tolower(static_cast<unsigned char>(ch)));
                if (cl.size() >= 3 && (low.find(cl) != std::string::npos || cl.find(low) != std::string::npos)) maybe.push_back(cn);
                for (const Json& f : c.value("fields", Json::array())) {
                    const std::string fn = f.value("name", "");
                    const std::string ft = f.value("type", "");
                    if (fn == low || (ft.starts_with("list:") && ft.substr(5) == n)) maybe.push_back(cn + "." + fn);
                }
            }
            unknown[n] = maybe.empty() ? Json("no component, field or record of that name") : Json{{"did_you_mean", maybe}};
        }
        if (picked.empty() && !unknown.empty()) {
            return fail("bad_args", "no component named {} ({}; world.schema without a name lists them)", Json(std::vector<std::string>(names.begin(), names.end())).dump(), unknown.dump());
        }
        Json out{{"components", picked}};
        if (!unknown.empty()) out["unknown"] = unknown;
        return out;
    }
    if (op == "update_transforms") {
        w.update_transforms();
        return Json::object();
    }
    if (op == "save") {
        if (p.contains("entity") && !p["entity"].is_null()) {
            world::EntityId id = resolve_entity(p["entity"]);
            if (!w.alive(id)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
            return w.save_subtree(id);
        }
        // Meshes made by code go with the scene that draws them.
        Json scene = w.save();
        if (&w == world_.get()) {
            if (!made_meshes_.empty()) scene["meshes"] = made_meshes_;
            if (Json maps = saved_maps(false); !maps.empty()) scene["tilemaps"] = maps;
        }
        return scene;
    }
    if (op == "mark") {
        // The world as it is now, kept under a name, for world.diff to compare with later.
        const std::string mark = opt<std::string>(p, "name", "default");
        Json snap = w.snapshot();
        const std::size_t n = snap.size();
        marks_[mark] = Json{{"tick", clock_.tick}, {"entities", std::move(snap)}};
        return Json{{"mark", mark}, {"tick", clock_.tick}, {"entities", n}};
    }
    if (op == "diff") {
        // What changed since a mark: entities spawned and destroyed, and for the others the
        // components added, removed, and each changed field's value then and now.
        const std::string mark = opt<std::string>(p, "since", "default");
        auto it = marks_.find(mark);
        if (it == marks_.end()) return fail("no_such_mark", "no mark named '{}' (world.mark {{name}} makes one)", mark);
        const std::size_t limit = static_cast<std::size_t>(std::clamp(opt<int>(p, "limit", 50), 1, 100000));
        const Json& then = it->second["entities"];
        const Json now = w.snapshot();
        Json spawned = Json::array(), destroyed = Json::array(), changed = Json::array();
        std::size_t n_spawned = 0, n_destroyed = 0, n_changed = 0;
        for (const auto& [id, e] : now.items()) {
            if (!then.contains(id)) {
                if (++n_spawned <= limit) spawned.push_back(Json{{"id", std::stoull(id)}, {"path", e["path"]}, {"components", [&] { Json c = Json::array(); for (const auto& [k, v] : e["components"].items()) c.push_back(k); return c; }()}});
                continue;
            }
            const Json& before = then[id]["components"];
            const Json& after = e["components"];
            Json comps = Json::object();
            for (const auto& [name, value] : after.items()) {
                if (!before.contains(name)) { comps[name] = Json{{"added", value}}; continue; }
                if (before[name] == value) continue;
                Json fields = Json::object();
                if (value.is_object() && before[name].is_object()) {
                    for (const auto& [f, v] : value.items()) if (!before[name].contains(f) || before[name][f] != v) fields[f] = Json::array({before[name].value(f, Json()), v});
                }
                comps[name] = Json{{"fields", fields}};
            }
            for (const auto& [name, value] : before.items()) if (!after.contains(name)) comps[name] = Json{{"removed", true}};
            if (then[id]["path"] != e["path"]) comps["path"] = Json::array({then[id]["path"], e["path"]});
            if (comps.empty()) continue;
            if (++n_changed <= limit) changed.push_back(Json{{"id", std::stoull(id)}, {"path", e["path"]}, {"changes", comps}});
        }
        for (const auto& [id, e] : then.items()) {
            if (now.contains(id)) continue;
            if (++n_destroyed <= limit) destroyed.push_back(Json{{"id", std::stoull(id)}, {"path", e["path"]}});
        }
        Json out{{"since", mark}, {"since_tick", it->second["tick"]}, {"tick", clock_.tick}, {"spawned", spawned}, {"destroyed", destroyed}, {"changed", changed},
                 {"counts", Json{{"spawned", n_spawned}, {"destroyed", n_destroyed}, {"changed", n_changed}}}};
        if (n_spawned > limit || n_destroyed > limit || n_changed > limit) out["note"] = std::format("lists cut at {} (limit)", limit);
        return out;
    }
    if (op == "load") {
        // An inline scene object, or a scene file by project-relative path.
        Json scene = p.value("scene", Json::object());
        std::string path = opt<std::string>(p, "path", "");
        if (!path.empty()) {
            std::filesystem::path base = std::filesystem::weakly_canonical(options_.project_dir);
            std::filesystem::path full = std::filesystem::weakly_canonical(base / path);
            auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
            if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", path);
            POCKET_TRY(text, fs::read_text(full));
            scene = Json::parse(text, nullptr, false);
            if (scene.is_discarded()) return fail("bad_scene", "{} is not valid JSON: {}", path, json_problem(text));
        }
        if (scene.contains("meshes") && scene["meshes"].is_object())
            for (const auto& [mname, spec] : scene["meshes"].items()) POCKET_TRY_VOID(make_mesh(mname, spec));
        if (scene.contains("tilemaps")) POCKET_TRY_VOID(restore_maps(scene["tilemaps"]));
        POCKET_TRY_VOID(w.load(scene, opt<bool>(p, "clear", true)));
        if (!path.empty()) w.events().emit(clock_.tick, "scene.loaded", 0, Json{{"path", path}, {"entities", w.entity_count()}}, 0, std::string(source));
        return Json{{"entities", w.entity_count()}};
    }
    if (op == "instantiate") {
        Json fragment = p.value("scene", Json());
        std::string prefab = opt<std::string>(p, "prefab", "");
        if (!prefab.empty()) {
            std::filesystem::path base = std::filesystem::weakly_canonical(options_.project_dir);
            std::filesystem::path full = std::filesystem::weakly_canonical(base / prefab);
            auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
            if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", prefab);
            auto it = prefab_cache_.find(prefab);
            if (it == prefab_cache_.end()) {
                POCKET_TRY(text, fs::read_text(full));
                Json parsed = Json::parse(text, nullptr, false);
                if (parsed.is_discarded()) return fail("bad_scene", "{} is not valid JSON: {}", prefab, json_problem(text));
                it = prefab_cache_.emplace(prefab, std::move(parsed)).first;
            }
            fragment = it->second;
        }
        std::string mesh_path = opt<std::string>(p, "mesh", "");
        std::vector<std::string> warnings;   // what a model's custom properties asked for that could not be
        if (!mesh_path.empty()) {
            // A glTF file's node tree as entities under one root: one per node with the node's own
            // transform and, when it carries geometry, a MeshRenderer drawing that node alone.
            // The file's nodes under one root that remembers the file (Model), so a changed file
            // can make them again in place (docs/design/assets.md, Live models).
            POCKET_TRY(kids, model_children(mesh_path, warnings));
            Json root;
            root["name"] = std::filesystem::path(mesh_path).stem().string();
            const Vec3 at = vec3_of(p.value("position", Json(nullptr)), Vec3{0, 0, 0});
            root["components"]["Transform"] = Json{{"position", {{"x", at.x}, {"y", at.y}, {"z", at.z}}}};
            root["components"]["Model"] = Json{{"path", mesh_path}, {"hash", file_hash(mesh_path)}};
            root["children"] = kids;
            fragment = Json{{"format", "pocket-scene"}, {"version", 1}, {"entities", Json::array({root})}};
        }
        if (fragment.is_null()) return fail("bad_args", "instantiate needs a prefab path, a scene object or a mesh");
        world::EntityId parent = 0;
        if (p.contains("parent") && !p["parent"].is_null()) {
            parent = resolve_entity(p["parent"]);
            if (!w.alive(parent)) return fail("no_such_entity", "no entity for {}", p["parent"].dump());
        }
        Json overrides = p.contains("components") && p["components"].is_object() ? p["components"] : Json::object();
        POCKET_TRY_VOID(w.check_components(overrides));
        std::uint64_t cause = static_cast<std::uint64_t>(opt<double>(p, "cause", 0));
        POCKET_TRY(roots, w.instantiate(fragment, parent, overrides, opt<std::string>(p, "name", ""), cause));
        Json j;
        j["roots"] = roots;
        if (!prefab.empty()) j["prefab"] = prefab;
        if (!mesh_path.empty()) j["mesh"] = mesh_path;
        if (!warnings.empty()) j["warnings"] = warnings;
        return j;
    }
    if (op == "pack") {
        // Numeric fields of one component for every matching entity, into Float32Array
        // __pocket.__pack_data (stride floats per entity) and Float64Array __pocket.__pack_ids.
        std::string component = opt<std::string>(p, "component", "");
        std::vector<std::string> fields = string_list(p, "fields");
        world::QueryOptions q;
        q.with = string_list(p, "with");
        q.without = string_list(p, "without");
        q.name = opt<std::string>(p, "name", "");
        q.limit = opt<int>(p, "limit", 1000000);
        if (p.contains("under") && !p["under"].is_null()) q.under = resolve_entity(p["under"]);
        std::vector<float> data;
        std::vector<double> ids;
        POCKET_TRY(info, w.pack(component, fields, q, data, ids));
        // Grow shared buffers by doubling; a script may still reference an old buffer, so old
        // storage is retired instead of freed.
        if (data.size() > pack_data_capacity_) {
            std::size_t cap = std::max<std::size_t>(data.size(), std::max<std::size_t>(pack_data_capacity_ * 2, 1024));
            if (!pack_data_.empty()) retired_data_.push_back(std::move(pack_data_));
            pack_data_ = std::vector<float>(cap, 0.0f);
            pack_data_capacity_ = cap;
            host_->share_f32("__pack_data", pack_data_.data(), cap);
        }
        if (ids.size() > pack_ids_capacity_) {
            std::size_t cap = std::max<std::size_t>(ids.size(), std::max<std::size_t>(pack_ids_capacity_ * 2, 256));
            if (!pack_ids_.empty()) retired_ids_.push_back(std::move(pack_ids_));
            pack_ids_ = std::vector<double>(cap, 0.0);
            pack_ids_capacity_ = cap;
            host_->share_f64("__pack_ids", pack_ids_.data(), cap);
        }
        std::copy(data.begin(), data.end(), pack_data_.begin());
        std::copy(ids.begin(), ids.end(), pack_ids_.begin());
        last_pack_ = info;
        last_pack_component_ = component;
        last_pack_fields_ = fields;
        Json j;
        j["count"] = info.count;
        j["stride"] = info.stride;
        Json layout = Json::object();
        for (auto& [f, off] : info.layout) layout[f] = off;
        j["layout"] = layout;
        return j;
    }
    if (op == "unpack") {
        // Write the shared buffers back for the rows of the last pack (or a given count).
        if (last_pack_component_.empty()) return fail("bad_args", "nothing packed yet");
        std::string component = opt<std::string>(p, "component", last_pack_component_);
        std::vector<std::string> fields = p.contains("fields") ? string_list(p, "fields") : last_pack_fields_;
        auto count = static_cast<std::size_t>(opt<int>(p, "count", static_cast<int>(last_pack_.count)));
        if (count > pack_ids_capacity_ || count * last_pack_.stride > pack_data_capacity_) return fail("bad_args", "count exceeds the packed buffers");
        POCKET_TRY_VOID(w.unpack(component, fields, pack_ids_.data(), count, pack_data_.data()));
        return Json{{"count", count}};
    }
    if (op == "save_prefab") {
        POCKET_TRY(id, need_entity("entity"));
        std::string rel = opt<std::string>(p, "path", "");
        if (rel.empty()) return fail("bad_args", "save_prefab needs a path such as prefabs/enemy.json");
        std::filesystem::path base = std::filesystem::weakly_canonical(options_.project_dir);
        std::filesystem::path full = std::filesystem::weakly_canonical(base / rel);
        auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
        if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", rel);
        Json fragment = w.save_subtree(id);
        std::filesystem::create_directories(full.parent_path());
        POCKET_TRY_VOID(fs::write_text(full, fragment.dump(2) + "\n"));
        prefab_cache_.erase(rel);
        std::size_t count = 0;
        std::function<void(const Json&)> count_entities = [&](const Json& e) { ++count; if (e.contains("children")) for (auto& c : e["children"]) count_entities(c); };
        for (auto& e : fragment["entities"]) count_entities(e);
        return Json{{"path", full.string()}, {"entities", count}};
    }
    if (op == "clear") {
        w.clear();
        return Json{{"ok", true}};
    }
    (void)source;
    return fail("unknown_command", "unknown world command '{}'", op);
}

Result<Json> Session::events_command(std::string_view op, const Json& p, std::string_view source) {
    auto& ev = world_->events();
    if (op == "emit") {
        std::string type = opt<std::string>(p, "type", "");
        if (type.empty()) return fail("bad_args", "events.emit needs a type");
        world::EntityId subject = p.contains("subject") ? resolve_entity(p["subject"]) : 0;
        std::uint64_t seq = ev.emit(world_->tick_index(), type, subject, p.value("data", Json(nullptr)), opt<std::uint64_t>(p, "cause", 0), source);
        return Json{{"seq", seq}};
    }
    if (op == "since") {
        auto list = ev.since(opt<std::uint64_t>(p, "seq", opt<std::uint64_t>(p, "since", 0)), static_cast<std::size_t>(opt<int>(p, "limit", 1000)), opt<std::string>(p, "type", ""));
        Json arr = Json::array();
        for (auto& e : list) arr.push_back(world::event_to_json(e));
        Json j;
        j["events"] = std::move(arr);
        j["last_seq"] = ev.last_seq();
        return j;
    }
    if (op == "recent") {
        // The newest events, oldest first: `limit` (or `n`) of them, of a type when `type` names one
        // (a prefix such as "player." takes the family).
        const std::size_t limit = static_cast<std::size_t>(std::max(0, opt<int>(p, "limit", opt<int>(p, "n", 50))));
        const std::string type = opt<std::string>(p, "type", "");
        Json arr = Json::array();
        if (type.empty()) {
            for (auto& e : ev.recent(limit)) arr.push_back(world::event_to_json(e));
            return arr;
        }
        std::vector<Json> picked;
        for (auto& e : ev.recent(std::numeric_limits<std::size_t>::max())) if (e.type.starts_with(type)) picked.push_back(world::event_to_json(e));
        const std::size_t from = picked.size() > limit ? picked.size() - limit : 0;
        for (std::size_t i = from; i < picked.size(); ++i) arr.push_back(std::move(picked[i]));
        return arr;
    }
    if (op == "histogram") return ev.histogram(opt<std::uint64_t>(p, "seq", 0));
    if (op == "last_seq") return Json{{"seq", ev.last_seq()}};
    if (op == "why") {
        std::uint64_t seq = opt<std::uint64_t>(p, "seq", 0);
        if (seq == 0) return fail("bad_args", "events.why needs a seq");
        auto chain = ev.why(seq, static_cast<std::size_t>(opt<int>(p, "limit", 32)));
        if (chain.empty()) return fail("not_found", "event {} is not in the log", seq);
        Json arr = Json::array();
        std::string story;
        for (const auto& e : chain) {
            arr.push_back(world::event_to_json(e));
            if (!story.empty()) story += " <- ";
            story += e.type;
            std::string who = e.subject && world_->alive(e.subject) ? world_->path(e.subject) : (e.data.is_object() && e.data.contains("path") && e.data["path"].is_string() ? e.data["path"].get<std::string>() : "");
            if (!who.empty()) story += "(" + who + ")";
            story += " at tick " + std::to_string(e.tick);
        }
        Json j;
        j["chain"] = std::move(arr);
        j["story"] = story;
        j["root"] = chain.back().seq;
        j["complete"] = chain.back().cause == 0;  // false when the chain stops at an evicted event
        return j;
    }
    return fail("unknown_command", "unknown events command '{}'", op);
}

void Session::build_debug_draw() {
    debug_draw_.clear();
    const world::World& w = *world_;
    if (debug_flags_.colliders) {
        w.ecs().each([&](flecs::entity e, const world::Collider& c, const world::Transform& authored) {
            rhi::Color color{0.6f, 0.6f, 0.6f, 1};  // static
            world::Transform t = authored;   // where physics puts it
            if (const auto* rb = e.try_get<world::RigidBody>()) {
                t = physics::placed(e, *rb, authored);
                if (rb->kind == 0) color = rb->sleeping ? rhi::Color{0.15f, 0.5f, 0.15f, 1} : rhi::Color{0.2f, 1.0f, 0.2f, 1};
                else if (rb->kind == 2) color = {0.3f, 0.6f, 1.0f, 1};
            }
            if (c.is_trigger) color = {1.0f, 0.9f, 0.2f, 1};
            Vec3 center = t.position + t.rotation.rotate(c.offset);
            if (c.shape == 1) debug_draw_.sphere(center, c.size.x, color);
            else if (c.shape == 2) debug_draw_.capsule(center, c.size.x, c.size.y, t.rotation, color);
            else debug_draw_.box(center, c.size, t.rotation, color);
        });
        // 2D shapes as outlines in their plane, in the same colours (docs/design/physics2d.md).
        w.ecs().each([&](flecs::entity e, const world::Collider2D& c, const world::Transform& authored) {
            rhi::Color color{0.6f, 0.6f, 0.6f, 1};
            Vec3 pos = authored.position;
            Quat rot = authored.rotation;
            if (const auto* rb = e.try_get<world::RigidBody2D>()) {
                if (rb->kind == 0) color = rb->awake ? rhi::Color{0.2f, 1.0f, 0.2f, 1} : rhi::Color{0.15f, 0.5f, 0.15f, 1};
                else if (rb->kind == 2) color = {0.3f, 0.6f, 1.0f, 1};
            } else if (const auto* wt = e.try_get<world::WorldTransform>()) {
                pos = wt->position;
                rot = wt->rotation;
            }
            if (c.sensor) color = {1.0f, 0.9f, 0.2f, 1};
            const Vec3 sc{authored.scale.x, authored.scale.y, 1};
            // A point of the shape's own space in the world.
            auto at = [&](float x, float y) { return pos + rot.rotate(Vec3{x * sc.x, y * sc.y, 0}); };
            auto loop = [&](const std::vector<std::pair<float, float>>& pts) {
                for (std::size_t i = 0; i < pts.size(); ++i) {
                    const auto& [x0, y0] = pts[i];
                    const auto& [x1, y1] = pts[(i + 1) % pts.size()];
                    debug_draw_.line(at(x0, y0), at(x1, y1), color);
                }
            };
            const float ca = std::cos(c.angle), sa = std::sin(c.angle);
            auto turned = [&](float x, float y) { return std::pair{c.offset.x + x * ca - y * sa, c.offset.y + x * sa + y * ca}; };
            std::vector<std::pair<float, float>> pts;
            if (c.shape == 1 || c.shape == 2) {
                // A circle, or a capsule's two half circles and its sides.
                const float r = c.radius;
                const float half = c.shape == 2 ? c.size.y : 0.0f;
                for (int i = 0; i <= 24; ++i) {
                    const float a = static_cast<float>(i) / 24.0f * 6.2831853f;
                    const float y = (a < 3.14159265f ? half : -half) + r * std::sin(a);
                    pts.push_back({c.offset.x + r * std::cos(a), c.offset.y + y});
                }
                loop(pts);
                if (c.shape == 1) debug_draw_.line(at(c.offset.x, c.offset.y), at(c.offset.x + r, c.offset.y), color);   // a spoke shows the turn
            } else if (c.shape == 3) {
                for (const world::Point2D& p : c.points) pts.push_back({p.x, p.y});
                if (pts.size() >= 2) loop(pts);
            } else {
                pts = {turned(-c.size.x, -c.size.y), turned(c.size.x, -c.size.y), turned(c.size.x, c.size.y), turned(-c.size.x, c.size.y)};
                loop(pts);
            }
        });
    }
    if (debug_flags_.joints) {
        // 2D joints: a line between the anchors, in the colour of their kind.
        w.ecs().each([&](flecs::entity, const world::Joint2D& j, const world::Transform& t) {
            const Vec3 a = t.position + t.rotation.rotate(Vec3{j.anchor.x, j.anchor.y, 0});
            Vec3 b{j.other_anchor.x, j.other_anchor.y, t.position.z};
            if (j.body) {
                const auto* tt = w.try_get<world::Transform>(static_cast<world::EntityId>(j.body));
                if (!tt) return;
                b = tt->position + tt->rotation.rotate(Vec3{j.other_anchor.x, j.other_anchor.y, 0});
            }
            const rhi::Color color = j.kind == 1 ? rhi::Color{1.0f, 0.4f, 0.8f, 1} : rhi::Color{1.0f, 0.6f, 0.2f, 1};
            debug_draw_.line(a, b, color);
            debug_draw_.sphere(a, 0.05f, color, 8);
            debug_draw_.sphere(b, 0.05f, color, 8);
        });
        w.ecs().each([&](flecs::entity e, const world::Joint& j, const world::Transform& t) {
            Vec3 a = t.position + t.rotation.rotate(j.anchor);
            Vec3 b = j.target_anchor;
            if (!j.target.empty()) {
                world::EntityId target = w.find(j.target);
                if (!target) return;
                const auto* tt = w.try_get<world::Transform>(target);
                if (!tt) return;
                b = tt->position + tt->rotation.rotate(j.target_anchor);
            }
            (void)e;
            rhi::Color color = j.kind == 2 ? rhi::Color{0.4f, 1.0f, 0.6f, 1} : j.kind == 1 ? rhi::Color{1.0f, 0.4f, 0.8f, 1} : rhi::Color{1.0f, 0.6f, 0.2f, 1};
            debug_draw_.line(a, b, color);
            debug_draw_.sphere(a, 0.05f, color, 8);
            debug_draw_.sphere(b, 0.05f, color, 8);
            if (j.kind == 2) {
                // The hinge axis through the anchor.
                Vec3 axis = t.rotation.rotate(length(j.axis) > 1e-6f ? normalize(j.axis) : Vec3{0, 0, 1});
                debug_draw_.line(a - axis * 0.4f, a + axis * 0.4f, color);
            }
        });
    }
    if (debug_flags_.paths) {
        // Paths as their curves, a dot at each of their points (docs/design/paths.md).
        std::vector<world::EntityId> ids;
        w.ecs().each([&](flecs::entity e, const world::Path&) { ids.push_back(e.id()); });
        for (world::EntityId id : ids) {
            const world::PathCurve* c = paths_.curve(*world_, id);
            if (!c) continue;
            const rhi::Color color{0.9f, 0.5f, 1.0f, 1};
            for (std::size_t i = 0; i + 1 < c->points.size(); ++i) debug_draw_.line(c->points[i], c->points[i + 1], color);
            if (c->closed && c->points.size() > 1) debug_draw_.line(c->points.back(), c->points.front(), color);
        }
    }
    if (debug_flags_.nav && nav_.baked()) {
        // Walkable cells as small crosses on the ground (or squares in a tile map's plane), then
        // the last paths asked for.
        const nav::Grid& g = nav_.grid();
        const float r = g.cell * 0.2f;
        const rhi::Color cell_color{0.3f, 0.9f, 0.9f, 0.6f};
        const rhi::Color blocked_color{1.0f, 0.45f, 0.15f, 0.9f};  // under an obstacle right now
        for (std::size_t i = 0; i < g.walkable.size(); ++i) {   // every floor's cells
            if (g.walkable[i] == 0) continue;
            const rhi::Color& col = (!g.blocked.empty() && g.blocked[i] != 0) ? blocked_color : cell_color;
            Vec3 c = g.center_at(i);
            if (g.plane == 0) {
                c.y += 0.02f;
                debug_draw_.line(c - Vec3{r, 0, 0}, c + Vec3{r, 0, 0}, col);
                debug_draw_.line(c - Vec3{0, 0, r}, c + Vec3{0, 0, r}, col);
            } else {
                debug_draw_.line(c - Vec3{r, 0, 0}, c + Vec3{r, 0, 0}, col);
                debug_draw_.line(c - Vec3{0, r, 0}, c + Vec3{0, r, 0}, col);
            }
        }
        // The navmesh's rectangles as outlines.
        const rhi::Color poly_color{0.85f, 0.45f, 0.95f, 0.9f};
        const nav::NavMesh& mesh = nav_.mesh();
        for (const nav::NavMesh::Poly& p : mesh.polys) {
            Vec3 c00 = g.center_at(g.index(p.x0, p.y0, p.layer)), c11 = g.center_at(g.index(p.x1, p.y1, p.layer)), c10 = g.center_at(g.index(p.x1, p.y0, p.layer)), c01 = g.center_at(g.index(p.x0, p.y1, p.layer));
            const float h = g.cell * 0.5f;
            Vec3 a, b, c, d;
            if (g.plane == 0) {
                a = c00 + Vec3{-h, 0.03f, -h}; b = c10 + Vec3{h, 0.03f, -h}; c = c11 + Vec3{h, 0.03f, h}; d = c01 + Vec3{-h, 0.03f, h};
            } else {
                a = c00 + Vec3{-h, h, 0}; b = c10 + Vec3{h, h, 0}; c = c11 + Vec3{h, -h, 0}; d = c01 + Vec3{-h, -h, 0};
            }
            debug_draw_.line(a, b, poly_color);
            debug_draw_.line(b, c, poly_color);
            debug_draw_.line(c, d, poly_color);
            debug_draw_.line(d, a, poly_color);
        }
        // Each moving agent: its velocity as a line and the corner it heads for.
        const rhi::Color agent_color{0.4f, 1.0f, 0.4f, 1};
        const Vec3 lift = g.plane == 0 ? Vec3{0, 0.05f, 0} : Vec3{0, 0, 0};
        w.ecs().each([&](flecs::entity, const world::NavAgent& a, const world::Transform& t) {
            if (a.state != 1) return;
            debug_draw_.line(t.position + lift, t.position + a.velocity * 0.5f + lift, agent_color);
            debug_draw_.sphere(a.corner + lift, g.cell * 0.15f, agent_color, 6);
        });
        const rhi::Color path_color{1.0f, 0.95f, 0.2f, 1};
        for (const auto& pts : nav_paths_) {
            for (std::size_t i = 1; i < pts.size(); ++i) debug_draw_.line(pts[i - 1] + Vec3{0, 0.05f, 0}, pts[i] + Vec3{0, 0.05f, 0}, path_color);
            for (const Vec3& p : pts) debug_draw_.sphere(p + Vec3{0, 0.05f, 0}, g.cell * 0.12f, path_color, 6);
        }
    }
    if (debug_flags_.bounds) {
        w.ecs().each([&](flecs::entity, const world::Bounds& b) { debug_draw_.aabb(b.min, b.max, {0.4f, 0.8f, 1.0f, 1}); });
    }
    if (debug_flags_.lights) {
        // Each light in its own color: the sun as an arrow along its direction from where its entity
        // stands, a point light as the sphere it reaches, a spot as its cone out to its range (the
        // outer edge full, the inner one faint).
        w.ecs().each([&](flecs::entity, const world::Light& l, const world::WorldTransform& t) {
            const rhi::Color color{std::min(l.color.r, 1.0f), std::min(l.color.g, 1.0f), std::min(l.color.b, 1.0f), 1};
            const rhi::Color faint{color.r, color.g, color.b, 0.45f};
            const Vec3 p = t.position;
            const Vec3 dir = normalize(t.rotation.rotate({0, 0, -1}));
            const Vec3 side = normalize(t.rotation.rotate({1, 0, 0})), up = normalize(t.rotation.rotate({0, 1, 0}));
            if (l.kind == 0) {
                const Vec3 tip = p + dir * 1.5f;
                debug_draw_.line(p, tip, color);
                debug_draw_.line(tip, tip - dir * 0.3f + side * 0.15f, color);
                debug_draw_.line(tip, tip - dir * 0.3f - side * 0.15f, color);
                debug_draw_.sphere(p, 0.08f, color, 8);
                return;
            }
            const float range = l.range > 0 ? l.range : 0.001f;
            debug_draw_.sphere(p, std::min(0.1f, range * 0.25f), color, 8);
            if (l.kind != 2) {
                debug_draw_.sphere(p, range, color);
                return;
            }
            const float outer = radians(std::clamp(l.outer_angle, 0.5f, 89.5f));
            const float inner = radians(std::clamp(l.inner_angle, 0.0f, std::clamp(l.outer_angle, 0.5f, 89.5f)));
            const Vec3 end = p + dir * (range * std::cos(outer));
            const float r = range * std::sin(outer);
            debug_draw_.circle(end, side, up, r, color);
            for (int k = 0; k < 4; ++k) {
                const float a = static_cast<float>(k) * 1.5707963f;
                debug_draw_.line(p, end + (side * std::cos(a) + up * std::sin(a)) * r, color);
            }
            debug_draw_.circle(p + dir * (range * std::cos(inner)), side, up, range * std::sin(inner), faint);
        });
    }
    if (debug_flags_.axes) debug_draw_.axes({0, 0, 0}, 1.0f);
    std::int64_t tick = clock_.tick;
    std::erase_if(debug_shapes_, [&](const DebugShape& d) { return tick >= d.until_tick; });
    for (const DebugShape& d : debug_shapes_) {
        if (d.kind == 0) debug_draw_.line(d.a, d.b, d.color);
        else if (d.kind == 1) debug_draw_.box(d.a, d.b, d.rotation, d.color);
        else debug_draw_.sphere(d.a, d.b.x, d.color);
    }
}

Result<Json> Session::debug_command(std::string_view op, const Json& p) {
    auto color_of = [&](rhi::Color fallback) {
        if (!p.contains("color")) return fallback;
        const Json& c = p["color"];
        if (c.is_object()) return rhi::Color{c.value("r", fallback.r), c.value("g", fallback.g), c.value("b", fallback.b), c.value("a", 1.0f)};
        if (c.is_array() && c.size() >= 3) return rhi::Color{c[0].get<float>(), c[1].get<float>(), c[2].get<float>(), c.size() > 3 ? c[3].get<float>() : 1.0f};
        return fallback;
    };
    auto add = [&](DebugShape d) {
        int ticks = std::max(opt<int>(p, "ticks", 1), 1);
        d.until_tick = clock_.tick + ticks;
        debug_shapes_.push_back(d);
        return Json{{"shapes", debug_shapes_.size()}, {"until_tick", d.until_tick}};
    };
    if (op == "line") {
        if (!p.contains("a") || !p.contains("b")) return fail("bad_args", "debug.line needs a and b");
        DebugShape d;
        d.kind = 0;
        d.a = vec3_of(p["a"], {0, 0, 0});
        d.b = vec3_of(p["b"], {0, 0, 0});
        d.color = color_of({1, 1, 1, 1});
        return add(d);
    }
    if (op == "box") {
        DebugShape d;
        d.kind = 1;
        d.a = vec3_of(p.value("center", Json(nullptr)), {0, 0, 0});
        d.b = vec3_of(p.value("half", Json(nullptr)), {0.5f, 0.5f, 0.5f});
        if (p.contains("rotation") && p["rotation"].is_object()) {
            const Json& q = p["rotation"];
            d.rotation = normalize(Quat{q.value("x", 0.0f), q.value("y", 0.0f), q.value("z", 0.0f), q.value("w", 1.0f)});
        }
        d.color = color_of({1, 1, 1, 1});
        return add(d);
    }
    if (op == "sphere") {
        DebugShape d;
        d.kind = 2;
        d.a = vec3_of(p.value("center", Json(nullptr)), {0, 0, 0});
        d.b = {opt<float>(p, "radius", 0.5f), 0, 0};
        d.color = color_of({1, 1, 1, 1});
        return add(d);
    }
    if (op == "clear") {
        std::size_t n = debug_shapes_.size();
        debug_shapes_.clear();
        return Json{{"cleared", n}};
    }
    if (op == "stats") {
        return Json{{"shapes", debug_shapes_.size()}, {"lines", renderer_->stats().debug_lines}, {"colliders", debug_flags_.colliders}, {"joints", debug_flags_.joints}, {"bounds", debug_flags_.bounds}, {"axes", debug_flags_.axes}, {"nav", debug_flags_.nav}, {"lights", debug_flags_.lights}, {"paths", debug_flags_.paths}};
    }
    return fail("unknown_command", "unknown debug command '{}'", op);
}

Result<Json> Session::recorder_command(std::string_view op, const Json& p) {
    if (op == "start") {
        recorder_.start(static_cast<std::size_t>(opt<int>(p, "ticks", 600)));
        return recorder_.status();
    }
    if (op == "stop") {
        recorder_.stop();
        return recorder_.status();
    }
    if (op == "clear") {
        recorder_.clear();
        return recorder_.status();
    }
    if (op == "status") return recorder_.status();
    auto entity_of = [&](const char* key) -> world::EntityId { return p.contains(key) ? resolve_entity(p[key]) : 0; };
    if (op == "at") {
        if (!p.contains("tick")) return fail("bad_args", "recorder.at needs a tick");
        return recorder_.at(p["tick"].get<std::int64_t>(), entity_of("entity"));
    }
    if (op == "diff") {
        std::int64_t from = opt<std::int64_t>(p, "from", recorder_.first_tick());
        std::int64_t to = opt<std::int64_t>(p, "to", recorder_.last_tick());
        return recorder_.diff(from, to, entity_of("entity"), static_cast<std::size_t>(opt<int>(p, "limit", 200)));
    }
    if (op == "track" || op == "first") {
        world::EntityId id = entity_of("entity");
        if (id == 0) return fail("not_found", "recorder.{} needs a live entity", op);
        std::string component = opt<std::string>(p, "component", "Transform");
        std::string field = opt<std::string>(p, "field", "");
        if (op == "track") {
            return recorder_.track(id, component, field, opt<std::int64_t>(p, "from", -1), opt<std::int64_t>(p, "to", -1), opt<int>(p, "every", 1));
        }
        if (!p.contains("value")) return fail("bad_args", "recorder.first needs op and value");
        return recorder_.first(id, component, field, opt<std::string>(p, "op", "<"), p["value"], opt<std::int64_t>(p, "from", -1));
    }
    return fail("unknown_command", "unknown recorder command '{}'", op);
}

Result<Json> Session::command(std::string_view name, const Json& params, std::string_view source) {
#if defined(__cpp_exceptions)
    try {
        return run_command(name, params, source);
    } catch (const Json::exception& e) {
        return fail("bad_args", "{}: a parameter has the wrong type ({}); help {{command: \"{}\"}} shows how to call it", name, e.what(), name);
    }
#else
    return run_command(name, params, source);   // web builds have no exceptions: a wrong type aborts there
#endif
}

Result<Json> Session::run_command(std::string_view name, const Json& params, std::string_view source) {
    if (!started_) return fail("not_started", "session not started");
    const Json& given = params.is_object() ? params : Json::object();
    // Reading a tile map under the names agents reach for first: tilemap.get (a cell when given one,
    // else the map as rows) and tilemap.text {entity} (the map as rows).
    if (name == "tilemap.get") name = given.contains("tile_x") || given.contains("x") ? "tilemap.tile" : "tilemap.rows";
    // Adding a component under the name agents reach for: world.set adds it when it is missing.
    else if (name == "world.add") name = "world.set";
    else if (name == "tilemap.text" && given.contains("entity") && !given.contains("rows")) name = "tilemap.rows";
    // A parameter an agent got wrong is said at once: a key a command does not take is refused with
    // the command's parameters (it would otherwise be ignored and the call look like it worked), and
    // `path` stands for `entity` where a command takes an entity and no path.
    Json adjusted;
    const Json* chosen = &given;
    const CommandHelp* h = command_help(name);
    if (!h) {
        // A function of the scripts' SDK asked for as a command (dialogue.check): it runs in a script.
        for (const SdkHelp& s : sdk_helps()) {
            if (s.name == name) return fail("unknown_command", "{} is a script function, not a command: call it through script.eval, e.g. {{source: \"{}(...)\"}} ({})", name, name, s.signature);
        }
        // An MCP tool's name sent as a method (runtime_commands, runtime_help, pocket_world_tree):
        // the command it stands for.
        for (std::string_view prefix : {"runtime_", "pocket_"}) {
            if (!name.starts_with(prefix)) continue;
            std::string cmd(name.substr(prefix.size()));
            const auto dot = cmd.find('_');
            if (!command_help(cmd) && dot != std::string::npos) cmd[dot] = '.';
            if (command_help(cmd)) return fail("unknown_command", "'{}' is the MCP tool's name; the command is '{}' (help {{command: \"{}\"}})", name, cmd, cmd);
        }
        // Every command has help (runtime_tests [help] holds it to that), so a name without is unknown.
        const std::vector<std::string> near = command_suggestions(name);
        return fail("unknown_command", "unknown command '{}'{}", name, near.empty() ? std::string("; `commands` lists them") : "; did you mean " + Json(near).dump() + "? (`help {command}` shows how to call one)");
    }
    {
        const std::vector<std::string>& keys = command_param_names(*h);
        auto takes = [&](const std::string& k) { return std::find(keys.begin(), keys.end(), k) != keys.end(); };
        // A place given whole (position, at or point; {x, y, z} or [x, y, z]) to a command that takes
        // its parts (wind.at, water.height, terrain.height: x, z).
        if (takes("x") && !given.contains("x")) {
            for (const char* alias : {"position", "at", "point"}) {
                if (takes(alias) || !given.contains(alias)) continue;
                const Json& v = given[alias];
                Json parts = Json::object();
                const char* axes[3] = {"x", "y", "z"};
                for (int k = 0; k < 3; ++k) {
                    if (v.is_array() && v.size() > static_cast<std::size_t>(k) && v[k].is_number()) parts[axes[k]] = v[k];
                    else if (v.is_object() && v.contains(axes[k])) parts[axes[k]] = v[axes[k]];
                }
                if (parts.empty()) continue;
                if (chosen != &adjusted) adjusted = given;
                for (const auto& [k, x] : parts.items()) if (takes(k)) adjusted[k] = x;
                adjusted.erase(alias);
                chosen = &adjusted;
                break;
            }
        }
        if (chosen->contains("id") && !chosen->contains("seq") && takes("seq") && !takes("id")) {
            // An event's id is its seq (events.why {id: 33}).
            if (chosen != &adjusted) adjusted = given;
            adjusted["seq"] = adjusted["id"];
            adjusted.erase("id");
            chosen = &adjusted;
        }
        if (chosen->contains("path") && !chosen->contains("entity") && takes("entity") && !takes("path")) {
            if (chosen != &adjusted) adjusted = given;
            adjusted["entity"] = adjusted["path"];
            adjusted.erase("path");
            chosen = &adjusted;
        }
        // And the other way: `entity` or `name` for a command that takes a path (world.find).
        for (const char* alias : {"entity", "name"}) {
            if (!chosen->contains(alias) || chosen->contains("path") || !takes("path") || takes(alias)) continue;
            if (chosen != &adjusted) adjusted = given;
            adjusted["path"] = adjusted[alias];
            adjusted.erase(alias);
            chosen = &adjusted;
        }
        // The other names a parameter is often given.
        if (name == "world.set" && !chosen->contains("value")) {
            for (const char* alias : {"values", "fields"}) {
                if (!chosen->contains(alias) || !(*chosen)[alias].is_object()) continue;
                if (chosen != &adjusted) adjusted = given;
                adjusted["value"] = adjusted[alias];
                adjusted.erase(alias);
                chosen = &adjusted;
                break;
            }
        }
        for (const char* alias : {"lines", "limit", "count"}) {
            if (name != "log.tail" || !chosen->contains(alias) || chosen->contains("n")) continue;
            if (chosen != &adjusted) adjusted = given;
            adjusted["n"] = adjusted[alias];
            adjusted.erase(alias);
            chosen = &adjusted;
        }
        // events.why {event: "campfire.stoked"} (or type): the latest event of that type, or of a type
        // it starts ("coin.").
        for (const char* alias : {"event", "type"}) {
            if (name != "events.why" || !world_ || !chosen->contains(alias) || !(*chosen)[alias].is_string() || chosen->contains("seq")) continue;
            const std::string prefix = (*chosen)[alias].get<std::string>();
            std::uint64_t seq = 0;
            for (const world::Event& e : world_->events().recent(100000)) if (e.type.starts_with(prefix)) seq = std::max(seq, e.seq);
            if (!seq) return fail("not_found", "events.why: no event of type {} in the log (events.histogram counts the types there are)", prefix);
            if (chosen != &adjusted) adjusted = given;
            adjusted["seq"] = seq;
            adjusted.erase(alias);
            chosen = &adjusted;
        }
        // world.set given the component the way world.spawn takes it ({entity, Transform: {...}}),
        // or a component's fields beside it ({entity, component: "Transform", position: {...}}).
        if (name == "world.set" && world_) {
            Json moved = Json::object(), fields = Json::object();
            const std::string comp = chosen->contains("component") && (*chosen)["component"].is_string() ? (*chosen)["component"].get<std::string>() : "";
            // A component's name in another case ("transform", "mesh_renderer") is that component.
            auto component_named = [&](const std::string& k) -> std::string {
                if (world_->known_component(k)) return k;
                std::string pascal;
                bool up = true;
                for (char c : k) {
                    if (c == '_' || c == '-') { up = true; continue; }
                    pascal += up ? static_cast<char>(std::toupper(static_cast<unsigned char>(c))) : c;
                    up = false;
                }
                return world_->known_component(pascal) ? pascal : std::string();
            };
            std::vector<std::pair<std::string, std::string>> named;   // the key given, the component
            for (const auto& [k, v] : chosen->items()) {
                if (takes(k) || k == "cause") continue;
                if (const std::string c = component_named(k); !c.empty() && v.is_object()) {
                    moved[c] = v;
                    named.emplace_back(k, c);
                } else if (!comp.empty() && !chosen->contains("value") && world_->has_field(comp, k)) fields[k] = v;
            }
            if (!moved.empty() || !fields.empty()) {
                if (chosen != &adjusted) adjusted = given;
                for (const auto& [k, c] : named) {
                    adjusted["components"][c] = moved[c];
                    adjusted.erase(k);
                }
                if (!fields.empty()) {
                    adjusted["value"] = fields;
                    for (const auto& [k, v] : fields.items()) adjusted.erase(k);
                }
                chosen = &adjusted;
            }
        }
        if (!command_params_open(h->params)) {
            for (const auto& [k, v] : chosen->items()) {
                if (k == "cause" || k == "quiet" || takes(k)) continue;   // quiet asks for less back: nothing to refuse where there is no less
                return fail("bad_args", "{} does not take '{}': {} (help {{command: \"{}\"}} says more)", name, k, h->params.empty() ? std::string("it takes no parameters") : "it takes " + std::string(h->params), name);
            }
        }
    }
    const Json& p = *chosen;
    // What reads the drawn frame or its camera sees the world as it is now: after headless ticks that
    // skipped drawing, the first such call draws. A script's render.stats (a game showing its frame
    // numbers each tick) describes the last frame drawn instead, so it does not undo the skipping.
    static constexpr std::string_view readers[] = {"capture", "render.stats", "render.pick", "render.ids", "render.visible", "render.compare", "render.views", "render.project", "render.unproject"};
    if (std::find(std::begin(readers), std::end(readers), name) != std::end(readers) && !(source == "script" && name == "render.stats")) POCKET_TRY_VOID(ensure_drawn());
    if (name.starts_with("world.")) return world_command(name.substr(6), p, source);
    if (name.starts_with("events.")) return events_command(name.substr(7), p, source);
    if (name.starts_with("recorder.")) return recorder_command(name.substr(9), p);
    if (name.starts_with("debug.")) return debug_command(name.substr(6), p);
    if (name == "render.post") {
        if (p.contains("effects")) return set_post_effects(p["effects"]);
        Json list = Json::array();
        for (const auto& e : renderer_->post_effects()) list.push_back(Json{{"name", e.name}, {"enabled", e.enabled}, {"params", Json(std::vector<float>(e.params.begin(), e.params.end()))}});
        return Json{{"effects", list}};
    }
    if (name == "render.views") {
        // The scene (or one entity and what is under it) seen from standard views at once, without
        // moving anything: a contact sheet an agent reads to check what it built from every side.
        const std::string path = opt<std::string>(p, "path", "");
        if (path.empty()) return fail("bad_args", "render.views needs a 'path' for the sheet (a .png)");
        std::vector<std::string> names;
        if (p.contains("views") && p["views"].is_array()) {
            for (const Json& v : p["views"]) if (v.is_string()) names.push_back(v.get<std::string>());
        } else {
            names = {"front", "right", "back", "left", "top", "perspective"};
        }
        // What to frame: the entity's bounds and its descendants', or every bound in the world.
        Vec3 lo{1e30f, 1e30f, 1e30f}, hi{-1e30f, -1e30f, -1e30f};
        bool any = false;
        auto grow = [&](world::EntityId id) {
            if (const auto* b = world_->try_get<world::Bounds>(id)) {
                lo = {std::min(lo.x, b->min.x), std::min(lo.y, b->min.y), std::min(lo.z, b->min.z)};
                hi = {std::max(hi.x, b->max.x), std::max(hi.y, b->max.y), std::max(hi.z, b->max.z)};
                any = true;
            }
        };
        world::EntityId focus = 0;
        if (p.contains("entity") && !p["entity"].is_null()) {
            focus = resolve_entity(p["entity"]);
            if (!world_->alive(focus)) return fail("no_such_entity", "no entity for {}", p["entity"].dump());
            world_->visit_all([&](world::EntityId id, world::EntityId, int) {
                for (world::EntityId a = id; a != 0; a = world_->parent(a)) if (a == focus) { grow(id); break; }
            });
            if (!any) if (const auto* t = world_->try_get<world::WorldTransform>(focus)) { lo = hi = t->position; any = true; }
        } else {
            world_->ecs().each([&](flecs::entity e, const world::Bounds&) { grow(e.id()); });
        }
        if (!any) return fail("nothing_to_see", "nothing with bounds to frame");
        const Vec3 center = (lo + hi) * 0.5f;
        const Vec3 half = (hi - lo) * 0.5f;
        const float radius = std::max(length(half), 0.25f);
        const float fov = 45.0f;
        const float tan_half = std::tan(fov * 0.5f * std::numbers::pi_v<float> / 180.0f);
        POCKET_TRY(frame_size, render_command("viewport", Json::object()));
        const float aspect = std::max(frame_size.value("pixels", Json::object()).value("w", 16.0f), 1.0f) / std::max(frame_size.value("pixels", Json::object()).value("h", 9.0f), 1.0f);
        struct View { const char* name; Vec3 dir; };
        const View known[] = {{"front", {0, 0, 1}}, {"back", {0, 0, -1}}, {"right", {1, 0, 0}}, {"left", {-1, 0, 0}}, {"top", {0, 1, 0}}, {"bottom", {0, -1, 0}}, {"perspective", normalize(Vec3{1, 0.8f, 1})}};
        std::vector<rhi::Image> shots;
        Json list = Json::array();
        for (const std::string& n : names) {
            const View* v = nullptr;
            for (const View& k : known) if (n == k.name) v = &k;
            if (!v) return fail("bad_args", "no view named '{}' (front, back, right, left, top, bottom, perspective)", n);
            const Vec3 up = std::fabs(v->dir.y) > 0.99f ? Vec3{0, 0, -1} : Vec3{0, 1, 0};
            // Far enough that the box's corners fit the view: each corner's reach across the view,
            // over the tangent of the half angle, plus how near it comes along the view.
            const Vec3 right = normalize(cross(v->dir * -1.0f, up)), true_up = cross(right, v->dir * -1.0f);
            float distance = 0;
            for (int c = 0; c < 8; ++c) {
                const Vec3 corner{(c & 1) ? half.x : -half.x, (c & 2) ? half.y : -half.y, (c & 4) ? half.z : -half.z};
                const float along = dot(corner, v->dir);
                const float need = std::max(std::fabs(dot(corner, right)) / (tan_half * aspect), std::fabs(dot(corner, true_up)) / tan_half);
                distance = std::max(distance, need + along);
            }
            distance = std::max(distance * 1.08f, radius * 0.5f);
            const Vec3 eye = center + v->dir * distance;
            renderer_->set_view(renderer::ViewOverride{eye, center, up, fov});
            POCKET_TRY_VOID(render_frame());
            POCKET_TRY(img, device_->capture());
            shots.push_back(std::move(img));
            list.push_back(Json{{"view", n}, {"eye", Json{{"x", eye.x}, {"y", eye.y}, {"z", eye.z}}}});
        }
        renderer_->set_view(std::nullopt);
        POCKET_TRY_VOID(render_frame());   // back to the scene's own camera
        // The sheet: the shots in rows of three (two for up to four), each labelled by its place.
        const std::uint32_t cols = shots.size() <= 4 ? std::min<std::uint32_t>(2, static_cast<std::uint32_t>(shots.size())) : 3;
        const std::uint32_t rows = static_cast<std::uint32_t>((shots.size() + cols - 1) / cols);
        const std::uint32_t w = shots[0].width, h = shots[0].height;
        rhi::Image sheet;
        sheet.width = w * cols;
        sheet.height = h * rows;
        sheet.rgba.assign(static_cast<std::size_t>(sheet.width) * sheet.height * 4, 0);
        for (std::size_t k = 0; k < shots.size(); ++k) {
            const std::uint32_t ox = static_cast<std::uint32_t>(k % cols) * w, oy = static_cast<std::uint32_t>(k / cols) * h;
            for (std::uint32_t y = 0; y < h && y < shots[k].height; ++y) {
                std::memcpy(&sheet.rgba[((static_cast<std::size_t>(oy) + y) * sheet.width + ox) * 4], &shots[k].rgba[static_cast<std::size_t>(y) * shots[k].width * 4], static_cast<std::size_t>(std::min(w, shots[k].width)) * 4);
            }
        }
        POCKET_TRY(out, output_path(options_.project_dir, path));
        POCKET_TRY_VOID(write_png(out, sheet));
        return Json{{"path", out.string()}, {"views", list}, {"columns", cols}, {"rows", rows}, {"width", sheet.width}, {"height", sheet.height},
                    {"center", Json{{"x", center.x}, {"y", center.y}, {"z", center.z}}}, {"radius", radius}, {"entity", focus ? Json(world_->path(focus)) : Json(nullptr)}};
    }
    if (name.starts_with("render.")) return render_command(name.substr(7), p);
    if (name.starts_with("physics.")) return physics_command(name.substr(8), p);
    if (name.starts_with("physics2d.")) return physics2d_command(name.substr(10), p);
    if (name == "combat.hitscan") return hitscan_command(p);
    if (name.starts_with("nav.")) return nav_command(name.substr(4), p);
    if (name.starts_with("env.")) return env_command(name.substr(4), p);
    if (name.starts_with("sprite.")) return sprite_command(name.substr(7), p);
    if (name.starts_with("particles.")) return particles_command(name.substr(10), p);
    if (name.starts_with("animation.")) return animation_command(name.substr(10), p);
    if (name.starts_with("tilemap.")) return tilemap_command(name.substr(8), p);
    if (name.starts_with("terrain.")) return terrain_command(name.substr(8), p);
    if (name.starts_with("water.")) return water_command(name.substr(6), p);
    if (name == "wind.at") {
        // The air's motion at a world point now (docs/design/wind.md).
        if (!p.contains("x") || !p.contains("z")) return fail("bad_args", "wind.at needs x and z");
        const world::WindField wf = world::wind_field(*world_);
        const float x = opt<float>(p, "x", 0.0f), z = opt<float>(p, "z", 0.0f), t = static_cast<float>(world_->seconds());
        const Vec3 v = world::wind_velocity(wf, x, z, t);
        Json j{{"entity", wf.on ? Json(wf.entity) : Json(nullptr)}, {"velocity", Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}}, {"speed", length(v)}};
        if (wf.on) {
            j["path"] = world_->path(wf.entity);
            j["gust"] = world::wind_gust(wf, x, z, t);
            j["direction"] = Json{{"x", wf.dir_x}, {"z", wf.dir_z}};
        }
        return j;
    }
    if (name.starts_with("timeline.")) return timeline_command(name.substr(9), p);
    if (name.starts_with("locale.")) return locale_command(name.substr(7), p);
    if (name.starts_with("net.")) return net_command(name.substr(4), p);
    if (name == "scatter.copies") {
        // Where a Scatter's copies stand (docs/design/terrain.md, Scattering): their points, sizes and shades.
        update_terrains();
        if (!p.contains("entity")) return fail("bad_args", "scatter.copies needs the entity with the Scatter");
        const world::EntityId id = resolve_entity(p["entity"]);
        if (!id || !world_->ecs().entity(id).has<world::Scatter>()) return fail("no_scatter", "{} has no Scatter", p["entity"].dump());
        const auto* copies = world_->derived_instances(id);
        const std::size_t limit = static_cast<std::size_t>(std::clamp(opt<int>(p, "limit", 100), 0, 20000));
        Json arr = Json::array();
        // With collide on, each copy's capsule as the physics has it (world units: collide and
        // collide_height times the copy's own size), so what the scene asked for can be checked.
        const world::Scatter& sc = *world_->ecs().entity(id).try_get<world::Scatter>();
        const auto* own = world_->try_get<world::WorldTransform>(id);
        const float own_x = own ? std::max(std::fabs(own->scale.x), 1e-6f) : 1.0f, own_y = own ? std::max(std::fabs(own->scale.y), 1e-6f) : 1.0f;
        if (copies) {
            for (std::size_t k = 0; k < copies->size() && k < limit; ++k) {
                const Mat4& m = (*copies)[k].model;
                Json c{{"x", m.at(3, 0)}, {"y", m.at(3, 1)}, {"z", m.at(3, 2)}, {"size", length(Vec3{m.at(1, 0), m.at(1, 1), m.at(1, 2)})}, {"shade", (*copies)[k].shade}};
                if (sc.collide > 0) {
                    const float r = std::max(sc.collide * length(Vec3{m.at(0, 0), m.at(0, 1), m.at(0, 2)}) / own_x, 0.01f);
                    c["radius"] = r;
                    c["height"] = std::max(sc.collide_height * length(Vec3{m.at(1, 0), m.at(1, 1), m.at(1, 2)}) / own_y, 2 * r);
                }
                arr.push_back(std::move(c));
            }
        }
        return Json{{"entity", id}, {"placed", copies ? copies->size() : 0}, {"copies", std::move(arr)}};
    }
    if (name.starts_with("assets.")) return assets_command(name.substr(7), p);
    if (name.starts_with("audio.")) return audio_command(name.substr(6), p);
    if (name.starts_with("input.")) return input_command(name.substr(6), p);
    if (name.starts_with("save.")) return save_command(name.substr(5), p);
    if (name.starts_with("mesh.")) return mesh_command(name.substr(5), p);
    if (name.starts_with("path.")) {
        // Where on a Path a distance is, and how far along it a point lies (docs/design/paths.md).
        const world::EntityId id = resolve_entity(p.value("entity", Json()));
        if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", p.value("entity", Json()).dump());
        const world::PathCurve* c = paths_.curve(*world_, id);
        if (!c) return fail("no_path", "{} has no Path of two points or more", world_->path(id));
        auto v3 = [](Vec3 v) { return Json{{"x", v.x}, {"y", v.y}, {"z", v.z}}; };
        if (name == "path.info") return Json{{"length", c->length}, {"closed", c->closed}, {"start", v3(c->sample(0))}, {"end", v3(c->sample(c->length))}};
        if (name == "path.sample") {
            float d = static_cast<float>(opt<double>(p, "distance", 0.0));
            if (p.contains("fraction")) d = static_cast<float>(opt<double>(p, "fraction", 0.0)) * c->length;
            Vec3 dir;
            const Vec3 at = c->sample(d, &dir);
            return Json{{"point", v3(at)}, {"direction", v3(dir)}, {"distance", d}, {"length", c->length}};
        }
        if (name == "path.nearest") {
            const Vec3 q = vec3_of(p.value("point", Json()), Vec3{0, 0, 0});
            Vec3 on;
            const float d = c->nearest(q, &on);
            return Json{{"distance", d}, {"point", v3(on)}, {"away", length(q - on)}, {"length", c->length}};
        }
        return fail("unknown_command", "{} is not a command (path.info, path.sample, path.nearest)", name);
    }
    if (name == "window.info" || name == "window.set") {
        // The game's window (docs/design/input.md, The window): fullscreen, size, title.
        if (!platform_) return fail("no_window", "no platform");
        if (name == "window.set") {
            if (p.contains("fullscreen")) platform_->set_fullscreen(opt<bool>(p, "fullscreen", false));
            if (p.contains("width") || p.contains("height")) {
                const auto now = platform_->window_state();
                const int w = opt<int>(p, "width", now.width), h = opt<int>(p, "height", now.height);
                if (w < 64 || h < 64 || w > 16384 || h > 16384) return fail("bad_args", "width and height are points, 64 to 16384");
                platform_->set_window_size(w, h);
            }
            if (p.contains("title")) platform_->set_title(opt<std::string>(p, "title", ""));
        }
        const auto s = platform_->window_state();
        return Json{{"width", s.width}, {"height", s.height}, {"fullscreen", s.fullscreen}, {"title", s.title}, {"pixel_width", platform_->pixel_width()}, {"pixel_height", platform_->pixel_height()}, {"pixel_density", platform_->pixel_density()}, {"headless", platform_->headless()}};
    }
    if (name.starts_with("ui.")) return ui_command(name.substr(3), p);
    if (name.starts_with("script.")) return script_command(name.substr(7), p);
    if (name.starts_with("project.")) return project_command(name.substr(8), p);
    if (name == "perf") {
        Json j = perf();
        if (opt<bool>(p, "reset", false)) {
            // The averages start over: what a stretch of play costs, measured from here.
            for (PhaseStats* s : {&perf_frame_, &perf_poll_, &perf_tick_, &perf_script_, &perf_physics_, &perf_world_, &perf_state_, &perf_render_}) *s = PhaseStats{};
            for (PhaseStats& s : perf_systems_) s = PhaseStats{};
        }
        return j;
    }
    if (name == "state") {
        if (p.contains("keys") && !p["keys"].is_null()) {
            // Only the exposed values asked for (and where the run is), for one who watches a few.
            if (!p["keys"].is_array()) return fail("bad_args", "keys is a list of exposed value names (state without it lists them)");
            Json picked = Json::object();
            Json missing = Json::array();
            for (const Json& k : p["keys"]) {
                const std::string key = k.is_string() ? k.get<std::string>() : k.dump();
                if (last_state_.is_object() && last_state_.contains(key)) picked[key] = last_state_[key];
                else missing.push_back(key);
            }
            Json j{{"tick", clock_.tick}, {"state", picked}, {"paused", paused_}, {"ok", errors_.empty()}};
            if (run_start_ > 0) j["run_tick"] = clock_.tick - run_start_;
            if (!missing.empty()) j["missing"] = missing;
            if (!errors_.empty()) j["errors"] = Json::array({errors_.back()});
            return j;
        }
        Json j;
        j["tick"] = clock_.tick;
        if (run_start_ > 0) j["run_tick"] = clock_.tick - run_start_;   // the scripts' t.tick since the last restart
        j["frames"] = frames_;
        j["sim_seconds"] = clock_.sim_seconds();
        j["state"] = last_state_;
        j["state_hash"] = hex64(hasher_.digest());
        j["world_hash"] = hex64(world_->hash());
        j["paused"] = paused_;
        j["ok"] = errors_.empty();
        if (!errors_.empty()) {
            // What stopped the simulation (the last few), so the one driving it can fix it.
            Json errs = Json::array();
            for (std::size_t i = errors_.size() > 3 ? errors_.size() - 3 : 0; i < errors_.size(); ++i) errs.push_back(errors_[i]);
            j["errors"] = errs;
            j["hint"] = "a script error stopped the simulation: fix the script, then project.apply (bundles and reloads; script.reload keeps the world)";
        }
        return j;
    }
    if (name == "step") {
        if (!errors_.empty()) return fail("script_error", "the simulation is stopped by a script error: {}; fix it and project.apply (state shows the errors)", errors_.back().value("message", std::string("unknown")));
        int ticks = opt<int>(p, "ticks", 1);
        if (ticks < 0 || ticks > 100000) return fail("bad_args", "ticks must be in [0, 100000]");
        // Headless, only the last tick is drawn unless every frame is asked for (render: "each"):
        // what an agent looks at afterwards is that frame, and drawing the others costs most of a
        // long step. Effects that build up over drawn frames (TAA, auto exposure, probe bounces)
        // want "each".
        const std::string render = opt<std::string>(p, "render", "last");
        if (render != "last" && render != "each") return fail("bad_args", "render is \"last\" (draw the last tick only, headless) or \"each\"");
        // `until`: stop at the first tick after which something holds, so "walk right until the
        // coin is taken" is one call: an event (by type or type prefix), an exposed state value or
        // a component field compared with equals, above, below, at_least, at_most or changes.
        std::function<bool(Json&)> met;
        std::uint64_t seen = world_->events().last_seq();
        // A value to read after every tick: an exposed one by name, or an entity's component field
        // ("Player:Transform.position.y", or {entity, component, field}).
        auto reader = [this](const Json& spec) -> Result<std::function<Result<Json>()>> {
            std::string key, comp, field;
            Json entity;
            if (spec.is_string()) {
                const std::string s = spec.get<std::string>();
                const std::size_t colon = s.rfind(':');
                if (colon == std::string::npos) {
                    key = s;
                } else {
                    entity = s.substr(0, colon);
                    const std::string rest = s.substr(colon + 1);
                    const std::size_t dot = rest.find('.');
                    comp = rest.substr(0, dot);
                    field = dot == std::string::npos ? "" : rest.substr(dot + 1);
                }
            } else if (spec.is_object() && spec.contains("state")) {
                key = spec["state"].is_string() ? spec["state"].get<std::string>() : "";
                if (key.empty()) return fail("bad_args", "state names an exposed value (state lists them)");
            } else if (spec.is_object() && spec.contains("entity") && spec.contains("component")) {
                entity = spec["entity"];
                comp = spec["component"].is_string() ? spec["component"].get<std::string>() : "";
                field = spec.contains("field") && spec["field"].is_string() ? spec["field"].get<std::string>() : "";
            } else {
                return fail("bad_args", "a value to read is an exposed name (\"score\"), \"Entity:Component.field\", or {{state}} or {{entity, component, field}}");
            }
            if (!key.empty()) {
                return std::function<Result<Json>()>([this, key]() -> Result<Json> { return last_state_.is_object() && last_state_.contains(key) ? last_state_[key] : Json(); });
            }
            const world::EntityId id = resolve_entity(entity);
            if (!world_->alive(id)) return fail("no_such_entity", "no entity for {}", entity.dump());
            if (!world_->known_component(comp)) return fail("unknown_component", "no component named '{}' (world.components lists them)", comp);
            return std::function<Result<Json>()>([this, id, comp, field]() -> Result<Json> {
                if (!world_->alive(id) || !world_->has(id, comp)) return Json();
                POCKET_TRY(v, world_->get(id, comp));
                Json at = v;
                std::size_t start = 0;
                while (!field.empty() && start <= field.size()) {
                    const std::size_t dot = field.find('.', start);
                    const std::string part = field.substr(start, dot == std::string::npos ? std::string::npos : dot - start);
                    if (!at.is_object() || !at.contains(part)) return Json();
                    at = at[part];
                    if (dot == std::string::npos) break;
                    start = dot + 1;
                }
                return at;
            });
        };
        // `watch`: values followed through the step, each answered with its first and last value,
        // its least and greatest (numbers) and the ticks they came at, and how often it changed;
        // `every` adds the value at every that many ticks. One call instead of a loop of steps.
        struct Watch {
            std::string name;
            std::function<Result<Json>()> read;
            Json first, last, min, max, series = Json::array();
            std::uint64_t min_tick = 0, max_tick = 0;
            int changes = 0;
        };
        std::vector<Watch> watches;
        const int every = opt<int>(p, "every", 0);
        if (every < 0) return fail("bad_args", "every is a number of ticks between the values kept (0: none)");
        if (p.contains("watch") && !p["watch"].is_null()) {
            const Json list = p["watch"].is_array() ? p["watch"] : Json::array({p["watch"]});
            if (list.size() > 32) return fail("bad_args", "watch at most 32 values at once");
            for (const Json& spec : list) {
                POCKET_TRY(read, reader(spec));
                Watch w;
                w.name = spec.is_string() ? spec.get<std::string>() : spec.dump();
                w.read = std::move(read);
                POCKET_TRY(v, w.read());
                w.first = w.last = v;
                if (v.is_number()) { w.min = w.max = v; w.min_tick = w.max_tick = clock_.tick > 0 ? clock_.tick - 1 : 0; }
                watches.push_back(std::move(w));
            }
        }
        if (p.contains("until") && !p["until"].is_null()) {
            // Text is a world.query condition: until some entity meets it ("Plot.stage == 3").
            const Json u = p["until"].is_string() ? Json{{"where", p["until"]}} : p["until"];
            if (!u.is_object()) return fail("bad_args", "until is {{event}}, {{state | entity, component, field}} with equals, above, below, at_least, at_most or changes, or {{where}} (a world.query condition, or its text alone)");
            if (u.contains("where")) {
                // Until enough entities meet a condition on their fields (docs/design/world-model.md,
                // world.query): one by default, or count: n or {at_least: n} (equals, above, below, at_most).
                POCKET_TRY(conditions, world::parse_where(u["where"]));
                world::QueryOptions q;
                q.where = std::move(conditions);
                q.with = string_list(u, "with");
                q.name = opt<std::string>(u, "name", "");
                std::string cop = "at_least";
                double want = 1;
                if (u.contains("count")) {
                    const Json& c = u["count"];
                    if (c.is_number()) { cop = "equals"; want = c.get<double>(); }
                    else if (c.is_object() && c.size() == 1 && c.begin().value().is_number()) { cop = c.begin().key(); want = c.begin().value().get<double>(); }
                    else return fail("bad_args", "until.count is a number or {{at_least: n}} (equals, above, below, at_most)");
                    if (cop != "equals" && cop != "above" && cop != "below" && cop != "at_least" && cop != "at_most") return fail("bad_args", "until.count compares with equals, above, below, at_least or at_most, not {}", cop);
                }
                const Json probe = world_->query(q);
                if (probe.contains("error")) return fail("bad_query", "until: {}", probe["error"].get<std::string>());
                met = [this, q, cop, want](Json& detail) {
                    const Json r = world_->query(q);
                    const double n = r.value("count", 0);
                    const bool ok = cop == "equals" ? n == want : cop == "above" ? n > want : cop == "below" ? n < want : cop == "at_least" ? n >= want : n <= want;
                    if (!ok) return false;
                    detail["count"] = n;
                    Json paths = Json::array();
                    for (const Json& e : r["entities"]) { if (paths.size() >= 8) break; paths.push_back(e["path"]); }
                    detail["entities"] = paths;
                    return true;
                };
            } else if (u.contains("event")) {
                if (!u["event"].is_string()) return fail("bad_args", "until.event is an event type or a prefix of one (\"coin.\")");
                const std::string prefix = u["event"].get<std::string>();
                met = [this, prefix, &seen](Json& detail) {
                    const auto found = world_->events().since(seen, 1, prefix);
                    seen = world_->events().last_seq();
                    if (found.empty()) return false;
                    detail["event"] = world::event_to_json(found.front());
                    return true;
                };
            } else {
                if (!u.contains("state") && !(u.contains("entity") && u.contains("component") && u.contains("field"))) {
                    return fail("bad_args", "until needs event, state, or entity with component and field");
                }
                Json spec = u.contains("state") ? Json{{"state", u["state"]}} : Json{{"entity", u["entity"]}, {"component", u["component"]}, {"field", u["field"]}};
                auto made = reader(spec);
                if (!made) return fail(made.error().code, "until: {}", made.error().message);
                std::function<Result<Json>()> read = std::move(*made);
                const char* ops[] = {"equals", "above", "below", "at_least", "at_most", "changes"};
                std::string op, given;
                for (const char* o : ops) if (u.contains(o)) op = given = o;
                // The other names a comparison is often given (value, is, eq, gt, lt, ...).
                static const std::pair<const char*, const char*> aliases[] = {{"value", "equals"}, {"is", "equals"}, {"eq", "equals"}, {"equal", "equals"}, {"gt", "above"}, {"greater", "above"}, {"lt", "below"}, {"less", "below"},
                                                                             {"gte", "at_least"}, {"min", "at_least"}, {"lte", "at_most"}, {"max", "at_most"}, {"changed", "changes"}};
                for (const auto& [alias, to] : aliases) if (op.empty() && u.contains(alias)) { op = to; given = alias; }
                if (op.empty()) return fail("bad_args", "until compares with one of equals, above, below, at_least, at_most, changes");
                const Json target = u[given];
                if (op != "equals" && op != "changes" && !target.is_number()) return fail("bad_args", "until.{} is a number", op);
                POCKET_TRY(first, read());
                met = [read, op, target, first](Json& detail) {
                    auto now = read();
                    if (!now) return false;
                    const Json& v = *now;
                    bool hit = false;
                    if (op == "equals") hit = v == target;
                    else if (op == "changes") hit = v != first;
                    else if (v.is_number()) {
                        const double x = v.get<double>(), t = target.get<double>();
                        hit = op == "above" ? x > t : op == "below" ? x < t : op == "at_least" ? x >= t : x <= t;
                    }
                    if (hit) detail["value"] = v;
                    return hit;
                };
            }
        }
        stepping_ = true;
        Json until_detail = Json::object();
        int ran = 0;
        bool hit = false;
        for (int i = 0; i < ticks && !hit; ++i) {
            bool was_paused = paused_;
            paused_ = false;
            skip_render_ = options_.headless && render == "last" && i + 1 < ticks;
            auto r = frame();
            skip_render_ = false;
            if (!r) { paused_ = was_paused; stepping_ = false; return fail(r.error()); }
            paused_ = was_paused;
            ++ran;
            for (Watch& w : watches) {
                auto v = w.read();
                if (!v) continue;
                if (*v != w.last) ++w.changes;
                w.last = *v;
                if (v->is_number()) {
                    const double x = v->get<double>();
                    if (w.min.is_null() || x < w.min.get<double>()) { w.min = *v; w.min_tick = clock_.tick - 1; }
                    if (w.max.is_null() || x > w.max.get<double>()) { w.max = *v; w.max_tick = clock_.tick - 1; }
                }
                if (every > 0 && ran % every == 0 && w.series.size() < 1000) w.series.push_back(Json::array({clock_.tick - 1, *v}));
            }
            if (met && met(until_detail)) hit = true;
        }
        stepping_ = false;
        POCKET_TRY(out, command("state", p.contains("keys") ? Json{{"keys", p["keys"]}} : Json::object(), source));
        if (!watches.empty()) {
            Json seen_values = Json::object();
            for (const Watch& w : watches) {
                Json o{{"first", w.first}, {"last", w.last}, {"changes", w.changes}};
                if (!w.min.is_null()) {
                    o["min"] = w.min;
                    o["min_tick"] = w.min_tick;
                    o["max"] = w.max;
                    o["max_tick"] = w.max_tick;
                }
                if (every > 0) o["series"] = w.series;
                seen_values[w.name] = o;
            }
            out["watch"] = seen_values;
        }
        if (met) {
            // Whether it came, at which tick, and what was seen; a step that ran out says so.
            until_detail["met"] = hit;
            until_detail["ticks"] = ran;
            if (hit) until_detail["tick"] = clock_.tick - 1;
            out["until"] = until_detail;
        }
        return out;
    }
    if (name == "pause") { paused_ = true; return Json{{"paused", true}}; }
    if (name == "resume") {
        if (!errors_.empty()) return fail("script_error", "the simulation is stopped by a script error: {}; fix it and project.apply", errors_.back().value("message", std::string("unknown")));
        paused_ = false;
        return Json{{"paused", false}};
    }
    if (name == "time.scale") {
        // Slow motion, fast forward and hit-stops: simulation seconds per real second, 0 to 8 (the
        // most ticks a frame runs), held for `seconds` of real time then 1 again, or until the next call.
        if (p.contains("scale")) {
            if (!p["scale"].is_number()) return fail("bad_args", "scale must be a number from 0 to 8");
            time_scale_ = std::clamp(p["scale"].get<double>(), 0.0, 8.0);
            scale_left_ = time_scale_ == 1.0 ? 0.0 : std::max(opt<double>(p, "seconds", 0.0), 0.0);
        }
        return Json{{"scale", time_scale_}, {"seconds", scale_left_}};
    }
    if (name == "quit") { quit_ = true; return Json{{"ok", true}}; }
    if (name == "capture.gif") {
        // An animated picture of play (docs/mcp.md, capture.gif): `seconds` of ticks stepped and
        // drawn, a frame every `every` ticks, made `width` pixels across, written as a looping GIF.
        const std::string path = opt<std::string>(p, "path", "");
        if (path.empty() || !path.ends_with(".gif")) return fail("bad_args", "capture.gif needs a path ending in .gif (project-relative, e.g. \"shots/play.gif\")");
        const double seconds = std::clamp(opt<double>(p, "seconds", 2.0), 0.05, 30.0);
        const int every = std::clamp(opt<int>(p, "every", 2), 1, 60);
        const int ticks = std::max(1, static_cast<int>(std::lround(seconds / clock_.tick_seconds)));
        std::vector<std::vector<std::uint8_t>> frames;
        int w = 0, h = 0, to_w = 0, to_h = 0;
        for (int t = 0; t < ticks; ++t) {
            POCKET_TRY(stepped, run_command("step", Json{{"ticks", 1}, {"render", "each"}}, source));
            (void)stepped;
            if (t % every != 0) continue;
            POCKET_TRY(img, device_->capture());
            if (w == 0) {
                w = static_cast<int>(img.width);
                h = static_cast<int>(img.height);
                to_w = std::min(w, std::clamp(opt<int>(p, "width", 480), 32, 1920));
                to_h = std::max(1, static_cast<int>(std::lround(static_cast<double>(h) * to_w / std::max(w, 1))));
            }
            frames.push_back(shrink_rgba(img.rgba, w, h, to_w, to_h));
        }
        const int delay = std::max(2, static_cast<int>(std::lround(every * clock_.tick_seconds * 100.0)));
        const std::string gif = encode_gif(frames, to_w, to_h, delay);
        POCKET_TRY(out, output_path(options_.project_dir, path));
        POCKET_TRY_VOID(fs::write_bytes(out, gif.data(), gif.size()));
        return Json{{"path", out.string()}, {"frames", frames.size()}, {"width", to_w}, {"height", to_h}, {"bytes", gif.size()}, {"seconds", ticks * clock_.tick_seconds}, {"tick", clock_.tick}};
    }
    if (name == "capture") {
        std::string path = opt<std::string>(p, "path", "");
        POCKET_TRY(img, device_->capture());
        Json c;
        c["width"] = img.width;
        c["height"] = img.height;
        std::size_t center = (static_cast<std::size_t>(img.height / 2) * img.width + img.width / 2) * 4;
        c["center_pixel"] = Json::array({img.rgba[center], img.rgba[center + 1], img.rgba[center + 2], img.rgba[center + 3]});
        c["corner_pixel"] = Json::array({img.rgba[0], img.rgba[1], img.rgba[2], img.rgba[3]});
        // The pixels asked for: `pixel: {x, y}` or `pixels: [{x, y}, ...]`, in the frame's pixels, clamped to it.
        auto pixel_at = [&](const Json& pt) {
            const std::uint32_t x = static_cast<std::uint32_t>(std::clamp(static_cast<int>(opt<double>(pt, "x", 0)), 0, static_cast<int>(img.width) - 1));
            const std::uint32_t y = static_cast<std::uint32_t>(std::clamp(static_cast<int>(opt<double>(pt, "y", 0)), 0, static_cast<int>(img.height) - 1));
            const std::size_t at = (static_cast<std::size_t>(y) * img.width + x) * 4;
            return Json::array({img.rgba[at], img.rgba[at + 1], img.rgba[at + 2], img.rgba[at + 3]});
        };
        if (p.contains("pixel") && p["pixel"].is_object()) c["pixel"] = pixel_at(p["pixel"]);
        if (p.contains("pixels") && p["pixels"].is_array()) {
            Json arr = Json::array();
            for (const Json& pt : p["pixels"]) if (pt.is_object()) arr.push_back(pixel_at(pt));
            c["pixels"] = arr;
        }
        if (!path.empty()) {
            POCKET_TRY(out, output_path(options_.project_dir, path));
            POCKET_TRY_VOID(write_png(out, img));
            c["path"] = out.string();
        }
        std::string ids_path = opt<std::string>(p, "ids", "");
        if (!ids_path.empty()) {
            POCKET_TRY(idj, render_command("ids", Json{{"path", ids_path}}));
            c["ids"] = idj;
        }
        if (p.contains("ascii")) {
            // The frame in characters for a model that reads (" .:-=+*#%@" dark to light) and in
            // colour letters, with the frame's colours by name.
            const int w = p["ascii"].is_boolean() ? (p["ascii"].get<bool>() ? 64 : 0) : opt<int>(p, "ascii", 0);
            if (w > 0) {
                assets::Image frame_img;
                frame_img.width = img.width;
                frame_img.height = img.height;
                frame_img.rgba = img.rgba;
                c["look"] = frame_img.look(w, true);
            }
        }
        c["render"] = renderer_->describe();
        capture_info_ = c;
        return c;
    }
    if (name == "log.tail") {
        Json arr = Json::array();
        auto min_level = log::level_from_name(opt<std::string>(p, "level", "trace"));
        for (auto& r : log::global().recent(static_cast<std::size_t>(opt<int>(p, "n", 50)), min_level)) arr.push_back(log::to_json(r));
        return arr;
    }
    if (name == "transcript") {
        world::TranscriptOptions to;
        to.since_tick = static_cast<std::int64_t>(opt<double>(p, "since_tick", 0));
        to.until_tick = static_cast<std::int64_t>(opt<double>(p, "until_tick", -1));
        to.max_lines = opt<int>(p, "max_lines", 40);
        to.tolerance = opt<double>(p, "tolerance", 1e-4);
        to.debounce = opt<int>(p, "debounce", 3);
        return world::build_transcript(state_history_, world_->events(), to).to_json();
    }
    if (name == "report") return report();
    // A command's family is what comes before its first dot (world, render, ...; the session's own,
    // such as step and state, are their own); search matches the name, parameters or summary.
    auto lower = [](std::string s) { for (char& c : s) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c))); return s; };
    const std::string family = opt<std::string>(p, "family", ""), search = lower(opt<std::string>(p, "search", ""));
    auto wanted = [&](const CommandHelp& h) {
        const std::string n(h.name);
        if (!family.empty() && n.substr(0, n.find('.')) != family) return false;
        if (!search.empty() && lower(n + " " + std::string(h.params) + " " + std::string(h.summary)).find(search) == std::string::npos) return false;
        return true;
    };
    if (name == "help") {
        // How to call a command, from the engine itself.
        if (p.contains("sdk")) {
            // The SDK's exports: one by name ("timer.after"), or every one whose name starts with
            // or contains the text ("timer", "Hit"), as signature and first sentence.
            const std::string q = p["sdk"].is_string() ? p["sdk"].get<std::string>() : "";
            auto entry = [](const SdkHelp& s) { return Json{{"name", s.name}, {"signature", s.signature}, {"doc", s.doc}, {"file", s.file}}; };
            for (const SdkHelp& s : sdk_helps()) if (s.name == q) return entry(s);
            if (q.empty()) {
                // The parts, each with its exports' names.
                Json parts = Json::object();
                for (const SdkHelp& s : sdk_helps()) {
                    std::string part(s.file);
                    part = part.substr(part.rfind('/') + 1);
                    part = part.substr(0, part.find('.'));
                    parts[part].push_back(std::string(s.name));
                }
                return Json{{"parts", parts}, {"note", "help {sdk: name} for one export's signature and documentation; a part's name lists its exports"}};
            }
            std::string lower = q;
            for (char& c : lower) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
            Json found = Json::array();
            for (const SdkHelp& s : sdk_helps()) {
                std::string n(s.name);
                for (char& c : n) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
                if (n.find(lower) != std::string::npos) found.push_back(Json{{"signature", s.signature}, {"doc", s.doc}});
                if (found.size() >= 60) break;
            }
            if (found.empty()) return fail("unknown_export", "nothing in the SDK is named like '{}' (help {{sdk: \"\"}} lists the exports by part, docs/generated/sdk.md has them all)", q);
            return Json{{"exports", found}};
        }
        const std::string which = opt<std::string>(p, "command", "");
        if (which.empty()) {
            Json all = Json::array();
            for (const CommandHelp& h : command_helps()) if (wanted(h)) all.push_back(command_help_json(h));
            return Json{{"commands", all}, {"note", "every command also answers help {command} on its own; parameters with ? are optional, a | b are alternatives; family or search narrows the list"}};
        }
        if (const CommandHelp* h = command_help(which)) return command_help_json(*h);
        if (which == "world.add") {
            Json j = command_help_json(*command_help("world.set"));
            j["note"] = "world.add is world.set by another name: it adds the component when the entity has none";
            return j;
        }
        return fail("unknown_command", "no command named '{}'{}", which, command_suggestions(which).empty() ? std::string() : "; did you mean " + Json(command_suggestions(which)).dump() + "?");
    }
    if (name == "commands" && opt<bool>(p, "text", false) && family.empty() && search.empty()) {
        // Without a family or a search: the names by family, an index a fifth the size of every
        // usage line (an agent's context carries what it reads for the rest of its task).
        std::vector<std::pair<std::string, std::vector<std::string>>> families;
        for (const CommandHelp& h : command_helps()) {
            const std::string n(h.name);
            const std::size_t dot = n.find('.');
            const std::string fam = dot == std::string::npos ? "session" : n.substr(0, dot);
            auto it = std::find_if(families.begin(), families.end(), [&](const auto& f) { return f.first == fam; });
            if (it == families.end()) it = families.insert(families.end(), {fam, {}});
            it->second.push_back(dot == std::string::npos ? n : n.substr(dot + 1));
        }
        std::string text;
        for (const auto& [fam, names] : families) {
            text += fam == "session" ? std::string("(no family)") : fam;
            text += ": ";
            for (std::size_t i = 0; i < names.size(); ++i) text += (i ? " " : "") + names[i];
            text += "\n";
        }
        return Json{{"text", text + "(family {name} or search {word} lists those with their parameters and use; help {command} has a command's full description)\n"}};
    }
    if (name == "commands" && opt<bool>(p, "text", false)) {
        // One line a command, the compact form for an agent's context: its usage and the first
        // clause of its summary (help {command} has the rest).
        std::string text;
        for (const CommandHelp& h : command_helps()) {
            if (!wanted(h)) continue;
            std::string what(h.summary);
            const std::size_t end = std::min({what.find(". "), what.find("; "), what.find(": ")});
            if (end != std::string::npos) what = what.substr(0, end);
            if (what.size() > 80) what = what.substr(0, 77) + "...";
            text += command_help_json(h)["usage"].get<std::string>() + " - " + what + "\n";
        }
        return Json{{"text", text + "(help {command} for any command's full description)\n"}};
    }
    if (name == "commands" && opt<bool>(p, "usage", false)) {
        Json all = Json::array();
        for (const CommandHelp& h : command_helps()) {
            if (!wanted(h)) continue;
            Json j = command_help_json(h);
            all.push_back(Json{{"usage", j["usage"]}, {"summary", j["summary"]}});
        }
        return all;
    }
    if (name == "commands" && (!family.empty() || !search.empty())) {
        Json names = Json::array();
        for (const CommandHelp& h : command_helps()) if (wanted(h)) names.push_back(std::string(h.name));
        return names;
    }
    if (name == "commands") {
        return Json::array({"ui.apply", "ui.snapshot", "ui.query", "ui.describe", "ui.hit", "ui.focus", "ui.click", "ui.drag", "ui.type", "ui.key", "ui.wheel", "ui.stats", "script.start", "script.reload", "script.contexts", "script.diagnostics", "script.eval", "script.profile", "project.info", "project.brief", "project.reload", "project.apply", "project.save_scene", "project.write", "project.read", "render.viewport", "render.shadows", "render.msaa", "render.bloom", "render.grade", "render.ambient", "render.tonemap", "render.ao", "render.taa", "render.dof", "render.motion_blur", "render.ssr", "render.ssgi", "render.scale", "render.colorblind", "render.toon", "render.post", "render.probes", "render.oit", "assets.list", "assets.describe", "assets.reload", "assets.stats", "assets.import", "assets.preview", "input.map", "input.actions", "input.describe", "input.hold", "input.press", "input.release", "input.axis", "input.touch", "input.pad", "input.rumble", "input.state", "input.cursor", "path.info", "path.sample", "path.nearest", "window.info", "window.set", "save.write", "save.read", "save.list", "save.delete", "save.dir", "audio.play", "audio.stop", "audio.set", "audio.list", "audio.clips", "audio.analyze", "audio.stats", "audio.master", "audio.reverb", "audio.bus", "audio.buses", "transcript", "physics.stats", "physics.cloth", "physics.raycast", "physics.sweep", "physics.overlap", "physics.contacts", "physics.gravity", "physics.joints", "physics.layers", "physics.ignore", "physics.ignored", "physics2d.raycast", "physics2d.overlap", "physics2d.impulse", "physics2d.stats", "combat.hitscan", "nav.bake", "nav.path", "nav.reachable", "nav.nearest", "nav.info", "nav.mesh", "nav.agents", "nav.clear", "env.describe", "env.reset", "env.step", "env.observe", "sprite.clip", "sprite.clips", "sprite.sheet", "sprite.play", "sprite.stop", "particles.stats", "particles.preset", "particles.burst", "particles.clear", "particles.list", "animation.clips", "animation.library", "animation.play", "animation.stop", "animation.pose", "animation.layer", "animation.param", "animation.trigger", "mesh.create", "mesh.list", "mesh.remove", "tilemap.create", "tilemap.text", "tilemap.info", "tilemap.rows", "tilemap.tile", "tilemap.solid", "tilemap.sight", "tilemap.fov", "tilemap.objects", "tilemap.spawn", "tilemap.paths", "tilemap.cell", "tilemap.set", "tilemap.fill", "tilemap.save", "tilemap.add_layer", "tilemap.remove_layer", "tilemap.layer", "tilemap.add_tileset", "tilemap.remove_tileset", "tilemap.copy", "terrain.info", "terrain.height", "terrain.sculpt", "terrain.paint", "terrain.paints", "terrain.save", "terrain.reset", "terrain.heights", "scatter.copies", "water.height", "wind.at", "timeline.play", "timeline.stop", "timeline.seek", "timeline.info", "locale.get", "locale.set", "locale.table", "locale.check", "net.info", "render.stats", "render.views", "render.pick", "render.project", "render.unproject", "render.compare", "render.ids", "render.visible", "render.debug", "debug.line", "debug.box", "debug.sphere", "debug.clear", "debug.stats", "world.spawn", "world.destroy", "world.get", "world.set", "world.remove", "world.has", "world.describe", "world.find", "world.children", "world.roots", "world.components", "world.reparent", "world.rename", "world.tree", "world.query", "world.summary", "world.lint", "world.schema", "world.save", "world.load", "world.mark", "world.diff", "world.instantiate", "world.save_prefab", "world.pack", "world.unpack", "world.update_transforms", "world.clear", "events.emit", "events.since", "events.recent", "events.histogram", "events.last_seq", "events.why", "recorder.start", "recorder.stop", "recorder.clear", "recorder.status", "recorder.at", "recorder.diff", "recorder.track", "recorder.first", "state", "perf", "step", "pause", "resume", "time.scale", "quit", "help", "capture", "capture.gif", "log.tail", "report", "commands"});
    }
    const std::vector<std::string> near = command_suggestions(name);
    return fail("unknown_command", "unknown command '{}'{}", name, near.empty() ? std::string("; `commands` lists them") : "; did you mean " + Json(near).dump() + "? (`help {command}` shows how to call one)");
}

Json Session::report() {
    Json report;
    report["ok"] = errors_.empty();
    report["project"] = name_;
    report["bundle"] = options_.bundle.string();
    report["seed"] = options_.seed;
    report["tick_rate"] = options_.tick_rate;
    if (platform_) report["platform"] = platform_->describe();
    if (device_) report["gpu"] = device_->describe();
    if (renderer_) report["render"] = renderer_->describe();
    if (physics_) report["physics"] = physics_->describe();
    if (audio_) report["audio"] = audio_->describe();
    if (ui_) report["ui"] = ui_->stats();
    if (host_) report["script"] = host_->describe();
    report["frames"] = frames_;
    report["ticks"] = ticks_;
    report["sim_seconds"] = clock_.sim_seconds();
    report["state"] = last_state_;
    report["state_hash"] = hex64(hasher_.digest());
    if (net_) report["net"] = net_->info();
    report["world_hash"] = hex64(world_->hash());
    if (!tick_hashes_.empty()) {
        Json last = Json::array();
        std::size_t start = tick_hashes_.size() > 8 ? tick_hashes_.size() - 8 : 0;
        for (std::size_t i = start; i < tick_hashes_.size(); ++i) last.push_back(hex64(tick_hashes_[i]));
        report["tick_hash_tail"] = last;
    }
    if (world_) {
        report["world"] = world_->summary();
        Json ev;
        ev["total"] = world_->events().total();
        ev["histogram"] = world_->events().histogram();
        Json tail = Json::array();
        for (auto& e : world_->events().recent(10)) tail.push_back(world::event_to_json(e));
        ev["tail"] = tail;
        report["events"] = ev;
    }
    if (!capture_info_.is_null()) report["capture"] = capture_info_;
    report["clear_color"] = Json::array({clear_.r, clear_.g, clear_.b, clear_.a});
    if (journal_) {
        Json j;
        j["path"] = journal_->path.string();
        j["mode"] = journal_->replaying ? "replay" : "record";
        j["frames"] = journal_->frames.size();
        report["journal"] = j;
    }
    if (!state_history_.empty()) {
        world::TranscriptOptions to;
        to.max_lines = 24;
        report["transcript"] = world::build_transcript(state_history_, world_->events(), to).text;
    }
    Json tail = Json::array();
    for (auto& r : log::global().recent(20)) tail.push_back(log::to_json(r));
    report["log_tail"] = tail;
    if (!errors_.empty()) report["errors"] = errors_;
    report["elapsed_ms"] = total_.ms();
    report["timings"] = perf();
    return report;
}

}  // namespace pocket::app
