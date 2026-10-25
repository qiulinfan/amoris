//! Dispersive surface waves for the resident field (water.md 49), after Jeschke and Wojtan,
//! "Generalizing Shallow Water Simulations with Dispersive Surface Waves", ACM TOG 42(4), 2023.
//! A step splits the state into a bulk flow, which the ordinary substeps advance, and Airy
//! surface waves, which advance exactly per mirrored Fourier mode and are then carried by the
//! bulk flow. Shaders: `water_waves.wgsl` and `water_fft.wgsl`.
use amoris_assets::water_field::FieldBoundary;
use bytemuck::{Pod, Zeroable};

/// Airy depth levels: 0.4 m and every factor 2.5 above (matches the shader).
pub const LEVELS: u32 = 4;
/// Stationary Gaussian pyramid levels, the finest included: the coarsest blurs over a standard
/// deviation of 74 columns, the split's for 10 m of water at 0.5 m columns.
const FLOORS: u32 = 8;
/// Surface slope beyond which waves are left to the nonlinear bulk flow.
const STEEP: f32 = 0.25;
/// Damping of convergent amplification (Jeschke and Wojtan's gamma).
const GAMMA: f32 = 0.25;
const SLOT: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    size: [u32; 2],
    ext: [u32; 2],
    cell: f32,
    dt: f32,
    limit: f32,
    /// 1 when the region breaks its waves: the split stays out of breaking water.
    breaking: f32,
    open: [u32; 4],
    gamma: f32,
    steep: f32,
    unused1: [f32; 2],
}

#[derive(Clone, Copy)]
enum Kind {
    Uniform,
    Slot,
    Write,
}

pub(crate) struct WaterWaves {
    queue: wgpu::Queue,
    columns: u64,
    spectrum: u64,
    params: wgpu::Buffer,
    levels: wgpu::Buffer,
    transforms: wgpu::Buffer,
    floors: wgpu::Buffer,
    pipelines: Pipelines,
    groups: Groups,
}

struct Pipelines {
    pyramid_base: wgpu::ComputePipeline,
    pyramid_down: wgpu::ComputePipeline,
    pyramid_split: wgpu::ComputePipeline,
    pack: wgpu::ComputePipeline,
    fft: wgpu::ComputePipeline,
    rotate: wgpu::ComputePipeline,
    accumulate: wgpu::ComputePipeline,
    split_finish: wgpu::ComputePipeline,
    split_store: wgpu::ComputePipeline,
    surface_flux: wgpu::ComputePipeline,
    donors: wgpu::ComputePipeline,
    rebuild: wgpu::ComputePipeline,
    velocities: wgpu::ComputePipeline,
}

/// Bind groups; those reading the field come in a pair, one per ping-pong state buffer.
struct Groups {
    pyramid_base: [wgpu::BindGroup; 2],
    pyramid_down: wgpu::BindGroup,
    pyramid_split: [wgpu::BindGroup; 2],
    pack: wgpu::BindGroup,
    /// Flux and surface-wave spectra, then a level's mean and end flux.
    fft: [wgpu::BindGroup; 4],
    rotate: wgpu::BindGroup,
    accumulate: wgpu::BindGroup,
    split_finish: [wgpu::BindGroup; 2],
    split_store: [wgpu::BindGroup; 2],
    surface_flux: wgpu::BindGroup,
    donors: [wgpu::BindGroup; 2],
    rebuild: [wgpu::BindGroup; 2],
    velocities: [wgpu::BindGroup; 2],
}

fn layout(device: &wgpu::Device, entries: &[(u32, Kind)]) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("water waves"),
        entries: &entries
            .iter()
            .map(|&(binding, kind)| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: match kind {
                        Kind::Uniform | Kind::Slot => wgpu::BufferBindingType::Uniform,
                        Kind::Write => wgpu::BufferBindingType::Storage { read_only: false },
                    },
                    has_dynamic_offset: matches!(kind, Kind::Slot),
                    min_binding_size: None,
                },
                count: None,
            })
            .collect::<Vec<_>>(),
    })
}

impl WaterWaves {
    /// `columns` and `spectrum` are the capacities in columns and mirrored transform cells.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        state: [&wgpu::Buffer; 2],
        columns: u64,
    ) -> Self {
        let spectrum = columns * 4;
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let uniform = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let params = buffer("water waves params", 64, uniform);
        let levels = buffer("water waves levels", SLOT * u64::from(LEVELS), uniform);
        let transforms = buffer("water waves transforms", SLOT * 4, uniform);
        let floors = buffer(
            "water waves pyramid levels",
            SLOT * u64::from(FLOORS + 1),
            uniform,
        );
        let pyramid = buffer(
            "water waves pyramid",
            columns * u64::from(FLOORS) * 16,
            storage,
        );
        let spec_q = buffer("water waves flux spectrum", spectrum * 8, storage);
        let spec_s = buffer("water waves surface spectrum", spectrum * 8, storage);
        let out_avg = buffer("water waves mean flux", spectrum * 8, storage);
        let out_end = buffer("water waves end flux", spectrum * 8, storage);
        let surf = buffer("water waves surface", columns * 16, storage);
        let bulk0 = buffer("water waves bulk", columns * 16, storage);
        let acc = buffer("water waves blended flux", columns * 16, storage);
        let split = buffer("water waves blended bulk", columns * 16, storage);
        let flux = buffer("water waves surface flux", columns * 8, storage);
        let donor = buffer("water waves donors", columns * 4, storage);
        let heights = buffer("water waves heights", columns * 8, storage);

        use Kind::{Slot, Uniform, Write};
        let waves = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water waves"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water_waves.wgsl").into()),
        });
        let fft_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water fft"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water_fft.wgsl").into()),
        });
        let make = |module: &wgpu::ShaderModule, entry: &str, entries: &[(u32, Kind)]| {
            let l = layout(device, entries);
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(&l)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            });
            (l, pipeline)
        };
        let group = |l: &wgpu::BindGroupLayout, entries: &[(u32, &wgpu::Buffer)]| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water waves"),
                layout: l,
                entries: &entries
                    .iter()
                    .map(|&(binding, buffer)| wgpu::BindGroupEntry {
                        binding,
                        resource: if matches!(binding, 16 | 19) || std::ptr::eq(buffer, &transforms)
                        {
                            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer,
                                offset: 0,
                                size: wgpu::BufferSize::new(16),
                            })
                        } else {
                            buffer.as_entire_binding()
                        },
                    })
                    .collect::<Vec<_>>(),
            })
        };
        // Every storage binding is declared read_write in the shaders.
        let kinds = |bindings: &[u32]| {
            bindings
                .iter()
                .map(|&b| {
                    (
                        b,
                        match b {
                            0 => Uniform,
                            16 | 19 => Slot,
                            _ => Write,
                        },
                    )
                })
                .collect::<Vec<_>>()
        };
        let resource = |b: u32, s: &'_ wgpu::Buffer| -> wgpu::Buffer {
            match b {
                0 => params.clone(),
                1 => s.clone(),
                2 => spec_q.clone(),
                3 => spec_s.clone(),
                6 => surf.clone(),
                7 => bulk0.clone(),
                10 => out_avg.clone(),
                11 => out_end.clone(),
                12 => acc.clone(),
                13 => flux.clone(),
                14 => donor.clone(),
                15 => heights.clone(),
                16 => levels.clone(),
                17 => split.clone(),
                18 => pyramid.clone(),
                _ => floors.clone(),
            }
        };
        let kernel = |entry: &str, bindings: &[u32]| {
            let (l, pipeline) = make(&waves, entry, &kinds(bindings));
            let groups = state.map(|s| {
                let owned: Vec<(u32, wgpu::Buffer)> =
                    bindings.iter().map(|&b| (b, resource(b, s))).collect();
                group(&l, &owned.iter().map(|(b, x)| (*b, x)).collect::<Vec<_>>())
            });
            (pipeline, groups)
        };
        let (pyramid_base, pyramid_base_groups) = kernel("pyramid_base", &[0, 1, 18]);
        let (pyramid_down, [pyramid_down_group, _]) = kernel("pyramid_down", &[0, 19, 18]);
        let (pyramid_split, pyramid_split_groups) = kernel("pyramid_split", &[0, 19, 1, 18, 17]);
        let (pack, [pack_group, _]) = kernel("pack", &[0, 6, 2, 3]);
        let (rotate, [rotate_group, _]) = kernel("rotate", &[0, 16, 2, 3, 10, 11]);
        let (accumulate, [accumulate_group, _]) = kernel("accumulate", &[0, 16, 7, 10, 11, 12]);
        let (split_finish, split_finish_groups) = kernel("split_finish", &[0, 1, 6, 7, 17]);
        let (split_store, split_store_groups) = kernel("split_store", &[0, 1, 7]);
        let (surface_flux, [surface_flux_group, _]) = kernel("surface_flux", &[0, 6, 7, 12, 13]);
        let (donors, donors_groups) = kernel("donors", &[0, 1, 6, 13, 14]);
        let (rebuild, rebuild_groups) = kernel("rebuild", &[0, 1, 6, 13, 14, 15]);
        let (velocities, velocities_groups) = kernel("velocities", &[0, 1, 7, 12, 15]);

        let (l, fft) = make(&fft_module, "fft_line", &[(0, Slot), (1, Write)]);
        let fft_groups = [&spec_q, &spec_s, &out_avg, &out_end]
            .map(|data| group(&l, &[(0, &transforms), (1, data)]));

        Self {
            queue: queue.clone(),
            columns,
            spectrum,
            params,
            levels,
            transforms,
            floors,
            pipelines: Pipelines {
                pyramid_base,
                pyramid_down,
                pyramid_split,
                pack,
                fft,
                rotate,
                accumulate,
                split_finish,
                split_store,
                surface_flux,
                donors,
                rebuild,
                velocities,
            },
            groups: Groups {
                pyramid_base: pyramid_base_groups,
                pyramid_down: pyramid_down_group,
                pyramid_split: pyramid_split_groups,
                pack: pack_group,
                fft: fft_groups,
                rotate: rotate_group,
                accumulate: accumulate_group,
                split_finish: split_finish_groups,
                split_store: split_store_groups,
                surface_flux: surface_flux_group,
                donors: donors_groups,
                rebuild: rebuild_groups,
                velocities: velocities_groups,
            },
        }
    }

    pub fn fits(&self, size: [u32; 2]) -> bool {
        u64::from(size[0]) * u64::from(size[1]) <= self.columns
            && u64::from(size[0]) * u64::from(size[1]) * 4 <= self.spectrum
    }

    /// Write this step's parameters. Call once per submission, before the split.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &self,
        size: [u32; 2],
        cell: f32,
        dt: f32,
        substep_dt: f32,
        edges: &[FieldBoundary; 4],
        breaking: bool,
    ) -> Result<(), String> {
        if size
            .iter()
            .any(|n| !n.is_power_of_two() || !(16..=512).contains(n))
        {
            return Err("dispersive water needs power-of-two sides of 16 to 512 columns".into());
        }
        if !self.fits(size) {
            return Err("dispersive water exceeds its allocated capacity".into());
        }
        let params = Params {
            size,
            ext: [size[0] * 2, size[1] * 2],
            cell,
            dt,
            limit: 0.5 * cell / substep_dt,
            breaking: if breaking { 1.0 } else { 0.0 },
            open: std::array::from_fn(|i| u32::from(edges[i] != FieldBoundary::Wall)),
            gamma: GAMMA,
            steep: STEEP,
            unused1: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let mut levels = vec![0u8; (SLOT * u64::from(LEVELS)) as usize];
        for l in 0..LEVELS {
            let words: [u32; 4] = [
                (0.4f32 * 2.5f32.powi(l as i32)).to_bits(),
                l,
                u32::from(l == 0),
                0,
            ];
            let at = (SLOT * u64::from(l)) as usize;
            levels[at..at + 16].copy_from_slice(bytemuck::cast_slice(&words));
        }
        self.queue.write_buffer(&self.levels, 0, &levels);
        let [w, h] = [size[0] * 2, size[1] * 2];
        let mut transforms = vec![0u8; (SLOT * 4) as usize];
        for (slot, (axis, inverse)) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)]
            .into_iter()
            .enumerate()
        {
            let words: [u32; 4] = [w, h, axis, inverse];
            let at = SLOT as usize * slot;
            transforms[at..at + 16].copy_from_slice(bytemuck::cast_slice(&words));
        }
        self.queue.write_buffer(&self.transforms, 0, &transforms);
        let mut floors = vec![0u8; (SLOT * u64::from(FLOORS + 1)) as usize];
        for l in 0..=FLOORS {
            let at = (SLOT * u64::from(l)) as usize;
            floors[at..at + 4].copy_from_slice(&l.to_le_bytes());
        }
        self.queue.write_buffer(&self.floors, 0, &floors);
        Ok(())
    }

    fn pass(
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        group: &wgpu::BindGroup,
        offsets: &[u32],
        workgroups: [u32; 2],
        label: &'static str,
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: profiler.and_then(|p| p.compute_scope(label)),
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, offsets);
        pass.dispatch_workgroups(workgroups[0], workgroups[1], 1);
    }

    /// Pyramid levels for a grid: every level, whatever its size.
    fn floors(_size: [u32; 2]) -> u32 {
        FLOORS
    }

    /// Split the current field into bulk (left in place for the substeps) and surface waves,
    /// and take the surface waves' Airy step over the whole step.
    pub fn encode_split(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        current: usize,
        mut profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) {
        let g = [size[0].div_ceil(8), size[1].div_ceil(8)];
        let ext = [(size[0] * 2).div_ceil(8), (size[1] * 2).div_ceil(8)];
        let (p, groups) = (&self.pipelines, &self.groups);
        let floors = Self::floors(size);
        Self::pass(
            encoder,
            &p.pyramid_base,
            &groups.pyramid_base[current],
            &[],
            g,
            "water waves pyramid",
            profiler.as_deref_mut(),
        );
        for l in 1..floors {
            let offset = [(SLOT * u64::from(l)) as u32];
            Self::pass(
                encoder,
                &p.pyramid_down,
                &groups.pyramid_down,
                &offset,
                g,
                "water waves pyramid",
                profiler.as_deref_mut(),
            );
        }
        let offset = [(SLOT * u64::from(floors)) as u32];
        Self::pass(
            encoder,
            &p.pyramid_split,
            &groups.pyramid_split[current],
            &offset,
            g,
            "water waves filter",
            profiler.as_deref_mut(),
        );
        Self::pass(
            encoder,
            &p.split_finish,
            &groups.split_finish[current],
            &[],
            g,
            "water waves split",
            profiler.as_deref_mut(),
        );
        Self::pass(
            encoder,
            &p.split_store,
            &groups.split_store[current],
            &[],
            g,
            "water waves bulk",
            profiler.as_deref_mut(),
        );
        Self::pass(
            encoder,
            &p.pack,
            &groups.pack,
            &[],
            ext,
            "water waves pack",
            profiler.as_deref_mut(),
        );
        self.transform(encoder, size, 0, false, profiler.as_deref_mut());
        self.transform(encoder, size, 1, false, profiler.as_deref_mut());
        for l in 0..LEVELS {
            let offset = [(SLOT * u64::from(l)) as u32];
            Self::pass(
                encoder,
                &p.rotate,
                &groups.rotate,
                &offset,
                ext,
                "water waves airy",
                profiler.as_deref_mut(),
            );
            self.transform(encoder, size, 2, true, profiler.as_deref_mut());
            self.transform(encoder, size, 3, true, profiler.as_deref_mut());
            Self::pass(
                encoder,
                &p.accumulate,
                &groups.accumulate,
                &offset,
                g,
                "water waves blend",
                profiler.as_deref_mut(),
            );
        }
    }

    fn transform(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        data: usize,
        inverse: bool,
        mut profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) {
        let [w, h] = [size[0] * 2, size[1] * 2];
        let base = if inverse { 2 } else { 0 };
        for (slot, lines) in [(base, h), (base + 1, w)] {
            Self::pass(
                encoder,
                &self.pipelines.fft,
                &self.groups.fft[data],
                &[(SLOT * slot as u64) as u32],
                [lines, 1],
                "water waves fft",
                profiler.as_deref_mut(),
            );
        }
    }

    /// Carry the surface waves by the bulk flow and rebuild the field in `current` from the
    /// bulk substeps' result and the surface fluxes.
    pub fn encode_surface(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
        current: usize,
        mut profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) {
        let g = [size[0].div_ceil(8), size[1].div_ceil(8)];
        let (p, groups) = (&self.pipelines, &self.groups);
        Self::pass(
            encoder,
            &p.surface_flux,
            &groups.surface_flux,
            &[],
            g,
            "water waves transport",
            profiler.as_deref_mut(),
        );
        Self::pass(
            encoder,
            &p.donors,
            &groups.donors[current],
            &[],
            g,
            "water waves donors",
            profiler.as_deref_mut(),
        );
        Self::pass(
            encoder,
            &p.rebuild,
            &groups.rebuild[current],
            &[],
            g,
            "water waves heights",
            profiler.as_deref_mut(),
        );
        Self::pass(
            encoder,
            &p.velocities,
            &groups.velocities[current],
            &[],
            g,
            "water waves recombine",
            profiler,
        );
    }
}
