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
