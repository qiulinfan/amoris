//! Per-frame traces (docs/bench/dx12.md, "Per-frame profiling"): for every frame of a window of
//! frames, the CPU spans of the render thread and the GPU time of every timestamped pass, exported
//! as Chrome Trace Event JSON that Perfetto (ui.perfetto.dev) and chrome://tracing open and
//! `tools/frame_trace.py` summarizes and compares.
//!
//! Armed by `POCKET_TRACE=frames=N[,skip=S][,out=PATH]` (`:` works as well as `=`; a bare number is
//! the frame count) or [`crate::Renderer::start_trace`]: the first S frames (default 60) are
//! skipped, the next N (default 300) recorded, then the trace is written (default
//! `out/profiler/trace-<backend>.json`) and the renderer stops tracing. Off, it costs one `None`
//! test per span. Writing stalls the render thread once (the GPU drain, the end calibration, the
//! export: 45 to 95 ms after 300 frames here), so a host that brackets its frames chooses when,
//! with [`crate::Renderer::finish_trace`]: the window as soon as the trace is complete (one frame
//! that long), or when it closes during a benchmark; the headless benchmark after its measured
//! frames.
//!
//! - CPU spans: what [`crate::Renderer::render`] does (asset polling, scene sync, uniforms, each
//!   pass's encoding, the submit) and what the host adds around it: the window's update, surface
//!   acquire and present (app.rs), the headless benchmark's wait for the GPU. A frame is what the
//!   host brackets with `trace_begin_frame` and `trace_end_frame`, or else one `render` (which
//!   then also writes the complete trace).
//! - GPU spans: every timestamped pass of every recorded frame, read back by the profiler
//!   (profiler.rs) with the frame it belongs to; while tracing the post chain is timed too
//!   ("post"). A frame whose queries found no free readback buffer has no GPU spans (counted).
//! - Placing GPU time on the CPU timeline: wgpu 30 offers no CPU/GPU clock calibration, so the
//!   trace measures one. Before the first recorded frame and after the last it submits an empty
//!   compute pass with timestamps to an idle GPU, eight times: its first timestamp lies between
//!   the CPU time just before the submit and the CPU time when the wait for the GPU returned.
//!   The intersection of those brackets bounds the clocks' offset; GPU spans are placed with its
//!   midpoint, interpolated linearly between the two calibrations (drift), and the half-widths are
//!   recorded as the placement's uncertainty.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::profiler::RawFrame;

/// What to trace: `frames` recorded after `skip` skipped, written to `out`.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceSpec {
    pub frames: u32,
    pub skip: u32,
    pub out: Option<PathBuf>,
}

impl Default for TraceSpec {
    fn default() -> Self {
        TraceSpec {
            frames: 300,
            skip: 60,
            out: None,
        }
    }
}

impl TraceSpec {
    pub const ENV: &'static str = "POCKET_TRACE";

    /// `frames=N,skip=S,out=PATH` in any order, `:` or `=` after a key, or a bare frame count.
    pub fn parse(s: &str) -> Result<TraceSpec, String> {
        let mut spec = TraceSpec::default();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if let Ok(n) = part.parse::<u32>() {
                spec.frames = n;
                continue;
            }
            let i = part
                .find([':', '='])
                .ok_or_else(|| format!("'{part}': expected frames=N, skip=N or out=PATH"))?;
            let (key, value) = (&part[..i], &part[i + 1..]);
            let count = |v: &str| {
                v.parse::<u32>()
                    .map_err(|_| format!("{key}: '{v}' is not a frame count"))
            };
            match key {
                "frames" => spec.frames = count(value)?,
                "skip" => spec.skip = count(value)?,
                "out" if !value.is_empty() => spec.out = Some(PathBuf::from(value)),
                _ => return Err(format!("'{part}': expected frames=N, skip=N or out=PATH")),
            }
        }
        if spec.frames == 0 {
            return Err("frames must be at least 1".into());
        }
        Ok(spec)
    }

    /// `POCKET_TRACE`, if set and valid (an invalid value is reported and ignored).
    pub fn from_env() -> Option<TraceSpec> {
        let v = std::env::var(Self::ENV).ok()?;
        match Self::parse(&v) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("{}={v}: {e}; not tracing", Self::ENV);
                None
            }
        }
    }
}

/// The offset between the CPU clock ([`crate::web_time`], seconds) and the GPU's timestamps
/// (ticks times the period, seconds): CPU time = GPU time + `offset_s`, within `half_width_s`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calibration {
    pub offset_s: f64,
    pub half_width_s: f64,
    /// When it was measured (CPU seconds).
    pub at_s: f64,
}

/// Intersects bracketing samples `(cpu_before, cpu_after, gpu)`, each saying that the GPU time
/// `gpu` happened between the two CPU times; `None` when there are none or they disagree.
pub fn intersect(samples: &[(f64, f64, f64)]) -> Option<Calibration> {
    let mut lo = f64::NEG_INFINITY;
    let mut hi = f64::INFINITY;
    for &(before, after, gpu) in samples {
        lo = lo.max(before - gpu);
        hi = hi.min(after - gpu);
    }
    (lo.is_finite() && hi.is_finite() && lo <= hi).then(|| Calibration {
        offset_s: (lo + hi) / 2.0,
        half_width_s: (hi - lo) / 2.0,
        at_s: samples
            .iter()
            .map(|s| s.1)
            .fold(f64::NEG_INFINITY, f64::max),
    })
}

/// Measures the clocks' offset (see the module's documentation); `None` without timestamps or
/// when the readback fails. Waits for the GPU several times: call it outside measured frames.
#[cfg(not(target_arch = "wasm32"))]
pub fn calibrate(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    period_ns: f32,
) -> Option<Calibration> {
    const ROUNDS: usize = 8;
    if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return None;
    }
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("trace clock"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let buffer = |usage, label| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 16,
            usage,
            mapped_at_creation: false,
        })
    };
    let resolve = buffer(
        wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        "trace clock (resolve)",
    );
    let readback = buffer(
        wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        "trace clock (readback)",
    );
    let mut samples = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("trace clock"),
        });
        drop(enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("trace clock"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &queries,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            }),
        }));
        enc.resolve_query_set(&queries, 0..2, &resolve, 0);
        enc.copy_buffer_to_buffer(&resolve, 0, &readback, 0, 16);
        let commands = enc.finish();
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let before = crate::web_time();
        queue.submit([commands]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let after = crate::web_time();
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r.is_ok());
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if rx.recv() != Ok(true) {
            return None;
        }
        let ticks = {
            let data = readback.slice(..).get_mapped_range().ok()?;
            u64::from_le_bytes(data[..8].try_into().ok()?)
        };
        readback.unmap();
        samples.push((before, after, ticks as f64 * f64::from(period_ns) * 1e-9));
    }
    intersect(&samples)
}

/// A CPU span: a name and its start and end (CPU seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct CpuSpan {
    pub name: &'static str,
    pub start: f64,
    pub end: f64,
}

/// A recorded frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRecord {
    pub start: f64,
    pub end: f64,
    /// The profiler frames encoded in it (one per `render`).
    pub gpu_frames: Vec<u64>,
    pub spans: Vec<CpuSpan>,
}

/// A trace being recorded (see the module's documentation).
#[derive(Debug)]
pub struct FrameTracer {
    pub spec: TraceSpec,
    /// Frames begun since the trace was armed, skipped ones included.
    begun: u32,
    /// A frame is open, and whether the host opened it (`render` opens the others).
    open: bool,
    explicit: bool,
    /// The recorded frame in progress (`None` while skipping).
    current: Option<FrameRecord>,
    pub frames: Vec<FrameRecord>,
    /// The start calibration: `None` before it was taken, `Some(None)` when it failed.
    pub start_clock: Option<Option<Calibration>>,
}

impl FrameTracer {
    pub fn new(spec: TraceSpec) -> FrameTracer {
        FrameTracer {
            spec,
            begun: 0,
            open: false,
            explicit: false,
            current: None,
            frames: Vec::new(),
            start_clock: None,
        }
    }

    /// Whether the next frame to begin is the second-last skipped one (or the first frame with
    /// fewer to skip): the moment to calibrate the clocks. The calibration waits for the GPU, and a
    /// window benchmark's frame interval that contains it ends at the next frame's start, so it
    /// must still be a skipped frame's.
    pub fn wants_clock(&self) -> bool {
        self.start_clock.is_none() && self.begun + 2 >= self.spec.skip.max(1)
    }

    /// Whether the host opened the current frame.
    pub fn host_frame_open(&self) -> bool {
        self.open && self.explicit
    }

    /// Whether the current frame is recorded (spans are kept).
    pub fn recording(&self) -> bool {
        self.current.is_some()
    }

    pub fn begin_frame(&mut self, now: f64, explicit: bool) {
        if self.open {
            self.end_frame(now);
        }
        self.open = true;
        self.explicit = explicit;
        self.begun += 1;
        if self.begun > self.spec.skip && self.frames.len() < self.spec.frames as usize {
            self.current = Some(FrameRecord {
                start: now,
                end: now,
                gpu_frames: Vec::new(),
                spans: Vec::new(),
            });
        }
    }

    pub fn end_frame(&mut self, now: f64) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(mut f) = self.current.take() {
            f.end = now;
            self.frames.push(f);
        }
    }

    /// Every frame is recorded and the last one has ended.
    pub fn complete(&self) -> bool {
        !self.open && self.frames.len() >= self.spec.frames as usize
    }

    pub fn span(&mut self, name: &'static str, start: f64, end: f64) {
        if let Some(f) = &mut self.current {
            f.spans.push(CpuSpan { name, start, end });
        }
    }

    pub fn gpu_frame(&mut self, profiler_frame: u64) {
        if let Some(f) = &mut self.current {
            f.gpu_frames.push(profiler_frame);
        }
    }
}

/// The CPU time now, when the current frame is recorded.
#[inline]
pub fn mark(trace: &Option<FrameTracer>) -> Option<f64> {
    match trace {
        Some(t) if t.recording() => Some(crate::web_time()),
        _ => None,
    }
}

/// Records span `name` from `start` (a [`mark`]) to now.
#[inline]
pub fn span(trace: &mut Option<FrameTracer>, name: &'static str, start: Option<f64>) {
    if let (Some(t), Some(start)) = (trace, start) {
        t.span(name, start, crate::web_time());
    }
}

/// What the export needs besides the frames.
pub struct ExportInput<'a> {
    pub gpu: &'a [RawFrame],
    pub period_ns: f64,
    pub start_clock: Option<Calibration>,
    pub end_clock: Option<Calibration>,
    /// Frames whose timestamps were never resolved (every readback buffer busy).
    pub dropped: u64,
    pub meta: Map<String, Value>,
}

/// How GPU time maps to CPU time.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Clocks {
    None,
    /// One offset: a single calibration, or two that agree within their uncertainty (their
    /// intersection).
    Fixed(Calibration),
    /// Two calibrations that disagree beyond their uncertainty: the clocks drifted.
    Drifting(Calibration, Calibration),
}

fn combine(start: Option<Calibration>, end: Option<Calibration>) -> Clocks {
    match (start, end) {
        (Some(a), Some(b)) => {
            let lo = (a.offset_s - a.half_width_s).max(b.offset_s - b.half_width_s);
            let hi = (a.offset_s + a.half_width_s).min(b.offset_s + b.half_width_s);
            if lo <= hi {
                Clocks::Fixed(Calibration {
                    offset_s: (lo + hi) / 2.0,
                    half_width_s: (hi - lo) / 2.0,
                    at_s: b.at_s,
                })
            } else if b.at_s > a.at_s {
                Clocks::Drifting(a, b)
            } else {
                Clocks::Fixed(a)
            }
        }
        (Some(c), None) | (None, Some(c)) => Clocks::Fixed(c),
        (None, None) => Clocks::None,
    }
}

const CPU_PID: u32 = 1;
const GPU_PID: u32 = 2;
const CPU_TID: u32 = 1;
const GPU_FRAMES_TID: u32 = 1;
/// GPU pass lanes start here: lane 0 holds the passes, later lanes only passes that overlap one
/// before them (timestamps at pass boundaries may overlap).
const GPU_PASSES_TID: u32 = 2;

/// Places spans `(start, end)` in lanes, each span in the first lane whose last span ended by
/// its start; returns each span's lane.
pub fn lanes(spans: &[(f64, f64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by(|&a, &b| spans[a].0.total_cmp(&spans[b].0));
    let mut ends: Vec<f64> = Vec::new();
    let mut out = vec![0; spans.len()];
    for i in order {
        let (s, e) = spans[i];
        let lane = match ends.iter().position(|&end| end <= s) {
            Some(l) => l,
            None => {
                ends.push(f64::NEG_INFINITY);
                ends.len() - 1
            }
        };
        ends[lane] = e;
        out[i] = lane;
    }
    out
}

fn us(seconds: f64) -> f64 {
    (seconds * 1e9).round() / 1e3
}

fn complete_event(name: &str, pid: u32, tid: u32, start: f64, end: f64, args: Value) -> Value {
    json!({
        "name": name, "ph": "X", "pid": pid, "tid": tid,
        "ts": us(start), "dur": us((end - start).max(0.0)), "args": args,
    })
}

fn named(kind: &str, pid: u32, tid: Option<u32>, name: &str, sort: u32) -> [Value; 2] {
    let (label, index) = match kind {
        "process" => ("process_name", "process_sort_index"),
        _ => ("thread_name", "thread_sort_index"),
    };
    let mut a = json!({"name": label, "ph": "M", "pid": pid, "args": {"name": name}});
    let mut b = json!({"name": index, "ph": "M", "pid": pid, "args": {"sort_index": sort}});
    if let Some(t) = tid {
        a["tid"] = json!(t);
        b["tid"] = json!(t);
    }
    [a, b]
}

impl FrameTracer {
    /// The trace as Chrome Trace Event JSON, times in microseconds from the first recorded
    /// frame's start.
    pub fn export(&self, input: ExportInput<'_>) -> Value {
        let epoch = self.frames.first().map_or(0.0, |f| f.start);
        let start_clock = input.start_clock;
        let end_clock = input.end_clock;
        let clocks = combine(start_clock, end_clock);
        // GPU seconds to CPU seconds: one offset, or with drift beyond the calibrations'
        // uncertainty the start's interpolated towards the end's.
        let place = |gpu_s: f64| -> f64 {
            match clocks {
                Clocks::Drifting(a, b) => {
                    let t = gpu_s + a.offset_s;
                    let k = ((t - a.at_s) / (b.at_s - a.at_s)).clamp(0.0, 1.0);
                    gpu_s + a.offset_s + (b.offset_s - a.offset_s) * k
                }
                Clocks::Fixed(c) => gpu_s + c.offset_s,
                Clocks::None => gpu_s,
            }
        };
        let calibrated = !matches!(clocks, Clocks::None);
        let gpu_s = |ticks: u64| ticks as f64 * input.period_ns * 1e-9;
        let mut events: Vec<Value> = Vec::new();
        let mut gpu_frames_spans: Vec<(f64, f64, Value)> = Vec::new();
        let mut passes: Vec<(f64, f64, &'static str, usize)> = Vec::new();
        let mut missing = 0;
        for (index, f) in self.frames.iter().enumerate() {
            let rel = |t: f64| t - epoch;
            events.push(complete_event(
                "frame",
                CPU_PID,
                CPU_TID,
                rel(f.start),
                rel(f.end),
                json!({"frame": index}),
            ));
            for s in &f.spans {
                events.push(complete_event(
                    s.name,
                    CPU_PID,
                    CPU_TID,
                    rel(s.start),
                    rel(s.end),
                    json!({"frame": index}),
                ));
            }
            let raws: Vec<&RawFrame> = f
                .gpu_frames
                .iter()
                .filter_map(|id| input.gpu.iter().find(|r| r.frame == *id))
                .collect();
            if raws.is_empty() {
                missing += 1;
                continue;
            }
            // Without a calibration, a frame's GPU work starts where its submit ended (a lower
            // bound): the placement is then only a picture.
            let submit_end = f
                .spans
                .iter()
                .filter(|s| s.name == "submit")
                .map(|s| s.end)
                .fold(f.start, f64::max);
            let first = raws
                .iter()
                .flat_map(|r| r.scopes.iter().map(|s| gpu_s(s.begin)))
                .fold(f64::INFINITY, f64::min);
            let to_cpu = |g: f64| {
                if calibrated {
                    place(g)
                } else {
                    g - first + submit_end
                }
            };
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            let mut busy = 0.0;
            for r in raws {
                for s in &r.scopes {
                    let (b, e) = (to_cpu(gpu_s(s.begin)), to_cpu(gpu_s(s.end)));
                    lo = lo.min(b);
                    hi = hi.max(e);
                    busy += (e - b).max(0.0);
                    passes.push((rel(b), rel(e), s.label, index));
                }
            }
            gpu_frames_spans.push((
                rel(lo),
                rel(hi),
                json!({"frame": index, "busy_ms": (busy * 1e6).round() / 1e3}),
            ));
            events.push(json!({
                "name": "GPU busy (ms)", "ph": "C", "pid": GPU_PID, "ts": us(rel(lo)),
                "args": {"busy": (busy * 1e6).round() / 1e3},
            }));
        }
        let frame_lanes = lanes(
            &gpu_frames_spans
                .iter()
                .map(|(s, e, _)| (*s, *e))
                .collect::<Vec<_>>(),
        );
        let mut gpu_frame_lanes = 0;
        for ((s, e, args), lane) in gpu_frames_spans.into_iter().zip(frame_lanes) {
            gpu_frame_lanes = gpu_frame_lanes.max(lane + 1);
            let tid = if lane == 0 {
                GPU_FRAMES_TID
            } else {
                GPU_PASSES_TID + 100 + lane as u32
            };
            events.push(complete_event("GPU frame", GPU_PID, tid, s, e, args));
        }
        let pass_lanes = lanes(&passes.iter().map(|p| (p.0, p.1)).collect::<Vec<_>>());
        let mut pass_lane_count = 0;
        for ((s, e, label, frame), lane) in passes.into_iter().zip(pass_lanes) {
            pass_lane_count = pass_lane_count.max(lane + 1);
            events.push(complete_event(
                label,
                GPU_PID,
                GPU_PASSES_TID + lane as u32,
                s,
                e,
                json!({"frame": frame}),
            ));
        }
        let backend = input
            .meta
            .get("backend")
            .and_then(Value::as_str)
            .unwrap_or("");
        let adapter = input
            .meta
            .get("adapter")
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut head: Vec<Value> = Vec::new();
        head.extend(named("process", CPU_PID, None, "CPU", 0));
        head.extend(named("thread", CPU_PID, Some(CPU_TID), "render thread", 0));
        head.extend(named(
            "process",
            GPU_PID,
            None,
            &format!("GPU ({backend}, {adapter})"),
            1,
        ));
        head.extend(named("thread", GPU_PID, Some(GPU_FRAMES_TID), "frames", 0));
        for lane in 0..pass_lane_count.max(1) {
            let name = if lane == 0 {
                "passes".to_owned()
            } else {
                format!("passes (overlapping, {lane})")
            };
            head.extend(named(
                "thread",
                GPU_PID,
                Some(GPU_PASSES_TID + lane as u32),
                &name,
                1 + lane as u32,
            ));
        }
        for lane in 1..gpu_frame_lanes {
            head.extend(named(
                "thread",
                GPU_PID,
                Some(GPU_PASSES_TID + 100 + lane as u32),
                &format!("frames (overlapping, {lane})"),
                100 + lane as u32,
            ));
        }
        // Viewers nest complete events on one track by time; a parent that starts with its child
        // must come first.
        events.sort_by(|a, b| {
            let key = |v: &Value| {
                (
                    v["pid"].as_u64().unwrap_or(0),
                    v["tid"].as_u64().unwrap_or(0),
                    v["ts"].as_f64().unwrap_or(0.0),
                    -v["dur"].as_f64().unwrap_or(0.0),
                )
            };
            let (ka, kb) = (key(a), key(b));
            (ka.0, ka.1)
                .cmp(&(kb.0, kb.1))
                .then(ka.2.total_cmp(&kb.2))
                .then(ka.3.total_cmp(&kb.3))
        });
        head.extend(events);
        let clock = |c: Option<Calibration>| {
            c.map(|c| {
                json!({
                    "offset_s": c.offset_s,
                    "uncertainty_us": (c.half_width_s * 1e9).round() / 1e3,
                    "at_s": c.at_s - epoch,
                })
            })
        };
        // Drift is reported only when the two calibrations disagree beyond their uncertainty.
        let (drift_ppm, uncertainty_us) = match clocks {
            Clocks::Drifting(a, b) => (
                Some(((b.offset_s - a.offset_s) / (b.at_s - a.at_s) * 1e9).round() / 1e3),
                Some((a.half_width_s.max(b.half_width_s) * 1e9).round() / 1e3),
            ),
            Clocks::Fixed(c) => (None, Some((c.half_width_s * 1e9).round() / 1e3)),
            Clocks::None => (None, None),
        };
        let mut other = input.meta;
        other.insert(
            "tool".into(),
            json!("pocket-render frame trace (crates/pocket-render/src/trace.rs)"),
        );
        other.insert("format".into(), json!(1));
        other.insert("frames".into(), json!(self.frames.len()));
        other.insert("skipped".into(), json!(self.spec.skip));
        other.insert("timestamp_period_ns".into(), json!(input.period_ns));
        other.insert("gpu_frames_missing".into(), json!(missing));
        other.insert("gpu_frames_dropped".into(), json!(input.dropped));
        other.insert(
            "gpu_placement".into(),
            json!(if calibrated {
                "calibrated: bracketed timestamps of an empty compute pass, start and end"
            } else {
                "uncalibrated: each frame's GPU work drawn from the end of its submit"
            }),
        );
        other.insert(
            "clock".into(),
            json!({
                "start": clock(start_clock), "end": clock(end_clock),
                "uncertainty_us": uncertainty_us, "drift_ppm": drift_ppm,
            }),
        );
        json!({"traceEvents": head, "displayTimeUnit": "ms", "otherData": other})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiler::RawScope;

    #[test]
    fn specs_parse_and_refuse() {
        assert_eq!(TraceSpec::parse("").unwrap(), TraceSpec::default());
        let s = TraceSpec::parse("frames:120, skip=5,out=C:/tmp/t.json").unwrap();
        assert_eq!(
            (s.frames, s.skip, s.out),
            (120, 5, Some(PathBuf::from("C:/tmp/t.json")))
        );
        assert_eq!(TraceSpec::parse("40").unwrap().frames, 40);
        for bad in ["frames=0", "frames=x", "fps=3", "skip", "out="] {
            assert!(TraceSpec::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn brackets_intersect_to_the_tightest_offset() {
        // GPU clock = CPU clock - 100 s; the timestamp lands 30 to 70 us after each submit.
        let samples = [
            (10.0, 10.000_200, 10.000_030 - 100.0),
            (11.0, 11.000_100, 11.000_070 - 100.0),
        ];
        let c = intersect(&samples).unwrap();
        // Offset interval: [100.000_000 - 0.000_030, 100 + 0.000_030] = the tightest bracket.
        assert!((c.offset_s - 100.0).abs() < 1e-9, "{c:?}");
        assert!((c.half_width_s - 0.000_030).abs() < 1e-9, "{c:?}");
        assert!(intersect(&[]).is_none());
        // Disagreeing brackets (a clock that jumped) give no calibration.
        assert!(intersect(&[(0.0, 1.0, 0.0), (5.0, 6.0, 0.0)]).is_none());
    }

    #[test]
    fn calibrations_combine_or_drift() {
        let c = |offset_s, half_width_s, at_s| Calibration {
            offset_s,
            half_width_s,
            at_s,
        };
        // Agreeing within their uncertainty: one offset, the intersection.
        let Clocks::Fixed(f) = combine(Some(c(1.0, 0.5e-4, 0.0)), Some(c(1.00004, 0.5e-4, 9.0)))
        else {
            panic!("not fixed");
        };
        assert!((f.offset_s - 1.00002).abs() < 1e-12 && (f.half_width_s - 0.3e-4).abs() < 1e-12);
        // Disagreeing: drift, interpolated.
        assert!(matches!(
            combine(Some(c(1.0, 1e-5, 0.0)), Some(c(1.001, 1e-5, 9.0))),
            Clocks::Drifting(..)
        ));
        assert_eq!(combine(None, None), Clocks::None);
    }

    #[test]
    fn overlapping_spans_get_their_own_lanes() {
        let l = lanes(&[(0.0, 2.0), (2.0, 3.0), (2.5, 4.0), (4.0, 5.0), (1.0, 1.5)]);
        assert_eq!(l, vec![0, 0, 1, 0, 1]);
    }

    fn tracer() -> FrameTracer {
        let mut t = FrameTracer::new(TraceSpec {
            frames: 2,
            skip: 1,
            out: None,
        });
        // Frame 0 is skipped: nothing it records is kept.
        assert!(t.wants_clock());
        t.start_clock = Some(None);
        t.begin_frame(1.0, true);
        assert!(!t.recording());
        t.span("render", 1.0, 1.1);
        // Two recorded frames of 10 ms, the second begun by `render` (closing the first).
        for (i, start) in [(0u64, 2.0), (1, 2.010)] {
            t.begin_frame(start, i == 0);
            assert!(t.recording());
            t.span("render", start + 0.001, start + 0.004);
            t.span("submit", start + 0.003, start + 0.004);
            t.gpu_frame(10 + i);
        }
        t.end_frame(2.020);
        assert!(t.complete());
        t
    }

    #[test]
    fn export_places_gpu_passes_with_the_calibration() {
        let t = tracer();
        // GPU ticks are ns (period 1); the GPU clock runs 1000 s ahead of the CPU's.
        let gpu = |frame: u64, at: f64| RawFrame {
            frame,
            scopes: vec![
                RawScope {
                    label: "cull",
                    begin: ((at + 1000.0) * 1e9) as u64,
                    end: ((at + 0.001 + 1000.0) * 1e9) as u64,
                },
                RawScope {
                    label: "opaque+sky",
                    begin: ((at + 0.001 + 1000.0) * 1e9) as u64,
                    end: ((at + 0.005 + 1000.0) * 1e9) as u64,
                },
            ],
        };
        let raw = [gpu(10, 2.005), gpu(11, 2.015), gpu(99, 0.0)];
        let clock = |at| Calibration {
            offset_s: -1000.0,
            half_width_s: 0.000_05,
            at_s: at,
        };
        let v = t.export(ExportInput {
            gpu: &raw,
            period_ns: 1.0,
            start_clock: Some(clock(1.5)),
            end_clock: Some(clock(2.5)),
            dropped: 0,
            meta: Map::new(),
        });
        let events = v["traceEvents"].as_array().unwrap();
        let find = |name: &str, frame: u64| {
            events
                .iter()
                .find(|e| e["name"] == name && e["args"]["frame"] == frame)
                .unwrap_or_else(|| panic!("{name} {frame}"))
        };
        // Times are microseconds from the first recorded frame's start (2.0 s).
        assert_eq!(find("frame", 0)["ts"], 0.0);
        assert_eq!(find("frame", 1)["dur"], 10_000.0);
        let opaque = find("opaque+sky", 1);
        assert!(
            (opaque["ts"].as_f64().unwrap() - 16_000.0).abs() < 0.01,
            "{opaque}"
        );
        assert!(
            (opaque["dur"].as_f64().unwrap() - 4_000.0).abs() < 0.01,
            "{opaque}"
        );
        let frame = find("GPU frame", 0);
        assert!(
            (frame["ts"].as_f64().unwrap() - 5_000.0).abs() < 0.01,
            "{frame}"
        );
        assert!((frame["args"]["busy_ms"].as_f64().unwrap() - 5.0).abs() < 1e-6);
        // The skipped frame's span and the unrecorded GPU frame 99 are not in the trace.
        assert_eq!(
            events
                .iter()
                .filter(|e| e["name"] == "render" && e["ph"] == "X")
                .count(),
            2
        );
        assert_eq!(v["otherData"]["frames"], 2);
        assert_eq!(v["otherData"]["gpu_frames_missing"], 0);
        // On the CPU track a parent precedes the children that start with it.
        let cpu: Vec<&Value> = events
            .iter()
            .filter(|e| e["pid"] == CPU_PID && e["ph"] == "X")
            .collect();
        assert_eq!(cpu[0]["name"], "frame");
        assert_eq!(cpu[1]["name"], "render");
        assert_eq!(cpu[2]["name"], "submit");
    }

    #[test]
    fn drifting_clocks_place_gpu_passes_between_the_calibrations() {
        // The clocks drift 100 us apart over the second between the calibrations (100 ppm, well
        // beyond their 10 us widths): a pass at either calibration lands at that one's offset, a
        // pass between them at the offset interpolated to its time.
        let offset = |cpu_s: f64| -1000.0 + 100e-6 * (cpu_s - 1.0);
        let times = [1.0, 1.25, 2.0];
        let mut t = FrameTracer::new(TraceSpec {
            frames: 3,
            skip: 0,
            out: None,
        });
        t.start_clock = Some(None);
        for (i, at) in times.into_iter().enumerate() {
            t.begin_frame(at, true);
            t.gpu_frame(i as u64);
            t.end_frame(at + 0.001);
        }
        let raw: Vec<RawFrame> = times
            .into_iter()
            .enumerate()
            .map(|(i, at)| {
                // GPU time = CPU time - offset, in ns ticks.
                let gpu = at - offset(at);
                RawFrame {
                    frame: i as u64,
                    scopes: vec![RawScope {
                        label: "opaque+sky",
                        begin: (gpu * 1e9).round() as u64,
                        end: ((gpu + 0.004) * 1e9).round() as u64,
                    }],
                }
            })
            .collect();
        let clock = |at_s| Calibration {
            offset_s: offset(at_s),
            half_width_s: 10e-6,
            at_s,
        };
        let v = t.export(ExportInput {
            gpu: &raw,
            period_ns: 1.0,
            start_clock: Some(clock(1.0)),
            end_clock: Some(clock(2.0)),
            dropped: 0,
            meta: Map::new(),
        });
        assert_eq!(v["otherData"]["clock"]["drift_ppm"], 100.0);
        let events = v["traceEvents"].as_array().unwrap();
        for (i, at) in times.into_iter().enumerate() {
            let pass = events
                .iter()
                .find(|e| e["name"] == "opaque+sky" && e["args"]["frame"] == i)
                .unwrap_or_else(|| panic!("pass {i}"));
            // Microseconds from the first frame's start (1.0 s); the placement estimates the
            // pass's CPU time with the start's offset before interpolating, 1e-8 s off at most.
            let expected = (at - 1.0) * 1e6;
            let ts = pass["ts"].as_f64().unwrap();
            assert!(
                (ts - expected).abs() < 0.05,
                "pass {i}: {ts} us, expected {expected}"
            );
        }
    }

    #[test]
    fn export_without_a_calibration_starts_gpu_work_after_the_submit() {
        let t = tracer();
        let raw = [RawFrame {
            frame: 10,
            scopes: vec![RawScope {
                label: "cull",
                begin: 5_000_000,
                end: 6_000_000,
            }],
        }];
        let v = t.export(ExportInput {
            gpu: &raw,
            period_ns: 1.0,
            start_clock: None,
            end_clock: None,
            dropped: 3,
            meta: Map::new(),
        });
        let events = v["traceEvents"].as_array().unwrap();
        let cull = events.iter().find(|e| e["name"] == "cull").unwrap();
        // Frame 0's submit ends 4 ms into the trace.
        assert!(
            (cull["ts"].as_f64().unwrap() - 4_000.0).abs() < 0.01,
            "{cull}"
        );
        assert_eq!(v["otherData"]["gpu_frames_missing"], 1);
        assert_eq!(v["otherData"]["gpu_frames_dropped"], 3);
    }
}
