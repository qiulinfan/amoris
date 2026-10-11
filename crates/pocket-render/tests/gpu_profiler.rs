//! In-flight query ownership and bounded trace capture, with no CPU wait between submissions.
use pocket_render::profiler::GpuProfiler;
use pocket_render::{BackendChoice, Gpu};

#[test]
fn asynchronous_frames_keep_their_queries_and_only_record_the_capture_window() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    if !gpu.caps.timestamps {
        eprintln!("no GPU timestamps: skipped");
        return;
    }
    let device = &gpu.device;
    let queue = &gpu.queue;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("profiler regression"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/trace_clock.wgsl").into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("profiler regression"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions {
            zero_initialize_workgroup_memory: false,
            ..Default::default()
        },
        cache: None,
    });
    let storage = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: storage.as_entire_binding(),
        }],
    });
    let mut profiler = GpuProfiler::new(device, queue, true, &gpu.info);
    profiler.set_tracing(device, true);
    const CAPTURE: u64 = 64;
    let mut accepted = Vec::new();
    for frame in 0..CAPTURE * 3 {
        if frame == CAPTURE {
            profiler.stop_recording();
        }
        profiler.poll();
        assert_eq!(profiler.frame(), frame);
        let available = profiler.times_frame();
        let mut enc = device.create_command_encoder(&Default::default());
        let writes = profiler.compute_scope("work");
        assert_eq!(writes.is_some(), available);
        if available && frame < CAPTURE {
            accepted.push(frame);
        }
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("work"),
                timestamp_writes: writes,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        profiler.resolve(&mut enc);
        queue.submit([enc.finish()]);
        profiler.after_submit();
    }
    // No later render is needed to drive sampling -> resolve -> mapping to completion.
    profiler.drain(device);
    let mut raw = profiler.take_raw();
    raw.sort_by_key(|f| f.frame);
    assert!(!accepted.is_empty());
    assert_eq!(raw.iter().map(|f| f.frame).collect::<Vec<_>>(), accepted);
    assert_eq!(profiler.dropped, CAPTURE - raw.len() as u64);
    let mut previous_end = 0;
    for frame in raw {
        assert_eq!(frame.scopes.len(), 1);
        let scope = &frame.scopes[0];
        assert_eq!(scope.label, "work");
        assert!(
            scope.begin > previous_end,
            "stale/duplicate frame {}",
            frame.frame
        );
        assert!(scope.end >= scope.begin);
        previous_end = scope.end;
    }
    assert!(profiler.take_raw().is_empty());
}
