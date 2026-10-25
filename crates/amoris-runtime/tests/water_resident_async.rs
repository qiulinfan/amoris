//! Scheduler/state tests with an identity host: edits apply in f32 but fluid transport is omitted.
//! Actual browser GPU tests, not this host, establish numerical correctness and transfer scope.
#![cfg(feature = "transpile")]
use amoris_assets::water_field::FieldCheckpoint;
use amoris_runtime::{
    Game, GameBuilder, Project,
    water_compute::{
        ComputeQueue, Job, ResidentBatchJob, ResidentJob, ResidentOutput, ResidentResult,
        ResidentStats, install,
    },
};
use serde_json::json;
use std::sync::Arc;

fn identity(r: &ResidentJob) -> ResidentOutput {
    identity_checkpoint(r, r.checkpoint_now)
}
fn identity_checkpoint(r: &ResidentJob, checkpoint_now: bool) -> ResidentOutput {
    let mut field = r.checkpoint.as_raw().clone();
    for step in r
        .recipe
        .steps()
        .iter()
        .map(|s| s.as_ref())
        .chain(r.step.iter())
    {
        for edit in &step.edits {
            let c = &mut field.columns[edit.column as usize];
            c[1] = amoris_sim::math::max_f32(f32::from_bits(c[1]) + edit.depth_m as f32, 0.0)
                .to_bits();
        }
    }
    let mut recipe = r.recipe.clone();
    if let Some(step) = &r.step {
        recipe.push_step(step.clone());
    }
    recipe.digest = field.digest();
    recipe.environment = "identity test host".into();
    let mut stats = ResidentStats::default();
    for c in &field.columns {
        let depth = f64::from(f32::from_bits(c[1]));
        stats.max_depth = amoris_sim::math::max(stats.max_depth, depth);
        stats.volume_cells += depth;
    }
    let samples = r
        .points
        .iter()
        .map(|p| {
            let n = p[0] as usize + p[1] as usize * field.size[0] as usize;
            let c = field.columns[n].map(f32::from_bits);
            [
                field.datum + f64::from(c[0]) + f64::from(c[1]),
                f64::from(c[1]),
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
            ]
        })
        .collect();
    let checkpoint = if recipe.steps().len() == 64 || checkpoint_now {
        recipe.reset(field.key(), field.digest());
        Some(Arc::new(field.checked().unwrap()))
    } else {
        None
    };
    // Close the protocol budget without simulating momentum transport in this scheduler host.
    let mut reaction = amoris_runtime::water_compute::ReactionBudget::default();
    if let Some(step) = &r.step {
        for input in &step.impulses {
            for a in 0..2 {
                reaction.applied[a] += f64::from(input.impulse[a] as f32);
            }
        }
    }
    ResidentOutput {
        reaction,
        boundary: Default::default(),
        recipe,
        stats,
        samples,
        checkpoint,
    }
}
fn identity_batch(batch: &ResidentBatchJob) -> Vec<ResidentResult> {
    batch
        .requests
        .iter()
        .map(|request| ResidentResult {
            region: request.region,
            output: identity(request),
        })
        .collect()
}
fn take(q: &ComputeQueue) -> ResidentBatchJob {
    match q.take_job().unwrap().expect("resident job") {
        Job::Resident(r) => *r,
        _ => panic!("grid job"),
    }
}

#[test]
fn body_queries_suspend_forces_reuse_sleeping_points_and_reset_on_restore() {
    std::thread::Builder::new().stack_size(amoris_runtime::GAME_STACK_BYTES).spawn(|| {
        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-floaters");
        let mut setup=Project::load(&root).unwrap().setup(false).unwrap();
        for e in &mut setup.scene.entities {
            if e.components.contains_key("Floater") {e.components["RigidBody"]=json!({"kind":"Fixed"});}
        }
        let q=ComputeQueue::default();let host=q.clone();
        let mut game=GameBuilder::new(Arc::new(setup)).system(move |sim| {install(sim,host.backend());Ok(())}).build().unwrap();
        let position=|game:&Game| {
            let e=amoris_sim::entity::entity(game.world(),amoris_sim::EntityId::new(3).unwrap()).unwrap();
            game.world().get::<amoris_physics::Transform>(e).unwrap().position
        };
        let before=position(&game);
        assert!(game.poll_step().unwrap().is_pending());
        let job=take(&q);assert_eq!(job.requests[0].points.len(),108);
        assert!(game.poll_step().unwrap().is_pending());assert!(q.take_job().unwrap().is_none());
        assert_eq!(position(&game),before,"no body integration before completed samples");
        q.complete_resident(job.id,Ok(identity_batch(&job))).unwrap();assert!(game.poll_step().unwrap().is_ready());
        for _ in 1..60 {step(&mut game,&q);}
        let saved=game.snapshot().unwrap();
        assert!(game.poll_step().unwrap().is_ready(),"unchanged sleeping body points need no GPU job");
        assert!(q.take_job().unwrap().is_none());
        let expected=game.world_hash().unwrap();
        game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(9),1,"world.edit",json!({"label":"submerge fixed floater","ops":[
            {"set":{"entity":"Float0","component":"Transform","value":{"position":[6.5,1.0,8.5]}}}
        ]}))).unwrap();
        assert!(game.poll_step().unwrap().is_pending());let job=take(&q);
        assert!(job.requests[0].step.is_none(),"unchanged sleeping field only needs new pose samples");
        assert_eq!(job.requests[0].points.len(),108);q.complete_resident(job.id,Ok(identity_batch(&job))).unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        let e=amoris_sim::entity::entity(game.world(),amoris_sim::EntityId::new(3).unwrap()).unwrap();
        assert_eq!(game.world().get::<amoris_physics::Floater>(e).unwrap().submerged,1.0);
        game.restore(&saved).unwrap();
        assert!(game.poll_step().unwrap().is_pending(),"restore discards derived body observations");
        let job=take(&q);assert!(job.requests[0].step.is_none());q.complete_resident(job.id,Ok(identity_batch(&job))).unwrap();
        assert!(game.poll_step().unwrap().is_ready());assert_eq!(game.world_hash().unwrap(),expected);
    }).unwrap().join().unwrap();
}
fn step(game: &mut Game, q: &ComputeQueue) {
    assert!(game.poll_step().unwrap().is_pending());
    let job = take(q);
    for _ in 0..3 {
        assert!(game.poll_step().unwrap().is_pending());
        assert!(q.take_job().unwrap().is_none());
    }
    assert!(
        q.complete_resident(job.id, Ok(identity_batch(&job)))
            .unwrap()
    );
    assert!(game.poll_step().unwrap().is_ready());
    assert_eq!(
        game.sim()
            .world()
            .resource::<amoris_physics::water::resident::WaterCheckpoints>()
            .fields
            .len(),
        1,
        "a single region must not publish both old and compacted checkpoint fields"
    );
}
#[test]
fn resident_continuations_commit_once_rebuild_branches_and_reject_bad_observations() {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../samples/water-resident");
            let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
            setup.scene.entities[2].components["WaterProbe"]["at"] = json!([0.5, 0.5]);
            setup.scene.entities.push(
                serde_json::from_value(json!({"name":"Spring","components":{
                    "WaterSource":{"region":2,"at":[0.5,0.5],"rate_m3s":60.0}
                }}))
                .unwrap(),
            );
            let q = ComputeQueue::default();
            let host = q.clone();
            let mut game = GameBuilder::new(Arc::new(setup))
                .system(move |sim| {
                    install(sim, host.backend());
                    Ok(())
                })
                .build()
                .unwrap();
            let feed = amoris_assets::Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = amoris_runtime::Extractor::default();
            game.present(&mut extractor, &feed);
            mailbox.take();
            assert!(game.poll_step().unwrap().is_pending());
            let job = take(&q);
            assert!(game.snapshot().is_err());
            assert!(game.fork().is_err());
            game.present(&mut extractor, &feed);
            assert!(mailbox.take().is_empty());
            q.complete_resident(job.id, Ok(identity_batch(&job)))
                .unwrap();
            assert!(game.poll_step().unwrap().is_ready());
            for _ in 1..27 {
                step(&mut game, &q);
            }
            let saved = game.snapshot().unwrap();
            let mut branch = game.fork().unwrap();
            let mut memo = amoris_runtime::SnapshotCache::new();
            let checkpoint_key = amoris_persist::SectionKey::new(
                amoris_persist::SectionKind::Resource,
                "WaterCheckpoints",
            );
            let mut kept_bytes = memo
                .snapshot(game.world(), game.registry())
                .unwrap()
                .section(&checkpoint_key)
                .unwrap()
                .bytes
                .clone();
            for _ in 27..70 {
                step(&mut game, &q);
                let cached = memo.snapshot(game.world(), game.registry()).unwrap();
                let bytes = &cached.section(&checkpoint_key).unwrap().bytes;
                if !game.tick().0.is_multiple_of(64) {
                    assert!(
                        Arc::ptr_eq(&kept_bytes, bytes),
                        "unchanged checkpoint was serialized again"
                    );
                }
                kept_bytes = bytes.clone();
                step(&mut branch, &q);
                assert_eq!(game.world_hash().unwrap(), branch.world_hash().unwrap());
            }
            let expected = game.world_hash().unwrap();
            game.restore(&saved).unwrap();
            for _ in 27..70 {
                step(&mut game, &q);
            }
            assert_eq!(expected, game.world_hash().unwrap());
            game.present(&mut extractor, &feed);
            let frame = mailbox.take();
            let w = &frame.resident_water[0];
            assert_eq!(w.recipe.steps.len(), 6);
            assert!(w.checkpoint.is_some());
            // Every fake-host step adds exactly one metre: resuming never reapplies the source.
            let shared = w.checkpoint.as_ref().unwrap();
            let stored = game
                .world()
                .resource::<amoris_physics::water::resident::WaterCheckpoints>()
                .fields
                .get(&w.recipe.checkpoint)
                .unwrap();
            assert!(
                Arc::ptr_eq(shared, stored),
                "publication deep-copied the immutable physics checkpoint"
            );
            let mut reconstructed: FieldCheckpoint = shared.as_raw().clone();
            for s in &w.recipe.steps {
                for e in &s.edits {
                    let c = &mut reconstructed.columns[e.column as usize];
                    c[1] = (f32::from_bits(c[1]) + e.depth_m as f32).to_bits();
                }
            }
            assert_eq!(f32::from_bits(reconstructed.columns[0][1]), 72.0);
            assert!(game.poll_step().unwrap().is_pending());
            let job = take(&q);
            let mut bad = identity_batch(&job);
            bad[0].output.samples[0][1] = -1.0;
            q.complete_resident(job.id, Ok(bad)).unwrap();
            assert_eq!(game.poll_step().unwrap_err().code, "water.resident_failed");
            game.present(&mut extractor, &feed);
            assert!(mailbox.take().is_empty());
            game.restore(&saved).unwrap();
            assert!(
                !q.complete_resident(job.id, Ok(identity_batch(&job)))
                    .unwrap()
            );
            step(&mut game, &q);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn sleeping_probes_resample_after_basis_or_region_changes_with_identical_column_digests() {
    std::thread::Builder::new().stack_size(amoris_runtime::GAME_STACK_BYTES).spawn(|| {
        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-resident");
        let mut project=Project::load(&root).unwrap();
        project.scene.entities[1].components.insert("Transform".into(),json!({"position":[0,0,0]}));
        let region=&mut project.scene.entities[1].components["WaterRegion"];
        for tile in region["tiles"].as_array_mut().unwrap() {
            let x=tile["key"][0].as_u64().unwrap() as usize*16;
            tile["bed"]=json!((0..256).map(|n|((x+n%16)%4) as f64*0.125).collect::<Vec<_>>());
            tile["depth"]=json!((0..256).map(|n|2.0-((x+n%16)%4) as f64*0.125).collect::<Vec<_>>());
        }
        project.scene.entities[2].components["WaterProbe"]["at"]=json!([1.5,0.5]);
        let mut upper=project.scene.entities[1].clone();upper.name=Some("Upper".into());
        upper.components["Transform"]=json!({"position":[1,0,0]});
        for tile in upper.components["WaterRegion"]["tiles"].as_array_mut().unwrap() {
            for bed in tile["bed"].as_array_mut().unwrap(){*bed=json!(bed.as_f64().unwrap()+10.0);}
        }
        project.scene.entities.push(upper);
        let q=ComputeQueue::default();let host=q.clone();
        let mut game=GameBuilder::new(Arc::new(project.setup(false).unwrap())).system(move|sim|{
            install(sim,host.backend());
            sim.add_exclusive("test.retarget",amoris_sim::TickPhase::Update,amoris_sim::RunCondition::Always,|world,_|{
                if world.resource::<amoris_sim::SimClock>().tick.0==63 {
                    let e=amoris_sim::entity::entity(world,amoris_sim::EntityId::new(3).unwrap()).unwrap();
                    // Rust systems may change request inputs directly, with no runtime setter.
                    world.get_mut::<amoris_physics::water::resident::WaterProbe>(e).unwrap().region=amoris_sim::EntityId::new(8);
                }
            })?;Ok(())
        }).build().unwrap();
        let drive=|game:&mut Game| {
            let mut observation_jobs=Vec::new();
            for _ in 0..8 {
                if game.poll_step().unwrap().is_ready(){return observation_jobs;}
                let job=take(&q);observation_jobs.extend(job.requests.iter().map(|r| r.step.is_none()));q.complete_resident(job.id,Ok(identity_batch(&job))).unwrap();
            }panic!("resident tick did not finish");
        };
        let probe=|game:&Game|{
            let e=amoris_sim::entity::entity(game.world(),amoris_sim::EntityId::new(3).unwrap()).unwrap();
            game.world().get::<amoris_physics::water::resident::WaterProbe>(e).unwrap().clone()
        };
        for _ in 0..60 {assert_eq!(drive(&mut game),vec![false,false]);}
        assert_eq!(probe(&game).depth,1.875);assert!(drive(&mut game).is_empty(),"unchanged sleeping requests should not submit GPU work");
        game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(9),1,"world.edit",json!({"label":"move field","ops":[{"set":{"entity":"Lake","component":"Transform","value":{"position":[1,0,0]}}}]}))).unwrap();
        assert_eq!(drive(&mut game),vec![true]);assert_eq!(probe(&game).depth,2.0);assert_eq!(probe(&game).sampled_grid,[0.5,0.5]);
        let before=probe(&game);assert_eq!(before.sampled_region,amoris_sim::EntityId::new(2));
        // Update switches regions before the water/force phase of tick 63.
        assert_eq!(drive(&mut game),vec![true]);let after=probe(&game);
        assert_eq!(before.state_digest,after.state_digest,"relative columns are deliberately identical");
        assert_eq!(before.sampled_grid,after.sampled_grid);
        assert_eq!(after.sampled_region,amoris_sim::EntityId::new(8));assert_eq!(after.surface,12.0);
        assert!(drive(&mut game).is_empty());
    }).unwrap().join().unwrap();
}

fn on_game_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

/// Uniform 16-square fields with a shared seed; all probe/source coordinates are world-space.
fn cohort_project(solvers: &[&str], rates: &[f64]) -> Project {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-resident");
    let mut project = Project::load(&root).unwrap();
    let mut region = project.scene.entities[1].clone();
    region.components["WaterRegion"]["size"] = json!([16, 16]);
    region.components["WaterRegion"]["tiles"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    project.scene.entities.truncate(1); // Keep the shared (empty) bed volume as entity 1.
    for (i, solver) in solvers.iter().enumerate() {
        let mut region = region.clone();
        region.name = Some(format!("Region{i}"));
        region.components["WaterRegion"]["solver"] = json!(solver);
        region
            .components
            .insert("Transform".into(), json!({"position":[i * 16, 0, 0]}));
        if *solver == "gpu_resident" {
            region.components.insert("WaterFeedback".into(), json!({}));
        }
        project.scene.entities.push(region);
    }
    for (i, rate) in rates.iter().enumerate() {
        project.scene.entities.push(serde_json::from_value(json!({
            "name": format!("Probe{i}"), "components": {"WaterProbe": {"region": i + 2, "at": [i as f64 * 16.0 + 0.5, 0.5]}}
        })).unwrap());
        project.scene.entities.push(serde_json::from_value(json!({
            "name": format!("Source{i}"), "components": {"WaterSource": {"region": i + 2, "at": [i as f64 * 16.0 + 0.5, 0.5], "rate_m3s": rate}}
        })).unwrap());
    }
    project
}
fn cohort_game(project: Project, q: &ComputeQueue) -> Game {
    let host = q.clone();
    GameBuilder::new(Arc::new(project.setup(false).unwrap()))
        .system(move |sim| {
            install(sim, host.backend());
            Ok(())
        })
        .build()
        .unwrap()
}
fn component<T: bevy_ecs::component::Component>(game: &Game, id: u64) -> &T {
    let entity =
        amoris_sim::entity::entity(game.world(), amoris_sim::EntityId::new(id).unwrap()).unwrap();
    game.world().get::<T>(entity).unwrap()
}
fn resident(game: &Game, i: usize) -> &amoris_physics::water::resident::WaterResident {
    component(game, i as u64 + 2)
}
fn probe(game: &Game, regions: usize, i: usize) -> &amoris_physics::water::resident::WaterProbe {
    component(game, (regions + 2 + 2 * i) as u64)
}
fn checkpoint_keys(game: &Game) -> std::collections::BTreeSet<amoris_sim::ContentHash> {
    game.world()
        .resource::<amoris_physics::water::resident::WaterCheckpoints>()
        .fields
        .keys()
        .copied()
        .collect()
}
fn set_source(game: &mut Game, i: usize, rate: f64) {
    game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(9), game.tick().0,
        "world.edit", json!({"label":"change test source","ops":[{"set":{"entity":format!("Source{i}"),"component":"WaterSource",
            "value":{"region":i+2,"at":[i as f64*16.0+0.5,0.5],"rate_m3s":rate}}}]}))).unwrap();
}

#[test]
fn resident_cohorts_prepare_all_inputs_before_receipts_and_apply_sources_once() {
    on_game_stack(|| {
        let q = ComputeQueue::default();
        let mut game = cohort_game(cohort_project(&["gpu_resident"; 4], &[60.0; 4]), &q);
        assert!(game.poll_step().unwrap().is_pending());
        let job = take(&q);
        assert_eq!(
            job.requests
                .iter()
                .map(|r| r.region.get())
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );
        for request in &job.requests {
            let step = request.step.as_ref().unwrap();
            assert_eq!(step.edits.len(), 1);
            assert_eq!(step.edits[0].depth_m, 1.0);
            assert!(Arc::ptr_eq(
                &request.checkpoint,
                &job.requests[0].checkpoint
            ));
        }
        for _ in 0..3 {
            assert!(game.poll_step().unwrap().is_pending());
            assert!(q.take_job().unwrap().is_none());
        }
        assert!(game.snapshot().is_err());
        assert!(game.fork().is_err());
        q.complete_resident(job.id, Ok(identity_batch(&job)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        for i in 0..4 {
            assert_eq!(resident(&game, i).completed_tick, 1);
            assert_eq!(resident(&game, i).recipe.steps().len(), 1);
            assert_eq!(probe(&game, 4, i).depth, 3.0);
        }
        assert!(game.poll_step().unwrap().is_pending());
        let next = take(&q);
        assert_eq!(next.requests.len(), 4);
        q.complete_resident(next.id, Ok(identity_batch(&next)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        for i in 0..4 {
            assert_eq!(probe(&game, 4, i).depth, 4.0);
        }
    });
}

#[test]
fn a_bad_second_or_fourth_receipt_commits_no_cohort_state_or_checkpoint() {
    on_game_stack(|| {
        for (bad, kind) in [
            (1, "sample"),
            (3, "ledger"),
            (3, "overflow"),
            (3, "feedback"),
            (1, "checkpoint"),
            (3, "order"),
        ] {
            let q = ComputeQueue::default();
            let mut game = cohort_game(cohort_project(&["gpu_resident"; 4], &[60.0; 4]), &q);
            assert!(game.poll_step().unwrap().is_pending());
            let warm = take(&q);
            let mut warm_outputs = identity_batch(&warm);
            if kind == "overflow" {
                warm_outputs[bad].output.boundary.entered = f64::MAX;
            }
            q.complete_resident(warm.id, Ok(warm_outputs)).unwrap();
            assert!(game.poll_step().unwrap().is_ready());
            let saved = game.snapshot().unwrap();
            let states: Vec<_> = (0..4).map(|i| resident(&game, i).clone()).collect();
            let probes: Vec<_> = (0..4).map(|i| probe(&game, 4, i).clone()).collect();
            let feedback: Vec<_> = (0..4)
                .map(|i| {
                    component::<amoris_physics::water::feedback::WaterFeedback>(&game, i + 2)
                        .clone()
                })
                .collect();
            let keys = checkpoint_keys(&game);
            assert!(game.poll_step().unwrap().is_pending());
            let job = take(&q);
            assert_eq!(job.requests.len(), 4);
            let mut outputs = identity_batch(&job);
            // A valid first compaction must remain local if any later member fails.
            outputs[0].output = identity_checkpoint(&job.requests[0], true);
            match kind {
                "sample" => outputs[bad].output.samples[0][1] = -1.0,
                "ledger" => outputs[bad].output.boundary.entered = f64::NAN,
                "overflow" => outputs[bad].output.boundary.entered = f64::MAX,
                "feedback" => outputs[bad].output.reaction.applied[0] = 1.0,
                "checkpoint" => {
                    outputs[bad].output.checkpoint = outputs[0].output.checkpoint.clone()
                }
                "order" => outputs.swap(0, bad),
                _ => unreachable!(),
            }
            q.complete_resident(job.id, Ok(outputs)).unwrap();
            assert_eq!(
                game.poll_step().unwrap_err().code,
                "water.resident_failed",
                "{kind}"
            );
            for i in 0..4 {
                assert_eq!(resident(&game, i), &states[i], "{kind}: region {i}");
                assert_eq!(probe(&game, 4, i), &probes[i], "{kind}: probe {i}");
                assert_eq!(
                    component::<amoris_physics::water::feedback::WaterFeedback>(
                        &game,
                        i as u64 + 2
                    ),
                    &feedback[i],
                    "{kind}: feedback {i}"
                );
            }
            assert_eq!(checkpoint_keys(&game), keys, "{kind}");
            game.restore(&saved).unwrap();
            assert!(
                !q.complete_resident(job.id, Ok(identity_batch(&job)))
                    .unwrap()
            );
            assert!(game.poll_step().unwrap().is_pending());
            let fresh = take(&q);
            assert!(fresh.id > job.id);
            q.complete_resident(fresh.id, Ok(identity_batch(&fresh)))
                .unwrap();
            assert!(game.poll_step().unwrap().is_ready());
            for i in 0..4 {
                assert_eq!(probe(&game, 4, i).depth, 4.0);
            }
        }
    });
}

#[test]
fn shared_checkpoints_survive_sleep_partial_compaction_and_all_final_reference_pruning() {
    on_game_stack(|| {
        let q = ComputeQueue::default();
        let mut game = cohort_game(cohort_project(&["gpu_resident"; 2], &[0.0; 2]), &q);
        for _ in 0..60 {
            assert!(game.poll_step().unwrap().is_pending());
            let job = take(&q);
            assert_eq!(job.requests.len(), 2);
            q.complete_resident(job.id, Ok(identity_batch(&job)))
                .unwrap();
            assert!(game.poll_step().unwrap().is_ready());
        }
        let seed = resident(&game, 1).recipe.checkpoint();
        let sleeping = resident(&game, 1).clone();
        let observation = probe(&game, 2, 1).clone();
        let feedback =
            component::<amoris_physics::water::feedback::WaterFeedback>(&game, 3).clone();
        set_source(&mut game, 0, 60.0);
        assert!(game.poll_step().unwrap().is_pending());
        let job = take(&q);
        assert_eq!(
            job.requests.len(),
            1,
            "the other region is a local-ready member of this cohort"
        );
        assert_eq!(resident(&game, 1), &sleeping);
        assert_eq!(probe(&game, 2, 1), &observation);
        assert_eq!(
            component::<amoris_physics::water::feedback::WaterFeedback>(&game, 3),
            &feedback
        );
        q.complete_resident(
            job.id,
            Ok(vec![ResidentResult {
                region: job.requests[0].region,
                output: identity_checkpoint(&job.requests[0], true),
            }]),
        )
        .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        assert_eq!(resident(&game, 1).completed_tick, 61);
        assert_eq!(probe(&game, 2, 1).tick, 61);
        assert_eq!(resident(&game, 1).recipe.checkpoint(), seed);
        assert_ne!(resident(&game, 0).recipe.checkpoint(), seed);
        assert_eq!(
            checkpoint_keys(&game).len(),
            2,
            "sleeping region still owns the shared seed"
        );
        set_source(&mut game, 0, 0.0);
        set_source(&mut game, 1, 60.0);
        assert!(game.poll_step().unwrap().is_pending());
        let job = take(&q);
        assert_eq!(job.requests.len(), 2);
        let outputs = job
            .requests
            .iter()
            .map(|r| ResidentResult {
                region: r.region,
                output: identity_checkpoint(r, true),
            })
            .collect();
        q.complete_resident(job.id, Ok(outputs)).unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        assert_eq!(
            resident(&game, 0).recipe.checkpoint(),
            resident(&game, 1).recipe.checkpoint()
        );
        assert_eq!(
            checkpoint_keys(&game).len(),
            1,
            "all final refs replaced the old shared seed"
        );
        assert!(!checkpoint_keys(&game).contains(&seed));
    });
}

#[test]
fn cohort_bounds_and_mixed_solver_order_keep_later_inputs_untouched_until_their_turn() {
    on_game_stack(|| {
        let q = ComputeQueue::default();
        let solvers = [
            "gpu_resident",
            "gpu_resident",
            "cpu",
            "gpu_resident",
            "gpu",
            "gpu_resident",
        ];
        let mut game = cohort_game(cohort_project(&solvers, &[60.0; 6]), &q);
        assert!(game.poll_step().unwrap().is_pending());
        let first = take(&q);
        assert_eq!(
            first
                .requests
                .iter()
                .map(|r| r.region.get())
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            component::<amoris_physics::water::WaterRegion>(&game, 4).volume(),
            512.0
        );
        q.complete_resident(first.id, Ok(identity_batch(&first)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_pending());
        let second = take(&q);
        assert_eq!(second.requests[0].region.get(), 5);
        assert_eq!(second.requests.len(), 1);
        assert!(
            (component::<amoris_physics::water::WaterRegion>(&game, 4).volume() - 513.0).abs()
                < 1e-9
        );
        q.complete_resident(second.id, Ok(identity_batch(&second)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_pending());
        let Job::Grid(grid) = q.take_job().unwrap().unwrap() else {
            panic!("grid region must retain its ordered turn")
        };
        assert_eq!(grid.depth.iter().sum::<f64>(), 513.0);
        for _ in 0..3 {
            assert!(game.poll_step().unwrap().is_pending());
            assert!(q.take_job().unwrap().is_none());
        }
        let [nx, nz] = grid.size;
        q.complete(
            grid.id,
            Ok(amoris_runtime::water_compute::Output {
                columns: (0..nx * nz)
                    .map(|n| {
                        [
                            grid.depth[n],
                            grid.u[n % nx + (nx + 1) * (n / nx)],
                            grid.w[n],
                        ]
                    })
                    .collect(),
            }),
        )
        .unwrap();
        assert!(game.poll_step().unwrap().is_pending());
        let third = take(&q);
        assert_eq!(third.requests[0].region.get(), 7);
        q.complete_resident(third.id, Ok(identity_batch(&third)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        assert_eq!(
            component::<amoris_physics::water::WaterRegion>(&game, 6).volume(),
            513.0,
            "grid source applies once across suspension"
        );

        let q = ComputeQueue::default();
        let mut game = cohort_game(cohort_project(&["gpu_resident"; 17], &[60.0; 17]), &q);
        assert!(game.poll_step().unwrap().is_pending());
        let first = take(&q);
        assert_eq!(first.requests.len(), 16);
        let seed = first.requests[0].recipe.checkpoint();
        q.complete_resident(
            first.id,
            Ok(first
                .requests
                .iter()
                .map(|r| ResidentResult {
                    region: r.region,
                    output: identity_checkpoint(r, true),
                })
                .collect()),
        )
        .unwrap();
        assert!(game.poll_step().unwrap().is_pending());
        let last = take(&q);
        assert_eq!(last.requests.len(), 1);
        assert_eq!(last.requests[0].recipe.checkpoint(), seed);
        assert!(
            checkpoint_keys(&game).contains(&seed),
            "an untouched region still needs its seed"
        );
        q.complete_resident(
            last.id,
            Ok(vec![ResidentResult {
                region: last.requests[0].region,
                output: identity_checkpoint(&last.requests[0], true),
            }]),
        )
        .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        assert_eq!(checkpoint_keys(&game).len(), 1);
        assert!(!checkpoint_keys(&game).contains(&seed));
    });
}

#[test]
fn body_points_split_across_regions_and_feedback_waits_for_the_whole_cohort() {
    on_game_stack(|| {
        let q = ComputeQueue::default();
        let mut project = cohort_project(&["gpu_resident"; 2], &[0.0; 2]);
        project.scene.entities.push(serde_json::from_value(json!({"name":"CrossingBody","components":{
            "Transform":{"position":[16,1,0.5]},
            "Velocity":{"linear":[1,0,0],"angular":[0,0,0]},
            "RigidBody":{"kind":"Dynamic"},
            "Collider":{"shape":{"Cuboid":{"half_extents":[0.5,0.5,0.5]}},"density":350},
            "ExternalForce":{},
            "Floater":{"points":[{"at":[-0.25,0,0],"volume":0.01,"size":0.2},{"at":[0.25,0,0],"volume":0.01,"size":0.2}],"drag":1,"heave":6}
        }})).unwrap());
        let mut game = cohort_game(project, &q);
        let pose = component::<amoris_physics::Transform>(&game, 8).position;
        assert!(game.poll_step().unwrap().is_pending());
        let job = take(&q);
        assert_eq!(job.requests.len(), 2);
        for request in &job.requests {
            assert_eq!(request.coupled_from, Some(1));
            assert_eq!(
                request.points.len(),
                2,
                "one explicit probe and the body's owned point"
            );
        }
        assert_eq!(job.requests[0].points[1], [15.75, 0.5]);
        assert_eq!(job.requests[1].points[1], [0.25, 0.5]);
        assert_eq!(
            component::<amoris_physics::Transform>(&game, 8).position,
            pose
        );
        q.complete_resident(job.id, Ok(identity_batch(&job)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        assert_eq!(
            component::<amoris_physics::Floater>(&game, 8).submerged,
            1.0
        );
        for id in [2, 3] {
            let feedback = component::<amoris_physics::water::feedback::WaterFeedback>(&game, id);
            assert_eq!(feedback.pending.len(), 1);
            assert_eq!(feedback.queued_tick, 1);
            assert!(feedback.pending[0].impulse[0] > 0.0);
        }
        let pose = component::<amoris_physics::Transform>(&game, 8).position;
        assert!(game.poll_step().unwrap().is_pending());
        let job = take(&q);
        assert_eq!(job.requests.len(), 2);
        for request in &job.requests {
            assert_eq!(request.step.as_ref().unwrap().impulses.len(), 1);
        }
        for _ in 0..3 {
            assert!(game.poll_step().unwrap().is_pending());
        }
        assert_eq!(
            component::<amoris_physics::Transform>(&game, 8).position,
            pose
        );
        q.complete_resident(job.id, Ok(identity_batch(&job)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        for id in [2, 3] {
            let feedback = component::<amoris_physics::water::feedback::WaterFeedback>(&game, id);
            assert_eq!(feedback.source_tick, 1);
            assert_eq!(feedback.applied_tick, 2);
            assert_eq!(
                feedback.pending.len(),
                1,
                "new reactions are queued only after the whole batch completes"
            );
            assert_eq!(feedback.queued_tick, 2);
        }
    });
}

#[test]
fn pruning_keeps_orphan_resident_references_and_runs_with_no_water_regions() {
    on_game_stack(|| {
        let q = ComputeQueue::default();
        let mut game = cohort_game(cohort_project(&["gpu_resident"; 2], &[60.0; 2]), &q);
        assert!(game.poll_step().unwrap().is_pending());
        let job = take(&q);
        let seed = job.requests[0].recipe.checkpoint();
        q.complete_resident(job.id, Ok(identity_batch(&job)))
            .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        let orphan = resident(&game, 1).clone();
        let remove = |game: &mut Game, i: usize, components: &[&str]| {
            game.apply(&amoris_runtime::Command::new(
                amoris_link::Source::Developer(9), game.tick().0, "world.edit",
                json!({"label":"remove test water components","ops":components.iter().map(|component|
                    json!({"remove":{"entity":format!("Region{i}"),"component":component}})).collect::<Vec<_>>()})
            )).unwrap();
        };
        remove(&mut game, 1, &["WaterRegion", "WaterFeedback"]);
        assert!(game.poll_step().unwrap().is_pending());
        let job = take(&q);
        assert_eq!(job.requests.len(), 1);
        q.complete_resident(
            job.id,
            Ok(vec![ResidentResult {
                region: job.requests[0].region,
                output: identity_checkpoint(&job.requests[0], true),
            }]),
        )
        .unwrap();
        assert!(game.poll_step().unwrap().is_ready());
        let compacted = resident(&game, 0).recipe.checkpoint();
        assert_ne!(compacted, seed);
        assert_eq!(
            resident(&game, 1),
            &orphan,
            "removing WaterRegion retains its resident component"
        );
        assert_eq!(
            checkpoint_keys(&game),
            [seed, compacted].into_iter().collect()
        );
        game.snapshot().unwrap();

        remove(&mut game, 0, &["WaterRegion", "WaterFeedback"]);
        assert!(game.poll_step().unwrap().is_ready());
        assert!(q.take_job().unwrap().is_none());
        assert_eq!(
            checkpoint_keys(&game),
            [seed, compacted].into_iter().collect(),
            "empty region order must retain every live resident reference"
        );
        remove(&mut game, 1, &["WaterResident"]);
        assert!(game.poll_step().unwrap().is_ready());
        assert_eq!(checkpoint_keys(&game), [compacted].into_iter().collect());
        remove(&mut game, 0, &["WaterResident"]);
        assert!(game.poll_step().unwrap().is_ready());
        assert!(
            checkpoint_keys(&game).is_empty(),
            "the last unreferenced checkpoint must be pruned with no regions"
        );
        game.snapshot().unwrap();
    });
}
