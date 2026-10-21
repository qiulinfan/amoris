//! The ocean (charter 2.4.1, the sailing showcase): a ring grid centred under the camera, displaced
//! in the vertex shader by the very waves amoris-physics computes buoyancy from, shaded with
//! Fresnel reflection of the sky, sun glints with the cascaded shadows, subsurface light through
//! the crests and foam on steep slopes (`fs_ocean` in forward.wgsl).

use amoris_assets::frame::SeaView;
use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};

const MAX_WAVES: usize = amoris_assets::water_field::MAX_SEA_WAVES;
const SEGMENTS: u32 = 192;
const GRAVITY: f32 = 9.81;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OceanUniform {
    level: f32,
    count: u32,
    time: f32,
    region_count: u32,
    waves: [[f32; 4]; MAX_WAVES],
    phases: [[f32; 4]; MAX_WAVES],
}

pub struct Ocean {
    pub pipeline: wgpu::RenderPipeline,
    masked_pipeline: wgpu::RenderPipeline,
    /// Depth only, one sample (`draw_depth`): the unjittered depth splats test against with TAA.
    depth_pipeline: wgpu::RenderPipeline,
    masked_depth_pipeline: wgpu::RenderPipeline,
    layout: wgpu::PipelineLayout,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    uniform: wgpu::Buffer,
    bind_layout: wgpu::BindGroupLayout,
    regions: wgpu::Buffer,
    region_capacity: usize,
    region_values: Vec<[f32; 4]>,
    views: [wgpu::BindGroup; 2],
    view_layout: wgpu::BindGroupLayout,
    view_buffers: [wgpu::Buffer; 2],
    materials: wgpu::Buffer,
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
    masked: bool,
) -> wgpu::RenderPipeline {
    let mut constants = format.constants().to_vec();
    constants.push(("OCEAN_EXCLUDE_REGIONS", f64::from(masked)));
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
                constants: &constants,
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
    masked: bool,
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
        fragment: masked.then_some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_ocean_depth"),
            compilation_options: Default::default(),
            targets: &[],
        }),
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
        self.pipeline = pipeline(device, module, &self.layout, format, false);
        self.masked_pipeline = pipeline(device, module, &self.layout, format, true);
    }

    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 3],
        format: SceneFormat,
        view_buffers: [&wgpu::Buffer; 2],
        materials: &wgpu::Buffer,
    ) -> Ocean {
        // Omit instance/draw storage. Alpha-aware RT sun shadows still need materials.
        // This leaves room for the exclusion buffer within baseline storage limits.
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean view"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let views = view_buffers.map(|b| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ocean view"),
                layout: &view_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: b.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: materials.as_entire_binding(),
                    },
                ],
            })
        });
        let layout3 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ocean"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ocean"),
            bind_group_layouts: &[
                Some(&view_layout),
                Some(groups[1]),
                Some(groups[2]),
                Some(&layout3),
            ],
            immediate_size: 0,
        });
        let pipeline = pipeline(device, module, &layout, format, false);
        let masked_pipeline = self::pipeline(device, module, &layout, format, true);
        let depth_pipeline = depth_pipeline(device, module, &layout, false);
        let masked_depth_pipeline = self::depth_pipeline(device, module, &layout, true);
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
        let regions = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ocean exclusions"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ocean"),
            layout: &layout3,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: regions.as_entire_binding(),
                },
            ],
        });
        Ocean {
            pipeline,
            masked_pipeline,
            depth_pipeline,
            masked_depth_pipeline,
            layout,
            vertices,
            indices,
            index_count: i.len() as u32,
            uniform,
            bind_layout: layout3,
            regions,
            region_capacity: 1,
            region_values: Vec::new(),
            views,
            view_layout,
            view_buffers: view_buffers.map(Clone::clone),
            materials: materials.clone(),
            group,
        }
    }

    /// Writes the sea's waves at simulated time `t`.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sea: &SeaView,
        t: f32,
        regions: Vec<[f32; 4]>,
        materials: &wgpu::Buffer,
    ) {
        if *materials != self.materials {
            self.views = self.view_buffers.each_ref().map(|b| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ocean view"),
                    layout: &self.view_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: b.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: materials.as_entire_binding(),
                        },
                    ],
                })
            });
            self.materials = materials.clone();
        }
        if regions != self.region_values {
            if regions.len() > self.region_capacity {
                self.region_capacity = regions.len().next_power_of_two();
                self.regions = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("ocean exclusions"),
                    size: self.region_capacity as u64 * 16,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ocean"),
                    layout: &self.bind_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.regions.as_entire_binding(),
                        },
                    ],
                });
            }
            if !regions.is_empty() {
                queue.write_buffer(&self.regions, 0, bytemuck::cast_slice(&regions));
            }
            self.region_values = regions;
        }
        let mut u = OceanUniform {
            level: sea.level,
            count: 0,
            time: t,
            region_count: self.region_values.len() as u32,
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
        pass.set_bind_group(0, &self.views[0], &[]);
        pass.set_pipeline(if self.region_values.is_empty() {
            &self.pipeline
        } else {
            &self.masked_pipeline
        });
        self.draw_grid(pass);
    }

    /// Draws the sea's depth alone into a one-sample depth pass (groups 0 to 2 bound as for
    /// [`Ocean::draw`]).
    pub fn draw_depth(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.views[1], &[]);
        pass.set_pipeline(if self.region_values.is_empty() {
            &self.depth_pipeline
        } else {
            &self.masked_depth_pipeline
        });
        self.draw_grid(pass);
    }

    fn draw_grid(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(3, &self.group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}
