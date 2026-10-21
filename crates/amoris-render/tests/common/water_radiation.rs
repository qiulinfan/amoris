//! Controlled wave-exit measurement on the actual MAC solver. Diagnostic maps are not timing.
#![allow(dead_code)]
use amoris_assets::water_field::{FieldBoundary, FieldCheckpoint};
use amoris_render::{Gpu, water_compute::GpuWater};
use serde::Serialize;

pub fn volume(cp: &FieldCheckpoint, cell: f32) -> f64 {
    cp.columns
        .iter()
        .map(|c| f64::from(f32::from_bits(c[1])))
        .sum::<f64>()
        * f64::from(cell).powi(2)
}
#[derive(Debug, Serialize)]
pub struct Pulse {
    pub side: usize,
    pub cells: u32,
    pub cell_m: f32,
    pub dt: f32,
    pub boundary: FieldBoundary,
    pub incident_energy: f64,
    pub reflected_energy: f64,
    pub apparent_amplitude_ratio: f64,
    pub max_mass_error_m3: f64,
    pub entered_m3: f64,
    pub left_m3: f64,
    pub rows: Vec<[f64; 4]>, // t, eta, incident characteristic amplitude, reflected amplitude
    pub profile: Vec<[f32; 3]>, // final coordinate, depth, normal velocity
}
pub fn pulse(gpu: &Gpu, side: usize, cells: u32, edge: FieldBoundary) -> Pulse {
    pulse_domain(gpu, side, cells, 32.0, edge)
}
pub fn pulse_domain(gpu: &Gpu, side: usize, cells: u32, length: f32, edge: FieldBoundary) -> Pulse {
    let cell = length / cells as f32;
    let size = if side < 2 { [cells, 4] } else { [4, cells] };
    let axis = usize::from(side >= 2);
    let sign = if side % 2 == 1 { 1.0f64 } else { -1.0 };
    let shape = |x: f64| 0.02 * (-0.5 * ((x - 8.0) / 1.5).powi(2)).exp();
    let c0 = 9.81f64.sqrt();
    let mut cp = FieldCheckpoint::new(
        size,
        0.0,
        (0..size[0] * size[1])
            .map(|n| {
                let ij = [n % size[0], n / size[0]];
                let coordinate = |x: f64| if sign > 0.0 { x } else { f64::from(length) - x };
                let h = 1.0 + shape(coordinate((f64::from(ij[axis]) + 0.5) * f64::from(cell)));
                let face = coordinate(f64::from(ij[axis]) * f64::from(cell));
                let v = sign * 2.0 * ((9.81 * (1.0 + shape(face))).sqrt() - c0);
                let mut r = [0.0, h as f32, 0.0, 0.0];
                r[axis + 2] = v as f32;
                if ij[axis] == 0 {
                    r[axis + 2] = 0.0;
                }
                r.map(f32::to_bits)
            })
            .collect(),
    );
    // Analytic packet tails have rounded to the still state at the outside faces.
    cp.far_faces.fill(0);
    let initial = volume(&cp, cell);
    let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
    field.upload_checkpoint(&cp).unwrap();
    let mut edges = [FieldBoundary::Wall; 4];
    edges[side] = edge;
    let dt = 0.02f32;
    let ticks = (15.0 / dt).round() as u32;
    let probe = (20.0 / cell) as u32;
    let mut entered = 0.0;
    let mut left = 0.0;
    let mut max_mass = 0.0f64;
    let mut rows = Vec::new();
    let mut incident = 0.0;
    let mut reflected = 0.0;
    let mut previous_sample_time = 0.0;
    for tick in 0..ticks {
        let mut e = gpu.device.create_command_encoder(&Default::default());
        field
            .encode_boundaries(&mut e, cell, dt, 1, 0.0, &edges)
            .unwrap();
        gpu.queue.submit([e.finish()]);
        let flow = if edge == FieldBoundary::Wall {
            Default::default()
        } else {
            field.read_boundary_report().unwrap()
        };
        entered += f64::from(flow.entered);
        left += f64::from(flow.left);
        if tick % 2 != 0 && tick + 1 != ticks {
            continue;
        }
        cp = field.checkpoint_complete(0.0).unwrap();
        max_mass = max_mass.max((volume(&cp, cell) - initial - entered + left).abs());
        let along = if sign > 0.0 { probe } else { cells - 1 - probe };
        let ij = if axis == 0 { [along, 1] } else { [1, along] };
        let n = (ij[0] + size[0] * ij[1]) as usize;
        let at = |n: usize| f64::from(f32::from_bits(cp.columns[n][axis + 2]));
        let next = if axis == 0 {
            n + 1
        } else {
            n + size[0] as usize
        };
        let u = sign * (at(n) + at(next)) * 0.5;
        let eta = f64::from(f32::from_bits(cp.columns[n][1])) - 1.0;
        let forward = 0.5 * (eta + u / c0);
        let back = 0.5 * (eta - u / c0);
        let t = f64::from(tick + 1) * f64::from(dt);
        let sample_dt = t - previous_sample_time;
        previous_sample_time = t;
        if (1.0..7.0).contains(&t) {
            incident += forward * forward * sample_dt;
        }
        if (8.0..15.01).contains(&t) {
            reflected += back * back * sample_dt;
        }
        rows.push([t, eta, forward, back]);
    }
    let profile = (0..cells)
        .map(|i| {
            let ij = if axis == 0 { [i, 1] } else { [1, i] };
            let n = (ij[0] + size[0] * ij[1]) as usize;
            [
                i as f32 * cell,
                f32::from_bits(cp.columns[n][1]),
                f32::from_bits(cp.columns[n][axis + 2]),
            ]
        })
        .collect();
    Pulse {
        side,
        cells,
        cell_m: cell,
        dt,
        boundary: edge,
        incident_energy: incident,
        reflected_energy: reflected,
        apparent_amplitude_ratio: (reflected / incident).sqrt(),
        max_mass_error_m3: max_mass,
        entered_m3: entered,
        left_m3: left,
        rows,
        profile,
    }
}

/// Boundary-generated return, after subtracting the same solver in a double-length domain.
pub fn return_difference(a: &Pulse, reference: &Pulse) -> f64 {
    assert_eq!(a.rows.len(), reference.rows.len());
    let mut sum = 0.0;
    let mut previous = 0.0;
    for (a, b) in a.rows.iter().zip(&reference.rows) {
        assert_eq!(a[0], b[0]);
        let dt = a[0] - previous;
        previous = a[0];
        if (8.0..15.01).contains(&a[0]) {
            sum += (a[3] - b[3]).powi(2) * dt;
        }
    }
    sum
}

pub fn browser_fixtures(gpu: &Gpu) -> serde_json::Value {
    let mut cases = Vec::new();
    let still = vec![[0, 1.0f32.to_bits(), 0, 0]; 256];
    let reservoir = |h: f64, u: f64| FieldBoundary::Radiation {
        surface_relative_m: h,
        outward_velocity_mps: u,
    };
    let stepped = (0..256)
        .map(|n| {
            let b = ((n * 13) % 7) as f32 * 0.1;
            [b, 1.0 - b, 0.0, 0.0].map(f32::to_bits)
        })
        .collect();
    cases.push((
        "stepped lake".to_string(),
        FieldCheckpoint::new([16, 16], 0.0, stepped),
        vec![([reservoir(1.0, 0.0); 4], 0.01f32)],
        None,
    ));
    for side in 0..4 {
        for filling in [false, true] {
            let mut edges = [FieldBoundary::Wall; 4];
            edges[side] = reservoir(if filling { 1.0 } else { 0.0 }, 0.0);
            cases.push((
                format!("sonic {side} filling={filling}"),
                FieldCheckpoint::new(
                    [16, 16],
                    0.0,
                    if filling {
                        vec![[0; 4]; 256]
                    } else {
                        still.clone()
                    },
                ),
                vec![(edges, 0.001)],
                Some((
                    filling,
                    (4.0 / 9.0) * (2.0 / 3.0) * 9.81f64.sqrt() * 16.0 * f64::from(0.001f32),
                )),
            ));
        }
        let steps = (0..12)
            .map(|n| {
                let mut edges = [FieldBoundary::Wall; 4];
                edges[side] = reservoir(
                    if n < 6 { 1.2 } else { 0.8 },
                    if n % 3 == 0 { 0.1 } else { -0.1 },
                );
                (edges, 0.01)
            })
            .collect();
        cases.push((
            format!("changed controls {side}"),
            FieldCheckpoint::new([16, 16], 0.0, still.clone()),
            steps,
            None,
        ));
    }
    for sign in [-1.0f32, 1.0] {
        let mut cp = FieldCheckpoint::new(
            [16, 16],
            0.0,
            vec![[0, 1.0f32.to_bits(), (sign * 4.0).to_bits(), 0]; 256],
        );
        cp.far_faces[..16].fill((sign * 4.0).to_bits());
        cases.push((
            format!("supercritical {sign}"),
            cp,
            vec![
                (
                    [
                        reservoir(1.0, -f64::from(sign) * 4.0),
                        reservoir(1.0, f64::from(sign) * 4.0),
                        FieldBoundary::Wall,
                        FieldBoundary::Wall
                    ],
                    0.001
                );
                3
            ],
            None,
        ));
    }
    let dry = (0..256)
        .map(|n| [((n * 13 % 7) as f32 * 0.1).to_bits(), 0, 0, 0])
        .collect();
    cases.push((
        "mixed dry/blocked corners".into(),
        FieldCheckpoint::new([16, 16], 0.0, dry),
        vec![([reservoir(0.25, 0.0); 4], 0.01); 8],
        None,
    ));
    let mut fixtures = Vec::new();
    for (name, initial, steps, analytic) in cases {
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, initial.size).unwrap();
        field.upload_checkpoint(&initial).unwrap();
        let mut parameters = Vec::new();
        let mut entered = 0.0;
        let mut left = 0.0;
        for (edges, dt) in steps {
            let mut p = vec![16, 16, 1.0f32.to_bits(), dt.to_bits(), 0, 0, 0, 0];
            let mut kind = [0; 4];
            let mut levels = [0.0f32; 4];
            let mut normals = [0.0f32; 4];
            for (i, edge) in edges.iter().enumerate() {
                match edge {
                    FieldBoundary::Radiation {
                        surface_relative_m: h,
                        outward_velocity_mps: u,
                    } => {
                        kind[i] = 4;
                        levels[i] = *h as f32;
                        normals[i] = *u as f32;
                    }
                    FieldBoundary::Wall => {}
                    _ => panic!("fixture uses unsupported boundary"),
                }
            }
            p.extend(kind);
            p.extend(levels.map(f32::to_bits));
            p.extend(normals.map(f32::to_bits));
            parameters.push(p);
            let mut e = gpu.device.create_command_encoder(&Default::default());
            field
                .encode_boundaries(&mut e, 1.0, dt, 1, 0.0, &edges)
                .unwrap();
            gpu.queue.submit([e.finish()]);
            let flow = field.read_boundary_report().unwrap();
            entered += f64::from(flow.entered);
            left += f64::from(flow.left);
        }
        let expected = field.checkpoint_complete(0.0).unwrap();
        fixtures.push(serde_json::json!({"name":name,"initial":initial,"expected":expected,"parameters":parameters,"expected_entered":entered,"expected_left":left,"analytic":analytic}));
    }
    assert_eq!(fixtures.len(), 16);
    serde_json::json!({"scope":"production MAC/characteristic kernels; native state comparisons plus independent sonic mass flux; not world Sea coupling","uniform_words":24+8*amoris_assets::water_field::MAX_SEA_WAVES,"fixtures":fixtures})
}
