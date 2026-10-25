//! Splashes over block liquids (docs/spec/splashes.md), seen headless: a lake on a cliff whose
//! rim a script breaks at 1.5 s, so `physics.fluids` pours it over the edge into a pool below. The
//! game runs for real and its render feed drives the renderer, frame by frame, as a host would.
//!
//! ```sh
//! cargo run --release -p amoris-app --example splashes -- --shots out/splashes \
//!     [--blocks samples/blockcraft] [--frames 600] [--off] [--close]
//! ```
//!
//! `--blocks DIR` takes `blocks.gltf` and `textures/` from DIR (blockcraft's) for the blocks'
//! looks; without it the blocks are plain. Writes a PNG every 15 frames from the break to 5 s and
//! every 30 after, and prints the median frame time before the break and while the water falls.
//! `--off` turns splashes off (the same frames without them); `--breach N` breaks 2N + 1 cells of
//! the rim (1 by default, at most 4); `--close` looks at the stream's foot from a few metres. It
//! also prints the most particles alive at once (read back every 30 frames, outside the timing).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amoris_assets::Feed;
use amoris_render::loader::FileAssets;
use amoris_render::{Antialiasing, CameraState, Gpu, Renderer};
use amoris_runtime::{Extractor, Game, Project};
use glam::Vec3;
use serde_json::json;

const SCRIPT: &str = r#"
import { component, game, system } from "amoris";

export const Land = component("Land", { version: 1, doc: "The volume.", fields: {} });

export const dam = system({
    name: "dam", phase: "update", doc: "Breaks the lake's rim at 1.5 s.",
    queries: {},
    run(ctx) {
        if (ctx.tick !== 90) return;
        ctx.voxels.fill(ctx.single("Land"), [-5, 6, -BREACH], [-5, 6, BREACH], 0);
    },
});

export default game({ components: [Land], systems: [dam] });
"#;

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// The project: stone ground with a pool, a cliff with a lake on it held by a rim.
fn project(blocks: Option<&Path>, breach: i32) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-splashes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).expect("the project directory");
    std::fs::write(
        root.join("project.toml"),
        "name = 'splashes'\nrate = 60\nseed = 1\n",
    )
    .expect("project.toml");
    std::fs::write(
        root.join("scripts/main.ts"),
        SCRIPT.replace("BREACH", &breach.to_string()),
    )
    .expect("the script");
    let face = |name: &str| {
        if blocks.is_some() {
            format!("blocks.gltf#{name}")
        } else {
            format!("b#{name}")
        }
    };
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
    use amoris_sim::voxels::{Block, BlockShape, Flow, Voxels};
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
    land.palette.push(Block {
        name: "water".into(),
        shape: BlockShape::Liquid,
        faces: [face("water"), String::new(), String::new()],
        flow: Some(Flow {
            fog: [0.02, 0.09, 0.32, 0.11],
            splash: Some([6.0, 1.8, 0.9]),
            ..Flow::default()
        }),
        ..Default::default()
    });
    for (lo, hi, block) in [
        // Ground, a pool dug in it under the cliff's edge.
        ([-14, -2, -10], [14, -2, 10], 1),
        ([-14, -1, -10], [14, -1, 10], 2),
        ([-4, -1, -4], [0, -1, 4], 3),
        // The cliff, the lake on it and its rim.
        ([-14, 0, -10], [-5, 5, 10], 1),
        ([-12, 6, -5], [-5, 6, 5], 1),
        ([-11, 6, -4], [-6, 6, 4], 3),
    ] {
        land.fill(lo, hi, block, 0).expect("a fill");
    }
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": [
        {"name": "Land", "components": {"Land": {}, "Voxels": land}}
    ]});
    std::fs::write(root.join("scene.json"), scene.to_string()).expect("scene.json");
    root
}

fn main() {
    amoris_render::stderr_log::init();
    let args: Vec<String> = std::env::args().collect();
    let dir = value(&args, "--shots").unwrap_or_else(|| "out/splashes".into());
    let frames: u64 = value(&args, "--frames")
        .and_then(|s| s.parse().ok())
        .unwrap_or(600);
    let blocks = value(&args, "--blocks").map(PathBuf::from);
    let breach: i32 = value(&args, "--breach")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1)
        .clamp(0, 4);
    let root = project(blocks.as_deref(), breach);
    let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let mut game = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
    let feed = Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::new();
    game.present(&mut extractor, &feed);

    let gpu = Gpu::headless(amoris_render::host_backend()).expect("a GPU");
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1280, 720);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_asset_source(Box::new(FileAssets::new(root.clone())));
    r.set_splashes(!args.iter().any(|a| a == "--off"));
    // `--close`: the stream's foot from a few metres.
    let (eye, at) = if args.iter().any(|a| a == "--close") {
        (Vec3::new(1.5, 2.5, 5.0), Vec3::new(-3.5, 0.5, 0.0))
    } else {
        (Vec3::new(9.0, 8.0, 13.0), Vec3::new(-4.0, 2.0, 0.0))
    };
    r.set_camera_override(Some(CameraState::look_at(eye, at)));
    std::fs::create_dir_all(&dir).expect("the output directory");
    let (mut calm, mut falling) = (Vec::new(), Vec::new());
    let mut most = 0;
    for frame in 0..frames {
        game.step().unwrap_or_else(|e| panic!("{e:#?}"));
        game.present(&mut extractor, &feed);
        let mut f = mailbox.take();
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
        let now = frame as f64 / 60.0;
        r.apply(f, now);
        let t = std::time::Instant::now();
        let (w, h, rgba) = r.capture_rgba(now);
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if (30..90).contains(&frame) {
            calm.push(ms);
        } else if (150..300).contains(&frame) {
            falling.push(ms);
        }
        let shot = if (90..300).contains(&frame) {
            frame % 15 == 0
        } else {
            frame % 30 == 0
        };
        if frame % 30 == 0 {
            most = most.max(living(&gpu, &r));
        }
        if shot {
            let path = format!("{dir}/frame-{frame:03}.png");
            image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).expect("a PNG");
        }
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    println!(
        "frame with readback, median: {:.2} ms before the break, {:.2} ms while it falls; \
         at most {most} particles",
        median(&mut calm),
        median(&mut falling),
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Particles not marked dead in the splashes' ring (read back from the GPU).
fn living(gpu: &Gpu, r: &Renderer) -> usize {
    let Some(f) = r.splash_fluid() else {
        return 0;
    };
    let src = f.particle_buffer();
    let read = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particles readback"),
        size: src.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(src, 0, &read, 0, src.size());
    gpu.queue.submit([enc.finish()]);
    let slice = read.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv()
        .expect("a mapping answer")
        .expect("the buffer maps");
    let bytes = slice.get_mapped_range().expect("mapped").to_vec();
    read.unmap();
    // 80 bytes a particle; its tag is the eighth word (u32::MAX: dead; 0: never spawned).
    bytes
        .chunks(80)
        .map(|p| u32::from_le_bytes([p[28], p[29], p[30], p[31]]))
        .filter(|&tag| tag != u32::MAX && tag != 0)
        .count()
}
