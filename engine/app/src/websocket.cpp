#include <pocket/app/websocket.hpp>

namespace pocket::app::ws {

std::array<std::uint8_t, 20> sha1(std::string_view data) {
    std::uint32_t h[5] = {0x67452301u, 0xEFCDAB89u, 0x98BADCFEu, 0x10325476u, 0xC3D2E1F0u};
    std::string m(data);
    const std::uint64_t bits = static_cast<std::uint64_t>(data.size()) * 8;
    m.push_back(static_cast<char>(0x80));
    while (m.size() % 64 != 56) m.push_back('\0');
    for (int i = 7; i >= 0; --i) m.push_back(static_cast<char>((bits >> (i * 8)) & 0xFF));
    auto rol = [](std::uint32_t x, int n) { return (x << n) | (x >> (32 - n)); };
    for (std::size_t chunk = 0; chunk < m.size(); chunk += 64) {
        std::uint32_t w[80];
        for (int i = 0; i < 16; ++i) {
            const auto* p = reinterpret_cast<const unsigned char*>(m.data() + chunk + static_cast<std::size_t>(i) * 4);
            w[i] = (std::uint32_t{p[0]} << 24) | (std::uint32_t{p[1]} << 16) | (std::uint32_t{p[2]} << 8) | std::uint32_t{p[3]};
        }
        for (int i = 16; i < 80; ++i) w[i] = rol(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
        std::uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4];
        for (int i = 0; i < 80; ++i) {
            std::uint32_t f, k;
            if (i < 20) { f = (b & c) | (~b & d); k = 0x5A827999u; }
            else if (i < 40) { f = b ^ c ^ d; k = 0x6ED9EBA1u; }
            else if (i < 60) { f = (b & c) | (b & d) | (c & d); k = 0x8F1BBCDCu; }
            else { f = b ^ c ^ d; k = 0xCA62C1D6u; }
            const std::uint32_t t = rol(a, 5) + f + e + k + w[i];
            e = d; d = c; c = rol(b, 30); b = a; a = t;
        }
        h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e;
    }
    std::array<std::uint8_t, 20> out{};
    for (int i = 0; i < 5; ++i) for (int j = 0; j < 4; ++j) out[static_cast<std::size_t>(i * 4 + j)] = static_cast<std::uint8_t>(h[i] >> (24 - j * 8));
    return out;
}

std::string base64(const std::uint8_t* data, std::size_t n) {
    static const char* abc = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string out;
    for (std::size_t i = 0; i < n; i += 3) {
        const std::uint32_t v = (std::uint32_t{data[i]} << 16) | (i + 1 < n ? std::uint32_t{data[i + 1]} << 8 : 0) | (i + 2 < n ? std::uint32_t{data[i + 2]} : 0);
        out += abc[(v >> 18) & 63];
        out += abc[(v >> 12) & 63];
        out += i + 1 < n ? abc[(v >> 6) & 63] : '=';
        out += i + 2 < n ? abc[v & 63] : '=';
    }
    return out;
}

std::string accept_key(const std::string& key) {
    const auto d = sha1(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    return base64(d.data(), d.size());
}

std::string frame(std::string_view payload, int opcode, const std::uint8_t* mask) {
    std::string f;
    f.push_back(static_cast<char>(0x80 | opcode));
    const std::uint8_t m = mask ? 0x80 : 0;
    const std::uint64_t n = payload.size();
    if (n < 126) f.push_back(static_cast<char>(m | n));
    else if (n < 65536) { f.push_back(static_cast<char>(m | 126)); f.push_back(static_cast<char>(n >> 8)); f.push_back(static_cast<char>(n & 0xFF)); }
    else { f.push_back(static_cast<char>(m | 127)); for (int i = 7; i >= 0; --i) f.push_back(static_cast<char>((n >> (i * 8)) & 0xFF)); }
    if (mask) {
        for (int i = 0; i < 4; ++i) f.push_back(static_cast<char>(mask[i]));
        for (std::size_t i = 0; i < n; ++i) f.push_back(static_cast<char>(static_cast<std::uint8_t>(payload[i]) ^ mask[i % 4]));
    } else {
        f.append(payload);
    }
    return f;
}

bool take(std::string& in, int& opcode, std::string& payload) {
    if (in.size() < 2) return false;
    const auto* b = reinterpret_cast<const unsigned char*>(in.data());
    opcode = b[0] & 0x0F;
    const bool masked = b[1] & 0x80;
    std::uint64_t n = b[1] & 0x7F;
    std::size_t at = 2;
    if (n == 126) { if (in.size() < 4) return false; n = (std::uint64_t{b[2]} << 8) | b[3]; at = 4; }
    else if (n == 127) { if (in.size() < 10) return false; n = 0; for (int i = 0; i < 8; ++i) n = (n << 8) | b[2 + i]; at = 10; }
    const std::size_t head = at + (masked ? 4 : 0);
    if (in.size() < head + n) return false;
    payload.assign(in, head, static_cast<std::size_t>(n));
    if (masked) for (std::size_t i = 0; i < payload.size(); ++i) payload[i] = static_cast<char>(static_cast<std::uint8_t>(payload[i]) ^ b[at + (i % 4)]);
    in.erase(0, head + static_cast<std::size_t>(n));
    return true;
}

}  // namespace pocket::app::ws
