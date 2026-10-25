//! Bodies and resident GPU water together never gain energy (water.md 56): four floats dropped
//! into a closed lake with no friction or calm damping push its water aside and drag it, and the
//! energy of the floats and the water together only ever leaves. While a float felt its own push
//! on the water a tick late, that late spring pumped the lake: the energy left from the drop rose
//! from a quarter at 20 s to half at 150 s.
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};
use std::sync::Arc;

const RHO: f64 = 1000.0;
const G: f64 = 9.81;

fn setup() -> Arc<GameSetup> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/water-feedback");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    let lake = &mut setup.scene.entities[1].components;
    lake.insert("WaterDisplacement".into(), json!({}));
    let region = &mut lake["WaterRegion"];
    region["friction"] = json!(0.0);
    region["calm_per_s"] = json!(0.0);
    for n in 0..4 {
        let float = &mut setup.scene.entities[2 + n].components;
        float["Transform"]["position"][1] = json!(2.09 + 0.6);
        float["Velocity"]["linear"] = json!([0.0, 0.0, 0.0]);
        float["Floater"]["added_mass"] = json!(0.5);
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
    .unwrap()
}
fn big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

/// The floats' kinetic energy, translation and rotation, and their weight's potential, J.
fn floats(game: &mut Game) -> f64 {
    // 0.6 m cubes of 350 kg/m^3.
    let mass = 350.0 * 0.216;
    let inertia = mass * 0.36 / 6.0;
    (0..4)
        .map(|n| {
            let f = &read(game, &format!("Float{n}"), json!(["Transform", "Velocity"]))["fields"];
            let v: Vec<f64> = (0..3)
                .map(|a| f["Velocity"]["linear"][a].as_f64().unwrap())
                .collect();
            let w: Vec<f64> = (0..3)
                .map(|a| f["Velocity"]["angular"][a].as_f64().unwrap())
                .collect();
            let y = f["Transform"]["position"][1].as_f64().unwrap();
            0.5 * mass * v.iter().map(|x| x * x).sum::<f64>()
                + 0.5 * inertia * w.iter().map(|x| x * x).sum::<f64>()
                + mass * G * y
        })
        .sum()
}

/// The lake's energy over flat water at rest, J, from the checkpoint its last compaction read
/// back: kinetic on each face between wet columns, potential from the surface's departure from
/// its mean level. `None` between compactions.
fn lake(game: &mut Game, id: u64) -> Option<f64> {
    let world = game.world();
    let e = amoris_sim::entity::require(world, amoris_sim::EntityId::new(id).unwrap()).unwrap();
    let resident = world
        .get::<amoris_runtime::water_resident::WaterResident>(e)
        .unwrap();
    if !resident.recipe.steps().is_empty() {
        return None;
    }
    let checkpoints = world.resource::<amoris_runtime::water_resident::WaterCheckpoints>();
    let field = checkpoints
        .fields
        .get(&resident.recipe.checkpoint())?
        .as_raw();
    let cell = 0.25;
    let at = |i: u32, k: u32| field.columns[(i + field.size[0] * k) as usize].map(f32::from_bits);
    let (mut kinetic, mut sum, mut squares, mut count) = (0.0, 0.0, 0.0, 0.0);
    for k in 0..field.size[1] {
        for i in 0..field.size[0] {
            let c = at(i, k);
            if c[1] <= 0.0 {
                continue;
            }
            let surface = f64::from(c[0] + c[1]);
            sum += surface;
            squares += surface * surface;
            count += 1.0;
            if i > 0 && at(i - 1, k)[1] > 0.0 {
                kinetic += 0.25 * f64::from(c[1] + at(i - 1, k)[1]) * f64::from(c[2] * c[2]);
            }
            if k > 0 && at(i, k - 1)[1] > 0.0 {
                kinetic += 0.25 * f64::from(c[1] + at(i, k - 1)[1]) * f64::from(c[3] * c[3]);
            }
        }
    }
    let potential = 0.5 * G * (squares - sum * sum / count);
    Some(RHO * (kinetic + potential) * cell * cell)
}

#[test]
fn floats_and_their_lake_never_gain_energy_together() {
    big_stack(|| {
        let mut game = water_compute::builder(setup(), 1).build().unwrap();
        let id = read(&mut game, "Lake", json!(["WaterRegion"]))["id"]
            .as_u64()
            .unwrap();
        // The floats at rest over still water: their drop is all the energy there is.
        let start = floats(&mut game);
        let rest = 4.0 * 350.0 * 0.216 * G * 2.09;
        let drop = start - rest;
        let mut samples = Vec::new();
        let seconds: u32 = std::env::var("ENERGY_SECONDS").map_or(60, |v| v.parse().unwrap());
        for tick in 1..=(seconds * 60) {
            let report = game.step().unwrap();
            assert!(report.errors.is_empty(), "{:?}", report.errors);
            if let Some(water) = lake(&mut game, id) {
                let bodies = floats(&mut game);
                samples.push((tick, bodies - rest, water));
            }
        }
        for (tick, bodies, water) in &samples {
            eprintln!(
                "{:5.2} s: floats {bodies:8.1} J, lake {water:8.1} J, together {:.3} of the drop",
                f64::from(*tick) / 60.0,
                (bodies + water) / drop
            );
        }
        assert!(samples.len() >= 40, "{} compactions", samples.len());
        let together: Vec<f64> = samples.iter().map(|s| (s.1 + s.2) / drop).collect();
        let most = together.iter().copied().fold(f64::MIN, f64::max);
        assert!(
            most < 1.02,
            "the floats and the lake reached {most} of the drop's energy"
        );
        // Past the splash, each quarter minute holds less energy than the one before: the
        // waves the floats leave lose energy to them and to the step, and none comes back.
        let means: Vec<f64> = (1..seconds / 15)
            .map(|w| {
                let window: Vec<f64> = samples
                    .iter()
                    .zip(&together)
                    .filter(|((tick, _, _), _)| *tick / 900 == w)
                    .map(|(_, e)| *e)
                    .collect();
                window.iter().sum::<f64>() / window.len() as f64
            })
            .collect();
        eprintln!("quarter-minute means from 15 s: {means:.3?}");
        assert!(
            means.windows(2).all(|w| w[1] < w[0]),
            "the energy left rose: {means:?}"
        );
    });
}
