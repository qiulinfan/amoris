//! Real GPU field samples driving Rapier force/torque, not a marker following a height query.
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};
use std::sync::Arc;

fn setup(mode: &str, flat: bool) -> Arc<GameSetup> {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-floaters");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    let region = &mut setup.scene.entities[1].components["WaterRegion"];
    region["solver"] = json!(mode);
    if flat {
        for tile in region["tiles"].as_array_mut().unwrap() {
            tile["depth"] = json!(vec![2.0; 256]);
            tile["bed"] = json!(vec![0.0; 256]);
        }
    }
    Arc::new(setup)
}
fn read(game: &mut Game, name: &str) -> Value {
    game.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity":name,"fields":["Transform","Velocity","Floater"]}),
    ))
    .unwrap()["fields"]
        .clone()
}
fn step(game: &mut Game, n: u32) {
    for _ in 0..n {
        let report = game.step().unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
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
fn current_field_floats_four_crates_on_all_three_backends_and_matches_equilibrium() {
    big_stack(|| {
        let mut reference = None;
        for mode in ["cpu", "gpu", "gpu_resident"] {
            let mut game = water_compute::builder(setup(mode, true), 1)
                .build()
                .unwrap();
            step(&mut game, 360);
            let mut result = Vec::new();
            for n in 0..4 {
                let row = read(&mut game, &format!("Float{n}"));
                let y = row["Transform"]["position"][1].as_f64().unwrap();
                let wet = row["Floater"]["submerged"].as_f64().unwrap();
                assert!((wet - 0.35).abs() < 0.01, "{mode}: {row}");
                assert!((y - 2.09).abs() < 0.02, "{mode}: {row}");
                result.push(row);
            }
            if let Some(expected) = &reference {
                assert_eq!(
                    &result, expected,
                    "constant field must give identical body forces on {mode}"
                );
            } else {
                reference = Some(result);
            }
        }
    });
}
#[test]
fn dam_floaters_follow_current_and_restore_without_old_pose_samples() {
    big_stack(|| {
        let mut game = water_compute::builder(setup("gpu_resident", false), 1)
            .build()
            .unwrap();
        step(&mut game, 70);
        let snapshot = game.snapshot().unwrap();
        let mut fork = game.fork().unwrap();
        for _ in 0..10 {
            step(&mut game, 1);
            step(&mut fork, 1);
            assert_eq!(game.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let expected = game.world_hash().unwrap();
        game.restore(&snapshot).unwrap();
        step(&mut game, 10);
        assert_eq!(game.world_hash().unwrap(), expected);
        step(&mut game, 40);
        // The wet-side rarefaction needs time to reach the far crate. Use the near-dam crate
        // to check transport rather than expecting an instantaneous current across the pool.
        let row = read(&mut game, "Float3");
        let far = read(&mut game, "Float0");
        assert!(
            far["Transform"]["position"][0].as_f64().unwrap() < 5.6,
            "the far crate should not acquire an instantaneous pool-wide current"
        );
        assert!(
            row["Transform"]["position"][0].as_f64().unwrap() > 11.7,
            "current must carry the body: {row}"
        );
        assert!(row["Floater"]["submerged"].as_f64().unwrap() > 0.1, "{row}");
        // Teleport to a point far above the same water. No result from the old pose may lift it.
        game.apply(&Command::new(Source::Developer(9),1,"world.edit",json!({"label":"move floater","ops":[
            {"set":{"entity":"Float0","component":"Transform","value":{"position":[24.5,10.0,8.5]}}},
            {"set":{"entity":"Float0","component":"Velocity","value":{"linear":[0,0,0],"angular":[0,0,0]}}}
        ]}))).unwrap();
        step(&mut game, 1);
        let moved = read(&mut game, "Float0");
        assert_eq!(moved["Floater"]["submerged"], 0.0);
        assert!(
            moved["Velocity"]["linear"][1].as_f64().unwrap() < -0.15,
            "{moved}"
        );
    });
}
