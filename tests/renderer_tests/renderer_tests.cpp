#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>
#include <pocket/assets/assets.hpp>
#include <pocket/core/core.hpp>
#include <pocket/rhi/device.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <filesystem>
#include <array>
#include <cmath>
#include <fstream>
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
    REQUIRE(stats["meshes"] == 13);  // the scene's entities with meshes: the Walker, Pulse, Reacher, Gazer, Turner and Stepper arms included                 // ground, crate, pyramid, the skinned arm, the missing one, the plate, the orb
    REQUIRE(stats["draw_calls"].get<int>() == 8);   // the arms are instances of one mesh  // crate has two submeshes (two nodes share one material -> still two draws); the arm is one skinned draw
    REQUIRE(stats["skinned"] == 7);   // the Arm, the Walker, the Pulse arm, the Reacher, the Gazer, the Turner and the Stepper
    REQUIRE(stats["morphed"] == 1);   // the Pulse arm, its bulge weight already above zero
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
    REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["shadows"] == true);
    REQUIRE(stats["shadow_draws"].get<int>() >= 1);
    // Nothing moves: the next frame keeps the cascades it drew.
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["shadow_redrawn"] == 0);
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
    // Light overlays: the sun an arrow and a dot (3 + 24 lines), a point light the sphere it reaches
    // and a dot (72 + 24), a spot its cone (the outer circle, four edges, the inner circle) and a dot (24 + 4 + 24 + 24).
    REQUIRE(s.command("render.debug", Json{{"lights", true}}).value()["lights"] == true);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() == 27);
    REQUIRE(s.command("world.spawn", Json{{"name", "Bulb"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -2}, {"y", 1}, {"z", 0}}}}}, {"Light", Json{{"kind", 1}, {"range", 3}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Beam"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 2}, {"y", 3}, {"z", 0}}}, {"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}}}, {"Light", Json{{"kind", 2}, {"range", 4}, {"outer_angle", 25}, {"inner_angle", 15}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["debug_lines"].get<int>() == 27 + 96 + 76);
    REQUIRE(s.command("render.debug", Json{{"lights", false}}).has_value());
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
    // Three planes: no map, a flat map, a map bent toward +Y (glTF's convention: up in the image,
    // which is toward -Z in the world for a plane whose v runs along +Z, the image's top row at
    // v 0). The bent one faces the light, which comes from -Z and above, so it is brighter; the
    // flat map changes nothing.
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
    REQUIRE(bent > bare * 1.15);
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

TEST_CASE("MSAA smooths edges and keeps the ids through a pass of their own", "[renderer][msaa]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 20; ++i) REQUIRE(s.frame().has_value());
    // Hard transitions: pixels whose right neighbour differs by more than 60 in some channel. With
    // MSAA an edge is spread over an in-between pixel, so many transitions become two soft steps.
    auto hard_edges = [&]() {
        auto img = s.device().capture();
        REQUIRE(img.has_value());
        int hard = 0;
        for (std::uint32_t y = 0; y < img->height; ++y) {
            for (std::uint32_t x = 0; x + 1 < img->width; ++x) {
                const std::uint8_t* a = &img->rgba[(y * img->width + x) * 4];
                const std::uint8_t* b = a + 4;
                if (std::abs(int(a[0]) - int(b[0])) > 60 || std::abs(int(a[1]) - int(b[1])) > 60 || std::abs(int(a[2]) - int(b[2])) > 60) ++hard;
            }
        }
        return hard;
    };
    Json plain = s.command("render.stats", Json::object()).value();
    REQUIRE(plain["msaa"].get<int>() == 1);
    REQUIRE(plain["id_draws"].get<int>() == 0);
    const int hard_plain = hard_edges();
    Json pr = s.command("render.project", Json{{"entity", "/Level/Player"}}).value();
    REQUIRE(s.command("render.pick", Json{{"x", pr["x"]}, {"y", pr["y"]}}).value()["path"] == "/Level/Player");
    REQUIRE(s.command("render.msaa", Json{{"samples", 4}}).value()["msaa"].get<int>() == 4);
    REQUIRE(s.frame().has_value());
    Json aa = s.command("render.stats", Json::object()).value();
    INFO(aa.dump());
    REQUIRE(aa["msaa"].get<int>() == 4);
    REQUIRE(aa["id_draws"].get<int>() > 0);
    REQUIRE(aa["draw_calls"].get<int>() == plain["draw_calls"].get<int>());
    const int hard_aa = hard_edges();
    INFO("hard edges: " << hard_plain << " without MSAA, " << hard_aa << " with");
    REQUIRE(hard_plain > 50);
    REQUIRE(hard_aa < hard_plain * 3 / 4);
    // The ids still name the player under its projection, and the visible set is the same.
    REQUIRE(s.command("render.pick", Json{{"x", pr["x"]}, {"y", pr["y"]}}).value()["path"] == "/Level/Player");
    Json visible = s.command("render.visible", Json{{"limit", 20}}).value()["visible"];
    bool player = false, ground = false;
    for (const Json& v : visible) { if (v["path"] == "/Level/Player") player = true; if (v["path"] == "/Level/Ground") ground = true; }
    REQUIRE(player);
    REQUIRE(ground);
    // Back to one sample: the shared pass again, no id draws.
    REQUIRE(s.command("render.msaa", Json{{"samples", 1}}).value()["msaa"].get<int>() == 1);
    REQUIRE(s.frame().has_value());
    Json back = s.command("render.stats", Json::object()).value();
    REQUIRE(back["msaa"].get<int>() == 1);
    REQUIRE(back["id_draws"].get<int>() == 0);
    REQUIRE(s.command("render.pick", Json{{"x", pr["x"]}, {"y", pr["y"]}}).value()["path"] == "/Level/Player");
    REQUIRE(s.finish().has_value());
}

TEST_CASE("bloom spreads a glow past a bright surface and leaves the dark alone", "[renderer][bloom]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A white-hot cube in the dark, a camera straight above it.
    REQUIRE(s.command("world.spawn", Json{{"name", "Lamp"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"emissive", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 10}, {"z", 0.001}}}, {"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json vis = s.command("render.visible", Json{{"limit", 4}}).value();
    REQUIRE(vis["count"].get<int>() == 1);
    const Json& b = vis["visible"][0]["bounds"];
    // Just past the cube's right edge, and its center.
    const int outside_x = b["x"].get<int>() + b["width"].get<int>() + 3, y = b["y"].get<int>() + b["height"].get<int>() / 2;
    const Json pts = Json::array({Json{{"x", outside_x}, {"y", y}}, Json{{"x", b["x"].get<int>() + b["width"].get<int>() / 2}, {"y", y}}});
    Json plain = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("plain " << plain["pixels"].dump() << " bounds " << b.dump());
    REQUIRE(plain["render"]["bloom"] == false);
    REQUIRE(plain["pixels"][0][1].get<int>() < 60);    // the dark beside the cube
    REQUIRE(plain["pixels"][1][1].get<int>() > 200);   // the cube itself
    Json set = s.command("render.bloom", Json{{"enabled", true}, {"threshold", 0.6}, {"strength", 1.0}, {"radius", 2.0}}).value();
    REQUIRE(set["enabled"] == true);
    REQUIRE(set["radius"].get<double>() == Catch::Approx(2.0));
    REQUIRE(s.frame().has_value());
    Json glow = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("glow " << glow["pixels"].dump());
    REQUIRE(glow["render"]["bloom"] == true);
    REQUIRE(glow["render"]["draw_calls"].get<int>() == plain["render"]["draw_calls"].get<int>() + 4);
    REQUIRE(glow["pixels"][0][1].get<int>() > plain["pixels"][0][1].get<int>() + 12);   // the glow reaches past the edge
    REQUIRE(glow["pixels"][1][1].get<int>() > 200);
    // Far from anything bright, nothing changes.
    const Json far = Json::array({Json{{"x", 4}, {"y", 4}}});
    Json corner = s.command("capture", Json{{"pixels", far}}).value();
    REQUIRE(std::abs(corner["pixels"][0][1].get<int>() - plain["corner_pixel"][1].get<int>()) <= 2);
    // Off again: back to the plain frame.
    REQUIRE(s.command("render.bloom", Json{{"enabled", false}}).value()["enabled"] == false);
    REQUIRE(s.frame().has_value());
    Json back = s.command("capture", Json{{"pixels", pts}}).value();
    REQUIRE(back["render"]["bloom"] == false);
    REQUIRE(std::abs(back["pixels"][0][1].get<int>() - plain["pixels"][0][1].get<int>()) <= 2);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("grading exposes, tints, warms, rolls off and vignettes the finished frame", "[renderer][grade]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A white-hot slab filling the view (black albedo, white emissive: every pixel is white whatever the light).
    REQUIRE(s.command("world.spawn", Json{{"name", "Slab"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 0}}}, {"scale", Json{{"x", 40}, {"y", 1}, {"z", 40}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 10}, {"z", 0.001}}}, {"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    // The centre, a corner and a point a sixteenth of the way in from the left edge.
    const Json pts = Json::array({Json{{"x", 128}, {"y", 72}}, Json{{"x", 4}, {"y", 4}}, Json{{"x", 16}, {"y", 72}}});
    auto px = [&](const Json& cap, int i, int ch) { return cap["pixels"][static_cast<std::size_t>(i)][static_cast<std::size_t>(ch)].get<int>(); };
    Json plain = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("plain " << plain["pixels"].dump());
    REQUIRE(plain["render"]["grade"] == false);
    for (int i = 0; i < 3; ++i) for (int ch = 0; ch < 3; ++ch) REQUIRE(px(plain, i, ch) >= 250);
    // Half the exposure: half the light, one more draw, and the stats say so.
    Json set = s.command("render.grade", Json{{"enabled", true}, {"exposure", 0.5}}).value();
    REQUIRE(set["enabled"] == true);
    REQUIRE(set["exposure"].get<double>() == Catch::Approx(0.5));
    REQUIRE(set["tint"]["g"].get<double>() == Catch::Approx(1.0));
    REQUIRE(s.frame().has_value());
    Json half = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("half " << half["pixels"].dump());
    REQUIRE(half["render"]["grade"] == true);
    REQUIRE(half["render"]["draw_calls"].get<int>() == plain["render"]["draw_calls"].get<int>());   // grading is part of the final pass, not a pass of its own
    // Half the light in linear light is one stop down: 0.5 encodes to 188 of 255.
    REQUIRE(px(half, 0, 0) >= 182);
    REQUIRE(px(half, 0, 0) <= 194);
    // A tint washes the white by its channels; a hex string is a tint too.
    REQUIRE(s.command("render.grade", Json{{"exposure", 1.0}, {"tint", Json{{"r", 1}, {"g", 0.5}, {"b", 0.25}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json tinted = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("tinted " << tinted["pixels"].dump());
    REQUIRE(px(tinted, 0, 0) >= 240);
    REQUIRE(px(tinted, 0, 1) >= 110);
    REQUIRE(px(tinted, 0, 1) <= 145);
    REQUIRE(px(tinted, 0, 2) >= 50);
    REQUIRE(px(tinted, 0, 2) <= 80);
    Json hex = s.command("render.grade", Json{{"tint", "#ffffff"}}).value();
    REQUIRE(hex["tint"]["b"].get<double>() == Catch::Approx(1.0));
    REQUIRE_FALSE(s.command("render.grade", Json{{"tint", "warm"}}).has_value());
    // Warmth lifts red and drops blue; the filmic curve softens a plain white to about four fifths.
    REQUIRE(s.command("render.grade", Json{{"temperature", 1.0}}).has_value());
    REQUIRE(s.frame().has_value());
    Json warm = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("warm " << warm["pixels"].dump());
    REQUIRE(px(warm, 0, 0) >= 250);
    REQUIRE(px(warm, 0, 1) >= 250);
    REQUIRE(px(warm, 0, 2) >= 226);   // 0.85 of the light: 236
    REQUIRE(px(warm, 0, 2) <= 244);
    REQUIRE(s.command("render.grade", Json{{"temperature", 0.0}, {"filmic", true}}).has_value());
    REQUIRE(s.frame().has_value());
    Json filmic = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("filmic " << filmic["pixels"].dump());
    REQUIRE(px(filmic, 0, 0) >= 222);   // ACES takes white to about 0.8 of the light: 231
    REQUIRE(px(filmic, 0, 0) <= 238);
    // A full vignette: the centre stays, the corner goes black, the edge is part way.
    REQUIRE(s.command("render.grade", Json{{"filmic", false}, {"vignette", 1.0}}).has_value());
    REQUIRE(s.frame().has_value());
    Json vig = s.command("capture", Json{{"pixels", pts}}).value();
    INFO("vignette " << vig["pixels"].dump());
    REQUIRE(px(vig, 0, 0) >= 250);
    REQUIRE(px(vig, 1, 0) <= 10);
    REQUIRE(px(vig, 2, 0) >= 150);
    REQUIRE(px(vig, 2, 0) <= 215);
    // Off again: the plain frame, and no grade in the stats.
    REQUIRE(s.command("render.grade", Json{{"enabled", false}}).value()["enabled"] == false);
    REQUIRE(s.frame().has_value());
    Json back = s.command("capture", Json{{"pixels", pts}}).value();
    REQUIRE(back["render"]["grade"] == false);
    for (int i = 0; i < 3; ++i) REQUIRE(px(back, i, 0) >= 250);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("the scene is lit in HDR: light over white survives exposure, operators roll it off, the meter adapts, unlit art keeps its colors", "[renderer][hdr]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A black slab glowing four times white, filling the view from above.
    auto slab = [&](double glow) {
        return Json{{"entity", "Slab"}, {"component", "MeshRenderer"}, {"value", Json{{"emissive", Json{{"r", glow}, {"g", glow}, {"b", glow}, {"a", 1}}}}}};
    };
    REQUIRE(s.command("world.spawn", Json{{"name", "Slab"}, {"components", Json{{"Transform", Json{{"scale", Json{{"x", 40}, {"y", 1}, {"z", 40}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 4}, {"g", 4}, {"b", 4}, {"a", 1}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 10}, {"z", 0.001}}}, {"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    const Json centre = Json{{"x", 128}, {"y", 72}};
    auto grey = [&]() {
        REQUIRE(s.frame().has_value());
        Json c = s.command("capture", Json{{"pixel", centre}}).value();
        return c["pixel"][0].get<int>();
    };
    // Four times white clips at the screen, but an eighth of it is half the light (188), which an
    // 8-bit scene target would have clipped to white first and then darkened to 99.
    REQUIRE(grey() == 255);
    Json t = s.command("render.tonemap", Json{{"exposure", 0.125}}).value();
    REQUIRE(t["operator"] == "none");
    REQUIRE(t["exposure"].get<double>() == Catch::Approx(0.125));
    int g = grey();
    REQUIRE(g >= 182);
    REQUIRE(g <= 196);
    Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["hdr"] == true);
    REQUIRE(stats["tonemap"] == "none");
    // The operators on white: ACES about 0.8 of the light, AgX lower and softer, Neutral nearly white.
    REQUIRE(s.command("render.tonemap", Json{{"exposure", 0.25}, {"operator", "aces"}}).has_value());
    const int aces = grey();
    REQUIRE(s.command("render.tonemap", Json{{"operator", "agx"}}).has_value());
    const int agx = grey();
    REQUIRE(s.command("render.tonemap", Json{{"operator", "neutral"}}).has_value());
    const int neutral = grey();
    INFO("aces " << aces << " agx " << agx << " neutral " << neutral);
    REQUIRE(aces >= 222);
    REQUIRE(aces <= 240);
    REQUIRE(agx >= 185);
    REQUIRE(agx <= 218);
    REQUIRE(neutral >= 232);
    REQUIRE(neutral <= 250);
    REQUIRE(s.command("render.stats", Json::object()).value()["tonemap"] == "neutral");
    REQUIRE(s.command("render.tonemap", Json{{"operator", "sepia"}}).error().code == "bad_args");
    // The meter: a dim slab and a bright one both come out mid gray (0.18 of the light: 118).
    REQUIRE(s.command("render.tonemap", Json{{"operator", "none"}, {"exposure", 1}, {"auto_exposure", true}, {"speed", 20}}).has_value());
    REQUIRE(s.command("world.set", slab(0.25)).has_value());   // 0.25 as a color is 0.05 of the light
    g = grey();
    INFO("dim " << g);
    REQUIRE(std::abs(g - 118) <= 8);
    Json m = s.command("render.tonemap", Json::object()).value();
    INFO(m.dump());
    REQUIRE(m["metered"]["average_ev"].get<double>() == Catch::Approx(std::log2(0.058)).margin(0.4));
    REQUIRE(m["metered"]["exposure_ev"].get<double>() > 1.0);
    // Brighter by sixty times: the first frame after is still bright, the eye adapts over the next ones.
    REQUIRE(s.command("world.set", slab(3.0)).has_value());
    const int first = grey();
    INFO("first " << first);
    REQUIRE(first == 255);
    for (int i = 0; i < 40; ++i) REQUIRE(s.frame().has_value());
    g = grey();
    INFO("adapted " << g);
    REQUIRE(std::abs(g - 118) <= 8);
    m = s.command("render.tonemap", Json::object()).value();
    REQUIRE(m["metered"]["average_ev"].get<double>() == Catch::Approx(std::log2(3.0)).margin(0.2));
    REQUIRE(m["metered"]["exposure_ev"].get<double>() < -3.5);
    // One stop of compensation: twice the light (0.36: 161).
    REQUIRE(s.command("render.tonemap", Json{{"compensation", 1}}).has_value());
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    g = grey();
    INFO("compensated " << g);
    REQUIRE(std::abs(g - 161) <= 8);
    // Unlit art keeps its exact color through the linear scene: a mid-gray sprite is 128 again.
    REQUIRE(s.command("render.tonemap", Json{{"auto_exposure", false}, {"compensation", 0}}).has_value());
    REQUIRE(s.command("world.destroy", Json{{"entity", "Slab"}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 10}}}, {"rotation", Json{{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Card"}, {"components", Json{{"Transform", Json::object()}, {"Sprite", Json{{"size", Json{{"x", 40}, {"y", 40}}}, {"color", Json{{"r", 0.5}, {"g", 0.25}, {"b", 0.75}, {"a", 1}}}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json card = s.command("capture", Json{{"pixel", centre}}).value();
    INFO(card.dump());
    REQUIRE(std::abs(card["pixel"][0].get<int>() - 128) <= 1);
    REQUIRE(std::abs(card["pixel"][1].get<int>() - 64) <= 1);
    REQUIRE(std::abs(card["pixel"][2].get<int>() - 191) <= 1);
    REQUIRE(s.finish().has_value());
}

namespace {
// A flat Radiance .hdr (no run-length encoding): the top half `top`, the bottom half `bottom`.
void write_test_hdr(const std::filesystem::path& path, int w, int h, const float top[3], const float bottom[3]) {
    std::string out = "#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y " + std::to_string(h) + " +X " + std::to_string(w) + "\n";
    auto rgbe = [](const float c[3]) {
        const float m = std::max({c[0], c[1], c[2]});
        std::array<unsigned char, 4> px{0, 0, 0, 0};
        if (m < 1e-32f) return px;
        int e = 0;
        const float f = std::frexp(m, &e) * 256.0f / m;
        for (int i = 0; i < 3; ++i) px[static_cast<std::size_t>(i)] = static_cast<unsigned char>(c[i] * f);
        px[3] = static_cast<unsigned char>(e + 128);
        return px;
    };
    for (int y = 0; y < h; ++y) {
        const auto px = rgbe(y < h / 2 ? top : bottom);
        for (int x = 0; x < w; ++x) out.append(reinterpret_cast<const char*>(px.data()), 4);
    }
    std::ofstream(path, std::ios::binary) << out;
}
}  // namespace

TEST_CASE("a sky is drawn behind the scene and lights it: gradient, sun disc, reflections, diffuse light, HDR panoramas", "[renderer][sky]") {
    app::Session s(playground_options());
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A camera at the origin looking along -Z at the horizon, a sun low in front of it, no other light.
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json::object()}, {"Camera", Json{{"fov_degrees", 60}}}}}}).has_value());
    const Vec3 toward_sun = normalize(Vec3{0.3f, 0.25f, -1.0f});
    // The light shines along its -Z: turn -Z onto the direction away from the sun.
    const Vec3 away = toward_sun * -1.0f;
    const Vec3 axis = normalize(cross(Vec3{0, 0, -1}, away));
    const Quat q = Quat::from_axis_angle(axis, std::acos(std::clamp(dot(Vec3{0, 0, -1}, away), -1.0f, 1.0f)));
    const Json rotation{{"x", q.x}, {"y", q.y}, {"z", q.z}, {"w", q.w}};
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", rotation}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 1.0}};
    REQUIRE(s.command("world.spawn", sun).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json::object()}}}}).has_value());
    REQUIRE(s.frame().has_value());
    auto at = [&](const Json& cap, int i, int ch) { return cap["pixels"][static_cast<std::size_t>(i)][static_cast<std::size_t>(ch)].get<int>(); };
    auto project = [&](Vec3 p) {
        Json r = s.command("render.project", Json{{"point", Json{{"x", p.x}, {"y", p.y}, {"z", p.z}}}}).value();
        return Json{{"x", r["x"]}, {"y", r["y"]}};
    };
    // Up in the sky, down on the ground, and the sun.
    Json cap = s.command("capture", Json{{"pixels", Json::array({Json{{"x", 60}, {"y", 3}}, Json{{"x", 60}, {"y", 140}}, project(toward_sun * 100.0f)})}}).value();
    INFO(cap["pixels"].dump() << " " << cap["render"].dump());
    REQUIRE(cap["render"]["sky"] == "procedural");
    REQUIRE(cap["render"]["env_updates"].get<int>() >= 1);
    REQUIRE(at(cap, 0, 2) > at(cap, 0, 0) + 20);    // the sky is blue up high
    REQUIRE(at(cap, 0, 2) > 150);
    REQUIRE(at(cap, 1, 2) < at(cap, 0, 2) - 40);    // the ground is darker and not blue
    REQUIRE(at(cap, 2, 0) >= 250);                  // the sun's disc
    REQUIRE(s.command("render.pick", Json{{"x", 60}, {"y", 3}}).value()["id"] == 0);   // the sky is background: nothing to pick
    // The environment is rebuilt only when what it is made of changes.
    const int updates = cap["render"]["env_updates"].get<int>();
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["env_updates"].get<int>() == updates);
    // A mirror ball in front: its top reflects the sky, its bottom the ground.
    Json ball;
    ball["name"] = "Ball";
    ball["components"]["Transform"]["position"] = Json{{"x", 0}, {"y", 0}, {"z", -3}};
    ball["components"]["MeshRenderer"] = Json{{"mesh", "sphere"}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}, {"metallic", 1.0}, {"roughness", 0.05}};
    REQUIRE(s.command("world.spawn", ball).has_value());
    REQUIRE(s.frame().has_value());
    const Vec3 top{0, 0.35f, -3 + 0.357f}, bottom{0, -0.35f, -3 + 0.357f};
    cap = s.command("capture", Json{{"pixels", Json::array({project(top), project(bottom)})}}).value();
    INFO("mirror " << cap["pixels"].dump());
    REQUIRE(at(cap, 0, 2) > at(cap, 0, 0) + 15);    // the sky in the top
    REQUIRE(at(cap, 0, 2) > at(cap, 1, 2) + 40);    // the ground in the bottom
    // A rough white ball lit by the sky alone (the sun turned off): lit with it, dark without it.
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Light"}, {"value", Json{{"intensity", 0.0}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}, {"value", Json{{"metallic", 0.0}, {"roughness", 1.0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const Json lit = s.command("capture", Json{{"pixels", Json::array({project(top), project(bottom)})}}).value();
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"diffuse", 0.0}, {"specular", 0.0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const Json unlit = s.command("capture", Json{{"pixels", Json::array({project(top), project(bottom)})}}).value();
    INFO("sky-lit " << lit["pixels"].dump() << " unlit " << unlit["pixels"].dump());
    REQUIRE(at(lit, 0, 1) > 100);
    REQUIRE(at(lit, 0, 2) > at(lit, 0, 0));          // lit from the blue above
    REQUIRE(at(lit, 0, 1) > at(lit, 1, 1) + 15);     // brighter on top than underneath
    REQUIRE(at(unlit, 0, 1) < 12);
    // An HDR panorama, red above and dim green below: seen behind, and reflected by a mirror.
    const std::filesystem::path pano = root() / "samples" / "playground" / "assets" / "sky-test.hdr";
    const float red[3] = {2.0f, 0.0f, 0.0f}, green[3] = {0.0f, 0.5f, 0.0f};
    const bool made_dir = std::filesystem::create_directories(pano.parent_path());
    write_test_hdr(pano, 64, 32, red, green);
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"mode", 2}, {"image", "assets/sky-test.hdr"}, {"diffuse", 1.0}, {"specular", 1.0}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}, {"value", Json{{"metallic", 1.0}, {"roughness", 0.05}}}}).has_value());
    REQUIRE(s.frame().has_value());
    cap = s.command("capture", Json{{"pixels", Json::array({Json{{"x", 60}, {"y", 3}}, Json{{"x", 60}, {"y", 140}}, project(top), project(bottom)})}}).value();
    std::filesystem::remove(pano);
    if (made_dir) std::filesystem::remove(pano.parent_path());
    INFO("panorama " << cap["pixels"].dump() << " " << cap["render"].dump());
    REQUIRE(cap["render"]["sky"] == "image");
    REQUIRE(at(cap, 0, 0) >= 250);
    REQUIRE(at(cap, 0, 1) < 10);
    REQUIRE(at(cap, 1, 1) > 150);                   // 0.5 of the light encodes to 188
    REQUIRE(at(cap, 1, 0) < 10);
    REQUIRE(at(cap, 2, 0) > at(cap, 2, 1) + 100);   // the mirror's top is red
    REQUIRE(at(cap, 3, 1) > at(cap, 3, 0) + 60);    // and its bottom green
    const Json described = s.command("assets.describe", Json{{"path", "assets/sky-test.hdr"}}).value();
    REQUIRE(s.finish().has_value());
}

TEST_CASE("cascaded shadows keep a nearby shadow's edge sharp and still reach far away", "[renderer][cascades]") {
    app::Options o = playground_options();
    o.width = 480;
    o.height = 270;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A ground 400 units wide seen from eye height, a thin post four units ahead and a block fifty
    // units off, the sun shining toward +X and down at 45 degrees.
    Json ground;
    ground["name"] = "Ground";
    ground["components"]["Transform"]["scale"] = Json{{"x", 400}, {"y", 1}, {"z", 400}};
    ground["components"]["MeshRenderer"] = Json{{"mesh", "plane"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}};
    REQUIRE(s.command("world.spawn", ground).has_value());
    Json post;
    post["name"] = "Post";
    post["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 0.5}, {"z", -4}}}, {"scale", Json{{"x", 0.2}, {"y", 1}, {"z", 0.2}}}};
    post["components"]["MeshRenderer"] = Json{{"mesh", "cube"}};
    REQUIRE(s.command("world.spawn", post).has_value());
    Json block;
    block["name"] = "Block";
    block["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", -50}}}, {"scale", Json{{"x", 2}, {"y", 2}, {"z", 2}}}};
    block["components"]["MeshRenderer"] = Json{{"mesh", "cube"}};
    REQUIRE(s.command("world.spawn", block).has_value());
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"]["rotation"] = Json{{"x", -0.5}, {"y", -0.5}, {"z", 0}, {"w", 0.70710678}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 1.2}};
    REQUIRE(s.command("world.spawn", sun).has_value());
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0.5}, {"y", 1.7}, {"z", 0}}}, {"rotation", Json{{"x", -0.1736}, {"y", 0}, {"z", 0}, {"w", 0.9848}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 60}, {"far", 400}};
    REQUIRE(s.command("world.spawn", camera).has_value());
    // The width of the post's shadow edge in samples along the ground, from 10 to 90 percent of the
    // way from shadowed to lit (the shadow runs from x 0.1 to about 1.1 at z -4).
    auto edge = [&](const char* name) {
        for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
        const std::string path = (root() / "build" / "test-out" / name).string();
        Json cap = s.command("capture", Json{{"path", path}}).value();
        std::vector<double> b;
        for (int i = 0; i <= 60; ++i) b.push_back(brightness_at(s, cap, {0.6f + 0.02f * static_cast<float>(i), 0, -4}));
        const double lo = *std::min_element(b.begin(), b.end()), hi = *std::max_element(b.begin(), b.end());
        int width = 0;
        for (double v : b) width += (v > lo + 0.1 * (hi - lo) && v < lo + 0.9 * (hi - lo)) ? 1 : 0;
        INFO(name << " shadowed " << lo << " lit " << hi << " edge samples " << width);
        REQUIRE(hi > lo + 150);   // there is a shadow with an edge in the strip
        return std::make_pair(width, cap);
    };
    auto [sharp, cap4] = edge("cascades-4.png");
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["shadow_cascades"] == 4);
    REQUIRE(stats["shadow_distance"].get<double>() == Catch::Approx(80.0).margin(0.5));
    // The far block's shadow lands too, fifty units out (seen from above, where it is more than a line).
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 0}, {"y", 30}, {"z", -10}}}, {"rotation", Json{{"x", -0.4226}, {"y", 0}, {"z", 0}, {"w", 0.9063}}}}}}).has_value());
    for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
    Json high = s.command("capture", Json{{"path", (root() / "build" / "test-out" / "cascades-far.png").string()}}).value();
    const double far_shade = brightness_at(s, high, {2.0f, 0, -50}), far_lit = brightness_at(s, high, {-3.0f, 0, -50});
    INFO("far shade " << far_shade << " far lit " << far_lit);
    REQUIRE(far_shade < far_lit * 0.8);
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 0.5}, {"y", 1.7}, {"z", 0}}}, {"rotation", Json{{"x", -0.1736}, {"y", 0}, {"z", 0}, {"w", 0.9848}}}}}}).has_value());
    // One cascade over the same eighty units: the near edge is softer (wider in samples).
    Json one = s.command("render.shadows", Json{{"cascades", 1}}).value();
    REQUIRE(one["cascades"] == 1);
    auto [soft, cap1] = edge("cascades-1.png");
    REQUIRE(s.command("render.stats", Json::object()).value()["shadow_cascades"] == 1);
    INFO("four cascades " << sharp << " one cascade " << soft);
    REQUIRE(soft > sharp + 2);
    REQUIRE(s.command("render.shadows", Json{{"cascades", 9}}).value()["cascades"] == 4);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("soft shadows are sharp at their casters and soft far from them; contact shadows fill what the maps miss", "[renderer][softshadows]") {
    app::Options o = playground_options();
    o.width = 480;
    o.height = 270;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A post 4 high and 0.6 wide on a floor, the sun at 45 degrees toward -z (so the post's shadow
    // runs 4 units that way), seen from straight above: the shadow's side edge near the post's foot
    // is cast from 0.4 away, near the far end from nearly 5.
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", components}}).has_value()); };
    spawn("Floor", Json{{"Transform", Json{{"scale", Json{{"x", 40}, {"y", 1}, {"z", 40}}}}}, {"MeshRenderer", Json{{"mesh", "plane"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}}}});
    spawn("Post", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 2}, {"z", 0}}}, {"scale", Json{{"x", 0.6}, {"y", 4}, {"z", 0.6}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}});
    spawn("Sun", Json{{"Transform", Json{{"rotation", Json{{"x", -0.3827}, {"y", 0}, {"z", 0}, {"w", 0.9239}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1.2}}}});
    spawn("Camera", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 12}, {"z", -2}}}, {"rotation", Json{{"x", -0.7071}, {"y", 0}, {"z", 0}, {"w", 0.7071}}}}}, {"Camera", Json{{"fov_degrees", 50}}}});
    // A row of floor points across the shadow's left edge (x -0.3), each one's brightness from one capture.
    auto row = [&](float z) {
        Json pts = Json::array();
        for (int i = 0; i <= 50; ++i) {
            Json pr = s.command("render.project", Json{{"point", Json{{"x", -0.8f + 0.02f * static_cast<float>(i)}, {"y", 0}, {"z", z}}}}).value();
            pts.push_back(Json{{"x", pr["x"]}, {"y", pr["y"]}});
        }
        return pts;
    };
    REQUIRE(s.frame().has_value());   // render.project reads the camera the last frame drew with
    const Json low_row = row(-0.7f), high_row = row(-3.4f);
    // The widths of the two edges in samples, from 10 to 90 percent of the way from shadowed to lit.
    auto edges = [&]() {
        for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
        Json all = low_row;
        for (const Json& p : high_row) all.push_back(p);
        Json cap = s.command("capture", Json{{"pixels", all}, {"path", (root() / "build" / "test-out" / "soft-shadows.png").string()}}).value();
        auto width = [&](std::size_t from) {
            std::vector<double> b;
            for (std::size_t i = from; i < from + 51; ++i) b.push_back(cap["pixels"][i][1].get<double>());
            const double lo = *std::min_element(b.begin(), b.end()), hi = *std::max_element(b.begin(), b.end());
            REQUIRE(hi > lo + 60);   // an edge between shadow and light in the row
            int w = 0;
            for (double v : b) w += (v > lo + 0.1 * (hi - lo) && v < lo + 0.9 * (hi - lo)) ? 1 : 0;
            return w;
        };
        return std::make_pair(width(0), width(51));
    };
    auto [near_hard, far_hard] = edges();
    Json soft = s.command("render.shadows", Json{{"softness", 1.5}}).value();
    REQUIRE(soft["softness"].get<double>() == Catch::Approx(1.5));
    auto [near_soft, far_soft] = edges();
    INFO("edge widths in 2 cm samples, near the foot / far out: sharp " << near_hard << " / " << far_hard << ", soft " << near_soft << " / " << far_soft);
    REQUIRE(s.command("render.stats", Json::object()).value()["soft_shadows"] == true);
    REQUIRE(std::abs(far_hard - near_hard) <= 2);   // the sharp filter: one width anywhere
    REQUIRE(far_soft > far_hard + 3);               // under a sun of some size the far edge widens
    REQUIRE(far_soft > near_soft + 3);              // and more than the edge by the post's foot
    REQUIRE(s.command("render.shadows", Json{{"softness", 99}}).value()["softness"].get<double>() == Catch::Approx(5.0));
    // Contact shadows, with the maps off: a small block on the floor darkens the floor on its far
    // side from the sun, and open floor stays as it was.
    REQUIRE(s.command("render.shadows", Json{{"softness", 0}, {"enabled", false}}).has_value());
    spawn("Block", Json{{"Transform", Json{{"position", Json{{"x", 3}, {"y", 0.2}, {"z", 2}}}, {"scale", Json{{"x", 0.4}, {"y", 0.4}, {"z", 0.4}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}});
    auto contact = [&]() {
        for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
        auto at = [&](Vec3 p) {
            Json pr = s.command("render.project", Json{{"point", Json{{"x", p.x}, {"y", p.y}, {"z", p.z}}}}).value();
            return Json{{"x", pr["x"]}, {"y", pr["y"]}};
        };
        // Just past the block on the side away from the sun (-z), and open floor beside it.
        Json cap = s.command("capture", Json{{"pixels", Json::array({at({3, 0, 1.68f}), at({1.5f, 0, 1.68f})})}}).value();
        return std::make_pair(cap["pixels"][0][1].get<int>(), cap["pixels"][1][1].get<int>());
    };
    auto [behind_off, open_off] = contact();
    Json on = s.command("render.shadows", Json{{"contact", true}, {"contact_length", 0.6}}).value();
    REQUIRE(on["contact"] == true);
    auto [behind_on, open_on] = contact();
    Json stats = s.command("render.stats", Json::object()).value();
    INFO("behind the block " << behind_off << " -> " << behind_on << ", open floor " << open_off << " -> " << open_on << " " << stats.dump());
    REQUIRE(stats["contact_shadows"] == true);
    REQUIRE(stats["depth_prepass"] == true);
    REQUIRE(stats["ao"] == false);
    REQUIRE(std::abs(behind_off - open_off) < 8);   // without them the maps' absence leaves it lit
    REQUIRE(behind_on < behind_off - 30);            // with them the floor at the block's foot darkens
    REQUIRE(std::abs(open_on - open_off) < 6);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("cloth, specular, brushed metal and unlit surfaces answer the light their own way", "[renderer][materials]") {
    app::Options o = playground_options();
    o.width = 480;
    o.height = 270;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", components}}).has_value()); };
    auto set_mr = [&](const char* name, Json value) { REQUIRE(s.command("world.set", Json{{"entity", name}, {"component", "MeshRenderer"}, {"value", value}}).has_value()); };
    auto grey = [&](const Json& cap, std::size_t i) { return cap["pixels"][i][1].get<int>(); };
    // A sphere at the origin seen from 3 units along +z, the sun shining from behind the camera.
    spawn("Ball", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "sphere"}, {"color", Json{{"r", 0.1}, {"g", 0.1}, {"b", 0.1}, {"a", 1}}}, {"roughness", 0.3}, {"metallic", 0}}}});
    spawn("Sun", Json{{"Transform", Json::object()}, {"Light", Json{{"kind", 0}, {"intensity", 1.2}}}});
    spawn("Camera", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 3}}}}}, {"Camera", Json{{"fov_degrees", 40}}}});
    REQUIRE(s.frame().has_value());
    auto at = [&](Vec3 p) {
        Json pr = s.command("render.project", Json{{"point", Json{{"x", p.x}, {"y", p.y}, {"z", p.z}}}}).value();
        return Json{{"x", pr["x"]}, {"y", pr["y"]}};
    };
    // The highlight at the middle, and a ring 0.4 out toward the rim.
    Json probes = Json::array({at({0, 0, 0.5f})});
    for (int k = 0; k < 8; ++k) probes.push_back(at({0.4f * std::cos(0.785f * static_cast<float>(k)), 0.4f * std::sin(0.785f * static_cast<float>(k)), 0.3f}));
    auto look = [&]() {
        for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
        Json cap = s.command("capture", Json{{"pixels", probes}}).value();
        int ring = 0;
        for (std::size_t i = 1; i < 9; ++i) ring += grey(cap, i);
        return std::make_pair(grey(cap, 0), ring / 8);
    };
    auto [plain_mid, plain_ring] = look();
    // KHR_materials_specular: no reflection from a non-metal at 0.
    set_mr("Ball", Json{{"specular", 0.0}});
    auto [matte_mid, matte_ring] = look();
    // KHR_materials_sheen: a white sheen over the dark ball brightens it toward its rim.
    set_mr("Ball", Json{{"specular", -1.0}, {"sheen", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}, {"sheen_roughness", 0.3}});
    auto [sheen_mid, sheen_ring] = look();
    INFO("middle / ring: plain " << plain_mid << "/" << plain_ring << ", specular 0 " << matte_mid << "/" << matte_ring << ", sheen " << sheen_mid << "/" << sheen_ring);
    REQUIRE(matte_mid < plain_mid - 40);          // the highlight is gone
    REQUIRE(sheen_ring > plain_ring + 25);        // the cloth's glow
    // Unlit: with the sun from the side, a lit ball is bright on one side and dark on the other;
    // unlit it is its colour on both.
    set_mr("Ball", Json{{"sheen", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"color", Json{{"r", 0.2}, {"g", 0.6}, {"b", 0.2}, {"a", 1}}}});
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"x", 0}, {"y", 0.7071}, {"z", 0}, {"w", 0.7071}}}}}}).has_value());
    const Json sides = Json::array({at({-0.35f, 0, 0.35f}), at({0.35f, 0, 0.35f})});
    auto both = [&]() {
        for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
        Json cap = s.command("capture", Json{{"pixels", sides}}).value();
        return std::make_pair(grey(cap, 0), grey(cap, 1));
    };
    auto [lit_l, lit_r] = both();
    set_mr("Ball", Json{{"unlit", true}});
    auto [unlit_l, unlit_r] = both();
    INFO("left / right lit " << lit_l << "/" << lit_r << ", unlit " << unlit_l << "/" << unlit_r);
    REQUIRE(std::abs(lit_l - lit_r) > 60);
    REQUIRE(std::abs(unlit_l - unlit_r) <= 3);
    // KHR_materials_anisotropy: a polished metal floor under a lamp, seen from straight above. The
    // highlight is round, stretched along the uv's u at 1, and along the other axis turned 90 degrees.
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    spawn("Floor", Json{{"Transform", Json{{"scale", Json{{"x", 6}, {"y", 1}, {"z", 6}}}}}, {"MeshRenderer", Json{{"mesh", "plane"}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}, {"metallic", 1}, {"roughness", 0.2}}}});
    spawn("Lamp", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 0}}}}}, {"Light", Json{{"kind", 1}, {"intensity", 0.6}, {"range", 8}}}});
    spawn("Dark", Json{{"Transform", Json::object()}, {"Light", Json{{"kind", 0}, {"intensity", 0.0}}}});   // no sun: else a default key light
    spawn("Camera", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 4}, {"z", 0}}}, {"rotation", Json{{"x", -0.7071}, {"y", 0}, {"z", 0}, {"w", 0.7071}}}}}, {"Camera", Json{{"fov_degrees", 50}}}});
    REQUIRE(s.frame().has_value());
    Json cross = Json::array();
    for (int i = -60; i <= 60; ++i) cross.push_back(Json{{"x", 240 + 2 * i}, {"y", 135}});
    for (int i = -60; i <= 60; ++i) cross.push_back(Json{{"x", 240}, {"y", 135 + 2 * i}});
    // How far the highlight reaches across the image and down it: samples over half its peak.
    int shot = 0;
    auto spread = [&]() {
        for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());
        Json cap = s.command("capture", Json{{"pixels", cross}, {"path", (root() / "build" / "test-out" / ("anisotropy-" + std::to_string(shot++) + ".png")).string()}}).value();
        int peak = 0, floor_level = 255;
        for (std::size_t i = 0; i < 242; ++i) { peak = std::max(peak, grey(cap, i)); floor_level = std::min(floor_level, grey(cap, i)); }
        const int half = floor_level + (peak - floor_level) / 2;
        int across = 0, down = 0;
        for (std::size_t i = 0; i < 121; ++i) across += grey(cap, i) > half ? 1 : 0;
        for (std::size_t i = 121; i < 242; ++i) down += grey(cap, i) > half ? 1 : 0;
        return std::make_pair(across, down);
    };
    auto [round_x, round_y] = spread();
    set_mr("Floor", Json{{"anisotropy", 1.0}});
    auto [brushed_x, brushed_y] = spread();
    set_mr("Floor", Json{{"anisotropy", 1.0}, {"anisotropy_rotation", 90.0}});
    auto [turned_x, turned_y] = spread();
    INFO("highlight across / down: round " << round_x << "/" << round_y << ", brushed " << brushed_x << "/" << brushed_y << ", turned 90 " << turned_x << "/" << turned_y);
    REQUIRE(std::max(round_x, round_y) < std::min(round_x, round_y) * 1.3 + 2);
    const bool along_x = brushed_x > brushed_y;
    REQUIRE(std::max(brushed_x, brushed_y) > std::min(brushed_x, brushed_y) * 1.8);
    REQUIRE((turned_x > turned_y) != along_x);   // turned a quarter, it runs the other way
    REQUIRE(std::max(turned_x, turned_y) > std::min(turned_x, turned_y) * 1.8);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("ambient occlusion darkens the ground where a crate sits and leaves open ground alone; ids still come from the prepass", "[renderer][ao]") {
    app::Options o = playground_options();
    o.width = 320;
    o.height = 180;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // Lit by a sky alone (the sun at zero), a crate on a wide floor, seen from above at an angle.
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"]["scale"] = Json{{"x", 40}, {"y", 1}, {"z", 40}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "plane"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}};
    REQUIRE(s.command("world.spawn", floor).has_value());
    Json crate;
    crate["name"] = "Crate";
    crate["components"]["Transform"]["position"] = Json{{"x", 0}, {"y", 0.5}, {"z", 0}};
    crate["components"]["MeshRenderer"] = Json{{"mesh", "cube"}};
    REQUIRE(s.command("world.spawn", crate).has_value());
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json::object();
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 0.0}};
    REQUIRE(s.command("world.spawn", sun).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json::object()}}}}).has_value());
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 3.5}, {"z", 4}}}, {"rotation", Json{{"x", -0.3827}, {"y", 0}, {"z", 0}, {"w", 0.9239}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 50}};
    REQUIRE(s.command("world.spawn", camera).has_value());
    auto shot = [&]() {
        REQUIRE(s.frame().has_value());
        auto project = [&](Vec3 p) {
            Json r = s.command("render.project", Json{{"point", Json{{"x", p.x}, {"y", p.y}, {"z", p.z}}}}).value();
            return Json{{"x", r["x"]}, {"y", r["y"]}};
        };
        // The floor just in front of the crate's base, and open floor off to the side.
        Json cap = s.command("capture", Json{{"pixels", Json::array({project({0, 0, 0.56f}), project({-2.5f, 0, 1.0f})})}}).value();
        return std::make_pair(cap["pixels"][0][1].get<int>(), cap["pixels"][1][1].get<int>());
    };
    auto [near_off, open_off] = shot();
    Json on = s.command("render.ao", Json{{"enabled", true}, {"radius", 0.6}}).value();
    REQUIRE(on["enabled"] == true);
    auto [near_on, open_on] = shot();
    Json stats = s.command("render.stats", Json::object()).value();
    INFO("off " << near_off << "/" << open_off << " on " << near_on << "/" << open_on << " " << stats.dump());
    REQUIRE(stats["ao"] == true);
    REQUIRE(stats["depth_prepass"] == true);
    REQUIRE(std::abs(near_off - open_off) < 12);          // without it, the base looks like open floor
    REQUIRE(near_on < near_off - 15);                       // with it, the ground at the base is darker
    REQUIRE(std::abs(open_on - open_off) < 8);              // and open floor is as it was
    // The prepass also draws the ids: the crate is still picked where it is drawn.
    Json pr = s.command("render.project", Json{{"entity", "Crate"}}).value();
    REQUIRE(s.command("render.pick", Json{{"x", pr["x"]}, {"y", pr["y"]}}).value()["name"] == "Crate");
    REQUIRE(s.command("render.ao", Json{{"enabled", false}}).value()["enabled"] == false);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["depth_prepass"] == false);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("fog fades what is far toward its color, thins with height and leaves what is near", "[renderer][fog]") {
    app::Options o = playground_options();
    o.width = 320;
    o.height = 180;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // A near and a far black block on the horizon line, a red fog.
    auto block = [&](const char* name, float x, float z) {
        Json b;
        b["name"] = name;
        b["components"]["Transform"] = Json{{"position", Json{{"x", x}, {"y", 1}, {"z", z}}}, {"scale", Json{{"x", 2}, {"y", 2}, {"z", 1}}}};
        b["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}};
        REQUIRE(s.command("world.spawn", b).has_value());
    };
    block("Near", -1.5f, -3);
    block("Far", 6, -80);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 0}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 60}};
    REQUIRE(s.command("world.spawn", camera).has_value());
    auto red_at = [&](float z, float x) {
        REQUIRE(s.frame().has_value());
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", 1}, {"z", z + 0.5f}}}}).value();
        Json cap = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value();
        return cap["pixel"][0].get<int>();
    };
    const int near_plain = red_at(-3, -1.5f), far_plain = red_at(-80, 6);
    REQUIRE(near_plain < 30);
    REQUIRE(far_plain < 30);
    Json fog;
    fog["name"] = "Fog";
    fog["components"]["Fog"] = Json{{"color", Json{{"r", 1}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"density", 0.05}, {"falloff", 0.0}};
    REQUIRE(s.command("world.spawn", fog).has_value());
    const int near_fog = red_at(-3, -1.5f), far_fog = red_at(-80, 6);
    Json stats = s.command("render.stats", Json::object()).value();
    INFO("near " << near_plain << "->" << near_fog << " far " << far_plain << "->" << far_fog << " " << stats.dump());
    REQUIRE(stats["fog"] == true);
    REQUIRE(stats["depth_prepass"] == true);
    REQUIRE(far_fog > 220);                 // eighty units at 0.05: almost all fog
    REQUIRE(near_fog < far_fog - 60);       // three units: some, far less
    // Thinning with height: a strong falloff from a base far below leaves the far block nearly clear.
    REQUIRE(s.command("world.set", Json{{"entity", "Fog"}, {"component", "Fog"}, {"value", Json{{"height", -20.0}, {"falloff", 0.5}}}}).has_value());
    const int far_thin = red_at(-80, 6);
    INFO("thin " << far_thin);
    REQUIRE(far_thin < far_fog - 100);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("spot lights light a cone; any number of point lights reach the pixels they touch through the clusters", "[renderer][lights]") {
    app::Options o = playground_options();
    o.width = 320;
    o.height = 180;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    const Json down{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}};   // -Z turned to -Y
    // A white floor seen from straight above, a sun of no strength (so no default key light).
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 60}, {"y", 0.2}, {"z", 60}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(floor);
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", down}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 0}};
    spawn(sun);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 12}, {"z", 0}}}, {"rotation", down}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 60}};
    spawn(camera);
    auto pixel_at = [&](float x, float z) {
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", 0}, {"z", z}}}}).value();
        Json cap = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value();
        return std::array<int, 3>{cap["pixel"][0].get<int>(), cap["pixel"][1].get<int>(), cap["pixel"][2].get<int>()};
    };

    SECTION("a spot's cone") {
        Json spot;
        spot["name"] = "Spot";
        spot["components"]["Transform"] = Json{{"position", Json{{"x", -4}, {"y", 3}, {"z", 0}}}, {"rotation", down}};
        spot["components"]["Light"] = Json{{"kind", 2}, {"intensity", 3}, {"range", 5}, {"inner_angle", 15}, {"outer_angle", 20}};
        spawn(spot);
        Json point;
        point["name"] = "Point";
        point["components"]["Transform"] = Json{{"position", Json{{"x", 4}, {"y", 3}, {"z", 0}}}};
        point["components"]["Light"] = Json{{"kind", 1}, {"intensity", 3}, {"range", 5}};
        spawn(point);
        REQUIRE(s.frame().has_value());
        const int ambient = pixel_at(0, -5)[0];
        const int spot_center = pixel_at(-4, 0)[0], spot_aside = pixel_at(-4, 2)[0];
        const int point_center = pixel_at(4, 0)[0], point_aside = pixel_at(4, 2)[0];
        Json stats = s.command("render.stats", Json::object()).value();
        INFO("ambient " << ambient << " spot " << spot_center << "/" << spot_aside << " point " << point_center << "/" << point_aside << " " << stats.dump());
        REQUIRE(stats["spot_lights"] == 1);
        REQUIRE(stats["point_lights"] == 1);
        REQUIRE(spot_center > ambient + 80);        // inside the cone: lit
        REQUIRE(spot_aside < ambient + 6);          // two units aside, well outside twenty degrees: dark
        REQUIRE(point_aside > ambient + 30);        // the same place beside a point light is lit
        REQUIRE(std::abs(spot_center - point_center) < 12);   // under the cone's axis a spot is a point light
        // Turned to shine sideways, the spot leaves the floor under it dark.
        REQUIRE(s.command("world.set", Json{{"entity", "Spot"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
        REQUIRE(pixel_at(-4, 0)[0] < ambient + 6);
    }

    SECTION("four hundred lights, culled to the view and listed by cluster") {
        // A grid of small colored lights two units apart, each reaching 1.5: the floor under one is
        // lit by that one alone, in its color. The view covers about 24 by 14 units of the 40 by 40.
        for (int i = 0; i < 20; ++i)
            for (int j = 0; j < 20; ++j) {
                const int k = (i + j) % 3;
                Json l;
                l["name"] = "L" + std::to_string(i) + "_" + std::to_string(j);
                l["components"]["Transform"] = Json{{"position", Json{{"x", -19 + 2 * i}, {"y", 0.5}, {"z", -19 + 2 * j}}}};
                l["components"]["Light"] = Json{{"kind", 1}, {"intensity", 2}, {"range", 1.5}, {"color", Json{{"r", k == 0 ? 1 : 0}, {"g", k == 1 ? 1 : 0}, {"b", k == 2 ? 1 : 0}, {"a", 1}}}};
                spawn(l);
            }
        REQUIRE(s.frame().has_value());
        Json stats = s.command("render.stats", Json::object()).value();
        INFO(stats.dump());
        const int visible = stats["point_lights"].get<int>();
        REQUIRE(visible > 60);                                   // far past the old eight
        REQUIRE(visible < 400);
        REQUIRE(stats["lights"]["culled"].get<int>() == 400 - visible);
        REQUIRE(stats["lights"]["dropped"] == 0);
        REQUIRE(stats["lights"]["clusters"] == Json::array({16, 9, 24}));
        REQUIRE(stats["lights"]["entries"].get<int>() >= visible);
        REQUIRE(stats["lights"]["max_per_cluster"].get<int>() <= 20);   // a cluster lists only the lights near it, not all of them
        auto dominant = [](std::array<int, 3> p) { return p[0] > p[1] + 40 && p[0] > p[2] + 40 ? 0 : p[1] > p[0] + 40 && p[1] > p[2] + 40 ? 1 : p[2] > p[0] + 40 && p[2] > p[1] + 40 ? 2 : -1; };
        const auto red = pixel_at(-1, -1), green = pixel_at(1, -1), blue = pixel_at(1, 1), edge = pixel_at(9, 5);
        INFO("red " << red[0] << "," << red[1] << "," << red[2] << " green " << green[0] << "," << green[1] << "," << green[2] << " blue " << blue[0] << "," << blue[1] << "," << blue[2] << " edge " << edge[0] << "," << edge[1] << "," << edge[2]);
        REQUIRE(dominant(red) == 0);     // (i + j) % 3 == 0 at (-1, -1)
        REQUIRE(dominant(green) == 1);
        REQUIRE(dominant(blue) == 2);
        REQUIRE(dominant(edge) == ((14 + 12) % 3));   // near the corner of the view, a light is found as well
        // Past the budget: the farthest lights go and the nearest stay.
        for (int i = 0; i < 1000; ++i) {
            Json l;
            l["name"] = "M" + std::to_string(i);
            l["components"]["Transform"] = Json{{"position", Json{{"x", -8 + (i % 40) * 0.4}, {"y", 2}, {"z", -5 + (i / 40) * 0.4}}}};
            l["components"]["Light"] = Json{{"kind", 1}, {"intensity", 1}, {"range", 0.3}};
            spawn(l);
        }
        REQUIRE(s.frame().has_value());
        stats = s.command("render.stats", Json::object()).value();
        INFO(stats.dump());
        REQUIRE(stats["point_lights"] == 1024);
        REQUIRE(stats["lights"]["dropped"].get<int>() == visible + 1000 - 1024);
        REQUIRE(dominant(pixel_at(-1, -1)) == 0);   // the floor's lights under the middle of the view are among the nearest and stay
    }
    REQUIRE(s.finish().has_value());
}

TEST_CASE("point and spot lights cast shadows from the atlas when asked", "[renderer][lights][lightshadows]") {
    app::Options o = playground_options();
    o.width = 320;
    o.height = 180;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    const Json down{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}};
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 60}, {"y", 0.2}, {"z", 60}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(floor);
    // A block standing in the light's way: its shadow falls on the floor beyond it, toward +x.
    Json block;
    block["name"] = "Block";
    block["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 0.5}, {"z", 0}}}};
    block["components"]["MeshRenderer"] = Json{{"mesh", "cube"}};
    spawn(block);
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", down}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 0}};
    spawn(sun);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 12}, {"z", 0}}}, {"rotation", down}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 60}};
    spawn(camera);
    auto red_at = [&](float x, float z) {
        REQUIRE(s.frame().has_value());
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", 0}, {"z", z}}}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"][0].get<int>();
    };
    auto check = [&](Json light, int faces) {
        light["name"] = "Lamp";
        spawn(light);
        const int shade_off = red_at(1.1f, 0), open_off = red_at(-3, 1.5f);
        REQUIRE(s.command("world.set", Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"shadows", true}}}}).has_value());
        const int shade_on = red_at(1.1f, 0), open_on = red_at(-3, 1.5f);
        Json stats = s.command("render.stats", Json::object()).value();
        INFO("shade " << shade_off << "->" << shade_on << " open " << open_off << "->" << open_on << " " << stats["light_shadows"].dump());
        REQUIRE(stats["light_shadows"]["lights"] == 1);
        REQUIRE(stats["light_shadows"]["faces"] == faces);
        REQUIRE(shade_on < shade_off - 60);          // the floor behind the block goes dark
        REQUIRE(std::abs(open_on - open_off) < 6);   // open floor keeps its light: no acne, no leak
        // A mesh that casts no shadow lets the light through.
        REQUIRE(s.command("world.set", Json{{"entity", "Block"}, {"component", "MeshRenderer"}, {"value", Json{{"cast_shadows", false}}}}).has_value());
        REQUIRE(std::abs(red_at(1.1f, 0) - shade_off) < 6);
        REQUIRE(s.command("world.set", Json{{"entity", "Block"}, {"component", "MeshRenderer"}, {"value", Json{{"cast_shadows", true}}}}).has_value());
        REQUIRE(s.command("world.destroy", Json{{"entity", "Lamp"}}).has_value());
    };
    SECTION("a spot") {
        Json spot;
        spot["components"]["Transform"] = Json{{"position", Json{{"x", -2}, {"y", 4}, {"z", 0}}}, {"rotation", down}};
        spot["components"]["Light"] = Json{{"kind", 2}, {"intensity", 4}, {"range", 12}, {"inner_angle", 45}, {"outer_angle", 55}};
        check(spot, 1);
    }
    SECTION("a point light, a cube of six faces") {
        Json point;
        point["components"]["Transform"] = Json{{"position", Json{{"x", -2}, {"y", 3}, {"z", 0}}}};
        point["components"]["Light"] = Json{{"kind", 1}, {"intensity", 4}, {"range", 12}};
        check(point, 6);
    }
    SECTION("off with render.shadows") {
        REQUIRE(s.command("render.shadows", Json{{"enabled", false}}).has_value());
        Json point;
        point["name"] = "Lamp";
        point["components"]["Transform"] = Json{{"position", Json{{"x", -2}, {"y", 3}, {"z", 0}}}};
        point["components"]["Light"] = Json{{"kind", 1}, {"intensity", 4}, {"range", 12}, {"shadows", true}};
        spawn(point);
        REQUIRE(s.frame().has_value());
        REQUIRE(s.command("render.stats", Json::object()).value()["light_shadows"]["faces"] == 0);
    }
    REQUIRE(s.finish().has_value());
}

TEST_CASE("volumetric fog shows a spot's beam in the air, and a slab in the beam cuts a shaft of shadow", "[renderer][fog][volumetric]") {
    app::Options o = playground_options();
    o.width = 320;
    o.height = 180;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    const Json down{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}};
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", down}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 0}};
    spawn(sun);
    // A beam straight down, a slab over the right half of it halfway down.
    Json spot;
    spot["name"] = "Beam";
    spot["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 6}, {"z", 0}}}, {"rotation", down}};
    spot["components"]["Light"] = Json{{"kind", 2}, {"intensity", 6}, {"range", 12}, {"inner_angle", 12}, {"outer_angle", 15}, {"shadows", true}};
    spawn(spot);
    Json slab;
    slab["name"] = "Slab";
    slab["components"]["Transform"] = Json{{"position", Json{{"x", 1}, {"y", 3}, {"z", 0}}}, {"scale", Json{{"x", 2}, {"y", 0.1}, {"z", 2}}}};
    slab["components"]["MeshRenderer"] = Json{{"mesh", "cube"}};
    spawn(slab);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 2}, {"z", 8}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 50}};
    spawn(camera);
    Json fog;
    fog["name"] = "Mist";
    fog["components"]["Fog"] = Json{{"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}, {"density", 0.08}, {"falloff", 0.0}, {"volumetric", true}, {"anisotropy", 0.0}, {"steps", 64}, {"distance", 20}};
    spawn(fog);
    auto level_at = [&](float x, float y) {
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", 0}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        return (p[0].get<int>() + p[1].get<int>() + p[2].get<int>()) / 3;
    };
    REQUIRE(s.frame().has_value());
    const int lit = level_at(-0.25f, 1.5f), shaded = level_at(0.25f, 1.5f), outside = level_at(-1.5f, 1.5f), above = level_at(0.25f, 4.2f);
    Json stats = s.command("render.stats", Json::object()).value();
    INFO("lit " << lit << " shaded " << shaded << " outside " << outside << " above " << above << " " << stats.dump());
    REQUIRE(stats["volumetric"] == true);
    REQUIRE(lit > outside + 20);        // the beam shows in the air
    REQUIRE(shaded < lit - 15);         // under the slab the beam is cut
    REQUIRE(above > shaded + 10);       // over the slab the same side is lit
    // The plain fog shows no beam.
    REQUIRE(s.command("world.set", Json{{"entity", "Mist"}, {"component", "Fog"}, {"value", Json{{"volumetric", false}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const int lit_plain = level_at(-0.25f, 1.5f), outside_plain = level_at(-1.5f, 1.5f);
    INFO("plain lit " << lit_plain << " outside " << outside_plain);
    REQUIRE(s.command("render.stats", Json::object()).value()["volumetric"] == false);
    REQUIRE(std::abs(lit_plain - outside_plain) < 8);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("TAA smooths edges over a few frames, keeps flat areas and leaves no ghost behind a moving object", "[renderer][taa]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    // A glowing white card turned 20 degrees in front of a dark floor: without anti-aliasing each
    // pixel on its edges is either the card or the floor.
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 30}, {"y", 0.2}, {"z", 30}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0.1}, {"g", 0.1}, {"b", 0.12}, {"a", 1}}}};
    spawn(floor);
    Json card;
    card["name"] = "Card";
    card["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 0}}}, {"rotation", Json{{"x", 0}, {"y", 0}, {"z", 0.17364818}, {"w", 0.98480775}}}, {"scale", Json{{"x", 2.4}, {"y", 1.6}, {"z", 0.05}}}};
    card["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(card);
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", Json{{"x", -0.5}, {"y", -0.3}, {"z", 0}, {"w", 0.81}}}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 1.2}};
    spawn(sun);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 6}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 50}};
    spawn(camera);
    REQUIRE(s.command("render.shadows", Json{{"enabled", false}}).has_value());
    // Pixels in between: brighter than the darkest around the card and darker than the card.
    auto in_between = [&]() {
        auto img = s.device().capture();
        REQUIRE(img.has_value());
        int n = 0;
        for (std::uint32_t y = 0; y < img->height; ++y)
            for (std::uint32_t x = 0; x < img->width; ++x) {
                const std::uint8_t* a = &img->rgba[(y * img->width + x) * 4];
                if (a[0] > 90 && a[0] < 215) ++n;
            }
        return n;
    };
    auto level_at = [&](double x, double y, double z) {
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", z}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        return (p[0].get<int>() + p[1].get<int>() + p[2].get<int>()) / 3;
    };
    REQUIRE(s.frame().has_value());
    const int between_plain = in_between();
    const int floor_plain = level_at(0, 0, 2), card_plain = level_at(0, 1.5, 0.03);
    Json on = s.command("render.taa", Json{{"enabled", true}}).value();
    REQUIRE(on["enabled"] == true);
    for (int i = 0; i < 16; ++i) REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    const int between_taa = in_between();
    const int floor_taa = level_at(0, 0, 2), card_taa = level_at(0, 1.5, 0.03);
    INFO("pixels in between " << between_plain << " -> " << between_taa << ", floor " << floor_plain << " -> " << floor_taa << ", card " << card_plain << " -> " << card_taa << " " << stats.dump());
    REQUIRE(stats["taa"] == true);
    REQUIRE(stats["depth_prepass"] == true);
    REQUIRE(card_plain > 240);
    REQUIRE(between_plain < 20);                    // one sample: card or floor
    REQUIRE(between_taa > 60);                      // the edges spread over pixels partly covered (about half of the rim)
    REQUIRE(std::abs(floor_taa - floor_plain) < 5);   // a flat area stays what it was
    REQUIRE(std::abs(card_taa - card_plain) < 5);
    // A white block sliding over the dark floor: where it was a few frames ago is floor again, and
    // where it is now is white, not a smear.
    REQUIRE(s.command("world.destroy", Json{{"entity", "Card"}}).has_value());
    Json block;
    block["name"] = "Slider";
    block["components"]["Transform"] = Json{{"position", Json{{"x", -3}, {"y", 0.5}, {"z", 2}}}, {"scale", Json{{"x", 0.8}, {"y", 0.8}, {"z", 0.8}}}};
    block["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(block);
    double x = -3;
    for (int i = 0; i < 12; ++i) {
        x += 0.35;
        REQUIRE(s.command("world.set", Json{{"entity", "Slider"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", x}, {"y", 0.5}, {"z", 2}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
    }
    const int left_behind = level_at(x - 6 * 0.35, 0.02, 2), here = level_at(x, 0.5, 2.41);
    const int floor_there = level_at(x - 6 * 0.35, 0.02, 3.4);
    INFO("where it was " << left_behind << " (floor " << floor_there << "), where it is " << here);
    REQUIRE(std::abs(left_behind - floor_there) < 12);   // no ghost
    REQUIRE(here > 200);                                 // the block itself is still there, bright
    REQUIRE(s.command("render.taa", Json{{"enabled", false}}).value()["enabled"] == false);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["taa"] == false);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("depth of field blurs what is out of focus; motion blur smears what moves", "[renderer][dof][motion_blur]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 30}, {"y", 0.2}, {"z", 30}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0.1}, {"g", 0.1}, {"b", 0.12}, {"a", 1}}}};
    spawn(floor);
    Json card;
    card["name"] = "Card";
    card["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 0}}}, {"rotation", Json{{"x", 0}, {"y", 0}, {"z", 0.17364818}, {"w", 0.98480775}}}, {"scale", Json{{"x", 2.4}, {"y", 1.6}, {"z", 0.05}}}};
    card["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(card);
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", Json{{"x", -0.5}, {"y", -0.3}, {"z", 0}, {"w", 0.81}}}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 1.2}};
    spawn(sun);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 6}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 50}};
    spawn(camera);
    REQUIRE(s.command("render.shadows", Json{{"enabled", false}}).has_value());
    auto in_between = [&]() {
        auto img = s.device().capture();
        REQUIRE(img.has_value());
        int n = 0;
        for (std::uint32_t y = 0; y < img->height; ++y)
            for (std::uint32_t x = 0; x < img->width; ++x) {
                const std::uint8_t* a = &img->rgba[(y * img->width + x) * 4];
                if (a[0] > 90 && a[0] < 215) ++n;
            }
        return n;
    };
    // In focus on the card (six units off): its edges stay sharp. Focused a unit from the camera: blurred.
    Json d = s.command("render.dof", Json{{"enabled", true}, {"focus", 6.0}, {"aperture", 0.02}}).value();
    REQUIRE(d["enabled"] == true);
    REQUIRE(s.frame().has_value());
    const int sharp = in_between();
    REQUIRE(s.command("render.dof", Json{{"focus", 1.0}}).has_value());
    REQUIRE(s.frame().has_value());
    const int blurred = in_between();
    Json stats = s.command("render.stats", Json::object()).value();
    INFO("in between: focused " << sharp << ", out of focus " << blurred << " " << stats.dump());
    REQUIRE(stats["dof"] == true);
    REQUIRE(sharp < 20);
    REQUIRE(blurred > 150);
    REQUIRE(s.command("render.dof", Json{{"enabled", false}}).has_value());
    // A white block sliding over the floor: with motion blur it trails a smear, without it is crisp.
    REQUIRE(s.command("world.destroy", Json{{"entity", "Card"}}).has_value());
    Json block;
    block["name"] = "Slider";
    block["components"]["Transform"] = Json{{"position", Json{{"x", -3}, {"y", 0.5}, {"z", 2}}}, {"scale", Json{{"x", 0.8}, {"y", 0.8}, {"z", 0.8}}}};
    block["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(block);
    double x = -3;
    auto slide = [&](int frames) {
        for (int i = 0; i < frames; ++i) {
            x += 0.3;
            REQUIRE(s.command("world.set", Json{{"entity", "Slider"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", x}, {"y", 0.5}, {"z", 2}}}}}}).has_value());
            REQUIRE(s.frame().has_value());
        }
    };
    slide(3);
    const int crisp = in_between();
    REQUIRE(s.command("render.motion_blur", Json{{"enabled", true}, {"strength", 1.0}}).value()["enabled"] == true);
    slide(3);
    const int smeared = in_between();
    stats = s.command("render.stats", Json::object()).value();
    INFO("in between: still " << crisp << ", blurred " << smeared);
    REQUIRE(stats["motion_blur"] == true);
    REQUIRE(crisp < 20);
    REQUIRE(smeared > 60);
    // Standing still, nothing smears.
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(in_between() < 20);
    REQUIRE(s.finish().has_value());
}

namespace {
// A look-up table strip as an uncompressed TGA (N slices of N by N side by side), each texel the
// color `f` gives the color it stands for.
void write_lut(const std::filesystem::path& path, int n, auto f) {
    const int w = n * n, h = n;
    std::vector<std::uint8_t> bytes(18 + static_cast<std::size_t>(w * h * 4), 0);
    bytes[2] = 2;   // uncompressed true color
    bytes[12] = static_cast<std::uint8_t>(w & 0xFF); bytes[13] = static_cast<std::uint8_t>(w >> 8);
    bytes[14] = static_cast<std::uint8_t>(h & 0xFF); bytes[15] = static_cast<std::uint8_t>(h >> 8);
    bytes[16] = 32;
    bytes[17] = 0x28;   // 8 alpha bits, rows from the top
    for (int y = 0; y < h; ++y)
        for (int x = 0; x < w; ++x) {
            const float r = static_cast<float>(x % n) / static_cast<float>(n - 1), g = static_cast<float>(y) / static_cast<float>(n - 1), b = static_cast<float>(x / n) / static_cast<float>(n - 1);
            const std::array<float, 3> c = f(r, g, b);
            std::uint8_t* px = &bytes[18 + static_cast<std::size_t>((y * w + x) * 4)];
            px[0] = static_cast<std::uint8_t>(std::lround(std::clamp(c[2], 0.0f, 1.0f) * 255));
            px[1] = static_cast<std::uint8_t>(std::lround(std::clamp(c[1], 0.0f, 1.0f) * 255));
            px[2] = static_cast<std::uint8_t>(std::lround(std::clamp(c[0], 0.0f, 1.0f) * 255));
            px[3] = 255;
        }
    std::ofstream(path, std::ios::binary).write(reinterpret_cast<const char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
}
}  // namespace

TEST_CASE("a look-up table grades the finished frame: identity changes nothing, an inversion inverts, strength mixes", "[renderer][grade][lut]") {
    const std::filesystem::path dir = root() / "samples" / "playground" / "assets";
    std::filesystem::create_directories(dir);
    write_lut(dir / "lut-identity.tga", 16, [](float r, float g, float b) { return std::array<float, 3>{r, g, b}; });
    write_lut(dir / "lut-invert.tga", 16, [](float r, float g, float b) { return std::array<float, 3>{1 - r, 1 - g, 1 - b}; });
    {
        std::vector<std::uint8_t> bad(18 + 16 * 16 * 4, 255);
        std::fill(bad.begin(), bad.begin() + 18, 0);
        bad[2] = 2; bad[12] = 16; bad[14] = 16; bad[16] = 32; bad[17] = 0x28;
        std::ofstream(dir / "lut-square.tga", std::ios::binary).write(reinterpret_cast<const char*>(bad.data()), static_cast<std::streamsize>(bad.size()));
    }
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 30}, {"y", 0.2}, {"z", 30}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0.1}, {"g", 0.1}, {"b", 0.12}, {"a", 1}}}};
    spawn(floor);
    Json card;
    card["name"] = "Card";
    card["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 0}}}, {"scale", Json{{"x", 2.4}, {"y", 1.6}, {"z", 0.05}}}};
    card["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(card);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 6}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 50}};
    spawn(camera);
    auto level_at = [&](double x, double y, double z) {
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", z}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        return (p[0].get<int>() + p[1].get<int>() + p[2].get<int>()) / 3;
    };
    REQUIRE(s.frame().has_value());
    const int card0 = level_at(0, 1.5, 0.03), floor0 = level_at(0, 0, 2);
    auto grade = [&](Json p) {
        p["enabled"] = true;
        REQUIRE(s.command("render.grade", p).has_value());
        REQUIRE(s.frame().has_value());
    };
    grade(Json{{"lut", "assets/lut-identity.tga"}});
    REQUIRE(s.command("render.stats", Json::object()).value()["lut"] == true);
    const int card_id = level_at(0, 1.5, 0.03), floor_id = level_at(0, 0, 2);
    grade(Json{{"lut", "assets/lut-invert.tga"}});
    const int card_inv = level_at(0, 1.5, 0.03), floor_inv = level_at(0, 0, 2);
    grade(Json{{"lut_strength", 0.5}});
    const int card_half = level_at(0, 1.5, 0.03);
    INFO("card " << card0 << " identity " << card_id << " inverted " << card_inv << " half " << card_half << "; floor " << floor0 << " identity " << floor_id << " inverted " << floor_inv);
    REQUIRE(card0 > 240);
    REQUIRE(std::abs(card_id - card0) <= 3);
    REQUIRE(std::abs(floor_id - floor0) <= 3);
    REQUIRE(card_inv < 15);
    REQUIRE(std::abs(floor_inv - (255 - floor0)) <= 4);
    REQUIRE(std::abs(card_half - 128) <= 6);
    // A square picture is not a table: refused, reported, the frame left alone.
    grade(Json{{"lut", "assets/lut-square.tga"}, {"lut_strength", 1.0}});
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["lut"] == false);
    bool reported = false;
    for (const Json& m : stats["assets"].value("missing", Json::array())) if (m.get<std::string>().find("lut-square") != std::string::npos) reported = true;
    REQUIRE(reported);
    REQUIRE(std::abs(level_at(0, 1.5, 0.03) - card0) <= 3);
    REQUIRE(s.finish().has_value());
    for (const char* f : {"lut-identity.tga", "lut-invert.tga", "lut-square.tga"}) std::filesystem::remove(dir / f);
}

TEST_CASE("screen-space global illumination reddens the floor beside a glowing red wall and leaves the far floor", "[renderer][ssgi]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 40}, {"y", 0.2}, {"z", 40}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}, {"roughness", 0.9}};
    spawn(floor);
    Json wall;
    wall["name"] = "Wall";
    wall["components"]["Transform"] = Json{{"position", Json{{"x", -1.5}, {"y", 1.5}, {"z", 0}}}, {"scale", Json{{"x", 0.2}, {"y", 3}, {"z", 6}}}};
    wall["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 2}, {"g", 0}, {"b", 0}, {"a", 1}}}};
    spawn(wall);
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", Json{{"x", -0.5}, {"y", -0.3}, {"z", 0}, {"w", 0.81}}}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 0.6}};
    spawn(sun);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 1.5}, {"y", 2.5}, {"z", 5}}}, {"rotation", Json{{"x", -0.2}, {"y", 0.1}, {"z", 0}, {"w", 0.97}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 60}};
    spawn(camera);
    auto at = [&](double x, double z) {
        Json q = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", 0}, {"z", z}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", q["x"]}, {"y", q["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    const auto near_plain = at(-1.0, 0.5), far_plain = at(3.5, 0.5);
    Json on = s.command("render.ssgi", Json{{"enabled", true}}).value();
    REQUIRE(on["enabled"] == true);
    REQUIRE(on["rays"] == 2);
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    const auto near_lit = at(-1.0, 0.5), far_lit = at(3.5, 0.5);
    Json stats = s.command("render.stats", Json::object()).value();
    INFO("near " << near_plain[0] << "," << near_plain[1] << "," << near_plain[2] << " -> " << near_lit[0] << "," << near_lit[1] << "," << near_lit[2]
         << "; far " << far_plain[0] << "," << far_plain[1] << "," << far_plain[2] << " -> " << far_lit[0] << "," << far_lit[1] << "," << far_lit[2]);
    REQUIRE(stats["ssgi"] == true);
    REQUIRE(stats["depth_prepass"] == true);
    REQUIRE(near_lit[0] - near_lit[1] > near_plain[0] - near_plain[1] + 20);   // the floor by the wall reddens
    REQUIRE(std::abs(near_lit[1] - near_plain[1]) < 12);                       // red only
    REQUIRE(std::abs((far_lit[0] - far_lit[1]) - (far_plain[0] - far_plain[1])) < 12);   // the far floor much as it was
    // Off again: the bounce goes.
    REQUIRE(s.command("render.ssgi", Json{{"enabled", false}}).has_value());
    REQUIRE(s.frame().has_value());
    const auto near_off = at(-1.0, 0.5);
    REQUIRE(std::abs(near_off[0] - near_plain[0]) < 6);
    REQUIRE(s.command("render.stats", Json::object()).value()["ssgi"] == false);
}

TEST_CASE("screen-space reflections show what stands on a mirror floor where its mirror image falls", "[renderer][ssr]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 40}, {"y", 0.2}, {"z", 40}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}, {"metallic", 1.0}, {"roughness", 0.05}};
    spawn(floor);
    Json box;
    box["name"] = "Box";
    box["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 0.75}, {"z", -1}}}, {"scale", Json{{"x", 1.2}, {"y", 1.5}, {"z", 1.2}}}};
    box["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 0}, {"b", 0}, {"a", 1}}}};
    spawn(box);
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", Json{{"x", -0.5}, {"y", -0.3}, {"z", 0}, {"w", 0.81}}}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 1.0}};
    spawn(sun);
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 2.4}, {"z", 7}}}, {"rotation", Json{{"x", -0.14}, {"y", 0}, {"z", 0}, {"w", 0.99}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 55}};
    spawn(camera);
    REQUIRE(s.frame().has_value());
    // Where the box's front appears mirrored in the floor.
    Json mirrored = s.command("render.project", Json{{"point", Json{{"x", 0}, {"y", -0.75}, {"z", -0.4}}}}).value();
    auto pixel = [&]() {
        Json p = s.command("capture", Json{{"pixel", Json{{"x", mirrored["x"]}, {"y", mirrored["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    REQUIRE(s.frame().has_value());
    const auto plain = pixel();
    Json on = s.command("render.ssr", Json{{"enabled", true}}).value();
    REQUIRE(on["enabled"] == true);
    REQUIRE(s.frame().has_value());
    const auto traced = pixel();
    Json stats = s.command("render.stats", Json::object()).value();
    INFO("mirror image pixel " << plain[0] << "," << plain[1] << "," << plain[2] << " -> " << traced[0] << "," << traced[1] << "," << traced[2] << " " << stats.dump());
    REQUIRE(stats["ssr"] == true);
    REQUIRE(stats["depth_prepass"] == true);
    REQUIRE(plain[0] < 80);                        // the floor reflects only the dim ambient there
    REQUIRE(traced[0] > plain[0] + 80);            // with the box in it
    REQUIRE(traced[0] > traced[1] + 60);           // red
    // A rough floor keeps the sky's reflection.
    REQUIRE(s.command("world.set", Json{{"entity", "Floor"}, {"component", "MeshRenderer"}, {"value", Json{{"roughness", 0.9}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const auto rough = pixel();
    INFO("rough " << rough[0] << "," << rough[1] << "," << rough[2]);
    REQUIRE(rough[0] < traced[0] - 60);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a cut-out's holes let the sun through its shadow", "[renderer][shadows][cutout]") {
    const std::filesystem::path dir = root() / "samples" / "playground" / "assets";
    std::filesystem::create_directories(dir);
    {
        // Two texels: the left one solid, the right one clear.
        const std::uint8_t tga[18 + 8] = {0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1, 0, 32, 0x28, 200, 200, 200, 255, 200, 200, 200, 0};
        std::ofstream(dir / "half-clear.tga", std::ios::binary).write(reinterpret_cast<const char*>(tga), sizeof tga);
    }
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](Json e) { REQUIRE(s.command("world.spawn", e).has_value()); };
    Json floor;
    floor["name"] = "Floor";
    floor["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 30}, {"y", 0.2}, {"z", 30}}}};
    floor["components"]["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0.8}, {"g", 0.8}, {"b", 0.8}, {"a", 1}}}};
    spawn(floor);
    // A roof two units up, its texture solid on the left half and clear on the right, cut at 0.5.
    Json roof;
    roof["name"] = "Roof";
    roof["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 2}, {"z", 0}}}, {"scale", Json{{"x", 4}, {"y", 1}, {"z", 4}}}};
    roof["components"]["MeshRenderer"] = Json{{"mesh", "plane"}, {"texture", "assets/half-clear.tga"}, {"cutoff", 0.5}, {"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}};
    spawn(roof);
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", Json{{"x", -0.70710678}, {"y", 0}, {"z", 0}, {"w", 0.70710678}}}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 1.5}};
    spawn(sun);
    // Under the roof, looking along the floor.
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 6}}}, {"rotation", Json{{"x", -0.1}, {"y", 0}, {"z", 0}, {"w", 0.995}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 60}};
    spawn(camera);
    auto level_at = [&](double x, double z) {
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", 0}, {"z", z}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        return (p[0].get<int>() + p[1].get<int>() + p[2].get<int>()) / 3;
    };
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    const int under_solid = level_at(-1.2, 0.5), under_clear = level_at(1.2, 0.5);
    // Without a cutoff the roof is whole, in its shadow too.
    REQUIRE(s.command("world.set", Json{{"entity", "Roof"}, {"component", "MeshRenderer"}, {"value", Json{{"cutoff", 0.0}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const int whole_left = level_at(-1.2, 0.5), whole_right = level_at(1.2, 0.5);
    INFO("cut: under the solid half " << under_solid << ", under the clear half " << under_clear << "; whole: " << whole_left << ", " << whole_right);
    REQUIRE(under_clear > under_solid + 60);
    REQUIRE(std::abs(whole_left - whole_right) < 10);
    REQUIRE(whole_right < under_clear - 60);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(dir / "half-clear.tga");
}

TEST_CASE("vertex colors tint a mesh corner by corner", "[renderer][vcolor]") {
    const std::filesystem::path dir = root() / "samples" / "playground" / "assets";
    std::filesystem::create_directories(dir);
    // A quad facing +Z, its corners red, green, blue and white.
    std::ofstream(dir / "painted.gltf") << R"({"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}], "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1, "COLOR_0": 2}, "indices": 3}]}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [-1, -1, 0], "max": [1, 1, 0]},
                      {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3"},
                      {"bufferView": 2, "componentType": 5126, "count": 4, "type": "VEC3"},
                      {"bufferView": 3, "componentType": 5123, "count": 6, "type": "SCALAR"}],
        "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 48}, {"buffer": 0, "byteOffset": 48, "byteLength": 48}, {"buffer": 0, "byteOffset": 96, "byteLength": 48}, {"buffer": 0, "byteOffset": 144, "byteLength": 12}],
        "buffers": [{"byteLength": 156, "uri": "data:application/octet-stream;base64,AACAvwAAgL8AAAAAAACAPwAAgL8AAAAAAACAPwAAgD8AAAAAAACAvwAAgD8AAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AAAAAAAAAAAAAIA/AACAPwAAAAAAAAAAAAAAAAAAgD8AAAAAAAAAAAAAAAAAAIA/AACAPwAAgD8AAIA/AAABAAIAAAACAAMA"}]})";
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    Json quad;
    quad["name"] = "Painted";
    quad["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 0}}}};
    quad["components"]["MeshRenderer"] = Json{{"mesh", "assets/painted.gltf"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}};
    REQUIRE(s.command("world.spawn", quad).has_value());
    // Lit straight on by a white sun from the camera's side, so each corner shows its color.
    Json sun;
    sun["name"] = "Sun";
    sun["components"]["Transform"] = Json{{"rotation", Json{{"x", 0}, {"y", 0}, {"z", 0}, {"w", 1}}}};
    sun["components"]["Light"] = Json{{"kind", 0}, {"intensity", 1.0}};
    REQUIRE(s.command("world.spawn", sun).has_value());
    Json camera;
    camera["name"] = "Camera";
    camera["components"]["Transform"] = Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 4}}}};
    camera["components"]["Camera"] = Json{{"fov_degrees", 50}};
    REQUIRE(s.command("world.spawn", camera).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Painted"}, {"component", "MeshRenderer"}, {"value", Json{{"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    auto at = [&](double x, double y) {
        Json r = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", 0}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    const auto red = at(-0.9, -0.9), green = at(0.9, -0.9), blue = at(0.9, 0.9), white = at(-0.9, 0.9);
    INFO("red " << red[0] << "," << red[1] << "," << red[2] << " green " << green[0] << "," << green[1] << "," << green[2] << " blue " << blue[0] << "," << blue[1] << "," << blue[2] << " white " << white[0] << "," << white[1] << "," << white[2]);
    REQUIRE(red[0] > red[1] + 80);
    REQUIRE(red[0] > red[2] + 80);
    REQUIRE(green[1] > green[0] + 80);
    REQUIRE(blue[2] > blue[0] + 80);
    REQUIRE(std::abs(white[0] - white[2]) < 30);
    REQUIRE(white[0] > 120);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(dir / "painted.gltf");
}

TEST_CASE("a reflection probe makes a room's floor reflect the room, not the sky outside", "[renderer][probes]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto box = [&](const char* name, double x, double y, double z, double sx, double sy, double sz, Json mr) {
        mr["mesh"] = "cube";
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", z}}}, {"scale", Json{{"x", sx}, {"y", sy}, {"z", sz}}}}}, {"MeshRenderer", mr}}}}).has_value());
    };
    const Json gray{{"r", 0.6}, {"g", 0.6}, {"b", 0.62}, {"a", 1}};
    box("Floor", 0, -0.1, 0, 10, 0.2, 10, Json{{"color", Json{{"r", 0.9}, {"g", 0.9}, {"b", 0.9}, {"a", 1}}}, {"metallic", 1.0}, {"roughness", 0.05}});
    box("Far", 0, 1.5, -5.1, 10, 3, 0.2, Json{{"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 1}, {"g", 0.05}, {"b", 0.05}, {"a", 1}}}});
    box("Left", -5.1, 1.5, 0, 0.2, 3, 10, Json{{"color", gray}});
    box("Right", 5.1, 1.5, 0, 0.2, 3, 10, Json{{"color", gray}});
    box("Back", 0, 1.5, 5.1, 10, 3, 0.2, Json{{"color", gray}});
    box("Ceiling", 0, 3.1, 0, 10.4, 0.2, 10.4, Json{{"color", gray}});
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 1}, {"intensity", 1.5}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.5}, {"y", 0.3}, {"z", 0.1}, {"w", 0.8}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2.0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.6}, {"z", 4.2}}}, {"rotation", Json{{"x", -0.1}, {"y", 0}, {"z", 0}, {"w", 0.995}}}}}, {"Camera", Json{{"fov_degrees", 65}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    // The floor just in front of the red far wall, where its mirror image falls.
    Json at = s.command("render.project", Json{{"point", Json{{"x", 0}, {"y", 0}, {"z", -3.5}}}}).value();
    auto pixel = [&]() {
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    const auto sky = pixel();
    REQUIRE(s.command("world.spawn", Json{{"name", "RoomProbe"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 0}}}}}, {"ReflectionProbe", Json{{"size", Json{{"x", 10}, {"y", 3}, {"z", 10}}}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["probes"]["captured"] == 1);
    REQUIRE(stats["probes"]["in_use"] == 0);       // captured this frame, used from the next
    // Captured twice more, each lit by the one before (the light bouncing off the walls), then no more.
    for (int bounce = 2; bounce <= 3; ++bounce) {
        REQUIRE(s.frame().has_value());
        stats = s.command("render.stats", Json::object()).value();
        REQUIRE(stats["probes"]["in_use"] == 1);
        REQUIRE(stats["probes"]["captured"] == 1);
    }
    REQUIRE(s.frame().has_value());
    const auto room = pixel();
    stats = s.command("render.stats", Json::object()).value();
    INFO("floor before the red wall: sky " << sky[0] << "," << sky[1] << "," << sky[2] << " -> probe " << room[0] << "," << room[1] << "," << room[2] << " " << stats["probes"].dump());
    REQUIRE(stats["probes"]["in_use"] == 1);
    REQUIRE(stats["probes"]["captured"] == 0);     // enough while it stays
    REQUIRE(sky[2] >= sky[0]);                     // the sky's blue
    REQUIRE(room[0] > room[2] + 40);               // the red wall
    Json list = s.command("render.probes", Json::object()).value();
    INFO(list.dump());
    REQUIRE(list["probes"].size() == 1);
    REQUIRE(list["probes"][0]["captured"] == true);
    const auto first = list["probes"][0]["frame"].get<std::uint64_t>();
    // Asked, or moved, it is captured again.
    REQUIRE(s.command("render.probes", Json{{"refresh", true}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["probes"]["captured"] == 1);
    const auto second = s.command("render.probes", Json::object()).value()["probes"][0]["frame"].get<std::uint64_t>();
    REQUIRE(second > first);
    REQUIRE(s.command("world.set", Json{{"entity", "RoomProbe"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 0}, {"y", 1.4}, {"z", 0}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["probes"]["captured"] == 1);
    // Disabled, the sky's reflection is back.
    REQUIRE(s.command("world.set", Json{{"entity", "RoomProbe"}, {"component", "ReflectionProbe"}, {"value", Json{{"enabled", false}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const auto back = pixel();
    REQUIRE(std::abs(back[0] - sky[0]) < 8);
    REQUIRE(std::abs(back[2] - sky[2]) < 8);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("the showcase sample has every part of the renderer on at once", "[renderer][showcase]") {
    app::Options o;
    o.project_dir = root() / "samples" / "showcase";
    o.bundle = root() / "build" / "ts" / "showcase.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.width = 320;
    o.height = 180;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats.dump());
    REQUIRE(stats["sky"] == "procedural");
    REQUIRE(stats["shadow_cascades"].get<int>() == 4);
    REQUIRE(stats["point_lights"].get<int>() >= 10);
    REQUIRE(stats["spot_lights"] == 1);
    REQUIRE(stats["light_shadows"]["lights"].get<int>() >= 4);
    REQUIRE(stats["volumetric"] == true);
    REQUIRE(stats["taa"] == true);
    REQUIRE(stats["ssr"] == true);
    REQUIRE(stats["probes"]["in_use"] == 1);
    REQUIRE(stats["bloom"] == true);
    REQUIRE(stats["ao"] == true);
    REQUIRE(stats["tonemap"] == "agx");
    REQUIRE(stats["decals"]["drawn"].get<int>() >= 3);     // puddles, the sigil, the arrow (those in view)
    REQUIRE(stats["glass"] == 1);                          // the glass ball in the pavilion
    const Json dusk = s.command("world.get", Json{{"entity", "Dusk"}, {"component", "Timeline"}}).value();
    REQUIRE(dusk["error"] == "");                          // the dusk timeline plays every track
    REQUIRE(dusk["time"].get<double>() > 0.0);
    REQUIRE(stats["assets"].value("missing", Json::array()).empty());
    // The camera circles: its angle is exposed and moves.
    const double a0 = s.command("state", Json::object()).value()["state"]["camera.angle"].get<double>();
    for (int i = 0; i < 30; ++i) REQUIRE(s.frame().has_value());
    const double a1 = s.command("state", Json::object()).value()["state"]["camera.angle"].get<double>();
    REQUIRE(a1 > a0);
    REQUIRE(s.finish().has_value());
    REQUIRE(s.report()["ok"] == true);
}

TEST_CASE("a light's shadow faces draw only the casters it reaches", "[renderer][lightshadows][shadowcull]") {
    app::Options o = playground_options();
    o.width = 160;
    o.height = 90;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // Forty blocks three units apart along x; a lamp beside the first, reaching four units.
    for (int i = 0; i < 40; ++i) {
        REQUIRE(s.command("world.spawn", Json{{"name", "Block" + std::to_string(i)}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 3 * i}, {"y", 0.5}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}}}}).has_value());
    }
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 3}, {"z", 8}}}}}, {"Camera", Json{{"fov_degrees", 60}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Lamp"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 1}, {"y", 2}, {"z", 1}}}}}, {"Light", Json{{"kind", 1}, {"intensity", 2}, {"range", 4}, {"shadows", false}}}}}}).has_value());
    for (int i = 0; i < 2; ++i) REQUIRE(s.frame().has_value());   // the sun's cascades drawn, then kept
    const int without = s.command("render.stats", Json::object()).value()["shadow_instances"].get<int>();
    REQUIRE(s.command("world.set", Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"shadows", true}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json stats = s.command("render.stats", Json::object()).value();
    const int with = stats["shadow_instances"].get<int>();
    INFO("shadow instances without the lamp's shadows " << without << ", with " << with << " " << stats["light_shadows"].dump());
    REQUIRE(stats["light_shadows"]["faces"] == 6);
    // Six faces of the lamp, each drawing the two blocks within its reach, not all forty.
    REQUIRE(with - without > 0);
    REQUIRE(with - without <= 6 * 3);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("order-independent transparency blends interleaved translucent meshes alike whichever is drawn first", "[renderer][oit]") {
    // A thin red pane and a blue box around it share a center, so sorting by entity depth cannot
    // tell them apart: sorted blending draws them in the order they were made, OIT does not care.
    auto center = [&](bool blue_first, bool oit, int msaa = 1) {
        app::Options o = playground_options();
        o.width = 160;
        o.height = 90;
        app::Session s(o);
        REQUIRE(s.start().has_value());
        REQUIRE(s.command("world.clear", Json::object()).has_value());
        const Json pane{{"name", "Pane"}, {"components", Json{{"Transform", Json{{"scale", Json{{"x", 2}, {"y", 2}, {"z", 0.02}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 1}, {"g", 0}, {"b", 0}, {"a", 0.5}}}}}}}};
        const Json box{{"name", "Box"}, {"components", Json{{"Transform", Json{{"scale", Json{{"x", 1.2}, {"y", 1.2}, {"z", 0.4}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0}, {"g", 0}, {"b", 1}, {"a", 0.5}}}}}}}};
        REQUIRE(s.command("world.spawn", blue_first ? box : pane).has_value());
        REQUIRE(s.command("world.spawn", blue_first ? pane : box).has_value());
        REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 5}}}}}, {"Camera", Json{{"fov_degrees", 40}}}}}}).has_value());
        REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json::object()}, {"Light", Json{{"kind", 0}, {"intensity", 1.0}}}}}}).has_value());
        REQUIRE(s.command("render.oit", Json{{"enabled", oit}}).value()["enabled"] == oit);
        REQUIRE(s.command("render.msaa", Json{{"samples", msaa}}).value()["msaa"].get<int>() == msaa);
        REQUIRE(s.frame().has_value());
        const Json stats = s.command("render.stats", Json::object()).value();
        REQUIRE(stats["oit"] == oit);
        REQUIRE(stats["msaa"].get<int>() == msaa);
        Json r = s.command("render.project", Json{{"point", Json{{"x", 0}, {"y", 0}, {"z", 0}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", r["x"]}, {"y", r["y"]}}}}).value()["pixel"];
        REQUIRE(s.finish().has_value());
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    const auto sorted_a = center(false, false), sorted_b = center(true, false);
    const auto oit_a = center(false, true), oit_b = center(true, true);
    INFO("sorted " << sorted_a[0] << "," << sorted_a[2] << " vs " << sorted_b[0] << "," << sorted_b[2] << "; oit " << oit_a[0] << "," << oit_a[2] << " vs " << oit_b[0] << "," << oit_b[2]);
    // Sorted: which one ends on top depends on which was made first.
    REQUIRE(std::abs(sorted_a[0] - sorted_b[0]) + std::abs(sorted_a[2] - sorted_b[2]) > 30);
    // OIT: the same either way, and both colors in it, the nearer (blue) weighing more.
    REQUIRE(std::abs(oit_a[0] - oit_b[0]) <= 3);
    REQUIRE(std::abs(oit_a[2] - oit_b[2]) <= 3);
    REQUIRE(oit_a[0] > 40);
    REQUIRE(oit_a[2] > oit_a[0]);
    // With MSAA the same: accumulated at four samples and resolved before the composite.
    const auto ms_a = center(false, true, 4), ms_b = center(true, true, 4);
    INFO("oit with msaa " << ms_a[0] << "," << ms_a[2] << " vs " << ms_b[0] << "," << ms_b[2]);
    REQUIRE(std::abs(ms_a[0] - ms_b[0]) <= 3);
    REQUIRE(std::abs(ms_a[2] - ms_b[2]) <= 3);
    REQUIRE(std::abs(ms_a[0] - oit_a[0]) <= 3);
    REQUIRE(std::abs(ms_a[2] - oit_a[2]) <= 3);
}

TEST_CASE("a reflection probe lights a closed room from what it saw: its lamps, not the sky", "[renderer][probes][probediffuse]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto box = [&](const char* name, double x, double y, double z, double sx, double sy, double sz, Json mr) {
        mr["mesh"] = "cube";
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", z}}}, {"scale", Json{{"x", sx}, {"y", sy}, {"z", sz}}}}}, {"MeshRenderer", mr}}}}).has_value());
    };
    const Json gray{{"color", Json{{"r", 0.7}, {"g", 0.7}, {"b", 0.7}, {"a", 1}}}, {"roughness", 1.0}};
    box("Floor", 0, -0.1, 0, 10, 0.2, 10, gray);
    box("Far", 0, 1.5, -5.1, 10, 3, 0.2, gray);
    box("Left", -5.1, 1.5, 0, 0.2, 3, 10, gray);
    box("Right", 5.1, 1.5, 0, 0.2, 3, 10, gray);
    box("Back", 0, 1.5, 5.1, 10, 3, 0.2, gray);
    box("Ceiling", 0, 3.1, 0, 10.4, 0.2, 10.4, gray);
    // A white block in the room, its top facing the ceiling.
    box("Block", 2.5, 0.5, -2.5, 1, 1, 1, Json{{"color", Json{{"r", 1}, {"g", 1}, {"b", 1}, {"a", 1}}}, {"roughness", 1.0}});
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 1}, {"intensity", 1.5}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.5}, {"y", 0.3}, {"z", 0.1}, {"w", 0.8}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2.0}}}}}}).has_value());
    // A warm lamp in the far left corner whose reach stops well short of the block.
    REQUIRE(s.command("world.spawn", Json{{"name", "Lamp"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", -4}, {"y", 2.5}, {"z", 3.5}}}}}, {"Light", Json{{"kind", 1}, {"color", Json{{"r", 1}, {"g", 0.55}, {"b", 0.2}, {"a", 1}}}, {"intensity", 12}, {"range", 3.5}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 2.6}, {"z", 4.2}}}, {"rotation", Json{{"x", -0.2}, {"y", 0}, {"z", 0}, {"w", 0.98}}}}}, {"Camera", Json{{"fov_degrees", 65}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json at = s.command("render.project", Json{{"point", Json{{"x", 2.5}, {"y", 1.0}, {"z", -2.5}}}}).value();
    auto pixel = [&]() {
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    const auto sky = pixel();   // without a probe the block's top is lit by the sky above the roof
    // Shadows black, so what lights the room is only what the probe saw.
    REQUIRE(s.command("render.shadows", Json{{"strength", 1.0}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "RoomProbe"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 0}}}}}, {"ReflectionProbe", Json{{"size", Json{{"x", 10}, {"y", 3}, {"z", 10}}}}}}}}).has_value());
    for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.probes", Json::object()).value()["probes"][0]["bounces"] == 0);
    const auto lamp = pixel();
    // The lamp out: captured again, the room goes dark.
    REQUIRE(s.command("world.set", Json{{"entity", "Lamp"}, {"component", "Light"}, {"value", Json{{"intensity", 0}}}}).has_value());
    REQUIRE(s.command("render.probes", Json{{"refresh", true}}).has_value());
    for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    const auto dark = pixel();
    INFO("block top: sky " << sky[0] << "," << sky[1] << "," << sky[2] << "; probe with the lamp " << lamp[0] << "," << lamp[1] << "," << lamp[2] << "; without " << dark[0] << "," << dark[1] << "," << dark[2]);
    // Under the sky it is bright and cool; in the probe's light, lit by the lamp it saw, warm and
    // much dimmer (the sun and the sky do not get in).
    REQUIRE(sky[2] >= sky[0]);
    REQUIRE(lamp[0] + lamp[1] + lamp[2] < (sky[0] + sky[1] + sky[2]) / 2);
    REQUIRE(lamp[0] > lamp[2] + 5);
    // The lamp's light reached the block only through the capture: without it, far darker (what
    // is left is the lamp's light fading from the captures before, a bounce less each time).
    REQUIRE(3 * (dark[0] + dark[1] + dark[2]) < lamp[0] + lamp[1] + lamp[2]);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("an irradiance volume's probes beyond a wall do not light the room behind it", "[renderer][probes][irradiance][visibility]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto box = [&](const char* name, double x, double y, double z, double sx, double sy, double sz, Json mr) {
        mr["mesh"] = "cube";
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", z}}}, {"scale", Json{{"x", sx}, {"y", sy}, {"z", sz}}}}}, {"MeshRenderer", mr}}}}).has_value());
    };
    // Two closed rooms side by side, x -6..0 and 0..6, the wall between them whole; a white panel
    // glowing in the left one's ceiling is the only light, and one volume spans both.
    const Json gray{{"color", Json{{"r", 0.7}, {"g", 0.7}, {"b", 0.7}, {"a", 1}}}, {"roughness", 1.0}};
    box("Floor", 0, -0.1, 0, 12.4, 0.2, 6.4, gray);
    box("Ceiling", 0, 4.1, 0, 12.4, 0.2, 6.4, gray);
    box("Far", 0, 2, -3.1, 12, 4, 0.2, gray);
    box("Near", 0, 2, 3.1, 12, 4, 0.2, gray);
    box("Left", -6.1, 2, 0, 0.2, 4, 6, gray);
    box("Right", 6.1, 2, 0, 0.2, 4, 6, gray);
    box("Between", 0, 2, 0, 0.2, 4, 6, gray);
    box("Light", -3, 3.95, 0, 3, 0.1, 3, Json{{"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 3}, {"g", 3}, {"b", 3}, {"a", 1}}}, {"roughness", 1.0}});
    REQUIRE(s.command("render.ambient", Json{{"color", Json::array({0, 0, 0})}, {"intensity", 0}}).has_value());
    REQUIRE(s.command("render.tonemap", Json{{"auto_exposure", false}, {"exposure", 1.0}}).has_value());   // a dark room stays dark
    // A sun of no strength: without one the default key light shines in, dimmed by the shadows.
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json::object()}, {"Light", Json{{"kind", 0}, {"intensity", 0.0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Volume"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 2}, {"z", 0}}}}}, {"IrradianceVolume", Json{{"size", Json{{"x", 11.6}, {"y", 3.6}, {"z", 5.6}}}, {"probes", Json{{"x", 8}, {"y", 3}, {"z", 4}}}}}}}}).has_value());
    // Looking down into the dark room from under its ceiling.
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 2.5}, {"y", 3.6}, {"z", 0}}}, {"rotation", Json{{"x", -0.7071}, {"y", 0}, {"z", 0}, {"w", 0.7071}}}}}, {"Camera", Json{{"fov_degrees", 70}}}}}}).has_value());
    auto pixel_at = [&](double x, double y, double z) {
        Json at = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", z}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return p[0].get<int>() + p[1].get<int>() + p[2].get<int>();
    };
    REQUIRE(s.frame().has_value());
    for (int i = 0; i < 160 && s.command("render.probes", Json::object()).value()["volumes"][0]["passes"].get<int>() > 0; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.probes", Json::object()).value()["volumes"][0]["passes"] == 0);
    REQUIRE(s.frame().has_value());
    const int near_seen = pixel_at(0.5, 0, 0.5), far_seen = pixel_at(4.5, 0, 0.5);
    // The same probes weighed by nearness and facing alone: the left room's light comes through.
    REQUIRE(s.command("world.set", Json{{"entity", "Volume"}, {"component", "IrradianceVolume"}, {"value", Json{{"visibility", false}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const int near_leak = pixel_at(0.5, 0, 0.5), far_leak = pixel_at(4.5, 0, 0.5);
    INFO("by the wall " << near_seen << " (without visibility " << near_leak << "), across the room " << far_seen << " (" << far_leak << ")");
    REQUIRE(near_leak > 60);                    // the light that leaked through the wall
    REQUIRE(near_seen < near_leak / 3);         // most of it stopped
    REQUIRE(near_seen <= far_seen + 30);        // the floor by the wall no brighter than across the room
    REQUIRE(far_seen <= far_leak);
}

TEST_CASE("an irradiance volume's probes light each part of a room by what is near it", "[renderer][probes][irradiance]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto box = [&](const char* name, double x, double y, double z, double sx, double sy, double sz, Json mr) {
        mr["mesh"] = "cube";
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", z}}}, {"scale", Json{{"x", sx}, {"y", sy}, {"z", sz}}}}}, {"MeshRenderer", mr}}}}).has_value());
    };
    const Json gray{{"color", Json{{"r", 0.7}, {"g", 0.7}, {"b", 0.7}, {"a", 1}}}, {"roughness", 1.0}};
    box("Floor", 0, -0.1, 0, 10, 0.2, 10, gray);
    box("Far", 0, 1.5, -5.1, 10, 3, 0.2, gray);
    box("Left", -5.1, 1.5, 0, 0.2, 3, 10, gray);
    box("Right", 5.1, 1.5, 0, 0.2, 3, 10, gray);
    box("Back", 0, 1.5, 5.1, 10, 3, 0.2, gray);
    box("Ceiling", 0, 3.1, 0, 10.4, 0.2, 10.4, gray);
    // A red glowing panel along the left wall and a white one in the ceiling: the only light inside.
    box("RedPanel", -4.95, 1.4, 0, 0.1, 2.2, 7, Json{{"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 3}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"roughness", 1.0}});
    box("Light", 0, 2.95, 0, 3, 0.1, 3, Json{{"color", Json{{"r", 0}, {"g", 0}, {"b", 0}, {"a", 1}}}, {"emissive", Json{{"r", 2}, {"g", 2}, {"b", 2}, {"a", 1}}}, {"roughness", 1.0}});
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 1}, {"intensity", 1.5}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.5}, {"y", 0.3}, {"z", 0.1}, {"w", 0.8}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2.0}}}}}}).has_value());
    REQUIRE(s.command("render.shadows", Json{{"strength", 1.0}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 2.6}, {"z", 4.6}}}, {"rotation", Json{{"x", -0.26}, {"y", 0}, {"z", 0}, {"w", 0.966}}}}}, {"Camera", Json{{"fov_degrees", 80}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    auto pixel_at = [&](double x, double y, double z) {
        Json at = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", z}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    auto redness = [](std::array<int, 3> c) { return static_cast<double>(c[0] + 1) / static_cast<double>(c[1] + c[2] + 2); };
    // One reflection probe for the room: its light is the same everywhere in it.
    REQUIRE(s.command("world.spawn", Json{{"name", "RoomProbe"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 0}}}}}, {"ReflectionProbe", Json{{"size", Json{{"x", 10}, {"y", 3}, {"z", 10}}}}}}}}).has_value());
    for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    const auto probe_left = pixel_at(-3.8, 0, 1), probe_right = pixel_at(3.8, 0, 1);
    // A volume of probes, six across, two up, six deep: two captured a frame, three passes.
    REQUIRE(s.command("world.spawn", Json{{"name", "RoomLight"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 0}}}}}, {"IrradianceVolume", Json{{"size", Json{{"x", 9.6}, {"y", 2.6}, {"z", 9.6}}}, {"probes", Json{{"x", 6}, {"y", 2}, {"z", 6}}}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    Json listed = s.command("render.probes", Json::object()).value();
    REQUIRE(listed["volumes"].size() == 1);
    REQUIRE(listed["volumes"][0]["ready"] == false);
    REQUIRE(s.command("render.stats", Json::object()).value()["probes"]["volume_captures"] == 2);
    for (int i = 0; i < 36; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.probes", Json::object()).value()["volumes"][0]["ready"] == true);
    for (int i = 0; i < 80; ++i) REQUIRE(s.frame().has_value());
    listed = s.command("render.probes", Json::object()).value();
    REQUIRE(listed["volumes"][0]["passes"] == 0);
    REQUIRE(s.command("render.stats", Json::object()).value()["probes"]["volumes"] == 1);
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["probes"]["volume_captures"] == 0);   // done: nothing captured
    const auto grid_left = pixel_at(-3.8, 0, 1), grid_right = pixel_at(3.8, 0, 1);
    INFO("floor by the red panel / across the room: one probe " << probe_left[0] << "," << probe_left[1] << "," << probe_left[2] << " / " << probe_right[0] << "," << probe_right[1] << "," << probe_right[2]
         << "; the volume " << grid_left[0] << "," << grid_left[1] << "," << grid_left[2] << " / " << grid_right[0] << "," << grid_right[1] << "," << grid_right[2]);
    // One probe tints the floor alike on both sides; the volume reddens the side by the panel.
    REQUIRE(std::abs(redness(probe_left) - redness(probe_right)) < 0.15);
    REQUIRE(redness(grid_left) > redness(grid_right) + 0.1);
    REQUIRE(grid_left[0] > grid_right[0] + 20);
    // Switched off, the reflection probe's light again.
    REQUIRE(s.command("world.set", Json{{"entity", "RoomLight"}, {"component", "IrradianceVolume"}, {"value", Json{{"enabled", false}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const auto off_left = pixel_at(-3.8, 0, 1);
    REQUIRE(std::abs(off_left[0] - probe_left[0]) <= 3);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("water shows the bed through it, deep water its colour, the sky at a glance, and is picked", "[renderer][water]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto box = [&](const char* name, double x, double y, double z, double sx, double sy, double sz) {
        REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", y}, {"z", z}}}, {"scale", Json{{"x", sx}, {"y", sy}, {"z", sz}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0.85}, {"g", 0.75}, {"b", 0.55}, {"a", 1}}}, {"roughness", 1.0}}}}}}).has_value());
    };
    box("Bed", 0, -3, 0, 40, 2, 40);          // sand two units under the surface
    box("Shelf", 5, -1.2, 0, 6, 2, 10);       // and a shelf a fifth of a unit under it
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 1}, {"intensity", 1.2}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.5}, {"y", 0.3}, {"z", 0.1}, {"w", 0.8}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2.0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Lake"}, {"components", Json{{"Transform", Json::object()}, {"Water", Json{{"size", Json{{"x", 30}, {"y", 30}}}, {"wave_height", 0}, {"ripples", 0}, {"foam", 0}, {"clarity", 2.0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json::object()}, {"Camera", Json{{"fov_degrees", 60}}}}}}).has_value());
    auto camera = [&](double y, double z, double pitch_x, double pitch_w) {
        Json t{{"position", Json{{"x", 0}, {"y", y}, {"z", z}}}, {"rotation", Json{{"x", pitch_x}, {"y", 0}, {"z", 0}, {"w", pitch_w}}}};
        REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", t}}).has_value());
        REQUIRE(s.frame().has_value());
    };
    auto pixel_at = [&](double x, double y, double z) {
        Json at = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", z}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    auto set_water = [&](Json v) { REQUIRE(s.command("world.set", Json{{"entity", "Lake"}, {"component", "Water"}, {"value", v}}).has_value()); REQUIRE(s.frame().has_value()); };
    // From above, looking down at 45 degrees.
    camera(6, 6, -0.3827, 0.9239);
    Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["water"]["bodies"] == 1);
    REQUIRE(stats["water"]["underwater"] == false);
    const auto deep = pixel_at(-3, -2, 0);
    const auto shallow = pixel_at(5, -0.2, 0);
    // The water is picked where it covers the bed.
    Json at = s.command("render.project", Json{{"point", Json{{"x", -3}, {"y", 0}, {"z", 0}}}}).value();
    REQUIRE(s.command("render.pick", Json{{"x", at["x"]}, {"y", at["y"]}}).value()["path"] == "/Lake");
    set_water(Json{{"enabled", false}});
    const auto deep_dry = pixel_at(-3, -2, 0);
    const auto shallow_dry = pixel_at(5, -0.2, 0);
    INFO("deep " << deep[0] << "," << deep[1] << "," << deep[2] << " (dry " << deep_dry[0] << "," << deep_dry[1] << "," << deep_dry[2] << "); shallow " << shallow[0] << "," << shallow[1] << "," << shallow[2] << " (dry " << shallow_dry[0] << "," << shallow_dry[1] << "," << shallow_dry[2] << ")");
    // Sand under two units of water that clears at two: mostly the water's blue-green, the sand's red gone.
    REQUIRE(deep[0] * 2 < deep_dry[0]);
    REQUIRE(deep[2] > deep[0]);
    // Under a fifth of a unit it still shows, a little dimmer and cooler.
    REQUIRE(shallow[0] > deep[0] + 40);
    REQUIRE(shallow[0] <= shallow_dry[0]);
    REQUIRE(shallow[0] * 10 > shallow_dry[0] * 6);
    // Nearly level, the far water mirrors the sky instead of showing the sand.
    set_water(Json{{"enabled", true}});
    camera(0.6, 12, -0.02, 0.9998);
    const auto far = pixel_at(0, 0, -12);
    set_water(Json{{"enabled", false}});
    const auto far_dry = pixel_at(0, -2, -12);
    INFO("far " << far[0] << "," << far[1] << "," << far[2] << " (dry " << far_dry[0] << "," << far_dry[1] << "," << far_dry[2] << ")");
    REQUIRE(far[2] > far[0] + 20);
    REQUIRE(far_dry[0] > far_dry[2]);
    // Under the surface: everything seen through the water.
    set_water(Json{{"enabled", true}});
    camera(-1, 6, -0.3827, 0.9239);
    stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["water"]["underwater"] == true);
    const auto sunk = pixel_at(-3, -2, 0);
    INFO("under the surface " << sunk[0] << "," << sunk[1] << "," << sunk[2]);
    REQUIRE(sunk[2] > sunk[0]);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("the weather: wet surfaces darken, snow whitens what faces up, rain and snow are drawn, overcast dims the sun", "[renderer][weather]") {
    // The scene stands 200 units along x from the assets sample's, seen by its camera, lit by its sun.
    app::Session s(assets_options());
    REQUIRE(s.start().has_value());
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", components}}).has_value()); };
    auto xyz = [](double x, double y, double z) { return Json{{"x", x + 200}, {"y", y}, {"z", z}}; };
    auto v3 = [](double x, double y, double z) { return Json{{"x", x}, {"y", y}, {"z", z}}; };
    auto rgba = [](double r, double g, double b, double a = 1) { return Json{{"r", r}, {"g", g}, {"b", b}, {"a", a}}; };
    spawn("Floor", Json{{"Transform", Json{{"position", xyz(0, -0.1, 0)}, {"scale", v3(20, 0.2, 20)}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", rgba(0.5, 0.45, 0.4)}, {"roughness", 1.0}}}});
    spawn("Block", Json{{"Transform", Json{{"position", xyz(0.55, 0.4, 0.55)}, {"scale", v3(0.8, 0.8, 0.8)}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", rgba(0.3, 0.5, 0.3)}, {"roughness", 1.0}}}});
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"x", -0.6}, {"y", 0.2}, {"z", 0.1}, {"w", 0.77}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", xyz(1.5, 4, 4)}, {"rotation", Json{{"x", -0.34}, {"y", 0.12}, {"z", 0.04}, {"w", 0.93}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Camera"}, {"value", Json{{"fov_degrees", 50}}}}).has_value());
    auto pixel_at = [&](double x, double y, double z) {
        Json at = s.command("render.project", Json{{"point", xyz(x, y, z)}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    auto weather = [&](Json value) {
        REQUIRE(s.command("world.set", Json{{"entity", "Weather"}, {"component", "Weather"}, {"value", value}}).has_value());
        REQUIRE(s.frame().has_value());
    };
    REQUIRE(s.frame().has_value());
    const auto dry = pixel_at(-1.0, 0, -0.5);
    const auto dry_side = pixel_at(0.55, 0.4, 0.95);   // the block's face toward the camera
    const double sun_dry = s.command("render.stats", Json::object()).value()["sun_light"]["r"].get<double>();
    spawn("Weather", Json{{"Weather", Json{{"wet", 1.0}}}});
    REQUIRE(s.frame().has_value());
    const auto wet = pixel_at(-1.0, 0, -0.5);
    weather(Json{{"wet", 0.0}, {"cover", 1.0}});
    const auto white = pixel_at(-1.0, 0, -0.5);
    const auto white_side = pixel_at(0.55, 0.4, 0.95);
    const auto white_top = pixel_at(0.55, 0.8, 0.55);
    REQUIRE(s.command("capture", Json{{"path", (root() / "build" / "test-out" / "weather-snow.png").string()}}).has_value());
    INFO("dry " << dry[0] << "," << dry[1] << "," << dry[2] << " wet " << wet[0] << "," << wet[1] << "," << wet[2] << " white " << white[0] << "," << white[1] << "," << white[2]
         << " side " << dry_side[1] << " -> " << white_side[1] << " top " << white_top[0] << "," << white_top[1] << "," << white_top[2]);
    REQUIRE(wet[0] < dry[0] * 0.85);                                // wet: darker
    REQUIRE(white[2] > dry[2] + 50);                                // snow lying: white on the floor
    REQUIRE(std::abs(white[0] - white[2]) < 30);
    REQUIRE(white_top[0] > white_side[0] + 30);                     // and on the block's top, not its side
    REQUIRE(std::abs(white_side[1] - dry_side[1]) < 20);
    // Under a roof (high over the floor, above the camera and out of its view) none lies, nor is the
    // floor wet: the shelter map, drawn from above, knows what stands over each spot.
    REQUIRE(s.command("render.shadows", Json{{"enabled", false}}).has_value());   // its sun shadow elsewhere on the floor would muddy the open spot
    spawn("Roof", Json{{"Transform", Json{{"position", xyz(-1.0, 6, -0.5)}, {"scale", v3(1.2, 0.1, 1.2)}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}});
    REQUIRE(s.frame().has_value());
    const auto roofed = pixel_at(-1.0, 0, -0.5);
    const auto open = pixel_at(-2.4, 0, -0.5);
    INFO("under the roof " << roofed[0] << "," << roofed[1] << "," << roofed[2] << " in the open " << open[0] << "," << open[1] << "," << open[2]);
    REQUIRE(s.command("render.stats", Json::object()).value()["shelter_draws"].get<int>() >= 1);
    REQUIRE(open[2] > dry[2] + 50);                                 // white in the open
    REQUIRE(std::abs(roofed[2] - dry[2]) < 20);                     // bare under the roof
    weather(Json{{"cover", 0.0}, {"wet", 1.0}});
    const auto roofed_wet = pixel_at(-1.0, 0, -0.5);
    INFO("under the roof in the wet " << roofed_wet[0]);
    REQUIRE(std::abs(roofed_wet[0] - dry[0]) < 15);                 // and dry
    REQUIRE(s.command("world.destroy", Json{{"entity", "Roof"}}).has_value());
    REQUIRE(s.command("render.shadows", Json{{"enabled", true}}).has_value());
    weather(Json{{"cover", 1.0}, {"wet", 0.0}});
    // Rain and snow are drawn about the camera, as many as they are hard (and the density says).
    weather(Json{{"cover", 0.0}, {"rain", 1.0}, {"snow", 0.5}});
    Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["weather_drops"] == 9000 + 3500);
    REQUIRE(s.command("capture", Json{{"path", (root() / "build" / "test-out" / "weather-rain.png").string()}}).has_value());
    weather(Json{{"density", 0.5}});
    REQUIRE(s.command("render.stats", Json::object()).value()["weather_drops"] == 4500 + 1750);
    // Overcast follows the weather (seven tenths of the rain here) and dims the sun's direct light.
    const double sun_wet = s.command("render.stats", Json::object()).value()["sun_light"]["r"].get<double>();
    INFO("sun " << sun_dry << " -> " << sun_wet);
    REQUIRE(sun_wet == Catch::Approx(sun_dry * (1 - 0.8 * 0.7)).epsilon(0.02));
    weather(Json{{"overcast", 0.0}});
    REQUIRE(s.command("render.stats", Json::object()).value()["sun_light"]["r"].get<double>() == Catch::Approx(sun_dry).epsilon(0.02));
}

TEST_CASE("decals paint the surfaces in their boxes, facing the projection, in order, and glow", "[renderer][decals]") {
    // The assets project for its checker image; the test's scene stands 200 units along x from the
    // sample's (whose script keeps running), seen by the sample's camera and lit by its sun.
    app::Session s(assets_options());
    REQUIRE(s.start().has_value());
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", components}}).has_value()); };
    auto xyz = [](double x, double y, double z) { return Json{{"x", x + 200}, {"y", y}, {"z", z}}; };   // a place
    auto v3 = [](double x, double y, double z) { return Json{{"x", x}, {"y", y}, {"z", z}}; };           // a size
    auto rgba = [](double r, double g, double b, double a = 1) { return Json{{"r", r}, {"g", g}, {"b", b}, {"a", a}}; };
    spawn("Floor", Json{{"Transform", Json{{"position", xyz(0, -0.1, 0)}, {"scale", v3(20, 0.2, 20)}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", rgba(0.5, 0.5, 0.5)}, {"roughness", 1.0}}}});
    // A white block standing in the decal's box: its top is painted, its sides are not.
    spawn("Block", Json{{"Transform", Json{{"position", xyz(0.55, 0.2, 0.55)}, {"scale", v3(0.4, 0.4, 0.4)}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", rgba(1, 1, 1)}, {"roughness", 1.0}}}});
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"x", -0.6}, {"y", 0.2}, {"z", 0.1}, {"w", 0.77}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Light"}, {"value", Json{{"intensity", 1.5}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", xyz(1.5, 4, 4)}, {"rotation", Json{{"x", -0.34}, {"y", 0.12}, {"z", 0.04}, {"w", 0.93}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Camera"}, {"value", Json{{"fov_degrees", 50}}}}).has_value());
    spawn("Mark", Json{{"Transform", Json{{"position", xyz(0, 0.5, 0)}}}, {"Decal", Json{{"color", rgba(1, 0.1, 0.1)}, {"size", v3(2, 1.2, 2)}}}});
    REQUIRE(s.frame().has_value());
    auto pixel_at = [&](double x, double y, double z) {
        Json at = s.command("render.project", Json{{"point", xyz(x, y, z)}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    Json stats = s.command("render.stats", Json::object()).value();
    INFO(stats["decals"].dump());
    REQUIRE(s.command("capture", Json{{"path", (root() / "build" / "test-out" / "decals.png").string()}}).has_value());
    REQUIRE(stats["decals"]["drawn"] == 1);
    const auto centre = pixel_at(-0.2, 0, -0.2);
    const auto outside = pixel_at(-1.6, 0, -0.2);
    const auto top = pixel_at(0.55, 0.4, 0.55);
    const auto side = pixel_at(0.55, 0.2, 0.75);   // the face toward +z, the camera's side
    INFO("centre " << centre[0] << "," << centre[1] << "," << centre[2] << "; outside " << outside[0] << "," << outside[1] << "," << outside[2] << "; top " << top[0] << "," << top[1] << "," << top[2] << "; side " << side[0] << "," << side[1] << "," << side[2]);
    REQUIRE(centre[0] > centre[1] + 60);                       // red on the floor
    REQUIRE(std::abs(outside[0] - outside[1]) < 12);           // grey beyond the box
    REQUIRE(top[0] > top[1] + 60);                             // red on the block's top
    REQUIRE(std::abs(side[0] - side[1]) < 12);                 // its side, turned away from the projection, left white
    // A blue decal over it paints over the red when its order is higher, under it when lower.
    spawn("Over", Json{{"Transform", Json{{"position", xyz(0, 0.5, 0)}}}, {"Decal", Json{{"color", rgba(0.1, 0.2, 1)}, {"size", v3(1.6, 1.2, 1.6)}, {"order", 1}}}});
    REQUIRE(s.frame().has_value());
    const auto over = pixel_at(-0.2, 0, -0.2);
    REQUIRE(over[2] > over[0] + 60);
    REQUIRE(s.command("world.set", Json{{"entity", "Over"}, {"component", "Decal"}, {"value", Json{{"order", -1}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const auto under = pixel_at(-0.2, 0, -0.2);
    REQUIRE(under[0] > under[2] + 60);
    // With the sun out, a glowing decal still shows.
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Light"}, {"value", Json{{"intensity", 0}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Mark"}, {"component", "Decal"}, {"value", Json{{"emissive", 2.0}, {"color", rgba(0.1, 1, 0.2)}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const auto glow = pixel_at(-0.2, 0, -0.2);
    const auto dark = pixel_at(-1.6, 0, -0.2);
    INFO("glow " << glow[0] << "," << glow[1] << "," << glow[2] << "; dark " << dark[0] << "," << dark[1] << "," << dark[2]);
    REQUIRE(glow[1] > dark[1] + 80);
    // An image: the checker's two colours across the box; one image loaded.
    REQUIRE(s.command("world.set", Json{{"entity", "Mark"}, {"component", "Decal"}, {"value", Json{{"texture", "assets/checker.png"}, {"emissive", 0.0}, {"color", rgba(1, 1, 1)}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Light"}, {"value", Json{{"intensity", 1.5}}}}).has_value());
    REQUIRE(s.command("world.destroy", Json{{"entity", "Over"}}).has_value());
    REQUIRE(s.frame().has_value());
    stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats["decals"]["images"] == 1);
    int lo = 255, hi = 0;
    for (int i = 0; i < 8; ++i) {
        const auto p = pixel_at(-0.85 + i * 0.1, 0, -0.5);
        lo = std::min(lo, p[0] + p[1] + p[2]);
        hi = std::max(hi, p[0] + p[1] + p[2]);
    }
    INFO("checker brightness from " << lo << " to " << hi);
    REQUIRE(hi - lo > 90);
    // Out of view, it is not painted.
    REQUIRE(s.command("world.set", Json{{"entity", "Mark"}, {"component", "Transform"}, {"value", Json{{"position", xyz(0, 0.5, 60)}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.stats", Json::object()).value()["decals"]["drawn"] == 0);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("scattered copies sway on the simulation clock and fade out past their distance", "[renderer][scatter][sway]") {
    const std::filesystem::path ref = root() / "samples" / "playground" / ".pocket" / "test-sway.png";
    std::filesystem::remove(ref);
    app::Options o = playground_options();
    o.width = 192;
    o.height = 128;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Floor"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", -0.5}, {"z", 0}}}, {"scale", Json{{"x", 20}, {"y", 1}, {"z", 20}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0.3}, {"g", 0.5}, {"b", 0.25}, {"a", 1}}}}}, {"RigidBody", Json{{"kind", 1}}}, {"Collider", Json{{"shape", 0}, {"size", Json{{"x", 10}, {"y", 0.5}, {"z", 10}}}}}}}}).has_value());
    // Tall thin reeds over the floor (the Scatter finds it with a ray down), the camera 7 away.
    REQUIRE(s.command("world.spawn", Json{{"name", "Reeds"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 3}, {"z", 0}}}, {"scale", Json{{"x", 0.08}, {"y", 1.6}, {"z", 0.08}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0.9}, {"g", 0.85}, {"b", 0.4}, {"a", 1}}}}},
        {"Scatter", Json{{"count", 300}, {"area", Json{{"x", 8}, {"y", 8}}}, {"seed", 2}, {"spacing", 0.3}, {"scale", Json{{"x", 0.8}, {"y", 1.2}}}, {"sway", 0.5}, {"sway_speed", 0.8}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 7}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.38}, {"y", 0.3}, {"z", 0.1}, {"w", 0.87}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const int placed = s.command("scatter.copies", Json{{"entity", "Reeds"}}).value()["placed"].get<int>();
    REQUIRE(placed > 100);
    REQUIRE(s.command("render.stats", Json::object()).value()["scattered"] == placed);
    // Swaying: a third of a second later the same view differs where the reeds are.
    auto moved = [&](int ticks, bool idle = false) {
        REQUIRE(s.command("render.compare", Json{{"path", ".pocket/test-sway.png"}, {"update", true}}).has_value());
        for (int i = 0; i < ticks; ++i) REQUIRE((idle ? s.idle_frame() : s.frame()).has_value());
        return s.command("render.compare", Json{{"path", ".pocket/test-sway.png"}}).value()["fraction"].get<double>();
    };
    const double swaying = moved(20);
    INFO("swaying " << swaying);
    REQUIRE(swaying > 0.01);
    // Paused (frames drawn, no tick), the clock stands and so do they.
    s.set_paused(true);
    REQUIRE(moved(20, true) == 0.0);
    s.set_paused(false);
    // Without sway, time passing changes nothing.
    Json sc = s.command("world.get", Json{{"entity", "Reeds"}, {"component", "Scatter"}}).value();
    sc["sway"] = 0;
    REQUIRE(s.command("world.set", Json{{"entity", "Reeds"}, {"component", "Scatter"}, {"value", sc}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(moved(20) == 0.0);
    // Fading: past 7.5 units from the camera none are drawn, the nearer still are.
    sc["fade"] = 7.5;
    REQUIRE(s.command("world.set", Json{{"entity", "Reeds"}, {"component", "Scatter"}, {"value", sc}}).has_value());
    REQUIRE(s.frame().has_value());
    const int drawn = s.command("render.stats", Json::object()).value()["scattered"].get<int>();
    INFO("drawn " << drawn << " of " << placed);
    REQUIRE(drawn > 0);
    REQUIRE(drawn < placed);
    const Json copies = s.command("scatter.copies", Json{{"entity", "Reeds"}, {"limit", 1000}}).value()["copies"];
    int near = 0;
    for (const Json& c : copies) near += std::hypot(c["x"].get<double>(), c["y"].get<double>() - 1.5, c["z"].get<double>() - 7) < 7.5 ? 1 : 0;
    REQUIRE(drawn == near);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(ref);
}

TEST_CASE("a Wind carries dragged particles, answers wind.at, and leans swaying copies downwind", "[renderer][wind]") {
    app::Options o = playground_options();
    o.width = 192;
    o.height = 128;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    // No Wind: still air.
    Json air = s.command("wind.at", Json{{"x", 3}, {"z", 4}}).value();
    REQUIRE(air["entity"].is_null());
    REQUIRE(air["speed"].get<double>() == 0);
    REQUIRE(s.command("world.spawn", Json{{"name", "Breeze"}, {"components", Json{{"Wind", Json{{"direction", 0}, {"speed", 5}, {"gusts", 0}}}}}}).has_value());
    air = s.command("wind.at", Json{{"x", 3}, {"z", 4}}).value();
    REQUIRE(air["path"] == "/Breeze");
    REQUIRE(air["velocity"]["x"].get<double>() == Catch::Approx(5).margin(1e-4));
    REQUIRE(air["direction"]["z"].get<double>() == Catch::Approx(0).margin(1e-4));
    // Smoke with drag 3 and no gravity, puffed out still: carried at the wind's speed.
    REQUIRE(s.command("world.spawn", Json{{"name", "Smoke"}, {"components", Json{{"Transform", Json::object()}, {"ParticleEmitter", Json{{"rate", 30}, {"speed", Json{{"x", 0}, {"y", 0}}}, {"gravity", Json{{"x", 0}, {"y", 0}, {"z", 0}}}, {"drag", 3}, {"lifetime", Json{{"x", 3}, {"y", 3}}}, {"world_space", true}}}}}}).has_value());
    for (int i = 0; i < 120; ++i) REQUIRE(s.frame().has_value());
    const Json list = s.command("particles.list", Json{{"entity", "Smoke"}, {"limit", 500}}).value();
    const Json& ps = list.contains("particles") ? list["particles"] : list;
    int old = 0;
    for (const Json& q : ps) {
        if (q["age"].get<double>() < 1.5) continue;
        ++old;
        REQUIRE(q["velocity"]["x"].get<double>() == Catch::Approx(5).margin(0.1));   // 1 - e^-4.5 of the way
        REQUIRE(q["position"]["x"].get<double>() > 4);
    }
    REQUIRE(old > 5);
    // Reeds lean downwind: the same moment drawn with the wind turned about looks different.
    REQUIRE(s.command("world.destroy", Json{{"entity", "Smoke"}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Floor"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", -0.5}, {"z", 0}}}, {"scale", Json{{"x", 20}, {"y", 1}, {"z", 20}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}, {"RigidBody", Json{{"kind", 1}}}, {"Collider", Json{{"shape", 0}, {"size", Json{{"x", 10}, {"y", 0.5}, {"z", 10}}}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Reeds"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 3}, {"z", 0}}}, {"scale", Json{{"x", 0.08}, {"y", 1.6}, {"z", 0.08}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", Json{{"r", 0.9}, {"g", 0.85}, {"b", 0.4}, {"a", 1}}}}},
        {"Scatter", Json{{"count", 300}, {"area", Json{{"x", 8}, {"y", 8}}}, {"seed", 2}, {"spacing", 0.3}, {"sway", 0.5}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 7}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    s.set_paused(true);
    REQUIRE(s.idle_frame().has_value());
    const std::filesystem::path ref = root() / "samples" / "playground" / ".pocket" / "test-wind.png";
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/test-wind.png"}, {"update", true}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Breeze"}, {"component", "Wind"}, {"value", Json{{"direction", 180}}}}).has_value());
    REQUIRE(s.idle_frame().has_value());
    const double turned = s.command("render.compare", Json{{"path", ".pocket/test-wind.png"}}).value()["fraction"].get<double>();
    INFO("turned " << turned);
    REQUIRE(turned > 0.02);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(ref);
}

TEST_CASE("the atmosphere: a blue day, a warm low sun that reddens its own light, a dark night, haze and drifting clouds", "[renderer][sky][atmosphere]") {
    const std::filesystem::path ref = root() / "samples" / "playground" / ".pocket" / "test-clouds.png";
    std::filesystem::remove(ref);
    app::Options o = playground_options();
    o.width = 160;
    o.height = 120;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 3}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json::object()}, {"Light", Json{{"kind", 0}, {"intensity", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json::object()}, {"Camera", Json{{"fov_degrees", 40}}}}}}).has_value());
    // A rotation turning -z toward (yaw about y, then pitch up), in degrees.
    auto turn = [](double yaw, double pitch) {
        const double y = yaw * 3.14159265 / 360, p = pitch * 3.14159265 / 360;
        return Json{{"x", std::cos(y) * std::sin(p)}, {"y", std::sin(y) * std::cos(p)}, {"z", -std::sin(y) * std::sin(p)}, {"w", std::cos(y) * std::cos(p)}};
    };
    // The sun at `elevation` degrees straight ahead (-z): its light shines the other way.
    auto sun_at = [&](double elevation) {
        REQUIRE(s.command("world.set", Json{{"entity", "Sun"}, {"component", "Transform"}, {"value", Json{{"rotation", turn(180, -elevation)}}}}).has_value());
    };
    auto look = [&](double yaw, double pitch) {
        REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"rotation", turn(yaw, pitch)}}}}).has_value());
        REQUIRE(s.frame().has_value());
        const Json p = s.command("capture", Json{{"pixel", Json{{"x", 80}, {"y", 60}}}}).value()["pixel"];
        return std::array<int, 3>{p[0].get<int>(), p[1].get<int>(), p[2].get<int>()};
    };
    auto sun_light = [&]() {
        const Json l = s.command("render.stats", Json::object()).value()["sun_light"];
        return std::array<double, 3>{l["r"].get<double>(), l["g"].get<double>(), l["b"].get<double>()};
    };
    // Day: the sky overhead is blue, and says it is the atmosphere.
    sun_at(60);
    const auto zenith = look(0, 80);
    INFO("zenith " << zenith[0] << "," << zenith[1] << "," << zenith[2]);
    REQUIRE(s.command("render.stats", Json::object()).value()["sky"] == "atmosphere");
    REQUIRE(zenith[2] > zenith[0] + 30);
    REQUIRE(zenith[2] > zenith[1]);
    const auto noon = sun_light();   // high up, nearly as the light says (white), a touch less blue
    REQUIRE(noon[0] == Catch::Approx(1).margin(0.02));
    REQUIRE(noon[2] == Catch::Approx(1).margin(0.06));
    // A low sun: the sky toward it is warm, and its light on the block is redder than at noon.
    sun_at(3);
    const auto sunset = look(0, 6);
    INFO("sunset " << sunset[0] << "," << sunset[1] << "," << sunset[2]);
    REQUIRE(sunset[0] > sunset[2] + 30);
    const auto low = sun_light();
    INFO("sun light low " << low[0] << "," << low[1] << "," << low[2]);
    REQUIRE(low[0] > 2 * low[2]);   // reddened
    REQUIRE(low[0] < noon[0]);      // and dimmed
    // Night: the sun gone below, the sky overhead a deep blue, and the key light the moon's: cool and dim.
    sun_at(-6);
    const auto night = look(0, 80);
    INFO("night " << night[0] << "," << night[1] << "," << night[2]);
    const auto moon = sun_light();
    INFO("moon light " << moon[0] << "," << moon[1] << "," << moon[2]);
    REQUIRE(moon[2] > moon[0]);
    REQUIRE(moon[2] < noon[2] / 5);
    REQUIRE(night[2] > night[0]);
    REQUIRE(night[0] + night[1] + night[2] < (zenith[0] + zenith[1] + zenith[2]) / 6);
    // Haze whitens the day sky: the gap between blue and red closes.
    sun_at(60);
    const auto clear = look(90, 25);
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"haze", 6}}}}).has_value());
    const auto hazy = look(90, 25);
    INFO("clear " << clear[0] << "," << clear[2] << " hazy " << hazy[0] << "," << hazy[2]);
    REQUIRE(hazy[2] - hazy[0] < clear[2] - clear[0] - 10);
    // Clouds shade the ground under them: a floor seen from above, under a sky full of them, is darker.
    REQUIRE(s.command("world.spawn", Json{{"name", "Floor"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", -2}, {"z", 0}}}, {"scale", Json{{"x", 40}, {"y", 1}, {"z", 40}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}}}}).has_value());
    auto floor_seen = [&](double cover) {
        REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"clouds", cover}, {"haze", 1}}}}).has_value());
        const auto p = look(0, -89);
        return p[0] + p[1] + p[2];
    };
    const int open_floor = floor_seen(0), shaded_floor = floor_seen(1);
    INFO("floor " << open_floor << " under clouds " << shaded_floor);
    REQUIRE(shaded_floor < open_floor * 0.8);
    REQUIRE(s.command("world.destroy", Json{{"entity", "Floor"}}).has_value());
    // Clouds cover part of the sky and drift with the wind.
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"haze", 1}, {"clouds", 0}}}}).has_value());
    look(0, 50);
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/test-clouds.png"}, {"update", true}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"clouds", 0.7}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const double covered = s.command("render.compare", Json{{"path", ".pocket/test-clouds.png"}}).value()["fraction"].get<double>();
    INFO("clouds cover " << covered);
    REQUIRE(covered > 0.2);
    // A wind of 10 carries them at 60 up there: two seconds later the view overhead differs.
    REQUIRE(s.command("world.spawn", Json{{"name", "Gale"}, {"components", Json{{"Wind", Json{{"speed", 10}}}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/test-clouds.png"}, {"update", true}}).has_value());
    for (int i = 0; i < 120; ++i) REQUIRE(s.frame().has_value());
    const double drifted = s.command("render.compare", Json{{"path", ".pocket/test-clouds.png"}, {"threshold", 4}}).value()["fraction"].get<double>();
    INFO("drifted " << drifted);
    REQUIRE(drifted > 0.02);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(ref);
}

TEST_CASE("volumetric clouds: marched where the sky shows, whiter than the blue they cover, a darker deck under rain, grey all round from inside, and the flat layer without depth", "[renderer][sky][clouds]") {
    app::Options o = playground_options();
    o.width = 160;
    o.height = 120;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 3}, {"clouds", 0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"yaw", 30}, {"pitch", -55}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"pitch", 45}}}}}, {"Camera", Json{{"fov_degrees", 70}}}}}}).has_value());
    // The frame's mean colour over a grid of its pixels, and the spread of their brightness.
    struct Look {
        double r = 0, g = 0, b = 0, spread = 0;
    };
    auto look = [&](int frames) {
        for (int i = 0; i < frames; ++i) REQUIRE(s.frame().has_value());
        Json points = Json::array();
        for (int y = 5; y < 120; y += 10)
            for (int x = 5; x < 160; x += 10) points.push_back(Json{{"x", x}, {"y", y}});
        const Json px = s.command("capture", Json{{"pixels", points}}).value()["pixels"];
        Look l;
        double squares = 0;
        for (const Json& p : px) {
            l.r += p[0].get<double>();
            l.g += p[1].get<double>();
            l.b += p[2].get<double>();
            const double v = (p[0].get<double>() + p[1].get<double>() + p[2].get<double>()) / 3;
            squares += v * v;
        }
        const auto n = static_cast<double>(px.size());
        l.r /= n;
        l.g /= n;
        l.b /= n;
        const double mean = (l.r + l.g + l.b) / 3;
        l.spread = std::sqrt(std::max(0.0, squares / n - mean * mean));
        return l;
    };
    auto marched = [&]() { return s.command("render.stats", Json::object()).value()["clouds"].get<bool>(); };
    const Look clear = look(2);
    REQUIRE_FALSE(marched());
    // Clouds by default have depth: marched, whitening the blue, broken (the frame's brightness varies).
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"clouds", 0.6}}}}).has_value());
    const Look cloudy = look(8);
    INFO("clear " << clear.r << "," << clear.g << "," << clear.b << " spread " << clear.spread << "; cloudy " << cloudy.r << "," << cloudy.g << "," << cloudy.b << " spread " << cloudy.spread);
    REQUIRE(marched());
    REQUIRE(cloudy.r - cloudy.b > clear.r - clear.b + 10);
    REQUIRE(cloudy.spread > clear.spread + 5);
    // Rain's overcast closes them into a deck: no blue left between them, and darker.
    REQUIRE(s.command("world.spawn", Json{{"name", "Rain"}, {"components", Json{{"Weather", Json{{"overcast", 0.9}}}}}}).has_value());
    const Look rain = look(8);
    INFO("rain " << rain.r << "," << rain.g << "," << rain.b << " spread " << rain.spread);
    REQUIRE(rain.b - rain.r < cloudy.b - cloudy.r);
    REQUIRE(rain.b - rain.r < 12);
    REQUIRE(rain.r + rain.g + rain.b < cloudy.r + cloudy.g + cloudy.b);
    REQUIRE(s.command("world.destroy", Json{{"entity", "Rain"}}).has_value());
    // From inside a full layer, looking level: white every way (a little blue from the sky's light), even.
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"clouds", 1}, {"cloud_height", 100}, {"cloud_depth", 400}, {"cloud_scale", 400}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 0}, {"y", 200}, {"z", 0}}}, {"rotation", Json{{"pitch", 0}}}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Camera"}, {"value", Json{{"fov_degrees", 40}}}}).has_value());
    const Look inside = look(8);
    INFO("inside " << inside.r << "," << inside.g << "," << inside.b << " spread " << inside.spread);
    REQUIRE(inside.b - inside.r < (clear.b - clear.r) / 3);
    REQUIRE(inside.r + inside.g + inside.b > clear.r + clear.g + clear.b);
    REQUIRE(inside.spread < cloudy.spread);
    // No depth: the flat layer, drawn in the sky pass and not marched.
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"clouds", 0.6}, {"cloud_height", 1500}, {"cloud_depth", 0}, {"cloud_scale", 900}}}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Camera"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 0}}}, {"rotation", Json{{"pitch", 45}}}}}}).has_value());
    const Look flat = look(2);
    INFO("flat " << flat.r << "," << flat.g << "," << flat.b);
    REQUIRE_FALSE(marched());
    REQUIRE(flat.r - flat.b > clear.r - clear.b + 5);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("an ocean runs to the horizon: below it the view shows water, not the sky's ground; a lake does not", "[renderer][water][ocean]") {
    app::Options o = playground_options();
    o.width = 160;
    o.height = 120;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"yaw", 30}, {"pitch", -40}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 4}, {"z", 0}}}}}, {"Camera", Json{{"fov_degrees", 50}, {"far", 500}}}}}}).has_value());
    // Pixels under the horizon (the camera looks level), out to near the bottom.
    auto below = [&]() {
        for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
        const Json px = s.command("capture", Json{{"pixels", Json::array({Json{{"x", 80}, {"y", 72}}, Json{{"x", 20}, {"y", 90}}, Json{{"x", 80}, {"y", 115}}})}}).value()["pixels"];
        return px;
    };
    const Json bare = below();   // the procedural sky's ground below the horizon
    REQUIRE(s.command("world.spawn", Json{{"name", "Sea"}, {"components", Json{{"Transform", Json::object()}, {"Water", Json{{"ocean", true}, {"wave_height", 0.2}, {"color", Json{{"r", 0.05}, {"g", 0.25}, {"b", 0.4}, {"a", 1}}}}}}}}).has_value());
    const Json sea = below();
    INFO("bare " << bare.dump() << " sea " << sea.dump());
    REQUIRE(s.command("render.stats", Json::object()).value()["water"]["bodies"] == 1);
    for (int k = 0; k < 3; ++k) {
        // Water there, bluer than the ground it covers, far out as near.
        REQUIRE(sea[k][2].get<int>() > sea[k][0].get<int>() + 10);
        REQUIRE(sea[k][2].get<int>() - sea[k][0].get<int>() > bare[k][2].get<int>() - bare[k][0].get<int>() + 10);
    }
    // A lake ten units across, the same otherwise, leaves the ground under the horizon bare.
    REQUIRE(s.command("world.set", Json{{"entity", "Sea"}, {"component", "Water"}, {"value", Json{{"ocean", false}, {"size", Json{{"x", 10}, {"y", 10}}}}}}).has_value());
    const Json lake = below();
    INFO("lake " << lake.dump());
    REQUIRE(std::abs(lake[0][2].get<int>() - bare[0][2].get<int>()) < 6);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("grass grows on a terrain: blades over bare brown ground turn it green, none where its rules forbid, and they are the terrain's to pick", "[renderer][terrain][grass]") {
    app::Options o = playground_options();
    o.width = 160;
    o.height = 120;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"yaw", 30}, {"pitch", -50}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1}}}}}}).has_value());
    Json ground = Json::object();
    ground["Transform"] = Json::object();
    ground["Terrain"] = Json{{"size", Json{{"x", 40}, {"y", 40}}}, {"height", 0.05}, {"resolution", 33}, {"grass", "#806040"}};
    ground["MeshRenderer"] = Json::object();
    REQUIRE(s.command("world.spawn", Json{{"name", "Ground"}, {"components", ground}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.2}, {"z", 6}}}, {"rotation", Json{{"pitch", -12}}}}}, {"Camera", Json{{"fov_degrees", 50}}}}}}).has_value());
    // The frame's lower half, where the ground is: its mean green over its mean red.
    auto greenness = [&]() {
        for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
        Json points = Json::array();
        for (int y = 70; y < 120; y += 8)
            for (int x = 8; x < 160; x += 16) points.push_back(Json{{"x", x}, {"y", y}});
        const Json px = s.command("capture", Json{{"pixels", points}}).value()["pixels"];
        double r = 0, g = 0;
        for (const Json& p : px) { r += p[0].get<double>(); g += p[1].get<double>(); }
        return g / std::max(r, 1.0);
    };
    const double bare = greenness();
    REQUIRE(s.command("world.set", Json{{"entity", "Ground"}, {"component", "Grass"}, {"value", Json{{"density", 30}, {"height", 0.4}}}}).has_value());
    const double grown = greenness();
    INFO("bare " << bare << " grown " << grown);
    const Json stats = s.command("render.stats", Json::object()).value();
    REQUIRE(stats.value("grass_blades", 0) > 1000);
    REQUIRE(grown > bare + 0.15);
    // A pixel of grass picks the terrain.
    const Json picked = s.command("render.pick", Json{{"x", 80}, {"y", 110}}).value();
    INFO(picked.dump());
    REQUIRE(picked["path"] == "/Ground");
    // Grass that may grow only above the ground: none.
    REQUIRE(s.command("world.set", Json{{"entity", "Ground"}, {"component", "Grass"}, {"value", Json{{"min_height", 5}}}}).has_value());
    const double above = greenness();
    INFO("above " << above);
    REQUIRE(std::abs(above - bare) < 0.05);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("aerial perspective: under an atmosphere far land fades toward the air, the sky as it was, and a Fog replaces it", "[renderer][sky][aerial]") {
    app::Options o = playground_options();
    o.width = 160;
    o.height = 120;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sky"}, {"components", Json{{"Sky", Json{{"mode", 3}, {"aerial", 0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"yaw", 160}, {"pitch", -50}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1}}}}}}).has_value());
    // A dark block 900 units off filling the middle of the view, the sky above it.
    REQUIRE(s.command("world.spawn", Json{{"name", "Far"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", -900}}}, {"scale", Json{{"x", 300}, {"y", 200}, {"z", 10}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", "#202020"}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"Camera", Json{{"fov_degrees", 40}, {"far", 2000}}}}}}).has_value());
    auto look = [&]() {
        for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
        return s.command("capture", Json{{"pixels", Json::array({Json{{"x", 80}, {"y", 60}}, Json{{"x", 80}, {"y", 4}}})}}).value()["pixels"];
    };
    const Json clear = look();
    REQUIRE(s.command("world.set", Json{{"entity", "Sky"}, {"component", "Sky"}, {"value", Json{{"aerial", 1}}}}).has_value());
    const Json hazed = look();
    INFO("clear " << clear.dump() << " hazed " << hazed.dump());
    auto sum = [](const Json& p) { return p[0].get<int>() + p[1].get<int>() + p[2].get<int>(); };
    REQUIRE(sum(hazed[0]) > sum(clear[0]) + 40);                  // the block lightened toward the air
    REQUIRE(std::abs(sum(hazed[1]) - sum(clear[1])) <= 3);        // the sky above it as it was
    // A Fog of the scene's own takes its place: thin enough, the block is darker than in the air's haze.
    REQUIRE(s.command("world.spawn", Json{{"name", "Mist"}, {"components", Json{{"Fog", Json{{"density", 0.00001}}}}}}).has_value());
    const Json fogged = look();
    INFO("own fog " << fogged.dump());
    REQUIRE(sum(fogged[0]) < sum(hazed[0]) - 20);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("caustics: the sunlight on a bed under water gathers into lines that move with time, and none without", "[renderer][water][caustics]") {
    const std::filesystem::path ref = root() / "samples" / "playground" / ".pocket" / "test-caustics.png";
    std::filesystem::remove(ref);
    app::Options o = playground_options();
    o.width = 192;
    o.height = 128;
    o.frames = 1000;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    Json bed = Json::object();
    bed["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -2}, {"z", 0}}}, {"scale", Json{{"x", 40}, {"y", 2}, {"z", 40}}}};
    bed["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0.85}, {"g", 0.75}, {"b", 0.55}, {"a", 1}}}, {"roughness", 1.0}};
    REQUIRE(s.command("world.spawn", Json{{"name", "Bed"}, {"components", bed}}).has_value());   // its top a unit under the surface
    REQUIRE(s.command("world.spawn", Json{{"name", "Sun"}, {"components", Json{{"Transform", Json{{"rotation", Json{{"x", -0.64}, {"y", 0.1}, {"z", 0.05}, {"w", 0.76}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2.0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Lake"}, {"components", Json{{"Transform", Json::object()}, {"Water", Json{{"size", Json{{"x", 30}, {"y", 30}}}, {"wave_height", 0}, {"ripples", 0}, {"foam", 0}, {"clarity", 8.0}}}}}}).has_value());
    REQUIRE(s.command("world.spawn", Json{{"name", "Camera"}, {"components", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 5}, {"z", 0}}}, {"rotation", Json{{"x", -0.7071}, {"y", 0}, {"z", 0}, {"w", 0.7071}}}}}, {"Camera", Json{{"fov_degrees", 60}}}}}}).has_value());
    // How much the bed's brightness varies over a grid of points on it.
    auto spread = [&]() {
        REQUIRE(s.frame().has_value());
        Json pts = Json::array();
        for (int j = 0; j < 8; ++j) for (int i = 0; i < 12; ++i) pts.push_back(Json{{"x", 20 + i * 13}, {"y", 12 + j * 13}});
        const Json px = s.command("capture", Json{{"pixels", pts}}).value()["pixels"];
        double sum = 0, sq = 0;
        for (const Json& p : px) {
            const double l = p[0].get<double>() + p[1].get<double>() + p[2].get<double>();
            sum += l;
            sq += l * l;
        }
        const double n = static_cast<double>(px.size()), mean = sum / n;
        return std::sqrt(std::max(sq / n - mean * mean, 0.0));
    };
    REQUIRE(s.command("world.set", Json{{"entity", "Lake"}, {"component", "Water"}, {"value", Json{{"caustics", 0}}}}).has_value());
    const double flat = spread();
    REQUIRE(s.command("world.set", Json{{"entity", "Lake"}, {"component", "Water"}, {"value", Json{{"caustics", 1}}}}).has_value());
    const double lit = spread();
    INFO("spread without " << flat << ", with " << lit);
    REQUIRE(lit > flat * 1.3);   // about 66 against 42 (the flat bed varies with the sky it reflects)
    // They move: a third of a second later the bed looks different.
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/test-caustics.png"}, {"update", true}}).has_value());
    for (int i = 0; i < 20; ++i) REQUIRE(s.frame().has_value());
    const double moved = s.command("render.compare", Json{{"path", ".pocket/test-caustics.png"}, {"threshold", 6}}).value()["fraction"].get<double>();
    INFO("moved " << moved);
    REQUIRE(moved > 0.05);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(ref);
}

TEST_CASE("a decal's normal map bends the light on what it covers: tilted toward the sun it is brighter than tilted away", "[renderer][decals][decalnormals]") {
    // Two flat normal maps, tilted 40 degrees toward +x and toward -x (written with the terrain
    // paint's RGBA PNG writer: one colour all over).
    const std::filesystem::path dir = root() / "samples" / "playground" / "assets";
    auto normal_png = [&](const char* name, float nx, float nz) {
        assets::Terrain t;
        t.n = 2;
        t.h.assign(4, 0.0f);
        t.paint.assign(4, std::array<float, 4>{nx * 0.5f + 0.5f, 0.5f, nz * 0.5f + 0.5f, 1.0f});
        std::ofstream(dir / name, std::ios::binary) << assets::terrain_paint_png(t);
    };
    normal_png("test-tilt-east.png", std::sin(0.7f), std::cos(0.7f));
    normal_png("test-tilt-west.png", -std::sin(0.7f), std::cos(0.7f));
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", std::move(components)}}).has_value()); };
    Json floor = Json::object();
    floor["Transform"] = Json{{"position", Json{{"x", 0}, {"y", -0.1}, {"z", 0}}}, {"scale", Json{{"x", 20}, {"y", 0.2}, {"z", 20}}}};
    floor["MeshRenderer"] = Json{{"mesh", "cube"}, {"color", Json{{"r", 0.5}, {"g", 0.5}, {"b", 0.5}, {"a", 1}}}, {"roughness", 1.0}};
    spawn("Floor", floor);
    // The sun low in the east (+x), shining west and down at 25 degrees.
    spawn("Sun", Json{{"Transform", Json{{"rotation", Json{{"x", -0.1531}, {"y", 0.6903}, {"z", 0.1531}, {"w", 0.6903}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 2}}}});
    auto decal = [&](const char* name, double x, const char* map) {
        Json d{{"color", Json{{"r", 0.5}, {"g", 0.5}, {"b", 0.5}, {"a", 1}}}, {"size", Json{{"x", 2}, {"y", 1}, {"z", 2}}}};
        if (map) d["normal_map"] = map;
        spawn(name, Json{{"Transform", Json{{"position", Json{{"x", x}, {"y", 0.2}, {"z", 0}}}}}, {"Decal", d}});
    };
    decal("East", -3, "assets/test-tilt-east.png");
    decal("Flat", 0, nullptr);
    decal("West", 3, "assets/test-tilt-west.png");
    spawn("Camera", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 6}, {"z", 5}}}, {"rotation", Json{{"x", -0.4}, {"y", 0}, {"z", 0}, {"w", 0.9165}}}}}, {"Camera", Json{{"fov_degrees", 60}}}});
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    auto lum = [&](double x) {
        Json at = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", 0}, {"z", 0}}}}).value();
        Json p = s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
        return p[0].get<int>() + p[1].get<int>() + p[2].get<int>();
    };
    const int east = lum(-3), flat = lum(0), west = lum(3);
    INFO("toward the sun " << east << ", flat " << flat << ", away " << west);
    REQUIRE(east > flat + 30);
    REQUIRE(west < flat - 30);
    REQUIRE(s.command("render.stats", Json::object()).value()["decals"]["drawn"] == 3);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(dir / "test-tilt-east.png");
    std::filesystem::remove(dir / "test-tilt-west.png");
}

TEST_CASE("a terrain drawn from textured layers: the first everywhere, a painted one over it, its image tiled, colour paint over both", "[renderer][terrain][terrainlayers]") {
    // A 2 by 2 checker (written with the terrain paint's RGBA PNG writer), for a layer whose image shows.
    const std::filesystem::path dir = root() / "samples" / "playground" / "assets";
    {
        assets::Terrain t;
        t.n = 2;
        t.h.assign(4, 0.0f);
        t.paint = {{1, 1, 1, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {1, 1, 1, 1}};
        std::ofstream(dir / "test-checker.png", std::ios::binary) << assets::terrain_paint_png(t);
    }
    app::Options o = playground_options();
    o.width = 256;
    o.height = 256;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", std::move(components)}}).has_value()); };
    const Json down{{"x", -0.7071}, {"y", 0}, {"z", 0}, {"w", 0.7071}};
    spawn("Sun", Json{{"Transform", Json{{"rotation", down}}}, {"Light", Json{{"kind", 0}, {"intensity", 2}}}});
    Json layers = Json::array();
    layers.push_back(Json{{"name", "red"}, {"color", Json{{"r", 1}, {"g", 0.1}, {"b", 0.1}, {"a", 1}}}});
    layers.push_back(Json{{"name", "checker"}, {"texture", "assets/test-checker.png"}, {"tile", 2}, {"cover", 0}});
    spawn("Ground", Json{{"Transform", Json::object()}, {"Terrain", Json{{"size", Json{{"x", 20}, {"y", 20}}}, {"height", 0.01}, {"resolution", 41}, {"layers", layers}}}, {"MeshRenderer", Json{{"roughness", 1.0}}}});
    spawn("Camera", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 12}, {"z", 0}}}, {"rotation", down}}}, {"Camera", Json{{"fov_degrees", 60}}}});
    auto pixel = [&](double x, double z) {
        Json at = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", 0}, {"z", z}}}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
    };
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    // The first layer lies everywhere: red.
    Json p = pixel(0, 0);
    INFO("red layer " << p.dump());
    REQUIRE(p[0].get<int>() > 120);
    REQUIRE(p[1].get<int>() < p[0].get<int>() / 2);
    REQUIRE(p[2].get<int>() < p[0].get<int>() / 2);
    // Painted over it at full strength, the checker shows, a light and a dark square every unit.
    REQUIRE(s.command("terrain.paint", Json{{"x", 0}, {"z", 0}, {"layer", "checker"}, {"radius", 10}, {"amount", 1}}).has_value());
    REQUIRE(s.frame().has_value());
    int lo = 999, hi = -1;
    for (double x = -1.5; x <= 1.5; x += 0.25) {
        p = pixel(x, 0.5);
        const int lum = p[0].get<int>() + p[1].get<int>() + p[2].get<int>();
        lo = std::min(lo, lum);
        hi = std::max(hi, lum);
        REQUIRE(std::abs(p[0].get<int>() - p[2].get<int>()) < 30);   // grey, not red
    }
    INFO("checker from " << lo << " to " << hi);
    REQUIRE(hi - lo > 150);
    // Far from the paint, still red.
    p = pixel(8, 8);
    REQUIRE(p[0].get<int>() > p[2].get<int>() + 60);
    // A colour laid over both covers them.
    REQUIRE(s.command("terrain.paint", Json{{"x", 0}, {"z", 0}, {"color", Json{{"r", 0.1}, {"g", 0.8}, {"b", 0.1}}}, {"radius", 3}, {"amount", 1}}).has_value());
    REQUIRE(s.frame().has_value());
    p = pixel(0, 0);
    INFO("green over the checker " << p.dump());
    REQUIRE(p[1].get<int>() > p[0].get<int>() + 60);
    REQUIRE(p[1].get<int>() > p[2].get<int>() + 60);
    REQUIRE(s.finish().has_value());
    std::filesystem::remove(dir / "test-checker.png");
}

TEST_CASE("glass shows what is behind it, bent by its index of refraction and blurred by its roughness; a clear coat adds a sharp highlight", "[renderer][glass]") {
    app::Options o = playground_options();
    o.width = 320;
    o.height = 180;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", std::move(components)}}).has_value()); };
    auto xform = [](double x, double y, double z, double sx, double sy, double sz) { return Json{{"position", Json{{"x", x}, {"y", y}, {"z", z}}}, {"scale", Json{{"x", sx}, {"y", sy}, {"z", sz}}}}; };
    auto rgb = [](double r, double g, double b) { return Json{{"r", r}, {"g", g}, {"b", b}, {"a", 1}}; };
    spawn("Sun", Json{{"Transform", Json{{"rotation", Json{{"x", -0.42}, {"y", 0.3}, {"z", 0.15}, {"w", 0.84}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1.2}}}});
    // A wall of red and blue bars, half a unit each, three units behind the glass.
    for (int k = 0; k < 12; ++k) {
        const std::string name = "Bar" + std::to_string(k);
        spawn(name.c_str(), Json{{"Transform", xform(-2.75 + 0.5 * k, 1.5, -3, 0.5, 3, 0.2)}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", k % 2 == 0 ? rgb(0.9, 0.1, 0.1) : rgb(0.1, 0.2, 0.9)}}}});
    }
    spawn("Camera", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1.5}, {"z", 5}}}}}, {"Camera", Json{{"fov_degrees", 45}}}});
    auto pixel = [&](double x, double y, double z) {
        Json at = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", z}}}}).value();
        return s.command("capture", Json{{"pixel", Json{{"x", at["x"]}, {"y", at["y"]}}}}).value()["pixel"];
    };
    auto redness = [](const Json& p) { return p[0].get<int>() - p[2].get<int>(); };
    auto frame = [&]() { REQUIRE(s.frame().has_value()); REQUIRE(s.frame().has_value()); };
    // A white pane in front of a red bar hides it; the same pane as clear glass shows it.
    spawn("Pane", Json{{"Transform", xform(0.25, 1.5, 0, 1.2, 1.6, 0.04)}, {"MeshRenderer", Json{{"mesh", "cube"}, {"color", rgb(1, 1, 1)}, {"roughness", 0.1}}}});
    frame();
    const Json behind = pixel(0.25, 1.5, -3);   // the red bar at x 0.25, seen through the pane's middle
    const Json white = pixel(0.25, 1.5, 0);
    INFO("opaque pane " << white.dump());
    REQUIRE(std::abs(redness(white)) < 40);
    REQUIRE(s.command("world.set", Json{{"entity", "Pane"}, {"component", "MeshRenderer"}, {"value", Json{{"transmission", 1}}}}).has_value());
    frame();
    Json clear = pixel(0.25, 1.5, 0);
    INFO("glass pane " << clear.dump() << " the bar " << behind.dump());
    REQUIRE(redness(clear) > 100);
    REQUIRE(s.command("render.stats", Json::object()).value()["glass"] == 1);
    // Frosted, the bars behind blur together: the colours along a line across it vary much less.
    auto spread = [&]() {
        int lo = 999, hi = -999;
        for (double x = -0.25; x <= 0.75; x += 0.0625) {
            const int r = redness(pixel(x, 1.5, 0));
            lo = std::min(lo, r);
            hi = std::max(hi, r);
        }
        return hi - lo;
    };
    const int sharp = spread();
    REQUIRE(s.command("world.set", Json{{"entity", "Pane"}, {"component", "MeshRenderer"}, {"value", Json{{"roughness", 0.8}}}}).has_value());
    frame();
    const int frosted = spread();
    INFO("across the clear pane " << sharp << ", the frosted " << frosted);
    REQUIRE(sharp > 250);
    REQUIRE(frosted < sharp / 2);
    REQUIRE(s.command("world.destroy", Json{{"entity", "Pane"}}).has_value());
    // A thick ball of glass bends what is behind it: with ior 1 it shows the bars as they are, with
    // 1.5 the view through it changes.
    spawn("Ball", Json{{"Transform", xform(0, 1.5, 0, 1.6, 1.6, 1.6)}, {"MeshRenderer", Json{{"mesh", "sphere"}, {"color", rgb(1, 1, 1)}, {"roughness", 0.05}, {"transmission", 1}, {"ior", 1.0}, {"thickness", 1.6}}}});
    frame();
    std::vector<int> straight, bent;
    for (double x = -0.5; x <= 0.5; x += 0.125) straight.push_back(redness(pixel(x, 1.5, 0.8)));
    REQUIRE(s.command("world.set", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}, {"value", Json{{"ior", 1.5}}}}).has_value());
    frame();
    for (double x = -0.5; x <= 0.5; x += 0.125) bent.push_back(redness(pixel(x, 1.5, 0.8)));
    int changed = 0;
    for (std::size_t k = 0; k < straight.size(); ++k) changed += std::abs(straight[k] - bent[k]) > 100 ? 1 : 0;
    INFO("columns whose colour the bending changed: " << changed);
    REQUIRE(changed >= 2);
    REQUIRE(s.command("world.destroy", Json{{"entity", "Ball"}}).has_value());
    // A clear coat over a rough red ball: the sun's highlight on it is far brighter than without.
    auto brightest = [&]() {
        Json points = Json::array();
        for (double x = -0.45; x <= 0.45; x += 0.025)
            for (double y = 1.05; y <= 1.95; y += 0.025) {
                const Json at = s.command("render.project", Json{{"point", Json{{"x", x}, {"y", y}, {"z", 0.5}}}}).value();
                points.push_back(Json{{"x", at["x"]}, {"y", at["y"]}});
            }
        int best = 0;
        for (const Json& p : s.command("capture", Json{{"pixels", points}}).value()["pixels"]) best = std::max(best, p[1].get<int>());   // green: the red ball's own colour has little
        return best;
    };
    spawn("Lacquer", Json{{"Transform", xform(0, 1.5, 0, 1, 1, 1)}, {"MeshRenderer", Json{{"mesh", "sphere"}, {"color", rgb(0.7, 0.05, 0.05)}, {"roughness", 0.7}}}});
    frame();
    const int plain = brightest();
    REQUIRE(s.command("world.set", Json{{"entity", "Lacquer"}, {"component", "MeshRenderer"}, {"value", Json{{"clearcoat", 1}, {"clearcoat_roughness", 0.2}}}}).has_value());
    frame();
    const int coated = brightest();
    INFO("highlight without a coat " << plain << ", with " << coated);
    REQUIRE(coated > plain + 60);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("levels of detail: a mesh small on screen draws simpler, smaller still not at all; scattered copies pick their own", "[renderer][lod]") {
    app::Options o = playground_options();
    o.width = 256;
    o.height = 144;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", std::move(components)}}).has_value()); };
    spawn("Sun", Json{{"Transform", Json{{"rotation", Json{{"x", -0.42}, {"y", 0.3}, {"z", 0.15}, {"w", 0.84}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1.5}}}});
    spawn("Camera", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", 0}}}}}, {"Camera", Json{{"fov_degrees", 60}, {"far", 1000}}}});
    // A sphere with two levels (a quarter of its triangles below 0.3 of the view, a twentieth below
    // 0.08) and culled below 0.01.
    Json lods = Json::array({Json{{"screen", 0.08}, {"ratio", 0.05}}, Json{{"screen", 0.3}, {"ratio", 0.25}}});   // any order
    spawn("Ball", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", -2.5}}}}}, {"MeshRenderer", Json{{"mesh", "sphere"}, {"lods", lods}, {"cull_screen", 0.01}}}});
    auto at = [&](double z) {
        REQUIRE(s.command("world.set", Json{{"entity", "Ball"}, {"component", "Transform"}, {"value", Json{{"position", Json{{"x", 0}, {"y", 0}, {"z", z}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
        REQUIRE(s.frame().has_value());
        return s.command("render.stats", Json::object()).value();
    };
    // Its bounds' radius is 0.866 (half the unit box's diagonal), P11 is 1 / tan(30 degrees): it covers
    // 0.866 * 1.732 / d of the view's height at distance d.
    Json near = at(-2.5);
    INFO(near.dump());
    REQUIRE(near["lod"]["simplified"] == 0);
    REQUIRE(near["meshes"] == 1);
    Json mid = at(-8);   // 0.19: the first level
    REQUIRE(mid["lod"]["simplified"] == 1);
    const Json mid_pixels = s.command("capture", Json::object()).value()["center_pixel"];
    Json far = at(-40);  // 0.037: the second
    REQUIRE(far["lod"]["simplified"] == 1);
    Json gone = at(-200);   // 0.0075: under cull_screen
    REQUIRE(gone["lod"]["culled"] == 1);
    REQUIRE(gone["lod"]["simplified"] == 0);
    REQUIRE(s.command("capture", Json::object()).value()["center_pixel"] != mid_pixels);   // nothing drawn there now
    // Up close but forced to its coarsest level (a screen of 100 is always met), the ball's outline
    // is not the full sphere's.
    at(-2.5);
    REQUIRE(s.command("render.compare", Json{{"path", ".pocket/test-lod.png"}, {"update", true}}).has_value());
    REQUIRE(s.command("world.set", Json{{"entity", "Ball"}, {"component", "MeshRenderer"}, {"value", Json{{"lods", Json::array({Json{{"screen", 100}, {"ratio", 0.05}}})}}}}).has_value());
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    const double coarse = s.command("render.compare", Json{{"path", ".pocket/test-lod.png"}}).value()["fraction"].get<double>();
    INFO("changed by the coarse level " << coarse);
    REQUIRE(coarse > 0.005);
    std::filesystem::remove(root() / "samples" / "playground" / ".pocket" / "test-lod.png");
    REQUIRE(s.command("world.destroy", Json{{"entity", "Ball"}}).has_value());
    // A field of copies from near to far: the far ones take the simpler level, the near ones not.
    spawn("Floor", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", -1.5}, {"z", -60}}}, {"scale", Json{{"x", 60}, {"y", 1}, {"z", 130}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}}}, {"RigidBody", Json{{"kind", 1}}}, {"Collider", Json{{"shape", 0}, {"size", Json{{"x", 30}, {"y", 0.5}, {"z", 65}}}}}});
    spawn("Stones", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 5}, {"z", -60}}}}}, {"MeshRenderer", Json{{"mesh", "sphere"}, {"lods", Json::array({Json{{"screen", 0.1}, {"ratio", 0.1}}})}}},
        {"Scatter", Json{{"count", 400}, {"area", Json{{"x", 20}, {"y", 110}}}, {"seed", 3}, {"spacing", 1}}}});
    REQUIRE(s.frame().has_value());
    REQUIRE(s.frame().has_value());
    const Json field = s.command("render.stats", Json::object()).value();
    INFO(field.dump());
    const int scattered = field["scattered"].get<int>();
    const int simplified = field["lod"]["simplified"].get<int>();
    REQUIRE(scattered > 100);
    REQUIRE(simplified > scattered / 3);
    REQUIRE(simplified < scattered);
    // And they cost fewer triangles than the same field at full detail.
    REQUIRE(s.command("world.set", Json{{"entity", "Stones"}, {"component", "MeshRenderer"}, {"value", Json{{"lods", Json::array()}}}}).has_value());
    REQUIRE(s.frame().has_value());
    const Json full = s.command("render.stats", Json::object()).value();
    INFO("triangles with levels " << field["triangles"] << ", without " << full["triangles"]);
    REQUIRE(field["triangles"].get<double>() < full["triangles"].get<double>() * 0.8);
    REQUIRE(s.finish().has_value());
}

TEST_CASE("the frame's GPU time by pass, what the camera's passes leave out, and the casters each cascade draws", "[renderer][gpu][culling]") {
    app::Options o = playground_options();
    o.width = 320;
    o.height = 180;
    app::Session s(o);
    REQUIRE(s.start().has_value());
    REQUIRE(s.command("world.clear", Json::object()).has_value());
    auto spawn = [&](const std::string& name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", std::move(components)}}).has_value()); };
    auto at = [](double x, double y, double z) { return Json{{"position", Json{{"x", x}, {"y", y}, {"z", z}}}}; };
    spawn("Sun", Json{{"Transform", Json{{"rotation", Json{{"x", -0.42}, {"y", 0.3}, {"z", 0.15}, {"w", 0.84}}}}}, {"Light", Json{{"kind", 0}, {"intensity", 1.5}}}});
    spawn("Camera", Json{{"Transform", at(0, 1, 5)}, {"Camera", Json{{"fov_degrees", 60}, {"far", 500}}}});
    spawn("Ahead", Json{{"Transform", at(0, 0.5, 0)}, {"MeshRenderer", Json{{"mesh", "cube"}}}});
    spawn("Behind", Json{{"Transform", at(0, 0.5, 12)}, {"MeshRenderer", Json{{"mesh", "cube"}}}});
    // A line of blocks running away from the camera, 150 units long: each cascade covers a stretch.
    for (int i = 0; i < 30; ++i) spawn("Block" + std::to_string(i), Json{{"Transform", at(3, 0.5, -5.0 * i)}, {"MeshRenderer", Json{{"mesh", "cube"}}}});
    // The first frame draws every cascade; later ones keep those whose casters stand still.
    REQUIRE(s.frame().has_value());
    const Json first = s.command("render.stats", Json::object()).value();
    for (int i = 0; i < 3; ++i) REQUIRE(s.frame().has_value());
    REQUIRE(first["shadow_redrawn"] == 4);
    REQUIRE(s.command("render.stats", Json::object()).value()["shadow_redrawn"] == 0);
    // A block turning in front of the camera: its cascade is drawn again every frame.
    for (int i = 0; i < 8; ++i) {
        REQUIRE(s.command("world.set", Json{{"entity", "Ahead"}, {"component", "Transform"}, {"value", Json{{"rotation", Json{{"yaw", 10 * (i + 1)}}}}}}).has_value());
        REQUIRE(s.frame().has_value());
    }
    const Json st = s.command("render.stats", Json::object()).value();
    REQUIRE(st["shadow_redrawn"].get<int>() >= 1);
    INFO(st.dump());
    // The block behind the camera is left out of the camera's passes (and so not picked or seen).
    REQUIRE(st["out_of_view"].get<int>() >= 1);
    const Json seen = s.command("render.visible", Json::object()).value();
    bool behind_seen = false;
    for (const Json& e : seen["visible"]) behind_seen = behind_seen || e.value("path", "") == "/Behind";
    REQUIRE(seen["count"].get<int>() >= 1);
    REQUIRE_FALSE(behind_seen);
    // Each cascade draws the casters over its own square, not all 32 four times.
    const int cascades = st["shadow_cascades"].get<int>();
    REQUIRE(cascades == 4);
    REQUIRE(first["shadow_instances"].get<int>() < cascades * 32);
    REQUIRE(first["shadow_instances"].get<int>() >= 32);
    // On a device with timestamps (Metal, Vulkan, D3D12, a browser that grants them), each pass's
    // milliseconds, a few frames old.
    if (st.contains("gpu")) {
        const Json& gpu = st["gpu"];
        REQUIRE(gpu["frames_timed"].get<int>() > 0);
        REQUIRE(gpu["frames_ago"].get<int>() <= 6);
        REQUIRE(gpu["ms"].get<double>() > 0);
        // Which passes a frame's reading holds varies (a GPU may leave a pass's timestamps unwritten),
        // but there are some, named as the passes are, none negative.
        REQUIRE_FALSE(gpu["passes"].empty());
        for (const Json& p : gpu["passes"]) {
            REQUIRE(p["ms"].get<double>() >= 0);
            const std::string name = p["pass"].get<std::string>();
            REQUIRE((name == "scene" || name == "shadow" || name == "ids" || name == "post" || name.starts_with("scene")));
        }
    }
    REQUIRE(s.finish().has_value());
}

TEST_CASE("a session lets go of everything it made on the GPU: nothing is left holding the device", "[renderer][gpu][teardown]") {
    {
        app::Session s(playground_options());
        REQUIRE(s.start().has_value());
        auto spawn = [&](const char* name, Json components) { REQUIRE(s.command("world.spawn", Json{{"name", name}, {"components", std::move(components)}}).has_value()); };
        // The lazily made parts too: glass and its copy of the scene, a cut-out material, a particle.
        spawn("Pane", Json{{"Transform", Json{{"position", Json{{"x", 0}, {"y", 1}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"transmission", 1}}}});
        spawn("Leaf", Json{{"Transform", Json{{"position", Json{{"x", 1}, {"y", 1}, {"z", 0}}}}}, {"MeshRenderer", Json{{"mesh", "cube"}, {"cutoff", 0.5}}}});
        for (int i = 0; i < 4; ++i) REQUIRE(s.frame().has_value());
    }
    INFO("still held: " << rhi::Device::held_when_last_destroyed());
    REQUIRE(rhi::Device::held_when_last_destroyed().empty());
}
