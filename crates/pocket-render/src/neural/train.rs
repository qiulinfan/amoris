//! The neural texture encoder's trainer (docs/spec/neural-textures.md 4): every step runs on the
//! GPU (shaders/neural_train.wgsl) and nothing returns to the CPU until the end, when the latents
//! are quantized and the network rounded to halves into a [`NeuralTexture`]. Deterministic for a
//! seed on one device and backend: random numbers are hashes of (seed, step, sample), latent
//! gradients are integer sums, network gradients are summed in a fixed order.

use pocket_assets::neural::{
    Channel, LatentGrid, LatentLevel, NeuralLayout, NeuralTexture, f32_to_f16, level_dims, mip_size,
};
use wgpu::util::DeviceExt;

use super::profile_constants;
use crate::gi::nrc::read_buffer;

const TRAIN: &str = include_str!("../../shaders/neural_train.wgsl");
/// Samples per chunk of the network's gradient sum.
const CHUNK: u32 = 256;
/// Steps per command buffer.
const STEPS_PER_SUBMIT: u32 = 20;

/// A material's channels and their mip chain, values in [0, 1] as stored (sRGB-encoded color).
#[derive(Clone, Debug)]
pub struct Reference {
    pub width: u32,
    pub height: u32,
    pub channels: Vec<Channel>,
    /// Per mip, texel-major, channels fastest.
    pub mips: Vec<Vec<f32>>,
}

/// sRGB-encoded to linear.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

impl Reference {
    /// Builds `mips` levels from mip 0 by 2x2 box filtering: color in linear light, normals as
    /// vectors renormalized, everything else as stored.
    pub fn new(
        width: u32,
        height: u32,
        channels: Vec<Channel>,
        mip0: Vec<f32>,
        mips: u32,
    ) -> Result<Reference, String> {
        let c = channels.len();
        if mip0.len() != (width * height) as usize * c {
            return Err("reference mip 0 has the wrong size".into());
        }
        let nx = channels.iter().position(|&ch| ch == Channel::NormalX);
        let mut chain = vec![mip0];
        for m in 1..mips {
            let [w, h] = mip_size(width, height, m);
            let [pw, ph] = mip_size(width, height, m - 1);
            let prev = &chain[(m - 1) as usize];
            let mut out = vec![0.0f32; (w * h) as usize * c];
            for y in 0..h {
                for x in 0..w {
                    let mut taps = Vec::with_capacity(4);
                    for dy in 0..(ph / h).max(1) {
                        for dx in 0..(pw / w).max(1) {
                            taps.push(
                                (((y * (ph / h).max(1) + dy) * pw + x * (pw / w).max(1) + dx)
                                    as usize)
                                    * c,
                            );
                        }
                    }
                    let n = taps.len() as f32;
                    let at = (y * w + x) as usize * c;
                    for (i, ch) in channels.iter().enumerate() {
                        if Some(i) == nx || Some(i) == nx.map(|j| j + 1) {
                            continue;
                        }
                        out[at + i] = if ch.srgb() {
                            linear_to_srgb(
                                taps.iter()
                                    .map(|&t| srgb_to_linear(prev[t + i]))
                                    .sum::<f32>()
                                    / n,
                            )
                        } else {
                            taps.iter().map(|&t| prev[t + i]).sum::<f32>() / n
                        };
                    }
                    if let Some(j) = nx {
                        let mut v = [0.0f32; 3];
                        for &t in &taps {
                            let nx = prev[t + j] * 2.0 - 1.0;
                            let ny = prev[t + j + 1] * 2.0 - 1.0;
                            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                            v[0] += nx;
                            v[1] += ny;
                            v[2] += nz;
                        }
                        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
                        out[at + j] = (v[0] / len) * 0.5 + 0.5;
                        out[at + j + 1] = (v[1] / len) * 0.5 + 0.5;
                    }
                }
            }
            chain.push(out);
        }
        Ok(Reference {
            width,
            height,
            channels,
            mips: chain,
        })
    }
}

/// How to train.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TrainConfig {
    pub steps: u32,
    /// Samples per step (a multiple of 256).
    pub batch: u32,
    pub seed: u32,
    pub lr_mlp: f32,
    pub lr_latent: f32,
    /// The share of samples taken at texel centres (the rest anywhere, against the bilinearly
    /// filtered reference).
    pub center_fraction: f32,
    /// The fraction of the steps after which latents are rounded (before it: noise).
    pub round_from: f32,
    /// The fraction of the steps after which latents are frozen and only the network trains.
    pub freeze_from: f32,
    /// The learning rate's final value as a fraction of the first (cosine decay).
    pub final_lr: f32,
    /// Mip `m` is sampled in proportion to `mip_decay^m`.
    pub mip_decay: f32,
    /// Loss weight per channel (1 when empty or shorter).
    pub channel_weights: Vec<f32>,
}

impl Default for TrainConfig {
    fn default() -> TrainConfig {
        TrainConfig {
            steps: 6000,
            batch: 65536,
            seed: 1,
            lr_mlp: 0.005,
            lr_latent: 0.01,
            center_fraction: 0.5,
            round_from: 0.6,
            freeze_from: 0.85,
            final_lr: 0.05,
            mip_decay: 0.5,
            channel_weights: Vec::new(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TrainUniform {
    shape: [u32; 4],
    steps: [u32; 4],
    rates: [f32; 4],
    sizes: [u32; 4],
    channel_weights: [f32; 16],
}

/// xorshift32 in [-1, 1) (initialization only).
struct Init(u32);

impl Init {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// The network's starting weights: He-uniform hidden layers, a small output layer whose biases
/// start at the channels' means; padding zero.
pub fn initial_weights(layout: &NeuralLayout, seed: u32, means: &[f32]) -> Vec<f32> {
    let mut rng = Init(seed.wrapping_mul(2_654_435_761).max(1));
    let mut w = vec![0.0f32; layout.weight_count()];
    let fans = [layout.inputs(), layout.hidden[0], layout.hidden[1]];
    for (layer, (&(outputs, inputs), &(wo, bo))) in
        layout.layers().iter().zip(&layout.offsets()).enumerate()
    {
        let bound = (6.0 / fans[layer] as f32).sqrt() * if layer == 2 { 0.1 } else { 1.0 };
        for i in 0..(outputs * inputs) as usize {
            w[wo + i] = rng.next() * bound;
        }
        if layer == 2 {
            for (i, m) in means.iter().enumerate() {
                w[bo + i] = *m;
            }
        }
    }
    for (i, v) in w.iter_mut().enumerate() {
        if layout.is_padding(i) {
            *v = 0.0;
        }
    }
    w
}

pub struct Trainer {
    pub layout: NeuralLayout,
    pub config: TrainConfig,
    width: u32,
    height: u32,
    mips: u32,
    /// Per level, (fine first value, coarse first value).
    latent_firsts: Vec<[u32; 2]>,
    latent_values: u32,
    params: u32,
    latents: wgpu::Buffer,
    weights: wgpu::Buffer,
    history: wgpu::Buffer,
    pipelines: Vec<(wgpu::ComputePipeline, wgpu::BindGroup)>,
    step: u32,
}

fn storage(device: &wgpu::Device, label: &str, contents: &[u8]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
    })
}

fn zeroed(device: &wgpu::Device, label: &str, bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.max(16),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Trainer {
    pub fn new(
        device: &wgpu::Device,
        reference: &Reference,
        layout: NeuralLayout,
        config: TrainConfig,
    ) -> Result<Trainer, String> {
        layout.validate()?;
        if layout.channels != reference.channels {
            return Err("the layout's channels are not the reference's".into());
        }
        if config.batch == 0 || !config.batch.is_multiple_of(CHUNK) {
            return Err(format!("the batch must be a positive multiple of {CHUNK}"));
        }
        if config.steps == 0 {
            return Err("training needs at least one step".into());
        }
        let (width, height) = (reference.width, reference.height);
        let mips = reference.mips.len() as u32;
        let channels = layout.channels.len();
        let dims = level_dims(&layout, width, height, mips);
        let levels = dims.len() as u32;

        // Tables: grids per level, then mips.
        let mut tables: Vec<[u32; 4]> = Vec::new();
        let mut latent_firsts = Vec::new();
        let mut value = 0u32;
        for d in &dims {
            let fine_first = value;
            value += d.fine[0] * d.fine[1] * layout.fine.features;
            let coarse_first = value;
            value += d.coarse[0] * d.coarse[1] * layout.coarse.features;
            tables.push([d.fine[0], d.fine[1], fine_first, 0]);
            tables.push([d.coarse[0], d.coarse[1], coarse_first, 0]);
            latent_firsts.push([fine_first, coarse_first]);
        }
        let latent_values = value;
        let total: f32 = (0..mips).map(|m| config.mip_decay.powi(m as i32)).sum();
        let mut cdf = 0.0f32;
        let mut first = 0u32;
        for m in 0..mips {
            let [w, h] = mip_size(width, height, m);
            cdf += config.mip_decay.powi(m as i32) / total;
            let c = if m + 1 == mips { 1.0 } else { cdf };
            tables.push([w, h, first, c.to_bits()]);
            first += w * h * channels as u32;
        }
        let reference_values: Vec<f32> = reference.mips.concat();

        let mut rng = Init(config.seed.wrapping_mul(0x9e37_79b9) ^ 0x5bd1_e995);
        let latents: Vec<f32> = (0..latent_values)
            .map(|_| 0.5 + 0.25 * rng.next())
            .collect();
        let means: Vec<f32> = (0..channels)
            .map(|c| {
                let mip0 = &reference.mips[0];
                mip0.iter().skip(c).step_by(channels).sum::<f32>() / (mip0.len() / channels) as f32
            })
            .collect();
        let weights = initial_weights(&layout, config.seed, &means);
        let params = weights.len() as u32;
        let chunks = config.batch / CHUNK;
        let record = layout.inputs_padded()
            + 2 * layout.hidden[0]
            + 2 * layout.hidden[1]
            + layout.outputs_padded();

        let mut channel_weights = [1.0f32; 16];
        for (slot, w) in channel_weights.iter_mut().zip(&config.channel_weights) {
            *slot = *w;
        }
        let uniform = TrainUniform {
            shape: [config.batch, chunks, mips, levels],
            steps: [
                config.seed,
                config.steps,
                (config.round_from * config.steps as f32) as u32,
                (config.freeze_from * config.steps as f32) as u32,
            ],
            rates: [
                config.lr_mlp,
                config.lr_latent,
                config.center_fraction,
                config.final_lr,
            ],
            sizes: [latent_values, 0, 0, 0],
            channel_weights,
        };
        let limits = device.limits();
        let records_bytes = u64::from(config.batch) * u64::from(record) * 4;
        if records_bytes > limits.max_storage_buffer_binding_size
            || records_bytes > limits.max_buffer_size
        {
            return Err(format!(
                "a batch of {} needs {} MB of records; this device binds at most {} MB",
                config.batch,
                records_bytes >> 20,
                limits.max_storage_buffer_binding_size >> 20
            ));
        }

        let buffers = [
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("neural train params"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
            storage(device, "neural train tables", bytemuck::cast_slice(&tables)),
            storage(
                device,
                "neural train reference",
                bytemuck::cast_slice(&reference_values),
            ),
            storage(device, "neural latents", bytemuck::cast_slice(&latents)),
            zeroed(
                device,
                "neural latent gradients",
                u64::from(latent_values) * 4,
            ),
            zeroed(
                device,
                "neural latent moments",
                u64::from(latent_values) * 8,
            ),
            storage(device, "neural weights", bytemuck::cast_slice(&weights)),
            zeroed(
                device,
                "neural gradient partials",
                u64::from(chunks) * u64::from(params) * 4,
            ),
            zeroed(device, "neural weight moments", u64::from(params) * 8),
            zeroed(device, "neural records", records_bytes),
            zeroed(device, "neural losses", u64::from(config.batch) * 4),
            zeroed(device, "neural train state", 16),
            zeroed(device, "neural loss history", u64::from(config.steps) * 4),
        ];
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("neural train"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}\n{TRAIN}", profile_constants(&layout)).into(),
            ),
        });
        let kernels: [(&str, &[u32]); 5] = [
            ("forward_backward", &[0, 1, 2, 3, 4, 6, 9, 10, 11]),
            ("mlp_partials", &[0, 7, 9]),
            ("mlp_adam", &[0, 6, 7, 8, 11]),
            ("latent_adam", &[0, 3, 4, 5, 11]),
            ("finish_step", &[0, 10, 11, 12]),
        ];
        let pipelines = kernels
            .iter()
            .map(|(entry, bindings)| {
                let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: None,
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: crate::shaders::compute_options(),
                    cache: None,
                });
                let entries: Vec<_> = bindings
                    .iter()
                    .map(|&b| wgpu::BindGroupEntry {
                        binding: b,
                        resource: buffers[b as usize].as_entire_binding(),
                    })
                    .collect();
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(entry),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &entries,
                });
                (pipeline, group)
            })
            .collect();
        let [_, _, _, latents, _, _, weights, _, _, _, _, _, history] = buffers;
        Ok(Trainer {
            layout,
            config,
            width,
            height,
            mips,
            latent_firsts,
            latent_values,
            params,
            latents,
            weights,
            history,
            pipelines,
            step: 0,
        })
    }

    /// Steps run so far.
    pub fn step(&self) -> u32 {
        self.step
    }

    /// Records `n` steps (no more than remain).
    pub fn encode_steps(&mut self, enc: &mut wgpu::CommandEncoder, n: u32) {
        let n = n.min(self.config.steps - self.step);
        let batch_groups = self.config.batch / 64;
        let latent_groups = self.latent_values.div_ceil(256);
        let lx = latent_groups.min(32768);
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("neural train"),
            timestamp_writes: None,
        });
        for _ in 0..n {
            let dispatches = [
                (batch_groups.min(32768), batch_groups.div_ceil(32768)),
                (self.params.div_ceil(64), self.config.batch / CHUNK),
                (self.params.div_ceil(64), 1),
                (lx, latent_groups.div_ceil(lx)),
                (1, 1),
            ];
            for ((pipeline, group), (x, y)) in self.pipelines.iter().zip(dispatches) {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups(x, y, 1);
            }
        }
        self.step += n;
    }

    /// Runs every remaining step; `progress` gets (steps done, the last step's mean loss) every
    /// `report_every` steps.
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        report_every: u32,
        progress: &mut dyn FnMut(u32, f32),
    ) -> Result<(), String> {
        let mut next_report = report_every.max(1);
        while self.step < self.config.steps {
            let mut enc = device.create_command_encoder(&Default::default());
            self.encode_steps(&mut enc, STEPS_PER_SUBMIT);
            queue.submit([enc.finish()]);
            if self.step >= next_report || self.step == self.config.steps {
                let history = self.history(device, queue)?;
                progress(self.step, history[(self.step - 1) as usize]);
                next_report += report_every.max(1);
            }
        }
        Ok(())
    }

    /// The mean loss of every step run so far.
    pub fn history(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Vec<f32>, String> {
        let bytes = read_buffer(
            device,
            queue,
            &self.history,
            u64::from(self.step.max(1)) * 4,
        )?;
        let mut h: Vec<f32> = bytemuck::cast_slice(&bytes).to_vec();
        h.truncate(self.step as usize);
        Ok(h)
    }

    /// The trained texture: latents rounded to their bits, the network to halves.
    pub fn finish(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
    ) -> Result<NeuralTexture, String> {
        let latents_bytes = read_buffer(
            device,
            queue,
            &self.latents,
            u64::from(self.latent_values) * 4,
        )?;
        let latents: &[f32] = bytemuck::cast_slice(&latents_bytes);
        let weight_bytes = read_buffer(device, queue, &self.weights, u64::from(self.params) * 4)?;
        let weights: &[f32] = bytemuck::cast_slice(&weight_bytes);
        if let Some(i) = weights.iter().position(|w| !w.is_finite()) {
            return Err(format!("training diverged: weight {i} is not finite"));
        }
        let dims = level_dims(&self.layout, self.width, self.height, self.mips);
        let mut levels = Vec::new();
        for (d, firsts) in dims.iter().zip(&self.latent_firsts) {
            let pack = |size: [u32; 2], first: u32, spec: pocket_assets::neural::GridSpec| {
                let texels = (size[0] * size[1]) as usize;
                let mut words = vec![0u32; 2 * texels];
                for t in 0..texels {
                    for f in 0..spec.features {
                        let z = latents[first as usize + t * spec.features as usize + f as usize];
                        let q = quantize(z, spec.bits);
                        let bit = f * spec.bits;
                        words[2 * t + (bit / 32) as usize] |= q << (bit % 32);
                    }
                }
                LatentGrid {
                    width: size[0],
                    height: size[1],
                    words,
                }
            };
            levels.push(LatentLevel {
                fine: pack(d.fine, firsts[0], self.layout.fine),
                coarse: pack(d.coarse, firsts[1], self.layout.coarse),
            });
        }
        let halves = weights
            .iter()
            .enumerate()
            .map(|(i, &w)| {
                if self.layout.is_padding(i) {
                    0
                } else {
                    f32_to_f16(w)
                }
            })
            .collect();
        let texture = NeuralTexture {
            name: name.to_owned(),
            width: self.width,
            height: self.height,
            mip_count: self.mips,
            layout: self.layout.clone(),
            levels,
            weights: halves,
        };
        texture.validate()?;
        Ok(texture)
    }
}

/// A latent's stored value: `round(clamp(z) * (2^bits - 1))`, ties to even as WGSL's `round`.
pub fn quantize(z: f32, bits: u32) -> u32 {
    let levels = ((1u32 << bits) - 1) as f32;
    (z.clamp(0.0, 1.0) * levels).round_ties_even() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_average_color_in_linear_light_and_renormalize_normals() {
        let channels = vec![
            Channel::BaseR,
            Channel::BaseG,
            Channel::BaseB,
            Channel::NormalX,
            Channel::NormalY,
            Channel::Roughness,
        ];
        // 2x1: black and white, normals tilted left and right, roughness 0 and 1.
        let n = 0.6f32;
        let mip0 = vec![
            0.0,
            0.0,
            0.0,
            0.5 - n / 2.0,
            0.5,
            0.0, //
            1.0,
            1.0,
            1.0,
            0.5 + n / 2.0,
            0.5,
            1.0,
        ];
        let r = Reference::new(2, 1, channels, mip0, 2).unwrap();
        let m1 = &r.mips[1];
        let mid = linear_to_srgb(0.5);
        assert!((m1[0] - mid).abs() < 1e-5, "{} != {mid}", m1[0]);
        // The average of (-0.6, 0, 0.8) and (0.6, 0, 0.8) renormalizes to straight up.
        assert!((m1[3] - 0.5).abs() < 1e-6 && (m1[4] - 0.5).abs() < 1e-6);
        assert!((m1[5] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn quantization_rounds_ties_to_even_and_clamps() {
        assert_eq!(quantize(0.5 / 15.0, 4), 0);
        assert_eq!(quantize(1.5 / 15.0, 4), 2);
        assert_eq!(quantize(1.2, 8), 255);
        assert_eq!(quantize(-0.1, 8), 0);
    }
}
