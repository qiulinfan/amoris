//! The shallow-water spike (docs/spikes/shallow-water.md), seen headless: a lake 3 m deep on a
//! cliff, a pool under it and a plain around, as blocks. The water is not blocks: it is
//! amoris-physics `shallow` over the volume's beds, drawn by the renderer's height field, and five
//! crates float on it (Rapier, `shallow::Floaters`). At 1.5 s a script cuts a notch through the
//! cliff's edge; the lake pours over it, floods the plain and carries the crates.
//!
//! ```sh
//! cargo run --release -p amoris-app --example shallow_water -- --shots out/shallow \
//!     [--blocks ../amoris-samples/blockcraft] [--frames 600] [--notch N] [--close]
//! ```
//!
//! `--notch N` cuts 2N + 1 blocks wide (1 by default); `--close` looks at the notch's foot from a
//! few metres. Writes a PNG every 15 frames from the cut to 5 s and every 30 after; prints the water
//! and the crates' step times, the water's volume, and the final hashes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use amoris_assets::Feed;
use amoris_assets::frame::{InstanceUpdate, Look, Pose};
use amoris_render::loader::FileAssets;
use amoris_render::shallow_water::ShallowWaterView;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Renderer};
use amoris_runtime::shallow::{Floaters, Shallow};
use amoris_runtime::{Extractor, Game, Project};
use amoris_sim::voxels::{Block, Voxels};
use glam::Vec3;
use serde_json::json;

const SCRIPT: &str = r#"
import { component, game, system } from "amoris";

export const Land = component("Land", { version: 1, doc: "The volume.", fields: {} });

export const notch = system({
    name: "notch", phase: "update", doc: "Cuts the cliff's edge at 1.5 s.",
    queries: {},
    run(ctx) {
        if (ctx.tick !== 90) return;
        ctx.voxels.fill(ctx.single("Land"), [-5, 4, -NOTCH], [-5, 6, NOTCH], 0);
    },
});

export default game({ components: [Land], systems: [notch] });
"#;

/// The volume's x, y, z cell range the water covers.
const LO: [i32; 3] = [-14, -3, -14];
const HI: [i32; 3] = [22, 8, 14];
/// Columns per block edge.
const PER: usize = 2;
/// The lake's cells (x, z) and its surface; the pool's cells and its surface.
const LAKE: ([i32; 2], [i32; 2], f64) = ([-11, -4], [-6, 4], 7.0);
const POOL: ([i32; 2], [i32; 2], f64) = ([-4, -4], [0, 4], 0.0);

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// The blocks: stone ground with a grass top and a pool dug in it, a cliff with a basin 3 blocks
/// deep in its top behind a rim. No liquid blocks: the water is the height field's.
fn land(blocks: bool) -> Voxels {
    let face = |name: &str| {
        if blocks {
            format!("blocks.gltf#{name}")
        } else {
            format!("b#{name}")
        }
    };
    let mut land = Voxels::default();
    let cube = |name: &str, faces: [&str; 3]| Block {
        name: name.into(),
        solid: true,
        faces: faces.map(&face),
        ..Default::default()
    };
    land.palette.push(cube("stone", ["stone"; 3]));
    land.palette
        .push(cube("grass", ["grass_top", "grass_side", "dirt"]));
    for (lo, hi, block) in [
        ([-14, -3, -14], [22, -2, 14], 1),
        ([-14, -1, -14], [22, -1, 14], 2),
        ([-4, -1, -4], [0, -1, 4], 0),
        ([-14, 0, -14], [-5, 5, 14], 1),
        ([-12, 6, -5], [-5, 6, 5], 1),
        ([-11, 4, -4], [-6, 6, 4], 0),
    ] {
        land.fill(lo, hi, block, 0).expect("a fill");
    }
    land
}

fn project(blocks: Option<&Path>, notch: i32) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-shallow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).expect("the project directory");
    std::fs::write(
        root.join("project.toml"),
        "name = 'shallow'\nrate = 60\nseed = 1\n",
    )
    .expect("project.toml");
    std::fs::write(
        root.join("scripts/main.ts"),
        SCRIPT.replace("NOTCH", &notch.to_string()),
    )
    .expect("the script");
    if let Some(dir) = blocks {
        std::fs::copy(dir.join("blocks.gltf"), root.join("blocks.gltf")).expect("blocks.gltf");
        std::fs::create_dir_all(root.join("textures")).expect("textures");
        for e in std::fs::read_dir(dir.join("textures"))
            .expect("textures")
            .flatten()
        {
            std::fs::copy(e.path(), root.join("textures").join(e.file_name())).expect("a texture");
        }
    }
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": [
        {"name": "Land", "components": {"Land": {}, "Voxels": land(blocks.is_some())}}
    ]});
    std::fs::write(root.join("scene.json"), scene.to_string()).expect("scene.json");
    root
}

/// Fills the columns over cells `lo..=hi` (x, z) up to `surface`.
fn fill(w: &mut Shallow, lo: [i32; 2], hi: [i32; 2], surface: f64) {
    for k in 0..w.nz {
        for i in 0..w.nx {
            let x = w.origin[0] + (i as f64 + 0.5) * w.cell_m;
            let z = w.origin[1] + (k as f64 + 0.5) * w.cell_m;
            let inside = x >= f64::from(lo[0])
                && x < f64::from(hi[0] + 1)
                && z >= f64::from(lo[1])
                && z < f64::from(hi[1] + 1);
            if inside {
                let at = i + w.nx * k;
                w.depth[at] = (surface - w.bed[at]).max(0.0);
            }
        }
    }
}

fn view(w: &Shallow) -> ShallowWaterView {
    ShallowWaterView {
        origin: [w.origin[0] as f32, w.origin[1] as f32],
        cell_m: w.cell_m as f32,
        nx: w.nx as u32,
        nz: w.nz as u32,
        columns: (0..w.depth.len())
            .map(|at| {
                let v = w.velocity(at);
                [
                    w.surface(at) as f32,
                    w.depth[at] as f32,
                    v[0] as f32,
                    v[1] as f32,
                ]
            })
            .collect(),
    }
}

fn main() {
    amoris_render::stderr_log::init();
    let args: Vec<String> = std::env::args().collect();
    let dir = value(&args, "--shots").unwrap_or_else(|| "out/shallow".into());
    let frames: u64 = value(&args, "--frames")
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let blocks = value(&args, "--blocks").map(PathBuf::from);
    let notch: i32 = value(&args, "--notch")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .clamp(0, 4);
    let root = project(blocks.as_deref(), notch);
    let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let mut game = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
    let feed = Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::new();
    game.present(&mut extractor, &feed);

    // The water and the crates, over a copy of the blocks the script changes alike.
    let mut blocks_copy = land(blocks.is_some());
    let mut water = Shallow::from_voxels(&blocks_copy, [0.0; 3], LO, HI, PER);
    fill(&mut water, LAKE.0, LAKE.1, LAKE.2);
    fill(&mut water, POOL.0, POOL.1, POOL.2);
    let volume0 = water.volume();
    let floor = f64::from(LO[1]);
    let mut crates = Floaters::new(1.0 / 60.0);
    crates.set_terrain(&water, floor);
    for (i, z) in [-2.5, -1.2, 0.0, 1.3, 2.4].into_iter().enumerate() {
        crates.add_box([-9.0 + 0.7 * (i % 2) as f64, 7.1, z], 0.35, 450.0);
    }

    let gpu = Gpu::headless(BackendChoice::from_env()).expect("a GPU");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1280, 720);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_asset_source(Box::new(FileAssets::new(root.clone())));
    let (eye, at) = if args.iter().any(|a| a == "--close") {
        (Vec3::new(2.0, 3.5, 7.0), Vec3::new(-4.5, 1.5, 0.0))
    } else {
        (Vec3::new(14.0, 11.0, 17.0), Vec3::new(-3.0, 2.0, 0.0))
    };
    r.set_camera_override(Some(CameraState::look_at(eye, at)));
    std::fs::create_dir_all(&dir).expect("the output directory");
    let (mut water_ms, mut crates_ms) = (Vec::new(), Vec::new());
    for frame in 0..frames {
        game.step().unwrap_or_else(|e| panic!("{e:#?}"));
        game.present(&mut extractor, &feed);
        let mut f = mailbox.take();
        if frame == 90 {
            blocks_copy
                .fill([-5, 4, -notch], [-5, 6, notch], 0, 0)
                .expect("the notch");
            water.rebed(&blocks_copy, [0.0; 3], LO, HI, PER);
            crates.set_terrain(&water, floor);
        }
        let t = Instant::now();
        water.step(1.0 / 60.0, 4);
        water_ms.push(t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        crates.step(&water);
        crates_ms.push(t.elapsed().as_secs_f64() * 1e3);
        if frame == 0 {
            f.environment = Some(amoris_assets::frame::EnvironmentView {
                sky: 0,
                sky_color: [0.35, 0.5, 0.75],
                ambient: 1.0,
                baked_gi: String::new(),
                neural_gi: String::new(),
                gi_intensity: 1.0,
                fog_density: 0.0,
                fog_color: [0.6, 0.7, 0.8],
                exposure_ev: 0.5,
                bloom: 0.05,
            });
            f.lights = Some(vec![amoris_assets::frame::LightView {
                id: 1 << 40,
                kind: amoris_assets::frame::LightKindView::Directional,
                position: [0.0; 3],
                direction: Vec3::new(-0.35, -1.0, -0.45).normalize().to_array(),
                color: [1.0, 0.96, 0.9],
                intensity: 8.0,
                range: 0.0,
                inner_deg: 0.0,
                outer_deg: 0.0,
                shadows: true,
            }]);
        }
        for (i, (half, p, q)) in crates.poses().into_iter().enumerate() {
            let edge = (2.0 * half) as f32;
            f.instances.push(InstanceUpdate {
                id: (1 << 41) + i as u64,
                pose: Some(Pose {
                    position: p.map(|c| c as f32),
                    rotation: q.map(|c| c as f32),
                    scale: [edge; 3],
                }),
                look: (frame == 0).then(|| Look {
                    mesh: "cube".into(),
                    material: String::new(),
                    color: [0.55, 0.36, 0.2, 1.0],
                    metallic: 0.0,
                    roughness: 0.8,
                    transmission: None,
                    ior: None,
                    emissive: [0.0; 3],
                    cast_shadows: true,
                    visible: true,
                }),
                anim: None,
            });
        }
        r.set_shallow_water(Some(view(&water)));
        let now = frame as f64 / 60.0;
        r.apply(f, now);
        let (w, h, rgba) = r.capture_rgba(now);
        let shot = if (90..300).contains(&frame) {
            frame % 15 == 0
        } else {
            frame % 30 == 0
        };
        if shot {
            let path = format!("{dir}/frame-{frame:03}.png");
            image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("a PNG");
        }
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let poses: Vec<String> = crates
        .poses()
        .iter()
        .map(|(_, p, _)| format!("[{:.2}, {:.2}, {:.2}]", p[0], p[1], p[2]))
        .collect();
    println!(
        "{} x {} columns; water {:.3} ms a frame (4 substeps), crates {:.3} ms; volume {:.3} m3 \
         then {:.3}; water hash {:016x}; crates at {}",
        water.nx,
        water.nz,
        median(&mut water_ms),
        median(&mut crates_ms),
        volume0,
        water.volume(),
        water.hash(),
        poses.join(" ")
    );
    let _ = std::fs::remove_dir_all(root);
}
