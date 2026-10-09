//! Which entity is at a pixel, and which entities a view shows: an entity-id pass drawn on demand
//! (the camera view's visible instances into an `R32Uint` target with its own depth), read back
//! without stalling. The editor picks with it; agents get "what is on screen, how much of it" with
//! a capture (Amoris Pioneer's `render.visible`, the perception an agent has of the rendered view).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::post::DEPTH;

/// What to read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickRequest {
    /// One pixel (the editor's click).
    Pixel(u32, u32),
    /// Every pixel (coverage per entity).
    Full,
}

#[derive(Default)]
struct Pending {
    request: Option<PickRequest>,
    /// Width and height of the copied region and its row pitch, in u32 texels.
    size: (u32, u32, u32),
    /// `map_async` was called for this read (it must be called once).
    mapping: bool,
    ready: bool,
    failed: bool,
}

pub struct Picking {
    pipeline: wgpu::RenderPipeline,
    target: Option<(
        wgpu::Texture,
        wgpu::TextureView,
        wgpu::TextureView,
        u32,
        u32,
    )>,
    readback: Option<wgpu::Buffer>,
    want: Option<PickRequest>,
    pending: Arc<Mutex<Pending>>,
    in_flight: bool,
}

/// A finished read: slot ids (`slot + 1`, 0 for nothing) of the region.
pub struct PickResult {
    pub request: PickRequest,
    pub ids: Vec<u32>,
}

impl Picking {
    /// `groups`: the frame group, two empty ones and the batch base (batches.rs); the pass binds
    /// group 0 and `draw` the rest.
    pub fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        groups: [&wgpu::BindGroupLayout; 4],
        vertex: wgpu::VertexBufferLayout<'_>,
    ) -> Picking {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("entity ids"),
            bind_group_layouts: &groups.map(Some),
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("entity ids"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_id"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex)],
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_id"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::R32Uint.into())],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
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
        });
        Picking {
            pipeline,
            target: None,
            readback: None,
            want: None,
            pending: Arc::new(Mutex::new(Pending::default())),
            in_flight: false,
        }
    }

    /// The state, for diagnostics.
    pub fn debug(&self) -> String {
        let p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        format!(
            "want={:?} in_flight={} request={:?} mapping={} ready={} failed={}",
            self.want, self.in_flight, p.request, p.mapping, p.ready, p.failed
        )
    }

    pub fn request(&mut self, r: PickRequest) {
        self.want = Some(r);
    }

    /// Whether this frame draws the id pass.
    pub fn wanted(&self) -> bool {
        self.want.is_some() && !self.in_flight
    }

    /// Draws the id pass (the camera view's batches through `draw`) and copies the requested region
    /// for reading back.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        size: (u32, u32),
        frame: &wgpu::BindGroup,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        draw: &dyn Fn(&mut wgpu::RenderPass<'_>),
    ) {
        let Some(req) = self.want.take() else {
            return;
        };
        let (w, h) = size;
        if self.target.as_ref().is_none_or(|t| (t.3, t.4) != (w, h)) {
            let make = |format, label| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            };
            let t = make(wgpu::TextureFormat::R32Uint, "entity ids");
            let d = make(DEPTH, "entity ids depth");
            let tv = t.create_view(&Default::default());
            let dv = d.create_view(&Default::default());
            self.target = Some((t, tv, dv, w, h));
        }
        let Some((tex, view, depth, _, _)) = &self.target else {
            return;
        };
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("entity ids"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            if let PickRequest::Pixel(x, y) = req {
                pass.set_scissor_rect(x.min(w - 1), y.min(h - 1), 1, 1);
            }
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, frame, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            draw(&mut pass);
        }
        let (ox, oy, cw, ch) = match req {
            PickRequest::Pixel(x, y) => (x.min(w - 1), y.min(h - 1), 1, 1),
            PickRequest::Full => (0, 0, w, h),
        };
        let pitch = (cw * 4).div_ceil(256) * 256;
        let bytes = u64::from(pitch) * u64::from(ch);
        if self.readback.as_ref().is_none_or(|b| b.size() < bytes) {
            self.readback = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("entity ids readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
        }
        let Some(buf) = &self.readback else {
            return;
        };
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x: ox, y: oy, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(ch),
                },
            },
            wgpu::Extent3d {
                width: cw,
                height: ch,
                depth_or_array_layers: 1,
            },
        );
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        *p = Pending {
            request: Some(req),
            size: (cw, ch, pitch / 4),
            mapping: false,
            ready: false,
            failed: false,
        };
        self.in_flight = true;
    }

    /// After the frame's submit: starts mapping the readback.
    pub fn after_submit(&mut self) {
        let Some(buf) = &self.readback else {
            return;
        };
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !self.in_flight || p.mapping || p.ready || p.failed || p.request.is_none() {
            return;
        }
        p.mapping = true;
        let state = self.pending.clone();
        let bytes = u64::from(p.size.2) * 4 * u64::from(p.size.1);
        drop(p);
        buf.slice(..bytes).map_async(wgpu::MapMode::Read, move |r| {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if r.is_ok() {
                s.ready = true;
            } else {
                s.failed = true;
            }
        });
    }

    /// The finished read, if it is ready.
    pub fn take(&mut self) -> Option<PickResult> {
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if p.failed {
            *p = Pending::default();
            self.in_flight = false;
            return None;
        }
        if !p.ready {
            return None;
        }
        let buf = self.readback.as_ref()?;
        let (cw, ch, pitch) = p.size;
        let bytes = u64::from(pitch) * 4 * u64::from(ch);
        let mut ids = Vec::with_capacity((cw * ch) as usize);
        if let Ok(data) = buf.slice(..bytes).get_mapped_range() {
            // Mapped memory in the browser carries no alignment guarantee: decode, do not cast.
            for y in 0..ch {
                let row = &data[(y * pitch * 4) as usize..((y * pitch + cw) * 4) as usize];
                ids.extend(
                    row.chunks_exact(4)
                        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                );
            }
        }
        buf.unmap();
        let request = p.request.take()?;
        *p = Pending::default();
        self.in_flight = false;
        Some(PickResult { request, ids })
    }
}

/// Pixel counts per slot id of a full read.
pub fn coverage(ids: &[u32]) -> HashMap<u32, u32> {
    let mut m = HashMap::new();
    for &id in ids {
        if id != 0 {
            *m.entry(id - 1).or_insert(0) += 1;
        }
    }
    m
}
