//! The open sea and its seam with a water region (water.md 60), in the surf beach sample played
//! through its player: the rider lies on the board and paddles toward the beach while the swell
//! comes in. At the ticks given, the renderer draws the player's eye and five fixed views of the
//! bay's seaward side, where the region's water meets the sea drawn to the horizon: from the
//! lineup looking out to sea, from inside the bay's mouth, from high over it, down onto the seam,
//! and from the open sea looking in. The captures go to `<out>/<view>-<tick>.png`, under a
//! directory that belongs in ignored `out/`.
//! cargo run --release -p amoris-app --example ocean_shots -- --out out/ocean --at 1800,2400
//! `--view <name>` draws every frame from that view, not only the captures: with
//! `AMORIS_TRACE=frames=300,skip=1500,out=PATH`, a frame trace for `tools/frame_trace.py`.
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_render::{Antialiasing, CameraState, Gpu, Gtao, Renderer};
use amoris_runtime::{Command, Extractor, Game, Project};
use glam::Vec3;
use serde_json::{Value, json};
use std::sync::Arc;

const W: u32 = 1280;
const H: u32 = 720;

fn fields(g: &mut Game, entity: &str, components: &[&str]) -> Value {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity": entity, "fields": components}),
    ))
    .map_or(Value::Null, |v| v["fields"].clone())
}
/// The yaw a heading has, Walker style: 0 along -z, positive to the left.
fn yaw_of(x: f64, z: f64) -> f64 {
    (-x).atan2(-z)
}
fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}
/// The direction a body's nose (-z in its frame) points along the water.
fn nose(q: &Value) -> [f64; 2] {
    let [qx, qy, qz, qw] = [0, 1, 2, 3].map(|i| q[i].as_f64().unwrap());
    [
        -2.0 * (qx * qz + qw * qy),
        -(1.0 - 2.0 * (qx * qx + qy * qy)),
    ]
}

/// Lie on the board and paddle toward the beach, steering along +x.
fn paddle(g: &mut Game, sequence: &mut u64) {
    let board = fields(g, "Board", &["Transform"]);
    let n = nose(&board["Transform"]["rotation"]);
    let error = wrap(yaw_of(1.0, 0.0) - yaw_of(n[0], n[1]));
    *sequence += 1;
    g.apply(&Command::new(
        Source::Player(0),
        *sequence,
        "player.act",
        json!({ "actions": [{"do":"set","controls":{"forward":1.0,"strafe":(-2.0 * error).clamp(-1.0, 1.0)}}] }),
    ))
    .unwrap();
}

fn main() {
    amoris_render::stderr_log::init();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.windows(2).find(|a| a[0] == name).map(|a| a[1].clone());
    let out = std::path::PathBuf::from(arg("--out").unwrap_or_else(|| "out/ocean".into()));
    let at: Vec<u32> = arg("--at")
        .unwrap_or_else(|| "1800".into())
        .split(',')
        .map(|t| t.parse().unwrap())
        .collect();
    let held = arg("--view");
    std::fs::create_dir_all(&out).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/surf-beach");
    // The bay's seaward side is x = 0, from z = 0 to 128; the open sea lies at x < 0.
    let views: [(&str, Option<(Vec3, Vec3)>); 6] = [
        ("eye", None),
        (
            "lineup",
            Some((Vec3::new(160.0, 1.6, 20.0), Vec3::new(0.0, 0.0, 64.0))),
        ),
        (
            "mouth",
            Some((Vec3::new(40.0, 4.0, 64.0), Vec3::new(-200.0, 0.0, 64.0))),
        ),
        (
            "high",
            Some((Vec3::new(120.0, 60.0, 64.0), Vec3::new(-80.0, 0.0, 64.0))),
        ),
        (
            "seam",
            Some((Vec3::new(14.0, 7.0, 40.0), Vec3::new(-8.0, 0.0, 62.0))),
        ),
        (
            "outside",
            Some((Vec3::new(-60.0, 6.0, 64.0), Vec3::new(100.0, 0.0, 64.0))),
        ),
    ];
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(move || {
            let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
            let mut game = water_compute::builder(setup, 1).build().unwrap();
            let feed = amoris_assets::Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = Extractor::default();
            let gpu = Gpu::headless(amoris_render::host_backend()).unwrap();
            let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
            renderer.set_asset_source(Box::new(amoris_render::loader::FileAssets::new(
                root.clone(),
            )));
            renderer.set_antialiasing(Antialiasing::Msaa);
            renderer.set_gtao(Gtao::Off);
            let held = held.map(|name| {
                views
                    .iter()
                    .find(|(n, _)| *n == name)
                    .unwrap_or_else(|| panic!("no view {name}"))
                    .1
            });
            let mut sequence = 0;
            let last = *at.iter().max().unwrap();
            for tick in 1..=last {
                paddle(&mut game, &mut sequence);
                let result = game.step().unwrap();
                assert!(result.errors.is_empty(), "{:?}", result.errors);
                game.present(&mut extractor, &feed);
                let t = f64::from(tick) / 60.0;
                renderer.apply(mailbox.take(), t);
                if !at.contains(&tick) {
                    if let Some(camera) = held {
                        renderer.set_camera_override(
                            camera.map(|(eye, at)| CameraState::look_at(eye, at)),
                        );
                    }
                    let _ = renderer.capture_rgba(t);
                    continue;
                }
                for (name, camera) in &views {
                    renderer
                        .set_camera_override(camera.map(|(eye, at)| CameraState::look_at(eye, at)));
                    let (w, h, pixels) = renderer.capture_rgba(t);
                    image::save_buffer(
                        out.join(format!("{name}-{tick:04}.png")),
                        &pixels,
                        w,
                        h,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
                renderer.set_camera_override(None);
                println!("tick {tick}: {} views", views.len());
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
