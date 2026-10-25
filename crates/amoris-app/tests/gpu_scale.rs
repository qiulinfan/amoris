//! Water at scale (water.md 64): a steady river bakes and stops stepping, keeping its flow, until
//! something disturbs it; a region beyond every focus holds still.
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

fn river(edit: impl FnOnce(&mut amoris_runtime::GameSetup)) -> Game {
    let mut setup = Project::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/scenes/water-river"),
    )
    .unwrap()
    .setup(false)
    .unwrap();
    edit(&mut setup);
    water_compute::builder(Arc::new(setup), 1).build().unwrap()
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

fn step(g: &mut Game) {
    let r = g.step().unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

fn river_entity(setup: &mut amoris_runtime::GameSetup) -> &mut serde_json::Map<String, Value> {
    &mut setup
        .scene
        .entities
        .iter_mut()
        .find(|e| e.name.as_deref() == Some("River"))
        .unwrap()
        .components
}

#[test]
fn a_steady_river_bakes_keeps_its_flow_and_wakes_when_disturbed() {
    run(|| {
        let mut g = river(|s| {
            river_entity(s).get_mut("WaterRegion").unwrap()["bake_after_s"] = json!(1.0);
        });
        // The river fills from its inflow and runs out of its outflow until it holds steady.
        let mut baked_at = None;
        let mut last = read(&mut g)["WaterResident"]["recipe"]["digest"].clone();
        for tick in 1..=7200u32 {
            step(&mut g);
            let state = read(&mut g);
            let digest = state["WaterResident"]["recipe"]["digest"].clone();
            if digest == last && state["WaterResident"]["steady"].as_u64().unwrap() >= 60 {
                baked_at = Some(tick);
                break;
            }
            last = digest;
        }
        let baked_at = baked_at.expect("the river holds steady and bakes within two minutes");
        let state = read(&mut g);
        let entered = state["WaterResident"]["boundary"]["entered_total"]
            .as_f64()
            .unwrap();
        let speed = state["WaterResident"]["stats"]["max_speed"]
            .as_f64()
            .unwrap();
        eprintln!("baked at tick {baked_at}, its fastest water {speed:.3} m/s");
        assert!(speed > 0.3, "a baked river keeps its flow: {speed}");
        // Baked, it steps no further: its state and its ledger stand while the ticks go by.
        for _ in 0..120 {
            step(&mut g);
        }
        let held = read(&mut g);
        assert_eq!(held["WaterResident"]["recipe"]["digest"], last);
        assert_eq!(
            held["WaterResident"]["boundary"]["entered_total"]
                .as_f64()
                .unwrap(),
            entered
        );
        // A change to its inflow wakes it.
        g.apply(&Command::new(Source::Developer(0), 2, "world.edit", json!({"label": "more inflow",
            "ops": [{"set": {"entity": "River", "component": "WaterRegion",
                "value": {"edges": [{"Inflow": {"m3s_per_m": 1.2}}, "Outflow", "Wall", "Wall"]}}}]})))
            .unwrap();
        step(&mut g);
        step(&mut g);
        let woken = read(&mut g);
        assert_ne!(woken["WaterResident"]["recipe"]["digest"], last);
        assert_eq!(woken["WaterResident"]["steady"].as_u64().unwrap(), 0);
        assert!(
            woken["WaterResident"]["boundary"]["entered_total"]
                .as_f64()
                .unwrap()
                > entered,
            "{woken}"
        );
    });
}

#[test]
fn a_region_beyond_every_focus_holds_still_until_one_comes_near() {
    run(|| {
        let mut g = river(|s| {
            s.scene.entities.push(amoris_runtime::SceneEntity {
                name: Some("Focus".into()),
                prefab: None,
                components: serde_json::from_value(json!({
                    "Transform": {"position": [500.0, 0.0, 500.0]},
                    "WaterFocus": {"radius_m": 20.0}
                }))
                .unwrap(),
            });
        });
        // The first tick seeds the field; beyond the focus it steps no further.
        step(&mut g);
        let seeded = read(&mut g);
        let first = seeded["WaterResident"]["recipe"]["digest"].clone();
        let entered = seeded["WaterResident"]["boundary"]["entered_total"].clone();
        for _ in 0..60 {
            step(&mut g);
        }
        let away = read(&mut g);
        assert_eq!(away["WaterResident"]["recipe"]["digest"], first);
        assert_eq!(away["WaterResident"]["boundary"]["entered_total"], entered);
        // The focus comes within 20 m of the river, 16 x 8 m from the origin: it runs.
        g.apply(&Command::new(
            Source::Developer(0),
            2,
            "world.edit",
            json!({"label": "come near",
            "ops": [{"set": {"entity": "Focus", "component": "Transform",
                "value": {"position": [10.0, 0.0, 12.0]}}}]}),
        ))
        .unwrap();
        for _ in 0..60 {
            step(&mut g);
        }
        let near = read(&mut g);
        assert_ne!(near["WaterResident"]["recipe"]["digest"], first);
        assert!(
            near["WaterResident"]["boundary"]["entered_total"]
                .as_f64()
                .unwrap()
                > 0.5
        );
    });
}

/// A pool of 0.5 m columns from world x = `x`, flat-bedded, 1 m deep with a 0.2 m hump across
/// x = 8 m.
fn pool(x: f64, size: [u32; 2], edges: Value) -> Value {
    let tiles: Vec<Value> = (0..size[1] / 16)
        .flat_map(|kz| (0..size[0] / 16).map(move |kx| (kx, kz)))
        .map(|(kx, kz)| {
            let depth: Vec<f64> = (0..256)
                .map(|c| {
                    let at = x + (f64::from(kx * 16 + c % 16) + 0.5) * 0.5;
                    1.0 + 0.2 * (-(at - 8.0) * (at - 8.0) / 2.0).exp()
                })
                .collect();
            json!({"key": [kx, kz], "bed": vec![0.0; 256], "depth": depth,
                "u": vec![0.0; 256], "w": vec![0.0; 256]})
        })
        .collect();
    json!({"Transform": {"position": [x, 0.0, 0.0]}, "WaterRegion": {"solver": "gpu_resident",
        "cell_m": 0.5, "size": size, "bed": {"Voxels": {"volume": 1, "per": 2, "y_lo": 0, "y_hi": 0}},
        "edges": edges, "friction": 0.0, "calm_per_s": 0.0, "dry_below": 0, "tiles": tiles}})
}

/// Two 16 x 8 m pools joined along x = 16 m (regions 2 and 3), or one 32 x 8 m pool (region 2),
/// with a probe every metre along z = 4 m from x = 0.5 m.
fn pools(joined: bool) -> Game {
    pools_of(joined, true)
}

/// The two pools side by side, walled from each other unless `joined`, or one pool of both.
fn pools_of(two: bool, joined: bool) -> Game {
    let mut setup = Project::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/scenes/water-resident"),
    )
    .unwrap()
    .setup(false)
    .unwrap();
    let entity = |name: &str, components: Value| amoris_runtime::SceneEntity {
        name: Some(name.into()),
        prefab: None,
        components: serde_json::from_value(components).unwrap(),
    };
    setup.scene.entities.truncate(1);
    if two {
        let [east, west] = if joined {
            [
                json!({"Joined": {"region": 3}}),
                json!({"Joined": {"region": 2}}),
            ]
        } else {
            [json!("Wall"), json!("Wall")]
        };
        setup.scene.entities.push(entity(
            "West",
            pool(0.0, [32, 16], json!(["Wall", east, "Wall", "Wall"])),
        ));
        setup.scene.entities.push(entity(
            "East",
            pool(16.0, [32, 16], json!([west, "Wall", "Wall", "Wall"])),
        ));
    } else {
        setup.scene.entities.push(entity(
            "Whole",
            pool(0.0, [64, 16], json!(["Wall", "Wall", "Wall", "Wall"])),
        ));
    }
    for n in 0..32 {
        let x = 0.5 + f64::from(n);
        let region = if two && x > 16.0 { 3 } else { 2 };
        setup.scene.entities.push(entity(
            &format!("Probe{n}"),
            json!({"WaterProbe": {"region": region, "at": [x, 4.0]}}),
        ));
    }
    water_compute::builder(Arc::new(setup), 1).build().unwrap()
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

fn surfaces(g: &mut Game) -> Vec<f64> {
    (0..32)
        .map(|n| {
            get(g, &format!("Probe{n}"), "WaterProbe")["surface"]
                .as_f64()
                .unwrap()
        })
        .collect()
}

/// Water in the pools, its first tick's and now, m^3.
fn volumes(g: &mut Game, names: &[&str]) -> (f64, f64) {
    names.iter().fold((0.0, 0.0), |(first, now), name| {
        let ledger = get(g, name, "WaterResident")["boundary"].clone();
        (
            first + ledger["initial_volume"].as_f64().unwrap(),
            now + ledger["current_volume"].as_f64().unwrap(),
        )
    })
}

#[test]
fn joined_pools_pass_a_wave_as_one_pool_does() {
    run(|| {
        let mut joined = pools(true);
        let mut whole = pools(false);
        let mut worst = 0.0f64;
        // The hump parts into two waves 0.1 m high at 3.1 m/s: the east one crosses the seam at
        // about 2.6 s, runs into the far wall at 7.7 s and back across the seam at 12.8 s.
        for tick in 1..=900u32 {
            step(&mut joined);
            step(&mut whole);
            if tick % 60 == 0 {
                let (a, b) = (surfaces(&mut joined), surfaces(&mut whole));
                let off = a
                    .iter()
                    .zip(&b)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f64::max);
                let wave = b.iter().map(|s| (s - 1.0).abs()).fold(0.0, f64::max);
                eprintln!(
                    "{:.0} s: joined off by {off:.4} m where the wave is {wave:.3} m",
                    f64::from(tick) / 60.0
                );
                worst = worst.max(off);
            }
        }
        let (first, now) = volumes(&mut joined, &["West", "East"]);
        let (whole_first, whole_now) = volumes(&mut whole, &["Whole"]);
        eprintln!(
            "joined volume {first:.4} -> {now:.4} m^3 ({:+.2e}); one pool {whole_first:.4} -> \
             {whole_now:.4} ({:+.2e}); the hump holds {:.3} m^3",
            now - first,
            whole_now - whole_first,
            0.2 * (2.0 * std::f64::consts::PI).sqrt() * 8.0
        );
        assert!(
            worst < 0.003,
            "the joined pools follow the one pool: {worst}"
        );
        // Each seam is the same for both regions: the water one gives, the other receives.
        assert!((now - first).abs() < 2e-4, "{}", now - first);
    });
}

/// Steps until every named region's state holds from one tick to the next; the tick it held.
fn bake(g: &mut Game, names: &[&str]) -> (u32, Vec<Value>) {
    let digests = |g: &mut Game| -> Vec<Value> {
        names
            .iter()
            .map(|name| get(g, name, "WaterResident")["recipe"]["digest"].clone())
            .collect()
    };
    let mut last = digests(g);
    for tick in 1..=7200u32 {
        step(g);
        let now = digests(g);
        if now == last {
            return (tick, now);
        }
        last = now;
    }
    panic!("the river holds steady and bakes within two minutes");
}

fn fastest(g: &mut Game, name: &str) -> f64 {
    get(g, name, "WaterResident")["stats"]["max_speed"]
        .as_f64()
        .unwrap()
}

#[test]
fn a_river_split_in_two_bakes_as_one_and_wakes_as_one() {
    run(|| {
        let mut whole = river(|s| {
            river_entity(s).get_mut("WaterRegion").unwrap()["bake_after_s"] = json!(1.0);
        });
        bake(&mut whole, &["River"]);
        let unsplit = fastest(&mut whole, "River");
        // The river's first 4 m keep its inflow; the rest, from 4 m on, its pier and its outflow.
        // A seam carries the flow across it, not the flow along it: it lies in a calm reach.
        let mut g = river(|s| {
            let lower = s.scene.entities.len() as u64 + 1;
            let upper = river_entity(s).clone();
            let tiles = upper["WaterRegion"]["tiles"].as_array().unwrap().clone();
            let (mut a, mut b) = (upper.clone(), upper);
            for (part, half) in [(&mut a, 0), (&mut b, 1)] {
                let region = part.get_mut("WaterRegion").unwrap();
                region["size"] = json!([if half == 0 { 16 } else { 48 }, 32]);
                region["bake_after_s"] = json!(1.0);
                region["tiles"] = json!(
                    tiles
                        .iter()
                        .filter(|t| u64::from(t["key"][0].as_u64().unwrap() >= 1) == half)
                        .map(|t| {
                            let mut t = t.clone();
                            t["key"][0] = json!(t["key"][0].as_u64().unwrap() - half);
                            t
                        })
                        .collect::<Vec<_>>()
                );
            }
            let inflow = a["WaterRegion"]["edges"][0].clone();
            a["WaterRegion"]["edges"] =
                json!([inflow, {"Joined": {"region": lower}}, "Wall", "Wall"]);
            a.remove("WaterOutletStage");
            b["WaterRegion"]["edges"] =
                json!([{"Joined": {"region": 2}}, "Outflow", "Wall", "Wall"]);
            b.insert("Transform".into(), json!({"position": [4.0, 0.0, 0.0]}));
            *river_entity(s) = a;
            s.scene.entities.push(amoris_runtime::SceneEntity {
                name: Some("Lower".into()),
                prefab: None,
                components: b,
            });
        });
        let (baked_at, held) = bake(&mut g, &["River", "Lower"]);
        let split = fastest(&mut g, "Lower");
        eprintln!(
            "baked at tick {baked_at}, its fastest water {split:.3} m/s (one region: {unsplit:.3})"
        );
        assert!(
            (split - unsplit).abs() < 0.02 * unsplit,
            "{split} against {unsplit}"
        );
        for name in ["River", "Lower"] {
            assert!(
                get(&mut g, name, "WaterResident")["steady"]
                    .as_u64()
                    .unwrap()
                    >= 60
            );
        }
        // Both halves rest together...
        for _ in 0..120 {
            step(&mut g);
        }
        assert_eq!(bake(&mut g, &["River", "Lower"]).1, held);
        // ...and more inflow at the top wakes the bottom too.
        let mut edges = get(&mut g, "River", "WaterRegion")["edges"].clone();
        edges[0] = json!({"Inflow": {"m3s_per_m": 1.2}});
        g.apply(&Command::new(
            Source::Developer(0),
            2,
            "world.edit",
            json!({"label": "more inflow",
            "ops": [{"set": {"entity": "River", "component": "WaterRegion",
                "value": {"edges": edges}}}]}),
        ))
        .unwrap();
        step(&mut g);
        let woken: Vec<Value> = ["River", "Lower"]
            .iter()
            .map(|name| get(&mut g, name, "WaterResident")["recipe"]["digest"].clone())
            .collect();
        assert!(woken[0] != held[0] && woken[1] != held[1], "{woken:?}");
    });
}

/// The cost of a tick (liquids.md 16): the river stepping and baked, and the joined pools against
/// the one pool. `cargo test --release -p amoris-app --test gpu_scale time_scale -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn time_scale() {
    run(|| {
        let ms = |g: &mut Game, ticks: u32| {
            let start = std::time::Instant::now();
            for _ in 0..ticks {
                step(g);
            }
            start.elapsed().as_secs_f64() * 1e3 / f64::from(ticks)
        };
        let mut g = river(|s| {
            river_entity(s).get_mut("WaterRegion").unwrap()["bake_after_s"] = json!(1.0);
        });
        for _ in 0..300 {
            step(&mut g);
        }
        let stepping = ms(&mut g, 600);
        bake(&mut g, &["River"]);
        let baked = ms(&mut g, 600);
        let mut games = [pools_of(true, true), pools_of(true, false), pools(false)];
        for g in &mut games {
            for _ in 0..60 {
                step(g);
            }
        }
        let [joined, apart, one] = games.map(|mut g| ms(&mut g, 600));
        eprintln!(
            "a tick: the river stepping {stepping:.3} ms, baked {baked:.3} ms; two joined pools \
             {joined:.3} ms, the two walled apart {apart:.3} ms, one pool of both {one:.3} ms"
        );
    });
}
