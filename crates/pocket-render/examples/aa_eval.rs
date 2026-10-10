//! TAA and GTAO evaluation (docs/bench/taa-gtao.md, docs/spec/taa-gtao.md): image quality against
//! supersampled references and the cost of each anti-aliasing and occlusion option, on the built-in
//! check scene (`demo::aa_scene`) and samples/anim's skinned hero.
//!
//! `cargo run --release -p pocket-render --example aa_eval -- OUT [--size 960x540]
//!  [--only converge,motion,skinned,gtao,bench] [--bench-sizes 1600x900,2560x1440] [--frames 200]`
//!
//! Quality is measured on the HDR image the display transform reads (`Renderer::capture_hdr`),
//! tone mapped here exactly as post.wgsl does (exposure, AgX; bloom is off in the scene; no
//! dither), so every option is compared after the same display transform. A reference is the mean
//! of 256 frames drawn at one sample per pixel with jitter positions spread over the whole pixel
//! (`TaaMode::Jittered`), the scene and camera held still: a box-filtered supersampled image.
//! PSNR is over RGB in [0, 1].
//!
//! - `converge`: the camera and scene still; PSNR against the reference after 1 to 64 frames for
//!   every anti-aliasing mode, and TAA's sharpening amounts.
//! - `motion`: the camera pans and two instances move; at frames 30, 60 and 90 PSNR against that
//!   moment's reference, over the image and around the moving cube (ghosting), for each mode and
//!   TAA's ablations.
//! - `skinned`: the hero's clip at twice its speed, the camera still; TAA with and without the
//!   skinned vertices' own motion, around the hero.
//! - `gtao`: images with and without GTAO, and the visibility it applied.
//! - `bench`: GPU pass times (timestamps) and submit-to-idle wall time per option, provisional.
//!
//! Writes `OUT/aa_eval.json` and small PNG strips.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::{Mat4, Vec3, Vec4};
use pocket_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use pocket_render::demo::{AA_MODEL, aa_camera, aa_cube_position, aa_model, aa_scene};
use pocket_render::taa::{TaaMode, TaaTuning};
use pocket_render::{
    Antialiasing, AoNormals, BackendChoice, CameraState, Gpu, Gtao, OcclusionMode, PrepassMode,
    Renderer,
};
use serde_json::{Value, json};

const DT: f64 = 1.0 / 60.0;
const EXPOSURE: f32 = 0.9;
const REFERENCE_FRAMES: u32 = 256;
const HERO: &str = "hero.glb";
const HERO_ID: u64 = 700;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn size_of(s: &str) -> (u32, u32) {
    let (w, h) = s.split_once('x').expect("WIDTHxHEIGHT");
    (w.parse().expect("width"), h.parse().expect("height"))
}

// --- The display transform (post.wgsl), on the CPU ------------------------------------------------

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn agx_contrast(x: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x
        - 0.00232
}

fn mul3(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    // `m` is column-major, as WGSL's mat3x3f constructor takes columns.
    std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
}

// The matrices are post.wgsl's digits, kept as written there.
#[allow(clippy::excessive_precision)]
fn agx(c: [f32; 3]) -> [f32; 3] {
    let inset = [
        [0.842479062253094, 0.0423282422610123, 0.0423756549057051],
        [0.0784335999999992, 0.878468636469772, 0.0784336],
        [0.0792237451477643, 0.0791661274605434, 0.879142973793104],
    ];
    let outset = [
        [1.19687900512017, -0.0528968517574562, -0.0529716355144438],
        [-0.0980208811401368, 1.15190312990417, -0.0980434501171241],
        [-0.0990297440797205, -0.0989611768448433, 1.15107367264116],
    ];
    let (min_ev, max_ev) = (-12.47393f32, 4.026069f32);
    let v = mul3(inset, c).map(|x| {
        let l = x.max(1e-10).log2().clamp(min_ev, max_ev);
        agx_contrast((l - min_ev) / (max_ev - min_ev))
    });
    let luma = luminance(v);
    let v = v.map(|x| luma + 1.4 * (x.max(0.0).powf(1.35) - luma));
    mul3(outset, v).map(|x| x.clamp(0.0, 1.0))
}

/// The HDR image as the display shows it: `sharpen` as post.wgsl's `sharpened`, exposure, AgX.
fn display(hdr: &[f32], w: u32, h: u32, sharpen: f32) -> Vec<f32> {
    let px = |x: i64, y: i64| -> [f32; 3] {
        let x = x.clamp(0, i64::from(w) - 1) as usize;
        let y = y.clamp(0, i64::from(h) - 1) as usize;
        let i = (y * w as usize + x) * 4;
        [hdr[i], hdr[i + 1], hdr[i + 2]]
    };
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..i64::from(h) {
        for x in 0..i64::from(w) {
            let mut c = px(x, y);
            if sharpen > 0.0 {
                let p = |c: [f32; 3]| c.map(|v| v / (1.0 + luminance(c)));
                let n = [px(x, y - 1), px(x, y + 1), px(x + 1, y), px(x - 1, y)].map(p);
                let pc = p(c);
                let s: [f32; 3] = std::array::from_fn(|k| {
                    let lo = n.iter().fold(pc[k], |a, v| a.min(v[k]));
                    let hi = n.iter().fold(pc[k], |a, v| a.max(v[k]));
                    let mean = 0.25 * n.iter().map(|v| v[k]).sum::<f32>();
                    (pc[k] + sharpen * (pc[k] - mean)).clamp(lo, hi)
                });
                let l = luminance(s);
                c = s.map(|v| v / (1.0 - l).max(1e-4));
            }
            out.extend_from_slice(&agx(c.map(|v| v * EXPOSURE)));
        }
    }
    out
}

/// A rectangle of pixels: x0, y0, x1, y1 (exclusive).
type Rect = (u32, u32, u32, u32);

fn psnr(a: &[f32], b: &[f32], w: u32, roi: Option<Rect>) -> f64 {
    let (x0, y0, x1, y1) = roi.unwrap_or((0, 0, w, (a.len() / 3) as u32 / w));
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 3) as usize;
            for k in 0..3 {
                let d = f64::from(a[i + k] - b[i + k]);
                sum += d * d;
                n += 1;
            }
        }
    }
    let mse = sum / n.max(1) as f64;
    if mse <= 0.0 {
        return 99.0;
    }
    (10.0 * (1.0 / mse).log10() * 100.0).round() / 100.0
}

/// Display values cropped to `r` and scaled `zoom` times (nearest), as 8-bit RGB rows.
fn crop(img: &[f32], w: u32, r: Rect, zoom: u32) -> (u32, u32, Vec<u8>) {
    let (cw, ch) = ((r.2 - r.0) * zoom, (r.3 - r.1) * zoom);
    let mut out = Vec::with_capacity((cw * ch * 3) as usize);
    for y in 0..ch {
        for x in 0..cw {
            let i = (((r.1 + y / zoom) * w + r.0 + x / zoom) * 3) as usize;
            for k in 0..3 {
                out.push((img[i + k] * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
            }
        }
    }
    (cw, ch, out)
}

/// The whole image at half size (2x2 box), as 8-bit RGB rows: full frames stay small as evidence.
fn half(img: &[f32], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let (hw, hh) = (w / 2, h / 2);
    let mut out = Vec::with_capacity((hw * hh * 3) as usize);
    for y in 0..hh {
        for x in 0..hw {
            for k in 0..3 {
                let at = |dx: u32, dy: u32| img[(((2 * y + dy) * w + 2 * x + dx) * 3) as usize + k];
                let v = 0.25 * (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1));
                out.push((v * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
            }
        }
    }
    (hw, hh, out)
}

/// Saves crops side by side with a 4-pixel gap.
fn strip(path: &Path, tiles: &[(u32, u32, Vec<u8>)]) {
    let w: u32 = tiles.iter().map(|t| t.0 + 4).sum::<u32>() - 4;
    let h = tiles.iter().map(|t| t.1).max().unwrap_or(1);
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
    let mut x0 = 0;
    for (tw, th, px) in tiles {
        for y in 0..*th {
            for x in 0..*tw {
                let i = ((y * tw + x) * 3) as usize;
                img.put_pixel(x0 + x, y, image::Rgb([px[i], px[i + 1], px[i + 2]]));
            }
        }
        x0 += tw + 4;
    }
    img.save(path).expect("save strip");
    eprintln!("wrote {}", path.display());
}

/// The screen rectangle of world box `centre` +- `half` from `camera`, grown by `pad` pixels.
fn screen_rect(camera: &CameraState, w: u32, h: u32, centre: Vec3, half: Vec3, pad: u32) -> Rect {
    let vp = camera.proj(w as f32 / h as f32) * camera.view();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for k in 0..8 {
        let s = Vec3::new(
            if k & 1 == 0 { -1.0 } else { 1.0 },
            if k & 2 == 0 { -1.0 } else { 1.0 },
            if k & 4 == 0 { -1.0 } else { 1.0 },
        );
        let c = vp * Vec4::from((centre + half * s, 1.0));
        let (nx, ny) = (c.x / c.w, c.y / c.w);
        let (px, py) = ((nx * 0.5 + 0.5) * w as f32, (0.5 - ny * 0.5) * h as f32);
        x0 = x0.min(px);
        y0 = y0.min(py);
        x1 = x1.max(px);
        y1 = y1.max(py);
    }
    let p = pad as f32;
    (
        (x0 - p).clamp(0.0, w as f32 - 1.0) as u32,
        (y0 - p).clamp(0.0, h as f32 - 1.0) as u32,
        (x1 + p).clamp(1.0, w as f32) as u32,
        (y1 + p).clamp(1.0, h as f32) as u32,
    )
}

fn union(a: Rect, b: Rect) -> Rect {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

// --- Scenes ---------------------------------------------------------------------------------------

/// How the camera moves.
#[derive(Clone, Copy, PartialEq)]
enum Pan {
    Still,
    /// `demo::aa_camera`'s pan.
    Sideways,
    /// An orthographic camera moving this many pixels a frame sideways: whole pixels reproject
    /// onto texel centres exactly (the reprojection check).
    Ortho(f32),
}

fn camera(k: u64, pan: Pan, h: u32) -> CameraState {
    match pan {
        Pan::Still => aa_camera(0, false),
        Pan::Sideways => aa_camera(k as u32, true),
        Pan::Ortho(px) => {
            let height = 9.0;
            let mut c = CameraState::look_at(Vec3::new(-1.0, 4.0, 8.0), Vec3::new(-1.0, 0.8, -1.0));
            c.ortho_height = Some(height);
            c.position += c.rotation * Vec3::X * (k as f32 * px * height / h as f32);
            c
        }
    }
}

struct Setup<'a> {
    gpu: &'a Gpu,
    w: u32,
    h: u32,
    hero: Option<&'a pocket_assets::mesh::ModelAsset>,
}

/// samples/anim's hero, its clip at twice its speed, at tick `tick`.
fn hero_frame(tick: u64, clip: &str, full: bool) -> InstanceUpdate {
    InstanceUpdate {
        id: HERO_ID,
        pose: Some(Pose {
            position: [1.6, 0.0, 2.2],
            rotation: glam::Quat::from_rotation_y(-0.4).to_array(),
            scale: [1.0; 3],
        }),
        look: full.then(|| Look {
            mesh: HERO.into(),
            material: String::new(),
            color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.6,
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: true,
            visible: true,
        }),
        anim: Some(AnimView {
            clip: clip.into(),
            time: (tick as f64 * DT * 2.0) as f32,
            rate: 2.0,
            looped: true,
        }),
    }
}

impl Setup<'_> {
    fn renderer(&self, aa: Antialiasing, gtao: Gtao) -> Renderer {
        let mut r = Renderer::new(
            self.gpu,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            self.w,
            self.h,
        );
        r.set_occlusion(OcclusionMode::Off);
        // The opaque pass timed as before the depth prepass (docs/spec/prepass.md).
        r.set_prepass(PrepassMode::Off);
        r.set_antialiasing(aa);
        r.set_gtao(gtao);
        r.add_model(AA_MODEL, &aa_model());
        if let Some(hero) = self.hero {
            r.add_model(HERO, hero);
        }
        r
    }

    /// The scene at tick `k` (applied at its time): the whole of it at tick 0.
    fn apply(&self, r: &mut Renderer, k: u64, moving: bool, hero_clip: Option<&str>) {
        let mut f = aa_scene(k, moving, k == 0);
        if let Some(clip) = hero_clip {
            f.instances.push(hero_frame(k, clip, k == 0));
        }
        r.apply(f, k as f64 * DT);
    }

    /// Draws the frame of tick `k` half way to the next and returns its HDR image.
    fn draw(&self, r: &mut Renderer, k: u64, pan: Pan) -> Vec<f32> {
        r.set_camera_override(Some(camera(k, pan, self.h)));
        r.capture_hdr(k as f64 * DT + 0.5 * DT).2
    }

    /// The reference of the scene at tick `k` (prev tick `k - 1`), drawn half way between them.
    fn reference(&self, k: u64, moving: bool, pan: Pan, hero_clip: Option<&str>) -> Vec<f32> {
        let mut r = self.renderer(Antialiasing::Taa, Gtao::Off);
        r.taa_mut().mode = TaaMode::Jittered;
        self.apply(&mut r, 0, moving, hero_clip);
        if k > 1 {
            self.apply(&mut r, k - 1, moving, hero_clip);
        }
        if k > 0 {
            self.apply(&mut r, k, moving, hero_clip);
        }
        for _ in 0..16 {
            self.draw(&mut r, k, pan);
        }
        let mut sum = vec![0.0f64; (self.w * self.h * 4) as usize];
        for _ in 0..REFERENCE_FRAMES {
            for (s, v) in sum.iter_mut().zip(self.draw(&mut r, k, pan)) {
                *s += f64::from(v);
            }
        }
        sum.iter()
            .map(|s| (s / f64::from(REFERENCE_FRAMES)) as f32)
            .collect()
    }
}

const MODES: [Antialiasing; 4] = [
    Antialiasing::Off,
    Antialiasing::Msaa,
    Antialiasing::Taa,
    Antialiasing::MsaaTaa,
];

fn converge(s: &Setup, out: &Path) -> Value {
    let reference = s.reference(0, false, Pan::Still, None);
    let ref_disp = display(&reference, s.w, s.h, 0.0);
    let checkpoints = [1u32, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64];
    let mut results = BTreeMap::new();
    let mut finals = BTreeMap::new();
    let mut variants: Vec<(String, Antialiasing, TaaTuning)> = MODES
        .iter()
        .map(|m| (m.name().to_owned(), *m, TaaTuning::default()))
        .collect();
    variants.push((
        "msaa+taa (jitter 0.5)".into(),
        Antialiasing::MsaaTaa,
        TaaTuning {
            jitter: 0.5,
            ..TaaTuning::default()
        },
    ));
    variants.push((
        "taa (gamma 1.0)".into(),
        Antialiasing::Taa,
        TaaTuning {
            gamma: 1.0,
            ..TaaTuning::default()
        },
    ));
    variants.push((
        "taa (alpha 0.05)".into(),
        Antialiasing::Taa,
        TaaTuning {
            alpha: 0.05,
            ..TaaTuning::default()
        },
    ));
    variants.push((
        "taa (gamma 1.5)".into(),
        Antialiasing::Taa,
        TaaTuning {
            gamma: 1.5,
            ..TaaTuning::default()
        },
    ));
    variants.push((
        "taa (still gamma 3)".into(),
        Antialiasing::Taa,
        TaaTuning {
            still_gamma: 3.0,
            ..TaaTuning::default()
        },
    ));
    variants.push((
        "taa (inside alpha 0.05)".into(),
        Antialiasing::Taa,
        TaaTuning {
            alpha_inside: 0.05,
            ..TaaTuning::default()
        },
    ));
    for (name, aa, tuning) in &variants {
        let mut r = s.renderer(*aa, Gtao::Off);
        r.taa_mut().tuning = *tuning;
        s.apply(&mut r, 0, false, None);
        let mut curve = Vec::new();
        let mut last = Vec::new();
        for f in 1..=64u32 {
            let hdr = s.draw(&mut r, 0, Pan::Still);
            if checkpoints.contains(&f) {
                let d = display(&hdr, s.w, s.h, 0.0);
                curve.push(json!([f, psnr(&d, &ref_disp, s.w, None)]));
                eprintln!("converge {name} frame {f}: {:?}", curve.last());
            }
            last = hdr;
        }
        let mut entry = json!({ "psnr_by_frame": curve });
        if aa.taa() && name == aa.name() {
            let mut sharpen = Vec::new();
            for amount in [0.0f32, 0.25, 0.5, 1.0] {
                let d = display(&last, s.w, s.h, amount);
                sharpen.push(json!([amount, psnr(&d, &ref_disp, s.w, None)]));
            }
            entry["psnr_frame64_by_sharpen"] = json!(sharpen);
        }
        results.insert(name.clone(), entry);
        finals.insert(name.clone(), display(&last, s.w, s.h, 0.0));
    }
    // Crops: the fence and wires, the chrome spheres, the slab against the sky.
    let regions = [
        ("fence", fence_rect(s)),
        (
            "spheres",
            screen_rect(
                &aa_camera(0, false),
                s.w,
                s.h,
                Vec3::new(-0.4, 0.5, -0.8),
                Vec3::new(1.6, 0.5, 0.5),
                4,
            ),
        ),
        (
            "chain-link",
            screen_rect(
                &aa_camera(0, false),
                s.w,
                s.h,
                Vec3::new(3.6, 1.0, -2.2),
                Vec3::new(1.2, 0.8, 0.6),
                0,
            ),
        ),
    ];
    let mut by_region = BTreeMap::new();
    for (label, rect) in regions {
        let mut tiles = Vec::new();
        let mut row = BTreeMap::new();
        for m in MODES {
            tiles.push(crop(&finals[m.name()], s.w, rect, 2));
            row.insert(
                m.name(),
                psnr(&finals[m.name()], &ref_disp, s.w, Some(rect)),
            );
        }
        tiles.push(crop(&ref_disp, s.w, rect, 2));
        strip(&out.join(format!("converge-{label}.png")), &tiles);
        by_region.insert(
            label,
            json!({"rect": [rect.0, rect.1, rect.2, rect.3], "psnr_frame64": row}),
        );
    }
    json!({
        "reference": format!("{REFERENCE_FRAMES} frames, 1 sample, Halton (2,3) jitter over the pixel"),
        "strip_order": ["off", "msaa", "taa", "msaa+taa", "reference"],
        "modes": results,
        "regions": by_region,
    })
}

fn fence_rect(s: &Setup) -> Rect {
    screen_rect(
        &aa_camera(0, false),
        s.w,
        s.h,
        Vec3::new(-2.0, 1.6, -5.0),
        Vec3::new(3.0, 1.2, 1.0),
        0,
    )
}

/// PSNR of the change between two frames against the reference's change (temporal stability:
/// shimmer and crawl the reference does not have lower it).
fn temporal_psnr(a0: &[f32], a1: &[f32], r0: &[f32], r1: &[f32], w: u32) -> f64 {
    let d: Vec<f32> = a1.iter().zip(a0).map(|(x, y)| 0.5 + x - y).collect();
    let e: Vec<f32> = r1.iter().zip(r0).map(|(x, y)| 0.5 + x - y).collect();
    psnr(&d, &e, w, None)
}

fn motion_variants() -> Vec<(String, Antialiasing, TaaTuning)> {
    let mut variants: Vec<(String, Antialiasing, TaaTuning)> = MODES
        .iter()
        .map(|m| (m.name().to_owned(), *m, TaaTuning::default()))
        .collect();
    let t = TaaTuning::default;
    for (name, tuning) in [
        (
            "taa without object motion",
            TaaTuning {
                object_motion: false,
                ..t()
            },
        ),
        (
            "taa without disocclusion",
            TaaTuning {
                disocclusion: false,
                ..t()
            },
        ),
        (
            "taa without speed weight",
            TaaTuning {
                speed_weight: false,
                ..t()
            },
        ),
        ("taa (gamma 1.0)", TaaTuning { gamma: 1.0, ..t() }),
        ("taa (gamma 1.5)", TaaTuning { gamma: 1.5, ..t() }),
        ("taa (alpha 0.05)", TaaTuning { alpha: 0.05, ..t() }),
        (
            "taa (still gamma 3)",
            TaaTuning {
                still_gamma: 3.0,
                ..t()
            },
        ),
        (
            "taa (inside alpha 0.05)",
            TaaTuning {
                alpha_inside: 0.05,
                ..t()
            },
        ),
        (
            "taa (inside alpha 0.04, alpha 0.15)",
            TaaTuning {
                alpha_inside: 0.04,
                alpha: 0.15,
                ..t()
            },
        ),
    ] {
        variants.push((name.into(), Antialiasing::Taa, tuning));
    }
    variants
}

/// The scene's instances move; the camera pans (`pan`) or stands still. At frames 30, 60 and 90:
/// PSNR against that moment's reference over the image and around the moving cube (with its shadow
/// and the last 12 frames' positions: ghosting), and the temporal PSNR of the step from the frame
/// before.
fn motion(s: &Setup, out: &Path, pan: Pan) -> Value {
    let label = if pan == Pan::Still { "still" } else { "pan" };
    let checkpoints = [30u64, 60, 90];
    let refs: Vec<(Vec<f32>, Vec<f32>)> = checkpoints
        .iter()
        .map(|&k| {
            (
                display(&s.reference(k - 1, true, pan, None), s.w, s.h, 0.0),
                display(&s.reference(k, true, pan, None), s.w, s.h, 0.0),
            )
        })
        .collect();
    let roi = |k: u64| {
        let cam = camera(k, pan, s.h);
        let mut r = None;
        for j in 0..=12u64 {
            let t = (k.saturating_sub(j) as f64 * DT + 0.5 * DT) as f32;
            let c = Vec3::from(aa_cube_position(t));
            let rect = screen_rect(&cam, s.w, s.h, c, Vec3::splat(0.6), 3);
            let shadow = screen_rect(
                &cam,
                s.w,
                s.h,
                c + Vec3::new(0.4, -0.6, 0.3),
                Vec3::new(1.0, 0.01, 0.9),
                3,
            );
            r = Some(r.map_or(union(rect, shadow), |a| union(a, union(rect, shadow))));
        }
        r.unwrap_or((0, 0, 1, 1))
    };
    let mut results = BTreeMap::new();
    let mut tiles_at_60 = BTreeMap::new();
    for (name, aa, tuning) in motion_variants() {
        let mut r = s.renderer(aa, Gtao::Off);
        r.taa_mut().tuning = tuning;
        let mut rows = Vec::new();
        let mut before = Vec::new();
        for k in 0..=*checkpoints.last().unwrap_or(&90) {
            s.apply(&mut r, k, true, None);
            let hdr = s.draw(&mut r, k, pan);
            let d = display(&hdr, s.w, s.h, 0.0);
            if let Some(i) = checkpoints.iter().position(|&c| c == k) {
                let rect = roi(k);
                let mut row = json!({
                    "frame": k,
                    "psnr": psnr(&d, &refs[i].1, s.w, None),
                    "psnr_moving_cube": psnr(&d, &refs[i].1, s.w, Some(rect)),
                    "temporal_psnr": temporal_psnr(&before, &d, &refs[i].0, &refs[i].1, s.w),
                    "roi": [rect.0, rect.1, rect.2, rect.3],
                });
                if name == "taa" || name == "msaa+taa" {
                    row["psnr_by_sharpen"] = json!(
                        [0.25f32, 0.5]
                            .iter()
                            .map(|a| json!([
                                a,
                                psnr(&display(&hdr, s.w, s.h, *a), &refs[i].1, s.w, None)
                            ]))
                            .collect::<Vec<_>>()
                    );
                }
                eprintln!("motion ({label}) {name} {row}");
                rows.push(row);
                if k == 60 {
                    tiles_at_60.insert(name.clone(), crop(&d, s.w, rect, 2));
                }
            }
            before = d;
        }
        results.insert(name.clone(), json!(rows));
    }
    let order = [
        "msaa",
        "taa",
        "taa without object motion",
        "taa (still gamma 3)",
    ];
    let mut tiles: Vec<_> = order
        .iter()
        .filter_map(|n| tiles_at_60.get(*n).cloned())
        .collect();
    tiles.push(crop(&refs[1].1, s.w, roi(60), 2));
    strip(&out.join(format!("motion-{label}.png")), &tiles);
    json!({
        "camera": if pan == Pan::Still { "still" } else { "pans 0.6 m/s sideways" },
        "objects": "cube 2.4 m/s sideways, torus spinning",
        "strip_order": order.iter().chain(["reference"].iter()).collect::<Vec<_>>(),
        "variants": results,
    })
}

fn skinned(s: &Setup, out: &Path, clip: &str) -> Value {
    let k = 75u64;
    let reference = display(
        &s.reference(k, false, Pan::Still, Some(clip)),
        s.w,
        s.h,
        0.0,
    );
    let rect = screen_rect(
        &aa_camera(0, false),
        s.w,
        s.h,
        Vec3::new(1.6, 0.95, 2.2),
        Vec3::new(0.6, 1.0, 0.6),
        6,
    );
    let mut results = BTreeMap::new();
    let mut tiles = Vec::new();
    for (name, aa, skinned_motion) in [
        ("msaa", Antialiasing::Msaa, true),
        ("taa", Antialiasing::Taa, true),
        ("taa without skinned motion", Antialiasing::Taa, false),
    ] {
        let mut r = s.renderer(aa, Gtao::Off);
        r.taa_mut().tuning.skinned_motion = skinned_motion;
        let mut last = Vec::new();
        for f in 0..=k {
            s.apply(&mut r, f, false, Some(clip));
            last = s.draw(&mut r, f, Pan::Still);
        }
        let d = display(&last, s.w, s.h, 0.0);
        results.insert(
            name,
            json!({
                "psnr": psnr(&d, &reference, s.w, None),
                "psnr_hero": psnr(&d, &reference, s.w, Some(rect)),
            }),
        );
        eprintln!("skinned {name}: {:?}", results[name]);
        tiles.push(crop(&d, s.w, rect, 2));
    }
    tiles.push(crop(&reference, s.w, rect, 2));
    strip(&out.join("skinned-hero.png"), &tiles);
    json!({
        "clip": clip,
        "frame": k,
        "strip_order": ["msaa", "taa", "taa without skinned motion", "reference"],
        "results": results,
    })
}

/// The reprojection check: an orthographic camera panning whole pixels (and half pixels) a frame
/// over the still scene. With exact reprojection TAA after 64 frames is as close to the reference
/// as on a still camera; a wrong convention (sign, half-pixel offset) shows as a large drop.
fn reproject(s: &Setup) -> Value {
    let mut rows = BTreeMap::new();
    for px in [0.0f32, 1.0, 2.0, 0.5] {
        let k = 64u64;
        let reference = display(&s.reference(k, false, Pan::Ortho(px), None), s.w, s.h, 0.0);
        for aa in [Antialiasing::Msaa, Antialiasing::Taa] {
            let mut r = s.renderer(aa, Gtao::Off);
            s.apply(&mut r, 0, false, None);
            let mut last = Vec::new();
            for f in 0..=k {
                last = s.draw(&mut r, f, Pan::Ortho(px));
            }
            let v = psnr(&display(&last, s.w, s.h, 0.0), &reference, s.w, None);
            eprintln!("reproject {px} px/frame {}: {v}", aa.name());
            rows.insert(format!("{px} px/frame, {}", aa.name()), json!(v));
        }
    }
    json!(rows)
}

fn gtao(s: &Setup, out: &Path) -> Value {
    let mut images = BTreeMap::new();
    for (name, aa, g) in [
        ("msaa", Antialiasing::Msaa, Gtao::Off),
        ("msaa+gtao", Antialiasing::Msaa, Gtao::On(AoNormals::Depth)),
        (
            "msaa+gtao (normal target)",
            Antialiasing::Msaa,
            Gtao::On(AoNormals::Target),
        ),
        ("taa", Antialiasing::Taa, Gtao::Off),
        ("taa+gtao", Antialiasing::Taa, Gtao::On(AoNormals::Depth)),
        ("off", Antialiasing::Off, Gtao::Off),
        ("off+gtao", Antialiasing::Off, Gtao::On(AoNormals::Depth)),
    ] {
        let mut r = s.renderer(aa, g);
        s.apply(&mut r, 0, false, None);
        let mut hdr = Vec::new();
        for _ in 0..32 {
            hdr = s.draw(&mut r, 0, Pan::Still);
        }
        images.insert(name, hdr);
    }
    // The visibility applied: on / off per pixel (luminance), where the scene is lit indirectly.
    let off = &images["msaa"];
    // The visibility applied: with GTAO / without, per pixel, under the same anti-aliasing.
    let ratio_to = |off: &[f32], on: &[f32]| -> Vec<f32> {
        off.chunks(4)
            .zip(on.chunks(4))
            .flat_map(|(a, b)| {
                let la = luminance([a[0], a[1], a[2]]);
                let lb = luminance([b[0], b[1], b[2]]);
                let v = if la > 1e-4 {
                    (lb / la).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                [v, v, v]
            })
            .collect()
    };
    let ratio = |on: &[f32]| ratio_to(off, on);
    let mut summary = BTreeMap::new();
    for (name, img) in &images {
        if ["msaa", "taa", "off"].contains(name) {
            continue;
        }
        let base = if name.starts_with("taa") {
            &images["taa"]
        } else if name.starts_with("off") {
            &images["off"]
        } else {
            off
        };
        let r = ratio_to(base, img);
        let mean = r.iter().step_by(3).map(|v| f64::from(*v)).sum::<f64>() / (r.len() / 3) as f64;
        let darker =
            r.iter().step_by(3).filter(|v| **v < 0.95).count() as f64 / (r.len() / 3) as f64;
        // GTAO never brightens a pixel; with TAA the two runs' histories may also differ, and the
        // pixels that came out brighter measure that.
        let brighter = base
            .chunks(4)
            .zip(img.chunks(4))
            .filter(|(a, b)| luminance([b[0], b[1], b[2]]) > 1.05 * luminance([a[0], a[1], a[2]]))
            .count() as f64
            / (r.len() / 3) as f64;
        summary.insert(
            *name,
            json!({"mean_luminance_ratio": mean, "share_darkened_5pct": darker,
                   "share_brightened_5pct": brighter}),
        );
    }
    let d_off = display(off, s.w, s.h, 0.0);
    let d_on = display(&images["msaa+gtao"], s.w, s.h, 0.0);
    let d_target = display(&images["msaa+gtao (normal target)"], s.w, s.h, 0.0);
    summary.insert(
        "depth_vs_target_normals_psnr",
        json!(psnr(&d_on, &d_target, s.w, None)),
    );
    strip(
        &out.join("gtao.png"),
        &[
            half(&d_off, s.w, s.h),
            half(&d_on, s.w, s.h),
            half(&ratio(&images["msaa+gtao"]), s.w, s.h),
        ],
    );
    let corner = screen_rect(
        &aa_camera(0, false),
        s.w,
        s.h,
        Vec3::new(-5.0, 1.0, -1.2),
        Vec3::new(1.6, 1.3, 1.4),
        2,
    );
    strip(
        &out.join("gtao-corner.png"),
        &[
            crop(&d_off, s.w, corner, 2),
            crop(&d_on, s.w, corner, 2),
            crop(&d_target, s.w, corner, 2),
            crop(&display(&images["taa+gtao"], s.w, s.h, 0.0), s.w, corner, 2),
        ],
    );
    json!({
        "strip_order": ["off", "on", "visibility applied (on / off)"],
        "corner_strip_order": ["off", "on (normals from depth)", "on (normal target)", "taa + on"],
        "summary": summary,
    })
}

fn bench(gpu: &Gpu, sizes: &[(u32, u32)], frames: u32, filter: &[String]) -> Value {
    let mut rows = Vec::new();
    for &(w, h) in sizes {
        let setup = Setup {
            gpu,
            w,
            h,
            hero: None,
        };
        for aa in MODES {
            for g in [
                Gtao::Off,
                Gtao::On(AoNormals::Depth),
                Gtao::On(AoNormals::Target),
            ] {
                if g == Gtao::On(AoNormals::Target)
                    && aa != Antialiasing::Msaa
                    && aa != Antialiasing::Taa
                {
                    continue;
                }
                let key = format!("{}:{}", aa.name(), g.name());
                if !filter.is_empty() && !filter.contains(&key) {
                    continue;
                }
                let mut r = setup.renderer(aa, g);
                setup.apply(&mut r, 0, false, None);
                r.set_camera_override(Some(aa_camera(0, false)));
                let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("bench"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
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
                for _ in 0..30 {
                    r.render(&view, 0.0);
                    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                }
                let mut wall = Vec::new();
                let mut total = Vec::new();
                let mut passes: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
                for _ in 0..frames {
                    let t = Instant::now();
                    let st = r.render(&view, 0.0);
                    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                    wall.push(t.elapsed().as_secs_f64() * 1000.0);
                    total.push(f64::from(st.gpu_ms));
                    for (p, ms) in st.passes {
                        passes.entry(p).or_default().push(f64::from(ms));
                    }
                }
                // The median, and the 10th percentile (less of other processes' GPU work).
                let p50 = |v: &mut Vec<f64>| {
                    v.sort_by(f64::total_cmp);
                    let x = v.get(v.len() / 2).copied().unwrap_or(0.0);
                    (x * 1000.0).round() / 1000.0
                };
                let p10 = |v: &mut Vec<f64>| {
                    v.sort_by(f64::total_cmp);
                    let x = v.get(v.len() / 10).copied().unwrap_or(0.0);
                    (x * 1000.0).round() / 1000.0
                };
                let row = json!({
                    "size": format!("{w}x{h}"),
                    "aa": aa.name(),
                    "gtao": g.name(),
                    "wall_p50_ms": p50(&mut wall),
                    "profiled_gpu_p50_ms": p50(&mut total),
                    "passes_p50_ms": passes.iter_mut().map(|(k, v)| (*k, p50(v))).collect::<BTreeMap<_, _>>(),
                    "profiled_gpu_p10_ms": p10(&mut total),
                    "passes_p10_ms": passes.iter_mut().map(|(k, v)| (*k, p10(v))).collect::<BTreeMap<_, _>>(),
                });
                eprintln!("bench {row}");
                rows.push(row);
            }
        }
    }
    json!(rows)
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: aa_eval OUT [options]"),
    );
    std::fs::create_dir_all(&out).expect("output directory");
    let (w, h) = size_of(&arg("--size").unwrap_or_else(|| "960x540".into()));
    let only: Vec<String> = arg("--only")
        .unwrap_or_else(|| "converge,motion,skinned,gtao,bench".into())
        .split(',')
        .map(str::to_owned)
        .collect();
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("a GPU");
    let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let e = errors.clone();
    gpu.device
        .on_uncaptured_error(std::sync::Arc::new(move |error: wgpu::Error| {
            eprintln!("renderer validation: {error:?}");
            e.lock().expect("errors").push(format!("{error:?}"));
        }));
    let hero_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/anim/models/hero.glb");
    let hero = pocket_assets::import::import_gltf(&hero_path).ok();
    let clip = hero
        .as_ref()
        .and_then(|h| h.animations.first().map(|a| a.name.clone()))
        .unwrap_or_default();
    let setup = Setup {
        gpu: &gpu,
        w,
        h,
        hero: hero.as_ref(),
    };
    let mut report = json!({
        "adapter": gpu.info.name,
        "backend": gpu.backend_name(),
        "size": format!("{w}x{h}"),
        "provisional": "other agents shared the GPU; timings are provisional",
        "hero_clips": hero.as_ref().map(|h| h.animations.iter().map(|a| a.name.clone()).collect::<Vec<_>>()),
    });
    let started = Instant::now();
    if only.iter().any(|o| o == "converge") {
        report["converge"] = converge(&setup, &out);
    }
    if only.iter().any(|o| o == "motion") {
        report["motion_pan"] = motion(&setup, &out, Pan::Sideways);
        report["motion_still"] = motion(&setup, &out, Pan::Still);
    }
    if only.iter().any(|o| o == "skinned") && hero.is_some() {
        report["skinned"] = skinned(&setup, &out, &clip);
    }
    if only.iter().any(|o| o == "reproject") {
        report["reproject"] = reproject(&setup);
    }
    if only.iter().any(|o| o == "gtao") {
        report["gtao"] = gtao(&setup, &out);
    }
    if only.iter().any(|o| o == "bench") {
        let sizes: Vec<(u32, u32)> = arg("--bench-sizes")
            .unwrap_or_else(|| "1600x900,2560x1440".into())
            .split(',')
            .map(size_of)
            .collect();
        let frames = arg("--frames").and_then(|f| f.parse().ok()).unwrap_or(200);
        let filter: Vec<String> = arg("--bench-configs")
            .map(|f| f.split(',').map(str::to_owned).collect())
            .unwrap_or_default();
        report["bench"] = bench(&gpu, &sizes, frames, &filter);
    }
    report["seconds"] = json!(started.elapsed().as_secs_f64());
    report["validation_errors"] = json!(errors.lock().expect("errors").clone());
    let path = out.join(format!(
        "aa_eval-{}-{}.json",
        gpu.backend_name(),
        gpu.info
            .name
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join("_")
    ));
    std::fs::write(&path, serde_json::to_vec_pretty(&report).expect("json")).expect("write");
    eprintln!("wrote {}", path.display());
    let _ = Mat4::IDENTITY;
    let _ = RenderFrame::default();
}
