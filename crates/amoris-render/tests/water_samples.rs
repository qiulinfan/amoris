//! Real-device sparse asynchronous queries while physics stays resident. Every transfer is a
//! bounded point batch; a separate field with complete readback supplies the numerical reference.
use amoris_render::{
    BackendChoice, Gpu,
    water_compute::GpuWater,
    water_samples::{SampleStamp, WaterSamples},
};

fn sample(columns: &[[f32; 4]], size: [u32; 2], point: [f32; 2]) -> [f32; 4] {
    let q = [0, 1].map(|axis| (point[axis] - 0.5).clamp(0.0, (size[axis] - 1) as f32));
    let lo = q.map(|v| v.floor() as u32);
    let hi = [0, 1].map(|axis| (lo[axis] + 1).min(size[axis] - 1));
    let t = q.map(f32::fract);
    let at = |x: u32, z: u32| {
        let n = (x + size[0] * z) as usize;
        let c = columns[n];
        let u = if x + 1 < size[0] {
            columns[n + 1][2]
        } else {
            0.0
        };
        let w = if z + 1 < size[1] {
            columns[n + size[0] as usize][3]
        } else {
            0.0
        };
        [c[0] + c[1], c[1], (c[2] + u) * 0.5, (c[3] + w) * 0.5]
    };
    let [a, b, c, d] = [
        at(lo[0], lo[1]),
        at(hi[0], lo[1]),
        at(lo[0], hi[1]),
        at(hi[0], hi[1]),
    ];
    std::array::from_fn(|i| {
        let near = a[i] + (b[i] - a[i]) * t[0];
        let far = c[i] + (d[i] - c[i]) * t[0];
        near + (far - near) * t[1]
    })
}

#[test]
fn sparse_samples_name_the_sampled_tick_bound_backpressure_and_discard_old_epochs() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("water sample tests require a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("water sample validation: {e}")
        }));
        gpu.device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                panic!("water sample device lost: {reason:?}: {message}");
            }
        });
        let size = [32, 24];
        let mut columns: Vec<_> = (0..size[0] * size[1])
            .map(|n| {
                let x = n % size[0];
                let z = n / size[0];
                let bed = (x / 4 % 3) as f32 * 0.1;
                [
                    bed,
                    if x < 16 { 2.0 - bed } else { 0.0 },
                    if x > 0 {
                        (z as f32 * 0.3).sin() * 0.2
                    } else {
                        0.0
                    },
                    0.0,
                ]
            })
            .collect();
        let active = vec![1; columns.len()];
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        let mut reference = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        field.upload(&columns, &active).unwrap();
        let mut sampler = WaterSamples::new(&gpu.device, &gpu.queue, 8).unwrap();
        let points = [
            [16.5, 12.5],
            [18.25, 10.75],
            [-8.0, -2.0],
            [999.0, 999.0],
            [0.5, 0.5],
            [16.5, 12.5],
        ];
        let mut expected = Vec::new();
        for tick in 1..=60u64 {
            let substeps = 1 + tick as u32 % 3;
            reference
                .advance(&mut columns, &active, 0.5, 1.0 / 240.0, substeps, 0.02)
                .unwrap();
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            field
                .encode(&mut encoder, 0.5, 1.0 / 240.0, substeps, 0.02)
                .unwrap();
            if [10, 20, 60].contains(&tick) {
                assert!(
                    sampler
                        .request(
                            &mut encoder,
                            &field,
                            &points,
                            SampleStamp { epoch: 7, tick }
                        )
                        .unwrap()
                        .is_some()
                );
                expected.push((tick, points.map(|p| sample(&columns, size, p))));
            }
            gpu.queue.submit([encoder.finish()]);
            sampler.after_submit();
        }
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        assert!(
            sampler
                .request(
                    &mut encoder,
                    &field,
                    &points,
                    SampleStamp { epoch: 7, tick: 61 }
                )
                .unwrap()
                .is_none(),
            "three outstanding slots apply backpressure"
        );
        assert!(
            sampler
                .request(
                    &mut encoder,
                    &field,
                    &[[f32::NAN, 0.0]],
                    SampleStamp { epoch: 7, tick: 61 }
                )
                .is_err()
        );
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        let mut results = sampler.take_ready();
        results.sort_by_key(|r| r.stamp.tick);
        assert_eq!(results.len(), 3);
        for (request_id, (result, (tick, want))) in results.into_iter().zip(expected).enumerate() {
            assert_eq!(result.request_id, request_id as u64);
            assert_eq!(result.stamp, SampleStamp { epoch: 7, tick });
            let got = result.columns.unwrap();
            assert_eq!(got.len(), points.len());
            for (got, want) in got.iter().zip(want) {
                assert!(
                    got.iter().zip(want).all(|(a, b)| (*a - b).abs() < 2e-5),
                    "tick {tick}: {got:?} != {want:?}"
                );
            }
        }
        assert!(sampler.take_ready().is_empty());
        // Invalidate an old map before it completes, then immediately reuse every slot. A late
        // cancellation callback must never attach the old epoch's bytes to a new request.
        for epoch in [8, 9] {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            for index in 0..3 {
                assert!(
                    sampler
                        .request(
                            &mut encoder,
                            &field,
                            &points[index..],
                            SampleStamp { epoch, tick: 60 }
                        )
                        .unwrap()
                        .is_some()
                );
            }
            gpu.queue.submit([encoder.finish()]);
            sampler.after_submit();
            if epoch == 8 {
                sampler.clear();
            }
        }
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        let results = sampler.take_ready();
        assert_eq!(results.len(), 3);
        assert_eq!(
            results.iter().map(|r| r.request_id).collect::<Vec<_>>(),
            vec![6, 7, 8]
        );
        assert_eq!(
            results
                .iter()
                .map(|r| r.columns.as_ref().unwrap().len())
                .collect::<Vec<_>>(),
            vec![6, 5, 4]
        );
        assert!(
            results
                .iter()
                .all(|r| r.stamp.epoch == 9 && r.columns.is_ok())
        );
    }
}
