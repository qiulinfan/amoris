//! Ground-truth ambient occlusion on indirect light (charter 4.4, Pioneer 2026-10-09;
//! docs/spec/taa-gtao.md): after the opaque pass, at half resolution, four fragment passes
//! (gtao.wgsl): the view distance from the depth, the horizon search, a depth-aware blur, and a
//! depth-aware upsample that multiplies each pixel's color by `1 - share * (1 - visibility)`,
//! where `share` is the part of the color the forward shader computed from indirect light (sky
//! light, spherical harmonics, baked GI; post.rs `SHARE`). Direct light and emission are never
//! darkened, and no depth prepass is needed.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

use crate::post::{AoNormals, HDR, Targets, tex};
use crate::profiler::GpuProfiler;
use crate::shaders;

/// The occlusion radius in metres (what counts as "nearby" geometry).
pub const RADIUS: f32 = 0.6;
/// The share of the radius over which an occluder's weight falls to zero.
const FALLOFF: f32 = 0.6;
/// The visibility is raised to this power (XeGTAO's default 2.2 darkens creases as references do).
const POWER: f32 = 1.6;
/// Slices (directions) per pixel and steps per side.
const SLICES: u32 = 2;
const STEPS: u32 = 4;

fn view(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(v),
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    inv_proj: [[f32; 4]; 4],
    proj: [f32; 4],
    ortho: [f32; 4],
    full: [f32; 4],
    half: [f32; 4],
    settings: [f32; 4],
    flags: [u32; 4],
}

struct Images {
    size: (u32, u32),
    depth: wgpu::TextureView,
    raw: wgpu::TextureView,
    blurred: wgpu::TextureView,
    groups: [wgpu::BindGroup; 4],
    /// `apply_samples`'s group (its layout is its own).
    apply_samples: Option<wgpu::BindGroup>,
}

pub struct Gtao {
    depth: wgpu::RenderPipeline,
    main: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    apply: wgpu::RenderPipeline,
    /// With a multisampled opaque pass, `apply` drawn into its samples (`run`'s `into_samples`).
    apply_samples: Option<wgpu::RenderPipeline>,
    params: wgpu::Buffer,
    /// A 1x1 stand-in for the normal target when normals come from the depth.
    dummy: wgpu::TextureView,
    images: Option<Images>,
    /// The depth target the images' groups hold (they are rebuilt when it changes).
    holds: Option<wgpu::TextureView>,
    pub normals: AoNormals,
    pub radius: f32,
}

impl Gtao {
    /// For a depth target of `samples` samples.
    pub fn new(device: &wgpu::Device, samples: u32, normals: AoNormals) -> Gtao {
        let m = shaders::depth_module(device, "gtao", samples);
        let pipe = |entry: &str,
                    format: wgpu::TextureFormat,
                    blend: Option<wgpu::BlendState>,
                    count: u32| {
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
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        // color * factor: the factor is the source, the color the destination.
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let dummy = tex(
            device,
            "gtao (no normals)",
            1,
            1,
            crate::post::NORMALS,
            1,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&Default::default());
        Gtao {
            depth: pipe("fs_depth", wgpu::TextureFormat::R32Float, None, 1),
            main: pipe("fs_main", wgpu::TextureFormat::R8Unorm, None, 1),
            blur: pipe("fs_blur", wgpu::TextureFormat::R8Unorm, None, 1),
            apply: pipe("fs_apply", HDR, Some(multiply), 1),
            // The same factor for every sample of a pixel: resolving gives the factor times the
            // resolved color, as `apply` after the resolve does, to rounding.
            apply_samples: (samples > 1).then(|| pipe("fs_apply", HDR, Some(multiply), samples)),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gtao"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            dummy,
            images: None,
            holds: None,
            normals,
            radius: RADIUS,
        }
    }

    /// Darkens the indirect light of the opaque pass's resolved color (`t.raw` with TAA, else
    /// `t.hdr`), or with `into_samples` of its multisampled color, whose samples a later pass
    /// resolves (the transparent stage drawn after GTAO, renderer.rs). `proj` is this frame's
    /// projection (with its jitter); `frame` varies the noise when `temporal` (TAA integrates it).
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        t: &Targets,
        proj: Mat4,
        ortho: bool,
        frame: u32,
        temporal: bool,
        into_samples: bool,
    ) {
        let Some(share) = &t.share else {
            return;
        };
        // The opaque pass's size (below the output's at a render scale under 1).
        let size = t.scene;
        let half = (size.0.div_ceil(2), size.1.div_ceil(2));
        if self.images.as_ref().is_none_or(|i| i.size != size)
            || self.holds.as_ref() != Some(&t.depth)
        {
            let ra = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            let img = |label: &str, format| {
                tex(device, label, half.0, half.1, format, 1, 1, ra)
                    .create_view(&Default::default())
            };
            let depth = img("gtao depth", wgpu::TextureFormat::R32Float);
            let raw = img("gtao", wgpu::TextureFormat::R8Unorm);
            let blurred = img("gtao (blurred)", wgpu::TextureFormat::R8Unorm);
            let normals = t.normals.as_ref().map_or(&self.dummy, |n| &n.view);
            let buf = wgpu::BindGroupEntry {
                binding: 0,
                resource: self.params.as_entire_binding(),
            };
            let group = |p: &wgpu::RenderPipeline, entries: &[wgpu::BindGroupEntry<'_>]| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("gtao"),
                    layout: &p.get_bind_group_layout(0),
                    entries,
                })
            };
            let apply_entries = [
                buf.clone(),
                view(1, &t.depth),
                view(2, &depth),
                view(3, &blurred),
                view(5, &share.view),
            ];
            let apply_samples = self
                .apply_samples
                .as_ref()
                .map(|p| group(p, &apply_entries));
            let groups = [
                group(&self.depth, &[buf.clone(), view(1, &t.depth)]),
                group(
                    &self.main,
                    &[buf.clone(), view(2, &depth), view(4, normals)],
                ),
                group(&self.blur, &[buf.clone(), view(2, &depth), view(3, &raw)]),
                group(&self.apply, &apply_entries),
            ];
            self.images = Some(Images {
                size,
                depth,
                raw,
                blurred,
                groups,
                apply_samples,
            });
            self.holds = Some(t.depth.clone());
        }
        let Some(img) = &self.images else {
            return;
        };
        let c = |m: Mat4, col: usize, row: usize| m.col(col)[row];
        let params = Params {
            inv_proj: proj.inverse().to_cols_array_2d(),
            proj: [c(proj, 0, 0), c(proj, 1, 1), c(proj, 2, 0), c(proj, 2, 1)],
            ortho: [
                if ortho { 1.0 } else { 0.0 },
                c(proj, 3, 0),
                c(proj, 3, 1),
                0.0,
            ],
            full: [
                size.0 as f32,
                size.1 as f32,
                1.0 / size.0 as f32,
                1.0 / size.1 as f32,
            ],
            half: [
                half.0 as f32,
                half.1 as f32,
                1.0 / half.0 as f32,
                1.0 / half.1 as f32,
            ],
            settings: [self.radius, FALLOFF, POWER, (frame % 64) as f32],
            flags: [
                u32::from(self.normals == AoNormals::Target && t.normals.is_some()),
                u32::from(temporal),
                SLICES,
                STEPS,
            ],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        // The color to darken: the resolved one, or (`into_samples`) the multisampled one.
        let samples = match (&t.color.msaa, &self.apply_samples, &img.apply_samples) {
            (Some(target), Some(p), Some(g)) if into_samples => Some((p, target, g)),
            _ => None,
        };
        let (apply, color, apply_group) = samples.unwrap_or((
            &self.apply,
            t.raw.as_ref().unwrap_or(&t.hdr_view),
            &img.groups[3],
        ));
        let passes: [(
            &wgpu::RenderPipeline,
            &wgpu::TextureView,
            &wgpu::BindGroup,
            &str,
        ); 4] = [
            (&self.depth, &img.depth, &img.groups[0], "gtao depth"),
            (&self.main, &img.raw, &img.groups[1], "gtao"),
            (&self.blur, &img.blurred, &img.groups[2], "gtao blur"),
            (apply, color, apply_group, "gtao apply"),
        ];
        for (k, (pipeline, target, group, label)) in passes.into_iter().enumerate() {
            let ts = profiler.render_scope("gtao");
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // The half-resolution images are written whole; the color is multiplied.
                        load: if k == 3 {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color::WHITE)
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                timestamp_writes: ts,
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
