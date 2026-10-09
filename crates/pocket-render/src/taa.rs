//! Temporal anti-aliasing (charter 4.4, Pioneer 2026-10-09; docs/spec/taa-gtao.md): the
//! projection's sub-pixel jitter, the per-frame state motion vectors need (last frame's matrices
//! and pose alpha), when the history is reset, and the pass (taa.wgsl) that blends the opaque
//! pass's jittered image with the reprojected history into the image the rest of the frame reads.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use crate::camera::CameraState;
use crate::post::{HDR, Targets, tex};
use crate::profiler::GpuProfiler;
use crate::shaders;

/// The jitter sequence's length: Halton (2, 3) points 1 to 16.
pub const JITTER_PHASES: u32 = 16;
/// The sequence's length for [`TaaMode::Jittered`] (references average many more positions).
pub const REFERENCE_PHASES: u32 = 1024;
/// The least weight of the current frame (the history's exponential window: about 2 / ALPHA - 1
/// frames, at least one jitter cycle).
pub const ALPHA: f32 = 0.1;
/// The clip box's half size in standard deviations of the 3x3 neighbourhood.
pub const GAMMA: f32 = 1.25;
/// A camera that moves farther than this in a frame (metres), turns more (radians) or changes its
/// lens has cut: the history shows something else.
const CUT_DISTANCE: f32 = 25.0;
const CUT_ANGLE: f32 = 0.6;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    inv_view_proj: [[f32; 4]; 4],
    prev_view_proj: [[f32; 4]; 4],
    view_z: [f32; 4],
    prev_view_z: [f32; 4],
    size: [f32; 4],
    params: [f32; 4],
    flags: [f32; 4],
    blend: [f32; 4],
}

/// What TAA does this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaaMode {
    /// Reproject, clip and blend.
    Temporal,
    /// No history: each frame is the jittered frame itself, the jitter running through
    /// [`REFERENCE_PHASES`] positions. Averaged by the caller, a supersampled reference of a still
    /// scene (evaluation; docs/bench/taa-gtao.md).
    Jittered,
}

/// Switches for evaluation (each on by default): object motion (skinned vertices' own included),
/// disocclusion by depth, the current frame's extra weight when the history moved far; the least
/// current weight, the clip box, and the jitter's amplitude in pixels (1: the whole pixel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TaaTuning {
    pub object_motion: bool,
    pub skinned_motion: bool,
    pub disocclusion: bool,
    pub speed_weight: bool,
    pub alpha: f32,
    pub gamma: f32,
    /// The clip box in sigmas where nothing moved (0: `gamma` there too).
    pub still_gamma: f32,
    /// The current frame's weight while the history is inside the clip box (0: `alpha` there too).
    pub alpha_inside: f32,
    pub jitter: f32,
}

impl Default for TaaTuning {
    fn default() -> Self {
        TaaTuning {
            object_motion: true,
            skinned_motion: true,
            disocclusion: true,
            speed_weight: true,
            alpha: ALPHA,
            gamma: GAMMA,
            still_gamma: 0.0,
            alpha_inside: 0.0,
            jitter: 1.0,
        }
    }
}

/// Last frame, as the motion vectors need it.
#[derive(Clone, Copy, Debug)]
struct Prev {
    view_proj: Mat4,
    view: Mat4,
    camera: CameraState,
    /// The simulated time the frame's interpolated poses show.
    drawn_t: f64,
    size: (u32, u32),
}

/// This frame's jitter and what motion vectors need from last frame (`Taa::begin_frame`).
#[derive(Clone, Copy, Debug)]
pub struct FrameJitter {
    /// The jitter in normalized device coordinates (zero without TAA).
    pub ndc: Vec2,
    /// Last frame's unjittered view-projection and view (this frame's when there is no history).
    pub prev_view_proj: Mat4,
    pub prev_view: Mat4,
    /// The interpolation alpha whose poses last frame drew, relative to this frame's two ticks
    /// (`alpha - frames' time apart / dt`; below 0 when a tick passed in between).
    pub prev_alpha: f32,
    /// Whether the history was dropped this frame.
    pub reset: bool,
}

impl FrameJitter {
    /// `proj` moved by the jitter.
    pub fn apply(&self, proj: Mat4) -> Mat4 {
        Mat4::from_translation(Vec3::new(self.ndc.x, self.ndc.y, 0.0)) * proj
    }
}

/// The `i`th point of the Halton sequence of `base`.
fn halton(mut i: u32, base: u32) -> f32 {
    let mut f = 1.0;
    let mut r = 0.0;
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

/// The jitter of frame `k` in pixels, in [-0.5, 0.5): Halton (2, 3) point `k % phases + 1`.
pub fn jitter_pixels(k: u32, phases: u32) -> Vec2 {
    let i = k % phases + 1;
    Vec2::new(halton(i, 2) - 0.5, halton(i, 3) - 0.5)
}

fn v(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

struct History {
    size: (u32, u32),
    textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// Bind groups reading history `i` and writing the other, without and with the copy, for the
    /// targets they were made with.
    groups: [[wgpu::BindGroup; 2]; 2],
    holds: wgpu::TextureView,
}

pub struct Taa {
    /// Writes the history, which the rest of the frame reads.
    pipeline: wgpu::ComputePipeline,
    /// Also writes a copy into the targets' HDR image (splats are drawn over it, never into the
    /// history).
    copy: wgpu::ComputePipeline,
    layouts: [wgpu::BindGroupLayout; 2],
    params: wgpu::Buffer,
    sampler: wgpu::Sampler,
    history: Option<History>,
    /// Which history holds the last output.
    current: usize,
    /// Frames since the jitter sequence started (its phase).
    frame: u32,
    /// Frames accumulated since the last reset.
    frames: u32,
    prev: Option<Prev>,
    reset_requested: bool,
    pub mode: TaaMode,
    pub tuning: TaaTuning,
}

impl Taa {
    /// For a depth target of `samples` samples.
    pub fn new(device: &wgpu::Device, samples: u32) -> Taa {
        let m = shaders::depth_module(device, "taa", samples);
        let stage = wgpu::ShaderStages::COMPUTE;
        let texture = |binding, multisampled, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: stage,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: stage,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: HDR,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: stage,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            texture(1, false, float),
            texture(2, false, float),
            texture(3, false, float),
            texture(4, samples > 1, wgpu::TextureSampleType::Depth),
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: stage,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            storage(6),
        ];
        let history_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taa"),
            entries: &entries,
        });
        entries.push(storage(7));
        let copy_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taa (copy)"),
            entries: &entries,
        });
        let pipe = |entry: &str, layout: &wgpu::BindGroupLayout| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &m,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        Taa {
            pipeline: pipe("resolve_history", &history_layout),
            copy: pipe("resolve_copy", &copy_layout),
            layouts: [history_layout, copy_layout],
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("taa"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("taa history"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            }),
            history: None,
            current: 0,
            frame: 0,
            frames: 0,
            prev: None,
            reset_requested: true,
            mode: TaaMode::Temporal,
            tuning: TaaTuning::default(),
        }
    }

    /// Drops the history at the next frame (a camera cut the heuristics would not see).
    pub fn reset(&mut self) {
        self.reset_requested = true;
    }

    /// Drops the history and restarts the jitter sequence at the next frame, so what follows does
    /// not depend on what was drawn before (`Renderer::capture_still`).
    pub fn restart(&mut self) {
        self.reset_requested = true;
        self.frame = 0;
    }

    /// Frames accumulated since the history was last reset.
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// The history the last `run` wrote (the image the frame went on with, unless it also copied
    /// it for splats).
    pub fn output(&self) -> Option<(&wgpu::Texture, &wgpu::TextureView)> {
        self.history
            .as_ref()
            .map(|h| (&h.textures[self.current], &h.views[self.current]))
    }

    /// Starts a frame drawn from `camera` (view `view`, unjittered view-projection `view_proj`) at
    /// `size`, showing the poses of simulated time `drawn_t` at interpolation `alpha` between ticks
    /// `dt_s` apart. `active` is whether TAA runs (without, no jitter, but motion state is still
    /// kept so turning it on starts cleanly).
    #[allow(clippy::too_many_arguments)]
    pub fn begin_frame(
        &mut self,
        active: bool,
        camera: &CameraState,
        view: Mat4,
        view_proj: Mat4,
        size: (u32, u32),
        drawn_t: f64,
        alpha: f32,
        dt_s: f64,
    ) -> FrameJitter {
        let cut = match &self.prev {
            None => true,
            Some(p) => {
                let moved = p.camera.position.distance(camera.position);
                let turned = p.camera.rotation.angle_between(camera.rotation);
                p.size != size
                    || moved > CUT_DISTANCE
                    || turned > CUT_ANGLE
                    || (p.camera.fov_y - camera.fov_y).abs() > 1e-4
                    || p.camera.ortho_height != camera.ortho_height
                    || drawn_t < p.drawn_t
                    || drawn_t - p.drawn_t > 1.0
            }
        };
        let reset = cut || self.reset_requested || !active || self.mode == TaaMode::Jittered;
        let (prev_view_proj, prev_view, prev_alpha) = match (&self.prev, cut) {
            (Some(p), false) => (
                p.view_proj,
                p.view,
                alpha - ((drawn_t - p.drawn_t) / dt_s.max(1e-6)) as f32,
            ),
            _ => (view_proj, view, alpha),
        };
        self.prev = Some(Prev {
            view_proj,
            view,
            camera: *camera,
            drawn_t,
            size,
        });
        if reset {
            self.frames = 0;
            self.reset_requested = false;
        }
        let ndc = if active {
            self.frame = self.frame.wrapping_add(1);
            let phases = match self.mode {
                TaaMode::Temporal => JITTER_PHASES,
                TaaMode::Jittered => REFERENCE_PHASES,
            };
            let j = jitter_pixels(self.frame, phases) * self.tuning.jitter;
            Vec2::new(2.0 * j.x / size.0 as f32, -2.0 * j.y / size.1 as f32)
        } else {
            Vec2::ZERO
        };
        FrameJitter {
            ndc,
            prev_view_proj,
            prev_view,
            prev_alpha,
            reset,
        }
    }

    /// Blends this frame (`t.raw`) with the history into the next history ([`Taa::output`]) and,
    /// with `copy` (splats follow), into `t.hdr` too. `jitter` is this frame's (`begin_frame`),
    /// `view` and `view_proj` its unjittered matrices.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        t: &Targets,
        jitter: &FrameJitter,
        view: Mat4,
        view_proj: Mat4,
        copy: bool,
    ) {
        let (Some(raw), Some(motion)) = (&t.raw, &t.motion) else {
            return;
        };
        let size = (t.width, t.height);
        if self
            .history
            .as_ref()
            .is_none_or(|h| h.size != size || h.holds != t.depth)
        {
            // Copied out for evaluation (`Renderer::capture_hdr`).
            let usage = wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC;
            let textures =
                [0, 1].map(|_| tex(device, "taa history", size.0, size.1, HDR, 1, 1, usage));
            let views = [0, 1].map(|i| textures[i].create_view(&Default::default()));
            let groups = [0, 1].map(|copy: usize| {
                [0, 1].map(|i: usize| {
                    let mut entries = vec![
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.params.as_entire_binding(),
                        },
                        v(1, raw),
                        v(2, &views[i]),
                        v(3, &motion.view),
                        v(4, &t.depth),
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                        v(6, &views[1 - i]),
                    ];
                    if copy == 1 {
                        entries.push(v(7, &t.hdr_view));
                    }
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("taa"),
                        layout: &self.layouts[copy],
                        entries: &entries,
                    })
                })
            });
            self.history = Some(History {
                size,
                textures,
                views,
                groups,
                holds: t.depth.clone(),
            });
            self.frames = 0;
        }
        let Some(h) = &self.history else {
            return;
        };
        let mode = if self.frames == 0 || self.mode == TaaMode::Jittered {
            1.0
        } else {
            0.0
        };
        let row_z = |m: Mat4| m.row(2).to_array();
        let tune = &self.tuning;
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        let params = Params {
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            prev_view_proj: jitter.prev_view_proj.to_cols_array_2d(),
            view_z: row_z(view),
            prev_view_z: row_z(jitter.prev_view),
            size: [
                size.0 as f32,
                size.1 as f32,
                1.0 / size.0 as f32,
                1.0 / size.1 as f32,
            ],
            params: [tune.alpha, tune.gamma, mode, self.frames as f32],
            flags: [
                flag(tune.object_motion),
                flag(tune.disocclusion),
                flag(tune.speed_weight),
                tune.still_gamma,
            ],
            blend: [tune.alpha_inside, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let read = self.current;
        let write = 1 - read;
        let ts = profiler.compute_scope("taa");
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("taa"),
            timestamp_writes: ts,
        });
        pass.set_pipeline(if copy { &self.copy } else { &self.pipeline });
        pass.set_bind_group(0, &h.groups[usize::from(copy)][read], &[]);
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        drop(pass);
        self.current = write;
        self.frames = self.frames.saturating_add(1);
    }
}
