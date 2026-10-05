//! A bounded, entirely GPU-resident online radiance-cache experiment.
//!
//! The 14→32→32→3 ReLU MLP learns fresh uncached reflected path tails. Training minimizes
//! log-radiance error and querying terminates paths, so this is deliberately a biased toy.
//! One frame records at most `batch_size` labels, then runs one Adam update. The caller controls
//! which paths supply labels, warmup, image resets and whether cache queries are enabled.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub const INPUT_COUNT: usize = 14;
pub const HIDDEN_COUNT: usize = 32;
pub const PARAMETER_COUNT: usize = 1635;
pub const WGSL: &str = include_str!("../../shaders/online_nrc.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct NrcRecord {
    pub features: [[f32; 4]; 4],
    /// Reflected outgoing radiance, excluding this vertex's own emission.
    pub target: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct NrcParams {
    pub origin: [f32; 4],
    pub extent: [f32; 4],
    /// Batch capacity, train/query flag bits, Adam update number, warmup updates.
    pub shape: [u32; 4],
    /// Learning rate, radiance scale, gradient clip, max predicted log radiance.
    pub training: [f32; 4],
    pub query: [u32; 4],
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct NrcConfig {
    pub origin: [f32; 3],
    pub extent: [f32; 3],
    pub batch_size: u32,
    pub training: bool,
    pub querying: bool,
    pub learning_rate: f32,
    pub radiance_scale: f32,
    pub gradient_clip: f32,
    pub max_log_prediction: f32,
    pub warmup_updates: u32,
    pub min_query_depth: u32,
    pub seed: u32,
}

impl Default for NrcConfig {
    fn default() -> Self {
        Self {
            origin: [-1.0; 3],
            extent: [2.0; 3],
            batch_size: 256,
            training: true,
            querying: true,
            learning_rate: 0.002,
            radiance_scale: 1.0,
            gradient_clip: 10.0,
            max_log_prediction: 8.0,
            warmup_updates: 32,
            min_query_depth: 2,
            seed: 0x4e524331,
        }
    }
}

impl NrcConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.batch_size == 0 || self.batch_size > 1024 {
            return Err("online NRC batch size must be in 1..=1024".into());
        }
        if self.origin.iter().any(|v| !v.is_finite())
            || self.extent.iter().any(|v| !v.is_finite() || *v <= 1e-6)
        {
            return Err("online NRC bounds must be finite with positive extents".into());
        }
        for (name, v) in [
            ("learning rate", self.learning_rate),
            ("radiance scale", self.radiance_scale),
            ("gradient clip", self.gradient_clip),
            ("maximum log prediction", self.max_log_prediction),
        ] {
            if !v.is_finite() || v <= 0.0 {
                return Err(format!("online NRC {name} must be finite and positive"));
            }
        }
        if self.max_log_prediction > 20.0 {
            return Err("online NRC max log prediction must be <= 20".into());
        }
        Ok(())
    }

    pub fn params(&self, update: u32) -> NrcParams {
        NrcParams {
            origin: [self.origin[0], self.origin[1], self.origin[2], 0.0],
            extent: [self.extent[0], self.extent[1], self.extent[2], 0.0],
            shape: [
                self.batch_size,
                u32::from(self.training) | (u32::from(self.querying) << 1),
                update,
                self.warmup_updates,
            ],
            training: [
                self.learning_rate,
                self.radiance_scale,
                self.gradient_clip,
                self.max_log_prediction,
            ],
            query: [self.min_query_depth, 0, 0, 0],
        }
    }
}

/// Statistics and pre-update loss for one frame, never a history average.
#[derive(Clone, Debug, serde::Serialize)]
pub struct NrcFrameStats {
    pub requested_records: u32,
    pub cache_queries: u32,
    pub invalid_records: u32,
    pub invalid_updates: u32,
    pub overflow_records: u32,
    pub trained_records: u32,
    pub invalid_predictions: u32,
    pub mean_log_mse: f64,
    pub updates: u32,
}

pub struct OnlineNrc {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    pub params: wgpu::Buffer,
    pub weights: wgpu::Buffer,
    pub records: wgpu::Buffer,
    pub gradients: wgpu::Buffer,
    pub moments: wgpu::Buffer,
    pub stats: wgpu::Buffer,
    pub losses: wgpu::Buffer,
    pub config: NrcConfig,
    pub updates: u32,
    backward: wgpu::ComputePipeline,
    adam: wgpu::ComputePipeline,
    empty_group: wgpu::BindGroup,
    frame_prepared: bool,
}

fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

impl OnlineNrc {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: NrcConfig,
    ) -> Result<Self, String> {
        config.validate()?;
        let gradient_bytes = u64::from(config.batch_size) * PARAMETER_COUNT as u64 * 4;
        if gradient_bytes > u64::from(device.limits().max_storage_buffer_binding_size) {
            return Err("online NRC batch exceeds storage binding limit".into());
        }
        let weights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("online NRC weights"),
            contents: bytemuck::cast_slice(&initial_weights(config.seed)),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        });
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("online NRC parameters"),
            contents: bytemuck::bytes_of(&config.params(1)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let records = storage(
            device,
            "online NRC fresh path labels",
            u64::from(config.batch_size) * 80,
        );
        let gradients = storage(device, "online NRC per-label gradients", gradient_bytes);
        let moments = storage(
            device,
            "online NRC Adam moments",
            PARAMETER_COUNT as u64 * 8,
        );
        let stats = storage(device, "online NRC frame stats", 32);
        let losses = storage(
            device,
            "online NRC pre-update losses",
            u64::from(config.batch_size) * 4,
        );
        let entries: Vec<_> = (0..7)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage { read_only: false }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("online NRC group 1"),
            entries: &entries,
        });
        let buffers = [
            &params, &weights, &records, &gradients, &moments, &stats, &losses,
        ];
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, buffer)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: buffer.as_entire_binding(),
            })
            .collect();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("online NRC resources"),
            layout: &layout,
            entries: &entries,
        });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("online NRC unused group 0"),
            entries: &[],
        });
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("online NRC unused group 0"),
            layout: &empty_layout,
            entries: &[],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("online NRC training pipeline layout"),
            bind_group_layouts: &[Some(&empty_layout), Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("online NRC training and inference"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let pipeline = |entry: &'static str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    zero_initialize_workgroup_memory: false,
                    ..Default::default()
                },
                cache: None,
            })
        };
        let backward = pipeline("nrc_backward");
        let adam = pipeline("nrc_adam");
        let nrc = Self {
            layout,
            bind_group,
            params,
            weights,
            records,
            gradients,
            moments,
            stats,
            losses,
            config,
            updates: 0,
            backward,
            adam,
            empty_group,
            frame_prepared: false,
        };
        queue.write_buffer(&nrc.stats, 0, &[0; 32]);
        Ok(nrc)
    }

    /// Upload flags/bounds/rates and clear per-frame counters before encoding the trace pass.
    /// Changing batch size requires recreating the cache; weights otherwise survive changes.
    pub fn begin_frame(&mut self, queue: &wgpu::Queue) -> Result<(), String> {
        self.config.validate()?;
        if u64::from(self.config.batch_size) * 80 != self.records.size() {
            return Err("changing online NRC batch size requires recreating its buffers".into());
        }
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&self.config.params(self.updates.saturating_add(1))),
        );
        queue.write_buffer(&self.stats, 0, &[0; 28]);
        self.frame_prepared = true;
        Ok(())
    }

    /// Encode one Adam update after the uncached label-producing pass. No CPU label roundtrip.
    pub fn encode_training(&mut self, encoder: &mut wgpu::CommandEncoder) -> Result<(), String> {
        self.encode_training_with_timestamps(encoder, None)
    }

    /// Timestamp the beginning of backwards and end of Adam, isolating GPU training cost.
    /// The caller must provide a timestamp-enabled device and two allocated query indices.
    pub fn encode_training_with_timestamps(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        timestamps: Option<(&wgpu::QuerySet, u32)>,
    ) -> Result<(), String> {
        if !self.frame_prepared {
            return Err("online NRC training requires begin_frame".into());
        }
        self.frame_prepared = false;
        if !self.config.training {
            return Ok(());
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("online NRC backwards"),
                timestamp_writes: timestamps.map(|(query_set, base)| {
                    wgpu::ComputePassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: Some(base),
                        end_of_pass_write_index: None,
                    }
                }),
            });
            pass.set_pipeline(&self.backward);
            pass.set_bind_group(0, &self.empty_group, &[]);
            pass.set_bind_group(1, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.config.batch_size.div_ceil(32), 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("online NRC Adam reduction"),
                timestamp_writes: timestamps.map(|(query_set, base)| {
                    wgpu::ComputePassTimestampWrites {
                        query_set,
                        beginning_of_pass_write_index: None,
                        end_of_pass_write_index: Some(base + 1),
                    }
                }),
            });
            pass.set_pipeline(&self.adam);
            pass.set_bind_group(0, &self.empty_group, &[]);
            pass.set_bind_group(1, &self.bind_group, &[]);
            pass.dispatch_workgroups((PARAMETER_COUNT as u32).div_ceil(64), 1, 1);
        }
        self.updates = self.updates.saturating_add(1);
        Ok(())
    }

    /// Reset both weights and optimizer moments when scene/material changes invalidate the field.
    pub fn reset(&mut self, queue: &wgpu::Queue) {
        queue.write_buffer(
            &self.weights,
            0,
            bytemuck::cast_slice(&initial_weights(self.config.seed)),
        );
        queue.write_buffer(&self.moments, 0, &vec![0; PARAMETER_COUNT * 8]);
        queue.write_buffer(&self.stats, 0, &[0; 32]);
        self.updates = 0;
        self.frame_prepared = false;
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_stats(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<NrcFrameStats, String> {
        let stats_bytes = read_buffer(device, queue, &self.stats, 32)?;
        let stats: &[u32] = bytemuck::cast_slice(&stats_bytes);
        let count = stats[5].min(self.config.batch_size);
        let loss = if count == 0 {
            0.0
        } else {
            let bytes = read_buffer(device, queue, &self.losses, u64::from(count) * 4)?;
            let losses: &[f32] = bytemuck::cast_slice(&bytes);
            losses.iter().map(|v| f64::from(*v)).sum::<f64>() / f64::from(count)
        };
        if !loss.is_finite() {
            return Err("online NRC loss is not finite".into());
        }
        Ok(NrcFrameStats {
            requested_records: stats[0],
            cache_queries: stats[1],
            invalid_records: stats[2],
            invalid_updates: stats[3],
            overflow_records: stats[4],
            trained_records: stats[5],
            invalid_predictions: stats[6],
            mean_log_mse: loss,
            updates: stats[7],
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn read_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
    size: u64,
) -> Result<Vec<u8>, String> {
    use std::sync::mpsc;
    use std::time::Duration;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("online NRC readback"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("online NRC readback"),
    });
    encoder.copy_buffer_to_buffer(source, 0, &readback, 0, size);
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(30))
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let bytes = readback
        .get_mapped_range(..)
        .map_err(|e| e.to_string())?
        .to_vec();
    readback.unmap();
    Ok(bytes)
}

/// Deterministic He-uniform hidden layers; output weights are deliberately tiny at warmup.
pub fn initial_weights(seed: u32) -> Vec<f32> {
    let mut state = seed.max(1);
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        2.0 * (state as f64 / u32::MAX as f64) as f32 - 1.0
    };
    let mut weights = vec![0.0; PARAMETER_COUNT];
    for v in &mut weights[..448] {
        *v = random() * (6.0_f32 / 14.0).sqrt();
    }
    for v in &mut weights[480..1504] {
        *v = random() * (6.0_f32 / 32.0).sqrt();
    }
    for v in &mut weights[1536..1632] {
        *v = random() * 0.001;
    }
    weights[1632..].fill(0.01);
    weights
}

/// CPU reference used for gradient and actual GPU parity checks, never in the render loop.
pub fn reference_forward(weights: &[f32], record: &NrcRecord) -> ([f64; 32], [f64; 32], [f64; 3]) {
    assert_eq!(weights.len(), PARAMETER_COUNT);
    let a: [f64; 32] = std::array::from_fn(|j| {
        (f64::from(weights[448 + j])
            + (0..14)
                .map(|i| f64::from(weights[j * 14 + i]) * f64::from(record.features[i / 4][i % 4]))
                .sum::<f64>())
        .max(0.0)
    });
    let b: [f64; 32] = std::array::from_fn(|j| {
        (f64::from(weights[1504 + j])
            + (0..32)
                .map(|i| f64::from(weights[480 + j * 32 + i]) * a[i])
                .sum::<f64>())
        .max(0.0)
    });
    let output = std::array::from_fn(|j| {
        f64::from(weights[1632 + j])
            + (0..32)
                .map(|i| f64::from(weights[1536 + j * 32 + i]) * b[i])
                .sum::<f64>()
    });
    (a, b, output)
}

pub fn reference_gradient(weights: &[f32], record: &NrcRecord, scale: f32) -> (f64, Vec<f64>) {
    let (a, b, output) = reference_forward(weights, record);
    let dc: [f64; 3] = std::array::from_fn(|j| {
        output[j] - (f64::from(record.target[j]) / f64::from(scale)).ln_1p()
    });
    let loss = dc.iter().map(|d| d * d).sum::<f64>() / 3.0;
    let dc = dc.map(|d| d * 2.0 / 3.0);
    let mut gradient = vec![0.0; PARAMETER_COUNT];
    let db: [f64; 32] = std::array::from_fn(|i| {
        if b[i] == 0.0 {
            0.0
        } else {
            (0..3)
                .map(|j| f64::from(weights[1536 + j * 32 + i]) * dc[j])
                .sum()
        }
    });
    let da: [f64; 32] = std::array::from_fn(|i| {
        if a[i] == 0.0 {
            0.0
        } else {
            (0..32)
                .map(|j| f64::from(weights[480 + j * 32 + i]) * db[j])
                .sum()
        }
    });
    for j in 0..3 {
        gradient[1632 + j] = dc[j];
        for i in 0..32 {
            gradient[1536 + j * 32 + i] = dc[j] * b[i];
        }
    }
    for j in 0..32 {
        gradient[1504 + j] = db[j];
        for i in 0..32 {
            gradient[480 + j * 32 + i] = db[j] * a[i];
        }
    }
    for j in 0..32 {
        gradient[448 + j] = da[j];
        for i in 0..14 {
            gradient[j * 14 + i] = da[j] * f64::from(record.features[i / 4][i % 4]);
        }
    }
    (loss, gradient)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_contract_and_deterministic_initialization() {
        assert_eq!(std::mem::size_of::<NrcRecord>(), 80);
        assert_eq!(std::mem::size_of::<NrcParams>(), 80);
        assert_eq!(initial_weights(123), initial_weights(123));
        assert_ne!(initial_weights(123), initial_weights(124));
        assert_eq!(initial_weights(123).len(), 1635);
    }

    #[test]
    fn backpropagation_matches_finite_differences() {
        let mut weights = initial_weights(123);
        let record = NrcRecord {
            features: [
                [0.2, -0.3, 0.4, 0.5],
                [0.3, 0.1, -0.7, 0.6],
                [0.2, 0.4, 0.7, 0.3],
                [0.5, 0.2, 0.0, 0.0],
            ],
            target: [2.0, 0.5, 1.0, 0.0],
        };
        let (_, gradients) = reference_gradient(&weights, &record, 1.0);
        for i in [
            0, 67, 219, 447, 459, 490, 1097, 1504, 1536, 1595, 1632, 1634,
        ] {
            let old = weights[i];
            let h = 0.001;
            weights[i] = old + h;
            let up = reference_gradient(&weights, &record, 1.0).0;
            weights[i] = old - h;
            let down = reference_gradient(&weights, &record, 1.0).0;
            weights[i] = old;
            let numeric = (up - down) / f64::from(2.0 * h);
            assert!(
                (numeric - gradients[i]).abs() < 2e-4,
                "parameter {i}: {numeric} != {}",
                gradients[i]
            );
        }
    }

    #[test]
    fn invalid_config_rejected() {
        let mut config = NrcConfig::default();
        assert!(config.validate().is_ok());
        config.radiance_scale = 0.0;
        assert!(config.validate().is_err());
        config.radiance_scale = 1.0;
        config.batch_size = 0;
        assert!(config.validate().is_err());
        config.batch_size = 128;
        config.extent[0] = f32::NAN;
        assert!(config.validate().is_err());
    }
}
