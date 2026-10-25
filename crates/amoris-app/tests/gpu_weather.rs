//! The wind and the rain on the water (water.md 67): a steady wind piles a closed pool's water
//! downwind as far as its stress over the water's weight says, and drifts what floats at a few
//! percent of its speed; rain adds its depth a millimetre at a time, every drop in the ledger.
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

fn step(g: &mut Game) {
    let r = g.step().unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

fn entity(name: &str, components: Value) -> amoris_runtime::SceneEntity {
    amoris_runtime::SceneEntity {
        name: Some(name.into()),
        prefab: None,
        components: serde_json::from_value(components).unwrap(),
    }
}

/// A walled pool of `size` half-metre columns over a flat bed whose physics take the wind and the
/// rain, its water a metre deep with its surface tilted by `tilt` metres from end to end along x;
/// and `weather`.
fn basin(size: [u32; 2], tilt: f64, weather: &[(&str, Value)]) -> amoris_runtime::GameSetup {
    let mut setup = Project::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-resident"),
    )
    .unwrap()
    .setup(false)
    .unwrap();
    setup.scene.entities.truncate(1);
    let tiles: Vec<Value> = (0..size[1] / 16)
        .flat_map(|kz| (0..size[0] / 16).map(move |kx| (kx, kz)))
        .map(|(kx, kz)| {
            let depth: Vec<f64> = (0..256)
                .map(|c| {
                    let x = (f64::from(kx * 16 + c % 16) + 0.5) / f64::from(size[0]);
                    1.0 + tilt * (x - 0.5)
                })
                .collect();
            json!({"key": [kx, kz], "bed": vec![0.0; 256], "depth": depth,
                "u": vec![0.0; 256], "w": vec![0.0; 256]})
        })
        .collect();
    setup.scene.entities.push(entity(
        "Pool",
        json!({"Transform": {"position": [0.0, 0.0, 0.0]}, "WaterRegion": {"solver": "gpu_resident",
            "cell_m": 0.5, "size": size,
            "bed": {"Voxels": {"volume": 1, "per": 2, "y_lo": 0, "y_hi": 0}},
            "edges": ["Wall", "Wall", "Wall", "Wall"], "dry_below": 0, "tiles": tiles,
            "physics": {"wind": true, "rain": true}}}),
    ));
    for (name, components) in weather {
        setup.scene.entities.push(entity(name, components.clone()));
    }
    setup
}

/// A pool 32 x 8 m, level, with a probe every metre along z = 4 m, and `weather`.
fn pool(weather: &[(&str, Value)]) -> Game {
    let mut setup = basin([64, 16], 0.0, weather);
    for n in 0..32 {
        setup.scene.entities.push(entity(
            &format!("Probe{n}"),
            json!({"WaterProbe": {"region": 2, "at": [0.5 + f64::from(n), 4.0]}}),
        ));
    }
    water_compute::builder(Arc::new(setup), 1).build().unwrap()
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

#[test]
fn a_steady_wind_piles_a_pools_water_downwind() {
    run(|| {
        // 15 m/s toward +x.
        let mut g = pool(&[("Wind", json!({"Wind": {"from_deg": 270.0, "speed": 15.0}}))]);
        for _ in 0..2400 {
            step(&mut g);
        }
        // Over the next 10 s the seiche the wind started has died down: average it out.
        let mut mean = vec![0.0; 32];
        for _ in 0..60 {
            for _ in 0..10 {
                step(&mut g);
            }
            for (m, s) in mean.iter_mut().zip(surfaces(&mut g)) {
                *m += s / 60.0;
            }
        }
        // The surface's slope by least squares, against the stress over the water's weight:
        // rho_air C_D U^2 / (rho_water g h), C_D = (0.8 + 0.065 U) 1e-3 (J. Wu 1982).
        let xs: Vec<f64> = (0..32).map(|n| 0.5 + f64::from(n)).collect();
        let (xm, ym) = (
            xs.iter().sum::<f64>() / 32.0,
            mean.iter().sum::<f64>() / 32.0,
        );
        let slope = xs
            .iter()
            .zip(&mean)
            .map(|(x, y)| (x - xm) * (y - ym))
            .sum::<f64>()
            / xs.iter().map(|x| (x - xm) * (x - xm)).sum::<f64>();
        let cd = (0.8 + 0.065 * 15.0) * 1e-3;
        let expected = 1.225e-3 * cd * 15.0 * 15.0 / 9.81;
        eprintln!(
            "the wind's setup: {:.3} mm over the pool, slope {slope:.3e} against {expected:.3e}",
            (mean[31] - mean[0]) * 1e3
        );
        assert!(
            (slope - expected).abs() < 0.15 * expected,
            "{slope} against {expected}"
        );
        // Without the wind the pool lies level.
        let mut calm = pool(&[]);
        for _ in 0..600 {
            step(&mut calm);
        }
        let level = surfaces(&mut calm);
        assert!(
            (level[31] - level[0]).abs() < 1e-5,
            "{} {}",
            level[0],
            level[31]
        );
    });
}

#[test]
fn rain_adds_its_millimetres_to_every_column_and_the_ledger() {
    run(|| {
        // A cloudburst of 360 mm an hour: 0.1 mm a second, three millimetres in 30 s, of which the
        // whole millimetres reach the columns and the rest is owed.
        let mut g = pool(&[(
            "Rain",
            json!({"Rain": {"intensity_mm_h": 360.0, "drop_mm": 3.0}}),
        )]);
        step(&mut g);
        let before = get(&mut g, "Probe10", "WaterProbe")["depth"]
            .as_f64()
            .unwrap();
        for _ in 0..1800 {
            step(&mut g);
        }
        let ledger = get(&mut g, "Pool", "WaterResident")["boundary"].clone();
        let n = |k: &str| ledger[k].as_f64().unwrap();
        let rained = n("rained_total");
        let owed = get(&mut g, "Pool", "WaterResident")["rain_owed_m"]
            .as_f64()
            .unwrap();
        let area = 64.0 * 16.0 * 0.25;
        eprintln!(
            "rained {rained:.6} m^3; the water grew {:.6} m^3, residual {:.2e}",
            n("current_volume") - n("initial_volume"),
            n("numerical_residual")
        );
        assert!(
            (rained / area + owed - 1e-4 * 1801.0 / 60.0).abs() < 1e-12,
            "every drop: {rained} m^3 fallen and {owed} m owed"
        );
        assert!(rained / area >= 0.002 && owed < 0.001, "{rained} {owed}");
        assert!(
            (n("current_volume") - n("initial_volume") - rained).abs() < 1e-3 * rained,
            "{ledger}"
        );
        let after = get(&mut g, "Probe10", "WaterProbe")["depth"]
            .as_f64()
            .unwrap();
        assert!(
            (after - before - rained / area).abs() < 2e-5,
            "the depth rose {}",
            after - before
        );
    });
}

#[test]
fn the_wind_drifts_what_floats_downwind() {
    // A crate 0.6 m across, as light as pine, floating at rest in the pool, with and without a
    // wind of 10 m/s toward +x; its mean speed from 10 s to 40 s.
    let drift = |wind: bool| {
        let points: Vec<Value> = (0..27)
            .map(|n| {
                let at = [n % 3, n / 3 % 3, n / 9].map(|c| 0.2 * (f64::from(c) - 1.0));
                json!({"at": at, "volume": 0.008, "size": 0.2})
            })
            .collect();
        let crate_ = json!({"Transform": {"position": [6.0, 1.09, 4.0]},
            "Velocity": {"linear": [0, 0, 0], "angular": [0, 0, 0]},
            "RigidBody": {"kind": "Dynamic"},
            "Collider": {"shape": {"Cuboid": {"half_extents": [0.3, 0.3, 0.3]}}, "density": 350},
            "ExternalForce": {}, "Floater": {"points": points}});
        let mut weather = vec![("Crate", crate_)];
        if wind {
            weather.push(("Wind", json!({"Wind": {"from_deg": 270.0, "speed": 10.0}})));
        }
        let mut g = pool(&weather);
        let x = |g: &mut Game| {
            get(g, "Crate", "Transform")["position"][0]
                .as_f64()
                .unwrap()
        };
        for _ in 0..600 {
            step(&mut g);
        }
        let start = x(&mut g);
        for _ in 0..1800 {
            step(&mut g);
        }
        (x(&mut g) - start) / 30.0
    };
    run(move || {
        let (blown, still) = (drift(true), drift(false));
        eprintln!("the crate's mean speed downwind: {blown:.3} m/s, {still:.3} m/s without wind");
        // The surface drifts at 3% of 10 m/s, the water under the crate's bottom less.
        assert!(blown > 0.15 && blown < 0.32, "{blown}");
        assert!(still.abs() < 0.01, "{still}");
    });
}

/// What the weather costs a tick (liquids.md 19): the wall time of a tick, its GPU work and
/// readbacks waited for, over ticks 61 to 360 of a pool of 512 x 256 half-metre columns whose
/// surface starts tilted by 5 cm, so it sloshes whatever the weather: calm, under a gusting gale,
/// in rain. `cargo test --release -p amoris-app --test gpu_weather time_weather -- --ignored
/// --nocapture`.
#[test]
#[ignore]
#[allow(clippy::disallowed_methods)] // the test's own clock stands outside every tick
fn time_weather() {
    run(|| {
        let gale = json!({"Wind": {"from_deg": 270.0, "speed": 12.0, "gust": 0.3,
            "gust_period": 7.0, "gust_length": 45.0}});
        let rain = json!({"Rain": {"intensity_mm_h": 20.0, "drop_mm": 2.5}});
        for (name, weather) in [
            ("calm", vec![]),
            ("gale", vec![("Wind", gale)]),
            ("rain", vec![("Rain", rain)]),
        ] {
            let setup = basin([512, 256], 0.05, &weather);
            let mut g = water_compute::builder(Arc::new(setup), 1).build().unwrap();
            for _ in 0..60 {
                step(&mut g);
            }
            let start = std::time::Instant::now();
            for _ in 0..300 {
                step(&mut g);
            }
            eprintln!(
                "{name}: {:.3} ms a tick",
                start.elapsed().as_secs_f64() * 1e3 / 300.0
            );
        }
    });
}
