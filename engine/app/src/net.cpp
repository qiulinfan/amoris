// Lockstep networking (docs/design/networking.md): TCP between each player and the host, one JSON
// object per line, or a WebSocket, one JSON object per frame (a browser cannot open TCP). The host
// tells them apart by a newcomer's first bytes: a TCP player says hello, a WebSocket asks for the
// upgrade.
//
//   joining player -> host   {"t":"hello"} (TCP) or the WebSocket handshake
//   host -> joining player   {"t":"welcome","player":k,"players":n,"seed":s,"delay":d}
//   host -> everyone         {"t":"start"} once every player is in
//   anyone -> host -> others {"t":"in","tick":T,"p":k,"ev":[...]}   a player's input for tick T
//   anyone -> host           {"t":"hash","tick":T,"h":"..."}        the world after tick T
//   host -> everyone         {"t":"desync","tick":T,"hashes":{...}} {"t":"left","p":k,"tick":T}
#include <pocket/app/net.hpp>
#include <pocket/app/websocket.hpp>

#include <pocket/core/log.hpp>

#include <algorithm>
#include <cctype>
#include <chrono>
#include <cstring>
#include <map>
#include <random>
#include <thread>

#ifdef __EMSCRIPTEN__
#include <emscripten/emscripten.h>
#include <emscripten/websocket.h>
#endif

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
    // How it talks: JSON lines over TCP, or a JSON message a WebSocket frame (the host's frames
    // unmasked, a client's masked); a newcomer at the host is Unknown until its first bytes say.
    // A browser's socket (Web) hands whole messages to callbacks, which queue them as lines.
    enum class Kind { Unknown, Tcp, WsServer, WsClient, Web } kind = Kind::Tcp;
    int web = 0;                       // Web: the browser's socket
    bool open = false;                 // Web: the socket is open (messages wait in the outbox until then)
    std::vector<std::string> outbox;   // Web: messages to send
};

// "ws://host:port/path" or "host:port": whether it is a WebSocket, the host and the port.
bool parse_address(const std::string& address, bool& websocket, std::string& host, std::string& port) {
    std::string rest = address;
    websocket = rest.rfind("ws://", 0) == 0;
    if (websocket) rest = rest.substr(5);
    if (const auto slash = rest.find('/'); slash != std::string::npos) rest = rest.substr(0, slash);
    const auto colon = rest.rfind(':');
    if (colon == std::string::npos || colon == 0 || colon + 1 >= rest.size()) return false;
    host = rest.substr(0, colon);
    port = rest.substr(colon + 1);
    return true;
}

// A header's value in an HTTP request (names are case-insensitive), empty when absent.
std::string header(const std::string& request, std::string_view name) {
    std::string lower = request;
    std::transform(lower.begin(), lower.end(), lower.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    std::string key(name);
    std::transform(key.begin(), key.end(), key.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    for (std::size_t at = lower.find(key + ":"); at != std::string::npos; at = lower.find(key + ":", at + 1)) {
        if (at != 0 && lower[at - 1] != '\n') continue;
        std::size_t v = at + key.size() + 1;
        const std::size_t end = request.find("\r\n", v);
        while (v < end && request[v] == ' ') ++v;
        return request.substr(v, end - v);
    }
    return {};
}

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
    std::vector<Link> pending;                                    // host: connected, not yet known to be a player
    std::minstd_rand masks{static_cast<std::uint32_t>(std::chrono::steady_clock::now().time_since_epoch().count())};

    void send(Link& l, const Json& m) {
        if (!l.alive) return;
        const std::string text = m.dump();
        if (l.kind == Link::Kind::WsServer) {
            l.out += ws::frame(text, ws::Text);
        } else if (l.kind == Link::Kind::WsClient) {
            std::uint8_t mask[4];
            for (auto& b : mask) b = static_cast<std::uint8_t>(masks());
            l.out += ws::frame(text, ws::Text, mask);
        } else if (l.kind == Link::Kind::Web) {
            l.outbox.push_back(text);
        } else {
            l.out += text;
            l.out += '\n';
        }
    }
    void broadcast(const Json& m, int except = -1) {
        for (Link& l : links) if (l.alive && l.player != except) send(l, m);
    }

    void drop(Link& l) {
        if (!l.alive) return;
        l.alive = false;
#ifndef __EMSCRIPTEN__
        if (l.fd >= 0) ::close(l.fd);
#else
        if (l.web > 0) { emscripten_websocket_close(l.web, 1000, "gone"); emscripten_websocket_delete(l.web); l.web = 0; }
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

    // What arrived on a native socket, appended to the link's input.
    void receive(Link& l) {
#ifndef __EMSCRIPTEN__
        char buf[65536];
        while (l.alive && l.fd >= 0) {
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
#else
        (void)l;
#endif
    }

    void read_link(Link& l) {
        receive(l);
        if (l.kind == Link::Kind::WsServer || l.kind == Link::Kind::WsClient) {
            // Frames: text and binary carry a message; a ping is answered; a close ends it.
            int op = 0;
            std::string msg;
            while (l.alive && ws::take(l.in, op, msg)) {
                if (op == ws::Text || op == ws::Binary) {
                    Json m = Json::parse(msg, nullptr, false);
                    if (m.is_object()) handle(l, m);
                } else if (op == ws::Ping) {
                    std::uint8_t mask[4];
                    for (auto& b : mask) b = static_cast<std::uint8_t>(masks());
                    l.out += ws::frame(msg, ws::Pong, l.kind == Link::Kind::WsClient ? mask : nullptr);
                } else if (op == ws::Close) {
                    drop(l);
                }
            }
            return;
        }
        std::size_t start = 0;
        for (std::size_t nl = l.in.find('\n'); nl != std::string::npos; nl = l.in.find('\n', start)) {
            Json m = Json::parse(l.in.substr(start, nl - start), nullptr, false);
            start = nl + 1;
            if (m.is_object()) handle(l, m);
        }
        l.in.erase(0, start);
    }

    void flush_link(Link& l) {
#ifdef __EMSCRIPTEN__
        if (l.kind == Link::Kind::Web && l.alive && l.open) {
            for (const std::string& m : l.outbox) {
                emscripten_websocket_send_utf8_text(l.web, m.c_str());
                bytes_out += m.size();
            }
            l.outbox.clear();
        }
#else
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
#endif
    }

    // Newcomers: accepted, then told apart by their first bytes (a TCP player's hello line, a
    // WebSocket's upgrade request) and let in, or turned away (the game full or started, or bytes
    // that are neither).
    void accept_players() {
#ifndef __EMSCRIPTEN__
        if (listen_fd < 0) return;
        for (;;) {
            const int fd = ::accept(listen_fd, nullptr, nullptr);
            if (fd < 0) break;
            quiet_socket(fd);
            Link l;
            l.fd = fd;
            l.kind = Link::Kind::Unknown;
            pending.push_back(std::move(l));
        }
        for (Link& l : pending) {
            receive(l);
            if (!l.alive) continue;
            if (l.in.rfind("GET ", 0) == 0) {
                const std::size_t end = l.in.find("\r\n\r\n");
                if (end == std::string::npos) { if (l.in.size() > 16384) drop(l); continue; }
                const std::string key = header(l.in.substr(0, end + 2), "Sec-WebSocket-Key");
                if (key.empty()) {
                    l.out += "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n";
                    flush_link(l);
                    drop(l);
                    continue;
                }
                l.out += "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " + ws::accept_key(key) + "\r\n\r\n";
                l.in.erase(0, end + 4);
                l.kind = Link::Kind::WsServer;
                admit(l);
            } else if (!l.in.empty() && l.in[0] == '{') {
                const std::size_t nl = l.in.find('\n');
                if (nl == std::string::npos) { if (l.in.size() > 16384) drop(l); continue; }
                const Json hello = Json::parse(l.in.substr(0, nl), nullptr, false);
                l.in.erase(0, nl + 1);
                if (!hello.is_object() || hello.value("t", "") != "hello") { drop(l); continue; }
                l.kind = Link::Kind::Tcp;
                admit(l);
            } else if (!l.in.empty()) {
                drop(l);
            }
        }
        pending.erase(std::remove_if(pending.begin(), pending.end(), [](const Link& l) { return !l.alive || l.kind != Link::Kind::Unknown; }), pending.end());
#endif
    }

    // A newcomer whose way of talking is known: a player, or turned away.
    void admit(Link& from) {
#ifndef __EMSCRIPTEN__
        Link l = std::move(from);
        from.alive = false;
        from.fd = -1;
        {
            if (started || next_player >= players) {
                send(l, Json{{"t", "full"}});
                flush_link(l);
                ::close(l.fd);
                return;
            }
            l.player = next_player++;
            send(l, Json{{"t", "welcome"}, {"player", l.player}, {"players", players}, {"seed", seed}, {"delay", delay}});
            const bool websocket = l.kind == Link::Kind::WsServer;
            links.push_back(std::move(l));
            notes.push_back(Json{{"type", "net.joined"}, {"player", links.back().player}, {"websocket", websocket}});
            log::info("net", "player {} joined ({} of {}){}", links.back().player, next_player, players, websocket ? " over a WebSocket" : "");
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
    for (Link& l : impl_->pending) if (l.fd >= 0) ::close(l.fd);
    if (impl_->listen_fd >= 0) ::close(impl_->listen_fd);
#else
    for (Link& l : impl_->links) {
        impl_->flush_link(l);
        if (l.web > 0) { emscripten_websocket_close(l.web, 1000, "bye"); emscripten_websocket_delete(l.web); }
    }
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
    bool websocket = false;
    std::string hostname, service;
    if (!parse_address(address, websocket, hostname, service)) return fail("bad_args", "join needs host:port or ws://host:port, got '{}'", address);
    std::unique_ptr<Net> net(new Net());
    Impl& im = *net->impl_;
    const auto until = std::chrono::steady_clock::now() + std::chrono::duration<double>(timeout);
    im.player = -1;
#ifdef __EMSCRIPTEN__
    // The browser's own WebSocket: its callbacks queue what arrives as lines and mark it open or
    // gone; the wait for the welcome sleeps, so the page's event loop runs them.
    Link l;
    l.kind = Link::Kind::Web;
    l.player = 0;
    im.links.push_back(std::move(l));
    EmscriptenWebSocketCreateAttributes attr;
    emscripten_websocket_init_create_attributes(&attr);
    const std::string url = "ws://" + hostname + ":" + service;
    attr.url = url.c_str();
    attr.createOnMainThread = EM_TRUE;
    const EMSCRIPTEN_WEBSOCKET_T sock = emscripten_websocket_new(&attr);
    if (sock <= 0) return fail("net_error", "cannot open a WebSocket to {}", url);
    im.links[0].web = sock;
    auto on_open = [](int, const EmscriptenWebSocketOpenEvent*, void* user) -> EM_BOOL {
        static_cast<Impl*>(user)->links[0].open = true;
        return EM_TRUE;
    };
    auto on_message = [](int, const EmscriptenWebSocketMessageEvent* e, void* user) -> EM_BOOL {
        Impl* impl = static_cast<Impl*>(user);
        if (e->isText && e->data) {
            impl->links[0].in += reinterpret_cast<const char*>(e->data);
            impl->links[0].in += '\n';
            impl->bytes_in += e->numBytes;
        }
        return EM_TRUE;
    };
    static const auto on_gone = [](int, const void*, void* user) -> EM_BOOL {
        Impl* impl = static_cast<Impl*>(user);
        impl->links[0].open = false;
        impl->drop(impl->links[0]);
        return EM_TRUE;
    };
    emscripten_websocket_set_onopen_callback(sock, &im, on_open);
    emscripten_websocket_set_onmessage_callback(sock, &im, on_message);
    emscripten_websocket_set_onclose_callback(sock, &im, [](int t, const EmscriptenWebSocketCloseEvent* e, void* u) -> EM_BOOL { return on_gone(t, e, u); });
    emscripten_websocket_set_onerror_callback(sock, &im, [](int t, const EmscriptenWebSocketErrorEvent* e, void* u) -> EM_BOOL { return on_gone(t, e, u); });
    while (im.player < 0) {
        emscripten_sleep(10);
        im.read_link(im.links[0]);
        if (!im.links[0].alive) return fail("net_error", "the host at {} closed the connection (no game there, or it is full or running)", url);
        if (std::chrono::steady_clock::now() > until) return fail("net_error", "the host at {} did not welcome us in {} s", url, timeout);
    }
#else
    addrinfo hints{};
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    addrinfo* found = nullptr;
    if (getaddrinfo(hostname.c_str(), service.c_str(), &hints, &found) != 0 || !found) return fail("net_error", "cannot resolve {}", address);
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
    Link& host = im.links[0];
    if (websocket) {
        // The upgrade, then frames.
        std::uint8_t nonce[16];
        for (auto& b : nonce) b = static_cast<std::uint8_t>(im.masks());
        const std::string key = ws::base64(nonce, sizeof nonce);
        host.out += "GET / HTTP/1.1\r\nHost: " + hostname + ":" + service + "\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: " + key + "\r\nSec-WebSocket-Version: 13\r\n\r\n";
        im.flush_link(host);
        std::size_t end = std::string::npos;
        while (end == std::string::npos) {
            im.receive(host);
            end = host.in.find("\r\n\r\n");
            if (!host.alive) return fail("net_error", "the host at {} closed the connection during the WebSocket handshake", address);
            if (end == std::string::npos && std::chrono::steady_clock::now() > until) return fail("net_error", "the host at {} did not answer the WebSocket handshake in {} s", address, timeout);
            if (end == std::string::npos) std::this_thread::sleep_for(std::chrono::milliseconds(5));
        }
        const std::string answer = host.in.substr(0, end + 2);
        host.in.erase(0, end + 4);
        if (answer.find(" 101 ") == std::string::npos || header(answer, "Sec-WebSocket-Accept") != ws::accept_key(key)) return fail("net_error", "the host at {} refused the WebSocket: {}", address, answer.substr(0, answer.find("\r\n")));
        host.kind = Link::Kind::WsClient;
    } else {
        host.kind = Link::Kind::Tcp;
        im.send(host, Json{{"t", "hello"}});
        im.flush_link(host);
    }
    while (im.player < 0) {
        im.read_link(im.links[0]);
        im.flush_link(im.links[0]);
        if (!im.links[0].alive) return fail("net_error", "the host at {} closed the connection (the game may be full or running)", address);
        if (im.player < 0 && std::chrono::steady_clock::now() > until) return fail("net_error", "the host at {} did not welcome us in {} s", address, timeout);
        if (im.player < 0) std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
#endif
    log::info("net", "joined {} as player {} of {} (seed {}, input delay {} ticks)", address, im.player, im.players, im.seed, im.delay);
    return net;
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
