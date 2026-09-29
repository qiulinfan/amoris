// Sound.
//
// Clips are project-relative WAV files. A voice is one playing clip with volume, pitch and loop
// flags; its logical position advances with the simulation tick, so which voices are playing,
// where they are and when they finish is deterministic and observable without a sound card. When
// a playback device exists the mixer renders the same voices into an SDL audio stream.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>

#include <cstdint>
#include <filesystem>
#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace pocket::audio {

struct Config {
    std::filesystem::path project_dir;
    bool headless = false;  // never opens a device
    int sample_rate = 48000;
    // An Ogg clip at least this long is streamed: its compressed bytes stay in memory and each
    // voice decodes its own way through them a chunk at a time, so a music track costs its file
    // size rather than its decoded size. 0 streams every Ogg; WAV clips always decode whole.
    double stream_seconds = 10.0;
};

// The room every voice plays in: a reverb tail fed by each voice's `reverb` send.
struct ReverbSettings {
    float room = 0.0f;     // how long the tail rings, 0..1: 0 is no reverb (a dry mix), 0.5 a room, 0.9 a hall
    float damping = 0.5f;  // how fast the high end dies in the tail, 0..1
    float mix = 0.3f;      // the tail's level against the dry voices, 0..1
};

// A bus: a group of voices mixed together and set as one (music, effects, dialogue), so a
// settings menu's sliders and a pause menu's muffle are one call each.
struct BusSettings {
    float volume = 1.0f;         // 0..2
    bool muted = false;
    float lowpass = 1.0f;        // over the bus's whole mix, 0..1: 1 as it is, small values muffle it
    float highpass = 0.0f;       // 0..1: 0 as it is, larger values thin it out (a radio, a phone)
    float echo = 0.0f;           // seconds between the repeats of an echo, 0 for none (up to 2)
    float echo_feedback = 0.35f; // 0..0.9: how much of each repeat comes back again
    float echo_mix = 0.4f;       // 0..1: how loud the repeats are beside the sound
    float reverb = 1.0f;         // 0..2: how much of the bus the room gets (its voices' sends scaled)
    std::string duck_by;         // another bus: while a voice plays on it, this one falls to duck_amount
    float duck_amount = 0.3f;    // 0..1
    float duck_seconds = 0.25f;  // how long the fall and the recovery take
};

struct BusInfo {
    std::string name;
    BusSettings settings;
    float duck = 1.0f;     // the ducking gain right now, 1 when not ducked
    bool ducked = false;   // whether the bus it ducks under is playing
    int voices = 0;
};

struct PlayOptions {
    float volume = 1.0f;
    float pitch = 1.0f;   // playback rate multiplier
    bool loop = false;
    float pan = 0.0f;     // -1 left .. 1 right
    float lowpass = 1.0f; // how much of the high end is kept, 0..1: 1 is the clip as it is, small values muffle it (underwater, behind a door)
    float reverb = 1.0f;  // how much of the voice goes to the room's reverb, 0..1 (0 keeps interface clicks and music dry)
    std::uint64_t entity = 0;  // optional owner, reported in events
    std::string tag;      // optional label for stop/list
    std::string bus = "main";
};

struct VoiceInfo {
    std::uint32_t id = 0;
    std::string clip;
    double position = 0;   // seconds into the clip
    double duration = 0;   // clip length in seconds
    float volume = 1, pitch = 1, pan = 0;
    float lowpass = 1;
    float reverb = 1;
    bool loop = false;
    std::uint64_t entity = 0;
    std::string tag;
    std::string bus;
    std::uint32_t loops_done = 0;
};

// Something that happened to a voice during tick(): reported so the session can log events.
struct VoiceEvent {
    std::string type;  // "audio.finished", "audio.looped"; "audio.ducked" when a bus starts or stops ducking (voice empty)
    VoiceInfo voice;
    std::string bus;   // audio.ducked: the bus, `ducked` whether it is now
    bool ducked = false;
};

class Audio {
   public:
    static Result<std::unique_ptr<Audio>> create(const Config& config);
    ~Audio();
    Audio(const Audio&) = delete;
    Audio& operator=(const Audio&) = delete;

    // Decode (once) and cache a clip.
    Status load(const std::string& clip);
    Result<std::uint32_t> play(const std::string& clip, const PlayOptions& options = {});
    // Stop one voice, every voice of a clip/tag, or everything. Returns how many stopped.
    std::uint32_t stop(std::uint32_t voice);
    std::uint32_t stop_clip(const std::string& clip);
    std::uint32_t stop_tag(const std::string& tag);
    std::uint32_t stop_all();
    std::uint32_t stop_bus(const std::string& bus);
    Status set(std::uint32_t voice, const Json& params);  // volume, pitch, loop, pan, lowpass, reverb, bus
    // A bus's settings (made on first use with the defaults); the change is heard at the next render.
    void set_bus(const std::string& name, BusSettings settings);
    [[nodiscard]] BusSettings bus(const std::string& name) const;
    [[nodiscard]] std::vector<BusInfo> buses() const;
    // The room: a reverb every voice's send feeds (off at room 0). Takes effect at the next render.
    void set_reverb(ReverbSettings s);
    [[nodiscard]] ReverbSettings reverb() const;
    // Advance logical time; returns finished/looped events.
    std::vector<VoiceEvent> tick(double dt);
    // Render audio into the device stream (no-op without a device). Call once per frame.
    void pump();
    // Render the next `frames` stereo frames of the mix without a device (what pump would queue):
    // for tests and tools that want to hear a headless run. Interleaved left, right in -1..1.
    [[nodiscard]] const std::vector<float>& render_frames(int frames);
    void set_master_volume(float v);
    [[nodiscard]] float master_volume() const;
    void set_muted(bool m);
    [[nodiscard]] bool muted() const;

    [[nodiscard]] std::vector<VoiceInfo> voices() const;
    [[nodiscard]] Json clips() const;
    [[nodiscard]] Json describe() const;
    [[nodiscard]] bool has_device() const;

   private:
    Audio();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::audio
