// A sound described in numbers and words for someone who cannot hear it (docs/design/audio.md,
// Listening without ears): how long and loud, its envelope, its notes over time, how bright and
// how tonal, its onsets.
#pragma once

#include <pocket/core/json.hpp>

#include <vector>

namespace pocket::audio {

// `stereo`: interleaved left, right at `rate`.
Json analyze_samples(const std::vector<float>& stereo, int rate);

}  // namespace pocket::audio
