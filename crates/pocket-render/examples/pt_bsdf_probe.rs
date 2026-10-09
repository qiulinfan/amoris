//! GPU numeric validation of the real WGSL BSDF against its independent f64 reference, on the
//! backend `POCKET_BACKEND` names (Metal, Vulkan, Direct3D 12; `POCKET_ADAPTER` picks the GPU).
//! Run cargo run --release -p pocket-render --example pt_bsdf_probe.
//! Tests opaque/GGX, smooth and rough dielectric entering/exiting, mixed materials and grazing views.
#[allow(dead_code)]
#[path = "../src/gi/bsdf.rs"]
mod reference;

use bytemuck::{Pod, Zeroable};
use glam::DVec3;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use wgpu::util::DeviceExt;

const ABS_TOLERANCE: f64 = 2e-4;
const REL_TOLERANCE: f64 = 2e-3;
// Extremely grazing near-delta densities amplify unavoidable f32 half-vector cancellation.
// Direction, throughput and Fresnel remain at the tighter general tolerance.
const PDF_REL_TOLERANCE: f64 = 1e-2;
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Input {
    base: [f32; 4],
    surface: [f32; 4],
    wo: [f32; 4],
    wi: [f32; 4],
    random: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Output {
    evaluation: [f32; 4],
    direction: [f32; 4],
    weight: [f32; 4],
    meta: [f32; 4],
}
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / 16777216.0
    }
    fn sphere(&mut self) -> [f32; 4] {
        let z = 2.0 * self.next() - 1.0;
        let phi = 2.0 * std::f32::consts::PI * self.next();
        let r = (1.0 - z * z).sqrt();
        [r * phi.cos(), r * phi.sin(), z, 0.0]
    }
}
fn queries() -> Vec<Input> {
    let mut points = Vec::new();
    let mut rng = Rng(0x3dab44771359);
    for (metal, transmission, rough, eta) in [
        (1.0, 0.0, 0.0, 1.5),
        (1.0, 0.0, 0.03, 1.5),
        (1.0, 0.0, 0.75, 1.5),
        (0.0, 0.0, 0.0, 1.5),
        (0.0, 0.0, 0.75, 1.5),
        (0.0, 1.0, 0.0, 1.5),
        (0.0, 1.0, 0.0, 1.0 / 1.5),
        (0.0, 1.0, 0.35, 1.5),
        (0.0, 1.0, 0.75, 1.5),
        (0.0, 1.0, 0.75, 1.0 / 1.5),
        (0.0, 1.0, 0.75, 1.0),
        (0.15, 0.65, 0.75, 1.5),
    ] {
        for z in [1.0f32, 0.9, 0.65, 0.1, 0.01, 0.002] {
            let wo = [(1.0 - z * z).sqrt(), 0.0, z, 0.0];
            for i in 0..48 {
                let wi = if i == 0 {
                    [-wo[0], 0.0, wo[2], 0.0]
                } else {
                    rng.sphere()
                };
                points.push(Input {
                    base: [0.7, 0.5, 0.3, metal],
                    surface: [rough, transmission, eta, 0.0],
                    wo,
                    wi,
                    random: [rng.next(), rng.next(), rng.next(), rng.next()],
                });
            }
        }
    }
    points
}
fn vector(v: [f32; 4]) -> DVec3 {
    DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)
}
fn cpu(input: &Input) -> [[f64; 4]; 4] {
    let b = reference::Bsdf {
        base: vector(input.base),
        metallic: input.base[3] as f64,
        roughness: input.surface[0] as f64,
        transmission: input.surface[1] as f64,
        eta: input.surface[2] as f64,
    };
    let e = b.evaluate(vector(input.wo), vector(input.wi));
    let s = b.sample(vector(input.wo), input.random.map(f64::from));
    [
        [e.value.x, e.value.y, e.value.z, e.pdf],
        [s.direction.x, s.direction.y, s.direction.z, s.pdf],
        [s.weight.x, s.weight.y, s.weight.z, f64::from(s.delta)],
        [
            s.eta_scale,
            reference::fresnel(input.wo[2] as f64, b.eta),
            0.0,
            0.0,
        ],
    ]
}
fn execute(report: &mut Value) -> Result<(), String> {
    let gpu = pocket_render::Gpu::headless(pocket_render::BackendChoice::from_env())
        .map_err(|e| e.to_string())?;
    report["adapter"] = json!(gpu.info.name);
    report["backend"] = json!(gpu.backend_name());
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = errors.clone();
    gpu.device.on_uncaptured_error(Arc::new(move |e| {
        captured.lock().unwrap().push(e.to_string())
    }));
    let d = &gpu.device;
    let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
    // The production BSDF module's query directive is needed only by the full tracer.
    // This numeric kernel uses no rays and therefore runs on the ordinary renderer device.
    let source = [
        include_str!("../shaders/pt_bsdf.wgsl").replace("enable wgpu_ray_query;\n", ""),
        include_str!("shaders/pt_bsdf_probe.wgsl").to_string(),
    ]
    .join("\n");
    let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("real PT BSDF parity"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("PT BSDF parity"),
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: wgpu::PipelineCompilationOptions {
            zero_initialize_workgroup_memory: false,
            ..Default::default()
        },
        cache: None,
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(format!("BSDF shader compilation: {e}"));
    }
    let points = queries();
    report["queries"] = json!(points.len());
    let input = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("parity input"),
        contents: bytemuck::cast_slice(&points),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let bytes = (points.len() * std::mem::size_of::<Output>()) as u64;
    let output = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity result"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("BSDF parity"),
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
    let mut encoder = d.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("BSDF parity"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("real BSDF calls"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
    let submission = gpu.queue.submit([encoder.finish()]);
    let (send, receive) = mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = send.send(r);
    });
    d.poll(wgpu::PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(Duration::from_secs(30)),
    })
    .map_err(|e| e.to_string())?;
    receive
        .recv_timeout(Duration::from_secs(30))
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let mapping = readback
        .slice(..)
        .get_mapped_range()
        .map_err(|e| e.to_string())?;
    let actual: &[Output] = bytemuck::cast_slice(&mapping);
    let mut maximum = 0.0f64;
    let mut scaled_maximum = 0.0f64;
    let mut failures = 0;
    let mut ordinary_max_abs = 0.0f64;
    let mut ordinary_max_ratio = 0.0f64;
    let mut singular_pdf_max_relative = 0.0f64;
    let mut ordinary_worst = Value::Null;
    let mut worst = Value::Null;
    for (index, (i, a)) in points.iter().zip(actual).enumerate() {
        let expected = cpu(i);
        let rows = [a.evaluation, a.direction, a.weight, a.meta];
        let mut failed = false;
        for row in 0..4 {
            for c in 0..4 {
                let error = (rows[row][c] as f64 - expected[row][c]).abs();
                let singular = i.surface[0] > 0.0001 && i.surface[0] < 0.1 && i.wo[2] < 0.05;
                let relative = if singular && c == 3 && row < 2 {
                    PDF_REL_TOLERANCE
                } else {
                    REL_TOLERANCE
                };
                let tolerance = ABS_TOLERANCE + relative * expected[row][c].abs();
                if !rows[row][c].is_finite() || !expected[row][c].is_finite() || error > tolerance {
                    failed = true;
                }
                maximum = maximum.max(error);
                if singular && c == 3 && row < 2 {
                    singular_pdf_max_relative =
                        singular_pdf_max_relative.max(error / expected[row][c].abs().max(1e-12));
                }
                if !singular {
                    ordinary_max_abs = ordinary_max_abs.max(error);
                    if error / tolerance > ordinary_max_ratio {
                        ordinary_max_ratio = error / tolerance;
                        ordinary_worst = json!({"query":index,"row":row,"component":c,
                            "cpu":expected[row][c],"gpu":rows[row][c],"error":error});
                    }
                }
                if error / tolerance > scaled_maximum {
                    scaled_maximum = error / tolerance;
                    worst = json!({"query": index, "row": row, "component": c, "cpu": expected[row][c], "gpu": rows[row][c], "error": error,
                    "material": {"base": i.base, "surface": i.surface}, "wo": i.wo, "wi": i.wi, "u": i.random});
                }
            }
        }
        failures += usize::from(failed);
    }
    drop(mapping);
    readback.unmap();
    report["max_abs"] = json!(maximum);
    report["max_error_tolerance_ratio"] = json!(scaled_maximum);
    report["failed_queries"] = json!(failures);
    report["ordinary_max_abs"] = json!(ordinary_max_abs);
    report["ordinary_max_error_tolerance_ratio"] = json!(ordinary_max_ratio);
    report["ordinary_worst"] = ordinary_worst;
    report["singular_pdf_max_relative"] = json!(singular_pdf_max_relative);
    report["worst"] = worst;
    let errors = errors.lock().unwrap();
    report["gpu_errors"] = json!(*errors);
    if !errors.is_empty() {
        return Err("uncaptured GPU errors".into());
    }
    if failures > 0 {
        return Err(format!(
            "{failures} BSDF queries exceeded f32/f64 tolerance"
        ));
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    let mut report = json!({"adapter": null,"queries":0,"failed_queries":null,"gpu_errors":[],"error":null,
        "abs_tolerance":ABS_TOLERANCE,"relative_tolerance":REL_TOLERANCE,"pdf_relative_tolerance":PDF_REL_TOLERANCE});
    let result = execute(&mut report);
    if let Err(e) = &result {
        report["error"] = json!(e);
    }
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
