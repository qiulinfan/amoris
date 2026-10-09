//! Game UI (`UiText`, `UiBar`): text and bars anchored on the screen or over entities, laid out on
//! the CPU each frame into quads and drawn over the final image. Glyphs are rasterized on demand
//! (fontdue, DejaVu Sans Bold embedded) into a shelf-packed atlas, cached per character and size.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use pocket_assets::frame::UiView;

use crate::shaders;

const ATLAS: u32 = 1024;
const FONT: &[u8] = include_bytes!("../fonts/DejaVuSans-Bold.ttf");

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[derive(Clone, Copy)]
struct Glyph {
    uv: [f32; 4], // u0 v0 u1 v1
    w: f32,
    h: f32,
    xmin: f32,
    ymin: f32,
    advance: f32,
}

pub struct Ui {
    pipeline: wgpu::RenderPipeline,
    atlas: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    vbuf: Option<wgpu::Buffer>,
    font: fontdue::Font,
    glyphs: HashMap<(char, u32), Glyph>,
    shelf: (u32, u32, u32), // x, y, row height
    pending: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    pub items: Vec<UiView>,
    /// Device pixels per logical pixel.
    pub scale: f32,
    output_srgb: bool,
}

impl Ui {
    pub fn new(device: &wgpu::Device, output: wgpu::TextureFormat) -> Ui {
        let m = shaders::module(device, "ui");
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("game ui"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &m,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &m,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS,
                height: ATLAS,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = atlas.create_view(&Default::default());
        let font = fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default())
            .unwrap_or_else(|e| panic!("embedded font: {e}"));
        let mut ui = Ui {
            pipeline,
            atlas,
            view,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("glyphs"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vbuf: None,
            font,
            glyphs: HashMap::new(),
            shelf: (4, 0, 0),
            pending: Vec::new(),
            items: Vec::new(),
            scale: 1.0,
            output_srgb: output.is_srgb(),
        };
        // A white block at the atlas origin for bars.
        ui.pending.push((0, 0, 4, 4, vec![255; 16]));
        ui
    }

    fn glyph(&mut self, c: char, px: u32) -> Glyph {
        if let Some(g) = self.glyphs.get(&(c, px)) {
            return *g;
        }
        let (m, bitmap) = self.font.rasterize(c, px as f32);
        let (w, h) = (m.width as u32, m.height as u32);
        let (mut x, mut y, mut row) = self.shelf;
        if x + w + 1 > ATLAS {
            x = 0;
            y += row + 1;
            row = 0;
        }
        let g = if w == 0 || h == 0 || y + h + 1 > ATLAS {
            Glyph {
                uv: [0.0; 4],
                w: 0.0,
                h: 0.0,
                xmin: 0.0,
                ymin: 0.0,
                advance: m.advance_width,
            }
        } else {
            self.pending.push((x, y, w, h, bitmap));
            let a = ATLAS as f32;
            let g = Glyph {
                uv: [
                    x as f32 / a,
                    y as f32 / a,
                    (x + w) as f32 / a,
                    (y + h) as f32 / a,
                ],
                w: w as f32,
                h: h as f32,
                xmin: m.xmin as f32,
                ymin: m.ymin as f32,
                advance: m.advance_width,
            };
            x += w + 1;
            row = row.max(h);
            g
        };
        self.shelf = (x, y, row);
        self.glyphs.insert((c, px), g);
        g
    }

    fn text_width(&mut self, text: &str, px: u32) -> f32 {
        text.chars().map(|c| self.glyph(c, px).advance).sum()
    }

    /// Lays out and draws the UI over `output`.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        size: (u32, u32),
        view_proj: Mat4,
    ) {
        if self.items.is_empty() {
            return;
        }
        let (w, h) = (size.0 as f32, size.1 as f32);
        let s = self.scale;
        let mut verts: Vec<Vertex> = Vec::new();
        let quad = |verts: &mut Vec<Vertex>,
                    x0: f32,
                    y0: f32,
                    x1: f32,
                    y1: f32,
                    uv: [f32; 4],
                    c: [f32; 4]| {
            let v = |x, y, u, vv| Vertex {
                pos: [x, y],
                uv: [u, vv],
                color: c,
            };
            verts.extend_from_slice(&[
                v(x0, y0, uv[0], uv[1]),
                v(x1, y0, uv[2], uv[1]),
                v(x1, y1, uv[2], uv[3]),
                v(x0, y0, uv[0], uv[1]),
                v(x1, y1, uv[2], uv[3]),
                v(x0, y1, uv[0], uv[3]),
            ]);
        };
        let white = [
            0.5 / ATLAS as f32,
            0.5 / ATLAS as f32,
            3.5 / ATLAS as f32,
            3.5 / ATLAS as f32,
        ];
        let items = std::mem::take(&mut self.items);
        for it in &items {
            let px = (it.size[1] * s).round().max(4.0) as u32;
            let (bw, bh) = if it.bar {
                (it.size[0] * s, it.size[1] * s)
            } else {
                (self.text_width(&it.text, px), px as f32)
            };
            // The box's top-left from its anchor.
            let (bx, by) = if it.anchor == 9 {
                let p = view_proj * Vec3::from(it.world).extend(1.0);
                if p.w <= 0.0 {
                    continue;
                }
                let ndc = p.truncate() / p.w;
                let sx = (ndc.x * 0.5 + 0.5) * w;
                let sy = (0.5 - ndc.y * 0.5) * h;
                (sx - bw * 0.5 + it.offset[0] * s, sy - bh + it.offset[1] * s)
            } else {
                // Offsets point inward from right and bottom edges, right and down otherwise.
                let (ax, ay) = ((it.anchor % 3) as f32 * 0.5, (it.anchor / 3) as f32 * 0.5);
                let sx = if ax == 1.0 { -1.0 } else { 1.0 };
                let sy = if ay == 1.0 { -1.0 } else { 1.0 };
                (
                    ax * (w - bw) + it.offset[0] * s * sx,
                    ay * (h - bh) + it.offset[1] * s * sy,
                )
            };
            if it.bar {
                quad(&mut verts, bx, by, bx + bw, by + bh, white, it.back);
                let inset = s.max(1.0);
                let fw = (bw - 2.0 * inset) * it.fill.clamp(0.0, 1.0);
                if fw > 0.0 {
                    quad(
                        &mut verts,
                        bx + inset,
                        by + inset,
                        bx + inset + fw,
                        by + bh - inset,
                        white,
                        it.color,
                    );
                }
                continue;
            }
            // A one-pixel shadow under the text, then the text.
            let baseline = by + px as f32 * 0.8;
            for (dx, dy, col) in [
                (s, s, [0.0, 0.0, 0.0, it.color[3] * 0.65]),
                (0.0, 0.0, it.color),
            ] {
                let mut x = bx + dx;
                for c in it.text.chars() {
                    let g = self.glyph(c, px);
                    if g.w > 0.0 {
                        let x0 = (x + g.xmin).round();
                        let y1 = (baseline + dy - g.ymin).round();
                        quad(&mut verts, x0, y1 - g.h, x0 + g.w, y1, g.uv, col);
                    }
                    x += g.advance;
                }
            }
        }
        self.items = items;
        for (x, y, gw, gh, data) in self.pending.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(gw),
                    rows_per_image: Some(gh),
                },
                wgpu::Extent3d {
                    width: gw,
                    height: gh,
                    depth_or_array_layers: 1,
                },
            );
        }
        if verts.is_empty() {
            return;
        }
        let bytes = bytemuck::cast_slice::<Vertex, u8>(&verts);
        if self
            .vbuf
            .as_ref()
            .is_none_or(|b| b.size() < bytes.len() as u64)
        {
            self.vbuf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui vertices"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(vb) = &self.vbuf else { return };
        queue.write_buffer(vb, 0, bytes);
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[w, h, if self.output_srgb { 1.0 } else { 0.0 }, 0.0]),
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("game ui"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.draw(0..verts.len() as u32, 0..1);
    }
}
