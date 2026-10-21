//! The actual game tick on GPU versus CPU, including persistence and explicit host errors.
#[path = "../src/water_compute.rs"]
mod water_compute;

use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};
use std::sync::Arc;

fn setup(gpu: bool) -> Arc<GameSetup> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-dam");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    setup.scene.entities[1].components["WaterRegion"]["solver"] =
        json!(if gpu { "gpu" } else { "cpu" });
    Arc::new(setup)
}

fn region(game: &mut Game) -> Value {
    game.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity": "Lake", "fields": ["WaterRegion"]}),
    ))
    .unwrap()["fields"]["WaterRegion"]
        .clone()
}
fn depths(game: &mut Game) -> Vec<f64> {
    region(game)["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|tile| {
            tile["depth"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d.as_f64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}
fn big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn resident_game_persists_bounded_state_and_samples_across_forks_and_restores() {
    big_stack(|| {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-resident");
        let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
        let mut game = water_compute::builder(setup, 1).build().unwrap();
        let feed = amoris_assets::Feed::new();
        let mailbox = feed.subscribe();
        let mut extractor = amoris_runtime::Extractor::default();
        let mut merged = amoris_assets::frame::RenderFrame::default();
        let read = |game: &mut Game, entity, fields| {
            game.apply(&Command::new(
                Source::Developer(9),
                1,
                "world.get",
                json!({"entity": entity, "fields": fields}),
            ))
            .unwrap()["fields"]
                .clone()
        };
        for tick in 1..=70 {
            game.step().unwrap();
            let newcomer = (tick == 27).then(|| feed.subscribe());
            let state = read(&mut game, "Lake", json!(["WaterRegion", "WaterResident"]));
            game.present(&mut extractor, &feed);
            let frame = mailbox.take();
            if let Some(newcomer) = newcomer {
                let full = newcomer.take();
                assert!(full.reset);
                assert_eq!(full.resident_water[0].recipe.start, 0);
                assert!(full.resident_water[0].checkpoint.is_some());
                assert_eq!(
                    serde_json::to_value(full.resident_water[0].recipe.apply(None).unwrap())
                        .unwrap(),
                    state["WaterResident"]["recipe"]
                );
                newcomer.close();
            }
            assert!(frame.water.is_empty());
            assert_eq!(frame.resident_water.len(), 1);
            let visual = &frame.resident_water[0];
            assert!(
                visual.recipe.steps.len() <= 1,
                "steady publication sends only its new step"
            );
            assert_eq!(visual.checkpoint.is_some(), tick == 1 || tick % 64 == 0);
            if tick != 1 && tick % 64 != 0 {
                assert!(frame.encode().len() < 6000);
            }
            merged.merge(frame).unwrap();
            assert_eq!(
                serde_json::to_value(merged.resident_water[0].recipe.apply(None).unwrap()).unwrap(),
                state["WaterResident"]["recipe"]
            );
            assert!(merged.resident_water[0].checkpoint.is_some());
            assert_eq!(
                amoris_assets::frame::RenderFrame::decode(&merged.encode()).unwrap(),
                merged
            );
            assert_eq!(state["WaterResident"]["completed_tick"], tick);
            assert_eq!(
                state["WaterResident"]["recipe"]["steps"]
                    .as_array()
                    .unwrap()
                    .len(),
                tick as usize % 64
            );
            assert!(state["WaterRegion"]["tiles"].as_array().unwrap().is_empty());
            assert!(
                (state["WaterResident"]["stats"]["volume_cells"]
                    .as_f64()
                    .unwrap()
                    - 1024.0)
                    .abs()
                    < 0.02
            );
            let probe = read(&mut game, "Front", json!(["WaterProbe"]));
            assert_eq!(probe["WaterProbe"]["tick"], tick);
            assert_eq!(probe["WaterProbe"]["ready"], true);
            assert_eq!(
                probe["WaterProbe"]["state_digest"],
                state["WaterResident"]["recipe"]["digest"]
            );
        }
        let snapshot = game.snapshot().unwrap();
        let mut fork = game.fork().unwrap();
        for _ in 0..10 {
            game.step().unwrap();
            fork.step().unwrap();
            assert_eq!(game.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let expected = game.world_hash().unwrap();
        game.restore(&snapshot).unwrap();
        for _ in 0..10 {
            game.step().unwrap();
        }
        assert_eq!(game.world_hash().unwrap(), expected);
        let probe = read(&mut game, "Front", json!(["WaterProbe"]));
        assert!(probe["WaterProbe"]["depth"].as_f64().unwrap() > 0.05);
    });
}

#[test]
fn gpu_game_matches_cpu_within_tolerance_and_continues_forks_and_restores() {
    big_stack(|| {
        for substeps in [1, 2, 3] {
            let configured = |gpu| {
                let mut data = (*setup(gpu)).clone();
                data.scene.entities[1].components["WaterRegion"]["substeps"] = json!(substeps);
                Arc::new(data)
            };
            let mut cpu = water_compute::builder(configured(false), 1)
                .build()
                .unwrap();
            let mut gpu = water_compute::builder(configured(true), 1).build().unwrap();
            for _ in 0..60 {
                cpu.step().unwrap();
                gpu.step().unwrap();
            }
            let a = depths(&mut cpu);
            let b = depths(&mut gpu);
            let max = a
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f64::max);
            let mass = b.iter().sum::<f64>();
            eprintln!("GPU game: max depth error {max:.8} m; water {mass:.8} m3");
            assert!(max < 2e-4, "CPU/GPU depth error {max}");
            assert!((mass - 1024.0).abs() < 0.02);
            assert_eq!(region(&mut gpu)["solver"], "gpu");
            let snap = gpu.snapshot().unwrap();
            let mut fork = gpu.fork().unwrap();
            for _ in 0..10 {
                gpu.step().unwrap();
                fork.step().unwrap();
                assert_eq!(gpu.world_hash().unwrap(), fork.world_hash().unwrap());
            }
            let expected = gpu.world_hash().unwrap();
            gpu.restore(&snap).unwrap();
            for _ in 0..10 {
                gpu.step().unwrap();
            }
            assert_eq!(gpu.world_hash().unwrap(), expected);
            assert_eq!(region(&mut cpu)["solver"], "cpu");
        }
    });
}

#[test]
fn gpu_region_without_a_service_fails_and_poisons_the_tick() {
    big_stack(|| {
        let mut game = Game::new(setup(true), 1).unwrap();
        assert_eq!(game.step().unwrap_err().code, "water.compute_missing");
        assert_eq!(game.step().unwrap_err().code, "sim.world_poisoned");
    });
}

#[test]
fn gpu_advection_in_both_axes_matches_cpu_over_beds_far_from_the_origin() {
    big_stack(|| {
        let make = |gpu: bool| {
            let mut setup = (*setup(gpu)).clone();
            let tiles: Vec<_> = (0..2)
                .flat_map(|z| {
                    (0..2).map(move |x| {
                        let mut bed = Vec::new();
                        let mut depth = Vec::new();
                        let mut u = Vec::new();
                        let mut w = Vec::new();
                        for n in 0..256 {
                            let i = x * 16 + n % 16;
                            let k = z * 16 + n / 16;
                            let h = f64::from((i / 4 + k / 4) % 3) * 0.125;
                            bed.push(1_000_000.0 + h);
                            depth.push(2.0 - h);
                            u.push(if i == 0 {
                                0.0
                            } else {
                                (f64::from(k % 8) - 3.5) * 0.25
                            });
                            w.push(if k == 0 {
                                0.0
                            } else {
                                (f64::from(i % 8) - 3.5) * 0.1
                            });
                        }
                        json!({"key": [x,z], "bed": bed, "depth": depth, "u": u, "w": w})
                    })
                })
                .collect();
            setup.scene.entities[1].components["WaterRegion"]["tiles"] = json!(tiles);
            water_compute::builder(Arc::new(setup), 1).build().unwrap()
        };
        let mut cpu = make(false);
        let mut gpu = make(true);
        for _ in 0..40 {
            cpu.step().unwrap();
            gpu.step().unwrap();
        }
        let a = region(&mut cpu);
        let b = region(&mut gpu);
        for (a, b) in a["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .zip(b["tiles"].as_array().unwrap())
        {
            assert_eq!(
                a["bed"], b["bed"],
                "the GPU never rounds the persisted beds"
            );
            for field in ["depth", "u", "w"] {
                let error = a[field]
                    .as_array()
                    .unwrap()
                    .iter()
                    .zip(b[field].as_array().unwrap())
                    .map(|(a, b)| (a.as_f64().unwrap() - b.as_f64().unwrap()).abs())
                    .fold(0.0, f64::max);
                assert!(error < 2e-4, "{field}: max CPU/GPU error {error}");
            }
        }
    });
}

#[test]
fn scripts_observe_completed_gpu_results_and_invalidate_staged_probe_requests() {
    big_stack(|| {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-resident");
        let mut project = Project::load(&root).unwrap();
        project.source.files.insert("scripts/main.ts".into(),br#"
import {component,field,game,system} from "amoris";
export const Readings=component("Readings",{version:1,doc:"SDK probe checks.",fields:{
 tick:field.tick(0,"Tick checked."),ready:field.bool(false,"Observation available."),
 at:field.vec2({x:0,y:0},"Point observed."),depth:field.f64(0,"Observed depth.")
}});
export const check=system({name:"check",phase:"update",doc:"Check immutable observations and pending request edits.",run(ctx){
 const p=ctx.single("Readings"); const o=ctx.water.observe(p);
 const expected=ctx.tick!==1 && ctx.tick!==5 && ctx.tick!==8;
 if(!!o!==expected)throw new Error("availability at "+ctx.tick);
 if(o && (o.tick!==ctx.tick-1 || o.ageTicks!==1 || !Object.isFrozen(o)||!Object.isFrozen(o.at)||!Object.isFrozen(o.velocity)||o.stateDigest.length!==64))
   throw new Error("observation is stale or mutable");
 ctx.world.set(p,"Readings",{tick:ctx.tick,ready:!!o,at:{x:o?.at[0]??0,y:o?.at[1]??0},depth:o?.depth??0});
 if(ctx.world.has(p,"WaterProbe")){
   let refused=false;try{ctx.world.set(p,"WaterProbe",{depth:999});}catch(e){refused=true;}
   if(!refused)throw new Error("engine output was writable");
 }
 if(ctx.tick===2){const q=ctx.query({with:["WaterProbe"],fields:["WaterProbe.at"]});q.cols.WaterProbe.at.x[0]=7.5;
   if(ctx.water.observe(p)!==undefined)throw new Error("column request reused old data");}
 if(ctx.tick===3){ctx.world.set(p,"WaterProbe",{at:{x:8.5,y:3.5}});
   if(ctx.water.observe(p)!==undefined)throw new Error("overlay request reused old data");}
 if(ctx.tick===4){ctx.world.set(p,"WaterProbe",{region:null});if(ctx.water.observe(p)!==undefined)throw new Error("disabled result");}
 if(ctx.tick===5){ctx.world.set(p,"WaterProbe",{region:2,at:{x:9.5,y:4.5}});if(ctx.water.observe(p)!==undefined)throw new Error("request ran synchronously");}
 if(ctx.tick===6){ctx.world.set(p,"WaterProbe",{at:{x:9.5,y:4.5}});if(!ctx.water.observe(p))throw new Error("same request invalidated");}
 if(ctx.tick===7){ctx.world.remove(p,"WaterProbe");if(ctx.water.observe(p)!==undefined)throw new Error("removed result");}
 if(ctx.tick===8){ctx.world.insert(p,"WaterProbe",{region:2,at:{x:10.5,y:5.5}});if(ctx.water.observe(p)!==undefined)throw new Error("new request ready");}
}});
export default game({components:[Readings],systems:[check]});
"#.to_vec());
        project.scene.entities[2]
            .components
            .insert("Readings".into(), json!({}));
        let mut game = water_compute::builder(Arc::new(project.setup(false).unwrap()), 1)
            .build()
            .unwrap();
        for tick in 1..=9 {
            let result = game.step().unwrap();
            assert!(result.errors.is_empty(), "{:#?}", result.errors);
            let data = game
                .apply(&Command::new(
                    Source::Developer(9),
                    1,
                    "world.get",
                    json!({"entity":"Front","fields":["Readings"]}),
                ))
                .unwrap()["fields"]["Readings"]
                .clone();
            assert_eq!(data["tick"], tick);
            assert_eq!(data["ready"], tick != 1 && tick != 5 && tick != 8);
            if tick == 3 {
                assert_eq!(data["at"], json!({"x":7.5,"y":16.5}));
                assert!(data["depth"].as_f64().unwrap() > 1.0);
            }
            if tick == 9 {
                assert_eq!(data["at"], json!({"x":10.5,"y":5.5}));
            }
        }
        let saved = game.snapshot().unwrap();
        let mut fork = game.fork().unwrap();
        for _ in 0..4 {
            game.step().unwrap();
            fork.step().unwrap();
            assert_eq!(game.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let expected = game.world_hash().unwrap();
        game.restore(&saved).unwrap();
        for _ in 0..4 {
            game.step().unwrap();
        }
        assert_eq!(expected, game.world_hash().unwrap());
    });
}
