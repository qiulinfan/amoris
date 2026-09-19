#include <pocket/app/session.hpp>
#include <pocket/audio/audio.hpp>
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

std::unique_ptr<audio::Audio> headless_audio() {
    audio::Config c;
    c.project_dir = root() / "samples" / "audio";
    c.headless = true;
    auto a = audio::Audio::create(c);
    REQUIRE(a.has_value());
    return std::move(*a);
}

}  // namespace

TEST_CASE("clips decode and voices follow the tick", "[audio]") {
    auto a = headless_audio();
    REQUIRE_FALSE(a->has_device());
    REQUIRE(a->load("assets/beep.wav").has_value());
    Json clips = a->clips();
    REQUIRE(clips.size() == 1);
    REQUIRE(clips[0]["seconds"].get<double>() == Catch::Approx(0.3).margin(0.002));
    REQUIRE(clips[0]["source_rate"] == 22050);

    audio::PlayOptions o;
    o.volume = 0.5f;
    o.tag = "fx";
    auto id = a->play("assets/beep.wav", o);
    REQUIRE(id.has_value());
    REQUIRE(a->voices().size() == 1);
    // 10 ticks at 60 Hz: 1/6 s in.
    std::vector<audio::VoiceEvent> events;
    for (int i = 0; i < 10; ++i) for (auto& e : a->tick(1.0 / 60)) events.push_back(e);
    REQUIRE(events.empty());
    REQUIRE(a->voices()[0].position == Catch::Approx(10.0 / 60).margin(1e-6));
    // Past the end: finished, voice gone.
    for (int i = 0; i < 10; ++i) for (auto& e : a->tick(1.0 / 60)) events.push_back(e);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0].type == "audio.finished");
    REQUIRE(events[0].voice.id == *id);
    REQUIRE(events[0].voice.tag == "fx");
    REQUIRE(a->voices().empty());

    // Pitch doubles the rate: the same clip finishes in half the ticks.
    o.pitch = 2.0f;
    REQUIRE(a->play("assets/beep.wav", o).has_value());
    events.clear();
    for (int i = 0; i < 10; ++i) for (auto& e : a->tick(1.0 / 60)) events.push_back(e);
    REQUIRE(events.size() == 1);
}

TEST_CASE("loops report and stops by id, clip and tag", "[audio]") {
    auto a = headless_audio();
    audio::PlayOptions loop;
    loop.loop = true;
    loop.tag = "ambience";
    auto hum = a->play("assets/hum.wav", loop);  // 1 s
    REQUIRE(hum.has_value());
    audio::PlayOptions once;
    once.tag = "fx";
    REQUIRE(a->play("assets/click.wav", once).has_value());
    REQUIRE(a->play("assets/click.wav", once).has_value());
    REQUIRE(a->voices().size() == 3);
    int looped = 0;
    for (int i = 0; i < 150; ++i) for (auto& e : a->tick(1.0 / 60)) if (e.type == "audio.looped") ++looped;
    REQUIRE(looped == 2);  // 2.5 s of a 1 s loop
    REQUIRE(a->voices().size() == 1);  // the clicks finished
    REQUIRE(a->voices()[0].loops_done == 2);
    REQUIRE(a->voices()[0].position == Catch::Approx(0.5).margin(1e-6));
    REQUIRE(a->set(*hum, Json{{"volume", 0.2}, {"loop", false}}).has_value());
    REQUIRE(a->voices()[0].volume == Catch::Approx(0.2f));
    for (int i = 0; i < 40; ++i) (void)a->tick(1.0 / 60);
    REQUIRE(a->voices().empty());  // no longer looping: it ended
    REQUIRE(a->play("assets/hum.wav", loop).has_value());
    REQUIRE(a->play("assets/beep.wav", once).has_value());
    REQUIRE(a->stop_tag("ambience") == 1);
    REQUIRE(a->stop_clip("assets/beep.wav") == 1);
    REQUIRE(a->voices().empty());
    REQUIRE_FALSE(a->play("assets/nope.wav", once).has_value());
    REQUIRE_FALSE(a->play("../../README.md", once).has_value());
    REQUIRE_FALSE(a->set(999, Json::object()).has_value());
}

TEST_CASE("AudioSource components start voices in a headless session", "[audio]") {
    app::Options o;
    o.project_dir = root() / "samples" / "audio";
    o.bundle = root() / "build" / "ts" / "audio.js";
    o.project_config = o.bundle.string() + ".project.json";
    o.headless = true;
    o.width = 64;
    o.height = 64;
    o.log_level = "warn";
    app::Session s(o);
    REQUIRE(s.start().has_value());
    for (int i = 0; i < 100; ++i) REQUIRE(s.frame().has_value());
    Json src = s.command("world.get", Json{{"entity", "Generator"}, {"component", "AudioSource"}}).value();
    INFO(src.dump());
    REQUIRE(src["playing"] == true);
    REQUIRE(src["voice"].get<int>() >= 1);
    Json voices = s.command("audio.list", Json::object()).value();
    INFO(voices.dump());
    bool hum = false;
    for (auto& v : voices) if (v["clip"] == "assets/hum.wav") { hum = true; REQUIRE(v["loop"] == true); REQUIRE(v["entity"].get<std::uint64_t>() > 0); }
    REQUIRE(hum);
    Json st = s.command("state", Json::object()).value();
    REQUIRE(st["state"]["beeps"].get<int>() >= 1);
    REQUIRE(st["state"]["hum.position"].get<double>() > 0.0);
    Json hist = s.command("events.histogram", Json::object()).value();
    INFO(hist.dump());
    REQUIRE(hist["audio.started"].get<int>() >= 2);
    REQUIRE(hist["audio.finished"].get<int>() >= 1);
    Json stats = s.command("audio.stats", Json::object()).value();
    REQUIRE(stats["device"] == "none");
    REQUIRE(stats["clips"].get<int>() >= 2);
    REQUIRE(s.finish().has_value());
    REQUIRE(s.report()["audio"]["plays"].get<int>() >= 2);
}
