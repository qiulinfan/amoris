//! The ocean (charter 2.4.1, the sailing showcase): a ring grid centred under the camera, displaced
//! in the vertex shader by the very waves pocket-physics computes buoyancy from, shaded with
//! Fresnel reflection of the sky, sun glints with the cascaded shadows, subsurface light through
//! the crests and foam on steep slopes (`fs_ocean` in forward.wgsl).

use bytemuck::{Pod, Zeroable};
use pocket_assets::frame::SeaView;

use crate::post::{DEPTH, SceneFormat};

const MAX_WAVES: usize = 16;
const SEGMENTS: u32 = 192;
const GRAVITY: f32 = 9.81;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OceanUniform {
    level: f32,
    count: u32,
    time: f32,
    _p: f32,
    waves: [[f32; 4]; MAX_WAVES],
    phases: [[f32; 4]; MAX_WAVES],
}

pub struct Ocean {
    pub pipeline: wgpu::RenderPipeline,
    /// Depth only, one sample (`draw_depth`): the unjittered depth splats test against with TAA.
    depth_pipeline: wgpu::RenderPipeline,
    layout: wgpu::PipelineLayout,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    uniform: wgpu::Buffer,
    pub group: wgpu::BindGroup,
}

fn ring_grid() -> (Vec<[f32; 2]>, Vec<u32>) {
    let mut radii = vec![0.0f32];
    let mut r = 0.4f32;
    while r < 25_000.0 {
        radii.push(r);
        r *= 1.04;
    }
    let mut v = Vec::new();
    v.push([0.0, 0.0]);
    for &r in &radii[1..] {
        for s in 0..SEGMENTS {
            let a = std::f32::consts::TAU * s as f32 / SEGMENTS as f32;
            v.push([a.cos() * r, a.sin() * r]);
        }
    }
    let mut idx = Vec::new();
    // Centre fan.
    for s in 0..SEGMENTS {
        idx.extend_from_slice(&[0, 1 + (s + 1) % SEGMENTS, 1 + s]);
    }
    for ring in 0..(radii.len() as u32 - 2) {
        let a0 = 1 + ring * SEGMENTS;
        let b0 = a0 + SEGMENTS;
        for s in 0..SEGMENTS {
            let s1 = (s + 1) % SEGMENTS;
            idx.extend_from_slice(&[a0 + s, a0 + s1, b0 + s, a0 + s1, b0 + s1, b0 + s]);
        }
    }
    (v, idx)
}

/// The sea's pipeline in the opaque pass. `fs_ocean` writes every target (no indirect share, no
/// object motion, its normal): it writes depth over what lies under the water, whose share and
/// motion would otherwise show through (GTAO darkening the sea, a sunk hull's motion moving it).
fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: SceneFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ocean"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_ocean"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_ocean"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &format.constants(),
                ..Default::default()
            },
            targets: &format.targets(None, true),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: format.multisample(),
        multiview_mask: None,
        cache: None,
    })
}

/// The sea's depth alone, at one sample (renderer.rs `UnjitteredDepth`).
fn depth_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ocean (depth)"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_ocean"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2],
            })],
        },
        fragment: None,
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl Ocean {
    /// Rebuilds the pipeline for an opaque pass of `format` (`module`: the lit forward module).
    pub fn set_format(
        &mut self,
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        format: SceneFormat,
    ) {
        self.pipeline = pipeline(device, module, &self.layout, format);
    }

    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        format: SceneFormat,
    ) -> Ocean {
        let layout3 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ocean"),
            bind_group_layouts: &[
                Some(groups[0]),
                Some(groups[1]),
                Some(groups[2]),
                Some(&layout3),
            ],
            immediate_size: 0,
        });
        let pipeline = pipeline(device, module, &layout, format);
        let depth_pipeline = depth_pipeline(device, module, &layout);
        let (v, i) = ring_grid();
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean grid"),
            size: (v.len() * 8) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        vertices
            .slice(..)
            .get_mapped_range_mut()
            .expect("mapped at creation")
            .copy_from_slice(bytemuck::cast_slice(&v));
        vertices.unmap();
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean grid indices"),
            size: (i.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        indices
            .slice(..)
            .get_mapped_range_mut()
            .expect("mapped at creation")
            .copy_from_slice(bytemuck::cast_slice(&i));
        indices.unmap();
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean"),
            size: std::mem::size_of::<OceanUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean"),
            layout: &layout3,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        Ocean {
            pipeline,
            depth_pipeline,
            layout,
            vertices,
            indices,
            index_count: i.len() as u32,
            uniform,
            group,
        }
    }

    /// Writes the sea's waves at simulated time `t`.
    pub fn update(&self, queue: &wgpu::Queue, sea: &SeaView, t: f32) {
        let mut u = OceanUniform {
            level: sea.level,
            count: 0,
            time: t,
            _p: 0.0,
            waves: [[0.0; 4]; MAX_WAVES],
            phases: [[0.0; 4]; MAX_WAVES],
        };
        for w in sea.waves.iter().filter(|w| w.length > 0.0).take(MAX_WAVES) {
            let i = u.count as usize;
            let k = std::f32::consts::TAU / w.length;
            let b = w.toward_deg.to_radians();
            u.waves[i] = [b.sin(), -b.cos(), k, 0.5 * w.height];
            u.phases[i] = [(GRAVITY * k).sqrt(), w.phase, 0.0, 0.0];
            u.count += 1;
        }
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        self.draw_grid(pass);
    }

    /// Draws the sea's depth alone into a one-sample depth pass (groups 0 to 2 bound as for
    /// [`Ocean::draw`]).
    pub fn draw_depth(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.depth_pipeline);
        self.draw_grid(pass);
    }

    fn draw_grid(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(3, &self.group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}
