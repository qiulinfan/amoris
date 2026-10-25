//! Native host injection for GPU water. The runtime and physics crates never link wgpu.

use std::sync::{Arc, Mutex};

use amoris_contract::{Problem, detail};
use amoris_render::water_compute::GpuWater;
use amoris_render::{BackendChoice, Gpu};
use amoris_runtime::water_compute::{GridView, WaterCompute, install};
use amoris_runtime::{GameBuilder, GameSetup};

/// All native play/serve/run builders carry this factory, so Play and lookahead forks keep the
/// same execution path. CPU-only scenes do not open a GPU device.
pub fn builder(setup: Arc<GameSetup>, seed: u64) -> GameBuilder {
    configure(GameBuilder::new(setup).seed(seed))
}

pub fn configure(builder: GameBuilder) -> GameBuilder {
    // Forks reuse the device, pipelines and scratch buffers. Every call verifies its complete
    // input against the last GPU result or uploads it, so no branch inherits hidden water state.
    let device = Arc::new(Mutex::new(WaterDevice::default()));
    builder.system(move |sim| {
        install(
            sim,
            NativeWater {
                device: device.clone(),
            },
        );
        Ok(())
    })
}

#[allow(dead_code)] // This module is also shared with the GPU example and integration test.
pub fn configure_subject(subject: &mut amoris_check::Subject) {
    subject.build = Arc::new(|setup, seed| builder(setup.clone(), seed).build());
    subject.replay_build =
        Arc::new(|replay| amoris_runtime::Game::for_replay_with(replay, configure));
}

#[derive(Clone, Default)]
pub struct NativeWater {
    device: Arc<Mutex<WaterDevice>>,
}

/// Read-only service counters. Requesting these submits no GPU work and maps no buffers.
#[allow(dead_code)] // Diagnostic consumers include the standalone pipeline benchmark.
pub struct ResidentDiagnostics {
    pub slots: usize,
    pub evictions: u64,
    pub columns: u64,
    pub resizes: u64,
    pub transfers: amoris_render::water_resident::Transfers,
    pub batches: amoris_render::water_resident::ResidentBatchDiagnostics,
}
impl NativeWater {
    #[allow(dead_code)]
    pub fn resident_diagnostics(&self) -> Result<ResidentDiagnostics, String> {
        let shared = self
            .device
            .lock()
            .map_err(|_| "GPU water service was poisoned")?;
        Ok(ResidentDiagnostics {
            slots: shared.resident.len(),
            evictions: shared.resident.evictions(),
            columns: shared.resident.columns(),
            resizes: shared.resident.resizes(),
            transfers: shared.resident.transfers(),
            batches: shared.resident.batch_diagnostics(),
        })
    }
}

#[derive(Default)]
struct WaterDevice {
    gpu: Option<Gpu>,
    field: Option<GpuWater>,
    resident: amoris_render::water_resident::ResidentPool,
    columns: Vec<[f32; 4]>,
    active: Vec<u32>,
    /// The CPU shadow describes the last successful GPU result, including signed-zero bits.
    valid: bool,
    error: Arc<Mutex<Option<String>>>,
}
impl WaterDevice {
    fn gpu(&mut self) -> Result<Gpu, Problem> {
        if self.gpu.is_none() {
            let gpu =
                Gpu::headless(BackendChoice::from_env()).map_err(|e| failed(e.to_string()))?;
            let error = self.error.clone();
            gpu.device.on_uncaptured_error(Arc::new(move |e| {
                *error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(format!("GPU water validation: {e}"));
            }));
            let lost = self.error.clone();
            gpu.device.set_device_lost_callback(move |reason, message| {
                if reason != wgpu::DeviceLostReason::Destroyed {
                    *lost
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(format!("GPU water device lost: {reason:?}: {message}"));
                }
            });
            eprintln!(
                "amoris: GPU water on {} ({:?}), f32; cross-device replay is not guaranteed",
                gpu.info.name, gpu.info.backend
            );
            self.gpu = Some(gpu);
        }
        Ok(self
            .gpu
            .as_ref()
            .expect("GPU water device initialized")
            .clone())
    }
    fn check_error(&self) -> Result<(), Problem> {
        match self
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            Some(error) => Err(failed(error.clone())),
            None => Ok(()),
        }
    }
}

fn failed(message: impl Into<String>) -> Problem {
    Problem::new("water.compute_failed", message.into(), detail([]))
}

impl WaterCompute for NativeWater {
    #[allow(clippy::cast_possible_truncation)] // GPU observation coordinates are f32 grid units.
    fn poll_resident_batch(
        &mut self,
        requests: &[amoris_runtime::water_compute::ResidentRequest<'_>],
    ) -> Result<std::task::Poll<Vec<amoris_runtime::water_compute::ResidentResult>>, Problem> {
        use amoris_render::water_resident::ResidentBatchRequest;
        let mut shared = self
            .device
            .lock()
            .map_err(|_| failed("GPU water service was poisoned"))?;
        shared.check_error()?;
        let gpu = shared.gpu()?;
        // Keep every converted point set alive until the complete cohort has finished mapping.
        let points: Vec<Vec<_>> = requests
            .iter()
            .map(|r| r.points.iter().map(|p| p.map(|v| v as f32)).collect())
            .collect();
        let batch: Vec<_> = requests
            .iter()
            .zip(&points)
            .map(|(r, points)| ResidentBatchRequest {
                region: r.region.get(),
                checkpoint: &r.checkpoint,
                recipe: r.recipe,
                step: r.step,
                points,
                checkpoint_now: r.checkpoint_now,
                coupled_from: r.coupled_from,
            })
            .collect();
        let out = shared.resident.execute_batch(&gpu, &batch);
        shared.check_error()?;
        Ok(std::task::Poll::Ready(resident_results(
            requests,
            out.map_err(failed)?,
        )?))
    }
    #[allow(clippy::cast_possible_truncation)] // GPU mode explicitly uses f32 (water.md).
    fn advance(
        &mut self,
        grid: GridView<'_>,
        cell_m: f64,
        substep_dt: f64,
        substeps: u32,
        [friction, calm]: [f64; 2],
    ) -> Result<(), Problem> {
        let mut shared = self
            .device
            .lock()
            .map_err(|_| failed("GPU water service was poisoned"))?;
        shared.check_error()?;
        if shared.field.is_none() {
            let gpu = shared.gpu()?;
            shared.field =
                Some(GpuWater::new(&gpu.device, &gpu.queue, [512, 512]).map_err(failed)?);
        }
        let size = grid.size.map(|n| u32::try_from(n).unwrap_or(u32::MAX));
        let [nx, nz] = grid.size;
        // Relative beds retain sub-metre slopes even when the world lies far from its origin.
        let datum = grid.bed.iter().copied().fold(f64::INFINITY, f64::min);
        let reshaped = shared.field.as_ref().expect("GPU water initialized").size() != size;
        let mut dirty = !shared.valid || reshaped || shared.columns.len() != nx * nz;
        shared.columns.resize(nx * nz, [0.0; 4]);
        shared.active.resize(nx * nz, 0);
        for k in 0..nz {
            for i in 0..nx {
                let n = i + nx * k;
                let fresh = [
                    (grid.bed[n] - datum) as f32,
                    grid.depth[n] as f32,
                    grid.u[i + (nx + 1) * k] as f32,
                    grid.w[n] as f32,
                ];
                let active = u32::from(grid.active[n]);
                dirty = dirty
                    || fresh.map(f32::to_bits) != shared.columns[n].map(f32::to_bits)
                    || active != shared.active[n];
                shared.columns[n] = fresh;
                shared.active[n] = active;
            }
        }
        let WaterDevice {
            field,
            columns,
            active,
            valid,
            error,
            ..
        } = &mut *shared;
        // A failed upload, submission or readback must invalidate reuse, including after a restore.
        *valid = false;
        let field = field.as_mut().expect("GPU water initialized");
        field.set_size(size).map_err(failed)?;
        if dirty {
            field.upload(columns, active).map_err(failed)?;
        }
        field
            .advance_uploaded(
                columns,
                cell_m as f32,
                substep_dt as f32,
                substeps,
                [friction as f32, calm as f32],
            )
            .map_err(failed)?;
        if let Some(e) = error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            return Err(failed(e.clone()));
        }
        *valid = true;
        // Commit only a successful complete readback, leaving beds exactly as authored.
        for k in 0..nz {
            for i in 0..nx {
                let n = i + nx * k;
                grid.depth[n] = f64::from(columns[n][1]);
                grid.u[i + (nx + 1) * k] = f64::from(columns[n][2]);
                grid.w[n] = f64::from(columns[n][3]);
            }
        }
        Ok(())
    }
    fn invalidate(&mut self) {
        if let Ok(mut device) = self.device.lock()
            && device.check_error().is_err()
        {
            *device = WaterDevice::default();
        }
    }
}

fn resident_results(
    requests: &[amoris_runtime::water_compute::ResidentRequest<'_>],
    outputs: Vec<amoris_render::water_resident::ResidentAdvance>,
) -> Result<Vec<amoris_runtime::water_compute::ResidentResult>, Problem> {
    use amoris_runtime::water_compute::{ResidentOutput, ResidentResult, ResidentStats};
    if outputs.len() != requests.len() {
        return Err(failed(
            "resident batch output count differs from its requests",
        ));
    }
    requests
        .iter()
        .zip(outputs)
        .map(|(request, out)| {
            if out.samples.len() != request.points.len() {
                return Err(failed(
                    "resident batch output sample count differs from its request",
                ));
            }
            let s = out.summary;
            Ok(ResidentResult {
                region: request.region,
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
                            * request.step.map_or(0.0, |s| s.cell_m * s.cell_m),
                    },
                    recipe: out.recipe,
                    stats: ResidentStats {
                        max_depth: f64::from(s.max_depth),
                        max_speed: f64::from(s.max_speed),
                        max_pressure_jump: f64::from(s.max_pressure_jump),
                        volume_cells: f64::from(s.volume_cells),
                        wet_edge_surface: request.checkpoint.datum + f64::from(s.wet_edge_surface),
                        wet_edge_count: s.wet_edge_count,
                    },
                    samples: out
                        .samples
                        .into_iter()
                        .map(|p| {
                            let mut sample = p.map(f64::from);
                            sample[0] += request.checkpoint.datum;
                            sample
                        })
                        .collect(),
                    checkpoint: out.checkpoint,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    use amoris_assets::water_field::{FieldBoundary, FieldCheckpoint, FieldRecipe, FieldStep};
    use amoris_render::water_resident::ResidentAdvance;
    use amoris_runtime::water_compute::ResidentRequest;

    fn resident_output(
        checkpoint: &Arc<amoris_assets::water_field::CheckedCheckpoint>,
    ) -> ResidentAdvance {
        ResidentAdvance {
            recipe: FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new()),
            summary: amoris_render::water_summary::WaterStateSummary {
                digest: checkpoint.digest(),
                max_depth: 3.0,
                max_speed: 4.0,
                max_pressure_jump: 5.0,
                volume_cells: 6.0,
                wet_edge_surface: 1.0,
                wet_edge_count: 2,
            },
            samples: vec![[2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 0.0]],
            checkpoint: Some(checkpoint.clone()),
            reaction: Default::default(),
            boundary: amoris_render::water_compute::BoundaryReport {
                entered: 1.0,
                left: 2.0,
                edited: 3.0,
            },
        }
    }

    #[test]
    fn resident_batch_results_keep_each_regions_datum_area_and_identity() {
        let checkpoints = [10.0, -20.0].map(|datum| {
            Arc::new(
                FieldCheckpoint::new([2, 2], datum, vec![[0, 1.0f32.to_bits(), 0, 0]; 4])
                    .checked()
                    .unwrap(),
            )
        });
        let recipes = checkpoints.each_ref().map(|checkpoint| {
            FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new())
        });
        let steps = [2.0, 3.0].map(|cell_m| FieldStep {
            boundaries: [FieldBoundary::Wall; 4],
            sea: None,
            cell_m,
            substep_dt: 0.01,
            substeps: 1,
            friction: 0.0,
            calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
            dry_below: 0.0,
            edits: vec![],
            impulses: vec![],
            displacements: vec![],
            physics: Default::default(),
        });
        let regions = [2, 1].map(|id| amoris_sim::EntityId::new(id).unwrap());
        let requests: Vec<_> = (0..2)
            .map(|i| ResidentRequest {
                region: regions[i],
                checkpoint: checkpoints[i].clone(),
                recipe: &recipes[i],
                step: Some(&steps[i]),
                points: &[[0.5, 0.5]],
                checkpoint_now: true,
                coupled_from: None,
            })
            .collect();
        let outputs = checkpoints.each_ref().map(resident_output).into();
        let results = resident_results(&requests, outputs).unwrap();
        assert_eq!(
            results.iter().map(|r| r.region).collect::<Vec<_>>(),
            regions
        );
        assert_eq!(
            results[0].output.samples,
            vec![[12.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 0.0]]
        );
        assert_eq!(
            results[1].output.samples,
            vec![[-18.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 0.0]]
        );
        assert_eq!(results[0].output.stats.wet_edge_surface, 11.0);
        assert_eq!(results[1].output.stats.wet_edge_surface, -19.0);
        assert_eq!(results[0].output.boundary.edited, 12.0);
        assert_eq!(results[1].output.boundary.edited, 27.0);
        for (result, checkpoint) in results.iter().zip(&checkpoints) {
            assert!(Arc::ptr_eq(
                result.output.checkpoint.as_ref().unwrap(),
                checkpoint
            ));
        }
        assert!(resident_results(&requests, vec![resident_output(&checkpoints[0])]).is_err());
        let mut outputs: Vec<_> = checkpoints.each_ref().map(resident_output).into();
        outputs[1].samples.clear();
        assert!(resident_results(&requests, outputs).is_err());
    }

    #[test]
    fn a_validation_error_cannot_commit_zero_samples_or_a_grid() {
        let mut host = NativeWater::default();
        let gpu = host.device.lock().unwrap().gpu().unwrap();
        let _bad = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("intentional water validation failure"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../tests/fixtures/invalid-water.wgsl").into(),
                ),
            });
        let mut depth = vec![2.0; 4];
        let mut u = vec![0.0; 6];
        let mut w = vec![0.0; 6];
        let error = host
            .advance(
                GridView {
                    size: [2, 2],
                    bed: &[0.0; 4],
                    depth: &mut depth,
                    u: &mut u,
                    w: &mut w,
                    active: &[true; 4],
                },
                1.0,
                0.01,
                1,
                [0.0, 0.2],
            )
            .unwrap_err();
        assert_eq!(error.code, "water.compute_failed");
        assert_eq!(depth, vec![2.0; 4]);
        host.invalidate();
        assert!(host.device.lock().unwrap().check_error().is_ok());
    }
}
