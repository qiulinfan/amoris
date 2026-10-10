//! A real acceleration-structure/ray-query/readback capability probe for any backend with wgpu ray
//! queries: Metal, Vulkan (`VK_KHR_ray_query`), Direct3D 12 (DXR 1.1).
//!
//! Run with `cargo run --release -p pocket-render --example ray_query_probe`; `POCKET_BACKEND` and
//! `POCKET_ADAPTER` choose the GPU as for the renderer, `WGPU_VALIDATION=1` adds the backend's
//! validation layer. Emits one JSON object and exits nonzero on unsupported hardware, GPU errors or
//! wrong results.
//!
//! The same rays are traced twice. Against opaque geometry the traversal commits hits by itself
//! (the SHaRC/ReSTIR tracer's case). Against non-opaque geometry every hit is a candidate the shader
//! must inspect and confirm (the path tracer's case: alpha and sidedness); there the first
//! candidate's fields are recorded as well. Both must give the same committed hits. Besides hits,
//! misses, masks and instance transforms the probe checks the triangle facing convention the path
//! tracer's sidedness relies on: a ray whose origin is on the side from which the vertices appear
//! counterclockwise (in right-handed world coordinates, as glTF meshes are wound) must report a
//! front face, a ray from the other side a back face.
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
    front_face: u32,
    /// Candidates seen before the traversal finished (non-opaque pass only).
    candidates: u32,
}

/// Name, then hit distance, custom data, instance, primitive and front face.
type Expected = (&'static str, Option<(f32, u32, u32, u32, bool)>);

fn main() {
    match pollster::block_on(probe()) {
        Ok(report) => {
            println!("{}", serde_json::to_string(&report).unwrap());
            if report["passed"] != true {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("ray-query probe failed: {error}");
            println!("{}", json!({"passed": false, "error": error.to_string()}));
            std::process::exit(1);
        }
    }
}

fn matches(actual: &RayResult, expected: &Option<(f32, u32, u32, u32, bool)>) -> bool {
    match expected {
        Some((t, custom, instance, primitive, front)) => {
            actual.kind == 1
                && (actual.front_face != 0) == *front
                && (actual.t - t).abs() < 1e-5
                && actual.instance_custom_data == *custom
                && actual.instance_index == *instance
                && actual.primitive_index == *primitive
                && actual.geometry_index == 0
                && (actual.barycentrics[0] - 0.25).abs() < 1e-5
                && (actual.barycentrics[1] - 0.5).abs() < 1e-5
        }
        None => actual.kind == 0 && actual.t == -1.0,
    }
}

async fn probe() -> ProbeResult<Value> {
    let start = Instant::now();
    // The renderer's instance and adapter choice: Direct3D 12 needs its DXC (Shader Model 6.5).
    let gpu = pocket_render::Gpu::headless(pocket_render::BackendChoice::from_env())?;
    let adapter = &gpu.adapter;
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
            "backend": gpu.backend_name(),
            "adapter": adapter_report,
            "passed": false,
            "supported": false,
            "error": format!("{} adapter does not expose EXPERIMENTAL_RAY_QUERY", gpu.backend_name()),
        }));
    }
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("ray-query probe"),
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
    let sizes = [
        wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        wgpu::AccelerationStructureGeometryFlags::empty(),
    ]
    .map(|flags| wgpu::BlasTriangleGeometrySizeDescriptor {
        vertex_format: wgpu::VertexFormat::Float32x3,
        vertex_count: vertices.len() as u32,
        index_format: None,
        index_count: None,
        flags,
    });
    let structures: Vec<(wgpu::Blas, wgpu::Tlas)> = sizes
        .iter()
        .map(|size| {
            let blas = device.create_blas(
                &wgpu::CreateBlasDescriptor {
                    label: Some("probe BLAS"),
                    flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                    update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                },
                wgpu::BlasGeometrySizeDescriptors::Triangles {
                    descriptors: vec![size.clone()],
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
            (blas, tlas)
        })
        .collect();
    // The triangles' counterclockwise (right-handed) normal is +z: rays along +z see them from
    // behind, the last ray, along -z from z = 2, from the front.
    let ray = |origin: [f32; 3], z: f32, t_max: f32, mask| RayInput {
        origin_t_min: [origin[0], origin[1], origin[2], 0.01],
        direction_t_max: [0.0, 0.0, z, t_max],
        mask,
        padding: [0; 3],
    };
    let rays = [
        ray([0.0, 0.0, 0.0], 1.0, 10.0, 1),
        ray([2.0, 0.0, 0.0], 1.0, 10.0, 1),
        ray([0.0, 4.0, 0.0], 1.0, 10.0, 1),
        ray([4.0, 0.0, 0.0], 1.0, 10.0, 1),
        ray([0.0, 0.0, 0.0], 1.0, 0.5, 1),
        ray([0.0, 0.0, 0.0], 1.0, 10.0, 2),
        ray([0.0, 0.0, 2.0], -1.0, 10.0, 1),
    ];
    let expected: [Expected; 7] = [
        ("hit", Some((1.0, 17, 0, 0, false))),
        ("miss", None),
        ("transformed_instance", Some((3.0, 29, 1, 0, false))),
        ("second_primitive", Some((1.0, 17, 0, 1, false))),
        ("clipped_by_t_max", None),
        ("excluded_by_mask", None),
        (
            "front_face_from_counterclockwise_side",
            Some((1.0, 17, 0, 0, true)),
        ),
    ];
    let ray_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probe rays"),
        contents: bytemuck::cast_slice(&rays),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let result_size = (rays.len() * std::mem::size_of::<RayResult>()) as u64;
    // Committed hits of the opaque and non-opaque passes, then the non-opaque first candidates.
    let results: Vec<wgpu::Buffer> = (0..3)
        .map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("probe results"),
                size: result_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        })
        .collect();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe readback"),
        size: result_size * 3,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("probe ray query"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/ray_query_probe.wgsl").into()),
    });
    let pipeline = |entry| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: None,
            module: &module,
            entry_point: Some(entry),
            compilation_options: wgpu::PipelineCompilationOptions {
                zero_initialize_workgroup_memory: false,
                ..Default::default()
            },
            cache: None,
        })
    };
    let committed = pipeline("main");
    let confirmed = pipeline("confirm_candidates");
    let entry = |binding, resource| wgpu::BindGroupEntry { binding, resource };
    let opaque_bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe opaque bindings"),
        layout: &committed.get_bind_group_layout(0),
        entries: &[
            entry(0, structures[0].1.as_binding()),
            entry(1, ray_buffer.as_entire_binding()),
            entry(2, results[0].as_entire_binding()),
        ],
    });
    let candidate_bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe non-opaque bindings"),
        layout: &confirmed.get_bind_group_layout(0),
        entries: &[
            entry(0, structures[1].1.as_binding()),
            entry(1, ray_buffer.as_entire_binding()),
            entry(2, results[1].as_entire_binding()),
            entry(3, results[2].as_entire_binding()),
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
    let builds: Vec<_> = structures
        .iter()
        .zip(&sizes)
        .map(|((blas, _), size)| wgpu::BlasBuildEntry {
            blas,
            geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                size,
                vertex_buffer: &vertex_buffer,
                first_vertex: 0,
                vertex_stride: std::mem::size_of::<[f32; 3]>() as u64,
                index_buffer: None,
                first_index: None,
                transform_buffer: None,
                transform_buffer_offset: None,
            }]),
        })
        .collect();
    if info.backend == wgpu::Backend::Metal {
        // wgpu-hal 30.0.1 does not implement its Metal BLAS-to-TLAS barrier (#9215).
        // Complete the producers before encoding their consumers; preserve this probe's
        // candidate/committed-hit checks instead of relying on an unordered combined build.
        encoder.build_acceleration_structures(&builds, std::iter::empty());
        let blas_submission = queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(blas_submission),
            timeout: Some(Duration::from_secs(30)),
        })?;
        encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("trace probe after completed BLAS builds"),
        });
        encoder.build_acceleration_structures(
            std::iter::empty(),
            structures.iter().map(|(_, tlas)| tlas),
        );
    } else {
        encoder.build_acceleration_structures(&builds, structures.iter().map(|(_, tlas)| tlas));
    }
    for (pipeline, bindings) in [
        (&committed, &opaque_bindings),
        (&confirmed, &candidate_bindings),
    ] {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("trace probe rays"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bindings, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    for (index, buffer) in results.iter().enumerate() {
        encoder.copy_buffer_to_buffer(
            buffer,
            0,
            &readback,
            index as u64 * result_size,
            result_size,
        );
    }
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
    let (opaque, rest) = actual.split_at(rays.len());
    let (nonopaque, first_candidates) = rest.split_at(rays.len());
    let cases: Vec<_> = expected
        .iter()
        .enumerate()
        .map(|(i, (name, hit))| {
            // A hit is exactly one candidate in the non-opaque pass; a miss none.
            let candidate_ok = match hit {
                Some(_) => {
                    let mut c = first_candidates[i];
                    c.candidates = 0;
                    first_candidates[i].candidates == 1 && matches(&c, hit)
                }
                None => first_candidates[i].candidates == 0,
            };
            let passed = matches(&opaque[i], hit) && matches(&nonopaque[i], hit) && candidate_ok;
            json!({"name": name, "expected_hit_t_custom_instance_primitive_front": hit,
            "opaque": opaque[i], "nonopaque_committed": nonopaque[i],
            "nonopaque_first_candidate": first_candidates[i], "passed": passed})
        })
        .collect();
    let errors = gpu_errors.lock().unwrap().clone();
    Ok(json!({
        "backend": gpu.backend_name(),
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
