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
    bool double_sided = false;
    std::string name;
};

// One draw: an index range using one material.
struct Submesh {
    std::uint32_t first_index = 0;
    std::uint32_t index_count = 0;
    std::uint32_t material = 0;
};

struct Mesh {
    std::string path;
    std::vector<MeshVertex> vertices;
    std::vector<std::uint32_t> indices;
    std::vector<Submesh> submeshes;
    std::vector<Material> materials;
    Vec3 aabb_min{0, 0, 0}, aabb_max{0, 0, 0};
    std::uint32_t node_count = 0;  // glTF nodes baked into this mesh
    [[nodiscard]] Json describe() const;
};

struct Image {
    std::string path;
    std::uint32_t width = 0, height = 0;
    std::vector<std::uint8_t> rgba;  // 8-bit RGBA, row major, top-left origin
    [[nodiscard]] Json describe() const;
};

// Parse glTF 2.0 from memory. Every node with a mesh is baked into world space of the file
// (node transforms applied), so one file becomes one drawable with per-material submeshes.
Result<Mesh> parse_gltf(const std::string& bytes, const std::filesystem::path& base_dir, const std::string& display_path);
Result<Image> decode_image(const std::string& bytes, const std::string& display_path);

class AssetStore {
   public:
    explicit AssetStore(std::filesystem::path project_dir);
    // Loads (once) and returns the mesh; errors say what is wrong with the file.
    Result<const Mesh*> mesh(const std::string& path);
    Result<const Image*> image(const std::string& path);
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
    std::map<std::string, std::string> failures_;
    std::uint64_t version_ = 1;
};

}  // namespace pocket::assets
