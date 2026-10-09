//! The deterministic math library (docs/spec/numeric.md 6): the same bits on every target, in debug
//! and release, natively and in WebAssembly. The transcendental functions wrap the `libm` crate
//! (pinned `=0.2.16`, a Rust port of musl's libm built from operations IEEE 754 rounds exactly);
//! `min`, `max`, `clamp`, `lerp`, `powi`, `wrap_angle` and `canonical` are defined here by their
//! operation sequence. Simulation code calls these instead of `f64::sin` and its relatives, whose
//! precision Rust leaves to the platform (numeric.md 2 and 5; the clippy lists enforce it).
//!
//! Special values (NaN, infinities, signed zeros, domain edges) follow C99 Annex F as musl does.
//! Updating `libm` changes simulation results: the golden sweep (`sweep`) fails first, and the
//! update is an engine version change (numeric.md 6.1).

pub mod sweep;

/// The nearest double to pi.
pub const PI: f64 = core::f64::consts::PI;
/// The nearest double to 2 pi (exactly `2.0 * PI`).
pub const TAU: f64 = core::f64::consts::TAU;
/// The nearest double to pi / 2.
pub const FRAC_PI_2: f64 = core::f64::consts::FRAC_PI_2;
/// The nearest double to e.
pub const E: f64 = core::f64::consts::E;
/// The nearest double to ln 2.
pub const LN_2: f64 = core::f64::consts::LN_2;
/// The nearest double to ln 10.
pub const LN_10: f64 = core::f64::consts::LN_10;
/// The nearest double to the square root of 2.
pub const SQRT_2: f64 = core::f64::consts::SQRT_2;

macro_rules! unary {
    ($($(#[$doc:meta])* $name:ident => $libm:ident;)*) => {
        $(
            $(#[$doc])*
            #[inline]
            pub fn $name(x: f64) -> f64 {
                libm::$libm(x)
            }
        )*
    };
}

unary! {
    /// Sine (max error 0.68 ulp on [-2 pi, 2 pi]; correct argument reduction for large arguments).
    sin => sin;
    /// Cosine.
    cos => cos;
    /// Tangent.
    tan => tan;
    /// Arcsine; NaN outside [-1, 1].
    asin => asin;
    /// Arccosine; NaN outside [-1, 1].
    acos => acos;
    /// Arctangent.
    atan => atan;
    /// Hyperbolic sine.
    sinh => sinh;
    /// Hyperbolic cosine.
    cosh => cosh;
    /// Hyperbolic tangent.
    tanh => tanh;
    /// Inverse hyperbolic sine.
    asinh => asinh;
    /// Inverse hyperbolic cosine; NaN below 1.
    acosh => acosh;
    /// Inverse hyperbolic tangent.
    atanh => atanh;
    /// e^x; overflows to infinity, underflows to 0.
    exp => exp;
    /// 2^x.
    exp2 => exp2;
    /// e^x - 1, accurate near 0.
    expm1 => expm1;
    /// Natural logarithm; NaN below 0, -infinity at 0.
    ln => log;
    /// Base-2 logarithm.
    log2 => log2;
    /// Base-10 logarithm.
    log10 => log10;
    /// ln(1 + x), accurate near 0.
    ln_1p => log1p;
    /// Cube root.
    cbrt => cbrt;
}

/// Sine and cosine of one argument, as `sin` and `cos` give them.
#[inline]
pub fn sin_cos(x: f64) -> (f64, f64) {
    (libm::sin(x), libm::cos(x))
}

/// The angle of the point (x, y) from the x axis, in (-pi, pi].
#[inline]
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

/// x^y (C's `pow`; `Math.pow`'s ECMAScript special cases are the script host's, numeric.md 6.3).
#[inline]
pub fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

/// The length of (x, y) without undue overflow or underflow.
#[inline]
pub fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

/// The IEEE square root (correctly rounded; the hardware instruction).
#[inline]
pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// The smaller of `a` and `b` by comparison: on a tie (`+0` against `-0`) or a NaN, `a`.
/// `f64::min` may return either zero, and does so differently in debug and release (numeric.md 2).
#[inline]
pub fn min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// The larger of `a` and `b` by comparison: on a tie or a NaN, `a`.
#[inline]
pub fn max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

/// [`min`] for `f32` (Rapier's and the assets' scalars): on a tie or a NaN, `a`.
#[inline]
pub fn min_f32(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

/// [`max`] for `f32`: on a tie or a NaN, `a`.
#[inline]
pub fn max_f32(a: f32, b: f32) -> f32 {
    if b > a { b } else { a }
}

/// `x` limited to [lo, hi] by comparisons (lo <= hi): below `lo` gives `lo`, above `hi` gives `hi`.
#[inline]
pub fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// `a + (b - a) * t`, in that order.
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// x^n by squaring with a fixed multiplication order: r = 1, b = x, e = |n|; while e > 0 { if e is
/// odd { r *= b }; b *= b; e >>= 1 }; a negative n gives 1 / r. Exact whenever every intermediate
/// product is representable.
pub fn powi(x: f64, n: i32) -> f64 {
    let mut r = 1.0;
    let mut b = x;
    let mut e = n.unsigned_abs();
    while e > 0 {
        if e & 1 == 1 {
            r *= b;
        }
        b *= b;
        e >>= 1;
    }
    if n < 0 { 1.0 / r } else { r }
}

/// The angle `a - TAU * floor((a + PI) / TAU)`: in [-pi, pi) up to rounding.
#[inline]
pub fn wrap_angle(a: f64) -> f64 {
    a - TAU * ((a + PI) / TAU).floor()
}

/// The canonical NaN: what `canonical` maps every NaN to.
pub const CANONICAL_NAN: u64 = 0x7FF8_0000_0000_0000;

/// Every NaN to `0x7FF8_0000_0000_0000`, any other value unchanged: a NaN's sign and payload differ
/// between targets, so code that reads a float's bits reads them through this (numeric.md 5).
#[inline]
pub fn canonical(x: f64) -> f64 {
    if x.is_nan() {
        f64::from_bits(CANONICAL_NAN)
    } else {
        x
    }
}

/// [`canonical`] for `f32`: every NaN to `0x7FC0_0000`.
#[inline]
pub fn canonical_f32(x: f32) -> f32 {
    if x.is_nan() {
        f32::from_bits(0x7FC0_0000)
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defined_functions() {
        assert_eq!(min(0.0, -0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(min(-0.0, 0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(max(0.0, -0.0).to_bits(), 0.0f64.to_bits());
        assert_eq!(max(-0.0, 0.0).to_bits(), (-0.0f64).to_bits());
        assert!(min(f64::NAN, 1.0).is_nan());
        assert_eq!(min(1.0, f64::NAN), 1.0);
        assert_eq!(max(1.0, f64::NAN), 1.0);
        assert_eq!(min_f32(0.0, -0.0).to_bits(), 0.0f32.to_bits());
        assert_eq!(max_f32(-0.0, 0.0).to_bits(), (-0.0f32).to_bits());
        assert_eq!(min_f32(2.0, 1.0), 1.0);
        assert_eq!(max_f32(1.0, f32::NAN), 1.0);
        assert_eq!(clamp(5.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp(-5.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp(0.5, 0.0, 1.0), 0.5);
        assert_eq!(lerp(2.0, 4.0, 0.5), 3.0);
        assert_eq!(powi(2.0, 10), 1024.0);
        assert_eq!(powi(2.0, -2), 0.25);
        assert_eq!(powi(3.0, 0), 1.0);
        assert_eq!(powi(-2.0, 3), -8.0);
        assert_eq!(powi(2.0, i32::MIN), 0.0);
        assert_eq!(wrap_angle(PI), -PI);
        assert_eq!(wrap_angle(0.5), 0.5);
        assert!((wrap_angle(3.0 * PI) + PI).abs() < 1e-15);
        assert_eq!(
            canonical(f64::from_bits(0xFFF8_0000_0000_0001)).to_bits(),
            CANONICAL_NAN
        );
        assert_eq!(canonical(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(
            canonical_f32(f32::from_bits(0xFFC0_0001)).to_bits(),
            0x7FC0_0000
        );
        assert_eq!(TAU, 2.0 * PI);
    }

    #[test]
    fn identities_hold_only_to_rounding() {
        // PI is not pi: sin(PI) is about 1.22e-16 (numeric.md 6.2).
        assert!(sin(PI) > 1.2e-16 && sin(PI) < 1.3e-16);
        assert_eq!(sin_cos(0.7), (sin(0.7), cos(0.7)));
    }
}
