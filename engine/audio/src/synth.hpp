// Sounds made from a recipe (docs/design/audio.md, Sounds from a recipe): a `.sfx` file is JSON
// naming waves, pitches and envelopes, or a preset with a seed, and is rendered to samples when
// it is first played.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>

#include <vector>

namespace pocket::audio {

// Mono samples at `rate`, from a recipe; fails with what is wrong in it.
Result<std::vector<float>> synthesize(const Json& recipe, int rate);

// The preset names a recipe can start from.
std::vector<std::string> synth_presets();

// A song (docs/design/audio.md, Music from a score): instruments as recipes and tracks of notes
// on a grid of steps, rendered to mono samples at `rate`; a looping song's tails wrap round to
// its start, so it plays on without a seam.
Result<std::vector<float>> synthesize_song(const Json& song, int rate);

}  // namespace pocket::audio
