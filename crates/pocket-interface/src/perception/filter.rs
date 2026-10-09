//! `nearby`'s filters (shared/contract/perception.md, Queries): kinds, a distance, a sector of
//! directions (absolute bearings, or angles off the heading) and visibilities. Requests are taken
//! exactly as sent: a value out of range is refused with its range, never clamped or wrapped
//! (README, Numbers).

use pocket_contract::codes::{Range, out_of_range};
use pocket_contract::{Pointer, Problem};
use serde_json::Value;

use super::query::NearbyRequest;
use super::view::Percept;

fn refuse(path: Pointer, got: f64, range: Range, hint: Option<&str>) -> Problem {
    let got = serde_json::Number::from_f64(got).map_or(Value::Null, Value::Number);
    out_of_range(&path, &got, &range, hint)
}

/// Checks `within_m` (above 0) and the sector's angles: bearings in [0, 360), relative angles in
/// [-180, 180].
pub fn check_nearby(req: &NearbyRequest) -> Result<(), Problem> {
    if let Some(w) = req.within_m
        && !(w.is_finite() && w > 0.0)
    {
        let range = Range {
            min_exclusive: Some(0.into()),
            ..Range::default()
        };
        return Err(refuse(Pointer::root().key("within_m"), w, range, None));
    }
    if let Some(s) = &req.sector {
        for (key, x) in [("from_deg", s.from_deg), ("to_deg", s.to_deg)] {
            let path = Pointer::root().key("sector").key(key);
            if s.relative {
                if !(x.is_finite() && (-180.0..=180.0).contains(&x)) {
                    return Err(refuse(path, x, Range::inclusive(-180, 180), None));
                }
            } else if !(x.is_finite() && (0.0..360.0).contains(&x)) {
                let hint = (x == 360.0).then_some("A bearing of 360 is 0.");
                return Err(refuse(path, x, Range::half_open(0, 360), hint));
            }
        }
    }
    Ok(())
}

/// Whether `a` lies on the clockwise arc from `from` to `to`, inclusive (degrees).
pub fn in_arc(a: f64, from: f64, to: f64) -> bool {
    let norm = |x: f64| {
        let r = x % 360.0;
        if r < 0.0 { r + 360.0 } else { r }
    };
    norm(a - from) <= norm(to - from)
}

/// Whether a percept passes the request's filters; `relative` gives a bearing's angle off the
/// heading.
pub fn keeps(req: &NearbyRequest, p: &Percept, relative: impl Fn(f64) -> f64) -> bool {
    if let Some(kinds) = &req.kinds
        && !kinds.contains(&p.kind)
    {
        return false;
    }
    if let Some(w) = req.within_m
        && p.range_m > w
    {
        return false;
    }
    if let Some(v) = &req.visibility
        && !v.contains(&p.visibility)
    {
        return false;
    }
    if let Some(s) = &req.sector {
        let a = if s.relative {
            relative(p.bearing_deg)
        } else {
            p.bearing_deg
        };
        if !in_arc(a, s.from_deg, s.to_deg) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcs() {
        assert!(in_arc(0.0, -30.0, 30.0));
        assert!(in_arc(-30.0, -30.0, 30.0));
        assert!(in_arc(30.0, -30.0, 30.0));
        assert!(!in_arc(31.0, -30.0, 30.0));
        assert!(in_arc(350.0, 340.0, 20.0));
        assert!(in_arc(10.0, 340.0, 20.0));
        assert!(!in_arc(180.0, 340.0, 20.0));
    }
}
