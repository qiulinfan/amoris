// The retained element tree.
//
// Elements are created and mutated through small operations (create, set, append, remove,
// text) that TypeScript batches per frame; layout is Yoga flexbox; painting goes through the
// Painter; input becomes UI events (click, input, keydown, wheel, hover, focus, drag) that only
// elements with listeners receive. `snapshot()` renders the tree as text so an agent can read
// the interface the way it reads the world tree.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>
#include <pocket/platform/platform.hpp>
#include <pocket/ui/painter.hpp>
#include <functional>

#include <memory>
#include <string>
#include <vector>

namespace pocket::ui {

using NodeId = std::uint32_t;

struct SnapshotOptions {
    NodeId root = 0;
    int depth = -1;
    int max_nodes = 300;
    bool layout = true;    // include rects
    bool styles = false;   // include background/color
};

class Document {
   public:
    explicit Document(Font& font);
    ~Document();
    Document(const Document&) = delete;
    Document& operator=(const Document&) = delete;

    [[nodiscard]] NodeId root() const;
    // Where a box's `image` comes from: a texture view for a project-relative path with the
    // picture's pixel size, or a null view when there is no such image. The runtime plugs the
    // renderer's textures in; without a source, images are ignored.
    struct ImageSource {
        WGPUTextureView view = nullptr;
        float width = 0, height = 0;
    };
    void set_image_source(std::function<ImageSource(const std::string&)> source);
    // Apply a batch of operations: [["create", id, type], ["set", id, props], ["append", parent, id, index?],
    // ["remove", id], ["text", id, string], ["clear", id]]. Stops at the first invalid op.
    Status apply(const Json& ops);
    Result<NodeId> create(NodeId id, std::string_view type);
    Status set(NodeId id, const Json& props);
    Status append(NodeId parent, NodeId child, int index = -1);
    Status remove(NodeId id);  // detaches and frees the subtree
    Status set_text(NodeId id, std::string_view text);
    [[nodiscard]] bool exists(NodeId id) const;
    [[nodiscard]] std::size_t node_count() const;

    // Layout the tree into width x height points at the given pixel scale.
    void layout(float width, float height, float scale);
    void paint(Painter& painter);
    // Route platform events; returns UI events for script listeners. `text_input_wanted` tells the
    // platform whether an input element has focus.
    std::vector<Json> handle_events(const std::vector<platform::Event>& events, bool& text_input_wanted);

    [[nodiscard]] NodeId hit_test(float x, float y) const;
    [[nodiscard]] Json describe(NodeId id) const;
    [[nodiscard]] Rect rect_of(NodeId id) const;
    [[nodiscard]] std::string snapshot(const SnapshotOptions& options) const;
    // Find nodes by type, text content substring and/or a `name` prop.
    [[nodiscard]] Json query(const Json& params) const;
    [[nodiscard]] NodeId focused() const;
    void set_focus(NodeId id);
    [[nodiscard]] Json stats() const;

   private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::ui
