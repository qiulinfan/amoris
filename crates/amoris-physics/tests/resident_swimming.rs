//! Scheduling and field-contact tests with a static identity host. This host does no fluid
//! transport; actual native/browser GPU coverage is provided by the app's swimming checks.

use amoris_physics::{
    BuoyPoint, Floater, Transform, Walker,
    water::{
        self,
        compute::{
            ComputeQueue, Job, ResidentBatchJob, ResidentOutput, ResidentResult, ResidentStats,
        },
        feedback::WaterFeedback,
        resident::WaterProbe,
    },
};
use amoris_sim::{EntityId, NoHooks, Sim, SimConfig, entity};

fn world(depth: f64, feedback: bool) -> (Sim, ComputeQueue, EntityId) {
    let mut sim = Sim::new(SimConfig {
        rate: amoris_sim::TickRate::DEFAULT,
        seed: 7,
    })
    .unwrap();
    amoris_physics::plugin(&mut sim).unwrap();
    let queue = ComputeQueue::default();
    water::compute::install(&mut sim, queue.backend());
    let mut region = water::dam_scene(1);
    region.solver = water::Solver::GpuResident;
    region.cell_m = 1.0;
    for tile in &mut region.tiles {
        tile.bed.fill(0.0);
        tile.depth.fill(depth);
        tile.u.fill(0.0);
        tile.w.fill(0.0);
    }
    let id = if feedback {
        sim.boundary()
            .spawn((region, WaterFeedback::default()))
            .unwrap()
    } else {
        sim.boundary().spawn((region,)).unwrap()
    };
    (sim, queue, id)
}
fn actor(sim: &mut Sim, at: [f64; 3], walker: Walker) -> EntityId {
    sim.boundary().spawn((Transform::at(at), walker)).unwrap()
}
fn pose(sim: &Sim, id: EntityId) -> Transform {
    *sim.world()
        .get::<Transform>(entity::require(sim.world(), id).unwrap())
        .unwrap()
}
fn walker(sim: &Sim, id: EntityId) -> Walker {
    *sim.world()
        .get::<Walker>(entity::require(sim.world(), id).unwrap())
        .unwrap()
}
fn take(queue: &ComputeQueue) -> ResidentBatchJob {
    let Job::Resident(batch) = queue.take_job().unwrap().expect("resident batch") else {
        panic!("grid job");
    };
    *batch
}
fn complete(queue: &ComputeQueue, batch: &ResidentBatchJob) -> bool {
    let rows = batch
        .requests
        .iter()
        .map(|request| {
            let mut recipe = request.recipe.clone();
            if let Some(step) = &request.step {
                recipe.push_step(step.clone());
            }
            recipe.environment = "static swimming test host".into();
            let field = &request.checkpoint;
            let samples = request
                .points
                .iter()
                .map(|point| {
                    let x = usize::try_from(point[0].floor() as u64).unwrap();
                    let z = usize::try_from(point[1].floor() as u64).unwrap();
                    let column = field.columns[x + z * field.size[0] as usize]
                        .map(f32::from_bits)
                        .map(f64::from);
                    [
                        field.datum + column[0] + column[1],
                        column[1],
                        column[2],
                        column[3],
                    ]
                })
                .collect();
            let depths = field
                .columns
                .iter()
                .map(|c| f64::from(f32::from_bits(c[1])));
            let stats = ResidentStats {
                max_depth: depths.clone().fold(0.0, amoris_sim::math::max),
                volume_cells: depths.sum(),
                ..Default::default()
            };
            let checkpoint = if recipe.steps().len() == amoris_assets::water_field::MAX_STEPS {
                recipe.reset(field.key(), field.digest());
                Some(field.clone())
            } else {
                None
            };
            ResidentResult {
                region: request.region,
                output: ResidentOutput {
                    recipe,
                    stats,
                    samples,
                    checkpoint,
                    reaction: Default::default(),
                    boundary: Default::default(),
                },
            }
        })
        .collect();
    queue.complete_resident(batch.id, Ok(rows)).unwrap()
}
fn step(sim: &mut Sim, queue: &ComputeQueue) -> usize {
    match sim.poll_step(&mut NoHooks).unwrap() {
        std::task::Poll::Ready(report) => {
            assert!(report.errors.is_empty());
            0
        }
        std::task::Poll::Pending => {
            let batch = take(queue);
            let queries = batch.requests.iter().map(|r| r.points.len()).sum();
            assert!(complete(queue, &batch));
            let std::task::Poll::Ready(report) = sim.poll_step(&mut NoHooks).unwrap() else {
                panic!("completed tick");
            };
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            queries
        }
    }
}

#[test]
fn swimming_waits_for_current_tick_column_and_preserves_controls() {
    let (mut sim, queue, _) = world(2.0, false);
    let id = actor(
        &mut sim,
        [8.25, 1.7, 8.75],
        Walker {
            rise: true,
            forward: 1.0,
            ..Default::default()
        },
    );
    let before = pose(&sim, id);
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_pending());
    let batch = take(&queue);
    assert_eq!(
        batch.requests[0].points,
        [[8.5, 8.5]],
        "exact current column centre"
    );
    assert_eq!(batch.requests[0].coupled_from, None);
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_pending());
    assert!(queue.take_job().unwrap().is_none(), "do not queue twice");
    assert_eq!(pose(&sim, id), before);
    assert!(!walker(&sim, id).in_liquid);
    complete(&queue, &batch);
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_ready());
    let after = pose(&sim, id);
    let w = walker(&sim, id);
    assert!(w.in_liquid && w.eye_in_liquid);
    assert_eq!(w.vertical_mps, w.swim_mps);
    assert!((after.position[1] - before.position[1] - w.swim_mps / 60.0).abs() < 1e-12);
    assert!((after.position[2] - before.position[2] + w.speed_mps * 0.6 / 60.0).abs() < 1e-12);
    let e = entity::require(sim.world(), batch.requests[0].region).unwrap();
    assert!(
        sim.world()
            .get::<water::WaterRegion>(e)
            .unwrap()
            .tiles
            .is_empty()
    );
}

#[test]
fn dry_owned_column_and_an_outside_walker_do_not_use_other_water() {
    let (mut sim, queue, _) = world(0.0, false);
    sim.boundary()
        .spawn((amoris_physics::Sea {
            level: 10.0,
            ..Default::default()
        },))
        .unwrap();
    let inside = actor(&mut sim, [8.0, 1.7, 8.0], Walker::default());
    let outside = actor(&mut sim, [20.0, 1.7, 20.0], Walker::default());
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_pending());
    let batch = take(&queue);
    assert_eq!(
        batch.requests[0].points.len(),
        1,
        "outside actor allocates no field query"
    );
    complete(&queue, &batch);
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_ready());
    assert!(!walker(&sim, inside).in_liquid);
    assert!(!walker(&sim, outside).in_liquid);
    assert!(walker(&sim, inside).vertical_mps < -0.5);
}

#[test]
fn probes_walkers_and_buoys_share_order_and_feedback_split() {
    let (mut sim, queue, region) = world(2.0, true);
    let e = entity::require(sim.world(), region).unwrap();
    {
        let mut field = sim.world_mut().get_mut::<water::WaterRegion>(e).unwrap();
        field.tiles[0].depth[2 + 3 * 16] = 0.0;
        field.tiles[0].depth[6 + 7 * 16] = 0.0;
    }
    let probe = sim
        .boundary()
        .spawn((WaterProbe {
            region: Some(region),
            at: [2.25, 3.25],
            ..Default::default()
        },))
        .unwrap();
    let swimmer = actor(&mut sim, [4.25, 1.7, 5.75], Walker::default());
    let buoy = sim
        .boundary()
        .spawn((
            Transform::at([6.25, 1.7, 7.75]),
            Floater {
                points: vec![BuoyPoint {
                    at: [0.0; 3],
                    volume: 0.1,
                    size: 0.2,
                }],
                drag: 0.0,
                heave: 0.0,
                submerged: 0.0,
            },
        ))
        .unwrap();
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_pending());
    let batch = take(&queue);
    assert_eq!(
        batch.requests[0].points,
        [[2.25, 3.25], [4.5, 5.5], [6.25, 7.75]]
    );
    assert_eq!(
        batch.requests[0].coupled_from,
        Some(2),
        "only buoyancy uses supported-face feedback sampling"
    );
    assert!(complete(&queue, &batch));
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_ready());
    assert!(walker(&sim, swimmer).in_liquid);
    assert_eq!(
        sim.world()
            .get::<WaterProbe>(entity::require(sim.world(), probe).unwrap())
            .unwrap()
            .depth,
        0.0
    );
    assert_eq!(
        sim.world()
            .get::<Floater>(entity::require(sim.world(), buoy).unwrap())
            .unwrap()
            .submerged,
        0.0,
        "buoy receipt begins after both probe and Walker rows"
    );
}

#[test]
fn the_shared_point_limit_includes_walkers_before_submission() {
    let (mut sim, queue, region) = world(2.0, false);
    for _ in 0..amoris_assets::water_field::MAX_SAMPLES {
        sim.boundary()
            .spawn((WaterProbe {
                region: Some(region),
                at: [2.0, 3.0],
                ..Default::default()
            },))
            .unwrap();
    }
    actor(&mut sim, [8.0, 1.7, 8.0], Walker::default());
    let error = sim.poll_step(&mut NoHooks).unwrap_err();
    assert_eq!(error.code, "water.resident_failed");
    assert!(error.message.contains("4096"));
    assert!(queue.take_job().unwrap().is_none());
}

#[test]
fn sleeping_column_data_rebinds_pose_and_invalidates_with_restore_hook() {
    let (mut sim, queue, _) = world(2.0, false);
    let id = actor(
        &mut sim,
        [8.0, 1.7, 8.0],
        Walker {
            gravity_mps2: 0.0,
            ..Default::default()
        },
    );
    for _ in 0..60 {
        step(&mut sim, &queue);
    }
    assert!(
        sim.poll_step(&mut NoHooks).unwrap().is_ready(),
        "sleeping unchanged field and actor reuse"
    );
    assert!(queue.take_job().unwrap().is_none());
    let e = entity::require(sim.world(), id).unwrap();
    sim.world_mut().get_mut::<Transform>(e).unwrap().position[1] += 0.1;
    assert!(
        sim.poll_step(&mut NoHooks).unwrap().is_ready(),
        "vertical pose change rebinds the same sleeping column data"
    );
    assert!(queue.take_job().unwrap().is_none());
    sim.world_mut().get_mut::<Transform>(e).unwrap().position[0] += 1.0;
    assert!(
        sim.poll_step(&mut NoHooks).unwrap().is_pending(),
        "a different query column needs fresh data"
    );
    let old = take(&queue);
    assert!(old.requests[0].step.is_none());
    water::compute::invalidate(sim.world_mut());
    assert!(!complete(&queue, &old), "restore invalidates late callback");
    // This is the same hook persistence runs after replacing the world. It must also discard
    // derived observations, so even this sleeping field cannot reuse the previous receipt.
    let state = sim
        .world()
        .get::<water::resident::WaterResident>(
            entity::require(sim.world(), old.requests[0].region).unwrap(),
        )
        .unwrap();
    assert_eq!(state.still, 60);
}

#[test]
fn actor_motion_during_wait_is_rejected_instead_of_consuming_old_pose() {
    let (mut sim, queue, _) = world(2.0, false);
    let id = actor(&mut sim, [8.0, 1.7, 8.0], Walker::default());
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_pending());
    let batch = take(&queue);
    let e = entity::require(sim.world(), id).unwrap();
    // Deliberately violate the external boundary to exercise the consumer's identity check.
    sim.world_mut().get_mut::<Transform>(e).unwrap().position[1] += 0.1;
    assert!(complete(&queue, &batch));
    let error = sim.poll_step(&mut NoHooks).unwrap_err();
    assert_eq!(error.code, "water.swimming_failed");
    assert!(error.message.contains("current-pose"));
}

#[test]
fn failed_sample_job_stops_before_actor_integration() {
    let (mut sim, queue, _) = world(2.0, false);
    let id = actor(
        &mut sim,
        [8.0, 1.7, 8.0],
        Walker {
            rise: true,
            ..Default::default()
        },
    );
    let before = pose(&sim, id);
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_pending());
    let batch = take(&queue);
    assert!(
        queue
            .complete_resident(
                batch.id,
                Err(amoris_contract::Problem::new(
                    "water.test_failure",
                    "sample failed",
                    amoris_contract::detail([])
                ))
            )
            .unwrap()
    );
    assert_eq!(
        sim.poll_step(&mut NoHooks).unwrap_err().code,
        "water.test_failure"
    );
    assert_eq!(pose(&sim, id), before);
    assert!(!walker(&sim, id).in_liquid);
    assert_eq!(
        sim.poll_step(&mut NoHooks).unwrap_err().code,
        "sim.world_poisoned"
    );
}

#[test]
fn cpu_and_full_readback_overlap_keeps_additive_current_contact() {
    let (mut sim, _, region_id) = world(2.0, false);
    let e = entity::require(sim.world(), region_id).unwrap();
    let mut region = sim.world().get::<water::WaterRegion>(e).unwrap().clone();
    region.solver = water::Solver::Cpu;
    for tile in &mut region.tiles {
        tile.u.fill(1.0);
    }
    *sim.world_mut().get_mut::<water::WaterRegion>(e).unwrap() = region.clone();
    let second = sim.boundary().spawn((region,)).unwrap();
    let id = actor(
        &mut sim,
        [8.0, 1.7, 8.0],
        Walker {
            gravity_mps2: 0.0,
            ..Default::default()
        },
    );
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_ready());
    let sum: f64 = [region_id, second]
        .map(|id| {
            sim.world()
                .get::<water::WaterRegion>(entity::require(sim.world(), id).unwrap())
                .unwrap()
                .velocity_at(8, 8)[0]
        })
        .iter()
        .sum();
    assert!((pose(&sim, id).position[0] - 8.0 - sum / 60.0).abs() < 1e-12);
    assert!(
        sum > 1.9,
        "the existing two-field current sum remains active"
    );
}

#[test]
fn sleeping_rebind_matches_fresh_host_bits_with_fewer_queries() {
    let (mut rebound, rebound_queue, _) = world(2.0, false);
    let (mut fresh, fresh_queue, _) = world(2.0, false);
    let setup = Walker {
        gravity_mps2: 0.0,
        ..Default::default()
    };
    let rebound_id = actor(&mut rebound, [8.2, 1.7, 8.2], setup);
    let fresh_id = actor(&mut fresh, [8.2, 1.7, 8.2], setup);
    for _ in 0..60 {
        assert_eq!(step(&mut rebound, &rebound_queue), 1);
        assert_eq!(step(&mut fresh, &fresh_queue), 1);
    }
    let mut rebound_queries = 0;
    let mut fresh_queries = 0;
    for frame in 0u32..80 {
        let phase = usize::try_from(frame / 10).unwrap();
        let x = if phase == 5 {
            9.2
        } else if phase == 6 {
            9.8
        } else {
            8.2 + f64::from(frame % 2) * 0.6
        };
        let y = [1.7, 2.1, 3.7, 1.7, 2.1, 1.7, 1.7, 2.1][phase];
        let yaw = f64::from(frame) * 0.07;
        for (sim, id) in [(&mut rebound, rebound_id), (&mut fresh, fresh_id)] {
            let e = entity::require(sim.world(), id).unwrap();
            *sim.world_mut().get_mut::<Transform>(e).unwrap() = Transform {
                position: [x, y, 8.2],
                rotation: Walker { yaw, ..setup }.rotation(),
            };
            let mut w = sim.world_mut().get_mut::<Walker>(e).unwrap();
            w.yaw = yaw;
            w.vertical_mps = 0.0;
            w.rise = frame % 3 == 0;
        }
        // This is a control path only: clearing derived state forces the identical static host
        // query every tick without changing persisted field or actor state.
        water::compute::invalidate(fresh.world_mut());
        let queries = step(&mut rebound, &rebound_queue);
        assert_eq!(
            queries,
            usize::from(frame == 50 || frame == 70),
            "only crossed columns need new data at frame {frame}"
        );
        rebound_queries += queries;
        fresh_queries += step(&mut fresh, &fresh_queue);
        let expected =
            bincode::serialize(&(pose(&fresh, fresh_id), walker(&fresh, fresh_id))).unwrap();
        let actual =
            bincode::serialize(&(pose(&rebound, rebound_id), walker(&rebound, rebound_id)))
                .unwrap();
        assert_eq!(
            actual, expected,
            "every actor float bit and flag at frame {frame}"
        );
        let w = walker(&rebound, rebound_id);
        assert_eq!(w.in_liquid, y < 3.57);
        assert_eq!(w.eye_in_liquid, y < 2.0);
    }
    assert_eq!(rebound_queries, 2);
    assert_eq!(fresh_queries, 80);
    water::compute::invalidate(rebound.world_mut());
    assert_eq!(
        step(&mut rebound, &rebound_queue),
        1,
        "restore hook clears reusable data even for the same field and column"
    );
}

#[test]
fn changed_checkpoint_field_queries_before_rebinding_sleeping_actor() {
    let (mut sim, queue, region) = world(2.0, false);
    let id = actor(
        &mut sim,
        [8.2, 2.2, 8.2],
        Walker {
            gravity_mps2: 0.0,
            ..Default::default()
        },
    );
    for _ in 0..60 {
        step(&mut sim, &queue);
    }
    assert_eq!(step(&mut sim, &queue), 0);
    assert!(!walker(&sim, id).eye_in_liquid);
    let field = std::sync::Arc::new(
        amoris_assets::water_field::FieldCheckpoint::new(
            [16, 16],
            0.0,
            vec![[1.0f32.to_bits(), 2.0f32.to_bits(), 0, 0]; 256],
        )
        .checked()
        .unwrap(),
    );
    let e = entity::require(sim.world(), region).unwrap();
    sim.world_mut()
        .get_mut::<water::resident::WaterResident>(e)
        .unwrap()
        .recipe
        .reset(field.key(), field.digest());
    sim.world_mut()
        .resource_mut::<water::resident::WaterCheckpoints>()
        .fields
        .insert(field.key(), field);
    assert!(
        sim.poll_step(&mut NoHooks).unwrap().is_pending(),
        "a changed checkpoint/digest cannot reuse old column data"
    );
    let batch = take(&queue);
    assert!(
        batch.requests[0].step.is_none(),
        "only the field identity changed; this is a fresh observation"
    );
    assert!(complete(&queue, &batch));
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_ready());
    assert!(
        walker(&sim, id).eye_in_liquid,
        "fresh column surface is now one metre higher"
    );
    assert_eq!(
        step(&mut sim, &queue),
        0,
        "the newly authenticated unchanged column is reusable"
    );
}

#[test]
fn warm_cache_pending_query_still_rejects_changed_actor_pose() {
    let (mut sim, queue, _) = world(2.0, false);
    let id = actor(
        &mut sim,
        [8.2, 1.7, 8.2],
        Walker {
            gravity_mps2: 0.0,
            ..Default::default()
        },
    );
    for _ in 0..60 {
        step(&mut sim, &queue);
    }
    let e = entity::require(sim.world(), id).unwrap();
    sim.world_mut().get_mut::<Transform>(e).unwrap().position[0] += 1.0;
    assert!(sim.poll_step(&mut NoHooks).unwrap().is_pending());
    let batch = take(&queue);
    assert!(batch.requests[0].step.is_none());
    sim.world_mut().get_mut::<Transform>(e).unwrap().position[1] += 0.1;
    assert!(complete(&queue, &batch));
    assert_eq!(
        sim.poll_step(&mut NoHooks).unwrap_err().code,
        "water.swimming_failed",
        "pending data cannot be rebound to a pose it did not request"
    );
}

#[test]
fn edits_force_sleeping_field_advance_even_with_rebindable_column_data() {
    let (mut sim, queue, region) = world(2.0, false);
    actor(
        &mut sim,
        [8.2, 1.7, 8.2],
        Walker {
            gravity_mps2: 0.0,
            ..Default::default()
        },
    );
    for _ in 0..60 {
        step(&mut sim, &queue);
    }
    assert_eq!(step(&mut sim, &queue), 0);
    sim.boundary()
        .spawn((water::WaterSource {
            region: Some(region),
            at: [8.2, 8.2],
            rate_m3s: 1.0,
        },))
        .unwrap();
    assert!(
        sim.poll_step(&mut NoHooks).unwrap().is_pending(),
        "cached column data cannot suppress an edit/advance"
    );
    let batch = take(&queue);
    let step = batch.requests[0]
        .step
        .as_ref()
        .expect("the edit must advance the field");
    assert_eq!(step.edits.len(), 1);
    assert_eq!(batch.requests[0].points, [[8.5, 8.5]]);
    // Only inspect the owned request: the static identity host deliberately omits transport.
}
