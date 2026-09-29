#include <pocket/audio/audio.hpp>

#include <pocket/core/fs.hpp>
#include <pocket/core/log.hpp>

#include <SDL3/SDL.h>

#define STB_VORBIS_HEADER_ONLY
#include <stb_vorbis.c>
#undef STB_VORBIS_HEADER_ONLY

#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdlib>
#include <map>

namespace pocket::audio {

namespace {

struct Clip {
    std::string path;
    std::vector<float> samples;  // interleaved stereo at the mixer rate (decoded clips)
    std::uint32_t frames = 0;    // at the mixer rate (for a streamed clip, from the file's length)
    double duration = 0;
    int source_rate = 0, source_channels = 0;
    bool streamed = false;               // decoded by each voice as it plays, from `bytes`
    std::vector<std::uint8_t> bytes;     // the Ogg file (streamed clips)
};

// A streamed voice's way through its file: a decoder, a converter to the mixer format, and the
// converted frames from `start` on (the frames behind the mixer are dropped as it goes). The
// frame count keeps rising across loops, so a looping voice is one continuous timeline.
struct StreamState {
    stb_vorbis* dec = nullptr;
    SDL_AudioStream* conv = nullptr;
    std::vector<float> buf;
    std::uint64_t start = 0;
    bool eof = false;               // nothing more will come (the file's end, without a loop)
    std::vector<short> chunk;
    ~StreamState() {
        if (conv) SDL_DestroyAudioStream(conv);
        if (dec) stb_vorbis_close(dec);
    }
};

struct Voice {
    std::uint32_t id = 0;
    const Clip* clip = nullptr;
    std::string clip_path;
    double logical = 0;    // frames, advanced by tick()
    double render = 0;     // frames, advanced by the mixer
    float volume = 1, pitch = 1, pan = 0;
    bool loop = false;
    std::uint64_t entity = 0;
    std::string tag;
    std::uint32_t loops_done = 0;
    bool render_done = false;
    std::shared_ptr<StreamState> stream;   // set for a streamed clip
    // A one-pole low-pass per channel: `lowpass` squared is the coefficient, so 1 passes the
    // clip through and small values muffle it.
    float lowpass = 1;
    float reverb = 1;   // the send to the room
    float lp_l = 0, lp_r = 0;
    void filter(float& sl, float& sr) {
        if (lowpass >= 1) return;
        const float a = std::max(lowpass * lowpass, 1e-4f);
        lp_l += a * (sl - lp_l);
        lp_r += a * (sr - lp_r);
        sl = lp_l;
        sr = lp_r;
    }
};

// The room: four comb filters in parallel (the tail, damped in their feedback) into two all-pass
// filters in series (the diffusion), the right channel's lines a little longer than the left's so
// the tail has width. The lengths are Freeverb's at 44.1 kHz, scaled to the mixer's rate.
struct Reverb {
    struct Comb {
        std::vector<float> buf;
        std::size_t at = 0;
        float store = 0;
        float run(float in, float feedback, float damp) {
            const float out = buf[at];
            store = out * (1 - damp) + store * damp;
            buf[at] = in + store * feedback;
            if (++at >= buf.size()) at = 0;
            return out;
        }
    };
    struct AllPass {
        std::vector<float> buf;
        std::size_t at = 0;
        float run(float in) {
            const float out = buf[at];
            buf[at] = in + out * 0.5f;
            if (++at >= buf.size()) at = 0;
            return out - in;
        }
    };
    Comb comb[2][4];
    AllPass pass[2][2];
    bool ringing = false;   // something is in the lines: keep rendering the tail after the send stops

    void setup(int rate) {
        const int comb_len[4] = {1116, 1188, 1277, 1356}, pass_len[2] = {556, 441};
        const double k = rate / 44100.0;
        for (int ch = 0; ch < 2; ++ch) {
            for (int i = 0; i < 4; ++i) comb[ch][i].buf.assign(static_cast<std::size_t>(comb_len[i] * k) + (ch ? 23 : 0), 0.0f);
            for (int i = 0; i < 2; ++i) pass[ch][i].buf.assign(static_cast<std::size_t>(pass_len[i] * k) + (ch ? 23 : 0), 0.0f);
        }
        ringing = false;
    }
    void clear() {
        for (auto& row : comb) for (auto& c : row) { std::fill(c.buf.begin(), c.buf.end(), 0.0f); c.store = 0; }
        for (auto& row : pass) for (auto& p : row) std::fill(p.buf.begin(), p.buf.end(), 0.0f);
        ringing = false;
    }
    // One frame: the mono send in, the wet stereo pair out.
    void run(float send, float feedback, float damp, float& wl, float& wr) {
        float out[2] = {0, 0};
        for (int ch = 0; ch < 2; ++ch) {
            for (auto& c : comb[ch]) out[ch] += c.run(send, feedback, damp);
            for (auto& p : pass[ch]) out[ch] = p.run(out[ch]);
        }
        wl = out[0];
        wr = out[1];
    }
};

}  // namespace

struct Audio::Impl {
    Config config;
    ReverbSettings reverb_settings;
    Reverb reverb;
    std::vector<float> send;   // the mono send of the frames being rendered
    bool sdl_audio = false;
    SDL_AudioStream* stream = nullptr;  // bound to the default playback device
    std::string device_name;
    std::map<std::string, std::unique_ptr<Clip>> clips;
    std::vector<Voice> voices;
    std::uint32_t next_id = 1;
    float master = 1.0f;
    bool muted = false;
    std::vector<float> mix;
    std::uint64_t frames_rendered = 0;
    std::uint64_t plays = 0;

    ~Impl() {
        voices.clear();   // streamed voices own SDL converters: gone before the subsystem is
        clips.clear();
        if (stream) SDL_DestroyAudioStream(stream);
        if (sdl_audio) SDL_QuitSubSystem(SDL_INIT_AUDIO);
    }

    Result<std::filesystem::path> resolve(const std::string& path) const {
        if (path.empty()) return fail("bad_args", "clip path is empty");
        std::filesystem::path base = std::filesystem::weakly_canonical(config.project_dir);
        std::filesystem::path full = std::filesystem::weakly_canonical(base / path);
        auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
        if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", path);
        if (!std::filesystem::exists(full)) return fail("no_such_asset", "{} does not exist", path);
        return full;
    }

    Result<const Clip*> clip(const std::string& path) {
        if (auto it = clips.find(path); it != clips.end()) return it->second.get();
        POCKET_TRY(full, resolve(path));
        POCKET_TRY(bytes, fs::read_bytes(full));
        std::string ext = full.extension().string();
        std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char ch) { return static_cast<char>(std::tolower(ch)); });
        SDL_AudioSpec spec{};
        std::vector<std::uint8_t> pcm;   // the file's samples, in `spec`
        if (ext == ".ogg") {
            // A long file streams: its length and format come from a probe, the bytes stay.
            int err = 0;
            stb_vorbis* probe = stb_vorbis_open_memory(reinterpret_cast<const unsigned char*>(bytes.data()), static_cast<int>(bytes.size()), &err, nullptr);
            if (!probe) return fail("bad_audio", "{}: not an Ogg Vorbis file (stb_vorbis error {})", path, err);
            const stb_vorbis_info info = stb_vorbis_get_info(probe);
            const unsigned int length = stb_vorbis_stream_length_in_samples(probe);
            stb_vorbis_close(probe);
            if (info.channels <= 0 || info.sample_rate == 0) return fail("bad_audio", "{}: not an Ogg Vorbis file", path);
            const double seconds = static_cast<double>(length) / static_cast<double>(info.sample_rate);
            if (length > 0 && seconds >= config.stream_seconds) {
                auto c = std::make_unique<Clip>();
                c->path = path;
                c->streamed = true;
                c->bytes = std::move(bytes);
                c->source_rate = static_cast<int>(info.sample_rate);
                c->source_channels = info.channels;
                c->frames = static_cast<std::uint32_t>(std::llround(seconds * config.sample_rate));
                c->duration = static_cast<double>(c->frames) / config.sample_rate;
                if (c->frames == 0) return fail("bad_audio", "{}: no samples", path);
                log::info("audio", "streaming {} ({:.3f} s, {} Hz, {} ch, {} bytes)", path, c->duration, c->source_rate, c->source_channels, c->bytes.size());
                const Clip* raw = c.get();
                clips[path] = std::move(c);
                return raw;
            }
            // Ogg Vorbis through stb_vorbis: 16-bit frames, interleaved, at the file's rate.
            int channels = 0, rate = 0;
            short* out = nullptr;
            const int frames = stb_vorbis_decode_memory(reinterpret_cast<const unsigned char*>(bytes.data()), static_cast<int>(bytes.size()), &channels, &rate, &out);
            if (frames <= 0 || !out || channels <= 0 || rate <= 0) { std::free(out); return fail("bad_audio", "{}: not an Ogg Vorbis file", path); }
            spec.format = SDL_AUDIO_S16;
            spec.channels = channels;
            spec.freq = rate;
            const auto* raw = reinterpret_cast<const std::uint8_t*>(out);
            pcm.assign(raw, raw + static_cast<std::size_t>(frames) * static_cast<std::size_t>(channels) * sizeof(short));
            std::free(out);
        } else {
            SDL_IOStream* io = SDL_IOFromConstMem(bytes.data(), bytes.size());
            if (!io) return fail("bad_audio", "{}: {}", path, SDL_GetError());
            Uint8* buf = nullptr;
            Uint32 len = 0;
            if (!SDL_LoadWAV_IO(io, true, &spec, &buf, &len)) return fail("bad_audio", "{}: {}", path, SDL_GetError());
            pcm.assign(buf, buf + len);
            SDL_free(buf);
        }
        // Convert to the mixer format: float, stereo, mixer rate.
        SDL_AudioSpec dst{};
        dst.format = SDL_AUDIO_F32;
        dst.channels = 2;
        dst.freq = config.sample_rate;
        SDL_AudioStream* conv = SDL_CreateAudioStream(&spec, &dst);
        if (!conv) return fail("bad_audio", "{}: {}", path, SDL_GetError());
        SDL_PutAudioStreamData(conv, pcm.data(), static_cast<int>(pcm.size()));
        SDL_FlushAudioStream(conv);
        auto c = std::make_unique<Clip>();
        c->path = path;
        c->source_rate = spec.freq;
        c->source_channels = spec.channels;
        int available = SDL_GetAudioStreamAvailable(conv);
        if (available > 0) {
            c->samples.resize(static_cast<std::size_t>(available) / sizeof(float));
            int got = SDL_GetAudioStreamData(conv, c->samples.data(), available);
            c->samples.resize(got > 0 ? static_cast<std::size_t>(got) / sizeof(float) : 0);
        }
        SDL_DestroyAudioStream(conv);
        c->frames = static_cast<std::uint32_t>(c->samples.size() / 2);
        c->duration = static_cast<double>(c->frames) / config.sample_rate;
        if (c->frames == 0) return fail("bad_audio", "{}: no samples", path);
        log::info("audio", "loaded {} ({:.3f} s, {} Hz, {} ch)", path, c->duration, spec.freq, spec.channels);
        const Clip* raw = c.get();
        clips[path] = std::move(c);
        return raw;
    }

    static VoiceInfo info_of(const Voice& v, int rate) {
        VoiceInfo i;
        i.id = v.id;
        i.clip = v.clip_path;
        i.position = v.logical / rate;
        i.duration = v.clip ? v.clip->duration : 0;
        i.volume = v.volume;
        i.pitch = v.pitch;
        i.pan = v.pan;
        i.lowpass = v.lowpass;
        i.reverb = v.reverb;
        i.loop = v.loop;
        i.entity = v.entity;
        i.tag = v.tag;
        i.loops_done = v.loops_done;
        return i;
    }

    // A streamed voice's decoder and converter, at the file's start.
    Result<std::shared_ptr<StreamState>> open_stream(const Clip& c) const {
        auto s = std::make_shared<StreamState>();
        int err = 0;
        s->dec = stb_vorbis_open_memory(c.bytes.data(), static_cast<int>(c.bytes.size()), &err, nullptr);
        if (!s->dec) return fail("bad_audio", "{}: cannot open the stream (stb_vorbis error {})", c.path, err);
        SDL_AudioSpec src{};
        src.format = SDL_AUDIO_S16;
        src.channels = c.source_channels;
        src.freq = c.source_rate;
        SDL_AudioSpec dst{};
        dst.format = SDL_AUDIO_F32;
        dst.channels = 2;
        dst.freq = config.sample_rate;
        s->conv = SDL_CreateAudioStream(&src, &dst);
        if (!s->conv) return fail("bad_audio", "{}: {}", c.path, SDL_GetError());
        s->chunk.resize(static_cast<std::size_t>(4096) * static_cast<std::size_t>(c.source_channels));
        return s;
    }

    // Decode and convert until the buffer reaches frame `need` (or the file's end, with a loop
    // wrapping to its start), and take what the converter has ready.
    void feed(StreamState& s, const Clip& c, bool loop, std::uint64_t need) {
        auto pull = [&]() {
            const int available = SDL_GetAudioStreamAvailable(s.conv);
            if (available <= 0) return;
            const std::size_t at = s.buf.size();
            s.buf.resize(at + static_cast<std::size_t>(available) / sizeof(float));
            const int got = SDL_GetAudioStreamData(s.conv, s.buf.data() + at, available);
            s.buf.resize(at + (got > 0 ? static_cast<std::size_t>(got) / sizeof(float) : 0));
            if (s.buf.size() % 2) s.buf.pop_back();
        };
        int guard = 0;
        bool wrapped = false;   // a seek to the start that yields nothing ends the stream (an empty file)
        while (!s.eof && s.start + s.buf.size() / 2 < need && guard++ < 4096) {
            const int n = stb_vorbis_get_samples_short_interleaved(s.dec, c.source_channels, s.chunk.data(), static_cast<int>(s.chunk.size()));
            if (n > 0) {
                SDL_PutAudioStreamData(s.conv, s.chunk.data(), n * c.source_channels * static_cast<int>(sizeof(short)));
                wrapped = false;
            } else if (loop && !wrapped && stb_vorbis_seek_start(s.dec)) {
                wrapped = true;   // around again
            } else {
                SDL_FlushAudioStream(s.conv);
                s.eof = true;
            }
            pull();
        }
        pull();
    }

    // Render `frames` stereo frames of every voice into mix (additive, then master/mute).
    void render(int frames) {
        mix.assign(static_cast<std::size_t>(frames) * 2, 0.0f);
        if (muted) return;
        const bool room = reverb_settings.room > 0;
        if (room && reverb.comb[0][0].buf.empty()) reverb.setup(config.sample_rate);
        send.assign(room ? static_cast<std::size_t>(frames) : 0, 0.0f);
        for (Voice& v : voices) {
            if (v.render_done || !v.clip) continue;
            const Clip& c = *v.clip;
            float l = v.volume * master * (v.pan <= 0 ? 1.0f : 1.0f - v.pan);
            float r = v.volume * master * (v.pan >= 0 ? 1.0f : 1.0f + v.pan);
            double pos = v.render;
            double step = std::max(0.01f, v.pitch);
            if (v.stream) {
                // From the decoded window: fed ahead of the mixer, the frames behind it dropped.
                StreamState& s = *v.stream;
                feed(s, c, v.loop, static_cast<std::uint64_t>(pos + step * frames) + 2);
                const std::uint64_t have_end = s.start + s.buf.size() / 2;
                for (int f = 0; f < frames; ++f) {
                    const auto i0 = static_cast<std::uint64_t>(pos);
                    if (i0 >= have_end) { if (s.eof) v.render_done = true; break; }   // the end, or starved (silence)
                    const std::uint64_t i1 = i0 + 1 < have_end ? i0 + 1 : i0;
                    const std::size_t r0 = static_cast<std::size_t>(i0 - s.start) * 2, r1 = static_cast<std::size_t>(i1 - s.start) * 2;
                    const float t = static_cast<float>(pos - static_cast<double>(i0));
                    float sl = s.buf[r0] * (1 - t) + s.buf[r1] * t, sr = s.buf[r0 + 1] * (1 - t) + s.buf[r1 + 1] * t;
                    v.filter(sl, sr);
                    mix[static_cast<std::size_t>(f) * 2] += sl * l;
                    mix[static_cast<std::size_t>(f) * 2 + 1] += sr * r;
                    if (room && v.reverb > 0) send[static_cast<std::size_t>(f)] += (sl * l + sr * r) * 0.5f * v.reverb;
                    pos += step;
                }
                v.render = pos;
                const auto behind = static_cast<std::uint64_t>(pos);
                if (behind > s.start + 8) {
                    const std::uint64_t drop = std::min<std::uint64_t>(behind - s.start - 4, s.buf.size() / 2);
                    s.buf.erase(s.buf.begin(), s.buf.begin() + static_cast<std::ptrdiff_t>(drop * 2));
                    s.start += drop;
                }
                continue;
            }
            for (int f = 0; f < frames; ++f) {
                if (pos >= c.frames) {
                    if (!v.loop) { v.render_done = true; break; }
                    pos = std::fmod(pos, static_cast<double>(c.frames));
                }
                auto i0 = static_cast<std::size_t>(pos);
                std::size_t i1 = i0 + 1 < c.frames ? i0 + 1 : (v.loop ? 0 : i0);
                float t = static_cast<float>(pos - static_cast<double>(i0));
                float sl = c.samples[i0 * 2] * (1 - t) + c.samples[i1 * 2] * t;
                float sr = c.samples[i0 * 2 + 1] * (1 - t) + c.samples[i1 * 2 + 1] * t;
                v.filter(sl, sr);
                mix[static_cast<std::size_t>(f) * 2] += sl * l;
                mix[static_cast<std::size_t>(f) * 2 + 1] += sr * r;
                if (room && v.reverb > 0) send[static_cast<std::size_t>(f)] += (sl * l + sr * r) * 0.5f * v.reverb;
                pos += step;
            }
            v.render = pos;
        }
        if (room) {
            // The tail: fed by the send, added at the mix level; kept running while it rings.
            bool fed = false;
            for (float s : send) fed = fed || std::fabs(s) > 1e-6f;
            if (fed || reverb.ringing) {
                const float feedback = 0.7f + reverb_settings.room * 0.28f, damp = reverb_settings.damping * 0.4f;
                float energy = 0;
                for (int f = 0; f < frames; ++f) {
                    float wl = 0, wr = 0;
                    // The combs ring up to many times what goes in: the send is scaled down first (Freeverb's gain for its 8 combs is 0.015; these are 4).
                    reverb.run(send[static_cast<std::size_t>(f)] * 0.03f, feedback, damp, wl, wr);
                    mix[static_cast<std::size_t>(f) * 2] += wl * reverb_settings.mix;
                    mix[static_cast<std::size_t>(f) * 2 + 1] += wr * reverb_settings.mix;
                    energy = std::max(energy, std::fabs(wl) + std::fabs(wr));
                }
                reverb.ringing = energy > 1e-5f;
            }
        } else if (reverb.ringing) {
            reverb.clear();
        }
        for (float& s : mix) s = std::clamp(s, -1.0f, 1.0f);
    }
};

Audio::Audio() : impl_(std::make_unique<Impl>()) {}
Audio::~Audio() = default;

Result<std::unique_ptr<Audio>> Audio::create(const Config& config) {
    std::unique_ptr<Audio> a(new Audio());
    a->impl_->config = config;
    // The audio subsystem is needed for WAV decoding and conversion even without a device.
    if (SDL_InitSubSystem(SDL_INIT_AUDIO)) {
        a->impl_->sdl_audio = true;
    } else {
        log::warn("audio", "SDL audio unavailable: {}", SDL_GetError());
    }
    if (!config.headless && a->impl_->sdl_audio) {
        SDL_AudioSpec spec{};
        spec.format = SDL_AUDIO_F32;
        spec.channels = 2;
        spec.freq = config.sample_rate;
        a->impl_->stream = SDL_OpenAudioDeviceStream(SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK, &spec, nullptr, nullptr);
        if (a->impl_->stream) {
            SDL_ResumeAudioStreamDevice(a->impl_->stream);
            SDL_AudioDeviceID dev = SDL_GetAudioStreamDevice(a->impl_->stream);
            const char* name = SDL_GetAudioDeviceName(dev);
            a->impl_->device_name = name ? name : "default";
            log::info("audio", "playback on {} ({} Hz stereo float)", a->impl_->device_name, config.sample_rate);
        } else {
            log::warn("audio", "no playback device: {}", SDL_GetError());
        }
    }
    return a;
}

Status Audio::load(const std::string& clip) {
    POCKET_TRY_VOID(impl_->clip(clip));
    return {};
}

Result<std::uint32_t> Audio::play(const std::string& clip, const PlayOptions& options) {
    POCKET_TRY(c, impl_->clip(clip));
    Voice v;
    v.id = impl_->next_id++;
    v.clip = c;
    v.clip_path = clip;
    v.volume = std::clamp(options.volume, 0.0f, 4.0f);
    v.pitch = std::clamp(options.pitch, 0.05f, 8.0f);
    v.pan = std::clamp(options.pan, -1.0f, 1.0f);
    v.lowpass = std::clamp(options.lowpass, 0.0f, 1.0f);
    v.reverb = std::clamp(options.reverb, 0.0f, 1.0f);
    v.loop = options.loop;
    v.entity = options.entity;
    v.tag = options.tag;
    if (c->streamed) {
        POCKET_TRY(s, impl_->open_stream(*c));
        v.stream = s;
    }
    impl_->voices.push_back(v);
    impl_->plays++;
    return v.id;
}

std::uint32_t Audio::stop(std::uint32_t voice) {
    auto& vs = impl_->voices;
    auto before = vs.size();
    vs.erase(std::remove_if(vs.begin(), vs.end(), [&](const Voice& v) { return v.id == voice; }), vs.end());
    return static_cast<std::uint32_t>(before - vs.size());
}

std::uint32_t Audio::stop_clip(const std::string& clip) {
    auto& vs = impl_->voices;
    auto before = vs.size();
    vs.erase(std::remove_if(vs.begin(), vs.end(), [&](const Voice& v) { return v.clip_path == clip; }), vs.end());
    return static_cast<std::uint32_t>(before - vs.size());
}

std::uint32_t Audio::stop_tag(const std::string& tag) {
    auto& vs = impl_->voices;
    auto before = vs.size();
    vs.erase(std::remove_if(vs.begin(), vs.end(), [&](const Voice& v) { return v.tag == tag; }), vs.end());
    return static_cast<std::uint32_t>(before - vs.size());
}

std::uint32_t Audio::stop_all() {
    auto n = static_cast<std::uint32_t>(impl_->voices.size());
    impl_->voices.clear();
    return n;
}

Status Audio::set(std::uint32_t voice, const Json& params) {
    for (Voice& v : impl_->voices) {
        if (v.id != voice) continue;
        if (params.contains("volume") && params["volume"].is_number()) v.volume = std::clamp(params["volume"].get<float>(), 0.0f, 4.0f);
        if (params.contains("pitch") && params["pitch"].is_number()) v.pitch = std::clamp(params["pitch"].get<float>(), 0.05f, 8.0f);
        if (params.contains("pan") && params["pan"].is_number()) v.pan = std::clamp(params["pan"].get<float>(), -1.0f, 1.0f);
        if (params.contains("lowpass") && params["lowpass"].is_number()) v.lowpass = std::clamp(params["lowpass"].get<float>(), 0.0f, 1.0f);
        if (params.contains("reverb") && params["reverb"].is_number()) v.reverb = std::clamp(params["reverb"].get<float>(), 0.0f, 1.0f);
        if (params.contains("loop") && params["loop"].is_boolean()) v.loop = params["loop"].get<bool>();
        return {};
    }
    return fail("no_such_voice", "voice {} is not playing", voice);
}

std::vector<VoiceEvent> Audio::tick(double dt) {
    std::vector<VoiceEvent> events;
    Impl& im = *impl_;
    for (Voice& v : im.voices) {
        if (!v.clip) continue;
        v.logical += dt * im.config.sample_rate * v.pitch;
        while (v.logical >= v.clip->frames) {
            if (v.loop) {
                v.logical -= v.clip->frames;
                v.loops_done++;
                events.push_back({"audio.looped", Impl::info_of(v, im.config.sample_rate)});
            } else {
                v.logical = v.clip->frames;
                break;
            }
        }
    }
    auto& vs = im.voices;
    for (auto it = vs.begin(); it != vs.end();) {
        if (it->clip && !it->loop && it->logical >= it->clip->frames) {
            events.push_back({"audio.finished", Impl::info_of(*it, im.config.sample_rate)});
            it = vs.erase(it);
        } else {
            ++it;
        }
    }
    return events;
}

void Audio::pump() {
    Impl& im = *impl_;
    if (!im.stream) return;
    // Keep about 80 ms queued; render in 10 ms slices.
    const int target_bytes = im.config.sample_rate * 2 * static_cast<int>(sizeof(float)) * 80 / 1000;
    const int slice_frames = im.config.sample_rate / 100;
    int queued = SDL_GetAudioStreamQueued(im.stream);
    int guard = 0;
    while (queued < target_bytes && guard++ < 32) {
        im.render(slice_frames);
        SDL_PutAudioStreamData(im.stream, im.mix.data(), static_cast<int>(im.mix.size() * sizeof(float)));
        im.frames_rendered += static_cast<std::uint64_t>(slice_frames);
        queued += static_cast<int>(im.mix.size() * sizeof(float));
    }
}

const std::vector<float>& Audio::render_frames(int frames) {
    impl_->render(std::max(0, frames));
    impl_->frames_rendered += static_cast<std::uint64_t>(std::max(0, frames));
    return impl_->mix;
}

void Audio::set_master_volume(float v) { impl_->master = std::clamp(v, 0.0f, 2.0f); }
void Audio::set_reverb(ReverbSettings s) {
    s.room = std::clamp(s.room, 0.0f, 1.0f);
    s.damping = std::clamp(s.damping, 0.0f, 1.0f);
    s.mix = std::clamp(s.mix, 0.0f, 1.0f);
    impl_->reverb_settings = s;
}
ReverbSettings Audio::reverb() const { return impl_->reverb_settings; }
float Audio::master_volume() const { return impl_->master; }
void Audio::set_muted(bool m) { impl_->muted = m; }
bool Audio::muted() const { return impl_->muted; }
bool Audio::has_device() const { return impl_->stream != nullptr; }

std::vector<VoiceInfo> Audio::voices() const {
    std::vector<VoiceInfo> out;
    for (const Voice& v : impl_->voices) out.push_back(Impl::info_of(v, impl_->config.sample_rate));
    return out;
}

Json Audio::clips() const {
    Json arr = Json::array();
    for (const auto& [path, c] : impl_->clips) {
        Json j;
        j["path"] = path;
        j["seconds"] = c->duration;
        j["frames"] = c->frames;
        j["source_rate"] = c->source_rate;
        j["source_channels"] = c->source_channels;
        j["streamed"] = c->streamed;
        j["bytes"] = c->streamed ? c->bytes.size() : c->samples.size() * sizeof(float);   // what it costs in memory
        arr.push_back(j);
    }
    return arr;
}

Json Audio::describe() const {
    const Impl& im = *impl_;
    Json j;
    j["device"] = im.stream ? im.device_name : "none";
    j["sample_rate"] = im.config.sample_rate;
    j["voices"] = im.voices.size();
    j["clips"] = im.clips.size();
    std::size_t streamed = 0, streaming = 0;
    for (const auto& [path, c] : im.clips) streamed += c->streamed ? 1 : 0;
    for (const Voice& v : im.voices) streaming += v.stream ? 1 : 0;
    j["streamed_clips"] = streamed;
    j["streaming_voices"] = streaming;
    j["stream_seconds"] = im.config.stream_seconds;
    j["plays"] = im.plays;
    j["master_volume"] = im.master;
    j["muted"] = im.muted;
    j["reverb"] = Json{{"room", im.reverb_settings.room}, {"damping", im.reverb_settings.damping}, {"mix", im.reverb_settings.mix}, {"ringing", im.reverb.ringing}};
    j["frames_rendered"] = im.frames_rendered;
    return j;
}

}  // namespace pocket::audio
