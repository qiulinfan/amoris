#include <pocket/core/parallel.hpp>

#ifndef __EMSCRIPTEN__
#include <algorithm>
#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <mutex>
#include <string>
#include <thread>
#include <vector>
#endif

namespace pocket {

#ifdef __EMSCRIPTEN__

void parallel_for(std::size_t n, const std::function<void(std::size_t)>& fn) {
    for (std::size_t i = 0; i < n; ++i) fn(i);
}

std::size_t parallel_threads() { return 1; }

#else

namespace {

thread_local bool inside = false;   // this thread is running a batch's items

// Workers that sleep until a batch comes, then claim its items one at a time with the caller.
class Pool {
   public:
    explicit Pool(std::size_t workers) {
        for (std::size_t i = 0; i < workers; ++i) threads_.emplace_back([this] { loop(); });
    }
    ~Pool() {
        {
            std::lock_guard l(m_);
            stop_ = true;
        }
        wake_.notify_all();
        for (auto& t : threads_) t.join();
    }
    [[nodiscard]] std::size_t threads() const { return threads_.size() + 1; }

    void run(std::size_t n, const std::function<void(std::size_t)>& fn) {
        std::lock_guard one(batch_m_);   // one batch at a time
        {
            std::lock_guard l(m_);
            fn_ = &fn;
            n_ = n;
            next_.store(0);
            left_.store(n);
            ++batch_;
        }
        wake_.notify_all();
        take(fn, n);
        // Done when every item has run and no worker is still inside the batch (so none can
        // claim an item of the next one with this one's function).
        std::unique_lock l(m_);
        idle_.wait(l, [&] { return left_.load() == 0 && active_ == 0; });
        fn_ = nullptr;
    }

   private:
    void take(const std::function<void(std::size_t)>& fn, std::size_t n) {
        inside = true;
        for (;;) {
            const std::size_t i = next_.fetch_add(1);
            if (i >= n) break;
            fn(i);
            if (left_.fetch_sub(1) == 1) {
                std::lock_guard l(m_);
                idle_.notify_all();
            }
        }
        inside = false;
    }

    void loop() {
        std::uint64_t seen = 0;
        for (;;) {
            const std::function<void(std::size_t)>* fn = nullptr;
            std::size_t n = 0;
            {
                std::unique_lock l(m_);
                wake_.wait(l, [&] { return stop_ || batch_ != seen; });
                if (stop_) return;
                seen = batch_;
                if (!fn_) continue;   // that batch is over already
                fn = fn_;
                n = n_;
                ++active_;
            }
            take(*fn, n);
            {
                std::lock_guard l(m_);
                --active_;
            }
            idle_.notify_all();
        }
    }

    std::vector<std::thread> threads_;
    std::mutex m_, batch_m_;
    std::condition_variable wake_, idle_;
    const std::function<void(std::size_t)>* fn_ = nullptr;
    std::size_t n_ = 0;
    std::atomic<std::size_t> next_{0}, left_{0};
    std::uint64_t batch_ = 0;
    std::size_t active_ = 0;
    bool stop_ = false;
};

Pool& pool() {
    static Pool p([] {
        if (const char* t = std::getenv("POCKET_THREADS")) {
            const long want = std::strtol(t, nullptr, 10);
            if (want >= 1) return static_cast<std::size_t>(want - 1);
        }
        // A few of the cores: the rest are the renderer's, the audio's and the machine's.
        const unsigned hw = std::thread::hardware_concurrency();
        return hw > 1 ? std::min<std::size_t>(hw - 1, 7) : std::size_t{0};
    }());
    return p;
}

}  // namespace

void parallel_for(std::size_t n, const std::function<void(std::size_t)>& fn) {
    if (n == 0) return;
    if (n == 1 || inside || pool().threads() == 1) {
        for (std::size_t i = 0; i < n; ++i) fn(i);
        return;
    }
    pool().run(n, fn);
}

std::size_t parallel_threads() { return pool().threads(); }

#endif

}  // namespace pocket
