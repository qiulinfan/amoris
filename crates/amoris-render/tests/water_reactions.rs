//! Isolated MAC impulse budget. Transport/friction have not run when the budget is checked.
use amoris_assets::water_field::PointImpulse;
use amoris_render::{BackendChoice, Gpu, water_compute::GpuWater, water_reactions::WaterReactions};

fn momentum(columns: &[[f32; 4]], size: [u32; 2], cell: f64) -> [f64; 2] {
    let mut sum = [0.0; 2];
    let nx = size[0] as usize;
    for (n, c) in columns.iter().enumerate() {
        if n % nx > 0 {
            sum[0] +=
                f64::from(c[1] + columns[n - 1][1]) * 0.5 * 1000.0 * cell * cell * f64::from(c[2]);
        }
        if n >= nx {
            sum[1] +=
                f64::from(c[1] + columns[n - nx][1]) * 0.5 * 1000.0 * cell * cell * f64::from(c[3]);
        }
    }
    sum
}
#[test]
fn point_impulses_preserve_water_and_close_the_face_momentum_budget() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("reaction tests need a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("reaction GPU validation: {e}")
        }));
        let size = [16, 16];
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        let mut reactions = WaterReactions::new(&gpu.device, &gpu.queue);
        for case in 0..5 {
            let mut before = vec![[0.0, 2.0, 0.0, 0.0]; 256];
            if case == 1 {
                before.fill([0.0; 4]);
            }
            if case == 2 {
                for c in &mut before {
                    c[1] = 0.0;
                }
                before[8 + 8 * 16][1] = 0.2;
            }
            if case == 3 {
                for (n, c) in before.iter_mut().enumerate() {
                    c[2] = if n % 16 > 0 { 1.25 } else { 0.0 };
                    c[3] = if n >= 16 { -0.5 } else { 0.0 };
                }
            }
            let inputs = if case == 4 {
                vec![
                    PointImpulse {
                        at: [8.25, 7.75],
                        impulse: [0.002, -0.001]
                    };
                    4096
                ]
            } else {
                vec![
                    PointImpulse {
                        at: if case == 2 { [8.75, 8.2] } else { [8.25, 7.75] },
                        impulse: [10.0, -5.0],
                    },
                    PointImpulse {
                        at: [0.1, 15.9],
                        impulse: [-2.0, 3.0],
                    },
                ]
            };
            field.upload(&before, &vec![1; 256]).unwrap();
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            assert!(
                reactions
                    .encode(&mut encoder, &field, 1.0, &inputs)
                    .unwrap()
            );
            gpu.queue.submit([encoder.finish()]);
            let report = reactions.read_report().unwrap();
            let after: Vec<_> = field
                .checkpoint()
                .unwrap()
                .into_iter()
                .map(|c| c.map(f32::from_bits))
                .collect();
            let a = momentum(&before, size, 1.0);
            let b = momentum(&after, size, 1.0);
            for axis in 0..2 {
                let requested: f64 = inputs.iter().map(|p| p.impulse[axis]).sum();
                assert!(
                    (requested
                        - f64::from(
                            report.applied[axis] + report.rejected[axis] + report.roundoff[axis]
                        ))
                    .abs()
                        < 0.002,
                    "case {case} axis {axis}: requested {requested}, {report:?}"
                );
                assert!(
                    (b[axis] - a[axis] - f64::from(report.applied[axis])).abs() < 0.002,
                    "field delta must match actual report: {report:?}"
                );
            }
            for (n, (a, b)) in before.iter().zip(&after).enumerate() {
                assert_eq!(a[..2], b[..2], "impulses cannot pour or remove water");
                if n % 16 == 0 {
                    assert_eq!(b[2], 0.0);
                }
                if n < 16 {
                    assert_eq!(b[3], 0.0);
                }
            }
            if case == 1 {
                assert_eq!(after, before);
                assert_eq!(report.applied, [0.0; 2]);
            }
        }
    }
}

#[test]
fn coupled_sampling_uses_the_same_wet_face_weights_and_drag_does_not_create_wall_energy() {
    use amoris_assets::water_field::{FieldCheckpoint, FieldRecipe};
    use amoris_render::water_resident::ResidentWater;
    use std::sync::Arc;
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("coupled stencil test needs a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("coupled sampling validation: {e}")));
        let mut columns = vec![[0.0, 2.0, 0.0, 0.0]; 256];
        for (n, c) in columns.iter_mut().enumerate() {
            if n % 16 > 0 {
                c[2] = 1.0;
            }
        }
        let checkpoint = Arc::new(
            FieldCheckpoint::new(
                [16, 16],
                0.0,
                columns.iter().map(|c| c.map(f32::to_bits)).collect(),
            )
            .checked()
            .unwrap(),
        );
        let recipe = FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
        let mut resident = ResidentWater::new(&gpu).unwrap();
        resident.set_buoyancy_start(Some(1));
        let result = resident
            .observe(&checkpoint, &recipe, &[[0.5, 8.5]; 2])
            .unwrap();
        assert_eq!(
            result.samples[0][2], 0.5,
            "ordinary probes retain the old centred velocity"
        );
        assert_eq!(
            result.samples[1][2], 0.5,
            "coupled bodies retain the fixed-wall share at zero velocity"
        );
        let body_v = 0.75f64;
        let body_j = 0.01 * (f64::from(result.samples[1][2]) - body_v);
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, [16, 16]).unwrap();
        field.upload(&columns, &vec![1; 256]).unwrap();
        let energy = |data: &[[f32; 4]]| {
            data.iter()
                .enumerate()
                .filter(|(n, _)| n % 16 > 0)
                .map(|(n, c)| {
                    let mass = f64::from(c[1] + data[n - 1][1]) * 0.5 * 1000.0;
                    0.5 * mass * f64::from(c[2]).powi(2)
                })
                .sum::<f64>()
        };
        let before = energy(&columns) + 0.5 * body_v * body_v;
        let mut reactions = WaterReactions::new(&gpu.device, &gpu.queue);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        reactions
            .encode(
                &mut encoder,
                &field,
                1.0,
                &[PointImpulse {
                    at: [0.5, 8.5],
                    impulse: [-body_j, 0.0],
                }],
            )
            .unwrap();
        gpu.queue.submit([encoder.finish()]);
        let report = reactions.read_report().unwrap();
        assert!((f64::from(report.applied[0] + report.roundoff[0]) + body_j * 0.5).abs() < 1e-7);
        assert!(
            (f64::from(report.rejected[0]) + body_j * 0.5).abs() < 1e-7,
            "the other half belongs to the fixed wall"
        );
        let after: Vec<_> = field
            .checkpoint()
            .unwrap()
            .into_iter()
            .map(|c| c.map(f32::from_bits))
            .collect();
        let after = energy(&after) + 0.5 * (body_v + body_j).powi(2);
        assert!(
            after < before,
            "consistent wall stencil must dissipate drag energy: {before} -> {after}"
        );
    }
}
