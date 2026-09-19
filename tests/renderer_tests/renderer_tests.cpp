#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>
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
