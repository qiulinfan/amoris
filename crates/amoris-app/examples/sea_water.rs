//! Actual game-boundary measurements, not an analytic rendering substitute or a timing harness.
#[path = "../tests/common/water_sea.rs"]
mod experiment;
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_assets::Feed;
use amoris_runtime::Extractor;
use serde_json::json;
use std::sync::Arc;
fn run() {
    let args = std::env::args().collect::<Vec<_>>();
    let value = |name: &str| {
        args.windows(2)
            .find(|a| a[0] == name)
            .map(|a| a[1].as_str())
    };
    let shore = args.iter().any(|a| a == "--shore");
    let cell = value("--cell").map_or(0.25, |s| s.parse::<f64>().unwrap());
    let ticks = value("--ticks").map_or(if shore { 2400 } else { 1200 }, |s| {
        s.parse::<u32>().unwrap()
    });
    let path =
        std::path::PathBuf::from(value("--out").unwrap_or("out/water-sea/quality/tank-025.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let project = value("--project");
    let setup = if let Some(project) = project {
        amoris_runtime::Project::load(std::path::Path::new(project))
            .unwrap()
            .setup(false)
            .unwrap()
    } else if shore {
        experiment::shore()
    } else {
        experiment::tank(cell)
    };
    let sea = setup.scene.entities[2].components["Sea"].clone();
    let mut g = water_compute::builder(Arc::new(setup), 1).build().unwrap();
    let feed = Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::default();
    let mut rows = Vec::new();
    let mut checks = Vec::new();
    let mut merged = amoris_assets::frame::RenderFrame::default();
    for tick in 1..=ticks {
        experiment::steps(&mut g, 1);
        g.present(&mut extractor, &feed);
        let frame = mailbox.take();
        if tick % 64 == 0 {
            let cp = frame.resident_water[0].checkpoint.as_ref().unwrap();
            let a = if shore { 0.25f64 } else { cell };
            let volume = cp
                .columns
                .iter()
                .map(|c| f64::from(f32::from_bits(c[1])) * a * a)
                .sum::<f64>();
            let b = experiment::read(&mut g, "Shore", json!(["WaterResident.boundary"]));
            checks.push(
                json!({"tick":tick,"actual_volume":volume,"boundary":b["WaterResident.boundary"]}),
            );
        }
        merged.merge(frame).unwrap();
        if tick == 1 || tick % 5 == 0 || tick == ticks {
            let probes = experiment::probes(&mut g);
            let body = if shore {
                experiment::read(
                    &mut g,
                    "Float",
                    json!(["Transform.position", "Velocity.linear", "Floater.submerged"]),
                )
            } else {
                json!(null)
            };
            rows.push(json!({"tick":tick,"time":f64::from(tick)/60.0,"probes":probes,"body":body}));
        }
        if [1, 64, 600, 1200, 1800, 2400].contains(&tick) || tick == ticks {
            std::fs::write(
                path.parent().unwrap().join(format!(
                    "{}-tick-{tick}.bin",
                    if shore {
                        "shore"
                    } else if cell == 0.25 {
                        "tank-025"
                    } else {
                        "tank-050"
                    }
                )),
                merged.encode(),
            )
            .unwrap();
        }
    }
    let state = experiment::read(
        &mut g,
        "Shore",
        json!([
            "WaterResident.stats",
            "WaterResident.boundary",
            "WaterResident.controls"
        ]),
    );
    std::fs::write(&path,serde_json::to_vec_pretty(&json!({"scope":"actual native Game, resident GPU sea forcing, current script probes, independent checkpoint-volume sums; not timing","shore":shore,"project":project,"cell_m":if shore {0.25}else{cell},"ticks":ticks,"sea":sea,"state":state,"checkpoints":checks,"rows":rows})).unwrap()).unwrap();
    println!("sea quality: {}", path.display());
}
fn main() {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
