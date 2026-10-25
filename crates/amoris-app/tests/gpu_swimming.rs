//! Actual resident water observations consumed by Walker in the same complete tick.
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};
use std::sync::Arc;

fn setup(dry: bool) -> (Arc<GameSetup>, String) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/water-floaters");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    setup
        .scene
        .entities
        .retain(|e| !e.components.contains_key("Floater"));
    let pool = setup
        .scene
        .entities
        .iter_mut()
        .find(|e| e.components.contains_key("WaterRegion"))
        .unwrap();
    let name = pool.name.clone().expect("named pool fixture");
    let region = &mut pool.components["WaterRegion"];
    region["solver"] = json!("gpu_resident");
    for tile in region["tiles"].as_array_mut().unwrap() {
        tile["depth"] = json!(vec![if dry { 0.0 } else { 2.0 }; 256]);
        tile["bed"] = json!(vec![0.0; 256]);
    }
    setup.scene.entities.push(
        serde_json::from_value(json!({
            "name":"Swimmer", "components":{
                "Transform":{"position":[4.5,1.6,4.5]}, "Walker":{}
            }
        }))
        .unwrap(),
    );
    (Arc::new(setup), name)
}
fn read(g: &mut Game, entity: &str, fields: &[&str]) -> Value {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity":entity,"fields":fields}),
    ))
    .unwrap()["fields"]
        .clone()
}
fn actor(g: &mut Game) -> Value {
    read(g, "Swimmer", &["Transform", "Walker"])
}
fn edit(g: &mut Game, component: &str, value: Value) {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.edit",
        json!({
            "label":"swimming test control", "ops":[{"set":{
                "entity":"Swimmer","component":component,"value":value
            }}]
        }),
    ))
    .unwrap();
}
fn step(g: &mut Game, count: usize) {
    for _ in 0..count {
        let r = g.step().unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
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
fn resident_pool_drives_swimming_and_current_pose_contact_without_cpu_tiles() {
    big_stack(|| {
        let (setup, pool) = setup(false);
        let mut game = water_compute::builder(setup, 1).build().unwrap();
        step(&mut game, 1);
        let first = actor(&mut game);
        assert_eq!(first["Walker"]["in_liquid"], true, "{first}");
        assert_eq!(first["Walker"]["eye_in_liquid"], true, "{first}");
        let tiles = read(&mut game, &pool, &["WaterRegion.tiles"]);
        assert_eq!(
            tiles["WaterRegion.tiles"],
            json!([]),
            "resident contact must not require old tiles"
        );
        let old_y = first["Transform"]["position"][1].as_f64().unwrap();
        edit(&mut game, "Walker", json!({"rise":true}));
        step(&mut game, 30);
        let rising = actor(&mut game);
        assert!(
            rising["Transform"]["position"][1].as_f64().unwrap() > old_y + 0.5,
            "{rising}"
        );
        assert_eq!(rising["Walker"]["eye_in_liquid"], false, "{rising}");
        // Same x/z column, a new vertical pose: no old wet-contact flags may survive.
        edit(&mut game, "Transform", json!({"position":[4.5,8.,4.5]}));
        edit(
            &mut game,
            "Walker",
            json!({"rise":false,"vertical_mps":0.0}),
        );
        step(&mut game, 1);
        let high = actor(&mut game);
        assert_eq!(high["Walker"]["in_liquid"], false, "{high}");
        assert_eq!(high["Walker"]["eye_in_liquid"], false, "{high}");
        assert!(
            high["Walker"]["vertical_mps"].as_f64().unwrap() < 0.,
            "{high}"
        );
        // A Walker outside the resident rectangle must also be legal.
        edit(&mut game, "Transform", json!({"position":[100.,8.,100.]}));
        step(&mut game, 1);
        assert_eq!(actor(&mut game)["Walker"]["in_liquid"], false);
    });
}

#[test]
fn dry_resident_contact_and_swimmer_fork_restore_never_consume_derived_old_samples() {
    big_stack(|| {
        let (dry, _) = setup(true);
        let mut dry = water_compute::builder(dry, 1).build().unwrap();
        step(&mut dry, 1);
        let d = actor(&mut dry);
        assert_eq!(d["Walker"]["in_liquid"], false, "{d}");
        assert_eq!(d["Walker"]["eye_in_liquid"], false, "{d}");
        let (wet, _) = setup(false);
        let mut game = water_compute::builder(wet, 1).build().unwrap();
        step(&mut game, 3);
        edit(&mut game, "Walker", json!({"forward":0.3,"rise":true}));
        let checkpoint = game.snapshot().unwrap();
        let mut fork = game.fork().unwrap();
        for _ in 0..12 {
            step(&mut game, 1);
            step(&mut fork, 1);
            assert_eq!(game.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let expected = game.world_hash().unwrap();
        game.restore(&checkpoint).unwrap();
        step(&mut game, 12);
        assert_eq!(game.world_hash().unwrap(), expected);
    });
}

#[test]
fn actual_dam_current_pushes_the_swimmer_on_cpu_full_gpu_and_resident_gpu() {
    big_stack(|| {
        let mut positions = vec![];
        for mode in ["cpu", "gpu", "gpu_resident"] {
            let (source, _) = setup(false);
            let mut source = (*source).clone();
            let pool = source
                .scene
                .entities
                .iter_mut()
                .find(|e| e.components.contains_key("WaterRegion"))
                .unwrap();
            let region = &mut pool.components["WaterRegion"];
            region["solver"] = json!(mode);
            for tile in region["tiles"].as_array_mut().unwrap() {
                let depth = if tile["key"][0] == 0 { 2.0 } else { 0.0 };
                tile["depth"] = json!(vec![depth; 256]);
            }
            let swimmer = source
                .scene
                .entities
                .iter_mut()
                .find(|e| e.name.as_deref() == Some("Swimmer"))
                .unwrap();
            swimmer.components["Transform"]["position"] = json!([15.0, 1.62, 8.5]);
            if mode == "gpu_resident" {
                source.scene.entities.push(
                    serde_json::from_value(json!({"name":"CurrentProbe","components":{
                        "WaterProbe":{"region":2,"at":[15.5,8.5]}
                    }}))
                    .unwrap(),
                );
            }
            let mut game = water_compute::builder(Arc::new(source), 1).build().unwrap();
            if mode == "gpu_resident" {
                for _ in 0..60 {
                    let before = actor(&mut game);
                    let p = before["Transform"]["position"].as_array().unwrap();
                    let old_x = p[0].as_f64().unwrap();
                    let old_y = p[1].as_f64().unwrap();
                    let old_z = p[2].as_f64().unwrap();
                    game.apply(&Command::new(Source::Developer(9),1,"world.edit",json!({"label":"observe current actor column","ops":[{"set":{
                        "entity":"CurrentProbe","component":"WaterProbe","value":{"region":2,"at":[old_x.floor()+0.5,old_z.floor()+0.5]}
                    }}]}))).unwrap();
                    step(&mut game, 1);
                    let observation = read(&mut game, "CurrentProbe", &["WaterProbe"]);
                    let q = &observation["WaterProbe"];
                    assert_eq!(q["ready"], true);
                    assert_eq!(q["tick"], game.tick().0);
                    let u = q["velocity"][0].as_f64().unwrap();
                    let v = q["velocity"][1].as_f64().unwrap();
                    let scale = 2.0 / (u * u + v * v).sqrt().max(2.0);
                    let wet = q["depth"].as_f64().unwrap() > 0.0
                        && q["surface"].as_f64().unwrap() > old_y - 1.62 + 0.05;
                    let after = actor(&mut game);
                    assert_eq!(after["Walker"]["in_liquid"], wet);
                    let expected = if wet { u * scale / 60.0 } else { 0.0 };
                    let actual = after["Transform"]["position"][0].as_f64().unwrap() - old_x;
                    assert!(
                        (actual - expected).abs() < 1e-10,
                        "Walker did not consume its current authoritative column: {actual} versus {expected}, {q}"
                    );
                }
            } else {
                step(&mut game, 60);
            }
            let current = actor(&mut game);
            let x = current["Transform"]["position"][0].as_f64().unwrap();
            assert!(
                x > 15.1 && x <= 17.01,
                "{mode}: current did not drive the actor within its 2m/s cap: {current}"
            );
            positions.push(x);
        }
        // CPU and full-readback GPU use the classic scheme. Resident uses bounded MacCormack
        // plus linear depth transport, so its trajectory is checked against its own current
        // field above, rather than a different numerical scheme's final trajectory.
        assert!(
            (positions[0] - positions[1]).abs() < 0.0001,
            "classic CPU/GPU current differs: {positions:?}"
        );
    });
}

#[test]
fn pool_minigame_completes_and_retries_through_the_real_player_action_catalog() {
    big_stack(|| {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-pool");
        let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
        let mut game = water_compute::builder(setup, 1).build().unwrap();
        let mut sequence = 0;
        let act = |game: &mut Game, sequence: &mut u64, actions: Value| {
            *sequence += 1;
            game.apply(&Command::new(
                Source::Player(0),
                *sequence,
                "player.act",
                json!({"actions":actions}),
            ))
            .unwrap();
        };
        step(&mut game, 3);
        act(
            &mut game,
            &mut sequence,
            json!([{"do":"set","controls":{"forward":1.0,"yaw":0.0,"rise":false}}]),
        );
        let mut phase = 0;
        let mut visited = std::collections::BTreeSet::new();
        let trace_path = std::env::var_os("AMORIS_SWIM_TRACE");
        let mut trajectory = Vec::new();
        for _ in 0..1800 {
            step(&mut game, 1);
            let state = read(&mut game, "Swimmer", &["PoolRun", "Transform", "Walker"]);
            if trace_path.is_some() {
                let mut row = state.clone();
                row["world_hash"] = json!(game.world_hash().unwrap());
                trajectory.push(row);
            }
            let stage = state["PoolRun"]["stage"].as_u64().unwrap();
            visited.insert(stage);
            let z = state["Transform"]["position"][2].as_f64().unwrap();
            if phase == 0 && z <= 8.1 {
                act(
                    &mut game,
                    &mut sequence,
                    json!([{"do":"set","controls":{"forward":0.0}}]),
                );
                phase = 1;
            }
            if stage == 2 && phase < 2 {
                act(
                    &mut game,
                    &mut sequence,
                    json!([{"do":"set","controls":{"forward":0.0,"rise":true}}]),
                );
                phase = 2;
            }
            if stage == 3 && phase < 3 {
                act(
                    &mut game,
                    &mut sequence,
                    json!([{"do":"set","controls":{"forward":-1.0,"rise":true}}]),
                );
                phase = 3;
            }
            if stage == 4 {
                act(
                    &mut game,
                    &mut sequence,
                    json!([{"do":"set","controls":{"forward":0.0,"rise":false}}]),
                );
                break;
            }
        }
        let done = read(&mut game, "Swimmer", &["PoolRun", "Transform", "Walker"]);
        assert_eq!(
            done["PoolRun"]["stage"], 4,
            "real input route did not complete: {done}"
        );
        assert!(visited.is_superset(&std::collections::BTreeSet::from([1, 2, 3, 4])));
        assert!(done["PoolRun"]["underwater_s"].as_f64().unwrap() > 0.0);
        let best = done["PoolRun"]["best_s"].clone();
        let elapsed = done["PoolRun"]["elapsed_s"].clone();
        step(&mut game, 10);
        assert_eq!(
            read(&mut game, "Swimmer", &["PoolRun"])["PoolRun"]["elapsed_s"],
            elapsed
        );
        act(
            &mut game,
            &mut sequence,
            json!([{"do":"pulse","control":"secondary"}]),
        );
        step(&mut game, 1);
        let retry = read(&mut game, "Swimmer", &["PoolRun"]);
        assert_eq!(retry["PoolRun"]["stage"], 0);
        assert_eq!(retry["PoolRun"]["attempt"], 2);
        assert_eq!(retry["PoolRun"]["best_s"], best);
        assert_eq!(
            read(&mut game, "PoolWater", &["WaterRegion.tiles"])["WaterRegion.tiles"],
            json!([])
        );
        if let Some(path) = trace_path {
            std::fs::write(path, serde_json::to_vec(&trajectory).unwrap()).unwrap();
        }
    });
}
