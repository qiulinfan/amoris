// A running project: platform, GPU, script host, world, and the command surface that scripts,
// the HTTP control server and tests all share.
#pragma once

#include <pocket/app/runtime.hpp>
#include <pocket/core/core.hpp>
#include <pocket/physics/physics.hpp>
#include <pocket/platform/platform.hpp>
#include <pocket/renderer/renderer.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/script/script_host.hpp>
#include <pocket/world/transcript.hpp>
#include <pocket/world/world.hpp>

#include <deque>
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
    [[nodiscard]] std::int64_t tick() const { return clock_.tick; }
    Status finish();                      // dispatch "stop", capture, close journal

   private:
    Json dispatch(const char* kind, Json arg);
    void run_tick();
    void bind_natives();
    world::EntityId resolve_entity(const Json& v) const;
    Result<Json> world_command(std::string_view op, const Json& p, std::string_view source);
    Result<Json> events_command(std::string_view op, const Json& p, std::string_view source);
    Result<Json> render_command(std::string_view op, const Json& p);
    Result<Json> physics_command(std::string_view op, const Json& p);
    Status render_frame();

    Options options_;
    std::unique_ptr<platform::Platform> platform_;
    std::unique_ptr<rhi::Device> device_;
    std::unique_ptr<script::ScriptHost> host_;
    std::unique_ptr<world::World> world_;
    std::unique_ptr<renderer::Renderer> renderer_;
    std::unique_ptr<physics::Physics> physics_;
    std::unique_ptr<Journal> journal_;
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
