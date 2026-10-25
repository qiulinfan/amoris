//! Caustics under the water region nearest the camera and the regions joined to it
//! (water_caustics.wgsl): a map of the sun's light gathered on the bed through the displayed
//! surface, which the forward pass multiplies into the sun on everything under that surface, with
//! the absorption along the light's path down.
use bytemuck::{Pod, Zeroable};

/// The map's side in texels; a region wider than this many finest texels gets coarser ones.
pub(crate) const MAP: u32 = 512;
/// Photons along a texel's side (water_caustics.wgsl `SUB`).
const SUB: u32 = 2;
/// The finest texel, m: wavelets shorter than four of them fade out of the map.
const FINEST_M: f32 = 0.03;
/// The map's light follows the photons with this time constant, in drawn simulated time, s. Four
/// photons a texel land in it or in its neighbour as the surface moves under them, and a texel
/// whose light jumps from frame to frame flickers on the bed; a camera's shutter or the eye would
/// blend about this much. While drawn time stands still (paused, or a frame drawn again) the map
/// stands still too, as the foam does.
const SMOOTH_S: f32 = 0.04;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [f32; 2],
    texel: f32,
    on: f32,
    size: [u32; 2],
    /// How far this frame's photons move each texel's light from what the map showed, 0 to 1.
    blend: f32,
    unused: u32,
    /// Extinction of the sun's direct light per metre of water (rgb).
    sigma: [f32; 4],
}

/// A region whose caustics are drawn: its uniform (water.rs `Uniform`), displayed columns and
/// the ripples' texture with its sampler (blank when the region has none).
pub(crate) struct Source<'a> {
    pub uniform: &'a wgpu::Buffer,
    pub columns: &'a wgpu::Buffer,
    pub origin: [f32; 2],
    pub extent: [f32; 2],
    pub sigma: [f32; 3],
    pub ripples: (&'a wgpu::TextureView, &'a wgpu::Sampler),
}

/// Where the map lies: its corner (world x, z), its texel (m) and its size in texels.
type Placement = ([f32; 2], f32, [u32; 2]);

pub(crate) struct Caustics {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 2],
    view: wgpu::TextureView,
    params: wgpu::Buffer,
    /// Each pass's texels (water_caustics.wgsl `Part`), kept between frames.
    parts: Vec<wgpu::Buffer>,
    accum: wgpu::Buffer,
    /// Each texel's light as the map last showed it (water_caustics.wgsl `shown`).
    shown: wgpu::Buffer,
    /// Where the map last lay and the drawn time it showed: another place, or time gone back,
    /// starts it afresh.
    placed: Option<(Placement, f64)>,
    on: bool,
    /// `AMORIS_CAUSTICS=off` turns them off (natively; on in the browser).
    enabled: bool,
}

impl Caustics {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("water caustics"),
            size: wgpu::Extent3d {
                width: MAP,
                height: MAP,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water caustics params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let accum = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water caustics light"),
            size: u64::from(MAP * MAP) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let shown = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water caustics shown"),
            size: u64::from(MAP * MAP) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water caustics"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Uniform),
                buffer(1, wgpu::BufferBindingType::Uniform),
                buffer(2, wgpu::BufferBindingType::Storage { read_only: true }),
                buffer(3, wgpu::BufferBindingType::Uniform),
                buffer(4, wgpu::BufferBindingType::Storage { read_only: false }),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                buffer(8, wgpu::BufferBindingType::Uniform),
                buffer(9, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water caustics"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water caustics"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../shaders/common.wgsl"),
                    include_str!("../shaders/water_caustics.wgsl")
                )
                .into(),
            ),
        });
        let pipelines = ["photons", "resolve"].map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        });
        Self {
            layout,
            pipelines,
            view: texture.create_view(&Default::default()),
            params,
            parts: Vec::new(),
            accum,
            shown,
            placed: None,
            on: true,
            enabled: !matches!(
                std::env::var("AMORIS_CAUSTICS").as_deref(),
                Ok("off" | "0" | "false" | "no")
            ),
        }
    }

    /// Whether the map may be drawn at all (`AMORIS_CAUSTICS`).
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn params(&self) -> &wgpu::Buffer {
        &self.params
    }

    /// Gathers this frame's map under `sources`, the first the region nearest the camera and the
    /// rest the regions joined to it (none, or no sun: caustics off), at drawn simulated time `t`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::Buffer,
        sources: Vec<Source<'_>>,
        t: f64,
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        let Some(s) = sources.first().filter(|_| self.enabled) else {
            if self.on {
                queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params::zeroed()));
                self.on = false;
            }
            self.placed = None;
            return;
        };
        // One map over them all.
        let low: [f32; 2] =
            std::array::from_fn(|a| sources.iter().map(|s| s.origin[a]).fold(f32::MAX, f32::min));
        let high: [f32; 2] = std::array::from_fn(|a| {
            sources
                .iter()
                .map(|s| s.origin[a] + s.extent[a])
                .fold(f32::MIN, f32::max)
        });
        let extent = [high[0] - low[0], high[1] - low[1]];
        let texel = (extent[0].max(extent[1]) / MAP as f32).max(FINEST_M);
        let size = extent.map(|e| ((e / texel).ceil() as u32).clamp(1, MAP));
        let placed = (low, texel, size);
        let blend = match self.placed {
            Some((at, last)) if at == placed && t > last => {
                1.0 - (-((t - last) as f32) / SMOOTH_S).exp()
            }
            Some((at, last)) if at == placed && t == last => 0.0,
            _ => 1.0,
        };
        self.placed = Some((placed, t));
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                origin: low,
                texel,
                on: 1.0,
                size,
                blend,
                unused: 0,
                sigma: [s.sigma[0], s.sigma[1], s.sigma[2], 0.0],
            }),
        );
        // Each region's texels: those whose centres it holds, and out to the map's edge where
        // its side is the map's. With several regions, a first pass clears the map to dry where
        // none lies.
        let texels = |s: &Source<'_>, a: usize| -> [u32; 2] {
            let edge = |at: f32, end: f32, last: u32| {
                if (at - end).abs() <= 1e-3 * texel {
                    last
                } else {
                    (((at - low[a]) / texel - 0.5).ceil().max(0.0) as u32).min(size[a])
                }
            };
            [
                edge(s.origin[a], low[a], 0),
                edge(s.origin[a] + s.extent[a], high[a], size[a]),
            ]
        };
        let mut parts: Vec<([u32; 4], u32, usize)> = Vec::new();
        if sources.len() > 1 {
            parts.push(([0, 0, size[0], size[1]], 1, 0));
        }
        for (n, s) in sources.iter().enumerate() {
            let [x, z] = [texels(s, 0), texels(s, 1)];
            if x[1] > x[0] && z[1] > z[0] {
                parts.push(([x[0], z[0], x[1] - x[0], z[1] - z[0]], 0, n));
            }
        }
        while self.parts.len() < parts.len() {
            self.parts
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("water caustics part"),
                    size: 32,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        let groups: Vec<_> = parts
            .iter()
            .enumerate()
            .map(|(p, &(texels, dry, n))| {
                let s = &sources[n];
                let words = [texels[0], texels[1], texels[2], texels[3], dry, 0, 0, 0];
                queue.write_buffer(&self.parts[p], 0, bytemuck::cast_slice(&words));
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water caustics"),
                    layout: &self.layout,
                    entries: &[
                        (0, view.as_entire_binding()),
                        (1, s.uniform.as_entire_binding()),
                        (2, s.columns.as_entire_binding()),
                        (3, self.params.as_entire_binding()),
                        (4, self.accum.as_entire_binding()),
                        (5, wgpu::BindingResource::TextureView(&self.view)),
                        (6, wgpu::BindingResource::TextureView(s.ripples.0)),
                        (7, wgpu::BindingResource::Sampler(s.ripples.1)),
                        (8, self.parts[p].as_entire_binding()),
                        (9, self.shown.as_entire_binding()),
                    ]
                    .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource }),
                })
            })
            .collect();
        // Clearing, then every region's photons, then every region's texels: a photon may land
        // in a neighbour's.
        let order = parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.1 == 1)
            .map(|(n, _)| (1, n))
            .chain(
                (0..parts.len())
                    .filter(|&n| parts[n].1 == 0)
                    .map(|n| (0, n)),
            )
            .chain(
                (0..parts.len())
                    .filter(|&n| parts[n].1 == 0)
                    .map(|n| (1, n)),
            );
        for (stage, p) in order {
            let label = ["water caustics photons", "water caustics"][stage];
            let side = if stage == 0 { SUB } else { 1 };
            let count = [parts[p].0[2], parts[p].0[3]];
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: profiler.compute_scope(label),
            });
            pass.set_bind_group(0, &groups[p], &[]);
            pass.set_pipeline(&self.pipelines[stage]);
            pass.dispatch_workgroups(
                (count[0] * side).div_ceil(8),
                (count[1] * side).div_ceil(8),
                1,
            );
        }
        self.on = true;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{BackendChoice, Gpu};

    fn f16(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exponent = i32::from((bits >> 10) & 0x1f);
        let mantissa = f32::from(bits & 0x3ff);
        sign * match exponent {
            0 => mantissa * 2f32.powi(-24),
            31 => f32::INFINITY,
            e => (1.0 + mantissa / 1024.0) * 2f32.powi(e - 15),
        }
    }

    /// The map gathered under 64 x 32 columns of 0.25 m with the sun overhead; `surface` gives
    /// each column's surface over a flat bed at -2 m, `None` a dry column. Returns the used texels.
    fn gather(
        gpu: &Gpu,
        surface: impl Fn(usize, usize) -> Option<f32>,
    ) -> (Vec<[f32; 4]>, [u32; 2]) {
        let (nx, nz) = (64usize, 32usize);
        let columns: Vec<[f32; 4]> = (0..nx * nz)
            .map(|n| match surface(n % nx, n / nx) {
                Some(s) => [s, s + 2.0, 0.0, 0.0],
                None => [0.5, 0.0, 0.0, 0.0],
            })
            .collect();
        let buffer = |label, contents: &[u8], usage| {
            use wgpu::util::DeviceExt;
            gpu.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage,
                })
        };
        let columns = buffer(
            "caustics test columns",
            bytemuck::cast_slice(&columns),
            wgpu::BufferUsages::STORAGE,
        );
        // water.rs `Uniform`: origin, cell, time, count, then absorb, scatter, surround, fog,
        // optics (no wavelets: optics.z = 0) and the ripples' window (none).
        let mut region = [0u32; 32];
        region[2] = 0.25f32.to_bits();
        region[4] = nx as u32;
        region[5] = nz as u32;
        let region = buffer(
            "caustics test region",
            bytemuck::cast_slice(&region),
            wgpu::BufferUsages::UNIFORM,
        );
        // The view uniform: the sun straight overhead (`sun_dir` after four matrices and two
        // vectors).
        let mut view = vec![0f32; 256];
        view[72..76].copy_from_slice(&[0.0, 1.0, 0.0, 10.0]);
        let view = buffer(
            "caustics test view",
            bytemuck::cast_slice(&view),
            wgpu::BufferUsages::UNIFORM,
        );
        let mut caustics = Caustics::new(&gpu.device);
        let blank = gpu
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("caustics test ripples"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let sampler = gpu.device.create_sampler(&Default::default());
        let mut profiler =
            crate::profiler::GpuProfiler::new(&gpu.device, &gpu.queue, false, &gpu.info);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        caustics.encode(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &view,
            vec![Source {
                uniform: &region,
                columns: &columns,
                origin: [0.0; 2],
                extent: [16.0, 8.0],
                sigma: [0.0; 3],
                ripples: (&blank, &sampler),
            }],
            0.0,
            &mut profiler,
        );
        let stage = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("caustics test readback"),
            size: u64::from(MAP * MAP) * 8,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            caustics.view.texture().as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &stage,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(MAP * 8),
                    rows_per_image: Some(MAP),
                },
            },
            wgpu::Extent3d {
                width: MAP,
                height: MAP,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        let mut bits = vec![0u16; (MAP * MAP * 4) as usize];
        crate::readback::read_blocking(&gpu.device, &stage, &mut bits).unwrap();
        // 16 m over 512 texels: 3.125 cm texels, 512 x 256 of them.
        let size = [MAP, MAP / 2];
        let texels = (0..size[1])
            .flat_map(|j| (0..size[0]).map(move |i| (i, j)))
            .map(|(i, j)| {
                let at = ((j * MAP + i) * 4) as usize;
                std::array::from_fn(|c| f16(bits[at + c]))
            })
            .collect();
        (texels, size)
    }

    #[test]
    fn flat_water_gathers_its_own_light_and_a_wave_focuses_it_without_creating_any() {
        for backend in BackendChoice::under_test() {
            let gpu = Gpu::headless(backend).expect("caustics need a GPU");
            gpu.device
                .on_uncaptured_error(std::sync::Arc::new(|e| panic!("caustics: {e}")));
            let (flat, size) = gather(&gpu, |i, _| (i < 60).then_some(0.0));
            let at = |t: &[[f32; 4]], i: u32, j: u32| t[(j * size[0] + i) as usize];
            for j in 2..size[1] - 2 {
                for i in 2..450 {
                    let t = at(&flat, i, j);
                    assert!((t[0] - 1.0).abs() < 0.01, "flat light {t:?} at {i} {j}");
                    assert!((t[1] - 2.0).abs() < 0.01 && t[3].abs() < 1e-3, "{t:?}");
                }
            }
            // Columns 60 and on are dry: no water over them, nothing gathered.
            let dry = at(&flat, 505, 100);
            assert!(dry[0] == 1.0 && dry[1] == 0.0 && dry[3] < -1e3, "{dry:?}");
            // A 2 m wave of 4 cm along x: crests focus the light, troughs spread it, and the
            // light over whole wavelengths is what flat water gathers. With the sun overhead a
            // slope s turns the ray by (1 - 3/4) s, so 2 m down the light lands at
            // x + 0.5 eta'(x): its density is 1 / (1 + 0.5 eta''), between 1 / 1.197 and
            // 1 / 0.803 for eta'' = 0.04 pi^2 sin.
            let (wave, _) = gather(&gpu, |i, _| {
                Some(0.04 * (std::f32::consts::TAU * (i as f32 + 0.5) * 0.25 / 2.0).sin())
            });
            let row: Vec<f32> = (0..384).map(|i| at(&wave, i + 40, 128)[0]).collect();
            let (low, high) = row
                .iter()
                .fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
            assert!(
                (low - 0.835).abs() < 0.025 && (high - 1.245).abs() < 0.035,
                "{low} {high}"
            );
            // 2 m is 64 texels: average over six wavelengths.
            let mean = row.iter().sum::<f32>() / 384.0;
            assert!((mean - 1.0).abs() < 0.02, "mean {mean}");
        }
    }
}
