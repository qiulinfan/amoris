//! The surf beach's shore and break (water.md 61, 63): the swell breaking over the reef and the
//! sand, its swash running up the beach face and back, and the sand it wets. The world runs on its
//! own from the start; at the ticks given the renderer draws the shore from the dunes, from straight
//! over the swash, along the beach low down, from the lineup, from straight over the reef's break
//! and along a breaking wave from its shoulder and from its face. `--every N --from A --to B` draws
//! one view (`--view`, the dunes' by default) every N ticks between A and B instead, a sequence. The
//! captures go to `<out>/<view>-<tick>.png`, under a directory that belongs in ignored `out/`.
//! cargo run --release -p amoris-app --example shore_shots -- --out out/shore --at 1800,2400
//! cargo run --release -p amoris-app --example shore_shots -- --out out/shore --view along \
//!     --every 15 --from 1800 --to 2400
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_render::{Antialiasing, CameraState, Gpu, Gtao, Renderer};
use amoris_runtime::{Extractor, Project};
use glam::Vec3;
use std::sync::Arc;

const W: u32 = 1280;
const H: u32 = 720;

fn main() {
    amoris_render::stderr_log::init();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.windows(2).find(|a| a[0] == name).map(|a| a[1].clone());
    let number = |name: &str, default: u32| arg(name).map_or(default, |v| v.parse().unwrap());
    let out = std::path::PathBuf::from(arg("--out").unwrap_or_else(|| "out/shore".into()));
    let every = number("--every", 0);
    let at: Vec<u32> = if every > 0 {
        (number("--from", 1800)..=number("--to", 2400))
            .step_by(every as usize)
            .collect()
    } else {
        arg("--at")
            .unwrap_or_else(|| "1800".into())
            .split(',')
            .map(|t| t.parse().unwrap())
            .collect()
    };
    let only = arg("--view").or_else(|| (every > 0).then(|| "dunes".to_string()));
    std::fs::create_dir_all(&out).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/surf-beach");
    // The still water meets the sand at x = 236; the beach face rises 1 in 12.5 to the dunes at
    // x = 252. The bay runs along z from 0 to 128.
    let views: [(&str, Vec3, Vec3, Vec3); 7] = [
        (
            "dunes",
            Vec3::new(262.0, 5.0, 64.0),
            Vec3::new(222.0, 0.0, 64.0),
            Vec3::Y,
        ),
        (
            "swash",
            Vec3::new(242.0, 26.0, 64.0),
            Vec3::new(242.0, 0.0, 64.0),
            Vec3::X,
        ),
        (
            "along",
            Vec3::new(244.0, 2.2, 12.0),
            Vec3::new(236.0, 0.3, 80.0),
            Vec3::Y,
        ),
        (
            "lineup",
            Vec3::new(180.0, 3.0, 64.0),
            Vec3::new(250.0, 0.5, 64.0),
            Vec3::Y,
        ),
        // The reef's break from straight above: the reef's edge runs from x = 172 at z = 0 to the
        // beach at z = 85, so each wave breaks at its south end first and peels north.
        (
            "break",
            Vec3::new(200.0, 80.0, 45.0),
            Vec3::new(200.0, 0.0, 45.0),
            Vec3::X,
        ),
        // Low on the shoulder, looking along the breaking wave toward where it first broke.
        (
            "barrel",
            Vec3::new(207.0, 1.4, 44.0),
            Vec3::new(186.0, 0.8, 14.0),
            Vec3::Y,
        ),
        // In front of the face, ahead of where the wave peels at 38 s, looking back along it into
        // the tube.
        (
            "tube",
            Vec3::new(204.0, 0.6, 40.0),
            Vec3::new(195.0, 1.4, 28.0),
            Vec3::Y,
        ),
    ];
    let views: Vec<_> = views
        .into_iter()
        .filter(|v| only.as_deref().is_none_or(|name| name == v.0))
        .collect();
    assert!(!views.is_empty(), "no view {only:?}");
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
            let last = *at.iter().max().unwrap();
            for tick in 1..=last {
                let result = game.step().unwrap();
                assert!(result.errors.is_empty(), "{:?}", result.errors);
                game.present(&mut extractor, &feed);
                let t = f64::from(tick) / 60.0;
                renderer.apply(mailbox.take(), t);
                if !at.contains(&tick) {
                    let _ = renderer.capture_rgba(t);
                    continue;
                }
                for (name, eye, target, up) in &views {
                    renderer.set_camera_override(Some(CameraState::look_at_up(*eye, *target, *up)));
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
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
