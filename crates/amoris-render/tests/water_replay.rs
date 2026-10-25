//! Cold reconstruction submits each historical step; its oracle advances one warm step at a time.
//! Compare complete state and current-step receipts, not two instances of the cold replay path.
use amoris_assets::water_field::{
    CheckedCheckpoint, ColumnDelta, FieldBoundary, FieldCheckpoint, FieldRecipe, FieldSea,
    FieldSeaWave, FieldStep, PointImpulse,
};
use amoris_render::{
    BackendChoice, Gpu,
    water_resident::{ResidentAdvance, ResidentWater},
};
use std::{future::Future, sync::Arc, task::Poll};

fn checkpoint(size: [u32; 2]) -> Arc<CheckedCheckpoint> {
    let mut checkpoint = FieldCheckpoint::new(
        size,
        8.0,
        (0..size[0] * size[1])
            .map(|n| {
                [
                    ((n % 3) as f32 * 0.02).to_bits(),
                    (1.2 + (n % 7) as f32 * 0.01).to_bits(),
                    if n % size[0] == 0 {
                        0
                    } else {
                        0.03f32.to_bits()
                    },
                    if n < size[0] { 0 } else { (-0.02f32).to_bits() },
                ]
            })
            .collect(),
    );
    checkpoint.far_faces.fill(0.0125f32.to_bits());
    Arc::new(checkpoint.checked().unwrap())
}

fn initial(checkpoint: &CheckedCheckpoint) -> FieldRecipe {
    FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new())
}

fn step(tick: usize) -> FieldStep {
    let mut boundaries = [FieldBoundary::Wall; 4];
    let sea = match tick % 3 {
        1 => {
            boundaries[0] = FieldBoundary::Inflow { m3s_per_m: 0.025 };
            boundaries[1] = FieldBoundary::Stage {
                surface_relative_m: 0.9,
            };
            None
        }
        2 => {
            boundaries[0] = FieldBoundary::Sea;
            boundaries[3] = FieldBoundary::Radiation {
                surface_relative_m: 1.1,
                outward_velocity_mps: 0.01,
            };
            Some(FieldSea {
                level_relative_m: 1.1,
                waves: vec![FieldSeaWave {
                    amplitude_m: 0.01,
                    wave_vector_rad_m: [0.025, -0.04],
                    angular_frequency_rad_s: 0.2,
                    phase_at_start_rad: tick as f64 * 0.015 - 0.4,
                }],
            })
        }
        _ => None,
    };
    FieldStep {
        exterior: vec![],
        boundaries,
        sea,
        cell_m: 1.0,
        substep_dt: 0.003 + (tick % 4) as f64 * 0.001,
        substeps: 1 + (tick % 3) as u32,
        friction: (tick % 5) as f64 * 0.017,
        calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
        dry_below: 0.0,
        edits: if tick.is_multiple_of(4) {
            vec![]
        } else {
            // Removal/addition/removal has a different answer from summing the deltas first.
            vec![
                ColumnDelta {
                    column: 27,
                    depth_m: -4.0,
                },
                ColumnDelta {
                    column: 27,
                    depth_m: 0.3,
                },
                ColumnDelta {
                    column: 27,
                    depth_m: -0.1,
                },
                ColumnDelta {
                    column: 42,
                    depth_m: 0.02,
                },
            ]
        },
        impulses: if tick.is_multiple_of(5) {
            vec![]
        } else {
            vec![
                PointImpulse {
                    at: [3.25, 4.75],
                    impulse: [0.7, -0.3],
                },
                PointImpulse {
                    at: [0.1, 0.1],
                    impulse: [-0.1, 0.2],
                },
            ]
        },
        displacements: vec![],
        physics: Default::default(),
    }
}

fn equal(actual: &ResidentAdvance, expected: &ResidentAdvance) {
    assert_eq!(actual.recipe, expected.recipe);
    assert_eq!(actual.summary, expected.summary);
    assert_eq!(actual.samples, expected.samples);
    assert_eq!(actual.reaction, expected.reaction);
    assert_eq!(actual.boundary, expected.boundary);
    assert_eq!(
        actual.checkpoint.as_ref().map(|c| c.as_raw()),
        expected.checkpoint.as_ref().map(|c| c.as_raw()),
    );
}

#[test]
fn maximum_history_matches_independent_steps_and_current_step_compaction_receipts() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("water replay requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|error| panic!("water replay: {error}")));
        let seed = checkpoint([16, 16]);
        let mut recipe = initial(&seed);
        let mut reference = ResidentWater::new(&gpu).unwrap();
        let points = [[0.1, 0.1], [3.25, 4.75], [15.5, 15.5]];
        reference.set_buoyancy_start(Some(1));
        // Check zero- and one-history reconstruction as well as the longest bounded history.
        let mut empty = ResidentWater::new(&gpu).unwrap();
        assert_eq!(
            empty
                .observe(&seed, &recipe, &points)
                .unwrap()
                .summary
                .digest,
            seed.digest()
        );
        for tick in 0..63 {
            let out = reference
                .advance(&seed, &recipe, &step(tick), &points, false)
                .unwrap();
            recipe = out.recipe;
            if tick < 2 {
                let mut cold = ResidentWater::new(&gpu).unwrap();
                cold.set_buoyancy_start(Some(1));
                assert_eq!(
                    cold.observe(&seed, &recipe, &points).unwrap().samples,
                    out.samples
                );
                assert_eq!(cold.synchronize(&seed, &recipe).unwrap(), out.summary);
            }
        }
        assert_eq!(recipe.steps().len(), 63);
        let final_step = step(63);
        let expected = reference
            .advance(&seed, &recipe, &final_step, &points, false)
            .unwrap();
        let mut cold = ResidentWater::new(&gpu).unwrap();
        cold.set_buoyancy_start(Some(1));
        let actual = cold
            .advance(&seed, &recipe, &final_step, &points, false)
            .unwrap();
        equal(&actual, &expected);
        assert!(actual.recipe.steps().is_empty());
        assert_eq!(
            actual.checkpoint.as_ref().unwrap().digest(),
            actual.summary.digest
        );
        let compact = actual.checkpoint.unwrap();
        // Re-authenticate the compacted seed and continue, including the next step's receipts.
        let expected = reference
            .advance(&compact, &actual.recipe, &step(64), &points, true)
            .unwrap();
        let actual = cold
            .advance(&compact, &actual.recipe, &step(64), &points, true)
            .unwrap();
        equal(&actual, &expected);
    }
}

#[test]
fn failed_digest_and_cancelled_replay_require_a_new_authenticated_upload() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("water replay cancellation requires a GPU");
        gpu.device.on_uncaptured_error(Arc::new(|error| {
            panic!("water replay cancellation: {error}")
        }));
        let seed = checkpoint([16, 16]);
        let mut recipe = initial(&seed);
        let mut reference = ResidentWater::new(&gpu).unwrap();
        for tick in 0..7 {
            recipe = reference
                .advance(&seed, &recipe, &step(tick), &[], false)
                .unwrap()
                .recipe;
        }
        let points = [[3.25, 4.75], [15.5, 15.5]];
        let expected = reference.observe(&seed, &recipe, &points).unwrap();
        let mut cold = ResidentWater::new(&gpu).unwrap();
        let mut wrong = recipe.clone();
        wrong.digest[0] ^= 1;
        assert!(
            cold.synchronize(&seed, &wrong)
                .unwrap_err()
                .contains("diverged")
        );
        assert!(cold.field().is_err());
        cold.synchronize(&seed, &recipe).unwrap();
        let actual = cold.observe(&seed, &recipe, &points).unwrap();
        equal(&actual, &expected);
        let uploads = cold.transfers().uploads;
        assert!(
            cold.synchronize(&seed, &wrong)
                .unwrap_err()
                .contains("diverged")
        );
        let mut future = Box::pin(cold.synchronize_async(&seed, &recipe));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
        drop(future);
        assert!(cold.field().is_err());
        cold.synchronize(&seed, &recipe).unwrap();
        assert_eq!(cold.transfers().uploads, uploads + 2);
        let actual = cold.observe(&seed, &recipe, &points).unwrap();
        equal(&actual, &expected);
    }
}

#[test]
fn a_later_historical_input_error_reauthenticates_before_recovery() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("segmented water replay requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|error| panic!("segmented water replay: {error}")));
        let seed = checkpoint([16, 16]);
        let mut recipe = initial(&seed);
        let mut reference = ResidentWater::new(&gpu).unwrap();
        for tick in 0..8 {
            recipe = reference
                .advance(&seed, &recipe, &step(tick), &[], false)
                .unwrap()
                .recipe;
        }
        let mut rejected = recipe.clone();
        let mut unavailable = step(8);
        unavailable.dry_below = 0.01; // Valid input schema; unavailable in resident execution.
        rejected.push_step(unavailable);
        let mut cold = ResidentWater::new(&gpu).unwrap();
        // Each of the preceding eight historical steps has already been submitted. A later
        // input error must leave that prefix invalid, without a final summary or acceptance.
        assert!(
            cold.synchronize(&seed, &rejected)
                .unwrap_err()
                .contains("drying")
        );
        assert!(cold.field().is_err());
        assert_eq!(cold.transfers().uploads, 1);
        assert_eq!(cold.transfers().small_readbacks, 0);
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .expect("submitted replay prefix completes");
        assert!(cold.field().is_err());
        let points = [[3.25, 4.75], [15.5, 15.5]];
        let current = step(8);
        let expected = reference
            .advance(&seed, &recipe, &current, &points, true)
            .unwrap();
        let actual = cold
            .advance(&seed, &recipe, &current, &points, true)
            .unwrap();
        assert_eq!(cold.transfers().uploads, 2);
        equal(&actual, &expected);
    }
}

#[test]
fn large_ordered_edits_survive_stepwise_history_replay_and_recovery() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("large water replay requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|error| panic!("large water replay: {error}")));
        let seed = checkpoint([256, 256]);
        let mut reference = ResidentWater::new(&gpu).unwrap();
        let mut recipe = reference
            .advance(&seed, &initial(&seed), &step(0), &[], false)
            .unwrap()
            .recipe;
        let mut large = step(1);
        large.edits = (0..256 * 256)
            .map(|column| ColumnDelta {
                column,
                depth_m: 0.01,
            })
            .collect();
        large.edits.extend([
            ColumnDelta {
                column: 27,
                depth_m: -4.0,
            },
            ColumnDelta {
                column: 27,
                depth_m: 0.3,
            },
            ColumnDelta {
                column: 27,
                depth_m: -0.1,
            },
        ]);
        // A full field of unique-column runs exercises the real allocation and ordered edits.
        recipe = reference
            .advance(&seed, &recipe, &large, &[], false)
            .unwrap()
            .recipe;
        let points = [[3.25, 4.75], [255.5, 255.5]];
        let current = step(2);
        let expected = reference
            .advance(&seed, &recipe, &current, &points, true)
            .unwrap();
        let mut cold = ResidentWater::new(&gpu).unwrap();
        let mut wrong = recipe.clone();
        wrong.digest[0] ^= 1;
        assert!(
            cold.synchronize(&seed, &wrong)
                .unwrap_err()
                .contains("diverged")
        );
        assert!(cold.field().is_err());
        // Historical steps were submitted before final failure or cancellation.
        let mut future = Box::pin(cold.synchronize_async(&seed, &recipe));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(future);
        assert!(cold.field().is_err());
        let actual = cold
            .advance(&seed, &recipe, &current, &points, true)
            .unwrap();
        equal(&actual, &expected);
        assert_eq!(cold.transfers().uploads, 3);
    }
}
