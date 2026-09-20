// Renders the world into the device's offscreen target and an entity id buffer.
#pragma once

#include <pocket/assets/assets.hpp>
#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>
#include <pocket/renderer/animation.hpp>
#include <pocket/renderer/debug_draw.hpp>
#include <pocket/renderer/particles.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/world/world.hpp>

#include <memory>
#include <string>
#include <utility>
#include <vector>

namespace pocket::renderer {

struct ShadowSettings {
    bool enabled = true;
    float strength = 0.85f;   // how dark a fully shadowed surface gets (0 none, 1 black)
    float bias = 0.0008f;     // depth bias in shadow-map units, scaled by slope in the shader
};

// Bloom: bright parts of the frame blurred and added back, so lights and emissive surfaces glow.
struct BloomSettings {
    bool enabled = false;
    float threshold = 0.8f;   // brightness (0..1) above which a pixel glows; the glow is what is over it
    float strength = 0.6f;    // how much of the blurred glow is added back
    float radius = 1.0f;      // the blur's spread, in half-resolution texels (1 tight, 4 wide)
};

// Grading: the finished frame's look, applied last and in this order.
struct GradeSettings {
    bool enabled = false;
    float exposure = 1.0f;       // multiplies the frame (1 as rendered)
    bool filmic = false;         // roll the top of the range off, so an exposure above 1 brightens without clipping
    float temperature = 0.0f;    // -1 cool (toward blue) .. 0 .. 1 warm (toward red)
    float contrast = 1.0f;       // around mid gray (1 as rendered)
    float saturation = 1.0f;     // 0 gray .. 1 as rendered .. 2 vivid
    rhi::Color tint{1, 1, 1, 1}; // multiplies the result (a color wash; white leaves it)
    float vignette = 0.0f;       // how dark the corners get (0 none, 1 black)
};

struct RenderStats {
    std::uint32_t draw_calls = 0;     // instanced draws issued in the scene pass
    std::uint32_t shadow_draws = 0;   // instanced draws in the shadow pass
    bool shadows = false;             // whether a shadow map was rendered this frame
    std::uint32_t instances = 0;      // objects drawn (one per entity, or per glTF material)
    std::uint32_t sprites = 0;        // of which sprites
    std::uint32_t particles = 0;      // of which particles (drawn as sprites)
    std::uint32_t skinned = 0;        // of which skinned (posed) submesh instances
    std::uint32_t morphed = 0;        // of which drawn with morph target weights
    std::uint32_t translucent = 0;    // of which alpha blended (a color alpha under 1, or a glTF BLEND material), drawn after the opaque meshes far to near
    std::uint32_t moving_parts = 0;   // submeshes placed by an animated node (moving parts)
    std::uint32_t tile_layers = 0;    // tile map layers drawn (each one static mesh per tileset)
    std::uint32_t image_layers = 0;   // image layers drawn (a picture placed in map pixels, repeated across the map when asked)
    std::uint32_t tile_rebuilds = 0;  // layer meshes rebuilt after an edit, over the renderer's life
    std::uint32_t tile_frames = 0;    // animated cells rebuilt for a frame change, over the renderer's life
    int msaa = 1;                     // samples per pixel of the color pass (1 or 4)
    bool bloom = false;               // whether the bloom passes ran this frame
    bool grade = false;               // whether the grading pass ran this frame
    std::uint32_t id_draws = 0;       // draws of the separate id pass (MSAA only)
    std::uint32_t debug_lines = 0;    // debug lines drawn over the scene
    std::uint32_t meshes = 0;
    std::uint32_t point_lights = 0;
    bool has_camera = false;
    bool has_sun = false;
    world::EntityId camera = 0;
    std::uint32_t asset_meshes = 0;   // distinct glTF meshes on the GPU
    std::uint32_t textures = 0;       // distinct images on the GPU
    std::uint32_t materials = 0;      // distinct material bind groups (texture sets)
    std::vector<std::string> missing; // asset paths that failed to load this frame (drawn as magenta cubes)
};

struct IdImage {
    std::uint32_t width = 0, height = 0;
    std::vector<std::uint32_t> ids;  // entity id (low 32 bits) per pixel, 0 = background
};

// Sub-rectangle of the frame (pixels) that receives the scene; w == 0 means the whole frame.
struct Viewport {
    std::int32_t x = 0, y = 0;
    std::uint32_t w = 0, h = 0;
};

struct CameraView {
    Mat4 view;
    Mat4 proj;
    Vec3 position;
    Vec3 forward{0, 0, -1};   // world-space view direction
};

class Renderer {
   public:
    static Result<std::unique_ptr<Renderer>> create(rhi::Device& device);
    ~Renderer();
    Renderer(const Renderer&) = delete;
    Renderer& operator=(const Renderer&) = delete;

    // Draw the world into the frame (color + depth from the frame, ids into the renderer's own
    // target). Must be called between Device::begin_frame and Device::end_frame.
    Status render(rhi::Frame& frame, const world::World& world, rhi::Color clear, const Particles* particles = nullptr, const Animation* animation = nullptr, const DebugDraw* debug = nullptr);
    // Read back the id buffer of the last rendered frame.
    Result<IdImage> read_ids();
    // Entity under a pixel (0 when background). Reads back the whole id buffer.
    Result<world::EntityId> pick(std::uint32_t x, std::uint32_t y);
    [[nodiscard]] const RenderStats& stats() const;
    [[nodiscard]] Json describe() const;
    // A project image as a texture for the interface (uploaded and cached like a sprite's), with
    // its pixel size; a null view when the file is missing or undecodable (reported once).
    struct ImageView {
        WGPUTextureView view = nullptr;
        std::uint32_t width = 0, height = 0;
    };
    [[nodiscard]] ImageView image_view(const std::string& path);
    // Camera used by the last frame (default camera when the world has none).
    [[nodiscard]] const CameraView& camera() const;
    // Project a world point to pixel coordinates using the last frame's camera; false if behind.
    [[nodiscard]] bool project(Vec3 world_pos, float& out_x, float& out_y) const;
    // The world ray under a pixel of the last frame's viewport (false without a camera view).
    [[nodiscard]] bool unproject(float px, float py, Vec3& origin, Vec3& direction) const;
    void set_viewport(Viewport v);
    // Multisampling of the color pass: 1 (off) or 4; takes effect at the next frame (the scene
    // pipelines are rebuilt and the ids move to a pass of their own).
    void set_msaa(int samples);
    [[nodiscard]] int msaa() const;
    void set_shadows(ShadowSettings s);
    [[nodiscard]] ShadowSettings shadows() const;
    // Bloom over the finished scene (off by default); takes effect at the next frame.
    void set_bloom(BloomSettings s);
    [[nodiscard]] BloomSettings bloom() const;
    // Grading over the finished frame, after bloom (off by default); takes effect at the next frame.
    void set_grade(GradeSettings s);
    [[nodiscard]] GradeSettings grade() const;
    // Where glTF meshes and images come from (MeshRenderer.mesh / .texture paths). Optional.
    void set_assets(assets::AssetStore* store);
    // Local bounds of asset meshes first uploaded since the last call (path -> min/max).
    std::vector<std::pair<std::string, std::pair<Vec3, Vec3>>> take_new_bounds();
    // Release GPU copies of assets so they reload from the store.
    void drop_asset_cache();
    [[nodiscard]] Viewport viewport() const;
    // The viewport actually used by the last frame (clamped to the frame).
    [[nodiscard]] Viewport applied_viewport() const;

   private:
    Renderer();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::renderer
