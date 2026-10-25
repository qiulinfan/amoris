//! Explicit opt-in two-way vertical coupling (water.md 46). Floater points and Walkers in a
//! resident region queue the volume they hold under its free surface; the next step loads that
//! volume as a pressure head on the water columns, so bodies push the water aside and make waves.
//! The packet is persisted world state, exactly as WaterFeedback's horizontal reactions are.
use amoris_assets::water_field::{MAX_POINT_RADIUS, MAX_POINT_VOLUME_M3, MAX_SAMPLES, PointVolume};
use amoris_contract::Problem;
use bevy_ecs::prelude::Component;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The volume of a swimmer's body, m^3: about a 70 kg person at the density of water.
pub const WALKER_BODY_M3: f64 = 0.07;
/// The radius of the water a swimmer's body pushes aside, m.
pub const WALKER_FOOTPRINT_M: f64 = 0.4;

#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WaterDisplacement {
    /// Grid points and volumes queued at `queued_tick` for the next step.
    pub pending: Vec<PointVolume>,
    pub queued_tick: u64,
    pub applied_tick: u64,
    /// The volume the last step loaded, m^3.
    pub applied_m3: f64,
}
impl amoris_sim::Persisted for WaterDisplacement {
    const NAME: &'static str = "WaterDisplacement";
    const VERSION: u32 = 1;
}
impl WaterDisplacement {
    pub(crate) fn validate_source(&self, tick: u64) -> Result<(), Problem> {
        if !self.pending.is_empty() && self.queued_tick.checked_add(1) != Some(tick) {
            return Err(super::resident::failed(
                "water displacement packet is not from the preceding completed boundary",
            ));
        }
        Ok(())
    }
    /// After a completed step: the packet was loaded, and this tick's bodies queue the next.
    pub(crate) fn complete(&mut self, tick: u64) -> Result<(), Problem> {
        self.validate_source(tick)?;
        self.applied_m3 = self.pending.iter().map(|p| p.volume_m3).sum();
        self.applied_tick = tick;
        self.pending.clear();
        self.queued_tick = tick;
        Ok(())
    }
    /// Queue `volume_m3` at grid point `at`, spread over `radius` columns (clamped to the widest
    /// footprint a point may take).
    pub(crate) fn queue(
        &mut self,
        tick: u64,
        at: [f64; 2],
        volume_m3: f64,
        radius: f64,
    ) -> Result<(), Problem> {
        if self.queued_tick != tick {
            return Err(super::resident::failed(
                "water displacement queue is not at the running boundary",
            ));
        }
        if volume_m3 <= 0.0 {
            return Ok(());
        }
        if self.pending.len() >= MAX_SAMPLES
            || at
                .iter()
                .any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX))
            || !volume_m3.is_finite()
            || volume_m3 > MAX_POINT_VOLUME_M3
            || !radius.is_finite()
        {
            return Err(super::resident::failed(
                "water displacement is non-finite or exceeds the point budget",
            ));
        }
        self.pending.push(PointVolume {
            at,
            volume_m3,
            radius: radius.clamp(0.0, MAX_POINT_RADIUS),
        });
        Ok(())
    }
}

/// The head, m, that a cubic metre queued at grid point `source` over `radius` columns adds at
/// grid point `at`, as the step scatters it (water_occupancy.wgsl: weights (1 - d^2/R^2)^2 at the
/// covered column centres, R from one to eight columns, normalized over the columns inside the
/// region) and as samples read it (bilinear between column centres), in a region of `size`
/// columns `cell` metres wide.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn head_per_m3(
    source: [f64; 2],
    radius: f64,
    at: [f64; 2],
    size: [u32; 2],
    cell: f64,
) -> f64 {
    let r = radius.clamp(1.0, 8.0);
    let last = size.map(|n| i64::from(n) - 1);
    let weight = |c: [i64; 2]| {
        let d = [0, 1].map(|a| (c[a] as f64 + 0.5 - source[a]) / r);
        let s = amoris_sim::math::max(1.0 - d[0] * d[0] - d[1] * d[1], 0.0);
        s * s
    };
    let lo = [0, 1].map(|a| ((source[a] - 0.5 - r).floor() as i64).max(0));
    let hi = [0, 1].map(|a| ((source[a] - 0.5 + r).ceil() as i64).min(last[a]));
    let mut total = 0.0;
    for j in lo[1]..=hi[1] {
        for i in lo[0]..=hi[0] {
            total += weight([i, j]);
        }
    }
    let home = [0, 1].map(|a| (source[a].floor() as i64).clamp(0, last[a]));
    let share = |c: [i64; 2]| {
        if total <= 0.0 {
            return if c == home { 1.0 } else { 0.0 };
        }
        let inside = (0..2).all(|a| c[a] >= lo[a] && c[a] <= hi[a]);
        if inside { weight(c) / total } else { 0.0 }
    };
    let q = [0, 1].map(|a| (at[a] - 0.5).clamp(0.0, last[a] as f64));
    let l = q.map(|v| v.floor() as i64);
    let h = [0, 1].map(|a| (l[a] + 1).min(last[a]));
    let t = [0, 1].map(|a| q[a] - l[a] as f64);
    let row = |k: i64| share([l[0], k]) * (1.0 - t[0]) + share([h[0], k]) * t[0];
    (row(l[1]) * (1.0 - t[1]) + row(h[1]) * t[1]) / (cell * cell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_cubic_metre_spreads_to_its_volume_over_the_columns() {
        // Summed over every column centre, the head times a column's area is the volume.
        let size = [32, 24];
        for (source, radius) in [([10.3, 7.8], 2.5), ([0.4, 0.6], 3.0), ([31.5, 23.2], 1.0)] {
            let mut volume = 0.0;
            for k in 0..size[1] {
                for i in 0..size[0] {
                    let at = [f64::from(i) + 0.5, f64::from(k) + 0.5];
                    volume += head_per_m3(source, radius, at, size, 0.5) * 0.25;
                }
            }
            assert!((volume - 1.0).abs() < 1e-9, "{source:?} {radius}: {volume}");
        }
        // The peak sits at the source, three times the disk's mean head.
        let peak = head_per_m3([16.5, 12.5], 6.0, [16.5, 12.5], size, 1.0);
        let mean = 1.0 / (std::f64::consts::PI * 36.0);
        assert!((peak / mean - 3.0).abs() < 0.1, "{}", peak / mean);
    }

    #[test]
    fn a_packet_belongs_to_the_preceding_boundary_and_clears_once_applied() {
        let mut d = WaterDisplacement::default();
        d.complete(1).unwrap();
        d.queue(1, [3.5, 4.5], 0.25, 2.0).unwrap();
        d.queue(1, [3.5, 4.5], 0.0, 2.0).unwrap();
        assert_eq!(d.pending.len(), 1);
        assert!(d.queue(2, [1.0, 1.0], 0.1, 1.0).is_err());
        let pending = d.clone();
        assert!(d.complete(3).is_err());
        assert_eq!(d, pending);
        d.complete(2).unwrap();
        assert!(d.pending.is_empty());
        assert_eq!(d.applied_m3, 0.25);
        assert!(d.queue(2, [f64::NAN, 1.0], 0.1, 1.0).is_err());
        assert!(d.queue(2, [1.0, 1.0], 0.1, f64::NAN).is_err());
        d.queue(2, [1.0, 1.0], 0.1, 50.0).unwrap();
        assert_eq!(d.pending[0].radius, MAX_POINT_RADIUS);
    }
}
