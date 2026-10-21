#![allow(dead_code)]
use amoris_link::Source;
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};
pub fn shore() -> GameSetup {
    Project::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-shore"),
    )
    .unwrap()
    .setup(false)
    .unwrap()
}
pub fn tank(cell: f64) -> GameSetup {
    let mut s = shore();
    s.scene.entities.retain(|e| {
        !e.components.contains_key("Floater") && !e.components.contains_key("WaterProbe")
    });
    let terrain = &mut s.scene.entities[0].components;
    terrain["Transform"]["position"] = json!([0, 0, 0]);
    terrain["Voxels"]["cell_m"] = json!(cell);
    terrain["Voxels"]["chunks"] = json!([]);
    let nx = (32.0 / cell) as u32;
    let r = &mut s.scene.entities[1].components["WaterRegion"];
    r["cell_m"] = json!(cell);
    r["size"] = json!([nx, 16]);
    r["bed"] = json!({"Voxels":{"volume":1,"per":1,"y_lo":0,"y_hi":0}});
    r["edges"] = json!(["Sea", "Sea", "Wall", "Wall"]);
    r["friction"] = json!(0);
    r["tiles"]=json!((0..nx/16).map(|x|json!({"key":[x,0],"bed":vec![0.0;256],"depth":vec![0.125;256],"u":vec![0.0;256],"w":vec![0.0;256]})).collect::<Vec<_>>());
    s.scene.entities[2].components["Sea"] = json!({"level":0.125,"density":1000,"waves":[{"length":16,"height":0.002,"toward_deg":90,"phase":0}]});
    for (i, x) in [2.125, 4.125, 6.125].iter().enumerate() {
        s.scene.entities.push(serde_json::from_value(json!({"name":format!("Probe{i}"),"components":{"WaterProbe":{"region":2,"at":[x,2.0]}}})).unwrap());
    }
    s
}
pub fn read(g: &mut Game, name: &str, fields: Value) -> Value {
    g.apply(&Command::new(
        Source::Developer(0),
        1,
        "world.get",
        json!({"entity":name,"fields":fields}),
    ))
    .unwrap()["fields"]
        .clone()
}
pub fn probes(g: &mut Game) -> Value {
    g.apply(&Command::new(
        Source::Developer(0),
        1,
        "world.query",
        json!({"with":["WaterProbe"],"limit":u32::MAX,"fields":["WaterProbe"]}),
    ))
    .unwrap()
}
pub fn steps(g: &mut Game, n: u32) {
    for _ in 0..n {
        let out = g.step().unwrap();
        assert!(out.errors.is_empty(), "{:?}", out.errors);
    }
}

/// Independent three-parameter harmonic fit: amplitude, propagation phase, RMS residual.
pub fn harmonic(samples: &[(f64, [f64; 3])], probe: usize) -> [f64; 3] {
    let omega = (9.81 * std::f64::consts::TAU / 16.0).sqrt();
    let mut a = [[0.0; 4]; 3];
    for (t, y) in samples {
        let x = [(omega * t).sin(), (omega * t).cos(), 1.0];
        for i in 0..3 {
            for j in 0..3 {
                a[i][j] += x[i] * x[j];
            }
            a[i][3] += x[i] * y[probe];
        }
    }
    for i in 0..3 {
        let pivot = a[i][i];
        assert!(pivot.abs() > 1e-8);
        for value in &mut a[i][i..] {
            *value /= pivot;
        }
        let row = a[i];
        for (k, target) in a.iter_mut().enumerate() {
            if k == i {
                continue;
            }
            let f = target[i];
            for (value, p) in target[i..].iter_mut().zip(&row[i..]) {
                *value -= f * p;
            }
        }
    }
    let [s, c, mean] = [a[0][3], a[1][3], a[2][3]];
    let rms = (samples
        .iter()
        .map(|(t, y)| (s * (omega * t).sin() + c * (omega * t).cos() + mean - y[probe]).powi(2))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt();
    [s.hypot(c), c.atan2(-s), rms]
}
