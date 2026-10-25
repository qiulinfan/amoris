//! The wind and the rain on resident regions whose physics take them (water.md 67): what a step
//! takes from the world's `Wind` and `Rain`, the rain a region is owed between the whole
//! millimetres it gains, and the drift the wind gives the water's surface.
use amoris_assets::water_field::FieldWind;
use amoris_contract::{Problem, detail};
use amoris_sim::math;

use crate::geom::{self, V3};
use crate::{Rain, Wind};

/// Rain reaches a region's columns a millimetre at a time. Added a substep at a time, heavy rain's
/// 10 mm an hour, 23 nanometres a substep at 60 ticks a second and two substeps, would be under half
/// a unit in the last place of a metre of water in f32 (60 nanometres), and lost to rounding.
pub(crate) const RAIN_PULSE_M: f64 = 0.001;

/// The wind's drift of the water's surface as a share of the wind 10 m up: about 3 percent
/// (J. Wu, "Sea-surface drift currents induced by wind and waves", J. Phys. Oceanogr. 13(8),
/// 1983), carried by the top DRIFT_DEPTH_M of the water.
pub(crate) const DRIFT_SHARE: f64 = 0.03;
pub(crate) const DRIFT_DEPTH_M: f64 = 0.5;

fn invalid(message: &str) -> Problem {
    Problem::new("water.invalid_weather", message, detail([]))
}

/// The wind over a region whose corner is at `corner` (world x, z) at `time`: its velocity and
/// its gusts, as `Wind::at` gives them.
pub(crate) fn field_wind(w: &Wind, corner: [f64; 2], time: f64) -> Result<FieldWind, Problem> {
    let numbers = [w.from_deg, w.speed, w.gust, w.gust_period, w.gust_length];
    if numbers.iter().any(|v| !v.is_finite()) || !(0.0..=100.0).contains(&w.speed) {
        return Err(invalid(
            "a Wind over water needs finite numbers and a speed of 0 to 100 m/s",
        ));
    }
    if !(0.0..=1.0).contains(&w.gust) {
        return Err(invalid(
            "a Wind over water gusts by a share of 0 to 1 of its speed",
        ));
    }
    let toward = geom::from_bearing(w.from_deg + 180.0);
    let gusting = w.gust > 0.0 && w.gust_period > 0.0 && w.gust_length > 0.0;
    let (gust, phase, per_m) = if gusting {
        let along = toward[0] * corner[0] + toward[2] * corner[1];
        let phase = math::TAU * (time / w.gust_period - along / w.gust_length);
        let (s, c) = math::sin_cos(phase);
        (w.gust, math::atan2(s, c), math::TAU / w.gust_length)
    } else {
        (0.0, 0.0, 0.0)
    };
    let wind = FieldWind {
        velocity_mps: [toward[0] * w.speed, toward[2] * w.speed],
        gust,
        gust_phase_rad: phase,
        gust_rad_per_m: per_m,
    };
    wind.validate()
        .map_err(|_| invalid("a Wind's gusts do not fit a water step"))?;
    Ok(wind)
}

/// The rain a step adds to every column, m, and what the region is owed after it: the rain
/// fallen since the last millimetre, `dt` seconds more of it.
pub(crate) fn rain_step(r: &Rain, owed: f64, dt: f64) -> Result<(f64, f64), Problem> {
    if !r.valid() {
        return Err(invalid(
            "Rain falls at 0 to 500 mm an hour in drops of 0.5 to 6 mm",
        ));
    }
    let owed = owed + r.depth_per_s() * dt;
    Ok(if owed >= RAIN_PULSE_M {
        (owed, 0.0)
    } else {
        (0.0, owed)
    })
}

/// What the weather does to one region this step: its wind, the rain it gains and the rain it is
/// owed after it.
pub(crate) struct Weather {
    pub wind: Option<FieldWind>,
    pub rain_m: f64,
    pub owed_m: f64,
}
impl Weather {
    /// Nothing blows on the region and no rain reaches it this step: it may rest.
    pub fn calm(&self) -> bool {
        self.rain_m == 0.0 && self.wind.is_none_or(|w| w.velocity_mps == [0.0, 0.0])
    }
}

/// The weather a region's physics take (`WaterPhysics.wind`, `.rain`), from the world's first
/// `Wind` and `Rain`: none without them. `owed` is the rain the region was owed before this step.
#[allow(clippy::too_many_arguments)]
pub(crate) fn weather(
    region: &super::WaterRegion,
    wind: Option<&Wind>,
    rain: Option<&Rain>,
    owed: f64,
    corner: [f64; 2],
    time: f64,
    dt: f64,
) -> Result<Weather, Problem> {
    let wind = match wind.filter(|_| region.physics.wind) {
        Some(w) => Some(field_wind(w, corner, time)?),
        None => None,
    };
    let (rain_m, owed_m) = match rain.filter(|_| region.physics.rain) {
        Some(r) => rain_step(r, owed, dt)?,
        None => (0.0, owed),
    };
    Ok(Weather {
        wind,
        rain_m,
        owed_m,
    })
}

/// The wind's drift at a point `depth` metres under the surface (world x, z), m/s.
pub(crate) fn surface_drift(w: &Wind, at: V3, time: f64, depth: f64) -> [f64; 2] {
    let share = DRIFT_SHARE * math::max(1.0 - math::max(depth, 0.0) / DRIFT_DEPTH_M, 0.0);
    let u = w.at(at, time);
    [u[0] * share, u[2] * share]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wind_over_a_region_is_the_winds_and_rain_comes_a_millimetre_at_a_time() {
        let w = Wind {
            gust: 0.2,
            ..Wind::steady(270.0, 10.0)
        };
        let f = field_wind(&w, [30.0, -12.0], 7.5).unwrap();
        assert!((f.velocity_mps[0] - 10.0).abs() < 1e-12 && f.velocity_mps[1].abs() < 1e-9);
        // The gusts the kernel works out at a point 4 m downwind of the corner are Wind::at's.
        let speed = 10.0 * (1.0 + f.gust * math::sin(f.gust_phase_rad - f.gust_rad_per_m * 4.0));
        let at = geom::length(w.at([34.0, 0.0, -12.0], 7.5));
        assert!((speed - at).abs() < 1e-9, "{speed} {at}");
        // 36 mm an hour is 10 micrometres a second: over 150 s, one millimetre reaches the water
        // and the rest is owed, nothing lost.
        let r = Rain {
            intensity_mm_h: 36.0,
            drop_mm: 2.0,
        };
        let (mut owed, mut fallen, mut pulses) = (0.0, 0.0, 0);
        for _ in 0..9000 {
            let (rain, next) = rain_step(&r, owed, 1.0 / 60.0).unwrap();
            if rain > 0.0 {
                pulses += 1;
                assert!((0.001..0.00101).contains(&rain), "{rain}");
            }
            fallen += rain;
            owed = next;
        }
        assert_eq!(pulses, 1);
        assert!((fallen + owed - 0.0015).abs() < 1e-12, "{fallen} {owed}");
        assert!(
            rain_step(
                &Rain {
                    intensity_mm_h: 600.0,
                    ..r
                },
                0.0,
                0.1
            )
            .is_err()
        );
        // The drift: 3% of the wind at the surface, none half a metre down.
        let d = surface_drift(&Wind::steady(270.0, 10.0), [0.0; 3], 0.0, 0.0);
        assert!((d[0] - 0.3).abs() < 1e-12 && d[1].abs() < 1e-12);
        assert_eq!(
            surface_drift(&Wind::steady(270.0, 10.0), [0.0; 3], 0.0, 0.5),
            [0.0, 0.0]
        );
    }
}
