//! Resident-field world state. Checkpoints and bounded programs are persisted; GPU handles and
//! transport are not. Consumers use explicit observations rather than the old checkpoint tiles.
use amoris_assets::water_field::{
    CheckedCheckpoint, FieldBoundary, FieldCheckpoint, FieldPhysics, FieldRecipe, FieldStep,
};
use amoris_sim::{ContentHash, EntityId};
use bevy_ecs::prelude::{Component, Resource};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResidentStats {
    pub max_depth: f64,
    pub max_speed: f64,
    pub max_pressure_jump: f64,
    pub volume_cells: f64,
    pub wet_edge_surface: f64,
    pub wet_edge_count: u32,
}
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaterResident {
    pub recipe: FieldRecipe,
    pub stats: ResidentStats,
    pub completed_tick: u64,
    pub still: u32,
    pub controls: ResidentControls,
    pub boundary: BoundaryLedger,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResidentControls {
    pub boundaries: [FieldBoundary; 4],
    pub outlet_levels: [Option<f64>; 4],
    pub cell_m: f64,
    pub friction: f64,
    pub dry_below: f64,
    pub substeps: u32,
    pub dt: f64,
    pub physics: FieldPhysics,
}
impl ResidentControls {
    pub fn from_region(r: &super::WaterRegion, dt: f64) -> Result<Self, amoris_contract::Problem> {
        let mut boundaries = [FieldBoundary::Wall; 4];
        for (i, edge) in r.edges.iter().enumerate() {
            boundaries[i] = match edge {
                super::Edge::Wall => FieldBoundary::Wall,
                super::Edge::Outflow => FieldBoundary::Outflow,
                super::Edge::Inflow { m3s_per_m: q } => FieldBoundary::Inflow { m3s_per_m: *q },
                super::Edge::Sea => FieldBoundary::Sea,
            };
        }
        Ok(Self {
            boundaries,
            outlet_levels: [None; 4],
            cell_m: r.cell_m,
            friction: r.friction,
            dry_below: r.dry_below,
            substeps: r.substeps,
            dt,
            physics: r.physics.field(),
        })
    }
    pub fn open(&self) -> bool {
        self.boundaries.iter().any(|e| *e != FieldBoundary::Wall)
    }
}
/// Optional receiving surface for each Outflow side, in world metres. Other sides ignore it.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterOutletStage {
    pub levels: [Option<f64>; 4],
}
impl amoris_sim::Persisted for WaterOutletStage {
    const NAME: &'static str = "WaterOutletStage";
    const VERSION: u32 = 1;
}
impl WaterOutletStage {
    pub fn validate(&self) -> Result<(), amoris_contract::Problem> {
        if self.levels.iter().flatten().any(|v| !v.is_finite()) {
            Err(failed("outlet levels must be finite"))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BoundaryFlux {
    pub entered: f64,
    pub left: f64,
    pub edited: f64,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BoundaryLedger {
    pub initial_volume: f64,
    pub current_volume: f64,
    pub entered_total: f64,
    pub left_total: f64,
    pub numerical_residual: f64,
    pub configuration_total: f64,
    pub edited_total: f64,
    pub edit_requested_total: f64,
    pub edit_difference_total: f64,
    pub entered: f64,
    pub left: f64,
    pub tick: u64,
}
impl BoundaryLedger {
    pub(crate) fn record(
        &mut self,
        tick: u64,
        before: f64,
        after: f64,
        flow: BoundaryFlux,
        requested_edit: f64,
    ) -> Result<(), amoris_contract::Problem> {
        if [before, after, flow.entered, flow.left]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(failed("invalid boundary volume receipt"));
        }
        if !flow.edited.is_finite() || !requested_edit.is_finite() {
            return Err(failed("invalid local volume edit receipt"));
        }
        self.configuration_total += before - self.current_volume;
        self.numerical_residual += (after - before) - flow.entered + flow.left - flow.edited;
        self.edited_total += flow.edited;
        self.edit_requested_total += requested_edit;
        self.edit_difference_total += flow.edited - requested_edit;
        self.entered_total += flow.entered;
        self.left_total += flow.left;
        self.current_volume = after;
        self.entered = flow.entered;
        self.left = flow.left;
        self.tick = tick;
        if [
            self.configuration_total,
            self.numerical_residual,
            self.entered_total,
            self.left_total,
            self.edited_total,
            self.edit_requested_total,
            self.edit_difference_total,
        ]
        .iter()
        .any(|v| !v.is_finite())
        {
            return Err(failed("boundary volume ledger overflowed"));
        }
        Ok(())
    }
}
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct WaterCheckpoints {
    pub fields: BTreeMap<ContentHash, Arc<CheckedCheckpoint>>,
}

#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaterProbe {
    pub region: Option<EntityId>,
    pub at: [f64; 2],
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub tick: u64,
    #[serde(default)]
    pub surface: f64,
    #[serde(default)]
    pub depth: f64,
    #[serde(default)]
    pub velocity: [f64; 2],
    #[serde(default)]
    pub sampled_at: [f64; 2],
    #[serde(default)]
    pub state_digest: [u8; 32],
    #[serde(default)]
    pub sampled_region: Option<EntityId>,
    #[serde(default)]
    pub sampled_grid: [f64; 2],
    #[serde(default)]
    pub sampled_checkpoint: [u8; 32],
}
impl WaterProbe {
    /// The exact request and field/header identity behind a reusable observation. A digest of
    /// raw relative columns alone cannot distinguish two regions or a translated sampling grid.
    pub fn matches(
        &self,
        region: EntityId,
        at: [f64; 2],
        grid: [f64; 2],
        recipe: &FieldRecipe,
    ) -> bool {
        self.ready
            && self.region == Some(region)
            && self.sampled_region == Some(region)
            && self.sampled_at == at
            && self.sampled_grid == grid
            && self.state_digest == recipe.digest
            && self.sampled_checkpoint == recipe.checkpoint().0
    }
}
pub struct ResidentRequest<'a> {
    /// Cache selection only; the host must still authenticate the complete input state.
    pub region: EntityId,
    pub checkpoint: Arc<CheckedCheckpoint>,
    pub recipe: &'a FieldRecipe,
    pub step: Option<&'a FieldStep>,
    pub points: &'a [[f64; 2]],
    pub checkpoint_now: bool,
    pub coupled_from: Option<u32>,
}
/// One raw resident point observation: world free surface, depth, centred vx and vz, then the
/// free surface's rise per column along x and z, and net inflow (m^2/s). Consumers divide by
/// the region's cell size (`super::column`).
pub type RawSample = [f64; 8];
#[derive(Debug)]
pub struct ResidentOutput {
    pub recipe: FieldRecipe,
    pub stats: ResidentStats,
    pub samples: Vec<RawSample>,
    pub checkpoint: Option<Arc<CheckedCheckpoint>>,
    pub reaction: super::feedback::ReactionBudget,
    pub boundary: BoundaryFlux,
}

#[derive(Debug)]
pub struct ResidentResult {
    pub region: EntityId,
    pub output: ResidentOutput,
}

pub(crate) fn validate_batch(
    requests: &[ResidentRequest<'_>],
) -> Result<(), amoris_contract::Problem> {
    use amoris_assets::water_field::{
        MAX_BATCH_COLUMNS, MAX_BATCH_REGIONS, MAX_SAMPLES, MAX_STEPS,
    };
    let mut columns = 0usize;
    let mut regions = std::collections::BTreeSet::new();
    if requests.is_empty() || requests.len() > MAX_BATCH_REGIONS {
        return Err(failed("resident batch region count is outside its bounds"));
    }
    for r in requests {
        if r.points.len() > MAX_SAMPLES
            || r.coupled_from.is_some_and(|n| n as usize > r.points.len())
            || r.recipe.steps().len() > MAX_STEPS
            || r.step.is_some_and(|s| s.impulses.len() > MAX_SAMPLES)
        {
            return Err(failed(
                "resident batch samples or replay exceed their bounds",
            ));
        }
        let [nx, nz] = r.checkpoint.size.map(|n| n as usize);
        columns = columns
            .checked_add(
                nx.checked_mul(nz)
                    .ok_or_else(|| failed("resident batch columns overflow"))?,
            )
            .ok_or_else(|| failed("resident batch columns overflow"))?;
        if columns > MAX_BATCH_COLUMNS || !regions.insert(r.region) {
            return Err(failed(
                "resident batch exceeds its column budget or repeats a region",
            ));
        }
    }
    Ok(())
}
pub(crate) fn validate_results(
    requests: &[ResidentRequest<'_>],
    results: &[ResidentResult],
) -> Result<(), amoris_contract::Problem> {
    if requests.len() != results.len()
        || requests
            .iter()
            .zip(results)
            .any(|(r, out)| r.region != out.region)
    {
        return Err(failed(
            "resident batch results do not match the requested region order",
        ));
    }
    Ok(())
}

pub(crate) struct ResidentPlan {
    pub entity: bevy_ecs::entity::Entity,
    pub region: EntityId,
    pub controls: ResidentControls,
    pub area: f64,
    pub local_ready: bool,
    pub feedback: Option<super::feedback::WaterFeedback>,
    pub displacement: Option<super::displacement::WaterDisplacement>,
    pub invalid_probes: Vec<bevy_ecs::entity::Entity>,
    pub state: WaterResident,
    pub checkpoint: Arc<CheckedCheckpoint>,
    pub step: Option<FieldStep>,
    pub points: Vec<[f64; 2]>,
    pub probes: Vec<(bevy_ecs::entity::Entity, [f64; 2])>,
    /// All coupled-capable body points: Floater indices and the three distinct Hull roles.
    pub buoys: Vec<super::buoyancy::Point>,
    pub swimmers: Vec<super::swimming::Point>,
}
impl ResidentPlan {
    pub fn request(&self) -> ResidentRequest<'_> {
        ResidentRequest {
            region: self.region,
            checkpoint: self.checkpoint.clone(),
            recipe: &self.state.recipe,
            step: self.step.as_ref(),
            points: &self.points,
            checkpoint_now: false,
            coupled_from: self.feedback.as_ref().map(|_| {
                u32::try_from(self.probes.len() + self.swimmers.len()).expect("bounded point count")
            }),
        }
    }
    /// Mutate only the owned plan. Every plan in the cohort must pass before world writes.
    pub fn finish(
        &mut self,
        output: Option<&ResidentOutput>,
        tick: u64,
    ) -> Result<(), amoris_contract::Problem> {
        let before = self.state.stats.volume_cells * self.area;
        let reaction = if let Some(output) = output {
            if output.samples.len() != self.points.len()
                || output
                    .samples
                    .iter()
                    .any(|p| p[1] < 0.0 || p.iter().any(|v| !v.is_finite()))
                || !output.stats.wet_edge_surface.is_finite()
                || output.stats.wet_edge_count
                    > 2 * (self.checkpoint.size[0] + self.checkpoint.size[1]) - 4
                || [
                    output.stats.max_depth,
                    output.stats.max_speed,
                    output.stats.max_pressure_jump,
                    output.stats.volume_cells,
                ]
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
            {
                return Err(failed("resident compute output is invalid"));
            }
            let mut expected = self.state.recipe.clone();
            if let Some(step) = &self.step {
                expected.push_step(step.clone());
            }
            if output.recipe.environment.is_empty()
                || (!self.state.recipe.environment.is_empty()
                    && output.recipe.environment != self.state.recipe.environment)
                || (output.checkpoint.is_none()
                    && output.recipe.program_key() != expected.program_key())
                || (self.step.is_none() && output.recipe.digest != self.state.recipe.digest)
            {
                return Err(failed(
                    "resident output does not describe the requested transition",
                ));
            }
            if let Some(checkpoint) = &output.checkpoint {
                let (key, digest) = checkpoint.identity();
                if key != output.recipe.checkpoint()
                    || digest != output.recipe.digest
                    || checkpoint.size != self.checkpoint.size
                    || checkpoint.datum.to_bits() != self.checkpoint.datum.to_bits()
                    || !output.recipe.steps().is_empty()
                {
                    return Err(failed("resident checkpoint digest mismatch"));
                }
            }
            if self.step.is_none() && output.boundary != BoundaryFlux::default() {
                return Err(failed("an observation cannot return boundary flux"));
            }
            self.state.boundary.record(
                tick,
                before,
                output.stats.volume_cells * self.area,
                output.boundary,
                self.step
                    .as_ref()
                    .map_or(0.0, |s| s.edits.iter().map(|e| e.depth_m * self.area).sum()),
            )?;
            self.state.controls = self.controls.clone();
            self.state.recipe = output.recipe.clone();
            self.state.stats = output.stats;
            self.state.still = if self.state.stats.max_speed < super::STILL_SPEED
                && self.state.stats.max_pressure_jump < super::STILL_DEPTH
            {
                if self.step.as_ref().is_some_and(|s| {
                    !s.edits.is_empty() || !s.impulses.is_empty() || !s.displacements.is_empty()
                }) {
                    1
                } else {
                    self.state.still.saturating_add(1).min(super::SLEEP_TICKS)
                }
            } else {
                0
            };
            output.reaction
        } else {
            debug_assert!(self.local_ready);
            self.state
                .boundary
                .record(tick, before, before, BoundaryFlux::default(), 0.0)?;
            super::feedback::ReactionBudget::default()
        };
        self.state.completed_tick = tick;
        if let Some(feedback) = &mut self.feedback {
            feedback.complete(tick, reaction)?;
        }
        if let Some(displacement) = &mut self.displacement {
            displacement.complete(tick)?;
        }
        Ok(())
    }
}

pub(crate) fn initial(
    r: &super::WaterRegion,
    checkpoints: &mut WaterCheckpoints,
) -> Result<WaterResident, amoris_contract::Problem> {
    let checkpoint = seed(r)?;
    let (key, digest) = checkpoint.identity();
    let mut stats = ResidentStats::default();
    for c in &checkpoint.columns {
        stats.volume_cells += f64::from(f32::from_bits(c[1]));
        stats.max_depth = amoris_sim::math::max(stats.max_depth, f64::from(f32::from_bits(c[1])));
        stats.max_speed = amoris_sim::math::max(
            stats.max_speed,
            amoris_sim::math::max(
                f64::from(f32::from_bits(c[2])).abs(),
                f64::from(f32::from_bits(c[3])).abs(),
            ),
        );
    }
    let volume = stats.volume_cells * r.cell_m * r.cell_m;
    checkpoints.fields.entry(key).or_insert(checkpoint);
    Ok(WaterResident {
        recipe: FieldRecipe::new(key, vec![], digest, String::new()),
        stats,
        completed_tick: 0,
        still: 0,
        controls: ResidentControls::from_region(r, 0.0)?,
        boundary: BoundaryLedger {
            initial_volume: volume,
            current_volume: volume,
            ..Default::default()
        },
    })
}

impl amoris_sim::Persisted for WaterResident {
    const NAME: &'static str = "WaterResident";
    const VERSION: u32 = 8;
    fn trace(
        t: &mut serde_reflection::Tracer,
        s: &serde_reflection::Samples,
    ) -> serde_reflection::Result<()> {
        t.trace_type::<FieldBoundary>(s)?;
        t.trace_type::<amoris_assets::water_field::FieldSea>(s)?;
        t.trace_type::<FieldPhysics>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}
impl amoris_sim::Persisted for WaterProbe {
    const NAME: &'static str = "WaterProbe";
    const VERSION: u32 = 2;
}
impl amoris_sim::Persisted for WaterCheckpoints {
    const NAME: &'static str = "WaterCheckpoints";
    const VERSION: u32 = 2;
    fn trace(
        t: &mut serde_reflection::Tracer,
        _: &serde_reflection::Samples,
    ) -> serde_reflection::Result<()> {
        // The checked decoder rejects the tracer's synthetic zero-size grid. Supply a valid
        // payload sample, exactly as other constrained persisted values do; caches are not wire fields.
        let mut samples = serde_reflection::Samples::new();
        t.trace_value(
            &mut samples,
            &FieldCheckpoint::new([2, 2], 0.0, vec![[0; 4]; 4]),
        )?;
        t.trace_type::<Self>(&samples).map(|_| ())
    }
}

#[allow(clippy::cast_possible_truncation)] // The checkpoint codec quantizes the explicit f32 mode.
pub(crate) fn seed(
    r: &super::WaterRegion,
) -> Result<Arc<CheckedCheckpoint>, amoris_contract::Problem> {
    let [nx, nz] = r.size.map(|n| n as usize);
    if !r.well_formed() {
        return Err(failed(
            "resident initialization needs a complete starting grid",
        ));
    }
    let datum = r
        .tiles
        .iter()
        .flat_map(|t| t.bed.iter().copied())
        .fold(f64::INFINITY, amoris_sim::math::min);
    let mut columns = Vec::with_capacity(nx * nz);
    for k in 0..nz {
        for i in 0..nx {
            let (tile, c) = r.locate(i, k);
            let t = &r.tiles[tile];
            columns.push(
                [
                    (t.bed[c] - datum) as f32,
                    t.depth[c] as f32,
                    t.u[c] as f32,
                    t.w[c] as f32,
                ]
                .map(f32::to_bits),
            );
        }
    }
    let mut checkpoint = FieldCheckpoint::new(r.size, datum, columns);
    // Time-zero boundary velocities are part of the initial physical state too. Outside
    // positive faces have no authored tile slot, so initialize them from the declared control.
    for side in 0..4 {
        let count = if side < 2 { nz } else { nx };
        for along in 0..count {
            let (x, z) = match side {
                0 => (0, along),
                1 => (nx - 1, along),
                2 => (along, 0),
                _ => (along, nz - 1),
            };
            let axis = usize::from(side >= 2);
            let n = x + nx * z;
            let sign = if side % 2 == 0 { -1.0 } else { 1.0 };
            let near = match side {
                0 => n + 1,
                1 => n - 1,
                2 => n + nx,
                _ => n - nx,
            };
            let h = f64::from(f32::from_bits(checkpoint.columns[n][1]));
            let velocity = match r.edges[side] {
                super::Edge::Wall | super::Edge::Sea => 0.0,
                super::Edge::Inflow { m3s_per_m: q } => {
                    if q == 0.0 {
                        0.0
                    } else {
                        -sign * q
                            / amoris_sim::math::max(
                                h,
                                amoris_sim::math::pow(q * q / 9.81, 1.0 / 3.0),
                            )
                    }
                }
                super::Edge::Outflow => {
                    sign * amoris_sim::math::max(
                        sign * f64::from(f32::from_bits(checkpoint.columns[near][axis + 2])),
                        0.0,
                    )
                }
            } as f32;
            match side {
                0 | 2 => checkpoint.columns[n][axis + 2] = velocity.to_bits(),
                1 => checkpoint.far_faces[along] = velocity.to_bits(),
                _ => checkpoint.far_faces[nz + along] = velocity.to_bits(),
            }
        }
    }
    Ok(Arc::new(checkpoint.checked().map_err(failed)?))
}
pub(crate) fn failed(message: impl Into<String>) -> amoris_contract::Problem {
    amoris_contract::Problem::new(
        "water.resident_failed",
        message.into(),
        amoris_contract::detail([]),
    )
}
