//! GPU timings per pass with timestamp queries, read back a few frames late without stalling (a
//! ring of readback buffers; a frame that finds its buffer still busy is not timed). Each reading
//! names the frame it measured. On adapters without timestamps every timing is absent.
//!
//! While a frame trace records (trace.rs), every frame's raw timestamps are kept as well, with the
//! profiler frame they belong to, instead of only the latest frame's durations. Each in-flight
//! slot owns its queries until readback completes. Metal resolves only after the sampling
//! submission completes: resolving in that submission can return zero or the previous values.

use std::sync::{Arc, Mutex};

pub const MAX_SCOPES: u32 = 32;
const RING: usize = 3;
/// Readback buffers while a trace records: a frame's slot is reused only once read, and in a
/// window the GPU may run a few frames behind, so the usual three would drop frames.
const TRACE_RING: usize = 8;

/// Both native Metal and MoltenVK expose Metal counter samples before they are ready when
/// resolve is encoded in the sampling submission. Other Vulkan drivers keep the immediate path.
pub(crate) fn deferred_timestamps(info: &wgpu::AdapterInfo) -> bool {
    info.backend == wgpu::Backend::Metal
        || (info.backend == wgpu::Backend::Vulkan && info.driver.eq_ignore_ascii_case("MoltenVK"))
}

struct Queries {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
}

struct Slot {
    queries: Option<Queries>,
    readback: wgpu::Buffer,
    labels: Vec<&'static str>,
    /// The frame whose timings it holds ([`GpuProfiler::frame`] when it was encoded).
    frame: u64,
    recorded: bool,
    state: Arc<Mutex<SlotState>>,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum SlotState {
    #[default]
    Free,
    Encoded,
    Submitted,
    ResolveReady,
    Mapping,
    Ready,
    Failed,
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
    device: wgpu::Device,
    queue: wgpu::Queue,
    enabled: bool,
    deferred: bool,
    ring: Vec<Slot>,
    /// The frame being encoded: frames submitted before it.
    frame: u64,
    labels: Vec<&'static str>,
    skipped: bool,
    recording: bool,
    period_ns: f32,
    /// The latest complete frame's timings: (pass, milliseconds).
    pub last: Vec<(&'static str, f32)>,
    /// The frames read back since the latest `after_submit` began (by it, and by a `drain` after
    /// it), oldest first; `last` is the newest's. The depth prepass's auto mode takes them at the
    /// next frame (prepass.rs).
    pub arrived: Vec<Timings>,
    /// While a trace records: the frames read back since it was last taken.
    raw: Option<Vec<RawFrame>>,
    /// Recorded frames missed because their slot was busy or mapping failed. Frames outside
    /// the trace window do not change this count (the trace resets it when it starts).
    pub dropped: u64,
}

fn new_slot(device: &wgpu::Device, enabled: bool) -> Slot {
    Slot {
        queries: enabled.then(|| Queries {
            set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("pass timings"),
                ty: wgpu::QueryType::Timestamp,
                count: MAX_SCOPES * 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pass timings (resolve)"),
                size: u64::from(MAX_SCOPES) * 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
        }),
        readback: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pass timings (readback)"),
            size: u64::from(MAX_SCOPES) * 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        labels: Vec::new(),
        frame: 0,
        recorded: false,
        state: Arc::new(Mutex::new(SlotState::Free)),
    }
}

fn map_slot(slot: &Slot) {
    let state = slot.state.clone();
    *state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Mapping;
    slot.readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            *state.lock().unwrap_or_else(|p| p.into_inner()) = if result.is_ok() {
                SlotState::Ready
            } else {
                SlotState::Failed
            };
        });
}

impl GpuProfiler {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enabled: bool,
        info: &wgpu::AdapterInfo,
    ) -> GpuProfiler {
        GpuProfiler {
            device: device.clone(),
            queue: queue.clone(),
            enabled,
            deferred: deferred_timestamps(info),
            ring: (0..RING).map(|_| new_slot(device, enabled)).collect(),
            frame: 0,
            labels: Vec::new(),
            skipped: false,
            recording: false,
            period_ns: queue.get_timestamp_period(),
            last: Vec::new(),
            arrived: Vec::new(),
            raw: None,
            dropped: 0,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
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
        self.enabled
            && *self.slot().state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::Free
    }

    /// The readback buffer of the frame being encoded (the ring grows while a trace records).
    fn slot(&self) -> &Slot {
        &self.ring[self.frame as usize % self.ring.len()]
    }

    /// Whether every frame's raw timestamps are being kept (a trace records).
    pub fn tracing(&self) -> bool {
        self.recording
    }

    /// Starts or stops keeping every frame's raw timestamps; starting also adds readback buffers.
    pub fn set_tracing(&mut self, device: &wgpu::Device, on: bool) {
        self.recording = on;
        self.raw = on.then(Vec::new);
        // Replacing a capture abandons its pending records; stopping a completed window below
        // deliberately keeps them. This also works when a browser cannot synchronously drain.
        for slot in &mut self.ring {
            slot.recorded = false;
        }
        while on && self.enabled() && self.ring.len() < TRACE_RING {
            self.ring.push(new_slot(device, true));
        }
    }

    /// Stops retaining new frames, while pending recorded frames can still arrive.
    pub fn stop_recording(&mut self) {
        self.recording = false;
    }

    /// Selects whether this host frame belongs to the armed capture window. Calibration can
    /// arm the ring before the skipped frames finish; those frames must not retain GPU records.
    pub fn record_frame(&mut self, record: bool) {
        self.recording = record && self.raw.is_some();
    }

    /// The raw frames read back since the last call. A completed capture keeps its late
    /// readbacks until taken, even after new frames stop recording.
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
            query_set: &self.slot().queries.as_ref()?.set,
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
            query_set: &self.slot().queries.as_ref()?.set,
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
            query_set: &self.slot().queries.as_ref()?.set,
            beginning_of_pass_write_index: first.then_some(i * 2),
            end_of_pass_write_index: last.then_some(i * 2 + 1),
        })
    }

    fn next(&mut self, label: &'static str) -> Option<u32> {
        if !self.enabled {
            return None;
        }
        if !self.times_frame() {
            self.skipped = true;
            return None;
        }
        let i = self.labels.len() as u32;
        if i >= MAX_SCOPES {
            return None;
        }
        self.labels.push(label);
        Some(i)
    }

    /// Finishes this frame's query list. Metal resolves after sampling completion; other
    /// backends resolve in the sampling encoder. A busy slot is never overwritten.
    pub fn resolve(&mut self, enc: &mut wgpu::CommandEncoder) {
        let labels = std::mem::take(&mut self.labels);
        if std::mem::take(&mut self.skipped) && self.recording {
            self.dropped += 1;
        }
        if labels.is_empty() {
            return;
        }
        let idx = self.frame as usize % self.ring.len();
        let slot = &mut self.ring[idx];
        debug_assert!(*slot.state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::Free);
        slot.labels = labels;
        slot.frame = self.frame;
        slot.recorded = self.recording;
        if !self.deferred {
            Self::encode_resolve(slot, enc);
        }
        *slot.state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Encoded;
    }

    fn encode_resolve(slot: &Slot, enc: &mut wgpu::CommandEncoder) {
        let queries = slot.queries.as_ref().expect("a timestamped slot");
        let n = slot.labels.len() as u32 * 2;
        enc.resolve_query_set(&queries.set, 0..n, &queries.resolve, 0);
        enc.copy_buffer_to_buffer(&queries.resolve, 0, &slot.readback, 0, u64::from(n) * 8);
    }

    /// Called after the sampling submission. The completion callback only changes a flag:
    /// no queue/device handles are retained in it, and it cannot re-enter queue submission.
    pub fn after_submit(&mut self) {
        self.arrived.clear();
        let idx = self.frame as usize % self.ring.len();
        self.frame += 1;
        let slot = &self.ring[idx];
        let encoded = *slot.state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::Encoded;
        if encoded {
            if self.deferred {
                let state = slot.state.clone();
                *state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Submitted;
                self.queue.on_submitted_work_done(move || {
                    *state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::ResolveReady;
                });
            } else {
                map_slot(slot);
            }
        }
        self.resolve_completed();
        self.collect();
    }

    /// Services completed samples before choosing a slot for the next frame. Poll is
    /// nonblocking; measured frames never wait for the GPU or for a readback.
    pub fn poll(&mut self) {
        if !self.enabled {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.resolve_completed();
        self.collect();
    }

    fn resolve_completed(&mut self) {
        let ready: Vec<usize> = self
            .ring
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| {
                (*slot.state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::ResolveReady)
                    .then_some(i)
            })
            .collect();
        if ready.is_empty() {
            return;
        }
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("completed pass timings"),
            });
        for &i in &ready {
            Self::encode_resolve(&self.ring[i], &mut enc);
        }
        self.queue.submit([enc.finish()]);
        for i in ready {
            map_slot(&self.ring[i]);
        }
    }

    /// Completes sampling, deferred resolves and mapping, even with no more rendering.
    /// A single wait cannot cover resolve submissions created by that wait's callbacks.
    pub fn drain(&mut self, device: &wgpu::Device) {
        if !self.enabled {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        loop {
            if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
                for slot in &self.ring {
                    let mut state = slot.state.lock().unwrap_or_else(|p| p.into_inner());
                    if !matches!(*state, SlotState::Free | SlotState::Ready) {
                        *state = SlotState::Failed;
                    }
                }
            }
            self.resolve_completed();
            self.collect();
            if self.ring.iter().all(|slot| {
                *slot.state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::Free
            }) {
                break;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = device;
            // Browser callbacks progress through its event loop, never a synchronous wait.
            self.collect();
        }
    }

    /// Reads every buffer whose mapping finished into `arrived`, `last` and, while tracing, the
    /// raw frames.
    fn collect(&mut self) {
        // The ring is not in frame order: several buffers can finish between submits.
        let mut order: Vec<usize> = (0..self.ring.len()).collect();
        order.sort_by_key(|&i| self.ring[i].frame);
        for i in order {
            let slot = &mut self.ring[i];
            let state = *slot.state.lock().unwrap_or_else(|p| p.into_inner());
            if state == SlotState::Failed {
                if slot.recorded {
                    self.dropped += 1;
                }
                slot.labels.clear();
                *slot.state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Free;
                continue;
            }
            if state != SlotState::Ready {
                continue;
            }
            {
                let Ok(data) = slot.readback.slice(..).get_mapped_range() else {
                    *slot.state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Failed;
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
                if slot.recorded
                    && let Some(raw) = &mut self.raw
                {
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
