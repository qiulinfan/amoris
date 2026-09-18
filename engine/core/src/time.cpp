#include <pocket/core/time.hpp>

namespace pocket {

double process_ms() {
    static const auto start = std::chrono::steady_clock::now();
    return std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - start).count();
}

}  // namespace pocket
