//! The math library's checks (docs/spec/numeric.md 10): the golden sweep (`math.golden`), the
//! accuracy table (`math.accuracy`) and special values (`math.special`).

use pocket_sim::math::{self, sweep};

/// The committed sweep hashes. `POCKET_BLESS=1` rewrites the file instead (only with a deliberate
/// `libm` change, numeric.md 6.1).
#[test]
fn golden_sweep() {
    if std::env::var_os("POCKET_BLESS").is_some() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/math/golden.txt");
        let header = sweep::GOLDEN.lines().next().unwrap_or("#");
        std::fs::write(path, format!("{header}\n{}", sweep::report())).unwrap();
        return;
    }
    sweep::check_golden().unwrap();
    assert_eq!(
        sweep::GOLDEN
            .lines()
            .filter(|l| !l.starts_with('#'))
            .count(),
        sweep::SWEEPS.len()
    );
}

/// Every entry of the accuracy table (inputs and correctly rounded results from mpmath at 200
/// bits, `accuracy.py`) is within 2 ulp.
#[test]
fn accuracy_within_two_ulp() {
    let table = include_str!("golden/accuracy.txt");
    let mut worst: Vec<(String, f64)> = Vec::new();
    let mut count = 0;
    for line in table
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let f = parts[0];
        let bits = |s: &str| f64::from_bits(u64::from_str_radix(s, 16).unwrap());
        let x = bits(parts[1]);
        let (got, want) = match f {
            "atan2" | "pow" | "hypot" => {
                let y = bits(parts[2]);
                let got = match f {
                    "atan2" => math::atan2(x, y),
                    "pow" => math::pow(x, y),
                    _ => math::hypot(x, y),
                };
                (got, bits(parts[3]))
            }
            _ => (unary(f)(x), bits(parts[2])),
        };
        let err = ulp_error(got, want);
        assert!(
            err <= 2.0,
            "{f}({line}): {got:e} against {want:e}, {err} ulp"
        );
        match worst.iter_mut().find(|w| w.0 == f) {
            Some(w) => w.1 = math::max(w.1, err),
            None => worst.push((f.to_owned(), err)),
        }
        count += 1;
    }
    assert_eq!(count, 25 * 120, "the table holds {count} cases");
    eprintln!("worst errors (ulp): {worst:?}");
}

fn unary(name: &str) -> fn(f64) -> f64 {
    match name {
        "sin" => math::sin,
        "cos" => math::cos,
        "tan" => math::tan,
        "asin" => math::asin,
        "acos" => math::acos,
        "atan" => math::atan,
        "sinh" => math::sinh,
        "cosh" => math::cosh,
        "tanh" => math::tanh,
        "asinh" => math::asinh,
        "acosh" => math::acosh,
        "atanh" => math::atanh,
        "exp" => math::exp,
        "exp2" => math::exp2,
        "expm1" => math::expm1,
        "ln" => math::ln,
        "log2" => math::log2,
        "log10" => math::log10,
        "ln_1p" => math::ln_1p,
        "cbrt" => math::cbrt,
        "sqrt" => math::sqrt,
        other => panic!("no function {other}"),
    }
}

/// The error of `got` in units in the last place of the correctly rounded `want`.
fn ulp_error(got: f64, want: f64) -> f64 {
    if got == want {
        return 0.0;
    }
    let ulp = (f64::from_bits(want.abs().to_bits() + 1) - want.abs()).abs();
    ((got - want) / ulp).abs()
}

/// The values every function meets at its edges: ±0, ±infinity, NaN and ± the smallest subnormal.
const SPECIALS: [f64; 7] = [
    0.0,
    -0.0,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    TINY,
    -TINY,
];
/// The smallest subnormal, 2^-1074.
const TINY: f64 = f64::from_bits(1);
const INF: f64 = f64::INFINITY;
const NAN: f64 = f64::NAN;
/// The doubles next to 1: 1 + 2^-52 and 1 - 2^-53, the first inputs past a domain edge at 1.
const UP1: f64 = f64::from_bits(0x3FF0_0000_0000_0001);
const DOWN1: f64 = f64::from_bits(0x3FEF_FFFF_FFFF_FFFF);
/// 3 pi / 4 and pi / 4, correctly rounded.
const PI_3_4: f64 = f64::from_bits(0x4002_D97C_7F33_21D2);
const PI_1_4: f64 = f64::from_bits(0x3FE9_21FB_5444_2D18);
const PI_2: f64 = math::FRAC_PI_2;
const PI: f64 = math::PI;
/// ln(2^-1074) and log10(2^-1074) correctly rounded (60-digit decimal arithmetic, offline);
/// cbrt(2^-1074) = 2^-358 and sqrt(2^-1074) = 2^-537 exactly.
const LN_TINY: f64 = f64::from_bits(0xC087_4385_446D_71C3);
const LOG10_TINY: f64 = f64::from_bits(0xC074_34E6_420F_4374);
const CBRT_TINY: f64 = f64::from_bits(0x2990_0000_0000_0000);
const SQRT_TINY: f64 = f64::from_bits(0x1E60_0000_0000_0000);

/// The same bits, or both NaN (a NaN's sign and payload are not part of the contract).
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// One function's row: its results at [`SPECIALS`] in order, then its domain edges as
/// `(x, result)`.
type Row = (&'static str, [f64; 7], &'static [(f64, f64)]);

/// The rows of [`special_values`], one per unary function of numeric.md 6.2.
const ROWS: &[Row] = &[
    ("sin", [0.0, -0.0, NAN, NAN, NAN, TINY, -TINY], &[]),
    ("cos", [1.0, 1.0, NAN, NAN, NAN, 1.0, 1.0], &[]),
    ("tan", [0.0, -0.0, NAN, NAN, NAN, TINY, -TINY], &[]),
    (
        "asin",
        [0.0, -0.0, NAN, NAN, NAN, TINY, -TINY],
        &[(1.0, PI_2), (-1.0, -PI_2), (UP1, NAN), (-UP1, NAN)],
    ),
    (
        "acos",
        [PI_2, PI_2, NAN, NAN, NAN, PI_2, PI_2],
        &[(1.0, 0.0), (-1.0, PI), (UP1, NAN), (-UP1, NAN)],
    ),
    ("atan", [0.0, -0.0, PI_2, -PI_2, NAN, TINY, -TINY], &[]),
    ("sinh", [0.0, -0.0, INF, -INF, NAN, TINY, -TINY], &[]),
    ("cosh", [1.0, 1.0, INF, INF, NAN, 1.0, 1.0], &[]),
    ("tanh", [0.0, -0.0, 1.0, -1.0, NAN, TINY, -TINY], &[]),
    ("asinh", [0.0, -0.0, INF, -INF, NAN, TINY, -TINY], &[]),
    (
        "acosh",
        [NAN, NAN, INF, NAN, NAN, NAN, NAN],
        &[(1.0, 0.0), (DOWN1, NAN), (-1.0, NAN)],
    ),
    (
        "atanh",
        [0.0, -0.0, NAN, NAN, NAN, TINY, -TINY],
        &[(1.0, INF), (-1.0, -INF), (UP1, NAN), (-UP1, NAN)],
    ),
    (
        "exp",
        [1.0, 1.0, INF, 0.0, NAN, 1.0, 1.0],
        &[(800.0, INF), (-800.0, 0.0)],
    ),
    (
        "exp2",
        [1.0, 1.0, INF, 0.0, NAN, 1.0, 1.0],
        &[(1024.0, INF), (-1074.0, TINY), (-1076.0, 0.0)],
    ),
    (
        "expm1",
        [0.0, -0.0, INF, -1.0, NAN, TINY, -TINY],
        &[(-800.0, -1.0)],
    ),
    (
        "ln",
        [-INF, -INF, INF, NAN, NAN, LN_TINY, NAN],
        &[(1.0, 0.0), (-1.0, NAN)],
    ),
    (
        "log2",
        [-INF, -INF, INF, NAN, NAN, -1074.0, NAN],
        &[(1.0, 0.0), (-1.0, NAN)],
    ),
    (
        "log10",
        [-INF, -INF, INF, NAN, NAN, LOG10_TINY, NAN],
        &[(1.0, 0.0), (-1.0, NAN)],
    ),
    (
        "ln_1p",
        [0.0, -0.0, INF, NAN, NAN, TINY, -TINY],
        &[(-1.0, -INF), (-UP1, NAN)],
    ),
    (
        "cbrt",
        [0.0, -0.0, INF, -INF, NAN, CBRT_TINY, -CBRT_TINY],
        &[(-8.0, -2.0)],
    ),
    (
        "sqrt",
        [0.0, -0.0, INF, NAN, NAN, SQRT_TINY, NAN],
        &[(-1.0, NAN)],
    ),
];

/// C99 Annex F special values, as musl gives them (numeric.md 10, `math.special`): every unary
/// function of numeric.md 6.2 at ±0, ±infinity, NaN, ± the smallest subnormal and its domain edges,
/// compared by bits. Where Annex F leaves a value to the computation (ln of the subnormal), the
/// expected value is the correctly rounded one. The two-argument functions follow; `Math.pow` and
/// `Math.hypot`'s ECMAScript cases are the script host's (script-host.md 12).
#[test]
fn special_values() {
    let mut failures = Vec::new();
    let mut count = 0;
    for (name, at, edges) in ROWS {
        let f = unary(name);
        let cases = SPECIALS.iter().copied().zip(at.iter().copied());
        for (x, want) in cases.chain(edges.iter().copied()) {
            count += 1;
            let got = f(x);
            if !same(got, want) {
                failures.push(format!("{name}({x:e}) = {got:e}, want {want:e}"));
            }
        }
    }
    for x in SPECIALS {
        let (s, c) = math::sin_cos(x);
        assert!(
            same(s, math::sin(x)) && same(c, math::cos(x)),
            "sin_cos({x:e})"
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(ROWS.len(), 21, "every unary function of numeric.md 6.2");
    assert!(count > 170, "{count} cases");
}

/// Whether `y` is an odd integer.
fn odd(y: f64) -> bool {
    y.is_finite() && y.trunc() == y && (y % 2.0).abs() == 1.0
}

/// atan2(y, x) by C99 Annex F (F.9.1.4) on the table's values, where |y| = |x| when both are finite
/// and nonzero.
fn atan2_annex_f(y: f64, x: f64) -> f64 {
    let s = |v: f64| v.copysign(y);
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    if y == 0.0 {
        let right = x > 0.0 || (x == 0.0 && x.is_sign_positive());
        return s(if right { 0.0 } else { PI });
    }
    if y.is_infinite() {
        return s(if x == INF {
            PI_1_4
        } else if x == -INF {
            PI_3_4
        } else {
            PI_2
        });
    }
    if x == INF {
        s(0.0)
    } else if x == -INF {
        s(PI)
    } else if x == 0.0 {
        s(PI_2)
    } else {
        assert_eq!(x.abs(), y.abs(), "outside the table");
        s(if x > 0.0 { PI_1_4 } else { PI_3_4 })
    }
}

/// pow(x, y) by C99 Annex F (F.9.4.4); the other results the table needs are exact (integer
/// powers, the square roots of 2, 1/2 and 2^-1074) or round to 1 (y = ±2^-1074 for a positive x).
fn pow_annex_f(x: f64, y: f64) -> f64 {
    if y == 0.0 || x == 1.0 {
        return 1.0;
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    if x == 0.0 {
        return match (y < 0.0, odd(y)) {
            (true, true) => INF.copysign(x),
            (true, false) => INF,
            (false, true) => x,
            (false, false) => 0.0,
        };
    }
    if y.is_infinite() {
        if x == -1.0 {
            return 1.0;
        }
        return if (y > 0.0) == (x.abs() < 1.0) {
            0.0
        } else {
            INF
        };
    }
    if x.is_infinite() {
        let r = if y < 0.0 { 0.0 } else { INF };
        return if x < 0.0 && odd(y) { -r } else { r };
    }
    if y.trunc() == y {
        #[allow(clippy::cast_possible_truncation)] // the table's integers are small
        return math::powi(x, y as i32);
    }
    if x < 0.0 {
        return NAN;
    }
    if y == 0.5 {
        return math::sqrt(x);
    }
    assert_eq!(y.abs(), TINY, "outside the table");
    1.0
}

/// hypot(x, y) by C99 Annex F (F.9.4.3): an infinity wins over a NaN; with a zero, the other's
/// magnitude; two subnormals of one magnitude give it back (2^-1074 sqrt 2 rounds to 2^-1074).
fn hypot_annex_f(x: f64, y: f64) -> f64 {
    if x.is_infinite() || y.is_infinite() {
        return INF;
    }
    if x.is_nan() || y.is_nan() {
        return NAN;
    }
    if x == 0.0 {
        return y.abs();
    }
    if y == 0.0 {
        return x.abs();
    }
    assert_eq!(x.abs(), y.abs(), "outside the table");
    x.abs()
}

/// The two-argument functions on the cross product of the special values; for `pow` also ±1, ±2
/// and 1/2 as bases and ±1, ±2, ±3 and 1/2 as exponents, the values its special cases turn on.
#[test]
fn special_values_of_two_arguments() {
    let mut failures = Vec::new();
    let mut check = |name: &str, a: f64, b: f64, got: f64, want: f64| {
        if !same(got, want) {
            failures.push(format!("{name}({a:e}, {b:e}) = {got:e}, want {want:e}"));
        }
    };
    for a in SPECIALS {
        for b in SPECIALS {
            check("atan2", a, b, math::atan2(a, b), atan2_annex_f(a, b));
            check("hypot", a, b, math::hypot(a, b), hypot_annex_f(a, b));
        }
    }
    let extra = [1.0, -1.0, 2.0, -2.0, 0.5];
    let xs: Vec<f64> = SPECIALS.iter().chain(&extra).copied().collect();
    let ys: Vec<f64> = xs.iter().chain(&[3.0, -3.0]).copied().collect();
    for &x in &xs {
        for &y in &ys {
            check("pow", x, y, math::pow(x, y), pow_annex_f(x, y));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
