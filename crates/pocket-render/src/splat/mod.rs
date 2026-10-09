//! 3D Gaussian Splatting (charter 4.4, neural rendering; docs/spec/splats.md): splat clouds drawn
//! over the meshes' HDR image, depth-tested against the meshes' depth, so a scene can mix meshes
//! and captured or generated splats.
//!
//! Per frame, when any cloud is drawn:
//! 1. `splat preprocess` (compute): one thread per splat of every drawn cloud projects it (EWA),
//!    culls it, evaluates its spherical harmonics and appends it with a depth key; a one-thread
//!    dispatch then writes the sort's indirect dispatch and the draw's arguments;
//! 2. `splat sort` (compute): the portable radix sort (sort.rs) orders the visible splats back to
//!    front;
//! 3. after the opaque pass: `splat depth` copies the first sample of the multisampled depth into a
//!    single-sample depth target, and `splat draw` draws the quads (one indexed indirect draw)
//!    into the resolved HDR image, blended premultiplied, depth-tested and not written. Blending
//!    into one sample instead of four measured 2.2x cheaper (docs/bench/splats.md); the cost is a
//!    per-pixel, not per-sample, edge where a mesh hides splats.
//!
//! The CPU's per-frame work is per cloud, never per splat, and it never waits for the GPU.
//!
//! Clouds are assets by name: hosts insert them ([`Splats::insert`]; the browser after fetching,
//! examples after generating), or natively a worker thread loads `.ply` and `.splat` files under a
//! root ([`Splats::set_root`]) when the render feed names them.

pub mod cloud;
pub mod loader;
pub mod sort;
pub mod tile;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3, Vec4};

pub use cloud::{PackedSplat, RawSplat, SH_C0, SplatCloud};

use crate::camera::{CameraState, frustum_planes};
use crate::gpu::Gpu;
use crate::post::{DEPTH, HDR, Targets};
use crate::profiler::GpuProfiler;
use crate::scene::Scene;
use crate::shaders;
use sort::RadixSort;

/// Bytes of the control buffer: the visible count at 0, the sort's dispatch at 16, the draw at 32.
const CONTROL_BYTES: u64 = 64;
const DRAW_OFFSET: u64 = 32;
/// Quads per instance of the draw (`DRAW_BATCH` in splat_common.wgsl): an instance is a batch of
/// quads from a fixed 16-bit index buffer (on the M5 as fast as one 4-vertex instance per splat,
/// and the vertex cache shades each corner once).
pub(crate) const BATCH: u32 = 16383;
const PROJECTED_BYTES: u64 = 24;
/// `SPLAT_ANTIALIAS` in splat_common.wgsl: the params' flag for [`Splats::antialias`].
const ANTIALIAS: u32 = 1;

/// How the sorted splats are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplatRaster {
    /// One alpha-blended quad per splat, back to front, through the fixed-function blender.
    Quads,
    /// The compute tile rasterizer (tile.rs): per 16x16-pixel tile, front to back with early
    /// termination, then composited.
    Tiles,
}

impl SplatRaster {
    /// From `POCKET_SPLAT_RASTER` (`tile`, `tiles`, `quad`, `quads`); `default` otherwise.
    pub fn from_env(default: SplatRaster) -> SplatRaster {
        match std::env::var("POCKET_SPLAT_RASTER").as_deref() {
            Ok("tile") | Ok("tiles") => SplatRaster::Tiles,
            Ok("quad") | Ok("quads") => SplatRaster::Quads,
            _ => default,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct ParamsGpu {
    viewport: [f32; 4],
    proj: [f32; 4],
    depth: [f32; 4],
    counts: [u32; 4],
    color: [f32; 4],
    tiles: [u32; 4],
}

/// A drawn cloud (matches `Cloud` in splat_preprocess.wgsl, 112 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct CloudGpu {
    model_view: [[f32; 4]; 4],
    camera_local: [f32; 4],
    first: u32,
    offset: u32,
    count: u32,
    sh_degree: u32,
    sh_offset: u32,
    sh_words: u32,
    _p: [u32; 2],
}

/// A cloud in GPU memory.
#[derive(Clone, Copy, Debug)]
struct Asset {
    offset: u32,
    count: u32,
    sh_offset: u32,
    sh_degree: u32,
    bounds: (Vec3, Vec3),
}

/// What the last frame drew.
#[derive(Clone, Debug, Default)]
pub struct SplatStats {
    /// Clouds in GPU memory and their splats.
    pub assets: usize,
    pub stored: u64,
    /// Clouds drawn (after culling whole clouds) and their splats, which the preprocess visits.
    pub clouds: usize,
    pub submitted: u64,
    /// Splats that survived the preprocess, read back a few frames late.
    pub visible: Option<u32>,
    /// Their quads' pixels (each clipped to the screen's size): the fill the draw costs, read back
    /// with `visible`.
    pub quad_pixels: Option<u64>,
    /// Bytes of GPU buffers the splats hold.
    pub gpu_bytes: u64,
    /// The tile rasterizer's (tile, splat) pairs the last read frame wanted, read back with
    /// `visible`, and the capacity of its pair buffers.
    pub tile_pairs: Option<u64>,
    pub pair_capacity: u32,
    /// Pairs the tile rasterizer dropped (the farthest splats' tiles) because the wanted count
    /// exceeded what the device can bind; 0 when everything was drawn.
    pub tile_dropped: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Readback {
    Idle,
    Copied,
    Mapping,
    Ready,
}

struct DepthCopy {
    source: wgpu::TextureView,
    target: wgpu::TextureView,
    group: wgpu::BindGroup,
}

struct Binds {
    preprocess: wgpu::BindGroup,
    sort: sort::SortBinding,
    /// Reading the keys and values from buffer A (an even number of sort passes) or B.
    draw: [wgpu::BindGroup; 2],
}

pub struct Splats {
    gpu: Gpu,
    assets: HashMap<String, Asset>,
    #[cfg(not(target_arch = "wasm32"))]
    files: Option<loader::SplatFiles>,
    requested: HashSet<String>,
    /// Names no file root serves, for the page to fetch ([`Splats::take_requests`]).
    fetches: Vec<String>,
    /// The drawn clouds from the render feed: (asset, model matrix).
    views: Vec<(String, Mat4)>,

    splat_buf: wgpu::Buffer,
    splat_len: u64,
    sh_buf: wgpu::Buffer,
    sh_len: u64,
    clouds_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    projected: wgpu::Buffer,
    keys: [wgpu::Buffer; 2],
    vals: [wgpu::Buffer; 2],
    hist: wgpu::Buffer,
    control: wgpu::Buffer,
    capacity: u32,
    readback: wgpu::Buffer,
    readback_state: Arc<Mutex<Readback>>,
    /// The index buffer of one batch of quads.
    quads: wgpu::Buffer,

    pre_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    preprocess: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    draw_pipeline: wgpu::RenderPipeline,
    depth_pipeline: wgpu::RenderPipeline,
    /// The single-sample depth target and the bind group reading the multisampled depth it copies.
    depth: Option<DepthCopy>,
    sort: RadixSort,
    binds: Option<Binds>,
    /// Counts the rebinding of the buffers above (the tile rasterizer's bindings follow it).
    generation: u64,
    tiles: tile::TileRaster,
    /// The rasterizer of the frame being prepared.
    drawn_with: SplatRaster,
    /// Whether the current overflow of the tile pairs was logged.
    overflow_logged: bool,
    active: bool,

    /// Sort key width: 16 or 24 (log depth over the drawn clouds' range, 2 or 3 sort passes) or 32
    /// (the depth's float bits, 4 passes).
    pub key_bits: u32,
    /// Multiplies the clouds' linear colors into the HDR target (1: as an unlit material).
    pub radiance: f32,
    /// Whether the draw runs (benchmarks turn it off to time the rest).
    pub draw_enabled: bool,
    /// Quads or tiles (default: quads, or `POCKET_SPLAT_RASTER`).
    pub raster: SplatRaster,
    /// Opacity compensation for the 0.3 px^2 low-pass (gsplat's `antialiased` mode, the 2D filter
    /// of Mip-Splatting): a splat smaller than a pixel keeps its integral and fades instead of
    /// growing to a pixel at full opacity. Right for clouds trained that way and for generated
    /// ones; it thins clouds trained with the original 3DGS, which learned the plain low-pass.
    /// Default off, or `POCKET_SPLAT_AA=1`.
    pub antialias: bool,
    pub stats: SplatStats,
}

pub(crate) fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16).next_multiple_of(4),
        usage,
        mapped_at_creation: false,
    })
}

pub(crate) fn storage_usage() -> wgpu::BufferUsages {
    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC
}

fn layout_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Splats {
    pub fn new(gpu: &Gpu) -> Splats {
        let device = &gpu.device;
        let cs = wgpu::ShaderStages::COMPUTE;
        let uniform = wgpu::BufferBindingType::Uniform;
        let ro = wgpu::BufferBindingType::Storage { read_only: true };
        let rw = wgpu::BufferBindingType::Storage { read_only: false };
        let pre_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat preprocess"),
            entries: &[
                layout_entry(0, cs, uniform),
                layout_entry(1, cs, ro),
                layout_entry(2, cs, ro),
                layout_entry(3, cs, ro),
                layout_entry(4, cs, rw),
                layout_entry(5, cs, rw),
                layout_entry(6, cs, rw),
                layout_entry(7, cs, rw),
            ],
        });
        let vs = wgpu::ShaderStages::VERTEX;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat draw"),
            entries: &[
                layout_entry(0, vs, uniform),
                layout_entry(1, vs, ro),
                layout_entry(2, vs, ro),
                layout_entry(3, vs, ro),
                layout_entry(4, vs, ro),
            ],
        });
        let pre_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splat preprocess"),
            bind_group_layouts: &[Some(&pre_layout)],
            immediate_size: 0,
        });
        let pre_module = shaders::module(device, "splat_preprocess");
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pre_pl),
                module: &pre_module,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        let draw_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("splat draw"),
            bind_group_layouts: &[Some(&draw_layout)],
            immediate_size: 0,
        });
        let draw_module = shaders::module(device, "splat_draw");
        let draw_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("splats"),
            layout: Some(&draw_pl),
            vertex: wgpu::VertexState {
                module: &draw_module,
                entry_point: Some("vs_splat"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &draw_module,
                entry_point: Some("fs_splat"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let depth_module = shaders::module(device, "splat_depth");
        let depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("splat depth"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &depth_module,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &depth_module,
                entry_point: Some("fs_depth"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let su = storage_usage();
        let indices: Vec<u16> = (0..BATCH as u16)
            .flat_map(|q| {
                let v = q * 4;
                [v, v + 1, v + 2, v + 1, v + 3, v + 2]
            })
            .collect();
        let quads = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("splat quads"),
            size: (indices.len() * 2) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue
            .write_buffer(&quads, 0, bytemuck::cast_slice(&indices));
        Splats {
            quads,
            assets: HashMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            files: None,
            requested: HashSet::new(),
            fetches: Vec::new(),
            views: Vec::new(),
            splat_buf: buffer(device, "splats", 64, su),
            splat_len: 0,
            sh_buf: buffer(device, "splat sh", 64, su),
            sh_len: 0,
            clouds_buf: buffer(device, "splat clouds", 112 * 4, su),
            params_buf: buffer(
                device,
                "splat params",
                std::mem::size_of::<ParamsGpu>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            projected: buffer(device, "splat projected", 64, su),
            keys: [
                buffer(device, "splat keys A", 64, su),
                buffer(device, "splat keys B", 64, su),
            ],
            vals: [
                buffer(device, "splat values A", 64, su),
                buffer(device, "splat values B", 64, su),
            ],
            hist: buffer(device, "splat sort histogram", sort::hist_bytes(0), su),
            control: buffer(
                device,
                "splat control",
                CONTROL_BYTES,
                su | wgpu::BufferUsages::INDIRECT,
            ),
            capacity: 0,
            readback: buffer(
                device,
                "splat count readback",
                32,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            readback_state: Arc::new(Mutex::new(Readback::Idle)),
            preprocess: compute("preprocess"),
            finish: compute("finish"),
            draw_pipeline,
            depth_pipeline,
            depth: None,
            sort: RadixSort::new(device),
            pre_layout,
            draw_layout,
            binds: None,
            generation: 0,
            tiles: tile::TileRaster::new(gpu),
            drawn_with: SplatRaster::Quads,
            overflow_logged: false,
            active: false,
            key_bits: 24,
            radiance: 1.0,
            draw_enabled: true,
            raster: SplatRaster::from_env(SplatRaster::Quads),
            antialias: std::env::var("POCKET_SPLAT_AA").is_ok_and(|v| v == "1"),
            stats: SplatStats::default(),
            gpu: gpu.clone(),
        }
    }

    /// Loads clouds the render feed names from files under `root`, on a worker thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_root(&mut self, root: std::path::PathBuf) {
        self.files = Some(loader::SplatFiles::new(root));
        self.requested.retain(|n| self.assets.contains_key(n));
    }

    /// Caps the tile rasterizer's (tile, splat) pairs below what the device binds (`None`: the
    /// device's limit); beyond it the farthest splats' pairs are dropped and counted in
    /// [`SplatStats::tile_dropped`].
    pub fn limit_tile_pairs(&mut self, limit: Option<u32>) {
        self.tiles.limit_pairs(limit);
    }

    /// Whether a cloud named `name` is in GPU memory.
    pub fn contains(&self, name: &str) -> bool {
        self.assets.contains_key(name)
    }

    /// Uploads `cloud` under `name` (the name a `Splat` component's `asset` gives). Clouds are kept
    /// for the renderer's lifetime; inserting a name again draws the new cloud (the old one's
    /// memory is not reused).
    pub fn insert(&mut self, name: &str, cloud: &SplatCloud) {
        let device = &self.gpu.device;
        let queue = &self.gpu.queue;
        let limits = device.limits();
        let max = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let n = cloud.len() as u64;
        let need = (self.splat_len + n) * 32;
        let sh_need = (self.sh_len + cloud.sh.len() as u64) * 4;
        if need > max || sh_need > max {
            log::error!(
                "splats: {name} ({n} splats) does not fit: {need} bytes of splats and {sh_need} of SH, the device \
                 binds at most {max}"
            );
            return;
        }
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("splat upload"),
        });
        let mut grow = |buf: &mut wgpu::Buffer, used: u64, need: u64, label: &str| {
            if need > buf.size() {
                let size = need.max(buf.size() * 3 / 2).min(max);
                let nb = buffer(device, label, size, storage_usage());
                if used > 0 {
                    enc.copy_buffer_to_buffer(buf, 0, &nb, 0, used);
                }
                *buf = nb;
                true
            } else {
                false
            }
        };
        let mut grown = grow(&mut self.splat_buf, self.splat_len * 32, need, "splats");
        grown |= grow(&mut self.sh_buf, self.sh_len * 4, sh_need, "splat sh");
        if !cloud.is_empty() {
            queue.write_buffer(
                &self.splat_buf,
                self.splat_len * 32,
                bytemuck::cast_slice(&cloud.splats),
            );
        }
        if !cloud.sh.is_empty() {
            queue.write_buffer(
                &self.sh_buf,
                self.sh_len * 4,
                bytemuck::cast_slice(&cloud.sh),
            );
        }
        queue.submit([enc.finish()]);
        if grown {
            self.binds = None;
        }
        self.assets.insert(
            name.to_owned(),
            Asset {
                offset: self.splat_len as u32,
                count: n as u32,
                sh_offset: self.sh_len as u32,
                sh_degree: cloud.sh_degree,
                bounds: cloud.bounds,
            },
        );
        self.splat_len += n;
        self.sh_len += cloud.sh.len() as u64;
        self.stats.assets = self.assets.len();
        self.stats.stored = self.splat_len;
    }

    fn poll_files(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let done = match self.files.as_mut() {
                Some(f) => f.poll(),
                None => return,
            };
            for (path, r) in done {
                match r {
                    Ok(cloud) => self.insert(&path, &cloud),
                    Err(e) => log::warn!("splats: {e}"),
                }
            }
        }
    }

    fn request(&mut self, name: &str) {
        if !self.requested.insert(name.to_owned()) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(f) = self.files.as_mut() {
            f.request(name);
            return;
        }
        self.fetches.push(name.to_owned());
    }

    /// The clouds the feed named that no file root serves: in the browser the page fetches each and
    /// hands its bytes to [`Splats::deliver`].
    pub fn take_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.fetches)
    }

    /// Decodes a fetched `.ply` or `.splat` and uploads it under `name`.
    pub fn deliver(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let cloud = loader::parse(name, bytes)?;
        self.insert(name, &cloud);
        Ok(())
    }

    fn poll_visible(&mut self) {
        let state = *self
            .readback_state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match state {
            Readback::Copied => {
                *self
                    .readback_state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Readback::Mapping;
                let st = self.readback_state.clone();
                self.readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| {
                        *st.lock().unwrap_or_else(|p| p.into_inner()) = if r.is_ok() {
                            Readback::Ready
                        } else {
                            Readback::Idle
                        };
                    });
            }
            Readback::Ready => {
                if let Ok(data) = self.readback.slice(..).get_mapped_range() {
                    // Decoded, not cast: mapped memory in the browser has no alignment guarantee.
                    let v: Vec<u32> = data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| u32::from_le_bytes(*b))
                        .collect();
                    self.stats.visible = Some(v[0]);
                    self.stats.quad_pixels = Some(u64::from(v[1]) * 16);
                    // The tile control's [1], when the frame drew tiles (0 otherwise).
                    self.stats.tile_pairs = (v[5] > 0).then_some(u64::from(v[5]));
                }
                self.readback.unmap();
                *self
                    .readback_state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Readback::Idle;
            }
            Readback::Idle | Readback::Mapping => {}
        }
    }

    fn ensure_capacity(&mut self, total: u32) {
        if total <= self.capacity {
            return;
        }
        let device = &self.gpu.device;
        let cap = total
            .max(self.capacity.saturating_mul(3) / 2)
            .next_multiple_of(sort::TILE);
        let su = storage_usage();
        let c = u64::from(cap);
        self.projected = buffer(device, "splat projected", c * PROJECTED_BYTES, su);
        self.keys = [
            buffer(device, "splat keys A", c * 4, su),
            buffer(device, "splat keys B", c * 4, su),
        ];
        self.vals = [
            buffer(device, "splat values A", c * 4, su),
            buffer(device, "splat values B", c * 4, su),
        ];
        self.hist = buffer(device, "splat sort histogram", sort::hist_bytes(cap), su);
        self.capacity = cap;
        self.binds = None;
    }

    fn bind(&mut self) {
        let device = &self.gpu.device;
        fn e(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buf.as_entire_binding(),
            }
        }
        let preprocess = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("splat preprocess"),
            layout: &self.pre_layout,
            entries: &[
                e(0, &self.params_buf),
                e(1, &self.splat_buf),
                e(2, &self.sh_buf),
                e(3, &self.clouds_buf),
                e(4, &self.projected),
                e(5, &self.keys[0]),
                e(6, &self.vals[0]),
                e(7, &self.control),
            ],
        });
        let draw = |i: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("splat draw"),
                layout: &self.draw_layout,
                entries: &[
                    e(0, &self.params_buf),
                    e(1, &self.projected),
                    e(2, &self.keys[i]),
                    e(3, &self.vals[i]),
                    e(4, &self.control),
                ],
            })
        };
        let sort = self.sort.bind(
            device,
            &self.control,
            [&self.keys[0], &self.keys[1]],
            [&self.vals[0], &self.vals[1]],
            &self.hist,
        );
        self.binds = Some(Binds {
            preprocess,
            sort,
            draw: [draw(0), draw(1)],
        });
        self.generation += 1;
        let bytes: u64 = [
            &self.splat_buf,
            &self.sh_buf,
            &self.projected,
            &self.keys[0],
            &self.keys[1],
            &self.vals[0],
            &self.vals[1],
            &self.hist,
        ]
        .iter()
        .map(|b| b.size())
        .sum();
        self.stats.gpu_bytes = bytes + self.tiles.gpu_bytes();
    }

    fn sort_passes(&self) -> u32 {
        match self.key_bits {
            0..=16 => 2,
            17..=24 => 3,
            _ => 4,
        }
    }

    /// Before the opaque pass: takes the feed's splat list, then records the preprocess and the
    /// sort for the camera `cam` and a target of `size` pixels.
    pub fn prepare(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        scene: &mut Scene,
        cam: &CameraState,
        size: (u32, u32),
    ) {
        self.active = false;
        self.poll_files();
        self.poll_visible();
        if scene.splats_changed {
            scene.splats_changed = false;
            self.views = scene
                .splats
                .iter()
                .filter(|s| s.visible && !s.asset.is_empty())
                .map(|s| {
                    let p = &s.pose;
                    let model = Mat4::from_scale_rotation_translation(
                        Vec3::from(p.scale),
                        Quat::from_array(p.rotation).normalize(),
                        Vec3::from(p.position),
                    );
                    (s.asset.clone(), model)
                })
                .collect();
        }
        let missing: Vec<String> = self
            .views
            .iter()
            .filter(|(n, _)| !self.assets.contains_key(n) && !self.requested.contains(n))
            .map(|(n, _)| n.clone())
            .collect();
        for n in missing {
            self.request(&n);
        }

        let (w, h) = (size.0.max(1), size.1.max(1));
        let view = cam.view();
        let proj = cam.proj(w as f32 / h as f32);
        let planes = frustum_planes(proj * view, true);
        let near = cam.near.max(1e-4);
        let mut clouds = Vec::with_capacity(self.views.len());
        let mut total = 0u64;
        let (mut dmin, mut dmax) = (f32::INFINITY, 0.0f32);
        for (name, model) in &self.views {
            let Some(a) = self.assets.get(name) else {
                continue;
            };
            if a.count == 0 {
                continue;
            }
            let (lo, hi) = a.bounds;
            let corners: Vec<Vec4> = (0..8)
                .map(|i| {
                    let c = Vec3::new(
                        if i & 1 == 0 { lo.x } else { hi.x },
                        if i & 2 == 0 { lo.y } else { hi.y },
                        if i & 4 == 0 { lo.z } else { hi.z },
                    );
                    *model * c.extend(1.0)
                })
                .collect();
            if planes
                .iter()
                .any(|p| corners.iter().all(|c| p.dot(*c) < 0.0))
            {
                continue;
            }
            for c in &corners {
                let tz = -(view * *c).z;
                dmin = dmin.min(tz.max(near));
                dmax = dmax.max(tz);
            }
            let model_view = view * *model;
            let camera_local = model.inverse() * cam.position.extend(1.0);
            clouds.push(CloudGpu {
                model_view: model_view.to_cols_array_2d(),
                camera_local: camera_local.to_array(),
                first: total as u32,
                offset: a.offset,
                count: a.count,
                sh_degree: a.sh_degree,
                sh_offset: a.sh_offset,
                sh_words: cloud::sh_words(a.sh_degree) as u32,
                _p: [0; 2],
            });
            total += u64::from(a.count);
        }
        self.stats.clouds = clouds.len();
        self.stats.submitted = total;
        if total == 0 || total > u64::from(u32::MAX / 2) {
            return;
        }
        let total = total as u32;
        self.ensure_capacity(total);
        self.drawn_with = self.raster;
        let tiles = self.drawn_with == SplatRaster::Tiles;
        if tiles {
            let wanted = self.stats.tile_pairs.unwrap_or(0);
            if self
                .tiles
                .ensure(self.capacity, wanted + wanted / 4, (w, h))
            {
                // Rebinding recounts the GPU bytes; the tile bindings follow the generation.
                self.binds = None;
            }
            self.stats.pair_capacity = self.tiles.pair_capacity;
            self.stats.tile_dropped = wanted.saturating_sub(u64::from(self.tiles.pair_capacity));
            let at_limit = self.tiles.pair_capacity >= self.tiles.pair_limit;
            if self.stats.tile_dropped == 0 {
                self.overflow_logged = false;
            } else if at_limit && !self.overflow_logged {
                self.overflow_logged = true;
                log::warn!(
                    "splats: the tile rasterizer wants {wanted} pairs, the device binds {}: the \
                     farthest splats' {} are dropped",
                    self.tiles.pair_limit,
                    self.stats.tile_dropped
                );
            }
        }
        let device = self.gpu.device.clone();
        let queue = self.gpu.queue.clone();
        let cb = (clouds.len() * std::mem::size_of::<CloudGpu>()) as u64;
        if cb > self.clouds_buf.size() {
            self.clouds_buf = buffer(
                &device,
                "splat clouds",
                cb.next_power_of_two(),
                storage_usage(),
            );
            self.binds = None;
        }
        queue.write_buffer(&self.clouds_buf, 0, bytemuck::cast_slice(&clouds));
        if dmax.is_nan() || dmax <= dmin * 1.001 {
            dmax = dmin * 2.0;
        }
        let (l0, l1) = (dmin.log2(), dmax.log2());
        let bits = self.key_bits.clamp(16, 32);
        let largest = if bits >= 32 {
            u32::MAX
        } else {
            (1u32 << bits) - 1
        };
        let params = ParamsGpu {
            viewport: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32],
            proj: [proj.x_axis.x, proj.y_axis.y, proj.z_axis.z, proj.w_axis.z],
            depth: [near, l0, 1.0 / (l1 - l0), largest as f32],
            counts: [
                total,
                clouds.len() as u32,
                if bits >= 32 { 32 } else { bits },
                if self.antialias { ANTIALIAS } else { 0 },
            ],
            color: [self.radiance, 0.0, 0.0, 0.0],
            tiles: [
                self.tiles.tiles.0,
                self.tiles.tiles.1,
                self.tiles.pair_capacity,
                0,
            ],
        };
        queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&params));
        if self.binds.is_none() {
            self.bind();
        }
        let Some(binds) = self.binds.as_ref() else {
            return;
        };
        enc.clear_buffer(&self.control, 0, None);
        {
            let ts = profiler.compute_scope("splat preprocess");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat preprocess"),
                timestamp_writes: ts,
            });
            let groups = total.div_ceil(256);
            let (gx, gy) = if groups > 65535 {
                (65535, groups.div_ceil(65535))
            } else {
                (groups, 1)
            };
            pass.set_bind_group(0, &binds.preprocess, &[]);
            pass.set_pipeline(&self.preprocess);
            pass.dispatch_workgroups(gx, gy, 1);
            pass.set_pipeline(&self.finish);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let ts = profiler.compute_scope("splat sort");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("splat sort"),
                timestamp_writes: ts,
            });
            self.sort
                .encode(&mut pass, &binds.sort, self.sort_passes(), &self.control);
        }
        if tiles && self.draw_enabled {
            let s = (self.sort_passes() % 2) as usize;
            let inputs = tile::Inputs {
                params: &self.params_buf,
                control: &self.control,
                keys: &self.keys[s],
                vals: &self.vals[s],
                projected: &self.projected,
            };
            self.tiles
                .prepare(enc, profiler, &self.sort, &inputs, self.generation);
        }
        let mut st = self
            .readback_state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if *st == Readback::Idle {
            enc.copy_buffer_to_buffer(&self.control, 0, &self.readback, 0, 16);
            if tiles && self.draw_enabled {
                enc.copy_buffer_to_buffer(&self.tiles.control, 0, &self.readback, 16, 16);
            } else {
                enc.clear_buffer(&self.readback, 16, Some(16));
            }
            *st = Readback::Copied;
        }
        drop(st);
        self.active = true;
    }

    /// Whether splats draw this frame: the opaque pass must then keep its depth.
    pub fn active(&self) -> bool {
        self.active && self.draw_enabled
    }

    /// After the opaque pass (which kept its depth when [`Splats::active`]): copies the depth to a
    /// single sample and draws the sorted splats into the resolved HDR image.
    pub fn draw(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        targets: &Targets,
    ) {
        if !self.active() || self.binds.is_none() {
            return;
        }
        if self.drawn_with == SplatRaster::Tiles {
            self.tiles.draw(enc, profiler, targets, &self.params_buf);
            return;
        }
        if self
            .depth
            .as_ref()
            .is_none_or(|d| d.source != targets.depth)
        {
            let device = &self.gpu.device;
            let target = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("splat depth"),
                    size: wgpu::Extent3d {
                        width: targets.width.max(1),
                        height: targets.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("splat depth"),
                layout: &self.depth_pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&targets.depth),
                }],
            });
            self.depth = Some(DepthCopy {
                source: targets.depth.clone(),
                target,
                group,
            });
        }
        let (Some(depth), Some(binds)) = (self.depth.as_ref(), self.binds.as_ref()) else {
            return;
        };
        {
            let ts = profiler.render_scope("splat depth");
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("splat depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth.target,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: ts,
                ..Default::default()
            });
            pass.set_pipeline(&self.depth_pipeline);
            pass.set_bind_group(0, &depth.group, &[]);
            pass.draw(0..3, 0..1);
        }
        let ts = profiler.render_scope("splat draw");
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("splat draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.hdr_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth.target,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: ts,
            ..Default::default()
        });
        pass.set_pipeline(&self.draw_pipeline);
        pass.set_bind_group(0, &binds.draw[(self.sort_passes() % 2) as usize], &[]);
        pass.set_index_buffer(self.quads.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed_indirect(&self.control, DRAW_OFFSET);
    }
}
