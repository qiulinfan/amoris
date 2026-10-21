//! Patch-major dense indices emitted directly from selected templates. A key-preserving frame keeps
//! the previous list; current field heights and morph weights remain in the original GPU sources.
use crate::shaders;

pub(crate) struct Pipelines {
    local: wgpu::BindGroupLayout,
    prepare_local: wgpu::BindGroupLayout,
    prepare: wgpu::ComputePipeline,
    count: wgpu::ComputePipeline,
    scan: wgpu::ComputePipeline,
    emit: wgpu::ComputePipeline,
}
pub(crate) struct Compact {
    scratch: wgpu::Buffer,
    dispatch: wgpu::Buffer,
    spec: wgpu::Buffer,
    local: wgpu::BindGroup,
    prepare_local: wgpu::BindGroup,
}
fn entry(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: b.as_entire_binding(),
    }
}
fn buffer(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
impl Pipelines {
    pub fn new(device: &wgpu::Device) -> Self {
        let storage = |read_only| wgpu::BufferBindingType::Storage { read_only };
        let local = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water compact metadata and output"),
            entries: &[
                buffer(0, storage(false)),
                buffer(1, storage(false)),
                buffer(2, storage(true)),
                buffer(3, storage(true)),
                buffer(4, storage(true)),
                buffer(5, storage(false)),
                buffer(6, wgpu::BufferBindingType::Uniform),
            ],
        });
        let prepare_local = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water compact dispatch writer"),
            entries: &[buffer(0, storage(false))],
        });
        let prepare_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water compact prepare"),
            bind_group_layouts: &[Some(&local), Some(&prepare_local)],
            immediate_size: 0,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water compact"),
            bind_group_layouts: &[Some(&local)],
            immediate_size: 0,
        });
        let module = shaders::module(device, "water_compact");
        let pipeline = |name, layout: &wgpu::PipelineLayout| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: Some(layout),
                module: &module,
                entry_point: Some(name),
                compilation_options: shaders::compute_options(),
                cache: None,
            })
        };
        let prepare = pipeline("prepare", &prepare_layout);
        let count = pipeline("count", &layout);
        let scan = pipeline("scan", &layout);
        let emit = pipeline("emit", &layout);
        Self {
            local,
            prepare_local,
            prepare,
            count,
            scan,
            emit,
        }
    }
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        compact: &Compact,
        profiler: &mut crate::profiler::GpuProfiler,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("water compact"),
            timestamp_writes: profiler.compute_scope("water compact"),
        });
        pass.set_bind_group(0, &compact.local, &[]);
        pass.set_bind_group(1, &compact.prepare_local, &[]);
        pass.set_pipeline(&self.prepare);
        pass.dispatch_workgroups(1, 1, 1);
        // Neither indirect consumer's layout contains the writable dispatch buffer.
        pass.set_bind_group(1, None, &[]);
        pass.set_pipeline(&self.count);
        pass.dispatch_workgroups_indirect(&compact.dispatch, 0);
        pass.set_pipeline(&self.scan);
        pass.dispatch_workgroups_indirect(&compact.dispatch, 16);
        pass.set_pipeline(&self.emit);
        pass.dispatch_workgroups_indirect(&compact.dispatch, 32);
    }
}
impl Compact {
    pub fn new(
        device: &wgpu::Device,
        pipelines: &Pipelines,
        inputs: [&wgpu::Buffer; 5],
        spec: [u32; 4],
    ) -> Self {
        let [indices, args, mask, templates, geometry] = inputs;
        let limits = device.limits();
        let words = indices.size() / 4;
        assert!(
            words <= u64::from(u32::MAX),
            "water compact index count exceeds u32"
        );
        let [nx, nz, blocky, per] = spec;
        let units = if blocky != 0 {
            [nx / per.max(1), nz / per.max(1)]
        } else {
            [nx - 1, nz - 1]
        };
        let patches = u64::from(units[0].div_ceil(16)) * u64::from(units[1].div_ceil(16));
        assert!(
            patches <= u64::from(u32::MAX),
            "water compact patch count exceeds u32"
        );
        assert!(
            patches.div_ceil(u64::from(limits.max_compute_workgroups_per_dimension))
                <= u64::from(limits.max_compute_workgroups_per_dimension),
            "water compact dispatch exceeds device grid limits"
        );
        let scratch_bytes = 16 + (patches * 4).max(4);
        for bytes in [
            indices.size(),
            scratch_bytes,
            args.size(),
            mask.size(),
            templates.size(),
            geometry.size(),
        ] {
            assert!(
                bytes <= limits.max_buffer_size && bytes <= limits.max_storage_buffer_binding_size,
                "water compact buffer exceeds device storage limits"
            );
        }
        let scratch_usage = wgpu::BufferUsages::STORAGE;
        #[cfg(test)]
        let scratch_usage = scratch_usage | wgpu::BufferUsages::COPY_DST;
        let scratch = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("water compact patch cache and offsets"),
            size: scratch_bytes,
            usage: scratch_usage,
            mapped_at_creation: false,
        });
        let dispatch = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("water compact count scan and emit dispatch"),
                contents: bytemuck::cast_slice(&[
                    0u32,
                    1,
                    1,
                    limits.max_compute_workgroups_per_dimension,
                    1,
                    1,
                    1,
                    0,
                    0,
                    1,
                    1,
                    0,
                ]),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_DST,
            },
        );
        let spec = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("water compact grid"),
                contents: bytemuck::cast_slice(&spec),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let local = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water compact metadata and output"),
            layout: &pipelines.local,
            entries: &[
                entry(0, indices),
                entry(1, args),
                entry(2, mask),
                entry(3, templates),
                entry(4, geometry),
                entry(5, &scratch),
                entry(6, &spec),
            ],
        });
        let prepare_local = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water compact dispatch writer"),
            layout: &pipelines.prepare_local,
            entries: &[entry(0, &dispatch)],
        });
        Self {
            scratch,
            dispatch,
            spec,
            local,
            prepare_local,
        }
    }
    pub fn bytes(&self) -> u64 {
        self.scratch.size() + self.dispatch.size() + self.spec.size()
    }
    #[cfg(test)]
    pub fn invalidate_cache(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.scratch, 4, bytemuck::bytes_of(&0u32));
    }
    #[cfg(test)]
    pub fn force_axis(&self, queue: &wgpu::Queue, axis: u32) {
        queue.write_buffer(&self.dispatch, 12, bytemuck::bytes_of(&axis));
    }
}
