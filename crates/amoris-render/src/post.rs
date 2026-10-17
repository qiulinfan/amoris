//! The frame's targets (the opaque pass's color, depth and extra targets, the resolved HDR image,
//! the bloom chain), the anti-aliasing and ambient-occlusion options that shape them, and the post
//! passes: bloom down and up, then the display transform (and the optional sharpening) into the
//! output.

use bytemuck::{Pod, Zeroable};

use crate::profiler::GpuProfiler;
use crate::shaders;

pub const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The share of each pixel's color that is indirect light, per channel (what GTAO darkens).
pub const SHARE: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The object motion: where the surface was last frame relative to where it would be had it not
/// moved, in normalized device coordinates (TAA; camera motion comes from the depth).
pub const MOTION: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
/// View-space normals (GTAO's optional normal source), `n * 0.5 + 0.5`.
pub const NORMALS: wgpu::TextureFormat = wgpu::TextureFormat::Rgb10a2Unorm;
const BLOOM_MIPS: u32 = 6;

/// Anti-aliasing (charter 4.4, Pioneer 2026-10-09; docs/spec/taa-gtao.md): the opaque pass's
/// samples per pixel and whether TAA runs. Every combination draws; the defaults are measured
/// ([`defaults_for`], docs/bench/taa-gtao.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Antialiasing {
    /// One sample, no TAA (aliased; the cheapest).
    Off,
    /// Four samples per pixel, resolved (the renderer's only mode before TAA).
    Msaa,
    /// One sample, jittered, accumulated over frames.
    Taa,
    /// Four samples, jittered, accumulated over frames.
    MsaaTaa,
}

/// Apple's PCI vendor id (its GPUs on Metal, and through MoltenVK on Vulkan).
const APPLE: u32 = 0x106b;

/// The measured defaults for an adapter (docs/bench/taa-gtao.md 5): a discrete GPU keeps
/// multisampling (TAA costs within 0.45 ms of it there and loses to it under camera motion) and
/// gets GTAO (0.14 to 0.19 ms at 1600x900 on the RTX 5060); an integrated GPU on Vulkan or
/// Direct3D 12 gets TAA at one sample (multisampling cost the Radeon 780M up to 4.2 ms more, and
/// never less) and no GTAO (0.5 to 0.9 ms there). Apple's GPUs (tile-based, where multisampling is
/// cheap; Apple Silicon reports itself integrated), any GPU through MoltenVK, and the browser, whose
/// adapter's kind is unknown, were not measured and keep multisampling without GTAO.
pub fn defaults_for(info: &wgpu::AdapterInfo) -> (Antialiasing, Gtao) {
    let unmeasured = info.vendor == APPLE || info.driver.to_ascii_lowercase().contains("moltenvk");
    if unmeasured {
        return (Antialiasing::Msaa, Gtao::Off);
    }
    match (info.device_type, info.backend) {
        (wgpu::DeviceType::DiscreteGpu, wgpu::Backend::Vulkan | wgpu::Backend::Dx12) => {
            (Antialiasing::Msaa, Gtao::On(AoNormals::Depth))
        }
        (wgpu::DeviceType::IntegratedGpu, wgpu::Backend::Vulkan | wgpu::Backend::Dx12) => {
            (Antialiasing::Taa, Gtao::Off)
        }
        _ => (Antialiasing::Msaa, Gtao::Off),
    }
}

impl Antialiasing {
    /// `off`, `msaa`, `taa` or `msaa+taa` (any case; `taa+msaa` too).
    pub fn parse(s: &str) -> Option<Antialiasing> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "0" => Some(Antialiasing::Off),
            "msaa" | "msaa4" => Some(Antialiasing::Msaa),
            "taa" => Some(Antialiasing::Taa),
            "msaa+taa" | "taa+msaa" => Some(Antialiasing::MsaaTaa),
            _ => None,
        }
    }

    /// From `AMORIS_AA` (natively); `default` when it is unset or unknown.
    pub fn from_env(default: Antialiasing) -> Antialiasing {
        match std::env::var("AMORIS_AA") {
            Ok(v) => Antialiasing::parse(&v).unwrap_or_else(|| {
                log::warn!("AMORIS_AA: {v:?} is not off, msaa, taa or msaa+taa; using the default");
                default
            }),
            Err(_) => default,
        }
    }

    pub fn samples(self) -> u32 {
        match self {
            Antialiasing::Msaa | Antialiasing::MsaaTaa => 4,
            Antialiasing::Off | Antialiasing::Taa => 1,
        }
    }

    pub fn taa(self) -> bool {
        matches!(self, Antialiasing::Taa | Antialiasing::MsaaTaa)
    }

    /// This mode with TAA, at the same samples: what a render scale below 1 draws with (TAA
    /// upscales; charter 4.4, Pioneer 2026-10-15).
    pub fn with_taa(self) -> Antialiasing {
        match self {
            Antialiasing::Off | Antialiasing::Taa => Antialiasing::Taa,
            Antialiasing::Msaa | Antialiasing::MsaaTaa => Antialiasing::MsaaTaa,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Antialiasing::Off => "off",
            Antialiasing::Msaa => "msaa",
            Antialiasing::Taa => "taa",
            Antialiasing::MsaaTaa => "msaa+taa",
        }
    }
}

/// The least render scale ([`render_scale_from_env`], `Renderer::set_render_scale`).
pub const MIN_RENDER_SCALE: f32 = 0.25;

/// A render scale clamped to [[`MIN_RENDER_SCALE`], 1]; 1 (off) for a value that is not a
/// finite positive number, as `AMORIS_RENDER_SCALE` takes it.
pub fn clamp_render_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(MIN_RENDER_SCALE, 1.0)
    } else {
        1.0
    }
}

/// The render scale from `AMORIS_RENDER_SCALE` (natively): the opaque pass's size over the
/// output's, which TAA upscales (charter 4.4, Pioneer 2026-10-15; docs/spec/taa-gtao.md 4.1).
/// 1 when it is unset or not a finite positive number; clamped to [[`MIN_RENDER_SCALE`], 1]
/// ([`clamp_render_scale`], as `Renderer::set_render_scale` takes it).
pub fn render_scale_from_env() -> f32 {
    match std::env::var("AMORIS_RENDER_SCALE") {
        Ok(v) => match v.trim().parse::<f32>() {
            Ok(s) if s.is_finite() && s > 0.0 => {
                let c = clamp_render_scale(s);
                if c != s {
                    log::warn!("AMORIS_RENDER_SCALE: {s} is outside [0.25, 1]; using {c}");
                }
                c
            }
            _ => {
                log::warn!("AMORIS_RENDER_SCALE: {v:?} is not a number in (0, 1]; using 1");
                1.0
            }
        },
        Err(_) => 1.0,
    }
}

/// Where GTAO takes its view-space normals from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AoNormals {
    /// Reconstructed from the half-resolution depth (no extra target in the opaque pass).
    Depth,
    /// A normal target the forward shader writes (normal maps included).
    Target,
}

/// Ground-truth ambient occlusion on indirect light (docs/spec/taa-gtao.md): off, or on with a
/// normal source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gtao {
    Off,
    On(AoNormals),
}

impl Gtao {
    /// `off`, `on` or `depth` (normals from the depth), `target` (the normal target); any case,
    /// `0`/`1` too.
    pub fn parse(s: &str) -> Option<Gtao> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Some(Gtao::Off),
            "on" | "1" | "true" | "yes" | "depth" => Some(Gtao::On(AoNormals::Depth)),
            "target" | "normals" => Some(Gtao::On(AoNormals::Target)),
            _ => None,
        }
    }

    /// From `AMORIS_GTAO` (natively); `default` when it is unset or unknown.
    pub fn from_env(default: Gtao) -> Gtao {
        match std::env::var("AMORIS_GTAO") {
            Ok(v) => Gtao::parse(&v).unwrap_or_else(|| {
                log::warn!("AMORIS_GTAO: {v:?} is not off, on, depth or target; using the default");
                default
            }),
            Err(_) => default,
        }
    }

    pub fn on(self) -> bool {
        self != Gtao::Off
    }

    pub fn name(self) -> &'static str {
        match self {
            Gtao::Off => "off",
            Gtao::On(AoNormals::Depth) => "depth",
            Gtao::On(AoNormals::Target) => "target",
        }
    }
}

/// What the opaque pass draws into; every pipeline drawn in it is built for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneFormat {
    pub samples: u32,
    /// The indirect share target (GTAO).
    pub share: bool,
    /// The object motion target (TAA).
    pub motion: bool,
    /// The view-space normal target (GTAO with [`AoNormals::Target`]).
    pub normals: bool,
}

impl SceneFormat {
    pub fn new(aa: Antialiasing, gtao: Gtao) -> SceneFormat {
        SceneFormat {
            samples: aa.samples(),
            share: gtao.on(),
            motion: aa.taa(),
            normals: gtao == Gtao::On(AoNormals::Target),
        }
    }

    /// The color targets of a pipeline drawn in the opaque pass: the HDR color with `blend`, then
    /// the extra targets that exist, which `extras` says the fragment shader writes (the forward
    /// shader's `SceneOut`); without, they are left as they are (write mask empty).
    pub fn targets(
        &self,
        blend: Option<wgpu::BlendState>,
        extras: bool,
    ) -> Vec<Option<wgpu::ColorTargetState>> {
        let extra = |on: bool, format| {
            on.then_some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: if extras {
                    wgpu::ColorWrites::ALL
                } else {
                    wgpu::ColorWrites::empty()
                },
            })
        };
        let mut t = vec![
            Some(wgpu::ColorTargetState {
                format: HDR,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            }),
            extra(self.share, SHARE),
            extra(self.motion, MOTION),
            extra(self.normals, NORMALS),
        ];
        while t.last().is_some_and(Option::is_none) {
            t.pop();
        }
        t
    }

    /// The forward shader's pipeline-overridable constants for a pipeline that writes
    /// [`targets`](Self::targets) with extras: whether to compute the object motion and the view
    /// normal (forward.wgsl `scene_out`; the indirect share is computed always, for images equal
    /// to those before on NVIDIA's Direct3D 12).
    pub fn constants(&self) -> [(&'static str, f64); 2] {
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        [
            ("SCENE_MOTION", on(self.motion)),
            ("SCENE_NORMALS", on(self.normals)),
        ]
    }

    pub fn multisample(&self) -> wgpu::MultisampleState {
        wgpu::MultisampleState {
            count: self.samples,
            ..Default::default()
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostParams {
    texel: [f32; 4],
    exposure: [f32; 4],
}

/// A target of the opaque pass: drawn multisampled and resolved into `view`, or drawn into `view`.
pub struct SceneTarget {
    pub msaa: Option<wgpu::TextureView>,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

pub struct Targets {
    /// The output's size: the image the rest of the frame reads (`hdr`), bloom, splats.
    pub width: u32,
    pub height: u32,
    /// The opaque pass's size (its color, depth and extra targets; GTAO): the output's, or below
    /// it with a render scale under 1, which TAA upscales from (charter 4.4, Pioneer 2026-10-15;
    /// docs/spec/taa-gtao.md 4.1).
    pub scene: (u32, u32),
    pub format: SceneFormat,
    /// The opaque pass's color: `hdr`, or with TAA `raw`.
    pub color: SceneTarget,
    /// The opaque pass's depth (reversed-Z), `format.samples` samples.
    pub depth: wgpu::TextureView,
    /// The image the rest of the frame reads (splats, bloom, the display transform): the opaque
    /// pass's resolved color, or with TAA its output.
    pub hdr: wgpu::Texture,
    pub hdr_view: wgpu::TextureView,
    /// With TAA, the opaque pass's own (jittered) image: TAA's input.
    pub raw: Option<wgpu::TextureView>,
    pub share: Option<SceneTarget>,
    pub motion: Option<SceneTarget>,
    pub normals: Option<SceneTarget>,
    pub bloom: wgpu::Texture,
    bloom_mips: Vec<wgpu::TextureView>,
}

// A texture descriptor's fields, as the crate's other GPU helpers take them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tex(
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

/// The opaque pass's size for an output of `width` x `height` at render scale `scale` (1: the
/// output's): each side scaled and rounded, at least one pixel.
pub fn scene_size(width: u32, height: u32, scale: f32) -> (u32, u32) {
    if scale >= 1.0 {
        return (width, height);
    }
    let side = |n: u32| ((n as f32 * scale).round() as u32).clamp(1, n.max(1));
    (side(width), side(height))
}

impl Targets {
    /// Targets for an output of `width` x `height` whose opaque pass draws at `scene` (at most the
    /// output; below it only with TAA, `format.motion`, which upscales).
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        scene: (u32, u32),
        format: SceneFormat,
    ) -> Targets {
        debug_assert!(scene == (width, height) || format.motion);
        let ra = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let rs = ra | wgpu::TextureUsages::TEXTURE_BINDING;
        // The HDR image is copied out for evaluation (`Renderer::capture_hdr`).
        let rsc = rs | wgpu::TextureUsages::COPY_SRC;
        let samples = format.samples;
        let (sw, sh) = scene;
        let scene_target = |label: &str, f: wgpu::TextureFormat| {
            let msaa = (samples > 1).then(|| {
                tex(device, label, sw, sh, f, samples, 1, ra).create_view(&Default::default())
            });
            let texture = tex(device, label, sw, sh, f, 1, 1, rsc);
            SceneTarget {
                msaa,
                view: texture.create_view(&Default::default()),
                texture,
            }
        };
        // Sampled too: the splat pass copies it to one sample (splat/mod.rs); GTAO and TAA read it.
        let depth =
            tex(device, "depth", sw, sh, DEPTH, samples, 1, rs).create_view(&Default::default());
        let color = scene_target("hdr (scene)", HDR);
        // With TAA the opaque pass resolves into `raw` and TAA writes `hdr`.
        let (hdr, raw) = if format.motion {
            (
                tex(
                    device,
                    "hdr",
                    width,
                    height,
                    HDR,
                    1,
                    1,
                    rsc | wgpu::TextureUsages::STORAGE_BINDING,
                ),
                Some(color.view.clone()),
            )
        } else {
            (color.texture.clone(), None)
        };
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
            scene,
            format,
            color,
            depth,
            hdr,
            hdr_view,
            raw,
            share: format.share.then(|| scene_target("indirect share", SHARE)),
            motion: format.motion.then(|| scene_target("motion", MOTION)),
            normals: format.normals.then(|| scene_target("normals", NORMALS)),
            bloom,
            bloom_mips,
        }
    }

    /// The opaque pass's color attachments: `first` clears them (else they are loaded), `last`
    /// resolves the multisampled ones (and discards their samples).
    pub fn attachments(
        &self,
        first: bool,
        last: bool,
    ) -> Vec<Option<wgpu::RenderPassColorAttachment<'_>>> {
        let u = Use {
            clear: first,
            resolve: last,
            keep: !last,
        };
        self.with(u, u)
    }

    /// The last opaque pass when the transparent stage draws after GTAO (renderer.rs): `first`
    /// clears them (else they are loaded); the extra targets resolve (GTAO reads the share and the
    /// normals, TAA the motion) and discard their samples, the color keeps its samples, unresolved,
    /// for [`transparent_attachments`](Self::transparent_attachments).
    pub fn attachments_before_transparent(
        &self,
        first: bool,
    ) -> Vec<Option<wgpu::RenderPassColorAttachment<'_>>> {
        let color = Use {
            clear: first,
            resolve: false,
            keep: true,
        };
        let extras = Use {
            clear: first,
            resolve: true,
            keep: false,
        };
        self.with(color, extras)
    }

    /// The pass after GTAO that draws the transparent stage, the particles and the editor's grid
    /// into the opaque pass's color: loaded, then resolved. Their pipelines leave the extra targets
    /// as they are: a single-sampled one is kept, a multisampled one's samples (discarded by the
    /// opaque pass) are cleared and dropped, its resolved view untouched.
    pub fn transparent_attachments(&self) -> Vec<Option<wgpu::RenderPassColorAttachment<'_>>> {
        let color = Use {
            clear: false,
            resolve: true,
            keep: false,
        };
        fn extra(t: &SceneTarget) -> Option<wgpu::RenderPassColorAttachment<'_>> {
            let samples = t.msaa.is_some();
            let u = Use {
                clear: samples,
                resolve: false,
                keep: false,
            };
            attachment(t, u)
        }
        let mut v = vec![
            attachment(&self.color, color),
            self.share.as_ref().and_then(extra),
            self.motion.as_ref().and_then(extra),
            self.normals.as_ref().and_then(extra),
        ];
        while v.last().is_some_and(Option::is_none) {
            v.pop();
        }
        v
    }

    /// The color attachment treated as `color` says, then the extra targets that exist as
    /// `extras` says.
    fn with(&self, color: Use, extras: Use) -> Vec<Option<wgpu::RenderPassColorAttachment<'_>>> {
        let mut v = vec![
            attachment(&self.color, color),
            self.share.as_ref().and_then(|t| attachment(t, extras)),
            self.motion.as_ref().and_then(|t| attachment(t, extras)),
            self.normals.as_ref().and_then(|t| attachment(t, extras)),
        ];
        while v.last().is_some_and(Option::is_none) {
            v.pop();
        }
        v
    }
}

/// How a pass uses one of the opaque pass's targets: cleared (else loaded); multisampled, whether
/// it resolves into the target's view and keeps its samples. A single-sampled target is stored.
#[derive(Clone, Copy)]
struct Use {
    clear: bool,
    resolve: bool,
    keep: bool,
}

fn attachment(t: &SceneTarget, u: Use) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    let load = if u.clear {
        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
    } else {
        wgpu::LoadOp::Load
    };
    Some(match &t.msaa {
        Some(m) => wgpu::RenderPassColorAttachment {
            view: m,
            depth_slice: None,
            resolve_target: u.resolve.then_some(&t.view),
            ops: wgpu::Operations {
                load,
                store: if u.keep {
                    wgpu::StoreOp::Store
                } else {
                    wgpu::StoreOp::Discard
                },
            },
        },
        None => wgpu::RenderPassColorAttachment {
            view: &t.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        },
    })
}

pub struct Post {
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    output_srgb: bool,
    /// The parameters as last written (a 256-byte slot per pass), written again only when they
    /// change: each `write_buffer` stages through a buffer of its own, which costs more on
    /// Direct3D 12 (docs/bench/dx12.md 10).
    written: Vec<u8>,
    /// The passes' bind groups and the source image and bloom chain they hold.
    groups: Option<(wgpu::TextureView, wgpu::Texture, Vec<wgpu::BindGroup>)>,
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
            written: Vec::new(),
            groups: None,
        }
    }

    /// The bind group of the pass in parameter slot `slot`: `pipeline` reading `src` (and `bloom`).
    fn group(
        &self,
        device: &wgpu::Device,
        pipeline: &wgpu::RenderPipeline,
        slot: u64,
        src: &wgpu::TextureView,
        bloom: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
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
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        })
    }

    fn pass(
        enc: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        bg: &wgpu::BindGroup,
        dst: &wgpu::TextureView,
        clear: bool,
        label: &str,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
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
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Bloom, then the display transform from `src` (the frame's HDR image, the size of `t`) into
    /// `output`; `sharpen` (0: off) sharpens the image before the display transform
    /// (docs/spec/taa-gtao.md). While a frame trace records (trace.rs) the bloom chain and the
    /// display transform are timed as one pass, "post"; otherwise they are not, so the frame's GPU
    /// time stays the scene passes' that the benchmarks recorded.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        t: &Targets,
        src: &wgpu::TextureView,
        output: &wgpu::TextureView,
        exposure: f32,
        bloom: f32,
        sharpen: f32,
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
            exposure: [
                exposure,
                if self.output_srgb { 1.0 } else { 0.0 },
                sharpen,
                0.0,
            ],
        });
        let mut bytes = vec![0u8; slots.len() * 256];
        for (k, s) in slots.iter().enumerate() {
            bytes[k * 256..k * 256 + 32].copy_from_slice(bytemuck::bytes_of(s));
        }
        if bytes != self.written {
            queue.write_buffer(&self.params, 0, &bytes);
            self.written = bytes;
        }
        // Slots: the downsamples, the upsamples, then the display transform.
        let fresh = self
            .groups
            .as_ref()
            .is_none_or(|(view, bloom, _)| view != src || *bloom != t.bloom);
        if fresh {
            let mut groups = Vec::with_capacity(2 * mips);
            for i in 0..mips {
                let from = if i == 0 { src } else { &t.bloom_mips[i - 1] };
                groups.push(self.group(device, &self.down, groups.len() as u64, from, None));
            }
            for i in (0..mips.saturating_sub(1)).rev() {
                let from = &t.bloom_mips[i + 1];
                groups.push(self.group(device, &self.up, groups.len() as u64, from, None));
            }
            let last = groups.len() as u64;
            groups.push(self.group(device, &self.tonemap, last, src, Some(&t.bloom_mips[0])));
            self.groups = Some((src.clone(), t.bloom.clone(), groups));
        }
        let Some((_, _, groups)) = &self.groups else {
            return;
        };
        let span = if profiler.tracing() {
            profiler.span("post")
        } else {
            None
        };
        // The slots are the passes in order; the last is the display transform.
        let last = groups.len() - 1;
        let ts = |slot: usize| profiler.span_writes(span, slot == 0, slot == last);
        let mut slot = 0;
        for i in 0..mips {
            Self::pass(
                enc,
                &self.down,
                &groups[slot],
                &t.bloom_mips[i],
                true,
                "bloom down",
                ts(slot),
            );
            slot += 1;
        }
        for i in (0..mips.saturating_sub(1)).rev() {
            Self::pass(
                enc,
                &self.up,
                &groups[slot],
                &t.bloom_mips[i],
                false,
                "bloom up",
                ts(slot),
            );
            slot += 1;
        }
        Self::pass(
            enc,
            &self.tonemap,
            &groups[slot],
            output,
            true,
            "display transform",
            ts(slot),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One rule for every way in (`AMORIS_RENDER_SCALE`, `Renderer::set_render_scale`, the
    /// viewport's `?scale=`): in range kept, beyond it clamped, anything else off.
    #[test]
    fn render_scales_clamp_or_turn_off() {
        assert_eq!(clamp_render_scale(0.5), 0.5);
        assert_eq!(clamp_render_scale(1.0), 1.0);
        assert_eq!(clamp_render_scale(0.01), MIN_RENDER_SCALE);
        assert_eq!(clamp_render_scale(2.0), 1.0);
        for off in [0.0, -0.0, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(clamp_render_scale(off), 1.0, "{off}");
        }
    }

    #[test]
    fn defaults_by_adapter() {
        use wgpu::{Backend, DeviceType};
        let info = |kind, backend, vendor: u32, driver: &str| {
            let mut i = wgpu::AdapterInfo::new(kind, backend);
            i.vendor = vendor;
            i.driver = driver.into();
            i
        };
        let msaa = (Antialiasing::Msaa, Gtao::Off);
        let discrete = (Antialiasing::Msaa, Gtao::On(AoNormals::Depth));
        let integrated = (Antialiasing::Taa, Gtao::Off);
        for (kind, backend, vendor, driver, want) in [
            // Measured: the RTX 5060 and the Radeon 780M on Vulkan and Direct3D 12.
            (
                DeviceType::DiscreteGpu,
                Backend::Vulkan,
                0x10de,
                "NVIDIA",
                discrete,
            ),
            (DeviceType::DiscreteGpu, Backend::Dx12, 0x10de, "", discrete),
            (
                DeviceType::IntegratedGpu,
                Backend::Vulkan,
                0x1002,
                "AMD proprietary driver",
                integrated,
            ),
            (
                DeviceType::IntegratedGpu,
                Backend::Dx12,
                0x1002,
                "",
                integrated,
            ),
            // Not measured: Apple's GPUs on Metal or MoltenVK, any GPU through MoltenVK, the
            // browser, software adapters.
            (DeviceType::IntegratedGpu, Backend::Metal, APPLE, "", msaa),
            (
                DeviceType::IntegratedGpu,
                Backend::Vulkan,
                APPLE,
                "MoltenVK",
                msaa,
            ),
            (DeviceType::IntegratedGpu, Backend::Vulkan, APPLE, "", msaa),
            (
                DeviceType::DiscreteGpu,
                Backend::Vulkan,
                0x1002,
                "MoltenVK",
                msaa,
            ),
            (DeviceType::DiscreteGpu, Backend::BrowserWebGpu, 0, "", msaa),
            (DeviceType::Other, Backend::BrowserWebGpu, 0, "", msaa),
            (DeviceType::Cpu, Backend::Vulkan, 0x10005, "llvmpipe", msaa),
            (DeviceType::Cpu, Backend::Dx12, 0x1414, "", msaa),
        ] {
            assert_eq!(
                defaults_for(&info(kind, backend, vendor, driver)),
                want,
                "{kind:?} {backend:?} vendor {vendor:#x} driver {driver:?}"
            );
        }
    }
}
