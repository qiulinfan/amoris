// A running project: platform, GPU, script host, world, and the command surface that scripts,
// the HTTP control server and tests all share.
#pragma once

#include <pocket/app/input_map.hpp>
#include <pocket/app/runtime.hpp>
#include <pocket/assets/assets.hpp>
#include <pocket/audio/audio.hpp>
#include <pocket/core/core.hpp>
#include <pocket/physics/physics.hpp>
#include <pocket/platform/platform.hpp>
#include <pocket/renderer/renderer.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/script/script_host.hpp>
#include <pocket/ui/document.hpp>
#include <pocket/ui/font.hpp>
#include <pocket/ui/painter.hpp>
#include <pocket/world/transcript.hpp>
#include <pocket/world/world.hpp>

#include <deque>
#include <map>
#include <memory>
#include <string>
#include <vector>

namespace pocket::app {

struct Journal;

class Session {
   public:
    explicit Session(const Options& options);
    ~Session();

    Status start();                       // create subsystems, load the scene, evaluate the bundle, dispatch "start"
    Status frame();                       // one frame: input, ticks, render
    Status idle_frame();                  // paused frame: input, UI scripts and rendering, no simulation ticks
    Status run_ticks(int ticks);          // simulation only (used by `step` while paused)
    Result<Json> command(std::string_view name, const Json& params, std::string_view source = "agent");
    [[nodiscard]] Json report();          // snapshot of the run report
    [[nodiscard]] bool finished() const;  // frame budget reached or quit requested
    [[nodiscard]] bool paused() const { return paused_; }
    void set_paused(bool p) { paused_ = p; }
    void request_quit() { quit_ = true; }
    [[nodiscard]] bool ok() const { return errors_.empty(); }
    void record_error(const Error& e);
    [[nodiscard]] const Options& options() const { return options_; }
    [[nodiscard]] world::World& world() { return *world_; }
    [[nodiscard]] rhi::Device& device() { return *device_; }
    [[nodiscard]] renderer::Renderer& renderer() { return *renderer_; }
    [[nodiscard]] physics::Physics& physics() { return *physics_; }
    [[nodiscard]] assets::AssetStore& assets() { return *assets_; }
    [[nodiscard]] audio::Audio& audio() { return *audio_; }
    [[nodiscard]] ui::Document* ui() { return ui_.get(); }
    [[nodiscard]] std::int64_t tick() const { return clock_.tick; }
    Status finish();                      // dispatch "stop", capture, close journal

   private:
    Json dispatch(const char* kind, Json arg, std::string_view context = "");
    Status load_bundle(const std::filesystem::path& path, const std::string& name);
    Status load_scene_file();             // the project's scene from disk (no-op without one)
    bool context_active(const std::string& name);
    Result<bool> poll_input(Json& input_events, int& ticks, bool simulating);  // false: replay exhausted
    Json frame_info() const;
    void ui_size(float& width, float& height, float& scale) const;
    Json inject_events(std::vector<platform::Event> events);
    Result<Json> ui_command(std::string_view op, const Json& p);
    Result<Json> script_command(std::string_view op, const Json& p);
    Result<Json> project_command(std::string_view op, const Json& p);
    void run_tick();
    void bind_natives();
    world::EntityId resolve_entity(const Json& v) const;
    Result<Json> world_command(std::string_view op, const Json& p, std::string_view source);
    Result<Json> events_command(std::string_view op, const Json& p, std::string_view source);
    Result<Json> render_command(std::string_view op, const Json& p);
    Result<Json> physics_command(std::string_view op, const Json& p);
    Result<Json> assets_command(std::string_view op, const Json& p);
    Result<Json> audio_command(std::string_view op, const Json& p);
    Result<Json> input_command(std::string_view op, const Json& p);
    void release_expired_holds();
    void tick_audio(double dt);
    Status render_frame();

    Options options_;
    std::unique_ptr<platform::Platform> platform_;
    std::unique_ptr<rhi::Device> device_;
    std::unique_ptr<script::ScriptHost> host_;
    std::unique_ptr<world::World> world_;
    std::unique_ptr<renderer::Renderer> renderer_;
    std::unique_ptr<physics::Physics> physics_;
    std::unique_ptr<assets::AssetStore> assets_;
    std::unique_ptr<audio::Audio> audio_;
    std::unique_ptr<Journal> journal_;
    std::unique_ptr<ui::Font> font_;
    std::unique_ptr<ui::Painter> painter_;
    std::unique_ptr<ui::Document> ui_;
    std::filesystem::path font_path_;
    std::vector<std::string> bundle_names_;
    std::map<std::string, Json> prefab_cache_;  // parsed prefab files by project-relative path
    InputMap input_map_;
    std::map<std::string, std::int64_t> held_keys_;  // synthetic holds: key name -> tick at which it releases
    // Typed-array packing buffers shared with scripts (never freed while a script may hold them).
    std::vector<float> pack_data_;
    std::vector<double> pack_ids_;
    std::vector<std::vector<float>> retired_data_;
    std::vector<std::vector<double>> retired_ids_;
    std::size_t pack_data_capacity_ = 0, pack_ids_capacity_ = 0;
    world::World::PackInfo last_pack_;
    std::string last_pack_component_;
    std::vector<std::string> last_pack_fields_;
    TickClock clock_;
    StateHasher hasher_;
    Stopwatch total_;
    Stopwatch frame_timer_;
    rhi::Color clear_{0.1f, 0.1f, 0.12f, 1.0f};
    Random rng_;
    Json project_ = Json::object();
    std::string name_;
    Json last_state_ = Json::object();
    std::vector<std::uint64_t> tick_hashes_;
    std::vector<world::StateSample> state_history_;
    std::vector<Json> errors_;
    std::uint64_t frames_ = 0;
    std::int64_t ticks_ = 0;
    bool has_dispatch_ = false;
    bool paused_ = false;
    bool quit_ = false;
    bool started_ = false;
    bool stopped_ = false;
    bool warmed_up_ = false;     // windowed: first successful present seen (or grace period over)
    Stopwatch warmup_timer_;
    Stopwatch pace_timer_;
    Json capture_info_;
};

}  // namespace pocket::app
