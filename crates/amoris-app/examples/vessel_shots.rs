//! The pool party's dinghies (water.md 62): two 60 kg boats afloat by their hull alone, one filling
//! through a hole in its bottom until it sinks. The game runs on its own from the start; at the
//! ticks given the renderer draws the sound dinghy from the shallow end's deck, the leaky one from
//! the deep end's and the pool from above, and the leaky dinghy's height, water and heel print
//! every second. The captures go to `<out>/<view>-<tick>.png`, under a directory that belongs in
//! ignored `out/`.
//! cargo run --release -p amoris-app --example vessel_shots -- --out out/vessels --at 60,3600,7200
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use amoris_runtime::{Command, Extractor, Game, Project};
use glam::Vec3;
use serde_json::{Value, json};
use std::sync::Arc;

const W: u32 = 1280;
const H: u32 = 720;

fn fields(g: &mut Game, entity: &str) -> Value {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity": entity, "fields": ["Transform", "Vessel"]}),
    ))
    .map_or(Value::Null, |v| v["fields"].clone())
}

fn main() {
    amoris_render::stderr_log::init();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.windows(2).find(|a| a[0] == name).map(|a| a[1].clone());
    let out = std::path::PathBuf::from(arg("--out").unwrap_or_else(|| "out/vessels".into()));
    let at: Vec<u32> = arg("--at")
        .unwrap_or_else(|| "60,3600,7200".into())
        .split(',')
        .map(|t| t.parse().unwrap())
        .collect();
    std::fs::create_dir_all(&out).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/pool-party");
    // The pool runs along x from 0 to 16 m and z from 0 to 8, its water at 0, the deck at 0.35.
    let views: [(&str, Vec3, Vec3); 3] = [
        (
            "dinghy",
            Vec3::new(-0.6, 1.9, 9.6),
            Vec3::new(2.6, 0.0, 6.9),
        ),
        (
            "leaky",
            Vec3::new(15.8, 1.7, -1.6),
            Vec3::new(12.6, -0.2, 1.1),
        ),
        ("pool", Vec3::new(8.0, 10.0, -5.0), Vec3::new(8.0, 0.0, 4.0)),
    ];
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(move || {
            let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
            let mut game = water_compute::builder(setup, 1).build().unwrap();
            let feed = amoris_assets::Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = Extractor::default();
            let gpu = Gpu::headless(BackendChoice::from_env()).unwrap();
            let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
            renderer.set_asset_source(Box::new(amoris_render::loader::FileAssets::new(
                root.clone(),
            )));
            renderer.set_antialiasing(Antialiasing::Msaa);
            renderer.set_gtao(Gtao::Off);
            let last = *at.iter().max().unwrap();
            for tick in 1..=last {
                let result = game.step().unwrap();
                assert!(result.errors.is_empty(), "{:?}", result.errors);
                if tick % 60 == 0 {
                    let leaky = fields(&mut game, "LeakyDinghy");
                    let q = &leaky["Transform"]["rotation"];
                    let [qx, qy, qz, qw] = [0, 1, 2, 3].map(|i| q[i].as_f64().unwrap());
                    // The heel: how far the boat's up axis leans from the vertical, degrees.
                    let up_y = 1.0 - 2.0 * (qx * qx + qz * qz);
                    let _ = qy * qw;
                    println!(
                        "{:>4} s: leaky dinghy at {:.3} m, {:.3} m^3 inside, {:.2} under, heeled \
                         {:.1} deg",
                        tick / 60,
                        leaky["Transform"]["position"][1].as_f64().unwrap(),
                        leaky["Vessel"]["flooded"].as_f64().unwrap(),
                        leaky["Vessel"]["submerged"].as_f64().unwrap(),
                        up_y.clamp(-1.0, 1.0).acos().to_degrees()
                    );
                }
                game.present(&mut extractor, &feed);
                let t = f64::from(tick) / 60.0;
                renderer.apply(mailbox.take(), t);
                if !at.contains(&tick) {
                    let _ = renderer.capture_rgba(t);
                    continue;
                }
                for (name, eye, target) in &views {
                    renderer.set_camera_override(Some(CameraState::look_at(*eye, *target)));
                    let (w, h, pixels) = renderer.capture_rgba(t);
                    image::save_buffer(
                        out.join(format!("{name}-{tick:05}.png")),
                        &pixels,
                        w,
                        h,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
                renderer.set_camera_override(None);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
