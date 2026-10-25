//! TAA and GTAO (docs/spec/taa-gtao.md) on a headless GPU; skips without one.
//!
//! - Reprojection is exact: an orthographic camera that pans whole pixels a frame reprojects every
//!   history texel onto a texel centre, so TAA's image after the pan equals TAA's image from a still
//!   camera at the pan's end, away from the edge new content enters. The same holds for an object
//!   moving whole pixels a frame under a still camera, through the forward shader's object motion,
//!   and for a skinned part its bone slides, through last frame's skinned vertices; without them
//!   (the switches the checks flip) the history comes from the wrong place. With one sample and
//!   with four.
//! - A still camera's history stays near a supersampled reference; with four samples it ends
//!   closer to it than multisampling alone.
//! - The entity-id pass is never jittered: with TAA its coverage equals the coverage without.
//! - GTAO darkens only indirect light: under the sun alone it changes nothing; with sky light it
//!   never brightens a pixel and darkens the creases. Its two normal sources agree.
//! - The sea hides what lies under it: GTAO and a sunk object's motion leave it untouched.
//! - Splats behind meshes do not flicker under TAA: they test against an unjittered depth that holds
//!   what the opaque pass holds (alpha-tested holes, the sea).
//! - A one-shot capture under TAA is settled; camera cuts and resizes drop the history.

use amoris_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame, SeaView, SplatView};
use amoris_assets::mesh::{
    AnimationClip, Channel, ChannelPath, ImageData, Interpolation, MaterialData, MeshData,
    ModelAsset, NodeData, SkeletonNode, SkinAsset, SkinWeights, Vertex,
};
use amoris_render::demo::{AA_MODEL, aa_model, aa_scene};
use amoris_render::splat::{RawSplat, SplatCloud, SplatRaster};
use amoris_render::{Antialiasing, AoNormals, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::{Quat, Vec3};

const W: u32 = 256;
const H: u32 = 144;
const DT: f64 = 1.0 / 60.0;
/// The orthographic view's height in metres: a pixel is `ORTHO / H` metres.
const ORTHO: f32 = 9.0;

// A lost device or a validation error fails the case with its cause, never as a wrong image. The
// cases run in parallel: each keeps at most three renderers alive, and a renderer takes one
// timestamp query set, one of the 32 counter sample buffers Metal gives a process (profiler.rs).
fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless(BackendChoice::from_env()).ok();
    if let Some(gpu) = &gpu {
        gpu.device.set_device_lost_callback(|reason, message| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                panic!("TAA device lost: {reason:?}: {message}");
            }
        });
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|error| {
            panic!("TAA GPU validation failed: {error}");
        }));
    }
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
    // One sample, and four (the multisampled depth, motion and color resolved for TAA).
    for aa in [Antialiasing::Taa, Antialiasing::MsaaTaa] {
        whole_pixel_pan(&gpu, aa);
    }
}

fn whole_pixel_pan(gpu: &Gpu, aa: Antialiasing) {
    let frames = 24u32;
    // A still camera at the pan's end...
    let mut still = renderer(gpu, aa, Gtao::Off);
    still.apply(still_scene(1.0), 0.0);
    still.set_camera_override(Some(ortho_camera(frames as f32)));
    let mut a = Vec::new();
    for f in 1..=frames {
        a = still.capture_hdr(f64::from(f) * DT).2;
    }
    // The still camera's history is close to the plain mean of 64 jittered one-sample frames, a
    // supersampled reference (the clip box keeps it from being equal); a history that drifts
    // would not be.
    let mut jittered = renderer(gpu, Antialiasing::Taa, Gtao::Off);
    jittered.taa_mut().mode = amoris_render::taa::TaaMode::Jittered;
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
    eprintln!("{aa:?} still: {m:.1} dB against the mean of 64 jittered frames");
    assert!(
        m > 39.0,
        "{aa:?}: a still camera's history is {m:.1} dB from the jittered mean"
    );
    // ...and a camera panning one pixel a frame to it.
    let mut panning = renderer(gpu, aa, Gtao::Off);
    panning.apply(still_scene(1.0), 0.0);
    let mut b = Vec::new();
    for f in 1..=frames {
        panning.set_camera_override(Some(ortho_camera(f as f32)));
        b = panning.capture_hdr(f64::from(f) * DT).2;
    }
    assert_eq!(panning.taa_mut().frames(), frames, "the pan was not a cut");
    // New content enters at the right edge; the left keeps a margin for the clamped border.
    let p = psnr(&a, &b, 4..W - frames - 4, 4..H - 4);
    eprintln!("{aa:?} pan: {p:.1} dB against the still camera");
    assert!(
        p > 45.0,
        "{aa:?}: a whole-pixel pan reprojected to {p:.1} dB"
    );
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
fn board_run(
    gpu: &Gpu,
    aa: Antialiasing,
    frames: u64,
    moving: bool,
    object_motion: bool,
) -> Vec<f32> {
    let px = ORTHO / H as f32;
    let mut r = renderer(gpu, aa, Gtao::Off);
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
    // One sample, and four (the motion target multisampled and resolved).
    for aa in [Antialiasing::Taa, Antialiasing::MsaaTaa] {
        moving_board(&gpu, aa);
    }
}

fn moving_board(gpu: &Gpu, aa: Antialiasing) {
    let frames = 24u64;
    let still = board_run(gpu, aa, frames, false, true);
    let moving = board_run(gpu, aa, frames, true, true);
    let blind = board_run(gpu, aa, frames, true, false);
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
    eprintln!("{aa:?} board ({x0}, {y0})-({x1}, {y1}): {with:.1} dB, {without:.1} dB without");
    assert!(
        with > 45.0,
        "{aa:?}: the moving board reprojected to {with:.1} dB"
    );
    assert!(
        without < with - 10.0,
        "{aa:?}: object motion changed nothing ({without:.1} dB without)"
    );
}

#[test]
fn the_entity_id_pass_is_not_jittered() {
    let Some(gpu) = gpu() else { return };
    let coverage = |aa: Antialiasing| {
        let mut r = renderer(&gpu, aa, Gtao::Off);
        r.apply(still_scene(1.0), 0.0);
        r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
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
        r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
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
    r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    for f in 1..=5 {
        let _ = r.capture_rgba(f64::from(f) * DT);
    }
    assert_eq!(r.taa_mut().frames(), 5);
    // A slow pan keeps it.
    r.set_camera_override(Some(amoris_render::demo::aa_camera(30, true)));
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
        id: amoris_render::demo::AA_MOVING[0],
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
        if i.id == amoris_render::demo::AA_MOVING[0] {
            i.pose = sunk_cube(0).pose;
        }
    }
    f.sea = Some(sea.then(|| SeaView {
        level: SEA,
        waves: vec![],
        wind: None,
        look: None,
    }));
    f
}

/// The image after `frames` frames of [`sunk_scene`] from the still camera, the sunk cube sliding
/// when `slide`.
fn sunk_run(r: &mut Renderer, sea: bool, slide: bool, frames: u64) -> Vec<f32> {
    r.apply(sunk_scene(sea), 0.0);
    r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
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
    let mut r = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
    // Compare only the motion switch, with the same renderer, draw policy and jitter sequence.
    // Separate Auto policies can choose different prepass paths and perturb the sea's history.
    r.set_prepass(amoris_render::PrepassMode::Off);
    r.set_occlusion(amoris_render::OcclusionMode::Off);
    let mut taa = |object_motion: bool| {
        r.taa_mut().tuning.object_motion = object_motion;
        r.taa_mut().restart();
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
    let camera = amoris_render::demo::aa_camera(0, false);
    let t = 0.5;
    let fresh = |aa: Antialiasing| {
        let mut r = renderer(&gpu, aa, Gtao::Off);
        r.apply(still_scene(1.0), 0.0);
        r.set_camera_override(Some(camera));
        r
    };
    // The reference: the mean of 64 jittered frames as displayed.
    let mut jittered = fresh(Antialiasing::Taa);
    jittered.taa_mut().mode = amoris_render::taa::TaaMode::Jittered;
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

/// A wall of splats behind a cube and the alpha-tested chain-link panel, its foot under a calm sea,
/// the sky behind, from the still camera: the HDR images of frames 1 to `n`, with visible or
/// transparent splats, plus each frame's own TAA history when TAA is active. The history is the
/// opaque image before compositing: splats draw into a separate copy, never into this texture.
fn splat_frames(
    gpu: &Gpu,
    (aa, scale): (Antialiasing, f32),
    raster: SplatRaster,
    draw: bool,
    n: u32,
) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut wall = Vec::new();
    for j in 0..100 {
        for i in 0..100 {
            wall.push(RawSplat {
                position: [-2.8 + 0.04 * i as f32, 0.03 * j as f32, -1.6],
                scale: [0.025, 0.02, 0.004],
                rotation: [0.0, 0.0, 0.0, 1.0],
                color: [0.1, 0.9, 0.1],
                opacity: if draw { 0.95 } else { 0.0 },
            });
        }
    }
    let mut r = renderer(gpu, aa, Gtao::Off);
    r.set_render_scale(scale);
    r.splats
        .insert("wall", &SplatCloud::from_raw(&wall, 0, &[]));
    r.splats.raster = raster;
    let mut f = still_scene(1.0);
    let mut cube = f
        .instances
        .iter()
        .find(|i| i.id == 80)
        .cloned()
        .expect("the slab");
    cube.id = 999;
    cube.pose = Some(Pose {
        position: [-0.8, 1.4, -0.8],
        rotation: Quat::from_rotation_z(0.3).to_array(),
        scale: [0.9; 3],
    });
    let mut panel = f
        .instances
        .iter()
        .find(|i| i.id == 85)
        .cloned()
        .expect("the chain link");
    panel.pose = Some(Pose {
        position: [0.2, 2.1, -1.0],
        rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2).to_array(),
        scale: [2.4, 1.0, 1.6],
    });
    f.instances = vec![cube, panel];
    f.sea = Some(Some(SeaView {
        level: 0.5,
        waves: vec![],
        wind: None,
        look: None,
    }));
    f.splats = Some(vec![SplatView {
        id: 7,
        asset: "wall".into(),
        pose: Pose::default(),
        visible: true,
    }]);
    r.apply(f, 0.0);
    r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    let mut history = Vec::new();
    let images = (1..=n)
        .map(|k| {
            let image = r.capture_hdr(f64::from(k) * DT).2;
            if aa != Antialiasing::Msaa {
                history.push(splat_history(gpu, &mut r));
            }
            image
        })
        .collect();
    (images, history)
}

/// Reads the opaque TAA history after the frame. Comparing it with the composited image also
/// catches writing splats into the history itself: their measured contribution would disappear.
fn splat_history(gpu: &Gpu, r: &mut Renderer) -> Vec<f32> {
    let texture = r.taa_mut().output().expect("the frame ran TAA").0.clone();
    let row = (W * 8).div_ceil(256) * 256;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("TAA history readback"),
        size: u64::from(row * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the history readback completes");
    rx.recv()
        .expect("the mapping callback answers")
        .expect("history maps");
    let data = slice.get_mapped_range().expect("the mapped history");
    let image = (0..H)
        .flat_map(|y| {
            data[(y * row) as usize..(y * row + W * 8) as usize]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| amoris_assets::neural::f16_to_f32(u16::from_le_bytes([p[0], p[1]])))
        })
        .collect();
    drop(data);
    buffer.unmap();
    image
}

/// Splats are drawn after TAA with the unjittered camera, so they must be tested against an
/// unjittered depth too: against the jittered one, the edges where meshes cover them jump with the
/// jitter every frame (TAA cannot smooth them: splats never enter its history). That depth must
/// hold what the opaque pass holds: the chain link's holes, the sea.
#[test]
fn splats_behind_meshes_are_stable_under_taa() {
    let Some(gpu) = gpu() else { return };
    let n = 40u32;
    for raster in [SplatRaster::Quads, SplatRaster::Tiles] {
        let mut msaa_covered = 0;
        let mut near_splats = vec![false; (W * H) as usize];
        // Upscaled too (a render scale below 1): the splats and their depth stay at the output's
        // size, drawn over the upscaled history's copy.
        for (aa, scale) in [
            (Antialiasing::Msaa, 1.0),
            (Antialiasing::Taa, 1.0),
            (Antialiasing::MsaaTaa, 1.0),
            (Antialiasing::Taa, 0.5),
        ] {
            let (with, history) = splat_frames(&gpu, (aa, scale), raster, true, n);
            // Independently rendered TAA histories can diverge by a half-float step on Metal,
            // even with the original profiler. Measure compositing against the same frame's
            // opaque history, so that unrelated background drift cannot look like splat output.
            // MSAA has no history and retains its separately rendered transparent-splat control.
            let without = if aa == Antialiasing::Msaa {
                splat_frames(&gpu, (aa, scale), raster, false, n).0
            } else {
                history
            };
            // The splats' contribution to each frame.
            let c: Vec<Vec<f32>> = with
                .iter()
                .zip(&without)
                .map(|(a, b)| {
                    a.chunks(4)
                        .zip(b.chunks(4))
                        .map(|(p, q)| lum(p) - lum(q))
                        .collect()
                })
                .collect();
            let last = &c[n as usize - 1];
            let covered = last.iter().filter(|v| v.abs() > 0.05).count();
            assert!(
                covered > (W * H / 40) as usize,
                "{raster:?} {aa:?} x{scale}: the splats cover only {covered} pixels"
            );
            // Pixels whose contribution changed by more than 0.05 from one frame to the next, over
            // the last 8 frames (TAA has long converged; nothing moves). Upscaled, the shown value
            // must change too: under splats that hide it, the upscaled history moves a little from
            // frame to frame (its own stability: 3 pixels at 0.5), which changes the contribution,
            // not the image. Splats tested against a jittered depth change both: 787 pixels at
            // 0.5, 572 at 1 (docs/spec/taa-gtao.md 8).
            let shown: Vec<Vec<f32>> = with
                .iter()
                .map(|f| f.chunks(4).map(lum).collect())
                .collect();
            let changed = |shown_too: bool| -> usize {
                (n as usize - 8..n as usize)
                    .map(|k| {
                        (0..c[k].len())
                            .filter(|&i| {
                                (c[k][i] - c[k - 1][i]).abs() > 0.05
                                    && (!shown_too || (shown[k][i] - shown[k - 1][i]).abs() > 0.05)
                            })
                            .count()
                    })
                    .sum()
            };
            let flicker = changed(scale < 1.0);
            eprintln!(
                "{raster:?} {aa:?} x{scale}: {covered} pixels of splats, {flicker} flickering ({} \
                 by the contribution alone)",
                changed(false)
            );
            assert_eq!(
                flicker, 0,
                "{raster:?} {aa:?} x{scale}: splats flickered on {flicker} pixels of a still frame"
            );
            // As much of the wall shows as with multisampling (the panel's holes, not the sea).
            if aa == Antialiasing::Msaa {
                msaa_covered = covered;
                // Every pixel a splat touches with multisampling, and two around it.
                let (w, h) = (W as i32, H as i32);
                for i in 0..w * h {
                    let (x, y) = (i % w, i / w);
                    near_splats[i as usize] = (-2..=2).any(|dy| {
                        (-2..=2).any(|dx| {
                            let (u, v) = (x + dx, y + dy);
                            u >= 0 && v >= 0 && u < w && v < h && last[(v * w + u) as usize] != 0.0
                        })
                    });
                }
            }
            // Elsewhere TAA's image with splats is its image without them: the splats are drawn
            // over a copy of the history (`resolve_copy`) that equals it.
            let changed = last
                .iter()
                .zip(&near_splats)
                .filter(|&(d, &near)| !near && d.abs() > 1e-5)
                .count();
            assert_eq!(
                changed, 0,
                "{raster:?} {aa:?} x{scale}: drawing splats changed {changed} pixels away from them"
            );
            let off = covered.abs_diff(msaa_covered) as f64 / msaa_covered as f64;
            assert!(
                off < 0.03,
                "{raster:?} {aa:?} x{scale}: {covered} pixels of splats, {msaa_covered} with MSAA"
            );
        }
    }
}

/// The path [`skinned_board`] is registered under.
const SKINNED: &str = "test/skinned-board.glb";

/// A 3.2 x 2.4 m board of 8 x 6 checker squares facing +z, skinned to one bone whose clip
/// `slide` moves it along x from -0.75 m at 0 s by one orthographic pixel a tick (`px` metres in
/// `DT` seconds); its entity never moves.
fn skinned_board(px: f32) -> ModelAsset {
    let (tw, th) = (128u32, 96u32);
    let mut rgba8 = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        for x in 0..tw {
            let v: u32 = if (x / 16 + y / 16) % 2 == 0 { 230 } else { 30 };
            rgba8.extend_from_slice(&[v as u8, (v * 9 / 10) as u8, (v * 8 / 10) as u8, 255]);
        }
    }
    let vertex = |x: f32, y: f32, u: f32, v: f32| Vertex {
        position: [x, y, 0.0],
        normal: [0.0, 0.0, 1.0],
        uv: [u, v],
        tangent: [1.0, 0.0, 0.0, 1.0],
    };
    let mut mesh = MeshData::new(
        "board",
        vec![
            vertex(-1.6, -1.2, 0.0, 1.0),
            vertex(1.6, -1.2, 1.0, 1.0),
            vertex(1.6, 1.2, 1.0, 0.0),
            vertex(-1.6, 1.2, 0.0, 0.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    );
    mesh.material = Some(0);
    mesh.skin = Some(SkinWeights {
        joints: vec![[0; 4]; 4],
        weights: vec![[1.0, 0.0, 0.0, 0.0]; 4],
    });
    let speed = px / DT as f32;
    ModelAsset {
        meshes: vec![mesh],
        materials: vec![MaterialData {
            name: "checker".into(),
            base_color: [1.0; 4],
            roughness: 0.7,
            base_color_texture: Some(0),
            double_sided: true,
            ..MaterialData::default()
        }],
        images: vec![ImageData {
            name: "checker".into(),
            width: tw,
            height: th,
            rgba8,
            srgb: true,
        }],
        nodes: vec![NodeData {
            name: "board".into(),
            mesh: 0,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: Some(0),
        }],
        skeleton: vec![SkeletonNode {
            name: "bone".into(),
            parent: None,
            translation: [0.0; 3],
            rotation: Quat::IDENTITY.to_array(),
            scale: [1.0; 3],
        }],
        skins: vec![SkinAsset {
            joints: vec![0],
            inverse_bind: vec![glam::Mat4::IDENTITY.to_cols_array()],
        }],
        animations: vec![AnimationClip {
            name: "slide".into(),
            duration: 1.0,
            channels: vec![Channel {
                node: 0,
                path: ChannelPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![[-0.75, 0.0, 0.0, 0.0], [-0.75 + speed, 0.0, 0.0, 0.0]],
            }],
        }],
    }
}

/// The skinned board's entity at clip time `time` (and `rate` clip seconds a second), with its
/// look when `full` (the first frame).
fn skinned_entity(time: f32, rate: f32, full: bool) -> InstanceUpdate {
    InstanceUpdate {
        id: 950,
        pose: Some(Pose {
            position: [-2.5 + 0.75, 1.6, 1.0],
            rotation: Quat::IDENTITY.to_array(),
            scale: [1.0; 3],
        }),
        look: full.then(|| Look {
            mesh: SKINNED.into(),
            material: String::new(),
            color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.7,
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: false,
            visible: true,
        }),
        anim: Some(AnimView {
            clip: "slide".into(),
            time,
            rate,
            looped: false,
        }),
    }
}

/// TAA's image after `frames` frames of the skinned board sliding one pixel a frame (or standing
/// where the slide ends, its clip paused there).
fn skinned_run(gpu: &Gpu, frames: u64, moving: bool, skinned_motion: bool) -> Vec<f32> {
    let px = ORTHO / H as f32;
    let mut r = renderer(gpu, Antialiasing::Taa, Gtao::Off);
    r.add_model(SKINNED, &skinned_board(px));
    r.taa_mut().tuning.skinned_motion = skinned_motion;
    r.set_camera_override(Some(ortho_camera(0.0)));
    // Drawn half way between ticks: the clip shows tick k's time minus half a tick, so the board
    // moves exactly a pixel a frame and ends at tick `frames - 1` plus half a pixel.
    let end = (frames as f32 - 0.5) * DT as f32;
    let mut f0 = still_scene(1.0);
    f0.instances.push(if moving {
        skinned_entity(0.0, 1.0, true)
    } else {
        skinned_entity(end, 0.0, true)
    });
    r.apply(f0, 0.0);
    let mut img = Vec::new();
    for k in 1..=frames {
        if moving {
            r.apply(
                RenderFrame {
                    tick: k + 1,
                    t_s: k as f64 * DT,
                    dt_s: DT,
                    instances: vec![skinned_entity((k as f64 * DT) as f32, 1.0, false)],
                    ..RenderFrame::default()
                },
                k as f64 * DT,
            );
        }
        img = r.capture_hdr(k as f64 * DT + 0.5 * DT).2;
    }
    img
}

/// A skinned part's own motion (its vertices' last positions, skinning.rs `History`) reprojects
/// it: a board slid one pixel a frame by its bone, its entity still, ends as TAA's image of the
/// board standing there; without the skinned motion its history comes from the wrong place.
#[test]
fn skinned_motion_reprojects_a_sliding_board_exactly() {
    let Some(gpu) = gpu() else { return };
    let frames = 24u64;
    let still = skinned_run(&gpu, frames, false, true);
    let moving = skinned_run(&gpu, frames, true, true);
    let rigid = skinned_run(&gpu, frames, true, false);
    // The board's inside at the end: where the rigid board's check ends too.
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
        psnr(&still, &rigid, x0 + 3..x1 - 3, y0 + 3..y1 - 3),
    );
    eprintln!("skinned board ({x0}, {y0})-({x1}, {y1}): {with:.1} dB, {without:.1} dB rigid");
    // 45 to 47 dB on both GPUs and APIs (the rigid board: 51 to 53), 23 to 24 without.
    assert!(with > 40.0, "the skinned board reprojected to {with:.1} dB");
    assert!(
        without < with - 10.0,
        "skinned motion changed nothing ({without:.1} dB without)"
    );
}

/// The check scene's still view, its HDR image after `frames` frames of `aa`.
fn still_view(gpu: &Gpu, aa: Antialiasing, frames: u32) -> Vec<f32> {
    let mut r = renderer(gpu, aa, Gtao::Off);
    r.apply(still_scene(1.0), 0.0);
    r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    let mut img = Vec::new();
    for f in 1..=frames {
        img = r.capture_hdr(f64::from(f) * DT).2;
    }
    img
}

/// Multisampling with TAA converges past multisampling alone on the check scene's still view
/// (alpha-tested chain link, specular spheres, a fine checker): the four samples are jittered too,
/// and the history accumulates what they miss.
#[test]
fn msaa_with_taa_converges_beyond_msaa() {
    let Some(gpu) = gpu() else { return };
    // The reference: the mean of 64 jittered one-sample frames.
    let mut jittered = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
    jittered.taa_mut().mode = amoris_render::taa::TaaMode::Jittered;
    jittered.apply(still_scene(1.0), 0.0);
    jittered.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    let mut reference = vec![0.0f32; (W * H * 4) as usize];
    for f in 1..=64 {
        for (m, v) in reference
            .iter_mut()
            .zip(jittered.capture_hdr(f64::from(f) * DT).2)
        {
            *m += v / 64.0;
        }
    }
    let score = |img: &[f32]| psnr(img, &reference, 0..W, 0..H);
    let msaa = score(&still_view(&gpu, Antialiasing::Msaa, 1));
    let taa = score(&still_view(&gpu, Antialiasing::Taa, 32));
    let both = score(&still_view(&gpu, Antialiasing::MsaaTaa, 32));
    eprintln!("after 32 frames: msaa {msaa:.1} dB, taa {taa:.1} dB, msaa+taa {both:.1} dB");
    assert!(
        both > msaa + 3.0,
        "msaa+taa reached {both:.1} dB, msaa alone {msaa:.1} dB"
    );
}

/// GTAO's two normal sources agree: the normal target the forward shader writes (view space)
/// darkens the check scene where and as much as normals rebuilt from the depth do, seen from the
/// front and from the side (where view space is far from world space).
#[test]
fn gtao_normals_from_the_target_match_the_depth() {
    let Some(gpu) = gpu() else { return };
    let side = CameraState::look_at(Vec3::new(4.0, 2.6, 0.5), Vec3::new(-5.0, 0.8, -1.2));
    for camera in [amoris_render::demo::aa_camera(0, false), side] {
        gtao_normal_sources(&gpu, camera);
    }
}

fn gtao_normal_sources(gpu: &Gpu, camera: CameraState) {
    let image = |gtao: Gtao| {
        let mut r = renderer(gpu, Antialiasing::Msaa, gtao);
        r.apply(still_scene(1.0), 0.0);
        r.set_camera_override(Some(camera));
        let mut img = Vec::new();
        for f in 1..=3 {
            img = r.capture_hdr(f64::from(f) * DT).2;
        }
        img
    };
    let off = image(Gtao::Off);
    // What each takes away, per pixel.
    let darkening = |gtao: Gtao| -> Vec<f32> {
        image(gtao)
            .chunks(4)
            .zip(off.chunks(4))
            .map(|(p, q)| lum(q) - lum(p))
            .collect()
    };
    let depth = darkening(Gtao::On(AoNormals::Depth));
    let target = darkening(Gtao::On(AoNormals::Target));
    let total: f32 = depth.iter().map(|d| d.abs()).sum();
    let apart: f32 = depth.iter().zip(&target).map(|(a, b)| (a - b).abs()).sum();
    let ratio = target.iter().sum::<f32>() / depth.iter().sum::<f32>();
    eprintln!(
        "the target's darkening is {ratio:.3} times the depth's, {:.1}% of it apart",
        100.0 * apart / total
    );
    assert!(
        (0.8..1.25).contains(&ratio) && apart < 0.5 * total,
        "the normal target darkens {ratio:.3} times as much, {:.1}% apart",
        100.0 * apart / total
    );
}

// --- Upscaling (a render scale below 1; docs/spec/taa-gtao.md 4.1) ----------------------------

/// The mean of `n` jittered one-sample frames of the check scene's still view at the output's size:
/// a supersampled reference.
fn jittered_mean(gpu: &Gpu, n: u32) -> Vec<f32> {
    let mut jittered = renderer(gpu, Antialiasing::Taa, Gtao::Off);
    jittered.taa_mut().mode = amoris_render::taa::TaaMode::Jittered;
    jittered.apply(still_scene(1.0), 0.0);
    jittered.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    let mut mean = vec![0.0f32; (W * H * 4) as usize];
    for f in 1..=n {
        for (m, v) in mean
            .iter_mut()
            .zip(jittered.capture_hdr(f64::from(f) * DT).2)
        {
            *m += v / n as f32;
        }
    }
    mean
}

/// The check scene's still view at render scale `scale` with `aa`: the HDR images after each of
/// `frames` frames.
fn scaled_view(gpu: &Gpu, aa: Antialiasing, scale: f32, frames: u32) -> Vec<Vec<f32>> {
    let mut r = renderer(gpu, aa, Gtao::Off);
    r.set_render_scale(scale);
    r.apply(still_scene(1.0), 0.0);
    r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    (1..=frames)
        .map(|f| {
            let (w, h, img) = r.capture_hdr(f64::from(f) * DT);
            assert_eq!((w, h), (W, H), "the image is the output's size");
            img
        })
        .collect()
}

/// Below a render scale of 1 the opaque pass draws smaller and TAA reconstructs the output: on the
/// still check scene the history gains on its first frame (one jittered sample of the smaller
/// image, reconstructed) as each output pixel accumulates the samples that fall near it, and ends
/// close to TAA at the output's size.
#[test]
fn upscaling_converges_on_a_still_scene() {
    let Some(gpu) = gpu() else { return };
    let reference = jittered_mean(&gpu, 64);
    let score = |img: &[f32]| psnr(img, &reference, 0..W, 0..H);
    let full = score(&scaled_view(&gpu, Antialiasing::Taa, 1.0, 64)[63]);
    let scores: Vec<(f32, f64, f64)> = [0.5f32, 0.67]
        .into_iter()
        .map(|scale| {
            let frames = scaled_view(&gpu, Antialiasing::Taa, scale, 64);
            let (first, last) = (score(&frames[0]), score(&frames[63]));
            eprintln!(
                "scale {scale}: first frame {first:.1} dB, frame 64 {last:.1} dB (scale 1: \
                 {full:.1})"
            );
            (scale, first, last)
        })
        .collect();
    for (scale, first, last) in scores {
        assert!(
            last > first + UPSCALE_GAIN_DB,
            "scale {scale}: frame 64 at {last:.1} dB, its first frame {first:.1} dB"
        );
        assert!(
            last > full - UPSCALE_LOSS_DB[usize::from(scale > 0.6)],
            "scale {scale}: frame 64 at {last:.1} dB, scale 1 at {full:.1} dB"
        );
    }
}

/// How much the upscaled history gains over its first frame, at least; how far below TAA at the
/// output's size it may end at 0.5 and at 0.67. Measured with the wider box where nothing moved
/// (`TaaTuning::upscale_still_gamma`, on since 2026-10-16): 9.6 to 10.0 dB gained at 0.5 and 10.4 to
/// 10.7 at 0.67, 3.4 to 3.7 and 1.2 to 2.0 dB lost (both GPUs and APIs). A reconstruction reading
/// the jitter with the wrong sign gains 0.7 to 2.4 dB; a share not weighted by the samples gains 5.0
/// to 5.2 dB at 0.5 and loses 8.1 to 8.4, which the earlier bounds (4.5 dB gained, 9.5 and 7.5
/// lost) let pass once the box widened (docs/spec/taa-gtao.md 8).
const UPSCALE_GAIN_DB: f64 = 7.5;
const UPSCALE_LOSS_DB: [f64; 2] = [6.0, 4.0];

/// The scale's own state: the opaque pass's size, the mode it promotes, history resets when the
/// scale or the output's size changes, and a return to 1 that draws as an unscaled renderer with
/// the same output-size history. Resizing can change the retained LOD through its hysteresis.
#[test]
fn render_scale_changes_reset_and_return() {
    let Some(gpu) = gpu() else { return };
    let mut r = renderer(&gpu, Antialiasing::Msaa, Gtao::Off);
    r.apply(still_scene(1.0), 0.0);
    r.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    let _ = r.capture_rgba(DT);
    let mut unscaled = renderer(&gpu, Antialiasing::Msaa, Gtao::Off);
    unscaled.apply(still_scene(1.0), 0.0);
    unscaled.set_camera_override(Some(amoris_render::demo::aa_camera(0, false)));
    let _ = unscaled.capture_rgba(DT);
    // Below 1: TAA at the same samples, the opaque pass at the scale's size.
    r.set_render_scale(0.5);
    assert_eq!(r.antialiasing(), Antialiasing::MsaaTaa);
    assert_eq!(r.scene_size(), (W / 2, H / 2));
    for f in 2..=6 {
        let (w, h, _) = r.capture_rgba(f64::from(f) * DT);
        assert_eq!((w, h), (W, H));
    }
    assert_eq!(r.taa_mut().frames(), 5);
    assert_eq!(r.last.render_scale, 0.5);
    // Another scale drops the history (the reconstruction and the jitter cycle differ).
    r.set_render_scale(0.67);
    assert_eq!(r.scene_size(), (172, 96));
    let _ = r.capture_rgba(7.0 * DT);
    assert_eq!(r.taa_mut().frames(), 1, "a scale change kept the history");
    let _ = r.capture_rgba(8.0 * DT);
    // A resize keeps the scale and drops the history.
    r.resize(W / 2, H / 2);
    unscaled.resize(W / 2, H / 2);
    assert_eq!(r.scene_size(), (86, 48));
    let _ = r.capture_rgba(9.0 * DT);
    let _ = unscaled.capture_rgba(9.0 * DT);
    assert_eq!(r.taa_mut().frames(), 1, "a resize kept the history");
    r.resize(W, H);
    unscaled.resize(W, H);
    // Out of range: clamped.
    r.set_render_scale(0.01);
    assert_eq!(r.render_scale(), 0.25);
    assert_eq!(r.scene_size(), (64, 36));
    // Back at 1: the mode asked for, at the output's size, drawing what it drew before.
    r.set_render_scale(1.0);
    assert_eq!(r.antialiasing(), Antialiasing::Msaa);
    assert_eq!(r.scene_size(), (W, H));
    assert_eq!(
        r.capture_rgba(DT).2,
        unscaled.capture_rgba(DT).2,
        "scale 1 differs from the unscaled resize control"
    );
}

/// The entity-id pass stays at the output's size and unjittered below a render scale of 1: its
/// coverage and a pick are what they are at scale 1. A still capture is the output's size, settles
/// past TAA's first upscaled frame and repeats exactly.
#[test]
fn picking_and_captures_work_below_scale_one() {
    let Some(gpu) = gpu() else { return };
    let camera = amoris_render::demo::aa_camera(0, false);
    let fresh = |scale: f32| {
        let mut r = renderer(&gpu, Antialiasing::Taa, Gtao::Off);
        r.set_render_scale(scale);
        r.apply(still_scene(1.0), 0.0);
        r.set_camera_override(Some(camera));
        r
    };
    let ids = |r: &mut Renderer| {
        r.request_visible();
        for k in 0..30 {
            let _ = r.capture_rgba(f64::from(k + 1) * DT);
            if let Some(c) = r.take_visible() {
                return c;
            }
        }
        panic!("the id pass did not answer");
    };
    let pick = |r: &mut Renderer, x: u32, y: u32| {
        r.request_pick(x, y);
        for k in 0..30 {
            let _ = r.capture_rgba(f64::from(k + 40) * DT);
            if let Some(p) = r.take_pick() {
                return p;
            }
        }
        panic!("the pick did not answer");
    };
    let (mut full, mut half) = (fresh(1.0), fresh(0.5));
    let a = ids(&mut full);
    assert!(!a.is_empty());
    assert_eq!(
        a,
        ids(&mut half),
        "the id pass's coverage changed with the scale"
    );
    for (x, y) in [(W / 2, H / 2), (W / 4, H / 3), (3 * W / 4, 2 * H / 3)] {
        assert_eq!(
            pick(&mut full, x, y),
            pick(&mut half, x, y),
            "the pick at ({x}, {y}) changed with the scale"
        );
    }
    // A still capture: the output's size, a jitter cycle of the longer sequence, repeatable.
    let reference: Vec<f64> = {
        let mut jittered = fresh(1.0);
        jittered.taa_mut().mode = amoris_render::taa::TaaMode::Jittered;
        let mut m = vec![0.0f64; (W * H * 4) as usize];
        for _ in 0..64 {
            for (m, v) in m.iter_mut().zip(jittered.capture_rgba(0.5).2) {
                *m += f64::from(v) / 64.0;
            }
        }
        m
    };
    let first = psnr8(&fresh(0.5).capture_rgba(0.5).2, &reference);
    let mut r = fresh(0.5);
    let (w, h, still) = r.capture_still(0.5);
    assert_eq!((w, h), (W, H));
    let settled = psnr8(&still, &reference);
    eprintln!("scale 0.5: first frame {first:.2} dB, a still capture {settled:.2} dB");
    assert!(
        settled > first + 2.0,
        "a still capture at scale 0.5 reached {settled:.2} dB, its first frame {first:.2} dB"
    );
    assert_eq!(
        r.capture_still(0.5).2,
        still,
        "a second still capture differs"
    );
}
