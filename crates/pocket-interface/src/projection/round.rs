//! Rounding and value formatting every projection shares (shared/contract/projection.md,
//! Rounding): a value at its declared precision through Rust's `format!("{:.*}", p, v)`, which
//! rounds the exact binary value to the nearest decimal and ties to even; a negative zero loses its
//! sign; a bearing that rounds to 360 is 0. Stored values are that string parsed back, so both
//! lines store the same bits.

use crate::perception::defs::Unit;
use crate::perception::state::FactValue;

/// How a value is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A number at its precision with fixed decimals.
    Number,
    /// Degrees in [0, 360): three integer digits in text; 360 after rounding is 0.
    Bearing,
    Bool,
    /// Text and enumeration values: bare when they match `[A-Za-z0-9_.:+-]+`, else quoted.
    Text,
    /// `(x,y,z)` in text, `{"x","y","z"}` in JSON.
    Position,
    /// The raw omniscient view's component fields: no rounding beyond the number's own shortest
    /// form (perception.md, The omniscient view).
    Raw,
}

/// A value's style and precision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub style: Style,
    pub precision: u8,
}

impl Format {
    pub const TEXT: Format = Format {
        style: Style::Text,
        precision: 0,
    };

    pub fn of(unit: &Unit, precision: u8) -> Format {
        let style = match unit {
            Unit::Bearing => Style::Bearing,
            Unit::Bool => Style::Bool,
            Unit::Text | Unit::Enum { .. } => Style::Text,
            Unit::Position => Style::Position,
            _ => Style::Number,
        };
        Format { style, precision }
    }

    pub const RAW: Format = Format {
        style: Style::Raw,
        precision: 0,
    };

    pub fn number(precision: u8) -> Format {
        Format {
            style: Style::Number,
            precision,
        }
    }
}

/// `format!("{:.*}", p, v)` with a negative zero's sign dropped (`-0.0` is written `0.0`).
pub fn fixed(v: f64, p: u8) -> String {
    let s = format!("{:.*}", usize::from(p), v);
    match s.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|c| c == b'0' || c == b'.') => rest.to_owned(),
        _ => s,
    }
}

/// A bearing at precision `p`: 360 after rounding is 0.
pub fn fixed_bearing(v: f64, p: u8) -> String {
    let s = fixed(v, p);
    if s.parse::<f64>().is_ok_and(|x| x == 360.0) {
        fixed(0.0, p)
    } else {
        s
    }
}

/// The stored form of a number at precision `p`: the formatted decimal parsed back.
pub fn stored(v: f64, p: u8) -> f64 {
    fixed(v, p).parse().unwrap_or(v)
}

/// The stored form of a bearing at precision `p`.
pub fn stored_bearing(v: f64, p: u8) -> f64 {
    fixed_bearing(v, p).parse().unwrap_or(v)
}

/// The precision of a reported range (and of a bearing in JSON): 0 from 100 m, 1 below.
pub fn range_precision(range_m: f64) -> u8 {
    if range_m >= 100.0 { 0 } else { 1 }
}

/// A fact value's stored form at its format (projection.md, Stored values).
pub fn store(v: &FactValue, f: Format) -> FactValue {
    match (v, f.style) {
        (FactValue::Number(x), Style::Bearing) => {
            FactValue::Number(stored_bearing(*x, f.precision))
        }
        (FactValue::Number(x), _) => FactValue::Number(stored(*x, f.precision)),
        (FactValue::Position(p), _) => FactValue::Position(p.map(|x| stored(x, f.precision))),
        _ => v.clone(),
    }
}

/// `true` for text written bare: `[A-Za-z0-9_.:+-]+`.
pub fn bare(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:+-".contains(&c))
}

/// A JSON string literal.
pub fn quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned())
}

/// Text written bare or as a JSON string.
pub fn text_word(s: &str) -> String {
    if bare(s) { s.to_owned() } else { quote(s) }
}

/// A bearing as three integer digits (`007`, `270`), with decimals when its precision has them.
pub fn bbb(v: f64, p: u8) -> String {
    let s = fixed_bearing(v, p);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s.as_str(), None),
    };
    let mut out = format!("{int:0>3}");
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// A number in its own shortest form (the raw omniscient view).
pub fn raw(x: f64) -> String {
    if x == 0.0 {
        "0".to_owned()
    } else {
        x.to_string()
    }
}

/// A number in JSON in its own shortest form.
pub fn json_raw(x: f64) -> String {
    serde_json::to_string(&x).unwrap_or_else(|_| "null".to_owned())
}

/// A value in the text projection.
pub fn text(v: &FactValue, f: Format) -> String {
    match v {
        FactValue::Bool(b) => b.to_string(),
        FactValue::Text(s) => text_word(s),
        FactValue::Number(x) => match f.style {
            Style::Bearing => bbb(*x, f.precision),
            Style::Raw => raw(*x),
            _ => fixed(*x, f.precision),
        },
        FactValue::Position(p) if f.style == Style::Raw => {
            format!("({},{},{})", raw(p[0]), raw(p[1]), raw(p[2]))
        }
        FactValue::Position(p) => format!(
            "({},{},{})",
            fixed(p[0], f.precision),
            fixed(p[1], f.precision),
            fixed(p[2], f.precision)
        ),
    }
}

/// A number in JSON at precision `p`: the number its formatted decimal denotes, an integer at
/// precision 0.
pub fn json_number(s: &str, p: u8) -> String {
    if p == 0 {
        return s.to_owned();
    }
    match s.parse::<f64>() {
        Ok(x) => serde_json::to_string(&x).unwrap_or_else(|_| s.to_owned()),
        Err(_) => s.to_owned(),
    }
}

/// A number at precision `p` in JSON.
pub fn json_fixed(v: f64, p: u8) -> String {
    json_number(&fixed(v, p), p)
}

/// A bearing at precision `p` in JSON.
pub fn json_bearing(v: f64, p: u8) -> String {
    json_number(&fixed_bearing(v, p), p)
}

/// A position in JSON at precision `p`.
pub fn json_position(p: [f64; 3], precision: u8) -> String {
    format!(
        "{{\"x\":{},\"y\":{},\"z\":{}}}",
        json_fixed(p[0], precision),
        json_fixed(p[1], precision),
        json_fixed(p[2], precision)
    )
}

/// A value in the JSON projection.
pub fn json(v: &FactValue, f: Format) -> String {
    match v {
        FactValue::Bool(b) => b.to_string(),
        FactValue::Text(s) => quote(s),
        FactValue::Number(x) => match f.style {
            Style::Bearing => json_bearing(*x, f.precision),
            Style::Raw => json_raw(*x),
            _ => json_fixed(*x, f.precision),
        },
        FactValue::Position(p) if f.style == Style::Raw => format!(
            "{{\"x\":{},\"y\":{},\"z\":{}}}",
            json_raw(p[0]),
            json_raw(p[1]),
            json_raw(p[2])
        ),
        FactValue::Position(p) => json_position(*p, f.precision),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_cases() {
        // projection.md, Rounding: ties to even on the exact binary value.
        assert_eq!(fixed(0.5, 0), "0");
        assert_eq!(fixed(1.5, 0), "2");
        assert_eq!(fixed(2.5, 0), "2");
        assert_eq!(fixed(0.25, 1), "0.2");
        assert_eq!(fixed(0.35, 1), "0.3");
        assert_eq!(fixed(-0.04, 1), "0.0");
        assert_eq!(fixed(-0.0, 0), "0");
        assert_eq!(fixed(-0.4, 0), "0");
        assert_eq!(fixed(-1.5, 0), "-2");
        assert_eq!(fixed_bearing(359.6, 0), "0");
        assert_eq!(fixed_bearing(359.4, 0), "359");
        assert_eq!(bbb(7.0, 0), "007");
        assert_eq!(bbb(359.6, 0), "000");
        assert_eq!(bbb(92.25, 1), "092.2");
        assert_eq!(stored(-0.04, 1).to_bits(), 0f64.to_bits());
        assert_eq!(stored_bearing(359.7, 0), 0.0);
        assert_eq!(json_fixed(412.4, 0), "412");
        assert_eq!(json_fixed(3.40, 2), "3.4");
        assert_eq!(json_fixed(0.0, 1), "0.0");
        assert_eq!(
            json_position([424.24, 0.0, 11.26], 1),
            r#"{"x":424.2,"y":0.0,"z":11.3}"#
        );
        assert_eq!(range_precision(99.99), 1);
        assert_eq!(range_precision(100.0), 0);
    }

    #[test]
    fn words() {
        assert_eq!(text_word("close_hauled"), "close_hauled");
        assert_eq!(text_word("sail.no_progress"), "sail.no_progress");
        assert_eq!(text_word("No progress: 5 m"), "\"No progress: 5 m\"");
        assert_eq!(text_word(""), "\"\"");
        let f = Format::of(&Unit::Fraction { min: 0.0, max: 1.0 }, 2);
        assert_eq!(text(&FactValue::Number(1.0), f), "1.00");
        assert_eq!(json(&FactValue::Number(1.0), f), "1.0");
        let pos = Format::of(&Unit::Position, 1);
        assert_eq!(
            text(&FactValue::Position([12.44, 0.0, -3.06]), pos),
            "(12.4,0.0,-3.1)"
        );
    }
}
