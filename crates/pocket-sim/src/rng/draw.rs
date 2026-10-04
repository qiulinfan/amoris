//! The draw operations of a stream (docs/spec/rng.md 6). Each is written with exactly rounded
//! operations and the deterministic math library, so its results are the same on every target.

use pocket_contract::{Detail, Problem};
use serde_json::{Value, json};

use super::{MAX_SAFE, Pcg32};
use crate::math;

/// A stream as a system holds it during one tick: the table's generator for its key.
pub type Stream<'a> = &'a mut Pcg32;

fn bound_invalid(op: &str, args: &[(&str, Value)], why: &str) -> Problem {
    let mut d = Detail::new();
    d.insert("op".into(), json!(op));
    for (k, v) in args {
        d.insert((*k).into(), v.clone());
    }
    Problem::new("rng.bound_invalid", format!("{op}: {why}."), d)
}

/// A float for a detail: JSON has no NaN or infinity, so those are written as text.
fn f(x: f64) -> Value {
    if x.is_finite() {
        json!(x)
    } else {
        json!(x.to_string())
    }
}

impl Pcg32 {
    /// `(hi << 32) | lo`, `hi` drawn first.
    pub fn next_u64(&mut self) -> u64 {
        let hi = u64::from(self.next_u32());
        let lo = u64::from(self.next_u32());
        (hi << 32) | lo
    }

    /// A multiple of 2^-53 in [0, 1), each equally likely: `(next_u64() >> 11) * 2^-53`.
    pub fn next_f64(&mut self) -> f64 {
        #[allow(clippy::cast_precision_loss)] // below 2^53: exact
        let m = (self.next_u64() >> 11) as f64;
        m * (1.0 / 9_007_199_254_740_992.0)
    }

    /// An integer in [0, n) without bias: the reference `pcg32_boundedrand_r`
    /// (`threshold = n.wrapping_neg() % n`; draw until `r >= threshold`; `r % n`). `n == 0` is
    /// refused.
    pub fn below(&mut self, n: u32) -> Result<u32, Problem> {
        if n == 0 {
            return Err(bound_invalid(
                "below",
                &[("n", json!(0))],
                "the bound must be at least 1",
            ));
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return Ok(r % n);
            }
        }
    }

    /// [`Pcg32::below`] over `next_u64`.
    pub fn below_u64(&mut self, n: u64) -> Result<u64, Problem> {
        if n == 0 {
            return Err(bound_invalid(
                "below_u64",
                &[("n", json!(0))],
                "the bound must be at least 1",
            ));
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u64();
            if r >= threshold {
                return Ok(r % n);
            }
        }
    }

    /// An integer in [lo, hi], both ends inclusive: `lo + below(hi - lo + 1)`, through `below_u64`
    /// when the span exceeds 2^32 - 1. Both ends within ±(2^53 - 1) and `lo <= hi`.
    pub fn int(&mut self, lo: i64, hi: i64) -> Result<i64, Problem> {
        if lo > hi || lo.unsigned_abs() > MAX_SAFE || hi.unsigned_abs() > MAX_SAFE {
            return Err(bound_invalid(
                "int",
                &[("lo", json!(lo)), ("hi", json!(hi))],
                "the ends must be integers within ±(2^53 - 1) with lo <= hi",
            ));
        }
        #[allow(clippy::cast_sign_loss)] // hi >= lo, so the span is positive
        let span = (hi - lo) as u64 + 1;
        let offset = match u32::try_from(span) {
            Ok(n) => u64::from(self.below(n)?),
            _ => self.below_u64(span)?,
        };
        #[allow(clippy::cast_possible_wrap)] // offset < span <= 2^54
        Ok(lo + offset as i64)
    }

    /// `lo + (hi - lo) * next_f64()`: in [lo, hi], equal to `hi` only through rounding. Finite ends
    /// with `lo <= hi`.
    pub fn range(&mut self, lo: f64, hi: f64) -> Result<f64, Problem> {
        if !(lo.is_finite() && hi.is_finite() && lo <= hi) {
            return Err(bound_invalid(
                "range",
                &[("lo", f(lo)), ("hi", f(hi))],
                "the ends must be finite with lo <= hi",
            ));
        }
        Ok(lo + (hi - lo) * self.next_f64())
    }

    /// `next_f64() < p`: never at most 0, always at least 1. Draws twice whatever `p` is.
    pub fn chance(&mut self, p: f64) -> Result<bool, Problem> {
        if !p.is_finite() {
            return Err(bound_invalid(
                "chance",
                &[("p", f(p))],
                "the probability must be finite",
            ));
        }
        Ok(self.next_f64() < p)
    }

    /// An index into a list of `len` items; `len == 0` is refused.
    pub fn pick(&mut self, len: usize) -> Result<usize, Problem> {
        if len == 0 {
            return Err(bound_invalid(
                "pick",
                &[("len", json!(0))],
                "there is nothing to pick from",
            ));
        }
        match u32::try_from(len) {
            Ok(n) => Ok(self.below(n)? as usize),
            Err(_) => {
                let i = self.below_u64(len as u64)?;
                Ok(usize::try_from(i).unwrap_or(0))
            }
        }
    }

    /// Durstenfeld's Fisher-Yates: for `i` from `len - 1` down to 1, `j = below(i + 1)`, swap.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = match u32::try_from(i + 1) {
                Ok(n) => self.below(n).map_or(0, |j| j as usize),
                Err(_) => self
                    .below_u64((i + 1) as u64)
                    .map_or(0, |j| usize::try_from(j).unwrap_or(0)),
            };
            items.swap(i, j);
        }
    }

    /// `out[i] = next_f64()` for `i` from 0 up.
    pub fn fill(&mut self, out: &mut [f64]) {
        for x in out {
            *x = self.next_f64();
        }
    }

    /// An index chosen with probability proportional to its weight. Weights finite and at least 0
    /// with a positive total, summed in index order; `r = next_f64() * total`; the first index
    /// whose running sum exceeds `r`, or the last positive weight if rounding passes them all.
    pub fn weighted(&mut self, weights: &[f64]) -> Result<usize, Problem> {
        let mut total = 0.0;
        let mut last_positive = None;
        for (i, &w) in weights.iter().enumerate() {
            if !(w.is_finite() && w >= 0.0) {
                return Err(bound_invalid(
                    "weighted",
                    &[("index", json!(i)), ("weight", f(w))],
                    "every weight must be finite and at least 0",
                ));
            }
            total += w;
            if w > 0.0 {
                last_positive = Some(i);
            }
        }
        let (Some(last), true) = (last_positive, total > 0.0 && total.is_finite()) else {
            return Err(bound_invalid(
                "weighted",
                &[("total", f(total))],
                "the weights must have a positive finite total",
            ));
        };
        let r = self.next_f64() * total;
        let mut sum = 0.0;
        for (i, &w) in weights.iter().enumerate() {
            sum += w;
            if sum > r {
                return Ok(i);
            }
        }
        Ok(last)
    }

    /// A normal deviate by Box-Muller with the math library: `u1 = 1 - next_f64()` (in (0, 1]),
    /// `u2 = next_f64()`, `mean + sd * sqrt(-2 ln u1) * cos(2 pi u2)`; the sine half is discarded,
    /// so no state is kept.
    pub fn normal(&mut self, mean: f64, sd: f64) -> f64 {
        let u1 = 1.0 - self.next_f64();
        let u2 = self.next_f64();
        mean + sd * math::sqrt(-2.0 * math::ln(u1)) * math::cos(math::TAU * u2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals() {
        let mut r = Pcg32::new(1, 2);
        assert_eq!(r.below(0).unwrap_err().code, "rng.bound_invalid");
        assert!(r.pick(0).is_err());
        assert!(r.int(3, 2).is_err());
        assert!(r.int(0, 1 << 53).is_err());
        assert!(r.range(1.0, 0.0).is_err());
        assert!(r.range(0.0, f64::INFINITY).is_err());
        assert!(r.chance(f64::NAN).is_err());
        assert!(r.weighted(&[0.0, 0.0]).is_err());
        assert!(r.weighted(&[1.0, -1.0]).is_err());
        assert!(r.weighted(&[]).is_err());
    }

    #[test]
    fn ranges_and_counts() {
        let mut r = Pcg32::new(7, 9);
        for _ in 0..10_000 {
            let i = r.int(-3, 3).unwrap();
            assert!((-3..=3).contains(&i));
            let x = r.range(-2.0, 5.0).unwrap();
            assert!((-2.0..=5.0).contains(&x));
            let u = r.next_f64();
            assert!((0.0..1.0).contains(&u));
        }
        // The whole safe range goes through below_u64.
        let lo = -((1i64 << 53) - 1);
        let hi = (1i64 << 53) - 1;
        let i = r.int(lo, hi).unwrap();
        assert!(i >= lo && i <= hi);
        assert_eq!(r.int(5, 5).unwrap(), 5);
        // chance draws twice whatever p is.
        let (mut a, mut b) = (Pcg32::new(1, 1), Pcg32::new(1, 1));
        assert!(!a.chance(0.0).unwrap());
        b.next_f64();
        assert_eq!(a.next_u32(), b.next_u32());
        // weighted skips zero weights.
        let mut w = Pcg32::new(3, 3);
        for _ in 0..1000 {
            let i = w.weighted(&[0.0, 2.0, 0.0, 1.0]).unwrap();
            assert!(i == 1 || i == 3);
        }
        let mut items = [1, 2, 3, 4, 5];
        w.shuffle(&mut items);
        let mut sorted = items;
        sorted.sort_unstable();
        assert_eq!(sorted, [1, 2, 3, 4, 5]);
        let mut buf = [0.0; 4];
        w.fill(&mut buf);
        assert!(buf.iter().all(|x| (0.0..1.0).contains(x)));
        let n = w.normal(10.0, 0.0);
        assert_eq!(n, 10.0);
    }
}
