//! A resident field must survive lost caches and checkpoint/program round trips, authenticate
//! actual GPU state and avoid complete readback during the normal bounded journal interval.
use amoris_assets::water_field::{ColumnDelta, FieldCheckpoint, FieldRecipe, FieldStep};
use amoris_render::{
    BackendChoice, Gpu,
    water_compute::GpuWater,
    water_resident::{ResidentPool, ResidentWater},
};
use std::sync::Arc;

#[test]
fn complete_mac_state_round_trips_and_observes_every_outside_face() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("complete MAC state requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("complete MAC validation: {e}")));
        let mut cp = FieldCheckpoint::new([16, 16], 10.0, vec![[0, 2.0f32.to_bits(), 0, 0]; 256]);
        for k in 0..16 {
            cp.columns[k * 16][2] = 2.0f32.to_bits();
        }
        for i in 0..16 {
            cp.columns[i][3] = (-3.0f32).to_bits();
        }
        cp.far_faces[..16].fill(4.0f32.to_bits());
        cp.far_faces[16..].fill(5.0f32.to_bits());
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, cp.size).unwrap();
        field.upload_checkpoint(&cp).unwrap();
        assert_eq!(field.checkpoint_complete(cp.datum).unwrap(), cp);
        let cp = Arc::new(cp.checked().unwrap());
        let recipe = FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new());
        let mut resident = ResidentWater::new(&gpu).unwrap();
        let points = [[0.5, 0.5], [15.5, 15.5]];
        let observed = resident.observe(&cp, &recipe, &points).unwrap();
        assert_eq!(observed.summary.digest, cp.digest());
        assert_eq!(observed.summary.max_speed, 5.0);
        assert_eq!(
            observed.samples,
            vec![[2.0, 2.0, 1.0, -1.5], [2.0, 2.0, 2.0, 2.5]]
        );
        resident.set_buoyancy_start(Some(0));
        assert_eq!(
            resident.observe(&cp, &recipe, &points).unwrap().samples,
            observed.samples
        );
        let mut fresh = ResidentWater::new(&gpu).unwrap();
        assert_eq!(
            fresh
                .observe(&cp, &observed.recipe, &points)
                .unwrap()
                .samples,
            observed.samples
        );
    }
}

#[test]
fn region_pool_reuses_independent_fields_and_rebuilds_evicted_or_divergent_inputs() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident pool requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("resident pool validation: {e}")));
        let checkpoint = Arc::new(
            FieldCheckpoint::new([16, 16], 0.0, vec![[0, 2.0f32.to_bits(), 0, 0]; 256])
                .checked()
                .unwrap(),
        );
        let initial =
            FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
        let mut recipes = vec![initial.clone(); ResidentPool::CAPACITY];
        let mut pool = ResidentPool::default();
        let mut reference = ResidentWater::new(&gpu).unwrap();
        let points = [[5.5, 5.5]];
        let mut saved = None;
        for tick in 0..3 {
            for (n, recipe) in recipes.iter_mut().enumerate() {
                let step = FieldStep {
                    sea: None,
                    boundaries: [amoris_assets::water_field::FieldBoundary::Wall; 4],
                    cell_m: 1.0,
                    substep_dt: 0.01,
                    substeps: 1,
                    friction: 0.02,
                    dry_below: 0.0,
                    edits: vec![ColumnDelta {
                        column: 85,
                        depth_m: 0.01 * (n + 1) as f64,
                    }],
                    impulses: vec![amoris_assets::water_field::PointImpulse {
                        at: [5.5, 5.5],
                        impulse: [(n + 1) as f64, 0.0],
                    }],
                };
                let expected = reference
                    .advance(&checkpoint, recipe, &step, &points, false)
                    .unwrap();
                let actual = pool
                    .get(&gpu, n as u64 + 1, [512, 512])
                    .unwrap()
                    .advance(&checkpoint, recipe, &step, &points, false)
                    .unwrap();
                assert_eq!(actual.recipe, expected.recipe);
                assert_eq!(actual.summary, expected.summary);
                assert_eq!(actual.samples, expected.samples);
                *recipe = actual.recipe;
            }
            if tick == 0 {
                saved = Some(recipes[0].clone());
            }
        }
        assert_eq!(pool.len(), 4);
        assert_eq!(pool.evictions(), 0);
        assert_eq!(
            pool.transfers().uploads,
            4,
            "alternating regions must not reupload/replay their history"
        );
        assert_eq!(reference.transfers().uploads, 12);
        assert_eq!(pool.transfers().field_readbacks, 0);
        // Touch 1, so 2 is the least recently used region when 5 arrives.
        pool.get(&gpu, 1, [512, 512]).unwrap();
        pool.get(&gpu, 5, [512, 512])
            .unwrap()
            .observe(&checkpoint, &initial, &points)
            .unwrap();
        assert_eq!(pool.evictions(), 1);
        pool.get(&gpu, 1, [512, 512])
            .unwrap()
            .observe(&checkpoint, &recipes[0], &points)
            .unwrap();
        assert_eq!(pool.transfers().uploads, 5);
        let rebuilt = pool
            .get(&gpu, 2, [512, 512])
            .unwrap()
            .observe(&checkpoint, &recipes[1], &points)
            .unwrap();
        assert_eq!(rebuilt.recipe, recipes[1]);
        assert_eq!(pool.evictions(), 2);
        assert_eq!(pool.transfers().uploads, 6);
        assert_eq!(pool.len(), ResidentPool::CAPACITY);
        // The same id on an older timeline is still authenticated, never treated as a cache hit.
        let old = saved.unwrap();
        let restored = pool
            .get(&gpu, 1, [512, 512])
            .unwrap()
            .observe(&checkpoint, &old, &points)
            .unwrap();
        assert_eq!(restored.recipe, old);
        assert_eq!(pool.transfers().uploads, 7);
        let continued = pool
            .get(&gpu, 1, [512, 512])
            .unwrap()
            .observe(&checkpoint, &recipes[0], &points)
            .unwrap();
        assert_eq!(continued.recipe, recipes[0]);
        let mut branch = (*old.steps()[0]).clone();
        branch.impulses[0].impulse[0] += 3.0;
        let expected = reference
            .advance(&checkpoint, &old, &branch, &points, false)
            .unwrap();
        let actual = pool
            .get(&gpu, 1, [512, 512])
            .unwrap()
            .advance(&checkpoint, &old, &branch, &points, false)
            .unwrap();
        assert_eq!(actual.summary, expected.summary);
        assert_eq!(actual.recipe, expected.recipe);
        assert_eq!(
            pool.get(&gpu, 1, [512, 512])
                .unwrap()
                .observe(&checkpoint, &recipes[0], &points)
                .unwrap()
                .recipe,
            recipes[0]
        );
    }
}

#[test]
fn bounded_readback_accepts_4096_points_and_rejects_an_oversized_batch() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident samples require a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("resident samples validation: {e}")));
        let checkpoint = Arc::new(
            FieldCheckpoint::new(
                [16, 16],
                10.0,
                vec![[0.0f32.to_bits(), 2.0f32.to_bits(), 0, 0]; 256],
            )
            .checked()
            .unwrap(),
        );
        let recipe = FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
        let points: Vec<_> = (0..amoris_assets::water_field::MAX_SAMPLES)
            .map(|n| [(n % 16) as f32 + 0.5, ((n / 16) % 16) as f32 + 0.5])
            .collect();
        let mut resident = ResidentWater::new(&gpu).unwrap();
        let out = resident.observe(&checkpoint, &recipe, &points).unwrap();
        assert_eq!(out.samples, vec![[2.0, 2.0, 0.0, 0.0]; points.len()]);
        assert_eq!(resident.transfers().field_readbacks, 0);
        assert_eq!(
            resident.transfers().readback_bytes,
            128 + points.len() as u64 * 16
        );
        let mut too_many = points.clone();
        too_many.push([0.5; 2]);
        assert!(
            resident
                .observe(&checkpoint, &recipe, &too_many)
                .unwrap_err()
                .contains("4096")
        );
        assert_eq!(
            resident
                .observe(&checkpoint, &recipe, &[[0.5; 2]])
                .unwrap()
                .samples,
            vec![[2.0, 2.0, 0.0, 0.0]]
        );
    }
}

#[test]
fn bounded_resident_execution_replays_branches_and_compacts_exact_state() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident water tests require a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("resident GPU validation: {e}")));
        gpu.device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                panic!("resident device lost: {reason:?}: {message}");
            }
        });
        let size = [32, 24];
        let n = (size[0] * size[1]) as usize;
        let mut reference: Vec<[f32; 4]> = (0..n)
            .map(|i| [0.0, if i % 32 < 16 { 2.0 } else { 0.0 }, 0.0, 0.0])
            .collect();
        let mut checkpoint = Arc::new(
            FieldCheckpoint::new(
                size,
                1_000_000.0,
                reference.iter().map(|c| c.map(f32::to_bits)).collect(),
            )
            .checked()
            .unwrap(),
        );
        let mut recipe =
            FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
        let mut resident = ResidentWater::new(&gpu).unwrap();
        let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        let active = vec![1; n];
        let mut branch = None;
        let mut step = FieldStep {
            sea: None,
            boundaries: [amoris_assets::water_field::FieldBoundary::Wall; 4],
            cell_m: 0.5,
            substep_dt: 1.0 / 240.0,
            substeps: 2,
            friction: 0.02,
            dry_below: 0.0,
            edits: vec![],
            impulses: vec![],
        };
        for tick in 1..=64 {
            step.substeps = 1 + tick % 3;
            step.edits = if tick % 11 == 0 {
                vec![
                    ColumnDelta {
                        column: 17,
                        depth_m: 0.25,
                    },
                    ColumnDelta {
                        column: 0,
                        depth_m: -100.0,
                    },
                    ColumnDelta {
                        column: 0,
                        depth_m: 0.125,
                    },
                ]
            } else {
                vec![]
            };
            for e in &step.edits {
                reference[e.column as usize][1] =
                    (reference[e.column as usize][1] + e.depth_m as f32).max(0.0);
            }
            field
                .advance_maccormack(
                    &mut reference,
                    &active,
                    0.5,
                    step.substep_dt as f32,
                    step.substeps,
                    0.02,
                )
                .unwrap();
            let out = resident
                .advance(
                    &checkpoint,
                    &recipe,
                    &step,
                    &[[15.5, 8.5], [22.25, 4.75]],
                    false,
                )
                .unwrap();
            assert_eq!(
                &out.summary.digest,
                &FieldCheckpoint::new(
                    size,
                    checkpoint.datum,
                    reference.iter().map(|c| c.map(f32::to_bits)).collect()
                )
                .digest(),
                "tick {tick}"
            );
            assert_eq!(out.samples.len(), 2);
            assert!(out.samples.iter().flatten().all(|v| v.is_finite()));
            if tick < 64 {
                assert!(out.checkpoint.is_none());
                assert_eq!(resident.transfers().field_readbacks, 0);
            }
            recipe = out.recipe;
            if let Some(c) = out.checkpoint {
                checkpoint = c;
            }
            if tick == 27 {
                let bytes = encode_json_for_test(&checkpoint, &recipe);
                branch = Some(bytes);
            }
        }
        assert!(recipe.steps().is_empty());
        assert_eq!(resident.transfers().uploads, 1);
        assert_eq!(resident.transfers().field_readbacks, 1);
        assert_eq!(
            checkpoint.columns,
            reference
                .iter()
                .map(|c| c.map(f32::to_bits))
                .collect::<Vec<_>>()
        );
        assert_eq!(checkpoint.digest(), recipe.digest);
        let (saved, program) = decode_for_test(&branch.unwrap());
        let saved = Arc::new(saved.checked().unwrap());
        program.validate(&saved).unwrap();
        resident.synchronize(&saved, &program).unwrap();
        let mut fresh = ResidentWater::new(&gpu).unwrap();
        assert_eq!(
            resident.synchronize(&saved, &program).unwrap(),
            fresh.synchronize(&saved, &program).unwrap()
        );
        let mut invalid = program.clone();
        invalid.digest[0] ^= 1;
        assert!(
            fresh
                .synchronize(&saved, &invalid)
                .unwrap_err()
                .contains("diverged")
        );
        fresh.synchronize(&saved, &program).unwrap();
        let mut other = program.clone();
        other.environment = "a different device".into();
        assert!(
            fresh
                .synchronize(&saved, &other)
                .unwrap_err()
                .contains("environment")
        );
        step.edits.clear();
        let a = resident
            .advance(&saved, &program, &step, &[], true)
            .unwrap();
        let b = fresh.advance(&saved, &program, &step, &[], true).unwrap();
        assert_eq!(a.recipe, b.recipe);
        assert_eq!(a.checkpoint, b.checkpoint);
    }
}

// The crate's existing frame codec uses bincode2; JSON here deliberately exercises an independent
// portable snapshot representation without retaining a GPU or Arc in the saved bytes.
fn encode_json_for_test(c: &FieldCheckpoint, r: &FieldRecipe) -> Vec<u8> {
    serde_json::to_vec(&(c, r)).unwrap()
}
fn decode_for_test(bytes: &[u8]) -> (FieldCheckpoint, FieldRecipe) {
    serde_json::from_slice(bytes).unwrap()
}

#[test]
fn sized_pool_retains_small_fields_and_authenticates_growth_restore_and_item_eviction() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("sized pool needs a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("sized pool validation: {e}")));
        let checkpoint = Arc::new(
            FieldCheckpoint::new(
                [128, 128],
                0.0,
                vec![[0, 2.0f32.to_bits(), 0, 0]; 128 * 128],
            )
            .checked()
            .unwrap(),
        );
        let initial =
            FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new());
        let mut recipes = vec![initial.clone(); 5];
        let mut pool = ResidentPool::default();
        let mut reference = ResidentWater::new(&gpu).unwrap();
        let points = [[5.5, 5.5]];
        for _ in 0..3 {
            for (n, recipe) in recipes.iter_mut().enumerate() {
                let step = FieldStep {
                    sea: None,
                    boundaries: [amoris_assets::water_field::FieldBoundary::Wall; 4],
                    cell_m: 1.0,
                    substep_dt: 0.01,
                    substeps: 1,
                    friction: 0.02,
                    dry_below: 0.0,
                    edits: vec![ColumnDelta {
                        column: 645,
                        depth_m: 0.01 * (n + 1) as f64,
                    }],
                    impulses: vec![],
                };
                let expected = reference
                    .advance(&checkpoint, recipe, &step, &points, false)
                    .unwrap();
                let actual = pool
                    .get(&gpu, n as u64 + 1, checkpoint.size)
                    .unwrap()
                    .advance(&checkpoint, recipe, &step, &points, false)
                    .unwrap();
                assert_eq!(
                    pool.get(&gpu, n as u64 + 1, checkpoint.size)
                        .unwrap()
                        .field()
                        .unwrap()
                        .buffer()
                        .size(),
                    128_u64 * 128 * 16 + 4096,
                    "small fields still reserve the maximum state buffer"
                );
                assert_eq!(expected.recipe, actual.recipe);
                assert_eq!(expected.summary, actual.summary);
                assert_eq!(expected.samples, actual.samples);
                *recipe = actual.recipe;
            }
        }
        assert_eq!(pool.len(), 5);
        assert_eq!(pool.columns(), 5 * 128 * 128);
        assert_eq!(pool.evictions(), 0);
        assert_eq!(pool.transfers().uploads, 5);
        let large = Arc::new(
            FieldCheckpoint::new(
                [256, 128],
                0.0,
                vec![[0, 1.0f32.to_bits(), 0, 0]; 256 * 128],
            )
            .checked()
            .unwrap(),
        );
        let large_recipe = FieldRecipe::new(large.key(), vec![], large.digest(), String::new());
        let expected = reference.observe(&large, &large_recipe, &points).unwrap();
        let actual = pool
            .get(&gpu, 1, large.size)
            .unwrap()
            .observe(&large, &large_recipe, &points)
            .unwrap();
        assert_eq!(actual.summary, expected.summary);
        assert_eq!(actual.samples, expected.samples);
        assert_eq!(pool.resizes(), 1);
        assert_eq!(pool.columns(), 6 * 128 * 128);
        let expected = reference
            .observe(&checkpoint, &recipes[0], &points)
            .unwrap();
        let actual = pool
            .get(&gpu, 1, checkpoint.size)
            .unwrap()
            .observe(&checkpoint, &recipes[0], &points)
            .unwrap();
        assert_eq!(actual.summary, expected.summary);
        assert_eq!(actual.samples, expected.samples);
        assert_eq!(pool.resizes(), 1);
        assert_eq!(
            pool.columns(),
            6 * 128 * 128,
            "a shrink must still account for reserved capacity"
        );
        // Force the item bound while staying well below the column budget. Region 2 is LRU.
        for id in 6..=17 {
            pool.get(&gpu, id, [16, 16]).unwrap();
        }
        assert_eq!(pool.len(), ResidentPool::MAX_REGIONS);
        assert_eq!(pool.evictions(), 1);
        assert!(pool.columns() < ResidentPool::COLUMN_BUDGET);
        let expected = reference
            .observe(&checkpoint, &recipes[1], &points)
            .unwrap();
        let actual = pool
            .get(&gpu, 2, checkpoint.size)
            .unwrap()
            .observe(&checkpoint, &recipes[1], &points)
            .unwrap();
        assert_eq!(actual.summary, expected.summary);
        assert_eq!(actual.samples, expected.samples);
        assert_eq!(pool.evictions(), 2);
        assert_eq!(pool.transfers().field_readbacks, 0);
        let before = (
            pool.len(),
            pool.columns(),
            pool.evictions(),
            pool.resizes(),
            pool.transfers(),
        );
        assert!(pool.get(&gpu, 99, [0, 512]).is_err());
        assert!(pool.get(&gpu, 99, [512, 513]).is_err());
        assert_eq!(
            before,
            (
                pool.len(),
                pool.columns(),
                pool.evictions(),
                pool.resizes(),
                pool.transfers()
            )
        );
        // Four maximum-capacity reservations must evict every smaller one to fit the same bound.
        for id in 100..104 {
            pool.get(&gpu, id, [512, 512]).unwrap();
            assert!(pool.columns() <= ResidentPool::COLUMN_BUDGET);
        }
        assert_eq!(pool.len(), 4);
        assert_eq!(pool.columns(), ResidentPool::COLUMN_BUDGET);
    }
}
