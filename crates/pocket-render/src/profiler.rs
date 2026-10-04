//! GPU timings per pass with timestamp queries, read back a few frames late without stalling (a
//! ring of readback buffers). On adapters without timestamps every timing is absent.

use std::sync::{Arc, Mutex};

pub const MAX_SCOPES: u32 = 32;
const RING: usize = 3;

struct Slot {
    readback: wgpu::Buffer,
    labels: Vec<&'static str>,
    state: Arc<Mutex<SlotState>>,
}

#[derive(Default, PartialEq)]
enum SlotState {
    #[default]
    Free,
    Mapping,
    Ready,
}

pub struct GpuProfiler {
    queries: Option<wgpu::QuerySet>,
    resolve: Option<wgpu::Buffer>,
    ring: Vec<Slot>,
    frame: usize,
    labels: Vec<&'static str>,
    period_ns: f32,
    /// The latest complete frame's timings: (pass, milliseconds).
    pub last: Vec<(&'static str, f32)>,
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
        let ring = (0..RING)
            .map(|_| Slot {
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pass timings (readback)"),
                    size: u64::from(MAX_SCOPES) * 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                labels: Vec::new(),
                state: Arc::new(Mutex::new(SlotState::Free)),
            })
            .collect();
        GpuProfiler {
            queries,
            resolve,
            ring,
            frame: 0,
            labels: Vec::new(),
            period_ns: queue.get_timestamp_period(),
            last: Vec::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.queries.is_some()
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
        let slot = &mut self.ring[self.frame % RING];
        if *slot.state.lock().unwrap_or_else(|p| p.into_inner()) != SlotState::Free {
            return;
        }
        let n = labels.len() as u32 * 2;
        enc.resolve_query_set(q, 0..n, r, 0);
        enc.copy_buffer_to_buffer(r, 0, &slot.readback, 0, u64::from(n) * 8);
        slot.labels = labels;
    }

    /// After submitting: starts mapping the buffer just filled and collects any finished one.
    pub fn after_submit(&mut self) {
        if self.queries.is_none() {
            return;
        }
        let idx = self.frame % RING;
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
        for slot in &mut self.ring {
            let ready = *slot.state.lock().unwrap_or_else(|p| p.into_inner()) == SlotState::Ready;
            if !ready {
                continue;
            }
            {
                let Ok(data) = slot.readback.slice(..).get_mapped_range() else {
                    continue;
                };
                let ticks: &[u64] = bytemuck::cast_slice(&data[..slot.labels.len() * 16]);
                self.last = slot
                    .labels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| {
                        let d = ticks[i * 2 + 1].saturating_sub(ticks[i * 2]);
                        (*l, d as f32 * self.period_ns / 1e6)
                    })
                    .collect();
            }
            slot.readback.unmap();
            slot.labels.clear();
            *slot.state.lock().unwrap_or_else(|p| p.into_inner()) = SlotState::Free;
        }
    }

    pub fn total_ms(&self) -> f32 {
        self.last.iter().map(|(_, ms)| ms).sum()
    }
}
