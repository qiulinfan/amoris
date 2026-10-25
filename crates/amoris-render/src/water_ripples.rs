//! Sub-grid interactive ripples (water.md 57): capillary-gravity waves finer than a water region's
//! columns, made by the bodies that stir it, on a window of 512 x 512 cells over the region nearest
//! the camera. A region up to 16.5 m across lies wholly in a window that stands still over it, its
//! walls inside, at cells of 1/32 to 1/26 m; a larger one takes a 16 m window of 1/32 m cells that
//! follows the camera. Each mode of the window's Fourier transform advances exactly by its own
//! dispersion relation at four depth levels (J. Tessendorf's eWave, 2014), the levels blended per
//! cell by the depth of the field under it; the field's current carries the result, its dry ground
//! and walls reflect it and its shores absorb it. Waves as long as the field carries itself are
//! left to it. Shader: `water_ripples.wgsl`.
//!
//! The sources are the feed's stirring points (amoris-assets `StirView`): a Floater's buoyancy
//! points, a swimmer's body and the hands of a stroke. The renderer measures each one's immersion
//! against the surface it draws, and the change of the volume it holds under that surface, from
//! its place a step ago carried on by the water to its place now, rises in the window.
//!
//! Presentation only, like the foam (water_foam.rs): it advances with drawn simulated time, holds
//! still while the game is paused, and starts over on a reset, a rewind or another region. Nothing
//! it computes reaches the game. Until a point has stirred the window, the shading reads none of
//! it, so a scene with no stirring points draws exactly what it drew without the layer.
use std::collections::HashMap;

use amoris_assets::frame::{MAX_STIRS, StirView};
use bytemuck::{Pod, Zeroable};

/// Cells along the window's side, and the finest cell's edge, metres.
pub const SIDE: u32 = 512;
pub const CELL_M: f32 = 1.0 / 32.0;
/// The sponge along the window's edge, cells (water_ripples.wgsl `SPONGE`), and the water a
/// standing window keeps between its region's edges and the sponge, metres.
const SPONGE: u32 = 32;
const CLEARANCE_M: f32 = 0.25;
/// The widest region a standing window covers, metres; its cells are then 1/26 m.
const STANDING_M: f32 = 16.5;
/// The window's texture levels: 512 down to 8 texels a side (the shading's `RIPPLE_LODS`).
pub const MIPS: u32 = 7;
/// Depth levels, 0.064 m and every factor 2.5 above (water_ripples.wgsl `LEVELS`, `LEVEL_0`).
const LEVELS: u32 = 4;
/// A field carries its own waves from four columns long (its numerics blur shorter ones); the
/// window holds none longer than a quarter of its side either way.
const CARRIED_COLUMNS: f32 = 4.0;
const LONGEST_M: f32 = 4.0;
/// One step covers at most this much drawn time, s; a longer gap between frames is cut to it.
const MAX_STEP_S: f64 = 1.0 / 15.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [i32; 2],
    previous: [i32; 2],
    region_origin: [f32; 2],
    region_cell: f32,
    dt: f32,
    count: [u32; 4],
    cutoff: f32,
    fresh: u32,
    cell: f32,
    unused: f32,
}

/// The water the ripples ride on: a region's displayed columns (surface, depth, velocity).
pub struct Field<'a> {
    pub origin: [f32; 2],
    pub cell_m: f32,
    pub size: [u32; 2],
    pub tiled: bool,
    pub columns: &'a wgpu::Buffer,
}

/// The window as the shading places it: its corner (world x, z), its side (metres), and 1 while
/// it holds ripples (else all zero).
pub type Window = [f32; 4];

/// The window's rise as read back: its first cell (world cells along x and z), its cells' edge
/// (metres) and each cell's rise (metres), row by row from the first cell, x fastest.
pub struct Heights {
    pub origin: [i32; 2],
    pub cell_m: f32,
    pub rise: Vec<f32>,
}

/// Where a frame's window goes: its centre (world x, z) and its cells' edge, metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub centre: [f32; 2],
    pub cell_m: f32,
}

impl Placement {
    /// Over the region of corner `origin` and `extent` (metres), seen from `camera` (world x, z):
    /// a region that fits stands in a still window with its edges clear of the sponge, so its
    /// walls reflect ripples; a larger one is followed by a window centred on the camera, kept
    /// over the region.
    pub fn over(origin: [f32; 2], extent: [f32; 2], camera: [f32; 2]) -> Placement {
        let sponge = SPONGE as f32;
        let widest = extent[0].max(extent[1]);
        if widest <= STANDING_M {
            let cell = ((widest + 2.0 * CLEARANCE_M) / (SIDE as f32 - 2.0 * sponge)).max(CELL_M);
            return Placement {
                centre: [0, 1].map(|a| origin[a] + 0.5 * extent[a]),
                cell_m: cell,
            };
        }
        let half = 0.5 * SIDE as f32 * CELL_M;
        let inset = half - sponge * CELL_M;
        Placement {
            centre: [0, 1].map(|a| {
                let (lo, hi) = (origin[a] + inset, origin[a] + extent[a] - inset);
                if lo < hi {
                    camera[a].clamp(lo, hi)
                } else {
                    origin[a] + 0.5 * extent[a]
                }
            }),
            cell_m: CELL_M,
        }
    }
}

struct Pipelines {
    stir: wgpu::ComputePipeline,
    forward_rows: wgpu::ComputePipeline,
    columns: wgpu::ComputePipeline,
    rows: wgpu::ComputePipeline,
    finish: wgpu::ComputePipeline,
    surface: wgpu::ComputePipeline,
    mip: wgpu::ComputePipeline,
}

/// The bind groups that do not read a field's columns.
struct Groups {
    columns: wgpu::BindGroup,
    rows: wgpu::BindGroup,
    finish: wgpu::BindGroup,
    surface: wgpu::BindGroup,
    mips: Vec<wgpu::BindGroup>,
}

/// The step `prepare` decided for this frame, which `encode` records.
struct Step {
    dt: f32,
    previous: [i32; 2],
    points: Vec<[f32; 4]>,
}

/// The feed's stirring points: its two latest lists with their simulated times (drawn time falls
/// between them), and each point's slot in the GPU's record of where it was.
#[derive(Default)]
struct Stirs {
    before: Option<(f64, Vec<StirView>)>,
    latest: Option<(f64, Vec<StirView>)>,
    slots: HashMap<(u64, u32), u32>,
    free: Vec<u32>,
    issued: u32,
}

impl Stirs {
    fn set(&mut self, t: f64, stirs: &[StirView]) {
        let list = stirs[..stirs.len().min(MAX_STIRS)].to_vec();
        match &self.latest {
            Some((at, _)) if *at < t => self.before = self.latest.take(),
            Some((at, _)) if *at > t => self.before = None,
            _ => {}
        }
        self.latest = Some((t, list));
    }

    /// The points at drawn time `t`, two words each (centre and height; volume, slot, fresh), and
    /// whether any of them has a record to stir from.
    fn at(&mut self, t: f64) -> (Vec<[f32; 4]>, bool) {
        let Some((t1, list)) = &self.latest else {
            return (Vec::new(), false);
        };
        let earlier: HashMap<(u64, u32), [f32; 3]> = self
            .before
            .as_ref()
            .map(|(_, l)| l.iter().map(|s| ((s.id, s.point), s.at)).collect())
            .unwrap_or_default();
        let blend = self.before.as_ref().map_or(1.0, |(t0, _)| {
            if t1 > t0 {
                ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) as f32
            } else {
                1.0
            }
        });
        let mut kept = HashMap::with_capacity(list.len());
        let mut out = Vec::with_capacity(list.len() * 2);
        let mut stirring = false;
        for s in list {
            let key = (s.id, s.point);
            let (slot, fresh) = match self.slots.remove(&key) {
                Some(slot) => (slot, false),
                None => (
                    self.free.pop().unwrap_or_else(|| {
                        self.issued += 1;
                        self.issued - 1
                    }),
                    true,
                ),
            };
            kept.insert(key, slot);
            stirring |= !fresh;
            let at = earlier.get(&key).map_or(s.at, |a| {
                std::array::from_fn(|i| a[i] + (s.at[i] - a[i]) * blend)
            });
            out.push([at[0], at[1], at[2], s.size]);
            out.push([
                s.volume,
                f32::from_bits(slot),
                f32::from_bits(u32::from(fresh)),
                0.0,
            ]);
        }
        // Points that left: their slots wait for new points, which start fresh.
        self.free.extend(self.slots.drain().map(|(_, slot)| slot));
        self.slots = kept;
        (out, stirring)
    }

    fn forget(&mut self) {
        self.slots.clear();
        self.free.clear();
        self.issued = 0;
    }
}

pub struct Ripples {
    pipelines: Pipelines,
    groups: Groups,
    params: wgpu::Buffer,
    source: wgpu::Buffer,
    state: wgpu::Buffer,
    spectrum: wgpu::Buffer,
    /// Kept with the bind groups and views that use them.
    _scratch: wgpu::Buffer,
    _next: wgpu::Buffer,
    cells: wgpu::Buffer,
    blend: wgpu::Buffer,
    points: wgpu::Buffer,
    memory: wgpu::Buffer,
    depth_min: wgpu::Buffer,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    stirs: Stirs,
    origin: Option<[i32; 2]>,
    cell: f32,
    cutoff: f32,
    last_t: Option<f64>,
    step: Option<Step>,
    /// No step has run since the last clear: every cell and record starts at rest.
    fresh: bool,
    /// A step has run with a point able to stir: the window may hold ripples.
    active: bool,
}

fn group(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    entries: &[(u32, wgpu::BindingResource<'_>)],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("water ripples"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries
            .iter()
            .map(|(binding, resource)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: resource.clone(),
            })
            .collect::<Vec<_>>(),
    })
}

impl Ripples {
    pub fn new(device: &wgpu::Device) -> Ripples {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water ripples"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../shaders/common.wgsl"),
                    include_str!("../shaders/water_ripples.wgsl")
                )
                .into(),
            ),
        });
        let make = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        };
        let pipelines = Pipelines {
            stir: make("stir"),
            forward_rows: make("forward_rows"),
            columns: make("turn_columns"),
            rows: make("turn_rows"),
            finish: make("finish"),
            surface: make("surface"),
            mip: make("mip"),
        };
        let cells = u64::from(SIDE * SIDE);
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let uniform = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let params = buffer(
            "water ripples params",
            std::mem::size_of::<Params>() as u64,
            uniform,
        );
        let state = buffer(
            "water ripples state",
            cells * 8,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let spectrum = buffer("water ripples spectrum", cells * 8, storage);
        let scratch = buffer(
            "water ripples levels",
            cells * 8 * u64::from(LEVELS),
            storage,
        );
        let next = buffer("water ripples blend", cells * 8, storage);
        let source = buffer("water ripples rise", cells * 4, storage);
        let cell_water = buffer("water ripples water", cells * 16, storage);
        let blend = buffer("water ripples depth levels", cells * 4, storage);
        let points = buffer("water ripples points", MAX_STIRS as u64 * 32, storage);
        let memory = buffer("water ripples records", MAX_STIRS as u64 * 32, storage);
        let depth_min = buffer("water ripples shallowest", 4, storage);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water ripples"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let level_view = |level: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let groups = Groups {
            columns: group(
                device,
                &pipelines.columns,
                &[
                    (0, params.as_entire_binding()),
                    (3, spectrum.as_entire_binding()),
                    (4, scratch.as_entire_binding()),
                    (10, depth_min.as_entire_binding()),
                ],
            ),
            rows: group(
                device,
                &pipelines.rows,
                &[
                    (4, scratch.as_entire_binding()),
                    (5, next.as_entire_binding()),
                    (10, depth_min.as_entire_binding()),
                    (16, blend.as_entire_binding()),
                ],
            ),
            finish: group(
                device,
                &pipelines.finish,
                &[
                    (0, params.as_entire_binding()),
                    (2, state.as_entire_binding()),
                    (5, next.as_entire_binding()),
                    (7, cell_water.as_entire_binding()),
                ],
            ),
            surface: group(
                device,
                &pipelines.surface,
                &[
                    (0, params.as_entire_binding()),
                    (2, state.as_entire_binding()),
                    (13, wgpu::BindingResource::TextureView(&level_view(0))),
                ],
            ),
            mips: (1..MIPS)
                .map(|m| {
                    group(
                        device,
                        &pipelines.mip,
                        &[
                            (14, wgpu::BindingResource::TextureView(&level_view(m - 1))),
                            (15, wgpu::BindingResource::TextureView(&level_view(m))),
                        ],
                    )
                })
                .collect(),
        };
        Ripples {
            pipelines,
            groups,
            params,
            source,
            state,
            spectrum,
            _scratch: scratch,
            _next: next,
            cells: cell_water,
            blend,
            points,
            memory,
            depth_min,
            _texture: texture,
            view,
            stirs: Stirs::default(),
            origin: None,
            cell: CELL_M,
            cutoff: 0.0,
            last_t: None,
            step: None,
            fresh: true,
            active: false,
        }
    }

    /// The texture the shading samples: the rise and its slopes with their mean square, mipmapped,
    /// texel `i` holding world cell `I` with `I = i` modulo [`SIDE`].
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The window as the shading places it, all zero while it holds no ripples.
    pub fn window(&self) -> Window {
        match self.origin {
            Some(o) if self.active => [
                o[0] as f32 * self.cell,
                o[1] as f32 * self.cell,
                SIDE as f32 * self.cell,
                1.0,
            ],
            _ => [0.0; 4],
        }
    }

    /// The window's cells' edge, metres.
    pub fn cell_m(&self) -> f32 {
        self.cell
    }

    /// The feed's list of stirring points at simulated time `t`.
    pub fn set_stirs(&mut self, t: f64, stirs: &[StirView]) {
        self.stirs.set(t, stirs);
    }

    /// Starts over: no ripples, no point's record, no clock (a reset, a rewind, another region).
    pub fn clear(&mut self) {
        self.stirs.forget();
        self.origin = None;
        self.last_t = None;
        self.step = None;
        self.fresh = true;
        self.active = false;
    }

    /// Forgets the feed's lists as well (a reset frame replaces the scene).
    pub fn reset(&mut self) {
        self.clear();
        self.stirs = Stirs::default();
    }

    /// Decides this frame's step to drawn time `t`, the window placed by `place` over a field of
    /// `columns_m` columns; returns the window for the shading. A paused frame takes no step and
    /// keeps the ripples as they are.
    pub fn prepare(&mut self, t: f64, place: Placement, columns_m: f32) -> Window {
        self.step = None;
        if place.cell_m != self.cell {
            self.clear();
            self.cell = place.cell_m;
        }
        let last = self.last_t.replace(t);
        let Some(last) = last else {
            return self.window();
        };
        // Adjacent ticks can compute the same drawn boundary one ulp apart (water.rs's foam).
        let tolerance = last.abs().max(t.abs()).max(1.0) * f64::EPSILON * 4.0;
        if t < last - tolerance {
            self.clear();
            self.last_t = Some(t);
            return self.window();
        }
        if t <= last {
            self.last_t = Some(last);
            return self.window();
        }
        let dt = (t - last).min(MAX_STEP_S);
        let half = (SIDE / 2) as i32;
        let origin = [0, 1].map(|a| (place.centre[a] / self.cell).floor() as i32 - half);
        let previous = self.origin.replace(origin).unwrap_or(origin);
        let (points, stirring) = self.stirs.at(t);
        if !self.active && !stirring {
            // Nothing has stirred the window: it is still and exactly zero. Points seen now are
            // recorded once something steps.
            if points.is_empty() {
                return self.window();
            }
        }
        self.active |= stirring;
        self.cutoff = std::f32::consts::TAU / (CARRIED_COLUMNS * columns_m).min(LONGEST_M);
        self.step = Some(Step {
            dt: dt as f32,
            previous,
            points,
        });
        self.window()
    }

    /// Records the step `prepare` decided, over `field`; returns whether it recorded one.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        field: &Field<'_>,
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> bool {
        let Some(step) = self.step.take() else {
            return false;
        };
        let Some(origin) = self.origin else {
            return false;
        };
        let count = (step.points.len() / 2) as u32;
        let params = Params {
            origin,
            previous: step.previous,
            region_origin: field.origin,
            region_cell: field.cell_m,
            dt: step.dt,
            count: [field.size[0], field.size[1], u32::from(field.tiled), count],
            cutoff: self.cutoff,
            fresh: u32::from(self.fresh),
            cell: self.cell,
            unused: 0.0,
        };
        self.fresh = false;
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&self.depth_min, 0, &f32::MAX.to_bits().to_le_bytes());
        if count > 0 {
            queue.write_buffer(&self.points, 0, bytemuck::cast_slice(&step.points));
        }
        let stir = group(
            device,
            &self.pipelines.stir,
            &[
                (0, self.params.as_entire_binding()),
                (1, field.columns.as_entire_binding()),
                (6, self.source.as_entire_binding()),
                (8, self.points.as_entire_binding()),
                (9, self.memory.as_entire_binding()),
            ],
        );
        let rows = group(
            device,
            &self.pipelines.forward_rows,
            &[
                (0, self.params.as_entire_binding()),
                (1, field.columns.as_entire_binding()),
                (2, self.state.as_entire_binding()),
                (3, self.spectrum.as_entire_binding()),
                (6, self.source.as_entire_binding()),
                (7, self.cells.as_entire_binding()),
                (10, self.depth_min.as_entire_binding()),
                (16, self.blend.as_entire_binding()),
            ],
        );
        let tiles = SIDE.div_ceil(8);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water ripples"),
            timestamp_writes: profiler.and_then(|p| p.compute_scope("water ripples")),
        });
        let p = &self.pipelines;
        let g = &self.groups;
        if count > 0 {
            pass.set_pipeline(&p.stir);
            pass.set_bind_group(0, &stir, &[]);
            pass.dispatch_workgroups(count, 1, 1);
        }
        pass.set_pipeline(&p.forward_rows);
        pass.set_bind_group(0, &rows, &[]);
        pass.dispatch_workgroups(SIDE, 1, 1);
        // Column pairs m and SIDE - m, the self-mirrored 0 and SIDE / 2 alone.
        pass.set_pipeline(&p.columns);
        pass.set_bind_group(0, &g.columns, &[]);
        pass.dispatch_workgroups(SIDE / 2 + 1, 1, 1);
        pass.set_pipeline(&p.rows);
        pass.set_bind_group(0, &g.rows, &[]);
        pass.dispatch_workgroups(SIDE, 1, 1);
        pass.set_pipeline(&p.finish);
        pass.set_bind_group(0, &g.finish, &[]);
        pass.dispatch_workgroups(tiles, tiles, 1);
        pass.set_pipeline(&p.surface);
        pass.set_bind_group(0, &g.surface, &[]);
        pass.dispatch_workgroups(tiles, tiles, 1);
        pass.set_pipeline(&p.mip);
        for (m, bind) in g.mips.iter().enumerate() {
            pass.set_bind_group(0, bind, &[]);
            let side = (SIDE >> (m + 1)).div_ceil(8);
            pass.dispatch_workgroups(side, side, 1);
        }
        true
    }

    /// The window's rise read back at once: checks and tools.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn heights(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Heights, String> {
        let origin = self.origin.ok_or("the ripples have no window yet")?;
        let mut packed = vec![[0f32; 2]; (SIDE * SIDE) as usize];
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water ripples readback"),
            size: u64::from(SIDE * SIDE) * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.state, 0, &stage, 0, u64::from(SIDE * SIDE) * 8);
        queue.submit([encoder.finish()]);
        crate::readback::read_blocking(device, &stage, &mut packed)?;
        let n = SIDE as i32;
        let wrap = |a: i32| a.rem_euclid(n) as usize;
        let mut rise = Vec::with_capacity((SIDE * SIDE) as usize);
        for k in 0..n {
            for i in 0..n {
                let at = wrap(origin[0] + i) + SIDE as usize * wrap(origin[1] + k);
                rise.push(packed[at][0]);
            }
        }
        Ok(Heights {
            origin,
            cell_m: self.cell,
            rise,
        })
    }
}
