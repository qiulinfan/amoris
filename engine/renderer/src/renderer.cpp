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
constexpr std::uint32_t kObjectStride = 176;  // sizeof(ObjectUniforms)
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
};
static_assert(sizeof(ObjectUniforms) == kObjectStride);

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
};
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<storage, read> objects: array<Object>;
@group(2) @binding(0) var base_tex: texture_2d<f32>;
@group(2) @binding(1) var base_samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) world_pos: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @location(3) color: vec4f,
    @location(4) @interpolate(flat) id: u32,
};

@vertex fn vs(@builtin(instance_index) instance: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f) -> VsOut {
    let object = objects[instance];
    var out: VsOut;
    let world = object.model * vec4f(position, 1.0);
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((object.normal * vec4f(normal, 0.0)).xyz);
    out.uv = mix(object.uv_rect.xy, object.uv_rect.zw, uv);
    out.color = object.color;
    out.id = object.id.x;
    return out;
}

// Shadow pass: depth only, from the sun.
@vertex fn vs_shadow(@builtin(instance_index) instance: u32, @location(0) position: vec3f, @location(1) normal: vec3f, @location(2) uv: vec2f) -> @builtin(position) vec4f {
    let object = objects[instance];
    return frame.light_view_proj * (object.model * vec4f(position, 1.0));
}

struct FsOut {
    @location(0) color: vec4f,
    @location(1) id: u32,
};

@fragment fn fs(in: VsOut) -> FsOut {
    let n = normalize(in.normal);
    let v = normalize(frame.camera_pos.xyz - in.world_pos);
    var light = frame.ambient.rgb;
    // Directional light.
    let l = normalize(-frame.sun_dir.xyz);
    let ndl = max(dot(n, l), 0.0);
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 32.0) * 0.25;
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
    light += frame.sun_color.rgb * (ndl + spec * ndl) * shadow;
    // Point lights.
    for (var i = 0u; i < frame.point_count.x; i = i + 1u) {
        let to_light = frame.point_pos[i].xyz - in.world_pos;
        let dist = length(to_light);
        let range = frame.point_pos[i].w;
        let att = clamp(1.0 - (dist * dist) / (range * range), 0.0, 1.0);
        let pl = to_light / max(dist, 0.0001);
        let pndl = max(dot(n, pl), 0.0);
        light += frame.point_color[i].rgb * pndl * att * att;
    }
    let base = textureSample(base_tex, base_samp, in.uv) * in.color;
    var out: FsOut;
    out.color = vec4f(base.rgb * light, base.a);
    out.id = in.id;
    return out;
}

// Sprites: no lighting, alpha blended, transparent pixels leave no id behind.
@fragment fn fs_unlit(in: VsOut) -> FsOut {
    let base = textureSample(base_tex, base_samp, in.uv) * in.color;
    if (base.a < 0.02) { discard; }
    var out: FsOut;
    out.color = base;
    out.id = in.id;
    return out;
}
)WGSL";

struct GpuMesh {
    WGPUBuffer vertices = nullptr;
    WGPUBuffer indices = nullptr;
    std::uint32_t index_count = 0;
    Vec3 aabb_min, aabb_max;
};

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
    WGPURenderPipeline sprite_pipeline = nullptr;
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
    std::array<GpuMesh, kPrimitiveCount> meshes{};
    // Assets: glTF meshes and images uploaded on first use.
    assets::AssetStore* assets = nullptr;
    WGPUBindGroupLayout material_bgl = nullptr;
    WGPUSampler sampler = nullptr;
    WGPUSampler nearest_sampler = nullptr;   // pixel art: no filtering, no bleeding between sheet tiles
    struct GpuTexture {
        WGPUTexture texture = nullptr;
        WGPUTextureView view = nullptr;
        WGPUBindGroup bind_group = nullptr;
        WGPUBindGroup bind_group_nearest = nullptr;
    };
    GpuTexture white;
    std::map<std::string, GpuTexture> textures;
    struct AssetMesh {
        GpuMesh gpu;
        std::vector<assets::Submesh> submeshes;
        std::vector<assets::Material> materials;
    };
    std::map<std::string, AssetMesh> asset_meshes;
    std::set<std::string> failed;  // asset paths reported once
    std::vector<std::pair<std::string, std::pair<Vec3, Vec3>>> new_bounds;
    RenderStats stats;
    CameraView camera;
    Viewport viewport;   // requested
    Viewport applied;    // used by the last frame
    std::uint32_t last_width = 0, last_height = 0;
    std::vector<std::uint8_t> object_staging;

    void release_texture(GpuTexture& t) {
        if (t.bind_group) wgpuBindGroupRelease(t.bind_group);
        if (t.bind_group_nearest) wgpuBindGroupRelease(t.bind_group_nearest);
        if (t.view) wgpuTextureViewRelease(t.view);
        if (t.texture) wgpuTextureRelease(t.texture);
        t = GpuTexture{};
    }

    void release_assets() {
        for (auto& [path, am] : asset_meshes) {
            if (am.gpu.vertices) wgpuBufferRelease(am.gpu.vertices);
            if (am.gpu.indices) wgpuBufferRelease(am.gpu.indices);
        }
        asset_meshes.clear();
        for (auto& [path, t] : textures) release_texture(t);
        textures.clear();
        failed.clear();
    }

    ~Impl() {
        release_assets();
        release_texture(white);
        if (sampler) wgpuSamplerRelease(sampler);
        if (nearest_sampler) wgpuSamplerRelease(nearest_sampler);
        if (material_bgl) wgpuBindGroupLayoutRelease(material_bgl);
        for (auto& m : meshes) {
            if (m.vertices) wgpuBufferRelease(m.vertices);
            if (m.indices) wgpuBufferRelease(m.indices);
        }
        if (id_view) wgpuTextureViewRelease(id_view);
        if (id_texture) wgpuTextureRelease(id_texture);
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
        if (pipeline) wgpuRenderPipelineRelease(pipeline);
        if (layout) wgpuPipelineLayoutRelease(layout);
        if (object_bgl) wgpuBindGroupLayoutRelease(object_bgl);
        if (frame_bgl) wgpuBindGroupLayoutRelease(frame_bgl);
        if (shader) wgpuShaderModuleRelease(shader);
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

        WGPUBindGroupLayoutEntry oe{};
        oe.binding = 0;
        oe.visibility = WGPUShaderStage_Vertex | WGPUShaderStage_Fragment;
        oe.buffer.type = WGPUBufferBindingType_ReadOnlyStorage;
        oe.buffer.hasDynamicOffset = false;
        oe.buffer.minBindingSize = sizeof(ObjectUniforms);
        WGPUBindGroupLayoutDescriptor od{};
        od.label = rhi::str("pocket.object");
        od.entryCount = 1;
        od.entries = &oe;
        object_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &od);

        WGPUBindGroupLayoutEntry me[2]{};
        me[0].binding = 0;
        me[0].visibility = WGPUShaderStage_Fragment;
        me[0].texture.sampleType = WGPUTextureSampleType_Float;
        me[0].texture.viewDimension = WGPUTextureViewDimension_2D;
        me[1].binding = 1;
        me[1].visibility = WGPUShaderStage_Fragment;
        me[1].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor md{};
        md.label = rhi::str("pocket.material");
        md.entryCount = 2;
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

        WGPUVertexAttribute attrs[3]{};
        attrs[0].format = WGPUVertexFormat_Float32x3;
        attrs[0].offset = 0;
        attrs[0].shaderLocation = 0;
        attrs[1].format = WGPUVertexFormat_Float32x3;
        attrs[1].offset = sizeof(float) * 3;
        attrs[1].shaderLocation = 1;
        attrs[2].format = WGPUVertexFormat_Float32x2;
        attrs[2].offset = sizeof(float) * 6;
        attrs[2].shaderLocation = 2;
        WGPUVertexBufferLayout vbl{};
        vbl.stepMode = WGPUVertexStepMode_Vertex;
        vbl.arrayStride = sizeof(Vertex);
        vbl.attributeCount = 3;
        vbl.attributes = attrs;

        WGPUColorTargetState targets[2]{};
        targets[0].format = device->color_format();
        targets[0].writeMask = WGPUColorWriteMask_All;
        targets[1].format = WGPUTextureFormat_R32Uint;
        targets[1].writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str("fs");
        fs.targetCount = 2;
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
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!pipeline) return fail("gpu_pipeline_failed", "mesh pipeline creation failed");
        // Sprites: same layout and vertex path, unlit fragment, alpha blend, no depth writes,
        // both faces (a sprite seen from behind is still a sprite).
        WGPUBlendState blend{};
        blend.color = {WGPUBlendOperation_Add, WGPUBlendFactor_SrcAlpha, WGPUBlendFactor_OneMinusSrcAlpha};
        blend.alpha = {WGPUBlendOperation_Add, WGPUBlendFactor_One, WGPUBlendFactor_OneMinusSrcAlpha};
        targets[0].blend = &blend;
        fs.entryPoint = rhi::str("fs_unlit");
        ds.depthWriteEnabled = WGPUOptionalBool_False;
        ds.depthCompare = WGPUCompareFunction_LessEqual;
        rpd.label = rhi::str("pocket.sprite");
        rpd.primitive.cullMode = WGPUCullMode_None;
        sprite_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!sprite_pipeline) return fail("gpu_pipeline_failed", "sprite pipeline creation failed");
        // Shadow map: depth only from the sun, both faces (thin geometry still casts), the bias
        // in the lookup handles acne.
        WGPUDepthStencilState sds = ds;
        sds.format = WGPUTextureFormat_Depth32Float;
        sds.depthWriteEnabled = WGPUOptionalBool_True;
        sds.depthCompare = WGPUCompareFunction_Less;
        rpd.label = rhi::str("pocket.shadow");
        rpd.layout = shadow_layout;
        rpd.vertex.entryPoint = rhi::str("vs_shadow");
        rpd.fragment = nullptr;
        rpd.depthStencil = &sds;
        rpd.primitive.cullMode = WGPUCullMode_None;
        shadow_pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!shadow_pipeline) return fail("gpu_pipeline_failed", "shadow pipeline creation failed");

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

        WGPUBindGroupEntry obe{};
        obe.binding = 0;
        obe.buffer = object_buffer;
        obe.size = static_cast<std::uint64_t>(kObjectStride) * kMaxObjects;
        WGPUBindGroupDescriptor obd{};
        obd.label = rhi::str("pocket.object");
        obd.layout = object_bgl;
        obd.entryCount = 1;
        obd.entries = &obe;
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
        WGPUBindGroupEntry entries[2]{};
        entries[0].binding = 0;
        entries[0].textureView = t.view;
        entries[1].binding = 1;
        entries[1].sampler = sampler;
        WGPUBindGroupDescriptor bd{};
        bd.label = rhi::str(label);
        bd.layout = material_bgl;
        bd.entryCount = 2;
        bd.entries = entries;
        t.bind_group = wgpuDeviceCreateBindGroup(device->device(), &bd);
        entries[1].sampler = nearest_sampler;
        t.bind_group_nearest = wgpuDeviceCreateBindGroup(device->device(), &bd);
        return t;
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
    WGPUBindGroup texture_for(const std::string& path, bool nearest = false) {
        auto pick = [&](const GpuTexture& t) { return nearest ? t.bind_group_nearest : t.bind_group; };
        if (path.empty()) return pick(white);
        if (auto it = textures.find(path); it != textures.end()) return pick(it->second);
        if (!assets) { report_missing(path, "no asset store"); return pick(white); }
        if (failed.contains(path)) { note_missing(path); return pick(white); }
        auto img = assets->image(path);
        if (!img) {
            report_missing(path, img.error().message);
            return pick(white);
        }
        auto t = upload_texture(path.c_str(), (*img)->width, (*img)->height, (*img)->rgba.data());
        if (!t) {
            report_missing(path, t.error().message);
            return pick(white);
        }
        textures[path] = *t;
        return pick(*t);
    }

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
        am.gpu.index_count = static_cast<std::uint32_t>(src.indices.size());
        am.gpu.aabb_min = src.aabb_min;
        am.gpu.aabb_max = src.aabb_max;
        am.submeshes = src.submeshes;
        am.materials = src.materials;
        new_bounds.emplace_back(path, std::make_pair(src.aabb_min, src.aabb_max));
        auto [it, inserted] = asset_meshes.emplace(path, std::move(am));
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

Status Renderer::render(rhi::Frame& frame, const world::World& world, rhi::Color clear) {
    Impl& im = *impl_;
    POCKET_TRY_VOID(im.ensure_id_target(frame.width, frame.height));
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
    };
    std::vector<Draw> draws;
    std::uint32_t count = 0;
    std::uint32_t entities = 0;
    world.ecs().each([&](flecs::entity e, const world::MeshRenderer& mr, const world::WorldTransform& t) {
        if (!mr.visible || count >= kMaxObjects) return;
        ++entities;
        Mat4 model = Mat4::trs(t.position, t.rotation, t.scale);
        ObjectUniforms ou{};
        to_array(model, ou.model);
        to_array(transpose(model.inverse_affine()), ou.normal);
        ou.id[0] = static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu);
        auto push = [&](const GpuMesh* gpu, std::uint32_t first, std::uint32_t n, const std::string& tex, Vec4 color, const std::string& mesh_key) {
            if (count >= kMaxObjects) return;
            ou.color[0] = color.x; ou.color[1] = color.y; ou.color[2] = color.z; ou.color[3] = color.w;
            draws.push_back({tex, mesh_key, gpu, first, n, im.texture_for(tex), ou});
            ++count;
        };
        ou.uv_rect[0] = 0; ou.uv_rect[1] = 0; ou.uv_rect[2] = 1; ou.uv_rect[3] = 1;
        int kind = Impl::primitive_index(mr.mesh);
        if (kind >= 0) {
            const GpuMesh& gm = im.meshes[static_cast<std::size_t>(kind)];
            push(&gm, 0, gm.index_count, mr.texture, {mr.color.r, mr.color.g, mr.color.b, mr.color.a}, mr.mesh);
            return;
        }
        Impl::AssetMesh* am = im.asset_mesh(mr.mesh);
        if (!am) {
            // Missing asset: a magenta cube marks the spot instead of hiding the problem.
            const GpuMesh& gm = im.meshes[0];
            push(&gm, 0, gm.index_count, "", {1, 0, 1, 1}, "cube");
            return;
        }
        for (const assets::Submesh& sm : am->submeshes) {
            const assets::Material& mat = am->materials[std::min<std::size_t>(sm.material, am->materials.size() - 1)];
            Vec4 color{mr.color.r * mat.base_color.x, mr.color.g * mat.base_color.y, mr.color.b * mat.base_color.z, mr.color.a * mat.base_color.w};
            push(&am->gpu, sm.first_index, sm.index_count, mr.texture.empty() ? mat.texture : mr.texture, color, mr.mesh);
        }
    });
    std::stable_sort(draws.begin(), draws.end(), [](const Draw& a, const Draw& b) {
        if (a.texture != b.texture) return a.texture < b.texture;
        if (a.mesh != b.mesh) return a.mesh < b.mesh;
        return a.first < b.first;
    });
    for (std::size_t i = 0; i < draws.size(); ++i) std::memcpy(im.object_staging.data() + i * kObjectStride, &draws[i].object, sizeof(ObjectUniforms));
    // Sprites: unlit quads after every mesh, by layer then far to near, in runs per texture.
    struct SpriteDraw {
        std::string texture;
        WGPUBindGroup material;
        std::int32_t layer;
        float depth;
        ObjectUniforms object;
    };
    std::vector<SpriteDraw> sprites;
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
    std::stable_sort(sprites.begin(), sprites.end(), [](const SpriteDraw& a, const SpriteDraw& b) {
        if (a.layer != b.layer) return a.layer < b.layer;
        if (a.depth != b.depth) return a.depth > b.depth;
        return a.texture < b.texture;
    });
    for (std::size_t i = 0; i < sprites.size(); ++i) std::memcpy(im.object_staging.data() + (static_cast<std::size_t>(count) + i) * kObjectStride, &sprites[i].object, sizeof(ObjectUniforms));
    std::uint32_t total = count + static_cast<std::uint32_t>(sprites.size());
    if (total > 0) im.device->write_buffer(im.object_buffer, 0, im.object_staging.data(), static_cast<std::uint64_t>(total) * kObjectStride);
    im.stats.meshes = entities;
    im.stats.instances = total;
    im.stats.sprites = static_cast<std::uint32_t>(sprites.size());
    im.stats.asset_meshes = static_cast<std::uint32_t>(im.asset_meshes.size());
    im.stats.textures = static_cast<std::uint32_t>(im.textures.size());

    WGPURenderPassColorAttachment ca[2]{};
    ca[0].view = frame.color;
    ca[0].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca[0].loadOp = WGPULoadOp_Clear;
    ca[0].storeOp = WGPUStoreOp_Store;
    ca[0].clearValue = {clear.r, clear.g, clear.b, clear.a};
    ca[1].view = im.id_view;
    ca[1].depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca[1].loadOp = WGPULoadOp_Clear;
    ca[1].storeOp = WGPUStoreOp_Store;
    ca[1].clearValue = {0, 0, 0, 0};
    WGPURenderPassDepthStencilAttachment ds{};
    ds.view = frame.depth;
    ds.depthLoadOp = WGPULoadOp_Clear;
    ds.depthStoreOp = WGPUStoreOp_Store;
    ds.depthClearValue = 1.0f;
    ds.stencilLoadOp = WGPULoadOp_Undefined;
    ds.stencilStoreOp = WGPUStoreOp_Undefined;
    ds.stencilReadOnly = true;
    // Instanced runs of equal mesh, submesh and material; the same loop serves both passes.
    auto draw_runs = [&](WGPURenderPassEncoder pass, bool with_materials, std::uint32_t& counter) {
        const GpuMesh* current_mesh = nullptr;
        WGPUBindGroup current_material = nullptr;
        std::size_t i = 0;
        while (i < draws.size()) {
            const Draw& d = draws[i];
            std::size_t run = 1;
            while (i + run < draws.size()) {
                const Draw& n = draws[i + run];
                if (n.gpu != d.gpu || n.first != d.first || n.count != d.count || n.material != d.material) break;
                ++run;
            }
            if (d.gpu != current_mesh) {
                wgpuRenderPassEncoderSetVertexBuffer(pass, 0, d.gpu->vertices, 0, WGPU_WHOLE_SIZE);
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
        wgpuRenderPassEncoderSetPipeline(spass, im.shadow_pipeline);
        wgpuRenderPassEncoderSetBindGroup(spass, 0, im.frame_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(spass, 1, im.object_bg, 0, nullptr);
        draw_runs(spass, false, im.stats.shadow_draws);
        wgpuRenderPassEncoderEnd(spass);
        wgpuRenderPassEncoderRelease(spass);
    }
    WGPURenderPassDescriptor rp{};
    rp.label = rhi::str("pocket.scene");
    rp.colorAttachmentCount = 2;
    rp.colorAttachments = ca;
    rp.depthStencilAttachment = &ds;
    WGPURenderPassEncoder pass = wgpuCommandEncoderBeginRenderPass(frame.encoder, &rp);
    if (im.applied.w != frame.width || im.applied.h != frame.height) {
        wgpuRenderPassEncoderSetViewport(pass, static_cast<float>(im.applied.x), static_cast<float>(im.applied.y), static_cast<float>(im.applied.w), static_cast<float>(im.applied.h), 0.0f, 1.0f);
        wgpuRenderPassEncoderSetScissorRect(pass, static_cast<std::uint32_t>(im.applied.x), static_cast<std::uint32_t>(im.applied.y), im.applied.w, im.applied.h);
    }
    if (!draws.empty()) {
        wgpuRenderPassEncoderSetPipeline(pass, im.pipeline);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        draw_runs(pass, true, im.stats.draw_calls);
    }
    if (!sprites.empty()) {
        const GpuMesh& quad = im.meshes[static_cast<std::size_t>(Primitive::Quad)];
        wgpuRenderPassEncoderSetPipeline(pass, im.sprite_pipeline);
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.scene_bg, 0, nullptr);
        wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 0, nullptr);
        wgpuRenderPassEncoderSetVertexBuffer(pass, 0, quad.vertices, 0, WGPU_WHOLE_SIZE);
        wgpuRenderPassEncoderSetIndexBuffer(pass, quad.indices, WGPUIndexFormat_Uint32, 0, WGPU_WHOLE_SIZE);
        std::size_t i = 0;
        while (i < sprites.size()) {
            std::size_t run = 1;
            while (i + run < sprites.size() && sprites[i + run].material == sprites[i].material) ++run;
            wgpuRenderPassEncoderSetBindGroup(pass, 2, sprites[i].material, 0, nullptr);
            wgpuRenderPassEncoderDrawIndexed(pass, quad.index_count, static_cast<std::uint32_t>(run), 0, 0, count + static_cast<std::uint32_t>(i));
            im.stats.draw_calls++;
            i += run;
        }
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
