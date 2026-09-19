#include <pocket/assets/assets.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <filesystem>

using namespace pocket;

namespace {

std::filesystem::path root() {
    const char* r = std::getenv("POCKET_ROOT");
    REQUIRE(r != nullptr);
    return r;
}

std::filesystem::path project() { return root() / "samples" / "assets"; }

}  // namespace

TEST_CASE("glb with two nodes, normals, uvs and a texture", "[assets]") {
    assets::AssetStore store(project());
    auto m = store.mesh("assets/crate.glb");
    REQUIRE(m.has_value());
    const assets::Mesh& mesh = **m;
    REQUIRE(mesh.node_count == 2);
    REQUIRE(mesh.vertices.size() == 48);   // 24 per cube instance
    REQUIRE(mesh.indices.size() == 72);    // 12 triangles per instance
    REQUIRE(mesh.submeshes.size() == 2);
    REQUIRE(mesh.materials.size() == 1);
    REQUIRE(mesh.materials[0].texture == "assets/checker.png");
    REQUIRE(mesh.materials[0].base_color.y == Catch::Approx(0.9f));
    REQUIRE(mesh.materials[0].metallic == Catch::Approx(0.0f));
    // The second node is translated by +1.5 and scaled by 0.5: the bounds reach 1.75 on X.
    REQUIRE(mesh.aabb_min.x == Catch::Approx(-0.5f));
    REQUIRE(mesh.aabb_max.x == Catch::Approx(1.75f));
    REQUIRE(mesh.aabb_max.y == Catch::Approx(0.5f));
    // Normals survive the bake and stay unit length; uvs are inside [0, 1].
    for (const auto& v : mesh.vertices) {
        REQUIRE(length(v.normal) == Catch::Approx(1.0f).margin(1e-4));
        REQUIRE(v.uv.x >= 0.0f); REQUIRE(v.uv.x <= 1.0f);
        REQUIRE(v.uv.y >= 0.0f); REQUIRE(v.uv.y <= 1.0f);
    }
    Json d = mesh.describe();
    REQUIRE(d["triangles"] == 24);
    REQUIRE(d["materials"][0]["texture"] == "assets/checker.png");
    // Cached: the same pointer comes back and the store's version is unchanged.
    std::uint64_t version = store.version();
    auto again = store.mesh("assets/crate.glb");
    REQUIRE(*again == *m);
    REQUIRE(store.version() == version);
}

TEST_CASE("a skinned glb keeps joints, weights, the skeleton and its clips", "[assets][animation]") {
    assets::AssetStore store(project());
    auto m = store.mesh("assets/arm.glb");
    REQUIRE(m.has_value());
    const assets::Mesh& mesh = **m;
    REQUIRE(mesh.skinned());
    REQUIRE(mesh.skin_vertices.size() == mesh.vertices.size());
    REQUIRE(mesh.nodes.size() == 3);
    REQUIRE(mesh.nodes[2].parent == 1);
    REQUIRE(mesh.nodes[2].translation.y == Catch::Approx(1.0f));
    REQUIRE(mesh.skins.size() == 1);
    REQUIRE(mesh.skins[0].joints == std::vector<int>{1, 2});
    REQUIRE(mesh.skins[0].inverse_bind[1].at(3, 1) == Catch::Approx(-1.0f));   // tip bind pose is 1 up
    REQUIRE(mesh.submeshes.size() == 1);
    REQUIRE(mesh.submeshes[0].skin == 0);
    REQUIRE(mesh.animations.size() == 2);
    REQUIRE(mesh.clip("wave") != nullptr);
    REQUIRE(mesh.clip("wave")->duration == Catch::Approx(1.0f));
    REQUIRE(mesh.clip("wave")->channels[0].path == 1);
    REQUIRE(mesh.clip("wave")->channels[0].times.size() == 3);
    REQUIRE(mesh.clip("nod")->duration == Catch::Approx(1.5f));
    // Weights are normalized and the top ring belongs to the tip joint, the bottom to the root.
    for (const auto& sv : mesh.skin_vertices) {
        float sum = sv.weights.x + sv.weights.y + sv.weights.z + sv.weights.w;
        REQUIRE(sum == Catch::Approx(1.0f).margin(1e-5));
    }
    REQUIRE(mesh.skin_vertices.front().weights.x == Catch::Approx(1.0f));
    REQUIRE(mesh.skin_vertices.back().weights.y == Catch::Approx(1.0f));
    // The skinned node's own transform is not baked: vertices stay in bind space (y from 0 to 2).
    REQUIRE(mesh.aabb_min.y == Catch::Approx(0.0f));
    REQUIRE(mesh.aabb_max.y == Catch::Approx(2.0f));
    Json d = mesh.describe();
    REQUIRE(d["skinned"] == true);
    REQUIRE(d["animations"].size() == 2);
}

TEST_CASE("a Tiled map parses layers, tilesets, flips, properties and objects", "[assets][tilemap]") {
    assets::AssetStore store(root() / "samples" / "sprites");
    auto m = store.tilemap("assets/level.tmj");
    REQUIRE(m.has_value());
    const assets::TileMap& map = **m;
    REQUIRE(map.width == 20);
    REQUIRE(map.height == 10);
    REQUIRE(map.tile_width == 16);
    REQUIRE(map.layers.size() == 3);
    REQUIRE(map.layers[0].name == "ground");
    REQUIRE(map.layers[0].gids[8 * 20 + 3] == 1);
    REQUIRE(map.layers[0].gids[9 * 20 + 3] == 2);
    REQUIRE(map.layers[0].gids[0] == 0);
    REQUIRE(map.tilesets.size() == 1);
    REQUIRE(map.tilesets[0].image == "assets/tiles.png");   // resolved next to the map
    REQUIRE(map.tilesets[0].columns == 3);
    REQUIRE(map.tilesets[0].solid(0));
    REQUIRE_FALSE(map.tilesets[0].one_way(0));
    REQUIRE(map.tilesets[0].one_way(2));
    REQUIRE(map.tileset_for(2) == &map.tilesets[0]);
    // The deco layer flips its tiles horizontally and is solid as a whole.
    std::uint32_t deco = map.layers[1].gids[6 * 20 + 13];
    REQUIRE((deco & assets::TileMap::kFlipH) != 0);
    REQUIRE((deco & assets::TileMap::kIdMask) == 1);
    REQUIRE(map.layers[1].solid_layer());
    REQUIRE(map.solid_at(13, 6));
    REQUIRE(map.solid_at(3, 8));
    REQUIRE_FALSE(map.solid_at(3, 7));
    REQUIRE_FALSE(map.solid_at(-1, 8));
    // The plank on the platforms layer is one-way: solidity 2, not solid for a wall test.
    REQUIRE(map.layers[2].name == "platforms");
    REQUIRE(map.solidity_at(5, 6) == 2);
    REQUIRE_FALSE(map.solid_at(5, 6));
    REQUIRE(map.solidity_at(3, 8) == 1);
    REQUIRE(map.solidity_at(5, 5) == 0);
    REQUIRE(map.object_layers.size() == 1);
    REQUIRE(map.object_layers[0].objects.size() == 7);
    REQUIRE(map.object_layers[0].objects[0].name == "player");
    REQUIRE(map.object_layers[0].objects[1].properties["bob"] == 0.3);
    REQUIRE(map.properties["title"] == "coins");
    Json d = map.describe();
    REQUIRE(d["layers"][0]["tiles"] == 40);
    REQUIRE(store.describe("assets/level.tmj")["kind"] == "tilemap");
    // Bad maps say why.
    REQUIRE(assets::parse_tilemap("{\"orientation\":\"isometric\",\"width\":1,\"height\":1,\"tilewidth\":1,\"tileheight\":1}", "x.tmj").has_value() == false);
    REQUIRE(assets::parse_tilemap("{\"width\":2,\"height\":2,\"tilewidth\":16,\"tileheight\":16,\"tilesets\":[],\"layers\":[{\"type\":\"tilelayer\",\"name\":\"a\",\"width\":2,\"height\":2,\"data\":[0,0,0,5]}]}", "x.tmj").has_value() == false);
}

TEST_CASE("a tile map is edited in memory and written back as Tiled JSON", "[assets][tilemap]") {
    assets::AssetStore store(root() / "samples" / "sprites");
    auto m = store.tilemap_mut("assets/level.tmj");
    REQUIRE(m.has_value());
    assets::TileMap& map = **m;
    const assets::TileSet tileset = map.tilesets[0];
    REQUIRE(map.revision == 0);
    REQUIRE(map.layers[0].id == 1);
    REQUIRE(map.layers[2].id == 4);
    auto was = map.set("ground", 3, 7, 1);
    REQUIRE(was.has_value());
    REQUIRE(*was == 0);
    REQUIRE(map.revision == 1);
    REQUIRE(map.solid_at(3, 7));
    REQUIRE(*map.set("ground", 3, 7, 1) == 1);  // the same tile again changes nothing
    REQUIRE(map.revision == 1);
    REQUIRE(map.set("nope", 0, 0, 1).error().code == "unknown_layer");
    REQUIRE(map.set("ground", 20, 0, 1).error().code == "out_of_map");
    REQUIRE(map.set("ground", 0, 0, 7).error().code == "bad_gid");
    REQUIRE(*map.set("ground", 0, 0, 2u | assets::TileMap::kFlipH) == 0);
    // The document keeps everything it had; only the layer data follows the edits.
    Json doc = map.to_json();
    REQUIRE(doc["nextlayerid"] == 5);
    REQUIRE(doc["layers"][0]["data"][7 * 20 + 3] == 1);
    REQUIRE(doc["layers"][0]["data"][0] == (2u | assets::TileMap::kFlipH));
    auto again = assets::parse_tilemap(doc.dump(), "again.tmj");
    REQUIRE(again.has_value());
    REQUIRE(again->layers.size() == 3);
    REQUIRE(again->layers[0].gids[7 * 20 + 3] == 1);
    REQUIRE(again->object_layers[0].objects.size() == 7);
    REQUIRE(again->properties["title"] == "coins");
    REQUIRE(again->tilesets[0].one_way(2));
    // Readers get the same object, so the world sees the edit; reloading the file forgets it.
    REQUIRE((*store.tilemap("assets/level.tmj"))->solid_at(3, 7));
    store.invalidate("assets/level.tmj");
    REQUIRE_FALSE((*store.tilemap("assets/level.tmj"))->solid_at(3, 7));
    // A map with a group keeps its structure: tile layers are patched in document order.
    auto grouped = assets::parse_tilemap(R"({"width":2,"height":1,"tilewidth":16,"tileheight":16,"tilesets":[{"firstgid":1,"name":"t","image":"t.png","tilewidth":16,"tileheight":16,"columns":1,"tilecount":1}],"layers":[{"id":1,"type":"group","name":"g","layers":[{"id":2,"type":"tilelayer","name":"inner","width":2,"height":1,"data":[0,0]}]},{"id":3,"type":"tilelayer","name":"top","width":2,"height":1,"data":[1,0]}]})", "g.tmj");
    REQUIRE(grouped.has_value());
    REQUIRE(*grouped->set("inner", 1, 0, 1) == 0);
    Json gd = grouped->to_json();
    REQUIRE(gd["layers"][0]["layers"][0]["data"] == Json::array({0, 1}));
    REQUIRE(gd["layers"][1]["data"] == Json::array({1, 0}));
    // A map built in memory writes a minimal document that reads back.
    assets::TileMap fresh;
    fresh.width = 2;
    fresh.height = 1;
    fresh.tile_width = fresh.tile_height = 16;
    fresh.tilesets.push_back(tileset);
    assets::TileLayer l;
    l.name = "a";
    l.width = 2;
    l.height = 1;
    l.gids = {1, 0};
    fresh.layers.push_back(l);
    auto fd = assets::parse_tilemap(fresh.to_json().dump(), "fresh.tmj");
    REQUIRE(fd.has_value());
    REQUIRE(fd->layers[0].gids == std::vector<std::uint32_t>{1, 0});
    REQUIRE(fd->tilesets[0].one_way(2));
}

TEST_CASE("gltf with an embedded buffer and no normals gets flat normals", "[assets]") {
    assets::AssetStore store(project());
    auto m = store.mesh("assets/pyramid.gltf");
    REQUIRE(m.has_value());
    const assets::Mesh& mesh = **m;
    REQUIRE(mesh.vertices.size() == 5);
    REQUIRE(mesh.indices.size() == 18);
    REQUIRE(mesh.materials[0].base_color.y == Catch::Approx(0.8f));
    REQUIRE(mesh.materials[0].texture.empty());
    REQUIRE(mesh.aabb_max.y == Catch::Approx(1.0f));
    // The apex is shared by four faces; its normal is whichever face wrote last, but every
    // vertex normal is unit length.
    for (const auto& v : mesh.vertices) REQUIRE(length(v.normal) == Catch::Approx(1.0f).margin(1e-4));
}

TEST_CASE("images decode and bad files report why", "[assets]") {
    assets::AssetStore store(project());
    auto img = store.image("assets/checker.png");
    REQUIRE(img.has_value());
    REQUIRE((*img)->width == 64);
    REQUIRE((*img)->height == 64);
    // The origin cell is the red marker; the next cell along X is light.
    const auto& px = (*img)->rgba;
    REQUIRE(px[0] > 200); REQUIRE(px[1] < 80);
    std::size_t next = (2 * 64 + 12) * 4;
    REQUIRE(px[next] > 200); REQUIRE(px[next + 1] > 200);

    auto missing = store.mesh("assets/nope.glb");
    REQUIRE_FALSE(missing.has_value());
    REQUIRE(missing.error().code == "no_such_asset");
    auto outside = store.image("../../README.md");
    REQUIRE_FALSE(outside.has_value());
    REQUIRE(outside.error().code == "forbidden");
    auto not_gltf = store.mesh("assets/checker.png");
    REQUIRE_FALSE(not_gltf.has_value());
    REQUIRE(not_gltf.error().code == "bad_gltf");
    // Failures are remembered until invalidated.
    auto again = store.mesh("assets/checker.png");
    REQUIRE_FALSE(again.has_value());
    REQUIRE(again.error().code == "bad_asset");
    store.invalidate("assets/checker.png");
    REQUIRE_FALSE(store.mesh("assets/checker.png").has_value());

    Json list = store.list();
    REQUIRE(list.size() >= 3);
    bool saw_crate = false;
    for (auto& f : list) if (f["path"] == "assets/crate.glb") { saw_crate = true; REQUIRE(f["kind"] == "mesh"); }
    REQUIRE(saw_crate);
    Json d = store.describe("assets/crate.glb");
    REQUIRE(d["kind"] == "mesh");
    REQUIRE(d["nodes"] == 2);
    Json bad = store.describe("assets/nope.png");
    REQUIRE(bad.contains("error"));
}

TEST_CASE("parse_gltf rejects malformed input with a location", "[assets]") {
    auto r = assets::parse_gltf("{not json", ".", "x.gltf");
    REQUIRE_FALSE(r.has_value());
    REQUIRE(r.error().message.find("x.gltf") != std::string::npos);
    auto empty = assets::parse_gltf(R"({"asset":{"version":"2.0"}})", ".", "empty.gltf");
    REQUIRE_FALSE(empty.has_value());
    REQUIRE(empty.error().message.find("no triangle geometry") != std::string::npos);
    std::string glb = "glTF";
    std::uint32_t v = 1, len = 12;
    glb.append(reinterpret_cast<const char*>(&v), 4);
    glb.append(reinterpret_cast<const char*>(&len), 4);
    auto old = assets::parse_gltf(glb, ".", "old.glb");
    REQUIRE_FALSE(old.has_value());
    REQUIRE(old.error().message.find("version 1") != std::string::npos);
}

TEST_CASE("glTF material maps are read with their factors", "[assets][pbr]") {
    assets::AssetStore store(root() / "samples" / "assets");
    auto mesh = store.mesh("assets/plate.glb");
    REQUIRE(mesh.has_value());
    REQUIRE((*mesh)->materials.size() == 1);
    const assets::Material& m = (*mesh)->materials[0];
    REQUIRE(m.name == "plate");
    REQUIRE(m.texture.empty());
    REQUIRE(m.metallic_roughness_texture == "assets/plate_mr.png");
    REQUIRE(m.normal_texture == "assets/plate_normal.png");
    REQUIRE(m.normal_scale == Catch::Approx(1.0f));
    REQUIRE(m.emissive_texture == "assets/plate_glow.png");
    REQUIRE(m.emissive.x == Catch::Approx(1.0f));
    REQUIRE(m.metallic == Catch::Approx(1.0f));
    REQUIRE(m.roughness == Catch::Approx(1.0f));
    // The maps decode as images of the expected size.
    auto img = store.image("assets/plate_normal.png");
    REQUIRE(img.has_value());
    REQUIRE((*img)->width == 64);
    // A material without maps keeps empty paths.
    auto crate = store.mesh("assets/crate.glb");
    REQUIRE(crate.has_value());
    REQUIRE((*crate)->materials[0].normal_texture.empty());
    REQUIRE((*crate)->materials[0].texture == "assets/checker.png");
}
