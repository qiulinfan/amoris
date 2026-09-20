#include <pocket/assets/assets.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <filesystem>
#include <fstream>

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
    REQUIRE(map.object_layers.size() == 1);
    REQUIRE(map.object_layers[0].objects.size() == 7);
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
        std::ofstream out(file);
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
