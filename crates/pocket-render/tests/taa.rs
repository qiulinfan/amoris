//! TAA and GTAO (docs/spec/taa-gtao.md) on a headless GPU; skips without one.
//!
//! - Reprojection is exact: an orthographic camera that pans whole pixels a frame reprojects every
//!   history texel onto a texel centre, so TAA's image after the pan equals TAA's image from a still
//!   camera at the pan's end, away from the edge new content enters. The same holds for an object
//!   moving whole pixels a frame under a still camera, through the forward shader's object motion;
//!   without it (the switch the check flips) the object's history comes from the wrong place.
//! - The entity-id pass is never jittered: with TAA its coverage equals the coverage without.
//! - GTAO darkens only indirect light: under the sun alone it changes nothing; with sky light it
//!   never brightens a pixel and darkens the creases.
//! - Camera cuts and resizes drop the history.

use glam::{Quat, Vec3};
use pocket_assets::frame::{InstanceUpdate, Look, Pose, RenderFrame, SeaView};
use pocket_render::demo::{AA_MODEL, aa_model, aa_scene};
use pocket_render::{Antialiasing, AoNormals, BackendChoice, CameraState, Gpu, Gtao, Renderer};

const W: u32 = 256;
const H: u32 = 144;
const DT: f64 = 1.0 / 60.0;
/// The orthographic view's height in metres: a pixel is `ORTHO / H` metres.
const ORTHO: f32 = 9.0;

fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless(BackendChoice::from_env()).ok();
    if gpu.is_none() {
        eprintln!("no GPU: skipped");
    }
    gpu
}

fn renderer(gpu: &Gpu, aa: Antialiasing, gtao: Gtao) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(aa);
    r.set_gtao(gtao);
    r.add_model(AA_MODEL, &aa_model());
    r
}

/// `demo::aa_scene`, still, without sun shadows (the cascades follow the camera, so a panning
/// camera's shadows would differ by more than the reprojection) and with sky light `ambient`.
fn still_scene(ambient: f32) -> RenderFrame {
    let mut f = aa_scene(0, false, true);
    for l in f.lights.iter_mut().flatten() {
        l.shadows = false;
    }
    if let Some(e) = &mut f.environment {
        e.ambient = ambient;
    }
    f
}

/// An orthographic camera looking at the scene, moved `px` pixels to its right.
fn ortho_camera(px: f32) -> CameraState {
    let mut c = CameraState::look_at(Vec3::new(-1.0, 1.4, 9.0), Vec3::new(-1.0, 1.0, -1.0));
    c.ortho_height = Some(ORTHO);
    c.position += c.rotation * Vec3::X * (px * ORTHO / H as f32);
    c
}

/// PSNR of two HDR images over `columns` and `rows`, on c / (1 + c) (bright values do not
/// dominate).
fn psnr(a: &[f32], b: &[f32], columns: std::ops::Range<u32>, rows: std::ops::Range<u32>) -> f64 {
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for y in rows {
        for x in columns.clone() {
            let i = ((y * W + x) * 4) as usize;
            for k in 0..3 {
                let (p, q) = (a[i + k], b[i + k]);
                let d = f64::from(p / (1.0 + p) - q / (1.0 + q));
                sum += d * d;
                n += 1;
            }
        }
    }
    let mse = sum / n.max(1) as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (1.0 / mse).log10()
    }
}

#[test]
fn taa_reprojects_a_whole_pixel_pan_exactly() {
    let Some(gpu) = gpu() else { return };
    let frames = 24u32;
    // A still camera at the pan's end...
    let mut still = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
    still.apply(still_scene(1.0), 0.0);
    still.set_camera_override(Some(ortho_camera(frames as f32)));
    let mut a = Vec::new();
    for f in 1..=frames {
        a = still.capture_hdr(f64::from(f) * DT).2;
    }
    // The still camera's history is close to the plain mean of the jittered frames it saw (the
    // clip box keeps it from being equal); a history that drifts would not be.
    let mut jittered = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
    jittered.taa_mut().mode = pocket_render::taa::TaaMode::Jittered;
    jittered.apply(still_scene(1.0), 0.0);
    jittered.set_camera_override(Some(ortho_camera(frames as f32)));
    let mut mean = vec![0.0f32; a.len()];
    for f in 1..=64 {
        for (m, v) in mean
            .iter_mut()
            .zip(jittered.capture_hdr(f64::from(f) * DT).2)
        {
            *m += v / 64.0;
        }
    }
    let m = psnr(&a, &mean, 4..W - 4, 4..H - 4);
    eprintln!("still: {m:.1} dB against the mean of 64 jittered frames");
    assert!(
        m > 39.0,
        "a still camera's history is {m:.1} dB from the jittered mean"
    );
    // ...and a camera panning one pixel a frame to it.
    let mut panning = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
    panning.apply(still_scene(1.0), 0.0);
    let mut b = Vec::new();
    for f in 1..=frames {
        panning.set_camera_override(Some(ortho_camera(f as f32)));
        b = panning.capture_hdr(f64::from(f) * DT).2;
    }
    assert_eq!(panning.taa_mut().frames(), frames, "the pan was not a cut");
    // New content enters at the right edge; the left keeps a margin for the clamped border.
    let p = psnr(&a, &b, 4..W - frames - 4, 4..H - 4);
    eprintln!("pan: {p:.1} dB against the still camera");
    assert!(p > 45.0, "a whole-pixel pan reprojected to {p:.1} dB");
}

/// A checkerboard of 8 x 6 cubes 0.4 m wide facing the camera, `x` metres right of its start.
fn board(x: f32, full: bool) -> Vec<InstanceUpdate> {
    (0..48u64)
        .map(|i| {
            let (cx, cy) = ((i % 8) as f32, (i / 8) as f32);
            let v = if (i % 8 + i / 8) % 2 == 0 { 0.9 } else { 0.08 };
            InstanceUpdate {
                id: 900 + i,
                pose: Some(Pose {
                    position: [-2.5 + x + (cx - 3.5) * 0.4, 1.6 + (cy - 2.5) * 0.4, 1.0],
                    rotation: Quat::IDENTITY.to_array(),
                    scale: [0.4, 0.4, 0.05],
                }),
                look: full.then(|| Look {
                    mesh: "cube".into(),
                    material: String::new(),
                    color: [v, v * 0.9, v * 0.8, 1.0],
                    metallic: 0.0,
                    roughness: 0.7,
                    transmission: None,
                    ior: None,
                    emissive: [0.0; 3],
                    cast_shadows: false,
                    visible: true,
                }),
                anim: None,
            }
        })
        .collect()
}

/// TAA's image after `frames` frames of the board sliding one pixel a frame (or standing where
/// the slide ends), and the board's pixel rectangle at the end.
fn board_run(gpu: &Gpu, frames: u64, moving: bool, object_motion: bool) -> Vec<f32> {
    let px = ORTHO / H as f32;
    let mut r = renderer(gpu, Antialiasing::Taa, Gtao::Off);
    r.taa_mut().tuning.object_motion = object_motion;
    r.set_camera_override(Some(ortho_camera(0.0)));
    // Drawn half way between ticks: the board's drawn position moves exactly a pixel a frame and
    // ends at tick `frames - 1` plus half a pixel.
    let end = (frames as f32 - 0.5) * px;
    let mut f0 = still_scene(1.0);
    f0.instances
        .extend(board(if moving { 0.0 } else { end }, true));
    r.apply(f0, 0.0);
    let mut img = Vec::new();
    for k in 1..=frames {
        if moving {
            r.apply(
                RenderFrame {
                    tick: k + 1,
                    t_s: k as f64 * DT,
                    dt_s: DT,
                    instances: board(k as f32 * px, false),
                    ..RenderFrame::default()
                },
                k as f64 * DT,
            );
        }
        img = r.capture_hdr(k as f64 * DT + 0.5 * DT).2;
    }
    img
}

#[test]
fn object_motion_reprojects_a_moving_board_exactly() {
    let Some(gpu) = gpu() else { return };
    let frames = 24u64;
    let still = board_run(&gpu, frames, false, true);
    let moving = board_run(&gpu, frames, true, true);
    let blind = board_run(&gpu, frames, true, false);
    // The board's inside at the end (3.2 x 2.4 m: about 51 x 38 pixels).
    let camera = ortho_camera(0.0);
    let vp = camera.proj(W as f32 / H as f32) * camera.view();
    let end = (frames as f32 - 0.5) * ORTHO / H as f32;
    let corner = |x: f32, y: f32| {
        let c = vp * glam::Vec4::new(-2.5 + end + x, 1.6 + y, 1.0, 1.0);
        (
            ((c.x / c.w * 0.5 + 0.5) * W as f32) as u32,
            ((0.5 - c.y / c.w * 0.5) * H as f32) as u32,
        )
    };
    let (x0, y0) = corner(-1.6, 1.2);
    let (x1, y1) = corner(1.6, -1.2);
    let (with, without) = (
        psnr(&still, &moving, x0 + 3..x1 - 3, y0 + 3..y1 - 3),
        psnr(&still, &blind, x0 + 3..x1 - 3, y0 + 3..y1 - 3),
    );
    eprintln!(
        "board ({x0}, {y0})-({x1}, {y1}): {with:.1} dB with object motion, {without:.1} dB without"
    );
    assert!(with > 45.0, "the moving board reprojected to {with:.1} dB");
    assert!(
        without < with - 10.0,
        "object motion changed nothing ({without:.1} dB without)"
    );
}

#[test]
fn the_entity_id_pass_is_not_jittered() {
    let Some(gpu) = gpu() else { return };
    let coverage = |aa: Antialiasing| {
        let mut r = renderer(&gpu, aa, Gtao::Off);
        r.apply(still_scene(1.0), 0.0);
        r.set_camera_override(Some(pocket_render::demo::aa_camera(0, false)));
        let mut all = Vec::new();
        // Several frames: each draws the id pass at another jitter phase.
        for f in 1..=6u32 {
            r.request_visible();
            let mut v = None;
            for k in 0..30 {
                let _ = r.capture_rgba(f64::from(f * 40 + k) * DT);
                if let Some(c) = r.take_visible() {
                    v = Some(c);
                    break;
                }
            }
            all.push(v.expect("the id pass answers"));
        }
        all
    };
    let off = coverage(Antialiasing::Off);
    let taa = coverage(Antialiasing::Taa);
    assert!(!off[0].is_empty());
    for (a, b) in off.iter().zip(&taa) {
        assert_eq!(a, b, "the id pass's coverage moved with TAA's jitter");
    }
}

#[test]
fn gtao_darkens_only_indirect_light() {
    let Some(gpu) = gpu() else { return };
    let image = |gtao: Gtao, ambient: f32| {
        let mut r = renderer(&gpu, Antialiasing::Msaa, gtao);
        r.apply(still_scene(ambient), 0.0);
        r.set_camera_override(Some(pocket_render::demo::aa_camera(0, false)));
        let mut img = Vec::new();
        for f in 1..=3 {
            img = r.capture_hdr(f64::from(f) * DT).2;
        }
        img
    };
    // The sun alone: nothing indirect to darken.
    let (off, on) = (
        image(Gtao::Off, 0.0),
        image(Gtao::On(AoNormals::Depth), 0.0),
    );
    assert_eq!(off, on, "GTAO changed a scene lit only directly");
    // Sky light: never brighter, darker in the creases.
    let (off, on) = (
        image(Gtao::Off, 1.0),
        image(Gtao::On(AoNormals::Depth), 1.0),
    );
    let mut darker = 0;
    for (a, b) in off.chunks(4).zip(on.chunks(4)) {
        for k in 0..3 {
            assert!(
                b[k] <= a[k] * 1.001 + 1e-6,
                "GTAO brightened {} to {}",
                a[k],
                b[k]
            );
        }
        if b[1] < a[1] * 0.97 {
            darker += 1;
        }
    }
    let share = darker as f64 / f64::from(W * H);
    eprintln!(
        "GTAO darkened {:.1}% of the pixels by 3% or more",
        share * 100.0
    );
    assert!(
        share > 0.02,
        "GTAO darkened only {:.2}% of the pixels",
        share * 100.0
    );
}

#[test]
fn camera_cuts_and_resizes_drop_the_history() {
    let Some(gpu) = gpu() else { return };
    let mut r = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
    r.apply(still_scene(1.0), 0.0);
    r.set_camera_override(Some(pocket_render::demo::aa_camera(0, false)));
    for f in 1..=5 {
        let _ = r.capture_rgba(f64::from(f) * DT);
    }
    assert_eq!(r.taa_mut().frames(), 5);
    // A slow pan keeps it.
    r.set_camera_override(Some(pocket_render::demo::aa_camera(30, true)));
    let _ = r.capture_rgba(6.0 * DT);
    assert_eq!(r.taa_mut().frames(), 6);
    // A cut drops it: this frame starts a new history.
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(30.0, 8.0, -20.0),
        Vec3::ZERO,
    )));
    let _ = r.capture_rgba(7.0 * DT);
    assert_eq!(r.taa_mut().frames(), 1, "a camera cut kept the history");
    let _ = r.capture_rgba(8.0 * DT);
    r.resize(W / 2, H / 2);
    let _ = r.capture_rgba(9.0 * DT);
    assert_eq!(r.taa_mut().frames(), 1, "a resize kept the history");
    // Turning TAA off and on starts over too.
    r.set_antialiasing(Antialiasing::Msaa);
    let _ = r.capture_rgba(10.0 * DT);
    r.set_antialiasing(Antialiasing::MsaaTaa);
    let _ = r.capture_rgba(11.0 * DT);
    assert_eq!(r.taa_mut().frames(), 1);
}

/// Luminance of an HDR pixel.
fn lum(p: &[f32]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// The pixels where `a` and `b` differ by more than 2% in luminance, and every pixel within 2 of
/// them does too (the inside of what one image has and the other has not).
fn inside_of_difference(a: &[f32], b: &[f32]) -> Vec<bool> {
    let raw: Vec<bool> = a
        .chunks(4)
        .zip(b.chunks(4))
        .map(|(p, q)| (lum(p) - lum(q)).abs() > 0.02 * lum(p).max(1e-3))
        .collect();
    let (w, h) = (W as i32, H as i32);
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            (-2..=2).all(|dy| {
                (-2..=2).all(|dx| {
                    let (u, v) = (x + dx, y + dy);
                    u >= 0 && v >= 0 && u < w && v < h && raw[(v * w + u) as usize]
                })
            })
        })
        .collect()
}

/// The sea's level in [`sunk_scene`].
const SEA: f32 = 0.6;

/// The moving cube of `still_scene`, shrunk and sunk under the sea, at tick `k` (sliding 4 cm a
/// tick: never visible).
fn sunk_cube(k: u64) -> InstanceUpdate {
    InstanceUpdate {
        id: pocket_render::demo::AA_MOVING[0],
        pose: Some(Pose {
            position: [3.0 - 0.04 * k as f32, 0.25, 2.6],
            rotation: Quat::IDENTITY.to_array(),
            scale: [0.4; 3],
        }),
        look: None,
        anim: None,
    }
}

/// `still_scene` with its moving cube sunk ([`sunk_cube`]) and, with `sea`, a calm sea at [`SEA`]
/// over the ground and the cube; the walled corner and the spheres stand in it. Without the
/// fence, the wires and the chain link: thinner than a pixel, they share pixels with the sea.
fn sunk_scene(sea: bool) -> RenderFrame {
    let mut f = still_scene(1.0);
    f.instances.retain(|i| !matches!(i.id, 10..=65 | 85));
    for i in &mut f.instances {
        if i.id == pocket_render::demo::AA_MOVING[0] {
            i.pose = sunk_cube(0).pose;
        }
    }
    f.sea = Some(sea.then(|| SeaView {
        level: SEA,
        waves: vec![],
    }));
    f
}

/// The image after `frames` frames of [`sunk_scene`] from the still camera, the sunk cube sliding
/// when `slide`.
fn sunk_run(r: &mut Renderer, sea: bool, slide: bool, frames: u64) -> Vec<f32> {
    r.apply(sunk_scene(sea), 0.0);
    r.set_camera_override(Some(pocket_render::demo::aa_camera(0, false)));
    let mut img = Vec::new();
    for k in 1..=frames {
        if slide {
            r.apply(
                RenderFrame {
                    tick: k + 1,
                    t_s: k as f64 * DT,
                    dt_s: DT,
                    instances: vec![sunk_cube(k)],
                    ..RenderFrame::default()
                },
                k as f64 * DT,
            );
        }
        img = r.capture_hdr(k as f64 * DT + 0.5 * DT).2;
    }
    img
}

/// The sea writes depth over the ground and whatever is sunk in it, so it must also write the
/// opaque pass's extra targets: otherwise each sea pixel keeps the indirect share and the object
/// motion of the surface under it, GTAO darkens the water and TAA moves it with a hidden object.
#[test]
fn the_sea_hides_what_lies_under_it() {
    let Some(gpu) = gpu() else { return };
    let msaa = |gtao: Gtao, sea: bool| {
        let mut r = renderer(&gpu, Antialiasing::Msaa, gtao);
        sunk_run(&mut r, sea, false, 3)
    };
    let dry = msaa(Gtao::Off, false);
    let wet = msaa(Gtao::Off, true);
    let ocean = inside_of_difference(&dry, &wet);
    let n = ocean.iter().filter(|&&o| o).count();
    eprintln!("{n} pixels inside the sea");
    assert!(n > (W * H / 8) as usize, "the sea covers only {n} pixels");
    // GTAO darkens indirect light only, and the sea has none.
    let shaded = msaa(Gtao::On(AoNormals::Depth), true);
    let changed = |a: &[f32], b: &[f32]| {
        (0..(W * H) as usize)
            .filter(|&i| {
                let (p, q) = (lum(&a[i * 4..i * 4 + 3]), lum(&b[i * 4..i * 4 + 3]));
                ocean[i] && (p - q).abs() > 1e-4 * q.max(1e-4)
            })
            .count()
    };
    let darkened = changed(&shaded, &wet);
    eprintln!("GTAO changed {darkened} of them");
    assert_eq!(darkened, 0, "GTAO changed {darkened} pixels of the sea");
    // A cube sliding under the water moves nothing on it.
    let taa = |object_motion: bool| {
        let mut r = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
        r.taa_mut().tuning.object_motion = object_motion;
        sunk_run(&mut r, true, true, 40)
    };
    let moved = changed(&taa(true), &taa(false));
    eprintln!("object motion changed {moved} of them");
    assert_eq!(
        moved, 0,
        "a hidden cube's motion moved {moved} pixels of the sea"
    );
}

/// PSNR of an 8-bit image against a reference in the same units (0 to 255).
fn psnr8(a: &[u8], reference: &[f64]) -> f64 {
    let mut sum = 0.0;
    let mut n = 0u64;
    for (i, (&p, &q)) in a.iter().zip(reference).enumerate() {
        if i % 4 != 3 {
            let d = f64::from(p) - q;
            sum += d * d;
            n += 1;
        }
    }
    10.0 * (255.0f64 * 255.0 / (sum / n.max(1) as f64).max(1e-12)).log10()
}

/// A one-shot capture (an agent's `capture`) under TAA draws a whole jitter cycle from a reset:
/// TAA's first frame alone is one jittered sample, below multisampling; the settled capture is
/// above it, and the same capture taken twice is the same image.
#[test]
fn a_still_capture_settles_taa() {
    let Some(gpu) = gpu() else { return };
    let camera = pocket_render::demo::aa_camera(0, false);
    let t = 0.5;
    let fresh = |aa: Antialiasing| {
        let mut r = renderer(&gpu, aa, Gtao::Off);
        r.apply(still_scene(1.0), 0.0);
        r.set_camera_override(Some(camera));
        r
    };
    // The reference: the mean of 64 jittered frames as displayed.
    let mut jittered = fresh(Antialiasing::Taa);
    jittered.taa_mut().mode = pocket_render::taa::TaaMode::Jittered;
    let mut reference = vec![0.0f64; (W * H * 4) as usize];
    for _ in 0..64 {
        for (m, v) in reference.iter_mut().zip(jittered.capture_rgba(t).2) {
            *m += f64::from(v) / 64.0;
        }
    }
    let msaa = psnr8(&fresh(Antialiasing::Msaa).capture_rgba(t).2, &reference);
    let first = psnr8(&fresh(Antialiasing::Taa).capture_rgba(t).2, &reference);
    let mut taa = fresh(Antialiasing::Taa);
    let still = taa.capture_still(t).2;
    let settled = psnr8(&still, &reference);
    eprintln!(
        "against 64 jittered frames: msaa {msaa:.2} dB, taa's first frame {first:.2} dB, a still \
         capture {settled:.2} dB"
    );
    assert!(first < msaa, "TAA's first frame is not below MSAA here");
    assert!(
        settled > msaa + 1.0,
        "a still capture with TAA reached {settled:.2} dB (MSAA {msaa:.2} dB)"
    );
    assert_eq!(
        taa.capture_still(t).2,
        still,
        "a second still capture differs"
    );
}
