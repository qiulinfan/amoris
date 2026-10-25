//! Independent resident regions share map phases while preserving exact per-region execution,
//! complete checkpoint validation, replay boundaries and cancellation safety.
use amoris_assets::water_field::{
    CheckedCheckpoint, ColumnDelta, FieldBoundary, FieldCheckpoint, FieldRecipe, FieldStep,
    MAX_BATCH_REGIONS, PointImpulse,
};
use amoris_render::{
    BackendChoice, Gpu,
    water_resident::{ResidentAdvance, ResidentBatchRequest, ResidentPool},
};
use std::{future::Future, sync::Arc, task::Poll};

fn make_checkpoint(size: [u32; 2], index: usize) -> Arc<CheckedCheckpoint> {
    let columns = (0..size[0] * size[1])
        .map(|n| {
            [
                0,
                (1.0 + index as f32 * 0.2 + (n % 3) as f32 * 0.03).to_bits(),
                if n % size[0] > 0 {
                    0.05f32.to_bits()
                } else {
                    0
                },
                0,
            ]
        })
        .collect();
    Arc::new(
        FieldCheckpoint::new(size, index as f64 * 10.0, columns)
            .checked()
            .unwrap(),
    )
}
fn make_recipe(checkpoint: &CheckedCheckpoint) -> FieldRecipe {
    FieldRecipe::new(checkpoint.key(), vec![], checkpoint.digest(), String::new())
}
fn step(index: usize, tick: usize) -> FieldStep {
    let mut boundaries = [FieldBoundary::Wall; 4];
    if index.is_multiple_of(2) {
        boundaries[0] = FieldBoundary::Inflow { m3s_per_m: 0.03 };
        boundaries[1] = FieldBoundary::Outflow;
    }
    FieldStep {
        exterior: vec![],
        boundaries,
        sea: None,
        cell_m: 1.0,
        substep_dt: 0.008 + tick as f64 * 0.001,
        substeps: 1 + (tick % 2) as u32,
        friction: 0.02 * index as f64,
        calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
        dry_below: 0.0,
        edits: vec![ColumnDelta {
            column: 9,
            depth_m: 0.01 * (index + tick + 1) as f64,
        }],
        impulses: vec![PointImpulse {
            at: [2.25, 3.75],
            impulse: [2.0 * (index + 1) as f64, -(tick as f64 + 1.0)],
        }],
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
fn drive<F: Future>(gpu: &Gpu, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    for _ in 0..8 {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .expect("batch async GPU progress");
    }
    panic!("resident batch did not complete its bounded phases");
}

#[test]
fn batch_matches_single_regions_through_warm_steps_replay_and_complete_checkpoints() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident batch tests require a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("resident batch validation: {e}")));
        let ids = [91, 7, 55, 2]; // Output order is the request order, independent of ID ordering.
        let mut checkpoints: Vec<_> = (0..4)
            // Open sides require dimensions divisible by four (the existing boundary contract).
            .map(|i| make_checkpoint([8 + 4 * i as u32, 8], i))
            .collect();
        let mut recipes: Vec<_> = checkpoints.iter().map(|c| make_recipe(c)).collect();
        let points = [[2.25, 3.75], [0.1, 0.1], [7.5, 7.5]];
        let mut batch = ResidentPool::default();
        let mut singles = ResidentPool::default();
        for tick in 0..4 {
            let steps: Vec<_> = (0..4).map(|i| step(i, tick)).collect();
            let requests: Vec<_> = (0..4)
                .map(|i| ResidentBatchRequest {
                    region: ids[i],
                    checkpoint: &checkpoints[i],
                    recipe: &recipes[i],
                    step: (!(tick == 2 && i == 1)).then_some(&steps[i]),
                    points: &points,
                    checkpoint_now: tick == 3 && i.is_multiple_of(2),
                    coupled_from: Some(1),
                })
                .collect();
            let expected: Vec<_> = requests
                .iter()
                .map(|r| {
                    let water = singles.get(&gpu, r.region, r.checkpoint.size).unwrap();
                    water.set_buoyancy_start(r.coupled_from);
                    if let Some(step) = r.step {
                        water
                            .advance(r.checkpoint, r.recipe, step, r.points, r.checkpoint_now)
                            .unwrap()
                    } else {
                        water.observe(r.checkpoint, r.recipe, r.points).unwrap()
                    }
                })
                .collect();
            let actual = if tick == 1 {
                drive(&gpu, batch.execute_batch_async(&gpu, &requests)).unwrap()
            } else {
                batch.execute_batch(&gpu, &requests).unwrap()
            };
            assert_eq!(actual.len(), expected.len());
            for (i, (a, e)) in actual.iter().zip(&expected).enumerate() {
                equal(a, e);
                recipes[i] = a.recipe.clone();
                if let Some(checkpoint) = &a.checkpoint {
                    assert_eq!(checkpoint.digest(), a.summary.digest);
                    checkpoints[i] = checkpoint.clone();
                }
            }
        }
        assert_eq!(batch.transfers(), singles.transfers());
        assert_eq!(batch.transfers().uploads, 4);
        assert_eq!(batch.transfers().field_readbacks, 2);
        let groups = batch.batch_diagnostics();
        assert_eq!(groups.sync_groups, 1);
        assert_eq!(groups.step_groups, 4);
        assert_eq!(groups.checkpoint_groups, 1);
        assert_eq!(groups.max_group_regions, 4);
        // Two regions have different multi-step histories and two now have compact checkpoints.
        // A cold cache must replay every historical step with its original parameters/inputs.
        let requests: Vec<_> = (0..4)
            .map(|i| ResidentBatchRequest {
                region: ids[i],
                checkpoint: &checkpoints[i],
                recipe: &recipes[i],
                step: None,
                points: &points,
                checkpoint_now: false,
                coupled_from: Some(1),
            })
            .collect();
        let expected = batch.execute_batch(&gpu, &requests).unwrap();
        let mut cold = ResidentPool::default();
        let actual = cold.execute_batch(&gpu, &requests).unwrap();
        for (a, e) in actual.iter().zip(&expected) {
            equal(a, e);
        }
        assert_eq!(cold.batch_diagnostics().sync_groups, 1);
        assert_eq!(cold.batch_diagnostics().step_groups, 1);
    }
}

#[test]
fn whole_selected_group_preserves_future_hits_with_seventeen_distinct_regions() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("selected group protection requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("selected group GPU validation: {e}")));
        let count = MAX_BATCH_REGIONS + 1;
        let checkpoints: Vec<_> = (0..count)
            .map(|i| {
                // The evicted pair has equal capacity but different shapes and complete face tails.
                let size = if i + 1 == count { [4, 16] } else { [8, 8] };
                let mut checkpoint = make_checkpoint(size, i).as_raw().clone();
                for (face, bits) in checkpoint.far_faces.iter_mut().enumerate() {
                    *bits = ((face as f32 - 3.0) * 0.125).to_bits();
                }
                Arc::new(checkpoint.checked().unwrap())
            })
            .collect();
        let mut recipes: Vec<_> = checkpoints.iter().map(|c| make_recipe(c)).collect();
        let points = [[2.25, 3.75], [0.1, 0.1]];
        let mut batch = ResidentPool::default();
        // Independent single-region references avoid introducing the pool's eviction policy
        // into the numerical oracle; their small fields keep total reference storage bounded.
        let mut singles: Vec<_> = (0..count).map(|_| ResidentPool::default()).collect();
        let mut buffers: Option<Vec<wgpu::Buffer>> = None;
        for round in 0..3usize {
            let uploads_before = batch.transfers().uploads;
            let evictions_before = batch.evictions();
            let steps: Vec<_> = (0..count)
                .map(|i| {
                    let mut step = step(i, round);
                    // An even number leaves the current buffer at zero after every checked replay,
                    // so handle equality below identifies the actual retained allocation.
                    step.substeps = 2;
                    step
                })
                .collect();
            for start in [0, MAX_BATCH_REGIONS] {
                let end = (start + MAX_BATCH_REGIONS).min(count);
                let requests: Vec<_> = (start..end)
                    .map(|i| ResidentBatchRequest {
                        region: i as u64 + 1,
                        checkpoint: &checkpoints[i],
                        recipe: &recipes[i],
                        step: Some(&steps[i]),
                        points: &points,
                        checkpoint_now: false,
                        coupled_from: (i + round).is_multiple_of(2).then_some(0),
                    })
                    .collect();
                let expected: Vec<_> = requests
                    .iter()
                    .enumerate()
                    .map(|(offset, r)| {
                        let water = singles[start + offset]
                            .get(&gpu, r.region, r.checkpoint.size)
                            .unwrap();
                        water.set_buoyancy_start(r.coupled_from);
                        water
                            .advance(r.checkpoint, r.recipe, r.step.unwrap(), r.points, false)
                            .unwrap()
                    })
                    .collect();
                let actual = batch.execute_batch(&gpu, &requests).unwrap();
                assert_eq!(actual.len(), expected.len());
                for (offset, (a, e)) in actual.iter().zip(&expected).enumerate() {
                    equal(a, e);
                    recipes[start + offset] = a.recipe.clone();
                }
                if start == 0 {
                    // Touch in the same order to preserve the cohort's original LRU order.
                    let current: Vec<_> = (1..=MAX_BATCH_REGIONS as u64)
                        .map(|region| {
                            batch
                                .get(&gpu, region, [8, 8])
                                .unwrap()
                                .field()
                                .unwrap()
                                .buffer()
                                .clone()
                        })
                        .collect();
                    if let Some(previous) = &buffers {
                        assert_eq!(
                            &current, previous,
                            "future hits and the recycled slot retain their buffers"
                        );
                    } else {
                        buffers = Some(current);
                    }
                } else {
                    let recycled = batch.get(&gpu, count as u64, [4, 16]).unwrap();
                    assert_eq!(
                        recycled.field().unwrap().buffer(),
                        &buffers.as_ref().unwrap()[0],
                        "the seventeenth region must reuse exactly the LRU victim's allocation"
                    );
                }
                assert!(batch.len() <= ResidentPool::MAX_REGIONS);
                assert!(batch.columns() <= ResidentPool::COLUMN_BUDGET);
            }
            assert_eq!(batch.len(), MAX_BATCH_REGIONS);
            assert_eq!(batch.columns(), MAX_BATCH_REGIONS as u64 * 8 * 8);
            if round == 0 {
                assert_eq!(batch.transfers().uploads - uploads_before, count as u64);
                assert_eq!(batch.evictions() - evictions_before, 1);
            } else {
                // The cache starts with 2..=17. Loading 1 must evict the unselected 17,
                // preserving future hits 2..=16. The final one-region cohort then reloads 17.
                // Pinning only acquired IDs instead causes seventeen cold misses per round.
                assert_eq!(batch.transfers().uploads - uploads_before, 2);
                assert_eq!(batch.evictions() - evictions_before, 2);
            }
        }
        assert_eq!(batch.resizes(), 0);
        assert_eq!(batch.transfers().field_readbacks, 0);
        assert_eq!(
            batch.batch_diagnostics().max_group_regions,
            MAX_BATCH_REGIONS
        );
    }
}

#[test]
fn retained_allocations_split_groups_and_pin_every_selected_slot() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident batch pool test requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("resident batch pool validation: {e}")));
        let checkpoint = make_checkpoint([8, 8], 0);
        let recipe = make_recipe(&checkpoint);
        let mut pool = ResidentPool::default();
        for id in 1..=4 {
            pool.get(&gpu, id, [512, 512]).unwrap();
        }
        assert_eq!(pool.columns(), ResidentPool::COLUMN_BUDGET);
        let requests: Vec<_> = (1..=8)
            .map(|region| ResidentBatchRequest {
                region,
                checkpoint: &checkpoint,
                recipe: &recipe,
                step: None,
                points: &[[2.5, 2.5]],
                checkpoint_now: false,
                coupled_from: None,
            })
            .collect();
        let actual = pool.execute_batch(&gpu, &requests).unwrap();
        assert_eq!(actual.len(), 8);
        for out in actual {
            assert_eq!(out.summary.digest, checkpoint.digest());
        }
        assert_eq!(pool.batch_diagnostics().sync_groups, 2);
        assert_eq!(pool.batch_diagnostics().step_groups, 2);
        assert_eq!(pool.batch_diagnostics().max_group_regions, 4);
        assert!(pool.columns() <= ResidentPool::COLUMN_BUDGET);
        assert!(pool.len() <= ResidentPool::MAX_REGIONS);
        assert_eq!(
            pool.transfers().uploads,
            8,
            "selected slots cannot be evicted before readback"
        );
        assert!(pool.evictions() > 0);
        let grown = make_checkpoint([32, 16], 0);
        let grown_recipe = make_recipe(&grown);
        pool.execute_batch(
            &gpu,
            &[ResidentBatchRequest {
                region: 8,
                checkpoint: &grown,
                recipe: &grown_recipe,
                step: None,
                points: &[],
                checkpoint_now: true,
                coupled_from: None,
            }],
        )
        .unwrap();
        assert_eq!(pool.resizes(), 1);
        assert!(pool.columns() <= ResidentPool::COLUMN_BUDGET);
    }
}

#[test]
fn failed_late_digest_and_cancelled_async_batch_invalidate_all_participating_caches() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident batch cancellation requires a GPU");
        gpu.device.on_uncaptured_error(Arc::new(|e| {
            panic!("resident batch cancellation validation: {e}")
        }));
        let checkpoint = make_checkpoint([8, 8], 0);
        let initial = make_recipe(&checkpoint);
        let steps: Vec<_> = (0..4).map(|i| step(i, 0)).collect();
        let requests: Vec<_> = (0..4)
            .map(|i| ResidentBatchRequest {
                region: i as u64,
                checkpoint: &checkpoint,
                recipe: &initial,
                step: Some(&steps[i]),
                points: &[[2.25, 3.75]],
                checkpoint_now: false,
                coupled_from: Some(0),
            })
            .collect();
        let mut pool = ResidentPool::default();
        let first = pool.execute_batch(&gpu, &requests).unwrap();
        let recipes: Vec<_> = first.iter().map(|out| out.recipe.clone()).collect();
        let mut broken = recipes.clone();
        broken[3].digest[0] ^= 1;
        let bad: Vec<_> = (0..4)
            .map(|i| ResidentBatchRequest {
                region: i as u64,
                checkpoint: &checkpoint,
                recipe: &broken[i],
                step: Some(&steps[i]),
                points: &[],
                checkpoint_now: false,
                coupled_from: None,
            })
            .collect();
        assert!(
            pool.execute_batch(&gpu, &bad)
                .unwrap_err()
                .contains("replay diverged")
        );
        for id in 0..4 {
            assert!(
                pool.get(&gpu, id, checkpoint.size)
                    .unwrap()
                    .field()
                    .is_err()
            );
        }
        // Replay after the failed fourth region must reproduce the first boundary everywhere.
        let observe: Vec<_> = (0..4)
            .map(|i| ResidentBatchRequest {
                region: i as u64,
                checkpoint: &checkpoint,
                recipe: &recipes[i],
                step: None,
                points: &[[2.25, 3.75]],
                checkpoint_now: false,
                coupled_from: Some(0),
            })
            .collect();
        let rebuilt = pool.execute_batch(&gpu, &observe).unwrap();
        for (actual, expected) in rebuilt.iter().zip(&first) {
            assert_eq!(actual.summary, expected.summary);
            assert_eq!(actual.samples, expected.samples);
        }
        let next: Vec<_> = (0..4)
            .map(|i| ResidentBatchRequest {
                region: i as u64,
                checkpoint: &checkpoint,
                recipe: &recipes[i],
                step: Some(&steps[i]),
                points: &[[2.25, 3.75]],
                checkpoint_now: false,
                coupled_from: Some(0),
            })
            .collect();
        let mut future = Box::pin(pool.execute_batch_async(&gpu, &next));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            future.as_mut().poll(&mut cx).is_pending(),
            "cancel while all step maps are armed"
        );
        drop(future);
        for id in 0..4 {
            assert!(
                pool.get(&gpu, id, checkpoint.size)
                    .unwrap()
                    .field()
                    .is_err()
            );
        }
        let mut cold = ResidentPool::default();
        let expected = cold.execute_batch(&gpu, &next).unwrap();
        let actual = pool.execute_batch(&gpu, &next).unwrap();
        for (a, e) in actual.iter().zip(&expected) {
            equal(a, e);
        }
    }
}

#[test]
fn recycled_slots_reauthenticate_after_branches_failed_summary_and_async_cancellation() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("recycled authentication requires a GPU");
        gpu.device
            .on_uncaptured_error(Arc::new(|e| panic!("recycled slot validation: {e}")));
        let mut raw = make_checkpoint([8, 8], 0).as_raw().clone();
        for c in &mut raw.columns {
            c[1] = 0;
            c[2] = 1.0f32.to_bits();
        }
        raw.columns[0][1] = 1.0f32.to_bits();
        let checkpoint = Arc::new(raw.checked().unwrap());
        let initial = make_recipe(&checkpoint);
        let points = [[2.5, 2.5]];
        let mut pool = ResidentPool::default();
        let initial_requests: Vec<_> = (1..=MAX_BATCH_REGIONS as u64)
            .map(|region| ResidentBatchRequest {
                region,
                checkpoint: &checkpoint,
                recipe: &initial,
                step: None,
                points: &points,
                checkpoint_now: false,
                coupled_from: Some(0),
            })
            .collect();
        let filled = pool.execute_batch(&gpu, &initial_requests).unwrap();
        let buffer = pool
            .get(&gpu, 1, checkpoint.size)
            .unwrap()
            .field()
            .unwrap()
            .buffer()
            .clone();
        let make_oldest = |pool: &mut ResidentPool| {
            for id in 2..=MAX_BATCH_REGIONS as u64 {
                pool.get(&gpu, id, [8, 8]).unwrap();
            }
        };
        make_oldest(&mut pool);
        let mut reference = ResidentPool::default();
        let ordinary = reference
            .get(&gpu, 100, checkpoint.size)
            .unwrap()
            .observe(&checkpoint, &initial, &points)
            .unwrap();
        assert_ne!(
            filled[0].samples, ordinary.samples,
            "the dry-face fixture distinguishes coupled queries"
        );
        {
            let recycled = pool.get(&gpu, 17, checkpoint.size).unwrap();
            assert!(
                recycled.field().is_err(),
                "a new region cannot inherit authentication"
            );
            assert_eq!(recycled.transfers(), Default::default());
            // The recycled slot had coupled_from=0; direct single-region observation must
            // recover the default ordinary query mode even for exactly identical input bytes.
            let actual = recycled.observe(&checkpoint, &initial, &points).unwrap();
            equal(&actual, &ordinary);
            assert_eq!(recycled.field().unwrap().buffer(), &buffer);
        }
        assert_eq!(pool.transfers().uploads, 17);
        let mut first_step = step(0, 1);
        first_step.substeps = 2;
        let first = pool
            .get(&gpu, 17, checkpoint.size)
            .unwrap()
            .advance(&checkpoint, &initial, &first_step, &points, false)
            .unwrap();
        let expected = reference
            .get(&gpu, 100, checkpoint.size)
            .unwrap()
            .advance(&checkpoint, &initial, &first_step, &points, false)
            .unwrap();
        equal(&first, &expected);
        let mut branch_step = first_step.clone();
        branch_step.edits[0].depth_m += 0.25;
        branch_step.impulses[0].impulse[0] += 3.0;
        let branch = pool
            .get(&gpu, 17, checkpoint.size)
            .unwrap()
            .advance(&checkpoint, &initial, &branch_step, &points, false)
            .unwrap();
        let expected = reference
            .get(&gpu, 100, checkpoint.size)
            .unwrap()
            .advance(&checkpoint, &initial, &branch_step, &points, false)
            .unwrap();
        equal(&branch, &expected);
        assert_eq!(
            pool.transfers().uploads,
            18,
            "a divergent branch still requires full reconstruction"
        );
        // A faulty GPU producer must invalidate the decoded summary before this slot is
        // recycled. The next region's complete upload must overwrite the bad word.
        gpu.queue
            .write_buffer(&buffer, 0, bytemuck::bytes_of(&f32::NAN));
        assert!(
            pool.execute_batch(
                &gpu,
                &[ResidentBatchRequest {
                    region: 17,
                    checkpoint: &checkpoint,
                    recipe: &branch.recipe,
                    step: None,
                    points: &points,
                    checkpoint_now: false,
                    coupled_from: Some(0),
                }]
            )
            .unwrap_err()
            .contains("invalid state")
        );
        make_oldest(&mut pool);
        let next = make_checkpoint([16, 4], 1);
        let next_initial = make_recipe(&next);
        let recycled = pool.get(&gpu, 18, next.size).unwrap();
        assert!(recycled.field().is_err());
        let next_output = pool
            .execute_batch(
                &gpu,
                &[ResidentBatchRequest {
                    region: 18,
                    checkpoint: &next,
                    recipe: &next_initial,
                    step: Some(&first_step),
                    points: &points,
                    checkpoint_now: false,
                    coupled_from: None,
                }],
            )
            .unwrap()
            .pop()
            .unwrap();
        let expected = reference
            .get(&gpu, 100, next.size)
            .unwrap()
            .advance(&next, &next_initial, &first_step, &points, false)
            .unwrap();
        equal(&next_output, &expected);
        assert_eq!(
            pool.get(&gpu, 18, next.size)
                .unwrap()
                .field()
                .unwrap()
                .buffer(),
            &buffer
        );
        let requests = [ResidentBatchRequest {
            region: 18,
            checkpoint: &next,
            recipe: &next_output.recipe,
            step: Some(&branch_step),
            points: &points,
            checkpoint_now: false,
            coupled_from: Some(0),
        }];
        let mut future = Box::pin(pool.execute_batch_async(&gpu, &requests));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(future);
        make_oldest(&mut pool);
        let recycled = pool.get(&gpu, 19, next.size).unwrap();
        assert!(recycled.field().is_err());
        let complete = pool
            .execute_batch(
                &gpu,
                &[ResidentBatchRequest {
                    region: 19,
                    checkpoint: &next,
                    recipe: &next_initial,
                    step: Some(&branch_step),
                    points: &points,
                    checkpoint_now: true,
                    coupled_from: None,
                }],
            )
            .unwrap()
            .pop()
            .unwrap();
        let expected = reference
            .get(&gpu, 100, next.size)
            .unwrap()
            .advance(&next, &next_initial, &branch_step, &points, true)
            .unwrap();
        equal(&complete, &expected);
        assert_eq!(
            pool.get(&gpu, 19, next.size)
                .unwrap()
                .field()
                .unwrap()
                .buffer(),
            &buffer
        );
        assert_eq!(pool.transfers().uploads, 20);
        assert_eq!(pool.transfers().field_readbacks, 1);
        make_oldest(&mut pool);
        let exact = complete.checkpoint.as_ref().unwrap();
        let observed = pool
            .get(&gpu, 20, next.size)
            .unwrap()
            .observe(exact, &complete.recipe, &points)
            .unwrap();
        assert_eq!(observed.summary, complete.summary);
        assert_eq!(observed.samples, complete.samples);
        assert_eq!(pool.transfers().uploads, 21);
        assert_eq!(
            pool.transfers().field_readbacks,
            1,
            "retiring recycled counters must not count them twice"
        );
        assert_eq!(pool.evictions(), 4);
        assert_eq!(pool.resizes(), 0);
        assert_eq!(pool.len(), MAX_BATCH_REGIONS);
        assert_eq!(pool.columns(), MAX_BATCH_REGIONS as u64 * 64);
    }
}

#[test]
fn malformed_batches_reject_before_gpu_work() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("resident batch bounds require a GPU");
        let checkpoint = make_checkpoint([8, 8], 0);
        let recipe = make_recipe(&checkpoint);
        let mut pool = ResidentPool::default();
        assert!(pool.execute_batch(&gpu, &[]).is_err());
        let request = |region| ResidentBatchRequest {
            region,
            checkpoint: &checkpoint,
            recipe: &recipe,
            step: None,
            points: &[],
            checkpoint_now: false,
            coupled_from: None,
        };
        assert!(
            pool.execute_batch(&gpu, &[request(1), request(1)])
                .unwrap_err()
                .contains("duplicate")
        );
        let too_many: Vec<_> = (0..=MAX_BATCH_REGIONS)
            .map(|id| request(id as u64))
            .collect();
        assert!(pool.execute_batch(&gpu, &too_many).is_err());
        let large = make_checkpoint([512, 512], 0);
        let large_recipe = make_recipe(&large);
        let too_large: Vec<_> = (0..5)
            .map(|region| ResidentBatchRequest {
                region,
                checkpoint: &large,
                recipe: &large_recipe,
                step: None,
                points: &[],
                checkpoint_now: false,
                coupled_from: None,
            })
            .collect();
        assert!(
            pool.execute_batch(&gpu, &too_large)
                .unwrap_err()
                .contains("column budget")
        );
        let mut invalid = request(1);
        invalid.points = &[[f32::NAN, 0.0]];
        assert!(pool.execute_batch(&gpu, &[invalid]).is_err());
        let mut invalid = request(1);
        invalid.coupled_from = Some(1);
        assert!(pool.execute_batch(&gpu, &[invalid]).is_err());
        assert!(pool.is_empty());
        assert_eq!(pool.transfers(), Default::default());
        assert_eq!(pool.batch_diagnostics(), Default::default());
    }
}
