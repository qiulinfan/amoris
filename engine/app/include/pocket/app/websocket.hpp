// WebSockets (RFC 6455), the part the lockstep networking needs so a browser can join a game
// (docs/design/networking.md, Browsers): the handshake's accept key and the frames around each
// message, both ways.
#pragma once

#include <array>
#include <cstdint>
#include <string>
#include <string_view>

namespace pocket::app::ws {

std::array<std::uint8_t, 20> sha1(std::string_view data);
std::string base64(const std::uint8_t* data, std::size_t n);
// The Sec-WebSocket-Accept value for a client's Sec-WebSocket-Key.
std::string accept_key(const std::string& key);

enum Opcode { Text = 1, Binary = 2, Close = 8, Ping = 9, Pong = 10 };
// A whole frame: FIN, the opcode, the length (7, 16 or 64 bits), the payload masked with `mask`
// when a client sends it (servers send unmasked frames).
std::string frame(std::string_view payload, int opcode, const std::uint8_t* mask = nullptr);
// The next whole frame at the front of `in`, unmasked, taken off it; false while it is not all there.
bool take(std::string& in, int& opcode, std::string& payload);

}  // namespace pocket::app::ws
