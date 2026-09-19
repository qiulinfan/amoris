#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <filesystem>
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
    REQUIRE(stats["draw_calls"].get<int>() >= 3);

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
