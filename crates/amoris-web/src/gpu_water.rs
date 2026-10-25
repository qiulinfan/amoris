//! The Web Worker's GPU host. Owns graphics resources outside the game, accepts owned jobs and
//! returns only completed numerical data. The game commits that data in its suspended system.
use amoris_contract::{Problem, detail};
use amoris_render::water_resident::{ResidentBatchRequest, ResidentPool};
use amoris_render::{BackendChoice, Gpu, gpu::Minimal, water_compute::GpuWater};
use amoris_runtime::water_compute::{
    Job, JobOutput, Output, Request, ResidentBatchJob, ResidentOutput, ResidentResult,
    ResidentStats,
};
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WaterJob {
    pub(crate) input: Job,
}
#[wasm_bindgen]
impl WaterJob {
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> u64 {
        self.input.id()
    }
    #[wasm_bindgen(getter)]
    pub fn kind(&self) -> String {
        match &self.input {
            Job::Grid(_) => "grid",
            Job::Resident(_) => "resident",
        }
        .into()
    }
    #[wasm_bindgen(getter)]
    pub fn region_count(&self) -> usize {
        match &self.input {
            Job::Grid(_) => 0,
            Job::Resident(batch) => batch.requests.len(),
        }
    }
}
#[wasm_bindgen]
pub struct WaterResult {
    pub(crate) id: u64,
    pub(crate) result: Result<JobOutput, Problem>,
}

struct Service {
    gpu: Gpu,
    field: Option<GpuWater>,
    resident: ResidentPool,
    error: Arc<Mutex<Option<String>>>,
}

#[wasm_bindgen]
pub struct BrowserWater {
    service: RefCell<Option<Service>>,
    device: RefCell<Option<wgpu::Device>>,
    epoch: Cell<u64>,
    busy: Cell<bool>,
}

impl Default for BrowserWater {
    fn default() -> Self {
        Self {
            service: RefCell::new(None),
            device: RefCell::new(None),
            epoch: Cell::new(0),
            busy: Cell::new(false),
        }
    }
}

fn failed(message: impl Into<String>) -> Problem {
    Problem::new("water.compute_failed", message.into(), detail([]))
}
impl Service {
    async fn new() -> Result<Self, Problem> {
        let minimal = Minimal::parse("features,limits,timestamps");
        let instance = amoris_render::gpu::instance_with(BackendChoice::Auto, minimal);
        let gpu = Gpu::new_with(instance, None, minimal)
            .await
            .map_err(|e| failed(e.to_string()))?;
        let error = Arc::new(Mutex::new(None));
        let validation = error.clone();
        gpu.device.on_uncaptured_error(Arc::new(move |e| {
            *validation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(format!("water validation: {e}"));
        }));
        let lost = error.clone();
        gpu.device.set_device_lost_callback(move |reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                *lost
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(format!("water device lost: {reason:?}: {message}"));
            }
        });
        Ok(Self {
            gpu,
            field: None,
            resident: ResidentPool::default(),
            error,
        })
    }
    fn take_error(&self) -> Result<(), Problem> {
        if let Some(e) = self
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            return Err(failed(e));
        }
        Ok(())
    }
    async fn advance(&mut self, r: Job) -> Result<JobOutput, Problem> {
        self.take_error()?;
        let out = match r {
            Job::Grid(r) => self.advance_grid(r).await.map(JobOutput::Grid),
            Job::Resident(r) => self.advance_resident(*r).await.map(JobOutput::Resident),
        };
        self.take_error()?;
        out
    }
    #[allow(clippy::cast_possible_truncation)] // Explicit f32 GPU execution contract (water.md 17).
    async fn advance_grid(&mut self, r: Request) -> Result<Output, Problem> {
        let [nx, nz] = r.size;
        let size = [nx, nz].map(|n| u32::try_from(n).unwrap_or(u32::MAX));
        if self.field.is_none() {
            self.field =
                Some(GpuWater::new(&self.gpu.device, &self.gpu.queue, [512, 512]).map_err(failed)?);
        }
        let field = self.field.as_mut().expect("grid service initialized");
        field.set_size(size).map_err(failed)?;
        let datum = r.bed.iter().copied().fold(f64::INFINITY, f64::min);
        let mut columns = Vec::with_capacity(nx * nz);
        for k in 0..nz {
            for i in 0..nx {
                let n = i + nx * k;
                columns.push([
                    (r.bed[n] - datum) as f32,
                    r.depth[n] as f32,
                    r.u[i + (nx + 1) * k] as f32,
                    r.w[n] as f32,
                ]);
            }
        }
        let active: Vec<u32> = r.active.iter().map(|a| u32::from(*a)).collect();
        let params = (
            r.cell_m as f32,
            r.dt as f32,
            r.substeps,
            [r.friction as f32, r.calm_per_s as f32],
        );
        // The owned f64 input is no longer needed once packed. Do not hold another full window
        // through mapping and output conversion; the game's continuation already owns its copy.
        drop(r);
        field
            .advance_async(
                &mut columns,
                &active,
                params.0,
                params.1,
                params.2,
                params.3,
            )
            .await
            .map_err(failed)?;
        Ok(Output {
            columns: columns
                .into_iter()
                .map(|c| [f64::from(c[1]), f64::from(c[2]), f64::from(c[3])])
                .collect(),
        })
    }
    #[allow(clippy::cast_possible_truncation)] // Query positions use the explicit f32 grid domain.
    async fn advance_resident(
        &mut self,
        r: ResidentBatchJob,
    ) -> Result<Vec<ResidentResult>, Problem> {
        // These owned point sets outlive every borrowed renderer request across the batch await.
        let points: Vec<Vec<_>> = r
            .requests
            .iter()
            .map(|r| r.points.iter().map(|p| p.map(|v| v as f32)).collect())
            .collect();
        let batch: Vec<_> = r
            .requests
            .iter()
            .zip(&points)
            .map(|(r, points)| ResidentBatchRequest {
                region: r.region.get(),
                checkpoint: &r.checkpoint,
                recipe: &r.recipe,
                step: r.step.as_ref(),
                points,
                checkpoint_now: r.checkpoint_now,
                coupled_from: r.coupled_from,
            })
            .collect();
        let outputs = self
            .resident
            .execute_batch_async(&self.gpu, &batch)
            .await
            .map_err(failed)?;
        if outputs.len() != r.requests.len() {
            return Err(failed(
                "resident batch output count differs from its requests",
            ));
        }
        r.requests
            .iter()
            .zip(outputs)
            .map(|(r, out)| {
                if out.samples.len() != r.points.len() {
                    return Err(failed(
                        "resident batch output sample count differs from its request",
                    ));
                }
                let s = out.summary;
                Ok(ResidentResult {
                    region: r.region,
                    output: ResidentOutput {
                        reaction: amoris_runtime::water_compute::ReactionBudget {
                            applied: out.reaction.applied.map(f64::from),
                            rejected: out.reaction.rejected.map(f64::from),
                            roundoff: out.reaction.roundoff.map(f64::from),
                        },
                        boundary: amoris_runtime::water_compute::BoundaryFlux {
                            entered: f64::from(out.boundary.entered),
                            left: f64::from(out.boundary.left),
                            edited: f64::from(out.boundary.edited)
                                * r.step.as_ref().map_or(0.0, |s| s.cell_m * s.cell_m),
                        },
                        recipe: out.recipe,
                        stats: ResidentStats {
                            max_depth: f64::from(s.max_depth),
                            max_speed: f64::from(s.max_speed),
                            max_pressure_jump: f64::from(s.max_pressure_jump),
                            volume_cells: f64::from(s.volume_cells),
                            wet_edge_surface: r.checkpoint.datum + f64::from(s.wet_edge_surface),
                            wet_edge_count: s.wet_edge_count,
                        },
                        samples: out
                            .samples
                            .into_iter()
                            .map(|p| {
                                let mut sample = p.map(f64::from);
                                sample[0] += r.checkpoint.datum;
                                sample
                            })
                            .collect(),
                        checkpoint: out.checkpoint,
                        edges: out
                            .edges
                            .iter()
                            .map(|c| {
                                [
                                    r.checkpoint.datum + f64::from(c[0]),
                                    r.checkpoint.datum + f64::from(c[1]),
                                    f64::from(c[2]),
                                    f64::from(c[3]),
                                ]
                            })
                            .collect(),
                    },
                })
            })
            .collect()
    }
}

#[wasm_bindgen]
impl BrowserWater {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Host diagnostics; counts actual transfers, not logical recipe sizes.
    pub fn diagnostics(&self) -> String {
        let service = self.service.borrow();
        let resident = service
            .as_ref()
            .filter(|s| !s.resident.is_empty())
            .map(|s| {
                let r = &s.resident;
                let t = r.transfers();
                let b = r.batch_diagnostics();
                serde_json::json!({"uploads":t.uploads,"field_readbacks":t.field_readbacks,
                "small_readbacks":t.small_readbacks,"readback_bytes":t.readback_bytes,
                "sync_groups":b.sync_groups,"step_groups":b.step_groups,
                "checkpoint_groups":b.checkpoint_groups,"max_group_regions":b.max_group_regions,
                "environment":r.environment(),"cached_regions":r.len(),
                "cache_capacity":ResidentPool::MAX_REGIONS,"cache_column_budget":ResidentPool::COLUMN_BUDGET,"cached_columns":r.columns(),"cache_resizes":r.resizes(),"cache_evictions":r.evictions()})
            });
        serde_json::json!({"busy":self.busy.get(),"resident":resident}).to_string()
    }

    /// Immutable self keeps abort callable while a map is pending. Service state is owned by
    /// this future across await; no RefCell borrow or game/world reference survives it.
    pub async fn advance(&self, job: WaterJob) -> WaterResult {
        let id = job.input.id();
        if self.busy.replace(true) {
            return WaterResult {
                id,
                result: Err(failed("water host already has an active job")),
            };
        }
        let epoch = self.epoch.get();
        let cached = self.service.borrow_mut().take();
        let service = match cached {
            Some(s) => Ok(s),
            None => Service::new().await,
        };
        let result = match service {
            Err(e) => Err(e),
            Ok(mut s) => {
                self.device.replace(Some(s.gpu.device.clone()));
                let result = if self.epoch.get() != epoch {
                    Err(failed("water computation cancelled"))
                } else {
                    s.advance(job.input).await
                };
                // Destruction can race a successful map callback. A result that crossed an
                // abort still belongs to the cancelled epoch and must never commit to the game.
                let result = if self.epoch.get() != epoch {
                    Err(failed("water computation cancelled"))
                } else {
                    result
                };
                if self.epoch.get() == epoch && result.is_ok() {
                    self.service.replace(Some(s));
                } else {
                    s.gpu.device.destroy();
                }
                result
            }
        };
        self.busy.set(false);
        WaterResult { id, result }
    }
    pub fn abort(&self) {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        if let Some(device) = self.device.borrow_mut().take() {
            device.destroy();
        }
        self.service.borrow_mut().take();
    }
}
