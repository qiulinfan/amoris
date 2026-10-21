//! A viewport's disposable GPU copy of a published water program. This never produces gameplay
//! observations or state receipts: only the simulation host verifies authoritative digests.
use crate::water_reactions::WaterReactions;
use crate::{water_compute::GpuWater, water_edits::WaterEdits};
use amoris_assets::water_field::{CheckedCheckpoint, FieldRecipe};

pub(crate) struct WaterReplica {
    field: GpuWater,
    edits: WaterEdits,
    reactions: Option<WaterReactions>,
    seed: Option<[u8; 32]>,
    datum: f64,
    program: Option<[u8; 32]>,
    applied: usize,
    digest: Option<[u8; 32]>,
    convert: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    dirty: bool,
}
impl WaterReplica {
    pub fn invalidate_presentation(&mut self) {
        self.dirty = true;
    }
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: [u32; 2]) -> Result<Self, String> {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water presentation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water_present.wgsl").into()),
        });
        Ok(Self {
            field: GpuWater::new(device, queue, size)?,
            edits: WaterEdits::new(device, queue),
            reactions: None,
            seed: None,
            datum: 0.0,
            program: None,
            applied: 0,
            digest: None,
            convert: device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("water presentation"),
                layout: None,
                module: &module,
                entry_point: Some("present"),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            }),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("water presentation parameters"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            dirty: true,
        })
    }
    pub fn synchronize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        checkpoint: &CheckedCheckpoint,
        recipe: &FieldRecipe,
    ) -> Result<(), String> {
        let result = self.synchronize_inner(device, queue, checkpoint, recipe);
        if result.is_err() {
            // Earlier historical steps may already have been submitted. Their prefix must not
            // be reused as though the old published program still described the GPU buffer.
            self.seed = None;
            self.program = None;
            self.digest = None;
            self.applied = 0;
        }
        result
    }
    fn synchronize_inner(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        checkpoint: &CheckedCheckpoint,
        recipe: &FieldRecipe,
    ) -> Result<(), String> {
        if recipe.steps().len() > amoris_assets::water_field::MAX_STEPS {
            return Err("viewport water replay bound exceeded".into());
        }
        // Compaction names the same physical state with a new checkpoint. Keep the live buffer.
        if self.seed != Some(recipe.checkpoint().0)
            && recipe.steps().is_empty()
            && self.digest == Some(recipe.digest)
            && self.field.size() == checkpoint.size
            && self.datum.to_bits() == checkpoint.datum.to_bits()
        {
            self.seed = Some(recipe.checkpoint().0);
            self.applied = 0;
            self.program = Some(recipe.program_key().0);
            return Ok(());
        }
        let prefix = self.seed == Some(recipe.checkpoint().0)
            && recipe.prefix_key(self.applied).map(|k| k.0) == self.program;
        if !prefix {
            recipe.validate_checked(checkpoint)?;
            self.field.set_size(checkpoint.size)?;
            self.field.upload_checked_checkpoint(checkpoint)?;
            self.applied = 0;
            self.datum = checkpoint.datum;
            self.seed = Some(recipe.checkpoint().0);
            self.dirty = true;
        }
        for step in &recipe.steps()[self.applied..] {
            step.validate(checkpoint.columns.len())?;
            if step.dry_below != 0.0 {
                return Err("viewport resident drying is unavailable".into());
            }
            // Each submission owns these parameter writes; putting several writes to the same
            // uniform before one submit would replay every step using the last step's parameters.
            let mut encoder = device.create_command_encoder(&Default::default());
            if !step.impulses.is_empty() {
                self.reactions
                    .get_or_insert_with(|| WaterReactions::new(device, queue))
                    .encode_presentation(&mut encoder, &self.field, step.cell_m, &step.impulses)?;
            }
            self.edits.encode(&mut encoder, &self.field, &step.edits)?;
            self.field.encode_transport(&mut encoder, step, None)?;
            queue.submit([encoder.finish()]);
            self.dirty = true;
        }
        self.applied = recipe.steps().len();
        self.program = Some(recipe.program_key().0);
        self.digest = Some(recipe.digest);
        Ok(())
    }
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        columns: &wgpu::Buffer,
        profiler: &mut crate::profiler::GpuProfiler,
    ) -> bool {
        if !self.dirty {
            return false;
        }
        let size = self.field.size();
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[size[0], size[1], (self.datum as f32).to_bits(), 0]),
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water presentation"),
            layout: &self.convert.get_bind_group_layout(0),
            entries: [&self.params, self.field.buffer(), columns]
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>()
                .as_slice(),
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water presentation"),
            timestamp_writes: profiler.compute_scope("water presentation"),
        });
        pass.set_pipeline(&self.convert);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        self.dirty = false;
        true
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{BackendChoice, Gpu, water_resident::ResidentWater};
    use amoris_assets::water_field::{
        ColumnDelta, FieldBoundary, FieldCheckpoint, FieldSea, FieldSeaWave, FieldStep,
        PointImpulse,
    };
    use std::sync::Arc;

    #[test]
    fn visual_replay_matches_warm_authority_and_recovers_after_rejected_program() {
        for backend in BackendChoice::under_test() {
            let gpu = Gpu::headless(backend).expect("visual water replay requires a GPU");
            gpu.device
                .on_uncaptured_error(Arc::new(|error| panic!("visual replay: {error}")));
            let checkpoint = Arc::new(
                FieldCheckpoint::new([8, 8], 7.0, vec![[0, 1.5f32.to_bits(), 0, 0]; 64])
                    .checked()
                    .unwrap(),
            );
            let mut recipe =
                FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
            let mut reference = ResidentWater::new(&gpu).unwrap();
            for tick in 0..7 {
                let mut boundaries = [FieldBoundary::Wall; 4];
                let sea = if tick % 3 == 2 {
                    boundaries[1] = FieldBoundary::Sea;
                    Some(FieldSea {
                        level_relative_m: 1.4,
                        waves: vec![FieldSeaWave {
                            amplitude_m: 0.02,
                            wave_vector_rad_m: [0.05, 0.01],
                            angular_frequency_rad_s: 0.3,
                            phase_at_start_rad: tick as f64 * 0.1,
                        }],
                    })
                } else {
                    if tick % 3 == 1 {
                        boundaries[0] = FieldBoundary::Inflow { m3s_per_m: 0.03 };
                        boundaries[1] = FieldBoundary::Outflow;
                    }
                    None
                };
                let step = FieldStep {
                    boundaries,
                    sea,
                    cell_m: 1.0,
                    substep_dt: 0.004 + tick as f64 * 0.001,
                    substeps: 1 + tick % 3,
                    friction: tick as f64 * 0.01,
                    dry_below: 0.0,
                    edits: if tick.is_multiple_of(2) {
                        vec![]
                    } else {
                        vec![
                            ColumnDelta {
                                column: 11,
                                depth_m: -3.0,
                            },
                            ColumnDelta {
                                column: 11,
                                depth_m: 0.25,
                            },
                            ColumnDelta {
                                column: 11,
                                depth_m: -0.1,
                            },
                        ]
                    },
                    impulses: vec![PointImpulse {
                        at: [3.5, 4.5],
                        impulse: [0.5, -0.2],
                    }],
                };
                recipe = reference
                    .advance(&checkpoint, &recipe, &step, &[], false)
                    .unwrap()
                    .recipe;
            }
            let field = reference.field().unwrap();
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            field.encode_checkpoint_copy(&mut encoder, true).unwrap();
            gpu.queue.submit([encoder.finish()]);
            let mut packed = vec![[0; 4]; field.state_words() as usize / 4];
            crate::readback::read_blocking(&gpu.device, field.checkpoint_buffer(), &mut packed)
                .unwrap();
            let expected = field.unpack_checkpoint(checkpoint.datum, packed);
            let mut replica = WaterReplica::new(&gpu.device, &gpu.queue, checkpoint.size).unwrap();
            replica
                .synchronize(&gpu.device, &gpu.queue, &checkpoint, &recipe)
                .unwrap();
            assert_eq!(
                replica.field.checkpoint_complete(checkpoint.datum).unwrap(),
                expected
            );
            let mut rejected = recipe.clone();
            let mut invalid = recipe.steps().last().unwrap().as_ref().clone();
            invalid.dry_below = 0.01;
            rejected.push_step(invalid);
            assert!(
                replica
                    .synchronize(&gpu.device, &gpu.queue, &checkpoint, &rejected)
                    .unwrap_err()
                    .contains("drying")
            );
            assert!(
                replica.seed.is_none() && replica.program.is_none() && replica.digest.is_none()
            );
            replica
                .synchronize(&gpu.device, &gpu.queue, &checkpoint, &recipe)
                .unwrap();
            assert_eq!(
                replica.field.checkpoint_complete(checkpoint.datum).unwrap(),
                expected
            );
        }
    }
}
