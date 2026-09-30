// The Unicode bidirectional algorithm (UAX #9) for one line: each character's type from the
// Unicode character database (bidi_classes.gen.inc), the weak and neutral rules resolved, the
// levels set and the level runs put in visual order. Explicit embeddings, overrides and isolates
// are not read: like the other format characters they are left out of the resolution (X9) and
// take the level of the character before them.
#include <pocket/ui/bidi.hpp>
#include <pocket/ui/font.hpp>

#include <algorithm>
#include <cstdint>
#include <iterator>

namespace pocket::ui {

namespace {

enum class T : std::uint8_t { L, R, AL, EN, ES, ET, AN, CS, NSM, BN, B, WS, ON };

struct Stretch {
    std::uint32_t first;
    T type;
};

constexpr Stretch kStretches[] = {
#include "bidi_classes.gen.inc"
};

T type_of(std::uint32_t c) {
    const auto* s = std::upper_bound(std::begin(kStretches), std::end(kStretches), c, [](std::uint32_t v, const Stretch& x) { return v < x.first; });
    return s == std::begin(kStretches) ? T::L : (s - 1)->type;
}

struct Char {
    std::size_t start, end;
    T type;
    int level = 0;
};

bool strong_rtl(T t) { return t == T::R || t == T::AL; }

}  // namespace

bool rtl_line(std::string_view text) {
    for (std::size_t i = 0; i < text.size();) {
        const T t = type_of(decode_utf8(text, i));
        if (t == T::L) return false;
        if (strong_rtl(t)) return true;
    }
    return false;
}

std::vector<BidiRun> bidi_runs(std::string_view text, TextDirection direction) {
    std::vector<BidiRun> runs;
    if (text.empty()) return runs;
    std::vector<Char> cs;
    bool any_rtl = false;
    for (std::size_t i = 0; i < text.size();) {
        const std::size_t at = i;
        const T t = type_of(decode_utf8(text, i));
        if (t == T::BN) {   // X9: ridden along by its neighbour
            if (!cs.empty()) cs.back().end = i;
            continue;
        }
        any_rtl = any_rtl || strong_rtl(t) || t == T::AN;
        cs.push_back({cs.empty() ? 0 : at, i, t});
    }
    const int base = rtl_line(text, direction) ? 1 : 0;
    if (cs.empty() || (!any_rtl && base == 0)) return {BidiRun{0, text.size(), 0}};
    const T sos = base ? T::R : T::L;
    const std::size_t n = cs.size();
    // W1: a mark takes the type of what it marks.
    for (std::size_t k = 0; k < n; ++k) if (cs[k].type == T::NSM) cs[k].type = k == 0 ? sos : (cs[k - 1].type == T::B ? T::ON : cs[k - 1].type);
    // W2: a European number after Arabic letters is an Arabic number. W3: Arabic letters are right to left.
    T last = sos;
    for (Char& c : cs) {
        if (c.type == T::L || c.type == T::R || c.type == T::AL) last = c.type;
        else if (c.type == T::EN && last == T::AL) c.type = T::AN;
    }
    for (Char& c : cs) if (c.type == T::AL) c.type = T::R;
    // W4: one separator between two numbers of a kind joins them.
    for (std::size_t k = 1; k + 1 < n; ++k) {
        const T a = cs[k - 1].type, b = cs[k + 1].type;
        if (cs[k].type == T::ES && a == T::EN && b == T::EN) cs[k].type = T::EN;
        else if (cs[k].type == T::CS && a == b && (a == T::EN || a == T::AN)) cs[k].type = a;
    }
    // W5: terminators (currency, percent) next to European numbers are numbers too.
    for (std::size_t k = 0; k < n; ++k) {
        if (cs[k].type != T::ET) continue;
        std::size_t e = k;
        while (e < n && cs[e].type == T::ET) ++e;
        const bool touch = (k > 0 && cs[k - 1].type == T::EN) || (e < n && cs[e].type == T::EN);
        if (touch) for (std::size_t j = k; j < e; ++j) cs[j].type = T::EN;
        k = e - 1;
    }
    // W6: the separators and terminators left are neutral. W7: European numbers after left-to-right letters are left to right.
    for (Char& c : cs) if (c.type == T::ES || c.type == T::ET || c.type == T::CS) c.type = T::ON;
    last = sos;
    for (Char& c : cs) {
        if (c.type == T::L || c.type == T::R) last = c.type;
        else if (c.type == T::EN && last == T::L) c.type = T::L;
    }
    // N1, N2: neutrals between two of one direction take it (numbers count as right to left);
    // otherwise the paragraph's.
    auto dir = [](T t) { return t == T::L ? 0 : (t == T::R || t == T::EN || t == T::AN) ? 1 : -1; };
    for (std::size_t k = 0; k < n;) {
        if (dir(cs[k].type) >= 0) { ++k; continue; }
        std::size_t e = k;
        while (e < n && dir(cs[e].type) < 0) ++e;
        const int before = k == 0 ? base : dir(cs[k - 1].type);
        const int after = e == n ? base : dir(cs[e].type);
        const T as = (before == after ? before : base) ? T::R : T::L;
        for (std::size_t j = k; j < e; ++j) cs[j].type = as;
        k = e;
    }
    // I1, I2: the levels.
    for (Char& c : cs) {
        if (base == 0) c.level = c.type == T::R ? 1 : (c.type == T::AN || c.type == T::EN) ? 2 : 0;
        else c.level = c.type == T::L || c.type == T::AN || c.type == T::EN ? 2 : 1;
    }
    // L1: spaces at the end of the line go back to the paragraph's level.
    for (std::size_t k = n; k > 0 && (cs[k - 1].type == T::L || cs[k - 1].type == T::R);) {
        const std::uint32_t c = static_cast<unsigned char>(text[cs[k - 1].start]);
        if (c != ' ' && c != '\t') break;
        cs[--k].level = base;
    }
    // Level runs in logical order, then L2: from the highest level down to the lowest odd one,
    // every stretch of runs at that level or above turned around.
    for (const Char& c : cs) {
        if (!runs.empty() && runs.back().level == c.level) runs.back().end = c.end;
        else runs.push_back({c.start, c.end, c.level});
    }
    int top = 0, low_odd = 99;
    for (const BidiRun& r : runs) {
        top = std::max(top, r.level);
        if (r.level & 1) low_odd = std::min(low_odd, r.level);
    }
    for (int lv = top; lv >= low_odd && lv > 0; --lv) {
        for (std::size_t k = 0; k < runs.size();) {
            if (runs[k].level < lv) { ++k; continue; }
            std::size_t e = k;
            while (e < runs.size() && runs[e].level >= lv) ++e;
            std::reverse(runs.begin() + static_cast<std::ptrdiff_t>(k), runs.begin() + static_cast<std::ptrdiff_t>(e));
            k = e;
        }
    }
    return runs;
}

}  // namespace pocket::ui
