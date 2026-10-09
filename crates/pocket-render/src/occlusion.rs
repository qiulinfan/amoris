//! Two-phase occlusion culling for the camera view (docs/spec/occlusion.md; charter 4.4).
//!
//! When it runs, a frame's camera view is drawn in two phases. The early culling pass (cull.wgsl
//! `main`) keeps the instances in the frustum that were visible at the end of the last frame; they
//! are drawn and their depth reduced into a depth pyramid (hiz.wgsl). The late culling pass
//! (cull.wgsl `late`) tests every instance in the frustum against that pyramid, draws the visible
//! ones the early pass left out through the late argument set (batches.rs [`LATE`]) and records
//! the visible set for the next frame. Only the late test removes instances, and only against
//! depth this frame actually drew, so the image never depends on the last frame's set: the first
//! frame, a resize or a camera cut draw correctly, just with more late work.
//!
//! [`OcclusionMode::Auto`] turns it off where it does not pay: the late pass counts what it
//! culled, read back a few frames later; when the occluded triangles do not cover the second
//! phase's cost (a fixed part and a part per instance in the frustum, both counted in triangles)
//! the renderer goes back to one culling pass and one opaque pass, and probes again
//! [`PROBE_EVERY`] frames later, waiting twice as long after each probe that finds it still does
//! not pay (up to [`PROBE_MAX`]). Such a probe runs two phases for two frames only (the second is
//! the one measured) and decides when that frame's reading arrives: a few frames later natively,
//! up to a second later in a browser. The first probe (at start, or when the mode is set) is
//! optimistic instead: it keeps two phases until its reading arrives, so a scene that needs
//! occlusion culling never waits a browser's readback without it.
//!
//! [`LATE`]: crate::batches::LATE

use std::sync::{Arc, Mutex};

use crate::profiler::GpuProfiler;
use crate::shaders;

/// Frames before the first probe after [`OcclusionMode::Auto`] turned occlusion culling off...
pub const PROBE_EVERY: u64 = 120;
/// ...doubled after every probe that does not pay, up to this.
pub const PROBE_MAX: u64 = 8 * PROBE_EVERY;
/// The second phase's cost in the triangles it must remove to pay (the auto mode's model,
/// calibrated on many_cubes on an RTX 5060, where about a million instanced triangles cost a
/// millisecond; docs/bench/occlusion.md): a fixed part (the pyramid, the second opaque pass's
/// load and resolve)...
const FIXED_TRIANGLES: f64 = 200_000.0;
/// ...and a part per instance in the frustum (the late culling pass's test).
const TRIANGLES_PER_INSTANCE: f64 = 0.5;
/// A probe turns occlusion culling on when the occluded triangles exceed the cost by this factor;
/// it then stays on while they cover the cost, going off after [`MISSES`] readings in a row that
/// do not.
const PROBE_MARGIN: f64 = 1.5;
const MISSES: u32 = 3;
/// Frames a probe waits for its reading before it counts as failed (a lost readback).
const PROBE_TIMEOUT: u64 = 600;
/// Readback buffers for the late pass's counters.
const RING: usize = 4;
// hiz.wgsl's first level reads the depth target as `texture_depth_multisampled_2d`.
const _: () = assert!(
    crate::post::SAMPLES > 1,
    "the depth pyramid reads a multisampled target"
);

/// Whether the camera view is occlusion culled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcclusionMode {
    /// Frustum culling only: one culling pass, one opaque pass.
    Off,
    /// Two-phase occlusion culling every frame.
    On,
    /// On while it culls enough (the late pass's counters), probed periodically while off.
    Auto,
}

impl OcclusionMode {
    /// `off`, `on` or `auto` (any case; also `0`/`1`, `false`/`true`).
    pub fn parse(s: &str) -> Option<OcclusionMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Some(OcclusionMode::Off),
            "on" | "1" | "true" | "yes" => Some(OcclusionMode::On),
            "auto" => Some(OcclusionMode::Auto),
            _ => None,
        }
    }

    /// From `POCKET_OCCLUSION` (natively); [`OcclusionMode::Auto`] when it is unset, unknown, or
    /// in the browser.
    pub fn from_env() -> OcclusionMode {
        match std::env::var("POCKET_OCCLUSION") {
            Ok(v) => OcclusionMode::parse(&v).unwrap_or_else(|| {
                log::warn!("POCKET_OCCLUSION: {v:?} is not off, on or auto; using auto");
                OcclusionMode::Auto
            }),
            Err(_) => OcclusionMode::Auto,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            OcclusionMode::Off => "off",
            OcclusionMode::On => "on",
            OcclusionMode::Auto => "auto",
        }
    }
}

/// What the late culling pass found in one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OcclusionStats {
    /// The frame it describes (the renderer's frame counter).
    pub frame: u64,
    /// Instances in the camera frustum.
    pub frustum: u32,
    /// Of those, hidden behind the depth pyramid (not drawn, or drawn early and hidden now).
    pub occluded: u32,
    /// Drawn by the early pass (visible the frame before).
    pub early: u32,
    /// Drawn by the late pass (newly visible).
    pub late: u32,
    /// Triangles of the instances in the frustum.
    pub frustum_triangles: u64,
    /// Triangles of the occluded ones.
    pub occluded_triangles: u64,
}

impl OcclusionStats {
    fn from_words(frame: u64, w: [u32; 8]) -> OcclusionStats {
        OcclusionStats {
            frame,
            frustum: w[0],
            occluded: w[1],
            early: w[2],
            late: w[3],
            frustum_triangles: u64::from(w[4]) | u64::from(w[5]) << 32,
            occluded_triangles: u64::from(w[6]) | u64::from(w[7]) << 32,
        }
    }

    /// The share of the frustum's triangles occlusion culling removed.
    pub fn occluded_share(&self) -> f64 {
        self.occluded_triangles as f64 / self.frustum_triangles.max(1) as f64
    }
}

/// The auto mode's state.
#[derive(Clone, Copy, Debug, Default)]
struct Auto {
    /// On (not probing): two phases every frame.
    on: bool,
    /// When on: the frame it went on. That frame's early pass draws a stale set, so readings
    /// count from the next.
    since: u64,
    /// A probe's first frame `p`: frames `p` and `p + 1` run two phases, and the reading of
    /// `p + 1` decides.
    probe: Option<u64>,
    misses: u32,
    next_probe: u64,
    /// The wait after the next probe that does not pay (0: [`PROBE_EVERY`]).
    backoff: u64,
    /// Probes run two frames only; until a probe has failed they run until their reading.
    brief: bool,
}

impl Auto {
    /// Whether frame `frame` runs two phases. A frame without instances runs none, and moves a
    /// probe or an activation that would have started there to the next frame.
    fn runs(&mut self, frame: u64, instances: bool) -> bool {
        if self.probe.is_some_and(|p| frame > p + 1 + PROBE_TIMEOUT) {
            self.probe = None;
            self.fail(frame, true);
        }
        if !self.on && self.probe.is_none() && frame >= self.next_probe {
            self.probe = Some(frame);
        }
        if !instances {
            if let Some(p) = self.probe.filter(|&p| frame <= p + 1) {
                self.probe = Some(p.max(frame + 1));
            }
            if self.on {
                self.since = frame;
            }
            return false;
        }
        self.on || self.probe.is_some_and(|p| frame <= p + 1 || !self.brief)
    }

    /// Whether the second phase paid for itself in reading `s`, against the cost times `margin`.
    fn pays(s: &OcclusionStats, margin: f64) -> bool {
        let cost = FIXED_TRIANGLES + TRIANGLES_PER_INSTANCE * f64::from(s.frustum);
        s.occluded_triangles as f64 >= cost * margin
    }

    fn reading(&mut self, s: &OcclusionStats, now: u64) {
        if let Some(p) = self.probe {
            if s.frame <= p {
                return;
            }
            self.probe = None;
            if Auto::pays(s, PROBE_MARGIN) {
                self.on = true;
                self.since = now;
                self.misses = 0;
                self.backoff = PROBE_EVERY;
            } else {
                self.fail(now, true);
            }
            return;
        }
        if !self.on || s.frame <= self.since {
            return;
        }
        if Auto::pays(s, 1.0) {
            self.misses = 0;
            return;
        }
        self.misses += 1;
        if self.misses >= MISSES {
            self.on = false;
            self.fail(now, false);
        }
    }

    /// Schedules the next probe: a failed probe waits longer each time; culling that stopped
    /// paying probes again soon.
    fn fail(&mut self, now: u64, probe: bool) {
        let wait = if probe {
            self.backoff.max(PROBE_EVERY)
        } else {
            PROBE_EVERY
        };
        self.backoff = (wait * 2).min(PROBE_MAX);
        self.brief = true;
        self.misses = 0;
        self.next_probe = now + wait;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Free,
    /// Holds a copy of the counters, not yet mapped.
    Copied,
    Mapping,
    Ready,
}

struct Readback {
    buffer: wgpu::Buffer,
    frame: u64,
    state: Arc<Mutex<Slot>>,
}

/// The depth pyramid: level `k` of `(w / 2) >> k` by `(h / 2) >> k` texels (at least 1) for a
/// `w` x `h` depth target.
struct Pyramid {
    size: (u32, u32),
    /// Every level, for the late culling pass.
    all: wgpu::TextureView,
    /// Per level: its bind group (level 0 reads the depth target, the others the level below) and
    /// its size.
    levels: Vec<(wgpu::BindGroup, (u32, u32))>,
}

fn b(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buf.as_entire_binding(),
    }
}

/// The buffers the late culling pass binds (the renderer's).
pub struct LateInputs<'a> {
    pub cull: &'a wgpu::Buffer,
    pub instances: &'a wgpu::Buffer,
    pub meshes: &'a wgpu::Buffer,
    pub draws: &'a wgpu::Buffer,
    pub offsets: &'a wgpu::Buffer,
    pub drawn: &'a wgpu::Buffer,
    pub state: &'a wgpu::Buffer,
}

pub struct Occlusion {
    pub mode: OcclusionMode,
    auto: Auto,
    /// Frames begun.
    frame: u64,
    /// Whether the current frame runs two phases.
    active: bool,
    from_depth: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
    late: wgpu::ComputePipeline,
    pyramid: Option<Pyramid>,
    late_group: Option<wgpu::BindGroup>,
    /// The late pass's counters (`stats` in cull.wgsl).
    stats: wgpu::Buffer,
    ring: Vec<Readback>,
    /// The latest reading of the late pass's counters.
    pub last: Option<OcclusionStats>,
}

/// The pyramid's levels for a `w` x `h` depth target.
pub fn levels(w: u32, h: u32) -> u32 {
    let (w0, h0) = ((w / 2).max(1), (h / 2).max(1));
    w0.max(h0).ilog2() + 1
}

impl Occlusion {
    pub fn new(device: &wgpu::Device, mode: OcclusionMode) -> Occlusion {
        let compute = |name: &str, entry: &str| {
            let m = shaders::module(device, name);
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &m,
                entry_point: Some(entry),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        let ring = (0..RING)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("occlusion stats (readback)"),
                    size: 32,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                frame: 0,
                state: Arc::new(Mutex::new(Slot::Free)),
            })
            .collect();
        Occlusion {
            mode,
            // Off with a probe due: the first frame activates it.
            auto: Auto::default(),
            frame: 0,
            active: false,
            from_depth: compute("hiz", "from_depth"),
            reduce: compute("hiz", "reduce"),
            late: compute("cull", "late"),
            pyramid: None,
            late_group: None,
            stats: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("occlusion stats"),
                size: 32,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            ring,
            last: None,
        }
    }

    /// Starts a frame: collects finished readings and decides whether this frame runs two phases
    /// (never without instances: an auto activation then starts counting at the first frame that
    /// has some, whose early set is the first one it computed).
    pub fn begin_frame(&mut self, instances: bool) -> bool {
        self.collect();
        self.frame += 1;
        self.active = match self.mode {
            OcclusionMode::Off => false,
            OcclusionMode::On => true,
            OcclusionMode::Auto => self.auto.runs(self.frame, instances),
        } && instances;
        self.active
    }

    /// Whether the current frame runs two phases.
    pub fn active(&self) -> bool {
        self.active
    }

    /// The mode and, for auto, its current state (`off`, `on`, `auto-on`, `auto-off`).
    pub fn label(&self) -> &'static str {
        match (self.mode, self.active) {
            (OcclusionMode::Auto, true) => "auto-on",
            (OcclusionMode::Auto, false) => "auto-off",
            (m, _) => m.name(),
        }
    }

    pub fn set_mode(&mut self, mode: OcclusionMode) {
        if mode != self.mode {
            self.mode = mode;
            // Auto starts with a probe on the next frame.
            self.auto = Auto::default();
        }
    }

    /// Forgets what holds the depth target or the renderer's buffers (after a resize or when
    /// buffers were replaced).
    pub fn invalidate(&mut self, pyramid: bool) {
        self.late_group = None;
        if pyramid {
            self.pyramid = None;
        }
    }

    /// Builds the depth pyramid from `depth` (the multisampled depth target, `size` pixels).
    pub fn encode_pyramid(
        &mut self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        depth: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        if self.pyramid.as_ref().is_none_or(|p| p.size != size) {
            self.pyramid = Some(self.make_pyramid(device, depth, size));
            self.late_group = None;
        }
        let Some(p) = &self.pyramid else {
            return;
        };
        let ts = profiler.compute_scope("hi-z");
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("hi-z"),
            timestamp_writes: ts,
        });
        for (k, (group, (w, h))) in p.levels.iter().enumerate() {
            pass.set_pipeline(if k == 0 {
                &self.from_depth
            } else {
                &self.reduce
            });
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
    }

    fn make_pyramid(
        &self,
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
        size: (u32, u32),
    ) -> Pyramid {
        let (w0, h0) = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let count = levels(size.0, size.1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hi-z"),
            size: wgpu::Extent3d {
                width: w0,
                height: h0,
                depth_or_array_layers: 1,
            },
            mip_level_count: count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let mip = |k: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("hi-z level"),
                base_mip_level: k,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let views: Vec<wgpu::TextureView> = (0..count).map(mip).collect();
        let levels = (0..count)
            .map(|k| {
                let (layout, src) = if k == 0 {
                    (self.from_depth.get_bind_group_layout(0), depth)
                } else {
                    (self.reduce.get_bind_group_layout(0), &views[k as usize - 1])
                };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("hi-z level"),
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: if k == 0 { 0 } else { 2 },
                            resource: wgpu::BindingResource::TextureView(src),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&views[k as usize]),
                        },
                    ],
                });
                (group, ((w0 >> k).max(1), (h0 >> k).max(1)))
            })
            .collect();
        Pyramid {
            size,
            all: texture.create_view(&Default::default()),
            levels,
        }
    }

    /// The late culling pass over `n` instances (after [`Occlusion::encode_pyramid`]): draws go to
    /// the late argument set, the counters to a readback buffer when one is free.
    pub fn encode_late(
        &mut self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        profiler: &mut GpuProfiler,
        inputs: &LateInputs<'_>,
        n: u32,
    ) {
        let Some(p) = &self.pyramid else {
            return;
        };
        if self.late_group.is_none() {
            self.late_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cull (late)"),
                layout: &self.late.get_bind_group_layout(0),
                entries: &[
                    b(0, inputs.cull),
                    b(1, inputs.instances),
                    b(2, inputs.meshes),
                    b(3, inputs.draws),
                    b(5, inputs.offsets),
                    b(6, inputs.drawn),
                    b(7, inputs.state),
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&p.all),
                    },
                    b(9, &self.stats),
                ],
            }));
        }
        let Some(group) = &self.late_group else {
            return;
        };
        enc.clear_buffer(&self.stats, 0, None);
        {
            let groups = n.div_ceil(256);
            let (gx, gy) = if groups > 65535 {
                (65535, groups.div_ceil(65535))
            } else {
                (groups, 1)
            };
            let ts = profiler.compute_scope("cull late");
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cull (late)"),
                timestamp_writes: ts,
            });
            pass.set_pipeline(&self.late);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        let frame = self.frame;
        if let Some(slot) = self
            .ring
            .iter_mut()
            .find(|s| *s.state.lock().unwrap_or_else(|e| e.into_inner()) == Slot::Free)
        {
            enc.copy_buffer_to_buffer(&self.stats, 0, &slot.buffer, 0, 32);
            slot.frame = frame;
            *slot.state.lock().unwrap_or_else(|e| e.into_inner()) = Slot::Copied;
        }
    }

    /// After the frame's submit: starts mapping the counters copied this frame.
    pub fn after_submit(&mut self) {
        for slot in &self.ring {
            let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
            if *st != Slot::Copied {
                continue;
            }
            *st = Slot::Mapping;
            let state = slot.state.clone();
            slot.buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| {
                    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                    *s = if r.is_ok() { Slot::Ready } else { Slot::Free };
                });
        }
    }

    /// Reads the finished counters, oldest first, and lets the auto mode decide.
    fn collect(&mut self) {
        let mut readings = Vec::new();
        for slot in &mut self.ring {
            let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
            if *st != Slot::Ready {
                continue;
            }
            if let Ok(data) = slot.buffer.slice(..).get_mapped_range() {
                // Decode rather than cast: mapped memory may be unaligned in the browser.
                let mut w = [0u32; 8];
                for (i, b) in data.as_chunks::<4>().0.iter().take(8).enumerate() {
                    w[i] = u32::from_le_bytes(*b);
                }
                readings.push(OcclusionStats::from_words(slot.frame, w));
            }
            slot.buffer.unmap();
            *st = Slot::Free;
        }
        readings.sort_by_key(|s| s.frame);
        for s in readings {
            if self.mode == OcclusionMode::Auto {
                self.auto.reading(&s, self.frame);
            }
            self.last = Some(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(frame: u64, instances: u32, occluded: u64) -> OcclusionStats {
        OcclusionStats {
            frame,
            frustum: instances,
            frustum_triangles: 20_000_000,
            occluded_triangles: occluded,
            ..OcclusionStats::default()
        }
    }

    #[test]
    fn levels_halve_down_to_one_texel() {
        assert_eq!(levels(1280, 720), 10); // 640x360 .. 1x1
        assert_eq!(levels(1, 1), 1);
        assert_eq!(levels(3, 2), 1);
        assert_eq!(levels(4, 4), 2);
        assert_eq!(levels(1920, 1080), 10);
    }

    #[test]
    fn auto_keeps_what_pays_and_drops_what_does_not() {
        // The first frame probes, and keeps two phases until frame 2's reading arrives.
        let mut a = Auto::default();
        assert!(a.runs(1, true) && a.runs(2, true));
        assert!(a.runs(3, true) && a.runs(4, true));
        // The probe's first frame drew a stale early set: its reading does not count.
        a.reading(&stats(1, 400_000, 0), 4);
        assert!(a.probe.is_some());
        a.reading(&stats(2, 400_000, 9_000_000), 5);
        assert!(a.on && a.since == 5);
        assert!(a.runs(6, true));
        // Readings up to the activation do not count; then 300,000 triangles cover the cost of
        // 200,000 instances (200,000 + 100,000)...
        a.reading(&stats(5, 200_000, 0), 7);
        assert!(a.on && a.misses == 0);
        a.reading(&stats(6, 200_000, 300_000), 8);
        assert!(a.on && a.misses == 0);
        // ...and less three times in a row turns it off, to be probed again 120 frames later.
        for f in 7..9 {
            a.reading(&stats(f, 200_000, 299_000), f + 2);
            assert!(a.on);
        }
        a.reading(&stats(9, 200_000, 299_000), 11);
        assert!(!a.on && a.next_probe == 11 + PROBE_EVERY);
        assert!(!a.runs(12, true));
        // Once a probe has failed, probes are brief: two frames, then off until the reading.
        assert!(a.brief && a.runs(131, true) && a.runs(132, true) && !a.runs(133, true));
        // A probe must clear the cost by half again: 400,000 triangles do not pay for it there.
        let probe = |instances, occluded| {
            let mut p = Auto {
                probe: Some(10),
                ..Auto::default()
            };
            p.reading(&stats(11, instances, occluded), 13);
            p.on
        };
        assert!(!probe(200_000, 400_000));
        assert!(probe(200_000, 450_000));
        // Many instances in the frustum raise the bar; few occluded triangles never pay.
        assert!(!probe(1_600_000, 1_400_000));
        assert!(probe(1_600_000, 1_600_000));
        assert!(!probe(100, 1000));
        // Probes that keep failing wait 120, 240, 480, 960, 960... frames.
        let mut b = Auto::default();
        let mut now = 1;
        let mut waits = Vec::new();
        for _ in 0..5 {
            assert!(b.runs(now, true) && b.runs(now + 1, true));
            b.reading(&stats(now + 1, 1000, 0), now + 3);
            assert!(!b.on && b.probe.is_none());
            waits.push(b.next_probe - (now + 3));
            now = b.next_probe;
        }
        assert_eq!(waits, [120, 240, 480, 960, 960]);
        // A probe that pays resets the wait.
        assert!(b.runs(now, true) && b.runs(now + 1, true));
        b.reading(&stats(now + 1, 1000, 9_000_000), now + 3);
        assert!(b.on && b.backoff == PROBE_EVERY);
        // Frames without instances run nothing and push the probe back; a lost reading times out.
        let mut c = Auto {
            brief: true,
            ..Auto::default()
        };
        assert!(!c.runs(1, false) && !c.runs(2, false));
        assert!(c.runs(3, true) && c.runs(4, true) && !c.runs(5, true));
        assert_eq!(c.probe, Some(3));
        assert!(!c.runs(4 + PROBE_TIMEOUT + 1, true));
        assert!(c.probe.is_none() && c.next_probe == 4 + PROBE_TIMEOUT + 1 + PROBE_EVERY);
    }

    #[test]
    fn modes_parse() {
        assert_eq!(OcclusionMode::parse("ON"), Some(OcclusionMode::On));
        assert_eq!(OcclusionMode::parse(" off "), Some(OcclusionMode::Off));
        assert_eq!(OcclusionMode::parse("auto"), Some(OcclusionMode::Auto));
        assert_eq!(OcclusionMode::parse("maybe"), None);
    }
}
