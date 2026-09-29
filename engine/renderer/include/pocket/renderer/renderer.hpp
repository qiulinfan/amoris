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
    float bias = 0.0008f;     // depth bias in shadow-map units, scaled by slope in the shader (lookups also move out along the surface's normal by a texel)
    int cascades = 4;         // shadow maps along the view, each covering a farther slice at a coarser scale (1..4)
    float distance = 80.0f;   // how far from the camera shadows reach (clamped to the scene and the camera's far plane)
};

// Bloom: bright parts of the frame blurred and added back, so lights and emissive surfaces glow.
struct BloomSettings {
    bool enabled = false;
    float threshold = 0.8f;   // linear brightness above which a pixel glows (the scene is HDR: an emissive of 4 is four times over 1); the glow is what is over it
    float strength = 0.6f;    // how much of the blurred glow is added back
    float radius = 1.0f;      // the blur's spread, in half-resolution texels (1 tight, 4 wide)
};

// Screen-space reflections: glossy surfaces reflect what is on screen, found by marching the mirror
// ray through the depth, in place of the sky's reflection where a hit is found.
struct SsrSettings {
    bool enabled = false;
    float max_distance = 20.0f;   // how far a reflection ray is followed, in world units
    float max_roughness = 0.6f;   // rougher surfaces keep the sky's reflection
    int steps = 48;               // samples along each ray, 8..128
    float thickness = 0.3f;       // how far behind a surface a ray may pass and still hit it
    float intensity = 1.0f;
};

// Depth of field: what is nearer or farther than the focus blurred by how far it is from it, as a
// lens of the given aperture would (docs/design/rendering.md, Depth of field and motion blur).
struct DofSettings {
    bool enabled = false;
    float focus = 10.0f;       // the distance in focus, in world units
    float aperture = 0.01f;    // the blur of something at infinity, as a fraction of the view's height
    float max_blur = 0.02f;    // the most any point is blurred, the same units
};

// Motion blur: each pixel smeared along its motion over the frame, as a shutter open for `strength`
// of it would (0.5: half the frame).
struct MotionBlurSettings {
    bool enabled = false;
    float strength = 0.5f;
    int samples = 10;          // along each pixel's motion, 4..32
};

// Ambient occlusion: the sky's and the ambient light darkened where geometry crowds a point (a
// crevice, the ground under a crate), measured on screen from the depth prepass.
struct AoSettings {
    bool enabled = false;
    float radius = 0.6f;       // how far around a point is looked at, in world units
    float intensity = 1.0f;    // how dark a fully crowded point gets (0 none, 1 black)
    int samples = 12;          // samples per point (4..32)
};

// Temporal anti-aliasing: the view shifted by a different fraction of a pixel every frame, and each
// frame blended with the ones before it, found again where they were through the motion of the
// camera and of every object, what no longer fits the neighbourhood thrown away.
struct TaaSettings {
    bool enabled = false;
    float feedback = 0.9f;     // how much of the history each frame keeps (0.5..0.98)
};

// How the HDR scene becomes the 8-bit frame: an exposure (fixed, or metered from the frame and
// adapted over time), then a tone-mapping operator, then the sRGB encoding.
enum class Tonemap : int { None = 0, Aces = 1, Agx = 2, Neutral = 3 };
const char* tonemap_name(Tonemap t);
bool tonemap_from_name(const std::string& name, Tonemap& out);
struct TonemapSettings {
    Tonemap op = Tonemap::None;  // none clips at white (2D art keeps its exact colors), aces and agx roll highlights off filmically, neutral keeps hues and only compresses near white
    float exposure = 1.0f;       // multiplies the scene before the operator
    bool auto_exposure = false;  // meter the frame's average brightness and expose it to mid gray, adapting over time
    float compensation = 0.0f;   // EV added to the metered exposure (+1 twice as bright)
    float min_ev = -10.0f;       // the metered average is kept within [min_ev, max_ev] (log2 of luminance)
    float max_ev = 10.0f;
    float speed = 3.0f;          // how fast the eye adapts, per second (0 holds, large is instant)
};

// Grading: the finished frame's look, applied in the final pass after tone mapping.
struct GradeSettings {
    bool enabled = false;
    float exposure = 1.0f;       // multiplies the scene before tone mapping (1 as rendered)
    bool filmic = false;         // the ACES curve when the tone-mapping operator is none
    float temperature = 0.0f;    // -1 cool (toward blue) .. 0 .. 1 warm (toward red)
    float contrast = 1.0f;       // around mid gray (1 as rendered)
    float saturation = 1.0f;     // 0 gray .. 1 as rendered .. 2 vivid
    rhi::Color tint{1, 1, 1, 1}; // multiplies the result (a color wash; white leaves it)
    float vignette = 0.0f;       // how dark the corners get (0 none, 1 black)
    std::string lut;             // a look-up table image (a strip of N slices N by N, N*N wide), applied to the finished colors; empty for none
    float lut_strength = 1.0f;   // how much of the table's look (0 none .. 1 all)
};

struct RenderStats {
    std::uint32_t draw_calls = 0;     // instanced draws issued in the scene pass
    std::uint32_t shadow_draws = 0;   // instanced draws in the shadow pass
    bool shadows = false;             // whether a shadow map was rendered this frame
    int shadow_cascades = 0;          // cascades rendered this frame
    float shadow_distance = 0;        // view depth the last cascade reaches
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
    bool grade = false;               // whether grading was applied in the final pass this frame
    int tonemap = 0;                  // the operator of the final pass (Tonemap)
    int sky = 0;                      // the Sky drawn (0 none, 1 procedural, 2 image)
    bool depth_prepass = false;       // whether the ids and depth were drawn in a pass of their own first (MSAA, AO or fog)
    bool ao = false;                  // whether ambient occlusion was computed this frame
    bool fog = false;                 // whether fog was applied this frame
    bool volumetric = false;          // whether the fog was marched and lit (volumetric light) this frame
    bool taa = false;                 // whether the frame was resolved against its history (temporal anti-aliasing)
    bool lut = false;                 // whether a look-up table graded the frame
    bool ssr = false;                 // whether screen-space reflections were traced this frame
    std::uint32_t probes = 0;         // reflection probes in use (captured)
    std::uint32_t probe_captures = 0; // probes captured this frame (at most one a frame)
    bool dof = false;                 // whether depth of field was applied this frame
    bool motion_blur = false;         // whether motion blur was applied this frame
    std::uint32_t env_updates = 0;    // times the sky's environment light was rebuilt, over the renderer's life
    bool auto_exposure = false;       // whether the frame was metered for exposure
    std::uint32_t id_draws = 0;       // draws of the separate id pass (MSAA only)
    std::uint32_t debug_lines = 0;    // debug lines drawn over the scene
    std::uint32_t meshes = 0;
    std::uint32_t point_lights = 0;       // point lights in view this frame
    std::uint32_t spot_lights = 0;        // spot lights in view this frame
    std::uint32_t lights_culled = 0;      // point and spot lights out of view, left out
    std::uint32_t lights_dropped = 0;     // in view but over the budget (lights or cluster entries): the farthest go
    std::uint32_t cluster_entries = 0;    // light indices over all clusters
    std::uint32_t max_cluster_lights = 0; // the most lights one cluster lists
    std::uint32_t shadow_lights = 0;      // point and spot lights casting shadows this frame
    std::uint32_t shadow_faces = 0;       // faces of the shadow atlas they drew (a spot one, a point light six)
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
    float near = 0.1f, far = 1000.0f;
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
    // Exposure and tone mapping of the HDR scene into the frame; takes effect at the next frame.
    // Ambient occlusion (off by default); takes effect at the next frame.
    void set_ao(AoSettings s);
    void set_taa(TaaSettings s);
    [[nodiscard]] TaaSettings taa() const;
    // Reflection probes: the ones in use (entity, layer, center, size, when last captured), and a
    // request to capture every one again.
    [[nodiscard]] Json probes() const;
    void refresh_probes();
    void set_ssr(SsrSettings s);
    [[nodiscard]] SsrSettings ssr() const;
    void set_dof(DofSettings s);
    [[nodiscard]] DofSettings dof() const;
    void set_motion_blur(MotionBlurSettings s);
    [[nodiscard]] MotionBlurSettings motion_blur() const;
    [[nodiscard]] AoSettings ao() const;
    void set_tonemap(TonemapSettings s);
    [[nodiscard]] TonemapSettings tonemap() const;
    // Seconds between rendered frames, for the auto exposure's adaptation (a tick by default).
    void set_time_step(float seconds);
    // The auto exposure's state after the last frame, read back from the GPU: the exposure it
    // applied (EV, log2 of the multiplier) and the metered average (log2 of luminance).
    struct Metering { float exposure_ev = 0, average_ev = 0; bool valid = false; };
    Result<Metering> metering();
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
