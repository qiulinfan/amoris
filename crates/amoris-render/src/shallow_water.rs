//! Shallow water as a height field (spike, docs/spikes/shallow-water.md): the columns of
//! amoris-physics `shallow` drawn in the transparent layer by forward.wgsl's `vs_shallow` and
//! `fs_shallow`, a vertex at each column's centre. The host hands the columns over each frame
//! ([`ShallowWaterView`]); nothing here simulates.

use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};

/// One frame's water: the grid's corner (world x, z), its column size, its column counts, and per
/// column (x fastest) the surface (world y), the depth and the velocity (m/s, x and z).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShallowWaterView {
    pub origin: [f32; 2],
    pub cell_m: f32,
    pub nx: u32,
    pub nz: u32,
    pub columns: Vec<[f32; 4]>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    origin: [f32; 2],
    cell: f32,
    time: f32,
    count: [u32; 4],
}

pub struct ShallowWater {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::PipelineLayout,
    group_layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    columns: Option<wgpu::Buffer>,
    group: Option<wgpu::BindGroup>,
    /// Two triangles between each four neighbouring columns, for the grid's `size`.
    indices: Option<(wgpu::Buffer, [u32; 2])>,
    quads: u32,
}

fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: SceneFormat,
) -> wgpu::RenderPipeline {
    let premultiplied = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shallow water"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_shallow"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_shallow"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &format.constants(),
                ..Default::default()
            },
            targets: &format.targets(Some(premultiplied), false),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: format.multisample(),
        multiview_mask: None,
        cache: None,
    })
}

impl ShallowWater {
    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        format: SceneFormat,
    ) -> ShallowWater {
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shallow water"),
            entries: &[
                entry(1, wgpu::BufferBindingType::Uniform),
                wgpu::BindGroupLayoutEntry {
                    visibility: wgpu::ShaderStages::VERTEX,
                    ..entry(2, wgpu::BufferBindingType::Storage { read_only: true })
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shallow water"),
            bind_group_layouts: &[
                Some(groups[0]),
                Some(groups[1]),
                Some(groups[2]),
                Some(&group_layout),
            ],
            immediate_size: 0,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shallow water"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ShallowWater {
            pipeline: pipeline(device, module, &layout, format),
            layout,
            group_layout,
            uniform,
            columns: None,
            group: None,
            indices: None,
            quads: 0,
        }
    }

    pub fn set_format(
        &mut self,
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        format: SceneFormat,
    ) {
        self.pipeline = pipeline(device, module, &self.layout, format);
    }

    /// Writes the frame's water at drawn time `t` (seconds); `None` draws none.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        water: Option<&ShallowWaterView>,
        t: f32,
    ) {
        let Some(w) = water.filter(|w| w.nx > 1 && w.nz > 1) else {
            self.quads = 0;
            return;
        };
        let bytes: &[u8] = bytemuck::cast_slice(&w.columns);
        if self
            .columns
            .as_ref()
            .is_none_or(|b| b.size() < bytes.len() as u64)
        {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shallow water columns"),
                size: (bytes.len() as u64).max(16),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shallow water"),
                layout: &self.group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            }));
            self.columns = Some(buffer);
        }
        if let Some(b) = &self.columns {
            queue.write_buffer(b, 0, bytes);
        }
        if self
            .indices
            .as_ref()
            .is_none_or(|(_, size)| *size != [w.nx, w.nz])
        {
            let mut idx = Vec::with_capacity(((w.nx - 1) * (w.nz - 1) * 6) as usize);
            for k in 0..w.nz - 1 {
                for i in 0..w.nx - 1 {
                    let a = i + w.nx * k;
                    let (b, c, d) = (a + w.nx, a + w.nx + 1, a + 1);
                    idx.extend_from_slice(&[a, b, c, a, c, d]);
                }
            }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shallow water indices"),
                size: (idx.len() * 4) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&idx));
            self.indices = Some((buffer, [w.nx, w.nz]));
        }
        let u = Uniform {
            origin: w.origin,
            cell: w.cell_m,
            time: t,
            count: [w.nx, w.nz, 0, 0],
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
        self.quads = (w.nx - 1) * (w.nz - 1);
    }

    /// Draws the water, groups 0 to 2 bound as for the lit forward pipelines.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(g), Some((indices, _))) = (self.group.as_ref(), self.indices.as_ref()) else {
            return;
        };
        if self.quads == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(3, g, &[]);
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.quads * 6, 0, 0..1);
    }
}
