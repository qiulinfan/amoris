//! Numeric parity of the renderer's real neural_diffuse WGSL against independent f64 evaluation.
//! Run: cargo run --release -p pocket-render --example neural_gi_probe -- NETWORK.json
//! A nonzero exit and JSON error report signal validation, mapping or numeric failures.

use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use bytemuck::{Pod, Zeroable};
use pocket_assets::gi::{NEURAL_PARAMETER_COUNT, NeuralGi};
use serde_json::{Value, json};
use wgpu::util::DeviceExt;

const ABS_TOLERANCE: f64 = 1e-5;
const REL_TOLERANCE: f64 = 3e-5;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [f32; 4],
    spacing: [f32; 4],
    dimensions: [u32; 4],
    settings: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Sample {
    world: [f32; 4],
    normal: [f32; 4],
}

fn samples(network: &NeuralGi) -> Vec<Sample> {
    let low = network.input_min;
    // Bounds use f32 addition, as WGSL does. Network arithmetic below is independently f64.
    let high: [f32; 3] = std::array::from_fn(|i| low[i] + network.input_extent[i]);
    let center: [f32; 3] = std::array::from_fn(|i| low[i] + 0.5 * network.input_extent[i]);
    let mut points = vec![center];
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let corner = [x, y, z];
                points.push(std::array::from_fn(|i| {
                    if corner[i] == 0 { low[i] } else { high[i] }
                }));
            }
        }
    }
    for axis in 0..3 {
        for edge in [low[axis], high[axis]] {
            let mut point = center;
            point[axis] = edge;
            points.push(point);
        }
        // Use normal floats: Metal may flush subnormal inputs such as 0.next_down() to zero.
        // A representable step outside each face verifies zero RGB and zero coverage.
        let padding = (network.input_extent[axis] * 0.01)
            .max(f32::MIN_POSITIVE)
            .max(low[axis].abs().max(high[axis].abs()) * f32::EPSILON * 4.0);
        for outside in [low[axis] - padding, high[axis] + padding] {
            if !outside.is_finite() {
                continue;
            }
            let mut point = center;
            point[axis] = outside;
            points.push(point);
        }
    }
    for z in 0..4 {
        for y in 0..4 {
            for x in 0..4 {
                let cell = [x, y, z];
                points.push(std::array::from_fn(|i| {
                    low[i] + network.input_extent[i] * ((cell[i] as f32 + 0.5) / 4.0)
                }));
            }
        }
    }
    let mut normals = vec![
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    let diagonal = 1.0_f32 / 3.0_f32.sqrt();
    for x in [-diagonal, diagonal] {
        for y in [-diagonal, diagonal] {
            for z in [-diagonal, diagonal] {
                normals.push([x, y, z]);
            }
        }
    }
    points
        .iter()
        .flat_map(|p| {
            normals.iter().map(move |n| Sample {
                world: [p[0], p[1], p[2], 0.0],
                normal: [n[0], n[1], n[2], 0.0],
            })
        })
        .collect()
}

fn reference(network: &NeuralGi, point: &Sample) -> [f64; 4] {
    for axis in 0..3 {
        let high = network.input_min[axis] + network.input_extent[axis];
        if point.world[axis] < network.input_min[axis] || point.world[axis] > high {
            return [0.0; 4];
        }
    }
    let mut input: Vec<f64> = (0..3)
        .map(|axis| {
            2.0 * (f64::from(point.world[axis]) - f64::from(network.input_min[axis]))
                / f64::from(network.input_extent[axis])
                - 1.0
        })
        .chain(point.normal[..3].iter().copied().map(f64::from))
        .collect();
    for layer in &network.layers {
        input = layer
            .weights
            .iter()
            .zip(&layer.bias)
            .map(|(row, &bias)| {
                (f64::from(bias)
                    + row
                        .iter()
                        .zip(&input)
                        .map(|(&weight, &value)| f64::from(weight) * value)
                        .sum::<f64>())
                .max(0.0)
            })
            .collect();
    }
    [input[0], input[1], input[2], 1.0]
}

fn check_scoped(scope: wgpu::ErrorScopeGuard, phase: &str) -> Result<(), String> {
    match pollster::block_on(scope.pop()) {
        Some(error) => Err(format!("{phase}: {error}")),
        None => Ok(()),
    }
}

fn execute(network: &NeuralGi, report: &mut Value) -> Result<(), String> {
    let gpu = pocket_render::Gpu::headless(pocket_render::BackendChoice::from_env())
        .map_err(|error| error.to_string())?;
    report["adapter"] = json!({"name": gpu.info.name,
        "backend": format!("{:?}", gpu.info.backend),
        "device_type": format!("{:?}", gpu.info.device_type)});
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let uncaptured = errors.clone();
    gpu.device
        .on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            uncaptured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(error.to_string());
        }));
    let lost = errors.clone();
    gpu.device.set_device_lost_callback(move |reason, message| {
        lost.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("device lost {reason:?}: {message}"));
    });
    let d = &gpu.device;
    let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
    let source = format!(
        "{}\n{}",
        include_str!("../shaders/probe_gi.wgsl"),
        include_str!("shaders/neural_gi_probe.wgsl")
    );
    let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("neural GI parity: real renderer shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("neural GI parity"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        // Matches the renderer's private shaders::compute_options(). No workgroup memory is used.
        compilation_options: wgpu::PipelineCompilationOptions {
            zero_initialize_workgroup_memory: false,
            ..Default::default()
        },
        cache: None,
    });
    check_scoped(scope, "shader/pipeline validation")?;
    let points = samples(network);
    if !points
        .iter()
        .flat_map(|p| p.world.iter().chain(&p.normal))
        .all(|v| v.is_finite())
    {
        return Err("network bounds cannot represent finite boundary test points".into());
    }
    report["points"] = json!(points.len());
    let mut packed = Vec::<f32>::with_capacity(NEURAL_PARAMETER_COUNT + 1);
    for layer in &network.layers {
        for row in &layer.weights {
            packed.extend_from_slice(row);
        }
        packed.extend_from_slice(&layer.bias);
    }
    if packed.len() != NEURAL_PARAMETER_COUNT {
        return Err("packed scalar parameter count does not match the contract".into());
    }
    packed.push(0.0); // Real renderer stores array<vec4f>.
    let params = Params {
        origin: [
            network.input_min[0],
            network.input_min[1],
            network.input_min[2],
            0.0,
        ],
        spacing: [
            network.input_extent[0],
            network.input_extent[1],
            network.input_extent[2],
            0.0,
        ],
        dimensions: [32, 6, 3, 0],
        settings: [1.0, 0.0, 0.0, 2.0],
    };
    let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
    let input = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("neural parity world/normal"),
        contents: bytemuck::cast_slice(&points),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let weights = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("neural parity packed weights"),
        contents: bytemuck::cast_slice(&packed),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let uniform = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("neural parity same 64-byte params"),
        contents: bytemuck::bytes_of(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bytes = points.len() as u64 * 16;
    let output = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("neural parity RGB/coverage"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("neural parity readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let inputs = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("neural parity group 0"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let neural = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("neural parity group 1"),
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 8,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: weights.as_entire_binding(),
            },
        ],
    });
    check_scoped(scope, "resource validation")?;
    let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut encoder = d.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("neural parity"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("neural parity real inference"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &inputs, &[]);
        pass.set_bind_group(1, &neural, &[]);
        pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
    let submission = gpu.queue.submit([encoder.finish()]);
    let (send, receive) = mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
    d.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(Duration::from_secs(30)),
    })
    .map_err(|error| format!("GPU poll failed: {error}"))?;
    check_scoped(scope, "dispatch/readback validation")?;
    receive
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("missing readback callback: {error}"))?
        .map_err(|error| format!("GPU buffer map failed: {error}"))?;
    let messages = errors.lock().unwrap_or_else(|e| e.into_inner()).clone();
    report["gpu_errors"] = json!(messages);
    if !messages.is_empty() {
        return Err(format!("GPU reported {} errors", messages.len()));
    }
    let data = readback
        .slice(..)
        .get_mapped_range()
        .map_err(|error| format!("GPU mapped-range access failed: {error:?}"))?;
    let values: &[[f32; 4]] = bytemuck::cast_slice(&data);
    let mut maximum = 0.0_f64;
    let mut maximum_relative = 0.0_f64;
    let mut failures = 0usize;
    let mut worst = Value::Null;
    for (index, (point, actual)) in points.iter().zip(values).enumerate() {
        let expected = reference(network, point);
        let mut failed = false;
        for component in 0..4 {
            let value = f64::from(actual[component]);
            if !value.is_finite() || !expected[component].is_finite() {
                return Err(format!(
                    "point {index} component {component} has non-finite output"
                ));
            }
            let error = (value - expected[component]).abs();
            let relative = error / expected[component].abs().max(1e-6);
            if error > maximum {
                maximum = error;
                worst = json!({"index": index, "component": component,
                    "world": point.world[..3], "normal": point.normal[..3],
                    "cpu_f64": expected, "gpu_f32": actual});
            }
            maximum_relative = maximum_relative.max(relative);
            failed |= if component == 3 {
                error != 0.0
            } else {
                error > ABS_TOLERANCE + REL_TOLERANCE * expected[component].abs()
            };
        }
        failures += usize::from(failed);
    }
    drop(data);
    readback.unmap();
    report["max_abs"] = json!(maximum);
    report["max_relative"] = json!(maximum_relative);
    report["failed_points"] = json!(failures);
    report["worst"] = worst;
    if failures != 0 {
        return Err(format!(
            "{failures} points exceeded numeric tolerance or coverage differed"
        ));
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    let mut report = json!({"adapter": null, "points": 0, "max_abs": null,
        "abs_tolerance": ABS_TOLERANCE, "relative_tolerance": REL_TOLERANCE,
        "gpu_errors": [], "error": null});
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if arguments.len() == 1 {
        NeuralGi::load(Path::new(&arguments[0])).and_then(|network| execute(&network, &mut report))
    } else {
        Err("usage: neural_gi_probe NETWORK.json".into())
    };
    if let Err(error) = &result {
        report["error"] = json!(error);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).expect("finite report")
    );
    if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
