//! The third-person seat's derived instruments (docs/spec/player.md 5.1): the way its body faces
//! and its camera looks, as bearings, and whether it is locked on. A game names them in its
//! perception declarations (`samples/thornkeep/perception.json`); an affordance's `facing`
//! requirement reads the instrument named `heading_deg`.

use amoris_physics::{ThirdPerson, Walker};
use amoris_sim::math;

use super::defs::PerceptionDefs;
use super::facts::FactCx;
use super::geometry::bearing_deg;
use super::state::FactValue;

pub fn functions(defs: PerceptionDefs) -> PerceptionDefs {
    defs.derive("third_person.heading_deg", heading_deg)
        .derive("third_person.look_deg", look_deg)
        .derive("third_person.locked", locked)
}

/// A walker yaw (0 along -z, positive turning left) as a bearing (0 north, 90 east).
fn bearing_of(yaw: f64) -> f64 {
    let (s, c) = math::sin_cos(yaw);
    bearing_deg(-s, -c)
}

fn heading_deg(cx: &FactCx<'_>) -> Option<FactValue> {
    Some(FactValue::Number(bearing_of(
        cx.get::<ThirdPerson>()?.facing,
    )))
}

fn look_deg(cx: &FactCx<'_>) -> Option<FactValue> {
    Some(FactValue::Number(bearing_of(cx.get::<Walker>()?.yaw)))
}

fn locked(cx: &FactCx<'_>) -> Option<FactValue> {
    Some(FactValue::Bool(cx.get::<ThirdPerson>()?.lock.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaws_are_bearings() {
        assert!(bearing_of(0.0).abs() < 1e-9);
        assert!((bearing_of(std::f64::consts::FRAC_PI_2) - 270.0).abs() < 1e-9);
        assert!((bearing_of(-std::f64::consts::FRAC_PI_2) - 90.0).abs() < 1e-9);
        assert!((bearing_of(std::f64::consts::PI) - 180.0).abs() < 1e-9);
    }
}
