// Renders the world into the device's offscreen target and an entity id buffer.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/world/world.hpp>

#include <memory>
#include <vector>

namespace pocket::renderer {

struct RenderStats {
    std::uint32_t draw_calls = 0;
    std::uint32_t meshes = 0;
    std::uint32_t point_lights = 0;
    bool has_camera = false;
    bool has_sun = false;
    world::EntityId camera = 0;
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
};

class Renderer {
   public:
    static Result<std::unique_ptr<Renderer>> create(rhi::Device& device);
    ~Renderer();
    Renderer(const Renderer&) = delete;
    Renderer& operator=(const Renderer&) = delete;

    // Draw the world into the frame (color + depth from the frame, ids into the renderer's own
    // target). Must be called between Device::begin_frame and Device::end_frame.
    Status render(rhi::Frame& frame, const world::World& world, rhi::Color clear);
    // Read back the id buffer of the last rendered frame.
    Result<IdImage> read_ids();
    // Entity under a pixel (0 when background). Reads back the whole id buffer.
    Result<world::EntityId> pick(std::uint32_t x, std::uint32_t y);
    [[nodiscard]] const RenderStats& stats() const;
    [[nodiscard]] Json describe() const;
    // Camera used by the last frame (default camera when the world has none).
    [[nodiscard]] const CameraView& camera() const;
    // Project a world point to pixel coordinates using the last frame's camera; false if behind.
    [[nodiscard]] bool project(Vec3 world_pos, float& out_x, float& out_y) const;
    void set_viewport(Viewport v);
    [[nodiscard]] Viewport viewport() const;
    // The viewport actually used by the last frame (clamped to the frame).
    [[nodiscard]] Viewport applied_viewport() const;

   private:
    Renderer();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::renderer
