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
    int path = 0;              // 0 translation, 1 rotation, 2 scale
    bool step = false;         // STEP interpolation; otherwise linear (cubic splines read their values)
    std::vector<float> times;
    std::vector<float> values; // 3 per key (translation, scale) or 4 (rotation xyzw)
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
    [[nodiscard]] bool skinned() const { return !skin_vertices.empty(); }
    [[nodiscard]] const AnimationClip* clip(std::string_view name) const;
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
    [[nodiscard]] bool solid(int local_id) const;
};

struct TileLayer {
    std::string name;
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
    std::vector<TileSet> tilesets;
    std::vector<TileLayer> layers;
    std::vector<ObjectLayer> object_layers;
    Json properties;
    [[nodiscard]] const TileSet* tileset_for(std::uint32_t gid) const;
    [[nodiscard]] const TileLayer* layer(std::string_view name) const;
    // Is the tile at (x, y) of any visible layer solid (its tile or its layer says so)?
    [[nodiscard]] bool solid_at(int x, int y) const;
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
    [[nodiscard]] bool has_mesh(const std::string& path) const;
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
};

}  // namespace pocket::assets
