//! GPU gradient parity and live synthetic lighting-change fixture for the online NRC.
//! The actual path-traced-scene evidence is produced separately by `path_trace`.
//! Run: cargo run --release -p pocket-render --example nrc_online_probe

#[path = "../src/gi/nrc.rs"]
mod nrc;

use nrc::{NrcConfig, NrcRecord, OnlineNrc, PARAMETER_COUNT};
use serde_json::json;

fn run() -> Result<serde_json::Value, String> {
    let gpu = pocket_render::Gpu::headless(pocket_render::BackendChoice::from_env())
        .map_err(|e| e.to_string())?;
    let device = &gpu.device;
    let queue = &gpu.queue;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut cache = OnlineNrc::new(
        device,
        queue,
        NrcConfig {
            batch_size: 128,
            learning_rate: 0.003,
            ..Default::default()
        },
    )?;
    cache.reset(queue);
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("online NRC probe outputs"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let outputs = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("online NRC probe predictions"),
        size: u64::from(cache.config.batch_size) * 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("online NRC probe group"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: outputs.as_entire_binding(),
        }],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("online NRC probe layout"),
        bind_group_layouts: &[Some(&layout), Some(&cache.layout)],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("online NRC probe"),
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "{}\n{}",
                nrc::WGSL,
                include_str!("shaders/nrc_online_probe.wgsl")
            )
            .into(),
        ),
    });
    let pipeline = |entry| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pl),
            module: &shader,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions {
                zero_initialize_workgroup_memory: false,
                ..Default::default()
            },
            cache: None,
        })
    };
    let fill = pipeline("nrc_probe_fill");
    let eval = pipeline("nrc_probe_eval");
    if let Some(error) = pollster::block_on(scope.pop()) {
        return Err(format!("online NRC shader validation: {error}"));
    }
    let initial = nrc::initial_weights(cache.config.seed);
    let mut gradient_max_abs = 0.0_f64;
    let mut inference_max_abs = 0.0_f64;
    let mut frames = Vec::new();
    for frame in 0..128 {
        cache.begin_frame(queue)?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("online NRC fixture"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fresh synthetic labels"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&fill);
            pass.set_bind_group(0, &group, &[]);
            pass.set_bind_group(1, &cache.bind_group, &[]);
            pass.dispatch_workgroups(cache.config.batch_size.div_ceil(32), 1, 1);
        }
        cache.encode_training(&mut encoder)?;
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("updated prediction"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&eval);
            pass.set_bind_group(0, &group, &[]);
            pass.set_bind_group(1, &cache.bind_group, &[]);
            pass.dispatch_workgroups(cache.config.batch_size.div_ceil(32), 1, 1);
        }
        queue.submit([encoder.finish()]);
        if frame == 0 {
            let record_bytes = nrc::read_buffer(
                device,
                queue,
                &cache.records,
                u64::from(cache.config.batch_size) * 80,
            )?;
            let records: &[NrcRecord] = bytemuck::cast_slice(&record_bytes);
            let gradient_bytes = nrc::read_buffer(
                device,
                queue,
                &cache.gradients,
                u64::from(cache.config.batch_size) * PARAMETER_COUNT as u64 * 4,
            )?;
            let gradients: &[f32] = bytemuck::cast_slice(&gradient_bytes);
            for i in [0, 1, 31, 63, 127] {
                let (_, reference) =
                    nrc::reference_gradient(&initial, &records[i], cache.config.radiance_scale);
                for j in 0..PARAMETER_COUNT {
                    let error =
                        (reference[j] - f64::from(gradients[i * PARAMETER_COUNT + j])).abs();
                    gradient_max_abs = gradient_max_abs.max(error);
                    if error > 1e-4 + reference[j].abs() * 2e-4 {
                        return Err(format!(
                            "GPU gradient mismatch at sample {i}, parameter {j}: {error}"
                        ));
                    }
                }
            }
        }
        if [0, 15, 31, 47, 63, 64, 79, 95, 111, 127].contains(&frame) {
            let stats = cache.read_stats(device, queue)?;
            if stats.invalid_records + stats.invalid_updates + stats.invalid_predictions != 0
                || stats.trained_records != 128
            {
                return Err(format!("invalid online NRC frame {frame}: {stats:?}"));
            }
            let bytes = nrc::read_buffer(
                device,
                queue,
                &outputs,
                u64::from(cache.config.batch_size) * 16,
            )?;
            let predictions: &[[f32; 4]] = bytemuck::cast_slice(&bytes);
            let mean_rgb: [f64; 3] = std::array::from_fn(|c| {
                predictions.iter().map(|p| f64::from(p[c])).sum::<f64>() / predictions.len() as f64
            });
            frames.push(json!({ "frame":frame,"stats":stats,"mean_prediction_rgb":mean_rgb }));
            if frame == 127 {
                let bytes =
                    nrc::read_buffer(device, queue, &cache.weights, PARAMETER_COUNT as u64 * 4)?;
                let weights: &[f32] = bytemuck::cast_slice(&bytes);
                let bytes = nrc::read_buffer(
                    device,
                    queue,
                    &cache.records,
                    u64::from(cache.config.batch_size) * 80,
                )?;
                let records: &[NrcRecord] = bytemuck::cast_slice(&bytes);
                for (i, record) in records.iter().enumerate() {
                    let (_, _, output) = nrc::reference_forward(weights, record);
                    for c in 0..3 {
                        let expected = (output[c]
                            .clamp(0.0, f64::from(cache.config.max_log_prediction))
                            .exp()
                            - 1.0)
                            * f64::from(cache.config.radiance_scale);
                        inference_max_abs =
                            inference_max_abs.max((expected - f64::from(predictions[i][c])).abs());
                    }
                }
            }
        }
    }
    let loss = |index: usize| frames[index]["stats"]["mean_log_mse"].as_f64().unwrap();
    if !(loss(4) < loss(0) * 0.2 && loss(9) < loss(5) * 0.2) {
        return Err(format!(
            "online learning did not converge before/after light change: {frames:?}"
        ));
    }
    if inference_max_abs > 5e-5 {
        return Err(format!("GPU inference mismatch: {inference_max_abs}"));
    }
    Ok(
        json!({ "adapter":gpu.info.name,"backend":format!("{:?}",gpu.info.backend),
        "fixture":"synthetic dynamic labels, not path-traced scene evidence",
        "architecture":[nrc::INPUT_COUNT,nrc::HIDDEN_COUNT,nrc::HIDDEN_COUNT,3],"parameter_count":PARAMETER_COUNT,"batch_size":128,
        "gradient_max_abs_error":gradient_max_abs,"inference_max_abs_error":inference_max_abs,
        "frames":frames,"status":"passed" }),
    )
}

fn main() {
    match run() {
        Ok(report) => println!("{}", serde_json::to_string_pretty(&report).unwrap()),
        Err(error) => {
            eprintln!("{}", json!({"status":"failed","error":error}));
            std::process::exit(1);
        }
    }
}
