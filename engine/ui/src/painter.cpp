#include <pocket/ui/painter.hpp>

#include <pocket/core/log.hpp>

#include <cstring>

#include <map>

namespace pocket::ui {

namespace {

struct Vertex {
    float x, y;
    float u, v;
    float r, g, b, a;
    float cx, cy, hw, hh;     // rounded rect center and half size (pixels)
    float radius, mode, border, pad;
};

struct Globals {
    float viewport[2];
    float scale;
    float pad;
};

constexpr const char* kUiWgsl = R"WGSL(
struct Globals { viewport: vec2f, scale: f32, pad: f32 };
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

struct VsIn {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
    @location(3) rect: vec4f,
    @location(4) params: vec4f,
};
struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) rect: vec4f,
    @location(3) params: vec4f,
    @location(4) px: vec2f,
};

@vertex fn vs(in: VsIn) -> VsOut {
    var out: VsOut;
    let ndc = (in.pos / g.viewport) * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    out.clip = vec4f(ndc, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.rect = in.rect;
    out.params = in.params;
    out.px = in.pos;
    return out;
}

fn sd_round_box(p: vec2f, half: vec2f, r: f32) -> f32 {
    let q = abs(p) - half + vec2f(r, r);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment fn fs(in: VsOut) -> @location(0) vec4f {
    let mode = in.params.y;
    var alpha = in.color.a;
    // Sampled outside the branches: implicit-derivative sampling must stay in uniform control flow.
    let tex = textureSample(atlas, smp, in.uv);
    let coverage = tex.r;
    var rgb = in.color.rgb;
    if (mode > 0.5 && mode < 1.5) {
        alpha = alpha * coverage;
    } else if (mode > 1.5 && mode < 2.5) {
        let d = sd_round_box(in.px - in.rect.xy, in.rect.zw, in.params.x);
        alpha = alpha * (1.0 - smoothstep(-0.6, 0.6, d));
    } else if (mode > 2.5 && mode < 3.5) {
        let d = sd_round_box(in.px - in.rect.xy, in.rect.zw, in.params.x);
        let outer = 1.0 - smoothstep(-0.6, 0.6, d);
        let inner = 1.0 - smoothstep(-0.6, 0.6, d + in.params.z);
        alpha = alpha * max(outer - inner, 0.0);
    } else if (mode > 3.5) {
        // An image: the texture's color and alpha under the tint, inside rounded corners when asked.
        rgb = rgb * tex.rgb;
        alpha = alpha * tex.a;
        if (in.params.x > 0.0) {
            let d = sd_round_box(in.px - in.rect.xy, in.rect.zw, in.params.x);
            alpha = alpha * (1.0 - smoothstep(-0.6, 0.6, d));
        }
    }
    if (alpha <= 0.001) { discard; }
    return vec4f(rgb * alpha, alpha);
}
)WGSL";

struct DrawRange {
    std::uint32_t first_index = 0, index_count = 0;
    Rect clip;  // in pixels
    WGPUTextureView view = nullptr;  // an image's texture; null draws from the font atlas
};

}  // namespace

struct Painter::Impl {
    rhi::Device* device = nullptr;
    Font* font = nullptr;
    WGPUShaderModule shader = nullptr;
    WGPUBindGroupLayout bgl = nullptr;
    WGPUPipelineLayout layout = nullptr;
    WGPURenderPipeline pipeline = nullptr;
    WGPUSampler sampler = nullptr;
    WGPUBuffer globals = nullptr;
    WGPUBuffer vbuf = nullptr;
    WGPUBuffer ibuf = nullptr;
    std::uint64_t vbuf_size = 0, ibuf_size = 0;
    WGPUBindGroup bind_group = nullptr;
    WGPUTextureView bound_view = nullptr;
    std::map<WGPUTextureView, WGPUBindGroup> image_groups;   // this frame's image textures, released at the next begin()
    std::vector<Vertex> vertices;
    std::vector<std::uint32_t> indices;
    std::vector<DrawRange> ranges;
    std::vector<Rect> clip_stack;
    float width = 0, height = 0, scale = 1;
    std::uint32_t draws = 0;

    ~Impl() {
        if (bind_group) wgpuBindGroupRelease(bind_group);
        if (ibuf) wgpuBufferRelease(ibuf);
        if (vbuf) wgpuBufferRelease(vbuf);
        if (globals) wgpuBufferRelease(globals);
        if (sampler) wgpuSamplerRelease(sampler);
        if (pipeline) wgpuRenderPipelineRelease(pipeline);
        if (layout) wgpuPipelineLayoutRelease(layout);
        if (bgl) wgpuBindGroupLayoutRelease(bgl);
        if (shader) wgpuShaderModuleRelease(shader);
    }

    Status init() {
        POCKET_TRY(module, device->create_shader("pocket.ui", kUiWgsl));
        shader = module;
        WGPUBindGroupLayoutEntry entries[3]{};
        entries[0].binding = 0;
        entries[0].visibility = WGPUShaderStage_Vertex | WGPUShaderStage_Fragment;
        entries[0].buffer.type = WGPUBufferBindingType_Uniform;
        entries[0].buffer.minBindingSize = sizeof(Globals);
        entries[1].binding = 1;
        entries[1].visibility = WGPUShaderStage_Fragment;
        entries[1].texture.sampleType = WGPUTextureSampleType_Float;
        entries[1].texture.viewDimension = WGPUTextureViewDimension_2D;
        entries[2].binding = 2;
        entries[2].visibility = WGPUShaderStage_Fragment;
        entries[2].sampler.type = WGPUSamplerBindingType_Filtering;
        WGPUBindGroupLayoutDescriptor bd{};
        bd.label = rhi::str("pocket.ui");
        bd.entryCount = 3;
        bd.entries = entries;
        bgl = wgpuDeviceCreateBindGroupLayout(device->device(), &bd);
        WGPUPipelineLayoutDescriptor pld{};
        pld.label = rhi::str("pocket.ui");
        pld.bindGroupLayoutCount = 1;
        pld.bindGroupLayouts = &bgl;
        layout = wgpuDeviceCreatePipelineLayout(device->device(), &pld);

        WGPUVertexAttribute attrs[5]{};
        attrs[0] = {nullptr, WGPUVertexFormat_Float32x2, 0, 0};
        attrs[1] = {nullptr, WGPUVertexFormat_Float32x2, sizeof(float) * 2, 1};
        attrs[2] = {nullptr, WGPUVertexFormat_Float32x4, sizeof(float) * 4, 2};
        attrs[3] = {nullptr, WGPUVertexFormat_Float32x4, sizeof(float) * 8, 3};
        attrs[4] = {nullptr, WGPUVertexFormat_Float32x4, sizeof(float) * 12, 4};
        WGPUVertexBufferLayout vbl{};
        vbl.stepMode = WGPUVertexStepMode_Vertex;
        vbl.arrayStride = sizeof(Vertex);
        vbl.attributeCount = 5;
        vbl.attributes = attrs;

        WGPUBlendState blend{};
        blend.color.operation = WGPUBlendOperation_Add;
        blend.color.srcFactor = WGPUBlendFactor_One;  // premultiplied output
        blend.color.dstFactor = WGPUBlendFactor_OneMinusSrcAlpha;
        blend.alpha.operation = WGPUBlendOperation_Add;
        blend.alpha.srcFactor = WGPUBlendFactor_One;
        blend.alpha.dstFactor = WGPUBlendFactor_OneMinusSrcAlpha;
        WGPUColorTargetState target{};
        target.format = device->color_format();
        target.blend = &blend;
        target.writeMask = WGPUColorWriteMask_All;
        WGPUFragmentState fs{};
        fs.module = shader;
        fs.entryPoint = rhi::str("fs");
        fs.targetCount = 1;
        fs.targets = &target;
        WGPURenderPipelineDescriptor rpd{};
        rpd.label = rhi::str("pocket.ui");
        rpd.layout = layout;
        rpd.vertex.module = shader;
        rpd.vertex.entryPoint = rhi::str("vs");
        rpd.vertex.bufferCount = 1;
        rpd.vertex.buffers = &vbl;
        rpd.primitive.topology = WGPUPrimitiveTopology_TriangleList;
        rpd.primitive.frontFace = WGPUFrontFace_CCW;
        rpd.primitive.cullMode = WGPUCullMode_None;
        rpd.multisample.count = 1;
        rpd.multisample.mask = 0xFFFFFFFFu;
        rpd.fragment = &fs;
        pipeline = wgpuDeviceCreateRenderPipeline(device->device(), &rpd);
        if (!pipeline) return fail("gpu_pipeline_failed", "ui pipeline creation failed");

        WGPUSamplerDescriptor sd{};
        sd.label = rhi::str("pocket.ui");
        sd.addressModeU = WGPUAddressMode_ClampToEdge;
        sd.addressModeV = WGPUAddressMode_ClampToEdge;
        sd.addressModeW = WGPUAddressMode_ClampToEdge;
        sd.magFilter = WGPUFilterMode_Linear;
        sd.minFilter = WGPUFilterMode_Linear;
        sd.mipmapFilter = WGPUMipmapFilterMode_Nearest;
        sd.lodMaxClamp = 32.0f;
        sd.maxAnisotropy = 1;
        sampler = wgpuDeviceCreateSampler(device->device(), &sd);
        globals = device->create_buffer("pocket.ui.globals", WGPUBufferUsage_Uniform | WGPUBufferUsage_CopyDst, sizeof(Globals));
        return {};
    }

    void ensure_bind_group(WGPUTextureView view) {
        if (bind_group && bound_view == view) return;
        if (bind_group) wgpuBindGroupRelease(bind_group);
        WGPUBindGroupEntry entries[3]{};
        entries[0].binding = 0;
        entries[0].buffer = globals;
        entries[0].size = sizeof(Globals);
        entries[1].binding = 1;
        entries[1].textureView = view;
        entries[2].binding = 2;
        entries[2].sampler = sampler;
        WGPUBindGroupDescriptor bgd{};
        bgd.label = rhi::str("pocket.ui");
        bgd.layout = bgl;
        bgd.entryCount = 3;
        bgd.entries = entries;
        bind_group = wgpuDeviceCreateBindGroup(device->device(), &bgd);
        bound_view = view;
    }

    WGPUBindGroup group_for(WGPUTextureView view) {
        if (auto it = image_groups.find(view); it != image_groups.end()) return it->second;
        WGPUBindGroupEntry entries[3]{};
        entries[0].binding = 0;
        entries[0].buffer = globals;
        entries[0].size = sizeof(Globals);
        entries[1].binding = 1;
        entries[1].textureView = view;
        entries[2].binding = 2;
        entries[2].sampler = sampler;
        WGPUBindGroupDescriptor bgd{};
        bgd.label = rhi::str("pocket.ui.image");
        bgd.layout = bgl;
        bgd.entryCount = 3;
        bgd.entries = entries;
        WGPUBindGroup g = wgpuDeviceCreateBindGroup(device->device(), &bgd);
        image_groups[view] = g;
        return g;
    }

    void release_image_groups() {
        for (auto& [view, g] : image_groups) wgpuBindGroupRelease(g);
        image_groups.clear();
    }

    Rect clip_px() const {
        if (clip_stack.empty()) return {0, 0, width * scale, height * scale};
        const Rect& c = clip_stack.back();
        return {c.x * scale, c.y * scale, c.w * scale, c.h * scale};
    }

    void new_range_if_needed(WGPUTextureView view) {
        Rect clip = clip_px();
        if (!ranges.empty()) {
            DrawRange& last = ranges.back();
            if (last.view == view && last.clip.x == clip.x && last.clip.y == clip.y && last.clip.w == clip.w && last.clip.h == clip.h) return;
            if (last.index_count == 0) {
                last.clip = clip;
                last.view = view;
                return;
            }
        }
        ranges.push_back({static_cast<std::uint32_t>(indices.size()), 0, clip, view});
    }

    void quad(const Vertex& a, const Vertex& b, const Vertex& c, const Vertex& d, WGPUTextureView view = nullptr) {
        new_range_if_needed(view);
        auto base = static_cast<std::uint32_t>(vertices.size());
        vertices.insert(vertices.end(), {a, b, c, d});
        indices.insert(indices.end(), {base, base + 1, base + 2, base, base + 2, base + 3});
        ranges.back().index_count += 6;
    }

    void box(const Rect& r_points, Color color, float radius_points, float mode, float border_points) {
        float x = r_points.x * scale, y = r_points.y * scale, w = r_points.w * scale, h = r_points.h * scale;
        if (w <= 0 || h <= 0) return;
        float cx = x + w * 0.5f, cy = y + h * 0.5f, hw = w * 0.5f, hh = h * 0.5f;
        float radius = std::min(radius_points * scale, std::min(hw, hh));
        float border = border_points * scale;
        // Expand by one pixel for anti-aliasing when using the SDF modes.
        float e = mode >= 2.0f ? 1.0f : 0.0f;
        Vertex v{};
        v.r = color.r; v.g = color.g; v.b = color.b; v.a = color.a;
        v.cx = cx; v.cy = cy; v.hw = hw; v.hh = hh;
        v.radius = radius; v.mode = mode; v.border = border;
        v.u = 0.0005f; v.v = 0.0005f;
        Vertex a = v, b = v, c = v, d = v;
        a.x = x - e; a.y = y - e;
        b.x = x + w + e; b.y = y - e;
        c.x = x + w + e; c.y = y + h + e;
        d.x = x - e; d.y = y + h + e;
        quad(a, b, c, d);
    }
};

Painter::Painter() : impl_(std::make_unique<Impl>()) {}
Painter::~Painter() {
    if (impl_) impl_->release_image_groups();
}

Result<std::unique_ptr<Painter>> Painter::create(rhi::Device& device, Font& font) {
    std::unique_ptr<Painter> p(new Painter());
    p->impl_->device = &device;
    p->impl_->font = &font;
    POCKET_TRY_VOID(p->impl_->init());
    return p;
}

void Painter::begin(float width_points, float height_points, float scale) {
    Impl& im = *impl_;
    im.width = width_points;
    im.height = height_points;
    im.scale = scale;
    im.vertices.clear();
    im.indices.clear();
    im.ranges.clear();
    im.clip_stack.clear();
    im.draws = 0;
    im.release_image_groups();
}

void Painter::rect(const Rect& r, Color color, float radius) {
    impl_->box(r, color, radius, radius > 0 ? 2.0f : 0.0f, 0);
}

void Painter::border(const Rect& r, Color color, float thickness, float radius) {
    impl_->box(r, color, radius, 3.0f, thickness);
}

void Painter::image(const Rect& r_points, WGPUTextureView view, float u0, float v0, float u1, float v1, Color tint, float radius_points) {
    Impl& im = *impl_;
    if (!view) return;
    const float x = r_points.x * im.scale, y = r_points.y * im.scale, w = r_points.w * im.scale, h = r_points.h * im.scale;
    if (w <= 0 || h <= 0) return;
    Vertex v{};
    v.r = tint.r; v.g = tint.g; v.b = tint.b; v.a = tint.a;
    v.cx = x + w * 0.5f; v.cy = y + h * 0.5f; v.hw = w * 0.5f; v.hh = h * 0.5f;
    v.radius = std::min(radius_points * im.scale, std::min(v.hw, v.hh));
    v.mode = 4.0f;
    Vertex a = v, b = v, c = v, d = v;
    a.x = x; a.y = y; a.u = u0; a.v = v0;
    b.x = x + w; b.y = y; b.u = u1; b.v = v0;
    c.x = x + w; c.y = y + h; c.u = u1; c.v = v1;
    d.x = x; d.y = y + h; d.u = u0; d.v = v1;
    im.quad(a, b, c, d, view);
}

void Painter::line(float x0, float y0, float x1, float y1, Color color, float thickness) {
    if (x0 == x1) rect({x0 - thickness * 0.5f, std::min(y0, y1), thickness, std::fabs(y1 - y0)}, color);
    else if (y0 == y1) rect({std::min(x0, x1), y0 - thickness * 0.5f, std::fabs(x1 - x0), thickness}, color);
    else {
        // Diagonal: draw as a thin rotated quad.
        Impl& im = *impl_;
        float s = im.scale;
        float dx = (x1 - x0) * s, dy = (y1 - y0) * s;
        float len = std::sqrt(dx * dx + dy * dy);
        if (len <= 0) return;
        float nx = -dy / len * thickness * s * 0.5f, ny = dx / len * thickness * s * 0.5f;
        Vertex v{};
        v.r = color.r; v.g = color.g; v.b = color.b; v.a = color.a;
        v.u = 0.0005f; v.v = 0.0005f;
        Vertex a = v, b = v, c = v, d = v;
        a.x = x0 * s + nx; a.y = y0 * s + ny;
        b.x = x1 * s + nx; b.y = y1 * s + ny;
        c.x = x1 * s - nx; c.y = y1 * s - ny;
        d.x = x0 * s - nx; d.y = y0 * s - ny;
        im.quad(a, b, c, d);
    }
}

float Painter::text(float x, float y, std::string_view text, float size_points, Color color) {
    Impl& im = *impl_;
    float px = size_points * im.scale;
    TextMetrics m = im.font->metrics(px);
    float baseline = y * im.scale + m.ascent;
    float pen = x * im.scale;
    // Snap the pen to whole pixels so glyph bitmaps are not resampled.
    pen = std::round(pen);
    baseline = std::round(baseline);
    auto run = im.font->shape(text, px);
    for (const ShapedGlyph& sg : run) {
        const Glyph& g = sg.glyph;
        if (!g.empty) {
            float gx = pen + sg.x + g.bearing_x;
            float gy = baseline - g.bearing_y + sg.y;
            Vertex v{};
            v.r = color.r; v.g = color.g; v.b = color.b; v.a = color.a;
            v.mode = 1.0f;
            Vertex a = v, b = v, c = v, d = v;
            a.x = gx; a.y = gy; a.u = g.u0; a.v = g.v0;
            b.x = gx + g.width; b.y = gy; b.u = g.u1; b.v = g.v0;
            c.x = gx + g.width; c.y = gy + g.height; c.u = g.u1; c.v = g.v1;
            d.x = gx; d.y = gy + g.height; d.u = g.u0; d.v = g.v1;
            im.quad(a, b, c, d);
        }
    }
    float width_px = run.empty() ? 0.0f : run.back().x + run.back().glyph.advance;
    return width_px / im.scale;
}

float Painter::text_aligned(const Rect& box, std::string_view text, float size_points, Color color, TextAlign align, bool vcenter) {
    float w = measure(text, size_points);
    float lh = line_height(size_points);
    float x = box.x;
    if (align == TextAlign::Center) x = box.x + (box.w - w) * 0.5f;
    else if (align == TextAlign::Right) x = box.x + box.w - w;
    float y = vcenter ? box.y + (box.h - lh) * 0.5f : box.y;
    return this->text(x, y, text, size_points, color);
}

float Painter::measure(std::string_view text, float size_points) {
    return impl_->font->measure(text, size_points * impl_->scale) / impl_->scale;
}

float Painter::line_height(float size_points) {
    return impl_->font->metrics(size_points * impl_->scale).line_height / impl_->scale;
}

void Painter::push_clip(const Rect& r) {
    Rect c = impl_->clip_stack.empty() ? r : impl_->clip_stack.back().intersect(r);
    impl_->clip_stack.push_back(c);
}

void Painter::pop_clip() {
    if (!impl_->clip_stack.empty()) impl_->clip_stack.pop_back();
}

Rect Painter::current_clip() const {
    if (impl_->clip_stack.empty()) return {0, 0, impl_->width, impl_->height};
    return impl_->clip_stack.back();
}

Status Painter::flush(rhi::Frame& frame) {
    Impl& im = *impl_;
    if (im.indices.empty()) return {};
    WGPUTextureView atlas = im.font->atlas_view();
    im.ensure_bind_group(atlas);
    Globals g{};
    g.viewport[0] = static_cast<float>(frame.width);
    g.viewport[1] = static_cast<float>(frame.height);
    g.scale = im.scale;
    im.device->write_buffer(im.globals, 0, &g, sizeof g);
    std::uint64_t vbytes = im.vertices.size() * sizeof(Vertex);
    std::uint64_t ibytes = im.indices.size() * sizeof(std::uint32_t);
    if (!im.vbuf || im.vbuf_size < vbytes) {
        if (im.vbuf) wgpuBufferRelease(im.vbuf);
        im.vbuf_size = std::max<std::uint64_t>(vbytes * 2, 64 * 1024);
        im.vbuf = im.device->create_buffer("pocket.ui.vertices", WGPUBufferUsage_Vertex | WGPUBufferUsage_CopyDst, im.vbuf_size);
    }
    if (!im.ibuf || im.ibuf_size < ibytes) {
        if (im.ibuf) wgpuBufferRelease(im.ibuf);
        im.ibuf_size = std::max<std::uint64_t>(ibytes * 2, 64 * 1024);
        im.ibuf = im.device->create_buffer("pocket.ui.indices", WGPUBufferUsage_Index | WGPUBufferUsage_CopyDst, im.ibuf_size);
    }
    im.device->write_buffer(im.vbuf, 0, im.vertices.data(), vbytes);
    im.device->write_buffer(im.ibuf, 0, im.indices.data(), ibytes);

    WGPURenderPassColorAttachment ca{};
    ca.view = frame.color;
    ca.depthSlice = WGPU_DEPTH_SLICE_UNDEFINED;
    ca.loadOp = WGPULoadOp_Load;
    ca.storeOp = WGPUStoreOp_Store;
    WGPURenderPassDescriptor rp{};
    rp.label = rhi::str("pocket.ui");
    rp.colorAttachmentCount = 1;
    rp.colorAttachments = &ca;
    WGPURenderPassEncoder pass = wgpuCommandEncoderBeginRenderPass(frame.encoder, &rp);
    wgpuRenderPassEncoderSetPipeline(pass, im.pipeline);
    wgpuRenderPassEncoderSetBindGroup(pass, 0, im.bind_group, 0, nullptr);
    wgpuRenderPassEncoderSetVertexBuffer(pass, 0, im.vbuf, 0, vbytes);
    wgpuRenderPassEncoderSetIndexBuffer(pass, im.ibuf, WGPUIndexFormat_Uint32, 0, ibytes);
    for (const DrawRange& r : im.ranges) {
        if (r.index_count == 0) continue;
        auto sx = static_cast<std::uint32_t>(std::max(0.0f, std::floor(r.clip.x)));
        auto sy = static_cast<std::uint32_t>(std::max(0.0f, std::floor(r.clip.y)));
        auto ex = static_cast<std::uint32_t>(std::min(static_cast<float>(frame.width), std::ceil(r.clip.x + r.clip.w)));
        auto ey = static_cast<std::uint32_t>(std::min(static_cast<float>(frame.height), std::ceil(r.clip.y + r.clip.h)));
        if (ex <= sx || ey <= sy) continue;
        wgpuRenderPassEncoderSetBindGroup(pass, 0, r.view ? im.group_for(r.view) : im.bind_group, 0, nullptr);
        wgpuRenderPassEncoderSetScissorRect(pass, sx, sy, ex - sx, ey - sy);
        wgpuRenderPassEncoderDrawIndexed(pass, r.index_count, 1, r.first_index, 0, 0);
        im.draws++;
    }
    wgpuRenderPassEncoderEnd(pass);
    wgpuRenderPassEncoderRelease(pass);
    return {};
}

std::uint32_t Painter::vertex_count() const { return static_cast<std::uint32_t>(impl_->vertices.size()); }
std::uint32_t Painter::draw_count() const { return impl_->draws; }
float Painter::scale() const { return impl_->scale; }

}  // namespace pocket::ui
