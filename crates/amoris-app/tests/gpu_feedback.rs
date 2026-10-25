//! Pending body drag is real replayable GPU input, not water-depth injection or a wake decal.
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};
use std::sync::Arc;

fn setup(feedback: bool) -> Arc<GameSetup> {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-feedback");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    if !feedback {
        setup.scene.entities[1].components.remove("WaterFeedback");
    }
    Arc::new(setup)
}
fn read(game: &mut Game, name: &str, fields: Value) -> Value {
    game.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity":name,"fields":fields}),
    ))
    .unwrap()["fields"]
        .clone()
}
fn step(game: &mut Game, n: u32) {
    for _ in 0..n {
        let r = game.step().unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
}
fn lake(game: &mut Game) -> Value {
    read(
        game,
        "Lake",
        json!(["WaterRegion", "WaterResident", "WaterFeedback"]),
    )
}
fn pending(f: &Value, axis: usize) -> f64 {
    f["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["impulse"][axis].as_f64().unwrap())
        .sum()
}
fn budget(f: &Value) {
    for a in 0..2 {
        let emitted = f["emitted_total"][a].as_f64().unwrap();
        let accounted = f["applied_total"][a].as_f64().unwrap()
            + f["rejected_total"][a].as_f64().unwrap()
            + f["residual_total"][a].as_f64().unwrap()
            + pending(f, a);
        assert!(
            (emitted - accounted).abs() < 1e-6 + emitted.abs() * 1e-6,
            "{f}"
        );
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
fn body_drag_reaches_the_field_next_tick_and_replays_through_compaction() {
    big_stack(|| {
        let mut game = water_compute::builder(setup(true), 1).build().unwrap();
        let mut control = water_compute::builder(setup(false), 1).build().unwrap();
        step(&mut game, 1);
        step(&mut control, 1);
        let first = lake(&mut game);
        let f = &first["WaterFeedback"];
        budget(f);
        assert_eq!(f["source_tick"], 0);
        assert_eq!(f["applied_total"], json!([0.0, 0.0]));
        // Drag is implicit over each point's share of the float's mass: a little under the
        // explicit c v dt (15.1 N s here).
        assert!(pending(f, 0) > 14.3 && pending(f, 0) < 14.6, "{f}");
        assert_eq!(first["WaterResident"]["stats"]["max_speed"], 0.0);
        let mut body_momentum = 0.0;
        for n in 0..4 {
            body_momentum += 75.6
                * read(&mut game, &format!("Float{n}"), json!(["Velocity"]))["Velocity"]["linear"]
                    [0]
                .as_f64()
                .unwrap();
        }
        assert!(
            (4.0 * 75.6 * 3.0 - body_momentum - pending(f, 0)).abs() < 0.001,
            "body drag and queued water impulse differ"
        );
        step(&mut game, 1);
        step(&mut control, 1);
        let second = lake(&mut game);
        let f = &second["WaterFeedback"];
        budget(f);
        assert_eq!(f["source_tick"], 1);
        assert!(f["applied"][0].as_f64().unwrap() > 14.3);
        assert!(
            second["WaterResident"]["stats"]["max_speed"]
                .as_f64()
                .unwrap()
                > 1e-4
        );
        let still = read(&mut control, "Lake", json!(["WaterResident"]));
        assert_ne!(
            second["WaterResident"]["recipe"]["digest"], still["WaterResident"]["recipe"]["digest"],
            "the actual field must change"
        );
        step(&mut game, 58);
        let saved = game.snapshot().unwrap();
        let mut fork = game.fork().unwrap();
        for _ in 60..70 {
            step(&mut game, 1);
            step(&mut fork, 1);
            assert_eq!(game.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let expected = game.world_hash().unwrap();
        let fields = lake(&mut game);
        budget(&fields["WaterFeedback"]);
        assert!(
            (fields["WaterResident"]["stats"]["volume_cells"]
                .as_f64()
                .unwrap()
                * 0.0625
                - 2048.0)
                .abs()
                < 0.03
        );
        assert!(
            fields["WaterFeedback"]["applied_total"][0]
                .as_f64()
                .unwrap()
                > 100.0
        );
        game.restore(&saved).unwrap();
        step(&mut game, 10);
        assert_eq!(game.world_hash().unwrap(), expected);
        // The command plane cannot forge or replace a reaction receipt/packet.
        let error = game
            .apply(&Command::new(
                Source::Developer(9),
                2,
                "world.edit",
                json!({"label":"forge reaction","ops":[
                    {"set":{"entity":"Lake","component":"WaterFeedback","value":{"pending":[]}}}
                ]}),
            ))
            .unwrap_err();
        assert_eq!(error.code, "water.feedback_readonly");
    });
}
