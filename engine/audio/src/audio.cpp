#include <pocket/audio/audio.hpp>

#include <pocket/core/fs.hpp>
#include <pocket/core/log.hpp>

#include <SDL3/SDL.h>

#include <algorithm>
#include <cmath>
#include <map>

namespace pocket::audio {

namespace {

struct Clip {
    std::string path;
    std::vector<float> samples;  // interleaved stereo at the mixer rate
    std::uint32_t frames = 0;
    double duration = 0;
    int source_rate = 0, source_channels = 0;
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
};

}  // namespace

struct Audio::Impl {
    Config config;
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
        SDL_IOStream* io = SDL_IOFromConstMem(bytes.data(), bytes.size());
        if (!io) return fail("bad_audio", "{}: {}", path, SDL_GetError());
        SDL_AudioSpec spec{};
        Uint8* buf = nullptr;
        Uint32 len = 0;
        if (!SDL_LoadWAV_IO(io, true, &spec, &buf, &len)) return fail("bad_audio", "{}: {}", path, SDL_GetError());
        // Convert to the mixer format: float, stereo, mixer rate.
        SDL_AudioSpec dst{};
        dst.format = SDL_AUDIO_F32;
        dst.channels = 2;
        dst.freq = config.sample_rate;
        SDL_AudioStream* conv = SDL_CreateAudioStream(&spec, &dst);
        if (!conv) { SDL_free(buf); return fail("bad_audio", "{}: {}", path, SDL_GetError()); }
        SDL_PutAudioStreamData(conv, buf, static_cast<int>(len));
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
        SDL_free(buf);
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
        i.loop = v.loop;
        i.entity = v.entity;
        i.tag = v.tag;
        i.loops_done = v.loops_done;
        return i;
    }

    // Render `frames` stereo frames of every voice into mix (additive, then master/mute).
    void render(int frames) {
        mix.assign(static_cast<std::size_t>(frames) * 2, 0.0f);
        if (muted) return;
        for (Voice& v : voices) {
            if (v.render_done || !v.clip) continue;
            const Clip& c = *v.clip;
            float l = v.volume * master * (v.pan <= 0 ? 1.0f : 1.0f - v.pan);
            float r = v.volume * master * (v.pan >= 0 ? 1.0f : 1.0f + v.pan);
            double pos = v.render;
            double step = std::max(0.01f, v.pitch);
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
                mix[static_cast<std::size_t>(f) * 2] += sl * l;
                mix[static_cast<std::size_t>(f) * 2 + 1] += sr * r;
                pos += step;
            }
            v.render = pos;
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
    v.loop = options.loop;
    v.entity = options.entity;
    v.tag = options.tag;
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

void Audio::set_master_volume(float v) { impl_->master = std::clamp(v, 0.0f, 2.0f); }
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
    j["plays"] = im.plays;
    j["master_volume"] = im.master;
    j["muted"] = im.muted;
    j["frames_rendered"] = im.frames_rendered;
    return j;
}

}  // namespace pocket::audio
