#include <pocket/renderer/renderer.hpp>

#include <pocket/core/log.hpp>
#include <pocket/renderer/primitives.hpp>

#ifndef __EMSCRIPTEN__
#include <webgpu/wgpu.h>
#endif

#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>
#include <map>
#include <set>

namespace pocket::renderer {

namespace {

constexpr std::uint32_t kMaxPointLights = 8;
// The scene is drawn into a half-float target (lighting in linear light, values over 1 kept), and
// a final pass exposes, tone-maps and sRGB-encodes it into the 8-bit frame.
constexpr WGPUTextureFormat kHdrFormat = WGPUTextureFormat_RGBA16Float;

// Colors in components are authored as a color picker shows them (sRGB); lighting needs linear
// light. Values over 1 are intensities and pass through (an emissive of 4 is four times white).
float decode(float c) { return c <= 0.04045f ? c / 12.92f : (c <= 1.0f ? std::pow((c + 0.055f) / 1.055f, 2.4f) : c); }

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
constexpr std::uint32_t kObjectStride = 256;  // sizeof(ObjectUniforms)
constexpr std::uint32_t kMaxObjects = 65536;

struct alignas(16) FrameUniforms {
    float view_proj[16];
    float camera_pos[4];
    float sun_dir[4];
    float sun_color[4];
    float ambient[4];
    std::uint32_t point_count[4];
    float point_pos[kMaxPointLights][4];
    float point_color[kMaxPointLights][4];
    float light_view_proj[16];   // the sun's orthographic view for the shadow map
    float shadow[4];             // texel size, depth bias, strength, enabled
    float inv_view_proj[16];     // clip to world, for the sky's view directions
    float env[4];                // environment light: on, diffuse, specular, the prefiltered map's last level
    float sky[4];                // sky drawn: mode, cos of the sun disc's radius, the disc's brightness, a sun exists
    float cascade_vp[4][16];     // the sun's view-projection per cascade
    float cascade_far[4];        // the view depth each cascade reaches
    float cascade_texel[4];      // a texel of each cascade in world units (the lookup's normal offset)
    float camera_fwd[4];         // xyz: the camera's forward (view depth), w: cascades in use
};
constexpr std::uint32_t kCascades = 4;
// The environment map: an equirectangular panorama with its GGX prefiltered levels as mips.
constexpr std::uint32_t kEnvWidth = 512, kEnvHeight = 256, kEnvLevels = 6;
constexpr std::uint32_t kShadowMapSize = 2048;

struct alignas(16) ObjectUniforms {
    float model[16];
    float normal[16];
    float color[4];
    std::uint32_t id[4];      // x: entity id, y: flags (1 = unlit)
    float uv_rect[4];         // u0, v0, u1, v1 (sprites cut a sheet; meshes use 0,0,1,1)
    float pbr[4];             // metallic, roughness, normal scale, 1 when a normal map is bound
    float emissive[4];        // linear RGB added after lighting; w: alpha cutoff (texels under it are cut out; 0 for none)
    std::uint32_t morph[4];   // x: first vec4 of the asset's morph deltas, y: vertices per target, z: targets weighed (0: none)
    float morph_weights[8];   // one per target, up to eight
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

constexpr const char* kMeshWgsl = R"WGSL(
struct Frame {
    view_proj: mat4x4f,
    camera_pos: vec4f,
    sun_dir: vec4f,
    sun_color: vec4f,
    ambient: vec4f,
    point_count: vec4u,
    point_pos: array<vec4f, 8>,
    point_color: array<vec4f, 8>,
    light_view_proj: mat4x4f,
    shadow: vec4f,
    inv_view_proj: mat4x4f,
    env: vec4f,
    sky: vec4f,
    cascade_vp: array<mat4x4f, 4>,
    cascade_far: vec4f,
    cascade_texel: vec4f,
    camera_fwd: vec4f,
};
@group(0) @binding(1) var shadow_map: texture_depth_2d_array;
@group(0) @binding(2) var shadow_samp: sampler_comparison;
// The sky's light: the panorama with GGX-prefiltered mips (roughness 0 to 1), and its diffuse
// irradiance as nine spherical-harmonic coefficients (already divided by pi).
@group(0) @binding(3) var env_tex: texture_2d<f32>;
@group(0) @binding(4) var env_samp: sampler;
@group(0) @binding(5) var<storage, read> sh: array<vec4f, 9>;
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

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) world_pos: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) color: vec4f,
    @location(4) @interpolate(flat) id: u32,
    @location(5) @interpolate(flat) instance: u32,
};

@vertex fn vs(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f) -> VsOut {
    let object = objects[instance];
    var out: VsOut;
    let world = object.model * vec4f(morph_position(object, vid, position), 1.0);
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((object.normal * vec4f(morph_normal(object, vid, normal), 0.0)).xyz);
    out.uv = mix(object.uv_rect.xy, object.uv_rect.zw, uv);
    out.color = object.color;
    out.id = object.id.x;
    out.instance = instance;
    return out;
}

// Shadow pass: depth only, from the sun, once per cascade (its matrix comes with a dynamic offset).
struct Cascade { view_proj: mat4x4f };
@group(2) @binding(5) var<uniform> cascade: Cascade;
@vertex fn vs_shadow(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f) -> @builtin(position) vec4f {
    let object = objects[instance];
    return cascade.view_proj * (object.model * vec4f(morph_position(object, vid, position), 1.0));
}

// Skinned meshes: the joint matrices of this instance start at object.id.z in the joints array.
fn skin_matrix(base: u32, j: vec4u, w: vec4f) -> mat4x4f {
    return joints[base + j.x] * w.x + joints[base + j.y] * w.y + joints[base + j.z] * w.z + joints[base + j.w] * w.w;
}

@vertex fn vs_skinned(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f, @location(3) j: vec4u, @location(4) w: vec4f) -> VsOut {
    let object = objects[instance];
    let model = object.model * skin_matrix(object.id.z, j, w);
    var out: VsOut;
    let world = model * vec4f(morph_position(object, vid, position), 1.0);
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((model * vec4f(morph_normal(object, vid, normal), 0.0)).xyz);
    out.uv = mix(object.uv_rect.xy, object.uv_rect.zw, uv);
    out.color = object.color;
    out.id = object.id.x;
    out.instance = instance;
    return out;
}

@vertex fn vs_shadow_skinned(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f, @location(3) j: vec4u, @location(4) w: vec4f) -> @builtin(position) vec4f {
    let object = objects[instance];
    let model = object.model * skin_matrix(object.id.z, j, w);
    return cascade.view_proj * (model * vec4f(morph_position(object, vid, position), 1.0));
}

struct FsOut {
    @location(0) color: vec4f,
    @location(1) id: u32,
};

// Tangent frame from screen-space derivatives (no vertex tangents needed), then the map's
// +Y-up (glTF) normal bent into world space.
fn perturb_normal(n: vec3f, dp1: vec3f, dp2: vec3f, duv1: vec2f, duv2: vec2f, map: vec3f) -> vec3f {
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let invmax = inverseSqrt(max(dot(t, t), dot(b, b)) + 1e-12);
    let tbn = mat3x3f(t * invmax, b * invmax, n);
    return normalize(tbn * vec3f(map.x, -map.y, map.z));
}

// GGX / Schlick / Smith specular plus Lambert diffuse for one light direction; the diffuse term
// is not divided by pi (and the specular scaled to match) so brightness stays comparable to a plain
// Lambert surface under a light of intensity one.
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

// How lit a point is by the sun in one cascade (0 shadowed .. 1 lit), outside the map lit.
fn cascade_lit(c: i32, world_pos: vec3f, gn: vec3f, ndl: f32) -> f32 {
    let p = world_pos + gn * frame.cascade_texel[c] * 1.5;
    let sp = frame.cascade_vp[c] * vec4f(p, 1.0);
    let ndc = sp.xyz / sp.w;
    let suv = vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0 || ndc.z < 0.0 || ndc.z > 1.0) { return 1.0; }
    let bias = frame.shadow.y * (1.0 + 2.0 * (1.0 - ndl));
    var lit = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            lit = lit + textureSampleCompareLevel(shadow_map, shadow_samp, suv + vec2f(f32(i), f32(j)) * frame.shadow.x, c, ndc.z - bias);
        }
    }
    return lit / 9.0;
}

fn shade(in: VsOut) -> vec4f {
    let object = objects[in.instance];
    // Derivatives and samples first: both must stay in uniform control flow.
    let dp1 = dpdx(in.world_pos);
    let dp2 = dpdy(in.world_pos);
    let duv1 = dpdx(in.uv);
    let duv2 = dpdy(in.uv);
    let base = textureSample(base_tex, base_samp, in.uv) * in.color;
    let mr = textureSample(mr_tex, base_samp, in.uv);
    let nm = textureSample(normal_tex, base_samp, in.uv).xyz * 2.0 - 1.0;
    let em = textureSample(emissive_tex, base_samp, in.uv).rgb;
    // A cut-out: texels under the cutoff are not drawn (nor picked, the id goes with the color).
    if (object.emissive.w > 0.0 && base.a < object.emissive.w) { discard; }
    var n = normalize(in.normal);
    if (object.pbr.w > 0.5) {
        n = perturb_normal(n, dp1, dp2, duv1, duv2, vec3f(nm.xy * object.pbr.z, nm.z));
    }
    let v = normalize(frame.camera_pos.xyz - in.world_pos);
    let metallic = clamp(object.pbr.x * mr.b, 0.0, 1.0);
    let roughness = clamp(object.pbr.y * mr.g, 0.04, 1.0);
    let albedo = base.rgb;
    let f0 = mix(vec3f(0.04), albedo, metallic);
    var color = frame.ambient.rgb * mix(albedo, f0, metallic);
    if (frame.env.x > 0.5) {
        // The sky's light instead of the flat ambient: diffuse from its harmonics, specular from the
        // prefiltered level matching the roughness, in the mirror direction.
        let ndv = max(dot(n, v), 1e-4);
        let spec = textureSampleLevel(env_tex, env_samp, env_uv(reflect(-v, n)), roughness * frame.env.w).rgb;
        color = albedo * (1.0 - metallic) * sh_irradiance(n) * frame.env.y + spec * env_brdf(f0, roughness, ndv) * frame.env.z;
    }
    // Directional light.
    let l = normalize(-frame.sun_dir.xyz);
    let ndl = max(dot(n, l), 0.0);
    // Shadow: the cascade covering this depth (blended into the next over the last tenth of it),
    // looked up a texel out along the surface's normal, with a 3x3 comparison filter.
    var shadow = 1.0;
    if (frame.shadow.w > 0.5) {
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
    color += frame.sun_color.rgb * brdf(n, v, l, albedo, metallic, roughness) * shadow;
    // Point lights.
    for (var i = 0u; i < frame.point_count.x; i = i + 1u) {
        let to_light = frame.point_pos[i].xyz - in.world_pos;
        let dist = length(to_light);
        let range = frame.point_pos[i].w;
        let att = clamp(1.0 - (dist * dist) / (range * range), 0.0, 1.0);
        let pl = to_light / max(dist, 0.0001);
        color += frame.point_color[i].rgb * brdf(n, v, pl, albedo, metallic, roughness) * att * att;
    }
    color += object.emissive.rgb * em;
    return vec4f(color, base.a);
}

// One pass writes color and id together (no MSAA); with MSAA the color pass and the id pass are
// separate, since an integer id target cannot be multisampled and resolved.

@fragment fn fs(in: VsOut) -> FsOut {
    var out: FsOut;
    out.color = shade(in);
    out.id = in.id;
    return out;
}
@fragment fn fs_color(in: VsOut) -> @location(0) vec4f {
    return shade(in);
}
@fragment fn fs_id(in: VsOut) -> @location(0) u32 {
    let object = objects[in.instance];
    let a = textureSample(base_tex, base_samp, in.uv).a * in.color.a;
    if (object.emissive.w > 0.0 && a < object.emissive.w) { discard; }
    return in.id;
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
fn sky_color(ndc: vec2f) -> vec3f {
    let far = frame.inv_view_proj * vec4f(ndc, 1.0, 1.0);
    let near = frame.inv_view_proj * vec4f(ndc, 0.0, 1.0);
    let d = normalize(far.xyz / far.w - near.xyz / near.w);
    var c = textureSampleLevel(env_tex, env_samp, env_uv(d), 0.0).rgb;
    if (frame.sky.x > 0.5 && frame.sky.x < 1.5 && frame.sky.w > 0.5) {
        let mu = dot(d, normalize(-frame.sun_dir.xyz));
        let edge = frame.sky.y;
        c = c + frame.sun_color.rgb * frame.sky.z * smoothstep(edge, edge + (1.0 - edge) * 0.2, mu);
    }
    return c;
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
@fragment fn fs_unlit_id(in: VsOut) -> @location(0) u32 {
    let base = unlit(in);
    if (base.a < 0.02) { discard; }
    return in.id;
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
struct Post { exposure: f32, op: u32, auto_on: u32, grade_on: u32, tint: vec4f, temperature: f32, contrast: f32, saturation: f32, vignette: f32, viewport: vec4f };
@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var hdr: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> metered: array<f32, 4>;   // [0] the exposure the meter settled on, in EV
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
    var c = max(s.rgb * exposure, vec3f(0.0));
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
struct SkyParams { zenith: vec4f, horizon: vec4f, ground: vec4f, sun: vec4f, sun_color: vec4f, misc: vec4f, size: vec4f };
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
@compute @workgroup_size(8, 8) fn fill(@builtin(global_invocation_id) id: vec3u) {
    if (f32(id.x) >= sp.size.x || f32(id.y) >= sp.size.y) { return; }
    let uv = (vec2f(id.xy) + 0.5) / sp.size.xy;
    let d = dir_of(uv);
    var c: vec3f;
    if (sp.misc.x > 1.5) {
        let r = sp.misc.y;
        let rd = vec3f(d.x * cos(r) + d.z * sin(r), d.y, -d.x * sin(r) + d.z * cos(r));
        c = textureSampleLevel(src, samp, uv_of(rd), 0.0).rgb;
    } else {
        c = procedural(d);
    }
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
    WGPURenderPipeline blend_pipeline = nullptr;           // the lit shading alpha blended, no depth writes
    WGPURenderPipeline blend_skinned_pipeline = nullptr;
    WGPURenderPipeline shadow_skinned_pipeline = nullptr;
    WGPUBuffer joint_buffer = nullptr;
    std::vector<float> joint_staging;  // 16 floats per matrix
    std::uint32_t joint_count = 0;
    WGPUBuffer morph_buffer = nullptr;  // every morphed asset's deltas, appended as assets load
    std::uint32_t morph_used = 0;       // vec4s of it in use
    bool morph_full_warned = false;
    const Animation* animation = nullptr;  // poses for the frame being drawn
    WGPURenderPipeline sprite_pipeline = nullptr;
    WGPURenderPipeline line_pipeline = nullptr;
    WGPUPipelineLayout line_layout = nullptr;
    WGPUShaderModule line_shader = nullptr;
    WGPUBuffer line_buffer = nullptr;
    std::size_t line_capacity = 0;  // vertices
    WGPURenderPipeline shadow_pipeline = nullptr;
    WGPUPipelineLayout shadow_layout = nullptr;
    WGPUBindGroupLayout scene_bgl = nullptr;   // frame uniforms + shadow map + comparison sampler
    WGPUBindGroup scene_bg = nullptr;
    WGPUTexture shadow_texture = nullptr;
    WGPUTextureView shadow_view = nullptr;              // every cascade, for the lookups
    WGPUTextureView cascade_view[kCascades]{};          // one cascade each, for its pass
    WGPUBindGroupLayout cascade_bgl = nullptr;          // the cascade's matrix at group 2 binding 5, dynamic offset
    WGPUBuffer cascade_buffer = nullptr;                // one 256-byte slot per cascade
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
    TonemapSettings tonemap;
    float time_step = 1.0f / 60.0f;
    WGPUShaderModule post_shader = nullptr;
    WGPUBindGroupLayout post_bgl = nullptr;       // uniform, the HDR target, the meter's state
    WGPUPipelineLayout post_layout = nullptr;
    WGPURenderPipeline post_pipeline = nullptr;
    WGPUBuffer post_uniforms = nullptr;
    WGPUBindGroup post_bg = nullptr;
    WGPUShaderModule meter_shader = nullptr;
    WGPUBindGroupLayout meter_bgl = nullptr;
    WGPUPipelineLayout meter_layout = nullptr;
    WGPUComputePipeline meter_pipeline = nullptr;
    WGPUBuffer meter_uniforms = nullptr;
    WGPUBuffer meter_state = nullptr;             // 4 floats: exposure EV, average EV, initialized
    WGPUBindGroup meter_bg = nullptr;
    bool meter_reset = true;                      // the next metered frame snaps instead of easing
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
    WGPUBindGroup frame_bg = nullptr;
    WGPUBindGroup object_bg = nullptr;
    WGPUTexture id_texture = nullptr;
    WGPUTextureView id_view = nullptr;
    std::uint32_t id_width = 0, id_height = 0;
    // MSAA: the scene draws into multisampled color and depth and resolves into the frame; the
    // ids then come from their own single-sample pass with these pipelines.
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
    WGPURenderPipeline id_sprite_pipeline = nullptr;
    WGPUVertexAttribute mesh_attrs[3]{};
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
    GpuTexture flat_normal;   // (0.5, 0.5, 1): no bend
    GpuTexture sky_source;    // the sky's panorama (half floats); 1x1 black without one
    std::map<std::string, GpuTexture> textures;
    std::map<std::string, WGPUBindGroup> material_groups;  // "base|mr|normal|emissive|filter" -> group 2
    struct AssetMesh {
        GpuMesh gpu;
        std::vector<assets::Submesh> submeshes;
        std::vector<Mat4> rest;   // per submesh: where its node rests, for a moving part drawn without a pose
        std::vector<Mat4> unbake; // per submesh: the inverse of its node's rest for baked geometry, so a node draws in its own space
        std::vector<std::string> node_names;   // the file's nodes by index, for MeshRenderer.node
        std::vector<assets::Material> materials;
        std::uint32_t morph_base = 0, morph_targets = 0, morph_vertices = 0;  // the asset's deltas in the morph buffer
    };
    std::map<std::string, AssetMesh> asset_meshes;
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
    std::uint32_t tile_rebuilds = 0;  // layer meshes rebuilt after edits, over the renderer's life
    std::uint32_t tile_frames = 0;    // animated cells rebuilt for a frame change, over the renderer's life
    std::set<std::string> failed;  // asset paths reported once
    std::vector<std::pair<std::string, std::pair<Vec3, Vec3>>> new_bounds;
    RenderStats stats;
    CameraView camera;
    Viewport viewport;   // requested
    Viewport applied;    // used by the last frame
    std::uint32_t last_width = 0, last_height = 0;
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
        asset_meshes.clear();
        for (auto& [path, t] : textures) release_texture(t);
        textures.clear();
        for (auto& [key, bg] : material_groups) wgpuBindGroupRelease(bg);
        material_groups.clear();
        failed.clear();
    }

    ~Impl() {
        release_assets();
        release_texture(white);
        release_texture(flat_normal);
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
        if (object_bg) wgpuBindGroupRelease(object_bg);
        if (frame_bg) wgpuBindGroupRelease(frame_bg);
        if (object_buffer) wgpuBufferRelease(object_buffer);
        if (frame_buffer) wgpuBufferRelease(frame_buffer);
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
        if (line_pipeline) wgpuRenderPipelineRelease(line_pipeline);
        if (line_layout) wgpuPipelineLayoutRelease(line_layout);
        if (line_shader) wgpuShaderModuleRelease(line_shader);
        if (line_buffer) wgpuBufferRelease(line_buffer);
        if (pipeline) wgpuRenderPipelineRelease(pipeline);
        if (skinned_pipeline) wgpuRenderPipelineRelease(skinned_pipeline);
        if (blend_pipeline) wgpuRenderPipelineRelease(blend_pipeline);
        if (blend_skinned_pipeline) wgpuRenderPipelineRelease(blend_skinned_pipeline);
        if (shadow_skinned_pipeline) wgpuRenderPipelineRelease(shadow_skinned_pipeline);
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
            WGPURenderPassEncoder enc = wgpuCommandEncoderBeginRenderPass(frame.encoder, &rp);
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
        WGPUBindGroupLayoutEntry be[3]{};
        be[0].binding = 0;
        be[0].visibility = WGPUShaderStage_Fragment;
        be[0].buffer.type = WGPUBufferBindingType_Uniform;
        be[0].buffer.minBindingSize = sizeof(PostUniforms);
        be[1].binding = 1;
        be[1].visibility = WGPUShaderStage_Fragment;
        be[1].texture.sampleType = WGPUTextureSampleType_UnfilterableFloat;
        be[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        be[2].binding = 2;
        be[2].visibility = WGPUShaderStage_Fragment;
        be[2].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        be[2].buffer.minBindingSize = sizeof(float) * 4;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.post");
        bd.entryCount = 3;
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
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopySrc;
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
        post_bg = group("pocket.post", post_bgl, post_uniforms, sizeof(PostUniforms));
        meter_bg = group("pocket.meter", meter_bgl, meter_uniforms, sizeof(MeterUniforms));
        hdr_w = w;
        hdr_h = h;
        return {};
    }

    // Meter the viewport of the HDR target (auto exposure only), then draw the frame from it:
    // everything outside the viewport is the clear color, as the scene pass left it before.
    Status draw_post(rhi::Frame& frame, rhi::Color clear) {
        const bool metered = tonemap.auto_exposure;
        if (metered) {
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
            WGPUComputePassEncoder cp = wgpuCommandEncoderBeginComputePass(frame.encoder, &cpd);
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
        device->write_buffer(post_uniforms, 0, &u, sizeof u);
        WGPURenderPassColorAttachment ca{};
        ca.view = frame.color;
        ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ca.loadOp = WGPULoadOp_Clear;
        ca.storeOp = WGPUStoreOp_Store;
        ca.clearValue = {clear.r, clear.g, clear.b, clear.a};
        WGPURenderPassDescriptor rp{};
        rp.label = rhi::str("pocket.post");
        rp.colorAttachmentCount = 1;
        rp.colorAttachments = &ca;
        WGPURenderPassEncoder enc = wgpuCommandEncoderBeginRenderPass(frame.encoder, &rp);
        if (applied.w != frame.width || applied.h != frame.height) {
            wgpuRenderPassEncoderSetScissorRect(enc, static_cast<std::uint32_t>(applied.x), static_cast<std::uint32_t>(applied.y), applied.w, applied.h);
        }
        wgpuRenderPassEncoderSetPipeline(enc, post_pipeline);
        wgpuRenderPassEncoderSetBindGroup(enc, 0, post_bg, 0, nullptr);
        wgpuRenderPassEncoderDraw(enc, 3, 1, 0, 0);
        wgpuRenderPassEncoderEnd(enc);
        wgpuRenderPassEncoderRelease(enc);
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
        sh_buffer = device->create_buffer("pocket.sky.harmonics", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, zero_sh.size() * sizeof(float), zero_sh.data());
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
    bool update_environment(rhi::Frame& frame, const world::Sky& sky, bool have_sun, Vec3 toward_sun, Vec3 sun_linear) {
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
        std::snprintf(key, sizeof key, "%d|%s|%g,%g,%g|%g,%g,%g|%g,%g,%g|%g|%g|%d|%g,%g,%g|%g,%g,%g", sky.mode, image ? sky.image.c_str() : "", sky.zenith.r, sky.zenith.g, sky.zenith.b, sky.horizon.r, sky.horizon.g, sky.horizon.b, sky.ground.r, sky.ground.g, sky.ground.b, sky.intensity, sky.rotation, have_sun && !image ? 1 : 0, toward_sun.x, toward_sun.y, toward_sun.z, sun_linear.x, sun_linear.y, sun_linear.z);
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
        WGPUComputePassEncoder cp = wgpuCommandEncoderBeginComputePass(frame.encoder, &cpd);
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

    void release_scene_pipelines() {
        if (sky_pipeline) wgpuRenderPipelineRelease(sky_pipeline);
        sky_pipeline = nullptr;
        for (WGPURenderPipeline* p : {&pipeline, &skinned_pipeline, &blend_pipeline, &blend_skinned_pipeline, &sprite_pipeline, &line_pipeline, &id_pipeline, &id_skinned_pipeline, &id_sprite_pipeline}) {
            if (*p) wgpuRenderPipelineRelease(*p);
            *p = nullptr;
        }
    }

    // The scene pipelines for a sample count: with one sample, color and id share a pass (two
    // targets); with more, the color pipelines are multisampled with one target and separate
    // single-sample pipelines write the ids.
    Status create_scene_pipelines(int samples) {
        release_scene_pipelines();
        const bool split = samples > 1;
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
        // Skinned meshes: the same lit fragment, a vertex stage that blends joint matrices.
        rpd.label = rhi::str("pocket.mesh.skinned");
        rpd.vertex.entryPoint = rhi::str("vs_skinned");
        rpd.vertex.bufferCount = 2;
        rpd.vertex.buffers = vbls;
        skinned_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!skinned_pipeline) return fail("gpu_pipeline_failed", "skinned pipeline creation failed");
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
            WGPUColorTargetState idt{};
            idt.format = WGPUTextureFormat_R32Uint;
            idt.writeMask = WGPUColorWriteMask_All;
            WGPUFragmentState ifs{};
            ifs.module = shader;
            ifs.entryPoint = rhi::str("fs_id");
            ifs.targetCount = 1;
            ifs.targets = &idt;
            WGPUDepthStencilState ids = ds;
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
            irpd.label = rhi::str("pocket.ids.skinned");
            irpd.vertex.entryPoint = rhi::str("vs_skinned");
            irpd.vertex.bufferCount = 2;
            irpd.vertex.buffers = vbls;
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
        return {};
    }

    Status ensure_id_target(std::uint32_t w, std::uint32_t h) {
        if (id_texture && id_width == w && id_height == h) return {};
        if (id_view) { wgpuTextureViewRelease(id_view); id_view = nullptr; }
        if (id_texture) { wgpuTextureRelease(id_texture); id_texture = nullptr; }
        WGPUTextureDescriptor td{};
        td.label = rhi::str("pocket.ids");
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_CopySrc;
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
        WGPUBindGroupLayoutEntry se[6]{};
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
        se[5].binding = 5;
        se[5].visibility = WGPUShaderStage_Fragment;
        se[5].buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        se[5].buffer.minBindingSize = sizeof(float) * 36;
        WGPUBindGroupLayoutDescriptor scene_ld{};
        scene_ld.label = rhi::str("pocket.scene");
        scene_ld.entryCount = 6;
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

        WGPUBindGroupLayoutEntry me[5]{};
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
        WGPUBindGroupLayoutDescriptor md{};
        md.label = rhi::str("pocket.material");
        md.entryCount = 5;
        md.entries = me;
        material_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &md);

        WGPUBindGroupLayout bgls[3] = {scene_bgl, object_bgl, material_bgl};
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.mesh");
        pld.bindGroupLayoutCount = 3;
        pld.bindGroupLayouts = bgls;
        layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);
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
        vbl.arrayStride = sizeof(Vertex);
        vbl.attributeCount = 3;
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
        sds.format = WGPUTextureFormat_Depth32Float;
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
        joint_buffer = device->create_buffer("pocket.joints", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kMaxJoints) * sizeof(float) * 16);
        joint_staging.resize(static_cast<std::size_t>(kMaxJoints) * 16);
        morph_buffer = device->create_buffer("pocket.morphs", WGPUBufferUsage_Storage | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kMaxMorphVec4) * sizeof(float) * 4);

        frame_buffer = device->create_buffer("pocket.frame", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(FrameUniforms));
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
        std_.size = {kShadowMapSize, kShadowMapSize, kCascades};
        std_.format = WGPUTextureFormat_Depth32Float;
        std_.mipLevelCount = 1;
        std_.sampleCount = 1;
        shadow_texture = wgpuDeviceCreateTexture(device->device(), &std_);
        if (!shadow_texture) return fail("gpu_texture_failed", "cannot create the shadow map");
        WGPUTextureViewDescriptor svd{};
        svd.format = std_.format;
        svd.dimension = WGPUTextureViewDimension_2DArray;
        svd.mipLevelCount = 1;
        svd.arrayLayerCount = kCascades;
        svd.aspect = WGPUTextureAspect_DepthOnly;
        svd.usage = std_.usage;
        shadow_view = wgpuTextureCreateView(shadow_texture, &svd);
        for (std::uint32_t c = 0; c < kCascades; ++c) {
            svd.dimension = WGPUTextureViewDimension_2D;
            svd.baseArrayLayer = c;
            svd.arrayLayerCount = 1;
            cascade_view[c] = wgpuTextureCreateView(shadow_texture, &svd);
        }
        cascade_buffer = device->create_buffer("pocket.cascades", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, 256ull * kCascades);
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
        WGPUBindGroupEntry sbe[6]{};
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
        WGPUBindGroupDescriptor sbd{};
        sbd.label = rhi::str("pocket.scene");
        sbd.layout = scene_bgl;
        sbd.entryCount = 6;
        sbd.entries = sbe;
        scene_bg = wgpuDeviceCreateBindGroup(device->device(), &sbd);

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
        WGPUBindGroupEntry entries[5]{};
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
        WGPUBindGroupDescriptor bd{};
        bd.label = rhi::str("pocket.material");
        bd.layout = material_bgl;
        bd.entryCount = 5;
        bd.entries = entries;
        WGPUBindGroup bg = wgpuDeviceCreateBindGroup(device->device(), &bd);
        material_groups[key] = bg;
        return bg;
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

    // A glTF mesh by project path, uploaded on first use; null when unavailable.
    AssetMesh* asset_mesh(const std::string& path) {
        if (auto it = asset_meshes.find(path); it != asset_meshes.end()) return &it->second;
        if (!assets) { report_missing(path, "no asset store"); return nullptr; }
        if (failed.contains(path)) { note_missing(path); return nullptr; }
        auto m = assets->mesh(path);
        if (!m) {
            report_missing(path, m.error().message);
            return nullptr;
        }
        const assets::Mesh& src = **m;
        std::vector<Vertex> verts;
        verts.reserve(src.vertices.size());
        for (const auto& v : src.vertices) verts.push_back({v.position, v.normal, v.uv});
        AssetMesh am;
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
        return -1;
    }

    CameraView find_camera(const world::World& w, float aspect) {
        CameraView cv;
        world::EntityId cam_id = 0;
        world::Camera cam;
        world::WorldTransform ct;
        w.ecs().each([&](flecs::entity e, const world::Camera& c, const world::WorldTransform& t) {
            if (cam_id == 0 && c.active) {
                cam_id = e.id();
                cam = c;
                ct = t;
            }
        });
        stats.has_camera = cam_id != 0;
        stats.camera = cam_id;
        if (cam_id == 0) {
            ct.position = {0, 3, 8};
            Vec3 target{0, 0, 0};
            cv.view = Mat4::look_at(ct.position, target, {0, 1, 0});
            cv.proj = Mat4::perspective(radians(60), aspect, 0.1f, 1000.0f);
            cv.position = ct.position;
            cv.forward = normalize(target - ct.position);
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
        return cv;
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

Renderer::ImageView Renderer::image_view(const std::string& path) {
    Impl& im = *impl_;
    if (path.empty() || !im.assets) return {};
    if (im.failed.contains(path)) { im.note_missing(path); return {}; }
    auto img = im.assets->image(path);
    if (!img) {
        im.report_missing(path, img.error().message);
        return {};
    }
    return {im.view_for(path, im.white), (*img)->width, (*img)->height};
}

Status Renderer::render(rhi::Frame& frame, const world::World& world, rhi::Color clear, const Particles* particles, const Animation* animation, const DebugDraw* debug) {
    Impl& im = *impl_;
    if (im.msaa != im.msaa_applied) POCKET_TRY_VOID(im.create_scene_pipelines(im.msaa));
    POCKET_TRY_VOID(im.ensure_id_target(frame.width, frame.height));
    POCKET_TRY_VOID(im.ensure_hdr_target(frame.width, frame.height));
    POCKET_TRY_VOID(im.ensure_msaa_targets(frame.width, frame.height, im.msaa_applied));
    im.last_width = frame.width;
    im.last_height = frame.height;
    // Clamp the requested viewport to the frame; an empty request means the whole frame.
    Viewport vp = im.viewport;
    if (vp.w == 0 || vp.h == 0) vp = Viewport{0, 0, frame.width, frame.height};
    std::int32_t x0 = std::clamp(vp.x, 0, static_cast<std::int32_t>(frame.width));
    std::int32_t y0 = std::clamp(vp.y, 0, static_cast<std::int32_t>(frame.height));
    std::int32_t x1 = std::clamp(vp.x + static_cast<std::int32_t>(vp.w), 0, static_cast<std::int32_t>(frame.width));
    std::int32_t y1 = std::clamp(vp.y + static_cast<std::int32_t>(vp.h), 0, static_cast<std::int32_t>(frame.height));
    if (x1 <= x0 || y1 <= y0) { x0 = 0; y0 = 0; x1 = static_cast<std::int32_t>(frame.width); y1 = static_cast<std::int32_t>(frame.height); }
    im.applied = Viewport{x0, y0, static_cast<std::uint32_t>(x1 - x0), static_cast<std::uint32_t>(y1 - y0)};
    float aspect = im.applied.h > 0 ? static_cast<float>(im.applied.w) / static_cast<float>(im.applied.h) : 1.0f;
    im.stats = RenderStats{};
    im.camera = im.find_camera(world, aspect);

    FrameUniforms fu{};
    to_array(im.camera.proj * im.camera.view, fu.view_proj);
    fu.camera_pos[0] = im.camera.position.x;
    fu.camera_pos[1] = im.camera.position.y;
    fu.camera_pos[2] = im.camera.position.z;
    fu.ambient[0] = decode(0.18f); fu.ambient[1] = decode(0.19f); fu.ambient[2] = decode(0.22f);
    fu.sun_dir[0] = 0.3f; fu.sun_dir[1] = -1.0f; fu.sun_dir[2] = -0.4f;
    fu.sun_color[0] = 0; fu.sun_color[1] = 0; fu.sun_color[2] = 0;
    bool have_sun = false;
    std::uint32_t points = 0;
    world.ecs().each([&](flecs::entity, const world::Light& l, const world::WorldTransform& t) {
        if (l.kind == 0) {
            if (have_sun) return;
            have_sun = true;
            Vec3 dir = t.rotation.rotate({0, 0, -1});
            fu.sun_dir[0] = dir.x; fu.sun_dir[1] = dir.y; fu.sun_dir[2] = dir.z;
            fu.sun_color[0] = decode(l.color.r) * l.intensity;
            fu.sun_color[1] = decode(l.color.g) * l.intensity;
            fu.sun_color[2] = decode(l.color.b) * l.intensity;
        } else if (points < kMaxPointLights) {
            fu.point_pos[points][0] = t.position.x;
            fu.point_pos[points][1] = t.position.y;
            fu.point_pos[points][2] = t.position.z;
            fu.point_pos[points][3] = l.range > 0 ? l.range : 0.001f;
            fu.point_color[points][0] = decode(l.color.r) * l.intensity;
            fu.point_color[points][1] = decode(l.color.g) * l.intensity;
            fu.point_color[points][2] = decode(l.color.b) * l.intensity;
            ++points;
        }
    });
    if (!have_sun) {
        // Default key light so a scene without lights is still visible.
        fu.sun_color[0] = decode(0.9f); fu.sun_color[1] = decode(0.88f); fu.sun_color[2] = decode(0.85f);
    }
    fu.point_count[0] = points;
    im.stats.has_sun = have_sun;
    im.stats.point_lights = points;
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
    }
    im.stats.shadows = shadows_on;
    // The sky: the first enabled Sky; its environment is rebuilt when anything it is made of changed.
    world::Sky sky;
    bool has_sky = false;
    world.ecs().each([&](flecs::entity, const world::Sky& s) {
        if (!has_sky && s.enabled && (s.mode == 1 || s.mode == 2)) { sky = s; has_sky = true; }
    });
    bool env_on = false;
    if (has_sky) {
        const Vec3 toward = normalize(Vec3{-fu.sun_dir[0], -fu.sun_dir[1], -fu.sun_dir[2]});
        env_on = im.update_environment(frame, sky, have_sun, toward, Vec3{fu.sun_color[0], fu.sun_color[1], fu.sun_color[2]});
        if (!env_on) has_sky = false;
    }
    to_array((im.camera.proj * im.camera.view).inverse(), fu.inv_view_proj);
    fu.env[0] = env_on && (sky.diffuse > 0 || sky.specular > 0) ? 1.0f : 0.0f;
    fu.env[1] = std::max(sky.diffuse, 0.0f);
    fu.env[2] = std::max(sky.specular, 0.0f);
    fu.env[3] = static_cast<float>(kEnvLevels - 1);
    fu.sky[0] = has_sky ? static_cast<float>(sky.mode) : 0.0f;
    fu.sky[1] = std::cos(radians(std::clamp(sky.sun_size, 0.0f, 30.0f) * 0.5f));
    fu.sky[2] = sky.sun_size > 0 ? 40.0f : 0.0f;
    fu.sky[3] = have_sun ? 1.0f : 0.0f;
    im.stats.sky = has_sky ? sky.mode : 0;
    if (has_sky) fu.ambient[0] = fu.ambient[1] = fu.ambient[2] = 0.0f;   // the sky's light replaces the flat ambient, even at zero
    im.device->write_buffer(im.frame_buffer, 0, &fu, sizeof fu);

    // Gather one object per primitive entity and one per material of a glTF entity, sorted by
    // texture, mesh and submesh so equal runs become single instanced draws and the order is
    // deterministic.
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
    };
    std::vector<Draw> draws;
    std::uint32_t count = 0;
    std::uint32_t entities = 0;
    std::uint32_t skinned_instances = 0;
    std::uint32_t morphed_instances = 0;
    std::uint32_t translucent_instances = 0;
    std::uint32_t moving_parts = 0;
    im.animation = animation;
    im.joint_count = 0;
    world.ecs().each([&](flecs::entity e, const world::MeshRenderer& mr, const world::WorldTransform& t) {
        if (!mr.visible || count >= kMaxObjects) return;
        ++entities;
        Mat4 model = Mat4::trs(t.position, t.rotation, t.scale);
        ObjectUniforms ou{};
        to_array(model, ou.model);
        to_array(transpose(model.inverse_affine()), ou.normal);
        ou.id[0] = static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu);
        // Material inputs: the asset's material, overridden per entity by the MeshRenderer.
        auto push = [&](const GpuMesh* gpu, std::uint32_t first, std::uint32_t n, const std::string& tex, Vec4 color, const std::string& mesh_key, const assets::Material* mat, bool skinned = false) {
            if (count >= kMaxObjects) return;
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
            ou.emissive[3] = mr.cutoff > 0 ? mr.cutoff : (mat ? mat->alpha_cutoff : 0.0f);
            WGPUBindGroup group = im.material_for(tex, mr_map, normal_map, em_map, false);
            const bool blend = color.w < 0.999f || (mat && mat->blend);
            const Vec3 to_cam = t.position - im.camera.position;
            const float depth = to_cam.x * im.camera.forward.x + to_cam.y * im.camera.forward.y + to_cam.z * im.camera.forward.z;
            draws.push_back({tex + "|" + normal_map + "|" + mr_map, mesh_key, gpu, first, n, group, ou, skinned, blend, depth});
            if (blend) ++translucent_instances;
            ++count;
        };
        ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
        ou.morph[0] = ou.morph[1] = ou.morph[2] = ou.morph[3] = 0;
        for (float& mw : ou.morph_weights) mw = 0;
        int kind = Impl::primitive_index(mr.mesh);
        if (kind >= 0) {
            const GpuMesh& gm = im.meshes[static_cast<std::size_t>(kind)];
            push(&gm, 0, gm.index_count, mr.texture, {decode(mr.color.r), decode(mr.color.g), decode(mr.color.b), mr.color.a}, mr.mesh, nullptr);
            return;
        }
        Impl::AssetMesh* am = im.asset_mesh(mr.mesh);
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
                im.report_missing(mr.mesh + "#" + mr.node, "no such node in the file");
                return;
            }
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
            push(&am->gpu, sm.first_index, sm.index_count, mr.texture.empty() ? mat.texture : mr.texture, color, mr.mesh, &mat, skinned);
            if (part || alone) {
                to_array(model, ou.model);
                to_array(transpose(model.inverse_affine()), ou.normal);
            }
        }
        ou.id[2] = 0;
    });
    std::stable_sort(draws.begin(), draws.end(), [](const Draw& a, const Draw& b) {
        if (a.blend != b.blend) return !a.blend;            // opaque first
        if (a.blend) return a.depth > b.depth;              // translucent far to near
        if (a.skinned != b.skinned) return !a.skinned;
        if (a.texture != b.texture) return a.texture < b.texture;
        if (a.mesh != b.mesh) return a.mesh < b.mesh;
        return a.first < b.first;
    });
    if (im.joint_count > 0) im.device->write_buffer(im.joint_buffer, 0, im.joint_staging.data(), static_cast<std::uint64_t>(im.joint_count) * sizeof(float) * 16);
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
                sprites.push_back({part.texture, im.texture_for(part.texture, true), tmc.order, depth, ou, &lm->gpu, part.first, part.count, 2 * this_index + 1});
            }
            for (const auto& part : lm->anim_parts) {
                if (count + sprites.size() >= kMaxObjects) return;
                sprites.push_back({part.texture, im.texture_for(part.texture, true), tmc.order, depth, ou, &lm->anim, part.first, part.count, 2 * this_index + 1});
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
                        sprites.push_back({il.image, material, tmc.order, depth, iu, nullptr, 0, 0, 2 * il.before});
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
        sprites.push_back({sp.texture, im.texture_for(sp.texture, sp.filter == "nearest"), sp.layer, sp.sort_y ? t.position.y : depth, ou});
    });
    // Particles: one unlit quad each, facing the camera (or flat in XY), sized and tinted by age.
    std::uint32_t particle_count = 0;
    if (particles) {
        const Mat4& view = im.camera.view;
        const Vec3 right{view.at(0, 0), view.at(1, 0), view.at(2, 0)};
        const Vec3 up{view.at(0, 1), view.at(1, 1), view.at(2, 1)};
        for (const auto& [id, pool] : particles->pools()) {
            const auto* e = world.try_get<world::ParticleEmitter>(id);
            if (!e || pool.alive.empty()) continue;
            const auto* wt = world.try_get<world::WorldTransform>(id);
            const Vec3 origin = (!e->world_space && wt) ? wt->position : Vec3{0, 0, 0};
            WGPUBindGroup material = im.texture_for(e->texture, false);
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
                sprites.push_back({e->texture, material, e->layer, depth, ou});
                ++particle_count;
            }
        }
    }
    std::stable_sort(sprites.begin(), sprites.end(), [](const SpriteDraw& a, const SpriteDraw& b) {
        if (a.layer != b.layer) return a.layer < b.layer;
        if (a.depth != b.depth) return a.depth > b.depth;
        if (a.sub != b.sub) return a.sub < b.sub;
        return a.texture < b.texture;
    });
    for (std::size_t i = 0; i < sprites.size(); ++i) std::memcpy(im.object_staging.data() + (static_cast<std::size_t>(count) + i) * kObjectStride, &sprites[i].object, sizeof(ObjectUniforms));
    std::uint32_t total = count + static_cast<std::uint32_t>(sprites.size());
    if (total > 0) im.device->write_buffer(im.object_buffer, 0, im.object_staging.data(), static_cast<std::uint64_t>(total) * kObjectStride);
    im.stats.meshes = entities;
    im.stats.instances = total;
    im.stats.sprites = static_cast<std::uint32_t>(sprites.size()) - particle_count - tile_layers_parts(sprites) - image_quads;
    im.stats.particles = particle_count;
    im.stats.tile_layers = tile_layers;
    im.stats.image_layers = image_layers;
    im.stats.translucent = translucent_instances;
    im.stats.tile_rebuilds = im.tile_rebuilds;
    im.stats.tile_frames = im.tile_frames;
    im.stats.msaa = im.msaa_applied;
    im.stats.bloom = false;
    im.stats.grade = false;
    im.stats.tonemap = 0;
    im.stats.auto_exposure = false;
    im.stats.env_updates = im.env_updates;
    im.stats.skinned = skinned_instances;
    im.stats.morphed = morphed_instances;
    im.stats.moving_parts = moving_parts;
    im.stats.asset_meshes = static_cast<std::uint32_t>(im.asset_meshes.size());
    im.stats.textures = static_cast<std::uint32_t>(im.textures.size());
    im.stats.materials = static_cast<std::uint32_t>(im.material_groups.size());

    const bool split = im.msaa_applied > 1;  // color resolves from the multisampled target; ids get their own pass
    WGPURenderPassColorAttachment ca[2]{};
    ca[0].view = split ? im.ms_color_view : im.hdr_view;
    ca[0].resolveTarget = split ? im.hdr_view : nullptr;
    ca[0].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca[0].loadOp = WGPULoadOp_Clear;
    ca[0].storeOp = split ? WGPUStoreOp_Discard : WGPUStoreOp_Store;
    ca[0].clearValue = {decode(clear.r), decode(clear.g), decode(clear.b), clear.a};
    ca[1].view = im.id_view;
    ca[1].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca[1].loadOp = WGPULoadOp_Clear;
    ca[1].storeOp = WGPUStoreOp_Store;
    ca[1].clearValue = {0, 0, 0, 0};
    WGPURenderPassDepthStencilAttachment ds{};
    ds.view = split ? im.ms_depth_view : frame.depth;
    ds.depthLoadOp = WGPULoadOp_Clear;
    ds.depthStoreOp = WGPUStoreOp_Store;
    ds.depthClearValue = 1.0f;
    ds.stencilLoadOp = WGPULoadOp_Undefined;
    ds.stencilStoreOp = WGPUStoreOp_Undefined;
    ds.stencilReadOnly = true;
    // Instanced runs of equal mesh, submesh and material; the same loop serves both passes.
    // The blend pipelines are given for the color pass only: the shadow and id passes draw a
    // translucent mesh like any other (it casts a shadow and is picked).
    auto draw_runs = [&](WGPURenderPassEncoder pass, bool with_materials, std::uint32_t& counter, WGPURenderPipeline plain, WGPURenderPipeline skinned, WGPURenderPipeline blend_plain = nullptr, WGPURenderPipeline blend_skinned = nullptr) {
        const GpuMesh* current_mesh = nullptr;
        WGPUBindGroup current_material = nullptr;
        bool current_skinned = false, current_blend = false;
        wgpuRenderPassEncoderSetPipeline(pass, plain);
        std::size_t i = 0;
        while (i < draws.size()) {
            const Draw& d = draws[i];
            const bool blend = d.blend && blend_plain != nullptr;
            std::size_t run = 1;
            while (i + run < draws.size()) {
                const Draw& n = draws[i + run];
                if (n.gpu != d.gpu || n.first != d.first || n.count != d.count || n.material != d.material || n.skinned != d.skinned || n.blend != d.blend) break;
                if (blend && n.depth != d.depth) break;   // translucent instances keep their far-to-near order
                ++run;
            }
            if (d.skinned != current_skinned || blend != current_blend) {
                wgpuRenderPassEncoderSetPipeline(pass, blend ? (d.skinned ? blend_skinned : blend_plain) : (d.skinned ? skinned : plain));
                current_skinned = d.skinned;
                current_blend = blend;
                current_mesh = nullptr;
            }
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
            WGPURenderPassEncoder spass = wgpuCommandEncoderBeginRenderPass(frame.encoder, &srp);
            wgpuRenderPassEncoderSetBindGroup(spass, 0, im.frame_bg, 0, nullptr);
            wgpuRenderPassEncoderSetBindGroup(spass, 1, im.object_bg, 0, nullptr);
            const std::uint32_t offset = 256u * static_cast<std::uint32_t>(c);
            wgpuRenderPassEncoderSetBindGroup(spass, 2, im.cascade_bg, 1, &offset);
            draw_runs(spass, false, im.stats.shadow_draws, im.shadow_pipeline, im.shadow_skinned_pipeline);
            wgpuRenderPassEncoderEnd(spass);
            wgpuRenderPassEncoderRelease(spass);
        }
    }
    // Sprites and tile layers in draw order; the same loop serves the color pass and the id pass.
    auto draw_sprites = [&](WGPURenderPassEncoder pass, WGPURenderPipeline pipe, std::uint32_t& counter) {
        const GpuMesh& quad = im.meshes[static_cast<std::size_t>(Primitive::Quad)];
        wgpuRenderPassEncoderSetPipeline(pass, pipe);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        wgpuRenderPassEncoderSetVertexBuffer(pass, 0, quad.vertices, 0, WGPU_WHOLE_SIZE);
        wgpuRenderPassEncoderSetIndexBuffer(pass, quad.indices, WGPUIndexFormat_Uint32, 0, WGPU_WHOLE_SIZE);
        const GpuMesh* bound = &quad;
        std::size_t i = 0;
        while (i < sprites.size()) {
            const SpriteDraw& s = sprites[i];
            if (s.mesh) {
                // A tile layer: its own buffers, one draw, then back to the quad for sprites.
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
            std::size_t run = 1;
            while (i + run < sprites.size() && !sprites[i + run].mesh && sprites[i + run].material == sprites[i].material) ++run;
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
    if (split) {
        // The id pass first: single-sample ids and the frame's own depth buffer, no lines.
        WGPURenderPassColorAttachment ica{};
        ica.view = im.id_view;
        ica.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
        ica.loadOp = WGPULoadOp_Clear;
        ica.storeOp = WGPUStoreOp_Store;
        ica.clearValue = {0, 0, 0, 0};
        WGPURenderPassDepthStencilAttachment ids = ds;
        ids.view = frame.depth;
        WGPURenderPassDescriptor irp{};
        irp.label = rhi::str("pocket.ids");
        irp.colorAttachmentCount = 1;
        irp.colorAttachments = &ica;
        irp.depthStencilAttachment = &ids;
        WGPURenderPassEncoder ipass = wgpuCommandEncoderBeginRenderPass(frame.encoder, &irp);
        set_viewport(ipass);
        if (!draws.empty()) {
            wgpuRenderPassEncoderSetBindGroup(ipass, 0, im.scene_bg, 0, nullptr);
            wgpuRenderPassEncoderSetBindGroup(ipass, 1, im.object_bg, 0, nullptr);
            draw_runs(ipass, true, im.stats.id_draws, im.id_pipeline, im.id_skinned_pipeline);
        }
        if (!sprites.empty()) draw_sprites(ipass, im.id_sprite_pipeline, im.stats.id_draws);
        wgpuRenderPassEncoderEnd(ipass);
        wgpuRenderPassEncoderRelease(ipass);
    }
    WGPURenderPassDescriptor rp{};
    rp.label = rhi::str("pocket.scene");
    rp.colorAttachmentCount = split ? 1 : 2;
    rp.colorAttachments = ca;
    rp.depthStencilAttachment = &ds;
    WGPURenderPassEncoder pass = wgpuCommandEncoderBeginRenderPass(frame.encoder, &rp);
    set_viewport(pass);
    if (im.stats.sky != 0) {
        wgpuRenderPassEncoderSetPipeline(pass, im.sky_pipeline);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderDraw(pass, 3, 1, 0, 0);
        im.stats.draw_calls++;
    }
    if (!draws.empty()) {
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        draw_runs(pass, true, im.stats.draw_calls, im.pipeline, im.skinned_pipeline, im.blend_pipeline, im.blend_skinned_pipeline);
    }
    if (!sprites.empty()) draw_sprites(pass, im.sprite_pipeline, im.stats.draw_calls);
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
    if (im.bloom.enabled && im.bloom.strength > 0) POCKET_TRY_VOID(im.draw_bloom(frame));
    POCKET_TRY_VOID(im.draw_post(frame, clear));
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
    if (x >= img.width || y >= img.height) return fail("bad_args", "pixel ({}, {}) outside {}x{}", x, y, img.width, img.height);
    return static_cast<world::EntityId>(img.ids[static_cast<std::size_t>(y) * img.width + x]);
}

const RenderStats& Renderer::stats() const { return impl_->stats; }
const CameraView& Renderer::camera() const { return impl_->camera; }

bool Renderer::unproject(float px, float py, Vec3& origin, Vec3& direction) const {
    const Impl& im = *impl_;
    if (im.applied.w == 0 || im.applied.h == 0) return false;
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
    out_x = static_cast<float>(im.applied.x) + (nx * 0.5f + 0.5f) * static_cast<float>(im.applied.w);
    out_y = static_cast<float>(im.applied.y) + (1.0f - (ny * 0.5f + 0.5f)) * static_cast<float>(im.applied.h);
    return true;
}

void Renderer::set_viewport(Viewport v) { impl_->viewport = v; }
void Renderer::set_msaa(int samples) { impl_->msaa = samples > 1 ? 4 : 1; }  // WebGPU multisamples at 1 or 4
int Renderer::msaa() const { return impl_->msaa; }
void Renderer::set_shadows(ShadowSettings s) {
    s.cascades = std::clamp(s.cascades, 1, static_cast<int>(kCascades));
    s.distance = std::max(s.distance, 1.0f);
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
Viewport Renderer::applied_viewport() const { return impl_->applied; }

Json Renderer::describe() const {
    const RenderStats& s = impl_->stats;
    Json j;
    j["draw_calls"] = s.draw_calls;
    j["shadow_draws"] = s.shadow_draws;
    j["shadows"] = s.shadows;
    j["shadow_cascades"] = s.shadow_cascades;
    j["shadow_distance"] = s.shadow_distance;
    j["instances"] = s.instances;
    j["translucent"] = s.translucent;
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
    j["auto_exposure"] = s.auto_exposure;
    j["sky"] = s.sky == 1 ? "procedural" : s.sky == 2 ? "image" : "none";
    j["env_updates"] = s.env_updates;
    j["id_draws"] = s.id_draws;
    j["debug_lines"] = s.debug_lines;
    j["materials"] = s.materials;
    j["meshes"] = s.meshes;
    j["point_lights"] = s.point_lights;
    j["has_camera"] = s.has_camera;
    j["has_sun"] = s.has_sun;
    if (s.camera) j["camera"] = s.camera;
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
