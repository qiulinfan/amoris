#include <pocket/renderer/renderer.hpp>

#include <pocket/core/log.hpp>
#include <pocket/renderer/primitives.hpp>

#include <webgpu/wgpu.h>

#include <array>
#include <cstring>

namespace pocket::renderer {

namespace {

constexpr std::uint32_t kMaxPointLights = 8;
constexpr std::uint32_t kObjectStride = 256;  // uniform buffer offset alignment
constexpr std::uint32_t kMaxObjects = 4096;

struct alignas(16) FrameUniforms {
    float view_proj[16];
    float camera_pos[4];
    float sun_dir[4];
    float sun_color[4];
    float ambient[4];
    std::uint32_t point_count[4];
    float point_pos[kMaxPointLights][4];
    float point_color[kMaxPointLights][4];
};

struct alignas(16) ObjectUniforms {
    float model[16];
    float normal[16];
    float color[4];
    std::uint32_t id[4];
};

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
};
struct Object {
    model: mat4x4f,
    normal: mat4x4f,
    color: vec4f,
    id: vec4u,
};
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> object: Object;

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) world_pos: vec3f,
    @location(1) normal: vec3f,
};

@vertex fn vs(@location(0) position: vec3f, @location(1) normal: vec3f) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4f(position, 1.0);
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((object.normal * vec4f(normal, 0.0)).xyz);
    return out;
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
    light += frame.sun_color.rgb * (ndl + spec * ndl);
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
    var out: FsOut;
    out.color = vec4f(object.color.rgb * light, object.color.a);
    out.id = object.id.x;
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
    WGPUBuffer frame_buffer = nullptr;
    WGPUBuffer object_buffer = nullptr;
    WGPUBindGroup frame_bg = nullptr;
    WGPUBindGroup object_bg = nullptr;
    WGPUTexture id_texture = nullptr;
    WGPUTextureView id_view = nullptr;
    std::uint32_t id_width = 0, id_height = 0;
    std::array<GpuMesh, 4> meshes{};
    RenderStats stats;
    CameraView camera;
    Viewport viewport;   // requested
    Viewport applied;    // used by the last frame
    std::uint32_t last_width = 0, last_height = 0;
    std::vector<std::uint8_t> object_staging;

    ~Impl() {
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

        WGPUBindGroupLayoutEntry oe{};
        oe.binding = 0;
        oe.visibility = WGPUShaderStage_Vertex | WGPUShaderStage_Fragment;
        oe.buffer.type = WGPUBufferBindingType_Uniform;
        oe.buffer.hasDynamicOffset = true;
        oe.buffer.minBindingSize = sizeof(ObjectUniforms);
        WGPUBindGroupLayoutDescriptor od{};
        od.label = rhi::str("pocket.object");
        od.entryCount = 1;
        od.entries = &oe;
        object_bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &od);

        WGPUBindGroupLayout bgls[2] = {frame_bgl, object_bgl};
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.mesh");
        pld.bindGroupLayoutCount = 2;
        pld.bindGroupLayouts = bgls;
        layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);

        WGPUVertexAttribute attrs[2]{};
        attrs[0].format = WGPUVertexFormat_Float32x3;
        attrs[0].offset = 0;
        attrs[0].shaderLocation = 0;
        attrs[1].format = WGPUVertexFormat_Float32x3;
        attrs[1].offset = sizeof(float) * 3;
        attrs[1].shaderLocation = 1;
        WGPUVertexBufferLayout vbl{};
        vbl.stepMode = WGPUVertexStepMode_Vertex;
        vbl.arrayStride = sizeof(Vertex);
        vbl.attributeCount = 2;
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

        frame_buffer = device->create_buffer("pocket.frame", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(FrameUniforms));
        object_buffer = device->create_buffer("pocket.objects", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, static_cast<std::uint64_t>(kObjectStride) * kMaxObjects);
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

        WGPUBindGroupEntry obe{};
        obe.binding = 0;
        obe.buffer = object_buffer;
        obe.size = sizeof(ObjectUniforms);
        WGPUBindGroupDescriptor obd{};
        obd.label = rhi::str("pocket.object");
        obd.layout = object_bgl;
        obd.entryCount = 1;
        obd.entries = &obe;
        object_bg = wgpuDeviceCreateBindGroup(device->device(), &obd);

        for (int k = 0; k < 4; ++k) {
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
            return cv;
        }
        Vec3 forward = ct.rotation.rotate({0, 0, -1});
        Vec3 up = ct.rotation.rotate({0, 1, 0});
        cv.view = Mat4::look_at(ct.position, ct.position + forward, up);
        cv.proj = Mat4::perspective(radians(cam.fov_degrees), aspect, cam.near, cam.far);
        cv.position = ct.position;
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
    im.device->write_buffer(im.frame_buffer, 0, &fu, sizeof fu);

    // Gather draws.
    struct Draw { std::uint32_t mesh; std::uint32_t offset; };
    std::vector<Draw> draws;
    std::uint32_t count = 0;
    world.ecs().each([&](flecs::entity e, const world::MeshRenderer& mr, const world::WorldTransform& t) {
        if (!mr.visible || count >= kMaxObjects) return;
        ObjectUniforms ou{};
        Mat4 model = Mat4::trs(t.position, t.rotation, t.scale);
        to_array(model, ou.model);
        to_array(transpose(model.inverse_affine()), ou.normal);
        ou.color[0] = mr.color.r; ou.color[1] = mr.color.g; ou.color[2] = mr.color.b; ou.color[3] = mr.color.a;
        ou.id[0] = static_cast<std::uint32_t>(e.id() & 0xFFFFFFFFu);
        std::memcpy(im.object_staging.data() + static_cast<std::size_t>(count) * kObjectStride, &ou, sizeof ou);
        int kind = mr.mesh < 0 || mr.mesh > 3 ? 0 : mr.mesh;
        draws.push_back({static_cast<std::uint32_t>(kind), count * kObjectStride});
        ++count;
    });
    if (count > 0) im.device->write_buffer(im.object_buffer, 0, im.object_staging.data(), static_cast<std::uint64_t>(count) * kObjectStride);
    im.stats.meshes = count;

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
        wgpuRenderPassEncoderSetBindGroup(pass, 0, im.frame_bg, 0, nullptr);
        std::uint32_t current_mesh = 0xFFFFFFFFu;
        for (const Draw& d : draws) {
            if (d.mesh != current_mesh) {
                const GpuMesh& gm = im.meshes[d.mesh];
                wgpuRenderPassEncoderSetVertexBuffer(pass, 0, gm.vertices, 0, WGPU_WHOLE_SIZE);
                wgpuRenderPassEncoderSetIndexBuffer(pass, gm.indices, WGPUIndexFormat_Uint32, 0, WGPU_WHOLE_SIZE);
                current_mesh = d.mesh;
            }
            wgpuRenderPassEncoderSetBindGroup(pass, 1, im.object_bg, 1, &d.offset);
            wgpuRenderPassEncoderDrawIndexed(pass, im.meshes[d.mesh].index_count, 1, 0, 0, 0);
            im.stats.draw_calls++;
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
        wgpuDevicePoll(im.device->device(), true, nullptr);
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
Viewport Renderer::viewport() const { return impl_->viewport; }
Viewport Renderer::applied_viewport() const { return impl_->applied; }

Json Renderer::describe() const {
    const RenderStats& s = impl_->stats;
    Json j;
    j["draw_calls"] = s.draw_calls;
    j["meshes"] = s.meshes;
    j["point_lights"] = s.point_lights;
    j["has_camera"] = s.has_camera;
    j["has_sun"] = s.has_sun;
    if (s.camera) j["camera"] = s.camera;
    const Viewport& v = impl_->applied;
    if (v.w != impl_->last_width || v.h != impl_->last_height) j["viewport"] = Json{{"x", v.x}, {"y", v.y}, {"w", v.w}, {"h", v.h}};
    return j;
}

}  // namespace pocket::renderer
