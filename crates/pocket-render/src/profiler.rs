//! GPU timings per pass with timestamp queries, read back a few frames late without stalling (a
//! ring of readback buffers; a frame that finds its buffer still busy is not timed). Each reading
//! names the frame it measured. On adapters without timestamps every timing is absent.
//!
//! While a frame trace records (trace.rs), every frame's raw timestamps are kept as well, with the
//! profiler frame they belong to, instead of only the latest frame's durations.

use std::sync::{Arc, Mutex};

pub const MAX_SCOPES: u32 = 32;
const RING: usize = 3;
/// Readback buffers while a trace records: a frame's slot is reused only once read, and in a
/// window the GPU may run a few frames behind, so the usual three would drop frames.
const TRACE_RING: usize = 8;

struct Slot {
    readback: wgpu::Buffer,
    labels: Vec<&'static str>,
    /// The frame whose timings it holds ([`GpuProfiler::frame`] when it was encoded).
    frame: u64,
    state: Arc<Mutex<SlotState>>,
}

#[derive(Default, PartialEq)]
enum SlotState {
    #[default]
    Free,
    Mapping,
    Ready,
}

/// One frame's pass timings, read back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timings {
    /// The frame measured ([`GpuProfiler::frame`] while it was encoded).
    pub frame: u64,
    /// (pass, milliseconds), in the order the passes were encoded.
    pub passes: Vec<(&'static str, f32)>,
}

/// One timed scope of a frame, in GPU timestamp ticks (multiply by [`GpuProfiler::period_ns`]).
#[derive(Clone, Debug, PartialEq)]
pub struct RawScope {
    pub label: &'static str,
    pub begin: u64,
    pub end: u64,
}

/// A frame's timed scopes in the order they were encoded.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFrame {
    /// The frame measured ([`GpuProfiler::frame`] while it was encoded).
    pub frame: u64,
    pub scopes: Vec<RawScope>,
}

pub struct GpuProfiler {
    queries: Option<wgpu::QuerySet>,
    resolve: Option<wgpu::Buffer>,
    ring: Vec<Slot>,
    /// The frame being encoded: frames submitted before it.
    frame: u64,
    labels: Vec<&'static str>,
    period_ns: f32,
    /// The latest complete frame's timings: (pass, milliseconds).
    pub last: Vec<(&'static str, f32)>,
    /// The frames read back since the latest `after_submit` began (by it, and by a `drain` after
    /// it), oldest first; `last` is the newest's. The depth prepass's auto mode takes them at the
    /// next frame (prepass.rs).
    pub arrived: Vec<Timings>,
    /// While a trace records: the frames read back since it was last taken.
    raw: Option<Vec<RawFrame>>,
    /// Frames whose queries were not resolved because their readback buffer was busy (a trace
    /// resets it when it starts).
    pub dropped: u64,
}

fn new_slot(device: &wgpu::Device) -> Slot {
    Slot {
        readback: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pass timings (readback)"),
            size: u64::from(MAX_SCOPES) * 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        labels: Vec::new(),
        frame: 0,
        state: Arc::new(Mutex::new(SlotState::Free)),
    }
}

impl GpuProfiler {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, enabled: bool) -> GpuProfiler {
        let (queries, resolve) = if enabled {
            (
                Some(device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("pass timings"),
                    ty: wgpu::QueryType::Timestamp,
                    count: MAX_SCOPES * 2,
                })),
                Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pass timings (resolve)"),
                    size: u64::from(MAX_SCOPES) * 16,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })),
            )
        } else {
            (None, None)
        };
        let ring = (0..RING).map(|_| new_slot(device)).collect();
        GpuProfiler {
            queries,
            resolve,
            ring,
            frame: 0,
            labels: Vec::new(),
            period_ns: queue.get_timestamp_period(),
            last: Vec::new(),
            arrived: Vec::new(),
            raw: None,
            dropped: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.queries.is_some()
    }

    /// Nanoseconds per timestamp tick.
    pub fn period_ns(&self) -> f32 {
        self.period_ns
    }

    /// The frame being encoded (its timings will name it): the frames submitted before it.
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Whether the frame being encoded will be timed: timestamps exist and its readback buffer is
    /// free (`resolve` skips the frame otherwise; nothing frees a buffer before it runs).
    pub fn times_frame(&self) -> bool {
        self.queries.is_some()
            && *self.slot().state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::Free
    }

    /// The readback buffer of the frame being encoded (the ring grows while a trace records).
    fn slot(&self) -> &Slot {
        &self.ring[self.frame as usize % self.ring.len()]
    }

    /// Whether every frame's raw timestamps are being kept (a trace records).
    pub fn tracing(&self) -> bool {
        self.raw.is_some()
    }

    /// Starts or stops keeping every frame's raw timestamps; starting also adds readback buffers.
    pub fn set_tracing(&mut self, device: &wgpu::Device, on: bool) {
        if !on {
            self.raw = None;
            return;
        }
        if self.raw.is_none() {
            self.raw = Some(Vec::new());
        }
        while self.enabled() && self.ring.len() < TRACE_RING {
            self.ring.push(new_slot(device));
        }
    }

    /// The raw frames read back since the last call (empty unless tracing).
    pub fn take_raw(&mut self) -> Vec<RawFrame> {
        self.raw.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// Timestamp writes for a pass named `label`, or `None` when disabled or out of scopes.
    pub fn compute_scope(
        &mut self,
        label: &'static str,
    ) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let i = self.next(label)?;
        Some(wgpu::ComputePassTimestampWrites {
            query_set: self.queries.as_ref()?,
            beginning_of_pass_write_index: Some(i * 2),
            end_of_pass_write_index: Some(i * 2 + 1),
        })
    }

    pub fn render_scope(
        &mut self,
        label: &'static str,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let i = self.next(label)?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: self.queries.as_ref()?,
            beginning_of_pass_write_index: Some(i * 2),
            end_of_pass_write_index: Some(i * 2 + 1),
        })
    }

    /// A scope over several consecutive render passes, named `label`: give the index to
    /// [`GpuProfiler::span_writes`] for each of them. `None` when disabled or out of scopes.
    /// (From branch `explore/ue5`'s `POCKET_TIME_POST`.)
    pub fn span(&mut self, label: &'static str) -> Option<u32> {
        self.next(label)
    }

    /// Timestamp writes of span `i` for one of its passes: its beginning on the `first` pass, its
    /// end on the `last` (both for a span of one pass), none in between.
    pub fn span_writes(
        &self,
        i: Option<u32>,
        first: bool,
        last: bool,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let i = i?;
        if !first && !last {
            return None;
        }
        Some(wgpu::RenderPassTimestampWrites {
            query_set: self.queries.as_ref()?,
            beginning_of_pass_write_index: first.then_some(i * 2),
            end_of_pass_write_index: last.then_some(i * 2 + 1),
        })
    }

    fn next(&mut self, label: &'static str) -> Option<u32> {
        self.queries.as_ref()?;
        let i = self.labels.len() as u32;
        if i >= MAX_SCOPES {
            return None;
        }
        self.labels.push(label);
        Some(i)
    }

    /// Resolves this frame's queries into a free readback buffer (skipped when all are busy).
    pub fn resolve(&mut self, enc: &mut wgpu::CommandEncoder) {
        let labels = std::mem::take(&mut self.labels);
        let (Some(q), Some(r)) = (&self.queries, &self.resolve) else {
            return;
        };
        if labels.is_empty() {
            return;
        }
        let frame = self.frame;
        let n_ring = self.ring.len();
        let slot = &mut self.ring[frame as usize % n_ring];
        if *slot.state.lock().unwrap_or_else(|p| p.into_inner()) != SlotState::Free {
            self.dropped += 1;
            return;
        }
        let n = labels.len() as u32 * 2;
        enc.resolve_query_set(q, 0..n, r, 0);
        enc.copy_buffer_to_buffer(r, 0, &slot.readback, 0, u64::from(n) * 8);
        slot.labels = labels;
        slot.frame = frame;
    }

    /// After submitting: starts mapping the buffer just filled and collects the finished ones
    /// into `arrived` (oldest first) and `last`.
    pub fn after_submit(&mut self) {
        self.arrived.clear();
        if self.queries.is_none() {
            return;
        }
        let idx = self.frame as usize % self.ring.len();
        self.frame += 1;
        {
            let slot = &self.ring[idx];
            let mut st = slot.state.lock().unwrap_or_else(|p| p.into_inner());
            if *st == SlotState::Free && !slot.labels.is_empty() {
                *st = SlotState::Mapping;
                let state = slot.state.clone();
                slot.readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |r| {
                        let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
                        *s = if r.is_ok() {
                            SlotState::Ready
                        } else {
                            SlotState::Free
                        };
                    });
            }
        }
        self.collect();
    }

    /// Waits for the GPU and collects every frame still in flight (the end of a trace). They join
    /// `arrived` for the next frame.
    pub fn drain(&mut self, device: &wgpu::Device) {
        if self.queries.is_none() {
            return;
        }
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        self.collect();
    }

    /// Reads every buffer whose mapping finished into `arrived`, `last` and, while tracing, the
    /// raw frames.
    fn collect(&mut self) {
        // The ring is not in frame order: several buffers can finish between submits.
        let mut order: Vec<usize> = (0..self.ring.len()).collect();
        order.sort_by_key(|&i| self.ring[i].frame);
        for i in order {
            let slot = &mut self.ring[i];
            let ready = *slot.state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::Ready;
            if !ready {
                continue;
            }
            {
                let Ok(data) = slot.readback.slice(..).get_mapped_range() else {
                    continue;
                };
                // Decode rather than cast: mapped memory may be unaligned in the browser.
                let ticks: Vec<u64> = data[..slot.labels.len() * 16]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|b| u64::from_le_bytes(*b))
                    .collect();
                self.arrived.push(Timings {
                    frame: slot.frame,
                    passes: slot
                        .labels
                        .iter()
                        .enumerate()
                        .map(|(i, l)| {
                            let d = ticks[i * 2 + 1].saturating_sub(ticks[i * 2]);
                            (*l, d as f32 * self.period_ns / 1e6)
                        })
                        .collect(),
                });
                if let Some(raw) = &mut self.raw {
                    raw.push(RawFrame {
                        frame: slot.frame,
                        scopes: slot
                            .labels
                            .iter()
                            .enumerate()
                            .map(|(i, l)| RawScope {
                                label: l,
                                begin: ticks[i * 2],
                                end: ticks[i * 2 + 1],
                            })
                            .collect(),
                    });
                }
            }
            slot.readback.unmap();
            slot.labels.clear();
            *slot.state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Free;
        }
        // A drain appends to what the last `after_submit` read.
        self.arrived.sort_by_key(|t| t.frame);
        if let Some(t) = self.arrived.last() {
            self.last.clone_from(&t.passes);
        }
    }

    pub fn total_ms(&self) -> f32 {
        self.last.iter().map(|(_, ms)| ms).sum()
    }
}
