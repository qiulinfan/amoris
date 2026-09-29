#include <pocket/core/repro.hpp>

#include <cmath>
#include <limits>

// Every step below is an IEEE operation on doubles (or floor, fmod, frexp, ldexp, copysign, which
// are exact); the build has no fused multiply-adds (pocket.toml), so each gives the same bits
// everywhere. Series are Horner forms, their coefficients folded by the compiler to the nearest double.

namespace pocket::repro {

namespace {

constexpr double kPi = 3.14159265358979311600e+00;
constexpr double kHalfPi = 1.57079632679489655800e+00;
// pi/2 in two parts: the first has 33 significant bits, so k times it is exact for |k| < 2^20.
constexpr double kHalfPiHi = 1.57079632673412561417e+00;
constexpr double kHalfPiLo = 6.07710050650619224932e-11;
// ln 2 the same way.
constexpr double kLn2 = 6.93147180559945286227e-01;
constexpr double kLn2Hi = 6.93147180369123816490e-01;
constexpr double kLn2Lo = 1.90821492927058770002e-10;
constexpr double kNaN = std::numeric_limits<double>::quiet_NaN();
constexpr double kInf = std::numeric_limits<double>::infinity();

// sin and cos of r in [-pi/4, pi/4]: Taylor series to r^13 and r^16 (errors under 1e-14).
double sin_poly(double r) {
    const double r2 = r * r;
    return r + r * r2 * (-1.0 / 6 + r2 * (1.0 / 120 + r2 * (-1.0 / 5040 + r2 * (1.0 / 362880 + r2 * (-1.0 / 39916800 + r2 * (1.0 / 6227020800.0))))));
}

double cos_poly(double r) {
    const double r2 = r * r;
    return 1 + r2 * (-1.0 / 2 + r2 * (1.0 / 24 + r2 * (-1.0 / 720 + r2 * (1.0 / 40320 + r2 * (-1.0 / 3628800 + r2 * (1.0 / 479001600.0 + r2 * (-1.0 / 87178291200.0)))))));
}

// sin and cos of x: x less the nearest multiple k of pi/2, then the quarter turn k selects.
void sincos_d(double x, double& s, double& c) {
    if (!std::isfinite(x)) {
        s = c = kNaN;
        return;
    }
    const double k = std::floor(x * (2 / kPi) + 0.5);
    const double r = (x - k * kHalfPiHi) - k * kHalfPiLo;
    int quarter = static_cast<int>(std::fmod(k, 4.0));
    if (quarter < 0) quarter += 4;
    const double sr = sin_poly(r), cr = cos_poly(r);
    switch (quarter) {
        case 0: s = sr; c = cr; break;
        case 1: s = cr; c = -sr; break;
        case 2: s = -sr; c = -cr; break;
        default: s = -cr; c = sr; break;
    }
}

// atan of x >= 0: past 1 it is pi/2 less atan(1/x); halving the angle twice (atan x = 2 atan(x /
// (1 + sqrt(1 + x^2)))) brings the argument under tan(pi/16), where the series to t^17 is enough.
double atan_pos(double x) {
    bool flip = false;
    if (x > 1) {
        x = 1 / x;
        flip = true;
    }
    double t = x / (1 + std::sqrt(1 + x * x));
    t = t / (1 + std::sqrt(1 + t * t));
    const double t2 = t * t;
    const double a = 4 * t * (1 - t2 * (1.0 / 3 - t2 * (1.0 / 5 - t2 * (1.0 / 7 - t2 * (1.0 / 9 - t2 * (1.0 / 11 - t2 * (1.0 / 13 - t2 * (1.0 / 15 - t2 * (1.0 / 17)))))))));
    return flip ? kHalfPi - a : a;
}

double atan2_d(double y, double x) {
    if (std::isnan(x) || std::isnan(y)) return kNaN;
    if (y == 0) return (x < 0 || (x == 0 && std::signbit(x))) ? std::copysign(kPi, y) : std::copysign(0.0, y);
    if (x == 0) return std::copysign(kHalfPi, y);
    if (std::isinf(x) && std::isinf(y)) return std::copysign(x > 0 ? kPi / 4 : 3 * kPi / 4, y);
    const double a = atan_pos(std::fabs(y / x));
    if (x > 0) return std::copysign(a, y);
    return std::copysign(kPi - a, y);
}

// e^x: x less the nearest multiple k of ln 2, the series to r^13 for the rest (|r| <= 0.35), times 2^k.
double exp_d(double x) {
    if (std::isnan(x)) return kNaN;
    if (x > 709) return kInf;
    if (x < -745) return 0;
    const double k = std::floor(x / kLn2 + 0.5);
    const double r = (x - k * kLn2Hi) - k * kLn2Lo;
    const double p = 1 + r * (1 + r * (1.0 / 2 + r * (1.0 / 6 + r * (1.0 / 24 + r * (1.0 / 120 + r * (1.0 / 720 + r * (1.0 / 5040 + r * (1.0 / 40320 + r * (1.0 / 362880 + r * (1.0 / 3628800 + r * (1.0 / 39916800 + r * (1.0 / 479001600.0 + r * (1.0 / 6227020800.0)))))))))))));
    return std::ldexp(p, static_cast<int>(k));
}

// ln x: x = m 2^e with m in [sqrt(1/2), sqrt(2)), ln m = 2 atanh((m - 1) / (m + 1)) by its series.
double log_d(double x) {
    if (std::isnan(x) || x < 0) return kNaN;
    if (x == 0) return -kInf;
    if (std::isinf(x)) return kInf;
    int e = 0;
    double m = std::frexp(x, &e);
    if (m < 0.70710678118654752440) {
        m *= 2;
        e -= 1;
    }
    const double f = (m - 1) / (m + 1), f2 = f * f;
    const double s = 2 * f * (1 + f2 * (1.0 / 3 + f2 * (1.0 / 5 + f2 * (1.0 / 7 + f2 * (1.0 / 9 + f2 * (1.0 / 11 + f2 * (1.0 / 13 + f2 * (1.0 / 15 + f2 * (1.0 / 17 + f2 * (1.0 / 19 + f2 * (1.0 / 21)))))))))));
    return e * kLn2Hi + (e * kLn2Lo + s);
}

}  // namespace

float sin(float x) {
    double s, c;
    sincos_d(x, s, c);
    return static_cast<float>(s);
}

float cos(float x) {
    double s, c;
    sincos_d(x, s, c);
    return static_cast<float>(c);
}

void sincos(float x, float& s, float& c) {
    double sd, cd;
    sincos_d(x, sd, cd);
    s = static_cast<float>(sd);
    c = static_cast<float>(cd);
}

float tan(float x) {
    double s, c;
    sincos_d(x, s, c);
    return static_cast<float>(s / c);
}

float atan(float x) {
    if (std::isnan(x)) return x;
    return static_cast<float>(std::copysign(atan_pos(std::fabs(static_cast<double>(x))), static_cast<double>(x)));
}

float atan2(float y, float x) { return static_cast<float>(atan2_d(y, x)); }

float asin(float x) {
    const double d = x;
    if (!(std::fabs(d) <= 1)) return std::numeric_limits<float>::quiet_NaN();
    return static_cast<float>(atan2_d(d, std::sqrt((1 - d) * (1 + d))));
}

float acos(float x) {
    const double d = x;
    if (!(std::fabs(d) <= 1)) return std::numeric_limits<float>::quiet_NaN();
    return static_cast<float>(atan2_d(std::sqrt((1 - d) * (1 + d)), d));
}

float exp(float x) { return static_cast<float>(exp_d(x)); }

float log(float x) { return static_cast<float>(log_d(x)); }

float pow(float x, float y) {
    const double b = x, p = y;
    if (p == 0 || b == 1) return 1;
    if (std::isnan(b) || std::isnan(p)) return std::numeric_limits<float>::quiet_NaN();
    const bool integer = std::isfinite(p) && std::floor(p) == p;
    const bool odd = integer && std::fmod(p, 2.0) != 0;
    if (b == 0) {
        const double r = p > 0 ? 0.0 : kInf;
        return static_cast<float>(odd && std::signbit(b) ? -r : r);
    }
    if (b < 0) {
        if (!integer) return std::numeric_limits<float>::quiet_NaN();
        const double r = exp_d(p * log_d(-b));
        return static_cast<float>(odd ? -r : r);
    }
    return static_cast<float>(exp_d(p * log_d(b)));
}

float cbrt(float x) {
    const double a = std::fabs(static_cast<double>(x));
    if (a == 0 || !std::isfinite(a)) return x;
    double y = exp_d(log_d(a) / 3);
    y -= (y * y * y - a) / (3 * y * y);
    return static_cast<float>(std::copysign(y, static_cast<double>(x)));
}

float hypot(float x, float y) {
    if (std::isinf(x) || std::isinf(y)) return std::numeric_limits<float>::infinity();
    const double a = x, b = y;
    return static_cast<float>(std::sqrt(a * a + b * b));
}

}  // namespace pocket::repro
