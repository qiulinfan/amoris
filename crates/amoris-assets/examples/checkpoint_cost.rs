//! CPU-only checkpoint costs. Not a fluid or complete-loop benchmark.
use amoris_assets::{
    frame::{RenderFrame, ResidentWaterUpdate, WaterLookView},
    water_field::{FieldCheckpoint, FieldRecipe},
};
use std::{hint::black_box, sync::Arc, time::Instant};
#[allow(clippy::disallowed_methods)] // CPU-only benchmark, never part of a game tick.
fn measure(mut f: impl FnMut(), n: u32) -> serde_json::Value {
    for _ in 0..3 {
        f();
    }
    let mut times = Vec::new();
    for _ in 0..n {
        let start = Instant::now();
        f();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(f64::total_cmp);
    serde_json::json!({"median_ms":times[times.len()/2],"p95_ms":times[times.len()*95/100]})
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let kind = args
        .windows(2)
        .find(|w| w[0] == "--kind")
        .map_or("dam", |w| w[1].as_str());
    assert!(matches!(kind, "dam" | "flat" | "dense"));
    let distinct = args.iter().any(|a| a == "--distinct");
    let make = |region: u32| {
        let mut seed = 7u32.wrapping_add(region);
        Arc::new(FieldCheckpoint::new(
            [512, 512],
            0.0,
            (0..512 * 512)
                .map(|n| {
                    if kind == "flat" {
                        return [0, (1.0f32 + region as f32 / 16.0).to_bits(), 0, 0];
                    }
                    if kind == "dense" {
                        let mut words = [0; 4];
                        for (lane, word) in words.iter_mut().enumerate() {
                            seed ^= seed << 13;
                            seed ^= seed >> 17;
                            seed ^= seed << 5;
                            *word = (seed & 0x807f_ffff) | ((120 + seed % 12) << 23);
                            if lane == 1 {
                                *word &= 0x7fff_ffff;
                            }
                        }
                        return words;
                    }
                    [
                        0,
                        if n % 512 < 256 {
                            (2.0f32 + region as f32 / 16.0).to_bits()
                        } else {
                            1.0f32.to_bits()
                        },
                        if (224..288).contains(&(n % 512)) {
                            0.5f32.to_bits()
                        } else {
                            0
                        },
                        0,
                    ]
                })
                .collect(),
        ))
    };
    let raw = make(0);
    let cp = Arc::new(raw.as_ref().clone().checked().unwrap());
    let recipe = FieldRecipe::new(cp.key(), vec![], cp.digest(), String::new());
    let frame = RenderFrame {
        resident_water: (0..4)
            .map(|id| {
                let c = if distinct {
                    Arc::new(make(id as u32).as_ref().clone().checked().unwrap())
                } else {
                    cp.clone()
                };
                let recipe = FieldRecipe::new(c.key(), vec![], c.digest(), String::new());
                ResidentWaterUpdate {
                    id,
                    origin: [id as f32 * 130.0, 0.0],
                    cell_m: 0.25,
                    look: WaterLookView::default(),
                    checkpoint: Some(c),
                    recipe: recipe.patch(0),
                    edge_level: None,
                }
            })
            .collect(),
        ..Default::default()
    };
    let bytes = frame.encode();
    let mut pending = RenderFrame::default();
    let result = serde_json::json!({"scope":"CPU-only 512-square checkpoints, four presented fields; fixed synthetic inputs, not a solver benchmark", "kind":kind,"distinct":distinct, "bytes":bytes.len(),
    "validate":measure(||{black_box(raw.as_ref()).validate().unwrap();},40),
    "digest":measure(||{black_box(black_box(raw.as_ref()).digest());},40),
    "recipe_validate":measure(||{black_box(&recipe).validate_checked(black_box(cp.as_ref())).unwrap();},40),
    "frame_clone":measure(||{black_box(frame.clone());},40),
    "frame_encode":measure(||{black_box(frame.encode());},40),
    "frame_decode":measure(||{black_box(RenderFrame::decode(&bytes).unwrap());},40),
    "frame_merge":measure(||{black_box(&mut pending).merge(black_box(frame.clone())).unwrap();},40)});
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
