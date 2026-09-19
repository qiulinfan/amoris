// Project assets. Paths are relative to the project directory ("assets/tree.glb"); the store
// loads each file once, keeps the decoded data, and can list and describe what a project has so
// an agent can see the material it is working with.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/math.hpp>
#include <pocket/core/result.hpp>

#include <cstdint>
#include <filesystem>
#include <map>
#include <memory>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::assets {

struct MeshVertex {
    Vec3 position;
    Vec3 normal;
    Vec2 uv;
};

struct Material {
    Vec4 base_color{1, 1, 1, 1};  // linear RGBA
    std::string texture;       // project-relative image path, empty for none
    float metallic = 0;
    float roughness = 1;
    std::string metallic_roughness_texture;  // glTF: roughness in G, metallic in B; multiplies the factors
    std::string normal_texture;              // tangent-space normal map, +Y up (glTF convention)
    float normal_scale = 1;
    std::string emissive_texture;
    Vec3 emissive{0, 0, 0};                  // linear RGB, multiplied by the emissive texture
    bool double_sided = false;
    std::string name;
};

// One draw: an index range using one material.
struct Submesh {
    std::uint32_t first_index = 0;
    std::uint32_t index_count = 0;
    std::uint32_t material = 0;
    int skin = -1;  // index into Mesh::skins when the geometry is skinned
    // The node a clip moves (or one below it), when the geometry is a moving part: its vertices
    // stay in the node's own space and the pose places them (docs/design/animation.md, Moving
    // parts); -1 for geometry baked into the file's space.
    int node = -1;
};

// Skinning data of one vertex, parallel to Mesh::vertices (zeros for unskinned geometry).
struct SkinVertex {
    std::uint16_t joints[4]{0, 0, 0, 0};
    Vec4 weights{0, 0, 0, 0};
};

// The file's node hierarchy (every node, whether or not it carries geometry), for skeletons.
struct Node {
    std::string name;
    int parent = -1;
    Vec3 translation{0, 0, 0};
    Quat rotation;
    Vec3 scale{1, 1, 1};
    Mat4 rest;  // the node's local matrix as authored (TRS above, or an explicit matrix)
    std::vector<int> children;
};

struct Skin {
    std::string name;
    std::vector<int> joints;         // node indices
    std::vector<Mat4> inverse_bind;  // one per joint
};

struct AnimationChannel {
    int node = -1;
    int path = 0;              // 0 translation, 1 rotation, 2 scale, 3 morph target weights
    int width = 3;             // values per key: 3, 4, or the number of targets for weights
    bool step = false;         // STEP interpolation; otherwise linear (cubic splines read their values)
    std::vector<float> times;
    std::vector<float> values; // width per key
};

// A morph target (blend shape): per-vertex position and normal deltas parallel to Mesh::vertices
// (zero where the target does not touch a primitive), weighted by a clip's weights track or by
// the Morph component (docs/design/animation.md, Morph targets).
struct MorphTarget {
    std::string name;
    std::vector<Vec3> positions;
    std::vector<Vec3> normals;
};

struct AnimationClip {
    std::string name;
    float duration = 0;
    std::vector<AnimationChannel> channels;
};

struct Mesh {
    std::string path;
    std::vector<MeshVertex> vertices;
    std::vector<std::uint32_t> indices;
    std::vector<Submesh> submeshes;
    std::vector<Material> materials;
    Vec3 aabb_min{0, 0, 0}, aabb_max{0, 0, 0};
    std::uint32_t node_count = 0;  // glTF nodes baked into this mesh
    std::vector<SkinVertex> skin_vertices;  // same length as vertices when skinned() (else empty)
    std::vector<Node> nodes;
    std::vector<Skin> skins;
    std::vector<AnimationClip> animations;
    std::vector<MorphTarget> morph_targets;
    std::vector<float> default_weights;     // one per target, from the file's mesh weights (zeros otherwise)
    [[nodiscard]] bool skinned() const { return !skin_vertices.empty(); }
    // A node's matrix in the file's space with the nodes at rest (identity for -1 or out of range).
    [[nodiscard]] Mat4 rest_global(int node) const;
    // Submeshes that are moving parts (Submesh::node set).
    [[nodiscard]] std::size_t moving_parts() const;
    [[nodiscard]] const AnimationClip* clip(std::string_view name) const;
    // A target's index by name, or by its index written as a string; -1 when there is none.
    [[nodiscard]] int morph_target(std::string_view name) const;
    [[nodiscard]] Json describe() const;
};

// A Tiled map (JSON, .tmj or .json; orthogonal only): tile layers of global tile ids over one or
// more tilesets, object layers for placement, properties on the map, layers and tiles.
struct TileSet {
    std::string name;
    std::uint32_t first_gid = 1;
    std::string image;          // project-relative image path
    int tile_width = 0, tile_height = 0;
    int columns = 0, tile_count = 0;
    int image_width = 0, image_height = 0;
    int spacing = 0, margin = 0;
    std::map<int, Json> tile_properties;  // local id -> {name: value}
    // Tiled's tile animations: the frames a tile shows in turn, each a local id for a duration
    // in milliseconds (docs/design/tilemaps.md, Drawing). frame_at answers the id drawn at a time
    // of the simulation clock (the tile's own id when it has no animation).
    struct Animation {
        std::vector<std::pair<int, int>> frames;   // local id, milliseconds
        int total_ms = 0;
    };
    std::map<int, Animation> animations;
    [[nodiscard]] int frame_at(int local_id, std::uint64_t time_ms) const;
    [[nodiscard]] bool solid(int local_id) const;
    [[nodiscard]] bool one_way(int local_id) const;  // solid only from above (a platform to jump through)
    [[nodiscard]] int slope(int local_id) const;     // 1: a floor rising to the right across the cell, -1: to the left, 0: none
};

struct TileLayer {
    std::string name;
    int id = 0;  // Tiled's layer id (0 when the file has none)
    std::uint64_t revision = 0;  // bumped by every edit of this layer (renderers rebuild its mesh)
    int width = 0, height = 0;
    std::vector<std::uint32_t> gids;  // row-major, 0 = empty; flip flags in the top bits
    bool visible = true;
    float opacity = 1;
    float offset_x = 0, offset_y = 0;  // pixels
    Json properties;
    [[nodiscard]] bool solid_layer() const;
};

struct MapObject {
    std::string name, type;
    float x = 0, y = 0, width = 0, height = 0;  // pixels, Tiled's top-left origin (y down)
    std::uint32_t gid = 0;
    bool point = false;
    Json properties;
};

struct ObjectLayer {
    std::string name;
    std::vector<MapObject> objects;
    Json properties;
};

struct TileMap {
    static constexpr std::uint32_t kFlipH = 0x80000000u, kFlipV = 0x40000000u, kFlipD = 0x20000000u, kIdMask = 0x1FFFFFFFu;
    std::string path;
    int width = 0, height = 0;          // tiles
    int tile_width = 0, tile_height = 0;  // pixels
    // Tiled's orientation: orthogonal, isometric, staggered (isometric with staggered rows or
    // columns) or hexagonal; the last two stagger every other row (`stagger_y`) or column, the
    // odd ones (`stagger_odd`) or the even ones, and a hexagon's flat side is `hex_side` pixels.
    std::string orientation = "orthogonal";
    int hex_side = 0;
    bool stagger_y = true, stagger_odd = true;
    std::vector<TileSet> tilesets;
    std::vector<TileLayer> layers;
    std::vector<ObjectLayer> object_layers;
    Json properties;
    Json source;                 // the parsed Tiled document, kept so edits can be written back
    std::uint64_t revision = 0;  // bumped by every edit; renderers rebuild a layer whose revision moved
    bool file = true;            // false for a copy made at runtime: it has no file of its own until saved to one
    // The map's geometry in its own pixels (y down from the top-left of the drawn map), the way
    // Tiled draws it (docs/design/tilemaps.md, Orientations): where a cell's box (tile_width by
    // tile_height) has its top-left, the cell under a pixel (false outside the map), the drawn
    // map's size, and where an object's Tiled coordinates land (isometric objects are given in
    // the unprojected tile space, the others in pixels).
    [[nodiscard]] bool orthogonal() const { return orientation == "orthogonal"; }
    [[nodiscard]] Vec2 tile_pixel(int x, int y) const;
    [[nodiscard]] bool cell_at_pixel(float px, float py, int& x, int& y) const;
    [[nodiscard]] Vec2 pixel_size() const;
    [[nodiscard]] Vec2 object_pixel(float ox, float oy) const;
    [[nodiscard]] const TileSet* tileset_for(std::uint32_t gid) const;
    [[nodiscard]] const TileLayer* layer(std::string_view name) const;
    [[nodiscard]] TileLayer* layer_mut(std::string_view name);
    // Put a gid (0 clears; flip flags allowed) into a cell of a tile layer; returns what was there.
    // Errors name an unknown layer, a cell outside the layer or a gid no tileset covers.
    Result<std::uint32_t> set(std::string_view layer_name, int x, int y, std::uint32_t gid);
    // Layers and tilesets at runtime (docs/design/tilemaps.md, Editing): a new, empty tile layer
    // of the map's size, added last (drawn on top); a layer removed; a tileset added after the
    // last (its first gid follows). Each is kept in the source document too, so save writes it.
    Result<TileLayer*> add_layer(const std::string& name, bool visible, float opacity, Json properties);
    Status remove_layer(std::string_view name);
    // Move a tile layer to a position among the tile layers (0 is drawn first, under the others),
    // in the document's order with groups flattened: a layer moved to where a group's layer is
    // joins that group.
    Status move_layer(std::string_view name, std::size_t index);
    Result<TileSet*> add_tileset(TileSet set);
    // Remove a tileset no layer uses a tile of (refused with `tileset_in_use` otherwise); the other
    // tilesets keep their ids.
    Status remove_tileset(std::string_view name);
    // The map as Tiled JSON: the source document with every tile layer's data replaced by the
    // current cells (a map built in memory gets a minimal document).
    [[nodiscard]] Json to_json() const;
    // Is the tile at (x, y) of any visible layer solid (its tile or its layer says so)?
    [[nodiscard]] bool solid_at(int x, int y) const;
    // 0 empty, 1 solid, 2 one-way (solid from above only), 3 slope (a floor whose height rises
    // across the cell; see slope_at); looks through every visible layer.
    [[nodiscard]] int solidity_at(int x, int y) const;
    // 1 for a floor rising to the right, -1 rising to the left (a horizontally flipped slope tile
    // counts the other way), 0 for no slope at the cell.
    [[nodiscard]] int slope_at(int x, int y) const;
    [[nodiscard]] Json describe() const;
};

Result<TileMap> parse_tilemap(const std::string& text, const std::string& display_path);

struct Image {
    std::string path;
    std::uint32_t width = 0, height = 0;
    std::vector<std::uint8_t> rgba;  // 8-bit RGBA, row major, top-left origin
    [[nodiscard]] Json describe() const;
};

// Parse glTF 2.0 from memory. Every node with a mesh is baked into world space of the file
// (node transforms applied), so one file becomes one drawable with per-material submeshes.
// Skinned meshes keep their bind-space vertices with joints and weights; the node hierarchy,
// skins and animations come along so the runtime can pose them (docs/design/animation.md).
Result<Mesh> parse_gltf(const std::string& bytes, const std::filesystem::path& base_dir, const std::string& display_path);
Result<Image> decode_image(const std::string& bytes, const std::string& display_path);

class AssetStore {
   public:
    explicit AssetStore(std::filesystem::path project_dir);
    // Loads (once) and returns the mesh; errors say what is wrong with the file.
    Result<const Mesh*> mesh(const std::string& path);
    Result<const Image*> image(const std::string& path);
    Result<const TileMap*> tilemap(const std::string& path);
    // The same map for editing (docs/design/tilemaps.md, Editing); every entity drawing it sees the change.
    Result<TileMap*> tilemap_mut(const std::string& path);
    // A copy of a loaded map under a new name, edited apart from it (an entity's own map); it has
    // no file until saved to one, and reloading the assets keeps it. Refused for a name in use.
    Result<TileMap*> copy_tilemap(const std::string& path, const std::string& name);
    [[nodiscard]] bool has_mesh(const std::string& path) const;
    // The simulation time the animated tiles are drawn at, in milliseconds; the session sets it
    // every tick, so the animations run on the simulation clock and pause with it.
    void set_tile_time(std::uint64_t ms) { tile_time_ms_ = ms; }
    [[nodiscard]] std::uint64_t tile_time() const { return tile_time_ms_; }
    // Forget cached data so the next access reloads from disk.
    void invalidate(const std::string& path);
    void invalidate_all();
    // Files under <project>/assets by kind (mesh, image, other) with sizes.
    [[nodiscard]] Json list() const;
    [[nodiscard]] Json describe(const std::string& path);
    [[nodiscard]] Json stats() const;
    [[nodiscard]] const std::filesystem::path& project_dir() const { return project_dir_; }
    // Version bumps whenever something is (re)loaded or invalidated; renderers use it to
    // drop GPU copies.
    [[nodiscard]] std::uint64_t version() const { return version_; }

   private:
    Result<std::filesystem::path> resolve(const std::string& path) const;
    std::filesystem::path project_dir_;
    std::map<std::string, std::unique_ptr<Mesh>> meshes_;
    std::map<std::string, std::unique_ptr<Image>> images_;
    std::map<std::string, std::unique_ptr<TileMap>> tilemaps_;
    std::map<std::string, std::string> failures_;
    std::uint64_t version_ = 1;
    std::uint64_t tile_time_ms_ = 0;
};

}  // namespace pocket::assets
