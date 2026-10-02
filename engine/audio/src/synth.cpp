// The synthesizer behind `.sfx` recipes: one or more voices, each a wave whose pitch slides
// exponentially from `frequency` to `to` and steps by `steps`, under an attack-hold-decay
// envelope, with vibrato, a duty for squares, sample-and-hold noise and a one-pole low-pass;
// voices are mixed. Presets give the parameters of common game sounds, varied by a seed through
// a small generator of their own, so the same recipe always renders the same samples.
#include "synth.hpp"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <format>
#include <string>

namespace pocket::audio {
namespace {

constexpr double kPi = 3.14159265358979323846;

struct Step {
    double at = 0;      // seconds into the sound
    double times = 1;   // the pitch from then on, times the slide's
};

struct Voice {
    std::string wave = "square";
    double frequency = 440, to = -1;   // to < 0: no slide
    double duty = 0.5;
    double attack = 0.005, hold = 0.05, decay = 0.2;
    double volume = 0.5;
    double vibrato_depth = 0, vibrato_rate = 0;
    double lowpass = 0;                  // Hz; 0 for none
    double delay = 0;                    // seconds before the voice starts
    std::vector<Step> steps;
};

// A seeded generator for presets' variation and noise (xorshift; the same numbers everywhere).
struct Rng {
    std::uint64_t s;
    explicit Rng(std::uint64_t seed) : s(seed * 0x9E3779B97F4A7C15ull + 0x2545F4914F6CDD1Dull) { if (!s) s = 1; }
    double next() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        return static_cast<double>(s >> 11) / 9007199254740992.0;
    }
    double around(double v, double spread) { return v * (1.0 + (next() * 2.0 - 1.0) * spread); }
};

const std::vector<std::string> kPresets = {"coin", "jump", "hit", "explosion", "laser", "powerup", "blip", "hurt", "select", "shot", "step", "click"};

// A preset's voices, varied by the seed (seed 0: as listed).
Result<std::vector<Json>> preset(const std::string& name, std::uint64_t seed) {
    Rng r(seed);
    const double v = seed == 0 ? 0.0 : 0.12;   // how much a seed moves pitches and times
    auto f = [&](double x) { return seed == 0 ? x : r.around(x, v); };
    if (name == "coin") return std::vector<Json>{Json{{"wave", "square"}, {"frequency", f(988)}, {"steps", Json::array({Json{{"at", f(0.06)}, {"times", 1.335}}})}, {"attack", 0.002}, {"hold", f(0.08)}, {"decay", f(0.22)}, {"volume", 0.4}}};
    if (name == "jump") return std::vector<Json>{Json{{"wave", "square"}, {"duty", 0.3}, {"frequency", f(260)}, {"to", f(640)}, {"attack", 0.004}, {"hold", f(0.06)}, {"decay", f(0.16)}, {"volume", 0.4}}};
    if (name == "hit") return std::vector<Json>{Json{{"wave", "noise"}, {"frequency", f(1800)}, {"to", f(400)}, {"attack", 0.001}, {"hold", 0.02}, {"decay", f(0.12)}, {"volume", 0.5}},
                                                Json{{"wave", "square"}, {"frequency", f(180)}, {"to", f(60)}, {"attack", 0.001}, {"hold", 0.01}, {"decay", f(0.12)}, {"volume", 0.35}}};
    if (name == "explosion") return std::vector<Json>{Json{{"wave", "noise"}, {"frequency", f(1100)}, {"to", f(90)}, {"attack", 0.004}, {"hold", f(0.12)}, {"decay", f(0.7)}, {"lowpass", f(2600)}, {"volume", 0.7}},
                                                      Json{{"wave", "sine"}, {"frequency", f(70)}, {"to", f(35)}, {"attack", 0.004}, {"hold", 0.1}, {"decay", f(0.5)}, {"volume", 0.5}}};
    if (name == "laser") return std::vector<Json>{Json{{"wave", "saw"}, {"frequency", f(1500)}, {"to", f(280)}, {"attack", 0.002}, {"hold", f(0.04)}, {"decay", f(0.14)}, {"volume", 0.35}}};
    if (name == "powerup") return std::vector<Json>{Json{{"wave", "square"}, {"frequency", f(330)}, {"to", f(660)}, {"steps", Json::array({Json{{"at", 0.08}, {"times", 1.26}}, Json{{"at", 0.16}, {"times", 1.5}}})}, {"vibrato", Json{{"depth", 0.03}, {"rate", 14}}}, {"attack", 0.005}, {"hold", f(0.24)}, {"decay", f(0.25)}, {"volume", 0.35}}};
    if (name == "blip") return std::vector<Json>{Json{{"wave", "square"}, {"frequency", f(880)}, {"attack", 0.002}, {"hold", f(0.03)}, {"decay", f(0.05)}, {"volume", 0.3}}};
    if (name == "hurt") return std::vector<Json>{Json{{"wave", "saw"}, {"frequency", f(420)}, {"to", f(140)}, {"vibrato", Json{{"depth", 0.08}, {"rate", 30}}}, {"attack", 0.002}, {"hold", f(0.05)}, {"decay", f(0.2)}, {"volume", 0.4}}};
    if (name == "shot") return std::vector<Json>{Json{{"wave", "noise"}, {"frequency", f(4200)}, {"to", f(700)}, {"attack", 0.0005}, {"hold", 0.008}, {"decay", f(0.16)}, {"lowpass", f(7000)}, {"volume", 0.6}},
                                                 Json{{"wave", "sine"}, {"frequency", f(150)}, {"to", f(50)}, {"attack", 0.001}, {"hold", 0.02}, {"decay", f(0.14)}, {"volume", 0.45}}};
    if (name == "step") return std::vector<Json>{Json{{"wave", "noise"}, {"frequency", f(900)}, {"to", f(300)}, {"attack", 0.002}, {"hold", 0.01}, {"decay", f(0.07)}, {"lowpass", f(1600)}, {"volume", 0.25}}};
    if (name == "click") return std::vector<Json>{Json{{"wave", "square"}, {"frequency", f(1800)}, {"attack", 0.0005}, {"hold", 0.004}, {"decay", f(0.02)}, {"volume", 0.25}}};
    if (name == "select") return std::vector<Json>{Json{{"wave", "triangle"}, {"frequency", f(660)}, {"steps", Json::array({Json{{"at", 0.04}, {"times", 1.5}}})}, {"attack", 0.002}, {"hold", 0.05}, {"decay", f(0.1)}, {"volume", 0.4}}};
    std::string names;
    for (const auto& n : kPresets) names += (names.empty() ? "" : ", ") + n;
    return fail("bad_audio", "no preset '{}' (presets: {})", name, names);
}

Result<Voice> read_voice(const Json& j) {
    // A field the synthesizer does not have is refused, not passed over: what it was meant to do
    // would not happen.
    static const std::vector<std::string> kFields = {"wave", "frequency", "to", "duty", "attack", "hold", "decay", "volume", "lowpass", "delay", "vibrato", "steps", "preset", "seed"};
    for (const auto& [key, value] : j.items()) {
        if (std::find(kFields.begin(), kFields.end(), key) != kFields.end()) continue;
        std::string list;
        for (std::size_t i = 0; i + 2 < kFields.size(); ++i) list += (list.empty() ? "" : ", ") + kFields[i];
        return fail("bad_audio", "'{}' is not a field of a voice ({}){}", key, list, key == "reverb" || key == "echo" || key == "pan" ? "; reverb, echo and pan belong to the bus, the room or audio.play" : "");
    }
    Voice v;
    auto num = [&](const char* key, double& out, double lo, double hi) -> Status {
        if (!j.contains(key)) return {};
        if (!j[key].is_number()) return fail("bad_audio", "'{}' is a number", key);
        out = j[key].get<double>();
        if (!(out >= lo && out <= hi)) return fail("bad_audio", "'{}' is {}, not between {} and {}", key, out, lo, hi);
        return {};
    };
    if (j.contains("wave")) {
        if (!j["wave"].is_string()) return fail("bad_audio", "'wave' is sine, square, triangle, saw or noise");
        v.wave = j["wave"].get<std::string>();
        if (v.wave != "sine" && v.wave != "square" && v.wave != "triangle" && v.wave != "saw" && v.wave != "noise") return fail("bad_audio", "'wave' is sine, square, triangle, saw or noise, not '{}'", v.wave);
    }
    POCKET_TRY_VOID(num("frequency", v.frequency, 1, 20000));
    POCKET_TRY_VOID(num("to", v.to, 1, 20000));
    POCKET_TRY_VOID(num("duty", v.duty, 0.01, 0.99));
    POCKET_TRY_VOID(num("attack", v.attack, 0, 10));
    POCKET_TRY_VOID(num("hold", v.hold, 0, 30));
    POCKET_TRY_VOID(num("decay", v.decay, 0, 30));
    POCKET_TRY_VOID(num("volume", v.volume, 0, 1));
    POCKET_TRY_VOID(num("lowpass", v.lowpass, 0, 20000));
    POCKET_TRY_VOID(num("delay", v.delay, 0, 10));
    if (j.contains("vibrato")) {
        const Json& vb = j["vibrato"];
        if (!vb.is_object()) return fail("bad_audio", "'vibrato' is {{depth, rate}}");
        v.vibrato_depth = vb.value("depth", 0.0);
        v.vibrato_rate = vb.value("rate", 0.0);
    }
    if (j.contains("steps")) {
        if (!j["steps"].is_array()) return fail("bad_audio", "'steps' is a list of {{at, times}}");
        for (const Json& s : j["steps"]) v.steps.push_back(Step{s.value("at", 0.0), s.value("times", 1.0)});
        std::sort(v.steps.begin(), v.steps.end(), [](const Step& a, const Step& b) { return a.at < b.at; });
    }
    return v;
}

void render(const Voice& v, int rate, std::uint64_t seed, std::vector<float>& out) {
    const double length = v.attack + v.hold + v.decay;
    const auto start = static_cast<std::size_t>(std::llround(v.delay * rate));
    const auto frames = static_cast<std::size_t>(std::llround(length * rate));
    if (out.size() < start + frames) out.resize(start + frames, 0.0f);
    Rng noise(seed + 0x51u);
    double phase = 0, held = 0, low = 0;
    const double alpha = v.lowpass > 0 ? 1.0 - std::exp(-2.0 * kPi * v.lowpass / rate) : 1.0;
    for (std::size_t i = 0; i < frames; ++i) {
        const double t = static_cast<double>(i) / rate;
        // The pitch: an exponential slide over the sound, its steps, the vibrato.
        double f = v.frequency;
        if (v.to > 0 && length > 0) f = v.frequency * std::pow(v.to / v.frequency, t / length);
        for (const Step& s : v.steps) if (t >= s.at) f *= s.times;
        if (v.vibrato_depth > 0) f *= 1.0 + v.vibrato_depth * std::sin(2.0 * kPi * v.vibrato_rate * t);
        const double before = phase;
        phase += f / rate;
        phase -= std::floor(phase);
        double s = 0;
        if (v.wave == "sine") s = std::sin(2.0 * kPi * phase);
        else if (v.wave == "square") s = phase < v.duty ? 1.0 : -1.0;
        else if (v.wave == "triangle") s = phase < 0.5 ? 4.0 * phase - 1.0 : 3.0 - 4.0 * phase;
        else if (v.wave == "saw") s = 2.0 * phase - 1.0;
        else {
            // Noise held for a period of the pitch: higher sounds hiss, lower ones rumble.
            if (phase < before || i == 0) held = noise.next() * 2.0 - 1.0;
            s = held;
        }
        // Attack up, hold, then a decay that falls away (squared), to nothing at its end.
        double env = 1.0;
        if (t < v.attack) env = v.attack > 0 ? t / v.attack : 1.0;
        else if (t > v.attack + v.hold) {
            const double d = v.decay > 0 ? 1.0 - (t - v.attack - v.hold) / v.decay : 0.0;
            env = std::max(0.0, d) * std::max(0.0, d);
        }
        low += alpha * (s - low);
        out[start + i] += static_cast<float>(low * env * v.volume);
    }
}

}  // namespace

std::vector<std::string> synth_presets() { return kPresets; }

namespace {

// "C4" (middle C), "F#5", "Bb2", "A4" (440 Hz): the frequency, or below 0 when it is not a note.
double note_frequency(std::string_view n) {
    if (n.size() < 2) return -1;
    static constexpr int kSemis[7] = {9, 11, 0, 2, 4, 5, 7};   // A B C D E F G from C
    const char letter = static_cast<char>(std::toupper(static_cast<unsigned char>(n[0])));
    if (letter < 'A' || letter > 'G') return -1;
    int semi = kSemis[letter - 'A'];
    std::size_t i = 1;
    while (i < n.size() && (n[i] == '#' || n[i] == 'b')) semi += n[i++] == '#' ? 1 : -1;
    if (i >= n.size()) return -1;
    int octave = 0;
    bool neg = false;
    if (n[i] == '-') { neg = true; ++i; }
    if (i >= n.size()) return -1;
    for (; i < n.size(); ++i) {
        if (n[i] < '0' || n[i] > '9') return -1;
        octave = octave * 10 + (n[i] - '0');
    }
    if (neg) octave = -octave;
    const int midi = (octave + 1) * 12 + semi;
    return 440.0 * std::pow(2.0, (midi - 69) / 12.0);
}

}  // namespace

Result<std::vector<float>> synthesize_song(const Json& song, int rate) {
    if (!song.is_object()) return fail("bad_audio", "a song is a JSON object: {{bpm, instruments, tracks}}");
    const double bpm = song.value("bpm", 120.0);
    const double per_beat = song.value("steps_per_beat", 2.0);
    if (!(bpm >= 20 && bpm <= 400)) return fail("bad_audio", "'bpm' is {}, not between 20 and 400", bpm);
    if (!(per_beat >= 1 && per_beat <= 16)) return fail("bad_audio", "'steps_per_beat' is {}, not between 1 and 16", per_beat);
    const double step = 60.0 / bpm / per_beat;
    if (!song.contains("instruments") || !song["instruments"].is_object()) return fail("bad_audio", "a song needs instruments: {{name: recipe, ...}}");
    if (!song.contains("tracks") || !song["tracks"].is_array() || song["tracks"].empty()) return fail("bad_audio", "a song needs tracks: [{{instrument, notes}}, ...]");
    // Every track's notes as tokens; the song is as long as its longest track (or `steps`).
    struct Track { std::string instrument; std::vector<std::string> notes; double volume = 1; };
    std::vector<Track> tracks;
    std::size_t steps = 0;
    for (const Json& t : song["tracks"]) {
        Track tr;
        tr.instrument = t.value("instrument", std::string());
        if (!song["instruments"].contains(tr.instrument)) return fail("bad_audio", "a track plays '{}', which is not one of the instruments", tr.instrument);
        tr.volume = t.value("volume", 1.0);
        std::string text = t.value("notes", std::string());
        for (std::size_t s = 0; s < text.size();) {
            while (s < text.size() && (std::isspace(static_cast<unsigned char>(text[s])) || text[s] == '|')) ++s;
            std::size_t e = s;
            while (e < text.size() && !std::isspace(static_cast<unsigned char>(text[e])) && text[e] != '|') ++e;
            if (e > s) tr.notes.push_back(text.substr(s, e - s));
            s = e;
        }
        steps = std::max(steps, tr.notes.size());
        tracks.push_back(std::move(tr));
    }
    if (song.contains("steps") && song["steps"].is_number()) steps = static_cast<std::size_t>(std::max(0.0, song["steps"].get<double>()));
    if (steps == 0) return fail("bad_audio", "the song has no notes");
    const bool loop = song.value("loop", true);
    const auto length = static_cast<std::size_t>(std::llround(static_cast<double>(steps) * step * rate));
    std::vector<float> out(length, 0.0f);
    std::uint64_t k = 0;
    for (const Track& tr : tracks) {
        const Json& recipe = song["instruments"][tr.instrument];
        // An instrument is a recipe: its first voice (or its preset's) is what each note plays.
        Json voice = recipe;
        if (recipe.contains("preset")) {
            POCKET_TRY(base, preset(recipe["preset"].get<std::string>(), static_cast<std::uint64_t>(recipe.value("seed", 0))));
            voice = base[0];
            for (const auto& [kk, val] : recipe.items()) if (kk != "preset" && kk != "seed") voice[kk] = val;
        }
        auto parsed = read_voice(voice);
        if (!parsed) return fail("bad_audio", "instrument '{}': {}", tr.instrument, parsed.error().message);
        // A track shorter than the song repeats.
        for (std::size_t i = 0; i < steps && !tr.notes.empty(); ++i) {
            const std::string& n = tr.notes[i % tr.notes.size()];
            if (n == "." || n == "-") continue;
            Voice v = *parsed;
            if (n != "x" && n != "X") {
                const double f = note_frequency(n);
                if (f <= 0) return fail("bad_audio", "track of '{}': '{}' is not a note (C4, F#5, Bb2), a rest (.), a hold (-) or a hit (x)", tr.instrument, n);
                if (v.to > 0) v.to *= f / v.frequency;   // the instrument's slide, kept in proportion
                v.frequency = f;
            }
            // Held through the steps of "-" after it.
            std::size_t held = 1;
            while (i + held < steps && tr.notes[(i + held) % tr.notes.size()] == "-") ++held;
            v.hold = std::max(v.hold, static_cast<double>(held) * step - v.attack - v.decay * 0.5);
            v.volume *= tr.volume;
            std::vector<float> note;
            render(v, rate, 0x5eed + k++, note);
            const auto at = static_cast<std::size_t>(std::llround(static_cast<double>(i) * step * rate));
            for (std::size_t j = 0; j < note.size(); ++j) {
                std::size_t p = at + j;
                if (p >= length) {
                    if (!loop) break;
                    p %= length;   // the tail of the last notes rings into the first
                }
                out[p] += note[j];
            }
        }
    }
    float peak = 0;
    for (float s : out) peak = std::max(peak, std::fabs(s));
    if (peak > 1.0f) for (float& s : out) s /= peak;
    return out;
}

Result<std::vector<float>> synthesize(const Json& recipe, int rate) {
    if (!recipe.is_object()) return fail("bad_audio", "a recipe is a JSON object: {{wave, frequency, to, attack, hold, decay, ...}}, {{preset, seed}} or {{layers: [...]}}");
    const auto seed = static_cast<std::uint64_t>(recipe.value("seed", 0));
    std::vector<Json> voices;
    if (recipe.contains("preset")) {
        if (!recipe["preset"].is_string()) return fail("bad_audio", "'preset' is a name");
        POCKET_TRY(base, preset(recipe["preset"].get<std::string>(), seed));
        voices = std::move(base);
        // The recipe's own fields over the preset's first voice: {preset: "coin", volume: 0.2}.
        for (const auto& [k, val] : recipe.items()) if (k != "preset" && k != "seed" && k != "layers") voices[0][k] = val;
    } else if (recipe.contains("layers")) {
        if (!recipe["layers"].is_array() || recipe["layers"].empty()) return fail("bad_audio", "'layers' is a list of voices");
        for (const Json& l : recipe["layers"]) {
            Json voice = Json::object();
            for (const auto& [k, val] : recipe.items()) if (k != "layers" && k != "seed") voice[k] = val;   // the recipe's fields as defaults
            for (const auto& [k, val] : l.items()) voice[k] = val;
            voices.push_back(std::move(voice));
        }
    } else {
        voices.push_back(recipe);
    }
    std::vector<float> out;
    std::uint64_t k = 0;
    for (const Json& j : voices) {
        auto v = read_voice(j);
        if (!v) return fail("bad_audio", "{}{}", voices.size() > 1 ? std::format("voice {}: ", k + 1) : "", v.error().message);
        render(*v, rate, seed + k * 7919, out);
        ++k;
    }
    if (out.empty()) return fail("bad_audio", "the recipe makes no sound (attack, hold and decay are all 0)");
    // Never past full scale.
    float peak = 0;
    for (float s : out) peak = std::max(peak, std::fabs(s));
    if (peak > 1.0f) for (float& s : out) s /= peak;
    return out;
}

}  // namespace pocket::audio
