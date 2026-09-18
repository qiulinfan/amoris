// Simulation time.
//
// The engine advances the world in fixed ticks (default 60 Hz). Rendering interpolates between
// ticks. Real time only decides how many ticks to run per frame; nothing inside the simulation
// reads the wall clock, which is what makes replays and state hashes reproducible.
#pragma once

#include <chrono>
#include <cstdint>

namespace pocket {

struct TickClock {
    double tick_seconds = 1.0 / 60.0;
    std::int64_t tick = 0;       // number of completed ticks
    double accumulator = 0.0;    // unconsumed real time in seconds
    int max_ticks_per_frame = 8; // spiral-of-death guard

    [[nodiscard]] double sim_seconds() const { return static_cast<double>(tick) * tick_seconds; }

    // Feed real elapsed seconds and return how many ticks should run now.
    int advance(double elapsed_seconds) {
        accumulator += elapsed_seconds;
        int n = 0;
        while (accumulator >= tick_seconds && n < max_ticks_per_frame) {
            accumulator -= tick_seconds;
            ++n;
        }
        if (n == max_ticks_per_frame) accumulator = 0.0;  // drop the backlog
        return n;
    }

    // Interpolation factor in [0,1) for rendering between tick and tick+1.
    [[nodiscard]] double alpha() const { return accumulator / tick_seconds; }
};

struct Stopwatch {
    using clock = std::chrono::steady_clock;
    clock::time_point start = clock::now();
    [[nodiscard]] double seconds() const { return std::chrono::duration<double>(clock::now() - start).count(); }
    [[nodiscard]] double ms() const { return seconds() * 1000.0; }
    double lap() {
        auto now = clock::now();
        double s = std::chrono::duration<double>(now - start).count();
        start = now;
        return s;
    }
};

// Milliseconds since the process started (wall clock, for logs only).
double process_ms();

}  // namespace pocket
