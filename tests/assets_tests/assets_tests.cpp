#include <pocket/assets/assets.hpp>
#include <pocket/core/core.hpp>

#include <meshoptimizer.h>

#include <catch_amalgamated.hpp>

#include <algorithm>
#include <array>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <format>
#include <fstream>
#include <set>

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
    REQUIRE(mesh.animations.size() == 5);
    // Morph targets: two named targets with deltas parallel to the vertices, a weights track on
    // the pulse clip (two values per key) and the walk clip's root translation track.
    REQUIRE(mesh.morph_targets.size() == 2);
    REQUIRE(mesh.morph_targets[0].name == "bulge");
    REQUIRE(mesh.morph_targets[1].name == "lean");
    REQUIRE(mesh.morph_targets[0].positions.size() == mesh.vertices.size());
    REQUIRE(mesh.morph_targets[1].normals.size() == mesh.vertices.size());
    REQUIRE(mesh.morph_target("lean") == 1);
    REQUIRE(mesh.morph_target("1") == 1);
    REQUIRE(mesh.morph_target("smile") == -1);
    REQUIRE(mesh.default_weights == std::vector<float>({0.0f, 0.0f}));
    {
        float bulge_max = 0, lean_max = 0;
        for (const Vec3& d : mesh.morph_targets[0].positions) bulge_max = std::max(bulge_max, std::hypot(d.x, d.z));
        for (const Vec3& d : mesh.morph_targets[1].positions) lean_max = std::max(lean_max, d.x);
        REQUIRE(bulge_max == Catch::Approx(0.15f * std::sqrt(2.0f)).margin(1e-4));
        REQUIRE(lean_max == Catch::Approx(0.5f).margin(1e-4));
    }
    const assets::AnimationClip* pulse = mesh.clip("pulse");
    REQUIRE(pulse != nullptr);
    REQUIRE(pulse->channels.size() == 1);
    REQUIRE(pulse->channels[0].path == 3);
    REQUIRE(pulse->channels[0].width == 2);
    REQUIRE(pulse->channels[0].values == std::vector<float>({0, 0, 1, 0, 0, 0}));
    const assets::AnimationClip* walk = mesh.clip("walk");
    REQUIRE(walk != nullptr);
    REQUIRE(walk->channels.size() == 2);
    REQUIRE(walk->channels[0].path == 0);
    REQUIRE(walk->channels[0].node == 1);
    REQUIRE(walk->channels[0].values.back() == 1.0f);
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
    REQUIRE(d["animations"].size() == 5);
    REQUIRE(d["targets"] == Json::array({"bulge", "lean"}));
}

TEST_CASE("a Tiled map parses layers, tilesets, flips, properties and objects", "[assets][tilemap]") {
    assets::AssetStore store(root() / "samples" / "sprites");
    auto m = store.tilemap("assets/level.tmj");
    REQUIRE(m.has_value());
    const assets::TileMap& map = **m;
    REQUIRE(map.width == 20);
    REQUIRE(map.height == 10);
    REQUIRE(map.tile_width == 16);
    REQUIRE(map.layers.size() == 4);
    REQUIRE(map.layers[0].name == "ground");
    REQUIRE(map.layers[0].gids[8 * 20 + 3] == 1);
    REQUIRE(map.layers[0].gids[9 * 20 + 3] == 2);
    REQUIRE(map.layers[0].gids[0] == 0);
    REQUIRE(map.tilesets.size() == 1);
    REQUIRE(map.tilesets[0].image == "assets/tiles.png");   // resolved next to the map
    REQUIRE(map.tilesets[0].columns == 7);
    REQUIRE(map.tilesets[0].slope(3) == 1);
    REQUIRE(map.tilesets[0].slope(4) == -1);
    REQUIRE(map.tilesets[0].slope(0) == 0);
    REQUIRE(map.slope_at(16, 7) == 1);
    REQUIRE(map.slope_at(18, 7) == -1);
    REQUIRE(map.solidity_at(16, 7) == 3);
    REQUIRE_FALSE(map.solid_at(16, 7));
    REQUIRE(map.tilesets[0].solid(0));
    REQUIRE_FALSE(map.tilesets[0].one_way(0));
    REQUIRE(map.tilesets[0].one_way(2));
    // The water tile animates between two frames of 400 ms; other tiles show themselves.
    REQUIRE(map.tilesets[0].animations.size() == 1);
    REQUIRE(map.tilesets[0].frame_at(5, 0) == 5);
    REQUIRE(map.tilesets[0].frame_at(5, 399) == 5);
    REQUIRE(map.tilesets[0].frame_at(5, 400) == 6);
    REQUIRE(map.tilesets[0].frame_at(5, 800) == 5);
    REQUIRE(map.tilesets[0].frame_at(0, 1234) == 0);
    REQUIRE(map.layer("water")->gids[7 * 20] == 6);
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
    REQUIRE(map.object_layers.size() == 2);
    REQUIRE(map.object_layers[0].objects.size() == 7);
    // The routes layer's patrol is a polyline: its points from the object's place, open.
    REQUIRE(map.object_layers[1].objects[0].points.size() == 3);
    REQUIRE(map.object_layers[1].objects[0].points[2].y == -32);
    REQUIRE_FALSE(map.object_layers[1].objects[0].closed);
    REQUIRE(map.object_layers[0].objects[0].name == "player");
    REQUIRE(map.object_layers[0].objects[1].properties["bob"] == 0.3);
    REQUIRE(map.properties["title"] == "coins");
    Json d = map.describe();
    REQUIRE(d["layers"][0]["tiles"] == 43);  // two ground rows and the three hill cells
    REQUIRE(store.describe("assets/level.tmj")["kind"] == "tilemap");
    // Bad maps say why.
    REQUIRE(assets::parse_tilemap("{\"orientation\":\"spherical\",\"width\":1,\"height\":1,\"tilewidth\":1,\"tileheight\":1}", "x.tmj").has_value() == false);
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
    REQUIRE(map.set("ground", 0, 0, 8).error().code == "bad_gid");
    REQUIRE(*map.set("ground", 0, 0, 2u | assets::TileMap::kFlipH) == 0);
    // The document keeps everything it had; only the layer data follows the edits.
    Json doc = map.to_json();
    REQUIRE(doc["nextlayerid"] == 7);
    REQUIRE(doc["layers"][1]["data"][7 * 20 + 3] == 1);   // the sky picture comes first in the file
    REQUIRE(doc["layers"][1]["data"][0] == (2u | assets::TileMap::kFlipH));
    auto again = assets::parse_tilemap(doc.dump(), "again.tmj");
    REQUIRE(again.has_value());
    REQUIRE(again->layers.size() == 4);
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
    // Reordering counts layers with the groups flattened: the top layer moved under the inner one
    // joins the group before it, and the inner one moved last follows it there.
    REQUIRE(grouped->move_layer("top", 0).has_value());
    REQUIRE(grouped->layers[0].name == "top");
    gd = grouped->to_json();
    REQUIRE(gd["layers"].size() == 1);
    REQUIRE(gd["layers"][0]["layers"][0]["name"] == "top");
    REQUIRE(gd["layers"][0]["layers"][1]["name"] == "inner");
    REQUIRE(grouped->move_layer("inner", 1).has_value());
    REQUIRE(grouped->layers[1].name == "inner");
    REQUIRE(grouped->to_json()["layers"][0]["layers"][1]["name"] == "inner");
    auto reread = assets::parse_tilemap(grouped->to_json().dump(), "g.tmj");
    REQUIRE(reread.has_value());
    REQUIRE(reread->layers[0].name == "top");
    REQUIRE(reread->layers[0].gids == std::vector<std::uint32_t>{1, 0});
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
    REQUIRE(fd->tilesets[0].animations.size() == 1);   // the animation written with the tiles
    REQUIRE(fd->tilesets[0].frame_at(5, 500) == 6);
}

TEST_CASE("isometric, staggered and hexagonal maps place their cells the way Tiled draws them", "[assets][tilemap][orientation]") {
    auto near = [](Vec2 a, float x, float y) { return std::fabs(a.x - x) < 1e-4f && std::fabs(a.y - y) < 1e-4f; };
    // Isometric: 2 by 2, diamonds 32 wide and 16 tall; the map is 64 by 32 pixels with tile (0, 0)
    // at the top and (0, 1) at the left.
    auto iso = assets::parse_tilemap(R"({"width":2,"height":2,"tilewidth":32,"tileheight":16,"orientation":"isometric","tilesets":[{"firstgid":1,"name":"t","image":"t.png","tilewidth":32,"tileheight":16,"columns":1,"tilecount":1}],"layers":[{"id":1,"type":"tilelayer","name":"a","width":2,"height":2,"data":[1,1,1,1]}]})", "iso.tmj");
    REQUIRE(iso.has_value());
    REQUIRE(iso->orientation == "isometric");
    REQUIRE(near(iso->tile_pixel(0, 0), 16, 0));
    REQUIRE(near(iso->tile_pixel(1, 0), 32, 8));
    REQUIRE(near(iso->tile_pixel(0, 1), 0, 8));
    REQUIRE(near(iso->tile_pixel(1, 1), 16, 16));
    REQUIRE(near(iso->pixel_size(), 64, 32));
    int x = -1, y = -1;
    REQUIRE(iso->cell_at_pixel(32, 8, x, y));   // the center of (0, 0)
    REQUIRE((x == 0 && y == 0));
    REQUIRE(iso->cell_at_pixel(32, 24, x, y));  // the center of (1, 1)
    REQUIRE((x == 1 && y == 1));
    REQUIRE(iso->cell_at_pixel(10, 16, x, y));  // left, in (0, 1)
    REQUIRE((x == 0 && y == 1));
    REQUIRE_FALSE(iso->cell_at_pixel(2, 2, x, y));   // the corner outside the diamonds
    REQUIRE(near(iso->object_pixel(16, 16), 32, 16));   // unprojected tile (1, 1): the top corner of its diamond
    REQUIRE(iso->describe()["orientation"] == "isometric");
    REQUIRE(iso->to_json()["orientation"] == "isometric");
    // Hexagonal, rows staggered (the odd ones shifted right by half a tile), a flat side of 8 in
    // tiles 32 wide and 32 tall: rows are 20 pixels apart.
    auto hex = assets::parse_tilemap(R"({"width":3,"height":3,"tilewidth":32,"tileheight":32,"orientation":"hexagonal","hexsidelength":8,"staggeraxis":"y","staggerindex":"odd","tilesets":[{"firstgid":1,"name":"t","image":"t.png","tilewidth":32,"tileheight":32,"columns":1,"tilecount":1}],"layers":[{"id":1,"type":"tilelayer","name":"a","width":3,"height":3,"data":[1,1,1,1,1,1,1,1,1]}]})", "hex.tmj");
    REQUIRE(hex.has_value());
    REQUIRE(near(hex->tile_pixel(0, 1), 16, 20));
    REQUIRE(near(hex->tile_pixel(2, 2), 64, 40));
    REQUIRE(near(hex->pixel_size(), 112, 72));
    REQUIRE(hex->cell_at_pixel(64, 36, x, y));   // the center of (1, 1)
    REQUIRE((x == 1 && y == 1));
    REQUIRE(hex->cell_at_pixel(80, 16, x, y));   // the center of (2, 0)
    REQUIRE((x == 2 && y == 0));
    REQUIRE(hex->cell_at_pixel(48, 20, x, y));   // between (1, 0) below-left and (1, 1): nearer (1, 0)'s center (48, 16)
    REQUIRE((x == 1 && y == 0));
    REQUIRE(hex->describe()["hex_side"] == 8);
    REQUIRE(hex->describe()["stagger_axis"] == "y");
    REQUIRE(hex->to_json()["hexsidelength"] == 8);
    // Staggered isometric in columns, the even ones shifted down: column 0 starts half a tile down.
    auto stag = assets::parse_tilemap(R"({"width":3,"height":2,"tilewidth":32,"tileheight":16,"orientation":"staggered","staggeraxis":"x","staggerindex":"even","tilesets":[{"firstgid":1,"name":"t","image":"t.png","tilewidth":32,"tileheight":16,"columns":1,"tilecount":1}],"layers":[{"id":1,"type":"tilelayer","name":"a","width":3,"height":2,"data":[1,1,1,1,1,1]}]})", "stag.tmj");
    REQUIRE(stag.has_value());
    REQUIRE(near(stag->tile_pixel(0, 0), 0, 8));
    REQUIRE(near(stag->tile_pixel(1, 0), 16, 0));
    REQUIRE(near(stag->tile_pixel(2, 1), 32, 24));
    REQUIRE(near(stag->pixel_size(), 64, 40));
    REQUIRE(stag->cell_at_pixel(32, 8, x, y));   // the center of (1, 0)
    REQUIRE((x == 1 && y == 0));
    REQUIRE(stag->cell_at_pixel(16, 32, x, y));  // the center of (0, 1)
    REQUIRE((x == 0 && y == 1));
    REQUIRE(stag->to_json()["staggerindex"] == "even");
    // An orientation Tiled does not have, and a hexagon's side longer than the tile, are refused.
    REQUIRE_FALSE(assets::parse_tilemap(R"({"width":1,"height":1,"tilewidth":16,"tileheight":16,"orientation":"round","tilesets":[],"layers":[]})", "bad.tmj").has_value());
    REQUIRE_FALSE(assets::parse_tilemap(R"({"width":1,"height":1,"tilewidth":16,"tileheight":16,"orientation":"hexagonal","hexsidelength":40,"tilesets":[],"layers":[]})", "bad.tmj").has_value());
}

TEST_CASE("geometry under an animated node is a moving part kept in its node's space", "[assets][gltf][parts]") {
    assets::AssetStore store(project());
    auto m = store.mesh("assets/fan.glb");
    REQUIRE(m.has_value());
    const assets::Mesh& fan = **m;
    REQUIRE(fan.submeshes.size() == 2);
    REQUIRE(fan.moving_parts() == 1);
    REQUIRE(fan.nodes.size() == 2);
    // The hub is baked; the blade (node 1, half a unit up, turned by the spin clip) is a part whose
    // vertices stay around its own origin, so the file's bounds place it by its rest.
    REQUIRE(fan.submeshes[0].node == -1);
    REQUIRE(fan.submeshes[1].node == 1);
    float top = -1e9f;
    for (std::uint32_t i = fan.submeshes[1].first_index; i < fan.submeshes[1].first_index + fan.submeshes[1].index_count; ++i) top = std::max(top, fan.vertices[fan.indices[i]].position.y);
    REQUIRE(top == Catch::Approx(0.15f));
    REQUIRE(fan.aabb_max.y == Catch::Approx(0.65f));
    REQUIRE(fan.aabb_max.x == Catch::Approx(0.8f));
    REQUIRE(fan.rest_global(1).transform_point({0, 0, 0}).y == Catch::Approx(0.5f));
    REQUIRE(store.describe("assets/fan.glb")["moving_parts"] == 1);
    // The arm is skinned: no parts, the skin does the moving.
    REQUIRE((*store.mesh("assets/arm.glb"))->moving_parts() == 0);
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
    // Its vertices get tangents from the uvs (the file has none): unit length, across the normal.
    for (const assets::MeshVertex& v : (*mesh)->vertices) {
        const Vec3 t{v.tangent.x, v.tangent.y, v.tangent.z};
        REQUIRE(std::abs(v.tangent.w) == Catch::Approx(1.0f));
        REQUIRE(length(t) == Catch::Approx(1.0f).margin(1e-4));
        REQUIRE(std::abs(dot(t, v.normal)) < 1e-3);
    }
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

TEST_CASE("image layers are read with their place among the tile layers and follow layer edits", "[tilemap][imagelayer]") {
    const char* text = R"({"type":"map","version":"1.10","orientation":"orthogonal","renderorder":"right-down","width":2,"height":2,"tilewidth":16,"tileheight":16,"infinite":false,"nextlayerid":4,"nextobjectid":1,
        "tilesets":[{"firstgid":1,"name":"t","image":"t.png","imagewidth":16,"imageheight":16,"tilewidth":16,"tileheight":16,"columns":1,"tilecount":1}],
        "layers":[
            {"id":1,"type":"tilelayer","name":"a","width":2,"height":2,"x":0,"y":0,"opacity":1,"visible":true,"data":[0,0,0,0]},
            {"id":2,"type":"imagelayer","name":"bg","image":"art/bg.png","x":0,"y":0,"offsetx":8,"offsety":4,"opacity":0.5,"visible":true,"repeatx":true,"repeaty":false,"parallaxx":0.5,"parallaxy":0.25,"tintcolor":"#80ff0000"},
            {"id":3,"type":"tilelayer","name":"b","width":2,"height":2,"x":0,"y":0,"opacity":1,"visible":true,"data":[0,0,0,0]}
        ]})";
    auto parsed = assets::parse_tilemap(text, "maps/x.tmj");
    REQUIRE(parsed.has_value());
    assets::TileMap& map = *parsed;
    REQUIRE(map.layers.size() == 2);
    REQUIRE(map.image_layers.size() == 1);
    const assets::ImageLayer& bg = map.image_layers[0];
    REQUIRE(bg.name == "bg");
    REQUIRE(bg.image == "maps/art/bg.png");   // next to the map
    REQUIRE(bg.offset_x == 8);
    REQUIRE(bg.offset_y == 4);
    REQUIRE(bg.opacity == Catch::Approx(0.5));
    REQUIRE(bg.repeat_x);
    REQUIRE_FALSE(bg.repeat_y);
    REQUIRE(bg.tint.x == Catch::Approx(1.0));
    REQUIRE(bg.tint.y == Catch::Approx(0.0));
    REQUIRE(bg.tint.w == Catch::Approx(128.0 / 255.0));
    REQUIRE(bg.parallax_x == Catch::Approx(0.5));
    REQUIRE(bg.parallax_y == Catch::Approx(0.25));
    REQUIRE(bg.before == 1);                  // after "a", under "b"
    Json d = map.describe();
    REQUIRE(d["image_layers"].size() == 1);
    REQUIRE(d["image_layers"][0]["before"] == 1);
    // Tile layers moved around it: "b" brought to the front puts both before the picture.
    REQUIRE(map.move_layer("b", 0).has_value());
    REQUIRE(map.image_layers[0].before == 2);
    REQUIRE(map.move_layer("b", 1).has_value());   // a, b: still both before it
    REQUIRE(map.image_layers[0].before == 2);
    REQUIRE(map.remove_layer("a").has_value());
    REQUIRE(map.image_layers[0].before == 1);
    REQUIRE(map.add_layer("c", true, 1.0f, Json::object()).has_value());   // appended after everything
    REQUIRE(map.image_layers[0].before == 1);
    // The picture stays in the document, so a save keeps it.
    Json doc = map.to_json();
    int pictures = 0;
    for (const Json& l : doc["layers"]) pictures += l.value("type", "") == "imagelayer";
    REQUIRE(pictures == 1);
    auto again = assets::parse_tilemap(doc.dump(), "maps/again.tmj");
    REQUIRE(again.has_value());
    REQUIRE(again->image_layers.size() == 1);
    REQUIRE(again->image_layers[0].before == 1);
    // The sample's sky: first in the file, so it draws under every tile layer, repeated across the level.
    assets::AssetStore store(root() / "samples" / "sprites");
    auto level = store.tilemap("assets/level.tmj");
    REQUIRE(level.has_value());
    REQUIRE((*level)->image_layers.size() == 1);
    REQUIRE((*level)->image_layers[0].name == "sky");
    REQUIRE((*level)->image_layers[0].image == "assets/sky.png");
    REQUIRE((*level)->image_layers[0].before == 0);
    REQUIRE((*level)->image_layers[0].repeat_x);
    REQUIRE((*level)->image_layers[0].parallax_x == Catch::Approx(1.0));   // none asked: it moves with the map
    REQUIRE(store.describe("assets/level.tmj")["image_layers"][0]["name"] == "sky");
}

TEST_CASE("an infinite map is read as the box around its chunks, cells and objects shifted to it", "[tilemap][infinite]") {
    // Two 4 by 4 chunks, one at (-4, -4) and one at (0, 0): an 8 by 8 box starting at (-4, -4).
    const char* text = R"({"type":"map","version":"1.10","orientation":"orthogonal","renderorder":"right-down","width":4,"height":4,"tilewidth":16,"tileheight":16,"infinite":true,"nextlayerid":4,"nextobjectid":2,
        "tilesets":[{"firstgid":1,"name":"t","image":"t.png","imagewidth":16,"imageheight":16,"tilewidth":16,"tileheight":16,"columns":1,"tilecount":1}],
        "layers":[
            {"id":1,"type":"tilelayer","name":"ground","startx":-4,"starty":-4,"width":8,"height":8,"x":0,"y":0,"opacity":1,"visible":true,"chunks":[
                {"x":-4,"y":-4,"width":4,"height":4,"data":[1,0,0,0, 0,0,0,0, 0,0,0,0, 0,0,0,1]},
                {"x":0,"y":0,"width":4,"height":4,"data":[0,0,0,0, 0,1,0,0, 0,0,0,0, 0,0,0,0]}
            ]},
            {"id":2,"type":"objectgroup","name":"spawns","objects":[{"id":1,"name":"player","type":"spawn","x":8,"y":8,"width":0,"height":0,"point":true}]},
            {"id":3,"type":"imagelayer","name":"bg","image":"bg.png","x":0,"y":0,"offsetx":0,"offsety":0,"opacity":1,"visible":true}
        ]})";
    auto parsed = assets::parse_tilemap(text, "maps/big.tmj");
    if (!parsed) INFO(parsed.error().to_string());
    REQUIRE(parsed.has_value());
    assets::TileMap& map = *parsed;
    REQUIRE(map.infinite);
    REQUIRE(map.width == 8);
    REQUIRE(map.height == 8);
    REQUIRE(map.chunk_x == -4);
    REQUIRE(map.chunk_y == -4);
    REQUIRE(map.layers.size() == 1);
    const assets::TileLayer& ground = map.layers[0];
    REQUIRE(ground.width == 8);
    REQUIRE(ground.gids.size() == 64);
    REQUIRE(ground.gids[0] == 1);                 // chunk (-4,-4) local (0,0)
    REQUIRE(ground.gids[3 * 8 + 3] == 1);         // chunk (-4,-4) local (3,3)
    REQUIRE(ground.gids[5 * 8 + 5] == 1);         // chunk (0,0) local (1,1)
    int filled = 0;
    for (std::uint32_t g : ground.gids) filled += g != 0;
    REQUIRE(filled == 3);
    // Tiled's (8, 8) pixels is the box's (72, 72): four tiles further in each direction.
    REQUIRE(map.object_layers.size() == 1);
    REQUIRE(map.object_layers[0].objects[0].x == Catch::Approx(72.0));
    REQUIRE(map.object_layers[0].objects[0].y == Catch::Approx(72.0));
    REQUIRE(map.image_layers[0].offset_x == Catch::Approx(64.0));
    Json d = map.describe();
    REQUIRE(d["infinite"] == true);
    REQUIRE(d["origin"]["x"] == -4);
    REQUIRE(d["width"] == 8);
    // A finite map reports neither.
    REQUIRE_FALSE(assets::parse_tilemap(R"({"orientation":"isometric","width":2,"height":2,"tilewidth":32,"tileheight":16,"infinite":true,"tilesets":[],"layers":[]})", "maps/iso.tmj").has_value());
}

TEST_CASE("a tile's collision rectangles make it solid inside them, flipped with the tile", "[tilemap][shapes]") {
    // Tile 0: the bottom half (a low block); its polygon is skipped. Tile 1: solid by property.
    const char* text = R"({"type":"map","orientation":"orthogonal","width":3,"height":2,"tilewidth":16,"tileheight":16,"infinite":false,
        "tilesets":[{"firstgid":1,"name":"t","image":"t.png","imagewidth":32,"imageheight":16,"tilewidth":16,"tileheight":16,"columns":2,"tilecount":2,
            "tiles":[{"id":0,"objectgroup":{"type":"objectgroup","objects":[{"id":1,"x":0,"y":8,"width":16,"height":8},{"id":2,"x":0,"y":0,"polygon":[{"x":0,"y":0},{"x":16,"y":16},{"x":0,"y":16}]}]}},
                     {"id":1,"properties":[{"name":"solid","type":"bool","value":true}]}]}],
        "layers":[{"id":1,"type":"tilelayer","name":"ground","width":3,"height":2,"x":0,"y":0,"opacity":1,"visible":true,"data":[1,1073741825,0,2,2,2]}]})";
    auto parsed = assets::parse_tilemap(text, "maps/shapes.tmj");
    if (!parsed) INFO(parsed.error().to_string());
    REQUIRE(parsed.has_value());
    assets::TileMap& map = *parsed;
    const assets::TileSet& ts = map.tilesets[0];
    const auto* sh = ts.shapes_of(0);
    REQUIRE(sh != nullptr);
    REQUIRE(sh->size() == 1);
    REQUIRE((*sh)[0].y0 == Catch::Approx(0.5));
    REQUIRE((*sh)[0].y1 == Catch::Approx(1.0));
    REQUIRE((*sh)[0].x1 == Catch::Approx(1.0));
    REQUIRE(ts.solid(0));
    REQUIRE(ts.shapes_of(1) == nullptr);
    REQUIRE(map.solidity_at(0, 0) == 1);
    std::vector<assets::TileSet::Shape> boxes;
    map.solid_boxes(0, 0, boxes);
    REQUIRE(boxes.size() == 1);
    REQUIRE(boxes[0].y0 == Catch::Approx(0.5));
    // Cell (1, 0) holds the same tile flipped vertically: the block is its top half.
    map.solid_boxes(1, 0, boxes);
    REQUIRE(boxes.size() == 1);
    REQUIRE(boxes[0].y0 == Catch::Approx(0.0));
    REQUIRE(boxes[0].y1 == Catch::Approx(0.5));
    // A whole solid tile is one box over the cell; an empty cell has none.
    map.solid_boxes(0, 1, boxes);
    REQUIRE(boxes.size() == 1);
    REQUIRE(boxes[0].y0 == Catch::Approx(0.0));
    REQUIRE(boxes[0].y1 == Catch::Approx(1.0));
    map.solid_boxes(2, 0, boxes);
    REQUIRE(boxes.empty());
    // Points: the top half of cell (0, 0) is air, the bottom half solid.
    REQUIRE_FALSE(map.solid_at_point(0, 0, 0.5f, 0.25f));
    REQUIRE(map.solid_at_point(0, 0, 0.5f, 0.75f));
    REQUIRE(map.solid_at_point(0, 1, 0.5f, 0.25f));
}

TEST_CASE("a material's KHR_texture_transform offset and scale are read", "[assets][gltf][transform]") {
    // A textured quad beside the sample's assets, removed after: its checker tiles four times, shifted.
    const std::filesystem::path file = project() / "assets" / "transform-test.gltf";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    {
        std::ofstream out(file, std::ios::binary);
        out << R"({"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"name":"quad","mesh":0}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0}]}],
            "materials":[{"name":"tiled","pbrMetallicRoughness":{"baseColorTexture":{"index":0,"extensions":{"KHR_texture_transform":{"offset":[0.25,0.5],"scale":[4,4],"rotation":0}}}}}],
            "textures":[{"source":0}],"images":[{"uri":"checker.png"}],
            "buffers":[{"byteLength":140,"uri":"data:application/octet-stream;base64,)" << "AAAAvwAAAL8AAAAAAAAAPwAAAL8AAAAAAAAAPwAAAD8AAAAAAAAAvwAAAD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAgD8AAIA/AACAPwAAgD8AAAAAAAAAAAAAAAAAAAEAAgAAAAIAAwA=" << R"("}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48},{"buffer":0,"byteOffset":48,"byteLength":48},{"buffer":0,"byteOffset":96,"byteLength":32},{"buffer":0,"byteOffset":128,"byteLength":12}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-0.5,-0.5,0],"max":[0.5,0.5,0]},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}]})";
    }
    assets::AssetStore store(project());
    auto m = store.mesh("assets/transform-test.gltf");
    if (!m) INFO(m.error().to_string());
    REQUIRE(m.has_value());
    const assets::Mesh& mesh = **m;
    REQUIRE(mesh.materials.size() == 1);
    const assets::Material& mat = mesh.materials[0];
    REQUIRE(mat.texture == "assets/checker.png");
    REQUIRE(mat.uv_transformed);
    REQUIRE(mat.uv_offset.x == Catch::Approx(0.25));
    REQUIRE(mat.uv_offset.y == Catch::Approx(0.5));
    REQUIRE(mat.uv_scale.x == Catch::Approx(4.0));
    REQUIRE(mat.uv_scale.y == Catch::Approx(4.0));
    Json d = mesh.describe();
    REQUIRE(d["materials"][0]["uv_transform"]["scale"][0] == 4.0);
    // The crate's material has none.
    auto crate = store.mesh("assets/crate.glb");
    REQUIRE(crate.has_value());
    REQUIRE_FALSE((*crate)->materials[0].uv_transformed);
}

TEST_CASE("a double-sided material gets a back: its triangles again, wound the other way, normals turned over", "[assets][gltf][doublesided]") {
    // The same quad twice: single-sided, then double-sided.
    auto quad = [&](const char* name, bool two) {
        const std::filesystem::path file = project() / "assets" / name;
        {
            std::ofstream out(file, std::ios::binary);
            out << R"({"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"name":"quad","mesh":0}],
                "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0}]}],
                "materials":[{"name":"leaf","doubleSided":)" << (two ? "true" : "false") << R"(}],
                "buffers":[{"byteLength":140,"uri":"data:application/octet-stream;base64,)" << "AAAAvwAAAL8AAAAAAAAAPwAAAL8AAAAAAAAAPwAAAD8AAAAAAAAAvwAAAD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAgD8AAIA/AACAPwAAgD8AAAAAAAAAAAAAAAAAAAEAAgAAAAIAAwA=" << R"("}],
                "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48},{"buffer":0,"byteOffset":48,"byteLength":48},{"buffer":0,"byteOffset":96,"byteLength":32},{"buffer":0,"byteOffset":128,"byteLength":12}],
                "accessors":[{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-0.5,-0.5,0],"max":[0.5,0.5,0]},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}]})";
        }
        assets::AssetStore store(project());
        auto m = store.mesh(std::string("assets/") + name);
        std::filesystem::remove(file);
        REQUIRE(m.has_value());
        return **m;
    };
    const assets::Mesh one = quad("one-sided-test.gltf", false);
    const assets::Mesh two = quad("two-sided-test.gltf", true);
    REQUIRE(one.vertices.size() == 4);
    REQUIRE(one.indices.size() == 6);
    REQUIRE(one.submeshes.size() == 1);
    REQUIRE(two.vertices.size() == 8);
    REQUIRE(two.indices.size() == 12);
    REQUIRE(two.submeshes.size() == 2);
    REQUIRE(two.submeshes[1].first_index == 6);
    REQUIRE(two.submeshes[1].material == 0);
    for (std::size_t v = 0; v < 4; ++v) {
        REQUIRE(two.vertices[4 + v].position == two.vertices[v].position);
        REQUIRE(two.vertices[4 + v].normal.z == Catch::Approx(-two.vertices[v].normal.z));
    }
    // Each back triangle faces the other way: its winding's normal is the turned-over one.
    for (std::size_t t = 6; t < 12; t += 3) {
        const Vec3 a = two.vertices[two.indices[t]].position, b = two.vertices[two.indices[t + 1]].position, c = two.vertices[two.indices[t + 2]].position;
        REQUIRE(cross(b - a, c - a).z < 0);
    }
}

TEST_CASE("OBJ files with MTL materials: objects as nodes, materials, uvs flipped, missing normals smoothed", "[assets][obj]") {
    const std::filesystem::path dir = project() / "assets" / "obj-test";
    std::filesystem::create_directories(dir);
    std::ofstream(dir / "things.mtl", std::ios::binary) << "# two materials\nnewmtl Red\nKd 0.8 0.1 0.1\nNs 250\nmap_Kd ../checker.png\nmap_Bump -bm 1.0 ../normal_up.png\n\nnewmtl Glass\nKd 0.2 0.4 0.9\nd 0.5\nPm 0.0\nPr 0.1\nKe 0.5 0.5 0\n";
    std::ofstream(dir / "things.obj", std::ios::binary) <<
        "mtllib things.mtl\n"
        "o Floor\nv -1 0 -1\nv 1 0 -1\nv 1 0 1\nv -1 0 1\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n"
        "usemtl Red\nf 1/1 4/4 3/3 2/2\n"
        "o Pane\nv 0 1 0\nv 1 1 0\nv 0 2 0\nvn 0 0 1\n"
        "usemtl Glass\nf -3//1 -2//1 -1//1\n";
    assets::AssetStore store(project());
    auto m = store.mesh("assets/obj-test/things.obj");
    INFO((m ? std::string() : m.error().to_string()));
    REQUIRE(m.has_value());
    const assets::Mesh& mesh = **m;
    REQUIRE(mesh.importer == "obj");
    REQUIRE(mesh.nodes.size() == 2);
    REQUIRE(mesh.nodes[0].name == "Floor");
    REQUIRE(mesh.nodes[1].name == "Pane");
    REQUIRE(mesh.submeshes.size() == 2);
    REQUIRE(mesh.submeshes[0].origin == 0);
    REQUIRE(mesh.submeshes[1].origin == 1);
    REQUIRE(mesh.submeshes[0].index_count == 6);   // the quad as two triangles
    REQUIRE(mesh.submeshes[1].index_count == 3);
    REQUIRE(mesh.vertices.size() == 7);
    // The floor had no normals: smoothed from its faces, straight up.
    REQUIRE(mesh.vertices[0].normal.y == Catch::Approx(1.0f));
    // v runs up in OBJ and down in images.
    REQUIRE(mesh.vertices[0].uv.y == Catch::Approx(1.0f));
    REQUIRE(mesh.materials.size() == 2);
    const assets::Material& red = mesh.materials[0];
    REQUIRE(red.name == "Red");
    REQUIRE(red.base_color.x == Catch::Approx(0.8f));
    REQUIRE(red.texture == "assets/checker.png");
    REQUIRE(red.normal_texture == "assets/normal_up.png");
    REQUIRE(red.roughness == Catch::Approx(0.5f));   // Ns 250: half the way
    REQUIRE_FALSE(red.blend);
    const assets::Material& glass = mesh.materials[1];
    REQUIRE(glass.base_color.w == Catch::Approx(0.5f));
    REQUIRE(glass.blend);
    REQUIRE(glass.roughness == Catch::Approx(0.1f));
    REQUIRE(glass.emissive.x == Catch::Approx(0.5f));
    REQUIRE(mesh.aabb_max.y == Catch::Approx(2.0f));
    REQUIRE(mesh.describe()["importer"] == "obj");
    // The store lists it as a mesh read by the OBJ reader, and its library as a material file.
    bool listed = false;
    for (const Json& f : store.list()) if (f["path"] == "assets/obj-test/things.obj") listed = f["kind"] == "mesh" && f["importer"] == "obj";
    REQUIRE(listed);
    // A face pointing past the vertices is refused with the line.
    std::ofstream(dir / "bad.obj", std::ios::binary) << "v 0 0 0\nv 1 0 0\nf 1 2 9\n";
    auto bad = store.mesh("assets/obj-test/bad.obj");
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().message.find(":3:") != std::string::npos);
    std::filesystem::remove_all(dir);
    std::filesystem::remove_all(project() / ".imported" / "assets" / "obj-test");
}

namespace {

// An OBJ read straight from text, with the settings given (no store, no cache).
assets::Mesh obj_mesh(const std::string& text, const assets::ImportSettings& settings = {}) {
    auto m = assets::parse_obj(text, "assets/test.obj", [](const std::string& p) -> Result<std::string> { return fail("no_such_asset", "{}", p); }, settings);
    INFO((m ? std::string() : m.error().to_string()));
    REQUIRE(m.has_value());
    return std::move(*m);
}

// A capped cylinder of `n` sides, radius 1, from y 0 to 1, welded (each position written once),
// without normals; `extra` goes before its faces (an `s` line).
std::string obj_cylinder(int n, const std::string& extra = "") {
    std::string s;
    for (int ring = 0; ring < 2; ++ring) {
        for (int i = 0; i < n; ++i) {
            const double a = 2 * 3.141592653589793 * i / n;
            s += std::format("v {:.9f} {} {:.9f}\n", std::cos(a), ring, std::sin(a));
        }
    }
    s += extra;
    for (int i = 0; i < n; ++i) {
        const int j = (i + 1) % n;
        s += std::format("f {} {} {} {}\n", i + 1, n + i + 1, n + j + 1, j + 1);   // outward
    }
    s += "f";
    for (int i = n - 1; i >= 0; --i) s += std::format(" {}", n + i + 1);   // the top, counter-clockwise seen from above
    s += "\nf";
    for (int i = 0; i < n; ++i) s += std::format(" {}", i + 1);
    s += "\n";
    return s;
}

// Every triangle's area and whether it faces along `up` (its winding against the vertex normals).
struct TriangleCheck {
    double area = 0;
    bool facing = true;
};
TriangleCheck triangles_of(const assets::Mesh& m, Vec3 up) {
    TriangleCheck c;
    for (std::size_t k = 0; k + 2 < m.indices.size(); k += 3) {
        const Vec3 a = m.vertices[m.indices[k]].position, b = m.vertices[m.indices[k + 1]].position, d = m.vertices[m.indices[k + 2]].position;
        const Vec3 n = cross(b - a, d - a);
        c.area += 0.5 * length(n);
        if (dot(n, up) <= 0) c.facing = false;
    }
    return c;
}

}  // namespace

TEST_CASE("OBJ without normals: faces past the crease angle shaded apart, s off flat, s N smoothing groups", "[assets][obj][crease]") {
    // A welded cube (CAD and STL converters write every position once): its edges are 90 degrees,
    // so each corner is three vertices with the three faces' normals, not one rounded normal.
    const std::string cube = "v -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1\nv -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\n";
    const std::string faces = "f 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n";
    auto axis_aligned = [](const assets::Mesh& m) {
        for (const auto& v : m.vertices) {
            const float big = std::max({std::fabs(v.normal.x), std::fabs(v.normal.y), std::fabs(v.normal.z)});
            if (big < 0.9999f) return false;
            // And it points out of the face it belongs to: along the position's own coordinate.
            const float along = std::fabs(v.normal.x) > 0.5f ? v.normal.x * v.position.x : std::fabs(v.normal.y) > 0.5f ? v.normal.y * v.position.y : v.normal.z * v.position.z;
            if (along <= 0) return false;
        }
        return true;
    };
    const assets::Mesh sharp = obj_mesh(cube + faces);
    REQUIRE(sharp.vertices.size() == 24);
    REQUIRE(sharp.indices.size() == 36);
    REQUIRE(axis_aligned(sharp));
    // s off: flat whatever the angle (the stress kit's cube_soff).
    const assets::Mesh flat = obj_mesh(cube + "s off\n" + faces);
    REQUIRE(flat.vertices.size() == 24);
    REQUIRE(axis_aligned(flat));
    // A crease of 180 smooths every shared corner, as the reader did before: eight rounded corners.
    assets::ImportSettings round;
    round.crease = 180;
    const assets::Mesh smooth = obj_mesh(cube + faces, round);
    REQUIRE(smooth.vertices.size() == 8);
    for (const auto& v : smooth.vertices) REQUIRE(std::fabs(v.normal.x) == Catch::Approx(1 / std::sqrt(3.0f)).margin(1e-5));
    // A smoothing group is smooth across any angle, as 3ds Max and Maya mean it.
    REQUIRE(obj_mesh(cube + "s 1\n" + faces).vertices.size() == 8);

    // Two quads folded at a right angle along x = 0, y = 0: a floor facing up and a wall facing +x.
    const std::string fold = "v 0 0 0\nv 0 0 1\nv 1 0 1\nv 1 0 0\nv 0 1 0\nv 0 1 1\n";
    const std::string floor_face = "f 1 2 3 4\n", wall_face = "f 1 5 6 2\n";
    REQUIRE(obj_mesh(fold + floor_face + wall_face).vertices.size() == 8);                       // 90 degrees: past the crease
    REQUIRE(obj_mesh(fold + "s 1\n" + floor_face + wall_face).vertices.size() == 6);             // one group: smooth across
    REQUIRE(obj_mesh(fold + "s 1\n" + floor_face + "s 2\n" + wall_face).vertices.size() == 8);   // two groups: apart
    REQUIRE(obj_mesh(fold + "s 0\n" + floor_face + wall_face).vertices.size() == 8);
    const assets::Mesh folded = obj_mesh(fold + "s 1\n" + floor_face + wall_face);
    REQUIRE(folded.vertices[0].normal.x == Catch::Approx(std::sqrt(0.5f)).margin(1e-5));        // the shared edge leans halfway
    REQUIRE(folded.vertices[0].normal.y == Catch::Approx(std::sqrt(0.5f)).margin(1e-5));
    // A face without area on the edge (a corner written twice, a T-junction's sliver, as CAD
    // tessellators leave them) has no direction to agree with: it joins neither side, and the edge
    // stays sharp.
    for (const std::string sliver : {"f 1 2 2\n", "v 0 0 0.5\nf 1 7 2\n"}) {
        const assets::Mesh m = obj_mesh(fold + floor_face + wall_face + sliver);
        INFO(sliver);
        for (std::size_t i = 0; i < 4; ++i) REQUIRE(m.vertices[i].normal.y == Catch::Approx(1.0f));        // the floor's
        for (std::size_t i = 4; i < 8; ++i) REQUIRE(m.vertices[i].normal.x == Catch::Approx(1.0f));        // the wall's
    }

    // A drilled hole or a round tower: sixteen sides 22.5 degrees apart are one smooth wall whose
    // normals point straight out from the axis, and the flat caps keep their own.
    const assets::Mesh tower = obj_mesh(obj_cylinder(16));
    REQUIRE(tower.indices.size() / 3 == 16 * 2 + 2 * 14);
    REQUIRE(tower.vertices.size() == 64);   // each position once on the wall, once on its cap
    int wall = 0, cap = 0;
    for (const auto& v : tower.vertices) {
        if (std::fabs(v.normal.y) > 0.9999f) {
            ++cap;
            REQUIRE(v.normal.y * (v.position.y - 0.5f) > 0);   // the top faces up, the bottom down
        } else {
            ++wall;
            REQUIRE(v.normal.y == Catch::Approx(0.0f).margin(1e-5));
            REQUIRE(v.normal.x == Catch::Approx(v.position.x).margin(1e-4));   // radial: the unit circle's own direction
            REQUIRE(v.normal.z == Catch::Approx(v.position.z).margin(1e-4));
        }
    }
    REQUIRE(wall == 32);
    REQUIRE(cap == 32);
    // A crease of 10 degrees makes the same sides facets.
    assets::ImportSettings fine;
    fine.crease = 10;
    REQUIRE(obj_mesh(obj_cylinder(16), fine).vertices.size() == 96);
    // Faces with the file's own normals keep them.
    const assets::Mesh given = obj_mesh(cube + "vn 0 0 1\n" + "f 1//1 4//1 3//1 2//1\n");
    for (const auto& v : given.vertices) REQUIRE(v.normal.z == Catch::Approx(1.0f));
}

TEST_CASE("OBJ polygons: convex ones fanned as before, concave ones ear-clipped in their plane", "[assets][obj][ngon]") {
    // An L written from its tip, (1.2, 0): fanned from there, two of its triangles would face down
    // and cover the notch. In the floor plane, counter-clockwise from above (z = -y of the drawing).
    std::string l_shape = "v 0 0 0\nv 1.2 0 0\nv 1.2 0 -0.5\nv 0.5 0 -0.5\nv 0.5 0 -1.2\nv 0 0 -1.2\n";
    const assets::Mesh l = obj_mesh(l_shape + "f 2 3 4 5 6 1\n");
    REQUIRE(l.indices.size() == 12);
    TriangleCheck lc = triangles_of(l, Vec3{0, 1, 0});
    REQUIRE(lc.facing);
    REQUIRE(lc.area == Catch::Approx(1.2 * 0.5 + 0.5 * 0.7).epsilon(1e-5));
    // The same from each of its corners, and wound the other way (facing down).
    for (int start = 0; start < 6; ++start) {
        std::string f = "f", back = "f";
        for (int k = 0; k < 6; ++k) f += std::format(" {}", (start + k) % 6 + 1);
        for (int k = 0; k < 6; ++k) back += std::format(" {}", (start + 6 - k) % 6 + 1);
        const TriangleCheck up = triangles_of(obj_mesh(l_shape + f + "\n"), Vec3{0, 1, 0});
        const TriangleCheck down = triangles_of(obj_mesh(l_shape + back + "\n"), Vec3{0, -1, 0});
        INFO(f);
        REQUIRE(up.facing);
        REQUIRE(down.facing);
        REQUIRE(up.area == Catch::Approx(0.95).epsilon(1e-5));
        REQUIRE(down.area == Catch::Approx(0.95).epsilon(1e-5));
    }
    // A corner written twice (an edge an exporter collapsed) turns neither way: it is left out, so it
    // neither passes the L for convex nor hides the ears beside it.
    for (const char* f : {"f 2 3 4 4 5 6 1\n", "f 2 2 3 4 5 6 1 1\n", "f 2 3 4 5 5 5 6 1\n"}) {
        const TriangleCheck c = triangles_of(obj_mesh(l_shape + f), Vec3{0, 1, 0});
        INFO(f);
        REQUIRE(c.facing);
        REQUIRE(c.area == Catch::Approx(0.95).epsilon(1e-5));
    }
    // A concave quad written A B C C D A, clockwise from above: its ears are beside the repeats.
    const TriangleCheck quad = triangles_of(obj_mesh("v 0 0 1\nv 1.5 0 -1\nv 1 0 0.5\nv 0.5 0 0.5\nf 1 2 3 3 4 1\n"), Vec3{0, -1, 0});
    REQUIRE(quad.facing);
    REQUIRE(quad.area == Catch::Approx(0.5).epsilon(1e-5));
    // A five-pointed star standing in a tilted plane, and a comb, each from a corner a fan gets wrong.
    std::string star;
    const Vec3 ux = normalize(Vec3{1, 0, 1}), uy = Vec3{0, 1, 0};
    double star_area = 0;
    std::vector<std::array<double, 2>> pts;
    for (int k = 0; k < 10; ++k) {
        const double a = 3.141592653589793 / 2 + k * 3.141592653589793 / 5, r = k % 2 == 0 ? 0.75 : 0.3;
        pts.push_back({r * std::cos(a), r * std::sin(a)});
        const Vec3 p = ux * static_cast<float>(pts.back()[0]) + uy * static_cast<float>(pts.back()[1]);
        star += std::format("v {:.9f} {:.9f} {:.9f}\n", p.x, p.y, p.z);
    }
    for (int k = 0; k < 10; ++k) star_area += 0.5 * (pts[k][0] * pts[(k + 1) % 10][1] - pts[(k + 1) % 10][0] * pts[k][1]);
    const assets::Mesh s = obj_mesh(star + "f 2 3 4 5 6 7 8 9 10 1\n");
    REQUIRE(s.indices.size() == 24);
    const TriangleCheck sc = triangles_of(s, cross(ux, uy));
    REQUIRE(sc.facing);
    REQUIRE(sc.area == Catch::Approx(star_area).epsilon(1e-4));
    const std::string comb = "v 0 0 0\nv 1.6 0 0\nv 1.6 0 -1\nv 1.3 0 -1\nv 1.3 0 -0.3\nv 1 0 -0.3\nv 1 0 -1\nv 0.7 0 -1\nv 0.7 0 -0.3\nv 0.4 0 -0.3\nv 0.4 0 -1\nv 0 0 -1\n";
    const TriangleCheck cc = triangles_of(obj_mesh(comb + "f 2 3 4 5 6 7 8 9 10 11 12 1\n"), Vec3{0, 1, 0});
    REQUIRE(cc.facing);
    REQUIRE(cc.area == Catch::Approx(1.6 * 1.0 - 2 * 0.3 * 0.7).epsilon(1e-5));
    // A convex polygon is the fan it always was: from its first corner, in order.
    const assets::Mesh pentagon = obj_mesh("v 0 0 0\nv 1 0 0\nv 1.3 0 -1\nv 0.5 0 -1.6\nv -0.3 0 -1\nf 1 2 3 4 5\n");
    REQUIRE(pentagon.indices == std::vector<std::uint32_t>{0, 1, 2, 0, 2, 3, 0, 3, 4});
    // The stress kit's polygon export: every polygon facing its normal, 320 triangles as Blender reads it.
    const std::filesystem::path dir = project() / "assets" / "ngon-test";
    std::filesystem::create_directories(dir);
    std::ofstream(dir / "flat.obj", std::ios::binary) << "mtllib none.mtl\no thing\n" << comb << "vn 0 1 0\nf 2//1 3//1 4//1 5//1 6//1 7//1 8//1 9//1 10//1 11//1 12//1 1//1\n";
    assets::AssetStore store(project());
    auto m = store.mesh("assets/ngon-test/flat.obj");
    REQUIRE(m.has_value());
    REQUIRE(triangles_of(**m, Vec3{0, 1, 0}).facing);
    std::filesystem::remove_all(dir);
    std::filesystem::remove_all(project() / ".imported" / "assets" / "ngon-test");
}

TEST_CASE("OBJ coordinates are doubles: a part far out is recentred, and its origin says where it was", "[assets][obj][precision]") {
    // A 6 mm square 4.5 km from the origin (plant or map coordinates, in metres). As floats there
    // its corners would be 0.49 mm steps apart; read as doubles and moved to its centre, they keep
    // their micrometres.
    const std::string far = "v 4500.000 0 4500.000\nv 4500.006 0 4500.000\nv 4500.006 0 4499.994\nv 4500.000 0 4499.994\nf 1 2 3 4\n";
    const assets::Mesh m = obj_mesh(far);
    REQUIRE(m.origin[0] == Catch::Approx(4500.003).margin(1e-9));
    REQUIRE(m.origin[1] == 0.0);
    REQUIRE(m.origin[2] == Catch::Approx(4499.997).margin(1e-9));
    REQUIRE(m.vertices[0].position.x == Catch::Approx(-0.003f).margin(1e-7));
    REQUIRE(m.vertices[1].position.x == Catch::Approx(0.003f).margin(1e-7));
    REQUIRE(m.vertices[2].position.z == Catch::Approx(-0.003f).margin(1e-7));
    const Json d = m.describe();
    REQUIRE(d["import"]["recentered"] == true);
    REQUIRE(d["import"]["origin"][0].get<double>() == Catch::Approx(4500.003).margin(1e-9));
    REQUIRE(d["size"][0].get<double>() == Catch::Approx(0.006).margin(1e-7));
    // Kept where the file has it, the corners land on float steps.
    assets::ImportSettings keep;
    keep.recenter = assets::ImportSettings::Recenter::Off;
    const assets::Mesh kept = obj_mesh(far, keep);
    REQUIRE(kept.origin == std::array<double, 3>{0, 0, 0});
    REQUIRE(kept.aabb_min.x == Catch::Approx(4500.0f));
    REQUIRE(std::fabs((kept.vertices[1].position.x - kept.vertices[0].position.x) - 0.006f) > 1e-5f);
    // A model near its own origin stays where it is (a crate at x 10 keeps its pivot)...
    const assets::Mesh near = obj_mesh("v 10 0 0\nv 11 0 0\nv 11 1 0\nf 1 2 3\n");
    REQUIRE(near.origin == std::array<double, 3>{0, 0, 0});
    REQUIRE(near.aabb_min.x == Catch::Approx(10.0f));
    // ...unless asked.
    assets::ImportSettings centre;
    centre.recenter = assets::ImportSettings::Recenter::On;
    const assets::Mesh moved = obj_mesh("v 10 0 0\nv 11 0 0\nv 11 1 0\nf 1 2 3\n", centre);
    REQUIRE(moved.origin[0] == Catch::Approx(10.5));
    REQUIRE(moved.aabb_min.x == Catch::Approx(-0.5f));
}

TEST_CASE("OBJ import settings: up axis and unit baked in, size in metres, Z up suggested, refused when wrong", "[assets][obj][units]") {
    // A Z-up millimetre box (CAD): 100 x 200 x 50 mm standing on z = 0.
    const std::string box = "v -50 -100 0\nv 50 -100 0\nv 50 100 0\nv -50 100 0\nv -50 -100 50\nv 50 -100 50\nv 50 100 50\nv -50 100 50\n"
                            "f 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n";
    const assets::Mesh as_is = obj_mesh(box);
    const Json d = as_is.describe();
    REQUIRE(d["size"][2].get<double>() == Catch::Approx(50.0));
    REQUIRE(d["hints"].size() == 2);
    REQUIRE(d["hints"][0].get<std::string>().find("up: \"z\"") != std::string::npos);
    REQUIRE(d["hints"][1].get<std::string>().find("unit: \"mm\"") != std::string::npos);
    REQUIRE(d["import"]["up"] == "y");
    auto z_mm = assets::import_settings(Json{{"up", "z"}, {"unit", "mm"}});
    REQUIRE(z_mm.has_value());
    REQUIRE(z_mm->z_up);
    REQUIRE(z_mm->unit == Catch::Approx(0.001));
    const assets::Mesh upright = obj_mesh(box, *z_mm);
    // Turned so its z is the engine's y, in metres: 0.1 wide, 0.05 tall, 0.2 deep, on the ground.
    REQUIRE(upright.aabb_min.y == Catch::Approx(0.0f).margin(1e-7));
    REQUIRE(upright.aabb_max.y == Catch::Approx(0.05f));
    REQUIRE(upright.aabb_max.x - upright.aabb_min.x == Catch::Approx(0.1f));
    REQUIRE(upright.aabb_max.z - upright.aabb_min.z == Catch::Approx(0.2f));
    REQUIRE_FALSE(upright.describe().contains("hints"));
    REQUIRE(upright.describe()["import"]["unit"].get<double>() == Catch::Approx(0.001));
    // The top is up, and every face still winds toward its normal (the turn keeps the winding).
    bool top = false;
    for (const auto& v : upright.vertices) top = top || (v.position.y > 0.049f && v.normal.y > 0.9999f);
    REQUIRE(top);
    for (std::size_t k = 0; k < upright.indices.size(); k += 3) {
        const auto& a = upright.vertices[upright.indices[k]];
        const Vec3 n = cross(upright.vertices[upright.indices[k + 1]].position - a.position, upright.vertices[upright.indices[k + 2]].position - a.position);
        REQUIRE(dot(n, a.normal) > 0);
    }
    // Settings refused name what they take.
    auto refused = [](const Json& j, const char* says) {
        auto r = assets::import_settings(j);
        REQUIRE_FALSE(r.has_value());
        INFO(r.error().message);
        REQUIRE(r.error().message.find(says) != std::string::npos);
    };
    refused(Json{{"units", 1}}, "up, unit, recenter and crease");
    refused(Json{{"up", "x"}}, "\"y\" or \"z\"");
    refused(Json{{"unit", -1}}, "above 0");
    refused(Json{{"unit", "furlong"}}, "\"mm\"");
    refused(Json{{"recenter", "yes"}}, "\"auto\"");
    refused(Json{{"crease", 200}}, "0 to 180");
    REQUIRE(assets::import_settings(Json{{"unit", 0.0254}, {"recenter", "auto"}, {"crease", 45}})->crease == 45);
    // Kept per file by the store: assets.import's settings over the project's table over the defaults.
    const std::filesystem::path dir = project() / "assets" / "units-test";
    std::filesystem::create_directories(dir);
    std::ofstream(dir / "box.obj", std::ios::binary) << box;
    std::ofstream(dir / "tri.stl", std::ios::binary) << "solid t\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid t\n";
    assets::AssetStore store(project());
    REQUIRE((*store.mesh("assets/units-test/box.obj"))->aabb_max.y == Catch::Approx(100.0f));
    store.set_project_imports(Json{{"assets/units-test/box.obj", Json{{"up", "z"}, {"unit", 0.001}}}});
    REQUIRE((*store.mesh("assets/units-test/box.obj"))->aabb_max.y == Catch::Approx(0.05f));   // read again under the table's settings
    auto answer = store.import("assets/units-test/box.obj", false, Json{{"unit", "cm"}});
    INFO((answer ? answer->dump() : answer.error().to_string()));
    REQUIRE(answer.has_value());
    REQUIRE((*answer)["mesh"]["import"]["up"] == "z");   // the table's up, kept under the unit given
    REQUIRE((*answer)["mesh"]["size"][1].get<double>() == Catch::Approx(0.5));
    REQUIRE((*answer)["project_toml"].get<std::string>().find("\"assets/units-test/box.obj\" = { up = \"z\", unit = 0.01") != std::string::npos);
    REQUIRE((*store.mesh("assets/units-test/box.obj"))->aabb_max.y == Catch::Approx(0.5f));
    REQUIRE_FALSE(store.import("assets/units-test/tri.stl", false, Json{{"up", "z"}}).has_value());
    REQUIRE_FALSE(store.import("assets/units-test/box.obj", false, Json{{"up", "sideways"}}).has_value());
    // A wrong entry in the table fails the file's load with where it is.
    assets::AssetStore other(project());
    other.set_project_imports(Json{{"assets/units-test/box.obj", Json{{"unit", "parsec"}}}});
    auto wrong = other.mesh("assets/units-test/box.obj");
    REQUIRE_FALSE(wrong.has_value());
    REQUIRE(wrong.error().message.find("project.toml [assets.import] \"assets/units-test/box.obj\"") != std::string::npos);
    other.set_project_imports(Json::object());
    REQUIRE(other.mesh("assets/units-test/box.obj").has_value());
    std::filesystem::remove_all(dir);
    std::filesystem::remove_all(project() / ".imported" / "assets" / "units-test");
}

TEST_CASE("a parsed OBJ is kept in .imported/ and read back while its text, materials and settings stay the same", "[assets][obj][cache]") {
    const std::filesystem::path dir = project() / "assets" / "objcache-test";
    const std::filesystem::path cache = project() / ".imported" / "assets" / "objcache-test" / "part.obj.mesh";
    std::filesystem::remove_all(dir);
    std::filesystem::remove_all(cache.parent_path());
    std::filesystem::create_directories(dir);
    std::ofstream(dir / "part.mtl", std::ios::binary) << "newmtl Steel\nKd 0.5 0.5 0.55\nPm 1\nmap_Kd ../checker.png\n";
    std::ofstream(dir / "part.obj", std::ios::binary) << "mtllib part.mtl\no Part\nusemtl Steel\n" << obj_cylinder(16) << "o Lid\nv 0 2 0\nv 1 2 0\nv 0 2 1\nvt 0 0\nvt 1 0\nvt 0 1\nf -3/1 -1/3 -2/2\n";
    auto parsed = [&](assets::AssetStore& s) {
        auto m = s.mesh("assets/objcache-test/part.obj");
        INFO((m ? std::string() : m.error().to_string()));
        REQUIRE(m.has_value());
        return *m;
    };
    assets::AssetStore first(project());
    const assets::Mesh& a = *parsed(first);
    REQUIRE_FALSE(a.cached);
    REQUIRE(std::filesystem::is_regular_file(cache));
    assets::AssetStore second(project());
    const assets::Mesh& b = *parsed(second);
    REQUIRE(b.cached);
    REQUIRE(b.describe()["import"]["cached"] == true);
    // The same mesh to the bit: vertices (with their tangents), triangles, parts and materials.
    REQUIRE(b.vertices.size() == a.vertices.size());
    REQUIRE(std::memcmp(b.vertices.data(), a.vertices.data(), a.vertices.size() * sizeof(assets::MeshVertex)) == 0);
    REQUIRE(b.indices == a.indices);
    REQUIRE(b.submeshes.size() == a.submeshes.size());
    for (std::size_t i = 0; i < a.submeshes.size(); ++i) {
        REQUIRE(b.submeshes[i].first_index == a.submeshes[i].first_index);
        REQUIRE(b.submeshes[i].index_count == a.submeshes[i].index_count);
        REQUIRE(b.submeshes[i].material == a.submeshes[i].material);
        REQUIRE(b.submeshes[i].origin == a.submeshes[i].origin);
    }
    REQUIRE(b.nodes.size() == 2);
    REQUIRE(b.nodes[1].name == "Lid");
    REQUIRE(b.materials[0].name == "Steel");
    REQUIRE(b.materials[0].metallic == 1.0f);
    REQUIRE(b.materials[0].texture == "assets/checker.png");
    REQUIRE(b.aabb_max.y == a.aabb_max.y);
    Json da = a.describe(), db = b.describe();
    da["import"].erase("cached");
    db["import"].erase("cached");
    REQUIRE(da == db);
    // A changed material library, text or setting is parsed again.
    std::ofstream(dir / "part.mtl", std::ios::binary) << "newmtl Steel\nKd 0.9 0.1 0.1\n";
    assets::AssetStore third(project());
    REQUIRE_FALSE(parsed(third)->cached);
    REQUIRE(parsed(third)->materials[0].base_color.x == Catch::Approx(0.9f));
    assets::AssetStore fourth(project());
    REQUIRE(parsed(fourth)->cached);
    fourth.set_project_imports(Json{{"assets/objcache-test/part.obj", Json{{"crease", 5}}}});
    REQUIRE_FALSE(parsed(fourth)->cached);
    REQUIRE(parsed(fourth)->vertices.size() > a.vertices.size());
    std::ofstream(dir / "part.obj", std::ios::app | std::ios::binary) << "f -1 -2 -3\n";
    assets::AssetStore fifth(project());
    REQUIRE_FALSE(parsed(fifth)->cached);
    REQUIRE(parsed(fifth)->indices.size() == a.indices.size() + 3);
    // A cache cut short or written by something else is parsed past, and written again.
    std::filesystem::resize_file(cache, std::filesystem::file_size(cache) / 2);
    assets::AssetStore sixth(project());
    REQUIRE_FALSE(parsed(sixth)->cached);
    assets::AssetStore seventh(project());
    REQUIRE(parsed(seventh)->cached);
    // assets.import with force parses again.
    auto forced = seventh.import("assets/objcache-test/part.obj", true);
    REQUIRE(forced.has_value());
    REQUIRE((*forced)["cached"] == false);
    REQUIRE((*forced)["cache"] == ".imported/assets/objcache-test/part.obj.mesh");
    // A material library that is missing is said to be each time, cached or not.
    auto last = [] {
        const auto r = log::global().recent(1);
        return r.empty() ? std::uint64_t{0} : r.back().seq;
    };
    auto warned_since = [](std::uint64_t seq) {
        for (const log::Record& r : log::global().recent(64, log::Level::Warn))
            if (r.seq > seq && r.message.find("material library assets/objcache-test/part.mtl") != std::string::npos) return true;
        return false;
    };
    std::filesystem::remove(dir / "part.mtl");
    std::uint64_t mark = last();
    assets::AssetStore eighth(project());
    REQUIRE_FALSE(parsed(eighth)->cached);
    REQUIRE(warned_since(mark));
    mark = last();
    assets::AssetStore ninth(project());
    REQUIRE(parsed(ninth)->cached);
    REQUIRE(warned_since(mark));
    std::filesystem::remove_all(dir);
    std::filesystem::remove_all(cache.parent_path());
}

TEST_CASE("STL files, binary and ASCII, become one flat-shaded gray mesh", "[assets][stl]") {
    const std::filesystem::path dir = project() / "assets" / "stl-test";
    std::filesystem::create_directories(dir);
    {
        std::ofstream out(dir / "tri.stl", std::ios::binary);
        std::string header(80, ' ');
        out.write(header.data(), 80);
        const std::uint32_t count = 1;
        out.write(reinterpret_cast<const char*>(&count), 4);
        const float f[12] = {0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0};
        out.write(reinterpret_cast<const char*>(f), sizeof f);
        const std::uint16_t attr = 0;
        out.write(reinterpret_cast<const char*>(&attr), 2);
    }
    std::ofstream(dir / "tri-ascii.stl", std::ios::binary) << "solid t\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid t\n";
    assets::AssetStore store(project());
    auto b = store.mesh("assets/stl-test/tri.stl");
    REQUIRE(b.has_value());
    REQUIRE((*b)->importer == "stl");
    REQUIRE((*b)->indices.size() == 3);
    REQUIRE((*b)->aabb_max.x == Catch::Approx(2.0f));
    REQUIRE((*b)->vertices[0].normal.z == Catch::Approx(1.0f));
    auto a = store.mesh("assets/stl-test/tri-ascii.stl");
    REQUIRE(a.has_value());
    REQUIRE((*a)->indices.size() == 3);
    REQUIRE((*a)->aabb_max.y == Catch::Approx(1.0f));
    std::filesystem::remove_all(dir);
}

TEST_CASE("vertex colors come in from glTF (COLOR_0, floats or normalized bytes) and from OBJ (r g b after a vertex)", "[assets][vcolor]") {
    const std::filesystem::path dir = project() / "assets" / "vcolor-test";
    std::filesystem::create_directories(dir);
    // One triangle twice: colors as float RGB, then as normalized unsigned-byte RGBA.
    std::ofstream(dir / "tri.gltf", std::ios::binary) << R"({"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0, 1]}],
        "nodes": [{"mesh": 0}, {"mesh": 1}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "COLOR_0": 1}}]}, {"primitives": [{"attributes": {"POSITION": 0, "COLOR_0": 2}}]}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
                      {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
                      {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC4"}],
        "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}, {"buffer": 0, "byteOffset": 36, "byteLength": 36}, {"buffer": 0, "byteOffset": 72, "byteLength": 12}],
        "buffers": [{"byteLength": 84, "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA//wAA/wD/AIAAAP//"}]})";
    assets::AssetStore store(project());
    auto g = store.mesh("assets/vcolor-test/tri.gltf");
    INFO((g ? std::string() : g.error().to_string()));
    REQUIRE(g.has_value());
    REQUIRE((*g)->vertex_colors);
    REQUIRE((*g)->vertices.size() == 6);
    REQUIRE((*g)->vertices[0].color.x == Catch::Approx(1.0f));
    REQUIRE((*g)->vertices[1].color.y == Catch::Approx(1.0f));
    REQUIRE((*g)->vertices[2].color.z == Catch::Approx(1.0f));
    REQUIRE((*g)->vertices[2].color.w == Catch::Approx(1.0f));
    REQUIRE((*g)->vertices[4].color.y == Catch::Approx(1.0f));
    REQUIRE((*g)->vertices[4].color.w == Catch::Approx(128.0f / 255.0f).margin(1e-3));
    REQUIRE(store.describe("assets/vcolor-test/tri.gltf")["vertex_colors"] == true);
    // OBJ: sRGB colors after the positions; a vertex without one is white.
    std::ofstream(dir / "paint.obj", std::ios::binary) << "v 0 0 0 1 0 0\nv 1 0 0 0.5 0.5 0.5\nv 0 1 0\nf 1 2 3\n";
    auto o = store.mesh("assets/vcolor-test/paint.obj");
    REQUIRE(o.has_value());
    REQUIRE((*o)->vertex_colors);
    REQUIRE((*o)->vertices[0].color.x == Catch::Approx(1.0f));
    REQUIRE((*o)->vertices[0].color.y == Catch::Approx(0.0f));
    REQUIRE((*o)->vertices[1].color.x == Catch::Approx(0.214f).margin(0.002));   // sRGB 0.5 in linear light
    REQUIRE((*o)->vertices[2].color.x == Catch::Approx(1.0f));
    // A file without colors has none.
    std::ofstream(dir / "plain.obj", std::ios::binary) << "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    auto p = store.mesh("assets/vcolor-test/plain.obj");
    REQUIRE(p.has_value());
    REQUIRE_FALSE((*p)->vertex_colors);
    std::filesystem::remove_all(dir);
    std::filesystem::remove_all(project() / ".imported" / "assets" / "vcolor-test");
}

TEST_CASE("the built-in humanoid: a skinned model with its clips, its colours from the query", "[assets][humanoid]") {
    auto plain = assets::humanoid_glb("");
    REQUIRE(plain.has_value());
    auto m = assets::parse_gltf(*plain, std::filesystem::temp_directory_path(), "humanoid");
    REQUIRE(m.has_value());
    REQUIRE(m->skinned());
    REQUIRE(m->skins.size() == 1);
    REQUIRE(m->skins[0].joints.size() == 11);
    std::vector<std::string> names;
    for (const assets::AnimationClip& c : m->animations) names.push_back(c.name);
    for (const char* want : {"idle", "walk", "run", "walk_back", "strafe_left", "strafe_right", "crouch", "crouch_walk", "jump", "wave", "punch", "die", "sit", "talk", "cheer", "pickup"})
        REQUIRE(std::find(names.begin(), names.end(), want) != names.end());
    REQUIRE(m->aabb_min.y == Catch::Approx(0.0f).margin(1e-4));     // standing on its feet at the origin
    REQUIRE(m->aabb_max.y == Catch::Approx(2.02f).margin(0.01));    // two metres and a hair
    // A red shirt (sRGB #c83c32 in linear light), no hair: two boxes fewer.
    auto red = assets::parse_gltf(*assets::humanoid_glb("shirt=red&hair=none"), std::filesystem::temp_directory_path(), "humanoid?shirt=red&hair=none");
    REQUIRE(red.has_value());
    const auto shirt = std::find_if(red->materials.begin(), red->materials.end(), [](const assets::Material& mm) { return mm.name == "shirt"; });
    REQUIRE(shirt != red->materials.end());
    REQUIRE(shirt->base_color.x == Catch::Approx(0.5776f).margin(0.01));
    REQUIRE(shirt->base_color.y == Catch::Approx(0.0452f).margin(0.01));
    REQUIRE(red->vertices.size() + 2 * 24 == m->vertices.size());
    // A part it does not have and a colour it cannot read are said by name.
    auto bad = assets::humanoid_glb("hat=red");
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().message.find("hat") != std::string::npos);
    REQUIRE_FALSE(assets::humanoid_glb("shirt=sparkly").has_value());
}

TEST_CASE("a large terrain is drawn in squares whose coarser levels stay within their errors", "[assets][terrain][lod]") {
    const assets::Terrain t = assets::terrain_from_noise(5, 30.0f, 5, 257, {128, 128}, 16.0f);
    const assets::Mesh m = assets::terrain_mesh(t, {}, "terrain:lod@1");
    REQUIRE(m.chunks.size() == 64);   // 256 cells across in squares of 32
    std::size_t grid = 0;
    for (const assets::Submesh& sm : m.submeshes) grid += sm.index_count;
    REQUIRE(m.submeshes.size() == 64);
    REQUIRE(grid == 256u * 256u * 6u);   // the full grid, once: what collides
    const std::size_t samples = 257u * 257u;
    REQUIRE(m.vertices.size() > samples);   // the skirts' own vertices
    for (std::size_t c = 0; c < m.chunks.size(); c += 9) {
        const assets::TerrainChunk& ch = m.chunks[c];
        REQUIRE(ch.levels.size() == 6);   // strides 1 to 32
        for (std::size_t k = 0; k < ch.levels.size(); ++k) {
            const assets::TerrainChunk::Level& l = ch.levels[k];
            REQUIRE(l.stride == (1 << k));
            if (k > 0) REQUIRE(l.error >= ch.levels[k - 1].error);
            // Every point of the level's own triangles is within its error of the ground.
            for (std::uint32_t i = l.first; i + 2 < l.first + l.count; i += 3) {
                const std::uint32_t a = m.indices[i], b = m.indices[i + 1], d = m.indices[i + 2];
                if (a >= samples || b >= samples || d >= samples) continue;   // a skirt
                const Vec3 p0 = m.vertices[a].position, p1 = m.vertices[b].position, p2 = m.vertices[d].position;
                REQUIRE(cross(p1 - p0, p2 - p0).y > 0);
                for (float u : {0.2f, 0.45f})
                    for (float v : {0.15f, 0.4f}) {
                        const Vec3 q = p0 + (p1 - p0) * u + (p2 - p0) * v;
                        REQUIRE(std::fabs(t.sample(q.x, q.z) - q.y) <= l.error + 1e-3f);
                    }
            }
            REQUIRE(length(m.vertices[m.indices[l.first]].position - ch.center) <= ch.radius + 1e-3f);
        }
    }
}

TEST_CASE("terrains: noise from a seed, heights the mesh draws, a 16-bit PNG that reads back", "[assets][terrain]") {
    const assets::Terrain a = assets::terrain_from_noise(7, 20.0f, 4, 65, {40, 30}, 10.0f);
    const assets::Terrain b = assets::terrain_from_noise(7, 20.0f, 4, 65, {40, 30}, 10.0f);
    const assets::Terrain c = assets::terrain_from_noise(8, 20.0f, 4, 65, {40, 30}, 10.0f);
    REQUIRE(a.n == 65);
    REQUIRE(a.h == b.h);   // the same seed makes the same hills
    REQUIRE(a.h != c.h);
    REQUIRE(*std::min_element(a.h.begin(), a.h.end()) == Catch::Approx(0).margin(1e-5));
    REQUIRE(*std::max_element(a.h.begin(), a.h.end()) == Catch::Approx(10));
    // sample() answers what the mesh's triangles hold: a point inside one is on its plane.
    const assets::Mesh m = assets::terrain_mesh(a, {}, "terrain:test@1");
    REQUIRE(m.vertices.size() == 65u * 65u);   // one square: no skirts
    REQUIRE(m.submeshes.size() == 1);
    REQUIRE(m.submeshes[0].first_index == 0);
    REQUIRE(m.submeshes[0].index_count == 64u * 64u * 6u);
    REQUIRE(m.vertex_colors);
    REQUIRE(m.aabb_min.x == Catch::Approx(-20));
    REQUIRE(m.aabb_max.z == Catch::Approx(15));
    for (int k = 0; k < 200; ++k) {
        const std::size_t tri = static_cast<std::size_t>((k * 7919) % (m.submeshes[0].index_count / 3));
        const Vec3 p0 = m.vertices[m.indices[tri * 3]].position, p1 = m.vertices[m.indices[tri * 3 + 1]].position, p2 = m.vertices[m.indices[tri * 3 + 2]].position;
        const float u = 0.2f + 0.1f * static_cast<float>(k % 3), v = 0.25f;
        const Vec3 q = p0 + (p1 - p0) * u + (p2 - p0) * v;
        INFO("triangle " << tri);
        REQUIRE(a.sample(q.x, q.z) == Catch::Approx(q.y).margin(1e-3));
        const Vec3 n = normalize(cross(p1 - p0, p2 - p0));
        const Vec3 up = n.y < 0 ? n * -1.0f : n;
        REQUIRE(a.normal(q.x, q.z).y == Catch::Approx(up.y).margin(1e-3));
        REQUIRE(n.y > 0);   // wound to face up
    }
    // Outside the grid, the nearest edge answers.
    REQUIRE(a.sample(1000, 0) == Catch::Approx(a.sample(20, 0)));
    // The 16-bit PNG reads back to within a step of 65535.
    const std::string png = assets::terrain_png16(a);
    REQUIRE(png.substr(1, 3) == "PNG");
    auto back = assets::terrain_from_image(png, "heights.png", 65, {40, 30}, 10.0f);
    REQUIRE(back.has_value());
    float worst = 0;
    for (std::size_t k = 0; k < a.h.size(); ++k) worst = std::max(worst, std::fabs(back->h[k] - a.h[k]));
    INFO("worst " << worst);
    REQUIRE(worst < 10.0f / 65535.0f * 1.01f);
    // Resampled to another resolution it keeps its corners.
    auto coarse = assets::terrain_from_image(png, "heights.png", 17, {40, 30}, 10.0f);
    REQUIRE(coarse.has_value());
    REQUIRE(coarse->at(0, 0) == Catch::Approx(a.at(0, 0)).margin(1e-3));
    REQUIRE(coarse->at(16, 16) == Catch::Approx(a.at(64, 64)).margin(1e-3));
    REQUIRE_FALSE(assets::terrain_from_image("not a picture", "x.png", 17, {1, 1}, 1).has_value());
}

namespace {
std::string parse_ply_error(assets::AssetStore& store, const std::string& path) {
    auto m = store.mesh(path);
    return m ? std::string() : m.error().message;
}
}  // namespace

TEST_CASE("PLY files, ASCII and binary in either byte order, with normals, uvs and colours, and what they refuse", "[assets][ply]") {
    const std::filesystem::path dir = project() / "assets" / "ply-test";
    std::filesystem::create_directories(dir);
    // A unit square as one quad, coloured red, green, blue and white (bytes, sRGB), no normals, and
    // a comment and an extra property to skip.
    std::ofstream(dir / "quad.ply", std::ios::binary) << "ply\nformat ascii 1.0\ncomment made by hand\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\n"
                                        "property uchar red\nproperty uchar green\nproperty uchar blue\nproperty float quality\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n"
                                        "0 0 0 255 0 0 1\n1 0 0 0 255 0 1\n1 0 -1 0 0 255 1\n0 0 -1 255 255 255 1\n4 0 1 2 3\n";
    // One triangle with normals and uvs, as binary, both byte orders; a trailing element of edges to skip.
    auto binary = [&](const char* name, bool big) {
        std::ofstream out(dir / name, std::ios::binary);
        out << "ply\nformat " << (big ? "binary_big_endian" : "binary_little_endian") << " 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\n"
            << "property float nx\nproperty float ny\nproperty float nz\nproperty float s\nproperty float t\nelement face 1\nproperty list uchar uint vertex_indices\n"
            << "element edge 1\nproperty int vertex1\nproperty int vertex2\nend_header\n";
        auto put = [&](const void* p, std::size_t n) {
            unsigned char b[8];
            std::memcpy(b, p, n);
            if (big) std::reverse(b, b + n);
            out.write(reinterpret_cast<const char*>(b), static_cast<std::streamsize>(n));
        };
        const float v[3][8] = {{0, 0, 0, 0, 0, 1, 0, 0}, {2, 0, 0, 0, 0, 1, 1, 0}, {0, 3, 0, 0, 0, 1, 0, 1}};
        for (const auto& row : v) for (float f : row) put(&f, 4);
        const std::uint8_t three = 3;
        out.write(reinterpret_cast<const char*>(&three), 1);
        for (std::uint32_t i : {0u, 1u, 2u}) put(&i, 4);
        for (std::int32_t i : {0, 1}) put(&i, 4);
    };
    binary("tri-le.ply", false);
    binary("tri-be.ply", true);
    assets::AssetStore store(project());
    auto q = store.mesh("assets/ply-test/quad.ply");
    INFO((q ? std::string() : q.error().to_string()));
    REQUIRE(q.has_value());
    REQUIRE((*q)->importer == "ply");
    REQUIRE((*q)->indices.size() == 6);                                   // the quad fanned into two triangles
    REQUIRE((*q)->vertex_colors);
    REQUIRE((*q)->vertices[0].color.x == Catch::Approx(1.0f));
    REQUIRE((*q)->vertices[0].color.y == Catch::Approx(0.0f));
    REQUIRE((*q)->vertices[3].color.z == Catch::Approx(1.0f));
    REQUIRE((*q)->materials[0].base_color.x == Catch::Approx(1.0f));      // white: the vertices carry the colour
    REQUIRE((*q)->vertices[0].normal.y == Catch::Approx(1.0f).margin(1e-4));   // smoothed: the square faces up
    REQUIRE((*q)->aabb_min.z == Catch::Approx(-1.0f));
    for (const char* name : {"assets/ply-test/tri-le.ply", "assets/ply-test/tri-be.ply"}) {
        auto t = store.mesh(name);
        INFO(name << ": " << (t ? std::string() : t.error().to_string()));
        REQUIRE(t.has_value());
        REQUIRE((*t)->indices.size() == 3);
        REQUIRE_FALSE((*t)->vertex_colors);
        REQUIRE((*t)->aabb_max.x == Catch::Approx(2.0f));
        REQUIRE((*t)->aabb_max.y == Catch::Approx(3.0f));
        REQUIRE((*t)->vertices[2].normal.z == Catch::Approx(1.0f));
        REQUIRE((*t)->vertices[1].uv.x == Catch::Approx(1.0f));
        REQUIRE((*t)->vertices[2].uv.y == Catch::Approx(0.0f));            // t 1 at the top of the image
        REQUIRE((*t)->vertices[0].uv.y == Catch::Approx(1.0f));
    }
    // Refused: points without faces, a face past the vertices, a body shorter than the header.
    std::ofstream(dir / "points.ply", std::ios::binary) << "ply\nformat ascii 1.0\nelement vertex 2\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 1 1\n";
    std::ofstream(dir / "past.ply", std::ios::binary) << "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 7\n";
    std::ofstream(dir / "short.ply", std::ios::binary) << "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 0 0\n";
    const auto points = parse_ply_error(store, "assets/ply-test/points.ply");
    REQUIRE(points.find("point cloud") != std::string::npos);
    REQUIRE(parse_ply_error(store, "assets/ply-test/past.ply").find("vertex 7") != std::string::npos);
    REQUIRE(parse_ply_error(store, "assets/ply-test/short.ply").find("ends before") != std::string::npos);
    std::filesystem::remove_all(dir);
}

TEST_CASE("terrain layers: shares from slope and height rules stacked in order, paint over them, a layer map that reads back", "[assets][terrain][layers]") {
    // A flat strip, then a ramp rising two units a cell (63 degrees) to the terrain's full height.
    assets::Terrain t;
    t.n = 9;
    t.size_x = t.size_z = 8;
    t.height = 8;
    t.h.resize(81);
    for (int j = 0; j < 9; ++j)
        for (int i = 0; i < 9; ++i) t.h[static_cast<std::size_t>(j * 9 + i)] = i < 4 ? 0.0f : static_cast<float>(i - 4) * 2.0f;
    assets::TerrainLook look;
    look.layers.push_back({"grass.png", {1, 1, 1}, 4, {0, 90}, {0, 1}, 1});
    look.layers.push_back({"rock.png", {1, 1, 1}, 4, {30, 90}, {0, 1}, 1});
    look.layers.push_back({"snow.png", {1, 1, 1}, 4, {0, 90}, {0.8, 1}, 1});
    look.layers.push_back({"road.png", {1, 1, 1}, 4, {0, 90}, {0, 1}, 0});   // only where painted
    auto shares = assets::terrain_layer_weights(t, look);
    REQUIRE(shares.size() == 81u);
    auto at = [&](int i, int j) { return shares[static_cast<std::size_t>(j * 9 + i)]; };
    for (const auto& w : shares) REQUIRE(w[0] + w[1] + w[2] + w[3] == Catch::Approx(1).margin(1e-5));
    REQUIRE(at(1, 4)[0] == Catch::Approx(1));        // flat and low: the first layer
    REQUIRE(at(5, 4)[1] == Catch::Approx(1));        // steep: rock
    REQUIRE(at(8, 4)[2] == Catch::Approx(1));        // at the top: snow, over the rock
    for (const auto& w : shares) REQUIRE(w[3] == 0);   // cover 0 and unpainted: no road
    // Paint: half road on the flat, a full coat of it on the ramp.
    t.layer_paint.assign(81, {0, 0, 0, 0});
    t.layer_paint[4 * 9 + 1] = {0, 0, 0, 0.5f};
    t.layer_paint[4 * 9 + 5] = {0, 0, 0, 1};
    shares = assets::terrain_layer_weights(t, look);
    REQUIRE(at(1, 4)[0] == Catch::Approx(0.5));
    REQUIRE(at(1, 4)[3] == Catch::Approx(0.5));
    REQUIRE(at(5, 4)[3] == Catch::Approx(1));
    REQUIRE(at(5, 4)[1] == Catch::Approx(0));
    // More paint than ground is scaled back to it.
    t.layer_paint[4 * 9 + 2] = {1, 1, 0, 0};
    shares = assets::terrain_layer_weights(t, look);
    REQUIRE(at(2, 4)[0] == Catch::Approx(0.5));
    REQUIRE(at(2, 4)[1] == Catch::Approx(0.5));
    // The layer map writes and reads back within an 8-bit step.
    const std::string png = assets::terrain_layers_png(t);
    auto back = assets::terrain_layers_from_image(png, "layers.png", 9);
    REQUIRE(back.has_value());
    REQUIRE(back->size() == 81u);
    for (std::size_t k = 0; k < 81; ++k)
        for (std::size_t c = 0; c < 4; ++c) REQUIRE((*back)[k][c] == Catch::Approx(std::min(t.layer_paint[k][c], 1.0f) / std::max(1.0f, t.layer_paint[k][0] + t.layer_paint[k][1] + t.layer_paint[k][2] + t.layer_paint[k][3])).margin(0.003));
    // The mesh made with layers is white where unpainted, and its material carries the shares.
    const assets::Mesh mesh = assets::terrain_mesh(t, look, "terrain:test");
    REQUIRE(mesh.materials.size() == 1u);
    REQUIRE(mesh.materials[0].terrain != nullptr);
    REQUIRE(mesh.materials[0].terrain->layers.size() == 4u);
    REQUIRE(mesh.materials[0].terrain->weights.size() == 81u * 4u);
    REQUIRE(mesh.materials[0].terrain->weights[(4 * 9 + 5) * 4 + 3] == 255);
    REQUIRE(mesh.vertices[0].color.w == 0.0f);   // no colour laid over the layers there
    // Without layers, no shares and no terrain material.
    REQUIRE(assets::terrain_layer_weights(t, assets::TerrainLook{}).empty());
    REQUIRE(assets::terrain_mesh(t, assets::TerrainLook{}, "terrain:plain").materials[0].terrain == nullptr);
}

TEST_CASE("glass and lacquer from glTF: KHR_materials_transmission, _ior, _volume and _clearcoat are read", "[assets][gltf][glass]") {
    // Two quads' materials as Blender writes them for Transmission and Coat, beside the sample's assets.
    const std::filesystem::path file = project() / "assets" / "glass-test.gltf";
    struct Cleanup { std::filesystem::path p; ~Cleanup() { std::filesystem::remove(p); } } cleanup{file};
    {
        std::ofstream out(file, std::ios::binary);
        out << R"({"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0,1]}],"nodes":[{"name":"glass","mesh":0},{"name":"paint","mesh":1}],
            "extensionsUsed":["KHR_materials_transmission","KHR_materials_ior","KHR_materials_volume","KHR_materials_clearcoat"],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":0}]},{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3,"material":1}]}],
            "materials":[{"name":"bottle","pbrMetallicRoughness":{"roughnessFactor":0.05,"metallicFactor":0},"extensions":{"KHR_materials_transmission":{"transmissionFactor":1.0},"KHR_materials_ior":{"ior":1.45},"KHR_materials_volume":{"thicknessFactor":0.2,"attenuationDistance":0.5,"attenuationColor":[0.4,0.9,0.5]}}},
                         {"name":"car","pbrMetallicRoughness":{"baseColorFactor":[0.6,0.02,0.02,1],"roughnessFactor":0.5,"metallicFactor":0.3},"extensions":{"KHR_materials_clearcoat":{"clearcoatFactor":1.0,"clearcoatRoughnessFactor":0.04}}}],
            "buffers":[{"byteLength":140,"uri":"data:application/octet-stream;base64,)" << "AAAAvwAAAL8AAAAAAAAAPwAAAL8AAAAAAAAAPwAAAD8AAAAAAAAAvwAAAD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAgD8AAIA/AACAPwAAgD8AAAAAAAAAAAAAAAAAAAEAAgAAAAIAAwA=" << R"("}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":48},{"buffer":0,"byteOffset":48,"byteLength":48},{"buffer":0,"byteOffset":96,"byteLength":32},{"buffer":0,"byteOffset":128,"byteLength":12}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[-0.5,-0.5,0],"max":[0.5,0.5,0]},{"bufferView":1,"componentType":5126,"count":4,"type":"VEC3"},{"bufferView":2,"componentType":5126,"count":4,"type":"VEC2"},{"bufferView":3,"componentType":5123,"count":6,"type":"SCALAR"}]})";
    }
    assets::AssetStore store(project());
    auto m = store.mesh("assets/glass-test.gltf");
    if (!m) INFO(m.error().to_string());
    REQUIRE(m.has_value());
    const assets::Mesh& mesh = **m;
    REQUIRE(mesh.materials.size() == 2);
    const assets::Material& glass = mesh.materials[0];
    REQUIRE(glass.transmission == Catch::Approx(1.0));
    REQUIRE(glass.ior == Catch::Approx(1.45));
    REQUIRE(glass.thickness == Catch::Approx(0.2));
    REQUIRE(glass.attenuation_distance == Catch::Approx(0.5));
    REQUIRE(glass.attenuation_color.y == Catch::Approx(0.9));
    REQUIRE(glass.clearcoat == 0);
    const assets::Material& car = mesh.materials[1];
    REQUIRE(car.transmission == 0);
    REQUIRE(car.ior == Catch::Approx(1.5));   // the default
    REQUIRE(car.clearcoat == Catch::Approx(1.0));
    REQUIRE(car.clearcoat_roughness == Catch::Approx(0.04));
    const Json d = mesh.describe();
    REQUIRE(d["materials"][0]["transmission"] == 1.0);
    REQUIRE(d["materials"][0]["attenuation"]["distance"] == 0.5);
    REQUIRE(d["materials"][1]["clearcoat"] == 1.0);
    REQUIRE_FALSE(d["materials"][1].contains("transmission"));
}

TEST_CASE("levels of detail: a mesh simplified to a share of its triangles, part by part, on the same vertices", "[assets][lod]") {
    // A grid of 32 by 32 quads bent into a dome, in two parts (two materials) side by side.
    assets::Mesh m;
    const int n = 33;
    for (int j = 0; j < n; ++j)
        for (int i = 0; i < n; ++i) {
            assets::MeshVertex v;
            const float x = static_cast<float>(i) / 32.0f - 0.5f, z = static_cast<float>(j) / 32.0f - 0.5f;
            v.position = {x, 0.5f - (x * x + z * z), z};
            v.normal = normalize(Vec3{2 * x, 1, 2 * z});
            v.uv = {x + 0.5f, z + 0.5f};
            m.vertices.push_back(v);
        }
    for (int part = 0; part < 2; ++part) {
        assets::Submesh sm;
        sm.first_index = static_cast<std::uint32_t>(m.indices.size());
        sm.material = static_cast<std::uint32_t>(part);
        for (int j = 0; j < 32; ++j)
            for (int i = part * 16; i < part * 16 + 16; ++i) {
                const auto a = static_cast<std::uint32_t>(j * n + i), b = a + 1, c = a + static_cast<std::uint32_t>(n), d = c + 1;
                m.indices.insert(m.indices.end(), {a, c, b, b, c, d});
            }
        sm.index_count = static_cast<std::uint32_t>(m.indices.size()) - sm.first_index;
        m.submeshes.push_back(sm);
    }
    REQUIRE(m.indices.size() == 32u * 32u * 6u);
    const assets::MeshLod half = assets::simplify(m, 0.5f);
    const assets::MeshLod tenth = assets::simplify(m, 0.1f);
    INFO("half " << half.triangles << " tenth " << tenth.triangles << " errors " << half.error << ", " << tenth.error);
    REQUIRE(half.submeshes.size() == 2u);
    REQUIRE(half.triangles <= 1024u + 8u);
    REQUIRE(half.triangles >= 900u);
    REQUIRE(tenth.triangles < half.triangles);
    REQUIRE(tenth.triangles <= 2048u / 10u + 16u);
    REQUIRE(tenth.error >= half.error);
    // Each part keeps its range, one after the other, and every index is a vertex of the mesh.
    REQUIRE(half.submeshes[0].first_index == 0u);
    REQUIRE(half.submeshes[1].first_index == half.submeshes[0].index_count);
    REQUIRE(half.submeshes[1].material == 1u);
    for (std::uint32_t k : tenth.indices) REQUIRE(k < m.vertices.size());
    // Where the parts meet, the edge stays: every vertex on the line x = 0 that the full mesh uses
    // is used by both parts' simplified triangles alike.
    auto used = [&](const assets::Submesh& sm, const std::vector<std::uint32_t>& idx) {
        std::set<std::uint32_t> s;
        for (std::uint32_t k = 0; k < sm.index_count; ++k) {
            const std::uint32_t v = idx[sm.first_index + k];
            if (v % static_cast<std::uint32_t>(n) == 16u) s.insert(v);
        }
        return s;
    };
    REQUIRE(used(tenth.submeshes[0], tenth.indices) == used(tenth.submeshes[1], tenth.indices));
    // A ratio of 1 keeps them all.
    REQUIRE(assets::simplify(m, 1.0f).indices == m.indices);
}

TEST_CASE("patterns are images the engine draws: wrapping, in their colours, with normal maps", "[assets][pattern]") {
    assets::AssetStore store(project());
    auto at = [](const assets::Image* i, std::uint32_t x, std::uint32_t y) {
        const std::size_t k = (static_cast<std::size_t>(y) * i->width + x) * 4;
        return std::array<int, 4>{i->rgba[k], i->rgba[k + 1], i->rgba[k + 2], i->rgba[k + 3]};
    };
    // A checker of four squares: the first colour top-left, the second beside it.
    auto checker = store.image("pattern:checker?a=#ff0000&b=#0000ff&count=2&size=64");
    REQUIRE(checker.has_value());
    REQUIRE((*checker)->width == 64);
    REQUIRE(at(*checker, 8, 8) == std::array<int, 4>{255, 0, 0, 255});
    REQUIRE(at(*checker, 40, 8) == std::array<int, 4>{0, 0, 255, 255});
    // Every pattern draws, the same twice; the noisy ones wrap: the left column is like the right.
    for (const char* name : {"stripes", "grid", "bricks", "tiles", "planks", "cobble", "shingles", "noise", "concrete", "sand", "dirt", "rock", "grass", "metal"}) {
        INFO(name);
        const std::string spec = std::string("pattern:") + name + "?size=128";
        auto a = assets::pattern_image(spec), b = assets::pattern_image(spec);
        REQUIRE(a.has_value());
        REQUIRE(a->rgba == b->rgba);
        if (std::string_view(name) == "stripes" || std::string_view(name) == "grid" || std::string_view(name) == "bricks" || std::string_view(name) == "tiles" || std::string_view(name) == "planks" || std::string_view(name) == "shingles") continue;   // edges on the seam, by design
        int far = 0;
        for (std::uint32_t y = 0; y < 128; ++y) {
            const auto l = at(&*a, 0, y), r = at(&*a, 127, y);
            for (int k = 0; k < 3; ++k) far = std::max(far, std::abs(l[static_cast<std::size_t>(k)] - r[static_cast<std::size_t>(k)]));
        }
        REQUIRE(far < 90);   // neighbours across the seam, not two unrelated places
    }
    // Bricks in their colour; the mortar between rows; a normal map that leans at the brick's edges.
    auto bricks = assets::pattern_image("pattern:bricks?color=#a0522d&mortar=#ffffff&rows=4&columns=2&vary=0&size=128");
    REQUIRE(bricks.has_value());
    const auto brick = at(&*bricks, 32, 16);
    REQUIRE(brick[0] > brick[2] + 40);
    const auto mortar = at(&*bricks, 32, 0);
    REQUIRE(mortar[2] > 180);
    auto normals = assets::pattern_image("pattern:bricks?rows=4&columns=2&size=128&map=normal&bump=12");
    REQUIRE(normals.has_value());
    REQUIRE(at(&*normals, 32, 16)[2] > 240);                       // flat on a brick's face
    REQUIRE(std::abs(at(&*normals, 32, 2)[1] - 128) > 20);          // tilted at its top edge
    // What is wrong is said.
    REQUIRE_FALSE(assets::pattern_image("pattern:marble").has_value());
    REQUIRE(assets::pattern_image("pattern:bricks?color=zzz").error().message.find("not a colour") != std::string::npos);
    REQUIRE_FALSE(assets::pattern_image("pattern:tiles?map=rough").has_value());
}

TEST_CASE("props are meshes the engine makes: standing on their origin, in their colours, seeded", "[assets][prop]") {
    assets::AssetStore store(project());
    for (const char* name : {"tree", "pine", "rock", "bush", "barrel", "lamp", "fence", "house", "crate", "chest", "torch", "bench", "table", "chair", "well", "sign", "tower", "crop", "boat", "boat?furled=1", "grass?height=0.6"}) {
        INFO(name);
        REQUIRE(assets::is_prop(name));
        auto m = store.mesh(name);
        REQUIRE(m.has_value());
        REQUIRE((*m)->importer == "builtin");
        REQUIRE((*m)->aabb_min.y > -0.07f);   // on the ground (a rock a little into it)
        REQUIRE((*m)->aabb_max.y > 0.5f);
        for (const auto& v : (*m)->vertices) REQUIRE(std::isfinite(v.normal.x + v.normal.y + v.normal.z));
    }
    REQUIRE_FALSE(assets::is_prop("treehouse"));
    REQUIRE_FALSE(assets::is_prop("cube"));
    // A tall tree is tall; another seed is another tree; the same seed the same.
    auto tall = store.mesh("tree?height=8");
    REQUIRE(tall.has_value());
    REQUIRE((*tall)->aabb_max.y > 7.0f);
    REQUIRE(*assets::prop_glb("tree?seed=2") != *assets::prop_glb("tree?seed=3"));
    REQUIRE(*assets::prop_glb("tree?seed=2") == *assets::prop_glb("tree?seed=2"));
    // Its leaves in the colour asked (linear: #ff0000 is 1, 0, 0).
    auto red = store.mesh("tree?leaves=#ff0000");
    REQUIRE(red.has_value());
    bool found = false;
    for (const auto& mat : (*red)->materials) found = found || (mat.base_color.x > 0.99f && mat.base_color.y < 0.01f);
    REQUIRE(found);
    // What is wrong is said.
    REQUIRE(assets::prop_glb("rock?leaves=red").error().message.find("rock takes size, color, seed") != std::string::npos);
    REQUIRE(assets::prop_glb("tree?height=tall").error().message.find("not a number") != std::string::npos);
    REQUIRE_FALSE(store.mesh("bush?color=blurple").has_value());
}

TEST_CASE("an SVG is read as an image drawn at twice its size, or at the size asked", "[assets][svg]") {
    assets::AssetStore store(project());
    auto img = store.image("assets/star.svg");
    REQUIRE(img.has_value());
    REQUIRE((*img)->width == 96);
    REQUIRE((*img)->height == 96);
    auto at = [&](const assets::Image* i, std::uint32_t x, std::uint32_t y) {
        const std::size_t k = (static_cast<std::size_t>(y) * i->width + x) * 4;
        return std::array<int, 4>{i->rgba[k], i->rgba[k + 1], i->rgba[k + 2], i->rgba[k + 3]};
    };
    // The star's gold at the middle, the disc's blue between the star's points, nothing in a corner.
    const auto mid = at(*img, 48, 50);
    INFO(mid[0] << " " << mid[1] << " " << mid[2] << " " << mid[3]);
    REQUIRE(mid[0] > 230);
    REQUIRE(mid[1] > 180);
    REQUIRE(mid[2] < 90);
    REQUIRE(mid[3] == 255);
    const auto disc = at(*img, 48, 84);
    REQUIRE(disc[2] > 150);
    REQUIRE(disc[0] < 80);
    REQUIRE(at(*img, 1, 1)[3] == 0);
    // Asked for a size, the longer side is that many pixels; a scale, that factor.
    auto big = store.image("assets/star.svg?size=200");
    REQUIRE(big.has_value());
    REQUIRE((*big)->width == 200);
    REQUIRE(store.image("assets/star.svg?scale=0.5").value()->width == 24);
    // Read again, every size goes.
    store.invalidate("assets/star.svg");
    REQUIRE(store.image("assets/star.svg?size=200").value() != nullptr);
    Json listed = store.list();
    bool found = false;
    for (const Json& a : listed.is_array() ? listed : listed["files"]) if (a.value("path", std::string()) == "assets/star.svg") { found = true; REQUIRE(a["kind"] == "image"); }
    REQUIRE(found);
}

TEST_CASE("an image says what it looks like, in numbers, colour names and characters", "[assets][look]") {
    assets::AssetStore store(project());
    const Json d = store.describe("assets/star.svg", 24);
    INFO(d.dump());
    REQUIRE(d["kind"] == "image");
    REQUIRE(d["coverage"].get<double>() == Catch::Approx(0.75).margin(0.05));   // a disc in its square
    REQUIRE(d["mirror_symmetry"].get<double>() > 0.95);
    std::vector<std::string> names;
    for (const Json& c : d["colours"]) names.push_back(c["name"].get<std::string>());
    REQUIRE(names.front() == "blue");
    REQUIRE(std::find(names.begin(), names.end(), "gold") != names.end());
    REQUIRE(d["ascii"].size() == 12);   // half as many rows as columns for a square
    REQUIRE(d["ascii"][0].get<std::string>().size() == 24);
    // The middle is the star's gold; a corner is empty.
    REQUIRE(d["ascii_colours"][6].get<std::string>()[12] == 'G');
    REQUIRE(d["ascii"][0].get<std::string>()[0] == ' ');
    // Without ascii, the numbers alone.
    REQUIRE_FALSE(store.describe("assets/star.svg").contains("ascii"));
}

TEST_CASE("voxel models: text layers and MagicaVoxel files meshed greedily, colours in the vertices", "[assets][voxels]") {
    const std::filesystem::path dir = project() / "assets" / "voxel-test";
    std::filesystem::create_directories(dir);
    // Two red cells side by side and one green on top of the right one; a glowing cell apart.
    std::ofstream(dir / "pair.voxels", std::ios::binary) << R"({
  // comments are allowed
  "voxel": 0.5,
  "palette": {"r": "#ff0000", "g": [0, 1, 0], "l": {"color": "#ffd27a", "emissive": 4}},
  "layers": [
    ["rr.l"],
    [".g"]
  ]
})";
    assets::AssetStore store(project());
    auto m = store.mesh("assets/voxel-test/pair.voxels");
    REQUIRE(m.has_value());
    const assets::Mesh& v = **m;
    REQUIRE(v.importer == "voxels");
    // Red: the pair's shared face hidden and its faces merged (bottom, front, back, left one quad
    // each; the top only over the left cell, the right end one) = 6 quads; green: 5 (its bottom
    // sits on red); the lamp: 6.
    REQUIRE(v.indices.size() == static_cast<std::size_t>(6 * (6 + 5 + 6)));
    REQUIRE(v.aabb_min.x == Catch::Approx(-1.0f));   // four cells of 0.5 across, centred
    REQUIRE(v.aabb_max.x == Catch::Approx(1.0f));
    REQUIRE(v.aabb_min.y == Catch::Approx(0.0f));
    REQUIRE(v.aabb_max.y == Catch::Approx(1.0f));
    bool red = false, green = false;
    for (const assets::MeshVertex& x : v.vertices) {
        red = red || (x.color.x > 0.99f && x.color.y < 0.01f);
        green = green || (x.color.y > 0.99f && x.color.x < 0.01f);
    }
    REQUIRE((red && green));
    // The lamp glows: a material of its own with an emissive colour; the rest share a plain one.
    REQUIRE(v.materials.size() == 2);
    REQUIRE(v.materials[1].emissive.x > 3.0f);
    REQUIRE(v.submeshes.size() == 2);
    REQUIRE_FALSE(store.mesh("assets/voxel-test/missing.voxels").has_value());
    std::ofstream(dir / "bad.voxels", std::ios::binary) << R"({"palette": {"r": "#f00"}, "layers": [["rx"]]})";
    auto bad = store.mesh("assets/voxel-test/bad.voxels");
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().message.find("'x'") != std::string::npos);

    // MagicaVoxel: z is up; two cells along its x, palette entries 1 (red) and 2 (green).
    {
        std::string body;
        auto u32 = [&](std::string& s, std::uint32_t x) { s.append(reinterpret_cast<const char*>(&x), 4); };
        auto chunk = [&](const char* id, const std::string& content) {
            std::string c(id, 4);
            u32(c, static_cast<std::uint32_t>(content.size()));
            u32(c, 0);
            return c + content;
        };
        std::string size, xyzi, rgba;
        u32(size, 2); u32(size, 1); u32(size, 3);
        u32(xyzi, 3);
        for (std::array<std::uint8_t, 4> c : {std::array<std::uint8_t, 4>{0, 0, 0, 1}, {1, 0, 0, 2}, {1, 0, 2, 1}}) xyzi.append(reinterpret_cast<const char*>(c.data()), 4);
        for (int i = 0; i < 256; ++i) u32(rgba, i == 0 ? 0xFF0000FFu : i == 1 ? 0xFF00FF00u : 0xFF808080u);
        body = chunk("SIZE", size) + chunk("XYZI", xyzi) + chunk("RGBA", rgba);
        std::string file = "VOX ";
        u32(file, 150);
        file += "MAIN";
        u32(file, 0);
        u32(file, static_cast<std::uint32_t>(body.size()));
        file += body;
        std::ofstream(dir / "pair.vox", std::ios::binary) << file;
    }
    auto x = store.mesh("assets/voxel-test/pair.vox");
    REQUIRE(x.has_value());
    REQUIRE((*x)->importer == "vox");
    // The file's z up is the engine's y: the third cell floats two above the second. The red and
    // green side by side hide the face between them (5 quads each), the floating one shows 6.
    REQUIRE((*x)->aabb_max.y == Catch::Approx(0.3f));
    REQUIRE((*x)->indices.size() == static_cast<std::size_t>(6 * (5 + 5 + 6)));
    std::filesystem::remove_all(dir);
}

TEST_CASE("glTF compressed with EXT_meshopt_compression reads as the plain file would", "[assets][meshopt]") {
    // A quad, its vertices (position and normal, 24 bytes apart) and its indices compressed by
    // meshoptimizer's own encoders, the way gltfpack writes them, into a data URI.
    const float verts[4][6] = {{0, 0, 0, 0, 0, 1}, {2, 0, 0, 0, 0, 1}, {2, 1, 0, 0, 0, 1}, {0, 1, 0, 0, 0, 1}};
    const unsigned int idx[6] = {0, 1, 2, 0, 2, 3};
    std::vector<unsigned char> v(meshopt_encodeVertexBufferBound(4, 24)), i(meshopt_encodeIndexBufferBound(6, 4));
    v.resize(meshopt_encodeVertexBuffer(v.data(), v.size(), verts, 4, 24));
    i.resize(meshopt_encodeIndexBuffer(i.data(), i.size(), idx, 6));
    std::string raw(v.begin(), v.end());
    raw.append(i.begin(), i.end());
    static const char* kAlphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string b64;
    for (std::size_t k = 0; k < raw.size(); k += 3) {
        const std::uint32_t b0 = static_cast<unsigned char>(raw[k]);
        const std::uint32_t b1 = k + 1 < raw.size() ? static_cast<unsigned char>(raw[k + 1]) : 0u;
        const std::uint32_t b2 = k + 2 < raw.size() ? static_cast<unsigned char>(raw[k + 2]) : 0u;
        const std::uint32_t n = (b0 << 16) | (b1 << 8) | b2;
        b64 += kAlphabet[(n >> 18) & 63];
        b64 += kAlphabet[(n >> 12) & 63];
        b64 += k + 1 < raw.size() ? kAlphabet[(n >> 6) & 63] : '=';
        b64 += k + 2 < raw.size() ? kAlphabet[n & 63] : '=';
    }
    const Json ext_v{{"buffer", 0}, {"byteOffset", 0}, {"byteLength", v.size()}, {"byteStride", 24}, {"count", 4}, {"mode", "ATTRIBUTES"}};
    const Json ext_i{{"buffer", 0}, {"byteOffset", v.size()}, {"byteLength", i.size()}, {"byteStride", 2}, {"count", 6}, {"mode", "TRIANGLES"}};
    const Json doc{
        {"asset", {{"version", "2.0"}}},
        {"extensionsUsed", {"EXT_meshopt_compression"}},
        {"extensionsRequired", {"EXT_meshopt_compression"}},
        {"buffers", {Json{{"byteLength", raw.size()}, {"uri", "data:application/octet-stream;base64," + b64}}, Json{{"byteLength", 108}, {"extensions", {{"EXT_meshopt_compression", {{"fallback", true}}}}}}}},
        {"bufferViews", {Json{{"buffer", 1}, {"byteOffset", 0}, {"byteLength", 96}, {"byteStride", 24}, {"extensions", {{"EXT_meshopt_compression", ext_v}}}},
                         Json{{"buffer", 1}, {"byteOffset", 96}, {"byteLength", 12}, {"extensions", {{"EXT_meshopt_compression", ext_i}}}}}},
        {"accessors", {Json{{"bufferView", 0}, {"componentType", 5126}, {"count", 4}, {"type", "VEC3"}, {"min", {0, 0, 0}}, {"max", {2, 1, 0}}},
                       Json{{"bufferView", 0}, {"byteOffset", 12}, {"componentType", 5126}, {"count", 4}, {"type", "VEC3"}},
                       Json{{"bufferView", 1}, {"componentType", 5123}, {"count", 6}, {"type", "SCALAR"}}}},
        {"meshes", {Json{{"primitives", {Json{{"attributes", {{"POSITION", 0}, {"NORMAL", 1}}}, {"indices", 2}}}}}}},
        {"nodes", {Json{{"mesh", 0}}}},
        {"scenes", {Json{{"nodes", {0}}}}},
        {"scene", 0}};
    const std::filesystem::path dir = project() / "assets" / "meshopt-test";
    std::filesystem::create_directories(dir);
    std::ofstream(dir / "quad.gltf", std::ios::binary) << doc.dump();
    assets::AssetStore store(project());
    auto m = store.mesh("assets/meshopt-test/quad.gltf");
    INFO((m ? std::string() : m.error().to_string()));
    REQUIRE(m.has_value());
    REQUIRE((*m)->vertices.size() == 4);
    REQUIRE((*m)->indices.size() == 6);
    REQUIRE((*m)->aabb_max.x == Catch::Approx(2.0f));
    REQUIRE((*m)->aabb_max.y == Catch::Approx(1.0f));
    REQUIRE((*m)->vertices[0].normal.z == Catch::Approx(1.0f));
    REQUIRE((*m)->indices[5] == 3);
    std::filesystem::remove_all(dir);
}

TEST_CASE("models gltf-transform compressed (meshopt, quantized) read as their originals", "[assets][meshopt]") {
    // samples/assets/assets/meshopt/: arm, crate and fan from the sample run through
    // `gltf-transform meshopt` (4.x): EXT_meshopt_compression and KHR_mesh_quantization, the arm's
    // skin given the dequantization in its inverse bind matrices.
    assets::AssetStore store(project());
    for (const char* name : {"arm", "crate", "fan"}) {
        INFO(name);
        auto a = store.mesh(std::string("assets/") + name + ".glb");
        auto b = store.mesh(std::string("assets/meshopt/") + name + ".glb");
        REQUIRE(a.has_value());
        REQUIRE(b.has_value());
        REQUIRE((*b)->vertices.size() == (*a)->vertices.size());
        REQUIRE((*b)->indices.size() == (*a)->indices.size());
        for (int k = 0; k < 3; ++k) {
            REQUIRE((&(*b)->aabb_min.x)[k] == Catch::Approx((&(*a)->aabb_min.x)[k]).margin(1e-3));
            REQUIRE((&(*b)->aabb_max.x)[k] == Catch::Approx((&(*a)->aabb_max.x)[k]).margin(1e-3));
        }
    }
}

TEST_CASE("WebP images, alone and inside glTF (EXT_texture_webp)", "[assets][webp]") {
    assets::AssetStore store(project());
    // samples/assets/assets/webp/halves.webp (lossless, made with Pillow): 8 by 4, its left half
    // opaque red, its right half half-transparent blue.
    auto img = store.image("assets/webp/halves.webp");
    REQUIRE(img.has_value());
    REQUIRE((*img)->width == 8);
    REQUIRE((*img)->height == 4);
    const auto& px = (*img)->rgba;
    REQUIRE((px[0] == 255 && px[1] == 0 && px[2] == 0 && px[3] == 255));
    const std::size_t right = (1 * 8 + 6) * 4;
    REQUIRE((px[right] == 0 && px[right + 2] == 255 && px[right + 3] == 128));
    // The sample's plate through `gltf-transform webp`: its three textures only WebP now, embedded,
    // read as the PNG originals within a lossy encoder's error.
    auto a = store.mesh("assets/plate.glb");
    auto b = store.mesh("assets/webp/plate.glb");
    REQUIRE(a.has_value());
    REQUIRE(b.has_value());
    REQUIRE((*b)->materials.size() == (*a)->materials.size());
    for (std::size_t m = 0; m < (*a)->materials.size(); ++m) {
        for (auto field : {&assets::Material::texture, &assets::Material::normal_texture, &assets::Material::metallic_roughness_texture}) {
            const std::string& ta = (*a)->materials[m].*field;
            const std::string& tb = (*b)->materials[m].*field;
            REQUIRE(ta.empty() == tb.empty());
            if (ta.empty()) continue;
            auto ia = store.image(ta);
            auto ib = store.image(tb);
            INFO(ta << " against " << tb);
            REQUIRE(ia.has_value());
            REQUIRE(ib.has_value());
            REQUIRE((*ib)->width == (*ia)->width);
            REQUIRE((*ib)->height == (*ia)->height);
            double diff = 0;
            for (std::size_t k = 0; k < (*ia)->rgba.size(); ++k) diff += std::abs(static_cast<int>((*ia)->rgba[k]) - static_cast<int>((*ib)->rgba[k]));
            REQUIRE(diff / static_cast<double>((*ia)->rgba.size()) < 8.0);
        }
    }
}

TEST_CASE("models compressed with Draco (KHR_draco_mesh_compression) read as their originals", "[assets][draco]") {
    // samples/assets/assets/draco/: the sample's arm, crate, fan and plate through
    // `gltf-transform draco` (4.x, Draco 1.5). Draco keeps its own order of points and quantizes the
    // positions (14 bits), so the triangles and the bounds are what must agree.
    assets::AssetStore store(project());
    for (const char* name : {"arm", "crate", "fan", "plate"}) {
        INFO(name);
        auto a = store.mesh(std::string("assets/") + name + ".glb");
        auto b = store.mesh(std::string("assets/draco/") + name + ".glb");
        REQUIRE(a.has_value());
        INFO((b ? std::string() : b.error().to_string()));
        REQUIRE(b.has_value());
        REQUIRE((*b)->indices.size() == (*a)->indices.size());
        REQUIRE((*b)->materials.size() == (*a)->materials.size());
        for (int k = 0; k < 3; ++k) {
            REQUIRE((&(*b)->aabb_min.x)[k] == Catch::Approx((&(*a)->aabb_min.x)[k]).margin(2e-3));
            REQUIRE((&(*b)->aabb_max.x)[k] == Catch::Approx((&(*a)->aabb_max.x)[k]).margin(2e-3));
        }
        // Every triangle's corners face the way the file's normals say: the decoded normals are unit.
        for (const assets::MeshVertex& v : (*b)->vertices) REQUIRE(length(v.normal) == Catch::Approx(1.0f).margin(1e-2));
    }
}

TEST_CASE("KTX2 textures (Basis Universal, ETC1S and UASTC), alone and through KHR_texture_basisu", "[assets][ktx2]") {
    // samples/assets/assets/ktx2/: a 32 by 16 gradient as PNG and as KTX2 encoded by the basisu tool
    // (2.50) in both of Basis's modes; each read back close to the PNG (ETC1S is the lossier).
    assets::AssetStore store(project());
    auto png = store.image("assets/ktx2/gradient.png");
    REQUIRE(png.has_value());
    for (auto [name, tolerance] : {std::pair{"gradient-etc1s", 12.0}, {"gradient-uastc", 2.0}}) {
        INFO(name);
        auto k = store.image(std::string("assets/ktx2/") + name + ".ktx2");
        INFO((k ? std::string() : k.error().to_string()));
        REQUIRE(k.has_value());
        REQUIRE((*k)->width == 32);
        REQUIRE((*k)->height == 16);
        double diff = 0;
        for (std::size_t i = 0; i < (*png)->rgba.size(); ++i) diff += std::abs(static_cast<int>((*png)->rgba[i]) - static_cast<int>((*k)->rgba[i]));
        diff /= static_cast<double>((*png)->rgba.size());
        INFO(name << " differs from the PNG by " << diff << " a channel on average");
        REQUIRE(diff < tolerance);
    }
    // A glTF quad whose base colour names the KTX2 file through KHR_texture_basisu only.
    const Json doc{
        {"asset", {{"version", "2.0"}}},
        {"extensionsUsed", {"KHR_texture_basisu"}},
        {"extensionsRequired", {"KHR_texture_basisu"}},
        {"buffers", {Json{{"byteLength", 48}, {"uri", "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAACAPwAAgD8AAAAAAAAAAAAAgD8AAAAA"}}}},
        {"bufferViews", {Json{{"buffer", 0}, {"byteLength", 48}}}},
        {"accessors", {Json{{"bufferView", 0}, {"componentType", 5126}, {"count", 4}, {"type", "VEC3"}, {"min", {0, 0, 0}}, {"max", {1, 1, 0}}}}},
        {"images", {Json{{"uri", "../ktx2/gradient-uastc.ktx2"}, {"mimeType", "image/ktx2"}}}},
        {"textures", {Json{{"extensions", {{"KHR_texture_basisu", {{"source", 0}}}}}}}},
        {"materials", {Json{{"pbrMetallicRoughness", {{"baseColorTexture", {{"index", 0}}}}}}}},
        {"meshes", {Json{{"primitives", {Json{{"attributes", {{"POSITION", 0}}}, {"material", 0}}}}}}},
        {"nodes", {Json{{"mesh", 0}}}},
        {"scenes", {Json{{"nodes", {0}}}}},
        {"scene", 0}};
    const std::filesystem::path dir = project() / "assets" / "ktx2-test";
    std::filesystem::create_directories(dir);
    std::ofstream(dir / "quad.gltf", std::ios::binary) << doc.dump();
    auto m = store.mesh("assets/ktx2-test/quad.gltf");
    REQUIRE(m.has_value());
    REQUIRE((*m)->materials[0].texture == "assets/ktx2/gradient-uastc.ktx2");
    REQUIRE(store.image((*m)->materials[0].texture).has_value());
    std::filesystem::remove_all(dir);
}
