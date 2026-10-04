//! The frame's targets (multisampled HDR color and reversed-Z depth, the resolved HDR image, the
//! bloom chain) and the post passes: bloom down and up, then the display transform into the output.

use bytemuck::{Pod, Zeroable};

use crate::shaders;

pub const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;
const BLOOM_MIPS: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostParams {
    texel: [f32; 4],
    exposure: [f32; 4],
}

pub struct Targets {
    pub width: u32,
    pub height: u32,
    pub color_msaa: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    pub hdr: wgpu::Texture,
    pub hdr_view: wgpu::TextureView,
    pub bloom: wgpu::Texture,
    bloom_mips: Vec<wgpu::TextureView>,
}

fn tex(
    device: &wgpu::Device,
    label: &str,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    samples: u32,
    mips: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

impl Targets {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Targets {
        let ra = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let rs = ra | wgpu::TextureUsages::TEXTURE_BINDING;
        let color_msaa = tex(device, "hdr (msaa)", width, height, HDR, SAMPLES, 1, ra)
            .create_view(&Default::default());
        // Sampled too: the splat pass copies it to one sample (splat/mod.rs).
        let depth = tex(device, "depth (msaa)", width, height, DEPTH, SAMPLES, 1, rs)
            .create_view(&Default::default());
        let hdr = tex(device, "hdr", width, height, HDR, 1, 1, rs);
        let hdr_view = hdr.create_view(&Default::default());
        let (bw, bh) = ((width / 2).max(1), (height / 2).max(1));
        let mips = BLOOM_MIPS.min(bw.min(bh).ilog2().max(1));
        let bloom = tex(device, "bloom", bw, bh, HDR, 1, mips, rs);
        let bloom_mips = (0..mips)
            .map(|m| {
                bloom.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: m,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        Targets {
            width,
            height,
            color_msaa,
            depth,
            hdr,
            hdr_view,
            bloom,
            bloom_mips,
        }
    }
}

pub struct Post {
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    output_srgb: bool,
}

impl Post {
    pub fn new(device: &wgpu::Device, output: wgpu::TextureFormat) -> Post {
        let m = shaders::module(device, "post");
        let pipe = |entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &m,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &m,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        Post {
            down: pipe("fs_down", HDR, None),
            up: pipe("fs_up", HDR, Some(add)),
            tonemap: pipe("fs_tonemap", output, None),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("post"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            }),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("post params"),
                size: 256 * 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            output_srgb: output.is_srgb(),
        }
    }

    fn pass(
        &self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        slot: u64,
        src: &wgpu::TextureView,
        bloom: Option<&wgpu::TextureView>,
        dst: &wgpu::TextureView,
        clear: bool,
        label: &str,
    ) {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.params,
                    offset: slot * 256,
                    size: wgpu::BufferSize::new(32),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(src),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ];
        if let Some(b) = bloom {
            entries.push(wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(b),
            });
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Bloom, then the display transform from the targets' HDR image into `output`.
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        t: &Targets,
        output: &wgpu::TextureView,
        exposure: f32,
        bloom: f32,
    ) {
        let mips = t.bloom_mips.len();
        let mut slots: Vec<PostParams> = Vec::new();
        // Down: hdr -> mip 0 (first, Karis), mip i-1 -> mip i.
        for i in 0..mips {
            let (sw, sh) = if i == 0 {
                (t.width, t.height)
            } else {
                ((t.width / 2) >> (i - 1), (t.height / 2) >> (i - 1))
            };
            slots.push(PostParams {
                texel: [
                    1.0 / sw.max(1) as f32,
                    1.0 / sh.max(1) as f32,
                    0.0,
                    if i == 0 { 1.0 } else { 0.0 },
                ],
                exposure: [exposure, 0.0, 0.0, 0.0],
            });
        }
        // Up: mip i+1 -> mip i, added.
        for i in (0..mips.saturating_sub(1)).rev() {
            let (sw, sh) = ((t.width / 2) >> (i + 1), (t.height / 2) >> (i + 1));
            slots.push(PostParams {
                texel: [1.0 / sw.max(1) as f32, 1.0 / sh.max(1) as f32, 0.0, 0.0],
                exposure: [exposure, 0.0, 0.0, 0.0],
            });
        }
        slots.push(PostParams {
            texel: [
                1.0 / t.width as f32,
                1.0 / t.height as f32,
                bloom,
                mips as f32,
            ],
            exposure: [exposure, if self.output_srgb { 1.0 } else { 0.0 }, 0.0, 0.0],
        });
        for (k, s) in slots.iter().enumerate() {
            queue.write_buffer(&self.params, k as u64 * 256, bytemuck::bytes_of(s));
        }
        let mut slot = 0u64;
        for i in 0..mips {
            let src = if i == 0 {
                &t.hdr_view
            } else {
                &t.bloom_mips[i - 1]
            };
            self.pass(
                device,
                enc,
                &self.down,
                slot,
                src,
                None,
                &t.bloom_mips[i],
                true,
                "bloom down",
            );
            slot += 1;
        }
        for i in (0..mips.saturating_sub(1)).rev() {
            self.pass(
                device,
                enc,
                &self.up,
                slot,
                &t.bloom_mips[i + 1],
                None,
                &t.bloom_mips[i],
                false,
                "bloom up",
            );
            slot += 1;
        }
        self.pass(
            device,
            enc,
            &self.tonemap,
            slot,
            &t.hdr_view,
            Some(&t.bloom_mips[0]),
            output,
            true,
            "display transform",
        );
    }
}
