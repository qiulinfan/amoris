//! Area-measure RIS/ReSTIR GI reference math for a single secondary-surface sample.
//!
//! A candidate is a point on a triangle, its world normal, and the outgoing radiance arriving at
//! the receiving surface. It is not a pixel color or a temporal color average. For receiver x₁ and
//! secondary point x₂ the contribution is (albedo / π) × Lₒ(x₂) × cos₁ cos₂ / |x₂−x₁|².
//! Convert the initial directional proposal to area measure: q_A = q_ω cos₂ / distance².
//!
//! Weighted reservoir selection stores sum w, represented candidate count M, and the selected
//! candidate's target p̂. Its normalization is sum w / (M p̂_selected). An initial candidate has
//! w = p̂ / q_A. A source reservoir reused at a new receiver has w = p̂_new × W_source × M_source.
//!
//! The caller must ray-test the receiver→selected surface segment with a geometric-normal origin
//! offset and verify the target triangle identity. `estimate` requires explicit visibility.
//! Reuse also needs compatible static geometry/material/light epochs, normals and reprojection;
//! dynamic scenes and history resets must reject old reservoirs. These equations alone do not
//! supply ReSTIR GI's complete support/visibility/M correction or generalized full-path
//! reconnection. A fixed secondary surface point is reused in the same area domain: the initial
//! solid-angle-to-area conversion already handles geometry, so do not apply that Jacobian twice. This module is a single-bounce GI prototype, not ReSTIR PT.
//! Reference: https://research.nvidia.com/publication/2021-06_restir-gi-path-resampling-real-time-path-tracing

#![allow(clippy::disallowed_methods)]

use glam::Vec3;
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GiReceiver {
    pub position: Vec3,
    pub normal: Vec3,
    pub albedo: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GiCandidate {
    pub second_position: Vec3,
    pub second_normal: Vec3,
    pub outgoing_radiance: Vec3,
    pub primitive_id: u32,
    pub material_id: u32,
}

impl GiCandidate {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.second_position.is_finite()
            || !unit_normal(self.second_normal)
            || !self.outgoing_radiance.is_finite()
            || self.outgoing_radiance.min_element() < 0.0
        {
            return Err(
                "candidate must have finite position, unit normal and nonnegative radiance",
            );
        }
        Ok(())
    }

    pub fn evaluate(&self, receiver: &GiReceiver) -> Result<Vec3, &'static str> {
        self.validate()?;
        validate_receiver(receiver)?;
        let delta = self.second_position - receiver.position;
        let distance_squared = delta.length_squared();
        if !distance_squared.is_finite() {
            return Err("candidate distance is nonfinite");
        }
        if distance_squared <= 1e-8 {
            return Ok(Vec3::ZERO);
        }
        let direction = delta / distance_squared.sqrt();
        let receiver_cosine = receiver.normal.dot(direction).max(0.0);
        let secondary_cosine = self.second_normal.dot(-direction).max(0.0);
        let contribution = receiver.albedo
            * self.outgoing_radiance
            * (receiver_cosine * secondary_cosine / (PI * distance_squared));
        if !contribution.is_finite() {
            return Err("candidate contribution is nonfinite");
        }
        Ok(contribution)
    }

    pub fn area_pdf(
        &self,
        receiver_position: Vec3,
        directional_pdf: f64,
    ) -> Result<f64, &'static str> {
        self.validate()?;
        if !receiver_position.is_finite() || !directional_pdf.is_finite() || directional_pdf <= 0.0
        {
            return Err("area PDF conversion needs a finite position and positive directional PDF");
        }
        let delta = self.second_position - receiver_position;
        let distance_squared = delta.length_squared();
        if !distance_squared.is_finite() {
            return Err("candidate distance is nonfinite");
        }
        if distance_squared <= 1e-8 {
            return Ok(0.0);
        }
        let direction = delta / distance_squared.sqrt();
        Ok(
            directional_pdf * f64::from(self.second_normal.dot(-direction).max(0.0))
                / f64::from(distance_squared),
        )
    }
}

/// Positive scalar target; vector contributions remain RGB through the final normalization.
pub fn target_luminance(contribution: Vec3) -> Result<f64, &'static str> {
    if !contribution.is_finite() || contribution.min_element() < 0.0 {
        return Err("target contribution must be finite and nonnegative");
    }
    Ok(f64::from(contribution.x) * 0.2126
        + f64::from(contribution.y) * 0.7152
        + f64::from(contribution.z) * 0.0722)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GiReservoir {
    pub selected: Option<GiCandidate>,
    pub sum_weights: f64,
    pub m: u32,
    pub selected_target: f64,
}

impl GiReservoir {
    /// Adds one valid proposal. A zero target still increments M; malformed proposals do not.
    /// Returns whether the selected sample changed, independently of whether the proposal counted.
    pub fn update(
        &mut self,
        candidate: GiCandidate,
        target: f64,
        area_pdf: f64,
        random: f64,
    ) -> Result<bool, &'static str> {
        if !area_pdf.is_finite() || area_pdf <= 0.0 {
            return Err("area PDF must be finite and positive");
        }
        self.update_weight(candidate, target, target / area_pdf, 1, random)
    }

    /// Merges one already sampled source reservoir, after reevaluating its selected candidate at
    /// this receiver. Use `reuse_allowed` and a fresh visibility query before consuming the result.
    pub fn merge(
        &mut self,
        source: &Self,
        reevaluated_target: f64,
        random: f64,
    ) -> Result<bool, &'static str> {
        source.validate()?;
        let candidate = source
            .selected
            .ok_or("source reservoir has no selected candidate")?;
        let normalization = source.normalization();
        if normalization <= 0.0 {
            return Err("source reservoir normalization must be positive");
        }
        self.update_weight(
            candidate,
            reevaluated_target,
            reevaluated_target * normalization * f64::from(source.m),
            source.m,
            random,
        )
    }

    pub fn normalization(&self) -> f64 {
        if self.selected.is_none()
            || self.m == 0
            || self.selected_target <= 0.0
            || !self.sum_weights.is_finite()
            || !self.selected_target.is_finite()
        {
            return 0.0;
        }
        let weight = self.sum_weights / (f64::from(self.m) * self.selected_target);
        if weight.is_finite() && weight >= 0.0 {
            weight
        } else {
            0.0
        }
    }

    /// Visibility must come from a receiver→candidate triangle ray. Occluded candidates evaluate
    /// to zero; visibility must not be inferred from a previous pixel's depth/color history.
    pub fn estimate(&self, receiver: &GiReceiver, visible: bool) -> Result<Vec3, &'static str> {
        self.validate()?;
        if !visible {
            return Ok(Vec3::ZERO);
        }
        let Some(candidate) = self.selected else {
            return Ok(Vec3::ZERO);
        };
        let estimate = candidate.evaluate(receiver)? * self.normalization() as f32;
        if !estimate.is_finite() {
            return Err("reservoir estimate is nonfinite");
        }
        Ok(estimate)
    }

    /// Limits history's represented count while preserving W. This bounds temporal influence;
    /// it does not make correlated spatial/temporal reuse unbiased.
    pub fn limit_history(&mut self, maximum_m: u32) -> Result<(), &'static str> {
        self.validate()?;
        if maximum_m == 0 {
            return Err("history limit must be positive");
        }
        if self.m > maximum_m {
            self.sum_weights *= f64::from(maximum_m) / f64::from(self.m);
            self.m = maximum_m;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.sum_weights.is_finite()
            || self.sum_weights < 0.0
            || !self.selected_target.is_finite()
            || self.selected_target < 0.0
        {
            return Err("reservoir weights and targets must be finite and nonnegative");
        }
        match self.selected {
            Some(candidate) => {
                candidate.validate()?;
                if self.m == 0 || self.selected_target <= 0.0 || self.sum_weights <= 0.0 {
                    return Err("selected reservoir requires positive M, target and weight");
                }
            }
            None if self.sum_weights != 0.0 || self.selected_target != 0.0 => {
                return Err("unselected reservoir must have zero weight and target");
            }
            None => {}
        }
        Ok(())
    }

    fn update_weight(
        &mut self,
        candidate: GiCandidate,
        target: f64,
        weight: f64,
        represented_m: u32,
        random: f64,
    ) -> Result<bool, &'static str> {
        self.validate()?;
        candidate.validate()?;
        if !target.is_finite() || target < 0.0 || !weight.is_finite() || weight < 0.0 {
            return Err("target and weight must be finite and nonnegative");
        }
        if (target == 0.0) != (weight == 0.0) {
            return Err("positive weights require positive targets");
        }
        if !random.is_finite() || !(0.0..1.0).contains(&random) {
            return Err("random must be finite in [0, 1)");
        }
        if represented_m == 0 {
            return Err("represented M must be positive");
        }
        let next_m = self
            .m
            .checked_add(represented_m)
            .ok_or("reservoir M overflow")?;
        let next_sum = self.sum_weights + weight;
        if !next_sum.is_finite() {
            return Err("reservoir weight overflow");
        }
        let selected = weight > 0.0 && random * next_sum < weight;
        self.m = next_m;
        self.sum_weights = next_sum;
        if selected {
            self.selected = Some(candidate);
            self.selected_target = target;
        }
        Ok(selected)
    }
}

/// Admission guard only, without reprojection or tracing. Epochs must change when geometry,
/// materials, lights or bake configuration change. Temporal samples need an earlier frame;
/// same-frame sources are allowed only for the spatial mode. `history_valid` must include the
/// caller's reprojection/depth/primitive checks. Dynamic geometry is outside this prototype.
#[derive(Clone, Copy, Debug)]
pub struct ReuseContext {
    pub current_scene_epoch: u32,
    pub source_scene_epoch: u32,
    pub current_frame: u32,
    pub source_frame: u32,
    pub temporal: bool,
    pub history_valid: bool,
    pub scene_static: bool,
    pub current_normal: Vec3,
    pub source_normal: Vec3,
    pub minimum_normal_cosine: f32,
}

pub fn reuse_allowed(context: &ReuseContext) -> bool {
    context.history_valid
        && context.scene_static
        && context.current_scene_epoch == context.source_scene_epoch
        && ((!context.temporal && context.current_frame == context.source_frame)
            || (context.temporal && context.source_frame < context.current_frame))
        && unit_normal(context.current_normal)
        && unit_normal(context.source_normal)
        && context.minimum_normal_cosine.is_finite()
        && (-1.0..=1.0).contains(&context.minimum_normal_cosine)
        && context.current_normal.dot(context.source_normal) >= context.minimum_normal_cosine
}

fn unit_normal(normal: Vec3) -> bool {
    normal.is_finite() && (normal.length_squared() - 1.0).abs() <= 1e-3
}
fn validate_receiver(receiver: &GiReceiver) -> Result<(), &'static str> {
    if !receiver.position.is_finite()
        || !unit_normal(receiver.normal)
        || !receiver.albedo.is_finite()
        || receiver.albedo.min_element() < 0.0
    {
        return Err("receiver needs finite position, unit normal and nonnegative albedo");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receiver() -> GiReceiver {
        GiReceiver {
            position: Vec3::ZERO,
            normal: Vec3::Z,
            albedo: Vec3::splat(0.5),
        }
    }
    fn candidate() -> GiCandidate {
        GiCandidate {
            second_position: Vec3::Z * 2.0,
            second_normal: -Vec3::Z,
            outgoing_radiance: Vec3::new(4.0, 2.0, 1.0),
            primitive_id: 7,
            material_id: 3,
        }
    }
    struct Random(u64);
    impl Random {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
            let mut x = self.0;
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
            ((x ^ (x >> 31)) >> 11) as f64 / 9007199254740992.0
        }
    }

    #[test]
    fn area_measure_conversion_matches_directional_diffuse_estimator() {
        let receiver = receiver();
        let candidate = candidate();
        let value = candidate.evaluate(&receiver).unwrap();
        let pdf = candidate
            .area_pdf(receiver.position, 1.0 / f64::from(PI))
            .unwrap();
        assert!(
            (value / pdf as f32 - receiver.albedo * candidate.outgoing_radiance)
                .abs()
                .max_element()
                < 1e-6
        );
        let mut far = candidate;
        far.second_position *= 2.0;
        assert!(
            (far.evaluate(&receiver).unwrap() * 4.0 - value)
                .abs()
                .max_element()
                < 1e-6
        );
        assert!(
            (far.area_pdf(receiver.position, 1.0 / f64::from(PI))
                .unwrap()
                * 4.0
                - pdf)
                .abs()
                < 1e-8
        );
        let mut tilted = receiver;
        tilted.normal = Vec3::new(3.0f32.sqrt() / 2.0, 0.0, 0.5);
        let tilted_pdf = candidate
            .area_pdf(tilted.position, 0.5 / f64::from(PI))
            .unwrap();
        assert!(
            (candidate.evaluate(&tilted).unwrap() / tilted_pdf as f32 - value / pdf as f32)
                .abs()
                .max_element()
                < 1e-6
        );
        let mut backwards = candidate;
        backwards.second_normal = Vec3::Z;
        assert_eq!(backwards.evaluate(&receiver).unwrap(), Vec3::ZERO);
        assert_eq!(backwards.area_pdf(receiver.position, 1.0).unwrap(), 0.0);
    }

    #[test]
    fn seeded_ris_constant_integrator_converges_under_nonuniform_proposal_and_target() {
        // Three unit-area bins: f=1, q=[.05,.15,.8], target deliberately differs from f.
        let q = [0.05, 0.15, 0.8];
        let target = [0.3, 1.4, 4.0];
        let mut random = Random(19);
        let mut sum = 0.0;
        let runs = 100_000;
        for _ in 0..runs {
            let mut reservoir = GiReservoir::default();
            for _ in 0..4 {
                let draw = random.next();
                let i = if draw < q[0] {
                    0
                } else if draw < q[0] + q[1] {
                    1
                } else {
                    2
                };
                let mut candidate = candidate();
                candidate.primitive_id = i as u32;
                reservoir
                    .update(candidate, target[i], q[i], random.next())
                    .unwrap();
            }
            sum += reservoir.normalization(); // f(selected)=1, visibility=1.
        }
        let mean = sum / runs as f64;
        assert!((mean - 3.0).abs() < 0.04, "RIS integral {mean}, expected 3");
    }

    #[test]
    fn source_reservoir_merge_converges_and_counts_represented_candidates() {
        let q = [0.2, 0.8];
        let target = [1.0, 5.0];
        let mut random = Random(73);
        let runs = 80_000;
        let mut sum = 0.0;
        for _ in 0..runs {
            let mut combined = GiReservoir::default();
            for _ in 0..3 {
                let mut source = GiReservoir::default();
                for _ in 0..2 {
                    let i = usize::from(random.next() >= q[0]);
                    let mut candidate = candidate();
                    candidate.primitive_id = i as u32;
                    source
                        .update(candidate, target[i], q[i], random.next())
                        .unwrap();
                }
                let i = source.selected.unwrap().primitive_id as usize;
                combined.merge(&source, target[i], random.next()).unwrap();
            }
            assert_eq!(combined.m, 6);
            sum += combined.normalization();
        }
        let mean = sum / runs as f64;
        assert!(
            (mean - 2.0).abs() < 0.025,
            "merged integral {mean}, expected 2"
        );
    }

    #[test]
    fn receiver_reconnection_reevaluates_target_without_double_area_jacobian() {
        let old_receiver = receiver();
        let mut new_receiver = old_receiver;
        new_receiver.position = -Vec3::Z * 2.0;
        let candidate = candidate();
        let old_target = target_luminance(candidate.evaluate(&old_receiver).unwrap()).unwrap();
        let new_target = target_luminance(candidate.evaluate(&new_receiver).unwrap()).unwrap();
        assert!((new_target * 4.0 - old_target).abs() < 1e-7);
        let area_pdf = candidate
            .area_pdf(old_receiver.position, 1.0 / f64::from(PI))
            .unwrap();
        let mut source = GiReservoir::default();
        source.update(candidate, old_target, area_pdf, 0.0).unwrap();
        let mut result = GiReservoir::default();
        result.merge(&source, new_target, 0.0).unwrap();
        assert!((result.sum_weights - new_target / area_pdf).abs() < 1e-7);
        assert_eq!(result.selected_target, new_target);
        assert!((result.normalization() - source.normalization()).abs() < 1e-7);
        assert!(
            (result.estimate(&new_receiver, true).unwrap()
                - candidate.evaluate(&new_receiver).unwrap() / area_pdf as f32)
                .abs()
                .max_element()
                < 1e-6
        );
    }

    #[test]
    fn zero_targets_count_without_selection_and_bad_numbers_do_not_mutate() {
        let mut reservoir = GiReservoir::default();
        assert!(!reservoir.update(candidate(), 0.0, 0.5, 0.2).unwrap());
        assert_eq!(reservoir.m, 1);
        assert_eq!(reservoir.normalization(), 0.0);
        reservoir.update(candidate(), 2.0, 0.5, 0.0).unwrap();
        assert_eq!(reservoir.m, 2);
        assert_eq!(reservoir.sum_weights, 4.0);
        let valid = reservoir;
        for (target, pdf, random) in [
            (f64::NAN, 0.5, 0.2),
            (-1.0, 0.5, 0.2),
            (1.0, 0.0, 0.2),
            (1.0, f64::INFINITY, 0.2),
            (1.0, 0.5, 1.0),
            (1.0, 0.5, f64::NAN),
        ] {
            assert!(reservoir.update(candidate(), target, pdf, random).is_err());
            assert_eq!(reservoir, valid);
        }
        let mut source = GiReservoir::default();
        source.update(candidate(), 1.0, 0.25, 0.0).unwrap();
        reservoir.merge(&source, 0.0, 0.5).unwrap();
        assert_eq!(reservoir.m, 3);
        assert_eq!(reservoir.sum_weights, 4.0);
        assert_eq!(reservoir.selected_target, 2.0);
    }

    #[test]
    fn visibility_is_explicit_and_history_limit_preserves_normalization() {
        let mut reservoir = GiReservoir::default();
        for _ in 0..12 {
            reservoir.update(candidate(), 2.0, 0.5, 0.0).unwrap();
        }
        let normalization = reservoir.normalization();
        assert_eq!(reservoir.estimate(&receiver(), false).unwrap(), Vec3::ZERO);
        assert!(reservoir.estimate(&receiver(), true).unwrap().x > 0.0);
        reservoir.limit_history(3).unwrap();
        assert_eq!(reservoir.m, 3);
        assert_eq!(reservoir.normalization(), normalization);
        assert!(reservoir.limit_history(0).is_err());
        let mut malformed = reservoir;
        malformed.m = u32::MAX;
        let unchanged = malformed;
        assert!(malformed.update(candidate(), 1.0, 0.5, 0.2).is_err());
        assert_eq!(malformed, unchanged);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn standalone_reservoir_wgsl_parses_and_validates_without_tracer_globals() {
        let source = include_str!("../../shaders/gi_reservoir.wgsl");
        let module = wgpu::naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn reuse_rejects_dynamic_scene_epoch_mismatch_bad_normals_and_history_reset() {
        let context = ReuseContext {
            current_scene_epoch: 2,
            source_scene_epoch: 2,
            current_frame: 8,
            source_frame: 7,
            temporal: true,
            history_valid: true,
            scene_static: true,
            current_normal: Vec3::Z,
            source_normal: Vec3::Z,
            minimum_normal_cosine: 0.9,
        };
        assert!(reuse_allowed(&context));
        for invalid in [
            ReuseContext {
                history_valid: false,
                ..context
            },
            ReuseContext {
                scene_static: false,
                ..context
            },
            ReuseContext {
                source_scene_epoch: 1,
                ..context
            },
            ReuseContext {
                source_frame: 8,
                ..context
            },
            ReuseContext {
                source_normal: Vec3::X,
                ..context
            },
            ReuseContext {
                source_normal: Vec3::splat(f32::NAN),
                ..context
            },
        ] {
            assert!(!reuse_allowed(&invalid));
        }
        assert!(reuse_allowed(&ReuseContext {
            temporal: false,
            source_frame: 8,
            ..context
        }));
    }
}
