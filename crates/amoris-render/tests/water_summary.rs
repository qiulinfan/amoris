//! The 64-byte GPU result must describe actual packed state, including odd tree shapes and
//! signed-zero bits. A host-side command log is not a substitute for this field digest.
use amoris_assets::water_field::FieldCheckpoint;
use amoris_render::{
    BackendChoice, Gpu,
    water_compute::GpuWater,
    water_summary::{WaterStateSummary, WaterSummary},
};

fn capture(
    gpu: &Gpu,
    summary: &mut WaterSummary,
    field: &GpuWater,
) -> Result<WaterStateSummary, String> {
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    summary.encode(&mut encoder, field)?;
    gpu.queue.submit([encoder.finish()]);
    summary.read()
}
fn check(got: WaterStateSummary, columns: &[[f32; 4]], size: [u32; 2]) {
    check_faces(got, columns, size, &[]);
}
fn check_faces(got: WaterStateSummary, columns: &[[f32; 4]], size: [u32; 2], far_faces: &[u32]) {
    let mut expected = blake3::Hasher::new();
    expected.update(bytemuck::cast_slice(columns));
    let mut packed_faces = vec![0u32; (size[0] + size[1]).div_ceil(4) as usize * 4];
    packed_faces[..far_faces.len()].copy_from_slice(far_faces);
    expected.update(bytemuck::cast_slice(&packed_faces));
    let expected = expected.finalize();
    assert_eq!(
        &got.digest,
        expected.as_bytes(),
        "GPU BLAKE3 for {} columns",
        columns.len()
    );
    let deepest = columns.iter().map(|c| c[1]).fold(0.0, f32::max);
    let fastest = columns
        .iter()
        .map(|c| c[2].abs().max(c[3].abs()))
        .chain(far_faces.iter().map(|&bits| f32::from_bits(bits).abs()))
        .fold(0.0, f32::max);
    assert_eq!(got.max_depth, deepest);
    assert_eq!(got.max_speed, fastest);
    let volume: f64 = columns.iter().map(|c| f64::from(c[1])).sum();
    assert!((f64::from(got.volume_cells) - volume).abs() <= 1e-6 * volume.max(1.0));
    let [nx, nz] = size;
    let edge: Vec<_> = columns
        .iter()
        .enumerate()
        .filter(|(n, c)| {
            let n = *n as u32;
            let x = n % nx;
            let z = n / nx;
            c[1] > 0.01 && (x == 0 || z == 0 || x + 1 == nx || z + 1 == nz)
        })
        .map(|(_, c)| c[0] + c[1])
        .collect();
    assert_eq!(got.wet_edge_count, edge.len() as u32);
    let mean = edge.iter().map(|v| f64::from(*v)).sum::<f64>() / edge.len().max(1) as f64;
    assert!((f64::from(got.wet_edge_surface) - mean).abs() < 2e-5);
}

#[test]
fn resident_digest_matches_rust_blake3_and_summary_tracks_current_state() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident digest tests require a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("water summary GPU validation: {e}")
        }));
        gpu.device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                panic!("water summary device lost: {reason:?}: {message}");
            }
        });
        let mut summary = WaterSummary::new(&gpu.device, &gpu.queue);
        for size in [
            [2, 2],
            [5, 3],
            [16, 4],
            [17, 4],
            [31, 17],
            [64, 64],
            [128, 192],
            [512, 512],
        ] {
            eprintln!("water summary {backend:?}: {size:?}");
            let [nx, nz] = size;
            let mut columns: Vec<_> = (0..nx * nz)
                .map(|n| {
                    [
                        (n % 13) as f32 * 0.125 - 0.25,
                        0.25 + (n % 7) as f32 * 0.125,
                        if n % nx == 0 {
                            0.0
                        } else {
                            ((n % 9) as f32 - 4.0) * 0.25
                        },
                        if n < nx {
                            0.0
                        } else {
                            ((n % 5) as f32 - 2.0) * 0.125
                        },
                    ]
                })
                .collect();
            let active = vec![1; columns.len()];
            let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
            field.upload(&columns, &active).unwrap();
            let original = capture(&gpu, &mut summary, &field).unwrap();
            check(original, &columns, size);
            // Preserve signed zero and the last chunk's bytes, rather than canonicalizing floats.
            columns[0][2] = -0.0;
            columns.last_mut().unwrap()[3] += 0.125;
            field.upload(&columns, &active).unwrap();
            let changed = capture(&gpu, &mut summary, &field).unwrap();
            assert_ne!(original.digest, changed.digest);
            check(changed, &columns, size);
            if nx <= 64 {
                for substeps in [1, 2, 3] {
                    field
                        .advance(&mut columns, &active, 0.5, 1.0 / 240.0, substeps, 0.02)
                        .unwrap();
                    check(capture(&gpu, &mut summary, &field).unwrap(), &columns, size);
                }
            }
        }
        // A broken producer must be diagnosed by the GPU summary before gameplay sees it.
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, [16, 16]).unwrap();
        field
            .upload(&vec![[0.0, 1.0, 0.0, 0.0]; 256], &vec![1; 256])
            .unwrap();
        gpu.queue.write_buffer(
            field.buffer(),
            0,
            bytemuck::cast_slice(&[f32::NAN, 1.0, 0.0, 0.0]),
        );
        assert!(
            capture(&gpu, &mut summary, &field)
                .unwrap_err()
                .contains("invalid state")
        );
    }
}

#[test]
fn invalid_outside_face_bits_cannot_hide_outside_column_statistics() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("face validity requires a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("face summary validation: {e}")
        }));
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, [16, 16]).unwrap();
        field
            .upload(&vec![[0.0, 1.0, 0.0, 0.0]; 256], &[1; 256])
            .unwrap();
        // Model a faulty GPU writer: a well-typed checkpoint would reject this before upload.
        gpu.queue.write_buffer(
            field.buffer(),
            256 * 16,
            bytemuck::bytes_of(&f32::NAN.to_bits()),
        );
        let mut summary = WaterSummary::new(&gpu.device, &gpu.queue);
        assert!(capture(&gpu, &mut summary, &field).is_err());
    }
}

#[test]
fn independent_odd_trees_repeat_in_one_encoder_with_exact_complete_faces_and_one_profile_scope() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("combined summaries require a GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("combined summary GPU validation: {e}")
        }));
        gpu.device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                panic!("combined summary device lost: {reason:?}: {message}");
            }
        });
        // Five and nine BLAKE3 chunks exercise odd trees and distinct reduction parameters.
        // Both complete face tails need padding and contain nonzero/signed-zero words.
        let mut checkpoints: Vec<_> = [[17, 17], [33, 17]]
            .into_iter()
            .enumerate()
            .map(|(region, size)| {
                let columns = (0..size[0] * size[1])
                    .map(|n| {
                        [
                            ((n % 5) as f32 * 0.125).to_bits(),
                            (0.5 + ((n + region as u32) % 7) as f32 * 0.125).to_bits(),
                            if n % size[0] == 0 {
                                (-0.0f32).to_bits()
                            } else {
                                0.125f32.to_bits()
                            },
                            if n < size[0] { 0 } else { (-0.25f32).to_bits() },
                        ]
                    })
                    .collect();
                let mut checkpoint = FieldCheckpoint::new(size, region as f64 * 7.0, columns);
                for (face, bits) in checkpoint.far_faces.iter_mut().enumerate() {
                    *bits = if face % 11 == 0 {
                        (-0.0f32).to_bits()
                    } else {
                        ((face as f32 - 12.0) * 0.25).to_bits()
                    };
                }
                *checkpoint.far_faces.last_mut().unwrap() = (12.0 + region as f32).to_bits();
                checkpoint
            })
            .collect();
        let mut fields: Vec<_> = checkpoints
            .iter()
            .map(|c| GpuWater::new(&gpu.device, &gpu.queue, c.size).unwrap())
            .collect();
        let mut summaries: Vec<_> = (0..2)
            .map(|_| WaterSummary::new(&gpu.device, &gpu.queue))
            .collect();
        let mut profiler = amoris_render::profiler::GpuProfiler::new(
            &gpu.device,
            &gpu.queue,
            gpu.caps.timestamps,
            &gpu.info,
        );
        profiler.set_tracing(&gpu.device, true);
        if !gpu.caps.timestamps {
            eprintln!(
                "combined summary {backend:?}: GPU timestamps unavailable; profile scope not checked"
            );
        }
        let mut previous: Option<Vec<WaterStateSummary>> = None;
        for round in 0u64..4 {
            if round.is_multiple_of(2) {
                for (region, (field, checkpoint)) in
                    fields.iter_mut().zip(&mut checkpoints).enumerate()
                {
                    if round == 2 {
                        checkpoint.columns.last_mut().unwrap()[1] = (2.0 + region as f32).to_bits();
                        checkpoint.far_faces[1] = (-3.5 - region as f32).to_bits();
                    }
                    field.upload_checkpoint(checkpoint).unwrap();
                }
            }
            profiler.poll();
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            summaries[0]
                .encode_profiled(&mut encoder, &fields[0], Some(&mut profiler))
                .unwrap();
            summaries[1].encode(&mut encoder, &fields[1]).unwrap();
            profiler.resolve(&mut encoder);
            gpu.queue.submit([encoder.finish()]);
            profiler.after_submit();
            profiler.drain(&gpu.device);
            let results: Vec<_> = summaries.iter_mut().map(|s| s.read().unwrap()).collect();
            for (summary, checkpoint) in results.iter().zip(&checkpoints) {
                let columns: Vec<_> = checkpoint
                    .columns
                    .iter()
                    .map(|c| c.map(f32::from_bits))
                    .collect();
                check_faces(*summary, &columns, checkpoint.size, &checkpoint.far_faces);
            }
            assert_ne!(results[0].digest, results[1].digest);
            if let Some(previous) = &previous {
                if !round.is_multiple_of(2) {
                    assert_eq!(
                        &results, previous,
                        "unchanged inputs must repeat exact summaries"
                    );
                } else {
                    for (current, old) in results.iter().zip(previous) {
                        assert_ne!(
                            current.digest, old.digest,
                            "both regions must observe their changed inputs"
                        );
                    }
                }
            }
            previous = Some(results);
            if gpu.caps.timestamps {
                let recorded = profiler.take_raw();
                assert_eq!(recorded.len(), 1);
                assert_eq!(recorded[0].frame, round);
                assert_eq!(recorded[0].scopes.len(), 1);
                let scope = &recorded[0].scopes[0];
                assert_eq!(scope.label, "water summary");
                assert!(scope.end >= scope.begin);
            }
        }
    }
}
