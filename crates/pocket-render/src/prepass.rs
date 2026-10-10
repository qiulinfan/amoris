//! The depth prepass of the opaque forward pass (docs/spec/prepass.md; charter 4.4, Pioneer
//! 2026-10-10): whether a frame draws it, and the auto mode that decides by measured cost.
//!
//! With the prepass, each opaque phase (the one opaque pass, or with occlusion culling each of its
//! two phases) first draws its batches depth only, every variant with its own culling and alpha
//! test (renderer.rs `DepthPrepass`), then draws them with the forward pipelines' equal-depth
//! twins: no depth writes, and a sample is shaded only where its surface is the one the depth
//! holds, so each visible sample is shaded once however many surfaces cover it. Both passes compute
//! the clip position with the same expression from the same records, marked `@invariant`
//! (forward.wgsl), so their depths agree to the bit.
//!
//! [`PrepassMode::Auto`] measures instead of modelling: the GPU's pass timings (profiler.rs) of the
//! opaque passes, the prepass's included, with and without it. A probe counts only the readings of
//! its own frames (each reading names the frame it measured): frames of its kind, with instances,
//! drawn since it started, so a frame without instances, a frame from before a resize or a frame of
//! the other kind never decides. It draws the way frames draw now (at the first probe the prepass,
//! which costs at most a second geometry pass where the other way's cost has no bound) until it has
//! [`SAMPLES`] readings, then the other way one frame at a time, each only once the last one's
//! reading has arrived and only in a frame the profiler will time (at most [`OTHER_FRAMES`] in a
//! probe, should their readings be lost). The incumbent stays unless the other way is cheaper by
//! [`MARGIN`] (the first probe's incumbent is no prepass). More readings of the way drawn second
//! could only lower its minimum (the GPU's interference only ever adds time), so it wins as soon as
//! the minimums say so; it loses after [`SAMPLES`] readings, or at its first reading above the
//! first way's minimum by more than [`BAIL`]: a probe draws a much slower way for one frame. Probes
//! repeat [`PROBE_EVERY`] frames after a decision that changed the choice, twice as long after each
//! one that kept it, up to [`PROBE_MAX`], and give up [`PROBE_TIMEOUT`] frames after they started.
//! Frames with one opaque phase and frames with two (occlusion culling, occlusion.rs) are timed and
//! decided separately: occlusion culling changes what the opaque passes draw, so their costs do not
//! compare. Without timestamps the auto mode never draws the prepass.

use std::collections::VecDeque;

use crate::profiler::Timings;

/// The prepass's timestamp scope (both phases' passes; `FrameStats::passes` merges them).
pub const SCOPE: &str = "depth prepass";
/// The opaque passes' scopes (renderer.rs): one phase, or occlusion culling's early phase and its
/// late phase with the sky.
const OPAQUE_SCOPES: [&str; 2] = ["opaque early", "opaque+sky"];
/// The scope only a frame with two opaque phases has.
const EARLY_SCOPE: &str = "opaque early";
/// Frames between a probe that changed the choice and the next one...
pub const PROBE_EVERY: u64 = 240;
/// ...doubled after every probe that keeps it, up to this.
pub const PROBE_MAX: u64 = 8 * PROBE_EVERY;
/// Readings of each way a probe compares, by their minimums (the GPU's interference only ever adds
/// time).
pub const SAMPLES: usize = 4;
/// The share by which the other way must be cheaper to take over (no switching on noise; the
/// first probe's incumbent is no prepass, which draws less).
pub const MARGIN: f32 = 0.05;
/// A reading of the way a probe draws second that exceeds the first way's minimum by more than
/// this share ends the probe: that way loses without another frame drawn its way.
pub const BAIL: f32 = 0.5;
/// Frames a probe draws the way it draws second, at most (more than [`SAMPLES`] only when their
/// readings are lost); then it draws the first way until it decides or gives up.
pub const OTHER_FRAMES: usize = 2 * SAMPLES;
/// Frames (of any kind) after its start at which a probe still waiting gives up (a lost readback,
/// timestamps that stopped, a kind of frame too rare to measure): the choice stays and the next
/// probe is scheduled. Every reading a probe takes measured a frame within this span.
pub const PROBE_TIMEOUT: u64 = 600;

/// Whether the opaque pass draws the depth prepass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrepassMode {
    /// Never: the opaque pass shades in draw order.
    Off,
    /// Every frame with instances.
    On,
    /// Where it measures cheaper (the GPU's timings; off without timestamps).
    Auto,
}

impl PrepassMode {
    /// The default: measured (docs/bench/prepass.md).
    pub const DEFAULT: PrepassMode = PrepassMode::Auto;

    /// `off`, `on` or `auto` (any case; also `0`/`1`, `false`/`true`).
    pub fn parse(s: &str) -> Option<PrepassMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "false" | "no" => Some(PrepassMode::Off),
            "on" | "1" | "true" | "yes" => Some(PrepassMode::On),
            "auto" => Some(PrepassMode::Auto),
            _ => None,
        }
    }

    /// From `POCKET_PREPASS` (natively); [`PrepassMode::DEFAULT`] when it is unset, unknown, or in
    /// the browser.
    pub fn from_env() -> PrepassMode {
        match std::env::var("POCKET_PREPASS") {
            Ok(v) => PrepassMode::parse(&v).unwrap_or_else(|| {
                log::warn!("POCKET_PREPASS: {v:?} is not off, on or auto; using the default");
                PrepassMode::DEFAULT
            }),
            Err(_) => PrepassMode::DEFAULT,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PrepassMode::Off => "off",
            PrepassMode::On => "on",
            PrepassMode::Auto => "auto",
        }
    }
}

/// One frame's timing of its opaque passes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// The frame measured (`GpuProfiler::frame`).
    pub frame: u64,
    /// Whether the frame drew the prepass.
    pub prepass: bool,
    /// Whether it drew two opaque phases (occlusion culling).
    pub two_phase: bool,
    /// The opaque passes' GPU time, the prepass's included, in milliseconds.
    pub ms: f32,
}

impl Reading {
    /// From a frame's pass timings, if it has an opaque pass.
    pub fn from_timings(t: &Timings) -> Option<Reading> {
        let mut r = Reading {
            frame: t.frame,
            prepass: false,
            two_phase: false,
            ms: 0.0,
        };
        let mut opaque = false;
        for &(label, ms) in &t.passes {
            if label == SCOPE {
                r.prepass = true;
                r.ms += ms;
            } else if OPAQUE_SCOPES.contains(&label) {
                opaque = true;
                r.two_phase |= label == EARLY_SCOPE;
                r.ms += ms;
            }
        }
        opaque.then_some(r)
    }
}

/// A running probe of one kind of frame.
#[derive(Clone, Debug)]
struct Probe {
    /// Its first frame.
    start: u64,
    /// The way it draws first: the choice so far, or the prepass at the first probe.
    first: bool,
    /// Readings of the first way and of the other.
    a: Vec<f32>,
    b: Vec<f32>,
    /// Frames drawn the other way.
    others: usize,
    /// Its frames the profiler times whose readings have not arrived, oldest first, with whether
    /// each drew the prepass.
    timed: VecDeque<(u64, bool)>,
}

impl Probe {
    fn new(start: u64, first: bool) -> Probe {
        Probe {
            start,
            first,
            a: Vec::new(),
            b: Vec::new(),
            others: 0,
            timed: VecDeque::new(),
        }
    }

    /// Whether frame `frame` draws the prepass (`timed`: the profiler times it, and it is
    /// recorded). The other way once the first has its readings: in timed frames only, one at a
    /// time, [`OTHER_FRAMES`] at most.
    fn draw(&mut self, frame: u64, timed: bool) -> bool {
        let other = timed
            && self.a.len() >= SAMPLES
            && self.others < OTHER_FRAMES
            && self.timed.iter().all(|&(_, d)| d == self.first);
        self.others += usize::from(other);
        let draw = if other { !self.first } else { self.first };
        if timed {
            self.timed.push_back((frame, draw));
        }
        draw
    }

    /// Takes reading `r` if it measured one of the probe's frames. Readings arrive in frame order,
    /// so a frame awaited before `r`'s has lost its timings.
    fn take(&mut self, r: &Reading) -> bool {
        while self.timed.front().is_some_and(|&(f, _)| f < r.frame) {
            self.timed.pop_front();
        }
        let Some(&(f, drew)) = self.timed.front() else {
            return false;
        };
        if f != r.frame {
            return false;
        }
        self.timed.pop_front();
        // A frame whose timings miss its prepass scope (out of scopes) does not count.
        if drew != r.prepass {
            return false;
        }
        if drew == self.first {
            self.a.push(r.ms);
        } else {
            self.b.push(r.ms);
        }
        true
    }

    /// Whether frames draw the prepass, once the readings decide: `incumbent` stays unless the
    /// other way is cheaper by [`MARGIN`].
    fn decision(&self, incumbent: bool) -> Option<bool> {
        let &last = self.b.last()?;
        if self.a.len() < SAMPLES {
            return None;
        }
        let min = |v: &[f32]| v.iter().copied().fold(f32::INFINITY, f32::min);
        let (first, other) = (min(&self.a), min(&self.b));
        // Final as soon as it holds: more readings of the other way can only lower its minimum.
        let other_wins = if self.first == incumbent {
            other < first * (1.0 - MARGIN)
        } else {
            first >= other * (1.0 - MARGIN)
        };
        if other_wins {
            Some(!self.first)
        } else if self.b.len() >= SAMPLES || last > first * (1.0 + BAIL) {
            Some(self.first)
        } else {
            None
        }
    }
}

/// The auto mode's choice for one kind of frame (one opaque phase, or two).
#[derive(Clone, Debug, Default)]
struct Choice {
    /// Whether these frames draw the prepass (between probes).
    on: bool,
    /// Whether a probe has decided (until then there is no choice to keep).
    decided: bool,
    probe: Option<Probe>,
    /// The frame from which the next probe starts (0 at first: the first frame probes).
    next_probe: u64,
    /// The wait after the next probe that keeps the choice (0: [`PROBE_EVERY`]).
    backoff: u64,
}

impl Choice {
    /// Begins frame `frame`, of this kind (`timed`: the profiler times it): whether it draws the
    /// prepass.
    fn begin(&mut self, frame: u64, timed: bool) -> bool {
        if self
            .probe
            .as_ref()
            .is_some_and(|p| frame > p.start + PROBE_TIMEOUT)
        {
            self.probe = None;
            self.schedule(frame, false);
        }
        if self.probe.is_none() && frame >= self.next_probe {
            self.probe = Some(Probe::new(frame, self.on || !self.decided));
        }
        match &mut self.probe {
            Some(p) => p.draw(frame, timed),
            None => self.on,
        }
    }

    /// A frame's reading arrived as frame `now` begins (a reading of another frame is ignored).
    fn reading(&mut self, r: &Reading, now: u64) {
        let Some(p) = &mut self.probe else {
            return;
        };
        if !p.take(r) {
            return;
        }
        let Some(on) = p.decision(self.decided && self.on) else {
            return;
        };
        let changed = !self.decided || on != self.on;
        self.on = on;
        self.decided = true;
        self.probe = None;
        self.schedule(now, changed);
    }

    /// Schedules the next probe: soon after a change, later after each probe that kept the choice.
    fn schedule(&mut self, now: u64, changed: bool) {
        let wait = if changed {
            PROBE_EVERY
        } else {
            self.backoff.max(PROBE_EVERY)
        };
        self.backoff = (wait * 2).min(PROBE_MAX);
        self.next_probe = now + wait;
    }

    /// Drops the running probe and probes again at the next frame; the choice holds until then.
    fn restart(&mut self) {
        self.probe = None;
        self.next_probe = 0;
        self.backoff = 0;
    }
}

/// Whether frames draw the depth prepass: the mode, and the auto mode's state.
pub struct Prepass {
    pub mode: PrepassMode,
    /// Whether the GPU's pass timings exist (the auto mode measures with them).
    timestamps: bool,
    /// The auto mode's choices for frames with one opaque phase and with two.
    choices: [Choice; 2],
    /// Whether the current frame draws the prepass.
    active: bool,
}

impl Prepass {
    pub fn new(mode: PrepassMode, timestamps: bool) -> Prepass {
        Prepass {
            mode,
            timestamps,
            choices: Default::default(),
            active: false,
        }
    }

    pub fn set_mode(&mut self, mode: PrepassMode) {
        if mode != self.mode {
            self.mode = mode;
            self.restart();
        }
    }

    /// What the auto mode measured no longer holds (the target's size or format changed): each
    /// kind of frame probes again at its next frame, drawing as decided before until that probe
    /// decides, and no reading of an earlier frame counts.
    pub fn restart(&mut self) {
        for c in &mut self.choices {
            c.restart();
        }
    }

    /// Starts frame `frame` (`GpuProfiler::frame`): takes the timings that arrived since the last
    /// frame (`arrived`, oldest first: `GpuProfiler::arrived`) and says whether this frame draws
    /// the prepass (`timed`: the profiler times this frame, `GpuProfiler::times_frame`). A frame
    /// without instances never does, and is no frame of its kind.
    pub fn begin_frame(
        &mut self,
        frame: u64,
        arrived: &[Timings],
        timed: bool,
        two_phase: bool,
        instances: bool,
    ) -> bool {
        if self.mode == PrepassMode::Auto {
            for r in arrived.iter().filter_map(Reading::from_timings) {
                for c in &mut self.choices {
                    c.reading(&r, frame);
                }
            }
        }
        self.active = instances
            && match self.mode {
                PrepassMode::Off => false,
                PrepassMode::On => true,
                PrepassMode::Auto => {
                    self.timestamps && self.choices[usize::from(two_phase)].begin(frame, timed)
                }
            };
        self.active
    }

    /// Whether the current frame draws the prepass.
    pub fn active(&self) -> bool {
        self.active
    }

    /// The mode and, for auto, this frame's choice (`off`, `on`, `auto-on`, `auto-off`).
    pub fn label(&self) -> &'static str {
        match (self.mode, self.active) {
            (PrepassMode::Auto, true) => "auto-on",
            (PrepassMode::Auto, false) => "auto-off",
            (m, _) => m.name(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dense grid's opaque passes on Direct3D 12 (docs/bench/prepass.md 3): 11.2 + 4.5 ms with
    /// the prepass, 50.9 ms without.
    const DENSE: (f32, f32) = (15.7, 50.9);
    /// The sphere's (the prepass costs about a third more).
    const SPHERE: (f32, f32) = (3.95, 3.0);

    /// A frame's pass timings: `prepass` and `opaque` milliseconds, split over two phases.
    fn passes(prepass: Option<f32>, two_phase: bool, opaque: f32) -> Vec<(&'static str, f32)> {
        let mut v = vec![("cull", 0.5)];
        if two_phase {
            if let Some(p) = prepass {
                v.push((SCOPE, p / 2.0));
            }
            v.push(("opaque early", opaque / 2.0));
            v.push(("hi-z", 0.1));
            if let Some(p) = prepass {
                v.push((SCOPE, p / 2.0));
            }
            v.push(("opaque+sky", opaque / 2.0));
        } else {
            if let Some(p) = prepass {
                v.push((SCOPE, p));
            }
            v.push(("opaque+sky", opaque));
        }
        v.push(("post", 0.3));
        v
    }

    /// One frame of a simulated scene.
    #[derive(Clone, Copy)]
    struct Frame {
        /// The opaque passes' cost with and without the prepass (milliseconds); `None`: no
        /// instances (the opaque pass draws the sky alone, 0.1 ms).
        cost: Option<(f32, f32)>,
        two_phase: bool,
        /// Its readback fails (the profiler timed it, its timings never arrive).
        lost: bool,
    }

    fn scene(cost: (f32, f32)) -> Frame {
        Frame {
            cost: Some(cost),
            two_phase: false,
            lost: false,
        }
    }

    const EMPTY: Frame = Frame {
        cost: None,
        two_phase: false,
        lost: false,
    };

    /// A renderer and its profiler, simulated: the auto mode decides each frame, the frame's
    /// timings go to the readback buffer of the profiler's ring of three that its number names if
    /// that one is free (or the frame is not timed), and arrive as frame `f + lag` begins
    /// (natively `lag` is 2 headless, 2 or 3 in a window; the frame two after a reading frees its
    /// buffer, so from 4 on some frames are not timed).
    struct Sim {
        p: Prepass,
        lag: u64,
        frame: u64,
        ring: [Option<(Timings, bool)>; 3],
        /// What each frame drew.
        drawn: Vec<bool>,
    }

    impl Sim {
        fn new(lag: u64) -> Sim {
            Sim {
                p: Prepass::new(PrepassMode::Auto, true),
                lag,
                frame: 0,
                ring: Default::default(),
                drawn: Vec::new(),
            }
        }

        fn step(&mut self, f: Frame) -> bool {
            let now = self.frame;
            let mut arrived = Vec::new();
            for slot in &mut self.ring {
                if slot
                    .as_ref()
                    .is_some_and(|(t, _)| t.frame + self.lag <= now)
                    && let Some((t, lost)) = slot.take()
                    && !lost
                {
                    arrived.push(t);
                }
            }
            arrived.sort_by_key(|t| t.frame);
            let slot = &mut self.ring[(now % 3) as usize];
            let timed = slot.is_none();
            let instances = f.cost.is_some();
            let drew = self
                .p
                .begin_frame(now, &arrived, timed, f.two_phase, instances);
            if timed {
                let passes = match f.cost {
                    Some((with, _)) if drew => passes(Some(with * 0.7), f.two_phase, with * 0.3),
                    Some((_, without)) => passes(None, f.two_phase, without),
                    None => passes(None, false, 0.1),
                };
                *slot = Some((Timings { frame: now, passes }, f.lost));
            }
            self.drawn.push(drew);
            self.frame += 1;
            drew
        }

        /// `n` frames of `scene(frame)`; what they drew.
        fn run(&mut self, n: u64, scene: impl Fn(u64) -> Frame) -> Vec<bool> {
            (0..n).map(|_| self.step(scene(self.frame))).collect()
        }

        /// The runs of frames that drew `way` from frame `from` on: (first frame, length).
        fn runs(&self, from: usize, way: bool) -> Vec<(usize, usize)> {
            let mut runs: Vec<(usize, usize)> = Vec::new();
            for (f, &d) in self.drawn.iter().enumerate().skip(from) {
                if d != way {
                    continue;
                }
                match runs.last_mut() {
                    Some((start, len)) if *start + *len == f => *len += 1,
                    _ => runs.push((f, 1)),
                }
            }
            runs
        }
    }

    #[test]
    fn readings_sum_the_opaque_passes_and_the_prepass() {
        let t = |passes| Timings { frame: 7, passes };
        let r = Reading::from_timings(&t(passes(Some(2.0), true, 3.0))).unwrap();
        assert!(
            r.frame == 7 && r.prepass && r.two_phase && (r.ms - 5.0).abs() < 1e-6,
            "{r:?}"
        );
        let r = Reading::from_timings(&t(passes(None, false, 3.0))).unwrap();
        assert!(
            !r.prepass && !r.two_phase && (r.ms - 3.0).abs() < 1e-6,
            "{r:?}"
        );
        assert_eq!(Reading::from_timings(&t(vec![("cull", 1.0)])), None);
    }

    #[test]
    fn auto_turns_on_where_the_prepass_pays_and_stays_off_where_it_does_not() {
        for lag in 1..=3 {
            let mut s = Sim::new(lag);
            let drawn = s.run(100, |_| scene(DENSE));
            assert!(drawn[0], "the first frame probes, the prepass first");
            assert!(drawn[30..].iter().all(|&d| d), "lag {lag}: {drawn:?}");
            assert_eq!(s.p.label(), "auto-on");
            // Light: the prepass adds 4%: below the margin, so the incumbent (off) stays.
            let mut s = Sim::new(lag);
            let drawn = s.run(100, |_| scene((1.04, 1.0)));
            assert!(drawn[30..].iter().all(|&d| !d), "lag {lag}: {drawn:?}");
            // Saving 4% does not switch either; saving 6% does.
            let mut s = Sim::new(lag);
            let drawn = s.run(100, |_| scene((0.96, 1.0)));
            assert!(drawn[30..].iter().all(|&d| !d), "lag {lag}: {drawn:?}");
            let mut s = Sim::new(lag);
            let drawn = s.run(100, |_| scene((0.94, 1.0)));
            assert!(drawn[30..].iter().all(|&d| d), "lag {lag}: {drawn:?}");
        }
    }

    /// Readings of frames without instances (the sky alone) never count: before the fix ten empty
    /// frames at start (a loading screen) made the first probe take 0.1 ms for the frames without
    /// the prepass, and the dense grid drew without it until the next probe, 240 frames later.
    #[test]
    fn frames_without_instances_never_decide() {
        for lag in 1..=3 {
            let mut s = Sim::new(lag);
            let drawn = s.run(300, |f| if f < 10 { EMPTY } else { scene(DENSE) });
            assert!(
                drawn[..10].iter().all(|&d| !d),
                "no prepass without instances"
            );
            let off = drawn[40..].iter().filter(|&&d| !d).count();
            assert!(off <= 1, "lag {lag}: {off} frames without the prepass");
            // Empty frames between probes do not count either.
            let drawn = s.run(1000, |f| if f % 7 == 0 { EMPTY } else { scene(DENSE) });
            let off = drawn
                .iter()
                .enumerate()
                .filter(|&(i, &d)| !d && (300 + i) % 7 != 0)
                .count();
            assert!(off <= 3, "lag {lag}: {off} frames without the prepass");
        }
    }

    /// A resize forgets the old size's readings: here the small window drew without the prepass
    /// (cheaper there); its frames' readings arrive after the resize, and must not decide for the
    /// large one, where the prepass pays.
    #[test]
    fn readings_from_before_a_restart_do_not_count() {
        for lag in 1..=3 {
            for at in [100, 245, 249] {
                // At 100 between probes; at 245 and 249 in the re-probe that starts at about 240.
                let mut s = Sim::new(lag);
                s.run(at, |_| scene((2.0, 1.0)));
                assert!(s.drawn[30..].iter().all(|&d| !d));
                s.p.restart();
                s.run(400, |_| scene(DENSE));
                let late = &s.drawn[at as usize + 30..];
                assert!(
                    late.iter().filter(|&&d| !d).count() <= 1,
                    "lag {lag}, restart at {at}: {:?}",
                    &s.drawn[at as usize..]
                );
            }
        }
    }

    /// Every reading a probe takes measured a frame within [`PROBE_TIMEOUT`] frames of any kind
    /// after it started: two-phase frames that come only with occlusion culling's probes do not
    /// keep a probe of their kind waiting for thousands of frames.
    #[test]
    fn a_probe_gives_up_after_its_timeout_in_frames_of_any_kind() {
        let mut s = Sim::new(2);
        let two = |cost| Frame {
            two_phase: true,
            ..scene(cost)
        };
        // Two two-phase frames (an occlusion probe), then only one-phase frames.
        s.run(2, |_| two(DENSE));
        assert!(s.p.choices[1].probe.is_some());
        s.run(PROBE_TIMEOUT, |_| scene(DENSE));
        assert!(s.p.choices[1].probe.is_some(), "not yet");
        s.run(1, |_| two(DENSE));
        assert!(
            s.p.choices[1].probe.is_none() && !s.p.choices[1].decided,
            "{:?}",
            s.p.choices[1]
        );
        // The next probe of the kind starts afresh, its old readings gone.
        let next = s.p.choices[1].next_probe;
        s.run(next - s.frame, |_| scene(DENSE));
        s.run(1, |_| two(DENSE));
        let p = s.p.choices[1].probe.as_ref().unwrap();
        assert!(p.a.is_empty() && p.start == next, "{p:?}");
    }

    /// A probe draws a much slower way for one frame, not for the several its readings take to
    /// arrive: before the fix every re-probe of the dense grid drew five frames in a row without
    /// the prepass, about three times as slow.
    #[test]
    fn a_much_slower_way_is_drawn_for_one_frame_per_probe() {
        for lag in 1..=3 {
            let mut s = Sim::new(lag);
            s.run(4000, |_| scene(DENSE));
            let runs = s.runs(0, false);
            assert!(runs.iter().all(|&(_, len)| len == 1), "lag {lag}: {runs:?}");
            // The first probe, then probes 240, 480, 960 and 1,920 frames apart.
            assert!(runs.len() <= 5, "lag {lag}: {runs:?}");
        }
        // The same where the prepass is the slow way: the first probe draws it until it has its
        // readings (at most a second geometry pass), then each re-probe draws it once.
        for lag in 1..=3 {
            let mut s = Sim::new(lag);
            s.run(4000, |_| scene((50.9, 15.7)));
            let runs = s.runs(20, true);
            assert!(runs.iter().all(|&(_, len)| len == 1), "lag {lag}: {runs:?}");
            assert!(runs.len() <= 4, "lag {lag}: {runs:?}");
        }
    }

    /// Where the other way is only somewhat slower (the prepass on the sphere) a re-probe takes
    /// [`SAMPLES`] readings of it, one frame at a time.
    #[test]
    fn a_somewhat_slower_way_is_drawn_one_frame_at_a_time() {
        for lag in 2..=3 {
            let mut s = Sim::new(lag);
            s.run(4000, |_| scene(SPHERE));
            assert!(s.drawn[30..].iter().filter(|&&d| !d).count() > 3900);
            let runs = s.runs(30, true);
            assert!(runs.iter().all(|&(_, len)| len == 1), "lag {lag}: {runs:?}");
            assert!(runs.len() <= 4 * SAMPLES, "lag {lag}: {runs:?}");
        }
    }

    #[test]
    fn a_probe_measures_both_ways_whatever_readings_are_lost() {
        // A third of the readbacks fail: the probe decides on the readings it has.
        for lag in 1..=3 {
            let mut s = Sim::new(lag);
            let drawn = s.run(200, |f| Frame {
                lost: (f.wrapping_mul(0x9e37_79b9) >> 7) % 3 == 0,
                ..scene(DENSE)
            });
            assert!(drawn[60..].iter().all(|&d| d), "lag {lag}: {drawn:?}");
        }
        // Every other one fails, in step with the probe (each frame without the prepass is drawn
        // as the last one's loss shows, two frames on): it draws that way OTHER_FRAMES times, then
        // waits for its timeout.
        let mut s = Sim::new(3);
        let drawn = s.run(PROBE_TIMEOUT, |f| Frame {
            lost: f % 2 == 1,
            ..scene(DENSE)
        });
        assert_eq!(drawn.iter().filter(|&&d| !d).count(), OTHER_FRAMES);
        // A long readback lag (a browser): most frames find their buffer busy and are not timed;
        // the other way is drawn only in timed frames.
        for (cost, way) in [(DENSE, true), ((50.9, 15.7), false)] {
            let mut s = Sim::new(20);
            let drawn = s.run(2000, |_| scene(cost));
            assert!(drawn[300..].iter().filter(|&&d| d != way).count() <= 3);
            assert!(s.runs(300, !way).iter().all(|&(_, len)| len == 1));
        }
    }

    #[test]
    fn auto_probes_again_and_follows_a_change_with_hysteresis() {
        // On at first; from frame 200 the prepass costs more (the scene changed).
        let mut s = Sim::new(3);
        let drawn = s.run(1200, |f| scene(if f < 200 { DENSE } else { (2.0, 1.0) }));
        assert!(drawn[40..200].iter().all(|&d| d));
        // The probe after the first decision (PROBE_EVERY frames later) finds it off; after that
        // only probes draw it, one frame each.
        let first = PROBE_EVERY as usize;
        assert!(
            drawn[first + 60..first + 230].iter().all(|&d| !d),
            "{:?}",
            &drawn[first..first + 60]
        );
        let probing = drawn[first + 60..].iter().filter(|&&d| d).count();
        assert!(probing <= 3, "{probing} frames with the prepass");
        // Within the margin a decided choice stays.
        let mut s = Sim::new(3);
        let drawn = s.run(1200, |f| scene(if f < 200 { DENSE } else { (1.03, 1.0) }));
        assert!(
            drawn[40..].iter().filter(|&&d| !d).count() < 30,
            "{drawn:?}"
        );
    }

    #[test]
    fn probes_back_off_while_the_choice_holds() {
        let mut c = Choice::default();
        let mut waits = Vec::new();
        let mut now = 0;
        for _ in 0..6 {
            c.schedule(now, false);
            waits.push(c.next_probe - now);
            now = c.next_probe;
        }
        assert_eq!(waits, [240, 480, 960, 1920, 1920, 1920]);
        c.schedule(now, true);
        assert_eq!(c.next_probe - now, PROBE_EVERY);
    }

    #[test]
    fn one_and_two_phase_frames_decide_separately() {
        let mut s = Sim::new(3);
        // With occlusion culling the prepass costs more; without, it pays.
        let two = |cost| Frame {
            two_phase: true,
            ..scene(cost)
        };
        let drawn = s.run(80, |_| two((1.5, 1.0)));
        assert!(drawn[40..].iter().all(|&d| !d));
        let drawn = s.run(80, |_| scene(DENSE));
        assert!(drawn[40..].iter().all(|&d| d));
        assert!(
            !s.step(two((1.5, 1.0))),
            "two-phase frames keep their choice"
        );
    }

    #[test]
    fn modes_without_timings_and_without_instances() {
        let mut p = Prepass::new(PrepassMode::Auto, false);
        assert!(!p.begin_frame(0, &[], false, false, true));
        assert_eq!(p.label(), "auto-off");
        let mut p = Prepass::new(PrepassMode::On, false);
        assert!(p.begin_frame(0, &[], false, false, true));
        assert!(!p.begin_frame(1, &[], false, false, false));
        assert_eq!(p.label(), "on");
        let mut p = Prepass::new(PrepassMode::Off, true);
        assert!(!p.begin_frame(0, &[], true, true, true));
        assert_eq!(PrepassMode::parse(" Auto "), Some(PrepassMode::Auto));
        assert_eq!(PrepassMode::parse("1"), Some(PrepassMode::On));
        assert_eq!(PrepassMode::parse("maybe"), None);
    }
}
