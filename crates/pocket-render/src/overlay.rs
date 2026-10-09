//! Editor overlays (charter 4.5): the gizmo's world-space lines and polygons drawn on top of the
//! final image, the selection's outline (the selected entities drawn into a mask, its edge drawn
//! over the image), and the ground grid with the world axes inside the HDR pass. The editor owns the
//! gizmo's geometry and hit testing; the renderer only draws what it is given.

use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};
use crate::shaders;

/// A line segment in world space, `width` pixels wide, colour sRGB with alpha.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct OverlayLine {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub color: [f32; 4],
    pub width: f32,
}

/// A vertex of a filled overlay triangle.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct OverlayVertex {
    pub p: [f32; 3],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    params: [f32; 4],
    select: [f32; 4],
    hover: [f32; 4],
}

pub struct Overlays {
    line: wgpu::RenderPipeline,
    poly: wgpu::RenderPipeline,
    outline: wgpu::RenderPipeline,
    mask_pipe: wgpu::RenderPipeline,
    pub grid_pipe: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    line_buf: Option<wgpu::Buffer>,
    poly_buf: Option<wgpu::Buffer>,
    pub lines: Vec<OverlayLine>,
    pub polys: Vec<OverlayVertex>,
    pub selection: Vec<u64>,
    pub hovered: Option<u64>,
    pub grid: bool,
    pub axes: bool,
    mask: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    output_srgb: bool,
    module: wgpu::ShaderModule,
}

/// The ground grid's pipeline in the opaque pass (alpha blended, depth tested, not written; the
/// opaque pass's extra targets left as they are).
fn grid_pipeline(
    device: &wgpu::Device,
    m: &wgpu::ShaderModule,
    format: SceneFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ground grid"),
        layout: None,
        vertex: wgpu::VertexState {
            module: m,
            entry_point: Some("vs_grid"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: m,
            entry_point: Some("fs_grid"),
            compilation_options: Default::default(),
            targets: &format.targets(Some(wgpu::BlendState::ALPHA_BLENDING), false),
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

const LINE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32];
const POLY_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

impl Overlays {
    /// Rebuilds the ground grid's pipeline for an opaque pass of `format`.
    pub fn set_format(&mut self, device: &wgpu::Device, format: SceneFormat) {
        self.grid_pipe = grid_pipeline(device, &self.module, format);
    }

    pub fn new(
        device: &wgpu::Device,
        output: wgpu::TextureFormat,
        format: SceneFormat,
    ) -> Overlays {
        let m = shaders::module(device, "overlay");
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let pipe = |label: &str,
                    vs: &str,
                    fs: &str,
                    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
                    format: wgpu::TextureFormat,
                    blend: Option<wgpu::BlendState>,
                    depth: Option<wgpu::DepthStencilState>,
                    samples: u32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &m,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &m,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let line = pipe(
            "overlay lines",
            "vs_line",
            "fs_flat",
            &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<OverlayLine>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &LINE_ATTRS,
            })],
            output,
            alpha,
            None,
            1,
        );
        let poly = pipe(
            "overlay polygons",
            "vs_poly",
            "fs_flat",
            &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &POLY_ATTRS,
            })],
            output,
            alpha,
            None,
            1,
        );
        let outline = pipe(
            "selection outline",
            "vs_full",
            "fs_outline",
            &[],
            output,
            alpha,
            None,
            1,
        );
        let mask_pipe = pipe(
            "selection mask",
            "vs_mask",
            "fs_mask",
            &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<pocket_assets::Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            })],
            wgpu::TextureFormat::R8Uint,
            None,
            None,
            1,
        );
        let grid_pipe = grid_pipeline(device, &m, format);
        Overlays {
            line,
            poly,
            outline,
            mask_pipe,
            grid_pipe,
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay params"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            line_buf: None,
            poly_buf: None,
            lines: Vec::new(),
            polys: Vec::new(),
            selection: Vec::new(),
            hovered: None,
            grid: false,
            axes: false,
            mask: None,
            output_srgb: output.is_srgb(),
            module: m,
        }
    }

    pub fn any(&self) -> bool {
        !self.lines.is_empty()
            || !self.polys.is_empty()
            || !self.selection.is_empty()
            || self.hovered.is_some()
    }

    pub fn write_params(&self, queue: &wgpu::Queue) {
        let p = Params {
            params: [
                if self.output_srgb { 1.0 } else { 0.0 },
                2.0,
                if self.axes { 1.0 } else { 0.0 },
                0.0,
            ],
            select: [1.0, 0.62, 0.12, 1.0],
            hover: [0.55, 0.78, 1.0, 0.9],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&p));
    }

    /// The ground grid inside the HDR pass (call after the opaque geometry).
    pub fn draw_grid(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        view: &wgpu::Buffer,
    ) {
        if !self.grid {
            return;
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("grid"),
            layout: &self.grid_pipe.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        pass.set_pipeline(&self.grid_pipe);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..1);
    }

    /// Selection mask, outline, then gizmo polygons and lines over `output`. `selected` lists
    /// (slot, index range, base vertex, hovered) for every part of the outlined entities.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        size: (u32, u32),
        view: &wgpu::Buffer,
        instances: &wgpu::Buffer,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        selected: &[(u32, std::ops::Range<u32>, i32, bool)],
    ) {
        if !self.any() {
            return;
        }
        let (w, h) = size;
        if !selected.is_empty() {
            if self.mask.as_ref().is_none_or(|m| (m.2, m.3) != (w, h)) {
                let t = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("selection mask"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Uint,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let v = t.create_view(&Default::default());
                self.mask = Some((t, v, w, h));
            }
            let Some((_, mask_view, _, _)) = &self.mask else {
                return;
            };
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("selection mask"),
                layout: &self.mask_pipe.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: view.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: instances.as_entire_binding(),
                    },
                ],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("selection mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: mask_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.mask_pipe);
            pass.set_bind_group(0, &bg, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            for (slot, range, base, hovered) in selected {
                let id = if *hovered { slot | 0x8000_0000 } else { *slot };
                pass.draw_indexed(range.clone(), *base, id..id + 1);
            }
        }
        let line_bytes = bytemuck::cast_slice::<OverlayLine, u8>(&self.lines);
        if !line_bytes.is_empty()
            && self
                .line_buf
                .as_ref()
                .is_none_or(|b| b.size() < line_bytes.len() as u64)
        {
            self.line_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay lines"),
                size: (line_bytes.len() as u64).next_power_of_two().max(256),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let poly_bytes = bytemuck::cast_slice::<OverlayVertex, u8>(&self.polys);
        if !poly_bytes.is_empty()
            && self
                .poly_buf
                .as_ref()
                .is_none_or(|b| b.size() < poly_bytes.len() as u64)
        {
            self.poly_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay polygons"),
                size: (poly_bytes.len() as u64).next_power_of_two().max(256),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if let (false, Some(b)) = (line_bytes.is_empty(), &self.line_buf) {
            queue.write_buffer(b, 0, line_bytes);
        }
        if let (false, Some(b)) = (poly_bytes.is_empty(), &self.poly_buf) {
            queue.write_buffer(b, 0, poly_bytes);
        }
        let flat_bg = |pipe: &wgpu::RenderPipeline| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("overlay"),
                layout: &pipe.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: view.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.params.as_entire_binding(),
                    },
                ],
            })
        };
        let outline_bg =
            self.mask
                .as_ref()
                .filter(|_| !selected.is_empty())
                .map(|(_, mv, _, _)| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("selection outline"),
                        layout: &self.outline.get_bind_group_layout(0),
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: self.params.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(mv),
                            },
                        ],
                    })
                });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlays"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        if let Some(bg) = &outline_bg {
            pass.set_pipeline(&self.outline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
        if let (false, Some(b)) = (self.polys.is_empty(), &self.poly_buf) {
            let bg = flat_bg(&self.poly);
            pass.set_pipeline(&self.poly);
            pass.set_bind_group(0, &bg, &[]);
            pass.set_vertex_buffer(0, b.slice(..));
            pass.draw(0..self.polys.len() as u32, 0..1);
        }
        if let (false, Some(b)) = (self.lines.is_empty(), &self.line_buf) {
            let bg = flat_bg(&self.line);
            pass.set_pipeline(&self.line);
            pass.set_bind_group(0, &bg, &[]);
            pass.set_vertex_buffer(0, b.slice(..));
            pass.draw(0..6, 0..self.lines.len() as u32);
        }
    }
}
