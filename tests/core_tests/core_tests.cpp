#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cstdlib>
#include <filesystem>

using namespace pocket;

TEST_CASE("Random is deterministic per seed", "[core]") {
    Random a(42), b(42), c(43);
    for (int i = 0; i < 1000; ++i) {
        REQUIRE(a.next_u32() == b.next_u32());
    }
    REQUIRE(a.next_u32() != c.next_u32());
    Random d(7);
    for (int i = 0; i < 10000; ++i) {
        double v = d.next_double();
        REQUIRE(v >= 0.0);
        REQUIRE(v < 1.0);
        REQUIRE(d.next_below(10) < 10);
    }
}

TEST_CASE("StateHasher is order sensitive and stable", "[core]") {
    StateHasher a, b, c;
    a.i64(1); a.f32(2.5f); a.str("x");
    b.i64(1); b.f32(2.5f); b.str("x");
    c.str("x"); c.i64(1); c.f32(2.5f);
    REQUIRE(a.digest() == b.digest());
    REQUIRE(a.digest() != c.digest());
    REQUIRE(hex64(a.digest()).size() == 16);
    REQUIRE(fnv1a("hello") != fnv1a("hellp"));
}

TEST_CASE("TickClock produces fixed ticks", "[core]") {
    TickClock clock;
    clock.tick_seconds = 1.0 / 60.0;
    int total = 0;
    for (int i = 0; i < 60; ++i) total += clock.advance(1.0 / 60.0);
    REQUIRE(total >= 59);
    REQUIRE(total <= 60);
    REQUIRE(clock.advance(10.0) == clock.max_ticks_per_frame);
    REQUIRE(clock.accumulator == 0.0);
    REQUIRE(clock.alpha() >= 0.0);
    REQUIRE(clock.alpha() < 1.0);
}

TEST_CASE("Math basics", "[core]") {
    Vec3 p{1, 2, 3};
    Mat4 t = Mat4::translation({10, 0, 0});
    Vec3 q = t.transform_point(p);
    REQUIRE(q.x == Catch::Approx(11));
    REQUIRE(q.y == Catch::Approx(2));
    Quat r = Quat::from_axis_angle({0, 1, 0}, radians(90));
    Vec3 f = r.rotate({0, 0, -1});
    REQUIRE(f.x == Catch::Approx(-1).margin(1e-5));
    REQUIRE(f.z == Catch::Approx(0).margin(1e-5));
    Mat4 m = Mat4::trs({1, 2, 3}, r, {2, 2, 2});
    Mat4 inv = m.inverse_affine();
    Vec3 back = inv.transform_point(m.transform_point(p));
    REQUIRE(back.x == Catch::Approx(p.x).margin(1e-4));
    REQUIRE(back.y == Catch::Approx(p.y).margin(1e-4));
    REQUIRE(back.z == Catch::Approx(p.z).margin(1e-4));
    Mat4 proj = Mat4::perspective(radians(60), 16.0f / 9.0f, 0.1f, 100.0f);
    Vec4 c = proj * Vec4{0, 0, -0.1f, 1};
    REQUIRE(c.z / c.w == Catch::Approx(0).margin(1e-5));
}

TEST_CASE("Result and errors", "[core]") {
    auto f = [](int x) -> Result<int> {
        if (x < 0) return fail("negative", "x was {}", x);
        return x * 2;
    };
    auto g = [&](int x) -> Result<int> {
        POCKET_TRY(v, f(x));
        return v + 1;
    };
    REQUIRE(g(2).value() == 5);
    REQUIRE_FALSE(g(-1).has_value());
    REQUIRE(g(-1).error().code == "negative");
    REQUIRE(g(-1).error().message == "x was -1");
}

TEST_CASE("Logger ring and JSON records", "[core]") {
    log::Logger logger;
    logger.set_ring_capacity(4);
    logger.set_level(log::Level::Debug);
    std::vector<log::Record> seen;
    logger.add_sink([&](const log::Record& r) { seen.push_back(r); });
    for (int i = 0; i < 6; ++i) logger.emit(log::Level::Info, "test", std::format("m{}", i), Json{{"i", i}});
    logger.emit(log::Level::Trace, "test", "dropped");
    REQUIRE(seen.size() == 6);
    auto recent = logger.recent(10);
    REQUIRE(recent.size() == 4);
    REQUIRE(recent.front().message == "m2");
    REQUIRE(recent.back().message == "m5");
    Json j = log::to_json(recent.back());
    REQUIRE(j["level"] == "info");
    REQUIRE(j["fields"]["i"] == 5);
    REQUIRE(log::level_from_name("warn") == log::Level::Warn);
}

TEST_CASE("Filesystem helpers", "[core]") {
    auto dir = std::filesystem::temp_directory_path() / "pocket_core_tests";
    auto file = dir / "a" / "b.txt";
    REQUIRE(fs::write_text(file, "hello").has_value());
    REQUIRE(fs::read_text(file).value() == "hello");
    REQUIRE(fs::read_bytes(file).value().size() == 5);
    REQUIRE_FALSE(fs::read_text(dir / "missing").has_value());
    std::filesystem::remove_all(dir);
}
