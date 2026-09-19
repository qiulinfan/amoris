#include <pocket/renderer/renderer.hpp>

#include <pocket/core/log.hpp>
#include <pocket/renderer/primitives.hpp>

#ifndef __EMSCRIPTEN__
#include <webgpu/wgpu.h>
#endif

#include <algorithm>
#include <array>
#include <cstring>
#include <map>
#include <set>

namespace pocket::renderer {

namespace {

constexpr std::uint32_t kMaxPointLights = 8;
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
};
constexpr std::uint32_t kShadowMapSize = 2048;

struct alignas(16) ObjectUniforms {
    float model[16];
    float normal[16];
    float color[4];
    std::uint32_t id[4];      // x: entity id, y: flags (1 = unlit)
    float uv_rect[4];         // u0, v0, u1, v1 (sprites cut a sheet; meshes use 0,0,1,1)
    float pbr[4];             // metallic, roughness, normal scale, 1 when a normal map is bound
    float emissive[4];        // linear RGB added after lighting, w unused
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
    out.color = color;
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
};
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_samp: sampler_comparison;
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

// Shadow pass: depth only, from the sun.
@vertex fn vs_shadow(@builtin(instance_index) instance: u32, @builtin(vertex_index) vid: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f) -> @builtin(position) vec4f {
    let object = objects[instance];
    return frame.light_view_proj * (object.model * vec4f(morph_position(object, vid, position), 1.0));
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
    return frame.light_view_proj * (model * vec4f(morph_position(object, vid, position), 1.0));
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
    // Directional light.
    let l = normalize(-frame.sun_dir.xyz);
    let ndl = max(dot(n, l), 0.0);
    // Shadow map lookup with a 3x3 comparison filter; slope-scaled bias against acne.
    var shadow = 1.0;
    if (frame.shadow.w > 0.5) {
        let sp = frame.light_view_proj * vec4f(in.world_pos, 1.0);
        let ndc = sp.xyz / sp.w;
        let suv = vec2f(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        if (suv.x >= 0.0 && suv.x <= 1.0 && suv.y >= 0.0 && suv.y <= 1.0 && ndc.z >= 0.0 && ndc.z <= 1.0) {
            let bias = frame.shadow.y * (1.0 + 2.0 * (1.0 - ndl));
            var lit = 0.0;
            for (var j = -1; j <= 1; j = j + 1) {
                for (var i = -1; i <= 1; i = i + 1) {
                    lit = lit + textureSampleCompareLevel(shadow_map, shadow_samp, suv + vec2f(f32(i), f32(j)) * frame.shadow.x, ndc.z - bias);
                }
            }
            shadow = mix(1.0 - frame.shadow.z, 1.0, lit / 9.0);
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
    return in.id;
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
    WGPUTextureView shadow_view = nullptr;
    WGPUSampler shadow_sampler = nullptr;
    ShadowSettings shadows;
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
        WGPUTextureView view = nullptr;
    };
    GpuTexture white;
    GpuTexture flat_normal;   // (0.5, 0.5, 1): no bend
    std::map<std::string, GpuTexture> textures;
    std::map<std::string, WGPUBindGroup> material_groups;  // "base|mr|normal|emissive|filter" -> group 2
    struct AssetMesh {
        GpuMesh gpu;
        std::vector<assets::Submesh> submeshes;
        std::vector<Mat4> rest;   // per submesh: where its node rests, for a moving part drawn without a pose
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
        if (shadow_texture) wgpuTextureRelease(shadow_texture);
        if (shadow_sampler) wgpuSamplerRelease(shadow_sampler);
        if (sprite_pipeline) wgpuRenderPipelineRelease(sprite_pipeline);
        if (line_pipeline) wgpuRenderPipelineRelease(line_pipeline);
        if (line_layout) wgpuPipelineLayoutRelease(line_layout);
        if (line_shader) wgpuShaderModuleRelease(line_shader);
        if (line_buffer) wgpuBufferRelease(line_buffer);
        if (pipeline) wgpuRenderPipelineRelease(pipeline);
        if (skinned_pipeline) wgpuRenderPipelineRelease(skinned_pipeline);
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
        POCKET_TRY_VOID(make("pocket.msaa.color", device->color_format(), ms_color, ms_color_view));
        POCKET_TRY_VOID(make("pocket.msaa.depth", device->depth_format(), ms_depth, ms_depth_view));
        ms_width = w;
        ms_height = h;
        ms_samples = samples;
        return {};
    }

    void release_scene_pipelines() {
        for (WGPURenderPipeline* p : {&pipeline, &skinned_pipeline, &sprite_pipeline, &line_pipeline, &id_pipeline, &id_skinned_pipeline, &id_sprite_pipeline}) {
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
        targets[0].format = device->color_format();
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
        WGPUBindGroupLayoutEntry se[3]{};
        se[0] = fe;
        se[1].binding = 1;
        se[1].visibility = WGPUShaderStage_Fragment;
        se[1].texture.sampleType = WGPUTextureSampleType_Depth;
        se[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        se[2].binding = 2;
        se[2].visibility = WGPUShaderStage_Fragment;
        se[2].sampler.type = WGPUSamplerBindingType_Comparison;
        WGPUBindGroupLayoutDescriptor scene_ld{};
        scene_ld.label = rhi::str("pocket.scene");
        scene_ld.entryCount = 3;
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
        WGPUBindGroupLayout sbgls[2] = {frame_bgl, object_bgl};
        WGPUPipelineLayoutDescriptor spld{};
        spld.label = rhi::str("pocket.shadow");
        spld.bindGroupLayoutCount = 2;
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
        std_.size = {kShadowMapSize, kShadowMapSize, 1};
        std_.format = WGPUTextureFormat_Depth32Float;
        std_.mipLevelCount = 1;
        std_.sampleCount = 1;
        shadow_texture = wgpuDeviceCreateTexture(device->device(), &std_);
        if (!shadow_texture) return fail("gpu_texture_failed", "cannot create the shadow map");
        WGPUTextureViewDescriptor svd{};
        svd.format = std_.format;
        svd.dimension = WGPUTextureViewDimension_2D;
        svd.mipLevelCount = 1;
        svd.arrayLayerCount = 1;
        svd.aspect = WGPUTextureAspect_DepthOnly;
        svd.usage = std_.usage;
        shadow_view = wgpuTextureCreateView(shadow_texture, &svd);
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
        WGPUBindGroupEntry sbe[3]{};
        sbe[0] = fbe;
        sbe[1].binding = 1;
        sbe[1].textureView = shadow_view;
        sbe[2].binding = 2;
        sbe[2].sampler = shadow_sampler;
        WGPUBindGroupDescriptor sbd{};
        sbd.label = rhi::str("pocket.scene");
        sbd.layout = scene_bgl;
        sbd.entryCount = 3;
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
        sd.maxAnisotropy = 1;
        sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        sd.label = rhi::str("pocket.material.nearest");
        sd.magFilter = WGPUFilterMode_Nearest;
        sd.minFilter = WGPUFilterMode_Nearest;
        sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
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

    Result<GpuTexture> upload_texture(const char* label, std::uint32_t w, std::uint32_t h, const std::uint8_t* rgba) {
        GpuTexture t;
        WGPUTextureDescriptor td{};
        td.label = rhi::str(label);
        td.usage = WGPUTextureUsage_TextureBinding | WGPUTextureUsage_CopyDst;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {w, h, 1};
        td.format = WGPUTextureFormat_RGBA8Unorm;
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
        WGPUTexelCopyTextureInfo dst{};
        dst.texture = t.texture;
        dst.origin = {0, 0, 0};
        dst.aspect = WGPUTextureAspect_All;
        WGPUTexelCopyBufferLayout layout{};
        layout.offset = 0;
        layout.bytesPerRow = w * 4;
        layout.rowsPerImage = h;
        WGPUExtent3D ext{w, h, 1};
        wgpuQueueWriteTexture(device->queue(), &dst, rgba, static_cast<std::size_t>(w) * h * 4, &layout, &ext);
        return t;
    }

    // Group 2 for a set of maps: base color, metallic-roughness, normal, emissive (empty paths take
    // the white or flat-normal defaults), with the linear or nearest sampler. Cached by key.
    WGPUBindGroup material_for(const std::string& base, const std::string& mr, const std::string& normal, const std::string& emissive, bool nearest) {
        std::string key = base + "|" + mr + "|" + normal + "|" + emissive + (nearest ? "|n" : "|l");
        if (auto it = material_groups.find(key); it != material_groups.end()) return it->second;
        WGPUBindGroupEntry entries[5]{};
        entries[0].binding = 0;
        entries[0].textureView = view_for(base, white);
        entries[1].binding = 1;
        entries[1].sampler = nearest ? nearest_sampler : sampler;
        entries[2].binding = 2;
        entries[2].textureView = view_for(mr, white);
        entries[3].binding = 3;
        entries[3].textureView = view_for(normal, flat_normal);
        entries[4].binding = 4;
        entries[4].textureView = view_for(emissive, white);
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
    WGPUTextureView view_for(const std::string& path, const GpuTexture& fallback) {
        if (path.empty()) return fallback.view;
        if (auto it = textures.find(path); it != textures.end()) return it->second.view;
        if (!assets) { report_missing(path, "no asset store"); return fallback.view; }
        if (failed.contains(path)) { note_missing(path); return fallback.view; }
        auto img = assets->image(path);
        if (!img) {
            report_missing(path, img.error().message);
            return fallback.view;
        }
        auto t = upload_texture(path.c_str(), (*img)->width, (*img)->height, (*img)->rgba.data());
        if (!t) {
            report_missing(path, t.error().message);
            return fallback.view;
        }
        textures[path] = *t;
        return t->view;
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
        for (const assets::Submesh& sm : src.submeshes) am.rest.push_back(src.rest_global(sm.node));
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

Status Renderer::render(rhi::Frame& frame, const world::World& world, rhi::Color clear, const Particles* particles, const Animation* animation, const DebugDraw* debug) {
    Impl& im = *impl_;
    if (im.msaa != im.msaa_applied) POCKET_TRY_VOID(im.create_scene_pipelines(im.msaa));
    POCKET_TRY_VOID(im.ensure_id_target(frame.width, frame.height));
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
    fu.ambient[0] = 0.18f; fu.ambient[1] = 0.19f; fu.ambient[2] = 0.22f;
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
            fu.sun_color[0] = l.color.r * l.intensity;
            fu.sun_color[1] = l.color.g * l.intensity;
            fu.sun_color[2] = l.color.b * l.intensity;
        } else if (points < kMaxPointLights) {
            fu.point_pos[points][0] = t.position.x;
            fu.point_pos[points][1] = t.position.y;
            fu.point_pos[points][2] = t.position.z;
            fu.point_pos[points][3] = l.range > 0 ? l.range : 0.001f;
            fu.point_color[points][0] = l.color.r * l.intensity;
            fu.point_color[points][1] = l.color.g * l.intensity;
            fu.point_color[points][2] = l.color.b * l.intensity;
            ++points;
        }
    });
    if (!have_sun) {
        // Default key light so a scene without lights is still visible.
        fu.sun_color[0] = 0.9f; fu.sun_color[1] = 0.88f; fu.sun_color[2] = 0.85f;
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
        Vec3 center = (lo + hi) * 0.5f;
        Vec3 ext = (hi - lo) * 0.5f;
        float radius = std::max(1.0f, std::sqrt(ext.x * ext.x + ext.y * ext.y + ext.z * ext.z));
        Vec3 dir = normalize(Vec3{fu.sun_dir[0], fu.sun_dir[1], fu.sun_dir[2]});
        Vec3 up = std::abs(dir.y) > 0.99f ? Vec3{1, 0, 0} : Vec3{0, 1, 0};
        Mat4 light_view = Mat4::look_at(center - dir * (radius * 2.0f), center, up);
        Mat4 light_proj = Mat4::orthographic(-radius, radius, -radius, radius, 0.05f, radius * 4.0f);
        to_array(light_proj * light_view, fu.light_view_proj);
        fu.shadow[0] = 1.0f / static_cast<float>(kShadowMapSize);
        fu.shadow[1] = im.shadows.bias;
        fu.shadow[2] = im.shadows.strength;
        fu.shadow[3] = shadows_on ? 1.0f : 0.0f;
    }
    im.stats.shadows = shadows_on;
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
    };
    std::vector<Draw> draws;
    std::uint32_t count = 0;
    std::uint32_t entities = 0;
    std::uint32_t skinned_instances = 0;
    std::uint32_t morphed_instances = 0;
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
            ou.emissive[0] = mr.emissive.r + (mat ? mat->emissive.x : 0.0f);
            ou.emissive[1] = mr.emissive.g + (mat ? mat->emissive.y : 0.0f);
            ou.emissive[2] = mr.emissive.b + (mat ? mat->emissive.z : 0.0f);
            ou.emissive[3] = 0;
            WGPUBindGroup group = im.material_for(tex, mr_map, normal_map, em_map, false);
            draws.push_back({tex + "|" + normal_map + "|" + mr_map, mesh_key, gpu, first, n, group, ou, skinned});
            ++count;
        };
        ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
        ou.morph[0] = ou.morph[1] = ou.morph[2] = ou.morph[3] = 0;
        for (float& mw : ou.morph_weights) mw = 0;
        int kind = Impl::primitive_index(mr.mesh);
        if (kind >= 0) {
            const GpuMesh& gm = im.meshes[static_cast<std::size_t>(kind)];
            push(&gm, 0, gm.index_count, mr.texture, {mr.color.r, mr.color.g, mr.color.b, mr.color.a}, mr.mesh, nullptr);
            return;
        }
        Impl::AssetMesh* am = im.asset_mesh(mr.mesh);
        if (!am) {
            // Missing asset: a magenta cube marks the spot instead of hiding the problem.
            const GpuMesh& gm = im.meshes[0];
            push(&gm, 0, gm.index_count, "", {1, 0, 1, 1}, "cube", nullptr);
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
            const assets::Material& mat = am->materials[std::min<std::size_t>(sm.material, am->materials.size() - 1)];
            Vec4 color{mr.color.r * mat.base_color.x, mr.color.g * mat.base_color.y, mr.color.b * mat.base_color.z, mr.color.a * mat.base_color.w};
            const bool skinned = sm.skin >= 0 && static_cast<std::size_t>(sm.skin) < joint_base.size() && joint_base[static_cast<std::size_t>(sm.skin)] < kMaxJoints;
            ou.id[2] = skinned ? joint_base[static_cast<std::size_t>(sm.skin)] : 0;
            if (skinned) ++skinned_instances;
            if (morphed) ++morphed_instances;
            // A moving part: placed by its node's matrix from the pose (its rest without one).
            const bool part = sm.node >= 0;
            if (part) {
                const Mat4 placed = model * (pose && static_cast<std::size_t>(sm.node) < pose->globals.size() ? pose->globals[static_cast<std::size_t>(sm.node)] : am->rest[si]);
                to_array(placed, ou.model);
                to_array(transpose(placed.inverse_affine()), ou.normal);
                ++moving_parts;
            }
            push(&am->gpu, sm.first_index, sm.index_count, mr.texture.empty() ? mat.texture : mr.texture, color, mr.mesh, &mat, skinned);
            if (part) {
                to_array(model, ou.model);
                to_array(transpose(model.inverse_affine()), ou.normal);
            }
        }
        ou.id[2] = 0;
    });
    std::stable_sort(draws.begin(), draws.end(), [](const Draw& a, const Draw& b) {
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
    std::uint32_t tile_layers = 0;
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
            ou.color[0] = tmc.color.r; ou.color[1] = tmc.color.g; ou.color[2] = tmc.color.b; ou.color[3] = tmc.color.a * layer.opacity;
            for (const auto& part : lm->parts) {
                if (count + sprites.size() >= kMaxObjects) return;
                sprites.push_back({part.texture, im.texture_for(part.texture, true), tmc.order, depth, ou, &lm->gpu, part.first, part.count, this_index});
            }
            for (const auto& part : lm->anim_parts) {
                if (count + sprites.size() >= kMaxObjects) return;
                sprites.push_back({part.texture, im.texture_for(part.texture, true), tmc.order, depth, ou, &lm->anim, part.first, part.count, this_index});
            }
            ++tile_layers;
        }
    });
    world.ecs().each([&](flecs::entity e, const world::Sprite& sp, const world::WorldTransform& t) {
        if (!sp.visible || count + sprites.size() >= kMaxObjects) return;
        Mat4 model = Mat4::trs(t.position, t.rotation, t.scale) * Mat4::translation({(0.5f - sp.anchor.x) * sp.size.x, (0.5f - sp.anchor.y) * sp.size.y, 0}) * Mat4::scale({sp.size.x, sp.size.y, 1});
        ObjectUniforms ou{};
        to_array(model, ou.model);
        to_array(transpose(model.inverse_affine()), ou.normal);
        ou.color[0] = sp.color.r; ou.color[1] = sp.color.g; ou.color[2] = sp.color.b; ou.color[3] = sp.color.a;
        ou.id[0] = static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu);
        ou.id[1] = 1;
        float u0 = sp.uv.x, v0 = sp.uv.y, u1 = sp.uv.z, v1 = sp.uv.w;
        if (sp.flip_x) std::swap(u0, u1);
        if (sp.flip_y) std::swap(v0, v1);
        ou.uv_rect[0] = u0; ou.uv_rect[1] = v0; ou.uv_rect[2] = u1; ou.uv_rect[3] = v1;
        Vec3 d = t.position - im.camera.position;
        float depth = d.x * im.camera.forward.x + d.y * im.camera.forward.y + d.z * im.camera.forward.z;
        sprites.push_back({sp.texture, im.texture_for(sp.texture, sp.filter == "nearest"), sp.layer, depth, ou});
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
                ou.color[0] = e->color.r + (e->color_end.r - e->color.r) * k;
                ou.color[1] = e->color.g + (e->color_end.g - e->color.g) * k;
                ou.color[2] = e->color.b + (e->color_end.b - e->color.b) * k;
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
    im.stats.sprites = static_cast<std::uint32_t>(sprites.size()) - particle_count - tile_layers_parts(sprites);
    im.stats.particles = particle_count;
    im.stats.tile_layers = tile_layers;
    im.stats.tile_rebuilds = im.tile_rebuilds;
    im.stats.tile_frames = im.tile_frames;
    im.stats.msaa = im.msaa_applied;
    im.stats.skinned = skinned_instances;
    im.stats.morphed = morphed_instances;
    im.stats.moving_parts = moving_parts;
    im.stats.asset_meshes = static_cast<std::uint32_t>(im.asset_meshes.size());
    im.stats.textures = static_cast<std::uint32_t>(im.textures.size());
    im.stats.materials = static_cast<std::uint32_t>(im.material_groups.size());

    const bool split = im.msaa_applied > 1;  // color resolves from the multisampled target; ids get their own pass
    WGPURenderPassColorAttachment ca[2]{};
    ca[0].view = split ? im.ms_color_view : frame.color;
    ca[0].resolveTarget = split ? frame.color : nullptr;
    ca[0].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca[0].loadOp = WGPULoadOp_Clear;
    ca[0].storeOp = split ? WGPUStoreOp_Discard : WGPUStoreOp_Store;
    ca[0].clearValue = {clear.r, clear.g, clear.b, clear.a};
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
    auto draw_runs = [&](WGPURenderPassEncoder pass, bool with_materials, std::uint32_t& counter, WGPURenderPipeline plain, WGPURenderPipeline skinned) {
        const GpuMesh* current_mesh = nullptr;
        WGPUBindGroup current_material = nullptr;
        bool current_skinned = false;
        wgpuRenderPassEncoderSetPipeline(pass, plain);
        std::size_t i = 0;
        while (i < draws.size()) {
            const Draw& d = draws[i];
            std::size_t run = 1;
            while (i + run < draws.size()) {
                const Draw& n = draws[i + run];
                if (n.gpu != d.gpu || n.first != d.first || n.count != d.count || n.material != d.material || n.skinned != d.skinned) break;
                ++run;
            }
            if (d.skinned != current_skinned) {
                wgpuRenderPassEncoderSetPipeline(pass, d.skinned ? skinned : plain);
                current_skinned = d.skinned;
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
        WGPURenderPassDepthStencilAttachment sds{};
        sds.view = im.shadow_view;
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
        draw_runs(spass, false, im.stats.shadow_draws, im.shadow_pipeline, im.shadow_skinned_pipeline);
        wgpuRenderPassEncoderEnd(spass);
        wgpuRenderPassEncoderRelease(spass);
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
    if (!draws.empty()) {
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        draw_runs(pass, true, im.stats.draw_calls, im.pipeline, im.skinned_pipeline);
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
void Renderer::set_shadows(ShadowSettings s) { impl_->shadows = s; }
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
    j["instances"] = s.instances;
    j["sprites"] = s.sprites;
    j["particles"] = s.particles;
    j["skinned"] = s.skinned;
    j["morphed"] = s.morphed;
    j["moving_parts"] = s.moving_parts;
    j["tile_layers"] = s.tile_layers;
    j["tile_rebuilds"] = s.tile_rebuilds;
    j["tile_frames"] = s.tile_frames;
    j["msaa"] = s.msaa;
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
