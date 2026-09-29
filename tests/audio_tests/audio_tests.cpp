#include <pocket/app/session.hpp>
#include <pocket/audio/audio.hpp>
#include <pocket/core/core.hpp>

#include <catch_amalgamated.hpp>

#include <cmath>
#include <cstdlib>
#include <filesystem>
#include <fstream>

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

TEST_CASE("an Ogg Vorbis clip decodes like a WAV", "[audio][ogg]") {
    auto a = headless_audio();
    REQUIRE(a->load("assets/chime.ogg").has_value());
    Json clips = a->clips();
    REQUIRE(clips.size() == 1);
    INFO(clips.dump());
    REQUIRE(clips[0]["source_rate"] == 22050);
    REQUIRE(clips[0]["source_channels"] == 2);
    REQUIRE(clips[0]["seconds"].get<double>() == Catch::Approx(0.4).margin(0.03));   // the encoder pads a frame or so
    // It plays to its end like any clip.
    audio::PlayOptions o;
    auto id = a->play("assets/chime.ogg", o);
    REQUIRE(id.has_value());
    std::vector<audio::VoiceEvent> events;
    for (int i = 0; i < 20; ++i) for (auto& e : a->tick(1.0 / 60)) events.push_back(e);
    REQUIRE(events.empty());
    REQUIRE(a->voices().size() == 1);
    for (int i = 0; i < 8; ++i) for (auto& e : a->tick(1.0 / 60)) events.push_back(e);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0].type == "audio.finished");
    REQUIRE(a->voices().empty());
    // A file that is not Vorbis is refused by name, not by crashing.
    REQUIRE_FALSE(a->load("assets/beep.wav").has_value() == false);
    auto bad = a->load("assets/nothing.ogg");
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().code == "no_such_asset");
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

TEST_CASE("a streamed Ogg clip mixes like a decoded one, deterministically, and loops without a seam", "[audio][stream]") {
    // The same clip streamed (every Ogg streams) and decoded whole (none does).
    auto make = [&](double stream_seconds) {
        audio::Config c;
        c.project_dir = root() / "samples" / "audio";
        c.headless = true;
        c.stream_seconds = stream_seconds;
        auto a = audio::Audio::create(c);
        REQUIRE(a.has_value());
        return std::move(*a);
    };
    auto streamed = make(0.0), whole = make(1e9), again = make(0.0);
    for (auto* a : {&streamed, &whole, &again}) REQUIRE((*a)->load("assets/chime.ogg").has_value());
    Json sc = streamed->clips(), wc = whole->clips();
    INFO(sc.dump() << " " << wc.dump());
    REQUIRE(sc[0]["streamed"] == true);
    REQUIRE(wc[0]["streamed"] == false);
    REQUIRE(sc[0]["bytes"].get<std::size_t>() < wc[0]["bytes"].get<std::size_t>() / 4);   // the file against the decoded floats
    REQUIRE(std::abs(sc[0]["frames"].get<int>() - wc[0]["frames"].get<int>()) < 200);     // the encoder's padding
    REQUIRE(streamed->describe()["streamed_clips"] == 1);
    // Played once through at pitch 1.25: the two mixes agree sample for sample within a little, and two streamed runs exactly.
    audio::PlayOptions o;
    o.pitch = 1.25f;
    for (auto* a : {&streamed, &whole, &again}) REQUIRE((*a)->play("assets/chime.ogg", o).has_value());
    REQUIRE(streamed->describe()["streaming_voices"] == 1);
    std::vector<float> s_mix, w_mix, a_mix;
    for (int i = 0; i < 5; ++i) {
        const auto& s = streamed->render_frames(4000);
        s_mix.insert(s_mix.end(), s.begin(), s.end());
        const auto& w = whole->render_frames(4000);
        w_mix.insert(w_mix.end(), w.begin(), w.end());
        const auto& g = again->render_frames(4000);
        a_mix.insert(a_mix.end(), g.begin(), g.end());
    }
    REQUIRE(s_mix.size() == w_mix.size());
    REQUIRE(s_mix == a_mix);
    float worst = 0, energy = 0;
    for (std::size_t i = 0; i < s_mix.size(); ++i) { worst = std::max(worst, std::abs(s_mix[i] - w_mix[i])); energy = std::max(energy, std::abs(w_mix[i])); }
    INFO("worst difference " << worst << ", loudest " << energy);
    REQUIRE(energy > 0.05f);
    REQUIRE(worst < 0.05f);
    // Both fall silent after the clip's end (0.4 s at pitch 1.25 is about 15,400 frames of 20,000).
    float tail = 0;
    for (std::size_t i = s_mix.size() - 4000; i < s_mix.size(); ++i) tail = std::max(tail, std::abs(s_mix[i]));
    REQUIRE(tail == 0.0f);
    // A looping streamed voice keeps sounding past the clip's length, and the ticks report its wraps.
    streamed->stop_all();
    o.pitch = 1.0f;
    o.loop = true;
    REQUIRE(streamed->play("assets/chime.ogg", o).has_value());
    float loud_late = 0;
    for (int i = 0; i < 12; ++i) {
        const auto& s = streamed->render_frames(4000);   // 48,000 frames: two and a half times through
        if (i >= 8) for (float x : s) loud_late = std::max(loud_late, std::abs(x));
    }
    REQUIRE(loud_late > 0.05f);
    std::uint32_t looped = 0;
    for (int i = 0; i < 60; ++i) for (auto& e : streamed->tick(1.0 / 60)) if (e.type == "audio.looped") ++looped;
    REQUIRE(looped == 2);
    REQUIRE(streamed->voices().size() == 1);
}

TEST_CASE("a low-passed voice is smoother than the clip and opens up again when set", "[audio][lowpass]") {
    auto plain = headless_audio(), muffled = headless_audio();
    audio::PlayOptions o;
    REQUIRE(plain->play("assets/beep.wav", o).has_value());
    o.lowpass = 0.05f;
    auto id = muffled->play("assets/beep.wav", o);
    REQUIRE(id.has_value());
    REQUIRE(muffled->voices()[0].lowpass == Catch::Approx(0.05));
    // Roughness: how much the signal moves from one frame to the next.
    auto roughness = [](const std::vector<float>& mix) {
        float sum = 0;
        for (std::size_t i = 2; i < mix.size(); i += 2) sum += std::abs(mix[i] - mix[i - 2]);
        return sum;
    };
    auto loudness = [](const std::vector<float>& mix) {
        float peak = 0;
        for (float x : mix) peak = std::max(peak, std::abs(x));
        return peak;
    };
    const std::vector<float> a = plain->render_frames(2400), b = muffled->render_frames(2400);
    INFO("plain " << roughness(a) << " / " << loudness(a) << ", muffled " << roughness(b) << " / " << loudness(b));
    REQUIRE(loudness(a) > 0.1f);
    REQUIRE(loudness(b) > 0.001f);
    REQUIRE(roughness(b) < roughness(a) * 0.3f);
    // Opened again, the next stretch moves like the plain one.
    REQUIRE(muffled->set(*id, Json{{"lowpass", 1.0}}).has_value());
    const std::vector<float> a2 = plain->render_frames(2400), b2 = muffled->render_frames(2400);
    REQUIRE(roughness(b2) > roughness(a2) * 0.8f);
    REQUIRE(roughness(b2) < roughness(a2) * 1.2f);
    // Out of range is clamped, not refused.
    REQUIRE(muffled->set(*id, Json{{"lowpass", 5.0}}).has_value());
    REQUIRE(muffled->voices()[0].lowpass == Catch::Approx(1.0));
}

TEST_CASE("a room rings on after a voice stops, a dry voice does not feed it, and room 0 is silence", "[audio][reverb]") {
    auto dry = headless_audio(), wet = headless_audio(), kept = headless_audio();
    audio::ReverbSettings r;
    r.room = 0.8f;
    r.damping = 0.3f;
    r.mix = 0.5f;
    wet->set_reverb(r);
    kept->set_reverb(r);
    REQUIRE(wet->reverb().room == Catch::Approx(0.8));
    audio::PlayOptions o;
    REQUIRE(dry->play("assets/beep.wav", o).has_value());
    auto id = wet->play("assets/beep.wav", o);
    REQUIRE(id.has_value());
    REQUIRE(wet->voices()[0].reverb == Catch::Approx(1.0));
    o.reverb = 0.0f;
    REQUIRE(kept->play("assets/beep.wav", o).has_value());
    auto peak = [](const std::vector<float>& mix) {
        float p = 0;
        for (float x : mix) p = std::max(p, std::abs(x));
        return p;
    };
    // The beep is three tenths of a second. A tenth at a time: while it plays, every mix has it;
    // after it, the dry mix is silent while the room still rings, and the dry-sent voice's room is silent too.
    float dry_peak[10], wet_peak[10], kept_peak[10];
    for (int i = 0; i < 10; ++i) {
        dry_peak[i] = peak(dry->render_frames(4800));
        wet_peak[i] = peak(wet->render_frames(4800));
        kept_peak[i] = peak(kept->render_frames(4800));
    }
    INFO("dry " << dry_peak[0] << " " << dry_peak[4] << " " << dry_peak[9] << ", wet " << wet_peak[0] << " " << wet_peak[4] << " " << wet_peak[9] << ", kept " << kept_peak[4]);
    REQUIRE(dry_peak[0] > 0.1f);
    REQUIRE(dry_peak[4] == 0.0f);
    REQUIRE(dry_peak[9] == 0.0f);
    REQUIRE(wet_peak[0] > 0.1f);
    REQUIRE(wet_peak[4] > 0.005f);        // the tail
    REQUIRE(wet_peak[4] < wet_peak[0]);
    REQUIRE(wet_peak[9] < wet_peak[4]);   // dying away
    REQUIRE(kept_peak[4] == 0.0f);
    REQUIRE(wet->describe()["reverb"]["ringing"] == true);
    // Room 0: the tail is dropped and the mix is dry again at once.
    r.room = 0;
    wet->set_reverb(r);
    REQUIRE(peak(wet->render_frames(4800)) == 0.0f);
    REQUIRE(wet->describe()["reverb"]["ringing"] == false);
    // A voice's send can change while it plays; the settings clamp.
    auto again = wet->play("assets/hum.wav", audio::PlayOptions{});
    REQUIRE(again.has_value());
    REQUIRE(wet->set(*again, Json{{"reverb", 0.25}}).has_value());
    float send = -1;
    for (const auto& v : wet->voices()) if (v.id == *again) send = v.reverb;
    REQUIRE(send == Catch::Approx(0.25));
    r.room = 3;
    r.mix = -1;
    wet->set_reverb(r);
    REQUIRE(wet->reverb().room == Catch::Approx(1.0));
    REQUIRE(wet->reverb().mix == Catch::Approx(0.0));
}

TEST_CASE("an MP3 clip decodes whole or streams, like an Ogg", "[audio][mp3]") {
    auto make = [&](double stream_seconds) {
        audio::Config c;
        c.project_dir = root() / "samples" / "audio";
        c.headless = true;
        c.stream_seconds = stream_seconds;
        auto a = audio::Audio::create(c);
        REQUIRE(a.has_value());
        return std::move(*a);
    };
    auto whole = make(1e9), streamed = make(0.0);
    for (auto* a : {&whole, &streamed}) REQUIRE((*a)->load("assets/blip.mp3").has_value());
    Json wc = whole->clips(), sc = streamed->clips();
    INFO(wc.dump() << " " << sc.dump());
    REQUIRE(wc[0]["source_rate"] == 44100);
    REQUIRE(wc[0]["source_channels"] == 1);
    REQUIRE(wc[0]["seconds"].get<double>() == Catch::Approx(0.4).margin(0.06));   // the encoder's padding
    REQUIRE(wc[0]["streamed"] == false);
    REQUIRE(sc[0]["streamed"] == true);
    // Played through, both sound, agree closely, and end.
    audio::PlayOptions o;
    for (auto* a : {&whole, &streamed}) REQUIRE((*a)->play("assets/blip.mp3", o).has_value());
    std::vector<float> w_mix, s_mix;
    for (int i = 0; i < 6; ++i) {
        const auto& w = whole->render_frames(4000);
        w_mix.insert(w_mix.end(), w.begin(), w.end());
        const auto& s = streamed->render_frames(4000);
        s_mix.insert(s_mix.end(), s.begin(), s.end());
    }
    float loud = 0, worst = 0;
    for (std::size_t i = 0; i < w_mix.size(); ++i) { loud = std::max(loud, std::abs(w_mix[i])); worst = std::max(worst, std::abs(w_mix[i] - s_mix[i])); }
    INFO("loudest " << loud << ", worst difference " << worst);
    REQUIRE(loud > 0.1f);
    REQUIRE(worst < 0.05f);
    float tail = 0;
    for (std::size_t i = w_mix.size() - 4000; i < w_mix.size(); ++i) tail = std::max(tail, std::abs(w_mix[i]));
    REQUIRE(tail == 0.0f);
    // Something that is not an MP3 is refused by name.
    std::ofstream(root() / "samples" / "audio" / "assets" / "fake.mp3") << "not an mp3 at all";
    auto bad = whole->load("assets/fake.mp3");
    std::filesystem::remove(root() / "samples" / "audio" / "assets" / "fake.mp3");
    REQUIRE_FALSE(bad.has_value());
    REQUIRE(bad.error().code == "bad_audio");
}
