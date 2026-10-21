use crate::water_compute::GpuWater;
use amoris_assets::water_field::ColumnDelta;

#[derive(Debug, PartialEq, Eq)]
struct EditCapacity {
    entries: usize,
    runs_bytes: u64,
    changes_bytes: u64,
}

/// Preserve the original power-of-two allocations and whole-buffer bindings. Counts also bound
/// every packed run's start/length and the shader's u32 indices, before CPU grouping allocates.
fn edit_capacity(count: usize, limits: &wgpu::Limits) -> Result<EditCapacity, String> {
    u32::try_from(count).map_err(|_| "resident water edit count exceeds GPU index range")?;
    let entries = if count == 0 {
        0
    } else {
        count
            .checked_next_power_of_two()
            .ok_or("resident water edit capacity overflowed")?
    };
    let bytes = |stride| {
        entries
            .checked_mul(stride)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or("resident water edit buffer size overflowed")
    };
    let runs_bytes = bytes(16)?;
    let changes_bytes = bytes(4)?;
    if [runs_bytes, changes_bytes]
        .iter()
        .any(|&n| n > limits.max_buffer_size)
    {
        return Err("resident water edits exceed the device buffer size limit".into());
    }
    if [runs_bytes, changes_bytes]
        .iter()
        .any(|&n| n > limits.max_storage_buffer_binding_size)
    {
        return Err("resident water edits exceed the device storage binding size limit".into());
    }
    Ok(EditCapacity {
        entries,
        runs_bytes,
        changes_bytes,
    })
}

pub(crate) struct WaterEdits {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
    receipt: wgpu::Buffer,
    has_receipt: bool,
    params: wgpu::Buffer,
    runs: wgpu::Buffer,
    changes: wgpu::Buffer,
    capacity: usize,
}
impl WaterEdits {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("resident water edits"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water_edits.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("resident water edits"),
            layout: None,
            module: &shader,
            entry_point: Some("apply_edits"),
            compilation_options: crate::shaders::compute_options(),
            cache: None,
        });
        let reduce = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("water edits receipt"),
            layout: None,
            module: &shader,
            entry_point: Some("report_edits"),
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
        Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            reduce,
            receipt: buffer(
                "water edits receipt",
                16,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            ),
            has_receipt: false,
            params: buffer(
                "water edit parameters",
                32,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            runs: buffer(
                "water edit runs",
                16,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            changes: buffer(
                "water edit changes",
                4,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            ),
            capacity: 1,
        }
    }
    #[allow(clippy::cast_possible_truncation)] // The explicit GPU step stores f32 depth changes.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        field: &GpuWater,
        edits: &[ColumnDelta],
    ) -> Result<(), String> {
        self.has_receipt = false;
        if edits.is_empty() {
            return Ok(());
        }
        let capacity = edit_capacity(edits.len(), &self.device.limits())?;
        let n = field.size()[0] * field.size()[1];
        let mut grouped = std::collections::BTreeMap::<u32, Vec<f32>>::new();
        for e in edits {
            let delta = e.depth_m as f32;
            if e.column >= n || !delta.is_finite() {
                return Err("resident water edit is outside the field or f32 range".into());
            }
            grouped.entry(e.column).or_default().push(delta);
        }
        if edits.len() > self.capacity {
            let buffer = |label, size| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            self.runs = buffer("water edit runs", capacity.runs_bytes);
            self.changes = buffer("water edit changes", capacity.changes_bytes);
            self.capacity = capacity.entries;
        }
        let mut runs = Vec::with_capacity(grouped.len());
        let mut changes = Vec::with_capacity(edits.len());
        for (column, values) in grouped {
            runs.push([column, changes.len() as u32, values.len() as u32, 0]);
            changes.extend(values);
        }
        self.queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[runs.len() as u32, 0, 0, 0, 0, 0, 0, 0]),
        );
        self.queue
            .write_buffer(&self.runs, 0, bytemuck::cast_slice(&runs));
        self.queue
            .write_buffer(&self.changes, 0, bytemuck::cast_slice(&changes));
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water depth edits"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[&self.params, field.buffer(), &self.runs, &self.changes]
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect::<Vec<_>>(),
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water depth edits"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((runs.len() as u32).div_ceil(64), 1, 1);
        drop(pass);
        let receipt_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water edits receipt"),
            layout: &self.reduce.get_bind_group_layout(0),
            entries: &[
                (0, &self.params),
                (1, field.buffer()),
                (2, &self.runs),
                (4, &self.receipt),
            ]
            .iter()
            .map(|(binding, buffer)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: buffer.as_entire_binding(),
            })
            .collect::<Vec<_>>(),
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water edits receipt"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.reduce);
        pass.set_bind_group(0, &receipt_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        self.has_receipt = true;
        Ok(())
    }
    pub fn copy_receipt(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Buffer,
        offset: u64,
    ) -> Result<(), String> {
        if !self.has_receipt {
            return Err("no water edit receipt for this step".into());
        }
        encoder.copy_buffer_to_buffer(&self.receipt, 0, target, offset, 16);
        Ok(())
    }
    pub fn decode_receipt(bytes: &[u8]) -> Result<f32, String> {
        if bytes.len() != 16 {
            return Err("water edit receipt must have 16 bytes".into());
        }
        let v: [f32; 4] = bytemuck::pod_read_unaligned(bytes);
        if v.iter().any(|v| !v.is_finite()) || v[1] != 0.0 || v[2] != 0.0 || v[3] != 0.0 {
            return Err("invalid water edit receipt".into());
        }
        Ok(v[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_capacity_accepts_zero_and_the_inclusive_default_binding_boundary() {
        let limits = wgpu::Limits::default();
        assert_eq!(
            edit_capacity(0, &limits).unwrap(),
            EditCapacity {
                entries: 0,
                runs_bytes: 0,
                changes_bytes: 0
            }
        );
        let count = usize::try_from(limits.max_storage_buffer_binding_size / 16).unwrap();
        assert!(count.is_power_of_two());
        assert_eq!(
            edit_capacity(count, &limits).unwrap(),
            EditCapacity {
                entries: count,
                runs_bytes: limits.max_storage_buffer_binding_size,
                changes_bytes: limits.max_storage_buffer_binding_size / 4,
            }
        );
        assert!(
            edit_capacity(count + 1, &limits)
                .unwrap_err()
                .contains("binding")
        );
    }

    #[test]
    fn edit_capacity_checks_rounded_allocation_against_buffer_and_binding_limits() {
        let mut limits = wgpu::Limits {
            max_buffer_size: 64,
            max_storage_buffer_binding_size: 64,
            ..Default::default()
        };
        assert_eq!(edit_capacity(3, &limits).unwrap().entries, 4);
        assert_eq!(edit_capacity(4, &limits).unwrap().runs_bytes, 64);
        assert!(edit_capacity(5, &limits).unwrap_err().contains("buffer"));
        limits.max_buffer_size = 48;
        assert!(edit_capacity(3, &limits).unwrap_err().contains("buffer"));
        limits.max_buffer_size = 64;
        limits.max_storage_buffer_binding_size = 48;
        assert!(edit_capacity(3, &limits).unwrap_err().contains("binding"));
    }

    #[test]
    fn edit_capacity_rejects_unrepresentable_counts_without_allocating_or_overflowing() {
        let limits = wgpu::Limits {
            max_buffer_size: u64::MAX,
            max_storage_buffer_binding_size: u64::MAX,
            ..Default::default()
        };
        assert!(edit_capacity(usize::MAX, &limits).is_err());
        #[cfg(target_pointer_width = "64")]
        {
            let largest = u32::MAX as usize;
            assert_eq!(
                edit_capacity(largest, &limits).unwrap().entries,
                largest + 1
            );
            assert!(
                edit_capacity(largest + 1, &limits)
                    .unwrap_err()
                    .contains("index")
            );
        }
        #[cfg(target_pointer_width = "32")]
        assert!(edit_capacity(1usize << 31, &limits).is_err());
    }
}
