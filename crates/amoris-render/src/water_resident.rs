//! Reconstructible GPU execution. The caller owns its checkpoint/program; this cache may be
//! discarded and must reproduce the stored digest before advancing a restored branch.
use crate::{
    Gpu,
    water_compute::{BoundaryReport, GpuWater},
    water_edits::WaterEdits,
    water_reactions::{ReactionReport, WaterReactions},
    water_samples::{SAMPLE_WORDS, SampleStamp, WaterSamples},
    water_summary::{WaterStateSummary, WaterSummary},
};
use amoris_assets::water_field::{
    CheckedCheckpoint, FieldCheckpoint, FieldRecipe, FieldStep, MAX_BATCH_COLUMNS,
    MAX_BATCH_REGIONS, MAX_SAMPLES, MAX_STEPS,
};
use std::sync::Arc;

/// Numerical source compatibility shared by the authoritative service and visual replicas.
/// Device/backend compatibility remains part of the complete authoritative environment string.
pub fn kernel_key() -> &'static str {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        amoris_assets::water_field::execution_key(&[
            include_bytes!("../shaders/water_compute.wgsl"),
            include_bytes!("../shaders/water_depth_first_order.wgsl"),
            include_bytes!("../shaders/water_maccormack.wgsl"),
            include_bytes!("../shaders/water_boundaries.wgsl"),
            include_bytes!("../shaders/water_edits.wgsl"),
            include_bytes!("../shaders/water_reactions.wgsl"),
            include_bytes!("../shaders/water_point.wgsl"),
            include_bytes!("../shaders/water_samples.wgsl"),
            include_bytes!("../shaders/water_summary.wgsl"),
            include_bytes!("../shaders/water_waves.wgsl"),
            include_bytes!("../shaders/water_fft.wgsl"),
            include_bytes!("../shaders/water_occupancy.wgsl"),
            include_bytes!("../shaders/water_nh.wgsl"),
            include_bytes!("water_compute.rs"),
            include_bytes!("water_waves.rs"),
            include_bytes!("water_occupancy.rs"),
            include_bytes!("water_nh.rs"),
            include_bytes!("water_edits.rs"),
            include_bytes!("water_reactions.rs"),
            include_bytes!("water_samples.rs"),
            include_bytes!("water_summary.rs"),
            include_bytes!("water_resident.rs"),
            include_bytes!("shaders.rs"),
            include_bytes!("gpu.rs"),
            include_bytes!("../../../Cargo.lock"),
            include_bytes!("../../amoris-assets/src/water_field.rs"),
        ])
    })
    .as_str()
}

pub(crate) fn validate_recipe_kernel(recipe: &FieldRecipe) -> Result<(), String> {
    // A bare checkpoint contains exact words and needs no numerical reconstruction.
    if recipe.steps().is_empty() {
        return Ok(());
    }
    if recipe
        .environment
        .rsplit_once("; kernels ")
        .map(|(_, key)| key)
        != Some(kernel_key())
    {
        return Err(
            "resident water numerical kernels differ: rebuild the game and viewport together"
                .into(),
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transfers {
    pub uploads: u64,
    pub field_readbacks: u64,
    pub small_readbacks: u64,
    pub readback_bytes: u64,
}
impl Transfers {
    fn include(&mut self, other: Self) {
        self.uploads += other.uploads;
        self.field_readbacks += other.field_readbacks;
        self.small_readbacks += other.small_readbacks;
        self.readback_bytes += other.readback_bytes;
    }
}

/// Bounded, size-aware per-region working set. IDs select a disposable slot, never authenticate
/// it: every operation still verifies checkpoint, program, digest and execution environment.
struct Slot {
    region: u64,
    columns: u64,
    water: ResidentWater,
}
#[derive(Default)]
pub struct ResidentPool {
    slots: Vec<Slot>,
    retired: Transfers,
    evictions: u64,
    resizes: u64,
    batches: ResidentBatchDiagnostics,
}
impl ResidentPool {
    /// Four largest supported fields, rather than four fields of any size.
    pub const CAPACITY: usize = 4;
    pub const COLUMN_BUDGET: u64 = MAX_BATCH_COLUMNS as u64;
    /// Bounds per-field pipelines and fixed metadata even for very small grids.
    pub const MAX_REGIONS: usize = MAX_BATCH_REGIONS;
    pub fn get(
        &mut self,
        gpu: &Gpu,
        region: u64,
        size: [u32; 2],
    ) -> Result<&mut ResidentWater, String> {
        self.get_pinned(gpu, region, size, &[])
    }
    fn get_pinned(
        &mut self,
        gpu: &Gpu,
        region: u64,
        size: [u32; 2],
        pinned: &[u64],
    ) -> Result<&mut ResidentWater, String> {
        if size.iter().any(|n| !(2..=512).contains(n)) {
            return Err("GPU water window dimensions must be 2..=512".into());
        }
        let columns = u64::from(size[0]) * u64::from(size[1]);
        let previous = self.slots.iter().position(|s| s.region == region);
        if previous.is_none() && self.recycle_eviction(gpu, region, columns, pinned) {
            return Ok(&mut self.slots.last_mut().expect("recycled resident slot").water);
        }
        if let Some(index) = previous
            && columns <= self.slots[index].columns
        {
            self.slots[index..].rotate_left(1);
        } else {
            // Construct first: a failed allocation leaves every existing cache slot intact.
            let water = ResidentWater::with_size(gpu, size)?;
            if let Some(index) = previous {
                let old = self.slots.remove(index);
                self.retired.include(old.water.transfers());
                self.resizes += 1;
            }
            while self.slots.len() >= Self::MAX_REGIONS
                || self.columns() + columns > Self::COLUMN_BUDGET
            {
                let index = self
                    .slots
                    .iter()
                    .position(|s| !pinned.contains(&s.region))
                    .ok_or("resident batch exceeds its pinned working set")?;
                let old = self.slots.remove(index);
                self.retired.include(old.water.transfers());
                self.evictions += 1;
            }
            self.slots.push(Slot {
                region,
                columns,
                water,
            });
        }
        Ok(&mut self.slots.last_mut().expect("resident cache slot").water)
    }
    /// Reuse only the slot the existing eviction policy would remove, with exactly the
    /// incoming capacity. No spare allocation or changed retained-capacity prediction is needed.
    fn recycle_eviction(&mut self, gpu: &Gpu, region: u64, columns: u64, pinned: &[u64]) -> bool {
        if self.slots.len() < Self::MAX_REGIONS && self.columns() + columns <= Self::COLUMN_BUDGET {
            return false;
        }
        let Some(index) = self.slots.iter().position(|s| !pinned.contains(&s.region)) else {
            return false;
        };
        let victim = &self.slots[index];
        if victim.columns != columns
            || victim.water.gpu.device != gpu.device
            || victim.water.gpu.queue != gpu.queue
        {
            return false;
        }
        let mut slot = self.slots.remove(index);
        self.retired
            .include(std::mem::take(&mut slot.water.transfers));
        slot.water.reset_identity();
        slot.region = region;
        self.slots.push(slot);
        self.evictions += 1;
        true
    }
    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
    /// Reserved columns include retained capacity after a smaller logical window is restored.
    pub fn columns(&self) -> u64 {
        self.slots.iter().map(|s| s.columns).sum()
    }
    pub fn evictions(&self) -> u64 {
        self.evictions
    }
    pub fn resizes(&self) -> u64 {
        self.resizes
    }
    pub fn transfers(&self) -> Transfers {
        let mut total = self.retired;
        for slot in &self.slots {
            total.include(slot.water.transfers());
        }
        total
    }
    pub fn environment(&self) -> Option<&str> {
        self.slots.last().map(|s| s.water.environment())
    }
}
/// Independent regions at one simulation boundary. Inputs remain borrowed until the complete
/// cohort has finished; outputs preserve this order, including observe-only requests.
pub struct ResidentBatchRequest<'a> {
    pub region: u64,
    pub checkpoint: &'a Arc<CheckedCheckpoint>,
    pub recipe: &'a FieldRecipe,
    pub step: Option<&'a FieldStep>,
    pub points: &'a [[f32; 2]],
    pub checkpoint_now: bool,
    pub coupled_from: Option<u32>,
}

/// Scratch diagnostics count aggregated map phases, rather than per-region transfers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResidentBatchDiagnostics {
    pub sync_groups: u64,
    pub step_groups: u64,
    pub checkpoint_groups: u64,
    pub max_group_regions: usize,
}

#[derive(Clone, Copy)]
enum ReadMode {
    Async,
    #[cfg(not(target_arch = "wasm32"))]
    Blocking,
}
impl ReadMode {
    async fn read<T: bytemuck::Pod>(
        self,
        gpu: &Gpu,
        buffers: &[wgpu::Buffer],
        output: &mut [Vec<T>],
    ) -> Result<(), String> {
        let mut maps: Vec<_> = buffers
            .iter()
            .zip(output.iter_mut())
            .map(|(buffer, words)| (buffer, words.as_mut_slice()))
            .collect();
        let _ = gpu; // WebGPU drives callbacks through its event loop.
        match self {
            Self::Async => crate::readback::read_many(&mut maps).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Blocking => crate::readback::read_many_blocking(&gpu.device, &mut maps),
        }
    }
}

/// Invalidate every participating cache on failure or future cancellation, including groups
/// already completed. A suspended execute_batch_async never leaves an authenticated partial tick.
struct BatchExecution<'a> {
    pool: &'a mut ResidentPool,
    regions: Vec<u64>,
    completed: bool,
}
impl Drop for BatchExecution<'_> {
    fn drop(&mut self) {
        if !self.completed {
            for slot in &mut self.pool.slots {
                if self.regions.contains(&slot.region) {
                    slot.water.valid = false;
                    slot.water.summary.discard();
                    slot.water.samples.clear();
                }
            }
        }
    }
}

impl ResidentPool {
    pub fn batch_diagnostics(&self) -> ResidentBatchDiagnostics {
        self.batches
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute_batch(
        &mut self,
        gpu: &Gpu,
        requests: &[ResidentBatchRequest<'_>],
    ) -> Result<Vec<ResidentAdvance>, String> {
        pollster::block_on(self.execute_batch_with(gpu, requests, ReadMode::Blocking))
    }

    pub async fn execute_batch_async(
        &mut self,
        gpu: &Gpu,
        requests: &[ResidentBatchRequest<'_>],
    ) -> Result<Vec<ResidentAdvance>, String> {
        self.execute_batch_with(gpu, requests, ReadMode::Async)
            .await
    }

    async fn execute_batch_with(
        &mut self,
        gpu: &Gpu,
        requests: &[ResidentBatchRequest<'_>],
        mode: ReadMode,
    ) -> Result<Vec<ResidentAdvance>, String> {
        Self::validate_batch(requests)?;
        let mut execution = BatchExecution {
            pool: self,
            regions: requests.iter().map(|r| r.region).collect(),
            completed: false,
        };
        let mut output = Vec::with_capacity(requests.len());
        let mut start = 0;
        while start < requests.len() {
            // A smaller logical restore keeps its existing allocation. Group by the actual
            // retained capacity, then pin the selected slots until every map has finished.
            let mut columns = 0;
            let mut end = start;
            while end < requests.len() && end - start < Self::MAX_REGIONS {
                let request = &requests[end];
                let logical =
                    u64::from(request.checkpoint.size[0]) * u64::from(request.checkpoint.size[1]);
                let retained = execution
                    .pool
                    .slots
                    .iter()
                    .find(|s| s.region == request.region)
                    .map_or(logical, |s| s.columns.max(logical));
                if columns + retained > Self::COLUMN_BUDGET {
                    break;
                }
                columns += retained;
                end += 1;
            }
            if end == start {
                return Err("resident batch region exceeds its working set budget".into());
            }
            output.extend(
                execution
                    .pool
                    .execute_group(gpu, &requests[start..end], mode)
                    .await?,
            );
            start = end;
        }
        execution.completed = true;
        Ok(output)
    }

    fn validate_batch(requests: &[ResidentBatchRequest<'_>]) -> Result<(), String> {
        if requests.is_empty() || requests.len() > MAX_BATCH_REGIONS {
            return Err(format!(
                "resident batch must contain 1..={MAX_BATCH_REGIONS} regions"
            ));
        }
        let mut columns = 0usize;
        for (index, request) in requests.iter().enumerate() {
            if requests[..index].iter().any(|r| r.region == request.region) {
                return Err("resident batch contains a duplicate region".into());
            }
            let size = request.checkpoint.size;
            if size.iter().any(|n| !(2..=512).contains(n)) {
                return Err("GPU water window dimensions must be 2..=512".into());
            }
            columns += size[0] as usize * size[1] as usize;
            if columns > MAX_BATCH_COLUMNS {
                return Err("resident batch exceeds its column budget".into());
            }
            request.recipe.validate_checked(request.checkpoint)?;
            if request.points.len() > MAX_SAMPLES
                || request.points.iter().flatten().any(|v| !v.is_finite())
                || request
                    .coupled_from
                    .is_some_and(|n| n as usize > request.points.len())
            {
                return Err(format!(
                    "resident queries must be finite and fit {MAX_SAMPLES} points"
                ));
            }
            if let Some(step) = request.step {
                if request.recipe.steps().len() >= MAX_STEPS {
                    return Err("compact the bounded water program before advancing it".into());
                }
                step.validate(size[0] as usize * size[1] as usize)?;
                if step.dry_below != 0.0 {
                    return Err("resident drying has not been connected; use dry_below=0".into());
                }
            }
        }
        Ok(())
    }

    async fn execute_group(
        &mut self,
        gpu: &Gpu,
        requests: &[ResidentBatchRequest<'_>],
        mode: ReadMode,
    ) -> Result<Vec<ResidentAdvance>, String> {
        // Protect future hits as well as slots already acquired: an earlier miss must not
        // evict another member of this selected group and cascade through the working set.
        let pinned: Vec<_> = requests.iter().map(|r| r.region).collect();
        for request in requests {
            self.get_pinned(gpu, request.region, request.checkpoint.size, &pinned)?;
        }
        let indices: Vec<_> = requests
            .iter()
            .map(|r| {
                self.slots
                    .iter()
                    .position(|s| s.region == r.region)
                    .expect("pinned resident slot")
            })
            .collect();
        self.batches.max_group_regions = self.batches.max_group_regions.max(requests.len());

        let mut synchronize = Vec::new();
        for (request, &index) in requests.iter().zip(&indices) {
            let water = &mut self.slots[index].water;
            water.set_buoyancy_start(request.coupled_from);
            // prepare_sync submits EACH historical step before its next queue.write_buffer.
            // Reused per-region params/edit/reaction buffers forbid merging that history.
            if water.prepare_sync(request.checkpoint, request.recipe)? {
                synchronize.push(index);
            }
        }
        if !synchronize.is_empty() {
            let buffers: Vec<_> = synchronize
                .iter()
                .map(|&i| self.slots[i].water.staging.clone())
                .collect();
            let mut words = vec![vec![0u32; 16]; synchronize.len()];
            self.batches.sync_groups += 1;
            mode.read(gpu, &buffers, &mut words).await?;
            for (&index, words) in synchronize.iter().zip(&words) {
                let request = &requests[indices
                    .iter()
                    .position(|&i| i == index)
                    .expect("sync request")];
                self.slots[index].water.accept_sync(words, request.recipe)?;
            }
        }

        // All regions have independently authenticated their old digest before any current step.
        // Their buffers are independent, so current steps/observations share one submission.
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let mut words = Vec::with_capacity(requests.len());
        for (request, &index) in requests.iter().zip(&indices) {
            let water = &mut self.slots[index].water;
            if let Some(step) = request.step {
                water.encode_advance(&mut encoder, step, request.points)?;
            } else {
                water.encode_observe(&mut encoder, request.points)?;
            }
            words.push(vec![
                0u32;
                readout_words(request.step, request.points.len())
            ]);
        }
        gpu.queue.submit([encoder.finish()]);
        let buffers: Vec<_> = indices
            .iter()
            .map(|&i| self.slots[i].water.staging.clone())
            .collect();
        self.batches.step_groups += 1;
        mode.read(gpu, &buffers, &mut words).await?;
        let mut output = Vec::with_capacity(requests.len());
        for ((request, &index), words) in requests.iter().zip(&indices).zip(&words) {
            let water = &mut self.slots[index].water;
            output.push(if let Some(step) = request.step {
                water.accept_advance(words, request.recipe, step)?
            } else {
                water.accept_observe(words, request.recipe)?
            });
        }

        let due: Vec<_> = requests
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                (r.checkpoint_now
                    || (r.step.is_some() && output[i].recipe.steps().len() == MAX_STEPS))
                    .then_some(i)
            })
            .collect();
        if !due.is_empty() {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            let mut packed = Vec::with_capacity(due.len());
            let mut buffers = Vec::with_capacity(due.len());
            for &i in &due {
                let water = &mut self.slots[indices[i]].water;
                water.valid = false;
                water.field.encode_checkpoint_copy(&mut encoder, true)?;
                packed.push(vec![[0u32; 4]; water.field.state_words() as usize / 4]);
                buffers.push(water.field.checkpoint_buffer().clone());
            }
            gpu.queue.submit([encoder.finish()]);
            self.batches.checkpoint_groups += 1;
            mode.read(gpu, &buffers, &mut packed).await?;
            for (&i, packed) in due.iter().zip(packed) {
                let water = &mut self.slots[indices[i]].water;
                let datum = water
                    .seed
                    .as_ref()
                    .ok_or("resident seed is unavailable")?
                    .datum;
                let checkpoint = water.field.unpack_checkpoint(datum, packed);
                // Each payload independently runs complete CPU validation and authenticates its
                // digest before a new recipe/checkpoint is exposed to the game.
                water.compact(checkpoint, &mut output[i])?;
            }
        }
        Ok(output)
    }
}

fn readout_words(step: Option<&FieldStep>, points: usize) -> usize {
    16 + points * SAMPLE_WORDS
        + step.map_or(0, |s| {
            (if s.impulses.is_empty() { 0 } else { 8 })
                + if s
                    .boundaries
                    .iter()
                    .any(|e| *e != amoris_assets::water_field::FieldBoundary::Wall)
                {
                    4
                } else {
                    0
                }
                + if s.edits.is_empty() { 0 } else { 4 }
        })
}

#[derive(Debug)]
pub struct ResidentAdvance {
    pub recipe: FieldRecipe,
    pub summary: WaterStateSummary,
    /// Relative free surface, depth, centred velocity, then surface rise per column along x/z
    /// and net inflow (m^2/s), in query order.
    pub samples: Vec<[f32; SAMPLE_WORDS]>,
    pub checkpoint: Option<Arc<CheckedCheckpoint>>,
    pub reaction: ReactionReport,
    pub boundary: BoundaryReport,
}

struct Readout {
    summary: WaterStateSummary,
    samples: Vec<[f32; SAMPLE_WORDS]>,
    reaction: ReactionReport,
    boundary: BoundaryReport,
}

pub struct ResidentWater {
    gpu: Gpu,
    field: GpuWater,
    summary: WaterSummary,
    edits: WaterEdits,
    reactions: Option<WaterReactions>,
    samples: WaterSamples,
    staging: wgpu::Buffer,
    seed: Option<Arc<CheckedCheckpoint>>,
    seed_key: Option<[u8; 32]>,
    applied: usize,
    program: Option<[u8; 32]>,
    latest: Option<WaterStateSummary>,
    valid: bool,
    environment: String,
    transfers: Transfers,
    body_start: u32,
}
impl ResidentWater {
    pub fn new(gpu: &Gpu) -> Result<Self, String> {
        Self::with_size(gpu, [512, 512])
    }
    fn with_size(gpu: &Gpu, size: [u32; 2]) -> Result<Self, String> {
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resident water metadata and samples"),
            size: (64 + MAX_SAMPLES * SAMPLE_WORDS * 4 + 32 + 16 + 16) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            gpu: gpu.clone(),
            field: GpuWater::new(&gpu.device, &gpu.queue, size)?,
            summary: WaterSummary::new(&gpu.device, &gpu.queue),
            edits: WaterEdits::new(&gpu.device, &gpu.queue),
            reactions: None,
            samples: WaterSamples::new(&gpu.device, &gpu.queue, MAX_SAMPLES)?,
            staging,
            seed: None,
            seed_key: None,
            applied: 0,
            program: None,
            latest: None,
            valid: false,
            environment: format!(
                "{} {:?}; {}; {}; features {:?}; kernels {}",
                gpu.info.name,
                gpu.info.backend,
                gpu.info.driver,
                gpu.info.driver_info,
                gpu.device.features(),
                kernel_key()
            ),
            transfers: Transfers::default(),
            body_start: u32::MAX,
        })
    }
    /// Transports the kernel's face velocities by another scheme than the regions' (water.md
    /// 61.1), to compare them. The execution environment names it, so no replay mixes schemes.
    pub fn set_advection(&mut self, advection: crate::water_compute::Advection) {
        self.field.set_advection(advection);
        let base = self
            .environment
            .split("; advection ")
            .next()
            .unwrap_or_default();
        self.environment = if advection == Default::default() {
            base.to_string()
        } else {
            format!("{base}; advection {advection:?}")
        };
    }
    /// Retain GPU allocations, but require a complete upload and authenticated replay before
    /// exposing another region. Outstanding batch maps have completed or been cancelled first.
    fn reset_identity(&mut self) {
        self.seed = None;
        self.seed_key = None;
        self.program = None;
        self.latest = None;
        self.applied = 0;
        self.valid = false;
        self.body_start = u32::MAX;
        self.summary.discard();
        self.samples.clear();
    }
    pub fn transfers(&self) -> Transfers {
        self.transfers
    }
    /// Only feedback-coupled body queries use the same wet-face stencil as reactions.
    /// Explicit probes and ordinary one-way Floaters retain their original velocity definition.
    pub fn set_buoyancy_start(&mut self, start: Option<u32>) {
        self.body_start = start.unwrap_or(u32::MAX);
    }
    pub fn environment(&self) -> &str {
        &self.environment
    }
    pub fn field(&self) -> Result<&GpuWater, String> {
        if self.valid {
            Ok(&self.field)
        } else {
            Err("resident field has no verified state".into())
        }
    }

    fn encode_step(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        step: &FieldStep,
    ) -> Result<(), String> {
        step.validate((self.field.size()[0] * self.field.size()[1]) as usize)?;
        if step.dry_below != 0.0 {
            return Err("resident drying has not been connected; use dry_below=0".into());
        }
        if !step.impulses.is_empty() {
            self.reactions
                .get_or_insert_with(|| WaterReactions::new(&self.gpu.device, &self.gpu.queue))
                .encode(encoder, &self.field, step.cell_m, &step.impulses)?;
        }
        self.edits.encode(encoder, &self.field, &step.edits)?;
        self.field.encode_transport(encoder, step, None)
    }
    fn encode_readout(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        points: &[[f32; 2]],
        reaction: bool,
        boundary: bool,
        edited: bool,
    ) -> Result<(), String> {
        if points.len() > MAX_SAMPLES || points.iter().flatten().any(|v| !v.is_finite()) {
            return Err(format!(
                "resident queries must be finite and fit {MAX_SAMPLES} points"
            ));
        }
        self.summary.encode(encoder, &self.field)?;
        self.summary.copy_to(encoder, &self.staging, 0)?;
        if !points.is_empty() {
            let id = self
                .samples
                .request_buoyancy(
                    encoder,
                    &self.field,
                    points,
                    SampleStamp {
                        epoch: 0,
                        tick: self.applied as u64 + 1,
                    },
                    self.body_start,
                )?
                .ok_or("resident sample slot was not released")?;
            self.samples.copy_to(encoder, id, &self.staging, 64)?;
        }
        if reaction {
            self.reactions
                .as_ref()
                .ok_or("reaction budget is unavailable")?
                .copy_to(
                    encoder,
                    &self.staging,
                    (64 + points.len() * SAMPLE_WORDS * 4) as u64,
                );
        }
        if boundary {
            self.field.copy_boundary_report(
                encoder,
                &self.staging,
                (64 + points.len() * SAMPLE_WORDS * 4 + if reaction { 32 } else { 0 }) as u64,
            )?;
        }
        if edited {
            self.edits.copy_receipt(
                encoder,
                &self.staging,
                (64 + points.len() * SAMPLE_WORDS * 4
                    + if reaction { 32 } else { 0 }
                    + if boundary { 16 } else { 0 }) as u64,
            )?;
        }
        Ok(())
    }
    fn decode_readout(
        &mut self,
        words: &[u32],
        reaction: bool,
        boundary: bool,
        edited: bool,
    ) -> Result<Readout, String> {
        self.summary.discard();
        self.samples.clear();
        let summary = WaterStateSummary::from_bytes(bytemuck::cast_slice(&words[..16]))?;
        let flow_end = words.len() - if edited { 4 } else { 0 };
        let receipt_end = flow_end - if boundary { 4 } else { 0 };
        let end = receipt_end - if reaction { 8 } else { 0 };
        let samples: Vec<[f32; SAMPLE_WORDS]> = bytemuck::cast_slice(&words[16..end]).to_vec();
        if samples
            .iter()
            .any(|p| p[1] < 0.0 || p.iter().any(|v| !v.is_finite()))
        {
            return Err("resident water sample is invalid".into());
        }
        let report = if reaction {
            ReactionReport::from_bytes(bytemuck::cast_slice(&words[end..receipt_end]))?
        } else {
            ReactionReport::default()
        };
        let mut flow = if boundary {
            BoundaryReport::from_bytes(bytemuck::cast_slice(&words[receipt_end..flow_end]))?
        } else {
            BoundaryReport::default()
        };
        if edited {
            flow.edited = WaterEdits::decode_receipt(bytemuck::cast_slice(&words[flow_end..]))?;
        }
        Ok(Readout {
            summary,
            samples,
            reaction: report,
            boundary: flow,
        })
    }
    fn prepare_sync(
        &mut self,
        checkpoint: &Arc<CheckedCheckpoint>,
        recipe: &FieldRecipe,
    ) -> Result<bool, String> {
        if !recipe.environment.is_empty() && recipe.environment != self.environment {
            return Err("resident water execution environment differs from its checkpoint (adapter, driver, backend, features or numerical kernels changed)".into());
        }
        if recipe.steps().len() > MAX_STEPS {
            return Err("resident replay bound exceeded".into());
        }
        // The immutable payload already owns its checked identity. Compare it on every
        // request; no pointer-based exemption or repeated whole-field hash is needed.
        if checkpoint.key() != recipe.checkpoint() {
            return Err("resident checkpoint content differs from its key".into());
        }
        self.summary.discard();
        self.samples.clear();
        let same_base = self.seed_key == Some(recipe.checkpoint().0);
        let prefix =
            self.valid && same_base && recipe.prefix_key(self.applied).map(|h| h.0) == self.program;
        // A compacted checkpoint can name exactly the verified current field, without uploading it.
        if self.valid
            && !same_base
            && recipe.steps().is_empty()
            && self.field.size() == checkpoint.size
            && self
                .seed
                .as_ref()
                .is_some_and(|s| s.datum.to_bits() == checkpoint.datum.to_bits())
            && self.latest.is_some_and(|s| s.digest == recipe.digest)
            && checkpoint.digest() == recipe.digest
        {
            self.seed = Some(checkpoint.clone());
            self.seed_key = Some(recipe.checkpoint().0);
            self.applied = 0;
            self.program = Some(recipe.program_key().0);
            return Ok(false);
        }
        if !prefix {
            self.valid = false;
            self.field.set_size(checkpoint.size)?;
            self.field.upload_checked_checkpoint(checkpoint)?;
            self.transfers.uploads += 1;
            self.applied = 0;
            self.latest = None;
        } else if self.applied == recipe.steps().len()
            && self.latest.is_some_and(|s| s.digest == recipe.digest)
        {
            self.seed = Some(checkpoint.clone());
            return Ok(false);
        }
        self.seed = Some(checkpoint.clone());
        self.seed_key = Some(recipe.checkpoint().0);
        self.valid = false;
        for step in &recipe.steps()[self.applied..] {
            let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
            self.encode_step(&mut encoder, step)?;
            self.gpu.queue.submit([encoder.finish()]);
        }
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.encode_readout(&mut encoder, &[], false, false, false)?;
        self.gpu.queue.submit([encoder.finish()]);
        self.transfers.small_readbacks += 1;
        self.transfers.readback_bytes += 64;
        Ok(true)
    }
    fn accept_sync(
        &mut self,
        words: &[u32],
        recipe: &FieldRecipe,
    ) -> Result<WaterStateSummary, String> {
        let Readout { summary, .. } = self.decode_readout(words, false, false, false)?;
        if summary.digest != recipe.digest {
            return Err("resident water replay diverged from the stored state digest".into());
        }
        self.applied = recipe.steps().len();
        self.program = Some(recipe.program_key().0);
        self.latest = Some(summary);
        self.valid = true;
        Ok(summary)
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn synchronize(
        &mut self,
        checkpoint: &Arc<CheckedCheckpoint>,
        recipe: &FieldRecipe,
    ) -> Result<WaterStateSummary, String> {
        if !self.prepare_sync(checkpoint, recipe)? {
            return self.latest.ok_or("resident summary is unavailable".into());
        }
        let mut words = [0u32; 16];
        crate::readback::read_blocking(&self.gpu.device, &self.staging, &mut words)?;
        self.accept_sync(&words, recipe)
    }
    pub async fn synchronize_async(
        &mut self,
        checkpoint: &Arc<CheckedCheckpoint>,
        recipe: &FieldRecipe,
    ) -> Result<WaterStateSummary, String> {
        if !self.prepare_sync(checkpoint, recipe)? {
            return self.latest.ok_or("resident summary is unavailable".into());
        }
        let mut words = [0u32; 16];
        crate::readback::read(&self.staging, &mut words).await?;
        self.accept_sync(&words, recipe)
    }
    fn prepare_observe(&mut self, points: &[[f32; 2]]) -> Result<(), String> {
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.encode_observe(&mut encoder, points)?;
        self.gpu.queue.submit([encoder.finish()]);
        Ok(())
    }
    fn encode_observe(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        points: &[[f32; 2]],
    ) -> Result<(), String> {
        self.encode_readout(encoder, points, false, false, false)?;
        self.transfers.small_readbacks += 1;
        self.transfers.readback_bytes += (64 + points.len() * SAMPLE_WORDS * 4) as u64;
        Ok(())
    }
    fn accept_observe(
        &mut self,
        words: &[u32],
        recipe: &FieldRecipe,
    ) -> Result<ResidentAdvance, String> {
        let Readout {
            summary,
            samples,
            reaction,
            boundary,
        } = self.decode_readout(words, false, false, false)?;
        if summary.digest != recipe.digest {
            self.valid = false;
            return Err("resident observation differs from the stored state digest".into());
        }
        self.latest = Some(summary);
        Ok(ResidentAdvance {
            recipe: recipe.clone(),
            summary,
            samples,
            checkpoint: None,
            reaction,
            boundary,
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn observe(
        &mut self,
        checkpoint: &Arc<CheckedCheckpoint>,
        recipe: &FieldRecipe,
        points: &[[f32; 2]],
    ) -> Result<ResidentAdvance, String> {
        self.synchronize(checkpoint, recipe)?;
        self.prepare_observe(points)?;
        let mut words = vec![0u32; 16 + points.len() * SAMPLE_WORDS];
        crate::readback::read_blocking(&self.gpu.device, &self.staging, &mut words)?;
        self.accept_observe(&words, recipe)
    }
    pub async fn observe_async(
        &mut self,
        checkpoint: &Arc<CheckedCheckpoint>,
        recipe: &FieldRecipe,
        points: &[[f32; 2]],
    ) -> Result<ResidentAdvance, String> {
        self.synchronize_async(checkpoint, recipe).await?;
        self.prepare_observe(points)?;
        let mut words = vec![0u32; 16 + points.len() * SAMPLE_WORDS];
        crate::readback::read(&self.staging, &mut words).await?;
        self.accept_observe(&words, recipe)
    }
    fn prepare_advance(&mut self, step: &FieldStep, points: &[[f32; 2]]) -> Result<(), String> {
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.encode_advance(&mut encoder, step, points)?;
        self.gpu.queue.submit([encoder.finish()]);
        Ok(())
    }
    fn encode_advance(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        step: &FieldStep,
        points: &[[f32; 2]],
    ) -> Result<(), String> {
        self.valid = false;
        self.encode_step(encoder, step)?;
        self.encode_readout(
            encoder,
            points,
            !step.impulses.is_empty(),
            step.boundaries
                .iter()
                .any(|e| *e != amoris_assets::water_field::FieldBoundary::Wall),
            !step.edits.is_empty(),
        )?;
        self.transfers.small_readbacks += 1;
        self.transfers.readback_bytes += (readout_words(Some(step), points.len()) * 4) as u64;
        Ok(())
    }
    fn accept_advance(
        &mut self,
        words: &[u32],
        before: &FieldRecipe,
        step: &FieldStep,
    ) -> Result<ResidentAdvance, String> {
        let Readout {
            summary,
            samples,
            reaction,
            boundary,
        } = self.decode_readout(
            words,
            !step.impulses.is_empty(),
            step.boundaries
                .iter()
                .any(|e| *e != amoris_assets::water_field::FieldBoundary::Wall),
            !step.edits.is_empty(),
        )?;
        let mut recipe = before.clone();
        recipe.push_step(step.clone());
        recipe.digest = summary.digest;
        recipe.environment = self.environment.clone();
        self.applied = recipe.steps().len();
        self.program = Some(recipe.program_key().0);
        self.latest = Some(summary);
        self.valid = true;
        Ok(ResidentAdvance {
            recipe,
            summary,
            samples,
            checkpoint: None,
            reaction,
            boundary,
        })
    }
    fn compact(
        &mut self,
        checkpoint: FieldCheckpoint,
        out: &mut ResidentAdvance,
    ) -> Result<(), String> {
        let checkpoint = Arc::new(checkpoint.checked()?);
        let (key, digest) = checkpoint.identity();
        if digest != out.summary.digest {
            self.valid = false;
            return Err("resident checkpoint differs from the GPU state digest".into());
        }
        self.transfers.field_readbacks += 1;
        self.transfers.readback_bytes +=
            (checkpoint.columns.len() * 16 + checkpoint.packed_faces().len() * 4) as u64;
        out.recipe.reset(key, digest);
        out.checkpoint = Some(checkpoint.clone());
        self.seed = Some(checkpoint);
        self.seed_key = Some(out.recipe.checkpoint().0);
        self.applied = 0;
        self.program = Some(out.recipe.program_key().0);
        self.valid = true;
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn advance(
        &mut self,
        checkpoint: &Arc<CheckedCheckpoint>,
        before: &FieldRecipe,
        step: &FieldStep,
        points: &[[f32; 2]],
        checkpoint_now: bool,
    ) -> Result<ResidentAdvance, String> {
        if before.steps().len() >= MAX_STEPS {
            return Err("compact the bounded water program before advancing it".into());
        }
        self.synchronize(checkpoint, before)?;
        self.prepare_advance(step, points)?;
        let mut words = vec![
            0u32;
            16 + points.len() * SAMPLE_WORDS
                + if step.impulses.is_empty() { 0 } else { 8 }
                + if step
                    .boundaries
                    .iter()
                    .any(|e| *e != amoris_assets::water_field::FieldBoundary::Wall)
                {
                    4
                } else {
                    0
                }
                + if step.edits.is_empty() { 0 } else { 4 }
        ];
        crate::readback::read_blocking(&self.gpu.device, &self.staging, &mut words)?;
        let mut out = self.accept_advance(&words, before, step)?;
        if checkpoint_now || out.recipe.steps().len() == MAX_STEPS {
            self.valid = false;
            let datum = self
                .seed
                .as_ref()
                .ok_or("resident seed is unavailable")?
                .datum;
            let bits = self.field.checkpoint_complete(datum)?;
            self.compact(bits, &mut out)?;
        }
        Ok(out)
    }
    pub async fn advance_async(
        &mut self,
        checkpoint: &Arc<CheckedCheckpoint>,
        before: &FieldRecipe,
        step: &FieldStep,
        points: &[[f32; 2]],
        checkpoint_now: bool,
    ) -> Result<ResidentAdvance, String> {
        if before.steps().len() >= MAX_STEPS {
            return Err("compact the bounded water program before advancing it".into());
        }
        self.synchronize_async(checkpoint, before).await?;
        self.prepare_advance(step, points)?;
        let mut words = vec![
            0u32;
            16 + points.len() * SAMPLE_WORDS
                + if step.impulses.is_empty() { 0 } else { 8 }
                + if step
                    .boundaries
                    .iter()
                    .any(|e| *e != amoris_assets::water_field::FieldBoundary::Wall)
                {
                    4
                } else {
                    0
                }
                + if step.edits.is_empty() { 0 } else { 4 }
        ];
        crate::readback::read(&self.staging, &mut words).await?;
        let mut out = self.accept_advance(&words, before, step)?;
        if checkpoint_now || out.recipe.steps().len() == MAX_STEPS {
            self.valid = false;
            let datum = self
                .seed
                .as_ref()
                .ok_or("resident seed is unavailable")?
                .datum;
            let bits = self.field.checkpoint_complete_async(datum).await?;
            self.compact(bits, &mut out)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod kernel_tests {
    use super::*;
    #[test]
    fn visual_programs_require_the_current_numerical_sources() {
        let checkpoint = FieldCheckpoint::new([2, 2], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 4]);
        let step = FieldStep {
            sea: None,
            boundaries: [amoris_assets::water_field::FieldBoundary::Wall; 4],
            cell_m: 1.0,
            substep_dt: 0.01,
            substeps: 1,
            friction: 0.0,
            calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
            dry_below: 0.0,
            edits: vec![],
            impulses: vec![],
            displacements: vec![],
            physics: Default::default(),
        };
        let mut recipe = FieldRecipe::new(
            checkpoint.key(),
            vec![],
            checkpoint.digest(),
            "unknown old host".into(),
        );
        validate_recipe_kernel(&recipe).unwrap();
        recipe.push_step(step);
        assert!(validate_recipe_kernel(&recipe).is_err());
        recipe.environment = format!("another device; kernels {}", kernel_key());
        validate_recipe_kernel(&recipe).unwrap();
        recipe.environment.push('0');
        assert!(validate_recipe_kernel(&recipe).is_err());
    }
}
