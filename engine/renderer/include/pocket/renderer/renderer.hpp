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
#include <optional>
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
    // The sun's shadows soften with distance from what casts them, as under a sun this many degrees
    // across in radius (0: the sharp 3x3 filter; docs/design/rendering.md, Soft shadows).
    float softness = 0.0f;
    // Contact shadows: a short march toward the sun through the depth buffer, for the small gaps
    // and thin casters the shadow maps are too coarse to see (a foot on the ground, a cup on a table).
    bool contact = false;
    float contact_length = 0.3f;   // how far the march goes, in world units
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

// Screen-space global illumination: light that bounces off what is on screen onto what is near it
// (a red wall reddening the floor beside it), gathered by rays marched through the depth.
struct SsgiSettings {
    bool enabled = false;
    float distance = 3.0f;     // how far a ray is followed, in world units
    int rays = 2;              // rays a pixel a frame (at half resolution), 1..8
    int steps = 12;            // samples along each ray, 4..64
    float thickness = 0.5f;    // how far behind a surface a ray may pass and still hit it
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
// The flat light every surface gets from all around when there is no Sky (render.ambient): the
// darkness of a 2D dungeon between its torches, or a brighter fill. The color as a picker shows it.
struct AmbientSettings {
    rhi::Color color{0.18f, 0.19f, 0.22f, 1.0f};
    float intensity = 1.0f;
};

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

// Render scale (docs/design/rendering.md, Render scale): the window's view drawn at a fraction of its
// pixels and stretched up to fill it, sharpened; `dynamic` moves the fraction between `least` and
// `scale` to keep the GPU's frame under `target_ms`.
struct RenderScaleSettings {
    float scale = 1.0f;       // 0.25..1 of the window's width and height (the most, when dynamic)
    bool dynamic = false;
    float target_ms = 12.0f;  // dynamic: the GPU's frame it keeps under
    float least = 0.5f;       // dynamic: the smallest fraction it goes to
    float sharpen = 0.25f;    // 0..1: how much the stretched picture is sharpened
    bool pixelated = false;   // stretched with hard pixel edges (no filtering, no sharpening): a retro look
};

// Colour vision (docs/design/rendering.md, Colour vision): the finished frame seen as a person with
// a dichromacy sees it (simulate), or corrected so the colours they cannot tell apart differ in
// ones they can.
struct ColourVisionSettings {
    int mode = 0;            // 0 off, 1 protanopia (red), 2 deuteranopia (green), 3 tritanopia (blue)
    bool simulate = false;   // show what they see (for a designer) instead of correcting (for a player)
    float strength = 1.0f;   // 0..1
};

// A cel look (docs/design/rendering.md, Toon): the light in flat bands and outlines round every
// entity where the id under the pixels changes.
struct ToonSettings {
    bool enabled = false;
    int bands = 3;                       // flat steps of a light's strength
    float softness = 0.04f;              // how far each step's edge is blurred (0..0.5 of a band)
    float outline = 2.0f;                // pixels across; 0 none
    float outline_color[3] = {0.08f, 0.07f, 0.07f};   // sRGB
    float opacity = 1.0f;
};

struct RenderStats {
    std::uint32_t draw_calls = 0;     // instanced draws issued in the scene pass
    std::uint32_t shadow_draws = 0;   // instanced draws in the shadow pass
    std::uint32_t shadow_redrawn = 0; // the sun's cascades drawn this frame (a far one is kept a frame or three while it still holds its slice)
    std::uint32_t shadow_instances = 0; // objects drawn into the shadow passes (a light's faces draw only what its reach touches)
    bool shadows = false;             // whether a shadow map was rendered this frame
    int shadow_cascades = 0;          // cascades rendered this frame
    float shadow_distance = 0;        // view depth the last cascade reaches
    std::uint32_t instances = 0;      // objects drawn (one per entity, or per glTF material)
    std::uint32_t scattered = 0;      // of which copies a Scatter placed (World::derived_instances)
    std::uint32_t sprites = 0;        // of which sprites
    std::uint32_t particles = 0;      // of which particles (drawn as sprites)
    std::uint32_t skinned = 0;        // of which skinned (posed) submesh instances
    std::uint32_t morphed = 0;        // of which drawn with morph target weights
    std::uint32_t translucent = 0;    // of which alpha blended (a color alpha under 1, or a glTF BLEND material), drawn after the opaque meshes far to near
    std::uint32_t glass = 0;          // of which transmitting, drawn in a pass after the solid meshes over a copy of them
    std::uint32_t lod_simplified = 0; // draws (a scattered copy each) at a simpler level of detail
    std::uint32_t lod_culled = 0;     // draws left out for covering less of the view than their cull_screen
    std::uint64_t triangles = 0;      // triangles of the meshes drawn (each copy and level counted), before the view's culling
    std::uint32_t out_of_view = 0;    // draws whose bounds are outside the camera's view: not drawn by its passes (shadows may still)
    // GPU time (render.stats.gpu): each pass's milliseconds from its timestamps, read back a frame or
    // two after it was drawn (gpu_age frames ago); empty where the device has no timestamps.
    std::vector<std::pair<std::string, double>> gpu_passes;
    double gpu_ms = 0;
    float render_scale = 1.0f;        // the fraction of the window's pixels the view was drawn at
    int colour_vision = 0;            // the colour vision mode applied in the final pass (0 off)
    bool toon = false;                // the cel look drew this frame
    std::uint32_t highlights = 0;     // entities outlined by MeshRenderer.highlight
    std::uint32_t render_width = 0, render_height = 0;   // its size in pixels
    std::uint64_t gpu_age = 0;
    std::uint64_t gpu_frames = 0;   // frames whose timings have come back so far
    bool gpu_timing = false;
    std::uint32_t moving_parts = 0;   // submeshes placed by an animated node (moving parts)
    std::uint32_t tile_layers = 0;    // tile map layers drawn (each one static mesh per tileset)
    std::uint32_t image_layers = 0;   // image layers drawn (a picture placed in map pixels, repeated across the map when asked)
    std::uint32_t tile_rebuilds = 0;  // layer meshes rebuilt after an edit, over the renderer's life
    std::uint32_t tile_frames = 0;    // animated cells rebuilt for a frame change, over the renderer's life
    int msaa = 1;                     // samples per pixel of the color pass (1 or 4)
    bool bloom = false;               // whether the bloom passes ran this frame
    bool grade = false;               // whether grading was applied in the final pass this frame
    int tonemap = 0;                  // the operator of the final pass (Tonemap)
    int post_effects = 0;             // a project's post effects run this frame
    int sky = 0;                      // the Sky drawn (0 none, 1 procedural, 2 image)
    bool depth_prepass = false;       // whether the ids and depth were drawn in a pass of their own first (MSAA, AO or fog)
    bool ao = false;                  // whether ambient occlusion was computed this frame
    bool soft_shadows = false;        // whether the sun's shadows softened with distance (ShadowSettings::softness)
    bool contact_shadows = false;     // whether contact shadows were marched this frame
    bool fog = false;                 // whether fog was applied this frame
    bool volumetric = false;          // whether the fog was marched and lit (volumetric light) this frame
    std::uint32_t grass_blades = 0;   // grass blades drawn round the camera (docs/design/terrain.md, Grass; the shader culls those that do not grow)
    bool clouds = false;              // whether volumetric clouds were marched this frame (Sky.cloud_depth)
    bool taa = false;                 // whether the frame was resolved against its history (temporal anti-aliasing)
    bool oit = false;                 // whether translucent meshes were blended order-independently this frame
    bool lut = false;                 // whether a look-up table graded the frame
    bool ssr = false;                 // whether screen-space reflections were traced this frame
    bool ssgi = false;                // whether screen-space global illumination was gathered this frame
    std::uint32_t probes = 0;         // reflection probes in use (captured)
    std::uint32_t probe_captures = 0; // probes captured this frame (at most one a frame)
    std::uint32_t grids = 0;          // irradiance volumes ready (every probe captured)
    std::uint32_t grid_captures = 0;  // irradiance volume probes captured this frame
    std::uint32_t water = 0;          // bodies of water drawn
    bool underwater = false;          // the camera is under a water surface
    std::uint32_t decals = 0;         // decals painted this frame (the nearest in view, at most 64)
    std::uint32_t decal_images = 0;   // decal images loaded (layers of the array besides the built-in spot)
    bool dof = false;                 // whether depth of field was applied this frame
    bool motion_blur = false;         // whether motion blur was applied this frame
    std::uint32_t env_updates = 0;    // times the sky's environment light was rebuilt, over the renderer's life
    bool auto_exposure = false;       // whether the frame was metered for exposure
    std::uint32_t id_draws = 0;       // draws of the separate id pass (MSAA only)
    std::uint32_t debug_lines = 0;    // debug lines drawn over the scene
    std::uint32_t weather_drops = 0;  // rain drops and snow flakes drawn about the camera (Weather)
    std::uint32_t shelter_draws = 0;  // draws into the shelter map, the last time it was drawn (Weather)
    std::uint32_t footprints = 0;     // prints pressed into lying snow (Weather)
    std::uint32_t water_rings = 0;    // rings spreading on water (Water)
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
    float sun_light[3] = {0, 0, 0};   // the sun light as it reaches the ground (linear; an atmosphere colours it)
    world::EntityId camera = 0;
    world::EntityId blend_from = 0;   // the camera the window's view is blending from (0: none, or one gone)
    float blend = 1.0f;               // how far the window's view has come to `camera` (eased; 1: there)
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

// A view the renderer looks from instead of the scene's camera, for a frame or a few: what
// render.views draws its sheet from without moving anything in the world.
struct ViewOverride {
    Vec3 eye, target, up{0, 1, 0};
    float fov_degrees = 45;
};

class Renderer {
   public:
    static Result<std::unique_ptr<Renderer>> create(rhi::Device& device);
    ~Renderer();
    Renderer(const Renderer&) = delete;
    Renderer& operator=(const Renderer&) = delete;

    // Draw the world into the frame (color + depth from the frame, ids into the renderer's own
    // target). Must be called between Device::begin_frame and Device::end_frame.
    // One view of several (docs/design/cameras.md, Several cameras): drawn through `camera` into
    // `viewport`, keeping what earlier views drew. A secondary view leaves the frame-to-frame state
    // (TAA's history, motion, the meter, the volume's history, probe captures) to the first one, and
    // the project's post effects run in the first view only.
    struct RenderView {
        world::EntityId camera = 0;
        Viewport viewport{};
        bool secondary = false;
        std::string target;   // drawing into this view texture (Camera.target): cleared, and not sampled by what it draws
    };
    // The texture a camera with Camera.target draws into, made (again, at a new size) when asked:
    // "view:<name>" names it to sprites, meshes and the interface.
    struct ViewTexture {
        WGPUTexture texture = nullptr;
        WGPUTextureView view = nullptr;
    };
    Result<ViewTexture> view_texture(const std::string& name, std::uint32_t width, std::uint32_t height);
    Status render(rhi::Frame& frame, const world::World& world, rhi::Color clear, const Particles* particles = nullptr, const Animation* animation = nullptr, const DebugDraw* debug = nullptr, const RenderView* view = nullptr);
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
    // Look from `view` instead of the scene's camera until it is cleared (nullopt).
    void set_view(std::optional<ViewOverride> view);
    // The next frame follows a gap (frames simulated and not drawn): what builds up over frames
    // (TAA's history, motion blur's last view) starts over.
    void cut();
    [[nodiscard]] int msaa() const;
    void set_shadows(ShadowSettings s);
    [[nodiscard]] ShadowSettings shadows() const;
    // Bloom over the finished scene (off by default); takes effect at the next frame.
    void set_bloom(BloomSettings s);
    [[nodiscard]] BloomSettings bloom() const;
    // Grading over the finished frame, after bloom (off by default); takes effect at the next frame.
    void set_grade(GradeSettings s);
    [[nodiscard]] GradeSettings grade() const;
    void set_ambient(AmbientSettings s);
    [[nodiscard]] AmbientSettings ambient() const;
    // Exposure and tone mapping of the HDR scene into the frame; takes effect at the next frame.
    // Ambient occlusion (off by default); takes effect at the next frame.
    void set_ao(AoSettings s);
    void set_taa(TaaSettings s);
    [[nodiscard]] TaaSettings taa() const;
    // Reflection probes: the ones in use (entity, layer, center, size, when last captured), and a
    // request to capture every one again.
    [[nodiscard]] Json probes() const;
    void refresh_probes();
    // Order-independent transparency (weighted blended): translucent meshes need no sorting.
    void set_oit(bool enabled);
    [[nodiscard]] bool oit() const;
    void set_ssr(SsrSettings s);
    [[nodiscard]] SsrSettings ssr() const;
    void set_ssgi(SsgiSettings s);
    [[nodiscard]] SsgiSettings ssgi() const;
    void set_colour_vision(ColourVisionSettings s);
    [[nodiscard]] ColourVisionSettings colour_vision() const;
    void set_toon(ToonSettings s);
    [[nodiscard]] ToonSettings toon() const;
    // Prints pressed into lying snow (docs/design/rendering.md, Weather): where each foot came down,
    // its heading (radians about +y) and how deep it still is (1 fresh .. 0 filled in). At most 64.
    struct Footprint { float x = 0, z = 0, angle = 0, depth = 1; };
    void set_footprints(std::vector<Footprint> prints);
    // Rings spreading on water (docs/design/water.md, Rings): where something fell in or moves
    // through it, seconds since, how strong. At most 32.
    struct WaterRing { float x = 0, z = 0, age = 0, strength = 1, foam = 0; };   // foam: a boat's wake churns it white
    void set_water_rings(std::vector<WaterRing> rings);
    // The ground grass grows on (docs/design/terrain.md, Grass): a terrain's samples as the session
    // has them, n by n over size_x by size_z centred on its entity: each the height, the share of
    // the layer the grass grows on and the paint's cover. `revision` changes when they do.
    struct GroundField {
        std::uint64_t revision = 0;
        int n = 0;
        float size_x = 0, size_z = 0;
        std::vector<std::array<float, 3>> samples;
    };
    void set_ground_field(world::EntityId terrain, GroundField field);
    void set_render_scale(RenderScaleSettings s);
    [[nodiscard]] RenderScaleSettings render_scale() const;
    void set_dof(DofSettings s);
    [[nodiscard]] DofSettings dof() const;
    void set_motion_blur(MotionBlurSettings s);
    [[nodiscard]] MotionBlurSettings motion_blur() const;
    [[nodiscard]] AoSettings ao() const;
    void set_tonemap(TonemapSettings s);
    // Post effects a project wrote (docs/design/rendering.md, Post effects): WGSL that defines
    // `fn effect(uv: vec2f) -> vec4f`, run in order after the tonemap over the frame as shown (the
    // interface is drawn after them). Each compiles on its own: the answer has, per effect, the
    // compiler's message, empty when it compiled; only those that compiled run.
    struct PostEffect {
        std::string name;
        std::string wgsl;
        std::array<float, 8> params{};
        bool enabled = true;
    };
    std::vector<std::string> set_post_effects(const std::vector<PostEffect>& effects);
    // A sprite material a project wrote (docs/design/sprites.md, Materials), by the name Sprite.material
    // gives (its file): WGSL defining fn material(texel, tint, uv, params, time) -> vec4f. Answers the
    // compiler's message, empty when it compiled; a sprite whose material did not compile is drawn plain.
    std::string set_sprite_material(const std::string& name, const std::string& wgsl);
    [[nodiscard]] bool has_sprite_material(const std::string& name) const;
    // The same for meshes (MeshRenderer.material; docs/design/rendering.md, Materials a project writes): WGSL defining fn material(lit: vec4f, s: Surface) ->
    // vec4f over the engine's lit colour; opaque, unskinned meshes in the lit pass.
    std::string set_mesh_material(const std::string& name, const std::string& wgsl);
    [[nodiscard]] bool has_mesh_material(const std::string& name) const;
    void forget_sprite_materials();
    [[nodiscard]] std::vector<PostEffect> post_effects() const;
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
    // One view drawn whole into `frame` (render() draws the window's at a scale through it).
    Status render_scene(rhi::Frame& frame, const world::World& world, rhi::Color clear, const Particles* particles, const Animation* animation, const DebugDraw* debug, const RenderView* view);
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::renderer
