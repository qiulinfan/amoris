// Lockstep networking (docs/design/networking.md). Every peer runs the whole game from the same
// scene and seed; a tick runs only once every player's input for it has arrived, so every peer's
// world stays the same without sending any of it. The host is player 0: the others connect to it,
// it hands out player numbers and relays every input to everyone, and it compares the world hashes
// the peers report to catch a desync.
#pragma once

#include <pocket/core/core.hpp>

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace pocket::app {

class Net {
   public:
    ~Net();
    Net(const Net&) = delete;
    Net& operator=(const Net&) = delete;

    // Listen on `port` (0: any free port) for `players` - 1 others; the game starts when they are all in.
    // `dedicated`: the host runs the game and relays it without playing (no input of its own); the
    // players are the `players` who join, numbered from 0.
    static Result<std::unique_ptr<Net>> host(int port, int players, int delay, std::uint64_t seed, bool dedicated = false);
    // Connect to a host ("address:port") and wait up to `timeout` seconds for its welcome: this
    // peer's player number, the player count, the seed and the input delay.
    static Result<std::unique_ptr<Net>> join(const std::string& address, double timeout);

    // Move bytes: greet joining players (the host), read what arrived, send what is queued.
    void pump();
    [[nodiscard]] bool started() const;
    [[nodiscard]] bool is_host() const;
    [[nodiscard]] int player() const;
    [[nodiscard]] int players() const;
    [[nodiscard]] int delay() const;
    [[nodiscard]] int port() const;
    [[nodiscard]] std::uint64_t seed() const;
    // A player who came back into a running game and is still replaying it up to the others: the
    // session runs its ticks as fast as they go, not at the clock's pace.
    [[nodiscard]] bool catching_up() const;

    // This peer's input for `tick` (an array of input events), for everyone.
    void commit(std::int64_t tick, Json events);
    // Whether every player's input for `tick` is here (the first `delay` ticks have none to wait for,
    // and a player who left has none from the tick the host says they left).
    [[nodiscard]] bool ready(std::int64_t tick) const;
    // Every player's input for `tick`, by player number (empty arrays where there is none).
    [[nodiscard]] std::vector<Json> inputs(std::int64_t tick) const;
    // Forget the inputs of ticks before `tick` (already run).
    void release(std::int64_t tick);
    // This peer's world hash after `tick`; the host compares everyone's.
    void report_hash(std::int64_t tick, const std::string& hash);
    // What happened since the last call, for the event log: net.joined, net.started, net.left,
    // net.back, net.desync.
    std::vector<Json> take_notes();
    [[nodiscard]] Json info() const;

   private:
    Net();
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

}  // namespace pocket::app
