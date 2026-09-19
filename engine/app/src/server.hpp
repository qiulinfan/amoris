// HTTP control server: JSON-RPC 2.0 over POST /rpc, executed on the main thread between frames.
#pragma once

#include <pocket/app/session.hpp>

#include <memory>

namespace pocket::app {

class ControlServer {
   public:
    ControlServer(Session& session, int port);
    ~ControlServer();
    Status start();
    // Run queued requests on the calling (main) thread. When `wait_ms` > 0, block up to that long
    // for a request to arrive (used while paused).
    void pump(int wait_ms = 0);
    [[nodiscard]] int port() const;
    [[nodiscard]] Json describe() const;

   private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::app
