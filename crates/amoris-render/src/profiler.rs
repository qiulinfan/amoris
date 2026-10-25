//! GPU timings per pass with timestamp queries, read back a few frames late without stalling (a
//! ring of readback buffers; a frame that finds its buffer still busy is not timed). Each reading
//! names the frame it measured. On adapters without timestamps every timing is absent.
//!
//! While a frame trace records (trace.rs), every frame's raw timestamps are kept as well, with the
//! profiler frame they belong to, instead of only the latest frame's durations. Each in-flight
//! slot owns its range of the profiler's one query set until readback completes ([`QUERIES`]).
//! Metal, MoltenVK and Vulkan on AMD adapters resolve only after the sampling submission
//! completes: resolving in that submission returned zero or earlier values there (on AMD's Vulkan
//! a driver fault, [`deferred_timestamps`]).

use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The passes a frame can time. A water scene with its spray, medium tiles and the liquid surface
/// timed takes over 32; past this the later passes (TAA, post) go untimed.
pub const MAX_SCOPES: u32 = 64;
const RING: usize = 3;
/// Readback buffers while a trace records: a frame's slot is reused only once read, and in a
/// window the GPU may run a few frames behind, so the usual three would drop frames.
const TRACE_RING: usize = 8;
/// The profiler's timestamp queries, in one query set that every slot of the ring (up to
/// [`TRACE_RING`]) takes `MAX_SCOPES * 2` of. One set, not one a slot: Metal gives a process 32
/// counter sample buffers (one a timestamp query set) across all its devices, and creating the 33rd
/// fails ("Cannot allocate sample buffer"), which wgpu 30 reports as an unexpected device error and
/// so loses that device. With a set a slot, three a renderer, the eleventh renderer alive in a
/// process lost its device, as one of a test binary's parallel cases did (M5, 2026-10-25).
const QUERIES: u32 = TRACE_RING as u32 * MAX_SCOPES * 2;
/// The counter sample buffers native Metal gives a process ([`QUERIES`]), one a timestamp query
/// set (MoltenVK created 64 sets without loss: docs/bench/metal-profiling.md).
const METAL_SAMPLE_BUFFERS: usize = 32;
/// The timestamp query sets alive on native Metal in this process: past [`METAL_SAMPLE_BUFFERS`] a
/// new one is not created, and its owner goes without timestamps instead of losing its device.
static METAL_SETS: AtomicUsize = AtomicUsize::new(0);

/// Takes one of `cap` places in `live`, if one is free.
fn reserve(live: &AtomicUsize, cap: usize) -> bool {
    live.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
        (n < cap).then_some(n + 1)
    })
    .is_ok()
}

/// Whether timestamps are resolved only after the submission that sampled them completed (charter,
/// 2026-10-10 and its Pioneer note of 2026-10-15). Native Metal and MoltenVK expose Metal counter
/// samples before they are ready when resolve is encoded in the sampling submission. Through wgpu
/// 30 on the Radeon 780M with AMD's Vulkan driver 24.30.62.03, a resolve in the sampling command
/// buffer read zeros for a fresh query set's first submission and earlier values afterwards, while
/// a resolve after that submission completed read none (docs/bench/gpu-matrix.md 3 a). The driver
/// is at fault (gpu-matrix.md 8, 2026-10-16): wgpu's commands pass the Khronos validation layer
/// with synchronization validation, and a raw Vulkan reproduction (examples/vk_timestamps.rs)
/// shows that a buffer copy following vkCmdCopyQueryPoolResults in the same command buffer, behind
/// the TRANSFER barrier the specification asks for, reads the destination before the query results
/// land in most rounds. wgpu records an immediate resolve and the readback copy of one encoder into
/// one command buffer; the resolve of queries an earlier submission wrote goes into a command
/// buffer of its own (wgpu-core's deferred query resolve), which the driver orders correctly, as it
/// does a readback copy in a second encoder of the same submit (measured, not used). Every Vulkan
/// adapter with AMD's vendor id takes the deferred path (only this driver was measured; the path
/// is correct everywhere, and readings only arrive later). NVIDIA's Vulkan driver and Direct3D 12
/// on both GPUs read correct values in the same submission and keep the immediate path.
pub(crate) fn deferred_timestamps(info: &wgpu::AdapterInfo) -> bool {
    /// AMD's PCI vendor id.
    const AMD: u32 = 0x1002;
    info.backend == wgpu::Backend::Metal
        || (info.backend == wgpu::Backend::Vulkan
            && (info.driver.eq_ignore_ascii_case("MoltenVK") || info.vendor == AMD))
}

/// A timestamp query set (the profiler's, the path tracers' and the trace clock's) whose release on
/// MoltenVK first waits for its queue to go idle. MoltenVK 1.4.2 marks a command buffer's queries
/// available in a Metal completed handler (`MVKCommandEncoder::finishQueries`) that holds the query
/// pool without retaining it, and its timeline semaphores are MTLEvents the GPU signals before
/// Metal runs that handler. So wgpu's wait for the work can return first, a set dropped then is
/// destroyed, and the handler reads the freed pool: a SIGSEGV in 18 of 30 TAA test runs with
/// parallel cases (M5, 2026-10-25; docs/bench/metal-profiling.md). MoltenVK's queue wait commits an
/// empty command buffer and waits for its completion, which Metal reports only after the handlers
/// of the command buffers before it. Other backends release the set at once. The wait goes once the
/// MoltenVK in use retains the pools (KhronosGroup/MoltenVK c0af3a5658ba, after 1.4.2). A set whose
/// wait fails (a lost device returns at once, before those handlers ran) is leaked, not released.
///
/// On native Metal a set takes one of the process's [`METAL_SAMPLE_BUFFERS`]; with none left
/// [`TimestampSet::new`] gives none.
pub(crate) struct TimestampSet {
    set: ManuallyDrop<wgpu::QuerySet>,
    /// The queue to wait for before the set goes: on MoltenVK only.
    idle: Option<wgpu::Queue>,
    /// Whether it holds one of [`METAL_SETS`].
    metal: bool,
}

impl TimestampSet {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        info: &wgpu::AdapterInfo,
        label: &str,
        count: u32,
    ) -> Option<TimestampSet> {
        let moltenvk =
            info.backend == wgpu::Backend::Vulkan && info.driver.eq_ignore_ascii_case("MoltenVK");
        let metal = info.backend == wgpu::Backend::Metal;
        if metal && !reserve(&METAL_SETS, METAL_SAMPLE_BUFFERS) {
            log::warn!(
                "{label}: no timestamps, the process has the {METAL_SAMPLE_BUFFERS} timestamp \
                 query sets Metal allows"
            );
            return None;
        }
        Some(TimestampSet {
            set: ManuallyDrop::new(device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some(label),
                ty: wgpu::QueryType::Timestamp,
                count,
            })),
            idle: moltenvk.then(|| queue.clone()),
            metal,
        })
    }
}

impl std::ops::Deref for TimestampSet {
    type Target = wgpu::QuerySet;
    fn deref(&self) -> &wgpu::QuerySet {
        &self.set
    }
}

impl Drop for TimestampSet {
    fn drop(&mut self) {
        if let Some(queue) = self.idle.take()
            && !wait_for_idle(&queue)
        {
            // MoltenVK's handlers may still read it: it stays, and keeps its sample buffer.
            return;
        }
        // SAFETY: the set is dropped here only, once.
        unsafe { ManuallyDrop::drop(&mut self.set) };
        if self.metal {
            METAL_SETS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Waits for the queue to go idle; false when the wait failed.
#[cfg(not(target_arch = "wasm32"))]
fn wait_for_idle(queue: &wgpu::Queue) -> bool {
    use wgpu::hal::Queue as _;
    // SAFETY: wgpu-hal asks that the wait not overlap another submit, present or wait on the queue.
    // A device here is driven from one thread, the one that drops its sets.
    let Some(hal) = (unsafe { queue.as_hal::<wgpu::hal::api::Vulkan>() }) else {
        return true;
    };
    match unsafe { hal.wait_for_idle() } {
        Ok(()) => true,
        Err(e) => {
            log::warn!(
                "waiting for the queue before a timestamp query set goes: {e}; it is leaked"
            );
            false
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn wait_for_idle(_: &wgpu::Queue) -> bool {
    true
}

/// The [`QUERIES`] and the buffer they resolve into (8 bytes a query), both shared by the ring.
struct Queries {
    set: TimestampSet,
    resolve: wgpu::Buffer,
}

struct Slot {
    /// Its first query: the slot at ring index `k` owns `MAX_SCOPES * 2` from `k * MAX_SCOPES * 2`.
    first: u32,
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
    /// None without timestamps.
    queries: Option<Queries>,
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
    /// The greatest common divisor of every timestamp read (0 before the first): the timestamps'
    /// resolution in ticks, one tick unless the implementation quantizes them (a browser).
    resolution: u64,
}

fn new_slot(device: &wgpu::Device, index: usize) -> Slot {
    Slot {
        first: index as u32 * MAX_SCOPES * 2,
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
        let queries = enabled
            .then(|| TimestampSet::new(device, queue, info, "pass timings", QUERIES))
            .flatten()
            .map(|set| Queries {
                set,
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pass timings (resolve)"),
                    size: u64::from(QUERIES) * 8,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
            });
        GpuProfiler {
            device: device.clone(),
            queue: queue.clone(),
            // Without a query set (no sample buffer left on Metal) the profiler times nothing.
            enabled: queries.is_some(),
            deferred: deferred_timestamps(info),
            queries,
            ring: (0..RING).map(|i| new_slot(device, i)).collect(),
            frame: 0,
            labels: Vec::new(),
            skipped: false,
            recording: false,
            period_ns: queue.get_timestamp_period(),
            last: Vec::new(),
            arrived: Vec::new(),
            raw: None,
            dropped: 0,
            resolution: 0,
        }
    }

    /// The timestamps' resolution in nanoseconds (the greatest common divisor of every timestamp
    /// read, in ticks, times [`GpuProfiler::period_ns`]); `None` before the first reading. Chrome
    /// quantizes WebGPU's timestamps to 65.536 us without `--enable-unsafe-webgpu`
    /// (docs/bench/prepass.md 6).
    pub fn resolution_ns(&self) -> Option<f32> {
        (self.resolution > 0).then_some(self.resolution as f32 * self.period_ns)
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
            self.ring.push(new_slot(device, self.ring.len()));
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
        let i = self.slot().first + self.next(label)? * 2;
        Some(wgpu::ComputePassTimestampWrites {
            query_set: &self.queries.as_ref()?.set,
            beginning_of_pass_write_index: Some(i),
            end_of_pass_write_index: Some(i + 1),
        })
    }

    pub fn render_scope(
        &mut self,
        label: &'static str,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let i = self.slot().first + self.next(label)? * 2;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries.as_ref()?.set,
            beginning_of_pass_write_index: Some(i),
            end_of_pass_write_index: Some(i + 1),
        })
    }

    /// A scope over several consecutive render passes, named `label`: give the index to
    /// [`GpuProfiler::span_writes`] for each of them. `None` when disabled or out of scopes.
    /// (From branch `explore/ue5`'s `AMORIS_TIME_POST`.)
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
        let i = self.slot().first + i? * 2;
        if !first && !last {
            return None;
        }
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries.as_ref()?.set,
            beginning_of_pass_write_index: first.then_some(i),
            end_of_pass_write_index: last.then_some(i + 1),
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

    /// Finishes this frame's query list. Where [`deferred_timestamps`] holds (Metal, MoltenVK,
    /// Vulkan on AMD adapters) the queries resolve after the sampling submission completes; other
    /// adapters resolve in the sampling encoder. A busy slot is never overwritten.
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
            Self::encode_resolve(self.queries.as_ref(), slot, enc);
        }
        *slot.state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Encoded;
    }

    fn encode_resolve(queries: Option<&Queries>, slot: &Slot, enc: &mut wgpu::CommandEncoder) {
        let queries = queries.expect("a timestamped slot");
        let n = slot.labels.len() as u32 * 2;
        let (range, at) = (slot.first..slot.first + n, u64::from(slot.first) * 8);
        enc.resolve_query_set(&queries.set, range, &queries.resolve, at);
        enc.copy_buffer_to_buffer(&queries.resolve, at, &slot.readback, 0, u64::from(n) * 8);
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
            Self::encode_resolve(self.queries.as_ref(), &self.ring[i], &mut enc);
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
                for &t in &ticks {
                    self.resolution = gcd(self.resolution, t);
                }
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

/// Euclid's greatest common divisor (`gcd(0, t) == t`).
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Metal's sample buffers are counted out to the cap and back: past it no place is taken, so an
    /// owner goes without timestamps, and a place freed is taken again.
    #[test]
    fn metal_query_sets_stop_at_the_sample_buffers() {
        let live = AtomicUsize::new(0);
        for _ in 0..METAL_SAMPLE_BUFFERS {
            assert!(reserve(&live, METAL_SAMPLE_BUFFERS));
        }
        assert!(!reserve(&live, METAL_SAMPLE_BUFFERS));
        assert_eq!(live.load(Ordering::Acquire), METAL_SAMPLE_BUFFERS);
        live.fetch_sub(1, Ordering::AcqRel);
        assert!(reserve(&live, METAL_SAMPLE_BUFFERS));
        assert!(!reserve(&live, METAL_SAMPLE_BUFFERS));
    }

    /// Which adapters resolve timestamps after their submission: Metal, MoltenVK and Vulkan on AMD
    /// adapters (on the Radeon 780M `gpu_profiler` and the trace's clock test fail with the
    /// immediate path); NVIDIA's Vulkan driver and Direct3D 12 keep it.
    #[test]
    fn timestamps_resolve_late_where_early_resolves_read_stale_values() {
        use wgpu::{Backend, DeviceType};
        let info = |backend, vendor: u32, driver: &str| {
            let mut i = wgpu::AdapterInfo::new(DeviceType::IntegratedGpu, backend);
            i.vendor = vendor;
            i.driver = driver.into();
            i
        };
        for (backend, vendor, driver, deferred) in [
            (Backend::Metal, 0x106b, "", true),
            (Backend::Vulkan, 0x106b, "MoltenVK", true),
            (Backend::Vulkan, 0x1002, "AMD proprietary driver", true),
            (Backend::Dx12, 0x1002, "", false),
            (Backend::Vulkan, 0x10de, "NVIDIA", false),
            (Backend::Dx12, 0x10de, "", false),
        ] {
            assert_eq!(
                deferred_timestamps(&info(backend, vendor, driver)),
                deferred,
                "{backend:?} {vendor:#x} {driver}"
            );
        }
    }
}
