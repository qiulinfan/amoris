//! Writes the scene's changed instance slots to the GPU (docs/bench/dynamic-scenes.md).
//!
//! `Queue::write_buffer` stages every call through a staging buffer of its own: the scene sync of
//! slots spread one per run cost 5 to 18 us of CPU per run on Direct3D 12 and 1.5 on Vulkan here
//! (docs/bench/dynamic-scenes.md 2), so a write per run costs milliseconds once moving instances are
//! spread over the slots. A few runs are written one write each; more runs go in one write per
//! batch, their slots' indices and contents, and a compute pass at the start of the frame scatters
//! them into the instance buffer before any pass reads it. The instance buffer is only ever
//! written from the CPU's copy, so a slot reaches the GPU the same either way.

use std::num::NonZeroU64;
use std::ops::Range;

use crate::scene::InstanceGpu;

/// At most this many runs are written one write each; more are scattered.
pub const DIRECT_RUNS: usize = 8;
/// An instance slot's 32-bit words (80 bytes, `Instance` in common.wgsl).
const WORDS: u64 = 20;
const GROUP: u32 = 256;
/// Workgroups per dispatch row (scatter.wgsl `ROW`).
const ROW: u32 = 65535;

pub struct InstanceUpload {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    /// One per batch: the count, the slot indices and the slots' contents (scatter.wgsl `src`).
    sources: Vec<Source>,
    /// Slots per batch: as many as a source the device binds whole holds (WebGPU's default
    /// 128 MiB binding: about 1.6M slots).
    batch: u32,
    /// The scattered slots' indices, reused from frame to frame.
    indices: Vec<u32>,
    /// Slots written to each source for this frame's scatter pass.
    pending: Vec<u32>,
}

struct Source {
    buffer: wgpu::Buffer,
    /// The bind group of `buffer` and the instance buffer it was made for.
    group: Option<(wgpu::Buffer, wgpu::BindGroup)>,
}

/// The bytes of a batch of `n` slots in its source: the count, the indices, the contents.
fn batch_bytes(n: u64) -> u64 {
    (1 + n * (1 + WORDS)) * 4
}

/// `runs` cut into batches of at most `cap` slots each, in order; a run may be cut in two.
fn batches(runs: &[Range<u32>], cap: u32) -> Vec<Vec<Range<u32>>> {
    let mut out = Vec::new();
    let mut batch = Vec::new();
    let mut room = cap;
    for run in runs {
        let mut run = run.clone();
        while !run.is_empty() {
            if room == 0 {
                out.push(std::mem::take(&mut batch));
                room = cap;
            }
            let take = (run.end - run.start).min(room);
            batch.push(run.start..run.start + take);
            run.start += take;
            room -= take;
        }
    }
    if !batch.is_empty() {
        out.push(batch);
    }
    out
}

impl InstanceUpload {
    pub fn new(device: &wgpu::Device) -> InstanceUpload {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scatter"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/scatter.wgsl").into()),
        });
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scatter"),
            entries: &[storage(0, true), storage(1, false)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scatter"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        // A source is bound whole.
        let limits = device.limits();
        let limit = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let batch = ((limit / 4 - 1) / (1 + WORDS)).min(u64::from(u32::MAX)) as u32;
        InstanceUpload {
            pipeline: device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("scatter"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("main"),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            }),
            layout,
            sources: Vec::new(),
            batch,
            indices: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Writes the `runs` of `slots` to `instances`: a few runs directly, more into the scatter
    /// pass's sources for [`InstanceUpload::encode`] to copy over. `runs` are sorted and disjoint.
    pub fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &wgpu::Buffer,
        slots: &[InstanceGpu],
        runs: &[Range<u32>],
    ) {
        self.pending.clear();
        if runs.len() <= DIRECT_RUNS {
            for run in runs {
                let s = &slots[run.start as usize..run.end as usize];
                queue.write_buffer(
                    instances,
                    u64::from(run.start) * 80,
                    bytemuck::cast_slice(s),
                );
            }
            return;
        }
        let n: u64 = runs.iter().map(|r| u64::from(r.end - r.start)).sum();
        if n <= u64::from(self.batch) {
            self.write_batch(0, device, queue, instances, slots, runs);
        } else {
            for (k, batch) in batches(runs, self.batch).iter().enumerate() {
                self.write_batch(k, device, queue, instances, slots, batch);
            }
        }
    }

    /// Writes one batch of at most `batch` slots to the `k`-th source.
    fn write_batch(
        &mut self,
        k: usize,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &wgpu::Buffer,
        slots: &[InstanceGpu],
        runs: &[Range<u32>],
    ) {
        let n: u64 = runs.iter().map(|r| u64::from(r.end - r.start)).sum();
        let bytes = batch_bytes(n);
        if self.sources.len() <= k {
            self.sources.push(Source {
                buffer: source(device, 4096),
                group: None,
            });
        }
        let src = &mut self.sources[k];
        // A replaced instance buffer is not kept alive by the old bind group.
        if src.group.as_ref().is_some_and(|(b, _)| b != instances) {
            src.group = None;
        }
        if bytes > src.buffer.size() {
            let cap = batch_bytes(u64::from(self.batch));
            src.buffer = source(device, bytes.next_power_of_two().min(cap));
            src.group = None;
        }
        if src.group.is_none() {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scatter"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: src.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: instances.as_entire_binding(),
                    },
                ],
            });
            src.group = Some((instances.clone(), group));
        }
        let Some(size) = NonZeroU64::new(bytes) else {
            return;
        };
        let Some(mut view) = queue.write_buffer_with(&src.buffer, 0, size) else {
            return;
        };
        self.indices.clear();
        for run in runs {
            self.indices.extend(run.clone());
        }
        let header = (1 + n as usize) * 4;
        view.slice(..4)
            .copy_from_slice(bytemuck::bytes_of(&(n as u32)));
        view.slice(4..header)
            .copy_from_slice(bytemuck::cast_slice(&self.indices));
        let mut at = header;
        for run in runs {
            let s: &[u8] = bytemuck::cast_slice(&slots[run.start as usize..run.end as usize]);
            view.slice(at..at + s.len()).copy_from_slice(s);
            at += s.len();
        }
        self.pending.resize(k + 1, 0);
        self.pending[k] = n as u32;
    }

    /// Scatters the slots [`InstanceUpload::write`] staged into the instance buffer; first in the
    /// frame, before any pass reads an instance.
    pub fn encode(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        timestamps: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        if !self.pending() {
            return;
        }
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("scatter instances"),
            timestamp_writes: timestamps,
        });
        pass.set_pipeline(&self.pipeline);
        for (src, &n) in self.sources.iter().zip(&self.pending) {
            let Some((_, group)) = src.group.as_ref().filter(|_| n > 0) else {
                continue;
            };
            let groups = (u64::from(n) * WORDS).div_ceil(u64::from(GROUP)) as u32;
            let (x, y) = if groups > ROW {
                (ROW, groups.div_ceil(ROW))
            } else {
                (groups, 1)
            };
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(x, y, 1);
        }
        drop(pass);
        self.pending.clear();
    }

    /// Whether [`InstanceUpload::encode`] has slots to scatter this frame.
    pub fn pending(&self) -> bool {
        self.pending.iter().any(|&n| n > 0)
    }
}

fn source(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("scattered instances"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{BackendChoice, Gpu};

    #[test]
    fn batches_cover_every_slot_once_in_order() {
        let runs = [0..3, 5..6, 8..20, 21..22, 30..31];
        for cap in [1, 2, 4, 5, 13, 17, 100] {
            let b = batches(&runs, cap);
            assert!(b.iter().all(|batch| {
                let n: u32 = batch.iter().map(|r| r.end - r.start).sum();
                n > 0 && n <= cap
            }));
            let flat: Vec<u32> = b.iter().flatten().flat_map(Clone::clone).collect();
            let want: Vec<u32> = runs.iter().flat_map(Clone::clone).collect();
            assert_eq!(flat, want, "cap {cap}");
        }
        assert!(batches(&[], 4).is_empty());
    }

    /// A slot whose every word says which slot and which round wrote it.
    fn slot(i: u32, round: u32) -> InstanceGpu {
        let mut words = [0u32; WORDS as usize];
        for (w, x) in words.iter_mut().enumerate() {
            *x = (i << 12) ^ (round << 5) ^ w as u32;
        }
        bytemuck::cast(words)
    }

    /// Every way `write` takes leaves the instance buffer equal to the CPU's slots: a few runs
    /// written directly, many scattered in one batch, and many split into batches (a device whose
    /// binding limit holds fewer slots than a frame changes, as at WebGPU's 128 MiB). Skipped
    /// without a GPU; `AMORIS_BACKEND` and `AMORIS_ADAPTER` pick it.
    #[test]
    fn written_slots_reach_the_instance_buffer() {
        let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
            eprintln!("no GPU, skipped");
            return;
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let count = 4000u32;
        let size = u64::from(count) * 80;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut slots: Vec<InstanceGpu> = (0..count).map(|i| slot(i, 0)).collect();
        let mut upload = InstanceUpload::new(device);
        let full = upload.batch;
        // Runs of 1 to 4 slots, 1 to 6 apart.
        let spread: Vec<Range<u32>> = {
            let (mut v, mut at, mut k) = (Vec::new(), 0u32, 0u32);
            while at < count {
                let len = (1 + k % 4).min(count - at);
                v.push(at..at + len);
                at += len + 1 + (k * 7) % 6;
                k += 1;
            }
            v
        };
        let cases: [(&str, u32, Vec<Range<u32>>); 5] = [
            (
                "everything, one run",
                full,
                std::iter::once(0..count).collect(),
            ),
            ("a few runs, direct", full, vec![3..9, 100..101, 2000..2500]),
            ("spread, one batch", full, spread.clone()),
            ("spread, batches of 37", 37, spread.clone()),
            ("spread, batches of one", 1, spread[..40].to_vec()),
        ];
        for (round, (name, batch, runs)) in cases.into_iter().enumerate() {
            for run in &runs {
                for i in run.clone() {
                    slots[i as usize] = slot(i, round as u32 + 1);
                }
            }
            upload.batch = batch;
            upload.write(device, queue, &instances, &slots, &runs);
            let mut enc = device.create_command_encoder(&Default::default());
            upload.encode(&mut enc, None);
            enc.copy_buffer_to_buffer(&instances, 0, &readback, 0, size);
            queue.submit([enc.finish()]);
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            let got = readback
                .slice(..)
                .get_mapped_range()
                .expect("mapped")
                .to_vec();
            readback.unmap();
            assert!(
                got == bytemuck::cast_slice::<_, u8>(&slots),
                "{} ({}): {name}",
                gpu.backend_name(),
                gpu.info.name
            );
        }
        assert!(upload.sources.len() > 1, "the batches used several sources");
    }
}
