//! The golden sweeps of the math library (docs/spec/numeric.md 10, `math.golden`): every function
//! evaluated over 100,000 inputs drawn by PCG32 (built with integer bit construction and exact
//! arithmetic only), the result bits folded into an FNV-1a hash per sweep. The committed hashes
//! (`golden.txt`) must come out of the native debug and release builds and of the WebAssembly
//! build alike; a `libm` update that changes one result fails here first.
//!
//! The first 26 sweeps are numeric.md 6.2's functions over its sampled ranges; the rest pin the
//! allowed operations that reach a C library in some build (`%`, `floor`, `ceil`, `trunc`, `sqrt`
//! over every exponent, subnormals and signed zeros included), `min` and `max` and their `f32` forms
//! on signed zeros and NaNs, `canonical` over NaNs of every sign and payload, `powi` and
//! `wrap_angle`.

use super::*;
use crate::rng::{Pcg32, fnv1a64_extend};

/// Inputs per sweep.
pub const INPUTS: u32 = 100_000;

/// The committed hashes, one `name hash` line per sweep.
pub const GOLDEN: &str = include_str!("golden.txt");

/// A uniform double in [0, 1): 53 random bits times 2^-53, exact.
fn unit(r: &mut Pcg32) -> f64 {
    let hi = u64::from(r.next_u32());
    let lo = u64::from(r.next_u32());
    #[allow(clippy::cast_precision_loss)] // below 2^53: exact
    let m = (((hi << 32) | lo) >> 11) as f64;
    m * (1.0 / 9_007_199_254_740_992.0)
}

fn uniform(r: &mut Pcg32, lo: f64, hi: f64) -> f64 {
    lo + (hi - lo) * unit(r)
}

/// A double with a random mantissa and an exponent in [lo, hi): log-uniform over [2^lo, 2^hi).
fn log_uniform(r: &mut Pcg32, lo: i32, hi: i32) -> f64 {
    let span = u32::try_from(hi - lo).unwrap_or(1);
    let e = lo + i32::try_from(r.next_u32() % span).unwrap_or(0);
    let mantissa = ((u64::from(r.next_u32()) << 32) | u64::from(r.next_u32())) & ((1 << 52) - 1);
    let biased = u64::try_from(e + 1023).unwrap_or(1023);
    f64::from_bits((biased << 52) | mantissa)
}

/// Any finite double: a random sign, exponent field 0 to 2046 (subnormals and zeros included) and
/// mantissa; one in eight is a signed zero, so they are well covered.
fn any_finite(r: &mut Pcg32) -> f64 {
    let sign = u64::from(r.next_u32() & 1) << 63;
    if r.next_u32().is_multiple_of(8) {
        return f64::from_bits(sign);
    }
    let exp = u64::from(r.next_u32() % 2047);
    let mantissa = ((u64::from(r.next_u32()) << 32) | u64::from(r.next_u32())) & ((1 << 52) - 1);
    f64::from_bits(sign | (exp << 52) | mantissa)
}

/// A NaN of random sign and payload.
fn any_nan(r: &mut Pcg32) -> f64 {
    let sign = u64::from(r.next_u32() & 1) << 63;
    let payload =
        (((u64::from(r.next_u32()) << 32) | u64::from(r.next_u32())) & ((1 << 52) - 1)) | 1;
    f64::from_bits(sign | (0x7FF << 52) | payload)
}

/// A value for `min` and `max`: often a signed zero or a NaN.
fn edgy(r: &mut Pcg32) -> f64 {
    match r.next_u32() % 6 {
        0 => 0.0,
        1 => -0.0,
        2 => any_nan(r),
        _ => uniform(r, -10.0, 10.0),
    }
}

/// [`edgy`] in `f32`, NaNs of every sign and payload built from their bits: a conversion from `f64`
/// would be an arithmetic operation, whose NaN bits differ between targets (numeric.md 2).
fn edgy_f32(r: &mut Pcg32) -> f32 {
    match r.next_u32() % 6 {
        0 => 0.0,
        1 => -0.0,
        2 => {
            let sign = (r.next_u32() & 1) << 31;
            f32::from_bits(sign | (0xFF << 23) | (r.next_u32() & ((1 << 23) - 1)) | 1)
        }
        _ => {
            // 24 random bits are exact in f32: [0, 1) times 20, minus 10, rounded once each.
            #[allow(clippy::cast_precision_loss)]
            let u = (r.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0);
            u * 20.0 - 10.0
        }
    }
}

/// An `f32` result as a sweep's value: its bits in the low half, no conversion (which would change
/// a NaN's bits on some targets).
fn bits_f32(x: f32) -> f64 {
    f64::from_bits(u64::from(x.to_bits()))
}

/// One sweep: its name, and the function that evaluates input `i` with the sweep's generator.
type Sweep = (&'static str, fn(&mut Pcg32) -> f64);

const R2PI: f64 = 2.0 * PI;

/// Every sweep, in the order of the golden file.
pub const SWEEPS: &[Sweep] = &[
    ("sin", |r| sin(uniform(r, -R2PI, R2PI))),
    ("cos", |r| cos(uniform(r, -R2PI, R2PI))),
    ("sin_1e5", |r| sin(uniform(r, -1e5, 1e5))),
    ("sin_1e9", |r| sin(uniform(r, -1e9, 1e9))),
    ("tan", |r| tan(uniform(r, -1.5, 1.5))),
    ("asin", |r| asin(uniform(r, -1.0, 1.0))),
    ("acos", |r| acos(uniform(r, -1.0, 1.0))),
    ("atan", |r| atan(uniform(r, -1e3, 1e3))),
    ("atan2", |r| {
        let y = uniform(r, -100.0, 100.0);
        atan2(y, uniform(r, -100.0, 100.0))
    }),
    ("sinh", |r| sinh(uniform(r, -10.0, 10.0))),
    ("cosh", |r| cosh(uniform(r, -10.0, 10.0))),
    ("tanh", |r| tanh(uniform(r, -10.0, 10.0))),
    ("asinh", |r| asinh(uniform(r, -1e3, 1e3))),
    ("acosh", |r| acosh(uniform(r, 1.0, 1e3))),
    ("atanh", |r| atanh(uniform(r, -0.999, 0.999))),
    ("exp", |r| exp(uniform(r, -50.0, 50.0))),
    ("exp2", |r| exp2(uniform(r, -50.0, 50.0))),
    ("expm1", |r| expm1(uniform(r, -5.0, 5.0))),
    ("ln", |r| ln(log_uniform(r, -34, 34))),
    ("log2", |r| log2(log_uniform(r, -34, 34))),
    ("log10", |r| log10(log_uniform(r, -34, 34))),
    ("ln_1p", |r| ln_1p(uniform(r, -0.9, 10.0))),
    ("pow", |r| {
        let x = log_uniform(r, -7, 7);
        pow(x, uniform(r, -10.0, 10.0))
    }),
    ("cbrt", |r| cbrt(uniform(r, -1e6, 1e6))),
    ("hypot", |r| {
        let x = uniform(r, -1e3, 1e3);
        hypot(x, uniform(r, -1e3, 1e3))
    }),
    ("sqrt", |r| sqrt(log_uniform(r, -20, 20))),
    ("rem", |r| {
        let x = any_finite(r);
        canonical(x % any_finite(r))
    }),
    ("floor", |r| any_finite(r).floor()),
    ("ceil", |r| any_finite(r).ceil()),
    ("trunc", |r| any_finite(r).trunc()),
    ("sqrt_any", |r| sqrt(any_finite(r).abs())),
    ("min", |r| {
        let a = edgy(r);
        min(a, edgy(r))
    }),
    ("max", |r| {
        let a = edgy(r);
        max(a, edgy(r))
    }),
    ("min_f32", |r| {
        let a = edgy_f32(r);
        bits_f32(min_f32(a, edgy_f32(r)))
    }),
    ("max_f32", |r| {
        let a = edgy_f32(r);
        bits_f32(max_f32(a, edgy_f32(r)))
    }),
    ("canonical", |r| canonical(any_nan(r))),
    ("powi", |r| {
        let x = uniform(r, -4.0, 4.0);
        let n = i32::try_from(r.next_u32() % 41).unwrap_or(0) - 20;
        powi(x, n)
    }),
    ("wrap_angle", |r| wrap_angle(uniform(r, -1e4, 1e4))),
];

/// The hash of one sweep: the generator seeded from the sweep's name, `INPUTS` evaluations, every
/// result's bits (little-endian) folded into FNV-1a 64.
pub fn sweep_hash(name: &str, f: fn(&mut Pcg32) -> f64) -> u64 {
    let mut r = Pcg32::new(fnv1a64_extend(0xcbf2_9ce4_8422_2325, name.as_bytes()), 54);
    let mut h = 0xcbf2_9ce4_8422_2325;
    for _ in 0..INPUTS {
        h = fnv1a64_extend(h, &f(&mut r).to_bits().to_le_bytes());
    }
    h
}

/// Every sweep's `name hash` line, as the golden file holds them.
pub fn report() -> String {
    let mut out = String::new();
    for (name, f) in SWEEPS {
        out.push_str(&format!("{name} {:016x}\n", sweep_hash(name, *f)));
    }
    out
}

/// Compares [`report`] with the committed hashes: the web test `math.golden`.
pub fn check_golden() -> Result<(), String> {
    let got = report();
    let want: String = GOLDEN
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| format!("{l}\n"))
        .collect();
    if got == want {
        return Ok(());
    }
    let diff: Vec<String> = got
        .lines()
        .zip(want.lines().chain(std::iter::repeat("")))
        .filter(|(g, w)| g != w)
        .map(|(g, w)| format!("got {g}, golden {w}"))
        .collect();
    Err(format!(
        "math sweeps differ from golden.txt: {}",
        diff.join("; ")
    ))
}
