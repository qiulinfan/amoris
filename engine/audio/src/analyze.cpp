// What a clip sounds like, measured: loudness and peak; the envelope from the loudness of 10 ms
// windows (attack to 90% of the loudest, decay from it to a tenth, the audible length, onsets);
// the pitch of 30 ms frames every 10 ms by the cumulative mean normalised difference of a
// 16 kHz copy (YIN's measure), joined into segments and named as notes; the spectral centroid of
// frames as brightness; and the share of the audible frames that have a pitch as tonality.
#include "analyze.hpp"

#include <algorithm>
#include <cmath>
#include <complex>
#include <format>
#include <string>

namespace pocket::audio {
namespace {

std::string note_name(double hz) {
    static const char* kNames[12] = {"C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"};
    const double midi = 69.0 + 12.0 * std::log2(hz / 440.0);
    const int m = static_cast<int>(std::lround(midi));
    const int cents = static_cast<int>(std::lround((midi - m) * 100.0));
    std::string n = std::format("{}{}", kNames[((m % 12) + 12) % 12], m / 12 - 1);
    if (std::abs(cents) >= 10) n += std::format(" {:+d}c", cents);
    return n;
}

void fft(std::vector<std::complex<double>>& a) {
    const std::size_t n = a.size();
    for (std::size_t i = 1, j = 0; i < n; ++i) {
        std::size_t bit = n >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) std::swap(a[i], a[j]);
    }
    for (std::size_t len = 2; len <= n; len <<= 1) {
        const double ang = -2.0 * 3.14159265358979323846 / static_cast<double>(len);
        const std::complex<double> wl(std::cos(ang), std::sin(ang));
        for (std::size_t i = 0; i < n; i += len) {
            std::complex<double> w(1);
            for (std::size_t k = 0; k < len / 2; ++k) {
                const auto u = a[i + k], v = a[i + k + len / 2] * w;
                a[i + k] = u + v;
                a[i + k + len / 2] = u - v;
                w *= wl;
            }
        }
    }
}

}  // namespace

Json analyze_samples(const std::vector<float>& stereo, int rate) {
    Json out = Json::object();
    const std::size_t frames = stereo.size() / 2;
    out["seconds"] = rate > 0 ? static_cast<double>(frames) / rate : 0.0;
    if (frames == 0 || rate <= 0) return out;
    std::vector<float> mono(frames);
    for (std::size_t i = 0; i < frames; ++i) mono[i] = 0.5f * (stereo[2 * i] + stereo[2 * i + 1]);
    double peak = 0, sum2 = 0;
    for (float v : mono) {
        peak = std::max(peak, static_cast<double>(std::fabs(v)));
        sum2 += static_cast<double>(v) * v;
    }
    const double rms = std::sqrt(sum2 / static_cast<double>(frames));
    out["peak"] = std::round(peak * 1000) / 1000;
    out["loudness_db"] = rms > 0 ? std::round(20.0 * std::log10(rms) * 10) / 10 : -120.0;
    // The envelope: 10 ms windows.
    const std::size_t w = std::max<std::size_t>(1, static_cast<std::size_t>(rate / 100));
    std::vector<double> env;
    for (std::size_t s = 0; s < frames; s += w) {
        double e = 0;
        const std::size_t end = std::min(frames, s + w);
        for (std::size_t i = s; i < end; ++i) e += static_cast<double>(mono[i]) * mono[i];
        env.push_back(std::sqrt(e / static_cast<double>(end - s)));
    }
    const auto top = static_cast<std::size_t>(std::max_element(env.begin(), env.end()) - env.begin());
    const double loudest = env[top];
    if (loudest <= 1e-6) {
        out["silent"] = true;
        return out;
    }
    std::size_t attack = 0;
    while (attack < env.size() && env[attack] < 0.9 * loudest) ++attack;
    std::size_t decay = top;
    while (decay < env.size() && env[decay] >= 0.1 * loudest) ++decay;
    std::size_t last = env.size();
    while (last > 0 && env[last - 1] < 0.01 * loudest) --last;
    out["attack_ms"] = static_cast<int>(attack * 10);
    out["decay_ms"] = static_cast<int>((decay - top) * 10);
    out["audible_seconds"] = std::round(static_cast<double>(last) * 0.01 * 1000) / 1000;
    // Onsets: the envelope rising past half of what it has been near, from under a quarter.
    int onsets = 0;
    bool low = true;
    for (std::size_t i = 0; i < env.size(); ++i) {
        double near = 0;
        for (std::size_t k = (i >= 30 ? i - 30 : 0); k < std::min(env.size(), i + 30); ++k) near = std::max(near, env[k]);
        if (low && env[i] > 0.5 * near && env[i] > 0.05 * loudest) { ++onsets; low = false; }
        else if (env[i] < 0.25 * near) low = true;
    }
    out["onsets"] = onsets;
    const double seconds = static_cast<double>(frames) / rate;
    out["onsets_per_second"] = std::round(onsets / std::max(seconds, 1e-3) * 10) / 10;
    // Pitch: a 16 kHz copy (box-averaged), 30 ms frames every 10 ms (60 Hz and up), the first 10 seconds.
    const int dr = std::max(1, rate / 16000);
    const double srate = static_cast<double>(rate) / dr;
    std::vector<float> s;
    for (std::size_t i = 0; i + dr <= std::min(frames, static_cast<std::size_t>(rate) * 10); i += static_cast<std::size_t>(dr)) {
        float a = 0;
        for (int k = 0; k < dr; ++k) a += mono[i + static_cast<std::size_t>(k)];
        s.push_back(a / static_cast<float>(dr));
    }
    const auto N = static_cast<std::size_t>(srate * 0.03), hop = static_cast<std::size_t>(srate * 0.01);
    const auto tmin = static_cast<std::size_t>(srate / 2000.0), tmax = static_cast<std::size_t>(srate / 60.0);
    struct Seg { double from, to, hz; int n; };
    std::vector<Seg> segs;
    int voiced = 0, audible = 0;
    std::vector<double> d(tmax + 1);
    for (std::size_t start = 0; start + N + tmax < s.size(); start += hop) {
        const double t = static_cast<double>(start) / srate;
        const auto ei = std::min(env.size() - 1, static_cast<std::size_t>(t * 100));
        if (env[ei] < 0.05 * loudest) continue;
        ++audible;
        // YIN's difference, normalised by its running mean; the bottom of the first dip under 0.15.
        double running = 0;
        for (std::size_t tau = 1; tau <= tmax; ++tau) {
            double acc = 0;
            for (std::size_t i = 0; i < N; ++i) {
                const double diff = static_cast<double>(s[start + i]) - s[start + i + tau];
                acc += diff * diff;
            }
            running += acc;
            d[tau] = running > 0 ? acc * static_cast<double>(tau) / running : 1.0;
        }
        // The deepest dip, then the first lag nearly as deep (so the period, not a multiple of it),
        // to the bottom of its own dip, between whole samples by a parabola through its neighbours.
        const std::size_t lo = std::max<std::size_t>(tmin, 2);
        double dmin = 1e9;
        for (std::size_t tau = lo; tau < tmax; ++tau) dmin = std::min(dmin, d[tau]);
        std::size_t best = 0;
        if (dmin < 0.35) {
            for (std::size_t tau = lo; tau < tmax; ++tau) {
                if (d[tau] <= dmin + 0.1) {
                    while (tau + 1 < tmax && d[tau + 1] < d[tau]) ++tau;
                    best = tau;
                    break;
                }
            }
        }
        double lag = static_cast<double>(best);
        if (best > 1 && best + 1 <= tmax) {
            const double a = d[best - 1], b = d[best], c = d[best + 1];
            const double den = a - 2 * b + c;
            if (den > 1e-12) lag += 0.5 * (a - c) / den;
        }
        if (best == 0) continue;
        ++voiced;
        const double hz = srate / lag;
        if (!segs.empty() && std::fabs(segs.back().hz - hz) / hz < 0.03 && t - segs.back().to < 0.03) {
            Seg& g = segs.back();
            g.hz = (g.hz * g.n + hz) / (g.n + 1);
            g.to = t + 0.01;
            ++g.n;
        } else {
            segs.push_back({t, t + 0.01, hz, 1});
        }
    }
    Json pitch = Json::array();
    for (const Seg& g : segs) {
        if (g.n < 3) continue;   // a frame or two alone is a guess (a frame across two notes finds their common period)
        if (pitch.size() >= 48) break;
        pitch.push_back(Json{{"from", std::round(g.from * 1000) / 1000}, {"to", std::round(g.to * 1000) / 1000}, {"hz", std::round(g.hz * 10) / 10}, {"note", note_name(g.hz)}});
    }
    out["pitch"] = pitch;
    const double tonal = audible > 0 ? static_cast<double>(voiced) / audible : 0.0;
    out["tonal"] = std::round(tonal * 100) / 100;
    // Brightness: the spectral centroid of 2048-sample frames, weighed by their energy.
    double num = 0, den = 0;
    const std::size_t F = 2048;
    for (std::size_t start = 0; start + F <= frames && start < static_cast<std::size_t>(rate) * 20; start += F) {
        std::vector<std::complex<double>> a(F);
        for (std::size_t i = 0; i < F; ++i) {
            const double hann = 0.5 - 0.5 * std::cos(2.0 * 3.14159265358979323846 * static_cast<double>(i) / (F - 1));
            a[i] = mono[start + i] * hann;
        }
        fft(a);
        for (std::size_t k = 1; k < F / 2; ++k) {
            const double m = std::abs(a[k]);
            num += m * static_cast<double>(k) * rate / F;
            den += m;
        }
    }
    const double centroid = den > 0 ? num / den : 0.0;
    out["brightness_hz"] = static_cast<int>(std::lround(centroid));
    // In words.
    std::vector<std::string> words;
    const double heard = static_cast<double>(last) * 0.01;
    words.push_back(heard < 0.15 ? "very short" : heard < 0.6 ? "short" : heard < 2.0 ? "medium" : "long");
    words.push_back(centroid < 700 ? "mellow" : centroid < 2500 ? "warm" : centroid < 9000 ? "bright" : "hissy");
    words.push_back(tonal > 0.6 ? "tonal" : tonal > 0.25 ? "partly tonal" : "noisy");
    if (pitch.size() >= 2) {
        const double a0 = pitch.front()["hz"].get<double>(), a1 = pitch.back()["hz"].get<double>();
        if (pitch.size() >= 4 && onsets >= 4) words.push_back("melodic");
        else if (a1 > a0 * 1.1) words.push_back("rising");
        else if (a1 < a0 / 1.1) words.push_back("falling");
    }
    if (attack * 10 <= 15) words.push_back("sharp attack");
    std::string text;
    for (const auto& x : words) text += (text.empty() ? "" : ", ") + x;
    out["character"] = text;
    return out;
}

}  // namespace pocket::audio
