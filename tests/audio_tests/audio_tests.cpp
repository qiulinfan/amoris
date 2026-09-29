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
    // The project's buses: the hum is on ambience, which ducked under each beep on fx.
    for (auto& v : voices) if (v["clip"] == "assets/hum.wav") REQUIRE(v["bus"] == "ambience");
    Json buses = s.command("audio.buses", Json::object()).value();
    INFO(buses.dump());
    bool ambience = false;
    for (auto& b : buses) if (b["name"] == "ambience") { ambience = true; REQUIRE(b["duck_by"] == "fx"); REQUIRE(b["duck_amount"].get<double>() == Catch::Approx(0.5)); }
    REQUIRE(ambience);
    REQUIRE(hist["audio.ducked"].get<int>() >= 2);   // down with the first beep, up after it
    REQUIRE(s.finish().has_value());
    REQUIRE(s.report()["audio"]["plays"].get<int>() >= 2);
}

TEST_CASE("a bus sets its voices as one: volume, mute, a low-pass, and ducking on the tick clock", "[audio][buses]") {
    auto loudness = [](const std::vector<float>& mix) {
        float peak = 0;
        for (float x : mix) peak = std::max(peak, std::abs(x));
        return peak;
    };
    auto roughness = [](const std::vector<float>& mix) {
        float sum = 0;
        for (std::size_t i = 2; i < mix.size(); i += 2) sum += std::abs(mix[i] - mix[i - 2]);
        return sum;
    };
    auto plain = headless_audio(), music = headless_audio();
    audio::PlayOptions o;
    o.reverb = 0;
    REQUIRE(plain->play("assets/beep.wav", o).has_value());
    o.bus = "music";
    REQUIRE(music->play("assets/beep.wav", o).has_value());
    REQUIRE(music->voices()[0].bus == "music");
    audio::BusSettings half;
    half.volume = 0.5f;
    music->set_bus("music", half);
    std::vector<float> a = plain->render_frames(2400), b = music->render_frames(2400);
    INFO("plain " << loudness(a) << ", music at half " << loudness(b));
    REQUIRE(loudness(a) > 0.1f);
    REQUIRE(loudness(b) == Catch::Approx(loudness(a) * 0.5f).epsilon(0.02));
    // Muted: the change ramps across one slice, then silence while the voice plays on.
    audio::BusSettings muted = half;
    muted.muted = true;
    music->set_bus("music", muted);
    (void)plain->render_frames(480);
    (void)music->render_frames(480);
    a = plain->render_frames(2400);
    b = music->render_frames(2400);
    REQUIRE(loudness(a) > 0.1f);
    REQUIRE(loudness(b) == 0.0f);
    REQUIRE(music->voices().size() == 1);
    // A low-pass over the bus muffles every voice on it.
    audio::BusSettings muffled;
    muffled.lowpass = 0.05f;
    music->set_bus("music", muffled);
    (void)plain->render_frames(480);
    (void)music->render_frames(480);
    a = plain->render_frames(2400);
    b = music->render_frames(2400);
    INFO("plain " << roughness(a) << ", muffled bus " << roughness(b));
    REQUIRE(loudness(b) > 0.001f);
    REQUIRE(roughness(b) < roughness(a) * 0.3f);
    REQUIRE(music->stop_bus("music") == 1);

    // Ducking: music under dialogue falls to a quarter over half a second while a line plays,
    // and comes back the same way once it stops, with an event each way.
    auto d = headless_audio();
    audio::BusSettings under;
    under.duck_by = "dialogue";
    under.duck_amount = 0.25f;
    under.duck_seconds = 0.5f;
    d->set_bus("music", under);
    audio::PlayOptions m;
    m.bus = "music";
    m.loop = true;
    REQUIRE(d->play("assets/hum.wav", m).has_value());
    auto bus = [&](const std::string& name) {
        for (const audio::BusInfo& i : d->buses()) if (i.name == name) return i;
        FAIL("no bus " << name);
        return audio::BusInfo{};
    };
    std::vector<audio::VoiceEvent> events;
    auto ticks = [&](int n) { for (int i = 0; i < n; ++i) for (auto& e : d->tick(1.0 / 60)) if (e.type == "audio.ducked") events.push_back(e); };
    ticks(5);
    REQUIRE(bus("music").duck == 1.0f);
    REQUIRE(events.empty());
    audio::PlayOptions line;
    line.bus = "dialogue";
    line.loop = true;
    auto said = d->play("assets/beep.wav", line);
    REQUIRE(said.has_value());
    ticks(15);
    REQUIRE(events.size() == 1);
    REQUIRE(events[0].type == "audio.ducked");
    REQUIRE(events[0].bus == "music");
    REQUIRE(events[0].ducked);
    REQUIRE(bus("music").ducked);
    REQUIRE(bus("music").duck == Catch::Approx(1.0 - 0.75 * 0.25 / 0.5).margin(0.01));   // a quarter second in: half way down
    REQUIRE(bus("dialogue").voices == 1);
    ticks(30);
    REQUIRE(bus("music").duck == Catch::Approx(0.25));
    // The ducked music is quieter in the mix than it was.
    REQUIRE(d->stop(*said) == 1);
    const float ducked_peak = loudness(d->render_frames(2400));
    ticks(40);
    REQUIRE(events.size() == 2);
    REQUIRE_FALSE(events[1].ducked);
    REQUIRE(bus("music").duck == 1.0f);
    (void)d->render_frames(480);
    const float open_peak = loudness(d->render_frames(2400));
    INFO("ducked " << ducked_peak << ", back " << open_peak);
    REQUIRE(ducked_peak < open_peak * 0.35f);
    // A bus never ducks under itself; out-of-range settings are clamped.
    audio::BusSettings odd;
    odd.duck_by = "music";
    odd.volume = 9.0f;
    d->set_bus("music", odd);
    REQUIRE(d->bus("music").duck_by.empty());
    REQUIRE(d->bus("music").volume == 2.0f);
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

TEST_CASE("a bus thins with a high-pass, echoes with feedback and rings on, and sends to the room by its reverb", "[audio][buses][busfx]") {
    const int rate = audio::Config{}.sample_rate;
    auto peak_in = [](const std::vector<float>& mix, std::size_t from, std::size_t to) {
        float p = 0;
        for (std::size_t i = from * 2; i < std::min(to * 2, mix.size()); ++i) p = std::max(p, std::abs(mix[i]));
        return p;
    };
    // A high-pass: the hum's low body goes (a slow average of it), its edges stay.
    auto lows = [](const std::vector<float>& mix) {
        float lp = 0, sum = 0;
        for (std::size_t i = 0; i < mix.size(); i += 2) { lp += 0.002f * (mix[i] - lp); sum += std::abs(lp); }
        return sum;
    };
    auto plain = headless_audio(), thin = headless_audio();
    audio::PlayOptions o;
    o.reverb = 0;
    o.bus = "radio";
    REQUIRE(plain->play("assets/hum.wav", o).has_value());
    REQUIRE(thin->play("assets/hum.wav", o).has_value());
    audio::BusSettings radio;
    radio.highpass = 0.3f;
    thin->set_bus("radio", radio);
    REQUIRE(thin->bus("radio").highpass == Catch::Approx(0.3f));
    const std::vector<float> a = plain->render_frames(rate / 2), b = thin->render_frames(rate / 2);
    INFO("lows plain " << lows(a) << ", thinned " << lows(b));
    REQUIRE(lows(b) < lows(a) * 0.3f);
    REQUIRE(peak_in(b, 0, rate / 2) > 0.05f);
    // An echo a quarter second apart, half of each repeat fed back, at full mix: the click, then
    // itself again at 0.25 s, half as loud at 0.5 s, a quarter at 0.75 s, long after the 30 ms click ended.
    auto canyon = headless_audio();
    audio::BusSettings e;
    e.echo = 0.25f;
    e.echo_feedback = 0.5f;
    e.echo_mix = 1.0f;
    canyon->set_bus("canyon", e);
    o.bus = "canyon";
    REQUIRE(canyon->play("assets/click.wav", o).has_value());
    const std::vector<float> m = canyon->render_frames(rate);
    const auto q = static_cast<std::size_t>(rate / 4);
    const float first = peak_in(m, 0, q / 2), gap = peak_in(m, q / 2, q - 10), second = peak_in(m, q, q + q / 2), third = peak_in(m, 2 * q, 2 * q + q / 2), fourth = peak_in(m, 3 * q, 3 * q + q / 2);
    INFO("click " << first << ", gap " << gap << ", echoes " << second << " " << third << " " << fourth);
    REQUIRE(first > 0.1f);
    REQUIRE(gap < first * 0.01f);
    REQUIRE(second == Catch::Approx(first).epsilon(0.1));
    REQUIRE(third == Catch::Approx(first * 0.5f).epsilon(0.1));
    REQUIRE(fourth == Catch::Approx(first * 0.25f).epsilon(0.1));
    // Echo off: it stops at once.
    e.echo = 0;
    canyon->set_bus("canyon", e);
    REQUIRE(peak_in(canyon->render_frames(rate / 2), 0, rate / 2) == 0.0f);
    // The room hears a bus by its reverb: none at 0, so no tail after the beep.
    audio::ReverbSettings r;
    r.room = 0.8f;
    r.mix = 0.5f;
    auto open = headless_audio(), closed = headless_audio();
    open->set_reverb(r);
    closed->set_reverb(r);
    audio::BusSettings dry;
    dry.reverb = 0;
    closed->set_bus("sfx", dry);
    o.bus = "sfx";
    o.reverb = 1;
    REQUIRE(open->play("assets/beep.wav", o).has_value());
    REQUIRE(closed->play("assets/beep.wav", o).has_value());
    (void)open->render_frames(rate / 2);
    (void)closed->render_frames(rate / 2);
    const float open_tail = peak_in(open->render_frames(rate / 4), 0, rate / 4), closed_tail = peak_in(closed->render_frames(rate / 4), 0, rate / 4);
    INFO("tails " << open_tail << " and " << closed_tail);
    REQUIRE(open_tail > 1e-3f);
    REQUIRE(closed_tail == 0.0f);
}
