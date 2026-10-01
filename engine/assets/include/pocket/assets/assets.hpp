// Project assets. Paths are relative to the project directory ("assets/tree.glb"); the store
// loads each file once, keeps the decoded data, and can list and describe what a project has so
// an agent can see the material it is working with.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/math.hpp>
#include <pocket/core/result.hpp>

#include <algorithm>
#include <array>
#include <cstdint>
#include <filesystem>
#include <functional>
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
    Vec4 color{1, 1, 1, 1};   // linear; a file's vertex colors (glTF COLOR_0, OBJ's r g b after a vertex), white without
    Vec4 tangent{0, 0, 0, 0}; // along the uv's u, w the bitangent's side (glTF TANGENT, else made from the uvs); 0 for none
};

// A terrain's textured layers (docs/design/terrain.md, Layers), made with its mesh: how much of each
// lies at every sample of the grid, and what each looks like. The renderer tiles each layer's image
// over the ground and mixes them by the shares.
struct TerrainLayers {
    struct Layer {
        std::string texture;   // project-relative image; empty for plain white
        Vec3 color{1, 1, 1};   // linear, multiplies the image
        float tile = 4;        // units per repeat
    };
    std::vector<Layer> layers;           // one to four
    int n = 0;                           // samples along a side
    Vec2 size{64, 64};                   // the ground's extent along x and z
    float texture_tile = 4;              // units per repeat of the mesh's own uv
    std::vector<std::uint8_t> weights;   // n * n * 4: each layer's share at a sample (0..255, together 255), in the heights' order
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
    bool blend = false;                      // glTF alphaMode BLEND: drawn translucent, after the opaque meshes
    float alpha_cutoff = 0;                  // glTF alphaMode MASK: texels with alpha under this are cut out (0: none)
    // KHR_texture_transform on the base color texture: uv' = uv_offset + uv_scale * uv (a
    // rotation in the file is not applied). The same transform is used for the other maps.
    Vec2 uv_offset{0, 0};
    Vec2 uv_scale{1, 1};
    bool uv_transformed = false;
    std::string name;
    // Glass and lacquer (glTF KHR_materials_transmission, _ior, _volume, _clearcoat): how much light
    // passes through, bent by the index of refraction over the thickness and absorbed on the way
    // (attenuation_color left after attenuation_distance; 0 distance absorbs nothing); a clear coat
    // over the surface with its own roughness.
    float transmission = 0;
    float ior = 1.5f;
    float thickness = 0;
    Vec3 attenuation_color{1, 1, 1};
    float attenuation_distance = 0;
    float clearcoat = 0;
    float clearcoat_roughness = 0.03f;
    // Cloth, specular and brushed metal (glTF KHR_materials_sheen, _specular, _anisotropy) and
    // lighting left out (KHR_materials_unlit): a sheen's colour (linear) and roughness, the
    // dielectric reflection's strength and tint, a highlight stretched along the surface's
    // tangent by `anisotropy` (0..1) turned `anisotropy_rotation` radians from it.
    Vec3 sheen_color{0, 0, 0};
    float sheen_roughness = 0;
    float specular = 1;
    Vec3 specular_color{1, 1, 1};
    float anisotropy = 0;
    float anisotropy_rotation = 0;
    bool unlit = false;
    std::shared_ptr<const TerrainLayers> terrain;   // a terrain drawn from textured layers; null otherwise
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
    // The node whose mesh instance produced this geometry (whether baked or not); -1 for none.
    int origin = -1;
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
    int light = -1;   // index into Mesh::lights (KHR_lights_punctual), -1 for none
    int camera = -1;  // index into Mesh::cameras, -1 for none
    Json extras;      // the node's glTF extras (Blender's custom properties), null for none
};

// A light a file places on a node (KHR_lights_punctual): it shines along the node's -Z.
struct LightDef {
    std::string name;
    int type = 1;             // 0 directional, 1 point, 2 spot
    Vec3 color{1, 1, 1};      // linear
    float intensity = 1;      // as the file says (lux for directional, candela for the others, or unitless from Blender)
    float range = 0;          // 0: unlimited in the file
    float inner_cone = 0, outer_cone = 0.785398f;   // spot cone half-angles, radians
};

// A camera a file places on a node: it looks along the node's -Z.
struct CameraDef {
    std::string name;
    bool orthographic = false;
    float yfov = 0.8f;        // radians, perspective
    float ymag = 5;           // half height, orthographic
    float znear = 0.1f, zfar = 1000;
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
    bool vertex_colors = false;    // whether the file gave its vertices colors
    bool file_tangents = false;    // whether the file gave its vertices tangents (glTF TANGENT, as Blender exports them); else they are made from the uvs
    std::vector<SkinVertex> skin_vertices;  // same length as vertices when skinned() (else empty)
    std::vector<Node> nodes;
    std::vector<Skin> skins;
    std::vector<AnimationClip> animations;
    std::vector<MorphTarget> morph_targets;
    std::vector<float> default_weights;     // one per target, from the file's mesh weights (zeros otherwise)
    std::vector<LightDef> lights;
    std::vector<CameraDef> cameras;
    std::string importer = "gltf";          // how the file was read: gltf, obj, stl, or blender (converted to glTF by Blender)
    std::string converted;                  // for blender: the project-relative glTF it became
    [[nodiscard]] bool skinned() const { return !skin_vertices.empty(); }
    // A node's matrix in the file's space with the nodes at rest (identity for -1 or out of range).
    [[nodiscard]] Mat4 rest_global(int node) const;
    // Submeshes that are moving parts (Submesh::node set).
    [[nodiscard]] std::size_t moving_parts() const;
    [[nodiscard]] const AnimationClip* clip(std::string_view name) const;
    // A target's index by name, or by its index written as a string; -1 when there is none.
    [[nodiscard]] int morph_target(std::string_view name) const;
    // A node's index by name, or by its index written as a string; -1 when there is none.
    [[nodiscard]] int node_index(std::string_view name) const;
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
    // Tiled's collision editor: the rectangles drawn inside a tile, as fractions of the tile
    // (x0, y0 from its top-left corner, x1, y1 across and down). A tile with shapes is solid
    // inside them only; ellipses, points and polygons are skipped.
    struct Shape { float x0 = 0, y0 = 0, x1 = 1, y1 = 1; };
    std::map<int, std::vector<Shape>> shapes;   // local id -> rectangles
    [[nodiscard]] const std::vector<Shape>* shapes_of(int local_id) const;
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
    // A polyline's or a polygon's points, in pixels from the object's x, y (Tiled's), the polygon closed.
    std::vector<Vec2> points;
    bool closed = false;
    Json properties;
};

struct ObjectLayer {
    std::string name;
    std::vector<MapObject> objects;
    Json properties;
};

// A Tiled image layer: one picture placed in map pixels, drawn among the tile layers in file
// order, repeated across the map's extent when the file asks (docs/design/tilemaps.md, Drawing).
struct ImageLayer {
    std::string name;
    int id = 0;
    std::string image;                 // project-relative, resolved next to the map; empty draws nothing
    float offset_x = 0, offset_y = 0;  // pixels, the picture's top-left from the map's
    float opacity = 1;
    bool visible = true;
    bool repeat_x = false, repeat_y = false;
    float parallax_x = 1, parallax_y = 1;   // Tiled's parallax factors: 1 moves with the map, 0 stays with the camera
    Vec4 tint{1, 1, 1, 1};             // Tiled's tintcolor, multiplied in
    Json properties;
    int before = 0;                    // tile layers that precede it in the file: it draws after them, under the rest
};

struct TileMap {
    static constexpr std::uint32_t kFlipH = 0x80000000u, kFlipV = 0x40000000u, kFlipD = 0x20000000u, kIdMask = 0x1FFFFFFFu;
    // The solid boxes of a cell as fractions of it, for the platformer: the shapes of the tile
    // that makes the cell solid (or one-way), flipped with it, or the whole cell when that tile
    // has none; empty when the cell is neither (slopes are their own thing).
    void solid_boxes(int x, int y, std::vector<TileSet::Shape>& out) const;
    // Whether a point of a solid cell (fractions across and down from its top-left) is inside one
    // of its boxes.
    [[nodiscard]] bool solid_at_point(int x, int y, float fx, float fy) const;
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
    std::vector<ImageLayer> image_layers;
    Json properties;
    Json source;                 // the parsed Tiled document, kept so edits can be written back
    std::uint64_t revision = 0;  // bumped by every edit; renderers rebuild a layer whose revision moved
    bool file = true;            // false for a copy made at runtime: it has no file of its own until saved to one
    // The map's geometry in its own pixels (y down from the top-left of the drawn map), the way
    // Tiled draws it (docs/design/tilemaps.md, Orientations): where a cell's box (tile_width by
    // tile_height) has its top-left, the cell under a pixel (false outside the map), the drawn
    // map's size, and where an object's Tiled coordinates land (isometric objects are given in
    // the unprojected tile space, the others in pixels).
    // An infinite map (Tiled's chunks) is read as the box around its chunks, its cells and objects
    // shifted so the box starts at (0, 0); chunk_x/chunk_y is that box's origin in Tiled's tile coordinates.
    bool infinite = false;
    int chunk_x = 0, chunk_y = 0;
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
    std::vector<float> hdr;          // for a Radiance .hdr: linear float RGBA (rgba then holds it clipped and sRGB-encoded)
    [[nodiscard]] Json describe() const;
    // What it looks like, for a model that reads rather than sees: coverage, where it is drawn,
    // its colours by name with their shares, how alike its halves are, and with ascii_width over 0
    // itself in that many characters across (docs/design/assets.md, Looking without eyes).
    // `frame`: a rendered frame (everything drawn): its colours, and characters by lightness.
    [[nodiscard]] Json look(int ascii_width = 0, bool frame = false) const;
};

// Parse glTF 2.0 from memory. Every node with a mesh is baked into world space of the file
// (node transforms applied), so one file becomes one drawable with per-material submeshes.
// Skinned meshes keep their bind-space vertices with joints and weights; the node hierarchy,
// skins and animations come along so the runtime can pose them (docs/design/animation.md).
Result<Mesh> parse_gltf(const std::string& bytes, const std::filesystem::path& base_dir, const std::string& display_path);
// Tangents made from the uvs for the vertices whose file gave none (every mesh the store loads,
// and terrains).
void fill_tangents(Mesh& mesh);
Result<Image> decode_image(const std::string& bytes, const std::string& display_path);
// Wavefront OBJ with its MTL libraries (read through `read`, project-relative paths): one node per
// object, one submesh per object and material, normals smoothed where the file has none.
Result<Mesh> parse_obj(const std::string& text, const std::string& display_path, const std::function<Result<std::string>(const std::string&)>& read);
// STL, binary or ASCII: one node, one gray material, flat normals.
Result<Mesh> parse_stl(const std::string& bytes, const std::string& display_path);
// PLY, ASCII or binary: vertex positions with their normals, texture coordinates and colours when
// given (normals smoothed when not), polygon faces fanned into triangles, one gray material (white
// when the vertices are coloured).
Result<Mesh> parse_ply(const std::string& bytes, const std::string& display_path);
// Formats Blender reads and the engine converts through it (.blend, .fbx, .dae, .usd*, .abc, ...).
bool blender_format(std::string_view extension);
// Blender's executable: `configured`, else POCKET_BLENDER, the usual install places, PATH; "" for none.
// A level of detail (docs/design/rendering.md, Levels of detail): the same vertices with fewer
// triangles, each submesh's range simplified on its own (meshoptimizer), so every material keeps
// its part and a skin or morph targets still fit the vertices.
struct MeshLod {
    std::vector<std::uint32_t> indices;
    std::vector<Submesh> submeshes;   // the given ones, their ranges into `indices`
    std::uint32_t triangles = 0;
    float error = 0;                  // the largest deviation, as a fraction of the mesh's extent
};
// Vertices as float runs `stride` bytes apart: a position (three floats) at `positions`, a normal
// and a uv (five floats) at `attributes`. `ratio` is the share of each part's triangles to keep
// (0.01 to 1); parts already small keep what their shape needs.
MeshLod simplify(const float* positions, const float* attributes, std::size_t stride, std::size_t vertex_count, const std::vector<std::uint32_t>& indices, const std::vector<Submesh>& submeshes, float ratio);
MeshLod simplify(const Mesh& mesh, float ratio);

std::string find_blender(const std::string& configured);
struct Conversion {
    std::filesystem::path glb;
    bool cached = false;      // the same content was converted before (or Blender is missing and a conversion exists)
    double seconds = 0;
    std::string blender;
};
// `source` as binary glTF at `out_glb` by running Blender headless, unless a conversion of the same
// content is there already (`force` converts again).
Result<Conversion> convert_with_blender(const std::filesystem::path& source, const std::filesystem::path& out_glb, const std::string& blender, bool force);

// A height field (docs/design/terrain.md): n by n samples over size_x by size_z centred on the
// origin, heights in world units between 0 and `height`. Sample (i, j) is at x = -size_x/2 + i *
// cell_x, z = -size_z/2 + j * cell_z; every cell is two triangles split along the diagonal from
// (i, j) to (i + 1, j + 1), which sample() and normal() follow exactly, so what they answer is what
// the mesh draws and the collider stops.
struct Terrain {
    int n = 0;
    float size_x = 64, size_z = 64, height = 8;
    std::vector<float> h;   // n * n, row j (along z) after row j - 1, x across a row
    // Colour painted over the ground's own (terrain.paint): n * n of sRGB r, g, b and a weight 0..1,
    // in the order of h; empty where nothing was ever painted.
    std::vector<std::array<float, 4>> paint;
    // The textured layers' paint (terrain.paint {layer}): n * n of how much of the ground each of the
    // four layers covers there, together at most 1, in the order of h; empty where none was painted.
    std::vector<std::array<float, 4>> layer_paint;
    [[nodiscard]] float cell_x() const { return size_x / static_cast<float>(std::max(n - 1, 1)); }
    [[nodiscard]] float cell_z() const { return size_z / static_cast<float>(std::max(n - 1, 1)); }
    [[nodiscard]] float at(int i, int j) const;
    // Height and normal at local (x, z); outside the grid, at its nearest edge.
    [[nodiscard]] float sample(float x, float z) const;
    [[nodiscard]] Vec3 normal(float x, float z) const;
    // The paint at local (x, z), bilinear between samples; weight 0 where unpainted.
    [[nodiscard]] std::array<float, 4> paint_at(float x, float z) const;
    // Bilinear between samples of any n * n grid of four numbers (the paint, the layers' shares); zeros when it is not n * n.
    [[nodiscard]] std::array<float, 4> grid_at(const std::vector<std::array<float, 4>>& grid, float x, float z) const;
};

// From a greyscale image (8 or 16 bits; the first channel), resampled to n by n: black is 0, white `height`.
Result<Terrain> terrain_from_image(const std::string& bytes, const std::string& display_path, int n, Vec2 size, float height);
// Fractal noise: `octaves` layers of gradient noise, the first with features `scale` units across,
// each next twice as fine and half as tall, reproducible from `seed`, spread over 0..height.
Terrain terrain_from_noise(std::uint32_t seed, float scale, int octaves, int n, Vec2 size, float height);
// How a terrain is coloured: grass below, rock where steeper than rock_slope degrees, snow above
// snow_line (a fraction of the height), blended across their borders; uv repeats every texture_tile units.
// With layers, those colours give way to the layers' images, stacked in order: each lies by its
// rules (slope and height windows, softened at their ends) over the ones before it by `cover`,
// and the layers' paint over all of that.
struct TerrainLook {
    Vec3 grass{0.30f, 0.45f, 0.22f}, rock{0.45f, 0.42f, 0.38f}, snow{0.92f, 0.93f, 0.95f};
    float snow_line = 0.85f, rock_slope = 35.0f, texture_tile = 4.0f;
    struct Layer {
        std::string texture;
        Vec3 color{1, 1, 1};              // linear
        float tile = 4;
        Vec2 slope{0, 90};                // degrees
        Vec2 height{0, 1};                // fractions of the terrain's height
        float cover = 1;
    };
    std::vector<Layer> layers;            // up to four are used
};
// Every sample's share of each layer (together 1): the rules, then the layers' paint over them;
// empty without layers.
std::vector<std::array<float, 4>> terrain_layer_weights(const Terrain& t, const TerrainLook& look);
// The terrain as a mesh (vertex colours from the look, one grey material) at `path`.
Mesh terrain_mesh(const Terrain& t, const TerrainLook& look, const std::string& path);
// A 16-bit greyscale PNG of the heights (0 is 0, 65535 is the terrain's height): what terrain_from_image reads back.
std::string terrain_png16(const Terrain& t);
// A paint map: an RGBA image (colour in sRGB, alpha the weight), its top row at -z, resampled to n by n.
Result<std::vector<std::array<float, 4>>> terrain_paint_from_image(const std::string& bytes, const std::string& display_path, int n);
// The paint as an 8-bit RGBA PNG, n by n: what terrain_paint_from_image reads back.
std::string terrain_paint_png(const Terrain& t);
// A layer map: an RGBA image, each channel how much of the ground one layer covers, resampled to n by
// n; where the channels add up to more than 1 they are scaled to 1.
Result<std::vector<std::array<float, 4>>> terrain_layers_from_image(const std::string& bytes, const std::string& display_path, int n);
// The layers' paint as an 8-bit RGBA PNG, n by n: what terrain_layers_from_image reads back.
std::string terrain_layers_png(const Terrain& t);

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
    // A map made at runtime (tilemap.create, or one a saved scene carries) under a name, replacing
    // any map loaded under it; it has no file until saved to one.
    TileMap* put_tilemap(TileMap map);
    // The maps loaded or made so far.
    [[nodiscard]] std::vector<const TileMap*> tilemaps() const;
    [[nodiscard]] bool has_mesh(const std::string& path) const;
    // A mesh made in memory (a terrain's) under a path no file has; the next mesh(path) answers it.
    void put_mesh(const std::string& path, Mesh mesh);
    void forget_mesh(const std::string& path);
    // The simulation time the animated tiles are drawn at, in milliseconds; the session sets it
    // every tick, so the animations run on the simulation clock and pause with it.
    void set_tile_time(std::uint64_t ms) { tile_time_ms_ = ms; }
    [[nodiscard]] std::uint64_t tile_time() const { return tile_time_ms_; }
    // Forget cached data so the next access reloads from disk.
    void invalidate(const std::string& path);
    void invalidate_all();
    // Files under <project>/assets by kind (mesh, image, other) with sizes.
    [[nodiscard]] Json list() const;
    // An image also says what it looks like (coverage, where it is drawn, its colours by name, how
    // alike its halves are) and, with ascii_width, itself in characters.
    [[nodiscard]] Json describe(const std::string& path, int ascii_width = 0);
    [[nodiscard]] Json stats() const;
    [[nodiscard]] const std::filesystem::path& project_dir() const { return project_dir_; }
    // Version bumps whenever something is (re)loaded or invalidated; renderers use it to
    // drop GPU copies.
    [[nodiscard]] std::uint64_t version() const { return version_; }
    // Blender for the formats it converts: a path, or "" to look for it (docs/design/assets.md, Importing models).
    void set_blender(std::string path) { blender_config_ = std::move(path); }
    [[nodiscard]] std::string blender() const { return find_blender(blender_config_); }
    // Convert a Blender-read file now (again with force) and report it; the mesh reloads from the result.
    Result<Json> import(const std::string& path, bool force);
    // Clips from another file onto a model (docs/design/animation.md, Clips from other files): every
    // channel goes to the model's node of the same name (or the same name without a namespace:
    // "mixamorig:Hips" to "Hips"), a channel with no such node is left out, and with
    // `root_translation_only` only the skeleton's root keeps its moves (other bones keep the model's
    // lengths). A file of one clip gives it the file's name; one of several keeps their names. A clip
    // of the same name is replaced. Answers the clips added and the channels left out.
    Result<Json> add_clips(const std::string& model, const std::string& source, bool root_translation_only);

   private:
    Result<std::filesystem::path> converted_glb(const std::string& path, const std::filesystem::path& full, bool force, Conversion* report = nullptr);
    std::string blender_config_;
    std::map<std::string, std::filesystem::path> converted_;   // project path of a Blender-read file -> the glTF it became
    Result<std::filesystem::path> resolve(const std::string& path) const;
    std::filesystem::path project_dir_;
    std::map<std::string, std::unique_ptr<Mesh>> meshes_;
    std::map<std::string, std::unique_ptr<Mesh>> animation_files_;   // files of clips with nothing to draw (animation.library)
    std::map<std::string, std::unique_ptr<Image>> images_;
    std::map<std::string, std::unique_ptr<TileMap>> tilemaps_;
    std::map<std::string, std::string> failures_;
    std::uint64_t version_ = 1;
    std::uint64_t tile_time_ms_ = 0;
};

}  // namespace pocket::assets
