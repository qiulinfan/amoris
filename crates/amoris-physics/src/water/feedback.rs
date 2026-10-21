//! Explicit opt-in and persisted next-tick horizontal reactions. Engine-owned outputs are not
//! an editable second copy of field state. The authoritative packet enters the resident program.
use amoris_assets::water_field::{MAX_SAMPLES, PointImpulse};
use amoris_contract::Problem;
use bevy_ecs::prelude::Component;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReactionBudget {
    pub applied: [f64; 2],
    pub rejected: [f64; 2],
    pub roundoff: [f64; 2],
}

#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct WaterFeedback {
    pub pending: Vec<PointImpulse>,
    pub queued_tick: u64,
    pub applied_tick: u64,
    pub source_tick: u64,
    pub requested: [f64; 2],
    pub applied: [f64; 2],
    pub rejected: [f64; 2],
    pub roundoff: [f64; 2],
    pub quantization: [f64; 2],
    pub accumulation: [f64; 2],
    pub emitted_total: [f64; 2],
    pub applied_total: [f64; 2],
    pub rejected_total: [f64; 2],
    pub residual_total: [f64; 2],
}
impl amoris_sim::Persisted for WaterFeedback {
    const NAME: &'static str = "WaterFeedback";
    const VERSION: u32 = 1;
}
impl WaterFeedback {
    pub(crate) fn validate_source(&self, tick: u64) -> Result<(), Problem> {
        if !self.pending.is_empty() && self.queued_tick.checked_add(1) != Some(tick) {
            return Err(super::resident::failed(
                "water reaction packet is not from the preceding completed boundary",
            ));
        }
        Ok(())
    }
    #[allow(clippy::cast_possible_truncation)] // Compare the explicit f32 GPU payload with the f64 body impulse.
    pub(crate) fn complete(&mut self, tick: u64, report: ReactionBudget) -> Result<(), Problem> {
        self.validate_source(tick)?;
        let mut requested = [0.0; 2];
        let mut quantized = [0.0; 2];
        let mut scale = [0.0; 2];
        for input in &self.pending {
            for a in 0..2 {
                requested[a] += input.impulse[a];
                scale[a] += input.impulse[a].abs();
                quantized[a] += f64::from(input.impulse[a] as f32);
            }
        }
        let accumulation = std::array::from_fn(|a| {
            quantized[a] - report.applied[a] - report.rejected[a] - report.roundoff[a]
        });
        for a in 0..2 {
            if ![
                requested[a],
                quantized[a],
                scale[a],
                report.applied[a],
                report.rejected[a],
                report.roundoff[a],
                accumulation[a],
            ]
            .iter()
            .all(|v| v.is_finite())
                || accumulation[a].abs() > 1e-6 + scale[a] * 0.001
            {
                return Err(super::resident::failed(
                    "water reaction budget does not close within bounded f32 accumulation error",
                ));
            }
        }
        self.source_tick = if self.pending.is_empty() {
            0
        } else {
            self.queued_tick
        };
        self.applied_tick = tick;
        self.requested = requested;
        self.applied = report.applied;
        self.rejected = report.rejected;
        self.roundoff = report.roundoff;
        self.quantization = std::array::from_fn(|a| requested[a] - quantized[a]);
        self.accumulation = accumulation;
        for (a, error) in accumulation.iter().enumerate() {
            self.applied_total[a] += report.applied[a];
            self.rejected_total[a] += report.rejected[a];
            self.residual_total[a] += report.roundoff[a] + self.quantization[a] + error;
        }
        self.pending.clear();
        self.queued_tick = tick;
        Ok(())
    }
    pub(crate) fn queue(
        &mut self,
        tick: u64,
        at: [f64; 2],
        impulse: [f64; 2],
    ) -> Result<(), Problem> {
        if self.queued_tick != tick {
            return Err(super::resident::failed(
                "water reaction queue is not at the running boundary",
            ));
        }
        if impulse == [0.0; 2] {
            return Ok(());
        }
        if self.pending.len() >= MAX_SAMPLES
            || at
                .iter()
                .chain(&impulse)
                .any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX))
        {
            return Err(super::resident::failed(
                "water reaction is non-finite or exceeds the point budget",
            ));
        }
        self.pending.push(PointImpulse { at, impulse });
        for (total, value) in self.emitted_total.iter_mut().zip(impulse) {
            *total += value;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejected_receipts_leave_the_packet_and_totals_intact() {
        let mut f = WaterFeedback::default();
        f.complete(1, ReactionBudget::default()).unwrap();
        f.queue(1, [3.5, 4.5], [3.0, -2.0]).unwrap();
        let pending = f.clone();
        assert!(f.complete(1, ReactionBudget::default()).is_err());
        assert_eq!(f, pending);
        assert!(f.complete(2, ReactionBudget::default()).is_err());
        assert_eq!(f, pending);
        f.complete(
            2,
            ReactionBudget {
                applied: [3.0, -2.0],
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(f.source_tick, 1);
        assert!(f.pending.is_empty());
        assert_eq!(f.applied_total, f.emitted_total);
        f.complete(3, ReactionBudget::default()).unwrap();
        assert_eq!(f.source_tick, 0);
        assert_eq!(f.applied_total, [3.0, -2.0]);
    }
}
