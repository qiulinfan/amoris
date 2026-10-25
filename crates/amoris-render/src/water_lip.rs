//! Plunging lips and falling sheets drawn as sheets of water (water.md 63.2): each face of the
//! displayed field over which the resident kernel throws water ballistically becomes a ribbon along
//! the jet's parabola, found on the GPU every frame and drawn indirectly (forward.wgsl `vs_lip`).
//! Presentation only: no field readback.
use crate::profiler::GpuProfiler;
use bytemuck::{Pod, Zeroable};

/// The ribbons a region draws at most.
pub const CAPACITY: u32 = 8192;
/// Segments along a ribbon's parabola (forward.wgsl `LIP_SEGMENTS`), two triangles each.
pub const SEGMENTS: u32 = 12;
/// A ribbon's bytes: three vec4s, read as instance attributes.
pub const STRIDE: u64 = 48;

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Params {
    count: [u32; 4],
    origin_cell: [f32; 4],
    flags: [f32; 4],
}

/// The field a region's lips are found in.
#[derive(Clone, Copy, PartialEq)]
pub struct FieldSpec {
    pub origin: [f32; 2],
    pub size: [u32; 2],
    pub cell: f32,
    pub tiled: bool,
    /// The falls' drop, metres (0: none), and whether crests plunge: the resident kernel's.
    pub falls_m: f32,
    pub plunging: bool,
}

pub struct Pipelines {
    layout: wgpu::BindGroupLayout,
    find: wgpu::ComputePipeline,
}

pub struct Pool {
    params: wgpu::Buffer,
    pub lips: wgpu::Buffer,
    pub args: wgpu::Buffer,
    group: wgpu::BindGroup,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let entry = |binding: u32, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage = |read_only| wgpu::BufferBindingType::Storage { read_only };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water lips"),
            entries: &[
                entry(0, wgpu::BufferBindingType::Uniform),
                entry(1, storage(true)),
                entry(2, storage(false)),
                entry(3, storage(false)),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water lips"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water lips"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water_lip.wgsl").into()),
        });
        let find = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water lips find"),
            layout: Some(&pl),
            module: &module,
            entry_point: Some("find"),
            compilation_options: crate::shaders::compute_options(),
            cache: None,
        });
        Self { layout, find }
    }
}

impl Pool {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        p: &Pipelines,
        columns: &wgpu::Buffer,
    ) -> Self {
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let params = buffer(
            "water lips params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let lips = buffer(
            "water lips",
            u64::from(CAPACITY) * STRIDE,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
        );
        let args = buffer(
            "water lips indirect draw",
            16,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::INDIRECT
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        );
        queue.write_buffer(&args, 0, bytemuck::cast_slice(&[SEGMENTS * 6, 0u32, 0, 0]));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water lips"),
            layout: &p.layout,
            entries: &[&params, columns, &lips, &args]
                .into_iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        });
        Self {
            params,
            lips,
            args,
            group,
        }
    }

    /// Finds this frame's lips and sheets in the region's displayed columns.
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        p: &Pipelines,
        f: FieldSpec,
        profiler: &mut GpuProfiler,
    ) {
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                count: [f.size[0], f.size[1], u32::from(f.tiled), CAPACITY],
                origin_cell: [f.origin[0], f.origin[1], f.cell, f.falls_m],
                flags: [f32::from(u8::from(f.plunging)), 0.0, 0.0, 0.0],
            }),
        );
        encoder.clear_buffer(&self.args, 4, Some(4));
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water lips"),
            timestamp_writes: profiler.compute_scope("water lips"),
        });
        pass.set_pipeline(&p.find);
        pass.set_bind_group(0, &self.group, &[]);
        pass.dispatch_workgroups(f.size[0].div_ceil(8), f.size[1].div_ceil(8), 1);
    }
}
