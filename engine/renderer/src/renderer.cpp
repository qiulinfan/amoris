#include <pocket/renderer/renderer.hpp>
#include <pocket/world/water.hpp>
#include <pocket/world/wind.hpp>

#include <pocket/core/log.hpp>
#include <pocket/renderer/primitives.hpp>

#ifndef __EMSCRIPTEN__
#include <webgpu/wgpu.h>
#endif

#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>
#include <format>
#include <functional>
#include <map>
#include <numbers>
#include <set>
#include <unordered_map>

namespace pocket::renderer {

namespace {

// Point and spot lights live in a storage buffer and reach a pixel through a froxel grid: the view
// cut into tiles across and down and into slices of depth spaced by its logarithm, each cluster
// listing the lights whose reach touches it (docs/design/rendering.md, Many lights).
constexpr std::uint32_t kMaxLights = 1024;
constexpr std::uint32_t kClusterX = 16, kClusterY = 9, kClusterZ = 24;
constexpr std::uint32_t kClusters = kClusterX * kClusterY * kClusterZ;
constexpr std::uint32_t kMaxClusterEntries = 1u << 18;   // light indices over all clusters
struct GpuLight {
    float pos_range[4];    // world position, range
    float color_kind[4];   // linear color times intensity, kind (1 point, 2 spot)
    float dir_cos[4];      // spot: the direction it shines, cos of the outer half-angle
    float cone[4];         // spot: cos of the inner half-angle; its first shadow face (-1: none); a texel's size at unit distance
};
// A decal as the shader reads it (docs/design/rendering.md, Decals).
struct GpuDecal {
    float inv[16];      // world to the box, -0.5 to 0.5 along each axis
    float axis[4];      // the projection's direction, the cosine past which surfaces fade
    float color[4];     // linear tint, opacity
    float params[4];    // image layer, roughness (negative: the surface's), glow, normal map layer (0 none)
    float sphere[4];    // centre, radius squared
    float extra[4];     // bumpiness
};
// Shadows of point and spot lights: one depth atlas of 512-texel faces, a spot's one face a
// perspective view down its cone, a point light's six the faces of a cube around it.
constexpr std::uint32_t kFaceSize = 512;
constexpr std::uint32_t kFaceTiles = 8;   // across and down
constexpr std::uint32_t kAtlasSize = kFaceSize * kFaceTiles;
constexpr std::uint32_t kMaxFaces = kFaceTiles * kFaceTiles;   // the shaders' shadow_faces array is this long
struct GpuFace {
    float view_proj[16];
    float rect[4];         // the face's corner in the atlas (u, v), its size in uv, a texel in uv
};
// The scene is drawn into a half-float target (lighting in linear light, values over 1 kept), and
// a final pass exposes, tone-maps and sRGB-encodes it into the 8-bit frame.
constexpr WGPUTextureFormat kHdrFormat = WGPUTextureFormat_RGBA16Float;

// Colors in components are authored as a color picker shows them (sRGB); lighting needs linear
// light. Values over 1 are intensities and pass through (an emissive of 4 is four times white).
float decode(float c) { return c <= 0.04045f ? c / 12.92f : (c <= 1.0f ? std::pow((c + 0.055f) / 1.055f, 2.4f) : c); }

// The sun's light after the air on its way down (docs/design/rendering.md, Atmosphere; the same air
// as kSkyWgsl's): its transmittance from the top of the atmosphere to the ground toward `toward_sun`,
// over the transmittance straight up, so a noon sun is as its light says, a low one redder and
// dimmer, and one below the horizon (the planet in the way) gone, fading over half a degree.
Vec3 atmosphere_tint(Vec3 toward_sun, float haze) {
    constexpr double kGround = 6360e3, kTop = 6420e3, kHr = 8000, kHm = 1200;
    constexpr double kBr[3] = {5.8e-6, 13.5e-6, 33.1e-6};
    const double bm = 21e-6 * 1.1 * std::max(static_cast<double>(haze), 0.0);
    auto transmittance = [&](double dx, double dy, double dz, double out[3]) {
        const double oy = kGround + 2.0;
        const double b = oy * dy, c = oy * oy - kTop * kTop;
        const double len = -b + std::sqrt(std::max(b * b - c, 0.0));
        double od_r = 0, od_m = 0;
        constexpr int kSteps = 64;
        const double ds = len / kSteps;
        for (int i = 0; i < kSteps; ++i) {
            const double t = ds * (i + 0.5);
            const double px = dx * t, py = oy + dy * t, pz = dz * t;
            const double h = std::max(std::sqrt(px * px + py * py + pz * pz) - kGround, 0.0);
            od_r += std::exp(-h / kHr) * ds;
            od_m += std::exp(-h / kHm) * ds;
        }
        for (int k = 0; k < 3; ++k) out[k] = std::exp(-(kBr[k] * od_r + bm * od_m));
    };
    const Vec3 s = normalize(toward_sun);
    const double lift = std::max(static_cast<double>(s.y), 0.0005);   // the air along the horizon at worst
    const double len = std::sqrt(static_cast<double>(s.x) * s.x + static_cast<double>(s.z) * s.z);
    const double horiz = std::sqrt(std::max(1.0 - lift * lift, 0.0));
    double low[3], up[3];
    transmittance(len > 1e-9 ? s.x / len * horiz : horiz, lift, len > 1e-9 ? s.z / len * horiz : 0.0, low);
    transmittance(0, 1, 0, up);
    const float fade = std::clamp((s.y + 0.0087f) / 0.0087f, 0.0f, 1.0f);   // the disc sinks over half a degree
    return {static_cast<float>(low[0] / up[0]) * fade, static_cast<float>(low[1] / up[1]) * fade, static_cast<float>(low[2] / up[2]) * fade};
}

// The atmosphere's colour along direction d (at or above the horizon), as kSkyWgsl's `air` makes it:
// the fog takes it (the mean around the horizon), so distance fades into the sky's own colour.
Vec3 atmosphere_air(Vec3 d, Vec3 s, float haze, Vec3 sun) {
    constexpr double kGround = 6360e3, kTop = 6420e3, kHr = 8000, kHm = 1200, kPi = 3.14159265358979;
    constexpr double kBr[3] = {5.8e-6, 13.5e-6, 33.1e-6};
    constexpr double kBm = 21e-6, kSunLight = 16.0;
    const double hz = std::max(static_cast<double>(haze), 0.0);
    auto to_top = [&](double ox, double oy, double oz, double dx, double dy, double dz) {
        const double b = ox * dx + oy * dy + oz * dz, c = ox * ox + oy * oy + oz * oz - kTop * kTop;
        return -b + std::sqrt(std::max(b * b - c, 0.0));
    };
    const double oy = kGround + 2.0;
    const double len = to_top(0, oy, 0, d.x, d.y, d.z);
    const double ds = len / 16;
    double od_r = 0, od_m = 0, sr[3] = {0, 0, 0}, sm[3] = {0, 0, 0};
    for (int i = 0; i < 16; ++i) {
        const double t = ds * (i + 0.5);
        const double px = d.x * t, py = oy + d.y * t, pz = d.z * t;
        const double h = std::max(std::sqrt(px * px + py * py + pz * pz) - kGround, 0.0);
        const double hr = std::exp(-h / kHr) * ds, hm = std::exp(-h / kHm) * ds;
        od_r += hr;
        od_m += hm;
        const double ls = to_top(px, py, pz, s.x, s.y, s.z), lds = ls / 8;
        double lr = 0, lm = 0;
        bool lit = true;
        for (int j = 0; j < 8 && lit; ++j) {
            const double tt = lds * (j + 0.5);
            const double qx = px + s.x * tt, qy = py + s.y * tt, qz = pz + s.z * tt;
            const double hq = std::sqrt(qx * qx + qy * qy + qz * qz) - kGround;
            if (hq < 0) lit = false;
            lr += std::exp(-hq / kHr) * lds;
            lm += std::exp(-hq / kHm) * lds;
        }
        if (!lit) continue;
        for (int k = 0; k < 3; ++k) {
            const double att = std::exp(-(kBr[k] * (0.5 * od_r + lr) + kBm * 1.1 * hz * (0.5 * od_m + lm)));
            sr[k] += att * hr;
            sm[k] += att * hm;
        }
    }
    const double mu = d.x * s.x + d.y * s.y + d.z * s.z, g = 0.76;
    const double phase_r = 3.0 / (16.0 * kPi) * (1.0 + mu * mu);
    const double phase_m = 3.0 / (8.0 * kPi) * ((1.0 - g * g) * (1.0 + mu * mu)) / ((2.0 + g * g) * std::pow(1.0 + g * g - 2.0 * g * mu, 1.5));
    const double sc[3] = {sun.x, sun.y, sun.z};
    Vec3 out;
    float* o[3] = {&out.x, &out.y, &out.z};
    for (int k = 0; k < 3; ++k) *o[k] = static_cast<float>(kSunLight * sc[k] * (sr[k] * kBr[k] * phase_r + sm[k] * kBm * hz * phase_m));
    return out;
}

// A float as IEEE half bits (for uploading light levels into half-float textures), clamped to the largest half.
std::uint16_t to_half(float f) {
    if (!(f == f)) return 0;
    f = std::clamp(f, -65504.0f, 65504.0f);
    std::uint32_t x;
    std::memcpy(&x, &f, 4);
    const std::uint32_t sign = (x >> 16) & 0x8000u;
    const int exp = static_cast<int>((x >> 23) & 0xFFu) - 127 + 15;
    std::uint32_t mant = x & 0x7FFFFFu;
    if (exp <= 0) {
        if (exp < -10) return static_cast<std::uint16_t>(sign);
        mant |= 0x800000u;
        return static_cast<std::uint16_t>(sign | (mant >> static_cast<std::uint32_t>(14 - exp)));
    }
    return static_cast<std::uint16_t>(sign | (static_cast<std::uint32_t>(exp) << 10) | (mant >> 13));
}
// Per-object data lives in one storage buffer indexed by instance_index, so a run of entities
// with the same mesh and material is one instanced draw.
constexpr std::uint32_t kObjectStride = 464;  // sizeof(ObjectUniforms)
constexpr std::uint32_t kMaxObjects = 65536;

struct alignas(16) FrameUniforms {
    float view_proj[16];
    float camera_pos[4];
    float sun_dir[4];
    float sun_color[4];
    float ambient[4];
    std::uint32_t clusters[4];   // tiles across, tiles down, depth slices, point and spot lights this frame
    float cluster_z[4];          // near, far, slices / ln(far / near), ln(near)
    float viewport[4];           // the scene's rectangle in pixels: x, y, w, h
    float light_view_proj[16];   // the sun's orthographic view for the shadow map
    float shadow[4];             // texel size, depth bias, strength, enabled
    float inv_view_proj[16];     // clip to world, for the sky's view directions
    float env[4];                // environment light: on, diffuse, specular, the prefiltered map's last level
    float sky[4];                // sky drawn: mode, cos of the sun disc's radius, the disc's brightness, a sun exists
    float cascade_vp[4][16];     // the sun's view-projection per cascade
    float cascade_far[4];        // the view depth each cascade reaches
    float cascade_texel[4];      // a texel of each cascade in world units (the lookup's normal offset)
    float camera_fwd[4];         // xyz: the camera's forward (view depth), w: cascades in use
    float ao[4];                 // ambient occlusion: on, 1 / frame width, 1 / frame height
    float cur_view_proj[16];     // this frame's view-projection without the TAA jitter
    float prev_view_proj[16];    // last frame's, without its jitter
    float taa[4];                // on
    float probe_box[8][4];       // reflection probes in use: center, the array layer
    float probe_ext[8][4];       // half size, intensity (negative: no box projection)
    float probe_info[4];         // how many, the last prefiltered level, a capture, how many reflect
    float decals[4];             // how many
    float clock[4];              // simulated seconds now and a frame ago (swaying copies)
    float wind[4];               // the Wind: where it blows to (x, z), its speed, 1 when there is one
    float wind_gust[4];          // its gusts (fraction), 2 pi / gust length
    float clouds[4];             // the atmosphere's clouds: cover, height, 1 / feature size, on
    float cloud_drift[4];        // how far they have drifted (x, z)
    float waters[8][4];          // up to four water bodies for caustics: (centre x, level, centre z, half x), (half z, depth, caustics)
    float water_info[4];         // how many, the time
    float cascade_depth[4];      // the world distance each cascade's depth range spans (0..1 in its map)
    float shadow_soft[4];        // soft shadows: tan of the sun's radius (0: off), contact shadows on, the blocker search's reach
    float occluders[4];          // 2D shadows: the casting map's top-left x and y, its tile size, on
    float occluder_size[4];      // its cells across and down
    float grid_box[4][4];        // irradiance volumes in use: center, the harmonics slot of the first probe
    float grid_ext[4][4];        // half size, intensity
    float grid_cells[4][4];      // probes along x, y and z; 1 once every probe has been captured
    float grid_info[4];          // how many
    float toon[4];               // a cel look (docs/design/rendering.md, Toon): x on, y light's bands, z the bands' softness
    float night[4];              // under an atmosphere, how far into the night (0 by day, 1 deep in it): the stars
    float weather[4];            // the Weather: wet, snow lying, drops of rain drawn, flakes of snow drawn
    float shelter_vp[16];        // the view from above the shelter map was drawn from
    float shelter[4];            // x 1 when there is one, y its depth bias
};
constexpr WGPUTextureFormat kPrepassDepth = WGPUTextureFormat_Depth32Float;
// The sun's cascades: an orthographic depth is linear, and 16 bits over a cascade's reach are
// millimetres against a bias of tenths of a unit, at half the memory traffic of 32-bit floats.
// The lights' atlas keeps 32-bit floats: a perspective depth crowds its precision near the light.
constexpr WGPUTextureFormat kCascadeDepth = WGPUTextureFormat_Depth16Unorm;
// The raw ambient occlusion: occlusion and contact shadow, and the view depth for the blur to weigh by.
constexpr WGPUTextureFormat kAoRawFormat = WGPUTextureFormat_RGBA16Float;
constexpr WGPUTextureFormat kVelocityFormat = WGPUTextureFormat_RG16Float;
// The id pass's surface targets (screen-space reflections): the normal (octahedral), roughness and
// metallic; and the albedo.
constexpr WGPUTextureFormat kSurfaceFormat = WGPUTextureFormat_RGBA16Float;
constexpr WGPUTextureFormat kAlbedoFormat = WGPUTextureFormat_RGBA8Unorm;
constexpr std::uint32_t kCascades = 4;
// The shadow map's layer after the cascades: what stands over the ground about the camera, seen from
// straight above, so rain and snow fall and lie only where the sky is open (docs/design/rendering.md,
// Weather).
constexpr std::uint32_t kShelterLayer = kCascades;
constexpr float kShelterReach = 32.0f;   // half the square it covers, centred on the camera
// The environment map: an equirectangular panorama with its GGX prefiltered levels as mips.
constexpr std::uint32_t kEnvWidth = 512, kEnvHeight = 256, kEnvLevels = 6;
// Reflection probes: each captured as six square views, turned into a panorama of its own (a layer
// of one array) with GGX-prefiltered mips like the sky's.
constexpr std::uint32_t kMaxProbes = 8;
constexpr std::uint32_t kProbeFace = 128;
constexpr std::uint32_t kProbeWidth = 256, kProbeHeight = 128, kProbeLevels = 5;
// Irradiance volumes: probes in grids, each captured as a reflection probe is and kept only as nine
// harmonics, in slots after the reflection probes' in the same buffer; a few captured a frame.
constexpr std::uint32_t kMaxGrids = 4;
constexpr std::uint32_t kMaxGridProbes = 1024;
constexpr std::uint32_t kGridPerFrame = 2;
// Captures a probe makes when it appears or is refreshed: each is lit by the one before, so a
// closed room's light bounces off its walls this many times (the first sees only the lights).
constexpr int kProbeBounces = 3;
// Bodies of water drawn at once (the first by id).
constexpr std::uint32_t kMaxWater = 8;
// Decals: the nearest in view, and their images (layers of one array, 0 the built-in spot).
constexpr std::uint32_t kMaxDecals = 64;   // the shaders' decals array is this long
constexpr std::uint32_t kDecalSize = 256, kDecalLayers = 16, kDecalLevels = 9;
// The built-in soft spot particles are drawn with when their emitter names no image.
constexpr const char* kDotTexture = "pocket:dot";
// Terrain layers (docs/design/terrain.md, Layers): each layer's image resampled into one array of this size.
constexpr std::uint32_t kLayerSize = 512, kLayerLevels = 10;
constexpr std::uint32_t kShadowMapSize = 2048;

constexpr std::size_t kMaxHighlights = 32;   // entities the post pass outlines in their own colour

struct alignas(16) ObjectUniforms {
    float model[16];
    float normal[16];
    float color[4];
    std::uint32_t id[4];      // x: entity id, y: flags (1 = unlit sprite or tile, 2 = casts no shadow, 4 = an unlit mesh)
    float uv_rect[4];         // u0, v0, u1, v1 (sprites cut a sheet; meshes use 0,0,1,1)
    float pbr[4];             // metallic, roughness, normal scale, 1 when a normal map is bound
    float emissive[4];        // linear RGB added after lighting; w: alpha cutoff (texels under it are cut out; 0 for none)
    std::uint32_t morph[4];   // x: first vec4 of the asset's morph deltas, y: vertices per target, z: targets weighed (0: none)
    float morph_weights[8];   // one per target, up to eight
    float prev_model[16];     // the model last frame (TAA's motion vectors); the model itself when new or TAA is off
    float sway[4];            // a swaying copy: how far its top leans (world units), sways a second, its mesh's local foot, 1 / its height; 0 reach for none
    float terrain[4];         // a terrain drawn from textured layers: x the layers (0 for none), yz the splat map's texels per unit of uv
    float layer_tile[4];      // each layer's repeats per unit of the mesh's uv
    float optics[4];          // glass and lacquer: transmission, index of refraction, clear coat, its roughness
    float volume[4];          // what the glass absorbs per unit (linear RGB), and its thickness in world units
    float sheen[4];           // cloth: the sheen's colour (linear RGB) and roughness
    float spec[4];            // the dielectric reflection's tint less 1 (linear RGB) and 1 less its strength: all 0 for a plain surface
    float aniso[4];           // brushed metal: strength, and the cosine and sine of its turn from the uv's u
    float user[4];            // a material the project wrote: its four numbers (MeshRenderer.material_params)
};
static_assert(sizeof(ObjectUniforms) == kObjectStride);

constexpr const char* kLineWgsl = R"WGSL(
struct LineFrame {
    view_proj: mat4x4f,
};
@group(0) @binding(0) var<uniform> frame: LineFrame;
struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) color: vec4f,
};
@vertex fn vs(@location(0) position: vec3f, @location(1) color: vec4f) -> VsOut {
    var out: VsOut;
    out.clip = frame.view_proj * vec4f(position, 1.0);
    out.clip.z = out.clip.z - 0.0005 * out.clip.w;  // a hair toward the camera: lines on a surface win
    // sRGB-authored colors into the linear scene.
    let c = color.rgb;
    out.color = vec4f(select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045)), color.a);
    return out;
}
@fragment fn fs(in: VsOut) -> @location(0) vec4f {
    return in.color;
}
)WGSL";

// GPU particles (docs/design/particles.md, On the GPU): one thread a slot of an emitter's ring. The
// window's births take the slots after the last ones, each born at its own moment of the window
// where the emitter was then; every live particle is moved through what is left of the window in
// steps of at most a sixtieth of a second: gravity, drag, turbulence, and the floor.
constexpr const char* kParticleSimWgsl = R"WGSL(
struct GpuParticle {
    pos: vec3f,
    age: f32,
    vel: vec3f,
    life: f32,
};
struct GpuEmitter {
    origin: vec4f,     // xyz where the emitter is at the window's end (0 for a local one), w the window's seconds
    prev: vec4f,       // xyz where it was at the window's start, w the seconds simulated before the window
    axis: vec4f,       // xyz the cone's axis in the world, w cos(spread)
    gravity: vec4f,    // xyz, w drag
    ranges: vec4f,     // speed least and most, life least and most
    ground: vec4f,     // x the floor in the particles' space, y bounce, z floor friction, w turbulence
    look: vec4f,       // x size at birth, y at death, z stretch, w one over the turbulence's scale
    color0: vec4f,     // linear, at birth
    color1: vec4f,     // at death
    place: vec4f,      // xyz added to every particle (a local emitter's position), w 1 for billboards
    counts: vec4u,     // x the ring's first new slot, y particles born in the window, z the ring's size, w seed
    extra: vec4u,      // x particles born before the window, y the entity's id
    depth_vp: mat4x4f, // the view the depth prepass was drawn from (last frame's, for the compute pass)
    depth_inv: mat4x4f,
    depth_info: vec4f, // x, y the prepass's size, z 1 when there is one, w 1 when the particles collide with it
    eye: vec4f,        // where that view's camera was
    area: vec4f,       // xyz the half extents of the box particles are born in, about the emitter (0: its point)
    turn: vec4f,       // the emitter's rotation, which turns that box
};
@group(0) @binding(0) var<storage, read_write> ring: array<GpuParticle>;
@group(0) @binding(1) var<uniform> e: GpuEmitter;
@group(0) @binding(2) var depth_tex: texture_depth_2d;

// A texel of the depth prepass back into the world.
fn depth_point(px: vec2i, z: f32) -> vec3f {
    let uv = (vec2f(px) + 0.5) / e.depth_info.xy;
    let w = e.depth_inv * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
    return w.xyz / w.w;
}

fn mixed(x: u32) -> u32 {
    var h = x;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}
fn rand(s: ptr<function, u32>) -> f32 {
    *s = mixed(*s + 0x9e3779b9u);
    return f32(*s >> 8u) / 16777216.0;
}
// The same field as the CPU's particles (renderer/src/particles.cpp).
fn lattice(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let h = mixed((bitcast<u32>(x) * 0x8da6b343u) ^ (bitcast<u32>(y) * 0xd8163841u) ^ (bitcast<u32>(z) * 0xcb1ab31fu) ^ (seed * 0x9e3779b9u));
    return f32(h >> 8u) / 16777216.0 * 2.0 - 1.0;
}
fn value_noise(p: vec3f, seed: u32) -> f32 {
    let f = floor(p);
    let i = vec3i(f);
    let t = p - f;
    let w = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let x00 = mix(lattice(i.x, i.y, i.z, seed), lattice(i.x + 1, i.y, i.z, seed), w.x);
    let x10 = mix(lattice(i.x, i.y + 1, i.z, seed), lattice(i.x + 1, i.y + 1, i.z, seed), w.x);
    let x01 = mix(lattice(i.x, i.y, i.z + 1, seed), lattice(i.x + 1, i.y, i.z + 1, seed), w.x);
    let x11 = mix(lattice(i.x, i.y + 1, i.z + 1, seed), lattice(i.x + 1, i.y + 1, i.z + 1, seed), w.x);
    return mix(mix(x00, x10, w.y), mix(x01, x11, w.y), w.z);
}
fn field(q: vec3f) -> vec3f {
    return vec3f(value_noise(q, 1u), value_noise(q, 2u), value_noise(q, 3u));
}
fn curl_noise(p0: vec3f, t: f32) -> vec3f {
    let p = p0 + vec3f(0.0, t * 0.25, 0.0);
    let h = 0.05;
    let dx = (field(p + vec3f(h, 0.0, 0.0)) - field(p - vec3f(h, 0.0, 0.0))) * (0.5 / h);
    let dy = (field(p + vec3f(0.0, h, 0.0)) - field(p - vec3f(0.0, h, 0.0))) * (0.5 / h);
    let dz = (field(p + vec3f(0.0, 0.0, h)) - field(p - vec3f(0.0, 0.0, h))) * (0.5 / h);
    return vec3f(dy.z - dz.y, dz.x - dx.z, dx.y - dy.x);
}

@compute @workgroup_size(64) fn simulate(@builtin(global_invocation_id) gid: vec3u) {
    let n = e.counts.z;
    let i = gid.x;
    if (i >= n) { return; }
    let dt = e.origin.w;
    let born_now = e.counts.y;
    var p = ring[i];
    var left = dt;
    let d = (i + n - e.counts.x) % n;   // which of the window's births this slot takes
    if (d < born_now) {
        var s = mixed(e.counts.w ^ mixed(e.extra.x + d));
        let a = e.axis.xyz;
        let cz = mix(1.0, e.axis.w, rand(&s));   // uniform over the cone's cap
        let sz = sqrt(max(0.0, 1.0 - cz * cz));
        let phi = 6.28318530718 * rand(&s);
        let helper = select(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), abs(a.y) < 0.99);
        let u = normalize(cross(helper, a));
        let v = cross(a, u);
        let speed = mix(e.ranges.x, e.ranges.y, rand(&s));
        let at = (f32(d) + 0.5) / f32(born_now);
        p.vel = (a * cz + u * (sz * cos(phi)) + v * (sz * sin(phi))) * speed;
        p.pos = mix(e.prev.xyz, e.origin.xyz, at);
        p.life = max(0.01, mix(e.ranges.z, e.ranges.w, rand(&s)));
        if (any(e.area.xyz > vec3f(0.0))) {
            // Somewhere in the box, turned with the emitter (drawn after the rest: the same births as before without one).
            let off = (vec3f(rand(&s), rand(&s), rand(&s)) * 2.0 - 1.0) * e.area.xyz;
            let q = e.turn;
            let t2 = cross(q.xyz, off) * 2.0;
            p.pos += off + t2 * q.w + cross(q.xyz, t2);
        }
        p.age = 0.0;
        left = dt * (1.0 - at);
    }
    if (p.life > 0.0 && p.age < p.life && left > 0.0) {
        let steps = u32(clamp(ceil(left * 60.0), 1.0, 8.0));
        let h = left / f32(steps);
        let keep = max(0.0, 1.0 - e.gravity.w * h);
        let slide = max(0.0, 1.0 - e.ground.z * h);
        var t = e.prev.w + dt - left;
        for (var k = 0u; k < steps; k++) {
            var push = e.gravity.xyz;
            if (e.ground.w != 0.0) { push += curl_noise((p.pos + e.place.xyz) * e.look.w, t) * e.ground.w; }
            p.vel = (p.vel + push * h) * keep;
            let was = p.pos;
            p.pos += p.vel * h;
            if (e.depth_info.w > 0.5) {
                // What is drawn: a particle that went behind the surface the prepass saw, by no more
                // than it moved (measured across the surface, so a glancing view does not let it
                // through), is put back and bounces off it (or slides along it).
                let wp = p.pos + e.place.xyz;
                let c = e.depth_vp * vec4f(wp, 1.0);
                if (c.w > 1e-4) {
                    let ndc = c.xyz / c.w;
                    let size = vec2i(e.depth_info.xy);
                    if (abs(ndc.x) < 1.0 && abs(ndc.y) < 1.0 && ndc.z > 0.0 && ndc.z < 1.0) {
                        let px = clamp(vec2i(vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * e.depth_info.xy), vec2i(0), size - vec2i(2));
                        let z = textureLoad(depth_tex, px, 0);
                        if (z < 1.0 && ndc.z > z) {
                            let surface = depth_point(px, z);
                            let a = depth_point(px + vec2i(1, 0), textureLoad(depth_tex, px + vec2i(1, 0), 0));
                            let b = depth_point(px + vec2i(0, 1), textureLoad(depth_tex, px + vec2i(0, 1), 0));
                            var n = cross(b - surface, a - surface);
                            if (dot(n, n) > 1e-12) {
                                n = normalize(n);
                                if (dot(n, e.eye.xyz - surface) < 0.0) { n = -n; }
                                let behind = -dot(wp - surface, n);
                                if (behind < length(p.vel) * h * 1.5 + 0.05) {
                                    p.pos = was;
                                    let into = -dot(p.vel, n);
                                    if (into > 0.0) {
                                        if (into * e.ground.y > 0.05) {
                                            p.vel += n * (into * (1.0 + e.ground.y));
                                        } else {
                                            p.vel = (p.vel + n * into) * slide;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if (p.pos.y < e.ground.x) {
                p.pos.y = e.ground.x;
                let into = -p.vel.y;
                if (into > 0.0 && into * e.ground.y > 0.05) {
                    p.vel.y = into * e.ground.y;
                } else {
                    p.vel.y = 0.0;
                    p.vel.x *= slide;
                    p.vel.z *= slide;
                }
            }
            t += h;
        }
        p.age += left;
    }
    ring[i] = p;
}
)WGSL";

// Render scale (docs/design/rendering.md, Render scale): the view drawn small, stretched up to the
// window by bilinear filtering and sharpened against its four neighbours, held within their range
// so edges do not ring.
constexpr const char* kUpscaleWgsl = R"WGSL(
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> up: vec4f;   // x sharpen, y unused, zw one over the window's size
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = pos.xy * up.zw;
    let c = textureSampleLevel(src, samp, uv, 0.0).rgb;
    if (up.x <= 0.0) { return vec4f(c, 1.0); }
    let t = 1.0 / vec2f(textureDimensions(src));
    let n = textureSampleLevel(src, samp, uv + vec2f(0.0, -t.y), 0.0).rgb;
    let s = textureSampleLevel(src, samp, uv + vec2f(0.0, t.y), 0.0).rgb;
    let e = textureSampleLevel(src, samp, uv + vec2f(t.x, 0.0), 0.0).rgb;
    let w = textureSampleLevel(src, samp, uv + vec2f(-t.x, 0.0), 0.0).rgb;
    let lo = min(min(min(n, s), min(e, w)), c);
    let hi = max(max(max(n, s), max(e, w)), c);
    let sharp = c + (c - (n + s + e + w) * 0.25) * (up.x * 2.0);
    return vec4f(clamp(sharp, lo, hi), 1.0);
}
)WGSL";

constexpr const char* kMeshWgsl = R"WGSL(
struct Frame {
    view_proj: mat4x4f,
    camera_pos: vec4f,
    sun_dir: vec4f,
    sun_color: vec4f,
    ambient: vec4f,
    clusters: vec4u,
    cluster_z: vec4f,
    viewport: vec4f,
    light_view_proj: mat4x4f,
    shadow: vec4f,
    inv_view_proj: mat4x4f,
    env: vec4f,
    sky: vec4f,
    cascade_vp: array<mat4x4f, 4>,
    cascade_far: vec4f,
    cascade_texel: vec4f,
    camera_fwd: vec4f,
    ao: vec4f,
    cur_view_proj: mat4x4f,
    prev_view_proj: mat4x4f,
    taa: vec4f,
    probe_box: array<vec4f, 8>,
    probe_ext: array<vec4f, 8>,
    probe_info: vec4f,
    decals: vec4f,
    clock: vec4f,
    wind: vec4f,
    wind_gust: vec4f,
    clouds: vec4f,
    cloud_drift: vec4f,
    waters: array<vec4f, 8>,
    water_info: vec4f,
    cascade_depth: vec4f,
    shadow_soft: vec4f,
    occluders: vec4f,
    occluder_size: vec4f,
    grid_box: array<vec4f, 4>,
    grid_ext: array<vec4f, 4>,
    grid_cells: array<vec4f, 4>,
    grid_info: vec4f,
    toon: vec4f,
    night: vec4f,
    weather: vec4f,
    shelter_vp: mat4x4f,
    shelter: vec4f,
};
@group(0) @binding(1) var shadow_map: texture_depth_2d_array;
// 2D shadows: the casting map's solid cells, one texel a cell (r 1 where solid).
@group(0) @binding(18) var occ_tex: texture_2d<f32>;
@group(0) @binding(2) var shadow_samp: sampler_comparison;
// The sky's light: the panorama with GGX-prefiltered mips (roughness 0 to 1), and its diffuse
// irradiance as nine spherical-harmonic coefficients (already divided by pi).
@group(0) @binding(3) var env_tex: texture_2d<f32>;
@group(0) @binding(4) var env_samp: sampler;
@group(0) @binding(5) var<uniform> sh: array<vec4f, 9>;
// Ambient occlusion at half resolution (white when it is off), and a clamping sampler for it.
@group(0) @binding(6) var ao_tex: texture_2d<f32>;
@group(0) @binding(7) var ao_samp: sampler;
// Point and spot lights, nearest first, and the froxel grid over them: per cluster the index of its
// first entry and its count, the entries (indices into local_lights) after the grid.
struct LocalLight { pos_range: vec4f, color_kind: vec4f, dir_cos: vec4f, cone: vec4f };
@group(0) @binding(8) var<storage, read> local_lights: array<LocalLight>;
@group(0) @binding(9) var<storage, read> cluster_data: array<u32>;
// Their shadows: per face the view it was drawn from and where it sits in the atlas.
struct ShadowFace { view_proj: mat4x4f, rect: vec4f };
@group(0) @binding(10) var<uniform> shadow_faces: array<ShadowFace, 64>;   // kMaxFaces
@group(0) @binding(11) var shadow_atlas: texture_depth_2d;
@group(0) @binding(12) var probe_env: texture_2d_array<f32>;
// Each probe's diffuse light: nine spherical harmonics of its capture (divided by pi), sixteen
// vec4s a probe apart: the reflection probes' (their layers' slots), then the irradiance volumes'.
@group(0) @binding(13) var<storage, read> probe_sh: array<vec4f>;
// The harmonics at slot `b` (its first vec4) evaluated at the normal.
fn sh_at(b: u32, n: vec3f) -> vec3f {
    return probe_sh[b].rgb * 0.282095
        + probe_sh[b + 1u].rgb * (0.488603 * n.y) + probe_sh[b + 2u].rgb * (0.488603 * n.z) + probe_sh[b + 3u].rgb * (0.488603 * n.x)
        + probe_sh[b + 4u].rgb * (1.092548 * n.x * n.y) + probe_sh[b + 5u].rgb * (1.092548 * n.y * n.z)
        + probe_sh[b + 6u].rgb * (0.315392 * (3.0 * n.z * n.z - 1.0)) + probe_sh[b + 7u].rgb * (1.092548 * n.x * n.z)
        + probe_sh[b + 8u].rgb * (0.546274 * (n.x * n.x - n.y * n.y));
}
// How much a grid probe sees of a point `dist` away along `dir` (from the probe): the probe kept,
// per texel of a 16 by 16 octahedral map after every slot's harmonics, the mean distance it saw to the
// nearest surface and the mean of its square; a point past the mean is weighed by Chebyshev's bound
// on its being in sight, cubed (a probe beyond a wall saw the wall short of the point).
fn probe_sees(probe: u32, dir: vec3f, dist: f32) -> f32 {
    let base = u32(frame.grid_info.y) + probe * 128u;
    let uv = clamp((oct_encode(dir) * 0.5 + 0.5) * 16.0 - 0.5, vec2f(0.0), vec2f(15.0));
    let t0 = vec2u(floor(uv));
    let t1 = min(t0 + vec2u(1u), vec2u(15u));
    let f = uv - floor(uv);
    var m = vec2f(0.0);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let t = vec2u(select(t0.x, t1.x, (k & 1u) == 1u), select(t0.y, t1.y, (k & 2u) == 2u));
        let w = select(1.0 - f.x, f.x, (k & 1u) == 1u) * select(1.0 - f.y, f.y, (k & 2u) == 2u);
        let i = t.y * 16u + t.x;
        let v = probe_sh[base + i / 2u];
        m = m + select(v.xy, v.zw, (i & 1u) == 1u) * w;
    }
    if (dist <= m.x) { return 1.0; }
    let variance = max(m.y - m.x * m.x, 1e-4);
    let gap = dist - m.x;
    let c = variance / (variance + gap * gap);
    return max(c * c * c, 0.0);
}
// The diffuse light of the irradiance volume a point is in (the first whose box, grown by half a
// unit, holds it, once all its probes are captured): the eight probes around it, each weighed by
// how near it is along each axis, by whether it lies in front of the surface (one behind, seeing
// the other side of a wall, counts for little) and, with the volume's visibility, by whether it saw
// the point, their harmonics at the normal. w as for probes.
fn grid_diffuse(p: vec3f, n: vec3f) -> vec4f {
    let count = u32(frame.grid_info.x);
    for (var i = 0u; i < count; i = i + 1u) {
        if (frame.grid_cells[i].w < 0.5) { continue; }
        let c = frame.grid_box[i].xyz;
        let e = frame.grid_ext[i].xyz;
        let local = p - c;
        let outside = length(max(abs(local) - e, vec3f(0.0)));
        if (outside > 0.5) { continue; }
        let cells = frame.grid_cells[i].xyz;
        let g = clamp((local + e) / (2.0 * e) * (cells - 1.0), vec3f(0.0), cells - 1.0);
        let g0 = min(floor(g), cells - 2.0);
        let f = g - g0;
        let seeing = frame.grid_cells[i].w > 1.5;
        // The point looked at from a little off its surface, so a probe does not lose it behind the
        // floor it stands on.
        let spacing = 2.0 * e / max(cells - 1.0, vec3f(1.0));
        let biased = p + n * (0.25 * min(min(spacing.x, spacing.y), spacing.z));
        var light = vec3f(0.0);
        var total = 0.0;
        for (var k = 0u; k < 8u; k = k + 1u) {
            let o = vec3f(f32(k & 1u), f32((k >> 1u) & 1u), f32((k >> 2u) & 1u));
            let at = g0 + o;
            let along = mix(1.0 - f, f, o);
            var w = along.x * along.y * along.z;
            let to = c - e + at / (cells - 1.0) * (2.0 * e) - p;
            let d = length(to);
            if (d > 1e-4) {
                let facing = (dot(to / d, n) + 1.0) * 0.5;
                w = w * (facing * facing + 0.2);
            }
            let slot = u32(frame.grid_box[i].w) + u32(at.x + at.y * cells.x + at.z * cells.x * cells.y);
            if (seeing) {
                let from_probe = biased - (c - e + at / (cells - 1.0) * (2.0 * e));
                let reach = length(from_probe);
                if (reach > 1e-4) { w = w * max(probe_sees(slot - u32(frame.grid_info.z), from_probe / reach, reach), 1e-3); }
            }
            light = light + sh_at(slot * 16u, n) * w;
            total = total + w;
        }
        if (total <= 0.0) { continue; }
        return vec4f(max(light / total, vec3f(0.0)) * frame.grid_ext[i].w, 1.0 - outside / 0.5);
    }
    return vec4f(0.0);
}
// The reflection probe a point is in (the first whose box holds it, a room's own walls and floor
// included): what arrives along the mirror direction from its capture, box-projected; w is how much
// it counts, 1 in the box and fading over half a unit outside it, 0 away from every probe.
fn probe_specular(p: vec3f, n: vec3f, v: vec3f, roughness: f32) -> vec4f {
    let count = u32(frame.probe_info.w);
    let r = reflect(-v, n);
    for (var i = 0u; i < count; i = i + 1u) {
        let c = frame.probe_box[i].xyz;
        let e = frame.probe_ext[i].xyz;
        let local = p - c;
        let outside = length(max(abs(local) - e, vec3f(0.0)));
        if (outside > 0.5) { continue; }
        var dir = r;
        if (frame.probe_ext[i].w > 0.0) {
            // Where the mirror ray leaves the box, seen from the probe's center.
            let t = max((e - local) / r, (-e - local) / r);
            dir = normalize(local + r * max(min(min(t.x, t.y), t.z), 0.0));
        }
        let s = textureSampleLevel(probe_env, env_samp, env_uv(dir), i32(frame.probe_box[i].w), roughness * frame.probe_info.y).rgb;
        return vec4f(s * abs(frame.probe_ext[i].w), 1.0 - outside / 0.5);
    }
    return vec4f(0.0);
}
// The diffuse light of the reflection probe a point is in (the same box as its reflections): its
// capture's harmonics at the normal; w is how much it counts, as for the reflections.
fn probe_diffuse(p: vec3f, n: vec3f) -> vec4f {
    let count = u32(frame.probe_info.x);
    for (var i = 0u; i < count; i = i + 1u) {
        let local = p - frame.probe_box[i].xyz;
        let outside = length(max(abs(local) - frame.probe_ext[i].xyz, vec3f(0.0)));
        if (outside > 0.5) { continue; }
        let c = sh_at(u32(frame.probe_box[i].w) * 16u, n);
        return vec4f(max(c, vec3f(0.0)) * abs(frame.probe_ext[i].w), 1.0 - outside / 0.5);
    }
    return vec4f(0.0);
}
// Decals (docs/design/rendering.md, Decals): the boxes in view, each an image projected along its
// -y onto whatever lies inside it, painted in order over the surface before it is lit (and before
// the id pass writes the surface, so reflections see a wet puddle).
struct Decal {
    inv: mat4x4f,      // the world into the box, -0.5 to 0.5 along each axis
    axis: vec4f,       // the projection's direction, the cosine past which surfaces fade
    color: vec4f,      // tint, opacity
    params: vec4f,     // image layer, roughness (negative: the surface's), glow, normal map layer (0 none)
    sphere: vec4f,     // around the box: centre, radius squared
    extra: vec4f,      // bumpiness
};
@group(0) @binding(14) var<uniform> decals: array<Decal, 64>;   // kMaxDecals
@group(0) @binding(15) var decal_tex: texture_2d_array<f32>;
@group(0) @binding(16) var decal_samp: sampler;
struct Painted {
    albedo: vec3f,
    roughness: f32,
    metallic: f32,
    glow: vec3f,
    normal: vec3f,
};
fn paint_decals(p: vec3f, n: vec3f, dp1: vec3f, dp2: vec3f, surface: Painted) -> Painted {
    var out = surface;
    let count = u32(frame.decals.x);
    for (var i = 0u; i < count; i = i + 1u) {
        let d = decals[i];
        let to = p - d.sphere.xyz;
        if (dot(to, to) > d.sphere.w) { continue; }
        let l = (d.inv * vec4f(p, 1.0)).xyz;
        if (any(abs(l) > vec3f(0.5))) { continue; }
        // Faint on surfaces turned away from the projection and toward the box's two ends.
        let fade = smoothstep(d.axis.w, min(d.axis.w + 0.2, 1.0), dot(n, -d.axis.xyz)) * (1.0 - smoothstep(0.4, 0.5, abs(l.y)));
        if (fade <= 0.0) { continue; }
        let t = textureSampleGrad(decal_tex, decal_samp, l.xz + vec2f(0.5), i32(d.params.x), (d.inv * vec4f(dp1, 0.0)).xz, (d.inv * vec4f(dp2, 0.0)).xz);
        let a = clamp(t.a * d.color.a * fade, 0.0, 1.0);
        out.albedo = mix(out.albedo, t.rgb * d.color.rgb, a);
        out.metallic = mix(out.metallic, 0.0, a);
        if (d.params.y >= 0.0) { out.roughness = mix(out.roughness, clamp(d.params.y, 0.04, 1.0), a); }
        out.glow = out.glow + t.rgb * d.color.rgb * (d.params.z * a);
        if (d.params.w > 0.5) {
            // Its normal map bends the surface: the image's x along the box's x, its y (up the
            // image) along the box's -z, both laid flat on the surface.
            let m = textureSampleGrad(decal_tex, decal_samp, l.xz + vec2f(0.5), i32(d.params.w), (d.inv * vec4f(dp1, 0.0)).xz, (d.inv * vec4f(dp2, 0.0)).xz).xyz * 2.0 - 1.0;
            let bx = normalize(vec3f(d.inv[0][0], d.inv[1][0], d.inv[2][0]));
            let bz = normalize(vec3f(d.inv[0][2], d.inv[1][2], d.inv[2][2]));
            let tx = normalize(bx - out.normal * dot(bx, out.normal));
            let tz = normalize(bz - out.normal * dot(bz, out.normal));
            let bent = normalize(out.normal * max(m.z, 0.05) + (tx * m.x - tz * m.y) * d.extra.x);
            out.normal = normalize(mix(out.normal, bent, a));
        }
    }
    return weathered(p, out);
}
// Weather on a surface (docs/design/rendering.md, Weather): wet, it darkens as porous things do
// and takes a damp sheen, most where it faces up, less on walls, metal not darkened; on what is
// flat, water stands in puddles (where a broad noise is high, more of them the wetter), dark and
// mirror-smooth. Snow lies on what faces up, in patches while there is little of it, white and matte.
fn weather_noise(q: vec2f) -> f32 {
    let i = floor(q);
    let f = fract(q);
    let u = f * f * (3.0 - 2.0 * f);
    let h = vec4f(dot(i, vec2f(127.1, 311.7)), dot(i + vec2f(1.0, 0.0), vec2f(127.1, 311.7)), dot(i + vec2f(0.0, 1.0), vec2f(127.1, 311.7)), dot(i + vec2f(1.0, 1.0), vec2f(127.1, 311.7)));
    let r = fract(sin(h) * 43758.5453);
    return mix(mix(r.x, r.y, u.x), mix(r.z, r.w, u.x), u.y);
}
// How open to the sky a point is (1 open, 0 under something), by the shelter map: the topmost
// surface over each spot of the ground about the camera, from straight above. Outside it, open.
fn open_sky(p: vec3f) -> f32 {
    if (frame.shelter.x < 0.5) { return 1.0; }
    let sp = frame.shelter_vp * vec4f(p, 1.0);
    let ndc = sp.xyz / sp.w;
    if (abs(ndc.x) > 0.99 || abs(ndc.y) > 0.99 || ndc.z < 0.0 || ndc.z > 1.0) { return 1.0; }
    return textureSampleCompareLevel(shadow_map, shadow_samp, vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5), 4, ndc.z - frame.shelter.y);
}
// Rings spreading where drops land on standing water: two offset grids of cells half a unit across,
// each cell one drop at its own place and time, its ring widening and fading over six tenths of the
// cell's turn; the slope of the rings, to bend the puddle's normal by.
fn ripples(p: vec2f, t: f32) -> vec2f {
    var g = vec2f(0.0);
    for (var layer = 0; layer < 2; layer = layer + 1) {
        let q = p * 2.0 + vec2f(f32(layer) * 0.5, f32(layer) * 0.37);
        let cell = floor(q);
        let h = fract(sin(vec2f(dot(cell, vec2f(127.1, 311.7)), dot(cell, vec2f(269.5, 183.3))) + f32(layer) * 3.1) * 43758.5453);
        let cycle = fract(t * 1.3 + h.x * 7.0 + h.y * 3.0);
        if (cycle > 0.6) { continue; }   // a ring for six tenths of the cell's turn, then still water
        let phase = cycle / 0.6;
        let v = q - (cell + 0.25 + 0.5 * h);
        let d = length(v);
        let x = (d - phase * 0.45) * 28.0;
        if (abs(x) < 3.14159) {
            g = g + (v / max(d, 1e-4)) * sin(x) * (0.5 + 0.5 * cos(x)) * (1.0 - phase);
        }
    }
    return g;
}
fn weathered(p: vec3f, surface: Painted) -> Painted {
    var out = surface;
    // Looked up a little out along the surface, so a wall in the open takes the rain its side does.
    let open = open_sky(p + out.normal * 0.2);
    let wet = frame.weather.x * open;
    let cover = frame.weather.y * open;
    if (wet <= 0.0 && cover <= 0.0) { return out; }
    let up = smoothstep(0.25, 0.9, out.normal.y);
    if (wet > 0.0) {
        let soak = wet * mix(0.45, 1.0, up);
        out.albedo = out.albedo * mix(1.0, 0.6, soak * (1.0 - out.metallic));
        out.roughness = mix(out.roughness, min(out.roughness, 0.3), soak);
        let puddle = smoothstep(0.7 - 0.25 * wet, 0.76 - 0.25 * wet, weather_noise(p.xz * 0.3 + vec2f(17.0, 3.0))) * smoothstep(0.85, 0.97, out.normal.y) * smoothstep(0.3, 0.7, wet);
        out.albedo = out.albedo * mix(1.0, 0.55, puddle);
        out.roughness = mix(out.roughness, 0.03, puddle);
        out.metallic = mix(out.metallic, 0.0, puddle);
        out.normal = normalize(mix(out.normal, vec3f(0.0, 1.0, 0.0), puddle));   // standing water is flat over the bumps under it
        if (puddle > 0.0 && frame.weather.z > 0.0) {
            // While it rains, the drops' rings on the puddles, more of them the harder it rains; gone by
            // sixteen units off, where a ring would be finer than the pixels and only glitter.
            let near = 1.0 - smoothstep(8.0, 16.0, distance(p, frame.camera_pos.xyz));
            let g = ripples(p.xz, frame.clock.x) * min(frame.weather.z / 4500.0, 1.0) * near;
            out.normal = normalize(out.normal + vec3f(g.x, 0.0, g.y) * 0.3 * puddle);
        }
    }
    if (cover > 0.0) {
        // Lying where the patches' noise is under the cover (so about that much of flat ground), all
        // of it at full cover.
        let patches = weather_noise(p.xz * 0.9) * 0.65 + weather_noise(p.xz * 3.7) * 0.35;
        let lie = smoothstep(-0.05, 0.05, up * cover * 1.12 - patches);
        out.albedo = mix(out.albedo, vec3f(0.8, 0.83, 0.88), lie);
        out.roughness = mix(out.roughness, 0.65, lie);
        out.metallic = mix(out.metallic, 0.0, lie);
    }
    return out;
}
// How lit a point is by one face of a local light's shadow (0 shadowed .. 1 lit): moved off the
// surface along its normal and toward the light by a texel's size there, filtered 3x3 inside the face.
fn face_lit(face: i32, world_pos: vec3f, gn: vec3f, pl: vec3f, texel: f32) -> f32 {
    let f = shadow_faces[face];
    let p = world_pos + gn * (texel * 1.5) + pl * texel;
    let sp = f.view_proj * vec4f(p, 1.0);
    if (sp.w <= 0.0) { return 1.0; }
    let ndc = sp.xyz / sp.w;
    if (abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) { return 1.0; }
    let uv = f.rect.xy + vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * f.rect.z;
    let lo = f.rect.xy + vec2f(f.rect.w * 0.5);
    let hi = f.rect.xy + vec2f(f.rect.z - f.rect.w * 0.5);
    var lit = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            lit = lit + textureSampleCompareLevel(shadow_atlas, shadow_samp, clamp(uv + vec2f(f32(i), f32(j)) * f.rect.w, lo, hi), ndc.z);
        }
    }
    return lit / 9.0;
}
fn env_uv(d: vec3f) -> vec2f {
    return vec2f(atan2(d.x, -d.z) / 6.2831853 + 0.5, acos(clamp(d.y, -1.0, 1.0)) / 3.14159265);
}
fn sh_irradiance(n: vec3f) -> vec3f {
    let c = sh[0].rgb * 0.282095
        + sh[1].rgb * (0.488603 * n.y) + sh[2].rgb * (0.488603 * n.z) + sh[3].rgb * (0.488603 * n.x)
        + sh[4].rgb * (1.092548 * n.x * n.y) + sh[5].rgb * (1.092548 * n.y * n.z)
        + sh[6].rgb * (0.315392 * (3.0 * n.z * n.z - 1.0)) + sh[7].rgb * (1.092548 * n.x * n.z)
        + sh[8].rgb * (0.546274 * (n.x * n.x - n.y * n.y));
    return max(c, vec3f(0.0));
}
// Karis's analytic fit of the split-sum environment BRDF.
fn env_brdf(f0: vec3f, roughness: f32, ndv: f32) -> vec3f {
    let c0 = vec4f(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4f(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * ndv)) * r.x + r.y;
    let ab = vec2f(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}
struct Object {
    model: mat4x4f,
    normal: mat4x4f,
    color: vec4f,
    id: vec4u,
    uv_rect: vec4f,
    pbr: vec4f,
    emissive: vec4f,
    morph: vec4u,
    morph_weights: array<vec4f, 2>,
    prev_model: mat4x4f,
    sway: vec4f,
    terrain: vec4f,
    layer_tile: vec4f,
    optics: vec4f,
    volume: vec4f,
    sheen: vec4f,
    spec: vec4f,
    aniso: vec4f,
    user: vec4f,
};
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<storage, read> objects: array<Object>;
@group(1) @binding(1) var<storage, read> joints: array<mat4x4f>;
// Morph target deltas of every morphed asset: per target, per vertex, a position delta then a
// normal delta (vec4 each), starting at object.morph.x for the instance's asset.
@group(1) @binding(2) var<storage, read> morphs: array<vec4f>;
fn morph_position(object: Object, vid: u32, p: vec3f) -> vec3f {
    var out = p;
    for (var t = 0u; t < object.morph.z; t = t + 1u) {
        let w = object.morph_weights[t / 4u][t % 4u];
        if (w != 0.0) { out = out + w * morphs[object.morph.x + (t * object.morph.y + vid) * 2u].xyz; }
    }
    return out;
}
fn morph_normal(object: Object, vid: u32, n: vec3f) -> vec3f {
    var out = n;
    for (var t = 0u; t < object.morph.z; t = t + 1u) {
        let w = object.morph_weights[t / 4u][t % 4u];
        if (w != 0.0) { out = out + w * morphs[object.morph.x + (t * object.morph.y + vid) * 2u + 1u].xyz; }
    }
    return normalize(out);
}
@group(2) @binding(0) var base_tex: texture_2d<f32>;
@group(2) @binding(1) var base_samp: sampler;
@group(2) @binding(2) var mr_tex: texture_2d<f32>;
@group(2) @binding(3) var normal_tex: texture_2d<f32>;
@group(2) @binding(4) var emissive_tex: texture_2d<f32>;
@group(2) @binding(6) var splat_tex: texture_2d<f32>;
@group(2) @binding(7) var layer_tex: texture_2d_array<f32>;
// A terrain's ground from its textured layers (docs/design/terrain.md, Layers): each layer's image
// tiled at its own scale, mixed by the layers' shares where the fragment is (the splat map's texels
// sit on the grid's samples). Explicit gradients: it runs only for terrains.
fn terrain_ground(object: Object, uv: vec2f, duv1: vec2f, duv2: vec2f) -> vec4f {
    let w = textureSampleLevel(splat_tex, base_samp, uv * object.terrain.yz + vec2f(0.5), 0.0);
    var c = vec3f(0.0);
    let n = u32(object.terrain.x);
    for (var i = 0u; i < n; i = i + 1u) {
        let k = object.layer_tile[i];
        if (w[i] > 0.002) {
            c = c + w[i] * textureSampleGrad(layer_tex, base_samp, uv * k, i32(i), duv1 * k, duv2 * k).rgb;
        }
    }
    return vec4f(c, 1.0);
}
// The terrain's base colour: its layers under the MeshRenderer's colour, and the painted colour laid
// over them by its coverage (the vertex colour's alpha).
fn terrain_base(object: Object, in: VsOut, duv1: vec2f, duv2: vec2f) -> vec4f {
    let ground = terrain_ground(object, in.uv, duv1, duv2).rgb * object.color.rgb;
    return vec4f(mix(ground, in.color.rgb, clamp(in.color.a / max(object.color.a, 1e-4), 0.0, 1.0)), object.color.a);
}

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) world_pos: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) color: vec4f,
    @location(4) @interpolate(flat) id: u32,
    @location(5) @interpolate(flat) instance: u32,
    @location(6) cur: vec4f,    // where the point is this frame and was last frame, unjittered (the id pass's motion)
    @location(7) prev: vec4f,
    @location(8) tangent: vec4f,   // along the uv's u in the world, w the bitangent's side; 0 for none
};
// A vertex's tangent into the world by a model matrix (its side turned over by a mirroring one).
fn world_tangent(model: mat4x4f, t: vec4f) -> vec4f {
    let w = (model * vec4f(t.xyz, 0.0)).xyz;
    if (dot(w, w) < 1e-12 || t.w == 0.0) { return vec4f(0.0); }
    let mirror = select(1.0, -1.0, dot(cross(model[0].xyz, model[1].xyz), model[2].xyz) < 0.0);
    return vec4f(normalize(w), t.w * mirror);
}

fn vertex_color(c: vec4f) -> vec4f {
    return vec4f(select(pow((c.rgb + 0.055) / 1.055, vec3f(2.4)), c.rgb / 12.92, c.rgb <= vec3f(0.04045)), c.a);
}
// A scattered copy in the wind (docs/design/terrain.md, Scattering): what is above its foot leans
// by the square of its height there, at seconds t. With a Wind (docs/design/wind.md) it leans
// downwind by its sway at 3 units a second, more in stronger wind, and fluttering with the gusts
// that run along it (world/wind.cpp's wind_gust); without one, it sways about its place in a
// breeze that runs across the field (neighbours move alike, far ones not).
fn sway(object: Object, local: vec3f, t: f32) -> vec3f {
    if (object.sway.x == 0.0) { return vec3f(0.0); }
    let h = clamp((local.y - object.sway.z) * object.sway.w, 0.0, 1.0);
    let at = object.model[3].xyz;
    let phase = dot(at.xz, vec2f(0.21, 0.13));
    let w = t * object.sway.y * 6.2831853;
    if (frame.wind.w > 0.5) {
        let d = frame.wind.xy;
        let k = frame.wind_gust.y;
        let along = dot(at.xz, d) - frame.wind.z * t;
        let across = dot(at.xz, vec2f(-d.y, d.x));
        let gust = 0.65 * sin(k * along) + 0.35 * sin(2.7 * k * along + 0.9 * k * across);
        let strength = frame.wind.z / 3.0 * (1.0 + frame.wind_gust.x * gust);
        let flutter = sin(w - phase);
        let lean = d * (strength * (0.8 + 0.2 * flutter)) + vec2f(-d.y, d.x) * (0.12 * strength * flutter);
        return vec3f(lean.x, 0.0, lean.y) * (object.sway.x * h * h);
    }
    let lean = vec2f(sin(w - phase) + 0.3 * sin(2.3 * w - 1.7 * phase), 0.45 * sin(1.3 * w - 0.8 * phase));
    return vec3f(lean.x, 0.0, lean.y) * (object.sway.x * h * h / 1.3);
}
@vertex fn vs(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f, @location(5) vcolor: vec4f, @location(6) tangent: vec4f) -> VsOut {
    let object = objects[instance];
    var out: VsOut;
    let local = vec4f(morph_position(object, vid, position), 1.0);
    let world = object.model * local + vec4f(sway(object, local.xyz, frame.clock.x), 0.0);
    out.clip = frame.view_proj * world;
    out.cur = frame.cur_view_proj * world;
    out.prev = frame.prev_view_proj * (object.prev_model * local + vec4f(sway(object, local.xyz, frame.clock.y), 0.0));
    out.world_pos = world.xyz;
    out.normal = normalize((object.normal * vec4f(morph_normal(object, vid, normal), 0.0)).xyz);
    out.tangent = world_tangent(object.model, tangent);
    out.uv = mix(object.uv_rect.xy, object.uv_rect.zw, uv);
    out.color = object.color * vertex_color(vcolor);
    out.id = object.id.x;
    out.instance = instance;
    return out;
}

// Shadow pass: depth only, from the sun, once per cascade (its matrix comes with a dynamic offset).
struct Cascade { view_proj: mat4x4f };
@group(2) @binding(5) var<uniform> cascade: Cascade;
@vertex fn vs_shadow(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f) -> @builtin(position) vec4f {
    let object = objects[instance];
    if ((object.id.y & 2u) != 0u) { return vec4f(0.0, 0.0, -2.0, 1.0); }   // casts no shadow: behind the near plane, clipped
    let local = morph_position(object, vid, position);
    return cascade.view_proj * (object.model * vec4f(local, 1.0) + vec4f(sway(object, local, frame.clock.x), 0.0));
}

// Cut-outs in the shadow passes: the same placement with the uv, and a fragment that drops what the
// texture's alpha cuts away (the material at group 3; the cascade or light face at group 2).
struct ShadowCut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) @interpolate(flat) instance: u32,
};
@group(3) @binding(0) var cut_tex: texture_2d<f32>;
@group(3) @binding(1) var cut_samp: sampler;
@vertex fn vs_shadow_cut(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f) -> ShadowCut {
    let object = objects[instance];
    var out: ShadowCut;
    let local = morph_position(object, vid, position);
    out.clip = cascade.view_proj * (object.model * vec4f(local, 1.0) + vec4f(sway(object, local, frame.clock.x), 0.0));
    if ((object.id.y & 2u) != 0u) { out.clip = vec4f(0.0, 0.0, -2.0, 1.0); }
    out.uv = mix(object.uv_rect.xy, object.uv_rect.zw, uv);
    out.instance = instance;
    return out;
}
@fragment fn fs_shadow_cut(in: ShadowCut) {
    let object = objects[in.instance];
    let a = textureSample(cut_tex, cut_samp, in.uv).a * object.color.a;
    if (a < object.emissive.w) { discard; }
}

// Skinned meshes: the joint matrices of this instance start at object.id.z in the joints array.
fn skin_matrix(base: u32, j: vec4u, w: vec4f) -> mat4x4f {
    return joints[base + j.x] * w.x + joints[base + j.y] * w.y + joints[base + j.z] * w.z + joints[base + j.w] * w.w;
}

@vertex fn vs_skinned(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f, @location(3) j: vec4u, @location(4) w: vec4f, @location(5) vcolor: vec4f, @location(6) tangent: vec4f) -> VsOut {
    let object = objects[instance];
    let skin = skin_matrix(object.id.z, j, w);
    let model = object.model * skin;
    var out: VsOut;
    let local = vec4f(morph_position(object, vid, position), 1.0);
    let world = model * local;
    out.clip = frame.view_proj * world;
    out.cur = frame.cur_view_proj * world;
    out.prev = frame.prev_view_proj * (object.prev_model * skin * local);   // this frame's pose: the joints' own motion is not followed
    out.world_pos = world.xyz;
    out.normal = normalize((model * vec4f(morph_normal(object, vid, normal), 0.0)).xyz);
    out.tangent = world_tangent(model, tangent);
    out.uv = mix(object.uv_rect.xy, object.uv_rect.zw, uv);
    out.color = object.color * vertex_color(vcolor);
    out.id = object.id.x;
    out.instance = instance;
    return out;
}

@vertex fn vs_shadow_skinned(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f, @location(3) j: vec4u, @location(4) w: vec4f) -> @builtin(position) vec4f {
    let object = objects[instance];
    if ((object.id.y & 2u) != 0u) { return vec4f(0.0, 0.0, -2.0, 1.0); }
    let model = object.model * skin_matrix(object.id.z, j, w);
    return cascade.view_proj * (model * vec4f(morph_position(object, vid, position), 1.0));
}

struct FsOut {
    @location(0) color: vec4f,
    @location(1) id: u32,
};

// Tangent frame from screen-space derivatives (no vertex tangents needed), then the map's
// +Y-up (glTF) normal bent into world space. WebGPU's dpdy runs down the screen, so this frame's
// t and b point against the uv's u and v: b is the texture's up, and the map's x turns over.
fn perturb_normal(n: vec3f, dp1: vec3f, dp2: vec3f, duv1: vec2f, duv2: vec2f, map: vec3f) -> vec3f {
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let invmax = inverseSqrt(max(dot(t, t), dot(b, b)) + 1e-12);
    let tbn = mat3x3f(t * invmax, b * invmax, n);
    return normalize(tbn * vec3f(-map.x, map.y, map.z));
}
// A normal map's normal: through the vertex's tangent frame when it has one (smooth across
// triangles, glTF's convention: the bitangent is cross(n, t) * w, toward the texture's up), else
// through the frame from the uv's screen derivatives (constant over each triangle).
// The uv a surface is textured at: its mesh's, or with MeshRenderer.texture_tile (flag 8) the
// world's place on the face of the axis it faces most, uv_rect.x repeats to a unit, the image's top
// up a wall.
fn surface_uv(object: Object, in: VsOut) -> vec2f {
    if ((object.id.y & 8u) == 0u) { return in.uv; }
    let a = abs(in.normal);
    let p = in.world_pos;
    var uv = vec2f(p.x, p.z);
    if (a.x > a.y && a.x >= a.z) { uv = vec2f(-p.z * sign(in.normal.x), -p.y); }
    else if (a.z > a.y && a.z > a.x) { uv = vec2f(p.x * sign(in.normal.z), -p.y); }
    return uv * object.uv_rect.x;
}

fn mapped_normal(in: VsOut, n: vec3f, dp1: vec3f, dp2: vec3f, duv1: vec2f, duv2: vec2f, map: vec3f) -> vec3f {
    let t = in.tangent.xyz - n * dot(n, in.tangent.xyz);
    if (dot(t, t) > 0.01) {
        let tn = normalize(t);
        return normalize(mat3x3f(tn, cross(n, tn) * in.tangent.w, n) * map);
    }
    return perturb_normal(n, dp1, dp2, duv1, duv2, map);
}

// GGX / Schlick / Smith specular plus Lambert diffuse for one light direction; the diffuse term
// is not divided by pi (and the specular scaled to match) so brightness stays comparable to a plain
// Lambert surface under a light of intensity one.
// The scene as drawn before the glass (docs/design/rendering.md, Glass): what transmitting
// surfaces show through them. A black texel where no glass is drawn.
@group(0) @binding(17) var glass_scene: texture_2d<f32>;

// A clear coat (glTF KHR_materials_clearcoat): a dielectric specular layer on the geometric normal
// over the surface. Its reflection of a light (rgb), and what it leaves the layer below (w).
fn coat(object: Object, gn: vec3f, v: vec3f, l: vec3f) -> vec4f {
    let h = normalize(l + v);
    let ndl = max(dot(gn, l), 0.0);
    let ndv = max(dot(gn, v), 1e-4);
    let ndh = max(dot(gn, h), 0.0);
    let r = clamp(object.optics.w, 0.03, 1.0);
    let a2 = r * r * r * r;
    let denom = ndh * ndh * (a2 - 1.0) + 1.0;
    let d = a2 / max(denom * denom, 1e-6);
    let k = (r + 1.0) * (r + 1.0) / 8.0;
    let g = (ndl / (ndl * (1.0 - k) + k)) * (ndv / (ndv * (1.0 - k) + k));
    let f = (0.04 + 0.96 * pow(1.0 - max(dot(v, h), 0.0), 5.0)) * object.optics.z;
    return vec4f(vec3f(d * g * f / max(4.0 * ndv, 1e-4)), 1.0 - f);
}
// KHR_materials_specular's tint and strength, kept as their differences from a plain surface's.
fn spec_tint(object: Object) -> vec3f { return vec3f(1.0) + object.spec.rgb; }
fn spec_level(object: Object) -> f32 { return 1.0 - object.spec.w; }
// The surface's response to a light (docs/design/rendering.md, Cloth, specular and brushed metal):
// its specular layer's strength and tint (KHR_materials_specular), a highlight stretched along
// `at` for brushed metal (KHR_materials_anisotropy: anisotropic GGX with its height-correlated
// Smith visibility), a sheen over it for cloth (KHR_materials_sheen: the Charlie distribution with
// Neubelt's visibility, the layer below dimmed by what the sheen takes), and its clear coat.
// A cel look (docs/design/rendering.md, Toon): the light on a surface in a few flat bands, each
// edge softened a little so it does not crawl.
fn toon_band(x: f32) -> f32 {
    if (frame.toon.x < 0.5) { return x; }
    let bands = max(frame.toon.y, 1.0);
    let y = x * bands;
    return (floor(y) + smoothstep(0.5 - frame.toon.z, 0.5 + frame.toon.z, fract(y))) / bands;
}
fn surface_light(object: Object, gn: vec3f, n: vec3f, v: vec3f, l: vec3f, albedo: vec3f, metallic: f32, roughness: f32, at: vec3f) -> vec3f {
    let h = normalize(l + v);
    let ndl = toon_band(max(dot(n, l), 0.0));
    let ndv = max(dot(n, v), 1e-4);
    let ndh = max(dot(n, h), 0.0);
    let vdh = max(dot(v, h), 0.0);
    let a = roughness * roughness;
    let f0 = mix(min(vec3f(0.04) * spec_tint(object), vec3f(1.0)) * spec_level(object), albedo, metallic);
    let f90 = mix(spec_level(object), 1.0, metallic);
    let f = f0 + (vec3f(f90) - f0) * pow(1.0 - vdh, 5.0);
    var spec: vec3f;
    if (object.aniso.x > 0.0) {
        let b = cross(n, at);
        let ta = max(mix(a, 1.0, object.aniso.x * object.aniso.x), 1e-3);
        let ba = max(a, 1e-3);
        let th = dot(at, h);
        let bh = dot(b, h);
        let dd = th * th / (ta * ta) + bh * bh / (ba * ba) + ndh * ndh;
        let d = 1.0 / max(ta * ba * dd * dd, 1e-6);   // times pi, as below
        let lv = ndl * length(vec3f(ta * dot(at, v), ba * dot(b, v), ndv));
        let lt = ndv * length(vec3f(ta * dot(at, l), ba * dot(b, l), ndl));
        spec = d * (0.5 / max(lv + lt, 1e-5)) * f * ndl;
    } else {
        let a2 = a * a;
        let denom = ndh * ndh * (a2 - 1.0) + 1.0;
        let d = a2 / max(denom * denom, 1e-6);            // GGX, times pi
        let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
        let g = (ndl / (ndl * (1.0 - k) + k)) * (ndv / (ndv * (1.0 - k) + k));
        spec = d * g * f / max(4.0 * ndv, 1e-4);
    }
    let diffuse = albedo * (1.0 - metallic) * (vec3f(1.0) - f);
    var base = diffuse * ndl + spec;
    let sheen_max = max(object.sheen.r, max(object.sheen.g, object.sheen.b));
    if (sheen_max > 0.0) {
        let sa = max(object.sheen.w, 0.07);
        let inv = 1.0 / (sa * sa);
        let sin2 = max(1.0 - ndh * ndh, 0.0078125);
        let ds = (2.0 + inv) * pow(sin2, inv * 0.5) * 0.5;   // Charlie, times pi
        let vs = 1.0 / max(4.0 * (ndl + ndv - ndl * ndv), 1e-4);
        base = base * (1.0 - sheen_max * 0.157) + object.sheen.rgb * (ds * vs * ndl);
    }
    if (object.optics.z <= 0.0) { return base; }
    let c = coat(object, gn, v, l);
    return base * c.w + c.rgb;
}
// What glass lets through (glTF KHR_materials_transmission, _ior, _volume): the scene behind it
// where the refracted ray leaves its far side (a thin pane, thickness 0, bends nothing), blurred
// by its roughness, absorbed over its thickness, tinted by its colour and less what its surface
// reflects.
fn transmitted(object: Object, p: vec3f, n: vec3f, v: vec3f, albedo: vec3f, roughness: f32) -> vec3f {
    let ior = max(object.optics.y, 1.0);
    let depth = max(object.volume.w, 0.0);
    let exit = p + refract(-v, n, 1.0 / ior) * depth;
    let clip = frame.view_proj * vec4f(exit, 1.0);
    let ndc = clip.xy / max(clip.w, 1e-4);
    let size = vec2f(textureDimensions(glass_scene));
    let uv = (frame.viewport.xy + (vec2f(ndc.x, -ndc.y) * 0.5 + 0.5) * frame.viewport.zw) / size;
    var behind = textureSampleLevel(glass_scene, ao_samp, uv, 0.0).rgb;
    let spread = roughness * roughness * 0.15;
    if (spread > 0.0005) {
        // Frosted: taps on a disc (a golden-angle spiral) as wide as the roughness asks.
        for (var i = 1; i < 12; i = i + 1) {
            let a = f32(i) * 2.39996;
            let r = sqrt(f32(i) / 11.0) * spread;
            behind = behind + textureSampleLevel(glass_scene, ao_samp, uv + vec2f(cos(a), sin(a)) * r * vec2f(size.y / size.x, 1.0), 0.0).rgb;
        }
        behind = behind / 12.0;
    }
    let f0 = pow((ior - 1.0) / (ior + 1.0), 2.0);
    let fres = f0 + (1.0 - f0) * pow(1.0 - max(dot(n, v), 0.0), 5.0);
    return behind * albedo * exp(-object.volume.xyz * depth) * ((1.0 - fres) * object.optics.x);
}

fn brdf(n: vec3f, v: vec3f, l: vec3f, albedo: vec3f, metallic: f32, roughness: f32) -> vec3f {
    let h = normalize(l + v);
    let ndl = max(dot(n, l), 0.0);
    let ndv = max(dot(n, v), 1e-4);
    let ndh = max(dot(n, h), 0.0);
    let vdh = max(dot(v, h), 0.0);
    let a = roughness * roughness;
    let a2 = a * a;
    let denom = ndh * ndh * (a2 - 1.0) + 1.0;
    let d = a2 / max(denom * denom, 1e-6);            // GGX, times pi
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let g = (ndl / (ndl * (1.0 - k) + k)) * (ndv / (ndv * (1.0 - k) + k));
    let f0 = mix(vec3f(0.04), albedo, metallic);
    let f = f0 + (vec3f(1.0) - f0) * pow(1.0 - vdh, 5.0);
    let spec = d * g * f / max(4.0 * ndv, 1e-4);      // the ndl of the denominator cancels with the one below
    let diffuse = albedo * (1.0 - metallic) * (vec3f(1.0) - f);
    return (diffuse * ndl + spec);
}

// Whether a cascade's map holds a point (a probe's capture takes the finest that does).
fn cascade_holds(c: i32, p: vec3f) -> bool {
    let sp = frame.cascade_vp[c] * vec4f(p, 1.0);
    let ndc = sp.xyz / sp.w;
    return abs(ndc.x) < 0.98 && abs(ndc.y) < 0.98 && ndc.z >= 0.0 && ndc.z <= 1.0;
}
// How lit a point is by the sun in one cascade (0 shadowed .. 1 lit), outside the map lit.
fn cascade_lit(c: i32, world_pos: vec3f, gn: vec3f, ndl: f32) -> f32 {
    let p = world_pos + gn * frame.cascade_texel[c] * 1.5;
    let sp = frame.cascade_vp[c] * vec4f(p, 1.0);
    let ndc = sp.xyz / sp.w;
    let suv = vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) { return 1.0; }
    let bias = frame.shadow.y * (1.0 + 2.0 * (1.0 - ndl));
    let receiver = ndc.z - bias;
    let texel = frame.cascade_texel[c];
    let search = clamp(frame.shadow_soft.x * frame.shadow_soft.z / texel, 1.0, 24.0);
    // Hard shadows, and a soft sun's in a cascade so coarse its widest penumbra is under a texel and
    // a half (far from the camera): a 3x3 comparison filter.
    if (frame.shadow_soft.x <= 0.0 || search < 1.5) {
        var lit = 0.0;
        for (var j = -1; j <= 1; j = j + 1) {
            for (var i = -1; i <= 1; i = i + 1) {
                lit = lit + textureSampleCompareLevel(shadow_map, shadow_samp, suv + vec2f(f32(i), f32(j)) * frame.shadow.x, c, receiver);
            }
        }
        return lit / 9.0;
    }
    // A sun of some size (docs/design/rendering.md, Soft shadows): the mean depth of what stands in
    // front of the point, looked for over the widest penumbra the sun's size allows, then a filter
    // as wide as the penumbra at that distance, so a shadow is sharp where it touches its caster
    // and soft far from it. Both on a spiral turned by the point, which TAA smooths; each step of
    // the spiral turns the last by the golden angle (a rotation, not a sine and cosine a tap).
    let dims = vec2i(textureDimensions(shadow_map));
    let spin = fract(sin(dot(world_pos, vec3f(12.9898, 78.233, 37.719))) * 43758.547) * 6.2831853;
    let first = vec2f(cos(spin), sin(spin));
    let golden = mat2x2f(-0.7373688, 0.6754903, -0.6754903, -0.7373688);
    var dir = first;
    var blockers = 0.0;
    var sum = 0.0;
    for (var i = 0; i < 16; i = i + 1) {
        let at = suv + dir * (sqrt((f32(i) + 0.5) / 16.0) * search * frame.shadow.x);
        dir = golden * dir;
        let q = clamp(vec2i(at * vec2f(dims)), vec2i(0), dims - vec2i(1));
        let d = textureLoad(shadow_map, q, c, 0);
        if (d < receiver) { blockers = blockers + 1.0; sum = sum + d; }
    }
    if (blockers < 0.5) { return 1.0; }
    let gap = (receiver - sum / blockers) * frame.cascade_depth[c];
    let radius = clamp(gap * frame.shadow_soft.x / texel, 1.0, 24.0);
    var lit = 0.0;
    dir = first;
    for (var i = 0; i < 16; i = i + 1) {
        let at = suv + dir * (sqrt((f32(i) + 0.5) / 16.0) * radius * frame.shadow.x);
        dir = golden * dir;
        lit = lit + textureSampleCompareLevel(shadow_map, shadow_samp, at, c, receiver);
    }
    return lit / 16.0;
}

// `cut`: whether this pipeline draws cut-outs. Only the cut-out pipelines are compiled with the
// discard, so the solid ones keep the GPU's early depth test and hidden-surface removal.
// 2D shadows (TileMap.shadows): whether the line from a point to a light, in the casting map's
// plane, crosses a solid cell between the two (the point's own cell and the light's left out, so a
// wall is lit on the face it shows the light). Cell by cell, Amanatides and Woo.
fn shadow_2d(p: vec3f, l: vec3f) -> f32 {
    let ts = frame.occluders.z;
    let a = vec2f((p.x - frame.occluders.x) / ts, (frame.occluders.y - p.y) / ts);
    let b = vec2f((l.x - frame.occluders.x) / ts, (frame.occluders.y - l.y) / ts);
    let size = vec2i(frame.occluder_size.xy);
    var cell = vec2i(floor(a));
    let last = vec2i(floor(b));
    let d = b - a;
    let stepv = vec2i(select(-1, 1, d.x > 0.0), select(-1, 1, d.y > 0.0));
    let inv = vec2f(select(1e30, abs(1.0 / d.x), d.x != 0.0), select(1e30, abs(1.0 / d.y), d.y != 0.0));
    var t = vec2f(select(1e30, select(a.x - floor(a.x), floor(a.x) + 1.0 - a.x, d.x > 0.0) * inv.x, d.x != 0.0),
                  select(1e30, select(a.y - floor(a.y), floor(a.y) + 1.0 - a.y, d.y > 0.0) * inv.y, d.y != 0.0));
    for (var i = 0; i < 96; i = i + 1) {
        if (cell.x == last.x && cell.y == last.y) { return 1.0; }
        if (t.x < t.y) { cell.x = cell.x + stepv.x; t.x = t.x + inv.x; } else { cell.y = cell.y + stepv.y; t.y = t.y + inv.y; }
        if (cell.x == last.x && cell.y == last.y) { return 1.0; }
        if (cell.x >= 0 && cell.y >= 0 && cell.x < size.x && cell.y < size.y && textureLoad(occ_tex, cell, 0).r > 0.5) { return 0.0; }
    }
    return 1.0;
}

fn shade(in: VsOut, cut: bool) -> vec4f {
    let object = objects[in.instance];
    // Derivatives and samples first: both must stay in uniform control flow.
    let dp1 = dpdx(in.world_pos);
    let dp2 = dpdy(in.world_pos);
    let uv = surface_uv(object, in);
    let world_mapped = (object.id.y & 8u) != 0u;
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    var base = textureSample(base_tex, base_samp, uv) * in.color;
    let mr = textureSample(mr_tex, base_samp, uv);
    let nm = textureSample(normal_tex, base_samp, uv).xyz * 2.0 - 1.0;
    let em = textureSample(emissive_tex, base_samp, uv).rgb;
    if (object.terrain.x > 0.5) { base = terrain_base(object, in, duv1, duv2); }
    // A cut-out: texels under the cutoff are not drawn (nor picked, the id goes with the color).
    if (cut && object.emissive.w > 0.0 && base.a < object.emissive.w) { discard; }
    // Unlit (MeshRenderer.unlit, KHR_materials_unlit): the colour as it is.
    if ((object.id.y & 4u) != 0u) { return vec4f(base.rgb + object.emissive.rgb * em, base.a); }
    var n = normalize(in.normal);
    if (object.pbr.w > 0.5) {
        let bent = vec3f(nm.xy * object.pbr.z, nm.z);
        n = select(mapped_normal(in, n, dp1, dp2, duv1, duv2, bent), perturb_normal(n, dp1, dp2, duv1, duv2, bent), world_mapped);
    }
    // Brushed metal's direction: the uv's u across the surface, turned by its rotation.
    var aniso_t = vec3f(0.0);
    if (object.aniso.x > 0.0) {
        let across = select(-(cross(dp2, n) * duv1.x + cross(n, dp1) * duv2.x), in.tangent.xyz, dot(in.tangent.xyz, in.tangent.xyz) > 0.25);
        var t = across - n * dot(n, across);
        if (dot(t, t) < 1e-12) { t = cross(n, select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(n.y) > 0.9)); }
        t = normalize(t);
        aniso_t = normalize(t * object.aniso.y + cross(n, t) * object.aniso.z);
    }
    let v = normalize(frame.camera_pos.xyz - in.world_pos);
    let paint = paint_decals(in.world_pos, n, dp1, dp2, Painted(base.rgb, clamp(object.pbr.y * mr.g, 0.04, 1.0), clamp(object.pbr.x * mr.b, 0.0, 1.0), vec3f(0.0), n));
    n = paint.normal;
    let metallic = paint.metallic;
    let roughness = paint.roughness;
    // Glass passes light instead of scattering it: its diffuse part gives way to what it transmits.
    let albedo = paint.albedo * (1.0 - clamp(object.optics.x, 0.0, 1.0));
    let gn = normalize(in.normal);
    // The reflection's strength and tint (KHR_materials_specular) for the sky's light too, and for
    // brushed metal a normal bent across the grain, so the sky's reflection stretches with it.
    let f0 = mix(min(vec3f(0.04) * spec_tint(object), vec3f(1.0)) * spec_level(object), paint.albedo, metallic);
    let spec_strength = mix(spec_level(object), 1.0, metallic);
    var rn = n;
    if (object.aniso.x > 0.0) {
        let grain = cross(n, aniso_t);
        let bend = pow(1.0 - object.aniso.x * (1.0 - sqrt(roughness)), 4.0);
        rn = normalize(mix(cross(cross(grain, v), grain), n, bend));
    }
    var color = frame.ambient.rgb * mix(albedo, f0, metallic);
    // Inside a reflection probe's box, what the probe saw takes the place of the sky's light: its
    // reflection, and its diffuse light from all around (a room lit by its lamps and walls, not the sky).
    let probe = probe_specular(in.world_pos, n, v, roughness);
    let probe_d = probe_diffuse(in.world_pos, n);
    // Inside an irradiance volume its probes give the diffuse light, over the sky's and a probe's.
    let grid_d = grid_diffuse(in.world_pos, n);
    if (frame.env.x > 0.5) {
        // The sky's light instead of the flat ambient: diffuse from its harmonics, specular from the
        // prefiltered level matching the roughness, in the mirror direction.
        let ndv = max(dot(n, v), 1e-4);
        let spec = textureSampleLevel(env_tex, env_samp, env_uv(reflect(-v, rn)), roughness * frame.env.w).rgb * frame.env.z;
        let diffuse = mix(mix(sh_irradiance(n) * frame.env.y, probe_d.rgb, probe_d.w), grid_d.rgb, grid_d.w);
        color = albedo * (1.0 - metallic) * diffuse + mix(spec, probe.rgb, probe.w) * env_brdf(f0, roughness, ndv) * spec_strength;
    } else {
        if (probe.w > 0.0) {
            let ndv = max(dot(n, v), 1e-4);
            color = mix(color, probe_d.rgb * albedo * (1.0 - metallic) + probe.rgb * env_brdf(f0, roughness, ndv) * spec_strength, probe.w);
        }
        if (grid_d.w > 0.0) {
            // The diffuse part so far (the flat ambient's, or the probe's over it) given way to the volume's.
            let had = mix(frame.ambient.rgb, probe_d.rgb, max(probe.w, 0.0)) * albedo * (1.0 - metallic);
            color = color + (grid_d.rgb * albedo * (1.0 - metallic) - had) * grid_d.w;
        }
    }
    // A clear coat reflects the sky over the rest, by its own Fresnel on the geometric normal.
    if (object.optics.z > 0.0 && frame.env.x > 0.5) {
        let fc = (0.04 + 0.96 * pow(1.0 - max(dot(gn, v), 0.0), 5.0)) * object.optics.z;
        let cspec = textureSampleLevel(env_tex, env_samp, env_uv(reflect(-v, gn)), clamp(object.optics.w, 0.03, 1.0) * frame.env.w).rgb * frame.env.z;
        color = color * (1.0 - fc) + cspec * fc;
    }
    // Ambient occlusion darkens only this light from all around, not the lights'.
    if (frame.ao.x > 0.5) {
        color = color * textureSampleLevel(ao_tex, ao_samp, in.clip.xy * frame.ao.yz, 0.0).r;
    }
    // Directional light.
    let l = normalize(-frame.sun_dir.xyz);
    let ndl = max(dot(n, l), 0.0);
    // Shadow: the cascade covering this depth (blended into the next over the last tenth of it),
    // looked up a texel out along the surface's normal, with a 3x3 comparison filter.
    var shadow = 1.0;
    if (frame.shadow.w > 0.5 && frame.probe_info.z > 0.5) {
        // A probe's capture: the camera's cascades, the finest that holds the point (beyond them, lit).
        let count = i32(frame.camera_fwd.w);
        for (var c = 0; c < count; c = c + 1) {
            if (cascade_holds(c, in.world_pos)) {
                shadow = mix(1.0 - frame.shadow.z, 1.0, cascade_lit(c, in.world_pos, normalize(in.normal), ndl));
                break;
            }
        }
    } else if (frame.shadow.w > 0.5) {
        let depth = dot(in.world_pos - frame.camera_pos.xyz, frame.camera_fwd.xyz);
        let count = i32(frame.camera_fwd.w);
        var c = 0;
        while (c < count && depth > frame.cascade_far[c]) { c = c + 1; }
        if (c < count) {
            let gn = normalize(in.normal);
            var lit = cascade_lit(c, in.world_pos, gn, ndl);
            let start = select(0.0, frame.cascade_far[max(c - 1, 0)], c > 0);
            let band = (frame.cascade_far[c] - start) * 0.1;
            let into = frame.cascade_far[c] - depth;
            if (c + 1 < count && into < band) {
                lit = mix(cascade_lit(c + 1, in.world_pos, gn, ndl), lit, into / band);
            }
            shadow = mix(1.0 - frame.shadow.z, 1.0, lit);
        }
    }
    if (frame.shadow_soft.y > 0.5) {
        // Contact shadows (the ambient occlusion pass's second channel) where the maps miss them.
        let contact = textureSampleLevel(ao_tex, ao_samp, in.clip.xy * frame.ao.yz, 0.0).g;
        shadow = min(shadow, mix(1.0 - frame.shadow.z, 1.0, contact));
    }
    // A lit sprite or tile map takes the sun only when the scene has one: the key light a scene
    // without lights gets so its meshes show is not for a dungeon lit by its torches.
    let own_sun = select(1.0, 0.0, (object.id.y & 1u) != 0u && frame.sky.w < 0.5);
    color += frame.sun_color.rgb * surface_light(object, gn, n, v, l, albedo, metallic, roughness, aniso_t) * shadow * cloud_shade(in.world_pos, l) * caustics_at(in.world_pos, l) * own_sun;
    // Point and spot lights: the ones the pixel's cluster lists.
    if (frame.clusters.w > 0u) {
        let at = (in.clip.xy - frame.viewport.xy) / frame.viewport.zw;
        let cx = min(u32(max(at.x, 0.0) * f32(frame.clusters.x)), frame.clusters.x - 1u);
        let cy = min(u32(max(at.y, 0.0) * f32(frame.clusters.y)), frame.clusters.y - 1u);
        let depth = dot(in.world_pos - frame.camera_pos.xyz, frame.camera_fwd.xyz);
        let slice = (log(max(depth, frame.cluster_z.x)) - frame.cluster_z.w) * frame.cluster_z.z;
        let cz = min(u32(max(slice, 0.0)), frame.clusters.z - 1u);
        let cluster = (cz * frame.clusters.y + cy) * frame.clusters.x + cx;
        let first = cluster_data[cluster * 2u];
        let count = cluster_data[cluster * 2u + 1u];
        for (var k = 0u; k < count; k = k + 1u) {
            let li = local_lights[cluster_data[first + k]];
            let to_light = li.pos_range.xyz - in.world_pos;
            let dist = length(to_light);
            let range = li.pos_range.w;
            let att = clamp(1.0 - (dist * dist) / (range * range), 0.0, 1.0);
            let pl = to_light / max(dist, 0.0001);
            var cone = 1.0;
            if (li.color_kind.w > 1.5) {
                cone = smoothstep(li.dir_cos.w, li.cone.x, dot(-pl, li.dir_cos.xyz));
            }
            var lit = 1.0;
            let first_face = i32(li.cone.y);
            if (first_face >= 0 && att * cone > 0.0 && frame.probe_info.z < 0.5) {
                var face = first_face;
                if (li.color_kind.w < 1.5) {
                    // A point light: the cube face the direction from the light falls in (+X -X +Y -Y +Z -Z).
                    let d = -pl;
                    let a = abs(d);
                    if (a.x >= a.y && a.x >= a.z) { face = first_face + select(1, 0, d.x > 0.0); }
                    else if (a.y >= a.z) { face = first_face + select(3, 2, d.y > 0.0); }
                    else { face = first_face + select(5, 4, d.z > 0.0); }
                }
                lit = face_lit(face, in.world_pos, normalize(in.normal), pl, li.cone.z * dist);
            }
            var blocked = 1.0;
            if (frame.occluders.w > 0.5 && (object.id.y & 1u) != 0u && att * cone > 0.0) { blocked = shadow_2d(in.world_pos, li.pos_range.xyz); }
            color += li.color_kind.rgb * surface_light(object, gn, n, v, pl, albedo, metallic, roughness, aniso_t) * (att * att * cone * lit * blocked);
        }
    }
    if (object.optics.x > 0.0) { color += transmitted(object, in.world_pos, n, v, paint.albedo, roughness); }
    color += object.emissive.rgb * em + paint.glow;
    return vec4f(color, base.a);
}

// One pass writes color and id together (no MSAA); with MSAA the color pass and the id pass are
// separate, since an integer id target cannot be multisampled and resolved.

@fragment fn fs(in: VsOut) -> FsOut {
    var out: FsOut;
    out.color = shade(in, false);
    out.id = in.id;
    return out;
}
@fragment fn fs_cut(in: VsOut) -> FsOut {
    var out: FsOut;
    out.color = shade(in, true);
    out.id = in.id;
    return out;
}
@fragment fn fs_color(in: VsOut) -> @location(0) vec4f {
    return shade(in, false);
}
@fragment fn fs_color_cut(in: VsOut) -> @location(0) vec4f {
    return shade(in, true);
}
// Order-independent transparency (weighted blended, McGuire and Bavoil): each translucent surface
// adds its premultiplied color into an accumulation target weighed by how near and how opaque it
// is, and multiplies a revealage target by what it lets through; a composite divides the one by
// its weight and lays it over the opaque scene by the other. No sorting, so interleaved surfaces
// blend alike whichever way round they are drawn.
struct OitOut {
    @location(0) accum: vec4f,
    @location(1) reveal: f32,
};
@fragment fn fs_oit(in: VsOut) -> OitOut {
    let c = shade(in, true);
    let a = clamp(c.a, 0.0, 1.0);
    let z = abs(dot(in.world_pos - frame.camera_pos.xyz, frame.camera_fwd.xyz));
    let w = a * clamp(10.0 / (1e-5 + pow(z / 5.0, 2.0) + pow(z / 200.0, 6.0)), 1e-2, 3e3);
    var out: OitOut;
    out.accum = vec4f(c.rgb * a, a) * w;
    out.reveal = a;
    return out;
}

// The id pass writes the entity and the pixel's motion since last frame, in viewport units (y down).
struct IdOut {
    @location(0) id: u32,
    @location(1) velocity: vec2f,
    @location(2) surface: vec4f,   // the normal (octahedral), roughness, metallic
    @location(3) albedo: vec4f,
};
fn oct_encode(n: vec3f) -> vec2f {
    var p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z < 0.0) { p = (1.0 - abs(p.yx)) * select(vec2f(-1.0), vec2f(1.0), p >= vec2f(0.0)); }
    return p;
}
fn oct_decode(e: vec2f) -> vec3f {
    var n = vec3f(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) { n = vec3f((1.0 - abs(n.yx)) * select(vec2f(-1.0), vec2f(1.0), n.xy >= vec2f(0.0)), n.z); }
    return normalize(n);
}
fn motion(in: VsOut) -> vec2f {
    return (in.cur.xy / in.cur.w - in.prev.xy / in.prev.w) * vec2f(0.5, -0.5);
}
@fragment fn fs_id(in: VsOut) -> IdOut {
    return id_surface(in, false);
}
@fragment fn fs_id_cut(in: VsOut) -> IdOut {
    return id_surface(in, true);
}
fn id_surface(in: VsOut, cut: bool) -> IdOut {
    let object = objects[in.instance];
    let dp1 = dpdx(in.world_pos);
    let dp2 = dpdy(in.world_pos);
    let uv = surface_uv(object, in);
    let world_mapped = (object.id.y & 8u) != 0u;
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    var base = textureSample(base_tex, base_samp, uv) * in.color;
    let mr = textureSample(mr_tex, base_samp, uv);
    let nm = textureSample(normal_tex, base_samp, uv).xyz * 2.0 - 1.0;
    if (object.terrain.x > 0.5) { base = terrain_base(object, in, duv1, duv2); }
    if (cut && object.emissive.w > 0.0 && base.a < object.emissive.w) { discard; }
    var n = normalize(in.normal);
    if (object.pbr.w > 0.5) {
        let bent = vec3f(nm.xy * object.pbr.z, nm.z);
        n = select(mapped_normal(in, n, dp1, dp2, duv1, duv2, bent), perturb_normal(n, dp1, dp2, duv1, duv2, bent), world_mapped);
    }
    let paint = paint_decals(in.world_pos, n, dp1, dp2, Painted(base.rgb, clamp(object.pbr.y * mr.g, 0.04, 1.0), clamp(object.pbr.x * mr.b, 0.0, 1.0), vec3f(0.0), n));
    n = paint.normal;
    var out: IdOut;
    out.id = in.id;
    out.velocity = motion(in);
    out.surface = vec4f(oct_encode(n), paint.roughness, paint.metallic);
    out.albedo = vec4f(paint.albedo, 1.0);
    return out;
}

// The sky behind everything: a full-screen triangle, the panorama in the view direction, and the
// sun's disc on a procedural sky. Drawn first in the scene pass; the id stays 0 (background).
struct SkyOut {
    @builtin(position) clip: vec4f,
    @location(0) ndc: vec2f,
};
@vertex fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    var out: SkyOut;
    out.clip = vec4f(x, y, 1.0, 1.0);
    out.ndc = vec2f(x, y);
    return out;
}
// The atmosphere's clouds (docs/design/rendering.md, Atmosphere): a layer of drifting noise at
// their height, lit by the sun (brightest looking toward it) and the sky, thinning to the horizon.
fn cloud_hash(i: vec2i) -> f32 {
    var v = (u32(i.x) * 1597334677u) ^ (u32(i.y) * 3812015801u);
    v = v ^ (v >> 16u);
    v = v * 0x7feb352du;
    v = v ^ (v >> 15u);
    v = v * 0x846ca68bu;
    v = v ^ (v >> 16u);
    return f32(v) / 4294967295.0;
}
fn cloud_noise(p: vec2f) -> f32 {
    let i = vec2i(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(cloud_hash(i), cloud_hash(i + vec2i(1, 0)), u.x), mix(cloud_hash(i + vec2i(0, 1)), cloud_hash(i + vec2i(1, 1)), u.x), u.y);
}
// Caustics (docs/design/water.md): under a Water body the sunlight on what lies below is gathered
// into a moving net of bright lines, as a wavy surface focuses it. The net is a warped grid of
// thin lines (where a warped sine crosses zero), two of them drifting apart, projected along the
// sun's way down through the water and softened with depth; it multiplies the sun's light there.
fn caustic_net(p: vec2f, t: f32) -> f32 {
    var q = p;
    var lines = 0.0;
    for (var i = 0; i < 3; i = i + 1) {
        let fi = f32(i);
        q = q + 0.55 * vec2f(sin(q.y * 1.31 + t * 0.9 + fi * 1.7), sin(q.x * 1.17 - t * 0.7 + fi * 2.3));
        let cell = abs(sin(q.x) * sin(q.y * 1.1));
        lines = lines + pow(1.0 - clamp(cell, 0.0, 1.0), 6.0);
    }
    return lines / 3.0;
}
fn caustics_at(p: vec3f, l: vec3f) -> f32 {
    let n = u32(frame.water_info.x);
    for (var i = 0u; i < n; i = i + 1u) {
        let a = frame.waters[i * 2u];
        let b = frame.waters[i * 2u + 1u];
        let depth = a.y - p.y;
        if (depth <= 0.0 || depth > b.y || abs(p.x - a.x) > a.w || abs(p.z - a.z) > b.x || b.z <= 0.0) { continue; }
        // Where the light that reaches p came through the surface, a little slanted by the refraction.
        let s = p.xz + l.xz / max(l.y, 0.2) * depth * 0.75;
        let t = frame.water_info.y;
        let net = 0.6 * caustic_net(s * 0.9, t) + 0.4 * caustic_net(s * 1.3 + vec2f(3.1, 1.7), t * 1.3);
        // Sharp in the shallows, washed out in the deep.
        let focus = exp(-depth * 0.35);
        return 1.0 + b.z * focus * (2.4 * net - 0.35);
    }
    return 1.0;
}
// How thick the clouds are where the layer (cloud_height above the camera) is over world x, z.
fn cloud_density(xz: vec2f) -> f32 {
    var n = 0.0;
    var a = 0.5;
    var q = (xz + frame.cloud_drift.xy) * frame.clouds.z;
    for (var k = 0; k < 5; k = k + 1) {
        n = n + a * cloud_noise(q);
        q = q * 2.03 + vec2f(1.7, 9.2);
        a = a * 0.5;
    }
    let edge = 0.75 - 0.5 * frame.clouds.x;
    return smoothstep(edge, edge + 0.2, n);
}
// The sun's light the clouds let through to a point: the layer where the way to the sun crosses it
// (a cloud's shadow on the ground, drifting with it).
fn cloud_shade(p: vec3f, l: vec3f) -> f32 {
    if (frame.clouds.w < 0.5 || l.y <= 0.02) { return 1.0; }
    let rise = frame.camera_pos.y + frame.clouds.y - p.y;
    return 1.0 - 0.7 * cloud_density(p.xz + l.xz * (rise / l.y));
}
fn clouded(d: vec3f, c: vec3f) -> vec3f {
    if (frame.clouds.w < 0.5 || d.y <= 0.0) { return c; }
    let t = frame.clouds.y / max(d.y, 0.03);
    let density = cloud_density(frame.camera_pos.xz + d.xz * t);
    if (density <= 0.0) { return c; }
    let s = normalize(-frame.sun_dir.xyz);
    let forward = pow(max(dot(d, s), 0.0), 6.0);
    let sky_light = textureSampleLevel(env_tex, env_samp, env_uv(vec3f(0.0, 1.0, 0.0)), frame.env.w).rgb;
    let body = frame.sun_color.rgb * (0.25 * clamp(s.y * 3.0 + 0.3, 0.0, 1.0) + 0.6 * forward) * (1.0 - 0.5 * density) + sky_light * 1.4;
    return mix(c, body, density * smoothstep(0.0, 0.15, d.y) * 0.95);
}
// The stars (docs/design/rendering.md, A day): a grid over the directions, a few cells in a thousand
// holding one at a point of their own, each as bright and as warm or cool as its hash says, all
// twinkling a little; they sink into the haze toward the horizon.
fn star_hash(c: vec3i) -> u32 {
    var h = (u32(c.x) * 73856093u) ^ (u32(c.y) * 19349663u) ^ (u32(c.z) * 83492791u);
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    return h ^ (h >> 16u);
}
fn stars(d: vec3f) -> vec3f {
    let g = d * 220.0;
    let cell = floor(g);
    let h = star_hash(vec3i(cell));
    if ((h & 1023u) > 7u) { return vec3f(0.0); }
    let at = cell + 0.2 + 0.6 * vec3f(f32((h >> 10u) & 63u), f32((h >> 16u) & 63u), f32((h >> 22u) & 63u)) / 63.0;
    let r = f32((h >> 28u) & 15u) / 15.0;
    let bright = 0.5 + 3.0 * r * r * r;
    let twinkle = 0.75 + 0.25 * sin(frame.clock.x * (2.0 + 3.0 * r) + f32(h & 255u));
    let tint = mix(vec3f(1.0, 0.82, 0.66), vec3f(0.72, 0.84, 1.0), f32((h >> 3u) & 7u) / 7.0);
    return tint * bright * twinkle * smoothstep(0.38, 0.0, length(g - at)) * smoothstep(0.02, 0.25, d.y);
}
fn sky_color(ndc: vec2f) -> vec3f {
    let far = frame.inv_view_proj * vec4f(ndc, 1.0, 1.0);
    let near = frame.inv_view_proj * vec4f(ndc, 0.0, 1.0);
    let d = normalize(far.xyz / far.w - near.xyz / near.w);
    var c = textureSampleLevel(env_tex, env_samp, env_uv(d), 0.0).rgb;
    if (((frame.sky.x > 0.5 && frame.sky.x < 1.5) || frame.sky.x > 2.5) && frame.sky.w > 0.5) {
        let mu = dot(d, normalize(-frame.sun_dir.xyz));
        let edge = frame.sky.y;
        c = c + frame.sun_color.rgb * frame.sky.z * smoothstep(edge, edge + (1.0 - edge) * 0.2, mu);
    }
    if (frame.night.x > 0.0) { c = c + stars(d) * frame.night.x; }
    return clouded(d, c);
}
@fragment fn fs_sky(in: SkyOut) -> FsOut {
    var out: FsOut;
    out.color = vec4f(sky_color(in.ndc), 1.0);
    out.id = 0u;
    return out;
}
@fragment fn fs_sky_color(in: SkyOut) -> @location(0) vec4f {
    return vec4f(sky_color(in.ndc), 1.0);
}

// Sprites: no lighting, alpha blended, transparent pixels leave no id behind.
// GPU particles (docs/design/particles.md, On the GPU): drawn from the ring the compute pass keeps,
// six vertices a slot, a quad facing the camera (or flat in XY) sized and tinted by age, through the
// sprites' unlit fragment. A dead slot's quad is behind the near plane.
struct GpuParticle {
    pos: vec3f,
    age: f32,
    vel: vec3f,
    life: f32,
};
struct GpuEmitter {
    origin: vec4f,     // xyz where the emitter is at the window's end (0 for a local one), w the window's seconds
    prev: vec4f,       // xyz where it was at the window's start, w the seconds simulated before the window
    axis: vec4f,       // xyz the cone's axis in the world, w cos(spread)
    gravity: vec4f,    // xyz, w drag
    ranges: vec4f,     // speed least and most, life least and most
    ground: vec4f,     // x the floor in the particles' space, y bounce, z floor friction, w turbulence
    look: vec4f,       // x size at birth, y at death, z stretch, w one over the turbulence's scale
    color0: vec4f,     // linear, at birth
    color1: vec4f,     // at death
    place: vec4f,      // xyz added to every particle (a local emitter's position), w 1 for billboards
    counts: vec4u,     // x the ring's first new slot, y particles born in the window, z the ring's size, w seed
    extra: vec4u,      // x particles born before the window, y the entity's id
    depth_vp: mat4x4f, // the view the depth prepass was drawn from (last frame's, for the compute pass)
    depth_inv: mat4x4f,
    depth_info: vec4f, // x, y the prepass's size, z 1 when there is one, w 1 when the particles collide with it
    eye: vec4f,        // where that view's camera was
    area: vec4f,       // xyz the half extents of the box particles are born in, about the emitter (0: its point)
    turn: vec4f,       // the emitter's rotation, which turns that box
};
@group(3) @binding(2) var<storage, read> gpu_ring: array<GpuParticle>;
@group(3) @binding(3) var<uniform> gpu_emitter: GpuEmitter;
@group(3) @binding(4) var gpu_depth: texture_depth_2d;
@vertex fn vs_particle(@builtin(vertex_index) vid: u32, @builtin(instance_index) slot: u32) -> VsOut {
    var out: VsOut;
    let p = gpu_ring[slot];
    let e = gpu_emitter;
    if (p.life <= 0.0 || p.age >= p.life) {
        out.clip = vec4f(0.0, 0.0, -2.0, 1.0);
        return out;
    }
    let k = clamp(p.age / p.life, 0.0, 1.0);
    let size = mix(e.look.x, e.look.y, k);
    var corners = array<vec2f, 6>(vec2f(-0.5, -0.5), vec2f(0.5, -0.5), vec2f(0.5, 0.5), vec2f(-0.5, -0.5), vec2f(0.5, 0.5), vec2f(-0.5, 0.5));
    let c = corners[vid % 6u];
    var r = vec3f(1.0, 0.0, 0.0);
    var u = vec3f(0.0, 1.0, 0.0);
    if (e.place.w > 0.5) {
        // The camera's right and up in the world, from the inverse of its view and projection.
        let o4 = frame.inv_view_proj * vec4f(0.0, 0.0, 0.5, 1.0);
        let r4 = frame.inv_view_proj * vec4f(1.0, 0.0, 0.5, 1.0);
        let u4 = frame.inv_view_proj * vec4f(0.0, 1.0, 0.5, 1.0);
        let o = o4.xyz / o4.w;
        r = normalize(r4.xyz / r4.w - o);
        u = normalize(u4.xyz / u4.w - o);
    }
    var along = size;
    if (e.look.z > 0.0) {
        // Stretched along its motion as seen: in the camera's plane, or in XY.
        let f = cross(r, u);
        let seen = p.vel - f * dot(p.vel, f);
        let l = length(seen);
        if (l > 1e-4) {
            r = seen / l;
            u = cross(f, r);
            along = size + e.look.z * l;
        }
    }
    let world = vec4f(e.place.xyz + p.pos + r * (c.x * along) + u * (c.y * size), 1.0);
    out.clip = frame.view_proj * world;
    out.cur = frame.cur_view_proj * world;
    out.prev = frame.prev_view_proj * world;
    out.world_pos = world.xyz;
    out.normal = cross(r, u);
    out.uv = vec2f(c.x + 0.5, 0.5 - c.y);
    out.color = mix(e.color0, e.color1, k);
    out.id = e.extra.y;
    return out;
}

fn unlit(in: VsOut) -> vec4f {
    return textureSample(base_tex, base_samp, in.uv) * in.color;
}
@fragment fn fs_unlit(in: VsOut) -> FsOut {
    let base = unlit(in);
    if (base.a < 0.02) { discard; }
    var out: FsOut;
    out.color = base;
    out.id = in.id;
    return out;
}
@fragment fn fs_unlit_color(in: VsOut) -> @location(0) vec4f {
    let base = unlit(in);
    if (base.a < 0.02) { discard; }
    return base;
}
// GPU particles fade where they near what is behind them (by the depth prepass, when there is one),
// so a plume meets the ground softly instead of along a line.
fn particle_base(in: VsOut) -> vec4f {
    var base = unlit(in);
    if (gpu_emitter.depth_info.z > 0.5) {
        let z = textureLoad(gpu_depth, vec2i(in.clip.xy), 0);
        if (z < 1.0) {
            let uv = (in.clip.xy - frame.viewport.xy) / frame.viewport.zw;
            let w = frame.inv_view_proj * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
            let gap = dot(w.xyz / w.w - in.world_pos, frame.camera_fwd.xyz);
            base.a *= clamp(gap / max(gpu_emitter.look.x, 0.02), 0.0, 1.0);
        }
    }
    return base;
}
@fragment fn fs_particle(in: VsOut) -> FsOut {
    let base = particle_base(in);
    if (base.a < 0.02) { discard; }
    var out: FsOut;
    out.color = base;
    out.id = in.id;
    return out;
}
@fragment fn fs_particle_color(in: VsOut) -> @location(0) vec4f {
    let base = particle_base(in);
    if (base.a < 0.02) { discard; }
    return base;
}
// Rain and snow (docs/design/rendering.md, Weather): made in the vertex stage from each drop's
// number and the time, with no state kept. A drop's place is a hash in a box 36 by 22 by 36 units
// that its fall (slanted by the Wind) slides it through, wrapped about the camera: the box goes
// with the camera while each drop keeps falling where it is in the world. Rain is a thin streak
// along its fall, snow a round flake swaying as it drifts; both take the sky's light from above and
// some of the sun's, and fade up close and toward the box's edge. The first frame.weather.z
// instances are rain, the rest snow.
const WEATHER_BOX = vec3f(36.0, 22.0, 36.0);
fn drop_hash(i: u32) -> vec4f {
    var h = i * 747796405u + 2891336453u;
    var o = vec4f(0.0);
    for (var k = 0; k < 4; k = k + 1) {
        h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
        h = (h >> 22u) ^ h;
        o[k] = f32(h >> 8u) / 16777216.0;
        h = h + 2654435769u;
    }
    return o;
}
@vertex fn vs_weather(@builtin(vertex_index) vid: u32, @builtin(instance_index) i: u32) -> VsOut {
    var out: VsOut;
    let snow = f32(i) >= frame.weather.z;
    let h = drop_hash(i);
    let t = frame.clock.x;
    let wind = select(vec2f(0.0), frame.wind.xy * frame.wind.z, frame.wind.w > 0.5);
    var fall = vec3f(wind.x * 0.3, -8.5 - 3.0 * h.w, wind.y * 0.3);
    if (snow) { fall = vec3f(wind.x * 0.7, -1.0 - 0.6 * h.w, wind.y * 0.7); }
    let cam = frame.camera_pos.xyz;
    var p = h.xyz * WEATHER_BOX + fall * t;
    p = cam + (fract((p - cam) / WEATHER_BOX + 0.5) - 0.5) * WEATHER_BOX;
    if (snow) {
        p.x = p.x + sin(t * (0.7 + h.w) + h.x * 40.0) * 0.3;
        p.z = p.z + cos(t * (0.6 + h.y) + h.z * 40.0) * 0.3;
    }
    let to_cam = cam - p;
    let dist = length(to_cam);
    let f = to_cam / max(dist, 1e-3);
    var corners = array<vec2f, 6>(vec2f(-0.5, -0.5), vec2f(0.5, -0.5), vec2f(0.5, 0.5), vec2f(-0.5, -0.5), vec2f(0.5, 0.5), vec2f(-0.5, 0.5));
    let c = corners[vid % 6u];
    var along = normalize(fall);
    var across = cross(along, f);
    across = select(vec3f(1.0, 0.0, 0.0), normalize(across), dot(across, across) > 1e-8);
    var size = vec2f(0.014, length(fall) * 0.025);   // across, along: a streak of a fortieth of a second
    if (snow) {
        let o4 = frame.inv_view_proj * vec4f(0.0, 0.0, 0.5, 1.0);
        let r4 = frame.inv_view_proj * vec4f(1.0, 0.0, 0.5, 1.0);
        let u4 = frame.inv_view_proj * vec4f(0.0, 1.0, 0.5, 1.0);
        across = normalize(r4.xyz / r4.w - o4.xyz / o4.w);
        along = normalize(u4.xyz / u4.w - o4.xyz / o4.w);
        size = vec2f(0.045 + 0.03 * h.w);
    }
    let world = vec4f(p + across * (c.x * size.x) + along * (c.y * size.y), 1.0);
    out.clip = frame.view_proj * world;
    out.cur = frame.cur_view_proj * world;
    out.prev = frame.prev_view_proj * world;
    out.world_pos = world.xyz;
    out.normal = vec3f(select(0.0, 1.0, snow), 0.0, 0.0);
    out.uv = vec2f(c.x + 0.5, 0.5 - c.y);
    let fade = smoothstep(0.4, 1.6, dist) * (1.0 - smoothstep(WEATHER_BOX.x * 0.3, WEATHER_BOX.x * 0.5, dist));
    if (snow) {
        out.color = vec4f(frame.sun_color.rgb * 0.35, 0.9 * fade);
    } else {
        out.color = vec4f(frame.sun_color.rgb * 0.12, 0.36 * fade);
    }
    out.id = 0u;
    return out;
}
fn weather_base(in: VsOut) -> vec4f {
    if (open_sky(in.world_pos) < 0.5) { return vec4f(0.0); }   // under a roof, a tree, a bridge
    let snow = in.normal.x > 0.5;
    var sky = frame.ambient.rgb;
    if (frame.env.x > 0.5) { sky = sky + sh_irradiance(vec3f(0.0, 1.0, 0.0)) * max(frame.env.y, 0.5); }
    let q = in.uv * 2.0 - 1.0;
    var a = (1.0 - abs(q.x)) * (1.0 - q.y * q.y);
    if (snow) { a = 1.0 - smoothstep(0.35, 1.0, length(q)); }
    return vec4f(in.color.rgb + sky * select(0.8, 0.9, snow), in.color.a * a);
}
@fragment fn fs_weather(in: VsOut) -> FsOut {
    let base = weather_base(in);
    if (base.a < 0.01) { discard; }
    var out: FsOut;
    out.color = base;
    out.id = 0u;
    return out;
}
@fragment fn fs_weather_color(in: VsOut) -> @location(0) vec4f {
    let base = weather_base(in);
    if (base.a < 0.01) { discard; }
    return base;
}
@fragment fn fs_unlit_id(in: VsOut) -> IdOut {
    let base = unlit(in);
    if (base.a < 0.02) { discard; }
    var out: IdOut;
    out.id = in.id;
    out.velocity = motion(in);
    out.surface = vec4f(0.0, 0.0, 1.0, 0.0);   // rough: no reflection traced
    out.albedo = vec4f(base.rgb, 0.0);          // alpha 0: unlit, no bounced light added
    return out;
}

// Screen-space reflections (docs/design/rendering.md): from each glossy pixel the mirror ray is
// marched in screen space (depth interpolated as 1/w, every pixel starting at its own offset), a hit
// is where it passes just behind the depth, refined by bisection; the scene's color there takes the
// place of the sky's reflection, weighed by the same BRDF and faded toward the view's edges, the
// ray's end and rough surfaces.
struct Ssr { params: vec4f, more: vec4f };
@group(1) @binding(10) var<uniform> ssr: Ssr;
@group(1) @binding(11) var ssr_scene: texture_2d<f32>;
@group(1) @binding(12) var ssr_depth: texture_depth_2d;
@group(1) @binding(13) var ssr_surface: texture_2d<f32>;
@group(1) @binding(14) var ssr_albedo: texture_2d<f32>;
fn ssr_world(px: vec2i, d: f32) -> vec3f {
    let uv = (vec2f(px) + vec2f(0.5) - frame.viewport.xy) / frame.viewport.zw;
    let h = frame.inv_view_proj * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    return h.xyz / h.w;
}
fn ssr_depth_at(q: vec2i) -> f32 {
    let d = textureLoad(ssr_depth, q, 0);
    if (d >= 1.0) { return 1e9; }
    return dot(ssr_world(q, d) - frame.camera_pos.xyz, frame.camera_fwd.xyz);
}
fn ssr_px(c: vec4f) -> vec2f {
    return frame.viewport.xy + vec2f(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5) * frame.viewport.zw;
}
@fragment fn fs_ssr(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let px = vec2i(pos.xy);
    let center = textureLoad(ssr_scene, px, 0);
    let d = textureLoad(ssr_depth, px, 0);
    if (d >= 1.0) { return center; }
    let s = textureLoad(ssr_surface, px, 0);
    let roughness = s.z;
    if (roughness > ssr.params.y) { return center; }
    let p = ssr_world(px, d);
    let n = oct_decode(s.xy);
    let v = normalize(frame.camera_pos.xyz - p);
    let r = reflect(-v, n);
    let start = p + n * 0.02;
    var len = ssr.params.x;
    let c0 = frame.cur_view_proj * vec4f(start, 1.0);
    var c1 = frame.cur_view_proj * vec4f(start + r * len, 1.0);
    if (c1.w < 0.05) {
        // Toward the eye: the ray stops at the near plane.
        len = len * (c0.w - 0.05) / max(c0.w - c1.w, 1e-5);
        c1 = frame.cur_view_proj * vec4f(start + r * len, 1.0);
    }
    let s0 = ssr_px(c0);
    let s1 = ssr_px(c1);
    let k0 = 1.0 / c0.w;
    let k1 = 1.0 / c1.w;
    let steps = max(u32(ssr.params.z), 8u);
    let jitter = fract(52.9829189 * fract(dot(pos.xy, vec2f(0.06711056, 0.00583715))) + ssr.more.y);
    let lo_px = vec2i(frame.viewport.xy);
    let hi_px = vec2i(frame.viewport.xy + frame.viewport.zw);
    var prev_t = 0.0;
    var hit_t = -1.0;
    for (var i = 0u; i < steps; i = i + 1u) {
        let t = (f32(i) + jitter + 0.5) / f32(steps);
        let q = vec2i(mix(s0, s1, t));
        if (any(q < lo_px) || any(q >= hi_px)) { break; }
        let ray_depth = 1.0 / mix(k0, k1, t);
        let scene_depth = ssr_depth_at(q);
        if (ray_depth > scene_depth && ray_depth - scene_depth < ssr.params.w * (1.0 + 0.05 * ray_depth)) { hit_t = t; break; }
        prev_t = t;
    }
    if (hit_t < 0.0) { return center; }
    var lo = prev_t;
    var hi = hit_t;
    for (var b = 0; b < 5; b = b + 1) {
        let m = 0.5 * (lo + hi);
        if (1.0 / mix(k0, k1, m) > ssr_depth_at(vec2i(mix(s0, s1, m)))) { hi = m; } else { lo = m; }
    }
    let hp = mix(s0, s1, hi);
    let hq = vec2i(hp);
    let uv = (hp - frame.viewport.xy) / frame.viewport.zw;
    let edge = clamp(min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y)) * 10.0, 0.0, 1.0);
    let conf = edge * (1.0 - hi * hi) * (1.0 - smoothstep(ssr.params.y * 0.5, ssr.params.y, roughness)) * ssr.more.x;
    // The light arriving along the ray, spread a little on rough surfaces.
    var hit = textureLoad(ssr_scene, hq, 0).rgb;
    let spread = i32(roughness * 10.0);
    if (spread >= 1) {
        let dims = vec2i(textureDimensions(ssr_scene));
        hit = hit + textureLoad(ssr_scene, clamp(hq + vec2i(spread, 0), vec2i(0), dims - vec2i(1)), 0).rgb
                  + textureLoad(ssr_scene, clamp(hq - vec2i(spread, 0), vec2i(0), dims - vec2i(1)), 0).rgb
                  + textureLoad(ssr_scene, clamp(hq + vec2i(0, spread), vec2i(0), dims - vec2i(1)), 0).rgb
                  + textureLoad(ssr_scene, clamp(hq - vec2i(0, spread), vec2i(0), dims - vec2i(1)), 0).rgb;
        hit = hit / 5.0;
    }
    // In place of what the sky's reflection gave the pixel, through the same BRDF.
    let albedo = textureLoad(ssr_albedo, px, 0).rgb;
    let f0 = mix(vec3f(0.04), albedo, s.w);
    let brdf = env_brdf(f0, roughness, max(dot(n, v), 1e-4));
    var env = frame.ambient.rgb;
    if (frame.env.x > 0.5) { env = textureSampleLevel(env_tex, env_samp, env_uv(r), roughness * frame.env.w).rgb * frame.env.z; }
    let probe = probe_specular(p, n, v, roughness);
    env = mix(env, probe.rgb, probe.w);
    return vec4f(max(center.rgb + (hit - env) * brdf * conf, vec3f(0.0)), center.a);
}

// Screen-space global illumination (docs/design/rendering.md, Screen-space global illumination):
// each pixel of a half-size target sends a few rays over the hemisphere about its normal (cosine
// weighted, every pixel and frame its own) through screen space against the depth; where one meets
// a surface facing it, that surface's lit color is light arriving. The frames are blended where a
// pixel's point was seen last frame at the same depth. A full-size pass then adds albedo times
// what arrived to the frame, the half-size result gathered by depth.
struct Gi { params: vec4f, more: vec4f };   // distance, intensity, steps, thickness; rays, jitter, history valid, blend
@group(1) @binding(20) var<uniform> gi: Gi;
@group(1) @binding(21) var gi_depth: texture_depth_2d;
@group(1) @binding(22) var gi_surface: texture_2d<f32>;
@group(1) @binding(23) var gi_scene: texture_2d<f32>;
@group(1) @binding(24) var gi_light: texture_2d<f32>;   // gathering: last frame's; adding: this frame's
@group(1) @binding(25) var gi_albedo: texture_2d<f32>;
// Independent numbers in [0, 1) for a pixel, a frame and a ray (a PCG hash of the three), so each
// pixel's rays cover the hemisphere over the frames blended.
fn gi_pcg(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}
fn gi_rand(px: vec2u, n: u32) -> f32 {
    return f32(gi_pcg(px.x ^ gi_pcg(px.y ^ gi_pcg(n))) >> 8u) / 16777216.0;
}
fn gi_depth_at(q: vec2i) -> f32 {
    let d = textureLoad(gi_depth, q, 0);
    if (d >= 1.0) { return 1e9; }
    return dot(ssr_world(q, d) - frame.camera_pos.xyz, frame.camera_fwd.xyz);
}
@fragment fn fs_gi(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let dims = vec2i(textureDimensions(gi_depth));
    let full = min(vec2i(pos.xy * 2.0), dims - vec2i(1));
    let d = textureLoad(gi_depth, full, 0);
    if (d >= 1.0) { return vec4f(0.0, 0.0, 0.0, -1.0); }
    let p = ssr_world(full, d);
    let w_here = (frame.cur_view_proj * vec4f(p, 1.0)).w;
    let n = oct_decode(textureLoad(gi_surface, full, 0).xy);
    let side = select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(n.y) > 0.9);
    let tx = normalize(cross(side, n));
    let ty = cross(n, tx);
    let start = p + n * 0.03;
    let rays = max(u32(gi.more.x), 1u);
    let steps = max(u32(gi.params.z), 4u);
    let lo_px = vec2i(frame.viewport.xy);
    let hi_px = vec2i(frame.viewport.xy + frame.viewport.zw);
    var sum = vec3f(0.0);
    let seed = vec2u(pos.xy);
    let frame_n = u32(gi.more.y) * 48u;
    for (var r = 0u; r < rays; r = r + 1u) {
        let u1 = gi_rand(seed, frame_n + r * 3u);
        let u2 = gi_rand(seed, frame_n + r * 3u + 1u);
        let phi = 6.2831853 * u1;
        let rad = sqrt(u2);
        let dir = normalize(tx * (cos(phi) * rad) + ty * (sin(phi) * rad) + n * sqrt(max(1.0 - u2, 0.0)));
        var len = gi.params.x;
        let c0 = frame.cur_view_proj * vec4f(start, 1.0);
        var c1 = frame.cur_view_proj * vec4f(start + dir * len, 1.0);
        if (c1.w < 0.05) {
            len = len * (c0.w - 0.05) / max(c0.w - c1.w, 1e-5);
            c1 = frame.cur_view_proj * vec4f(start + dir * len, 1.0);
        }
        let s0 = ssr_px(c0);
        let s1 = ssr_px(c1);
        let k0 = 1.0 / c0.w;
        let k1 = 1.0 / c1.w;
        let jitter = gi_rand(seed, frame_n + r * 3u + 2u);
        for (var i = 0u; i < steps; i = i + 1u) {
            // Denser near the point, where most of the light that reaches it comes from.
            var t = (f32(i) + jitter) / f32(steps);
            t = t * t;
            let q = vec2i(mix(s0, s1, t));
            if (any(q < lo_px) || any(q >= hi_px)) { break; }
            if (all(q == full)) { continue; }
            let ray_depth = 1.0 / mix(k0, k1, t);
            let scene_depth = gi_depth_at(q);
            if (ray_depth > scene_depth && ray_depth - scene_depth < gi.params.w * (1.0 + 0.05 * ray_depth)) {
                let hn = oct_decode(textureLoad(gi_surface, q, 0).xy);
                if (dot(hn, dir) < 0.0) {
                    let uv = (vec2f(q) - frame.viewport.xy) / frame.viewport.zw;
                    let edge = clamp(min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y)) * 10.0, 0.0, 1.0);
                    sum = sum + textureLoad(gi_scene, q, 0).rgb * edge;
                }
                break;
            }
        }
    }
    var result = vec4f(sum / f32(rays), w_here);
    if (gi.more.z > 0.5) {
        // Last frame's at this point, if it saw the same surface there (its depth then as it is now).
        let prev = frame.prev_view_proj * vec4f(p, 1.0);
        let pn = prev.xy / prev.w;
        let puv = vec2f(pn.x * 0.5 + 0.5, 0.5 - pn.y * 0.5);
        if (prev.w > 0.0 && all(puv >= vec2f(0.0)) && all(puv < vec2f(1.0))) {
            let ldims = vec2i(textureDimensions(gi_light));
            let lq = clamp(vec2i((frame.viewport.xy + puv * frame.viewport.zw) * 0.5), vec2i(0), ldims - vec2i(1));
            let past = textureLoad(gi_light, lq, 0);
            if (past.a > 0.0 && abs(past.a - prev.w) < 0.05 * prev.w + 0.02) {
                result = vec4f(mix(past.rgb, result.rgb, gi.more.w), w_here);
            }
        }
    }
    return result;
}
@fragment fn fs_gi_add(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let px = vec2i(pos.xy);
    let d = textureLoad(gi_depth, px, 0);
    if (d >= 1.0) { return vec4f(0.0); }
    let albedo = textureLoad(gi_albedo, px, 0);
    if (albedo.a < 0.5) { return vec4f(0.0); }   // unlit: nothing falls on it
    let p = ssr_world(px, d);
    let w_here = (frame.cur_view_proj * vec4f(p, 1.0)).w;
    // The half-size pixels around, each by its nearness and by how near its depth is to this one's.
    let ldims = vec2i(textureDimensions(gi_light));
    let hp = vec2f(px) * 0.5;
    let base = vec2i(floor(hp));
    var sum = vec3f(0.0);
    var weight = 0.0;
    // Nine taps two apart cover what twenty-five next to each other would, for a third of the reads.
    for (var y = -2; y <= 2; y = y + 2) {
        for (var x = -2; x <= 2; x = x + 2) {
            let q = clamp(base + vec2i(x, y), vec2i(0), ldims - vec2i(1));
            let l = textureLoad(gi_light, q, 0);
            if (l.a <= 0.0) { continue; }
            let off = vec2f(q) + vec2f(0.5) - hp;
            let w = exp(-dot(off, off) * 0.3) * exp(-abs(l.a - w_here) / (0.03 * w_here + 0.01));
            sum = sum + l.rgb * w;
            weight = weight + w;
        }
    }
    if (weight <= 1e-4) { return vec4f(0.0); }
    let metallic = textureLoad(gi_surface, px, 0).w;
    return vec4f(albedo.rgb * (sum / weight) * (1.0 - metallic) * gi.params.y, 0.0);
}

// Volumetric fog (docs/design/rendering.md, Volumetric light): each pixel of a half-size target
// marches its view ray through the fog to the depth the prepass left, gathering at every step the
// sun's light through its cascades (so what blocks the sun cuts a shaft) and the point and spot
// lights' through the clusters and their shadow faces, scattered toward the eye by the
// Henyey-Greenstein phase; it writes the light gathered and how much of what lies behind still shows.
struct Vol { medium: vec4f, albedo: vec4f, march: vec4f, target_size: vec4f, history: vec4f };
@group(1) @binding(8) var<uniform> vol: Vol;
@group(1) @binding(9) var vol_depth: texture_depth_2d;
@group(1) @binding(10) var vol_history: texture_2d<f32>;   // last frame's result (history.x: whether it is one)
@group(1) @binding(11) var vol_samp: sampler;
// Henyey-Greenstein, scaled so an even medium (g = 0) scatters 1: lit fog then matches a lit white
// surface, the engine's convention for light of intensity one.
fn phase_hg(cos_t: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / pow(max(1.0 + g2 - 2.0 * g * cos_t, 1e-4), 1.5);
}
fn sun_visible(p: vec3f) -> f32 {
    let count = i32(frame.camera_fwd.w);
    if (frame.shadow.w < 0.5 || count == 0) { return 1.0; }
    let depth = dot(p - frame.camera_pos.xyz, frame.camera_fwd.xyz);
    var c = 0;
    while (c < count && depth > frame.cascade_far[c]) { c = c + 1; }
    if (c >= count) { return 1.0; }
    let sp = frame.cascade_vp[c] * vec4f(p, 1.0);
    let ndc = sp.xyz / sp.w;
    let suv = vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0 || ndc.z > 1.0) { return 1.0; }
    return textureSampleCompareLevel(shadow_map, shadow_samp, suv, c, ndc.z - frame.shadow.y);
}
fn face_visible(face: i32, p: vec3f) -> f32 {
    let f = shadow_faces[face];
    let sp = f.view_proj * vec4f(p, 1.0);
    if (sp.w <= 0.0) { return 1.0; }
    let ndc = sp.xyz / sp.w;
    if (abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) { return 1.0; }
    return textureSampleCompareLevel(shadow_atlas, shadow_samp, f.rect.xy + vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * f.rect.z, ndc.z);
}
// The point and spot lights reaching a point of the fog, each scattered toward the eye. Every point
// of a pixel's march lies on its view ray, so the cluster column (cxy) is the pixel's; only the
// depth slice changes along it.
fn local_scatter(p: vec3f, dir: vec3f, g: f32, cxy: vec2u) -> vec3f {
    if (frame.clusters.w == 0u) { return vec3f(0.0); }
    let cx = cxy.x;
    let cy = cxy.y;
    let depth = dot(p - frame.camera_pos.xyz, frame.camera_fwd.xyz);
    if (depth <= 0.0) { return vec3f(0.0); }
    let slice = (log(max(depth, frame.cluster_z.x)) - frame.cluster_z.w) * frame.cluster_z.z;
    let cz = min(u32(max(slice, 0.0)), frame.clusters.z - 1u);
    let cluster = (cz * frame.clusters.y + cy) * frame.clusters.x + cx;
    let first = cluster_data[cluster * 2u];
    let count = cluster_data[cluster * 2u + 1u];
    var sum = vec3f(0.0);
    for (var k = 0u; k < count; k = k + 1u) {
        let li = local_lights[cluster_data[first + k]];
        let to_light = li.pos_range.xyz - p;
        let dist = length(to_light);
        let range = li.pos_range.w;
        let att = clamp(1.0 - (dist * dist) / (range * range), 0.0, 1.0);
        if (att <= 0.0) { continue; }
        let pl = to_light / max(dist, 0.0001);
        var cone = 1.0;
        if (li.color_kind.w > 1.5) { cone = smoothstep(li.dir_cos.w, li.cone.x, dot(-pl, li.dir_cos.xyz)); }
        if (cone <= 0.0) { continue; }
        var lit = 1.0;
        let first_face = i32(li.cone.y);
        if (first_face >= 0) {
            var face = first_face;
            if (li.color_kind.w < 1.5) {
                let d = -pl;
                let a = abs(d);
                if (a.x >= a.y && a.x >= a.z) { face = first_face + select(1, 0, d.x > 0.0); }
                else if (a.y >= a.z) { face = first_face + select(3, 2, d.y > 0.0); }
                else { face = first_face + select(5, 4, d.z > 0.0); }
            }
            lit = face_visible(face, p);
        }
        sum = sum + li.color_kind.rgb * (att * att * cone * lit * phase_hg(dot(pl, dir), g));
    }
    return sum;
}
@vertex fn vs_volume(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}
@fragment fn fs_volume(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    // The full-size pixel at the middle of this one's 2x2 block, and where it is in the scene's viewport.
    let dims = vec2i(textureDimensions(vol_depth));
    let full = min(vec2i(pos.xy * 2.0), dims - vec2i(1));
    let d = textureLoad(vol_depth, full, 0);
    let uv = (vec2f(full) + vec2f(1.0) - frame.viewport.xy) / frame.viewport.zw;
    let ndc = vec2f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let near_h = frame.inv_view_proj * vec4f(ndc, 0.0, 1.0);
    let far_h = frame.inv_view_proj * vec4f(ndc, select(d, 1.0, d >= 1.0), 1.0);
    let start = near_h.xyz / near_h.w;
    let ray = far_h.xyz / far_h.w - start;
    var len = length(ray);
    let dir = ray / max(len, 1e-5);
    if (d >= 1.0) { len = vol.march.y; }   // the sky: as far as the march goes
    len = min(len, vol.march.y);
    let steps = max(u32(vol.march.x), 1u);
    let dt = len / f32(steps);
    // Each pixel starts its steps at its own offset (interleaved gradient noise), so banding turns to
    // grain, and every frame at another (march.w), so the frames blended below sample between each other's steps.
    let jitter = fract(52.9829189 * fract(dot(pos.xy, vec2f(0.06711056, 0.00583715))) + vol.march.w);
    let g = vol.medium.w;
    let sun_phase = phase_hg(dot(frame.sun_dir.xyz, -dir), g);
    var ambient = frame.ambient.rgb;
    if (frame.env.x > 0.5) { ambient = ambient + sh_irradiance(vec3f(0.0, 1.0, 0.0)) * frame.env.y; }
    var trans = 1.0;
    var gathered = vec3f(0.0);
    let cxy = vec2u(min(u32(clamp(uv.x, 0.0, 1.0) * f32(frame.clusters.x)), max(frame.clusters.x, 1u) - 1u), min(u32(clamp(uv.y, 0.0, 1.0) * f32(frame.clusters.y)), max(frame.clusters.y, 1u) - 1u));
    for (var i = 0u; i < steps; i = i + 1u) {
        let t = (f32(i) + jitter) * dt;
        if (t < vol.march.z) { continue; }
        let p = start + dir * t;
        let sigma = vol.medium.x * exp(-vol.medium.z * (p.y - vol.medium.y));
        if (sigma <= 1e-6) { continue; }
        let light = ambient + frame.sun_color.rgb * (sun_phase * sun_visible(p)) + local_scatter(p, dir, g, cxy);
        let step_t = exp(-sigma * dt);
        gathered = gathered + trans * light * (1.0 - step_t);
        trans = trans * step_t;
        if (trans < 0.005) { break; }
    }
    // The most the fog may hide: what it gathered shrinks with what it may cover.
    let floor_t = 1.0 - vol.albedo.w;
    if (trans < floor_t && trans < 1.0) { gathered = gathered * (1.0 - floor_t) / (1.0 - trans); trans = floor_t; }
    var result = vec4f(gathered * vol.albedo.rgb, trans);
    if (vol.history.x > 0.5) {
        // Where this pixel's point was last frame, and what was gathered there: each frame adds its
        // share (history.y) to the blend, so a few steps a frame add up to many.
        let at = far_h.xyz / far_h.w;
        let prev = frame.prev_view_proj * vec4f(at, 1.0);
        let pn = prev.xy / prev.w;
        let puv = vec2f(pn.x * 0.5 + 0.5, 0.5 - pn.y * 0.5);
        if (prev.w > 0.0 && all(puv >= vec2f(0.0)) && all(puv <= vec2f(1.0))) {
            let tuv = (frame.viewport.xy + puv * frame.viewport.zw) / vec2f(dims);
            let past = textureSampleLevel(vol_history, vol_samp, tuv, 0.0);
            result = mix(past, result, vol.history.y);
        }
    }
    return result;
}

// Water (docs/design/water.md): each body a grid over its extent moved by the Gerstner waves of
// world::water_surface (the two must change together), drawn after the solid scene. What lies
// below shows through, bent by the waves and fading into the water's colour with the depth the
// view crosses; the sky, a probe and (through the surface target) screen-space reflections show at
// glancing angles; the sun glints; foam gathers where it is shallow and on sharp crests. From below,
// the surface shows the world above inside Snell's window and the water outside it, and a camera
// under the surface sees everything through the water.
struct WaterBody {
    center: vec4f,            // the rest level's centre, w: the time
    extent: vec4f,            // half size along x and z, grid cells along x and z
    color: vec4f,             // the deep colour, w: clarity
    dir_k: array<vec4f, 4>,   // per wave: direction x, z, wave number, angular speed
    amp: array<vec4f, 4>,     // per wave: amplitude, steepness, phase
    misc: vec4f,              // current x, z, ripples, foam
    more: vec4f,              // the waves' amplitudes summed, choppiness
    id: vec4u,                // the entity's id
};
@group(1) @binding(16) var<uniform> water: array<WaterBody, 8>;
@group(1) @binding(17) var water_scene: texture_2d<f32>;
@group(1) @binding(18) var water_depth: texture_depth_2d;
@group(1) @binding(19) var water_samp: sampler;
struct WaterOut {
    @builtin(position) clip: vec4f,
    @location(0) world_pos: vec3f,
    @location(1) rest: vec2f,
    @location(2) @interpolate(flat) body: u32,
};
fn water_phase(b: WaterBody, k: u32, rest: vec2f) -> f32 {
    let d = b.dir_k[k];
    let p = rest - b.misc.xy * b.center.w;
    return d.z * dot(d.xy, p) - d.w * b.center.w + b.amp[k].z;
}
fn water_offset(b: WaterBody, rest: vec2f) -> vec3f {
    var o = vec3f(0.0);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let theta = water_phase(b, k, rest);
        let a = b.amp[k];
        let d = b.dir_k[k];
        let qa = a.y * a.x;
        o = o + vec3f(qa * d.x * cos(theta), a.x * sin(theta), qa * d.y * cos(theta));
    }
    return o;
}
fn water_normal(b: WaterBody, rest: vec2f) -> vec3f {
    var n = vec3f(0.0, 1.0, 0.0);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let theta = water_phase(b, k, rest);
        let a = b.amp[k];
        let d = b.dir_k[k];
        let wa = d.z * a.x;
        n = n - vec3f(d.x * wa * cos(theta), a.y * wa * sin(theta), d.y * wa * cos(theta));
    }
    return normalize(n);
}
fn water_hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}
fn water_noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = water_hash(i);
    let b = water_hash(i + vec2f(1.0, 0.0));
    let c = water_hash(i + vec2f(0.0, 1.0));
    let d = water_hash(i + vec2f(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}
// The slope of the small ripples: two layers of noise drifting apart, carried by the current.
fn water_ripples(b: WaterBody, rest: vec2f) -> vec2f {
    let t = b.center.w;
    let p = rest - b.misc.xy * t;
    let q1 = p * 1.3 + vec2f(t * 0.31, t * 0.17);
    let q2 = p * 3.1 + vec2f(-t * 0.43, t * 0.29);
    let e = 0.05;
    let h1 = water_noise(q1);
    let h2 = water_noise(q2);
    let dx = (water_noise(q1 + vec2f(e, 0.0)) - h1) * 1.3 + (water_noise(q2 + vec2f(e, 0.0)) - h2) * 3.1 * 0.35;
    let dz = (water_noise(q1 + vec2f(0.0, e)) - h1) * 1.3 + (water_noise(q2 + vec2f(0.0, e)) - h2) * 3.1 * 0.35;
    return vec2f(dx, dz) / e * 0.06;
}
fn water_world(px: vec2f, d: f32) -> vec3f {
    let uv = (px - frame.viewport.xy) / frame.viewport.zw;
    let h = frame.inv_view_proj * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    return h.xyz / h.w;
}
// The sun on a point of the water, through the camera's cascades.
fn water_sun(p: vec3f, ndl: f32) -> f32 {
    if (frame.shadow.w < 0.5) { return 1.0; }
    let depth = dot(p - frame.camera_pos.xyz, frame.camera_fwd.xyz);
    let count = i32(frame.camera_fwd.w);
    var c = 0;
    while (c < count && depth > frame.cascade_far[c]) { c = c + 1; }
    if (c >= count) { return 1.0; }
    return mix(1.0 - frame.shadow.z, 1.0, cascade_lit(c, p, vec3f(0.0, 1.0, 0.0), ndl));
}
// The light the water scatters back toward the eye: its colour, lit by the sky (or a probe) and the sun.
fn water_scatter(b: WaterBody, p: vec3f) -> vec3f {
    var sky_light = frame.ambient.rgb;
    if (frame.env.x > 0.5) { sky_light = sh_irradiance(vec3f(0.0, 1.0, 0.0)) * frame.env.y; }
    let probe = probe_diffuse(p, vec3f(0.0, 1.0, 0.0));
    sky_light = mix(sky_light, probe.rgb, probe.w);
    let grid = grid_diffuse(p, vec3f(0.0, 1.0, 0.0));
    sky_light = mix(sky_light, grid.rgb, grid.w);
    let l = normalize(-frame.sun_dir.xyz);
    let sun = frame.sun_color.rgb * max(l.y, 0.0) * water_sun(p, max(l.y, 0.0));
    return b.color.rgb * (sky_light + sun * 0.5);
}
// What light that crossed `through` units of water keeps: the hue it takes on, then the colour.
fn water_absorb(b: WaterBody, seen: vec3f, through: f32, scatter: vec3f) -> vec3f {
    let tone = b.color.rgb / max(max(b.color.r, max(b.color.g, b.color.b)), 1e-3);
    let keep = exp(-3.0 * through / max(b.color.w, 0.01));
    return seen * keep * mix(tone, vec3f(1.0), keep) + scatter * (1.0 - keep);
}
@vertex fn vs_water(@builtin(vertex_index) vi: u32, @builtin(instance_index) body: u32) -> WaterOut {
    let b = water[body];
    let nx = u32(b.extent.z);
    let nz = u32(b.extent.w);
    let cell = vi / 6u;
    let corner = vi % 6u;
    // Two triangles a cell, counter-clockwise seen from above.
    var c = vec2u(0u, 0u);
    if (corner == 1u || corner == 4u) { c = vec2u(0u, 1u); }
    if (corner == 2u || corner == 3u) { c = vec2u(1u, 0u); }
    if (corner == 5u) { c = vec2u(1u, 1u); }
    let g = vec2u(cell % nx, cell / nx) + c;
    let f = vec2f(g) / vec2f(f32(nx), f32(nz));
    let rest = b.center.xz + mix(-b.extent.xy, b.extent.xy, f);
    var o = water_offset(b, rest);
    // The rim moves only up and down, so the water keeps meeting the shore and its neighbours.
    if (g.x == 0u || g.x == nx) { o.x = 0.0; }
    if (g.y == 0u || g.y == nz) { o.z = 0.0; }
    let world = vec3f(rest.x + o.x, b.center.y + o.y, rest.y + o.z);
    var out: WaterOut;
    out.clip = frame.view_proj * vec4f(world, 1.0);
    out.world_pos = world;
    out.rest = rest;
    out.body = body;
    return out;
}
struct WaterFsOut {
    @location(0) color: vec4f,
    @location(1) id: u32,
    @location(2) surface: vec4f,
};
@fragment fn fs_water(in: WaterOut, @builtin(front_facing) front: bool) -> WaterFsOut {
    let b = water[in.body];
    let px = in.clip.xy;
    let dims = vec2f(textureDimensions(water_scene));
    let dist = length(frame.camera_pos.xyz - in.world_pos);
    let rs = water_ripples(b, in.rest) * (b.misc.z / (1.0 + dist * 0.06));
    var n = normalize(water_normal(b, in.rest) - vec3f(rs.x, 0.0, rs.y));
    if (!front) { n = -n; }
    let v = normalize(frame.camera_pos.xyz - in.world_pos);
    let ndv = max(dot(n, v), 1e-4);
    // What lies behind the surface and how much water the view crosses to it; bent along the
    // waves' slope, less where the water is thin, and not onto anything in front of the water.
    let d0 = textureLoad(water_depth, vec2i(px), 0);
    var through = 1e4;
    if (d0 < 1.0) { through = length(water_world(px, d0) - in.world_pos); }
    var q = clamp(px + n.xz * (frame.viewport.w * 0.04) * clamp(through, 0.0, 1.0), vec2f(0.5), dims - vec2f(0.5));
    var d = textureLoad(water_depth, vec2i(q), 0);
    if (d < in.clip.z) { q = px; d = d0; }
    through = 1e4;
    if (d < 1.0) { through = length(water_world(q, d) - in.world_pos); }
    let behind = textureSampleLevel(water_scene, water_samp, q / dims, 0.0).rgb;
    let scatter = water_scatter(b, in.world_pos);
    var out: WaterFsOut;
    out.id = b.id.x;
    if (!front) {
        // From below: the world above inside Snell's window, the water's own light outside it,
        // both through the water between the eye and the surface.
        let window = smoothstep(0.62, 0.72, ndv);
        let above = behind * window + scatter * (1.0 - window);
        out.color = vec4f(water_absorb(b, above, dist, scatter), 1.0);
        out.surface = vec4f(oct_encode(n), 1.0, 0.0);
        return out;
    }
    let under = water_absorb(b, behind, through, scatter);
    // Reflected: the sky (or a probe) along the mirror direction; screen-space reflections, where
    // on, trace the same direction from the surface target and take its place.
    let rough = 0.04;
    let r = reflect(-v, n);
    var refl = frame.ambient.rgb;
    if (frame.env.x > 0.5) { refl = textureSampleLevel(env_tex, env_samp, env_uv(r), rough * frame.env.w).rgb * frame.env.z; }
    let probe = probe_specular(in.world_pos, n, v, rough);
    refl = mix(refl, probe.rgb, probe.w);
    let fresnel = env_brdf(vec3f(0.04), rough, ndv);
    let l = normalize(-frame.sun_dir.xyz);
    let glint = frame.sun_color.rgb * brdf(n, v, l, vec3f(0.0), 0.0, 0.12) * water_sun(in.world_pos, max(dot(n, l), 0.0));
    var color = under * (vec3f(1.0) - fresnel) + refl * fresnel + glint;
    // Foam: in the shallows (within `foam` of the ground below) and on the sharpest crests,
    // broken up by drifting noise.
    var foam = 0.0;
    if (b.misc.w > 0.0 && d0 < 1.0) {
        let below = in.world_pos.y - water_world(px, d0).y;
        let shore = clamp(1.0 - below / b.misc.w, 0.0, 1.0);
        foam = shore * shore;
    }
    let crest = (in.world_pos.y - b.center.y) / max(b.more.x, 1e-3);
    foam = max(foam, smoothstep(0.6, 1.0, crest) * b.more.y * 0.7);
    let t = b.center.w;
    let pattern = water_noise(in.rest * 2.1 + vec2f(t * 0.23, -t * 0.11)) * 0.6 + water_noise(in.rest * 6.3 - vec2f(t * 0.17, t * 0.29)) * 0.4;
    foam = smoothstep(0.3, 0.7, foam * (0.3 + pattern * 1.2));
    let foam_light = water_scatter(b, in.world_pos) / max(b.color.rgb, vec3f(1e-3)) * 0.85;
    color = mix(color, foam_light, foam);
    out.color = vec4f(color, 1.0);
    out.surface = vec4f(oct_encode(n), mix(rough, 1.0, foam), 0.0);
    return out;
}
// A camera under a body's surface: everything seen through the water between it and the eye
// (drawn before the surface, which covers what lies above it).
struct WaterUnderOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) body: u32,
};
@vertex fn vs_water_under(@builtin(vertex_index) i: u32, @builtin(instance_index) body: u32) -> WaterUnderOut {
    var out: WaterUnderOut;
    out.clip = vec4f(f32(i32(i & 1u) * 4 - 1), f32(i32(i >> 1u) * 4 - 1), 0.5, 1.0);
    out.body = body;
    return out;
}
@fragment fn fs_water_under(in: WaterUnderOut) -> @location(0) vec4f {
    let b = water[in.body];
    let pos = in.clip;
    let d = textureLoad(water_depth, vec2i(pos.xy), 0);
    var dist = 1e4;
    var p = frame.camera_pos.xyz;
    if (d < 1.0) {
        p = water_world(pos.xy, d);
        dist = length(p - frame.camera_pos.xyz);
    }
    let seen = textureLoad(water_scene, vec2i(pos.xy), 0).rgb;
    return vec4f(water_absorb(b, seen, dist, water_scatter(b, frame.camera_pos.xyz)), 1.0);
}
)WGSL";

constexpr const char* kBloomWgsl = R"WGSL(
struct U { texel: vec2f, dir: vec2f, threshold: f32, strength: f32, radius: f32, pad: f32 };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
struct VOut { @builtin(position) pos: vec4f, @location(0) uv: vec2f };
// One triangle over the whole target.
@vertex fn vs_screen(@builtin(vertex_index) i: u32) -> VOut {
    var out: VOut;
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    out.pos = vec4f(x, y, 0.0, 1.0);
    out.uv = vec2f((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}
// What is brighter than the threshold, by how much.
@fragment fn fs_bright(in: VOut) -> @location(0) vec4f {
    let c = textureSample(src, smp, in.uv).rgb;
    let lum = dot(c, vec3f(0.2126, 0.7152, 0.0722));
    let k = max(lum - u.threshold, 0.0) / max(lum, 1e-4);
    return vec4f(c * k, 1.0);
}
// A nine-tap gaussian along u.dir, u.radius texels apart.
@fragment fn fs_blur(in: VOut) -> @location(0) vec4f {
    var w = array<f32, 5>(0.2270270270, 0.1945945946, 0.1216216216, 0.0540540541, 0.0162162162);
    var acc = textureSample(src, smp, in.uv).rgb * w[0];
    for (var i = 1; i < 5; i++) {
        let o = u.dir * u.texel * (f32(i) * u.radius);
        acc += textureSample(src, smp, in.uv + o).rgb * w[i];
        acc += textureSample(src, smp, in.uv - o).rgb * w[i];
    }
    return vec4f(acc, 1.0);
}
// The glow, scaled, added onto the frame (the pipeline blends one plus one).
@fragment fn fs_add(in: VOut) -> @location(0) vec4f {
    return vec4f(textureSample(src, smp, in.uv).rgb * u.strength, 1.0);
}
)WGSL";

constexpr const char* kPostWgsl = R"WGSL(
struct Post { exposure: f32, op: u32, auto_on: u32, grade_on: u32, tint: vec4f, temperature: f32, contrast: f32, saturation: f32, vignette: f32, viewport: vec4f,
               inv_view_proj: mat4x4f, camera: vec4f, fog_color: vec4f, fog: vec4f, fog2: vec4f, lut: vec4f,
               cb: vec4f, cb0: vec4f, cb1: vec4f, cb2: vec4f,   // colour vision: on, then the matrix's rows (linear light)
               outline: vec4f, outline2: vec4f,   // the toon look's outlines: colour and width in pixels; opacity, the highlights' width and count
               highlight_ids: array<vec4u, 8>, highlight_colors: array<vec4f, 32> };   // entities outlined in a colour of their own
@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> metered: array<f32, 4>;   // [0] the exposure the meter settled on, in EV
@group(0) @binding(3) var depth: texture_depth_2d;                // the depth prepass (fog reads distances from it)
@group(0) @binding(4) var volume: texture_2d<f32>;                // volumetric fog at half size: light gathered, what still shows
@group(0) @binding(5) var volume_samp: sampler;
// The grade's look-up table: N slices of N by N side by side (red across a slice, green down, blue
// from slice to slice), read with the encoded color, the two nearest slices blended.
@group(0) @binding(6) var lut_tex: texture_2d<f32>;
@group(0) @binding(7) var lut_samp: sampler;
// The entity under each pixel (the id pass): outlines are drawn where it changes.
@group(0) @binding(8) var ids: texture_2d<u32>;
// Outlines (docs/design/rendering.md, Toon): how many of eight pixels `outline.w` away show another
// entity than this one (the background is one too), as a coverage that softens the line's edge.
fn outlined(pos: vec2f) -> f32 {
    let w = post.outline.w;
    if (w <= 0.0) { return 0.0; }
    let lo = vec2i(post.viewport.xy);
    let hi = vec2i(post.viewport.xy + post.viewport.zw) - vec2i(1);
    let p = vec2i(pos);
    let own = textureLoad(ids, p, 0).r;
    let r = max(w * 0.5, 1.0);
    var other = 0.0;
    for (var k = 0; k < 8; k = k + 1) {
        let a = f32(k) * 0.785398;
        let q = clamp(p + vec2i(round(vec2f(cos(a), sin(a)) * r)), lo, hi);
        if (textureLoad(ids, q, 0).r != own) { other = other + 1.0; }
    }
    return clamp(other / 3.0, 0.0, 1.0);
}
// Highlights (MeshRenderer.highlight): the entity's colour round it, on the pixels just outside it.
fn highlight_of(id: u32) -> i32 {
    let n = i32(post.outline2.z);
    for (var k = 0; k < n; k = k + 1) {
        if (post.highlight_ids[k / 4][k % 4] == id) { return k; }
    }
    return -1;
}
fn highlighted(pos: vec2f) -> vec4f {
    if (post.outline2.z < 0.5) { return vec4f(0.0); }
    let lo = vec2i(post.viewport.xy);
    let hi = vec2i(post.viewport.xy + post.viewport.zw) - vec2i(1);
    let p = vec2i(pos);
    let own = textureLoad(ids, p, 0).r;
    let r = max(post.outline2.y, 1.0);
    var best = vec4f(0.0);
    var hits = 0.0;
    for (var k = 0; k < 8; k = k + 1) {
        let a = f32(k) * 0.785398;
        let q = clamp(p + vec2i(round(vec2f(cos(a), sin(a)) * r)), lo, hi);
        let id = textureLoad(ids, q, 0).r;
        if (id == own) { continue; }
        let h = highlight_of(id);
        if (h >= 0) {
            best = post.highlight_colors[h];
            hits = hits + 1.0;
        }
    }
    return vec4f(best.rgb, best.a * clamp(hits / 2.0, 0.0, 1.0));
}
fn graded(e: vec3f) -> vec3f {
    let dims = textureDimensions(lut_tex);
    let n = f32(dims.y);
    if (dims.x != dims.y * dims.y || dims.y < 2u) { return e; }
    let b = clamp(e.b, 0.0, 1.0) * (n - 1.0);
    let b0 = floor(b);
    let b1 = min(b0 + 1.0, n - 1.0);
    let x = clamp(e.r, 0.0, 1.0) * (n - 1.0) + 0.5;
    let y = (clamp(e.g, 0.0, 1.0) * (n - 1.0) + 0.5) / n;
    let s0 = textureSampleLevel(lut_tex, lut_samp, vec2f((b0 * n + x) / (n * n), y), 0.0).rgb;
    let s1 = textureSampleLevel(lut_tex, lut_samp, vec2f((b1 * n + x) / (n * n), y), 0.0).rgb;
    return mix(s0, s1, b - b0);
}
// Exponential height fog along the ray from the camera to the point (fog: density, base height,
// falloff, start; fog2.x: the most it hides, fog2.y: on).
fn fogged(c: vec3f, pos: vec2f) -> vec3f {
    let d = textureLoad(depth, vec2i(pos.xy), 0);
    let uv = (pos.xy - post.viewport.xy) / max(post.viewport.zw, vec2f(1.0));
    let ndc = vec2f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = post.inv_view_proj * vec4f(ndc, select(d, 1.0, d >= 1.0), 1.0);
    let o = post.camera.xyz;
    var p = far.xyz / far.w;
    if (d >= 1.0) { p = o + normalize(p - o) * 1000.0; }   // the sky: a long way off
    let dist = length(p - o);
    let dy = p.y - o.y;
    let base = post.fog.x * exp(-post.fog.z * (o.y - post.fog.y));
    var amount = base * dist;
    if (abs(post.fog.z * dy) > 1e-4) { amount = base * dist * (1.0 - exp(-post.fog.z * dy)) / (post.fog.z * dy); }
    amount = amount * max(dist - post.fog.w, 0.0) / max(dist, 1e-4);
    let f = min(1.0 - exp(-max(amount, 0.0)), post.fog2.x);
    return mix(c, post.fog_color.rgb, f);
}
// A full-size pixel's distance from the camera, from the prepass depth (the sky: the far plane).
fn view_distance(px: vec2i) -> f32 {
    let d = textureLoad(depth, px, 0);
    let uv = (vec2f(px) + vec2f(0.5) - post.viewport.xy) / max(post.viewport.zw, vec2f(1.0));
    let w = post.inv_view_proj * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, select(d, 1.0, d >= 1.0), 1.0);
    return length(w.xyz / w.w - post.camera.xyz);
}
// The volumetric fog at a full-size pixel: the four half-size texels around it, each weighed by
// how near it is (bilinear) and by how close the depth it was marched to is to this pixel's, so fog
// marched past a slat's edge does not bleed onto the slat (a joint bilateral upsample).
fn volume_at(pos: vec2f) -> vec4f {
    let vdims = vec2i(textureDimensions(volume));
    let fdims = vec2i(textureDimensions(depth));
    let here = view_distance(vec2i(pos));
    let base = (pos - vec2f(1.0)) * 0.5;   // half-size texel i was marched from full-size pixel 2i + 1
    let i0 = vec2i(floor(base));
    let f = base - vec2f(i0);
    var sum = vec4f(0.0);
    var wsum = 0.0;
    for (var j = 0; j < 2; j = j + 1) {
        for (var i = 0; i < 2; i = i + 1) {
            let c = clamp(i0 + vec2i(i, j), vec2i(0), vdims - vec2i(1));
            let bw = select(1.0 - f.x, f.x, i == 1) * select(1.0 - f.y, f.y, j == 1);
            let there = view_distance(min(c * 2 + vec2i(1), fdims - vec2i(1)));
            let w = max(bw, 1e-3) / (1e-3 + abs(there - here) / max(here, 0.01));
            sum = sum + textureLoad(volume, c, 0) * w;
            wsum = wsum + w;
        }
    }
    return sum / max(wsum, 1e-6);
}
@vertex fn vs_screen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}
// Narkowicz's fit of the ACES filmic curve.
fn aces(c: vec3f) -> vec3f {
    return clamp((c * (2.51 * c + 0.03)) / (c * (2.43 * c + 0.59) + 0.14), vec3f(0.0), vec3f(1.0));
}
// AgX: into a log space through an inset matrix, a sigmoid (a polynomial fit), back out.
fn agx(c0: vec3f) -> vec3f {
    let inset = mat3x3f(vec3f(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
                        vec3f(0.0784335999999992, 0.878468636469772, 0.0784336),
                        vec3f(0.0792237451477643, 0.0791661274605434, 0.879142973793104));
    let outset = mat3x3f(vec3f(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
                         vec3f(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
                         vec3f(-0.0990297440797205, -0.0989611768448433, 1.15107367264116));
    let lo = -12.47393;
    let hi = 4.026069;
    var v = inset * max(c0, vec3f(1e-10));
    v = (clamp(log2(v), vec3f(lo), vec3f(hi)) - lo) / (hi - lo);
    let x2 = v * v;
    let x4 = x2 * x2;
    v = 15.5 * x4 * x2 - 40.14 * x4 * v + 31.96 * x4 - 6.868 * x2 * v + 0.4298 * x2 + 0.1191 * v - 0.00232;
    return pow(clamp(outset * v, vec3f(0.0), vec3f(1.0)), vec3f(2.2));
}
// Khronos PBR Neutral: colors under the shoulder are kept, only near white is compressed.
fn neutral(c0: vec3f) -> vec3f {
    let start = 0.76;
    let desat = 0.15;
    var c = c0;
    let x = min(c.r, min(c.g, c.b));
    c = c - select(0.04, x - 6.25 * x * x, x < 0.08);
    let peak = max(c.r, max(c.g, c.b));
    if (peak < start) { return c; }
    let d = 1.0 - start;
    let top = 1.0 - d * d / (peak + d - start);
    c = c * (top / peak);
    let g = 1.0 - 1.0 / (desat * (peak - top) + 1.0);
    return mix(c, vec3f(top), g);
}
fn encode(c: vec3f) -> vec3f {
    return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3f(0.0031308));
}
// The final pass: exposure, the operator, the grade, the sRGB encoding (contrast on the encoded values).
@fragment fn fs_post(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let s = textureLoad(hdr, vec2i(pos.xy), 0);
    var exposure = post.exposure;
    if (post.auto_on != 0u) { exposure = exposure * exp2(metered[0]); }
    var hdr_c = s.rgb;
    if (post.fog2.z > 0.5) {
        let v = volume_at(pos.xy);
        hdr_c = hdr_c * v.a + v.rgb;
    } else if (post.fog2.y > 0.5) {
        hdr_c = fogged(hdr_c, pos.xy);
    }
    var c = max(hdr_c * exposure, vec3f(0.0));
    switch post.op {
        case 1u: { c = aces(c); }
        case 2u: { c = agx(c); }
        case 3u: { c = neutral(c); }
        default: { }
    }
    if (post.grade_on != 0u) {
        c = c * vec3f(1.0 + 0.15 * post.temperature, 1.0, 1.0 - 0.15 * post.temperature);
        let lum = dot(c, vec3f(0.2126, 0.7152, 0.0722));
        c = max(mix(vec3f(lum), c, post.saturation), vec3f(0.0));
        c = c * post.tint.rgb;
        let uv = (pos.xy - post.viewport.xy) / max(post.viewport.zw, vec2f(1.0));
        let d = length((uv - 0.5) * 2.0);   // 0 at the centre, about 1.4 at a corner
        c = c * (1.0 - post.vignette * smoothstep(0.4, 1.3, d));   // black just inside the corners at 1
    }
    var e = encode(clamp(c, vec3f(0.0), vec3f(1.0)));
    if (post.grade_on != 0u) { e = clamp((e - 0.5) * post.contrast + 0.5, vec3f(0.0), vec3f(1.0)); }
    if (post.lut.x > 0.5) { e = mix(e, graded(e), post.lut.y); }
    e = mix(e, post.outline.rgb, outlined(pos.xy) * post.outline2.x);
    let lit = highlighted(pos.xy);
    e = mix(e, lit.rgb, clamp(lit.a, 0.0, 1.0));
    if (post.cb.x > 0.5) {
        // Colour vision (docs/design/rendering.md, Colour vision): the finished colours in linear
        // light through one matrix, a simulation of a dichromat's sight or its correction.
        let l = select(pow((e + 0.055) / 1.055, vec3f(2.4)), e / 12.92, e <= vec3f(0.04045));
        e = encode(clamp(vec3f(dot(post.cb0.xyz, l), dot(post.cb1.xyz, l), dot(post.cb2.xyz, l)), vec3f(0.0), vec3f(1.0)));
    }
    return vec4f(e, 1.0);
}
)WGSL";

// The auto exposure's meter: 4096 samples on a grid over the viewport, their mean log luminance,
// and an exposure that brings it to mid gray, eased toward from the last frame's in EV.
// The sky's environment: `fill` writes level 0 of the panorama (procedural, or the image turned
// by the rotation), `prefilter` convolves each next level with a GGX lobe (importance sampled from
// the level above, the lobe widened by what that level already has), `irradiance` projects a small
// level onto nine spherical harmonics for the diffuse light.
constexpr const char* kSkyWgsl = R"WGSL(
struct SkyParams { zenith: vec4f, horizon: vec4f, ground: vec4f, sun: vec4f, sun_color: vec4f, misc: vec4f, size: vec4f, atmo: vec4f, moon: vec4f, moon_color: vec4f };
@group(0) @binding(0) var<uniform> sp: SkyParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;
const PI = 3.14159265;
fn dir_of(uv: vec2f) -> vec3f {
    let phi = (uv.x - 0.5) * 2.0 * PI;
    let theta = uv.y * PI;
    return vec3f(sin(theta) * sin(phi), cos(theta), -sin(theta) * cos(phi));
}
fn uv_of(d: vec3f) -> vec2f {
    return vec2f(atan2(d.x, -d.z) / (2.0 * PI) + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}
fn procedural(d: vec3f) -> vec3f {
    let up = d.y;
    var c: vec3f;
    if (up >= 0.0) {
        c = mix(sp.horizon.rgb, sp.zenith.rgb, pow(up, 0.5));
    } else {
        c = mix(mix(sp.horizon.rgb, sp.ground.rgb, 0.5), sp.ground.rgb, pow(min(-up * 4.0, 1.0), 0.5));
    }
    if (sp.sun.w > 0.5 && up > -0.05) {
        // The glow around the sun (the disc itself is drawn by the sky pass, and lights through the sun light).
        let mu = max(dot(d, sp.sun.xyz), 0.0);
        c = c + sp.sun_color.rgb * (0.08 * pow(mu, 48.0) + 0.02 * pow(mu, 4.0));
    }
    return c;
}
// The atmosphere (docs/design/rendering.md, Atmosphere): the sun's light scattered once on its way
// through a planet's air toward the eye, by the air itself (Rayleigh: blue, all around) and by haze
// (Mie: white, forward around the sun), each thinning with height, after Nishita. The renderer's
// atmosphere_tint integrates the same air along the sun's way down for the sun light's colour.
const R_GROUND = 6360e3;
const R_TOP = 6420e3;
const BETA_R = vec3f(5.8e-6, 13.5e-6, 33.1e-6);
const BETA_M = 21e-6;
const H_R = 8000.0;
const H_M = 1200.0;
const SUN_LIGHT = 16.0;   // the sun's light at the top of the air, for a light of intensity one
// How far from o (inside) along d to the sphere of radius r; negative if the line misses it.
fn to_sphere(o: vec3f, d: vec3f, r: f32) -> f32 {
    let b = dot(o, d);
    let c = dot(o, o) - r * r;
    let disc = b * b - c;
    if (disc < 0.0) { return -1.0; }
    return -b + sqrt(disc);
}
fn air(d: vec3f, s: vec3f, light: vec3f) -> vec3f {
    let o = vec3f(0.0, R_GROUND + 2.0, 0.0);
    let haze = max(sp.atmo.x, 0.0);
    let len = to_sphere(o, d, R_TOP);
    let steps = 16;
    let ds = len / f32(steps);
    var od_r = 0.0;
    var od_m = 0.0;
    var sum_r = vec3f(0.0);
    var sum_m = vec3f(0.0);
    for (var i = 0; i < steps; i = i + 1) {
        let p = o + d * (ds * (f32(i) + 0.5));
        let h = max(length(p) - R_GROUND, 0.0);
        let hr = exp(-h / H_R) * ds;
        let hm = exp(-h / H_M) * ds;
        od_r = od_r + hr;
        od_m = od_m + hm;
        // The sun's light to here, unless the planet is in its way.
        let ls = to_sphere(p, s, R_TOP);
        let lds = ls / 8.0;
        var lr = 0.0;
        var lm = 0.0;
        var lit = true;
        for (var j = 0; j < 8; j = j + 1) {
            let q = p + s * (lds * (f32(j) + 0.5));
            let hq = length(q) - R_GROUND;
            if (hq < 0.0) { lit = false; break; }
            lr = lr + exp(-hq / H_R) * lds;
            lm = lm + exp(-hq / H_M) * lds;
        }
        if (lit) {
            // The view's own path counts half: light scattered more than once fills in what one
            // scattering takes out along the horizon (it would glow orange at noon otherwise).
            let tau = BETA_R * (0.5 * od_r + lr) + BETA_M * 1.1 * haze * (0.5 * od_m + lm);
            let att = exp(-tau);
            sum_r = sum_r + att * hr;
            sum_m = sum_m + att * hm;
        }
    }
    let mu = dot(d, s);
    let phase_r = 3.0 / (16.0 * PI) * (1.0 + mu * mu);
    let g = 0.76;
    let phase_m = 3.0 / (8.0 * PI) * ((1.0 - g * g) * (1.0 + mu * mu)) / ((2.0 + g * g) * pow(1.0 + g * g - 2.0 * g * mu, 1.5));
    return SUN_LIGHT * light * (sum_r * BETA_R * phase_r + sum_m * BETA_M * haze * phase_m);
}
// The air along d, lit by the sun and, by night, the moon: the same scattering, a dimmer light.
fn lit_air(d: vec3f) -> vec3f {
    var c = air(d, sp.sun.xyz, sp.sun_color.rgb);
    if (sp.moon.w > 0.5) { c = c + air(d, sp.moon.xyz, sp.moon_color.rgb); }
    return c;
}
fn atmosphere(d: vec3f) -> vec3f {
    if (sp.sun.w < 0.5) { return vec3f(0.0); }
    let s = sp.sun.xyz;
    if (d.y >= 0.0) { return lit_air(normalize(vec3f(d.x, max(d.y, 0.02), d.z))); }
    // Below the horizon: the ground, lit by the sun (or the moon) and the sky, fading from the horizon's air.
    let horizon = lit_air(normalize(vec3f(d.x, 0.02, d.z)));
    var lit = sp.ground.rgb * sp.sun_color.rgb * (0.05 + 0.25 * max(s.y, 0.0));
    if (sp.moon.w > 0.5) { lit = lit + sp.ground.rgb * sp.moon_color.rgb * (0.05 + 0.25 * max(sp.moon.y, 0.0)); }
    return mix(horizon, lit, smoothstep(0.0, 0.08, -d.y));
}
@compute @workgroup_size(8, 8) fn fill(@builtin(global_invocation_id) id: vec3u) {
    if (f32(id.x) >= sp.size.x || f32(id.y) >= sp.size.y) { return; }
    let uv = (vec2f(id.xy) + 0.5) / sp.size.xy;
    let d = dir_of(uv);
    var c: vec3f;
    if (sp.misc.x > 2.5) {
        c = atmosphere(d);
    } else if (sp.misc.x > 1.5) {
        let r = sp.misc.y;
        let rd = vec3f(d.x * cos(r) + d.z * sin(r), d.y, -d.x * sin(r) + d.z * cos(r));
        c = textureSampleLevel(src, samp, uv_of(rd), 0.0).rgb;
    } else {
        c = procedural(d);
    }
    // Overcast (a Weather), the sky's light greys toward its own brightness, so what reflects it and
    // what it lights is the grey of a clouded sky rather than the clear blue above the clouds.
    c = mix(c, vec3f(dot(c, vec3f(0.2126, 0.7152, 0.0722))), clamp(sp.atmo.y, 0.0, 1.0) * 0.85);
    textureStore(dst, vec2i(id.xy), vec4f(c * sp.misc.w, 1.0));
}
fn importance_ggx(xi: vec2f, n: vec3f, a: f32) -> vec3f {
    let phi = 2.0 * PI * xi.x;
    let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
    let h = vec3f(cos(phi) * sin_t, sin(phi) * sin_t, cos_t);
    let up = select(vec3f(1.0, 0.0, 0.0), vec3f(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let tx = normalize(cross(up, n));
    let ty = cross(n, tx);
    return normalize(tx * h.x + ty * h.y + n * h.z);
}
@compute @workgroup_size(8, 8) fn prefilter(@builtin(global_invocation_id) id: vec3u) {
    if (f32(id.x) >= sp.size.x || f32(id.y) >= sp.size.y) { return; }
    let uv = (vec2f(id.xy) + 0.5) / sp.size.xy;
    let n = dir_of(uv);
    let a = sp.misc.z;
    var acc = vec3f(0.0);
    var w = 0.0;
    for (var i = 0u; i < 64u; i = i + 1u) {
        let xi = vec2f(f32(i) / 64.0, f32(reverseBits(i)) * 2.3283064365386963e-10);
        let h = importance_ggx(xi, n, a);
        let l = normalize(2.0 * dot(n, h) * h - n);
        let ndl = dot(n, l);
        if (ndl > 0.0) {
            acc = acc + textureSampleLevel(src, samp, uv_of(l), 0.0).rgb * ndl;
            w = w + ndl;
        }
    }
    textureStore(dst, vec2i(id.xy), vec4f(acc / max(w, 1e-4), 1.0));
}
)WGSL";

// A probe's six views into level 0 of its panorama: each direction looks up the view it falls in
// (by its largest axis) through that view's own projection, so the views need no cube convention.
constexpr const char* kProbeFillWgsl = R"WGSL(
struct ProbeFill { faces: array<mat4x4f, 6>, center: vec4f, size: vec4f };
@group(0) @binding(0) var<uniform> pf: ProbeFill;
@group(0) @binding(1) var views: texture_2d_array<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;
const PI = 3.14159265;
@compute @workgroup_size(8, 8) fn probe_fill(@builtin(global_invocation_id) id: vec3u) {
    if (f32(id.x) >= pf.size.x || f32(id.y) >= pf.size.y) { return; }
    let uv = (vec2f(id.xy) + 0.5) / pf.size.xy;
    let phi = (uv.x - 0.5) * 2.0 * PI;
    let theta = uv.y * PI;
    let d = vec3f(sin(theta) * sin(phi), cos(theta), -sin(theta) * cos(phi));
    let a = abs(d);
    var face = 0;
    if (a.x >= a.y && a.x >= a.z) { face = select(1, 0, d.x > 0.0); }
    else if (a.y >= a.z) { face = select(3, 2, d.y > 0.0); }
    else { face = select(5, 4, d.z > 0.0); }
    let p = pf.faces[face] * vec4f(pf.center.xyz + d, 1.0);
    let fuv = clamp(vec2f(p.x / p.w * 0.5 + 0.5, 0.5 - p.y / p.w * 0.5), vec2f(0.0), vec2f(1.0));
    textureStore(dst, vec2i(id.xy), vec4f(textureSampleLevel(views, samp, fuv, face, 0.0).rgb, 1.0));
}
)WGSL";

constexpr const char* kIrradianceWgsl = R"WGSL(
@group(0) @binding(0) var env_lo: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> sh: array<vec4f, 9>;
var<workgroup> part: array<array<vec3f, 9>, 64>;
const PI = 3.14159265;
@compute @workgroup_size(64) fn irradiance(@builtin(local_invocation_index) li: u32) {
    let dims = textureDimensions(env_lo);
    var c: array<vec3f, 9>;
    let total = dims.x * dims.y;
    for (var t = li; t < total; t = t + 64u) {
        let x = t % dims.x;
        let y = t / dims.x;
        let uv = (vec2f(f32(x), f32(y)) + 0.5) / vec2f(dims);
        let phi = (uv.x - 0.5) * 2.0 * PI;
        let theta = uv.y * PI;
        let d = vec3f(sin(theta) * sin(phi), cos(theta), -sin(theta) * cos(phi));
        let dw = (2.0 * PI / f32(dims.x)) * (PI / f32(dims.y)) * sin(theta);
        let l = textureLoad(env_lo, vec2i(i32(x), i32(y)), 0).rgb * dw;
        c[0] = c[0] + l * 0.282095;
        c[1] = c[1] + l * (0.488603 * d.y);
        c[2] = c[2] + l * (0.488603 * d.z);
        c[3] = c[3] + l * (0.488603 * d.x);
        c[4] = c[4] + l * (1.092548 * d.x * d.y);
        c[5] = c[5] + l * (1.092548 * d.y * d.z);
        c[6] = c[6] + l * (0.315392 * (3.0 * d.z * d.z - 1.0));
        c[7] = c[7] + l * (1.092548 * d.x * d.z);
        c[8] = c[8] + l * (0.546274 * (d.x * d.x - d.y * d.y));
    }
    for (var k = 0; k < 9; k = k + 1) { part[li][k] = c[k]; }
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride = stride / 2u) {
        if (li < stride) {
            for (var k = 0; k < 9; k = k + 1) { part[li][k] = part[li][k] + part[li + stride][k]; }
        }
        workgroupBarrier();
    }
    if (li == 0u) {
        // Convolved with the cosine lobe and divided by pi (the engine's diffuse has no 1/pi):
        // A0 = pi, A1 = 2 pi / 3, A2 = pi / 4.
        var a = array<f32, 9>(1.0, 0.6666667, 0.6666667, 0.6666667, 0.25, 0.25, 0.25, 0.25, 0.25);
        for (var k = 0; k < 9; k = k + 1) { sh[k] = vec4f(part[0][k] * a[k], 0.0); }
    }
}
)WGSL";

// An irradiance volume's probe: its six views (as a reflection probe's capture draws them) looked
// up along 64 by 32 directions over the sphere and projected onto nine harmonics, convolved with
// the cosine lobe as the sky's are, into the probe's slot.
constexpr const char* kGridShWgsl = R"WGSL(
// misc: the slot's first vec4 in the buffer, its moments' first, the distance the moments stop at.
struct GridFill { faces: array<mat4x4f, 6>, center: vec4f, misc: vec4f };
@group(0) @binding(0) var<uniform> pf: GridFill;
@group(0) @binding(1) var views: texture_2d_array<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<storage, read_write> all_sh: array<vec4f>;
@group(0) @binding(4) var depths: texture_depth_2d_array;
var<workgroup> part: array<array<vec3f, 9>, 64>;
var<workgroup> moments: array<vec2f, 256>;
// The views' depth range (probe_views: near 0.05, far 1000).
const NEAR = 0.05;
const FAR = 1000.0;
fn oct_dir(e: vec2f) -> vec3f {
    var n = vec3f(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) { n = vec3f((1.0 - abs(n.yx)) * select(vec2f(-1.0), vec2f(1.0), n.xy >= vec2f(0.0)), n.z); }
    return normalize(n);
}
// How far the probe saw along d: the face's depth there, from view depth to distance along d.
fn seen(d: vec3f) -> f32 {
    let a = abs(d);
    var face = 0;
    var axis = a.x;
    if (a.x >= a.y && a.x >= a.z) { face = select(1, 0, d.x > 0.0); }
    else if (a.y >= a.z) { face = select(3, 2, d.y > 0.0); axis = a.y; }
    else { face = select(5, 4, d.z > 0.0); axis = a.z; }
    let p = pf.faces[face] * vec4f(pf.center.xyz + d, 1.0);
    let fuv = clamp(vec2f(p.x / p.w * 0.5 + 0.5, 0.5 - p.y / p.w * 0.5), vec2f(0.0), vec2f(0.9999));
    let z = textureLoad(depths, vec2i(fuv * vec2f(textureDimensions(depths))), face, 0);
    if (z >= 1.0) { return pf.misc.z; }
    let view = FAR * NEAR / (FAR - z * (FAR - NEAR));
    return min(view / max(axis, 1e-4), pf.misc.z);
}
const PI = 3.14159265;
fn look(d: vec3f) -> vec3f {
    let a = abs(d);
    var face = 0;
    if (a.x >= a.y && a.x >= a.z) { face = select(1, 0, d.x > 0.0); }
    else if (a.y >= a.z) { face = select(3, 2, d.y > 0.0); }
    else { face = select(5, 4, d.z > 0.0); }
    let p = pf.faces[face] * vec4f(pf.center.xyz + d, 1.0);
    let fuv = clamp(vec2f(p.x / p.w * 0.5 + 0.5, 0.5 - p.y / p.w * 0.5), vec2f(0.0), vec2f(1.0));
    return textureSampleLevel(views, samp, fuv, face, 0.0).rgb;
}
@compute @workgroup_size(64) fn grid_sh(@builtin(local_invocation_index) li: u32) {
    let w = 64u;
    let h = 32u;
    var c: array<vec3f, 9>;
    for (var t = li; t < w * h; t = t + 64u) {
        let uv = (vec2f(f32(t % w), f32(t / w)) + 0.5) / vec2f(f32(w), f32(h));
        let phi = (uv.x - 0.5) * 2.0 * PI;
        let theta = uv.y * PI;
        let d = vec3f(sin(theta) * sin(phi), cos(theta), -sin(theta) * cos(phi));
        let dw = (2.0 * PI / f32(w)) * (PI / f32(h)) * sin(theta);
        let l = look(d) * dw;
        c[0] = c[0] + l * 0.282095;
        c[1] = c[1] + l * (0.488603 * d.y);
        c[2] = c[2] + l * (0.488603 * d.z);
        c[3] = c[3] + l * (0.488603 * d.x);
        c[4] = c[4] + l * (1.092548 * d.x * d.y);
        c[5] = c[5] + l * (1.092548 * d.y * d.z);
        c[6] = c[6] + l * (0.315392 * (3.0 * d.z * d.z - 1.0));
        c[7] = c[7] + l * (1.092548 * d.x * d.z);
        c[8] = c[8] + l * (0.546274 * (d.x * d.x - d.y * d.y));
    }
    for (var k = 0; k < 9; k = k + 1) { part[li][k] = c[k]; }
    // This thread's four texels of the 16 by 16 octahedral map: the mean distance seen over each
    // (3 by 3 directions) and the mean of its square.
    for (var k = 0u; k < 4u; k = k + 1u) {
        let t = li * 4u + k;
        let texel = vec2f(f32(t % 16u), f32(t / 16u));
        var m = vec2f(0.0);
        for (var s = 0u; s < 9u; s = s + 1u) {
            let e = (texel + (vec2f(f32(s % 3u), f32(s / 3u)) + 0.5) / 3.0) / 16.0 * 2.0 - 1.0;
            let r = seen(oct_dir(e));
            m = m + vec2f(r, r * r);
        }
        moments[t] = m / 9.0;
    }
    workgroupBarrier();
    for (var stride = 32u; stride > 0u; stride = stride / 2u) {
        if (li < stride) {
            for (var k = 0; k < 9; k = k + 1) { part[li][k] = part[li][k] + part[li + stride][k]; }
        }
        workgroupBarrier();
    }
    if (li == 0u) {
        var a = array<f32, 9>(1.0, 0.6666667, 0.6666667, 0.6666667, 0.25, 0.25, 0.25, 0.25, 0.25);
        for (var k = 0; k < 9; k = k + 1) { all_sh[u32(pf.misc.x) + u32(k)] = vec4f(part[0][k] * a[k], 0.0); }
    }
    for (var k = 0u; k < 2u; k = k + 1u) {
        let v = li * 2u + k;
        all_sh[u32(pf.misc.y) + v] = vec4f(moments[v * 2u], moments[v * 2u + 1u]);
    }
}
)WGSL";

// Ambient occlusion from the depth prepass, at half resolution: for each point, its position and
// normal from depth, then samples in a hemisphere around the normal (a spiral turned per pixel);
// a sample the depth buffer has something in front of, within the radius, occludes. Beside it in
// the green channel, contact shadows: a short march from the point toward the sun, shadowed where
// the depth buffer has something in front of a step by less than the march's length (a thin
// caster, not a wall far behind). Then a blur that keeps to one surface (neighbors at another
// depth count less).
constexpr const char* kAoWgsl = R"WGSL(
struct Ao { view_proj: mat4x4f, inv_view_proj: mat4x4f, camera: vec4f, fwd: vec4f, params: vec4f, size: vec4f, sun: vec4f, contact: vec4f };
@group(0) @binding(0) var<uniform> ao: Ao;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var src: texture_2d<f32>;
@vertex fn vs_screen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}
fn depth_at(px: vec2i) -> f32 {
    let dims = vec2i(textureDimensions(depth));
    return textureLoad(depth, clamp(px, vec2i(0), dims - vec2i(1)), 0);
}
fn world_at(px: vec2f, d: f32) -> vec3f {
    let ndc = vec2f(px.x / ao.size.x * 2.0 - 1.0, 1.0 - px.y / ao.size.y * 2.0);
    let w = ao.inv_view_proj * vec4f(ndc, d, 1.0);
    return w.xyz / w.w;
}
fn view_depth(p: vec3f) -> f32 { return dot(p - ao.camera.xyz, ao.fwd.xyz); }
@fragment fn fs_ao(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let full = vec2i(pos.xy * 2.0);
    let d = depth_at(full);
    if (d >= 1.0) { return vec4f(1.0, 1.0, 6.0e4, 1.0); }   // the sky: unoccluded, and far for the blur
    let fp = vec2f(full) + 0.5;
    let p = world_at(fp, d);
    // The normal from the neighbors on the nearer side, so an edge does not bend it.
    let r = world_at(fp + vec2f(2.0, 0.0), depth_at(full + vec2i(2, 0)));
    let l = world_at(fp - vec2f(2.0, 0.0), depth_at(full - vec2i(2, 0)));
    let dn = world_at(fp + vec2f(0.0, 2.0), depth_at(full + vec2i(0, 2)));
    let up = world_at(fp - vec2f(0.0, 2.0), depth_at(full - vec2i(0, 2)));
    let dx = select(p - l, r - p, abs(view_depth(r) - view_depth(p)) < abs(view_depth(l) - view_depth(p)));
    let dy = select(p - up, dn - p, abs(view_depth(dn) - view_depth(p)) < abs(view_depth(up) - view_depth(p)));
    var n = normalize(cross(dy, dx));
    if (dot(n, ao.camera.xyz - p) < 0.0) { n = -n; }
    let t = normalize(cross(n, select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(n.y) > 0.9)));
    let b = cross(n, t);
    let noise = fract(52.9829189 * fract(dot(pos.xy, vec2f(0.06711056, 0.00583715))));
    let count = select(0u, u32(ao.params.z), ao.contact.y > 0.5);
    let radius = ao.params.x;
    let here = view_depth(p);
    var occluded = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let k = (f32(i) + 0.5) / f32(count);
        let angle = f32(i) * 2.3999632 + noise * 6.2831853;
        let spread = sqrt(k);
        let dir = t * (cos(angle) * spread) + b * (sin(angle) * spread) + n * sqrt(1.0 - k);
        let s = p + dir * radius * mix(0.25, 1.0, fract(k * 7.0 + noise));
        let clip = ao.view_proj * vec4f(s, 1.0);
        if (clip.w <= 0.0) { continue; }
        let sn = clip.xy / clip.w;
        let spx = vec2f((sn.x * 0.5 + 0.5) * ao.size.x, (0.5 - sn.y * 0.5) * ao.size.y);
        let bd = depth_at(vec2i(spx));
        if (bd >= 1.0) { continue; }
        let behind = view_depth(world_at(spx, bd));
        let at = view_depth(s);
        if (behind < at - 0.02 * radius) {
            occluded = occluded + smoothstep(0.0, 1.0, radius / max(abs(here - behind), 1e-4));
        }
    }
    let ambient = clamp(1.0 - ao.params.y * occluded / f32(max(count, 1u)), 0.0, 1.0);
    // Contact shadows: steps toward the sun from just off the surface.
    var contact = 1.0;
    let to_sun = ao.sun.xyz;
    if (ao.sun.w > 0.5 && dot(n, to_sun) > 0.0) {
        let len = ao.contact.x;
        let steps = 12;
        let start = p + n * (0.02 * len);
        for (var i = 0; i < steps; i = i + 1) {
            let s = start + to_sun * (len * (f32(i) + 0.5 + noise * 0.5) / f32(steps));
            let clip = ao.view_proj * vec4f(s, 1.0);
            if (clip.w <= 0.0) { break; }
            let sn = clip.xy / clip.w;
            if (abs(sn.x) > 1.0 || abs(sn.y) > 1.0) { break; }
            let spx = vec2f((sn.x * 0.5 + 0.5) * ao.size.x, (0.5 - sn.y * 0.5) * ao.size.y);
            let bd = depth_at(vec2i(spx));
            if (bd >= 1.0) { continue; }
            let gap = view_depth(s) - view_depth(world_at(spx, bd));
            if (gap > 0.02 * len && gap < len) {
                contact = smoothstep(0.7, 1.0, (f32(i) + 0.5) / f32(steps));   // dark, fading out at the march's end
                break;
            }
        }
    }
    // The view depth rides along in b, so the blur weighs its neighbours without rebuilding them.
    return vec4f(ambient, contact, here, 1.0);
}
@fragment fn fs_ao_blur(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let c = vec2i(pos.xy);
    let dims = vec2i(textureDimensions(src));
    let center = textureLoad(src, c, 0).b;
    var sum = vec2f(0.0);
    var weight = 0.0;
    for (var y = -2; y <= 2; y = y + 1) {
        for (var x = -2; x <= 2; x = x + 1) {
            let q = clamp(c + vec2i(x, y), vec2i(0), dims - vec2i(1));
            let t = textureLoad(src, q, 0);
            let w = 1.0 / (1.0 + abs(t.b - center) * 8.0 / max(ao.params.x, 1e-3));
            sum = sum + t.rg * w;
            weight = weight + w;
        }
    }
    return vec4f(sum / max(weight, 1e-4), 0.0, 1.0);
}
)WGSL";

// Temporal anti-aliasing (docs/design/rendering.md, Temporal anti-aliasing): this frame's HDR color
// blended with the history found where the pixel was last frame (the id pass's motion, or the
// camera's for the sky), the history first clipped to the range of this frame's 3x3 neighbourhood
// in YCoCg so what moved away or was uncovered does not linger; colors are weighed down by their
// luminance while they are blended, so one bright pixel does not flicker through the average.
constexpr const char* kTaaWgsl = R"WGSL(
struct Taa { reproject: mat4x4f, params: vec4f, viewport: vec4f };
@group(0) @binding(0) var<uniform> taa: Taa;
@group(0) @binding(1) var current: texture_2d<f32>;
@group(0) @binding(2) var history: texture_2d<f32>;
@group(0) @binding(3) var velocity: texture_2d<f32>;
@group(0) @binding(4) var depth: texture_depth_2d;
@group(0) @binding(5) var samp: sampler;
@vertex fn vs_screen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}
fn luma(c: vec3f) -> f32 { return dot(c, vec3f(0.2126, 0.7152, 0.0722)); }
fn weigh(c: vec3f) -> vec3f { return c / (1.0 + luma(c)); }
fn unweigh(c: vec3f) -> vec3f { return c / max(1.0 - luma(c), 1e-4); }
fn ycocg(c: vec3f) -> vec3f { return vec3f(0.25 * c.r + 0.5 * c.g + 0.25 * c.b, 0.5 * c.r - 0.5 * c.b, -0.25 * c.r + 0.5 * c.g - 0.25 * c.b); }
fn rgb(c: vec3f) -> vec3f { return vec3f(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z); }
@fragment fn fs_taa(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let dims = vec2i(textureDimensions(current));
    let p = vec2i(pos.xy);
    let cur = weigh(textureLoad(current, p, 0).rgb);
    if (taa.params.y > 0.5) { return vec4f(unweigh(cur), 1.0); }   // no history yet
    var lo = vec3f(1e9);
    var hi = vec3f(-1e9);
    var closest = 2.0;
    var at = p;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let q = clamp(p + vec2i(i, j), vec2i(0), dims - vec2i(1));
            let c = ycocg(weigh(textureLoad(current, q, 0).rgb));
            lo = min(lo, c);
            hi = max(hi, c);
            let d = textureLoad(depth, q, 0);
            if (d < closest) { closest = d; at = q; }
        }
    }
    // The motion of the nearest surface around (so an edge moves with what is in front of it).
    var v = textureLoad(velocity, at, 0).xy;
    if (closest >= 1.0) {
        // Nothing drawn here: the sky moves only with the camera.
        let uv = (pos.xy - taa.viewport.xy) / taa.viewport.zw;
        let h = taa.reproject * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
        v = uv - vec2f(h.x / h.w * 0.5 + 0.5, 0.5 - h.y / h.w * 0.5);
    }
    let back = pos.xy - v * taa.viewport.zw;
    if (back.x < taa.viewport.x || back.y < taa.viewport.y || back.x >= taa.viewport.x + taa.viewport.z || back.y >= taa.viewport.y + taa.viewport.w) {
        return vec4f(unweigh(cur), 1.0);   // it came in from outside the view
    }
    let hist = ycocg(weigh(textureSampleLevel(history, samp, back / vec2f(dims), 0.0).rgb));
    let blended = mix(clamp(hist, lo, hi), ycocg(cur), 1.0 - taa.params.x);
    return vec4f(unweigh(rgb(blended)), 1.0);
}
)WGSL";

// Depth of field and motion blur (docs/design/rendering.md): each a full-screen pass from the HDR
// target into a scratch one, copied back. Depth of field gathers a golden-angle disc as wide as the
// pixel's circle of confusion; a sample nearer than the pixel counts only as far as its own circle
// reaches, so a sharp foreground does not bleed into the blurred background behind it. Motion blur
// samples along the largest motion around the pixel (so a moving thing smears past its own edge),
// the sky's motion taken from the camera.
constexpr const char* kFxWgsl = R"WGSL(
struct Fx { reproject: mat4x4f, inv_view_proj: mat4x4f, camera: vec4f, viewport: vec4f, dof: vec4f, blur: vec4f };
@group(0) @binding(0) var<uniform> fx: Fx;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_2d;
@group(0) @binding(3) var velocity: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;
@vertex fn vs_screen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}
fn distance_at(px: vec2i) -> f32 {
    let dims = vec2i(textureDimensions(depth));
    let q = clamp(px, vec2i(0), dims - vec2i(1));
    let d = textureLoad(depth, q, 0);
    let uv = (vec2f(q) + vec2f(0.5) - fx.viewport.xy) / fx.viewport.zw;
    let w = fx.inv_view_proj * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, select(d, 1.0, d >= 1.0), 1.0);
    return length(w.xyz / w.w - fx.camera.xyz);
}
// The circle of confusion's radius in pixels: the aperture's blur times how far the point is from
// the focus relative to its own distance (a thin lens), capped.
fn coc(dist: f32) -> f32 {
    let r = fx.dof.y * fx.viewport.w * abs(dist - fx.dof.x) / max(dist, 1e-3);
    return min(r, fx.dof.z * fx.viewport.w);
}
@fragment fn fs_dof(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(scene));
    let here = distance_at(vec2i(pos.xy));
    let radius = coc(here);
    let center = textureLoad(scene, vec2i(pos.xy), 0);
    if (radius < 0.5) { return center; }
    var sum = center.rgb;
    var wsum = 1.0;
    let n = 24;
    for (var k = 1; k <= n; k = k + 1) {
        let r = radius * sqrt(f32(k) / f32(n));
        let a = f32(k) * 2.39996323;
        let o = vec2f(cos(a), sin(a)) * r;
        // The texel whose depth decides the weight is the one whose color is taken (no filtering
        // across an edge, which would pull a sharp foreground into the blur behind it).
        let q = clamp(vec2i(pos.xy + o), vec2i(0), vec2i(dims) - vec2i(1));
        let there = distance_at(q);
        // A nearer sample counts as far as its own blur reaches this pixel.
        var w = 1.0;
        if (there < here * 0.98) { w = clamp(coc(there) / max(r, 1.0), 0.0, 1.0); }
        sum = sum + textureLoad(scene, q, 0).rgb * w;
        wsum = wsum + w;
    }
    return vec4f(sum / wsum, center.a);
}
fn motion_at(px: vec2i) -> vec2f {
    let dims = vec2i(textureDimensions(depth));
    let q = clamp(px, vec2i(0), dims - vec2i(1));
    if (textureLoad(depth, q, 0) >= 1.0) {
        let uv = (vec2f(q) + vec2f(0.5) - fx.viewport.xy) / fx.viewport.zw;
        let h = fx.reproject * vec4f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
        return uv - vec2f(h.x / h.w * 0.5 + 0.5, 0.5 - h.y / h.w * 0.5);
    }
    return textureLoad(velocity, q, 0).xy;
}
@fragment fn fs_motion_blur(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(scene));
    // The largest motion around (every 6 pixels in a 5x5), so a moving thing smears past its edge.
    var v = motion_at(vec2i(pos.xy));
    for (var j = -2; j <= 2; j = j + 1) {
        for (var i = -2; i <= 2; i = i + 1) {
            let m = motion_at(vec2i(pos.xy) + vec2i(i, j) * 6);
            if (dot(m, m) > dot(v, v)) { v = m; }
        }
    }
    let step = v * fx.viewport.zw * fx.blur.x;   // pixels covered while the shutter is open
    let center = textureLoad(scene, vec2i(pos.xy), 0);
    if (dot(step, step) < 0.25) { return center; }
    let n = max(i32(fx.blur.y), 2);
    // A pixel that moves with the fastest motion around is smeared along it, what lies behind
    // showing through; a pixel that stays takes only what passes over it (the samples that move and
    // are nearer), keeping its own color for the rest, so a post beside a thrown ball is not smeared
    // itself, nor the ball drawn over it from behind.
    let own = motion_at(vec2i(pos.xy));
    let moving = dot(own, own) >= 0.25 * dot(v, v);
    let here = distance_at(vec2i(pos.xy));
    var sum = vec3f(0.0);
    var passing = 0.0;
    for (var k = 0; k < n; k = k + 1) {
        let t = f32(k) / f32(n - 1) - 0.5;
        let p = pos.xy + step * t;
        let c = textureSampleLevel(scene, samp, p / dims, 0.0).rgb;
        if (moving) {
            sum = sum + c;
            passing = passing + 1.0;
        } else {
            let m = motion_at(vec2i(p));
            // Only what passes in front: a ball behind a post does not smear over it.
            if (dot(m, m) >= 0.25 * dot(v, v) && distance_at(vec2i(p)) <= here * 1.02) {
                sum = sum + c;
                passing = passing + 1.0;
            }
        }
    }
    if (moving) { return vec4f(sum / f32(n), center.a); }
    return vec4f(center.rgb * (1.0 - passing / f32(n)) + sum / f32(n), center.a);
}
)WGSL";

// The composite of order-independent transparency over the HDR target (alpha blended by the pipeline).
constexpr const char* kOitCompositeWgsl = R"WGSL(
@group(0) @binding(0) var accum_tex: texture_2d<f32>;
@group(0) @binding(1) var reveal_tex: texture_2d<f32>;
@vertex fn vs_screen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}
@fragment fn fs_composite(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = vec2i(pos.xy);
    let reveal = textureLoad(reveal_tex, p, 0).r;
    if (reveal >= 0.9999) { discard; }
    let a = textureLoad(accum_tex, p, 0);
    return vec4f(a.rgb / clamp(a.a, 1e-4, 5e4), 1.0 - reveal);
}
)WGSL";

constexpr const char* kExposureWgsl = R"WGSL(
struct Meter { viewport: vec4f, min_ev: f32, max_ev: f32, compensation: f32, rate: f32 };
@group(0) @binding(0) var<uniform> meter: Meter;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> state: array<f32, 4>;   // exposure EV, average EV, initialized
var<workgroup> sums: array<f32, 256>;
@compute @workgroup_size(256) fn measure(@builtin(local_invocation_index) li: u32) {
    var s = 0.0;
    for (var k = 0u; k < 16u; k = k + 1u) {
        let idx = li * 16u + k;
        let g = vec2f(f32(idx % 64u) + 0.5, f32(idx / 64u) + 0.5) / 64.0;
        let p = vec2i(meter.viewport.xy + g * meter.viewport.zw);
        let c = textureLoad(hdr, p, 0).rgb;
        let lum = dot(c, vec3f(0.2126, 0.7152, 0.0722));
        s = s + clamp(log2(max(lum, 1e-8)), meter.min_ev, meter.max_ev);
    }
    sums[li] = s;
    workgroupBarrier();
    for (var stride = 128u; stride > 0u; stride = stride / 2u) {
        if (li < stride) { sums[li] = sums[li] + sums[li + stride]; }
        workgroupBarrier();
    }
    if (li == 0u) {
        let average = sums[0] / 4096.0;
        let target_ev = log2(0.18) - average + meter.compensation;
        var ev = target_ev;
        if (state[2] > 0.5 && meter.rate < 1.0) { ev = mix(state[0], target_ev, meter.rate); }
        state[0] = ev;
        state[1] = average;
        state[2] = 1.0;
    }
}
)WGSL";

struct GpuMesh {
    WGPUBuffer vertices = nullptr;
    WGPUBuffer indices = nullptr;
    WGPUBuffer skin = nullptr;      // joints and weights per vertex, skinned assets only
    std::uint32_t index_count = 0;
    Vec3 aabb_min, aabb_max;
};
constexpr std::uint32_t kMaxJoints = 16384;  // joint matrices per frame across every skinned instance
constexpr std::uint32_t kMaxMorphVec4 = 524288;  // morph deltas (position + normal vec4 per target and vertex) across every asset: 8 MB
constexpr std::uint32_t kMaxMorphTargets = 8;    // targets weighed per instance

void to_array(const Mat4& m, float* out) { std::memcpy(out, m.m, sizeof(float) * 16); }

Mat4 transpose(const Mat4& m) {
    Mat4 r;
    for (int c = 0; c < 4; ++c)
        for (int row = 0; row < 4; ++row) r.at(c, row) = m.at(row, c);
    return r;
}

}  // namespace

struct Renderer::Impl {
    rhi::Device* device = nullptr;
    WGPUShaderModule shader = nullptr;
    WGPUBindGroupLayout frame_bgl = nullptr;
    WGPUBindGroupLayout object_bgl = nullptr;
    WGPUPipelineLayout layout = nullptr;
    WGPURenderPipeline pipeline = nullptr;
    WGPURenderPipeline skinned_pipeline = nullptr;
    WGPURenderPipeline cut_pipeline = nullptr;             // cut-outs (MeshRenderer.cutoff): the only lit pipelines with a discard
    WGPURenderPipeline cut_skinned_pipeline = nullptr;
    WGPURenderPipeline blend_pipeline = nullptr;           // the lit shading alpha blended, no depth writes
    WGPURenderPipeline blend_skinned_pipeline = nullptr;
    WGPURenderPipeline shadow_skinned_pipeline = nullptr;
    WGPURenderPipeline shadow_cut_pipeline = nullptr;     // cut-outs: the texture's holes let the light through
    WGPUPipelineLayout shadow_cut_layout = nullptr;
    WGPUBuffer joint_buffer = nullptr;
    std::vector<float> joint_staging;  // 16 floats per matrix
    std::uint32_t joint_count = 0;
    WGPUBuffer morph_buffer = nullptr;  // every morphed asset's deltas, appended as assets load
    std::uint32_t morph_used = 0;       // vec4s of it in use
    bool morph_full_warned = false;
    const Animation* animation = nullptr;  // poses for the frame being drawn
    WGPURenderPipeline sprite_pipeline = nullptr;
    WGPURenderPipeline sprite_add_pipeline = nullptr;   // additive sprites and particles
    // GPU particles (docs/design/particles.md, On the GPU): each emitter's ring, the compute pass
    // that moves it, and the pipelines that draw it through the sprites' fragment.
    WGPURenderPipeline particle_pipeline = nullptr;
    WGPURenderPipeline particle_add_pipeline = nullptr;
    WGPURenderPipeline weather_pipeline = nullptr;   // rain and snow about the camera (docs/design/rendering.md, Weather)
    float env_overcast = 0;                          // the Weather's overcast, greying the sky's panorama
    float after_dark_lit = 1;                        // 0 by day .. 1 once the sun is down: Light and MeshRenderer after_dark
    std::uint32_t weather_drops = 0;                 // how many this frame
    WGPUBindGroupLayout particle_draw_bgl = nullptr;
    WGPUPipelineLayout particle_layout = nullptr;
    WGPUShaderModule particle_sim_shader = nullptr;
    WGPUBindGroupLayout particle_sim_bgl = nullptr;
    WGPUPipelineLayout particle_sim_layout = nullptr;
    WGPUComputePipeline particle_sim_pipeline = nullptr;
    struct GpuEmitterParams {
        float origin[4], prev[4], axis[4], gravity[4], ranges[4], ground[4], look[4], color0[4], color1[4], place[4];
        std::uint32_t counts[4], extra[4];
        float depth_vp[16], depth_inv[16], depth_info[4], eye[4];
        float area[4], turn[4];
    };
    struct GpuEmitter {
        WGPUBuffer ring = nullptr, params = nullptr;
        WGPUBindGroup sim = nullptr, draw = nullptr;
        std::uint32_t size = 0, head = 0;
        double seconds = 0;          // the pool's seconds simulated so far
        std::uint64_t spawned = 0;   // the pool's particles born so far
        Vec3 last{0, 0, 0};          // where the emitter was at the last window's end
        bool primed = false;
        bool due = false;            // to be moved this frame
        bool seen = false;
        WGPUTextureView depth = nullptr;   // the depth its groups were made with
        void release() {
            if (sim) wgpuBindGroupRelease(sim);
            if (draw) wgpuBindGroupRelease(draw);
            if (ring) wgpuBufferRelease(ring);
            if (params) wgpuBufferRelease(params);
            sim = draw = nullptr;
            ring = params = nullptr;
        }
    };
    std::map<world::EntityId, GpuEmitter> gpu_emitters;
    Mat4 particle_vp, particle_inv;   // the last frame's view, which drew the prepass the compute pass reads
    Vec3 particle_eye{0, 0, 0};
    bool particle_view = false;
    WGPURenderPipeline sprite_lit_pipeline = nullptr;   // lit sprites and tile maps: the meshes' shading, alpha blended
    AmbientSettings ambient;
    WGPURenderPipeline line_pipeline = nullptr;
    WGPUPipelineLayout line_layout = nullptr;
    WGPUShaderModule line_shader = nullptr;
    WGPUBuffer line_buffer = nullptr;
    std::size_t line_capacity = 0;  // vertices
    WGPURenderPipeline shadow_pipeline = nullptr;
    WGPURenderPipeline atlas_pipeline = nullptr, atlas_skinned_pipeline = nullptr, atlas_cut_pipeline = nullptr;   // the same for the lights' atlas
    WGPUPipelineLayout shadow_layout = nullptr;
    WGPUBindGroupLayout scene_bgl = nullptr;   // frame uniforms + shadow map + comparison sampler
    WGPUBindGroup scene_bg = nullptr;
    WGPUTexture shadow_texture = nullptr;
    WGPUTextureView shadow_view = nullptr;              // every cascade, for the lookups
    WGPUTextureView cascade_view[kCascades + 1]{};      // one cascade each, for its pass, and the shelter map
    WGPUBindGroupLayout cascade_bgl = nullptr;          // the cascade's matrix at group 2 binding 5, dynamic offset
    WGPUBuffer cascade_buffer = nullptr;                // one 256-byte slot per cascade, and one for the shelter map
    // The shelter map (kShelterLayer): drawn while there is weather, again when the camera has moved
    // two units on or every eight frames, its view kept for the lookups until the next.
    bool shelter_valid = false;
    float shelter_x = 1e30f, shelter_z = 1e30f;
    std::uint64_t shelter_frame = 0;
    float shelter_vp[16]{};
    float shelter_bias = 0;
    std::uint32_t shelter_draws = 0;   // what the last drawing of it drew
    std::uint64_t shelter_sig = 0;     // what stood over the square then (the draws' places and meshes)
    WGPUBindGroup cascade_bg = nullptr;
    WGPUSampler shadow_sampler = nullptr;
    ShadowSettings shadows;
    // Bloom: the frame's bright parts into a half-size texture, blurred across and down, added back.
    BloomSettings bloom;
    WGPUShaderModule bloom_shader = nullptr;
    WGPUBindGroupLayout bloom_bgl = nullptr;
    WGPUPipelineLayout bloom_layout = nullptr;
    WGPURenderPipeline bloom_bright_pipeline = nullptr;
    WGPURenderPipeline bloom_blur_pipeline = nullptr;
    WGPURenderPipeline bloom_add_pipeline = nullptr;
    WGPUSampler bloom_sampler = nullptr;
    WGPUBuffer bloom_uniforms = nullptr;          // four slots (bright, blur across, blur down, add), 256 bytes apart
    WGPUTexture bloom_tex[2]{};
    WGPUTextureView bloom_view[2]{};
    WGPUBindGroup bloom_bg[4]{};                  // bright (reads the frame), blur across, blur down, add
    WGPUTextureView bloom_src = nullptr;          // the frame view the bright bind group was made for
    std::uint32_t bloom_w = 0, bloom_h = 0;
    // The HDR scene target, and the final pass that exposes, tone-maps, grades and encodes it into
    // the frame; the auto exposure's meter is a compute pass over the same target.
    WGPUTexture hdr_tex = nullptr;
    WGPUTextureView hdr_view = nullptr;
    std::uint32_t hdr_w = 0, hdr_h = 0;
    GradeSettings grade;
    ColourVisionSettings colour_vision;
    ToonSettings toon;
    std::vector<std::pair<std::uint32_t, std::array<float, 4>>> highlights;   // this frame's (MeshRenderer.highlight)
    TonemapSettings tonemap;
    float time_step = 1.0f / 60.0f;
    WGPUShaderModule post_shader = nullptr;
    WGPUBindGroupLayout post_bgl = nullptr;       // uniform, the HDR target, the meter's state
    WGPUPipelineLayout post_layout = nullptr;
    WGPURenderPipeline post_pipeline = nullptr;
    WGPUBuffer post_uniforms = nullptr;
    WGPUBindGroup post_bg = nullptr;
    // Post effects a project wrote, run after the tonemap through two targets in turn.
    struct UserEffect {
        Renderer::PostEffect def;
        WGPURenderPipeline pipeline = nullptr;
        WGPUBuffer uniforms = nullptr;
        WGPUBindGroup bg[2] = {nullptr, nullptr};
        WGPUTextureView bg_view[2] = {nullptr, nullptr};
    };
    std::vector<UserEffect> user_effects;
    // Sprite materials a project wrote: the scene shader with the user's material and two fragment
    // stages around it, and the sprite pipelines over it (plain and additive), made for the sample
    // count and target layout the scene pipelines have now.
    struct SpriteMaterial {
        WGPUShaderModule module = nullptr;
        WGPURenderPipeline pipe = nullptr, add = nullptr;
        int samples = 0;
        bool split = false;
        std::string error;
    };
    std::map<std::string, SpriteMaterial> sprite_materials;
    std::map<std::string, SpriteMaterial> mesh_materials;   // the same for meshes: the lit pass's opaque pipeline over the user's material
    WGPUBindGroupLayout user_fx_bgl = nullptr;
    WGPUPipelineLayout user_fx_layout = nullptr;
    WGPUSampler user_fx_sampler = nullptr;
    WGPUTexture user_fx_tex[2] = {nullptr, nullptr};
    WGPUTextureView user_fx_view[2] = {nullptr, nullptr};
    std::uint32_t user_fx_w = 0, user_fx_h = 0;
    double user_fx_time = 0;
    std::uint64_t user_fx_frames = 0;
    WGPUShaderModule meter_shader = nullptr;
    WGPUBindGroupLayout meter_bgl = nullptr;
    WGPUPipelineLayout meter_layout = nullptr;
    WGPUComputePipeline meter_pipeline = nullptr;
    WGPUBuffer meter_uniforms = nullptr;
    WGPUBuffer meter_state = nullptr;             // 4 floats: exposure EV, average EV, initialized
    WGPUBindGroup meter_bg = nullptr;
    bool meter_reset = true;                      // the next metered frame snaps instead of easing
    WGPUTextureView post_depth = nullptr;         // the depth view the post group was made with
    WGPUTextureView post_ids = nullptr;           // and the id view
    WGPUTextureView post_volume = nullptr;        // and the volume view
    WGPUTextureView post_lut = nullptr;           // and the look-up table's
    // Volumetric fog: the half-size target, a stand-in (nothing gathered, everything shows) when it
    // is off, the pass (the mesh module's vs_volume/fs_volume over the scene group and its own).
    WGPUTexture volume_tex[2]{}, volume_none_tex = nullptr;            // this frame's and last frame's, in turn
    WGPUTextureView volume_view[2]{}, volume_none_view = nullptr;
    bool secondary = false;                  // drawing a secondary view (Renderer::RenderView)
    world::EntityId camera_pick = 0;         // the view's camera, when one was named
    // The window's view between cameras (docs/design/cameras.md, Blends): where it stood last frame,
    // and a blend from the camera that drew it (followed while it lives, else where it was).
    struct ViewPose {
        Vec3 position;
        Quat rotation;
        float fov = 60, ortho_size = 5;
        bool ortho = false;
    };
    world::EntityId window_camera = 0;
    ViewPose window_pose;
    double window_seconds = -1;
    world::EntityId blend_from = 0;
    ViewPose blend_pose;
    double blend_start = 0;
    float blend_seconds = 0;
    int volume_cur = 0;                                                  // the one written this frame
    bool volume_valid = false;                                           // the other holds last frame's
    std::uint64_t volume_frame = 0;
    WGPUBindGroup volume_bgs[2]{};
    WGPUSampler volume_samp = nullptr;
    std::uint32_t volume_w = 0, volume_h = 0;
    WGPUBindGroupLayout volume_bgl = nullptr;
    WGPUPipelineLayout volume_layout = nullptr;
    WGPURenderPipeline volume_pipeline = nullptr;
    WGPUBuffer volume_uniforms = nullptr;
    WGPUTextureView volume_depth = nullptr;       // the depth view volume_bgs read
    // Screen-space global illumination: half-size targets in turn (this frame's, last frame's), the
    // gathering pass and the full-size pass that adds what it gathered.
    SsgiSettings ssgi;
    WGPUTexture gi_tex[2]{};
    WGPUTextureView gi_view[2]{};
    int gi_cur = 0;
    bool gi_valid = false;
    std::uint64_t gi_frame = 0;
    std::uint32_t gi_w = 0, gi_h = 0;
    WGPUBindGroupLayout gi_bgl = nullptr;
    WGPUPipelineLayout gi_layout = nullptr;
    WGPURenderPipeline gi_pipeline = nullptr, gi_add_pipeline = nullptr;
    WGPUBuffer gi_uniforms = nullptr;
    WGPUBindGroup gi_bgs[2]{}, gi_add_bgs[2]{};
    WGPUTextureView gi_bg_views[4]{};             // depth, surface, scene and albedo the groups were made with
    // The depth prepass: the id pass at one sample with a depth target of its own, sampled by the AO
    // pass and the fog afterwards; a 1x1 stand-in when there is none.
    WGPUTexture prepass_tex = nullptr;
    WGPUTextureView prepass_view = nullptr;
    std::uint32_t prepass_w = 0, prepass_h = 0;
    WGPUTexture surface_tex = nullptr, albedo_tex = nullptr;          // the id pass's surface targets
    WGPUTextureView surface_view = nullptr, albedo_view = nullptr;
    SsrSettings ssr;
    WGPUBindGroupLayout ssr_bgl = nullptr;
    WGPUPipelineLayout ssr_layout = nullptr;
    WGPURenderPipeline ssr_pipeline = nullptr;
    WGPUBuffer ssr_uniforms = nullptr;
    std::uint64_t ssr_frame = 0;
    WGPUBindGroup ssr_bg = nullptr;
    WGPUTextureView ssr_bg_scene = nullptr, ssr_bg_depth = nullptr, ssr_bg_surface = nullptr;
    // Water (docs/design/water.md): this frame's bodies as the shader reads them, the pass that draws
    // them after the solid scene (from a copy of it and of the prepass depth), the one that adds them
    // to the prepass, and the camera's body when it is under a surface.
    struct WaterGpu {
        float center[4];     // the rest level's centre, the time
        float extent[4];     // half size x, z; grid cells x, z
        float color[4];      // the deep colour, clarity
        float dir_k[4][4];   // per wave: direction x, z, wave number, angular speed
        float amp[4][4];     // per wave: amplitude, steepness, phase
        float misc[4];       // current x, z, ripples, foam
        float more[4];       // the amplitudes summed, choppiness
        std::uint32_t id[4];
    };
    std::vector<WaterGpu> water_bodies;
    // Decals: this frame's, their images' array (a layer per image path, 0 the built-in spot).
    WGPUBuffer decal_buffer = nullptr;
    WGPUTexture decal_tex = nullptr;
    WGPUTextureView decal_view = nullptr;
    WGPUSampler decal_sampler = nullptr;
    std::map<std::string, std::uint32_t> decal_layers;
    std::vector<std::pair<world::Water, Vec3>> water_src;   // the components and centres behind them
    int water_under = -1;
    WGPUBindGroupLayout water_bgl = nullptr;
    WGPUPipelineLayout water_layout = nullptr;
    WGPURenderPipeline water_pipeline[2]{}, water_under_pipeline[2]{};   // against the frame's depth, against the prepass
    WGPURenderPipeline water_depth_pipeline = nullptr;
    WGPUBuffer water_uniforms = nullptr;
    WGPUSampler water_sampler = nullptr;
    WGPUTexture water_scene_tex = nullptr, water_depth_tex = nullptr;
    WGPUTextureView water_scene_view = nullptr, water_depth_view = nullptr;
    std::uint32_t water_w = 0, water_h = 0;
    WGPUBindGroup water_bg = nullptr;
    WGPUTexture velocity_tex = nullptr;       // the id pass's motion target, the prepass's size
    WGPUTextureView velocity_view = nullptr;
    // TAA: the settings, the resolved frames (two, one the history of the other), the pass, the
    // jitter's frame count, last frame's unjittered view-projection and each object's model then.
    TaaSettings taa;
    bool oit = false;
    WGPURenderPipeline oit_pipeline = nullptr, oit_skinned_pipeline = nullptr, oit_composite_pipeline = nullptr;
    WGPUShaderModule oit_shader = nullptr;
    WGPUBindGroupLayout oit_bgl = nullptr;
    WGPUPipelineLayout oit_layout = nullptr;
    WGPUTexture oit_accum_tex = nullptr, oit_reveal_tex = nullptr;
    WGPUTextureView oit_accum_view = nullptr, oit_reveal_view = nullptr;
    std::uint32_t oit_w = 0, oit_h = 0;
    // With MSAA: the same pipelines at 4 samples, and multisampled targets resolved into the two above.
    WGPURenderPipeline oit_ms_pipeline = nullptr, oit_ms_skinned_pipeline = nullptr, oit_ms_composite_pipeline = nullptr;
    WGPUTexture oit_ms_accum_tex = nullptr, oit_ms_reveal_tex = nullptr;
    WGPUTextureView oit_ms_accum_view = nullptr, oit_ms_reveal_view = nullptr;
    int oit_samples = 1;
    WGPUBindGroup oit_bg = nullptr;
    DofSettings dof;
    MotionBlurSettings motion_blur;
    WGPUShaderModule fx_shader = nullptr;
    WGPUBindGroupLayout fx_bgl = nullptr;
    WGPUPipelineLayout fx_layout = nullptr;
    WGPURenderPipeline dof_pipeline = nullptr, blur_pipeline = nullptr;
    WGPUBuffer fx_uniforms = nullptr;
    WGPUTexture fx_tex = nullptr;
    WGPUTextureView fx_view = nullptr;
    std::uint32_t fx_w = 0, fx_h = 0;
    WGPUBindGroup fx_bg = nullptr;
    WGPUTextureView fx_bg_scene = nullptr, fx_bg_depth = nullptr, fx_bg_velocity = nullptr;
    WGPUTexture taa_tex[2] = {nullptr, nullptr};
    WGPUTextureView taa_view[2] = {nullptr, nullptr};
    std::uint32_t taa_w = 0, taa_h = 0;
    int taa_next = 0;                          // the one written this frame
    bool taa_valid = false;                    // whether the history holds a frame of this view
    std::optional<ViewOverride> view_override;   // render.views: a view instead of the scene's camera
    bool motion_prev_set = false;              // whether taa_prev_vp is last frame's (motion blur without TAA)
    std::uint64_t taa_frame = 0;
    Mat4 taa_prev_vp = Mat4::identity();
    std::unordered_map<std::uint64_t, std::array<float, 16>> prev_models;
    WGPUShaderModule taa_shader = nullptr;
    WGPUBindGroupLayout taa_bgl = nullptr;
    WGPUPipelineLayout taa_layout = nullptr;
    WGPURenderPipeline taa_pipeline = nullptr;
    WGPUBuffer taa_uniforms = nullptr;
    WGPUBindGroup taa_bg[2] = {nullptr, nullptr};
    WGPUTextureView taa_bg_current = nullptr, taa_bg_velocity = nullptr, taa_bg_depth = nullptr;   // what taa_bg was made over
    WGPUTexture no_depth_tex = nullptr;
    WGPUTextureView no_depth_view = nullptr;
    bool split_applied = false;                   // whether the scene pipelines were built split (ids apart)
    // Ambient occlusion: a half-size target, then a depth-aware blur into a second one.
    AoSettings ao;
    WGPUShaderModule ao_shader = nullptr;
    WGPUBindGroupLayout ao_bgl = nullptr;
    WGPUPipelineLayout ao_layout = nullptr;
    WGPURenderPipeline ao_pipeline = nullptr;
    WGPURenderPipeline ao_blur_pipeline = nullptr;
    WGPUBuffer ao_uniforms = nullptr;
    WGPUTexture ao_tex[2]{};
    WGPUTextureView ao_view[2]{};
    WGPUBindGroup ao_bg[2]{};
    std::uint32_t ao_w = 0, ao_h = 0;
    WGPUTexture ao_white_tex = nullptr;
    WGPUTextureView ao_white_view = nullptr;
    WGPUTextureView scene_ao = nullptr;           // the AO view the scene group was made with
    WGPUBindGroupEntry scene_entries[19]{};       // the scene group's entries, to make it again when the AO target, the atlas or the glass copy changes
    WGPUTextureView scene_glass = nullptr;        // the glass copy the scene group was made with
    WGPUTexture glass_tex = nullptr, glass_stub = nullptr;   // the scene before the glass (frame-sized), and a black texel before there is one
    WGPUTextureView glass_view = nullptr, glass_stub_view = nullptr;
    // 2D shadows (TileMap.shadows): the casting map's solid cells, one R8 texel each, uploaded when
    // they change; a 1x1 empty stand-in without one.
    WGPUTexture occ_texture = nullptr, occ_stub = nullptr;
    WGPUTextureView occ_view = nullptr, occ_stub_view = nullptr, scene_occ = nullptr;
    std::uint32_t occ_w = 0, occ_h = 0;
    std::vector<std::uint8_t> occ_cells;
    float occ_uniform[8] = {};   // FrameUniforms.occluders and occluder_size for this frame

    // The first visible orthogonal TileMap with `shadows`: its solid cells into the texture.
    void gather_occluders(const world::World& world) {
        for (float& v : occ_uniform) v = 0;
        if (!assets) return;
        bool found = false;
        world.ecs().each([&](const world::TileMap& tmc, const world::WorldTransform& t) {
            if (found || !tmc.shadows || !tmc.visible || tmc.map.empty()) return;
            auto map = assets->tilemap(tmc.map);
            if (!map || !(*map)->orthogonal() || (*map)->width <= 0 || (*map)->height <= 0) return;
            found = true;
            const auto w = static_cast<std::uint32_t>((*map)->width), h = static_cast<std::uint32_t>((*map)->height);
            std::vector<std::uint8_t> cells(static_cast<std::size_t>(w) * h);
            for (std::uint32_t y = 0; y < h; ++y)
                for (std::uint32_t x = 0; x < w; ++x) cells[static_cast<std::size_t>(y) * w + x] = (*map)->solidity_at(static_cast<int>(x), static_cast<int>(y)) == 1 ? 255 : 0;
            if (w != occ_w || h != occ_h || !occ_texture) {
                if (occ_view) wgpuTextureViewRelease(occ_view);
                if (occ_texture) wgpuTextureRelease(occ_texture);
                auto [tex, view] = make_target("pocket.occluders", w, h, WGPUTextureFormat_R8Unorm, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
                occ_texture = tex;
                occ_view = view;
                occ_w = w;
                occ_h = h;
                occ_cells.clear();
            }
            if (!occ_texture) return;
            if (cells != occ_cells) {
                WGPUTexelCopyTextureInfo dst{};
                dst.texture = occ_texture;
                dst.aspect = WGPUTextureAspect_All;
                WGPUTexelCopyBufferLayout layout{};
                layout.bytesPerRow = w;
                layout.rowsPerImage = h;
                const WGPUExtent3D size{w, h, 1};
                wgpuQueueWriteTexture(device->queue(), &dst, cells.data(), cells.size(), &layout, &size);
                occ_cells = std::move(cells);
            }
            const float ts = tmc.tile_size > 0 ? tmc.tile_size : 1.0f;
            occ_uniform[0] = t.position.x;
            occ_uniform[1] = t.position.y;
            occ_uniform[2] = ts;
            occ_uniform[3] = 1;
            occ_uniform[4] = static_cast<float>(w);
            occ_uniform[5] = static_cast<float>(h);
        });
    }
    std::uint32_t glass_w = 0, glass_h = 0;
    bool glass_last = false;                      // glass was drawn last frame (the depth prepass stays on for it)
    Vec3 sun_toward{0, 1, 0};                     // toward this frame's sun (contact shadows march along it)
    bool contact_now = false;                     // contact shadows this frame: asked for, and a sun to cast them
    WGPUBuffer probe_sh_buffer = nullptr;         // each probe's diffuse harmonics, 256 bytes a slot
    WGPUBuffer probe_cluster_buffer = nullptr;    // a capture's one cluster: every local light that reaches the probe
    // Reflection probes: their slots (one array layer each), the panoramas, the six views a capture
    // draws, the passes that draw them and turn them into a panorama.
    struct ProbeSlot {
        std::uint64_t entity = 0;
        Vec3 center{0, 0, 0}, size{0, 0, 0};
        float intensity = 1;
        bool box = true, realtime = false, captured = false;
        int bounces = 0;           // captures still to make (kProbeBounces)
        std::uint64_t frame = 0;   // the frame it was last captured in
    };
    std::array<ProbeSlot, kMaxProbes> probe_slots{};
    std::uint32_t probe_count = 0;
    std::uint32_t probe_cursor = 0;   // where the search for a probe to capture starts (realtime ones take turns)
    bool probe_refresh = false;
    std::uint64_t frame_number = 0;
    WGPUTexture probe_env_tex = nullptr, probe_views_tex = nullptr, probe_depth_tex = nullptr;
    WGPUTextureView probe_env_view = nullptr, probe_views_array = nullptr, probe_depth_array = nullptr;
    WGPUTextureView probe_depth_face[6]{};       // each view's depth, kept for an irradiance probe's moments
    WGPUTextureView probe_level[kMaxProbes][kProbeLevels]{};
    WGPUTextureView probe_view[6]{};
    WGPUShaderModule probe_fill_shader = nullptr;
    WGPUBindGroupLayout probe_fill_bgl = nullptr;
    WGPUPipelineLayout probe_fill_layout = nullptr;
    WGPUComputePipeline probe_fill_pipeline = nullptr;
    WGPUBuffer probe_fill_params = nullptr, probe_prefilter_params = nullptr;
    WGPUBuffer probe_frame_buf[6]{};
    // Irradiance volumes: each a grid of probes whose harmonics follow the reflection probes' in
    // probe_sh_buffer, captured kGridPerFrame probes a frame, kProbeBounces passes over the grid.
    struct GridVolume {
        std::uint64_t entity = 0;
        Vec3 center{0, 0, 0}, size{0, 0, 0};
        std::uint32_t nx = 0, ny = 0, nz = 0;
        std::uint32_t first = 0;   // its first probe among all the volumes' probes
        float intensity = 1;
        bool visibility = true;    // probes weighed by whether they saw the point
        int passes = 0;            // passes over its probes still to make
        std::uint32_t next = 0;    // the next probe of this pass
        bool ready = false;        // every probe captured at least once
        std::uint64_t frame = 0;   // the frame a probe of it was last captured in
        [[nodiscard]] std::uint32_t count() const { return nx * ny * nz; }
        [[nodiscard]] Vec3 probe_at(std::uint32_t i) const {
            const std::uint32_t x = i % nx, y = (i / nx) % ny, z = i / (nx * ny);
            const Vec3 lo = center - size * 0.5f;
            return Vec3{lo.x + size.x * static_cast<float>(x) / static_cast<float>(nx - 1), lo.y + size.y * static_cast<float>(y) / static_cast<float>(ny - 1), lo.z + size.z * static_cast<float>(z) / static_cast<float>(nz - 1)};
        }
    };
    std::vector<GridVolume> grids;
    WGPUBuffer grid_frame_buf[kGridPerFrame][6]{};
    WGPUBuffer grid_params[kGridPerFrame]{};
    WGPUBuffer grid_cluster_buffer = nullptr;   // the lights reaching the volume being captured
    WGPUShaderModule grid_shader = nullptr;
    WGPUBindGroupLayout grid_bgl = nullptr;
    WGPUPipelineLayout grid_layout = nullptr;
    WGPUComputePipeline grid_pipeline = nullptr;
    std::vector<WGPUBindGroup> grid_groups;
    WGPURenderPipeline probe_pipeline = nullptr, probe_skinned_pipeline = nullptr, probe_sky_pipeline = nullptr;
    std::vector<WGPUBindGroup> probe_groups;   // the last capture's groups, released at the next
    // The sky: its pipelines, the environment map (level views for the compute passes, one view of
    // every level for sampling), the harmonics buffer, the panorama's source texture.
    WGPUPipelineLayout sky_layout = nullptr;      // the scene group only
    WGPURenderPipeline sky_pipeline = nullptr;    // follows the sample count like the scene pipelines
    WGPUShaderModule sky_shader = nullptr;
    WGPUShaderModule irr_shader = nullptr;
    WGPUBindGroupLayout env_bgl = nullptr;
    WGPUBindGroupLayout irr_bgl = nullptr;
    WGPUPipelineLayout env_layout = nullptr;
    WGPUPipelineLayout irr_layout = nullptr;
    WGPUComputePipeline fill_pipeline = nullptr;
    WGPUComputePipeline prefilter_pipeline = nullptr;
    WGPUComputePipeline irradiance_pipeline = nullptr;
    WGPUTexture env_tex = nullptr;
    WGPUTextureView env_view = nullptr;
    WGPUTextureView env_level[kEnvLevels]{};
    WGPUSampler env_sampler = nullptr;
    WGPUBuffer env_params = nullptr;              // one 256-byte slot per level
    WGPUBuffer sh_buffer = nullptr;
    WGPUBindGroup fill_bg = nullptr;
    WGPUBindGroup prefilter_bg[kEnvLevels]{};
    WGPUBindGroup irr_bg = nullptr;
    std::string sky_source_path;
    std::string env_key;                          // what the environment was last built from
    std::uint32_t env_updates = 0;
    WGPUBuffer frame_buffer = nullptr;
    WGPUBuffer object_buffer = nullptr;
    WGPUBuffer light_buffer = nullptr;     // point and spot lights (GpuLight), nearest first
    WGPUBuffer cluster_buffer = nullptr;   // the froxel grid (first entry and count per cluster), then the entries
    std::vector<GpuLight> light_staging, light_packed;
    std::vector<std::uint32_t> cluster_staging;
    std::vector<std::pair<Vec3, Vec3>> cluster_box;   // each cluster's box in view space, this frame
    // Local lights' shadows: the atlas (made when a light first asks for shadows; a 1x1 stand-in
    // until then), the faces' views for the shadow pass (256-byte slots, dynamic offsets) and for
    // the lookup (GpuFace).
    WGPUTexture atlas_texture = nullptr;
    WGPUTextureView atlas_view = nullptr;
    WGPUTexture atlas_stub = nullptr;
    WGPUTextureView atlas_stub_view = nullptr;
    WGPUTextureView scene_atlas = nullptr;   // the atlas view the scene group was made with
    WGPUBuffer face_slots = nullptr;
    WGPUBindGroup face_bg = nullptr;
    WGPUBuffer face_buffer = nullptr;
    std::vector<GpuFace> faces;
    std::vector<std::pair<Vec3, float>> face_light;   // per face, the light's position and range (what its shadow pass draws)
    WGPUBindGroup frame_bg = nullptr;
    WGPUBindGroup object_bg = nullptr;
    WGPUTexture id_texture = nullptr;
    WGPUTextureView id_view = nullptr;
    std::uint32_t id_width = 0, id_height = 0;
    // MSAA: the scene draws into multisampled color and depth and resolves into the frame; the
    // ids then come from their own single-sample pass with these pipelines.
    float last_clock = 0;  // the simulated seconds of the last frame drawn (swaying copies' motion)
    bool fog_from_sky = false;   // an atmosphere: the fog takes the sky's colour around the horizon
    Vec3 fog_sky{0, 0, 0};
    bool clock_valid = false;
    int msaa = 1;          // requested (1 or 4)
    int msaa_applied = 1;  // what the scene pipelines were built with
    WGPUTexture ms_color = nullptr;
    WGPUTextureView ms_color_view = nullptr;
    WGPUTexture ms_depth = nullptr;
    WGPUTextureView ms_depth_view = nullptr;
    std::uint32_t ms_width = 0, ms_height = 0;
    int ms_samples = 1;
    WGPURenderPipeline id_pipeline = nullptr;
    WGPURenderPipeline id_skinned_pipeline = nullptr;
    WGPURenderPipeline id_cut_pipeline = nullptr;
    WGPURenderPipeline id_cut_skinned_pipeline = nullptr;
    WGPURenderPipeline id_sprite_pipeline = nullptr;
    WGPUVertexAttribute mesh_attrs[5]{};
    WGPUVertexAttribute skin_attrs[2]{};
    WGPUVertexBufferLayout vbl{};
    WGPUVertexBufferLayout vbls[2]{};
    std::array<GpuMesh, kPrimitiveCount> meshes{};
    // Assets: glTF meshes and images uploaded on first use.
    assets::AssetStore* assets = nullptr;
    WGPUBindGroupLayout material_bgl = nullptr;
    WGPUSampler sampler = nullptr;
    WGPUSampler nearest_sampler = nullptr;   // pixel art: no filtering, no bleeding between sheet tiles
    struct GpuTexture {
        WGPUTexture texture = nullptr;
        WGPUTextureView view = nullptr;        // the texels as stored (data maps, and the interface, which paints in sRGB)
        WGPUTextureView srgb_view = nullptr;   // decoded to linear when sampled (base color and emissive maps)
    };
    GpuTexture white;
    // Cameras' textures (Camera.target), by name: what "view:<name>" samples.
    struct ViewTarget {
        GpuTexture tex;
        WGPUTextureView render_view = nullptr;
        std::uint32_t w = 0, h = 0;
    };
    std::map<std::string, ViewTarget> view_targets;
    std::string drawing_target;   // the view texture being drawn now: what it draws does not sample it
    GpuTexture flat_normal;   // (0.5, 0.5, 1): no bend
    GpuTexture soft_spot;     // a soft round white spot: particles without an image of their own
    GpuTexture sky_source;    // the sky's panorama (half floats); 1x1 black without one
    std::map<std::string, GpuTexture> textures;
    std::map<std::string, WGPUBindGroup> material_groups;  // "base|mr|normal|emissive|filter" -> group 2
    // GPU timings (render.stats.gpu): every pass begun through begin_pass writes timestamps at its
    // start and end into a query set, named by its label; at the frame's end they are resolved into
    // one of eight read-back buffers, mapped once that frame was submitted and read when it maps
    // (the GPU can be a dozen frames behind the CPU when nothing paces them, as headless).
    static constexpr std::uint32_t kTimedPasses = 96;
    WGPUQuerySet timer_set = nullptr;
    WGPUBuffer timer_resolve = nullptr;
    struct TimerRead {
        WGPUBuffer buffer = nullptr;
        std::vector<std::string> names;
        std::uint64_t submitted = 0;   // the device's submitted frames when it was filled
        int state = 0;                 // 0 free, 1 filled, 2 mapping, 3 mapped
    };
    std::array<TimerRead, 8> timer_reads{};
    std::array<WGPUPassTimestampWrites, kTimedPasses> timer_writes{};
    std::vector<std::string> timer_names;   // this frame's timed passes, in order
    std::vector<std::pair<std::string, double>> timer_last;
    double timer_last_ms = 0;
    std::uint64_t timer_last_frame = 0;
    std::uint64_t timer_reads_done = 0;   // frames whose timings came back
    GpuTexture no_splat;                                   // a 1x1 stand-in for materials that are not terrains
    GpuTexture no_layers;                                  // a 1x1, one-layer array, the same
    std::map<std::string, GpuTexture> layer_arrays;        // a terrain's layer images (paths and colours) -> their array
    struct TerrainGpu { GpuTexture splat; WGPUBindGroup group = nullptr; };
    std::map<std::string, TerrainGpu> terrain_gpu;         // a terrain mesh's path -> its splat map and material group
    struct AssetMesh {
        GpuMesh gpu;
        std::vector<assets::Submesh> submeshes;
        std::vector<Mat4> rest;   // per submesh: where its node rests, for a moving part drawn without a pose
        std::vector<Mat4> unbake; // per submesh: the inverse of its node's rest for baked geometry, so a node draws in its own space
        std::vector<std::string> node_names;   // the file's nodes by index, for MeshRenderer.node
        std::vector<assets::Material> materials;
        std::uint32_t morph_base = 0, morph_targets = 0, morph_vertices = 0;  // the asset's deltas in the morph buffer
        std::uint64_t revision = 0;   // the asset's revision uploaded (cloth: written again in place when it moves on)
        std::uint32_t vertex_count = 0;
        std::vector<assets::TerrainChunk> chunks;   // a terrain's squares, each drawn on its own at its level
    };
    std::map<std::string, AssetMesh> asset_meshes;
    // Levels of detail (docs/design/rendering.md): a mesh's vertices with fewer triangles, an index
    // buffer of their own over the mesh's vertex buffer (not owned). Key "path|ratio" or "#kind|ratio".
    struct LodMesh {
        GpuMesh gpu;
        std::vector<assets::Submesh> submeshes;
        float error = 0;
    };
    std::map<std::string, LodMesh> lod_meshes;
    // Tile layers as static meshes: key "map|layer|tile_size" -> submeshes per tileset texture.
    struct TileLayerMesh {
        GpuMesh gpu;
        struct Part { std::uint32_t first, count; std::string texture; };
        std::vector<Part> parts;
        std::int32_t index;  // position of the layer in the map's file order
        std::uint64_t revision = 0;  // the layer revision the mesh was built from
        // The layer's animated cells, a small mesh of their own rebuilt whenever a frame changes
        // (frame_key: the ids drawn now), so the static cells are built once per edit.
        GpuMesh anim;
        std::vector<Part> anim_parts;
        std::string frame_key;
        bool has_anim = false;
    };
    std::map<std::string, TileLayerMesh> tile_meshes;
    // Trails (docs/design/particles.md, Trails): each one's ribbon, made again every frame into
    // buffers kept while it lives (grown when it outgrows them).
    struct TrailMesh {
        GpuMesh gpu;
        std::uint32_t vertex_room = 0, index_room = 0;
        bool seen = false;
    };
    std::map<world::EntityId, TrailMesh> trail_meshes;
    std::uint32_t tile_rebuilds = 0;  // layer meshes rebuilt after edits, over the renderer's life
    std::uint32_t tile_frames = 0;    // animated cells rebuilt for a frame change, over the renderer's life
    std::set<std::string> failed;  // asset paths reported once
    std::vector<std::pair<std::string, std::pair<Vec3, Vec3>>> new_bounds;
    RenderStats stats;
    CameraView camera;
    Viewport viewport;   // requested
    Viewport applied;    // used by the last frame
    std::uint32_t last_width = 0, last_height = 0;
    // Render scale (docs/design/rendering.md, Render scale): the window's view drawn into these at a
    // fraction of its size, then stretched into the frame.
    RenderScaleSettings scale_settings;
    float scale_now = 1.0f;                       // the fraction drawn at (dynamic moves it)
    float window_sx = 1.0f, window_sy = 1.0f;     // window pixels per drawn pixel, last frame
    std::uint32_t scaled_w = 0, scaled_h = 0;
    WGPUTexture scaled_color = nullptr, scaled_depth = nullptr;
    WGPUTextureView scaled_color_view = nullptr, scaled_depth_view = nullptr;
    WGPUShaderModule upscale_shader = nullptr;
    WGPUBindGroupLayout upscale_bgl = nullptr;
    WGPUPipelineLayout upscale_layout = nullptr;
    WGPURenderPipeline upscale_pipeline = nullptr;
    WGPUSampler upscale_sampler = nullptr;
    WGPUSampler upscale_nearest = nullptr;
    WGPUBindGroup upscale_bg_nearest = nullptr;   // the same picture through the nearest texel (pixelated)
    WGPUBuffer upscale_params = nullptr;
    WGPUBindGroup upscale_bg = nullptr;
    std::vector<double> scale_samples;            // dynamic: the GPU's frames since the last change
    std::uint64_t scale_seen = 0;                 // the gpu_frames count last looked at

    void release_scaled() {
        if (upscale_bg) wgpuBindGroupRelease(upscale_bg);
        if (upscale_bg_nearest) wgpuBindGroupRelease(upscale_bg_nearest);
        upscale_bg_nearest = nullptr;
        if (scaled_color_view) wgpuTextureViewRelease(scaled_color_view);
        if (scaled_depth_view) wgpuTextureViewRelease(scaled_depth_view);
        if (scaled_color) wgpuTextureRelease(scaled_color);
        if (scaled_depth) wgpuTextureRelease(scaled_depth);
        upscale_bg = nullptr;
        scaled_color_view = scaled_depth_view = nullptr;
        scaled_color = scaled_depth = nullptr;
        scaled_w = scaled_h = 0;
    }

    Status ensure_scaled(std::uint32_t w, std::uint32_t h) {
        if (!upscale_pipeline) {
            POCKET_TRY(mod, device->create_shader("pocket.upscale", kUpscaleWgsl));
            upscale_shader = mod;
            WGPUBindGroupLayoutEntry e[3]{};
            e[0].binding = 0;
            e[0].visibility = WGPUShaderStage_Fragment;
            e[0].texture.sampleType = WGPUTextureSampleType_Float;
            e[0].texture.viewDimension = WGPUTextureViewDimension_2D;
            e[1].binding = 1;
            e[1].visibility = WGPUShaderStage_Fragment;
            e[1].sampler.type = WGPUSamplerBindingType_Filtering;
            e[2].binding = 2;
            e[2].visibility = WGPUShaderStage_Fragment;
            e[2].buffer.type = WGPUBufferBindingType_Uniform;
            WGPUBindGroupLayoutDescriptor bd{};
            bd.label = rhi::str("pocket.upscale");
            bd.entryCount = 3;
            bd.entries = e;
            upscale_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
            WGPUPipelineLayoutDescriptor pld{};
            pld.label = rhi::str("pocket.upscale");
            pld.bindGroupLayoutCount = 1;
            pld.bindGroupLayouts = &upscale_bgl;
            upscale_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
            WGPUColorTargetState target{};
            target.format = device->color_format();
            target.writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState fs{};
            fs.module = upscale_shader;
            fs.entryPoint = rhi::str("fs");
            fs.targetCount = 1;
            fs.targets = &target;
            WGPURenderPipelineDescriptor rpd{};
            rpd.label = rhi::str("pocket.upscale");
            rpd.layout = upscale_layout;
            rpd.vertex.module = upscale_shader;
            rpd.vertex.entryPoint = rhi::str("vs");
            rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
            rpd.multisample.count = 1;
            rpd.multisample.mask = 0xFFFFFFFF;
            rpd.fragment = &fs;
            upscale_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            if (!upscale_pipeline) return fail("gpu_pipeline_failed", "the render scale's stretch could not be created");
            WGPUSamplerDescriptor sd{};
            sd.addressModeU = sd.addressModeV = sd.addressModeW = WGPUAddressMode_ClampToEdge;
            sd.magFilter = sd.minFilter = WGPUFilterMode_Linear;
            sd.maxAnisotropy = 1;
            sd.lodMaxClamp = 32.0f;
            upscale_sampler = wgpuDeviceCreateSampler(device->device(), &sd);
            sd.magFilter = sd.minFilter = WGPUFilterMode_Nearest;
            upscale_nearest = wgpuDeviceCreateSampler(device->device(), &sd);
            upscale_params = device->create_buffer("pocket.upscale", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(float) * 4);
        }
        if (scaled_w == w && scaled_h == h && scaled_color) return {};
        release_scaled();
        auto [ct, cv] = make_target("pocket.scaled.color", w, h, device->color_format(), WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopySrc | WGPUTextureUsage_CopyDst);
        auto [dt, dv] = make_target("pocket.scaled.depth", w, h, device->depth_format(), WGPUTextureUsage_RenderAttachment);
        if (!ct || !dt) return fail("gpu_texture_failed", "cannot create the scaled view {}x{}", w, h);
        scaled_color = ct;
        scaled_color_view = cv;
        scaled_depth = dt;
        scaled_depth_view = dv;
        scaled_w = w;
        scaled_h = h;
        WGPUBindGroupEntry be[3]{};
        be[0].binding = 0;
        be[0].textureView = scaled_color_view;
        be[1].binding = 1;
        be[1].sampler = upscale_sampler;
        be[2].binding = 2;
        be[2].buffer = upscale_params;
        be[2].size = sizeof(float) * 4;
        WGPUBindGroupDescriptor bgd{};
        bgd.label = rhi::str("pocket.upscale");
        bgd.layout = upscale_bgl;
        bgd.entryCount = 3;
        bgd.entries = be;
        upscale_bg = wgpuDeviceCreateBindGroup(device->device(), &bgd);
        be[1].sampler = upscale_nearest;
        upscale_bg_nearest = wgpuDeviceCreateBindGroup(device->device(), &bgd);
        return {};
    }

    // Dynamic scale: every half second of frames timed, the fraction that brings the GPU's frame to
    // its target (the GPU's time going with the pixels drawn, so with the square of the fraction),
    // down at once when over, up a step at a time when well under, in steps of a twentieth.
    void steer_scale() {
        const RenderScaleSettings& s = scale_settings;
        const float most = std::clamp(s.scale, 0.25f, 1.0f);
        if (!s.dynamic) {
            scale_now = most;
            return;
        }
        if (stats.gpu_frames > scale_seen && stats.gpu_ms > 0) {
            scale_seen = stats.gpu_frames;
            scale_samples.push_back(stats.gpu_ms);
        }
        if (scale_samples.size() < 30) return;
        std::vector<double> v = scale_samples;
        scale_samples.clear();
        std::nth_element(v.begin(), v.begin() + static_cast<std::ptrdiff_t>(v.size() / 2), v.end());
        const double ms = v[v.size() / 2];
        const float least = std::clamp(s.least, 0.25f, most);
        float next = scale_now;
        if (ms > s.target_ms) next = scale_now * static_cast<float>(std::sqrt(s.target_ms / ms)) * 0.95f;
        else if (ms < s.target_ms * 0.6) next = scale_now + 0.05f;
        next = std::clamp(std::round(next * 20.0f) / 20.0f, least, most);
        scale_now = next;
    }
    std::vector<std::uint8_t> object_staging;

    void release_texture(GpuTexture& t) {
        if (t.srgb_view) wgpuTextureViewRelease(t.srgb_view);
        if (t.view) wgpuTextureViewRelease(t.view);
        if (t.texture) wgpuTextureRelease(t.texture);
        t = GpuTexture{};
    }

    void release_assets() {
        for (auto& [path, am] : asset_meshes) {
            if (am.gpu.vertices) wgpuBufferRelease(am.gpu.vertices);
            if (am.gpu.indices) wgpuBufferRelease(am.gpu.indices);
            if (am.gpu.skin) wgpuBufferRelease(am.gpu.skin);
        }
        for (auto& [key, tm] : tile_meshes) {
            if (tm.gpu.vertices) wgpuBufferRelease(tm.gpu.vertices);
            if (tm.gpu.indices) wgpuBufferRelease(tm.gpu.indices);
            if (tm.anim.vertices) wgpuBufferRelease(tm.anim.vertices);
            if (tm.anim.indices) wgpuBufferRelease(tm.anim.indices);
        }
        tile_meshes.clear();
        for (auto& [key, lm] : lod_meshes) if (lm.gpu.indices && !key.starts_with("#")) wgpuBufferRelease(lm.gpu.indices);
        for (auto it = lod_meshes.begin(); it != lod_meshes.end();) it = it->first.starts_with("#") ? std::next(it) : lod_meshes.erase(it);   // a primitive's stay with it
        asset_meshes.clear();
        for (auto& [path, t] : textures) release_texture(t);
        textures.clear();
        for (auto& [key, bg] : material_groups) wgpuBindGroupRelease(bg);
        material_groups.clear();
        for (auto& [key, tg] : terrain_gpu) {
            if (tg.group) wgpuBindGroupRelease(tg.group);
            release_texture(tg.splat);
        }
        terrain_gpu.clear();
        for (auto& [key, t] : layer_arrays) release_texture(t);
        layer_arrays.clear();
        failed.clear();
    }

    ~Impl() {
        // A timing read-back still mapping names its slot: let it come in (or fail) first.
        for (int i = 0; i < 10000 && device && std::any_of(timer_reads.begin(), timer_reads.end(), [](const TimerRead& r) { return r.state == 2; }); ++i) device->poll(true);
        for (auto& r : timer_reads) {
            if (r.state == 3) wgpuBufferUnmap(r.buffer);
            if (r.buffer) wgpuBufferRelease(r.buffer);
        }
        for (WGPUTextureView v : {occ_view, occ_stub_view}) if (v) wgpuTextureViewRelease(v);
        for (WGPUTexture t : {occ_texture, occ_stub}) if (t) wgpuTextureRelease(t);
        for (auto& [name, vt] : view_targets) {
            for (WGPUTextureView v : {vt.tex.view, vt.tex.srgb_view, vt.render_view}) if (v) wgpuTextureViewRelease(v);
            if (vt.tex.texture) wgpuTextureRelease(vt.tex.texture);
        }
        if (timer_resolve) wgpuBufferRelease(timer_resolve);
        if (timer_set) wgpuQuerySetRelease(timer_set);
        release_assets();
        for (GpuTexture* t : {&white, &flat_normal, &soft_spot, &no_splat, &no_layers}) release_texture(*t);
        if (sampler) wgpuSamplerRelease(sampler);
        if (nearest_sampler) wgpuSamplerRelease(nearest_sampler);
        if (material_bgl) wgpuBindGroupLayoutRelease(material_bgl);
        for (auto& m : meshes) {
            if (m.vertices) wgpuBufferRelease(m.vertices);
            if (m.indices) wgpuBufferRelease(m.indices);
        }
        if (id_view) wgpuTextureViewRelease(id_view);
        if (id_texture) wgpuTextureRelease(id_texture);
        release_msaa_targets();
        release_hdr_target();
        release_sky();
        release_ao_targets();
        for (WGPURenderPipeline* p : {&ao_pipeline, &ao_blur_pipeline}) if (*p) wgpuRenderPipelineRelease(*p);
        if (ao_layout) wgpuPipelineLayoutRelease(ao_layout);
        if (ao_bgl) wgpuBindGroupLayoutRelease(ao_bgl);
        if (ao_shader) wgpuShaderModuleRelease(ao_shader);
        if (ao_uniforms) wgpuBufferRelease(ao_uniforms);
        if (gi_uniforms) wgpuBufferRelease(gi_uniforms);
        for (WGPURenderPipeline gp : {gi_pipeline, gi_add_pipeline}) if (gp) wgpuRenderPipelineRelease(gp);
        if (gi_layout) wgpuPipelineLayoutRelease(gi_layout);
        if (gi_bgl) wgpuBindGroupLayoutRelease(gi_bgl);
        for (WGPUBindGroup g : {gi_bgs[0], gi_bgs[1], gi_add_bgs[0], gi_add_bgs[1]}) if (g) wgpuBindGroupRelease(g);
        for (WGPUTextureView v : gi_view) if (v) wgpuTextureViewRelease(v);
        for (WGPUTexture t : gi_tex) if (t) wgpuTextureRelease(t);
        if (volume_uniforms) wgpuBufferRelease(volume_uniforms);
        if (volume_pipeline) wgpuRenderPipelineRelease(volume_pipeline);
        if (volume_layout) wgpuPipelineLayoutRelease(volume_layout);
        if (volume_bgl) wgpuBindGroupLayoutRelease(volume_bgl);
        for (WGPUTextureView v : {volume_view[0], volume_view[1], volume_none_view}) if (v) wgpuTextureViewRelease(v);
        for (WGPUTexture t : {volume_tex[0], volume_tex[1], volume_none_tex}) if (t) wgpuTextureRelease(t);
        for (WGPUBindGroup g : volume_bgs) if (g) wgpuBindGroupRelease(g);
        if (volume_samp) wgpuSamplerRelease(volume_samp);
        if (ao_white_view) wgpuTextureViewRelease(ao_white_view);
        if (ao_white_tex) wgpuTextureRelease(ao_white_tex);
        release_scaled();
        if (upscale_pipeline) wgpuRenderPipelineRelease(upscale_pipeline);
        if (upscale_layout) wgpuPipelineLayoutRelease(upscale_layout);
        if (upscale_bgl) wgpuBindGroupLayoutRelease(upscale_bgl);
        if (upscale_shader) wgpuShaderModuleRelease(upscale_shader);
        if (upscale_sampler) wgpuSamplerRelease(upscale_sampler);
        if (upscale_nearest) wgpuSamplerRelease(upscale_nearest);
        if (upscale_params) wgpuBufferRelease(upscale_params);
        if (prepass_view) wgpuTextureViewRelease(prepass_view);
        if (prepass_tex) wgpuTextureRelease(prepass_tex);
        if (velocity_view) wgpuTextureViewRelease(velocity_view);
        if (velocity_tex) wgpuTextureRelease(velocity_tex);
        for (WGPUTextureView tv : {surface_view, albedo_view}) if (tv) wgpuTextureViewRelease(tv);
        for (WGPUTexture tt : {surface_tex, albedo_tex}) if (tt) wgpuTextureRelease(tt);
        if (ssr_bg) wgpuBindGroupRelease(ssr_bg);
        if (ssr_uniforms) wgpuBufferRelease(ssr_uniforms);
        if (ssr_pipeline) wgpuRenderPipelineRelease(ssr_pipeline);
        if (ssr_layout) wgpuPipelineLayoutRelease(ssr_layout);
        if (ssr_bgl) wgpuBindGroupLayoutRelease(ssr_bgl);
        release_water_targets();
        for (WGPURenderPipeline p : {water_pipeline[0], water_pipeline[1], water_under_pipeline[0], water_under_pipeline[1], water_depth_pipeline}) if (p) wgpuRenderPipelineRelease(p);
        if (water_uniforms) wgpuBufferRelease(water_uniforms);
        if (decal_buffer) wgpuBufferRelease(decal_buffer);
        if (decal_view) wgpuTextureViewRelease(decal_view);
        if (decal_tex) wgpuTextureRelease(decal_tex);
        if (decal_sampler) wgpuSamplerRelease(decal_sampler);
        if (water_sampler) wgpuSamplerRelease(water_sampler);
        if (water_layout) wgpuPipelineLayoutRelease(water_layout);
        if (water_bgl) wgpuBindGroupLayoutRelease(water_bgl);
        for (int k = 0; k < 2; ++k) {
            if (taa_bg[k]) wgpuBindGroupRelease(taa_bg[k]);
            if (taa_view[k]) wgpuTextureViewRelease(taa_view[k]);
            if (taa_tex[k]) wgpuTextureRelease(taa_tex[k]);
        }
        if (taa_uniforms) wgpuBufferRelease(taa_uniforms);
        if (taa_pipeline) wgpuRenderPipelineRelease(taa_pipeline);
        if (taa_layout) wgpuPipelineLayoutRelease(taa_layout);
        if (taa_bgl) wgpuBindGroupLayoutRelease(taa_bgl);
        if (taa_shader) wgpuShaderModuleRelease(taa_shader);
        if (fx_bg) wgpuBindGroupRelease(fx_bg);
        if (fx_view) wgpuTextureViewRelease(fx_view);
        if (fx_tex) wgpuTextureRelease(fx_tex);
        if (fx_uniforms) wgpuBufferRelease(fx_uniforms);
        if (dof_pipeline) wgpuRenderPipelineRelease(dof_pipeline);
        if (blur_pipeline) wgpuRenderPipelineRelease(blur_pipeline);
        if (fx_layout) wgpuPipelineLayoutRelease(fx_layout);
        if (fx_bgl) wgpuBindGroupLayoutRelease(fx_bgl);
        if (fx_shader) wgpuShaderModuleRelease(fx_shader);
        if (no_depth_view) wgpuTextureViewRelease(no_depth_view);
        if (no_depth_tex) wgpuTextureRelease(no_depth_tex);
        release_user_effects();
        release_sprite_materials();
        for (int k = 0; k < 2; ++k) {
            if (user_fx_view[k]) wgpuTextureViewRelease(user_fx_view[k]);
            if (user_fx_tex[k]) wgpuTextureRelease(user_fx_tex[k]);
        }
        if (user_fx_sampler) wgpuSamplerRelease(user_fx_sampler);
        if (user_fx_layout) wgpuPipelineLayoutRelease(user_fx_layout);
        if (user_fx_bgl) wgpuBindGroupLayoutRelease(user_fx_bgl);
        if (post_uniforms) wgpuBufferRelease(post_uniforms);
        if (post_pipeline) wgpuRenderPipelineRelease(post_pipeline);
        if (post_layout) wgpuPipelineLayoutRelease(post_layout);
        if (post_bgl) wgpuBindGroupLayoutRelease(post_bgl);
        if (post_shader) wgpuShaderModuleRelease(post_shader);
        if (meter_uniforms) wgpuBufferRelease(meter_uniforms);
        if (meter_state) wgpuBufferRelease(meter_state);
        if (meter_pipeline) wgpuComputePipelineRelease(meter_pipeline);
        if (meter_layout) wgpuPipelineLayoutRelease(meter_layout);
        if (meter_bgl) wgpuBindGroupLayoutRelease(meter_bgl);
        if (meter_shader) wgpuShaderModuleRelease(meter_shader);
        release_bloom_targets();
        if (bloom_uniforms) wgpuBufferRelease(bloom_uniforms);
        if (bloom_sampler) wgpuSamplerRelease(bloom_sampler);
        for (WGPURenderPipeline* p : {&bloom_bright_pipeline, &bloom_blur_pipeline, &bloom_add_pipeline}) if (*p) wgpuRenderPipelineRelease(*p);
        if (bloom_layout) wgpuPipelineLayoutRelease(bloom_layout);
        if (bloom_bgl) wgpuBindGroupLayoutRelease(bloom_bgl);
        if (bloom_shader) wgpuShaderModuleRelease(bloom_shader);
        if (id_pipeline) wgpuRenderPipelineRelease(id_pipeline);
        if (id_skinned_pipeline) wgpuRenderPipelineRelease(id_skinned_pipeline);
        if (id_sprite_pipeline) wgpuRenderPipelineRelease(id_sprite_pipeline);
        for (WGPURenderPipeline p : {cut_pipeline, cut_skinned_pipeline, id_cut_pipeline, id_cut_skinned_pipeline}) if (p) wgpuRenderPipelineRelease(p);
        for (WGPUTextureView v : {glass_view, glass_stub_view}) if (v) wgpuTextureViewRelease(v);
        for (WGPUTexture t : {glass_tex, glass_stub}) if (t) wgpuTextureRelease(t);
        if (object_bg) wgpuBindGroupRelease(object_bg);
        if (frame_bg) wgpuBindGroupRelease(frame_bg);
        if (object_buffer) wgpuBufferRelease(object_buffer);
        if (frame_buffer) wgpuBufferRelease(frame_buffer);
        if (light_buffer) wgpuBufferRelease(light_buffer);
        if (cluster_buffer) wgpuBufferRelease(cluster_buffer);
        if (face_bg) wgpuBindGroupRelease(face_bg);
        if (face_slots) wgpuBufferRelease(face_slots);
        if (face_buffer) wgpuBufferRelease(face_buffer);
        if (atlas_view) wgpuTextureViewRelease(atlas_view);
        if (atlas_texture) wgpuTextureRelease(atlas_texture);
        if (atlas_stub_view) wgpuTextureViewRelease(atlas_stub_view);
        if (atlas_stub) wgpuTextureRelease(atlas_stub);
        if (shadow_pipeline) wgpuRenderPipelineRelease(shadow_pipeline);
        if (shadow_layout) wgpuPipelineLayoutRelease(shadow_layout);
        if (scene_bg) wgpuBindGroupRelease(scene_bg);
        if (scene_bgl) wgpuBindGroupLayoutRelease(scene_bgl);
        if (shadow_view) wgpuTextureViewRelease(shadow_view);
        for (WGPUTextureView v : cascade_view) if (v) wgpuTextureViewRelease(v);
        if (cascade_bg) wgpuBindGroupRelease(cascade_bg);
        if (cascade_buffer) wgpuBufferRelease(cascade_buffer);
        if (cascade_bgl) wgpuBindGroupLayoutRelease(cascade_bgl);
        if (shadow_texture) wgpuTextureRelease(shadow_texture);
        if (shadow_sampler) wgpuSamplerRelease(shadow_sampler);
        if (sprite_pipeline) wgpuRenderPipelineRelease(sprite_pipeline);
        if (sprite_add_pipeline) wgpuRenderPipelineRelease(sprite_add_pipeline);
        if (sprite_lit_pipeline) wgpuRenderPipelineRelease(sprite_lit_pipeline);
        for (auto& [id, ge] : gpu_emitters) ge.release();
        if (particle_pipeline) wgpuRenderPipelineRelease(particle_pipeline);
        if (particle_add_pipeline) wgpuRenderPipelineRelease(particle_add_pipeline);
        if (weather_pipeline) wgpuRenderPipelineRelease(weather_pipeline);
        if (particle_layout) wgpuPipelineLayoutRelease(particle_layout);
        if (particle_draw_bgl) wgpuBindGroupLayoutRelease(particle_draw_bgl);
        if (particle_sim_pipeline) wgpuComputePipelineRelease(particle_sim_pipeline);
        if (particle_sim_layout) wgpuPipelineLayoutRelease(particle_sim_layout);
        if (particle_sim_bgl) wgpuBindGroupLayoutRelease(particle_sim_bgl);
        if (particle_sim_shader) wgpuShaderModuleRelease(particle_sim_shader);
        if (line_pipeline) wgpuRenderPipelineRelease(line_pipeline);
        if (line_layout) wgpuPipelineLayoutRelease(line_layout);
        if (line_shader) wgpuShaderModuleRelease(line_shader);
        if (line_buffer) wgpuBufferRelease(line_buffer);
        if (pipeline) wgpuRenderPipelineRelease(pipeline);
        if (skinned_pipeline) wgpuRenderPipelineRelease(skinned_pipeline);
        if (blend_pipeline) wgpuRenderPipelineRelease(blend_pipeline);
        if (blend_skinned_pipeline) wgpuRenderPipelineRelease(blend_skinned_pipeline);
        if (shadow_skinned_pipeline) wgpuRenderPipelineRelease(shadow_skinned_pipeline);
        if (shadow_cut_pipeline) wgpuRenderPipelineRelease(shadow_cut_pipeline);
        for (WGPURenderPipeline pl : {atlas_pipeline, atlas_skinned_pipeline, atlas_cut_pipeline}) if (pl) wgpuRenderPipelineRelease(pl);
        for (WGPURenderPipeline p : {oit_pipeline, oit_skinned_pipeline, oit_composite_pipeline, oit_ms_pipeline, oit_ms_skinned_pipeline, oit_ms_composite_pipeline}) if (p) wgpuRenderPipelineRelease(p);
        if (oit_bg) wgpuBindGroupRelease(oit_bg);
        for (WGPUTextureView v : {oit_accum_view, oit_reveal_view, oit_ms_accum_view, oit_ms_reveal_view}) if (v) wgpuTextureViewRelease(v);
        for (WGPUTexture t : {oit_accum_tex, oit_reveal_tex, oit_ms_accum_tex, oit_ms_reveal_tex}) if (t) wgpuTextureRelease(t);
        if (oit_layout) wgpuPipelineLayoutRelease(oit_layout);
        if (oit_bgl) wgpuBindGroupLayoutRelease(oit_bgl);
        if (oit_shader) wgpuShaderModuleRelease(oit_shader);
        for (WGPUBindGroup g : probe_groups) wgpuBindGroupRelease(g);
        for (WGPURenderPipeline p : {probe_pipeline, probe_skinned_pipeline, probe_sky_pipeline}) if (p) wgpuRenderPipelineRelease(p);
        if (probe_fill_pipeline) wgpuComputePipelineRelease(probe_fill_pipeline);
        if (probe_fill_layout) wgpuPipelineLayoutRelease(probe_fill_layout);
        if (probe_fill_bgl) wgpuBindGroupLayoutRelease(probe_fill_bgl);
        if (probe_fill_shader) wgpuShaderModuleRelease(probe_fill_shader);
        for (WGPUBuffer b : {probe_fill_params, probe_prefilter_params, probe_sh_buffer, probe_cluster_buffer}) if (b) wgpuBufferRelease(b);
        for (WGPUBuffer b : probe_frame_buf) if (b) wgpuBufferRelease(b);
        for (auto& bufs : grid_frame_buf) for (WGPUBuffer b : bufs) if (b) wgpuBufferRelease(b);
        for (WGPUBuffer b : grid_params) if (b) wgpuBufferRelease(b);
        if (grid_cluster_buffer) wgpuBufferRelease(grid_cluster_buffer);
        for (WGPUBindGroup g : grid_groups) wgpuBindGroupRelease(g);
        if (grid_pipeline) wgpuComputePipelineRelease(grid_pipeline);
        if (grid_layout) wgpuPipelineLayoutRelease(grid_layout);
        if (grid_bgl) wgpuBindGroupLayoutRelease(grid_bgl);
        if (grid_shader) wgpuShaderModuleRelease(grid_shader);
        for (auto& layer : probe_level) for (WGPUTextureView v : layer) if (v) wgpuTextureViewRelease(v);
        for (WGPUTextureView v : probe_view) if (v) wgpuTextureViewRelease(v);
        for (WGPUTextureView v : {probe_env_view, probe_views_array, probe_depth_array}) if (v) wgpuTextureViewRelease(v);
        for (WGPUTextureView v : probe_depth_face) if (v) wgpuTextureViewRelease(v);
        for (WGPUTexture t : {probe_env_tex, probe_views_tex, probe_depth_tex}) if (t) wgpuTextureRelease(t);
        if (shadow_cut_layout) wgpuPipelineLayoutRelease(shadow_cut_layout);
        if (joint_buffer) wgpuBufferRelease(joint_buffer);
        if (morph_buffer) wgpuBufferRelease(morph_buffer);
        if (layout) wgpuPipelineLayoutRelease(layout);
        if (object_bgl) wgpuBindGroupLayoutRelease(object_bgl);
        if (frame_bgl) wgpuBindGroupLayoutRelease(frame_bgl);
        if (shader) wgpuShaderModuleRelease(shader);
    }

    void release_msaa_targets() {
        if (ms_color_view) wgpuTextureViewRelease(ms_color_view);
        if (ms_color) wgpuTextureRelease(ms_color);
        if (ms_depth_view) wgpuTextureViewRelease(ms_depth_view);
        if (ms_depth) wgpuTextureRelease(ms_depth);
        ms_color_view = nullptr;
        ms_color = nullptr;
        ms_depth_view = nullptr;
        ms_depth = nullptr;
        ms_width = ms_height = 0;
        ms_samples = 1;
    }

    Status ensure_msaa_targets(std::uint32_t w, std::uint32_t h, int samples) {
        if (samples <= 1) {
            release_msaa_targets();
            return {};
        }
        if (ms_color && ms_width == w && ms_height == h && ms_samples == samples) return {};
        release_msaa_targets();
        auto make = [&](const char* label, WGPUTextureFormat format, WGPUTexture& tex, WGPUTextureView& view) -> Status {
            WGPUTextureDescriptor td{};
            td.label = rhi::str(label);
            td.usage = WGPUTextureUsage_RenderAttachment;
            td.dimension = WGPUTextureDimension_2D;
            td.size = {w, h, 1};
            td.format = format;
            td.mipLevelCount = 1;
            td.sampleCount = static_cast<std::uint32_t>(samples);
            tex = wgpuDeviceCreateTexture(device->device(), &td);
            if (!tex) return fail("gpu_texture_failed", "cannot create {} ({} samples)", label, samples);
            WGPUTextureViewDescriptor vd{};
            vd.format = format;
            vd.dimension = WGPUTextureViewDimension_2D;
            vd.mipLevelCount = 1;
            vd.arrayLayerCount = 1;
            vd.aspect = WGPUTextureAspect_All;
            vd.usage = td.usage;
            view = wgpuTextureCreateView(tex, &vd);
            return {};
        };
        POCKET_TRY_VOID(make("pocket.msaa.color", kHdrFormat, ms_color, ms_color_view));
        POCKET_TRY_VOID(make("pocket.msaa.depth", device->depth_format(), ms_depth, ms_depth_view));
        ms_width = w;
        ms_height = h;
        ms_samples = samples;
        return {};
    }

    struct BloomUniforms {
        float texel[2];      // one texel of the source, in uv
        float dir[2];        // the blur's direction (across or down)
        float threshold, strength, radius, pad;
    };
    static constexpr std::uint32_t kBloomSlot = 256;   // the uniform buffer's offset alignment

    void release_bloom_targets() {
        for (WGPUBindGroup& g : bloom_bg) { if (g) wgpuBindGroupRelease(g); g = nullptr; }
        for (int i = 0; i < 2; ++i) {
            if (bloom_view[i]) wgpuTextureViewRelease(bloom_view[i]);
            if (bloom_tex[i]) wgpuTextureRelease(bloom_tex[i]);
            bloom_view[i] = nullptr;
            bloom_tex[i] = nullptr;
        }
        bloom_src = nullptr;
        bloom_w = bloom_h = 0;
    }

    Status create_bloom() {
        POCKET_TRY(module, device->create_shader("pocket.bloom", kBloomWgsl));
        bloom_shader = module;
        WGPUBindGroupLayoutEntry be[3]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(BloomUniforms);
        be[1].binding = 1;
        be[1].visibility = WGPUShaderStage_Fragment;
        be[1].texture.sampleType = WGPUTextureSampleType_Float;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 2;
        be[2].visibility = WGPUShaderStage_Fragment;
        be[2].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.bloom");
        bd.entryCount = 3;
        bd.entries = be;
        bloom_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.bloom");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &bloom_bgl;
        bloom_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        auto make = [&](const char* label, const char* entry, bool additive) -> Result<WGPURenderPipeline> {
            WGPUBlendState blend{};
            blend.color.operation = WGPUBlendOperation_Add;
            blend.color.srcFactor = WGPUBlendFactor_One;
            blend.color.dstFactor = WGPUBlendFactor_One;
            blend.alpha.operation = WGPUBlendOperation_Add;
            blend.alpha.srcFactor = WGPUBlendFactor_Zero;
            blend.alpha.dstFactor = WGPUBlendFactor_One;
            WGPUColorTargetState ct{};
            ct.format = kHdrFormat;
            ct.blend = additive ? &blend : nullptr;
            ct.writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState fs{};
            fs.module = bloom_shader;
            fs.entryPoint = rhi::str(entry);
            fs.targetCount = 1;
            fs.targets = &ct;
            WGPURenderPipelineDescriptor rpd{};
            rpd.label = rhi::str(label);
            rpd.layout = bloom_layout;
            rpd.vertex.module = bloom_shader;
            rpd.vertex.entryPoint = rhi::str("vs_screen");
            rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
            rpd.primitive.frontFace = WGPUFrontFace_CCW;
            rpd.primitive.cullMode = WGPUCullMode_None;
            rpd.multisample.count = 1;
            rpd.multisample.mask = 0xFFFFFFFFu;
            rpd.fragment = &fs;
            WGPURenderPipeline p = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            if (!p) return fail("gpu_pipeline_failed", "{} pipeline creation failed", label);
            return p;
        };
        POCKET_TRY(bright, make("pocket.bloom.bright", "fs_bright", false));
        bloom_bright_pipeline = bright;
        POCKET_TRY(blur, make("pocket.bloom.blur", "fs_blur", false));
        bloom_blur_pipeline = blur;
        POCKET_TRY(add, make("pocket.bloom.add", "fs_add", true));
        bloom_add_pipeline = add;
        WGPUSamplerDescriptor sd{};
        sd.label = rhi::str("pocket.bloom");
        sd.addressModeU = WGPUAddressMode_ClampToEdge;
        sd.addressModeV = WGPUAddressMode_ClampToEdge;
        sd.addressModeW = WGPUAddressMode_ClampToEdge;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
        sd.lodMaxClamp = 32.0f;
        sd.maxAnisotropy = 1;
        bloom_sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        bloom_uniforms = device->create_buffer("pocket.bloom", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kBloomSlot) * 4);
        return {};
    }

    // Half-size targets for the frame's size, and the bind groups over them and the frame.
    Status ensure_bloom_targets(std::uint32_t w, std::uint32_t h, WGPUTextureView frame_view) {
        const std::uint32_t bw = std::max(1u, (w + 1) / 2), bh = std::max(1u, (h + 1) / 2);
        if (bloom_tex[0] && bloom_w == bw && bloom_h == bh && bloom_src == frame_view) return {};
        release_bloom_targets();
        for (int i = 0; i < 2; ++i) {
            WGPUTextureDescriptor td{};
            td.label = rhi::str(i == 0 ? "pocket.bloom.a" : "pocket.bloom.b");
            td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding;
            td.dimension = WGPUTextureDimension_2D;
            td.size = {bw, bh, 1};
            td.format = kHdrFormat;
            td.mipLevelCount = 1;
            td.sampleCount = 1;
            bloom_tex[i] = wgpuDeviceCreateTexture(device->device(), &td);
            if (!bloom_tex[i]) return fail("gpu_texture_failed", "cannot create the bloom target {}x{}", bw, bh);
            WGPUTextureViewDescriptor vd{};
            vd.format = td.format;
            vd.dimension = WGPUTextureViewDimension_2D;
            vd.mipLevelCount = 1;
            vd.arrayLayerCount = 1;
            vd.aspect = WGPUTextureAspect_All;
            vd.usage = td.usage;
            bloom_view[i] = wgpuTextureCreateView(bloom_tex[i], &vd);
        }
        const WGPUTextureView sources[4] = {frame_view, bloom_view[0], bloom_view[1], bloom_view[0]};
        for (int i = 0; i < 4; ++i) {
            WGPUBindGroupEntry entries[3]{};
            entries[0].binding = 0;
            entries[0].buffer = bloom_uniforms;
            entries[0].offset = static_cast<std::uint64_t>(kBloomSlot) * static_cast<std::uint64_t>(i);
            entries[0].size = sizeof(BloomUniforms);
            entries[1].binding = 1;
            entries[1].textureView = sources[i];
            entries[2].binding = 2;
            entries[2].sampler = bloom_sampler;
            WGPUBindGroupDescriptor bgd{};
            bgd.label = rhi::str("pocket.bloom");
            bgd.layout = bloom_bgl;
            bgd.entryCount = 3;
            bgd.entries = entries;
            bloom_bg[i] = wgpuDeviceCreateBindGroup(device->device(), &bgd);
        }
        bloom_src = frame_view;
        bloom_w = bw;
        bloom_h = bh;
        return {};
    }

    // The bloom passes over a finished frame: bright parts into A, blurred across into B and down
    // into A, A added onto the frame.
    Status draw_bloom(rhi::Frame& frame) {
        POCKET_TRY_VOID(ensure_bloom_targets(frame.width, frame.height, hdr_view));
        BloomUniforms u[4]{};
        const float fw = 1.0f / static_cast<float>(std::max(1u, frame.width)), fh = 1.0f / static_cast<float>(std::max(1u, frame.height));
        const float tw = 1.0f / static_cast<float>(bloom_w), th = 1.0f / static_cast<float>(bloom_h);
        const float radius = std::clamp(bloom.radius, 0.25f, 8.0f);
        u[0] = {{fw, fh}, {0, 0}, bloom.threshold, bloom.strength, radius, 0};
        u[1] = {{tw, th}, {1, 0}, bloom.threshold, bloom.strength, radius, 0};
        u[2] = {{tw, th}, {0, 1}, bloom.threshold, bloom.strength, radius, 0};
        u[3] = {{tw, th}, {0, 0}, bloom.threshold, bloom.strength, radius, 0};
        for (int i = 0; i < 4; ++i) device->write_buffer(bloom_uniforms, static_cast<std::uint64_t>(kBloomSlot) * static_cast<std::uint64_t>(i), &u[i], sizeof(BloomUniforms));
        auto pass = [&](const char* label, WGPUTextureView target, bool keep, WGPURenderPipeline pipe, WGPUBindGroup bg) {
            WGPURenderPassColorAttachment ca{};
            ca.view = target;
            ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
            ca.loadOp = keep ? WGPULoadOp_Load : WGPULoadOp_Clear;
            ca.storeOp = WGPUStoreOp_Store;
            ca.clearValue = {0, 0, 0, 1};
            WGPURenderPassDescriptor rp{};
            rp.label = rhi::str(label);
            rp.colorAttachmentCount = 1;
            rp.colorAttachments = &ca;
            WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
            wgpuRenderPassEncoderSetPipeline(enc, pipe);
            wgpuRenderPassEncoderSetBindGroup(enc, 0, bg, 0, nullptr);
            wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
            wgpuRenderPassEncoderEnd(enc);
            wgpuRenderPassEncoderRelease(enc);
            stats.draw_calls++;
        };
        pass("pocket.bloom.bright", bloom_view[0], false, bloom_bright_pipeline, bloom_bg[0]);
        pass("pocket.bloom.across", bloom_view[1], false, bloom_blur_pipeline, bloom_bg[1]);
        pass("pocket.bloom.down", bloom_view[0], false, bloom_blur_pipeline, bloom_bg[2]);
        pass("pocket.bloom.add", hdr_view, true, bloom_add_pipeline, bloom_bg[3]);
        stats.bloom = true;
        return {};
    }

    struct PostUniforms {
        float exposure;
        std::uint32_t op, auto_on, grade_on;
        float tint[4];
        float temperature, contrast, saturation, vignette;
        float viewport[4];
        float inv_view_proj[16];
        float camera[4];
        float fog_color[4];      // linear
        float fog[4];            // density, base height, falloff, start
        float fog2[4];           // the most it hides, on, volumetric
        float lut[4];            // on, strength
        float cb[4];             // colour vision on
        float cb_rows[12];       // its matrix, three rows of four
        float outline[4];        // the toon look's outlines: colour (sRGB), width in pixels (0 none)
        float outline2[4];       // their opacity; y the highlights' width in pixels, z how many
        std::uint32_t highlight_ids[kMaxHighlights];   // highlighted entities (MeshRenderer.highlight), low 32 bits
        float highlight_colors[kMaxHighlights][4];     // their colours (sRGB) and strength
    };
    struct FxUniforms {
        float reproject[16];     // last frame's view-projection times the inverse of this frame's
        float inv_view_proj[16];
        float camera[4];
        float viewport[4];
        float dof[4];            // focus, aperture, the most blur (fractions of the view's height)
        float blur[4];           // strength, samples
    };
    struct TaaUniforms {
        float reproject[16];     // last frame's view-projection times the inverse of this frame's (both unjittered)
        float params[4];         // feedback, reset
        float viewport[4];
    };
    struct VolumeUniforms {
        float medium[4];         // density, base height, falloff, anisotropy
        float albedo[4];         // the fog's color (linear), the most it hides
        float march[4];          // steps, distance, start, this frame's jitter offset
        float target_size[4];
        float history[4];        // last frame's result usable (1/0), the share of this frame
    };
    struct MeterUniforms {
        float viewport[4];
        float min_ev, max_ev, compensation, rate;
    };

    void release_hdr_target() {
        if (post_bg) wgpuBindGroupRelease(post_bg);
        if (meter_bg) wgpuBindGroupRelease(meter_bg);
        if (hdr_view) wgpuTextureViewRelease(hdr_view);
        if (hdr_tex) wgpuTextureRelease(hdr_tex);
        post_bg = nullptr;
        meter_bg = nullptr;
        hdr_view = nullptr;
        hdr_tex = nullptr;
        hdr_w = hdr_h = 0;
    }

    // The final pass (a full-screen triangle reading the HDR target texel for texel) and the meter.
    Status create_post() {
        POCKET_TRY(module, device->create_shader("pocket.post", kPostWgsl));
        post_shader = module;
        WGPUBindGroupLayoutEntry be[9]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(PostUniforms);
        be[8].binding = 8;
        be[8].visibility = WGPUShaderStage_Fragment;
        be[8].texture.sampleType = WGPUTextureSampleType_Uint;
        be[8].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[6].binding = 6;
        be[6].visibility = WGPUShaderStage_Fragment;
        be[6].texture.sampleType = WGPUTextureSampleType_Float;
        be[6].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[7].binding = 7;
        be[7].visibility = WGPUShaderStage_Fragment;
        be[7].sampler.type = WGPUSamplerBindingType_Filtering;
        be[4].binding = 4;
        be[4].visibility = WGPUShaderStage_Fragment;
        be[4].texture.sampleType = WGPUTextureSampleType_Float;
        be[4].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[5].binding = 5;
        be[5].visibility = WGPUShaderStage_Fragment;
        be[5].sampler.type = WGPUSamplerBindingType_Filtering;
        be[1].binding = 1;
        be[1].visibility = WGPUShaderStage_Fragment;
        be[1].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 2;
        be[2].visibility = WGPUShaderStage_Fragment;
        be[2].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        be[2].buffer.minBindingSize = sizeof(float) * 4;
        be[3].binding = 3;
        be[3].visibility = WGPUShaderStage_Fragment;
        be[3].texture.sampleType = WGPUTextureSampleType_Depth;
        be[3].texture.viewDimension = WGPUTextureViewDimension_2D;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.post");
        bd.entryCount = 9;
        bd.entries = be;
        post_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.post");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &post_bgl;
        post_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUColorTargetState ct{};
        ct.format = device->color_format();
        ct.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = post_shader;
        fs.entryPoint = rhi::str("fs_post");
        fs.targetCount = 1;
        fs.targets = &ct;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.post");
        rpd.layout = post_layout;
        rpd.vertex.module = post_shader;
        rpd.vertex.entryPoint = rhi::str("vs_screen");
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        post_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!post_pipeline) return fail("gpu_pipeline_failed", "pocket.post pipeline creation failed");
        post_uniforms = device->create_buffer("pocket.post", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(PostUniforms));

        POCKET_TRY(mm, device->create_shader("pocket.meter", kExposureWgsl));
        meter_shader = mm;
        WGPUBindGroupLayoutEntry me[3]{};
        me[0].binding = 0;
        me[0].visibility = WGPUShaderStage_Compute;
        me[0].buffer.type = WGPUBufferBindingType_Uniform;
        me[0].buffer.minBindingSize = sizeof(MeterUniforms);
        me[1].binding = 1;
        me[1].visibility = WGPUShaderStage_Compute;
        me[1].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        me[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        me[2].binding = 2;
        me[2].visibility = WGPUShaderStage_Compute;
        me[2].buffer.type = WGPUBufferBindingType_Storage;
        me[2].buffer.minBindingSize = sizeof(float) * 4;
        WGPUBindGroupLayoutDescriptor md{};
        md.label = rhi::str("pocket.meter");
        md.entryCount = 3;
        md.entries = me;
        meter_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &md);
        WGPUPipelineLayoutDescriptor mpld{};
        mpld.label = rhi::str("pocket.meter");
        mpld.bindGroupLayoutCount = 1;
        mpld.bindGroupLayouts = &meter_bgl;
        meter_layout = wgpuDeviceCreatePipelineLayout(device->device(), &mpld);
        WGPUComputePipelineDescriptor cpd{};
        cpd.label = rhi::str("pocket.meter");
        cpd.layout = meter_layout;
        cpd.compute.module = meter_shader;
        cpd.compute.entryPoint = rhi::str("measure");
        meter_pipeline = wgpuDeviceCreateComputePipeline(device->device(), &cpd);
        if (!meter_pipeline) return fail("gpu_pipeline_failed", "pocket.meter pipeline creation failed");
        meter_uniforms = device->create_buffer("pocket.meter", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(MeterUniforms));
        const float zero[4] = {0, 0, 0, 0};
        meter_state = device->create_buffer("pocket.meter.state", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst | WGPUBufferUsage_CopySrc, sizeof zero, zero);
        return {};
    }

    // The HDR target at the frame's size, and the bind groups that read it.
    Status ensure_hdr_target(std::uint32_t w, std::uint32_t h) {
        if (hdr_tex && hdr_w == w && hdr_h == h) return {};
        release_hdr_target();
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.hdr");
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopySrc | WGPUTextureUsage_CopyDst;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {w, h, 1};
        td.format = kHdrFormat;
        td.mipLevelCount = 1;
        td.sampleCount = 1;
        hdr_tex = wgpuDeviceCreateTexture(device->device(), &td);
        if (!hdr_tex) return fail("gpu_texture_failed", "cannot create the HDR target {}x{}", w, h);
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = 1;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        hdr_view = wgpuTextureCreateView(hdr_tex, &vd);
        auto group = [&](const char* label, WGPUBindGroupLayout layout, WGPUBuffer uniforms, std::uint64_t size) {
            WGPUBindGroupEntry entries[3]{};
            entries[0].binding = 0;
            entries[0].buffer = uniforms;
            entries[0].size = size;
            entries[1].binding = 1;
            entries[1].textureView = hdr_view;
            entries[2].binding = 2;
            entries[2].buffer = meter_state;
            entries[2].size = sizeof(float) * 4;
            WGPUBindGroupDescriptor bgd{};
            bgd.label = rhi::str(label);
            bgd.layout = layout;
            bgd.entryCount = 3;
            bgd.entries = entries;
            return wgpuDeviceCreateBindGroup(device->device(), &bgd);
        };
        meter_bg = group("pocket.meter", meter_bgl, meter_uniforms, sizeof(MeterUniforms));
        post_depth = nullptr;   // the post group is made at the frame, with the depth it reads
        hdr_w = w;
        hdr_h = h;
        return {};
    }

    // Meter the viewport of the HDR target (auto exposure only), then draw the frame from it:
    // everything outside the viewport is the clear color, as the scene pass left it before.
    // A compiler message about a shader made from the engine's code and a user's: its line numbers
    // ("wgsl:1469:52") turned into the user's own ("flash.wgsl:1:52") where they fall in the user's part.
    static std::string user_lines(const std::string& message, std::size_t prefix_lines, const std::string& name) {
        std::string out;
        std::size_t i = 0;
        while (i < message.size()) {
            const std::size_t at = message.find("wgsl:", i);
            if (at == std::string::npos) { out += message.substr(i); break; }
            out += message.substr(i, at - i);
            std::size_t j = at + 5, line = 0;
            while (j < message.size() && std::isdigit(static_cast<unsigned char>(message[j]))) line = line * 10 + static_cast<std::size_t>(message[j++] - '0');
            if (j > at + 5 && line > prefix_lines) out += std::format("{}:{}", name, line - prefix_lines);
            else out += message.substr(at, j - at);
            i = j;
        }
        return out;
    }

    void release_sprite_materials() {
        for (auto* table : {&sprite_materials, &mesh_materials}) {
            for (auto& [name, m] : *table) {
                if (m.pipe) wgpuRenderPipelineRelease(m.pipe);
                if (m.add) wgpuRenderPipelineRelease(m.add);
                if (m.module) wgpuShaderModuleRelease(m.module);
            }
            table->clear();
        }
    }

    std::string set_mesh_material(const std::string& name, const std::string& body) {
        if (auto it = mesh_materials.find(name); it != mesh_materials.end()) {
            if (it->second.pipe) wgpuRenderPipelineRelease(it->second.pipe);
            if (it->second.module) wgpuShaderModuleRelease(it->second.module);
            mesh_materials.erase(it);
        }
        const std::string head = std::string(kMeshWgsl) + R"WGSL(
// What a mesh material is given beside the engine's lit colour (docs/design/rendering.md, Materials a project writes).
struct Surface {
    base: vec4f,        // the surface's colour: texture times colour (linear), alpha
    normal: vec3f,      // the geometry's normal in the world, unit length
    position: vec3f,    // the point in the world
    view: vec3f,        // toward the eye, unit length
    uv: vec2f,
    sun_dir: vec3f,     // toward the sun, unit length
    sun_color: vec3f,   // the sun's light (linear)
    params: vec4f,      // MeshRenderer.material_params
    time: f32,          // the simulation's seconds
};
)WGSL";
        const std::string source = head + body + R"WGSL(
fn pocket_mesh_material(in: VsOut) -> vec4f {
    var s: Surface;
    s.base = textureSample(base_tex, base_samp, in.uv) * in.color;
    s.normal = normalize(in.normal);
    s.position = in.world_pos;
    s.view = normalize(frame.camera_pos.xyz - in.world_pos);
    s.uv = in.uv;
    s.sun_dir = normalize(frame.sun_dir.xyz);
    s.sun_color = frame.sun_color.rgb;
    s.params = objects[in.instance].user;
    s.time = frame.clock.x;
    let lit = shade(in, false);
    return material(lit, s);
}
@fragment fn fs_mesh_material(in: VsOut) -> FsOut {
    var out: FsOut;
    out.color = pocket_mesh_material(in);
    out.id = in.id;
    return out;
}
@fragment fn fs_mesh_material_color(in: VsOut) -> @location(0) vec4f {
    return pocket_mesh_material(in);
}
)WGSL";
        SpriteMaterial m;
        device->push_error_scope();
        auto module = device->create_shader("pocket.mesh.material", source);
        m.error = user_lines(device->pop_error_scope(), static_cast<std::size_t>(std::count(head.begin(), head.end(), '\n')), name);
        if (!module && m.error.empty()) m.error = "the material did not compile";
        if (module && m.error.empty()) m.module = *module;
        else if (module) wgpuShaderModuleRelease(*module);
        const std::string error = m.error;
        mesh_materials[name] = std::move(m);
        return error;
    }

    // A mesh material's pipeline: the opaque lit pipeline's state over its module, made again when
    // the sample count or the target layout changed.
    WGPURenderPipeline mesh_material_pipeline(SpriteMaterial& m) {
        if (!m.module) return nullptr;
        if (m.pipe && (m.samples != msaa_applied || m.split != split_applied)) {
            wgpuRenderPipelineRelease(m.pipe);
            m.pipe = nullptr;
        }
        if (!m.pipe) {
            const bool split = split_applied;
            WGPUColorTargetState targets[2]{};
            targets[0].format = kHdrFormat;
            targets[0].writeMask = WGPUColorWriteMask_All;
            targets[1].format = WGPUTextureFormat_R32Uint;
            targets[1].writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState fs{};
            fs.module = m.module;
            fs.entryPoint = rhi::str(split ? "fs_mesh_material_color" : "fs_mesh_material");
            fs.targetCount = split ? 1 : 2;
            fs.targets = targets;
            WGPUDepthStencilState ds{};
            ds.format = device->depth_format();
            ds.depthWriteEnabled = WGPUOptionalBool_True;
            ds.depthCompare = WGPUCompareFunction_Less;
            ds.stencilFront.compare = WGPUCompareFunction_Always;
            ds.stencilBack.compare = WGPUCompareFunction_Always;
            ds.stencilReadMask = 0xFFFFFFFF;
            ds.stencilWriteMask = 0xFFFFFFFF;
            WGPURenderPipelineDescriptor rpd{};
            rpd.label = rhi::str("pocket.mesh.material");
            rpd.layout = layout;
            rpd.vertex.module = m.module;
            rpd.vertex.entryPoint = rhi::str("vs");
            rpd.vertex.bufferCount = 1;
            rpd.vertex.buffers = &vbl;
            rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
            rpd.primitive.frontFace = WGPUFrontFace_CCW;
            rpd.primitive.cullMode = WGPUCullMode_Back;
            rpd.depthStencil = &ds;
            rpd.multisample.count = static_cast<std::uint32_t>(msaa_applied);
            rpd.multisample.mask = 0xFFFFFFFFu;
            rpd.fragment = &fs;
            m.pipe = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            m.samples = msaa_applied;
            m.split = split;
        }
        return m.pipe;
    }

    std::string set_sprite_material(const std::string& name, const std::string& body) {
        if (auto it = sprite_materials.find(name); it != sprite_materials.end()) {
            if (it->second.pipe) wgpuRenderPipelineRelease(it->second.pipe);
            if (it->second.add) wgpuRenderPipelineRelease(it->second.add);
            if (it->second.module) wgpuShaderModuleRelease(it->second.module);
            sprite_materials.erase(it);
        }
        const std::string head = std::string(kMeshWgsl) + "\n";
        const std::string source = head + body + R"WGSL(
// The sprite's texture at another point (its own sheet's uv), and the size of one of its texels.
fn sprite_texture(uv: vec2f) -> vec4f { return textureSampleLevel(base_tex, base_samp, uv, 0.0); }
fn texel_size() -> vec2f { return 1.0 / vec2f(textureDimensions(base_tex, 0)); }
fn pocket_sprite_material(in: VsOut) -> vec4f {
    let texel = textureSample(base_tex, base_samp, in.uv);
    return material(texel, in.color, in.uv, objects[in.instance].optics, frame.clock.x);
}
@fragment fn fs_sprite_material(in: VsOut) -> FsOut {
    let c = pocket_sprite_material(in);
    if (c.a < 0.02) { discard; }
    var out: FsOut;
    out.color = c;
    out.id = in.id;
    return out;
}
@fragment fn fs_sprite_material_color(in: VsOut) -> @location(0) vec4f {
    let c = pocket_sprite_material(in);
    if (c.a < 0.02) { discard; }
    return c;
}
)WGSL";
        SpriteMaterial m;
        device->push_error_scope();
        auto module = device->create_shader("pocket.sprite.material", source);
        m.error = user_lines(device->pop_error_scope(), static_cast<std::size_t>(std::count(head.begin(), head.end(), '\n')), name);
        if (!module && m.error.empty()) m.error = "the material did not compile";
        if (module && m.error.empty()) m.module = *module;
        else if (module) wgpuShaderModuleRelease(*module);
        const std::string error = m.error;
        sprite_materials[name] = std::move(m);
        return error;
    }

    // A material's sprite pipeline for the scene pipelines as they are now (made again when the
    // sample count or the target layout changed): the sprite pipeline's state over its module.
    WGPURenderPipeline sprite_material_pipeline(SpriteMaterial& m, bool additive) {
        if (!m.module) return nullptr;
        if (m.pipe && (m.samples != msaa_applied || m.split != split_applied)) {
            wgpuRenderPipelineRelease(m.pipe);
            if (m.add) wgpuRenderPipelineRelease(m.add);
            m.pipe = m.add = nullptr;
        }
        if (!m.pipe) {
            const bool split = split_applied;
            WGPUColorTargetState targets[2]{};
            targets[0].format = kHdrFormat;
            targets[0].writeMask = WGPUColorWriteMask_All;
            targets[1].format = WGPUTextureFormat_R32Uint;
            targets[1].writeMask = WGPUColorWriteMask_All;
            WGPUBlendState blend{};
            blend.color = {WGPUBlendOperation_Add, WGPUBlendFactor_SrcAlpha, WGPUBlendFactor_OneMinusSrcAlpha};
            blend.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_One, WGPUBlendFactor_OneMinusSrcAlpha};
            targets[0].blend = &blend;
            WGPUFragmentState fs{};
            fs.module = m.module;
            fs.entryPoint = rhi::str(split ? "fs_sprite_material_color" : "fs_sprite_material");
            fs.targetCount = split ? 1 : 2;
            fs.targets = targets;
            WGPUDepthStencilState ds{};
            ds.format = device->depth_format();
            ds.depthWriteEnabled = WGPUOptionalBool_False;
            ds.depthCompare = WGPUCompareFunction_LessEqual;
            ds.stencilFront.compare = WGPUCompareFunction_Always;
            ds.stencilBack.compare = WGPUCompareFunction_Always;
            ds.stencilReadMask = 0xFFFFFFFF;
            ds.stencilWriteMask = 0xFFFFFFFF;
            WGPURenderPipelineDescriptor rpd{};
            rpd.label = rhi::str("pocket.sprite.material");
            rpd.layout = layout;
            rpd.vertex.module = m.module;
            rpd.vertex.entryPoint = rhi::str("vs");
            rpd.vertex.bufferCount = 1;
            rpd.vertex.buffers = &vbl;
            rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
            rpd.primitive.frontFace = WGPUFrontFace_CCW;
            rpd.primitive.cullMode = WGPUCullMode_None;
            rpd.depthStencil = &ds;
            rpd.multisample.count = static_cast<std::uint32_t>(msaa_applied);
            rpd.multisample.mask = 0xFFFFFFFFu;
            rpd.fragment = &fs;
            m.pipe = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            WGPUBlendState add{};
            add.color = {WGPUBlendOperation_Add, WGPUBlendFactor_SrcAlpha, WGPUBlendFactor_One};
            add.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_Zero, WGPUBlendFactor_One};
            targets[0].blend = &add;
            m.add = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            m.samples = msaa_applied;
            m.split = split;
        }
        return additive ? m.add : m.pipe;
    }

    void release_user_effects() {
        for (UserEffect& e : user_effects) {
            for (WGPUBindGroup& g : e.bg) if (g) { wgpuBindGroupRelease(g); g = nullptr; }
            if (e.uniforms) wgpuBufferRelease(e.uniforms);
            if (e.pipeline) wgpuRenderPipelineRelease(e.pipeline);
        }
        user_effects.clear();
    }

    // A project's effect as a whole shader: its uniforms (the frame's size, the view within it,
    // the time, eight parameters), the frame so far to sample, the user's `effect`, and around it a
    // fragment stage that keeps what lies outside the view as it was.
    static std::string user_effect_source(const std::string& body) {
        return std::string(R"WGSL(
struct PocketFx { frame: vec4f, viewport: vec4f, time: vec4f, params: array<vec4f, 2> };
@group(0) @binding(0) var<uniform> fx: PocketFx;
@group(0) @binding(1) var frame_texture: texture_2d<f32>;
@group(0) @binding(2) var frame_sampler: sampler;
// The frame so far (as it will be shown: colours 0..1) at a point of the view, 0..1 across and down.
fn sample_frame(uv: vec2f) -> vec4f {
    let p = (fx.viewport.xy + clamp(uv, vec2f(0.0), vec2f(1.0)) * fx.viewport.zw) * fx.frame.zw;
    return textureSampleLevel(frame_texture, frame_sampler, p, 0.0);
}
// One of the eight parameters render.post gave.
fn param(i: u32) -> f32 { return fx.params[min(i, 7u) / 4u][min(i, 7u) % 4u]; }
// The view's size in pixels, the seconds since the effects were set, the frame's number.
fn resolution() -> vec2f { return fx.viewport.zw; }
fn time() -> f32 { return fx.time.x; }
@vertex fn pocket_fx_vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}
)WGSL") + body + R"WGSL(
@fragment fn pocket_fx_fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    if (pos.x < fx.viewport.x || pos.y < fx.viewport.y || pos.x >= fx.viewport.x + fx.viewport.z || pos.y >= fx.viewport.y + fx.viewport.w) {
        return textureLoad(frame_texture, vec2i(pos.xy), 0);
    }
    return effect((pos.xy - fx.viewport.xy) / fx.viewport.zw);
}
)WGSL";
    }

    std::vector<std::string> set_user_effects(const std::vector<Renderer::PostEffect>& defs) {
        release_user_effects();
        std::vector<std::string> errors(defs.size());
        if (!user_fx_bgl) {
            WGPUBindGroupLayoutEntry be[3]{};
            be[0].binding = 0;
            be[0].visibility = WGPUShaderStage_Fragment;
            be[0].buffer.type = WGPUBufferBindingType_Uniform;
            be[1].binding = 1;
            be[1].visibility = WGPUShaderStage_Fragment;
            be[1].texture.sampleType = WGPUTextureSampleType_Float;
            be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
            be[2].binding = 2;
            be[2].visibility = WGPUShaderStage_Fragment;
            be[2].sampler.type = WGPUSamplerBindingType_Filtering;
            WGPUBindGroupLayoutDescriptor bd{};
            bd.label = rhi::str("pocket.user_fx");
            bd.entryCount = 3;
            bd.entries = be;
            user_fx_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
            WGPUPipelineLayoutDescriptor pld{};
            pld.label = rhi::str("pocket.user_fx");
            pld.bindGroupLayoutCount = 1;
            pld.bindGroupLayouts = &user_fx_bgl;
            user_fx_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
            WGPUSamplerDescriptor sd{};
            sd.label = rhi::str("pocket.user_fx");
            sd.magFilter = WGPUFilterMode_Linear;
            sd.minFilter = WGPUFilterMode_Linear;
            sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
            sd.addressModeU = WGPUAddressMode_ClampToEdge;
            sd.addressModeV = WGPUAddressMode_ClampToEdge;
            sd.addressModeW = WGPUAddressMode_ClampToEdge;
            sd.maxAnisotropy = 1;
            user_fx_sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        }
        for (std::size_t i = 0; i < defs.size(); ++i) {
            const std::string source = user_effect_source(defs[i].wgsl);
            const std::string head = user_effect_source("");
            const std::size_t prefix = static_cast<std::size_t>(std::count(head.begin(), head.begin() + static_cast<std::ptrdiff_t>(head.find("@fragment fn pocket_fx_fs")), '\n')) - 1;
            device->push_error_scope();
            auto module = device->create_shader("pocket.user_fx", source);
            WGPURenderPipeline pipeline = nullptr;
            if (module) {
                WGPUColorTargetState ct{};
                ct.format = device->color_format();
                ct.writeMask = WGPUColorWriteMask_All;
                WGPUFragmentState fs{};
                fs.module = *module;
                fs.entryPoint = rhi::str("pocket_fx_fs");
                fs.targetCount = 1;
                fs.targets = &ct;
                WGPURenderPipelineDescriptor rpd{};
                rpd.label = rhi::str("pocket.user_fx");
                rpd.layout = user_fx_layout;
                rpd.vertex.module = *module;
                rpd.vertex.entryPoint = rhi::str("pocket_fx_vs");
                rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
                rpd.multisample.count = 1;
                rpd.multisample.mask = 0xFFFFFFFFu;
                rpd.fragment = &fs;
                pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            }
            const std::string message = user_lines(device->pop_error_scope(), prefix, defs[i].name.empty() ? "effect" : defs[i].name);
            if (module) wgpuShaderModuleRelease(*module);
            if (!message.empty() || !pipeline) {
                errors[i] = message.empty() ? "the effect did not compile" : message;
                if (pipeline) wgpuRenderPipelineRelease(pipeline);
                continue;
            }
            UserEffect e;
            e.def = defs[i];
            e.pipeline = pipeline;
            e.uniforms = device->create_buffer("pocket.user_fx", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, 80);
            user_effects.push_back(std::move(e));
        }
        user_fx_time = 0;
        return errors;
    }

    // The frame's effects, after the post pass drew into the first of the two targets: each reads
    // the one before and writes the other, the last the frame itself.
    void draw_user_effects(rhi::Frame& frame, WGPUTextureView first_input) {
        std::vector<UserEffect*> on;
        for (UserEffect& e : user_effects) if (e.def.enabled) on.push_back(&e);
        user_fx_time += time_step;
        ++user_fx_frames;
        WGPUTextureView input = first_input;
        int target = 1;
        for (std::size_t i = 0; i < on.size(); ++i) {
            UserEffect& e = *on[i];
            const bool last = i + 1 == on.size();
            const int slot = input == user_fx_view[0] ? 0 : 1;
            if (e.bg_view[slot] != input || !e.bg[slot]) {
                if (e.bg[slot]) wgpuBindGroupRelease(e.bg[slot]);
                WGPUBindGroupEntry be[3]{};
                be[0].binding = 0;
                be[0].buffer = e.uniforms;
                be[0].size = 80;
                be[1].binding = 1;
                be[1].textureView = input;
                be[2].binding = 2;
                be[2].sampler = user_fx_sampler;
                WGPUBindGroupDescriptor d{};
                d.label = rhi::str("pocket.user_fx");
                d.layout = user_fx_bgl;
                d.entryCount = 3;
                d.entries = be;
                e.bg[slot] = wgpuDeviceCreateBindGroup(device->device(), &d);
                e.bg_view[slot] = input;
            }
            float u[20] = {};
            u[0] = static_cast<float>(frame.width);
            u[1] = static_cast<float>(frame.height);
            u[2] = 1.0f / static_cast<float>(std::max(frame.width, 1u));
            u[3] = 1.0f / static_cast<float>(std::max(frame.height, 1u));
            u[4] = static_cast<float>(applied.x);
            u[5] = static_cast<float>(applied.y);
            u[6] = static_cast<float>(applied.w);
            u[7] = static_cast<float>(applied.h);
            u[8] = static_cast<float>(user_fx_time);
            u[9] = time_step;
            u[10] = static_cast<float>(user_fx_frames);
            for (int k = 0; k < 8; ++k) u[12 + k] = e.def.params[static_cast<std::size_t>(k)];
            device->write_buffer(e.uniforms, 0, u, sizeof u);
            WGPURenderPassColorAttachment ca{};
            ca.view = last ? frame.color : user_fx_view[target];
            ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
            ca.loadOp = WGPULoadOp_Clear;
            ca.storeOp = WGPUStoreOp_Store;
            WGPURenderPassDescriptor rp{};
            rp.label = rhi::str("pocket.user_fx");
            rp.colorAttachmentCount = 1;
            rp.colorAttachments = &ca;
            WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
            wgpuRenderPassEncoderSetPipeline(enc, e.pipeline);
            wgpuRenderPassEncoderSetBindGroup(enc, 0, e.bg[slot], 0, nullptr);
            wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
            wgpuRenderPassEncoderEnd(enc);
            wgpuRenderPassEncoderRelease(enc);
            input = user_fx_view[target];
            target = 1 - target;
        }
        stats.post_effects = static_cast<int>(on.size());
    }

    // The two targets the effects pass the frame through, at the frame's size.
    void ensure_user_fx_targets(std::uint32_t w, std::uint32_t h) {
        if (user_fx_tex[0] && user_fx_w == w && user_fx_h == h) return;
        for (int k = 0; k < 2; ++k) {
            if (user_fx_view[k]) wgpuTextureViewRelease(user_fx_view[k]);
            if (user_fx_tex[k]) wgpuTextureRelease(user_fx_tex[k]);
            WGPUTextureDescriptor td{};
            td.label = rhi::str("pocket.user_fx");
            td.size = {w, h, 1};
            td.mipLevelCount = 1;
            td.sampleCount = 1;
            td.dimension = WGPUTextureDimension_2D;
            td.format = device->color_format();
            td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding;
            user_fx_tex[k] = wgpuDeviceCreateTexture(device->device(), &td);
            user_fx_view[k] = wgpuTextureCreateView(user_fx_tex[k], nullptr);
        }
        for (UserEffect& e : user_effects) {
            for (int k = 0; k < 2; ++k) {
                if (e.bg[k]) wgpuBindGroupRelease(e.bg[k]);
                e.bg[k] = nullptr;
                e.bg_view[k] = nullptr;
            }
        }
        user_fx_w = w;
        user_fx_h = h;
    }

    Status draw_post(rhi::Frame& frame, rhi::Color clear, const world::Fog* fog_settings) {
        // The post group reads the prepass depth when there is one (for the fog).
        WGPUTextureView depth_view = prepass_view && split_applied ? prepass_view : no_depth_view;
        WGPUTextureView volume_now = stats.volumetric ? volume_view[volume_cur] : volume_none_view;
        // The grade's look-up table, when it names an image of the right shape.
        WGPUTextureView lut_now = white.view;
        bool lut_on = false;
        if (grade.enabled && !grade.lut.empty() && assets) {
            if (auto img = assets->image(grade.lut); img && (*img)->height >= 2 && (*img)->width == (*img)->height * (*img)->height) {
                lut_now = view_for(grade.lut, white, false);
                lut_on = lut_now != white.view;
            } else if (img) {
                report_missing(grade.lut, "a look-up table is N slices of N by N side by side (N*N wide, N high)");
            } else {
                report_missing(grade.lut, img.error().message);
            }
        }
        if (!post_bg || post_depth != depth_view || post_volume != volume_now || post_lut != lut_now || post_ids != id_view) {
            if (post_bg) wgpuBindGroupRelease(post_bg);
            WGPUBindGroupEntry e[9]{};
            e[6].binding = 6;
            e[6].textureView = lut_now;
            e[7].binding = 7;
            e[7].sampler = bloom_sampler;
            post_lut = lut_now;
            e[4].binding = 4;
            e[4].textureView = volume_now;
            e[5].binding = 5;
            e[5].sampler = bloom_sampler;
            post_volume = volume_now;
            e[0].binding = 0;
            e[0].buffer = post_uniforms;
            e[0].size = sizeof(PostUniforms);
            e[1].binding = 1;
            e[1].textureView = hdr_view;
            e[2].binding = 2;
            e[2].buffer = meter_state;
            e[2].size = sizeof(float) * 4;
            e[3].binding = 3;
            e[3].textureView = depth_view;
            e[8].binding = 8;
            e[8].textureView = id_view;
            WGPUBindGroupDescriptor d{};
            d.label = rhi::str("pocket.post");
            d.layout = post_bgl;
            d.entryCount = 9;
            d.entries = e;
            post_bg = wgpuDeviceCreateBindGroup(device->device(), &d);
            post_depth = depth_view;
            post_ids = id_view;
        }
        const bool metered = tonemap.auto_exposure;
        if (metered && !secondary) {
            MeterUniforms mu{};
            mu.viewport[0] = static_cast<float>(applied.x);
            mu.viewport[1] = static_cast<float>(applied.y);
            mu.viewport[2] = static_cast<float>(applied.w);
            mu.viewport[3] = static_cast<float>(applied.h);
            mu.min_ev = std::min(tonemap.min_ev, tonemap.max_ev);
            mu.max_ev = std::max(tonemap.min_ev, tonemap.max_ev);
            mu.compensation = tonemap.compensation;
            mu.rate = meter_reset ? 1.0f : 1.0f - std::exp(-std::max(time_step, 0.0f) * std::max(tonemap.speed, 0.0f));
            if (meter_reset) {
                const float zero[4] = {0, 0, 0, 0};
                device->write_buffer(meter_state, 0, zero, sizeof zero);
                meter_reset = false;
            }
            device->write_buffer(meter_uniforms, 0, &mu, sizeof mu);
            WGPUComputePassDescriptor cpd{};
            cpd.label = rhi::str("pocket.meter");
            WGPUComputePassEncoder cp = begin_compute(frame.encoder, cpd);
            wgpuComputePassEncoderSetPipeline(cp, meter_pipeline);
            wgpuComputePassEncoderSetBindGroup(cp, 0, meter_bg, 0, nullptr);
            wgpuComputePassEncoderDispatchWorkgroups(cp, 1, 1, 1);
            wgpuComputePassEncoderEnd(cp);
            wgpuComputePassEncoderRelease(cp);
        }
        PostUniforms u{};
        u.exposure = tonemap.exposure * (grade.enabled ? grade.exposure : 1.0f);
        Tonemap op = tonemap.op;
        if (op == Tonemap::None && grade.enabled && grade.filmic) op = Tonemap::Aces;
        u.op = static_cast<std::uint32_t>(op);
        u.auto_on = metered ? 1u : 0u;
        u.grade_on = grade.enabled ? 1u : 0u;
        u.tint[0] = decode(grade.tint.r);
        u.tint[1] = decode(grade.tint.g);
        u.tint[2] = decode(grade.tint.b);
        u.tint[3] = 1.0f;
        u.temperature = grade.temperature;
        u.contrast = grade.contrast;
        u.saturation = grade.saturation;
        u.vignette = grade.vignette;
        u.viewport[0] = static_cast<float>(applied.x);
        u.viewport[1] = static_cast<float>(applied.y);
        u.viewport[2] = static_cast<float>(applied.w);
        u.viewport[3] = static_cast<float>(applied.h);
        to_array((camera.proj * camera.view).inverse(), u.inv_view_proj);
        u.camera[0] = camera.position.x; u.camera[1] = camera.position.y; u.camera[2] = camera.position.z;
        if (fog_settings && depth_view == prepass_view) {
            u.fog_color[0] = decode(fog_settings->color.r);
            u.fog_color[1] = decode(fog_settings->color.g);
            u.fog_color[2] = decode(fog_settings->color.b);
            if (fog_from_sky) {
                // Under an atmosphere the fog is the sky's own colour around the horizon.
                u.fog_color[0] = fog_sky.x;
                u.fog_color[1] = fog_sky.y;
                u.fog_color[2] = fog_sky.z;
            }
            u.fog[0] = std::max(fog_settings->density, 0.0f);
            u.fog[1] = fog_settings->height;
            u.fog[2] = std::max(fog_settings->falloff, 0.0f);
            u.fog[3] = std::max(fog_settings->start, 0.0f);
            u.fog2[0] = std::clamp(fog_settings->max_opacity, 0.0f, 1.0f);
            u.fog2[1] = 1.0f;
            u.fog2[2] = stats.volumetric ? 1.0f : 0.0f;
        }
        u.lut[0] = lut_on ? 1.0f : 0.0f;
        u.lut[1] = std::clamp(grade.lut_strength, 0.0f, 1.0f);
        stats.lut = lut_on;
        {
            // Colour vision: Machado, Oliveira and Fernandes's (2009) matrices for a full dichromacy,
            // in linear RGB; corrected, what the eye loses is shifted into what it sees (Fidaner's
            // daltonization) as one matrix, I + S (I - D); strength mixes it with the identity.
            static const float sims[3][9] = {
                {0.152286f, 1.052583f, -0.204868f, 0.114503f, 0.786281f, 0.099216f, -0.003882f, -0.048116f, 1.051998f},    // protanopia
                {0.367322f, 0.860646f, -0.227968f, 0.280085f, 0.672501f, 0.047413f, -0.011820f, 0.042940f, 0.968881f},     // deuteranopia
                {1.255528f, -0.076749f, -0.178779f, -0.078411f, 0.930809f, 0.147602f, 0.004733f, 0.691367f, 0.303900f}};   // tritanopia
            const int mode = std::clamp(colour_vision.mode, 0, 3);
            u.cb[0] = mode > 0 ? 1.0f : 0.0f;
            float m[9] = {1, 0, 0, 0, 1, 0, 0, 0, 1};
            if (mode > 0) {
                const float* d = sims[mode - 1];
                if (colour_vision.simulate) {
                    std::copy(d, d + 9, m);
                } else {
                    const float shift_rg[9] = {0, 0, 0, 0.7f, 1, 0, 0.7f, 0, 1};
                    const float shift_b[9] = {1, 0, 0.7f, 0, 1, 0.7f, 0, 0, 0};
                    const float* s = mode == 3 ? shift_b : shift_rg;
                    float lost[9];
                    for (int k = 0; k < 9; ++k) lost[k] = (k % 4 == 0 ? 1.0f : 0.0f) - d[k];
                    for (int r = 0; r < 3; ++r)
                        for (int c = 0; c < 3; ++c) {
                            float sum = 0;
                            for (int k = 0; k < 3; ++k) sum += s[r * 3 + k] * lost[k * 3 + c];
                            m[r * 3 + c] = (r == c ? 1.0f : 0.0f) + sum;
                        }
                }
                const float t = std::clamp(colour_vision.strength, 0.0f, 1.0f);
                for (int k = 0; k < 9; ++k) m[k] = (k % 4 == 0 ? 1.0f : 0.0f) + (m[k] - (k % 4 == 0 ? 1.0f : 0.0f)) * t;
            }
            for (int r = 0; r < 3; ++r) {
                for (int c = 0; c < 3; ++c) u.cb_rows[r * 4 + c] = m[r * 3 + c];
                u.cb_rows[r * 4 + 3] = 0;
            }
            stats.colour_vision = mode;
        }
        if (toon.enabled && toon.outline > 0) {
            for (int k = 0; k < 3; ++k) u.outline[k] = std::clamp(toon.outline_color[k], 0.0f, 1.0f);
            u.outline[3] = std::clamp(toon.outline, 0.0f, 16.0f) * (secondary ? 1.0f : std::clamp(scale_now, 0.25f, 1.0f));   // window pixels, in the drawn frame's
            u.outline2[0] = std::clamp(toon.opacity, 0.0f, 1.0f);
        }
        {
            const std::size_t n = std::min(highlights.size(), kMaxHighlights);
            for (std::size_t k = 0; k < n; ++k) {
                u.highlight_ids[k] = highlights[k].first;
                std::copy(highlights[k].second.begin(), highlights[k].second.end(), u.highlight_colors[k]);
            }
            u.outline2[1] = 3.0f * (secondary ? 1.0f : std::clamp(scale_now, 0.25f, 1.0f));
            u.outline2[2] = static_cast<float>(n);
            stats.highlights = static_cast<std::uint32_t>(n);
        }
        device->write_buffer(post_uniforms, 0, &u, sizeof u);
        const bool effects = !secondary && std::any_of(user_effects.begin(), user_effects.end(), [](const UserEffect& e) { return e.def.enabled; });
        if (effects) ensure_user_fx_targets(frame.width, frame.height);
        stats.post_effects = 0;
        WGPURenderPassColorAttachment ca{};
        ca.view = effects ? user_fx_view[0] : frame.color;
        ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ca.loadOp = secondary && drawing_target.empty() ? WGPULoadOp_Load : WGPULoadOp_Clear;   // a secondary view keeps what the others drew; a view texture is its own
        ca.storeOp = WGPUStoreOp_Store;
        ca.clearValue = {clear.r, clear.g, clear.b, clear.a};
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str("pocket.post");
        rp.colorAttachmentCount = 1;
        rp.colorAttachments = &ca;
        WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
        if (applied.w != frame.width || applied.h != frame.height) {
            wgpuRenderPassEncoderSetScissorRect(enc, static_cast<std::uint32_t>(applied.x), static_cast<std::uint32_t>(applied.y), applied.w, applied.h);
        }
        wgpuRenderPassEncoderSetPipeline(enc, post_pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, post_bg, 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        if (effects) draw_user_effects(frame, user_fx_view[0]);
        stats.grade = grade.enabled;
        stats.tonemap = static_cast<int>(op);
        stats.auto_exposure = metered;
        return {};
    }

    struct SkyParams {
        float zenith[4], horizon[4], ground[4];
        float sun[4];          // toward the sun, w: a sun exists
        float sun_color[4];
        float misc[4];         // mode, rotation (radians), GGX alpha for a prefilter level, intensity
        float size[4];         // the level's width and height
        float atmo[4];         // mode 3: haze; the Weather's overcast
        float moon[4];         // mode 3 by night: toward the moon, w: up
        float moon_color[4];
    };
    static constexpr std::uint32_t kSkySlot = 256;

    void release_sky() {
        if (sky_pipeline) wgpuRenderPipelineRelease(sky_pipeline);
        sky_pipeline = nullptr;
        for (WGPUBindGroup& g : prefilter_bg) { if (g) wgpuBindGroupRelease(g); g = nullptr; }
        if (fill_bg) wgpuBindGroupRelease(fill_bg);
        if (irr_bg) wgpuBindGroupRelease(irr_bg);
        fill_bg = irr_bg = nullptr;
        for (WGPUTextureView& v : env_level) { if (v) wgpuTextureViewRelease(v); v = nullptr; }
        if (env_view) wgpuTextureViewRelease(env_view);
        if (env_tex) wgpuTextureRelease(env_tex);
        env_view = nullptr;
        env_tex = nullptr;
        release_texture(sky_source);
        if (env_sampler) wgpuSamplerRelease(env_sampler);
        if (env_params) wgpuBufferRelease(env_params);
        if (sh_buffer) wgpuBufferRelease(sh_buffer);
        for (WGPUComputePipeline* p : {&fill_pipeline, &prefilter_pipeline, &irradiance_pipeline}) { if (*p) wgpuComputePipelineRelease(*p); *p = nullptr; }
        for (WGPUPipelineLayout* l : {&env_layout, &irr_layout, &sky_layout}) { if (*l) wgpuPipelineLayoutRelease(*l); *l = nullptr; }
        for (WGPUBindGroupLayout* l : {&env_bgl, &irr_bgl}) { if (*l) wgpuBindGroupLayoutRelease(*l); *l = nullptr; }
        if (sky_shader) wgpuShaderModuleRelease(sky_shader);
        if (irr_shader) wgpuShaderModuleRelease(irr_shader);
        sky_shader = irr_shader = nullptr;
    }

    // A half-float texture from linear RGBA floats (the panorama's source).
    Result<GpuTexture> upload_half(const char* label, std::uint32_t w, std::uint32_t h, const std::vector<float>& rgba) {
        GpuTexture t;
        WGPUTextureDescriptor td{};
        td.label = rhi::str(label);
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {w, h, 1};
        td.format = kHdrFormat;
        td.mipLevelCount = 1;
        td.sampleCount = 1;
        t.texture = wgpuDeviceCreateTexture(device->device(), &td);
        if (!t.texture) return fail("gpu_texture_failed", "cannot create texture {} ({}x{})", label, w, h);
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = 1;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        t.view = wgpuTextureCreateView(t.texture, &vd);
        std::vector<std::uint16_t> half(rgba.size());
        for (std::size_t i = 0; i < rgba.size(); ++i) half[i] = to_half(rgba[i]);
        WGPUTexelCopyTextureInfo dst{};
        dst.texture = t.texture;
        dst.aspect = WGPUTextureAspect_All;
        WGPUTexelCopyBufferLayout layout{};
        layout.bytesPerRow = w * 8;
        layout.rowsPerImage = h;
        WGPUExtent3D ext{w, h, 1};
        wgpuQueueWriteTexture(device->queue(), &dst, half.data(), half.size() * sizeof(std::uint16_t), &layout, &ext);
        return t;
    }

    // The fill pass's group reads the current source (the panorama, or the black texel).
    void make_fill_group() {
        if (fill_bg) wgpuBindGroupRelease(fill_bg);
        WGPUBindGroupEntry e[4]{};
        e[0].binding = 0;
        e[0].buffer = env_params;
        e[0].offset = 0;
        e[0].size = sizeof(SkyParams);
        e[1].binding = 1;
        e[1].textureView = sky_source.view;
        e[2].binding = 2;
        e[2].sampler = env_sampler;
        e[3].binding = 3;
        e[3].textureView = env_level[0];
        WGPUBindGroupDescriptor d{};
        d.label = rhi::str("pocket.sky.fill");
        d.layout = env_bgl;
        d.entryCount = 4;
        d.entries = e;
        fill_bg = wgpuDeviceCreateBindGroup(device->device(), &d);
    }

    Status create_sky() {
        POCKET_TRY(sm, device->create_shader("pocket.sky", kSkyWgsl));
        sky_shader = sm;
        POCKET_TRY(im, device->create_shader("pocket.irradiance", kIrradianceWgsl));
        irr_shader = im;
        WGPUBindGroupLayoutEntry be[4]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Compute;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(SkyParams);
        be[1].binding = 1;
        be[1].visibility = WGPUShaderStage_Compute;
        be[1].texture.sampleType = WGPUTextureSampleType_Float;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 2;
        be[2].visibility = WGPUShaderStage_Compute;
        be[2].sampler.type = WGPUSamplerBindingType_Filtering;
        be[3].binding = 3;
        be[3].visibility = WGPUShaderStage_Compute;
        be[3].storageTexture.access = WGPUStorageTextureAccess_WriteOnly;
        be[3].storageTexture.format = kHdrFormat;
        be[3].storageTexture.viewDimension = WGPUTextureViewDimension_2D;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.sky");
        bd.entryCount = 4;
        bd.entries = be;
        env_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUBindGroupLayoutEntry ie[2]{};
        ie[0].binding = 0;
        ie[0].visibility = WGPUShaderStage_Compute;
        ie[0].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        ie[0].texture.viewDimension = WGPUTextureViewDimension_2D;
        ie[1].binding = 1;
        ie[1].visibility = WGPUShaderStage_Compute;
        ie[1].buffer.type = WGPUBufferBindingType_Storage;
        ie[1].buffer.minBindingSize = sizeof(float) * 36;
        WGPUBindGroupLayoutDescriptor id{};
        id.label = rhi::str("pocket.irradiance");
        id.entryCount = 2;
        id.entries = ie;
        irr_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &id);
        auto pipeline_layout = [&](const char* label, WGPUBindGroupLayout g) {
            WGPUPipelineLayoutDescriptor pld{};
            pld.label = rhi::str(label);
            pld.bindGroupLayoutCount = 1;
            pld.bindGroupLayouts = &g;
            return wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        };
        env_layout = pipeline_layout("pocket.sky", env_bgl);
        irr_layout = pipeline_layout("pocket.irradiance", irr_bgl);
        auto compute = [&](const char* label, WGPUPipelineLayout layout, WGPUShaderModule module, const char* entry) {
            WGPUComputePipelineDescriptor cpd{};
            cpd.label = rhi::str(label);
            cpd.layout = layout;
            cpd.compute.module = module;
            cpd.compute.entryPoint = rhi::str(entry);
            return wgpuDeviceCreateComputePipeline(device->device(), &cpd);
        };
        fill_pipeline = compute("pocket.sky.fill", env_layout, sky_shader, "fill");
        prefilter_pipeline = compute("pocket.sky.prefilter", env_layout, sky_shader, "prefilter");
        irradiance_pipeline = compute("pocket.sky.irradiance", irr_layout, irr_shader, "irradiance");
        if (!fill_pipeline || !prefilter_pipeline || !irradiance_pipeline) return fail("gpu_pipeline_failed", "sky pipelines could not be created");

        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.environment");
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_StorageBinding;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {kEnvWidth, kEnvHeight, 1};
        td.format = kHdrFormat;
        td.mipLevelCount = kEnvLevels;
        td.sampleCount = 1;
        env_tex = wgpuDeviceCreateTexture(device->device(), &td);
        if (!env_tex) return fail("gpu_texture_failed", "cannot create the environment map");
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = kEnvLevels;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = WGPUTextureUsage_TextureBinding;
        env_view = wgpuTextureCreateView(env_tex, &vd);
        for (std::uint32_t l = 0; l < kEnvLevels; ++l) {
            vd.baseMipLevel = l;
            vd.mipLevelCount = 1;
            vd.usage = WGPUTextureUsage_None;   // the texture's own usage: sampled and written
            env_level[l] = wgpuTextureCreateView(env_tex, &vd);
        }
        WGPUSamplerDescriptor sd{};
        sd.label = rhi::str("pocket.environment");
        sd.addressModeU = WGPUAddressMode_Repeat;
        sd.addressModeV = WGPUAddressMode_ClampToEdge;
        sd.addressModeW = WGPUAddressMode_ClampToEdge;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Linear;
        sd.lodMinClamp = 0;
        sd.lodMaxClamp = 32;
        sd.maxAnisotropy = 1;
        env_sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        env_params = device->create_buffer("pocket.sky", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kSkySlot) * kEnvLevels);
        std::vector<float> zero_sh(36, 0.0f);
        sh_buffer = device->create_buffer("pocket.sky.harmonics", WGPUBufferUsage_Storage | WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, zero_sh.size() * sizeof(float), zero_sh.data());
        POCKET_TRY(black, upload_half("pocket.sky.none", 1, 1, std::vector<float>{0, 0, 0, 1}));
        sky_source = black;
        make_fill_group();
        for (std::uint32_t l = 1; l < kEnvLevels; ++l) {
            WGPUBindGroupEntry e[4]{};
            e[0].binding = 0;
            e[0].buffer = env_params;
            e[0].offset = static_cast<std::uint64_t>(kSkySlot) * l;
            e[0].size = sizeof(SkyParams);
            e[1].binding = 1;
            e[1].textureView = env_level[l - 1];
            e[2].binding = 2;
            e[2].sampler = env_sampler;
            e[3].binding = 3;
            e[3].textureView = env_level[l];
            WGPUBindGroupDescriptor d{};
            d.label = rhi::str("pocket.sky.prefilter");
            d.layout = env_bgl;
            d.entryCount = 4;
            d.entries = e;
            prefilter_bg[l] = wgpuDeviceCreateBindGroup(device->device(), &d);
        }
        WGPUBindGroupEntry ie2[2]{};
        ie2[0].binding = 0;
        ie2[0].textureView = env_level[3];
        ie2[1].binding = 1;
        ie2[1].buffer = sh_buffer;
        ie2[1].size = sizeof(float) * 36;
        WGPUBindGroupDescriptor idd{};
        idd.label = rhi::str("pocket.irradiance");
        idd.layout = irr_bgl;
        idd.entryCount = 2;
        idd.entries = ie2;
        irr_bg = wgpuDeviceCreateBindGroup(device->device(), &idd);
        return {};
    }

    // Rebuild the environment when what it is made of changed: the sky's settings, its image, and
    // for a procedural sky the sun's direction and color. False when the image is unavailable.
    bool update_environment(rhi::Frame& frame, const world::Sky& sky, bool have_sun, Vec3 toward_sun, Vec3 sun_linear, Vec3 toward_moon = {}, Vec3 moon_linear = {}) {
        const bool image = sky.mode == 2;
        if (image) {
            if (sky.image.empty()) return false;
            if (sky.image != sky_source_path) {
                if (!assets) { report_missing(sky.image, "no asset store"); return false; }
                if (failed.contains(sky.image)) { note_missing(sky.image); return false; }
                auto img = assets->image(sky.image);
                if (!img) { report_missing(sky.image, img.error().message); return false; }
                const assets::Image& src = **img;
                std::vector<float> rgba(static_cast<std::size_t>(src.width) * src.height * 4);
                for (std::size_t i = 0; i < rgba.size(); ++i) rgba[i] = (i % 4 == 3) ? 1.0f : (src.hdr.empty() ? decode(src.rgba[i] / 255.0f) : src.hdr[i]);
                auto t = upload_half(sky.image.c_str(), src.width, src.height, rgba);
                if (!t) { report_missing(sky.image, t.error().message); return false; }
                release_texture(sky_source);
                sky_source = *t;
                sky_source_path = sky.image;
                make_fill_group();
                env_key.clear();
            }
        }
        char key[512];
        const bool moon = sky.mode == 3 && (moon_linear.x > 0 || moon_linear.y > 0 || moon_linear.z > 0);
        // Under a running day the sun moves every frame: the panorama follows it in steps of about a
        // fifth of a degree and of its light to three figures, finer than shows and far fewer
        // rebuilds (each is a few tenths of a millisecond).
        auto way = [](Vec3 v) { return Vec3{std::round(v.x * 256) / 256, std::round(v.y * 256) / 256, std::round(v.z * 256) / 256}; };
        const Vec3 ks = way(toward_sun), km = way(toward_moon);
        std::snprintf(key, sizeof key, "%d|%s|%g,%g,%g|%g,%g,%g|%g,%g,%g|%g|%g|%d|%g,%g,%g|%.3g,%.3g,%.3g|%g|%g,%g,%g|%.3g,%.3g,%.3g|%.2f", sky.mode, image ? sky.image.c_str() : "", sky.zenith.r, sky.zenith.g, sky.zenith.b, sky.horizon.r, sky.horizon.g, sky.horizon.b, sky.ground.r, sky.ground.g, sky.ground.b, sky.intensity, sky.rotation, have_sun && !image ? 1 : 0, ks.x, ks.y, ks.z, sun_linear.x, sun_linear.y, sun_linear.z, sky.mode == 3 ? sky.haze : 0.0f, km.x, km.y, km.z, moon_linear.x, moon_linear.y, moon_linear.z, env_overcast);
        if (env_key == key) return true;
        SkyParams p{};
        auto color = [](float* out, const world::Color4& c) { out[0] = decode(c.r); out[1] = decode(c.g); out[2] = decode(c.b); out[3] = 1; };
        color(p.zenith, sky.zenith);
        color(p.horizon, sky.horizon);
        color(p.ground, sky.ground);
        p.sun[0] = toward_sun.x; p.sun[1] = toward_sun.y; p.sun[2] = toward_sun.z; p.sun[3] = have_sun && !image ? 1.0f : 0.0f;
        p.sun_color[0] = sun_linear.x; p.sun_color[1] = sun_linear.y; p.sun_color[2] = sun_linear.z; p.sun_color[3] = 1;
        p.misc[0] = static_cast<float>(sky.mode);
        p.misc[1] = radians(sky.rotation);
        p.misc[3] = std::max(sky.intensity, 0.0f);
        p.atmo[0] = std::max(sky.haze, 0.0f);
        p.atmo[1] = env_overcast;
        if (moon) {
            p.moon[0] = toward_moon.x; p.moon[1] = toward_moon.y; p.moon[2] = toward_moon.z; p.moon[3] = 1;
            p.moon_color[0] = moon_linear.x; p.moon_color[1] = moon_linear.y; p.moon_color[2] = moon_linear.z; p.moon_color[3] = 1;
        }
        std::vector<std::uint8_t> slots(static_cast<std::size_t>(kSkySlot) * kEnvLevels, 0);
        float prev_alpha = 0;
        for (std::uint32_t l = 0; l < kEnvLevels; ++l) {
            SkyParams q = p;
            q.size[0] = static_cast<float>(std::max(1u, kEnvWidth >> l));
            q.size[1] = static_cast<float>(std::max(1u, kEnvHeight >> l));
            const float rough = static_cast<float>(l) / static_cast<float>(kEnvLevels - 1);
            const float alpha = rough * rough;
            // The level above is already blurred by its own lobe: only the difference is added here.
            q.misc[2] = std::sqrt(std::max(alpha * alpha - prev_alpha * prev_alpha, 1e-4f));
            prev_alpha = alpha;
            std::memcpy(slots.data() + static_cast<std::size_t>(kSkySlot) * l, &q, sizeof q);
        }
        device->write_buffer(env_params, 0, slots.data(), slots.size());
        WGPUComputePassDescriptor cpd{};
        cpd.label = rhi::str("pocket.sky");
        WGPUComputePassEncoder cp = begin_compute(frame.encoder, cpd);
        wgpuComputePassEncoderSetPipeline(cp, fill_pipeline);
        wgpuComputePassEncoderSetBindGroup(cp, 0, fill_bg, 0, nullptr);
        wgpuComputePassEncoderDispatchWorkgroups(cp, (kEnvWidth + 7) / 8, (kEnvHeight + 7) / 8, 1);
        wgpuComputePassEncoderSetPipeline(cp, prefilter_pipeline);
        for (std::uint32_t l = 1; l < kEnvLevels; ++l) {
            const std::uint32_t w = std::max(1u, kEnvWidth >> l), h = std::max(1u, kEnvHeight >> l);
            wgpuComputePassEncoderSetBindGroup(cp, 0, prefilter_bg[l], 0, nullptr);
            wgpuComputePassEncoderDispatchWorkgroups(cp, (w + 7) / 8, (h + 7) / 8, 1);
        }
        wgpuComputePassEncoderSetPipeline(cp, irradiance_pipeline);
        wgpuComputePassEncoderSetBindGroup(cp, 0, irr_bg, 0, nullptr);
        wgpuComputePassEncoderDispatchWorkgroups(cp, 1, 1, 1);
        wgpuComputePassEncoderEnd(cp);
        wgpuComputePassEncoderRelease(cp);
        env_key = key;
        ++env_updates;
        return true;
    }

    struct AoUniforms {
        float view_proj[16];
        float inv_view_proj[16];
        float camera[4];
        float fwd[4];
        float params[4];   // radius, intensity, samples
        float size[4];     // the full frame's width and height
        float sun[4];      // toward the sun, contact shadows on
        float contact[4];  // the contact march's length, ambient occlusion on
    };

    void release_ao_targets() {
        for (WGPUBindGroup& g : ao_bg) { if (g) wgpuBindGroupRelease(g); g = nullptr; }
        for (int i = 0; i < 2; ++i) {
            if (ao_view[i]) wgpuTextureViewRelease(ao_view[i]);
            if (ao_tex[i]) wgpuTextureRelease(ao_tex[i]);
            ao_view[i] = nullptr;
            ao_tex[i] = nullptr;
        }
        ao_w = ao_h = 0;
    }

    // A single-sample texture and its view.
    std::pair<WGPUTexture, WGPUTextureView> make_target(const char* label, std::uint32_t w, std::uint32_t h, WGPUTextureFormat format, WGPUTextureUsage usage, std::uint32_t samples = 1) {
        WGPUTextureDescriptor td{};
        td.label = rhi::str(label);
        td.usage = usage;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {w, h, 1};
        td.format = format;
        td.mipLevelCount = 1;
        td.sampleCount = samples;
        WGPUTexture t = wgpuDeviceCreateTexture(device->device(), &td);
        WGPUTextureViewDescriptor vd{};
        vd.format = format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = 1;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = usage;
        return {t, t ? wgpuTextureCreateView(t, &vd) : nullptr};
    }

    Status create_ao() {
        POCKET_TRY(module, device->create_shader("pocket.ao", kAoWgsl));
        ao_shader = module;
        WGPUBindGroupLayoutEntry be[3]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(AoUniforms);
        be[1].binding = 1;
        be[1].visibility = WGPUShaderStage_Fragment;
        be[1].texture.sampleType = WGPUTextureSampleType_Depth;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 2;
        be[2].visibility = WGPUShaderStage_Fragment;
        be[2].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        be[2].texture.viewDimension = WGPUTextureViewDimension_2D;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.ao");
        bd.entryCount = 3;
        bd.entries = be;
        ao_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.ao");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &ao_bgl;
        ao_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        auto make = [&](const char* label, const char* entry, WGPUTextureFormat format) -> WGPURenderPipeline {
            WGPUColorTargetState ct{};
            ct.format = format;
            ct.writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState fs{};
            fs.module = ao_shader;
            fs.entryPoint = rhi::str(entry);
            fs.targetCount = 1;
            fs.targets = &ct;
            WGPURenderPipelineDescriptor rpd{};
            rpd.label = rhi::str(label);
            rpd.layout = ao_layout;
            rpd.vertex.module = ao_shader;
            rpd.vertex.entryPoint = rhi::str("vs_screen");
            rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
            rpd.primitive.frontFace = WGPUFrontFace_CCW;
            rpd.primitive.cullMode = WGPUCullMode_None;
            rpd.multisample.count = 1;
            rpd.multisample.mask = 0xFFFFFFFFu;
            rpd.fragment = &fs;
            return wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        };
        ao_pipeline = make("pocket.ao", "fs_ao", kAoRawFormat);
        ao_blur_pipeline = make("pocket.ao.blur", "fs_ao_blur", WGPUTextureFormat_RG8Unorm);
        if (!ao_pipeline || !ao_blur_pipeline) return fail("gpu_pipeline_failed", "ambient occlusion pipelines could not be created");
        ao_uniforms = device->create_buffer("pocket.ao", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(AoUniforms));
        auto [wt, wv] = make_target("pocket.ao.white", 1, 1, WGPUTextureFormat_RG8Unorm, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
        ao_white_tex = wt;
        ao_white_view = wv;
        const std::uint8_t one[2] = {255, 255};
        WGPUTexelCopyTextureInfo dst{};
        dst.texture = ao_white_tex;
        dst.aspect = WGPUTextureAspect_All;
        WGPUTexelCopyBufferLayout layout{};
        layout.bytesPerRow = 2;
        layout.rowsPerImage = 1;
        WGPUExtent3D ext{1, 1, 1};
        wgpuQueueWriteTexture(device->queue(), &dst, one, 2, &layout, &ext);
        auto [dt, dv] = make_target("pocket.depth.none", 1, 1, kPrepassDepth, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_RenderAttachment);
        no_depth_tex = dt;
        no_depth_view = dv;
        return {};
    }

    // Reflection probes' textures: the panoramas (a layer each, all their mips), the six views of a
    // capture and its depth.
    Status create_probe_textures() {
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.probes");
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_StorageBinding;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {kProbeWidth, kProbeHeight, kMaxProbes};
        td.format = kHdrFormat;
        td.mipLevelCount = kProbeLevels;
        td.sampleCount = 1;
        probe_env_tex = wgpuDeviceCreateTexture(device->device(), &td);
        if (!probe_env_tex) return fail("gpu_texture_failed", "cannot create the reflection probes' panoramas");
        WGPUTextureViewDescriptor vd{};
        vd.format = kHdrFormat;
        vd.dimension = WGPUTextureViewDimension_2DArray;
        vd.mipLevelCount = kProbeLevels;
        vd.arrayLayerCount = kMaxProbes;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = WGPUTextureUsage_TextureBinding;
        probe_env_view = wgpuTextureCreateView(probe_env_tex, &vd);
        for (std::uint32_t p = 0; p < kMaxProbes; ++p)
            for (std::uint32_t l = 0; l < kProbeLevels; ++l) {
                WGPUTextureViewDescriptor lv = vd;
                lv.dimension = WGPUTextureViewDimension_2D;
                lv.baseMipLevel = l;
                lv.mipLevelCount = 1;
                lv.baseArrayLayer = p;
                lv.arrayLayerCount = 1;
                lv.usage = WGPUTextureUsage_None;   // the texture's own: sampled and written
                probe_level[p][l] = wgpuTextureCreateView(probe_env_tex, &lv);
            }
        td.label = rhi::str("pocket.probe.views");
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding;
        td.size = {kProbeFace, kProbeFace, 6};
        td.mipLevelCount = 1;
        probe_views_tex = wgpuDeviceCreateTexture(device->device(), &td);
        if (!probe_views_tex) return fail("gpu_texture_failed", "cannot create the reflection probes' views");
        vd.mipLevelCount = 1;
        vd.arrayLayerCount = 6;
        vd.usage = WGPUTextureUsage_TextureBinding;
        probe_views_array = wgpuTextureCreateView(probe_views_tex, &vd);
        for (std::uint32_t f = 0; f < 6; ++f) {
            WGPUTextureViewDescriptor fv = vd;
            fv.dimension = WGPUTextureViewDimension_2D;
            fv.baseArrayLayer = f;
            fv.arrayLayerCount = 1;
            fv.usage = WGPUTextureUsage_RenderAttachment;
            probe_view[f] = wgpuTextureCreateView(probe_views_tex, &fv);
        }
        td.label = rhi::str("pocket.probe.depth");
        td.format = kPrepassDepth;
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding;
        probe_depth_tex = wgpuDeviceCreateTexture(device->device(), &td);
        if (!probe_depth_tex) return fail("gpu_texture_failed", "cannot create the reflection probes' depth");
        WGPUTextureViewDescriptor dvd{};
        dvd.format = kPrepassDepth;
        dvd.dimension = WGPUTextureViewDimension_2DArray;
        dvd.mipLevelCount = 1;
        dvd.arrayLayerCount = 6;
        dvd.aspect = WGPUTextureAspect_DepthOnly;
        dvd.usage = WGPUTextureUsage_TextureBinding;
        probe_depth_array = wgpuTextureCreateView(probe_depth_tex, &dvd);
        for (std::uint32_t f = 0; f < 6; ++f) {
            WGPUTextureViewDescriptor fv = dvd;
            fv.dimension = WGPUTextureViewDimension_2D;
            fv.baseArrayLayer = f;
            fv.arrayLayerCount = 1;
            fv.aspect = WGPUTextureAspect_All;
            fv.usage = WGPUTextureUsage_RenderAttachment;
            probe_depth_face[f] = wgpuTextureCreateView(probe_depth_tex, &fv);
        }
        return {};
    }

    // The passes a capture runs: the scene's own shading into a probe's six views (a pipeline for
    // the views' format and depth), the sky behind, and the fill that turns the views into level 0.
    Status create_probe_passes() {
        WGPUColorTargetState ct{};
        ct.format = kHdrFormat;
        ct.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str("fs_color_cut");   // probes are drawn rarely and small: one pipeline, cut-outs included
        fs.targetCount = 1;
        fs.targets = &ct;
        WGPUDepthStencilState ds{};
        ds.format = kPrepassDepth;
        ds.depthWriteEnabled = WGPUOptionalBool_True;
        ds.depthCompare = WGPUCompareFunction_Less;
        ds.stencilFront.compare = WGPUCompareFunction_Always;
        ds.stencilBack.compare = WGPUCompareFunction_Always;
        ds.stencilReadMask = 0xFFFFFFFF;
        ds.stencilWriteMask = 0xFFFFFFFF;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.probe.mesh");
        rpd.layout = layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_Back;
        rpd.depthStencil = &ds;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        probe_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        rpd.label = rhi::str("pocket.probe.skinned");
        rpd.vertex.entryPoint = rhi::str("vs_skinned");
        rpd.vertex.bufferCount = 2;
        rpd.vertex.buffers = vbls;
        probe_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        WGPUFragmentState kfs = fs;
        kfs.entryPoint = rhi::str("fs_sky_color");
        WGPUDepthStencilState kds = ds;
        kds.depthWriteEnabled = WGPUOptionalBool_False;
        kds.depthCompare = WGPUCompareFunction_Always;
        WGPURenderPipelineDescriptor k{};
        k.label = rhi::str("pocket.probe.sky");
        k.layout = sky_layout;
        k.vertex.module = shader;
        k.vertex.entryPoint = rhi::str("vs_sky");
        k.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        k.primitive.frontFace = WGPUFrontFace_CCW;
        k.primitive.cullMode = WGPUCullMode_None;
        k.depthStencil = &kds;
        k.multisample.count = 1;
        k.multisample.mask = 0xFFFFFFFFu;
        k.fragment = &kfs;
        probe_sky_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &k);
        if (!probe_pipeline || !probe_skinned_pipeline || !probe_sky_pipeline) return fail("gpu_pipeline_failed", "reflection probe pipelines could not be created");
        POCKET_TRY(module, device->create_shader("pocket.probe.fill", kProbeFillWgsl));
        probe_fill_shader = module;
        WGPUBindGroupLayoutEntry be[4]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Compute;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(float) * (16 * 6 + 8);
        be[1].binding = 1;
        be[1].visibility = WGPUShaderStage_Compute;
        be[1].texture.sampleType = WGPUTextureSampleType_Float;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2DArray;
        be[2].binding = 2;
        be[2].visibility = WGPUShaderStage_Compute;
        be[2].sampler.type = WGPUSamplerBindingType_Filtering;
        be[3].binding = 3;
        be[3].visibility = WGPUShaderStage_Compute;
        be[3].storageTexture.access = WGPUStorageTextureAccess_WriteOnly;
        be[3].storageTexture.format = kHdrFormat;
        be[3].storageTexture.viewDimension = WGPUTextureViewDimension_2D;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.probe.fill");
        bd.entryCount = 4;
        bd.entries = be;
        probe_fill_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.probe.fill");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &probe_fill_bgl;
        probe_fill_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUComputePipelineDescriptor cpd{};
        cpd.label = rhi::str("pocket.probe.fill");
        cpd.layout = probe_fill_layout;
        cpd.compute.module = probe_fill_shader;
        cpd.compute.entryPoint = rhi::str("probe_fill");
        probe_fill_pipeline = wgpuDeviceCreateComputePipeline(device->device(), &cpd);
        if (!probe_fill_pipeline) return fail("gpu_pipeline_failed", "the reflection probe fill could not be created");
        probe_fill_params = device->create_buffer("pocket.probe.fill", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(float) * (16 * 6 + 8));
        probe_prefilter_params = device->create_buffer("pocket.probe.prefilter", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kSkySlot) * kProbeLevels);
        for (auto& b : probe_frame_buf) b = device->create_buffer("pocket.probe.frame", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(FrameUniforms));
        // Irradiance volumes: the harmonics of a probe straight from its six views.
        POCKET_TRY(gmodule, device->create_shader("pocket.grid.harmonics", kGridShWgsl));
        grid_shader = gmodule;
        WGPUBindGroupLayoutEntry ge[5]{};
        ge[0].binding = 0;
        ge[0].visibility = WGPUShaderStage_Compute;
        ge[0].buffer.type = WGPUBufferBindingType_Uniform;
        ge[0].buffer.minBindingSize = sizeof(float) * (16 * 6 + 8);
        ge[1].binding = 1;
        ge[1].visibility = WGPUShaderStage_Compute;
        ge[1].texture.sampleType = WGPUTextureSampleType_Float;
        ge[1].texture.viewDimension = WGPUTextureViewDimension_2DArray;
        ge[2].binding = 2;
        ge[2].visibility = WGPUShaderStage_Compute;
        ge[2].sampler.type = WGPUSamplerBindingType_Filtering;
        ge[3].binding = 3;
        ge[3].visibility = WGPUShaderStage_Compute;
        ge[3].buffer.type = WGPUBufferBindingType_Storage;
        ge[3].buffer.minBindingSize = sizeof(float) * 4;
        ge[4].binding = 4;
        ge[4].visibility = WGPUShaderStage_Compute;
        ge[4].texture.sampleType = WGPUTextureSampleType_Depth;
        ge[4].texture.viewDimension = WGPUTextureViewDimension_2DArray;
        WGPUBindGroupLayoutDescriptor gbd{};
        gbd.label = rhi::str("pocket.grid.harmonics");
        gbd.entryCount = 5;
        gbd.entries = ge;
        grid_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &gbd);
        WGPUPipelineLayoutDescriptor gpl{};
        gpl.label = rhi::str("pocket.grid.harmonics");
        gpl.bindGroupLayoutCount = 1;
        gpl.bindGroupLayouts = &grid_bgl;
        grid_layout = wgpuDeviceCreatePipelineLayout(device->device(), &gpl);
        WGPUComputePipelineDescriptor gcp{};
        gcp.label = rhi::str("pocket.grid.harmonics");
        gcp.layout = grid_layout;
        gcp.compute.module = grid_shader;
        gcp.compute.entryPoint = rhi::str("grid_sh");
        grid_pipeline = wgpuDeviceCreateComputePipeline(device->device(), &gcp);
        if (!grid_pipeline) return fail("gpu_pipeline_failed", "the irradiance volumes' harmonics pass could not be created");
        for (auto& bufs : grid_frame_buf) for (auto& b : bufs) b = device->create_buffer("pocket.grid.frame", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(FrameUniforms));
        for (auto& b : grid_params) b = device->create_buffer("pocket.grid.fill", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(float) * (16 * 6 + 8));
        return {};
    }

    // Which probes are in use this frame (the first eight enabled by id, each keeping its layer
    // while it stays), which of them is captured this frame, and what the lit pass is told of the
    // captured ones. A probe is captured when it is new, moved or resized, realtime, or asked for.
    int gather_probes(const world::World& w, FrameUniforms& fu) {
        std::vector<std::pair<std::uint64_t, std::pair<world::ReflectionProbe, Vec3>>> found;
        w.ecs().each([&](flecs::entity e, const world::ReflectionProbe& p, const world::WorldTransform& t) {
            if (p.enabled) found.push_back({e.id(), {p, t.position}});
        });
        std::sort(found.begin(), found.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
        if (found.size() > kMaxProbes) found.resize(kMaxProbes);
        for (std::size_t i = 0; i < kMaxProbes; ++i) {
            ProbeSlot& s = probe_slots[i];
            if (i >= found.size()) { s = ProbeSlot{}; continue; }
            const auto& [id, pv] = found[i];
            const auto& [p, at] = pv;
            const Vec3 size{std::max(p.size.x, 0.01f), std::max(p.size.y, 0.01f), std::max(p.size.z, 0.01f)};
            if (s.entity != id || length(s.center - at) > 1e-4f || length(s.size - size) > 1e-4f) {
                s = ProbeSlot{id, at, size};
                s.bounces = kProbeBounces;
            }
            s.intensity = std::max(p.intensity, 0.0f);
            s.box = p.box_projection;
            s.realtime = p.realtime;
        }
        probe_count = static_cast<std::uint32_t>(found.size());
        if (probe_refresh) { for (auto& s : probe_slots) s.bounces = kProbeBounces; probe_refresh = false; }
        int capture = -1;
        for (std::uint32_t k = 0; k < probe_count && capture < 0; ++k) {
            const std::uint32_t i = (probe_cursor + k) % probe_count;
            if (probe_slots[i].bounces > 0 || probe_slots[i].realtime) capture = static_cast<int>(i);
        }
        if (capture >= 0) probe_cursor = static_cast<std::uint32_t>(capture) + 1;
        std::uint32_t used = 0;
        for (std::uint32_t i = 0; i < probe_count; ++i) {
            const ProbeSlot& s = probe_slots[i];
            if (!s.captured) continue;
            fu.probe_box[used][0] = s.center.x; fu.probe_box[used][1] = s.center.y; fu.probe_box[used][2] = s.center.z;
            fu.probe_box[used][3] = static_cast<float>(i);
            fu.probe_ext[used][0] = s.size.x * 0.5f; fu.probe_ext[used][1] = s.size.y * 0.5f; fu.probe_ext[used][2] = s.size.z * 0.5f;
            fu.probe_ext[used][3] = std::max(s.intensity, 1e-4f) * (s.box ? 1.0f : -1.0f);
            ++used;
        }
        fu.probe_info[0] = static_cast<float>(used);
        fu.probe_info[1] = static_cast<float>(kProbeLevels - 1);
        fu.probe_info[3] = static_cast<float>(used);
        stats.probes = used;
        return capture;
    }

    // The irradiance volumes in use (the first four enabled by id, their probes counted in order,
    // none past kMaxGridProbes), what the lit pass is told of the ready ones, and which probes are
    // captured this frame: up to kGridPerFrame of the first volume with passes still to make. A
    // volume starts its passes again when it is new, moved, resized or given other probe counts,
    // or on render.probes {refresh: true}.
    struct GridCapture { std::size_t volume = 0; std::uint32_t probe = 0; };
    std::vector<GridCapture> gather_grids(const world::World& w, FrameUniforms& fu, bool refresh, bool capture) {
        std::vector<std::pair<std::uint64_t, std::pair<world::IrradianceVolume, Vec3>>> found;
        w.ecs().each([&](flecs::entity e, const world::IrradianceVolume& v, const world::WorldTransform& t) {
            if (v.enabled) found.push_back({e.id(), {v, t.position}});
        });
        std::sort(found.begin(), found.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
        std::vector<GridVolume> next;
        std::uint32_t used = 0;
        for (const auto& [id, vv] : found) {
            if (next.size() >= kMaxGrids) break;
            const auto& [v, at] = vv;
            auto axis = [](float f) { return static_cast<std::uint32_t>(std::clamp(std::lround(f), 2l, 16l)); };
            GridVolume g;
            g.entity = id;
            g.center = at;
            g.size = Vec3{std::max(v.size.x, 0.01f), std::max(v.size.y, 0.01f), std::max(v.size.z, 0.01f)};
            g.nx = axis(v.probes.x);
            g.ny = axis(v.probes.y);
            g.nz = axis(v.probes.z);
            if (used + g.count() > kMaxGridProbes) continue;
            g.first = used;
            g.intensity = std::max(v.intensity, 0.0f);
            g.visibility = v.visibility;
            used += g.count();
            auto was = std::find_if(grids.begin(), grids.end(), [&](const GridVolume& o) { return o.entity == id; });
            const bool same = was != grids.end() && was->first == g.first && was->nx == g.nx && was->ny == g.ny && was->nz == g.nz && length(was->center - g.center) <= 1e-4f && length(was->size - g.size) <= 1e-4f;
            if (same && !refresh) {
                g.passes = was->passes;
                g.next = was->next;
                g.ready = was->ready;
                g.frame = was->frame;
            } else if (same) {
                g.passes = kProbeBounces;   // again from the light it has, as a refreshed probe does
                g.ready = was->ready;
                g.frame = was->frame;
            } else {
                g.passes = kProbeBounces;
            }
            next.push_back(g);
        }
        grids = std::move(next);
        std::uint32_t shown = 0;
        for (const GridVolume& g : grids) {
            fu.grid_box[shown][0] = g.center.x; fu.grid_box[shown][1] = g.center.y; fu.grid_box[shown][2] = g.center.z;
            fu.grid_box[shown][3] = static_cast<float>(kMaxProbes + g.first);
            fu.grid_ext[shown][0] = g.size.x * 0.5f; fu.grid_ext[shown][1] = g.size.y * 0.5f; fu.grid_ext[shown][2] = g.size.z * 0.5f;
            fu.grid_ext[shown][3] = g.intensity;
            fu.grid_cells[shown][0] = static_cast<float>(g.nx); fu.grid_cells[shown][1] = static_cast<float>(g.ny); fu.grid_cells[shown][2] = static_cast<float>(g.nz);
            fu.grid_cells[shown][3] = g.ready ? (g.visibility ? 2.0f : 1.0f) : 0.0f;
            ++shown;
        }
        fu.grid_info[0] = static_cast<float>(shown);
        fu.grid_info[1] = static_cast<float>(16 * (kMaxProbes + kMaxGridProbes));   // the moments' first vec4
        fu.grid_info[2] = static_cast<float>(kMaxProbes);                           // the grid probes' first slot
        stats.grids = static_cast<std::uint32_t>(std::count_if(grids.begin(), grids.end(), [](const GridVolume& g) { return g.ready; }));
        std::vector<GridCapture> out;
        for (std::size_t v = 0; capture && v < grids.size() && out.empty(); ++v) {
            GridVolume& g = grids[v];
            while (g.passes > 0 && out.size() < kGridPerFrame) {
                out.push_back({v, g.next});
                if (++g.next >= g.count()) {
                    g.next = 0;
                    --g.passes;
                }
            }
        }
        return out;
    }

    // The six views of a probe at `center`: along +X, -X, +Y, -Y, +Z and -Z, each 90 degrees.
    std::array<Mat4, 6> probe_views(Vec3 center) const {
        static const Vec3 dirs[6] = {{1, 0, 0}, {-1, 0, 0}, {0, 1, 0}, {0, -1, 0}, {0, 0, 1}, {0, 0, -1}};
        static const Vec3 ups[6] = {{0, 1, 0}, {0, 1, 0}, {0, 0, 1}, {0, 0, 1}, {0, 1, 0}, {0, 1, 0}};
        std::array<Mat4, 6> out;
        for (int f = 0; f < 6; ++f) out[static_cast<std::size_t>(f)] = Mat4::perspective(radians(90.0f), 1.0f, 0.05f, 1000.0f) * Mat4::look_at(center, center + dirs[f], ups[f]);
        return out;
    }

    // Screen-space reflections: the mesh module's vs_volume/fs_ssr over the scene group (the frame,
    // the sky's light) and a group of their own (the HDR target, depth, the surface targets).
    Status create_ssr() {
        WGPUBindGroupLayoutEntry be[5]{};
        be[0].binding = 10;
        be[0].visibility = WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(float) * 8;
        be[1].binding = 11;
        be[1].visibility = WGPUShaderStage_Fragment;
        be[1].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 12;
        be[2].visibility = WGPUShaderStage_Fragment;
        be[2].texture.sampleType = WGPUTextureSampleType_Depth;
        be[2].texture.viewDimension = WGPUTextureViewDimension_2D;
        for (std::uint32_t b = 3; b < 5; ++b) {
            be[b].binding = 10 + b;
            be[b].visibility = WGPUShaderStage_Fragment;
            be[b].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
            be[b].texture.viewDimension = WGPUTextureViewDimension_2D;
        }
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.ssr");
        bd.entryCount = 5;
        bd.entries = be;
        ssr_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUBindGroupLayout layouts[2] = {scene_bgl, ssr_bgl};
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.ssr");
        pld.bindGroupLayoutCount = 2;
        pld.bindGroupLayouts = layouts;
        ssr_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUColorTargetState ct{};
        ct.format = kHdrFormat;
        ct.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str("fs_ssr");
        fs.targetCount = 1;
        fs.targets = &ct;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.ssr");
        rpd.layout = ssr_layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs_volume");
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        ssr_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!ssr_pipeline) return fail("gpu_pipeline_failed", "the screen-space reflection pipeline could not be created");
        ssr_uniforms = device->create_buffer("pocket.ssr", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(float) * 8);
        return {};
    }

    // One layer of the decal array from RGBA8 pixels of any size: resampled to kDecalSize square
    // (bilinear), then its mip chain, each level averaging 2 by 2 weighted by alpha.
    void write_decal_layer(std::uint32_t layer, std::uint32_t w, std::uint32_t h, const std::uint8_t* rgba) {
        std::vector<std::uint8_t> level(static_cast<std::size_t>(kDecalSize) * kDecalSize * 4);
        for (std::uint32_t y = 0; y < kDecalSize; ++y) {
            const float fy = std::clamp((static_cast<float>(y) + 0.5f) * static_cast<float>(h) / kDecalSize - 0.5f, 0.0f, static_cast<float>(h - 1));
            const auto y0 = static_cast<std::uint32_t>(fy), y1 = std::min(y0 + 1, h - 1);
            for (std::uint32_t x = 0; x < kDecalSize; ++x) {
                const float fx = std::clamp((static_cast<float>(x) + 0.5f) * static_cast<float>(w) / kDecalSize - 0.5f, 0.0f, static_cast<float>(w - 1));
                const auto x0 = static_cast<std::uint32_t>(fx), x1 = std::min(x0 + 1, w - 1);
                const float ax = fx - static_cast<float>(x0), ay = fy - static_cast<float>(y0);
                for (int c = 0; c < 4; ++c) {
                    auto at = [&](std::uint32_t px, std::uint32_t py) { return static_cast<float>(rgba[(static_cast<std::size_t>(py) * w + px) * 4 + static_cast<std::size_t>(c)]); };
                    const float v = (at(x0, y0) * (1 - ax) + at(x1, y0) * ax) * (1 - ay) + (at(x0, y1) * (1 - ax) + at(x1, y1) * ax) * ay;
                    level[(static_cast<std::size_t>(y) * kDecalSize + x) * 4 + static_cast<std::size_t>(c)] = static_cast<std::uint8_t>(std::lround(v));
                }
            }
        }
        std::uint32_t size = kDecalSize;
        for (std::uint32_t l = 0; l < kDecalLevels; ++l) {
            WGPUTexelCopyTextureInfo dst{};
            dst.texture = decal_tex;
            dst.mipLevel = l;
            dst.origin = {0, 0, layer};
            dst.aspect = WGPUTextureAspect_All;
            WGPUTexelCopyBufferLayout lay{};
            lay.bytesPerRow = size * 4;
            lay.rowsPerImage = size;
            WGPUExtent3D ext{size, size, 1};
            wgpuQueueWriteTexture(device->queue(), &dst, level.data(), level.size(), &lay, &ext);
            if (size == 1) break;
            const std::uint32_t half = size / 2;
            std::vector<std::uint8_t> next(static_cast<std::size_t>(half) * half * 4);
            for (std::uint32_t y = 0; y < half; ++y) {
                for (std::uint32_t x = 0; x < half; ++x) {
                    float weighted[3] = {0, 0, 0}, alpha = 0;
                    for (std::uint32_t k = 0; k < 4; ++k) {
                        const std::uint8_t* p = &level[((static_cast<std::size_t>(y) * 2 + (k >> 1)) * size + x * 2 + (k & 1)) * 4];
                        const float a = static_cast<float>(p[3]) + 1e-3f;
                        for (int c = 0; c < 3; ++c) weighted[c] += static_cast<float>(p[c]) * a;
                        alpha += a;
                    }
                    std::uint8_t* q = &next[(static_cast<std::size_t>(y) * half + x) * 4];
                    for (int c = 0; c < 3; ++c) q[c] = static_cast<std::uint8_t>(std::lround(weighted[c] / alpha));
                    q[3] = static_cast<std::uint8_t>(std::lround(std::max(alpha - 4e-3f, 0.0f) / 4));
                }
            }
            level.swap(next);
            size = half;
        }
    }

    // The decals' buffer, image array (sRGB: the images are colours), sampler, and the built-in
    // soft round spot in layer 0.
    Status create_decals() {
        decal_buffer = device->create_buffer("pocket.decals", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(GpuDecal) * kMaxDecals);
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.decals");
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {kDecalSize, kDecalSize, kDecalLayers};
        td.format = WGPUTextureFormat_RGBA8UnormSrgb;
        td.mipLevelCount = kDecalLevels;
        td.sampleCount = 1;
        decal_tex = wgpuDeviceCreateTexture(device->device(), &td);
        if (!decal_tex) return fail("gpu_texture_failed", "cannot create the decal images");
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2DArray;
        vd.mipLevelCount = kDecalLevels;
        vd.arrayLayerCount = kDecalLayers;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        decal_view = wgpuTextureCreateView(decal_tex, &vd);
        std::vector<std::uint8_t> spot(64 * 64 * 4);
        for (int y = 0; y < 64; ++y) {
            for (int x = 0; x < 64; ++x) {
                const float dx = (static_cast<float>(x) + 0.5f) / 32.0f - 1.0f, dy = (static_cast<float>(y) + 0.5f) / 32.0f - 1.0f;
                const float r = std::sqrt(dx * dx + dy * dy);
                const float a = std::clamp((1.0f - r) / 0.35f, 0.0f, 1.0f);
                std::uint8_t* p = &spot[(static_cast<std::size_t>(y) * 64 + static_cast<std::size_t>(x)) * 4];
                p[0] = p[1] = p[2] = 255;
                p[3] = static_cast<std::uint8_t>(std::lround(255.0f * a * a * (3.0f - 2.0f * a)));
            }
        }
        write_decal_layer(0, 64, 64, spot.data());
        WGPUSamplerDescriptor sd{};
        sd.label = rhi::str("pocket.decals");
        sd.addressModeU = WGPUAddressMode_ClampToEdge;
        sd.addressModeV = WGPUAddressMode_ClampToEdge;
        sd.addressModeW = WGPUAddressMode_ClampToEdge;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Linear;
        sd.lodMinClamp = 0;
        sd.lodMaxClamp = 32;
        sd.maxAnisotropy = 8;
        decal_sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        return {};
    }

    // The array layer of a decal's image: loaded on first use into the next free layer; the spot
    // for an empty path, and (reported missing) for an image that cannot be read or has no layer left.
    // A decal image's layer (loaded the first time). A normal map is stored so the layer's sRGB
    // decoding gives its values back as they are in the file.
    std::uint32_t decal_layer(const std::string& path, bool normals = false) {
        if (path.empty()) return 0;
        const std::string key = normals ? path + "#normals" : path;
        if (auto it = decal_layers.find(key); it != decal_layers.end()) return it->second;
        if (failed.contains(path) || !assets) { note_missing(path); return 0; }
        if (decal_layers.size() + 1 >= kDecalLayers) {
            report_missing(path, std::format("more than {} decal images", kDecalLayers - 1));
            return 0;
        }
        auto img = assets->image(path);
        if (!img || (*img)->width == 0 || (*img)->height == 0) {
            report_missing(path, img ? "empty image" : img.error().message);
            return 0;
        }
        const auto layer = static_cast<std::uint32_t>(decal_layers.size() + 1);
        if (normals) {
            std::vector<std::uint8_t> enc((*img)->rgba);
            for (std::size_t k = 0; k < enc.size(); ++k) {
                if (k % 4 == 3) continue;
                const float v = static_cast<float>(enc[k]) / 255.0f;
                const float s = v <= 0.0031308f ? v * 12.92f : 1.055f * std::pow(v, 1.0f / 2.4f) - 0.055f;
                enc[k] = static_cast<std::uint8_t>(std::lround(std::clamp(s, 0.0f, 1.0f) * 255.0f));
            }
            write_decal_layer(layer, (*img)->width, (*img)->height, enc.data());
        } else {
            write_decal_layer(layer, (*img)->width, (*img)->height, (*img)->rgba.data());
        }
        decal_layers[key] = layer;
        return layer;
    }

    // The enabled decals whose boxes reach into the view, the nearest kMaxDecals, painted in order
    // (then by entity).
    void gather_decals(const world::World& w, FrameUniforms& fu) {
        struct Found { std::uint64_t id; world::Decal d; Mat4 model; Vec3 centre; float radius; float dist; };
        std::vector<Found> found;
        const Mat4 vp = camera.proj * camera.view;
        w.ecs().each([&](flecs::entity e, const world::Decal& d, const world::WorldTransform& t) {
            if (!d.enabled || d.color.a <= 0) return;
            const Vec3 box{t.scale.x * d.size.x, t.scale.y * d.size.y, t.scale.z * d.size.z};
            if (std::fabs(box.x) < 1e-5f || std::fabs(box.y) < 1e-5f || std::fabs(box.z) < 1e-5f) return;
            const float r = 0.5f * length(box);
            const float depth = dot(t.position - camera.position, camera.forward);
            if (depth + r < camera.near || depth - r > camera.far) return;
            float nx0 = 1e30f, nx1 = -1e30f, ny0 = 1e30f, ny1 = -1e30f;
            bool behind = false;
            for (int k = 0; k < 8; ++k) {
                const Vec3 c = t.position;
                const Vec4 p = vp * Vec4{c.x + ((k & 1) ? r : -r), c.y + ((k & 2) ? r : -r), c.z + ((k & 4) ? r : -r), 1};
                if (p.w <= 1e-4f) { behind = true; break; }
                nx0 = std::min(nx0, p.x / p.w); nx1 = std::max(nx1, p.x / p.w);
                ny0 = std::min(ny0, p.y / p.w); ny1 = std::max(ny1, p.y / p.w);
            }
            if (!behind && (nx1 < -1 || nx0 > 1 || ny1 < -1 || ny0 > 1)) return;
            found.push_back({e.id(), d, Mat4::trs(t.position, t.rotation, box), t.position, r, length(t.position - camera.position)});
        });
        std::sort(found.begin(), found.end(), [](const Found& a, const Found& b) { return a.dist < b.dist || (a.dist == b.dist && a.id < b.id); });
        if (found.size() > kMaxDecals) found.resize(kMaxDecals);
        std::sort(found.begin(), found.end(), [](const Found& a, const Found& b) { return a.d.order < b.d.order || (a.d.order == b.d.order && a.id < b.id); });
        std::vector<GpuDecal> out;
        out.reserve(found.size());
        for (const Found& f : found) {
            GpuDecal g{};
            to_array(f.model.inverse(), g.inv);
            const Vec3 axis = normalize(f.model.transform_dir(Vec3{0, -1, 0}));
            g.axis[0] = axis.x; g.axis[1] = axis.y; g.axis[2] = axis.z;
            g.axis[3] = std::cos(std::clamp(f.d.angle, 1.0f, 89.0f) * std::numbers::pi_v<float> / 180.0f);
            g.color[0] = decode(f.d.color.r); g.color[1] = decode(f.d.color.g); g.color[2] = decode(f.d.color.b); g.color[3] = std::clamp(f.d.color.a, 0.0f, 1.0f);
            g.params[0] = static_cast<float>(decal_layer(f.d.texture));
            g.params[1] = f.d.roughness;
            g.params[2] = std::max(f.d.emissive, 0.0f);
            g.params[3] = static_cast<float>(decal_layer(f.d.normal_map, true));
            g.extra[0] = std::max(f.d.bumpiness, 0.0f);
            g.sphere[0] = f.centre.x; g.sphere[1] = f.centre.y; g.sphere[2] = f.centre.z; g.sphere[3] = f.radius * f.radius;
            out.push_back(g);
        }
        if (!out.empty()) device->write_buffer(decal_buffer, 0, out.data(), out.size() * sizeof(GpuDecal));
        fu.decals[0] = static_cast<float>(out.size());
        stats.decals = static_cast<std::uint32_t>(out.size());
        stats.decal_images = static_cast<std::uint32_t>(decal_layers.size());
    }

    // Water: one layout (the bodies, the copies of the scene and of its depth, a sampler), the
    // surface's pipelines against the frame's depth and against the prepass's (with MSAA the pass
    // comes after the scene's and tests against the prepass), the view from under a surface, and
    // the surface's depth alone for the prepass.
    Status create_water() {
        WGPUBindGroupLayoutEntry be[4]{};
        be[0].binding = 16;
        be[0].visibility = WGPUShaderStage_Vertex | WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(WaterGpu) * kMaxWater;
        be[1].binding = 17;
        be[1].visibility = WGPUShaderStage_Fragment;
        be[1].texture.sampleType = WGPUTextureSampleType_Float;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 18;
        be[2].visibility = WGPUShaderStage_Fragment;
        be[2].texture.sampleType = WGPUTextureSampleType_Depth;
        be[2].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[3].binding = 19;
        be[3].visibility = WGPUShaderStage_Fragment;
        be[3].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.water");
        bd.entryCount = 4;
        bd.entries = be;
        water_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUBindGroupLayout layouts[2] = {scene_bgl, water_bgl};
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.water");
        pld.bindGroupLayoutCount = 2;
        pld.bindGroupLayouts = layouts;
        water_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUColorTargetState targets[3]{};
        targets[0].format = kHdrFormat;
        targets[1].format = WGPUTextureFormat_R32Uint;
        targets[2].format = kSurfaceFormat;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.targetCount = 3;
        fs.targets = targets;
        WGPUDepthStencilState ds{};
        ds.stencilFront.compare = WGPUCompareFunction_Always;
        ds.stencilBack.compare = WGPUCompareFunction_Always;
        ds.stencilReadMask = 0xFFFFFFFF;
        ds.stencilWriteMask = 0xFFFFFFFF;
        WGPURenderPipelineDescriptor rpd{};
        rpd.layout = water_layout;
        rpd.vertex.module = shader;
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;   // seen from below too
        rpd.depthStencil = &ds;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        const WGPUTextureFormat depths[2] = {device->depth_format(), kPrepassDepth};
        for (int v = 0; v < 2; ++v) {
            ds.format = depths[v];
            for (auto& t : targets) t.writeMask = WGPUColorWriteMask_All;
            ds.depthWriteEnabled = WGPUOptionalBool_True;
            ds.depthCompare = WGPUCompareFunction_Less;
            rpd.label = rhi::str("pocket.water");
            rpd.vertex.entryPoint = rhi::str("vs_water");
            fs.entryPoint = rhi::str("fs_water");
            water_pipeline[v] = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            if (!water_pipeline[v]) return fail("gpu_pipeline_failed", "the water pipeline could not be created");
            // Under a surface: every pixel through the water, the ids and surfaces left as they are.
            targets[1].writeMask = WGPUColorWriteMask_None;
            targets[2].writeMask = WGPUColorWriteMask_None;
            ds.depthWriteEnabled = WGPUOptionalBool_False;
            ds.depthCompare = WGPUCompareFunction_Always;
            rpd.label = rhi::str("pocket.water.under");
            rpd.vertex.entryPoint = rhi::str("vs_water_under");
            fs.entryPoint = rhi::str("fs_water_under");
            water_under_pipeline[v] = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            if (!water_under_pipeline[v]) return fail("gpu_pipeline_failed", "the underwater pipeline could not be created");
        }
        ds.format = kPrepassDepth;
        ds.depthWriteEnabled = WGPUOptionalBool_True;
        ds.depthCompare = WGPUCompareFunction_Less;
        rpd.label = rhi::str("pocket.water.depth");
        rpd.vertex.entryPoint = rhi::str("vs_water");
        rpd.fragment = nullptr;
        water_depth_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!water_depth_pipeline) return fail("gpu_pipeline_failed", "the water depth pipeline could not be created");
        water_uniforms = device->create_buffer("pocket.water", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(WaterGpu) * kMaxWater);
        WGPUSamplerDescriptor sd{};
        sd.label = rhi::str("pocket.water");
        sd.addressModeU = WGPUAddressMode_ClampToEdge;
        sd.addressModeV = WGPUAddressMode_ClampToEdge;
        sd.addressModeW = WGPUAddressMode_ClampToEdge;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
        sd.lodMinClamp = 0;
        sd.lodMaxClamp = 1;
        sd.maxAnisotropy = 1;
        water_sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        return {};
    }

    void release_water_targets() {
        if (water_bg) wgpuBindGroupRelease(water_bg);
        for (WGPUTextureView v : {water_scene_view, water_depth_view}) if (v) wgpuTextureViewRelease(v);
        for (WGPUTexture t : {water_scene_tex, water_depth_tex}) if (t) wgpuTextureRelease(t);
        water_bg = nullptr;
        water_scene_view = water_depth_view = nullptr;
        water_scene_tex = water_depth_tex = nullptr;
        water_w = water_h = 0;
    }

    // The copies the water reads, at the frame's size, and the group over them.
    Status ensure_water_targets(std::uint32_t w, std::uint32_t h) {
        if (water_scene_tex && water_w == w && water_h == h) return {};
        release_water_targets();
        auto [st, sv] = make_target("pocket.water.scene", w, h, kHdrFormat, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
        auto [dt, dv] = make_target("pocket.water.depth", w, h, kPrepassDepth, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
        if (!st || !dt) return fail("gpu_texture_failed", "cannot create the water's copies {}x{}", w, h);
        water_scene_tex = st;
        water_scene_view = sv;
        water_depth_tex = dt;
        water_depth_view = dv;
        water_w = w;
        water_h = h;
        WGPUBindGroupEntry e[4]{};
        e[0].binding = 16;
        e[0].buffer = water_uniforms;
        e[0].size = sizeof(WaterGpu) * kMaxWater;
        e[1].binding = 17;
        e[1].textureView = water_scene_view;
        e[2].binding = 18;
        e[2].textureView = water_depth_view;
        e[3].binding = 19;
        e[3].sampler = water_sampler;
        WGPUBindGroupDescriptor d{};
        d.label = rhi::str("pocket.water");
        d.layout = water_bgl;
        d.entryCount = 4;
        d.entries = e;
        water_bg = wgpuDeviceCreateBindGroup(device->device(), &d);
        return {};
    }

    // The enabled bodies (the first kMaxWater by id) with their waves at the world's time.
    void gather_water(const world::World& w) {
        water_bodies.clear();
        water_src.clear();
        std::vector<std::pair<std::uint64_t, std::pair<world::Water, Vec3>>> found;
        w.ecs().each([&](flecs::entity e, const world::Water& wa, const world::WorldTransform& t) {
            if (wa.enabled && wa.size.x > 0 && wa.size.y > 0) found.push_back({e.id(), {wa, t.position}});
        });
        std::sort(found.begin(), found.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
        if (found.size() > kMaxWater) found.resize(kMaxWater);
        const auto time = static_cast<float>(w.seconds());
        for (const auto& [id, src] : found) {
            const auto& [wa, c] = src;
            WaterGpu g{};
            g.center[0] = c.x; g.center[1] = c.y; g.center[2] = c.z; g.center[3] = time;
            // Four vertices along the smallest wave, at most 256 cells a side; still water a cell every four units.
            const float cell = wa.wave_height > 0 ? std::max(wa.wave_length * 0.24f / 4.0f, 0.05f) : 4.0f;
            g.extent[0] = wa.size.x * 0.5f;
            g.extent[1] = wa.size.y * 0.5f;
            g.extent[2] = std::clamp(std::ceil(wa.size.x / cell), 1.0f, 256.0f);
            g.extent[3] = std::clamp(std::ceil(wa.size.y / cell), 1.0f, 256.0f);
            g.color[0] = decode(wa.color.r); g.color[1] = decode(wa.color.g); g.color[2] = decode(wa.color.b);
            g.color[3] = std::max(wa.clarity, 0.01f);
            const auto waves = world::water_waves(wa);
            float sum = 0;
            for (int k = 0; k < world::kWaterWaves; ++k) {
                const world::Wave& v = waves[static_cast<std::size_t>(k)];
                g.dir_k[k][0] = v.dir_x; g.dir_k[k][1] = v.dir_z; g.dir_k[k][2] = v.k; g.dir_k[k][3] = v.omega;
                g.amp[k][0] = v.amplitude; g.amp[k][1] = v.steepness; g.amp[k][2] = v.phase;
                sum += v.amplitude;
            }
            g.misc[0] = wa.flow.x; g.misc[1] = wa.flow.y; g.misc[2] = std::max(wa.ripples, 0.0f); g.misc[3] = std::max(wa.foam, 0.0f);
            g.more[0] = sum;
            g.more[1] = std::clamp(wa.choppiness, 0.0f, 1.0f);
            g.id[0] = static_cast<std::uint32_t>(id & 0xFFFFFFFFu);
            water_bodies.push_back(g);
            water_src.emplace_back(wa, c);
        }
        stats.water = static_cast<std::uint32_t>(water_bodies.size());
    }

    // The water pass: the scene and its depth copied, the view from under a surface if the camera is
    // under one, then every surface; tested against `depth` (the frame's, then the surfaces are also
    // added to the prepass, or the prepass itself).
    Status draw_water(rhi::Frame& frame, WGPUTextureView depth, bool on_prepass, const std::function<void(WGPURenderPassEncoder)>& set_viewport) {
        POCKET_TRY_VOID(ensure_water_targets(frame.width, frame.height));
        WGPUTexelCopyTextureInfo src{}, dst{};
        src.aspect = WGPUTextureAspect_All;
        dst.aspect = WGPUTextureAspect_All;
        const WGPUExtent3D ext{frame.width, frame.height, 1};
        src.texture = hdr_tex;
        dst.texture = water_scene_tex;
        wgpuCommandEncoderCopyTextureToTexture(frame.encoder, &src, &dst, &ext);
        src.texture = prepass_tex;
        dst.texture = water_depth_tex;
        wgpuCommandEncoderCopyTextureToTexture(frame.encoder, &src, &dst, &ext);
        water_under = -1;
        for (std::size_t i = 0; i < water_src.size() && water_under < 0; ++i) {
            const auto& [wa, c] = water_src[i];
            const Vec3 eye = camera.position;
            if (!world::water_covers(wa, c, eye.x, eye.z) || eye.y < c.y - std::max(wa.depth, 0.0f)) continue;
            if (eye.y < world::water_at(wa, c.y, eye.x, eye.z, water_bodies[i].center[3]).position.y) water_under = static_cast<int>(i);
        }
        stats.underwater = water_under >= 0;
        device->write_buffer(water_uniforms, 0, water_bodies.data(), water_bodies.size() * sizeof(WaterGpu));
        WGPURenderPassColorAttachment ca[3]{};
        const WGPUTextureView views[3] = {hdr_view, id_view, surface_view};
        for (int k = 0; k < 3; ++k) {
            ca[k].view = views[k];
            ca[k].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
            ca[k].loadOp = WGPULoadOp_Load;
            ca[k].storeOp = WGPUStoreOp_Store;
        }
        WGPURenderPassDepthStencilAttachment ds{};
        ds.view = depth;
        ds.depthLoadOp = WGPULoadOp_Load;
        ds.depthStoreOp = WGPUStoreOp_Store;
        ds.stencilLoadOp = WGPULoadOp_Undefined;
        ds.stencilStoreOp = WGPUStoreOp_Undefined;
        ds.stencilReadOnly = true;
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str("pocket.water");
        rp.colorAttachmentCount = 3;
        rp.colorAttachments = ca;
        rp.depthStencilAttachment = &ds;
        const int v = on_prepass ? 1 : 0;
        auto draw_surfaces = [&](WGPURenderPassEncoder enc) {
            wgpuRenderPassEncoderSetBindGroup(enc, 0, scene_bg, 0, nullptr);
            wgpuRenderPassEncoderSetBindGroup(enc, 1, water_bg, 0, nullptr);
            for (std::size_t i = 0; i < water_bodies.size(); ++i) {
                const auto cells = static_cast<std::uint32_t>(water_bodies[i].extent[2] * water_bodies[i].extent[3]);
                wgpuRenderPassEncoderDraw(enc, cells * 6, 1, 0, static_cast<std::uint32_t>(i));
                stats.draw_calls++;
            }
        };
        WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
        set_viewport(enc);
        if (water_under >= 0) {
            wgpuRenderPassEncoderSetPipeline(enc, water_under_pipeline[v]);
            wgpuRenderPassEncoderSetBindGroup(enc, 0, scene_bg, 0, nullptr);
            wgpuRenderPassEncoderSetBindGroup(enc, 1, water_bg, 0, nullptr);
            wgpuRenderPassEncoderDraw(enc, 3, 1, 0, static_cast<std::uint32_t>(water_under));
            stats.draw_calls++;
        }
        wgpuRenderPassEncoderSetPipeline(enc, water_pipeline[v]);
        draw_surfaces(enc);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        if (!on_prepass) {
            // The surfaces in the prepass too, for what reads it after (fog, reflections, TAA, depth of field).
            WGPURenderPassDepthStencilAttachment pds = ds;
            pds.view = prepass_view;
            WGPURenderPassDescriptor prp{};
            prp.label = rhi::str("pocket.water.depth");
            prp.depthStencilAttachment = &pds;
            WGPURenderPassEncoder penc = begin_pass(frame.encoder, prp);
            set_viewport(penc);
            wgpuRenderPassEncoderSetPipeline(penc, water_depth_pipeline);
            draw_surfaces(penc);
            wgpuRenderPassEncoderEnd(penc);
            wgpuRenderPassEncoderRelease(penc);
        }
        return {};
    }

    // Trace the reflections into the scratch target and copy it back over the HDR target.
    Status draw_ssr(rhi::Frame& frame) {
        POCKET_TRY_VOID(ensure_fx_target(frame.width, frame.height));
        if (!ssr_bg || ssr_bg_scene != hdr_view || ssr_bg_depth != prepass_view || ssr_bg_surface != surface_view) {
            if (ssr_bg) wgpuBindGroupRelease(ssr_bg);
            WGPUBindGroupEntry e[5]{};
            e[0].binding = 10;
            e[0].buffer = ssr_uniforms;
            e[0].size = sizeof(float) * 8;
            e[1].binding = 11;
            e[1].textureView = hdr_view;
            e[2].binding = 12;
            e[2].textureView = prepass_view;
            e[3].binding = 13;
            e[3].textureView = surface_view;
            e[4].binding = 14;
            e[4].textureView = albedo_view;
            WGPUBindGroupDescriptor d{};
            d.label = rhi::str("pocket.ssr");
            d.layout = ssr_bgl;
            d.entryCount = 5;
            d.entries = e;
            ssr_bg = wgpuDeviceCreateBindGroup(device->device(), &d);
            ssr_bg_scene = hdr_view;
            ssr_bg_depth = prepass_view;
            ssr_bg_surface = surface_view;
        }
        // With TAA blending the frames, each frame marches half the steps from an offset that moves
        // by the golden ratio, and the frames together sample between each other's steps.
        const bool spread = taa.enabled;
        const int steps = spread ? std::max(8, ssr.steps / 2) : ssr.steps;
        const float offset = spread ? std::fmod(static_cast<float>(ssr_frame++ % 64) * 0.618034f, 1.0f) : 0.0f;
        const float u[8] = {ssr.max_distance, ssr.max_roughness, static_cast<float>(steps), ssr.thickness, ssr.intensity, offset, 0, 0};
        device->write_buffer(ssr_uniforms, 0, u, sizeof u);
        WGPURenderPassColorAttachment ca{};
        ca.view = fx_view;
        ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ca.loadOp = WGPULoadOp_Clear;
        ca.storeOp = WGPUStoreOp_Store;
        ca.clearValue = {0, 0, 0, 1};
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str("pocket.ssr");
        rp.colorAttachmentCount = 1;
        rp.colorAttachments = &ca;
        WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
        wgpuRenderPassEncoderSetPipeline(enc, ssr_pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(enc, 1, ssr_bg, 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        fx_copy_back(frame);
        stats.ssr = true;
        stats.draw_calls++;
        return {};
    }

    // Depth of field and motion blur: one layout (uniforms, the HDR target, depth, motion, a sampler),
    // two pipelines, a scratch target the pass draws into and is copied back from.
    Status create_fx() {
        POCKET_TRY(module, device->create_shader("pocket.fx", kFxWgsl));
        fx_shader = module;
        WGPUBindGroupLayoutEntry be[5]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(FxUniforms);
        be[1].binding = 1;
        be[1].visibility = WGPUShaderStage_Fragment;
        be[1].texture.sampleType = WGPUTextureSampleType_Float;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 2;
        be[2].visibility = WGPUShaderStage_Fragment;
        be[2].texture.sampleType = WGPUTextureSampleType_Depth;
        be[2].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[3].binding = 3;
        be[3].visibility = WGPUShaderStage_Fragment;
        be[3].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        be[3].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[4].binding = 4;
        be[4].visibility = WGPUShaderStage_Fragment;
        be[4].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.fx");
        bd.entryCount = 5;
        bd.entries = be;
        fx_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.fx");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &fx_bgl;
        fx_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        auto make = [&](const char* label, const char* entry) {
            WGPUColorTargetState ct{};
            ct.format = kHdrFormat;
            ct.writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState fs{};
            fs.module = fx_shader;
            fs.entryPoint = rhi::str(entry);
            fs.targetCount = 1;
            fs.targets = &ct;
            WGPURenderPipelineDescriptor rpd{};
            rpd.label = rhi::str(label);
            rpd.layout = fx_layout;
            rpd.vertex.module = fx_shader;
            rpd.vertex.entryPoint = rhi::str("vs_screen");
            rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
            rpd.primitive.frontFace = WGPUFrontFace_CCW;
            rpd.primitive.cullMode = WGPUCullMode_None;
            rpd.multisample.count = 1;
            rpd.multisample.mask = 0xFFFFFFFFu;
            rpd.fragment = &fs;
            return wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        };
        dof_pipeline = make("pocket.dof", "fs_dof");
        blur_pipeline = make("pocket.motion_blur", "fs_motion_blur");
        if (!dof_pipeline || !blur_pipeline) return fail("gpu_pipeline_failed", "the depth of field or motion blur pipeline could not be created");
        fx_uniforms = device->create_buffer("pocket.fx", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(FxUniforms));
        return {};
    }

    // The scratch target effects draw into before they are copied back into the HDR target.
    Status ensure_fx_target(std::uint32_t w, std::uint32_t h) {
        if (!fx_tex || fx_w != w || fx_h != h) {
            if (fx_view) wgpuTextureViewRelease(fx_view);
            if (fx_tex) wgpuTextureRelease(fx_tex);
            auto [t, v] = make_target("pocket.fx", w, h, kHdrFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopySrc);
            if (!t) return fail("gpu_texture_failed", "cannot create the effects target {}x{}", w, h);
            fx_tex = t;
            fx_view = v;
            fx_w = w;
            fx_h = h;
        }
        return {};
    }

    // Copy the scratch target back into the HDR target.
    void fx_copy_back(rhi::Frame& frame) {
        WGPUTexelCopyTextureInfo src{};
        src.texture = fx_tex;
        src.aspect = WGPUTextureAspect_All;
        WGPUTexelCopyTextureInfo dst{};
        dst.texture = hdr_tex;
        dst.aspect = WGPUTextureAspect_All;
        WGPUExtent3D ext{frame.width, frame.height, 1};
        wgpuCommandEncoderCopyTextureToTexture(frame.encoder, &src, &dst, &ext);
    }

    // One effect over the HDR target: drawn into the scratch target from it, then copied back.
    Status apply_fx(rhi::Frame& frame, WGPURenderPipeline pipeline, const char* label, const FxUniforms& u) {
        POCKET_TRY_VOID(ensure_fx_target(frame.width, frame.height));
        if (!fx_bg || fx_bg_scene != hdr_view || fx_bg_depth != prepass_view || fx_bg_velocity != velocity_view) {
            if (fx_bg) wgpuBindGroupRelease(fx_bg);
            WGPUBindGroupEntry e[5]{};
            e[0].binding = 0;
            e[0].buffer = fx_uniforms;
            e[0].size = sizeof(FxUniforms);
            e[1].binding = 1;
            e[1].textureView = hdr_view;
            e[2].binding = 2;
            e[2].textureView = prepass_view;
            e[3].binding = 3;
            e[3].textureView = velocity_view;
            e[4].binding = 4;
            e[4].sampler = bloom_sampler;
            WGPUBindGroupDescriptor d{};
            d.label = rhi::str("pocket.fx");
            d.layout = fx_bgl;
            d.entryCount = 5;
            d.entries = e;
            fx_bg = wgpuDeviceCreateBindGroup(device->device(), &d);
            fx_bg_scene = hdr_view;
            fx_bg_depth = prepass_view;
            fx_bg_velocity = velocity_view;
        }
        device->write_buffer(fx_uniforms, 0, &u, sizeof u);
        WGPURenderPassColorAttachment ca{};
        ca.view = fx_view;
        ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ca.loadOp = WGPULoadOp_Clear;
        ca.storeOp = WGPUStoreOp_Store;
        ca.clearValue = {0, 0, 0, 1};
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str(label);
        rp.colorAttachmentCount = 1;
        rp.colorAttachments = &ca;
        WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
        wgpuRenderPassEncoderSetPipeline(enc, pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, fx_bg, 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        fx_copy_back(frame);
        stats.draw_calls++;
        return {};
    }

    // Order-independent transparency: the translucent meshes' pipelines into the two targets (depth
    // tested against the opaque scene, not written) and the composite over the HDR target.
    Status create_oit() {
        WGPUBlendState add{};
        add.color = {WGPUBlendOperation_Add, WGPUBlendFactor_One, WGPUBlendFactor_One};
        add.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_One, WGPUBlendFactor_One};
        WGPUBlendState keep{};
        keep.color = {WGPUBlendOperation_Add, WGPUBlendFactor_Zero, WGPUBlendFactor_OneMinusSrc};
        keep.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_Zero, WGPUBlendFactor_OneMinusSrc};
        WGPUColorTargetState t[2]{};
        t[0].format = kHdrFormat;
        t[0].writeMask = WGPUColorWriteMask_All;
        t[0].blend = &add;
        t[1].format = WGPUTextureFormat_R16Float;
        t[1].writeMask = WGPUColorWriteMask_All;
        t[1].blend = &keep;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str("fs_oit");
        fs.targetCount = 2;
        fs.targets = t;
        WGPUDepthStencilState ds{};
        ds.format = device->depth_format();
        ds.depthWriteEnabled = WGPUOptionalBool_False;
        ds.depthCompare = WGPUCompareFunction_LessEqual;
        ds.stencilFront.compare = WGPUCompareFunction_Always;
        ds.stencilBack.compare = WGPUCompareFunction_Always;
        ds.stencilReadMask = 0xFFFFFFFF;
        ds.stencilWriteMask = 0xFFFFFFFF;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.oit");
        rpd.layout = layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_Back;
        rpd.depthStencil = &ds;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        oit_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        rpd.label = rhi::str("pocket.oit.skinned");
        rpd.vertex.entryPoint = rhi::str("vs_skinned");
        rpd.vertex.bufferCount = 2;
        rpd.vertex.buffers = vbls;
        oit_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        rpd.multisample.count = 4;   // WebGPU multisamples at 1 or 4 only
        oit_ms_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        rpd.label = rhi::str("pocket.oit.msaa");
        rpd.vertex.entryPoint = rhi::str("vs");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        oit_ms_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        POCKET_TRY(module, device->create_shader("pocket.oit", kOitCompositeWgsl));
        oit_shader = module;
        WGPUBindGroupLayoutEntry be[2]{};
        for (std::uint32_t b = 0; b < 2; ++b) {
            be[b].binding = b;
            be[b].visibility = WGPUShaderStage_Fragment;
            be[b].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
            be[b].texture.viewDimension = WGPUTextureViewDimension_2D;
        }
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.oit");
        bd.entryCount = 2;
        bd.entries = be;
        oit_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.oit");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &oit_bgl;
        oit_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUBlendState over{};
        over.color = {WGPUBlendOperation_Add, WGPUBlendFactor_SrcAlpha, WGPUBlendFactor_OneMinusSrcAlpha};
        over.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_One, WGPUBlendFactor_OneMinusSrcAlpha};
        WGPUColorTargetState ct{};
        ct.format = kHdrFormat;
        ct.writeMask = WGPUColorWriteMask_All;
        ct.blend = &over;
        WGPUFragmentState cfs{};
        cfs.module = oit_shader;
        cfs.entryPoint = rhi::str("fs_composite");
        cfs.targetCount = 1;
        cfs.targets = &ct;
        WGPURenderPipelineDescriptor c{};
        c.label = rhi::str("pocket.oit.composite");
        c.layout = oit_layout;
        c.vertex.module = oit_shader;
        c.vertex.entryPoint = rhi::str("vs_screen");
        c.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        c.primitive.frontFace = WGPUFrontFace_CCW;
        c.primitive.cullMode = WGPUCullMode_None;
        c.multisample.count = 1;
        c.multisample.mask = 0xFFFFFFFFu;
        c.fragment = &cfs;
        oit_composite_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &c);
        c.label = rhi::str("pocket.oit.composite.msaa");
        c.multisample.count = 4;
        oit_ms_composite_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &c);
        if (!oit_pipeline || !oit_skinned_pipeline || !oit_composite_pipeline || !oit_ms_pipeline || !oit_ms_skinned_pipeline || !oit_ms_composite_pipeline) return fail("gpu_pipeline_failed", "order-independent transparency pipelines could not be created");
        return {};
    }

    // Its two targets at the frame's size, and the composite's group over them.
    // With MSAA, multisampled ones too, which the accumulation pass resolves into them.
    Status ensure_oit_targets(std::uint32_t w, std::uint32_t h, int samples) {
        if (oit_accum_tex && oit_w == w && oit_h == h && oit_samples == samples) return {};
        if (oit_bg) wgpuBindGroupRelease(oit_bg);
        for (WGPUTextureView* v : {&oit_accum_view, &oit_reveal_view, &oit_ms_accum_view, &oit_ms_reveal_view}) {
            if (*v) wgpuTextureViewRelease(*v);
            *v = nullptr;
        }
        for (WGPUTexture* t : {&oit_accum_tex, &oit_reveal_tex, &oit_ms_accum_tex, &oit_ms_reveal_tex}) {
            if (*t) wgpuTextureRelease(*t);
            *t = nullptr;
        }
        auto [at, av] = make_target("pocket.oit.accum", w, h, kHdrFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
        auto [rt, rv] = make_target("pocket.oit.reveal", w, h, WGPUTextureFormat_R16Float, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
        if (!at || !rt) return fail("gpu_texture_failed", "cannot create the transparency targets {}x{}", w, h);
        oit_accum_tex = at;
        oit_accum_view = av;
        oit_reveal_tex = rt;
        oit_reveal_view = rv;
        if (samples > 1) {
            auto [mat, mav] = make_target("pocket.oit.accum.msaa", w, h, kHdrFormat, WGPUTextureUsage_RenderAttachment, static_cast<std::uint32_t>(samples));
            auto [mrt, mrv] = make_target("pocket.oit.reveal.msaa", w, h, WGPUTextureFormat_R16Float, WGPUTextureUsage_RenderAttachment, static_cast<std::uint32_t>(samples));
            if (!mat || !mrt) return fail("gpu_texture_failed", "cannot create the multisampled transparency targets {}x{}", w, h);
            oit_ms_accum_tex = mat;
            oit_ms_accum_view = mav;
            oit_ms_reveal_tex = mrt;
            oit_ms_reveal_view = mrv;
        }
        oit_w = w;
        oit_h = h;
        oit_samples = samples;
        WGPUBindGroupEntry e[2]{};
        e[0].binding = 0;
        e[0].textureView = oit_accum_view;
        e[1].binding = 1;
        e[1].textureView = oit_reveal_view;
        WGPUBindGroupDescriptor d{};
        d.label = rhi::str("pocket.oit");
        d.layout = oit_bgl;
        d.entryCount = 2;
        d.entries = e;
        oit_bg = wgpuDeviceCreateBindGroup(device->device(), &d);
        return {};
    }

    // The TAA resolve: a full-screen pass over this frame, the history, the motion and the depth.
    Status create_taa() {
        POCKET_TRY(module, device->create_shader("pocket.taa", kTaaWgsl));
        taa_shader = module;
        WGPUBindGroupLayoutEntry be[6]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(TaaUniforms);
        for (std::uint32_t b = 1; b <= 3; ++b) {
            be[b].binding = b;
            be[b].visibility = WGPUShaderStage_Fragment;
            be[b].texture.sampleType = b == 2 ? WGPUTextureSampleType_Float : WGPUTextureSampleType_UnfilterableFloat;
            be[b].texture.viewDimension = WGPUTextureViewDimension_2D;
        }
        be[4].binding = 4;
        be[4].visibility = WGPUShaderStage_Fragment;
        be[4].texture.sampleType = WGPUTextureSampleType_Depth;
        be[4].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[5].binding = 5;
        be[5].visibility = WGPUShaderStage_Fragment;
        be[5].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.taa");
        bd.entryCount = 6;
        bd.entries = be;
        taa_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.taa");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &taa_bgl;
        taa_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUColorTargetState ct{};
        ct.format = kHdrFormat;
        ct.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = taa_shader;
        fs.entryPoint = rhi::str("fs_taa");
        fs.targetCount = 1;
        fs.targets = &ct;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.taa");
        rpd.layout = taa_layout;
        rpd.vertex.module = taa_shader;
        rpd.vertex.entryPoint = rhi::str("vs_screen");
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        taa_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!taa_pipeline) return fail("gpu_pipeline_failed", "the TAA pipeline could not be created");
        taa_uniforms = device->create_buffer("pocket.taa", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(TaaUniforms));
        return {};
    }

    // Resolve this frame against its history into the next resolved texture, and put the result in
    // the HDR target for what follows (volumetric fog, bloom, the final pass).
    Status resolve_taa(rhi::Frame& frame, const Mat4& reproject) {
        const std::uint32_t w = frame.width, h = frame.height;
        if (!taa_tex[0] || taa_w != w || taa_h != h) {
            for (int k = 0; k < 2; ++k) {
                if (taa_bg[k]) wgpuBindGroupRelease(taa_bg[k]);
                if (taa_view[k]) wgpuTextureViewRelease(taa_view[k]);
                if (taa_tex[k]) wgpuTextureRelease(taa_tex[k]);
                taa_bg[k] = nullptr;
                auto [t, v] = make_target(k == 0 ? "pocket.taa.a" : "pocket.taa.b", w, h, kHdrFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopySrc);
                if (!t) return fail("gpu_texture_failed", "cannot create the TAA history {}x{}", w, h);
                taa_tex[k] = t;
                taa_view[k] = v;
            }
            taa_w = w;
            taa_h = h;
            taa_valid = false;
        }
        if (!taa_bg[0] || taa_bg_current != hdr_view || taa_bg_velocity != velocity_view || taa_bg_depth != prepass_view) {
            for (int k = 0; k < 2; ++k) {
                if (taa_bg[k]) wgpuBindGroupRelease(taa_bg[k]);
                // Group k writes taa_view[k] and reads the other as its history.
                WGPUBindGroupEntry e[6]{};
                e[0].binding = 0;
                e[0].buffer = taa_uniforms;
                e[0].size = sizeof(TaaUniforms);
                e[1].binding = 1;
                e[1].textureView = hdr_view;
                e[2].binding = 2;
                e[2].textureView = taa_view[1 - k];
                e[3].binding = 3;
                e[3].textureView = velocity_view;
                e[4].binding = 4;
                e[4].textureView = prepass_view;
                e[5].binding = 5;
                e[5].sampler = bloom_sampler;
                WGPUBindGroupDescriptor d{};
                d.label = rhi::str("pocket.taa");
                d.layout = taa_bgl;
                d.entryCount = 6;
                d.entries = e;
                taa_bg[k] = wgpuDeviceCreateBindGroup(device->device(), &d);
            }
            taa_bg_current = hdr_view;
            taa_bg_velocity = velocity_view;
            taa_bg_depth = prepass_view;
        }
        TaaUniforms u{};
        to_array(reproject, u.reproject);
        u.params[0] = std::clamp(taa.feedback, 0.5f, 0.98f);
        u.params[1] = taa_valid ? 0.0f : 1.0f;
        u.viewport[0] = static_cast<float>(applied.x);
        u.viewport[1] = static_cast<float>(applied.y);
        u.viewport[2] = static_cast<float>(std::max(1u, applied.w));
        u.viewport[3] = static_cast<float>(std::max(1u, applied.h));
        device->write_buffer(taa_uniforms, 0, &u, sizeof u);
        const int k = taa_next;
        WGPURenderPassColorAttachment ca{};
        ca.view = taa_view[k];
        ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ca.loadOp = WGPULoadOp_Clear;
        ca.storeOp = WGPUStoreOp_Store;
        ca.clearValue = {0, 0, 0, 1};
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str("pocket.taa");
        rp.colorAttachmentCount = 1;
        rp.colorAttachments = &ca;
        WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
        wgpuRenderPassEncoderSetPipeline(enc, taa_pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, taa_bg[k], 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        WGPUTexelCopyTextureInfo src{};
        src.texture = taa_tex[k];
        src.aspect = WGPUTextureAspect_All;
        WGPUTexelCopyTextureInfo dst{};
        dst.texture = hdr_tex;
        dst.aspect = WGPUTextureAspect_All;
        WGPUExtent3D ext{w, h, 1};
        wgpuCommandEncoderCopyTextureToTexture(frame.encoder, &src, &dst, &ext);
        taa_next = 1 - k;
        taa_valid = true;
        stats.taa = true;
        stats.draw_calls++;
        return {};
    }

    // The volumetric fog's pass and its stand-in (docs/design/rendering.md, Volumetric light).
    Status create_volume() {
        WGPUBindGroupLayoutEntry ve[4]{};
        ve[0].binding = 8;
        ve[0].visibility = WGPUShaderStage_Fragment;
        ve[0].buffer.type = WGPUBufferBindingType_Uniform;
        ve[0].buffer.minBindingSize = sizeof(VolumeUniforms);
        ve[1].binding = 9;
        ve[1].visibility = WGPUShaderStage_Fragment;
        ve[1].texture.sampleType = WGPUTextureSampleType_Depth;
        ve[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        ve[2].binding = 10;
        ve[2].visibility = WGPUShaderStage_Fragment;
        ve[2].texture.sampleType = WGPUTextureSampleType_Float;
        ve[2].texture.viewDimension = WGPUTextureViewDimension_2D;
        ve[3].binding = 11;
        ve[3].visibility = WGPUShaderStage_Fragment;
        ve[3].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor vd{};
        vd.label = rhi::str("pocket.volume");
        vd.entryCount = 4;
        vd.entries = ve;
        volume_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &vd);
        WGPUBindGroupLayout layouts[2] = {scene_bgl, volume_bgl};
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.volume");
        pld.bindGroupLayoutCount = 2;
        pld.bindGroupLayouts = layouts;
        volume_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUColorTargetState ct{};
        ct.format = kHdrFormat;
        ct.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str("fs_volume");
        fs.targetCount = 1;
        fs.targets = &ct;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.volume");
        rpd.layout = volume_layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs_volume");
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        volume_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!volume_pipeline) return fail("gpu_pipeline_failed", "the volumetric fog pipeline could not be created");
        volume_uniforms = device->create_buffer("pocket.volume", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(VolumeUniforms));
        WGPUSamplerDescriptor sd{};
        sd.label = rhi::str("pocket.volume.history");
        sd.addressModeU = sd.addressModeV = sd.addressModeW = WGPUAddressMode_ClampToEdge;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
        sd.lodMaxClamp = 1.0f;
        sd.maxAnisotropy = 1;
        volume_samp = wgpuDeviceCreateSampler(device->device(), &sd);
        auto [nt, nv] = make_target("pocket.volume.none", 1, 1, kHdrFormat, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
        volume_none_tex = nt;
        volume_none_view = nv;
        const std::uint16_t none[4] = {0, 0, 0, to_half(1.0f)};
        WGPUTexelCopyTextureInfo dst{};
        dst.texture = volume_none_tex;
        dst.aspect = WGPUTextureAspect_All;
        WGPUTexelCopyBufferLayout layout{};
        layout.bytesPerRow = sizeof none;
        layout.rowsPerImage = 1;
        WGPUExtent3D ext{1, 1, 1};
        wgpuQueueWriteTexture(device->queue(), &dst, none, sizeof none, &layout, &ext);
        return {};
    }

    // Screen-space global illumination's two passes (docs/design/rendering.md).
    Status create_gi() {
        WGPUBindGroupLayoutEntry ge[6]{};
        ge[0].binding = 20;
        ge[0].visibility = WGPUShaderStage_Fragment;
        ge[0].buffer.type = WGPUBufferBindingType_Uniform;
        ge[0].buffer.minBindingSize = sizeof(float) * 8;
        ge[1].binding = 21;
        ge[1].visibility = WGPUShaderStage_Fragment;
        ge[1].texture.sampleType = WGPUTextureSampleType_Depth;
        ge[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        for (int i = 2; i < 6; ++i) {
            ge[i].binding = static_cast<std::uint32_t>(20 + i);
            ge[i].visibility = WGPUShaderStage_Fragment;
            ge[i].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
            ge[i].texture.viewDimension = WGPUTextureViewDimension_2D;
        }
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.gi");
        bd.entryCount = 6;
        bd.entries = ge;
        gi_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUBindGroupLayout layouts[2] = {scene_bgl, gi_bgl};
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.gi");
        pld.bindGroupLayoutCount = 2;
        pld.bindGroupLayouts = layouts;
        gi_layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        WGPUColorTargetState ct{};
        ct.format = kHdrFormat;
        ct.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str("fs_gi");
        fs.targetCount = 1;
        fs.targets = &ct;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.gi");
        rpd.layout = gi_layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs_volume");
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        gi_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!gi_pipeline) return fail("gpu_pipeline_failed", "the global illumination pipeline could not be created");
        // Added to the frame: color plus color, its alpha kept.
        WGPUBlendState add{};
        add.color.operation = WGPUBlendOperation_Add;
        add.color.srcFactor = WGPUBlendFactor_One;
        add.color.dstFactor = WGPUBlendFactor_One;
        add.alpha.operation = WGPUBlendOperation_Add;
        add.alpha.srcFactor = WGPUBlendFactor_Zero;
        add.alpha.dstFactor = WGPUBlendFactor_One;
        ct.blend = &add;
        fs.entryPoint = rhi::str("fs_gi_add");
        rpd.label = rhi::str("pocket.gi.add");
        gi_add_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!gi_add_pipeline) return fail("gpu_pipeline_failed", "the global illumination pipeline could not be created");
        gi_uniforms = device->create_buffer("pocket.gi", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(float) * 8);
        return {};
    }

    // Gather the light bounced off what is on screen at half size, then add it to the frame (after
    // the scene pass, before the reflections; the prepass's depth, surface and albedo must be there).
    Status draw_gi(rhi::Frame& frame) {
        const std::uint32_t gw = std::max(1u, (frame.width + 1) / 2), gh = std::max(1u, (frame.height + 1) / 2);
        bool regroup = !gi_bgs[0] || gi_bg_views[0] != prepass_view || gi_bg_views[1] != surface_view || gi_bg_views[2] != hdr_view || gi_bg_views[3] != albedo_view;
        if (!gi_tex[0] || gi_w != gw || gi_h != gh) {
            for (int k = 0; k < 2; ++k) {
                if (gi_view[k]) wgpuTextureViewRelease(gi_view[k]);
                if (gi_tex[k]) wgpuTextureRelease(gi_tex[k]);
                auto [t, v] = make_target("pocket.gi", gw, gh, kHdrFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
                if (!t) return fail("gpu_texture_failed", "cannot create the global illumination target {}x{}", gw, gh);
                gi_tex[k] = t;
                gi_view[k] = v;
            }
            gi_w = gw;
            gi_h = gh;
            gi_valid = false;
            regroup = true;
        }
        if (regroup) {
            // Gathering into k reads last frame's from the other and the frame; adding into the frame
            // reads this frame's (k), and in the frame's place the other (unused there: a texture
            // drawn into may not be bound in the same pass).
            auto group = [&](WGPUTextureView light, WGPUTextureView scene) {
                WGPUBindGroupEntry e[6]{};
                e[0].binding = 20;
                e[0].buffer = gi_uniforms;
                e[0].size = sizeof(float) * 8;
                const WGPUTextureView views[5] = {prepass_view, surface_view, scene, light, albedo_view};
                for (int i = 0; i < 5; ++i) {
                    e[i + 1].binding = static_cast<std::uint32_t>(21 + i);
                    e[i + 1].textureView = views[i];
                }
                WGPUBindGroupDescriptor d{};
                d.label = rhi::str("pocket.gi");
                d.layout = gi_bgl;
                d.entryCount = 6;
                d.entries = e;
                return wgpuDeviceCreateBindGroup(device->device(), &d);
            };
            for (WGPUBindGroup g : {gi_bgs[0], gi_bgs[1], gi_add_bgs[0], gi_add_bgs[1]}) if (g) wgpuBindGroupRelease(g);
            for (int i = 0; i < 2; ++i) {
                gi_bgs[i] = group(gi_view[1 - i], hdr_view);
                gi_add_bgs[i] = group(gi_view[i], gi_view[1 - i]);
            }
            gi_bg_views[0] = prepass_view;
            gi_bg_views[1] = surface_view;
            gi_bg_views[2] = hdr_view;
            gi_bg_views[3] = albedo_view;
        }
        const int k = 1 - gi_cur;   // write into the one not holding last frame's
        // A frame without last frame's to blend with (the first, after a cut) sends four times the
        // rays, so it looks as the blend would.
        const float rays = static_cast<float>(gi_valid ? ssgi.rays : std::min(ssgi.rays * 4, 16));
        const float u[8] = {ssgi.distance, ssgi.intensity, static_cast<float>(ssgi.steps), ssgi.thickness,
                            rays, static_cast<float>(gi_frame++ % 65536), gi_valid ? 1.0f : 0.0f, 0.1f};
        device->write_buffer(gi_uniforms, 0, u, sizeof u);
        WGPURenderPassColorAttachment ca{};
        ca.view = gi_view[k];
        ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ca.loadOp = WGPULoadOp_Clear;
        ca.storeOp = WGPUStoreOp_Store;
        ca.clearValue = {0, 0, 0, -1};
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str("pocket.gi");
        rp.colorAttachmentCount = 1;
        rp.colorAttachments = &ca;
        WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
        wgpuRenderPassEncoderSetPipeline(enc, gi_pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(enc, 1, gi_bgs[k], 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        WGPURenderPassColorAttachment aca{};
        aca.view = hdr_view;
        aca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        aca.loadOp = WGPULoadOp_Load;
        aca.storeOp = WGPUStoreOp_Store;
        WGPURenderPassDescriptor arp{};
        arp.label = rhi::str("pocket.gi.add");
        arp.colorAttachmentCount = 1;
        arp.colorAttachments = &aca;
        enc = begin_pass(frame.encoder, arp);
        wgpuRenderPassEncoderSetPipeline(enc, gi_add_pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(enc, 1, gi_add_bgs[k], 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        gi_cur = k;
        gi_valid = true;
        stats.ssgi = true;
        stats.draw_calls += 2;
        return {};
    }

    // March the fog into the half-size target (the prepass depth must be there).
    Status draw_volume(rhi::Frame& frame, const world::Fog& fog) {
        const std::uint32_t vw = std::max(1u, (frame.width + 1) / 2), vh = std::max(1u, (frame.height + 1) / 2);
        if (!volume_tex[0] || volume_w != vw || volume_h != vh) {
            for (int k = 0; k < 2; ++k) {
                if (volume_view[k]) wgpuTextureViewRelease(volume_view[k]);
                if (volume_tex[k]) wgpuTextureRelease(volume_tex[k]);
                auto [t, v] = make_target("pocket.volume", vw, vh, kHdrFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
                if (!t) return fail("gpu_texture_failed", "cannot create the volumetric fog target {}x{}", vw, vh);
                volume_tex[k] = t;
                volume_view[k] = v;
            }
            volume_w = vw;
            volume_h = vh;
            volume_valid = false;
            volume_depth = nullptr;
            post_volume = nullptr;
        }
        if (!volume_bgs[0] || volume_depth != prepass_view) {
            // One group per direction of the turn: write into k, read last frame's from the other.
            for (int k = 0; k < 2; ++k) {
                if (volume_bgs[k]) wgpuBindGroupRelease(volume_bgs[k]);
                WGPUBindGroupEntry e[4]{};
                e[0].binding = 8;
                e[0].buffer = volume_uniforms;
                e[0].size = sizeof(VolumeUniforms);
                e[1].binding = 9;
                e[1].textureView = prepass_view;
                e[2].binding = 10;
                e[2].textureView = volume_view[1 - k];
                e[3].binding = 11;
                e[3].sampler = volume_samp;
                WGPUBindGroupDescriptor d{};
                d.label = rhi::str("pocket.volume");
                d.layout = volume_bgl;
                d.entryCount = 4;
                d.entries = e;
                volume_bgs[k] = wgpuDeviceCreateBindGroup(device->device(), &d);
            }
            volume_depth = prepass_view;
        }
        const int k = 1 - volume_cur;   // write into the one not holding last frame's
        VolumeUniforms u{};
        u.medium[0] = std::max(fog.density, 0.0f);
        u.medium[1] = fog.height;
        u.medium[2] = std::max(fog.falloff, 0.0f);
        u.medium[3] = std::clamp(fog.anisotropy, -0.9f, 0.9f);
        u.albedo[0] = decode(fog.color.r);
        u.albedo[1] = decode(fog.color.g);
        u.albedo[2] = decode(fog.color.b);
        u.albedo[3] = std::clamp(fog.max_opacity, 0.0f, 1.0f);
        // A frame with last frame's to blend with marches the steps asked for; one without (the
        // first, after a cut, an agent's capture after an undrawn step) four times as many, so it
        // looks as the blend would.
        const int steps = std::clamp(fog.steps, 4, 128);
        u.march[0] = static_cast<float>(volume_valid ? steps : std::min(steps * 4, 128));
        u.march[1] = std::max(fog.distance, 0.1f);
        u.march[2] = std::max(fog.start, 0.0f);
        u.march[3] = std::fmod(static_cast<float>(volume_frame++ % 64) * 0.618034f, 1.0f);   // golden-ratio steps through the jitter
        u.target_size[0] = static_cast<float>(vw);
        u.target_size[1] = static_cast<float>(vh);
        u.history[0] = volume_valid ? 1.0f : 0.0f;
        u.history[1] = 0.25f;
        device->write_buffer(volume_uniforms, 0, &u, sizeof u);
        WGPURenderPassColorAttachment ca{};
        ca.view = volume_view[k];
        ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ca.loadOp = WGPULoadOp_Clear;
        ca.storeOp = WGPUStoreOp_Store;
        ca.clearValue = {0, 0, 0, 1};
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str("pocket.volume");
        rp.colorAttachmentCount = 1;
        rp.colorAttachments = &ca;
        WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
        wgpuRenderPassEncoderSetPipeline(enc, volume_pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(enc, 1, volume_bgs[k], 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
        volume_cur = k;
        volume_valid = true;
        stats.draw_calls++;
        return {};
    }

    // The depth prepass target at the frame's size.
    Status ensure_prepass(std::uint32_t w, std::uint32_t h) {
        if (prepass_tex && prepass_w == w && prepass_h == h) return {};
        if (prepass_view) wgpuTextureViewRelease(prepass_view);
        if (prepass_tex) wgpuTextureRelease(prepass_tex);
        if (velocity_view) wgpuTextureViewRelease(velocity_view);
        if (velocity_tex) wgpuTextureRelease(velocity_tex);
        for (WGPUTextureView tv : {surface_view, albedo_view}) if (tv) wgpuTextureViewRelease(tv);
        for (WGPUTexture tt : {surface_tex, albedo_tex}) if (tt) wgpuTextureRelease(tt);
        taa_valid = false;
        auto [t, v] = make_target("pocket.prepass", w, h, kPrepassDepth, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopySrc);
        if (!t) return fail("gpu_texture_failed", "cannot create the depth prepass {}x{}", w, h);
        prepass_tex = t;
        prepass_view = v;
        auto [vt, vv] = make_target("pocket.velocity", w, h, kVelocityFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
        if (!vt) return fail("gpu_texture_failed", "cannot create the motion target {}x{}", w, h);
        velocity_tex = vt;
        velocity_view = vv;
        auto [st, sv] = make_target("pocket.surface", w, h, kSurfaceFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
        auto [at, av] = make_target("pocket.albedo", w, h, kAlbedoFormat, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
        if (!st || !at) return fail("gpu_texture_failed", "cannot create the surface targets {}x{}", w, h);
        surface_tex = st;
        surface_view = sv;
        albedo_tex = at;
        albedo_view = av;
        prepass_w = w;
        prepass_h = h;
        release_ao_targets();   // their groups read the old depth
        post_depth = nullptr;   // and so does the post group
        return {};
    }

    // The AO targets at half the frame, and their groups over the prepass depth.
    Status ensure_ao_targets(std::uint32_t w, std::uint32_t h) {
        const std::uint32_t aw = std::max(1u, (w + 1) / 2), ah = std::max(1u, (h + 1) / 2);
        if (ao_tex[0] && ao_w == aw && ao_h == ah) return {};
        release_ao_targets();
        for (int i = 0; i < 2; ++i) {
            auto [t, v] = make_target(i == 0 ? "pocket.ao.a" : "pocket.ao.b", aw, ah, i == 0 ? kAoRawFormat : WGPUTextureFormat_RG8Unorm, WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding);
            if (!t) return fail("gpu_texture_failed", "cannot create the AO target {}x{}", aw, ah);
            ao_tex[i] = t;
            ao_view[i] = v;
        }
        for (int i = 0; i < 2; ++i) {
            WGPUBindGroupEntry e[3]{};
            e[0].binding = 0;
            e[0].buffer = ao_uniforms;
            e[0].size = sizeof(AoUniforms);
            e[1].binding = 1;
            e[1].textureView = prepass_view;
            e[2].binding = 2;
            e[2].textureView = i == 0 ? ao_white_view : ao_view[0];
            WGPUBindGroupDescriptor d{};
            d.label = rhi::str("pocket.ao");
            d.layout = ao_bgl;
            d.entryCount = 3;
            d.entries = e;
            ao_bg[i] = wgpuDeviceCreateBindGroup(device->device(), &d);
        }
        ao_w = aw;
        ao_h = ah;
        scene_ao = nullptr;   // the scene group reads the new result
        return {};
    }

    // The scene group reads the AO result (or the white texel); made again when that changes.
    void make_scene_group(WGPUTextureView ao_view_now) {
        WGPUTextureView atlas_now = atlas_view ? atlas_view : atlas_stub_view;
        WGPUTextureView glass_now = glass_view ? glass_view : glass_stub_view;
        WGPUTextureView occ_now = occ_view ? occ_view : occ_stub_view;
        if (scene_bg && scene_ao == ao_view_now && scene_atlas == atlas_now && scene_glass == glass_now && scene_occ == occ_now) return;
        if (scene_bg) wgpuBindGroupRelease(scene_bg);
        scene_entries[18].binding = 18;
        scene_entries[18].textureView = occ_now;
        scene_occ = occ_now;
        scene_entries[17].binding = 17;
        scene_entries[17].textureView = glass_now;
        scene_glass = glass_now;
        scene_entries[11].binding = 11;
        scene_entries[11].textureView = atlas_now;
        scene_atlas = atlas_now;
        scene_entries[6].binding = 6;
        scene_entries[6].textureView = ao_view_now;
        scene_entries[7].binding = 7;
        scene_entries[7].sampler = bloom_sampler;
        WGPUBindGroupDescriptor sbd{};
        sbd.label = rhi::str("pocket.scene");
        sbd.layout = scene_bgl;
        sbd.entryCount = 19;
        sbd.entries = scene_entries;
        scene_bg = wgpuDeviceCreateBindGroup(device->device(), &sbd);
        scene_ao = ao_view_now;
    }

    // The frame-sized copy of the scene the glass shows through (made again with the frame's size).
    Status ensure_glass_target(std::uint32_t w, std::uint32_t h) {
        if (glass_tex && glass_w == w && glass_h == h) return {};
        if (glass_view) wgpuTextureViewRelease(glass_view);
        if (glass_tex) wgpuTextureRelease(glass_tex);
        auto [t, v] = make_target("pocket.glass.scene", w, h, kHdrFormat, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
        if (!t) return fail("gpu_texture_failed", "cannot create the glass copy {}x{}", w, h);
        glass_tex = t;
        glass_view = v;
        glass_w = w;
        glass_h = h;
        return {};
    }

    // Ambient occlusion over the prepass depth: the raw pass into A, the blur into B.
    void draw_ao(rhi::Frame& frame) {
        AoUniforms u{};
        to_array(camera.proj * camera.view, u.view_proj);
        to_array((camera.proj * camera.view).inverse(), u.inv_view_proj);
        u.camera[0] = camera.position.x; u.camera[1] = camera.position.y; u.camera[2] = camera.position.z;
        u.fwd[0] = camera.forward.x; u.fwd[1] = camera.forward.y; u.fwd[2] = camera.forward.z;
        u.params[0] = ao.radius;
        u.params[1] = ao.intensity;
        u.params[2] = static_cast<float>(ao.samples);
        u.size[0] = static_cast<float>(frame.width);
        u.size[1] = static_cast<float>(frame.height);
        u.sun[0] = sun_toward.x; u.sun[1] = sun_toward.y; u.sun[2] = sun_toward.z;
        u.sun[3] = contact_now ? 1.0f : 0.0f;
        u.contact[0] = shadows.contact_length;
        u.contact[1] = ao.enabled ? 1.0f : 0.0f;
        device->write_buffer(ao_uniforms, 0, &u, sizeof u);
        for (int i = 0; i < 2; ++i) {
            WGPURenderPassColorAttachment ca{};
            ca.view = ao_view[i];
            ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
            ca.loadOp = WGPULoadOp_Clear;
            ca.storeOp = WGPUStoreOp_Store;
            ca.clearValue = {1, 1, 1, 1};
            WGPURenderPassDescriptor rp{};
            rp.label = rhi::str(i == 0 ? "pocket.ao" : "pocket.ao.blur");
            rp.colorAttachmentCount = 1;
            rp.colorAttachments = &ca;
            WGPURenderPassEncoder enc = begin_pass(frame.encoder, rp);
            wgpuRenderPassEncoderSetPipeline(enc, i == 0 ? ao_pipeline : ao_blur_pipeline);
            wgpuRenderPassEncoderSetBindGroup(enc, 0, ao_bg[i], 0, nullptr);
            wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
            wgpuRenderPassEncoderEnd(enc);
            wgpuRenderPassEncoderRelease(enc);
        }
        stats.ao = ao.enabled;
    }

    void release_scene_pipelines() {
        if (sky_pipeline) wgpuRenderPipelineRelease(sky_pipeline);
        sky_pipeline = nullptr;
        for (WGPURenderPipeline* p : {&pipeline, &skinned_pipeline, &cut_pipeline, &cut_skinned_pipeline, &blend_pipeline, &blend_skinned_pipeline, &sprite_pipeline, &sprite_add_pipeline, &particle_pipeline, &particle_add_pipeline, &weather_pipeline, &sprite_lit_pipeline, &line_pipeline, &id_pipeline, &id_skinned_pipeline, &id_cut_pipeline, &id_cut_skinned_pipeline, &id_sprite_pipeline}) {
            if (*p) wgpuRenderPipelineRelease(*p);
            *p = nullptr;
        }
    }

    // The scene pipelines for a sample count: with one sample, color and id share a pass (two
    // targets); with more, the color pipelines are multisampled with one target and separate
    // single-sample pipelines write the ids.
    Status create_scene_pipelines(int samples, bool prepass = false) {
        release_scene_pipelines();
        const bool split = samples > 1 || prepass;
        WGPUColorTargetState targets[2]{};
        targets[0].format = kHdrFormat;
        targets[0].writeMask = WGPUColorWriteMask_All;
        targets[1].format = WGPUTextureFormat_R32Uint;
        targets[1].writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str(split ? "fs_color" : "fs");
        fs.targetCount = split ? 1 : 2;
        fs.targets = targets;
        WGPUDepthStencilState ds{};
        ds.format = device->depth_format();
        ds.depthWriteEnabled = WGPUOptionalBool_True;
        ds.depthCompare = WGPUCompareFunction_Less;
        ds.stencilFront.compare = WGPUCompareFunction_Always;
        ds.stencilBack.compare = WGPUCompareFunction_Always;
        ds.stencilReadMask = 0xFFFFFFFF;
        ds.stencilWriteMask = 0xFFFFFFFF;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.mesh");
        rpd.layout = layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_Back;
        rpd.depthStencil = &ds;
        rpd.multisample.count = static_cast<std::uint32_t>(samples);
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!pipeline) return fail("gpu_pipeline_failed", "mesh pipeline creation failed");
        // Cut-outs: the same shading with its discard, drawn after the solid meshes.
        fs.entryPoint = rhi::str(split ? "fs_color_cut" : "fs_cut");
        rpd.label = rhi::str("pocket.mesh.cut");
        cut_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!cut_pipeline) return fail("gpu_pipeline_failed", "cut-out pipeline creation failed");
        // Skinned meshes: the same lit fragment, a vertex stage that blends joint matrices.
        rpd.label = rhi::str("pocket.mesh.skinned.cut");
        rpd.vertex.entryPoint = rhi::str("vs_skinned");
        rpd.vertex.bufferCount = 2;
        rpd.vertex.buffers = vbls;
        cut_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!cut_skinned_pipeline) return fail("gpu_pipeline_failed", "skinned cut-out pipeline creation failed");
        fs.entryPoint = rhi::str(split ? "fs_color" : "fs");
        rpd.label = rhi::str("pocket.mesh.skinned");
        skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!skinned_pipeline) return fail("gpu_pipeline_failed", "skinned pipeline creation failed");
        fs.entryPoint = rhi::str(split ? "fs_color_cut" : "fs_cut");   // a translucent mesh may be cut out too
        // Translucent meshes: the same lit shading blended over what is behind, depth tested but
        // not written (so they never hide each other), drawn after the opaque ones far to near.
        WGPUBlendState mesh_blend{};
        mesh_blend.color = {WGPUBlendOperation_Add, WGPUBlendFactor_SrcAlpha, WGPUBlendFactor_OneMinusSrcAlpha};
        mesh_blend.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_One, WGPUBlendFactor_OneMinusSrcAlpha};
        targets[0].blend = &mesh_blend;
        ds.depthWriteEnabled = WGPUOptionalBool_False;
        ds.depthCompare = WGPUCompareFunction_LessEqual;
        rpd.label = rhi::str("pocket.mesh.blend.skinned");
        blend_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!blend_skinned_pipeline) return fail("gpu_pipeline_failed", "translucent skinned pipeline creation failed");
        rpd.label = rhi::str("pocket.mesh.blend");
        rpd.vertex.entryPoint = rhi::str("vs");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        blend_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!blend_pipeline) return fail("gpu_pipeline_failed", "translucent pipeline creation failed");
        targets[0].blend = nullptr;
        ds.depthWriteEnabled = WGPUOptionalBool_True;
        ds.depthCompare = WGPUCompareFunction_Less;
        rpd.vertex.entryPoint = rhi::str("vs");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        // Sprites: same layout and vertex path, unlit fragment, alpha blend, no depth writes,
        // both faces (a sprite seen from behind is still a sprite).
        WGPUBlendState blend{};
        blend.color = {WGPUBlendOperation_Add, WGPUBlendFactor_SrcAlpha, WGPUBlendFactor_OneMinusSrcAlpha};
        blend.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_One, WGPUBlendFactor_OneMinusSrcAlpha};
        targets[0].blend = &blend;
        fs.entryPoint = rhi::str(split ? "fs_unlit_color" : "fs_unlit");
        ds.depthWriteEnabled = WGPUOptionalBool_False;
        ds.depthCompare = WGPUCompareFunction_LessEqual;
        rpd.label = rhi::str("pocket.sprite");
        rpd.primitive.cullMode = WGPUCullMode_None;
        sprite_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!sprite_pipeline) return fail("gpu_pipeline_failed", "sprite pipeline creation failed");
        // Additive: the color times its alpha added to what is there; the alpha left as it was.
        WGPUBlendState add{};
        add.color = {WGPUBlendOperation_Add, WGPUBlendFactor_SrcAlpha, WGPUBlendFactor_One};
        add.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_Zero, WGPUBlendFactor_One};
        targets[0].blend = &add;
        rpd.label = rhi::str("pocket.sprite.add");
        sprite_add_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!sprite_add_pipeline) return fail("gpu_pipeline_failed", "additive sprite pipeline creation failed");
        // GPU particles: the sprites' states, their quads made from the ring (no vertex buffer).
        {
            WGPURenderPipelineDescriptor prpd = rpd;
            WGPUFragmentState pfs = fs;
            pfs.entryPoint = rhi::str(split ? "fs_particle_color" : "fs_particle");
            prpd.fragment = &pfs;
            prpd.layout = particle_layout;
            prpd.vertex.entryPoint = rhi::str("vs_particle");
            prpd.vertex.bufferCount = 0;
            prpd.vertex.buffers = nullptr;
            prpd.label = rhi::str("pocket.particles.add");
            particle_add_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &prpd);
            targets[0].blend = &blend;
            prpd.label = rhi::str("pocket.particles");
            particle_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &prpd);
            if (!particle_pipeline || !particle_add_pipeline) return fail("gpu_pipeline_failed", "GPU particle pipeline creation failed");
        }
        // Rain and snow: the sprites' blending and depth test, quads made from each drop's number
        // (no vertex buffer), the ids behind them left as they were.
        {
            WGPURenderPipelineDescriptor wrpd = rpd;
            WGPUColorTargetState wtargets[2] = {targets[0], targets[1]};
            wtargets[0].blend = &blend;
            wtargets[1].writeMask = WGPUColorWriteMask_None;
            WGPUFragmentState wfs = fs;
            wfs.entryPoint = rhi::str(split ? "fs_weather_color" : "fs_weather");
            wfs.targets = wtargets;
            wrpd.fragment = &wfs;
            wrpd.vertex.entryPoint = rhi::str("vs_weather");
            wrpd.vertex.bufferCount = 0;
            wrpd.vertex.buffers = nullptr;
            wrpd.label = rhi::str("pocket.weather");
            weather_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &wrpd);
            if (!weather_pipeline) return fail("gpu_pipeline_failed", "weather pipeline creation failed");
        }
        targets[0].blend = &blend;
        // Lit sprites and tile maps: the meshes' shading (the lights, the sun, the ambient and a
        // normal map) over the sprite's state, alpha blended.
        fs.entryPoint = rhi::str(split ? "fs_color" : "fs");
        rpd.label = rhi::str("pocket.sprite.lit");
        sprite_lit_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!sprite_lit_pipeline) return fail("gpu_pipeline_failed", "lit sprite pipeline creation failed");
        // Debug lines: their own tiny shader over the frame uniform, alpha blended, depth tested
        // without writing, and no id writes (a line over an entity leaves its id in place).
        {
            WGPUVertexAttribute lattrs[2]{};
            lattrs[0].format = WGPUVertexFormat_Float32x3;
            lattrs[0].offset = 0;
            lattrs[0].shaderLocation = 0;
            lattrs[1].format = WGPUVertexFormat_Float32x4;
            lattrs[1].offset = sizeof(float) * 3;
            lattrs[1].shaderLocation = 1;
            WGPUVertexBufferLayout lvbl{};
            lvbl.stepMode = WGPUVertexStepMode_Vertex;
            lvbl.arrayStride = sizeof(DebugVertex);
            lvbl.attributeCount = 2;
            lvbl.attributes = lattrs;
            WGPUColorTargetState ltargets[2] = {targets[0], targets[1]};
            ltargets[1].writeMask = WGPUColorWriteMask_None;
            WGPUFragmentState lfs{};
            lfs.module = line_shader;
            lfs.entryPoint = rhi::str("fs");
            lfs.targetCount = split ? 1 : 2;
            lfs.targets = ltargets;
            WGPURenderPipelineDescriptor lrpd{};
            lrpd.label = rhi::str("pocket.lines");
            lrpd.layout = line_layout;
            lrpd.vertex.module = line_shader;
            lrpd.vertex.entryPoint = rhi::str("vs");
            lrpd.vertex.bufferCount = 1;
            lrpd.vertex.buffers = &lvbl;
            lrpd.primitive.topology = WGPUPrimitiveTopology_LineList;
            lrpd.primitive.frontFace = WGPUFrontFace_CCW;
            lrpd.primitive.cullMode = WGPUCullMode_None;
            lrpd.depthStencil = &ds;  // LessEqual, no writes (the sprite settings)
            lrpd.multisample.count = static_cast<std::uint32_t>(samples);
            lrpd.multisample.mask = 0xFFFFFFFFu;
            lrpd.fragment = &lfs;
            line_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &lrpd);
            if (!line_pipeline) return fail("gpu_pipeline_failed", "line pipeline creation failed");
        }
        if (split) {
            // The id pass: one R32Uint target, one sample, its own depth test, the same vertex paths.
            WGPUColorTargetState idt[4]{};
            idt[0].format = WGPUTextureFormat_R32Uint;
            idt[0].writeMask = WGPUColorWriteMask_All;
            idt[1].format = kVelocityFormat;   // the motion TAA follows
            idt[1].writeMask = WGPUColorWriteMask_All;
            idt[2].format = kSurfaceFormat;    // what screen-space reflections read
            idt[2].writeMask = WGPUColorWriteMask_All;
            idt[3].format = kAlbedoFormat;
            idt[3].writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState ifs{};
            ifs.module = shader;
            ifs.entryPoint = rhi::str("fs_id");
            ifs.targetCount = 4;
            ifs.targets = idt;
            WGPUDepthStencilState ids = ds;
            ids.format = kPrepassDepth;   // the id pass has a depth target of its own, sampled afterwards
            ids.depthWriteEnabled = WGPUOptionalBool_True;
            ids.depthCompare = WGPUCompareFunction_Less;
            WGPURenderPipelineDescriptor irpd = rpd;
            irpd.label = rhi::str("pocket.ids.mesh");
            irpd.fragment = &ifs;
            irpd.depthStencil = &ids;
            irpd.multisample.count = 1;
            irpd.primitive.cullMode = WGPUCullMode_Back;
            irpd.vertex.entryPoint = rhi::str("vs");
            irpd.vertex.bufferCount = 1;
            irpd.vertex.buffers = &vbl;
            id_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &irpd);
            if (!id_pipeline) return fail("gpu_pipeline_failed", "id pipeline creation failed");
            ifs.entryPoint = rhi::str("fs_id_cut");
            irpd.label = rhi::str("pocket.ids.cut");
            id_cut_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &irpd);
            if (!id_cut_pipeline) return fail("gpu_pipeline_failed", "cut-out id pipeline creation failed");
            irpd.label = rhi::str("pocket.ids.skinned.cut");
            irpd.vertex.entryPoint = rhi::str("vs_skinned");
            irpd.vertex.bufferCount = 2;
            irpd.vertex.buffers = vbls;
            id_cut_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &irpd);
            if (!id_cut_skinned_pipeline) return fail("gpu_pipeline_failed", "skinned cut-out id pipeline creation failed");
            ifs.entryPoint = rhi::str("fs_id");
            irpd.label = rhi::str("pocket.ids.skinned");
            id_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &irpd);
            if (!id_skinned_pipeline) return fail("gpu_pipeline_failed", "skinned id pipeline creation failed");
            irpd.label = rhi::str("pocket.ids.sprite");
            ifs.entryPoint = rhi::str("fs_unlit_id");
            ids.depthWriteEnabled = WGPUOptionalBool_False;
            ids.depthCompare = WGPUCompareFunction_LessEqual;
            irpd.primitive.cullMode = WGPUCullMode_None;
            irpd.vertex.entryPoint = rhi::str("vs");
            irpd.vertex.bufferCount = 1;
            irpd.vertex.buffers = &vbl;
            id_sprite_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &irpd);
            if (!id_sprite_pipeline) return fail("gpu_pipeline_failed", "sprite id pipeline creation failed");
        }
        {
            // The sky: first in the scene pass, never tested against or written into depth.
            WGPUColorTargetState kt[2]{};
            kt[0].format = kHdrFormat;
            kt[0].writeMask = WGPUColorWriteMask_All;
            kt[1].format = WGPUTextureFormat_R32Uint;
            kt[1].writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState kfs{};
            kfs.module = shader;
            kfs.entryPoint = rhi::str(split ? "fs_sky_color" : "fs_sky");
            kfs.targetCount = split ? 1 : 2;
            kfs.targets = kt;
            WGPUDepthStencilState kds = ds;
            kds.depthWriteEnabled = WGPUOptionalBool_False;
            kds.depthCompare = WGPUCompareFunction_Always;
            WGPURenderPipelineDescriptor k{};
            k.label = rhi::str("pocket.sky");
            k.layout = sky_layout;
            k.vertex.module = shader;
            k.vertex.entryPoint = rhi::str("vs_sky");
            k.primitive.topology = WGPUPrimitiveTopology_TriangleList;
            k.primitive.frontFace = WGPUFrontFace_CCW;
            k.primitive.cullMode = WGPUCullMode_None;
            k.depthStencil = &kds;
            k.multisample.count = static_cast<std::uint32_t>(samples);
            k.multisample.mask = 0xFFFFFFFFu;
            k.fragment = &kfs;
            sky_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &k);
            if (!sky_pipeline) return fail("gpu_pipeline_failed", "sky pipeline creation failed");
        }
        msaa_applied = samples;
        split_applied = split;
        return {};
    }

    Status ensure_id_target(std::uint32_t w, std::uint32_t h) {
        if (id_texture && id_width == w && id_height == h) return {};
        if (id_view) { wgpuTextureViewRelease(id_view); id_view = nullptr; }
        if (id_texture) { wgpuTextureRelease(id_texture); id_texture = nullptr; }
        post_ids = nullptr;   // the post group reads it
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.ids");
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_CopySrc | WGPUTextureUsage_TextureBinding;   // the toon look's outlines read it
        td.dimension = WGPUTextureDimension_2D;
        td.size = {w, h, 1};
        td.format = WGPUTextureFormat_R32Uint;
        td.mipLevelCount = 1;
        td.sampleCount = 1;
        id_texture = wgpuDeviceCreateTexture(device->device(), &td);
        if (!id_texture) return fail("gpu_texture_failed", "cannot create id target");
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = 1;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        id_view = wgpuTextureCreateView(id_texture, &vd);
        id_width = w;
        id_height = h;
        return {};
    }

    Status init() {
        POCKET_TRY(module, device->create_shader("pocket.mesh", kMeshWgsl));
        shader = module;

        WGPUBindGroupLayoutEntry fe{};
        fe.binding = 0;
        fe.visibility = WGPUShaderStage_Vertex | WGPUShaderStage_Fragment;
        fe.buffer.type = WGPUBufferBindingType_Uniform;
        fe.buffer.minBindingSize = sizeof(FrameUniforms);
        WGPUBindGroupLayoutDescriptor fd{};
        fd.label = rhi::str("pocket.frame");
        fd.entryCount = 1;
        fd.entries = &fe;
        frame_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &fd);
        WGPUBindGroupLayoutEntry se[19]{};
        se[0] = fe;
        se[1].binding = 1;
        se[1].visibility = WGPUShaderStage_Fragment;
        se[1].texture.sampleType = WGPUTextureSampleType_Depth;
        se[1].texture.viewDimension = WGPUTextureViewDimension_2DArray;
        se[2].binding = 2;
        se[2].visibility = WGPUShaderStage_Fragment;
        se[2].sampler.type = WGPUSamplerBindingType_Comparison;
        se[3].binding = 3;
        se[3].visibility = WGPUShaderStage_Fragment;
        se[3].texture.sampleType = WGPUTextureSampleType_Float;
        se[3].texture.viewDimension = WGPUTextureViewDimension_2D;
        se[4].binding = 4;
        se[4].visibility = WGPUShaderStage_Fragment;
        se[4].sampler.type = WGPUSamplerBindingType_Filtering;
        // The small fixed arrays (the sky's harmonics, the shadow faces, the decals) are uniforms:
        // a fragment stage may read only four storage buffers on some GPUs (the iOS Simulator's),
        // and lights, clusters, probes and objects take them.
        se[5].binding = 5;
        se[5].visibility = WGPUShaderStage_Fragment;
        se[5].buffer.type = WGPUBufferBindingType_Uniform;
        se[5].buffer.minBindingSize = sizeof(float) * 36;
        se[6].binding = 6;
        se[6].visibility = WGPUShaderStage_Fragment;
        se[6].texture.sampleType = WGPUTextureSampleType_Float;
        se[6].texture.viewDimension = WGPUTextureViewDimension_2D;
        se[7].binding = 7;
        se[7].visibility = WGPUShaderStage_Fragment;
        se[7].sampler.type = WGPUSamplerBindingType_Filtering;
        for (std::uint32_t b = 8; b < 10; ++b) {
            se[b].binding = b;
            se[b].visibility = WGPUShaderStage_Fragment;
            se[b].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        }
        se[8].buffer.minBindingSize = sizeof(GpuLight);
        se[9].buffer.minBindingSize = sizeof(std::uint32_t) * 2 * kClusters;
        se[10].binding = 10;
        se[10].visibility = WGPUShaderStage_Fragment;
        se[10].buffer.type = WGPUBufferBindingType_Uniform;
        se[10].buffer.minBindingSize = sizeof(GpuFace) * kMaxFaces;
        se[11].binding = 11;
        se[11].visibility = WGPUShaderStage_Fragment;
        se[11].texture.sampleType = WGPUTextureSampleType_Depth;
        se[11].texture.viewDimension = WGPUTextureViewDimension_2D;
        se[12].binding = 12;
        se[12].visibility = WGPUShaderStage_Fragment;
        se[12].texture.sampleType = WGPUTextureSampleType_Float;
        se[12].texture.viewDimension = WGPUTextureViewDimension_2DArray;
        se[13].binding = 13;
        se[13].visibility = WGPUShaderStage_Fragment;
        se[13].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        se[13].buffer.minBindingSize = sizeof(float) * 4 * 16 * kMaxProbes;
        se[14].binding = 14;
        se[14].visibility = WGPUShaderStage_Fragment;
        se[14].buffer.type = WGPUBufferBindingType_Uniform;
        se[14].buffer.minBindingSize = sizeof(GpuDecal) * kMaxDecals;
        se[15].binding = 15;
        se[15].visibility = WGPUShaderStage_Fragment;
        se[15].texture.sampleType = WGPUTextureSampleType_Float;
        se[15].texture.viewDimension = WGPUTextureViewDimension_2DArray;
        se[16].binding = 16;
        se[16].visibility = WGPUShaderStage_Fragment;
        se[16].sampler.type = WGPUSamplerBindingType_Filtering;
        se[17] = se[3];
        se[17].binding = 17;
        se[18].binding = 18;
        se[18].visibility = WGPUShaderStage_Fragment;
        se[18].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        se[18].texture.viewDimension = WGPUTextureViewDimension_2D;
        WGPUBindGroupLayoutDescriptor scene_ld{};
        scene_ld.label = rhi::str("pocket.scene");
        scene_ld.entryCount = 19;
        scene_ld.entries = se;
        scene_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &scene_ld);

        WGPUBindGroupLayoutEntry oe[3]{};
        oe[0].binding = 0;
        oe[0].visibility = WGPUShaderStage_Vertex | WGPUShaderStage_Fragment;
        oe[0].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        oe[0].buffer.hasDynamicOffset = false;
        oe[0].buffer.minBindingSize = sizeof(ObjectUniforms);
        oe[1].binding = 1;
        oe[1].visibility = WGPUShaderStage_Vertex;
        oe[1].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        oe[1].buffer.hasDynamicOffset = false;
        oe[1].buffer.minBindingSize = sizeof(float) * 16;
        oe[2].binding = 2;
        oe[2].visibility = WGPUShaderStage_Vertex;
        oe[2].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        oe[2].buffer.hasDynamicOffset = false;
        oe[2].buffer.minBindingSize = sizeof(float) * 4;
        WGPUBindGroupLayoutDescriptor od{};
        od.label = rhi::str("pocket.object");
        od.entryCount = 3;
        od.entries = oe;
        object_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &od);

        WGPUBindGroupLayoutEntry me[7]{};
        me[0].binding = 0;
        me[0].visibility = WGPUShaderStage_Fragment;
        me[0].texture.sampleType = WGPUTextureSampleType_Float;
        me[0].texture.viewDimension = WGPUTextureViewDimension_2D;
        me[1].binding = 1;
        me[1].visibility = WGPUShaderStage_Fragment;
        me[1].sampler.type = WGPUSamplerBindingType_Filtering;
        for (std::uint32_t b = 2; b < 5; ++b) {
            me[b] = me[0];
            me[b].binding = b;
        }
        // A terrain's splat map and layer array (docs/design/terrain.md, Layers); stand-ins elsewhere.
        me[5] = me[0];
        me[5].binding = 6;
        me[6] = me[0];
        me[6].binding = 7;
        me[6].texture.viewDimension = WGPUTextureViewDimension_2DArray;
        WGPUBindGroupLayoutDescriptor md{};
        md.label = rhi::str("pocket.material");
        md.entryCount = 7;
        md.entries = me;
        material_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &md);

        WGPUBindGroupLayout bgls[3] = {scene_bgl, object_bgl, material_bgl};
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.mesh");
        pld.bindGroupLayoutCount = 3;
        pld.bindGroupLayouts = bgls;
        layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
        {
            // GPU particles: the ring and its emitter's uniform, read where the quads are made
            // (group 3, beside the meshes' three) and written by the compute pass.
            WGPUBindGroupLayoutEntry pe[3]{};
            pe[0].binding = 2;
            pe[0].visibility = WGPUShaderStage_Vertex;
            pe[0].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
            pe[1].binding = 3;
            pe[1].visibility = WGPUShaderStage_Vertex | WGPUShaderStage_Fragment;
            pe[1].buffer.type = WGPUBufferBindingType_Uniform;
            pe[2].binding = 4;
            pe[2].visibility = WGPUShaderStage_Fragment;
            pe[2].texture.sampleType = WGPUTextureSampleType_Depth;
            pe[2].texture.viewDimension = WGPUTextureViewDimension_2D;
            WGPUBindGroupLayoutDescriptor pd{};
            pd.label = rhi::str("pocket.particles.draw");
            pd.entryCount = 3;
            pd.entries = pe;
            particle_draw_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &pd);
            WGPUBindGroupLayout pbgls[4] = {scene_bgl, object_bgl, material_bgl, particle_draw_bgl};
            WGPUPipelineLayoutDescriptor ppld{};
            ppld.label = rhi::str("pocket.particles.draw");
            ppld.bindGroupLayoutCount = 4;
            ppld.bindGroupLayouts = pbgls;
            particle_layout = wgpuDeviceCreatePipelineLayout(device->device(), &ppld);
            WGPUBindGroupLayoutEntry se[3]{};
            se[0].binding = 0;
            se[0].visibility = WGPUShaderStage_Compute;
            se[0].buffer.type = WGPUBufferBindingType_Storage;
            se[1].binding = 1;
            se[1].visibility = WGPUShaderStage_Compute;
            se[1].buffer.type = WGPUBufferBindingType_Uniform;
            se[2].binding = 2;
            se[2].visibility = WGPUShaderStage_Compute;
            se[2].texture.sampleType = WGPUTextureSampleType_Depth;
            se[2].texture.viewDimension = WGPUTextureViewDimension_2D;
            WGPUBindGroupLayoutDescriptor sd{};
            sd.label = rhi::str("pocket.particles.simulate");
            sd.entryCount = 3;
            sd.entries = se;
            particle_sim_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &sd);
            WGPUPipelineLayoutDescriptor spl{};
            spl.label = rhi::str("pocket.particles.simulate");
            spl.bindGroupLayoutCount = 1;
            spl.bindGroupLayouts = &particle_sim_bgl;
            particle_sim_layout = wgpuDeviceCreatePipelineLayout(device->device(), &spl);
            POCKET_TRY(psim, device->create_shader("pocket.particles.simulate", kParticleSimWgsl));
            particle_sim_shader = psim;
            WGPUComputePipelineDescriptor cpd{};
            cpd.label = rhi::str("pocket.particles.simulate");
            cpd.layout = particle_sim_layout;
            cpd.compute.module = particle_sim_shader;
            cpd.compute.entryPoint = rhi::str("simulate");
            particle_sim_pipeline = wgpuDeviceCreateComputePipeline(device->device(), &cpd);
            if (!particle_sim_pipeline) return fail("gpu_pipeline_failed", "the GPU particles' simulation could not be created");
        }
        WGPUPipelineLayoutDescriptor kpld{};
        kpld.label = rhi::str("pocket.sky");
        kpld.bindGroupLayoutCount = 1;
        kpld.bindGroupLayouts = &scene_bgl;
        sky_layout = wgpuDeviceCreatePipelineLayout(device->device(), &kpld);
        WGPUBindGroupLayoutEntry ce{};
        ce.binding = 5;
        ce.visibility = WGPUShaderStage_Vertex;
        ce.buffer.type = WGPUBufferBindingType_Uniform;
        ce.buffer.hasDynamicOffset = true;
        ce.buffer.minBindingSize = sizeof(float) * 16;
        WGPUBindGroupLayoutDescriptor cld{};
        cld.label = rhi::str("pocket.cascade");
        cld.entryCount = 1;
        cld.entries = &ce;
        cascade_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &cld);
        WGPUBindGroupLayout sbgls[3] = {frame_bgl, object_bgl, cascade_bgl};
        WGPUPipelineLayoutDescriptor spld{};
        spld.label = rhi::str("pocket.shadow");
        spld.bindGroupLayoutCount = 3;
        spld.bindGroupLayouts = sbgls;
        shadow_layout = wgpuDeviceCreatePipelineLayout(device->device(), &spld);

        mesh_attrs[0].format = WGPUVertexFormat_Float32x3;
        mesh_attrs[0].offset = 0;
        mesh_attrs[0].shaderLocation = 0;
        mesh_attrs[1].format = WGPUVertexFormat_Float32x3;
        mesh_attrs[1].offset = sizeof(float) * 3;
        mesh_attrs[1].shaderLocation = 1;
        mesh_attrs[2].format = WGPUVertexFormat_Float32x2;
        mesh_attrs[2].offset = sizeof(float) * 6;
        mesh_attrs[2].shaderLocation = 2;
        vbl = WGPUVertexBufferLayout{};
        vbl.stepMode = WGPUVertexStepMode_Vertex;
        mesh_attrs[3].format = WGPUVertexFormat_Unorm8x4;
        mesh_attrs[3].offset = sizeof(float) * 8;
        mesh_attrs[3].shaderLocation = 5;
        mesh_attrs[4].format = WGPUVertexFormat_Snorm8x4;   // the tangent
        mesh_attrs[4].offset = sizeof(float) * 8 + sizeof(std::uint32_t);
        mesh_attrs[4].shaderLocation = 6;
        vbl.arrayStride = sizeof(Vertex);
        vbl.attributeCount = 5;
        vbl.attributes = mesh_attrs;
        skin_attrs[0].format = WGPUVertexFormat_Uint16x4;
        skin_attrs[0].offset = 0;
        skin_attrs[0].shaderLocation = 3;
        skin_attrs[1].format = WGPUVertexFormat_Float32x4;
        skin_attrs[1].offset = sizeof(std::uint16_t) * 4;
        skin_attrs[1].shaderLocation = 4;
        vbls[0] = vbl;
        vbls[1] = WGPUVertexBufferLayout{};
        vbls[1].stepMode = WGPUVertexStepMode_Vertex;
        vbls[1].arrayStride = sizeof(assets::SkinVertex);
        vbls[1].attributeCount = 2;
        vbls[1].attributes = skin_attrs;

        // The line shader and layout live for the renderer's life; the pipelines follow the sample count.
        POCKET_TRY(lm, device->create_shader("pocket.lines", kLineWgsl));
        line_shader = lm;
        WGPUBindGroupLayout lbgls[1] = {frame_bgl};
        WGPUPipelineLayoutDescriptor lpld{};
        lpld.label = rhi::str("pocket.lines");
        lpld.bindGroupLayoutCount = 1;
        lpld.bindGroupLayouts = lbgls;
        line_layout = wgpuDeviceCreatePipelineLayout(device->device(), &lpld);
        POCKET_TRY_VOID(create_scene_pipelines(msaa));
        POCKET_TRY_VOID(create_bloom());
        POCKET_TRY_VOID(create_post());

        // Shadow map: depth only from the sun, both faces (thin geometry still casts), the bias
        // in the lookup handles acne.
        WGPUDepthStencilState sds{};
        sds.format = kCascadeDepth;
        sds.depthWriteEnabled = WGPUOptionalBool_True;
        sds.depthCompare = WGPUCompareFunction_Less;
        sds.stencilFront.compare = WGPUCompareFunction_Always;
        sds.stencilBack.compare = WGPUCompareFunction_Always;
        sds.stencilReadMask = 0xFFFFFFFF;
        sds.stencilWriteMask = 0xFFFFFFFF;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.shadow");
        rpd.layout = shadow_layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs_shadow");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.depthStencil = &sds;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = nullptr;
        shadow_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!shadow_pipeline) return fail("gpu_pipeline_failed", "shadow pipeline creation failed");
        rpd.label = rhi::str("pocket.shadow.skinned");
        rpd.vertex.entryPoint = rhi::str("vs_shadow_skinned");
        rpd.vertex.bufferCount = 2;
        rpd.vertex.buffers = vbls;
        shadow_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!shadow_skinned_pipeline) return fail("gpu_pipeline_failed", "skinned shadow pipeline creation failed");
        {
            // Cut-outs: the shadow layout with the material after it, and a fragment that drops holes.
            WGPUBindGroupLayout cbgls[4] = {frame_bgl, object_bgl, cascade_bgl, material_bgl};
            WGPUPipelineLayoutDescriptor cpld{};
            cpld.label = rhi::str("pocket.shadow.cut");
            cpld.bindGroupLayoutCount = 4;
            cpld.bindGroupLayouts = cbgls;
            shadow_cut_layout = wgpuDeviceCreatePipelineLayout(device->device(), &cpld);
            WGPUFragmentState cfs{};
            cfs.module = shader;
            cfs.entryPoint = rhi::str("fs_shadow_cut");
            cfs.targetCount = 0;
            WGPURenderPipelineDescriptor crpd = rpd;
            crpd.label = rhi::str("pocket.shadow.cut");
            crpd.layout = shadow_cut_layout;
            crpd.vertex.entryPoint = rhi::str("vs_shadow_cut");
            crpd.vertex.bufferCount = 1;
            crpd.vertex.buffers = &vbl;
            crpd.fragment = &cfs;
            shadow_cut_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &crpd);
            if (!shadow_cut_pipeline) return fail("gpu_pipeline_failed", "cut-out shadow pipeline creation failed");
            // The same three for the lights' atlas, at its 32-bit depth.
            sds.format = WGPUTextureFormat_Depth32Float;
            crpd.label = rhi::str("pocket.atlas.cut");
            atlas_cut_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &crpd);
            rpd.label = rhi::str("pocket.atlas.skinned");
            atlas_skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            rpd.label = rhi::str("pocket.atlas");
            rpd.vertex.entryPoint = rhi::str("vs_shadow");
            rpd.vertex.bufferCount = 1;
            rpd.vertex.buffers = &vbl;
            atlas_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
            if (!atlas_pipeline || !atlas_skinned_pipeline || !atlas_cut_pipeline) return fail("gpu_pipeline_failed", "the shadow atlas pipelines could not be created");
        }
        joint_buffer = device->create_buffer("pocket.joints", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kMaxJoints) * sizeof(float) * 16);
        joint_staging.resize(static_cast<std::size_t>(kMaxJoints) * 16);
        morph_buffer = device->create_buffer("pocket.morphs", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kMaxMorphVec4) * sizeof(float) * 4);

        frame_buffer = device->create_buffer("pocket.frame", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(FrameUniforms));
        light_buffer = device->create_buffer("pocket.lights", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, sizeof(GpuLight) * kMaxLights);
        cluster_buffer = device->create_buffer("pocket.clusters", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, sizeof(std::uint32_t) * (2ull * kClusters + kMaxClusterEntries));
        object_buffer = device->create_buffer("pocket.objects", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kObjectStride) * kMaxObjects);
        object_staging.resize(static_cast<std::size_t>(kObjectStride) * kMaxObjects);

        WGPUBindGroupEntry fbe{};
        fbe.binding = 0;
        fbe.buffer = frame_buffer;
        fbe.size = sizeof(FrameUniforms);
        WGPUBindGroupDescriptor fbd{};
        fbd.label = rhi::str("pocket.frame");
        fbd.layout = frame_bgl;
        fbd.entryCount = 1;
        fbd.entries = &fbe;
        frame_bg = wgpuDeviceCreateBindGroup(device->device(), &fbd);

        WGPUTextureDescriptor std_{};
        std_.label = rhi::str("pocket.shadow");
        std_.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding;
        std_.dimension = WGPUTextureDimension_2D;
        std_.size = {kShadowMapSize, kShadowMapSize, kCascades + 1};
        std_.format = kCascadeDepth;
        std_.mipLevelCount = 1;
        std_.sampleCount = 1;
        shadow_texture = wgpuDeviceCreateTexture(device->device(), &std_);
        if (!shadow_texture) return fail("gpu_texture_failed", "cannot create the shadow map");
        WGPUTextureViewDescriptor svd{};
        svd.format = std_.format;
        svd.dimension = WGPUTextureViewDimension_2DArray;
        svd.mipLevelCount = 1;
        svd.arrayLayerCount = kCascades + 1;
        svd.aspect = WGPUTextureAspect_DepthOnly;
        svd.usage = std_.usage;
        shadow_view = wgpuTextureCreateView(shadow_texture, &svd);
        for (std::uint32_t c = 0; c <= kShelterLayer; ++c) {
            svd.dimension = WGPUTextureViewDimension_2D;
            svd.baseArrayLayer = c;
            svd.arrayLayerCount = 1;
            cascade_view[c] = wgpuTextureCreateView(shadow_texture, &svd);
        }
        cascade_buffer = device->create_buffer("pocket.cascades", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, 256ull * (kCascades + 1));
        WGPUBindGroupEntry cbe{};
        cbe.binding = 5;
        cbe.buffer = cascade_buffer;
        cbe.size = sizeof(float) * 16;
        WGPUBindGroupDescriptor cbd{};
        cbd.label = rhi::str("pocket.cascade");
        cbd.layout = cascade_bgl;
        cbd.entryCount = 1;
        cbd.entries = &cbe;
        cascade_bg = wgpuDeviceCreateBindGroup(device->device(), &cbd);
        WGPUSamplerDescriptor ssd{};
        ssd.label = rhi::str("pocket.shadow");
        ssd.addressModeU = WGPUAddressMode_ClampToEdge;
        ssd.addressModeV = WGPUAddressMode_ClampToEdge;
        ssd.addressModeW = WGPUAddressMode_ClampToEdge;
        ssd.magFilter = WGPUFilterMode_Linear;
        ssd.minFilter = WGPUFilterMode_Linear;
        ssd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
        ssd.lodMinClamp = 0;
        ssd.lodMaxClamp = 32;
        ssd.compare = WGPUCompareFunction_LessEqual;
        ssd.maxAnisotropy = 1;
        shadow_sampler = wgpuDeviceCreateSampler(device->device(), &ssd);
        POCKET_TRY_VOID(create_sky());
        POCKET_TRY_VOID(create_ao());
        POCKET_TRY_VOID(create_volume());
        POCKET_TRY_VOID(create_gi());
        POCKET_TRY_VOID(create_taa());
        POCKET_TRY_VOID(create_fx());
        POCKET_TRY_VOID(create_oit());
        POCKET_TRY_VOID(create_ssr());
        POCKET_TRY_VOID(create_water());
        POCKET_TRY_VOID(create_probe_textures());
        POCKET_TRY_VOID(create_probe_passes());
        WGPUBindGroupEntry* sbe = scene_entries;
        sbe[12].binding = 12;
        sbe[12].textureView = probe_env_view;
        {
            // Every slot's harmonics (sixteen vec4s), then each grid probe's moments (128).
            const std::vector<float> zero(4 * 16 * (kMaxProbes + kMaxGridProbes) + 4 * 128 * kMaxGridProbes, 0.0f);
            probe_sh_buffer = device->create_buffer("pocket.probes.harmonics", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, zero.size() * sizeof(float), zero.data());
            const std::vector<std::uint32_t> none(2 * kClusters + kMaxLights, 0u);
            probe_cluster_buffer = device->create_buffer("pocket.probes.lights", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, none.size() * sizeof(std::uint32_t), none.data());
            grid_cluster_buffer = device->create_buffer("pocket.grids.lights", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, none.size() * sizeof(std::uint32_t), none.data());
        }
        sbe[13].binding = 13;
        sbe[13].buffer = probe_sh_buffer;
        sbe[13].size = sizeof(float) * (4 * 16 * (kMaxProbes + kMaxGridProbes) + 4 * 128 * kMaxGridProbes);
        POCKET_TRY_VOID(create_decals());
        POCKET_TRY_VOID(create_timer());
        sbe[14].binding = 14;
        sbe[14].buffer = decal_buffer;
        sbe[14].size = sizeof(GpuDecal) * kMaxDecals;
        sbe[15].binding = 15;
        sbe[15].textureView = decal_view;
        sbe[16].binding = 16;
        sbe[16].sampler = decal_sampler;
        sbe[0] = fbe;
        sbe[1].binding = 1;
        sbe[1].textureView = shadow_view;
        sbe[2].binding = 2;
        sbe[2].sampler = shadow_sampler;
        sbe[3].binding = 3;
        sbe[3].textureView = env_view;
        sbe[4].binding = 4;
        sbe[4].sampler = env_sampler;
        sbe[5].binding = 5;
        sbe[5].buffer = sh_buffer;
        sbe[5].size = sizeof(float) * 36;
        sbe[8].binding = 8;
        sbe[8].buffer = light_buffer;
        sbe[8].size = sizeof(GpuLight) * kMaxLights;
        sbe[9].binding = 9;
        sbe[9].buffer = cluster_buffer;
        sbe[9].size = sizeof(std::uint32_t) * (2ull * kClusters + kMaxClusterEntries);
        face_buffer = device->create_buffer("pocket.shadow.faces", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(GpuFace) * kMaxFaces);
        sbe[10].binding = 10;
        sbe[10].buffer = face_buffer;
        sbe[10].size = sizeof(GpuFace) * kMaxFaces;
        {
            WGPUTextureDescriptor td{};
            td.label = rhi::str("pocket.shadow.atlas.stub");
            td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding;
            td.dimension = WGPUTextureDimension_2D;
            td.size = {1, 1, 1};
            td.format = WGPUTextureFormat_Depth32Float;
            td.mipLevelCount = 1;
            td.sampleCount = 1;
            atlas_stub = wgpuDeviceCreateTexture(device->device(), &td);
            if (!atlas_stub) return fail("gpu_texture_failed", "cannot create the shadow atlas stand-in");
            atlas_stub_view = wgpuTextureCreateView(atlas_stub, nullptr);
        }
        {
            auto [t, v] = make_target("pocket.glass.stub", 1, 1, kHdrFormat, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
            if (!t) return fail("gpu_texture_failed", "cannot create the glass copy's stand-in");
            glass_stub = t;
            glass_stub_view = v;
        }
        sbe[17].binding = 17;
        sbe[17].textureView = glass_stub_view;
        {
            auto [t, v] = make_target("pocket.occluders.stub", 1, 1, WGPUTextureFormat_R8Unorm, WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst);
            if (!t) return fail("gpu_texture_failed", "cannot create the 2D shadows' stand-in");
            occ_stub = t;
            occ_stub_view = v;
        }
        sbe[18].binding = 18;
        sbe[18].textureView = occ_stub_view;
        face_slots = device->create_buffer("pocket.shadow.face_slots", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, 256ull * kMaxFaces);
        {
            WGPUBindGroupEntry fse{};
            fse.binding = 5;
            fse.buffer = face_slots;
            fse.size = sizeof(float) * 16;
            WGPUBindGroupDescriptor fsd{};
            fsd.label = rhi::str("pocket.shadow.face");
            fsd.layout = cascade_bgl;
            fsd.entryCount = 1;
            fsd.entries = &fse;
            face_bg = wgpuDeviceCreateBindGroup(device->device(), &fsd);
        }
        make_scene_group(ao_white_view);

        WGPUBindGroupEntry obe[3]{};
        obe[0].binding = 0;
        obe[0].buffer = object_buffer;
        obe[0].size = static_cast<std::uint64_t>(kObjectStride) * kMaxObjects;
        obe[1].binding = 1;
        obe[1].buffer = joint_buffer;
        obe[1].size = static_cast<std::uint64_t>(kMaxJoints) * sizeof(float) * 16;
        obe[2].binding = 2;
        obe[2].buffer = morph_buffer;
        obe[2].size = static_cast<std::uint64_t>(kMaxMorphVec4) * sizeof(float) * 4;
        WGPUBindGroupDescriptor obd{};
        obd.label = rhi::str("pocket.object");
        obd.layout = object_bgl;
        obd.entryCount = 3;
        obd.entries = obe;
        object_bg = wgpuDeviceCreateBindGroup(device->device(), &obd);

        WGPUSamplerDescriptor sd{};
        sd.label = rhi::str("pocket.material");
        sd.addressModeU = WGPUAddressMode_Repeat;
        sd.addressModeV = WGPUAddressMode_Repeat;
        sd.addressModeW = WGPUAddressMode_Repeat;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Linear;
        sd.lodMinClamp = 0;
        sd.lodMaxClamp = 32;
        sd.maxAnisotropy = 8;   // a ground seen at a grazing angle stays sharp
        sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        sd.label = rhi::str("pocket.material.nearest");
        sd.maxAnisotropy = 1;   // anisotropy needs linear filters
        sd.magFilter = WGPUFilterMode_Nearest;
        sd.minFilter = WGPUFilterMode_Nearest;
        sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
        sd.lodMaxClamp = 0;   // pixel art keeps its texels: the full-size level however small it is drawn
        sd.addressModeU = WGPUAddressMode_ClampToEdge;
        sd.addressModeV = WGPUAddressMode_ClampToEdge;
        nearest_sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        const std::uint8_t white_px[4] = {255, 255, 255, 255};
        POCKET_TRY(w, upload_texture("pocket.white", 1, 1, white_px));
        white = w;
        const std::uint8_t flat_px[4] = {128, 128, 255, 255};
        POCKET_TRY(fnrm, upload_texture("pocket.flat_normal", 1, 1, flat_px));
        flat_normal = fnrm;
        std::vector<std::uint8_t> dot_px(64 * 64 * 4, 255);
        for (int y = 0; y < 64; ++y) {
            for (int x = 0; x < 64; ++x) {
                const float dx = (static_cast<float>(x) + 0.5f) / 32.0f - 1.0f, dy = (static_cast<float>(y) + 0.5f) / 32.0f - 1.0f;
                const float a = std::clamp((1.0f - std::sqrt(dx * dx + dy * dy)) / 0.6f, 0.0f, 1.0f);
                dot_px[(static_cast<std::size_t>(y) * 64 + static_cast<std::size_t>(x)) * 4 + 3] = static_cast<std::uint8_t>(std::lround(255.0f * a * a * (3.0f - 2.0f * a)));
            }
        }
        POCKET_TRY(dt, upload_texture("pocket.dot", 64, 64, dot_px.data()));
        soft_spot = dt;
        POCKET_TRY(ns, upload_texture("pocket.no_splat", 1, 1, white_px));
        no_splat = ns;
        POCKET_TRY(nl, create_layer_array("pocket.no_layers", 1, 1));
        no_layers = nl;
        {
            WGPUTexelCopyTextureInfo dst{};
            dst.texture = no_layers.texture;
            dst.aspect = WGPUTextureAspect_All;
            WGPUTexelCopyBufferLayout lay{};
            lay.bytesPerRow = 4;
            lay.rowsPerImage = 1;
            WGPUExtent3D ext{1, 1, 1};
            wgpuQueueWriteTexture(device->queue(), &dst, white_px, 4, &lay, &ext);
        }

        for (int k = 0; k < kPrimitiveCount; ++k) {
            MeshData data = make_primitive(k);
            GpuMesh& gm = meshes[static_cast<std::size_t>(k)];
            gm.vertices = device->create_buffer(primitive_name(k), WGPUBufferUsage_Vertex | WGPUBufferUsage_CopyDst, data.vertices.size() * sizeof(Vertex), data.vertices.data());
            gm.indices = device->create_buffer(primitive_name(k), WGPUBufferUsage_Index | WGPUBufferUsage_CopyDst, data.indices.size() * sizeof(std::uint32_t), data.indices.data());
            gm.index_count = static_cast<std::uint32_t>(data.indices.size());
            gm.aabb_min = data.aabb_min;
            gm.aabb_max = data.aabb_max;
        }
        return {};
    }

    // A texture with its full mip chain: each level averages 2 by 2 texels of the one above,
    // weighted by alpha so transparent texels do not darken the edges of what is drawn small.
    Result<GpuTexture> upload_texture(const char* label, std::uint32_t w, std::uint32_t h, const std::uint8_t* rgba) {
        GpuTexture t;
        std::uint32_t levels = 1;
        while ((w >> levels) > 0 || (h >> levels) > 0) ++levels;
        WGPUTextureDescriptor td{};
        td.label = rhi::str(label);
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {w, h, 1};
        td.format = WGPUTextureFormat_RGBA8Unorm;
        td.mipLevelCount = levels;
        td.sampleCount = 1;
        const WGPUTextureFormat srgb = WGPUTextureFormat_RGBA8UnormSrgb;
        td.viewFormatCount = 1;
        td.viewFormats = &srgb;
        t.texture = wgpuDeviceCreateTexture(device->device(), &td);
        if (!t.texture) return fail("gpu_texture_failed", "cannot create texture {} ({}x{})", label, w, h);
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = levels;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        t.view = wgpuTextureCreateView(t.texture, &vd);
        vd.format = srgb;
        t.srgb_view = wgpuTextureCreateView(t.texture, &vd);
        auto write_level = [&](std::uint32_t level, std::uint32_t lw, std::uint32_t lh, const std::uint8_t* data) {
            WGPUTexelCopyTextureInfo dst{};
            dst.texture = t.texture;
            dst.mipLevel = level;
            dst.origin = {0, 0, 0};
            dst.aspect = WGPUTextureAspect_All;
            WGPUTexelCopyBufferLayout layout{};
            layout.offset = 0;
            layout.bytesPerRow = lw * 4;
            layout.rowsPerImage = lh;
            WGPUExtent3D ext{lw, lh, 1};
            wgpuQueueWriteTexture(device->queue(), &dst, data, static_cast<std::size_t>(lw) * lh * 4, &layout, &ext);
        };
        write_level(0, w, h, rgba);
        if (levels == 1) return t;
        std::vector<std::uint8_t> prev(rgba, rgba + static_cast<std::size_t>(w) * h * 4), next;
        std::uint32_t pw = w, ph = h;
        for (std::uint32_t level = 1; level < levels; ++level) {
            const std::uint32_t nw = std::max(1u, pw / 2), nh = std::max(1u, ph / 2);
            next.assign(static_cast<std::size_t>(nw) * nh * 4, 0);
            for (std::uint32_t y = 0; y < nh; ++y) {
                for (std::uint32_t x = 0; x < nw; ++x) {
                    float weighted[3] = {0, 0, 0}, plain[3] = {0, 0, 0}, alpha = 0;
                    for (std::uint32_t dy = 0; dy < 2; ++dy) {
                        for (std::uint32_t dx = 0; dx < 2; ++dx) {
                            const std::uint32_t sx = std::min(pw - 1, x * 2 + dx), sy = std::min(ph - 1, y * 2 + dy);
                            const std::uint8_t* p = &prev[(static_cast<std::size_t>(sy) * pw + sx) * 4];
                            const float a = static_cast<float>(p[3]);
                            for (int c = 0; c < 3; ++c) { weighted[c] += static_cast<float>(p[c]) * a; plain[c] += static_cast<float>(p[c]); }
                            alpha += a;
                        }
                    }
                    std::uint8_t* q = &next[(static_cast<std::size_t>(y) * nw + x) * 4];
                    for (int c = 0; c < 3; ++c) q[c] = static_cast<std::uint8_t>(std::lround(alpha > 0 ? weighted[c] / alpha : plain[c] / 4));
                    q[3] = static_cast<std::uint8_t>(std::lround(alpha / 4));
                }
            }
            write_level(level, nw, nh, next.data());
            prev.swap(next);
            pw = nw;
            ph = nh;
        }
        return t;
    }

    // Group 2 for a set of maps: base color, metallic-roughness, normal, emissive (empty paths take
    // the white or flat-normal defaults), with the linear or nearest sampler. Cached by key.
    WGPUBindGroup material_for(const std::string& base, const std::string& mr, const std::string& normal, const std::string& emissive, bool nearest) {
        std::string key = base + "|" + mr + "|" + normal + "|" + emissive + (nearest ? "|n" : "|l");
        if (auto it = material_groups.find(key); it != material_groups.end()) return it->second;
        WGPUBindGroup bg = material_group(base, mr, normal, emissive, nearest, no_splat.view, no_layers.view);
        material_groups[key] = bg;
        return bg;
    }

    WGPUBindGroup material_group(const std::string& base, const std::string& mr, const std::string& normal, const std::string& emissive, bool nearest, WGPUTextureView splat, WGPUTextureView layers) {
        WGPUBindGroupEntry entries[7]{};
        entries[0].binding = 0;
        entries[0].textureView = view_for(base, white, true);
        entries[1].binding = 1;
        entries[1].sampler = nearest ? nearest_sampler : sampler;
        entries[2].binding = 2;
        entries[2].textureView = view_for(mr, white);
        entries[3].binding = 3;
        entries[3].textureView = view_for(normal, flat_normal);
        entries[4].binding = 4;
        entries[4].textureView = view_for(emissive, white, true);
        entries[5].binding = 6;
        entries[5].textureView = splat;
        entries[6].binding = 7;
        entries[6].textureView = layers;
        WGPUBindGroupDescriptor bd{};
        bd.label = rhi::str("pocket.material");
        bd.layout = material_bgl;
        bd.entryCount = 7;
        bd.entries = entries;
        return wgpuDeviceCreateBindGroup(device->device(), &bd);
    }

    // A 2D array texture of kLayerSize-style layers with its sRGB view (colours).
    Result<GpuTexture> create_layer_array(const char* label, std::uint32_t size, std::uint32_t levels, std::uint32_t layers = 1) {
        GpuTexture t;
        WGPUTextureDescriptor td{};
        td.label = rhi::str(label);
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {size, size, layers};
        td.format = WGPUTextureFormat_RGBA8UnormSrgb;
        td.mipLevelCount = levels;
        td.sampleCount = 1;
        t.texture = wgpuDeviceCreateTexture(device->device(), &td);
        if (!t.texture) return fail("gpu_texture_failed", "cannot create texture {}", label);
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2DArray;
        vd.mipLevelCount = levels;
        vd.arrayLayerCount = layers;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        t.view = wgpuTextureCreateView(t.texture, &vd);
        return t;
    }

    // A terrain's layer images, each resampled (wrapping round, so it still tiles) to kLayerSize
    // square and tinted by its colour in linear light, with their mip chains, in one array.
    WGPUTextureView layer_array(const assets::TerrainLayers& tl) {
        std::string key;
        for (const auto& l : tl.layers) key += std::format("{}|{:.4g},{:.4g},{:.4g};", l.texture, l.color.x, l.color.y, l.color.z);
        if (auto it = layer_arrays.find(key); it != layer_arrays.end()) return it->second.view;
        const auto count = static_cast<std::uint32_t>(std::clamp<std::size_t>(tl.layers.size(), 1, 4));
        auto made = create_layer_array("pocket.terrain_layers", kLayerSize, kLayerLevels, count);
        if (!made) {
            report_missing("terrain layers", made.error().message);
            return no_layers.view;
        }
        static const std::array<float, 256> to_linear = [] {
            std::array<float, 256> t{};
            for (int k = 0; k < 256; ++k) t[static_cast<std::size_t>(k)] = decode(static_cast<float>(k) / 255.0f);
            return t;
        }();
        auto to_srgb = [](float v) {
            v = std::clamp(v, 0.0f, 1.0f);
            const float s = v <= 0.0031308f ? v * 12.92f : 1.055f * std::pow(v, 1.0f / 2.4f) - 0.055f;
            return static_cast<std::uint8_t>(std::lround(s * 255.0f));
        };
        for (std::uint32_t layer = 0; layer < count; ++layer) {
            const auto& l = tl.layers[layer];
            std::uint32_t w = 1, h = 1;
            std::vector<std::uint8_t> white_px{255, 255, 255, 255};
            const std::uint8_t* src = white_px.data();
            if (!l.texture.empty() && assets && !failed.contains(l.texture)) {
                auto img = assets->image(l.texture);
                if (img && (*img)->width > 0 && (*img)->height > 0) {
                    w = (*img)->width;
                    h = (*img)->height;
                    src = (*img)->rgba.data();
                } else {
                    report_missing(l.texture, img ? "empty image" : img.error().message);
                }
            } else if (!l.texture.empty()) {
                note_missing(l.texture);
            }
            // Level 0 in linear light: bilinear, wrapping at the edges, times the colour.
            std::vector<float> lin(static_cast<std::size_t>(kLayerSize) * kLayerSize * 3);
            const float tint[3] = {l.color.x, l.color.y, l.color.z};
            for (std::uint32_t y = 0; y < kLayerSize; ++y) {
                const float fy = (static_cast<float>(y) + 0.5f) * static_cast<float>(h) / kLayerSize - 0.5f;
                const float fly = std::floor(fy);
                const float ay = fy - fly;
                const auto y0 = static_cast<std::uint32_t>((static_cast<std::int64_t>(fly) % h + h) % h), y1 = (y0 + 1) % h;
                for (std::uint32_t x = 0; x < kLayerSize; ++x) {
                    const float fx = (static_cast<float>(x) + 0.5f) * static_cast<float>(w) / kLayerSize - 0.5f;
                    const float flx = std::floor(fx);
                    const float ax = fx - flx;
                    const auto x0 = static_cast<std::uint32_t>((static_cast<std::int64_t>(flx) % w + w) % w), x1 = (x0 + 1) % w;
                    for (int c = 0; c < 3; ++c) {
                        auto at = [&](std::uint32_t px, std::uint32_t py) { return to_linear[src[(static_cast<std::size_t>(py) * w + px) * 4 + static_cast<std::size_t>(c)]]; };
                        const float v = (at(x0, y0) * (1 - ax) + at(x1, y0) * ax) * (1 - ay) + (at(x0, y1) * (1 - ax) + at(x1, y1) * ax) * ay;
                        lin[(static_cast<std::size_t>(y) * kLayerSize + x) * 3 + static_cast<std::size_t>(c)] = v * tint[c];
                    }
                }
            }
            std::uint32_t size = kLayerSize;
            for (std::uint32_t level = 0; level < kLayerLevels; ++level) {
                std::vector<std::uint8_t> bytes(static_cast<std::size_t>(size) * size * 4);
                for (std::size_t k = 0; k < static_cast<std::size_t>(size) * size; ++k) {
                    for (int c = 0; c < 3; ++c) bytes[k * 4 + static_cast<std::size_t>(c)] = to_srgb(lin[k * 3 + static_cast<std::size_t>(c)]);
                    bytes[k * 4 + 3] = 255;
                }
                WGPUTexelCopyTextureInfo dst{};
                dst.texture = made->texture;
                dst.mipLevel = level;
                dst.origin = {0, 0, layer};
                dst.aspect = WGPUTextureAspect_All;
                WGPUTexelCopyBufferLayout lay{};
                lay.bytesPerRow = size * 4;
                lay.rowsPerImage = size;
                WGPUExtent3D ext{size, size, 1};
                wgpuQueueWriteTexture(device->queue(), &dst, bytes.data(), bytes.size(), &lay, &ext);
                if (size == 1) break;
                const std::uint32_t half = size / 2;
                std::vector<float> next(static_cast<std::size_t>(half) * half * 3);
                for (std::uint32_t y = 0; y < half; ++y)
                    for (std::uint32_t x = 0; x < half; ++x)
                        for (int c = 0; c < 3; ++c) {
                            float sum = 0;
                            for (std::uint32_t q = 0; q < 4; ++q) sum += lin[((static_cast<std::size_t>(y) * 2 + (q >> 1)) * size + x * 2 + (q & 1)) * 3 + static_cast<std::size_t>(c)];
                            next[(static_cast<std::size_t>(y) * half + x) * 3 + static_cast<std::size_t>(c)] = sum * 0.25f;
                        }
                lin.swap(next);
                size = half;
            }
        }
        layer_arrays[key] = *made;
        return made->view;
    }

    // The material group of a terrain drawn from textured layers: its splat map (the layers' shares
    // at the grid's samples) and its layers' array, made once per mesh (a new mesh at every change).
    WGPUBindGroup terrain_group(const std::string& mesh_path, const assets::TerrainLayers& tl) {
        if (auto it = terrain_gpu.find(mesh_path); it != terrain_gpu.end()) return it->second.group;
        // Meshes the terrains have moved on from take their maps with them.
        for (auto it = terrain_gpu.begin(); it != terrain_gpu.end();) {
            if (assets && !assets->has_mesh(it->first)) {
                if (it->second.group) wgpuBindGroupRelease(it->second.group);
                release_texture(it->second.splat);
                it = terrain_gpu.erase(it);
            } else {
                ++it;
            }
        }
        TerrainGpu tg;
        const auto n = static_cast<std::uint32_t>(std::max(tl.n, 1));
        if (tl.weights.size() == static_cast<std::size_t>(n) * n * 4) {
            if (auto t = upload_texture("pocket.terrain_splat", n, n, tl.weights.data())) tg.splat = *t;
        }
        tg.group = material_group("", "", "", "", false, tg.splat.view ? tg.splat.view : no_splat.view, layer_array(tl));
        terrain_gpu[mesh_path] = tg;
        return tg.group;
    }

    // Logged once per path; listed in the stats of every frame that wanted it.
    void note_missing(const std::string& path) {
        if (std::find(stats.missing.begin(), stats.missing.end(), path) == stats.missing.end()) stats.missing.push_back(path);
    }
    void report_missing(const std::string& path, const std::string& why) {
        if (failed.insert(path).second) log::warn("renderer", "asset {}: {}", path, why);
        note_missing(path);
    }

    // The bind group for an image path (the white texture when empty or unavailable).
    // The view of an image by project path, uploaded on first use; `fallback` when unavailable.
    // Color maps (srgb) are decoded to linear light when sampled; data maps are read as stored.
    WGPUTextureView view_for(const std::string& path, const GpuTexture& fallback, bool srgb = false) {
        auto pick = [srgb](const GpuTexture& t) { return srgb && t.srgb_view ? t.srgb_view : t.view; };
        if (path.empty()) return pick(fallback);
        if (path.starts_with("view:")) {
            const auto it = view_targets.find(path.substr(5));
            return it != view_targets.end() ? pick(it->second.tex) : pick(fallback);
        }
        if (path == kDotTexture) return pick(soft_spot);
        if (auto it = textures.find(path); it != textures.end()) return pick(it->second);
        if (!assets) { report_missing(path, "no asset store"); return pick(fallback); }
        if (failed.contains(path)) { note_missing(path); return pick(fallback); }
        auto img = assets->image(path);
        if (!img) {
            report_missing(path, img.error().message);
            return pick(fallback);
        }
        auto t = upload_texture(path.c_str(), (*img)->width, (*img)->height, (*img)->rgba.data());
        if (!t) {
            report_missing(path, t.error().message);
            return pick(fallback);
        }
        textures[path] = *t;
        return pick(*t);
    }

    WGPUBindGroup texture_for(const std::string& path, bool nearest = false) { return material_for(path, "", "", "", nearest); }

    Status create_timer() {
        if (!device->timestamps()) return {};
        WGPUQuerySetDescriptor qd{};
        qd.label = rhi::str("pocket.timer");
        qd.type = WGPUQueryType_Timestamp;
        qd.count = kTimedPasses * 2;
        timer_set = wgpuDeviceCreateQuerySet(device->device(), &qd);
        if (!timer_set) return {};   // timings are a convenience: without them the frame is the same
        timer_resolve = device->create_buffer("pocket.timer.resolve", WGPUBufferUsage_QueryResolve | WGPUBufferUsage_CopySrc, sizeof(std::uint64_t) * kTimedPasses * 2);
        for (auto& r : timer_reads) r.buffer = device->create_buffer("pocket.timer.read", WGPUBufferUsage_MapRead | WGPUBufferUsage_CopyDst, sizeof(std::uint64_t) * kTimedPasses * 2);
        return {};
    }
    static std::string label_of(WGPUStringView v) {
        if (!v.data) return "pass";
        return v.length == WGPU_STRLEN ? std::string(v.data) : std::string(v.data, v.length);
    }
    // A pass begun with timestamps at its start and end (when there are slots left this frame).
    WGPUPassTimestampWrites* timed(WGPUStringView label) {
        if (!timer_set || timer_names.size() >= kTimedPasses) return nullptr;
        const auto k = static_cast<std::uint32_t>(timer_names.size());
        timer_writes[k] = WGPUPassTimestampWrites{nullptr, timer_set, 2 * k, 2 * k + 1};
        std::string name = label_of(label);
        if (name.starts_with("pocket.")) name = name.substr(7);
        timer_names.push_back(std::move(name));
        return &timer_writes[k];
    }
    WGPURenderPassEncoder begin_pass(WGPUCommandEncoder enc, WGPURenderPassDescriptor d) {
        if (!d.timestampWrites) d.timestampWrites = timed(d.label);
        return wgpuCommandEncoderBeginRenderPass(enc, &d);
    }
    WGPUComputePassEncoder begin_compute(WGPUCommandEncoder enc, WGPUComputePassDescriptor d) {
        if (!d.timestampWrites) d.timestampWrites = timed(d.label);
        return wgpuCommandEncoderBeginComputePass(enc, &d);
    }
    // At a frame's start: map what earlier frames resolved once they are on the GPU, read what has mapped.
    void timer_begin_frame() {
        timer_names.clear();
        if (!timer_set) return;
        for (auto& r : timer_reads) {
            if (r.state == 1 && device->submitted_frames() > r.submitted) {
                r.state = 2;
                WGPUBufferMapCallbackInfo cb{};
                cb.mode = WGPUCallbackMode_AllowSpontaneous;
                cb.callback = [](WGPUMapAsyncStatus status, WGPUStringView, void* u1, void*) {
                    auto* read = static_cast<TimerRead*>(u1);
                    read->state = status == WGPUMapAsyncStatus_Success ? 3 : 0;
                };
                cb.userdata1 = &r;
                wgpuBufferMapAsync(r.buffer, WGPUMapMode_Read, 0, sizeof(std::uint64_t) * r.names.size() * 2, cb);
            } else if (r.state == 3) {
                const auto* t = static_cast<const std::uint64_t*>(wgpuBufferGetConstMappedRange(r.buffer, 0, sizeof(std::uint64_t) * r.names.size() * 2));
                if (t) {
                    // A GPU overlaps passes (a tiler runs one's vertices under another's pixels), so a
                    // pass's own start-to-end span also holds its wait for the ones before it. What
                    // each adds is how much later it finishes than every pass before it did.
                    const double ns = device->timestamp_period();
                    timer_last.clear();
                    std::uint64_t first = UINT64_MAX, done = 0;
                    for (std::size_t k = 0; k < r.names.size(); ++k) {
                        const std::uint64_t a = t[2 * k], b = t[2 * k + 1];
                        if (b == 0 || b < a) continue;   // not written (a start the GPU left out reads 0)
                        if (first == UINT64_MAX) { first = a > 0 ? a : b; done = first; }
                        timer_last.emplace_back(r.names[k], b > done ? static_cast<double>(b - done) * ns / 1e6 : 0.0);
                        done = std::max(done, b);
                    }
                    timer_last_ms = first != UINT64_MAX ? static_cast<double>(done - first) * ns / 1e6 : 0.0;
                    timer_last_frame = r.submitted;
                    ++timer_reads_done;
                }
                wgpuBufferUnmap(r.buffer);
                r.state = 0;
            }
        }
    }
    // At a frame's end: this frame's timestamps into a free read-back buffer (skipped when none is free).
    void timer_end_frame(WGPUCommandEncoder enc) {
        if (!timer_set || timer_names.empty()) return;
        for (auto& r : timer_reads) {
            if (r.state != 0) continue;
            const auto n = static_cast<std::uint32_t>(timer_names.size());
            wgpuCommandEncoderResolveQuerySet(enc, timer_set, 0, n * 2, timer_resolve, 0);
            wgpuCommandEncoderCopyBufferToBuffer(enc, timer_resolve, 0, r.buffer, 0, sizeof(std::uint64_t) * n * 2);
            r.names = timer_names;
            r.submitted = device->submitted_frames();
            r.state = 1;
            break;
        }
    }

    // A level of detail of a primitive (kind) or an asset (path): the mesh simplified to `ratio` of
    // its triangles, made once.
    const LodMesh* lod_mesh(int kind, const std::string& path, float ratio) {
        ratio = std::clamp(ratio, 0.01f, 1.0f);
        const std::string key = kind >= 0 ? std::format("#{}|{:.3f}", kind, ratio) : std::format("{}|{:.3f}", path, ratio);
        if (auto it = lod_meshes.find(key); it != lod_meshes.end()) return &it->second;
        LodMesh lm;
        assets::MeshLod lod;
        if (kind >= 0) {
            const MeshData data = make_primitive(kind);
            if (data.vertices.empty()) return nullptr;
            const std::vector<assets::Submesh> whole{assets::Submesh{0, static_cast<std::uint32_t>(data.indices.size())}};
            lod = assets::simplify(&data.vertices[0].position.x, &data.vertices[0].normal.x, sizeof(Vertex), data.vertices.size(), data.indices, whole, ratio);
            lm.gpu = meshes[static_cast<std::size_t>(kind)];
        } else {
            AssetMesh* am = asset_mesh(path);
            if (!am || !assets) return nullptr;
            auto m = assets->mesh(path);
            if (!m) return nullptr;
            lod = assets::simplify(**m, ratio);
            lm.gpu = am->gpu;
        }
        if (lod.indices.empty()) return nullptr;
        lm.gpu.indices = device->create_buffer("pocket.lod", WGPUBufferUsage_Index | WGPUBufferUsage_CopyDst, lod.indices.size() * sizeof(std::uint32_t), lod.indices.data());
        lm.gpu.index_count = static_cast<std::uint32_t>(lod.indices.size());
        lm.submeshes = std::move(lod.submeshes);
        lm.error = lod.error;
        return &(lod_meshes[key] = std::move(lm));
    }

    // A glTF mesh by project path, uploaded on first use; null when unavailable.
    // A mesh's vertices as they go up to the GPU.
    static std::vector<Vertex> gpu_vertices(const assets::Mesh& src) {
        std::vector<Vertex> verts;
        verts.reserve(src.vertices.size());
        // Vertex colors go up sRGB-encoded in 8 bits (dark shades keep their steps), decoded per vertex.
        auto enc = [](float c) {
            c = std::clamp(c, 0.0f, 1.0f);
            const float e = c <= 0.0031308f ? c * 12.92f : 1.055f * std::pow(c, 1.0f / 2.4f) - 0.055f;
            return static_cast<std::uint32_t>(std::lround(e * 255.0f));
        };
        for (const auto& v : src.vertices) {
            const std::uint32_t a = static_cast<std::uint32_t>(std::lround(std::clamp(v.color.w, 0.0f, 1.0f) * 255.0f));
            verts.push_back({v.position, v.normal, v.uv, enc(v.color.x) | (enc(v.color.y) << 8) | (enc(v.color.z) << 16) | (a << 24), pack_tangent(v.tangent)});
        }
        return verts;
    }

    AssetMesh* asset_mesh(const std::string& path) {
        if (auto it = asset_meshes.find(path); it != asset_meshes.end()) {
            // A cloth's mesh is made again every tick it moves: its new vertices written over the old.
            if (path.starts_with("cloth:") && assets) {
                if (auto m = assets->mesh(path); m && (*m)->revision != it->second.revision && (*m)->vertices.size() == it->second.vertex_count && (*m)->indices.size() == it->second.gpu.index_count) {
                    const std::vector<Vertex> verts = gpu_vertices(**m);
                    device->write_buffer(it->second.gpu.vertices, 0, verts.data(), verts.size() * sizeof(Vertex));
                    it->second.gpu.aabb_min = (*m)->aabb_min;
                    it->second.gpu.aabb_max = (*m)->aabb_max;
                    it->second.revision = (*m)->revision;
                }
            }
            return &it->second;
        }
        if (!assets) { report_missing(path, "no asset store"); return nullptr; }
        if (failed.contains(path)) { note_missing(path); return nullptr; }
        auto m = assets->mesh(path);
        if (!m) {
            report_missing(path, m.error().message);
            return nullptr;
        }
        const assets::Mesh& src = **m;
        const std::vector<Vertex> verts = gpu_vertices(src);
        AssetMesh am;
        am.revision = src.revision;
        am.vertex_count = static_cast<std::uint32_t>(src.vertices.size());
        am.gpu.vertices = device->create_buffer(path.c_str(), WGPUBufferUsage_Vertex | WGPUBufferUsage_CopyDst, verts.size() * sizeof(Vertex), verts.data());
        am.gpu.indices = device->create_buffer(path.c_str(), WGPUBufferUsage_Index | WGPUBufferUsage_CopyDst, src.indices.size() * sizeof(std::uint32_t), src.indices.data());
        if (src.skinned()) am.gpu.skin = device->create_buffer(path.c_str(), WGPUBufferUsage_Vertex | WGPUBufferUsage_CopyDst, src.skin_vertices.size() * sizeof(assets::SkinVertex), src.skin_vertices.data());
        am.gpu.index_count = static_cast<std::uint32_t>(src.indices.size());
        for (const assets::Submesh& sm : src.submeshes) {
            am.rest.push_back(src.rest_global(sm.node));
            am.unbake.push_back(sm.origin >= 0 && sm.node < 0 && sm.skin < 0 ? src.rest_global(sm.origin).inverse_affine() : Mat4::identity());
        }
        for (const assets::Node& n : src.nodes) am.node_names.push_back(n.name);
        am.gpu.aabb_min = src.aabb_min;
        am.gpu.aabb_max = src.aabb_max;
        am.submeshes = src.submeshes;
        am.materials = src.materials;
        am.chunks = src.chunks;
        if (!src.morph_targets.empty()) {
            // The targets' deltas appended to the morph buffer: per target, per vertex, position then normal.
            const std::uint32_t targets = std::min<std::uint32_t>(static_cast<std::uint32_t>(src.morph_targets.size()), kMaxMorphTargets);
            const std::uint32_t vertices = static_cast<std::uint32_t>(src.vertices.size());
            const std::uint32_t needed = targets * vertices * 2;
            if (morph_used + needed <= kMaxMorphVec4) {
                std::vector<float> data(static_cast<std::size_t>(needed) * 4, 0.0f);
                for (std::uint32_t t = 0; t < targets; ++t) {
                    const assets::MorphTarget& mt = src.morph_targets[t];
                    for (std::uint32_t v = 0; v < vertices; ++v) {
                        float* p = data.data() + (static_cast<std::size_t>(t) * vertices + v) * 8;
                        const Vec3 dp = v < mt.positions.size() ? mt.positions[v] : Vec3{0, 0, 0};
                        const Vec3 dn = v < mt.normals.size() ? mt.normals[v] : Vec3{0, 0, 0};
                        p[0] = dp.x; p[1] = dp.y; p[2] = dp.z; p[3] = 0;
                        p[4] = dn.x; p[5] = dn.y; p[6] = dn.z; p[7] = 0;
                    }
                }
                device->write_buffer(morph_buffer, static_cast<std::uint64_t>(morph_used) * sizeof(float) * 4, data.data(), static_cast<std::uint64_t>(needed) * sizeof(float) * 4);
                am.morph_base = morph_used;
                am.morph_targets = targets;
                am.morph_vertices = vertices;
                morph_used += needed;
                if (src.morph_targets.size() > kMaxMorphTargets) log::warn("renderer", "{}: {} morph targets, the first {} are drawn", path, src.morph_targets.size(), kMaxMorphTargets);
            } else if (!morph_full_warned) {
                log::warn("renderer", "{}: the morph buffer is full ({} vec4 of {}); its targets are not drawn", path, morph_used, kMaxMorphVec4);
                morph_full_warned = true;
            }
        }
        new_bounds.emplace_back(path, std::make_pair(src.aabb_min, src.aabb_max));
        // An engine-made mesh (a terrain's, `terrain:<entity>@<revision>`) replaces its older revisions.
        if (const auto at = path.find('@'); path.starts_with("terrain:") && at != std::string::npos) {
            const std::string stem = path.substr(0, at + 1);
            for (auto old = asset_meshes.begin(); old != asset_meshes.end();) {
                if (old->first.starts_with(stem)) {
                    if (old->second.gpu.vertices) wgpuBufferRelease(old->second.gpu.vertices);
                    if (old->second.gpu.indices) wgpuBufferRelease(old->second.gpu.indices);
                    if (old->second.gpu.skin) wgpuBufferRelease(old->second.gpu.skin);
                    old = asset_meshes.erase(old);
                } else {
                    ++old;
                }
            }
        }
        auto [it, inserted] = asset_meshes.emplace(path, std::move(am));
        return &it->second;
    }

    // One tile layer of a map as a mesh of textured quads in the entity's XY plane, built once.
    // The ids the animated tiles of `map` show at the time, one entry per animated tile of every
    // tileset, in order: equal keys draw the same frames.
    static std::string frame_key_of(const assets::TileMap& map, std::uint64_t time_ms) {
        std::string key;
        for (const assets::TileSet& ts : map.tilesets) {
            for (const auto& [id, anim] : ts.animations) {
                key += std::to_string(ts.frame_at(id, time_ms));
                key += ',';
            }
        }
        return key;
    }

    // The quads of `cells` (with `frame` giving the id drawn for a gid), grouped per tileset so each
    // texture is one draw, uploaded as one mesh.
    void build_tile_quads(const assets::TileMap& map, const assets::TileLayer& layer, float tile_size, const std::map<const assets::TileSet*, std::vector<std::pair<int, int>>>& by_set, std::uint64_t time_ms, const std::string& key, GpuMesh& gpu, std::vector<TileLayerMesh::Part>& parts) {
        std::vector<Vertex> verts;
        std::vector<std::uint32_t> indices;
        // An orthogonal map's cells are tile_size square; the others keep the tiles' pixel
        // proportions, a cell's width being tile_size, and a tile taller than its cell (a wall on
        // an isometric map) stands up from the cell's bottom edge.
        const bool ortho = map.orthogonal();
        const float sx = tile_size / static_cast<float>(map.tile_width), sy = ortho ? tile_size / static_cast<float>(map.tile_height) : sx;
        const float ox = layer.offset_x * sx;
        const float oy = -layer.offset_y * sy;
        for (auto& [ts, cells] : by_set) {
            TileLayerMesh::Part part{static_cast<std::uint32_t>(indices.size()), 0, ts->image};
            const float iw = ts->image_width > 0 ? static_cast<float>(ts->image_width) : static_cast<float>(ts->columns * (ts->tile_width + ts->spacing) - ts->spacing + 2 * ts->margin);
            const int rows = ts->columns > 0 ? std::max(1, (ts->tile_count + ts->columns - 1) / ts->columns) : 1;
            const float ih = ts->image_height > 0 ? static_cast<float>(ts->image_height) : static_cast<float>(rows * (ts->tile_height + ts->spacing) - ts->spacing + 2 * ts->margin);
            for (auto [x, y] : cells) {
                std::uint32_t gid = layer.gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(layer.width) + static_cast<std::size_t>(x)];
                const std::uint32_t local = static_cast<std::uint32_t>(ts->frame_at(static_cast<int>((gid & assets::TileMap::kIdMask) - ts->first_gid), time_ms));
                const int col = static_cast<int>(local % static_cast<std::uint32_t>(ts->columns));
                const int row = static_cast<int>(local / static_cast<std::uint32_t>(ts->columns));
                float u0 = (static_cast<float>(ts->margin + col * (ts->tile_width + ts->spacing))) / iw;
                float v0 = (static_cast<float>(ts->margin + row * (ts->tile_height + ts->spacing))) / ih;
                float u1 = u0 + static_cast<float>(ts->tile_width) / iw;
                float v1 = v0 + static_cast<float>(ts->tile_height) / ih;
                // Tiled flips: horizontal and vertical swap the edges; diagonal swaps the axes.
                Vec2 uv[4] = {{u0, v1}, {u1, v1}, {u1, v0}, {u0, v0}};  // bottom-left, bottom-right, top-right, top-left
                if (gid & assets::TileMap::kFlipD) { std::swap(uv[0], uv[2]); }
                if (gid & assets::TileMap::kFlipH) { std::swap(uv[0], uv[1]); std::swap(uv[2], uv[3]); }
                if (gid & assets::TileMap::kFlipV) { std::swap(uv[0], uv[3]); std::swap(uv[1], uv[2]); }
                float wx0, wx1, wy0, wy1;
                if (ortho) {
                    wx0 = ox + static_cast<float>(x) * tile_size;
                    wx1 = wx0 + tile_size;
                    wy1 = oy - static_cast<float>(y) * tile_size;
                    wy0 = wy1 - tile_size;
                } else {
                    const Vec2 cell = map.tile_pixel(x, y);
                    wx0 = ox + cell.x * sx;
                    wx1 = wx0 + static_cast<float>(ts->tile_width) * sx;
                    wy0 = oy - (cell.y + static_cast<float>(map.tile_height)) * sy;
                    wy1 = wy0 + static_cast<float>(ts->tile_height) * sy;
                }
                auto base = static_cast<std::uint32_t>(verts.size());
                verts.push_back({{wx0, wy0, 0}, {0, 0, 1}, uv[0]});
                verts.push_back({{wx1, wy0, 0}, {0, 0, 1}, uv[1]});
                verts.push_back({{wx1, wy1, 0}, {0, 0, 1}, uv[2]});
                verts.push_back({{wx0, wy1, 0}, {0, 0, 1}, uv[3]});
                indices.insert(indices.end(), {base, base + 1, base + 2, base, base + 2, base + 3});
            }
            part.count = static_cast<std::uint32_t>(indices.size()) - part.first;
            parts.push_back(part);
        }
        if (verts.empty()) {
            verts.push_back({});
            indices.push_back(0);
        }
        gpu.vertices = device->create_buffer(key.c_str(), WGPUBufferUsage_Vertex | WGPUBufferUsage_CopyDst, verts.size() * sizeof(Vertex), verts.data());
        gpu.indices = device->create_buffer(key.c_str(), WGPUBufferUsage_Index | WGPUBufferUsage_CopyDst, indices.size() * sizeof(std::uint32_t), indices.data());
        gpu.index_count = static_cast<std::uint32_t>(indices.size());
    }

    const TileLayerMesh* tile_layer_mesh(const assets::TileMap& map, const assets::TileLayer& layer, std::int32_t layer_index, float tile_size, std::uint64_t time_ms) {
        const std::string key = map.path + "|" + layer.name + "|" + std::to_string(layer_index) + "|" + std::to_string(tile_size);
        auto it = tile_meshes.find(key);
        if (it != tile_meshes.end() && it->second.revision != layer.revision) {
            // The map was edited (tilemap.set / fill): drop the stale mesh and build the layer again.
            if (it->second.gpu.vertices) wgpuBufferRelease(it->second.gpu.vertices);
            if (it->second.gpu.indices) wgpuBufferRelease(it->second.gpu.indices);
            if (it->second.anim.vertices) wgpuBufferRelease(it->second.anim.vertices);
            if (it->second.anim.indices) wgpuBufferRelease(it->second.anim.indices);
            tile_meshes.erase(it);
            it = tile_meshes.end();
            ++tile_rebuilds;
        }
        if (it == tile_meshes.end()) {
            TileLayerMesh tm;
            tm.index = layer_index;
            tm.revision = layer.revision;
            std::map<const assets::TileSet*, std::vector<std::pair<int, int>>> fixed, animated;
            for (int y = 0; y < layer.height; ++y) {
                for (int x = 0; x < layer.width; ++x) {
                    std::uint32_t gid = layer.gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(layer.width) + static_cast<std::size_t>(x)];
                    if (gid == 0) continue;
                    const assets::TileSet* ts = map.tileset_for(gid);
                    if (!ts) continue;
                    const bool moving = ts->animations.contains(static_cast<int>((gid & assets::TileMap::kIdMask) - ts->first_gid));
                    (moving ? animated : fixed)[ts].push_back({x, y});
                }
            }
            build_tile_quads(map, layer, tile_size, fixed, time_ms, key, tm.gpu, tm.parts);
            tm.has_anim = !animated.empty();
            if (tm.has_anim) {
                tm.frame_key = frame_key_of(map, time_ms);
                build_tile_quads(map, layer, tile_size, animated, time_ms, key + "|anim", tm.anim, tm.anim_parts);
            }
            it = tile_meshes.emplace(key, std::move(tm)).first;
        } else if (it->second.has_anim) {
            // A frame moved on: only the animated cells are built again.
            std::string now = frame_key_of(map, time_ms);
            if (now != it->second.frame_key) {
                std::map<const assets::TileSet*, std::vector<std::pair<int, int>>> animated;
                for (int y = 0; y < layer.height; ++y) {
                    for (int x = 0; x < layer.width; ++x) {
                        std::uint32_t gid = layer.gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(layer.width) + static_cast<std::size_t>(x)];
                        if (gid == 0) continue;
                        const assets::TileSet* ts = map.tileset_for(gid);
                        if (ts && ts->animations.contains(static_cast<int>((gid & assets::TileMap::kIdMask) - ts->first_gid))) animated[ts].push_back({x, y});
                    }
                }
                if (it->second.anim.vertices) wgpuBufferRelease(it->second.anim.vertices);
                if (it->second.anim.indices) wgpuBufferRelease(it->second.anim.indices);
                it->second.anim = {};
                it->second.anim_parts.clear();
                it->second.frame_key = std::move(now);
                build_tile_quads(map, layer, tile_size, animated, time_ms, key + "|anim", it->second.anim, it->second.anim_parts);
                ++tile_frames;
            }
        }
        return &it->second;
    }

    static int primitive_index(const std::string& name) {
        if (name == "cube" || name == "0" || name.empty()) return 0;
        if (name == "sphere" || name == "1") return 1;
        if (name == "plane" || name == "2") return 2;
        if (name == "cylinder" || name == "3") return 3;
        if (name == "quad" || name == "4") return 4;
        if (name == "capsule" || name == "5") return 5;
        return -1;
    }

    CameraView find_camera(const world::World& w, float aspect) {
        CameraView cv;
        if (view_override) {
            const ViewOverride& v = *view_override;
            const float span = length(v.target - v.eye);
            cv.view = Mat4::look_at(v.eye, v.target, v.up);
            cv.near = std::max(span * 0.002f, 0.02f);
            cv.far = std::max(span * 8.0f, 100.0f);
            cv.proj = Mat4::perspective(radians(v.fov_degrees), aspect, cv.near, cv.far);
            cv.position = v.eye;
            cv.forward = normalize(v.target - v.eye);
            stats.has_camera = true;
            stats.camera = 0;
            return cv;
        }
        world::EntityId cam_id = 0;
        world::Camera cam;
        world::WorldTransform ct;
        // A named view's camera; for the window, the active one of highest priority (the first of equals).
        w.ecs().each([&](flecs::entity e, const world::Camera& c, const world::WorldTransform& t) {
            const bool wanted = camera_pick ? e.id() == camera_pick : c.active && c.target.empty();
            if (wanted && (cam_id == 0 || (!camera_pick && c.priority > cam.priority))) {
                cam_id = e.id();
                cam = c;
                ct = t;
            }
        });
        stats.has_camera = cam_id != 0;
        stats.camera = cam_id;
        if (!camera_pick && cam_id != 0) blend_window_view(w, cam_id, cam, ct);
        if (cam_id == 0) {
            ct.position = {0, 3, 8};
            Vec3 target{0, 0, 0};
            cv.view = Mat4::look_at(ct.position, target, {0, 1, 0});
            cv.proj = Mat4::perspective(radians(60), aspect, 0.1f, 1000.0f);
            cv.position = ct.position;
            cv.forward = normalize(target - ct.position);
            cv.near = 0.1f;
            cv.far = 1000.0f;
            return cv;
        }
        Vec3 forward = ct.rotation.rotate({0, 0, -1});
        Vec3 up = ct.rotation.rotate({0, 1, 0});
        cv.view = Mat4::look_at(ct.position, ct.position + forward, up);
        if (cam.orthographic) {
            float half_h = std::max(0.001f, cam.ortho_size);
            float half_w = half_h * aspect;
            cv.proj = Mat4::orthographic(-half_w, half_w, -half_h, half_h, cam.near, cam.far);
        } else {
            cv.proj = Mat4::perspective(radians(cam.fov_degrees), aspect, cam.near, cam.far);
        }
        cv.position = ct.position;
        cv.forward = forward;
        cv.near = cam.near;
        cv.far = cam.far;
        return cv;
    }

    // The window's view as it moves between cameras: when another camera takes the window and asks
    // for a blend, from where the view stood to that camera over `blend` seconds of world time,
    // eased in and out; the outgoing camera followed while it lives and the blend did not cut into
    // another (then from where that one had got to). A world started again (its clock went back)
    // cuts. The camera's pose and projection are written into `cam` and `ct`.
    void blend_window_view(const world::World& w, world::EntityId cam_id, world::Camera& cam, world::WorldTransform& ct) {
        const double now = w.seconds();
        const bool restarted = now < window_seconds;
        if (cam_id != window_camera) {
            if (window_camera != 0 && !restarted && cam.blend > 0) {
                const bool mid = blend_seconds > 0 && now - blend_start < blend_seconds;
                blend_from = mid ? 0 : window_camera;
                blend_pose = window_pose;
                blend_start = now;
                blend_seconds = cam.blend;
            } else {
                blend_seconds = 0;
            }
            window_camera = cam_id;
        }
        if (restarted) blend_seconds = 0;
        window_seconds = now;
        ViewPose to{ct.position, ct.rotation, cam.fov_degrees, cam.ortho_size, cam.orthographic};
        if (blend_seconds > 0) {
            const double t = (now - blend_start) / static_cast<double>(blend_seconds);
            if (t >= 1.0) {
                blend_seconds = 0;
            } else {
                ViewPose from = blend_pose;
                if (blend_from != 0 && w.ecs().is_alive(blend_from)) {
                    const flecs::entity e = w.ecs().entity(blend_from);
                    const auto* c = e.try_get<world::Camera>();
                    const auto* t0 = e.try_get<world::WorldTransform>();
                    if (c && t0) from = ViewPose{t0->position, t0->rotation, c->fov_degrees, c->ortho_size, c->orthographic};
                }
                const float x = static_cast<float>(std::clamp(t, 0.0, 1.0));
                const float e = x * x * (3.0f - 2.0f * x);
                Quat qa = from.rotation, qb = to.rotation;
                if (qa.x * qb.x + qa.y * qb.y + qa.z * qb.z + qa.w * qb.w < 0) qb = Quat{-qb.x, -qb.y, -qb.z, -qb.w};
                to.rotation = normalize(Quat{qa.x + (qb.x - qa.x) * e, qa.y + (qb.y - qa.y) * e, qa.z + (qb.z - qa.z) * e, qa.w + (qb.w - qa.w) * e});
                to.position = from.position + (to.position - from.position) * e;
                to.fov = from.fov + (to.fov - from.fov) * e;
                to.ortho_size = from.ortho_size + (to.ortho_size - from.ortho_size) * e;
                if (from.ortho != to.ortho && e < 0.5f) to.ortho = from.ortho;   // perspective and parallel do not mix: the switch at halfway
                stats.blend_from = w.ecs().is_alive(blend_from) ? blend_from : 0;
                stats.blend = e;
            }
        }
        window_pose = to;
        ct.position = to.position;
        ct.rotation = to.rotation;
        cam.fov_degrees = to.fov;
        cam.ortho_size = to.ortho_size;
        cam.orthographic = to.ortho;
    }

    // Faces of the shadow atlas for the kept lights that cast shadows, nearest first while faces
    // last: a spot's view down its cone (a little wider than the cone), a point light's six views
    // along the axes (a little wider than 90 degrees, so the 3x3 filter at a face's edge still reads
    // that face). Each light's first face and a texel's size at unit distance go into its cone.
    void assign_shadow_faces() {
        faces.clear();
        face_light.clear();
        for (GpuLight& g : light_packed) {
            const bool wants = g.cone[1] >= 0.0f;
            g.cone[1] = -1.0f;
            if (!wants || !shadows.enabled) continue;
            const bool spot = g.color_kind[3] > 1.5f;
            const std::uint32_t need = spot ? 1u : 6u;
            if (faces.size() + need > kMaxFaces) continue;
            if (!atlas_texture && !create_atlas()) return;
            const Vec3 at{g.pos_range[0], g.pos_range[1], g.pos_range[2]};
            const float range = g.pos_range[3];
            const float near = std::clamp(range * 0.005f, 0.02f, 0.2f);
            float half_tan = 0;
            auto add_face = [&](Vec3 dir, Vec3 up, float fov) {
                const Mat4 vp = Mat4::perspective(fov, 1.0f, near, range) * Mat4::look_at(at, at + dir, up);
                GpuFace f{};
                to_array(vp, f.view_proj);
                const auto slot = static_cast<std::uint32_t>(faces.size());
                f.rect[0] = static_cast<float>(slot % kFaceTiles) / kFaceTiles;
                f.rect[1] = static_cast<float>(slot / kFaceTiles) / kFaceTiles;
                f.rect[2] = 1.0f / kFaceTiles;
                f.rect[3] = 1.0f / kAtlasSize;
                faces.push_back(f);
                face_light.emplace_back(at, range);
            };
            g.cone[1] = static_cast<float>(faces.size());
            if (spot) {
                const Vec3 dir{g.dir_cos[0], g.dir_cos[1], g.dir_cos[2]};
                const float outer = std::acos(std::clamp(g.dir_cos[3], -1.0f, 1.0f));
                const float fov = std::min(2.0f * outer * 1.08f + radians(2.0f), radians(170.0f));
                const Vec3 up = std::fabs(dir.y) > 0.99f ? Vec3{1, 0, 0} : Vec3{0, 1, 0};
                add_face(dir, up, fov);
                half_tan = std::tan(fov * 0.5f);
            } else {
                half_tan = 1.0f + 3.0f / (kFaceSize * 0.5f);
                const float fov = 2.0f * std::atan(half_tan);
                add_face({1, 0, 0}, {0, 1, 0}, fov);
                add_face({-1, 0, 0}, {0, 1, 0}, fov);
                add_face({0, 1, 0}, {0, 0, 1}, fov);
                add_face({0, -1, 0}, {0, 0, 1}, fov);
                add_face({0, 0, 1}, {0, 1, 0}, fov);
                add_face({0, 0, -1}, {0, 1, 0}, fov);
            }
            g.cone[2] = 2.0f * half_tan / static_cast<float>(kFaceSize);
            ++stats.shadow_lights;
        }
        stats.shadow_faces = static_cast<std::uint32_t>(faces.size());
        if (faces.empty()) return;
        device->write_buffer(face_buffer, 0, faces.data(), faces.size() * sizeof(GpuFace));
        std::vector<std::uint8_t> slots(256 * faces.size(), 0);
        for (std::size_t i = 0; i < faces.size(); ++i) std::memcpy(slots.data() + 256 * i, faces[i].view_proj, sizeof(float) * 16);
        device->write_buffer(face_slots, 0, slots.data(), slots.size());
    }

    bool create_atlas() {
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.shadow.atlas");
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {kAtlasSize, kAtlasSize, 1};
        td.format = WGPUTextureFormat_Depth32Float;
        td.mipLevelCount = 1;
        td.sampleCount = 1;
        atlas_texture = wgpuDeviceCreateTexture(device->device(), &td);
        if (!atlas_texture) return false;
        atlas_view = wgpuTextureCreateView(atlas_texture, nullptr);
        make_scene_group(scene_ao);
        return true;
    }

    // Point and spot lights into the froxel grid (docs/design/rendering.md, Many lights): each
    // light's sphere of reach, seen from the camera, covers a rectangle of tiles (the screen box of
    // its corners, the whole screen when one is behind the eye) and a run of depth slices; of the
    // clusters there, those whose box in view space the sphere touches list it. A light wholly out
    // of view is left out; the nearest come first, so when the budget runs out it is the farthest
    // that go.
    void cluster_lights(FrameUniforms& fu) {
        const Mat4 vp = camera.proj * camera.view;
        const float near = std::max(camera.near, 0.05f);
        const float far = std::max(camera.far, near * 2.0f);
        const float scale = static_cast<float>(kClusterZ) / std::log(far / near);
        const float ln_near = std::log(near);
        auto slice_of = [&](float z) { return static_cast<std::uint32_t>(std::clamp((std::log(std::max(z, near)) - ln_near) * scale, 0.0f, static_cast<float>(kClusterZ - 1))); };
        auto tile = [](float t, std::uint32_t n) { return static_cast<std::uint32_t>(std::clamp(t * static_cast<float>(n), 0.0f, static_cast<float>(n - 1))); };
        // Every cluster's box in view space: its tile's corner rays (or, for an orthographic camera,
        // corner lines) between the depths its slice spans.
        const Mat4 inv_proj = camera.proj.inverse();
        const bool ortho = std::fabs(camera.proj.at(3, 3) - 1.0f) < 1e-5f;
        std::array<Vec3, (kClusterX + 1) * (kClusterY + 1)> corner{};
        for (std::uint32_t y = 0; y <= kClusterY; ++y)
            for (std::uint32_t x = 0; x <= kClusterX; ++x) {
                const float nx = -1.0f + 2.0f * static_cast<float>(x) / kClusterX, ny = 1.0f - 2.0f * static_cast<float>(y) / kClusterY;
                const Vec4 p = inv_proj * Vec4{nx, ny, 0.5f, 1.0f};
                const Vec3 q{p.x / p.w, p.y / p.w, p.z / p.w};
                // Perspective: the ray's point at depth 1; orthographic: the line's x and y.
                corner[y * (kClusterX + 1) + x] = ortho ? Vec3{q.x, q.y, 0} : Vec3{q.x / -q.z, q.y / -q.z, 1};
            }
        cluster_box.resize(kClusters);
        for (std::uint32_t z = 0; z < kClusterZ; ++z) {
            const float d0 = near * std::exp(static_cast<float>(z) / scale), d1 = near * std::exp(static_cast<float>(z + 1) / scale);
            for (std::uint32_t y = 0; y < kClusterY; ++y)
                for (std::uint32_t x = 0; x < kClusterX; ++x) {
                    Vec3 lo{1e30f, 1e30f, -d1}, hi{-1e30f, -1e30f, -d0};
                    for (std::uint32_t k = 0; k < 4; ++k) {
                        const Vec3 c = corner[(y + (k >> 1)) * (kClusterX + 1) + x + (k & 1)];
                        for (const float d : {d0, d1}) {
                            const float px = ortho ? c.x : c.x * d, py = ortho ? c.y : c.y * d;
                            lo.x = std::min(lo.x, px); hi.x = std::max(hi.x, px);
                            lo.y = std::min(lo.y, py); hi.y = std::max(hi.y, py);
                        }
                    }
                    cluster_box[(z * kClusterY + y) * kClusterX + x] = {lo, hi};
                }
        }
        struct Span { std::uint32_t light, x0, x1, y0, y1, z0, z1; float dist; Vec3 view; float r; };
        std::vector<Span> spans;
        spans.reserve(light_staging.size());
        for (std::uint32_t i = 0; i < light_staging.size(); ++i) {
            const GpuLight& g = light_staging[i];
            const Vec3 c{g.pos_range[0], g.pos_range[1], g.pos_range[2]};
            const float r = g.pos_range[3];
            const float d = dot(c - camera.position, camera.forward);
            if (d + r < near || d - r > far) { ++stats.lights_culled; continue; }
            float u0 = 0, u1 = 1, v0 = 0, v1 = 1;
            float nx0 = 1e30f, nx1 = -1e30f, ny0 = 1e30f, ny1 = -1e30f;
            bool behind = false;
            for (int k = 0; k < 8; ++k) {
                const Vec4 p = vp * Vec4{c.x + ((k & 1) ? r : -r), c.y + ((k & 2) ? r : -r), c.z + ((k & 4) ? r : -r), 1};
                if (p.w <= 1e-4f) { behind = true; break; }
                nx0 = std::min(nx0, p.x / p.w); nx1 = std::max(nx1, p.x / p.w);
                ny0 = std::min(ny0, p.y / p.w); ny1 = std::max(ny1, p.y / p.w);
            }
            if (!behind) {
                if (nx1 < -1 || nx0 > 1 || ny1 < -1 || ny0 > 1) { ++stats.lights_culled; continue; }
                u0 = (nx0 + 1) * 0.5f; u1 = (nx1 + 1) * 0.5f;
                v0 = (1 - ny1) * 0.5f; v1 = (1 - ny0) * 0.5f;
            }
            spans.push_back({i, tile(u0, kClusterX), tile(u1, kClusterX), tile(v0, kClusterY), tile(v1, kClusterY), slice_of(d - r), slice_of(d + r), length(c - camera.position), camera.view.transform_point(c), r});
        }
        std::stable_sort(spans.begin(), spans.end(), [](const Span& a, const Span& b) { return a.dist < b.dist; });
        // The clusters each light touches, within the budgets; then the lists laid out after the grid.
        std::vector<std::uint32_t> counts(kClusters, 0);
        std::vector<std::uint32_t> hits;
        std::vector<std::pair<std::size_t, std::size_t>> hit_range(spans.size());
        std::uint32_t total = 0;
        std::size_t kept = 0;
        for (std::size_t si = 0; si < spans.size(); ++si) {
            Span& s = spans[si];
            const std::size_t start = hits.size();
            if (kept < kMaxLights) {
                for (std::uint32_t z = s.z0; z <= s.z1; ++z)
                    for (std::uint32_t y = s.y0; y <= s.y1; ++y)
                        for (std::uint32_t x = s.x0; x <= s.x1; ++x) {
                            const std::uint32_t c = (z * kClusterY + y) * kClusterX + x;
                            const auto& [lo, hi] = cluster_box[c];
                            const float dx = std::max({lo.x - s.view.x, 0.0f, s.view.x - hi.x});
                            const float dy = std::max({lo.y - s.view.y, 0.0f, s.view.y - hi.y});
                            const float dz = std::max({lo.z - s.view.z, 0.0f, s.view.z - hi.z});
                            if (dx * dx + dy * dy + dz * dz <= s.r * s.r) hits.push_back(c);
                        }
            }
            const auto n = static_cast<std::uint32_t>(hits.size() - start);
            if (kept == kMaxLights || total + n > kMaxClusterEntries) {
                ++stats.lights_dropped;
                s.light = UINT32_MAX;
                hits.resize(start);
                continue;
            }
            hit_range[si] = {start, hits.size()};
            total += n;
            ++kept;
            for (std::size_t h = start; h < hits.size(); ++h) ++counts[hits[h]];
        }
        cluster_staging.assign(2ull * kClusters + total, 0);
        std::uint32_t running = 2 * kClusters;
        for (std::uint32_t c = 0; c < kClusters; ++c) {
            cluster_staging[2 * c] = running;
            running += counts[c];
            stats.max_cluster_lights = std::max(stats.max_cluster_lights, counts[c]);
        }
        light_packed.clear();
        for (std::size_t si = 0; si < spans.size(); ++si) {
            const Span& s = spans[si];
            if (s.light == UINT32_MAX) continue;
            const auto slot = static_cast<std::uint32_t>(light_packed.size());
            light_packed.push_back(light_staging[s.light]);
            if (light_staging[s.light].color_kind[3] > 1.5f) ++stats.spot_lights;
            else ++stats.point_lights;
            for (std::size_t h = hit_range[si].first; h < hit_range[si].second; ++h) {
                const std::uint32_t c = hits[h];
                cluster_staging[cluster_staging[2 * c] + cluster_staging[2 * c + 1]++] = slot;
            }
        }
        stats.cluster_entries = total;
        assign_shadow_faces();
        fu.clusters[0] = kClusterX; fu.clusters[1] = kClusterY; fu.clusters[2] = kClusterZ;
        fu.clusters[3] = static_cast<std::uint32_t>(light_packed.size());
        fu.cluster_z[0] = near; fu.cluster_z[1] = far; fu.cluster_z[2] = scale; fu.cluster_z[3] = ln_near;
        fu.viewport[0] = static_cast<float>(applied.x); fu.viewport[1] = static_cast<float>(applied.y);
        fu.viewport[2] = static_cast<float>(std::max(1u, applied.w)); fu.viewport[3] = static_cast<float>(std::max(1u, applied.h));
        if (!light_packed.empty()) device->write_buffer(light_buffer, 0, light_packed.data(), light_packed.size() * sizeof(GpuLight));
        device->write_buffer(cluster_buffer, 0, cluster_staging.data(), cluster_staging.size() * sizeof(std::uint32_t));
    }
};

Renderer::Renderer() : impl_(std::make_unique<Impl>()) {}
Renderer::~Renderer() = default;

Result<std::unique_ptr<Renderer>> Renderer::create(rhi::Device& device) {
    std::unique_ptr<Renderer> r(new Renderer());
    r->impl_->device = &device;
    POCKET_TRY_VOID(r->impl_->init());
    return r;
}

namespace {
template <class Draws>
std::uint32_t tile_layers_parts(const Draws& sprites) {
    std::uint32_t n = 0;
    for (const auto& s : sprites) n += s.mesh != nullptr;
    return n;
}
}  // namespace

Result<Renderer::ViewTexture> Renderer::view_texture(const std::string& name, std::uint32_t width, std::uint32_t height) {
    Impl& im = *impl_;
    width = std::max(width, 1u);
    height = std::max(height, 1u);
    auto it = im.view_targets.find(name);
    if (it != im.view_targets.end() && it->second.w == width && it->second.h == height) return ViewTexture{it->second.tex.texture, it->second.render_view};
    if (it != im.view_targets.end()) {
        // A new size: the bind groups that sampled the old texture go with it.
        const std::string key = "view:" + name;
        for (auto g = im.material_groups.begin(); g != im.material_groups.end();) {
            if (g->first.find(key) != std::string::npos) { wgpuBindGroupRelease(g->second); g = im.material_groups.erase(g); }
            else ++g;
        }
        Impl::ViewTarget& old = it->second;
        for (WGPUTextureView v : {old.tex.view, old.tex.srgb_view, old.render_view}) if (v) wgpuTextureViewRelease(v);
        if (old.tex.texture) wgpuTextureRelease(old.tex.texture);
        im.view_targets.erase(it);
    }
    Impl::ViewTarget t;
    WGPUTextureDescriptor td{};
    td.label = rhi::str("pocket.view_texture");
    td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_CopySrc;
    td.dimension = WGPUTextureDimension_2D;
    td.size = {width, height, 1};
    td.format = WGPUTextureFormat_RGBA8Unorm;
    td.mipLevelCount = 1;
    td.sampleCount = 1;
    const WGPUTextureFormat srgb = WGPUTextureFormat_RGBA8UnormSrgb;
    td.viewFormatCount = 1;
    td.viewFormats = &srgb;
    t.tex.texture = wgpuDeviceCreateTexture(im.device->device(), &td);
    if (!t.tex.texture) return fail("gpu_texture_failed", "cannot create the view texture '{}' ({}x{})", name, width, height);
    WGPUTextureViewDescriptor vd{};
    vd.format = td.format;
    vd.dimension = WGPUTextureViewDimension_2D;
    vd.mipLevelCount = 1;
    vd.arrayLayerCount = 1;
    vd.aspect = WGPUTextureAspect_All;
    vd.usage = WGPUTextureUsage_TextureBinding;
    t.tex.view = wgpuTextureCreateView(t.tex.texture, &vd);
    vd.format = srgb;
    t.tex.srgb_view = wgpuTextureCreateView(t.tex.texture, &vd);
    vd.format = td.format;
    vd.usage = WGPUTextureUsage_RenderAttachment;
    t.render_view = wgpuTextureCreateView(t.tex.texture, &vd);
    t.w = width;
    t.h = height;
    const ViewTexture out{t.tex.texture, t.render_view};
    im.view_targets[name] = t;
    return out;
}

Renderer::ImageView Renderer::image_view(const std::string& path) {
    Impl& im = *impl_;
    if (path.starts_with("view:")) {
        const auto it = im.view_targets.find(path.substr(5));
        if (it == im.view_targets.end()) return {};
        return {it->second.tex.view, it->second.w, it->second.h};
    }
    if (path.empty() || !im.assets) return {};
    if (im.failed.contains(path)) { im.note_missing(path); return {}; }
    auto img = im.assets->image(path);
    if (!img) {
        im.report_missing(path, img.error().message);
        return {};
    }
    return {im.view_for(path, im.white), (*img)->width, (*img)->height};
}

Status Renderer::render(rhi::Frame& frame, const world::World& world, rhi::Color clear, const Particles* particles, const Animation* animation, const DebugDraw* debug, const RenderView* view) {
    Impl& im = *impl_;
    // Render scale (docs/design/rendering.md, Render scale): the window's one view is drawn into a
    // smaller frame of its own and stretched into the window's; cameras with viewports or targets of
    // their own are drawn whole.
    im.steer_scale();
    const float scale = view ? 1.0f : im.scale_now;
    if (scale >= 0.999f || frame.width < 8 || frame.height < 8) {
        im.window_sx = im.window_sy = 1.0f;
        POCKET_TRY_VOID(render_scene(frame, world, clear, particles, animation, debug, view));
        im.stats.render_scale = 1.0f;
        im.stats.render_width = frame.width;
        im.stats.render_height = frame.height;
        return {};
    }
    const auto w = std::max(1u, static_cast<std::uint32_t>(std::lround(static_cast<float>(frame.width) * scale)));
    const auto h = std::max(1u, static_cast<std::uint32_t>(std::lround(static_cast<float>(frame.height) * scale)));
    POCKET_TRY_VOID(im.ensure_scaled(w, h));
    rhi::Frame inner = frame;
    inner.color = im.scaled_color_view;
    inner.color_texture = im.scaled_color;
    inner.depth = im.scaled_depth_view;
    inner.width = w;
    inner.height = h;
    // A viewport asked of the window, in the smaller frame's pixels.
    const float sx = static_cast<float>(w) / static_cast<float>(frame.width), sy = static_cast<float>(h) / static_cast<float>(frame.height);
    const Viewport asked = im.viewport;
    if (asked.w > 0 && asked.h > 0) {
        im.viewport = Viewport{static_cast<std::int32_t>(std::lround(asked.x * sx)), static_cast<std::int32_t>(std::lround(asked.y * sy)),
                               std::max(1u, static_cast<std::uint32_t>(std::lround(asked.w * sx))), std::max(1u, static_cast<std::uint32_t>(std::lround(asked.h * sy)))};
    }
    const Status drawn = render_scene(inner, world, clear, particles, animation, debug, view);
    im.viewport = asked;
    POCKET_TRY_VOID(drawn);
    im.window_sx = static_cast<float>(frame.width) / static_cast<float>(w);
    im.window_sy = static_cast<float>(frame.height) / static_cast<float>(h);
    im.stats.render_scale = scale;
    im.stats.render_width = w;
    im.stats.render_height = h;
    // Stretched into the window's frame, sharpened.
    const bool pixelated = im.scale_settings.pixelated;
    const float up[4] = {pixelated ? 0.0f : std::clamp(im.scale_settings.sharpen, 0.0f, 1.0f), 0.0f, 1.0f / static_cast<float>(frame.width), 1.0f / static_cast<float>(frame.height)};
    im.device->write_buffer(im.upscale_params, 0, up, sizeof up);
    WGPURenderPassColorAttachment ca{};
    ca.view = frame.color;
    ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca.loadOp = WGPULoadOp_Clear;
    ca.storeOp = WGPUStoreOp_Store;
    ca.clearValue = {0, 0, 0, 1};
    WGPURenderPassDescriptor rp{};
    rp.label = rhi::str("pocket.upscale");
    rp.colorAttachmentCount = 1;
    rp.colorAttachments = &ca;
    WGPURenderPassEncoder enc = im.begin_pass(frame.encoder, rp);
    wgpuRenderPassEncoderSetPipeline(enc, im.upscale_pipeline);
    wgpuRenderPassEncoderSetBindGroup(enc, 0, pixelated ? im.upscale_bg_nearest : im.upscale_bg, 0, nullptr);
    wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
    wgpuRenderPassEncoderEnd(enc);
    wgpuRenderPassEncoderRelease(enc);
    return {};
}

Status Renderer::render_scene(rhi::Frame& frame, const world::World& world, rhi::Color clear, const Particles* particles, const Animation* animation, const DebugDraw* debug, const RenderView* view) {
    Impl& im = *impl_;
    // A secondary view: the first view's frame-to-frame state kept aside and put back after, TAA and
    // the volume's history off for this one.
    im.secondary = view && view->secondary;
    im.camera_pick = view ? view->camera : 0;
    im.drawing_target = view ? view->target : std::string();
    struct Kept {
        Impl& im;
        bool on;
        TaaSettings taa;
        bool taa_valid, motion_prev_set, clock_valid, volume_valid;
        Mat4 taa_prev_vp;
        float last_clock;
        int volume_cur;
        ~Kept() {
            if (!on) return;
            im.taa = taa;
            im.taa_valid = taa_valid;
            im.motion_prev_set = motion_prev_set;
            im.clock_valid = clock_valid;
            im.volume_valid = volume_valid;
            im.taa_prev_vp = taa_prev_vp;
            im.last_clock = last_clock;
            im.volume_cur = volume_cur;
            im.secondary = false;
            im.camera_pick = 0;
            im.drawing_target.clear();
        }
    } kept{im, im.secondary, im.taa, im.taa_valid, im.motion_prev_set, im.clock_valid, im.volume_valid, im.taa_prev_vp, im.last_clock, im.volume_cur};
    if (im.secondary) {
        im.taa.enabled = false;
        im.volume_valid = false;
    }
    im.timer_begin_frame();
    // Fog and ambient occlusion read the depth of a prepass: the id pass at one sample with a depth
    // target of its own (with MSAA that pass is there anyway).
    world::Fog fog;
    bool fog_on = false;
    world.ecs().each([&](flecs::entity, const world::Fog& f) {
        if (!fog_on && f.enabled && f.density > 0) { fog = f; fog_on = true; }
    });
    // Water reads the prepass's depth and adds its surfaces to it.
    im.gather_water(world);
    // Glass reads a copy of the scene drawn before it, which the split passes make room for: known
    // from the MeshRenderers that ask for it, or from last frame's draws (an asset's glass).
    bool glass_hint = im.glass_last;
    if (!glass_hint) world.ecs().each([&](flecs::entity, const world::MeshRenderer& mr) { if (mr.visible && mr.transmission > 0) glass_hint = true; });
    const bool ao_pass = im.ao.enabled || im.shadows.contact;   // the half-size pass also holds contact shadows
    const bool prepass = im.msaa > 1 || ao_pass || fog_on || im.taa.enabled || im.dof.enabled || im.motion_blur.enabled || im.ssr.enabled || im.ssgi.enabled || !im.water_bodies.empty() || glass_hint;
    if (glass_hint) POCKET_TRY_VOID(im.ensure_glass_target(frame.width, frame.height));
    if (im.msaa != im.msaa_applied || prepass != im.split_applied) POCKET_TRY_VOID(im.create_scene_pipelines(im.msaa, prepass));
    if (prepass) POCKET_TRY_VOID(im.ensure_prepass(frame.width, frame.height));
    if (ao_pass) POCKET_TRY_VOID(im.ensure_ao_targets(frame.width, frame.height));
    im.gather_occluders(world);
    im.make_scene_group(ao_pass ? im.ao_view[1] : im.ao_white_view);
    POCKET_TRY_VOID(im.ensure_id_target(frame.width, frame.height));
    POCKET_TRY_VOID(im.ensure_hdr_target(frame.width, frame.height));
    POCKET_TRY_VOID(im.ensure_msaa_targets(frame.width, frame.height, im.msaa_applied));
    im.last_width = frame.width;
    im.last_height = frame.height;
    // Clamp the requested viewport to the frame; an empty request means the whole frame.
    Viewport vp = view ? view->viewport : im.viewport;
    if (vp.w == 0 || vp.h == 0) vp = Viewport{0, 0, frame.width, frame.height};
    std::int32_t x0 = std::clamp(vp.x, 0, static_cast<std::int32_t>(frame.width));
    std::int32_t y0 = std::clamp(vp.y, 0, static_cast<std::int32_t>(frame.height));
    std::int32_t x1 = std::clamp(vp.x + static_cast<std::int32_t>(vp.w), 0, static_cast<std::int32_t>(frame.width));
    std::int32_t y1 = std::clamp(vp.y + static_cast<std::int32_t>(vp.h), 0, static_cast<std::int32_t>(frame.height));
    if (x1 <= x0 || y1 <= y0) { x0 = 0; y0 = 0; x1 = static_cast<std::int32_t>(frame.width); y1 = static_cast<std::int32_t>(frame.height); }
    im.applied = Viewport{x0, y0, static_cast<std::uint32_t>(x1 - x0), static_cast<std::uint32_t>(y1 - y0)};
    float aspect = im.applied.h > 0 ? static_cast<float>(im.applied.w) / static_cast<float>(im.applied.h) : 1.0f;
    im.stats = RenderStats{};
    im.stats.water = static_cast<std::uint32_t>(im.water_bodies.size());
    im.camera = im.find_camera(world, aspect);

    FrameUniforms fu{};
    // The cel look's bands for the lights in the mesh shader (its outlines are the post pass's).
    if (im.toon.enabled) {
        fu.toon[0] = 1.0f;
        fu.toon[1] = static_cast<float>(std::clamp(im.toon.bands, 1, 16));
        fu.toon[2] = std::clamp(im.toon.softness, 0.0f, 0.5f);
    }
    im.stats.toon = im.toon.enabled;
    // Highlighted entities, nearest first when there are more than the post pass holds.
    im.highlights.clear();
    world.ecs().each([&](flecs::entity e, const world::MeshRenderer& mr) {
        if (mr.highlight.a <= 0.0f || !mr.visible) return;
        im.highlights.push_back({static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu), {mr.highlight.r, mr.highlight.g, mr.highlight.b, std::min(mr.highlight.a, 1.0f)}});
    });
    // The bodies' boxes for the caustics on what lies below them (the first four).
    {
        const std::size_t n = std::min<std::size_t>(im.water_src.size(), 4);
        for (std::size_t i = 0; i < n; ++i) {
            const auto& [wa, c] = im.water_src[i];
            float* a = fu.waters[i * 2];
            float* b = fu.waters[i * 2 + 1];
            a[0] = c.x; a[1] = c.y; a[2] = c.z; a[3] = wa.size.x * 0.5f;
            b[0] = wa.size.y * 0.5f; b[1] = std::max(wa.depth, 0.0f); b[2] = std::max(wa.caustics, 0.0f);
        }
        fu.water_info[0] = static_cast<float>(n);
        fu.water_info[1] = static_cast<float>(world.seconds());
    }
    // TAA: the projection shifted by a Halton (2, 3) point inside the pixel, a different one each of
    // eight frames; the unjittered matrices say where things are now and were last frame.
    const Mat4 cur_vp = im.camera.proj * im.camera.view;
    Mat4 raster_vp = cur_vp;
    if (!im.taa.enabled) im.taa_valid = false;
    const bool motion_known = im.taa.enabled || im.motion_blur.enabled;   // last frame's view means something
    // The simulation clock (swaying copies), and last frame's for their motion.
    const auto clock_now = static_cast<float>(world.seconds());
    fu.clock[0] = clock_now;
    fu.clock[1] = motion_known && im.clock_valid ? im.last_clock : clock_now;
    im.last_clock = clock_now;
    im.clock_valid = true;
    if (const world::WindField wf = world::wind_field(world); wf.on) {
        fu.wind[0] = wf.dir_x;
        fu.wind[1] = wf.dir_z;
        fu.wind[2] = wf.speed;
        fu.wind[3] = 1;
        fu.wind_gust[0] = wf.gusts;
        fu.wind_gust[1] = 2 * std::numbers::pi_v<float> / wf.gust_length;
    }
    if (im.taa.enabled) {
        auto halton = [](std::uint64_t i, std::uint64_t b) {
            float f = 1, r = 0;
            for (; i > 0; i /= b) { f /= static_cast<float>(b); r += f * static_cast<float>(i % b); }
            return r;
        };
        const std::uint64_t n = im.taa_frame % 8 + 1;
        const float jx = (halton(n, 2) - 0.5f) * 2.0f / static_cast<float>(std::max(1u, im.applied.w));
        const float jy = (halton(n, 3) - 0.5f) * 2.0f / static_cast<float>(std::max(1u, im.applied.h));
        raster_vp = Mat4::translation({jx, jy, 0}) * im.camera.proj * im.camera.view;
        ++im.taa_frame;
        fu.taa[0] = 1.0f;
    }
    to_array(raster_vp, fu.view_proj);
    to_array(cur_vp, fu.cur_view_proj);
    const Mat4 prev_vp = (im.taa_valid || (motion_known && im.motion_prev_set)) ? im.taa_prev_vp : cur_vp;
    im.motion_prev_set = motion_known;
    to_array(prev_vp, fu.prev_view_proj);
    const Mat4 taa_reproject = prev_vp * cur_vp.inverse();
    im.taa_prev_vp = cur_vp;
    fu.camera_pos[0] = im.camera.position.x;
    fu.camera_pos[1] = im.camera.position.y;
    fu.camera_pos[2] = im.camera.position.z;
    fu.ambient[0] = decode(im.ambient.color.r) * im.ambient.intensity;
    fu.ambient[1] = decode(im.ambient.color.g) * im.ambient.intensity;
    fu.ambient[2] = decode(im.ambient.color.b) * im.ambient.intensity;
    fu.sun_dir[0] = 0.3f; fu.sun_dir[1] = -1.0f; fu.sun_dir[2] = -0.4f;
    fu.sun_color[0] = 0; fu.sun_color[1] = 0; fu.sun_color[2] = 0;
    bool have_sun = false;
    im.light_staging.clear();
    std::vector<std::size_t> after_dark;   // the staged lights lit only once the sun is down
    world.ecs().each([&](flecs::entity, const world::Light& l, const world::WorldTransform& t) {
        if (l.kind == 0) {
            if (have_sun) return;
            have_sun = true;
            Vec3 dir = t.rotation.rotate({0, 0, -1});
            fu.sun_dir[0] = dir.x; fu.sun_dir[1] = dir.y; fu.sun_dir[2] = dir.z;
            im.sun_toward = normalize(Vec3{-dir.x, -dir.y, -dir.z});
            fu.sun_color[0] = decode(l.color.r) * l.intensity;
            fu.sun_color[1] = decode(l.color.g) * l.intensity;
            fu.sun_color[2] = decode(l.color.b) * l.intensity;
        } else {
            GpuLight g{};
            g.pos_range[0] = t.position.x;
            g.pos_range[1] = t.position.y;
            g.pos_range[2] = t.position.z;
            g.pos_range[3] = l.range > 0 ? l.range : 0.001f;
            g.color_kind[0] = decode(l.color.r) * l.intensity;
            g.color_kind[1] = decode(l.color.g) * l.intensity;
            g.color_kind[2] = decode(l.color.b) * l.intensity;
            g.color_kind[3] = l.kind == 2 ? 2.0f : 1.0f;
            g.cone[1] = l.shadows ? 0.0f : -1.0f;   // asks for shadows; the face comes with the clustering
            if (l.after_dark) after_dark.push_back(im.light_staging.size());
            if (l.kind == 2) {
                const Vec3 dir = normalize(t.rotation.rotate({0, 0, -1}));
                const float outer = std::clamp(l.outer_angle, 0.5f, 89.5f);
                const float inner = std::clamp(l.inner_angle, 0.0f, outer - 0.25f);
                g.dir_cos[0] = dir.x; g.dir_cos[1] = dir.y; g.dir_cos[2] = dir.z;
                g.dir_cos[3] = std::cos(radians(outer));
                g.cone[0] = std::cos(radians(inner));
            }
            im.light_staging.push_back(g);
        }
    });
    // How far toward lit what is lit after dark is: the sun from four degrees above the horizon to
    // two below (lights here, meshes' glow as their draws are made); without a sun, lit.
    im.after_dark_lit = 1;
    if (have_sun) {
        const float k = std::clamp((0.07f - im.sun_toward.y) / 0.105f, 0.0f, 1.0f);
        im.after_dark_lit = k * k * (3 - 2 * k);
    }
    if (!after_dark.empty() && have_sun) {
        const float lit = im.after_dark_lit;
        if (lit <= 0) {
            // By day they are not there at all: no clustering, no shadow faces.
            for (auto it = after_dark.rbegin(); it != after_dark.rend(); ++it) im.light_staging.erase(im.light_staging.begin() + static_cast<std::ptrdiff_t>(*it));
        } else {
            for (std::size_t i : after_dark)
                for (int c = 0; c < 3; ++c) im.light_staging[i].color_kind[c] *= lit;
        }
    }
    if (!have_sun) {
        // Default key light so a scene without lights is still visible.
        fu.sun_color[0] = decode(0.9f); fu.sun_color[1] = decode(0.88f); fu.sun_color[2] = decode(0.85f);
    }
    // Night under an atmosphere (docs/design/rendering.md, A day): once the sun is down the key light
    // is the moon's, across the sky from it and lifted so it never lies along the ground (some thirty
    // degrees up at dusk and dawn), cool and a tenth as bright, rising as the sun's glow goes; the air
    // keeps the sun's afterglow and takes the moon's light besides (a deep blue sky), and the stars
    // come out once the sun is some degrees down.
    bool moon_up = false;
    Vec3 sun_way = im.sun_toward, sun_light{fu.sun_color[0], fu.sun_color[1], fu.sun_color[2]};
    if (have_sun) {
        int sky_mode = 0;
        world.ecs().each([&](flecs::entity, const world::Sky& s) {
            if (!sky_mode && s.enabled && s.mode >= 1 && s.mode <= 3) sky_mode = s.mode;
        });
        if (sky_mode == 3 && im.sun_toward.y < 0) {
            moon_up = true;
            const float k = std::clamp(-im.sun_toward.y / 0.1f, 0.0f, 1.0f);
            const float rise = k * k * (3 - 2 * k);
            constexpr float kMoon[3] = {0.05f, 0.06f, 0.1f};
            im.sun_toward = normalize(Vec3{-sun_way.x, 0.6f - sun_way.y, -sun_way.z});
            for (int i = 0; i < 3; ++i) fu.sun_color[i] *= kMoon[i] * rise;
            fu.sun_dir[0] = -im.sun_toward.x; fu.sun_dir[1] = -im.sun_toward.y; fu.sun_dir[2] = -im.sun_toward.z;
            const float dark = std::clamp((-sun_way.y - 0.05f) / 0.2f, 0.0f, 1.0f);
            fu.night[0] = dark * dark * (3 - 2 * dark);
        }
    }
    // The weather (docs/design/rendering.md, Weather): the first enabled by id.
    float overcast = 0;
    bool weather_on = false;
    im.weather_drops = 0;
    {
        world::EntityId wid = 0;
        world::Weather wx;
        world.ecs().each([&](flecs::entity e, const world::Weather& x) {
            if (x.enabled && (!wid || e.id() < wid)) { wid = e.id(); wx = x; }
        });
        if (wid) {
            const float rain = std::clamp(wx.rain, 0.0f, 1.0f), snow = std::clamp(wx.snow, 0.0f, 1.0f);
            const float density = std::clamp(wx.density, 0.0f, 4.0f);
            const auto rain_n = static_cast<std::uint32_t>(rain * 9000.0f * density);
            const auto snow_n = static_cast<std::uint32_t>(snow * 7000.0f * density);
            fu.weather[0] = std::clamp(wx.wet, 0.0f, 1.0f);
            fu.weather[1] = std::clamp(wx.cover, 0.0f, 1.0f);
            fu.weather[2] = static_cast<float>(rain_n);
            fu.weather[3] = static_cast<float>(snow_n);
            im.weather_drops = rain_n + snow_n;
            overcast = std::clamp(wx.overcast >= 0 ? wx.overcast : 0.7f * std::max(rain, snow), 0.0f, 1.0f);
            weather_on = rain > 0 || snow > 0 || fu.weather[0] > 0 || fu.weather[1] > 0;
        }
    }
    // The shelter map's view: a square kShelterReach each way about the camera (its centre on a
    // two-unit grid, so the texels stay put), 80 units above it to 80 below, from straight above.
    // Drawn again when the camera crosses to another square, when what stands over it moves or
    // changes (below, by the draws), and every eight frames.
    bool shelter_due = false;
    if (!weather_on) im.shelter_valid = false;
    else if (!im.secondary) {
        const float cx = std::round(im.camera.position.x / 2) * 2, cz = std::round(im.camera.position.z / 2) * 2;
        if (!im.shelter_valid || cx != im.shelter_x || cz != im.shelter_z || frame.index >= im.shelter_frame + 8) {
            const float top = std::round(im.camera.position.y) + 80;
            const Mat4 view = Mat4::look_at({cx, top, cz}, {cx, top - 1, cz}, {0, 0, -1});
            const Mat4 proj = Mat4::orthographic(-kShelterReach, kShelterReach, -kShelterReach, kShelterReach, 0.0f, 160.0f);
            to_array(proj * view, im.shelter_vp);
            im.shelter_bias = 0.08f / 160.0f;   // a surface is not under itself
            im.shelter_x = cx;
            im.shelter_z = cz;
            im.shelter_frame = frame.index;
            im.shelter_valid = true;
            shelter_due = true;
        }
    }
    im.stats.shelter_draws = im.shelter_valid ? im.shelter_draws : 0;
    if (im.shelter_valid) {
        std::memcpy(fu.shelter_vp, im.shelter_vp, sizeof fu.shelter_vp);
        fu.shelter[0] = 1;
        fu.shelter[1] = im.shelter_bias;
    }
    im.stats.weather_drops = im.weather_drops;
    im.cluster_lights(fu);
    im.stats.has_sun = have_sun;
    // The sun's orthographic view fits a sphere around everything with Bounds.
    bool shadows_on = im.shadows.enabled;
    {
        Vec3 lo{0, 0, 0}, hi{0, 0, 0};
        bool any = false;
        world.ecs().each([&](flecs::entity, const world::Bounds& b) {
            if (!any) { lo = b.min; hi = b.max; any = true; return; }
            lo = {std::min(lo.x, b.min.x), std::min(lo.y, b.min.y), std::min(lo.z, b.min.z)};
            hi = {std::max(hi.x, b.max.x), std::max(hi.y, b.max.y), std::max(hi.z, b.max.z)};
        });
        if (!any) shadows_on = false;
        const Vec3 scene_center = (lo + hi) * 0.5f;
        const Vec3 ext = (hi - lo) * 0.5f;
        const float scene_radius = std::max(1.0f, length(ext));
        const Vec3 dir = normalize(Vec3{fu.sun_dir[0], fu.sun_dir[1], fu.sun_dir[2]});
        const Vec3 up = std::abs(dir.y) > 0.99f ? Vec3{1, 0, 0} : Vec3{0, 1, 0};
        // The whole view frustum's corners (near, then far), from which each slice is cut by view depth.
        const Mat4 inv = (im.camera.proj * im.camera.view).inverse();
        Vec3 corner[8];
        for (int k = 0; k < 8; ++k) {
            const Vec4 c = inv * Vec4{(k & 1) ? 1.0f : -1.0f, (k & 2) ? 1.0f : -1.0f, (k & 4) ? 1.0f : 0.0f, 1.0f};
            corner[k] = Vec3{c.x / c.w, c.y / c.w, c.z / c.w};
        }
        const Vec3 fwd = im.camera.forward;
        auto depth_of = [&](Vec3 p) { return dot(p - im.camera.position, fwd); };
        const float near_d = std::max(0.01f, depth_of(corner[0]));
        const float far_d = depth_of(corner[4]);
        // Shadows reach the setting's distance, no farther than the scene or the camera sees.
        const float scene_far = depth_of(scene_center) + scene_radius;
        const float reach = std::max(near_d + 0.1f, std::min({im.shadows.distance, far_d, scene_far}));
        const int count = std::clamp(im.shadows.cascades, 1, static_cast<int>(kCascades));
        float splits[kCascades + 1];
        splits[0] = near_d;
        for (int c = 1; c <= count; ++c) {
            // Practical splits: mostly logarithmic, a fifth uniform.
            const float t = static_cast<float>(c) / static_cast<float>(count);
            const float log_split = near_d * std::pow(reach / near_d, t);
            const float uni_split = near_d + (reach - near_d) * t;
            splits[c] = 0.8f * log_split + 0.2f * uni_split;
        }
        std::uint8_t slots[256 * kCascades] = {};
        for (int c = 0; c < static_cast<int>(kCascades); ++c) {
            const int cc = std::min(c, count - 1);
            // The slice's eight corners along the frustum's edges, their bounding sphere (radius rounded
            // up, so it does not breathe as the camera turns).
            Vec3 pts[8];
            Vec3 mid{0, 0, 0};
            for (int k = 0; k < 4; ++k) {
                const Vec3 a = corner[k], b = corner[k + 4];
                const float da = depth_of(a), db = depth_of(b);
                auto at = [&](float d) { return a + (b - a) * ((d - da) / std::max(db - da, 1e-6f)); };
                pts[k] = at(splits[cc]);
                pts[k + 4] = at(splits[cc + 1]);
                mid = mid + pts[k] + pts[k + 4];
            }
            mid = mid * (1.0f / 8.0f);
            float r = 0;
            for (const Vec3& p : pts) r = std::max(r, length(p - mid));
            r = std::ceil(r * 16.0f) / 16.0f;
            // Casters toward the sun: back to the far side of the scene, whatever is behind the slice.
            const float back = std::max(r, dot(mid - scene_center, dir) + scene_radius) + 1.0f;
            const Mat4 light_view = Mat4::look_at(mid - dir * back, mid, up);
            Mat4 light_proj = Mat4::orthographic(-r, r, -r, r, 0.05f, back + r + 1.0f);
            // Snap to the texel grid, so the shadow's edge does not crawl as the camera moves.
            const Mat4 vp0 = light_proj * light_view;
            const Vec4 o = vp0 * Vec4{0, 0, 0, 1};
            const float half = static_cast<float>(kShadowMapSize) * 0.5f;
            const float sx = std::round(o.x * half) / half - o.x, sy = std::round(o.y * half) / half - o.y;
            light_proj = Mat4::translation({sx, sy, 0}) * light_proj;
            const Mat4 vp = light_proj * light_view;
            to_array(vp, fu.cascade_vp[c]);
            std::memcpy(slots + 256 * c, fu.cascade_vp[c], sizeof(float) * 16);
            fu.cascade_far[c] = splits[cc + 1];
            fu.cascade_texel[c] = 2.0f * r / static_cast<float>(kShadowMapSize);
            fu.cascade_depth[c] = back + r + 1.0f - 0.05f;
        }
        if (shadows_on) im.device->write_buffer(im.cascade_buffer, 0, slots, sizeof slots);
        fu.camera_fwd[0] = fwd.x; fu.camera_fwd[1] = fwd.y; fu.camera_fwd[2] = fwd.z; fu.camera_fwd[3] = static_cast<float>(count);
        std::memcpy(fu.light_view_proj, fu.cascade_vp[0], sizeof(float) * 16);
        im.stats.shadow_cascades = shadows_on ? count : 0;
        im.stats.shadow_distance = shadows_on ? splits[count] : 0.0f;
        fu.shadow[0] = 1.0f / static_cast<float>(kShadowMapSize);
        fu.shadow[1] = im.shadows.bias;
        fu.shadow[2] = im.shadows.strength;
        fu.shadow[3] = shadows_on ? 1.0f : 0.0f;
        fu.shadow_soft[0] = std::tan(radians(im.shadows.softness));
        fu.shadow_soft[2] = 8.0f;   // blockers up to this far in front of a point soften its shadow
    }
    im.contact_now = im.shadows.contact && have_sun;
    fu.shadow_soft[1] = im.contact_now ? 1.0f : 0.0f;
    im.stats.soft_shadows = shadows_on && im.shadows.softness > 0;
    im.stats.contact_shadows = im.contact_now;
    im.stats.shadows = shadows_on;
    // The sky: the first enabled Sky; its environment is rebuilt when anything it is made of changed.
    world::Sky sky;
    bool has_sky = false;
    world.ecs().each([&](flecs::entity, const world::Sky& s) {
        if (!has_sky && s.enabled && s.mode >= 1 && s.mode <= 3) { sky = s; has_sky = true; }
    });
    bool env_on = false;
    if (has_sky) {
        const Vec3 toward = normalize(Vec3{-fu.sun_dir[0], -fu.sun_dir[1], -fu.sun_dir[2]});
        const Vec3 key_light{fu.sun_color[0], fu.sun_color[1], fu.sun_color[2]};
        im.env_overcast = overcast;
        env_on = moon_up ? im.update_environment(frame, sky, have_sun, sun_way, sun_light, toward, key_light)
                         : im.update_environment(frame, sky, have_sun, toward, key_light);
        if (!env_on) has_sky = false;
        im.fog_from_sky = env_on && sky.mode == 3;
        if (im.fog_from_sky) {
            // The mean of the air a little above the horizon, all round (the sky's intensity with it).
            Vec3 sum{0, 0, 0};
            const Vec3 sun_now{fu.sun_color[0], fu.sun_color[1], fu.sun_color[2]};
            for (int k = 0; k < 8; ++k) {
                const float a = static_cast<float>(k) * std::numbers::pi_v<float> / 4;
                const Vec3 d = normalize(Vec3{std::cos(a), 0.05f, std::sin(a)});
                if (!have_sun) continue;
                sum += moon_up ? atmosphere_air(d, sun_way, sky.haze, sun_light) + atmosphere_air(d, toward, sky.haze, sun_now) : atmosphere_air(d, toward, sky.haze, sun_now);
            }
            im.fog_sky = sum * (std::max(sky.intensity, 0.0f) / 8.0f);
        }
        if (env_on && sky.mode == 3 && have_sun) {
            // The sun light through the air: as the light says at noon, redder and dimmer low down,
            // gone below the horizon.
            const Vec3 tint = atmosphere_tint(toward, sky.haze);
            fu.sun_color[0] *= tint.x;
            fu.sun_color[1] *= tint.y;
            fu.sun_color[2] *= tint.z;
        }
        if (env_on && sky.mode == 3 && std::max(sky.clouds, overcast) > 0) {
            // Clouds drift with the Wind (six times faster up there), or at 12 along +x without one;
            // an overcast Weather covers the sky at least as much as it is overcast.
            const auto t = static_cast<float>(world.seconds());
            const world::WindField wf = world::wind_field(world);
            const Vec3 v = wf.on ? Vec3{wf.dir_x * wf.speed * 6, 0, wf.dir_z * wf.speed * 6} : Vec3{12, 0, 0};
            fu.clouds[0] = std::clamp(std::max(sky.clouds, overcast), 0.0f, 1.0f);
            fu.clouds[1] = std::max(sky.cloud_height, 1.0f);
            fu.clouds[2] = 1.0f / std::max(sky.cloud_scale, 1.0f);
            fu.clouds[3] = 1;
            fu.cloud_drift[0] = -v.x * t;
            fu.cloud_drift[1] = -v.z * t;
        }
    }
    // Overcast, the sun's direct light is dimmed (the sky's own light, built from it above, is not).
    for (int k = 0; k < 3; ++k) fu.sun_color[k] *= 1.0f - 0.8f * overcast;
    to_array((im.camera.proj * im.camera.view).inverse(), fu.inv_view_proj);
    for (int k = 0; k < 3; ++k) im.stats.sun_light[k] = have_sun ? fu.sun_color[k] : 0.0f;
    fu.env[0] = env_on && (sky.diffuse > 0 || sky.specular > 0) ? 1.0f : 0.0f;
    fu.env[1] = std::max(sky.diffuse, 0.0f);
    fu.env[2] = std::max(sky.specular, 0.0f);
    fu.env[3] = static_cast<float>(kEnvLevels - 1);
    fu.sky[0] = has_sky ? static_cast<float>(sky.mode) : 0.0f;
    fu.sky[1] = std::cos(radians(std::clamp(sky.sun_size, 0.0f, 30.0f) * 0.5f));
    fu.sky[2] = sky.sun_size > 0 ? 40.0f : 0.0f;
    fu.sky[3] = have_sun ? 1.0f : 0.0f;
    im.stats.sky = has_sky ? sky.mode : 0;
    fu.ao[0] = im.ao.enabled ? 1.0f : 0.0f;
    fu.ao[1] = 1.0f / static_cast<float>(std::max(1u, frame.width));
    fu.ao[2] = 1.0f / static_cast<float>(std::max(1u, frame.height));
    if (has_sky) fu.ambient[0] = fu.ambient[1] = fu.ambient[2] = 0.0f;   // the sky's light replaces the flat ambient, even at zero
    // The probes and volumes light every view; the first view captures.
    std::vector<Impl::GridCapture> grid_captures = im.gather_grids(world, fu, im.probe_refresh, !im.secondary);
    int probe_capture = im.gather_probes(world, fu);
    if (im.secondary) probe_capture = -1;
    im.gather_decals(world, fu);
    for (int k = 0; k < 4; ++k) {
        fu.occluders[k] = im.occ_uniform[k];
        fu.occluder_size[k] = im.occ_uniform[4 + k];
    }
    ++im.frame_number;
    im.device->write_buffer(im.frame_buffer, 0, &fu, sizeof fu);

    // Gather one object per primitive entity and one per material of a glTF entity, sorted by
    // texture, mesh and submesh so equal runs become single instanced draws and the order is
    // deterministic.
    // One level of detail a draw can take (docs/design/rendering.md, Levels of detail): below
    // `screen` (the fraction of the view's height its bounds cover), this geometry.
    struct LodLevel {
        float screen;
        const GpuMesh* gpu;
        std::uint32_t first, count;
        std::string mesh;
    };
    struct Draw {
        std::string texture;
        std::string mesh;
        const GpuMesh* gpu;
        std::uint32_t first, count;
        WGPUBindGroup material;
        ObjectUniforms object;
        bool skinned = false;
        bool blend = false;   // translucent: after every opaque draw, far to near
        float depth = 0;      // along the camera's forward, for that order
        bool cutout = false;  // an alpha cutoff: its shadow keeps the holes
        bool glass = false;   // transmits: drawn after the rest, over a copy of it (docs/design/rendering.md, Glass)
        std::shared_ptr<const std::vector<LodLevel>> lods;   // simpler meshes for when it is small on screen, largest screen first
        bool in_view = true;  // its bounds reach into the camera's view (the camera's passes draw only these)
        float cull = 0;       // not drawn below this fraction of the view's height
        Vec3 center{0, 0, 0}; // the entity's bounding sphere in the world (radius < 0: unknown, always drawn)
        float radius = -1;
        Impl::SpriteMaterial* user = nullptr;   // a material the project wrote (MeshRenderer.material)
    };
    std::vector<Draw> draws;
    std::uint32_t count = 0;
    std::uint32_t entities = 0;
    std::uint32_t skinned_instances = 0;
    std::uint32_t morphed_instances = 0;
    std::uint32_t translucent_instances = 0;
    std::uint32_t glass_instances = 0;
    std::uint32_t moving_parts = 0;
    im.animation = animation;
    im.joint_count = 0;
    world.ecs().each([&](flecs::entity e, const world::MeshRenderer& mr, const world::WorldTransform& t) {
        if (!mr.visible || count >= kMaxObjects) return;
        ++entities;
        // A mesh the engine made for the entity (a terrain's) stands in for MeshRenderer.mesh.
        const std::string* derived = world.derived_mesh(e.id());
        const std::string& mesh_path = derived ? *derived : mr.mesh;
        Mat4 model = Mat4::trs(t.position, t.rotation, t.scale);
        ObjectUniforms ou{};
        to_array(model, ou.model);
        to_array(transpose(model.inverse_affine()), ou.normal);
        ou.id[0] = static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu);
        ou.id[1] = mr.cast_shadows ? 0u : 2u;
        // Its levels of detail for one part (submesh `si`) of a primitive (kind) or an asset.
        std::shared_ptr<const std::vector<LodLevel>> pending_lods;
        auto levels_for = [&](int kind, std::size_t si) -> std::shared_ptr<const std::vector<LodLevel>> {
            if (mr.lods.empty()) return nullptr;
            auto out = std::make_shared<std::vector<LodLevel>>();
            for (const world::MeshLod& l : mr.lods) {
                LodLevel lv{l.screen, nullptr, 0, 0, ""};
                if (!l.mesh.empty()) {
                    // Another mesh: a primitive, or an asset whose parts match (the last stands in for any beyond it).
                    if (const int other = Impl::primitive_index(l.mesh); other >= 0) {
                        const GpuMesh& g = im.meshes[static_cast<std::size_t>(other)];
                        lv = {l.screen, &g, 0, g.index_count, l.mesh};
                    } else if (Impl::AssetMesh* am2 = im.asset_mesh(l.mesh); am2 && !am2->submeshes.empty()) {
                        const assets::Submesh& sm = am2->submeshes[std::min(si, am2->submeshes.size() - 1)];
                        lv = {l.screen, &am2->gpu, sm.first_index, sm.index_count, l.mesh};
                    }
                } else if (const Impl::LodMesh* lm = im.lod_mesh(kind, mesh_path, l.ratio)) {
                    const std::string key = std::format("{}#lod{:.3f}", mesh_path, std::clamp(l.ratio, 0.01f, 1.0f));
                    if (kind >= 0) lv = {l.screen, &lm->gpu, 0, lm->gpu.index_count, key};
                    else if (si < lm->submeshes.size()) lv = {l.screen, &lm->gpu, lm->submeshes[si].first_index, lm->submeshes[si].index_count, key};
                }
                if (lv.gpu && lv.count > 0) out->push_back(std::move(lv));
            }
            std::stable_sort(out->begin(), out->end(), [](const LodLevel& a, const LodLevel& b) { return a.screen > b.screen; });
            if (out->empty()) return nullptr;
            return out;
        };
        // Material inputs: the asset's material, overridden per entity by the MeshRenderer.
        auto push = [&](const GpuMesh* gpu, std::uint32_t first, std::uint32_t n, const std::string& tex, Vec4 color, const std::string& mesh_key, const assets::Material* mat, bool skinned = false) {
            if (count >= kMaxObjects) return;
            if (!im.drawing_target.empty() && tex.starts_with("view:") && tex.substr(5) == im.drawing_target) return;   // a screen is not drawn into its own picture
            ou.color[0] = color.x; ou.color[1] = color.y; ou.color[2] = color.z; ou.color[3] = color.w;
            float metallic = mr.metallic >= 0 ? mr.metallic : (mat ? mat->metallic : 0.0f);
            float roughness = mr.roughness >= 0 ? mr.roughness : (mat ? mat->roughness : 1.0f);
            const std::string& normal_map = !mr.normal_map.empty() ? mr.normal_map : (mat ? mat->normal_texture : std::string{});
            const std::string& mr_map = mat ? mat->metallic_roughness_texture : std::string{};
            const std::string& em_map = mat ? mat->emissive_texture : std::string{};
            ou.pbr[0] = metallic;
            ou.pbr[1] = roughness;
            ou.pbr[2] = mat ? mat->normal_scale : 1.0f;
            ou.pbr[3] = normal_map.empty() ? 0.0f : 1.0f;
            // The material's texture transform folds into the uv rectangle: uv' = offset + scale * uv.
            if (mat && mat->uv_transformed) {
                ou.uv_rect[0] = mat->uv_offset.x; ou.uv_rect[1] = mat->uv_offset.y;
                ou.uv_rect[2] = mat->uv_offset.x + mat->uv_scale.x; ou.uv_rect[3] = mat->uv_offset.y + mat->uv_scale.y;
            } else {
                ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
            }
            ou.emissive[0] = decode(mr.emissive.r) + (mat ? mat->emissive.x : 0.0f);
            ou.emissive[1] = decode(mr.emissive.g) + (mat ? mat->emissive.y : 0.0f);
            ou.emissive[2] = decode(mr.emissive.b) + (mat ? mat->emissive.z : 0.0f);
            if (mr.after_dark) for (int k = 0; k < 3; ++k) ou.emissive[k] *= im.after_dark_lit;   // glowing once the sun is down
            ou.emissive[3] = mr.cutoff > 0 ? mr.cutoff : (mat ? mat->alpha_cutoff : 0.0f);
            WGPUBindGroup group = nullptr;
            std::string material_key = tex + "|" + normal_map + "|" + mr_map;
            for (int k = 0; k < 4; ++k) ou.terrain[k] = ou.layer_tile[k] = 0;
            if (mat && mat->terrain && !mat->terrain->layers.empty() && mat->terrain->n > 1) {
                // A terrain from textured layers (docs/design/terrain.md, Layers).
                const assets::TerrainLayers& tl = *mat->terrain;
                const float texels = static_cast<float>(tl.n - 1) / static_cast<float>(tl.n);
                ou.terrain[0] = static_cast<float>(std::min<std::size_t>(tl.layers.size(), 4));
                ou.terrain[1] = tl.texture_tile / std::max(tl.size.x, 1e-3f) * texels;
                ou.terrain[2] = tl.texture_tile / std::max(tl.size.y, 1e-3f) * texels;
                for (std::size_t k = 0; k < std::min<std::size_t>(tl.layers.size(), 4); ++k) ou.layer_tile[k] = tl.texture_tile / std::max(tl.layers[k].tile, 1e-3f);
                group = im.terrain_group(mesh_key, tl);
                material_key = "terrain|" + mesh_key;
            } else {
                group = im.material_for(tex, mr_map, normal_map, em_map, false);
            }
            // Glass and lacquer: the MeshRenderer's values, or the material's (docs/design/rendering.md, Glass).
            const float transmission = std::clamp(mr.transmission >= 0 ? mr.transmission : (mat ? mat->transmission : 0.0f), 0.0f, 1.0f);
            ou.optics[0] = transmission;
            ou.optics[1] = std::max(mr.ior >= 0 ? mr.ior : (mat ? mat->ior : 1.5f), 1.0f);
            ou.optics[2] = std::clamp(mr.clearcoat >= 0 ? mr.clearcoat : (mat ? mat->clearcoat : 0.0f), 0.0f, 1.0f);
            ou.optics[3] = std::clamp(mr.clearcoat_roughness >= 0 ? mr.clearcoat_roughness : (mat ? mat->clearcoat_roughness : 0.03f), 0.0f, 1.0f);
            const float across = (std::fabs(t.scale.x) + std::fabs(t.scale.y) + std::fabs(t.scale.z)) / 3.0f;
            ou.volume[3] = mr.thickness >= 0 ? mr.thickness : (mat ? mat->thickness * across : 0.0f);   // the asset's is in its own units
            for (int k = 0; k < 3; ++k) {
                const float left = mat ? (&mat->attenuation_color.x)[k] : 1.0f;
                ou.volume[k] = mat && mat->attenuation_distance > 0 ? -std::log(std::clamp(left, 1e-4f, 1.0f)) / (mat->attenuation_distance * across) : 0.0f;
            }
            // Cloth, specular and brushed metal, and unlit (docs/design/rendering.md, Cloth, specular
            // and brushed metal): the MeshRenderer's values, or the material's.
            const bool own_sheen = mr.sheen.r > 0 || mr.sheen.g > 0 || mr.sheen.b > 0;
            const Vec3 sheen = own_sheen ? Vec3{decode(mr.sheen.r), decode(mr.sheen.g), decode(mr.sheen.b)} : (mat ? mat->sheen_color : Vec3{0, 0, 0});
            ou.sheen[0] = sheen.x; ou.sheen[1] = sheen.y; ou.sheen[2] = sheen.z;
            ou.sheen[3] = std::clamp(mr.sheen_roughness >= 0 ? mr.sheen_roughness : (mat ? mat->sheen_roughness : 0.0f), 0.0f, 1.0f);
            const Vec3 tint = mat ? mat->specular_color : Vec3{1, 1, 1};
            ou.spec[0] = tint.x - 1; ou.spec[1] = tint.y - 1; ou.spec[2] = tint.z - 1;
            ou.spec[3] = 1.0f - std::clamp(mr.specular >= 0 ? mr.specular : (mat ? mat->specular : 1.0f), 0.0f, 1.0f);
            const float aniso = std::clamp(mr.anisotropy >= 0 ? mr.anisotropy : (mat ? mat->anisotropy : 0.0f), 0.0f, 1.0f);
            const float turn = mr.anisotropy >= 0 ? radians(mr.anisotropy_rotation) : (mat ? mat->anisotropy_rotation : 0.0f);
            ou.aniso[0] = aniso; ou.aniso[1] = std::cos(turn); ou.aniso[2] = std::sin(turn);
            if (mr.unlit || (mat && mat->unlit)) ou.id[1] |= 4u;
            // Textured from the world's axes (MeshRenderer.texture_tile): repeats a unit in uv_rect.x.
            if (mr.texture_tile > 0.0f) {
                ou.id[1] |= 8u;
                ou.uv_rect[0] = 1.0f / mr.texture_tile;
            }
            const bool glass = transmission > 0.001f;
            const bool blend = !glass && (color.w < 0.999f || (mat && mat->blend));
            Impl::SpriteMaterial* user = nullptr;
            if (!mr.material.empty() && !skinned && !blend && !glass) {
                if (auto it = im.mesh_materials.find(mr.material); it != im.mesh_materials.end() && it->second.module) user = &it->second;
                ou.user[0] = mr.material_params.x; ou.user[1] = mr.material_params.y; ou.user[2] = mr.material_params.z; ou.user[3] = mr.material_params.w;
            }
            const Vec3 to_cam = t.position - im.camera.position;
            const float depth = to_cam.x * im.camera.forward.x + to_cam.y * im.camera.forward.y + to_cam.z * im.camera.forward.z;
            draws.push_back({material_key, mesh_key, gpu, first, n, group, ou, skinned, blend, depth, ou.emissive[3] > 0.0f, glass});
            draws.back().user = draws.back().cutout ? nullptr : user;
            if (glass) ++glass_instances;
            draws.back().lods = pending_lods;
            draws.back().cull = mr.cull_screen;
            if (const auto* b = e.try_get<world::Bounds>(); b && !skinned) {
                draws.back().center = (b->min + b->max) * 0.5f;
                draws.back().radius = length(b->max - b->min) * 0.5f;
            }
            if (blend) ++translucent_instances;
            ++count;
        };
        ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
        ou.morph[0] = ou.morph[1] = ou.morph[2] = ou.morph[3] = 0;
        for (float& mw : ou.morph_weights) mw = 0;
        int kind = Impl::primitive_index(mesh_path);
        if (kind >= 0) {
            const GpuMesh& gm = im.meshes[static_cast<std::size_t>(kind)];
            pending_lods = levels_for(kind, 0);
            push(&gm, 0, gm.index_count, mr.texture, {decode(mr.color.r), decode(mr.color.g), decode(mr.color.b), mr.color.a}, mesh_path, nullptr);
            return;
        }
        Impl::AssetMesh* am = im.asset_mesh(mesh_path);
        if (!am) {
            // Missing asset: a magenta cube marks the spot instead of hiding the problem.
            const GpuMesh& gm = im.meshes[0];
            push(&gm, 0, gm.index_count, "", {1, 0, 1, 1}, "cube", nullptr);
            return;
        }
        // One node drawn alone (MeshRenderer.node): its geometry moved back into the node's own
        // space, so the entity's transform places it as the node's would.
        int only = -1;
        if (!mr.node.empty()) {
            for (std::size_t ni = 0; ni < am->node_names.size(); ++ni) if (!am->node_names[ni].empty() && am->node_names[ni] == mr.node) { only = static_cast<int>(ni); break; }
            if (only < 0 && std::all_of(mr.node.begin(), mr.node.end(), [](char c) { return c >= '0' && c <= '9'; })) {
                const int idx = std::atoi(mr.node.c_str());
                if (idx >= 0 && static_cast<std::size_t>(idx) < am->node_names.size()) only = idx;
            }
            if (only < 0) {
                im.report_missing(mesh_path + "#" + mr.node, "no such node in the file");
                return;
            }
        }
        // A terrain's squares (docs/design/terrain.md, Levels of detail): each a draw on its own
        // bounds, whose levels are allowed while a sample's height error stays under a pixel and a
        // cell under four: for an error e at distance w, e * P11 / w * H / 2 <= 1, so the level
        // goes in at a screen fraction (r * P11 / w) of 2r / (H e).
        if (!am->chunks.empty() && only < 0 && !am->materials.empty()) {
            const assets::Material& mat = am->materials.front();
            const Vec4 color{decode(mr.color.r) * mat.base_color.x, decode(mr.color.g) * mat.base_color.y, decode(mr.color.b) * mat.base_color.z, mr.color.a * mat.base_color.w};
            const float across = std::max({std::fabs(t.scale.x), std::fabs(t.scale.z), 1e-6f});
            const float grow = std::max(across, std::fabs(t.scale.y));
            const float up = std::max(std::fabs(t.scale.y), 1e-6f);
            const float view_h = static_cast<float>(std::max(1u, frame.height));
            for (const assets::TerrainChunk& ch : am->chunks) {
                if (ch.levels.empty()) continue;
                const float r = ch.radius * grow;
                std::shared_ptr<std::vector<LodLevel>> lv;
                float last = 1e30f;
                for (std::size_t k = 1; k < ch.levels.size(); ++k) {
                    const assets::TerrainChunk::Level& l = ch.levels[k];
                    float at = 2.0f * 4.0f * r / (view_h * static_cast<float>(l.stride) * ch.cell * across);
                    if (l.error > 0) at = std::min(at, 2.0f * 2.0f * r / (view_h * l.error * up));
                    last = std::min(last, at);
                    if (!lv) lv = std::make_shared<std::vector<LodLevel>>();
                    lv->push_back({last, &am->gpu, l.first, l.count, mesh_path});
                }
                pending_lods = lv;
                const std::size_t before = draws.size();
                push(&am->gpu, ch.levels[0].first, ch.levels[0].count, mr.texture.empty() ? mat.texture : mr.texture, color, mesh_path, &mat);
                if (draws.size() > before) {
                    draws.back().center = model.transform_point(ch.center);
                    draws.back().radius = r;
                }
            }
            pending_lods = nullptr;
            return;
        }
        // A posed skin: its joint matrices go into the joint buffer once per entity and skin.
        const Pose* pose = animation ? animation->pose(e.id()) : nullptr;
        std::vector<std::uint32_t> joint_base(pose ? pose->joints.size() : 0, kMaxJoints);
        if (pose && am->gpu.skin) {
            for (std::size_t s = 0; s < pose->joints.size(); ++s) {
                const auto& jm = pose->joints[s];
                if (im.joint_count + jm.size() > kMaxJoints) break;
                joint_base[s] = im.joint_count;
                for (const Mat4& m : jm) {
                    std::memcpy(im.joint_staging.data() + static_cast<std::size_t>(im.joint_count) * 16, m.m, sizeof(float) * 16);
                    ++im.joint_count;
                }
            }
        }
        // Morph targets: the asset's deltas and this entity's weights (from its pose), when any is set.
        bool morphed = false;
        if (pose && am->morph_targets > 0) {
            for (std::uint32_t t = 0; t < am->morph_targets && t < pose->weights.size(); ++t) {
                ou.morph_weights[t] = pose->weights[t];
                if (pose->weights[t] != 0) morphed = true;
            }
            if (morphed) {
                ou.morph[0] = am->morph_base;
                ou.morph[1] = am->morph_vertices;
                ou.morph[2] = am->morph_targets;
            }
        }
        for (std::size_t si = 0; si < am->submeshes.size(); ++si) {
            const assets::Submesh& sm = am->submeshes[si];
            if (only >= 0 && sm.origin != only) continue;
            const assets::Material& mat = am->materials[std::min<std::size_t>(sm.material, am->materials.size() - 1)];
            Vec4 color{decode(mr.color.r) * mat.base_color.x, decode(mr.color.g) * mat.base_color.y, decode(mr.color.b) * mat.base_color.z, mr.color.a * mat.base_color.w};
            const bool skinned = sm.skin >= 0 && static_cast<std::size_t>(sm.skin) < joint_base.size() && joint_base[static_cast<std::size_t>(sm.skin)] < kMaxJoints;
            ou.id[2] = skinned ? joint_base[static_cast<std::size_t>(sm.skin)] : 0;
            if (skinned) ++skinned_instances;
            if (morphed) ++morphed_instances;
            // A moving part: placed by its node's matrix from the pose (its rest without one). A node
            // drawn alone is the entity itself: baked geometry is moved back into the node's space.
            const bool part = sm.node >= 0;
            const bool alone = only >= 0;
            if (part || alone) {
                Mat4 placed = model;
                if (alone) { if (!part) placed = model * am->unbake[si]; }
                else placed = model * (pose && static_cast<std::size_t>(sm.node) < pose->globals.size() ? pose->globals[static_cast<std::size_t>(sm.node)] : am->rest[si]);
                to_array(placed, ou.model);
                to_array(transpose(placed.inverse_affine()), ou.normal);
                if (part) ++moving_parts;
            }
            pending_lods = skinned ? nullptr : levels_for(-1, si);   // a skinned mesh keeps its detail
            push(&am->gpu, sm.first_index, sm.index_count, mr.texture.empty() ? mat.texture : mr.texture, color, mesh_path, &mat, skinned);
            if (part || alone) {
                to_array(model, ou.model);
                to_array(transpose(model.inverse_affine()), ou.normal);
            }
        }
        ou.id[2] = 0;
    });
    // Scattered copies (World::derived_instances, a Scatter's): every draw of such an entity is
    // made once per copy at the copy's matrix, its colour shaded, culled on its own bounds; the
    // entity's own draw goes. A copy draws the mesh's geometry as the file bakes it; its Scatter's
    // sway goes to the vertex shader, and past its fade distance it is left out (shrunk into the
    // ground over the last fifth).
    std::uint32_t scattered = 0;
    {
        std::unordered_map<std::uint32_t, std::pair<const std::vector<world::World::Instance>*, const world::Scatter*>> copies;
        world.ecs().each([&](flecs::entity e, const world::MeshRenderer&) {
            if (const auto* in = world.derived_instances(e.id())) copies[static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu)] = {in, e.try_get<world::Scatter>()};
        });
        if (!copies.empty()) {
            auto largest_axis = [](const Mat4& m) {
                return std::max({length(Vec3{m.at(0, 0), m.at(0, 1), m.at(0, 2)}), length(Vec3{m.at(1, 0), m.at(1, 1), m.at(1, 2)}), length(Vec3{m.at(2, 0), m.at(2, 1), m.at(2, 2)})});
            };
            std::vector<Draw> kept;
            kept.reserve(draws.size());
            std::vector<Draw> made;
            for (Draw& d : draws) {
                auto it = copies.find(d.object.id[0]);
                if (it == copies.end()) { kept.push_back(std::move(d)); continue; }
                Mat4 base;
                std::memcpy(base.m, d.object.model, sizeof base.m);
                const float base_scale = std::max(largest_axis(base), 1e-6f);
                const Vec3 local_centre = d.radius >= 0 ? base.inverse_affine().transform_point(d.center) : Vec3{0, 0, 0};
                const float local_radius = d.radius >= 0 ? d.radius / base_scale : 0.5f;
                const world::Scatter* sc = it->second.second;
                const float fade = sc ? sc->fade : 0.0f;
                for (const world::World::Instance& inst : *it->second.first) {
                    if (kept.size() + made.size() >= kMaxObjects) break;
                    Draw c = d;
                    Mat4 model = inst.model;
                    const Vec3 at{inst.model.at(3, 0), inst.model.at(3, 1), inst.model.at(3, 2)};
                    if (fade > 0) {
                        const float far = length(at - im.camera.position) / fade;
                        if (far >= 1.0f) continue;
                        if (far > 0.8f) model = model * Mat4::scale(Vec3{1, 1, 1} * ((1.0f - far) / 0.2f));   // shrinking about its foot
                    }
                    to_array(model, c.object.model);
                    to_array(transpose(model.inverse_affine()), c.object.normal);
                    for (int k = 0; k < 3; ++k) c.object.color[k] *= inst.shade;
                    if (d.radius >= 0) {
                        c.center = model.transform_point(local_centre);
                        c.radius = d.radius / base_scale * largest_axis(model);
                    }
                    if (sc && sc->sway > 0) {
                        c.object.sway[0] = sc->sway * largest_axis(inst.model) / base_scale;
                        c.object.sway[1] = sc->sway_speed;
                        c.object.sway[2] = local_centre.y - local_radius;
                        c.object.sway[3] = 1.0f / std::max(2.0f * local_radius, 1e-4f);
                    }
                    const Vec3 to_cam = at - im.camera.position;
                    c.depth = to_cam.x * im.camera.forward.x + to_cam.y * im.camera.forward.y + to_cam.z * im.camera.forward.z;
                    made.push_back(std::move(c));
                    ++scattered;
                }
            }
            kept.insert(kept.end(), std::make_move_iterator(made.begin()), std::make_move_iterator(made.end()));
            draws = std::move(kept);
            count = static_cast<std::uint32_t>(draws.size());
        }
    }
    // Levels of detail: every draw with levels (a scattered copy on its own bounds) takes the one
    // for how much of the view its bounds cover, r * P11 / w (w the view depth; 1 orthographic);
    // below its cull_screen it goes, shadow and all.
    {
        const Mat4 vp = im.camera.proj * im.camera.view;
        const float p11 = im.camera.proj.at(1, 1);
        // The view's six planes (clip space: -w <= x, y <= w, 0 <= z <= w), for each draw's bounds.
        auto row = [&](int r) { return Vec4{vp.at(0, r), vp.at(1, r), vp.at(2, r), vp.at(3, r)}; };
        const Vec4 r0 = row(0), r1 = row(1), r2 = row(2), r3 = row(3);
        auto plus = [](Vec4 a, Vec4 b, float k) { return Vec4{a.x + b.x * k, a.y + b.y * k, a.z + b.z * k, a.w + b.w * k}; };
        std::array<Vec4, 6> planes{plus(r3, r0, 1), plus(r3, r0, -1), plus(r3, r1, 1), plus(r3, r1, -1), r2, plus(r3, r2, -1)};
        for (Vec4& pl : planes) {
            const float n = 1.0f / std::max(length(Vec3{pl.x, pl.y, pl.z}), 1e-8f);
            pl = Vec4{pl.x * n, pl.y * n, pl.z * n, pl.w * n};
        }
        auto in_view = [&](const Draw& d) {
            if (d.radius < 0) return true;
            for (const Vec4& pl : planes)
                if (pl.x * d.center.x + pl.y * d.center.y + pl.z * d.center.z + pl.w < -d.radius) return false;
            return true;
        };
        std::size_t kept = 0;
        for (std::size_t i = 0; i < draws.size(); ++i) {
            Draw& d = draws[i];
            if ((d.lods || d.cull > 0) && d.radius > 0) {
                const Vec4 c = vp * Vec4{d.center.x, d.center.y, d.center.z, 1};
                const float screen = d.radius * std::fabs(p11) / std::max(c.w, 1e-4f);
                if (screen < d.cull) {
                    ++im.stats.lod_culled;
                    continue;
                }
                if (d.lods) {
                    const LodLevel* pick = nullptr;
                    for (const LodLevel& lv : *d.lods) if (screen < lv.screen) pick = &lv;
                    if (pick) {
                        d.gpu = pick->gpu;
                        d.first = pick->first;
                        d.count = pick->count;
                        d.mesh = pick->mesh;
                        ++im.stats.lod_simplified;
                    }
                }
            }
            im.stats.triangles += d.count / 3;
            d.in_view = in_view(d);
            if (!d.in_view) ++im.stats.out_of_view;
            if (kept != i) draws[kept] = std::move(d);
            ++kept;
        }
        draws.resize(kept);
    }
    // The camera's passes draw what reaches into its view; the shadow and probe passes cull their own way.
    const std::function<bool(std::size_t)> in_view_only = [&](std::size_t k) { return draws[k].in_view; };
    std::stable_sort(draws.begin(), draws.end(), [](const Draw& a, const Draw& b) {
        if (a.blend != b.blend) return !a.blend;            // opaque first
        if (a.blend) return a.depth > b.depth;              // translucent far to near
        if (a.in_view != b.in_view) return a.in_view;       // what the camera sees together, so its runs stay whole
        if (a.cutout != b.cutout) return !a.cutout;         // cut-outs after the solid meshes they may stand behind
        if (a.skinned != b.skinned) return !a.skinned;
        if (a.user != b.user) return std::less<const void*>()(a.user, b.user);   // a project's materials together
        if (a.texture != b.texture) return a.texture < b.texture;
        if (a.mesh != b.mesh) return a.mesh < b.mesh;
        return a.first < b.first;
    });
    if (im.joint_count > 0) im.device->write_buffer(im.joint_buffer, 0, im.joint_staging.data(), static_cast<std::uint64_t>(im.joint_count) * sizeof(float) * 16);
    // Each object's model last frame, for TAA's motion: found by its entity and the order of its
    // parts; an object new this frame, or any object while TAA is off, moves nowhere.
    {
        std::unordered_map<std::uint64_t, std::array<float, 16>> now;
        std::unordered_map<std::uint32_t, std::uint32_t> parts;
        for (auto& d : draws) {
            ObjectUniforms& o = d.object;
            std::memcpy(o.prev_model, o.model, sizeof o.model);
            if (!im.taa.enabled && !im.motion_blur.enabled) continue;
            const std::uint64_t key = (static_cast<std::uint64_t>(o.id[0]) << 16) | parts[o.id[0]]++;
            if (auto it = im.prev_models.find(key); it != im.prev_models.end()) std::memcpy(o.prev_model, it->second.data(), sizeof o.prev_model);
            std::array<float, 16> m{};
            std::memcpy(m.data(), o.model, sizeof o.model);
            now.emplace(key, m);
        }
        im.prev_models = std::move(now);
    }
    for (std::size_t i = 0; i < draws.size(); ++i) std::memcpy(im.object_staging.data() + i * kObjectStride, &draws[i].object, sizeof(ObjectUniforms));
    // Sprites: unlit quads after every mesh, by layer then far to near, in runs per texture.
    struct SpriteDraw {
        std::string texture;
        WGPUBindGroup material;
        std::int32_t layer;
        float depth;
        ObjectUniforms object;
        const GpuMesh* mesh = nullptr;   // a tile layer mesh instead of the unit quad
        std::uint32_t first = 0, count = 0;
        std::int32_t sub = 0;            // file order of the layer within its map
        bool additive = false;           // adds its light to what is behind (Sprite.additive, ParticleEmitter.additive)
        Impl::SpriteMaterial* user = nullptr;   // a material the project wrote (Sprite.material)
        bool lit = false;                // shaded by the scene's lights (Sprite.lit, TileMap.lit)
        Impl::GpuEmitter* gpu = nullptr; // a GPU emitter's ring instead of one quad
    };
    std::vector<SpriteDraw> sprites;
    std::uint32_t tile_layers = 0, image_layers = 0, image_quads = 0;
    // Tile maps: every visible layer of the map is one mesh (per tileset texture), drawn unlit.
    world.ecs().each([&](flecs::entity e, const world::TileMap& tmc, const world::WorldTransform& t) {
        if (!tmc.visible || tmc.map.empty() || !im.assets) return;
        auto map = im.assets->tilemap(tmc.map);
        if (!map) { im.report_missing(tmc.map, map.error().message); return; }
        Mat4 model = Mat4::trs(t.position, t.rotation, t.scale);
        ObjectUniforms ou{};
        to_array(model, ou.model);
        to_array(transpose(model.inverse_affine()), ou.normal);
        ou.id[0] = static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu);
        ou.id[1] = 1;
        ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
        if (tmc.lit) { ou.pbr[0] = 0; ou.pbr[1] = 1; ou.pbr[2] = 1; ou.pbr[3] = 0; }   // matte, no normal map
        Vec3 d = t.position - im.camera.position;
        float depth = d.x * im.camera.forward.x + d.y * im.camera.forward.y + d.z * im.camera.forward.z;
        std::int32_t index = 0;
        for (const assets::TileLayer& layer : (*map)->layers) {
            const std::int32_t this_index = index++;
            if (!layer.visible || (!tmc.layer.empty() && layer.name != tmc.layer)) continue;
            const Impl::TileLayerMesh* lm = im.tile_layer_mesh(**map, layer, this_index, tmc.tile_size > 0 ? tmc.tile_size : 1.0f, im.assets->tile_time());
            if (!lm) continue;
            ou.color[0] = decode(tmc.color.r); ou.color[1] = decode(tmc.color.g); ou.color[2] = decode(tmc.color.b); ou.color[3] = tmc.color.a * layer.opacity;
            for (const auto& part : lm->parts) {
                if (count + sprites.size() >= kMaxObjects) return;
                sprites.push_back({part.texture, im.texture_for(part.texture, true), tmc.order, depth, ou, &lm->gpu, part.first, part.count, 2 * this_index + 1, false, nullptr, tmc.lit});
            }
            for (const auto& part : lm->anim_parts) {
                if (count + sprites.size() >= kMaxObjects) return;
                sprites.push_back({part.texture, im.texture_for(part.texture, true), tmc.order, depth, ou, &lm->anim, part.first, part.count, 2 * this_index + 1, false, nullptr, tmc.lit});
            }
            ++tile_layers;
        }
        // Image layers: one picture each, placed in map pixels like a tile and repeated across the
        // map's extent when the file asks, drawn among the tile layers in file order (a picture
        // after `before` tile layers sorts between them: tile layers take the odd slots).
        if (tmc.layer.empty()) {
            const float ts = tmc.tile_size > 0 ? tmc.tile_size : 1.0f;
            const bool ortho = (*map)->orthogonal();
            const float sx = ts / static_cast<float>(std::max((*map)->tile_width, 1)), sy = ortho ? ts / static_cast<float>(std::max((*map)->tile_height, 1)) : sx;
            const Vec2 extent = (*map)->pixel_size();
            for (const assets::ImageLayer& il : (*map)->image_layers) {
                if (!il.visible || il.image.empty()) continue;
                auto img = im.assets->image(il.image);
                if (!img) { im.report_missing(il.image, img.error().message); continue; }
                const float iw = static_cast<float>((*img)->width), ih = static_cast<float>((*img)->height);
                if (iw <= 0 || ih <= 0) continue;
                int kx0 = 0, kx1 = 0, ky0 = 0, ky1 = 0;   // copies along each axis, inclusive
                if (il.repeat_x) { kx0 = static_cast<int>(std::floor(-il.offset_x / iw)); kx1 = std::min(static_cast<int>(std::ceil((extent.x - il.offset_x) / iw)) - 1, kx0 + 255); }
                if (il.repeat_y) { ky0 = static_cast<int>(std::floor(-il.offset_y / ih)); ky1 = std::min(static_cast<int>(std::ceil((extent.y - il.offset_y) / ih)) - 1, ky0 + 255); }
                // Parallax: a picture with a factor under 1 keeps part of the camera's motion, so a far
                // sky (0) stays with the camera and a near hill (0.8) drifts slowly. In the map's plane,
                // in world units, from where the camera stands relative to the map's origin.
                const float shift_x = (im.camera.position.x - t.position.x) * (1.0f - il.parallax_x);
                const float shift_y = (im.camera.position.y - t.position.y) * (1.0f - il.parallax_y);
                ObjectUniforms iu = ou;
                iu.id[0] = 0;   // a picture is backdrop: render.pick sees through it, so an empty cell still picks nothing
                iu.color[0] = decode(tmc.color.r * il.tint.x); iu.color[1] = decode(tmc.color.g * il.tint.y); iu.color[2] = decode(tmc.color.b * il.tint.z); iu.color[3] = tmc.color.a * il.tint.w * il.opacity;
                WGPUBindGroup material = im.texture_for(il.image, true);
                bool drawn = false;
                for (int ky = ky0; ky <= ky1; ++ky) {
                    for (int kx = kx0; kx <= kx1; ++kx) {
                        if (count + sprites.size() >= kMaxObjects) return;
                        const float left = (il.offset_x + static_cast<float>(kx) * iw) * sx + shift_x, top = -(il.offset_y + static_cast<float>(ky) * ih) * sy + shift_y;
                        const Mat4 quad = model * Mat4::translation({left + iw * sx * 0.5f, top - ih * sy * 0.5f, 0}) * Mat4::scale({iw * sx, ih * sy, 1});
                        to_array(quad, iu.model);
                        to_array(transpose(quad.inverse_affine()), iu.normal);
                        sprites.push_back({il.image, material, tmc.order, depth, iu, nullptr, 0, 0, 2 * il.before});   // backdrops stay as drawn
                        ++image_quads;
                        drawn = true;
                    }
                }
                if (drawn) ++image_layers;
            }
        }
    });
    world.ecs().each([&](flecs::entity e, const world::Sprite& sp, const world::WorldTransform& t) {
        if (!sp.visible || count + sprites.size() >= kMaxObjects) return;
        if (!im.drawing_target.empty() && sp.texture.starts_with("view:") && sp.texture.substr(5) == im.drawing_target) return;   // not into itself
        Mat4 model = Mat4::trs(t.position, t.rotation, t.scale) * Mat4::translation({(0.5f - sp.anchor.x) * sp.size.x, (0.5f - sp.anchor.y) * sp.size.y, 0}) * Mat4::scale({sp.size.x, sp.size.y, 1});
        ObjectUniforms ou{};
        to_array(model, ou.model);
        to_array(transpose(model.inverse_affine()), ou.normal);
        ou.color[0] = decode(sp.color.r); ou.color[1] = decode(sp.color.g); ou.color[2] = decode(sp.color.b); ou.color[3] = sp.color.a;
        ou.id[0] = static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu);
        ou.id[1] = 1;
        float u0 = sp.uv.x, v0 = sp.uv.y, u1 = sp.uv.z, v1 = sp.uv.w;
        if (sp.flip_x) std::swap(u0, u1);
        if (sp.flip_y) std::swap(v0, v1);
        ou.uv_rect[0] = u0; ou.uv_rect[1] = v0; ou.uv_rect[2] = u1; ou.uv_rect[3] = v1;
        Vec3 d = t.position - im.camera.position;
        float depth = d.x * im.camera.forward.x + d.y * im.camera.forward.y + d.z * im.camera.forward.z;
        // Y-sorted sprites take their Y as the depth: the higher on the screen, the earlier drawn.
        Impl::SpriteMaterial* user = nullptr;
        if (!sp.material.empty()) {
            // Its material's numbers ride in a field sprites do not use.
            if (auto it = im.sprite_materials.find(sp.material); it != im.sprite_materials.end() && it->second.module) user = &it->second;
            ou.optics[0] = sp.params.x; ou.optics[1] = sp.params.y; ou.optics[2] = sp.params.z; ou.optics[3] = sp.params.w;
        }
        WGPUBindGroup material = nullptr;
        const bool lit = sp.lit && !sp.additive && !user;
        if (lit) {
            // Matte, its normal map (when it has one) bent by the texture's frames.
            ou.pbr[0] = 0; ou.pbr[1] = 1; ou.pbr[2] = 1; ou.pbr[3] = sp.normal_map.empty() ? 0.0f : 1.0f;
            material = sp.normal_map.empty() ? im.texture_for(sp.texture, sp.filter == "nearest") : im.material_for(sp.texture, "", sp.normal_map, "", sp.filter == "nearest");
        } else {
            material = im.texture_for(sp.texture, sp.filter == "nearest");
        }
        sprites.push_back({sp.texture, material, sp.layer, sp.sort_y ? t.position.y : depth, ou, nullptr, 0, 0, 0, sp.additive, user, lit});
    });
    // Particles: one unlit quad each, facing the camera (or flat in XY), sized and tinted by age.
    std::uint32_t particle_count = 0;
    if (particles) {
        const Mat4& view = im.camera.view;
        const Vec3 right{view.at(0, 0), view.at(1, 0), view.at(2, 0)};
        const Vec3 up{view.at(0, 1), view.at(1, 1), view.at(2, 1)};
        for (auto& [gid, ge] : im.gpu_emitters) ge.seen = false;
        for (const auto& [id, pool] : particles->pools()) {
            const auto* e = world.try_get<world::ParticleEmitter>(id);
            if (e && e->gpu && pool.gpu) {
                // On the GPU (docs/design/particles.md, On the GPU): the window since the last
                // frame is moved by the compute pass (the first view's), the ring drawn by every view.
                Impl::GpuEmitter& ge = im.gpu_emitters[id];
                ge.seen = true;
                const auto size = static_cast<std::uint32_t>(std::clamp(e->max, 1, 1 << 20));
                const bool deep = im.prepass_view && im.split_applied;
                WGPUTextureView depth = deep ? im.prepass_view : im.no_depth_view;
                if (ge.size != size || !ge.ring || ge.depth != depth) {
                    if (ge.size != size || !ge.ring) {
                        ge.release();
                        ge.size = size;
                        ge.head = 0;
                        ge.ring = im.device->create_buffer("pocket.particles.ring", WGPUBufferUsage_Storage, static_cast<std::uint64_t>(size) * sizeof(float) * 8);
                        ge.params = im.device->create_buffer("pocket.particles.emitter", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(Impl::GpuEmitterParams));
                    }
                    if (ge.sim) wgpuBindGroupRelease(ge.sim);
                    if (ge.draw) wgpuBindGroupRelease(ge.draw);
                    ge.depth = depth;
                    WGPUBindGroupEntry be[3]{};
                    be[0].binding = 0;
                    be[0].buffer = ge.ring;
                    be[0].size = WGPU_WHOLE_SIZE;
                    be[1].binding = 1;
                    be[1].buffer = ge.params;
                    be[1].size = WGPU_WHOLE_SIZE;
                    be[2].binding = 2;
                    be[2].textureView = depth;
                    WGPUBindGroupDescriptor bd{};
                    bd.label = rhi::str("pocket.particles.simulate");
                    bd.layout = im.particle_sim_bgl;
                    bd.entryCount = 3;
                    bd.entries = be;
                    ge.sim = wgpuDeviceCreateBindGroup(im.device->device(), &bd);
                    be[0].binding = 2;
                    be[1].binding = 3;
                    be[2].binding = 4;
                    bd.label = rhi::str("pocket.particles.draw");
                    bd.layout = im.particle_draw_bgl;
                    ge.draw = wgpuDeviceCreateBindGroup(im.device->device(), &bd);
                }
                const auto* wt = world.try_get<world::WorldTransform>(id);
                const Vec3 at = wt ? wt->position : Vec3{0, 0, 0};
                if (!im.secondary) {
                    // The window: the seconds and births the ticks owe, no longer than a particle lives.
                    double dt = std::max(0.0, pool.gpu_seconds - ge.seconds);
                    std::uint64_t born = pool.spawned >= ge.spawned ? pool.spawned - ge.spawned : 0;
                    const double longest = std::max(e->lifetime.x, e->lifetime.y) + 0.1;
                    if (dt > longest) {
                        born = static_cast<std::uint64_t>(std::llround(static_cast<double>(born) * longest / dt));
                        dt = longest;
                    }
                    const std::uint64_t serial = pool.spawned - std::min(pool.spawned, born);
                    born = std::min<std::uint64_t>(born, size);
                    ge.seconds = pool.gpu_seconds;
                    ge.spawned = pool.spawned;
                    Impl::GpuEmitterParams gp{};
                    const Vec3 origin = e->world_space ? at : Vec3{0, 0, 0};
                    const Vec3 start = e->world_space && ge.primed ? ge.last : origin;
                    ge.last = origin;
                    ge.primed = true;
                    const Vec3 axis = (wt ? wt->rotation : Quat{}).rotate(length(e->direction) > 1e-6f ? normalize(e->direction) : Vec3{0, 1, 0});
                    auto put = [](float* d, Vec3 v, float w) { d[0] = v.x; d[1] = v.y; d[2] = v.z; d[3] = w; };
                    put(gp.origin, origin, static_cast<float>(dt));
                    put(gp.prev, start, static_cast<float>(pool.gpu_seconds - dt));
                    put(gp.axis, normalize(axis), std::cos(radians(std::clamp(e->spread, 0.0f, 180.0f))));
                    put(gp.gravity, e->gravity, e->drag);
                    gp.ranges[0] = e->speed.x; gp.ranges[1] = e->speed.y; gp.ranges[2] = e->lifetime.x; gp.ranges[3] = e->lifetime.y;
                    gp.ground[0] = e->world_space ? e->floor : e->floor - at.y;
                    gp.ground[1] = e->bounce; gp.ground[2] = e->floor_friction; gp.ground[3] = e->turbulence;
                    gp.look[0] = e->size.x; gp.look[1] = e->size.y; gp.look[2] = e->stretch; gp.look[3] = 1.0f / std::max(e->turbulence_scale, 1e-3f);
                    gp.color0[0] = decode(e->color.r); gp.color0[1] = decode(e->color.g); gp.color0[2] = decode(e->color.b); gp.color0[3] = e->color.a;
                    gp.color1[0] = decode(e->color_end.r); gp.color1[1] = decode(e->color_end.g); gp.color1[2] = decode(e->color_end.b); gp.color1[3] = e->color_end.a;
                    put(gp.place, e->world_space ? Vec3{0, 0, 0} : at, e->billboard ? 1.0f : 0.0f);
                    gp.counts[0] = ge.head; gp.counts[1] = static_cast<std::uint32_t>(born); gp.counts[2] = size;
                    gp.counts[3] = static_cast<std::uint32_t>(id * 0x9E3779B1u) ^ static_cast<std::uint32_t>(e->seed);
                    gp.extra[0] = static_cast<std::uint32_t>(serial); gp.extra[1] = static_cast<std::uint32_t>(id & 0xFFFFFFFFu);
                    ge.head = static_cast<std::uint32_t>((ge.head + born) % size);
                    // The prepass the compute pass reads is last frame's: its view, its size, whether to collide.
                    to_array(im.particle_vp, gp.depth_vp);
                    to_array(im.particle_inv, gp.depth_inv);
                    gp.depth_info[0] = static_cast<float>(std::max(1u, im.prepass_w));
                    gp.depth_info[1] = static_cast<float>(std::max(1u, im.prepass_h));
                    gp.depth_info[2] = deep && im.particle_view ? 1.0f : 0.0f;
                    gp.depth_info[3] = deep && im.particle_view && e->collide ? 1.0f : 0.0f;
                    put(gp.eye, im.particle_eye, 0.0f);
                    put(gp.area, Vec3{std::max(e->area.x, 0.0f), std::max(e->area.y, 0.0f), std::max(e->area.z, 0.0f)}, 0.0f);
                    const Quat turn = wt ? wt->rotation : Quat{};
                    gp.turn[0] = turn.x; gp.turn[1] = turn.y; gp.turn[2] = turn.z; gp.turn[3] = turn.w;
                    ge.due = dt > 0.0 || born > 0;
                    im.device->write_buffer(ge.params, 0, &gp, sizeof gp);
                }
                const Vec3 d = at - im.camera.position;
                SpriteDraw sd{e->texture, im.texture_for(e->texture.empty() ? std::string(kDotTexture) : e->texture, false), e->layer, d.x * im.camera.forward.x + d.y * im.camera.forward.y + d.z * im.camera.forward.z, ObjectUniforms{}, nullptr, 0, 0, 0, e->additive};
                sd.gpu = &ge;
                sprites.push_back(std::move(sd));
                continue;
            }
            if (!e || pool.alive.empty()) continue;
            const auto* wt = world.try_get<world::WorldTransform>(id);
            const Vec3 origin = (!e->world_space && wt) ? wt->position : Vec3{0, 0, 0};
            WGPUBindGroup material = im.texture_for(e->texture.empty() ? std::string(kDotTexture) : e->texture, false);
            for (const Particle& p : pool.alive) {
                if (count + sprites.size() >= kMaxObjects) break;
                const float k = std::clamp(p.age / p.life, 0.0f, 1.0f);
                const float size = e->size.x + (e->size.y - e->size.x) * k;
                const Vec3 pos = origin + p.position;
                Mat4 model;
                // Stretched along its motion: the quad's long axis follows the velocity as seen
                // (in the camera's plane for a billboard, in XY for a sprite), by stretch seconds of travel.
                const float speed = length(p.velocity);
                const bool streak = e->stretch > 0.0f && speed > 1e-4f;
                if (e->billboard) {
                    Vec3 r = right, u = up;
                    float along = size;
                    if (streak) {
                        const Vec3 f = cross(right, up);
                        Vec3 seen = p.velocity - f * dot(p.velocity, f);   // the motion in the camera's plane
                        if (length(seen) > 1e-4f) {
                            r = normalize(seen);
                            u = cross(f, r);
                            along = size + e->stretch * length(seen);
                        }
                    }
                    const Vec3 rr = r * along, uu = u * size;
                    model.m[0] = rr.x; model.m[1] = rr.y; model.m[2] = rr.z;
                    model.m[4] = uu.x; model.m[5] = uu.y; model.m[6] = uu.z;
                    const Vec3 f = cross(right, up);
                    model.m[8] = f.x; model.m[9] = f.y; model.m[10] = f.z;
                } else if (streak && std::hypot(p.velocity.x, p.velocity.y) > 1e-4f) {
                    const float l = std::hypot(p.velocity.x, p.velocity.y);
                    const float c = p.velocity.x / l, sn = p.velocity.y / l, along = size + e->stretch * l;
                    model.m[0] = c * along; model.m[1] = sn * along;
                    model.m[4] = -sn * size; model.m[5] = c * size;
                } else {
                    model.m[0] = size; model.m[5] = size;
                }
                model.m[12] = pos.x; model.m[13] = pos.y; model.m[14] = pos.z;
                ObjectUniforms ou{};
                to_array(model, ou.model);
                to_array(Mat4::identity(), ou.normal);
                ou.color[0] = decode(e->color.r + (e->color_end.r - e->color.r) * k);
                ou.color[1] = decode(e->color.g + (e->color_end.g - e->color.g) * k);
                ou.color[2] = decode(e->color.b + (e->color_end.b - e->color.b) * k);
                ou.color[3] = e->color.a + (e->color_end.a - e->color.a) * k;
                ou.id[0] = static_cast<std::uint32_t>(id & 0xFFFFFFFFu);
                ou.id[1] = 1;
                ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
                Vec3 d = pos - im.camera.position;
                float depth = d.x * im.camera.forward.x + d.y * im.camera.forward.y + d.z * im.camera.forward.z;
                sprites.push_back({e->texture, material, e->layer, depth, ou, nullptr, 0, 0, 0, e->additive});
                ++particle_count;
            }
        }
    }
    // Trails: a ribbon through each one's points, two vertices a point, across the view (or across
    // XY), its width and colour going from the newest point's to the oldest's.
    if (particles) {
        for (auto& [tid, tm] : im.trail_meshes) tm.seen = false;
        for (const auto& [id, ts] : particles->trails()) {
            const auto* tr = world.try_get<world::Trail>(id);
            if (!tr || ts.points.size() < 2 || count + sprites.size() >= kMaxObjects) continue;
            const std::size_t n = ts.points.size();
            const float life = std::max(tr->time, 1e-4f);
            std::vector<float> along(n, 0.0f);   // distance from the newest point
            for (std::size_t k = n - 1; k-- > 0;) along[k] = along[k + 1] + length(ts.points[k + 1].position - ts.points[k].position);
            const float total = std::max(along[0], 1e-4f);
            auto pack = [](float r, float g, float b, float a) {
                auto enc = [](float v) { return static_cast<std::uint32_t>(std::lround(std::clamp(v, 0.0f, 1.0f) * 255.0f)); };
                return enc(r) | (enc(g) << 8) | (enc(b) << 16) | (enc(a) << 24);
            };
            std::vector<Vertex> verts;
            std::vector<std::uint32_t> idx;
            verts.reserve(n * 2);
            idx.reserve((n - 1) * 6);
            for (std::size_t k = 0; k < n; ++k) {
                const Vec3 p = ts.points[k].position;
                const Vec3 a = ts.points[k == 0 ? 0 : k - 1].position, b = ts.points[k + 1 < n ? k + 1 : n - 1].position;
                Vec3 dir = b - a;
                if (length(dir) < 1e-6f) dir = Vec3{1, 0, 0};
                const Vec3 facing = tr->billboard ? p - im.camera.position : Vec3{0, 0, 1};
                Vec3 side = cross(dir, facing);
                side = length(side) > 1e-6f ? normalize(side) : Vec3{0, 1, 0};
                const float age = std::clamp(ts.points[k].age / life, 0.0f, 1.0f);
                const float w = (tr->width + (tr->width_end - tr->width) * age) * 0.5f;
                const std::uint32_t c = pack(tr->color.r + (tr->color_end.r - tr->color.r) * age, tr->color.g + (tr->color_end.g - tr->color.g) * age,
                                             tr->color.b + (tr->color_end.b - tr->color.b) * age, tr->color.a + (tr->color_end.a - tr->color.a) * age);
                const float u = along[k] / total;
                const Vec3 nrm = normalize(Vec3{0, 0, 0} - (tr->billboard ? facing : Vec3{0, 0, -1}));
                verts.push_back({p + side * w, nrm, {u, 0.0f}, c});
                verts.push_back({p - side * w, nrm, {u, 1.0f}, c});
                if (k + 1 < n) {
                    const auto v = static_cast<std::uint32_t>(k * 2);
                    idx.insert(idx.end(), {v, v + 2, v + 1, v + 1, v + 2, v + 3});
                }
            }
            Impl::TrailMesh& tm = im.trail_meshes[id];
            tm.seen = true;
            if (verts.size() > tm.vertex_room || idx.size() > tm.index_room) {
                if (tm.gpu.vertices) wgpuBufferRelease(tm.gpu.vertices);
                if (tm.gpu.indices) wgpuBufferRelease(tm.gpu.indices);
                tm.vertex_room = static_cast<std::uint32_t>(std::max<std::size_t>(verts.size() * 2, 64));
                tm.index_room = static_cast<std::uint32_t>(std::max<std::size_t>(idx.size() * 2, 96));
                tm.gpu.vertices = im.device->create_buffer("pocket.trail", WGPUBufferUsage_Vertex | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(tm.vertex_room) * sizeof(Vertex));
                tm.gpu.indices = im.device->create_buffer("pocket.trail", WGPUBufferUsage_Index | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(tm.index_room) * sizeof(std::uint32_t));
            }
            im.device->write_buffer(tm.gpu.vertices, 0, verts.data(), verts.size() * sizeof(Vertex));
            im.device->write_buffer(tm.gpu.indices, 0, idx.data(), idx.size() * sizeof(std::uint32_t));
            tm.gpu.index_count = static_cast<std::uint32_t>(idx.size());
            ObjectUniforms ou{};
            to_array(Mat4::identity(), ou.model);
            to_array(Mat4::identity(), ou.normal);
            ou.color[0] = ou.color[1] = ou.color[2] = ou.color[3] = 1.0f;
            ou.id[0] = static_cast<std::uint32_t>(id & 0xFFFFFFFFu);
            ou.id[1] = 1;
            ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
            const Vec3 d = ts.points.back().position - im.camera.position;
            SpriteDraw sd{tr->texture, im.texture_for(tr->texture, false), tr->layer,
                          d.x * im.camera.forward.x + d.y * im.camera.forward.y + d.z * im.camera.forward.z, ou, &tm.gpu, 0, tm.gpu.index_count, 0, tr->additive};
            sprites.push_back(std::move(sd));
        }
        for (auto it = im.trail_meshes.begin(); it != im.trail_meshes.end();) {
            if (it->second.seen) { ++it; continue; }
            if (it->second.gpu.vertices) wgpuBufferRelease(it->second.gpu.vertices);
            if (it->second.gpu.indices) wgpuBufferRelease(it->second.gpu.indices);
            it = im.trail_meshes.erase(it);
        }
    }
    std::stable_sort(sprites.begin(), sprites.end(), [](const SpriteDraw& a, const SpriteDraw& b) {
        if (a.layer != b.layer) return a.layer < b.layer;
        if (a.depth != b.depth) return a.depth > b.depth;
        if (a.sub != b.sub) return a.sub < b.sub;
        return a.texture < b.texture;
    });
    for (std::size_t i = 0; i < sprites.size(); ++i) {
        std::memcpy(sprites[i].object.prev_model, sprites[i].object.model, sizeof sprites[i].object.model);   // sprites move with the camera only
        std::memcpy(im.object_staging.data() + (static_cast<std::size_t>(count) + i) * kObjectStride, &sprites[i].object, sizeof(ObjectUniforms));
    }
    std::uint32_t total = count + static_cast<std::uint32_t>(sprites.size());
    if (total > 0) im.device->write_buffer(im.object_buffer, 0, im.object_staging.data(), static_cast<std::uint64_t>(total) * kObjectStride);
    im.stats.meshes = entities;
    im.stats.instances = total;
    im.stats.scattered = scattered;
    im.stats.sprites = static_cast<std::uint32_t>(sprites.size()) - particle_count - tile_layers_parts(sprites) - image_quads;
    im.stats.particles = particle_count;
    im.stats.tile_layers = tile_layers;
    im.stats.image_layers = image_layers;
    im.stats.translucent = translucent_instances;
    im.glass_last = glass_instances > 0;
    im.stats.tile_rebuilds = im.tile_rebuilds;
    im.stats.tile_frames = im.tile_frames;
    im.stats.msaa = im.msaa_applied;
    im.stats.bloom = false;
    im.stats.grade = false;
    im.stats.tonemap = 0;
    im.stats.auto_exposure = false;
    im.stats.env_updates = im.env_updates;
    im.stats.depth_prepass = prepass;
    im.stats.ao = false;
    im.stats.fog = fog_on;
    im.stats.skinned = skinned_instances;
    im.stats.morphed = morphed_instances;
    im.stats.moving_parts = moving_parts;
    im.stats.asset_meshes = static_cast<std::uint32_t>(im.asset_meshes.size());
    im.stats.textures = static_cast<std::uint32_t>(im.textures.size());
    im.stats.materials = static_cast<std::uint32_t>(im.material_groups.size());

    const bool split = im.split_applied;   // ids (and depth) in a pass of their own; with MSAA the color resolves from the multisampled target
    const bool resolve = im.msaa_applied > 1;
    WGPURenderPassColorAttachment ca[2]{};
    ca[0].view = resolve ? im.ms_color_view : im.hdr_view;
    ca[0].resolveTarget = resolve ? im.hdr_view : nullptr;
    ca[0].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca[0].loadOp = WGPULoadOp_Clear;
    ca[0].storeOp = resolve ? WGPUStoreOp_Discard : WGPUStoreOp_Store;
    ca[0].clearValue = {decode(clear.r), decode(clear.g), decode(clear.b), clear.a};
    ca[1].view = im.id_view;
    ca[1].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca[1].loadOp = WGPULoadOp_Clear;
    ca[1].storeOp = WGPUStoreOp_Store;
    ca[1].clearValue = {0, 0, 0, 0};
    WGPURenderPassDepthStencilAttachment ds{};
    ds.view = resolve ? im.ms_depth_view : frame.depth;
    ds.depthLoadOp = WGPULoadOp_Clear;
    ds.depthStoreOp = WGPUStoreOp_Store;
    ds.depthClearValue = 1.0f;
    ds.stencilLoadOp = WGPULoadOp_Undefined;
    ds.stencilStoreOp = WGPUStoreOp_Undefined;
    ds.stencilReadOnly = true;
    // Instanced runs of equal mesh, submesh and material; the same loop serves both passes.
    // The blend pipelines are given for the color pass only: the shadow and id passes draw a
    // translucent mesh like any other (it casts a shadow and is picked).
    // `cut` and `cut_skinned`: the pipelines a cut-out draws through, the only ones with a discard
    // (a solid mesh keeps the GPU's early depth test). The shadow passes have no material group:
    // their unskinned cut-outs bind theirs at group 3, so the texture's holes let the light through.
    // `keep` (a light's shadow faces): draws it turns down are skipped, splitting their runs.
    auto draw_runs = [&](WGPURenderPassEncoder pass, bool with_materials, std::uint32_t& counter, WGPURenderPipeline plain, WGPURenderPipeline skinned, WGPURenderPipeline blend_plain = nullptr, WGPURenderPipeline blend_skinned = nullptr, WGPURenderPipeline cut = nullptr, const std::function<bool(std::size_t)>* keep = nullptr, std::uint32_t* instances = nullptr, WGPURenderPipeline cut_skinned = nullptr, bool user_materials = false) {
        Impl::SpriteMaterial* current_user = nullptr;   // a project's material on the lit pass's opaque draws
        const GpuMesh* current_mesh = nullptr;
        WGPUBindGroup current_material = nullptr;
        bool current_skinned = false, current_blend = false, current_cut = false;
        wgpuRenderPassEncoderSetPipeline(pass, plain);
        std::size_t i = 0;
        while (i < draws.size()) {
            if (keep && !(*keep)(i)) { ++i; continue; }
            const Draw& d = draws[i];
            const bool blend = d.blend && blend_plain != nullptr;
            const bool cutting = d.cutout && !blend && (d.skinned ? cut_skinned : cut) != nullptr;
            std::size_t run = 1;
            while (i + run < draws.size()) {
                if (keep && !(*keep)(i + run)) break;
                const Draw& n = draws[i + run];
                if (n.gpu != d.gpu || n.first != d.first || n.count != d.count || n.material != d.material || n.skinned != d.skinned || n.blend != d.blend || n.cutout != d.cutout) break;
                if (user_materials && n.user != d.user) break;
                if (blend && n.depth != d.depth) break;   // translucent instances keep their far-to-near order
                ++run;
            }
            Impl::SpriteMaterial* user = user_materials && !blend && !cutting && !d.skinned ? d.user : nullptr;
            WGPURenderPipeline user_pipe = user ? im.mesh_material_pipeline(*user) : nullptr;
            if (!user_pipe) user = nullptr;
            if (d.skinned != current_skinned || blend != current_blend || cutting != current_cut || user != current_user) {
                wgpuRenderPassEncoderSetPipeline(pass, user_pipe ? user_pipe : blend ? (d.skinned ? blend_skinned : blend_plain) : cutting ? (d.skinned ? cut_skinned : cut) : (d.skinned ? skinned : plain));
                current_user = user;
                current_skinned = d.skinned;
                current_blend = blend;
                current_cut = cutting;
                current_mesh = nullptr;
            }
            if (cutting && !with_materials) wgpuRenderPassEncoderSetBindGroup(pass, 3, d.material, 0, nullptr);
            if (d.gpu != current_mesh) {
                wgpuRenderPassEncoderSetVertexBuffer(pass, 0, d.gpu->vertices, 0, WGPU_WHOLE_SIZE);
                if (d.skinned) wgpuRenderPassEncoderSetVertexBuffer(pass, 1, d.gpu->skin, 0, WGPU_WHOLE_SIZE);
                wgpuRenderPassEncoderSetIndexBuffer(pass, d.gpu->indices, WGPUIndexFormat_Uint32, 0, WGPU_WHOLE_SIZE);
                current_mesh = d.gpu;
            }
            if (with_materials && d.material != current_material) {
                wgpuRenderPassEncoderSetBindGroup(pass, 2, d.material, 0, nullptr);
                current_material = d.material;
            }
            wgpuRenderPassEncoderDrawIndexed(pass, d.count, static_cast<std::uint32_t>(run), d.first, 0, static_cast<std::uint32_t>(i));
            counter++;
            if (instances) *instances += static_cast<std::uint32_t>(run);
            i += run;
        }
    };
    if (shadows_on && !draws.empty()) {
        for (int c = 0; c < im.stats.shadow_cascades; ++c) {
            WGPURenderPassDepthStencilAttachment sds{};
            sds.view = im.cascade_view[c];
            sds.depthLoadOp = WGPULoadOp_Clear;
            sds.depthStoreOp = WGPUStoreOp_Store;
            sds.depthClearValue = 1.0f;
            sds.stencilLoadOp = WGPULoadOp_Undefined;
            sds.stencilStoreOp = WGPUStoreOp_Undefined;
            sds.stencilReadOnly = true;
            WGPURenderPassDescriptor srp{};
            srp.label = rhi::str("pocket.shadow");
            srp.colorAttachmentCount = 0;
            srp.depthStencilAttachment = &sds;
            WGPURenderPassEncoder spass = im.begin_pass(frame.encoder, srp);
            wgpuRenderPassEncoderSetBindGroup(spass, 0, im.frame_bg, 0, nullptr);
            wgpuRenderPassEncoderSetBindGroup(spass, 1, im.object_bg, 0, nullptr);
            const std::uint32_t offset = 256u * static_cast<std::uint32_t>(c);
            wgpuRenderPassEncoderSetBindGroup(spass, 2, im.cascade_bg, 1, &offset);
            // Only the casters over this cascade's square (its light-space box across, with their
            // own reach), and none that cast no shadow: a far cascade does not draw the grass at
            // the camera's feet a fourth time.
            Mat4 cvp;
            std::memcpy(cvp.m, fu.cascade_vp[c], sizeof cvp.m);
            const float half_extent = std::max(fu.cascade_texel[c] * static_cast<float>(kShadowMapSize) * 0.5f, 1e-4f);
            const std::function<bool(std::size_t)> over_cascade = [&](std::size_t k) {
                const Draw& d = draws[k];
                if (d.object.id[1] & 2u) return false;
                if (d.radius < 0) return true;
                const Vec4 p = cvp * Vec4{d.center.x, d.center.y, d.center.z, 1};
                const float reach = 1.0f + d.radius / half_extent;
                return std::fabs(p.x) <= reach && std::fabs(p.y) <= reach;
            };
            draw_runs(spass, false, im.stats.shadow_draws, im.shadow_pipeline, im.shadow_skinned_pipeline, nullptr, nullptr, im.shadow_cut_pipeline, &over_cascade, &im.stats.shadow_instances);
            wgpuRenderPassEncoderEnd(spass);
            wgpuRenderPassEncoderRelease(spass);
        }
    }
    // The shelter map (docs/design/rendering.md, Weather): what stands over the square about the
    // camera, from above, as a cascade is drawn; characters left out, so the ground under one walking
    // in the rain does not dry in its shape.
    if (im.shelter_valid) {
        // What stands over the square, by place (to a centimetre) and mesh: drawn again when it changes.
        Mat4 svp;
        std::memcpy(svp.m, im.shelter_vp, sizeof svp.m);
        std::uint64_t sig = 1469598103934665603ull;
        auto mix = [&](std::int64_t v) { sig = (sig ^ static_cast<std::uint64_t>(v)) * 1099511628211ull; };
        for (const Draw& d : draws) {
            if (d.skinned) continue;
            const Vec4 p = svp * Vec4{d.center.x, d.center.y, d.center.z, 1};
            if (d.radius >= 0 && (std::fabs(p.x) > 1.0f + d.radius / kShelterReach || std::fabs(p.y) > 1.0f + d.radius / kShelterReach)) continue;
            mix(std::llround(d.center.x * 100)); mix(std::llround(d.center.y * 100)); mix(std::llround(d.center.z * 100));
            mix(std::llround(d.radius * 100)); mix(d.first); mix(d.count);
        }
        shelter_due = shelter_due || sig != im.shelter_sig;
        im.shelter_sig = sig;
    }
    if (shelter_due) {
        im.device->write_buffer(im.cascade_buffer, 256ull * kShelterLayer, im.shelter_vp, sizeof im.shelter_vp);
        WGPURenderPassDepthStencilAttachment sds{};
        sds.view = im.cascade_view[kShelterLayer];
        sds.depthLoadOp = WGPULoadOp_Clear;
        sds.depthStoreOp = WGPUStoreOp_Store;
        sds.depthClearValue = 1.0f;
        sds.stencilLoadOp = WGPULoadOp_Undefined;
        sds.stencilStoreOp = WGPUStoreOp_Undefined;
        sds.stencilReadOnly = true;
        WGPURenderPassDescriptor srp{};
        srp.label = rhi::str("pocket.shelter");
        srp.colorAttachmentCount = 0;
        srp.depthStencilAttachment = &sds;
        WGPURenderPassEncoder spass = im.begin_pass(frame.encoder, srp);
        if (!draws.empty()) {
            wgpuRenderPassEncoderSetBindGroup(spass, 0, im.frame_bg, 0, nullptr);
            wgpuRenderPassEncoderSetBindGroup(spass, 1, im.object_bg, 0, nullptr);
            const std::uint32_t offset = 256u * kShelterLayer;
            wgpuRenderPassEncoderSetBindGroup(spass, 2, im.cascade_bg, 1, &offset);
            Mat4 svp;
            std::memcpy(svp.m, im.shelter_vp, sizeof svp.m);
            const std::function<bool(std::size_t)> over_square = [&](std::size_t k) {
                const Draw& d = draws[k];
                if (d.skinned) return false;
                if (d.radius < 0) return true;
                const Vec4 p = svp * Vec4{d.center.x, d.center.y, d.center.z, 1};
                const float reach = 1.0f + d.radius / kShelterReach;
                return std::fabs(p.x) <= reach && std::fabs(p.y) <= reach;
            };
            im.shelter_draws = 0;
            draw_runs(spass, false, im.shelter_draws, im.shadow_pipeline, im.shadow_skinned_pipeline, nullptr, nullptr, im.shadow_cut_pipeline, &over_square);
        }
        wgpuRenderPassEncoderEnd(spass);
        wgpuRenderPassEncoderRelease(spass);
    }
    // A reflection probe's capture: the scene drawn six ways from its center with the frame's own
    // shading (the sky, the sun with the camera's cascades, the point and spot lights reaching the
    // box without their shadows, what glows, and the probes' light: this one's from its capture
    // before, none the first time), then turned into its panorama, prefiltered, and its diffuse
    // light taken from it.
    // The point and spot lights whose reach touches a box, as the one cluster a capture's views look
    // up (every pixel of a capture lists them all; no shadows, drawn after it), into `buffer`.
    auto lights_reaching = [&](Vec3 center, Vec3 size, WGPUBuffer buffer) {
        std::vector<std::uint32_t> reach(2 * kClusters, 0u);
        for (std::uint32_t li = 0; li < im.light_packed.size() && reach.size() < 2 * kClusters + kMaxLights; ++li) {
            const GpuLight& g = im.light_packed[li];
            const Vec3 at{g.pos_range[0], g.pos_range[1], g.pos_range[2]};
            const Vec3 half = size * 0.5f;
            const Vec3 d{std::max(std::fabs(at.x - center.x) - half.x, 0.0f), std::max(std::fabs(at.y - center.y) - half.y, 0.0f), std::max(std::fabs(at.z - center.z) - half.z, 0.0f)};
            if (length(d) < g.pos_range[3]) reach.push_back(li);
        }
        const auto count = static_cast<std::uint32_t>(reach.size() - 2 * kClusters);
        reach[0] = 2 * kClusters;
        reach[1] = count;
        im.device->write_buffer(buffer, 0, reach.data(), reach.size() * sizeof(std::uint32_t));
        return count;
    };
    // A capture's six views of the scene from `center` into the probe views, each drawn with the
    // frame's shading as a capture sees it, its uniforms (changed by `tweak`) in its own buffer.
    auto draw_six = [&](const std::array<Mat4, 6>& views, Vec3 center, WGPUBuffer* bufs, WGPUBuffer cluster, std::uint32_t lights, std::vector<WGPUBindGroup>& groups, const std::function<void(FrameUniforms&)>& tweak) {
        const rhi::Color bg{decode(clear.r), decode(clear.g), decode(clear.b), 1.0f};
        for (int f = 0; f < 6; ++f) {
            FrameUniforms pu = fu;
            to_array(views[static_cast<std::size_t>(f)], pu.view_proj);
            to_array(views[static_cast<std::size_t>(f)], pu.cur_view_proj);
            to_array(views[static_cast<std::size_t>(f)], pu.prev_view_proj);
            to_array(views[static_cast<std::size_t>(f)].inverse(), pu.inv_view_proj);
            pu.camera_pos[0] = center.x; pu.camera_pos[1] = center.y; pu.camera_pos[2] = center.z;
            pu.clusters[0] = 1; pu.clusters[1] = 1; pu.clusters[2] = 1;
            pu.clusters[3] = lights;
            pu.probe_info[2] = 1;   // a capture: no local lights' shadows, the sun's by the cascade holding the point
            pu.ao[0] = 0;
            pu.shadow_soft[1] = 0;   // the camera's contact shadows are not the capture's
            pu.taa[0] = 0;
            if (tweak) tweak(pu);
            pu.viewport[0] = 0; pu.viewport[1] = 0; pu.viewport[2] = kProbeFace; pu.viewport[3] = kProbeFace;
            im.device->write_buffer(bufs[f], 0, &pu, sizeof pu);
            WGPUBindGroupEntry pe[19];
            std::memcpy(pe, im.scene_entries, sizeof pe);
            pe[17].textureView = im.glass_stub_view;   // glass in a probe's capture shows nothing through
            pe[0].buffer = bufs[f];
            pe[9].buffer = cluster;
            pe[9].size = sizeof(std::uint32_t) * (2ull * kClusters + kMaxLights);
            WGPUBindGroupDescriptor pd{};
            pd.label = rhi::str("pocket.probe.scene");
            pd.layout = im.scene_bgl;
            pd.entryCount = 19;
            pd.entries = pe;
            WGPUBindGroup group = wgpuDeviceCreateBindGroup(im.device->device(), &pd);
            groups.push_back(group);
            WGPURenderPassColorAttachment pca{};
            pca.view = im.probe_view[f];
            pca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
            pca.loadOp = WGPULoadOp_Clear;
            pca.storeOp = WGPUStoreOp_Store;
            pca.clearValue = {bg.r, bg.g, bg.b, 1.0};
            WGPURenderPassDepthStencilAttachment pds{};
            pds.view = im.probe_depth_face[f];
            pds.depthLoadOp = WGPULoadOp_Clear;
            pds.depthStoreOp = WGPUStoreOp_Store;
            pds.depthClearValue = 1.0f;
            pds.stencilLoadOp = WGPULoadOp_Undefined;
            pds.stencilStoreOp = WGPUStoreOp_Undefined;
            pds.stencilReadOnly = true;
            WGPURenderPassDescriptor prp{};
            prp.label = rhi::str("pocket.probe.view");
            prp.colorAttachmentCount = 1;
            prp.colorAttachments = &pca;
            prp.depthStencilAttachment = &pds;
            WGPURenderPassEncoder ppass = im.begin_pass(frame.encoder, prp);
            if (im.stats.sky != 0) {
                wgpuRenderPassEncoderSetPipeline(ppass, im.probe_sky_pipeline);
                wgpuRenderPassEncoderSetBindGroup(ppass, 0, group, 0, nullptr);
                wgpuRenderPassEncoderDraw(ppass, 3, 1, 0, 0);
            }
            if (!draws.empty()) {
                std::uint32_t probe_draws = 0;
                wgpuRenderPassEncoderSetBindGroup(ppass, 0, group, 0, nullptr);
                wgpuRenderPassEncoderSetBindGroup(ppass, 1, im.object_bg, 0, nullptr);
                draw_runs(ppass, true, probe_draws, im.probe_pipeline, im.probe_skinned_pipeline);
            }
            wgpuRenderPassEncoderEnd(ppass);
            wgpuRenderPassEncoderRelease(ppass);
        }
    };
    if (probe_capture >= 0) {
        Impl::ProbeSlot& slot = im.probe_slots[static_cast<std::size_t>(probe_capture)];
        if (!slot.captured) {
            const std::array<float, 64> none{};
            im.device->write_buffer(im.probe_sh_buffer, 256ull * static_cast<std::uint64_t>(probe_capture), none.data(), sizeof none);
        }
        for (WGPUBindGroup g : im.probe_groups) wgpuBindGroupRelease(g);
        im.probe_groups.clear();
        const std::array<Mat4, 6> views = im.probe_views(slot.center);
        const std::uint32_t probe_lights = lights_reaching(slot.center, slot.size, im.probe_cluster_buffer);
        draw_six(views, slot.center, im.probe_frame_buf, im.probe_cluster_buffer, probe_lights, im.probe_groups, [&](FrameUniforms& pu) {
            if (!slot.captured) {
                // Its own light, dark so far, for what is in its box (reflections only from the captured).
                const auto k = static_cast<std::size_t>(fu.probe_info[0]);
                pu.probe_box[k][0] = slot.center.x; pu.probe_box[k][1] = slot.center.y; pu.probe_box[k][2] = slot.center.z;
                pu.probe_box[k][3] = static_cast<float>(probe_capture);
                pu.probe_ext[k][0] = slot.size.x * 0.5f; pu.probe_ext[k][1] = slot.size.y * 0.5f; pu.probe_ext[k][2] = slot.size.z * 0.5f;
                pu.probe_ext[k][3] = 1;
                pu.probe_info[0] = static_cast<float>(k + 1);
            }
        });
        // The views into level 0 of the probe's layer, then each next level prefiltered from the one above.
        const auto layer = static_cast<std::size_t>(probe_capture);
        std::vector<float> fill(16 * 6 + 8, 0.0f);
        for (int f = 0; f < 6; ++f) to_array(views[static_cast<std::size_t>(f)], fill.data() + 16 * f);
        fill[96] = slot.center.x; fill[97] = slot.center.y; fill[98] = slot.center.z;
        fill[100] = kProbeWidth; fill[101] = kProbeHeight;
        im.device->write_buffer(im.probe_fill_params, 0, fill.data(), fill.size() * sizeof(float));
        std::vector<std::uint8_t> slots(static_cast<std::size_t>(Impl::kSkySlot) * kProbeLevels, 0);
        float prev_alpha = 0;
        for (std::uint32_t l = 0; l < kProbeLevels; ++l) {
            Impl::SkyParams q{};
            q.size[0] = static_cast<float>(std::max(1u, kProbeWidth >> l));
            q.size[1] = static_cast<float>(std::max(1u, kProbeHeight >> l));
            const float rough = static_cast<float>(l) / static_cast<float>(kProbeLevels - 1);
            const float alpha = rough * rough;
            q.misc[2] = std::sqrt(std::max(alpha * alpha - prev_alpha * prev_alpha, 1e-4f));
            prev_alpha = alpha;
            std::memcpy(slots.data() + static_cast<std::size_t>(Impl::kSkySlot) * l, &q, sizeof q);
        }
        im.device->write_buffer(im.probe_prefilter_params, 0, slots.data(), slots.size());
        WGPUBindGroupEntry fe4[4]{};
        fe4[0].binding = 0;
        fe4[0].buffer = im.probe_fill_params;
        fe4[0].size = sizeof(float) * (16 * 6 + 8);
        fe4[1].binding = 1;
        fe4[1].textureView = im.probe_views_array;
        fe4[2].binding = 2;
        fe4[2].sampler = im.env_sampler;
        fe4[3].binding = 3;
        fe4[3].textureView = im.probe_level[layer][0];
        WGPUBindGroupDescriptor fd{};
        fd.label = rhi::str("pocket.probe.fill");
        fd.layout = im.probe_fill_bgl;
        fd.entryCount = 4;
        fd.entries = fe4;
        WGPUBindGroup fill_group = wgpuDeviceCreateBindGroup(im.device->device(), &fd);
        im.probe_groups.push_back(fill_group);
        WGPUComputePassDescriptor cpd{};
        cpd.label = rhi::str("pocket.probe");
        WGPUComputePassEncoder cp = im.begin_compute(frame.encoder, cpd);
        wgpuComputePassEncoderSetPipeline(cp, im.probe_fill_pipeline);
        wgpuComputePassEncoderSetBindGroup(cp, 0, fill_group, 0, nullptr);
        wgpuComputePassEncoderDispatchWorkgroups(cp, (kProbeWidth + 7) / 8, (kProbeHeight + 7) / 8, 1);
        wgpuComputePassEncoderSetPipeline(cp, im.prefilter_pipeline);
        for (std::uint32_t l = 1; l < kProbeLevels; ++l) {
            WGPUBindGroupEntry e[4]{};
            e[0].binding = 0;
            e[0].buffer = im.probe_prefilter_params;
            e[0].offset = static_cast<std::uint64_t>(Impl::kSkySlot) * l;
            e[0].size = sizeof(Impl::SkyParams);
            e[1].binding = 1;
            e[1].textureView = im.probe_level[layer][l - 1];
            e[2].binding = 2;
            e[2].sampler = im.env_sampler;
            e[3].binding = 3;
            e[3].textureView = im.probe_level[layer][l];
            WGPUBindGroupDescriptor d{};
            d.label = rhi::str("pocket.probe.prefilter");
            d.layout = im.env_bgl;
            d.entryCount = 4;
            d.entries = e;
            WGPUBindGroup g = wgpuDeviceCreateBindGroup(im.device->device(), &d);
            im.probe_groups.push_back(g);
            wgpuComputePassEncoderSetBindGroup(cp, 0, g, 0, nullptr);
            wgpuComputePassEncoderDispatchWorkgroups(cp, (std::max(1u, kProbeWidth >> l) + 7) / 8, (std::max(1u, kProbeHeight >> l) + 7) / 8, 1);
        }
        {
            // Its diffuse light: level 3 (32 by 16) projected onto nine harmonics in the probe's slot.
            WGPUBindGroupEntry ie[2]{};
            ie[0].binding = 0;
            ie[0].textureView = im.probe_level[layer][3];
            ie[1].binding = 1;
            ie[1].buffer = im.probe_sh_buffer;
            ie[1].offset = 256ull * layer;
            ie[1].size = sizeof(float) * 36;
            WGPUBindGroupDescriptor id{};
            id.label = rhi::str("pocket.probe.irradiance");
            id.layout = im.irr_bgl;
            id.entryCount = 2;
            id.entries = ie;
            WGPUBindGroup ig = wgpuDeviceCreateBindGroup(im.device->device(), &id);
            im.probe_groups.push_back(ig);
            wgpuComputePassEncoderSetPipeline(cp, im.irradiance_pipeline);
            wgpuComputePassEncoderSetBindGroup(cp, 0, ig, 0, nullptr);
            wgpuComputePassEncoderDispatchWorkgroups(cp, 1, 1, 1);
        }
        wgpuComputePassEncoderEnd(cp);
        wgpuComputePassEncoderRelease(cp);
        slot.captured = true;
        slot.bounces = std::max(slot.bounces - 1, 0);
        slot.frame = im.frame_number;
        im.stats.probe_captures = 1;
    }
    // Irradiance volumes' probes: each drawn six ways as a reflection probe is (lit by the volume as
    // its probes so far give it, so each pass bounces the light once more), and its harmonics taken
    // from the views straight into its slot.
    if (!grid_captures.empty()) {
        for (WGPUBindGroup g : im.grid_groups) wgpuBindGroupRelease(g);
        im.grid_groups.clear();
        const Impl::GridVolume& volume = im.grids[grid_captures.front().volume];
        const std::uint32_t grid_lights = lights_reaching(volume.center, volume.size, im.grid_cluster_buffer);
        for (std::size_t k = 0; k < grid_captures.size(); ++k) {
            Impl::GridVolume& g = im.grids[grid_captures[k].volume];
            const std::uint32_t probe = grid_captures[k].probe;
            const Vec3 at = g.probe_at(probe);
            const std::array<Mat4, 6> views = im.probe_views(at);
            draw_six(views, at, im.grid_frame_buf[k], im.grid_cluster_buffer, grid_lights, im.grid_groups, nullptr);
            std::vector<float> fill(16 * 6 + 8, 0.0f);
            for (int f = 0; f < 6; ++f) to_array(views[static_cast<std::size_t>(f)], fill.data() + 16 * f);
            fill[96] = at.x; fill[97] = at.y; fill[98] = at.z;
            fill[100] = static_cast<float>(16 * (kMaxProbes + g.first + probe));
            fill[101] = static_cast<float>(16 * (kMaxProbes + kMaxGridProbes) + 128 * (g.first + probe));
            // The moments stop at twice the widest gap between probes: farther is in sight.
            const float gap = std::max({g.size.x / static_cast<float>(g.nx - 1), g.size.y / static_cast<float>(g.ny - 1), g.size.z / static_cast<float>(g.nz - 1)});
            fill[102] = 2.0f * gap;
            im.device->write_buffer(im.grid_params[k], 0, fill.data(), fill.size() * sizeof(float));
            WGPUBindGroupEntry ge[5]{};
            ge[0].binding = 0;
            ge[0].buffer = im.grid_params[k];
            ge[0].size = sizeof(float) * (16 * 6 + 8);
            ge[1].binding = 1;
            ge[1].textureView = im.probe_views_array;
            ge[2].binding = 2;
            ge[2].sampler = im.env_sampler;
            ge[3].binding = 3;
            ge[3].buffer = im.probe_sh_buffer;
            ge[3].size = sizeof(float) * (4 * 16 * (kMaxProbes + kMaxGridProbes) + 4 * 128 * kMaxGridProbes);
            ge[4].binding = 4;
            ge[4].textureView = im.probe_depth_array;
            WGPUBindGroupDescriptor gd{};
            gd.label = rhi::str("pocket.grid.harmonics");
            gd.layout = im.grid_bgl;
            gd.entryCount = 5;
            gd.entries = ge;
            WGPUBindGroup gg = wgpuDeviceCreateBindGroup(im.device->device(), &gd);
            im.grid_groups.push_back(gg);
            WGPUComputePassDescriptor gcd{};
            gcd.label = rhi::str("pocket.grid");
            WGPUComputePassEncoder gp = im.begin_compute(frame.encoder, gcd);
            wgpuComputePassEncoderSetPipeline(gp, im.grid_pipeline);
            wgpuComputePassEncoderSetBindGroup(gp, 0, gg, 0, nullptr);
            wgpuComputePassEncoderDispatchWorkgroups(gp, 1, 1, 1);
            wgpuComputePassEncoderEnd(gp);
            wgpuComputePassEncoderRelease(gp);
            if (probe + 1 == g.count()) g.ready = true;
            g.frame = im.frame_number;
        }
        im.stats.grid_captures = static_cast<std::uint32_t>(grid_captures.size());
    }
    // The local lights' shadow faces: one pass over the atlas, each face drawn into its own square.
    if (!im.faces.empty() && im.atlas_view && !draws.empty()) {
        WGPURenderPassDepthStencilAttachment ads{};
        ads.view = im.atlas_view;
        ads.depthLoadOp = WGPULoadOp_Clear;
        ads.depthStoreOp = WGPUStoreOp_Store;
        ads.depthClearValue = 1.0f;
        ads.stencilLoadOp = WGPULoadOp_Undefined;
        ads.stencilStoreOp = WGPUStoreOp_Undefined;
        ads.stencilReadOnly = true;
        WGPURenderPassDescriptor arp{};
        arp.label = rhi::str("pocket.shadow.local");
        arp.colorAttachmentCount = 0;
        arp.depthStencilAttachment = &ads;
        WGPURenderPassEncoder apass = im.begin_pass(frame.encoder, arp);
        wgpuRenderPassEncoderSetBindGroup(apass, 0, im.frame_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(apass, 1, im.object_bg, 0, nullptr);
        for (std::size_t f = 0; f < im.faces.size(); ++f) {
            const auto x = static_cast<std::uint32_t>(f % kFaceTiles) * kFaceSize, y = static_cast<std::uint32_t>(f / kFaceTiles) * kFaceSize;
            wgpuRenderPassEncoderSetViewport(apass, static_cast<float>(x), static_cast<float>(y), kFaceSize, kFaceSize, 0.0f, 1.0f);
            wgpuRenderPassEncoderSetScissorRect(apass, x, y, kFaceSize, kFaceSize);
            const std::uint32_t offset = 256u * static_cast<std::uint32_t>(f);
            wgpuRenderPassEncoderSetBindGroup(apass, 2, im.face_bg, 1, &offset);
            // Only what the light reaches can throw its shadow: the draws whose sphere meets the light's.
            const Vec3 at = im.face_light[f].first;
            const float reach = im.face_light[f].second;
            const std::function<bool(std::size_t)> keep = [&](std::size_t k) {
                const Draw& d = draws[k];
                return d.radius < 0 || length(d.center - at) <= reach + d.radius;
            };
            draw_runs(apass, false, im.stats.shadow_draws, im.atlas_pipeline, im.atlas_skinned_pipeline, nullptr, nullptr, im.atlas_cut_pipeline, &keep, &im.stats.shadow_instances);
        }
        wgpuRenderPassEncoderEnd(apass);
        wgpuRenderPassEncoderRelease(apass);
    }
    // Sprites and tile layers in draw order; the same loop serves the color pass and the id pass.
    // `add`: the pipeline additive sprites draw with (the id pass gives the same one for both).
    // `lit`: the pipeline lit sprites and tile maps draw with (none in the id pass: `pipe` then).
    auto draw_sprites = [&](WGPURenderPassEncoder pass, WGPURenderPipeline pipe, WGPURenderPipeline add, std::uint32_t& counter, bool materials = false, WGPURenderPipeline lit = nullptr) {
        const GpuMesh& quad = im.meshes[static_cast<std::size_t>(Primitive::Quad)];
        wgpuRenderPassEncoderSetPipeline(pass, pipe);
        WGPURenderPipeline current = pipe;
        auto use = [&](WGPURenderPipeline p) {
            if (p != current) { wgpuRenderPassEncoderSetPipeline(pass, p); current = p; }
        };
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        wgpuRenderPassEncoderSetVertexBuffer(pass, 0, quad.vertices, 0, WGPU_WHOLE_SIZE);
        wgpuRenderPassEncoderSetIndexBuffer(pass, quad.indices, WGPUIndexFormat_Uint32, 0, WGPU_WHOLE_SIZE);
        const GpuMesh* bound = &quad;
        std::size_t i = 0;
        while (i < sprites.size()) {
            const SpriteDraw& s = sprites[i];
            if (s.mesh) {
                // A tile layer or a trail: its own buffers, one draw, then back to the quad for sprites.
                use(s.additive ? add : s.lit && lit ? lit : pipe);
                wgpuRenderPassEncoderSetVertexBuffer(pass, 0, s.mesh->vertices, 0, WGPU_WHOLE_SIZE);
                wgpuRenderPassEncoderSetIndexBuffer(pass, s.mesh->indices, WGPUIndexFormat_Uint32, 0, WGPU_WHOLE_SIZE);
                bound = s.mesh;
                wgpuRenderPassEncoderSetBindGroup(pass, 2, s.material, 0, nullptr);
                wgpuRenderPassEncoderDrawIndexed(pass, s.count, 1, s.first, 0, count + static_cast<std::uint32_t>(i));
                counter++;
                ++i;
                continue;
            }
            if (bound != &quad) {
                wgpuRenderPassEncoderSetVertexBuffer(pass, 0, quad.vertices, 0, WGPU_WHOLE_SIZE);
                wgpuRenderPassEncoderSetIndexBuffer(pass, quad.indices, WGPUIndexFormat_Uint32, 0, WGPU_WHOLE_SIZE);
                bound = &quad;
            }
            if (s.gpu) {
                // A GPU emitter's ring (the colour passes only): six vertices a slot.
                if (materials && s.gpu->draw) {
                    use(s.additive ? im.particle_add_pipeline : im.particle_pipeline);
                    wgpuRenderPassEncoderSetBindGroup(pass, 2, s.material, 0, nullptr);
                    wgpuRenderPassEncoderSetBindGroup(pass, 3, s.gpu->draw, 0, nullptr);
                    wgpuRenderPassEncoderDraw(pass, 6, s.gpu->size, 0, 0);
                    counter++;
                }
                ++i;
                continue;
            }
            Impl::SpriteMaterial* user = materials ? s.user : nullptr;
            WGPURenderPipeline chosen = user ? im.sprite_material_pipeline(*user, s.additive) : nullptr;
            if (!chosen) chosen = s.additive ? add : s.lit && lit ? lit : pipe;
            use(chosen);
            std::size_t run = 1;
            while (i + run < sprites.size() && !sprites[i + run].mesh && !sprites[i + run].gpu && sprites[i + run].material == sprites[i].material && sprites[i + run].additive == s.additive && sprites[i + run].lit == s.lit && (!materials || sprites[i + run].user == s.user)) ++run;
            wgpuRenderPassEncoderSetBindGroup(pass, 2, sprites[i].material, 0, nullptr);
            wgpuRenderPassEncoderDrawIndexed(pass, quad.index_count, static_cast<std::uint32_t>(run), 0, 0, count + static_cast<std::uint32_t>(i));
            counter++;
            i += run;
        }
    };
    auto set_viewport = [&](WGPURenderPassEncoder pass) {
        if (im.applied.w != frame.width || im.applied.h != frame.height) {
            wgpuRenderPassEncoderSetViewport(pass, static_cast<float>(im.applied.x), static_cast<float>(im.applied.y), static_cast<float>(im.applied.w), static_cast<float>(im.applied.h), 0.0f, 1.0f);
            wgpuRenderPassEncoderSetScissorRect(pass, static_cast<std::uint32_t>(im.applied.x), static_cast<std::uint32_t>(im.applied.y), im.applied.w, im.applied.h);
        }
    };
    // GPU particles move before anything draws them (docs/design/particles.md, On the GPU).
    if (!im.secondary) {
        bool any = false;
        for (auto& [gid, ge] : im.gpu_emitters) any = any || (ge.seen && ge.due);
        if (any) {
            WGPUComputePassDescriptor cpd{};
            cpd.label = rhi::str("pocket.particles");
            WGPUComputePassEncoder cp = im.begin_compute(frame.encoder, cpd);
            wgpuComputePassEncoderSetPipeline(cp, im.particle_sim_pipeline);
            for (auto& [gid, ge] : im.gpu_emitters) {
                if (!ge.seen || !ge.due) continue;
                wgpuComputePassEncoderSetBindGroup(cp, 0, ge.sim, 0, nullptr);
                wgpuComputePassEncoderDispatchWorkgroups(cp, (ge.size + 63) / 64, 1, 1);
                ge.due = false;
            }
            wgpuComputePassEncoderEnd(cp);
            wgpuComputePassEncoderRelease(cp);
        }
        for (auto it = im.gpu_emitters.begin(); it != im.gpu_emitters.end();) {
            if (it->second.seen) { ++it; continue; }
            it->second.release();
            it = im.gpu_emitters.erase(it);
        }
        // This frame's view draws the prepass the next frame's particles collide with.
        im.particle_vp = im.camera.proj * im.camera.view;
        im.particle_inv = im.particle_vp.inverse();
        im.particle_eye = im.camera.position;
        im.particle_view = true;
    }
    if (split) {
        // The id pass first: single-sample ids and the frame's own depth buffer, no lines.
        WGPURenderPassColorAttachment ica{};
        ica.view = im.id_view;
        ica.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ica.loadOp = WGPULoadOp_Clear;
        ica.storeOp = WGPUStoreOp_Store;
        ica.clearValue = {0, 0, 0, 0};
        WGPURenderPassDepthStencilAttachment ids = ds;
        ids.view = im.prepass_view;
        WGPURenderPassColorAttachment icas[2] = {ica, ica};
        icas[1].view = im.velocity_view;
        WGPURenderPassColorAttachment surface_ca = ica, albedo_ca = ica;
        surface_ca.view = im.surface_view;
        surface_ca.clearValue = {0, 0, 1, 0};   // rough: nothing traced where nothing was drawn
        albedo_ca.view = im.albedo_view;
        WGPURenderPassColorAttachment icas4[4] = {icas[0], icas[1], surface_ca, albedo_ca};
        WGPURenderPassDescriptor irp{};
        irp.label = rhi::str("pocket.ids");
        irp.colorAttachmentCount = 4;
        irp.colorAttachments = icas4;
        irp.depthStencilAttachment = &ids;
        WGPURenderPassEncoder ipass = im.begin_pass(frame.encoder, irp);
        set_viewport(ipass);
        if (!draws.empty()) {
            wgpuRenderPassEncoderSetBindGroup(ipass, 0, im.scene_bg, 0, nullptr);
            wgpuRenderPassEncoderSetBindGroup(ipass, 1, im.object_bg, 0, nullptr);
            draw_runs(ipass, true, im.stats.id_draws, im.id_pipeline, im.id_skinned_pipeline, nullptr, nullptr, im.id_cut_pipeline, &in_view_only, nullptr, im.id_cut_skinned_pipeline);
        }
        if (!sprites.empty()) draw_sprites(ipass, im.id_sprite_pipeline, im.id_sprite_pipeline, im.stats.id_draws);
        wgpuRenderPassEncoderEnd(ipass);
        wgpuRenderPassEncoderRelease(ipass);
        if (ao_pass) im.draw_ao(frame);
    }
    // Order-independent transparency: the translucent meshes leave this pass for their own, and
    // what is drawn after them (sprites, lines) waits for a pass after the composite. With MSAA
    // this first part keeps its samples, unresolved, for the composite to be laid over.
    const bool oit = im.oit && translucent_instances > 0;
    // Glass (docs/design/rendering.md, Glass) waits for a pass of its own after the solid meshes,
    // which reads a copy of what they drew; the translucent meshes come after it.
    // (A frame that meets an asset's glass first has no copy to read yet: it draws it as solid.)
    const bool glassy = split && glass_instances > 0 && im.glass_tex && im.glass_w == frame.width && im.glass_h == frame.height;
    WGPURenderPassColorAttachment first[2] = {ca[0], ca[1]};
    if (oit && resolve) {
        first[0].resolveTarget = nullptr;
        first[0].storeOp = WGPUStoreOp_Store;
    }
    WGPURenderPassColorAttachment after_glass = first[0];   // the glass pass goes on as the scene pass would have
    after_glass.loadOp = WGPULoadOp_Load;
    if (glassy) {
        first[0].storeOp = WGPUStoreOp_Store;              // the samples, for the glass pass to go on with
        if (resolve) first[0].resolveTarget = im.hdr_view; // and the resolved colour, for the copy
    }
    WGPURenderPassDescriptor rp{};
    rp.label = rhi::str("pocket.scene");
    rp.colorAttachmentCount = split ? 1 : 2;
    rp.colorAttachments = first;
    rp.depthStencilAttachment = &ds;
    WGPURenderPassEncoder pass = im.begin_pass(frame.encoder, rp);
    set_viewport(pass);
    if (im.stats.sky != 0) {
        wgpuRenderPassEncoderSetPipeline(pass, im.sky_pipeline);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderDraw(pass, 3, 1, 0, 0);
        im.stats.draw_calls++;
    }
    const std::function<bool(std::size_t)> solid_only = [&](std::size_t k) { return draws[k].in_view && !(glassy && draws[k].glass) && !(draws[k].blend && (oit || glassy)); };
    const std::function<bool(std::size_t)> translucent_only = [&](std::size_t k) { return draws[k].in_view && draws[k].blend; };
    const std::function<bool(std::size_t)> glass_only = [&](std::size_t k) { return draws[k].in_view && draws[k].glass; };
    if (!draws.empty()) {
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        draw_runs(pass, true, im.stats.draw_calls, im.pipeline, im.skinned_pipeline, im.blend_pipeline, im.blend_skinned_pipeline, im.cut_pipeline, (oit || glassy) ? &solid_only : &in_view_only, nullptr, im.cut_skinned_pipeline, true);
    }
    if (glassy) {
        // The scene so far copied for the glass to show through, then the glass over it, and the
        // translucent meshes after (unless they have a pass of their own).
        wgpuRenderPassEncoderEnd(pass);
        wgpuRenderPassEncoderRelease(pass);
        WGPUTexelCopyTextureInfo csrc{}, cdst{};
        csrc.texture = im.hdr_tex;
        csrc.aspect = WGPUTextureAspect_All;
        cdst.texture = im.glass_tex;
        cdst.aspect = WGPUTextureAspect_All;
        const WGPUExtent3D cext{frame.width, frame.height, 1};
        wgpuCommandEncoderCopyTextureToTexture(frame.encoder, &csrc, &cdst, &cext);
        WGPURenderPassDepthStencilAttachment gds = ds;
        gds.depthLoadOp = WGPULoadOp_Load;
        WGPURenderPassDescriptor grp = rp;
        grp.label = rhi::str("pocket.glass");
        grp.colorAttachmentCount = 1;
        grp.colorAttachments = &after_glass;
        grp.depthStencilAttachment = &gds;
        pass = im.begin_pass(frame.encoder, grp);
        set_viewport(pass);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        draw_runs(pass, true, im.stats.draw_calls, im.pipeline, im.skinned_pipeline, nullptr, nullptr, im.cut_pipeline, &glass_only, nullptr, im.cut_skinned_pipeline);
        if (!oit && translucent_instances > 0) draw_runs(pass, true, im.stats.draw_calls, im.pipeline, im.skinned_pipeline, im.blend_pipeline, im.blend_skinned_pipeline, im.cut_pipeline, &translucent_only, nullptr, im.cut_skinned_pipeline);
        im.stats.glass = glass_instances;
    }
    if (oit) {
        wgpuRenderPassEncoderEnd(pass);
        wgpuRenderPassEncoderRelease(pass);
        // With MSAA the accumulation is multisampled against the scene's multisampled depth and
        // resolved (averaged) into the targets the composite reads.
        POCKET_TRY_VOID(im.ensure_oit_targets(frame.width, frame.height, resolve ? im.msaa_applied : 1));
        WGPURenderPassColorAttachment oca[2]{};
        oca[0].view = resolve ? im.oit_ms_accum_view : im.oit_accum_view;
        oca[0].resolveTarget = resolve ? im.oit_accum_view : nullptr;
        oca[0].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        oca[0].loadOp = WGPULoadOp_Clear;
        oca[0].storeOp = resolve ? WGPUStoreOp_Discard : WGPUStoreOp_Store;
        oca[0].clearValue = {0, 0, 0, 0};
        oca[1] = oca[0];
        oca[1].view = resolve ? im.oit_ms_reveal_view : im.oit_reveal_view;
        oca[1].resolveTarget = resolve ? im.oit_reveal_view : nullptr;
        oca[1].clearValue = {1, 1, 1, 1};
        WGPURenderPassDepthStencilAttachment ods = ds;
        ods.depthLoadOp = WGPULoadOp_Load;
        WGPURenderPassDescriptor orp{};
        orp.label = rhi::str("pocket.oit");
        orp.colorAttachmentCount = 2;
        orp.colorAttachments = oca;
        orp.depthStencilAttachment = &ods;
        WGPURenderPassEncoder opass = im.begin_pass(frame.encoder, orp);
        set_viewport(opass);
        wgpuRenderPassEncoderSetBindGroup(opass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(opass, 1, im.object_bg, 0, nullptr);
        draw_runs(opass, true, im.stats.draw_calls, resolve ? im.oit_ms_pipeline : im.oit_pipeline, resolve ? im.oit_ms_skinned_pipeline : im.oit_skinned_pipeline, nullptr, nullptr, nullptr, &translucent_only);
        wgpuRenderPassEncoderEnd(opass);
        wgpuRenderPassEncoderRelease(opass);
        WGPURenderPassColorAttachment cca{};
        cca.view = resolve ? im.ms_color_view : im.hdr_view;   // every sample, resolved with the sprites and lines
        cca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        cca.loadOp = WGPULoadOp_Load;
        cca.storeOp = WGPUStoreOp_Store;
        WGPURenderPassDescriptor crp{};
        crp.label = rhi::str("pocket.oit.composite");
        crp.colorAttachmentCount = 1;
        crp.colorAttachments = &cca;
        WGPURenderPassEncoder cpass = im.begin_pass(frame.encoder, crp);
        wgpuRenderPassEncoderSetPipeline(cpass, resolve ? im.oit_ms_composite_pipeline : im.oit_composite_pipeline);
        wgpuRenderPassEncoderSetBindGroup(cpass, 0, im.oit_bg, 0, nullptr);
        wgpuRenderPassEncoderDraw(cpass, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(cpass);
        wgpuRenderPassEncoderRelease(cpass);
        im.stats.draw_calls++;
        im.stats.oit = true;
        // The rest of the scene pass: sprites and lines over the composite.
        WGPURenderPassColorAttachment aca[2] = {ca[0], ca[1]};
        aca[0].loadOp = WGPULoadOp_Load;
        aca[1].loadOp = WGPULoadOp_Load;
        WGPURenderPassDepthStencilAttachment ads2 = ds;
        ads2.depthLoadOp = WGPULoadOp_Load;
        WGPURenderPassDescriptor arp2 = rp;
        arp2.label = rhi::str("pocket.scene.after");
        arp2.colorAttachments = aca;
        arp2.depthStencilAttachment = &ads2;
        pass = im.begin_pass(frame.encoder, arp2);
        set_viewport(pass);
    }
    // Water, after what is solid (and translucent); without MSAA before the sprites and lines,
    // which a pass after it draws, with MSAA after them (docs/design/water.md).
    if (!im.water_bodies.empty() && split && !resolve) {
        wgpuRenderPassEncoderEnd(pass);
        wgpuRenderPassEncoderRelease(pass);
        POCKET_TRY_VOID(im.draw_water(frame, frame.depth, false, set_viewport));
        WGPURenderPassColorAttachment wca[2] = {ca[0], ca[1]};
        wca[0].loadOp = WGPULoadOp_Load;
        wca[1].loadOp = WGPULoadOp_Load;
        WGPURenderPassDepthStencilAttachment wds = ds;
        wds.depthLoadOp = WGPULoadOp_Load;
        WGPURenderPassDescriptor wrp = rp;
        wrp.label = rhi::str("pocket.scene.over_water");
        wrp.colorAttachments = wca;
        wrp.depthStencilAttachment = &wds;
        pass = im.begin_pass(frame.encoder, wrp);
        set_viewport(pass);
    }
    if (!sprites.empty()) draw_sprites(pass, im.sprite_pipeline, im.sprite_add_pipeline, im.stats.draw_calls, true, im.sprite_lit_pipeline);
    if (im.weather_drops > 0 && im.weather_pipeline) {
        wgpuRenderPassEncoderSetPipeline(pass, im.weather_pipeline);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 2, im.texture_for(""), 0, nullptr);
        wgpuRenderPassEncoderDraw(pass, 6, im.weather_drops, 0, 0);
        im.stats.draw_calls++;
    }
    if (debug && !debug->vertices().empty()) {
        const auto& verts = debug->vertices();
        if (verts.size() > im.line_capacity) {
            if (im.line_buffer) wgpuBufferRelease(im.line_buffer);
            im.line_capacity = std::max<std::size_t>(verts.size() * 2, 1024);
            im.line_buffer = im.device->create_buffer("pocket.lines", WGPUBufferUsage_Vertex | WGPUBufferUsage_CopyDst, im.line_capacity * sizeof(DebugVertex));
        }
        im.device->write_buffer(im.line_buffer, 0, verts.data(), verts.size() * sizeof(DebugVertex));
        wgpuRenderPassEncoderSetPipeline(pass, im.line_pipeline);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.frame_bg, 0, nullptr);
        wgpuRenderPassEncoderSetVertexBuffer(pass, 0, im.line_buffer, 0, verts.size() * sizeof(DebugVertex));
        wgpuRenderPassEncoderDraw(pass, static_cast<std::uint32_t>(verts.size()), 1, 0, 0);
        im.stats.draw_calls++;
        im.stats.debug_lines = static_cast<std::uint32_t>(verts.size() / 2);
    }
    wgpuRenderPassEncoderEnd(pass);
    wgpuRenderPassEncoderRelease(pass);
    if (!im.water_bodies.empty() && split && resolve) POCKET_TRY_VOID(im.draw_water(frame, im.prepass_view, true, set_viewport));
    if (im.ssgi.enabled && split && im.surface_view && im.albedo_view) POCKET_TRY_VOID(im.draw_gi(frame));
    else im.gi_valid = false;
    if (im.ssr.enabled && split && im.surface_view) POCKET_TRY_VOID(im.draw_ssr(frame));
    if (im.taa.enabled && split && im.velocity_view) POCKET_TRY_VOID(im.resolve_taa(frame, taa_reproject));
    if ((im.dof.enabled || im.motion_blur.enabled) && split && im.velocity_view) {
        Impl::FxUniforms u{};
        to_array(taa_reproject, u.reproject);
        to_array(cur_vp.inverse(), u.inv_view_proj);
        u.camera[0] = im.camera.position.x; u.camera[1] = im.camera.position.y; u.camera[2] = im.camera.position.z;
        u.viewport[0] = static_cast<float>(im.applied.x);
        u.viewport[1] = static_cast<float>(im.applied.y);
        u.viewport[2] = static_cast<float>(std::max(1u, im.applied.w));
        u.viewport[3] = static_cast<float>(std::max(1u, im.applied.h));
        u.dof[0] = im.dof.focus;
        u.dof[1] = im.dof.aperture;
        u.dof[2] = im.dof.max_blur;
        u.blur[0] = im.motion_blur.strength;
        u.blur[1] = static_cast<float>(im.motion_blur.samples);
        if (im.dof.enabled) {
            POCKET_TRY_VOID(im.apply_fx(frame, im.dof_pipeline, "pocket.dof", u));
            im.stats.dof = true;
        }
        if (im.motion_blur.enabled) {
            POCKET_TRY_VOID(im.apply_fx(frame, im.blur_pipeline, "pocket.motion_blur", u));
            im.stats.motion_blur = true;
        }
    }
    im.stats.volumetric = fog_on && fog.volumetric && im.prepass_view && im.split_applied;
    if (im.stats.volumetric) POCKET_TRY_VOID(im.draw_volume(frame, fog));
    else im.volume_valid = false;   // a history from before a gap would show what is no longer there
    if (im.bloom.enabled && im.bloom.strength > 0) POCKET_TRY_VOID(im.draw_bloom(frame));
    POCKET_TRY_VOID(im.draw_post(frame, clear, fog_on ? &fog : nullptr));
    im.timer_end_frame(frame.encoder);
    im.stats.gpu_timing = im.timer_set != nullptr;
    im.stats.gpu_passes = im.timer_last;
    im.stats.gpu_ms = im.timer_last_ms;
    im.stats.gpu_age = im.timer_last_frame > 0 ? im.device->submitted_frames() - im.timer_last_frame : 0;
    im.stats.gpu_frames = im.timer_reads_done;
    return {};
}

Result<IdImage> Renderer::read_ids() {
    Impl& im = *impl_;
    if (!im.id_texture) return fail("no_frame", "nothing rendered yet");
    const std::uint32_t bpr_unpadded = im.id_width * 4;
    const std::uint32_t bpr = (bpr_unpadded + 255) / 256 * 256;
    WGPUBufferDescriptor bd{};
    bd.label = rhi::str("pocket.ids.readback");
    bd.usage = WGPUBufferUsage_MapRead | WGPUBufferUsage_CopyDst;
    bd.size = static_cast<std::uint64_t>(bpr) * im.id_height;
    WGPUBuffer readback = wgpuDeviceCreateBuffer(im.device->device(), &bd);
    if (!readback) return fail("gpu_buffer_failed", "cannot create id readback buffer");
    WGPUCommandEncoder enc = wgpuDeviceCreateCommandEncoder(im.device->device(), nullptr);
    WGPUTexelCopyTextureInfo src{};
    src.texture = im.id_texture;
    src.aspect = WGPUTextureAspect_All;
    WGPUTexelCopyBufferInfo dst{};
    dst.layout.bytesPerRow = bpr;
    dst.layout.rowsPerImage = im.id_height;
    dst.buffer = readback;
    WGPUExtent3D ext{im.id_width, im.id_height, 1};
    wgpuCommandEncoderCopyTextureToBuffer(enc, &src, &dst, &ext);
    WGPUCommandBuffer cb = wgpuCommandEncoderFinish(enc, nullptr);
    wgpuQueueSubmit(im.device->queue(), 1, &cb);
    wgpuCommandBufferRelease(cb);
    wgpuCommandEncoderRelease(enc);
    struct MapResult { bool done = false; WGPUMapAsyncStatus status{}; std::string msg; } mr;
    WGPUBufferMapCallbackInfo mci{};
    mci.mode = WGPUCallbackMode_AllowSpontaneous;
    mci.callback = [](WGPUMapAsyncStatus status, WGPUStringView message, void* u1, void*) {
        auto* r = static_cast<MapResult*>(u1);
        r->done = true;
        r->status = status;
        r->msg = rhi::to_string(message);
    };
    mci.userdata1 = &mr;
    wgpuBufferMapAsync(readback, WGPUMapMode_Read, 0, bd.size, mci);
    for (int i = 0; i < 10000 && !mr.done; ++i) {
        im.device->poll(true);
        wgpuInstanceProcessEvents(im.device->instance());
    }
    if (!mr.done || mr.status != WGPUMapAsyncStatus_Success) {
        wgpuBufferRelease(readback);
        return fail("gpu_map_failed", "id readback map failed: {}", mr.msg);
    }
    const auto* px = static_cast<const std::uint8_t*>(wgpuBufferGetConstMappedRange(readback, 0, bd.size));
    IdImage img;
    img.width = im.id_width;
    img.height = im.id_height;
    img.ids.resize(static_cast<std::size_t>(im.id_width) * im.id_height);
    for (std::uint32_t y = 0; y < im.id_height; ++y) {
        std::memcpy(img.ids.data() + static_cast<std::size_t>(y) * im.id_width, px + static_cast<std::size_t>(y) * bpr, bpr_unpadded);
    }
    wgpuBufferUnmap(readback);
    wgpuBufferRelease(readback);
    return img;
}

Result<world::EntityId> Renderer::pick(std::uint32_t x, std::uint32_t y) {
    POCKET_TRY(img, read_ids());
    // A window pixel, in the drawn frame's pixels when the view was drawn at a scale.
    x = static_cast<std::uint32_t>(static_cast<float>(x) / impl_->window_sx);
    y = static_cast<std::uint32_t>(static_cast<float>(y) / impl_->window_sy);
    if (x >= img.width || y >= img.height) return fail("bad_args", "pixel ({}, {}) outside {}x{}", x, y, img.width, img.height);
    return static_cast<world::EntityId>(img.ids[static_cast<std::size_t>(y) * img.width + x]);
}

const RenderStats& Renderer::stats() const { return impl_->stats; }
const CameraView& Renderer::camera() const { return impl_->camera; }

bool Renderer::unproject(float px, float py, Vec3& origin, Vec3& direction) const {
    const Impl& im = *impl_;
    if (im.applied.w == 0 || im.applied.h == 0) return false;
    px /= im.window_sx;   // window pixels to the drawn frame's
    py /= im.window_sy;
    const float nx = ((px - static_cast<float>(im.applied.x)) / static_cast<float>(im.applied.w)) * 2.0f - 1.0f;
    const float ny = 1.0f - ((py - static_cast<float>(im.applied.y)) / static_cast<float>(im.applied.h)) * 2.0f;
    const Mat4 inv = (im.camera.proj * im.camera.view).inverse();
    const Vec4 a = inv * Vec4{nx, ny, 0.0f, 1.0f};  // on the near plane (depth 0)
    const Vec4 b = inv * Vec4{nx, ny, 1.0f, 1.0f};  // on the far plane (depth 1)
    if (std::abs(a.w) < 1e-12f || std::abs(b.w) < 1e-12f) return false;
    const Vec3 pa{a.x / a.w, a.y / a.w, a.z / a.w};
    const Vec3 pb{b.x / b.w, b.y / b.w, b.z / b.w};
    const Vec3 d = pb - pa;
    const float len = length(d);
    if (!(len > 0)) return false;
    origin = pa;
    direction = d * (1.0f / len);
    return true;
}

bool Renderer::project(Vec3 world_pos, float& out_x, float& out_y) const {
    const Impl& im = *impl_;
    Vec4 clip = (im.camera.proj * im.camera.view) * Vec4{world_pos.x, world_pos.y, world_pos.z, 1};
    if (clip.w <= 0) return false;
    float nx = clip.x / clip.w, ny = clip.y / clip.w;
    out_x = (static_cast<float>(im.applied.x) + (nx * 0.5f + 0.5f) * static_cast<float>(im.applied.w)) * im.window_sx;
    out_y = (static_cast<float>(im.applied.y) + (1.0f - (ny * 0.5f + 0.5f)) * static_cast<float>(im.applied.h)) * im.window_sy;
    return true;
}

void Renderer::set_viewport(Viewport v) { impl_->viewport = v; }
void Renderer::set_view(std::optional<ViewOverride> view) {
    impl_->view_override = view;
    impl_->taa_valid = false;   // a cut: no history to blend with
}
void Renderer::cut() {
    impl_->taa_valid = false;
    impl_->motion_prev_set = false;
    impl_->volume_valid = false;
}

void Renderer::set_msaa(int samples) { impl_->msaa = samples > 1 ? 4 : 1; }  // WebGPU multisamples at 1 or 4
int Renderer::msaa() const { return impl_->msaa; }
void Renderer::set_shadows(ShadowSettings s) {
    s.cascades = std::clamp(s.cascades, 1, static_cast<int>(kCascades));
    s.distance = std::max(s.distance, 1.0f);
    s.softness = std::clamp(s.softness, 0.0f, 5.0f);
    s.contact_length = std::clamp(s.contact_length, 0.02f, 4.0f);
    impl_->shadows = s;
}
void Renderer::set_bloom(BloomSettings s) {
    s.threshold = std::clamp(s.threshold, 0.0f, 64.0f);
    s.strength = std::clamp(s.strength, 0.0f, 4.0f);
    s.radius = std::clamp(s.radius, 0.25f, 8.0f);
    impl_->bloom = s;
}
BloomSettings Renderer::bloom() const { return impl_->bloom; }
void Renderer::set_grade(GradeSettings s) {
    s.exposure = std::clamp(s.exposure, 0.0f, 8.0f);
    s.temperature = std::clamp(s.temperature, -1.0f, 1.0f);
    s.contrast = std::clamp(s.contrast, 0.0f, 4.0f);
    s.saturation = std::clamp(s.saturation, 0.0f, 4.0f);
    s.vignette = std::clamp(s.vignette, 0.0f, 1.0f);
    for (float* c : {&s.tint.r, &s.tint.g, &s.tint.b}) *c = std::clamp(*c, 0.0f, 4.0f);
    s.tint.a = 1.0f;
    impl_->grade = s;
}
GradeSettings Renderer::grade() const { return impl_->grade; }
void Renderer::set_ambient(AmbientSettings s) {
    s.intensity = std::clamp(s.intensity, 0.0f, 16.0f);
    impl_->ambient = s;
}
AmbientSettings Renderer::ambient() const { return impl_->ambient; }
void Renderer::set_ao(AoSettings s) {
    s.radius = std::clamp(s.radius, 0.01f, 50.0f);
    s.intensity = std::clamp(s.intensity, 0.0f, 4.0f);
    s.samples = std::clamp(s.samples, 4, 32);
    impl_->ao = s;
}
AoSettings Renderer::ao() const { return impl_->ao; }
void Renderer::set_taa(TaaSettings s) {
    s.feedback = std::clamp(s.feedback, 0.5f, 0.98f);
    if (s.enabled && !impl_->taa.enabled) impl_->taa_valid = false;   // a history from before is not this view's
    impl_->taa = s;
}
TaaSettings Renderer::taa() const { return impl_->taa; }
void Renderer::set_oit(bool enabled) { impl_->oit = enabled; }
bool Renderer::oit() const { return impl_->oit; }
void Renderer::set_dof(DofSettings s) {
    s.focus = std::clamp(s.focus, 0.01f, 100000.0f);
    s.aperture = std::clamp(s.aperture, 0.0f, 0.1f);
    s.max_blur = std::clamp(s.max_blur, 0.0f, 0.1f);
    impl_->dof = s;
}
DofSettings Renderer::dof() const { return impl_->dof; }
void Renderer::set_ssr(SsrSettings s) {
    s.max_distance = std::clamp(s.max_distance, 0.1f, 1000.0f);
    s.max_roughness = std::clamp(s.max_roughness, 0.0f, 1.0f);
    s.steps = std::clamp(s.steps, 8, 128);
    s.thickness = std::clamp(s.thickness, 0.001f, 10.0f);
    s.intensity = std::clamp(s.intensity, 0.0f, 2.0f);
    impl_->ssr = s;
}
SsrSettings Renderer::ssr() const { return impl_->ssr; }
void Renderer::set_ssgi(SsgiSettings s) {
    s.distance = std::clamp(s.distance, 0.1f, 100.0f);
    s.rays = std::clamp(s.rays, 1, 8);
    s.steps = std::clamp(s.steps, 4, 64);
    s.thickness = std::clamp(s.thickness, 0.001f, 10.0f);
    s.intensity = std::clamp(s.intensity, 0.0f, 4.0f);
    impl_->ssgi = s;
}
SsgiSettings Renderer::ssgi() const { return impl_->ssgi; }
Json Renderer::probes() const {
    Json list = Json::array();
    for (std::uint32_t i = 0; i < impl_->probe_count; ++i) {
        const Impl::ProbeSlot& s = impl_->probe_slots[i];
        list.push_back(Json{{"entity", s.entity}, {"layer", i}, {"center", Json{{"x", s.center.x}, {"y", s.center.y}, {"z", s.center.z}}}, {"size", Json{{"x", s.size.x}, {"y", s.size.y}, {"z", s.size.z}}},
                            {"captured", s.captured}, {"bounces", s.bounces}, {"frame", s.frame}, {"realtime", s.realtime}});
    }
    Json grids = Json::array();
    for (const Impl::GridVolume& g : impl_->grids) {
        grids.push_back(Json{{"entity", g.entity}, {"center", Json{{"x", g.center.x}, {"y", g.center.y}, {"z", g.center.z}}}, {"size", Json{{"x", g.size.x}, {"y", g.size.y}, {"z", g.size.z}}},
                             {"probes", Json{{"x", g.nx}, {"y", g.ny}, {"z", g.nz}}}, {"ready", g.ready}, {"passes", g.passes}, {"next", g.next}, {"frame", g.frame}});
    }
    return Json{{"probes", list}, {"volumes", grids}, {"frame", impl_->frame_number}, {"max", kMaxProbes}, {"max_volume_probes", kMaxGridProbes}};
}
void Renderer::refresh_probes() { impl_->probe_refresh = true; }
void Renderer::set_motion_blur(MotionBlurSettings s) {
    s.strength = std::clamp(s.strength, 0.0f, 2.0f);
    s.samples = std::clamp(s.samples, 4, 32);
    impl_->motion_blur = s;
}
MotionBlurSettings Renderer::motion_blur() const { return impl_->motion_blur; }
void Renderer::set_tonemap(TonemapSettings s) {
    s.exposure = std::clamp(s.exposure, 0.0f, 64.0f);
    s.compensation = std::clamp(s.compensation, -16.0f, 16.0f);
    s.min_ev = std::clamp(s.min_ev, -24.0f, 24.0f);
    s.max_ev = std::clamp(s.max_ev, -24.0f, 24.0f);
    s.speed = std::clamp(s.speed, 0.0f, 1000.0f);
    if (s.auto_exposure && !impl_->tonemap.auto_exposure) impl_->meter_reset = true;   // turned on: the first metered frame snaps
    impl_->tonemap = s;
}
TonemapSettings Renderer::tonemap() const { return impl_->tonemap; }
void Renderer::set_time_step(float seconds) { impl_->time_step = seconds; }

std::vector<std::string> Renderer::set_post_effects(const std::vector<PostEffect>& effects) { return impl_->set_user_effects(effects); }

std::string Renderer::set_sprite_material(const std::string& name, const std::string& wgsl) { return impl_->set_sprite_material(name, wgsl); }
std::string Renderer::set_mesh_material(const std::string& name, const std::string& wgsl) { return impl_->set_mesh_material(name, wgsl); }
bool Renderer::has_mesh_material(const std::string& name) const { return impl_->mesh_materials.contains(name); }
bool Renderer::has_sprite_material(const std::string& name) const { return impl_->sprite_materials.contains(name); }
void Renderer::forget_sprite_materials() { impl_->release_sprite_materials(); }

std::vector<Renderer::PostEffect> Renderer::post_effects() const {
    std::vector<PostEffect> out;
    for (const auto& e : impl_->user_effects) out.push_back(e.def);
    return out;
}

const char* tonemap_name(Tonemap t) {
    switch (t) {
        case Tonemap::Aces: return "aces";
        case Tonemap::Agx: return "agx";
        case Tonemap::Neutral: return "neutral";
        default: return "none";
    }
}
bool tonemap_from_name(const std::string& name, Tonemap& out) {
    for (Tonemap t : {Tonemap::None, Tonemap::Aces, Tonemap::Agx, Tonemap::Neutral}) {
        if (name == tonemap_name(t)) { out = t; return true; }
    }
    if (name == "filmic") { out = Tonemap::Aces; return true; }
    return false;
}

Result<Renderer::Metering> Renderer::metering() {
    Impl& im = *impl_;
    Metering m;
    if (!im.meter_state) return m;
    WGPUBufferDescriptor bd{};
    bd.label = rhi::str("pocket.meter.readback");
    bd.usage = WGPUBufferUsage_MapRead | WGPUBufferUsage_CopyDst;
    bd.size = sizeof(float) * 4;
    WGPUBuffer readback = wgpuDeviceCreateBuffer(im.device->device(), &bd);
    if (!readback) return fail("gpu_buffer_failed", "cannot create the meter readback buffer");
    WGPUCommandEncoder enc = wgpuDeviceCreateCommandEncoder(im.device->device(), nullptr);
    wgpuCommandEncoderCopyBufferToBuffer(enc, im.meter_state, 0, readback, 0, sizeof(float) * 4);
    WGPUCommandBuffer cb = wgpuCommandEncoderFinish(enc, nullptr);
    wgpuQueueSubmit(im.device->queue(), 1, &cb);
    wgpuCommandBufferRelease(cb);
    wgpuCommandEncoderRelease(enc);
    struct MapResult { bool done = false; WGPUMapAsyncStatus status{}; } mr;
    WGPUBufferMapCallbackInfo mci{};
    mci.mode = WGPUCallbackMode_AllowSpontaneous;
    mci.callback = [](WGPUMapAsyncStatus status, WGPUStringView, void* u1, void*) {
        auto* r = static_cast<MapResult*>(u1);
        r->status = status;
        r->done = true;
    };
    mci.userdata1 = &mr;
    wgpuBufferMapAsync(readback, WGPUMapMode_Read, 0, sizeof(float) * 4, mci);
    while (!mr.done) im.device->poll(true);
    if (mr.status != WGPUMapAsyncStatus_Success) {
        wgpuBufferRelease(readback);
        return fail("gpu_map_failed", "cannot read the meter back");
    }
    const auto* p = static_cast<const float*>(wgpuBufferGetConstMappedRange(readback, 0, sizeof(float) * 4));
    m.exposure_ev = p[0];
    m.average_ev = p[1];
    m.valid = p[2] > 0.5f;
    wgpuBufferUnmap(readback);
    wgpuBufferRelease(readback);
    return m;
}
ShadowSettings Renderer::shadows() const { return impl_->shadows; }
void Renderer::set_assets(assets::AssetStore* store) { impl_->assets = store; }
std::vector<std::pair<std::string, std::pair<Vec3, Vec3>>> Renderer::take_new_bounds() { return std::exchange(impl_->new_bounds, {}); }
void Renderer::drop_asset_cache() { impl_->release_assets(); }
Viewport Renderer::viewport() const { return impl_->viewport; }
Viewport Renderer::applied_viewport() const {
    const Impl& im = *impl_;
    if (im.window_sx == 1.0f && im.window_sy == 1.0f) return im.applied;
    return Viewport{static_cast<std::int32_t>(std::lround(im.applied.x * im.window_sx)), static_cast<std::int32_t>(std::lround(im.applied.y * im.window_sy)),
                    static_cast<std::uint32_t>(std::lround(im.applied.w * im.window_sx)), static_cast<std::uint32_t>(std::lround(im.applied.h * im.window_sy))};
}
void Renderer::set_render_scale(RenderScaleSettings s) {
    s.scale = std::clamp(s.scale, 0.25f, 1.0f);
    s.least = std::clamp(s.least, 0.25f, s.scale);
    s.target_ms = std::max(s.target_ms, 0.5f);
    s.sharpen = std::clamp(s.sharpen, 0.0f, 1.0f);
    impl_->scale_settings = s;
    impl_->scale_now = s.scale;   // dynamic starts from the most
    impl_->scale_samples.clear();
}
RenderScaleSettings Renderer::render_scale() const { return impl_->scale_settings; }
void Renderer::set_colour_vision(ColourVisionSettings s) {
    s.mode = std::clamp(s.mode, 0, 3);
    s.strength = std::clamp(s.strength, 0.0f, 1.0f);
    impl_->colour_vision = s;
}
ColourVisionSettings Renderer::colour_vision() const { return impl_->colour_vision; }
void Renderer::set_toon(ToonSettings s) {
    s.bands = std::clamp(s.bands, 1, 16);
    s.softness = std::clamp(s.softness, 0.0f, 0.5f);
    s.outline = std::clamp(s.outline, 0.0f, 16.0f);
    s.opacity = std::clamp(s.opacity, 0.0f, 1.0f);
    impl_->toon = s;
}
ToonSettings Renderer::toon() const { return impl_->toon; }

Json Renderer::describe() const {
    const RenderStats& s = impl_->stats;
    Json j;
    j["draw_calls"] = s.draw_calls;
    if (s.gpu_timing) {
        // Each pass's GPU milliseconds (the same label twice is summed), the busiest first.
        std::map<std::string, double> by;
        for (const auto& [name, ms] : s.gpu_passes) by[name] += ms;
        std::vector<std::pair<std::string, double>> sorted(by.begin(), by.end());
        std::sort(sorted.begin(), sorted.end(), [](const auto& a, const auto& b) { return a.second > b.second; });
        Json passes = Json::array();
        for (const auto& [name, ms] : sorted) passes.push_back(Json{{"pass", name}, {"ms", std::round(ms * 1000.0) / 1000.0}});
        j["gpu"] = Json{{"ms", std::round(s.gpu_ms * 1000.0) / 1000.0}, {"passes", passes}, {"frames_ago", s.gpu_age}, {"frames_timed", s.gpu_frames}};
    }
    if (s.render_scale < 1.0f) j["scale"] = Json{{"scale", s.render_scale}, {"width", s.render_width}, {"height", s.render_height}};
    if (s.toon) j["toon"] = true;
    if (s.highlights) j["highlights"] = s.highlights;
    if (s.colour_vision > 0) j["colour_vision"] = std::array<const char*, 4>{"off", "protanopia", "deuteranopia", "tritanopia"}[static_cast<std::size_t>(s.colour_vision)];
    j["shadow_draws"] = s.shadow_draws;
    j["shadow_instances"] = s.shadow_instances;
    j["shadows"] = s.shadows;
    j["shadow_cascades"] = s.shadow_cascades;
    j["shadow_distance"] = s.shadow_distance;
    j["instances"] = s.instances;
    j["scattered"] = s.scattered;
    j["translucent"] = s.translucent;
    j["glass"] = s.glass;
    j["lod"] = Json{{"simplified", s.lod_simplified}, {"culled", s.lod_culled}};
    j["out_of_view"] = s.out_of_view;
    j["triangles"] = s.triangles;
    j["sprites"] = s.sprites;
    j["particles"] = s.particles;
    j["skinned"] = s.skinned;
    j["morphed"] = s.morphed;
    j["moving_parts"] = s.moving_parts;
    j["tile_layers"] = s.tile_layers;
    j["image_layers"] = s.image_layers;
    j["tile_rebuilds"] = s.tile_rebuilds;
    j["tile_frames"] = s.tile_frames;
    j["msaa"] = s.msaa;
    j["bloom"] = s.bloom;
    j["grade"] = s.grade;
    j["hdr"] = true;
    j["tonemap"] = tonemap_name(static_cast<Tonemap>(s.tonemap));
    if (s.post_effects > 0) j["post_effects"] = s.post_effects;
    j["auto_exposure"] = s.auto_exposure;
    j["sky"] = s.sky == 1 ? "procedural" : s.sky == 2 ? "image" : s.sky == 3 ? "atmosphere" : "none";
    j["env_updates"] = s.env_updates;
    j["depth_prepass"] = s.depth_prepass;
    j["ao"] = s.ao;
    j["soft_shadows"] = s.soft_shadows;
    j["contact_shadows"] = s.contact_shadows;
    j["fog"] = s.fog;
    j["volumetric"] = s.volumetric;
    j["taa"] = s.taa;
    j["oit"] = s.oit;
    j["lut"] = s.lut;
    j["ssr"] = s.ssr;
    j["ssgi"] = s.ssgi;
    j["probes"] = Json{{"in_use", s.probes}, {"captured", s.probe_captures}, {"volumes", s.grids}, {"volume_captures", s.grid_captures}};
    j["water"] = Json{{"bodies", s.water}, {"underwater", s.underwater}};
    j["decals"] = Json{{"drawn", s.decals}, {"images", s.decal_images}};
    j["dof"] = s.dof;
    j["motion_blur"] = s.motion_blur;
    j["id_draws"] = s.id_draws;
    j["debug_lines"] = s.debug_lines;
    j["weather_drops"] = s.weather_drops;
    j["shelter_draws"] = s.shelter_draws;
    j["materials"] = s.materials;
    j["meshes"] = s.meshes;
    j["point_lights"] = s.point_lights;
    j["spot_lights"] = s.spot_lights;
    j["light_shadows"] = Json{{"lights", s.shadow_lights}, {"faces", s.shadow_faces}, {"atlas", kAtlasSize}, {"face_size", kFaceSize}};
    j["lights"] = Json{{"clusters", Json::array({kClusterX, kClusterY, kClusterZ})}, {"culled", s.lights_culled}, {"dropped", s.lights_dropped}, {"entries", s.cluster_entries}, {"max_per_cluster", s.max_cluster_lights}};
    j["has_camera"] = s.has_camera;
    j["has_sun"] = s.has_sun;
    j["sun_light"] = Json{{"r", s.sun_light[0]}, {"g", s.sun_light[1]}, {"b", s.sun_light[2]}};
    if (s.camera) j["camera"] = s.camera;
    if (s.blend < 1.0f) j["camera_blend"] = Json{{"from", s.blend_from}, {"progress", std::round(s.blend * 1000.0f) / 1000.0f}};
    const Viewport& v = impl_->applied;
    if (v.w != impl_->last_width || v.h != impl_->last_height) j["viewport"] = Json{{"x", v.x}, {"y", v.y}, {"w", v.w}, {"h", v.h}};
    Json a;
    a["meshes"] = s.asset_meshes;
    a["textures"] = s.textures;
    if (!s.missing.empty()) a["missing"] = s.missing;
    j["assets"] = a;
    return j;
}

}  // namespace pocket::renderer
