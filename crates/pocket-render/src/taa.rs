//! Temporal anti-aliasing (charter 4.4, Pioneer 2026-10-09; docs/spec/taa-gtao.md): the
//! projection's sub-pixel jitter, the per-frame state motion vectors need (last frame's matrices
//! and pose alpha), when the history is reset, and the pass (taa.wgsl) that blends the opaque
//! pass's jittered image with the reprojected history into the image the rest of the frame reads.
//! With a render scale below 1 (charter 4.4, Pioneer 2026-10-15; docs/spec/taa-gtao.md 4.1) the
//! opaque pass draws below the output's size and the pass upscales: the jitter is in the opaque
//! pass's pixels over a longer cycle ([`jitter_phases`]) and taa.wgsl's upscaling entry points
//! reconstruct each output pixel from the samples near it.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use crate::camera::CameraState;
use crate::post::{HDR, Targets, tex};
use crate::profiler::GpuProfiler;
use crate::shaders;

/// The jitter sequence's length: Halton (2, 3) points 1 to 16 (at a render scale of 1; see
/// [`jitter_phases`]).
pub const JITTER_PHASES: u32 = 16;
/// The upscaling reconstruction kernel's sharpness, `k` of `exp(-k d^2)` with `d` in output pixels
/// (a standard deviation of 0.35 output pixels): against 2.29 (a Gaussian close to a
/// Blackman-Harris window) 0.6 to 1.7 dB closer to the supersampled reference on the still check
/// scene, for up to 0.6 dB of frame-to-frame stability under a still camera; sharper kernels gain
/// little more and lose more stability (docs/bench/taau.md 2).
pub const UPSCALE_SHARPNESS: f32 = 4.0;
/// The sequence's length for [`TaaMode::Jittered`] (references average many more positions).
pub const REFERENCE_PHASES: u32 = 1024;
/// The least weight of the current frame (the history's exponential window: about 2 / ALPHA - 1
/// frames, at least one jitter cycle).
pub const ALPHA: f32 = 0.1;
/// The clip box's half size in standard deviations of the 3x3 neighbourhood.
pub const GAMMA: f32 = 1.25;
/// The same when upscaling: the box comes from the opaque pass's coarser neighbourhood, and a wider
/// one keeps more of the output's detail (0.8 to 1.1 dB on the still check scene against 1.25, no
/// worse under motion; docs/bench/taau.md 2).
pub const UPSCALE_GAMMA: f32 = 1.5;
/// The clip box's half size where nothing moved (the reprojection moved under 0.01 output pixels)
/// when upscaling, no longer intersected with the neighbourhood's extremes either (charter 4.4, the
/// owner's decision of 2026-10-16): the coarser neighbourhood clips away detail finer than its
/// pixels whenever this frame's samples miss it, and the wider box keeps it. 3.4 to 4.4 dB on the
/// still check scene, for up to 1.5 dB around a moving object and its shadow under a still camera
/// at 0.67 and 0.77 (docs/bench/taau.md 2). At the output's size it stays off
/// ([`TaaTuning::still_gamma`] 0), for that lag.
pub const UPSCALE_STILL_GAMMA: f32 = 3.0;
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
    scene: [f32; 4],
    jitter: [f32; 4],
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
    /// Upscaling: the clip box's half size in sigmas ([`UPSCALE_GAMMA`], in place of `gamma`); the
    /// reconstruction kernel's sharpness ([`UPSCALE_SHARPNESS`]); whether the
    /// current frame's share follows the weight of its samples near each output pixel (else it is
    /// the usual `alpha` everywhere); whether the opaque pass filters for the output's pixel:
    /// material textures at the output's mip level, specular anti-aliasing and the sea's wavelets
    /// over its footprint (their derivatives scaled by the render scale; `Renderer` reads it).
    pub upscale_gamma: f32,
    pub upscale_sharpness: f32,
    pub upscale_weight: bool,
    pub texture_lod: bool,
    /// Upscaling: the clip box in sigmas where nothing moved ([`UPSCALE_STILL_GAMMA`], in place of
    /// `still_gamma`; 0: `upscale_gamma` there too).
    pub upscale_still_gamma: f32,
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
            upscale_gamma: UPSCALE_GAMMA,
            upscale_sharpness: UPSCALE_SHARPNESS,
            upscale_weight: true,
            texture_lod: true,
            upscale_still_gamma: UPSCALE_STILL_GAMMA,
        }
    }
}

impl TaaTuning {
    /// The clip box's half sizes in sigmas, (where something moved, where nothing did), at the
    /// output's size or upscaling: the wider box where nothing moved is on only when upscaling
    /// (charter 4.4, the owner's decision of 2026-10-16), so scale 1 draws as it did before it.
    pub fn clip_gammas(&self, upscale: bool) -> (f32, f32) {
        if upscale {
            (self.upscale_gamma, self.upscale_still_gamma)
        } else {
            (self.gamma, self.still_gamma)
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
    /// The output's size and the opaque pass's.
    size: (u32, u32),
    scene: (u32, u32),
}

/// This frame's jitter and what motion vectors need from last frame (`Taa::begin_frame`).
#[derive(Clone, Copy, Debug)]
pub struct FrameJitter {
    /// The jitter in normalized device coordinates (zero without TAA).
    pub ndc: Vec2,
    /// The same in the opaque pass's pixels, y down: the projection moves the scene by it.
    pub pixels: Vec2,
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

/// The jitter cycle for an output of `size` drawn at `scene`: [`JITTER_PHASES`] at the output's
/// size, else `max(16, ceil(8 / s^2))` for `s^2` the opaque pass's pixels over the output's (each
/// output pixel is near a sample of this frame `s^2` times as often; FSR 2's rule, at least the
/// unscaled cycle).
pub fn jitter_phases(size: (u32, u32), scene: (u32, u32)) -> u32 {
    if scene == size {
        return JITTER_PHASES;
    }
    let s2 = (f64::from(scene.0) * f64::from(scene.1))
        / (f64::from(size.0.max(1)) * f64::from(size.1.max(1)));
    ((8.0 / s2.max(1e-6)).ceil() as u32).clamp(JITTER_PHASES, 1024)
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
    /// The same two upscaling, for an opaque pass below the output's size.
    upscale: [wgpu::ComputePipeline; 2],
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
            upscale: [
                pipe("upscale_history", &history_layout),
                pipe("upscale_copy", &copy_layout),
            ],
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

    /// Starts a frame drawn from `camera` (view `view`, unjittered view-projection `view_proj`) for
    /// an output of `size`, the opaque pass drawing at `scene`, showing the poses of simulated time
    /// `drawn_t` at interpolation `alpha` between ticks `dt_s` apart. `active` is whether TAA runs
    /// (without, no jitter, but motion state is still kept so turning it on starts cleanly).
    #[allow(clippy::too_many_arguments)]
    pub fn begin_frame(
        &mut self,
        active: bool,
        camera: &CameraState,
        view: Mat4,
        view_proj: Mat4,
        size: (u32, u32),
        scene: (u32, u32),
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
                    || p.scene != scene
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
            scene,
        });
        if reset {
            self.frames = 0;
            self.reset_requested = false;
        }
        let (pixels, ndc) = if active {
            self.frame = self.frame.wrapping_add(1);
            let phases = match self.mode {
                TaaMode::Temporal => jitter_phases(size, scene),
                TaaMode::Jittered => REFERENCE_PHASES,
            };
            let j = jitter_pixels(self.frame, phases) * self.tuning.jitter;
            // In normalized device coordinates, from the opaque pass's pixels.
            (
                j,
                Vec2::new(2.0 * j.x / scene.0 as f32, -2.0 * j.y / scene.1 as f32),
            )
        } else {
            (Vec2::ZERO, Vec2::ZERO)
        };
        FrameJitter {
            ndc,
            pixels,
            prev_view_proj,
            prev_view,
            prev_alpha,
            reset,
        }
    }

    /// Blends this frame (`t.raw`) with the history into the next history ([`Taa::output`]) and,
    /// with `copy` (splats follow), into `t.hdr` too. `jitter` is this frame's (`begin_frame`),
    /// `view` and `view_proj` its unjittered matrices. With the opaque pass below the output's size
    /// (`t.scene`) it upscales: the history and `t.hdr` are the output's size.
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
        let upscale = t.scene != size;
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
        // Upscaling's kernel and its expected weight per output pixel and frame: the samples per
        // output pixel times its integral, pi / k.
        let sharpness = self.tuning.upscale_sharpness.max(1e-3);
        let density = (t.scene.0 as f32 / size.0 as f32) * (t.scene.1 as f32 / size.1 as f32);
        // Zero: the usual share everywhere (the weighting off, for evaluation).
        let expected = if self.tuning.upscale_weight {
            density * std::f32::consts::PI / sharpness
        } else {
            0.0
        };
        let tune = &self.tuning;
        let (gamma, still_gamma) = tune.clip_gammas(upscale);
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
            params: [tune.alpha, gamma, mode, self.frames as f32],
            flags: [
                flag(tune.object_motion),
                flag(tune.disocclusion),
                flag(tune.speed_weight),
                still_gamma,
            ],
            blend: [tune.alpha_inside, 0.0, 0.0, 0.0],
            scene: [
                t.scene.0 as f32,
                t.scene.1 as f32,
                1.0 / t.scene.0 as f32,
                1.0 / t.scene.1 as f32,
            ],
            // exp(-k d^2) as exp2(z d^2).
            jitter: [
                jitter.pixels.x,
                jitter.pixels.y,
                -sharpness / std::f32::consts::LN_2,
                expected,
            ],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        let read = self.current;
        let write = 1 - read;
        let ts = profiler.compute_scope("taa");
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("taa"),
            timestamp_writes: ts,
        });
        pass.set_pipeline(match (upscale, copy) {
            (false, false) => &self.pipeline,
            (false, true) => &self.copy,
            (true, c) => &self.upscale[usize::from(c)],
        });
        pass.set_bind_group(0, &h.groups[usize::from(copy)][read], &[]);
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        drop(pass);
        self.current = write;
        self.frames = self.frames.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's decision of 2026-10-16 (charter 4.4): the wider box where nothing moved when
    /// upscaling, and none at the output's size, whose pixels must not change.
    #[test]
    fn the_still_box_widens_only_when_upscaling() {
        let t = TaaTuning::default();
        assert_eq!(t.clip_gammas(false), (GAMMA, 0.0));
        assert_eq!(t.clip_gammas(true), (UPSCALE_GAMMA, UPSCALE_STILL_GAMMA));
        let off = TaaTuning {
            upscale_still_gamma: 0.0,
            ..t
        };
        assert_eq!(off.clip_gammas(true), (UPSCALE_GAMMA, 0.0));
    }
}
