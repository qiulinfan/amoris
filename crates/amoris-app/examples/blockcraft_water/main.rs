//! Blockcraft's water headless (docs/spec/water.md, slice A): the game digs a pit beside the water
//! and breaks the bank at 2 s ([`breach`]), the field water floods the pit, and the render feed
//! drives two renderers frame by frame, one drawing the water Smooth and one Blocky (the feed's
//! look patched), as a host would. Writes close and far shots of each into `--shots`, and with
//! `--bench` the drawing's cost: the transparent pass's GPU time with the water and without it.
//!
//! ```sh
//! cargo run --release -p amoris-app --example blockcraft_water -- \
//!     [--project ../amoris-samples/blockcraft] [--shots out/blockcraft-water] [--frames 1200] \
//!     [--bench]
//! ```

#![allow(clippy::disallowed_methods)] // a host reads the wall clock, outside any tick

#[path = "breach.rs"]
mod breach;

use std::path::PathBuf;
use std::sync::Arc;

use amoris_assets::Feed;
use amoris_assets::frame::RenderFrame;
use amoris_render::loader::FileAssets;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Renderer};
use amoris_runtime::{Extractor, Game, Project};
use glam::Vec3;

const W: u32 = 1280;
const H: u32 = 720;

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// The frame as the Blocky renderer receives it: every water look made blocky.
fn blocky(mut f: RenderFrame) -> RenderFrame {
    for w in &mut f.water {
        if let Some(l) = &mut w.look {
            l.blocky = true;
        }
    }
    f
}

/// The frame without its water, for the renderer the water's cost is measured against.
fn dry(mut f: RenderFrame) -> RenderFrame {
    f.water.clear();
    f.water_removed.clear();
    f
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

/// GPU time of the renderers' frames at `now` (each renders in turn, so they share the GPU's
/// state): per renderer, the medians over `n` frames (after 60) of the opaque pass, where the water
/// is drawn after the opaque geometry when no transparent instance is drawn (`opaque` scopes), and
/// of the frame's busy time (every pass).
fn gpu_cost(
    rs: &mut [&mut Renderer],
    view: &wgpu::TextureView,
    now: f64,
    n: usize,
) -> Vec<[f64; 2]> {
    let mut samples = vec![(Vec::new(), Vec::new()); rs.len()];
    for i in 0..n + 60 {
        for (r, (opaque, busy)) in rs.iter_mut().zip(&mut samples) {
            let s = r.render(view, now);
            if i < 60 || s.passes.is_empty() {
                continue;
            }
            let o: f32 = s
                .passes
                .iter()
                .filter(|(name, _)| name.starts_with("opaque") || *name == "transparent")
                .map(|(_, ms)| ms)
                .sum();
            opaque.push(f64::from(o));
            busy.push(f64::from(s.passes.iter().map(|(_, ms)| ms).sum::<f32>()));
        }
    }
    samples
        .into_iter()
        .map(|(mut o, mut b)| [median(&mut o), median(&mut b)])
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let src = PathBuf::from(
        value(&args, "--project").unwrap_or_else(|| "../amoris-samples/blockcraft".into()),
    );
    let dir =
        PathBuf::from(value(&args, "--shots").unwrap_or_else(|| "out/blockcraft-water".into()));
    let frames: u64 = value(&args, "--frames")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1200);
    let bench = args.iter().any(|a| a == "--bench");
    std::fs::create_dir_all(&dir).expect("the output directory");

    let (root, site) = breach::prepare(&src);
    println!(
        "breach at cells {:?}, world {:?}; dig at tick {}, break at tick {}",
        site.cell,
        site.world,
        breach::DIG,
        breach::BREAK
    );
    let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let mut game = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
    let feed = Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::new();

    let gpu = Gpu::headless(BackendChoice::from_env()).expect("a GPU");
    let make = || {
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
        r.set_antialiasing(Antialiasing::Msaa);
        r.set_asset_source(Box::new(FileAssets::new(root.clone())));
        r
    };
    let (mut smooth, mut block, mut none) = (make(), make(), make());
    let b = Vec3::new(
        site.world[0] as f32,
        site.world[1] as f32,
        site.world[2] as f32,
    );
    // The shore runs along z and the pit lies to +x of the bank. Far: from high over the water,
    // the shore, the breach and the pit; close: from over the pit, the fall through the breach.
    let cameras = [
        (
            "far",
            b + Vec3::new(-14.0, 20.0, 14.0),
            b + Vec3::new(5.0, -3.0, 0.0),
        ),
        (
            "close",
            b + Vec3::new(7.5, 6.0, 0.5),
            b + Vec3::new(0.0, -3.5, 0.5),
        ),
    ];
    let shots: Vec<u64> = [-10i64, 15, 30, 60, 120, 240, 480, 900]
        .iter()
        .map(|d| (breach::BREAK as i64 + d) as u64)
        .collect();
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut costs = Vec::new();
    // The water's columns as the feed delivered them, by tile key: what the shots' log reads.
    let mut columns: std::collections::BTreeMap<[u32; 2], Vec<[f32; 4]>> = Default::default();
    for _ in 0..frames {
        game.step().unwrap_or_else(|e| panic!("{e:#?}"));
        game.present(&mut extractor, &feed);
        let mut f = mailbox.take();
        // The shots show the world, not the game's HUD.
        if let Some(ui) = &mut f.ui {
            ui.clear();
        }
        for w in &f.water {
            for t in &w.tiles {
                columns.insert(t.key, t.columns.clone());
            }
        }
        let tick = game.tick().0;
        let now = tick as f64 / 60.0;
        smooth.apply(f.clone(), now);
        block.apply(blocky(f.clone()), now);
        if bench {
            none.apply(dry(f), now);
        }
        if shots.contains(&tick) {
            // The pit's water: its columns (two a block) over cells x + 2 ..= x + 9, z ..= z + 5.
            let column = |i: u32, k: u32| {
                columns
                    .get(&[i / 16, k / 16])
                    .map_or([0.0; 4], |t| t[(i % 16 + 16 * (k % 16)) as usize])
            };
            let [cx, cz] = site.cell.map(|c| u32::try_from(c).unwrap_or(0));
            let (mut m3, mut deepest) = (0.0f64, 0.0f32);
            for k in 2 * cz..2 * (cz + 6) {
                for i in 2 * (cx + 2)..2 * (cx + 10) {
                    let c = column(i, k);
                    m3 += f64::from(c[1]) * 0.25;
                    deepest = deepest.max(c[1]);
                }
            }
            let gap = column(2 * (cx + 1), 2 * (cz + 2) + 1);
            println!(
                "tick {tick:4}: the pit holds {m3:7.2} m3 (deepest {deepest:.2} m); the breach's \
                 column: surface {:.3}, depth {:.3}, velocity ({:.2}, {:.2})",
                gap[0], gap[1], gap[2], gap[3]
            );
            for (name, eye, at) in &cameras {
                for (style, r) in [("smooth", &mut smooth), ("blocky", &mut block)] {
                    r.set_camera_override(Some(CameraState::look_at(*eye, *at)));
                    // Settle the temporal passes on the still camera before the shot.
                    for _ in 0..8 {
                        let _ = r.render(&view, now);
                    }
                    let (w, h, rgba) = r.capture_rgba(now);
                    let path = dir.join(format!(
                        "{style}-{name}-{:+04}.png",
                        tick as i64 - breach::BREAK as i64
                    ));
                    image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("a PNG");
                }
            }
            if bench {
                // The far camera: the region fills most of the view.
                let (eye, at) = (cameras[0].1, cameras[0].2);
                for r in [&mut none, &mut smooth, &mut block] {
                    r.set_camera_override(Some(CameraState::look_at(eye, at)));
                }
                let c = gpu_cost(&mut [&mut none, &mut smooth, &mut block], &view, now, 240);
                let mut row = vec![tick as f64 - breach::BREAK as f64];
                for x in c {
                    row.extend(x);
                }
                costs.push(row);
            }
        }
    }
    if bench {
        println!(
            "ticks after the break | opaque pass and GPU busy (ms), medians of 240 frames: \
             without water | Smooth | Blocky"
        );
        for row in costs {
            println!(
                "{:+5} | {:.3} {:.3} | {:.3} {:.3} | {:.3} {:.3}",
                row[0], row[1], row[2], row[3], row[4], row[5], row[6]
            );
        }
    }
    println!("shots in {}", dir.display());
    let _ = std::fs::remove_dir_all(root);
}
