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
    REQUIRE(stats["meshes"] == 7);                 // ground, crate, pyramid, the skinned arm, the missing one, the plate, the orb
    REQUIRE(stats["draw_calls"].get<int>() == 8);  // crate has two submeshes (two nodes share one material -> still two draws); the arm is one skinned draw
    REQUIRE(stats["skinned"] == 1);
    REQUIRE(stats["assets"]["meshes"] == 4);
    REQUIRE(stats["assets"]["textures"] == 4);     // checker and the plate's three maps
    REQUIRE(stats["materials"].get<int>() >= 3);       // checker, the plate's maps, the default white set
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
    REQUIRE(stats["assets"]["meshes"] == 4);  // re-uploaded after the reload
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

TEST_CASE("debug lines draw over the scene and overlays follow colliders", "[renderer][debug]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Ground"}, {"components", Json{{"Transform", Json{{"scale", Json{{"x", 12}, {"y", 1}, {"z", 12}}}}}, {"MeshRenderer", Json{{"mesh", "plane"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Crate"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 2}, {"y", 0.5}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}, {"RigidBody", Json{{"kind", 1}}}, {"Collider", Json::object()}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.5}, {"y", -0.5}, {"z", 0}, {"w", 0.70710678}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1.2}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 10}, {"z", 0.001}}}, {"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() == 0);
    // A magenta line above the ground for three ticks: the pixel at its middle takes its color.
    Json added = s.command("debug.line", Json{{"a", Json{{"x", -2}, {"y", 0.5}, {"z", -2}}}, {"b", Json{{"x", 2}, {"y", 0.5}, {"z", -2}}}, {"color", Json::array({1, 0, 1, 1})}, {"ticks", 3}}).value();
    REQUIRE(added["shapes"].get<int>() == 1);
    REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["debug_lines"].get<int>() == 1);
    std::string path = (root() / "build" / "test-out" / "debug-line.png").string();
    Json cap = s.command("capture", Json{{"path", path}}).value();
    Json pr = s.command("render.project", Json{{"point", Json{{"x", 0}, {"y", 0.5}, {"z", -2}}}}).value();
    REQUIRE(pr["visible"] == true);
    auto bytes = fs::read_text(path);
    REQUIRE(bytes.has_value());
    auto img = assets::decode_image(*bytes, "capture");
    REQUIRE(img.has_value());
    int x = static_cast<int>(pr["x"].get<double>()), y = static_cast<int>(pr["y"].get<double>());
    bool magenta = false;
    for (int dy = -1; dy <= 1 && !magenta; ++dy) {
        for (int dx = -1; dx <= 1 && !magenta; ++dx) {
            std::size_t at = (static_cast<std::size_t>(y + dy) * img->width + static_cast<std::size_t>(x + dx)) * 4;
            if (img->rgba[at] > 200 && img->rgba[at + 1] < 60 && img->rgba[at + 2] > 200) magenta = true;
        }
    }
    REQUIRE(magenta);
    // The line leaves the id buffer alone: the ground is still picked through it.
    Json pick = s.command("render.pick", Json{{"x", x}, {"y", y}}).value();
    REQUIRE(pick["name"] == "Ground");
    // Collider overlays: the crate's box adds twelve edges; the line expires after its ticks.
    REQUIRE(s.command("render.debug", Json{{"colliders", true}}).value()["colliders"] == true);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() == 13);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() == 12);
    Json ds = s.command("debug.stats", Json::object()).value();
    REQUIRE(ds["shapes"].get<int>() == 0);
    REQUIRE(ds["colliders"] == true);
    // Axes add three lines; all off draws nothing again.
    REQUIRE(s.command("render.debug", Json{{"axes", true}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() == 15);
    REQUIRE(s.command("render.debug", Json{{"all", false}}).value()["axes"] == false);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() == 0);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("material maps bend normals, make metals and glow", "[renderer][pbr]") {
    // The assets project keeps its scene (its script writes to the crate every tick and a cleared
    // world would make it error, which stops rendering); the test builds its own corner at x=43.
    app::Session s(assets_options());
    REQUIRE(s.start().has_value());
    // A sun shining toward +Z and 30 degrees down, a camera straight above the corner.
    Quat sun = Quat::from_axis_angle({0, 1, 0}, kPi) * Quat::from_axis_angle({1, 0, 0}, -radians(30.0f));
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"x", sun.x}, {"y", sun.y}, {"z", sun.z}, {"w", sun.w}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Light"}, {"value", Json{{"intensity", 1.0}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 43}, {"y", 10}, {"z", 0.001}}}, {"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}}}}).has_value());
    REQUIRE(s.command("render.shadows", Json{{"enabled", false}}).has_value());
    auto plane = [&](const char* name, float x, Json extra) {
        Json mr = Json{{"mesh", "plane"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}};
        for (auto& [k, v] : extra.items()) mr[k] = v;
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", 0}, {"z", 0}}}, {"scale", Json{{"x", 2}, {"y", 1}, {"z", 2}}}}}, {"MeshRenderer", mr}}}}).has_value());
    };
    // Three planes: no map, a flat map, a map bent toward +Y (up in the image = toward -Z in the
    // world for a plane whose v axis runs along +Z). The bent one faces away from a light that
    // comes from -Z, so it is darker; the flat map changes nothing.
    plane("Bare", 40.0f, Json::object());
    plane("Flat", 43.0f, Json{{"normal_map", "assets/normal_flat.png"}});
    plane("Bent", 46.0f, Json{{"normal_map", "assets/normal_up.png"}});
    for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
    Json st = s.command("state", Json::object()).value();
    INFO(st.dump());
    std::string path = (root() / "build" / "test-out" / "pbr-normals.png").string();
    Json cap = s.command("capture", Json{{"path", path}}).value();
    double bare = brightness_at(s, cap, {40.0f, 0, 0});
    double flat = brightness_at(s, cap, {43.0f, 0, 0});
    double bent = brightness_at(s, cap, {46.0f, 0, 0});
    INFO("bare " << bare << " flat " << flat << " bent " << bent);
    REQUIRE(bare > 150);
    REQUIRE(std::abs(bare - flat) < 12);
    REQUIRE(bent < bare * 0.85);
    // A map that does not exist: the flat default stands in and the miss is reported next to the
    // sample's own missing mesh.
    REQUIRE(s.command("world.set", Json{{"entity", "Bent"}, {"component", "MeshRenderer"}, {"value", Json{{"normal_map", "assets/normal_down.png"}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["assets"]["missing"].size() == 2);
    // A metal orb next to a matte one under the same light: the metal shows a highlight where the
    // matte does not, and away from the highlight the metal is darker (no diffuse).
    REQUIRE(s.command("world.destroy", Json{{"entity", "Bent"}}).has_value());
    REQUIRE(s.command("world.destroy", Json{{"entity", "Bare"}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Matte"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 46}, {"y", 0.8}, {"z", 0}}}, {"scale", Json{{"x", 1.6}, {"y", 1.6}, {"z", 1.6}}}}}, {"MeshRenderer", Json{{"mesh", "sphere"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}, {"metallic", 0.0}, {"roughness", 1.0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Metal"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 40}, {"y", 0.8}, {"z", 0}}}, {"scale", Json{{"x", 1.6}, {"y", 1.6}, {"z", 1.6}}}}}, {"MeshRenderer", Json{{"mesh", "sphere"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}, {"metallic", 1.0}, {"roughness", 0.4}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    std::string path2 = (root() / "build" / "test-out" / "pbr-metal.png").string();
    Json cap2 = s.command("capture", Json{{"path", path2}}).value();
    // The highlight sits where the surface normal is the half vector between the light (from -Z,
    // 30 degrees up) and the camera (straight above): 0.8 * (0, 0.866, -0.5) from the center. A
    // point farther toward -Z still faces the light (matte bright) but is off the highlight, where
    // a metal has no diffuse to show.
    double metal_hi = brightness_at(s, cap2, {40.0f, 1.493f, -0.4f});
    double matte_hi = brightness_at(s, cap2, {46.0f, 1.493f, -0.4f});
    double metal_side = brightness_at(s, cap2, {40.0f, 1.187f, -0.7f});
    double matte_side = brightness_at(s, cap2, {46.0f, 1.187f, -0.7f});
    INFO("metal highlight " << metal_hi << " matte " << matte_hi << "; metal side " << metal_side << " matte side " << matte_side);
    REQUIRE(metal_hi > 700);
    REQUIRE(metal_hi > matte_hi * 1.15);
    REQUIRE(matte_side > 500);
    REQUIRE(metal_side < matte_side * 0.6);
    // Emissive: a black plane that glows white reads bright regardless of the light.
    REQUIRE(s.command("world.set", Json{{"entity", "Flat"}, {"component", "MeshRenderer"}, {"value", Json{{"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 0.9}, {"g", 0.9}, {"b", 0.9}, {"a", 1}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    std::string path3 = (root() / "build" / "test-out" / "pbr-emissive.png").string();
    Json cap3 = s.command("capture", Json{{"path", path3}}).value();
    REQUIRE(brightness_at(s, cap3, {43.0f, 0, 0}) > 600);
    // The plate asset's material carries three maps.
    Json d = s.command("assets.describe", Json{{"path", "assets/plate.glb"}}).value();
    INFO(d.dump());
    REQUIRE(d["materials"][0]["normal_texture"] == "assets/plate_normal.png");
    REQUIRE(d["materials"][0]["metallic_roughness_texture"] == "assets/plate_mr.png");
    REQUIRE(d["materials"][0]["emissive_texture"] == "assets/plate_glow.png");
    REQUIRE(d["materials"][0]["emissive"]["r"].get<double>() == Catch::Approx(1.0));
    REQUIRE(s.finish().has_value());
}
