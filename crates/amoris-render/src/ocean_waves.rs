//! The wind sea (water.md 60): the waves a `Wind` raises over the sea's fetch, synthesized every
//! frame on the GPU in three cascades (`ocean_waves.wgsl`), drawn on the sea to the horizon and
//! as the surface detail of the water regions beside it (forward.wgsl `wind_surface`). Drawn
//! only: amoris-physics floats bodies on the sea's `waves`.

use amoris_assets::frame::WindSeaView;
use bytemuck::{Pod, Zeroable};

/// Wave vectors along a side of each cascade.
const N: u32 = 256;
/// Mip levels of the texture array, 256 down to 1.
const LEVELS: u32 = 9;
/// Displacement and foam for each cascade, then slopes and derivatives for each.
const LAYERS: u32 = 6;
/// Each cascade's tile (m). The bands meet where the next tile holds two waves: the first takes
/// wavelengths over 20 m, the second 3 to 20 m, the third down to 4.7 cm.
pub(crate) const TILES: [f32; 3] = [320.0, 40.0, 6.0];
const GRAVITY: f32 = 9.81;
/// How far the crests' horizontal displacement sharpens them (Tessendorf's lambda).
const CHOPPINESS: f32 = 0.9;
/// Foam fades by this share a second.
const FOAM_DECAY: f32 = 0.7;
/// How much longer a whitecap shows than its crest breaks: its foam fades over 1 / FOAM_DECAY s
/// after a break of about half a peak period.
const FOAM_LINGERS: f32 = 1.7;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Spectrum {
    cascades: [[f32; 4]; 3],
    wind: [f32; 4],
    shape: [f32; 4],
    foam: [f32; 4],
    /// Per cascade, the spread of its Jacobian about 1 (0: it makes no foam).
    spread: [f32; 4],
}

/// What the sea's and the regions' shaders read (forward.wgsl `WindSea`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub(crate) struct WindSeaUniform {
    /// Per cascade: 1 / its tile, its texel (m), the angular frequency at the middle of its band.
    cascades: [[f32; 4]; 3],
    /// Per mip level, the mean square slope of each cascade's waves finer than its texels there.
    lost: [[f32; 4]; LEVELS as usize],
    /// x: on, y: the mean square slope finer than the finest cascade, z: how much foam shows.
    params: [f32; 4],
}

/// The JONSWAP spectrum of a fetch-limited sea (Hasselmann et al. 1973): its peak frequency
/// (rad/s), 22 (g^2 / (U F))^(1/3) as C. J. Horvath gives it, and its alpha, scaled so that the
/// spectrum holds the energy JONSWAP's own law gives, g^2 m0 / U^4 = 1.6e-7 g F / U^2: the
/// published alpha, 0.076 (g F / U^2)^-0.22, puts 30 percent more into the significant height.
/// Past g F / U^2 = 17 000 the sea is fully developed: the peak is then Pierson and Moskowitz's
/// 0.855 g / U and the energy theirs too.
pub(crate) fn jonswap(speed: f32, fetch_m: f32) -> (f32, f32) {
    let fetch = (GRAVITY * fetch_m.max(1.0) / (speed * speed)).min(17_000.0);
    let peak = 22.0 * GRAVITY / speed * fetch.powf(-1.0 / 3.0);
    let unit = integrate((1e-4, 1e4), peak, 1.0, |_| 1.0);
    let energy = 1.6e-7 * fetch * speed.powi(4) / (GRAVITY * GRAVITY);
    (peak, energy / unit)
}

/// S(omega), m^2 s, for gamma 3.3.
pub(crate) fn spectrum(w: f32, peak: f32, alpha: f32) -> f32 {
    let sigma = if w <= peak { 0.07 } else { 0.09 };
    let r = (-(w - peak) * (w - peak) / (2.0 * sigma * sigma * peak * peak)).exp();
    alpha * GRAVITY * GRAVITY / w.powi(5) * (-1.25 * (peak / w).powi(4)).exp() * 3.3f32.powf(r)
}

/// The integral of `f(omega) S(omega)` between two wavenumbers (rad/m), deep water, by the
/// midpoint rule over log omega.
fn integrate(k: (f32, f32), peak: f32, alpha: f32, f: impl Fn(f32) -> f32) -> f32 {
    let (a, b) = ((GRAVITY * k.0).sqrt().ln(), (GRAVITY * k.1).sqrt().ln());
    if b <= a {
        return 0.0;
    }
    let n = 512;
    let step = (b - a) / n as f32;
    (0..n)
        .map(|i| {
            let w = (a + (i as f32 + 0.5) * step).exp();
            f(w) * spectrum(w, peak, alpha) * w * step
        })
        .sum()
}

/// Each cascade's band of wavenumbers [low, high).
fn bands() -> [(f32, f32); 3] {
    let nyquist = std::f32::consts::PI * N as f32 / TILES[2];
    let tau = std::f32::consts::TAU;
    [
        (tau / TILES[0], 2.0 * tau / TILES[1]),
        (2.0 * tau / TILES[1], 2.0 * tau / TILES[2]),
        (2.0 * tau / TILES[2], nyquist),
    ]
}

/// The share of the sea whitecaps cover, as E. C. Monahan and I. Ó Muircheartaigh fitted it
/// ("Optimal power-law description of oceanic whitecap coverage dependence on wind speed", J.
/// Phys. Oceanogr. 10, 1980): 3.84e-6 U^3.41, the wind at 10 m.
fn whitecaps(speed: f32) -> f32 {
    3.84e-6 * speed.powf(3.41)
}

/// The standard normal quantile of `p`, by bisection on Abramowitz and Stegun's erfc (7.1.26).
fn quantile(p: f32) -> f32 {
    let below = |z: f32| {
        let x = (z / std::f32::consts::SQRT_2).abs();
        let t = 1.0 / (1.0 + 0.327_591_1 * x);
        let erfc = t
            * (0.254_829_6
                + t * (-0.284_496_7 + t * (1.421_413_7 + t * (-1.453_152 + t * 1.061_405_4))))
            * (-x * x).exp();
        if z < 0.0 {
            0.5 * erfc
        } else {
            1.0 - 0.5 * erfc
        }
    };
    let (mut lo, mut hi) = (-8.0f32, 8.0f32);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if below(mid) < p { lo = mid } else { hi = mid }
    }
    0.5 * (lo + hi)
}

/// The mean square slope C. Cox and W. Munk measured over clean seas ("Measurement of the
/// roughness of the sea surface from photographs of the sun's glitter", JOSA 44(11), 1954):
/// 0.003 + 5.12e-3 U, the wind at 12.5 m (about the wind at 10 m here).
fn cox_munk(speed: f32) -> f32 {
    0.003 + 5.12e-3 * speed
}

impl Spectrum {
    /// The spectrum a wind raises, at time 0.
    fn new(wind: &WindSeaView) -> Spectrum {
        let (peak, alpha) = jonswap(wind.speed, wind.fetch_km * 1000.0);
        let toward = wind.toward_deg.to_radians();
        let bands = bands();
        // Whitecaps: the crests of the waves over 20 m, the wind sea's peak among them, fold where
        // their Jacobian, near 1 + lambda div D, which spreads about 1 by lambda times the band's
        // root mean square slope, falls under a threshold, set so that the foam they leave covers
        // Monahan's share of the sea. The shorter cascades' tiles repeat every 40 and 6 m, which
        // their foam would show as a lattice from high over the sea, so they make none.
        let foamy = (whitecaps(wind.speed) / FOAM_LINGERS).clamp(1e-6, 0.5);
        let spread: [f32; 3] = std::array::from_fn(|c| {
            let slope = integrate(bands[c], peak, alpha, |w| w.powi(4) / (GRAVITY * GRAVITY));
            if c == 0 {
                CHOPPINESS * slope.sqrt()
            } else {
                0.0
            }
        });
        let threshold = |c: usize| 1.0 + spread[c] * quantile(foamy);
        Spectrum {
            cascades: std::array::from_fn(|c| [TILES[c], bands[c].0, bands[c].1, threshold(c)]),
            wind: [toward.sin(), -toward.cos(), wind.speed, peak],
            shape: [alpha, 3.3, CHOPPINESS, 0.0],
            foam: [0.0, FOAM_DECAY, 0.0, f32::from_bits(0x5eed)],
            spread: [spread[0], spread[1], spread[2], 0.0],
        }
    }
}

impl WindSeaUniform {
    pub fn off() -> WindSeaUniform {
        WindSeaUniform {
            cascades: [[1.0, 1.0, 1.0, 0.0]; 3],
            lost: [[0.0; 4]; LEVELS as usize],
            params: [0.0; 4],
        }
    }

    fn new(wind: &WindSeaView) -> WindSeaUniform {
        let (peak, alpha) = jonswap(wind.speed, wind.fetch_km * 1000.0);
        let bands = bands();
        let slope = |k: (f32, f32)| integrate(k, peak, alpha, |w| w.powi(4) / (GRAVITY * GRAVITY));
        let mut u = WindSeaUniform::off();
        let mut resolved = 0.0;
        for (c, &band) in bands.iter().enumerate() {
            let texel = TILES[c] / N as f32;
            let middle = (band.0 * band.1).sqrt();
            u.cascades[c] = [1.0 / TILES[c], texel, (GRAVITY * middle).sqrt(), 0.0];
            resolved += slope(band);
            for level in 0..LEVELS as usize {
                // A mip level resolves the waves up to its Nyquist wavenumber; a coarser one never
                // loses less (the sums of a narrower band can come out a hair larger).
                let finest = std::f32::consts::PI / (texel * (1u32 << level) as f32);
                let lost = slope((finest.max(band.0), band.1));
                u.lost[level][c] = if level == 0 {
                    lost
                } else {
                    lost.max(u.lost[level - 1][c])
                };
            }
        }
        // What the cascades leave to the capillaries, as Cox and Munk measured the total.
        u.params = [1.0, (cox_munk(wind.speed) - resolved).max(0.0), 1.0, 0.0];
        u
    }
}

pub(crate) struct OceanWaves {
    rows: wgpu::ComputePipeline,
    columns: wgpu::ComputePipeline,
    mips: wgpu::ComputePipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// Levels 1 to 4 from level 0, then 5 to 8 from level 4.
    mip_groups: [wgpu::BindGroup; 2],
    view: wgpu::TextureView,
    params: Spectrum,
    /// The wind the spectrum was made for, and what the shaders read for it.
    made_for: Option<(WindSeaView, WindSeaUniform)>,
    last_time: Option<f32>,
}

impl OceanWaves {
    pub fn new(device: &wgpu::Device) -> OceanWaves {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ocean waves"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/ocean_waves.wgsl").into()),
        });
        let compute = wgpu::ShaderStages::COMPUTE;
        let buffer = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let written = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        };
        // One layout for the synthesis, one for the mip levels.
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean waves"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Uniform),
                buffer(1, wgpu::BufferBindingType::Storage { read_only: false }),
                buffer(2, wgpu::BufferBindingType::Storage { read_only: false }),
                written(3),
            ],
        });
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean waves mip"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                written(1),
                written(2),
                written(3),
                written(4),
            ],
        });
        let pipeline = |entry: &str, layout: &wgpu::BindGroupLayout| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some(entry),
                        bind_group_layouts: &[Some(layout)],
                        immediate_size: 0,
                    }),
                ),
                module: &module,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        };
        let rows = pipeline("waves_rows", &layout);
        let columns = pipeline("waves_columns", &layout);
        let mips = pipeline("waves_mips", &mip_layout);
        let storage = wgpu::BufferUsages::STORAGE;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean waves spectrum"),
            size: std::mem::size_of::<Spectrum>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cells = u64::from(N * N) * 3;
        // Four complex fields a cascade, transformed along x.
        let fields = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean waves fields"),
            size: cells * 4 * 8,
            usage: storage,
            mapped_at_creation: false,
        });
        let foam = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean waves foam"),
            size: cells * 4,
            usage: storage,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ocean waves"),
            size: wgpu::Extent3d {
                width: N,
                height: N,
                depth_or_array_layers: LAYERS,
            },
            mip_level_count: LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            // The checks read it back.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let level = |l: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_mip_level: l,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean waves"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fields.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: foam.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&level(0)),
                },
            ],
        });
        let mip_group = |from: u32| {
            let views: [wgpu::TextureView; 5] = std::array::from_fn(|i| level(from + i as u32));
            let entries: [wgpu::BindGroupEntry; 5] =
                std::array::from_fn(|i| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: wgpu::BindingResource::TextureView(&views[i]),
                });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ocean waves mip"),
                layout: &mip_layout,
                entries: &entries,
            })
        };
        let mip_groups = [mip_group(0), mip_group(4)];
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        OceanWaves {
            rows,
            columns,
            mips,
            uniform,
            group,
            mip_groups,
            view,
            params: Spectrum::zeroed(),
            made_for: None,
            last_time: None,
        }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// This frame's wind sea at simulated time `t`; returns what the shaders read.
    pub fn update(&mut self, queue: &wgpu::Queue, wind: &WindSeaView, t: f32) -> WindSeaUniform {
        if self.made_for.is_none_or(|(made, _)| made != *wind) {
            self.made_for = Some((*wind, WindSeaUniform::new(wind)));
            self.params = Spectrum::new(wind);
        }
        // Foam grows over the time since the last frame; a rewind clears it.
        let dt = match self.last_time {
            Some(last) if t < last => -1.0,
            Some(last) => t - last,
            None => 0.0,
        };
        self.last_time = Some(t);
        self.params.shape[3] = t;
        self.params.foam[0] = dt;
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&self.params));
        self.made_for.expect("the spectrum is made").1
    }

    /// Synthesizes the frame's waves.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ocean waves"),
            timestamp_writes: profiler.compute_scope("ocean waves"),
        });
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_pipeline(&self.rows);
        pass.dispatch_workgroups(N, 1, 3);
        pass.set_pipeline(&self.columns);
        pass.dispatch_workgroups(N, 1, 3);
        pass.set_pipeline(&self.mips);
        pass.set_bind_group(0, &self.mip_groups[0], &[]);
        pass.dispatch_workgroups(N / 16, N / 16, LAYERS);
        pass.set_bind_group(0, &self.mip_groups[1], &[]);
        pass.dispatch_workgroups(1, 1, LAYERS);
    }
}

/// A texture array of one texel a layer, all zero: what the shaders bind with no wind sea.
pub(crate) fn blank(device: &wgpu::Device) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("no ocean waves"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
}

/// The wrapping, filtered sampler the shaders read the waves with: trilinear and anisotropic, as
/// the sea runs to the horizon at a grazing angle. 8 samples: 16 made the surf bay's frames 0.4 ms
/// longer at 1280 x 720 on the M5 and changed them by under a quarter of a level on average.
pub(crate) fn sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("ocean waves"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Level `level` of the texture array: per layer, per texel (x fastest), rgba.
    fn read(gpu: &crate::Gpu, waves: &OceanWaves, level: u32) -> Vec<[f32; 4]> {
        let side = N >> level;
        // A copy's rows are 256-byte aligned.
        let row = (side * 8).next_multiple_of(256);
        let stage = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean waves check"),
            size: u64::from(row * side * LAYERS),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: waves.view.texture(),
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &stage,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(side),
                },
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: LAYERS,
            },
        );
        gpu.queue.submit([encoder.finish()]);
        stage
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        let data = stage.slice(..).get_mapped_range().unwrap();
        let half = |b: &[u8]| amoris_assets::neural::f16_to_f32(u16::from_le_bytes([b[0], b[1]]));
        (0..(LAYERS * side) as usize)
            .flat_map(|r| {
                let at = r * row as usize;
                data[at..at + side as usize * 8]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|t| std::array::from_fn(|c| half(&t[2 * c..])))
                    .collect::<Vec<[f32; 4]>>()
            })
            .collect()
    }

    /// The synthesized sea holds its spectrum: over the three tiles the heights' variance is the
    /// spectrum's m0 and the slopes' the mean square slope the bands hold, within a realization's
    /// spread; every texel is finite; each mip level averages the texels of level 0 it covers.
    #[test]
    fn the_synthesized_sea_holds_its_spectrum() {
        let Ok(gpu) = crate::Gpu::headless(crate::BackendChoice::from_env()) else {
            eprintln!("no GPU: skipped");
            return;
        };
        let mut waves = OceanWaves::new(&gpu.device);
        let wind = WindSeaView {
            speed: 8.0,
            toward_deg: 90.0,
            fetch_km: 50.0,
        };
        waves.update(&gpu.queue, &wind, 10.0);
        let mut profiler =
            crate::profiler::GpuProfiler::new(&gpu.device, &gpu.queue, false, &gpu.info);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        waves.encode(&mut encoder, &mut profiler);
        gpu.queue.submit([encoder.finish()]);
        let texels = read(&gpu, &waves, 0);
        assert!(
            texels.iter().flatten().all(|v| v.is_finite()),
            "a texel is not finite"
        );
        let cells = (N * N) as usize;
        // Layer c holds cascade c's displacement (x, y, z) and foam, layer c + 3 its slopes.
        let mean_square = |layer: usize, channel: usize| {
            texels[layer * cells..(layer + 1) * cells]
                .iter()
                .map(|t| f64::from(t[channel]).powi(2))
                .sum::<f64>()
                / cells as f64
        };
        let (peak, alpha) = jonswap(8.0, 50_000.0);
        let b = bands();
        let m0: f32 = b.iter().map(|&k| integrate(k, peak, alpha, |_| 1.0)).sum();
        let mss: f32 = b
            .iter()
            .map(|&k| integrate(k, peak, alpha, |w| w.powi(4) / (GRAVITY * GRAVITY)))
            .sum();
        let heights: f64 = (0..3).map(|c| mean_square(c, 1)).sum();
        let slopes: f64 = (0..3)
            .map(|c| mean_square(c + 3, 0) + mean_square(c + 3, 1))
            .sum();
        eprintln!(
            "heights' variance {heights:.4} m^2 against m0 {m0:.4}; slopes' {slopes:.4} against {mss:.4}"
        );
        assert!(
            (heights / f64::from(m0) - 1.0).abs() < 0.2,
            "{heights} {m0}"
        );
        assert!(
            (slopes / f64::from(mss) - 1.0).abs() < 0.1,
            "{slopes} {mss}"
        );
        // Level 4 (16 x 16 texels of 0 each) and level 8 (all of them), every layer and channel.
        for level in [4, 8] {
            let side = N >> level;
            let above = 1usize << level;
            let got = read(&gpu, &waves, level);
            for (n, texel) in got.iter().enumerate() {
                let (layer, y, x) = (
                    n / (side * side) as usize,
                    n / side as usize % side as usize,
                    n % side as usize,
                );
                for c in 0..4 {
                    let want = (0..above * above)
                        .map(|i| {
                            let at = layer * cells
                                + (y * above + i / above) * N as usize
                                + x * above
                                + i % above;
                            f64::from(texels[at][c])
                        })
                        .sum::<f64>()
                        / (above * above) as f64;
                    let error = (f64::from(texel[c]) - want).abs();
                    assert!(
                        error < 2e-3 * want.abs().max(0.05),
                        "level {level} texel {n} channel {c}: {} against {want}",
                        texel[c]
                    );
                }
            }
        }
    }

    /// At 8 m/s over 50 km: a 4.6 s peak and JONSWAP's significant height, 0.0016 sqrt(g F / U^2)
    /// U^2 / g = 0.91 m; the cascades cover the waves that hold the energy; the slopes they
    /// resolve stay under Cox and Munk's measured total, and coarser mip levels lose more.
    #[test]
    fn a_fetch_limited_sea() {
        let (peak, alpha) = jonswap(8.0, 50_000.0);
        let period = std::f32::consts::TAU / peak;
        assert!((period - 4.59).abs() < 0.05, "{period}");
        let m0 = integrate((1e-4, 1e4), peak, alpha, |_| 1.0);
        let hs = 4.0 * m0.sqrt();
        let law = 0.0016 * (GRAVITY * 50_000.0 / 64.0).sqrt() * 64.0 / GRAVITY;
        assert!((hs / law - 1.0).abs() < 0.02, "{hs} against {law}");
        let b = bands();
        assert_eq!(b[0].1, b[1].0);
        assert_eq!(b[1].1, b[2].0);
        let covered: f32 = b.iter().map(|&k| integrate(k, peak, alpha, |_| 1.0)).sum();
        assert!(covered / m0 > 0.97, "{}", covered / m0);
        let u = WindSeaUniform::new(&WindSeaView {
            speed: 8.0,
            toward_deg: 90.0,
            fetch_km: 50.0,
        });
        let resolved: f32 = b
            .iter()
            .map(|&k| integrate(k, peak, alpha, |w| w.powi(4) / (GRAVITY * GRAVITY)))
            .sum();
        assert!(resolved < cox_munk(8.0), "{resolved}");
        assert!((u.params[1] - (cox_munk(8.0) - resolved)).abs() < 1e-4);
        for c in 0..3 {
            for l in 1..LEVELS as usize {
                assert!(u.lost[l][c] >= u.lost[l - 1][c], "{c} {l}");
            }
        }
        // A fully developed sea: Pierson and Moskowitz's peak, however long the fetch.
        let (far, _) = jonswap(8.0, 5_000_000.0);
        assert!((far / (0.855 * GRAVITY / 8.0) - 1.0).abs() < 0.01, "{far}");
    }
}
