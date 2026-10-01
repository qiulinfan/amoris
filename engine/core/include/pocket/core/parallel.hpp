// Work split across the machine's cores: what is independent item by item (one cloth sheet, say)
// done on a few threads, the caller's among them. Each item must touch only its own state and read
// what nothing writes meanwhile; the results are then the same whichever thread did which, as the
// engine's determinism asks. Where there are no threads (the web build) it is a plain loop.
#pragma once

#include <cstddef>
#include <functional>

namespace pocket {

// fn(i) for every i in [0, n), on up to parallel_threads() threads; returns when all have run. A
// call made from inside another runs its items on the calling thread. POCKET_THREADS=1 (read
// once) keeps every call on the calling thread.
void parallel_for(std::size_t n, const std::function<void(std::size_t)>& fn);

// How many threads parallel_for uses, the caller's included (1 where there are no others).
[[nodiscard]] std::size_t parallel_threads();

}  // namespace pocket
