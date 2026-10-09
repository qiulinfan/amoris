//! A real Metal acceleration-structure/ray-query/readback capability probe.
//!
//! Run with `cargo run --release -p pocket-render --example metal_ray_query`.
//! Emits one JSON object and exits nonzero on unsupported hardware, GPU errors or wrong results.
//! Wall timings include setup and readback; these few rays are not a GI performance benchmark.
//! API references: wgpu/wgpu-types 30.0.1 `src/api/{blas,tlas,command_encoder}.rs` and
//! wgpu-hal 30.0.1 `examples/ray-traced-triangle/shader.wgsl`. No SDK implementation is copied.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use serde::Serialize;
use serde_json::{Value, json};
use wgpu::util::DeviceExt;

type ProbeResult<T> = Result<T, Box<dyn std::error::Error>>;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RayInput {
    origin_t_min: [f32; 4],
    direction_t_max: [f32; 4],
    mask: u32,
    padding: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, Serialize)]
struct RayResult {
    kind: u32,
    t: f32,
    instance_custom_data: u32,
    instance_index: u32,
    primitive_index: u32,
    geometry_index: u32,
    barycentrics: [f32; 2],
}

fn main() {
    match pollster::block_on(probe()) {
        Ok(report) => {
            println!("{}", serde_json::to_string(&report).unwrap());
            if report["passed"] != true {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("Metal ray-query probe failed: {error}");
            println!("{}", json!({"passed": false, "error": error.to_string()}));
            std::process::exit(1);
        }
    }
}

async fn probe() -> ProbeResult<Value> {
    let start = Instant::now();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::debugging(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await?;
    let info = adapter.get_info();
    let features = adapter.features();
    let adapter_report = json!({
        "name": info.name,
        "backend": format!("{:?}", info.backend),
        "device_type": format!("{:?}", info.device_type),
        "driver": info.driver,
        "driver_info": info.driver_info,
        "features": format!("{features:?}"),
    });
    if !features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
        return Ok(json!({
            "adapter": adapter_report,
            "passed": false,
            "supported": false,
            "error": "Metal adapter does not expose EXPERIMENTAL_RAY_QUERY",
        }));
    }
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("Metal ray-query probe"),
            required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY,
            required_limits: wgpu::Limits::default()
                .using_acceleration_structure_values(adapter.limits()),
            // SAFETY: opt in only for this isolated capability probe. Experimental wgpu APIs
            // may contain implementation bugs; validation remains enabled and errors are reported.
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        })
        .await?;
    let device_ms = start.elapsed().as_secs_f64() * 1000.0;

    let gpu_errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let uncaptured = gpu_errors.clone();
    device.on_uncaptured_error(Arc::new(move |error| {
        eprintln!("Uncaptured GPU error: {error}");
        uncaptured.lock().unwrap().push(error.to_string());
    }));
    let lost = gpu_errors.clone();
    device.set_device_lost_callback(move |reason, message| {
        eprintln!("GPU device lost ({reason:?}): {message}");
        lost.lock()
            .unwrap()
            .push(format!("Device lost ({reason:?}): {message}"));
    });
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let setup = Instant::now();
    // Two separated triangles let us validate a nonzero primitive index as well as a hit.
    let vertices: [[f32; 3]; 6] = [
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [0.0, 1.0, 1.0],
        [3.0, -1.0, 1.0],
        [5.0, -1.0, 1.0],
        [4.0, 1.0, 1.0],
    ];
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probe triangles"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::BLAS_INPUT,
    });
    let geometry_size = wgpu::BlasTriangleGeometrySizeDescriptor {
        vertex_format: wgpu::VertexFormat::Float32x3,
        vertex_count: vertices.len() as u32,
        index_format: None,
        index_count: None,
        flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
    };
    let blas = device.create_blas(
        &wgpu::CreateBlasDescriptor {
            label: Some("probe BLAS"),
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        },
        wgpu::BlasGeometrySizeDescriptors::Triangles {
            descriptors: vec![geometry_size.clone()],
        },
    );
    let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
        label: Some("probe TLAS"),
        max_instances: 2,
        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
    });
    tlas[0] = Some(wgpu::TlasInstance::new(
        &blas,
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        17,
        1,
    ));
    tlas[1] = Some(wgpu::TlasInstance::new(
        &blas,
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 4.0, 0.0, 0.0, 1.0, 2.0],
        29,
        1,
    ));
    let ray = |origin: [f32; 3], t_max: f32, mask| RayInput {
        origin_t_min: [origin[0], origin[1], origin[2], 0.01],
        direction_t_max: [0.0, 0.0, 1.0, t_max],
        mask,
        padding: [0; 3],
    };
    let rays = [
        ray([0.0, 0.0, 0.0], 10.0, 1),
        ray([2.0, 0.0, 0.0], 10.0, 1),
        ray([0.0, 4.0, 0.0], 10.0, 1),
        ray([4.0, 0.0, 0.0], 10.0, 1),
        ray([0.0, 0.0, 0.0], 0.5, 1),
        ray([0.0, 0.0, 0.0], 10.0, 2),
    ];
    let ray_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probe rays"),
        contents: bytemuck::cast_slice(&rays),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let result_size = (rays.len() * std::mem::size_of::<RayResult>()) as u64;
    let results = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe results"),
        size: result_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe readback"),
        size: result_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("probe ray query"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/metal_ray_query.wgsl").into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("probe ray query"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions {
            zero_initialize_workgroup_memory: false,
            ..Default::default()
        },
        cache: None,
    });
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe bindings"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: tlas.as_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: ray_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: results.as_entire_binding(),
            },
        ],
    });
    if let Some(error) = scope.pop().await {
        return Err(format!("GPU setup validation: {error}").into());
    }
    let setup_ms = setup.elapsed().as_secs_f64() * 1000.0;
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let execute = Instant::now();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("build and trace probe"),
    });
    let build = wgpu::BlasBuildEntry {
        blas: &blas,
        geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
            size: &geometry_size,
            vertex_buffer: &vertex_buffer,
            first_vertex: 0,
            vertex_stride: std::mem::size_of::<[f32; 3]>() as u64,
            index_buffer: None,
            first_index: None,
            transform_buffer: None,
            transform_buffer_offset: None,
        }]),
    };
    encoder.build_acceleration_structures([&build], [&tlas]);
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("trace six probe rays"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&results, 0, &readback, 0, result_size);
    let submission = queue.submit([encoder.finish()]);
    let (sender, receiver) = mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(Duration::from_secs(30)),
    })?;
    receiver.recv_timeout(Duration::from_secs(1))??;
    if let Some(error) = scope.pop().await {
        return Err(format!("GPU execution validation: {error}").into());
    }
    let mapped = readback.slice(..).get_mapped_range()?;
    let actual: Vec<RayResult> = bytemuck::cast_slice(&mapped).to_vec();
    drop(mapped);
    readback.unmap();
    let execute_ms = execute.elapsed().as_secs_f64() * 1000.0;
    let expected = [
        ("hit", Some((1.0, 17, 0, 0))),
        ("miss", None),
        ("transformed_instance", Some((3.0, 29, 1, 0))),
        ("second_primitive", Some((1.0, 17, 0, 1))),
        ("clipped_by_t_max", None),
        ("excluded_by_mask", None),
    ];
    let cases: Vec<_> = expected
        .iter()
        .zip(&actual)
        .map(|((name, hit), actual)| {
            let passed = match hit {
                Some((t, custom, instance, primitive)) => {
                    actual.kind == 1
                        && (actual.t - t).abs() < 1e-5
                        && actual.instance_custom_data == *custom
                        && actual.instance_index == *instance
                        && actual.primitive_index == *primitive
                        && actual.geometry_index == 0
                        && (actual.barycentrics[0] - 0.25).abs() < 1e-5
                        && (actual.barycentrics[1] - 0.5).abs() < 1e-5
                }
                None => actual.kind == 0 && actual.t == -1.0,
            };
            json!({"name": name, "expected_hit_t_custom_instance_primitive": hit,
            "actual": actual, "passed": passed})
        })
        .collect();
    let errors = gpu_errors.lock().unwrap().clone();
    Ok(json!({
        "adapter": adapter_report,
        "supported": true,
        "wgpu_version": "30.0.1",
        "device_features": format!("{:?}", device.features()),
        "triangle_count": 2,
        "instance_count": 2,
        "ray_count": rays.len(),
        "cases": cases,
        "gpu_errors": errors,
        "passed": cases.iter().all(|case| case["passed"] == true) && errors.is_empty(),
        "timings_wall_ms": {"device_request": device_ms, "resource_setup": setup_ms,
            "build_trace_readback": execute_ms, "total": start.elapsed().as_secs_f64() * 1000.0},
        "timing_note": "Single capability probe; includes CPU/driver/readback costs, not a GI benchmark",
    }))
}
