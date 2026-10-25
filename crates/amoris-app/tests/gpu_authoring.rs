//! Water declared by its features (water.md 65): the valley sample's terrain, lake, river and falls
//! come from a few components, and once it runs the river crosses from the lake's region into the
//! falls', while the walker stands on the terrain's ground.
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

fn get(g: &mut Game, entity: &str, field: &str) -> Value {
    g.apply(&Command::new(
        Source::Developer(0),
        1,
        "world.get",
        json!({"entity": entity, "fields": [field]}),
    ))
    .unwrap()["fields"][field]
        .clone()
}

#[test]
fn the_valley_runs_its_river_across_its_seam_and_stands_its_walker_on_the_ground() {
    run(|| {
        let setup = Project::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-valley"),
        )
        .unwrap()
        .setup(false)
        .unwrap();
        let mut g = water_compute::builder(Arc::new(setup), 1).build().unwrap();
        let mut heights = Vec::new();
        for tick in 1..=1200 {
            let r = g.step().unwrap();
            assert!(r.errors.is_empty(), "{:?}", r.errors);
            if tick % 120 == 0 {
                heights.push(
                    get(&mut g, "Walker", "Transform")["position"][1]
                        .as_f64()
                        .unwrap(),
                );
            }
        }
        eprintln!("the walker's height every 2 s: {heights:?}");
        let ledger = |g: &mut Game, name: &str| get(g, name, "WaterResident")["boundary"].clone();
        let (lake, falls) = (ledger(&mut g, "Lake"), ledger(&mut g, "Falls"));
        let number = |v: &Value, k: &str| v[k].as_f64().unwrap();
        eprintln!(
            "after 20 s: the lake's region holds {:.0} m^3, took {:.0} from its edge and gave {:.0} \
             to the falls'; the falls' region holds {:.0} m^3, took {:.0} and let {:.0} go east",
            number(&lake, "current_volume"),
            number(&lake, "entered_total"),
            number(&lake, "left_total"),
            number(&falls, "current_volume"),
            number(&falls, "entered_total"),
            number(&falls, "left_total"),
        );
        // The river runs from one region into the other across their seam...
        assert!(number(&lake, "left_total") > 20.0);
        assert!(
            (number(&lake, "left_total") - number(&falls, "entered_total")).abs()
                < 1e-3 * number(&lake, "left_total"),
            "the seam gives what it takes"
        );
        // ...and the falls' region holds what it took, less what left at its east edge, which
        // lets water go once it flows there.
        let gained = number(&falls, "current_volume") - number(&falls, "initial_volume");
        let net = number(&falls, "entered_total") - number(&falls, "left_total");
        assert!((gained - net).abs() < 0.01 * net, "{gained} against {net}");
        // The walker, set down 2 m over the ground, stands on the terrain's derived collider: the
        // ground there lies about 6 m up.
        let last = heights[heights.len() - 1];
        assert!(last > 5.0 && last < 8.0, "{heights:?}");
        assert!(
            (last - heights[heights.len() - 2]).abs() < 0.01,
            "{heights:?}"
        );
    });
}
