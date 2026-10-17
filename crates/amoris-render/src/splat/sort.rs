//! The GPU radix sort (splat_sort.wgsl; docs/spec/splats.md 5): stable LSD, 8 bits per pass, with
//! the key count and the tile dispatch read from a control buffer the GPU wrote, so the CPU never
//! waits for the number of visible splats.

use crate::shaders;

/// Keys per tile (one workgroup); matches `TILE` in splat_sort.wgsl and `SORT_TILE` in
/// splat_common.wgsl.
pub const TILE: u32 = 4096;
const BINS: u64 = 256;
/// Uniform offset alignment WebGPU guarantees.
const PASS_STRIDE: u64 = 256;

/// The control buffer's layout (shared with splat_preprocess.wgsl): the key count at byte 0, the
/// indirect dispatch (tiles, 1, 1) at [`DISPATCH_OFFSET`].
pub const DISPATCH_OFFSET: u64 = 16;

pub struct RadixSort {
    layout: wgpu::BindGroupLayout,
    histogram: wgpu::ComputePipeline,
    scan: wgpu::ComputePipeline,
    scatter: wgpu::ComputePipeline,
    passes: wgpu::Buffer,
}

/// Bind groups over one set of buffers: `[0]` reads A and writes B, `[1]` the reverse.
pub struct SortBinding {
    groups: [wgpu::BindGroup; 2],
}

/// The bytes of the histogram buffer for `capacity` keys.
pub fn hist_bytes(capacity: u32) -> u64 {
    (u64::from(capacity.div_ceil(TILE)) + 1) * BINS * 4
}

impl RadixSort {
    pub fn new(device: &wgpu::Device) -> RadixSort {
        let cs = wgpu::ShaderStages::COMPUTE;
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: cs,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat sort"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: cs,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(4, false),
                storage(5, false),
                storage(6, false),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splat sort"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = shaders::module(device, "splat_sort");
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        // One 16-byte uniform per pass at 256-byte strides: the digit's shift.
        let mut shifts = vec![0u8; (PASS_STRIDE * 4) as usize];
        for p in 0..4u32 {
            let o = (u64::from(p) * PASS_STRIDE) as usize;
            shifts[o..o + 4].copy_from_slice(&(p * 8).to_le_bytes());
        }
        let passes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat sort passes"),
            size: shifts.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        if let Ok(mut m) = passes.slice(..).get_mapped_range_mut() {
            m.copy_from_slice(&shifts);
        }
        passes.unmap();
        let [histogram, scan, scatter] =
            crate::par::map(["histogram", "scan_bins", "scatter"], pipe);
        RadixSort {
            histogram,
            scan,
            scatter,
            layout,
            passes,
        }
    }

    /// Binds a control buffer (key count at byte 0), two key and two value buffers and the
    /// histogram buffer ([`hist_bytes`]).
    pub fn bind(
        &self,
        device: &wgpu::Device,
        control: &wgpu::Buffer,
        keys: [&wgpu::Buffer; 2],
        vals: [&wgpu::Buffer; 2],
        hist: &wgpu::Buffer,
    ) -> SortBinding {
        let group = |from: usize| {
            let to = 1 - from;
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("splat sort"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.passes,
                            offset: 0,
                            size: wgpu::BufferSize::new(16),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: control.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: keys[from].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: vals[from].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: keys[to].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: vals[to].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: hist.as_entire_binding(),
                    },
                ],
            })
        };
        SortBinding {
            groups: [group(0), group(1)],
        }
    }

    /// Records `passes` (1 to 4) passes, sorting by the low `8 * passes` bits; the result is in
    /// buffer A when `passes` is even and B when odd. The tile dispatch is read from `control` at
    /// [`DISPATCH_OFFSET`].
    pub fn encode(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        binding: &SortBinding,
        passes: u32,
        control: &wgpu::Buffer,
    ) {
        for p in 0..passes.min(4) {
            let bg = &binding.groups[(p & 1) as usize];
            let offset = [(u64::from(p) * PASS_STRIDE) as u32];
            pass.set_bind_group(0, bg, &offset);
            pass.set_pipeline(&self.histogram);
            pass.dispatch_workgroups_indirect(control, DISPATCH_OFFSET);
            pass.set_pipeline(&self.scan);
            pass.dispatch_workgroups(BINS as u32, 1, 1);
            pass.set_pipeline(&self.scatter);
            pass.dispatch_workgroups_indirect(control, DISPATCH_OFFSET);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::gpu::{BackendChoice, Gpu};

    #[test]
    fn the_tile_matches_the_shaders() {
        assert!(shaders::source("splat_sort").contains(&format!("const TILE: u32 = {TILE}u;")));
        assert!(
            shaders::source("splat_preprocess")
                .contains(&format!("const SORT_TILE: u32 = {TILE}u;"))
        );
        let batch = super::super::BATCH;
        assert!(
            shaders::source("splat_draw").contains(&format!("const DRAW_BATCH: u32 = {batch}u;"))
        );
    }

    fn storage(device: &wgpu::Device, bytes: u64, extra: wgpu::BufferUsages) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes.max(16),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC
                | extra,
            mapped_at_creation: false,
        })
    }

    fn read(gpu: &Gpu, buf: &wgpu::Buffer, n: usize) -> Vec<u32> {
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (n as u64 * 4).max(16),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(buf, 0, &staging, 0, (n as u64 * 4).max(16));
        gpu.queue.submit([enc.finish()]);
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        bytemuck::cast_slice::<u8, u32>(&staging.slice(..).get_mapped_range().expect("mapped"))[..n]
            .to_vec()
    }

    /// Sorts random keys (with many duplicates, so stability shows) on the GPU and compares with a
    /// stable CPU sort. Skipped without a GPU.
    #[test]
    fn sorts_like_a_stable_cpu_sort() {
        let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
            eprintln!("no GPU: skipped");
            return;
        };
        let device = &gpu.device;
        let sort = RadixSort::new(device);
        for (n, passes, mask) in [
            (1usize, 4u32, u32::MAX),
            (5000, 4, u32::MAX),
            (100_003, 3, 0xff_ffff),
            (70_000, 2, 0x3ff),
        ] {
            let mut x = 0x1234_5678u32;
            let keys: Vec<u32> = (0..n)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x & mask
                })
                .collect();
            let vals: Vec<u32> = (0..n as u32).collect();
            let cap = n as u64 * 4;
            let ka = storage(device, cap, wgpu::BufferUsages::empty());
            let kb = storage(device, cap, wgpu::BufferUsages::empty());
            let va = storage(device, cap, wgpu::BufferUsages::empty());
            let vb = storage(device, cap, wgpu::BufferUsages::empty());
            let hist = storage(device, hist_bytes(n as u32), wgpu::BufferUsages::empty());
            let control = storage(device, 64, wgpu::BufferUsages::INDIRECT);
            gpu.queue.write_buffer(&ka, 0, bytemuck::cast_slice(&keys));
            gpu.queue.write_buffer(&va, 0, bytemuck::cast_slice(&vals));
            let tiles = (n as u32).div_ceil(TILE);
            gpu.queue.write_buffer(
                &control,
                0,
                bytemuck::cast_slice(&[n as u32, 0, 0, 0, tiles, 1, 1, 0]),
            );
            let binding = sort.bind(device, &control, [&ka, &kb], [&va, &vb], &hist);
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                sort.encode(&mut pass, &binding, passes, &control);
            }
            gpu.queue.submit([enc.finish()]);
            let (kout, vout) = if passes % 2 == 0 {
                (&ka, &va)
            } else {
                (&kb, &vb)
            };
            let got_k = read(&gpu, kout, n);
            let got_v = read(&gpu, vout, n);
            let mut expect: Vec<(u32, u32)> =
                keys.iter().copied().zip(vals.iter().copied()).collect();
            expect.sort_by_key(|&(k, _)| k);
            let ek: Vec<u32> = expect.iter().map(|e| e.0).collect();
            let ev: Vec<u32> = expect.iter().map(|e| e.1).collect();
            assert_eq!(got_k, ek, "keys, n = {n}");
            assert_eq!(got_v, ev, "values (stability), n = {n}");
        }
    }
}
