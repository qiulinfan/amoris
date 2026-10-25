//! Bounded asynchronous point readback from a resident GPU water field. No field download or
//! device wait: native callers poll their device; WebGPU completes maps through its event loop.
//! A result's epoch/tick names the sampled state, never the state when its callback arrived.
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use crate::water_compute::GpuWater;

/// f32 words a point returns: [surface, depth, vx, vz] and [rise x, rise z, inflow, 0].
pub const SAMPLE_WORDS: usize = 8;
const SAMPLE_BYTES: u64 = SAMPLE_WORDS as u64 * 4;

/// The timeline and boundary assigned by the host. An epoch changes on reset, restore or branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleStamp {
    pub epoch: u64,
    pub tick: u64,
}

#[derive(Debug)]
pub struct WaterSampleResult {
    /// Matches the token returned by `request`, including several batches at the same tick.
    pub request_id: u64,
    pub stamp: SampleStamp,
    /// Surface, depth, centred x/z velocity, then surface rise per column along x/z and net
    /// inflow (m^2/s); one result per point in request order. Surface is the free surface,
    /// relative to the datum used when the field was uploaded.
    pub columns: Result<Vec<[f32; SAMPLE_WORDS]>, String>,
}

enum Status {
    Idle,
    Encoded {
        request_id: u64,
        stamp: SampleStamp,
        count: usize,
    },
    Mapping {
        request_id: u64,
        stamp: SampleStamp,
        count: usize,
        done: Receiver<Result<(), wgpu::BufferAsyncError>>,
    },
}
struct Slot {
    params: wgpu::Buffer,
    points: wgpu::Buffer,
    output: wgpu::Buffer,
    staging: wgpu::Buffer,
    group: Option<([wgpu::Buffer; 2], wgpu::BindGroup)>,
    status: Status,
}

/// Three independently mapped slots bound transfer memory and prevent an unbounded request queue.
pub struct WaterSamples {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    slots: [Slot; 3],
    capacity: usize,
    next_request: u64,
}

impl WaterSamples {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        capacity: usize,
    ) -> Result<Self, String> {
        let bytes = u64::try_from(capacity)
            .ok()
            .and_then(|n| n.checked_mul(SAMPLE_BYTES));
        let Some(bytes) =
            bytes.filter(|&n| n > 0 && n <= device.limits().max_storage_buffer_binding_size)
        else {
            return Err("water sample capacity exceeds the device's storage binding limit".into());
        };
        if capacity.div_ceil(64) > device.limits().max_compute_workgroups_per_dimension as usize {
            return Err("water sample capacity exceeds the dispatch limit".into());
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water point samples"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/det/water_samples.wgsl").into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water point samples"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: crate::shaders::compute_options(),
            cache: None,
        });
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let slots = std::array::from_fn(|_| Slot {
            params: buffer(
                "water sample params",
                16,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            points: buffer(
                "water sample positions",
                bytes / 4,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            output: buffer(
                "water sample output",
                bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            staging: buffer(
                "water sample staging",
                bytes,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            ),
            group: None,
            status: Status::Idle,
        });
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            slots,
            capacity,
            next_request: 0,
        })
    }

    /// Encode after the field's substeps, before its next tick. Positions are local grid units
    /// (metres from the origin divided by cell_m). Finite out-of-bounds points clamp at the edge.
    /// Returns a unique request token, or None when all slots are busy without changing the encoder.
    pub fn request(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
        points: &[[f32; 2]],
        stamp: SampleStamp,
    ) -> Result<Option<u64>, String> {
        self.request_buoyancy(encoder, field, points, stamp, u32::MAX)
    }
    pub(crate) fn request_buoyancy(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
        points: &[[f32; 2]],
        stamp: SampleStamp,
        body_start: u32,
    ) -> Result<Option<u64>, String> {
        if points.is_empty()
            || points.len() > self.capacity
            || points.iter().flatten().any(|p| !p.is_finite())
            || (body_start != u32::MAX && body_start as usize > points.len())
        {
            return Err(
                "water sample points must be finite and fit the nonempty batch capacity".into(),
            );
        }
        if !field.is_initialized() {
            return Err("GPU water has not been uploaded".into());
        }
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| matches!(s.status, Status::Idle))
        else {
            return Ok(None);
        };
        let request_id = self.next_request;
        self.next_request = request_id
            .checked_add(1)
            .ok_or("water sample request tokens exhausted")?;
        if slot
            .group
            .as_ref()
            .is_none_or(|(b, _)| &b[0] != field.buffer() || &b[1] != field.occupancy_buffer())
        {
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water sample field"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    &slot.params,
                    field.buffer(),
                    &slot.points,
                    &slot.output,
                    field.occupancy_buffer(),
                ]
                .iter()
                .enumerate()
                .map(|(n, b)| wgpu::BindGroupEntry {
                    binding: n as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
            });
            slot.group = Some((
                [field.buffer().clone(), field.occupancy_buffer().clone()],
                group,
            ));
        }
        let [nx, nz] = field.size();
        self.queue.write_buffer(
            &slot.params,
            0,
            bytemuck::cast_slice(&[nx, nz, points.len() as u32, body_start]),
        );
        self.queue
            .write_buffer(&slot.points, 0, bytemuck::cast_slice(points));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("water samples"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &slot.group.as_ref().unwrap().1, &[]);
            pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &slot.output,
            0,
            &slot.staging,
            0,
            points.len() as u64 * SAMPLE_BYTES,
        );
        slot.status = Status::Encoded {
            request_id,
            stamp,
            count: points.len(),
        };
        Ok(Some(request_id))
    }

    /// Copy one encoded batch into combined readback storage. Do not call after_submit for a
    /// batch consumed this way; clear its slot after the combined submission has completed.
    pub fn copy_to(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        request_id: u64,
        target: &wgpu::Buffer,
        offset: u64,
    ) -> Result<(), String> {
        let slot = self
            .slots
            .iter()
            .find(|s| matches!(s.status,Status::Encoded {request_id:id,..} if id==request_id))
            .ok_or("water sample request is not encoded")?;
        let Status::Encoded { count, .. } = slot.status else {
            unreachable!()
        };
        encoder.copy_buffer_to_buffer(&slot.output, 0, target, offset, count as u64 * SAMPLE_BYTES);
        Ok(())
    }

    /// Call after submitting the encoder containing all requests. Mapping before submission
    /// would make those copies invalid; this method performs no polling or blocking waits.
    pub fn after_submit(&mut self) {
        for slot in &mut self.slots {
            if let Status::Encoded {
                request_id,
                stamp,
                count,
            } = slot.status
            {
                let (tx, done) = channel();
                slot.status = Status::Mapping {
                    request_id,
                    stamp,
                    count,
                    done,
                };
                slot.staging.slice(..count as u64 * SAMPLE_BYTES).map_async(
                    wgpu::MapMode::Read,
                    move |result| {
                        let _ = tx.send(result);
                    },
                );
            }
        }
    }

    /// Consume completed batches. Slot order need not be tick order; the stamp is authoritative.
    pub fn take_ready(&mut self) -> Vec<WaterSampleResult> {
        let mut ready = Vec::new();
        for slot in &mut self.slots {
            let Status::Mapping {
                request_id,
                stamp,
                count,
                done,
            } = &slot.status
            else {
                continue;
            };
            let completion = match done.try_recv() {
                Ok(result) => result.map_err(|e| format!("water sample map failed: {e}")),
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => Err("water sample callback was dropped".into()),
            };
            let stamp = *stamp;
            let request_id = *request_id;
            let columns = completion.and_then(|()| {
                let range = slot
                    .staging
                    .slice(..*count as u64 * SAMPLE_BYTES)
                    .get_mapped_range()
                    .map_err(|e| format!("water sample range failed: {e}"))?;
                let columns: Vec<[f32; SAMPLE_WORDS]> = bytemuck::cast_slice(&range).to_vec();
                if columns.iter().flatten().any(|v| !v.is_finite()) {
                    return Err("non-finite water sample".into());
                }
                Ok(columns)
            });
            slot.staging.unmap();
            slot.status = Status::Idle;
            ready.push(WaterSampleResult {
                request_id,
                stamp,
                columns,
            });
        }
        ready
    }

    /// Drop old timeline results on restore/reset. Each map has its own channel, so a late
    /// cancellation callback cannot complete a request that subsequently reuses the slot.
    /// Submit or discard all request encoders before clearing their slots.
    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            if matches!(slot.status, Status::Mapping { .. }) {
                slot.staging.unmap();
            }
            slot.status = Status::Idle;
        }
    }
}
