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
};

struct PlayOptions {
    float volume = 1.0f;
    float pitch = 1.0f;   // playback rate multiplier
    bool loop = false;
    float pan = 0.0f;     // -1 left .. 1 right
    std::uint64_t entity = 0;  // optional owner, reported in events
    std::string tag;      // optional label for stop/list
};

struct VoiceInfo {
    std::uint32_t id = 0;
    std::string clip;
    double position = 0;   // seconds into the clip
    double duration = 0;   // clip length in seconds
    float volume = 1, pitch = 1, pan = 0;
    bool loop = false;
    std::uint64_t entity = 0;
    std::string tag;
    std::uint32_t loops_done = 0;
};

// Something that happened to a voice during tick(): reported so the session can log events.
struct VoiceEvent {
    std::string type;  // "audio.finished", "audio.looped"
    VoiceInfo voice;
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
    Status set(std::uint32_t voice, const Json& params);  // volume, pitch, loop, pan
    // Advance logical time; returns finished/looped events.
    std::vector<VoiceEvent> tick(double dt);
    // Render audio into the device stream (no-op without a device). Call once per frame.
    void pump();
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
