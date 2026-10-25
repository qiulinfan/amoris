//! Draws a project from a camera at given ticks while its world runs on its own from the start:
//! headless captures of any scene for checking how its water looks. `--eye x,y,z --at x,y,z`
//! place the camera, `--ticks a,b,c` (or `--every n --from a --to b`) choose the frames, and the
//! captures go to `<out>/<name>-<tick>.png` under a directory that belongs in ignored `out/`.
//! MSAA unless `AMORIS_AA` says otherwise.
//! cargo run --release -p amoris-app --example scene_shots -- samples/river-falls --out out/falls \
//!     --eye 60,12,40 --at 80,4,40 --ticks 600,1200
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use amoris_runtime::{Extractor, Project};
use glam::Vec3;
use std::sync::Arc;

const W: u32 = 1280;
const H: u32 = 720;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.windows(2).find(|a| a[0] == name).map(|a| a[1].clone());
    let number = |name: &str, default: u32| arg(name).map_or(default, |v| v.parse().unwrap());
    let point = |name: &str| {
        let v: Vec<f32> = arg(name)
            .unwrap_or_else(|| panic!("{name} x,y,z"))
            .split(',')
            .map(|c| c.parse().unwrap())
            .collect();
        Vec3::new(v[0], v[1], v[2])
    };
    let project = std::path::PathBuf::from(&args[1]);
    let out = std::path::PathBuf::from(arg("--out").unwrap_or_else(|| "out/scene".into()));
    let name = arg("--name").unwrap_or_else(|| "view".into());
    let (eye, target) = (point("--eye"), point("--at"));
    let every = number("--every", 0);
    let ticks: Vec<u32> = if every > 0 {
        (number("--from", 60)..=number("--to", 600))
            .step_by(every as usize)
            .collect()
    } else {
        arg("--ticks")
            .unwrap_or_else(|| "600".into())
            .split(',')
            .map(|t| t.parse().unwrap())
            .collect()
    };
    std::fs::create_dir_all(&out).unwrap();
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(move || {
            let setup = Arc::new(Project::load(&project).unwrap().setup(false).unwrap());
            let mut game = water_compute::builder(setup, 1).build().unwrap();
            let feed = amoris_assets::Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = Extractor::default();
            let gpu = Gpu::headless(BackendChoice::from_env()).unwrap();
            let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
            renderer.set_asset_source(Box::new(amoris_render::loader::FileAssets::new(
                project.clone(),
            )));
            renderer.set_antialiasing(Antialiasing::from_env(Antialiasing::Msaa));
            renderer.set_gtao(Gtao::Off);
            renderer.set_camera_override(Some(CameraState::look_at(eye, target)));
            let last = *ticks.iter().max().unwrap();
            for tick in 1..=last {
                let result = game.step().unwrap();
                assert!(result.errors.is_empty(), "{:?}", result.errors);
                game.present(&mut extractor, &feed);
                let t = f64::from(tick) / 60.0;
                renderer.apply(mailbox.take(), t);
                let (w, h, pixels) = renderer.capture_rgba(t);
                if ticks.contains(&tick) {
                    image::save_buffer(
                        out.join(format!("{name}-{tick:05}.png")),
                        &pixels,
                        w,
                        h,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
