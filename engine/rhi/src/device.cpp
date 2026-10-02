#include <pocket/rhi/device.hpp>

#include <pocket/core/log.hpp>

#include <cstdio>
#include <cstdlib>
#include <deque>
#include <format>

#ifdef __EMSCRIPTEN__
#include <emscripten.h>
#else
#include <webgpu/wgpu.h>
#endif

#include <cstring>

namespace pocket::rhi {

std::string to_string(WGPUStringView s) {
    if (!s.data) return {};
    return std::string(s.data, s.length == WGPU_STRLEN ? std::strlen(s.data) : s.length);
}

namespace {

constexpr const char* kBlitWgsl = R"WGSL(
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    var p = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    return vec4f(p[i], 0.0, 1.0);
}
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@fragment fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let uv = pos.xy / vec2f(textureDimensions(src));
    return textureSample(src, smp, uv);
}
)WGSL";

const char* backend_name(WGPUBackendType t) {
    switch (t) {
        case WGPUBackendType_Metal: return "metal";
        case WGPUBackendType_Vulkan: return "vulkan";
        case WGPUBackendType_D3D12: return "d3d12";
        case WGPUBackendType_OpenGL: return "opengl";
        case WGPUBackendType_OpenGLES: return "opengles";
        case WGPUBackendType_WebGPU: return "webgpu";
        default: return "unknown";
    }
}

std::string& last_held() {
    static std::string held;
    return held;
}

}  // namespace

struct Device::Impl {
    Config config;
    bool timestamps = false;
    float timestamp_period = 1;
    std::uint64_t completed = 0;   // frames the GPU has finished (their submitted work done)
#ifndef __EMSCRIPTEN__
    std::deque<WGPUSubmissionIndex> in_flight;   // the frames submitted and not yet waited for, oldest first
#endif
    WGPUInstance instance = nullptr;
    WGPUAdapter adapter = nullptr;
    WGPUDevice device = nullptr;
    WGPUQueue queue = nullptr;
    WGPUSurface surface = nullptr;
    WGPUTextureFormat surface_format = WGPUTextureFormat_BGRA8Unorm;
    WGPUTexture color = nullptr;
    WGPUTextureView color_view = nullptr;
    WGPUTexture depth = nullptr;
    WGPUTextureView depth_view = nullptr;
    WGPUSampler blit_sampler = nullptr;
    WGPUBindGroupLayout blit_bgl = nullptr;
    WGPUBindGroup blit_bg = nullptr;
    WGPURenderPipeline blit_pipeline = nullptr;
    std::uint32_t width = 0, height = 0;
    bool unseen = false;   // the window has no area: frames are drawn offscreen and not presented
    std::uint64_t frame_index = 0;
    std::uint64_t presented = 0;
    std::uint64_t skipped_presents = 0;
    bool error = false;
    std::string adapter_name, adapter_backend, adapter_driver;

    void destroy_targets() {
        if (blit_bg) { wgpuBindGroupRelease(blit_bg); blit_bg = nullptr; }
        if (color_view) { wgpuTextureViewRelease(color_view); color_view = nullptr; }
        if (color) { wgpuTextureRelease(color); color = nullptr; }
        if (depth_view) { wgpuTextureViewRelease(depth_view); depth_view = nullptr; }
        if (depth) { wgpuTextureRelease(depth); depth = nullptr; }
    }

    Status create_targets(std::uint32_t w, std::uint32_t h) {
        destroy_targets();
        width = w == 0 ? 1 : w;
        height = h == 0 ? 1 : h;
        WGPUTextureDescriptor td{};
        td.label = str("pocket.color");
        td.usage = WGPUTextureUsage_RenderAttachment | WGPUTextureUsage_CopySrc | WGPUTextureUsage_TextureBinding;
        td.dimension = WGPUTextureDimension_2D;
        td.size = {width, height, 1};
        td.format = WGPUTextureFormat_RGBA8Unorm;
        td.mipLevelCount = 1;
        td.sampleCount = 1;
        color = wgpuDeviceCreateTexture(device, &td);
        if (!color) return fail("gpu_texture_failed", "cannot create color target {}x{}", width, height);
        WGPUTextureViewDescriptor vd{};
        vd.format = td.format;
        vd.dimension = WGPUTextureViewDimension_2D;
        vd.mipLevelCount = 1;
        vd.arrayLayerCount = 1;
        vd.aspect = WGPUTextureAspect_All;
        vd.usage = td.usage;
        color_view = wgpuTextureCreateView(color, &vd);

        WGPUTextureDescriptor dd{};
        dd.label = str("pocket.depth");
        dd.usage = WGPUTextureUsage_RenderAttachment;
        dd.dimension = WGPUTextureDimension_2D;
        dd.size = {width, height, 1};
        dd.format = WGPUTextureFormat_Depth24Plus;
        dd.mipLevelCount = 1;
        dd.sampleCount = 1;
        depth = wgpuDeviceCreateTexture(device, &dd);
        if (!depth) return fail("gpu_texture_failed", "cannot create depth target");
        WGPUTextureViewDescriptor dvd{};
        dvd.format = dd.format;
        dvd.dimension = WGPUTextureViewDimension_2D;
        dvd.mipLevelCount = 1;
        dvd.arrayLayerCount = 1;
        dvd.aspect = WGPUTextureAspect_All;
        dvd.usage = dd.usage;
        depth_view = wgpuTextureCreateView(depth, &dvd);

        if (surface) {
            WGPUSurfaceConfiguration sc{};
            sc.device = device;
            sc.format = surface_format;
            sc.usage = WGPUTextureUsage_RenderAttachment;
            sc.width = width;
            sc.height = height;
            sc.alphaMode = WGPUCompositeAlphaMode_Auto;
            sc.presentMode = config.vsync ? WGPUPresentMode_Fifo : WGPUPresentMode_Immediate;
            wgpuSurfaceConfigure(surface, &sc);
            WGPUBindGroupEntry entries[2]{};
            entries[0].binding = 0;
            entries[0].textureView = color_view;
            entries[1].binding = 1;
            entries[1].sampler = blit_sampler;
            WGPUBindGroupDescriptor bgd{};
            bgd.label = str("pocket.blit");
            bgd.layout = blit_bgl;
            bgd.entryCount = 2;
            bgd.entries = entries;
            blit_bg = wgpuDeviceCreateBindGroup(device, &bgd);
        }
        return {};
    }

    Status create_blit_pipeline() {
        WGPUShaderSourceWGSL wgsl{};
        wgsl.chain.sType = WGPUSType_ShaderSourceWGSL;
        wgsl.code = str(kBlitWgsl);
        WGPUShaderModuleDescriptor smd{};
        smd.nextInChain = &wgsl.chain;
        smd.label = str("pocket.blit");
        WGPUShaderModule module = wgpuDeviceCreateShaderModule(device, &smd);
        if (!module) return fail("gpu_shader_failed", "blit shader failed to compile");

        WGPUBindGroupLayoutEntry bgl_entries[2]{};
        bgl_entries[0].binding = 0;
        bgl_entries[0].visibility = WGPUShaderStage_Fragment;
        bgl_entries[0].texture.sampleType = WGPUTextureSampleType_Float;
        bgl_entries[0].texture.viewDimension = WGPUTextureViewDimension_2D;
        bgl_entries[1].binding = 1;
        bgl_entries[1].visibility = WGPUShaderStage_Fragment;
        bgl_entries[1].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor bgld{};
        bgld.label = str("pocket.blit");
        bgld.entryCount = 2;
        bgld.entries = bgl_entries;
        blit_bgl = wgpuDeviceCreateBindGroupLayout(device, &bgld);

        WGPUPipelineLayoutDescriptor pld{};
        pld.label = str("pocket.blit");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &blit_bgl;
        WGPUPipelineLayout layout = wgpuDeviceCreatePipelineLayout(device, &pld);

        WGPUColorTargetState target{};
        target.format = surface_format;
        target.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = module;
        fs.entryPoint = str("fs");
        fs.targetCount = 1;
        fs.targets = &target;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = str("pocket.blit");
        rpd.layout = layout;
        rpd.vertex.module = module;
        rpd.vertex.entryPoint = str("vs");
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        blit_pipeline = wgpuDeviceCreateRenderPipeline(device, &rpd);
        wgpuPipelineLayoutRelease(layout);
        wgpuShaderModuleRelease(module);
        if (!blit_pipeline) return fail("gpu_pipeline_failed", "blit pipeline creation failed");

        WGPUSamplerDescriptor sd{};
        sd.label = str("pocket.blit");
        sd.addressModeU = WGPUAddressMode_ClampToEdge;
        sd.addressModeV = WGPUAddressMode_ClampToEdge;
        sd.addressModeW = WGPUAddressMode_ClampToEdge;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
        sd.lodMinClamp = 0.0f;
        sd.lodMaxClamp = 32.0f;
        sd.maxAnisotropy = 1;
        blit_sampler = wgpuDeviceCreateSampler(device, &sd);
        return {};
    }

    ~Impl() {
#ifndef __EMSCRIPTEN__
        // The work-done callbacks name this; let them all come in before it goes.
        for (int i = 0; i < 10000 && device && completed < frame_index; ++i) {
            wgpuDevicePoll(device, true, nullptr);
            if (instance) wgpuInstanceProcessEvents(instance);
        }
#endif
        destroy_targets();
        if (blit_pipeline) wgpuRenderPipelineRelease(blit_pipeline);
        if (blit_bgl) wgpuBindGroupLayoutRelease(blit_bgl);
        if (blit_sampler) wgpuSamplerRelease(blit_sampler);
#ifndef __EMSCRIPTEN__
        // What is still held as the device goes (everything else has let go by now): an object a
        // subsystem forgot to release, by its kind. Printed with POCKET_GPU_REPORT=1.
        last_held().clear();
        if (instance) {
            WGPUGlobalReport r{};
            wgpuGenerateReport(instance, &r);
            const WGPUHubReport& h = r.hub;
            const std::pair<const char*, const WGPURegistryReport*> kinds[] = {
                {"pipeline layouts", &h.pipelineLayouts}, {"shader modules", &h.shaderModules}, {"bind group layouts", &h.bindGroupLayouts},
                {"bind groups", &h.bindGroups}, {"command buffers", &h.commandBuffers}, {"render bundles", &h.renderBundles},
                {"render pipelines", &h.renderPipelines}, {"compute pipelines", &h.computePipelines}, {"pipeline caches", &h.pipelineCaches},
                {"query sets", &h.querySets}, {"buffers", &h.buffers}, {"textures", &h.textures}, {"texture views", &h.textureViews}, {"samplers", &h.samplers}};
            std::string held;
            for (const auto& [name, reg] : kinds) {
                if (reg->numKeptFromUser > 0) held += std::format("{}{} {}", held.empty() ? "" : ", ", reg->numKeptFromUser, name);
            }
            last_held() = held;
            if (std::getenv("POCKET_GPU_REPORT")) {
                const std::string line = std::format("rhi: still held as the device goes: {}\n", held.empty() ? std::string("nothing") : held);
                std::fputs(line.c_str(), stderr);   // whatever the log level: it was asked for
            }
        }
#endif
        if (surface) { wgpuSurfaceUnconfigure(surface); wgpuSurfaceRelease(surface); }
        if (queue) wgpuQueueRelease(queue);
        if (device) wgpuDeviceRelease(device);
        if (adapter) wgpuAdapterRelease(adapter);
        if (instance) wgpuInstanceRelease(instance);
    }
};

namespace {
// Progress for asynchronous GPU work. wgpu-native processes callbacks from a poll; in the
// browser the callbacks arrive from the page's event loop, so waiting means yielding to it
// (emscripten_sleep, an Asyncify unwind), and a non-waiting poll is nothing to do.
void device_progress(WGPUInstance instance, WGPUDevice device, bool wait) {
#ifdef __EMSCRIPTEN__
    (void)device;
    if (wait) emscripten_sleep(1);
#else
    if (device) wgpuDevicePoll(device, wait, nullptr);
#endif
    if (instance) wgpuInstanceProcessEvents(instance);
}
}  // namespace

void Device::poll(bool wait) { device_progress(impl_->instance, impl_->device, wait); }

Device::Device() : impl_(std::make_unique<Impl>()) {}
Device::~Device() = default;

std::string Device::held_when_last_destroyed() { return last_held(); }

Result<std::unique_ptr<Device>> Device::create(const Config& config) {
    std::unique_ptr<Device> d(new Device());
    Impl& im = *d->impl_;
    im.config = config;
    im.instance = wgpuCreateInstance(nullptr);
    if (!im.instance) return fail("gpu_instance_failed", "wgpuCreateInstance returned null");

#ifdef __EMSCRIPTEN__
    if (!config.canvas_selector.empty()) {
        WGPUEmscriptenSurfaceSourceCanvasHTMLSelector src{};
        src.chain.sType = WGPUSType_EmscriptenSurfaceSourceCanvasHTMLSelector;
        src.selector = str(config.canvas_selector.c_str());
        WGPUSurfaceDescriptor sd{};
        sd.nextInChain = &src.chain;
        sd.label = str("pocket.surface");
        im.surface = wgpuInstanceCreateSurface(im.instance, &sd);
        if (!im.surface) return fail("gpu_surface_failed", "cannot create surface from canvas {}", config.canvas_selector);
    }
#else
    if (config.metal_layer) {
        WGPUSurfaceSourceMetalLayer src{};
        src.chain.sType = WGPUSType_SurfaceSourceMetalLayer;
        src.layer = config.metal_layer;
        WGPUSurfaceDescriptor sd{};
        sd.nextInChain = &src.chain;
        sd.label = str("pocket.surface");
        im.surface = wgpuInstanceCreateSurface(im.instance, &sd);
        if (!im.surface) return fail("gpu_surface_failed", "cannot create surface from Metal layer");
    } else if (config.wayland_surface) {
        WGPUSurfaceSourceWaylandSurface src{};
        src.chain.sType = WGPUSType_SurfaceSourceWaylandSurface;
        src.display = config.wayland_display;
        src.surface = config.wayland_surface;
        WGPUSurfaceDescriptor sd{};
        sd.nextInChain = &src.chain;
        sd.label = str("pocket.surface");
        im.surface = wgpuInstanceCreateSurface(im.instance, &sd);
        if (!im.surface) return fail("gpu_surface_failed", "cannot create surface from the Wayland surface");
    } else if (config.x11_window) {
        WGPUSurfaceSourceXlibWindow src{};
        src.chain.sType = WGPUSType_SurfaceSourceXlibWindow;
        src.display = config.x11_display;
        src.window = config.x11_window;
        WGPUSurfaceDescriptor sd{};
        sd.nextInChain = &src.chain;
        sd.label = str("pocket.surface");
        im.surface = wgpuInstanceCreateSurface(im.instance, &sd);
        if (!im.surface) return fail("gpu_surface_failed", "cannot create surface from the X11 window");
    }
#endif

    struct AdapterResult { WGPUAdapter adapter = nullptr; std::string msg; bool done = false; } ar;
    WGPURequestAdapterOptions opts{};
    opts.powerPreference = config.prefer_high_performance ? WGPUPowerPreference_HighPerformance : WGPUPowerPreference_LowPower;
    opts.compatibleSurface = im.surface;
    WGPURequestAdapterCallbackInfo aci{};
    aci.mode = WGPUCallbackMode_AllowSpontaneous;
    aci.callback = [](WGPURequestAdapterStatus status, WGPUAdapter adapter, WGPUStringView message, void* u1, void*) {
        auto* r = static_cast<AdapterResult*>(u1);
        r->msg = to_string(message);
        r->done = true;
        if (status == WGPURequestAdapterStatus_Success) r->adapter = adapter;
    };
    aci.userdata1 = &ar;
    wgpuInstanceRequestAdapter(im.instance, &opts, aci);
    for (int i = 0; i < 100000 && !ar.done; ++i) device_progress(im.instance, nullptr, true);
    if (!ar.adapter) return fail("gpu_adapter_failed", "no adapter: {}", ar.msg);
    im.adapter = ar.adapter;
    WGPUAdapterInfo info{};
    wgpuAdapterGetInfo(im.adapter, &info);
    im.adapter_name = to_string(info.device);
    im.adapter_driver = to_string(info.description);
    im.adapter_backend = backend_name(info.backendType);
    wgpuAdapterInfoFreeMembers(info);

    struct DeviceResult { WGPUDevice device = nullptr; std::string msg; bool done = false; } dr;
    WGPUDeviceDescriptor dd{};
    dd.label = str("pocket.device");
    dd.deviceLostCallbackInfo.mode = WGPUCallbackMode_AllowSpontaneous;
    dd.deviceLostCallbackInfo.callback = [](WGPUDevice const*, WGPUDeviceLostReason reason, WGPUStringView message, void* u1, void*) {
        auto* im = static_cast<Impl*>(u1);
        if (reason != WGPUDeviceLostReason_Destroyed) {
            im->error = true;
            log::error("rhi", "device lost ({}): {}", static_cast<int>(reason), to_string(message));
        }
    };
    dd.deviceLostCallbackInfo.userdata1 = &im;
    dd.uncapturedErrorCallbackInfo.callback = [](WGPUDevice const*, WGPUErrorType type, WGPUStringView message, void* u1, void*) {
        auto* im = static_cast<Impl*>(u1);
        im->error = true;
        log::error("rhi", "gpu error ({}): {}", static_cast<int>(type), to_string(message));
    };
    dd.uncapturedErrorCallbackInfo.userdata1 = &im;
    // Timestamps at the passes' ends, for the renderer's GPU timings, where the adapter has them.
    const WGPUFeatureName timestamp_feature = WGPUFeatureName_TimestampQuery;
    if (wgpuAdapterHasFeature(im.adapter, timestamp_feature)) {
        dd.requiredFeatureCount = 1;
        dd.requiredFeatures = &timestamp_feature;
        im.timestamps = true;
    }
    WGPURequestDeviceCallbackInfo dci{};
    dci.mode = WGPUCallbackMode_AllowSpontaneous;
    dci.callback = [](WGPURequestDeviceStatus status, WGPUDevice device, WGPUStringView message, void* u1, void*) {
        auto* r = static_cast<DeviceResult*>(u1);
        r->msg = to_string(message);
        r->done = true;
        if (status == WGPURequestDeviceStatus_Success) r->device = device;
    };
    dci.userdata1 = &dr;
    wgpuAdapterRequestDevice(im.adapter, &dd, dci);
    for (int i = 0; i < 100000 && !dr.done; ++i) device_progress(im.instance, nullptr, true);
    if (!dr.device) return fail("gpu_device_failed", "no device: {}", dr.msg);
    im.device = dr.device;
    im.queue = wgpuDeviceGetQueue(im.device);
#ifndef __EMSCRIPTEN__
    if (im.timestamps) im.timestamp_period = wgpuQueueGetTimestampPeriod(im.queue);   // the web's are nanoseconds already
#endif

    if (im.surface) {
        WGPUSurfaceCapabilities caps{};
        wgpuSurfaceGetCapabilities(im.surface, im.adapter, &caps);
        im.surface_format = caps.formatCount > 0 ? caps.formats[0] : WGPUTextureFormat_BGRA8Unorm;
        for (std::size_t i = 0; i < caps.formatCount; ++i) {
            if (caps.formats[i] == WGPUTextureFormat_BGRA8Unorm) im.surface_format = caps.formats[i];
        }
        wgpuSurfaceCapabilitiesFreeMembers(caps);
        POCKET_TRY_VOID(im.create_blit_pipeline());
    }
    POCKET_TRY_VOID(im.create_targets(config.width, config.height));
    log::info("rhi", "adapter {} ({}), backend {}, target {}x{}, surface {}", im.adapter_name, im.adapter_driver, im.adapter_backend, im.width, im.height, im.surface ? "yes" : "no");
    return d;
}

Status Device::resize(std::uint32_t width, std::uint32_t height) {
    // A window with no area (minimized, a canvas in a collapsed page) keeps its targets and draws
    // offscreen into them; nothing is presented until it has a size again.
    impl_->unseen = width == 0 || height == 0;
    if (impl_->unseen || (width == impl_->width && height == impl_->height)) return {};
    return impl_->create_targets(width, height);
}

Result<Frame> Device::begin_frame() {
    Frame f;
    f.encoder = wgpuDeviceCreateCommandEncoder(impl_->device, nullptr);
    if (!f.encoder) return fail("gpu_encoder_failed", "cannot create command encoder");
    f.color = impl_->color_view;
    f.color_texture = impl_->color;
    f.depth = impl_->depth_view;
    f.width = impl_->width;
    f.height = impl_->height;
    f.index = impl_->frame_index;
    return f;
}

WGPURenderPassEncoder Device::begin_main_pass(Frame& frame, Color clear) {
    WGPURenderPassColorAttachment ca{};
    ca.view = frame.color;
    ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca.loadOp = WGPULoadOp_Clear;
    ca.storeOp = WGPUStoreOp_Store;
    ca.clearValue = {clear.r, clear.g, clear.b, clear.a};
    WGPURenderPassDepthStencilAttachment ds{};
    ds.view = frame.depth;
    ds.depthLoadOp = WGPULoadOp_Clear;
    ds.depthStoreOp = WGPUStoreOp_Store;
    ds.depthClearValue = 1.0f;
    ds.depthReadOnly = false;
    ds.stencilLoadOp = WGPULoadOp_Undefined;
    ds.stencilStoreOp = WGPUStoreOp_Undefined;
    ds.stencilReadOnly = true;
    WGPURenderPassDescriptor rp{};
    rp.label = str("pocket.main");
    rp.colorAttachmentCount = 1;
    rp.colorAttachments = &ca;
    rp.depthStencilAttachment = &ds;
    return wgpuCommandEncoderBeginRenderPass(frame.encoder, &rp);
}

Status Device::end_frame(Frame& frame) {
    Impl& im = *impl_;
    WGPUSurfaceTexture st{};
    WGPUTextureView surface_view = nullptr;
    bool present = false;
    if (im.surface && !im.unseen) {
        wgpuSurfaceGetCurrentTexture(im.surface, &st);
        if (st.status == WGPUSurfaceGetCurrentTextureStatus_SuccessOptimal || st.status == WGPUSurfaceGetCurrentTextureStatus_SuccessSuboptimal) {
            surface_view = wgpuTextureCreateView(st.texture, nullptr);
            WGPURenderPassColorAttachment ca{};
            ca.view = surface_view;
            ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
            ca.loadOp = WGPULoadOp_Clear;
            ca.storeOp = WGPUStoreOp_Store;
            ca.clearValue = {0, 0, 0, 1};
            WGPURenderPassDescriptor rp{};
            rp.label = str("pocket.present");
            rp.colorAttachmentCount = 1;
            rp.colorAttachments = &ca;
            WGPURenderPassEncoder pass = wgpuCommandEncoderBeginRenderPass(frame.encoder, &rp);
            wgpuRenderPassEncoderSetPipeline(pass, im.blit_pipeline);
            wgpuRenderPassEncoderSetBindGroup(pass, 0, im.blit_bg, 0, nullptr);
            wgpuRenderPassEncoderDraw(pass, 3, 1, 0, 0);
            wgpuRenderPassEncoderEnd(pass);
            wgpuRenderPassEncoderRelease(pass);
            present = true;
        } else {
            // The window is not ready to be drawn (not yet mapped, occluded, being resized). The
            // frame still rendered offscreen; only the present is skipped.
            im.skipped_presents++;
            if (im.skipped_presents == 1) log::debug("rhi", "surface texture unavailable (status {:#x}); presenting skipped", static_cast<unsigned>(st.status));
            if (st.texture) { wgpuTextureRelease(st.texture); st.texture = nullptr; }
        }
    }
    WGPUCommandBuffer cb = wgpuCommandEncoderFinish(frame.encoder, nullptr);
#ifndef __EMSCRIPTEN__
    im.in_flight.push_back(wgpuQueueSubmitForIndex(im.queue, 1, &cb));
#else
    wgpuQueueSubmit(im.queue, 1, &cb);
#endif
    wgpuCommandBufferRelease(cb);
#ifndef __EMSCRIPTEN__
    // At most two frames queued behind this one: without a display to pace it (headless, or a
    // hidden window) the CPU would run a hundred frames ahead of the GPU, holding their resources
    // and making anything read back from the GPU that old. Only the oldest is waited for, so the GPU
    // keeps working on the next while the CPU builds another (a wait with no submission named waits
    // for everything queued, and left the GPU idle while the CPU caught up: 6.6 ms a frame where
    // the GPU's own work was 3.6). The browser paces its frames itself.
    WGPUQueueWorkDoneCallbackInfo done{};
    done.mode = WGPUCallbackMode_AllowSpontaneous;
    done.callback = [](WGPUQueueWorkDoneStatus, WGPUStringView, void* u1, void*) { ++static_cast<Impl*>(u1)->completed; };
    done.userdata1 = &im;
    wgpuQueueOnSubmittedWorkDone(im.queue, done);
    while (im.in_flight.size() > 2) {
        wgpuDevicePoll(im.device, true, &im.in_flight.front());
        im.in_flight.pop_front();
    }
#endif
    wgpuCommandEncoderRelease(frame.encoder);
    frame.encoder = nullptr;
    if (present) {
#ifndef __EMSCRIPTEN__
        wgpuSurfacePresent(im.surface);
#endif
        // The browser presents the canvas when the frame callback returns.
        im.presented++;
    }
    if (surface_view) wgpuTextureViewRelease(surface_view);
    if (st.texture) wgpuTextureRelease(st.texture);
    device_progress(im.instance, im.device, false);
    ++im.frame_index;
    if (im.error) return fail("gpu_error", "a GPU error was reported during frame {}", frame.index);
    return {};
}

Result<Image> Device::capture() { return read_texture(impl_->color, impl_->width, impl_->height); }

Result<Image> Device::read_texture(WGPUTexture texture, std::uint32_t width, std::uint32_t height) {
    Impl& im = *impl_;
    const std::uint32_t bpr_unpadded = width * 4;
    const std::uint32_t align = 256;
    const std::uint32_t bpr = (bpr_unpadded + align - 1) / align * align;
    WGPUBufferDescriptor bd{};
    bd.label = str("pocket.readback");
    bd.usage = WGPUBufferUsage_MapRead | WGPUBufferUsage_CopyDst;
    bd.size = static_cast<std::uint64_t>(bpr) * height;
    WGPUBuffer readback = wgpuDeviceCreateBuffer(im.device, &bd);
    if (!readback) return fail("gpu_buffer_failed", "cannot create readback buffer");
    WGPUCommandEncoder enc = wgpuDeviceCreateCommandEncoder(im.device, nullptr);
    WGPUTexelCopyTextureInfo src{};
    src.texture = texture;
    src.aspect = WGPUTextureAspect_All;
    WGPUTexelCopyBufferInfo dst{};
    dst.layout.offset = 0;
    dst.layout.bytesPerRow = bpr;
    dst.layout.rowsPerImage = height;
    dst.buffer = readback;
    WGPUExtent3D ext{width, height, 1};
    wgpuCommandEncoderCopyTextureToBuffer(enc, &src, &dst, &ext);
    WGPUCommandBuffer cb = wgpuCommandEncoderFinish(enc, nullptr);
    wgpuQueueSubmit(im.queue, 1, &cb);
    wgpuCommandBufferRelease(cb);
    wgpuCommandEncoderRelease(enc);

    struct MapResult { bool done = false; WGPUMapAsyncStatus status{}; std::string msg; } mr;
    WGPUBufferMapCallbackInfo mci{};
    mci.mode = WGPUCallbackMode_AllowSpontaneous;
    mci.callback = [](WGPUMapAsyncStatus status, WGPUStringView message, void* u1, void*) {
        auto* r = static_cast<MapResult*>(u1);
        r->done = true;
        r->status = status;
        r->msg = to_string(message);
    };
    mci.userdata1 = &mr;
    wgpuBufferMapAsync(readback, WGPUMapMode_Read, 0, bd.size, mci);
    for (int i = 0; i < 10000 && !mr.done; ++i) device_progress(im.instance, im.device, true);
    if (!mr.done || mr.status != WGPUMapAsyncStatus_Success) {
        wgpuBufferRelease(readback);
        return fail("gpu_map_failed", "readback map failed: {}", mr.msg);
    }
    const auto* px = static_cast<const std::uint8_t*>(wgpuBufferGetConstMappedRange(readback, 0, bd.size));
    Image img;
    img.width = width;
    img.height = height;
    img.rgba.resize(static_cast<std::size_t>(bpr_unpadded) * height);
    for (std::uint32_t y = 0; y < height; ++y) {
        std::memcpy(img.rgba.data() + static_cast<std::size_t>(y) * bpr_unpadded, px + static_cast<std::size_t>(y) * bpr, bpr_unpadded);
    }
    wgpuBufferUnmap(readback);
    wgpuBufferRelease(readback);
    return img;
}

WGPUDevice Device::device() const { return impl_->device; }
WGPUQueue Device::queue() const { return impl_->queue; }
bool Device::timestamps() const { return impl_->timestamps; }
float Device::timestamp_period() const { return impl_->timestamp_period; }
std::uint64_t Device::submitted_frames() const { return impl_->frame_index; }
WGPUInstance Device::instance() const { return impl_->instance; }
std::uint32_t Device::width() const { return impl_->width; }
std::uint32_t Device::height() const { return impl_->height; }
bool Device::had_error() const { return impl_->error; }
std::uint64_t Device::presented_frames() const { return impl_->presented; }
bool Device::has_surface() const { return impl_->surface != nullptr; }

Json Device::describe() const {
    Json j;
    j["api"] = "webgpu";
    j["implementation"] = "wgpu-native";
    j["adapter"] = impl_->adapter_name;
    j["driver"] = impl_->adapter_driver;
    j["backend"] = impl_->adapter_backend;
    j["width"] = impl_->width;
    j["height"] = impl_->height;
    j["surface"] = impl_->surface != nullptr;
    j["frames"] = impl_->frame_index;
    j["presented"] = impl_->presented;
    j["skipped_presents"] = impl_->skipped_presents;
    return j;
}

Result<WGPUShaderModule> Device::create_shader(const char* label, std::string_view wgsl) {
    WGPUShaderSourceWGSL src{};
    src.chain.sType = WGPUSType_ShaderSourceWGSL;
    src.code = WGPUStringView{wgsl.data(), wgsl.size()};
    WGPUShaderModuleDescriptor smd{};
    smd.nextInChain = &src.chain;
    smd.label = str(label);
    WGPUShaderModule m = wgpuDeviceCreateShaderModule(impl_->device, &smd);
    if (!m) return fail("gpu_shader_failed", "shader '{}' failed to compile", label);
    return m;
}

void Device::submit_so_far(Frame& frame) {
    WGPUCommandBuffer cb = wgpuCommandEncoderFinish(frame.encoder, nullptr);
    wgpuQueueSubmit(impl_->queue, 1, &cb);
    wgpuCommandBufferRelease(cb);
    wgpuCommandEncoderRelease(frame.encoder);
    frame.encoder = wgpuDeviceCreateCommandEncoder(impl_->device, nullptr);
}

void Device::push_error_scope() { wgpuDevicePushErrorScope(impl_->device, WGPUErrorFilter_Validation); }

std::string Device::pop_error_scope() {
    struct Answer {
        bool done = false;
        std::string message;
    } answer;
    WGPUPopErrorScopeCallbackInfo ci{};
    ci.mode = WGPUCallbackMode_AllowSpontaneous;
    ci.callback = [](WGPUPopErrorScopeStatus status, WGPUErrorType type, WGPUStringView message, void* u1, void*) {
        auto* a = static_cast<Answer*>(u1);
        if (status == WGPUPopErrorScopeStatus_Success && type != WGPUErrorType_NoError) a->message = to_string(message);
        a->done = true;
    };
    ci.userdata1 = &answer;
    wgpuDevicePopErrorScope(impl_->device, ci);
    for (int i = 0; i < 2000 && !answer.done; ++i) device_progress(impl_->instance, impl_->device, true);
    return answer.message;
}

WGPUBuffer Device::create_buffer(const char* label, WGPUBufferUsage usage, std::uint64_t size, const void* initial) {
    WGPUBufferDescriptor bd{};
    bd.label = str(label);
    bd.usage = usage;
    bd.size = (size + 3) / 4 * 4;
    WGPUBuffer b = wgpuDeviceCreateBuffer(impl_->device, &bd);
    if (b && initial) wgpuQueueWriteBuffer(impl_->queue, b, 0, initial, size);
    return b;
}

void Device::write_buffer(WGPUBuffer buffer, std::uint64_t offset, const void* data, std::uint64_t size) {
    wgpuQueueWriteBuffer(impl_->queue, buffer, offset, data, size);
}

}  // namespace pocket::rhi
