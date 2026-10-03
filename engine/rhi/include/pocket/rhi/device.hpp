// GPU device, frame lifecycle, capture.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>

#include <webgpu/webgpu.h>

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace pocket::rhi {

struct Config {
    void* metal_layer = nullptr;  // CAMetalLayer* on macOS; null for headless
    void* x11_display = nullptr;  // Linux: an X11 display and window, or a Wayland display and surface
    std::uint64_t x11_window = 0;
    void* wayland_display = nullptr;
    void* wayland_surface = nullptr;
    void* win32_hwnd = nullptr;   // Windows: the window's HWND and HINSTANCE
    void* win32_hinstance = nullptr;
    std::string canvas_selector;  // web: the <canvas> to present into ("#canvas"); empty for headless
    std::uint32_t width = 640;
    std::uint32_t height = 360;
    bool prefer_high_performance = true;
    bool vsync = true;
};

struct Color {
    float r = 0, g = 0, b = 0, a = 1;
};

struct Image {
    std::uint32_t width = 0, height = 0;
    std::vector<std::uint8_t> rgba;  // tightly packed, top-left origin
};

struct Frame {
    WGPUCommandEncoder encoder = nullptr;
    WGPUTextureView color = nullptr;  // offscreen RGBA8 target
    WGPUTexture color_texture = nullptr;  // the texture behind `color`, for a pass that copies the frame aside
    WGPUTextureView depth = nullptr;  // Depth24Plus
    std::uint32_t width = 0, height = 0;
    std::uint64_t index = 0;
};

inline WGPUStringView str(const char* s) { return WGPUStringView{s, WGPU_STRLEN}; }
std::string to_string(WGPUStringView s);

class Device {
   public:
    static Result<std::unique_ptr<Device>> create(const Config& config);
    ~Device();
    Device(const Device&) = delete;
    Device& operator=(const Device&) = delete;

    Status resize(std::uint32_t width, std::uint32_t height);
    Result<Frame> begin_frame();
    // Begin the main color+depth pass on the offscreen target. The caller ends the pass.
    WGPURenderPassEncoder begin_main_pass(Frame& frame, Color clear);
    // Submit; when a surface exists, blit the offscreen target to it and present.
    Status end_frame(Frame& frame);
    // Read back the offscreen target of the last submitted frame.
    Result<Image> capture();
    // Read back any RGBA8 texture with CopySrc usage (after the work that fills it was submitted).
    Result<Image> read_texture(WGPUTexture texture, std::uint32_t width, std::uint32_t height);
    // Let the GPU make progress: process callbacks (and on the web, yield to the page).
    void poll(bool wait);

    [[nodiscard]] WGPUDevice device() const;
    [[nodiscard]] WGPUQueue queue() const;
    [[nodiscard]] WGPUInstance instance() const;
    [[nodiscard]] WGPUTextureFormat color_format() const { return WGPUTextureFormat_RGBA8Unorm; }
    [[nodiscard]] WGPUTextureFormat depth_format() const { return WGPUTextureFormat_Depth24Plus; }
    [[nodiscard]] std::uint32_t width() const;
    [[nodiscard]] std::uint32_t height() const;
    [[nodiscard]] bool had_error() const;
    [[nodiscard]] std::uint64_t presented_frames() const;
    [[nodiscard]] bool has_surface() const;
    // Whether the device writes timestamps at the start and end of passes (the timestamp-query
    // feature), and nanoseconds per resolved timestamp unit.
    [[nodiscard]] bool timestamps() const;
    [[nodiscard]] float timestamp_period() const;
    // Frames submitted so far (end_frame calls): work recorded before the last one is on the GPU.
    [[nodiscard]] std::uint64_t submitted_frames() const;
    [[nodiscard]] Json describe() const;
    // What the last device to go was still held by as it went, by kind ("2 textures, 3 texture
    // views"; empty for nothing): an object a subsystem never released keeps the whole device, and
    // its memory, alive. POCKET_GPU_REPORT=1 prints it as each device goes. Native only.
    [[nodiscard]] static std::string held_when_last_destroyed();

    // Helpers shared by renderers.
    Result<WGPUShaderModule> create_shader(const char* label, std::string_view wgsl);
    // Validation errors of what is made between the two caught rather than reported as the device's:
    // pop answers the first one's message (empty when there was none), waiting for it. For code a
    // user wrote (a post effect's WGSL), whose mistakes are theirs to read and fix.
    // Submit what the frame recorded so far and go on in a new encoder: what is written to buffers
    // with the queue after this reaches only what is recorded after it (one camera's uniforms, then
    // the next's).
    void submit_so_far(Frame& frame);
    void push_error_scope();
    std::string pop_error_scope();
    WGPUBuffer create_buffer(const char* label, WGPUBufferUsage usage, std::uint64_t size, const void* initial = nullptr);
    void write_buffer(WGPUBuffer buffer, std::uint64_t offset, const void* data, std::uint64_t size);

   private:
    Device();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::rhi
