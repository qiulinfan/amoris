//! The compute tile rasterizer (splat_tile.wgsl, splat_composite.wgsl; docs/spec/splats.md 4.2):
//! the alternative to the quad draw, selected with [`super::SplatRaster::Tiles`].
//!
//! It reuses the preprocess and the depth sort (back to front) and adds, before the opaque pass:
//! binning (each visible splat, in front-to-back order, writes a (tile, splat) pair for every
//! 16x16-pixel tile its quad's bounding box overlaps, at offsets from a prefix sum), a stable radix
//! sort of the pairs by tile (the depth order survives inside each tile: the splats were emitted
//! front to back) and each tile's range. After the opaque pass one workgroup per tile blends its
//! splats front to back, stopping per pixel at the scene's depth or once the transmittance is below
//! 1/255, into an rgba16float storage image that a full-screen pass composites over the HDR image.
//!
//! The pairs live in buffers of a fixed capacity, grown from a readback of the wanted count a few
//! frames late (the CPU never waits) up to the device's storage binding size; pairs beyond the
//! capacity are dropped from the farthest splats and reported in [`super::SplatStats`].

use super::sort::{self, RadixSort, SortBinding};
use crate::gpu::Gpu;
use crate::post::{HDR, Targets};
use crate::profiler::GpuProfiler;
use crate::shaders;

/// Tile size in pixels (`TILE_PX` in splat_tile.wgsl).
pub(crate) const TILE_PX: u32 = 16;
const WG: u64 = 256;
/// Bytes of a rasterizer record (`TileSplat`).
const TILE_SPLAT_BYTES: u64 = 32;
/// The tile control: [0] pairs to sort, [1] pairs wanted, the sort's dispatch at 16, the dispatch
/// over the visible splats at 32.
const CONTROL_BYTES: u64 = 64;
const SPLAT_DISPATCH: u64 = 32;

/// The splat pipeline's buffers the binning reads.
pub(crate) struct Inputs<'a> {
    pub params: &'a wgpu::Buffer,
    pub control: &'a wgpu::Buffer,
    /// The depth-sorted keys and values (back to front).
    pub keys: &'a wgpu::Buffer,
    pub vals: &'a wgpu::Buffer,
    pub projected: &'a wgpu::Buffer,
}

struct Binds {
    setup: wgpu::BindGroup,
    count: wgpu::BindGroup,
    scan: wgpu::BindGroup,
    emit: wgpu::BindGroup,
    sort: SortBinding,
    /// Reading the sorted pairs from buffer A (even sort passes) or B.
    ranges: [wgpu::BindGroup; 2],
    /// The splat pipeline's buffers these were made for ([`super::Splats`]' generation).
    generation: u64,
}

struct Image {
    view: wgpu::TextureView,
    /// The multisampled depth it was made for.
    depth: wgpu::TextureView,
    size: (u32, u32),
    composite: wgpu::BindGroup,
    /// [even, odd sort passes]; rebuilt with the bindings.
    raster: Option<[wgpu::BindGroup; 2]>,
}

pub(crate) struct TileRaster {
    gpu: Gpu,
    setup: wgpu::ComputePipeline,
    count: wgpu::ComputePipeline,
    scan: wgpu::ComputePipeline,
    emit: wgpu::ComputePipeline,
    ranges: wgpu::ComputePipeline,
    raster: wgpu::ComputePipeline,
    composite: wgpu::RenderPipeline,
    pub(crate) control: wgpu::Buffer,
    rects: wgpu::Buffer,
    tsplats: wgpu::Buffer,
    blocks: wgpu::Buffer,
    pair_keys: [wgpu::Buffer; 2],
    pair_vals: [wgpu::Buffer; 2],
    hist: wgpu::Buffer,
    range_buf: wgpu::Buffer,
    /// Visible splats the per-splat buffers hold.
    capacity: u32,
    /// Pairs the pair buffers hold.
    pub(crate) pair_capacity: u32,
    /// The largest pair capacity: what the device binds, or less ([`TileRaster::limit_pairs`]).
    pub(crate) pair_limit: u32,
    device_pair_limit: u32,
    /// Tiles across and down.
    pub(crate) tiles: (u32, u32),
    tile_buf_tiles: u32,
    binds: Option<Binds>,
    image: Option<Image>,
}

fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    super::buffer(device, label, size, super::storage_usage())
}

fn entry(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buf.as_entire_binding(),
    }
}

impl TileRaster {
    pub(crate) fn new(gpu: &Gpu) -> TileRaster {
        let device = &gpu.device;
        let module = shaders::module(device, "splat_tile");
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        let cmod = shaders::module(device, "splat_composite");
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("splat composite"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &cmod,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &cmod,
                entry_point: Some("fs_composite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR,
                    // splats + transmittance x scene; the scene's alpha is kept.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::SrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let limits = device.limits();
        let max = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let su = super::storage_usage();
        let device_pair_limit = (max / 4).min(u64::from(u32::MAX - sort::TILE)) as u32;
        TileRaster {
            setup: compute("tile_setup"),
            count: compute("tile_count"),
            scan: compute("tile_scan"),
            emit: compute("tile_emit"),
            ranges: compute("tile_ranges"),
            raster: compute("tile_raster"),
            composite,
            control: super::buffer(
                device,
                "splat tile control",
                CONTROL_BYTES,
                su | wgpu::BufferUsages::INDIRECT,
            ),
            rects: storage(device, "splat tile rects", 64),
            tsplats: storage(device, "splat tile splats", 64),
            blocks: storage(device, "splat tile blocks", 64),
            pair_keys: [
                storage(device, "splat pair keys A", 64),
                storage(device, "splat pair keys B", 64),
            ],
            pair_vals: [
                storage(device, "splat pair values A", 64),
                storage(device, "splat pair values B", 64),
            ],
            hist: storage(device, "splat pair histogram", sort::hist_bytes(0)),
            range_buf: storage(device, "splat tile ranges", 64),
            capacity: 0,
            pair_capacity: 0,
            pair_limit: device_pair_limit,
            device_pair_limit,
            tiles: (0, 0),
            tile_buf_tiles: 0,
            binds: None,
            image: None,
            gpu: gpu.clone(),
        }
    }

    /// Caps the pair buffers below what the device binds (`None`: the device's limit), to test and
    /// show the overflow path.
    pub(crate) fn limit_pairs(&mut self, limit: Option<u32>) {
        let l = limit.map_or(self.device_pair_limit, |l| {
            l.clamp(sort::TILE, self.device_pair_limit)
        });
        if l != self.pair_limit {
            self.pair_limit = l;
            self.pair_capacity = 0;
            self.binds = None;
        }
    }

    /// Sort passes for the tile ids (8 bits each): enough that the sorted bits of `NO_TILE` (all
    /// ones) exceed every tile, so padding pairs sort last.
    pub(crate) fn sort_passes(&self) -> u32 {
        let tiles = (self.tiles.0 * self.tiles.1).max(1);
        let bits = 32 - tiles.leading_zeros();
        bits.div_ceil(8).max(1)
    }

    /// Sizes the buffers for `capacity` visible splats, at least `pairs` pairs and a target of
    /// `size` pixels; returns whether anything was reallocated.
    pub(crate) fn ensure(&mut self, capacity: u32, pairs: u64, size: (u32, u32)) -> bool {
        let device = self.gpu.device.clone();
        let mut changed = false;
        self.tiles = (size.0.div_ceil(TILE_PX), size.1.div_ceil(TILE_PX));
        let tiles = self.tiles.0 * self.tiles.1;
        if tiles > self.tile_buf_tiles {
            self.range_buf = storage(&device, "splat tile ranges", u64::from(tiles) * 8);
            self.tile_buf_tiles = tiles;
            changed = true;
        }
        if capacity > self.capacity {
            let c = u64::from(capacity);
            self.rects = storage(&device, "splat tile rects", c * 16);
            self.tsplats = storage(&device, "splat tile splats", c * TILE_SPLAT_BYTES);
            self.blocks = storage(&device, "splat tile blocks", c.div_ceil(WG) * 4);
            self.capacity = capacity;
            changed = true;
        }
        let want = pairs
            .max(u64::from(capacity) * 3)
            .max(1 << 20)
            .min(u64::from(self.pair_limit)) as u32;
        if want > self.pair_capacity {
            let cap = want
                .max(self.pair_capacity.saturating_mul(5) / 4)
                .min(self.pair_limit);
            let p = u64::from(cap);
            self.pair_keys = [
                storage(&device, "splat pair keys A", p * 4),
                storage(&device, "splat pair keys B", p * 4),
            ];
            self.pair_vals = [
                storage(&device, "splat pair values A", p * 4),
                storage(&device, "splat pair values B", p * 4),
            ];
            self.hist = storage(&device, "splat pair histogram", sort::hist_bytes(cap));
            self.pair_capacity = cap;
            changed = true;
        }
        if changed {
            self.binds = None;
        }
        changed
    }

    /// Bytes of GPU buffers the tile rasterizer holds.
    pub(crate) fn gpu_bytes(&self) -> u64 {
        [
            &self.rects,
            &self.tsplats,
            &self.blocks,
            &self.pair_keys[0],
            &self.pair_keys[1],
            &self.pair_vals[0],
            &self.pair_vals[1],
            &self.hist,
            &self.range_buf,
        ]
        .iter()
        .map(|b| b.size())
        .sum()
    }

    fn bind(&mut self, sorter: &RadixSort, inp: &Inputs<'_>, generation: u64) {
        let device = &self.gpu.device;
        let group =
            |p: &wgpu::ComputePipeline, label: &str, entries: &[wgpu::BindGroupEntry<'_>]| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: &p.get_bind_group_layout(0),
                    entries,
                })
            };
        let setup = group(
            &self.setup,
            "splat tile setup",
            &[entry(1, inp.control), entry(2, &self.control)],
        );
        let count = group(
            &self.count,
            "splat tile count",
            &[
                entry(0, inp.params),
                entry(1, inp.control),
                entry(3, inp.keys),
                entry(4, inp.vals),
                entry(5, inp.projected),
                entry(6, &self.rects),
                entry(7, &self.tsplats),
                entry(8, &self.blocks),
            ],
        );
        let scan = group(
            &self.scan,
            "splat tile scan",
            &[
                entry(0, inp.params),
                entry(1, inp.control),
                entry(2, &self.control),
                entry(8, &self.blocks),
            ],
        );
        let emit = group(
            &self.emit,
            "splat tile emit",
            &[
                entry(0, inp.params),
                entry(1, inp.control),
                entry(6, &self.rects),
                entry(7, &self.tsplats),
                entry(8, &self.blocks),
                entry(9, &self.pair_keys[0]),
                entry(10, &self.pair_vals[0]),
            ],
        );
        let ranges = |i: usize| {
            group(
                &self.ranges,
                "splat tile ranges",
                &[
                    entry(11, &self.control),
                    entry(12, &self.pair_keys[i]),
                    entry(14, &self.range_buf),
                ],
            )
        };
        let sort = sorter.bind(
            device,
            &self.control,
            [&self.pair_keys[0], &self.pair_keys[1]],
            [&self.pair_vals[0], &self.pair_vals[1]],
            &self.hist,
        );
        self.binds = Some(Binds {
            setup,
            count,
            scan,
            emit,
            sort,
            ranges: [ranges(0), ranges(1)],
            generation,
        });
        if let Some(img) = self.image.as_mut() {
            img.raster = None;
        }
    }

    /// Before the opaque pass, after the depth sort: bins the visible splats into tiles and sorts
    /// the pairs. `generation` changes whenever the splat pipeline's buffers do.
    pub(crate) fn prepare(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        sorter: &RadixSort,
        inp: &Inputs<'_>,
        generation: u64,
    ) {
        if self
            .binds
            .as_ref()
            .is_none_or(|b| b.generation != generation)
        {
            self.bind(sorter, inp, generation);
        }
        let Some(b) = self.binds.as_ref() else {
            return;
        };
        enc.clear_buffer(&self.range_buf, 0, None);
        {
            let ts = profiler.compute_scope("splat tile count");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat tile count"),
                timestamp_writes: ts,
            });
            pass.set_pipeline(&self.setup);
            pass.set_bind_group(0, &b.setup, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            pass.set_pipeline(&self.count);
            pass.set_bind_group(0, &b.count, &[]);
            pass.dispatch_workgroups_indirect(&self.control, SPLAT_DISPATCH);
        }
        {
            let ts = profiler.compute_scope("splat tile scan");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat tile scan"),
                timestamp_writes: ts,
            });
            pass.set_pipeline(&self.scan);
            pass.set_bind_group(0, &b.scan, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let ts = profiler.compute_scope("splat tile emit");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat tile emit"),
                timestamp_writes: ts,
            });
            pass.set_pipeline(&self.emit);
            pass.set_bind_group(0, &b.emit, &[]);
            pass.dispatch_workgroups_indirect(&self.control, SPLAT_DISPATCH);
        }
        let ts = profiler.compute_scope("splat tile sort");
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("splat tile sort"),
            timestamp_writes: ts,
        });
        let passes = self.sort_passes();
        sorter.encode(&mut pass, &b.sort, passes, &self.control);
        pass.set_pipeline(&self.ranges);
        pass.set_bind_group(0, &b.ranges[(passes % 2) as usize], &[]);
        pass.dispatch_workgroups_indirect(&self.control, sort::DISPATCH_OFFSET);
    }

    /// After the opaque pass: rasterizes the tiles against the scene's depth and composites the
    /// result over the HDR image.
    pub(crate) fn draw(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        targets: &Targets,
        params: &wgpu::Buffer,
    ) {
        let device = self.gpu.device.clone();
        let size = (targets.width.max(1), targets.height.max(1));
        let parity = (self.sort_passes() % 2) as usize;
        if self
            .image
            .as_ref()
            .is_none_or(|i| i.size != size || i.depth != targets.depth)
        {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("splat tile image"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("splat composite"),
                layout: &self.composite.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            self.image = Some(Image {
                view,
                depth: targets.depth.clone(),
                size,
                composite,
                raster: None,
            });
        }
        let Some(img) = self.image.as_mut() else {
            return;
        };
        if img.raster.is_none() {
            let raster = |i: usize| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("splat tile raster"),
                    layout: &self.raster.get_bind_group_layout(0),
                    entries: &[
                        entry(0, params),
                        entry(13, &self.pair_vals[i]),
                        entry(15, &self.range_buf),
                        entry(16, &self.tsplats),
                        wgpu::BindGroupEntry {
                            binding: 17,
                            resource: wgpu::BindingResource::TextureView(&img.depth),
                        },
                        wgpu::BindGroupEntry {
                            binding: 18,
                            resource: wgpu::BindingResource::TextureView(&img.view),
                        },
                    ],
                })
            };
            img.raster = Some([raster(0), raster(1)]);
        }
        let Some(raster) = img.raster.as_ref() else {
            return;
        };
        {
            let ts = profiler.compute_scope("splat raster");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat raster"),
                timestamp_writes: ts,
            });
            pass.set_pipeline(&self.raster);
            pass.set_bind_group(0, &raster[parity], &[]);
            pass.dispatch_workgroups(self.tiles.0, self.tiles.1, 1);
        }
        let ts = profiler.render_scope("splat composite");
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("splat composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.hdr_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: ts,
            ..Default::default()
        });
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &img.composite, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use glam::{Quat, Vec3};
    use pocket_assets::frame::{Pose, RenderFrame, SplatView};

    use crate::gpu::{BackendChoice, Gpu};
    use crate::splat::{RawSplat, SplatCloud, SplatRaster};
    use crate::{CameraState, Renderer};

    /// Random splats of every size and opacity in a box before the camera, some long and thin.
    fn cloud(n: usize) -> SplatCloud {
        let mut x = 0x9e37_79b9u32;
        let mut f = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as f32 / (1u32 << 24) as f32
        };
        let raw: Vec<RawSplat> = (0..n)
            .map(|i| {
                let s = 0.01 + 0.15 * f() * f();
                let thin = if i % 5 == 0 { 0.08 } else { 1.0 };
                let q = Quat::from_xyzw(f() - 0.5, f() - 0.5, f() - 0.5, f() - 0.5).normalize();
                RawSplat {
                    position: [f() * 4.0 - 2.0, f() * 2.4 - 1.2, -f() * 4.0],
                    scale: [s, s * thin, s * 0.5],
                    rotation: q.to_array(),
                    color: [f(), f(), f()],
                    opacity: 0.05 + 0.95 * f(),
                }
            })
            .collect();
        SplatCloud::from_raw(&raw, 0, &[])
    }

    fn renderer(gpu: &Gpu, n: usize) -> Renderer {
        let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 200, 120);
        r.splats.insert("c", &cloud(n));
        r.apply(
            RenderFrame {
                tick: 1,
                reset: true,
                splats: Some(vec![SplatView {
                    id: 7,
                    asset: "c".into(),
                    pose: Pose::default(),
                    visible: true,
                }]),
                ..RenderFrame::default()
            },
            0.0,
        );
        r.set_camera_override(Some(CameraState::look_at(
            Vec3::new(0.3, 0.4, 3.0),
            Vec3::new(0.0, 0.0, -2.0),
        )));
        r
    }

    fn shoot(r: &mut Renderer, raster: SplatRaster) -> Vec<u8> {
        r.splats.raster = raster;
        for i in 0..6 {
            let _ = r.capture_rgba(f64::from(i) / 60.0);
        }
        r.capture_rgba(0.1).2
    }

    fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
        let s: u64 = a
            .iter()
            .zip(b)
            .map(|(x, y)| u64::from(x.abs_diff(*y)))
            .sum();
        s as f64 / a.len() as f64
    }

    /// The tile rasterizer draws what the quads draw (they blend the same Gaussians; the quads round
    /// to f16 at every blend, the tiles once). Skipped without a GPU.
    #[test]
    fn tiles_match_quads() {
        let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
            eprintln!("no GPU: skipped");
            return;
        };
        let mut r = renderer(&gpu, 20_000);
        let quads = shoot(&mut r, SplatRaster::Quads);
        let tiles = shoot(&mut r, SplatRaster::Tiles);
        let st = r.splats.stats.clone();
        assert!(
            st.tile_pairs.unwrap_or(0) > 0 && st.tile_dropped == 0,
            "{st:?}"
        );
        r.splats.draw_enabled = false;
        let none = shoot(&mut r, SplatRaster::Tiles);
        let drawn = mean_abs_diff(&quads, &none);
        let diff = mean_abs_diff(&quads, &tiles);
        let max = quads.iter().zip(&tiles).map(|(a, b)| a.abs_diff(*b)).max();
        eprintln!(
            "splats change the image by {drawn:.2}; tiles differ from quads by {diff:.4}, at most {max:?}"
        );
        assert!(drawn > 3.0, "the splats are visible");
        assert!(diff < 0.5, "mean difference {diff}");
        assert!(max.unwrap_or(0) < 32, "largest difference {max:?}");
    }

    /// With fewer pair slots than the frame wants, the overflow is counted, not silent, and the
    /// nearest splats still draw. Skipped without a GPU.
    #[test]
    fn pair_overflow_is_reported() {
        let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
            eprintln!("no GPU: skipped");
            return;
        };
        let mut r = renderer(&gpu, 20_000);
        let full = shoot(&mut r, SplatRaster::Tiles);
        let wanted = r.splats.stats.tile_pairs.unwrap_or(0);
        r.splats.limit_tile_pairs(Some(4096));
        let cut = shoot(&mut r, SplatRaster::Tiles);
        let st = r.splats.stats.clone();
        assert_eq!(st.pair_capacity, 4096, "{st:?}");
        assert_eq!(st.tile_dropped, wanted - 4096, "{st:?}");
        let diff = mean_abs_diff(&full, &cut);
        assert!(diff > 0.5, "dropping pairs shows ({diff})");
        r.splats.limit_tile_pairs(None);
        let back = shoot(&mut r, SplatRaster::Tiles);
        assert_eq!(r.splats.stats.tile_dropped, 0);
        assert!(mean_abs_diff(&full, &back) < 0.01);
    }
}
