#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>
#include <pocket/assets/assets.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <filesystem>
#include <array>
#include <set>

using namespace pocket;

namespace {

std::filesystem::path root() {
    const char* r = std::getenv("POCKET_ROOT");
    REQUIRE(r != nullptr);
    return r;
}

app::Options playground_options() {
    app::Options o;
    o.project_dir = root() / "samples" / "playground";
    o.bundle = root() / "build" / "ts" / "playground.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = 60;
    o.width = 256;
    o.height = 144;
    o.log_level = "warn";
    return o;
}

}  // namespace

TEST_CASE("playground renders meshes and an id buffer", "[renderer]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 60; ++i) REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["has_camera"] == true);
    REQUIRE(stats["has_sun"] == true);
    REQUIRE(stats["point_lights"].get<int>() >= 1);
    // Four visible meshes in two instanced draws (cubes share one).
    REQUIRE(stats["instances"].get<int>() >= 3);
    REQUIRE(stats["draw_calls"].get<int>() >= 1);
    REQUIRE(stats["draw_calls"].get<int>() < stats["instances"].get<int>());

    Json ids = s.command("render.ids", Json{{"path", (root() / "build" / "test-out" / "playground-ids.png").string()}}).value();
    INFO(ids.dump());
    std::set<std::string> paths;
    for (auto& e : ids["visible"]) {
        if (e.contains("path")) paths.insert(e["path"].get<std::string>());
    }
    REQUIRE(paths.count("/Level/Ground") == 1);
    REQUIRE(paths.count("/Level/Player") == 1);

    // Projecting the player's position lands on a pixel whose id is the player.
    Json proj = s.command("render.project", Json{{"entity", "/Level/Player"}}).value();
    INFO(proj.dump());
    REQUIRE(proj["visible"] == true);
    REQUIRE(proj["inside"] == true);
    Json pick = s.command("render.pick", Json{{"x", proj["x"].get<double>()}, {"y", proj["y"].get<double>()}}).value();
    INFO(pick.dump());
    REQUIRE(pick["path"] == "/Level/Player");

    // The capture is not a flat clear color anymore.
    Json cap = s.command("capture", Json{{"path", (root() / "build" / "test-out" / "playground-30.png").string()}}).value();
    auto px = cap["center_pixel"];
    Json clear = s.report()["clear_color"];
    bool differs = false;
    for (int i = 0; i < 3; ++i) {
        int c = static_cast<int>(std::lround(clear[i].get<double>() * 255.0));
        if (std::abs(px[i].get<int>() - c) > 2) differs = true;
    }
    REQUIRE(differs);
    REQUIRE(s.finish().has_value());
    REQUIRE(s.report()["ok"] == true);
}

TEST_CASE("bounds follow transforms", "[renderer]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    REQUIRE(s.frame().has_value());
    Json ground = s.command("world.get", Json{{"entity", "/Level/Ground"}, {"component", "Bounds"}}).value();
    INFO(ground.dump());
    REQUIRE(ground["min"]["x"].get<double>() == Catch::Approx(-10.0));
    REQUIRE(ground["max"]["x"].get<double>() == Catch::Approx(10.0));
    REQUIRE(ground["max"]["y"].get<double>() == Catch::Approx(0.0));
    REQUIRE(s.finish().has_value());
}

namespace {
app::Options assets_options() {
    app::Options o;
    o.project_dir = root() / "samples" / "assets";
    o.bundle = root() / "build" / "ts" / "assets.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.width = 320;
    o.height = 180;
    o.log_level = "error";
    return o;
}
}  // namespace

TEST_CASE("glTF meshes and textures render; missing assets are marked", "[renderer][assets]") {
    app::Session s(assets_options());
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["meshes"] == 4);                 // ground, crate, pyramid, missing
    REQUIRE(stats["draw_calls"].get<int>() == 5);  // crate has two submeshes (two nodes share one material -> still two draws)
    REQUIRE(stats["assets"]["meshes"] == 2);
    REQUIRE(stats["assets"]["textures"] == 1);
    REQUIRE(stats["assets"]["missing"].size() == 1);
    REQUIRE(stats["assets"]["missing"][0] == "assets/does-not-exist.glb");
    // Asset bounds reach the world: the crate's Bounds span both baked nodes.
    Json bounds = s.command("world.get", Json{{"entity", "Crate"}, {"component", "Bounds"}}).value();
    INFO(bounds.dump());
    REQUIRE(bounds["max"]["x"].get<double>() > 0.0);
    REQUIRE(bounds["max"]["x"].get<double>() - bounds["min"]["x"].get<double>() > 2.0);
    // The textured ground is not flat: two pixels on the checkerboard differ noticeably.
    Json cap = s.command("capture", Json{{"path", (root() / "build" / "test-out" / "assets-3.png").string()}}).value();
    auto img = s.device().capture();
    REQUIRE(img.has_value());
    auto at = [&](std::uint32_t x, std::uint32_t y) { std::size_t i = (static_cast<std::size_t>(y) * img->width + x) * 4; return std::array<int, 3>{img->rgba[i], img->rgba[i + 1], img->rgba[i + 2]}; };
    int min_l = 255, max_l = 0;
    for (std::uint32_t x = 20; x < 300; x += 4) {
        auto p = at(x, 160);
        int l = (p[0] + p[1] + p[2]) / 3;
        min_l = std::min(min_l, l);
        max_l = std::max(max_l, l);
    }
    INFO("ground luminance range " << min_l << ".." << max_l);
    REQUIRE(max_l - min_l > 60);
    // assets.* commands answer.
    Json list = s.command("assets.list", Json::object()).value();
    REQUIRE(list.size() >= 3);
    Json d = s.command("assets.describe", Json{{"path", "assets/crate.glb"}}).value();
    REQUIRE(d["nodes"] == 2);
    Json reloaded = s.command("assets.reload", Json::object()).value();
    REQUIRE(reloaded["ok"] == true);
    REQUIRE(s.frame().has_value());
    stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["assets"]["meshes"] == 2);  // re-uploaded after the reload
    REQUIRE(s.finish().has_value());
}

namespace {
// Brightness of the pixel a world point projects to, from a fresh capture.
double brightness_at(app::Session& s, const Json& capture, Vec3 point) {
    Json pr = s.command("render.project", Json{{"point", Json{{"x", point.x}, {"y", point.y}, {"z", point.z}}}}).value();
    REQUIRE(pr["visible"] == true);
    int x = static_cast<int>(pr["x"].get<double>()), y = static_cast<int>(pr["y"].get<double>());
    auto bytes = fs::read_text(capture["path"].get<std::string>());
    REQUIRE(bytes.has_value());
    auto img = assets::decode_image(*bytes, "capture");
    REQUIRE(img.has_value());
    REQUIRE(x >= 0);
    REQUIRE(y >= 0);
    REQUIRE(static_cast<std::uint32_t>(x) < img->width);
    REQUIRE(static_cast<std::uint32_t>(y) < img->height);
    std::size_t at = (static_cast<std::size_t>(y) * img->width + static_cast<std::size_t>(x)) * 4;
    return img->rgba[at] + img->rgba[at + 1] + img->rgba[at + 2];
}
}  // namespace

TEST_CASE("the sun casts shadows onto the ground", "[renderer][shadows]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A wide ground, a cube floating above it, a sun shining toward +X and down at 45 degrees
    // (its -Z rotated by the quaternion (-0.5, -0.5, 0, 0.7071)), a camera straight above.
    REQUIRE(s.command("world.spawn", Json{{"name", "Ground"}, {"components", Json{{"Transform", Json{{"scale", Json{{"x", 12}, {"y", 1}, {"z", 12}}}}}, {"MeshRenderer", Json{{"mesh", "plane"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Cube"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.5}, {"y", -0.5}, {"z", 0}, {"w", 0.70710678}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1.2}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 10}, {"z", 0.001}}}, {"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["shadows"] == true);
    REQUIRE(stats["shadow_draws"].get<int>() >= 1);
    std::string path = (root() / "build" / "test-out" / "shadows.png").string();
    Json cap = s.command("capture", Json{{"path", path}}).value();
    // The cube's shadow lands at +X on the ground; open ground at -X is lit.
    double shaded = brightness_at(s, cap, {1.3f, 0, 0});
    double lit = brightness_at(s, cap, {-3.0f, 0, 0});
    INFO("shaded " << shaded << " lit " << lit);
    REQUIRE(lit > 200);
    REQUIRE(shaded < lit * 0.7);
    // Turned off, the same two points match.
    REQUIRE(s.command("render.shadows", Json{{"enabled", false}}).value()["enabled"] == false);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["shadows"] == false);
    std::string path2 = (root() / "build" / "test-out" / "shadows-off.png").string();
    Json cap2 = s.command("capture", Json{{"path", path2}}).value();
    double shaded2 = brightness_at(s, cap2, {1.3f, 0, 0});
    double lit2 = brightness_at(s, cap2, {-3.0f, 0, 0});
    INFO("off: shaded " << shaded2 << " lit " << lit2);
    REQUIRE(std::abs(shaded2 - lit2) < lit2 * 0.1);
    REQUIRE(s.finish().has_value());
}
