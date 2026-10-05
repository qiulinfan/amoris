//! Values entering integer and float fields (docs/spec/numeric.md 3.2, 7 and 8). A double enters
//! an integer field only when it is finite, integral and in the type's range: nothing rounds,
//! truncates, saturates or wraps, so 2.5 computed for a `u32` is an error naming the field. `-0`
//! stores as 0. NaN and infinities never enter persisted state.

use pocket_contract::{Problem, detail};
use serde_json::json;

use crate::time::Tick;

/// Why a value was refused.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NumError {
    NotFinite,
    NotInteger,
    OutOfRange { min: f64, max: f64 },
}

const MAX_TICK: f64 = 9_007_199_254_740_991.0;

fn integral(x: f64, min: f64, max: f64) -> Result<f64, NumError> {
    if !x.is_finite() {
        return Err(NumError::NotFinite);
    }
    if x != x.trunc() {
        return Err(NumError::NotInteger);
    }
    if x < min || x > max {
        return Err(NumError::OutOfRange { min, max });
    }
    Ok(x)
}

/// Into an `i32` field.
pub fn to_i32(x: f64) -> Result<i32, NumError> {
    let v = integral(x, f64::from(i32::MIN), f64::from(i32::MAX))?;
    #[allow(clippy::cast_possible_truncation)] // integral and within range: exact
    Ok(v as i32)
}

/// Into a `u32` field (`-0` stores 0).
pub fn to_u32(x: f64) -> Result<u32, NumError> {
    let v = integral(x, 0.0, f64::from(u32::MAX))?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // integral, 0..=u32::MAX
    Ok(v as u32)
}

/// Into a `tick` field: 0 to 2^53 - 1.
pub fn to_tick(x: f64) -> Result<Tick, NumError> {
    let v = integral(x, 0.0, MAX_TICK)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // integral, 0..=2^53 - 1
    Ok(Tick(v as u64))
}

/// Into an `f64` field: finite values only.
pub fn finite(x: f64) -> Result<f64, NumError> {
    if x.is_finite() {
        Ok(x)
    } else {
        Err(NumError::NotFinite)
    }
}

/// A value as a detail field: JSON holds finite numbers only, so others are written as text
/// (`"NaN"`, `"Infinity"`).
fn value(x: f64) -> serde_json::Value {
    if x.is_finite() {
        pocket_contract::codes::num(x)
    } else if x.is_nan() {
        json!("NaN")
    } else if x > 0.0 {
        json!("Infinity")
    } else {
        json!("-Infinity")
    }
}

impl NumError {
    /// The problem for a write outside the script host (`number.*`, numeric.md 9), naming the
    /// field and the value.
    pub fn problem(self, field: &str, x: f64) -> Problem {
        match self {
            NumError::NotFinite => Problem::new(
                "number.not_finite",
                format!(
                    "'{field}' cannot hold {}; only finite numbers enter the world.",
                    value(x)
                ),
                detail([("field", json!(field)), ("value", value(x))]),
            ),
            NumError::NotInteger => {
                let hint = "Round it first: Math.round, Math.floor or Math.trunc.";
                Problem::new(
                    "number.not_integer",
                    format!("'{field}' takes a whole number; got {}. {hint}", value(x)),
                    detail([
                        ("field", json!(field)),
                        ("value", value(x)),
                        ("hint", json!(hint)),
                    ]),
                )
            }
            NumError::OutOfRange { min, max } => Problem::new(
                "number.out_of_range",
                format!(
                    "'{field}' must be from {} to {}; got {}.",
                    value(min),
                    value(max),
                    value(x)
                ),
                detail([
                    ("field", json!(field)),
                    ("value", value(x)),
                    ("min", value(min)),
                    ("max", value(max)),
                ]),
            ),
        }
    }
}

/// The conversion cases of numeric.md 10, item 5, which hold on every target: the web test
/// `number.convert`.
pub fn check_conversions() -> Result<(), String> {
    let cases: [(&str, bool); 9] = [
        ("NaN into i32", to_i32(f64::NAN) == Err(NumError::NotFinite)),
        (
            "+inf into f64",
            finite(f64::INFINITY) == Err(NumError::NotFinite),
        ),
        (
            "-inf into u32",
            to_u32(f64::NEG_INFINITY) == Err(NumError::NotFinite),
        ),
        ("2.5 into i32", to_i32(2.5) == Err(NumError::NotInteger)),
        (
            "2^31 into i32",
            matches!(to_i32(2_147_483_648.0), Err(NumError::OutOfRange { .. })),
        ),
        (
            "-1 into u32",
            matches!(to_u32(-1.0), Err(NumError::OutOfRange { .. })),
        ),
        ("-0 into u32", to_u32(-0.0) == Ok(0)),
        (
            "2^53 into tick",
            matches!(
                to_tick(9_007_199_254_740_992.0),
                Err(NumError::OutOfRange { .. })
            ),
        ),
        (
            "2^53 - 1 into tick",
            to_tick(MAX_TICK) == Ok(Tick(9_007_199_254_740_991)),
        ),
    ];
    match cases.iter().find(|c| !c.1) {
        Some((name, _)) => Err(format!("conversion case failed: {name}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        check_conversions().unwrap();
        assert_eq!(to_i32(-2_147_483_648.0), Ok(i32::MIN));
        assert_eq!(to_u32(4_294_967_295.0), Ok(u32::MAX));
        assert_eq!(to_i32(-0.0), Ok(0));
        assert_eq!(finite(-0.0).map(f64::to_bits), Ok((-0.0f64).to_bits()));
    }

    #[test]
    fn problems_name_field_and_value() {
        let p = NumError::NotFinite.problem("hp", f64::NAN);
        assert_eq!(p.code, "number.not_finite");
        assert_eq!(p.detail["value"], json!("NaN"));
        let p = NumError::NotInteger.problem("count", 2.5);
        assert_eq!(p.code, "number.not_integer");
        assert_eq!(p.detail["value"], json!(2.5));
        assert!(p.message.contains("Math.round"));
        let e = to_u32(-1.0).unwrap_err();
        let p = e.problem("gold", -1.0);
        assert_eq!(p.code, "number.out_of_range");
        assert_eq!(p.detail["min"], json!(0));
        assert_eq!(p.detail["max"], json!(4_294_967_295u64));
    }
}
