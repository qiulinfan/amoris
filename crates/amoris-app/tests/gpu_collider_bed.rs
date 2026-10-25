#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::{Command, Game, Project};
use serde_json::{Value, json};
use std::sync::Arc;
fn run(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}
fn game() -> Game {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/water-slope");
    water_compute::builder(
        Arc::new(Project::load(&root).unwrap().setup(false).unwrap()),
        1,
    )
    .build()
    .unwrap()
}
fn steps(g: &mut Game, n: u32) {
    for _ in 0..n {
        let r = g.step().unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
}
fn read(g: &mut Game, name: &str, fields: Value) -> Value {
    g.apply(&Command::new(
        Source::Developer(0),
        1,
        "world.get",
        json!({"entity":name,"fields":fields}),
    ))
    .unwrap()["fields"]
        .clone()
}
#[test]
fn static_collider_beds_survive_resident_checkpoints_forks_and_restore() {
    run(|| {
        let mut g = game();
        steps(&mut g, 128);
        let state = read(
            &mut g,
            "Shore",
            json!(["WaterColliderBed", "WaterResident", "WaterRegion"]),
        );
        assert_eq!(
            state["WaterColliderBed"]["source"]
                .as_array()
                .unwrap()
                .len(),
            32
        );
        assert!(state["WaterRegion"]["tiles"].as_array().unwrap().is_empty());
        let snap = g.snapshot().unwrap();
        let mut fork = g.fork().unwrap();
        for _ in 0..16 {
            steps(&mut g, 1);
            steps(&mut fork, 1);
            assert_eq!(g.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let target = g.world_hash().unwrap();
        g.restore(&snap).unwrap();
        steps(&mut g, 16);
        assert_eq!(g.world_hash().unwrap(), target);
    });
}
#[test]
fn resident_floor_edits_refuse_stale_beds_but_friction_and_unrelated_geometry_do_not() {
    run(|| {
        let mut g = game();
        steps(&mut g, 1);
        let saved = g.snapshot().unwrap();
        g.apply(&Command::new(Source::Developer(0),2,"world.edit",json!({"label":"terrain friction","ops":[{"set":{"entity":"Terrain","component":"Collider","value":{"friction":0.8}}}]}))).unwrap();
        steps(&mut g, 1);
        g.apply(&Command::new(Source::Developer(0),4,"world.edit",json!({"label":"outside wall","ops":[{"set":{"entity":"FlumeNorth","component":"Transform","value":{"position":[16,0.35,8.5]}}}]}))).unwrap();
        steps(&mut g, 1);
        let position = read(&mut g, "Terrain", json!(["Transform"]))["Transform"]["position"]
            .as_array()
            .unwrap()
            .clone();
        let p = [
            position[0].as_f64().unwrap(),
            position[1].as_f64().unwrap() + 0.1,
            position[2].as_f64().unwrap(),
        ];
        g.apply(&Command::new(Source::Developer(0),3,"world.edit",json!({"label":"raise floor","ops":[{"set":{"entity":"Terrain","component":"Transform","value":{"position":p}}}]}))).unwrap();
        let error = g.step().unwrap_err();
        assert!(
            error.message.contains("collider-bed inputs changed"),
            "{error:?}"
        );
        assert_eq!(g.step().unwrap_err().code, "sim.world_poisoned");
        g.restore(&saved).unwrap();
        steps(&mut g, 1);
    });
}

#[test]
fn a_snapshot_between_geometry_edit_and_water_keeps_the_stale_bed_identity() {
    run(|| {
        let mut g = game();
        steps(&mut g, 1);
        let mut p = read(&mut g, "Terrain", json!(["Transform"]))["Transform"]["position"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect::<Vec<_>>();
        p[1] += 0.1;
        g.apply(&Command::new(Source::Developer(0),2,"world.edit",json!({"label":"edit before saved boundary","ops":[{"set":{"entity":"Terrain","component":"Transform","value":{"position":p}}}]}))).unwrap();
        // Disposable change ticks/cache are rebuilt, but the previously sampled identity is not.
        let saved = g.snapshot().unwrap();
        let mut fork = g.fork().unwrap();
        assert!(
            fork.step()
                .unwrap_err()
                .message
                .contains("collider-bed inputs changed")
        );
        g.restore(&saved).unwrap();
        assert!(
            g.step()
                .unwrap_err()
                .message
                .contains("collider-bed inputs changed")
        );
    });
}
