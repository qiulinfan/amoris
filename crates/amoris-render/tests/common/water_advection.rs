//! Manufactured translating shear, away from walls: u(z,t) is a damped sine and w(t) is uniform.
//! The reference integrates only the solver's documented calm damping; it never samples a grid.
use amoris_render::{Gpu, water_compute::GpuWater};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Shear {
    pub modern: bool,
    pub side: u32,
    pub steps: u32,
    pub dt: f32,
    pub cell_m: f32,
    pub relative_l2: f64,
    pub rms_error: f64,
    pub fitted_amplitude: f64,
    pub expected_amplitude: f64,
    pub phase_error_rad: f64,
    pub roi_depth_error: f64,
    pub roi_cross_velocity_error: f64,
    pub relative_mass_error: f64,
    pub batched_wall_ms: f64,
    #[serde(skip)]
    pub profile: Vec<[f64; 3]>,
}

pub fn shear(gpu: &Gpu, side: u32, cell_m: f32, modern: bool, steps: u32) -> Result<Shear, String> {
    let dt = 0.01f32;
    let amplitude = 0.4;
    let frequency = std::f64::consts::TAU / 8.0;
    let initial: Vec<_> = (0..side * side)
        .map(|n| {
            let (i, k) = (n % side, n / side);
            [
                0.0,
                2.0,
                if i == 0 {
                    0.0
                } else {
                    (amplitude * (frequency * ((f64::from(k) + 0.5) * f64::from(cell_m))).sin())
                        as f32
                },
                if k == 0 { 0.0 } else { 1.0 },
            ]
        })
        .collect();
    let mut field = GpuWater::new(&gpu.device, &gpu.queue, [side, side])?;
    let active = vec![1; initial.len()];
    field.upload(&initial, &active)?;
    // Compile/create lazy resources outside the timed batch and wait for their first work.
    let mut warmup = gpu.device.create_command_encoder(&Default::default());
    if modern {
        field.encode_maccormack(&mut warmup, cell_m, dt, 1, 0.0)?;
    } else {
        field.encode(&mut warmup, cell_m, dt, 1, 0.0)?;
    }
    gpu.queue.submit([warmup.finish()]);
    field.checkpoint()?;
    field.upload(&initial, &active)?;
    let start = std::time::Instant::now();
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    for _ in 0..steps {
        if modern {
            field.encode_maccormack(&mut encoder, cell_m, dt, 1, 0.0)?;
        } else {
            field.encode(&mut encoder, cell_m, dt, 1, 0.0)?;
        }
    }
    gpu.queue.submit([encoder.finish()]);
    let columns: Vec<_> = field
        .checkpoint()?
        .iter()
        .map(|c| c.map(f32::from_bits))
        .collect();
    let batched_wall_ms = start.elapsed().as_secs_f64() * 1e3;
    if columns
        .iter()
        .any(|c| c[1] < 0.0 || c.iter().any(|x| !x.is_finite()))
    {
        return Err("advection produced a negative/non-finite field".into());
    }
    let mut displacement = 0.0;
    let mut cross = 1.0;
    let mut expected_amplitude = amplitude;
    for _ in 0..steps {
        displacement += cross * f64::from(dt);
        expected_amplitude /= 1.0 + 0.2 * f64::from(dt);
        cross /= 1.0 + 0.1 * f64::from(dt) / cross;
    }
    let (mut error, mut norm, mut count) = (0.0, 0.0, 0);
    let (mut roi_depth_error, mut roi_cross_velocity_error) = (0.0f64, 0.0f64);
    let (mut sine, mut cosine) = (0.0, 0.0);
    let mut profile = vec![];
    for k in side / 4..side * 3 / 4 {
        let z = (f64::from(k) + 0.5) * f64::from(cell_m);
        let expected = expected_amplitude * (frequency * (z - displacement)).sin();
        let mut row = 0.0;
        for i in side / 3..side * 2 / 3 {
            let c = columns[(i + side * k) as usize];
            error += (f64::from(c[2]) - expected).powi(2);
            norm += expected * expected;
            count += 1;
            row += f64::from(c[2]);
            roi_depth_error = roi_depth_error.max((f64::from(c[1]) - 2.0).abs());
            roi_cross_velocity_error =
                roi_cross_velocity_error.max((f64::from(c[3]) - cross).abs());
        }
        row /= f64::from(side * 2 / 3 - side / 3);
        profile.push([z, expected, row]);
        sine += row * (frequency * z).sin();
        cosine += row * (frequency * z).cos();
    }
    let phase = cosine.atan2(sine);
    let phase_difference = phase + frequency * displacement;
    let volume: f64 = columns.iter().map(|c| f64::from(c[1])).sum();
    Ok(Shear {
        modern,
        side,
        steps,
        dt,
        cell_m,
        relative_l2: (error / norm).sqrt(),
        rms_error: (error / f64::from(count)).sqrt(),
        fitted_amplitude: 2.0 * sine.hypot(cosine) / profile.len() as f64,
        expected_amplitude,
        phase_error_rad: phase_difference.sin().atan2(phase_difference.cos()),
        roi_depth_error,
        roi_cross_velocity_error,
        relative_mass_error: (volume - 2.0 * f64::from(side * side))
            / (2.0 * f64::from(side * side)),
        batched_wall_ms,
        profile,
    })
}
