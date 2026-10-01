// A running project: platform, GPU, script host, world, and the command surface that scripts,
// the HTTP control server and tests all share.
#pragma once

#include <pocket/app/gestures.hpp>
#include <pocket/app/input_map.hpp>
#include <pocket/app/net.hpp>
#include <pocket/app/timeline.hpp>
#include <pocket/app/runtime.hpp>
#include <pocket/assets/assets.hpp>
#include <pocket/audio/audio.hpp>
#include <pocket/core/core.hpp>
#include <pocket/nav/nav.hpp>
#include <pocket/physics/physics.hpp>
#include <pocket/physics/tiles.hpp>
#include <pocket/platform/platform.hpp>
#include <pocket/renderer/renderer.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/script/script_host.hpp>
#include <pocket/ui/document.hpp>
#include <pocket/ui/font.hpp>
#include <pocket/ui/painter.hpp>
#include <pocket/world/recorder.hpp>
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
    [[nodiscard]] bool quit_requested() const { return quit_; }  // set by the quit command or request_quit
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
    // The gestures a batch of events completes (and, with time passing, long presses): appended
    // to the batch's input events and emitted as `input.gesture` world events.
    void recognize_gestures(const std::vector<platform::Event>& events, Json& input_events, bool time_passes);
    Result<Json> ui_command(std::string_view op, const Json& p);
    Result<Json> script_command(std::string_view op, const Json& p);
    Result<Json> project_command(std::string_view op, const Json& p);
    void run_tick();
    void bind_natives();
    world::EntityId resolve_entity(const Json& v) const;
    Result<Json> world_command(std::string_view op, const Json& p, std::string_view source);
    // The pocket tool run with arguments, its exit code and output (`project.apply`).
    Result<std::pair<int, std::string>> run_tool(const std::vector<std::string>& args) const;
    // Bundle and type-check the project's scripts with the tool, reload, step `ticks`.
    Result<Json> apply_project(int ticks);
    // A model file's nodes as entity descriptions (docs/design/assets.md, Live models).
    Result<Json> model_children(const std::string& mesh_path, std::vector<std::string>& warnings);
    // A project file's content hash, as Model.hash keeps it ("" when it cannot be read).
    [[nodiscard]] std::string file_hash(const std::string& path) const;
    // Live Model instances whose file changed, made again in place; what was made again.
    Json relink_models();
    Result<Json> world_lint(const Json& p);
    Result<Json> events_command(std::string_view op, const Json& p, std::string_view source);
    Result<Json> recorder_command(std::string_view op, const Json& p);
    Result<Json> debug_command(std::string_view op, const Json& p);
    void build_debug_draw();
    Result<Json> render_command(std::string_view op, const Json& p);
    Result<Json> physics_command(std::string_view op, const Json& p);
    Result<Json> nav_command(std::string_view op, const Json& p);
    Result<Json> env_command(std::string_view op, const Json& p);
    Result<Json> env_observation(const Json& p, bool first);
    Result<Json> sprite_command(std::string_view op, const Json& p);
    Result<Json> particles_command(std::string_view op, const Json& p);
    Result<Json> animation_command(std::string_view op, const Json& p);
    Result<Json> tilemap_command(std::string_view op, const Json& p);
    Result<Json> terrain_command(std::string_view op, const Json& p);
    Result<Json> water_command(std::string_view op, const Json& p);
    Result<Json> assets_command(std::string_view op, const Json& p);
    Result<Json> audio_command(std::string_view op, const Json& p);
    Result<Json> input_command(std::string_view op, const Json& p);
    Result<Json> save_command(std::string_view op, const Json& p);
    [[nodiscard]] std::filesystem::path save_dir() const;
    // Wall-clock cost of each phase since start (the `perf` command and report.timings).
    [[nodiscard]] Json perf() const;
    void release_expired_holds();
    void apply_project_settings();   // project.toml's live settings: audio, input, sprite clips, render, physics
    // Lockstep networking (docs/design/networking.md): this peer's input waits in net_queue_ until
    // it is committed for the tick `delay` ahead; a tick runs once every player's input is in, each
    // player's through their own input map (player 0's is input_map_).
    std::unique_ptr<Net> net_;
    std::unique_ptr<Timelines> timelines_;
    Result<Json> timeline_command(std::string_view op, const Json& p);
    void update_camera_rigs(float dt);
    std::map<world::EntityId, Vec3> rig_base_;   // each camera rig's place before its shake (docs/design/cameras.md)
    // Localization (docs/design/localization.md): the language scripts' `t` uses, the project's
    // default, and a count the scripts see change when the language files may have.
    std::string locale_, locale_default_;
    std::int64_t locale_rev_ = 0;
    Result<Json> locale_command(std::string_view op, const Json& p);
    Json net_queue_ = Json::array();
    std::int64_t net_committed_ = -1;
    std::vector<InputMap> player_maps_;
    void net_pump();
    bool net_tick_ready();
    Result<Json> net_command(std::string_view op, const Json& p);
    Result<Json> mesh_command(std::string_view op, const Json& p);
    Result<Json> make_mesh(const std::string& name, const Json& spec);
    Json saved_maps(bool edited) const;
    Status restore_maps(const Json& maps);
    std::map<std::string, Json> made_meshes_;   // mesh.create's requests by name, carried by saved scenes
    // Terrains (docs/design/terrain.md): heights made from a heightmap or noise when an entity's
    // Terrain settings change, meshed into the asset store under `terrain:<entity>@<revision>`.
    struct TerrainState {
        std::string shape_key, look_key, paint_key, layer_key, error;
        std::vector<std::array<float, 4>> shares;   // each textured layer's share at every sample (docs/design/terrain.md, Layers)
        assets::Terrain grid;
        std::uint64_t revision = 0;
        std::string mesh;
        bool edited = false;
    };
    std::map<world::EntityId, TerrainState> terrains_;
    void update_terrains();
    // Water splashes (docs/design/water.md): each water.entered event after `since` bursts its water's splash emitter.
    void splash_water(std::uint64_t since);
    // Scatters (docs/design/terrain.md, Scattering): copies placed again when their settings, place or ground change.
    std::map<world::EntityId, std::string> scatter_keys_;
    void update_scatters();
    void remesh_terrain(world::EntityId id, TerrainState& st, const world::Terrain& tc);
    bool advance_rumble();   // starts the steps of rumble patterns that are due; whether any motor answered
    void tick_audio(double dt);
    // A spatial voice's volume and pan from its entity's place against the camera (docs/design/audio.md, Where a sound is).
    // Place a spatial voice from its entity against the listener (volume by distance, pan by side);
    // with `occlusion` above 0 a collider across the line takes its share and the voice's low-pass is
    // set from `base_lowpass`. Returns whether something was in the way (`blocker` gets what).
    // With dt > 0 and doppler > 0 the voice's pitch also follows the Doppler effect of this tick's motion.
    bool place_voice(std::uint32_t voice, world::EntityId entity, float base_volume, float near, float range, float occlusion = 0, float base_lowpass = 1, world::EntityId* blocker = nullptr, float base_pitch = 1, float doppler = 0, double dt = 0);
    Vec3 listener_position(Vec3* forward = nullptr, world::EntityId* listener = nullptr) const;
    Status render_frame();

    Options options_;
    std::unique_ptr<platform::Platform> platform_;
    std::unique_ptr<rhi::Device> device_;
    std::unique_ptr<script::ScriptHost> host_;
    std::unique_ptr<world::World> world_;
    std::unique_ptr<renderer::Renderer> renderer_;
    std::unique_ptr<renderer::Renderer> preview_renderer_;   // assets.preview's own, made on first use (its frames never touch the scene's)
    std::unique_ptr<renderer::Particles> particles_;
    std::unique_ptr<renderer::Animation> animation_;
    std::unique_ptr<physics::Physics> physics_;
    std::unique_ptr<physics::Physics2D> physics2d_;
    nav::Nav nav_;
    // The environment interface (docs/design/environment.md): episodes over env.reset/step/observe.
    int env_episode_ = 0;
    std::int64_t env_start_tick_ = 0;
    std::uint64_t env_last_seq_ = 0;   // events before this were already observed
    double env_last_score_ = 0;
    int env_max_ticks_ = 0;            // 0: no limit
    std::vector<std::string> physics_layers_;  // [physics] layers names, bit 0 first
    std::vector<std::vector<Vec3>> nav_paths_;  // the last paths asked for, drawn by the nav overlay
    world::Recorder recorder_;
    struct DebugFlags { bool colliders = false, joints = false, bounds = false, axes = false, nav = false, lights = false; } debug_flags_;
    struct DebugShape {
        int kind = 0;  // 0 line, 1 box, 2 sphere
        Vec3 a, b;     // line ends; box center + half; sphere center (radius in b.x)
        Quat rotation;
        rhi::Color color;
        std::int64_t until_tick = 0;  // drawn while tick < until_tick
    };
    std::vector<DebugShape> debug_shapes_;
    renderer::DebugDraw debug_draw_;
    std::unique_ptr<assets::AssetStore> assets_;
    std::unique_ptr<audio::Audio> audio_;
    struct SpatialVoice { world::EntityId entity = 0; float volume = 1, near = 1, range = 20, occlusion = 0, lowpass = 1, pitch = 1, doppler = 1; };
    std::map<std::uint32_t, SpatialVoice> spatial_voices_;   // one-shots placed by their entity every tick until they end
    // The Doppler effect: where each spatial voice's entity and the listener were last tick.
    std::map<std::uint32_t, Vec3> voice_prev_pos_;
    Vec3 listener_prev_{0, 0, 0}, listener_vel_{0, 0, 0};
    bool listener_prev_set_ = false;
    std::unique_ptr<Journal> journal_;
    std::unique_ptr<ui::Font> font_;
    struct NamedFont { std::string name, path; std::unique_ptr<ui::Font> font; };
    std::vector<NamedFont> named_fonts_;   // [ui.fonts]: kept before ui_ so they outlive the document
    std::unique_ptr<ui::Painter> painter_;
    std::unique_ptr<ui::Document> ui_;
    std::filesystem::path font_path_;
    std::vector<std::string> bundle_names_;
    std::map<std::string, Json> prefab_cache_;  // parsed prefab files by project-relative path
    InputMap input_map_;
    Gestures gestures_;
    std::map<std::string, std::int64_t> held_keys_;  // synthetic holds: key name -> tick at which it releases
    std::map<int, std::pair<float, float>> touch_last_;  // synthetic fingers: index -> last position, for the deltas
    Json script_diagnostics_ = Json::array();   // the project's type errors, as `pocket run/editor --watch` last found them
    // Rumble patterns by pad: steps of motor strengths and lengths, played on the tick clock.
    struct RumbleStep { float low = 0, high = 0; int ms = 0; };
    struct RumblePattern { std::vector<RumbleStep> steps; std::size_t next = 0; std::int64_t next_tick = 0; int repeat = 1; };
    std::map<int, RumblePattern> rumble_;
    std::vector<std::pair<std::string, int>> pending_holds_;  // holds asked for during a tick: pressed at the next tick's start
    bool in_tick_ = false;
    // Hitboxes (docs/design/combat.md): who stands in which, and when it hits again.
    struct Touch { world::EntityId box = 0, target = 0; double next = 0; };
    std::vector<Touch> touches_;
    void update_hits(std::uint64_t since_seq);
    void update_attachments();
    Result<std::vector<std::string>> load_sprite_sheet(const std::string& rel, const std::string& prefix);   // Aseprite JSON: a clip per tag   // Attach: entities held at a joint of an animated model
    bool apply_hit(world::EntityId box, world::EntityId target, std::uint64_t cause, std::vector<world::EntityId>& spent);
    bool cursor_locked_ = false, cursor_visible_ = true;   // what the game asked for (input.cursor, [input] cursor)
    void set_cursor(bool locked, bool visible);
    void seed_math_random();   // Math.random's own stream from the run seed
    [[nodiscard]] bool cursor_held() const;   // the pointer is captured now: its presses and motion are the game's
    [[nodiscard]] ui::NodeId ui_press_target(const platform::Event& e) const;   // the element a mouse press lands on (0: the game's)
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
    double time_scale_ = 1.0;    // simulation seconds per real second (slow motion below 1, a hit-stop at 0)
    double scale_left_ = 0.0;    // real seconds the scale holds before returning to 1 (0: until the next call)
    bool stepping_ = false;      // inside `step`: a frame is exactly one tick whatever the scale
    bool skip_render_ = false;   // inside a headless `step` before its last tick: simulate, do not draw
    bool frame_stale_ = false;   // ticks ran since the last drawn frame: draw before a command reads it
    // Draw now when the last frame is stale (capture, render.pick and the rest read what was drawn).
    Status ensure_drawn();
    bool quit_ = false;
    bool started_ = false;
    bool stopped_ = false;
    bool warmed_up_ = false;     // windowed: first successful present seen (or grace period over)
    Stopwatch warmup_timer_;
    struct PhaseStats {
        double total_ms = 0, max_ms = 0, last_ms = 0;
        std::uint64_t samples = 0;
        void add(double ms) { total_ms += ms; last_ms = ms; if (ms > max_ms) max_ms = ms; ++samples; }
        [[nodiscard]] Json json() const;
    };
    PhaseStats perf_frame_, perf_poll_, perf_tick_, perf_script_, perf_physics_, perf_world_, perf_state_, perf_render_;
    Stopwatch pace_timer_;
    Json capture_info_;
};

}  // namespace pocket::app
