//! Real host, world ledger, control wakeups, periodic exact state and restored GPU continuation.
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
fn read(g: &mut Game) -> Value {
    g.apply(&Command::new(
        Source::Developer(0),
        1,
        "world.get",
        json!({"entity":"River","fields":["WaterResident","WaterRegion"]}),
    ))
    .unwrap()["fields"]
        .clone()
}
fn steps(g: &mut Game, n: u32) {
    for _ in 0..n {
        let r = g.step().unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
}
fn edit(g: &mut Game, edges: Value) {
    g.apply(&Command::new(Source::Developer(0),2,"world.edit",json!({"label":"boundary controls","ops":[{"set":{"entity":"River","component":"WaterRegion","value":{"edges":edges}}}]}))).unwrap();
}
fn setup() -> amoris_runtime::GameSetup {
    Project::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/scenes/water-river"),
    )
    .unwrap()
    .setup(false)
    .unwrap()
}
#[test]
fn live_river_accounts_flux_survives_compaction_forks_and_closes_without_stale_receipts() {
    run(|| {
        let mut g = water_compute::builder(Arc::new(setup()), 1)
            .build()
            .unwrap();
        let feed = amoris_assets::Feed::new();
        let mailbox = feed.subscribe();
        let mut extractor = amoris_runtime::present::Extractor::default();
        for _ in 0..128 {
            steps(&mut g, 1);
            g.present(&mut extractor, &feed);
            let f = mailbox.take();
            if g.tick().0 == 128 {
                let cp = f.resident_water[0]
                    .checkpoint
                    .as_ref()
                    .expect("exact periodic river checkpoint");
                let actual: f64 = cp
                    .columns
                    .iter()
                    .map(|c| f64::from(f32::from_bits(c[1])) * 0.0625)
                    .sum();
                let state = read(&mut g);
                let b = &state["WaterResident"]["boundary"];
                let expected = b["initial_volume"].as_f64().unwrap()
                    + b["entered_total"].as_f64().unwrap()
                    - b["left_total"].as_f64().unwrap();
                assert!(
                    (actual - expected).abs() < 0.002,
                    "independent checkpoint volume {actual} vs boundary flow {expected}: {b}"
                );
            }
        }
        let state = read(&mut g);
        let b = &state["WaterResident"]["boundary"];
        assert!(
            (b["entered_total"].as_f64().unwrap() - 128.0 / 60.0 * 8.0).abs() < 1e-4,
            "{b}"
        );
        assert!(b["left_total"].as_f64().unwrap() > 10.0, "{b}");
        assert!(state["WaterRegion"]["tiles"].as_array().unwrap().is_empty());
        let saved = g.snapshot().unwrap();
        let mut fork = g.fork().unwrap();
        for _ in 0..12 {
            steps(&mut g, 1);
            steps(&mut fork, 1);
            assert_eq!(g.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let expected = g.world_hash().unwrap();
        g.restore(&saved).unwrap();
        steps(&mut g, 12);
        assert_eq!(g.world_hash().unwrap(), expected);
        edit(&mut g, json!(["Wall", "Wall", "Wall", "Wall"]));
        steps(&mut g, 1);
        let b = read(&mut g)["WaterResident"]["boundary"].clone();
        assert_eq!(b["entered"], 0.0);
        assert_eq!(b["left"], 0.0);
    });
}
#[test]
fn edits_wake_a_sleeping_field_and_an_open_steady_region_keeps_executing() {
    run(|| {
        let mut s = setup();
        s.scene.entities.truncate(2);
        s.scene.entities[0].components["Voxels"]["chunks"] = json!([]);
        let r = &mut s.scene.entities[1].components["WaterRegion"];
        r["edges"] = json!(["Wall", "Wall", "Wall", "Wall"]);
        for t in r["tiles"].as_array_mut().unwrap() {
            t["depth"] = json!(vec![0.0; 256]);
            t["u"] = json!(vec![0.0; 256]);
            t["w"] = json!(vec![0.0; 256]);
        }
        let mut g = water_compute::builder(Arc::new(s), 1).build().unwrap();
        steps(&mut g, 65);
        assert_eq!(read(&mut g)["WaterResident"]["still"], 60);
        edit(
            &mut g,
            json!([{"Inflow":{"m3s_per_m":0.0}},"Wall","Wall","Wall"]),
        );
        steps(&mut g, 80);
        let state = read(&mut g);
        assert_eq!(state["WaterResident"]["still"], 60);
        assert_eq!(
            state["WaterResident"]["recipe"]["steps"]
                .as_array()
                .unwrap()
                .len(),
            12,
            "open zero-field execution must continue through compaction"
        );
        edit(
            &mut g,
            json!([{"Inflow":{"m3s_per_m":0.5}},"Wall","Wall","Wall"]),
        );
        steps(&mut g, 1);
        assert!(
            read(&mut g)["WaterResident"]["boundary"]["entered"]
                .as_f64()
                .unwrap()
                > 0.0
        );
    });
}

#[test]
fn excessive_inlet_forcing_fails_instead_of_silently_clipping_discharge() {
    run(|| {
        let mut g = water_compute::builder(Arc::new(setup()), 1)
            .build()
            .unwrap();
        steps(&mut g, 1);
        let saved = g.snapshot().unwrap();
        edit(
            &mut g,
            json!([{"Inflow":{"m3s_per_m":1e6}},"Outflow","Wall","Wall"]),
        );
        let error = g.step().unwrap_err();
        assert!(error.message.contains("CFL"), "{error:?}");
        assert_eq!(g.step().unwrap_err().code, "sim.world_poisoned");
        g.restore(&saved).unwrap();
        steps(&mut g, 1);
        assert!(
            read(&mut g)["WaterResident"]["boundary"]["entered"]
                .as_f64()
                .unwrap()
                < 1.0
        );
    });
}
