// Lockstep networking (docs/design/networking.md): TCP between each player and the host, one JSON
// object per line.
//
//   host -> joining player   {"t":"welcome","player":k,"players":n,"seed":s,"delay":d}
//   host -> everyone         {"t":"start"} once every player is in
//   anyone -> host -> others {"t":"in","tick":T,"p":k,"ev":[...]}   a player's input for tick T
//   anyone -> host           {"t":"hash","tick":T,"h":"..."}        the world after tick T
//   host -> everyone         {"t":"desync","tick":T,"hashes":{...}} {"t":"left","p":k,"tick":T}
#include <pocket/app/net.hpp>

#include <pocket/core/log.hpp>

#include <algorithm>
#include <chrono>
#include <cstring>
#include <map>
#include <thread>

#ifndef __EMSCRIPTEN__
#include <arpa/inet.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

#include <cerrno>
#endif

namespace pocket::app {

namespace {

struct Link {
    int fd = -1;
    std::string in, out;
    int player = -1;
    bool alive = true;
};

#ifndef __EMSCRIPTEN__
#ifdef MSG_NOSIGNAL
constexpr int kSendFlags = MSG_NOSIGNAL;
#else
constexpr int kSendFlags = 0;
#endif

void quiet_socket(int fd) {
    fcntl(fd, F_SETFL, fcntl(fd, F_GETFL, 0) | O_NONBLOCK);
    int one = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof one);
#ifdef SO_NOSIGPIPE
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
#endif
}
#endif

}  // namespace

struct Net::Impl {
    bool host = false, started = false;
    int player = 0, players = 2, delay = 3, port = 0;
    std::uint64_t seed = 1;
    int listen_fd = -1;
    int next_player = 1;
    std::vector<Link> links;                                      // host: one per joined player; a player: the host
    std::map<std::int64_t, std::map<int, Json>> inputs;           // tick -> player -> events
    std::map<int, std::int64_t> last_from;                        // player -> the last tick their input came for
    std::map<int, std::int64_t> left;                             // player -> the first tick without them
    std::map<std::int64_t, std::map<int, std::string>> hashes;    // host: tick -> player -> world hash
    std::vector<Json> notes;
    std::int64_t desyncs = 0, first_desync = -1;
    std::uint64_t bytes_in = 0, bytes_out = 0;

    void send(Link& l, const Json& m) {
        if (!l.alive) return;
        l.out += m.dump();
        l.out += '\n';
    }
    void broadcast(const Json& m, int except = -1) {
        for (Link& l : links) if (l.alive && l.player != except) send(l, m);
    }

    void drop(Link& l) {
        if (!l.alive) return;
        l.alive = false;
#ifndef __EMSCRIPTEN__
        if (l.fd >= 0) ::close(l.fd);
#endif
        l.fd = -1;
        if (host && l.player > 0) {
            // Everyone goes on without them from the tick after the last input they sent.
            auto it = last_from.find(l.player);
            const std::int64_t at = it == last_from.end() ? delay : it->second + 1;
            left[l.player] = at;
            broadcast(Json{{"t", "left"}, {"p", l.player}, {"tick", at}});
            notes.push_back(Json{{"type", "net.left"}, {"player", l.player}, {"tick", at}});
            log::warn("net", "player {} left; the game goes on without them from tick {}", l.player, at);
        } else if (!host) {
            notes.push_back(Json{{"type", "net.lost"}});
            log::warn("net", "the connection to the host is gone");
        }
    }

    void check_hashes(std::int64_t tick) {
        auto it = hashes.find(tick);
        if (it == hashes.end()) return;
        int expected = 0;
        for (int p = 0; p < players; ++p) {
            auto l = left.find(p);
            if (l == left.end() || l->second > tick) ++expected;
        }
        if (static_cast<int>(it->second.size()) < expected) return;
        const std::string& first = it->second.begin()->second;
        bool same = true;
        for (const auto& [p, h] : it->second) same = same && h == first;
        if (!same) {
            Json hs = Json::object();
            for (const auto& [p, h] : it->second) hs[std::to_string(p)] = h;
            ++desyncs;
            if (first_desync < 0) first_desync = tick;
            notes.push_back(Json{{"type", "net.desync"}, {"tick", tick}, {"hashes", hs}});
            broadcast(Json{{"t", "desync"}, {"tick", tick}, {"hashes", hs}});
            log::warn("net", "desync at tick {}: the players' worlds differ", tick);
        }
        hashes.erase(it);
    }

    void handle(Link& from, const Json& m) {
        const std::string t = m.value("t", "");
        if (host) {
            if (t == "in" && m.contains("tick") && m["tick"].is_number()) {
                const std::int64_t tick = m["tick"].get<std::int64_t>();
                const Json ev = m.value("ev", Json::array());
                inputs[tick][from.player] = ev;
                last_from[from.player] = std::max(last_from[from.player], tick);
                broadcast(Json{{"t", "in"}, {"tick", tick}, {"p", from.player}, {"ev", ev}}, from.player);
            } else if (t == "hash" && m.contains("tick")) {
                const std::int64_t tick = m["tick"].get<std::int64_t>();
                hashes[tick][from.player] = m.value("h", "");
                check_hashes(tick);
            }
            return;
        }
        if (t == "welcome") {
            player = m.value("player", 1);
            players = m.value("players", 2);
            seed = m.value("seed", std::uint64_t{1});
            delay = m.value("delay", 3);
        } else if (t == "start") {
            started = true;
            notes.push_back(Json{{"type", "net.started"}, {"players", players}});
        } else if (t == "in" && m.contains("tick")) {
            inputs[m["tick"].get<std::int64_t>()][m.value("p", 0)] = m.value("ev", Json::array());
        } else if (t == "left") {
            left[m.value("p", 0)] = m.value("tick", std::int64_t{0});
            notes.push_back(Json{{"type", "net.left"}, {"player", m.value("p", 0)}, {"tick", m.value("tick", std::int64_t{0})}});
        } else if (t == "desync") {
            ++desyncs;
            if (first_desync < 0) first_desync = m.value("tick", std::int64_t{0});
            notes.push_back(Json{{"type", "net.desync"}, {"tick", m.value("tick", std::int64_t{0})}, {"hashes", m.value("hashes", Json::object())}});
        } else if (t == "full") {
            notes.push_back(Json{{"type", "net.refused"}});
        }
    }

    void read_link(Link& l) {
#ifndef __EMSCRIPTEN__
        char buf[65536];
        while (l.alive) {
            const ssize_t n = ::recv(l.fd, buf, sizeof buf, 0);
            if (n > 0) {
                l.in.append(buf, static_cast<std::size_t>(n));
                bytes_in += static_cast<std::uint64_t>(n);
                continue;
            }
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR)) break;
            drop(l);
            break;
        }
        std::size_t start = 0;
        for (std::size_t nl = l.in.find('\n'); nl != std::string::npos; nl = l.in.find('\n', start)) {
            Json m = Json::parse(l.in.substr(start, nl - start), nullptr, false);
            start = nl + 1;
            if (m.is_object()) handle(l, m);
        }
        l.in.erase(0, start);
#else
        (void)l;
#endif
    }

    void flush_link(Link& l) {
#ifndef __EMSCRIPTEN__
        while (l.alive && !l.out.empty()) {
            const ssize_t n = ::send(l.fd, l.out.data(), l.out.size(), kSendFlags);
            if (n > 0) {
                l.out.erase(0, static_cast<std::size_t>(n));
                bytes_out += static_cast<std::uint64_t>(n);
                continue;
            }
            if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR)) break;
            drop(l);
        }
#else
        (void)l;
#endif
    }

    void accept_players() {
#ifndef __EMSCRIPTEN__
        if (listen_fd < 0) return;
        for (;;) {
            const int fd = ::accept(listen_fd, nullptr, nullptr);
            if (fd < 0) break;
            quiet_socket(fd);
            Link l;
            l.fd = fd;
            if (started || next_player >= players) {
                send(l, Json{{"t", "full"}});
                flush_link(l);
                ::close(fd);
                continue;
            }
            l.player = next_player++;
            send(l, Json{{"t", "welcome"}, {"player", l.player}, {"players", players}, {"seed", seed}, {"delay", delay}});
            links.push_back(std::move(l));
            notes.push_back(Json{{"type", "net.joined"}, {"player", links.back().player}});
            log::info("net", "player {} joined ({} of {})", links.back().player, next_player, players);
            if (next_player >= players) {
                started = true;
                broadcast(Json{{"t", "start"}});
                notes.push_back(Json{{"type", "net.started"}, {"players", players}});
                log::info("net", "all {} players are in: the game starts", players);
            }
        }
#endif
    }
};

Net::Net() : impl_(std::make_unique<Impl>()) {}

Net::~Net() {
#ifndef __EMSCRIPTEN__
    for (Link& l : impl_->links) {
        impl_->flush_link(l);
        if (l.fd >= 0) ::close(l.fd);
    }
    if (impl_->listen_fd >= 0) ::close(impl_->listen_fd);
#endif
}

Result<std::unique_ptr<Net>> Net::host(int port, int players, int delay, std::uint64_t seed) {
#ifdef __EMSCRIPTEN__
    (void)port; (void)players; (void)delay; (void)seed;
    return fail("unsupported", "networking is not in the browser build");
#else
    std::unique_ptr<Net> net(new Net());
    Impl& im = *net->impl_;
    im.host = true;
    im.player = 0;
    im.players = std::clamp(players, 1, 16);
    im.delay = std::clamp(delay, 1, 60);
    im.seed = seed;
    im.listen_fd = ::socket(AF_INET, SOCK_STREAM, 0);
    if (im.listen_fd < 0) return fail("net_error", "socket: {}", std::strerror(errno));
    int one = 1;
    setsockopt(im.listen_fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);
    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_ANY);
    addr.sin_port = htons(static_cast<std::uint16_t>(port));
    if (::bind(im.listen_fd, reinterpret_cast<sockaddr*>(&addr), sizeof addr) != 0) return fail("net_error", "cannot listen on port {}: {}", port, std::strerror(errno));
    if (::listen(im.listen_fd, 8) != 0) return fail("net_error", "listen: {}", std::strerror(errno));
    fcntl(im.listen_fd, F_SETFL, fcntl(im.listen_fd, F_GETFL, 0) | O_NONBLOCK);
    socklen_t len = sizeof addr;
    getsockname(im.listen_fd, reinterpret_cast<sockaddr*>(&addr), &len);
    im.port = ntohs(addr.sin_port);
    if (im.players <= 1) {
        im.started = true;
        im.notes.push_back(Json{{"type", "net.started"}, {"players", 1}});
    }
    log::info("net", "hosting on port {} for {} players (input delay {} ticks)", im.port, im.players, im.delay);
    return net;
#endif
}

Result<std::unique_ptr<Net>> Net::join(const std::string& address, double timeout) {
#ifdef __EMSCRIPTEN__
    (void)address; (void)timeout;
    return fail("unsupported", "networking is not in the browser build");
#else
    const auto colon = address.rfind(':');
    if (colon == std::string::npos) return fail("bad_args", "join needs host:port, got '{}'", address);
    const std::string hostname = address.substr(0, colon), service = address.substr(colon + 1);
    addrinfo hints{};
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    addrinfo* found = nullptr;
    if (getaddrinfo(hostname.c_str(), service.c_str(), &hints, &found) != 0 || !found) return fail("net_error", "cannot resolve {}", address);
    std::unique_ptr<Net> net(new Net());
    Impl& im = *net->impl_;
    const auto until = std::chrono::steady_clock::now() + std::chrono::duration<double>(timeout);
    int fd = -1;
    // The host may still be starting: try again until the timeout.
    while (fd < 0) {
        fd = ::socket(found->ai_family, found->ai_socktype, found->ai_protocol);
        if (fd >= 0 && ::connect(fd, found->ai_addr, found->ai_addrlen) == 0) break;
        if (fd >= 0) ::close(fd);
        fd = -1;
        if (std::chrono::steady_clock::now() > until) {
            freeaddrinfo(found);
            return fail("net_error", "no host answered at {}", address);
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    freeaddrinfo(found);
    quiet_socket(fd);
    Link l;
    l.fd = fd;
    l.player = 0;
    im.links.push_back(std::move(l));
    bool welcomed = false;
    im.player = -1;
    while (!welcomed) {
        im.read_link(im.links[0]);
        welcomed = im.player >= 0;
        if (!im.links[0].alive) return fail("net_error", "the host at {} closed the connection (the game may be full or running)", address);
        if (!welcomed && std::chrono::steady_clock::now() > until) return fail("net_error", "the host at {} did not welcome us in {} s", address, timeout);
        if (!welcomed) std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
    log::info("net", "joined {} as player {} of {} (seed {}, input delay {} ticks)", address, im.player, im.players, im.seed, im.delay);
    return net;
#endif
}

void Net::pump() {
    Impl& im = *impl_;
    if (im.host) im.accept_players();
    for (Link& l : im.links) im.read_link(l);
    for (Link& l : im.links) im.flush_link(l);
}

bool Net::started() const { return impl_->started; }
bool Net::is_host() const { return impl_->host; }
int Net::player() const { return impl_->player; }
int Net::players() const { return impl_->players; }
int Net::delay() const { return impl_->delay; }
int Net::port() const { return impl_->port; }
std::uint64_t Net::seed() const { return impl_->seed; }

void Net::commit(std::int64_t tick, Json events) {
    Impl& im = *impl_;
    im.inputs[tick][im.player] = events;
    im.last_from[im.player] = std::max(im.last_from[im.player], tick);
    const Json m{{"t", "in"}, {"tick", tick}, {"p", im.player}, {"ev", std::move(events)}};
    if (im.host) im.broadcast(m);
    else if (!im.links.empty()) im.send(im.links[0], m);
    pump();
}

bool Net::ready(std::int64_t tick) const {
    const Impl& im = *impl_;
    if (tick < im.delay) return true;
    auto it = im.inputs.find(tick);
    for (int p = 0; p < im.players; ++p) {
        if (it != im.inputs.end() && it->second.contains(p)) continue;
        auto l = im.left.find(p);
        if (l != im.left.end() && l->second <= tick) continue;
        return false;
    }
    return true;
}

std::vector<Json> Net::inputs(std::int64_t tick) const {
    const Impl& im = *impl_;
    std::vector<Json> out(static_cast<std::size_t>(im.players), Json::array());
    auto it = im.inputs.find(tick);
    if (it == im.inputs.end()) return out;
    for (const auto& [p, ev] : it->second) {
        if (p < 0 || p >= im.players) continue;
        auto l = im.left.find(p);
        if (l != im.left.end() && l->second <= tick) continue;   // after they left, nothing of theirs counts
        out[static_cast<std::size_t>(p)] = ev;
    }
    return out;
}

void Net::release(std::int64_t tick) {
    Impl& im = *impl_;
    im.inputs.erase(im.inputs.begin(), im.inputs.lower_bound(tick));
    im.hashes.erase(im.hashes.begin(), im.hashes.lower_bound(tick - 600));
}

void Net::report_hash(std::int64_t tick, const std::string& hash) {
    Impl& im = *impl_;
    if (im.host) {
        im.hashes[tick][0] = hash;
        im.check_hashes(tick);
    } else if (!im.links.empty()) {
        im.send(im.links[0], Json{{"t", "hash"}, {"tick", tick}, {"h", hash}});
    }
}

std::vector<Json> Net::take_notes() {
    std::vector<Json> out = std::move(impl_->notes);
    impl_->notes.clear();
    return out;
}

Json Net::info() const {
    const Impl& im = *impl_;
    Json j;
    j["mode"] = im.host ? "host" : "player";
    j["player"] = im.player;
    j["players"] = im.players;
    j["started"] = im.started;
    j["delay"] = im.delay;
    if (im.host) j["port"] = im.port;
    int connected = 0;
    for (const Link& l : im.links) connected += l.alive ? 1 : 0;
    j["connected"] = connected;
    Json left = Json::object();
    for (const auto& [p, t] : im.left) left[std::to_string(p)] = t;
    j["left"] = left;
    j["desyncs"] = im.desyncs;
    if (im.first_desync >= 0) j["first_desync"] = im.first_desync;
    j["pending_ticks"] = im.inputs.size();
    j["bytes_in"] = im.bytes_in;
    j["bytes_out"] = im.bytes_out;
    return j;
}

}  // namespace pocket::app
