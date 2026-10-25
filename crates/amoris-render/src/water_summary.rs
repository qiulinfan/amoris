//! Exact resident-state digest and bounded metadata readback: 64 bytes regardless of grid size.
//! BLAKE3 authenticates the actual GPU field, not merely its host's requested tick sequence.
use crate::water_compute::GpuWater;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct Raw {
    digest: [u32; 8],
    maximum: [f32; 4],
    edge: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterStateSummary {
    pub digest: [u8; 32],
    pub max_depth: f32,
    pub max_speed: f32,
    pub max_pressure_jump: f32,
    /// Sum of column depths; multiply by cell_m² for cubic metres.
    pub volume_cells: f32,
    /// Relative to the field's bed datum.
    pub wet_edge_surface: f32,
    pub wet_edge_count: u32,
}
impl WaterStateSummary {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 64 {
            return Err("water summary must contain exactly 64 bytes".into());
        }
        bytemuck::pod_read_unaligned::<Raw>(bytes).decoded()
    }
}
impl Raw {
    fn decoded(self) -> Result<WaterStateSummary, String> {
        if self.edge[2] != 0.0
            || self
                .maximum
                .iter()
                .chain(&self.edge)
                .any(|v| !v.is_finite())
            || self.maximum.iter().any(|v| *v < 0.0)
        {
            return Err(
                "resident water summary contains invalid state or overflowed statistics".into(),
            );
        }
        let mut digest = [0u8; 32];
        for (out, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.digest) {
            out.copy_from_slice(&word.to_le_bytes());
        }
        Ok(WaterStateSummary {
            digest,
            max_depth: self.maximum[0],
            max_speed: self.maximum[1],
            max_pressure_jump: self.maximum[2],
            volume_cells: self.maximum[3],
            wet_edge_surface: if self.edge[1] > 0.0 {
                self.edge[0] / self.edge[1]
            } else {
                0.0
            },
            wet_edge_count: self.edge[1] as u32,
        })
    }
}

pub struct WaterSummary {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::ComputePipeline; 2],
    params: wgpu::Buffer,
    stride: u32,
    nodes: [wgpu::Buffer; 2],
    groups: Option<(wgpu::Buffer, [wgpu::BindGroup; 2])>,
    readback: wgpu::Buffer,
    encoded: bool,
    root: usize,
}
impl WaterSummary {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water state digest"),
            entries: &std::array::from_fn::<_, 4, _>(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding as u32,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: wgpu::BindingType::Buffer {
                    ty: if binding == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: binding != 3,
                        }
                    },
                    has_dynamic_offset: binding == 0,
                    min_binding_size: if binding == 0 {
                        std::num::NonZeroU64::new(16)
                    } else {
                        None
                    },
                },
            }),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water state digest"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water summary"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/det/water_summary.wgsl").into(),
            ),
        });
        let pipelines = ["leaves", "parents"].map(|entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: crate::shaders::compute_options(),
                cache: None,
            })
        });
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let stride = device.limits().min_uniform_buffer_offset_alignment.max(16);
        let params = buffer(
            "water summary parameters",
            u64::from(stride) * 16,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let nodes = std::array::from_fn(|_| {
            buffer(
                "water summary tree",
                4100 * 64,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            )
        });
        let readback = buffer(
            "water summary readback",
            64,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        Self {
            device: device.clone(),
            queue: queue.clone(),
            layout,
            pipelines,
            params,
            stride,
            nodes,
            groups: None,
            readback,
            encoded: false,
            root: 0,
        }
    }

    /// Encode after the solver's substeps, then submit before calling read/read_async or encoding
    /// another summary. Parameters use independent aligned slots for the entire reduction tree.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
    ) -> Result<(), String> {
        self.encode_profiled(encoder, field, None)
    }
    /// The same reduction with one timestamp scope for the complete summary pass.
    pub fn encode_profiled(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
        profiler: Option<&mut crate::profiler::GpuProfiler>,
    ) -> Result<(), String> {
        if self.encoded {
            return Err(
                "the previous water summary must be read or discarded before encoding another"
                    .into(),
            );
        }
        if !field.is_initialized() {
            return Err("GPU water has not been uploaded".into());
        }
        if self
            .groups
            .as_ref()
            .is_none_or(|(source, _)| source != field.buffer())
        {
            let groups = std::array::from_fn(|i| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("water summary tree level"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.params,
                                offset: 0,
                                size: std::num::NonZeroU64::new(16),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: field.buffer().as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.nodes[i].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: self.nodes[1 - i].as_entire_binding(),
                        },
                    ],
                })
            });
            self.groups = Some((field.buffer().clone(), groups));
        }
        let [nx, nz] = field.size();
        let columns = nx * nz;
        let words = field.state_words();
        let mut count = words.div_ceil(256);
        let mut slot = 0u32;
        let mut current = 0usize;
        // Each dispatch is a WebGPU usage scope (spec 3.4.4); wgpu inserts the required
        // storage barriers between these dependent dispatches within the same pass.
        // Uniform slots remain independent, so their queue writes cannot overwrite a
        // previous level's parameters before this encoder is submitted.
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water summary"),
            timestamp_writes: profiler.and_then(|p| p.compute_scope("water summary")),
        });
        loop {
            self.queue.write_buffer(
                &self.params,
                u64::from(slot * self.stride),
                bytemuck::cast_slice(&[columns, nx, count, words]),
            );
            let leaves = slot == 0;
            pass.set_pipeline(&self.pipelines[usize::from(!leaves)]);
            pass.set_bind_group(
                0,
                &self.groups.as_ref().unwrap().1[current],
                &[slot * self.stride],
            );
            let outputs = if leaves { count } else { count.div_ceil(2) };
            pass.dispatch_workgroups(outputs.div_ceil(64), 1, 1);
            current = 1 - current;
            if !leaves {
                count = count.div_ceil(2);
            }
            if count == 1 {
                break;
            }
            slot += 1;
        }
        drop(pass);
        encoder.copy_buffer_to_buffer(&self.nodes[current], 0, &self.readback, 0, 64);
        self.encoded = true;
        self.root = current;
        Ok(())
    }
    /// Append an encoded result to a combined GPU-only staging copy (for metadata plus samples).
    pub fn copy_to(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Buffer,
        offset: u64,
    ) -> Result<(), String> {
        if !self.encoded {
            return Err("no encoded water summary is available".into());
        }
        encoder.copy_buffer_to_buffer(&self.nodes[self.root], 0, target, offset, 64);
        Ok(())
    }
    pub async fn read_async(&mut self) -> Result<WaterStateSummary, String> {
        if !self.encoded {
            return Err("no encoded water summary is available".into());
        }
        self.encoded = false;
        let mut raw = Raw::zeroed();
        crate::readback::read(&self.readback, std::slice::from_mut(&mut raw)).await?;
        raw.decoded()
    }
    /// Ignore an unread result. Submit or drop its encoder before encoding a replacement.
    pub fn discard(&mut self) {
        self.encoded = false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(&mut self) -> Result<WaterStateSummary, String> {
        if !self.encoded {
            return Err("no encoded water summary is available".into());
        }
        self.encoded = false;
        let mut raw = Raw::zeroed();
        crate::readback::read_blocking(
            &self.device,
            &self.readback,
            std::slice::from_mut(&mut raw),
        )?;
        raw.decoded()
    }
}
