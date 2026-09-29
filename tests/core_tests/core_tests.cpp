#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <algorithm>
#include <bit>
#include <cstdlib>
#include <filesystem>
#include <limits>

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

TEST_CASE("Mat4 inverse undoes projections", "[core]") {
    Mat4 p = Mat4::perspective(radians(60.0f), 1.5f, 0.1f, 100.0f) * Mat4::look_at({1, 2, 3}, {0, 0, 0}, {0, 1, 0});
    Mat4 id = p * p.inverse();
    for (int c = 0; c < 4; ++c)
        for (int r = 0; r < 4; ++r) REQUIRE(id.at(c, r) == Catch::Approx(c == r ? 1.0f : 0.0f).margin(1e-4));
    Mat4 o = Mat4::orthographic(-2, 2, -1, 1, 0.1f, 10.0f);
    Vec4 v = o.inverse() * (o * Vec4{0.5f, -0.25f, -3.0f, 1.0f});
    REQUIRE(v.x == Catch::Approx(0.5f));
    REQUIRE(v.y == Catch::Approx(-0.25f));
    REQUIRE(v.z == Catch::Approx(-3.0f));
    Mat4 singular;
    for (float& f : singular.m) f = 0;
    REQUIRE(singular.inverse().at(0, 0) == 1.0f);  // identity when there is no inverse
}

namespace {

// How many floats apart two floats are (0 the same).
std::int64_t ulps(float a, float b) {
    auto key = [](float f) {
        const auto i = std::bit_cast<std::int32_t>(f);
        return i < 0 ? std::int64_t{INT32_MIN} - i : std::int64_t{i};
    };
    return std::llabs(key(a) - key(b));
}

}  // namespace

TEST_CASE("Reproducible math is as close as libm and pinned to the bit", "[core][repro]") {
    // Each function against the double libm rounded to float, over a sweep; the sweep's results
    // fold into a hash pinned here, so a build whose results differ in one bit fails (a web build
    // and a native one must agree, docs/design/networking.md).
    StateHasher h;
    std::int64_t worst_trig = 0, worst_inv = 0, worst_exp = 0;
    for (int i = -200000; i <= 200000; ++i) {
        const float x = static_cast<float>(i) * 0.00049f;
        const float s = repro::sin(x), c = repro::cos(x);
        worst_trig = std::max({worst_trig, ulps(s, static_cast<float>(std::sin(static_cast<double>(x)))), ulps(c, static_cast<float>(std::cos(static_cast<double>(x))))});
        float s2, c2;
        repro::sincos(x, s2, c2);
        REQUIRE(s2 == s);
        REQUIRE(c2 == c);
        h.f32(s);
        h.f32(c);
    }
    for (float x : {1000.5f, -31415.9f, 123456.7f, 1e-8f, -0.0f}) {
        worst_trig = std::max(worst_trig, ulps(repro::sin(x), static_cast<float>(std::sin(static_cast<double>(x)))));
        h.f32(repro::sin(x));
    }
    for (int i = -60; i <= 60; ++i) {
        for (int j = -60; j <= 60; ++j) {
            const float y = static_cast<float>(i) * 0.37f, x = static_cast<float>(j) * 0.41f;
            const float a = repro::atan2(y, x);
            worst_inv = std::max(worst_inv, ulps(a, static_cast<float>(std::atan2(static_cast<double>(y), static_cast<double>(x)))));
            const float hy = repro::hypot(x, y);
            REQUIRE(ulps(hy, static_cast<float>(std::hypot(static_cast<double>(x), static_cast<double>(y)))) <= 1);
            h.f32(a);
            h.f32(hy);
        }
    }
    for (int i = -1000; i <= 1000; ++i) {
        const float x = static_cast<float>(i) / 1000.0f;
        worst_inv = std::max({worst_inv, ulps(repro::asin(x), static_cast<float>(std::asin(static_cast<double>(x)))), ulps(repro::acos(x), static_cast<float>(std::acos(static_cast<double>(x)))),
                              ulps(repro::atan(x * 50), static_cast<float>(std::atan(static_cast<double>(x * 50))))});
        h.f32(repro::asin(x));
        h.f32(repro::acos(x));
    }
    for (int i = -8000; i <= 8000; ++i) {
        const float x = static_cast<float>(i) * 0.01f;
        worst_exp = std::max(worst_exp, ulps(repro::exp(x), static_cast<float>(std::exp(static_cast<double>(x)))));
        const float l = std::ldexp(1.0f + static_cast<float>(i + 8000) / 16001.0f, i / 300);
        worst_exp = std::max(worst_exp, ulps(repro::log(l), static_cast<float>(std::log(static_cast<double>(l)))));
        worst_exp = std::max(worst_exp, ulps(repro::cbrt(x * 13), static_cast<float>(std::cbrt(static_cast<double>(x * 13)))));
        h.f32(repro::exp(x));
        h.f32(repro::log(l));
        h.f32(repro::cbrt(x * 13));
    }
    for (int i = 0; i <= 200; ++i) {
        for (int j = -40; j <= 40; ++j) {
            const float b = static_cast<float>(i) * 0.05f, e = static_cast<float>(j) * 0.25f;
            worst_exp = std::max(worst_exp, ulps(repro::pow(b, e), static_cast<float>(std::pow(static_cast<double>(b), static_cast<double>(e)))));
            h.f32(repro::pow(b, e));
        }
    }
    CHECK(worst_trig <= 1);
    CHECK(worst_inv <= 1);
    CHECK(worst_exp <= 1);
    // Edges as <cmath> has them.
    CHECK(repro::pow(-2.0f, 3.0f) == -8.0f);
    CHECK(repro::pow(-2.0f, 2.0f) == 4.0f);
    CHECK(std::isnan(repro::pow(-2.0f, 0.5f)));
    CHECK(repro::pow(0.0f, -1.0f) == std::numeric_limits<float>::infinity());
    CHECK(repro::pow(5.0f, 0.0f) == 1.0f);
    CHECK(repro::atan2(0.0f, -1.0f) == std::numbers::pi_v<float>);
    CHECK(repro::atan2(-0.0f, -1.0f) == -std::numbers::pi_v<float>);
    CHECK(repro::atan2(1.0f, 0.0f) == std::numbers::pi_v<float> / 2);
    CHECK(repro::exp(0.0f) == 1.0f);
    CHECK(repro::log(1.0f) == 0.0f);
    CHECK(repro::cbrt(-27.0f) == -3.0f);
    CHECK(std::isnan(repro::sin(std::numeric_limits<float>::infinity())));
    CHECK(std::isnan(repro::asin(1.5f)));
    INFO("sweep hash " << hex64(h.digest()));
    CHECK(hex64(h.digest()) == "62265331655f6ff2");
}
