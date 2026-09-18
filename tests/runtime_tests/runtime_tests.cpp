#include <pocket/app/runtime.hpp>
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

app::Options hello_options(int frames) {
    app::Options o;
    o.project_dir = root() / "samples" / "hello";
    o.bundle = root() / "build" / "ts" / "hello.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.frames = frames;
    o.width = 128;
    o.height = 72;
    o.log_level = "warn";
    return o;
}

}  // namespace

TEST_CASE("hello runs headless and reports state", "[runtime]") {
    auto o = hello_options(120);
    o.capture = root() / "build" / "test-out" / "hello-120.png";
    auto r = app::run(o);
    REQUIRE(r.has_value());
    const Json& rep = *r;
    INFO(rep.dump(2));
    REQUIRE(rep["ok"] == true);
    REQUIRE(rep["frames"] == 120);
    REQUIRE(rep["ticks"] == 120);
    REQUIRE(rep["state"].contains("bounces"));
    REQUIRE(rep["state"]["bounces"].get<int>() >= 1);
    REQUIRE(rep["state"]["ball.y"].get<double>() >= 0.0);
    REQUIRE(rep["state_hash"].get<std::string>().size() == 16);
    REQUIRE(rep["gpu"]["backend"] == "metal");
    REQUIRE(rep["script"]["engine"] == "JavaScriptCore");
    // The capture must show the clear color the script set on the last tick.
    auto px = rep["capture"]["center_pixel"];
    auto cc = rep["clear_color"];
    for (int i = 0; i < 3; ++i) {
        int expected = static_cast<int>(std::lround(cc[i].get<double>() * 255.0));
        REQUIRE(std::abs(px[i].get<int>() - expected) <= 1);
    }
    REQUIRE(px[3] == 255);
    REQUIRE(std::filesystem::exists(o.capture));
}

TEST_CASE("hello is deterministic across runs", "[runtime]") {
    auto a = app::run(hello_options(90));
    auto b = app::run(hello_options(90));
    REQUIRE(a.has_value());
    REQUIRE(b.has_value());
    REQUIRE((*a)["state_hash"] == (*b)["state_hash"]);
    REQUIRE((*a)["state"] == (*b)["state"]);
    auto c = hello_options(90);
    c.seed = 2;
    auto cr = app::run(c);
    REQUIRE(cr.has_value());
    // The seed only feeds random(); hello's exposed state does not depend on it, so the hash
    // must still match. This pins down that the hash covers state, not incidental values.
    REQUIRE((*cr)["state_hash"] == (*a)["state_hash"]);
}

TEST_CASE("golden state hash for hello", "[runtime]") {
    auto golden_path = root() / "tests" / "evidence" / "hello-golden.json";
    auto r = app::run(hello_options(120));
    REQUIRE(r.has_value());
    std::string hash = (*r)["state_hash"].get<std::string>();
    if (std::filesystem::exists(golden_path)) {
        Json golden = Json::parse(fs::read_text(golden_path).value());
        INFO("golden " << golden.dump() << " actual " << (*r)["state"].dump());
        REQUIRE(golden["state_hash"] == hash);
        REQUIRE(golden["state"] == (*r)["state"]);
    } else {
        Json golden;
        golden["note"] = "state hash of samples/hello after 120 headless ticks at 60 Hz; regenerate deliberately when the sample or the hash contract changes";
        golden["frames"] = 120;
        golden["state_hash"] = hash;
        golden["state"] = (*r)["state"];
        REQUIRE(fs::write_text(golden_path, golden.dump(2) + "\n").has_value());
        WARN("wrote new golden file " << golden_path.string());
    }
}

TEST_CASE("script errors are reported, not fatal", "[runtime]") {
    auto o = hello_options(3);
    auto broken = root() / "build" / "test-out" / "broken.js";
    REQUIRE(fs::write_text(broken, "globalThis.__pocket_dispatch = function(kind){ if (kind === 'tick') throw new Error('boom'); };").has_value());
    o.bundle = broken;
    o.project_config = "";
    o.project_config = broken.string() + ".project.json";
    auto r = app::run(o);
    REQUIRE(r.has_value());
    REQUIRE((*r)["ok"] == false);
    REQUIRE((*r)["errors"].size() >= 1);
    REQUIRE((*r)["errors"][0]["code"] == "script_error");
    REQUIRE((*r)["errors"][0]["message"].get<std::string>().find("boom") != std::string::npos);
}
