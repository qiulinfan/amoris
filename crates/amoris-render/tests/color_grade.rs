//! The environment's color-grading table (docs/spec/color-grading.md 6): drawn into an 8-bit
//! output, with the surface encoding to sRGB and without (the browser's canvas), a mixed scene under
//! the atmosphere's sky is graded by tables handed in through the asset source, as the environment
//! names them, and each image is compared with the ungraded one:
//!
//! - no table, a table at share 0, and a table cleared again: the ungraded frame, to the bit;
//! - the identity (33 and 17 entries a side): at most one 8-bit step off, on at most 0.1% of the
//!   channels (the table's trilinear read rounds within 1e-6 of its input; on the RTX 5060 and the
//!   Radeon 780M, D3D12 and Vulkan, none differs);
//! - a channel swap (r, g, b) to (g, b, r): the ungraded frame's channels swapped, within one step
//!   (and more than half the channels changed, so the scene's colours were graded at all);
//! - a sepia matrix: the CPU's `ColorLut::sample` of the ungraded frame, within two steps (the
//!   dither and the 8-bit input are amplified by the matrix's rows);
//! - the inversion at share 0.5: mid grey everywhere, within one step and a half;
//! - a table that fails to load: the ungraded frame, and the error reported.
//!
//! Multisampled without TAA or GTAO, so a frame depends only on the scene. Skips when there is no
//! GPU.

use std::collections::HashMap;

use amoris_assets::frame::RenderFrame;
use amoris_assets::lut::ColorLut;
use amoris_assets::mesh::ModelAsset;
use amoris_render::loader::AssetSource;
use amoris_render::{Antialiasing, BackendChoice, Gpu, Gtao, Renderer, demo};

const W: u32 = 256;
const H: u32 = 144;
const GRID: u32 = 4;

/// Tables by path, each delivered on the poll after its request; a path not here fails to load.
struct Tables {
    tables: HashMap<String, ColorLut>,
    asked: Vec<String>,
}

impl AssetSource for Tables {
    fn request(&mut self, _path: &str) {}
    fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)> {
        Vec::new()
    }
    fn request_color_lut(&mut self, path: &str) {
        self.asked.push(path.to_owned());
    }
    fn poll_color_luts(&mut self) -> Vec<(String, Result<ColorLut, String>)> {
        self.asked
            .drain(..)
            .map(|p| {
                let r = self
                    .tables
                    .get(&p)
                    .cloned()
                    .ok_or_else(|| format!("no table {p}"));
                (p, r)
            })
            .collect()
    }
}

fn sepia([r, g, b]: [f32; 3]) -> [f32; 3] {
    [
        r * 0.393 + g * 0.769 + b * 0.189,
        r * 0.349 + g * 0.686 + b * 0.168,
        r * 0.272 + g * 0.534 + b * 0.131,
    ]
    .map(|v| v.min(1.0))
}

fn tables() -> HashMap<String, ColorLut> {
    let t = |n, f: fn([f32; 3]) -> [f32; 3]| ColorLut::from_fn(n, f).unwrap();
    HashMap::from([
        ("grade/identity33.cube".to_owned(), t(33, |c| c)),
        ("grade/identity17.cube".to_owned(), t(17, |c| c)),
        ("grade/swap.cube".to_owned(), t(33, |[r, g, b]| [g, b, r])),
        ("grade/sepia.cube".to_owned(), t(33, sepia)),
        (
            "grade/invert.cube".to_owned(),
            t(17, |c| c.map(|v| 1.0 - v)),
        ),
    ])
}

/// The environment of the mixed scene with the table `lut` at share `share`, as a frame of its own.
fn graded(base: &RenderFrame, tick: u64, lut: &str, share: f32) -> RenderFrame {
    let mut env = base
        .environment
        .clone()
        .expect("the mixed scene's environment");
    env.color_lut = lut.into();
    env.color_lut_intensity = share;
    RenderFrame {
        tick,
        dt_s: 1.0 / 60.0,
        environment: Some(env),
        ..RenderFrame::default()
    }
}

/// The largest difference between two images (8-bit steps, RGB) and how many channels differ.
fn diff(a: &[u8], b: &[u8]) -> (u8, usize) {
    let mut max = 0;
    let mut n = 0;
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        for k in 0..3 {
            let d = pa[k].abs_diff(pb[k]);
            max = max.max(d);
            n += usize::from(d > 0);
        }
    }
    (max, n)
}

/// The largest difference between an image and `f` of another, channel by channel.
fn off_by(img: &[u8], base: &[u8], f: impl Fn([f32; 3]) -> [f32; 3]) -> f32 {
    let mut max = 0.0f32;
    for (p, q) in img.as_chunks::<4>().0.iter().zip(base.as_chunks::<4>().0) {
        let want = f([q[0], q[1], q[2]].map(|v| f32::from(v) / 255.0));
        for k in 0..3 {
            max = max.max((f32::from(p[k]) - want[k].clamp(0.0, 1.0) * 255.0).abs());
        }
    }
    max
}

fn check(gpu: &Gpu, format: wgpu::TextureFormat) {
    let mut r = Renderer::new(gpu, format, W, H);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.set_asset_source(Box::new(Tables {
        tables: tables(),
        asked: Vec::new(),
    }));
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    r.set_camera_override(Some(demo::mixed_camera(GRID)));
    let scene = demo::mixed(GRID);
    r.apply(scene.clone(), 0.0);
    let base = r.capture_rgba(0.0).2;
    let mut tick = 10;
    let mut draw = |r: &mut Renderer, lut: &str, share: f32| {
        tick += 1;
        r.apply(graded(&scene, tick, lut, share), 0.0);
        r.capture_rgba(0.0).2
    };
    let name = format!("{format:?}");

    let same = |what: &str, img: &[u8]| {
        let (max, n) = diff(img, &base);
        eprintln!("{name}: {what}: {n} channels differ, by at most {max}");
        assert_eq!(
            (max, n),
            (0, 0),
            "{name}: {what} must leave the frame unchanged"
        );
    };
    same("no table", &draw(&mut r, "", 1.0));
    same(
        "the identity at share 0",
        &draw(&mut r, "grade/identity33.cube", 0.0),
    );

    for table in ["grade/identity33.cube", "grade/identity17.cube"] {
        let img = draw(&mut r, table, 1.0);
        let (max, n) = diff(&img, &base);
        let share = n as f64 / f64::from(W * H * 3);
        eprintln!(
            "{name}: {table}: {n} channels ({:.4}%) differ, by at most {max}",
            share * 100.0
        );
        assert!(
            max <= 1 && share <= 0.001,
            "{name}: {table} changed the frame"
        );
    }

    // The scene is coloured: a swap and sepia change most of its channels.
    let changed = |what: &str, img: &[u8]| {
        let (max, n) = diff(img, &base);
        let share = n as f64 / f64::from(W * H * 3);
        eprintln!(
            "{name}: {what}: {:.1}% of the channels changed, by at most {max}",
            share * 100.0
        );
        assert!(share > 0.5, "{name}: {what} left the frame nearly alone");
    };
    let img = draw(&mut r, "grade/swap.cube", 1.0);
    changed("channel swap", &img);
    let off = off_by(&img, &base, |[r, g, b]| [g, b, r]);
    eprintln!("{name}: channel swap: off by at most {off:.2}");
    assert!(off <= 1.0, "{name}: the channel swap is off by {off}");

    let img = draw(&mut r, "grade/sepia.cube", 1.0);
    changed("sepia", &img);
    let off = off_by(&img, &base, sepia);
    eprintln!("{name}: sepia: off by at most {off:.2}");
    assert!(off <= 2.0, "{name}: sepia is off by {off}");

    let img = draw(&mut r, "grade/invert.cube", 0.5);
    let off = off_by(&img, &base, |_| [0.5; 3]);
    eprintln!("{name}: half an inversion: off mid grey by at most {off:.2}");
    assert!(
        off <= 1.5,
        "{name}: half an inversion is off mid grey by {off}"
    );

    same("the table cleared", &draw(&mut r, "", 1.0));

    let img = draw(&mut r, "grade/missing.cube", 1.0);
    assert!(
        r.color_lut_error().is_some_and(|e| e.contains("no table")),
        "{name}: a missing table reports its error"
    );
    same("a table that failed to load", &img);
}

#[test]
fn color_tables_grade_the_display_transform_and_leave_it_alone_without_one() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    eprintln!("{:?} on {}", gpu.info.backend, gpu.info.name);
    for format in [
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Rgba8Unorm,
    ] {
        check(&gpu, format);
    }
}
